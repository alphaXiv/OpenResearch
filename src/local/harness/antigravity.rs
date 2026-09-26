//! Google Antigravity harness.
//!
//! Chat: one `agy --output-format stream-json` child per turn. Multi-turn continues
//! via `--conversation <conversation_id>` from the init/result `conversation_id`. Isolated
//! ORX worktrees are the child's current working directory.
//!
//! The playbook is pointed at on the first turn (the file is already in the
//! worktree via [`ensure_playbook`]); session skills land in `.agents/skills`.
//!
//! Headless tools use the OpenResearch approval hook; explicit bypass skips its cards.
//!
//! Detection: `agy` on PATH or in `~/.local/bin`, `~/.gemini/bin`, or `~/.gemini/antigravity-cli/bin`;
//! `agy models` for catalog and authentication verification.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use super::detect::{resolve_symlinks, HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{
    HarnessOptions, OptionChoice, PermissionMode, PlanActivation, REASONING_DEFAULT_ID,
};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult, TURN_WATCHDOG};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, harness_log, prepare_env, set_chat_session_env, DeliveryState, PromptAnswer,
    ResumeCtx, TurnCtx, WirePart, WirePrompt, WireToolState,
};
use crate::local::opencode::{ensure_playbook, PLAYBOOK_REL};
use crate::local::shell_env::{find_in_dir, find_on_path};

const AGY_REINSTALL: &str =
    "Reinstall Antigravity CLI via curl -fsSL https://antigravity.google/cli/install.sh | bash";
const MODELS_TIMEOUT: Duration = Duration::from_secs(15);

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
                "Sign in by running `agy` in your terminal, then re-check this harness."
                    .to_string(),
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
            if let Some(native_id) = &prompt.native_id {
                if !ctx.is_busy().await {
                    ctx.host
                        .resolve_zombie_prompt(&ctx.session_id, &answer.prompt_id);
                    return Err(anyhow!("this approval is no longer pending"));
                }
                let decision = if answer.approve {
                    crate::local::chat::PermissionDecision::Allow {
                        updated_input: prompt.tool_input.clone(),
                    }
                } else {
                    crate::local::chat::PermissionDecision::Deny {
                        message: format!(
                            "The user denied this action. Do not retry it. {}",
                            answer.note.as_deref().unwrap_or("")
                        ),
                    }
                };
                ctx.host.settle_permission(native_id, decision)?;
                return Ok(ResumeAction::Handled { plan_mode: None });
            }
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
        Some(dirs::home_dir()?.join(".gemini").join("antigravity-cli"))
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

