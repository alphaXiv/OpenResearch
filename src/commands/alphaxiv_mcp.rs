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
const SIGNED_OUT: &str = "alphaXiv is signed out on this computer. Ask the user to sign in with alphaXiv from the account menu at the bottom left of OpenResearch, then try again.";
const INSTRUCTIONS: &str = "alphaXiv tools research papers and researchers. Library tools read the user's saved folders, notes and highlights and require alphaXiv sign-in from the account menu at the bottom left of OpenResearch. Save papers or create folders only when requested. Want to read, Reading and Completed are folders, not exclusive states. Link papers as https://www.alphaxiv.org/abs/<arXiv id>.";

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
            let reply = handle(message).await;
            let _ = sender.send(reply);
        });
    }
    drop(sender);
    writer.await??;
    Ok(())
}

async fn handle(message: Value) -> Value {
    let id = message["id"].clone();
    let result = match message["method"].as_str() {
        Some("initialize") => json!({
            "protocolVersion": message["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "alphaxiv", "version": env!("CARGO_PKG_VERSION")},
            "instructions": INSTRUCTIONS,
        }),
        Some("ping") => json!({}),
        Some(method @ ("tools/list" | "tools/call")) => {
            let signed_in = crate::alphaxiv::signed_in();
            if method == "tools/call" {
                let name = message["params"]["name"].as_str().unwrap_or("");
                if (LIBRARY_READ_TOOLS.contains(&name) || WRITE_TOOLS.contains(&name)) && !signed_in
                {
                    return tool_error(id, SIGNED_OUT);
                }
                if !(RESEARCH_TOOLS.contains(&name)
                    || LIBRARY_READ_TOOLS.contains(&name)
                    || WRITE_TOOLS.contains(&name))
                {
                    return tool_error(id, "Not available in OpenResearch.");
                }
            }
            match tokio::time::timeout(Duration::from_secs(300), forward(&message, signed_in))
                .await
                .unwrap_or_else(|_| Err(anyhow!("The request timed out. Try again.")))
            {
                Ok(mut reply) => {
                    if method == "tools/list" {
                        if let Some(tools) = reply
                            .pointer_mut("/result/tools")
                            .and_then(Value::as_array_mut)
                        {
                            tools.retain(|tool| {
                                let name = tool["name"].as_str().unwrap_or("");
                                RESEARCH_TOOLS.contains(&name)
                                    || signed_in
                                        && (LIBRARY_READ_TOOLS.contains(&name)
                                            || WRITE_TOOLS.contains(&name))
                            });
                        }
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
                    if method == "tools/call" {
                        return tool_error(id, &error);
                    }
                    return json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32603,"message":error}});
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
    let request = |api: &str, path: &str| {
        crate::client::http()
            .post(format!("{api}{path}"))
            .header("Accept", "application/json, text/event-stream")
            .json(message)
            .timeout(Duration::from_secs(300))
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
        if std::env::var_os("ORX_MCP_TEST_CHILD").is_none() {
            let root = crate::local::git::TemporaryDirectory::new("orx-mcp-test").unwrap();
            let status = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "commands::alphaxiv_mcp::tests::proxy_enforces_tools_and_switches_authentication", "--nocapture"])
                .env("ORX_MCP_TEST_CHILD", "1")
                .env("XDG_CONFIG_HOME", root.path())
                .status().await.unwrap();
            assert!(status.success());
            return;
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = calls.clone();
        let app = Router::new().route("/{*path}", post(move |headers: HeaderMap, uri: axum::http::Uri, Json(message): Json<Value>| {
            let seen = seen.clone();
            async move {
                assert!(!headers.contains_key("origin"));
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
        let calls = calls.lock().unwrap();
        assert!(calls[..2]
            .iter()
            .all(|(path, auth)| path == "/mcp/v1/anonymous" && auth.is_none()));
        assert!(calls[2..].iter().all(
            |(path, auth)| path == "/mcp/v1" && auth.as_ref().unwrap() == "Bearer test-access"
        ));
        server.abort();
    }
}
