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
                        cursor_command_json(bin, &["status", "--format", "json"]),
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
            info.agent_note =
                Some("Sign in with `agent login`, then re-check this harness.".to_string());
        } else {
            info.agent_note = Some(
                "Install Cursor CLI with `curl https://cursor.com/install -fsS | bash`, then sign in with `agent login`."
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
        HarnessOptions::none().with_permission_choices(
            vec![
                OptionChoice::described("ask", "Ask", "Answer questions without changing files"),
                OptionChoice::described("auto", "Auto", "Allow commands unless explicitly denied"),
                OptionChoice::described(
                    "full-access",
                    "Full access",
                    "Allow commands and disable the sandbox",
                ),
            ],
            "auto",
            PlanActivation::Command,
        )
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

async fn cursor_command_json(bin: &Path, args: &[&str]) -> Option<Value> {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
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
        .stderr(Stdio::null())
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    let out = tokio::time::timeout(MODELS_TIMEOUT, cmd.output())
        .await
        .ok()?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let parsed = parse_cursor_model_list(&text);
    (!parsed.is_empty()).then_some(parsed)
}

fn fallback_models() -> Vec<ModelInfo> {
    vec![ModelInfo::new("auto").with_label(Some("Auto"), None)]
}

// `agent models` lists `id - display name`, followed by a usage tip.
// It does not report effort capabilities; leave those to Cursor's model variants.
fn parse_cursor_model_list(text: &str) -> Vec<ModelInfo> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (id, label) = line.split_once(" - ").unwrap_or((line, ""));
            if id.is_empty()
                || id == REASONING_DEFAULT_ID
                || !id.chars().all(|c| {
                    c.is_ascii_alphanumeric()
                        || matches!(c, '-' | '_' | '.' | '[' | ']' | '=' | ',')
                })
            {
                return None;
            }
            let label = [" (current, default)", " (current)", " (default)"]
                .iter()
                .find_map(|suffix| label.strip_suffix(suffix))
                .unwrap_or(label);
            Some(ModelInfo::new(id).with_label((!label.is_empty()).then_some(label), None))
        })
        .collect()
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
    use std::io::{Read, Seek, SeekFrom};

    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(max as u64)));
    let mut data = Vec::new();
    let _ = file.take(max as u64).read_to_end(&mut data);
    String::from_utf8_lossy(&data).into_owned()
}

fn cursor_exit_detail(status: std::process::ExitStatus, log: &Path) -> String {
    let tail = read_log_tail(log, 8 * 1024);
    cursor_cli_error(&tail).unwrap_or_else(|| {
        format!(
            "Cursor ended without a result ({status}); see {}",
            log.display()
        )
    })
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
    ]);
    if let Some(model) = request.model.filter(|model| !model.is_empty()) {
        cmd.args(["--model", model]);
    } else if matches!(request.quality, OneShotQuality::Cheap) {
        cmd.args(["--model", "auto"]);
    }
    cmd.arg(&message)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .current_dir(std::env::temp_dir());
    prepare_env(&mut cmd);
    let cursor_home =
        tokio::task::spawn_blocking(|| native_store::prepare_cursor(NativeStore::Isolated))
            .await
            .ok()?
            .ok()?;
    cmd.env("CURSOR_CONFIG_DIR", &cursor_home);
    cmd.env("CURSOR_DATA_DIR", &cursor_home);
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
        "--workspace",
    ])
    .arg(&repo);
    if let Some(model) = ctx.model.as_deref().filter(|model| !model.is_empty()) {
        cmd.args(["--model", model]);
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
    let log_name = format!("cursor-{}", uuid::Uuid::new_v4());
    cmd.arg(&prompt)
        .current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log(&log_name)?))
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    cmd.env("CURSOR_CONFIG_DIR", &cursor_home);
    if native_store == NativeStore::Isolated {
        cmd.env("CURSOR_DATA_DIR", &cursor_home);
    }
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
                if let Some(sid) = state.native_session_id.as_deref() {
                    ctx.set_native_session_id(sid);
                }
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
                return Err(anyhow!(
                    "Cursor Agent went silent for {} minutes and was interrupted.",
                    TURN_WATCHDOG.as_secs() / 60
                ));
            }
        }
    }

    let status = child.wait().await?;
    let log_path = crate::store::data_dir().join(format!("agent-{log_name}.log"));
    if !state.saw_result {
        return Err(anyhow!("{}", cursor_exit_detail(status, &log_path)));
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
    } else if status.success() {
        let _ = std::fs::remove_file(log_path);
    }
    let _ = ctx.flush();
    Ok(())
}

