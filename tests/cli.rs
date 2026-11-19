// Out of the crate, so `crate::process` is unreachable; tests run from a terminal anyway.
#![allow(clippy::disallowed_methods)]

#[path = "common/home.rs"]
mod home;

#[path = "cli/agents_cli.rs"]
mod agents_cli;
#[path = "cli/antigravity_gate_cli.rs"]
mod antigravity_gate_cli;
#[path = "cli/compute_cli.rs"]
mod compute_cli;
#[path = "cli/discover_cli.rs"]
mod discover_cli;
#[path = "cli/engine_cli.rs"]
mod engine_cli;
#[path = "cli/estimate_cli.rs"]
mod estimate_cli;
#[path = "cli/invocation_gate_cli.rs"]
mod invocation_gate_cli;
#[path = "cli/logs_cli.rs"]
mod logs_cli;
#[path = "cli/up_auto_restart.rs"]
mod up_auto_restart;
#[path = "cli/up_sign_in.rs"]
mod up_sign_in;

#[test]
fn unknown_commands_show_real_commands_and_scoped_help() {
    for (args, help, real) in [
        (vec!["exp", "list"], " exp --help`", "status"),
        (vec!["exp", "veiw"], " exp --help`", "view"),
        (vec!["exp", "view", "missing"], " exp view --help`", "check"),
        (vec!["missing"], " --help`", "projects"),
    ] {
        let root = std::env::temp_dir().join(format!("orx-command-hint-{}", uuid::Uuid::new_v4()));
        let output = home::isolate_home(
            &mut std::process::Command::new(env!("CARGO_BIN_EXE_orx")),
            &root,
        )
        .args(args)
        .output()
        .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(help), "{error}");
        assert!(error.contains(real), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn yield_and_goal_relay_as_agent_tools() {
    use axum::{routing::post, Json, Router};
    use serde_json::{json, Value};

    let (requests_tx, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let app = Router::new().route(
        "/api/internal/mcp",
        post(move |Json(request): Json<Value>| async move {
            let denied = request["params"]["arguments"]["kind"] == "running";
            requests_tx.send(request).unwrap();
            Json(json!({
                "content": [{"type": "text", "text": if denied { "No wake work" } else { "Recorded" }}],
                "isError": denied,
            }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = std::env::temp_dir().join(format!("orx-chat-tool-{}", uuid::Uuid::new_v4()));

    for (args, name, arguments, success) in [
        (
            vec!["yield", "need", "Share", "the", "corpus"],
            "yield",
            json!({"kind": "need", "text": "Share the corpus"}),
            true,
        ),
        (
            vec!["goal", "set", "Compare two baselines"],
            "set_goal",
            json!({"goal": "Compare two baselines"}),
            true,
        ),
        (
            vec!["goal", "clear"],
            "set_goal",
            json!({"goal": null}),
            true,
        ),
        (
            vec!["yield", "running", "Waiting"],
            "yield",
            json!({"kind": "running", "text": "Waiting"}),
            false,
        ),
    ] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_orx"));
        home::isolate_home(&mut command, &root)
            .args(args)
            .env("ORX_UP_PORT", port.to_string())
            .env("ORX_SESSION_ID", "chat_test")
            .env("ORX_AGENT_TOKEN", "test-token");
        let output = tokio::process::Command::from(command)
            .output()
            .await
            .unwrap();
        assert_eq!(output.status.success(), success, "{output:?}");
        assert_eq!(
            requests.try_recv().unwrap(),
            json!({
                "sessionId": "chat_test",
                "token": "test-token",
                "method": "tools/call",
                "params": {"name": name, "arguments": arguments},
            }),
        );
        if success {
            assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "Recorded");
        } else {
            assert!(String::from_utf8_lossy(&output.stderr).contains("No wake work"));
        }
    }
    server.abort();
    let _ = std::fs::remove_dir_all(root);
}
