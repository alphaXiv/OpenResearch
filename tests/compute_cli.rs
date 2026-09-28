#[cfg(unix)]
use serde_json::json;
use serde_json::Value;
use std::path::PathBuf;
use std::process::{Command, Output};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("orx-compute-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_orx"));
        cmd.env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("ORX_DATA_DIR", self.0.join("data"))
            .env("ORX_NO_UPDATE_CHECK", "1")
            .env_remove("HF_TOKEN")
            .env_remove("TINKER_API_KEY")
            .env_remove("MODAL_TOKEN_ID")
            .env_remove("MODAL_TOKEN_SECRET")
            .arg("--no-telemetry");
        cmd
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn standalone_settings_defaults_and_instruction_conflicts() {
    let sandbox = Sandbox::new();
    let saved = sandbox.json(&[
        "compute",
        "configure",
        "slurm",
        "--host",
        "lab",
        "--partition",
        "gpu",
        "--time-limit",
        "24h",
        "--json",
    ]);
    assert_eq!(saved["timeLimit"], "24h");
    let saved = sandbox.json(&[
        "compute",
        "configure",
        "slurm",
        "--clear",
        "partition",
        "--json",
    ]);
    assert_eq!(saved["timeLimit"], "24h");
    assert_eq!(saved["partition"], Value::Null);
    assert_eq!(
        sandbox.json(&["compute", "status", "--json"])["defaultBackend"],
        "local"
    );
    assert_eq!(
        sandbox.json(&["compute", "default", "set", "slurm", "--json"])["defaultBackend"],
        "slurm"
    );
    assert_eq!(
        sandbox.json(&["compute", "default", "clear", "--json"])["defaultBackend"],
        "local"
    );
    assert!(!sandbox
        .run(&["compute", "configure", "slurm", "--time-limit", "nonsense"])
        .status
        .success());
    assert_eq!(
        sandbox.json(&["compute", "show", "slurm", "--json"])["timeLimit"],
        "24h"
    );
    assert!(!sandbox
        .run(&["compute", "--cpu", "status"])
        .status
        .success());
    let before = sandbox.json(&["compute", "--json", "instructions", "show"]);
    let recipe = sandbox.0.join("recipe.md");
    std::fs::write(&recipe, "## Local\nUse .venv/bin/python.\n").unwrap();
    let args = [
        "compute",
        "instructions",
        "set",
        "--file",
        recipe.to_str().unwrap(),
        "--expected-revision",
        before["revision"].as_str().unwrap(),
        "--json",
    ];
    sandbox.json(&args);
    assert!(!sandbox.run(&args).status.success());
    assert!(
        sandbox.json(&["compute", "instructions", "show", "--json"])["content"]
            .as_str()
            .unwrap()
            .contains(".venv/bin/python")
    );
}

#[cfg(unix)]
#[test]
fn ssh_config_conflict_and_execution_target_checks() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::new();
    std::fs::write(
        sandbox.0.join(".ssh/config"),
        "Host lab\n  HostName localhost\n",
    )
    .unwrap();
    sandbox.json(&[
        "compute",
        "configure",
        "ssh",
        "--default-host",
        "lab",
        "--json",
    ]);
    sandbox.json(&[
        "compute",
        "configure",
        "ssh",
        "--host",
        "lab",
        "--container",
        "research",
        "--json",
    ]);
    let settings = sandbox.json(&["compute", "show", "ssh", "--json"]);
    assert_eq!(settings["hosts"][0]["container"], "research");
    let bin = sandbox.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/calls\"\nprintf 'Permission denied\\n' >&2\nexit 255\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let result = sandbox
        .command()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .args(["compute", "test", "ssh", "--json"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let value: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["ready"], false);
    assert_eq!(value["container"]["reference"], "research");
    let old = sandbox.0.join("old");
    let new = sandbox.0.join("new");
    std::fs::write(&old, "stale").unwrap();
    std::fs::write(&new, "replacement").unwrap();
    assert!(!sandbox
        .run(&[
            "compute",
            "ssh-config",
            "set",
            "--file",
            new.to_str().unwrap(),
            "--previous-file",
            old.to_str().unwrap()
        ])
        .status
        .success());
    assert!(std::fs::read_to_string(sandbox.0.join(".ssh/config"))
        .unwrap()
        .contains("Host lab"));
}

#[cfg(unix)]
#[test]
fn slurm_supervisor_survives_connection_and_accounting_loss() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let sandbox = Sandbox::new();
    sandbox.json(&["projects", "--json"]);
    let db = rusqlite::Connection::open(sandbox.0.join("data/orx.db")).unwrap();
    db.execute("INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, created_at, updated_at) VALUES ('exp','project','exp','main','true',0,0)", []).unwrap();
    let backend = json!({"kind":"slurm_job", "namespace":"lab", "jobId":"42", "timeoutSecs":86400});
    db.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at) VALUES ('run','exp','project','running',?1,0,0)", [backend.to_string()]).unwrap();
    let bin = sandbox.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/calls"
