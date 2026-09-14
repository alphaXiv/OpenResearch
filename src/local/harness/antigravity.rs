//! Google Antigravity harness.
//!
//! Chat: one `agy --output-format stream-json` child per turn. Multi-turn continues
//! via `--conversation <conversation_id>` from the init/result `conversation_id`. Isolated
//! ORX worktrees are the child's current working directory and added workspace (`--add-dir`).
//!
//! The playbook is pointed at on the first turn (the file is already in the
//! worktree via [`ensure_playbook`]); session skills land in `.agents/skills`.
//!
//! Options: Ask prompts before changes, Auto allows changes unless denied, and
//! Bypass passes `--dangerously-skip-permissions`.
//!
//! Detection: `agy` on PATH or in `~/.local/bin` / `~/.gemini/antigravity-cli/bin`;
//! `agy models` for catalog and authentication verification.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::detect::{probe_bin, resolve_symlinks, HarnessAuthState, HarnessInfo, ModelInfo};
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

const AGY_REINSTALL: &str =
    "Reinstall Antigravity CLI via curl -sSf https://antigravity.google/install | sh";
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Antigravity;

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

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        if let Some(bin) = find_agy() {
            info.record_bin(&bin, probe_bin(&bin).await);
        }
        if info.installed && !info.install_broken {
            let authed = match info.bin_path.as_deref().map(Path::new) {
                Some(bin) => check_auth_ready(bin).await,
                None => false,
            };
            if authed {
                info.authenticated = true;
                info.auth_state = HarnessAuthState::Ready;
                info.auth_method = Some("oauth");
            } else {
                info.auth_state = HarnessAuthState::NeedsLogin;
            }
        }

        info.agent_ready = info.ready();
        if info.agent_ready {
            let models = match info.bin_path.as_deref().map(Path::new) {
                Some(bin) => agy_model_list(bin).await,
                None => None,
            };
            info = info.with_models(models.unwrap_or_else(fallback_models));
        } else if info.install_broken {
            info.agent_note = Some(info.broken_note(AGY_REINSTALL));
        } else if info.installed {
            info.agent_note = Some(
                "Sign in by running `agy` in your terminal, then re-check this harness."
                    .to_string(),
            );
        } else {
            info.agent_note = Some(
                "Install Antigravity CLI with `curl -sSf https://antigravity.google/install | sh`, then sign in with `agy`."
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

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none()
            .with_permission_choices(
                vec![
                    OptionChoice::described(
                        "ask",
                        "Ask",
                        "Prompt before running commands or modifying files",
                    ),
                    OptionChoice::described(
                        "auto",
                        "Auto",
                        "Allow actions unless explicitly denied",
                    ),
                    OptionChoice::described(
                        "bypass",
                        "Bypass",
                        "Allow commands and skip tool confirmation prompts",
                    ),
                ],
                "auto",
                PlanActivation::Command,
            )
            .with_reasoning_levels(&["low", "medium", "high"])
    }

    async fn resume_from_prompt(
        &self,
        _ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
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

    async fn one_shot(&self, request: OneShot<'_>) -> Option<String> {
        agy_one_shot(&find_agy()?, request).await
    }

    fn config_home(&self) -> Option<PathBuf> {
        Some(native_store::antigravity_home(NativeStore::Legacy))
    }

    fn skill_target(&self) -> Option<PathBuf> {
        Some(
            self.config_home()?
                .join("skills")
                .join("orx")
                .join("SKILL.md"),
        )
    }

    fn extra_skill_targets(&self) -> Vec<(PathBuf, &'static str)> {
        dirs::home_dir()
            .map(|h| {
                vec![(
                    h.join(".agents")
                        .join("skills")
                        .join("orx")
                        .join("SKILL.md"),
                    super::CLAUDE_SKILL,
                )]
            })
            .unwrap_or_default()
    }

    fn skill_shim(&self) -> Option<&'static str> {
        Some(super::CLAUDE_SKILL)
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(".agents/skills")
    }
}

