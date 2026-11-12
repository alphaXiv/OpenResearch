//! The signed-in user's alphaXiv library, read with this computer's own alphaXiv
//! tokens: `orx login` signs in at alphaXiv as a native app (RFC 8252), or with a
//! code approved on another device (RFC 8628), and keeps them owner-only in the
//! config directory. A host without its own reads with the access token an
//! attached laptop forwards.

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
const SCOPE: &str = "openid email profile";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// RFC 8628 §3.5: each `slow_down` lengthens the polling interval by this much.
const SLOW_DOWN: Duration = Duration::from_secs(5);

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const PROFILE_TTL: Duration = Duration::from_secs(600);
const MAX_PROFILE_AREAS: usize = 20;
/// A laptop renews the access token it forwards below this much life, so a
/// host's copy lasts at least this long after the laptop goes offline.
const FORWARDED_MIN_LIFE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub enum NotConnected {
    SignedOut,
    /// The access token an attached laptop forwarded expired or was refused.
    ForwardedLapsed,
}
impl std::fmt::Display for NotConnected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::SignedOut => "alphaXiv isn't connected. Run `orx login --alphaxiv` to connect it.",
            Self::ForwardedLapsed => "The alphaXiv sign-in your laptop shared with this computer has expired. Reconnect to this computer from OpenResearch on your laptop, with the laptop online, or run `orx login --alphaxiv` here.",
        })
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

/// alphaXiv's token endpoint refused a grant, with its OAuth error code (RFC 6749 §5.2).
#[derive(Debug)]
struct Refused {
    status: reqwest::StatusCode,
    error: Option<String>,
}
impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.error.as_deref() {
            Some("invalid_grant") => f.write_str("alphaXiv didn't accept the sign-in. Try again."),
            Some(error) => write!(f, "alphaXiv sign-in failed ({}: {error}).", self.status),
            None => write!(f, "alphaXiv sign-in failed ({}).", self.status),
        }
    }
}
impl std::error::Error for Refused {}

fn refusal(error: &crate::error::Error) -> Option<&str> {
    error.downcast_ref::<Refused>()?.error.as_deref()
}

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

/// An access token without the refresh token that renews it: what a laptop
/// forwards to a host it is attached to, which never refreshes it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    api_url: String,
    access_token: String,
    expires_at_ms: i64,
}

impl From<Tokens> for Access {
    fn from(tokens: Tokens) -> Self {
        Self {
            api_url: tokens.api_url,
            access_token: tokens.access_token,
            expires_at_ms: tokens.expires_at_ms,
        }
    }
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
                ("scope", SCOPE),
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

/// A sign-in approved in a browser on any device (RFC 8628), for a computer that
/// alphaXiv can't send a browser back to.
pub struct DeviceAuthorization {
    /// alphaXiv's page that approves it, with the code filled in.
    pub url: String,
    /// What that page shows, so the user can tell it is this sign-in.
    pub code: String,
    api_url: String,
    device_code: String,
    interval: Duration,
    expires_in: Duration,
}

#[derive(Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

impl DeviceAuthorization {
    pub async fn start() -> Result<Self> {
        let api_url = config::alphaxiv_api_url();
        let resource = format!("{api_url}/mcp/v1");
        let response = crate::client::http()
            .post(format!("{api_url}/auth/device/code"))
            .form(&[
                ("client_id", CLIENT_ID),
                ("scope", SCOPE),
                ("resource", resource.as_str()),
            ])
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|e| crate::client::transport_error(&api_url, e.without_url()))?;

