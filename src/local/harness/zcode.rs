//! ZCode harness (Z.ai's coding agent).
//!
//! Chat: one `zcode -p <prompt> --output-format stream-json` child per turn,
//! continued with `--resume <sessionId>`. The stream is one JSON event per
//! line: `model.streaming` (text, reasoning and tool-call deltas),
//! `tool.updated` (tool results), `permission.resolved`, and a closing
//! `turn.completed` or `turn.failed`.
//!
//! Print mode has no approval channel: in `build`/`edit` mode ZCode denies
//! high-risk tools itself ("No permission client configured"). So the
//! composer offers `edit` (edits allowed, risky commands denied) and `yolo`
//! (everything allowed); Plan runs in `build` with a planning instruction.
//!
//! Detection: a `zcode` CLI (official `~/.zcode/runtime` install, npm, PATH),
//! else the runtime bundled with the ZCode desktop app (`resources/glm/zcode.cjs`)
//! run through the app's own Electron binary as Node (`ELECTRON_RUN_AS_NODE=1`).
//! The desktop login in `~/.zcode/v2` is shared with both.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

use super::detect::{HarnessAuthState, HarnessInfo};
use super::options::{HarnessOptions, OptionChoice, PermissionMode, PlanActivation};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult, TURN_WATCHDOG};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, harness_log, prepare_env, set_chat_session_env, DeliveryState, PromptAnswer,
    ResumeCtx, TurnCtx, WirePart, WirePrompt, WireToolState,
};
use crate::local::opencode::{ensure_playbook, PLAYBOOK_REL};
use crate::local::shell_env::{find_in_dir, find_on_path};

const KEY: &str = "zcode";
const INSTALL_HINT: &str =
    "Install the ZCode desktop app from https://zcode.z.ai and sign in there, then re-check this harness.";
const LOGIN_HINT: &str =
    "Sign in to Z.ai in the ZCode desktop app (or configure a provider in ~/.zcode/v2/provider_config.json), then re-check this harness.";
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

pub struct ZCode;

/// How to start the ZCode runtime.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Launch {
    pub program: PathBuf,
    /// Arguments before ZCode's own (the bundled script path).
    pub prefix: Vec<OsString>,
    pub env: Vec<(&'static str, OsString)>,
    pub bundled: bool,
}

impl Launch {
    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.prefix);
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }
}

/// The desktop app's resources directory for this platform, if installed.
fn desktop_candidates() -> Vec<(PathBuf, PathBuf)> {
    let mut found = Vec::new();
    if cfg!(windows) {
        if let Some(local) = dirs::data_local_dir() {
            let dir = local.join("Programs").join("ZCode");
            found.push((dir.join("ZCode.exe"), dir.join("resources")));
        }
    } else if cfg!(target_os = "macos") {
        let apps = [PathBuf::from("/Applications")]
            .into_iter()
            .chain(dirs::home_dir().map(|home| home.join("Applications")));
        for apps in apps {
            let contents = apps.join("ZCode.app").join("Contents");
            found.push((
                contents.join("MacOS").join("ZCode"),
                contents.join("Resources"),
            ));
        }
    } else {
        for dir in [PathBuf::from("/opt/ZCode"), PathBuf::from("/usr/lib/zcode")] {
            found.push((dir.join("zcode"), dir.join("resources")));
        }
    }
    found
}

/// The runtime bundled with the desktop app, launched as Node.
fn bundled_launch_in(exe: &Path, resources: &Path) -> Option<Launch> {
    let script = resources.join("glm").join("zcode.cjs");
    if !exe.is_file() || !script.is_file() {
        return None;
    }
    let mut env = vec![("ELECTRON_RUN_AS_NODE", OsString::from("1"))];
    let builtin = resources
        .join("config")
        .join("provider")
        .join("zcode-builtin.json");
    if builtin.is_file() {
        env.push((
            "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE",
            builtin.into_os_string(),
        ));
    }
    Some(Launch {
        program: exe.to_path_buf(),
        prefix: vec![script.into_os_string()],
        env,
        bundled: true,
    })
}

