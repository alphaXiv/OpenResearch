//! opencode bootstrap for `orx up` — binary discovery, per-session config +
//! playbook written into the session's worktree, spawn + health check, and the
//! shared `AgentHost` handle the axum server holds (`Arc<AgentHost>` in state).
//!
//! One opencode serve child **per chat session**, cwd = that session's private
//! worktree (see `git::ensure_session_worktree`) so parallel agents never share
//! a checkout. Env is inherited (that's where ANTHROPIC_API_KEY /
//! OPENROUTER_API_KEY live — opencode auto-detects providers from env).
//! Children die with `orx up` via `kill_on_drop`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::json;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::error::{anyhow, Result};
use crate::local::git;
use crate::local::model::LocalProject;
use crate::local::native_store::{self, NativeStore};
use crate::store;

/// Playbook path inside the session worktree; opencode re-reads it every turn,
/// so rewriting the file retargets a running server without a restart.
const PLAYBOOK_REL: &str = ".openresearch/agent/autoresearch-local.md";

const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// `opencode` on PATH, else the installer's default drop location.
pub fn find_opencode() -> Result<PathBuf> {
    if let Some(found) = crate::local::shell_env::find_on_path("opencode") {
        return Ok(found);
    }
    if let Some(home) = dirs::home_dir() {
        let fallback = home.join(".opencode").join("bin").join("opencode");
        if fallback.is_file() {
            return Ok(fallback);
        }
    }
    Err(anyhow!(
        "opencode not found (checked PATH and ~/.opencode/bin/opencode).\n\
         Install it with: curl -fsSL https://opencode.ai/install | bash"
    ))
}

/// Ask the OS for a free loopback port (bind :0, read it back, release).
fn free_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| anyhow!("Could not pick a free port: {}", e))?;
    Ok(listener.local_addr()?.port())
}

/// Where the spawned server's stdout/stderr land (startup diagnostics).
pub fn agent_log_path() -> PathBuf {
    store::data_dir().join("agent-opencode.log")
}

/// Project-local opencode config. Every real permission is pre-approved so a
/// headless turn never stalls on a TUI prompt; the interactive `question` tool
/// is denied AND disabled (it would deadlock serve mode — nothing can answer
/// it), repeated on the default `build` agent because the tool filter is
/// agent-scoped. `model` only when the user passed `orx up --model`.
fn opencode_config_json(model: Option<&str>, instructions: &str) -> String {
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
            "question": "deny",
        },
        "tools": { "question": false },
        "agent": {
            "build": { "tools": { "question": false }, "permission": { "question": "deny" } }
        },
        "instructions": [instructions],
    });
    if let Some(model) = model {
        cfg["model"] = json!(model);
    }
    serde_json::to_string_pretty(&cfg).unwrap_or_else(|_| "{}".to_string())
}

/// The research playbook: durable project context and skill routing for `orx up`.
/// The playbook template — a literal, GitHub-readable markdown file. Rendered
/// by [`playbook_md`]: the leading HTML comment is stripped and `{token}`
/// placeholders are substituted (project facts, state, skills, and the compute default).
const SYSTEM_PROMPT: &str = include_str!("../../SYSTEM_PROMPT.md");

#[derive(Default)]
struct ProjectState {
    experiments: usize,
    runs: usize,
    active_runs: usize,
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
        })
    }
}