case "$*" in
  *sbatch*|*scancel*) exit 99;;
  *exit_code*)
    phase=$(cat "$HOME/phase")
    case "$phase" in
      outage) echo 'Permission denied (MFA expired)' >&2; exit 255;;
      missing) echo GONE;;
      running) echo 'SQ RUNNING';;
      done) echo 'SA COMPLETED';;
    esac;;
  *) exit 0;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let phase = sandbox.0.join("phase");
    std::fs::write(&phase, "outage").unwrap();
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = ChildGuard(
        sandbox
            .command()
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .args(["supervise", "run"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    let metadata = || -> Value {
        let raw: String = db
            .query_row("SELECT backend_json FROM runs WHERE id='run'", [], |r| {
                r.get(0)
            })
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    };
    let wait = |condition: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "supervisor did not reach expected state"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    wait(&|| {
        metadata()["monitoringError"]
            .as_str()
            .is_some_and(|s| s.contains("Reconnect"))
    });
    std::fs::write(&phase, "missing").unwrap();
    wait(&|| {
        metadata()["monitoringError"]
            .as_str()
            .is_some_and(|s| s.contains("scheduler record"))
    });
    // Exceeds the old one-minute GONE-to-failed threshold.
    std::thread::sleep(Duration::from_secs(61));
    let status: String = db
        .query_row("SELECT status FROM runs WHERE id='run'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(status, "running");
    assert_eq!(metadata()["jobId"], "42");
    std::fs::write(&phase, "running").unwrap();
    wait(&|| metadata()["monitoringError"].is_null());
    std::fs::write(&phase, "done").unwrap();
    wait(&|| {
        db.query_row("SELECT status FROM runs WHERE id='run'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
            == "done"
    });
    let deadline = Instant::now() + Duration::from_secs(25);
    while child.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(100));
    }
    let calls = std::fs::read_to_string(sandbox.0.join("calls")).unwrap();
    assert!(!calls.contains("sbatch") && !calls.contains("scancel"));
}

#[cfg(unix)]
#[test]
fn credential_files_are_masked_and_can_be_cleared() {
    let sandbox = Sandbox::new();
    let file = sandbox.0.join("credentials.json");
    let secret = "or304-fixture-secret-never-print-me";
    std::fs::write(
        &file,
        format!(r#"{{"tokenId":"fixture-id-123456","tokenSecret":"{secret}"}}"#),
    )
    .unwrap();
    let output = sandbox.run(&[
        "compute",
        "configure",
        "modal",
        "--credentials-file",
        file.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["tokenConfigured"], true);
    let value = sandbox.json(&[
        "compute",
        "configure",
        "modal",
        "--clear",
        "credentials",
        "--json",
    ]);
    assert_eq!(value["tokenConfigured"], false);
    assert_eq!(value["clearedSavedCredentials"], true);
}

#[test]
fn slurm_status_preserves_submission_failure_without_job_handle() {
    let sandbox = Sandbox::new();
    sandbox.json(&["projects", "--json"]);
    let db = rusqlite::Connection::open(sandbox.0.join("data/orx.db")).unwrap();
    db.execute("INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, created_at, updated_at) VALUES ('exp','project','exp','main','true',0,0)", []).unwrap();
    db.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, result_markdown, created_at, updated_at) VALUES ('run','exp','project','failed','{\"kind\":\"slurm_job\"}','Submission failed: invalid partition',0,0)", []).unwrap();
    for args in [
        vec!["exp", "status", "exp"],
        vec!["exp", "status", "exp", "--scheduler"],
    ] {
        let output = sandbox.run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("invalid partition"));
    }
}

#[cfg(unix)]
#[test]
fn slurm_cancel_requires_acknowledgement_and_survives_restart_without_accounting() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let sandbox = Sandbox::new();
    sandbox.json(&["projects", "--json"]);
    let db = rusqlite::Connection::open(sandbox.0.join("data/orx.db")).unwrap();
    db.execute("INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, created_at, updated_at) VALUES ('exp','project','exp','main','true',0,0)", []).unwrap();
    db.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, cancel_requested, created_at, updated_at) VALUES ('run','exp','project','starting','{\"kind\":\"slurm_job\",\"namespace\":\"lab\",\"jobId\":\"42\"}',1,0,0)", []).unwrap();
    let bin = sandbox.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    for (name, script) in [
        (
            "ssh",
            r#"#!/bin/sh
for cmd do :; done
case "$cmd" in
  *scancel*|*exit_code*) exec /bin/sh -c "$cmd";;
  *) exit 0;;
esac
"#,
        ),
        (
            "scancel",
            r#"#!/bin/sh
echo called >> "$HOME/cancel-calls"
if test -f "$HOME/accept"; then exit 0; fi
echo 'Invalid job id specified' >&2
exit 1
"#,
        ),
        (
            "squeue",
            r#"#!/bin/sh
case "$(cat "$HOME/phase")" in
  unavailable) exit 1;;
  hidden) case "$*" in
    *--jobs=*) exit 1;;
    *--all*--states=all*) echo '42 SUSPENDED';;
  esac;;
  gone) case "$*" in *--jobs=*) echo 'slurm_load_jobs error: Invalid job id specified' >&2; exit 1;; esac;;
