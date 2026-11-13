//! Persistent Claude Code host — one long-lived `claude --print --input-format
//! stream-json` child per chat session, mirroring `AgentHost`/`CodexHost`. The
//! child stays resident across turns: a stable `session_id` survives every turn,
//! stdin never closes, and each user message is one JSON line
//! (`{"type":"user","message":{"role":"user","content":[…]}}`). This replaces
//! today's spawn-per-turn + `--resume` fork (`harness/claude.rs`'s old
//! `run_turn`), whose ~3–6s of per-turn spawn/config/MCP-handshake overhead the
//! latency probe measured.
//!
//! Wire shapes were pinned live against claude CLI 2.1.197 (the echo contract
//! below re-verified on 2.1.200/2.1.212). In persistent mode `--print
//! --input-format stream-json --output-format stream-json` accepts one user
//! message per stdin line and ends each turn with a `result` event; the
//! `session_id` is stable across turns in one process, and `--resume <id>`
//! composes with stream-json input, so a crash/restart or config-change respawn
//! recovers cheaply. A `--resume` respawn can replay prior-session output
//! *before* touching the new message — including a stale `result` — so
//! `--replay-user-messages` re-emits each submitted stdin message on stdout and
//! the echo, not the send, is a turn's start boundary
//! (`harness/claude.rs`'s `belongs_to_current_turn`). The flag long predates
//! the pinned CLI version (the Claude Agent SDK's stream-json transport has
//! always passed it); a CLI without it fails loudly at spawn, never silently.
//!
//! Control requests ride the same stdin stream
//! (`{"type":"control_request","request_id":…,"request":{"subtype":…}}`) and are
//! answered by a `control_response`; `set_model` is the only one we use — native
//! `interrupt` gave no response and is treated as unreliable, so v1 interrupts
//! by killing the child (`ChatHost::interrupt`'s `claude-code` branch).
//!
//! Non-goals (separate tasks): MCP config isolation (`--strict-mcp-config`);
//! codex/opencode paths; the UI rendering fix.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, oneshot, Mutex, Notify};

use crate::error::{anyhow, Result};
use crate::local::agent_lifecycle::{IdlePolicy, REAPER_INTERVAL};
use crate::local::harness::claude::{
    claude_permission_mode, uses_permission_bridge, write_mcp_config, write_settings,
    CLAUDE_ENTRYPOINT,
};
use crate::local::harness::{HarnessAuthState, PermissionMode};
use crate::local::native_store::NativeStore;

/// Ceiling on a control request's response wait. Control requests (`set_model`)
/// are quick; a wedged child must never hold the respawn path for long.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

const STALE_AFTER: Duration = Duration::from_secs(90 * 60);

/// How long a child is kept after a background task finishes for the segment
/// reporting it to begin; the CLI starts it at once.
const TASK_REPORT_GRACE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LifecycleStopReason {
    IdleExpired,
    IdleLimit,
    Stale,
    Exited,
}

impl LifecycleStopReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::IdleExpired => "idle-expired",
            Self::IdleLimit => "idle-limit",
            Self::Stale => "stale",
            Self::Exited => "exited",
        }
    }
}

struct TurnSlot {
    sender: Option<mpsc::UnboundedSender<TurnEvent>>,
    idle_since: Instant,
    /// A turn has ended on this child, so a segment it starts with no turn
    /// listening is its own continuation rather than startup output.
    served: bool,
    /// Background tasks the child reported started and not yet finished. It
    /// resumes on its own when one finishes, so it must outlive them, even one
    /// that never ends.
    background: HashSet<String>,
    /// A segment the child began on its own after a turn ended, held until a
    /// turn takes it.
    unclaimed: Option<Vec<Value>>,
    /// A turn is starting to record `unclaimed`, so no other is started for it.
    /// If that turn never takes the lines, they wait for the chat's next turn.
    recording: bool,
    /// When a background task last finished. The child resumes to report it a
    /// moment later, so it is kept until then.
    task_finished_at: Option<Instant>,
}

impl TurnSlot {
    fn new(now: Instant) -> Self {
        Self {
            sender: None,
            idle_since: now,
            served: false,
            background: HashSet::new(),
            unclaimed: None,
            recording: false,
            task_finished_at: None,
        }
    }

    fn stop_reason(
        &self,
        started_at: Instant,
        now: Instant,
        idle_timeout: Duration,
    ) -> Option<LifecycleStopReason> {
        if !self.idle(now) {
            return None;
        }
        if now.duration_since(started_at) >= STALE_AFTER {
            return Some(LifecycleStopReason::Stale);
        }
        if now.duration_since(self.idle_since) >= idle_timeout {
            return Some(LifecycleStopReason::IdleExpired);
        }
        None
    }

    fn idle(&self, now: Instant) -> bool {
        let resuming = self
            .task_finished_at
            .is_some_and(|at| now.saturating_duration_since(at) < TASK_REPORT_GRACE);
        self.sender.is_none() && self.background.is_empty() && self.unclaimed.is_none() && !resuming
    }

    /// Hands any held continuation to the new turn first; returns whether the
    /// child has served a turn before.
    fn register(&mut self, sender: mpsc::UnboundedSender<TurnEvent>) -> bool {
        for line in self.unclaimed.take().into_iter().flatten() {
            let _ = sender.send(TurnEvent::Line(line));
        }
        self.recording = false;
        self.sender = Some(sender);
        self.served
    }

    fn release(&mut self, sender: &mpsc::UnboundedSender<TurnEvent>, now: Instant) -> bool {
        if !self
            .sender
            .as_ref()
            .is_some_and(|current| current.same_channel(sender))
        {
            return false;
        }
        self.sender = None;
        self.idle_since = now;
        self.served = true;
        true
    }

    fn track_background(&mut self, event: &Value, now: Instant) {
        if event.get("type").and_then(Value::as_str) != Some("system") {
            return;
        }
        let task_id = || {
            event
                .get("task_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        match event.get("subtype").and_then(Value::as_str) {
            Some("task_started") => self.background.extend(task_id()),
            Some("task_notification") => {
                if let Some(id) = task_id() {
                    self.background.remove(&id);
                }
                self.task_finished_at = Some(now);
            }
            Some("background_tasks_changed") => {
                if let Some(tasks) = event.get("tasks").and_then(Value::as_array) {
                    self.background = tasks
                        .iter()
                        .filter_map(|task| task.get("task_id").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect();
                }
            }
            _ => {}
        }
    }

    /// Whether a line no turn is listening for starts a segment the child
    /// began on its own; if so it is held for a turn to record.
    fn starts_continuation(&mut self, event: &Value) -> bool {
        if !self.served || !starts_segment(event) {
            return false;
        }
        self.unclaimed = Some(vec![event.clone()]);
        true
    }
}

/// What the resident child was spawned with. Any field that can only be applied
/// at launch (permission mode, effort, the `orx mcp` bridge) forces a respawn
/// when it changes; `model` is the exception — it retunes live via `set_model`,
/// so it is updated in place on a successful control request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnConfig {
    pub permission_mode: Option<PermissionMode>,
    pub effort: Option<String>,
    pub model: Option<String>,
    pub native_store: NativeStore,
    pub profile: Option<String>,
    pub alphaxiv_signed_in: bool,
    /// Whether the child has its input bridge; failed setup is retried on the next turn.
    pub bridge_active: bool,
    /// The executable detection selected, so a newer install respawns the child.
    pub bin: Option<PathBuf>,
}

/// Everything needed to spawn (or respawn) a session's child, built per turn by
/// the harness. `resume` carries the native session id for `--resume` recovery
/// (crash/restart or config-change respawn); `None` on a session's first spawn.
/// `chat` is the spawn-time handle for the plan-mode bridge (`up_port` +
/// `mint_gate_token`) — the spec is transient (built per turn, consumed by
/// `ensure`), so the strong ref cannot cycle.
#[derive(Clone)]
pub struct SpawnSpec {
    pub chat: Arc<crate::local::chat::ChatHost>,
    pub session_id: String,
    pub repo: PathBuf,
    pub playbook: PathBuf,
    pub resume: Option<String>,
    /// Message within `resume` to branch from: the child starts a new native
    /// session holding `resume`'s history up to it.
    pub fork_at: Option<String>,
    pub config: SpawnConfig,
}

/// A segment a child started on its own, routed to the turn that records it.
pub type ClaudeContinuation = (TurnRoute, mpsc::UnboundedReceiver<TurnEvent>);

/// One event delivered to the session's in-flight turn.
#[derive(Debug)]
pub enum TurnEvent {
    /// A stream-json output line (an `assistant`/`user`/`system`/`result`
    /// object). The turn loop folds these through `apply_event`.
    Line(Value),
    /// Child died (EOF on stdout). The turn cannot continue.
    Closed,
}

/// One inbound stdout line, classified. Claude interleaves two shapes on the
/// same stream: `control_response` (settling one of our control requests) and
/// ordinary stream-json output objects (turn events).
#[derive(Debug, PartialEq)]
pub enum ClaudeLine {
    /// A `control_response` for a request we sent: `request_id` echoed back,
    /// plus `Ok`/`Err` from the response `subtype`.
    ControlResponse {
        request_id: String,
        result: Result<Value, String>,
    },
    /// A stream-json output object (turn event) — routed to the live turn.
    Event(Value),
    /// Unparseable, or a control message we don't originate (`control_request`
    /// from the child). Ignored.
    Junk,
}

/// Classify one stdout line. Pure — the reader task and the tests share it.
pub fn classify_line(line: &str) -> ClaudeLine {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return ClaudeLine::Junk;
    };
    match msg.get("type").and_then(Value::as_str) {
        Some("control_response") => {
            let response = msg.get("response").unwrap_or(&Value::Null);
            let Some(request_id) = response
                .get("request_id")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return ClaudeLine::Junk;
            };
            let result = match response.get("subtype").and_then(Value::as_str) {
                Some("success") => Ok(response.get("response").cloned().unwrap_or(Value::Null)),
                _ => Err(response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("claude control request failed")
                    .to_string()),
            };
            ClaudeLine::ControlResponse { request_id, result }
        }
        // A control_request FROM the child (e.g. a permission prompt) rides the
        // `orx mcp` bridge over HTTP, not this stream — we never answer it here.
        Some("control_request") | None => ClaudeLine::Junk,
        // Everything else is a stream-json output object: assistant/user/system/
        // result. The turn loop discriminates on `type`.
        Some(_) => ClaudeLine::Event(msg),
    }
}

