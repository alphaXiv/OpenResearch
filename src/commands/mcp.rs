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

/// The longest one `Stop` waits for commands; the agent then decides whether to
/// keep waiting by ending its turn again.
#[cfg(unix)]
const BACKGROUND_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(50 * 60);

#[derive(Debug, PartialEq)]
#[cfg_attr(not(unix), allow(dead_code))]
enum BackgroundWait {
    /// Commands were running and have all exited.
    Finished,
    /// Commands still run, or none could be seen.
    Unknown,
}

/// agy finishes background work in turns of its own, which never reach its ACP
/// client, so a `Stop` that leaves work running continues the same turn instead:
/// once agy's commands exit or, where they cannot be seen, after a pause that
/// grows with each stop of the turn.
async fn background_stop(payload: &Value) -> Value {
    save_stop_error(payload);
    if payload.get("fullyIdle").and_then(Value::as_bool) != Some(false) {
        return json!({});
    }
    let wait = wait_for_background_commands().await;
    if wait == BackgroundWait::Unknown {
        let stops = payload
            .get("executionNum")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        tokio::time::sleep(crate::config::paced(stop_backoff(stops))).await;
    }
    json!({"decision": "continue", "reason": match wait {
        BackgroundWait::Finished => "[orx] A command you ran in the background has finished. Check its result and continue the request; end your turn once the request is complete.",
        BackgroundWait::Unknown => "[orx] Work you started in the background may still be running. Continue with anything else the request needs, or end your turn to keep waiting for it.",
    }})
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

fn stop_backoff(stops: u64) -> std::time::Duration {
    std::time::Duration::from_secs(15u64 << stops.min(5)).min(std::time::Duration::from_secs(300))
}

#[cfg(unix)]
async fn wait_for_background_commands() -> BackgroundWait {
    let started = std::time::Instant::now();
    let mut seen = false;
    loop {
        match background_commands() {
            Ok(commands) if !commands.is_empty() => seen = true,
            Ok(_) if seen => return BackgroundWait::Finished,
            Ok(_) => return BackgroundWait::Unknown,
            Err(error) => {
                eprintln!("orx antigravity-gate: could not list agy's commands: {error}");
                return BackgroundWait::Unknown;
            }
        }
        if started.elapsed() >= BACKGROUND_WAIT_LIMIT {
            return BackgroundWait::Unknown;
        }
        tokio::time::sleep(crate::config::paced(std::time::Duration::from_secs(1))).await;
    }
}

#[cfg(not(unix))]
async fn wait_for_background_commands() -> BackgroundWait {
    BackgroundWait::Unknown
}

/// agy's running commands. orx starts agy as the leader of its process group,
/// which its hooks share, while agy starts each command as the leader of a group
/// of its own.
#[cfg(unix)]
fn background_commands() -> Result<Vec<u32>> {
    // SAFETY: plain process-identity syscalls.
    let (agy, me) = (unsafe { libc::getpgrp() }, unsafe { libc::getpid() });
    let output = crate::process::command("ps")
        .args(["-ww", "-axo", "pid=,ppid=,pgid=,args="])
        .output()?;
    if !output.status.success() {
        return Err(anyhow!("ps exited with {}", output.status));
    }
    let supervisor = crate::paths::spawnable_exe().ok();
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| background_command(line, agy, me, supervisor.as_deref()))
        .collect())
}

#[cfg(unix)]
fn background_command(
    line: &str,
    agy: libc::pid_t,
    me: libc::pid_t,
    supervisor: Option<&std::path::Path>,
) -> Option<u32> {
    let mut fields = line.split_whitespace();
    let pid: libc::pid_t = fields.next()?.parse().ok()?;
    let ppid: libc::pid_t = fields.next()?.parse().ok()?;
    let pgid: libc::pid_t = fields.next()?.parse().ok()?;
    let command = fields.collect::<Vec<_>>().join(" ");
    (ppid == agy
        && pid == pgid
        && pid != me
        && !crate::local::chat::is_experiment_supervisor(&command, supervisor))
    .then(|| u32::try_from(pid).ok())
    .flatten()
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

    #[test]
    fn stops_back_off_where_commands_cannot_be_seen() {
        let waits: Vec<u64> = (0..8).map(|stops| stop_backoff(stops).as_secs()).collect();
        assert_eq!(waits, [15, 30, 60, 120, 240, 300, 300, 300]);
    }

    #[cfg(unix)]
    #[test]
    fn only_agys_own_command_groups_are_background_work() {
        let supervisor = Some(std::path::Path::new("/opt/orx"));
        let command = |line| background_command(line, 100, 150, supervisor);
        assert_eq!(
            command(" 120   100   120 zsh -c sleep 60; echo done"),
            Some(120)
        );
        // agy's MCP servers, helpers and hooks share its group.
        assert_eq!(command(" 130   100   100 python3 mcp_server.py"), None);
        assert_eq!(command(" 150   100   100 /opt/orx antigravity-gate"), None);
        assert_eq!(command(" 140   120   140 sleep 60"), None);
        assert_eq!(command(" 160   100   160 /opt/orx supervise run-id"), None);
        assert_eq!(command("garbage"), None);
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
