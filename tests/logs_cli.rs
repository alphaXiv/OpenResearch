use std::io::{Read as _, Write as _};
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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

struct TestChild(std::process::Child);
impl Deref for TestChild {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for TestChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl Drop for TestChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
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
    assert!(stdout.contains(path.to_str().unwrap()));
    assert!(stdout.contains(&format!("{} bytes", body.len())));
    assert!(stdout.contains(std::str::from_utf8(late).unwrap()));
    assert!(!stdout.contains(std::str::from_utf8(early).unwrap()));
    assert!(!stdout.contains(std::str::from_utf8(old_tail).unwrap()));
    let report = stdout.split_once("\n\n").unwrap().1;
    let (preview, _) = report.split_once("\nUse targeted search").unwrap();
    assert_eq!(preview.chars().count(), 500);
    assert!(stdout.contains("targeted search"));
    assert!(output.stderr.is_empty());
}

#[test]
fn explicit_raw_modes_keep_stdout_clean_and_stderr_footer() {
    let sandbox = Sandbox::new();
    let run_id = "run-raw-modes";
    sandbox.seed_run(run_id);
    let body: Vec<u8> = (0..80 * 1024).map(|i| (i % 256) as u8).collect();
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
    let mut expected_tail = body[file_len - 4096..file_len].to_vec();
    expected_tail.push(b'\n');
    assert_eq!(tail.stdout, expected_tail);

    let window = sandbox.run(&["logs", run_id, "--range", "100:200"]);
    assert!(window.status.success());
    let mut expected_window = body[100..200].to_vec();
    expected_window.push(b'\n');
    assert_eq!(window.stdout, expected_window);

    let head_bytes = sandbox.run(&["logs", run_id, "--head", "--bytes", "23"]);
    assert!(head_bytes.status.success());
    let mut expected_head_bytes = body[..23].to_vec();
    expected_head_bytes.push(b'\n');
    assert_eq!(head_bytes.stdout, expected_head_bytes);

    let head_range = sandbox.run(&["logs", run_id, "--head", "--range", "100:200"]);
    assert!(head_range.status.success());
    assert_eq!(head_range.stdout, expected_window);
    assert!(String::from_utf8_lossy(&head_range.stderr).contains("bytes 100–200 of 81920"));

    let full = sandbox.run(&["logs", run_id, "--full"]);
    assert!(full.status.success());
    let mut expected_full = body;
    expected_full.push(b'\n');
    assert_eq!(full.stdout, expected_full);
    assert_eq!(
        String::from_utf8_lossy(&full.stderr),
        format!("[local file] bytes 0–{file_len} of {file_len}\n")
    );
}

#[test]
fn raw_modes_preserve_newline_terminated_stdout_exactly() {
    let sandbox = Sandbox::new();
    let run_id = "run-raw-newline";
    sandbox.seed_run(run_id);
    let mut body = vec![b'x'; 70 * 1024];
    body[64 * 1024 - 1] = b'\n';
    body[100..200].fill(b'r');
    body[199] = b'\n';
    let last = body.len() - 1;
    body[last] = b'\n';
    sandbox.write_log(run_id, &body);

    let full = sandbox.run(&["logs", run_id, "--full"]);
    assert!(full.status.success());
    assert_eq!(full.stdout, body);

    let bytes = sandbox.run(&["logs", run_id, "--bytes", "4096"]);
    assert!(bytes.status.success());
    assert_eq!(bytes.stdout, body[body.len() - 4096..]);

    let range = sandbox.run(&["logs", run_id, "--range", "100:200"]);
    assert!(range.status.success());
    assert_eq!(range.stdout, body[100..200]);

    let head = sandbox.run(&["logs", run_id, "--head"]);
    assert!(head.status.success());
    assert_eq!(head.stdout, body[..64 * 1024]);
}

