//! Shared driver for harnesses that speak the Agent Client Protocol (ACP):
//! JSON-RPC 2.0, one message per line over the child's stdio.
//!
//! One `<agent> acp` child runs per turn. The turn opens (or resumes) the
//! native session, applies the composer's mode/model/reasoning through the
//! session's advertised `modes` and `configOptions`, sends one
//! `session/prompt`, and folds the `session/update` stream into wire parts
//! until the prompt answers with a `stopReason`.
//!
//! Tool approvals arrive as `session/request_permission` requests. Outside
//! bypass they become the same held permission card the Claude bridge uses
//! ([`crate::local::chat::ChatHost::request_permission`]); the harness settles
//! it from `resume_from_prompt` with [`settle_permission_prompt`].
//!
//! The client advertises no `fs` or `terminal` capability, so the agent keeps
//! using its own file and shell tools inside the session worktree.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, oneshot};

use super::options::PermissionMode;
use super::{ResumeAction, TURN_WATCHDOG};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, harness_log, prepare_env, set_chat_session_env, ContextUsage, DeliveryState,
    PermissionDecision, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt, WireToolState,
};
use crate::local::opencode::{ensure_playbook, PLAYBOOK_REL};

/// JSON-RPC code agents use for "not signed in" (`Authentication required`).
const AUTH_REQUIRED: i64 = -32000;
const METHOD_NOT_FOUND: i64 = -32601;

/// How one ACP harness is launched and configured.
pub(crate) struct AcpAgent {
    pub harness_id: &'static str,
    pub display: &'static str,
    /// Shown when the agent answers `Authentication required`.
    pub login_hint: &'static str,
    /// Worktree-relative native skills dir, as for [`super::Harness::session_skills_dir`].
    pub skills_dir: Option<&'static str>,
    /// The native session mode and config values for a composer state.
    pub settings: fn(Option<PermissionMode>, bool) -> AcpSettings,
}

/// Session mode and config options to apply before the prompt.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AcpSettings {
    pub mode: Option<&'static str>,
    pub config: Vec<(&'static str, &'static str)>,
    /// Answer every permission request with its allow option, without a card.
    pub auto_allow: bool,
}

/// The executable and arguments that start the agent's ACP server.
pub(crate) struct AcpLaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(&'static str, OsString)>,
    /// Directories prepended to the child's PATH (e.g. a bundled Node runtime).
    pub path_prepend: Vec<PathBuf>,
}

/// Whether `path` is an Electron desktop app (`Kimi.exe`, `ZCode.exe`) rather
/// than a command-line tool. Windows resolves `kimi` to `Kimi.exe` because the
/// file system ignores case, and launching the app opens its window.
pub(crate) fn is_desktop_app(path: &Path) -> bool {
    path.parent()
        .is_some_and(|dir| dir.join("resources").join("app.asar").exists())
}

// --- JSON-RPC connection ---------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct RpcError {
    pub code: i64,
    pub message: String,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

type RpcResult = std::result::Result<Value, RpcError>;

/// A message the agent sent that is not a response to one of ours.
#[derive(Debug)]
enum Inbound {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}

#[derive(Clone)]
struct Connection {
    stdin: Arc<tokio::sync::Mutex<ChildStdin>>,
    next_id: Arc<AtomicI64>,
    pending: Arc<Mutex<HashMap<i64, oneshot::Sender<RpcResult>>>>,
}

impl Connection {
    async fn write(&self, message: Value) -> Result<()> {
        let mut line = message.to_string();
        line.push('\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Send a request; the returned receiver resolves with its response.
    async fn send(&self, method: &str, params: Value) -> Result<oneshot::Receiver<RpcResult>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        Ok(rx)
    }

    async fn respond(&self, id: Value, result: Value) -> Result<()> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await
    }

    async fn respond_error(&self, id: Value, code: i64, message: &str) -> Result<()> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
            .await
    }
}

