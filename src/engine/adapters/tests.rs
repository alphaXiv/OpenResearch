use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orx_host_proto::{ExitReason, LogPage};
use orx_test_support::{Faults, Sshd};
use serde_json::json;

use super::{local::Local, slurm::Slurm, ssh::Ssh};
use crate::engine::clock::{Clock, TestClock};
use crate::engine::transport::{Config, HostConn, Status};
use crate::engine::*;
use crate::local::model::LocalExperiment;
use crate::store::{LaunchSpec, Store, StoredRun, SubmittingRun};
use orx_test_support::host_binaries as binaries;

struct Fixture {
    server: Sshd,
    clock: Arc<TestClock>,
    transport_clock: Arc<TestClock>,
    engine: Engine,
    ssh: Arc<Ssh>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_host_env("").await
    }

    /// `exports` are shell lines run before each `orx-host` the fixture starts.
    async fn with_host_env(exports: &str) -> Self {
        let server = Sshd::start().unwrap();
        let command = server.directory().join("command");
        let text = fs::read_to_string(&command).unwrap();
        let bin = server.directory().join("shims");
        fs::create_dir(&bin).unwrap();
        fs::write(
            command,
            text.replace(
                "export HOME",
                &format!(
                    "export HOME\nexport PATH='{}':\"$PATH\"\nexport ORX_TEST_FAST_STOPS=1\n{exports}",
                    bin.display()
                ),
            ),
        )
        .unwrap();
        let store = Store::open_at(server.directory().join("store")).unwrap();
        store
            .create_local_experiment(&LocalExperiment {
                id: "exp".into(),
                project_id: "proj".into(),
                parent_experiment_id: None,
                slug: "exp".into(),
                branch_name: "orx/exp".into(),
                title: None,
                description: None,
                run_command: "true".into(),
                agent_status: "idle".into(),
                created_at: 1,
                updated_at: 1,
                chat_session_id: None,
                archived: false,
            })
            .unwrap();
        let clock = Arc::new(TestClock::new());
        let transport_clock = Arc::new(TestClock::new());
        let ssh = connect(&server, transport_clock.clone()).await;
        let engine = Engine::new(store, clock.clone());
        Self {
            server,
            clock,
            transport_clock,
            engine,
            ssh,
        }
    }

    fn store(&self) -> Store {
        Store::open_at(self.server.directory().join("store")).unwrap()
    }

    fn dir(&self, id: &str) -> PathBuf {
        self.server.home().join("engine-root/runs").join(id)
    }

    fn intent(&self, id: &str, script: &str, kind: &str) -> RunIntent {
        RunIntent {
            run: StoredRun {
                id: id.into(),
                experiment_id: "exp".into(),
                project_id: "proj".into(),
                status: "submitting".into(),
                backend_json: "{\"kind\":\"engine\"}".into(),
                command: script.into(),
                created_at: 1,
                updated_at: 1,
                ended_at: None,
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: Some("chat".into()),
            },
            launch: LaunchSpec {
                kind: kind.into(),
                target: Some(self.server.alias().into()),
                options: json!({}),
            },
            spec_digest: orx_host::sha(script.as_bytes()),
            attempt: 1,
        }
    }

    async fn submit(&self, backend: &dyn Backend, intent: &RunIntent) {
        self.engine
            .submit(backend, submitting(intent))
            .await
            .unwrap();
    }

    async fn poll(&self, backend: &dyn Backend, ids: &[&str], budget: Duration) {
        let runs: Vec<_> = ids
            .iter()
            .map(|id| {
                let run = self.store().get_run(id).unwrap().unwrap();
                WatchedRun {
                    id: id.to_string(),
                    handle: handle_of(&run).unwrap(),
                }
            })
            .collect();
        self.engine.observe_runs(backend, &runs, budget).await;
    }

    async fn terminal(&self, backend: &dyn Backend, id: &str) -> StoredRun {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                self.poll(backend, &[id], Duration::from_millis(50)).await;
                let run = self.store().get_run(id).unwrap().unwrap();
                if Phase::parse(&run.status).unwrap().is_terminal() {
                    return run;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{id} never became terminal: {:?}",
                self.store().engine_run_state(id)
            )
        })
    }

    fn shim(&self, name: &str, script: &str) {
        let path = self.server.directory().join("shims").join(name);
        orx_test_support::write_executable(path, format!("#!/bin/sh\n{script}\n"), 0o700).unwrap();
    }
}

fn submitting(intent: &RunIntent) -> SubmittingRun<'_> {
    static METADATA: std::sync::OnceLock<crate::jobs::BackendDescriptor> =
        std::sync::OnceLock::new();
    SubmittingRun {
        id: &intent.run.id,
        experiment_id: "exp",
        project_id: "proj",
        metadata: METADATA.get_or_init(|| {
            crate::jobs::BackendDescriptor::parse("{\"kind\":\"ssh_job\"}").unwrap()
        }),
        command: &intent.run.command,
        created_at: 1,
        commit_sha: None,
        chat_session_id: Some("chat"),
        spec_digest: &intent.spec_digest,
        launch: &intent.launch,
    }
}

fn config(directory: &Path, alias: &str) -> Config {
    Config {
        alias: alias.into(),
        root: "~/engine-root".into(),
        ssh_config: Some(directory.join("ssh_config")),
        control_dir: directory.join("cm"),
        binary_dir: Some(binaries()),
        #[cfg(unix)]
        sharing_home: None,
        askpass_executable: std::env::current_exe().unwrap(),
    }
}

async fn live(conn: &HostConn) {
    let mut state = conn.state();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let current = state.borrow_and_update().clone();
            if current.status == Status::Live {
                break;
            }
            assert_ne!(current.status, Status::NeedsAuth, "{current:?}");
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Host did not connect: {:?}", *conn.state().borrow()));
}

async fn connect(server: &Sshd, clock: Arc<dyn Clock>) -> Arc<Ssh> {
    let mut config = config(server.directory(), server.alias());
    config.ssh_config = Some(server.config_path());
    let conn = {
        let store = Store::open_at(server.directory().join("store")).unwrap();
        let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, clock, auth)
    }
    .unwrap();
    live(&conn).await;
    Arc::new(Ssh::new(
        server.alias().into(),
        "~/engine-root".into(),
        conn,
    ))
}

async fn exists(path: &Path) {
    until(|| path.exists()).await;
}

