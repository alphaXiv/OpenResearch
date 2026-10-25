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
    fn stop_service(&self, data: &std::path::Path) {
        let _ = Command::new(env!("CARGO_BIN_EXE_orx"))
            .env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
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
fn interactive_ssh_retires_rejected_masters_without_unlinking_unknown_connections() {
    std::thread::scope(|scope| {
        for case in [
            "healthy",
            "legacy_other",
            "legacy_unresolvable",
            "legacy_same",
            "rejection",
            "exit_zero",
            "stop_failure",
            "probe_timeout",
            "check_timeout",
            "stop_timeout",
        ] {
            scope.spawn(move || interactive_ssh_case(case));
        }
    });
}

#[cfg(unix)]
fn interactive_ssh_case(case: &str) {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;

    let sandbox = Sandbox::new();
    std::fs::write(
        sandbox.0.join(".ssh/config"),
        "Host lab\n  HostName localhost\n",
    )
    .unwrap();
    let bin = sandbox.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(&ssh, r#"#!/bin/sh
if [ "$1" = -G ]; then
  for dest do :; done
  if [ "$dest" = unresolvable-host ]; then echo "Invalid legacy SSH route" >&2; exit 1; fi
  exec /usr/bin/ssh -F /dev/null "$@"
fi
printf '%s\n' "$*" >> "$HOME/calls"
case "$*" in
  *'-O check -S '*)
    socket=$4
    printf '%s' "$socket" > "$HOME/socket-path"
    : > "$socket"
    if [ "$CASE" = check_timeout ]; then exec sleep 30; fi
    exit 0;;
  *'-O stop -S '*)
    socket=$4
    case "$CASE" in
      stop_failure) exit 1;;
      stop_timeout) exec sleep 30;;
    esac
    rm "$socket"
    exit 0;;
  *'echo __ORX_SESSION_OK__'*)
    case "$*" in
      *'-oBatchMode=yes -oControlMaster=no -oControlPersist=no -oProxyCommand=false -oClearAllForwardings=yes -oRemoteCommand=none -T -S '*) ;;
      *) echo 'probe allows fallback or user configuration' >&2; exit 99;;
    esac
    case "$CASE" in
      healthy) echo __ORX_SESSION_OK__;;
      rejection) echo 'cannot assign requested address' >&2; exit 1;;
      exit_zero|stop_failure|stop_timeout) echo 'cannot assign requested address';;
      probe_timeout) exec sleep 30;;
    esac
    exit 0;;
  *'-- lab true') echo LOGIN; exit 0;;
  *'sys.stdin.buffer.read'*) for cmd do :; done; exec /bin/sh -c "$cmd";;
  *'command -v bash'*) exit 0;;
  *) exit 1;;
