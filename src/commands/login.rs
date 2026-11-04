//! The `login` command: one browser trip to alphaXiv, which signs in to
//! OpenResearch and keeps alphaXiv tokens for the library.

use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::browser::open_browser;
use crate::config::{default_api_url, load_credentials, Credentials};
use crate::error::{anyhow, Result};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub async fn run(args: crate::LoginArgs) -> Result<()> {
    let (api_url, linking) = if args.alphaxiv {
        let creds = load_credentials()
            .await?
            .ok_or_else(|| anyhow!("Not logged in. Run `orx login` first."))?;
        (creds.api_url, Some(creds.token))
    } else {
        (args.api_url.unwrap_or_else(default_api_url), None)
    };

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let authorization = crate::alphaxiv::Authorization::new(listener.local_addr()?.port())?;
    println!("Opening your browser to sign in with alphaXiv\u{2026}");
    println!("If it doesn't open, visit:\n  {}\n", authorization.url);
    open_browser(&authorization.url, None);

    tokio::time::timeout(
        LOGIN_TIMEOUT,
        complete(&listener, &authorization, api_url, linking),
    )
    .await
    .map_err(|_| anyhow!("Sign-in timed out after 15 minutes."))??;
    if args.alphaxiv {
        println!("\u{2713} alphaXiv connected.");
        return Ok(());
    }
    println!("\u{2713} Logged in. Credentials saved.");
    if args.non_interactive {
        return Ok(());
    }

    // Offer to install the `orx` skill shim into any coding agent already set up
    // here, so it auto-discovers how to drive the CLI. Consent-gated and
    // best-effort: writes nothing without a yes, never fails login over it.
    crate::commands::install_skills::offer_install_after_login().await;

    Ok(())
}

/// Waits for alphaXiv to send the browser back, signs in to OpenResearch with
/// the id_token, or links it to the account whose personal access token is
/// `linking`, keeps both credentials, and tells the browser how it went.
async fn complete(
    listener: &TcpListener,
    authorization: &crate::alphaxiv::Authorization,
    api_url: String,
    linking: Option<String>,
) -> Result<Credentials> {
    let (mut browser, code) = accept_callback(listener, &authorization.state).await?;
    let finished = async {
        let (tokens, id_token) = authorization.redeem(&code).await?;
        let kept: Result<Credentials> = async {
            match linking {
                Some(token) => {
                    let creds = Credentials { api_url, token };
                    crate::client::link_alphaxiv(&creds, &id_token).await?;
                    crate::alphaxiv::store(&tokens, None).await?;
                    Ok(creds)
                }
                None => {
                    let token = crate::client::alphaxiv_desktop_token(&api_url, &id_token).await?;
                    let creds = Credentials { api_url, token };
                    crate::alphaxiv::store(&tokens, Some(&creds)).await?;
                    Ok(creds)
                }
            }
        }
        .await;
        if kept.is_err() {
            crate::alphaxiv::revoke(&tokens).await;
        }
        kept
    }
    .await;

    let _ = match &finished {
        Ok(_) => {
            let alpha_url = crate::config::alphaxiv_web_url()
                .replace('&', "&amp;")
                .replace('"', "&quot;")
                .replace('<', "&lt;");
            let page = format!(
                r#"<!doctype html>
<meta charset="utf-8">
<title>OpenResearch</title>
<body style="font-family:system-ui;display:grid;place-items:center;height:100vh;margin:0">
<div style="text-align:center">
<h1>You're signed in</h1>
<p>You can close this tab.</p>
<p><a href="openresearch://">Open OpenResearch</a></p>
<p><a rel="noreferrer" href="{alpha_url}">Open alphaXiv</a></p>
</div></body>"#
            );
            write_response(&mut browser, "200 OK", "text/html", &page).await
        }
        Err(error) => {
            let page = format!("Sign-in didn't finish: {error}\nYou can close this tab.");
            write_response(&mut browser, "400 Bad Request", "text/plain", &page).await
        }
    };
    finished
}

