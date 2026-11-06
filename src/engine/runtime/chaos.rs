//! Seeded chaos over real SSH: the production `Runtime`, transport and `orx-host` run through
//! network, sshd, engine-kill and clock faults while payloads crash, daemonize and flood logs.
//! Every second asserts that phases only advance, a run starts once and sends one terminal
//! notice, an auth failure brings no further login, and kept logs match the remote bytes.
//!
//! Opt-in; the ordinary suite compiles it and leaves it ignored:
//!
//! ```sh
//! env -u CLAUDE_CONFIG_DIR -u CODEX_HOME cargo test --locked --bin orx -- --ignored chaos --nocapture
//! ```
//!
//! `CHAOS_DURATION` (seconds of faults) and `CHAOS_RUNS` scale it. `CHAOS_SEED` takes a `u64`,
//! or the `seed:seconds:runs` value a run prints, which overrides both and replays the launch
//! scripts, fault order and clock jumps; OS scheduling stays real.

use super::*;
use crate::commands::up::api_v2::KeptRange;
use crate::engine::transport::Status;
use crate::jobs::BackendDescriptor;
use crate::local::model::LocalExperiment;
use orx_test_support::{Faults, Process, Sshd, SshdBuilder};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::num::NonZeroU64;
use std::time::Instant;

pub(crate) mod rng;
use rng::Rng;

const RECONCILE_BUDGET: Duration = Duration::from_secs(60);