#[derive(Default)]
struct TurnState {
    native_session_id: Option<String>,
    text_part_id: Option<String>,
    reasoning_part_id: Option<String>,
    streamed_current: bool,
    text_seq: usize,
    text_len_before_delta: Option<usize>,
    turn_errored: bool,
    saw_result: bool,
}

fn apply_event(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) -> bool {
    if let Some(sid) = event.get("session_id").and_then(Value::as_str) {
        state.native_session_id = Some(sid.to_string());
    }
    match event.get("type").and_then(Value::as_str) {
        Some("thinking") => {
            if event.get("subtype").and_then(Value::as_str) == Some("delta") {
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    let id = match &state.reasoning_part_id {
                        Some(id) => id.clone(),
                        None => {
                            close_text_segment(state);
                            let id = next_text_id(state);
                            ctx.upsert_part(WirePart::reasoning(id.clone(), ""));
                            state.reasoning_part_id = Some(id.clone());
                            id
                        }
                    };
                    ctx.append_part_text(&id, text);
                }
            } else {
                state.reasoning_part_id = None;
            }
            false
        }
        Some("assistant") => {
            state.reasoning_part_id = None;
            apply_assistant(ctx, state, event);
            false
        }
        Some("tool_call") => {
            apply_tool_call(ctx, state, event);
            false
        }
        Some("retry") => {
            // Cursor flushes its buffered text as a delta immediately before retry.
            if let (Some(id), Some(len)) = (&state.text_part_id, state.text_len_before_delta) {
                if let Some(text) = ctx
                    .assistant
                    .parts
                    .iter()
                    .find(|part| &part.id == id)
                    .and_then(|part| part.text.as_deref())
                {
                    ctx.upsert_part(WirePart::text(id.clone(), &text[..len]));
                }
            }
            close_text_segment(state);
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
            } else {
                // Cursor's result concatenates progress and answers across model calls.
                ctx.mark_final_text_tail();
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
        state.text_len_before_delta = ctx
            .assistant
            .parts
            .iter()
            .find(|part| part.id == id)
            .and_then(|part| part.text.as_ref())
            .map(String::len);
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
    state.reasoning_part_id = None;
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
                    title: None,
                }),
                prompt: None,
                phase: None,
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
                        title: None,
                    }),
                    prompt: None,
                    phase: None,
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
                    if stem == "shell" {
                        "Bash".into()
                    } else {
                        title_case(stem)
                    },
                    value.get("args").cloned(),
                    value.get("result").cloned(),
                );
            }
        }
    }
    ("tool".into(), None, None)
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
    state.text_len_before_delta = None;
}

