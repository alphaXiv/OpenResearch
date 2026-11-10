//! `orx mcp` — the orx MCP server every agent harness is given.
//!
//! Hidden from `--help`: it's not a user command, it's the stdio MCP server a
//! harness spawns from orx's config — Claude via `--mcp-config`, Codex through
//! its thread config, OpenCode through its config file, ACP agents through
//! `session/new`. Stdio is the one transport every harness supports (ACP
//! requires it), so each relays here and this process forwards over localhost
//! HTTP to the `orx up` that spawned the agent.
//!
//! Two tool families ride it. `approve` is Claude's permission prompt tool
//! (`--permission-prompt-tool mcp__orx__approve`): each decision Claude would
//! have shown interactively arrives as a `tools/call`, and `orx up` either
//! decides by policy or surfaces a card and *blocks until the user answers*.
//! The orchestration tools (delegate and friends) are listed and served by
//! `orx up` itself, so this process only relays them.
//!
//! The wire is MCP's stdio transport: newline-delimited JSON-RPC 2.0, the same
//! framing as `local::codex` (this end is the server). Only three methods
//! matter — `initialize`, `tools/list`, `tools/call` — so it's hand-rolled
//! rather than pulling in an MCP crate.
//!
//! Failure posture: never hang the agent and never allow by accident. Any
//! transport or orx-side error answers `deny` (or a tool error) with the
//! reason; unknown methods get a JSON-RPC error; a missing env contract exits
//! nonzero (the harness reports the server as failed).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::error::{anyhow, Result};

/// Env contract the harness adapters put in the server's `env` block, so the
/// values survive however the harness spawns us. `token` is Claude's per-child
/// permission token; `agent_token` names the chat for orchestration tools.
struct GateEnv {
    up_port: u16,
    session_id: String,
    token: Option<String>,
    agent_token: Option<String>,
}

impl GateEnv {
    fn from_env() -> Result<Self> {
        let up_port = std::env::var("ORX_UP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow!("ORX_UP_PORT missing or invalid"))?;
        let session_id =
            std::env::var("ORX_SESSION_ID").map_err(|_| anyhow!("ORX_SESSION_ID missing"))?;
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Ok(Self {
            up_port,
            session_id,
            token: var("ORX_GATE_TOKEN"),
            agent_token: var("ORX_AGENT_TOKEN"),
        })
    }

    fn agent_request(&self, method: &str, params: Value) -> Option<Value> {
        let token = self.agent_token.as_ref()?;
        Some(json!({
            "sessionId": self.session_id,
            "token": token,
            "method": method,
            "params": params,
        }))
    }
}

pub async fn run() -> Result<()> {
    let env = GateEnv::from_env()?;
    // The long-poll deliberately blocks for as long as the user thinks; only
    // connecting is bounded. (orx up itself times pending cards out.)
    let http = crate::net::loopback_client()
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| anyhow!("http client: {e}"))?;
    let tools_url = format!("http://127.0.0.1:{}/api/internal/mcp", env.up_port);