fn plural(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn project_state_md(project: &LocalProject, state: &ProjectState) -> String {
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

fn playbook_md(project: &LocalProject, state: &ProjectState) -> String {
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
        format!(
            "- Paper: arXiv {p} (https://arxiv.org/abs/{p}) — the paper this project starts \
             from; `orx paper {p}` fetches its report\n"
        )
    });
    // The default compute target is read fresh on every
    // playbook rewrite, but how soon a rewrite reaches a live agent varies:
    // claude reads it at child spawn, so a rewrite reaches the agent on the next
    // respawn (config change / interrupt / crash), not every turn; codex only on
    // thread start/resume; a live opencode server keeps its playbook until
    // respawn (`AgentHost::ensure` early-returns for a running child). Launch-time
    // resolution in `exp run` stays authoritative either way: the agent is
    // told to OMIT `--backend`, never to echo the default back, so even a
    // stale prompt launches on the current default.
    let configured_compute_default = crate::config::compute_default();
    let compute_default = configured_compute_default
        .clone()
        .unwrap_or_else(|| ("local".to_string(), None));
    let compute_default_source = if configured_compute_default.is_some() {
        "the user's configured default"
    } else {
        "the default `local` backend"
    };
    let (compute_backend, compute_flavor) = &compute_default;
    let flavor_part = compute_flavor
        .as_ref()
        .map_or(String::new(), |flavor| format!(" (`--flavor {flavor}`)"));
    let compute_bullet = format!(
        "- Compute: default target **{compute_backend}**{flavor_part} — \
         {compute_default_source}; load **`orx-compute`** before launching"
    );
    let project_state = project_state_md(project, state);
    let skill_names = super::agent_skills::skills(super::agent_skills::SkillSet::Local)
        .iter()
        .map(|skill| format!("- `{}`", skill.name))
        .collect::<Vec<_>>()
        .join("\n");
    let template = SYSTEM_PROMPT
        .split_once("-->\n\n")
        .map(|(_, rest)| rest)
        .unwrap_or(SYSTEM_PROMPT);
    template
        .replace("{name}", name)
        .replace("{id}", id)
        .replace("{publication_line}", publication_line)
        .replace("{paper_line}", &paper_line)
        .replace("{compute_bullet}", &compute_bullet)
        .replace("{artifacts}", &artifacts)
        .replace("{project_state}", &project_state)
        .replace("{skill_names}", &skill_names)
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
/// legacy exec: first-turn context). Returns
/// `(workdir, playbook)` — the worktree the harness runs in and the playbook
/// path inside it.
///
/// `session_skills_dir` is the harness's worktree-relative native-skills dir
/// (`.claude/skills`, `.opencode/skills`, `.agents/skills`); when `Some`, the
/// modular `orx` skills are written there too, fresh alongside the playbook, so
/// the session's own agent auto-loads them with zero drift.
pub fn ensure_playbook(
    project: &LocalProject,
    session_id: &str,
    session_skills_dir: Option<&str>,
) -> Result<(PathBuf, PathBuf)> {
    let workdir = git::ensure_session_worktree(project, session_id)?;
    let playbook = workdir.join(PLAYBOOK_REL);
    if let Some(parent) = playbook.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let project_state = ProjectState::load(&project.id)?;
    std::fs::write(&playbook, playbook_md(project, &project_state))
        .map_err(|e| anyhow!("Could not write {}: {}", playbook.display(), e))?;
    // Modular skills, written fresh beside the playbook (same freshness
    // semantics) so this session's agent discovers them natively.
    if let Some(dir) = session_skills_dir {
        super::agent_skills::ensure_session_skills(&workdir, dir)?;
        // User-uploaded skills land beside the built-ins, same freshness.
        super::user_skills::write_into_session(&workdir, dir, &project.id)?;
    }
    // LaTeX templates the agent copies from, written fresh for the same reason —
    // and independent of the skills dir, since no harness owns them.
    super::latex_templates::write_into_session(&workdir, &project.id)?;
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
) -> Result<(PathBuf, Option<PathBuf>)> {
    // Source of truth for the session-skills dir is the harness trait.
    use crate::local::harness::Harness;
    let skills_dir = crate::local::harness::opencode::OpenCode.session_skills_dir();
    let (repo, playbook) = ensure_playbook(project, session_id, skills_dir)?;
    let config_override = if git::is_tracked(&repo, "opencode.json") {
        // Out-of-root config: absolute instructions path (no root to anchor it).
        let path = repo
            .join(".openresearch")
            .join("agent")
            .join("opencode.json");
        std::fs::write(
            &path,
            opencode_config_json(model, &playbook.to_string_lossy()),
        )
        .map_err(|e| anyhow!("Could not write {}: {}", path.display(), e))?;
        Some(path)
    } else {
        std::fs::write(
            repo.join("opencode.json"),
            opencode_config_json(model, PLAYBOOK_REL),
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

struct AgentChild {
    child: Child,
    port: u16,
    project_id: String,
    session_id: String,
    model: Option<String>,
    native_store: NativeStore,
}

impl AgentChild {
    fn status(&self) -> AgentStatus {
        AgentStatus {
            running: true,
            port: Some(self.port),
            project_id: Some(self.project_id.clone()),
            session_id: Some(self.session_id.clone()),
            model: self.model.clone(),
        }
    }
}

/// Poll `/global/health` until opencode answers, watching for early exit.
async fn wait_healthy(child: &mut Child, port: u16) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()?;
    let url = format!("http://127.0.0.1:{port}/global/health");
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "opencode exited during startup ({status}); see {}",
                agent_log_path().display()
            ));
        }
        if let Ok(resp) = client.get(&url).send().await {
            if resp.status().is_success() {
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err(anyhow!(
                "opencode did not become healthy on 127.0.0.1:{port} within {}s; see {}",
                HEALTH_TIMEOUT.as_secs(),
                agent_log_path().display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
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
) -> Result<AgentChild> {
    let bin = find_opencode()?;
    // The clone/worktree setup inside can hit the network; keep it off the
    // async workers.
    let (repo, config_override) = {
        let (project, model) = (project.clone(), model.map(str::to_string));
        let session = session_id.to_string();
        tokio::task::spawn_blocking(move || write_agent_files(&project, model.as_deref(), &session))
            .await
            .map_err(|e| anyhow!("agent file task failed: {e}"))??
    };
    // Best-effort: the playbook is the real guide; the shim just lets
    // opencode's skill tool surface `orx skill` too.
    if let Err(err) = crate::commands::install_skills::install_opencode_shim().await {
        eprintln!("warning: could not install the orx opencode skill: {err}");
    }
    let port = free_port()?;
    // The data dir may not exist yet (fresh machine, no Store::open before us).
    if let Some(parent) = agent_log_path().parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(agent_log_path())
        .map_err(|e| anyhow!("Could not open {}: {}", agent_log_path().display(), e))?;

    let mut cmd = Command::new(&bin);
    cmd.arg("serve")
        .arg("--port")
        .arg(port.to_string())
        .arg("--hostname")
        .arg("127.0.0.1")
        // Without --print-logs the log file stays empty and startup failures
        // are undiagnosable.
        .arg("--print-logs")
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().map_err(|e| anyhow!("{e}"))?))
        .stderr(Stdio::from(log))
        // Dies with `orx up` when the runtime drops the handle (Ctrl-C, exit).
        .kill_on_drop(true);
    // This orx first on PATH (the agent shells out to plain `orx`), the imported
    // shell environment, and the dashboard's Environment tab vars.
    crate::local::chat::prepare_env(&mut cmd);
    cmd.env("OPENCODE_DB", native_store::prepare_opencode(native_store)?);
    // Tag runs the agent launches (`orx exp run`) with this session so they can
    // be explicitly subscribed to. One serve child per session; set after the
    // synced-env loop so it isn't shadowed.
    crate::local::chat::set_chat_session_env(&mut cmd, session_id, up_port);
    if let Some(config) = &config_override {
        // The repo tracks its own opencode.json; ours rides OPENCODE_CONFIG.
        // Project configs load after OPENCODE_CONFIG and would override our
        // headless permission grants, so they are disabled for this child.
        cmd.env("OPENCODE_CONFIG", config)
            .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1");
    }
    // Own process group: a terminal SIGINT reaches orx up alone, which then
    // tears the child down deliberately (kill_on_drop / shutdown()).
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Could not spawn {}: {}", bin.display(), e))?;
    if let Err(err) = wait_healthy(&mut child, port).await {
        let _ = child.kill().await;
        return Err(err);
    }
    Ok(AgentChild {
        child,
        port,
        project_id: project.id.clone(),
        session_id: session_id.to_string(),
        model: model.map(str::to_string),
        native_store,
    })
}

/// The `orx up` opencode host: one serve child per chat session, keyed by the
/// orx session id, each running in that session's worktree. Share as
/// `Arc<AgentHost>` in axum state.
pub struct AgentHost {
    /// `orx up --model` override, applied to every spawn.
    model_override: Option<String>,
    /// Serializes ensure() spawns (across all sessions — a spawn is seconds,
    /// and one at a time keeps clone/fetch traffic sane). Never taken by
    /// status()/port_for(), and `inner` is never held across a spawn — a slow
    /// clone or health poll must not block status reads or turn replies.
    spawn_lock: Mutex<()>,
    inner: Mutex<HashMap<String, AgentChild>>,
    up_port: std::sync::OnceLock<u16>,
}

impl AgentHost {
    pub fn new(model_override: Option<String>) -> Self {
        Self {
            model_override,
            spawn_lock: Mutex::new(()),
            inner: Mutex::new(HashMap::new()),
            up_port: std::sync::OnceLock::new(),
        }
    }

    pub fn set_up_port(&self, port: u16) {
        let _ = self.up_port.set(port);
    }

    /// Status of every live child; reaps children that died behind our back.
    pub async fn status(&self) -> Vec<AgentStatus> {
        let mut guard = self.inner.lock().await;
        guard.retain(|_, agent| matches!(agent.child.try_wait(), Ok(None)));
        guard.values().map(AgentChild::status).collect()
    }

    /// Loopback port of the session's live server (for inline replies/aborts).
    pub async fn port_for(&self, session_id: &str) -> Option<u16> {
        let mut guard = self.inner.lock().await;
        let agent = guard.get_mut(session_id)?;
        if matches!(agent.child.try_wait(), Ok(None)) {
            Some(agent.port)
        } else {
            guard.remove(session_id);
            None
        }
    }

    /// Spawn (or reuse) the opencode server for this session. Idempotent when
    /// the session's server is already alive; a dead child is replaced.
    pub async fn ensure(
        &self,
        project: &LocalProject,
        session_id: &str,
        native_store: NativeStore,
    ) -> Result<AgentStatus> {
        let _spawning = self.spawn_lock.lock().await;
        {
            let mut guard = self.inner.lock().await;
            if let Some(agent) = guard.get_mut(session_id) {
                if agent.project_id == project.id
                    && agent.native_store == native_store
                    && matches!(agent.child.try_wait(), Ok(None))
                {
                    return Ok(agent.status());
                }
            }
            if let Some(mut old) = guard.remove(session_id) {
                let _ = old.child.kill().await; // kill() also reaps
            }
        }
        // inner released: status()/port reads keep answering while the spawn
        // (clone/fetch + health poll) is in flight instead of hanging.
        let agent = spawn_agent(
            project,
            self.model_override.as_deref(),
            session_id,
            self.up_port.get().copied(),
            native_store,
        )
        .await?;
        let status = agent.status();
        self.inner
            .lock()
            .await
            .insert(session_id.to_string(), agent);
        Ok(status)
    }

    /// Kill and reap one session's child (on session delete). No-op when the
    /// session has none.
    pub async fn kill_session(&self, session_id: &str) {
        if let Some(mut agent) = self.inner.lock().await.remove(session_id) {
            let _ = agent.child.kill().await;
        }
    }

    /// Kill and reap every child (also happens via kill_on_drop on exit).
    pub async fn shutdown(&self) {
        for (_, mut agent) in self.inner.lock().await.drain() {
            let _ = agent.child.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::agent_skills::{self, SkillSet};

    fn sample_project() -> LocalProject {
        LocalProject {
            id: "proj_test".into(),
            name: "Test Project".into(),
            slug: "test-project".into(),
            github_owner: "acme".into(),
            github_repo: "widget".into(),
            github_sync_enabled: true,
            baseline_branch: "main".into(),
            repo_path: "/tmp/nonexistent".into(),
            run_command: None,
            paper_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn sample_playbook() -> String {
        playbook_md(&sample_project(), &ProjectState::default())
    }

    /// The playbook's runtime placeholders must all resolve.
    #[test]
    fn playbook_has_no_unresolved_placeholders() {
        let md = sample_playbook();
        // Scanned, not listed: a NEWLY ADDED token playbook_md doesn't know
        // about is exactly the case a hardcoded list cannot catch, and the
        // agent would read the literal `{token}` as instruction.
        let leftover: Vec<&str> = md
            .lines()
            .flat_map(|line| {
                line.match_indices('{').filter_map(move |(i, _)| {
                    let rest = &line[i + 1..];
                    let end = rest.find('}')?;
                    let token = &rest[..end];
                    (!token.is_empty() && token.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
                        .then_some(&line[i..=i + end + 1])
                })
            })
            .collect();
        assert!(leftover.is_empty(), "unresolved placeholders: {leftover:?}");
        for retired in [
            "{files}",
            "{memory}",
            "{experiment_publish_clause}",
            "{edit_step}",
            "{compute_contract}",
            "{launch_step}",
            "{backends_intro}",
            "{run_invocation}",
            "{run_guidance}",
            "{compute_guidance}",
            "{skills_scope}",
            "{max_spawns}",
            "{skills_list}",
        ] {
            assert!(!md.contains(retired), "retired placeholder {retired}");
        }
        // The template's leading HTML comment (repo-reader documentation) must
        // be stripped — the prompt starts at the title.
        assert!(
            md.starts_with("# OpenResearch agent"),
            "template comment not stripped"
        );
        assert!(!md.contains("<!--"), "HTML comment leaked into the prompt");
        // Sanity: skill routing names every installed native skill without
        // duplicating the descriptions already surfaced by the harness.
        assert!(md.contains("Use the available OpenResearch skills"));
        assert!(md.contains("execute important user flows"));
        assert!(!md.contains("orx skill <name>"));
        for skill in agent_skills::skills(SkillSet::Local) {
            assert!(md.contains(&format!("- `{}`", skill.name)));
            assert!(!md.contains(skill.description));
        }
        assert!(md.contains("orx-compute"));
        assert!(md.contains("helping the user across the research process"));
        assert!(md.contains("The user's current project is **Test Project**"));
        assert!(!md.contains("running inside `orx up`"));
        assert!(!md.contains("## Memory"));
        assert!(!md.contains("User memory"));
        assert!(!md.contains("Project memory"));
    }

    #[test]
    fn playbook_keeps_runtime_facts_and_delegates_procedures() {
        let mut project = sample_project();
        project.github_owner.clear();
        project.github_repo.clear();
        let md = playbook_md(&project, &ProjectState::default());
        assert!(md.contains("default target"));
        assert!(md.contains("orx-compute"));
        assert!(md.contains("orx-instances"));
        assert!(!md.contains("## Cardinal rules"));
        assert!(!md.contains("## Command index"));
        assert!(!md.contains("## The auto-research loop"));
        assert!(!md.contains("## Compute backends"));
    }

    #[test]
    fn playbook_short_circuits_orientation_for_a_fresh_project() {
        let md = sample_playbook();
        assert!(md.contains("## Project state"));
        assert!(md.contains("fresh project: **0 experiments and 0 runs**"));
        assert!(md.contains("experiment tree is empty and no fixed run command is configured"));
        assert!(!md.contains("Do not inspect the tree"));

        let mut project = sample_project();
        project.run_command = Some("python train.py".into());
        let configured = playbook_md(&project, &ProjectState::default());
        assert!(
            configured.contains("experiment tree is empty and the fixed run command is configured")
        );
    }

    #[test]
    fn playbook_summarizes_existing_project_state_without_inlining_the_tree() {
        let mut project = sample_project();
        project.run_command = Some("python train.py".into());
        let state = ProjectState {
            experiments: 12,
            runs: 18,
            active_runs: 1,
        };
        let md = playbook_md(&project, &state);
        assert!(md.contains("**12 experiments**"));
        assert!(md.contains("**18 runs** (1 run active)"));
        assert!(md.contains("fixed run command is configured"));
        assert!(md.contains("orientation snapshot"));
        assert!(!md.contains("Experiment tree:"));
    }

    #[test]
    fn compute_skill_explains_opt_in_run_wakeups() {
        let md = sample_playbook();
        let compute = agent_skills::find("orx-compute", SkillSet::Local).unwrap();
        assert!(compute.content.contains("`orx exp wake <expId>`"));
        assert!(compute.content.contains("Wake-up is opt-in"));
        assert!(!md.contains("`orx exp wake <expId>`"));
        assert!(!md.contains("OpenResearch injects an `[orx]` message"));
    }

    #[test]
    fn reports_skill_owns_artifact_output_policy() {
        let md = sample_playbook();
        let reports = agent_skills::find("orx-reports", SkillSet::Local).unwrap();
        assert!(reports.content.contains("descriptive filename"));
        assert!(reports.content.contains("artifacts root"));
        assert!(!md.contains("PROJECT.md"));
    }

    #[test]
    fn playbook_owns_clickable_references() {
        let md = sample_playbook();
        let evidence = agent_skills::find("orx-evidence", SkillSet::Local).unwrap();
        assert!(md.contains("## Evidence and links in chat"));
        assert!(md.contains("<file path=\"relative/path.py\" />"));
        assert!(md.contains("exp=\"<experimentId>\""));
        assert!(md.contains("<run id=\"<runId>\" />"));
        assert!(md.contains("<file path=\"artifacts/<relative-path>\" />"));
        assert!(md.contains("Scholarly claims use the source links"));
        assert!(md.contains("Use `$...$` for inline math"));
        assert!(evidence.content.contains("Validate before reporting"));
        assert!(evidence
            .content
            .contains("Truncated output is not evidence of absence"));
        assert!(!evidence.content.contains("<file path="));
    }

    #[test]
    fn playbook_avoids_ui_navigation() {
        let mut local_only = sample_project();
        local_only.github_owner.clear();
        local_only.github_repo.clear();

        for md in [
            sample_playbook(),
            playbook_md(&local_only, &ProjectState::default()),
        ] {
            crate::local::assert_agent_guidance_is_ui_agnostic("playbook", &md);
        }
    }
}
