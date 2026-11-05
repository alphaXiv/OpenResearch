use std::sync::OnceLock;
use std::time::Duration;

use futures::TryStreamExt;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::{anyhow, Result};

pub const RESEARCH_TOOLS: &[&str] = &[
    "answer_pdf_queries",
    "get_paper_content",
    "read_files_from_github_repository",
    "discover_papers",
    "find_researchers",
    "get_researcher",
    "get_researcher_papers",
    "resolve_researchers",
    "get_hot_feed",
];
pub const LIBRARY_READ_TOOLS: &[&str] =
    &["list_library", "list_annotated_papers", "get_paper_notes"];
pub const WRITE_TOOLS: &[&str] = &["save_papers_to_folder", "create_folder"];
const SIGNED_OUT: &str = "alphaXiv is signed out on the computer running this chat, so library tools are unavailable. Research tools still work.";
const INSTRUCTIONS: &str = "alphaXiv tools research papers and researchers. Library tools read the user's saved folders, notes and highlights and need alphaXiv sign-in on the computer running this chat. Save papers or create folders only when requested. Want to read, Reading and Completed are folders, not exclusive states. Link papers as https://www.alphaxiv.org/abs/<arXiv id>.";

/// Under the agents' 300 s tool-call timeouts, so this error reaches the agent first.
const CALL_TIMEOUT: Duration = Duration::from_secs(240);
/// Under Codex's 10 s default MCP startup timeout.
const LIST_TIMEOUT: Duration = Duration::from_secs(8);

pub async fn run() -> Result<()> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = receiver.recv().await {
            let line = format!("{message}\n");
            stdout.write_all(line.as_bytes()).await?;
            stdout.flush().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if message.get("id").is_none() {
            continue;
        }
        let sender = sender.clone();
        tokio::spawn(async move {
            let reply = handle(message.clone()).await;
            let list_failed = message["method"] == "tools/list" && reply.get("error").is_some();
            let _ = sender.send(reply);
            if !list_failed {
                return;
            }
            // Agents keep their first tool list; ask them to list again once alphaXiv answers.
            let sender = sender.downgrade();
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                if handle(message.clone()).await.get("error").is_none() {
                    if let Some(sender) = sender.upgrade() {
                        let _ = sender.send(
                            json!({"jsonrpc":"2.0", "method":"notifications/tools/list_changed"}),
                        );
                    }
                    return;
                }
            }
        });
    }
    drop(sender);
    writer.await??;
    Ok(())
}

fn allowed(name: &str, signed_in: bool) -> bool {
    RESEARCH_TOOLS.contains(&name)
        || signed_in && (LIBRARY_READ_TOOLS.contains(&name) || WRITE_TOOLS.contains(&name))
}

async fn handle(message: Value) -> Value {
    let id = message["id"].clone();
    let result = match message["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {"listChanged": true}},
            "serverInfo": {"name": "alphaxiv", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        }),
        Some("ping") => json!({}),
        Some(method @ ("tools/list" | "tools/call")) => {
            let signed_in = crate::alphaxiv::signed_in();
            let listing = method == "tools/list";
            let name = message["params"]["name"].as_str().unwrap_or("");
            if !listing && !allowed(name, signed_in) {
                let error = if allowed(name, true) {
                    SIGNED_OUT
                } else {
                    "Not available in OpenResearch."
                };
                return tool_error(id, error);
            }
            let anonymous_ok = listing || RESEARCH_TOOLS.contains(&name);
            // An expired grant signs this computer out; research still works anonymously.
            let attempt = async {
                match forward(&message, signed_in).await {
                    Err(error)
                        if signed_in
                            && anonymous_ok
                            && error.is::<crate::alphaxiv::NotConnected>() =>
                    {
                        (false, forward(&message, false).await)
                    }
                    reply => (signed_in, reply),
                }
            };
            let timeout = if listing { LIST_TIMEOUT } else { CALL_TIMEOUT };
            let (signed_in, reply) = tokio::time::timeout(timeout, attempt)
                .await
                .unwrap_or_else(|_| (signed_in, Err(anyhow!("The request timed out. Try again."))));
            match reply {
                Ok(mut reply) => {
                    if let Some(tools) = reply
                        .pointer_mut("/result/tools")
                        .and_then(Value::as_array_mut)
                    {
                        tools
                            .retain(|tool| allowed(tool["name"].as_str().unwrap_or(""), signed_in));
                    }
                    return reply;
                }
                Err(error) => {
                    let error = if error.is::<crate::alphaxiv::NotConnected>() {
                        SIGNED_OUT.to_owned()
                    } else {
                        format!("alphaXiv tools are unavailable: {error}")
                    };
                    eprintln!("{error}");
                    if listing {
                        return json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32603,"message":error}});
                    }
                    return tool_error(id, &error);
                }
            }
        }
        _ => {
            return json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"method not found"}})
        }
    };
    json!({"jsonrpc":"2.0", "id":id, "result":result})
}