    // Single writer task: tool calls are handled concurrently (Claude may
    // check several tools at once), so responses funnel through one channel
    // to keep stdout lines whole.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = out_rx.recv().await {
            let mut line = msg.to_string();
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    // A call the agent cancels, such as an interrupted `wait`, stops here too,
    // so `orx up` does not go on to claim news nobody will read.
    let calls: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>> = Arc::default();
    let stdin = BufReader::new(tokio::io::stdin());
    let mut lines = stdin.lines();
    while let Some(line) = lines.next_line().await.unwrap_or(None) {
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = msg.get("id").cloned();
        match msg.get("method").and_then(Value::as_str) {
            Some("initialize") => {
                // Echo the client's protocol version: we do nothing
                // version-specific, and echoing avoids a handshake mismatch.
                let version = msg
                    .pointer("/params/protocolVersion")
                    .cloned()
                    .unwrap_or_else(|| json!("2025-06-18"));
                let _ = out_tx.send(reply(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "orx", "version": env!("CARGO_PKG_VERSION") },
                    }),
                ));
            }
            Some("tools/list") => {
                let mut tools = Vec::new();
                if env.token.is_some() {
                    tools.push(json!({
                        "name": "approve",
                        "description": "Ask the orx user to approve a tool call",
                        "inputSchema": { "type": "object", "additionalProperties": true },
                    }));
                }
                // An unreachable `orx up` leaves only `approve`: the agent
                // keeps working without orchestration rather than failing.
                if let Some(body) = env.agent_request("tools/list", json!({})) {
                    if let Ok(listed) = relay(&http, &tools_url, body).await {
                        tools.extend(listed["tools"].as_array().cloned().unwrap_or_default());
                    }
                }
                let _ = out_tx.send(reply(id, json!({ "tools": tools })));
            }
            Some("tools/call") if msg.pointer("/params/name") == Some(&json!("approve")) => {
                // Handled concurrently: a held approval must not block the next
                // permission check (Claude can run tools in parallel).
                let args = msg
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let url = format!("http://127.0.0.1:{}/api/internal/permissions", env.up_port);
                let body = json!({
                    "sessionId": env.session_id,
                    "token": env.token,
                    "toolName": args.get("tool_name").and_then(Value::as_str).unwrap_or(""),
                    "toolInput": args.get("input").cloned().unwrap_or_else(|| json!({})),
                    "toolUseId": args.get("tool_use_id").and_then(Value::as_str),
                });
                let http = http.clone();
                let out = out_tx.clone();
                tokio::spawn(async move {
                    let decision = relay(&http, &url, body).await.unwrap_or_else(|e| {
                        json!({
                            "behavior": "deny",
                            "message": format!("orx approval bridge unavailable: {e}"),
                        })
                    });
                    // The permission-prompt-tool contract: the decision rides
                    // JSON-*stringified* inside an MCP text content block.
                    let _ = out.send(reply(
                        id,
                        json!({ "content": [{ "type": "text", "text": decision.to_string() }] }),
                    ));
                });
            }
            Some("tools/call") => {
                let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
                let body = env.agent_request("tools/call", params);
                let http = http.clone();
                let url = tools_url.clone();
                let out = out_tx.clone();
                let key = id.as_ref().map(Value::to_string);
                let running = calls.clone();
                let finished = key.clone();
                let call = tokio::spawn(async move {
                    let result = match body {
                        Some(body) => relay(&http, &url, body).await,
                        None => Err(anyhow!("orx tools are unavailable in this session")),
                    };
                    let result = result.unwrap_or_else(|e| {
                        json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true })
                    });
                    if let Some(key) = &finished {
                        running.lock().unwrap().remove(key);
                    }
                    let _ = out.send(reply(id, result));
                });
                if let Some(key) = key {
                    calls.lock().unwrap().insert(key, call.abort_handle());
                }
            }
            Some("notifications/cancelled") => {
                if let Some(key) = msg.pointer("/params/requestId").map(Value::to_string) {
                    if let Some(call) = calls.lock().unwrap().remove(&key) {
                        call.abort();
                    }
                }
            }
            // Notifications (no id) are fire-and-forget; anything else with an
            // id gets a proper method-not-found so the client never stalls.
            _ => {
                if let Some(id) = id {
                    let _ = out_tx.send(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32601, "message": "method not found" },
                    }));
                }
            }
        }
    }

    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

fn reply(id: Option<Value>, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result })
}

/// POST one request to `orx up` and return its JSON body; an error status
/// carries the server's `error` text.
async fn relay(http: &reqwest::Client, url: &str, body: Value) -> Result<Value> {
    let resp = http
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("{e}"))?;
    let status = resp.status();
    let body = resp.json::<Value>().await.map_err(|e| anyhow!("{e}"))?;
    if !status.is_success() {
        let reason = body["error"].as_str().unwrap_or("no reason given");
        return Err(anyhow!("orx up answered {status}: {reason}"));
    }
    Ok(body)
}

