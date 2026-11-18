//! Codex app-server host — one long-lived `codex app-server` child per chat
//! session, speaking newline-delimited JSON-RPC 2.0 over stdio (the protocol
//! the Codex TUI/IDE use; `jsonrpc` field omitted on the wire, requests flow
//! in BOTH directions — the server sends us approval requests we must answer
//! by id). Mirrors `AgentHost` (opencode.rs): spawn on demand, reap on read,
//! release when idle (`agent_lifecycle`), kill on session delete / shutdown.
//!
//! Wire shapes were pinned against codex-cli 0.144.0 via
//! `codex app-server generate-json-schema` plus a live spike (see the fixture
//! transcript in `harness/codex.rs` tests): notifications and server requests
//! carry camelCase params; approval replies are `{"decision": "accept" |
//! "acceptForSession" | "decline" | "cancel"}` (wrapped, not bare); thread ids
//! are plain UUIDs persisted as rollout files under the selected `CODEX_HOME`,
//! so `thread/resume {threadId}` survives an `orx up` restart.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::keep::{self, KeptHandle, KeptStdin, KeptStdout};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::error::{anyhow, Result};
use crate::local::agent_lifecycle::{IdlePolicy, REAPER_INTERVAL};
use crate::local::harness::codex::{find_codex, find_codex_required};
use crate::local::harness::SetupFailure;
use crate::local::native_store::{self, NativeStore};

/// Ceiling on a request's response wait — generous because `thread/start`
/// blocks on the user's own MCP servers coming up.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(150);
const HISTORY_READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Interrupts are best-effort and sit on the user-facing interrupt/delete
/// paths — never let a wedged child hold those hostage.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);
/// A healthy app-server answers `initialize` immediately.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// `ensure` reaps under the host locks — a wedged child must not stall every bring-up.
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(5);

/// One inbound line, classified. JSON-RPC over one stream: a message with both
/// `id` and `method` is a server→client *request* (approvals — must be
/// answered); `id` alone is a response to one of our requests; `method` alone
/// is a notification.
#[derive(Debug, PartialEq)]
pub enum Line {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Response {
        id: i64,
        result: std::result::Result<Value, JsonRpcError>,
    },
    Notification {
        method: String,
        params: Value,
    },
    Junk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (JSON-RPC {})", self.message, self.code)
    }
}

impl std::error::Error for JsonRpcError {}

/// Classify one wire line. Pure — the reader task and the tests share it.
pub fn classify_line(line: &str) -> Line {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        return Line::Junk;
    };
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = msg.get("id");
    match (id, method) {
        (Some(id), Some(method)) => Line::Request {
            // Echoed back verbatim in the reply — never assume integer.
            id: id.clone(),
            method,
            params: msg.get("params").cloned().unwrap_or(Value::Null),
        },
        (Some(id), None) => {
            let Some(id) = id.as_i64() else {
                return Line::Junk; // we only ever send integer ids
            };
            let result = match msg.get("error") {
                Some(err) => Err(JsonRpcError {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(-32000),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("codex app-server error")
                        .to_string(),
                    data: err.get("data").cloned(),
                }),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            Line::Response { id, result }
        }
        (None, Some(method)) => Line::Notification {
            method,
            params: msg.get("params").cloned().unwrap_or(Value::Null),
        },
        (None, None) => Line::Junk,
    }
}

/// How a server→client request is answered — the reply schema differs per kind,
/// and a request settled without a turn (raced abort, interrupt) must get the
/// shape *its* method can parse or codex stays blocked on us.
///
/// * `Approval` — `{"decision": accept|acceptForSession|decline|cancel}` (the two
///   sandbox-escalation requests).
/// * `McpApproval` — `{"action": accept|decline|cancel, "content": ...}`.
/// * `UserInput` — `{"answers": {<qid>: {"answers": [...]}}}`; `{}` is tolerated.
/// * `Other` — a reply schema we don't speak (e.g.
///   `item/permissions/requestApproval`, whose reply is a permission-profile
///   object) → a JSON-RPC method-not-found error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerReqKind {
    Approval,
    McpApproval,
    UserInput,
    Other,
}

/// Classify a server→client request by its method — the single source of truth
/// for both the reply shape (settle paths below) and whether the turn loop
/// surfaces it as an approval card vs a question card.
pub fn server_req_kind(method: &str) -> ServerReqKind {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
            ServerReqKind::Approval
        }
        "mcpServer/elicitation/request" => ServerReqKind::McpApproval,
        "item/tool/requestUserInput" => ServerReqKind::UserInput,
        _ => ServerReqKind::Other,
    }
}

/// One event delivered to the session's in-flight turn.
#[derive(Debug)]
pub enum TurnEvent {
    StartResult(std::result::Result<Value, JsonRpcError>),
    At {
        offset: u64,
        event: Box<TurnEvent>,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A server→client request (approval). The turn loop decides the reply
    /// (surface a card / auto-answer) and sends it via [`CodexClient::respond`].
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// Child died (EOF on stdout). The turn cannot continue.
    Closed,
}

/// A live JSON-RPC connection to one session's `codex app-server` child.
pub struct CodexClient {
    child: KeptHandle,
    terminated: AtomicBool,
    /// Closes only to stop the child: stdin EOF ends the app-server.
    stdin: Mutex<Option<KeptStdin>>,
    durable: std::sync::Mutex<ClientState>,
    reader_ready: tokio::sync::Notify,
    next_id: AtomicI64,
    /// Our outstanding requests. Sync mutex: touched from the reader task and
    /// from `request()`, held only for map ops, never across an await.
    pending:
        std::sync::Mutex<HashMap<i64, oneshot::Sender<std::result::Result<Value, JsonRpcError>>>>,
    /// The session's in-flight turn, if any (one per session-child). The
    /// registration guard drops synchronously on task abort, hence sync mutex.
    turn: std::sync::Mutex<Option<mpsc::UnboundedSender<TurnEvent>>>,
    /// Server→client requests we have not answered yet: raw JSON id text (ids
    /// are echoed verbatim) → the kind that dictates the settle shape. Guards
    /// `respond` against stale answers and lets every settle path pick the
    /// reply schema `id`'s method can actually parse.
    unanswered: std::sync::Mutex<HashMap<String, ServerReqKind>>,
    /// The in-flight turn's id (from the `turn/start` response), for
    /// `turn/interrupt`.
    active_turn: std::sync::Mutex<Option<String>>,
    /// The thread this child has started/resumed — a fresh child (after crash
    /// or restart) must `thread/resume` before its next `turn/start`.
    resumed_thread: std::sync::Mutex<Option<String>>,
    /// The effective model codex reported for this child's thread (top-level
    /// `model` in the `thread/start` / `thread/resume` response) — the required
    /// `settings.model` when attaching a `collaborationMode` mask, and the
    /// escape path when the session carries no explicit model.
    thread_model: std::sync::Mutex<Option<String>>,
    /// The collaboration-mode mask this child last sent on a `turn/start`, in
    /// memory only (belt-and-braces alongside the DB `prev_permission_mode`
    /// signal): a `plan` mask leaves the thread sticky-planned, so a later
    /// non-plan turn must attach `default` to un-stick it. `None` on a fresh
    /// child (crash/restart replacement) — the DB signal covers that case.
    last_collab_mode: std::sync::Mutex<Option<&'static str>>,
    native_store: NativeStore,
    pub(crate) profile: Option<String>,
    alphaxiv_signed_in: bool,
    /// The executable this child runs; once detection picks another, the next turn respawns it.
    bin: PathBuf,
    session_id: String,
    /// Each native (thread, turn)'s model, outliving any one orx turn: sub-agents do.
    turn_models: std::sync::Mutex<HashMap<(String, String), String>>,
    /// Last time this child was handed out, reported thread work, or ended a
    /// turn — the idle reaper's clock.
    last_active: std::sync::Mutex<Instant>,
}

#[derive(Clone, Serialize, Deserialize)]
struct OutboundRequest {
    method: String,
    response_at: Option<u64>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct ClientState {
    legacy: bool,
    profile: Option<String>,
    bin: PathBuf,
    alphaxiv_signed_in: bool,
    initialize: Option<Value>,
    offset: u64,
    models: Vec<((String, String), String)>,
    collab_mode: Option<String>,
    next_id: i64,
    outbound: HashMap<i64, OutboundRequest>,
    inbound: HashMap<String, (String, Value)>,
    answered: std::collections::HashSet<String>,
    outbox: HashMap<String, Value>,
    thread: Option<String>,
    model: Option<String>,
    active_turn: Option<String>,
}

impl CodexClient {
    /// Send a client→server request; `Ok(Err(msg))` is a *server-reported*
    /// JSON-RPC error, `Err(..)` a transport failure (child gone / timeout).
    /// Callers that must tell "codex said no" apart from "codex didn't answer"
    /// (e.g. the thread/resume fallback) use this; everyone else wants
    /// [`Self::request`].
    pub async fn try_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<std::result::Result<Value, JsonRpcError>> {
        self.try_request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    /// [`Self::try_request`] with a caller-chosen deadline.
    pub async fn try_request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<std::result::Result<Value, JsonRpcError>> {
        let id = self
            .next_id
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map_err(|_| anyhow!("Codex request id space is exhausted"))?;
        {
            let mut state = self.durable.lock().unwrap();
            state.next_id = state.next_id.max(id + 1);
            state.outbound.insert(
                id,
                OutboundRequest {
                    method: method.into(),
                    response_at: None,
                },
            );
            if method == "turn/start" {
                state.active_turn = None;
                *self.active_turn.lock().unwrap() = None;
            }
            self.save_owner(&state)?;
        }
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let _pending = PendingRequest { client: self, id };
        self.write_line(&json!({ "id": id, "method": method, "params": params }))
            .await?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(anyhow!("codex app-server closed during {method}")),
            Err(_) => Err(anyhow!(
                "codex app-server did not answer {method} within {}s",
                timeout.as_secs()
            )),
        }
    }