        let status = response.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!(
                "alphaXiv at {api_url} doesn't support signing in from another device yet. Sign in with a browser on the computer running orx."
            ));
        }
        if !status.is_success() {
            let error = response
                .json::<Value>()
                .await
                .ok()
                .and_then(|body| body.get("error")?.as_str().map(str::to_owned));
            return Err(Refused { status, error }.into());
        }
        let issued = response
            .json::<DeviceCode>()
            .await
            .ok()
            .filter(|issued| {
                !issued.user_code.is_empty()
                    && issued
                        .user_code
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
            .ok_or_else(|| anyhow!("alphaXiv returned an incorrect sign-in code."))?;

        // The link is shown to the user and opened by the dashboard, so it must lead to alphaXiv.
        let site = url::Url::parse(&config::alphaxiv_web_url())?;
        let bare = |url: &url::Url| {
            url.host_str()
                .map(|host| host.trim_start_matches("www.").to_owned())
        };
        let link = issued
            .verification_uri_complete
            .as_deref()
            .unwrap_or(&issued.verification_uri);
        let url = url::Url::parse(link)
            .ok()
            .filter(|url| url.scheme() == site.scheme() && bare(url) == bare(&site))
            .ok_or_else(|| anyhow!("alphaXiv sent a sign-in link that isn't on {site}."))?;

        Ok(Self {
            url: url.to_string(),
            code: issued.user_code,
            api_url,
            device_code: issued.device_code,
            interval: Duration::from_secs(issued.interval.unwrap_or(5)),
            expires_in: Duration::from_secs(issued.expires_in),
        })
    }

    /// Waits for the user to approve the sign-in, then returns the tokens to keep
    /// and the id_token OpenResearch signs in with.
    pub async fn redeem(&self) -> Result<(Tokens, String)> {
        let expired = || {
            anyhow!("The sign-in code expired before it was approved. Try again for a new code.")
        };
        let deadline = tokio::time::Instant::now() + config::paced(self.expires_in);
        let mut interval = self.interval;
        loop {
            tokio::time::sleep(config::paced(interval)).await;
            let grant = [
                ("grant_type", DEVICE_GRANT),
                ("device_code", self.device_code.as_str()),
            ];
            let error = match token_request(&self.api_url, &grant).await {
                Ok(mut response) => {
                    let id_token = response.id_token.take().ok_or_else(|| {
                        anyhow!("alphaXiv's sign-in didn't say who signed in. Try again.")
                    })?;
                    return Ok((Tokens::issued(&self.api_url, response), id_token));
                }
                Err(error) => error,
            };
            match refusal(&error) {
                // alphaXiv rules on a late approval; the deadline only ends a code it never settles.
                Some("authorization_pending" | "slow_down")
                    if tokio::time::Instant::now() >= deadline =>
                {
                    return Err(expired())
                }
                Some("authorization_pending") => {}
                Some("slow_down") => interval += SLOW_DOWN,
                Some("access_denied") => {
                    return Err(anyhow!(
                        "The sign-in was denied on alphaXiv. Try again to sign in."
                    ))
                }
                Some("expired_token") => return Err(expired()),
                _ => return Err(error),
            }
        }
    }
}

/// The id_token's `nonce`. Its signature needs no check here: it came straight
/// from alphaXiv's token endpoint (OIDC Core 3.1.3.7), and OpenResearch
/// verifies it before trusting it.
fn id_token_nonce(id_token: &str) -> Option<String> {
    jwt_claim(id_token, "nonce")
}

fn jwt_claim(token: &str, name: &str) -> Option<String> {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(token.split('.').nth(1)?)
        .ok()?;
    let claims = serde_json::from_slice::<Value>(&payload).ok()?;
    claims.get(name)?.as_str().map(str::to_owned)
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
    Err(Refused { status, error }.into())
}

fn tokens_path() -> PathBuf {
    config::config_dir().join("alphaxiv.json")
}

fn forwarded_path() -> PathBuf {
    config::config_dir().join("alphaxiv-forwarded.json")
}

/// Own or forwarded; a forwarded token that lapsed still counts, so agents
/// don't restart each time a laptop goes offline.
pub fn signed_in() -> bool {
    tokens_path().exists() || forwarded_path().exists()
}

fn load() -> Option<Tokens> {
    serde_json::from_str(&std::fs::read_to_string(tokens_path()).ok()?).ok()
}

