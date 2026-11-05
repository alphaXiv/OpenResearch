use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn uncaptured_invocation_allows_the_tool_call() {
    let root = std::env::temp_dir().join(format!("orx-invocation-gate-{}", uuid::Uuid::new_v4()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_orx"))
        .arg("invocation-gate")
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("ORX_DATA_DIR", root.join("data"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"tool_name":"Bash","tool_use_id":"toolu_never_captured","tool_input":{"command":"echo ok"}}"#)
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let _ = std::fs::remove_dir_all(root);
}