esac
"#).unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();

    for case in std::iter::once(case).chain((case == "healthy").then_some("backoff")) {
        let _ = std::fs::remove_file(sandbox.0.join("calls"));
        let command = sandbox.command();
        let mut builder = CommandBuilder::new(command.get_program());
        builder.args(command.get_args());
        for (key, value) in command.get_envs() {
            match value {
                Some(value) => builder.env(key, value),
                None => builder.env_remove(key),
            }
        }
        builder.env("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        let data = sandbox
            .0
            .join(if case == "backoff" { "healthy" } else { case });
        if case == "backoff" {
            let root = std::fs::read_dir(&data)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("ssh-service-")
                })
                .unwrap();
            let db = rusqlite::Connection::open(root.join("state.sqlite3")).unwrap();
            db.execute(
                "UPDATE connections SET failures=6, next_at=?1",
                [i64::try_from(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis(),
                )
                .unwrap()
                    + 300_000],
            )
            .unwrap();
        }
        let legacy = matches!(case, "legacy_other" | "legacy_same" | "legacy_unresolvable");
        let mut legacy_lock = if legacy {
            let output = sandbox
                .command()
                .env("ORX_DATA_DIR", &data)
                .args(["projects", "--json"])
                .output()
                .unwrap();
            assert!(output.status.success());
            let db = rusqlite::Connection::open(data.join("orx.db")).unwrap();
            let host = match case {
                "legacy_same" => "lab",
                "legacy_unresolvable" => "unresolvable-host",
                _ => "unrelated-host",
            };
            let backend = json!({"kind":"ssh_job", "namespace":host, "jobId":".orx/runs/legacy"});
            db.execute("INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, created_at, updated_at) VALUES ('exp','project','exp','main','true',0,0)", []).unwrap();
            db.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at) VALUES ('legacy','exp','project','running',?1,0,0)", [backend.to_string()]).unwrap();
            std::fs::create_dir_all(data.join("run-logs")).unwrap();
            Some(fd_lock::RwLock::new(
                std::fs::File::create(data.join("run-logs/legacy.supervisor.lock")).unwrap(),
            ))
        } else {
            None
        };
        let _legacy_guard = legacy_lock.as_mut().map(|lock| lock.write().unwrap());
        builder.env("ORX_DATA_DIR", &data);
        builder.env(
            "CASE",
            if case == "backoff" || legacy {
                "healthy"
            } else {
                case
            },
        );
        builder.args(["compute", "connect", "ssh", "--host", "lab"]);
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let mut child = pair.slave.spawn_command(builder).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut output = Vec::new();
        // Linux PTYs report EIO instead of EOF when the slave closes.
        if let Err(error) = reader.read_to_end(&mut output) {
            assert_eq!(error.raw_os_error(), Some(libc::EIO));
        }
        let status = child.wait().unwrap();
        let calls = std::fs::read_to_string(sandbox.0.join("calls")).unwrap();
        let socket = PathBuf::from(std::fs::read_to_string(sandbox.0.join("socket-path")).unwrap());
        let output = String::from_utf8_lossy(&output);
        if legacy {
            rusqlite::Connection::open(data.join("orx.db"))
                .unwrap()
                .execute("UPDATE runs SET status='cancelled' WHERE id='legacy'", [])
                .unwrap();
            sandbox.stop_service(&data);
        }
        match case {
            "healthy" | "backoff" | "legacy_other" | "legacy_unresolvable" => {
                assert!(status.success(), "{output}");
                assert!(!calls.contains("-O stop"));
                assert!(socket.exists());
                assert!(output.contains("LOGIN"));
            }
            "legacy_same" => {
                assert!(!status.success(), "{output}");
                assert!(output.contains("older OpenResearch process"), "{output}");
            }
            "rejection" | "exit_zero" => {
                assert!(status.success(), "{output}");
                assert!(calls.find("-O stop").unwrap() < calls.find("-- lab true").unwrap());
                assert!(!socket.exists());
                assert!(output.contains("LOGIN"));
            }
            _ => {
                assert!(!status.success(), "{case}: {output}");
                assert!(socket.exists(), "{case} removed an unknown/live socket");
                assert!(!output.contains("LOGIN"));
                if case != "stop_failure" && case != "stop_timeout" {
                    assert!(!calls.contains("-O stop"));
                }
            }
        }
        sandbox.stop_service(&data);
        assert!(!calls.contains("-O exit"));
        let _ = std::fs::remove_file(&socket);
        let _ = std::fs::remove_file(socket.with_extension("lock"));
    }
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
    supervisor_recovers_monitoring_and_logs(false);
}

#[cfg(unix)]
#[test]
fn ssh_supervisor_survives_proxy_output_without_skipping_logs() {
    supervisor_recovers_monitoring_and_logs(true);
}

#[cfg(unix)]
fn supervisor_recovers_monitoring_and_logs(ssh_backend: bool) {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, Instant};
    let sandbox = Sandbox::new();
    sandbox.json(&["projects", "--json"]);
    let db = rusqlite::Connection::open(sandbox.0.join("data/orx.db")).unwrap();
    db.execute("INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, created_at, updated_at) VALUES ('exp','project','exp','main','true',0,0)", []).unwrap();
    let backend = if ssh_backend {
        json!({"kind":"ssh_job", "namespace":"lab", "jobId":".orx/runs/00000000-0000-4000-8000-000000000001"})
    } else {
        json!({"kind":"slurm_job", "namespace":"lab", "jobId":"42", "timeoutSecs":86400})
    };
    db.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at) VALUES ('00000000-0000-4000-8000-000000000001','exp','project','running',?1,0,0)", [backend.to_string()]).unwrap();
    let bin = sandbox.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        r#"#!/bin/sh