fn save(tokens: &Tokens) -> Result<()> {
    std::fs::create_dir_all(config::config_dir())?;
    let body = format!("{}\n", serde_json::to_string_pretty(tokens)?);
    crate::local::git::atomic_write_with_mode(&tokens_path(), body.as_bytes(), Some(0o600))
}

fn remove(path: PathBuf) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Held until dropped, across threads and processes: reusing a rotated refresh
/// token makes alphaXiv revoke this user's desktop sign-ins everywhere.
async fn lock() -> Result<crate::file_lock::FileLock> {
    std::fs::create_dir_all(config::config_dir())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(config::config_dir().join("alphaxiv.lock"))?;
    Ok(tokio::task::spawn_blocking(move || {
        file.lock().map(|()| crate::file_lock::FileLock::new(file))
    })
    .await??)
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
            let _ = previous
                .as_ref()
                .map_or_else(|| remove(tokens_path()), save);
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
    remove(tokens_path())
}

/// Keeps the access token an attached laptop forwarded, as sent, so fields a
/// newer laptop adds survive. It is only ever sent to this computer's alphaXiv.
pub fn keep_forwarded(body: &[u8]) -> Result<()> {
    let rejected = |message: String| HttpFailure {
        status: reqwest::StatusCode::BAD_REQUEST,
        message,
    };
    let access = serde_json::from_slice::<Access>(body)
        .ok()
        .filter(|access| {
            !access.access_token.is_empty()
                && access.access_token.bytes().all(|b| b.is_ascii_graphic())
        })
        .ok_or_else(|| rejected("The forwarded alphaXiv sign-in is malformed.".into()))?;
    let here = config::alphaxiv_api_url();
    if access.api_url != here {
        return Err(rejected(format!(
            "This computer uses alphaXiv at {here}, not {}.",
            access.api_url
        ))
        .into());
    }
    std::fs::create_dir_all(config::config_dir())?;
    crate::local::git::atomic_write_with_mode(&forwarded_path(), body, Some(0o600))
}

pub fn forget_forwarded() -> Result<()> {
    remove(forwarded_path())
}

/// This computer's own access token for a host it is attached to, renewed
/// early; `None` while signed out.
pub async fn forwardable() -> Result<Option<Access>> {
    let min_life_ms = config::paced(FORWARDED_MIN_LIFE).as_millis() as i64;
    match fresh_tokens(None, min_life_ms).await {
        Ok(tokens) => Ok(Some(tokens.into())),
        Err(error) if error.is::<NotConnected>() => Ok(None),
        Err(error) => Err(error),
    }
}

/// This computer's own sign-in, else the access token an attached laptop
/// forwarded, which only that laptop renews.
async fn access(rejected: Option<String>) -> Result<Access> {
    if tokens_path().exists() {
        return Ok(fresh_tokens(rejected, 60_000).await?.into());
    }
    let path = forwarded_path();
    let body = match std::fs::read(&path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(NotConnected::SignedOut.into())
        }
        Err(error) => {
            return Err(anyhow!("Could not read {}: {error}", path.display()));
        }
    };
    let forwarded: Access = serde_json::from_slice(&body).map_err(|error| {
        anyhow!(
            "{} is damaged ({error}). Delete it, or run `orx login --alphaxiv` here.",
            path.display()
        )
    })?;
    if Some(&forwarded.access_token) == rejected.as_ref()
        || forwarded.expires_at_ms <= crate::store::now_ms()
    {
        return Err(NotConnected::ForwardedLapsed.into());
    }
    Ok(forwarded)
}

