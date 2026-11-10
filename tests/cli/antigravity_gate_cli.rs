use std::io::Write;
use std::process::{Command, Stdio};

fn gate(payload: &str) -> serde_json::Value {
    let root = std::env::temp_dir().join(format!("orx-antigravity-gate-{}", uuid::Uuid::new_v4()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_orx"))
        .arg("antigravity-gate")
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
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(root);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn a_subagent_is_denied_with_the_way_to_delegate() {
    let decision = gate(
        r#"{"conversationId":"c","modelName":"m","toolCall":{"name":"invoke_subagent","args":{"Prompt":"run tests"}}}"#,
    );
    assert_eq!(decision["decision"], "deny");
    assert!(decision["reason"].as_str().unwrap().contains("delegate"));
}

#[test]
fn other_tools_are_allowed() {
    let decision = gate(
        r#"{"conversationId":"c","modelName":"m","toolCall":{"name":"view_file","args":{"AbsolutePath":"/tmp/x"}}}"#,
    );
    assert_eq!(decision["decision"], "allow");
}
