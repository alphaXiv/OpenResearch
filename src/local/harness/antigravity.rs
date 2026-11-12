//! Google Antigravity harness.
//!
//! Chat: one resident `agy acp` child per chat, driven over the Agent Client
//! Protocol like any ACP agent ([`super::acp`]). Its native session is agy's
//! conversation, so chats begun in print mode resume where they left off. Bypass
//! starts agy with `--dangerously-skip-permissions`; otherwise its permission
//! requests wait on the live connection. Plan mode is agy's own `plan` mode.
//!
//! agy's workspace hooks ([`write_hooks`], handled by `orx antigravity-gate`)
//! stamp shell commands with the invoking model, record each invocation, and
//! hold a turn open while commands it moved to the background still run: agy
//! finishes those in turns of its own that never reach an ACP client.
//!
//! A turn that agy ends on its own error reads the cause from agy's log, since
//! the prompt response reports it as a normal end of turn.
//!
//! The playbook is pointed at on the first turn (the file is already in the
//! worktree via [`ensure_playbook`]); session skills land in `.agents/skills`.
//!
//! Detection: `agy` on PATH or in `~/.local/bin`, `~/.gemini/bin`, or `~/.gemini/antigravity-cli/bin`;
//! `agy models` for catalog and authentication verification.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::detect::{resolve_symlinks, HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{
    HarnessOptions, OptionChoice, PermissionMode, PlanActivation, REASONING_DEFAULT_ID,
};
use super::{
    CompactCtx, CompactOutcome, Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult,
};
use crate::error::{anyhow, Context, Result};
use crate::local::acp::{host::Event, Definition};
use crate::local::chat::{prepare_env, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt};
use crate::local::opencode::ensure_playbook;
use crate::local::shell_env::installs;

/// What agy prints as the answer when it gives up on a turn.
const ERROR_MARKER: &str = "Agent execution terminated due to error.";
/// The Stop hook's own bound; it returns well before this.
const STOP_HOOK_TIMEOUT_SECS: u64 = 3600;

const AGY_REINSTALL: &str =
    "Reinstall Antigravity CLI via curl -fsSL https://antigravity.google/cli/install.sh | bash";
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);
const COMPACT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub struct Antigravity;

impl Antigravity {
    /// `snapshot` reports discovery only: `agy`'s auth *is* the model list,
    /// so a child-free pass marks the install pending and leaves readiness to
    /// the background full pass.
    async fn detect_at(&self, snapshot: bool) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        super::detect::record_selected(
            &mut info,
            snapshot,
            "antigravity",
            find_agy,
            find_agy_working(),
        )
        .await;
        // Nothing else about an installed agy is cheap to verify — `detect_one`
        // marks the snapshot answer pending. A missing install still falls
        // through to pick up its note.
        if snapshot && info.installed {
            return Some(info);
        }
        if info.installed && !info.install_broken {
            if let Some(bin) = info.bin_path.as_deref().map(Path::new) {
                match super::detect::timed_probe("antigravity", "models", agy_model_list(bin)).await
                {
                    Ok(models) => {
                        info.authenticated = true;
                        info.auth_state = HarnessAuthState::Ready;
                        info.auth_method = Some("oauth");
                        info.configured_model = crate::config::home_dir()
                            .and_then(|home| {
                                super::detect::read_json(
                                    home.join(".gemini/antigravity-cli/settings.json"),
                                )
                            })
                            .and_then(|settings| settings["model"].as_str().map(str::to_owned));
                        info = info.with_models(models);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        info.auth_state = if message.to_lowercase().contains("sign in") {
                            HarnessAuthState::NeedsLogin
                        } else {
                            HarnessAuthState::Unknown
                        };
                        info.agent_note = Some(message);
                    }
                }
            }
        }
        info.agent_ready = info.ready();
        if info.agent_ready {
            return Some(info);
        } else if info.install_broken {
            info.agent_note = Some(info.broken_note(AGY_REINSTALL));
        } else if info.installed && info.agent_note.is_none() {
            info.agent_note = Some(
                "Sign in by running `agy` in your terminal, then re-check this agent.".to_string(),
            );
        } else if !info.installed {
            info.agent_note = Some(
                "Install Antigravity CLI with `curl -fsSL https://antigravity.google/cli/install.sh | bash`, then sign in with `agy`."
                    .to_string(),
            );
        }
        Some(info)
    }
}

