//! `orx serve` — loopback HTTP/SSE surface over the local run store.
//!
//! The jobs sibling of `opencode serve`: orx owns external-run truth (SQLite
//! store + run-log files) and this daemon makes it observable. Locally a UI
//! hits it directly; on an agent box the api SSH-tunnels to it and re-streams
//! (the same lens pattern as opencode's port 4779).
//!
//! Routes:
//!   GET /health                       {"ok":true}
//!   GET /runs                         run list (newest first)
//!   GET /runs/{id}                    one run
//!   GET /runs/{id}/logs[?offset=N]    raw log bytes from offset
//!   GET /event                        SSE: run.updated + run.log events
//!
//! Hand-rolled HTTP/1.1 on a tokio TcpListener (the login.rs idiom) — no
//! framework dependency for a single-tenant loopback daemon.
//!
//! Auth: loopback bind alone is not a trust boundary against *other local
//! users* on a shared box (this daemon's own doc note above says the api
//! reaches it by SSH-tunneling in — but any other user on that box can also
//! just connect to 127.0.0.1:4790 directly, bypassing the tunnel entirely).
//! `--token`/`ORX_SERVE_TOKEN` closes that gap: when set, every request
//! (`/health` included, for the same reason `orx up --remote` gates its own
//! health route — a liveness probe still discloses the running version) must
//! carry a matching `Authorization: Bearer <token>`, compared as a SHA-256
//! digest in constant time via `token_auth`. See SECURITY.md.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::error::{anyhow, Result};
use crate::local::is_terminal;
use crate::store::{log_path, Store, StoredRun};
use crate::token_auth::{constant_time_eq, digest};

pub async fn run(args: crate::ServeArgs) -> Result<()> {
    let port = args.port.unwrap_or(4790);
    let token = args.token.or_else(|| {
        crate::local::shell_env::var("ORX_SERVE_TOKEN").and_then(|v| v.into_string().ok())
    });
    let auth: Arc<Option<[u8; 32]>> = Arc::new(token.as_deref().map(digest));
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| anyhow!("Could not bind 127.0.0.1:{}: {}", port, e))?;
    eprintln!("orx serve: listening on http://127.0.0.1:{port}");
    if auth.is_none() {
        eprintln!(
            "orx serve: no --token/ORX_SERVE_TOKEN set — any local user who can reach \
             127.0.0.1:{port} can read every run's metadata and logs. Set one on shared hosts."
        );
    }

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(x) => x,
            Err(err) => {
                eprintln!("orx serve: accept failed: {err}");
                continue;
            }
        };
        let auth = auth.clone();
        tokio::spawn(async move {
            if let Err(err) = handle(stream, &auth).await {
                eprintln!("orx serve: request failed: {err}");
            }
        });
    }
}