    /// [`Self::try_request`] with server errors collapsed into `Err`.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        match self.try_request(method, params).await? {
            Ok(result) => Ok(result),
            Err(err) => Err(anyhow!("codex {method} failed: {err}")),
        }
    }

    /// Answer a server→client request (approval or userInput). Errors if `id`
    /// isn't pending — the stale-answer guard (child restarted, turn ended).
    pub async fn respond(&self, id: &Value, result: Value) -> Result<()> {
        if self
            .unanswered
            .lock()
            .unwrap()
            .remove(&id.to_string())
            .is_none()
        {
            return Err(anyhow!("this request is no longer pending"));
        }
        self.write_line(&json!({ "id": id, "result": result }))
            .await
    }

    pub fn approval_reply(&self, id: &Value, approve: bool) -> Value {
        if self.unanswered.lock().unwrap().get(&id.to_string()) == Some(&ServerReqKind::McpApproval)
        {
            json!({"action":if approve {"accept"} else {"decline"}, "content":if approve {json!({})} else {Value::Null}, "_meta":null})
        } else {
            json!({"decision":if approve {"accept"} else {"decline"}})
        }
    }

    pub async fn respond_decline(&self, id: &Value) -> Result<()> {
        self.respond(id, self.approval_reply(id, false)).await
    }

    /// Reject a server→client request whose reply schema we don't speak (e.g.
    /// `item/tool/requestUserInput`) with a JSON-RPC error — codex fails that
    /// call instead of blocking on an answer that will never come.
    pub async fn respond_method_unsupported(&self, id: &Value) -> Result<()> {
        if self
            .unanswered
            .lock()
            .unwrap()
            .remove(&id.to_string())
            .is_none()
        {
            return Err(anyhow!("this request is no longer pending"));
        }
        self.write_line(&json!({
            "id": id,
            "error": { "code": -32601, "message": "orx does not handle this request type" },
        }))
        .await
    }

    /// Settle every outstanding server request in the shape its method can
    /// parse, so codex never stays blocked on a request orx is about to
    /// abandon. Used on interrupt paths. Approval → `{"decision":"cancel"}`
    /// (deny + interrupt that turn); UserInput → an empty `{"answers": {}}`
    /// (codex proceeds without answers); Other → a JSON-RPC error. (The old
    /// blanket `{decision:cancel}` was a latent bug: fired at a userInput id it
    /// left the request hanging.)
    pub async fn settle_pending_requests(&self) {
        let ids: Vec<(String, ServerReqKind)> = self.unanswered.lock().unwrap().drain().collect();
        for (id, kind) in ids {
            let Ok(id) = serde_json::from_str::<Value>(&id) else {
                continue;
            };
            let msg = match kind {
                ServerReqKind::McpApproval => {
                    json!({"id":id,"result":{"action":"cancel","content":null,"_meta":null}})
                }
                ServerReqKind::Approval => json!({ "id": id, "result": { "decision": "cancel" } }),
                ServerReqKind::UserInput => json!({ "id": id, "result": { "answers": {} } }),
                ServerReqKind::Other => json!({
                    "id": id,
                    "error": { "code": -32601, "message": "orx does not handle this request type" },
                }),
            };
            let _ = self.write_line(&msg).await;
        }
    }

    /// Let the child exit on its own, then stop the keeper's process tree; an orphaned
    /// native app-server would keep the thread's writer lock.
    async fn terminate(self: &Arc<Self>) {
        self.end(false).await;
    }

    /// With `interrupt`, a child whose turn orx ended without telling it gets SIGTERM at
    /// once. Detached, so a caller that gives up never leaves a child no stop can reach.
    async fn end(self: &Arc<Self>, interrupt: bool) {
        let client = self.clone();
        let ended = tokio::spawn(async move { client.end_detached(interrupt).await });
        if let Err(error) = ended.await {
            eprintln!("orx up: stopping a codex app-server failed: {error}");
        }
    }

    async fn end_detached(&self, interrupt: bool) {
        if keep::handing_off() || self.terminated.swap(true, Ordering::AcqRel) {
            return;
        }
        // A writer blocked on a wedged child holds stdin; that child gets SIGTERM instead.
        let input_closed = self.stdin.try_lock().map(|mut stdin| stdin.take()).is_ok();
        // Also ends what the app-server left behind: the npm wrapper's native child, tool shells.
        if self
            .child
            .end_agent(input_closed && !interrupt, TERMINATION_TIMEOUT)
            .await
            .is_none()
        {
            eprintln!("orx up: timed out reaping codex app-server");
        }
    }

