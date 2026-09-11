//! Cursor harness.
//!
//! Chat: one `agent --print --output-format stream-json` child per turn. Multi-turn
//! continues via `--resume <session_id>` from the init/result `session_id`. Isolated
//! ORX worktrees are the child's `--workspace` — Cursor's own `--worktree` flag is
//! not used, so the dashboard sees the same checkout the agent edits.
//!
//! The playbook is pointed at on the first turn (the file is already in the
//! worktree via [`ensure_playbook`]); session skills land in `.cursor/skills`.
//! Print mode cannot prompt, so Ask is `--mode ask` (read-only), Auto
//! `--force`s commands, and Full access also disables the sandbox (a denial
//! has nothing to escalate to).
//!
//! Detection: `cursor-agent` / `agent` on PATH; `agent status --format json` plus
//! `CURSOR_API_KEY` for login; `agent models` for the catalog.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::detect::{
    api_key, nonempty_str, probe_bin, read_json, resolve_symlinks, title_case, HarnessAuthState,
    HarnessInfo, ModelInfo,
};
use super::options::{
    HarnessOptions, OptionChoice, PermissionMode, PlanActivation, REASONING_DEFAULT_ID,
};
use super::{
    Harness, OneShot, OneShotQuality, ResumeAction, TurnFailure, TurnOutcome, TurnResult,
    TURN_WATCHDOG,
};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, harness_log, prepare_env, set_chat_session_env, DeliveryState, PromptAnswer,
    ResumeCtx, TurnCtx, WirePart, WirePrompt, WireToolState,
};
use crate::local::native_store::{self, NativeStore};
use crate::local::opencode::{ensure_playbook, PLAYBOOK_REL};
use crate::local::shell_env::{find_in_dir, find_on_path};

const CURSOR_REINSTALL: &str = "Reinstall it from cursor.com/install";
const AUTH_STATUS_TIMEOUT: Duration = Duration::from_secs(10);
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);

/// Fallback catalog when `agent models` cannot run. Ids match the CLI's
/// `--model` examples and the current default Composer/Grok aliases.
const CURSOR_MODELS: [&str; 5] = [
    "auto",
    "composer-2.5",
    "grok-4.6",
    "gpt-5",
    "sonnet-4-thinking",
];

const CURSOR_EFFORT_LEVELS: [&str; 3] = ["low", "medium", "high"];

pub struct Cursor;