/// Route one line from the agent: responses resolve their pending request,
/// everything else is returned for the turn loop.
fn route_line(
    line: &str,
    pending: &Mutex<HashMap<i64, oneshot::Sender<RpcResult>>>,
) -> Option<Inbound> {
    let message: Value = serde_json::from_str(line).ok()?;
    let method = message.get("method").and_then(Value::as_str);
    let id = message.get("id").filter(|id| !id.is_null());
    match (method, id) {
        (Some(method), Some(id)) => Some(Inbound::Request {
            id: id.clone(),
            method: method.to_string(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        }),
        (Some(method), None) => Some(Inbound::Notification {
            method: method.to_string(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        }),
        (None, Some(id)) => {
            let tx = pending.lock().unwrap().remove(&id.as_i64()?)?;
            let result = match message.get("error") {
                Some(error) => Err(RpcError {
                    code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed")
                        .to_string(),
                }),
                None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(result);
            None
        }
        (None, None) => None,
    }
}

// --- turn --------------------------------------------------------------------------

fn first_turn_prompt(text: &str) -> String {
    format!(
        "Read and follow `{PLAYBOOK_REL}` before acting. It is the OpenResearch session playbook for this worktree.\n\n{text}"
    )
}

/// Run one chat turn against an ACP agent.
pub(crate) async fn run_turn(ctx: &mut TurnCtx, agent: &AcpAgent, launch: AcpLaunch) -> Result<()> {
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let skills_dir = agent.skills_dir;
    let (repo, _playbook) =
        tokio::task::spawn_blocking(move || ensure_playbook(&project, &session_id, skills_dir))
            .await
            .map_err(|e| anyhow!("playbook task failed: {e}"))??;

    let log_name = format!("{}-{}", agent.harness_id, uuid::Uuid::new_v4());
    let mut cmd = Command::new(&launch.program);
    cmd.args(&launch.args)
        .current_dir(&repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(harness_log(&log_name)?))
        .kill_on_drop(true);
    prepare_env(&mut cmd);
    if !launch.path_prepend.is_empty() {
        let current = cmd
            .as_std()
            .get_envs()
            .find(|(key, _)| key.eq_ignore_ascii_case("PATH"))
            .and_then(|(_, value)| value.map(OsString::from))
            .or_else(|| std::env::var_os("PATH"))
            .unwrap_or_default();
        let joined = std::env::join_paths(
            launch
                .path_prepend
                .iter()
                .cloned()
                .chain(std::env::split_paths(&current)),
        )
        .map_err(|e| anyhow!("invalid PATH entry: {e}"))?;
        cmd.env("PATH", joined);
    }
    for (key, value) in &launch.env {
        cmd.env(key, value);
    }
    cmd.env("NO_COLOR", "1");
    set_chat_session_env(
        &mut cmd,
        &ctx.session_id,
        agent.harness_id,
        ctx.host.up_port(),
    );

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
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let conn = Connection {
        stdin: Arc::new(tokio::sync::Mutex::new(stdin)),
        next_id: Arc::new(AtomicI64::new(1)),
        pending: Arc::new(Mutex::new(HashMap::new())),
    };
    let (inbound_tx, mut inbound) = mpsc::unbounded_channel();
    let reader_pending = conn.pending.clone();
    let reader = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(message) = route_line(&line, &reader_pending) {
                if inbound_tx.send(message).is_err() {
                    break;
                }
            }
        }
        // EOF: fail every request still waiting so the turn does not hang.
        for (_, tx) in reader_pending.lock().unwrap().drain() {
            let _ = tx.send(Err(RpcError {
                code: 0,
                message: "the agent exited".into(),
            }));
        }
    });
    let log_path = crate::store::data_dir().join(format!("agent-{log_name}.log"));
    let outcome = drive(ctx, agent, &conn, &mut inbound, &repo).await;
    reader.abort();
    drop(conn);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
    match outcome {
        Ok(()) => {
            let _ = std::fs::remove_file(log_path);
            Ok(())
        }
        Err(error) => Err(anyhow!("{error} (log: {})", log_path.display())),
    }
}

/// Await a response while still answering the agent's own requests (it may
/// ask for permission or send updates before it replies).
async fn call(
    ctx: &mut TurnCtx,
    conn: &Connection,
    inbound: &mut mpsc::UnboundedReceiver<Inbound>,
    state: &mut TurnState,
    method: &str,
    params: Value,
) -> Result<RpcResult> {
    let mut rx = conn.send(method, params).await?;
    loop {
        tokio::select! {
            response = &mut rx => {
                return Ok(response.unwrap_or_else(|_| Err(RpcError { code: 0, message: "the agent exited".into() })));
            }
            message = inbound.recv() => {
                let Some(message) = message else {
                    return Ok(Err(RpcError { code: 0, message: "the agent exited".into() }));
                };
                handle_inbound(ctx, conn, state, message).await?;
                ctx.maybe_flush();
            }
            _ = tokio::time::sleep(TURN_WATCHDOG) => {
                if ctx.host.has_pending_permission(&ctx.session_id) {
                    continue;
                }
                return Err(anyhow!(
                    "The agent went silent for {} minutes and was interrupted.",
                    TURN_WATCHDOG.as_secs() / 60
                ));
            }
        }
    }
}