    async fn write_line(&self, msg: &Value) -> Result<()> {
        if msg.get("method").is_none() {
            if let Some(id) = msg.get("id") {
                let mut state = self.durable.lock().unwrap();
                state.outbox.insert(id.to_string(), msg.clone());
                self.save_owner(&state)?;
            }
        }
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin
            .as_mut()
            .ok_or_else(|| anyhow!("codex app-server stdin: closed to stop Codex"))?;
        stdin
            .write_all(format!("{msg}\n").as_bytes())
            .await
            .map_err(|e| anyhow!("codex app-server stdin: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| anyhow!("codex app-server stdin: {e}"))?;
        Ok(())
    }

    fn save_owner(&self, state: &ClientState) -> Result<()> {
        let mut state = state.clone();
        state.models = self
            .turn_models
            .lock()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        self.child.set_owner(
            &keep::Owner::Agent {
                harness: "codex".into(),
                session_id: self.session_id.clone(),
                state: serde_json::to_value(&state)?,
            }
            .to_value(),
        )
    }

    pub fn checkpoint_offset(&self) -> u64 {
        self.durable.lock().unwrap().offset
    }

    pub fn saved_through(&self, offset: u64) -> Result<()> {
        let mut state = self.durable.lock().unwrap();
        state.offset = state.offset.max(offset);
        state
            .outbound
            .retain(|_, request| request.response_at.is_none_or(|end| end > offset));
        self.save_owner(&state)
    }

    pub fn preparing_turn(&self) -> Result<()> {
        *self.active_turn.lock().unwrap() = None;
        let mut state = self.durable.lock().unwrap();
        state.active_turn = None;
        self.save_owner(&state)
    }

    pub fn awaiting_turn_start(&self) -> bool {
        self.durable
            .lock()
            .unwrap()
            .outbound
            .values()
            .any(|request| request.method == "turn/start")
    }

    pub fn active_turn(&self) -> Option<String> {
        self.active_turn.lock().unwrap().clone()
    }

    pub fn keeper(&self) -> &KeptHandle {
        &self.child
    }

    /// Register the session's turn event sink. The returned guard deregisters
    /// on drop (including task abort mid-turn).
    pub fn register_turn(self: &Arc<Self>, tx: mpsc::UnboundedSender<TurnEvent>) -> TurnRoute {
        *self.turn.lock().unwrap() = Some(tx.clone());
        let state = self.durable.lock().unwrap();
        for (id, (method, params)) in &state.inbound {
            if state.outbox.contains_key(id) {
                continue;
            }
            if let Ok(id) = serde_json::from_str(id) {
                let _ = tx.send(TurnEvent::Request {
                    id,
                    method: method.clone(),
                    params: params.clone(),
                });
            }
        }
        self.reader_ready.notify_one();
        TurnRoute {
            client: self.clone(),
            tx,
        }
    }

    fn touch(&self) {
        *self.last_active.lock().unwrap() = Instant::now();
    }

    /// When this child was last used, or `None` while a turn or request is in flight.
    fn idle_since(&self) -> Option<Instant> {
        if self.terminated.load(Ordering::Acquire)
            || self.turn.lock().unwrap().is_some()
            || !self.pending.lock().unwrap().is_empty()
        {
            return None;
        }
        Some(*self.last_active.lock().unwrap())
    }

    /// The thread this child has already started/resumed, if any.
    pub fn resumed_thread(&self) -> Option<String> {
        self.resumed_thread.lock().unwrap().clone()
    }

    pub fn set_resumed_thread(&self, thread_id: &str) {
        *self.resumed_thread.lock().unwrap() = Some(thread_id.to_string());
        let mut state = self.durable.lock().unwrap();
        state.thread = Some(thread_id.to_string());
        if let Err(error) = self.save_owner(&state) {
            eprintln!("codex owner state: {error:#}");
        }
    }

    /// The effective model codex reported for this child's thread, if known.
    pub fn thread_model(&self) -> Option<String> {
        self.thread_model.lock().unwrap().clone()
    }

    /// Record the effective model from a `thread/start` / `thread/resume`
    /// response (the top-level `model` field); ignores absent/empty values so a
    /// response without one never clobbers a known model.
    pub fn set_thread_model(&self, model: Option<&str>) {
        if let Some(model) = model.filter(|m| !m.is_empty()) {
            *self.thread_model.lock().unwrap() = Some(model.to_string());
            let mut state = self.durable.lock().unwrap();
            state.model = Some(model.to_string());
            if let Err(error) = self.save_owner(&state) {
                eprintln!("codex owner state: {error:#}");
            }
        }
    }

    /// The collaboration-mode mask this child last sent (`"plan"` / `"default"`),
    /// if any — the in-memory belt-and-braces un-stick signal.
    pub fn last_collab_mode(&self) -> Option<&'static str> {
        *self.last_collab_mode.lock().unwrap()
    }

    pub fn set_last_collab_mode(&self, mode: &'static str) {
        *self.last_collab_mode.lock().unwrap() = Some(mode);
        let mut state = self.durable.lock().unwrap();
        state.collab_mode = Some(mode.into());
        if let Err(error) = self.save_owner(&state) {
            eprintln!("codex collaboration mode: {error:#}");
        }
    }

    pub fn native_store(&self) -> NativeStore {
        self.native_store
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn turn_models(&self) -> &std::sync::Mutex<HashMap<(String, String), String>> {
        &self.turn_models
    }

    pub fn set_active_turn(&self, turn_id: &str) {
        *self.active_turn.lock().unwrap() = Some(turn_id.to_string());
        let mut state = self.durable.lock().unwrap();
        state.active_turn = Some(turn_id.to_string());
        if let Err(error) = self.save_owner(&state) {
            eprintln!("codex owner state: {error:#}");
        }
    }

    pub async fn read_turn_items(&self, thread_id: &str, turn_id: &str) -> Option<Vec<Value>> {
        let result = self
            .try_request_with_timeout(
                "thread/read",
                json!({ "threadId": thread_id, "includeTurns": true }),
                HISTORY_READ_TIMEOUT,
            )
            .await
            .ok()?
            .ok()?;
        result
            .get("thread")?
            .get("turns")?
            .as_array()?
            .iter()
            .find(|turn| turn.get("id").and_then(Value::as_str) == Some(turn_id))?
            .get("items")?
            .as_array()
            .cloned()
    }

    /// Best-effort native interrupt of the in-flight turn. Bounded: this sits
    /// on the user-facing interrupt/delete paths, and a wedged child (stdin
    /// full, not answering) must not hold them for the full request timeout.
    pub async fn interrupt_active_turn(&self) {
        // Settle outstanding requests first, each in its own reply shape (an
        // approval `cancel` both denies and interrupts server-side; a userInput
        // gets an empty answer set) — so codex is never left waiting on a
        // request whose turn orx is abandoning.
        self.settle_pending_requests().await;
        let thread_id = self.resumed_thread();
        let turn_id = self.active_turn.lock().unwrap().clone();
        if let (Some(thread_id), Some(turn_id)) = (thread_id, turn_id) {
            let _ = tokio::time::timeout(
                INTERRUPT_TIMEOUT,
                self.request(
                    "turn/interrupt",
                    json!({ "threadId": thread_id, "turnId": turn_id }),
                ),
            )
            .await;
        }
    }
}

/// Removes a request's `pending` entry however its caller exits, including
/// cancellation by an outer timeout; a stale entry would mark the child busy forever.
struct PendingRequest<'a> {
    client: &'a CodexClient,
    id: i64,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        self.client.pending.lock().unwrap().remove(&self.id);
    }
}

/// RAII turn registration — dropping (normal exit or task abort) detaches the
/// event sink so a dangling turn can't receive another turn's events. An
/// aborted task's guard drops *asynchronously*, possibly after a successor
/// turn has already registered — so drop only detaches its *own* channel,
/// never a successor's.
pub struct TurnRoute {
    client: Arc<CodexClient>,
    tx: mpsc::UnboundedSender<TurnEvent>,
}

impl Drop for TurnRoute {
    fn drop(&mut self) {
        let mut turn = self.client.turn.lock().unwrap();
        if turn.as_ref().is_some_and(|t| t.same_channel(&self.tx)) {
            *turn = None;
            drop(turn);
            *self.client.active_turn.lock().unwrap() = None;
            self.client.touch();
        }
    }
}

/// Reader task: pump child stdout, route each line. On EOF every pending
/// request fails and the live turn (if any) gets [`TurnEvent::Closed`].
async fn read_loop(client: Arc<CodexClient>, stdout: KeptStdout) {
    read_loop_at(client, stdout, 0).await;
}

async fn read_loop_at(client: Arc<CodexClient>, stdout: KeptStdout, offset: u64) {
    let mut lines = stdout.lines();
    if let Err(error) = lines.skip_to(offset) {
        eprintln!("codex resume offset: {error}");
        client.terminate().await;
        return;
    }
    let mut deferred_usage: Vec<(u64, String, Value)> = Vec::new();
    loop {
        deferred_usage.retain(|(offset, _, _)| *offset > client.checkpoint_offset());
        let next = tokio::select! {
            biased;
            line = lines.next_line() => line,
            _ = client.child.wait() => break,
        };
        let (line, offset) = match next {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                eprintln!("codex app-server stdout: {error}");
                break;
            }
        };
        match classify_line(&line) {
            Line::Response { id, result } => {
                client.touch();
                let starting;
                {
                    let mut state = client.durable.lock().unwrap();
                    starting = state.outbound.get_mut(&id).is_some_and(|request| {
                        request.response_at = Some(offset);
                        request.method == "turn/start"
                    });
                    if starting {
                        if let Ok(value) = &result {
                            state.active_turn = value
                                .pointer("/turn/id")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            *client.active_turn.lock().unwrap() = state.active_turn.clone();
                        }
                    }
                    if let Err(error) = client.save_owner(&state) {
                        eprintln!("codex request state: {error:#}");
                        break;
                    }
                }
                if let Some(tx) = client.pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(result);
                } else if starting {
                    if let Some(tx) = client.turn.lock().unwrap().as_ref() {
                        let _ = tx.send(TurnEvent::At {
                            offset,
                            event: Box::new(TurnEvent::StartResult(result)),
                        });
                    }
                }
            }
            Line::Request { id, method, params } => {
                client.touch();
                let kind = server_req_kind(&method);
                if client
                    .durable
                    .lock()
                    .unwrap()
                    .outbox
                    .contains_key(&id.to_string())
                {
                    continue;
                }
                {
                    let mut state = client.durable.lock().unwrap();
                    if state.answered.contains(&id.to_string()) {
                        continue;
                    }
                    state
                        .inbound
                        .insert(id.to_string(), (method.clone(), params.clone()));
                    if let Err(error) = client.save_owner(&state) {
                        eprintln!("codex inbound request state: {error:#}");
                        break;
                    }
                }
                client
                    .unanswered
                    .lock()
                    .unwrap()
                    .insert(id.to_string(), kind);
                let routed = {
                    let turn = client.turn.lock().unwrap();
                    turn.as_ref().is_some_and(|tx| {
                        tx.send(TurnEvent::At {
                            offset,
                            event: Box::new(TurnEvent::Request {
                                id: id.clone(),
                                method,
                                params,
                            }),
                        })
                        .is_ok()
                    })
                };
                if !routed {
                    // No turn is listening (raced an abort). Never leave the
                    // server hanging on a request nobody will answer — each
                    // kind gets the reply shape its method can parse.
                    match kind {
                        ServerReqKind::Approval | ServerReqKind::McpApproval => {
                            let _ = client.respond_decline(&id).await;
                        }
                        ServerReqKind::UserInput => {
                            let _ = client.respond(&id, json!({ "answers": {} })).await;
                        }
                        ServerReqKind::Other => {
                            let _ = client.respond_method_unsupported(&id).await;
                        }
                    }
                }
            }
            Line::Notification { method, params } => {
                // Thread work (incl. sub-agent tails) is activity; account/status chatter is not.
                if params.get("threadId").is_some() {
                    client.touch();
                }
                // Codex settled a request itself (approval deadline, answer
                // raced): the id is no longer answerable — drop it from the
                // pending set so the stale-answer guard stays truthful.
                if method == "serverRequest/resolved" {
                    if let Some(request_id) = params.get("requestId") {
                        let mut state = client.durable.lock().unwrap();
                        state.inbound.remove(&request_id.to_string());
                        state.outbox.remove(&request_id.to_string());
                        state.answered.insert(request_id.to_string());
                        if let Err(error) = client.save_owner(&state) {
                            eprintln!("codex resolved request state: {error:#}");
                            break;
                        }
                        client
                            .unanswered
                            .lock()
                            .unwrap()
                            .remove(&request_id.to_string());
                    }
                }
                if method == "model/rerouted" {
                    crate::local::harness::codex::capture_reroute(
                        None,
                        &client.turn_models,
                        &params,
                    );
                }
                let turn = client.turn.lock().unwrap();
                // Raw event tracing for sub-agent lifecycle debugging; enable
                // with ORX_CODEX_EVENT_LOG=1 on the backend (dev only). The
                // flag is read once — this sits on the per-notification path.
                static EVENT_LOG: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
                    std::env::var("ORX_CODEX_EVENT_LOG").is_ok_and(|v| !v.is_empty() && v != "0")
                });
                if *EVENT_LOG {
                    eprintln!(
                        "[codex-event] listener={} method={} thread={} turn={}",
                        turn.is_some(),
                        method,
                        params
                            .get("threadId")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("-"),
                        params
                            .get("turnId")
                            .or_else(|| params.pointer("/turn/id"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("-")
                    );
                }
                if turn.is_some()
                    && matches!(
                        method.as_str(),
                        "thread/tokenUsage/updated" | "turn/completed"
                    )
                {
                    deferred_usage.push((offset, method.clone(), params.clone()));
                }
                let event = TurnEvent::Notification { method, params };
                let unsent = match turn.as_ref() {
                    Some(tx) => tx
                        .send(TurnEvent::At {
                            offset,
                            event: Box::new(event),
                        })
                        .err()
                        .map(|error| error.0),
                    None => Some(event),
                };
                drop(turn);
                if let Some(TurnEvent::Notification { method, params }) = unsent {
                    match crate::store::Store::open().and_then(|store| {
                        for (_, method, params) in &deferred_usage {
                            crate::local::harness::codex::record_unowned_in(
                                &store,
                                &client.session_id,
                                &client.turn_models,
                                method,
                                params,
                            )?;
                        }
                        crate::local::harness::codex::record_unowned_in(
                            &store,
                            &client.session_id,
                            &client.turn_models,
                            &method,
                            &params,
                        )
                    }) {
                        Ok(()) => {
                            deferred_usage.clear();
                            if keep::handing_off() {
                                continue;
                            }
                            if let Err(error) = client.saved_through(offset) {
                                eprintln!("codex idle offset: {error:#}");
                                break;
                            }
                            client.child.ack(offset);
                        }
                        Err(error) => {
                            eprintln!("codex idle output: {error:#}");
                            break;
                        }
                    }
                }
            }
            Line::Junk => {}
        }
    }
    if client.child.try_wait().is_none() {
        client.terminate().await;
    }
    client.pending.lock().unwrap().clear();
    client.unanswered.lock().unwrap().clear();
    if let Some(tx) = client.turn.lock().unwrap().as_ref() {
        let _ = tx.send(TurnEvent::Closed);
    }
    crate::local::harness::codex::close_unowned(&client.session_id);
}