/// `agy` on PATH, then common install locations under `~/.local/bin`,
/// `~/.gemini/bin`, or `~/.gemini/antigravity-cli/bin`, in preference order.
fn agy_candidates() -> Vec<PathBuf> {
    let home_dirs = dirs::home_dir().into_iter().flat_map(|home| {
        let gemini = home.join(".gemini");
        [
            home.join(".local").join("bin"),
            gemini.join("bin"),
            gemini.join("antigravity-cli").join("bin"),
        ]
    });
    let drops = home_dirs
        .chain(dirs::data_local_dir().map(|dir| dir.join("agy").join("bin")))
        .filter_map(|dir| find_in_dir(&dir, "agy"));
    find_on_path("agy")
        .into_iter()
        .chain(drops)
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
    let mut cmd = Command::new(bin);
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

    let up_port = ctx
        .host
        .up_port()
        .ok_or_else(|| anyhow!("Antigravity requires the OpenResearch approval bridge"))?;
    let bypass = ctx.permission_mode.unwrap_or(PermissionMode::Bypass) == PermissionMode::Bypass;
    let hook_enabled = !bypass || ctx.plan_mode;
    write_approval_hook(&repo, hook_enabled)?;
    let resume = ctx.native_session_id.clone();
    let mut prompt = ctx.text.clone();
    if resume.is_none() {
        prompt = first_turn_prompt(&prompt);
    }

    let mut cmd = Command::new(&bin);
    cmd.args([
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
    ]);

    if let Some(model) = ctx.model.as_deref().filter(|model| !model.is_empty()) {
        cmd.args(["--model", model]);
    }

    // Native headless permission checks deny even after a hook allows the action.
    cmd.arg("--dangerously-skip-permissions");
    if ctx.plan_mode {
        cmd.arg("--mode=plan");
    }
    cmd.args(["--print-timeout", "60m"]);

    if let Some(native_id) = &resume {
        cmd.args(["--conversation", native_id]);
    }

    cmd.current_dir(&repo);
    cmd.arg("--add-dir").arg(&repo);

    let log_name = format!("antigravity-{}", uuid::Uuid::new_v4());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log(&log_name)?))
        .kill_on_drop(true);

    prepare_env(&mut cmd);
    cmd.env("NO_COLOR", "1");
    set_chat_session_env(&mut cmd, &ctx.session_id, "antigravity", Some(up_port));
    cmd.env("ORX_SESSION_ID", &ctx.session_id);
    cmd.env(
        "ORX_GATE_TOKEN",
        ctx.host
            .mint_gate_token(&ctx.session_id, ctx.plan_mode, bypass),
    );
    cmd.env(
        "ORX_AGY_GATE",
        if bypass && !ctx.plan_mode {
            "bypass"
        } else {
            "ask"
        },
    );

    ctx.persist_delivery(DeliveryState::Unknown)?;
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => {
            ctx.mark_delivery(DeliveryState::NotSent);
            return Err(anyhow!("Could not spawn {}: {}", bin.display(), error));
        }
    };
    let mut cancellation = TurnProcesses(child.id());
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let message =
        serde_json::json!({"event":"user","message":{"content":prompt}}).to_string() + "\n";
    tokio::time::timeout(TURN_WATCHDOG, stdin.write_all(message.as_bytes()))
        .await
        .map_err(|_| anyhow!("Antigravity did not read the prompt"))??;
    drop(stdin);
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut state = TurnState::default();

    loop {
        match tokio::time::timeout(TURN_WATCHDOG, lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if matches!(
                    event.get("event").and_then(Value::as_str),
                    Some("step_update")
                ) {
                    ctx.mark_delivery(DeliveryState::Accepted);
                }
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
            Err(_) if ctx.host.has_pending_permission(&ctx.session_id) => continue,
            Err(_) => {
                return Err(anyhow!(
                    "Antigravity went silent for {} minutes and was interrupted.",
                    TURN_WATCHDOG.as_secs() / 60
                ));
            }
        }
    }

    let status = tokio::time::timeout(TURN_WATCHDOG, child.wait())
        .await
        .map_err(|_| anyhow!("Antigravity did not exit after its response"))??;
    cancellation.0 = None;
    let log_path = crate::store::data_dir().join(format!("agent-{log_name}.log"));
    if !state.saw_result {
        return Err(anyhow!(
            "Antigravity ended without a result ({status}); see {}",
            log_path.display()
        ));
    }
    if !status.success() && !state.turn_errored {
        return Err(anyhow!("Antigravity ended with error ({status})"));
    }
    if ctx.plan_mode && !state.turn_errored {
        if let Some(card) = plan_card(&ctx.assistant.parts, &ctx.assistant.id) {
            ctx.upsert_part(card);
        }
    }
    if !state.turn_errored {
        let _ = std::fs::remove_file(log_path);
    }
    Ok(())
}

struct TurnProcesses(Option<u32>);

impl Drop for TurnProcesses {
    fn drop(&mut self) {
        if let Some(pid) = self.0 {
            stop_turn_process(pid);
        }
    }
}

#[cfg(unix)]
fn stop_turn_process(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    // Freeze each parent before discovering children: agy commands create their own sessions.
    if unsafe { libc::kill(pid, libc::SIGSTOP) } != 0 {
        return;
    }
    let snapshot = std::process::Command::new("ps")
        .args(["-ww", "-axo", "pid=,ppid=,args="])
        .output();
    let supervisor_exe = std::env::current_exe().ok();
    match snapshot {
        Ok(output) if output.status.success() => {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(child) = turn_child(line, pid, supervisor_exe.as_deref()) {
                    stop_turn_process(child);
                }
            }
        }
        _ => eprintln!("Could not inspect Antigravity descendants during cancellation"),
    }
    // SAFETY: this is the owned child or a descendant observed while its parent was stopped.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(unix)]
fn turn_child(line: &str, parent: libc::pid_t, supervisor_exe: Option<&Path>) -> Option<u32> {
    let mut fields = line.trim().splitn(2, char::is_whitespace);
    let pid = fields.next()?.parse().ok()?;
    let mut fields = fields.next()?.trim_start().splitn(2, char::is_whitespace);
    if fields.next()?.parse::<libc::pid_t>().ok()? != parent {
        return None;
    }
    let command = fields.next()?.trim_start();
    if let Some((exe, _)) = command.split_once(" supervise ") {
        if supervisor_exe.is_some_and(|expected| Path::new(exe) == expected)
            || (Path::new(exe).file_name().is_some_and(|name| name == "orx")
                && Path::new(exe).is_file())
        {
            return None;
        }
    }
    Some(pid)
}