#[async_trait]
impl Harness for Antigravity {
    fn id(&self) -> &'static str {
        "antigravity"
    }

    fn name(&self) -> &'static str {
        "Google Antigravity"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    fn supports_compaction(&self) -> bool {
        true
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        self.detect_at(false).await
    }

    async fn detect_snapshot(&self) -> Option<HarnessInfo> {
        self.detect_at(true).await
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        match run_turn(ctx).await {
            Ok(()) => Ok(TurnOutcome::Completed),
            Err(error) => {
                ctx.host.acp.stop(&ctx.session_id).await;
                Err(TurnFailure::adapter(error, ctx.delivery_state()))
            }
        }
    }

    async fn compact(&self, ctx: &CompactCtx) -> Result<CompactOutcome> {
        let native_id = ctx
            .native_session_id
            .clone()
            .ok_or_else(|| anyhow!("This Antigravity chat has no saved conversation to compact"))?;
        let result = compact(ctx, &native_id).await;
        if result.is_err() {
            ctx.host.acp.stop(&ctx.session_id).await;
        }
        result.map(|()| CompactOutcome::Native)
    }

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none().with_permission_choices(
            vec![
                OptionChoice::described(
                    "default",
                    "Ask for approval",
                    "Ask before changes; allow read-only planning",
                ),
                OptionChoice::described(
                    "bypass",
                    "Bypass permissions",
                    "Allow commands and skip tool confirmation prompts",
                ),
            ],
            "bypass",
            PlanActivation::Command,
        )
    }

    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        if prompt.kind == "permission" {
            let id = prompt
                .native_id
                .as_deref()
                .ok_or_else(|| anyhow!("This approval is no longer pending"))?;
            let selected = match answer.answers.as_slice() {
                [selected] => selected.clone(),
                _ => permission_choice(prompt, answer.approve)
                    .ok_or_else(|| anyhow!("Select an agent permission option"))?,
            };
            ctx.host.acp.answer(&ctx.session_id, id, &selected).await?;
            return Ok(ResumeAction::Handled { plan_mode: None });
        }
        if prompt.kind != "plan" {
            return Ok(ResumeAction::Nothing);
        }
        if !answer.approve && answer.note.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Ok(ResumeAction::Nothing);
        }
        Ok(ResumeAction::SendMessage {
            text: super::synthesize_resume("plan", answer).0,
            mode: None,
            plan_mode: Some(!answer.approve),
        })
    }

    fn config_home(&self) -> Option<PathBuf> {
        Some(
            crate::config::home_dir()?
                .join(".gemini")
                .join("antigravity-cli"),
        )
    }

    fn native_skill_roots(&self) -> Vec<PathBuf> {
        self.config_home()
            .map(|home| vec![home.join("skills")])
            .unwrap_or_default()
    }

    fn skill_shim(&self) -> Option<&'static str> {
        Some(super::CLAUDE_SKILL)
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(super::acp::SESSION_SKILLS_DIR)
    }
}

/// The agent's option for an approve or deny answer that names none, as older
/// clients send.
fn permission_choice(prompt: &WirePrompt, approve: bool) -> Option<String> {
    let kind = if approve { "allow_once" } else { "reject_once" };
    prompt
        .native_choices
        .iter()
        .find(|choice| choice.kind == kind)
        .map(|choice| choice.id.clone())
}

/// Every `agy` install, in preference order: Antigravity's own locations are
/// `~/.gemini/bin`, `~/.gemini/antigravity-cli/bin` and the local data dir.
pub(crate) fn agy_candidates() -> Vec<PathBuf> {
    let gemini = crate::config::home_dir().map(|home| home.join(".gemini"));
    let own = gemini
        .iter()
        .flat_map(|gemini| {
            [
                gemini.join("bin"),
                gemini.join("antigravity-cli").join("bin"),
            ]
        })
        .chain(crate::config::data_local_dir().map(|dir| dir.join("agy").join("bin")))
        .collect::<Vec<_>>();
    installs("agy", own)
        .into_iter()
        .map(resolve_symlinks)
        .collect()
}

/// The executable detection selected, else the first candidate — sync callers
/// cannot probe, and must not spawn a launcher detection already skipped.
pub(crate) fn find_agy() -> Option<PathBuf> {
    super::detect::selected_bin("antigravity", agy_candidates())
}

/// The first candidate that actually runs, with its version probe.
pub(super) async fn find_agy_working() -> Option<(PathBuf, super::detect::BinProbe)> {
    super::detect::select_working("antigravity", agy_candidates(), None).await
}

async fn agy_model_list(bin: &Path) -> Result<Vec<ModelInfo>> {
    let mut cmd = crate::process::tokio_command(bin);
    cmd.arg("models")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    let out = super::detect::detect_spawn_output_timed(cmd, MODELS_TIMEOUT)
        .await
        .ok_or_else(|| {
            anyhow!("Antigravity model discovery timed out. Re-check when connected.")
        })??;
    if !out.status.success() {
        let error = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!(
            "Antigravity model discovery failed: {}",
            error.trim()
        ));
    }
    let models = parse_agy_model_list(&String::from_utf8_lossy(&out.stdout));
    if models.is_empty() {
        return Err(anyhow!(
            "Antigravity returned no available models. Re-check your account."
        ));
    }
    Ok(models)
}