/// A live connection to one session's resident `claude` child.
pub struct ClaudeClient {
    chat: Weak<crate::local::chat::ChatHost>,
    session_id: String,
    child: Mutex<Child>,
    #[cfg(unix)]
    process_group_id: u32,
    /// Held open for the life of the child: stdin EOF ends the session, so we
    /// never drop it between turns. One JSON line per user message / control
    /// request.
    stdin: Mutex<ChildStdin>,
    next_id: AtomicI64,
    /// Our outstanding control requests. Sync mutex: touched from the reader
    /// task and from `control_request`, held only for map ops, never across an
    /// await.
    pending: std::sync::Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    /// The session's in-flight turn, if any (one per session-child). The
    /// registration guard drops synchronously on task abort, hence sync mutex.
    turn: std::sync::Mutex<TurnSlot>,
    /// What this child was spawned with — the reuse/respawn decision reads it,
    /// and a successful `set_model` updates its `model` in place.
    config: std::sync::Mutex<SpawnConfig>,
    auth_generation: u64,
    started_at: Instant,
    terminated: AtomicBool,
}

impl ClaudeClient {
    /// The config this child is currently running under.
    pub fn config(&self) -> SpawnConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn auth_generation(&self) -> u64 {
        self.auth_generation
    }

    /// Send a user message as one stream-json line. The child answers with a
    /// stream of output objects ending in a `result`.
    pub async fn send_user_message(&self, text: &str) -> Result<()> {
        let msg = json!({
            "type": "user",
            "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        });
        self.write_line(&msg).await
    }

    /// Send a control request and await its `control_response`. `Ok(Err(msg))`
    /// is a *child-reported* failure, `Err(..)` a transport failure (child gone
    /// / timeout).
    async fn control_request(&self, request: Value) -> Result<Result<Value, String>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request_id = format!("req_{id}");
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(request_id.clone(), tx);
        let sent = self
            .write_line(&json!({
                "type": "control_request",
                "request_id": request_id,
                "request": request,
            }))
            .await;
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&request_id);
            return Err(e);
        }
        match tokio::time::timeout(CONTROL_TIMEOUT, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(anyhow!("claude closed during control request")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&request_id);
                Err(anyhow!(
                    "claude did not answer the control request within {}s",
                    CONTROL_TIMEOUT.as_secs()
                ))
            }
        }
    }

    /// Run Claude's own `/compact` on the resident child and wait for the turn
    /// it opens to finish. The child keeps its session id — the point of using
    /// the native command rather than reseeding a new session.
    pub(crate) async fn compact(
        self: &Arc<Self>,
        mut rx: mpsc::UnboundedReceiver<TurnEvent>,
        timeout: Duration,
    ) -> Result<()> {
        self.send_user_message("/compact").await?;
        let settle = async {
            let mut saw_user_echo = false;
            let mut saw_compacting = false;
            let mut compact_outcome: Option<Result<(), String>> = None;
            while let Some(event) = rx.recv().await {
                match event {
                    TurnEvent::Line(line) => {
                        let system = line.get("type").and_then(Value::as_str) == Some("system");
                        let subtype = line.get("subtype").and_then(Value::as_str);
                        // Native /compact emits status events instead of a user-message echo.
                        if system
                            && subtype == Some("status")
                            && line.get("status").and_then(Value::as_str) == Some("compacting")
                        {
                            saw_compacting = true;
                        }
                        if !saw_compacting
                            && !crate::local::harness::claude::belongs_to_current_turn(
                                &line,
                                "/compact",
                                &mut saw_user_echo,
                            )
                        {
                            continue;
                        }
                        if system && subtype == Some("status") {
                            match line.get("compact_result").and_then(Value::as_str) {
                                Some("success") => compact_outcome = Some(Ok(())),
                                Some("failed") => {
                                    compact_outcome = Some(Err(line
                                        .get("compact_error")
                                        .and_then(Value::as_str)
                                        .unwrap_or("compaction failed")
                                        .to_string()))
                                }
                                _ => {}
                            }
                        } else if system && subtype == Some("compact_boundary") {
                            compact_outcome.get_or_insert(Ok(()));
                        }
                        if line.get("type").and_then(Value::as_str) != Some("result") {
                            continue;
                        }
                        // Fail closed, and let `is_error` win over the subtype
                        // as the turn path does: a shape we don't recognize
                        // must not read as a compaction that happened.
                        let subtype = line.get("subtype").and_then(Value::as_str);
                        let failed = line
                            .get("is_error")
                            .and_then(Value::as_bool)
                            .unwrap_or(subtype != Some("success"));
                        return if failed {
                            Err(anyhow!(
                                "claude /compact failed: {}",
                                subtype.unwrap_or("no result status")
                            ))
                        } else {
                            // Local commands can return result.success even when compact_result failed.
                            compact_outcome
                                .unwrap_or_else(|| {
                                    if saw_user_echo {
                                        Ok(())
                                    } else {
                                        Err("Claude did not confirm compaction".to_string())
                                    }
                                })
                                .map_err(|error| anyhow!("claude /compact failed: {error}"))
                        };
                    }
                    TurnEvent::Closed => return Err(anyhow!("claude closed during /compact")),
                }
            }
            Err(anyhow!("claude stopped reporting during /compact"))
        };
        tokio::time::timeout(timeout, settle).await.map_err(|_| {
            anyhow!(
                "claude did not finish /compact within {}s",
                timeout.as_secs()
            )
        })?
    }

    /// Retune the resident child's model in place via `set_model`. On success,
    /// record the new model in `config` so the reuse decision stays truthful.
    async fn set_model(&self, model: &str) -> Result<()> {
        match self
            .control_request(json!({ "subtype": "set_model", "model": model }))
            .await?
        {
            Ok(_) => {
                self.config.lock().unwrap().model = Some(model.to_string());
                Ok(())
            }
            Err(msg) => Err(anyhow!("claude set_model failed: {msg}")),
        }
    }

    async fn write_line(&self, msg: &Value) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .map_err(|e| anyhow!("claude stdin: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| anyhow!("claude stdin: {e}"))?;
        Ok(())
    }

    /// Register the session's turn event sink. The returned guard deregisters on
    /// drop (including task abort mid-turn).
    fn register_turn(
        self: &Arc<Self>,
        tx: mpsc::UnboundedSender<TurnEvent>,
        reaper_notify: Arc<Notify>,
    ) -> TurnRoute {
        let served = self.turn.lock().unwrap().register(tx.clone());
        // The reader may have already sent its last `Closed` to no one.
        if self.terminated.load(Ordering::Acquire) {
            let _ = tx.send(TurnEvent::Closed);
        }
        TurnRoute {
            client: self.clone(),
            tx,
            reaper_notify,
            served,
        }
    }

    fn stop_reason(&self, now: Instant, idle_timeout: Duration) -> Option<LifecycleStopReason> {
        self.turn
            .lock()
            .unwrap()
            .stop_reason(self.started_at, now, idle_timeout)
    }

    fn age_and_idle(&self, now: Instant) -> (Duration, Option<Duration>) {
        let turn = self.turn.lock().unwrap();
        (
            now.duration_since(self.started_at),
            turn.sender
                .is_none()
                .then(|| now.duration_since(turn.idle_since)),
        )
    }
}

/// RAII turn registration — dropping (normal exit or task abort) detaches the
/// event sink so a dangling turn can't receive another turn's events. An
/// aborted task's guard drops *asynchronously*, possibly after a successor turn
/// has already registered — so drop only detaches its *own* channel, never a
/// successor's (the codex `same_channel` guard).
pub struct TurnRoute {
    client: Arc<ClaudeClient>,
    tx: mpsc::UnboundedSender<TurnEvent>,
    reaper_notify: Arc<Notify>,
    served: bool,
}

impl TurnRoute {
    pub fn client(&self) -> &Arc<ClaudeClient> {
        &self.client
    }

    /// The child ended a turn before this one, so a segment it writes before
    /// the prompt's echo is one it started on its own, not startup replay.
    pub fn served(&self) -> bool {
        self.served
    }

    /// Detaches the turn from the child unless the reader has already routed
    /// it another event, which is returned instead. Atomic with the reader, so
    /// whatever the child writes next reaches this turn or is held for another.
    pub fn end(&self, rx: &mut mpsc::UnboundedReceiver<TurnEvent>) -> Option<TurnEvent> {
        let mut turn = self.client.turn.lock().unwrap();
        if let Ok(event) = rx.try_recv() {
            return Some(event);
        }
        if turn.release(&self.tx, Instant::now()) {
            drop(turn);
            self.reaper_notify.notify_one();
        }
        None
    }
}

impl Drop for TurnRoute {
    fn drop(&mut self) {
        let mut turn = self.client.turn.lock().unwrap();
        if turn.release(&self.tx, Instant::now()) {
            drop(turn);
            self.reaper_notify.notify_one();
        }
    }
}

/// The `system/init` that opens each segment the CLI writes, prompted or not.
pub(crate) fn starts_segment(event: &Value) -> bool {
    event.get("type").and_then(Value::as_str) == Some("system")
        && event.get("subtype").and_then(Value::as_str) == Some("init")
}

fn reported_permission_mode(event: &Value) -> Option<PermissionMode> {
    if event.get("type").and_then(Value::as_str) != Some("system") {
        return None;
    }
    match event.get("subtype").and_then(Value::as_str) {
        Some("init" | "status") => match event.get("permissionMode").and_then(Value::as_str)? {
            "default" | "manual" => Some(PermissionMode::Ask),
            "auto" => Some(PermissionMode::Auto),
            "acceptEdits" => Some(PermissionMode::AcceptEdits),
            "plan" => Some(PermissionMode::Plan),
            "bypassPermissions" => Some(PermissionMode::Bypass),
            _ => None,
        },
        _ => None,
    }
}