#[async_trait]
impl Harness for Cursor {
    fn id(&self) -> &'static str {
        "cursor"
    }

    fn name(&self) -> &'static str {
        "Cursor"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        if let Some(bin) = find_cursor() {
            info.record_bin(&bin, probe_bin(&bin).await);
        }
        if info.installed && !info.install_broken {
            let bin = info.bin_path.as_deref().map(Path::new);
            let (status, about) = match bin {
                Some(bin) => {
                    tokio::join!(
                        cursor_status_json(bin),
                        cursor_command_json(bin, &["about", "--format", "json"])
                    )
                }
                None => (None, None),
            };
            apply_auth(&mut info, status.as_ref(), about.as_ref());
        }

        info.agent_ready = info.ready();
        if info.agent_ready {
            let models = match info.bin_path.as_deref().map(Path::new) {
                Some(bin) => cursor_model_list(bin).await,
                None => None,
            };
            info = info.with_models(models.unwrap_or_else(fallback_models));
        } else if info.install_broken {
            info.agent_note = Some(info.broken_note(CURSOR_REINSTALL));
        } else if info.installed {
            info.agent_note = Some(
                if api_key("CURSOR_API_KEY").is_some() {
                    "Cursor could not verify `CURSOR_API_KEY`. Fix or unset it, then re-check this harness."
                } else {
                    "Sign in with `agent login`, then re-check this harness."
                }
                .to_string(),
            );
        } else {
            info.agent_note = Some(
                "Install Cursor CLI (curl https://cursor.com/install -fsS | bash), then sign in with `agent login`."
                    .to_string(),
            );
        }
        Some(info)
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        run_turn(ctx)
            .await
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    async fn one_shot(&self, request: OneShot<'_>) -> Option<String> {
        cursor_one_shot(&find_cursor()?, request).await
    }

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none()
            .with_permission_choices(
                vec![
                    OptionChoice::described("ask", "Ask", "Propose changes without applying them"),
                    OptionChoice::described(
                        "auto",
                        "Auto",
                        "Allow commands unless explicitly denied",
                    ),
                    OptionChoice::described(
                        "full-access",
                        "Full access",
                        "Allow commands and disable the sandbox",
                    ),
                ],
                "auto",
                PlanActivation::Command,
            )
            .with_reasoning_levels(&CURSOR_EFFORT_LEVELS)
    }

    async fn resume_from_prompt(
        &self,
        _ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        // End-turn plan cards only — print mode has no live protocol to reply on.
        if prompt.kind != "plan" {
            return Ok(ResumeAction::Nothing);
        }
        if !answer.approve && answer.note.as_deref().is_none_or(|s| s.trim().is_empty()) {
            return Ok(ResumeAction::Nothing);
        }
        let note = answer.note.as_deref().filter(|s| !s.trim().is_empty());
        let (text, plan_mode) = if answer.approve {
            let mut text = "Implement the plan.".to_string();
            if let Some(note) = note {
                text.push_str(&format!("\n\nAdditional guidance: {note}"));
            }
            (text, false)
        } else {
            (super::synthesize_resume("plan", answer).0, true)
        };
        Ok(ResumeAction::SendMessage {
            text,
            mode: None,
            plan_mode: Some(plan_mode),
        })
    }

    fn config_home(&self) -> Option<PathBuf> {
        Some(native_store::cursor_home(NativeStore::Legacy))
    }

    fn skill_target(&self) -> Option<PathBuf> {
        Some(
            self.config_home()?
                .join("skills")
                .join("orx")
                .join("SKILL.md"),
        )
    }

    fn skill_shim(&self) -> Option<&'static str> {
        Some(super::CLAUDE_SKILL)
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(".cursor/skills")
    }
}

/// `cursor-agent` on PATH, else an `agent` binary that is actually Cursor, else
/// the installer drop under `~/.local/bin`. `agent` is a generic name, so a
/// hit is only accepted when the path (or the symlink it resolves to) names
/// Cursor.
pub(crate) fn find_cursor() -> Option<PathBuf> {
    find_on_path("cursor-agent")
        .or_else(|| find_on_path("agent").filter(|path| looks_like_cursor(path)))
        .or_else(|| {
            let home = dirs::home_dir()?;
            let local = home.join(".local").join("bin");
            find_in_dir(&local, "cursor-agent")
                .or_else(|| find_in_dir(&local, "agent").filter(|path| looks_like_cursor(path)))
        })
        .map(resolve_symlinks)
}

fn looks_like_cursor(path: &Path) -> bool {
    let mentions_cursor = |p: &Path| p.to_string_lossy().to_ascii_lowercase().contains("cursor");
    mentions_cursor(path)
        || crate::paths::canonicalize(path).is_ok_and(|real| mentions_cursor(&real))
}

fn apply_auth(info: &mut HarnessInfo, status: Option<&Value>, about: Option<&Value>) {
    let api = api_key("CURSOR_API_KEY");
    let logged_in = status
        .and_then(|value| {
            value
                .get("isAuthenticated")
                .and_then(Value::as_bool)
                .or_else(|| {
                    value
                        .get("status")
                        .and_then(Value::as_str)
                        .map(|status| status.eq_ignore_ascii_case("authenticated"))
                })
        })
        .unwrap_or(false);
    if api.is_some() {
        info.authenticated = true;
        info.auth_state = HarnessAuthState::Ready;
        info.auth_method = Some("apiKey");
    } else if logged_in {
        info.authenticated = true;
        info.auth_state = HarnessAuthState::Ready;
        info.auth_method = Some("oauth");
    } else if info.installed && !info.install_broken {
        info.auth_state = HarnessAuthState::NeedsLogin;
    }

    let cfg = read_json(native_store::cursor_home(NativeStore::Legacy).join("cli-config.json"));
    info.account = nonempty_str(about.unwrap_or(&Value::Null), "userEmail").or_else(|| {
        cfg.as_ref()
            .and_then(|cfg| cfg.get("authInfo"))
            .and_then(|auth| nonempty_str(auth, "email"))
    });
    info.plan = nonempty_str(about.unwrap_or(&Value::Null), "subscriptionTier");
}