pub(crate) fn alphaxiv_config(servers: &Value) -> Result<Value> {
    let name = if servers.get("alphaxiv").is_some() {
        "orx_alphaxiv"
    } else {
        "alphaxiv"
    };
    if name == "orx_alphaxiv" && servers.get(name).is_some() {
        return Err(anyhow!("Codex's orx_alphaxiv server name is reserved for OpenResearch. Rename that server in your Codex configuration."));
    }
    let mut overrides = json!({});
    if name == "orx_alphaxiv" {
        overrides["alphaxiv"] = json!({"enabled":false});
    }
    let tools: serde_json::Map<_, _> = crate::commands::alphaxiv_mcp::WRITE_TOOLS
        .iter()
        .map(|name| ((*name).to_owned(), json!({"approval_mode":"prompt"})))
        .collect();
    overrides[name] = json!({
        "command":crate::paths::spawnable_exe()?, "args":["alphaxiv-mcp"],
        "env_vars":["XDG_CONFIG_HOME", "ALPHAXIV_API_URL"],
        "tool_timeout_sec":300,
        "default_tools_approval_mode":"approve",
        "tools":tools
    });
    Ok(overrides)
}

/// Spawn `codex app-server` (no handshake yet — see `CodexHost::ensure`, which
/// registers the client *before* the handshake so every kill path can reach
/// the child even if the spawning turn task is aborted mid-handshake).
async fn spawn_client(
    session_id: &str,
    up_port: Option<u16>,
    native_store: NativeStore,
    profile: Option<String>,
    bin: PathBuf,
    codex_home: PathBuf,
) -> Result<Arc<CodexClient>> {
    let mut cmd = crate::process::tokio_command(&bin);
    cmd.arg("app-server");
    if let Some(override_arg) =
        native_store::codex_sqlite_override(native_store, &codex_home, profile.is_some())
    {
        cmd.args(["-c", &override_arg]);
    }

    crate::local::chat::prepare_env(&mut cmd);
    cmd.env("CODEX_HOME", codex_home);
    // Stamp the launching session (one app-server child per orx session) so a
    // run the agent starts via `orx exp run` is tagged with it and can be
    // explicitly subscribed to. After prepare_env so it isn't shadowed.
    crate::local::chat::set_chat_session_env(&mut cmd, session_id, "codex", up_port)?;
    let state = ClientState {
        legacy: native_store == NativeStore::Legacy,
        profile: profile.clone(),
        bin: bin.clone(),
        alphaxiv_signed_in: crate::alphaxiv::signed_in(),
        next_id: 1,
        ..Default::default()
    };
    let mut child = keep::spawn(keep::Keep {
        command: cmd.as_std(),
        io: keep::Io::Pipes {
            stderr: Some(crate::store::data_dir().join("agent-codex.log")),
        },
        retain: keep::LOSSLESS,
        owner: keep::Owner::Agent {
            harness: "codex".into(),
            session_id: session_id.into(),
            state: serde_json::to_value(&state)?,
        }
        .to_value(),
    })
    .await?;
    let stdout = child
        .take_stdout()
        .ok_or_else(|| anyhow!("Codex keeper has no stdout"))?;
    let client = client_from_kept(child, session_id, state)?;
    tokio::spawn(read_loop(client.clone(), stdout));
    Ok(client)
}

fn client_from_kept(
    mut child: keep::Kept,
    session_id: &str,
    state: ClientState,
) -> Result<Arc<CodexClient>> {
    let stdin = child
        .take_stdin()
        .ok_or_else(|| anyhow!("Codex keeper has no stdin"))?;
    let unanswered = state
        .inbound
        .iter()
        .filter(|(id, _)| !state.outbox.contains_key(*id))
        .map(|(id, (method, _))| (id.clone(), server_req_kind(method)))
        .collect();
    Ok(Arc::new(CodexClient {
        child: child.handle(),
        stdin: Mutex::new(Some(stdin)),
        next_id: AtomicI64::new(state.next_id.max(1)),
        pending: std::sync::Mutex::new(HashMap::new()),
        turn: std::sync::Mutex::new(None),
        unanswered: std::sync::Mutex::new(unanswered),
        active_turn: std::sync::Mutex::new(state.active_turn.clone()),
        resumed_thread: std::sync::Mutex::new(state.thread.clone()),
        thread_model: std::sync::Mutex::new(state.model.clone()),
        last_collab_mode: std::sync::Mutex::new(match state.collab_mode.as_deref() {
            Some("plan") => Some("plan"),
            Some("default") => Some("default"),
            _ => None,
        }),
        native_store: if state.legacy {
            NativeStore::Legacy
        } else {
            NativeStore::Isolated
        },
        profile: state.profile.clone(),
        alphaxiv_signed_in: state.alphaxiv_signed_in,
        bin: state.bin.clone(),
        session_id: session_id.into(),
        turn_models: std::sync::Mutex::new(state.models.iter().cloned().collect()),
        last_active: std::sync::Mutex::new(Instant::now()),
        durable: std::sync::Mutex::new(state),
        reader_ready: tokio::sync::Notify::new(),
        terminated: AtomicBool::new(false),
    }))
}