/// agy's workspace hooks (see `crate::local::harness::antigravity`), told apart by
/// their payloads: `PostInvocation` records the invocation, `Stop` holds a turn
/// open for background work, and `PreToolUse` stamps shell commands. Approvals
/// stay with agy, which asks over ACP.
pub async fn run_antigravity() -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut input = String::new();
    tokio::io::stdin().read_to_string(&mut input).await?;
    let payload = serde_json::from_str::<Value>(&input).unwrap_or(Value::Null);
    let output = if payload.get("initialNumSteps").is_some() {
        record_post_invocation(
            &crate::store::Store::open()?,
            &payload,
            std::env::var("ORX_USAGE_EXECUTION_ID").ok().as_deref(),
        )?;
        json!({})
    } else if payload.get("terminationReason").is_some() {
        background_stop(&payload).await
    } else if payload.pointer("/toolCall/name") == Some(&json!("invoke_subagent"))
        && chat_asks_before_tools()
    {
        json!({"decision": "deny", "reason": "[orx] This chat asks before running tools, and your subagents cannot ask. Use orx's `delegate` tool to hand the task to another chat, or do it yourself."})
    } else {
        let mut decision = json!({"decision": "allow"});
        if let Some(overwrite) = invocation_overwrite(&input) {
            decision["overwrite"] = overwrite;
        }
        decision
    };
    println!("{output}");
    Ok(())
}

fn chat_asks_before_tools() -> bool {
    let Ok(session) = std::env::var("ORX_SESSION_ID") else {
        return false;
    };
    let path = crate::local::harness::antigravity::approvals_path(&session);
    path.try_exists().unwrap_or_else(|error| {
        eprintln!(
            "orx antigravity-gate: could not read {}: {error}",
            path.display()
        );
        false
    })
}

/// The longest one `Stop` holds a turn without news, under the turn's silence watchdog; the agent
/// then decides whether to keep waiting by ending its turn again.
const STOP_HOLD_LIMIT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

/// agy finishes background work in turns of its own, which never reach its ACP client, so a `Stop`
/// that leaves work pending continues the same turn instead, once that work reports back. agy
/// reports each finished command, timer and subagent to the conversation's inbox; a daemon never
/// does, so one cannot hold the turn.
async fn background_stop(payload: &Value) -> Value {
    save_stop_error(payload);
    if payload.get("fullyIdle").and_then(Value::as_bool) != Some(false) {
        return json!({});
    }
    let inbox = payload["artifactDirectoryPath"]
        .as_str()
        .map(|dir| std::path::Path::new(dir).join(".system_generated/messages"));
    let reported = inbox.as_deref().map(read_inbox);
    if let Some(Err(error)) = &reported {
        eprintln!("orx antigravity-gate: could not read agy's inbox: {error}");
    }
    // agy fires no timer while `Stop` holds its turn.
    let timer_due = match (&reported, payload["transcriptPath"].as_str()) {
        (Some(Ok(inbox)), Some(path)) => match std::fs::read_to_string(path) {
            Ok(transcript) => timer_due(&transcript, &inbox.senders),
            Err(error) => {
                eprintln!("orx antigravity-gate: could not read agy's transcript {path}: {error}");
                None
            }
        },
        _ => None,
    };
    let started = std::time::Instant::now();
    loop {
        let news = inbox
            .as_deref()
            .is_some_and(|dir| read_inbox(dir).is_ok_and(|inbox| inbox.unread));
        if news || timer_due.is_some_and(|due| crate::store::now_ms() >= due) {
            return json!({"decision": "continue", "reason": "[orx] Background work you started has reported back. Continue the request; end your turn once it is complete."});
        }
        if started.elapsed() >= crate::config::paced(STOP_HOLD_LIMIT) {
            return json!({"decision": "continue", "reason": "[orx] Work you started in the background is still running. Continue with anything else the request needs, or end your turn to keep waiting for it."});
        }
        tokio::time::sleep(crate::config::paced(std::time::Duration::from_secs(1))).await;
    }
}

/// agy answers a turn it gave up on as a normal end of turn, so its error
/// reaches the turn through a file of the chat's.
fn save_stop_error(payload: &Value) {
    let Some(error) = payload
        .get("error")
        .and_then(Value::as_str)
        .filter(|error| payload["terminationReason"] == "ERROR" && !error.trim().is_empty())
    else {
        return;
    };
    let Ok(session) = std::env::var("ORX_SESSION_ID") else {
        return;
    };
    let saved = json!({"conversationId": payload["conversationId"], "error": error});
    if let Err(failure) = std::fs::write(
        crate::local::harness::antigravity::stop_error_path(&session),
        saved.to_string(),
    ) {
        eprintln!("orx antigravity-gate: could not save agy's turn error: {failure}");
    }
}