fn plan_card(parts: &[WirePart], assistant_id: &str, errored: bool) -> Option<WirePart> {
    let last_text = parts
        .iter()
        .rev()
        .find_map(|part| {
            if part.tool.as_deref() != Some("CreatePlan") {
                return None;
            }
            let state = part.state.as_ref()?;
            (state.status == "completed")
                .then(|| state.input.as_ref()?.get("plan")?.as_str())
                .flatten()
                .filter(|plan| !plan.trim().is_empty())
        })
        .or_else(|| {
            parts.iter().rev().find_map(|part| {
                (part.kind == "text")
                    .then_some(part.text.as_deref())
                    .flatten()
                    .filter(|text| !text.trim().is_empty())
            })
        })?;
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
    fn streamed_work_collapses_only_after_a_successful_result() {
        use crate::local::chat::MessagePhase;
        use serde_json::json;

        let delta = |text: &str| {
            json!({
                "type": "assistant", "timestamp_ms": 1,
                "message": {"content": [{"type": "text", "text": text}]}
            })
        };
        let (mut ctx, mut state) = fold(&[
            json!({"type": "thinking", "subtype": "delta", "text": "Read the file."}),
            json!({"type": "thinking", "subtype": "completed"}),
            delta("I'll read README.md."),
            json!({"type": "assistant", "timestamp_ms": 1, "model_call_id": "call-1",
                "message": {"content": [{"type": "text", "text": "I'll read README.md."}]}}),
            json!({"type": "tool_call", "subtype": "started", "call_id": "read-1",
                "tool_call": {"readToolCall": {"args": {"path": "README.md"}}}}),
            json!({"type": "tool_call", "subtype": "completed", "call_id": "read-1",
                "tool_call": {"readToolCall": {"result": {"success": {"content": "An otter."}}}}}),
            json!({"type": "thinking", "subtype": "delta", "text": "The mascot is an otter."}),
            json!({"type": "thinking", "subtype": "completed"}),
            delta("The mascot is "),
            delta("an otter."),
            json!({"type": "assistant",
                "message": {"content": [{"type": "text", "text": "The mascot is an otter."}]}}),
        ]);
        assert_eq!(ctx.assistant.parts.len(), 5);
        assert_eq!(ctx.assistant.parts[0].kind, "reasoning");
        assert_eq!(ctx.assistant.parts[3].kind, "reasoning");
        assert!(ctx.assistant.parts.iter().all(|part| part.phase.is_none()));

        apply_event(
            &mut ctx,
            &mut state,
            &json!({"type": "result", "subtype": "success",
            "result": "I'll read README.md.The mascot is an otter."}),
        );
        let saved = serde_json::to_string(&ctx.assistant.parts).unwrap();
        let restored: Vec<WirePart> = serde_json::from_str(&saved).unwrap();
        assert_eq!(restored[1].phase, Some(MessagePhase::Commentary));
        assert_eq!(restored[4].phase, Some(MessagePhase::FinalAnswer));
        assert_eq!(restored[4].text.as_deref(), Some("The mascot is an otter."));
    }

    #[test]
    fn failed_result_does_not_promote_partial_text_to_an_answer() {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        ctx.upsert_part(WirePart::tool("read", "Read", "completed", None));
        ctx.upsert_part(WirePart::text("partial", "Still checking"));
        apply_event(
            &mut ctx,
            &mut state,
            &serde_json::json!({
                "type": "result", "is_error": true, "result": "Rate limit exceeded"
            }),
        );
        assert!(state.turn_errored);
        assert!(ctx.assistant.parts.iter().all(|part| part.phase.is_none()));
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
    fn model_list_preserves_cli_labels_without_inventing_effort_controls() {
        let models = parse_cursor_model_list(
            "Available models\n\nauto - Auto (current, default)\ngrok-4.6 - Grok 4.6\nclaude-opus-4-8[effort=high] - Opus 4.8 High\n\nTip: use --model <id> to switch.\n",
        );
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "auto");
        assert_eq!(models[0].display_name.as_deref(), Some("Auto"));
        assert_eq!(models[1].display_name.as_deref(), Some("Grok 4.6"));
        assert_eq!(models[2].id, "claude-opus-4-8[effort=high]");
        assert!(models.iter().all(|model| model.reasoning_levels.is_none()));
        assert!(parse_cursor_model_list("No models available for this account.").is_empty());
        assert_eq!(fallback_models()[0].id, "auto");
    }

    #[test]
    fn retry_flush_does_not_duplicate_streamed_text() {
        let text = |value: &str| {
            serde_json::json!({
                "type": "assistant", "timestamp_ms": 1,
                "message": {"content": [{"type": "text", "text": value}]}
            })
        };
        let (ctx, _) = fold(&[
            text("Hello"),
            text("Hello"),
            serde_json::json!({"type": "retry", "subtype": "started"}),
            text(" again"),
            serde_json::json!({"type": "assistant", "message": {
                "content": [{"type": "text", "text": " again"}]
            }}),
        ]);
        let rendered: String = ctx
            .assistant
            .parts
            .iter()
            .filter_map(|part| part.text.as_deref())
            .collect();
        assert_eq!(rendered, "Hello again");
    }

    #[test]
    fn plan_card_uses_the_native_plan_instead_of_the_closing_message() {
        let (mut ctx, _) = fold(&[serde_json::json!({
            "type": "tool_call", "subtype": "completed", "call_id": "plan-1",
            "tool_call": {"createPlanToolCall": {
                "args": {"plan": "# Plan\n1. Run the baseline", "name": "Baseline"},
                "result": {"success": {}}
            }}
        })]);
        ctx.upsert_part(WirePart::text("closing", "The plan is ready."));
        let card = plan_card(&ctx.assistant.parts, "msg", false).unwrap();
        assert_eq!(
            card.prompt.unwrap().plan.as_deref(),
            Some("# Plan\n1. Run the baseline")
        );
    }

    #[test]
    fn shell_tool_uses_dashboard_command_activity() {
        let event = serde_json::json!({
            "type": "tool_call", "subtype": "completed", "call_id": "shell-1",
            "tool_call": {"shellToolCall": {
                "args": {"command": "orx exp status"},
                "result": {"success": {"content": "No runs"}}
            }}
        });
        let (ctx, _) = fold(&[event]);
        assert_eq!(ctx.assistant.parts[0].tool.as_deref(), Some("Bash"));
        assert_eq!(
            ctx.assistant.parts[0]
                .state
                .as_ref()
                .unwrap()
                .input
                .as_ref()
                .unwrap()["command"],
            "orx exp status"
        );
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
    fn error_log_reads_only_the_requested_tail() {
        let path = std::env::temp_dir().join(format!("cursor-log-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, "earlier diagnostics\ncurrent error").unwrap();
        assert_eq!(read_log_tail(&path, 13), "current error");
        std::fs::remove_file(path).unwrap();
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
