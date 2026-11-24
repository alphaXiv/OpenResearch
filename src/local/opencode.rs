//! opencode bootstrap for `orx up` — binary discovery, per-session config +
//! playbook written into the session's worktree, spawn + health check, and the
//! shared `AgentHost` handle the axum server holds (`Arc<AgentHost>` in state).
//!
//! One opencode serve child **per chat session**, cwd = that session's private
//! worktree (see `git::ensure_session_worktree`) so parallel agents never share
//! a checkout. Env is inherited (that's where ANTHROPIC_API_KEY /
//! OPENROUTER_API_KEY live — opencode auto-detects providers from env).
//! Each server is kept (`crate::keep`): a restart hands it to the next `orx up`, which adopts it;
//! a quit stops it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::keep::KeptHandle;

use crate::error::{anyhow, Context, Result};
use crate::local::git;
use crate::local::harness::PermissionMode;
use crate::local::model::LocalProject;
use crate::local::native_store::{self, NativeStore};
use crate::store;

#[path = "opencode_runtime.rs"]
mod runtime;
pub(crate) use runtime::{
    prepare_database, resolve_binary, resolve_binary_at, start_server, user_binary, AgentEndpoint,
    ResolvedBinary, ServerChild,
};

/// The harness id kept servers are recorded under.
const HARNESS: &str = "opencode";

/// Playbook path inside the session worktree; opencode re-reads it every turn,
/// so rewriting the file retargets a running server without a restart.
pub(crate) const PLAYBOOK_REL: &str = ".openresearch/agent/autoresearch-local.md";

/// Image and path details the playbook sends the agent to, written beside it.
const CHAT_FORMAT_REL: &str = ".openresearch/agent/chat-format.md";
pub(super) const CHAT_FORMAT: &str = include_str!("../../agent-playbook/chat-format.md");

const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const COMPACTION_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a kept server may take to answer the server adopting it.
const ADOPT_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct ResolvedRuntime {
    pub binary: ResolvedBinary,
    pub database: PathBuf,
    pub store: NativeStore,
}

/// Every `opencode` install, in preference order. The native installer writes
/// `~/.opencode/bin`, which a stale npm-style `opencode` shim earlier on PATH
/// otherwise hides.
pub(crate) fn opencode_candidates() -> Vec<PathBuf> {
    let own = crate::config::home_dir().map(|home| home.join(".opencode").join("bin"));
    // Resolved and de-duplicated: the native install is both on PATH and in its
    // drop directory, and probing that one binary twice costs a 15s timeout.
    crate::local::harness::unique_bins(
        crate::local::shell_env::installs("opencode", own)
            .into_iter()
            .map(|path| crate::paths::canonicalize(&path).unwrap_or(path))
            .collect(),
    )
}

pub(crate) fn not_found() -> crate::error::Error {
    anyhow!(
        "opencode not found (checked PATH and ~/.opencode/bin/opencode). \
         Install OpenCode from OpenResearch's coding agent settings."
    )
}

/// orx's own copy, else `opencode` on PATH, else the installer's default drop location.
pub fn find_opencode() -> Result<PathBuf> {
    crate::local::harness::managed::installed("opencode")
        .map(|(bin, _)| bin)
        .or_else(|| opencode_candidates().into_iter().next())
        .ok_or_else(not_found)
}

/// Ask the OS for a free loopback port (bind :0, read it back, release).
pub(crate) fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| anyhow!("Could not pick a free port: {}", e))?;
    Ok(listener.local_addr()?.port())
}

/// Where the spawned server's stdout/stderr land (startup diagnostics).
pub fn agent_log_path() -> PathBuf {
    store::data_dir().join("agent-opencode.log")
}

/// Chat-server config; native questions are answered through the dashboard.
/// With orx's MCP server present, native sub-agents are off: delegation goes
/// through orx, which can reach any model.
fn opencode_config_json(
    model: Option<&str>,
    instructions: &str,
    mcp: Option<&crate::local::chat::delegation::McpServer>,
) -> Result<String> {
    let mut cfg = json!({
        "$schema": "https://opencode.ai/config.json",
        "permission": {
            "edit": "allow",
            "bash": "allow",
            "webfetch": "allow",
            "websearch": "allow",
            "read": "allow",
            "glob": "allow",
            "grep": "allow",
            "task": "allow",
            "skill": "allow",
            "lsp": "allow",
            "doom_loop": "allow",
            "external_directory": "allow",
        },
        "instructions": [instructions],
        // A name of orx's own keeps a user's `alphaxiv` server from merging into this one.
        "mcp":{"orx_alphaxiv":{"type":"local", "command":[crate::paths::spawnable_exe()?, "alphaxiv-mcp"], "timeout":300000}},
    });
    if let Some(model) = model {
        cfg["model"] = json!(model);
    }
    cfg["permission"]["orx_alphaxiv_*"] = json!("allow");
    for tool in crate::commands::alphaxiv_mcp::WRITE_TOOLS {
        cfg["permission"][format!("orx_alphaxiv_{tool}")] = json!("ask");
    }
    if let Some(server) = mcp {
        // Side chats and delegates share this file with their parent's
        // worktree, so it names the env vars and each chat's serve child
        // supplies its own values (OpenCode substitutes `{env:…}`).
        let environment: serde_json::Map<String, Value> = server
            .env
            .iter()
            .map(|(name, _)| (name.to_string(), json!(format!("{{env:{name}}}"))))
            .collect();
        cfg["mcp"]["orx"] = json!({
            "type": "local",
            "command": [server.command.as_str(), server.args[0]],
            "environment": environment,
            "enabled": true,
            "timeout": crate::local::chat::delegation::TOOL_TIMEOUT_SECONDS * 1000,
        });
        cfg["permission"]["task"] = json!("deny");
    }
    Ok(serde_json::to_string_pretty(&cfg)?)
}

/// The research playbook: durable project context and skill routing for `orx up`.
/// The playbook template — a literal, GitHub-readable markdown file. Rendered
/// by [`playbook_md`]: the leading HTML comment is stripped and `{token}`
/// placeholders are substituted (project facts, state, skills, and the compute default).
const SYSTEM_PROMPT: &str = include_str!("../../SYSTEM_PROMPT.md");

#[derive(Default)]
pub(super) struct ProjectState {
    experiments: usize,
    runs: usize,
    active_runs: usize,
    compute: Option<super::ComputeTarget>,
}

