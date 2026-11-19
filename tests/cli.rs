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