fn tool_error(id: Value, text: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "result":{"isError":true,"content":[{"type":"text","text":text}]}})
}

async fn forward(message: &Value, signed_in: bool) -> Result<Value> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        crate::net::remote_client()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("http client")
    });
    let request = |api: &str, path: &str| {
        client
            .post(format!("{api}{path}"))
            .header("Accept", "application/json, text/event-stream")
            .json(message)
    };
    let response = if signed_in {
        crate::alphaxiv::send(|api| request(api, "/mcp/v1")).await?
    } else {
        request(&crate::config::alphaxiv_api_url(), "/mcp/v1/anonymous")
            .send()
            .await?
            .error_for_status()?
    };
    if !response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"))
    {
        return Ok(response.json().await?);
    }
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let mut lines = BufReader::new(tokio_util::io::StreamReader::new(stream)).lines();
    let mut data = String::new();
    while let Some(line) = lines.next_line().await? {
        if line.is_empty() {
            if !data.is_empty() {
                let reply: Value = serde_json::from_str(&data)?;
                if reply.get("id") == message.get("id") {
                    return Ok(reply);
                }
                data.clear();
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
            data.push('\n');
        }
    }
    Err(anyhow!("alphaXiv closed the stream before replying."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::HeaderMap, response::IntoResponse, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn proxy_enforces_tools_and_switches_authentication() {
        if !crate::config::in_test_child(&["ALPHAXIV_API_URL", "ALPHAXIV_WEB_URL"]) {
            return;
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let app = Router::new().route("/{*path}", post(move |headers: HeaderMap, uri: axum::http::Uri, body: axum::body::Bytes| {
            let seen = seen.clone();
            async move {
                assert!(!headers.contains_key("origin"));
                if uri.path() == "/auth/oauth2/token" {
                    return (axum::http::StatusCode::BAD_REQUEST, Json(json!({"error":"invalid_grant"}))).into_response();
                }
                let message: Value = serde_json::from_slice(&body).unwrap();
                seen.lock().unwrap().push((uri.path().to_owned(), headers.get("authorization").cloned()));
                let result = if message["method"] == "tools/list" {
                    json!({"tools":[{"name":"discover_papers"},{"name":"list_library"},{"name":"create_folder"},{"name":"delete_folder"}]})
                } else {
                    json!({"isError":true, "content":[{"type":"text","text":"IP rate limit"}]})
                };
                let reply = json!({"jsonrpc":"2.0", "id":message["id"], "result":result});
                if uri.path().ends_with("anonymous") {
                    Json(reply).into_response()
                } else {
                    ([("content-type","text/event-stream")], format!(": heartbeat\r\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\r\n\r\nevent: message\r\ndata: {reply}\r\n\r\n")).into_response()
                }
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        std::env::set_var("ALPHAXIV_API_URL", &api);
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let list = json!({"jsonrpc":"2.0", "id":"tools", "method":"tools/list"});
        let reply = handle(list.clone()).await;
        assert_eq!(
            reply["result"]["tools"],
            json!([{"name":"discover_papers"}])
        );
        let call =
            |name| json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name}});
        let signed_out = handle(call("list_library")).await;
        assert_eq!(signed_out["result"]["content"][0]["text"], SIGNED_OUT);
        assert_eq!(
            handle(call("delete_folder")).await["result"]["isError"],
            true
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(
            handle(call("discover_papers")).await["result"]["content"][0]["text"],
            "IP rate limit"
        );
        let config = crate::config::config_dir();
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join("alphaxiv.json"), json!({"apiUrl":api,"accessToken":"test-access","refreshToken":"test-refresh","expiresAtMs":crate::store::now_ms()+3_600_000}).to_string()).unwrap();
        let reply = handle(list).await;
        assert_eq!(reply["id"], "tools");
        assert_eq!(reply["result"]["tools"].as_array().unwrap().len(), 3);
        assert_eq!(
            handle(call("create_folder")).await["result"]["isError"],
            true
        );
        std::fs::write(
            config.join("alphaxiv.json"),
            json!({"apiUrl":api,"accessToken":"expired","refreshToken":"revoked","expiresAtMs":0})
                .to_string(),
        )
        .unwrap();
        assert_eq!(
            handle(call("discover_papers")).await["result"]["content"][0]["text"],
            "IP rate limit"
        );
        assert!(!crate::alphaxiv::signed_in());
        let calls = calls.lock().unwrap();
        assert_eq!(calls[4], ("/mcp/v1/anonymous".to_owned(), None));
        assert!(calls[..2]
            .iter()
            .all(|(path, auth)| path == "/mcp/v1/anonymous" && auth.is_none()));
        assert!(calls[2..4].iter().all(
            |(path, auth)| path == "/mcp/v1" && auth.as_ref().unwrap() == "Bearer test-access"
        ));
        server.abort();
    }
}
