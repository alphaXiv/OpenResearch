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
        cmd.env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
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
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lossy_stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn default_summary_shows_path_size_tail_hint_not_whole_tail() {
    let sandbox = Sandbox::new();
    let run_id = "run-default-summary";
    sandbox.seed_run(run_id);
    let early = "EARLY_SENTINEL_";
    let late = "LATE_SENTINEL_END";
    let filler = "x".repeat(70 * 1024 - early.len() - late.len());
    let body = format!("{early}{filler}{late}");
    let path = sandbox.write_log(run_id, body.as_bytes());
    let output = sandbox.run(&["logs", run_id]);
    assert!(output.status.success(), "{}", lossy_stdout(&output));
    let stdout = lossy_stdout(&output);
    assert!(stdout.contains(path.to_str().unwrap()));
    assert!(stdout.contains(&format!("{} bytes", body.len())));
    assert!(stdout.contains(late));
    assert!(!stdout.contains(early));
    assert!(stdout.contains("targeted search"));
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_raw_modes_keep_stdout_clean_and_stderr_footer() {
    let sandbox = Sandbox::new();
    let run_id = "run-raw-modes";
    sandbox.seed_run(run_id);
    let mut body: Vec<u8> = (0..80 * 1024).map(|i| (i % 256) as u8).collect();
    body.push(b'\n');
    sandbox.write_log(run_id, &body);
    let file_len = body.len();

    let head = sandbox.run(&["logs", run_id, "--head"]);
    assert!(head.status.success(), "{}", lossy_stdout(&head));
    assert_eq!(&head.stdout[..64 * 1024], &body[..64 * 1024]);
    assert_eq!(head.stdout.len(), 64 * 1024 + 1);
    assert_eq!(head.stdout[64 * 1024], b'\n');
    assert!(String::from_utf8_lossy(&head.stderr).contains("[local file] bytes"));
    assert!(!lossy_stdout(&head).contains("targeted search"));

    let tail = sandbox.run(&["logs", run_id, "--bytes", "4096"]);
    assert!(tail.status.success());
    assert_eq!(&tail.stdout, &body[file_len - 4096..file_len]);

    let window = sandbox.run(&["logs", run_id, "--range", "100:200"]);
    assert!(window.status.success());
    assert!(window.stdout.starts_with(&body[100..200]));

    let full = sandbox.run(&["logs", run_id, "--full"]);
    assert!(full.status.success());
    assert_eq!(full.stdout, body);
    let stderr = String::from_utf8_lossy(&full.stderr);
    assert!(stderr.contains("bytes 0–"));
}

#[test]
fn utf8_preview_respects_character_boundaries_and_raw_modes_stay_lossless() {
    let sandbox = Sandbox::new();
    let run_id = "run-utf8";
    sandbox.seed_run(run_id);
    let marker = "🚀";
    let prefix = "a".repeat(600);
    let body = format!("{prefix}{marker}");
    sandbox.write_log(run_id, body.as_bytes());
    let output = sandbox.run(&["logs", run_id]);
    assert!(output.status.success());
    let stdout = lossy_stdout(&output);
    assert!(stdout.contains(marker));
    assert!(stdout.chars().filter(|c| *c == '🚀').count() >= 1);

    let invalid = b"ok\xff\xfeok";
    sandbox.write_log(run_id, invalid);
    let raw = sandbox.run(&["logs", run_id, "--range", "0:7"]);
    assert_eq!(&raw.stdout[..invalid.len()], invalid);
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
fn missing_unknown_and_invalid_args() {
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

    assert!(!sandbox
        .run(&["logs", run_id, "--range", "bad"])
        .status
        .success());
    assert!(!sandbox
        .run(&["logs", run_id, "--bytes", "nope"])
        .status
        .success());
    assert!(!sandbox
        .run(&["logs", run_id, "--full", "--head"])
        .status
        .success());
}