/// A standalone `zcode` CLI — never the desktop app, which Windows would
/// otherwise match for `zcode` because its file system ignores case.
fn cli_candidates() -> Vec<PathBuf> {
    let home_bins = dirs::home_dir().into_iter().flat_map(|home| {
        [
            home.join(".local").join("bin"),
            home.join(".zcode").join("runtime").join("bin"),
            home.join(".zcode").join("runtime"),
        ]
    });
    let found = find_on_path("zcode")
        .into_iter()
        .chain(home_bins.filter_map(|dir| find_in_dir(&dir, "zcode")))
        .filter(|path| !super::acp::is_desktop_app(path))
        .map(super::detect::resolve_symlinks)
        .collect();
    super::detect::unique(found)
}

pub(crate) fn find_launch() -> Option<Launch> {
    if let Some(program) = cli_candidates().into_iter().next() {
        return Some(Launch {
            program,
            prefix: Vec::new(),
            env: Vec::new(),
            bundled: false,
        });
    }
    desktop_candidates()
        .into_iter()
        .find_map(|(exe, resources)| bundled_launch_in(&exe, &resources))
}

fn zcode_home() -> Option<PathBuf> {
    crate::local::shell_env::var("ZCODE_DATA_BASE_DIR")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .map(|base| base.join(".zcode"))
}

/// Whether ZCode has model access: a desktop/CLI Z.ai login, or a personal
/// provider with an API key.
fn has_access(home: &Path) -> bool {
    let v2 = home.join("v2");
    if v2
        .join("credentials.json")
        .metadata()
        .is_ok_and(|meta| meta.len() > 2)
    {
        return true;
    }
    let Some(config) = super::detect::read_json(v2.join("provider_config.json")) else {
        return false;
    };
    config
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array)
        .is_some_and(|rules| {
            rules.iter().any(|rule| {
                rule.get("enabled").and_then(Value::as_bool) != Some(false)
                    && rule
                        .pointer("/config/access/apiKey")
                        .and_then(Value::as_str)
                        .is_some_and(|key| !key.trim().is_empty())
            })
        })
}

async fn version(launch: &Launch) -> Option<String> {
    let mut cmd = launch.command();
    cmd.arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let out = super::detect::detect_spawn_output_timed(cmd, VERSION_TIMEOUT)
        .await?
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

impl ZCode {
    async fn detect_at(&self, snapshot: bool) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        if let Some(launch) = find_launch() {
            info.installed = true;
            info.bin_path = Some(launch.program.to_string_lossy().into_owned());
            if !snapshot {
                match version(&launch).await {
                    Some(version) => info.version = Some(version),
                    None if launch.bundled => {
                        info.install_broken = true;
                        info.agent_note = Some(
                            "The ZCode desktop app's runtime did not start from the command line. Update ZCode, or install the ZCode CLI, then re-check this harness."
                                .into(),
                        );
                    }
                    None => {}
                }
            }
            if !info.install_broken {
                if zcode_home().is_some_and(|home| has_access(&home)) {
                    info.authenticated = true;
                    info.auth_state = HarnessAuthState::Ready;
                    info.auth_method = Some("oauth");
                } else {
                    info.auth_state = HarnessAuthState::NeedsLogin;
                    info.agent_note = Some(LOGIN_HINT.into());
                }
            }
        } else {
            info.agent_note = Some(INSTALL_HINT.into());
        }
        info.agent_ready = info.ready();
        Some(info)
    }
}