/// The `initialize` → `initialized` handshake. Split from `spawn_client` so
/// the host can register the client between the two (abort-safe teardown).
async fn handshake(client: &CodexClient) -> Result<()> {
    let init = client.request(
        "initialize",
        json!({
            "clientInfo": {
                "name": "orx",
                "title": "OpenResearch",
                "version": env!("CARGO_PKG_VERSION"),
            },
            // Unconditional: `turn/start.collaborationMode` (the plan-mode mask,
            // see harness/codex.rs) is rejected with -32600 "requires
            // experimentalApi capability" without this. Harmless when unused —
            // it only unlocks the experimental surface. Pinned in the 0.144
            // live spike.
            "capabilities": { "experimentalApi": true },
        }),
    );
    let result = match tokio::time::timeout(HANDSHAKE_TIMEOUT, init).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(anyhow!(
                "codex app-server did not answer initialize within {}s; see {}",
                HANDSHAKE_TIMEOUT.as_secs(),
                crate::store::data_dir().join("agent-codex.log").display()
            ));
        }
    };
    client
        .write_line(&json!({ "method": "initialized" }))
        .await?;
    let mut state = client.durable.lock().unwrap();
    state.initialize = Some(result);
    client.save_owner(&state)
}

/// The `orx up` codex host: one `codex app-server` child per chat session,
/// keyed by the orx session id. Share as `Arc<CodexHost>` in axum state.
pub struct CodexHost {
    /// Serializes ensure() spawns (a spawn is quick, and one at a time keeps
    /// the handshake traffic sane). `inner` is never held across a spawn.
    spawn_lock: Mutex<()>,
    inner: Mutex<HashMap<String, Arc<CodexClient>>>,
    up_port: std::sync::OnceLock<u16>,
}

impl Default for CodexHost {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexHost {
    pub fn new() -> Self {
        Self {
            spawn_lock: Mutex::new(()),
            inner: Mutex::new(HashMap::new()),
            up_port: std::sync::OnceLock::new(),
        }
    }

    pub async fn adopt(&self, adoption: crate::local::harness::Adoption) -> Result<()> {
        let _spawning = self.spawn_lock.lock().await;
        let state: ClientState = serde_json::from_value(adoption.state)?;
        if state.initialize.is_none() {
            return Err(anyhow!(
                "Codex keeper has no completed initialize handshake"
            ));
        }
        if self.inner.lock().await.contains_key(&adoption.session_id) {
            return Err(anyhow!(
                "Codex session {} already has an app-server",
                adoption.session_id
            ));
        }
        let mut child = keep::attach(&adoption.keep_id).await?;
        let stdout = child
            .take_stdout()
            .ok_or_else(|| anyhow!("Codex keeper has no stdout"))?;
        let client = client_from_kept(child, &adoption.session_id, state)?;
        let answers: Vec<_> = client
            .durable
            .lock()
            .unwrap()
            .outbox
            .values()
            .cloned()
            .collect();
        for answer in answers {
            client.write_line(&answer).await?;
        }
        self.inner
            .lock()
            .await
            .insert(adoption.session_id, client.clone());
        tokio::spawn(async move {
            if adoption.resume_at.is_some() {
                client.reader_ready.notified().await;
            }
            read_loop_at(client, stdout, adoption.resume_at.unwrap_or(0)).await;
        });
        Ok(())
    }

    pub fn set_up_port(&self, port: u16) {
        let _ = self.up_port.set(port);
    }

    /// Spawn (or reuse) this session's app-server child. Idempotent while the
    /// child is alive; a dead child is replaced (its thread is re-resumed by
    /// the caller — `resumed_thread` starts empty on the replacement).
    ///
    /// Cancelled startup retires its child; the detached handoff owns cleanup.
    pub async fn ensure(
        self: &Arc<Self>,
        session_id: &str,
        native_store: NativeStore,
        profile: Option<&str>,
    ) -> Result<Arc<CodexClient>> {
        let profile = profile.map(str::to_string);
        let preparing_profile = profile.clone();
        let codex_home = tokio::task::spawn_blocking(move || match preparing_profile {
            Some(id) => crate::local::agent_profiles::prepare(&id, "codex"),
            None => native_store::prepare_codex(native_store),
        })
        .await
        .map_err(|error| anyhow!("Codex config preparation failed: {error}"))?
        .map_err(|e| SetupFailure::tag("codex_setup_agent_home", e))?;
        let host = self.clone();
        let session = session_id.to_string();
        let spawn_profile = profile.clone();
        let selected = find_codex();
        self.ensure_with_spawn(session_id, native_store, profile, selected, async move {
            let bin =
                find_codex_required().map_err(|e| SetupFailure::tag("codex_setup_binary", e))?;
            Ok(move || async move {
                spawn_client(
                    &session,
                    host.up_port.get().copied(),
                    native_store,
                    spawn_profile,
                    bin,
                    codex_home,
                )
                .await
            })
        })
        .await
    }

    async fn ensure_with_spawn<F, Fut>(
        self: &Arc<Self>,
        session_id: &str,
        native_store: NativeStore,
        profile: Option<String>,
        bin: Option<PathBuf>,
        prepare: impl std::future::Future<Output = Result<F>>,
    ) -> Result<Arc<CodexClient>>
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Arc<CodexClient>>> + Send,
    {
        {
            let _spawning = self.spawn_lock.lock().await;
            if let Some(existing) = self
                .reusable_client(session_id, native_store, profile.as_deref(), bin.as_deref())
                .await
            {
                return Ok(existing);
            }
        }
        let spawn = prepare.await?;
        let host = self.clone();
        let session = session_id.to_string();
        let (ready_tx, ready_rx) = oneshot::channel();
        let (accepted_tx, mut accepted_rx) = oneshot::channel();
        tokio::spawn(async move {
            let _spawning = host.spawn_lock.lock().await;
            if ready_tx.is_closed() {
                return;
            }
            // Another caller may have completed startup during preparation.
            if let Some(existing) = host
                .reusable_client(&session, native_store, profile.as_deref(), bin.as_deref())
                .await
            {
                let _ = ready_tx.send(Ok(existing));
                return;
            }
            let stale = host.inner.lock().await.remove(&session);
            if let Some(stale) = stale {
                stale.terminate().await;
            }
            if ready_tx.is_closed() {
                return;
            }
            let client = match spawn().await {
                Ok(client) => client,
                Err(error) => {
                    let _ = ready_tx.send(Err(SetupFailure::tag("codex_setup_spawn", error)));
                    return;
                }
            };
            if ready_tx.is_closed() {
                client.terminate().await;
                return;
            }
            host.inner
                .lock()
                .await
                .insert(session.clone(), client.clone());
            let initialized = tokio::select! {
                result = handshake(&client) => result.map_err(|e| SetupFailure::tag("codex_setup_initialize", e)),
                _ = &mut accepted_rx => Err(anyhow!("Codex startup was cancelled")),
            };
            if let Err(error) = initialized {
                host.retire_client(&session, &client).await;
                let _ = ready_tx.send(Err(error));
                return;
            }
            if ready_tx.send(Ok(client.clone())).is_err() || accepted_rx.await.is_err() {
                host.retire_client(&session, &client).await;
            }
        });
        let result = ready_rx
            .await
            .map_err(|error| anyhow!("codex app-server bring-up task failed: {error}"))?;
        // No await between receiving and accepting: cancellation before this
        // hand-off closes accepted_rx and leaves cleanup to the bring-up task.
        let _ = accepted_tx.send(());
        result
    }

    /// Called under spawn_lock, so a client cannot be reused mid-handshake.
    async fn reusable_client(
        &self,
        session_id: &str,
        native_store: NativeStore,
        profile: Option<&str>,
        bin: Option<&Path>,
    ) -> Option<Arc<CodexClient>> {
        let guard = self.inner.lock().await;
        let client = guard.get(session_id)?;
        if client.native_store() == native_store
            && client.profile.as_deref() == profile
            && bin.is_none_or(|bin| bin == client.bin)
            && client.alphaxiv_signed_in == crate::alphaxiv::signed_in()
            && client.child.try_wait().is_none()
        {
            client.touch();
            Some(client.clone())
        } else {
            None
        }
    }

    async fn retire_client(&self, session_id: &str, client: &Arc<CodexClient>) {
        let mut guard = self.inner.lock().await;
        if guard
            .get(session_id)
            .is_some_and(|c| Arc::ptr_eq(c, client))
        {
            guard.remove(session_id);
        }
        drop(guard);
        client.terminate().await;
    }

    pub async fn resume_client(&self, session_id: &str, keep_id: &str) -> Option<Arc<CodexClient>> {
        self.inner
            .lock()
            .await
            .get(session_id)
            .filter(|client| client.child.id() == keep_id)
            .cloned()
    }

    /// The session's live client, if any (for inline replies / interrupts).
    pub async fn client_for(&self, session_id: &str) -> Option<Arc<CodexClient>> {
        let mut guard = self.inner.lock().await;
        let client = guard.get(session_id)?;
        if client.child.try_wait().is_none() {
            client.touch();
            return Some(client.clone());
        }
        let dead = guard.remove(session_id)?;
        drop(guard);
        // The wrapper can die alone while its native child lives on.
        dead.terminate().await;
        None
    }