async fn cursor_status_json(bin: &Path) -> Option<Value> {
    cursor_command_json(bin, &["status", "--format", "json"]).await
}

async fn cursor_command_json(bin: &Path, args: &[&str]) -> Option<Value> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    let out = tokio::time::timeout(AUTH_STATUS_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

async fn cursor_model_list(bin: &Path) -> Option<Vec<ModelInfo>> {
    let mut cmd = Command::new(bin);
    cmd.args(["models"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    let out = tokio::time::timeout(MODELS_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let parsed = parse_cursor_model_list(&text);
    (!parsed.is_empty()).then_some(parsed)
}

fn fallback_models() -> Vec<ModelInfo> {
    CURSOR_MODELS
        .iter()
        .map(|id| ModelInfo::new(*id).with_reasoning(&CURSOR_EFFORT_LEVELS))
        .collect()
}

/// Parse `agent models` / `--list-models` stdout. Accepts a JSON array/object
/// or a plain list (one id per line). Unknown shapes yield nothing so the
/// caller can fall back.
fn parse_cursor_model_list(text: &str) -> Vec<ModelInfo> {
    let trimmed = text.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
            return parse_cursor_model_json(&value);
        }
    }
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| !line.ends_with(':'))
        .filter(|line| {
            !line.eq_ignore_ascii_case("available models") && !line.eq_ignore_ascii_case("models")
        })
        .filter_map(|line| {
            let id = line
                .trim_start_matches(['-', '*', '•'])
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(['`', '"', '\'', ',', ';']);
            cursor_model_id_ok(id).then(|| ModelInfo::new(id).with_reasoning(&CURSOR_EFFORT_LEVELS))
        })
        .collect()
}

fn parse_cursor_model_json(value: &Value) -> Vec<ModelInfo> {
    let entries = value
        .as_array()
        .cloned()
        .or_else(|| value.get("models").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| {
            if let Some(id) = entry.as_str() {
                return cursor_model_id_ok(id)
                    .then(|| ModelInfo::new(id).with_reasoning(&CURSOR_EFFORT_LEVELS));
            }
            let id = ["id", "modelId", "value", "name"]
                .iter()
                .find_map(|key| nonempty_str(entry, key))?;
            if !cursor_model_id_ok(&id) {
                return None;
            }
            let display =
                nonempty_str(entry, "displayName").or_else(|| nonempty_str(entry, "display_name"));
            let description = nonempty_str(entry, "description");
            let efforts = cursor_model_efforts(entry);
            Some(
                ModelInfo::new(id)
                    .with_label(display.as_deref(), description.as_deref())
                    .with_reasoning(&efforts),
            )
        })
        .collect()
}

fn cursor_model_efforts(entry: &Value) -> Vec<&str> {
    let params = entry
        .get("parameters")
        .or_else(|| entry.get("modelParameters"))
        .and_then(Value::as_array);
    let Some(params) = params else {
        return CURSOR_EFFORT_LEVELS.to_vec();
    };
    let effort = params.iter().find(|param| {
        param
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| id == "effort")
    });
    let Some(effort) = effort else {
        return CURSOR_EFFORT_LEVELS.to_vec();
    };
    let values = effort
        .get("values")
        .or_else(|| effort.get("options"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| {
                    value
                        .as_str()
                        .or_else(|| value.get("id").and_then(Value::as_str))
                })
                .filter(|id| *id != REASONING_DEFAULT_ID)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if values.is_empty() {
        CURSOR_EFFORT_LEVELS.to_vec()
    } else {
        values
    }
}

fn cursor_model_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id != REASONING_DEFAULT_ID
        && id.len() < 80
        && !id.contains(' ')
        && id.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '[' | ']' | '=' | ',')
        })
}

fn cursor_effort(level: Option<&str>) -> Option<&str> {
    let level = level?;
    (level != REASONING_DEFAULT_ID && !level.is_empty()).then_some(level)
}