/// Parse the whitespace-separated model catalog, ignoring
/// informational banner lines such as `Fetching available models...`.
fn parse_agy_model_list(text: &str) -> Vec<ModelInfo> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with("Fetching")
                || line.starts_with("Available")
                || line.starts_with("Listing")
            {
                return None;
            }
            let (id, label) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            let label = label.trim();
            if id.is_empty()
                || id == REASONING_DEFAULT_ID
                || !id.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '-' | '_' | '.' | '[' | ']' | '=' | ',')
                })
            {
                return None;
            }
            Some(ModelInfo::new(id).with_label((!label.is_empty()).then_some(label), None))
        })
        .collect()
}

/// agy's log for the chat's resident process.
fn log_path(session_id: &str) -> PathBuf {
    crate::store::data_dir().join(format!("agent-antigravity-{session_id}.log"))
}

/// Where agy's `Stop` hook leaves the error a turn ended on, which the prompt
/// response itself never carries.
pub(crate) fn stop_error_path(session_id: &str) -> PathBuf {
    crate::store::data_dir().join(format!("agent-antigravity-{session_id}.error.json"))
}

/// Present while the chat's agy asks before running tools, which its subagents cannot do over ACP.
pub(crate) fn approvals_path(session_id: &str) -> PathBuf {
    crate::store::data_dir().join(format!("agent-antigravity-{session_id}.approvals"))
}

/// The error the `Stop` hook saved for this conversation's turn, consumed.
fn take_stop_error(session_id: &str, conversation: &str) -> Option<String> {
    let path = stop_error_path(session_id);
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    let _ = std::fs::remove_file(path);
    (saved["conversationId"] == conversation)
        .then(|| saved["error"].as_str().map(str::to_owned))
        .flatten()
}

/// The resident `agy acp` launch for a chat. A change restarts the process with
/// its conversation resumed.
fn launch(bin: &Path, session_id: &str, skip_permissions: bool) -> Definition {
    let mut arguments = vec![
        "acp".to_string(),
        "--log-file".to_string(),
        log_path(session_id).to_string_lossy().into_owned(),
    ];
    if skip_permissions {
        arguments.push("--dangerously-skip-permissions".into());
    }
    Definition {
        id: "antigravity".into(),
        name: Antigravity.name().into(),
        executable: bin.to_string_lossy().into_owned(),
        arguments,
        unknown: Default::default(),
    }
}

/// The chat's worktree with the playbook, skills and agy's hooks in place.
async fn prepare_worktree(
    project: &crate::local::model::LocalProject,
    session_id: &str,
    skip_permissions: bool,
) -> Result<PathBuf> {
    // Side chats can share the worktree, so its hook file cannot hold a chat's mode.
    let approvals = approvals_path(session_id);
    let recorded = if skip_permissions {
        match std::fs::remove_file(&approvals) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    } else {
        std::fs::write(&approvals, "")
    };
    recorded.with_context(|| {
        format!(
            "could not record this chat's approval mode at {}",
            approvals.display()
        )
    })?;
    let project = project.clone();
    let session_id = session_id.to_owned();
    let (repo, _playbook) = tokio::task::spawn_blocking(move || {
        ensure_playbook(&project, &session_id, Some(super::acp::SESSION_SKILLS_DIR))
    })
    .await
    .map_err(|e| anyhow!("playbook task failed: {e}"))??;
    write_hooks(&repo)?;
    Ok(repo)
}

fn find_agy_or_error() -> Result<PathBuf> {
    find_agy()
        .ok_or_else(|| anyhow!("agy not found on PATH — install Antigravity CLI and sign in first"))
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    let log = log_path(&ctx.session_id);
    if ctx.host.acp.connected(&ctx.session_id).await.is_err() {
        // No process writes it: each one starts its own.
        let _ = std::fs::remove_file(&log);
    }
    let logged = std::fs::metadata(&log).map_or(0, |metadata| metadata.len());
    // Offline, agy blames a consequence (an empty catalog has no such model).
    turn(ctx, &log, logged)
        .await
        .map_err(|error| match logged_network_failure(&log, logged) {
            Some(cause) => anyhow!("{error}. Antigravity logged: {cause}"),
            None => error,
        })
}