impl ProjectState {
    fn load(project_id: &str) -> Result<Self> {
        let store = store::Store::open()?;
        let experiments = store.list_experiments_by_project(project_id)?.len();
        let runs = store.list_runs_by_project(project_id)?;
        let active_runs = runs
            .iter()
            .filter(|run| matches!(run.status.as_str(), "starting" | "running"))
            .count();
        Ok(Self {
            experiments,
            runs: runs.len(),
            active_runs,
            compute: super::compute_default(&store, project_id)?,
        })
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn project_state_md(project: &LocalProject, state: &ProjectState) -> String {
    if project.is_scratch() {
        return "This chat has **no project** yet: OpenResearch keeps its repository in a \
                temporary folder of its own, and deleting the chat deletes it. When the work \
                is worth keeping, or the user asks to make it a project, run \
                `orx project create --name \"<name>\"` (add `--path <folder>` for a folder \
                other than `~/OpenResearch/<name>`). The user reviews the name and folder in \
                the chat and creates it; the repository then moves there with your commits, \
                and this chat continues in that project with its worktree as it was."
            .to_string();
    }
    let has_run_command = project
        .run_command
        .as_deref()
        .is_some_and(|command| !command.trim().is_empty());
    let run_command = if has_run_command {
        "The fixed run command is configured."
    } else {
        "No fixed run command is configured."
    };

    if state.experiments == 0 && state.runs == 0 {
        let fresh_run_command = if has_run_command {
            "the fixed run command is configured."
        } else {
            "no fixed run command is configured."
        };
        return format!(
            "This is a fresh project: **0 experiments and 0 runs**. The experiment tree is empty \
             and {fresh_run_command}"
        );
    }

    let active = if state.active_runs == 0 {
        "no active runs".to_string()
    } else {
        format!("{} active", plural(state.active_runs, "run", "runs"))
    };
    format!(
        "This project currently has **{}** and **{}** ({active}). {run_command} This is an \
         orientation snapshot; use `orx` when you need live details.",
        plural(state.experiments, "experiment", "experiments"),
        plural(state.runs, "run", "runs"),
    )
}

pub(super) fn playbook_md(project: &LocalProject, state: &ProjectState) -> String {
    let id = &project.id;
    let name = &project.name;
    let publication_line = if project.github_enabled() {
        "- GitHub publication: enabled for experiment visibility; never used for compute transport"
    } else {
        "- GitHub publication: disabled"
    };
    let artifacts = super::files::files_dir(project)
        .to_string_lossy()
        .into_owned();
    let paper_line = project.paper_id.as_deref().map_or(String::new(), |p| {
        let source = if crate::client::is_arxiv_id(crate::client::versionless_id(p)) {
            format!("arXiv {p} (https://arxiv.org/abs/{p})")
        } else {
            format!("{p} ({})", crate::commands::paper::alphaxiv_paper_url(p))
        };
        format!(
            "- Paper: {source} — the paper this project starts from; `orx paper {p}` fetches \
             its report\n"
        )
    });
    // Read fresh on every playbook rewrite, which reaches a live agent only on its
    // next respawn or thread start. `exp run` resolves the default at launch, and
    // the agent is told to omit `--backend`, so a stale prompt still launches there.
    let compute_default = state
        .compute
        .as_ref()
        .map_or(("local".to_string(), None), |target| target.value.clone());
    let compute_default_source = match &state.compute {
        Some(target) if target.project => "this project's default",
        Some(_) => "the machine's default, which this project follows",
        None => "the default `local` backend",
    };
    let (compute_backend, compute_flavor) = &compute_default;
    let flavor_part = compute_flavor
        .as_ref()
        .map_or(String::new(), |flavor| format!(" (`--flavor {flavor}`)"));
    let compute_bullet = format!(
        "- Compute: default target **{compute_backend}**{flavor_part} — \
         {compute_default_source}; load **`orx-compute`** and read `orx compute instructions show` before configuring or launching"
    );
    let folder_line = if project.is_scratch() {
        String::new()
    } else {
        format!(
            "- Project folder: `{}`, the user's own checkout; you work in your worktree\n",
            project.repo_path
        )
    };
    let project_state = project_state_md(project, state);
    let template = SYSTEM_PROMPT
        .split_once("-->\n\n")
        .map(|(_, rest)| rest)
        .unwrap_or(SYSTEM_PROMPT);
    template
        .replace("{name}", name)
        .replace("{id}", id)
        .replace("{folder_line}", &folder_line)
        .replace("{publication_line}", publication_line)
        .replace("{paper_line}", &paper_line)
        .replace("{compute_bullet}", &compute_bullet)
        .replace("{artifacts}", &artifacts)
        .replace("{project_state}", &project_state)
        .replace("{chat_format}", CHAT_FORMAT_REL)
}

/// Keep the files we drop into the checkout out of `git status` / accidental
/// commits via the local-only `.git/info/exclude` (never touches tracked
/// files or the repo's own `.gitignore`). Takes the **hub clone** path — its
/// `.git/info/exclude` is shared by every session worktree (a worktree's own
/// `.git` is just a pointer file). Best-effort.
fn exclude_agent_files(hub: &Path) {
    let Ok(git_dir) = git::common_git_dir(hub) else {
        return;
    };
    let path = git_dir.join("info").join("exclude");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<&str> = [
        "opencode.json",
        ".openresearch/",
        ".claude/skills/",
        ".opencode/skills/",
        ".agents/skills/",
        ".agents/hooks.json",
        ".cursor/skills/",
        ".orx/latex-templates/",
    ]
    .into_iter()
    .filter(|entry| !existing.lines().any(|l| l.trim() == *entry))
    .collect();
    if missing.is_empty() {
        return;
    }
    let mut block = String::new();
    if !existing.is_empty() && !existing.ends_with('\n') {
        block.push('\n');
    }
    for entry in missing {
        block.push_str(entry);
        block.push('\n');
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, block.as_bytes()));
}

/// Ensure the project's hub clone and this session's private worktree exist,
/// and write the autoresearch playbook into the worktree. Every harness
/// adapter injects this same file (opencode via config `instructions`, Claude
/// Code via `--append-system-prompt`, Codex via `developerInstructions` —
/// legacy exec: first-turn context; Cursor: first-turn pointer at this file). Returns
/// `(workdir, playbook)` — the worktree the harness runs in and the playbook
/// path inside it.
///
/// `session_skills_dir` is the harness's worktree-relative native-skills dir
/// (`.claude/skills`, `.opencode/skills`, `.agents/skills`, `.cursor/skills`); when `Some`, the
/// modular `orx` skills are written there too, fresh alongside the playbook, so
/// the session's own agent auto-loads them with zero drift.
pub fn ensure_playbook(
    project: &LocalProject,
    session_id: &str,
    session_skills_dir: Option<&str>,
) -> Result<(PathBuf, PathBuf)> {
    let workdir = git::ensure_session_worktree(&crate::store::Store::open()?, project, session_id)?;
    let playbook = workdir.join(PLAYBOOK_REL);
    if let Some(parent) = playbook.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let project_state = ProjectState::load(&project.id)?;
    std::fs::write(&playbook, playbook_md(project, &project_state))
        .map_err(|e| anyhow!("Could not write {}: {}", playbook.display(), e))?;
    let chat_format = workdir.join(CHAT_FORMAT_REL);
    std::fs::write(&chat_format, CHAT_FORMAT)
        .map_err(|e| anyhow!("Could not write {}: {}", chat_format.display(), e))?;
    // Modular skills, written fresh beside the playbook (same freshness
    // semantics) so this session's agent discovers them natively.
    if let Some(dir) = session_skills_dir {
        super::agent_skills::ensure_session_skills(&workdir, dir)?;
        // Explicit uploads land beside the built-ins; discovered skills stay native.
        super::user_skills::write_into_session(&workdir, dir)?;
    }
    // LaTeX templates the agent copies from, written fresh for the same reason —
    // and independent of the skills dir, since no harness owns them.
    super::latex_templates::write_into_session(&workdir)?;
    // One shared exclude covers every worktree.
    exclude_agent_files(Path::new(&project.repo_path));
    // The playbook points the agent at the artifacts dir — make sure it exists.
    let _ = super::files::ensure_dir(project);
    Ok((workdir, playbook))
}

/// Write the opencode config + the playbook into the session's worktree
/// (self-healing via `ensure_session_worktree` if the cache was wiped).
/// Returns the worktree path plus, when the repo tracks its own
/// `opencode.json` (which we must never clobber — the agent commits and
/// pushes from this worktree), the path of our config to pass via
/// `OPENCODE_CONFIG` instead.
fn write_agent_files(
    project: &LocalProject,
    model: Option<&str>,
    session_id: &str,
    up_port: Option<u16>,
) -> Result<(PathBuf, Option<PathBuf>)> {
    // Source of truth for the session-skills dir is the harness trait.
    use crate::local::harness::Harness;
    let skills_dir = crate::local::harness::opencode::OpenCode.session_skills_dir();
    let (repo, playbook) = ensure_playbook(project, session_id, skills_dir)?;
    let mcp = up_port
        .map(|port| crate::local::chat::delegation::mcp_server(session_id, port))
        .transpose()?;
    let config_override = if git::is_tracked(&repo, "opencode.json") {
        // Out-of-root config: absolute instructions path (no root to anchor it).
        let path = repo
            .join(".openresearch")
            .join("agent")
            .join("opencode.json");
        std::fs::write(
            &path,
            opencode_config_json(model, &playbook.to_string_lossy(), mcp.as_ref())?,
        )
        .map_err(|e| anyhow!("Could not write {}: {}", path.display(), e))?;
        Some(path)
    } else {
        std::fs::write(
            repo.join("opencode.json"),
            opencode_config_json(model, PLAYBOOK_REL, mcp.as_ref())?,
        )
        .map_err(|e| anyhow!("Could not write opencode.json: {}", e))?;
        None
    };
    Ok((repo, config_override))
}

/// Wire status of one serve child for `GET /api/agent/status`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// What a kept server's `owner.json` records: everything [`AgentHost::ensure`] compares to
/// reuse it, and what reaching it takes. The owner directory is owner-only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Resident {
    project_id: String,
    model: Option<String>,
    /// A digest of the `OPENCODE_CONFIG_CONTENT` it started with, which carries provider keys
    /// and endpoints.
    config: Option<String>,
    native_store: NativeStore,
    alphaxiv_signed_in: bool,
    binary: ResolvedBinary,
    database: PathBuf,
    base_url: String,
    legacy_v2_api: bool,
    password: String,
}