/// The stored tokens, refreshed when the access token has under `min_life_ms`
/// left or is the one alphaXiv just `rejected`.
async fn fresh_tokens(rejected: Option<String>, min_life_ms: i64) -> Result<Tokens> {
    // A task of its own, so a caller that gives up mid-refresh can't drop the
    // rotated refresh token before it is saved.
    tokio::spawn(async move {
        let fresh = |tokens: &Tokens| {
            Some(&tokens.access_token) != rejected.as_ref()
                && tokens.expires_at_ms > crate::store::now_ms() + min_life_ms
        };
        let tokens = load().ok_or(NotConnected::SignedOut)?;
        if fresh(&tokens) {
            return Ok(tokens);
        }

        let _lock = lock().await?;
        // Another process may have refreshed them while this one waited.
        let tokens = load().ok_or(NotConnected::SignedOut)?;
        if fresh(&tokens) {
            return Ok(tokens);
        }
        let grant = [
            ("grant_type", "refresh_token"),
            ("refresh_token", tokens.refresh_token.as_str()),
        ];
        let refreshed = match token_request(&tokens.api_url, &grant).await {
            Ok(response) => Tokens::issued(&tokens.api_url, response),
            Err(error) if refusal(&error) == Some("invalid_grant") => {
                remove(tokens_path())?;
                return Err(NotConnected::SignedOut.into());
            }
            Err(error) => return Err(error),
        };
        save(&refreshed)?;
        Ok(refreshed)
    })
    .await?
}

pub async fn send(request: impl Fn(&str) -> reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let attempt = |access: &Access| {
        let api_url = access.api_url.clone();
        let sent = request(&api_url).bearer_auth(&access.access_token).send();
        async move {
            sent.await
                .map_err(|e| crate::client::transport_error(&api_url, e.without_url()))
        }
    };
    let first = access(None).await?;
    let mut response = attempt(&first).await?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        response = attempt(&access(Some(first.access_token)).await?).await?;
    }

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(NotConnected::SignedOut.into());
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
    .map_err(|_| anyhow!("alphaXiv returned an unexpected response."))
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

async fn user_id() -> Result<String> {
    let access = access(None).await?;
    jwt_claim(&access.access_token, "sub")
        .ok_or_else(|| anyhow!("alphaXiv sent an access token without a user."))
}

/// The signed-in user's avatar, preferring the thumbnail.
pub async fn me() -> Result<Value> {
    let user = get(&format!("/users/v3/{}", user_id().await?), &[]).await?;
    let avatars = user.get("avatar").and_then(Value::as_array);
    let url = |kind: &str| {
        avatars?
            .iter()
            .find(|avatar| avatar.get("type").and_then(Value::as_str) == Some(kind))?
            .get("url")?
            .as_str()
    };
    Ok(serde_json::json!({ "avatarUrl": url("thumbnail").or_else(|| url("full_size")) }))
}

