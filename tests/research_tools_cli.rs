use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("orx-research-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_orx"))
            .env("ORX_DATA_DIR", self.0.join("data"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("ORX_NO_UPDATE_CHECK", "1")
            .arg("--no-telemetry")
            .args(args)
            .output()
            .unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn doctor_local_reports_the_executable_and_validates_options() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(&["compute", "doctor", "local", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ready"], true);
    assert!(body["executable"].as_str().unwrap().contains("orx"));
    assert!(!sandbox
        .run(&["compute", "doctor", "colab", "--host", "lab"])
        .status
        .success());
}

#[test]
fn imports_metrics_only_into_the_named_run_and_requires_explicit_replacement() {
    let sandbox = Sandbox::new();
    assert!(!sandbox
        .run(&["exp", "metrics", "exp", "--run", "missing"])
        .status
        .success());
    let conn = rusqlite::Connection::open(sandbox.0.join("data/orx.db")).unwrap();
    conn.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at) VALUES ('run', 'exp', 'project', 'done', '{}', 0, 0)", []).unwrap();
    let path = sandbox.0.join("metrics.json");
    let payload = json!({"version":1,"dataset":"fixture-v1","split":"test","metrics":{"accuracy":0.9},"parameters":{"batch":32},"seed":0});
    std::fs::write(&path, payload.to_string()).unwrap();
    let args = [
        "exp",
        "metrics",
        "exp",
        "--run",
        "run",
        "--file",
        path.to_str().unwrap(),
    ];
    let output = sandbox.run(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        payload
    );
    assert!(!sandbox.run(&args).status.success());
    let mut replacement = args.to_vec();
    replacement.push("--replace");
    assert!(sandbox.run(&replacement).status.success());
    assert!(!sandbox
        .run(&["exp", "metrics", "wrong", "--run", "run"])
        .status
        .success());
    std::fs::write(&path, "{\"version\":2}").unwrap();
    assert!(!sandbox.run(&replacement).status.success());
    let saved = sandbox.run(&["exp", "metrics", "exp", "--run", "run"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&saved.stdout).unwrap(),
        payload
    );
}

#[test]
fn deep_research_and_role_references_are_discoverable_from_cli() {
    let sandbox = Sandbox::new();
    for name in [
        "deep-research",
        "deep-research/references/researcher.md",
        "deep-research/references/report-writer.md",
    ] {
        let output = sandbox.run(&["skill", name]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
    }
}