impl Resident {
    fn endpoint(&self) -> Result<AgentEndpoint> {
        Ok(AgentEndpoint {
            base_url: self.base_url.clone(),
            client: runtime::server_client(&self.password)?,
            legacy_v2_api: self.legacy_v2_api,
        })
    }

    fn owner(&self, session_id: &str) -> Result<Value> {
        Ok(crate::keep::Owner::Agent {
            harness: HARNESS.into(),
            session_id: session_id.into(),
            state: serde_json::to_value(self)?,
        }
        .to_value())
    }
}

struct AgentChild {
    keeper: KeptHandle,
    session_id: String,
    resident: Resident,
    endpoint: AgentEndpoint,
    database: native_store::opencode_database::DatabaseLease,
    /// Held from adoption until the turn it resumes starts, so no update replaces OpenCode
    /// under that turn in between.
    resuming: Option<tokio::sync::RwLockReadGuard<'static, ()>>,
}

impl AgentChild {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            running: true,
            port: self.endpoint.port().ok(),
            project_id: Some(self.resident.project_id.clone()),
            session_id: Some(self.session_id.clone()),
            model: self.resident.model.clone(),
        }
    }

    fn alive(&self) -> bool {
        self.keeper.try_wait().is_none()
    }

    async fn stop(&self) -> Result<()> {
        runtime::stop_kept(&self.keeper).await
    }
}

/// Spawn `opencode serve` in the session's worktree and wait for it to come
/// up healthy.
async fn spawn_agent(
    project: &LocalProject,
    model: Option<&str>,
    session_id: &str,
    up_port: Option<u16>,
    native_store: NativeStore,
    binary: ResolvedBinary,
    database: native_store::opencode_database::DatabaseLease,
) -> Result<AgentChild> {
    // The clone/worktree setup inside can hit the network; keep it off the
    // async workers.
    let (repo, config_override) = {
        let (project, model) = (project.clone(), model.map(str::to_string));
        let session = session_id.to_string();
        tokio::task::spawn_blocking(move || {
            write_agent_files(&project, model.as_deref(), &session, up_port)
        })
        .await
        .map_err(|e| anyhow!("agent file task failed: {e}"))??
    };
    // Best-effort: the playbook is the real guide; the shim just lets
    // opencode's skill tool surface `orx skill` too.
    if let Err(err) = crate::commands::install_skills::install_opencode_shim().await {
        eprintln!("warning: could not install the orx opencode skill: {err}");
    }
    let mut cmd = crate::process::tokio_command(&binary.path);
    cmd.current_dir(&repo);
    // This orx first on PATH (the agent shells out to plain `orx`), the imported
    // shell environment, and the dashboard's Environment tab vars.
    prepare_chat_env(&mut cmd, model, session_id)?;
    let config = launch_config(&cmd);
    cmd.env("OPENCODE_DB", database.path())
        .env("OPENCODE_DISABLE_AUTOUPDATE", "1");
    // Tag runs the agent launches (`orx exp run`) with this session so they can
    // be explicitly subscribed to. One serve child per session; set after the
    // synced-env loop so it isn't shadowed.
    crate::local::chat::set_chat_session_env(&mut cmd, session_id, "opencode", up_port)?;
    if let Some(port) = up_port {
        for (name, value) in crate::local::chat::delegation::mcp_server(session_id, port)?.env {
            cmd.env(name, value);
        }
    }
    if let Some(config) = &config_override {
        // The repo tracks its own opencode.json; ours rides OPENCODE_CONFIG.
        // Project configs load after OPENCODE_CONFIG and would override our
        // headless permission grants, so they are disabled for this child.
        cmd.env("OPENCODE_CONFIG", config)
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1");
    }
    let owner = crate::keep::Owner::Agent {
        harness: HARNESS.into(),
        session_id: session_id.into(),
        state: Value::Null,
    };
    let (keeper, endpoint, password) = runtime::start_kept_server(&binary, cmd, &owner).await?;
    let resident = Resident {
        project_id: project.id.clone(),
        model: model.map(str::to_string),
        config,
        native_store,
        alphaxiv_signed_in: crate::alphaxiv::signed_in(),
        database: database.path().to_path_buf(),
        binary,
        base_url: endpoint.base_url.clone(),
        legacy_v2_api: endpoint.legacy_v2_api,
        password,
    };
    let verified = match model {
        Some(model) => verify_generation_models(&endpoint, model).await,
        None => Ok(()),
    };
    if let Err(error) = verified.and_then(|()| keeper.set_owner(&resident.owner(session_id)?)) {
        if let Err(stop_error) = runtime::stop_kept(&keeper).await {
            eprintln!("orx up: failed to stop OpenCode for {session_id}: {stop_error}");
        }
        return Err(error);
    }
    Ok(AgentChild {
        keeper,
        session_id: session_id.to_string(),
        resident,
        endpoint,
        database,
        resuming: None,
    })
}

/// What a permission mode makes ask: everything that changes something in
/// Manual, shell commands and anything outside the worktree in Auto, which orx
/// then reviews, and nothing in Full access.
fn asks(mode: PermissionMode) -> &'static [&'static str] {
    match mode {
        PermissionMode::Ask => &[
            "edit",
            "bash",
            "webfetch",
            "websearch",
            "external_directory",
        ],
        PermissionMode::Auto => &["bash", "external_directory"],
        _ => &[],
    }
}

/// The server's environment: its models, and what its chat's mode asks. The mode
/// rides the inline config, not the worktree's config file, which side chats and
/// delegates share and OpenCode reloads live; a mode switch changes the inline
/// config, which restarts the server.
fn prepare_chat_env(
    cmd: &mut tokio::process::Command,
    model: Option<&str>,
    session_id: &str,
) -> Result<()> {
    use crate::local::harness::{effective_permission_id, permission_mode_for};
    crate::local::local_models::prepare_generation_env(cmd, model)?;
    let stored = crate::store::Store::open()?
        .get_chat_session(session_id)?
        .and_then(|session| session.permission_mode);
    let mode = effective_permission_id("opencode", stored.as_deref())
        .and_then(|id| permission_mode_for("opencode", &id))
        .unwrap_or(PermissionMode::Bypass);
    crate::local::local_models::merge_permission_asks(cmd, asks(mode))
}

/// A digest of the inline config `cmd` passes, so a changed provider key restarts the server
/// without the key being recorded.
fn launch_config(cmd: &tokio::process::Command) -> Option<String> {
    use sha2::{Digest, Sha256};
    cmd.as_std()
        .get_envs()
        .find(|(name, _)| *name == "OPENCODE_CONFIG_CONTENT")
        .and_then(|(_, value)| value)
        .map(|value| format!("{:x}", Sha256::digest(value.as_encoded_bytes())))
}

pub(crate) async fn verify_generation_models(endpoint: &AgentEndpoint, model: &str) -> Result<()> {
    verify_generation_models_with_timeout(endpoint, model, Duration::from_secs(30)).await
}