async fn drive(
    ctx: &mut TurnCtx,
    agent: &AcpAgent,
    conn: &Connection,
    inbound: &mut mpsc::UnboundedReceiver<Inbound>,
    repo: &Path,
) -> Result<()> {
    let settings = (agent.settings)(ctx.permission_mode, ctx.plan_mode);
    let had_native_session = ctx.native_session_id.is_some();
    let mut state = TurnState {
        gate_token: ctx.host.mint_gate_token(
            &ctx.session_id,
            ctx.plan_mode,
            settings.auto_allow && !ctx.plan_mode,
        ),
        auto_allow: settings.auto_allow && !ctx.plan_mode,
        ..TurnState::default()
    };

    let init = call(
        ctx,
        conn,
        inbound,
        &mut state,
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
            "clientInfo": {"name": "openresearch", "version": env!("CARGO_PKG_VERSION")},
        }),
    )
    .await?
    .map_err(|e| anyhow!("{} did not start its ACP server: {e}", agent.display))?;
    let capabilities = init
        .get("agentCapabilities")
        .cloned()
        .unwrap_or(Value::Null);
    let cwd = repo.to_string_lossy().into_owned();

    // Resume the native session when there is one; replayed history from a
    // `session/load` is dropped (orx already holds the transcript).
    let mut session: Option<(String, Value)> = None;
    if let Some(native_id) = ctx.native_session_id.clone() {
        let method = if capabilities
            .pointer("/sessionCapabilities/resume")
            .is_some()
        {
            Some("session/resume")
        } else if capabilities.get("loadSession").and_then(Value::as_bool) == Some(true) {
            Some("session/load")
        } else {
            None
        };
        if let Some(method) = method {
            state.replaying = true;
            let resumed = call(
                ctx,
                conn,
                inbound,
                &mut state,
                method,
                json!({"sessionId": native_id, "cwd": cwd, "mcpServers": []}),
            )
            .await?;
            state.replaying = false;
            match resumed {
                Ok(result) => session = Some((native_id, result)),
                Err(error) if error.code == AUTH_REQUIRED => {
                    return Err(anyhow!(
                        "{} is not signed in. {}",
                        agent.display,
                        agent.login_hint
                    ))
                }
                Err(_) => {}
            }
        }
    }
    let resumed = session.is_some();
    let (session_id, opened) = match session {
        Some(session) => session,
        None => {
            let result = call(
                ctx,
                conn,
                inbound,
                &mut state,
                "session/new",
                json!({"cwd": cwd, "mcpServers": []}),
            )
            .await?
            .map_err(|error| {
                if error.code == AUTH_REQUIRED {
                    anyhow!("{} is not signed in. {}", agent.display, agent.login_hint)
                } else {
                    anyhow!("{} could not open a session: {error}", agent.display)
                }
            })?;
            let id = result
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("{} opened a session without an id", agent.display))?
                .to_string();
            (id, result)
        }
    };
    ctx.set_native_session_id(&session_id);
    state.session_id = session_id.clone();

    for (method, params) in session_setup(
        &session_id,
        &opened,
        &settings,
        ctx.model.as_deref(),
        ctx.reasoning_level.as_deref(),
    ) {
        if let Err(error) = call(ctx, conn, inbound, &mut state, method, params).await? {
            eprintln!("{}: {method} failed: {error}", agent.harness_id);
        }
    }

    let mut text = ctx.text.clone();
    if had_native_session && !resumed {
        if let Some(recovery) = super::native_recovery_context(ctx, agent.display) {
            text = format!("{recovery}\n\n{text}");
        }
    }
    if !resumed {
        text = first_turn_prompt(&text);
    }
    let response = call(
        ctx,
        conn,
        inbound,
        &mut state,
        "session/prompt",
        json!({"sessionId": session_id, "prompt": [{"type": "text", "text": text}]}),
    )
    .await?;
    ctx.mark_delivery(DeliveryState::Accepted);
    let result = response.map_err(|error| {
        if error.code == AUTH_REQUIRED {
            anyhow!("{} is not signed in. {}", agent.display, agent.login_hint)
        } else {
            anyhow!("{}: {}", agent.display, error.message)
        }
    })?;
    match result.get("stopReason").and_then(Value::as_str) {
        Some("refusal") => ctx.mark_terminal_failure(
            "acp_refusal",
            format!("{} refused to continue", agent.display),
        ),
        Some("max_tokens") => ctx.mark_terminal_failure(
            "acp_max_tokens",
            format!("{} hit its output token limit", agent.display),
        ),
        Some("max_turn_requests") => ctx.mark_terminal_failure(
            "acp_max_turn_requests",
            format!("{} hit its per-turn request limit", agent.display),
        ),
        _ => {}
    }
    if ctx.plan_mode {
        if let Some(card) = synthesized_plan_card(&ctx.assistant.parts, &ctx.assistant.id) {
            ctx.upsert_part(card);
        }
    }
    let _ = ctx.flush();
    Ok(())
}