#[test]
fn full_reports_error_after_log_is_truncated_while_stdout_is_gated() {
    let sandbox = Sandbox::new();
    let run_id = "run-full-truncated";
    sandbox.seed_run(run_id);
    let body = vec![b'x'; 16 * 1024 * 1024];
    let original_len = body.len();
    let path = sandbox.write_log(run_id, &body);

    let mut child = TestChild(
        sandbox
            .command()
            .args(["logs", run_id, "--full"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdout = child.stdout.take().unwrap();
    let (chunk_tx, chunk_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut output = vec![0; 64 * 1024];
        let first_chunk = stdout.read_exact(&mut output);
        chunk_tx.send(first_chunk.is_ok()).unwrap();
        if first_chunk.is_err() {
            return output;
        }
        if resume_rx.recv().is_err() {
            return output;
        }
        stdout.read_to_end(&mut output).unwrap();
        output
    });

    match chunk_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => {}
        result => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = resume_tx.send(());
            let _ = reader.join();
            panic!("--full did not emit a complete stdout chunk: {result:?}");
        }
    }

    // With the reader gated after one chunk, a large source must fill the OS
    // pipe and block the child before it can reach EOF. Check that condition
    // rather than truncating merely after the first observed byte.
    thread::sleep(Duration::from_millis(250));
    if let Some(status) = child.try_wait().unwrap() {
        let _ = resume_tx.send(());
        let _ = reader.join();
        let mut stderr = Vec::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        panic!(
            "--full exited before truncation ({status}): {}",
            String::from_utf8_lossy(&stderr)
        );
    }

    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(0)
        .unwrap();
    resume_tx.send(()).unwrap();

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                panic!("--full child exceeded the 15-second timeout after truncation");
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let output = reader.join().unwrap();
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();
    let stderr = String::from_utf8_lossy(&stderr);

    assert!(
        !status.success(),
        "--full accepted a truncated log: {stderr}"
    );
    assert!(output.len() < original_len);
    assert!(
        stderr.contains(&format!("expected {original_len} bytes")),
        "{stderr}"
    );
    assert!(!stderr.contains(&format!("bytes 0–{original_len} of {original_len}")));
}

// Process-level coverage complements the deterministic read-boundary unit test in
// commands::logs: the latter proves the append arrives before source EOF.
#[test]
fn full_does_not_stream_bytes_appended_after_open() {
    let sandbox = Sandbox::new();
    let run_id = "run-full-append";
    sandbox.seed_run(run_id);
    let body = vec![b'x'; 8 * 1024 * 1024];
    let original_len = body.len();
    let path = sandbox.write_log(run_id, &body);

    let mut child = TestChild(
        sandbox
            .command()
            .args(["logs", run_id, "--full"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdout = child.stdout.take().unwrap();
    let (first_byte_tx, first_byte_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut output = Vec::new();
        let mut first_byte = [0u8; 1];
        let count = stdout.read(&mut first_byte).unwrap();
        first_byte_tx.send(count).unwrap();
        resume_rx.recv().unwrap();
        output.extend_from_slice(&first_byte[..count]);
        stdout.read_to_end(&mut output).unwrap();
        output
    });

    match first_byte_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(1) => {}
        result => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = resume_tx.send(());
            let _ = reader.join();
            panic!("--full did not start writing log data: {result:?}");
        }
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"APPENDED_AFTER_OPEN")
        .unwrap();
    resume_tx.send(()).unwrap();

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                panic!("--full child exceeded the 15-second timeout");
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    };
    let output = reader.join().unwrap();
    let mut stderr = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut stderr)
        .unwrap();

    assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
    let mut expected = body;
    expected.push(b'\n');
    assert_eq!(
        output.len(),
        expected.len(),
        "streamed {} bytes, expected {} bytes",
        output.len(),
        expected.len()
    );
    assert!(
        output == expected,
        "streamed bytes differ despite matching length"
    );
    assert!(!output
        .windows(b"APPENDED_AFTER_OPEN".len())
        .any(|window| window == b"APPENDED_AFTER_OPEN"));
    assert_eq!(
        String::from_utf8_lossy(&stderr),
        format!("[local file] bytes 0–{original_len} of {original_len}\n")
    );
}

#[test]
fn utf8_preview_respects_character_boundaries_and_raw_modes_stay_lossless() {
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
    let report = stdout.split_once("\n\n").unwrap().1;
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

    let invalid = b"ok\xff\xfeok";
    sandbox.write_log(run_id, invalid);
    let raw = sandbox.run(&["logs", run_id, "--range", "0:7"]);
    assert_eq!(&raw.stdout[..invalid.len()], invalid);
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

    for args in [vec!["logs", run_id], vec!["logs", run_id, "--full"]] {
        let output = sandbox.run(&args);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("no log captured yet"), "{stderr}");
    }
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