async fn turn(ctx: &mut TurnCtx, log: &Path, logged: u64) -> Result<()> {
    let bin = find_agy_or_error()?;
    let bypass = ctx.permission_mode.unwrap_or(PermissionMode::Bypass) == PermissionMode::Bypass;
    let skip_permissions = bypass && !ctx.plan_mode;
    let repo = prepare_worktree(&ctx.project, &ctx.session_id, skip_permissions).await?;
    let definition = launch(&bin, &ctx.session_id, skip_permissions);
    let session = super::acp::open_session(ctx, &definition, &repo, false).await?;
    let native_id = session.native_id.clone();
    if let Some(model) = ctx
        .model
        .as_deref()
        .filter(|model| !model.is_empty() && *model != REASONING_DEFAULT_ID)
    {
        session
            .client
            .request(
                "session/set_model",
                json!({"sessionId": native_id, "modelId": model}),
            )
            .await
            .map_err(|error| anyhow!("Antigravity could not select model {model}: {error}"))?;
    }
    session
        .client
        .request(
            "session/set_mode",
            json!({"sessionId": native_id, "modeId": if ctx.plan_mode { "plan" } else { "default" }}),
        )
        .await?;
    let _ = std::fs::remove_file(stop_error_path(&ctx.session_id));
    let started = crate::store::now_ms();
    let result = super::acp::prompt_session(ctx, &session).await?;
    if let Some(usage) = turn_usage(&result) {
        let model = turn_model(&ctx.session_id, started);
        let sample = format!("antigravity-acp-{}", ctx.attempt_count_for_usage());
        ctx.record_native_usage(&sample, model.as_deref(), None, usage);
    }
    if take_error_marker(ctx) {
        let cause = take_stop_error(&ctx.session_id, &native_id)
            .or_else(|| logged_network_failure(log, logged));
        let message = match cause {
            Some(cause) => format!("Antigravity stopped with an error: {cause}"),
            None => format!("Antigravity stopped with an error; see {}", log.display()),
        };
        ctx.mark_terminal_failure("antigravity_terminal", message);
        ctx.flush()?;
        return Ok(());
    }
    if ctx.plan_mode {
        if let Some(card) = plan_card(&ctx.assistant.parts, &ctx.assistant.id) {
            ctx.upsert_part(card);
            ctx.flush()?;
        }
    }
    Ok(())
}

/// Removes agy's give-up line from the answer; true when it ended the turn.
fn take_error_marker(ctx: &mut TurnCtx) -> bool {
    let Some(part) = ctx
        .assistant
        .parts
        .iter()
        .rev()
        .find(|part| part.kind == "text")
        .filter(|part| {
            part.text
                .as_deref()
                .is_some_and(|text| text.contains(ERROR_MARKER))
        })
        .cloned()
    else {
        return false;
    };
    let text = part.text.as_deref().unwrap_or_default();
    let kept = text
        .replace(&format!("⚠ {ERROR_MARKER}"), "")
        .replace(ERROR_MARKER, "");
    if kept.trim().is_empty() {
        ctx.assistant
            .parts
            .retain(|existing| existing.id != part.id);
    } else {
        ctx.upsert_part(WirePart::text(&part.id, kept.trim_end().to_string()));
    }
    true
}

/// The first network failure agy logged after `offset`. agy fails before its
/// hooks run when it cannot reach Google, and first reports that as a missing
/// sign-in or an unknown model; its other errors include startup noise.
fn logged_network_failure(log: &Path, offset: u64) -> Option<String> {
    use std::io::{Read as _, Seek as _};
    let mut file = std::fs::File::open(log).ok()?;
    // A restarted agy writes the log afresh.
    let offset = if file.metadata().ok()?.len() < offset {
        0
    } else {
        offset
    };
    file.seek(std::io::SeekFrom::Start(offset)).ok()?;
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(logged_error)
        .find(|message| {
            NETWORK_FAILURES
                .iter()
                .any(|failure| message.contains(failure))
        })
        .map(|message| {
            let mut message = message.trim().to_string();
            if message.len() > 600 {
                let end = (0..=600)
                    .rev()
                    .find(|end| message.is_char_boundary(*end))
                    .unwrap_or(0);
                message.truncate(end);
                message.push('…');
            }
            message
        })
}

/// Go's wording for a connection that never reached Google.
const NETWORK_FAILURES: &[&str] = &[
    "dial tcp",
    "connection refused",
    "no such host",
    "network is unreachable",
    "i/o timeout",
    "TLS handshake timeout",
];

/// The message of a glog error line (`E1109 14:48:42.864 314 file.go:273] message`).
fn logged_error(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('E').or_else(|| line.strip_prefix('F'))?;
    if !rest.get(..4)?.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let (_, message) = rest.split_once("] ")?;
    (!message.trim().is_empty()).then_some(message)
}

/// The native model that ran the turn, from what agy's `PostInvocation` hook
/// recorded for the chat while it ran (the hook finishes before agy answers the
/// prompt). Several models leave the turn's single total unattributed.
fn turn_model(session_id: &str, started: i64) -> Option<String> {
    let models = crate::store::Store::open()
        .and_then(|store| store.chat_invocation_models("antigravity", session_id, started))
        .inspect_err(|error| {
            eprintln!("orx up: could not read Antigravity's invocation models: {error}")
        })
        .ok()?;
    match models.as_slice() {
        [model] => Some(model.clone()),
        _ => None,
    }
}