/// The signed-in researcher's public alphaXiv profile, remembered briefly so
/// reopening an empty chat neither waits on alphaXiv nor changes its brief.
/// alphaXiv has no route for the caller's own profile, so this reads the public
/// one the token's `sub` names.
pub async fn research_profile() -> Result<crate::telemetry::ResearchProfile> {
    type Memo = Option<(
        String,
        std::time::Instant,
        crate::telemetry::ResearchProfile,
    )>;
    static MEMO: std::sync::Mutex<Memo> = std::sync::Mutex::new(None);
    let id = user_id().await?;
    let cached = MEMO
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|(user, at, _)| *user == id && at.elapsed() < PROFILE_TTL)
        .map(|(_, _, profile)| profile.clone());
    if let Some(profile) = cached {
        return Ok(profile);
    }
    let (user_path, topics_path) = (
        format!("/users/v3/{id}"),
        format!("/users/v3/{id}/following/topics"),
    );
    let (user, topics) = tokio::join!(get(&user_path, &[]), get(&topics_path, &[]));
    let user = user?;
    let text = |value: &Value, key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let slug = text(&user, "researcherSlug").filter(|slug| {
        slug.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    let card = match &slug {
        Some(slug) => get(&format!("/researchers/v1/cards/{slug}"), &[])
            .await
            .ok(),
        None => None,
    };
    let complete = topics.is_ok() && (slug.is_none() || card.is_some());
    let areas = card
        .as_ref()
        .and_then(|card| card.get("researchAreas"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    let followed = topics.ok();
    let followed = followed
        .as_ref()
        .and_then(|topics| topics.get("topics"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|topic| topic.get("topic")?.as_str());
    let mut research_areas = Vec::<String>::new();
    for area in areas.chain(followed) {
        if research_areas.len() < MAX_PROFILE_AREAS && !research_areas.iter().any(|a| a == area) {
            research_areas.push(area.to_owned());
        }
    }
    let card_text = |key| card.as_ref().and_then(|card| text(card, key));
    let background = [
        card_text("headline"),
        text(&user, "institution").or_else(|| card_text("affiliation")),
        text(&user, "biography").or_else(|| card_text("bio")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    let profile = crate::telemetry::ResearchProfile {
        research_areas,
        background: (!background.is_empty()).then_some(background),
        ..Default::default()
    };
    // A partial answer is used once but not remembered, so a blip doesn't stick.
    if complete {
        *MEMO.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((id, std::time::Instant::now(), profile.clone()));
    }
    Ok(profile)
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
        if !crate::config::in_test_child(&["ALPHAXIV_API_URL", "ALPHAXIV_WEB_URL"]) {
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
            .route(
                "/users/v3/u1",
                get(|| async {
                    Json(json!({
                        "researcherSlug": "ada",
                        "institution": "MIT",
                        "biography": " ",
                        "avatar": [
                            { "type": "full_size", "url": "https://cdn/full.png" },
                            { "type": "thumbnail", "url": "https://cdn/thumb.png" },
                        ],
                    }))
                }),
            )
            .route(
                "/users/v3/u1/following/topics",
                get(|| async { Json(json!({ "topics": [{ "id": "t", "topic": "RL" }, { "id": "t2", "topic": "Robotics" }] })) }),
            )
            .route(
                "/researchers/v1/cards/ada",
                get(|| async { Json(json!({ "researchAreas": ["Robotics"], "headline": "PhD student", "bio": "Learns to walk." })) }),
            )
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
        let callers: Vec<_> = (0..8)
            .map(|_| tokio::spawn(fresh_tokens(None, 60_000)))
            .collect();
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

        // The profile is the public one the token's `sub` names.
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"sub":"u1"}"#);
        save(&stored(&format!("h.{claims}.s"), "refresh-3", 3_600_000)).unwrap();
        let profile = research_profile().await.unwrap();
        assert_eq!(profile.research_areas, ["Robotics", "RL"]);
        assert_eq!(
            profile.background.as_deref(),
            Some("PhD student\nMIT\nLearns to walk.")
        );
        assert_eq!(
            me().await.unwrap(),
            json!({ "avatarUrl": "https://cdn/thumb.png" })
        );

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

    #[test]
    fn forwarded_access_matches_its_contract() {
        let stored: Vec<Value> =
            serde_json::from_str(include_str!("../tests/contracts/alphaxiv-forwarded.json"))
                .unwrap();
        for row in stored {
            let access: Access = serde_json::from_value(row.clone()).unwrap();
            assert_eq!(serde_json::to_value(&access).unwrap(), row);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_forwarded_token_is_used_until_it_lapses_and_never_refreshed() {
        if !crate::config::in_test_child(&["ALPHAXIV_API_URL", "ORX_TEST_SPEEDUP"]) {
            return;
        }

        let upstream = Arc::new(Mutex::new(Upstream {
            live_refresh: "refresh-0".into(),
            accepted: "forwarded-1".into(),
            ..Default::default()
        }));
        let app = Router::new()
            .route("/auth/oauth2/token", post(token))
            .route("/folders/v3", get(folders))
            .with_state(upstream.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        std::env::set_var("ALPHAXIV_API_URL", &api);
        let forwarded = |access: &str, expires_in_ms: i64| {
            json!({
                "apiUrl": api,
                "accessToken": access,
                "expiresAtMs": crate::store::now_ms() + expires_in_ms,
                "addedByANewerLaptop": true,
            })
            .to_string()
        };
        let refused = |body: String| {
            let error = keep_forwarded(body.as_bytes()).unwrap_err();
            error.downcast_ref::<HttpFailure>().unwrap().status
        };

        // A laptop's token reads the library on a host with no sign-in of its own.
        assert!(!signed_in());
        assert!(super::get("/folders/v3", &[])
            .await
            .unwrap_err()
            .is::<NotConnected>());
        let body = forwarded("forwarded-1", 3_600_000);
        keep_forwarded(body.as_bytes()).unwrap();
        assert!(signed_in());
        assert_eq!(std::fs::read_to_string(forwarded_path()).unwrap(), body);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(forwarded_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));

        // Another laptop's push replaces it; a token for another alphaXiv, or
        // one that can't be a header, is refused and changes nothing.
        upstream.lock().unwrap().accepted = "forwarded-2".into();
        keep_forwarded(forwarded("forwarded-2", 3_600_000).as_bytes()).unwrap();
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));
        let elsewhere = forwarded("x", 3_600_000).replace(&api, "https://alphaxiv.invalid");
        assert_eq!(refused(elsewhere), reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(
            refused(forwarded("two words", 3_600_000)),
            reqwest::StatusCode::BAD_REQUEST
        );
        assert_eq!(refused("{}".into()), reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));

        // This computer's own sign-in wins.
        save(&Tokens {
            api_url: api.clone(),
            access_token: "own".into(),
            refresh_token: "refresh-0".into(),
            expires_at_ms: crate::store::now_ms() + 3_600_000,
        })
        .unwrap();
        upstream.lock().unwrap().accepted = "own".into();
        assert_eq!(super::get("/folders/v3", &[]).await.unwrap(), json!([]));
        remove(tokens_path()).unwrap();

        // Expired or refused, it asks for the laptop, and nothing is refreshed,
        // created or forgotten: agents keep their tools until the laptop returns.
        keep_forwarded(forwarded("forwarded-3", -1).as_bytes()).unwrap();
        let lapsed = super::get("/folders/v3", &[]).await.unwrap_err();
        assert!(matches!(
            lapsed.downcast_ref::<NotConnected>(),
            Some(NotConnected::ForwardedLapsed)
        ));
        assert!(lapsed.to_string().contains("laptop"));
        keep_forwarded(forwarded("forwarded-4", 3_600_000).as_bytes()).unwrap();
        assert!(matches!(
            super::get("/folders/v3", &[])
                .await
                .unwrap_err()
                .downcast_ref::<NotConnected>(),
            Some(NotConnected::ForwardedLapsed)
        ));
        assert_eq!(upstream.lock().unwrap().refreshes, 0);
        assert!(!tokens_path().exists());
        assert!(signed_in());

        // A laptop forwards only its own sign-in, renewed while under ten minutes remain.
        assert!(forwardable().await.unwrap().is_none());
        save(&Tokens {
            api_url: api.clone(),
            access_token: "access-0".into(),
            refresh_token: "refresh-0".into(),
            expires_at_ms: crate::store::now_ms() + 5 * 60_000,
        })
        .unwrap();
        let renewed = forwardable().await.unwrap().unwrap();
        assert_eq!(renewed.access_token, "access-1");
        assert_eq!(
            forwardable().await.unwrap().unwrap().access_token,
            "access-1"
        );
        assert_eq!(upstream.lock().unwrap().refreshes, 1);

        forget_forwarded().unwrap();
        forget_forwarded().unwrap();
        remove(tokens_path()).unwrap();
        assert!(!signed_in());

        // A file that can't be read is reported with its path, not as signed out.
        std::fs::write(forwarded_path(), "{").unwrap();
        let damaged = super::get("/folders/v3", &[]).await.unwrap_err();
        assert!(!damaged.is::<NotConnected>());
        assert!(damaged.to_string().contains("alphaxiv-forwarded.json"));
    }
}