/// Accepts connections until one hits `/callback`, then returns it, unanswered,
/// with its `code` once `state` matches and it carries no `error`. Other paths
/// get a 404 and we keep waiting.
async fn accept_callback(listener: &TcpListener, state: &str) -> Result<(TcpStream, String)> {
    loop {
        let (stream, _) = listener.accept().await?;
        if let Some(result) = handle_connection(stream, state).await {
            return result;
        }
    }
}

/// `None` for non-callback requests so the caller keeps listening.
async fn handle_connection(
    mut stream: TcpStream,
    state: &str,
) -> Option<Result<(TcpStream, String)>> {
    let request_line = match read_request_line(&mut stream).await {
        Ok(line) => line,
        Err(_) => return None,
    };

    // Request line: "METHOD <target> HTTP/1.1"
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target, ""),
    };

    if path != "/callback" {
        let _ = write_response(&mut stream, "404 Not Found", "text/plain", "").await;
        return None;
    }

    let mut params: HashMap<String, String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| {
            let decoded = urlencoding::decode(value)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| value.to_string());
            (key.to_string(), decoded)
        })
        .collect();

    let state_matches = params.get("state").map(String::as_str) == Some(state);
    if let (true, Some(error)) = (state_matches, params.get("error")) {
        let _ = write_response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            "Sign-in didn't finish. You can close this tab.",
        )
        .await;
        return Some(Err(anyhow!("Sign-in didn't finish ({error}).")));
    }
    match params.remove("code").filter(|code| !code.is_empty()) {
        Some(code) if state_matches => Some(Ok((stream, code))),
        _ => {
            let _ = write_response(
                &mut stream,
                "400 Bad Request",
                "text/plain",
                "Invalid login response.",
            )
            .await;
            Some(Err(anyhow!(
                "Login failed: invalid or mismatched response from the server."
            )))
        }
    }
}