fn cursor_model_arg(model: Option<&str>, effort: Option<&str>) -> Option<String> {
    let model = model.filter(|model| !model.is_empty())?;
    match effort {
        Some(effort) if !model.contains('[') => Some(format!("{model}[effort={effort}]")),
        _ => Some(model.to_string()),
    }
}

fn cursor_cli_error(stderr: &str) -> Option<String> {
    let line = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let detail = line
        .strip_prefix("ActionRequiredError:")
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(line);
    Some(detail.to_string())
}

fn read_log_tail(path: &Path, max: usize) -> String {
    let Ok(data) = std::fs::read(path) else {
        return String::new();
    };
    let start = data.len().saturating_sub(max);
    String::from_utf8_lossy(&data[start..]).into_owned()
}

fn cursor_exit_detail(status: std::process::ExitStatus, log: &Path) -> String {
    let tail = read_log_tail(log, 8 * 1024);
    cursor_cli_error(&tail)
        .unwrap_or_else(|| format!("cursor exited with {status}; see {}", log.display()))
}

async fn cursor_one_shot(bin: &Path, request: OneShot<'_>) -> Option<String> {
    let message = format!("{}\n\n{}", request.system, request.prompt);
    let mut cmd = Command::new(bin);
    cmd.args([
        "--print",
        "--output-format",
        "text",
        "--mode",
        "ask",
        "--trust",
        "--approve-mcps",
    ]);
    if let Some(model) = request.model.filter(|model| !model.is_empty()) {
        cmd.args(["--model", model]);
    } else if matches!(request.quality, OneShotQuality::Cheap) {
        cmd.args(["--model", "composer-2.5"]);
    }
    cmd.arg(&message)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .current_dir(std::env::temp_dir());
    prepare_env(&mut cmd);
    cmd.env(
        "CURSOR_CONFIG_DIR",
        native_store::prepare_cursor(NativeStore::Isolated).ok()?,
    );
    cmd.env("NO_COLOR", "1");
    let out = tokio::time::timeout(request.timeout, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn first_turn_prompt(text: &str) -> String {
    format!(
        "Read and follow `{PLAYBOOK_REL}` before acting. It is the OpenResearch session playbook for this worktree.\n\n{text}"
    )
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    let bin = find_cursor().ok_or_else(|| {
        anyhow!("cursor-agent not found on PATH — install Cursor CLI and run `agent login` first")
    })?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let skills_dir = Cursor.session_skills_dir();
    let (repo, _playbook) =
        tokio::task::spawn_blocking(move || ensure_playbook(&project, &session_id, skills_dir))
            .await
            .map_err(|e| anyhow!("playbook task failed: {e}"))??;

    let native_session = match ctx.native_session_id.clone() {
        Some(id) => tokio::task::spawn_blocking(move || native_store::cursor_session(&id))
            .await
            .map_err(|error| anyhow!("Cursor session lookup failed: {error}"))??,
        None => None,
    };
    let native_store = native_session
        .as_ref()
        .map(|session| session.store)
        .unwrap_or(NativeStore::Isolated);
    let cursor_home =
        tokio::task::spawn_blocking(move || native_store::prepare_cursor(native_store))
            .await
            .map_err(|error| anyhow!("Cursor config preparation failed: {error}"))??;

    let resume = ctx
        .native_session_id
        .clone()
        .filter(|_| native_session.is_some());
    let mut prompt = ctx.text.clone();
    if ctx.native_session_id.is_some() && resume.is_none() {
        if let Some(recovery) = super::native_recovery_context(ctx, "Cursor") {
            prompt = format!("{recovery}\n\n{prompt}");
        }
    }
    if resume.is_none() {
        prompt = first_turn_prompt(&prompt);
    }

    let mut cmd = Command::new(&bin);
    cmd.args([
        "--print",
        "--output-format",
        "stream-json",
        "--stream-partial-output",
        "--trust",
        "--approve-mcps",
        "--workspace",
    ])
    .arg(&repo);
    if let Some(model) = cursor_model_arg(
        ctx.model.as_deref(),
        cursor_effort(ctx.reasoning_level.as_deref()),
    ) {
        cmd.args(["--model", &model]);
    }
    if ctx.plan_mode || ctx.permission_mode == Some(PermissionMode::Plan) {
        cmd.args(["--mode", "plan"]);
    } else {
        match ctx.permission_mode.unwrap_or(PermissionMode::Auto) {
            PermissionMode::Ask => {
                cmd.args(["--mode", "ask"]);
            }
            PermissionMode::Bypass => {
                cmd.args(["--force", "--sandbox", "disabled"]);
            }
            PermissionMode::Auto | PermissionMode::AcceptEdits | PermissionMode::Plan => {
                cmd.arg("--force");
            }
        }
    }
    if let Some(native_id) = &resume {
        cmd.args(["--resume", native_id]);
    }
    cmd.arg(&prompt)
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log("cursor")?))
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    cmd.env("CURSOR_CONFIG_DIR", &cursor_home);
    cmd.env("NO_COLOR", "1");
    set_chat_session_env(&mut cmd, &ctx.session_id, "cursor", ctx.host.up_port());

    ctx.persist_delivery(DeliveryState::Unknown)?;
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            ctx.mark_delivery(DeliveryState::NotSent);
            return Err(anyhow!("Could not spawn {}: {}", bin.display(), error));
        }
    };
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut state = TurnState::default();

    loop {
        match tokio::time::timeout(TURN_WATCHDOG, lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                ctx.mark_delivery(DeliveryState::Accepted);
                let terminal = apply_event(ctx, &mut state, &event);
                ctx.maybe_flush();
                if terminal {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                return Err(anyhow!("cursor stdout: {error}"));
            }
            Err(_) => {
                let _ = child.start_kill();
                ctx.push_error(
                    "Cursor Agent went silent for 30 minutes and was interrupted.".to_string(),
                );
                break;
            }
        }
    }

    let status = child.wait().await?;
    if let Some(sid) = state.native_session_id.as_deref() {
        ctx.set_native_session_id(sid);
    }
    if ctx.plan_mode {
        if let Some(card) = plan_card(&ctx.assistant.parts, &ctx.assistant.id, state.turn_errored) {
            ctx.upsert_part(card);
        }
    }
    if state.turn_errored {
        let message = ctx
            .assistant
            .parts
            .iter()
            .rev()
            .find_map(|part| part.state.as_ref()?.error.clone())
            .unwrap_or_else(|| "Cursor reported a terminal turn error".into());
        ctx.mark_terminal_failure("cursor_terminal", message);
    } else if !status.success() && !state.saw_result {
        let log_path = crate::store::data_dir().join("agent-cursor.log");
        return Err(anyhow!("{}", cursor_exit_detail(status, &log_path)));
    }
    let _ = ctx.flush();
    Ok(())
}

