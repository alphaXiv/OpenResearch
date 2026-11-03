//! The signed-in user's alphaXiv library, read with this computer's own alphaXiv
//! tokens: `orx login` signs in at alphaXiv as a native app (RFC 8252) and keeps
//! them owner-only in the config directory.

use std::path::PathBuf;
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config;
use crate::error::{anyhow, Result};

/// alphaXiv's public OAuth client for OpenResearch on the desktop.
const CLIENT_ID: &str = "openresearch-desktop";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct NotConnected;
impl std::fmt::Display for NotConnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("alphaXiv isn't connected. Run `orx login --alphaxiv` to connect it.")
    }
}
impl std::error::Error for NotConnected {}

#[derive(Debug)]
pub struct HttpFailure {
    pub status: reqwest::StatusCode,
    pub message: String,
}
impl std::fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for HttpFailure {}

#[derive(Debug)]
struct InvalidGrant;
impl std::fmt::Display for InvalidGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("alphaXiv didn't accept the sign-in. Try again.")
    }
}
impl std::error::Error for InvalidGrant {}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tokens {
    /// The alphaXiv that issued them, the only one they work at.
    api_url: String,
    access_token: String,
    refresh_token: String,
    expires_at_ms: i64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    id_token: Option<String>,
}

impl Tokens {
    fn issued(api_url: &str, response: TokenResponse) -> Self {
        Self {
            api_url: api_url.to_string(),
            access_token: response.access_token,
            refresh_token: response.refresh_token,
            expires_at_ms: crate::store::now_ms() + response.expires_in * 1000,
        }
    }
}

/// One trip to alphaXiv's sign-in: a loopback redirect, PKCE, and a nonce the
/// id_token must carry back.
pub struct Authorization {
    pub url: String,
    pub state: String,
    api_url: String,
    redirect_uri: String,
    verifier: String,
    nonce: String,
}

impl Authorization {
    pub fn new(port: u16) -> Result<Self> {
        let api_url = config::alphaxiv_api_url();
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let challenge =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier));
        let state = uuid::Uuid::new_v4().to_string();
        let nonce = uuid::Uuid::new_v4().to_string();
        let url = url::Url::parse_with_params(
            &format!("{api_url}/auth/oauth2/authorize"),
            [
                ("response_type", "code"),
                ("client_id", CLIENT_ID),
                ("redirect_uri", redirect_uri.as_str()),
                ("scope", "openid email profile"),
                ("state", state.as_str()),
                ("nonce", nonce.as_str()),
                ("code_challenge", challenge.as_str()),
                ("code_challenge_method", "S256"),
                ("prompt", "select_account"),
            ],
        )?
        .to_string();
        Ok(Self {
            url,
            state,
            api_url,
            redirect_uri,
            verifier,
            nonce,
        })
    }

    /// Redeems the callback's `code` for the tokens to keep and the id_token
    /// OpenResearch signs in with.
    pub async fn redeem(&self, code: &str) -> Result<(Tokens, String)> {
        let mut response = token_request(
            &self.api_url,
            &[
                ("grant_type", "authorization_code"),
                ("code", code),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("code_verifier", self.verifier.as_str()),
            ],
        )
        .await?;
        let id_token = response
            .id_token
            .take()
            .filter(|token| id_token_nonce(token).as_deref() == Some(self.nonce.as_str()))
            .ok_or_else(|| anyhow!("alphaXiv's sign-in didn't match this request. Try again."))?;
        Ok((Tokens::issued(&self.api_url, response), id_token))
    }
}

/// The id_token's `nonce`. Its signature needs no check here: it came straight
/// from alphaXiv's token endpoint (OIDC Core 3.1.3.7), and OpenResearch
/// verifies it before trusting it.
fn id_token_nonce(id_token: &str) -> Option<String> {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(id_token.split('.').nth(1)?)
        .ok()?;
    let claims = serde_json::from_slice::<Value>(&payload).ok()?;
    claims.get("nonce")?.as_str().map(str::to_owned)
}

async fn token_request(api_url: &str, grant: &[(&str, &str)]) -> Result<TokenResponse> {
    // alphaXiv's library routes take only access tokens bound to its API
    // resource, which it names after its MCP endpoint.
    let resource = format!("{api_url}/mcp/v1");
    let mut form = grant.to_vec();
    form.extend([("client_id", CLIENT_ID), ("resource", resource.as_str())]);
    let response = crate::client::http()
        .post(format!("{api_url}/auth/oauth2/token"))
        .form(&form)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| crate::client::transport_error(api_url, e.without_url()))?;

    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|_| anyhow!("alphaXiv returned an incorrect token response."));
    }
    let error = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|body| body.get("error")?.as_str().map(str::to_owned));
    if error.as_deref() == Some("invalid_grant") {
        return Err(InvalidGrant.into());
    }
    Err(anyhow!(
        "alphaXiv sign-in failed ({status}{}).",
        error.map(|e| format!(": {e}")).unwrap_or_default()
    ))
}