async fn verify_generation_models_with_timeout(
    endpoint: &AgentEndpoint,
    model: &str,
    timeout: Duration,
) -> Result<()> {
    let path = "/api/agent";
    let mut mismatch = None;
    tokio::time::timeout(timeout, async {
        loop {
            let response: Value = endpoint.client.get(format!("{}{path}", endpoint.base_url))
                .send().await?.error_for_status()?.json().await?;
            let agents = response.get("data").and_then(Value::as_array)
                .ok_or_else(|| anyhow!("OpenCode returned no effective agent configuration"))?;
            if agents.is_empty() {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            mismatch = None;
            for name in ["title", "compaction"] {
                let Some(agent) = agents.iter().find(|agent| agent["id"] == name) else {
                    continue;
                };
                let configured = agent.get("model");
                let actual = configured.and_then(|value| Some(format!("{}/{}",
                    value.get("providerID")?.as_str()?, value.get("id")?.as_str()?)));
                if actual.as_deref() != Some(model) {
                    mismatch = Some(name);
                    break;
                }
            }
            if mismatch.is_none() {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await.map_err(|_| match mismatch {
        Some(name) => anyhow!("OpenCode's effective {name} model does not match the selected model. Check managed OpenCode settings."),
        None => anyhow!("OpenCode agent configuration timed out"),
    })?
}

async fn compact_v2(
    endpoint: &AgentEndpoint,
    native_id: &str,
    provider: &str,
    model: &str,
) -> Result<()> {
    let base = format!("{}/api/session/{native_id}", endpoint.base_url);
    endpoint
        .client
        .post(format!("{base}/model"))
        .json(&json!({"model":{"providerID":provider,"id":model}}))
        .send()
        .await?
        .error_for_status()?;
    let admitted: Value = endpoint
        .client
        .post(format!("{base}/compact"))
        .json(&json!({}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let id = admitted
        .pointer("/data/id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("OpenCode compaction response has no id"))?;
    tokio::time::timeout(COMPACTION_TIMEOUT, async {
        loop {
            let response = endpoint
                .client
                .get(format!("{base}/message/{id}"))
                .send()
                .await?;
            // Admission precedes the durable message projection.
            if response.status() != reqwest::StatusCode::NOT_FOUND {
                let message: Value = response.error_for_status()?.json().await?;
                match message.pointer("/data/status").and_then(Value::as_str) {
                    Some("completed") => return Ok(()),
                    Some("failed") => {
                        return Err(anyhow!(
                            "OpenCode compaction failed: {}",
                            message
                                .pointer("/data/error/message")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown error")
                        ))
                    }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("OpenCode compaction timed out"))?
}

/// Why a summarize attempt did or did not compact in place.
pub(crate) enum SummarizeOutcome {
    Compacted,
    NoServer,
    NoModel,
    ModelChanged,
}

/// The `orx up` opencode host: one serve child per chat session, keyed by the
/// orx session id, each running in that session's worktree. Share as
/// `Arc<AgentHost>` in axum state.
pub struct AgentHost {
    /// Explicit `orx up --model` default when the session has no model.
    pub(crate) model_override: Option<String>,
    /// Serializes ensure() spawns (across all sessions — a spawn is seconds,
    /// and one at a time keeps clone/fetch traffic sane). Never taken by
    /// status()/endpoint_for(), and `inner` is never held across a spawn — a slow
    /// clone or health poll must not block status reads or turn replies.
    spawn_lock: Mutex<()>,
    inner: Mutex<HashMap<String, AgentChild>>,
    up_port: std::sync::OnceLock<u16>,
    starting: std::sync::Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>,
    stopping: std::sync::atomic::AtomicBool,
}

struct StartupRegistration<'a> {
    host: &'a AgentHost,
    session: &'a str,
}

impl Drop for StartupRegistration<'_> {
    fn drop(&mut self) {
        if let Ok(mut starting) = self.host.starting.lock() {
            starting.remove(self.session);
        }
    }
}

impl AgentHost {
    fn selected_model<'a>(&'a self, model: Option<&'a str>) -> Option<&'a str> {
        model.filter(|model| !model.trim().is_empty()).or(self
            .model_override
            .as_deref()
            .filter(|model| !model.trim().is_empty()))
    }

    pub fn new(model_override: Option<String>) -> Self {
        Self {
            model_override,
            spawn_lock: Mutex::new(()),
            inner: Mutex::new(HashMap::new()),
            up_port: std::sync::OnceLock::new(),
            starting: std::sync::Mutex::new(HashMap::new()),
            stopping: std::sync::atomic::AtomicBool::new(false),
        }
    }

    pub fn set_up_port(&self, port: u16) {
        let _ = self.up_port.set(port);
    }

    /// Status of every live child; reaps children that died behind our back.
    pub async fn status(&self) -> Vec<AgentStatus> {
        let mut guard = self.inner.lock().await;
        guard.retain(|_, agent| agent.alive());
        guard.values().map(AgentChild::status).collect()
    }

    pub(crate) async fn endpoint_for(&self, session_id: &str) -> Option<AgentEndpoint> {
        self.server_for(session_id)
            .await
            .map(|(endpoint, _)| endpoint)
    }

    /// The session's live server and its keeper, which a turn checkpoints against.
    pub(crate) async fn server_for(&self, session_id: &str) -> Option<(AgentEndpoint, KeptHandle)> {
        let mut guard = self.inner.lock().await;
        let agent = guard.get(session_id)?;
        if agent.alive() {
            Some((agent.endpoint.clone(), agent.keeper.clone()))
        } else {
            guard.remove(session_id);
            None
        }
    }

    /// Lets go of what adoption held for a resuming turn once a turn holds it itself.
    pub(crate) async fn release_resume(&self, session_id: &str) {
        if let Some(agent) = self.inner.lock().await.get_mut(session_id) {
            agent.resuming = None;
        }
    }

    pub(crate) async fn resident_model(&self, session_id: &str) -> Option<String> {
        let guard = self.inner.lock().await;
        let agent = guard.get(session_id).filter(|agent| agent.alive())?;
        agent
            .resident
            .model
            .as_ref()
            .filter(|model| !model.trim().is_empty())
            .cloned()
    }

    pub(crate) async fn interrupt(&self, session_id: &str, native_id: &str) -> Result<()> {
        let Some(endpoint) = self.endpoint_for(session_id).await else {
            return Ok(());
        };
        endpoint
            .client
            .post(format!(
                "{}/api/session/{native_id}/interrupt",
                endpoint.base_url
            ))
            .timeout(Duration::from_secs(2))
            .json(&json!({}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub(crate) async fn retire_idle_children(&self) -> Result<()> {
        let mut children = self.inner.lock().await;
        for session in children.keys().cloned().collect::<Vec<_>>() {
            if let Some(agent) = children.get(&session) {
                agent.stop().await.with_context(|| {
                    format!("Could not stop the idle OpenCode server for chat {session}")
                })?;
                children.remove(&session);
            }
        }
        Ok(())
    }

    pub(crate) async fn compact(
        &self,
        project: &LocalProject,
        session_id: &str,
        native_id: &str,
        model: Option<&str>,
    ) -> Result<()> {
        let operation = async {
            let _in_use = crate::local::harness::managed::OPENCODE_IN_USE.read().await;
            match self.summarize(session_id, native_id, model).await? {
                SummarizeOutcome::Compacted => return Ok(()),
                SummarizeOutcome::NoModel => {
                    return Err(anyhow!(
                        "Select an OpenCode model before compacting this chat"
                    ));
                }
                SummarizeOutcome::NoServer | SummarizeOutcome::ModelChanged => {}
            }
            let resident_model = self.resident_model(session_id).await;
            let model = self.selected_model(model).or(resident_model.as_deref());
            let model = model
                .filter(|model| {
                    model.split_once('/').is_some_and(|(provider, id)| {
                        !provider.trim().is_empty() && !id.trim().is_empty()
                    })
                })
                .ok_or_else(|| anyhow!("Select an OpenCode model before compacting this chat"))?;
            let saved_id = native_id.to_string();
            let native =
                tokio::task::spawn_blocking(move || native_store::opencode_session(&saved_id))
                    .await
                    .map_err(|error| anyhow!("OpenCode session lookup failed: {error}"))??
                    .ok_or_else(|| {
                        anyhow!("OpenCode's saved session is missing — cannot compact this chat")
                    })?;
            let binary = resolve_binary().await?;
            let (progress, _) = tokio::sync::watch::channel(String::new());
            self.ensure(
                project,
                session_id,
                Some(model),
                ResolvedRuntime {
                    binary,
                    database: native.path,
                    store: native.store,
                },
                progress,
            )
            .await?;
            match self.summarize(session_id, native_id, Some(model)).await? {
                SummarizeOutcome::Compacted => Ok(()),
                SummarizeOutcome::NoServer => Err(anyhow!("OpenCode stopped during compaction")),
                SummarizeOutcome::NoModel | SummarizeOutcome::ModelChanged => Err(anyhow!(
                    "OpenCode could not restore the selected compaction model"
                )),
            }
        };
        match tokio::time::timeout(COMPACTION_TIMEOUT, operation).await {
            Ok(result) => result,
            Err(_) => {
                self.kill_session(session_id).await;
                Err(anyhow!(
                    "OpenCode did not finish compacting within {}s",
                    COMPACTION_TIMEOUT.as_secs()
                ))
            }
        }
    }

    /// Ask opencode to summarize (compact) a session in place.
    pub(crate) async fn summarize(
        &self,
        session_id: &str,
        native_id: &str,
        model: Option<&str>,
    ) -> Result<SummarizeOutcome> {
        let Some(endpoint) = self.endpoint_for(session_id).await else {
            return Ok(SummarizeOutcome::NoServer);
        };
        // Native compaction can retain the model already bound to its resident process.
        let resident_model = self.resident_model(session_id).await;
        let model = self.selected_model(model).or(resident_model.as_deref());
        let Some((provider, model_id)) = model
            .and_then(|model| model.split_once('/'))
            .filter(|(provider, id)| !provider.trim().is_empty() && !id.trim().is_empty())
        else {
            return Ok(SummarizeOutcome::NoModel);
        };
        if resident_model.as_deref() != model {
            return Ok(SummarizeOutcome::ModelChanged);
        }
        verify_generation_models(&endpoint, model.unwrap_or_default()).await?;
        compact_v2(&endpoint, native_id, provider, model_id).await?;
        Ok(SummarizeOutcome::Compacted)
    }

    /// Spawn (or reuse) the opencode server for this session. Idempotent when
    /// the session's server is already alive; a dead child is replaced.
    pub async fn ensure(
        &self,
        project: &LocalProject,
        session_id: &str,
        model: Option<&str>,
        runtime: ResolvedRuntime,
        progress: tokio::sync::watch::Sender<String>,
    ) -> Result<AgentStatus> {
        let _spawning = self.spawn_lock.lock().await;
        if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(anyhow!("OpenCode is shutting down"));
        }
        let (sender, mut cancel) = tokio::sync::watch::channel(false);
        self.starting
            .lock()
            .map_err(|_| anyhow!("OpenCode startup lock failed"))?
            .insert(session_id.to_string(), sender);
        let _registration = StartupRegistration {
            host: self,
            session: session_id,
        };
        let cancelled = cancel.clone();
        tokio::select! {
            result = self.ensure_started(project, session_id, model, runtime, progress, &cancelled) => result,
            _ = cancel.changed() => Err(anyhow!("OpenCode startup was cancelled")),
        }
    }

    async fn ensure_started(
        &self,
        project: &LocalProject,
        session_id: &str,
        model: Option<&str>,
        runtime: ResolvedRuntime,
        progress: tokio::sync::watch::Sender<String>,
        cancelled: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<AgentStatus> {
        let ResolvedRuntime {
            binary,
            database: database_path,
            store: native_store,
        } = runtime;
        let model = self.selected_model(model);
        let database_path = native_store::opencode_database::normalize_path(&database_path)?;
        let mut current = crate::process::tokio_command(&binary.path);
        prepare_chat_env(&mut current, model, session_id)?;
        let config = launch_config(&current);
        {
            let mut guard = self.inner.lock().await;
            if let Some(agent) = guard.get(session_id) {
                let resident = &agent.resident;
                if resident.project_id == project.id
                    && resident.alphaxiv_signed_in == crate::alphaxiv::signed_in()
                    && resident.native_store == native_store
                    && resident.binary == binary
                    && agent.database.path() == database_path
                    && resident.model.as_deref() == model
                    && resident.config == config
                    && agent.alive()
                {
                    let status = agent.status();
                    let endpoint = agent.endpoint.clone();
                    drop(guard);
                    if let Some(model) = model {
                        verify_generation_models(&endpoint, model).await?;
                    }
                    return Ok(status);
                }
            }
            if let Some(old) = guard.remove(session_id) {
                if let Err(error) = old.stop().await {
                    eprintln!("orx up: failed to stop stale OpenCode for {session_id}: {error}");
                }
            }
        }
        // A binary replacement invalidates every owned child using this database.
        {
            let mut guard = self.inner.lock().await;
            let stale: Vec<String> = guard
                .iter()
                .filter(|(_, child)| {
                    child.database.path() == database_path && child.resident.binary != binary
                })
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                if let Some(child) = guard.remove(&id) {
                    child.stop().await?;
                }
            }
        }
        let database =
            runtime::prepare_database_with_progress(&binary, &database_path, |message| {
                progress.send_replace(message.to_string());
            })
            .await?;
        // inner released: status()/port reads keep answering while the spawn
        // (clone/fetch + health poll) is in flight instead of hanging.
        let agent = spawn_agent(
            project,
            model,
            session_id,
            self.up_port.get().copied(),
            native_store,
            binary,
            database,
        )
        .await?;
        let mut inner = self.inner.lock().await;
        if *cancelled.borrow() || self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            if let Err(error) = agent.stop().await {
                eprintln!("orx up: failed to stop cancelled OpenCode for {session_id}: {error}");
            }
            return Err(anyhow!("OpenCode startup was cancelled"));
        }
        let status = agent.status();
        inner.insert(session_id.to_string(), agent);
        Ok(status)
    }

    /// Takes back a server a previous `orx up` kept for `adoption.session_id`: it answers again,
    /// holds its database lease, and from here on is reused, restarted and stopped like one this
    /// host started.
    pub(crate) async fn adopt(&self, adoption: crate::local::harness::Adoption) -> Result<()> {
        let session_id = adoption.session_id;
        let resident: Resident = serde_json::from_value(adoption.state).map_err(|error| {
            anyhow!("The OpenCode server kept for chat {session_id} has no usable record: {error}")
        })?;
        let mut kept = crate::keep::attach(&adoption.keep_id).await?;
        drop(kept.take_stdout());
        let keeper = kept.handle();
        let mut endpoint = resident.endpoint()?;
        let deadline = tokio::time::Instant::now() + ADOPT_TIMEOUT;
        let answered = tokio::time::timeout_at(deadline, async {
            while keeper.try_wait().is_none() {
                if endpoint.check_health(deadline).await.unwrap_or(false) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            false
        })
        .await
        .unwrap_or(false);
        if !answered {
            return Err(anyhow!(
                "The OpenCode server kept for chat {session_id} no longer answers at {}",
                resident.base_url
            ));
        }
        let path = resident.database.clone();
        let database = tokio::task::spawn_blocking(move || {
            native_store::opencode_database::DatabaseLease::acquire(&path, 2)
        })
        .await??;
        if database.requires_migration() {
            return Err(anyhow!(
                "OpenCode database {} needs an upgrade its kept server cannot run",
                database.path().display()
            ));
        }
        let resuming = match adoption.resume_at {
            Some(_) => Some(crate::local::harness::managed::OPENCODE_IN_USE.read().await),
            None => None,
        };
        let mut inner = self.inner.lock().await;
        if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(anyhow!("OpenCode is shutting down"));
        }
        if inner.contains_key(&session_id) {
            return Err(anyhow!("Chat {session_id} already has an OpenCode server"));
        }
        inner.insert(
            session_id.clone(),
            AgentChild {
                keeper,
                session_id,
                resident,
                endpoint,
                database,
                resuming,
            },
        );
        Ok(())
    }

    /// Kill and reap one session's child (on session delete). No-op when the
    /// session has none.
    pub async fn kill_session(&self, session_id: &str) {
        if let Ok(mut starting) = self.starting.lock() {
            if let Some(cancel) = starting.remove(session_id) {
                let _ = cancel.send(true);
            }
        }
        if let Some(agent) = self.inner.lock().await.remove(session_id) {
            if let Err(error) = agent.stop().await {
                eprintln!("orx up: failed to stop OpenCode for {session_id}: {error}");
            }
        }
    }

    /// Stops every server, except while this server hands them to the next.
    pub async fn shutdown(&self) {
        self.stopping
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut starting) = self.starting.lock() {
            for (_, cancel) in starting.drain() {
                let _ = cancel.send(true);
            }
        }
        for (session_id, agent) in self.inner.lock().await.drain() {
            if let Err(error) = agent.stop().await {
                eprintln!("orx up: failed to stop OpenCode for {session_id}: {error}");
            }
        }
    }
}

#[cfg(all(test, unix))]
impl Resident {
    fn for_test(binary: ResolvedBinary, port: u16, database: &Path, model: Option<&str>) -> Self {
        Self {
            project_id: crate::local::agent_text::sample_project().id,
            model: model.map(str::to_string),
            config: None,
            native_store: NativeStore::Isolated,
            alphaxiv_signed_in: false,
            binary,
            database: native_store::opencode_database::normalize_path(database).unwrap(),
            base_url: format!("http://127.0.0.1:{port}"),
            legacy_v2_api: false,
            password: "test".into(),
        }
    }
}

#[cfg(all(test, unix))]
impl AgentHost {
    /// Routes `session_id` to an OpenCode API on `port`, with a kept `sleep` standing in for the
    /// server; the record it keeps is the one a successor adopts.
    pub(crate) async fn serve_for_test(&self, session_id: &str, port: u16, root: &Path) {
        let path = root.join("opencode");
        orx_test_support::write_executable(&path, "#!/bin/sh\necho 2.0.12\n", 0o700).unwrap();
        let binary = runtime::resolve_binary_at(path).await.unwrap();
        let resident = Resident::for_test(binary, port, &root.join("opencode.db"), None);
        self.insert_for_test(session_id, resident).await;
    }

    async fn insert_for_test(&self, session_id: &str, resident: Resident) -> KeptHandle {
        let mut sleep = crate::process::command("sleep");
        sleep.arg("60");
        let kept = crate::keep::spawn(crate::keep::Keep {
            command: &sleep,
            io: crate::keep::Io::Pipes { stderr: None },
            retain: crate::keep::Retain::Tail { bytes: 1024 },
            owner: resident.owner(session_id).unwrap(),
        })
        .await
        .unwrap();
        let database =
            native_store::opencode_database::DatabaseLease::acquire(&resident.database, 2).unwrap();
        let keeper = kept.handle();
        self.inner.lock().await.insert(
            session_id.into(),
            AgentChild {
                keeper: keeper.clone(),
                session_id: session_id.into(),
                endpoint: resident.endpoint().unwrap(),
                resident,
                database,
                resuming: None,
            },
        );
        keeper
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::agent_text::sample_project;

    #[test]
    fn alphaxiv_server_has_its_own_name_and_asks_before_writes() {
        let cfg: Value =
            serde_json::from_str(&opencode_config_json(None, "p.md", None).unwrap()).unwrap();
        assert!(cfg["mcp"].get("alphaxiv").is_none());
        assert_eq!(cfg["mcp"]["orx_alphaxiv"]["command"][1], "alphaxiv-mcp");
        assert_eq!(cfg["permission"]["orx_alphaxiv_*"], "allow");
        assert_eq!(cfg["permission"]["orx_alphaxiv_create_folder"], "ask");
        assert_eq!(
            cfg["permission"]["orx_alphaxiv_save_papers_to_folder"],
            "ask"
        );
    }

    #[test]
    fn each_mode_asks_for_what_it_gates() {
        assert!(asks(PermissionMode::Bypass).is_empty());
        assert_eq!(asks(PermissionMode::Auto), ["bash", "external_directory"]);
        assert_eq!(
            asks(PermissionMode::Ask),
            [
                "edit",
                "bash",
                "webfetch",
                "websearch",
                "external_directory"
            ]
        );
    }

    #[test]
    fn shared_config_names_the_orx_server_env_without_any_chats_values() {
        let server = crate::local::chat::delegation::mcp_server("chat_a", 4900).unwrap();
        let config: Value =
            serde_json::from_str(&opencode_config_json(None, "p.md", Some(&server)).unwrap())
                .unwrap();
        let environment = &config["mcp"]["orx"]["environment"];
        assert_eq!(environment["ORX_SESSION_ID"], "{env:ORX_SESSION_ID}");
        assert_eq!(environment["ORX_AGENT_TOKEN"], "{env:ORX_AGENT_TOKEN}");
        assert!(!config.to_string().contains("chat_a"));
        assert_eq!(config["permission"]["task"], "deny");
        let without: Value =
            serde_json::from_str(&opencode_config_json(None, "p.md", None).unwrap()).unwrap();
        assert_eq!(without["permission"]["task"], "allow");
        assert!(without["mcp"].get("orx").is_none());
    }

    #[test]
    fn resident_selection_keeps_cloud_session_ahead_of_host_default() {
        let host = AgentHost::new(Some("host/default".into()));
        assert_eq!(
            host.selected_model(Some("openrouter/selected")),
            Some("openrouter/selected")
        );
        assert_eq!(
            host.selected_model(Some("orx-local-test/qwen")),
            Some("orx-local-test/qwen")
        );
        assert_eq!(host.selected_model(None), Some("host/default"));
        assert_eq!(host.selected_model(Some(" ")), Some("host/default"));
        assert_eq!(AgentHost::new(None).selected_model(None), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn native_compaction_reuses_the_resident_model_and_requires_reconfiguration_on_change() {
        use axum::{
            routing::{get, post},
            Json, Router,
        };
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let fixture = git::TemporaryDirectory::new("orx-resident-model").unwrap();
        let path = fixture.path().join("opencode");
        orx_test_support::write_executable(&path, "#!/bin/sh\necho 2.0.1\n", 0o700).unwrap();
        let binary = resolve_binary_at(path).await.unwrap();
        let database_path = fixture.path().join("opencode.db");
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let compacted = Arc::new(AtomicUsize::new(0));
        let summaries = compacted.clone();
        let app = Router::new()
            .route(
                "/api/agent",
                get(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    async {
                        Json(json!({"data":[
                            {"id":"title","model":{"providerID":"cloud","id":"selected"}},
                            {"id":"compaction","model":{"providerID":"cloud","id":"selected"}}
                        ]}))
                    }
                }),
            )
            .route("/api/session/native/model", post(|| async {}))
            .route(
                "/api/session/native/compact",
                post(move || {
                    summaries.fetch_add(1, Ordering::SeqCst);
                    async { Json(json!({"data":{"id":"compaction"}})) }
                }),
            )
            .route(
                "/api/session/native/message/compaction",
                get(|| async { Json(json!({"data":{"status":"completed"}})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let project = sample_project();
        let mut host = AgentHost::new(Some("host/default".into()));
        let resident = Resident {
            config: config_for(&binary, "cloud/selected"),
            ..Resident::for_test(binary.clone(), port, &database_path, Some("cloud/selected"))
        };
        let kept = host.insert_for_test("chat", resident).await;
        let (progress, _) = tokio::sync::watch::channel(String::new());
        let status = host
            .ensure(
                &project,
                "chat",
                Some("cloud/selected"),
                ResolvedRuntime {
                    binary,
                    database: database_path,
                    store: NativeStore::Isolated,
                },
                progress,
            )
            .await
            .unwrap();
        assert_eq!(status.model.as_deref(), Some("cloud/selected"));
        assert_eq!(host.inner.lock().await["chat"].keeper.id(), kept.id());
        let before = requests.load(Ordering::SeqCst);
        assert!(matches!(
            host.summarize("chat", "native", Some("cloud/switched"))
                .await
                .unwrap(),
            SummarizeOutcome::ModelChanged
        ));
        assert_eq!(requests.load(Ordering::SeqCst), before);
        assert_eq!(compacted.load(Ordering::SeqCst), 0);
        host.model_override = None;
        assert_eq!(
            host.resident_model("chat").await.as_deref(),
            Some("cloud/selected")
        );
        host.compact(&project, "chat", "native", None)
            .await
            .unwrap();
        assert_eq!(compacted.load(Ordering::SeqCst), 1);
        host.shutdown().await;
        assert!(host.resident_model("chat").await.is_none());
        server.abort();
    }

    fn config_for(binary: &ResolvedBinary, model: &str) -> Option<String> {
        let mut cmd = crate::process::tokio_command(&binary.path);
        crate::local::local_models::prepare_generation_env(&mut cmd, Some(model)).unwrap();
        launch_config(&cmd)
    }

    #[cfg(unix)]
    #[test]
    fn a_changed_provider_key_restarts_the_resident_child() {
        if !crate::config::in_test_child(&["OPENCODE_CONFIG_CONTENT"]) {
            return;
        }
        use axum::{routing::get, Json, Router};
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let fixture = git::TemporaryDirectory::new("orx-resident-config").unwrap();
            let path = fixture.path().join("opencode");
            orx_test_support::write_executable(&path, "#!/bin/sh\necho 2.0.1\n", 0o700).unwrap();
            let binary = resolve_binary_at(path).await.unwrap();
            let database_path = fixture.path().join("opencode.db");
            let write_key = |key: &str| {
                let path = crate::local::local_models::providers_path();
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let records = json!({"openai":{"kind":"key","key":key,"name":"Work"}});
                std::fs::write(path, records.to_string()).unwrap();
            };
            write_key("sk-old");
            let app = Router::new().route(
                "/api/agent",
                get(|| async {
                    Json(json!({"data":[
                        {"id":"title","model":{"providerID":"cloud","id":"selected"}},
                        {"id":"compaction","model":{"providerID":"cloud","id":"selected"}}
                    ]}))
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let project = sample_project();
            let host = AgentHost::new(None);
            let resident = Resident {
                config: config_for(&binary, "cloud/selected"),
                ..Resident::for_test(binary.clone(), port, &database_path, Some("cloud/selected"))
            };
            let kept = host.insert_for_test("chat", resident).await;
            let ensure = || {
                let (progress, _) = tokio::sync::watch::channel(String::new());
                host.ensure(
                    &project,
                    "chat",
                    Some("cloud/selected"),
                    ResolvedRuntime {
                        binary: binary.clone(),
                        database: database_path.clone(),
                        store: NativeStore::Isolated,
                    },
                    progress,
                )
            };
            ensure().await.unwrap();
            assert_eq!(host.inner.lock().await["chat"].keeper.id(), kept.id());

            write_key("sk-new");
            let _ = tokio::time::timeout(Duration::from_secs(30), ensure()).await;
            let resident = host
                .inner
                .lock()
                .await
                .get("chat")
                .map(|agent| agent.keeper.id().to_string());
            assert_ne!(resident.as_deref(), Some(kept.id()));
            host.shutdown().await;
            server.abort();
        });
    }

    #[tokio::test]
    async fn cold_compaction_requires_a_valid_selected_model_before_startup() {
        let host = AgentHost::new(None);
        for model in [
            None,
            Some(" "),
            Some("model"),
            Some("provider/"),
            Some("/model"),
        ] {
            let error = host
                .compact(&sample_project(), "cold-chat", "native", model)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Select an OpenCode model"));
            assert!(host.inner.lock().await.is_empty());
            assert!(host.starting.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn cold_compaction_does_not_replace_a_missing_native_session() {
        const CHILD: &str = "ORX_TEST_OPENCODE_MISSING_COMPACTION_SESSION";
        if std::env::var_os(CHILD).is_some() {
            tokio::runtime::Runtime::new().unwrap().block_on(async {
                let host = AgentHost::new(None);
                let error = host
                    .compact(
                        &sample_project(),
                        "cold-chat",
                        "missing-native",
                        Some("provider/model"),
                    )
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains("saved session is missing"));
                assert!(host.status().await.is_empty());
                assert!(!native_store::opencode_db(NativeStore::Isolated).exists());
                assert!(!native_store::opencode_db(NativeStore::Legacy).exists());
            });
            return;
        }
        let fixture = git::TemporaryDirectory::new("orx-missing-opencode-compaction").unwrap();
        let output = crate::process::command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local::opencode::tests::cold_compaction_does_not_replace_a_missing_native_session",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("ORX_DATA_DIR", fixture.path().join("orx"))
            .env("XDG_DATA_HOME", fixture.path().join("xdg"))
            .env("OPENCODE_DB", fixture.path().join("missing.db"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn effective_agent_models_reject_overrides_but_allow_disabled_agents() {
        use axum::{routing::get, Json, Router};
        for (models, expected) in [
            (vec!["selected", "selected"], true),
            (vec!["managed", "selected"], false),
            (vec![], true),
        ] {
            let agents: Vec<_> = ["title", "compaction"]
                .iter()
                .zip(models)
                .map(|(name, model)| json!({"id":name,"model":{"providerID":"custom","id":model}}))
                .chain(std::iter::once(json!({"id":"build","name":"build"})))
                .collect();
            let body = json!({"data":agents});
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = AgentEndpoint {
                base_url: format!("http://{}", listener.local_addr().unwrap()),
                client: crate::net::loopback_client().build().unwrap(),
                legacy_v2_api: false,
            };
            let app = Router::new().route(
                "/api/agent",
                get(move || {
                    let body = body.clone();
                    async move { Json(body) }
                }),
            );
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let result = verify_generation_models_with_timeout(
                &endpoint,
                "custom/selected",
                Duration::from_secs(1),
            )
            .await;
            assert_eq!(result.is_ok(), expected, "{result:?}");
            if !expected {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("effective title model"));
            }
            server.abort();
        }
    }

    #[tokio::test]
    async fn effective_agent_models_wait_for_configuration_to_converge() {
        use axum::{routing::get, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        let polls = Arc::new(AtomicUsize::new(0));
        let count = polls.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = AgentEndpoint {
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            client: crate::net::loopback_client().build().unwrap(),
            legacy_v2_api: false,
        };
        let app = Router::new().route(
            "/api/agent",
            get(move || {
                let count = count.clone();
                async move {
                    let poll = count.fetch_add(1, Ordering::SeqCst);
                    let model = if poll == 1 { "managed" } else { "selected" };
                    let agents = if poll == 0 {
                        json!([])
                    } else {
                        json!([{"id":"title","model":{"providerID":"custom","id":model}}])
                    };
                    Json(json!({"data":agents}))
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        verify_generation_models_with_timeout(&endpoint, "custom/selected", Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(polls.load(Ordering::SeqCst), 3);
        server.abort();
    }

    #[tokio::test]
    async fn v2_compaction_waits_for_completion_and_propagates_failure() {
        use axum::{
            routing::{get, post},
            Json, Router,
        };
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        for failed in [false, true] {
            let polls = Arc::new(AtomicUsize::new(0));
            let count = polls.clone();
            let app = Router::new()
                .route("/api/session/chat/model", post(|Json(body): Json<Value>| async move {
                    assert_eq!(body, json!({"model":{"providerID":"custom","id":"selected"}}));
                }))
                .route("/api/session/chat/compact", post(|| async { Json(json!({"data":{"id":"compact-id"}})) }))
                .route("/api/session/chat/message/compact-id", get(move || {
                    let count = count.clone();
                    async move { Json(json!({"data": if count.fetch_add(1, Ordering::SeqCst) == 0 { json!({"status":"running"}) } else if failed { json!({"status":"failed","error":{"message":"fixture failure"}}) } else { json!({"status":"completed"}) }})) }
                }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = AgentEndpoint {
                base_url: format!("http://{}", listener.local_addr().unwrap()),
                client: crate::net::loopback_client().build().unwrap(),
                legacy_v2_api: false,
            };
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let result = compact_v2(&endpoint, "chat", "custom", "selected").await;
            assert_eq!(polls.load(Ordering::SeqCst), 2);
            if failed {
                assert!(result.unwrap_err().to_string().contains("fixture failure"));
            } else {
                result.unwrap();
            }
            server.abort();
        }
    }

    #[tokio::test]
    async fn cancellation_does_not_wait_for_migration_or_allow_late_startup() {
        let host = AgentHost::new(None);
        let _spawning = host.spawn_lock.lock().await;
        let (cancel, receiver) = tokio::sync::watch::channel(false);
        host.starting.lock().unwrap().insert("chat".into(), cancel);
        tokio::time::timeout(Duration::from_secs(1), host.kill_session("chat"))
            .await
            .unwrap();
        assert!(*receiver.borrow());
        tokio::time::timeout(Duration::from_secs(1), host.shutdown())
            .await
            .unwrap();
        assert!(host.stopping.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_and_drop_kill_descendants_in_the_server_process_group() {
        use tokio::io::AsyncBufReadExt as _;

        for mode in ["kill", "drop", "reaped_then_drop"] {
            let script = if mode == "reaped_then_drop" {
                "sleep 60 & echo $!"
            } else {
                "sleep 60 & echo $!; wait"
            };
            let mut cmd = crate::process::tokio_command("sh");
            cmd.args(["-c", script])
                .stdout(std::process::Stdio::piped())
                .kill_on_drop(true)
                .process_group(0);
            let mut spawned = cmd.spawn().unwrap();
            let mut lines = tokio::io::BufReader::new(spawned.stdout.take().unwrap()).lines();
            let descendant_pid: libc::pid_t =
                lines.next_line().await.unwrap().unwrap().parse().unwrap();
            assert_eq!(unsafe { libc::kill(descendant_pid, 0) }, 0);

            let mut child = ServerChild::new(spawned);
            match mode {
                "kill" => child.kill().await.unwrap(),
                "drop" => drop(child),
                "reaped_then_drop" => {
                    let mut reaped = false;
                    for _ in 0..100 {
                        if child.try_wait().unwrap().is_some() {
                            reaped = true;
                            break;
                        }
                        tokio::time::sleep(crate::config::paced(Duration::from_millis(10))).await;
                    }
                    assert!(reaped, "leader did not exit before drop");
                    assert_eq!(child.id(), None, "reaped PID must not be kept for later");
                    drop(child);
                }
                _ => unreachable!(),
            }

            let mut exited = false;
            for _ in 0..100 {
                if unsafe { libc::kill(descendant_pid, 0) } != 0 {
                    exited = true;
                    break;
                }
                tokio::time::sleep(crate::config::paced(Duration::from_millis(10))).await;
            }
            assert!(
                exited,
                "descendant {descendant_pid} survived server cleanup (mode={mode})"
            );
        }
    }

    #[cfg(unix)]
    async fn keeper_exits(id: &str) {
        let running = || {
            crate::keep::entries()
                .unwrap()
                .iter()
                .any(|entry| entry.id == id && entry.liveness == crate::keep::Liveness::Alive)
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while running() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the server's keeper exits once it is stopped");
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_server_is_adopted_reused_and_stopped_by_the_next_host() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        use axum::{routing::get, Router};
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let app = Router::new().route(
                "/api/info",
                get(|| async { axum::Json(json!({"version": "2.0.12"})) }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let fixture = git::TemporaryDirectory::new("orx-adopt-opencode").unwrap();
            let path = fixture.path().join("opencode");
            orx_test_support::write_executable(&path, "#!/bin/sh\necho 2.0.12\n", 0o700).unwrap();
            let binary = resolve_binary_at(path).await.unwrap();
            let database_path = fixture.path().join("opencode.db");
            rusqlite::Connection::open(&database_path)
                .unwrap()
                .execute_batch(
                    "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT);
                    CREATE TABLE session_message (id TEXT, session_id TEXT, data TEXT);
                    CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);
                    CREATE TABLE migration (id TEXT PRIMARY KEY, time_completed INTEGER);
                    INSERT INTO migration VALUES ('20260910120000_clear_v1_session_permission', 1);",
                )
                .unwrap();
            let mut current = crate::process::tokio_command(&binary.path);
            crate::local::local_models::prepare_generation_env(&mut current, None).unwrap();
            let resident = Resident {
                config: launch_config(&current),
                ..Resident::for_test(binary.clone(), port, &database_path, None)
            };
            let record = |id: &str| {
                let entry = crate::keep::entries()
                    .unwrap()
                    .into_iter()
                    .find(|entry| entry.id == id)
                    .unwrap();
                let crate::keep::Record::Known(crate::keep::Owner::Agent { state, .. }) =
                    entry.owner
                else {
                    panic!("an agent record");
                };
                (entry.liveness == crate::keep::Liveness::Alive, state)
            };
            let adoption = |keep_id: &str, state: Value| crate::local::harness::Adoption {
                keep_id: keep_id.into(),
                session_id: "chat".into(),
                state,
                resume_at: None,
                commands: Vec::new(),
            };

            // A restart hands the server off; the next host takes it back.
            let previous = AgentHost::new(None);
            let kept = previous.insert_for_test("chat", resident).await;
            kept.detach();
            drop(previous);
            let (alive, state) = record(kept.id());
            assert!(alive);
            let host = AgentHost::new(None);
            host.adopt(adoption(kept.id(), state)).await.unwrap();
            let status = host.status().await;
            assert_eq!(status.len(), 1);
            assert_eq!(status[0].port, Some(port));
            let (progress, _) = tokio::sync::watch::channel(String::new());
            host.ensure(
                &sample_project(),
                "chat",
                None,
                ResolvedRuntime {
                    binary: binary.clone(),
                    database: database_path.clone(),
                    store: NativeStore::Isolated,
                },
                progress,
            )
            .await
            .unwrap();
            assert_eq!(host.inner.lock().await["chat"].keeper.id(), kept.id());
            host.kill_session("chat").await;
            keeper_exits(kept.id()).await;

            // A server that died during the gap, or whose keeper is gone, is not taken back.
            let error = host.adopt(adoption(kept.id(), Value::Null)).await;
            assert!(error.unwrap_err().to_string().contains("no usable record"));
            let died = Resident::for_test(binary, port, &database_path, None);
            let exited = crate::process::command("true");
            let kept = crate::keep::spawn(crate::keep::Keep {
                command: &exited,
                io: crate::keep::Io::Pipes { stderr: None },
                retain: crate::keep::Retain::Tail { bytes: 1024 },
                owner: died.owner("chat").unwrap(),
            })
            .await
            .unwrap();
            let id = kept.handle().id().to_string();
            kept.handle().detach();
            drop(kept);
            let (_, state) = record(&id);
            let error = host.adopt(adoption(&id, state.clone())).await;
            assert!(error.unwrap_err().to_string().contains("no longer answers"));
            assert!(host.status().await.is_empty());
            keeper_exits(&id).await;
            assert!(host.adopt(adoption(&id, state)).await.is_err());
        });
    }

    #[test]
    fn project_state_summarizes_counts_and_the_run_command() {
        let mut project = sample_project();
        let fresh = project_state_md(&project, &ProjectState::default());
        assert!(fresh.contains("**0 experiments and 0 runs**"));
        assert!(fresh.contains("no fixed run command"));

        project.run_command = Some("python train.py".into());
        let state = ProjectState {
            experiments: 12,
            runs: 18,
            active_runs: 1,
            compute: None,
        };
        let existing = project_state_md(&project, &state);
        assert!(existing.contains("**12 experiments**"));
        assert!(existing.contains("**18 runs** (1 run active)"));
        assert!(existing.contains("fixed run command is configured"));
    }
}