#[derive(Default)]
struct TurnState {
    native_session_id: Option<String>,
    text_part_id: Option<String>,
    streamed_current: bool,
    text_seq: usize,
    last_text: String,
    turn_errored: bool,
    saw_result: bool,
}

fn apply_event(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) -> bool {
    if let Some(sid) = event.get("session_id").and_then(Value::as_str) {
        state.native_session_id = Some(sid.to_string());
    }
    match event.get("type").and_then(Value::as_str) {
        Some("system") => {
            if event.get("subtype").and_then(Value::as_str) == Some("init") {
                if let Some(sid) = event.get("session_id").and_then(Value::as_str) {
                    state.native_session_id = Some(sid.to_string());
                }
            }
            false
        }
        Some("assistant") => {
            apply_assistant(ctx, state, event);
            false
        }
        Some("tool_call") => {
            apply_tool_call(ctx, state, event);
            false
        }
        Some("result") => {
            state.saw_result = true;
            let is_error = event
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || event.get("subtype").and_then(Value::as_str) == Some("error");
            if is_error {
                state.turn_errored = true;
                let detail = event
                    .get("result")
                    .and_then(Value::as_str)
                    .or_else(|| event.get("error").and_then(Value::as_str))
                    .unwrap_or("Cursor reported an error")
                    .to_string();
                ctx.push_error(detail);
            }
            true
        }
        _ => false,
    }
}