#[cfg(not(unix))]
fn stop_turn_process(pid: u32) {
    let script = r#"
$ErrorActionPreference = 'Stop'
$processes = @(Get-CimInstance Win32_Process)
function Stop-TurnProcess([uint32] $processId) {
    $children = @($processes | Where-Object { $_.ParentProcessId -eq $processId })
    Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
    foreach ($child in $children) {
        if ($child.Name -eq 'orx.exe' -and $child.CommandLine -match '^(?:"[^"\r\n]+"|\S+)\s+supervise\s') { continue }
        Stop-TurnProcess $child.ProcessId
    }
}
Stop-TurnProcess ([uint32] $env:ORX_STOP_PID)
"#;
    let result = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .env("ORX_STOP_PID", pid.to_string())
        .stdout(Stdio::null())
        .status();
    if !result.is_ok_and(|status| status.success()) {
        eprintln!("Could not inspect Antigravity descendants during cancellation");
    }
}

fn write_approval_hook(repo: &Path, enabled: bool) -> Result<()> {
    std::fs::create_dir_all(repo)?;
    let tracked = std::process::Command::new("git")
        .args(["ls-files", "--error-unmatch", ".agents/hooks.json"])
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if tracked.success() {
        return Err(anyhow!("This project tracks .agents/hooks.json. Antigravity approval setup requires an untracked hook file and will not modify the tracked file."));
    }
    let path = repo.join(".agents/hooks.json");
    let mut hooks = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.into()),
    };
    let object = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow!("Antigravity hooks must be an object"))?;
    let exe = std::env::current_exe()?;
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
            "The Antigravity approval bridge requires the executable name orx.exe"
        );
        // prepare_env puts this executable's directory first on PATH.
        "orx antigravity-gate".to_string()
    };
    object.insert(
        "openresearch-approval".into(),
        serde_json::json!({
            "enabled": enabled,
            "PreToolUse": [{"matcher":"*","hooks":[{"type":"command","command":command,"timeout":3600}]}]
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
        "run_command" => ("Bash", &[("CommandLine", "command"), ("Cwd", "cwd")]),
        "view_file" => ("Read", &[("AbsolutePath", "file_path")]),
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

fn error_text(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| value.get("message")?.as_str())
}

fn step_is_terminal(state: &str) -> bool {
    matches!(state, "DONE" | "ERROR" | "CANCELED")
}

fn denied_actions_error(result: &Value) -> Option<String> {
    if result
        .get("response")
        .and_then(Value::as_str)
        .is_some_and(|response| !response.trim().is_empty())
    {
        return None;
    }
    let actions = result.get("denied_actions")?.as_array()?;
    if actions.is_empty() {
        return None;
    }
    let names = actions
        .iter()
        .filter_map(|action| {
            action
                .get("display_name")
                .or_else(|| action.get("action"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>();
    Some(if names.is_empty() {
        "Antigravity denied one or more required actions".into()
    } else {
        format!(
            "Antigravity denied required action(s): {}",
            names.join(", ")
        )
    })
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
            if let Some(cid) = event
                .get("conversation_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                state.conversation_id = Some(cid.to_string());
            }
            false
        }
        "step_update" => {
            if let Some(step) = event.get("step_update") {
                if let Some(cid) = step
                    .get("conversation_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
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
                        if step_is_terminal(step_state) {
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

                        let (tool, params) = normalize_tool(tool_name, tool_info.get("parameters"));
                        let output = tool_info.get("output").and_then(Value::as_str);
                        let error =
                            tool_info
                                .get("error")
                                .and_then(error_text)
                                .or(match step_state {
                                    "ERROR" => Some("Antigravity tool failed"),
                                    "CANCELED" => Some("Antigravity tool was canceled"),
                                    _ => None,
                                });
                        let is_done = step_is_terminal(step_state) || error.is_some();

                        let status = if !is_done {
                            "running"
                        } else if error.is_some() {
                            "error"
                        } else {
                            "completed"
                        };
                        if let Some(part) = find_part_mut(&mut ctx.assistant.parts, &call_id) {
                            if let Some(part_state) = part.state.as_mut() {
                                if params.is_some() {
                                    part_state.input = params;
                                }
                                if is_done {
                                    part_state.status = status.into();
                                    if let Some(out) = output {
                                        part_state.output = Some(out.to_string());
                                    }
                                    if let Some(err) = error {
                                        part_state.error = Some(err.to_string());
                                    }
                                }
                            }
                        } else {
                            ctx.upsert_part(WirePart {
                                id: call_id,
                                kind: "tool".into(),
                                text: None,
                                tool: Some(tool.to_string()),
                                state: Some(WireToolState {
                                    status: status.into(),
                                    input: params,
                                    output: output.map(str::to_string),
                                    error: error.map(str::to_string),
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
            false
        }
        "result" => {
            state.saw_result = true;
            let res = event.get("result").unwrap_or(&Value::Null);
            if let Some(cid) = res
                .get("conversation_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                state.conversation_id = Some(cid.to_string());
            }
            let status = res
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("INVALID");
            if status != "SUCCESS" {
                state.turn_errored = true;
                let error = res.get("error").and_then(error_text).unwrap_or(status);
                ctx.mark_terminal_failure("antigravity_terminal", format!("Antigravity: {error}"));
            } else if let Some(error) = denied_actions_error(res) {
                ctx.mark_delivery(DeliveryState::Accepted);
                state.turn_errored = true;
                ctx.mark_terminal_failure("antigravity_permission_denied", error);
            } else {
                ctx.mark_delivery(DeliveryState::Accepted);
                ctx.mark_final_text_tail();
            }
            true
        }
        _ => false,
    }
}

fn plan_card(parts: &[WirePart], assistant_id: &str) -> Option<WirePart> {
    let last_text = parts.iter().rev().find_map(|part| {
        (part.kind == "text")
            .then_some(part.text.as_deref())
            .flatten()
            .filter(|text| !text.trim().is_empty())
    })?;
    if !super::should_synthesize_plan(true, false, false, last_text) {
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
        assert_eq!(ctx.assistant.parts[1].tool.as_deref(), Some("Bash"));
        assert_eq!(tool_state.input.as_ref().unwrap()["command"], "ls -la");
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
    fn non_success_results_never_finish_the_answer() {
        for status in [
            "ERROR",
            "CANCELED",
            "INTERRUPTED",
            "INVALID",
            "WAITING",
            "RUNNING",
        ] {
            let (ctx, state) = fold(&[json!({
                "event": "result",
                "result": {"conversation_id": "", "status": status, "error": "Quota limit exceeded"}
            })]);
            assert!(state.saw_result);
            assert!(state.turn_errored, "{status}");
            assert!(state.conversation_id.is_none());
            assert!(ctx.assistant.parts.is_empty());
        }
    }

    #[test]
    fn tool_updates_preserve_inputs_and_surface_object_errors() {
        let (ctx, _) = fold(&[
            json!({"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"tool","tool_name":"view_file","tool_info":{"parameters":{"AbsolutePath":"/repo/file.rs"}}}}),
            json!({"event":"step_update","step_update":{"step_index":1,"state":"DONE","step_type":"tool","tool_name":"view_file","tool_info":{"error":{"type":"permission","message":"Denied"}}}}),
        ]);
        let part = &ctx.assistant.parts[0];
        assert_eq!(part.tool.as_deref(), Some("Read"));
        let state = part.state.as_ref().unwrap();
        assert_eq!(state.status, "error");
        assert_eq!(state.error.as_deref(), Some("Denied"));
        assert_eq!(state.input.as_ref().unwrap()["file_path"], "/repo/file.rs");
        for name in [
            "write_to_file",
            "replace_file_content",
            "multi_replace_file_content",
        ] {
            let (_, input) = normalize_tool(name, Some(&json!({"TargetFile":"/repo/file.rs"})));
            assert_eq!(input.unwrap()["file_path"], "/repo/file.rs");
        }
    }

    #[test]
    fn permission_denial_is_not_a_successful_turn() {
        let (ctx, state) = fold(&[
            json!({"event":"step_update","step_update":{"step_index":2,"state":"ERROR","step_type":"tool","tool_name":"view_file","tool_info":{"error":{"type":"TOOL_ERROR","message":"Permission denied"}}}}),
            json!({"event":"result","result":{"status":"SUCCESS","response":"","denied_actions":[{"action":"read_file","display_name":"ViewFile"}]}}),
        ]);
        assert!(state.turn_errored);
        let tool = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(tool.status, "error");
        assert_eq!(tool.error.as_deref(), Some("Permission denied"));
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_preserves_detached_experiment_supervisors() {
        let supervisor = Some(Path::new("/path with spaces/orx"));
        assert_eq!(
            turn_child(" 12 10 /bin/sh -c sleep 60", 10, supervisor),
            Some(12)
        );
        assert_eq!(turn_child(" 13 12 sleep 60", 10, supervisor), None);
        assert_eq!(
            turn_child(
                " 14 10 /path with spaces/orx supervise run-id",
                10,
                supervisor
            ),
            None
        );
        assert_eq!(
            turn_child(
                " 17 10 /bin/sh -c /path with spaces/orx supervise run-id",
                10,
                supervisor
            ),
            Some(17)
        );
        assert_eq!(
            turn_child(" 15 10 /path/orx exp run experiment-id", 10, supervisor),
            Some(15)
        );
        assert_eq!(
            turn_child(" 16 10 python -c print('supervise')", 10, supervisor),
            Some(16)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_command_descendants() {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 60 & echo $!; wait"])
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let cancellation = TurnProcesses(child.id());
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let pid: libc::pid_t = lines.next_line().await.unwrap().unwrap().parse().unwrap();
        drop(cancellation);
        child.wait().await.unwrap();
        for _ in 0..100 {
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("command descendant survived cancellation: {pid}");
    }

    #[test]
    fn approval_setup_leaves_tracked_hooks_untouched() {
        let repo = std::env::temp_dir().join(format!("orx-agy-hooks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(repo.join(".agents")).unwrap();
        let path = repo.join(".agents/hooks.json");
        std::fs::write(&path, "{}").unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args(["add", ".agents/hooks.json"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        assert!(write_approval_hook(&repo, true)
            .unwrap_err()
            .to_string()
            .contains("tracks .agents/hooks.json"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{}");
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn approval_hook_respects_enabled_flag() {
        let repo =
            std::env::temp_dir().join(format!("orx-agy-hooks-flag-{}", uuid::Uuid::new_v4()));
        write_approval_hook(&repo, false).unwrap();
        let path = repo.join(".agents/hooks.json");
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["openresearch-approval"]["enabled"], false);

        write_approval_hook(&repo, true).unwrap();
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["openresearch-approval"]["enabled"], true);
        std::fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn failed_tool_states_are_terminal_even_without_error_payloads() {
        for (step_state, expected_error) in [
            ("ERROR", "Antigravity tool failed"),
            ("CANCELED", "Antigravity tool was canceled"),
        ] {
            let (ctx, _) = fold(&[json!({
                "event": "step_update",
                "step_update": {
                    "step_index": 1,
                    "state": step_state,
                    "step_type": "tool",
                    "tool_name": "run_command",
                    "tool_info": {"parameters": {"CommandLine": "false"}}
                }
            })]);
            let tool = ctx.assistant.parts[0].state.as_ref().unwrap();
            assert_eq!(tool.status, "error", "{step_state}");
            assert_eq!(tool.error.as_deref(), Some(expected_error), "{step_state}");
        }
    }

    #[test]
    fn an_error_payload_terminates_an_active_tool() {
        let (ctx, _) = fold(&[json!({
            "event": "step_update",
            "step_update": {
                "step_index": 1,
                "state": "ACTIVE",
                "step_type": "tool",
                "tool_name": "view_file",
                "tool_info": {"error": {"message": "Denied"}}
            }
        })]);
        let tool = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(tool.status, "error");
        assert_eq!(tool.error.as_deref(), Some("Denied"));
    }

    #[test]
    fn terminal_agent_response_clears_the_streamed_text_part() {
        for step_state in ["ERROR", "CANCELED"] {
            let (_, state) = fold(&[
                json!({"event":"step_update","step_update":{"step_index":1,"state":"ACTIVE","step_type":"agent_response","text_delta":"partial"}}),
                json!({"event":"step_update","step_update":{"step_index":1,"state":step_state,"step_type":"agent_response"}}),
            ]);
            assert!(state.text_part_id.is_none(), "{step_state}");
        }
    }

    #[test]
    fn empty_success_with_denied_actions_is_a_failed_turn() {
        let (ctx, state) = fold(&[json!({
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "",
                "denied_actions": [{"action": "command", "display_name": "RunCommand"}]
            }
        })]);
        assert!(state.saw_result);
        assert!(state.turn_errored);
        assert_eq!(ctx.delivery_state(), DeliveryState::Accepted);
        assert_eq!(
            denied_actions_error(&json!({
                "response": "",
                "denied_actions": [{"display_name": "RunCommand"}]
            }))
            .as_deref(),
            Some("Antigravity denied required action(s): RunCommand")
        );
    }

    #[test]
    fn denied_actions_do_not_discard_a_nonempty_response() {
        assert!(denied_actions_error(&json!({
            "response": "I could not run it, but here is an explanation.",
            "denied_actions": [{"display_name": "RunCommand"}]
        }))
        .is_none());
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