async fn until(condition: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(60), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn watched(f: &Fixture, id: &str) -> WatchedRun {
    let run = f.store().get_run(id).unwrap().unwrap();
    WatchedRun {
        id: id.into(),
        handle: handle_of(&run).unwrap(),
    }
}

fn data(page: LogPage) -> (FileIdent, Vec<u8>, bool) {
    match page {
        LogPage::Data {
            ident, bytes, eof, ..
        } => (ident, bytes, eof),
        page => panic!("Expected data: {page:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_idempotent_launch_staging_notify_and_cleanup() {
    let f = Fixture::new().await;
    let snapshot = f.server.directory().join("source");
    fs::create_dir(&snapshot).unwrap();
    fs::write(snapshot.join("value"), "staged").unwrap();
    let archive = f.server.directory().join("source.tar");
    assert!(crate::process::command("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&snapshot)
        .arg(".")
        .status()
        .unwrap()
        .success());
    let bytes = fs::read(&archive).unwrap();
    let digest = orx_host::sha(&bytes);
    let mut intent = f.intent(
        "staged",
        r#"cat repo/value; printf notice > "$ORX_NOTIFY_FILE"; python3 -c 'import os,time; open("daemon.pid","w").write(str(os.getpid())); time.sleep(60)' >/dev/null 2>&1 & while [ ! -f daemon.pid ]; do sleep .01; done"#,
        "ssh_job",
    );
    intent.run.backend_json = json!({"kind":"ssh_job", "sourceDigest":digest,"sourcePath":archive,"sourceSize":bytes.len()}).to_string();
    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
    let mut row = submitting(&intent);
    row.metadata = &metadata;
    f.engine.submit(f.ssh.as_ref(), row).await.unwrap();
    let subscription = f.ssh.follow_logs("staged");
    let second_subscription = f.ssh.follow_logs("staged");
    drop(subscription);
    assert!(f
        .ssh
        .changes(&["staged".into()], Duration::ZERO)
        .await
        .unwrap()[0]
        .1
        .log_len
        .is_some());
    assert_eq!(f.terminal(f.ssh.as_ref(), "staged").await.status, "done");
    let run = watched(&f, "staged");
    assert_eq!(
        data(
            f.ssh
                .read_log(
                    &run,
                    LogFile::Log,
                    ByteRange {
                        start: 0,
                        end: 1024
                    },
                    None
                )
                .await
                .unwrap()
        )
        .1,
        b"staged"
    );
    assert_eq!(
        data(
            f.ssh
                .read_log(
                    &run,
                    LogFile::Notify,
                    ByteRange {
                        start: 0,
                        end: 1024
                    },
                    None
                )
                .await
                .unwrap()
        )
        .1,
        b"notice"
    );
    assert!(f.ssh.summary("staged").await.is_some());
    let receipt: orx_host_proto::ExitReceipt =
        serde_json::from_slice(&fs::read(f.dir("staged").join("exit.json")).unwrap()).unwrap();
    assert_eq!(receipt.reason, ExitReason::Exited);
    assert!(!receipt.processes.is_empty());
    let started = fs::read(f.dir("staged").join("started")).unwrap();
    assert!(matches!(f.ssh.recover(&intent).await, Recovery::Found(_)));
    assert_eq!(fs::read(f.dir("staged").join("started")).unwrap(), started);
    assert_eq!(
        f.ssh.cleanup(&run).await.unwrap(),
        orx_host_proto::CleanupAck::Accepted
    );
    exists(&f.dir("staged").join("cleanup.json")).await;
    let cleanup: orx_host_proto::ProcessOutcome =
        serde_json::from_slice(&fs::read(f.dir("staged").join("cleanup.json")).unwrap()).unwrap();
    assert!(cleanup.processes.is_empty());
    #[cfg(target_os = "linux")]
    assert_eq!(cleanup.outcome, orx_host_proto::CancelOutcome::Verified);
    drop(second_subscription);
    let notices = f.store().run_notice_batch("chat").unwrap();
    assert_eq!(notices[0].dedupe, "lingering");
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.dedupe == "lingering")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn black_hole_preserves_phase_and_reconnects_after_one_backoff_step() {
    let f = Fixture::new().await;
    let intent = f.intent("outage", "sleep 60", "ssh_job");
    f.submit(f.ssh.as_ref(), &intent).await;
    exists(&f.dir("outage").join("started")).await;
    f.poll(f.ssh.as_ref(), &["outage"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("outage").unwrap().unwrap().status,
        "running"
    );
    f.server
        .proxy()
        .set(Faults {
            black_hole: true,
            ..Faults::default()
        })
        .unwrap();
    f.transport_clock.advance(Duration::from_secs(36));
    tokio::time::sleep(Duration::from_millis(150)).await;
    f.poll(f.ssh.as_ref(), &["outage"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("outage").unwrap().unwrap().status,
        "running"
    );
    assert!(matches!(
        f.store()
            .engine_run_state("outage")
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Unreachable { .. }
    ));
    f.engine
        .cancel(f.ssh.as_ref(), "outage", "test")
        .await
        .unwrap();
    assert!(!f.dir("outage").join("cancel.req").exists());
    f.server.proxy().clear().unwrap();
    f.transport_clock.advance(Duration::from_secs(7));
    live(&f.ssh.conn).await;
    f.poll(f.ssh.as_ref(), &["outage"], Duration::ZERO).await;
    assert_eq!(
        f.store()
            .engine_run_state("outage")
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Live
    );

    exists(&f.dir("outage").join("exit.json")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_during_backoff_reconciles_a_finished_run_without_waiting_for_retry() {
    let f = Fixture::new().await;
    let id = "backoff-restart";
    let intent = f.intent(
        id,
        "touch ready; while [ ! -f release ]; do sleep .01; done",
        "ssh_job",
    );
    f.submit(f.ssh.as_ref(), &intent).await;
    exists(&f.dir(id).join("ready")).await;
    f.poll(f.ssh.as_ref(), &[id], Duration::ZERO).await;
    assert_eq!(f.store().get_run(id).unwrap().unwrap().status, "running");

    f.server
        .proxy()
        .set(Faults {
            refuse_new: true,
            ..Faults::default()
        })
        .unwrap();
    f.server.proxy().reset().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.ssh.conn.state().borrow().status == Status::Live {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(f.ssh.conn.retry().await.is_err());
    assert_eq!(f.ssh.conn.state().borrow().status, Status::Backoff);
    fs::write(f.dir(id).join("release"), "").unwrap();
    exists(&f.dir(id).join("exit.json")).await;
    f.server.proxy().clear().unwrap();

    let restarted = connect(&f.server, f.transport_clock.clone()).await;
    f.poll(restarted.as_ref(), &[id], Duration::ZERO).await;
    assert_eq!(f.store().get_run(id).unwrap().unwrap().status, "done");
    let terminals = f
        .store()
        .run_notice_batch("chat")
        .unwrap()
        .iter()
        .filter(|notice| notice.kind == crate::store::TERMINAL)
        .count();
    assert_eq!(terminals, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_while_paused_arrives_in_one_feed_reply() {
    let f = Fixture::new().await;
    f.submit(
        f.ssh.as_ref(),
        &f.intent(
            "paused",
            "while [ ! -f finish ]; do sleep 0.05; done",
            "ssh_job",
        ),
    )
    .await;
    exists(&f.dir("paused").join("started")).await;
    f.poll(f.ssh.as_ref(), &["paused"], Duration::ZERO).await;
    f.server
        .proxy()
        .set(Faults {
            paused: true,
            ..Faults::default()
        })
        .unwrap();
    let run = watched(&f, "paused");
    let runs = [run];
    let reply = f.ssh.observe(&runs, Duration::from_secs(25));
    tokio::pin!(reply);
    assert!(futures::poll!(reply.as_mut()).is_pending());
    fs::write(f.dir("paused").join("finish"), "").unwrap();
    exists(&f.dir("paused").join("exit.json")).await;
    f.server.proxy().clear().unwrap();
    for (id, observation) in tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .unwrap()
    {
        f.engine.observe(&id, observation).unwrap();
    }
    assert_eq!(f.store().get_run("paused").unwrap().unwrap().status, "done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_reaches_new_sessions_and_double_fork_children() {
    let f = Fixture::new().await;
    let script = "python3 -c 'import os,time; p=os.fork(); p and os._exit(0); os.setsid(); p=os.fork(); p and os._exit(0); open(\"daemon\",\"w\").write(str(os.getpid())); time.sleep(60)'\nwhile [ ! -f daemon ]; do sleep .01; done\nsleep 60";
    f.submit(f.ssh.as_ref(), &f.intent("fork", script, "ssh_job"))
        .await;
    exists(&f.dir("fork").join("daemon")).await;
    f.engine
        .cancel(f.ssh.as_ref(), "fork", "test")
        .await
        .unwrap();
    exists(&f.dir("fork").join("exit.json")).await;
    f.poll(f.ssh.as_ref(), &["fork"], Duration::ZERO).await;
    let receipt: orx_host_proto::ExitReceipt =
        serde_json::from_slice(&fs::read(f.dir("fork").join("exit.json")).unwrap()).unwrap();
    assert!(receipt.processes.is_empty());
    #[cfg(target_os = "linux")]
    {
        assert_eq!(receipt.outcome, orx_host_proto::CancelOutcome::Verified);
        assert_eq!(
            f.store().get_run("fork").unwrap().unwrap().status,
            "cancelled"
        );
    }
    #[cfg(target_os = "macos")]
    assert!(
        matches!(receipt.outcome, orx_host_proto::CancelOutcome::Verified)
            || matches!(&receipt.outcome, orx_host_proto::CancelOutcome::Unverifiable { unreadable } if !unreadable.is_empty())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_script_is_staged_before_payload_starts() {
    let f = Fixture::new().await;
    let mut intent = f.intent("container", "sleep 60", "ssh_job");
    intent.launch.options = json!({"cancel_sh":"printf stopped > container-stopped"});
    f.submit(f.ssh.as_ref(), &intent).await;
    exists(&f.dir("container").join("started")).await;
    assert!(f.dir("container").join("cancel.sh").is_file());
    f.engine
        .cancel(f.ssh.as_ref(), "container", "test")
        .await
        .unwrap();
    exists(&f.dir("container").join("exit.json")).await;
    assert_eq!(
        fs::read_to_string(f.dir("container").join("container-stopped")).unwrap(),
        "stopped"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fifty_runs_share_one_ssh_session() {
    let f = Fixture::new().await;
    for index in 0..50 {
        f.submit(
            f.ssh.as_ref(),
            &f.intent(&format!("run-{index}"), "printf x", "ssh_job"),
        )
        .await;
    }
    let ids: Vec<_> = (0..50).map(|index| format!("run-{index}")).collect();
    let refs: Vec<_> = ids.iter().map(String::as_str).collect();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            f.poll(f.ssh.as_ref(), &refs, Duration::from_millis(50))
                .await;
            if ids
                .iter()
                .all(|id| f.store().get_run(id).unwrap().unwrap().status == "done")
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    let output = crate::process::command("ps")
        .args(["-A", "-ww", "-o", "comm=,args="])
        .output()
        .unwrap();
    let sessions = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            let mut fields = line.split_whitespace();
            fields.next().is_some_and(|name| {
                Path::new(name)
                    .file_name()
                    .is_some_and(|name| name == "ssh")
            }) && line.contains(f.server.config_path().to_str().unwrap())
        })
        .count();
    assert!((1..=2).contains(&sessions), "{sessions} ssh processes");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gigabyte_random_pages_survive_one_hundred_reconnects() {
    let f = Fixture::new().await;
    let dir = f.dir("large");
    fs::create_dir_all(&dir).unwrap();
    let mut file = fs::File::create(dir.join("log")).unwrap();
    let len = 1024 * 1024 * 1024 + 17;
    file.set_len(len).unwrap();
    let mut seed = 7u64;
    let mut offsets = Vec::new();
    for index in 0..100 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let offset = seed % (len - 8192);
        offsets.push(offset);
        file.seek(SeekFrom::Start(offset)).unwrap();
        file.write_all(&vec![index as u8; 8192]).unwrap();
    }
    drop(file);
    let run = WatchedRun {
        id: "large".into(),
        handle: f.ssh.handle(),
    };
    let mut local = fs::File::open(dir.join("log")).unwrap();
    let first = data(
        f.ssh
            .read_log(
                &run,
                LogFile::Log,
                ByteRange {
                    start: 0,
                    end: 1024 * 1024,
                },
                None,
            )
            .await
            .unwrap(),
    );
    let identity = first.0;
    for offset in offsets {
        f.server.proxy().reset().unwrap();
        let mut state = f.ssh.conn.state();
        tokio::time::timeout(
            Duration::from_secs(5),
            state.wait_for(|state| state.status != Status::Live),
        )
        .await
        .unwrap()
        .unwrap();
        f.ssh.conn.retry().await.unwrap();
        let mut expected = vec![0; 8192];
        local.seek(SeekFrom::Start(offset)).unwrap();
        local.read_exact(&mut expected).unwrap();
        let page = data(
            f.ssh
                .read_log(
                    &run,
                    LogFile::Log,
                    ByteRange {
                        start: offset,
                        end: offset + 8192,
                    },
                    Some(&identity),
                )
                .await
                .unwrap(),
        );
        assert_eq!(page.1, expected);
    }
    let (_, final_bytes, eof) = data(
        f.ssh
            .read_log(
                &run,
                LogFile::Log,
                ByteRange {
                    start: len - 17,
                    end: len + 4096,
                },
                Some(&identity),
            )
            .await
            .unwrap(),
    );
    assert_eq!(final_bytes, vec![0; 17]);
    assert!(eof);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopted_legacy_run_uses_read_only_inspection_and_group_cancel() {
    use std::os::unix::process::CommandExt;
    let f = Fixture::new().await;
    let dir = f.server.home().join("legacy");
    fs::create_dir(&dir).unwrap();
    let script = "sleep 60";
    fs::write(dir.join("run.sh"), script).unwrap();
    let mut child = crate::process::command("bash")
        .arg(dir.join("run.sh"))
        .process_group(0)
        .spawn()
        .unwrap();
    fs::write(dir.join("pid"), child.id().to_string()).unwrap();
    let descriptor = crate::jobs::BackendDescriptor::parse(
        &json!({"kind":"ssh_job","namespace":f.server.alias(),"jobId":dir}).to_string(),
    )
    .unwrap();
    let run = WatchedRun {
        id: "legacy".into(),
        handle: Handle::Legacy {
            descriptor: Box::new(descriptor),
        },
    };
    let began = Instant::now();
    assert!(matches!(
        f.ssh
            .observe(std::slice::from_ref(&run), Duration::from_millis(100))
            .await[0]
            .1,
        Observation::Running { started_at: None }
    ));
    assert!(began.elapsed() >= Duration::from_millis(100));
    let outcome = f.ssh.cancel(&run).await;
    let _ = child.wait();
    assert!(matches!(outcome, CancelOutcome::Verified), "{outcome:?}");
    assert!(matches!(
        Backend::cleanup(f.ssh.as_ref(), &run).await,
        CancelOutcome::Verified
    ));
    assert_eq!(fs::read_to_string(dir.join("run.sh")).unwrap(), script);
    assert!(!dir.join("launch.json").exists());
    fs::write(dir.join("exit_code"), "0\n").unwrap();
    assert!(f
        .ssh
        .observe(&[run], Duration::ZERO)
        .await
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Exited { code: Some(0), .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_wrapper_receipt_wins_and_missing_accounting_never_terminalizes() {
    let f = Fixture::new().await;
    f.shim("sbatch", r#"cat > "$HOME/batch"
SLURM_JOB_ID=123 python3 -c 'import os,subprocess; subprocess.Popen(["bash",os.environ["HOME"]+"/batch"],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)'
printf '123\n'"#);
    f.shim("squeue", "exit 0");
    f.shim("sacct", "exit 1");
    f.shim("scancel", "printf '%s' \"$*\" > \"$HOME/scancel\"");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("scheduled", "printf compute; printf notice > \"$ORX_NOTIFY_FILE\"; while [ ! -f release ]; do sleep .01; done", "slurm_job");
    f.submit(&slurm, &intent).await;
    exists(&f.dir("scheduled").join("notify")).await;
    let observed = slurm
        .observe(&[watched(&f, "scheduled")], Duration::ZERO)
        .await;
    assert_eq!(
        observed
            .iter()
            .filter(|(_, observation)| matches!(observation, Observation::Notify { .. }))
            .count(),
        1
    );
    fs::write(f.dir("scheduled").join("release"), "").unwrap();
    assert_eq!(f.terminal(&slurm, "scheduled").await.status, "done");
    assert!(
        matches!(slurm.recover(&intent).await, Recovery::Found(Handle::Slurm { job, .. }) if job.as_deref() == Some("123"))
    );
    assert_eq!(
        data(
            slurm
                .read_log(
                    &watched(&f, "scheduled"),
                    LogFile::Log,
                    ByteRange { start: 0, end: 100 },
                    None
                )
                .await
                .unwrap()
        )
        .1,
        b"compute"
    );
    fs::remove_file(f.dir("scheduled").join("slurm.job")).unwrap();
    assert!(matches!(
        slurm.recover(&intent).await,
        Recovery::Found(Handle::Slurm { job: None, .. })
    ));
    f.shim("sbatch", "cat >/dev/null; printf '456\\n'");
    let intent = f.intent("forgotten", "false", "slurm_job");
    f.submit(&slurm, &intent).await;
    f.poll(&slurm, &["forgotten"], Duration::ZERO).await;
    assert!(matches!(
        f.store()
            .engine_run_state("forgotten")
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Unreconciled { .. }
    ));
    f.clock.advance(Duration::from_secs(3600));
    f.poll(&slurm, &["forgotten"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("forgotten").unwrap().unwrap().status,
        "starting"
    );
    f.shim("sacct", "printf '456|forgotten|FAILED|7:0\\n'");
    f.clock.advance(Duration::from_secs(31));
    f.poll(&slurm, &["forgotten"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("forgotten").unwrap().unwrap().status,
        "starting"
    );
    let run = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            f.clock.advance(Duration::from_secs(31));
            f.poll(&slurm, &["forgotten"], Duration::ZERO).await;
            let run = f.store().get_run("forgotten").unwrap().unwrap();
            if Phase::parse(&run.status).unwrap().is_terminal() {
                break run;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(run.status, "failed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_slurm_job_shows_why_it_waits() {
    let f = Fixture::new().await;
    let waiting = |id: &str| {
        f.store()
            .engine_run_state(id)
            .unwrap()
            .unwrap()
            .waiting_reason
    };
    f.shim("sbatch", "cat >/dev/null; printf '456\\n'");
    f.shim("sacct", "exit 1");
    f.shim("squeue", "printf '456|pending|PENDING|Resources\\n'");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("pending", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    f.poll(&slurm, &["pending"], Duration::ZERO).await;
    assert_eq!(waiting("pending").as_deref(), Some("Resources"));

    f.shim("squeue", "printf '456|pending|RUNNING|None\\n'");
    f.clock.advance(Duration::from_secs(31));
    f.poll(&slurm, &["pending"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("pending").unwrap().unwrap().status,
        "running"
    );
    assert_eq!(waiting("pending"), None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_cancel_keeps_started_wrappers_and_scancels_pending_jobs() {
    let f = Fixture::new().await;
    let home = f.server.home();
    let status = |id: &str| f.store().get_run(id).unwrap().unwrap().status;
    f.shim("sbatch", r#"printf '%s' "$*" > "$HOME/sbatch"
cat > "$HOME/batch"
SLURM_JOB_ID=123 python3 -c 'import os,subprocess; subprocess.Popen(["bash",os.environ["HOME"]+"/batch"],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)'
printf '123\n'"#);
    f.shim(
        "squeue",
        "printf '%s' \"$*\" > \"$HOME/squeue\"; printf '123|started|RUNNING\\n'",
    );
    f.shim("sacct", "exit 1");
    f.shim("scancel", "printf '%s' \"$*\" > \"$HOME/scancel\"");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent(
        "started",
        "while [ ! -f release ]; do sleep .01; done",
        "slurm_job",
    );
    f.submit(&slurm, &intent).await;
    exists(&f.dir("started").join("started")).await;
    let sbatch = fs::read_to_string(home.join("sbatch")).unwrap();
    assert!(sbatch.contains(&format!(
        "--output={}",
        f.dir("started").join("slurm.out").display()
    )));
    // A started wrapper owns the cancel: scancel would kill it before its receipt.
    assert!(matches!(
        slurm.cancel(&watched(&f, "started")).await,
        CancelOutcome::Pending
    ));
    exists(&f.dir("started").join("exit.json")).await;
    let receipt: orx_host_proto::ExitReceipt =
        serde_json::from_slice(&fs::read(f.dir("started").join("exit.json")).unwrap()).unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    assert!(!home.join("scancel").exists());
    #[cfg(target_os = "linux")]
    assert_eq!(f.terminal(&slurm, "started").await.status, "cancelled");

    f.shim("sbatch", "cat >/dev/null; printf '456\\n'");
    f.shim(
        "squeue",
        "printf '%s' \"$*\" > \"$HOME/squeue\"; printf '456|pending|PENDING\\n'",
    );
    let intent = f.intent("pending", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    f.poll(&slurm, &["pending"], Duration::ZERO).await;
    assert_eq!(status("pending"), "starting");
    assert!(fs::read_to_string(home.join("squeue"))
        .unwrap()
        .contains("--states=all"));
    // A due queue refresh reports at once instead of long-polling an unchanged feed.
    f.shim("squeue", "printf '456|pending|RUNNING\\n'");
    f.clock.advance(Duration::from_secs(31));
    let began = Instant::now();
    f.poll(&slurm, &["pending"], Duration::from_secs(25)).await;
    assert!(began.elapsed() < Duration::from_secs(5));
    assert_eq!(status("pending"), "running");
    assert!(matches!(
        slurm.cancel(&watched(&f, "pending")).await,
        CancelOutcome::Pending
    ));
    assert_eq!(
        fs::read_to_string(home.join("scancel")).unwrap(),
        "--name=pending"
    );
    assert_eq!(f.terminal(&slurm, "pending").await.status, "cancelled");

    // scancel behind orx's back: squeue keeps the terminal state until MinJobAge, no receipt.
    f.shim("sbatch", "cat >/dev/null; printf '789\\n'");
    f.shim("squeue", "printf '789|killed|CANCELLED\\n'");
    let intent = f.intent("killed", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    f.poll(&slurm, &["killed"], Duration::ZERO).await;
    assert_eq!(status("killed"), "starting");
    tokio::time::sleep(Duration::from_millis(150)).await;
    f.clock.advance(Duration::from_secs(31));
    f.poll(&slurm, &["killed"], Duration::ZERO).await;
    assert_eq!(status("killed"), "cancelled");
}

/// The sleep preventions the wrapper of the run in `dir` holds: IOKit assertion types on macOS,
/// elsewhere the `--what` of each inhibitor the fake `systemd-inhibit` recorded in `inhibitors`.
fn sleep_locks(dir: &Path, inhibitors: &Path) -> Vec<String> {
    let run = dir.file_name().unwrap().to_string_lossy();
    if cfg!(target_os = "macos") {
        let wrapper: orx_host_proto::ProcessIdentity =
            orx_host::read_json(&dir.join("wrapper.json")).unwrap();
        let assertions = crate::process::command("pmset")
            .args(["-g", "assertions"])
            .output()
            .unwrap();
        return String::from_utf8_lossy(&assertions.stdout)
            .lines()
            .filter(|line| {
                line.contains(&format!("pid {}(", wrapper.pid))
                    && line.contains(&format!("OpenResearch experiment run {run}"))
            })
            .filter_map(|line| {
                line.split_whitespace()
                    .find(|word| word.starts_with("Prevent"))
            })
            .map(str::to_owned)
            .collect();
    }
    fs::read_dir(inhibitors)
        .unwrap()
        .filter_map(|entry| fs::read_to_string(entry.unwrap().path()).ok())
        .filter(|args| args.contains(&format!("Experiment run {run}")))
        .filter_map(|args| {
            args.split_whitespace()
                .find_map(|arg| arg.strip_prefix("--what="))
                .map(str::to_owned)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_an_unfinished_local_run_keeps_this_machine_awake() {
    let f = Fixture::new().await;
    let inhibitors = f.server.directory().join("inhibitors");
    fs::create_dir(&inhibitors).unwrap();
    f.shim(
        "systemd-inhibit",
        &format!(
            "for last; do :; done\nprintf '%s\\n' \"$*\" > '{}'/$$\n\"$last\"\nrm '{}'/$$",
            inhibitors.display(),
            inhibitors.display()
        ),
    );
    let script = "touch ready; while [ ! -f release ]; do sleep .01; done";
    let local = Local::new(f.server.directory().join("local-runs"));
    let mut intent = f.intent("nearby", script, "local_job");
    let path = format!(
        "{}:{}",
        f.server.directory().join("shims").display(),
        std::env::var("PATH").unwrap()
    );
    intent.launch.options = json!({"env": {"PATH": path}});
    f.submit(&local, &intent).await;
    f.submit(f.ssh.as_ref(), &f.intent("faraway", script, "ssh_job"))
        .await;
    let nearby = f.server.directory().join("local-runs/nearby");
    let faraway = f.dir("faraway");
    exists(&nearby.join("ready")).await;
    exists(&faraway.join("ready")).await;

    let idle_only = [if cfg!(target_os = "macos") {
        "PreventUserIdleSystemSleep"
    } else {
        "sleep"
    }];
    until(|| sleep_locks(&nearby, &inhibitors) == idle_only).await;
    assert!(sleep_locks(&faraway, &inhibitors).is_empty());

    fs::write(nearby.join("release"), "").unwrap();
    fs::write(faraway.join("release"), "").unwrap();
    assert_eq!(f.terminal(&local, "nearby").await.status, "done");
    assert_eq!(f.terminal(f.ssh.as_ref(), "faraway").await.status, "done");
    until(|| sleep_locks(&nearby, &inhibitors).is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_direct_child_completion_recovery_and_pages() {
    let f = Fixture::new().await;
    let local = Local::new(f.server.directory().join("local-runs"));
    let intent = f.intent(
        "local",
        "printf local; printf notice > \"$ORX_NOTIFY_FILE\"; while [ ! -f release ]; do sleep .01; done; sleep 3 >/dev/null 2>&1 &",
        "local_job",
    );
    f.submit(&local, &intent).await;
    exists(&f.server.directory().join("local-runs/local/notify")).await;
    assert!(matches!(
        local.recover(&intent).await,
        Recovery::Found(Handle::Local { .. })
    ));
    let run = watched(&f, "local");
    let (identity, bytes, eof) = data(
        local
            .read_log(&run, LogFile::Log, ByteRange { start: 0, end: 100 }, None)
            .await
            .unwrap(),
    );
    assert_eq!(bytes, b"local");
    assert!(eof);
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(f.server.directory().join("local-runs/local/log"))
        .unwrap();
    log.write_all(b" append").unwrap();
    assert_eq!(
        data(
            local
                .read_log(
                    &run,
                    LogFile::Log,
                    ByteRange { start: 5, end: 100 },
                    Some(&identity)
                )
                .await
                .unwrap()
        )
        .1,
        b" append"
    );

    fs::write(f.server.directory().join("local-runs/local/release"), "").unwrap();
    let started = Instant::now();
    assert_eq!(f.terminal(&local, "local").await.status, "done");
    let dir = f.server.directory().join("local-runs/local");
    let source_start = orx_host::wrapper::started_at(&dir).unwrap();
    assert_eq!(
        f.store()
            .engine_run_state("local")
            .unwrap()
            .unwrap()
            .started_at,
        Some(source_start)
    );
    assert_eq!(
        f.store().get_run("local").unwrap().unwrap().ended_at,
        orx_host::wrapper::receipt(&dir)
            .unwrap()
            .unwrap()
            .finished_at
            .map(|finished_at| finished_at.min(f.clock.wall_ms()))
    );
    assert!(started.elapsed() < Duration::from_secs(2));

    assert!(matches!(
        local
            .read_log(
                &run,
                LogFile::Log,
                ByteRange {
                    start: 50,
                    end: 100
                },
                Some(&identity)
            )
            .await
            .unwrap(),
        LogPage::Reset { .. }
    ));
    assert_eq!(
        data(
            local
                .read_log(
                    &run,
                    LogFile::Notify,
                    ByteRange { start: 0, end: 100 },
                    None
                )
                .await
                .unwrap()
        )
        .1,
        b"notice"
    );
    assert!(matches!(
        local
            .recover(&f.intent("absent", "true", "tinker_job"))
            .await,
        Recovery::Absent
    ));
}

struct Crash<'a> {
    backend: &'a dyn Backend,
    directory: PathBuf,
    point: String,
}

fn kill_engine() -> ! {
    unsafe {
        libc::kill(std::process::id() as i32, libc::SIGKILL);
    }
    unreachable!()
}

#[async_trait]
impl Backend for Crash<'_> {
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        if self.point == "fence" {
            kill_engine();
        }
        let handle = self.backend.launch(intent, fence).await;
        if self.point == "response" {
            exists(&self.directory.join("executions")).await;
            kill_engine();
        }
        handle
    }
    async fn recover(&self, intent: &RunIntent) -> Recovery {
        self.backend.recover(intent).await
    }
    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)> {
        self.backend.observe(runs, budget).await
    }
    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        self.backend.cancel(run).await
    }
    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> crate::error::Result<LogPage> {
        self.backend.read_log(run, file, range, ident).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submission_child() {
    let Ok(directory) = std::env::var("ORX_ADAPTER_CRASH_DIRECTORY") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let alias = std::env::var("ORX_ADAPTER_CRASH_ALIAS").unwrap();
    let point = std::env::var("ORX_ADAPTER_CRASH_POINT").unwrap();
    let intent: serde_json::Value =
        serde_json::from_slice(&fs::read(directory.join("intent")).unwrap()).unwrap();
    let store = Store::open_at(directory.join("store")).unwrap();
    let clock = Arc::new(TestClock::new());
    let conn = {
        let config = config(&directory, &alias);
        let store = Store::open_at(directory.join("store")).unwrap();
        let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, clock.clone(), auth)
    }
    .unwrap();
    live(&conn).await;
    let ssh = Ssh::new(alias.clone(), "~/engine-root".into(), conn);
    let engine = Engine::new(store, clock);
    let script = intent["script"].as_str().unwrap();
    let metadata = crate::jobs::BackendDescriptor::parse("{\"kind\":\"ssh_job\"}").unwrap();
    let kind = intent["kind"].as_str().unwrap();
    let launch = LaunchSpec {
        kind: kind.into(),
        target: Some(alias),
        options: json!({}),
    };
    let digest = orx_host::sha(script.as_bytes());
    let row = SubmittingRun {
        id: intent["id"].as_str().unwrap(),
        experiment_id: "exp",
        project_id: "proj",
        metadata: &metadata,
        command: script,
        created_at: 1,
        commit_sha: None,
        chat_session_id: Some("chat"),
        spec_digest: &digest,
        launch: &launch,
    };
    if point == "intent" {
        engine.store().insert_submitting_run(row).unwrap();
        kill_engine();
    }
    let local = Local::new(directory.join("local-runs"));
    let backend: &dyn Backend = if kind == "ssh_job" { &ssh } else { &local };
    let run_directory = if kind == "ssh_job" {
        directory.join("home/engine-root/runs").join(row.id)
    } else {
        directory.join("local-runs").join(row.id)
    };
    engine
        .submit(
            &Crash {
                backend,
                point,
                directory: run_directory.clone(),
            },
            row,
        )
        .await
        .unwrap();
    exists(&run_directory.join("executions")).await;
    kill_engine();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_engine_at_each_submission_boundary_then_restart_executes_once() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    std::env::set_var(crate::jobs::tinker::API_KEY_ENV, "test-only-tinker-key");

    let mut f = Fixture::new().await;
    let local = Local::new(f.server.directory().join("local-runs"));
    for kind in ["ssh_job", "local_job", "tinker_job"] {
        for point in ["intent", "fence", "response", "handle"] {
            let id = format!("crash-{kind}-{point}");
            let script = "printf x >> executions; while [ ! -f release ]; do sleep .01; done";
            fs::write(
                f.server.directory().join("intent"),
                json!({"id":id,"script":script,"kind":kind}).to_string(),
            )
            .unwrap();
            let output = crate::process::tokio_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "engine::adapters::tests::submission_child",
                    "--nocapture",
                ])
                .env("ORX_ADAPTER_CRASH_DIRECTORY", f.server.directory())
                .env("ORX_ADAPTER_CRASH_ALIAS", f.server.alias())
                .env("ORX_ADAPTER_CRASH_POINT", point)
                .output()
                .await
                .unwrap();
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(
                output.status.signal(),
                Some(libc::SIGKILL),
                "{kind} {point}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            let run_directory = if kind == "ssh_job" {
                f.dir(&id)
            } else {
                f.server.directory().join("local-runs").join(&id)
            };
            f.ssh = connect(&f.server, f.transport_clock.clone()).await;
            let backend: &dyn Backend = if kind == "ssh_job" {
                f.ssh.as_ref()
            } else {
                &local
            };
            let stored = f.store().engine_run_state(&id).unwrap().unwrap();
            assert!(stored
                .submission_fence
                .as_ref()
                .is_none_or(|fence| fence.submitter.is_none()));
            f.engine.recover(backend, &id).await.unwrap();
            exists(&run_directory.join("executions")).await;
            f.poll(backend, &[&id], Duration::ZERO).await;
            assert_eq!(f.store().get_run(&id).unwrap().unwrap().status, "running");
            fs::write(run_directory.join("release"), "").unwrap();
            assert_eq!(f.terminal(backend, &id).await.status, "done");
            let started = fs::read(run_directory.join("started")).unwrap();
            let intent = f.intent(&id, script, kind);
            for _ in 0..3 {
                assert!(matches!(backend.recover(&intent).await, Recovery::Found(_)));
            }
            assert_eq!(fs::read(run_directory.join("started")).unwrap(), started);
            assert_eq!(fs::read(run_directory.join("executions")).unwrap(), b"x");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn adopted_slurm_uses_recorded_job_id_and_old_receipt_directory() {
    let f = Fixture::new().await;
    f.shim("squeue", "printf '789|orx-legacy-s|RUNNING\n'");
    f.shim("sacct", "exit 1");
    f.shim("scancel", "printf '%s' \"$*\" > \"$HOME/scancel\"");
    let root = f.server.home().join("old-root");
    let dir = root.join("runs/legacy-slurm");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("job.sbatch"), "old batch script").unwrap();
    fs::write(dir.join("log"), "old log").unwrap();
    fs::write(dir.join("notify"), "legacy notice").unwrap();
    let descriptor = crate::jobs::BackendDescriptor::parse(
        &json!({
            "kind":"slurm_job", "namespace":f.server.alias(), "jobId":"789", "remoteRoot":root,
        })
        .to_string(),
    )
    .unwrap();
    let run = WatchedRun {
        id: "legacy-slurm".into(),
        handle: Handle::Legacy {
            descriptor: Box::new(descriptor),
        },
    };
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let observed = slurm
        .observe(std::slice::from_ref(&run), Duration::ZERO)
        .await;
    assert!(matches!(observed[0].1, Observation::Running { .. }));
    assert_eq!(
        observed
            .iter()
            .filter(|(_, observation)| matches!(observation, Observation::Notify { len: 13 }))
            .count(),
        1
    );
    assert!(matches!(slurm.cancel(&run).await, CancelOutcome::Pending));
    assert!(matches!(
        Backend::cleanup(&slurm, &run).await,
        CancelOutcome::Pending
    ));
    assert_eq!(
        fs::read_to_string(f.server.home().join("scancel")).unwrap(),
        "789"
    );
    assert_eq!(
        data(
            slurm
                .read_log(&run, LogFile::Log, ByteRange { start: 0, end: 100 }, None)
                .await
                .unwrap()
        )
        .1,
        b"old log"
    );
    fs::write(dir.join("exit_code"), "").unwrap();
    f.clock.advance(Duration::from_secs(31));
    assert!(matches!(
        slurm
            .observe(std::slice::from_ref(&run), Duration::ZERO)
            .await[0]
            .1,
        Observation::Unreconciled { .. }
    ));
    fs::write(dir.join("exit_code"), "0\n").unwrap();
    f.clock.advance(Duration::from_secs(31));
    assert!(matches!(
        slurm.observe(&[run], Duration::ZERO).await[0].1,
        Observation::Exited { code: Some(0), .. }
    ));
    assert_eq!(
        fs::read_to_string(dir.join("job.sbatch")).unwrap(),
        "old batch script"
    );
    assert!(!dir.join("launch.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_cancel_uses_wrapper_observations_and_tinker_recovers_the_same_layout() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    f.submit(&local, &f.intent("cancel-local", "sleep 60", "local_job"))
        .await;
    exists(&root.join("cancel-local/started")).await;
    assert!(matches!(
        local.cancel(&watched(&f, "cancel-local")).await,
        CancelOutcome::Pending
    ));
    exists(&root.join("cancel-local/exit.json")).await;
    let observed = local
        .observe(&[watched(&f, "cancel-local")], Duration::ZERO)
        .await;
    let receipt = orx_host::wrapper::receipt(&root.join("cancel-local"))
        .unwrap()
        .unwrap();
    let summary = orx_host_proto::Summary {
        started_at: orx_host::wrapper::started_at(&root.join("cancel-local")),
        state: orx_host_proto::RunState::Exited { receipt },
        log_len: Some(0),
        notify_len: 0,
        cleanup: None,
    };
    assert_eq!(
        observed,
        super::ssh::observations("cancel-local".into(), &summary)
    );
    let intent = f.intent("tinker", "true", "tinker_job");
    fs::create_dir(root.join("tinker")).unwrap();
    orx_host::publish(
        &root.join("tinker/launch.json"),
        &orx_host_proto::LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh: "true".into(),
            env: Default::default(),
            source: None,
            cancel_sh: None,
        },
    )
    .unwrap();
    fs::write(root.join("tinker/started"), "").unwrap();
    assert!(matches!(
        local.recover(&intent).await,
        Recovery::Found(Handle::Tinker { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn term_resistant_local_run_stays_running_until_the_wrapper_receipt() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let script = "trap '' TERM; echo first; touch ready; while :; do sleep 1; done";
    f.submit(&local, &f.intent("stubborn", script, "local_job"))
        .await;
    let dir = root.join("stubborn");
    exists(&dir.join("ready")).await;
    f.poll(&local, &["stubborn"], Duration::ZERO).await;
    assert!(f.engine.cancel(&local, "stubborn", "user").await.unwrap());
    assert_eq!(
        f.store().get_run("stubborn").unwrap().unwrap().status,
        "running"
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !dir.join("exit.json").exists() {
        assert!(Instant::now() < deadline);
        f.poll(&local, &["stubborn"], Duration::ZERO).await;
        if !dir.join("exit.json").exists() {
            assert_eq!(
                f.store().get_run("stubborn").unwrap().unwrap().status,
                "running"
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let receipt = orx_host::wrapper::receipt(&dir).unwrap().unwrap();
    assert!(receipt.processes.is_empty());
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    f.poll(&local, &["stubborn"], Duration::ZERO).await;
    let status = f.store().get_run("stubborn").unwrap().unwrap().status;
    assert_eq!(status, "cancelled");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_launch_configuration_is_rejected_without_a_job_or_recovery_loop() {
    let f = Fixture::new().await;
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let local = Local::new(f.server.directory().join("local-runs"));
    let mut ssh_intent = f.intent("invalid-ssh", "true", "ssh_job");
    ssh_intent.launch.options = json!({"env":42});
    let mut slurm_intent = f.intent("invalid-slurm", "true", "slurm_job");
    slurm_intent.launch.options = json!({"directives":{"output":"wrong"}});
    let mut local_intent = f.intent("invalid-local", "true", "local_job");
    local_intent.launch.options = json!({"env":42});
    for (backend, intent) in [
        (f.ssh.as_ref() as &dyn Backend, &ssh_intent),
        (&slurm, &slurm_intent),
        (&local, &local_intent),
    ] {
        let store = std::sync::Mutex::new(f.store());
        let fence = Fence {
            store: &store,
            run_id: &intent.run.id,
            attempt: 1,
            taken_at: 1,
        };
        assert!(matches!(
            backend.launch(intent, &fence).await,
            Err(LaunchError::Rejected { .. })
        ));
        f.submit(backend, intent).await;
        assert_eq!(
            f.store().get_run(&intent.run.id).unwrap().unwrap().status,
            "failed"
        );
        assert!(!f.dir(&intent.run.id).exists());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupted_blob_upload_recovers_without_failing_the_run() {
    let f = Fixture::new().await;
    let source = f.server.directory().join("upload-source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("large"), vec![b'x'; 8 * 1024 * 1024]).unwrap();
    let archive = f.server.directory().join("upload.tar");
    assert!(crate::process::command("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .arg(".")
        .status()
        .unwrap()
        .success());
    let bytes = fs::read(&archive).unwrap();
    let digest = orx_host::sha(&bytes);
    let mut intent = f.intent("upload", "printf x >> executions", "ssh_job");
    intent.run.backend_json = json!({"kind":"ssh_job", "sourceDigest":digest,"sourcePath":archive,"sourceSize":bytes.len()}).to_string();
    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
    let mut row = submitting(&intent);
    row.metadata = &metadata;
    let interrupted = async {
        let part = f
            .server
            .home()
            .join("engine-root/sources")
            .join(format!("{digest}.part"));
        tokio::time::timeout(Duration::from_secs(10), async {
            while !fs::metadata(&part).is_ok_and(|metadata| metadata.len() >= 256 * 1024) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        f.server.proxy().reset().unwrap();
    };
    let (submitted, ()) = tokio::join!(f.engine.submit(f.ssh.as_ref(), row), interrupted);
    submitted.unwrap();
    assert_eq!(
        f.store().get_run("upload").unwrap().unwrap().status,
        "submitting"
    );
    f.transport_clock.advance(Duration::from_secs(7));
    live(&f.ssh.conn).await;
    f.clock.advance(Duration::from_secs(7));
    f.engine.recover(f.ssh.as_ref(), "upload").await.unwrap();
    assert_eq!(f.terminal(f.ssh.as_ref(), "upload").await.status, "done");
    assert_eq!(fs::read(f.dir("upload").join("executions")).unwrap(), b"x");
}

/// The host refusing the source upload fails the run with that cause, its host and its
/// directory, since no launch was sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_refusing_the_source_upload_fails_the_run_naming_host_and_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let archive = f.server.directory().join("refused.tar");
    fs::write(&archive, "source").unwrap();
    let digest = orx_host::sha(b"source");
    let mut intent = f.intent("refused", "true", "ssh_job");
    intent.run.backend_json =
        json!({"kind":"ssh_job", "sourceDigest":digest,"sourcePath":archive,"sourceSize":6})
            .to_string();
    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
    let mut row = submitting(&intent);
    row.metadata = &metadata;
    let sources = f.server.home().join("engine-root/sources");
    fs::set_permissions(&sources, fs::Permissions::from_mode(0o500)).unwrap();
    let submitted = f.engine.submit(f.ssh.as_ref(), row).await;
    fs::set_permissions(&sources, fs::Permissions::from_mode(0o700)).unwrap();
    submitted.unwrap();

    let run = f.store().get_run("refused").unwrap().unwrap();
    let reason = run.result_markdown.unwrap_or_default();
    assert_eq!(run.status, "failed", "{reason}");
    assert!(reason.contains(f.server.alias()), "{reason}");
    assert!(reason.contains("engine-root/sources"), "{reason}");
    assert!(!reason.contains("Interrupted"), "{reason}");
    let state = f.store().engine_run_state("refused").unwrap().unwrap();
    assert_eq!(state.contact, crate::store::Contact::Live);
}

/// A source sweep can remove a blob between `HasBlob` and its pin: the attempt ends unsent
/// rather than leaving the run staging forever, and the next attempt stages the source again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_swept_before_launch_is_restaged_and_runs() {
    let f = Fixture::new().await;
    let source = f.server.directory().join("swept-source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("train"), "x").unwrap();
    let archive = f.server.directory().join("swept.tar");
    assert!(crate::process::command("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&source)
        .arg(".")
        .status()
        .unwrap()
        .success());
    let bytes = fs::read(&archive).unwrap();
    let digest = orx_host::sha(&bytes);
    let mut intent = f.intent("swept", "cat repo/train >> executions", "ssh_job");
    intent.run.backend_json = json!({"kind":"ssh_job", "sourceDigest":digest,"sourcePath":archive,"sourceSize":bytes.len()}).to_string();
    f.ssh
        .stage_blob(&digest, &archive, bytes.len() as u64, &|| false)
        .await
        .unwrap();
    let blob = f.server.home().join("engine-root/sources").join(&digest);
    fs::remove_file(&blob).unwrap();
    assert!(matches!(
        f.ssh.pin_source("swept", &digest, &|| false).await,
        Err(LaunchError::Blocked {
            contact: crate::store::Contact::Live
        })
    ));
    assert!(matches!(f.ssh.recover(&intent).await, Recovery::Absent));
    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
    let mut row = submitting(&intent);
    row.metadata = &metadata;
    f.engine.submit(f.ssh.as_ref(), row).await.unwrap();
    let executions = f.dir("swept").join("executions");
    tokio::time::timeout(Duration::from_secs(10), async {
        while fs::read(&executions).unwrap_or_default() != b"x" {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(blob.exists());
}

/// A cancel accepted while a source is pinned slice by slice stops the pin and is honored by
/// recovery; the payload never launches.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_accepted_while_pinning_stops_before_launch() {
    let f = Fixture::with_host_env("export ORX_TEST_CROSS_DEVICE=1\nexport ORX_TEST_SLOW_RPC=100")
        .await;
    let archive = f.server.directory().join("large.tar");
    fs::write(&archive, vec![b'x'; 8 << 20]).unwrap();
    let bytes = fs::read(&archive).unwrap();
    let digest = orx_host::sha(&bytes);
    let mut intent = f.intent("pinning", "touch ran", "ssh_job");
    intent.run.backend_json = json!({"kind":"ssh_job", "sourceDigest":digest,"sourcePath":archive,"sourceSize":bytes.len()}).to_string();
    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
    let mut row = submitting(&intent);
    row.metadata = &metadata;
    let part = f.dir("pinning").join(format!("source.tar.{digest}.part"));
    let submitted = f.engine.submit(f.ssh.as_ref(), row);
    let cancelled = async {
        // Staging the source over a loaded machine can take a while.
        tokio::time::timeout(Duration::from_secs(60), async {
            while !part.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(f
            .engine
            .cancel(f.ssh.as_ref(), "pinning", "user")
            .await
            .unwrap());
    };
    let (submitted, ()) = tokio::join!(submitted, cancelled);
    submitted.unwrap();
    assert_eq!(
        f.store().get_run("pinning").unwrap().unwrap().status,
        "cancelled"
    );
    assert!(fs::metadata(&part).unwrap().len() < bytes.len() as u64);
    for never in ["source.tar", "launch.json", "started", "ran"] {
        assert!(!f.dir("pinning").join(never).exists(), "{never}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_script_carries_placement_and_the_users_own_arguments() {
    let f = Fixture::new().await;
    f.shim("sbatch", "cat >/dev/null; printf '321\\n'");
    f.shim("squeue", "exit 0");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let mut intent = f.intent("placed", "true", "slurm_job");
    intent.launch.options = json!({
        "directives": {
            "qos": "acc_ehpc",
            "nodelist": "node[01-02]",
            "constraint": "a100&ib",
            "exclude": "node07",
        },
        "sbatch_args": ["--rsc p=1:g=1", "--mail-type=END"],
    });
    f.submit(&slurm, &intent).await;
    let script = fs::read_to_string(f.dir("placed").join("job.sbatch")).unwrap();
    assert!(script.starts_with(
        "#!/bin/bash\n#SBATCH --constraint=a100&ib\n#SBATCH --exclude=node07\n\
         #SBATCH --nodelist=node[01-02]\n#SBATCH --qos=acc_ehpc\n\
         #SBATCH --rsc p=1:g=1\n#SBATCH --mail-type=END\nexec "
    ));

    let mut split = f.intent("split-argument", "true", "slurm_job");
    split.launch.options = json!({"sbatch_args": ["--rsc g=1\n#SBATCH --output=elsewhere"]});
    f.submit(&slurm, &split).await;
    assert_eq!(
        f.store().get_run("split-argument").unwrap().unwrap().status,
        "failed"
    );
    assert!(!f.dir("split-argument").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sbatch_rejection_fails_with_stderr_and_slurm_containers_are_rejected() {
    let f = Fixture::new().await;
    f.shim(
        "sbatch",
        "cat >/dev/null; echo 'Invalid partition name: missing' >&2; exit 2",
    );
    f.shim("squeue", "exit 0");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("rejected-batch", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    let run = f.store().get_run(&intent.run.id).unwrap().unwrap();
    assert_eq!(run.status, "failed");
    assert!(run
        .result_markdown
        .unwrap()
        .contains("Invalid partition name: missing"));
    assert!(!f.dir(&intent.run.id).join("job.sbatch").exists());
    assert!(matches!(slurm.recover(&intent).await, Recovery::Absent));
    let mut container = f.intent("slurm-container", "true", "slurm_job");
    container.launch.options = json!({"cancel_sh":"true"});
    let source = f.server.directory().join("rejected-source");
    let bytes = b"This run must never stage";
    fs::write(&source, bytes).unwrap();
    let sha = orx_host::sha(bytes);
    container.run.backend_json =
        json!({"kind":"slurm_job","sourceDigest":sha,"sourcePath":source,"sourceSize":bytes.len()})
            .to_string();
    let metadata = crate::jobs::BackendDescriptor::parse(&container.run.backend_json).unwrap();
    let mut row = submitting(&container);
    row.metadata = &metadata;
    f.engine.submit(&slurm, row).await.unwrap();
    assert!(!f
        .server
        .home()
        .join("engine-root/sources")
        .join(sha)
        .exists());
    let run = f.store().get_run(&container.run.id).unwrap().unwrap();
    assert_eq!(run.status, "failed");
    assert!(run
        .result_markdown
        .unwrap()
        .contains("Use SSH for container runs"));
    assert!(!f.dir(&container.run.id).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interleaved_recovery_keeps_other_feed_cursors() {
    let f = Fixture::new().await;
    let first = f.intent(
        "cursor-a",
        "touch ready; while [ ! -f release ]; do sleep .01; done",
        "ssh_job",
    );
    let second = f.intent("cursor-b", &first.run.command, "ssh_job");
    f.submit(f.ssh.as_ref(), &first).await;
    f.submit(f.ssh.as_ref(), &second).await;
    exists(&f.dir("cursor-a").join("ready")).await;
    exists(&f.dir("cursor-b").join("ready")).await;
    f.ssh
        .changes(&["cursor-a".into(), "cursor-b".into()], Duration::ZERO)
        .await
        .unwrap();
    assert!(matches!(f.ssh.recover(&first).await, Recovery::Found(_)));
    let began = Instant::now();
    f.ssh
        .changes(&["cursor-b".into()], Duration::from_millis(150))
        .await
        .unwrap();
    assert!(began.elapsed() >= Duration::from_millis(140));
    fs::write(f.dir("cursor-a").join("release"), "").unwrap();
    fs::write(f.dir("cursor-b").join("release"), "").unwrap();
    f.terminal(f.ssh.as_ref(), "cursor-a").await;
    f.terminal(f.ssh.as_ref(), "cursor-b").await;
    assert!(f.ssh.summary("cursor-a").await.is_some());
    assert!(f.ssh.summary("cursor-b").await.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn killed_local_payload_has_a_receipt_and_observation_honors_its_budget() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let intent = f.intent(
        "killed-local",
        "echo $$ > payload-pid; touch ready; exec sleep 60",
        "local_job",
    );
    f.submit(&local, &intent).await;
    let dir = root.join("killed-local");
    exists(&dir.join("ready")).await;
    let pid: i32 = fs::read_to_string(dir.join("payload-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    exists(&dir.join("exit.json")).await;
    let began = Instant::now();
    f.poll(&local, &["killed-local"], Duration::from_millis(1100))
        .await;
    assert!(began.elapsed() >= Duration::from_millis(1100));
    assert_eq!(
        f.store().get_run("killed-local").unwrap().unwrap().status,
        "failed"
    );
    assert_eq!(
        f.store()
            .engine_run_state("killed-local")
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Live
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_notify_notices_are_not_repeated_after_exit() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let intent = f.intent("notify-exit", "printf one > \"$ORX_NOTIFY_FILE\"; touch ready; while [ ! -f grow ]; do sleep .01; done; printf two >> \"$ORX_NOTIFY_FILE\"; touch grown; while [ ! -f release ]; do sleep .01; done", "local_job");
    f.submit(&local, &intent).await;
    let dir = root.join("notify-exit");
    exists(&dir.join("ready")).await;
    for text in ["one", "two"] {
        if text == "two" {
            f.clock.advance(Duration::from_secs(10 * 60));
            fs::write(dir.join("grow"), "").unwrap();
            exists(&dir.join("grown")).await;
        }
        f.poll(&local, &["notify-exit"], Duration::ZERO).await;
        f.engine
            .deliver_notify(&local, "notify-exit")
            .await
            .unwrap();
        let notices = f.store().run_notice_batch("chat").unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, crate::store::NOTIFY);
        assert!(notices[0].body.ends_with(text));
        f.store()
            .mark_run_notice_batch_delivered("chat", &notices[0].client_turn_id, f.clock.wall_ms())
            .unwrap();
    }
    fs::write(dir.join("release"), "").unwrap();
    exists(&dir.join("exit.json")).await;
    f.poll(&local, &["notify-exit"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("notify-exit").unwrap().unwrap().status,
        "done"
    );
    f.clock.advance(Duration::from_secs(10 * 60));
    f.engine
        .deliver_notify(&local, "notify-exit")
        .await
        .unwrap();
    let notices = f.store().run_notice_batch("chat").unwrap();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.kind == crate::store::NOTIFY)
            .count(),
        0
    );
    let cursor = f
        .store()
        .engine_run_state("notify-exit")
        .unwrap()
        .unwrap()
        .notify_cursor
        .unwrap();
    assert_eq!(cursor.generation, 0);
    assert_eq!(cursor.offset, 6);
    assert!(f.engine.notify_due().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sbatch_error_adopts_the_job_already_in_the_queue() {
    let f = Fixture::new().await;
    f.shim(
        "sbatch",
        "cat >/dev/null; echo 'Socket timed out' >&2; exit 1",
    );
    f.shim("squeue", "printf '987|queued-on-error|PENDING\\n'");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("queued-on-error", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    let run = f.store().get_run(&intent.run.id).unwrap().unwrap();
    assert_eq!(run.status, "starting");
    assert!(matches!(handle_of(&run), Some(Handle::Slurm { job: Some(job), .. }) if job == "987"));
    assert_eq!(
        fs::read_to_string(f.dir(&intent.run.id).join("slurm.job")).unwrap(),
        "987"
    );
    assert!(f.dir(&intent.run.id).join("job.sbatch").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn controller_errors_keep_the_submit_marker_and_recover_without_resubmitting() {
    let f = Fixture::new().await;
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    f.shim("sacct", "exit 1");
    for (index, error) in [
        "Socket timed out",
        "Unable to contact slurm controller",
        "Connection refused",
        "Connection reset by peer",
    ]
    .iter()
    .enumerate()
    {
        f.shim("squeue", "exit 0");
        f.shim(
            "sbatch",
            &format!(
                "cat >/dev/null; printf x >> \"$HOME/sbatch-calls\"; printf '%s\\n' {} >&2; exit 1",
                crate::jobs::ssh::sh_quote(error)
            ),
        );
        let id = format!("controller-{index}");
        let intent = f.intent(&id, "true", "slurm_job");
        f.submit(&slurm, &intent).await;
        assert_eq!(
            f.store().get_run(&id).unwrap().unwrap().status,
            "submitting"
        );
        assert!(f.dir(&id).join("job.sbatch").exists());
        f.shim("squeue", &format!("printf '987|{id}|RUNNING\\n'"));
        f.clock.advance(Duration::from_secs(7));
        f.engine.recover(&slurm, &id).await.unwrap();
        let run = f.store().get_run(&id).unwrap().unwrap();
        assert_eq!(run.status, "starting");
        assert!(
            matches!(handle_of(&run), Some(Handle::Slurm { job: Some(job), .. }) if job == "987")
        );
        assert_eq!(
            fs::read(f.server.home().join("sbatch-calls"))
                .unwrap()
                .len(),
            index + 1
        );
    }
}

#[tokio::test]
async fn escaped_launch_frames_are_rejected_before_transmission() {
    use orx_host_proto::{LaunchIntent, Request};
    let f = Fixture::new().await;
    let script = "\u{1}".repeat(1024 * 1024);
    let result = f
        .ssh
        .launch_request(Request::Launch(LaunchIntent {
            run: "too-large".into(),
            digest: orx_host::sha(b"too-large"),
            run_sh: script.clone(),
            env: Default::default(),
            source: None,
            cancel_sh: Some(script.clone()),
        }))
        .await;
    assert!(
        matches!(result, Err(LaunchError::Rejected { reason }) if reason.contains("8 MiB frame limit"))
    );
    assert!(!f.dir("too-large").exists());
    let result = f
        .ssh
        .launch_request(Request::SlurmSubmit {
            intent: LaunchIntent {
                run: "too-large".into(),
                digest: orx_host::sha(b"too-large"),
                run_sh: script.clone(),
                env: Default::default(),
                source: None,
                cancel_sh: None,
            },
            directives: script.repeat(2),
        })
        .await;
    assert!(
        matches!(result, Err(LaunchError::Rejected { reason }) if reason.contains("8 MiB frame limit"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_launch_auth_failure_before_send_keeps_a_retryable_submission() {
    let f = Fixture::new().await;
    fs::write(f.server.directory().join("authorized_keys"), "").unwrap();
    let mut cfg = config(f.server.directory(), f.server.alias());
    cfg.root = "~/never-launched".into();
    let conn = {
        let config = cfg;
        let store = f.store();
        let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, f.transport_clock.clone(), auth)
    }
    .unwrap();
    let ssh = Ssh::new(f.server.alias().into(), "~/never-launched".into(), conn);
    let intent = f.intent("blocked-auth", "echo should-not-start", "ssh_job");
    f.submit(&ssh, &intent).await;
    let state = f.store().engine_run_state(&intent.run.id).unwrap().unwrap();
    assert_eq!(
        state.contact,
        crate::store::Contact::NeedsAuth {
            since: ssh.conn.state().borrow().since_ms,
            cause: "Permission denied (publickey)".into()
        }
    );
    assert!(state.submission_fence.is_none());
    assert_eq!(
        f.store().get_run(&intent.run.id).unwrap().unwrap().status,
        "submitting"
    );
    assert!(!f
        .server
        .home()
        .join("never-launched/runs/blocked-auth")
        .exists());
    // Recovery can't ask the host either; it waits for Connect without counting a failure.
    assert!(matches!(
        ssh.recover(&intent).await,
        Recovery::Blocked {
            contact: crate::store::Contact::NeedsAuth { .. }
        }
    ));
    fs::write(
        f.server.directory().join("authorized_keys"),
        fs::read(f.server.directory().join("client_key.pub")).unwrap(),
    )
    .unwrap();
    ssh.conn.retry().await.unwrap();
    f.engine.recover(&ssh, &intent.run.id).await.unwrap();
    assert_eq!(f.terminal(&ssh, &intent.run.id).await.status, "done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_launch_unreachable_before_send_keeps_a_retryable_submission() {
    let f = Fixture::new().await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let alias = "unreachable";
    let cfg_path = f.server.directory().join("refused_config");
    fs::write(
        &cfg_path,
        fs::read_to_string(f.server.config_path())
            .unwrap()
            .replace(
                &format!("Host {}", f.server.alias()),
                &format!("Host {alias}"),
            )
            .replace(
                &format!("Port {}", f.server.port()),
                &format!("Port {port}"),
            ),
    )
    .unwrap();
    let mut cfg = config(f.server.directory(), alias);
    cfg.ssh_config = Some(cfg_path);
    cfg.root = "~/never-launched".into();
    let conn = {
        let config = cfg;
        let store = f.store();
        let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, f.transport_clock.clone(), auth)
    }
    .unwrap();
    let ssh = Ssh::new(alias.into(), "~/never-launched".into(), conn);
    let mut intent = f.intent("blocked-network", "true", "ssh_job");
    intent.launch.target = Some(alias.into());
    f.submit(&ssh, &intent).await;
    let state = f.store().engine_run_state(&intent.run.id).unwrap().unwrap();
    assert!(
        matches!(state.contact, crate::store::Contact::Unreachable { cause, .. } if cause == "Connection refused")
    );
    assert_eq!(ssh.conn.state().borrow().status, Status::Backoff);
    assert!(state.submission_fence.is_none());
    assert_eq!(
        f.store().get_run(&intent.run.id).unwrap().unwrap().status,
        "submitting"
    );
    assert!(!f
        .server
        .home()
        .join("never-launched/runs/blocked-network")
        .exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_launch_written_with_response_lost_waits_for_the_host_then_recovers() {
    let f = Fixture::new().await;
    let command = f.server.directory().join("command");
    let original = fs::read_to_string(&command).unwrap();
    fs::write(
        &command,
        original.replace(
            "export HOME",
            "export HOME\nexport ORX_TEST_CRASH_AT=launch_published",
        ),
    )
    .unwrap();
    f.server.proxy().reset().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.ssh.conn.state().borrow().status == Status::Live {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    f.ssh.conn.retry().await.unwrap();
    let intent = f.intent("lost-response", "echo launched", "ssh_job");
    f.submit(f.ssh.as_ref(), &intent).await;
    let state = f.store().engine_run_state(&intent.run.id).unwrap().unwrap();
    // Recovery could not ask the host that went away with the reply.
    assert!(matches!(
        state.contact,
        crate::store::Contact::Unreachable { .. }
    ));
    assert!(state.submission_fence.is_some());
    assert!(f.dir(&intent.run.id).join("launch.json").exists());
    assert_eq!(
        f.store().get_run(&intent.run.id).unwrap().unwrap().status,
        "submitting"
    );
    fs::write(command, original).unwrap();
    f.transport_clock.advance(Duration::from_secs(7));
    live(&f.ssh.conn).await;
    f.clock.advance(Duration::from_secs(7));
    f.engine
        .recover(f.ssh.as_ref(), &intent.run.id)
        .await
        .unwrap();
    assert_eq!(
        f.terminal(f.ssh.as_ref(), &intent.run.id).await.status,
        "done"
    );
}

#[test]
fn cancelled_receipts_use_the_latest_process_proof_and_describe_unchecked_processes() {
    use orx_host_proto::{CancelOutcome as Outcome, ProcessOutcome, RunState, Summary, Survivor};
    let mut summary = Summary {
        started_at: None,
        state: RunState::Exited {
            receipt: orx_host_proto::ExitReceipt {
                finished_at: Some(1234),
                code: Some(0),
                signal: Some(15),
                reason: ExitReason::Cancelled,
                processes: vec![Survivor {
                    pid: 123,
                    starttime: 1,
                    state: "S".into(),
                }],
                outcome: Outcome::Survivors,
                detail: None,
            },
        },
        log_len: None,
        notify_len: 0,
        cleanup: None,
    };
    assert!(matches!(
        &super::ssh::observations("run".into(), &summary)[0].1,
        Observation::CancelReceipt {
            finished_at: Some(1234),
            remaining: 1,
            note: None,
            ..
        }
    ));
    for unreadable in [vec![], vec![789]] {
        summary.cleanup = Some(ProcessOutcome {
            processes: vec![],
            outcome: Outcome::Unverifiable {
                unreadable: unreadable.clone(),
            },
        });
        let observed = super::ssh::observations("run".into(), &summary);
        assert!(matches!(&observed[0].1,
            Observation::CancelReceipt { remaining: 0, note: Some(note), .. } if *note == unreadable));
    }
    summary.cleanup = Some(ProcessOutcome {
        processes: vec![],
        outcome: Outcome::Verified,
    });
    assert!(matches!(
        &super::ssh::observations("run".into(), &summary)[0].1,
        Observation::CancelReceipt {
            remaining: 0,
            note: None,
            ..
        }
    ));
    summary.started_at = Some(500);
    let observed = super::ssh::observations("run".into(), &summary);
    assert!(matches!(
        &observed[0].1,
        Observation::Running {
            started_at: Some(500)
        }
    ));
    assert!(matches!(
        &observed[1].1,
        Observation::CancelReceipt {
            finished_at: Some(1234),
            ..
        }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_checks_accepted_cancel_before_and_during_resumed_staging() {
    use orx_host_proto::LaunchIntent;
    for during in [false, true] {
        let f =
            Fixture::with_host_env("export ORX_TEST_CROSS_DEVICE=1\nexport ORX_TEST_SLOW_RPC=100")
                .await;
        let archive = f.server.directory().join("resume.tar");
        fs::write(&archive, vec![b'x'; 4 << 20]).unwrap();
        let bytes = fs::read(&archive).unwrap();
        let sha = orx_host::sha(&bytes);
        let mut intent = f.intent("resume", "touch ran", "ssh_job");
        intent.run.backend_json = json!({"kind":"ssh_job", "sourceDigest":sha, "sourcePath":archive, "sourceSize":bytes.len()}).to_string();
        let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
        let mut row = submitting(&intent);
        row.metadata = &metadata;
        f.engine.insert(row).unwrap();
        assert!(f
            .store()
            .set_submission_fence(
                "resume",
                1,
                &crate::store::SubmissionFence {
                    taken_at: 1,
                    submitter: None
                }
            )
            .unwrap());
        let dir = f.dir("resume");
        fs::create_dir_all(&dir).unwrap();
        orx_host::publish(
            &dir.join("launch.json"),
            &LaunchIntent {
                run: "resume".into(),
                digest: intent.spec_digest.clone(),
                run_sh: "touch ran".into(),
                env: Default::default(),
                source: Some(sha.clone()),
                cancel_sh: None,
            },
        )
        .unwrap();
        let accept = || {
            assert!(f
                .store()
                .accept_run_cancel("resume", Some("user"))
                .unwrap()
                .is_some());
            assert!(
                !f.store()
                    .get_run("resume")
                    .unwrap()
                    .unwrap()
                    .cancel_requested
            );
        };
        if during {
            f.ssh
                .stage_blob(&sha, &archive, bytes.len() as u64, &|| false)
                .await
                .unwrap();
            let part = dir.join(format!("source.tar.{sha}.part"));
            let recovery = f.engine.recover(f.ssh.as_ref(), "resume");
            let cancellation = async {
                exists(&part).await;
                accept();
            };
            let (result, ()) = tokio::join!(recovery, cancellation);
            result.unwrap();
            assert!(fs::metadata(part).unwrap().len() < bytes.len() as u64);
        } else {
            accept();
            f.engine.recover(f.ssh.as_ref(), "resume").await.unwrap();
        }
        assert_eq!(
            f.terminal(f.ssh.as_ref(), "resume").await.status,
            "cancelled"
        );
        for file in ["started", "run.sh", "ran", "source.tar"] {
            assert!(!dir.join(file).exists(), "{file}");
        }
        let receipt: orx_host_proto::ExitReceipt =
            orx_host::read_json(&dir.join("exit.json")).unwrap();
        assert_eq!(receipt.reason, ExitReason::Cancelled);
        assert_eq!(receipt.outcome, orx_host_proto::CancelOutcome::Verified);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_recovery_cancels_a_fully_staged_intent_before_launch_repair() {
    let f = Fixture::new().await;
    let intent = f.intent("staged-cancel", "touch should-not-run", "ssh_job");
    f.engine.insert(submitting(&intent)).unwrap();
    f.store()
        .set_submission_fence(
            &intent.run.id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let dir = f.dir(&intent.run.id);
    fs::create_dir_all(dir.join("repo")).unwrap();
    let archive = dir.join("source.tar");
    assert!(crate::process::command("tar")
        .args(["-cf"])
        .arg(&archive)
        .arg("-C")
        .arg(dir.join("repo"))
        .arg(".")
        .status()
        .unwrap()
        .success());
    let source = orx_host::sha(&fs::read(archive).unwrap());
    orx_host::publish(
        &dir.join("launch.json"),
        &orx_host_proto::LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh: intent.run.command.clone(),
            env: Default::default(),
            source: Some(source),
            cancel_sh: None,
        },
    )
    .unwrap();
    fs::write(dir.join("run.sh"), &intent.run.command).unwrap();
    f.store()
        .accept_run_cancel(&intent.run.id, Some("test"))
        .unwrap();
    f.engine
        .recover(f.ssh.as_ref(), &intent.run.id)
        .await
        .unwrap();
    assert_eq!(
        f.terminal(f.ssh.as_ref(), &intent.run.id).await.status,
        "cancelled"
    );
    assert!(dir.join("cancel.req").exists());
    assert!(!dir.join("started").exists());
    assert!(!dir.join("wrapper.log").exists());
    assert!(!dir.join("repo/should-not-run").exists());
    assert!(!dir.join("should-not-run").exists());

    let absent = f.intent("absent-cancel", "touch should-not-run", "ssh_job");
    assert!(matches!(
        f.ssh.recover_with_cancel(&absent, &|| true).await,
        Recovery::Absent
    ));
    assert!(!f.dir(&absent.run.id).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_recovery_finishes_batch_only_preparation_without_a_scheduler_record() {
    let f = Fixture::new().await;
    f.shim(
        "sbatch",
        "cat >/dev/null; echo submit >> \"$HOME/submissions\"; printf '123\\n'",
    );
    f.shim("squeue", "exit 0");
    f.shim("sacct", "exit 0");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("batch-only", "true", "slurm_job");
    f.engine.insert(submitting(&intent)).unwrap();
    f.store()
        .set_submission_fence(
            &intent.run.id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let dir = f.dir(&intent.run.id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("job.sbatch"), "interrupted batch preparation").unwrap();
    f.engine.recover(&slurm, &intent.run.id).await.unwrap();
    assert_eq!(fs::read_to_string(dir.join("slurm.job")).unwrap(), "123");
    assert_eq!(
        fs::read_to_string(f.server.home().join("submissions")).unwrap(),
        "submit\n"
    );
    assert!(dir.join("launch.json").exists());
    assert!(matches!(
        watched(&f, &intent.run.id).handle,
        Handle::Slurm { job: Some(_), .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_foreign_node_uses_scheduler_without_inspecting_or_signalling_local_pid() {
    let f = Fixture::new().await;
    f.shim("sbatch", "cat >/dev/null; printf '123\\n'");
    f.shim("squeue", "printf '123|foreign-node|RUNNING\\n'");
    f.shim("sacct", "exit 1");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("foreign-node", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    let dir = f.dir(&intent.run.id);
    let mut identity = orx_host::platform::identity(std::process::id()).unwrap();
    identity.node.push_str("-compute-node");
    orx_host::publish(&dir.join("wrapper.json"), &identity).unwrap();
    fs::write(dir.join("started"), "foreign-tag").unwrap();
    let run = watched(&f, &intent.run.id);
    let observed = slurm
        .observe(std::slice::from_ref(&run), Duration::ZERO)
        .await;
    assert!(observed
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Running { .. })));
    assert!(!observed
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Unreachable { .. })));
    assert!(matches!(slurm.cancel(&run).await, CancelOutcome::Pending));
    assert!(orx_host::platform::process(identity.pid).unwrap().live());
    f.shim("squeue", "printf '123|foreign-node|NODE_FAIL\\n'");
    f.clock.advance(Duration::from_secs(31));
    let observed = slurm
        .observe(std::slice::from_ref(&run), Duration::ZERO)
        .await;
    assert!(!observed
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Failed { .. })));
    tokio::time::sleep(Duration::from_millis(150)).await;
    f.clock.advance(Duration::from_secs(31));
    let observed = slurm.observe(&[run], Duration::ZERO).await;
    assert!(observed.iter().any(|(_, observation)| matches!(observation, Observation::Failed { reason, .. } if reason == "scheduler NODE_FAIL; no receipt")));
    assert!(!dir.join("exit.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_receipt_survivors_do_not_offer_wrapper_cleanup_after_step_teardown() {
    let f = Fixture::new().await;
    f.shim("sbatch", "cat >/dev/null; printf '123\\n'");
    f.shim("squeue", "exit 0");
    f.shim("sacct", "exit 1");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("step-teardown", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    let receipt = orx_host_proto::ExitReceipt {
        code: Some(0),
        signal: None,
        reason: ExitReason::Exited,
        processes: vec![orx_host_proto::Survivor {
            pid: 12345,
            starttime: 1,
            state: "S".into(),
        }],
        outcome: orx_host_proto::CancelOutcome::Survivors,
        finished_at: None,
        detail: None,
    };
    orx_host::publish(&f.dir(&intent.run.id).join("exit.json"), &receipt).unwrap();
    let observed = slurm
        .observe(&[watched(&f, &intent.run.id)], Duration::ZERO)
        .await;
    assert!(observed
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Exited { code: Some(0), .. })));
    assert!(observed.iter().any(
        |(_, observation)| matches!(observation, Observation::Lingering { pids } if pids.is_empty())
    ));
    assert!(!observed.iter().any(|(_, observation)| matches!(observation, Observation::Lingering { pids } if !pids.is_empty())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_merge_keeps_invalid_and_unreadable_receipts_authoritative() {
    use orx_host_proto::{Request, Response};
    let f = Fixture::new().await;
    f.shim("sbatch", "cat >/dev/null; printf '123\\n'");
    f.shim("squeue", "printf '123|receipt-errors|FAILED\\n'");
    f.shim("sacct", "exit 1");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("receipt-errors", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    let run = watched(&f, "receipt-errors");
    slurm
        .observe(std::slice::from_ref(&run), Duration::ZERO)
        .await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    f.clock.advance(Duration::from_secs(31));
    let observations = slurm
        .observe(std::slice::from_ref(&run), Duration::ZERO)
        .await;
    assert!(observations
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Failed { .. })));
    let receipt = f.dir("receipt-errors").join("exit.json");
    for bytes in [b"{".as_slice(), br#"{"code":0,"signal":null,"reason":"future","processes":[],"outcome":{"kind":"verified"}}"#] {
        fs::write(&receipt, bytes).unwrap();
        let observations = slurm.observe(std::slice::from_ref(&run), Duration::ZERO).await;
        assert!(observations.iter().any(|(_, observation)| matches!(observation, Observation::Unreconciled { .. })));
        assert!(!observations.iter().any(|(_, observation)| matches!(observation, Observation::Failed { .. } | Observation::Lost { .. })));
    }
    fs::remove_file(&receipt).unwrap();
    fs::create_dir(&receipt).unwrap();
    let observations = slurm
        .observe(std::slice::from_ref(&run), Duration::ZERO)
        .await;
    assert!(observations
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Unreachable { .. })));
    // A receipt metadata error is also authoritative through the scheduler query.
    fs::remove_dir(&receipt).unwrap();
    std::os::unix::fs::symlink("exit.json", &receipt).unwrap();
    let Response::SlurmQueue { jobs, .. } = f
        .ssh
        .request(Request::SlurmQueue {
            names: vec![run.id],
            legacy: vec![],
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        jobs[0].summary.state,
        orx_host_proto::RunState::Unreachable { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paused_container_cancel_retries_until_unpause_or_removal() {
    use orx_host_proto::{ExitReceipt, Request, Response, RunState};
    let f = Fixture::with_host_env("unset ORX_TEST_FAST_STOPS").await;
    f.shim("docker", r#"case "$1 $2" in
'container ls') [ -f "$HOME/removed" ] || printf cid;;
'container inspect') if [ -f "$HOME/unpaused" ]; then printf 'generation running'; else printf 'generation paused'; fi;;
'exec -i') printf x >> "$HOME/hook-attempts"; [ -f "$HOME/unpaused" ];;
*) exit 1;; esac"#);
    for remove in [false, true] {
        let id = if remove {
            "removed-container"
        } else {
            "unpaused-container"
        };
        let container = crate::jobs::ssh::ContainerRun {
            reference: "fixture".into(),
            id: "cid".into(),
            started_at: "generation".into(),
            run_dir: "/unused".into(),
            unknown: Default::default(),
        };
        let mut intent = f.intent(id, "touch ready; sleep 60", "ssh_job");
        intent.launch.options = json!({"cancel_sh":container.engine_cancel_sh()});
        f.submit(f.ssh.as_ref(), &intent).await;
        exists(&f.dir(id).join("ready")).await;
        let run = watched(&f, id);
        assert!(matches!(f.ssh.cancel(&run).await, CancelOutcome::Pending));
        let attempts = f.server.home().join("hook-attempts");
        tokio::time::timeout(Duration::from_secs(60), async {
            while fs::read(&attempts).map_or(0, |bytes| bytes.len()) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!f.dir(id).join("exit.json").exists());
        fs::write(
            f.server
                .home()
                .join(if remove { "removed" } else { "unpaused" }),
            "",
        )
        .unwrap();
        exists(&f.dir(id).join("exit.json")).await;
        let receipt: ExitReceipt = orx_host::read_json(&f.dir(id).join("exit.json")).unwrap();
        assert_eq!(receipt.reason, ExitReason::Cancelled);
        assert!(matches!(
            receipt.outcome,
            orx_host_proto::CancelOutcome::Unverifiable { .. }
        ));
        let Response::Changes { runs } = f
            .ssh
            .request(Request::Changes {
                watch: vec![(id.into(), super::ssh::missing())],
                follow_logs: vec![],
                wait_ms: 0,
            })
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(matches!(runs[0].1.state, RunState::Exited { .. }));
        for marker in ["unpaused", "hook-attempts"] {
            let _ = fs::remove_file(f.server.home().join(marker));
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_cancel_reaches_new_sessions_and_double_fork_children() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let script = "python3 -c 'import os,time; p=os.fork(); p and os._exit(0); os.setsid(); p=os.fork(); p and os._exit(0); f=open(\"daemon.tmp\",\"w\"); f.write(str(os.getpid())); f.close(); os.rename(\"daemon.tmp\",\"daemon\"); time.sleep(60)'\nwhile [ ! -f daemon ]; do sleep .01; done\nsleep 60";
    f.submit(&local, &f.intent("fork-local", script, "local_job"))
        .await;
    let dir = root.join("fork-local");
    exists(&dir.join("daemon")).await;
    let pid: u32 = fs::read_to_string(dir.join("daemon"))
        .unwrap()
        .parse()
        .unwrap();
    let identity = orx_host::platform::identity(pid).unwrap();
    f.engine.cancel(&local, "fork-local", "test").await.unwrap();
    exists(&dir.join("exit.json")).await;
    let receipt = orx_host::wrapper::receipt(&dir).unwrap().unwrap();
    assert!(receipt.processes.is_empty());
    assert!(!orx_host::platform::alive(&identity).unwrap());
    let expected = super::ssh::observations(
        "fork-local".into(),
        &orx_host_proto::Summary {
            started_at: orx_host::wrapper::started_at(&dir),
            state: orx_host_proto::RunState::Exited { receipt },
            log_len: Some(fs::metadata(dir.join("log")).unwrap().len()),
            notify_len: 0,
            cleanup: None,
        },
    );
    assert_eq!(
        local
            .observe(&[watched(&f, "fork-local")], Duration::ZERO)
            .await,
        expected
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_cleanup_stops_a_worker_that_outlives_the_payload() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let script = "python3 -c 'import os,time; open(\"worker\",\"w\").write(str(os.getpid())); time.sleep(60)' >/dev/null 2>&1 &\nwhile [ ! -f worker ]; do sleep .01; done";
    f.submit(&local, &f.intent("cleanup-local", script, "local_job"))
        .await;
    let dir = root.join("cleanup-local");
    exists(&dir.join("exit.json")).await;
    let receipt = orx_host::wrapper::receipt(&dir).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::Exited);
    assert!(!receipt.processes.is_empty());
    let pid: u32 = fs::read_to_string(dir.join("worker"))
        .unwrap()
        .parse()
        .unwrap();
    let identity = orx_host::platform::identity(pid).unwrap();
    let run = watched(&f, "cleanup-local");
    assert!(matches!(
        Backend::cleanup(&local, &run).await,
        CancelOutcome::Pending
    ));
    assert!(dir.join("cleanup.req").exists());
    exists(&dir.join("cleanup.json")).await;
    assert!(orx_host::wrapper::cleanup_result(&dir)
        .unwrap()
        .unwrap()
        .processes
        .is_empty());
    assert!(!orx_host::platform::alive(&identity).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_recovery_never_starts_a_staged_payload_after_an_accepted_cancel() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let intent = f.intent("staged-cancel", "touch should-not-run", "local_job");
    f.engine.insert(submitting(&intent)).unwrap();
    f.store()
        .set_submission_fence(
            &intent.run.id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let dir = root.join(&intent.run.id);
    fs::create_dir_all(&dir).unwrap();
    orx_host::publish(
        &dir.join("launch.json"),
        &orx_host_proto::LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh: intent.run.command.clone(),
            env: Default::default(),
            source: None,
            cancel_sh: None,
        },
    )
    .unwrap();
    f.store()
        .accept_run_cancel(&intent.run.id, Some("test"))
        .unwrap();
    f.engine.recover(&local, &intent.run.id).await.unwrap();
    f.poll(&local, &[&intent.run.id], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run(&intent.run.id).unwrap().unwrap().status,
        "cancelled"
    );
    assert!(!dir.join("run.sh").exists());
    assert!(!dir.join("started").exists());
    assert!(!dir.join("should-not-run").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_launch_without_bash_is_rejected_before_its_intent() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let mut intent = f.intent("no-bash", "touch should-not-run", "local_job");
    intent.launch.options = json!({"env":{"PATH":"/nonexistent"}});
    let store = std::sync::Mutex::new(f.store());
    let fence = Fence {
        store: &store,
        run_id: &intent.run.id,
        attempt: 1,
        taken_at: 1,
    };
    let Err(LaunchError::Rejected { reason }) = local.launch(&intent, &fence).await else {
        panic!("a launch without bash was not rejected")
    };
    assert!(reason.contains("bash"), "{reason}");
    assert!(!root.join("no-bash/launch.json").exists());
}

fn local_intent_on_disk(dir: &Path, intent: &RunIntent, env: &[(&str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    orx_host::publish(
        &dir.join("launch.json"),
        &orx_host_proto::LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh: intent.run.command.clone(),
            env: env
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            source: None,
            cancel_sh: None,
        },
    )
    .unwrap();
    orx_host::publish_bytes(&dir.join("run.sh"), intent.run.command.as_bytes()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_wrapper_that_cannot_start_the_payload_fails_the_run_with_its_cause() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let intent = f.intent("spawn-failed", "touch should-not-run", "local_job");
    let dir = root.join(&intent.run.id);
    local_intent_on_disk(&dir, &intent, &[("PATH", "/nonexistent")]);
    let wrapped = crate::process::command(crate::engine::transport::local_binary(&root).unwrap())
        .arg("wrap")
        .arg(&dir)
        .stderr(fs::File::create(dir.join("wrapper.log")).unwrap())
        .status()
        .unwrap();
    assert!(!wrapped.success());
    let run = WatchedRun {
        id: intent.run.id.clone(),
        handle: Handle::Local {
            dir: dir.to_string_lossy().into_owned(),
        },
    };
    let observed = local.observe(&[run], Duration::ZERO).await;
    let reason = observed
        .iter()
        .find_map(|(_, observation)| match observation {
            Observation::Failed { reason, .. } => Some(reason),
            _ => None,
        })
        .expect("a run whose payload never started did not fail");
    assert!(reason.contains("run.sh"), "{reason}");
    assert!(!dir.join("should-not-run").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_cancel_ends_a_run_whose_wrapper_died_before_starting_it() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let intent = f.intent("died-before-start", "touch should-not-run", "local_job");
    let dir = root.join(&intent.run.id);
    local_intent_on_disk(&dir, &intent, &[]);
    let mut identity = orx_host::platform::identity(std::process::id()).unwrap();
    identity.starttime += 1;
    orx_host::publish(&dir.join("wrapper.json"), &identity).unwrap();
    fs::write(dir.join("cancel.req"), "").unwrap();
    let run = WatchedRun {
        id: intent.run.id.clone(),
        handle: Handle::Local {
            dir: dir.to_string_lossy().into_owned(),
        },
    };
    let observed = local.observe(&[run], Duration::ZERO).await;
    assert!(
        observed
            .iter()
            .any(|(_, observation)| matches!(observation, Observation::CancelReceipt { .. })),
        "{observed:?}"
    );
    assert!(!dir.join("started").exists());
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_after_a_legacy_local_run_finished_keeps_its_result() {
    let f = Fixture::new().await;
    let dir = f.server.directory().join("finished-legacy");
    fs::create_dir_all(&dir).unwrap();
    let mut finished = crate::process::command("true").spawn().unwrap();
    finished.wait().unwrap();
    fs::write(dir.join("pid"), finished.id().to_string()).unwrap();
    fs::write(dir.join("exit_code"), "0\n").unwrap();
    let descriptor =
        crate::jobs::BackendDescriptor::parse(&json!({"kind":"local_job","jobId":dir}).to_string())
            .unwrap();
    let run = WatchedRun {
        id: "finished-legacy".into(),
        handle: Handle::Legacy {
            descriptor: Box::new(descriptor),
        },
    };
    let local = Local::new(f.dir("unused"));
    assert!(matches!(local.cancel(&run).await, CancelOutcome::Pending));
    assert!(local
        .observe(&[run], Duration::ZERO)
        .await
        .iter()
        .any(|(_, observation)| matches!(observation, Observation::Exited { code: Some(0), .. })));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_recovery_waits_for_the_wrapper_start_marker_before_returning_a_handle() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    let intent = f.intent("starting-local", "sleep 60", "local_job");
    let dir = root.join(&intent.run.id);
    fs::create_dir_all(&dir).unwrap();
    orx_host::publish(
        &dir.join("launch.json"),
        &orx_host_proto::LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh: intent.run.command.clone(),
            env: Default::default(),
            source: None,
            cancel_sh: None,
        },
    )
    .unwrap();
    let gate = orx_host::lock(&dir.join("start.lock")).unwrap().unwrap();
    let recovery = local.recover(&intent);
    tokio::pin!(recovery);
    let wrapper = dir.join("wrapper.json");
    tokio::select! {
        biased;
        result = &mut recovery => panic!("recovery returned before started: {result:?}"),
        () = exists(&wrapper) => {},
    }
    assert!(!dir.join("started").exists());
    drop(gate);
    let Recovery::Found(handle) = recovery.await else {
        panic!("wrapper did not start")
    };
    assert!(dir.join("started").exists());
    local
        .cancel(&WatchedRun {
            id: intent.run.id.clone(),
            handle,
        })
        .await;
    exists(&dir.join("exit.json")).await;
}

#[tokio::test]
async fn abandon_before_handle_recovery_attaches_once_and_settles_through_real_ssh() {
    let mut f = Fixture::new().await;
    let intent = f.intent(
        "abandoned-response",
        "echo payload >> count; sleep 60 &",
        "ssh_job",
    );
    f.engine.insert(submitting(&intent)).unwrap();
    f.store()
        .set_submission_fence(
            &intent.run.id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let fence = Fence {
        store: &f.engine.store,
        run_id: &intent.run.id,
        attempt: 1,
        taken_at: 1,
    };
    let launched = f.ssh.launch(&intent, &fence).await.unwrap();
    assert!(matches!(launched, Handle::Ssh { .. }));
    exists(&f.dir(&intent.run.id).join("exit.json")).await;
    f.engine
        .unreconciled(&intent.run.id, &crate::error::anyhow!("lost reply"))
        .unwrap();
    assert!(f.engine.abandon(&intent.run.id).unwrap());
    f.engine = Engine::new(f.store(), f.clock.clone());
    f.engine
        .recover(f.ssh.as_ref(), &intent.run.id)
        .await
        .unwrap();
    let row = f.store().get_run(&intent.run.id).unwrap().unwrap();
    assert_eq!(row.status, "lost");
    assert!(handle_of(&row).is_some());
    assert!(f.store().log_collection_pending(&intent.run.id).unwrap());
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            f.poll(f.ssh.as_ref(), &[&intent.run.id], Duration::from_millis(50))
                .await;
            if f.store()
                .engine_run_state(&intent.run.id)
                .unwrap()
                .unwrap()
                .contact
                == crate::store::Contact::Live
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    for _ in 0..3 {
        f.engine
            .recover(f.ssh.as_ref(), &intent.run.id)
            .await
            .unwrap();
    }
    assert_eq!(
        f.store().get_run(&intent.run.id).unwrap().unwrap().status,
        "lost"
    );
    assert_eq!(
        fs::read_to_string(f.dir(&intent.run.id).join("count")).unwrap(),
        "payload\n"
    );
    let notices = f.store().run_notice_batch("chat").unwrap();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.kind == crate::store::TERMINAL)
            .count(),
        1
    );
}

#[tokio::test]
async fn a_verified_cancel_receipt_after_a_lost_reply_settles_the_recovered_tombstone() {
    let f = Fixture::new().await;
    let intent = f.intent("already-cancelled", "true", "ssh_job");
    f.engine.insert(submitting(&intent)).unwrap();
    f.store()
        .set_submission_fence(
            &intent.run.id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let dir = f.dir(&intent.run.id);
    fs::create_dir_all(&dir).unwrap();
    orx_host::publish(
        &dir.join("exit.json"),
        &orx_host_proto::ExitReceipt {
            finished_at: None,
            code: None,
            signal: Some(15),
            reason: ExitReason::Cancelled,
            processes: Vec::new(),
            outcome: orx_host_proto::CancelOutcome::Verified,
            detail: None,
        },
    )
    .unwrap();
    assert!(f.engine.abandon(&intent.run.id).unwrap());
    f.engine
        .recover(f.ssh.as_ref(), &intent.run.id)
        .await
        .unwrap();
    assert!(handle_of(&f.store().get_run(&intent.run.id).unwrap().unwrap()).is_some());
    f.poll(f.ssh.as_ref(), &[&intent.run.id], Duration::from_millis(50))
        .await;
    assert_eq!(
        f.store()
            .engine_run_state(&intent.run.id)
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Live
    );
    assert_eq!(
        f.store().get_run(&intent.run.id).unwrap().unwrap().status,
        "lost"
    );
    f.engine
        .recover(f.ssh.as_ref(), &intent.run.id)
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_tombstone_cleanup_keeps_the_once_per_connection_latch() {
    let f = Fixture::new().await;
    let root = f.server.directory().join("local-runs");
    let local = Local::new(root.clone());
    for (id, backend, kind, dir) in [
        (
            "pending-local",
            &local as &dyn Backend,
            "local_job",
            root.join("pending-local"),
        ),
        (
            "pending-ssh",
            f.ssh.as_ref() as &dyn Backend,
            "ssh_job",
            f.dir("pending-ssh"),
        ),
    ] {
        f.submit(
            backend,
            &f.intent(id, "touch ready; while :; do sleep .1; done", kind),
        )
        .await;
        exists(&dir.join("ready")).await;
        let identity: orx_host_proto::ProcessIdentity =
            orx_host::read_json(&dir.join("wrapper.json")).unwrap();
        assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGSTOP) }, 0);
        assert!(f.engine.abandon(id).unwrap());
        let first = f.engine.deliver_cancel(backend, id).await;
        let latched = f.engine.memory().cancels_sent.contains(id);
        let second = f.engine.deliver_cancel(backend, id).await;
        assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGCONT) }, 0);
        first.unwrap();
        second.unwrap();
        assert!(latched);
        assert!(f.engine.memory().cancels_sent.contains(id));
        exists(&dir.join("exit.json")).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slurm_cancel_survivors_settle_on_scheduler_teardown_without_a_new_receipt() {
    let f = Fixture::new().await;
    f.shim("sbatch", "cat > /dev/null; printf '123\n'");
    f.shim(
        "squeue",
        "echo query >> \"$HOME/scheduler-calls\"; printf '123|survivors|RUNNING\n'",
    );
    f.shim("sacct", "exit 1");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("survivors", "sleep 60", "slurm_job");
    f.submit(&slurm, &intent).await;
    let calls = f.server.home().join("scheduler-calls");
    let submitted_calls = fs::read_to_string(&calls)
        .unwrap_or_default()
        .lines()
        .count();
    let dir = f.dir("survivors");
    fs::write(dir.join("started"), "survivors:tag\n").unwrap();
    let mut identity = orx_host::platform::identity(std::process::id()).unwrap();
    identity.node = "compute-node".into();
    orx_host::publish(&dir.join("wrapper.json"), &identity).unwrap();
    orx_host::publish(
        &f.dir("survivors").join("exit.json"),
        &orx_host_proto::ExitReceipt {
            finished_at: None,
            code: None,
            signal: None,
            reason: ExitReason::Cancelled,
            processes: vec![orx_host_proto::Survivor {
                pid: 123456,
                starttime: 1,
                state: "D".into(),
            }],
            outcome: orx_host_proto::CancelOutcome::Survivors,
            detail: None,
        },
    )
    .unwrap();
    let receipt = fs::read(dir.join("exit.json")).unwrap();
    f.poll(&slurm, &["survivors"], Duration::ZERO).await;
    assert_eq!(
        f.store().get_run("survivors").unwrap().unwrap().status,
        "cancelled"
    );
    assert!(matches!(
        f.store()
            .engine_run_state("survivors")
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Stopping { count: 1, .. }
    ));
    assert_eq!(
        fs::read_to_string(&calls).unwrap().lines().count(),
        submitted_calls + 1
    );
    f.shim(
        "squeue",
        "echo query >> \"$HOME/scheduler-calls\"; printf '123|survivors|COMPLETED\n'",
    );
    f.clock.advance(Duration::from_secs(31));
    f.poll(&slurm, &["survivors"], Duration::ZERO).await;
    assert_eq!(
        f.store()
            .engine_run_state("survivors")
            .unwrap()
            .unwrap()
            .contact,
        crate::store::Contact::Live
    );
    assert_eq!(
        fs::read_to_string(&calls).unwrap().lines().count(),
        submitted_calls + 2
    );
    f.clock.advance(Duration::from_secs(31));
    f.poll(&slurm, &["survivors"], Duration::ZERO).await;
    assert_eq!(
        fs::read_to_string(&calls).unwrap().lines().count(),
        submitted_calls + 2
    );
    let notices = f.store().run_notice_batch("chat").unwrap();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.dedupe == "cancel-stopped")
            .count(),
        1
    );
    assert_eq!(fs::read(dir.join("exit.json")).unwrap(), receipt);
    let proof = orx_host::wrapper::cleanup_result(&dir).unwrap().unwrap();
    assert!(proof.processes.is_empty());
    assert_eq!(proof.outcome, orx_host_proto::CancelOutcome::Verified);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrapper_gone_cancel_ack_stays_pending_until_observation() {
    let f = Fixture::new().await;
    let intent = f.intent("gone", "true", "ssh_job");
    let dir = f.dir("gone");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("started"), "gone:tag").unwrap();
    let mut identity = orx_host::platform::identity(std::process::id()).unwrap();
    identity.starttime += 1;
    orx_host::publish(&dir.join("wrapper.json"), &identity).unwrap();
    f.engine.insert(submitting(&intent)).unwrap();
    let run = WatchedRun {
        id: "gone".into(),
        handle: f.ssh.handle(),
    };
    assert!(matches!(
        Backend::cancel(f.ssh.as_ref(), &run).await,
        CancelOutcome::Pending
    ));
    let local = Local::new(f.dir("unused"));
    let run = WatchedRun {
        id: "gone".into(),
        handle: Handle::Local {
            dir: dir.to_string_lossy().into_owned(),
        },
    };
    assert!(matches!(
        Backend::cancel(&local, &run).await,
        CancelOutcome::Pending
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_wrapper_survivors_settle_through_host_observation_after_engine_restart() {
    let mut f =
        Fixture::with_host_env("export ORX_TEST_DEFER_SIGNALS=\"$HOME/hold-signals\"").await;
    let hold = f.server.home().join("hold-signals");
    fs::write(&hold, "hold").unwrap();
    let id = "dead-wrapper";
    f.submit(
        f.ssh.as_ref(),
        &f.intent(id, "touch ready; exec sleep 60", "ssh_job"),
    )
    .await;
    let dir = f.dir(id);
    exists(&dir.join("ready")).await;
    f.engine.cancel(f.ssh.as_ref(), id, "test").await.unwrap();
    exists(&dir.join("exit.json")).await;
    let bytes = fs::read(dir.join("exit.json")).unwrap();
    f.poll(f.ssh.as_ref(), &[id], Duration::ZERO).await;
    assert!(matches!(
        f.store().engine_run_state(id).unwrap().unwrap().contact,
        crate::store::Contact::Stopping { count, .. } if count > 0
    ));
    let identity: orx_host_proto::ProcessIdentity =
        orx_host::read_json(&dir.join("wrapper.json")).unwrap();
    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGKILL) }, 0);
    tokio::time::timeout(Duration::from_secs(3), async {
        while orx_host::platform::alive(&identity).unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    f.engine = Engine::new(f.store(), f.clock.clone());
    f.poll(f.ssh.as_ref(), &[id], Duration::ZERO).await;
    let proof = orx_host::wrapper::cleanup_result(&dir).unwrap().unwrap();
    assert!(!proof.processes.is_empty());
    assert!(matches!(
        f.store().engine_run_state(id).unwrap().unwrap().contact,
        crate::store::Contact::Stopping { count, .. } if count == proof.processes.len()
    ));
    fs::remove_file(hold).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            f.poll(f.ssh.as_ref(), &[id], Duration::ZERO).await;
            if f.store().engine_run_state(id).unwrap().unwrap().contact
                == crate::store::Contact::Live
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    f.engine = Engine::new(f.store(), f.clock.clone());
    f.poll(f.ssh.as_ref(), &[id], Duration::ZERO).await;
    let notices = f.store().run_notice_batch("chat").unwrap();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.kind == crate::store::TERMINAL)
            .count(),
        1
    );
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.dedupe == "cancel-stopped")
            .count(),
        1
    );
    assert_eq!(fs::read(dir.join("exit.json")).unwrap(), bytes);
    let proof = orx_host::wrapper::cleanup_result(&dir).unwrap().unwrap();
    assert!(proof.processes.is_empty());
    // macOS can hide processes from a tag scan, so there a dead wrapper's stop stays unconfirmed.
    if cfg!(target_os = "macos") {
        assert!(matches!(
            proof.outcome,
            orx_host_proto::CancelOutcome::Unverifiable { .. }
        ));
    } else {
        assert_eq!(proof.outcome, orx_host_proto::CancelOutcome::Verified);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_survivors_settle_on_group_absence_without_reconnect() {
    use std::os::unix::process::CommandExt;
    let f = Fixture::new().await;
    let local = Local::new(f.server.home().join("legacy-local"));
    for (id, kind, backend) in [
        ("legacy-ssh", "ssh_job", f.ssh.as_ref() as &dyn Backend),
        ("legacy-local", "local_job", &local as &dyn Backend),
    ] {
        let dir = f.server.home().join(id);
        fs::create_dir(&dir).unwrap();
        let mut child = crate::process::command("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id().to_string();
        fs::write(dir.join("pid"), &pid).unwrap();
        let mut row = f.intent(id, "sleep 60", kind).run;
        row.status = "running".into();
        row.backend_json =
            json!({"kind":kind,"namespace":f.server.alias(),"jobId":dir}).to_string();
        f.store().upsert_run(&row).unwrap();
        assert_eq!(f.engine.scan_legacy().unwrap().len(), 1);
        f.engine
            .apply_cancel(&row, CancelOutcome::Survivors { count: 1 })
            .unwrap();
        f.engine.memory().cancels_sent.insert(id.into());
        let pending = f.store().engine_run_state(id).unwrap().unwrap().contact;
        assert_ne!(pending, crate::store::Contact::Live);
        f.poll(backend, &[id], Duration::ZERO).await;
        assert_eq!(
            f.store().engine_run_state(id).unwrap().unwrap().contact,
            pending
        );

        fs::write(dir.join("pid"), "invalid").unwrap();
        let failed = backend.observe(&[watched(&f, id)], Duration::ZERO).await;
        assert!(!failed.iter().any(|(_, observation)| {
            matches!(observation, Observation::Lingering { pids } if pids.is_empty())
        }));
        fs::remove_file(dir.join("pid")).unwrap();
        fs::create_dir(dir.join("pid")).unwrap();
        let failed = backend.observe(&[watched(&f, id)], Duration::ZERO).await;
        assert!(!failed.iter().any(|(_, observation)| {
            matches!(observation, Observation::Lingering { pids } if pids.is_empty())
        }));
        fs::remove_dir(dir.join("pid")).unwrap();
        fs::write(dir.join("pid"), pid).unwrap();
        assert!(f.engine.memory().cancels_sent.contains(id));
        child.kill().unwrap();
        child.wait().unwrap();
        f.poll(backend, &[id], Duration::ZERO).await;
        assert_eq!(
            f.store().engine_run_state(id).unwrap().unwrap().contact,
            crate::store::Contact::Live
        );
        assert_eq!(f.store().get_run(id).unwrap().unwrap().status, "cancelled");
        assert!(!f.engine.memory().cancels_sent.contains(id));
    }
}

/// A launch another orx-host still prepares holds launch.lock; recovery waits for it instead
/// of reading the run as absent, which would end a cancelled run before its payload starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_never_reads_a_launch_in_preparation_as_absent() {
    let f = Fixture::new().await;
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    f.shim("squeue", "exit 0");
    f.shim("sacct", "exit 0");
    for (backend, id) in [
        (f.ssh.as_ref() as &dyn Backend, "preparing-ssh"),
        (&slurm, "preparing-slurm"),
    ] {
        let kind = if id == "preparing-ssh" {
            "ssh_job"
        } else {
            "slurm_job"
        };
        let intent = f.intent(id, "touch ran", kind);
        let dir = f.dir(id);
        fs::create_dir_all(&dir).unwrap();
        if kind == "slurm_job" {
            fs::write(dir.join("job.sbatch"), "batch preparation in progress").unwrap();
        }
        let preparation = orx_host::lock(&dir.join("launch.lock")).unwrap().unwrap();
        let recovery = backend.recover_with_cancel(&intent, &|| true).await;
        assert!(
            matches!(&recovery, Recovery::Unknown { detail } if detail.contains("still being prepared")),
            "{id}: {recovery:?}"
        );
        drop(preparation);
        assert!(
            matches!(
                backend.recover_with_cancel(&intent, &|| true).await,
                Recovery::Absent
            ),
            "{id}"
        );
        assert!(!dir.join("ran").exists());
    }
}

/// A cancel that reached no wrapper is honoured by the next feed instead of respawning one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dead_wrapper_with_a_pending_cancel_is_cancelled_not_respawned() {
    let f = Fixture::new().await;
    let intent = f.intent("cancel-pending", "touch ran", "ssh_job");
    let dir = f.dir("cancel-pending");
    fs::create_dir_all(&dir).unwrap();
    orx_host::publish(
        &dir.join("launch.json"),
        &orx_host_proto::LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh: intent.run.command.clone(),
            env: Default::default(),
            source: None,
            cancel_sh: None,
        },
    )
    .unwrap();
    fs::write(dir.join("cancel.req"), "").unwrap();
    let run = WatchedRun {
        id: intent.run.id.clone(),
        handle: f.ssh.handle(),
    };
    let observed = f.ssh.observe(&[run], Duration::ZERO).await;
    assert!(
        observed
            .iter()
            .any(|(_, observation)| matches!(observation, Observation::CancelReceipt { .. })),
        "{observed:?}"
    );
    let receipt = orx_host::wrapper::receipt(&dir).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    for never in ["started", "wrapper.log", "ran"] {
        assert!(!dir.join(never).exists(), "{never}");
    }
}

/// A wrapper that keeps dying before it starts the payload, leaving nothing behind, is given
/// up on with a spawn_failed receipt that ends the run, instead of respawning forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrapper_dying_before_start_is_respawned_a_bounded_number_of_times() {
    let f = Fixture::with_host_env("export ORX_TEST_CRASH_AT=wrapper_locked").await;
    let intent = f.intent("never-starts", "touch ran", "ssh_job");
    f.submit(f.ssh.as_ref(), &intent).await;
    let run = f.terminal(f.ssh.as_ref(), "never-starts").await;
    assert_eq!(run.status, "failed");
    let dir = f.dir("never-starts");
    let receipt = orx_host::wrapper::receipt(&dir).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::SpawnFailed);
    let reason = run.result_markdown.unwrap_or_default();
    assert!(reason.contains("exited 5 times"), "{reason}");
    assert!(!dir.join("started").exists());
    assert!(!dir.join("ran").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unrunnable_sbatch_fails_the_launch_with_what_to_fix() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    f.shim("sbatch", "exit 0");
    let sbatch = f.server.directory().join("shims/sbatch");
    fs::set_permissions(&sbatch, fs::Permissions::from_mode(0o600)).unwrap();
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("no-sbatch", "true", "slurm_job");
    f.submit(&slurm, &intent).await;
    let run = f.store().get_run("no-sbatch").unwrap().unwrap();
    assert_eq!(run.status, "failed");
    let reason = run.result_markdown.unwrap();
    assert!(
        reason.contains("sbatch can't run") && reason.contains("PATH"),
        "{reason}"
    );
    for retracted in ["launch.json", "job.sbatch"] {
        assert!(!f.dir("no-sbatch").join(retracted).exists(), "{retracted}");
    }
}

/// A host orx-host can't run on fails a launch that was never sent, naming the host and why,
/// rather than waiting on a connection that can never succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsupported_host_rejects_the_launch_instead_of_waiting() {
    let f = Fixture::new().await;
    f.shim(
        "uname",
        "case \"$1\" in -s) echo FreeBSD ;; *) /usr/bin/uname \"$@\" ;; esac",
    );
    let mut cfg = config(f.server.directory(), f.server.alias());
    cfg.ssh_config = Some(f.server.config_path());
    cfg.root = "~/unsupported".into();
    let conn = {
        let store = f.store();
        let auth = crate::engine::transport::Auth::new(&store, &cfg.alias).unwrap();
        HostConn::start_shared(cfg, store, f.transport_clock.clone(), auth)
    }
    .unwrap();
    let ssh = Ssh::new(f.server.alias().into(), "~/unsupported".into(), conn);
    let intent = f.intent("unsupported", "true", "ssh_job");
    f.submit(&ssh, &intent).await;
    let run = f.store().get_run("unsupported").unwrap().unwrap();
    assert_eq!(run.status, "failed");
    let reason = run.result_markdown.unwrap();
    assert!(
        reason.contains(f.server.alias()) && reason.contains("FreeBSD"),
        "{reason}"
    );
}

/// Slurm lost the submission's outcome and shows no job: a cancel can't wait for the scheduler
/// to answer, so recovery has the host scancel by name and publish the cancelled receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_ends_a_slurm_submission_the_scheduler_never_shows() {
    let f = Fixture::new().await;
    f.shim(
        "sbatch",
        "cat >/dev/null; echo 'Socket timed out' >&2; exit 1",
    );
    f.shim("squeue", "exit 0");
    f.shim("sacct", "exit 0");
    f.shim("scancel", "printf '%s' \"$*\" > \"$HOME/scancel\"");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    let intent = f.intent("unseen", "touch ran", "slurm_job");
    f.submit(&slurm, &intent).await;
    assert_eq!(
        f.store().get_run("unseen").unwrap().unwrap().status,
        "submitting"
    );
    assert!(f.engine.cancel(&slurm, "unseen", "user").await.unwrap());
    f.engine.recover(&slurm, "unseen").await.unwrap();
    assert_eq!(f.terminal(&slurm, "unseen").await.status, "cancelled");
    assert_eq!(
        fs::read_to_string(f.server.home().join("scancel")).unwrap(),
        "--name=unseen"
    );
    assert!(!f.dir("unseen").join("started").exists());
}

/// Before the wrapper starts, the queued job is scancelled before any receipt claims the
/// stop; a failed scancel leaves the cancel undelivered, to be sent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_scancel_publishes_no_cancelled_receipt() {
    let f = Fixture::new().await;
    f.shim("sbatch", "cat >/dev/null; printf '456\\n'");
    f.shim("squeue", "printf '456|queued|PENDING\\n'");
    f.shim("sacct", "exit 1");
    f.shim("scancel", "echo 'Socket timed out' >&2; exit 1");
    let slurm = Slurm::new(f.ssh.clone(), f.clock.clone());
    f.submit(&slurm, &f.intent("queued", "true", "slurm_job"))
        .await;
    let run = watched(&f, "queued");
    assert!(matches!(
        slurm.cancel(&run).await,
        CancelOutcome::Undelivered { detail } if detail.contains("Socket timed out")
    ));
    assert!(!f.dir("queued").join("exit.json").exists());
    f.shim("scancel", "exit 0");
    assert!(matches!(slurm.cancel(&run).await, CancelOutcome::Pending));
    let receipt = orx_host::wrapper::receipt(&f.dir("queued"))
        .unwrap()
        .unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
}

/// A wrapper killed after it accepted a cancel, before its receipt, leaves the payload running:
/// the next contact sends the stop again and the run ends cancelled once nothing is left.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_outlives_a_wrapper_killed_before_its_receipt() {
    cancel_after_the_wrapper_is_killed(Fixture::new().await, cfg!(target_os = "macos")).await;
}

/// As on a macOS host whose payload is one of Apple's binaries, which show other processes an
/// empty environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancel_never_claims_a_stop_a_host_cannot_see() {
    cancel_after_the_wrapper_is_killed(
        Fixture::with_host_env("export ORX_TEST_HIDDEN_TAG=1").await,
        true,
    )
    .await;
}

/// Where the host can't see run tags, the run waits unreconciled instead of claiming a stop;
/// elsewhere the cancel ends it and its payload.
async fn cancel_after_the_wrapper_is_killed(f: Fixture, hidden: bool) {
    let intent = f.intent(
        "wrapper-killed",
        "printf $$ > payload; exec sleep 600",
        "ssh_job",
    );
    f.submit(f.ssh.as_ref(), &intent).await;
    let dir = f.dir("wrapper-killed");
    exists(&dir.join("payload")).await;
    let identity: orx_host_proto::ProcessIdentity =
        orx_host::read_json(&dir.join("wrapper.json")).unwrap();
    let wrapper = orx_host::platform::process(identity.pid).unwrap();
    orx_host::platform::signal(&wrapper, libc::SIGSTOP).unwrap();
    assert!(f
        .engine
        .cancel(f.ssh.as_ref(), "wrapper-killed", "user")
        .await
        .unwrap());
    assert!(dir.join("cancel.req").exists());
    orx_host::platform::signal(&wrapper, libc::SIGKILL).unwrap();
    let payload: u32 = fs::read_to_string(dir.join("payload"))
        .unwrap()
        .parse()
        .unwrap();
    if hidden {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                f.poll(
                    f.ssh.as_ref(),
                    &["wrapper-killed"],
                    Duration::from_millis(50),
                )
                .await;
                let state = f
                    .store()
                    .engine_run_state("wrapper-killed")
                    .unwrap()
                    .unwrap();
                if let crate::store::Contact::Unreconciled { detail, .. } = state.contact {
                    assert!(detail.contains("can't see"), "{detail}");
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(
            !Phase::parse(&f.store().get_run("wrapper-killed").unwrap().unwrap().status)
                .unwrap()
                .is_terminal()
        );
        unsafe { libc::kill(payload as i32, libc::SIGKILL) };
    } else {
        assert_eq!(
            f.terminal(f.ssh.as_ref(), "wrapper-killed").await.status,
            "cancelled"
        );
        // A bash whose environment the wrapper can read may still exec one of Apple's binaries.
        #[cfg(target_os = "linux")]
        assert!(
            orx_host::platform::process(payload).map_or(true, |process| !process.live()),
            "payload survived"
        );
        #[cfg(not(target_os = "linux"))]
        unsafe {
            libc::kill(payload as i32, libc::SIGKILL)
        };
    }
    assert!(!dir.join("exit.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_source_the_host_cannot_unpack_fails_the_run_with_why() {
    let f = Fixture::new().await;
    f.shim(
        "tar",
        "echo 'tar: Cannot open: No space left on device' >&2; exit 2",
    );
    let archive = f.server.directory().join("source.tar");
    fs::write(&archive, "archive").unwrap();
    let digest = orx_host::sha(b"archive");
    let mut intent = f.intent("unpack", "touch ran", "ssh_job");
    intent.run.backend_json =
        json!({"kind":"ssh_job", "sourceDigest":digest, "sourcePath":archive, "sourceSize":7})
            .to_string();
    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).unwrap();
    let mut row = submitting(&intent);
    row.metadata = &metadata;
    f.engine.submit(f.ssh.as_ref(), row).await.unwrap();
    let run = f.terminal(f.ssh.as_ref(), "unpack").await;
    assert_eq!(run.status, "failed");
    let reason = run.result_markdown.unwrap_or_default();
    assert!(
        reason.contains("The run couldn't start") && reason.contains("No space left on device"),
        "{reason}"
    );
    assert!(!f.dir("unpack").join("ran").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staging_stops_at_an_accepted_cancel() {
    let f = Fixture::new().await;
    let archive = f.server.directory().join("cancelled.tar");
    fs::write(&archive, vec![b'x'; 1 << 20]).unwrap();
    let digest = orx_host::sha(&fs::read(&archive).unwrap());
    assert!(matches!(
        f.ssh.stage_blob(&digest, &archive, 1 << 20, &|| true).await,
        Err(LaunchError::Interrupted { detail }) if detail.contains("cancel")
    ));
    let sources = f.server.home().join("engine-root/sources");
    for staged in [digest.clone(), format!("{digest}.part")] {
        assert!(!sources.join(staged).exists());
    }
}