/// Reader task: pump child stdout, route each line. On EOF every pending control
/// request fails and the live turn (if any) gets [`TurnEvent::Closed`].
async fn read_loop(client: Arc<ClaudeClient>, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match classify_line(&line) {
            ClaudeLine::ControlResponse { request_id, result } => {
                if let Some(tx) = client.pending.lock().unwrap().remove(&request_id) {
                    let _ = tx.send(result);
                }
            }
            ClaudeLine::Event(value) => {
                if let Some(mode) = reported_permission_mode(&value) {
                    let previous = {
                        let mut config = client.config.lock().unwrap();
                        let previous = config.permission_mode.unwrap_or(PermissionMode::Auto);
                        config.permission_mode = Some(mode);
                        previous
                    };
                    if previous != mode && !client.terminated.load(Ordering::Acquire) {
                        if let Some(chat) = client.chat.upgrade() {
                            if let Err(error) = chat
                                .sync_claude_permission_mode(&client.session_id, previous, mode)
                                .await
                            {
                                eprintln!(
                                    "orx up: could not synchronize Claude permission mode: {error}"
                                );
                            }
                        }
                    }
                }
                // Route to the live turn. With none listening, a segment the
                // child starts itself is held for a turn to record; any other
                // line (an aborted turn's tail) is dropped once its tool
                // identities are recorded.
                let mut turn = client.turn.lock().unwrap();
                turn.track_background(&value, Instant::now());
                if let Some(tx) = turn.sender.as_ref() {
                    let _ = tx.send(TurnEvent::Line(value));
                } else if let Some(lines) = turn.unclaimed.as_mut() {
                    lines.push(value);
                } else if turn.starts_continuation(&value) {
                    drop(turn);
                    if let Some(chat) = client.chat.upgrade() {
                        let session_id = client.session_id.clone();
                        tokio::spawn(async move { chat.record_continuation(&session_id).await });
                    }
                } else if value.get("type").and_then(Value::as_str) == Some("assistant") {
                    drop(turn);
                    if let Some(message) = value.get("message").cloned() {
                        let session_id = client.session_id.clone();
                        // Off the reader so a busy store never stalls Claude's stdout.
                        tokio::task::spawn_blocking(move || {
                            crate::local::chat::record_native_invocations(
                                "claude-code",
                                &session_id,
                                &message,
                            )
                        });
                    }
                }
            }
            ClaudeLine::Junk => {}
        }
    }
    // EOF or read error: the connection is unusable. Kill the child if it is
    // somehow still alive (a half-dead child left in the registry would fail
    // every turn until restart), then fail everything still waiting.
    terminate_client(&client).await;
    for (_, tx) in client.pending.lock().unwrap().drain() {
        let _ = tx.send(Err("claude exited".into()));
    }
    if let Some(tx) = client.turn.lock().unwrap().sender.as_ref() {
        let _ = tx.send(TurnEvent::Closed);
    }
}

pub(crate) async fn terminate_client(client: &ClaudeClient) {
    if client.terminated.swap(true, Ordering::AcqRel) {
        return;
    }
    #[cfg(unix)]
    if let Ok(process_group_id) = i32::try_from(client.process_group_id) {
        // SAFETY: spawn assigns the child its own process group before this id is captured.
        unsafe {
            libc::kill(-process_group_id, libc::SIGKILL);
        }
    }
    let mut child = client.child.lock().await;
    let _ = child.start_kill();
    if tokio::time::timeout(TERMINATION_TIMEOUT, child.wait())
        .await
        .is_err()
    {
        eprintln!(
            "orx up: timed out reaping Claude worker session={}",
            client.session_id
        );
    }
}

async fn stop_client(client: Arc<ClaudeClient>, reason: &str) {
    stop_client_at(client, reason, Instant::now()).await;
}

async fn stop_client_at(client: Arc<ClaudeClient>, reason: &str, now: Instant) {
    let (age, idle) = client.age_and_idle(now);
    eprintln!(
        "orx up: stopping Claude worker session={} reason={} age_s={} idle_s={}",
        client.session_id,
        reason,
        age.as_secs(),
        idle.map(|duration| duration.as_secs().to_string())
            .unwrap_or_else(|| "active".to_string())
    );
    terminate_client(&client).await;
}