    /// A running child signed in to `profile`'s account, None for the user's
    /// own, which every native store shares. Reading through it doesn't count
    /// as the chat's activity.
    pub async fn live_client(&self, profile: Option<&str>) -> Option<Arc<CodexClient>> {
        // Under spawn_lock, so a child is never asked before its handshake.
        let _spawning = self.spawn_lock.lock().await;
        let clients: Vec<_> = self.inner.lock().await.values().cloned().collect();
        for client in clients {
            if client.profile.as_deref() == profile && client.child.try_wait().is_none() {
                return Some(client);
            }
        }
        None
    }

    /// Interrupt and harvest the in-flight turn while its child is reachable,
    /// then retire the child so a successor cannot race native cancellation.
    pub async fn interrupt_session(&self, session_id: &str) -> Option<Vec<Value>> {
        let client = self.client_for(session_id).await?;
        let thread_id = client.resumed_thread();
        let turn_id = client.active_turn.lock().unwrap().clone();
        client.interrupt_active_turn().await;
        let items = match (thread_id, turn_id) {
            (Some(thread_id), Some(turn_id)) => client.read_turn_items(&thread_id, &turn_id).await,
            _ => None,
        };
        let retired = {
            let mut guard = self.inner.lock().await;
            if guard
                .get(session_id)
                .is_some_and(|current| Arc::ptr_eq(current, &client))
            {
                guard.remove(session_id)
            } else {
                None
            }
        };
        if let Some(retired) = retired {
            retired.terminate().await;
        }
        items
    }

