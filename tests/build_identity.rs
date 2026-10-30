use std::process::Command;

#[test]
fn desktop_build_identity_is_available_without_an_update_check() {
    let output = Command::new(env!("CARGO_BIN_EXE_orx"))
        .args(["--no-telemetry", "version", "--build-id"])
        .env("ORX_NO_UPDATE_CHECK", "1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        option_env!("ORX_DESKTOP_CANARY_TAG").unwrap_or(env!("CARGO_PKG_VERSION"))
    );
}