if [ "$1" = -G ]; then exec /usr/bin/ssh -F /dev/null "$@"; fi
for cmd do :; done
exec python3 "$HOME/ssh-monitor-fixture.py" "$cmd"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        sandbox.0.join("ssh-monitor-fixture.py"),
        include_str!("../ssh-monitor-fixture.py"),
    )
    .unwrap();
    let phase = sandbox.0.join("phase");
    std::fs::write(&phase, "missing").unwrap();
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let supervisor_log = sandbox.0.join("supervisor.log");
    let mut child = ChildGuard(
        sandbox
            .command()
            .env("SSH_BACKEND", if ssh_backend { "1" } else { "0" })
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .args(["supervise", "00000000-0000-4000-8000-000000000001"])
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&supervisor_log).unwrap())
            .spawn()
            .unwrap(),
    );
    let metadata = || -> Value {
        let raw: String = db
            .query_row(
                "SELECT backend_json FROM runs WHERE id='00000000-0000-4000-8000-000000000001'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&raw).unwrap()
    };
    let wait_for = |timeout: Duration, condition: &dyn Fn() -> bool| {
        let deadline = Instant::now() + timeout;
        while !condition() {
            assert!(
                Instant::now() < deadline,
                "supervisor did not reach expected state; metadata: {}; stderr: {}",
                metadata(),
                std::fs::read_to_string(&supervisor_log).unwrap_or_default()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let wait = |condition: &dyn Fn() -> bool| wait_for(Duration::from_secs(15), condition);
    // A missing scheduler record is reported only after about a minute, without failing the run.
    wait(&|| {
        std::fs::read_to_string(sandbox.0.join("calls")).is_ok_and(|c| c.contains("exit_code"))
    });
    std::thread::sleep(Duration::from_secs(10));
    assert!(metadata()["monitoringError"].is_null());
    wait_for(Duration::from_secs(75), &|| {
        metadata()["monitoringError"].as_str().is_some_and(|s| {
            s.contains(if ssh_backend {
                "unexpected inspect output"
            } else {
                "scheduler record"
            })
        })
    });
    std::fs::write(&phase, "outage").unwrap();
    wait(&|| {
        metadata()["monitoringError"]
            .as_str()
            .is_some_and(|s| s.contains("Reconnect"))
    });
    let status: String = db
        .query_row(
            "SELECT status FROM runs WHERE id='00000000-0000-4000-8000-000000000001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "running");
    assert_eq!(
        metadata()["jobId"],
        if ssh_backend {
            ".orx/runs/00000000-0000-4000-8000-000000000001"
        } else {
            "42"
        }
    );
    if ssh_backend {
        std::fs::write(&phase, "log_failure").unwrap();
        wait(&|| {
            metadata()["monitoringError"]
                .as_str()
                .is_some_and(|s| s.contains("unexpected SSH log output"))
        });
    }
    std::fs::write(&phase, "running").unwrap();
    wait(&|| metadata()["monitoringError"].is_null());
    std::fs::write(&phase, "done").unwrap();
    wait(&|| {
        db.query_row(
            "SELECT status FROM runs WHERE id='00000000-0000-4000-8000-000000000001'",
            [],
            |r| r.get::<_, String>(0),
        )
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
    assert_eq!(
        {
            let cursor: Value = serde_json::from_slice(
                &std::fs::read(
                    sandbox
                        .0
                        .join("data/run-logs/00000000-0000-4000-8000-000000000001.cursor.json"),
                )
                .unwrap(),
            )
            .unwrap();
            std::fs::read_to_string(sandbox.0.join(format!(
                "data/run-logs/00000000-0000-4000-8000-000000000001.{}.log",
                cursor["generation"].as_str().unwrap()
            )))
            .unwrap()
        },
        "first\nsecond\n"
    );
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
    db.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, cancel_requested, created_at, updated_at) VALUES ('00000000-0000-4000-8000-000000000001','exp','project','starting','{\"kind\":\"slurm_job\",\"namespace\":\"lab\",\"jobId\":\"42\"}',1,0,0)", []).unwrap();
    let bin = sandbox.0.join("bin");
    std::fs::create_dir(&bin).unwrap();
    for (name, script) in [
        (
            "ssh",
            r#"#!/bin/sh
if [ "$1" = -G ]; then exec /usr/bin/ssh -F /dev/null "$@"; fi
for cmd do :; done
case "$cmd" in
  *sys.stdin.buffer.read*) exec /bin/sh -c "$cmd";;
  *scancel*|*exit_code*) exec /bin/sh -c "$cmd";;
  *__ORX_LOG_START__*) echo __ORX_LOG_START__;;
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
    *'-j '*|*--jobs=*) exit 1;;
    *--all*--states=all*--user=*) echo '42 SUSPENDED';;
    *--all*--states=all*'-u '*) echo '42|SUSPENDED';;
  esac;;
  gone) case "$*" in *'-j '*|*--jobs=*) echo 'slurm_load_jobs error: Invalid job id specified' >&2; exit 1;; esac;;
