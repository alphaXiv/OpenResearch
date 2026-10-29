#![cfg(unix)]

#[test]
fn acp_startup_and_restore_install_native_skills() {
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/cli/acp_skills.py"
        ))
        .arg(env!("CARGO_BIN_EXE_orx"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