/// Spawn a resident `claude` child for one session. Mirrors the flag block the
/// old per-turn `run_turn` built (`harness/claude.rs`), plus the persistent-mode
/// wiring: `--input-format stream-json` and, on recovery/respawn,
/// `--resume <native_id>`. No handshake — the first turn's `system`/`init` line
/// is the health signal.
async fn spawn_client(spec: &SpawnSpec, auth_generation: u64) -> Result<Arc<ClaudeClient>> {
    let bin = spec.config.bin.clone().ok_or_else(|| {
        anyhow!("claude not found on PATH — install Claude Code and run `claude` once to sign in")
    })?;
    let mut cmd = crate::process::tokio_command(&bin);
    cmd.args([
        "--print",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        // Stream text/thinking deltas (stream_event lines) instead of only
        // complete assistant messages — apply_event paints them token by token.
        "--include-partial-messages",
        // Echo each submitted user message back on stdout — the exact boundary
        // between `--resume`/startup replay output (which can carry a stale
        // `result` that would end the turn instantly) and the turn we sent;
        // the harness ignores events until the echo arrives.
        "--replay-user-messages",
        "--verbose",
        "--permission-mode",
        claude_permission_mode(spec.config.permission_mode),
    ])
    .arg("--append-system-prompt-file")
    .arg(&spec.playbook)
    .current_dir(&spec.repo)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::from(crate::local::chat::harness_log("claude")?))
    // Backstop only: the child is deliberately kept resident across turns, so
    // its lifetime is managed by the host reaper and explicit stop paths, not
    // by dropping a per-turn handle.
    .kill_on_drop(true);
    if let Some(model) = &spec.config.model {
        cmd.args(["--model", model]);
    }
    if let Some(effort) = spec.config.effort.as_deref() {
        cmd.args(["--effort", effort]);
    }
    if let Some(native_id) = &spec.resume {
        cmd.args(["--resume", native_id]);
        if let Some(message_id) = &spec.fork_at {
            cmd.args(["--resume-session-at", message_id, "--fork-session"]);
        }
    }

    let mut config = spec.config.clone();
    let settings = write_settings(
        &spec.repo,
        spec.config.permission_mode == Some(PermissionMode::Plan),
    )?;
    cmd.arg("--settings").arg(settings);
    // The gate token is minted only here, once per child: `request_permission`
    // checks it without expiry, so re-minting would strand the bridge's held requests.
    let gate = if uses_permission_bridge(spec.config.permission_mode) {
        spec.chat.up_port().map(|port| {
            let token = spec.chat.mint_gate_token(
                &spec.session_id,
                spec.config.permission_mode == Some(PermissionMode::Plan),
                spec.config.permission_mode == Some(PermissionMode::Bypass),
            );
            (port, token)
        })
    } else {
        None
    };
    // Record successful bridge setup so reconciliation retries failures next turn.
    config.bridge_active = false;
    let gate_config = gate.as_ref().map(|(port, token)| (*port, token.as_str()));
    match write_mcp_config(&spec.repo, &spec.session_id, gate_config) {
        Ok(path) => {
            cmd.arg("--mcp-config").arg(path);
            config.bridge_active = gate.is_some();
        }
        Err(e) => eprintln!(
            "orx up: MCP config not written, alphaXiv tools are unavailable and gray-area tools will be denied: {e}"
        ),
    }
    // A held approval gets an hour before the CLI abandons the tool call; orx denies at 55 min.
    cmd.env(
        "MCP_TOOL_TIMEOUT",
        if config.bridge_active {
            "3600000"
        } else {
            "300000"
        },
    );
    if config.bridge_active {
        cmd.args(["--permission-prompt-tool", "mcp__orx__approve"]);
        // Sub-agents go through orx's `delegate`, which can reach any model
        // and wakes this chat with the answer.
        cmd.args(["--disallowedTools", "Agent,Task,Workflow"]);
        cmd.args(["--allowedTools", "mcp__orx"]);
    }
    crate::local::chat::prepare_env(&mut cmd);
    cmd.env("CLAUDE_CODE_ENTRYPOINT", CLAUDE_ENTRYPOINT);
    let delegated = crate::store::Store::open()
        .and_then(|store| store.delegation(&spec.session_id))
        .is_ok_and(|delegation| delegation.parent.is_some());
    if let Some(ttl) = delegated
        .then(crate::local::chat::delegation::delegate_prompt_cache_ttl)
        .flatten()
    {
        cmd.env("CLAUDE_CODE_PROMPT_CACHE_TTL", ttl);
    }
    if let Some(profile) = spec.config.profile.as_deref() {
        crate::local::agent_profiles::home(profile, "claude-code")?;
        cmd.envs(crate::local::agent_profiles::login_env(profile)?);
    } else {
        let native_store = spec.config.native_store;
        let config_home = tokio::task::spawn_blocking(move || {
            crate::local::native_store::prepare_claude(native_store)
        })
        .await
        .map_err(|error| anyhow!("Claude config preparation failed: {error}"))??;
        cmd.env("CLAUDE_CONFIG_DIR", config_home);
        cmd.env(
            "CLAUDE_SECURESTORAGE_CONFIG_DIR",
            crate::local::native_store::claude_secure_storage_config_dir(),
        );
    }
    // Stamp the launching session so `orx exp run` (a fresh subprocess the
    // agent shells out) tags its run and can explicitly subscribe this chat.
    // After prepare_env so it wins.
    crate::local::chat::set_chat_session_env(
        &mut cmd,
        &spec.session_id,
        "claude-code",
        spec.chat.up_port(),
    )?;
    // Own process group: a terminal SIGINT reaches orx up alone, which then
    // tears the resident child down deliberately (kill_session / shutdown). A
    // shared group would let Ctrl-C kill a persistent child mid-turn.
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Could not spawn {}: {}", bin.display(), e))?;
    #[cfg(unix)]
    let process_group_id = child
        .id()
        .ok_or_else(|| anyhow!("claude: spawned child has no process id"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("claude: no stdout"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("claude: no stdin"))?;

    let started_at = Instant::now();
    let client = Arc::new(ClaudeClient {
        chat: Arc::downgrade(&spec.chat),
        session_id: spec.session_id.clone(),
        child: Mutex::new(child),
        #[cfg(unix)]
        process_group_id,
        stdin: Mutex::new(stdin),
        next_id: AtomicI64::new(1),
        pending: std::sync::Mutex::new(HashMap::new()),
        turn: std::sync::Mutex::new(TurnSlot::new(started_at)),
        config: std::sync::Mutex::new(config),
        auth_generation,
        started_at,
        terminated: AtomicBool::new(false),
    });
    tokio::spawn(read_loop(client.clone(), stdout));
    Ok(client)
}

/// How a live child reconciles with the turn's wanted config.
///
/// * `Reuse` — identical spawn config → hand back the running child (zero cost).
/// * `SetModel` — only the model changed, to a concrete value → retune in place.
/// * `Respawn` — mode/effort/bridge changed, or the model was cleared (a launch
///   flag can't be unset live) → kill and respawn with `--resume`.
#[derive(Debug, PartialEq, Eq)]
pub enum ChildAction {
    Reuse,
    SetModel(String),
    Respawn,
}

/// Pure reuse/respawn decision (unit-tested). `current` is the running child's
/// config, `wanted` the turn's.
pub fn child_action(current: &SpawnConfig, wanted: &SpawnConfig) -> ChildAction {
    // Any launch-only axis differing forces a respawn.
    if current.permission_mode != wanted.permission_mode
        || current.effort != wanted.effort
        || current.bridge_active != wanted.bridge_active
        || current.alphaxiv_signed_in != wanted.alphaxiv_signed_in
        || current.native_store != wanted.native_store
        || current.profile != wanted.profile
        || current.bin != wanted.bin
    {
        return ChildAction::Respawn;
    }
    if current.model == wanted.model {
        return ChildAction::Reuse;
    }
    // Model differs. A change TO a concrete model retunes live; clearing the
    // model (Some → None) can't be expressed as a flag toggle, so respawn.
    match &wanted.model {
        Some(model) => ChildAction::SetModel(model.clone()),
        None => ChildAction::Respawn,
    }
}

/// The `orx up` Claude host: one resident `claude` child per chat session, keyed
/// by the orx session id. Share as `Arc<ClaudeHost>` in axum state.
pub struct ClaudeHost {
    /// Reconciliation is serialized per chat session, never across unrelated
    /// concurrent agents.
    spawn_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    spawn_epochs: Mutex<HashMap<String, u64>>,
    inner: Mutex<HashMap<String, Arc<ClaudeClient>>>,
    auth: std::sync::RwLock<AuthSnapshot>,
    auth_recovery_lock: Mutex<()>,
    announced_generation: AtomicU64,
    next_spawn_epoch: AtomicU64,
    reaper_notify: Arc<Notify>,
    /// Session ids are UUIDs and never reused; tombstones block turns queued
    /// before deletion from registering a replacement worker afterward.
    forgotten_sessions: Mutex<HashSet<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthSnapshot {
    pub state: HarnessAuthState,
    pub generation: u64,
    pub runtime_rejected: bool,
    pub method: Option<&'static str>,
    pub provider: Option<String>,
    pub credential_conflict: bool,
    pub auth_check_failed: bool,
    sequence: u64,
    last_recovered_generation: Option<u64>,
}

impl Default for ClaudeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeHost {
    pub fn new() -> Self {
        Self {
            spawn_locks: Mutex::new(HashMap::new()),
            spawn_epochs: Mutex::new(HashMap::new()),
            inner: Mutex::new(HashMap::new()),
            auth: std::sync::RwLock::new(AuthSnapshot {
                state: HarnessAuthState::Unknown,
                generation: 0,
                runtime_rejected: false,
                method: None,
                provider: None,
                credential_conflict: false,
                auth_check_failed: false,
                sequence: 0,
                last_recovered_generation: None,
            }),
            auth_recovery_lock: Mutex::new(()),
            announced_generation: AtomicU64::new(0),
            next_spawn_epoch: AtomicU64::new(1),
            reaper_notify: Arc::new(Notify::new()),
            forgotten_sessions: Mutex::new(HashSet::new()),
        }
    }

    pub fn start_reaper(self: &Arc<Self>) {
        let host = Arc::downgrade(self);
        let notify = self.reaper_notify.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REAPER_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = notify.notified() => {}
                }
                let Some(host) = host.upgrade() else {
                    break;
                };
                host.reap_expired().await;
            }
        });
    }

    async fn reap_expired(&self) {
        self.reap_expired_at(Instant::now(), IdlePolicy::current())
            .await;
    }

    async fn reap_expired_at(&self, now: Instant, policy: IdlePolicy) {
        let clients = self
            .inner
            .lock()
            .await
            .iter()
            .map(|(session_id, client)| (session_id.clone(), client.clone()))
            .collect::<Vec<_>>();
        let mut doomed = Vec::new();
        let mut idle = Vec::new();
        for (session_id, client) in clients {
            if client.terminated.load(Ordering::Acquire) {
                continue;
            }
            let exited = !matches!(client.child.lock().await.try_wait(), Ok(None));
            let turn = client.turn.lock().unwrap();
            if turn.sender.is_some() || (!exited && !turn.idle(now)) {
                continue;
            }
            let idle_since = turn.idle_since;
            // An exited child may leave descendants in its group; release it, never keep it warm.
            let reason = if exited {
                Some(LifecycleStopReason::Exited)
            } else {
                turn.stop_reason(client.started_at, now, policy.idle_timeout)
            };
            drop(turn);
            match reason {
                Some(reason) => doomed.push((session_id, client, idle_since, reason)),
                None => idle.push((session_id, client, idle_since)),
            }
        }
        let idle_for = idle
            .iter()
            .map(|(_, _, idle_since)| now.duration_since(*idle_since))
            .collect::<Vec<_>>();
        for index in policy.over_limit(&idle_for) {
            let (session_id, client, idle_since) = idle[index].clone();
            doomed.push((
                session_id,
                client,
                idle_since,
                LifecycleStopReason::IdleLimit,
            ));
        }
        for (session_id, client, idle_since, reason) in doomed {
            let session_lock = {
                let mut locks = self.spawn_locks.lock().await;
                locks
                    .entry(session_id.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(())))
                    .clone()
            };
            let _spawning = session_lock.lock().await;
            let expired = {
                let mut clients = self.inner.lock().await;
                // A turn may have run since the snapshot; only an untouched idle child goes.
                let untouched = clients
                    .get(&session_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &client))
                    && {
                        let turn = client.turn.lock().unwrap();
                        let releasable = reason == LifecycleStopReason::Exited || turn.idle(now);
                        turn.sender.is_none() && releasable && turn.idle_since == idle_since
                    };
                if !untouched {
                    continue;
                }
                clients.remove(&session_id)
            };
            if let Some(client) = expired {
                stop_client_at(client, reason.as_str(), now).await;
            }
        }
    }

    pub fn auth_snapshot(&self) -> AuthSnapshot {
        self.auth.read().unwrap().clone()
    }

    pub fn claim_auth_announcement(&self, generation: u64) -> bool {
        let mut announced = self.announced_generation.load(Ordering::Acquire);
        while generation > announced {
            match self.announced_generation.compare_exchange_weak(
                announced,
                generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => announced = current,
            }
        }
        false
    }

    /// Publish one live status result; worker generation is not probe order.
    pub fn observe_auth_probe(&self, probe: &crate::local::harness::claude::AuthProbe) -> bool {
        let mut auth = self.auth.write().unwrap();
        if probe.sequence <= auth.sequence || auth.runtime_rejected {
            return false;
        }
        auth.sequence = probe.sequence;
        let before = auth.clone();
        if probe.failed_check {
            auth.auth_check_failed = true;
            if auth.state == HarnessAuthState::Unknown {
                auth.method = probe.method;
                auth.provider = probe.provider.clone();
            }
        } else {
            let rotate = auth.state != probe.state
                || (auth.state == HarnessAuthState::Ready
                    && probe.state == HarnessAuthState::Ready
                    && (auth.method != probe.method || auth.provider != probe.provider));
            auth.state = probe.state;
            auth.method = probe.method;
            auth.provider = probe.provider.clone();
            auth.credential_conflict = probe.credential_conflict;
            auth.auth_check_failed = false;
            if rotate {
                auth.generation += 1;
                auth.last_recovered_generation = None;
            }
        }
        auth.state != before.state
            || auth.method != before.method
            || auth.provider != before.provider
            || auth.credential_conflict != before.credential_conflict
            || auth.auth_check_failed != before.auth_check_failed
            || auth.generation != before.generation
    }

    fn auth_barrier(auth: &mut AuthSnapshot) {
        auth.sequence = crate::local::harness::claude::auth_barrier_sequence();
    }

    pub fn reserve_auth_check(&self) {
        let mut auth = self.auth.write().unwrap();
        Self::auth_barrier(&mut auth);
    }

    /// Explicit UI re-check after the user changed credentials. This clears a
    /// runtime rejection without initiating login; the following detector is
    /// still responsible for proving readiness.
    pub fn clear_runtime_rejection(&self) -> bool {
        let mut auth = self.auth.write().unwrap();
        if !auth.runtime_rejected {
            return false;
        }
        auth.runtime_rejected = false;
        auth.state = HarnessAuthState::Unknown;
        Self::auth_barrier(&mut auth);
        auth.auth_check_failed = false;
        auth.generation += 1;
        auth.last_recovered_generation = None;
        true
    }

    pub fn defer_auth_verification(&self, expected_generation: u64) -> bool {
        let mut auth = self.auth.write().unwrap();
        if auth.generation != expected_generation
            || auth.runtime_rejected
            || auth.state != HarnessAuthState::Ready
        {
            return false;
        }
        auth.state = HarnessAuthState::Unknown;
        Self::auth_barrier(&mut auth);
        auth.auth_check_failed = false;
        auth.generation += 1;
        auth.last_recovered_generation = None;
        true
    }

    /// Quarantine a failed worker and coordinate one live credential re-check
    /// across every Claude session. A Ready result advances the generation even
    /// when the prior snapshot was Ready: the failed child proved its credential
    /// snapshot stale, so sibling workers must recycle lazily before reuse.
    pub async fn recover_auth_failure(
        &self,
        session_id: &str,
        failed_generation: u64,
    ) -> HarnessAuthState {
        self.kill_session(session_id).await;
        let _recovering = self.auth_recovery_lock.lock().await;
        let current = self.auth_snapshot();
        if current.generation > failed_generation {
            return current.state;
        }
        if current.last_recovered_generation == Some(failed_generation) {
            return current.state;
        }
        if current.state == HarnessAuthState::NeedsLogin {
            return current.state;
        }
        let probe = crate::local::harness::claude::current_auth_state().await;
        let mut auth = self.auth.write().unwrap();
        if auth.generation != current.generation
            || auth.runtime_rejected
            || probe.sequence <= auth.sequence
        {
            return auth.state;
        }
        auth.last_recovered_generation = Some(failed_generation);
        Self::auth_barrier(&mut auth);
        auth.generation += 1;
        auth.state = probe.state;
        auth.method = probe.method;
        auth.provider = probe.provider;
        auth.credential_conflict = probe.credential_conflict;
        auth.auth_check_failed = probe.failed_check;
        auth.runtime_rejected = false;
        auth.state
    }

    /// A replacement from a freshly checked generation failed too. Runtime
    /// rejection is stronger evidence than `auth status` metadata, so keep the
    /// harness non-ready until the user changes credentials and explicitly
    /// re-checks it.
    pub async fn reject_auth_generation(
        &self,
        session_id: &str,
        failed_generation: u64,
    ) -> HarnessAuthState {
        self.kill_session(session_id).await;
        let _recovering = self.auth_recovery_lock.lock().await;
        let mut auth = self.auth.write().unwrap();
        if auth.generation > failed_generation {
            return auth.state;
        }
        auth.state = HarnessAuthState::NeedsLogin;
        auth.runtime_rejected = true;
        Self::auth_barrier(&mut auth);
        auth.auth_check_failed = false;
        auth.generation += 1;
        auth.last_recovered_generation = Some(failed_generation);
        auth.state
    }

    async fn resolve_unknown(&self) -> AuthSnapshot {
        let _recovering = self.auth_recovery_lock.lock().await;
        let current = self.auth_snapshot();
        if current.state != HarnessAuthState::Unknown || current.runtime_rejected {
            return current;
        }
        let probe = crate::local::harness::claude::current_auth_state().await;
        self.observe_auth_probe(&probe);
        self.auth_snapshot()
    }

    /// Acquire this session's resident child for one turn, reconciled to
    /// `spec`'s config. The turn route is registered while the per-session
    /// spawn lock is still held, so the lifecycle reaper cannot race the send.
    ///
    /// The spawn runs in a *detached* task: the calling turn task is abortable
    /// (interrupt / delete), and an aborted future would drop its `Arc` while
    /// the reader task keeps the child alive — an unregistered client would be
    /// unreachable by every kill path and leak the process for the rest of `orx
    /// up`'s life. Detached, the bring-up always runs to completion.
    pub async fn acquire_turn(
        self: &Arc<Self>,
        spec: SpawnSpec,
        tx: mpsc::UnboundedSender<TurnEvent>,
    ) -> Result<TurnRoute> {
        let session = spec.session_id.clone();
        let session_lock = {
            let mut locks = self.spawn_locks.lock().await;
            locks
                .entry(session.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _spawning = session_lock.lock().await;
        if self.forgotten_sessions.lock().await.contains(&session) {
            return Err(anyhow!("Claude session was deleted"));
        }
        let profile = spec.config.profile.is_some();
        if let Some(id) = spec.config.profile.as_deref() {
            crate::local::agent_profiles::prepare(id, "claude-code")?;
        }
        let mut auth = self.auth_snapshot();
        if !profile && auth.state == HarnessAuthState::Unknown && !auth.runtime_rejected {
            auth = self.resolve_unknown().await;
        }
        match if profile {
            HarnessAuthState::Ready
        } else {
            auth.state
        } {
            HarnessAuthState::NeedsLogin => {
                return Err(anyhow!(
                    "{}",
                    crate::local::harness::claude::auth_recovery_note(
                        auth.method,
                        auth.provider.as_deref(),
                    )
                ));
            }
            HarnessAuthState::Unsupported => {
                return Err(anyhow!("Claude Code 2.1.211 or newer is required"));
            }
            HarnessAuthState::Unknown => {
                return Err(anyhow!(
                    "Claude Code authentication could not be verified — run `claude auth status`, then re-check the agent"
                ));
            }
            HarnessAuthState::Ready => {}
        }
        let live = self.inner.lock().await.get(&session).cloned();
        if let Some(live) = live {
            if live.terminated.load(Ordering::Acquire)
                || !matches!(live.child.lock().await.try_wait(), Ok(None))
            {
                self.inner.lock().await.remove(&session);
                stop_client(live, "exited").await;
            } else if let Some(reason) =
                live.stop_reason(Instant::now(), IdlePolicy::current().idle_timeout)
            {
                self.inner.lock().await.remove(&session);
                stop_client(live, reason.as_str()).await;
            } else if !profile && live.auth_generation() != auth.generation {
                self.inner.lock().await.remove(&session);
                stop_client(live, "authentication-changed").await;
            } else {
                match child_action(&live.config(), &spec.config) {
                    ChildAction::Reuse => {
                        let latest = self.auth_snapshot();
                        if profile
                            || (latest.state == HarnessAuthState::Ready
                                && live.auth_generation() == latest.generation)
                        {
                            return Ok(live.register_turn(tx, self.reaper_notify.clone()));
                        }
                        self.inner.lock().await.remove(&session);
                        stop_client(live, "authentication-changed").await;
                    }
                    ChildAction::SetModel(model) => match live.set_model(&model).await {
                        Ok(()) => {
                            let latest = self.auth_snapshot();
                            if profile
                                || (latest.state == HarnessAuthState::Ready
                                    && live.auth_generation() == latest.generation)
                            {
                                return Ok(live.register_turn(tx, self.reaper_notify.clone()));
                            }
                            self.inner.lock().await.remove(&session);
                            stop_client(live, "authentication-changed").await;
                        }
                        Err(e) => {
                            eprintln!("orx up: claude set_model failed, respawning: {e}");
                            self.inner.lock().await.remove(&session);
                            stop_client(live, "set-model-failed").await;
                        }
                    },
                    ChildAction::Respawn => {
                        self.inner.lock().await.remove(&session);
                        stop_client(live, "configuration-changed").await;
                    }
                }
            }
        }
        auth = self.auth_snapshot();
        if !profile && auth.state != HarnessAuthState::Ready {
            return Err(anyhow!(
                "Claude Code authentication changed while preparing this session"
            ));
        }
        let spawn_epoch = self.next_spawn_epoch.fetch_add(1, Ordering::AcqRel);
        self.spawn_epochs
            .lock()
            .await
            .insert(session.clone(), spawn_epoch);
        let host = self.clone();
        let client = tokio::spawn(async move {
            let client = spawn_client(&spec, auth.generation).await?;
            {
                let mut guard = host.inner.lock().await;
                let latest = host.auth_snapshot();
                let latest_epoch = host
                    .spawn_epochs
                    .lock()
                    .await
                    .get(&session)
                    .copied()
                    .unwrap_or_default();
                if latest_epoch != spawn_epoch
                    || (!profile
                        && (latest.state != HarnessAuthState::Ready
                            || latest.generation != client.auth_generation()))
                {
                    drop(guard);
                    stop_client(client, "startup-superseded").await;
                    return Err(anyhow!("Claude Code authentication changed during startup"));
                }
                if let Some(existing) = guard.get(&session) {
                    if matches!(existing.child.lock().await.try_wait(), Ok(None)) {
                        let stale = existing.clone();
                        guard.remove(&session);
                        stop_client(stale, "duplicate-worker").await;
                    }
                }
                guard.insert(session.clone(), client.clone());
            }
            Ok(client)
        })
        .await
        .map_err(|e| anyhow!("claude bring-up task failed: {e}"))??;
        Ok(client.register_turn(tx, self.reaper_notify.clone()))
    }

    /// Whether the session's child holds a segment it started on its own that
    /// no turn is starting to record.
    pub async fn has_continuation(&self, session_id: &str) -> bool {
        let live = self.inner.lock().await.get(session_id).cloned();
        live.is_some_and(|client| {
            let turn = client.turn.lock().unwrap();
            turn.unclaimed.is_some() && !turn.recording
        })
    }

    /// Marks the held segment as being recorded by a turn that will call
    /// [`Self::take_continuation`]; false when there is none to record.
    pub async fn claim_continuation(&self, session_id: &str) -> bool {
        let Some(live) = self.inner.lock().await.get(session_id).cloned() else {
            return false;
        };
        let mut turn = live.turn.lock().unwrap();
        let claimable = turn.sender.is_none() && turn.unclaimed.is_some() && !turn.recording;
        turn.recording |= claimable;
        claimable
    }

    /// Route the held segment, and what the child writes after it, to a new
    /// turn. The live child is taken as it is: respawning it would lose the segment.
    pub async fn take_continuation(&self, session_id: &str) -> Option<ClaudeContinuation> {
        let live = self.inner.lock().await.get(session_id).cloned()?;
        let held = {
            let turn = live.turn.lock().unwrap();
            turn.sender.is_none() && turn.unclaimed.is_some()
        };
        if !held {
            return None;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        Some((live.register_turn(tx, self.reaper_notify.clone()), rx))
    }

    /// Kill and reap one session's child. The load-bearing interrupt/delete
    /// path: a resident child survives task-abort/`kill_on_drop`, so it must be
    /// killed explicitly; the next turn respawns with `--resume`.
    pub async fn kill_session(&self, session_id: &str) {
        self.kill_session_for(session_id, "session-stop").await;
    }

    /// Stops the session's child so its next turn resumes whatever the store
    /// now names. Refused while the child still owes a reply: stopping it would
    /// end its background tasks and lose what it writes about them.
    pub async fn stop_idle(&self, session_id: &str) -> Result<()> {
        let live = self.inner.lock().await.get(session_id).cloned();
        let owes_reply = live.is_some_and(|client| {
            !client.terminated.load(Ordering::Acquire)
                && !client.turn.lock().unwrap().idle(Instant::now())
        });
        if owes_reply {
            return Err(anyhow!(
                "Claude is still finishing a background task. Wait for its reply, or ask Claude to stop the task."
            ));
        }
        self.kill_session_for(session_id, "fork-switched").await;
        Ok(())
    }

    async fn kill_session_for(&self, session_id: &str, reason: &str) {
        let epoch = self.next_spawn_epoch.fetch_add(1, Ordering::AcqRel);
        self.spawn_epochs
            .lock()
            .await
            .insert(session_id.to_string(), epoch);
        if let Some(client) = self.inner.lock().await.remove(session_id) {
            stop_client(client, reason).await;
        }
    }

    /// Resume or reuse the session's child before invoking its native compaction.
    pub(crate) async fn compact_session(
        self: &Arc<Self>,
        spec: SpawnSpec,
        timeout: Duration,
    ) -> Result<()> {
        let session_id = spec.session_id.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let result = tokio::time::timeout(timeout, async {
            let route = self.acquire_turn(spec, tx).await?;
            route.client().compact(rx, timeout).await
        })
        .await
        .map_err(|_| anyhow!("Claude Code compaction exceeded {}s", timeout.as_secs()))
        .and_then(|result| result);
        if let Err(error) = result {
            // The child is still mid-`/compact`; its trailing `result` would
            // otherwise land in whatever turn registers next.
            self.kill_session(&session_id).await;
            return Err(error);
        }
        Ok(())
    }

    /// Permanently remove one deleted session's child and reconciliation state.
    pub async fn forget_session(&self, session_id: &str) {
        let session_lock = {
            let mut locks = self.spawn_locks.lock().await;
            locks
                .entry(session_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _spawning = session_lock.lock().await;
        self.forgotten_sessions
            .lock()
            .await
            .insert(session_id.to_string());
        self.kill_session_for(session_id, "session-deleted").await;
        drop(_spawning);
        self.spawn_epochs.lock().await.remove(session_id);
        self.spawn_locks.lock().await.remove(session_id);
    }

    /// Kill and reap every child (also happens via kill_on_drop on exit).
    pub async fn shutdown(&self) {
        let clients = self
            .inner
            .lock()
            .await
            .drain()
            .map(|(_, client)| client)
            .collect::<Vec<_>>();
        for client in clients {
            stop_client(client, "shutdown").await;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    const IDLE_TIMEOUT: Duration = IdlePolicy::DEFAULT.idle_timeout;

    #[cfg(unix)]
    async fn test_client(
        session_id: &str,
        started_at: Instant,
        idle_since: Instant,
        shell: &str,
    ) -> Arc<ClaudeClient> {
        chat_test_client(session_id, started_at, idle_since, shell, Weak::new())
    }

    #[cfg(unix)]
    fn chat_test_client(
        session_id: &str,
        started_at: Instant,
        idle_since: Instant,
        shell: &str,
        chat: Weak<crate::local::chat::ChatHost>,
    ) -> Arc<ClaudeClient> {
        let mut command = crate::process::tokio_command("sh");
        command
            .args(["-c", shell])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let process_group_id = child.id().unwrap();
        let stdin = child.stdin.take().unwrap();
        Arc::new(ClaudeClient {
            chat,
            session_id: session_id.to_string(),
            child: Mutex::new(child),
            process_group_id,
            stdin: Mutex::new(stdin),
            next_id: AtomicI64::new(1),
            pending: std::sync::Mutex::new(HashMap::new()),
            turn: std::sync::Mutex::new(TurnSlot::new(idle_since)),
            config: std::sync::Mutex::new(cfg(None, None, None, false)),
            auth_generation: 0,
            started_at,
            terminated: AtomicBool::new(false),
        })
    }

    #[cfg(unix)]
    async fn install_test_client(host: &ClaudeHost, client: Arc<ClaudeClient>) {
        host.spawn_locks
            .lock()
            .await
            .insert(client.session_id.clone(), Arc::new(Mutex::new(())));
        host.inner
            .lock()
            .await
            .insert(client.session_id.clone(), client);
    }

    /// A fake CLI, read by the real read loop, that writes back each line it is
    /// sent and answers a user message with `REPLY`. A prompt containing
    /// `RESUMES_FIRST` finds it resuming on its own with `TASK_DONE` before the echo.
    #[cfg(unix)]
    pub(crate) async fn echo_worker(session_id: &str) -> (Arc<ClaudeHost>, Arc<ClaudeClient>) {
        let host = ready_host();
        let client = chat_echo_worker(&host, session_id, Weak::new()).await;
        (host, client)
    }

    #[cfg(unix)]
    pub(crate) fn ready_host() -> Arc<ClaudeHost> {
        let host = Arc::new(ClaudeHost::new());
        host.auth.write().unwrap().state = HarnessAuthState::Ready;
        host
    }

    /// [`echo_worker`] on `host`, recording what the child starts on its own in `chat`.
    #[cfg(unix)]
    pub(crate) async fn chat_echo_worker(
        host: &ClaudeHost,
        session_id: &str,
        chat: Weak<crate::local::chat::ChatHost>,
    ) -> Arc<ClaudeClient> {
        let now = Instant::now();
        let shell = r#"while IFS= read -r line; do
            case "$line" in *RESUMES_FIRST*)
                printf '%s\n' '{"type":"system","subtype":"init"}' '{"type":"assistant","message":{"id":"resumed","content":[{"type":"text","text":"TASK_DONE"}]}}' '{"type":"result","subtype":"success"}' ;;
            esac
            printf '%s\n' "$line"
            case "$line" in *'"type":"user"'*)
                printf '%s\n' '{"type":"assistant","message":{"id":"reply","content":[{"type":"text","text":"REPLY"}]}}' '{"type":"result","subtype":"success"}' ;;
            esac
        done"#;
        let client = chat_test_client(session_id, now, now, shell, chat);
        let stdout = client.child.lock().await.stdout.take().unwrap();
        tokio::spawn(read_loop(client.clone(), stdout));
        install_test_client(host, client.clone()).await;
        client
    }

    /// Runs one prompted turn on the worker to its result.
    #[cfg(unix)]
    pub(crate) async fn serve_turn(host: &ClaudeHost, client: &Arc<ClaudeClient>) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let route = client.register_turn(tx, host.reaper_notify.clone());
        assert!(!route.served());
        client.send_user_message("first").await.unwrap();
        while let Some(TurnEvent::Line(line)) = rx.recv().await {
            if line["type"] == "result" {
                break;
            }
        }
    }

    /// Makes the fake CLI write `lines`, and returns once the read loop has handled them.
    #[cfg(unix)]
    pub(crate) async fn emit(client: &ClaudeClient, lines: &[Value]) {
        let request_id = format!("sync-{}", client.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        client
            .pending
            .lock()
            .unwrap()
            .insert(request_id.clone(), tx);
        let sync = json!({"type": "control_response",
            "response": {"subtype": "success", "request_id": request_id}});
        let mut input = String::new();
        for line in lines.iter().chain([&sync]) {
            input.push_str(&line.to_string());
            input.push('\n');
        }
        client
            .stdin
            .lock()
            .await
            .write_all(input.as_bytes())
            .await
            .unwrap();
        rx.await.unwrap().unwrap();
    }

    fn cfg(
        mode: Option<PermissionMode>,
        effort: Option<&str>,
        model: Option<&str>,
        bridge: bool,
    ) -> SpawnConfig {
        SpawnConfig {
            permission_mode: mode,
            effort: effort.map(str::to_string),
            model: model.map(str::to_string),
            native_store: NativeStore::Isolated,
            profile: None,
            alphaxiv_signed_in: false,
            bridge_active: bridge,
            bin: None,
        }
    }

    #[test]
    fn changing_profile_always_respawns_the_child() {
        let current = cfg(None, None, Some("sonnet"), false);
        let mut wanted = current.clone();
        wanted.profile = Some("claude-012345abcd".into());
        assert_eq!(child_action(&current, &wanted), ChildAction::Respawn);
        let mut second = wanted.clone();
        second.profile = Some("claude-abcdefghij".into());
        assert_eq!(child_action(&wanted, &second), ChildAction::Respawn);
    }

    #[test]
    fn a_newly_selected_claude_respawns_the_child() {
        let mut current = cfg(None, None, Some("sonnet"), false);
        current.bin = Some(PathBuf::from("/user/claude"));
        let mut wanted = current.clone();
        assert_eq!(child_action(&current, &wanted), ChildAction::Reuse);
        wanted.bin = Some(PathBuf::from("/managed/claude"));
        assert_eq!(child_action(&current, &wanted), ChildAction::Respawn);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compaction_waits_for_its_echo_before_accepting_a_result() {
        for failed in [false, true] {
            let now = Instant::now();
            let client = test_client("compact", now, now, "sleep 600").await;
            let (tx, rx) = mpsc::unbounded_channel();
            for line in [
                json!({"type": "result", "subtype": "success"}),
                json!({"type": "user", "message": {"content": "previous message"}}),
                json!({"type": "result", "subtype": "error_during_execution", "is_error": true}),
                json!({"type": "user", "message": {"content": "/compact"}}),
                json!({"type": "result", "subtype": "success", "is_error": failed}),
            ] {
                tx.send(TurnEvent::Line(line)).unwrap();
            }
            let result = client.compact(rx, Duration::from_secs(1)).await;
            terminate_client(&client).await;
            assert_eq!(result.is_err(), failed);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compaction_uses_native_status_outcome_without_a_user_echo() {
        for outcome in ["success", "failed"] {
            let now = Instant::now();
            let client = test_client("compact", now, now, "sleep 600").await;
            let (tx, rx) = mpsc::unbounded_channel();
            for line in [
                json!({"type": "system", "subtype": "compact_boundary"}),
                json!({"type": "result", "subtype": "success", "is_error": false}),
                json!({"type": "system", "subtype": "status", "status": "compacting"}),
                json!({"type": "system", "subtype": "status", "status": null,
                    "compact_result": outcome, "compact_error": "Not enough messages to compact."}),
                json!({"type": "system", "subtype": "init"}),
                json!({"type": "result", "subtype": "success", "is_error": false, "local_command": "compact"}),
            ] {
                tx.send(TurnEvent::Line(line)).unwrap();
            }
            let result = client.compact(rx, Duration::from_secs(1)).await;
            terminate_client(&client).await;
            if outcome == "failed" {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "claude /compact failed: Not enough messages to compact."
                );
            } else {
                assert!(result.is_ok());
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compaction_requires_confirmation_after_its_live_status() {
        for boundary in [false, true] {
            let now = Instant::now();
            let client = test_client("compact", now, now, "sleep 600").await;
            let (tx, rx) = mpsc::unbounded_channel();
            tx.send(TurnEvent::Line(
                json!({"type": "system", "subtype": "status", "status": "compacting"}),
            ))
            .unwrap();
            if boundary {
                tx.send(TurnEvent::Line(
                    json!({"type": "system", "subtype": "compact_boundary"}),
                ))
                .unwrap();
            }
            tx.send(TurnEvent::Line(
                json!({"type": "result", "subtype": "success", "is_error": false}),
            ))
            .unwrap();
            let result = client.compact(rx, Duration::from_secs(1)).await;
            terminate_client(&client).await;
            assert_eq!(result.is_ok(), boundary);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compaction_does_not_succeed_from_replayed_results_without_its_echo() {
        let now = Instant::now();
        let client = test_client("compact", now, now, "sleep 600").await;
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(TurnEvent::Line(
            json!({"type": "result", "subtype": "success"}),
        ))
        .unwrap();
        let result = client.compact(rx, Duration::from_millis(10)).await;
        terminate_client(&client).await;
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("did not finish /compact"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compaction_reports_a_closed_child_after_its_echo() {
        let now = Instant::now();
        let client = test_client("compact", now, now, "sleep 600").await;
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(TurnEvent::Line(
            json!({"type": "user", "message": {"content": [{"type": "text", "text": "/compact"}]}}),
        ))
        .unwrap();
        tx.send(TurnEvent::Closed).unwrap();
        let result = client.compact(rx, Duration::from_secs(1)).await;
        terminate_client(&client).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "claude closed during /compact"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn compaction_timeout_reaps_the_acquired_worker_and_releases_its_route() {
        let now = Instant::now();
        let host = Arc::new(ClaudeHost::new());
        host.auth.write().unwrap().state = HarnessAuthState::Ready;
        let client = test_client("compact", now, now, "sleep 600").await;
        install_test_client(&host, client.clone()).await;
        let chat = Arc::new(crate::local::chat::ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            host.clone(),
        ));
        let result = host
            .compact_session(
                SpawnSpec {
                    chat,
                    session_id: "compact".into(),
                    repo: PathBuf::new(),
                    playbook: PathBuf::new(),
                    resume: Some("saved-native-id".into()),
                    fork_at: None,
                    config: client.config(),
                },
                Duration::from_millis(10),
            )
            .await;
        assert!(result.is_err());
        assert!(client.terminated.load(Ordering::Acquire));
        assert!(!host.inner.lock().await.contains_key("compact"));
        assert!(client.turn.lock().unwrap().sender.is_none());
    }

    #[test]
    fn native_permission_status_reports_fallback_and_ignores_unrelated_events() {
        assert_eq!(
            reported_permission_mode(
                &json!({"type":"system","subtype":"status","permissionMode":"default"})
            ),
            Some(PermissionMode::Ask)
        );
        assert_eq!(
            reported_permission_mode(
                &json!({"type":"system","subtype":"init","permissionMode":"auto"})
            ),
            Some(PermissionMode::Auto)
        );
        assert_eq!(
            reported_permission_mode(
                &json!({"type":"system","subtype":"status","status":"compacting"})
            ),
            None
        );
        assert_eq!(
            reported_permission_mode(&json!({"type":"assistant","permissionMode":"default"})),
            None
        );
        assert_eq!(
            reported_permission_mode(
                &json!({"type":"system","subtype":"status","permissionMode":"unknown"})
            ),
            None
        );
    }

    #[test]
    fn child_action_matrix() {
        // Identical config → reuse.
        assert_eq!(
            child_action(
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
            ),
            ChildAction::Reuse
        );
        let mut legacy = cfg(None, None, None, false);
        legacy.native_store = NativeStore::Legacy;
        assert_eq!(
            child_action(&cfg(None, None, None, false), &legacy),
            ChildAction::Respawn
        );
        // Model-only change to a concrete value → set_model.
        assert_eq!(
            child_action(
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("sonnet"),
                    false
                ),
            ),
            ChildAction::SetModel("sonnet".into())
        );
        // Clearing the model (Some → None) can't be a live flag toggle → respawn.
        assert_eq!(
            child_action(
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
                &cfg(Some(PermissionMode::Auto), Some("high"), None, false),
            ),
            ChildAction::Respawn
        );
        // Setting a model from None is a concrete target → set_model.
        assert_eq!(
            child_action(
                &cfg(Some(PermissionMode::Auto), Some("high"), None, false),
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
            ),
            ChildAction::SetModel("opus".into())
        );
        // Permission-mode change → respawn (launch-only).
        assert_eq!(
            child_action(
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
                &cfg(
                    Some(PermissionMode::Plan),
                    Some("high"),
                    Some("opus"),
                    false
                ),
            ),
            ChildAction::Respawn
        );
        // Effort change → respawn (launch-only).
        assert_eq!(
            child_action(
                &cfg(
                    Some(PermissionMode::Auto),
                    Some("high"),
                    Some("opus"),
                    false
                ),
                &cfg(Some(PermissionMode::Auto), Some("max"), Some("opus"), false),
            ),
            ChildAction::Respawn
        );
        // Bridge toggling on (a plan turn that got the bridge where the child
        // didn't have it) → respawn.
        assert_eq!(
            child_action(
                &cfg(Some(PermissionMode::Plan), None, Some("opus"), false),
                &cfg(Some(PermissionMode::Plan), None, Some("opus"), true),
            ),
            ChildAction::Respawn
        );
        // Mode-and-model both differing still respawns (launch axis wins over
        // the model-only set_model shortcut).
        assert_eq!(
            child_action(
                &cfg(Some(PermissionMode::Auto), None, Some("opus"), false),
                &cfg(Some(PermissionMode::Plan), None, Some("sonnet"), false),
            ),
            ChildAction::Respawn
        );
    }

    #[test]
    fn alphaxiv_sign_in_changes_respawn_the_resident_child() {
        let signed_out = cfg(Some(PermissionMode::Ask), None, None, true);
        let mut signed_in = signed_out.clone();
        signed_in.alphaxiv_signed_in = true;
        assert_eq!(child_action(&signed_out, &signed_in), ChildAction::Respawn);
        assert_eq!(child_action(&signed_in, &signed_out), ChildAction::Respawn);
    }

    #[test]
    fn lifecycle_reaps_idle_and_stale_workers_at_the_boundaries() {
        let now = Instant::now();
        let recent_start = now - Duration::from_secs(20 * 60);
        let mut slot = TurnSlot::new(now - IDLE_TIMEOUT + Duration::from_secs(1));

        assert_eq!(slot.stop_reason(recent_start, now, IDLE_TIMEOUT), None);

        slot.idle_since = now - IDLE_TIMEOUT;
        assert_eq!(
            slot.stop_reason(recent_start, now, IDLE_TIMEOUT),
            Some(LifecycleStopReason::IdleExpired)
        );

        slot.idle_since = now;
        assert_eq!(
            slot.stop_reason(now - STALE_AFTER, now, IDLE_TIMEOUT),
            Some(LifecycleStopReason::Stale)
        );
    }

    #[test]
    fn lifecycle_never_reaps_an_active_turn_and_release_starts_idle_clock() {
        let now = Instant::now();
        let started_at = now - STALE_AFTER - Duration::from_secs(1);
        let mut slot = TurnSlot::new(started_at);
        let (tx, _rx) = mpsc::unbounded_channel();
        slot.register(tx.clone());

        assert_eq!(slot.stop_reason(started_at, now, IDLE_TIMEOUT), None);

        let released_at = now + Duration::from_secs(5);
        assert!(slot.release(&tx, released_at));
        assert_eq!(slot.idle_since, released_at);
        assert_eq!(
            slot.stop_reason(started_at, released_at, IDLE_TIMEOUT),
            Some(LifecycleStopReason::Stale)
        );
    }

    #[test]
    fn stale_route_drop_cannot_clear_a_successor_turn() {
        let now = Instant::now();
        let mut slot = TurnSlot::new(now);
        let (old_tx, _old_rx) = mpsc::unbounded_channel();
        let (new_tx, _new_rx) = mpsc::unbounded_channel();
        slot.register(old_tx.clone());
        slot.register(new_tx);

        assert!(!slot.release(&old_tx, now + Duration::from_secs(1)));
        assert!(slot.sender.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn output_the_child_starts_after_a_turn_waits_for_the_next_turn() {
        let (host, client) = echo_worker("resumes").await;
        let init = json!({"type": "system", "subtype": "init"});
        let reply = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "TASK_DONE"}]}});
        let result = json!({"type": "result", "subtype": "success"});

        emit(&client, std::slice::from_ref(&init)).await;
        assert!(!host.has_continuation("resumes").await);

        serve_turn(&host, &client).await;
        emit(&client, &[init.clone(), reply.clone(), result.clone()]).await;
        assert!(host.has_continuation("resumes").await);
        host.reap_expired_at(Instant::now() + STALE_AFTER, IdlePolicy::DEFAULT)
            .await;
        assert!(host.inner.lock().await.contains_key("resumes"));

        assert!(host.claim_continuation("resumes").await);
        assert!(!host.has_continuation("resumes").await);
        assert!(!host.claim_continuation("resumes").await);
        let (route, mut rx) = host.take_continuation("resumes").await.unwrap();
        assert!(route.served());
        for expected in [init, reply, result] {
            assert!(matches!(rx.recv().await, Some(TurnEvent::Line(line)) if line == expected));
        }
        assert!(host.take_continuation("resumes").await.is_none());
        drop(route);
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_held_segment_from_an_exited_child_ends_its_turn() {
        let (host, client) = echo_worker("exits").await;
        serve_turn(&host, &client).await;
        let init = json!({"type": "system", "subtype": "init"});
        emit(&client, std::slice::from_ref(&init)).await;
        terminate_client(&client).await;

        let (_route, mut rx) = host.take_continuation("exits").await.unwrap();
        assert!(matches!(rx.recv().await, Some(TurnEvent::Line(line)) if line == init));
        assert!(matches!(rx.recv().await, Some(TurnEvent::Closed)));
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_ending_turn_takes_what_was_routed_to_it_and_later_output_is_held() {
        let (host, client) = echo_worker("ending").await;
        serve_turn(&host, &client).await;
        let (tx, mut rx) = mpsc::unbounded_channel();
        let route = client.register_turn(tx, host.reaper_notify.clone());
        let init = json!({"type": "system", "subtype": "init"});

        emit(&client, std::slice::from_ref(&init)).await;
        assert!(matches!(route.end(&mut rx), Some(TurnEvent::Line(line)) if line == init));
        assert!(route.end(&mut rx).is_none());

        emit(&client, std::slice::from_ref(&init)).await;
        assert!(host.has_continuation("ending").await);
        drop(route);
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_keeps_a_worker_whose_task_just_finished_until_it_reports() {
        let host = ready_host();
        let reporting = chat_echo_worker(&host, "reporting", Weak::new()).await;
        let newer = chat_echo_worker(&host, "newer", Weak::new()).await;
        serve_turn(&host, &reporting).await;
        serve_turn(&host, &newer).await;
        let finished = json!({"type": "system", "subtype": "task_notification", "task_id": "t"});
        emit(&reporting, &[finished]).await;

        let evictable = Instant::now() + crate::local::agent_lifecycle::MIN_IDLE_BEFORE_LIMIT;
        host.reap_expired_at(evictable, IdlePolicy::LOW_MEMORY)
            .await;
        assert!(host.inner.lock().await.contains_key("reporting"));
        host.reap_expired_at(Instant::now() + TASK_REPORT_GRACE, IdlePolicy::LOW_MEMORY)
            .await;
        assert!(!host.inner.lock().await.contains_key("reporting"));
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_keeps_a_worker_until_its_background_tasks_finish() {
        let (host, client) = echo_worker("background").await;
        let started =
            |id: &str| json!({"type": "system", "subtype": "task_started", "task_id": id});
        emit(&client, &[started("shell"), started("agent")]).await;

        let later = Instant::now() + STALE_AFTER;
        for policy in [IdlePolicy::DEFAULT, IdlePolicy::LOW_MEMORY] {
            host.reap_expired_at(later, policy).await;
            assert!(host.inner.lock().await.contains_key("background"));
        }

        emit(
            &client,
            &[
                json!({"type": "system", "subtype": "background_tasks_changed",
                "tasks": [{"task_id": "agent"}]}),
            ],
        )
        .await;
        host.reap_expired_at(later, IdlePolicy::DEFAULT).await;
        assert!(host.inner.lock().await.contains_key("background"));

        emit(
            &client,
            &[json!({"type": "system", "subtype": "task_notification",
                "task_id": "agent", "status": "completed"})],
        )
        .await;
        host.reap_expired_at(later, IdlePolicy::DEFAULT).await;
        assert!(!host.inner.lock().await.contains_key("background"));
        assert!(client.terminated.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_keeps_active_stale_worker_then_reaps_it_after_release() {
        let now = Instant::now();
        let host = ClaudeHost::new();
        let client = test_client(
            "active-stale",
            now - STALE_AFTER - Duration::from_secs(1),
            now,
            "sleep 600",
        )
        .await;
        install_test_client(&host, client.clone()).await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let route = client.register_turn(tx, host.reaper_notify.clone());

        host.reap_expired_at(now, IdlePolicy::DEFAULT).await;
        assert!(host.inner.lock().await.contains_key("active-stale"));

        drop(route);
        host.reap_expired_at(
            Instant::now() + Duration::from_secs(60),
            IdlePolicy::DEFAULT,
        )
        .await;
        assert!(!host.inner.lock().await.contains_key("active-stale"));
        assert!(client.terminated.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_honors_idle_timeout_and_stale_age() {
        let now = Instant::now();
        let host = ClaudeHost::new();
        let idle = test_client(
            "idle",
            now - Duration::from_secs(20 * 60),
            now - IDLE_TIMEOUT,
            "sleep 600",
        )
        .await;
        let stale = test_client("stale", now - STALE_AFTER, now, "sleep 600").await;
        install_test_client(&host, idle.clone()).await;
        install_test_client(&host, stale.clone()).await;

        host.reap_expired_at(now, IdlePolicy::DEFAULT).await;

        let clients = host.inner.lock().await;
        assert!(!clients.contains_key("idle"));
        assert!(!clients.contains_key("stale"));
        drop(clients);
        assert!(idle.terminated.load(Ordering::Acquire));
        assert!(stale.terminated.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_keeps_only_the_most_recent_idle_worker_on_low_memory() {
        let now = Instant::now();
        let host = ClaudeHost::new();
        let recent = test_client("recent", now, now - Duration::from_secs(40), "sleep 600").await;
        let older = test_client("older", now, now - Duration::from_secs(60), "sleep 600").await;
        let active = test_client("active", now, now - Duration::from_secs(90), "sleep 600").await;
        install_test_client(&host, recent.clone()).await;
        install_test_client(&host, older.clone()).await;
        install_test_client(&host, active.clone()).await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let _route = active.register_turn(tx, host.reaper_notify.clone());

        host.reap_expired_at(now, IdlePolicy::LOW_MEMORY).await;

        let clients = host.inner.lock().await;
        assert!(clients.contains_key("recent"));
        assert!(clients.contains_key("active"));
        assert!(!clients.contains_key("older"));
        drop(clients);
        assert!(older.terminated.load(Ordering::Acquire));
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exited_worker_is_released_instead_of_kept_warm() {
        let now = Instant::now();
        let host = ClaudeHost::new();
        let warm = test_client("warm", now, now - Duration::from_secs(60), "sleep 600").await;
        let exited = test_client("exited", now, now - Duration::from_secs(40), "exit 0").await;
        exited.child.lock().await.wait().await.unwrap();
        install_test_client(&host, warm.clone()).await;
        install_test_client(&host, exited.clone()).await;

        host.reap_expired_at(now, IdlePolicy::LOW_MEMORY).await;

        let clients = host.inner.lock().await;
        assert!(clients.contains_key("warm"));
        assert!(!clients.contains_key("exited"));
        drop(clients);
        assert!(!warm.terminated.load(Ordering::Acquire));
        assert!(exited.terminated.load(Ordering::Acquire));
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turn_registration_wins_a_race_with_the_reaper() {
        let now = Instant::now();
        let host = Arc::new(ClaudeHost::new());
        let client = test_client(
            "racing",
            now - Duration::from_secs(20 * 60),
            now - IDLE_TIMEOUT,
            "sleep 600",
        )
        .await;
        install_test_client(&host, client.clone()).await;
        let session_lock = host.spawn_locks.lock().await.get("racing").unwrap().clone();
        let acquiring = session_lock.lock().await;
        let (started_tx, started_rx) = oneshot::channel();
        let reaping = {
            let host = host.clone();
            tokio::spawn(async move {
                let _ = started_tx.send(());
                host.reap_expired_at(now, IdlePolicy::DEFAULT).await;
            })
        };
        started_rx.await.unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();
        let route = client.register_turn(tx, host.reaper_notify.clone());
        drop(acquiring);
        reaping.await.unwrap();

        assert!(host.inner.lock().await.contains_key("racing"));
        drop(route);
        host.shutdown().await;
    }

    #[tokio::test]
    async fn deleted_session_tombstone_rejects_future_turns() {
        let host = Arc::new(ClaudeHost::new());
        let chat = Arc::new(crate::local::chat::ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            host.clone(),
        ));
        host.forget_session("deleted").await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let result = host
            .acquire_turn(
                SpawnSpec {
                    chat,
                    session_id: "deleted".to_string(),
                    repo: PathBuf::new(),
                    playbook: PathBuf::new(),
                    resume: None,
                    fork_at: None,
                    config: cfg(None, None, None, false),
                },
                tx,
            )
            .await;

        assert_eq!(
            result.err().unwrap().to_string(),
            "Claude session was deleted"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn termination_kills_the_worker_process_group() {
        let now = Instant::now();
        let client = test_client("tree", now, now, "sleep 600 & echo $!; wait").await;
        let descendant_pid = {
            let mut child = client.child.lock().await;
            let stdout = child.stdout.take().unwrap();
            drop(child);
            let mut line = String::new();
            BufReader::new(stdout).read_line(&mut line).await.unwrap();
            line.trim().parse::<u32>().unwrap()
        };

        terminate_client(&client).await;

        let mut live = true;
        for _ in 0..50 {
            live = unsafe { libc::kill(descendant_pid as i32, 0) == 0 };
            if !live {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !live,
            "descendant process {descendant_pid} survived group kill"
        );
    }

    #[test]
    fn classify_discriminates_control_responses_and_events() {
        // Success control_response carries the request_id and inner response.
        assert_eq!(
            classify_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_1","response":{"ok":true}}}"#
            ),
            ClaudeLine::ControlResponse {
                request_id: "req_1".into(),
                result: Ok(json!({"ok": true})),
            }
        );
        // Success with no inner response → Ok(Null).
        assert_eq!(
            classify_line(
                r#"{"type":"control_response","response":{"subtype":"success","request_id":"req_2"}}"#
            ),
            ClaudeLine::ControlResponse {
                request_id: "req_2".into(),
                result: Ok(Value::Null),
            }
        );
        // Error control_response surfaces the message.
        assert_eq!(
            classify_line(
                r#"{"type":"control_response","response":{"subtype":"error","request_id":"req_3","error":"nope"}}"#
            ),
            ClaudeLine::ControlResponse {
                request_id: "req_3".into(),
                result: Err("nope".into()),
            }
        );
        // A control_response without a request_id is junk, not a panic.
        assert_eq!(
            classify_line(r#"{"type":"control_response","response":{"subtype":"success"}}"#),
            ClaudeLine::Junk
        );
        // Ordinary stream-json output objects are Events.
        assert!(matches!(
            classify_line(r#"{"type":"system","subtype":"init","session_id":"abc"}"#),
            ClaudeLine::Event(_)
        ));
        assert!(matches!(
            classify_line(r#"{"type":"assistant","message":{"id":"m1","content":[]}}"#),
            ClaudeLine::Event(_)
        ));
        assert!(matches!(
            classify_line(r#"{"type":"result","subtype":"success","session_id":"abc"}"#),
            ClaudeLine::Event(_)
        ));
        // A control_request FROM the child (bridge rides HTTP, not this stream).
        assert_eq!(
            classify_line(r#"{"type":"control_request","request_id":"x","request":{}}"#),
            ClaudeLine::Junk
        );
        // Junk never panics.
        assert_eq!(classify_line("not json"), ClaudeLine::Junk);
        assert_eq!(classify_line("{}"), ClaudeLine::Junk);
    }

    #[test]
    fn auth_observations_order_without_recycling_on_transient_failure() {
        let host = ClaudeHost::new();
        assert_eq!(host.auth_snapshot().generation, 0);
        let first = crate::local::harness::claude::parse_auth_status(
            Some(1),
            br#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#,
            crate::local::harness::claude::auth_barrier_sequence(),
        );
        assert!(host.observe_auth_probe(&first));
        assert_eq!(host.auth_snapshot().generation, 1);
        assert!(!host.observe_auth_probe(&first));
        assert_eq!(host.auth_snapshot().generation, 1);
        let ready = crate::local::harness::claude::parse_auth_status(
            Some(0),
            br#"{"loggedIn":true,"authMethod":"third_party","apiProvider":"bedrock"}"#,
            crate::local::harness::claude::auth_barrier_sequence(),
        );
        assert!(host.observe_auth_probe(&ready));
        assert_eq!(host.auth_snapshot().generation, 2);
        let unknown = crate::local::harness::claude::parse_auth_status(
            Some(2),
            b"not json",
            crate::local::harness::claude::auth_barrier_sequence(),
        );
        assert!(host.observe_auth_probe(&unknown));
        assert_eq!(host.auth_snapshot().state, HarnessAuthState::Ready);
        assert!(host.auth_snapshot().auth_check_failed);
        assert_eq!(host.auth_snapshot().generation, 2);
        host.reserve_auth_check();
        assert_eq!(host.auth_snapshot().generation, 2);
        assert!(!host.observe_auth_probe(&unknown));
    }

    #[test]
    fn newer_provider_wins_in_both_completion_orders() {
        for newer_first in [false, true] {
            let host = ClaudeHost::new();
            let older = crate::local::harness::claude::parse_auth_status(
                Some(0),
                br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}"#,
                crate::local::harness::claude::auth_barrier_sequence(),
            );
            let newer = crate::local::harness::claude::parse_auth_status(
                Some(0),
                br#"{"loggedIn":true,"authMethod":"third_party","apiProvider":"bedrock"}"#,
                crate::local::harness::claude::auth_barrier_sequence(),
            );
            if newer_first {
                assert!(host.observe_auth_probe(&newer));
                assert!(!host.observe_auth_probe(&older));
                assert_eq!(host.auth_snapshot().generation, 1);
            } else {
                assert!(host.observe_auth_probe(&older));
                assert!(host.observe_auth_probe(&newer));
                assert_eq!(host.auth_snapshot().generation, 2);
            }
            let snapshot = host.auth_snapshot();
            assert_eq!(snapshot.method, Some("thirdParty"));
            assert_eq!(snapshot.provider.as_deref(), Some("bedrock"));
            assert!(!host.observe_auth_probe(&newer));
        }
    }

    #[test]
    fn ready_credential_route_change_recycles_workers() {
        let host = ClaudeHost::new();
        for status in [
            br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty"}"#.as_slice(),
            br#"{"loggedIn":true,"authMethod":"api-key","apiProvider":"firstParty"}"#.as_slice(),
            br#"{"loggedIn":true,"authMethod":"api-key"}"#.as_slice(),
        ] {
            let probe = crate::local::harness::claude::parse_auth_status(
                Some(0),
                status,
                crate::local::harness::claude::auth_barrier_sequence(),
            );
            assert!(host.observe_auth_probe(&probe));
        }
        assert_eq!(host.auth_snapshot().generation, 3);
    }
}