/// The turn's counters from the prompt response; agy reports each prompt alone.
fn turn_usage(result: &Value) -> Option<crate::store::TokenUsage> {
    let usage = &result["usage"];
    let cache_read = usage["cachedReadTokens"].as_u64();
    let cache_write = usage["cachedWriteTokens"].as_u64();
    let tokens = crate::store::TokenUsage {
        input_tokens: Some(
            usage["inputTokens"]
                .as_u64()?
                .checked_add(cache_read.unwrap_or(0))?
                .checked_add(cache_write.unwrap_or(0))?,
        ),
        output_tokens: Some(usage["outputTokens"].as_u64()?),
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        reasoning_tokens: usage["thoughtTokens"].as_u64(),
    };
    (tokens.total()? == usage["totalTokens"].as_u64()? && tokens.validate().is_ok())
        .then_some(tokens)
}

/// agy's `/compact`, sent to the chat's resident process, which holds the
/// conversation.
async fn compact(ctx: &CompactCtx, native_id: &str) -> Result<()> {
    let bin = find_agy_or_error()?;
    let bypass = ctx.permission_mode.unwrap_or(PermissionMode::Bypass) == PermissionMode::Bypass;
    let repo = prepare_worktree(&ctx.project, &ctx.session_id, bypass).await?;
    let definition = launch(&bin, &ctx.session_id, bypass);
    let host = ctx.host.acp.clone();
    let mut client = host.get(&ctx.session_id, &definition, &repo).await?;
    if client.native_id().is_some_and(|id| id != native_id) {
        host.stop(&ctx.session_id).await;
        client = host.get(&ctx.session_id, &definition, &repo).await?;
    }
    if client.native_id().is_none() {
        client
            .restore(
                native_id,
                &repo,
                super::acp::orx_mcp_servers(&ctx.session_id, ctx.host.up_port())?,
            )
            .await
            .map_err(|error| {
                anyhow!("Antigravity could not reopen this conversation to compact it: {error}")
            })?;
    }
    struct Idle<'a>(&'a crate::local::acp::host::Client);
    impl Drop for Idle<'_> {
        fn drop(&mut self) {
            self.0.end_turn();
        }
    }
    let log = log_path(&ctx.session_id);
    let logged = std::fs::metadata(&log).map_or(0, |metadata| metadata.len());
    let _ = std::fs::remove_file(stop_error_path(&ctx.session_id));
    let mut events = client.begin_turn();
    let _idle = Idle(&client);
    let result = tokio::time::timeout(
        COMPACT_TIMEOUT,
        client.request(
            "session/prompt",
            json!({"sessionId": native_id, "prompt": [{"type": "text", "text": "/compact"}]}),
        ),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "Antigravity did not finish compacting within {}s",
            COMPACT_TIMEOUT.as_secs()
        )
    })??;
    let mut answer = String::new();
    while let Ok(event) = events.try_recv() {
        if let Event::Update(update) = event {
            if update["sessionUpdate"] == "agent_message_chunk" {
                answer.push_str(update["content"]["text"].as_str().unwrap_or_default());
            }
        }
    }
    compaction_outcome(&result, &answer, || {
        take_stop_error(&ctx.session_id, native_id)
            .or_else(|| logged_network_failure(&log, logged))
            .unwrap_or_else(|| format!("see {}", log.display()))
    })
}

fn compaction_outcome(result: &Value, answer: &str, cause: impl FnOnce() -> String) -> Result<()> {
    match result["stopReason"].as_str() {
        Some("end_turn") if !answer.contains(ERROR_MARKER) => Ok(()),
        Some("end_turn") => Err(anyhow!(
            "Antigravity could not compact this conversation: {}",
            cause()
        )),
        Some(reason) => Err(anyhow!("Antigravity stopped compacting: {reason}")),
        None => Err(anyhow!(
            "Antigravity answered the compaction without a stop reason"
        )),
    }
}