    pub fn start_reaper(self: &Arc<Self>) {
        let host = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REAPER_INTERVAL);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(host) = host.upgrade() else {
                    break;
                };
                host.reap_idle_at(Instant::now(), IdlePolicy::current())
                    .await;
            }
        });
    }

    /// Release exited children and those idle past the policy's timeout or over
    /// its idle limit; the next turn respawns and `thread/resume`s.
    async fn reap_idle_at(&self, now: Instant, policy: IdlePolicy) {
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
            let exited = !client.child.try_wait().is_none();
            let Some(idle_since) = client.idle_since() else {
                continue;
            };
            // An exited npm wrapper can leave its native app-server running: release
            // it now rather than let it hold the warm slot.
            if exited {
                doomed.push((session_id, client, idle_since, "exited"));
            } else {
                idle.push((session_id, client, idle_since));
            }
        }
        let idle_for = idle
            .iter()
            .map(|(_, _, idle_since)| now.duration_since(*idle_since))
            .collect::<Vec<_>>();
        let (expired, warm): (Vec<usize>, Vec<usize>) =
            (0..idle.len()).partition(|&index| idle_for[index] >= policy.idle_timeout);
        let warm_idle_for = warm
            .iter()
            .map(|&index| idle_for[index])
            .collect::<Vec<_>>();
        let released = expired
            .into_iter()
            .map(|index| (index, "idle-expired"))
            .chain(
                policy
                    .over_limit(&warm_idle_for)
                    .into_iter()
                    .map(|index| (warm[index], "idle-limit")),
            )
            .collect::<Vec<_>>();
        for (index, reason) in released {
            let (session_id, client, idle_since) = idle[index].clone();
            doomed.push((session_id, client, idle_since, reason));
        }
        for (session_id, client, idle_since, reason) in doomed {
            // A replacement waits for the exit: two app-servers on one thread race its writer lock.
            let _spawning = self.spawn_lock.lock().await;
            let removed = {
                // Atomic with the touch in `ensure`/`client_for`: a child handed out since
                // the snapshot is spared.
                let mut guard = self.inner.lock().await;
                let untouched = guard
                    .get(&session_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &client))
                    && client.idle_since() == Some(idle_since);
                untouched.then(|| guard.remove(&session_id)).flatten()
            };
            if let Some(client) = removed {
                eprintln!(
                    "orx up: stopping codex app-server session={session_id} reason={reason} idle_s={}",
                    now.duration_since(idle_since).as_secs()
                );
                client.terminate().await;
            }
        }
    }

    /// Kill and reap one session's child (on session delete).
    pub async fn kill_session(&self, session_id: &str) {
        // A replacement waits for the exit: two app-servers on one thread race its writer lock.
        let _spawning = self.spawn_lock.lock().await;
        // Released before the stop, which can take seconds, so other chats' turns go on.
        let client = self.inner.lock().await.remove(session_id);
        if let Some(client) = client {
            client.terminate().await;
        }
    }

    /// Stop every keeper unless handing children to the next server. The shutdown already
    /// ended their turns without telling them.
    pub async fn shutdown(&self) {
        if keep::handing_off() {
            return;
        }
        let clients = self.inner.lock().await.drain().collect::<Vec<_>>();
        futures::future::join_all(clients.iter().map(|(_, client)| client.end(true))).await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[cfg(unix)]
    async fn idle_test_client(last_active: Instant) -> Arc<CodexClient> {
        test_client("cat >/dev/null", last_active, false).await
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn stopping_an_app_server_lets_codex_finish_saving_its_sign_in() {
        let dir = crate::local::git::TemporaryDirectory::new("orx-codex-stop").unwrap();
        let saved = dir.path().join("auth.json");
        // Like Codex, the fake app-server exits only once its input closes.
        let shell = format!("cat >/dev/null; echo refreshed > '{}'", saved.display());
        let client = test_client_running(&shell, Instant::now()).await;

        client.terminate().await;

        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "refreshed\n");
    }

    #[cfg(unix)]
    async fn test_client_running(shell: &str, last_active: Instant) -> Arc<CodexClient> {
        test_client(shell, last_active, false).await
    }
    #[cfg(unix)]
    pub(crate) async fn rpc_test_client(shell: &str) -> Arc<CodexClient> {
        test_client(shell, Instant::now(), true).await
    }
    #[cfg(unix)]
    async fn test_client(shell: &str, last_active: Instant, read_events: bool) -> Arc<CodexClient> {
        let mut command = crate::process::command("sh");
        command.args(["-c", shell]);
        let mut child = keep::spawn(keep::Keep {
            command: &command,
            io: keep::Io::Pipes { stderr: None },
            retain: keep::LOSSLESS,
            owner: serde_json::Value::Null,
        })
        .await
        .unwrap();
        let stdout = child.take_stdout().unwrap();
        let client = client_from_kept(
            child,
            "",
            ClientState {
                next_id: 1,
                ..Default::default()
            },
        )
        .unwrap();
        *client.last_active.lock().unwrap() = last_active;
        if read_events {
            tokio::spawn(read_loop(client.clone(), stdout));
        }
        client
    }

    #[cfg(unix)]
    pub(crate) async fn stop_test_client(client: &Arc<CodexClient>) {
        client.terminate().await;
    }

    #[test]
    fn configured_alphaxiv_is_disabled_when_our_server_uses_the_reserved_name() {
        let configured =
            json!({"alphaxiv":{"url":"https://example.com/mcp"},"other":{"command":"other"}});
        let overrides = alphaxiv_config(&configured).unwrap();
        assert_eq!(overrides["alphaxiv"], json!({"enabled":false}));
        assert_eq!(overrides["orx_alphaxiv"]["args"], json!(["alphaxiv-mcp"]));
        assert!(overrides.get("other").is_none());
        assert!(alphaxiv_config(&json!({"alphaxiv":{},"orx_alphaxiv":{}})).is_err());
        assert!(alphaxiv_config(&Value::Null)
            .unwrap()
            .get("alphaxiv")
            .is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mcp_approval_accepts_and_declines_in_its_native_reply_shape() {
        let client = idle_test_client(Instant::now()).await;
        let id = json!(0);
        client
            .unanswered
            .lock()
            .unwrap()
            .insert(id.to_string(), ServerReqKind::McpApproval);
        assert_eq!(
            client.approval_reply(&id, true),
            json!({"action":"accept","content":{},"_meta":null})
        );
        assert_eq!(
            client.approval_reply(&id, false),
            json!({"action":"decline","content":null,"_meta":null})
        );
        client.terminate().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_request_does_not_leave_the_child_busy() {
        let client = idle_test_client(Instant::now()).await;

        let cancelled = tokio::time::timeout(
            Duration::from_millis(20),
            client.try_request("thread/read", json!({})),
        )
        .await;

        assert!(cancelled.is_err());
        assert!(client.pending.lock().unwrap().is_empty());
        assert!(client.idle_since().is_some());
        client.terminate().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn client_reuse_requires_the_same_profile() {
        let host = Arc::new(CodexHost::new());
        let mut client = idle_test_client(Instant::now()).await;
        Arc::get_mut(&mut client).unwrap().profile = Some("codex-012345abcd".into());
        host.inner
            .lock()
            .await
            .insert("chat".into(), client.clone());
        assert!(host
            .reusable_client(
                "chat",
                NativeStore::Isolated,
                Some("codex-012345abcd"),
                None
            )
            .await
            .is_some());
        assert!(host
            .reusable_client(
                "chat",
                NativeStore::Isolated,
                Some("codex-abcdefghij"),
                None
            )
            .await
            .is_none());
        assert!(host
            .reusable_client("chat", NativeStore::Isolated, None, None)
            .await
            .is_none());
        host.kill_session("chat").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn client_reuse_requires_the_selected_binary() {
        let host = Arc::new(CodexHost::new());
        let mut client = idle_test_client(Instant::now()).await;
        Arc::get_mut(&mut client).unwrap().bin = PathBuf::from("/user/codex");
        host.inner
            .lock()
            .await
            .insert("chat".into(), client.clone());
        let reusable = |bin: &'static str| {
            let host = host.clone();
            async move {
                host.reusable_client("chat", NativeStore::Isolated, None, Some(Path::new(bin)))
                    .await
                    .is_some()
            }
        };
        assert!(reusable("/user/codex").await);
        assert!(!reusable("/managed/codex").await);
        host.kill_session("chat").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn plan_limits_are_read_through_a_chats_live_app_server() {
        use crate::local::usage::Read;

        let host = CodexHost::new();
        let result = r#"{"rateLimits":{"limitId":"codex","primary":{"usedPercent":2,"windowDurationMins":10080,"resetsAt":1795202690},"planType":"pro"}}"#;
        let client = rpc_test_client(&format!(
            r#"IFS= read -r request; case "$request" in *account/rateLimits/read*) printf '%s\n' '{{"id":1,"result":{result}}}' ;; esac; cat >/dev/null"#
        )).await;
        host.inner.lock().await.insert("chat".into(), client);
        // No binary to spawn: an answer can only come from the live child.
        let missing = Path::new("/nonexistent/codex");
        let read = crate::local::harness::codex::read_plan(&host, None, missing, None).await;
        assert!(matches!(read, Read::Plan(_)), "{read:?}");
        let other =
            crate::local::harness::codex::read_plan(&host, Some("codex-other"), missing, None)
                .await;
        assert!(
            matches!(&other, Read::Failed(message) if message.contains("/nonexistent/codex")),
            "another account's child is never asked: {other:?}"
        );
        host.kill_session("chat").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_mid_handshake_is_never_asked_about_the_account() {
        use futures::FutureExt;

        let host = CodexHost::new();
        let spawning = host.spawn_lock.lock().await;
        host.inner
            .lock()
            .await
            .insert("chat".into(), rpc_test_client("cat >/dev/null").await);
        assert!(
            host.live_client(None).now_or_never().is_none(),
            "waits for the handshake"
        );
        drop(spawning);
        assert!(host.live_client(None).await.is_some());
        host.kill_session("chat").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn initialized_bring_up_hands_off_a_reusable_client() {
        let host = Arc::new(CodexHost::new());
        let client = rpc_test_client(
            r#"IFS= read -r request; printf '%s\n' '{"id":1,"result":{}}'; cat >/dev/null"#,
        )
        .await;
        let spawned = client.clone();
        let ensured = tokio::time::timeout(
            Duration::from_secs(2),
            host.ensure_with_spawn("compact", NativeStore::Isolated, None, None, async {
                Ok(move || async move { Ok(spawned) })
            }),
        )
        .await;
        let reused = tokio::time::timeout(
            Duration::from_secs(2),
            host.ensure_with_spawn("compact", NativeStore::Isolated, None, None, async {
                Ok(|| async { panic!("initialized client must be reused") })
            }),
        )
        .await;
        host.kill_session("compact").await;
        assert!(Arc::ptr_eq(&ensured.unwrap().unwrap(), &client));
        assert!(Arc::ptr_eq(&reused.unwrap().unwrap(), &client));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_bring_up_does_not_block_other_chats_or_leak_children() {
        let host = Arc::new(CodexHost::new());
        let warm = idle_test_client(Instant::now()).await;
        host.inner.lock().await.insert("warm".into(), warm.clone());
        let (started_tx, started_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let launched = Arc::new(AtomicBool::new(false));
        let spawn_flag = launched.clone();
        let operation_host = host.clone();
        let operation = tokio::spawn(async move {
            operation_host
                .ensure_with_spawn("compact", NativeStore::Isolated, None, None, async move {
                    let _ = started_tx.send(());
                    let _ = release_rx.await;
                    Ok(move || async move {
                        spawn_flag.store(true, Ordering::Release);
                        Ok(idle_test_client(Instant::now()).await)
                    })
                })
                .await
        });
        started_rx.await.unwrap();
        // A stalled preparation neither blocks reuse nor another cold startup.
        let reused = tokio::time::timeout(
            Duration::from_secs(2),
            host.ensure_with_spawn("warm", NativeStore::Isolated, None, None, async {
                Ok(|| async { panic!("a warm client must not be spawned again") })
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(Arc::ptr_eq(&reused, &warm));
        let cold = tokio::time::timeout(
            Duration::from_secs(2),
            host.ensure_with_spawn("other", NativeStore::Isolated, None, None, async {
                Err::<fn() -> std::future::Ready<Result<Arc<CodexClient>>>, _>(anyhow!(
                    "preparation reached"
                ))
            }),
        )
        .await
        .unwrap();
        assert_eq!(cold.err().unwrap().to_string(), "preparation reached");
        operation.abort();
        let _ = operation.await;
        assert!(release_tx.send(()).is_err());
        assert!(!launched.load(Ordering::Acquire));
        host.kill_session("warm").await;

        // Cancellation after process launch still reaps the registered child.
        let client = idle_test_client(Instant::now()).await;
        let spawned = client.clone();
        let operation_host = host.clone();
        let operation = tokio::spawn(async move {
            operation_host
                .ensure_with_spawn("compact", NativeStore::Isolated, None, None, async {
                    Ok(move || async move { Ok(spawned) })
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while client.pending.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        operation.abort();
        let _ = operation.await;
        let _settled = tokio::time::timeout(Duration::from_secs(2), host.spawn_lock.lock())
            .await
            .unwrap();
        assert!(client.terminated.load(Ordering::Acquire));
        assert!(host.inner.lock().await.is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_releases_an_exited_wrapper_instead_of_keeping_it_warm() {
        let now = Instant::now();
        let host = CodexHost::new();
        let warm = idle_test_client(now - Duration::from_secs(60)).await;
        // The npm shape: the wrapper exits while its native child keeps running.
        let exited = test_client_running("sleep 600 & exit 0", now - Duration::from_secs(40)).await;
        exited.child.wait().await;
        for (session_id, client) in [("warm", &warm), ("exited", &exited)] {
            host.inner
                .lock()
                .await
                .insert(session_id.to_string(), client.clone());
        }

        host.reap_idle_at(now, IdlePolicy::LOW_MEMORY).await;

        let live = host.inner.lock().await.keys().cloned().collect::<Vec<_>>();
        assert_eq!(live, ["warm"]);
        assert!(exited.terminated.load(Ordering::Acquire));
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_releases_idle_children_and_spares_busy_ones() {
        let now = Instant::now();
        let host = CodexHost::new();
        let expired = idle_test_client(now - IdlePolicy::DEFAULT.idle_timeout).await;
        let recent = idle_test_client(now - Duration::from_secs(60)).await;
        let newest = idle_test_client(now - Duration::from_secs(40)).await;
        let in_turn = idle_test_client(now - Duration::from_secs(3600)).await;
        let in_request = idle_test_client(now - Duration::from_secs(3600)).await;
        let (tx, _rx) = mpsc::unbounded_channel();
        let _route = in_turn.register_turn(tx);
        let (request_tx, _request_rx) = oneshot::channel();
        in_request.pending.lock().unwrap().insert(1, request_tx);
        for (session_id, client) in [
            ("expired", &expired),
            ("recent", &recent),
            ("newest", &newest),
            ("in-turn", &in_turn),
            ("in-request", &in_request),
        ] {
            host.inner
                .lock()
                .await
                .insert(session_id.to_string(), client.clone());
        }

        host.reap_idle_at(now, IdlePolicy::DEFAULT).await;

        let mut live = host.inner.lock().await.keys().cloned().collect::<Vec<_>>();
        live.sort();
        assert_eq!(live, ["in-request", "in-turn", "newest", "recent"]);
        assert!(expired.terminated.load(Ordering::Acquire));

        host.reap_idle_at(now, IdlePolicy::LOW_MEMORY).await;
        assert!(recent.terminated.load(Ordering::Acquire));
        assert!(!newest.terminated.load(Ordering::Acquire));
        assert!(!in_request.terminated.load(Ordering::Acquire));
        assert!(!in_turn.terminated.load(Ordering::Acquire));
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adoption_preserves_pending_approval_and_request_ids_without_initialize() {
        let old = test_client(
            r#"printf '%s\n' '{"id":7,"result":{"late":true}}'; IFS= read -r approval; IFS= read -r request; printf '{"id":42,"result":{"approval":%s,"request":%s}}\n' "$approval" "$request"; cat >/dev/null"#,
            Instant::now(), false,
        ).await;
        let state = ClientState {
            initialize: Some(json!({"capabilities":{}})),
            next_id: 42,
            thread: Some("thread".into()),
            outbound: HashMap::from([(
                7,
                OutboundRequest {
                    method: "account/rateLimits/read".into(),
                    response_at: None,
                },
            )]),
            inbound: HashMap::from([(
                "5".into(),
                (
                    "item/commandExecution/requestApproval".into(),
                    json!({"threadId":"thread","turnId":"turn","itemId":"tool"}),
                ),
            )]),
            ..Default::default()
        };
        old.child
            .set_owner(
                &keep::Owner::Agent {
                    harness: "codex".into(),
                    session_id: "chat".into(),
                    state: serde_json::to_value(&state).unwrap(),
                }
                .to_value(),
            )
            .unwrap();
        let id = old.child.id().to_string();
        old.child.detach();
        drop(old);
        let host = CodexHost::new();
        host.adopt(crate::local::harness::Adoption {
            keep_id: id,
            session_id: "chat".into(),
            state: serde_json::to_value(state).unwrap(),
            resume_at: Some(0),
            commands: Vec::new(),
        })
        .await
        .unwrap();
        let client = host.client_for("chat").await.unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _route = client.register_turn(tx);
        let event = rx.recv().await.unwrap();
        assert!(matches!(event, TurnEvent::Request { id, .. } if id == json!(5)));
        client
            .respond(&json!(5), json!({"decision":"accept"}))
            .await
            .unwrap();
        assert!(client
            .respond(&json!(5), json!({"decision":"accept"}))
            .await
            .is_err());
        let result = client
            .request("account/rateLimits/read", json!({}))
            .await
            .unwrap();
        assert_eq!(result["approval"]["id"], 5);
        assert_eq!(result["request"]["id"], 42);
        assert_eq!(result["request"]["method"], "account/rateLimits/read");
        assert_eq!(client.next_id.load(Ordering::Relaxed), 43);
        assert!(client.pending.lock().unwrap().is_empty());
        host.kill_session("chat").await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn idle_adoption_resends_an_unconfirmed_reply_before_accepting_requests() {
        for resume_at in [None, Some(0)] {
            let old = test_client(
                r#"IFS= read -r approval; IFS= read -r request; printf '{"id":42,"result":{"approval":%s,"request":%s}}\n' "$approval" "$request"; cat >/dev/null"#,
                Instant::now(), false,
            ).await;
            let reply = json!({"id":5,"result":{"decision":"accept"}});
            let state = ClientState {
                initialize: Some(json!({})),
                next_id: 42,
                thread: Some("thread".into()),
                inbound: HashMap::from([(
                    "5".into(),
                    ("item/commandExecution/requestApproval".into(), json!({})),
                )]),
                outbox: HashMap::from([("5".into(), reply.clone())]),
                ..Default::default()
            };
            let keep_id = old.child.id().to_string();
            old.child.detach();
            drop(old);
            let host = CodexHost::new();
            host.adopt(crate::local::harness::Adoption {
                keep_id,
                session_id: "chat".into(),
                state: serde_json::to_value(state).unwrap(),
                resume_at,
                commands: Vec::new(),
            })
            .await
            .unwrap();
            let client = host.client_for("chat").await.unwrap();
            let (tx, mut rx) = mpsc::unbounded_channel();
            let _route = client.register_turn(tx);
            assert!(rx.try_recv().is_err());
            let result = tokio::time::timeout(
                Duration::from_secs(5),
                client.request("account/rateLimits/read", json!({})),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result["approval"], reply);
            assert_eq!(result["request"]["method"], "account/rateLimits/read");
            assert!(Arc::ptr_eq(
                &client,
                &host
                    .reusable_client("chat", NativeStore::Isolated, None, None)
                    .await
                    .unwrap()
            ));
            host.kill_session("chat").await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn app_server_resumes_the_same_turn_without_repeating_prompt_or_text() {
        use crate::local::chat::{Resume, TurnCtx, WirePart};
        use crate::local::harness::{codex::Codex, Harness};

        let prefix = "{\"method\":\"item/agentMessage/delta\",\"params\":{\"threadId\":\"thread\",\"turnId\":\"turn\",\"itemId\":\"item\",\"delta\":\"Hel\"}}\n";
        let old = test_client(
            r#"printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","itemId":"item","delta":"Hel"}}'; IFS= read -r go; printf '%s\n' '{"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","itemId":"item","delta":"lo"}}' '{"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed"}}}'; IFS= read -r request; case "$request" in *thread/read*) printf '%s\n' '{"id":42,"result":{"thread":{"turns":[{"id":"turn","items":[]}]}}}' ;; *) exit 23 ;; esac; cat >/dev/null"#,
            Instant::now(), false,
        ).await;
        let owner = ClientState {
            initialize: Some(json!({})),
            next_id: 42,
            thread: Some("thread".into()),
            active_turn: Some("turn".into()),
            ..Default::default()
        };
        let state = json!({"turn_id":"turn","open_requests":{},"sub_threads":{},"parent_done":false,"finished":false});
        let mut ctx = TurnCtx::test_stub();
        ctx.upsert_part(WirePart::text("item", "Hel"));
        ctx.set_native_session_id("thread");
        let offset = prefix.len() as u64;
        ctx.flush_at(old.keeper(), offset, &state).unwrap();
        old.write_line(&json!({"go":true})).await.unwrap();
        let keep_id = old.child.id().to_string();
        old.child.detach();
        drop(old);
        ctx.host
            .codex
            .adopt(crate::local::harness::Adoption {
                keep_id: keep_id.clone(),
                session_id: ctx.session_id.clone(),
                state: serde_json::to_value(owner).unwrap(),
                resume_at: Some(offset),
                commands: Vec::new(),
            })
            .await
            .unwrap();
        ctx.resume = Some(Resume {
            keep_id,
            offset,
            state,
        });
        let result = tokio::time::timeout(Duration::from_secs(10), Codex.run_turn(&mut ctx))
            .await
            .unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(
            ctx.assistant
                .parts
                .iter()
                .filter_map(|part| part.text.as_deref())
                .collect::<String>(),
            "Hello"
        );
        let client = ctx.host.codex.client_for(&ctx.session_id).await.unwrap();
        assert_eq!(client.next_id.load(Ordering::Relaxed), 43);
        ctx.host.codex.shutdown().await;
    }

    #[test]
    fn classify_discriminates_the_three_wire_shapes() {
        // Server→client request: id + method. Id kept as raw Value. (Fixture
        // captured live from the 0.144 spike, trimmed.)
        assert_eq!(
            classify_line(
                r#"{"method":"item/commandExecution/requestApproval","id":0,"params":{"threadId":"t1","turnId":"turn1","itemId":"call_1","startedAtMs":1,"reason":"Allow writing the requested probe file outside the workspace?","command":"/bin/zsh -lc 'touch /outside/probe.txt'","cwd":"/ws"}}"#
            ),
            Line::Request {
                id: json!(0),
                method: "item/commandExecution/requestApproval".into(),
                params: json!({
                    "threadId": "t1", "turnId": "turn1", "itemId": "call_1",
                    "startedAtMs": 1,
                    "reason": "Allow writing the requested probe file outside the workspace?",
                    "command": "/bin/zsh -lc 'touch /outside/probe.txt'", "cwd": "/ws",
                }),
            }
        );
        // Response to one of our requests: id only.
        assert_eq!(
            classify_line(r#"{"id":2,"result":{"thread":{"id":"abc"}}}"#),
            Line::Response {
                id: 2,
                result: Ok(json!({"thread":{"id":"abc"}}))
            }
        );
        // Error response preserves the structured JSON-RPC contract.
        assert_eq!(
            classify_line(
                r#"{"id":3,"error":{"code":-32001,"message":"busy","data":{"retryAfterMs":250}}}"#
            ),
            Line::Response {
                id: 3,
                result: Err(JsonRpcError {
                    code: -32001,
                    message: "busy".into(),
                    data: Some(json!({"retryAfterMs": 250})),
                })
            }
        );
        // Notification: method only.
        assert_eq!(
            classify_line(r#"{"method":"turn/completed","params":{"threadId":"t"}}"#),
            Line::Notification {
                method: "turn/completed".into(),
                params: json!({"threadId":"t"})
            }
        );
        // Junk never panics.
        assert_eq!(classify_line("not json"), Line::Junk);
        assert_eq!(classify_line("{}"), Line::Junk);
        // Non-integer response id (we only send integers) is junk, not a panic.
        assert_eq!(classify_line(r#"{"id":"weird","result":{}}"#), Line::Junk);
    }
}