fn tokens_path() -> PathBuf {
    config::config_dir().join("alphaxiv.json")
}

pub fn signed_in() -> bool {
    tokens_path().exists()
}

fn load() -> Option<Tokens> {
    serde_json::from_str(&std::fs::read_to_string(tokens_path()).ok()?).ok()
}

fn save(tokens: &Tokens) -> Result<()> {
    std::fs::create_dir_all(config::config_dir())?;
    let body = format!("{}\n", serde_json::to_string_pretty(tokens)?);
    crate::local::git::atomic_write_with_mode(&tokens_path(), body.as_bytes(), Some(0o600))
}

fn remove() -> Result<()> {
    match std::fs::remove_file(tokens_path()) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Held until dropped, across threads and processes: reusing a rotated refresh
/// token makes alphaXiv revoke this user's desktop sign-ins everywhere.
async fn lock() -> Result<std::fs::File> {
    std::fs::create_dir_all(config::config_dir())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config::config_dir().join("alphaxiv.lock"))?;
    Ok(tokio::task::spawn_blocking(move || file.lock().map(|()| file)).await??)
}

/// Keeps `tokens`, and the OpenResearch `credentials` they signed in with, in
/// place of any others, revoking the tokens they replace. On failure neither
/// changes.
pub async fn store(tokens: &Tokens, credentials: Option<&config::Credentials>) -> Result<()> {
    let lock = lock().await?;
    let previous = load();
    save(tokens)?;
    if let Some(credentials) = credentials {
        if let Err(error) = config::save_credentials(credentials).await {
            let _ = previous.as_ref().map_or_else(remove, save);
            return Err(error);
        }
    }
    drop(lock);

    if let Some(previous) = previous {
        revoke(&previous).await;
    }
    Ok(())
}

/// Revokes the refresh token at alphaXiv, best effort.
pub async fn revoke(tokens: &Tokens) {
    let _ = crate::client::http()
        .post(format!("{}/auth/oauth2/revoke", tokens.api_url))
        .form(&[
            ("token", tokens.refresh_token.as_str()),
            ("token_type_hint", "refresh_token"),
            ("client_id", CLIENT_ID),
        ])
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await;
}

/// Forgets the stored tokens, revoking them first.
pub async fn sign_out() -> Result<()> {
    if !tokens_path().exists() {
        return Ok(());
    }
    let _lock = lock().await?;
    if let Some(tokens) = load() {
        revoke(&tokens).await;
    }
    remove()
}

/// The stored tokens, refreshed when the access token is within a minute of
/// expiring or is the one alphaXiv just `rejected`.
async fn fresh_tokens(rejected: Option<String>) -> Result<Tokens> {
    // A task of its own, so a caller that gives up mid-refresh can't drop the
    // rotated refresh token before it is saved.
    tokio::spawn(async move {
        let fresh = |tokens: &Tokens| {
            Some(&tokens.access_token) != rejected.as_ref()
                && tokens.expires_at_ms > crate::store::now_ms() + 60_000
        };
        let tokens = load().ok_or(NotConnected)?;
        if fresh(&tokens) {
            return Ok(tokens);
        }

        let _lock = lock().await?;
        // Another process may have refreshed them while this one waited.
        let tokens = load().ok_or(NotConnected)?;
        if fresh(&tokens) {
            return Ok(tokens);
        }
        let grant = [
            ("grant_type", "refresh_token"),
            ("refresh_token", tokens.refresh_token.as_str()),
        ];
        let refreshed = match token_request(&tokens.api_url, &grant).await {
            Ok(response) => Tokens::issued(&tokens.api_url, response),
            Err(error) if error.is::<InvalidGrant>() => {
                remove()?;
                return Err(NotConnected.into());
            }
            Err(error) => return Err(error),
        };
        save(&refreshed)?;
        Ok(refreshed)
    })
    .await?
}