async fn handle(mut stream: TcpStream, auth: &Option<[u8; 32]>) -> Result<()> {
    // Read the head (requests are header-only GETs; 8 KB is plenty).
    let mut buf = vec![0u8; 8192];
    let mut len = 0;
    loop {
        let n = stream.read(&mut buf[len..]).await?;
        if n == 0 {
            return Ok(());
        }
        len += n;
        if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") || len == buf.len() {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..len]);
    let request_line = head.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    if method != "GET" {
        return respond(
            &mut stream,
            405,
            "application/json",
            b"{\"error\":\"method\"}",
        )
        .await;
    }
    if let Some(expected) = auth {
        let provided = bearer_token(&head).map(digest);
        if !provided.is_some_and(|provided| constant_time_eq(&provided, expected)) {
            return respond(
                &mut stream,
                401,
                "application/json",
                b"{\"error\":\"unauthorized\"}",
            )
            .await;
        }
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };
    let query: HashMap<String, String> = query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    match path {
        // Version-stamped so the api can detect a stale daemon: a serve
        // process outlives binary updates, and an old process speaking an old
        // event format would otherwise pass a bare liveness probe forever.
        "/health" => {
            let body = format!(
                "{{\"ok\":true,\"version\":\"{}\"}}",
                env!("CARGO_PKG_VERSION")
            );
            respond(&mut stream, 200, "application/json", body.as_bytes()).await
        }
        "/runs" => {
            let runs = Store::open()?.list_runs(200)?;
            let body = serde_json::to_vec(&serde_json::json!({ "runs": runs }))?;
            respond(&mut stream, 200, "application/json", &body).await
        }
        "/event" => serve_events(&mut stream).await,
        _ => {
            if let Some(rest) = path.strip_prefix("/runs/") {
                match rest.split_once('/') {
                    None => {
                        let Some(run) = Store::open()?.get_run(rest)? else {
                            return respond(
                                &mut stream,
                                404,
                                "application/json",
                                b"{\"error\":\"not_found\"}",
                            )
                            .await;
                        };
                        let body = serde_json::to_vec(&serde_json::json!({ "run": run }))?;
                        respond(&mut stream, 200, "application/json", &body).await
                    }
                    Some((id, "logs")) => {
                        let offset: u64 = query
                            .get("offset")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        let bytes = read_log_from(id, offset);
                        respond(&mut stream, 200, "text/plain; charset=utf-8", &bytes).await
                    }
                    _ => {
                        respond(
                            &mut stream,
                            404,
                            "application/json",
                            b"{\"error\":\"not_found\"}",
                        )
                        .await
                    }
                }
            } else {
                respond(
                    &mut stream,
                    404,
                    "application/json",
                    b"{\"error\":\"not_found\"}",
                )
                .await
            }
        }
    }
}

/// Extracts the bearer token from a raw HTTP head's `Authorization` header.
/// Case-insensitive on the header name (per RFC 9110); the scheme itself
/// (`Bearer `) is matched literally, matching every client this daemon talks
/// to today.
fn bearer_token(head: &str) -> Option<&str> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("authorization") {
            return None;
        }
        value.trim().strip_prefix("Bearer ")
    })
}

fn read_log_from(run_id: &str, offset: u64) -> Vec<u8> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(log_path(run_id)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if f.seek(SeekFrom::Start(offset)).is_ok() {
        let _ = f.take(4_000_000).read_to_end(&mut out);
    }
    out
}

async fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

/// SSE loop: every 500ms diff the store (status changes → `run.updated`) and
/// each live run's log file (appended bytes → `run.log`). A comment ping every
/// ~15s keeps intermediaries from timing the stream out. Ends when the client
/// disconnects (write fails).
async fn serve_events(stream: &mut TcpStream) -> Result<()> {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
        )
        .await?;

    // Baseline: emit current runs once so a fresh subscriber can render
    // without a separate /runs fetch, then only diffs.
    let mut known: HashMap<String, (String, i64)> = HashMap::new();
    let mut log_offsets: HashMap<String, u64> = HashMap::new();
    {
        let runs = Store::open()?.list_runs(200)?;
        for run in &runs {
            known.insert(run.id.clone(), (run.status.clone(), run.updated_at));
            // Live runs replay their WHOLE log from byte 0 through this stream
            // (chunked per tick), so a subscriber needs no separate backfill
            // fetch and byte offsets make reconnect dedup exact. Terminal runs
            // start at EOF — their history lives in R2 via the run's logKey.
            let start = if is_terminal(&run.status) {
                log_size(&run.id)
            } else {
                0
            };
            log_offsets.insert(run.id.clone(), start);
            write_event(stream, "run.updated", &serde_json::json!({ "run": run })).await?;
        }
    }

    let mut ticks: u32 = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        ticks += 1;
        if ticks.is_multiple_of(30) {
            stream.write_all(b": keep-alive\n\n").await?;
        }

        let runs = match Store::open().and_then(|s| s.list_runs(200)) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for run in &runs {
            let changed = match known.get(&run.id) {
                None => true,
                Some((status, updated)) => *status != run.status || *updated != run.updated_at,
            };
            if changed {
                known.insert(run.id.clone(), (run.status.clone(), run.updated_at));
                write_event(stream, "run.updated", &serde_json::json!({ "run": run })).await?;
            }
            emit_log_delta(stream, run, &mut log_offsets).await?;
        }
    }
}

fn log_size(run_id: &str) -> u64 {
    std::fs::metadata(log_path(run_id))
        .map(|m| m.len())
        .unwrap_or(0)
}