/// agy's workspace hooks for this chat, merged into the worktree's untracked
/// `.agents/hooks.json` (a tracked one is the project's and stays untouched).
fn write_hooks(repo: &Path) -> Result<()> {
    std::fs::create_dir_all(repo)?;
    let tracked = crate::local::git::git_command()
        .args(["ls-files", "--error-unmatch", ".agents/hooks.json"])
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if tracked.success() {
        return Err(anyhow!("This project tracks .agents/hooks.json. Antigravity needs OpenResearch's hooks in an untracked hook file and will not modify the tracked file."));
    }
    let path = repo.join(".agents/hooks.json");
    let mut hooks = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(error.into()),
    };
    let object = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("Antigravity hooks must be an object"))?;
    let exe = crate::paths::spawnable_exe()?;
    #[cfg(not(windows))]
    let command = format!(
        "{} antigravity-gate",
        crate::jobs::ssh::sh_quote(&exe.to_string_lossy())
    );
    #[cfg(windows)]
    let command = {
        anyhow::ensure!(
            cfg!(test)
                || exe
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("orx.exe")),
            "Antigravity's OpenResearch hooks require the executable name orx.exe"
        );
        // prepare_env puts this executable's directory first on PATH.
        "orx antigravity-gate".to_string()
    };
    // Print-mode chats relayed approvals through this hook; ACP holds them on the connection.
    object.remove("openresearch-approval");
    object.insert(
        "openresearch-invocation".into(),
        json!({
            // The stamp is a POSIX shell export.
            "enabled": !cfg!(windows),
            "PreToolUse": [{"matcher": "run_command", "hooks": [{"type": "command", "command": command, "timeout": 60}]}]
        }),
    );
    // agy asks about a subagent's tools only in its own terminal, never over ACP.
    object.insert(
        "openresearch-subagents".into(),
        json!({
            "enabled": true,
            "PreToolUse": [{"matcher": "invoke_subagent", "hooks": [{"type": "command", "command": command, "timeout": 60}]}]
        }),
    );
    object.insert(
        "openresearch-accounting".into(),
        json!({"enabled": true, "PostInvocation": [{"type": "command", "command": command}]}),
    );
    object.insert(
        "openresearch-background".into(),
        json!({
            "enabled": true,
            "Stop": [{"type": "command", "command": command, "timeout": STOP_HOOK_TIMEOUT_SECS}]
        }),
    );
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_vec_pretty(&hooks)?)?;
    Ok(())
}

pub(crate) fn normalize_tool<'a>(
    name: &'a str,
    params: Option<&Value>,
) -> (&'a str, Option<Value>) {
    let (tool, aliases): (&str, &[(&str, &str)]) = match name {
        "invoke_subagent" => ("task", &[("Role", "description")]),
        "run_command" => ("Bash", &[("CommandLine", "command"), ("Cwd", "cwd")]),
        "view_file" => ("Read", &[("AbsolutePath", "file_path")]),
        "read_file" => ("Read", &[("Path", "file_path")]),
        "write_file" => ("Write", &[("Path", "file_path"), ("Content", "content")]),
        "edit_file" => (
            "Edit",
            &[
                ("Path", "file_path"),
                ("Target", "old_string"),
                ("Replacement", "new_string"),
            ],
        ),
        "write_to_file" => (
            "Write",
            &[("TargetFile", "file_path"), ("CodeContent", "content")],
        ),
        "replace_file_content" | "multi_replace_file_content" => {
            ("Edit", &[("TargetFile", "file_path")])
        }
        "list_dir" => ("Glob", &[("DirectoryPath", "path")]),
        "grep_search" | "code_search" => ("Grep", &[("Query", "pattern"), ("SearchPath", "path")]),
        "find_by_name" => (
            "Glob",
            &[("Pattern", "pattern"), ("SearchDirectory", "path")],
        ),
        "read_url_content" => ("WebFetch", &[("Url", "url")]),
        "search_web" => ("WebSearch", &[]),
        _ => (name, &[]),
    };
    let mut input = params.cloned();
    if let Some(object) = input.as_mut().and_then(Value::as_object_mut) {
        for &(native, normalized) in aliases {
            if let Some(value) = object.get(native).cloned() {
                object.insert(normalized.into(), value);
            }
        }
    }
    (tool, input)
}

pub(crate) fn invocation_sample_id(conversation: &str, step: i64) -> String {
    format!("antigravity:{conversation}:step:{step}")
}