fn apply_assistant(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) {
    let text = assistant_text(event);
    if text.is_empty() {
        return;
    }
    // `--stream-partial-output` emits three assistant shapes. Only the delta
    // (timestamp, no model_call_id) is new text; the others duplicate it.
    let has_ts = event.get("timestamp_ms").is_some();
    let has_mc = event.get("model_call_id").is_some();
    if has_mc || (!has_ts && state.streamed_current) {
        if !has_ts {
            close_text_segment(state);
        }
        return;
    }
    if !text.trim().is_empty() {
        state.last_text = text.clone();
    }
    if has_ts {
        state.streamed_current = true;
        let id = match state.text_part_id.as_ref() {
            Some(id) => id.clone(),
            None => {
                let id = next_text_id(state);
                state.text_part_id = Some(id.clone());
                id
            }
        };
        if ctx.assistant.parts.iter().all(|part| part.id != id) {
            ctx.upsert_part(WirePart::text(id.clone(), ""));
        }
        ctx.append_part_text(&id, &text);
    } else {
        let id = state
            .text_part_id
            .take()
            .unwrap_or_else(|| next_text_id(state));
        ctx.upsert_part(WirePart::text(id, &text));
        close_text_segment(state);
    }
}

fn apply_tool_call(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) {
    close_text_segment(state);
    let subtype = event.get("subtype").and_then(Value::as_str).unwrap_or("");
    let call_id = event
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("tool-{}", state.text_seq));
    let (name, args, result) = tool_call_parts(event.get("tool_call").unwrap_or(&Value::Null));
    match subtype {
        "started" => {
            let title = tool_title(args.as_ref());
            ctx.upsert_part(WirePart {
                id: call_id,
                kind: "tool".into(),
                text: None,
                tool: Some(name),
                state: Some(WireToolState {
                    status: "running".into(),
                    input: args,
                    output: None,
                    error: None,
                    title,
                }),
                prompt: None,
                children: Vec::new(),
            });
        }
        "completed" => {
            let (ok, output) = result
                .as_ref()
                .map(tool_result_text)
                .unwrap_or((true, String::new()));
            if let Some(part) = find_part_mut(&mut ctx.assistant.parts, &call_id) {
                if let Some(part_state) = part.state.as_mut() {
                    part_state.status = if ok { "completed" } else { "error" }.into();
                    if ok {
                        part_state.output = Some(output);
                    } else {
                        part_state.error = Some(output);
                    }
                }
            } else {
                let title = tool_title(args.as_ref());
                ctx.upsert_part(WirePart {
                    id: call_id,
                    kind: "tool".into(),
                    text: None,
                    tool: Some(name),
                    state: Some(WireToolState {
                        status: if ok { "completed" } else { "error" }.into(),
                        input: args,
                        output: ok.then_some(output.clone()),
                        error: (!ok).then_some(output),
                        title,
                    }),
                    prompt: None,
                    children: Vec::new(),
                });
            }
        }
        _ => {}
    }
}

