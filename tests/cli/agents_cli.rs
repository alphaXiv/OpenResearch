use std::process::Command;

fn orx(root: &std::path::Path, env: &[(&str, &str)], args: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_orx"));
    crate::home::isolate_home(&mut command, root)
        .env("ORX_DATA_DIR", root.join("data"))
        .env("ORX_NO_UPDATE_CHECK", "1")
        .env_remove("ORX_LOCAL_SESSION")
        .env_remove("ORX_UP_PORT")
        .env_remove("ORX_UP_AUTH_TOKEN")
        .envs(env.iter().copied())
        .arg("--no-telemetry")
        .args(args);
    command.output().unwrap()
}

#[test]
fn agents_needs_the_dashboard_that_started_its_chat() {
    let root = std::env::temp_dir().join(format!("orx-agents-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let outside = orx(&root, &[], &["agents"]);
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains(
        "`orx agents` talks to the OpenResearch dashboard that started this chat; run it from an OpenResearch chat."
    ));

    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port().to_string();
    drop(closed);
    let gone = orx(
        &root,
        &[("ORX_LOCAL_SESSION", "1"), ("ORX_UP_PORT", &port)],
        &["agents", "--json"],
    );
    assert!(!gone.status.success());
    let stderr = String::from_utf8_lossy(&gone.stderr);
    assert!(
        stderr.contains("Could not reach the OpenResearch dashboard at http://127.0.0.1:")
            && stderr.contains("Check that OpenResearch is still running."),
        "{stderr}"
    );

    let unset = orx(
        &root,
        &[("ORX_LOCAL_SESSION", "1"), ("ORX_UP_PORT", &port)],
        &[
            "agents",
            "add-endpoint",
            "--name",
            "Box",
            "--url",
            "http://127.0.0.1:1/v1",
            "--model",
            "m",
            "--key-env",
            "ORX_TEST_UNSET_KEY",
        ],
    );
    assert!(String::from_utf8_lossy(&unset.stderr).contains(
        "`ORX_TEST_UNSET_KEY` is not set in this environment. Set it, or pass the key with --key."
    ));
    let _ = std::fs::remove_dir_all(root);
}