// A process-local clock reads atomically replaced offsets; wall and monotonic move independently.
struct ChaosClock {
    origin: Instant,
    path: PathBuf,
}
impl Clock for ChaosClock {
    fn monotonic(&self) -> Duration {
        self.origin.elapsed() + Duration::from_millis(self.offsets().0)
    }
    fn wall_ms(&self) -> i64 {
        crate::store::now_ms() + self.offsets().1
    }
}
impl ChaosClock {
    fn offsets(&self) -> (u64, i64) {
        fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

fn set_clock(directory: &Path, offsets: (u64, i64)) {
    let pending = directory.join("clock.tmp");
    fs::write(&pending, serde_json::to_vec(&offsets).unwrap()).unwrap();
    fs::rename(pending, directory.join("clock.json")).unwrap();
}

pub(crate) fn crash_point(name: &str) {
    if std::env::var("ORX_TEST_CRASH_AT").as_deref() == Ok(name) {
        let directory =
            PathBuf::from(std::env::var_os("ORX_CHAOS_WORKER").expect("crash worker directory"));
        fs::write(directory.join("crashed"), name).unwrap();
        unsafe {
            libc::kill(libc::getpid(), libc::SIGKILL);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "chaos subprocess entry point"]
async fn chaos_engine_worker() {
    let Some(directory) = std::env::var_os("ORX_CHAOS_WORKER") else {
        return;
    };
    let directory = PathBuf::from(directory);
    let parent: libc::pid_t = std::env::var("ORX_CHAOS_PARENT").unwrap().parse().unwrap();
    let _payloads = Payloads(directory.join("home/chaos-root/runs"));
    let clock = Arc::new(ChaosClock {
        origin: Instant::now(),
        path: directory.join("clock.json"),
    });
    let data = directory.join("store");
    let engine = Arc::new(Engine::new(
        Store::open_at(data.clone()).unwrap(),
        clock.clone(),
    ));
    let runtime = Runtime::new(
        engine.clone(),
        clock,
        data,
        Some(directory.join("ssh_config")),
    );
    let live = directory.join("host-live");
    let _ = fs::remove_file(&live);
    fs::write(directory.join("worker-ready"), b"ready").unwrap();
    if let Ok(point) = std::env::var("ORX_TEST_CRASH_AT") {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            let backend = runtime
                .backend_for(&Key::Ssh {
                    host: "orx-test".into(),
                    root: "~/chaos-root".into(),
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            while !lock(&runtime.conns)
                .values()
                .filter_map(|cell| cell.get())
                .any(|conn| conn.state().borrow().status == Status::Live)
            {
                assert!(Instant::now() < deadline, "crash hook host not live");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let id = format!("hook-{point}");
            let script = "printf 'start\\n' >> payload-starts; sleep 1";
            engine
                .submit(
                    backend.as_ref(),
                    submission(
                        &id,
                        script,
                        &orx_host::sha(script.as_bytes()),
                        "exp",
                        &BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap(),
                        &LaunchSpec {
                            kind: "ssh_job".into(),
                            target: Some("orx-test".into()),
                            options: json!({"root":"~/chaos-root"}),
                        },
                    ),
                )
                .await
                .unwrap();
            panic!("crash hook did not fire: {point}");
        });
    } else {
        runtime.clone().start();
    }
    while unsafe { libc::getppid() } == parent {
        let connected = lock(&runtime.conns).iter().any(|((alias, _), conn)| {
            alias == "orx-test"
                && conn
                    .get()
                    .is_some_and(|conn| conn.state().borrow().status == Status::Live)
        });
        if connected {
            if !live.exists() {
                fs::write(&live, b"live").unwrap();
            }
        } else {
            let _ = fs::remove_file(&live);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn worker(server: &Sshd, hook: Option<&str>) -> Process {
    let ready = server.directory().join("worker-ready");
    let _ = fs::remove_file(&ready);
    let mut command = crate::process::command(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "engine::runtime::chaos::chaos_engine_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("ORX_CHAOS_WORKER", server.directory())
        .env("ORX_CHAOS_PARENT", std::process::id().to_string())
        .env("HOME", server.home())
        .env("XDG_CONFIG_HOME", server.home().join("config"))
        .env("ORX_DATA_DIR", server.directory().join("store"))
        .env("ORX_HOST_BIN_DIR", orx_test_support::host_binaries());
    if let Some(point) = hook {
        command.env("ORX_TEST_CRASH_AT", point);
    } else {
        command.env_remove("ORX_TEST_CRASH_AT");
    }
    let mut process = Process::spawn(&mut command).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !ready.exists() {
        if let Ok(output) = process.wait(Duration::from_millis(20)) {
            panic!("worker exited: {output:?}");
        }
        assert!(Instant::now() < deadline, "engine worker did not start");
    }
    process
}

fn kill_worker(process: &mut Process) {
    assert_eq!(
        unsafe { libc::kill(process.pid() as i32, libc::SIGKILL) },
        0
    );
    assert!(!process
        .wait(Duration::from_secs(5))
        .unwrap()
        .status
        .success());
}

fn submission<'a>(
    id: &'a str,
    script: &'a str,
    digest: &'a str,
    experiment: &'a str,
    metadata: &'a BackendDescriptor,
    launch: &'a LaunchSpec,
) -> SubmittingRun<'a> {
    SubmittingRun {
        id,
        experiment_id: experiment,
        project_id: "proj",
        metadata,
        command: script,
        created_at: crate::store::now_ms(),
        commit_sha: None,
        chat_session_id: Some("chaos"),
        spec_digest: digest,
        launch,
    }
}

fn insert(store: &Store, alias: &str, experiment: &str, id: &str, script: &str) {
    let metadata = BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "ssh_job".into(),
        target: Some(alias.into()),
        options: json!({"root":"~/chaos-root"}),
    };
    store
        .insert_submitting_run(submission(
            id,
            script,
            &orx_host::sha(script.as_bytes()),
            experiment,
            &metadata,
            &launch,
        ))
        .unwrap();
}

struct Payloads(PathBuf);
impl Drop for Payloads {
    fn drop(&mut self) {
        let Ok(runs) = fs::read_dir(&self.0) else {
            return;
        };
        for run in runs.flatten() {
            let directory = run.path();
            let identity = orx_host::read_json::<orx_host_proto::ProcessIdentity>(
                &directory.join("wrapper.json"),
            )
            .ok();
            if let Ok(tag) = fs::read_to_string(directory.join("started")) {
                if let Ok(scan) = orx_host::platform::scan(identity.as_ref(), tag.trim(), false) {
                    for child in scan.processes {
                        let _ = orx_host::platform::signal(&child, libc::SIGKILL);
                    }
                }
            }
            if let Some(identity) = identity {
                if let Ok(wrapper) = orx_host::platform::process(identity.pid) {
                    if wrapper.starttime == identity.starttime {
                        let _ = orx_host::platform::signal(&wrapper, libc::SIGKILL);
                    }
                }
            }
        }
    }
}

fn assert_step(
    server: &Sshd,
    worker: &Process,
    starts: &mut BTreeMap<String, u64>,
    reachable_since: Option<Instant>,
) -> usize {
    use std::os::unix::fs::MetadataExt;
    let db = rusqlite::Connection::open(server.directory().join("store/orx.db")).unwrap();
    let mut query = db.prepare("SELECT id, status, (SELECT count(*) FROM run_notices n WHERE n.run_id=runs.id AND n.kind='terminal' AND n.chat_session_id='chaos') FROM runs WHERE experiment_id='exp'").unwrap();
    let runs = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    let mut statement = db
        .prepare("SELECT run,phase FROM chaos_phases ORDER BY rowid")
        .unwrap();
    let mut phases = BTreeMap::<String, Phase>::new();
    for row in statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
    {
        let (id, phase) = row.unwrap();
        let phase = Phase::parse(&phase).unwrap();
        if let Some(before) = phases.insert(id.clone(), phase) {
            assert!(
                phase.rank() >= before.rank(),
                "phase history regressed: {id}"
            );
            if before.is_terminal() {
                assert_eq!(before, phase);
            }
        }
    }
    let mut terminal = 0;
    for (id, status, notices) in runs {
        let phase = Phase::parse(&status).unwrap();
        assert_eq!(
            notices,
            usize::from(phase.is_terminal()),
            "terminal notices: {}",
            id
        );
        let remote = server.home().join("chaos-root/runs").join(&id);
        if let Ok(metadata) = fs::metadata(remote.join("started")) {
            if let Some(inode) = starts.insert(id.clone(), metadata.ino()) {
                assert_eq!(inode, metadata.ino(), "started replaced: {}", id);
            }
        }
        let settled = |path: PathBuf| {
            fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(35))
        };
        if reachable_since.is_some_and(|since| since.elapsed() > Duration::from_secs(35))
            && settled(server.directory().join("host-live"))
            && settled(remote.join("exit.json"))
        {
            assert!(
                phase.is_terminal(),
                "host returned but receipt stayed nonterminal: {id}"
            );
        }
        if phase == Phase::Lost {
            assert_eq!(id, "boot-swap", "unexpected Lost run without receipt: {id}");
            assert!(
                matches!(orx_host::wrapper::state(&remote).unwrap(), orx_host_proto::RunState::Lost {reason} if reason == "host rebooted")
            );
        } else if phase.is_terminal() {
            let receipt = orx_host::wrapper::receipt(&remote)
                .unwrap_or_else(|error| panic!("receipt unreadable for {id} ({phase:?}): {error}"))
                .unwrap_or_else(|| panic!("terminal {id} ({phase:?}) has no receipt"));
            let expected = if receipt.code == Some(0) && receipt.signal.is_none() {
                Phase::Done
            } else {
                Phase::Failed
            };
            assert_eq!(phase, expected, "receipt disagrees with phase: {id}");
        }
        let launch_count = fs::read_to_string(remote.join("payload-starts"))
            .unwrap_or_default()
            .lines()
            .count();
        assert!(launch_count <= 1, "duplicate payload: {}", id);
        let manifest = server
            .directory()
            .join("store/run-logs")
            .join(format!("{}.ranges.json", id));
        if let Ok(bytes) = fs::read(manifest) {
            let kept: Vec<KeptRange> = serde_json::from_slice(&bytes).unwrap();
            for range in kept {
                let local = fs::read(range.file).unwrap();
                let mut file = fs::File::open(remote.join("log")).unwrap();
                file.seek(SeekFrom::Start(range.offset)).unwrap();
                let mut remote_bytes = vec![0; range.len as usize];
                file.read_exact(&mut remote_bytes).unwrap();
                assert_eq!(local.len() as u64, range.len);
                assert_eq!(
                    local, remote_bytes,
                    "log mismatch: {} @{}",
                    id, range.offset
                );
            }
        } else if phase.is_terminal() && phase != Phase::Lost {
            // A finished run may wait for its log, but then owes it.
            let owed: bool = db
                .query_row(
                    "SELECT done = 0 FROM log_collections WHERE run_id = ?1",
                    [&id],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            assert!(
                owed,
                "terminal without kept log: {id}\n{}",
                worker.stderr().unwrap()
            );
        }
        terminal += usize::from(phase.is_terminal());
    }
    let output = crate::process::command("ps")
        .args(["-axo", "comm=,args="])
        .output()
        .unwrap();
    let config = server.config_path();
    let count = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            let comm = line.split_whitespace().next().unwrap_or_default();
            comm.ends_with("ssh")
                && line.contains(config.to_str().unwrap())
                && !line.contains(" -G ")
        })
        .count();
    assert!(count <= 2, "{count} SSH processes for one host");
    terminal
}

fn receipt_overdue(reconnected: Duration, receipt_age: Option<Duration>) -> bool {
    receipt_age.is_some_and(|age| reconnected.min(age) > RECONCILE_BUDGET)
}

#[test]
fn final_deadline_waits_for_the_natural_exit_receipt() {
    for (reconnected, receipt, expected) in [
        (120, None, false),
        (120, Some(1), false),
        (1, Some(120), false),
        (120, Some(60), false),
        (120, Some(61), true),
        (61, Some(120), true),
    ] {
        assert_eq!(
            receipt_overdue(
                Duration::from_secs(reconnected),
                receipt.map(Duration::from_secs)
            ),
            expected,
            "reconnect age={reconnected}, receipt age={receipt:?}"
        );
    }
}

#[test]
fn held_outage_does_not_make_an_old_live_marker_prove_receipt_reconciliation() {
    let server = Sshd::start().unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    insert(&store, server.alias(), "exp", "outage", "true");
    let db = rusqlite::Connection::open(server.directory().join("store/orx.db")).unwrap();
    db.execute_batch("CREATE TABLE chaos_phases(run TEXT, phase TEXT);")
        .unwrap();
    let backend = json!({
        "kind": "engine",
        "handle": Handle::Ssh {
            host: server.alias().into(),
            root: "~/chaos-root".into(),
        },
    });
    db.execute(
        "UPDATE runs SET status = 'running', backend_json = ?1",
        [backend.to_string()],
    )
    .unwrap();
    let remote = server.home().join("chaos-root/runs/outage");
    fs::create_dir_all(&remote).unwrap();
    let receipt = orx_host_proto::ExitReceipt {
        finished_at: None,
        code: Some(0),
        signal: None,
        reason: orx_host_proto::ExitReason::Exited,
        processes: vec![],
        outcome: orx_host_proto::CancelOutcome::Verified,
    };
    orx_host::publish(&remote.join("exit.json"), &receipt).unwrap();
    let live = server.directory().join("host-live");
    fs::write(&live, "live").unwrap();
    let age = Duration::from_secs(60);
    let old = std::time::SystemTime::now() - age;
    for path in [live, remote.join("exit.json")] {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    let worker = Process::spawn(&mut crate::process::command("true")).unwrap();
    let mut starts = BTreeMap::new();
    assert_eq!(assert_step(&server, &worker, &mut starts, None), 0);
    let healthy = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_step(&server, &worker, &mut starts, Some(Instant::now() - age))
    }));
    assert!(healthy.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real SSH seeded chaos and soak"]
async fn chaos_seeded_soak() {
    let parent = unsafe { libc::getppid() };
    tokio::select! {
        _ = schedule() => {},
        _ = tokio::signal::ctrl_c() => panic!("chaos interrupted; owned processes cleaned up"),
        _ = async {
            while unsafe { libc::getppid() } == parent {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } => panic!("cargo exited; owned processes cleaned up"),
    }
}

async fn schedule() {
    let replay = std::env::var("CHAOS_SEED").unwrap_or_else(|_| "8030613221891662191".into());
    let parts: Vec<_> = replay.split(':').collect();
    let seed = parts[0].parse().expect("CHAOS_SEED must start with a u64");
    let (seconds, runs): (u64, usize) = match parts.as_slice() {
        [_] => (
            std::env::var("CHAOS_DURATION")
                .map(|s| s.parse().expect("CHAOS_DURATION must be seconds"))
                .unwrap_or(120),
            std::env::var("CHAOS_RUNS")
                .map(|s| s.parse().expect("CHAOS_RUNS must be a count"))
                .unwrap_or(32),
        ),
        [_, seconds, runs] => (
            seconds.parse().expect("seed duration"),
            runs.parse().expect("seed run count"),
        ),
        _ => panic!("CHAOS_SEED must be u64 or u64:seconds:runs"),
    };
    assert!(seconds >= 30 && runs > 0);
    eprintln!("CHAOS_SEED={seed}:{seconds}:{runs}");
    let mut rng = Rng(seed);
    let mut server = Sshd::start().unwrap();
    let _payloads = Payloads(server.home().join("chaos-root/runs"));
    let store = Store::open_at(server.directory().join("store")).unwrap();
    store
        .create_local_experiment(&LocalExperiment {
            id: "exp".into(),
            project_id: "proj".into(),
            parent_experiment_id: None,
            slug: "exp".into(),
            branch_name: "chaos".into(),
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
    // Record every phase in SQLite too, so sub-second transitions and killed workers cannot hide regressions.
    let db = rusqlite::Connection::open(server.directory().join("store/orx.db")).unwrap();
    db.execute_batch("CREATE TABLE chaos_phases(run TEXT, phase TEXT); CREATE TRIGGER chaos_phase AFTER UPDATE OF status ON runs BEGIN INSERT INTO chaos_phases VALUES(NEW.id,NEW.status); END;").unwrap();
    let auth = SshdBuilder {
        identity_files: vec!["host_key".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    let rejected = fs::read_to_string(auth.config_path())
        .unwrap()
        .replace("Host orx-test", "Host chaos-rejected");
    let mut config = fs::read_to_string(server.config_path()).unwrap();
    config.push_str(&rejected);
    fs::write(server.config_path(), config).unwrap();
    let mut experiment = store.get_local_experiment("exp").unwrap().unwrap();
    experiment.id = "auth-exp".into();
    experiment.slug = "auth-exp".into();
    store.create_local_experiment(&experiment).unwrap();
    insert(
        &store,
        "chaos-rejected",
        "auth-exp",
        "auth-rejected",
        "true",
    );
    let mut process = worker(&server, None);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !store
        .engine_host("chaos-rejected", "~/chaos-root")
        .unwrap()
        .is_some_and(|host| host.status == Status::NeedsAuth)
    {
        assert!(
            Instant::now() < deadline,
            "Runtime did not open auth breaker: {}",
            process.stderr().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let auth_attempts = auth.auth_attempts().unwrap();
    assert_eq!(auth_attempts.values().sum::<usize>(), 1);
    let mut offsets = (
        1_800_000u64,
        if rng.next() & 1 == 0 {
            3_600_000i64
        } else {
            -3_600_000i64
        },
    );
    set_clock(server.directory(), offsets);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        auth.auth_attempts().unwrap(),
        auth_attempts,
        "Runtime retried auth after clock jump"
    );
    kill_worker(&mut process);
    for point in [
        "engine_intent",
        "engine_fence",
        "engine_response",
        "engine_handle",
    ] {
        let _ = fs::remove_file(server.directory().join("crashed"));
        let mut crashing = worker(&server, Some(point));
        let output = crashing.wait(Duration::from_secs(30)).unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            output.status.signal(),
            Some(libc::SIGKILL),
            "crash {point}: {output:?}"
        );
        assert_eq!(
            fs::read_to_string(server.directory().join("crashed")).unwrap(),
            point
        );
    }
    process = worker(&server, None);
    insert(
        &store,
        server.alias(),
        "exp",
        "boot-swap",
        "printf 'start\\n' >> payload-starts; sleep 20",
    );
    let boot = server.home().join("chaos-root/runs/boot-swap");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !boot.join("payload-starts").exists() {
        assert!(
            Instant::now() < deadline,
            "boot probe did not start: {}",
            process.stderr().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut identity: orx_host_proto::ProcessIdentity =
        orx_host::read_json(&boot.join("wrapper.json")).unwrap();
    let wrapper = orx_host::platform::process(identity.pid).unwrap();
    orx_host::platform::signal(&wrapper, libc::SIGKILL).unwrap();
    let tag = fs::read_to_string(boot.join("started")).unwrap();
    for child in orx_host::platform::scan(Some(&identity), tag.trim(), false)
        .unwrap()
        .processes
    {
        let _ = orx_host::platform::signal(&child, libc::SIGKILL);
    }
    identity.boot_id.push_str("-chaos-reboot");
    orx_host::publish(&boot.join("wrapper.json"), &identity).unwrap();
    let mut starts = BTreeMap::new();
    let started = Instant::now();
    let mut reachable_since = Some(started);
    let mut faults: Vec<_> = (0..9).collect();
    for launched in 0..runs {
        let id = format!("chaos-{seed:016x}-{launched:04}");
        let choice = rng.next() % 7;
        let delay = 1 + rng.next() % (seconds - 20);
        let payload = match choice {
            0 => format!("sleep {delay}; exit {}",rng.next()%4),
            1 => format!("sleep {delay}; kill -KILL $$"),
            2 | 3 => format!("python3 -c 'import os,time; p=os.fork(); os._exit(0) if p else None; os.setsid(); p=os.fork(); os._exit(0) if p else None; time.sleep({delay}+4)'\nsleep {delay}"),
            4 => format!("python3 - <<'PYCPU'\nimport os,time\nend=time.monotonic()+{delay}\nchildren=[]\nfor _ in range(os.cpu_count() or 1):\n    pid=os.fork()\n    if pid==0:\n        while time.monotonic()<end: pass\n        os._exit(0)\n    children.append(pid)\nfor pid in children: os.waitpid(pid,0)\nPYCPU"),
            _ => format!("head -c {} /dev/zero | tr '\\000' x; printf '\\n'; sleep {delay}",2*1024*1024+rng.next()%(6*1024*1024)),
        };
        let script =
            format!("printf 'start\\n' >> payload-starts\nprintf 'run {id}\\n'\n{payload}\n");
        insert(&store, server.alias(), "exp", &id, &script);
    }
    let mut elapsed = 0;
    let mut step = 0;
    while elapsed < seconds {
        if step % faults.len() == 0 {
            for index in (1..faults.len()).rev() {
                let swap = rng.next() as usize % (index + 1);
                faults.swap(index, swap);
            }
        }
        let fault = faults[step % faults.len()];
        let hold = 1 + rng.next() % 60;
        eprintln!("seed={seed} step={step} fault={fault} hold={hold}s runs={runs}");
        match fault {
            0 => server
                .proxy()
                .set(Faults {
                    black_hole: true,
                    ..Faults::default()
                })
                .unwrap(),
            1 => server
                .proxy()
                .set(Faults {
                    paused: true,
                    ..Faults::default()
                })
                .unwrap(),
            2 => server
                .proxy()
                .set(Faults {
                    latency: Duration::from_millis(50 + rng.next() % 150),
                    ..Faults::default()
                })
                .unwrap(),
            3 => server
                .proxy()
                .set(Faults {
                    bytes_per_second: NonZeroU64::new(4096 + rng.next() % 32768),
                    ..Faults::default()
                })
                .unwrap(),
            4 => server.proxy().reset().unwrap(),
            5 => {
                server
                    .proxy()
                    .set(Faults {
                        refuse_new: true,
                        ..Faults::default()
                    })
                    .unwrap();
                server.proxy().reset().unwrap();
            }
            6 => server.restart().unwrap(),
            7 => {
                kill_worker(&mut process);
                process = worker(&server, None);
            }
            8 => {
                offsets.0 += 1_800_000;
                offsets.1 += if rng.next() & 1 == 0 {
                    3_600_000
                } else {
                    -3_600_000
                };
                set_clock(server.directory(), offsets);
            }
            _ => unreachable!(),
        }
        if matches!(fault, 0 | 1 | 5) {
            reachable_since = None;
        } else if matches!(fault, 4 | 6 | 7 | 8) {
            reachable_since = Some(Instant::now());
        }
        let hold_started = Instant::now();
        for second in 1..=hold {
            tokio::time::sleep_until((hold_started + Duration::from_secs(second)).into()).await;
            assert_step(&server, &process, &mut starts, reachable_since);
            if matches!(fault, 0 | 1 | 5) && second >= 37 {
                assert_ne!(
                    store
                        .engine_host(server.alias(), "~/chaos-root")
                        .unwrap()
                        .unwrap()
                        .status,
                    Status::Live,
                    "host stayed Live during a held outage"
                );
            }
            assert_eq!(
                auth.auth_attempts().unwrap(),
                auth_attempts,
                "Runtime login after auth failure, seed={seed} step={step}"
            );
            assert_eq!(
                store
                    .engine_host("chaos-rejected", "~/chaos-root")
                    .unwrap()
                    .unwrap()
                    .status,
                Status::NeedsAuth
            );
        }
        server.proxy().clear().unwrap();
        if fault == 0 {
            server.proxy().reset().unwrap();
        }
        reachable_since.get_or_insert_with(Instant::now);
        assert_step(&server, &process, &mut starts, reachable_since);
        elapsed += hold;
        step += 1;
    }
    server.proxy().clear().unwrap();
    kill_worker(&mut process);
    process = worker(&server, None);
    let reconnected = Instant::now();
    // Payload delays reserve 20 seconds for setup and output.
    let deadline = reconnected + Duration::from_secs(seconds) + RECONCILE_BUDGET;
    while assert_step(&server, &process, &mut starts, None) != runs + 5 {
        for run in store.list_runs_by_experiment("exp").unwrap() {
            if Phase::parse(&run.status).unwrap().is_terminal() {
                continue;
            }
            let receipt_age = fs::metadata(
                server
                    .home()
                    .join("chaos-root/runs")
                    .join(&run.id)
                    .join("exit.json"),
            )
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|published| published.elapsed().ok());
            assert!(
                !receipt_overdue(reconnected.elapsed(), receipt_age),
                "receipt stayed nonterminal after reconnect, seed={seed}, run={}, reconnect_age={:?}, receipt_age={receipt_age:?}",
                run.id,
                reconnected.elapsed(),
            );
        }
        assert!(
            Instant::now() < deadline,
            "payload lifetime plus reconciliation grace exceeded, seed={seed}: {:?}",
            store.list_runs_by_experiment("exp").unwrap()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // A restarted engine retries every log still owed, and a clean host keeps them.
    let owed = || -> i64 {
        let db = rusqlite::Connection::open(server.directory().join("store/orx.db")).unwrap();
        db.query_row(
            "SELECT count(*) FROM log_collections WHERE done = 0",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    while owed() != 0 {
        assert!(
            Instant::now() < deadline,
            "logs still owed after the host returned, seed={seed}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert_step(&server, &process, &mut starts, None);
    for run in store.list_runs_by_experiment("exp").unwrap() {
        let id = &run.id;
        let remote = server.home().join("chaos-root/runs").join(id);
        assert!(remote.join("started").exists(), "never started: {id}");
        assert_eq!(
            fs::read_to_string(remote.join("payload-starts"))
                .unwrap()
                .lines()
                .count(),
            1,
            "{id}"
        );
    }
    kill_worker(&mut process);
    eprintln!(
        "CHAOS PASS seed={seed}:{seconds}:{runs} steps={step} elapsed={:?}",
        started.elapsed()
    );
}