esac
"#,
        ),
        ("sacct", "#!/bin/sh\nexit 1\n"),
    ] {
        let path = bin.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::create_dir_all(
        sandbox
            .0
            .join(".orx/runs/00000000-0000-4000-8000-000000000001"),
    )
    .unwrap();
    std::fs::write(
        sandbox
            .0
            .join(".orx/runs/00000000-0000-4000-8000-000000000001/log"),
        "",
    )
    .unwrap();
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
                .args(["supervise", "00000000-0000-4000-8000-000000000001"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let metadata = || -> Value {
        serde_json::from_str(
            &db.query_row(
                "SELECT backend_json FROM runs WHERE id='00000000-0000-4000-8000-000000000001'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let status = || {
        db.query_row(
            "SELECT status FROM runs WHERE id='00000000-0000-4000-8000-000000000001'",
            [],
            |r| r.get::<_, String>(0),
        )
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
    db.execute("UPDATE runs SET status='running', ended_at=NULL, backend_json='{\"kind\":\"slurm_job\",\"namespace\":\"lab\",\"jobId\":\"42\"}' WHERE id='00000000-0000-4000-8000-000000000001'", []).unwrap();
    let already_gone = spawn();
    wait(&|| status() == "cancelled");
    drop(already_gone);

    // Queued jobs can be cancelled before Slurm creates their log.
    std::fs::remove_file(
        sandbox
            .0
            .join(".orx/runs/00000000-0000-4000-8000-000000000001/log"),
    )
    .unwrap();
    db.execute("UPDATE runs SET status='starting', ended_at=NULL, backend_json='{\"kind\":\"slurm_job\",\"namespace\":\"lab\",\"jobId\":\"42\",\"monitoringError\":\"Waiting for logs\"}' WHERE id='00000000-0000-4000-8000-000000000001'", []).unwrap();
    std::fs::write(sandbox.0.join("phase"), "hidden").unwrap();
    let _without_log = spawn();
    wait(&|| {
        metadata()["monitoringError"]
            .as_str()
            .is_some_and(|error| error.contains("Remote log is not available"))
    });
    assert_ne!(status(), "cancelled");
    std::fs::write(sandbox.0.join("accept"), "").unwrap();
    std::fs::write(sandbox.0.join("phase"), "gone").unwrap();
    let deadline = Instant::now() + Duration::from_secs(35);
    while status() != "cancelled" {
        assert!(
            Instant::now() < deadline,
            "missing logs blocked cancellation"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(metadata()["monitoringError"]
        .as_str()
        .is_some_and(|error| error.contains("Remote log is not available")));
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