esac
"#,
        ),
        ("sacct", "#!/bin/sh\nexit 1\n"),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(sandbox.0.join("phase"), "hidden").unwrap();
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let spawn = || {
        Child(
            sandbox
                .command()
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .args(["supervise", "run"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let metadata = || -> Value {
        serde_json::from_str(
            &db.query_row("SELECT backend_json FROM runs WHERE id='run'", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap(),
        )
        .unwrap()
    };
    let status = || {
        db.query_row("SELECT status FROM runs WHERE id='run'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
    };
    let wait = |condition: &dyn Fn() -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !condition() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let child = spawn();
    wait(&|| status() == "running");
    assert!(sandbox.0.join("cancel-calls").exists());
    assert_ne!(metadata()["cancellationAccepted"], true);
    assert_eq!(status(), "running");
    std::fs::write(sandbox.0.join("phase"), "unavailable").unwrap();
    std::fs::write(sandbox.0.join("accept"), "").unwrap();
    wait(&|| metadata()["cancellationAccepted"] == true);
    assert_eq!(status(), "running");
    drop(child);
    std::fs::remove_file(sandbox.0.join("accept")).unwrap();
    std::fs::write(sandbox.0.join("phase"), "gone").unwrap();
    let restarted = spawn();
    wait(&|| status() == "cancelled");
    drop(restarted);
    // Cancelling after the scheduler has already forgotten the job also finishes.
    db.execute("UPDATE runs SET status='running', ended_at=NULL, backend_json='{\"kind\":\"slurm_job\",\"namespace\":\"lab\",\"jobId\":\"42\"}' WHERE id='run'", []).unwrap();
    let _already_gone = spawn();
    wait(&|| status() == "cancelled");
}

#[test]
fn concurrent_instruction_writers_cannot_overwrite_each_other() {
    let sandbox = Sandbox::new();
    let before = sandbox.json(&["compute", "instructions", "show", "--json"]);
    let mut children = Vec::new();
    for content in ["first recipe", "second recipe"] {
        let file = sandbox.0.join(content);
        std::fs::write(&file, content).unwrap();
        children.push(
            sandbox
                .command()
                .args([
                    "compute",
                    "instructions",
                    "set",
                    "--file",
                    file.to_str().unwrap(),
                    "--expected-revision",
                    before["revision"].as_str().unwrap(),
                    "--json",
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
    }
    let winners = children
        .into_iter()
        .map(|child| child.wait_with_output().unwrap())
        .filter(|output| output.status.success())
        .count();
    assert_eq!(winners, 1);
    let final_value = sandbox.json(&["compute", "instructions", "show", "--json"]);
    assert!(["first recipe", "second recipe"].contains(&final_value["content"].as_str().unwrap()));
}

#[test]
fn instruction_set_rejects_prior_direct_edit() {
    let sandbox = Sandbox::new();
    let path = sandbox.run(&["compute", "instructions", "path"]);
    assert!(path.status.success());
    let path = String::from_utf8(path.stdout).unwrap();
    let before = sandbox.json(&["compute", "instructions", "show", "--json"]);
    std::fs::write(path.trim(), "editor guidance").unwrap();
    let draft = sandbox.0.join("draft.md");
    std::fs::write(&draft, "stale agent guidance").unwrap();

    let result = sandbox.run(&[
        "compute",
        "instructions",
        "set",
        "--file",
        draft.to_str().unwrap(),
        "--expected-revision",
        before["revision"].as_str().unwrap(),
    ]);
    assert!(!result.status.success());
    assert_eq!(
        sandbox.json(&["compute", "instructions", "show", "--json"])["content"],
        "editor guidance"
    );
}

#[cfg(unix)]
#[test]
fn custom_instructions_survive_reinstall_and_harness_switches_across_projects() {
    let sandbox = Sandbox::new();
    let path = sandbox.run(&["compute", "instructions", "path"]);
    assert!(path.status.success());
    let path = PathBuf::from(String::from_utf8(path.stdout).unwrap().trim());
    assert!(path.starts_with(&sandbox.0));
    std::fs::write(&path, "## Local\nUse .venv/bin/python.\n").unwrap();
    let saved = sandbox.json(&["compute", "instructions", "show", "--json"]);
    for project in ["one", "two"] {
        let cwd = sandbox.0.join(project);
        std::fs::create_dir(&cwd).unwrap();
        let output = sandbox
            .command()
            .current_dir(&cwd)
            .env_remove("CODEX_HOME")
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CURSOR_CONFIG_DIR")
            .args(["install-skills", "--agent", "all", "--full"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let output = sandbox
            .command()
            .current_dir(&cwd)
            .args(["compute", "instructions", "show", "--json"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            saved
        );
        std::fs::remove_dir_all(cwd).unwrap();
    }
    assert!(sandbox
        .run(&["compute", "instructions", "path"])
        .status
        .success());
    assert_eq!(
        sandbox.json(&["compute", "instructions", "show", "--json"]),
        saved
    );
}