pub async fn send(request: impl Fn(&str) -> reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let attempt = |tokens: &Tokens| {
        let api_url = tokens.api_url.clone();
        let sent = request(&api_url).bearer_auth(&tokens.access_token).send();
        async move {
            sent.await
                .map_err(|e| crate::client::transport_error(&api_url, e.without_url()))
        }
    };
    let tokens = fresh_tokens(None).await?;
    let mut response = attempt(&tokens).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        let tokens = fresh_tokens(Some(tokens.access_token)).await?;
        response = attempt(&tokens).await?;
    }

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(NotConnected.into());
    }
    if status == reqwest::StatusCode::FORBIDDEN {
        return Err(HttpFailure {
            status,
            message: "alphaXiv denied this request.".into(),
        }
        .into());
    }
    if !status.is_success() {
        let text = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or(text);
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(HttpFailure {
                status,
                message: format!("alphaXiv: {message}"),
            }
            .into());
        }
        return Err(anyhow!("alphaXiv request failed ({status})."));
    }
    Ok(response)
}

async fn get(path: &str, query: &[(&str, &str)]) -> Result<Value> {
    send(|api| {
        crate::client::http()
            .get(format!("{api}{path}"))
            .query(query)
    })
    .await?
    .json()
    .await
    .map_err(|_| anyhow!("alphaXiv returned an incorrect library response."))
}

pub async fn folders() -> Result<Value> {
    let mut folders = get("/folders/v3", &[]).await?;
    if let Some(items) = folders.as_array_mut() {
        for folder in items {
            let count = folder
                .get("papers")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if let Some(object) = folder.as_object_mut() {
                object.remove("papers");
                object.insert("paperCount".into(), count.into());
            }
        }
    }
    Ok(folders)
}

pub async fn papers(
    folder: Option<&str>,
    cursor: Option<&str>,
    search: Option<&str>,
) -> Result<Value> {
    let mut query = vec![("scope", if folder.is_some() { "folder" } else { "all" })];
    if let Some(folder) = folder {
        query.push(("folder", folder));
    }
    if let Some(cursor) = cursor {
        query.push(("cursor", cursor));
    }
    if let Some(search) = search {
        query.push(("search", search));
    }
    let mut page = get("/folders/v4/papers", &query).await?;
    if let Some(papers) = page.get_mut("papers").and_then(Value::as_array_mut) {
        for paper in papers {
            if let Some(id) = paper.get("universalId").and_then(Value::as_str) {
                let url = format!(
                    "{}/abs/{}",
                    config::alphaxiv_web_url().trim_end_matches('/'),
                    id
                );
                paper["url"] = url.into();
            }
        }
    }
    Ok(page)
}