/// The plan card for a plan-mode turn: agy's plan artifact (the file it asks
/// feedback on), else the turn's last text.
fn plan_card(parts: &[WirePart], assistant_id: &str) -> Option<WirePart> {
    let artifact = parts.iter().rev().find_map(|part| {
        let input = part.state.as_ref()?.input.as_ref()?;
        (input.pointer("/ArtifactMetadata/RequestFeedback") == Some(&Value::Bool(true)))
            .then(|| input["Content"].as_str())
            .flatten()
            .filter(|plan| !plan.trim().is_empty())
    });
    let plan = match artifact {
        Some(plan) => plan,
        None => {
            let last_text = parts.iter().rev().find_map(|part| {
                (part.kind == "text")
                    .then_some(part.text.as_deref())
                    .flatten()
                    .filter(|text| !text.trim().is_empty())
            })?;
            if !super::should_synthesize_plan(true, false, false, last_text) {
                return None;
            }
            last_text
        }
    };
    Some(WirePart::prompt(
        format!("plan-synth-{assistant_id}"),
        WirePrompt {
            kind: "plan".into(),
            plan: Some(plan.to_string()),
            synthesized: true,
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hooks_leave_a_tracked_hook_file_untouched() {
        let repo = std::env::temp_dir().join(format!("orx-agy-hooks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(repo.join(".agents")).unwrap();
        let path = repo.join(".agents/hooks.json");
        std::fs::write(&path, "{}").unwrap();
        for args in [&["init", "--quiet"][..], &["add", ".agents/hooks.json"]] {
            assert!(crate::process::command("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        }
        assert!(write_hooks(&repo)
            .unwrap_err()
            .to_string()
            .contains("tracks .agents/hooks.json"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{}");
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn hooks_stamp_commands_account_invocations_and_wait_for_background_work() {
        let repo = std::env::temp_dir().join(format!("orx-agy-hooks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(repo.join(".agents")).unwrap();
        let path = repo.join(".agents/hooks.json");
        // A print-mode chat's relay, and the user's own hook, from before.
        std::fs::write(
            &path,
            r#"{"openresearch-approval":{"enabled":true},"mine":{"enabled":true}}"#,
        )
        .unwrap();
        write_hooks(&repo).unwrap();
        let hooks: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(hooks["openresearch-subagents"]["enabled"], true);
        assert_eq!(
            hooks["openresearch-subagents"]["PreToolUse"][0]["matcher"],
            "invoke_subagent"
        );
        assert!(hooks.get("openresearch-approval").is_none());
        assert_eq!(hooks["mine"]["enabled"], true);
        assert_eq!(hooks["openresearch-invocation"]["enabled"], !cfg!(windows));
        assert_eq!(
            hooks["openresearch-invocation"]["PreToolUse"][0]["matcher"],
            "run_command"
        );
        assert_eq!(hooks["openresearch-accounting"]["enabled"], true);
        let stop = &hooks["openresearch-background"];
        assert_eq!(stop["enabled"], true);
        assert!(stop["Stop"][0]["command"]
            .as_str()
            .unwrap()
            .ends_with("antigravity-gate"));
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn launch_skips_permissions_only_when_asked() {
        let bin = std::env::temp_dir().join("agy");
        let ask = launch(&bin, "chat", false);
        let bypass = launch(&bin, "chat", true);
        assert_eq!(ask.id, "antigravity");
        assert_eq!(ask.arguments[0], "acp");
        assert!(!ask
            .arguments
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"));
        assert!(bypass
            .arguments
            .iter()
            .any(|arg| arg == "--dangerously-skip-permissions"));
        assert_ne!(ask, bypass, "a mode change restarts the resident agent");
        ask.validate_launch().unwrap();
    }

    #[test]
    fn a_turn_agy_gave_up_on_fails_with_the_logged_network_failure() {
        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text(
            "t",
            "\n⚠ Agent execution terminated due to error.\n",
        ));
        assert!(take_error_marker(&mut ctx));
        assert!(ctx.assistant.parts.is_empty());

        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text(
            "t",
            "Partial answer.\n⚠ Agent execution terminated due to error.\n",
        ));
        assert!(take_error_marker(&mut ctx));
        assert_eq!(
            ctx.assistant.parts[0].text.as_deref(),
            Some("Partial answer.")
        );

        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text("t", "PONG"));
        assert!(!take_error_marker(&mut ctx));

        let log = std::env::temp_dir().join(format!("orx-agy-log-{}", uuid::Uuid::new_v4()));
        // An earlier turn's failure, and startup noise that is no cause.
        let earlier = "E1109 14:40:00.000000     1 errorreport.go:273] dial tcp: no such host\n\
                       E1109 14:48:41.000000    54 launcher.go:249] Failed to resolve GeminiDir\n";
        std::fs::write(
            &log,
            format!("{earlier}I1109 14:48:42.000000   314 server.go:1] starting\n\
                     W1109 14:48:42.864603   314 cache.go:157] refresh failed\n\
                     E1109 14:48:42.539824    86 errorreport.go:273] error getting token source: You are not logged into Antigravity.\n\
                     E1109 14:48:42.864613   314 errorreport.go:273] failed to get load code assist response: dial tcp 127.0.0.1:9: connect: connection refused\n\
                     E1109 14:48:42.868674   315 errorreport.go:273] plan model not specified\n"),
        )
        .unwrap();
        assert_eq!(
            logged_network_failure(&log, earlier.len() as u64).as_deref(),
            Some("failed to get load code assist response: dial tcp 127.0.0.1:9: connect: connection refused")
        );
        assert_eq!(
            logged_network_failure(&log, std::fs::metadata(&log).unwrap().len()),
            None
        );
        assert_eq!(
            logged_network_failure(&log.with_extension("missing"), 0),
            None
        );
        std::fs::write(&log, &earlier[earlier.find('\n').unwrap() + 1..]).unwrap();
        assert_eq!(logged_network_failure(&log, 0), None);
        std::fs::remove_file(log).unwrap();
    }

    #[test]
    fn the_stop_hooks_error_belongs_to_its_own_conversation_once() {
        let session = format!("chat-{}", uuid::Uuid::new_v4());
        std::fs::create_dir_all(crate::store::data_dir()).unwrap();
        let save = |conversation: &str| {
            std::fs::write(
                stop_error_path(&session),
                json!({"conversationId": conversation, "error": "RESOURCE_EXHAUSTED: quota"})
                    .to_string(),
            )
            .unwrap()
        };
        save("other");
        assert_eq!(take_stop_error(&session, "mine"), None);
        assert!(!stop_error_path(&session).exists());
        save("mine");
        assert_eq!(
            take_stop_error(&session, "mine").as_deref(),
            Some("RESOURCE_EXHAUSTED: quota")
        );
        assert_eq!(take_stop_error(&session, "mine"), None);
    }

    #[test]
    fn turn_usage_counts_cache_as_input_and_rejects_inconsistent_counters() {
        let result = json!({"stopReason": "end_turn", "usage": {
            "inputTokens": 4599, "outputTokens": 245, "cachedReadTokens": 11398,
            "cachedWriteTokens": 0, "thoughtTokens": 86, "totalTokens": 16242
        }});
        let usage = turn_usage(&result).unwrap();
        assert_eq!(usage.input_tokens, Some(15997));
        assert_eq!(usage.reasoning_tokens, Some(86));
        assert_eq!(usage.total(), Some(16242));
        let mut inconsistent = result.clone();
        inconsistent["usage"]["totalTokens"] = json!(1);
        assert!(turn_usage(&inconsistent).is_none());
        assert!(turn_usage(&json!({"stopReason": "end_turn"})).is_none());
    }

    #[test]
    fn compaction_succeeds_only_on_a_clean_end_of_turn() {
        let done = json!({"stopReason": "end_turn"});
        compaction_outcome(&done, "", || unreachable!()).unwrap();
        let error = compaction_outcome(
            &done,
            "⚠ Agent execution terminated due to error.",
            || "quota".into(),
        );
        assert!(error.unwrap_err().to_string().contains("quota"));
        assert!(compaction_outcome(&json!({"stopReason": "cancelled"}), "", String::new).is_err());
        assert!(compaction_outcome(&json!({}), "", String::new).is_err());
    }

    #[test]
    fn the_plan_card_shows_agys_plan_artifact() {
        let mut ctx = TurnCtx::test_stub();
        let mut edit = WirePart::tool("plan", "edit", "completed", None);
        edit.state.as_mut().unwrap().input = Some(json!({
            "ArtifactMetadata": {"RequestFeedback": true},
            "Content": "# Plan\n1. Add planned.txt",
            "Path": "/home/u/.gemini/antigravity-cli/brain/c/plan.md"
        }));
        ctx.upsert_part(edit);
        ctx.upsert_part(WirePart::text("t", "Please review and approve the plan."));
        let card = plan_card(&ctx.assistant.parts, "msg").unwrap();
        assert_eq!(
            card.prompt.unwrap().plan.as_deref(),
            Some("# Plan\n1. Add planned.txt")
        );

        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text(
            "t",
            "Plan:\n1. Add planned.txt\n2. Commit it",
        ));
        let card = plan_card(&ctx.assistant.parts, "msg").unwrap();
        assert!(card.prompt.unwrap().plan.unwrap().contains("Commit it"));
    }

    #[test]
    fn unnamed_answers_pick_the_matching_permission_option() {
        let prompt = WirePrompt {
            native_choices: vec![
                crate::local::chat::NativePermissionChoice {
                    id: "allow".into(),
                    label: "Allow".into(),
                    kind: "allow_once".into(),
                },
                crate::local::chat::NativePermissionChoice {
                    id: "deny".into(),
                    label: "Deny".into(),
                    kind: "reject_once".into(),
                },
            ],
            ..Default::default()
        };
        assert_eq!(permission_choice(&prompt, true).as_deref(), Some("allow"));
        assert_eq!(permission_choice(&prompt, false).as_deref(), Some("deny"));
        assert_eq!(permission_choice(&WirePrompt::default(), true), None);
    }

    #[test]
    fn parses_agy_model_list_output() {
        let sample = "Fetching available models...\n\
                      gemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
                      gemini-3.1-pro-high    Gemini 3.1 Pro (High)\n\
                      claude-sonnet-4-6\tClaude Sonnet 4.6 (Thinking)\n";
        let models = parse_agy_model_list(sample);
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "gemini-3.8-flash-high");
        assert_eq!(
            models[0].display_name.as_deref(),
            Some("Gemini 3.8 Flash (High)")
        );
        assert_eq!(models[1].id, "gemini-3.1-pro-high");
        assert_eq!(models[2].id, "claude-sonnet-4-6");
    }
}