/// The `session/set_mode` / `session/set_config_option` calls that move a
/// freshly opened session onto the composer's state. Only values the session
/// advertised are sent, and only when they differ from its current value.
fn session_setup(
    session_id: &str,
    opened: &Value,
    settings: &AcpSettings,
    model: Option<&str>,
    reasoning: Option<&str>,
) -> Vec<(&'static str, Value)> {
    let mut calls = Vec::new();
    if let Some(mode) = settings.mode {
        let modes = opened.get("modes");
        let available = modes
            .and_then(|m| m.get("availableModes"))
            .and_then(Value::as_array)
            .is_some_and(|all| {
                all.iter()
                    .any(|m| m.get("id").and_then(Value::as_str) == Some(mode))
            });
        let current = modes
            .and_then(|m| m.get("currentModeId"))
            .and_then(Value::as_str);
        if available && current != Some(mode) {
            calls.push((
                "session/set_mode",
                json!({"sessionId": session_id, "modeId": mode}),
            ));
        }
    }
    let options = opened
        .get("configOptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut wanted: Vec<(String, String)> = settings
        .config
        .iter()
        .map(|(id, value)| (id.to_string(), value.to_string()))
        .collect();
    if let Some(model) = model.filter(|m| !m.is_empty()) {
        if let Some(option) = option_by(&options, "model") {
            wanted.push((option_id(option), model.to_string()));
        }
    }
    if let Some(level) = reasoning.filter(|l| *l != super::options::REASONING_DEFAULT_ID) {
        if let Some(option) = option_by(&options, "thought_level") {
            wanted.push((option_id(option), level.to_string()));
        }
    }
    for (id, value) in wanted {
        let Some(option) = options
            .iter()
            .find(|o| o.get("id").and_then(Value::as_str) == Some(&id))
        else {
            continue;
        };
        let offered = option
            .get("options")
            .and_then(Value::as_array)
            .is_some_and(|all| {
                all.iter()
                    .any(|o| o.get("value").and_then(Value::as_str) == Some(&value))
            });
        let current = option.get("currentValue").and_then(Value::as_str);
        if offered && current != Some(value.as_str()) {
            calls.push((
                "session/set_config_option",
                json!({"sessionId": session_id, "configId": id, "value": value}),
            ));
        }
    }
    calls
}

fn option_by<'a>(options: &'a [Value], category: &str) -> Option<&'a Value> {
    options.iter().find(|o| {
        o.get("category").and_then(Value::as_str) == Some(category)
            || o.get("id").and_then(Value::as_str) == Some(category)
    })
}

