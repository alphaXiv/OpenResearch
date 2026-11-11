#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use orx_test_support::Sshd;
use sha2::Digest;

const RUN_FREE: &str = "00000000-0000-4000-8000-0000000000f1";
const RUN_HELD: &str = "00000000-0000-4000-8000-0000000000a2";

struct Sandbox {
    root: PathBuf,
    server: Sshd,
}

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("orx-engine-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".ssh")).unwrap();
        Self {
            root,
            server: Sshd::start().unwrap(),
        }
    }

    fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_orx"));
        crate::home::isolate_home(&mut command, &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("ORX_DATA_DIR", self.data())
            .env("ORX_NO_UPDATE_CHECK", "1")
            .env("ORX_SSH_CONFIG", self.server.config_path())
            .env("ORX_HOST_BIN_DIR", orx_test_support::host_binaries())
            .env("ORX_CHAT_SESSION_ID", "chat")
            .env_remove("ORX_LOCAL_SESSION")
            .env_remove("ORX_UP_PORT")
            .env_remove("ORX_UP_AUTH_TOKEN")
            .env_remove("HF_TOKEN")
            .arg("--no-telemetry");
        command
    }

    fn stop_service(&self) -> Output {
        crate::home::isolate_home(&mut Command::new(env!("CARGO_BIN_EXE_orx")), &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("ORX_DATA_DIR", self.data())
            .args(["ssh-service", "stop"])
            .output()
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn db(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(self.data().join("orx.db")).unwrap()
    }

    fn status(&self, run: &str) -> String {
        self.db()
            .query_row("SELECT status FROM runs WHERE id = ?1", [run], |row| {
                row.get(0)
            })
            .unwrap()
    }

    fn terminal_notices(&self, run: &str) -> Vec<String> {
        let db = self.db();
        let mut statement = db
            .prepare(
                "SELECT body FROM run_notices
                 WHERE run_id = ?1 AND chat_session_id = 'chat' AND kind = 'terminal'",
            )
            .unwrap();
        statement
            .query_map([run], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// Project `project` with experiment `exp`, whose run command is `bash train.sh`.
    fn project(&self, train: &str) {
        assert!(self.run(&["projects", "--json"]).status.success());
        let repo = self.root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("train.sh"), train).unwrap();
        git(&repo, &["add", "train.sh"]);
        git(&repo, &["commit", "-m", "train"]);
        let db = self.db();
        db.execute(
            "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, github_sync_enabled,
                                         baseline_branch, repo_path, created_at, updated_at)
             VALUES ('project', 'P', 'p', 'o', 'r', 0, 'main', ?1, 1, 1)",
            [repo.to_str().unwrap()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, agent_status,
                                            created_at, updated_at)
             VALUES ('exp', 'project', 'exp', 'main', 'bash train.sh', 'idle', 1, 1)",
            [],
        )
        .unwrap();
    }

    /// A run row and remote run dir shaped like orx 0.2.x left them.
    fn legacy_run(&self, id: &str, exit_code: Option<&str>) {
        let dir = self.server.home().join(".orx/runs").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("log"), "legacy output\n").unwrap();
        if let Some(code) = exit_code {
            std::fs::write(dir.join("exit_code"), code).unwrap();
        }
        let backend = serde_json::json!({
            "kind": "ssh_job",
            "namespace": self.server.alias(),
            "jobId": format!(".orx/runs/{id}"),
        });
        self.db()
            .execute(
                "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command,
                                   created_at, updated_at, chat_session_id)
                 VALUES (?1, 'exp', 'project', 'running', ?2, 'train', 1, 1, 'chat')",
                rusqlite::params![id, backend.to_string()],
            )
            .unwrap();
    }

    /// Where the autostarted server keeps its runtime files for this data directory.
    fn runtime(&self, extension: &str) -> Option<PathBuf> {
        let data = std::fs::canonicalize(self.data()).ok()?;
        let digest = sha2::Sha256::digest(data.as_os_str().as_encoded_bytes());
        let name: String = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let uid = unsafe { libc::geteuid() };
        Some(PathBuf::from(format!("/tmp/orx-{uid}/{name}.{extension}")))
    }

    fn descriptor(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&std::fs::read(self.runtime("server.json")?).ok()?).ok()
    }

    fn server_log(&self) -> String {
        self.runtime("log")
            .and_then(|path| std::fs::read_to_string(path).ok())
            .unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        orx_test_support::stop_server(&self.data());
        let _ = self.stop_service();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn wait_for(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

fn git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(repo)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn diagnostics(output: &Output) -> String {
    format!(
        "stdout: {}\nstderr: {}",
        stdout(output),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The engine end to end: `orx exp run` autostarts the server, which launches over SSH through
/// `orx-host`; `orx exp wait` returns from one event stream; the launching chat gets exactly
/// one result. Startup recovers legacy supervisors; a second `orx up` attaches.
#[test]
fn ssh_runs_go_through_the_autostarted_engine() {
    let sandbox = Sandbox::new();
    sandbox.project("echo engine-says-hi\necho step 2\n");
    sandbox.legacy_run(RUN_FREE, Some("0\n"));
    sandbox.legacy_run(RUN_HELD, None);
    let locks = sandbox.data().join("run-logs");
    std::fs::create_dir_all(&locks).unwrap();
    let held = std::fs::File::create(locks.join(format!("{RUN_HELD}.supervisor.lock"))).unwrap();
    held.try_lock().unwrap();
    let shared =
        std::fs::File::create(locks.join(format!("{RUN_HELD}.supervisor.shared-ssh.lock")))
            .unwrap();
    shared.try_lock().unwrap();

    let launched = sandbox.run(&[
        "exp",
        "run",
        "exp",
        "--force",
        "--backend",
        "ssh",
        "--host",
        sandbox.server.alias(),
    ]);
    assert!(launched.status.success(), "{}", diagnostics(&launched));
    let run = stdout(&launched)
        .split_whitespace()
        .find(|word| word.len() == 36 && word.matches('-').count() == 4)
        .unwrap()
        .to_owned();

    let started = Instant::now();
    let waited = sandbox.run(&["exp", "wait", "exp", "--timeout", "120"]);
    assert!(waited.status.success(), "{}", diagnostics(&waited));
    assert!(
        stdout(&waited).contains(&format!("{run} done")),
        "{}",
        diagnostics(&waited)
    );
    assert!(started.elapsed() < Duration::from_secs(90));
    assert!(
        wait_for(Duration::from_secs(10), || sandbox
            .terminal_notices(&run)
            .len()
            == 1),
        "{:?}",
        sandbox.terminal_notices(&run)
    );
    let notices = sandbox.terminal_notices(&run);
    assert!(notices[0].contains("engine-says-hi"), "{notices:?}");

    // With final output kept, the run's remote dir is queued for retention cleanup.
    let queued = || -> i64 {
        sandbox
            .db()
            .query_row(
                "SELECT COUNT(*) FROM remote_cleanups WHERE run_id = ?1",
                [&run],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert!(
        wait_for(Duration::from_secs(60), || queued() == 1),
        "{}",
        sandbox.server_log()
    );

    // The dashboard reads the log the engine kept when the run finished.
    let port = sandbox.descriptor().unwrap()["port"].as_u64().unwrap();
    let page = Command::new("curl")
        .args([
            "-sf",
            &format!("http://127.0.0.1:{port}/api/runs/{run}/log?start=0&end=1048576"),
        ])
        .output()
        .unwrap();
    assert!(page.status.success(), "{}", diagnostics(&page));
    let page: serde_json::Value = serde_json::from_slice(&page.stdout).unwrap();
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(page["data"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes, b"engine-says-hi\nstep 2\n");
    assert_eq!(page["kind"], "data");
    assert_eq!(page["start"], 0);
    assert_eq!(page["len"], bytes.len());
    assert_eq!(page["kept"], serde_json::json!([[0, bytes.len()]]));
    assert_eq!(page["eof"], true);
    // `exp wake` from another chat subscribes it; a finished run's result is queued at once.
    sandbox
        .db()
        .execute(
            "INSERT INTO chat_sessions (id, project_id, harness, created_at, updated_at)
             VALUES ('other', 'project', 'codex', 1, 1)",
            [],
        )
        .unwrap();
    for _ in 0..2 {
        let woke = sandbox
            .command()
            .env("ORX_LOCAL_SESSION", "1")
            .env("ORX_CHAT_SESSION_ID", "other")
            .args(["exp", "wake", "exp"])
            .output()
            .unwrap();
        assert!(woke.status.success(), "{}", diagnostics(&woke));
    }
    let other: i64 = sandbox
        .db()
        .query_row(
            "SELECT COUNT(*) FROM run_notices WHERE run_id = ?1 AND chat_session_id = 'other'",
            [&run],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(other, 1);

    assert!(
        wait_for(Duration::from_secs(60), || sandbox.status(RUN_FREE)
            == "done"),
        "{}\n{}",
        sandbox.status(RUN_FREE),
        sandbox.server_log()
    );
    assert_eq!(sandbox.terminal_notices(RUN_FREE).len(), 1);
    let kept = sandbox.data().join(format!("run-logs/{RUN_FREE}.log"));
    assert_eq!(std::fs::read_to_string(kept).unwrap(), "legacy output\n");
    assert_eq!(sandbox.status(RUN_HELD), "running");
    let watched: i64 = sandbox
        .db()
        .query_row(
            "SELECT COUNT(*) FROM legacy_watches WHERE run_id = ?1",
            [RUN_HELD],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(watched, 1);

    // The old supervisor finishes its run; the engine only reads it and tells the chat.
    sandbox
        .db()
        .execute(
            "UPDATE runs SET status = 'done', exit_code = 0, updated_at = 2, ended_at = 2
             WHERE id = ?1",
            [RUN_HELD],
        )
        .unwrap();
    assert!(wait_for(Duration::from_secs(20), || {
        sandbox.terminal_notices(RUN_HELD).len() == 1
    }));
    drop(held);

    let mut second = sandbox
        .command()
        .args(["up", "--no-browser", "--port", "0"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    assert!(
        wait_for(Duration::from_secs(20), || second
            .try_wait()
            .unwrap()
            .is_some()),
        "a second orx up started another server"
    );
    let attached = second.wait_with_output().unwrap();
    assert!(attached.status.success(), "{}", diagnostics(&attached));
    assert!(String::from_utf8_lossy(&attached.stderr).contains("dashboard on http://127.0.0.1:"));
}

/// A local run's output reaches `orx logs -f` while it runs, not only once it ends.
#[test]
fn local_logs_stream_while_the_run_is_live() {
    use std::io::{BufRead, BufReader};
    let sandbox = Sandbox::new();
    let (proceed, release) = (sandbox.root.join("proceed"), sandbox.root.join("release"));
    sandbox.project(&format!(
        "echo first\nwhile [ ! -f '{}' ]; do sleep 0.2; done\necho second\n\
         while [ ! -f '{}' ]; do sleep 0.2; done\n",
        proceed.display(),
        release.display()
    ));
    let launched = sandbox.run(&["exp", "run", "exp", "--force", "--backend", "local"]);
    assert!(launched.status.success(), "{}", diagnostics(&launched));
    let run = stdout(&launched)
        .split_whitespace()
        .find(|word| word.len() == 36 && word.matches('-').count() == 4)
        .unwrap()
        .to_owned();
    /// A follower left running would autostart a new server once the sandbox stops its own.
    struct Follower(std::process::Child);
    impl Drop for Follower {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut follow = Follower(
        sandbox
            .command()
            .args(["logs", &run, "-f"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let (lines, received) = std::sync::mpsc::channel();
    let output = follow.0.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            if lines.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    let first = std::iter::from_fn(|| received.recv_timeout(Duration::from_secs(60)).ok())
        .find(|line| line.contains("first"));
    assert!(first.is_some(), "{}", sandbox.server_log());
    // Past its last phase change, only log growth can wake the follower.
    assert!(wait_for(Duration::from_secs(30), || sandbox.status(&run)
        == "running"));
    std::fs::write(&proceed, "").unwrap();
    let second = std::iter::from_fn(|| received.recv_timeout(Duration::from_secs(20)).ok())
        .find(|line| line.contains("second"));
    assert!(second.is_some(), "{}", sandbox.server_log());
    let status = sandbox.status(&run);
    assert!(
        matches!(status.as_str(), "starting" | "running"),
        "{status}"
    );
    std::fs::write(&release, "").unwrap();
    assert!(wait_for(Duration::from_secs(30), || follow
        .0
        .try_wait()
        .unwrap()
        .is_some()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_labels_agree_in_runs_status_and_wait() {
    use axum::{routing::get, Json, Router};
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    let sandbox = Sandbox::new();
    assert!(sandbox.run(&["projects", "--json"]).status.success());
    let db = sandbox.db();
    db.execute("INSERT INTO local_projects (id,name,slug,github_owner,github_repo,repo_path,created_at,updated_at) VALUES ('project','P','p','','',?1,1,1)", [sandbox.root.to_str().unwrap()]).unwrap();
    db.execute_batch("INSERT INTO local_experiments (id,project_id,slug,branch_name,run_command,agent_status,created_at,updated_at) VALUES ('exp','project','exp','main','sleep 60','idle',1,1);
        INSERT INTO runs (id,experiment_id,project_id,status,backend_json,command,created_at,updated_at) VALUES ('run','exp','project','running','{\"kind\":\"local_job\"}','sleep 60',1,1)").unwrap();
    let value = Arc::new(Mutex::new(Value::Null));
    let (rows, events) = (value.clone(), value.clone());
    let app = Router::new()
        .route(
            "/api/v2/runs",
            get(move || {
                let run = rows.lock().unwrap().clone();
                async move { Json(json!({"runs":[run], "titles":{}})) }
            }),
        )
        .route(
            "/api/v2/events",
            get(move || {
                let run = events.lock().unwrap().clone();
                async move {
                    (
                        [("content-type", "text/event-stream")],
                        format!("event: run\ndata: {run}\n\n"),
                    )
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (status, contact, cancelling, unconfirmed, note, label) in [
        (
            "running",
            json!({"type":"live"}),
            true,
            false,
            None,
            "Cancelling…",
        ),
        (
            "running",
            json!({"type":"unreachable","since":1,"cause":"Offline"}),
            true,
            false,
            None,
            "Cancellation pending",
        ),
        (
            "running",
            json!({"type":"needs_auth","since":1,"cause":"Connect"}),
            true,
            false,
            None,
            "Cancellation pending",
        ),
        (
            "submitting",
            json!({"type":"live"}),
            true,
            false,
            None,
            "Cancellation pending",
        ),
        (
            "submitting",
            json!({"type":"unreconciled","since":1,"detail":"No handle"}),
            true,
            false,
            None,
            "Cancellation pending",
        ),
        (
            "running",
            json!({"type":"unreconciled","since":1,"detail":"Stop pending"}),
            true,
            false,
            None,
            "Cancelling…",
        ),
        (
            "cancelled",
            json!({"type":"stopping","since":1,"count":2}),
            false,
            false,
            None,
            "Not fully stopped · 2 processes still running on localhost",
        ),
        (
            "cancelled",
            json!({"type":"stopping","since":1,"count":1}),
            false,
            false,
            None,
            "Not fully stopped · 1 process still running on localhost",
        ),
        (
            "cancelled",
            json!({"type":"live"}),
            false,
            false,
            None,
            "Cancelled",
        ),
        (
            "cancelled",
            json!({"type":"live"}),
            false,
            true,
            Some("Container processes couldn't be checked."),
            "Cancelled · couldn't confirm",
        ),
        (
            "cancelled",
            json!({"type":"live"}),
            false,
            false,
            Some("Editable result."),
            "Cancelled",
        ),
    ] {
        *value.lock().unwrap() = json!({"id":"run", "experimentId":"exp", "status":status,
            "contact":contact, "cancelling":cancelling,
            "cancelUnconfirmed":if unconfirmed { json!([]) } else { Value::Null },
            "host":null, "commitSha":null, "durationSecs":1,
            "updatedDisplay":"now", "cancelReason":null, "resultMarkdown":note});
        let stored_contact = (contact["type"] != "live").then(|| contact.to_string());
        db.execute(
            "UPDATE runs SET status=?1,contact=?2,cancel_requested=?3,cancel_unconfirmed=?4,result_markdown=?5 WHERE id='run'",
            rusqlite::params![status, stored_contact, cancelling, unconfirmed.then_some("[]"), note],
        ).unwrap();
        for args in [
            vec!["runs", "project"],
            vec!["exp", "status", "exp"],
            vec![
                "exp",
                "wait",
                "exp",
                "--timeout",
                if cancelling { "3" } else { "10" },
            ],
        ] {
            let output = sandbox
                .command()
                .env("ORX_UP_PORT", port.to_string())
                .env("ORX_UP_AUTH_TOKEN", "test-only")
                .args(&args)
                .output()
                .unwrap();
            assert_eq!(
                output.status.success(),
                !(cancelling && args[1] == "wait"),
                "{args:?}: {}",
                diagnostics(&output)
            );
            assert_eq!(
                format!(
                    "{}{}",
                    stdout(&output),
                    String::from_utf8_lossy(&output.stderr)
                )
                .matches(label)
                .count(),
                1,
                "{args:?}: {}",
                diagnostics(&output)
            );
            if unconfirmed {
                assert!(
                    diagnostics(&output).contains(note.unwrap()),
                    "{args:?}: {}",
                    diagnostics(&output)
                );
            }
        }
    }
    server.abort();
}

#[test]
fn session_hover_runs_preserve_their_fields_and_expose_cancellation_proof() {
    use serde_json::{json, Value};

    let sandbox = Sandbox::new();
    assert!(sandbox.run(&["projects", "--json"]).status.success());
    let db = sandbox.db();
    db.execute("INSERT INTO local_projects (id,name,slug,github_owner,github_repo,repo_path,created_at,updated_at) VALUES ('project','P','p','','',?1,1,1)", [sandbox.root.to_str().unwrap()]).unwrap();
    db.execute("INSERT INTO chat_sessions (id,project_id,harness,created_at,updated_at) VALUES ('chat','project','codex',1,1)", []).unwrap();
    for (id, status, contact, unconfirmed) in [
        ("pending", "running", None, false),
        ("verified", "cancelled", None, false),
        (
            "survivors",
            "cancelled",
            Some(json!({"type":"stopping","since":1,"count":2})),
            false,
        ),
        ("unconfirmed", "cancelled", None, true),
    ] {
        db.execute("INSERT INTO local_experiments (id,project_id,slug,branch_name,run_command,agent_status,created_at,updated_at,chat_session_id) VALUES (?1,'project',?1,'main','sleep 60','idle',1,1,'chat')", [id]).unwrap();
        db.execute("INSERT INTO runs (id,experiment_id,project_id,status,backend_json,command,created_at,updated_at,cancel_requested,contact,cancel_unconfirmed) VALUES (?1,?1,'project',?2,'{\"kind\":\"engine\"}','sleep 60',100,100,1,?3,?4)", rusqlite::params![id,status,contact.map(|value|value.to_string()),unconfirmed.then_some("[]")]).unwrap();
    }
    let started = sandbox.run(&["runs", "project"]);
    assert!(started.status.success(), "{}", diagnostics(&started));
    let port = sandbox.descriptor().unwrap()["port"].as_u64().unwrap();
    let response = Command::new("curl")
        .args([
            "-sf",
            "--noproxy",
            "*",
            "--max-time",
            "10",
            &format!("http://127.0.0.1:{port}/api/chat/sessions/chat/experiments"),
        ])
        .output()
        .unwrap();
    assert!(response.status.success(), "{}", diagnostics(&response));
    let body: Value = serde_json::from_slice(&response.stdout).unwrap();
    let rows = body["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    for row in rows {
        let id = row["experiment"]["id"].as_str().unwrap();
        let run = &row["currentRun"];
        assert_eq!(
            run["status"],
            if id == "pending" {
                "running"
            } else {
                "cancelled"
            }
        );
        assert_eq!(run["cancelRequested"], true);
        assert_eq!(run["createdAt"], 100);
        assert_eq!(run.get("endedAt"), Some(&Value::Null));
        assert_eq!(run["cancelling"], id == "pending");
        assert_eq!(run["cancelUnconfirmed"].is_array(), id == "unconfirmed");
        assert!(run.get("host").is_some());
        if id == "survivors" {
            assert_eq!(run["contact"]["type"], "stopping");
            assert_eq!(run["contact"]["count"], 2);
        } else if id != "pending" {
            assert!(run["contact"].is_null());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abandon_warns_that_actual_compute_must_be_shut_down() {
    use axum::{routing::get, Json, Router};
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    let sandbox = Sandbox::new();
    let contact = Arc::new(Mutex::new("unreachable"));
    let calls = Arc::new(AtomicUsize::new(0));
    let (read_contact, abandoned) = (contact.clone(), calls.clone());
    let app = Router::new()
        .route(
            "/api/v2/runs/run",
            get(move || {
                let contact = *read_contact.lock().unwrap();
                async move { Json(json!({"id":"run", "contact":{"type":contact}})) }
            }),
        )
        .route(
            "/api/v2/runs/run/abandon",
            axum::routing::post(move || {
                abandoned.fetch_add(1, Ordering::SeqCst);
                async { Json(json!({"ok":true})) }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for (index, kind) in ["unreachable", "needs_auth", "unreconciled"]
        .into_iter()
        .enumerate()
    {
        *contact.lock().unwrap() = kind;
        let output = sandbox
            .command()
            .env("ORX_UP_PORT", port.to_string())
            .env("ORX_UP_AUTH_TOKEN", "test-only")
            .args(["exp", "abandon", "run"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", diagnostics(&output));
        let warning = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            warning.contains("The machine is unreachable."),
            kind == "unreachable"
        );
        assert!(warning.contains("OpenResearch will stop tracking this run. Double-check that the actual compute is shut down."), "{warning}");
        assert!(stdout(&output).contains("cancel stays pending"));
        assert_eq!(calls.load(Ordering::SeqCst), index + 1);
    }
    server.abort();
}

#[test]
fn an_engine_run_finishing_while_up_is_down_keeps_the_receipt_finish_time() {
    let sandbox = Sandbox::new();
    let release = sandbox.root.join("release");
    sandbox.project(&format!(
        "echo started\nwhile [ ! -f '{}' ]; do sleep 0.05; done\necho finished\n",
        release.display(),
    ));
    let launched = sandbox.run(&["exp", "run", "exp", "--force", "--backend", "local"]);
    assert!(launched.status.success(), "{}", diagnostics(&launched));
    let run = stdout(&launched)
        .split_whitespace()
        .find(|word| word.len() == 36 && word.matches('-').count() == 4)
        .unwrap()
        .to_owned();
    assert!(wait_for(Duration::from_secs(30), || sandbox.status(&run)
        == "running"));
    let backend: String = sandbox
        .db()
        .query_row(
            "SELECT backend_json FROM runs WHERE id = ?1",
            [&run],
            |row| row.get(0),
        )
        .unwrap();
    let backend: serde_json::Value = serde_json::from_str(&backend).unwrap();
    let dir = PathBuf::from(backend["handle"]["dir"].as_str().unwrap());
    orx_test_support::stop_server(&sandbox.data());
    std::fs::write(release, "").unwrap();
    assert!(wait_for(Duration::from_secs(15), || dir
        .join("exit.json")
        .exists()));
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("exit.json")).unwrap()).unwrap();
    let finished_at = receipt["finished_at"].as_i64().unwrap();
    assert_eq!(sandbox.status(&run), "running");
    std::thread::sleep(Duration::from_millis(150));
    let resumed = sandbox.run(&["runs", "project"]);
    assert!(resumed.status.success(), "{}", diagnostics(&resumed));
    assert!(wait_for(Duration::from_secs(30), || sandbox.status(&run) == "done"));
    let (started_at, ended_at): (i64, i64) = sandbox
        .db()
        .query_row(
            "SELECT started_at, ended_at FROM runs WHERE id = ?1",
            [&run],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(ended_at, finished_at);
    assert!(started_at <= ended_at);
}

/// An agent whose config dir is not orx up's, as in a sandbox with its own home, launches
/// unless that dir sets compute settings orx up would not use.
#[test]
fn a_caller_with_its_own_config_dir_launches_only_with_the_settings_orx_up_uses() {
    let sandbox = Sandbox::new();
    sandbox.project("echo done\n");
    let started = sandbox.run(&["exp", "run", "exp", "--force", "--backend", "local"]);
    assert!(started.status.success(), "{}", diagnostics(&started));
    let agent_config = sandbox.root.join("agent-config");
    let slurm = |partition: &str| format!(r#"{{"partition":"{partition}"}}"#);
    let launch_with = |flags: &[&str]| {
        let output = sandbox
            .command()
            .env("XDG_CONFIG_HOME", &agent_config)
            .args(["exp", "run", "exp", "--force", "--backend", "slurm"])
            .args(["--host", sandbox.server.alias()])
            .args(flags)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stderr).into_owned()
    };
    let launch = || launch_with(&[]);
    let refused = |stderr: &str| stderr.contains(agent_config.to_str().unwrap());

    let empty = launch();
    assert!(!refused(&empty), "{empty}");

    std::fs::create_dir_all(agent_config.join("openresearch")).unwrap();
    std::fs::write(agent_config.join("openresearch/slurm.json"), slurm("gpu")).unwrap();
    let different = launch();
    assert!(refused(&different), "{different}");
    assert!(different.contains("partition"), "{different}");
    assert!(
        different.contains(sandbox.root.join("config").to_str().unwrap()),
        "{different}"
    );

    std::fs::create_dir_all(sandbox.root.join("config/openresearch")).unwrap();
    std::fs::write(
        sandbox.root.join("config/openresearch/slurm.json"),
        slurm("gpu"),
    )
    .unwrap();
    let equal = launch();
    assert!(!refused(&equal), "{equal}");

    let limits = |mem: &str| format!(r#"{{"partition":"gpu","mem":"{mem}"}}"#);
    std::fs::write(agent_config.join("openresearch/slurm.json"), limits("32G")).unwrap();
    std::fs::write(
        sandbox.root.join("config/openresearch/slurm.json"),
        limits("64G"),
    )
    .unwrap();
    let saved_limit = launch();
    assert!(refused(&saved_limit), "{saved_limit}");
    assert!(saved_limit.contains("mem"), "{saved_limit}");
    let flagged = launch_with(&["--mem", "16G"]);
    assert!(!refused(&flagged), "{flagged}");

    let save = |caller: &str, own: &str| {
        let saved = |field: &str| format!(r#"{{"partition":"gpu",{field}}}"#);
        std::fs::write(agent_config.join("openresearch/slurm.json"), saved(caller)).unwrap();
        std::fs::write(
            sandbox.root.join("config/openresearch/slurm.json"),
            saved(own),
        )
        .unwrap();
    };
    for (name, caller, own) in [
        ("qos", r#""qos":"acc_debug""#, r#""qos":"acc_ehpc""#),
        (
            "sbatchArgs",
            r#""sbatchArgs":["--rsc g=1"]"#,
            r#""sbatchArgs":[]"#,
        ),
        (
            "requestOneTask",
            r#""requestOneTask":false"#,
            r#""requestOneTask":true"#,
        ),
    ] {
        save(caller, own);
        let stderr = launch();
        assert!(
            refused(&stderr) && stderr.contains(name),
            "{name}: {stderr}"
        );
    }
    save(r#""qos":"acc_debug""#, r#""qos":"acc_ehpc""#);
    let qos_flag = launch_with(&["--qos", "acc_debug"]);
    assert!(!refused(&qos_flag), "{qos_flag}");

    // Without the setting, orx writes --ntasks=1 just as `true` asks.
    std::fs::write(
        agent_config.join("openresearch/slurm.json"),
        r#"{"partition":"gpu","requestOneTask":true}"#,
    )
    .unwrap();
    std::fs::write(
        sandbox.root.join("config/openresearch/slurm.json"),
        r#"{"partition":"gpu"}"#,
    )
    .unwrap();
    let one_task = launch();
    assert!(!refused(&one_task), "{one_task}");
}
