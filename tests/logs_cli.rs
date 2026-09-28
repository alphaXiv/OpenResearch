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
