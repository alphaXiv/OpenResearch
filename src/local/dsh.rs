//! DeepSeek Harness ACP host — one long-lived `dsh --profile acp` child per
//! chat session, speaking newline-delimited JSON-RPC over stdio. Outbound
//! lines always include `jsonrpc: "2.0"` (DSH rejects the handshake without
//! it). Requests flow both ways: the agent sends `session/request_permission`
//! which we must answer by id.

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, oneshot, Mutex};

use crate::error::{anyhow, Result};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(150);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);
/// Cold `dsh --profile acp` can take >10s to answer initialize on first boot.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);

/// One inbound line, classified. JSON-RPC over one stream: a message with both
/// `id` and `method` is a server→client *request*; `id` alone is a response to
/// one of our requests; `method` alone is a notification.
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
                        .unwrap_or("dsh acp error")
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

/// One event delivered to the session's in-flight turn.
#[derive(Debug)]
pub enum TurnEvent {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Closed,
}

/// A live JSON-RPC connection to one session's `dsh --profile acp` child.
pub struct DshClient {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicI64,
    pending:
        std::sync::Mutex<HashMap<i64, oneshot::Sender<std::result::Result<Value, JsonRpcError>>>>,
    turn: std::sync::Mutex<Option<mpsc::UnboundedSender<TurnEvent>>>,
    unanswered: std::sync::Mutex<HashSet<String>>,
    /// ACP `sessionId` from session/new or session/resume.
    acp_session: std::sync::Mutex<Option<String>>,
}

impl DshClient {
    pub async fn try_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<std::result::Result<Value, JsonRpcError>> {
        self.try_request_with_timeout(method, params, REQUEST_TIMEOUT)
            .await
    }

    pub async fn try_request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<std::result::Result<Value, JsonRpcError>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        let sent = self
            .write_line(&json!({ "id": id, "method": method, "params": params }))
            .await;
        if let Err(e) = sent {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(_)) => Err(anyhow!("dsh acp closed during {method}")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!(
                    "dsh acp did not answer {method} within {}s",
                    timeout.as_secs()
                ))
            }
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        match self.try_request(method, params).await? {
            Ok(result) => Ok(result),
            Err(err) => Err(anyhow!("dsh {method} failed: {err}")),
        }
    }

    /// Answer a server→client request. Errors if `id` isn't pending.
    pub async fn respond(&self, id: &Value, result: Value) -> Result<()> {
        if !self.unanswered.lock().unwrap().remove(&id.to_string()) {
            return Err(anyhow!("this request is no longer pending"));
        }
        self.write_line(&json!({ "id": id, "result": result }))
            .await
    }

    pub async fn respond_method_unsupported(&self, id: &Value) -> Result<()> {
        if !self.unanswered.lock().unwrap().remove(&id.to_string()) {
            return Err(anyhow!("this request is no longer pending"));
        }
        self.write_line(&json!({
            "id": id,
            "error": { "code": -32601, "message": "orx does not handle this request type" },
        }))
        .await
    }

    /// Settle outstanding permission requests with reject-once so dsh is never
    /// left blocked on a request orx is about to abandon.
    pub async fn settle_pending_permissions(&self) {
        let ids: Vec<String> = self.unanswered.lock().unwrap().drain().collect();
        for id in ids {
            let Ok(id) = serde_json::from_str::<Value>(&id) else {
                continue;
            };
            let _ = self
                .write_line(&json!({
                    "id": id,
                    "result": {
                        "outcome": { "outcome": "selected", "optionId": "reject-once" }
                    }
                }))
                .await;
        }
    }

    async fn write_line(&self, msg: &Value) -> Result<()> {
        // DSH's ACP stack requires the JSON-RPC 2.0 envelope field; without it
        // initialize is rejected with id:null and the handshake times out.
        let mut obj = match msg {
            Value::Object(map) => map.clone(),
            other => {
                return Err(anyhow!("dsh acp write expects a JSON object, got {other}"));
            }
        };
        obj.entry("jsonrpc".to_string())
            .or_insert_with(|| json!("2.0"));
        let line = Value::Object(obj);
        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map_err(|e| anyhow!("dsh acp stdin: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| anyhow!("dsh acp stdin: {e}"))?;
        Ok(())
    }

    pub fn register_turn(self: &Arc<Self>, tx: mpsc::UnboundedSender<TurnEvent>) -> TurnRoute {
        *self.turn.lock().unwrap() = Some(tx.clone());
        TurnRoute {
            client: self.clone(),
            tx,
        }
    }

    pub fn acp_session_id(&self) -> Option<String> {
        self.acp_session.lock().unwrap().clone()
    }

    pub fn set_acp_session_id(&self, session_id: &str) {
        *self.acp_session.lock().unwrap() = Some(session_id.to_string());
    }

    pub fn has_unanswered(&self) -> bool {
        !self.unanswered.lock().unwrap().is_empty()
    }

    /// Best-effort: settle outstanding permission requests, then cancel the
    /// in-flight prompt so DSH is never left blocked on us.
    pub async fn interrupt_active(&self) {
        self.settle_pending_permissions().await;
        if let Some(session_id) = self.acp_session_id() {
            // ponytail: ACP session/cancel is a notification; $/cancel_request
            // if a hung prompt outlives this + kill.
            let _ = tokio::time::timeout(
                INTERRUPT_TIMEOUT,
                self.write_line(&json!({
                    "method": "session/cancel",
                    "params": { "sessionId": session_id }
                })),
            )
            .await;
        }
    }

    async fn kill(&self) {
        let _ = self.child.lock().await.kill().await;
    }
}

pub struct TurnRoute {
    client: Arc<DshClient>,
    tx: mpsc::UnboundedSender<TurnEvent>,
}

impl Drop for TurnRoute {
    fn drop(&mut self) {
        let mut turn = self.client.turn.lock().unwrap();
        if turn.as_ref().is_some_and(|t| t.same_channel(&self.tx)) {
            *turn = None;
        }
    }
}

async fn read_loop(client: Arc<DshClient>, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        match classify_line(&line) {
            Line::Response { id, result } => {
                if let Some(tx) = client.pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(result);
                }
            }
            Line::Request { id, method, params } => {
                client.unanswered.lock().unwrap().insert(id.to_string());
                let is_perm = method == "session/request_permission";
                let routed = {
                    let turn = client.turn.lock().unwrap();
                    turn.as_ref().is_some_and(|tx| {
                        tx.send(TurnEvent::Request {
                            id: id.clone(),
                            method,
                            params,
                        })
                        .is_ok()
                    })
                };
                if !routed {
                    if is_perm {
                        let _ = client
                            .respond(
                                &id,
                                json!({
                                    "outcome": { "outcome": "selected", "optionId": "reject-once" }
                                }),
                            )
                            .await;
                    } else {
                        let _ = client.respond_method_unsupported(&id).await;
                    }
                }
            }
            Line::Notification { method, params } => {
                let turn = client.turn.lock().unwrap();
                if let Some(tx) = turn.as_ref() {
                    let _ = tx.send(TurnEvent::Notification { method, params });
                }
            }
            Line::Junk => {}
        }
    }
    let _ = client.child.lock().await.kill().await;
    client.pending.lock().unwrap().clear();
    client.unanswered.lock().unwrap().clear();
    if let Some(tx) = client.turn.lock().unwrap().as_ref() {
        let _ = tx.send(TurnEvent::Closed);
    }
}