async fn emit_log_delta(
    stream: &mut TcpStream,
    run: &StoredRun,
    offsets: &mut HashMap<String, u64>,
) -> Result<()> {
    let offset = *offsets.entry(run.id.clone()).or_insert(0);
    let size = log_size(&run.id);
    if size <= offset {
        return Ok(());
    }
    let chunk = read_log_from(&run.id, offset);
    offsets.insert(run.id.clone(), offset + chunk.len() as u64);
    // base64, not lossy UTF-8: chunk boundaries are arbitrary byte positions,
    // and exact byte lengths are what lets the client dedup replays.
    use base64::Engine as _;
    write_event(
        stream,
        "run.log",
        &serde_json::json!({
            "runId": run.id,
            "offset": offset,
            "chunkB64": base64::engine::general_purpose::STANDARD.encode(&chunk),
        }),
    )
    .await
}

async fn write_event(stream: &mut TcpStream, event: &str, data: &serde_json::Value) -> Result<()> {
    // SSE data must be newline-free per line; JSON-encode guarantees that.
    let frame = format!("event: {event}\ndata: {data}\n\n");
    stream.write_all(frame.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_extracts_value_case_insensitively() {
        let head = "GET /health HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer secret-token\r\n\r\n";
        assert_eq!(bearer_token(head), Some("secret-token"));

        let head_lower = "GET /health HTTP/1.1\r\nauthorization: Bearer secret-token\r\n\r\n";
        assert_eq!(bearer_token(head_lower), Some("secret-token"));
    }

    #[test]
    fn bearer_token_absent_without_header() {
        let head = "GET /health HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(bearer_token(head), None);
    }

    #[test]
    fn bearer_token_ignores_non_bearer_scheme() {
        let head = "GET /health HTTP/1.1\r\nAuthorization: Basic dXNlcjpwYXNz\r\n\r\n";
        assert_eq!(bearer_token(head), None);
    }

    /// Starts a real `handle()` loop on a loopback socket and returns its
    /// address — exercises the actual accept/parse/auth/respond path used by
    /// `run()`, not a reimplementation of it. Only `/health` is used across
    /// these tests: it needs no `Store`, so it can't collide with the
    /// process-global data dir other tests mutate under the parallel runner.
    async fn spawn_server(auth: Option<[u8; 32]>) -> std::net::SocketAddr {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let auth = Arc::new(auth);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let auth = auth.clone();
                tokio::spawn(async move {
                    let _ = handle(stream, &auth).await;
                });
            }
        });
        addr
    }

    /// Issues one raw HTTP GET and returns (status code, body).
    async fn get(
        addr: std::net::SocketAddr,
        path: &str,
        authorization: Option<&str>,
    ) -> (u16, String) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        let mut request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        if let Some(value) = authorization {
            request.push_str(&format!("Authorization: {value}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap_or(());

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response);
        let status = response
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        let body = response.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
        (status, body)
    }

    #[tokio::test]
    async fn no_token_configured_allows_unauthenticated_requests() {
        let addr = spawn_server(None).await;
        let (status, body) = get(addr, "/health", None).await;
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"));
    }

    #[tokio::test]
    async fn token_configured_rejects_missing_authorization() {
        let addr = spawn_server(Some(digest("s3cret"))).await;
        let (status, _) = get(addr, "/health", None).await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn token_configured_rejects_wrong_token() {
        let addr = spawn_server(Some(digest("s3cret"))).await;
        let (status, _) = get(addr, "/health", Some("Bearer wrong")).await;
        assert_eq!(status, 401);
    }

    #[tokio::test]
    async fn token_configured_accepts_matching_token() {
        let addr = spawn_server(Some(digest("s3cret"))).await;
        let (status, body) = get(addr, "/health", Some("Bearer s3cret")).await;
        assert_eq!(status, 200);
        assert!(body.contains("\"ok\":true"));
    }

    #[tokio::test]
    async fn token_gate_applies_to_every_route_not_just_health() {
        // /runs would touch the global Store; confirm it's rejected before
        // that ever happens rather than exercising the Store-backed path.
        let addr = spawn_server(Some(digest("s3cret"))).await;
        let (status, _) = get(addr, "/runs", None).await;
        assert_eq!(status, 401);
    }
}
