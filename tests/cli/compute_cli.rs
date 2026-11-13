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
        crate::home::isolate_home(&mut cmd, &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("ORX_DATA_DIR", self.0.join("data"))
            .env("ORX_NO_UPDATE_CHECK", "1")
            .env_remove("HF_TOKEN")
            .env_remove("TINKER_API_KEY")
            .env_remove("MODAL_TOKEN_ID")
            .env_remove("MODAL_TOKEN_SECRET")
            .arg("--no-telemetry");
        #[cfg(unix)]
        cmd.env("PATH", orx_test_support::agentless_path(&self.0));
        cmd
    }
    fn stop_service(&self, data: &std::path::Path) {
        let _ = crate::home::isolate_home(&mut Command::new(env!("CARGO_BIN_EXE_orx")), &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("ORX_DATA_DIR", data)
            .args(["ssh-service", "stop"])
            .output();
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
        #[cfg(unix)]
        orx_test_support::stop_server(&self.0.join("data"));
        if let Ok(db) = rusqlite::Connection::open(self.0.join("data/orx.db")) {
            let _ = db.execute(
                "UPDATE runs SET status='cancelled' WHERE status IN ('running','starting')",
                [],
            );
        }
        self.stop_service(&self.0.join("data"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

enum HintShell {
    Posix(PathBuf),
    #[cfg(windows)]
    Cmd,
    #[cfg(windows)]
    PowerShell,
}

impl HintShell {
    fn quote(&self, path: &str) -> String {
        match self {
            Self::Posix(_) => format!("'{}'", path.replace('\'', "'\\''")),
            #[cfg(windows)]
            Self::PowerShell => format!("'{}'", path.replace('\'', "''")),
            #[cfg(windows)]
            Self::Cmd => format!("\"{path}\""),
        }
    }

    fn run(&self, sandbox: &Sandbox, cwd: &std::path::Path, script: &str) -> Output {
        let mut command = match self {
            Self::Posix(path) => {
                let mut command = Command::new(path);
                command.args(["-c", script]);
                command
            }
            #[cfg(windows)]
            Self::Cmd => {
                use std::os::windows::process::CommandExt;
                let mut command = Command::new("cmd.exe");
                command
                    .args(["/d", "/s", "/c"])
                    .raw_arg(format!("\"{script}\""));
                command
            }
            #[cfg(windows)]
            Self::PowerShell => {
                let mut command = Command::new("powershell.exe");
                command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
                command
            }
        };
        for (key, value) in sandbox.command().get_envs() {
            match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        command.current_dir(cwd).output().unwrap()
    }
}

fn checkout_shells() -> Vec<HintShell> {
    #[cfg(windows)]
    {
        let output = Command::new("git").arg("--exec-path").output().unwrap();
        assert!(output.status.success());
        let bash = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
            .ancestors()
            .map(|parent| parent.join("bin/bash.exe"))
            .find(|path| path.is_file())
            .expect("Git for Windows bash");
        vec![
            HintShell::Cmd,
            HintShell::PowerShell,
            HintShell::Posix(bash),
        ]
    }
    #[cfg(unix)]
    {
        vec![HintShell::Posix(PathBuf::from("sh"))]
    }
}

fn init_checkout_repo(repo: &std::path::Path) {
    std::fs::create_dir(repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        assert!(Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap()
            .status
            .success());
    }
}

fn current_checkout(repo: &std::path::Path) -> String {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["branch", "--show-current"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[test]
fn experiment_checkout_hint_matches_the_invoking_shell_and_stops_when_cd_fails() {
    for shell in checkout_shells() {
        let sandbox = Sandbox::new();
        let repo = sandbox.0.join("researcher's project $ [baseline] & notes");
        let unrelated = sandbox.0.join("unrelated");
        init_checkout_repo(&repo);
        init_checkout_repo(&unrelated);
        assert!(Command::new("git")
            .current_dir(&unrelated)
            .args(["branch", "orx/baseline"])
            .status()
            .unwrap()
            .success());
        sandbox.json(&["projects", "--json"]);
        let db = rusqlite::Connection::open(sandbox.0.join("data/orx.db")).unwrap();
        db.execute(
            "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, github_sync_enabled, baseline_branch, repo_path, created_at, updated_at) VALUES ('project', 'Research', 'research', '', '', 0, 'main', ?1, 0, 0)",
            [repo.to_str().unwrap()],
        ).unwrap();
        let executable = shell.quote(env!("CARGO_BIN_EXE_orx"));
        let call = match &shell {
            #[cfg(windows)]
            HintShell::PowerShell => "& ",
            _ => "",
        };
        let separator = match &shell {
            #[cfg(windows)]
            HintShell::Cmd => "&",
            _ => ";",
        };
        // A following command keeps sh alive rather than letting it exec the CLI.
        let output = shell.run(&sandbox, &unrelated, &format!(
            "{call}{executable} --no-telemetry create-experiment project --title Baseline {separator} echo shell-still-running"
        ));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        let command = text
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with("cd ") || line.starts_with("Set-Location "))
            .unwrap_or_else(|| {
                panic!(
                    "No checkout command in {text}\n{}",
                    String::from_utf8_lossy(&output.stderr)
                )
            });
        let location = "git rev-parse --show-toplevel";
        let output = shell.run(
            &sandbox,
            &unrelated,
            &format!("{command} {separator} {location}"),
        );
        assert!(
            output.status.success(),
            "{command}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            PathBuf::from(stdout.lines().last().unwrap().trim())
                .canonicalize()
                .unwrap(),
            repo.canonicalize().unwrap()
        );
        assert_eq!(current_checkout(&repo), "orx/baseline");
        assert_eq!(current_checkout(&unrelated), "main");

        std::fs::rename(&repo, sandbox.0.join("moved-project")).unwrap();
        let output = shell.run(&sandbox, &unrelated, command);
        assert!(
            !output.status.success(),
            "Missing project must fail: {command}"
        );
        assert_eq!(
            current_checkout(&unrelated),
            "main",
            "Failed cd must not check out the unrelated repository"
        );
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
    orx_test_support::write_executable(&ssh, "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/calls\"\nprintf 'Permission denied\\n' >&2\nexit 255\n", 0o755).unwrap();
    let result = sandbox
        .command()
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                orx_test_support::agentless_path(&sandbox.0)
            ),
        )
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