pub async fn print_library(json: bool) -> Result<()> {
    let mut folders = folders().await?;
    let items = folders
        .as_array_mut()
        .ok_or_else(|| anyhow!("alphaXiv returned an incorrect folder list."))?;
    for folder in items {
        let id = folder
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("alphaXiv returned an incorrect folder."))?
            .to_string();
        let mut all = Vec::new();
        let mut cursor = None;
        loop {
            let page = papers(Some(&id), cursor.as_deref(), None).await?;
            all.extend(
                page.get("papers")
                    .and_then(Value::as_array)
                    .ok_or_else(|| anyhow!("alphaXiv returned an incorrect paper list."))?
                    .iter()
                    .cloned(),
            );
            cursor = page
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                break;
            }
        }
        folder["papers"] = all.into();
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&folders)?);
    } else if let Some(items) = folders.as_array() {
        for folder in items {
            println!(
                "{} ({})",
                folder["name"].as_str().unwrap_or(""),
                folder["paperCount"]
            );
            if let Some(papers) = folder["papers"].as_array() {
                for paper in papers {
                    let authors = paper["authors"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    println!(
                        "  {} — {} · {}\n    {}",
                        paper["title"].as_str().unwrap_or(""),
                        authors,
                        paper["publishedAt"].as_str().unwrap_or(""),
                        paper["url"].as_str().unwrap_or("")
                    );
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
        Form, Json, Router,
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Upstream {
        live_refresh: String,
        accepted: String,
        refreshes: usize,
        reused: bool,
        revoked: Vec<HashMap<String, String>>,
    }

    type Shared = State<Arc<Mutex<Upstream>>>;

    async fn token(State(upstream): Shared, Form(form): Form<HashMap<String, String>>) -> Response {
        // Widens the window in which concurrent refreshes would collide.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut upstream = upstream.lock().unwrap();
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(form["client_id"], CLIENT_ID);
        assert!(form["resource"].ends_with("/mcp/v1"));
        if form["refresh_token"] != upstream.live_refresh {
            upstream.reused |= form["refresh_token"] != "dead";
            let body = Json(json!({ "error": "invalid_grant" }));
            return (StatusCode::BAD_REQUEST, body).into_response();
        }
        upstream.refreshes += 1;
        let n = upstream.refreshes;
        upstream.live_refresh = format!("refresh-{n}");
        upstream.accepted = format!("access-{n}");
        Json(json!({
            "access_token": format!("access-{n}"),
            "refresh_token": format!("refresh-{n}"),
            "expires_in": 3600,
            "token_type": "Bearer",
        }))
        .into_response()
    }

    async fn folders(State(upstream): Shared, headers: HeaderMap) -> Response {
        let accepted = format!("Bearer {}", upstream.lock().unwrap().accepted);
        if headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&accepted) {
            Json(json!([])).into_response()
        } else {
            StatusCode::UNAUTHORIZED.into_response()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn stored_tokens_refresh_once_rotate_and_revoke() {
        // Credentials and the config dir are process-global, so the test body
        // runs in a child process with its own config dir.
        if std::env::var("ORX_ALPHAXIV_TEST_CHILD").is_err() {
            let root = std::env::temp_dir().join(format!("orx-alphaxiv-{}", uuid::Uuid::new_v4()));
            let status = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "alphaxiv::tests::stored_tokens_refresh_once_rotate_and_revoke",
                    "--nocapture",
                ])
                .env("ORX_ALPHAXIV_TEST_CHILD", "1")
                .env("XDG_CONFIG_HOME", &root)
                .status()
                .await
                .unwrap();
            let _ = std::fs::remove_dir_all(root);
            assert!(status.success());
            return;
        }

        let upstream = Arc::new(Mutex::new(Upstream {
            live_refresh: "refresh-0".into(),
            accepted: "access-0".into(),
            ..Default::default()
        }));
        let app = Router::new()
            .route("/auth/oauth2/token", post(token))
            .route(
                "/auth/oauth2/revoke",
                post(|State(upstream): Shared, Form(form): Form<HashMap<String, String>>| async move {
                    upstream.lock().unwrap().revoked.push(form);
                }),
            )
            .route("/folders/v3", get(folders))
            .route("/forbidden", get(|| async { StatusCode::FORBIDDEN }))
            .route("/limited", get(|| async { (StatusCode::TOO_MANY_REQUESTS, "Try later") }))
            .with_state(upstream.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let stored = |access: &str, refresh: &str, expires_in_ms: i64| Tokens {
            api_url: api.clone(),
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at_ms: crate::store::now_ms() + expires_in_ms,
        };

        // Within a minute of expiry: refreshed before use, and the rotated
        // refresh token is what's kept.
        save(&stored("access-0", "refresh-0", 30_000)).unwrap();
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));
        assert_eq!(upstream.lock().unwrap().refreshes, 1);
        let kept = load().unwrap();
        assert_eq!(
            (kept.access_token.as_str(), kept.refresh_token.as_str()),
            ("access-1", "refresh-1")
        );

        // Concurrent callers refresh once; the rest wait and use the result.
        save(&stored("access-1", "refresh-1", -1)).unwrap();
        let callers: Vec<_> = (0..8).map(|_| tokio::spawn(fresh_tokens(None))).collect();
        for caller in callers {
            assert_eq!(caller.await.unwrap().unwrap().access_token, "access-2");
        }
        assert_eq!(upstream.lock().unwrap().refreshes, 2);
        assert!(!upstream.lock().unwrap().reused);

        // A token alphaXiv rejects is refreshed once and the request retried.
        upstream.lock().unwrap().accepted = "nobody".into();
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));
        assert_eq!(upstream.lock().unwrap().refreshes, 3);

        // invalid_grant forgets the tokens, which reads as not connected.
        save(&stored("access-x", "dead", -1)).unwrap();
        assert!(super::get("/folders/v3", &[])
            .await
            .unwrap_err()
            .is::<NotConnected>());
        assert!(!tokens_path().exists());
        assert!(super::get("/folders/v3", &[])
            .await
            .unwrap_err()
            .is::<NotConnected>());

        save(&stored("access-3", "refresh-3", 3_600_000)).unwrap();
        let denied = super::get("/forbidden", &[]).await.unwrap_err();
        assert_eq!(
            denied.downcast_ref::<HttpFailure>().unwrap().status,
            StatusCode::FORBIDDEN
        );
        let limited = super::get("/limited", &[]).await.unwrap_err();
        assert_eq!(
            limited.downcast_ref::<HttpFailure>().unwrap().status,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert!(limited.to_string().contains("Try later"));

        // Sign-out revokes the refresh token at alphaXiv and forgets both.
        sign_out().await.unwrap();
        sign_out().await.unwrap();
        assert!(!tokens_path().exists());
        let revoked = upstream.lock().unwrap().revoked.clone();
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0]["token"], "refresh-3");
        assert_eq!(revoked[0]["token_type_hint"], "refresh_token");
        assert_eq!(revoked[0]["client_id"], CLIENT_ID);
        assert!(!upstream.lock().unwrap().reused);
    }
}