pub fn find_dsh() -> Option<std::path::PathBuf> {
    // Test hook: absolute path to a fake/real binary without mutating PATH.
    if let Ok(path) = std::env::var("ORX_DSH_BIN") {
        let path = std::path::PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    crate::local::shell_env::find_on_path("dsh")
}

async fn spawn_client(session_id: &str, up_port: Option<u16>) -> Result<Arc<DshClient>> {
    let bin = find_dsh().ok_or_else(|| {
        anyhow!("dsh not found on PATH — install DeepSeek Harness and add it to PATH first")
    })?;
    let mut cmd = Command::new(&bin);
    cmd.args(["--profile", "acp"]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(crate::local::chat::harness_log("dsh")?))
        .kill_on_drop(true);
    crate::local::chat::prepare_env(&mut cmd);
    crate::local::chat::set_chat_session_env(&mut cmd, session_id, "dsh", up_port);
    #[cfg(unix)]
    cmd.process_group(0);

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Could not spawn {} acp: {}", bin.display(), e))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("dsh acp: no stdout"))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("dsh acp: no stdin"))?;

    let client = Arc::new(DshClient {
        child: Mutex::new(child),
        stdin: Mutex::new(stdin),
        next_id: AtomicI64::new(1),
        pending: std::sync::Mutex::new(HashMap::new()),
        turn: std::sync::Mutex::new(None),
        unanswered: std::sync::Mutex::new(HashSet::new()),
        acp_session: std::sync::Mutex::new(None),
    });
    tokio::spawn(read_loop(client.clone(), stdout));
    Ok(client)
}

async fn handshake(client: &DshClient) -> Result<()> {
    let init = client.request(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": {
                "fs": { "readTextFile": false, "writeTextFile": false },
                "terminal": false
            },
            "clientInfo": {
                "name": "orx",
                "title": "OpenResearch",
                "version": env!("CARGO_PKG_VERSION"),
            },
        }),
    );
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, init).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => return Err(e),
        Err(_) => {
            return Err(anyhow!(
                "dsh acp did not answer initialize within {}s; see {}",
                HANDSHAKE_TIMEOUT.as_secs(),
                crate::store::data_dir().join("agent-dsh.log").display()
            ));
        }
    }
    match client
        .try_request("authenticate", json!({ "methodId": "deepseek" }))
        .await
    {
        Ok(Ok(_)) | Ok(Err(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

/// The `orx up` DSH host: one `dsh --profile acp` child per chat session.
pub struct DshHost {
    spawn_lock: Mutex<()>,
    inner: Mutex<HashMap<String, Arc<DshClient>>>,
    up_port: std::sync::OnceLock<u16>,
}

impl Default for DshHost {
    fn default() -> Self {
        Self::new()
    }
}

impl DshHost {
    pub fn new() -> Self {
        Self {
            spawn_lock: Mutex::new(()),
            inner: Mutex::new(HashMap::new()),
            up_port: std::sync::OnceLock::new(),
        }
    }

    pub fn set_up_port(&self, port: u16) {
        let _ = self.up_port.set(port);
    }

    pub async fn ensure(self: &Arc<Self>, session_id: &str) -> Result<Arc<DshClient>> {
        let _spawning = self.spawn_lock.lock().await;
        {
            let mut guard = self.inner.lock().await;
            if let Some(client) = guard.get(session_id) {
                if matches!(client.child.lock().await.try_wait(), Ok(None)) {
                    return Ok(client.clone());
                }
            }
            if let Some(stale) = guard.remove(session_id) {
                stale.kill().await;
            }
        }
        let host = self.clone();
        let session = session_id.to_string();
        tokio::spawn(async move {
            let client = spawn_client(&session, host.up_port.get().copied()).await?;
            {
                let mut guard = host.inner.lock().await;
                if let Some(existing) = guard.get(&session) {
                    if matches!(existing.child.lock().await.try_wait(), Ok(None)) {
                        let existing = existing.clone();
                        drop(guard);
                        client.kill().await;
                        return Ok(existing);
                    }
                }
                if let Some(stale) = guard.remove(&session) {
                    stale.kill().await;
                }
                guard.insert(session.clone(), client.clone());
            }
            if let Err(e) = handshake(&client).await {
                client.kill().await;
                let mut guard = host.inner.lock().await;
                if guard.get(&session).is_some_and(|c| Arc::ptr_eq(c, &client)) {
                    guard.remove(&session);
                }
                return Err(e);
            }
            Ok(client)
        })
        .await
        .map_err(|e| anyhow!("dsh acp bring-up task failed: {e}"))?
    }

    pub async fn client_for(&self, session_id: &str) -> Option<Arc<DshClient>> {
        let mut guard = self.inner.lock().await;
        let client = guard.get(session_id)?;
        if matches!(client.child.lock().await.try_wait(), Ok(None)) {
            Some(client.clone())
        } else {
            guard.remove(session_id);
            None
        }
    }

    pub async fn interrupt_session(&self, session_id: &str) {
        let Some(client) = self.client_for(session_id).await else {
            return;
        };
        client.interrupt_active().await;
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
            retired.kill().await;
        }
    }

    pub async fn kill_session(&self, session_id: &str) {
        if let Some(client) = self.inner.lock().await.remove(session_id) {
            if let Some(acp) = client.acp_session_id() {
                let _ = tokio::time::timeout(
                    INTERRUPT_TIMEOUT,
                    client.request("session/close", json!({ "sessionId": acp })),
                )
                .await;
            }
            client.kill().await;
        }
    }

    pub async fn shutdown(&self) {
        for (_, client) in self.inner.lock().await.drain() {
            client.kill().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_discriminates_the_three_wire_shapes() {
        assert_eq!(
            classify_line(
                r#"{"method":"session/request_permission","id":0,"params":{"sessionId":"s1","toolCall":{"toolCallId":"call_1"},"options":[{"optionId":"allow-once"}]}}"#
            ),
            Line::Request {
                id: json!(0),
                method: "session/request_permission".into(),
                params: json!({
                    "sessionId": "s1",
                    "toolCall": { "toolCallId": "call_1" },
                    "options": [{ "optionId": "allow-once" }],
                }),
            }
        );
        assert_eq!(
            classify_line(r#"{"id":2,"result":{"sessionId":"abc"}}"#),
            Line::Response {
                id: 2,
                result: Ok(json!({"sessionId":"abc"}))
            }
        );
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
        assert_eq!(
            classify_line(
                r#"{"method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk"}}}"#
            ),
            Line::Notification {
                method: "session/update".into(),
                params: json!({"sessionId":"s","update":{"sessionUpdate":"agent_message_chunk"}}),
            }
        );
        assert_eq!(classify_line("not json"), Line::Junk);
        assert_eq!(classify_line("{}"), Line::Junk);
        assert_eq!(classify_line(r#"{"id":"weird","result":{}}"#), Line::Junk);
    }
}