/// agy's inbox for a conversation: one JSON file per message, and `read.json` naming those already
/// delivered to the model.
struct Inbox {
    unread: bool,
    senders: std::collections::HashSet<String>,
}

fn read_inbox(dir: &std::path::Path) -> Result<Inbox> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Inbox {
                unread: false,
                senders: Default::default(),
            })
        }
        Err(error) => return Err(error.into()),
    };
    let delivered: serde_json::Map<String, Value> = match std::fs::read(dir.join("read.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Default::default(),
        Err(error) => return Err(error.into()),
    };
    let mut inbox = Inbox {
        unread: false,
        senders: Default::default(),
    };
    for entry in entries {
        let path = entry?.path();
        let Some(id) = path
            .file_name()
            .and_then(|name| name.to_str()?.strip_suffix(".json"))
            .filter(|id| *id != "read")
        else {
            continue;
        };
        inbox.unread |= !delivered.contains_key(id);
        // A message agy is still writing parses on a later read.
        if let Some(sender) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|message| message["sender"].as_str().map(str::to_owned))
        {
            inbox.senders.insert(sender);
        }
    }
    Ok(inbox)
}

/// When the earliest of agy's timers that has not yet reported is due, from its transcript: each
/// `schedule` call starts a task stamped with its start and named by the messages it sends.
fn timer_due(transcript: &str, reported: &std::collections::HashSet<String>) -> Option<i64> {
    let mut durations = std::collections::VecDeque::new();
    let mut due: Option<i64> = None;
    for row in transcript
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        for call in row["tool_calls"].as_array().into_iter().flatten() {
            if call["name"] == "schedule" {
                let seconds = &call["args"]["DurationSeconds"];
                durations.push_back(
                    seconds
                        .as_i64()
                        .or_else(|| seconds.as_str()?.trim_matches('"').parse().ok()),
                );
            }
        }
        let content = row["content"].as_str().unwrap_or_default();
        if !content.contains("Task Description: Timer:") {
            continue;
        }
        let seconds = durations.pop_front().flatten();
        let task = content
            .split_once("task id: ")
            .and_then(|(_, rest)| rest.split_whitespace().next());
        let started = row["created_at"]
            .as_str()
            .and_then(crate::local::native_chats::parse_iso_ms);
        if let (Some(seconds), Some(task), Some(started)) = (seconds, task, started) {
            if !reported.contains(task) {
                let at = started + seconds * 1000;
                due = Some(due.map_or(at, |due| due.min(at)));
            }
        }
    }
    due
}

/// A shell command runs with its invocation's native `modelName` exported, so `orx exp run` knows
/// the model that launched it (native `overwrite` merges into the tool's arguments).
fn invocation_overwrite(input: &str) -> Option<Value> {
    if cfg!(windows) {
        return None;
    }
    let payload: Value = serde_json::from_str(input).ok()?;
    if payload.pointer("/toolCall/name")? != "run_command" {
        return None;
    }
    let command = payload.pointer("/toolCall/args/CommandLine")?.as_str()?;
    let identity = crate::store::InvocationIdentity {
        harness: "antigravity".into(),
        model: payload.get("modelName")?.as_str()?.to_string(),
        provider: None,
    };
    let command =
        crate::commands::invocation_gate::with_invocation_context(command, &identity).ok()?;
    Some(json!({ "CommandLine": command }))
}

pub(crate) fn record_post_invocation(
    store: &crate::store::Store,
    payload: &Value,
    execution: Option<&str>,
) -> Result<()> {
    let conversation = payload
        .get("conversationId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing native conversation identity"))?;
    let step = payload
        .get("initialNumSteps")
        .and_then(Value::as_i64)
        .filter(|step| *step >= 0)
        .ok_or_else(|| anyhow!("Missing native invocation step"))?;
    let model = payload
        .get("modelName")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing native invocation model"))?;
    let identity = crate::store::InvocationIdentity {
        harness: "antigravity".into(),
        model: model.to_string(),
        provider: None,
    };
    let planner = invocation_key(payload, conversation, step);
    let key = planner
        .clone()
        .unwrap_or_else(|| format!("antigravity:{conversation}:invocation:{step}"));
    store.record_native_invocation(&key, &identity, Some(conversation))?;
    // Sub-agents never reach the stream: this is their only per-call record. Only a planner row
    // proves the model ran; without one the call may have failed before executing.
    if let (Some(planner), Some(execution)) = (planner, execution) {
        store.record_usage_sample(
            execution,
            &planner,
            "antigravity",
            Some(model),
            None,
            &Default::default(),
        )?;
    }
    Ok(())
}