fn option_id(option: &Value) -> String {
    option
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// --- inbound messages ----------------------------------------------------------------

#[derive(Default)]
struct TurnState {
    session_id: String,
    gate_token: String,
    auto_allow: bool,
    /// Updates arriving while `session/load` replays history are dropped.
    replaying: bool,
    text_part_id: Option<String>,
    message_id: Option<String>,
    reasoning_part_id: Option<String>,
    seq: usize,
}

impl TurnState {
    fn next_id(&mut self, prefix: &str) -> String {
        self.seq += 1;
        format!("{prefix}-{}", self.seq)
    }

    fn close_segments(&mut self) {
        self.text_part_id = None;
        self.reasoning_part_id = None;
    }
}

async fn handle_inbound(
    ctx: &mut TurnCtx,
    conn: &Connection,
    state: &mut TurnState,
    message: Inbound,
) -> Result<()> {
    match message {
        Inbound::Notification { method, params } if method == "session/update" => {
            if !state.replaying {
                if let Some(update) = params.get("update") {
                    apply_update(ctx, state, update);
                }
            }
        }
        Inbound::Notification { .. } => {}
        Inbound::Request { id, method, params } if method == "session/request_permission" => {
            handle_permission(ctx, conn, state, id, params).await?;
        }
        Inbound::Request { id, method, .. } => {
            conn.respond_error(
                id,
                METHOD_NOT_FOUND,
                &format!("{method} is not supported by this client"),
            )
            .await?;
        }
    }
    Ok(())
}

/// Answer a permission request: straight away in bypass, otherwise through a
/// held card whose answer arrives on a background task.
async fn handle_permission(
    ctx: &mut TurnCtx,
    conn: &Connection,
    state: &mut TurnState,
    id: Value,
    params: Value,
) -> Result<()> {
    let options = params
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let allow = pick_option(&options, &["allow_once", "allow_always"]);
    let reject = pick_option(&options, &["reject_once", "reject_always"]);
    if state.auto_allow {
        return conn.respond(id, selected_outcome(allow.as_deref())).await;
    }
    let tool_call = params.get("toolCall").cloned().unwrap_or(Value::Null);
    let (tool, input) = permission_tool(&tool_call);
    let host = ctx.host.clone();
    let session_id = ctx.session_id.clone();
    let token = state.gate_token.clone();
    let conn = conn.clone();
    // The card blocks until the user answers; the turn keeps reading updates.
    tokio::spawn(async move {
        let decision = host
            .request_permission(&session_id, &token, &tool, input)
            .await
            .unwrap_or_else(|error| PermissionDecision::Deny {
                message: error.to_string(),
            });
        let choice = match decision {
            PermissionDecision::Allow { .. } => allow,
            PermissionDecision::Deny { .. } => reject,
        };
        let _ = conn.respond(id, selected_outcome(choice.as_deref())).await;
    });
    Ok(())
}

fn pick_option(options: &[Value], kinds: &[&str]) -> Option<String> {
    kinds.iter().find_map(|kind| {
        options
            .iter()
            .find(|o| o.get("kind").and_then(Value::as_str) == Some(kind))
            .and_then(|o| o.get("optionId").and_then(Value::as_str))
            .map(str::to_string)
    })
}

fn selected_outcome(option: Option<&str>) -> Value {
    match option {
        Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

/// The tool name and input a permission card shows. Names follow the Claude
/// vocabulary so plan-mode's read-only policy recognizes reads and edits.
fn permission_tool(tool_call: &Value) -> (String, Value) {
    let kind = tool_call.get("kind").and_then(Value::as_str).unwrap_or("");
    let title = tool_call.get("title").and_then(Value::as_str).unwrap_or("");
    let mut input = tool_call
        .get("rawInput")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let (Some(object), false) = (input.as_object_mut(), title.is_empty()) {
        object
            .entry("description")
            .or_insert_with(|| Value::String(title.to_string()));
    }
    (tool_name(kind, title), input)
}

/// ACP tool `kind` → the tool name the UI renders.
fn tool_name(kind: &str, title: &str) -> String {
    match kind {
        "read" => "Read",
        "edit" => "Edit",
        "delete" => "Delete",
        "move" => "Move",
        "search" => "Grep",
        "execute" => "Bash",
        "fetch" => "WebFetch",
        "think" => "Think",
        _ if !title.is_empty() => return title.to_string(),
        _ => "Tool",
    }
    .to_string()
}

fn tool_status(status: Option<&str>) -> &'static str {
    match status {
        Some("completed") => "completed",
        Some("failed") => "error",
        _ => "running",
    }
}

/// Text carried by a tool call's `content` blocks (text, diffs, terminals).
fn tool_content_text(content: &Value) -> Option<String> {
    let blocks = content.as_array()?;
    let text = blocks
        .iter()
        .filter_map(|block| match block.get("type").and_then(Value::as_str) {
            Some("content") => block
                .pointer("/content/text")
                .and_then(Value::as_str)
                .map(str::to_string),
            Some("diff") => Some(format!(
                "{}\n{}",
                block.get("path").and_then(Value::as_str).unwrap_or(""),
                block.get("newText").and_then(Value::as_str).unwrap_or("")
            )),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn raw_output_text(raw: &Value) -> Option<String> {
    match raw {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// Fold one `session/update` into the assistant message.
fn apply_update(ctx: &mut TurnCtx, state: &mut TurnState, update: &Value) {
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("agent_message_chunk") => {
            let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
                return;
            };
            let message_id = update.get("messageId").and_then(Value::as_str);
            state.reasoning_part_id = None;
            let new_message = message_id.is_some() && message_id != state.message_id.as_deref();
            if new_message {
                state.message_id = message_id.map(str::to_string);
            }
            let id = match (&state.text_part_id, new_message) {
                (Some(id), false) => id.clone(),
                _ => {
                    let id = state.next_id("text");
                    ctx.upsert_part(WirePart::text(id.clone(), ""));
                    state.text_part_id = Some(id.clone());
                    id
                }
            };
            ctx.append_part_text(&id, text);
        }
        Some("agent_thought_chunk") => {
            let Some(text) = update.pointer("/content/text").and_then(Value::as_str) else {
                return;
            };
            if text.is_empty() {
                return;
            }
            state.text_part_id = None;
            let id = match &state.reasoning_part_id {
                Some(id) => id.clone(),
                None => {
                    let id = state.next_id("reasoning");
                    ctx.upsert_part(WirePart::reasoning(id.clone(), ""));
                    state.reasoning_part_id = Some(id.clone());
                    id
                }
            };
            ctx.append_part_text(&id, text);
        }
        Some("tool_call") => {
            state.close_segments();
            let Some(call_id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let kind = update.get("kind").and_then(Value::as_str).unwrap_or("");
            let title = update.get("title").and_then(Value::as_str).unwrap_or("");
            let status = tool_status(update.get("status").and_then(Value::as_str));
            let output = update
                .get("content")
                .and_then(tool_content_text)
                .or_else(|| update.get("rawOutput").and_then(raw_output_text));
            ctx.upsert_part(WirePart {
                id: call_id.to_string(),
                kind: "tool".into(),
                text: None,
                tool: Some(tool_name(kind, title)),
                state: Some(WireToolState {
                    status: status.into(),
                    input: update.get("rawInput").cloned(),
                    output: (status != "error").then(|| output.clone()).flatten(),
                    error: (status == "error").then_some(output).flatten(),
                    title: (!title.is_empty()).then(|| title.to_string()),
                }),
                prompt: None,
                phase: None,
                children: Vec::new(),
            });
        }
        Some("tool_call_update") => {
            state.close_segments();
            let Some(call_id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            if find_part_mut(&mut ctx.assistant.parts, call_id).is_none() {
                ctx.upsert_part(WirePart::tool(call_id, "Tool", "running", None));
            }
            let status = update.get("status").and_then(Value::as_str);
            let output = update
                .get("content")
                .and_then(tool_content_text)
                .or_else(|| update.get("rawOutput").and_then(raw_output_text));
            if let Some(part) = find_part_mut(&mut ctx.assistant.parts, call_id) {
                if let Some(kind) = update.get("kind").and_then(Value::as_str) {
                    let title = update.get("title").and_then(Value::as_str).unwrap_or("");
                    part.tool = Some(tool_name(kind, title));
                }
                let part_state = part.state.get_or_insert_with(|| WireToolState {
                    status: "running".into(),
                    input: None,
                    output: None,
                    error: None,
                    title: None,
                });
                if let Some(status) = status {
                    part_state.status = tool_status(Some(status)).into();
                }
                if let Some(title) = update.get("title").and_then(Value::as_str) {
                    part_state.title = Some(title.to_string());
                }
                if let Some(input) = update.get("rawInput") {
                    part_state.input = Some(input.clone());
                }
                if let Some(output) = output {
                    if part_state.status == "error" {
                        part_state.error = Some(output);
                    } else {
                        part_state.output = Some(output);
                    }
                }
            }
        }
        Some("usage_update") => {
            if let Some(used) = update.get("used").and_then(Value::as_u64) {
                ctx.report_usage(ContextUsage {
                    used_tokens: used,
                    context_window: update.get("size").and_then(Value::as_u64),
                });
            }
        }
        Some("session_info_update") => {
            if let Some(title) = update.get("title").and_then(Value::as_str) {
                ctx.set_title(title);
            }
        }
        _ => {}
    }
}

/// A plan card from the turn's final text, for agents with no native plan tool.
pub(crate) fn synthesized_plan_card(parts: &[WirePart], assistant_id: &str) -> Option<WirePart> {
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

/// `resume_from_prompt` for ACP harnesses: permission cards are answered
/// inline over the live turn; an approved or annotated plan card continues
/// with a new message that leaves plan mode.
pub(crate) async fn resume_from_prompt(
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
                PermissionDecision::Allow {
                    updated_input: prompt.tool_input.clone(),
                }
            } else {
                PermissionDecision::Deny {
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

/// The config option values a session advertised for `category`, as
/// `(value, name)` pairs — read from a recorded or probed `session/new`.
#[cfg(test)]
pub(crate) fn advertised_values(opened: &Value, category: &str) -> Vec<(String, String)> {
    let options = opened
        .get("configOptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    option_by(&options, category)
        .and_then(|o| o.get("options"))
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter_map(|o| {
                    Some((
                        o.get("value")?.as_str()?.to_string(),
                        o.get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The agent → client messages of a recorded exchange (`<< ` lines).
    pub(crate) fn recorded(fixture: &str) -> Vec<Value> {
        fixture
            .lines()
            .filter_map(|line| line.strip_prefix("<< "))
            .map(|line| serde_json::from_str(line).expect("fixture line is JSON"))
            .collect()
    }

    pub(crate) fn opened_session(fixture: &str) -> Value {
        recorded(fixture)
            .into_iter()
            .find(|m| m.get("id") == Some(&json!(2)))
            .and_then(|m| m.get("result").cloned())
            .expect("fixture has a session/new response")
    }

    pub(crate) fn fold(fixture: &str) -> TurnCtx {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for message in recorded(fixture) {
            if message.get("method").and_then(Value::as_str) == Some("session/update") {
                if let Some(update) = message.pointer("/params/update") {
                    apply_update(&mut ctx, &mut state, update);
                }
            }
        }
        ctx
    }

    #[test]
    fn routes_responses_requests_and_notifications() {
        let pending = Mutex::new(HashMap::new());
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(7, tx);
        assert!(route_line(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#, &pending).is_none());
        assert_eq!(rx.try_recv().unwrap().unwrap(), json!({"ok": true}));

        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(8, tx);
        route_line(
            r#"{"jsonrpc":"2.0","id":8,"error":{"code":-32000,"message":"Authentication required"}}"#,
            &pending,
        );
        let error = rx.try_recv().unwrap().unwrap_err();
        assert_eq!(error.code, AUTH_REQUIRED);

        assert!(matches!(
            route_line(r#"{"jsonrpc":"2.0","id":"p1","method":"session/request_permission","params":{}}"#, &pending),
            Some(Inbound::Request { method, .. }) if method == "session/request_permission"
        ));
        assert!(matches!(
            route_line(r#"{"jsonrpc":"2.0","method":"session/update","params":{}}"#, &pending),
            Some(Inbound::Notification { method, .. }) if method == "session/update"
        ));
        assert!(route_line("not json", &pending).is_none());
    }

    #[test]
    fn folds_thoughts_then_answer_from_a_recorded_kimi_turn() {
        let ctx = fold(include_str!("fixtures/kimi_acp_ok.jsonl"));
        let parts = &ctx.assistant.parts;
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].kind, "reasoning");
        assert!(parts[0]
            .text
            .as_deref()
            .unwrap()
            .starts_with("We need respond"));
        assert_eq!(parts[1].kind, "text");
        assert_eq!(parts[1].text.as_deref(), Some("OK"));
    }

    #[test]
    fn folds_a_recorded_minimax_turn_and_its_usage() {
        let ctx = fold(include_str!("fixtures/minimax_acp_ok.jsonl"));
        let texts: Vec<_> = ctx
            .assistant
            .parts
            .iter()
            .filter(|p| p.kind == "text")
            .filter_map(|p| p.text.as_deref())
            .collect();
        assert_eq!(texts, vec!["OK"]);
        let usage = ctx.context_usage.expect("usage_update reported");
        assert_eq!(usage.used_tokens, 14486);
        assert_eq!(usage.context_window, Some(512000));
    }

    #[test]
    fn tool_calls_stream_status_input_and_output() {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        apply_update(
            &mut ctx,
            &mut state,
            &json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Checking."}}),
        );
        apply_update(
            &mut ctx,
            &mut state,
            &json!({
                "sessionUpdate": "tool_call", "toolCallId": "call-1", "title": "Run ls", "kind": "execute",
                "status": "pending", "rawInput": {"command": "ls"}
            }),
        );
        apply_update(
            &mut ctx,
            &mut state,
            &json!({
                "sessionUpdate": "tool_call_update", "toolCallId": "call-1", "status": "completed",
                "content": [{"type": "content", "content": {"type": "text", "text": "a.txt"}}]
            }),
        );
        apply_update(
            &mut ctx,
            &mut state,
            &json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Done."}}),
        );
        let parts = &ctx.assistant.parts;
        assert_eq!(parts.len(), 3);
        let tool = &parts[1];
        assert_eq!(tool.tool.as_deref(), Some("Bash"));
        let tool_state = tool.state.as_ref().unwrap();
        assert_eq!(tool_state.status, "completed");
        assert_eq!(tool_state.input, Some(json!({"command": "ls"})));
        assert_eq!(tool_state.output.as_deref(), Some("a.txt"));
        assert_eq!(parts[2].text.as_deref(), Some("Done."));
    }

    #[test]
    fn failed_tool_output_becomes_the_error() {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        apply_update(
            &mut ctx,
            &mut state,
            &json!({"sessionUpdate": "tool_call", "toolCallId": "c", "kind": "read", "status": "in_progress"}),
        );
        apply_update(
            &mut ctx,
            &mut state,
            &json!({"sessionUpdate": "tool_call_update", "toolCallId": "c", "status": "failed", "rawOutput": "no such file"}),
        );
        let tool_state = ctx.assistant.parts[0].state.as_ref().unwrap();
        assert_eq!(tool_state.status, "error");
        assert_eq!(tool_state.error.as_deref(), Some("no such file"));
    }

    #[test]
    fn new_message_ids_start_new_text_parts() {
        let mut ctx = TurnCtx::test_stub();
        let mut state = TurnState::default();
        for (message, text) in [("m1", "a"), ("m1", "b"), ("m2", "c")] {
            apply_update(
                &mut ctx,
                &mut state,
                &json!({"sessionUpdate": "agent_message_chunk", "messageId": message, "content": {"type": "text", "text": text}}),
            );
        }
        let texts: Vec<_> = ctx
            .assistant
            .parts
            .iter()
            .filter_map(|p| p.text.as_deref())
            .collect();
        assert_eq!(texts, vec!["ab", "c"]);
    }

    #[test]
    fn setup_applies_only_advertised_values_that_differ() {
        let opened = opened_session(include_str!("fixtures/kimi_acp_ok.jsonl"));
        let settings = AcpSettings {
            mode: Some("yolo"),
            config: vec![("nonexistent", "x")],
            auto_allow: true,
        };
        let calls = session_setup("s1", &opened, &settings, Some("kimi-code/k3"), Some("low"));
        let methods: Vec<_> = calls.iter().map(|(m, p)| (*m, p.clone())).collect();
        assert_eq!(
            methods,
            vec![
                (
                    "session/set_mode",
                    json!({"sessionId": "s1", "modeId": "yolo"})
                ),
                (
                    "session/set_config_option",
                    json!({"sessionId": "s1", "configId": "model", "value": "kimi-code/k3"})
                ),
                (
                    "session/set_config_option",
                    json!({"sessionId": "s1", "configId": "thinking", "value": "low"})
                ),
            ]
        );
        // Current values and unknown models are left alone.
        let calls = session_setup(
            "s1",
            &opened,
            &AcpSettings {
                mode: Some("default"),
                ..Default::default()
            },
            Some("kimi-code/kimi-for-coding"),
            Some("unknown-level"),
        );
        assert!(calls.is_empty());
    }

    #[test]
    fn permission_options_map_to_allow_and_reject() {
        let options = vec![
            json!({"optionId": "yes", "kind": "allow_once", "name": "Allow"}),
            json!({"optionId": "always", "kind": "allow_always", "name": "Always"}),
            json!({"optionId": "no", "kind": "reject_once", "name": "Reject"}),
        ];
        assert_eq!(
            pick_option(&options, &["allow_once", "allow_always"]).as_deref(),
            Some("yes")
        );
        assert_eq!(
            pick_option(&options, &["reject_once", "reject_always"]).as_deref(),
            Some("no")
        );
        assert_eq!(
            selected_outcome(Some("yes")),
            json!({"outcome": {"outcome": "selected", "optionId": "yes"}})
        );
        assert_eq!(
            selected_outcome(None),
            json!({"outcome": {"outcome": "cancelled"}})
        );
        let (tool, input) = permission_tool(
            &json!({"kind": "edit", "title": "Edit a.py", "rawInput": {"path": "a.py"}}),
        );
        assert_eq!(tool, "Edit");
        assert_eq!(input, json!({"path": "a.py", "description": "Edit a.py"}));
    }
}