/// `agy` on PATH, else search common install locations under `~/.local/bin`
/// or `~/.gemini/antigravity-cli/bin`.
pub(crate) fn find_agy() -> Option<PathBuf> {
    find_on_path("agy")
        .or_else(|| {
            let home = dirs::home_dir()?;
            let local = home.join(".local").join("bin");
            find_in_dir(&local, "agy").or_else(|| {
                let agy_bin = home.join(".gemini").join("antigravity-cli").join("bin");
                find_in_dir(&agy_bin, "agy")
            })
        })
        .map(resolve_symlinks)
}

async fn check_auth_ready(bin: &Path) -> bool {
    let mut cmd = Command::new(bin);
    cmd.args(["models"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");

    if let Ok(Ok(out)) = tokio::time::timeout(MODELS_TIMEOUT, cmd.output()).await {
        return out.status.success();
    }
    false
}

async fn agy_model_list(bin: &Path) -> Option<Vec<ModelInfo>> {
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
    let parsed = parse_agy_model_list(&text);
    (!parsed.is_empty()).then_some(parsed)
}

fn fallback_models() -> Vec<ModelInfo> {
    vec![
        ModelInfo::new("gemini-3.8-flash-high").with_label(Some("Gemini 3.8 Flash (High)"), None),
        ModelInfo::new("gemini-3.1-pro-high").with_label(Some("Gemini 3.1 Pro (High)"), None),
        ModelInfo::new("claude-sonnet-4-6").with_label(Some("Claude Sonnet 4.6 (Thinking)"), None),
    ]
}

/// Parse `agy models` output. Lines are tab-separated `<id>\t<label>`, ignoring
/// informational banner lines such as `Fetching available models...`.
fn parse_agy_model_list(text: &str) -> Vec<ModelInfo> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty()
                || line.starts_with("Fetching")
                || line.starts_with("Available")
                || line.starts_with("Listing")
            {
                return None;
            }
            let (id, label) = line.split_once('\t').unwrap_or((line, ""));
            let id = id.trim();
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

async fn agy_one_shot(bin: &Path, request: OneShot<'_>) -> Option<String> {
    let message = if request.system.is_empty() {
        request.prompt.to_string()
    } else {
        format!("{}\n\n{}", request.system, request.prompt)
    };
    let mut cmd = Command::new(bin);
    cmd.args([
        "--output-format",
        "text",
        "--effort",
        if matches!(request.quality, OneShotQuality::Cheap) {
            "low"
        } else {
            "medium"
        },
    ]);
    if let Some(model) = request.model.filter(|model| !model.is_empty()) {
        cmd.args(["--model", model]);
    } else if matches!(request.quality, OneShotQuality::Cheap) {
        cmd.args(["--model", "gemini-3.8-flash-low"]);
    }
    cmd.arg(format!("--print={message}"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .current_dir(std::env::temp_dir());
    prepare_env(&mut cmd);
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
    let bin = find_agy().ok_or_else(|| {
        anyhow!("agy not found on PATH — install Antigravity CLI and sign in first")
    })?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let skills_dir = Antigravity.session_skills_dir();
    let (repo, _playbook) =
        tokio::task::spawn_blocking(move || ensure_playbook(&project, &session_id, skills_dir))
            .await
            .map_err(|e| anyhow!("playbook task failed: {e}"))??;

    let resume = ctx.native_session_id.clone();
    let mut prompt = ctx.text.clone();
    if ctx.native_session_id.is_some() && resume.is_none() {
        if let Some(recovery) = super::native_recovery_context(ctx, "Antigravity") {
            prompt = format!("{recovery}\n\n{prompt}");
        }
    }
    if resume.is_none() {
        prompt = first_turn_prompt(&prompt);
    }

    let mut cmd = Command::new(&bin);
    // Crucial: Pass all option flags before `--print`
    cmd.args(["--output-format", "stream-json"]);

    if let Some(model) = ctx.model.as_deref().filter(|model| !model.is_empty()) {
        cmd.args(["--model", model]);
    }

    if let Some(effort) = &ctx.reasoning_level {
        if matches!(effort.as_str(), "low" | "medium" | "high") {
            cmd.args(["--effort", effort]);
        }
    }

    if ctx.plan_mode || ctx.permission_mode == Some(PermissionMode::Plan) {
        cmd.args(["--mode", "plan"]);
    } else {
        match ctx.permission_mode.unwrap_or(PermissionMode::Auto) {
            PermissionMode::Ask => {}
            PermissionMode::AcceptEdits => {
                cmd.args(["--mode", "accept-edits"]);
            }
            PermissionMode::Bypass => {
                cmd.arg("--dangerously-skip-permissions");
            }
            PermissionMode::Auto | PermissionMode::Plan => {}
        }
    }

    if let Some(native_id) = &resume {
        cmd.args(["--conversation", native_id]);
    }

    cmd.arg(format!("--print={prompt}"));
    cmd.current_dir(&repo);
    cmd.args(["--add-dir", repo.to_string_lossy().as_ref()]);

    let log_name = format!("antigravity-{}", uuid::Uuid::new_v4());
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log(&log_name)?))
        .kill_on_drop(true);

    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    set_chat_session_env(&mut cmd, &ctx.session_id, "antigravity", ctx.host.up_port());

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
                if let Some(sid) = state.conversation_id.as_deref() {
                    ctx.set_native_session_id(sid);
                }
                ctx.maybe_flush();
                if terminal {
                    break;
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                return Err(anyhow!("antigravity stdout: {error}"));
            }
            Err(_) => {
                return Err(anyhow!(
                    "Antigravity went silent for {} minutes and was interrupted.",
                    TURN_WATCHDOG.as_secs() / 60
                ));
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|error| anyhow!("wait for Antigravity child: {error}"))?;

    if !status.success() && !state.saw_result {
        return Err(anyhow!("Antigravity ended with error ({status})"));
    }

    if (ctx.plan_mode || ctx.permission_mode == Some(PermissionMode::Plan)) && !state.turn_errored {
        if let Some(card) = plan_card(&ctx.assistant.parts, &ctx.assistant.id, state.turn_errored) {
            ctx.upsert_part(card);
        }
    }

    Ok(())
}

#[derive(Default)]
struct TurnState {
    conversation_id: Option<String>,
    text_part_id: Option<String>,
    text_seq: usize,
    saw_result: bool,
    turn_errored: bool,
}

fn apply_event(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) -> bool {
    let event_type = event.get("event").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "init" => {
            if let Some(cid) = event.get("conversation_id").and_then(Value::as_str) {
                state.conversation_id = Some(cid.to_string());
            }
            false
        }
        "step_update" => {
            if let Some(step) = event.get("step_update") {
                if let Some(cid) = step.get("conversation_id").and_then(Value::as_str) {
                    state.conversation_id = Some(cid.to_string());
                }
                let step_type = step.get("step_type").and_then(Value::as_str).unwrap_or("");
                let step_state = step.get("state").and_then(Value::as_str).unwrap_or("");

                match step_type {
                    "agent_response" => {
                        if let Some(delta) = step.get("text_delta").and_then(Value::as_str) {
                            if !delta.is_empty() {
                                let id = match &state.text_part_id {
                                    Some(id) => id.clone(),
                                    None => {
                                        state.text_seq += 1;
                                        let id = format!("text-{}", state.text_seq);
                                        ctx.upsert_part(WirePart::text(id.clone(), ""));
                                        state.text_part_id = Some(id.clone());
                                        id
                                    }
                                };
                                ctx.append_part_text(&id, delta);
                            }
                        }
                        if step_state == "DONE" {
                            state.text_part_id = None;
                        }
                    }
                    "tool" => {
                        state.text_part_id = None;
                        let tool_name = step
                            .get("tool_name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let tool_info = step.get("tool_info").unwrap_or(&Value::Null);
                        let step_index =
                            step.get("step_index").and_then(Value::as_i64).unwrap_or(0);
                        let call_id = format!("tool-{step_index}-{tool_name}");

                        let title = match tool_name {
                            "run_command" => "Bash",
                            "view_file" => "Read",
                            "write_to_file" | "replace_file_content" => "Edit",
                            "list_dir" | "grep_search" | "find_by_name" => "Search",
                            "read_url_content" | "search_web" => "Web",
                            other => other,
                        };

                        let is_done = step_state == "DONE";
                        let params = tool_info.get("parameters").cloned();
                        let output = tool_info.get("output").and_then(Value::as_str);
                        let error = tool_info.get("error").and_then(Value::as_str);

                        if let Some(part) = find_part_mut(&mut ctx.assistant.parts, &call_id) {
                            if let Some(part_state) = part.state.as_mut() {
                                if is_done {
                                    part_state.status = if error.is_some() {
                                        "error"
                                    } else {
                                        "completed"
                                    }
                                    .into();
                                    if let Some(out) = output {
                                        part_state.output = Some(out.to_string());
                                    }
                                    if let Some(err) = error {
                                        part_state.error = Some(err.to_string());
                                    }
                                }
                            }
                        } else {
                            let status = if is_done {
                                if error.is_some() {
                                    "error"
                                } else {
                                    "completed"
                                }
                            } else {
                                "running"
                            };
                            ctx.upsert_part(WirePart {
                                id: call_id,
                                kind: "tool".into(),
                                text: None,
                                tool: Some(tool_name.to_string()),
                                state: Some(WireToolState {
                                    status: status.into(),
                                    input: params,
                                    output: output.map(str::to_string),
                                    error: error.map(str::to_string),
                                    title: Some(title.to_string()),
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
            false
        }
        "result" => {
            state.saw_result = true;
            if let Some(res) = event.get("result") {
                if let Some(cid) = res.get("conversation_id").and_then(Value::as_str) {
                    state.conversation_id = Some(cid.to_string());
                }
                let status = res.get("status").and_then(Value::as_str).unwrap_or("");
                if status == "ERROR" {
                    state.turn_errored = true;
                    let err = res
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("Antigravity execution failed");
                    ctx.push_error(err.to_string());
                } else {
                    ctx.mark_final_text_tail();
                }
            }
            true
        }
        _ => false,
    }
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
    use serde_json::json;

    fn fold(events: &[Value]) -> (TurnCtx, TurnState) {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for event in events {
            apply_event(&mut ctx, &mut state, event);
        }
        (ctx, state)
    }

    #[test]
    fn stream_folds_init_text_tools_and_result() {
        let (mut ctx, mut state) = fold(&[
            json!({
                "event": "init",
                "conversation_id": "test-conv-123",
                "init": {"tools": ["run_command", "view_file"]}
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 0,
                    "state": "DONE",
                    "step_type": "user_input"
                }
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 1,
                    "state": "ACTIVE",
                    "step_type": "agent_response",
                    "text_delta": "Checking directory "
                }
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 1,
                    "state": "DONE",
                    "step_type": "agent_response",
                    "text_delta": "contents..."
                }
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 2,
                    "state": "ACTIVE",
                    "step_type": "tool",
                    "tool_name": "run_command",
                    "tool_info": {
                        "name": "run_command",
                        "parameters": {"CommandLine": "ls -la"}
                    }
                }
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 2,
                    "state": "DONE",
                    "step_type": "tool",
                    "tool_name": "run_command",
                    "tool_info": {
                        "name": "run_command",
                        "parameters": {"CommandLine": "ls -la"},
                        "output": "file.txt\n"
                    }
                }
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 3,
                    "state": "ACTIVE",
                    "step_type": "agent_response",
                    "text_delta": "Found file.txt"
                }
            }),
            json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 3,
                    "state": "DONE",
                    "step_type": "agent_response",
                    "text_delta": "."
                }
            }),
        ]);

        assert_eq!(state.conversation_id.as_deref(), Some("test-conv-123"));
        assert_eq!(ctx.assistant.parts.len(), 3);
        assert_eq!(ctx.assistant.parts[0].kind, "text");
        assert_eq!(
            ctx.assistant.parts[0].text.as_deref(),
            Some("Checking directory contents...")
        );

        assert_eq!(ctx.assistant.parts[1].kind, "tool");
        let tool_state = ctx.assistant.parts[1].state.as_ref().unwrap();
        assert_eq!(tool_state.status, "completed");
        assert_eq!(tool_state.title.as_deref(), Some("Bash"));
        assert_eq!(tool_state.output.as_deref(), Some("file.txt\n"));

        assert_eq!(ctx.assistant.parts[2].kind, "text");
        assert_eq!(
            ctx.assistant.parts[2].text.as_deref(),
            Some("Found file.txt.")
        );

        let done = apply_event(
            &mut ctx,
            &mut state,
            &json!({
                "event": "result",
                "result": {
                    "conversation_id": "test-conv-123",
                    "status": "SUCCESS",
                    "response": "Done"
                }
            }),
        );
        assert!(done);
        assert!(state.saw_result);
        assert!(!state.turn_errored);
    }

    #[test]
    fn error_result_marks_turn_errored() {
        let (ctx, state) = fold(&[json!({
            "event": "result",
            "result": {
                "conversation_id": "conv-err",
                "status": "ERROR",
                "error": "Quota limit exceeded"
            }
        })]);
        assert!(state.saw_result);
        assert!(state.turn_errored);
        assert_eq!(
            ctx.assistant
                .parts
                .last()
                .and_then(|p| p.state.as_ref())
                .and_then(|s| s.error.as_deref()),
            Some("Quota limit exceeded")
        );
    }

    #[test]
    fn parses_agy_model_list_output() {
        let sample = "Fetching available models...\n\
                      gemini-3.8-flash-high\tGemini 3.8 Flash (High)\n\
                      gemini-3.1-pro-high\tGemini 3.1 Pro (High)\n\
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

    #[tokio::test]
    async fn live_detect_antigravity_if_installed() {
        if find_agy().is_none() {
            return;
        }
        let info = Antigravity.detect().await;
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.id, "antigravity");
        assert!(info.installed);
        assert!(!info.install_broken);
        assert!(info.agent_ready);
        assert!(!info.models.is_empty());
    }

    #[tokio::test]
    async fn live_one_shot_antigravity_if_installed() {
        if find_agy().is_none() {
            return;
        }
        let reply = Antigravity
            .one_shot(OneShot {
                system: "",
                prompt: "respond with the single word PONG",
                quality: OneShotQuality::Cheap,
                model: None,
                timeout: Duration::from_secs(20),
            })
            .await;
        assert!(reply.is_some());
        let reply = reply.unwrap();
        assert!(reply.to_lowercase().contains("pong"));
    }

    #[tokio::test]
    async fn live_generate_title_if_installed() {
        if find_agy().is_none() {
            return;
        }
        let title = Antigravity
            .generate_title("Add support for Google Antigravity harness", None)
            .await;
        assert!(title.is_some());
        let title = title.unwrap();
        assert!(!title.is_empty());
    }

    #[tokio::test]
    async fn live_registry_detects_antigravity_if_installed() {
        if find_agy().is_none() {
            return;
        }
        let harnesses = crate::local::harness::detect_harnesses().await;
        let agy_harness = harnesses.into_iter().find(|h| h.id == "antigravity");
        assert!(agy_harness.is_some());
        let agy = agy_harness.unwrap();
        assert_eq!(agy.name, "Google Antigravity");
        assert!(agy.installed);
        assert!(agy.agent_ready);
        assert!(!agy.models.is_empty());
    }
}