/// The invocation's own planner step from its native transcript (native steps can be inserted at
/// or after `initialNumSteps`); none flushed yet means no step, never a guessed one.
fn invocation_key(payload: &Value, conversation: &str, step: i64) -> Option<String> {
    let planner = payload
        .get("transcriptPath")
        .and_then(Value::as_str)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|row| row["type"] == "PLANNER_RESPONSE")
                .find_map(|row| row["step_index"].as_i64().filter(|index| *index >= step))
        });
    planner.map(|planner| {
        crate::local::harness::antigravity::invocation_sample_id(conversation, planner)
    })
}

#[cfg(test)]
mod antigravity_tests {
    use super::*;

    #[tokio::test]
    async fn a_stop_with_nothing_in_the_background_ends_the_turn() {
        for payload in [
            json!({"terminationReason": "NO_TOOL_CALL", "fullyIdle": true}),
            json!({"terminationReason": "ERROR", "error": "quota"}),
        ] {
            assert_eq!(background_stop(&payload).await, json!({}));
        }
    }

    #[test]
    fn a_stop_on_an_error_saves_it_for_the_chats_turn() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        std::fs::create_dir_all(crate::store::data_dir()).unwrap();
        std::env::set_var("ORX_SESSION_ID", "chat-error");
        let path = crate::local::harness::antigravity::stop_error_path("chat-error");
        save_stop_error(&json!({"terminationReason": "NO_TOOL_CALL", "error": ""}));
        assert!(!path.exists());
        save_stop_error(
            &json!({"terminationReason": "ERROR", "error": "quota", "conversationId": "c"}),
        );
        let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved, json!({"conversationId": "c", "error": "quota"}));
    }

    /// An agy conversation's artifact directory with `delivered` messages already read and a
    /// transcript of `rows`.
    fn agy_conversation(delivered: &[(&str, &str)], rows: &[Value]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("orx-agy-{}", uuid::Uuid::new_v4()));
        let inbox = dir.join(".system_generated/messages");
        std::fs::create_dir_all(&inbox).unwrap();
        let mut read = serde_json::Map::new();
        for (id, sender) in delivered {
            std::fs::write(
                inbox.join(format!("{id}.json")),
                json!({"id": id, "sender": sender}).to_string(),
            )
            .unwrap();
            read.insert(id.to_string(), json!(true));
        }
        std::fs::write(inbox.join("read.json"), Value::Object(read).to_string()).unwrap();
        let transcript: Vec<String> = rows.iter().map(Value::to_string).collect();
        std::fs::write(dir.join("transcript.jsonl"), transcript.join("\n")).unwrap();
        dir
    }

    fn pending_stop(dir: &std::path::Path) -> Value {
        json!({
            "terminationReason": "NO_TOOL_CALL",
            "fullyIdle": false,
            "artifactDirectoryPath": dir,
            "transcriptPath": dir.join("transcript.jsonl"),
        })
    }

    /// agy's transcript rows for a `schedule` call and the timer task it started.
    fn timer(step: u64, started: &str, seconds: Value) -> [Value; 2] {
        [
            json!({"step_index": step, "type": "PLANNER_RESPONSE", "tool_calls": [{"name": "schedule", "args": {"DurationSeconds": seconds, "Prompt": "\"check\""}}]}),
            json!({"step_index": step + 1, "type": "GENERIC", "status": "RUNNING", "created_at": started, "content": format!("Created At: {started}\nTool is running as a background task with task id: conv/task-{}\nTask Description: Timer: 1m, Prompt: check", step + 1)}),
        ]
    }

    #[tokio::test]
    async fn a_stop_holds_the_turn_until_agys_inbox_has_news() {
        // A daemon from an earlier turn never reports, and its run_command started a task too.
        let dir = agy_conversation(
            &[("earlier", "conv/task-2")],
            &[
                json!({"step_index": 1, "type": "PLANNER_RESPONSE", "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"python3 -m http.server\"", "IsDaemon": "true"}}]}),
                json!({"step_index": 2, "type": "GENERIC", "status": "RUNNING", "created_at": "2026-11-09T20:33:53Z", "content": "Tool is running as a background task with task id: conv/task-2\nTask Description: python3 -m http.server"}),
            ],
        );
        let payload = pending_stop(&dir);
        let hook = tokio::spawn(async move { background_stop(&payload).await });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!hook.is_finished());
        std::fs::write(
            dir.join(".system_generated/messages/report.json"),
            json!({"id": "report", "sender": "conv/task-6", "content": "Command exited with code 0."}).to_string(),
        )
        .unwrap();
        let output = tokio::time::timeout(std::time::Duration::from_secs(10), hook)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output["decision"], "continue");
        assert!(output["reason"].as_str().unwrap().contains("reported back"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_due_timer_ends_the_hold_it_would_otherwise_defer() {
        let dir = agy_conversation(&[], &timer(3, "2026-11-09T20:34:20Z", json!("60")));
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            background_stop(&pending_stop(&dir)),
        )
        .await
        .unwrap();
        assert!(output["reason"].as_str().unwrap().contains("reported back"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_timers_that_have_not_reported_are_due() {
        let rows: Vec<String> = [
            timer(13, "2026-11-09T20:34:20Z", json!("60")),
            timer(20, "2026-11-09T20:40:00Z", json!(300)),
            timer(30, "2026-11-09T20:50:00Z", json!("90")),
        ]
        .iter()
        .flatten()
        .map(Value::to_string)
        .collect();
        let reported = ["conv/task-14".to_string()].into_iter().collect();
        assert_eq!(
            timer_due(&rows.join("\n"), &reported),
            crate::local::native_chats::parse_iso_ms("2026-11-09T20:45:00Z")
        );
        assert_eq!(timer_due("not agy's transcript", &reported), None);
    }

    #[test]
    fn a_stop_without_news_lets_the_agent_decide_after_the_hold_limit() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        std::env::set_var("ORX_TEST_SPEEDUP", "2400");
        let dir = agy_conversation(&[("earlier", "conv/task-2")], &[]);
        let output = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(background_stop(&pending_stop(&dir)));
        assert!(output["reason"].as_str().unwrap().contains("still running"));
    }

    #[test]
    fn shell_commands_carry_their_invocations_native_model() {
        let payload = |tool: &str| {
            json!({"modelName": "gemini-3.8-flash-high", "toolCall": {"name": tool, "args": {"CommandLine": "orx exp run e", "Cwd": "/w"}}})
                .to_string()
        };
        let overwrite = invocation_overwrite(&payload("run_command"));
        if cfg!(windows) {
            assert_eq!(overwrite, None);
            return;
        }
        let command = overwrite.unwrap()["CommandLine"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(command.contains("gemini-3.8-flash-high") && command.ends_with("; orx exp run e"));
        assert_eq!(invocation_overwrite(&payload("view_file")), None);
    }

    #[test]
    fn hooks_key_their_own_planner_and_never_guess_a_step() {
        let path =
            std::env::temp_dir().join(format!("orx-agy-hook-{}.jsonl", uuid::Uuid::new_v4()));
        let rows = [
            (1, "PLANNER_RESPONSE"),
            (3, "SYSTEM_MESSAGE"),
            (4, "PLANNER_RESPONSE"),
        ]
        .map(|(step, kind)| json!({"step_index": step, "type": kind}).to_string());
        std::fs::write(&path, rows.join("\n")).unwrap();
        let payload = json!({"transcriptPath": path});
        assert_eq!(
            invocation_key(&payload, "c", 3).as_deref(),
            Some("antigravity:c:step:4")
        );
        assert_eq!(invocation_key(&payload, "c", 5), None);
        std::fs::remove_file(path).unwrap();
    }
}