#[async_trait]
impl Harness for ZCode {
    fn id(&self) -> &'static str {
        KEY
    }

    fn name(&self) -> &'static str {
        "ZCode"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        self.detect_at(false).await
    }

    async fn detect_snapshot(&self) -> Option<HarnessInfo> {
        self.detect_at(true).await
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        run_turn(ctx)
            .await
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    fn options(&self) -> HarnessOptions {
        HarnessOptions::none().with_permission_choices(
            vec![
                OptionChoice::described(
                    "accept-edits",
                    "Edit",
                    "Allow file edits; ZCode denies high-risk commands",
                ),
                OptionChoice::described("bypass", "YOLO", "Allow every tool"),
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
        super::acp::resume_from_prompt(ctx, prompt, answer).await
    }

    fn config_home(&self) -> Option<PathBuf> {
        zcode_home()
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
        Some(".agents/skills")
    }
}

fn zcode_mode(mode: Option<PermissionMode>, plan: bool) -> &'static str {
    if plan || mode == Some(PermissionMode::Plan) {
        return "build";
    }
    match mode.unwrap_or(PermissionMode::Bypass) {
        PermissionMode::Bypass => "yolo",
        PermissionMode::Ask => "build",
        PermissionMode::AcceptEdits | PermissionMode::Auto | PermissionMode::Plan => "edit",
    }
}

fn turn_prompt(text: &str, first: bool, plan: bool) -> String {
    let mut prompt = String::new();
    if first {
        prompt.push_str(&format!(
            "Read and follow `{PLAYBOOK_REL}` before acting. It is the OpenResearch session playbook for this worktree.\n\n"
        ));
    }
    if plan {
        prompt.push_str("Plan mode: investigate read-only and reply with a concrete plan. Do not modify files or run commands that change state.\n\n");
    }
    prompt.push_str(text);
    prompt
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    let launch = find_launch().ok_or_else(|| anyhow!("ZCode not found. {INSTALL_HINT}"))?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let (repo, _playbook) = tokio::task::spawn_blocking(move || {
        ensure_playbook(&project, &session_id, Some(".agents/skills"))
    })
    .await
    .map_err(|e| anyhow!("playbook task failed: {e}"))??;

    let resume = ctx.native_session_id.clone();
    let plan = ctx.plan_mode || ctx.permission_mode == Some(PermissionMode::Plan);
    let prompt = turn_prompt(&ctx.text, resume.is_none(), plan);

    let log_name = format!("zcode-{}", uuid::Uuid::new_v4());
    let mut cmd = launch.command();
    cmd.arg("-p")
        .arg(&prompt)
        .args(["--output-format", "stream-json", "--no-color", "--mode"])
        .arg(zcode_mode(ctx.permission_mode, ctx.plan_mode))
        .arg("--cwd")
        .arg(&repo);
    if let Some(native_id) = &resume {
        cmd.args(["--resume", native_id]);
    }
    cmd.current_dir(&repo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log(&log_name)?))
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    // prepare_env may reset the environment; the launch's own variables win.
    for (key, value) in &launch.env {
        cmd.env(key, value);
    }
    cmd.env("NO_COLOR", "1");
    set_chat_session_env(&mut cmd, &ctx.session_id, KEY, ctx.host.up_port());

    ctx.persist_delivery(DeliveryState::Unknown)?;
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            ctx.mark_delivery(DeliveryState::NotSent);
            return Err(anyhow!(
                "Could not spawn {}: {}",
                launch.program.display(),
                error
            ));
        }
    };
    let _processes = super::antigravity::TurnProcesses(child.id());
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
                apply_event(ctx, &mut state, &event);
                if let Some(sid) = state.session_id.as_deref() {
                    ctx.set_native_session_id(sid);
                }
                ctx.maybe_flush();
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(anyhow!("zcode stdout: {error}")),
            Err(_) => {
                return Err(anyhow!(
                    "ZCode went silent for {} minutes and was interrupted.",
                    TURN_WATCHDOG.as_secs() / 60
                ))
            }
        }
    }
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait())
        .await
        .map_err(|_| anyhow!("ZCode did not exit after its response"))??;
    let log_path = crate::store::data_dir().join(format!("agent-{log_name}.log"));
    if let Some(message) = state.failure.take() {
        ctx.mark_terminal_failure("zcode_turn_failed", message);
    } else if !state.completed {
        let detail = std::fs::read_to_string(&log_path)
            .ok()
            .and_then(|log| {
                log.lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_default();
        return Err(anyhow!(
            "ZCode ended without a result ({status}). {detail} (log: {})",
            log_path.display()
        ));
    } else {
        let _ = std::fs::remove_file(&log_path);
    }
    if plan {
        if let Some(card) =
            super::acp::synthesized_plan_card(&ctx.assistant.parts, &ctx.assistant.id)
        {
            ctx.upsert_part(card);
        }
    }
    let _ = ctx.flush();
    Ok(())
}

#[derive(Default)]
struct TurnState {
    session_id: Option<String>,
    text_part: Option<(String, String)>,
    reasoning_part: Option<(String, String)>,
    completed: bool,
    failure: Option<String>,
}

fn apply_event(ctx: &mut TurnCtx, state: &mut TurnState, event: &Value) {
    if let Some(sid) = event.get("sessionId").and_then(Value::as_str) {
        if state.session_id.as_deref() != Some(sid) {
            state.session_id = Some(sid.to_string());
        }
    }
    let payload = event.get("payload").unwrap_or(&Value::Null);
    match event.get("type").and_then(Value::as_str) {
        Some("model.streaming") => apply_streaming(ctx, state, payload),
        Some("tool.updated") => apply_tool(ctx, payload),
        Some("permission.resolved") => {
            if payload.get("decision").and_then(Value::as_str) == Some("deny") {
                if let Some(call_id) = payload.get("toolCallId").and_then(Value::as_str) {
                    let reason = payload
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or("denied")
                        .to_string();
                    if let Some(part_state) = find_part_mut(&mut ctx.assistant.parts, call_id)
                        .and_then(|part| part.state.as_mut())
                    {
                        part_state.status = "error".into();
                        part_state.error = Some(format!("ZCode denied this tool: {reason}"));
                    }
                }
            }
        }
        Some("session.titleUpdated") => {
            if let Some(title) = payload.get("title").and_then(Value::as_str) {
                ctx.set_title(title);
            }
        }
        Some("turn.completed") => state.completed = true,
        Some("turn.failed") => {
            let message = payload
                .pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("ZCode reported a failed turn");
            state.failure = Some(message.to_string());
        }
        _ => {}
    }
}

fn apply_streaming(ctx: &mut TurnCtx, state: &mut TurnState, payload: &Value) {
    let message = payload
        .get("assistantMessageId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let delta = payload.get("delta").and_then(Value::as_str).unwrap_or("");
    match payload.get("kind").and_then(Value::as_str) {
        Some("text_delta") if !delta.is_empty() => {
            state.reasoning_part = None;
            let id = match &state.text_part {
                Some((owner, id)) if *owner == message => id.clone(),
                _ => {
                    let id = format!("text-{message}-{}", ctx.assistant.parts.len());
                    ctx.upsert_part(WirePart::text(id.clone(), ""));
                    state.text_part = Some((message.clone(), id.clone()));
                    id
                }
            };
            ctx.append_part_text(&id, delta);
        }
        Some("reasoning_delta" | "thinking_delta") if !delta.is_empty() => {
            state.text_part = None;
            let id = match &state.reasoning_part {
                Some((owner, id)) if *owner == message => id.clone(),
                _ => {
                    let id = format!("reasoning-{message}-{}", ctx.assistant.parts.len());
                    ctx.upsert_part(WirePart::reasoning(id.clone(), ""));
                    state.reasoning_part = Some((message.clone(), id.clone()));
                    id
                }
            };
            ctx.append_part_text(&id, delta);
        }
        Some("text_end") => state.text_part = None,
        Some("reasoning_end") => state.reasoning_part = None,
        Some("tool_call") => {
            state.text_part = None;
            state.reasoning_part = None;
            let Some(call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let name = payload
                .get("toolName")
                .and_then(Value::as_str)
                .unwrap_or("Tool");
            ctx.upsert_part(WirePart {
                id: call_id.to_string(),
                kind: "tool".into(),
                text: None,
                tool: Some(name.to_string()),
                state: Some(WireToolState {
                    status: "running".into(),
                    input: payload.get("input").cloned(),
                    output: None,
                    error: None,
                    title: payload
                        .pointer("/input/description")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                prompt: None,
                phase: None,
                children: Vec::new(),
            });
        }
        _ => {}
    }
}

fn apply_tool(ctx: &mut TurnCtx, payload: &Value) {
    if payload.get("kind").and_then(Value::as_str) != Some("result") {
        return;
    }
    let Some(call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
        return;
    };
    let result = payload.get("result").unwrap_or(&Value::Null);
    let ok = result
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let content = match result.get("content").or_else(|| result.get("error")) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    if find_part_mut(&mut ctx.assistant.parts, call_id).is_none() {
        ctx.upsert_part(WirePart::tool(call_id, "Tool", "running", None));
    }
    if let Some(part_state) =
        find_part_mut(&mut ctx.assistant.parts, call_id).and_then(|part| part.state.as_mut())
    {
        part_state.status = if ok { "completed" } else { "error" }.into();
        if ok {
            part_state.output = Some(content);
        } else {
            part_state.error = Some(content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(fixture: &str) -> (TurnCtx, TurnState) {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for line in fixture.lines() {
            let event: Value = serde_json::from_str(line).expect("fixture line is JSON");
            apply_event(&mut ctx, &mut state, &event);
        }
        (ctx, state)
    }

    #[test]
    fn folds_a_recorded_tool_turn() {
        let (ctx, state) = fold(include_str!("fixtures/zcode_stream_tool.jsonl"));
        assert!(state.completed);
        assert_eq!(
            state.session_id.as_deref(),
            Some("sess_475edcb4-9d96-4355-bba3-aca8b8960880")
        );
        let parts = &ctx.assistant.parts;
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].text.as_deref(), Some("Listing files."));
        let tool = &parts[1];
        assert_eq!(tool.tool.as_deref(), Some("Bash"));
        let tool_state = tool.state.as_ref().unwrap();
        assert_eq!(tool_state.status, "completed");
        assert_eq!(tool_state.output.as_deref(), Some("a.txt"));
        assert_eq!(
            tool_state.input,
            Some(serde_json::json!({"command": "ls", "description": "List files"}))
        );
        assert_eq!(parts[2].text.as_deref(), Some("Done: OK"));
    }

    #[test]
    fn a_resumed_turn_keeps_its_session() {
        let (ctx, state) = fold(include_str!("fixtures/zcode_stream_resume.jsonl"));
        assert!(state.completed);
        assert_eq!(
            state.session_id.as_deref(),
            Some("sess_475edcb4-9d96-4355-bba3-aca8b8960880")
        );
        assert_eq!(ctx.assistant.parts[0].text.as_deref(), Some("Done: OK"));
    }

    #[test]
    fn a_failed_turn_reports_the_provider_error() {
        let (_, state) = fold(include_str!("fixtures/zcode_stream_failed.jsonl"));
        assert!(!state.completed);
        assert!(state
            .failure
            .as_deref()
            .is_some_and(|message| message.contains("Insufficient balance")));
    }

    #[test]
    fn a_denied_tool_shows_the_reason() {
        let (ctx, state) = fold(include_str!("fixtures/zcode_stream_denied.jsonl"));
        assert!(state.completed);
        let tool = ctx
            .assistant
            .parts
            .iter()
            .find(|part| part.kind == "tool")
            .unwrap();
        let tool_state = tool.state.as_ref().unwrap();
        assert_eq!(tool_state.status, "error");
        assert!(tool_state
            .error
            .as_deref()
            .unwrap()
            .contains("No permission client configured"));
    }

    #[test]
    fn composer_modes_map_onto_zcode_modes() {
        assert_eq!(zcode_mode(None, false), "yolo");
        assert_eq!(zcode_mode(Some(PermissionMode::AcceptEdits), false), "edit");
        assert_eq!(zcode_mode(Some(PermissionMode::Bypass), true), "build");
    }

    #[test]
    fn bundled_runtime_runs_the_desktop_binary_as_node() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-{}", uuid::Uuid::new_v4()));
        let resources = dir.join("resources");
        std::fs::create_dir_all(resources.join("glm")).unwrap();
        std::fs::create_dir_all(resources.join("config").join("provider")).unwrap();
        let exe = dir.join("ZCode.exe");
        std::fs::write(&exe, "").unwrap();
        assert!(bundled_launch_in(&exe, &resources).is_none());
        std::fs::write(resources.join("glm").join("zcode.cjs"), "").unwrap();
        std::fs::write(
            resources
                .join("config")
                .join("provider")
                .join("zcode-builtin.json"),
            "{}",
        )
        .unwrap();
        let launch = bundled_launch_in(&exe, &resources).unwrap();
        assert!(launch.bundled);
        assert_eq!(
            launch.prefix,
            vec![resources.join("glm").join("zcode.cjs").into_os_string()]
        );
        assert!(launch
            .env
            .iter()
            .any(|(k, v)| *k == "ELECTRON_RUN_AS_NODE" && v == "1"));
        assert!(launch
            .env
            .iter()
            .any(|(k, _)| *k == "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn access_comes_from_a_login_or_a_keyed_provider() {
        let dir = std::env::temp_dir().join(format!("orx-zcode-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("v2")).unwrap();
        assert!(!has_access(&dir));
        std::fs::write(
            dir.join("v2").join("provider_config.json"),
            r#"{"config":{"providerConfigRules":{"providerRules":[{"providerId":"c","config":{"access":{"type":"api-key","apiKey":"sk"}}}]}}}"#,
        )
        .unwrap();
        assert!(has_access(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }
}