fn assistant_text(event: &Value) -> String {
    event
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn tool_call_parts(tool_call: &Value) -> (String, Option<Value>, Option<Value>) {
    if let Some(func) = tool_call.get("function") {
        let name = func
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool")
            .to_string();
        let args = func.get("arguments").cloned().and_then(|value| {
            if let Some(raw) = value.as_str() {
                serde_json::from_str(raw).ok()
            } else {
                Some(value)
            }
        });
        return (name, args, func.get("result").cloned());
    }
    if let Some(obj) = tool_call.as_object() {
        for (key, value) in obj {
            if let Some(stem) = key.strip_suffix("ToolCall") {
                return (
                    title_case(stem),
                    value.get("args").cloned(),
                    value.get("result").cloned(),
                );
            }
        }
    }
    ("tool".into(), None, None)
}

fn tool_title(args: Option<&Value>) -> Option<String> {
    let args = args?;
    args.get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("filePath"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            args.get("command")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn tool_result_text(result: &Value) -> (bool, String) {
    if let Some(success) = result.get("success") {
        if let Some(content) = success.get("content").and_then(Value::as_str) {
            return (true, content.to_string());
        }
        return (true, compact_json(success));
    }
    if let Some(error) = result.get("error") {
        let text = error
            .as_str()
            .map(str::to_string)
            .or_else(|| {
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| compact_json(error));
        return (false, text);
    }
    (true, compact_json(result))
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn next_text_id(state: &mut TurnState) -> String {
    state.text_seq += 1;
    format!("text-{}", state.text_seq)
}

fn close_text_segment(state: &mut TurnState) {
    state.text_part_id = None;
    state.streamed_current = false;
}

fn plan_card(parts: &[WirePart], assistant_id: &str, errored: bool) -> Option<WirePart> {
    let last_text = parts
        .iter()
        .rev()
        .find(|part| {
            part.kind == "text"
                && part
                    .text
                    .as_deref()
                    .is_some_and(|text| !text.trim().is_empty())
        })
        .and_then(|part| part.text.as_deref())?;
    if !super::should_synthesize_plan(true, false, errored, last_text) {
        return None;
    }
    Some(WirePart::prompt(
        format!("plan-synth-{assistant_id}"),
        WirePrompt {
            kind: "plan".into(),
            plan: Some(last_text.to_string()),
            synthesized: true,
            ..Default::default()
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::TurnCtx;

    fn fold(events: &[Value]) -> (TurnCtx, TurnState) {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for event in events {
            apply_event(&mut ctx, &mut state, event);
        }
        (ctx, state)
    }

    #[test]
    fn documented_stream_records_session_tools_and_text() {
        let events = vec![
            serde_json::json!({
                "type": "system",
                "subtype": "init",
                "session_id": "c6b62c6f-7ead-4fd6-9922-e952131177ff",
                "model": "Claude 4 Sonnet",
            }),
            serde_json::json!({
                "type": "assistant",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "I'll read README.md"}]},
                "session_id": "c6b62c6f-7ead-4fd6-9922-e952131177ff",
            }),
            serde_json::json!({
                "type": "tool_call",
                "subtype": "started",
                "call_id": "toolu_read",
                "tool_call": {"readToolCall": {"args": {"path": "README.md"}}},
            }),
            serde_json::json!({
                "type": "tool_call",
                "subtype": "completed",
                "call_id": "toolu_read",
                "tool_call": {"readToolCall": {
                    "args": {"path": "README.md"},
                    "result": {"success": {"content": "# Project", "totalLines": 1}}
                }},
            }),
            serde_json::json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "result": "I'll read README.md",
                "session_id": "c6b62c6f-7ead-4fd6-9922-e952131177ff",
            }),
        ];
        let (ctx, state) = fold(&events);
        assert_eq!(
            state.native_session_id.as_deref(),
            Some("c6b62c6f-7ead-4fd6-9922-e952131177ff")
        );
        assert!(state.saw_result);
        assert_eq!(ctx.assistant.parts[0].kind, "text");
        assert_eq!(
            ctx.assistant.parts[0].text.as_deref(),
            Some("I'll read README.md")
        );
        assert_eq!(ctx.assistant.parts[1].tool.as_deref(), Some("Read"));
        assert_eq!(
            ctx.assistant.parts[1].state.as_ref().unwrap().status,
            "completed"
        );
        assert_eq!(
            ctx.assistant.parts[1]
                .state
                .as_ref()
                .unwrap()
                .output
                .as_deref(),
            Some("# Project")
        );
    }

    #[test]
    fn streaming_deltas_append_and_duplicate_flushes_are_skipped() {
        let events = vec![
            serde_json::json!({
                "type": "assistant",
                "timestamp_ms": 1,
                "message": {"content": [{"type": "text", "text": "Hel"}]}
            }),
            serde_json::json!({
                "type": "assistant",
                "timestamp_ms": 2,
                "message": {"content": [{"type": "text", "text": "lo"}]}
            }),
            serde_json::json!({
                "type": "assistant",
                "timestamp_ms": 3,
                "model_call_id": "mc-1",
                "message": {"content": [{"type": "text", "text": "Hello"}]}
            }),
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "text", "text": "Hello"}]}
            }),
        ];
        let (ctx, _) = fold(&events);
        let texts: Vec<_> = ctx
            .assistant
            .parts
            .iter()
            .filter(|part| part.kind == "text")
            .map(|part| part.text.clone().unwrap_or_default())
            .collect();
        assert_eq!(texts, vec!["Hello".to_string()]);
    }

    #[test]
    fn model_list_parses_json_and_plain_lines() {
        let json = serde_json::json!({
            "models": [
                {"id": "auto", "displayName": "Auto"},
                {"id": "grok-4.6", "displayName": "Grok 4.6", "parameters": [
                    {"id": "effort", "values": ["low", "high"]}
                ]},
                {"id": "default"},
            ]
        });
        let parsed = parse_cursor_model_json(&json);
        assert_eq!(
            parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["auto", "grok-4.6"]
        );
        assert_eq!(parsed[0].display_name.as_deref(), Some("Auto"));
        let efforts: Vec<_> = parsed[1]
            .reasoning_levels
            .as_ref()
            .unwrap()
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(efforts, ["default", "low", "high"]);

        let lines = parse_cursor_model_list("Available models:\n- grok-4.6\n- composer-2.5\n");
        assert_eq!(
            lines.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["grok-4.6", "composer-2.5"]
        );
    }

    #[test]
    fn effort_is_appended_unless_model_is_already_parameterized() {
        assert_eq!(
            cursor_model_arg(Some("grok-4.6"), Some("high")).as_deref(),
            Some("grok-4.6[effort=high]")
        );
        assert_eq!(
            cursor_model_arg(Some("grok-4.6[fast=true]"), Some("high")).as_deref(),
            Some("grok-4.6[fast=true]")
        );
        assert_eq!(cursor_effort(Some(REASONING_DEFAULT_ID)), None);
        assert_eq!(cursor_effort(Some("high")), Some("high"));
        assert_eq!(cursor_model_arg(None, Some("high")), None);
    }

    #[test]
    fn cursor_cli_error_strips_action_required_prefix() {
        let log = "\
ActionRequiredError: Named models unavailable Free plans can only use Auto. Switch to Auto or upgrade plans to continue.\n";
        assert_eq!(
            cursor_cli_error(log).as_deref(),
            Some(
                "Named models unavailable Free plans can only use Auto. Switch to Auto or upgrade plans to continue."
            )
        );
    }

    #[test]
    fn looks_like_cursor_requires_cursor_in_the_path() {
        assert!(looks_like_cursor(Path::new("/usr/bin/cursor-agent")));
        assert!(!looks_like_cursor(Path::new("/usr/bin/agent")));
    }

    #[test]
    fn plan_card_synthesizes_from_last_text() {
        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text("t1", "Do the following: step one."));
        let card = plan_card(&ctx.assistant.parts, "msg", false).unwrap();
        assert!(card.prompt.as_ref().unwrap().synthesized);
        assert_eq!(
            card.prompt.as_ref().unwrap().plan.as_deref(),
            Some("Do the following: step one.")
        );
        assert!(plan_card(&ctx.assistant.parts, "msg", true).is_none());
    }

    #[tokio::test]
    async fn plan_resume_leaves_plan_on_approve() {
        let harness = Cursor;
        let prompt = WirePrompt {
            kind: "plan".into(),
            plan: Some("do it".into()),
            synthesized: true,
            ..Default::default()
        };
        let ctx = ResumeCtx {
            host: TurnCtx::test_stub().host.clone(),
            session_id: "s".into(),
            native_session_id: None,
        };
        let approve = harness
            .resume_from_prompt(
                &ctx,
                &prompt,
                &PromptAnswer {
                    session_id: "s".into(),
                    prompt_id: "p".into(),
                    approve: true,
                    answers: Vec::new(),
                    note: None,
                    resume_mode: None,
                    annotations: Vec::new(),
                },
            )
            .await
            .unwrap();
        match approve {
            ResumeAction::SendMessage {
                text,
                plan_mode,
                mode,
            } => {
                assert!(text.contains("Implement the plan"));
                assert_eq!(plan_mode, Some(false));
                assert!(mode.is_none());
            }
            _ => panic!("expected SendMessage"),
        }
        let reject = harness
            .resume_from_prompt(
                &ctx,
                &prompt,
                &PromptAnswer {
                    session_id: "s".into(),
                    prompt_id: "p".into(),
                    approve: false,
                    answers: Vec::new(),
                    note: None,
                    resume_mode: None,
                    annotations: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert!(matches!(reject, ResumeAction::Nothing));
    }
}
