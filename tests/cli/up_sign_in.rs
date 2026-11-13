#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use axum::{routing::post, Json, Router};
use futures::StreamExt as _;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest as _, http::header, Message};

struct Sandbox(PathBuf, std::process::Child);
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = self.1.kill();
        let _ = self.1.wait();
        orx_test_support::stop_server(&self.0.join("data"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// alphaXiv sends a browser back only to the machine it ran on, so a dashboard
/// opened from another machine signs in to OpenResearch with a code.
#[tokio::test(flavor = "multi_thread")]
async fn openresearch_sign_in_from_another_machine_shows_a_code() {
    let alphaxiv = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let site = format!("http://{}", alphaxiv.local_addr().unwrap());
    let approval = format!("{site}/device?user_code=BCDFGHJK");
    let fake = Router::new()
        .route(
            "/auth/device/code",
            post({
                let approval = approval.clone();
                move || async move {
                    Json(serde_json::json!({
                        "device_code": "device-1",
                        "user_code": "BCDFGHJK",
                        "verification_uri": approval.split('?').next().unwrap(),
                        "verification_uri_complete": approval,
                        "expires_in": 600,
                        "interval": 5,
                    }))
                }
            }),
        )
        .route(
            "/auth/oauth2/token",
            post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "authorization_pending" })),
                )
            }),
        );
    tokio::spawn(async move { axum::serve(alphaxiv, fake).await });

    let root = std::env::temp_dir().join(format!("orx-up-sign-in-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let up = crate::home::isolate_home(&mut Command::new(env!("CARGO_BIN_EXE_orx")), &root)
        .args([
            "--no-telemetry",
            "up",
            "--no-browser",
            "--host",
            "0.0.0.0",
            "--port",
        ])
        .arg(port.to_string())
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("share"))
        .env("ORX_DATA_DIR", root.join("data"))
        .env("ORX_NO_UPDATE_CHECK", "1")
        .env("PATH", orx_test_support::agentless_path(&root))
        .env("ALPHAXIV_API_URL", &site)
        .env("ALPHAXIV_WEB_URL", &site)
        .env_remove("SSH_CONNECTION")
        .env_remove("SSH_CLIENT")
        .env_remove("SSH_TTY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _sandbox = Sandbox(root, up);

    let mut serving = false;
    for _ in 0..300 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            serving = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(serving, "orx up never started serving");

    let mut request = format!("ws://127.0.0.1:{port}/api/settings/openresearch/login")
        .into_client_request()
        .unwrap();
    let page = format!("192.168.1.5:{port}");
    let headers = request.headers_mut();
    headers.insert(header::HOST, page.parse().unwrap());
    headers.insert(header::ORIGIN, format!("http://{page}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    let mut output = String::new();
    let shown = tokio::time::timeout(Duration::from_secs(60), async {
        while let Some(frame) = socket.next().await {
            match frame.unwrap() {
                Message::Binary(bytes) => output.push_str(&String::from_utf8_lossy(&bytes)),
                Message::Text(text) => panic!("sign-in ended before showing a code: {text}"),
                _ => {}
            }
            if output.contains("BCDFGHJK") && output.contains(&approval) {
                return;
            }
        }
    })
    .await;
    assert!(shown.is_ok(), "no code in the sign-in output:\n{output}");
}