/// Reads the request head through its blank line and returns the request line.
/// Windows resets a socket closed with unread bytes, so the browser would show
/// an error instead of the page.
async fn read_request_line(stream: &mut TcpStream) -> Result<String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && !head.ends_with(b"\n\n") && head.len() <= 16384 {
        if stream.read(&mut byte).await? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head);
    Ok(head.lines().next().unwrap_or_default().to_owned())
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::post,
        Form, Json, Router,
    };
    use base64::Engine as _;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::sync::{Arc, Mutex};

    async fn callback(target: &str) -> (Result<String>, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = tokio::spawn(async move { accept_callback(&listener, "expected").await });
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream
            .write_all(format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let result = accepted.await.unwrap().map(|(_, code)| code);
        let mut page = String::new();
        stream.read_to_string(&mut page).await.unwrap();
        (result, page)
    }

    #[tokio::test]
    async fn callback_requires_state_code_and_no_error() {
        let (result, _) = callback("/callback?code=c%2B1&state=expected").await;
        assert_eq!(result.unwrap(), "c+1");

        for target in ["/callback?code=c&state=wrong", "/callback?state=expected"] {
            let (result, page) = callback(target).await;
            assert!(result.unwrap_err().to_string().contains("mismatched"));
            assert!(page.starts_with("HTTP/1.1 400"));
        }

        let (result, page) = callback("/callback?state=expected&error=access_denied").await;
        assert!(result.unwrap_err().to_string().contains("access_denied"));
        assert!(page.starts_with("HTTP/1.1 400"));
    }

    #[derive(Default)]
    struct Upstream {
        redirect_uri: String,
        challenge: String,
        nonce: String,
        issued: usize,
        revoked: Vec<String>,
        linked_by: Vec<String>,
        desktop_failure: Option<(StatusCode, &'static str)>,
        link_failure: Option<(StatusCode, &'static str)>,
    }

    type Shared = State<Arc<Mutex<Upstream>>>;

    fn claims(id_token: &str) -> Value {
        let payload = id_token.split('.').nth(1).unwrap();
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn token(State(upstream): Shared, Form(form): Form<HashMap<String, String>>) -> Response {
        let mut upstream = upstream.lock().unwrap();
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(&form["code_verifier"]));
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["client_id"], "openresearch-desktop");
        assert_eq!(form["code"], "code-1");
        assert_eq!(form["redirect_uri"], upstream.redirect_uri);
        assert_eq!(challenge, upstream.challenge);
        assert!(form["resource"].ends_with("/mcp/v1"));
        upstream.issued += 1;
        let n = upstream.issued;
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(json!({ "sub": "ax-1", "nonce": upstream.nonce }).to_string());
        Json(json!({
            "access_token": format!("access-{n}"),
            "refresh_token": format!("refresh-{n}"),
            "expires_in": 3600,
            "token_type": "Bearer",
            "id_token": format!("e30.{claims}.sig"),
        }))
        .into_response()
    }

    async fn revoke(State(upstream): Shared, Form(form): Form<HashMap<String, String>>) {
        assert_eq!(form["client_id"], "openresearch-desktop");
        assert_eq!(form["token_type_hint"], "refresh_token");
        upstream.lock().unwrap().revoked.push(form["token"].clone());
    }

    async fn desktop(State(upstream): Shared, Json(body): Json<Value>) -> Response {
        assert_eq!(claims(body["idToken"].as_str().unwrap())["sub"], "ax-1");
        if let Some((status, error)) = upstream.lock().unwrap().desktop_failure {
            return (status, Json(json!({ "error": error }))).into_response();
        }
        Json(json!({ "token": "pat-1" })).into_response()
    }

    async fn link(
        State(upstream): Shared,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Response {
        assert_eq!(claims(body["idToken"].as_str().unwrap())["sub"], "ax-1");
        let mut upstream = upstream.lock().unwrap();
        let bearer = headers["authorization"].to_str().unwrap().to_string();
        upstream.linked_by.push(bearer);
        if let Some((status, error)) = upstream.link_failure {
            return (status, Json(json!({ "error": error }))).into_response();
        }
        StatusCode::NO_CONTENT.into_response()
    }

    /// Plays the browser: alphaXiv sends it back to the loopback redirect.
    async fn trip(
        upstream: &Arc<Mutex<Upstream>>,
        api: &str,
        linking: Option<&str>,
    ) -> (Result<Credentials>, StatusCode, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let authorization = crate::alphaxiv::Authorization::new(port).unwrap();
        let url = url::Url::parse(&authorization.url).unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert!(authorization
            .url
            .starts_with(&format!("{api}/auth/oauth2/authorize?")));
        assert_eq!(query["client_id"], "openresearch-desktop");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["scope"], "openid email profile");
        assert_eq!(query["prompt"], "select_account");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["redirect_uri"],
            format!("http://127.0.0.1:{port}/callback")
        );
        assert_eq!(query["state"], authorization.state);
        {
            let mut upstream = upstream.lock().unwrap();
            upstream.redirect_uri = query["redirect_uri"].clone();
            upstream.challenge = query["code_challenge"].clone();
            upstream.nonce = query["nonce"].clone();
        }

        let back = format!(
            "{}?code=code-1&state={}",
            query["redirect_uri"], query["state"]
        );
        let (finished, page) = tokio::join!(
            complete(
                &listener,
                &authorization,
                api.to_string(),
                linking.map(str::to_owned)
            ),
            async {
                let response = crate::client::http().get(&back).send().await.unwrap();
                (response.status(), response.text().await.unwrap())
            }
        );
        (finished, page.0, page.1)
    }

    fn stored_refresh_token() -> String {
        let path = crate::config::config_dir().join("alphaxiv.json");
        let tokens: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        tokens["refreshToken"].as_str().unwrap().to_string()
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sign_in_never_creates_or_uploads_an_ssh_key() {
        use tokio::io::AsyncBufReadExt;

        // The app's sign-in, run for real in a child with an empty home and no
        // PATH, so it can open no browser and run no ssh-keygen.
        if let Ok(api) = std::env::var("ORX_LOGIN_RUN_CHILD") {
            run(crate::LoginArgs {
                api_url: Some(api),
                non_interactive: true,
                alphaxiv: false,
            })
            .await
            .unwrap();
            return;
        }

        let upstream = Arc::new(Mutex::new(Upstream::default()));
        let key_requests = Arc::new(Mutex::new(0));
        let counted = key_requests.clone();
        let app = Router::new()
            .route("/auth/oauth2/token", post(token))
            .route("/auth/alphaxiv/desktop", post(desktop))
            .route(
                "/ssh-keys",
                axum::routing::any(move || {
                    *counted.lock().unwrap() += 1;
                    async { StatusCode::NOT_FOUND }
                }),
            )
            .with_state(upstream.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let home = std::env::temp_dir().join(format!("orx-login-home-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&home).unwrap();
        let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::login::tests::sign_in_never_creates_or_uploads_an_ssh_key",
                "--nocapture",
            ])
            .env("ORX_LOGIN_RUN_CHILD", &api)
            .env("ALPHAXIV_API_URL", &api)
            .env("ALPHAXIV_WEB_URL", &api)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("PATH", "")
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut stdout = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let authorize = loop {
            let line = stdout.next_line().await.unwrap().expect("sign-in URL");
            if line.trim().starts_with(&api) {
                break url::Url::parse(line.trim()).unwrap();
            }
        };
        let query: HashMap<String, String> = authorize.query_pairs().into_owned().collect();
        {
            let mut upstream = upstream.lock().unwrap();
            upstream.redirect_uri = query["redirect_uri"].clone();
            upstream.challenge = query["code_challenge"].clone();
            upstream.nonce = query["nonce"].clone();
        }
        let back = format!(
            "{}?code=code-1&state={}",
            query["redirect_uri"], query["state"]
        );
        let page = crate::client::http().get(&back).send().await.unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        while stdout.next_line().await.unwrap().is_some() {}
        let status = child.wait().await.unwrap();

        let ssh_dir_created = home.join(".ssh").exists();
        let _ = std::fs::remove_dir_all(&home);
        assert!(status.success());
        assert!(!ssh_dir_created);
        assert_eq!(*key_requests.lock().unwrap(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn one_browser_trip_signs_in_or_links() {
        // Credentials and the alphaXiv URL are process-global, so the test body
        // runs in a child process with its own config dir.
        if std::env::var("ORX_LOGIN_TEST_CHILD").is_err() {
            let root = std::env::temp_dir().join(format!("orx-login-{}", uuid::Uuid::new_v4()));
            let status = tokio::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "commands::login::tests::one_browser_trip_signs_in_or_links",
                    "--nocapture",
                ])
                .env("ORX_LOGIN_TEST_CHILD", "1")
                .env("XDG_CONFIG_HOME", &root)
                .status()
                .await
                .unwrap();
            let _ = std::fs::remove_dir_all(root);
            assert!(status.success());
            return;
        }

        let upstream = Arc::new(Mutex::new(Upstream::default()));
        let app = Router::new()
            .route("/auth/oauth2/token", post(token))
            .route("/auth/oauth2/revoke", post(revoke))
            .route("/auth/alphaxiv/desktop", post(desktop))
            .route("/auth/alphaxiv/link", post(link))
            .with_state(upstream.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        std::env::set_var("ALPHAXIV_API_URL", &api);
        std::env::set_var("ALPHAXIV_WEB_URL", &api);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let (signed_in, status, page) = trip(&upstream, &api, None).await;
        let creds = signed_in.unwrap();
        assert_eq!(
            (creds.api_url.as_str(), creds.token.as_str()),
            (api.as_str(), "pat-1")
        );
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("href=\"openresearch://\""));
        assert!(page.contains(&format!("href=\"{api}\">Open alphaXiv</a>")));
        assert_eq!(load_credentials().await.unwrap().unwrap().token, "pat-1");
        assert_eq!(stored_refresh_token(), "refresh-1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = crate::config::config_dir().join("alphaxiv.json");
            let mode = std::fs::metadata(path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        upstream.lock().unwrap().link_failure = Some((StatusCode::CONFLICT, "already_linked"));
        let (linked, status, page) = trip(&upstream, &api, Some("pat-1")).await;
        assert!(linked.unwrap_err().to_string().contains("already linked"));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(page.contains("already linked"));
        assert_eq!(upstream.lock().unwrap().revoked, ["refresh-2"]);
        assert_eq!(stored_refresh_token(), "refresh-1");

        // Replacing tokens revokes the ones they replace.
        upstream.lock().unwrap().link_failure = None;
        let (linked, status, _) = trip(&upstream, &api, Some("pat-1")).await;
        assert_eq!(linked.unwrap().token, "pat-1");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(stored_refresh_token(), "refresh-3");
        assert_eq!(upstream.lock().unwrap().revoked, ["refresh-2", "refresh-1"]);
        assert_eq!(
            upstream.lock().unwrap().linked_by,
            ["Bearer pat-1", "Bearer pat-1"]
        );

        upstream.lock().unwrap().desktop_failure = Some((StatusCode::CONFLICT, "failed"));
        let (signed_in, status, _) = trip(&upstream, &api, None).await;
        assert!(signed_in
            .unwrap_err()
            .to_string()
            .contains("connect alphaXiv there"));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            upstream.lock().unwrap().revoked,
            ["refresh-2", "refresh-1", "refresh-4"]
        );
        assert_eq!(stored_refresh_token(), "refresh-3");
        assert_eq!(load_credentials().await.unwrap().unwrap().token, "pat-1");

        // OpenResearch couldn't fetch alphaXiv's keys: a failed sign-in like any other.
        let unavailable = Some((StatusCode::SERVICE_UNAVAILABLE, "unavailable"));
        upstream.lock().unwrap().desktop_failure = unavailable;
        let (signed_in, _, page) = trip(&upstream, &api, None).await;
        assert!(signed_in
            .unwrap_err()
            .is::<crate::client::AlphaXivUnavailable>());
        assert!(page.contains("couldn't reach alphaXiv. Try again in a moment."));

        upstream.lock().unwrap().link_failure = unavailable;
        let (linked, _, page) = trip(&upstream, &api, Some("pat-1")).await;
        assert!(linked
            .unwrap_err()
            .is::<crate::client::AlphaXivUnavailable>());
        assert!(page.contains("couldn't reach alphaXiv. Try again in a moment."));

        // A rejected id_token is not a rejected OpenResearch token.
        upstream.lock().unwrap().link_failure = Some((StatusCode::UNAUTHORIZED, "invalid_token"));
        let (linked, _, _) = trip(&upstream, &api, Some("pat-1")).await;
        assert!(linked.unwrap_err().to_string().contains("couldn't verify"));
        upstream.lock().unwrap().link_failure = Some((StatusCode::UNAUTHORIZED, "Unauthorized"));
        let (linked, _, _) = trip(&upstream, &api, Some("pat-1")).await;
        assert!(linked.unwrap_err().is::<crate::client::Unauthorized>());

        upstream.lock().unwrap().desktop_failure = Some((StatusCode::NOT_FOUND, "not_found"));
        let (signed_in, _, _) = trip(&upstream, &api, None).await;
        assert!(signed_in.unwrap_err().to_string().contains("too old"));

        // Credentials that can't be saved leave the earlier sign-in in place.
        upstream.lock().unwrap().desktop_failure = None;
        let credentials = crate::config::config_dir().join("credentials.json");
        std::fs::remove_file(&credentials).unwrap();
        std::fs::create_dir(&credentials).unwrap();
        let (signed_in, _, _) = trip(&upstream, &api, None).await;
        assert!(signed_in.is_err());
        std::fs::remove_dir(&credentials).unwrap();
        crate::config::save_credentials(&creds).await.unwrap();

        assert_eq!(
            upstream.lock().unwrap().revoked,
            [
                "refresh-2",
                "refresh-1",
                "refresh-4",
                "refresh-5",
                "refresh-6",
                "refresh-7",
                "refresh-8",
                "refresh-9",
                "refresh-10"
            ]
        );
        assert_eq!(stored_refresh_token(), "refresh-3");
        assert_eq!(load_credentials().await.unwrap().unwrap().token, "pat-1");
    }
}
