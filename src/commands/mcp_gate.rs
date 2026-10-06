//! `orx mcp-gate` — the plan-mode permission bridge.
//!
//! Hidden from `--help`: it's not a user command, it's the stdio MCP server
//! Claude Code spawns via `--mcp-config` and consults via
//! `--permission-prompt-tool mcp__orx__approve` (see
//! `local::harness::claude::write_mcp_config`). Every permission decision the
//! CLI would have shown as an interactive prompt arrives here as a
//! `tools/call`; we relay it to the running `orx up` over localhost HTTP —
//! which auto-decides by policy or surfaces a card and *blocks until the user
//! answers* — and hand the decision back. That held call is what turns
//! headless plan mode into desktop-style mid-turn approvals.
//!
//! The wire is MCP's stdio transport: newline-delimited JSON-RPC 2.0, the same
//! framing as `local::codex` (this end is the server). Only three methods
//! matter — `initialize`, `tools/list`, `tools/call` — so it's hand-rolled
//! rather than pulling in an MCP crate.
//!
//! Failure posture: never hang Claude and never allow by accident. Any
//! transport or orx-side error answers `deny` with the reason; unknown methods
//! get a JSON-RPC error; a missing env contract exits nonzero (Claude reports
//! the server as failed and plan mode degrades to its default gating).

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::error::{anyhow, Result};

/// Env contract injected by `write_mcp_config` (values ride the MCP server's
/// `env` block, so they survive however Claude spawns us).
struct GateEnv {
    up_port: u16,
    session_id: String,
    token: String,
}

impl GateEnv {
    fn from_env() -> Result<Self> {
        let up_port = std::env::var("ORX_UP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow!("ORX_UP_PORT missing or invalid"))?;
        let session_id =
            std::env::var("ORX_SESSION_ID").map_err(|_| anyhow!("ORX_SESSION_ID missing"))?;
        let token =
            std::env::var("ORX_GATE_TOKEN").map_err(|_| anyhow!("ORX_GATE_TOKEN missing"))?;
        Ok(Self {
            up_port,
            session_id,
            token,
        })
    }
}

pub async fn run() -> Result<()> {
    let env = GateEnv::from_env()?;
    // The long-poll deliberately blocks for as long as the user thinks; only
    // connecting is bounded. (orx up itself times pending cards out.)
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| anyhow!("http client: {e}"))?;

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
                let _ = out_tx.send(reply(
                    id,
                    json!({
                        "tools": [{
                            "name": "approve",
                            "description": "Ask the orx user to approve a tool call",
                            "inputSchema": { "type": "object", "additionalProperties": true },
                        }]
                    }),
                ));
            }
            Some("tools/call") => {
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

/// POST the permission request to `orx up` and return its decision JSON.
/// The response body is the decision verbatim (`{"behavior": ...}`).
async fn relay(http: &reqwest::Client, url: &str, body: Value) -> Result<Value> {
    let resp = http
        .post(url)
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("{e}"))?;
    if !resp.status().is_success() {
        return Err(anyhow!("orx up answered {}", resp.status()));
    }
    resp.json::<Value>().await.map_err(|e| anyhow!("{e}"))
}

pub async fn run_antigravity() -> Result<()> {
    use tokio::io::AsyncReadExt;
    let mut input = String::new();
    tokio::io::stdin().read_to_string(&mut input).await?;
    if let Ok(payload) = serde_json::from_str::<Value>(&input) {
        if payload.get("initialNumSteps").is_some() {
            record_post_invocation(
                &crate::store::Store::open()?,
                &payload,
                std::env::var("ORX_USAGE_EXECUTION_ID").ok().as_deref(),
            )?;
            println!("{{}}");
            return Ok(());
        }
    }
    let mut decision = antigravity_decision(&input).await.unwrap_or_else(|error| {
        json!({"decision": "deny", "reason": format!("OpenResearch approval bridge unavailable: {error}")})
    });
    if decision["decision"] == "allow" {
        if let Some(overwrite) = invocation_overwrite(&input) {
            decision["overwrite"] = overwrite;
        }
    }
    println!("{decision}");
    Ok(())
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

async fn antigravity_decision(input: &str) -> Result<Value> {
    if std::env::var("ORX_AGY_GATE").as_deref() == Ok("bypass") {
        return Ok(json!({"decision": "allow"}));
    }
    let payload: Value = serde_json::from_str(input)?;
    let name = payload
        .pointer("/toolCall/name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("missing tool name"))?;
    let args = payload
        .pointer("/toolCall/args")
        .ok_or_else(|| anyhow!("missing tool arguments"))?;
    let env = GateEnv::from_env()?;
    if workspace_read(name, args, &payload) {
        return Ok(json!({"decision": "allow"}));
    }
    let (tool, input) = crate::local::harness::antigravity::normalize_tool(name, Some(args));
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()?;
    let decision = relay(
        &http,
        &format!("http://127.0.0.1:{}/api/internal/permissions", env.up_port),
        json!({"sessionId":env.session_id,"token":env.token,"toolName":tool,"toolInput":input}),
    )
    .await?;
    Ok(match decision.get("behavior").and_then(Value::as_str) {
        Some("allow") => json!({"decision":"allow"}),
        _ => {
            json!({"decision":"deny", "reason":decision.get("message").and_then(Value::as_str).unwrap_or("Action denied")})
        }
    })
}

fn workspace_read(name: &str, args: &Value, payload: &Value) -> bool {
    let key = match name {
        "view_file" => "AbsolutePath",
        "list_dir" => "DirectoryPath",
        "grep_search" => "SearchPath",
        "find_by_name" => "SearchDirectory",
        _ => return false,
    };
    let Some(path) = args
        .get(key)
        .and_then(Value::as_str)
        .and_then(|path| std::fs::canonicalize(path).ok())
    else {
        return false;
    };
    payload
        .get("workspacePaths")
        .and_then(Value::as_array)
        .is_some_and(|roots| {
            roots
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|root| std::fs::canonicalize(root).ok())
                .any(|root| path.starts_with(root))
        })
}

#[cfg(test)]
mod antigravity_tests {
    use super::*;

    #[test]
    fn only_known_reads_inside_the_workspace_skip_approval() {
        let root = std::env::current_dir().unwrap();
        let payload = json!({"workspacePaths":[root]});
        assert!(workspace_read(
            "view_file",
            &json!({"AbsolutePath":root.join("Cargo.toml")}),
            &payload
        ));
        assert!(!workspace_read(
            "view_file",
            &json!({"AbsolutePath":"/etc/passwd"}),
            &payload
        ));
        assert!(!workspace_read(
            "run_command",
            &json!({"AbsolutePath":root}),
            &payload
        ));
        assert!(!workspace_read("view_file", &json!({}), &payload));
    }

    #[tokio::test]
    async fn bypass_env_var_allows_immediately() {
        std::env::set_var("ORX_AGY_GATE", "bypass");
        let res = antigravity_decision("invalid-json").await.unwrap();
        assert_eq!(res["decision"], "allow");
        std::env::remove_var("ORX_AGY_GATE");
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
