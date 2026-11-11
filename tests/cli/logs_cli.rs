use std::path::PathBuf;
use std::process::{Command, Output};

struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("orx-logs-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        Self(root)
    }
    fn data_dir(&self) -> PathBuf {
        self.0.join("data")
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_orx"));
        crate::home::isolate_home(&mut cmd, &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("ORX_DATA_DIR", self.data_dir())
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
    fn init_store(&self) {
        let _ = self.run(&["logs", "not-a-registered-run-id"]);
    }
    fn seed_run(&self, run_id: &str) {
        self.init_store();
        let db = self.data_dir().join("orx.db");
        let conn = rusqlite::Connection::open(db).unwrap();
        let now = 1_700_000_000_000i64;
        conn.execute(
            "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, github_sync_enabled, baseline_branch, repo_path, created_at, updated_at)
             VALUES (?1, 'P', ?2, 'o', 'r', 1, 'main', '/tmp/repo', ?3, ?3)",
            rusqlite::params!["proj-1", "slug-proj-1", now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, agent_status, created_at, updated_at)
             VALUES (?1, 'proj-1', ?2, 'orx/e1', 'echo', 'idle', ?3, ?3)",
            rusqlite::params!["exp-1", "exp-slug-1", now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command, created_at, updated_at)
             VALUES (?1, 'exp-1', 'proj-1', 'done', '{}', 'echo', ?2, ?2)",
            rusqlite::params![run_id, now],
        )
        .unwrap();
    }
    fn write_log(&self, run_id: &str, bytes: &[u8]) -> PathBuf {
        let dir = self.data_dir().join("run-logs");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{run_id}.log"));
        std::fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        #[cfg(unix)]
        orx_test_support::stop_server(&self.data_dir());
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lossy_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn default_summary_shows_bounded_path_size_preview_and_hint() {
    let sandbox = Sandbox::new();
    let run_id = "run-default-summary";
    sandbox.seed_run(run_id);
    let early = b"EARLY_SENTINEL_";
    let old_tail = b"OLD_64K_WINDOW_SENTINEL";
    let late = b"LATE_SENTINEL_END";
    let mut body = vec![b'x'; 70 * 1024];
    body[..early.len()].copy_from_slice(early);
    let old_tail_start = body.len() - 1024;
    body[old_tail_start..old_tail_start + old_tail.len()].copy_from_slice(old_tail);
    let late_start = body.len() - late.len();
    body[late_start..].copy_from_slice(late);
    let path = sandbox.write_log(run_id, &body);
    let output = sandbox.run(&["logs", run_id]);
    assert!(output.status.success(), "{}", lossy_stdout(&output));
    let stdout = lossy_stdout(&output);
    assert!(stdout.contains(&format!(
        "This is the path to the full log file: {}",
        path.display()
    )));
    assert!(stdout.contains(&format!("It is {} bytes.", body.len())));
    assert!(stdout.contains(std::str::from_utf8(late).unwrap()));
    assert!(!stdout.contains(std::str::from_utf8(early).unwrap()));
    assert!(!stdout.contains(std::str::from_utf8(old_tail).unwrap()));
    let report = stdout
        .split_once("Here are the last 500 characters of the log file:\n")
        .unwrap()
        .1;
    let (preview, _) = report.split_once("\nUse targeted search").unwrap();
    assert_eq!(preview.chars().count(), 500);
    assert!(stdout.contains("targeted search"));
    assert!(stdout.contains("Avoid reading the entire log file at once into the context window"));
    assert!(output.stderr.is_empty());
}

#[test]
fn utf8_preview_respects_character_boundaries() {
    let sandbox = Sandbox::new();
    let run_id = "run-utf8";
    sandbox.seed_run(run_id);
    let prefix = "a".repeat(100);
    let body = format!("{prefix}🚀{}", "z".repeat(2046));
    assert!(body.len() > 2048);
    assert_eq!(body.len() - 2048, prefix.len() + 2);
    sandbox.write_log(run_id, body.as_bytes());
    let output = sandbox.run(&["logs", run_id]);
    assert!(output.status.success());
    let stdout = lossy_stdout(&output);
    let report = stdout
        .split_once("Here are the last 500 characters of the log file:\n")
        .unwrap()
        .1;
    let (preview, _) = report.split_once("\nUse targeted search").unwrap();
    let expected_preview: String = body
        .chars()
        .rev()
        .take(500)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    assert_eq!(preview, expected_preview);
    assert!(!preview.contains('\u{fffd}'));
}

#[cfg(unix)]
#[test]
fn only_not_found_log_open_errors_are_reported_as_missing() {
    use std::os::unix::fs::symlink;

    let sandbox = Sandbox::new();
    let run_id = "run-log-open-error";
    sandbox.seed_run(run_id);
    let path = sandbox
        .data_dir()
        .join("run-logs")
        .join(format!("{run_id}.log"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&path, &path).unwrap();

    let output = sandbox.run(&["logs", run_id]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("no log captured yet"), "{stderr}");
}

#[test]
fn small_and_empty_logs_preview_without_missing_message() {
    let sandbox = Sandbox::new();
    let run_id = "run-small-empty";
    sandbox.seed_run(run_id);

    let tiny = "tiny-log-line\n";
    let path = sandbox.write_log(run_id, tiny.as_bytes());
    let small = sandbox.run(&["logs", run_id]);
    assert!(small.status.success());
    let stdout = lossy_stdout(&small);
    assert!(stdout.contains(tiny.trim_end()));
    assert!(stdout.contains(path.to_str().unwrap()));

    sandbox.write_log(run_id, b"");
    let empty = sandbox.run(&["logs", run_id]);
    assert!(empty.status.success());
    let empty_out = lossy_stdout(&empty);
    assert!(empty_out.contains("0 bytes"));
    assert!(!empty_out.contains("no log captured"));
}

#[test]
fn missing_unknown_and_removed_flags() {
    let sandbox = Sandbox::new();
    let run_id = "run-missing-log";
    sandbox.seed_run(run_id);
    let missing = sandbox.run(&["logs", run_id]);
    assert!(missing.status.success());
    assert!(missing.stdout.is_empty());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("no log captured yet"));

    let unknown = sandbox.run(&["logs", "not-registered-run"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("not found"));

    let flags = sandbox.run(&["logs", run_id, "--full"]);
    assert!(!flags.status.success());
}

#[test]
fn large_kept_log_previews_tail_and_identifies_both_ranges_and_missing_middle() {
    use std::io::{Seek, SeekFrom, Write};

    let sandbox = Sandbox::new();
    let run_id = "run-large-kept";
    sandbox.seed_run(run_id);
    let head = sandbox.write_log(run_id, b"SETUP_SENTINEL");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&head)
        .unwrap()
        .set_len(64 << 20)
        .unwrap();
    let tail = head.with_file_name(format!("{run_id}.tail.log"));
    let mut file = std::fs::File::create(&tail).unwrap();
    file.set_len(256 << 20).unwrap();
    let result = b"TRAINING_COMPLETE_SENTINEL\n";
    file.seek(SeekFrom::End(-(result.len() as i64))).unwrap();
    file.write_all(result).unwrap();
    let total: u64 = 3 << 30;
    let ranges = serde_json::json!([
        {"offset":0,"len":64 << 20,"file":head,"ident":"1-2","eof":false},
        {"offset":total - (256 << 20),"len":256 << 20,"file":tail,"ident":"1-2","eof":true}
    ]);
    std::fs::write(
        head.with_file_name(format!("{run_id}.ranges.json")),
        ranges.to_string(),
    )
    .unwrap();
    let output = sandbox.run(&["logs", run_id]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = lossy_stdout(&output);
    for expected in [
        "Original log: 3221225472 bytes.",
        "Kept bytes 0..67108864:",
        "Kept bytes 2952790016..3221225472:",
        head.to_str().unwrap(),
        tail.to_str().unwrap(),
        "The middle of the log is not on this computer.",
        "TRAINING_COMPLETE_SENTINEL",
    ] {
        assert!(stdout.contains(expected), "missing {expected}");
    }
    assert!(!stdout.contains("SETUP_SENTINEL"));
    assert!(!stdout.contains("full log file"));
}

#[test]
fn a_closed_stdout_ends_short_commands_quietly() {
    let sandbox = Sandbox::new();
    let run_id = "run-closed-stdout";
    sandbox.seed_run(run_id);
    sandbox.write_log(run_id, b"line\n");
    for args in [&["logs", run_id][..], &["projects"]] {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        let output = sandbox
            .command()
            .args(args)
            .stdout(writer)
            .stderr(std::process::Stdio::piped())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.is_empty(), "{args:?}: {stderr}");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt as _;
            assert_eq!(output.status.signal(), Some(libc::SIGPIPE), "{args:?}");
        }
        #[cfg(windows)]
        assert!(output.status.success(), "{args:?}: {}", output.status);
    }
}

#[test]
fn read_only_commands_work_on_a_read_only_store() {
    let sandbox = Sandbox::new();
    let run_id = "run-read-only-store";
    sandbox.seed_run(run_id);
    sandbox.write_log(run_id, b"READ_ONLY_SENTINEL\n");
    let listed = sandbox.run(&["runs", "proj-1"]);
    assert!(
        listed.status.success(),
        "{}",
        String::from_utf8_lossy(&listed.stderr)
    );
    for name in ["orx.db", "orx.db-wal", "orx.db-shm"] {
        let path = sandbox.data_dir().join(name);
        if let Ok(metadata) = std::fs::metadata(&path) {
            let mut permissions = metadata.permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
    }
    for args in [
        &["runs", "proj-1"][..],
        &["projects"],
        &["project", "view", "proj-1"],
        &["exp", "status", "exp-1"],
        &["logs", run_id],
    ] {
        let output = sandbox.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert!(lossy_stdout(&sandbox.run(&["runs", "proj-1"])).contains(run_id));
    assert!(lossy_stdout(&sandbox.run(&["logs", run_id])).contains("READ_ONLY_SENTINEL"));

    // Nothing listens on the port, so each wait fails only once it has read the store.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = closed.local_addr().unwrap().port().to_string();
    drop(closed);
    for target in [&["exp-1"][..], &["--project", "proj-1"]] {
        let output = sandbox
            .command()
            .env("ORX_UP_PORT", &port)
            .env("ORX_UP_AUTH_TOKEN", "token")
            .args(["exp", "wait", "--timeout", "60"])
            .args(target)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("127.0.0.1:{port}")),
            "{target:?}: {stderr}"
        );
    }
}

#[test]
fn an_unusable_config_dir_is_named_in_the_error() {
    let sandbox = Sandbox::new();
    let config = sandbox.0.join("config/openresearch");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "").unwrap();
    let output = sandbox.run(&["projects"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(config.to_str().unwrap()), "{stderr}");
}
