use super::*;
use orx_test_support::{Faults, Sshd, SshdBuilder};
use std::fs;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Instant;

struct TestClock {
    start: Instant,
    wall: AtomicI64,
    mono: AtomicU64,
}

impl TestClock {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            wall: AtomicI64::new(0),
            mono: AtomicU64::new(0),
        })
    }

    fn advance(&self, millis: u64) {
        self.wall.fetch_add(millis as i64, Ordering::SeqCst);
        self.mono.fetch_add(millis, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn wall_ms(&self) -> i64 {
        1_700_000_000_000
            + self.start.elapsed().as_millis() as i64
            + self.wall.load(Ordering::SeqCst)
    }
    fn monotonic(&self) -> Duration {
        self.start.elapsed() + Duration::from_millis(self.mono.load(Ordering::SeqCst))
    }
}

use orx_test_support::host_binaries as binaries;

/// Runs the calling test alone in a child process where paced waits, the transport deadline
/// among them, are ten times shorter. True in that child.
fn paced_child() -> bool {
    if !crate::config::in_test_child(&[]) {
        return false;
    }
    std::env::set_var("ORX_TEST_SPEEDUP", "10");
    true
}

fn config(server: &Sshd) -> Config {
    Config {
        alias: server.alias().into(),
        root: "~/engine-root".into(),
        ssh_config: Some(server.config_path()),
        control_dir: server.directory().join("cm"),
        binary_dir: Some(binaries()),
        #[cfg(unix)]
        sharing_home: None,
        askpass_executable: std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("orx"),
    }
}

fn start(server: &Sshd, clock: Arc<TestClock>) -> HostConn {
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let config = config(server);
    let auth = Auth::new(&store, &config.alias).unwrap();
    HostConn::start_shared(config, store, clock, auth).unwrap()
}

fn alias_auth(server: &Sshd) -> AuthState {
    Store::open_at(server.directory().join("store"))
        .unwrap()
        .engine_host_auth(server.alias())
        .unwrap()
}

async fn status(conn: &HostConn, wanted: Status) -> HostState {
    let mut state = conn.state();
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let snapshot = state.borrow_and_update().clone();
            if snapshot.status == wanted {
                return snapshot;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("waiting for {wanted:?}: {:?}", *conn.state.borrow()))
}

fn auth_attempts(server: &Sshd) -> BTreeMap<(String, String), usize> {
    server.auth_attempts().unwrap()
}

async fn logged_attempts(server: &Sshd, count: usize) -> BTreeMap<(String, String), usize> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let attempts = auth_attempts(server);
            if attempts.values().sum::<usize>() >= count {
                return attempts;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Missing auth events: {:?}", auth_attempts(server)))
}

fn ssh_pids(server: &Sshd) -> Vec<u32> {
    let output = crate::process::command("ps")
        .args(["-axo", "pid=,ppid=,comm=,args="])
        .output()
        .unwrap();
    let config = server.config_path();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent: u32 = fields.next()?.parse().ok()?;
            let comm = fields.next()?;
            (parent == std::process::id()
                && comm.ends_with("ssh")
                && line.contains(config.to_str().unwrap())
                && !line.contains(" -G "))
            .then_some(pid)
        })
        .collect()
}

async fn exec(conn: &HostConn, command: String) -> orx_host_proto::ExecResult {
    let Response::Exec { result } = conn
        .request(Request::Exec {
            command,
            stdin: Vec::new(),
            timeout_ms: 10_000,
        })
        .await
        .unwrap()
    else {
        panic!("Exec response");
    };
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_bootstrap_reuses_binary_and_fifty_requests_share_one_session() {
    let server = Sshd::start().unwrap();
    let conn = start(&server, TestClock::new());
    assert_eq!(
        {
            status(&conn, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
    assert!(matches!(
        conn.request(Request::Hello {
            proto: orx_host_proto::PROTOCOL_VERSION
        })
        .await
        .unwrap(),
        Response::Hello { .. }
    ));
    let installed: Vec<_> = fs::read_dir(server.home().join("engine-root/bin"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(installed.len(), 1);
    use std::os::unix::fs::MetadataExt;
    let before = fs::metadata(&installed[0]).unwrap();
    let original = ssh_pids(&server);
    assert_eq!(original.len(), 1);
    let accepted = ("publickey".into(), "Accepted".into());
    assert_eq!(auth_attempts(&server)[&accepted], 1);
    let requests = (0..50).map(|index| {
        let conn = conn.clone();
        async move {
            assert_eq!(
                exec(&conn, format!("printf '{index}'")).await.stdout,
                index.to_string().as_bytes()
            );
        }
    });
    futures::future::join_all(requests).await;
    assert_eq!(ssh_pids(&server), original);
    conn.retry().await.unwrap();
    status(&conn, Status::Live).await;
    let after = fs::metadata(&installed[0]).unwrap();
    assert_eq!(
        (before.ino(), before.mtime(), before.mtime_nsec()),
        (after.ino(), after.mtime(), after.mtime_nsec())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn needs_auth_key_host_makes_no_startup_attempt() {
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    store
        .seed_engine_host(
            server.alias(),
            "~/engine-root",
            Some(HostClass::Key),
            &HostState {
                status: Status::NeedsAuth,
                cause: Some("Authentication failed".into()),
                ..HostState::default()
            },
        )
        .unwrap();
    let mut pinned = store.engine_host_auth(server.alias()).unwrap();
    pinned.route = Some(ssh::route(&config(&server)).await.unwrap());
    store
        .save_engine_host_auth(server.alias(), &pinned)
        .unwrap();
    for _ in 0..2 {
        let conn = start(&server, clock.clone());
        assert!(conn
            .request(Request::Hello {
                proto: orx_host_proto::PROTOCOL_VERSION,
            })
            .await
            .is_err());
        assert_eq!(conn.state().borrow().status, Status::NeedsAuth);
        assert_eq!(auth_attempts(&server).values().sum::<usize>(), 0);
        clock.advance(3_600_000);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_first_key_opens_durable_breaker_and_never_retries() {
    let server = Sshd::start().unwrap();
    let config = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("LogLevel ERROR", "LogLevel QUIET");
    fs::write(server.config_path(), config).unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    assert_eq!(
        {
            status(&conn, Status::NeedsAuth).await;
            alias_auth(&server).class
        },
        Some(HostClass::Interactive)
    );
    assert!(conn.state().borrow().last_live_ms.is_none());
    assert!(ssh_pids(&server).is_empty());
    let attempts = auth_attempts(&server);
    assert_eq!(attempts.values().sum::<usize>(), 1, "{attempts:?}");
    clock.wall.fetch_add(30 * 60 * 1000, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(ssh_pids(&server).is_empty());
    assert_eq!(auth_attempts(&server), attempts);
    drop(conn);
    let restarted = start(&server, clock);
    status(&restarted, Status::NeedsAuth).await;
    assert_eq!(auth_attempts(&server), attempts);
    let key = fs::read(server.directory().join("client_key.pub")).unwrap();
    fs::write(server.directory().join("authorized_keys"), key).unwrap();
    restarted.retry().await.unwrap();
    assert_eq!(
        {
            status(&restarted, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_dead_master_cannot_login_and_connect_relays_askpass() {
    let passphrase = "orx transport 'passphrase'";
    let server = SshdBuilder {
        extra_client_keys: vec![Some(passphrase.into()), None],
        identity_files: vec!["client_key_1".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    interactive_lifecycle(server, 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interactive_mfa_master_recovers_without_background_logins() {
    let server = SshdBuilder {
        authentication_methods: "publickey,publickey".into(),
        extra_client_keys: vec![Some("orx transport 'passphrase'".into()), None],
        identity_files: vec!["client_key".into(), "client_key_1".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    interactive_lifecycle(server, 2).await;
}

async fn interactive_lifecycle(server: Sshd, factors: usize) {
    let passphrase = "orx transport 'passphrase'";
    let clock = TestClock::new();
    let mut conn = start(&server, clock.clone());
    status(&conn, Status::NeedsAuth).await;
    let prompts = Arc::new(AtomicU64::new(0));
    let fixture = server.askpass(passphrase).unwrap();
    let prompt: Prompt = Arc::new({
        let prompts = prompts.clone();
        move |question| {
            let prompts = prompts.clone();
            let fixture = fixture.clone();
            Box::pin(async move {
                prompts.fetch_add(1, Ordering::SeqCst);
                let answer = crate::process::tokio_command(fixture)
                    .arg(question)
                    .output()
                    .await?;
                Ok(Some(String::from_utf8(answer.stdout)?.trim_end().into()))
            })
        }
    });
    conn.connect_interactive(prompt.clone(), false)
        .await
        .unwrap();
    status(&conn, Status::Live).await;
    assert!(prompts.load(Ordering::SeqCst) > 0);
    let authenticated = auth_attempts(&server);
    conn.connect_interactive(prompt.clone(), false)
        .await
        .unwrap();
    assert_eq!(auth_attempts(&server), authenticated);
    drop(conn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    conn = start(&server, clock.clone());
    status(&conn, Status::Live).await;
    assert_eq!(auth_attempts(&server), authenticated);
    unsafe {
        libc::kill(ssh_pids(&server)[0] as i32, libc::SIGKILL);
    }
    status(&conn, Status::Backoff).await;
    clock.advance(6001);
    status(&conn, Status::Live).await;
    assert_eq!(alias_auth(&server).class, Some(HostClass::Interactive));
    assert_eq!(auth_attempts(&server), authenticated);
    clock.wall.fetch_add(31_000, Ordering::SeqCst);
    status(&conn, Status::Backoff).await;
    clock.advance(6001);
    status(&conn, Status::Live).await;
    assert_eq!(auth_attempts(&server), authenticated);
    let pin = alias_auth(&server).route.unwrap();
    let path = ssh::socket(&config(&server), &pin).unwrap();
    let output = server
        .spawn(&["-S", path.to_str().unwrap(), "-O", "exit"], None)
        .unwrap()
        .wait(Duration::from_secs(5))
        .unwrap();
    assert!(output.status.success());
    status(&conn, Status::Backoff).await;
    let attempts = auth_attempts(&server);
    clock.advance(6001);
    status(&conn, Status::NeedsAuth).await;
    assert!(ssh_pids(&server).is_empty());
    assert_eq!(auth_attempts(&server), attempts);
    assert!(conn
        .request(Request::Hello {
            proto: orx_host_proto::PROTOCOL_VERSION
        })
        .await
        .is_err());
    let retry_error = conn.retry().await.unwrap_err();
    let retried =
        logged_attempts(&server, attempts.values().sum::<usize>() + factors * 2 - 1).await;
    assert_eq!(
        retried.values().sum::<usize>() - attempts.values().sum::<usize>(),
        factors * 2 - 1,
        "{retry_error:#}; before={attempts:?}, after={retried:?}"
    );
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(ssh_pids(&server).is_empty());
    assert_eq!(auth_attempts(&server), retried);
    conn.connect_interactive(prompt, false).await.unwrap();
    status(&conn, Status::Live).await;
    assert!(matches!(
        conn.request(Request::Hello {
            proto: orx_host_proto::PROTOCOL_VERSION
        })
        .await
        .unwrap(),
        Response::Hello { .. }
    ));
    let original = ssh::socket(&config(&server), &alias_auth(&server).route.unwrap()).unwrap();
    let updated = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("client_key_1", "client_key_2");
    fs::write(server.config_path(), updated).unwrap();
    let prompt: Prompt =
        Arc::new(|_| Box::pin(async { bail!("Unexpected prompt for unencrypted key") }));
    conn.connect_interactive(prompt, false).await.unwrap();
    assert_ne!(
        original,
        ssh::socket(&config(&server), &alias_auth(&server).route.unwrap()).unwrap()
    );
    assert!(!original.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_master_is_ignored_and_route_edits_require_explicit_approval() {
    let server = SshdBuilder {
        extra_client_keys: vec![None],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    let user_socket = server.directory().join("user-master");
    let text = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("ControlMaster no", "ControlMaster auto")
        .replace(
            &format!("{}/control-%C", server.directory().display()),
            user_socket.to_str().unwrap(),
        );
    fs::write(server.config_path(), &text).unwrap();
    let master = server
        .spawn(&["-M", "-N", "-S", user_socket.to_str().unwrap()], None)
        .unwrap();
    // The master has authenticated once it answers a control check.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let check = ["-S", user_socket.to_str().unwrap(), "-O", "check"];
            let output = server
                .spawn(&check, None)
                .unwrap()
                .wait(Duration::from_secs(30));
            if output.unwrap().status.success() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let before = auth_attempts(&server);
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    status(&conn, Status::Live).await;
    assert_eq!(ssh_pids(&server).len(), 2);
    let accepted = ("publickey".into(), "Accepted".into());
    assert_eq!(auth_attempts(&server)[&accepted] - before[&accepted], 1);
    for (from, to) in [
        (
            "HostName 127.0.0.1".to_owned(),
            "HostName localhost".to_owned(),
        ),
        (format!("Port {}", server.port()), "Port 1".to_owned()),
        ("client_key\n".to_owned(), "client_key_1\n".to_owned()),
    ] {
        let attempts = auth_attempts(&server);
        fs::write(server.config_path(), text.replace(&from, &to)).unwrap();
        let session_pid = ssh_pids(&server)
            .into_iter()
            .find(|pid| *pid != master.pid())
            .unwrap();
        unsafe {
            libc::kill(session_pid as i32, libc::SIGKILL);
        }
        status(&conn, Status::Backoff).await;
        clock.advance(6001);
        status(&conn, Status::NeedsAuth).await;
        assert_eq!(auth_attempts(&server), attempts);
        fs::write(server.config_path(), &text).unwrap();
        conn.retry().await.unwrap();
        status(&conn, Status::Live).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn black_hole_is_unreachable_by_deadline_and_recovers_in_one_backoff_step() {
    if !paced_child() {
        return;
    }
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    status(&conn, Status::Live).await;
    server
        .proxy()
        .set(Faults {
            black_hole: true,
            ..Faults::default()
        })
        .unwrap();
    let began = Instant::now();
    let failed = status(&conn, Status::Backoff).await;
    assert!(
        began.elapsed() <= deadline() + Duration::from_secs(1),
        "{:?}",
        began.elapsed()
    );
    assert_eq!(alias_auth(&server).class, Some(HostClass::Key));
    let delay = failed.next_retry_ms.unwrap() - clock.wall_ms();
    assert!((0..=6000).contains(&delay));
    server.proxy().clear().unwrap();
    server.proxy().reset().unwrap();
    clock.advance(delay.max(0) as u64 + 1);
    tokio::time::timeout(Duration::from_secs(6), status(&conn, Status::Live))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_rental_reconnects_after_repeated_failures_within_collection_grace() {
    if !paced_child() {
        return;
    }
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let mut config = config(&server);
    config.alias = "orx-box-recovery".into();
    let text = fs::read_to_string(server.config_path()).unwrap();
    fs::write(
        server.config_path(),
        text.replace(
            &format!("Host {}", server.alias()),
            &format!("Host {}", config.alias),
        ),
    )
    .unwrap();
    let auth = Auth::new(&store, &config.alias).unwrap();
    let conn = HostConn::start_shared(config, store, clock.clone(), auth).unwrap();
    status(&conn, Status::Live).await;
    let disconnected_at = clock.wall_ms();
    server
        .proxy()
        .set(Faults {
            refuse_new: true,
            ..Faults::default()
        })
        .unwrap();
    server.proxy().reset().unwrap();

    let mut state = conn.state();
    for failures in 1..=5 {
        let failed = tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                let snapshot = state.borrow_and_update().clone();
                if snapshot.status == Status::Backoff && snapshot.failures >= failures {
                    return snapshot;
                }
                state.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        if failures < 5 {
            let delay = failed.next_retry_ms.unwrap() - clock.wall_ms();
            clock.advance(delay.max(0) as u64 + 1);
        }
    }

    server.proxy().clear().unwrap();
    let elapsed = u64::try_from(clock.wall_ms() - disconnected_at).unwrap();
    clock.advance(420_000_u64.saturating_sub(elapsed));
    tokio::time::timeout(Duration::from_secs(6), status(&conn, Status::Live))
        .await
        .expect("rental must retry before collection grace expires");
    assert!(clock.wall_ms() - disconnected_at < 900_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wall_clock_jump_reconnects_key_host_without_backoff() {
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    status(&conn, Status::Live).await;
    let before = ssh_pids(&server);
    clock.wall.fetch_add(30 * 60 * 1000, Ordering::SeqCst);
    tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            tokio::time::sleep(Duration::from_millis(50)).await;
            if conn.state.borrow().status == Status::Live && ssh_pids(&server) != before {
                break;
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarted_key_host_retries_and_auth_failure_keeps_its_breaker_open() {
    let server = Sshd::start().unwrap();
    let config = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("LogLevel ERROR", "LogLevel QUIET");
    fs::write(server.config_path(), config).unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    let live = status(&conn, Status::Live).await;
    assert_eq!(live.last_live_ms, Some(live.since_ms));
    server
        .proxy()
        .set(Faults {
            refuse_new: true,
            ..Faults::default()
        })
        .unwrap();
    unsafe {
        libc::kill(ssh_pids(&server)[0] as i32, libc::SIGKILL);
    }
    let backoff = status(&conn, Status::Backoff).await;
    drop(conn);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let restarted = start(&server, clock.clone());
    let restored = status(&restarted, Status::Backoff).await;
    assert_eq!(restored.since_ms, backoff.since_ms);
    assert_eq!(restored.failures, backoff.failures + 1);
    let attempts = auth_attempts(&server);
    server.proxy().clear().unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let delay = restored.next_retry_ms.unwrap() - clock.wall_ms();
    clock.advance(delay.max(0) as u64 + 1);
    let failed = status(&restarted, Status::NeedsAuth).await;
    assert_eq!(alias_auth(&server).class, Some(HostClass::Key));
    assert!(failed.last_live_ms >= live.last_live_ms);
    assert!(failed.since_ms > backoff.since_ms);
    let after = auth_attempts(&server);
    assert_eq!(
        after.values().sum::<usize>() - attempts.values().sum::<usize>(),
        1
    );
    drop(restarted);
    let restarted = start(&server, clock.clone());
    assert_eq!(status(&restarted, Status::NeedsAuth).await, failed);
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(ssh_pids(&server).is_empty());
    assert_eq!(auth_attempts(&server), after);
    assert!(restarted.retry().await.is_err());
    let retried = status(&restarted, Status::NeedsAuth).await;
    assert!(retried.since_ms >= failed.since_ms + 30 * 60 * 1000);
    assert_eq!(retried.last_live_ms, failed.last_live_ms);
}

#[test]
fn jittered_cadence_is_persisted_and_capped_at_fifteen_minutes() {
    let server = Sshd::start().unwrap();
    let state = HostState::default();
    let (publish, _) = watch::channel(state.clone());
    let (_, incoming) = mpsc::channel(1);
    let mut actor = Actor {
        auth: Auth::new(
            &Store::open_at(server.directory().join("store")).unwrap(),
            server.alias(),
        )
        .unwrap(),
        config: config(&server),
        store: Store::open_at(server.directory().join("store")).unwrap(),
        clock: TestClock::new(),
        state,
        publish,
        incoming,
    };
    actor.classify(HostClass::Key);
    let mut since = None;
    for base in [5000, 15000, 60000, 300000, 900000, 900000] {
        actor.failed(false, "network failure".into());
        let delay = actor.state.next_retry_ms.unwrap() - actor.clock.wall_ms();
        assert!((base * 8 / 10 - 10..=(base * 12 / 10).min(900000)).contains(&delay));
        assert_eq!(
            actor
                .store
                .engine_host(server.alias(), "~/engine-root")
                .unwrap()
                .unwrap(),
            actor.state
        );
        assert_eq!(
            *since.get_or_insert(actor.state.since_ms),
            actor.state.since_ms
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_without_askpass_preserves_key_reconnection() {
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    status(&conn, Status::Live).await;
    let prompt: Prompt = Arc::new(|_| Box::pin(async { bail!("Key auth must not prompt") }));
    conn.connect_interactive(prompt, false).await.unwrap();
    assert_eq!(alias_auth(&server).class, Some(HostClass::Key));
    let path = ssh::socket(&config(&server), &alias_auth(&server).route.unwrap()).unwrap();
    assert!(!path.exists());
    assert!(!ssh::master_alive(&config(&server), &path).await.unwrap());
    unsafe {
        libc::kill(ssh_pids(&server)[0] as i32, libc::SIGKILL);
    }
    status(&conn, Status::Backoff).await;
    clock.advance(6001);
    assert_eq!(
        {
            status(&conn, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_retry_and_connect_actions_share_one_attempt() {
    let server = Sshd::start().unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::NeedsAuth).await;
    let before = auth_attempts(&server);
    let retries = (0..30).map(|index| {
        let conn = conn.clone();
        async move {
            if index % 2 == 0 {
                assert!(conn.retry().await.is_err());
            } else {
                let prompt: Prompt = Arc::new(|_| Box::pin(async { bail!("Unexpected prompt") }));
                assert!(conn.connect_interactive(prompt, false).await.is_err());
            }
        }
    });
    futures::future::join_all(retries).await;
    assert!(ssh_pids(&server).is_empty());
    let after = auth_attempts(&server);
    assert_eq!(
        after.values().sum::<usize>() - before.values().sum::<usize>(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_stderr_is_retained_even_when_a_child_holds_the_pipe_open() {
    let server = Sshd::start().unwrap();
    let text = fs::read_to_string(server.config_path()).unwrap();
    fs::write(server.config_path(), format!("ProxyCommand /bin/sh -c 'echo Permission denied sentinel >&2; sleep 3 >/dev/null & exit 1'\n{text}")).unwrap();
    let conn = start(&server, TestClock::new());
    let failed = status(&conn, Status::NeedsAuth).await;
    assert!(failed.cause.unwrap().contains("Permission denied sentinel"));
    assert!(auth_attempts(&server).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bootstrap_uses_posix_sh_under_the_available_login_shell() {
    let server = Sshd::start().unwrap();
    let shell = if std::path::Path::new("/bin/csh").is_file() {
        "/bin/csh"
    } else {
        "/bin/sh"
    };
    let command = server.directory().join("command");
    let text = fs::read_to_string(&command)
        .unwrap()
        .replace("exec /bin/sh -c", &format!("exec {shell} -f -c"));
    fs::write(command, text).unwrap();
    let stub = ssh::stub("~/engine-root");
    assert!(stub.starts_with("exec /bin/sh -c '"));
    assert!(!stub.contains('\n'));
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_host_key_failure_opens_the_breaker_without_authentication() {
    let server = Sshd::start().unwrap();
    let text = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("LogLevel ERROR", "LogLevel QUIET");
    fs::write(server.config_path(), text).unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
    let before = auth_attempts(&server);
    let wrong = fs::read_to_string(server.directory().join("client_key.pub")).unwrap();
    fs::write(
        server.directory().join("known_hosts"),
        format!("[127.0.0.1]:{} {wrong}", server.port()),
    )
    .unwrap();
    server.proxy().reset().unwrap();
    status(&conn, Status::Backoff).await;
    assert!(conn.retry().await.is_err());
    assert_eq!(
        status(&conn, Status::NeedsAuth).await.cause.as_deref(),
        Some("Host key verification failed")
    );
    assert!(ssh_pids(&server).is_empty());
    assert_eq!(auth_attempts(&server), before);
}

#[test]
fn authentication_errors_win_over_secondary_proxy_disconnects() {
    assert!(ssh::failure("closed", "", None, true).breaker);
    assert!(ssh::failure("closed", "Permission denied (publickey).\nkex_exchange_identification: Connection closed by UNKNOWN port 65535", None, true).breaker);
    assert!(
        !ssh::failure(
            "closed",
            "ssh: connect to host box port 22: Connection refused",
            None,
            true
        )
        .breaker
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_key_confirmations_are_relayed_without_classifying_interactive_auth() {
    let server = Sshd::start().unwrap();
    let text = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("StrictHostKeyChecking yes", "StrictHostKeyChecking ask");
    fs::write(server.config_path(), text).unwrap();
    fs::write(server.directory().join("known_hosts"), "").unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::NeedsAuth).await;
    let questions = Arc::new(std::sync::Mutex::new(Vec::new()));
    let prompt: Prompt = Arc::new({
        let questions = questions.clone();
        move |question| {
            let mut questions = questions.lock().unwrap();
            questions.push(question);
            let answer = if questions.len() == 1 {
                "invalid"
            } else {
                "yes"
            };
            Box::pin(async move { Ok(Some(answer.into())) })
        }
    });
    conn.connect_interactive(prompt, false).await.unwrap();
    assert_eq!(
        {
            status(&conn, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
    assert_eq!(questions.lock().unwrap().len(), 2);
    let path = ssh::socket(&config(&server), &alias_auth(&server).route.unwrap()).unwrap();
    assert!(!path.exists());
    assert!(!ssh::master_alive(&config(&server), &path).await.unwrap());
    assert_eq!(ssh_pids(&server).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_connect_keeps_its_prompt_and_route_approval_after_automatic_failure() {
    let passphrase = "queued-connect";
    let server = SshdBuilder {
        extra_client_keys: vec![Some(passphrase.into())],
        identity_files: vec!["client_key_1".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    server
        .proxy()
        .set(Faults {
            paused: true,
            ..Faults::default()
        })
        .unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Connecting).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while ssh_pids(&server).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let text = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("HostName 127.0.0.1", "HostName localhost");
    fs::write(server.config_path(), text).unwrap();
    let key = fs::read_to_string(server.directory().join("host_key.pub")).unwrap();
    let mut known = fs::read_to_string(server.directory().join("known_hosts")).unwrap();
    known.push_str(&format!("[localhost]:{} {key}", server.port()));
    fs::write(server.directory().join("known_hosts"), known).unwrap();
    let prompts = Arc::new(AtomicU64::new(0));
    let prompt: Prompt = Arc::new({
        let prompts = prompts.clone();
        move |_| {
            prompts.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Some(passphrase.into())) })
        }
    });
    let connect = conn.connect_interactive(prompt, false);
    tokio::pin!(connect);
    assert!(futures::poll!(connect.as_mut()).is_pending());
    server.proxy().clear().unwrap();
    tokio::time::timeout(Duration::from_secs(20), connect)
        .await
        .unwrap()
        .unwrap();
    status(&conn, Status::Live).await;
    let pinned = alias_auth(&server);
    assert_eq!(pinned.class, Some(HostClass::Interactive));
    assert!(pinned.route.unwrap().contains("hostname localhost"));
    assert_eq!(prompts.load(Ordering::SeqCst), 1);
    let attempts = logged_attempts(&server, 3).await;
    assert_eq!(attempts[&("publickey".into(), "Accepted".into())], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_success_coalesces_queued_connect_without_another_login() {
    let server = Sshd::start().unwrap();
    server
        .proxy()
        .set(Faults {
            paused: true,
            ..Faults::default()
        })
        .unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Connecting).await;
    let prompt: Prompt =
        Arc::new(|_| Box::pin(async { bail!("Successful key auth must not prompt") }));
    let connect = conn.connect_interactive(prompt, false);
    tokio::pin!(connect);
    assert!(futures::poll!(connect.as_mut()).is_pending());
    server.proxy().clear().unwrap();
    tokio::time::timeout(Duration::from_secs(20), connect)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        {
            status(&conn, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
    let attempts = logged_attempts(&server, 2).await;
    assert_eq!(attempts[&("publickey".into(), "Accepted".into())], 1);
    assert_eq!(ssh_pids(&server).len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_feed_uses_its_wait_budget_while_probes_keep_the_host_live() {
    if !paced_child() {
        return;
    }
    let server = Sshd::start().unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
    let before = ssh_pids(&server);
    let attempts = auth_attempts(&server);
    server
        .proxy()
        .set(Faults {
            latency: crate::config::paced(Duration::from_secs(6)),
            ..Faults::default()
        })
        .unwrap();
    tokio::time::sleep(crate::config::paced(Duration::from_secs(70))).await;
    assert_eq!(conn.state.borrow().status, Status::Live);
    assert_eq!(ssh_pids(&server), before);
    assert_eq!(auth_attempts(&server), attempts);
    server.proxy().clear().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_retry_queued_during_auth_failure_cannot_reopen_the_breaker() {
    let server = Sshd::start().unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    server
        .proxy()
        .set(Faults {
            latency: Duration::from_millis(100),
            ..Faults::default()
        })
        .unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    store
        .seed_engine_host(
            server.alias(),
            "~/engine-root",
            Some(HostClass::Key),
            &HostState {
                ..HostState::default()
            },
        )
        .unwrap();
    let conn = {
        let config = config(&server);
        let auth = Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, TestClock::new(), auth)
    }
    .unwrap();
    status(&conn, Status::Connecting).await;
    conn.retry_key().await.unwrap();
    status(&conn, Status::NeedsAuth).await;
    assert_eq!(auth_attempts(&server).values().sum::<usize>(), 1);
    assert!(conn.retry_key().await.is_err());
    assert_eq!(auth_attempts(&server).values().sum::<usize>(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_retry_resets_the_backoff_step_without_prompting() {
    let server = Sshd::start().unwrap();
    server
        .proxy()
        .set(Faults {
            refuse_new: true,
            ..Faults::default()
        })
        .unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    store
        .seed_engine_host(
            server.alias(),
            "~/engine-root",
            Some(HostClass::Key),
            &HostState {
                status: Status::Backoff,
                failures: 4,
                next_retry_ms: Some(clock.wall_ms() + 900_000),
                ..HostState::default()
            },
        )
        .unwrap();
    let conn = {
        let config = config(&server);
        let auth = Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, clock.clone(), auth)
    }
    .unwrap();
    assert!(conn.retry_key().await.is_err());
    let state = status(&conn, Status::Backoff).await;
    assert_eq!(state.failures, 1);
    let delay = state.next_retry_ms.unwrap() - state.since_ms;
    assert!((4000..=6000).contains(&delay), "{delay}");
    assert!(auth_attempts(&server).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_retry_cannot_approve_a_changed_route() {
    let server = Sshd::start().unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
    let pinned = alias_auth(&server).route;
    server
        .proxy()
        .set(Faults {
            refuse_new: true,
            ..Faults::default()
        })
        .unwrap();
    server.proxy().reset().unwrap();
    status(&conn, Status::Backoff).await;
    let config = fs::read_to_string(server.config_path()).unwrap();
    fs::write(
        server.config_path(),
        format!("{config}\n    ProxyCommand false\n"),
    )
    .unwrap();
    let attempts = auth_attempts(&server);
    assert!(conn.retry_key().await.is_err());
    let failed = status(&conn, Status::NeedsAuth).await;
    assert_eq!(alias_auth(&server).route, pinned);
    assert_eq!(
        failed.cause.as_deref(),
        Some("SSH route changed; press Connect to approve it")
    );
    assert_eq!(auth_attempts(&server), attempts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn key_retry_is_a_noop_while_connecting_and_during_live_calls() {
    let server = Sshd::start().unwrap();
    server
        .proxy()
        .set(Faults {
            latency: Duration::from_millis(100),
            ..Faults::default()
        })
        .unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Connecting).await;
    conn.retry_key().await.unwrap();
    status(&conn, Status::Live).await;
    server.proxy().clear().unwrap();
    let pids = ssh_pids(&server);
    let attempts = auth_attempts(&server);
    assert_eq!(attempts[&("publickey".into(), "Accepted".into())], 1);
    let calls = conn.clone();
    let marker = server.home().join("in-flight");
    let call = tokio::spawn(async move {
        exec(
            &calls,
            "touch ~/in-flight; sleep 1; printf completed".into(),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    conn.retry_key().await.unwrap();
    assert_eq!(call.await.unwrap().stdout, b"completed");
    assert_eq!(ssh_pids(&server), pids);
    assert_eq!(auth_attempts(&server), attempts);
}

#[test]
fn ssh_causes_are_exact_human_diagnostics() {
    for (stderr, expected) in [
        ("clo@gpu-box: Permission denied (publickey).\r\n", "Permission denied (publickey)"),
        ("ssh: connect to host gpu-box port 22: Connection refused\r\n", "Connection refused"),
        ("ssh: connect to host gpu-box port 22: Operation timed out\r\n", "Connection timed out"),
        ("ssh: connect to host gpu-box port 22: Connection timed out\r\n", "Connection timed out"),
        ("@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @\r\nOffending key in /tmp/known_hosts:1\r\nHost key verification failed.\r\n", "Host key verification failed"),
        ("Permission denied (publickey).\nkex_exchange_identification: Connection closed by UNKNOWN port 65535", "Permission denied (publickey)"),
    ] {
        assert_eq!(ssh::failure("SSH connection closed", stderr, None, true).cause, expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_auth_rejection_is_not_a_bootstrap_failure() {
    let server = Sshd::start().unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let conn = start(&server, TestClock::new());
    let failed = status(&conn, Status::NeedsAuth).await;
    assert_eq!(
        failed.cause.as_deref(),
        Some("Permission denied (publickey)")
    );
    let error = conn
        .request(Request::Hello {
            proto: orx_host_proto::PROTOCOL_VERSION,
        })
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Permission denied (publickey)");
    assert!(!error.downcast_ref::<Unavailable>().unwrap().sent);
}

#[test]
fn ssh_diagnostics_preserve_primary_errors_and_proxy_hops() {
    for (error, stderr, code, expected, breaker) in [
        ("SSH connection closed", "bash: module: command not found\n", Some(98), "Could not prepare the remote run directory: command not found", true),
        ("SSH connection closed", "zsh:1: command not found: cloudflared\n", Some(255), "SSH connection closed: command not found: cloudflared", true),
        ("SSH connection closed", "ssh: Could not resolve hostname x: nodename nor servname provided, or not known\n", Some(255), "Could not resolve hostname x: nodename nor servname provided, or not known", false),
        ("SSH connection closed", "kex_exchange_identification: read: X\n", Some(255), "SSH connection closed: X", true),
        ("SSH connection closed", "user@jump: Permission denied (publickey).\r\nkex_exchange_identification: Connection closed by UNKNOWN port 65535\n", Some(255), "Permission denied (publickey)", true),
        ("invalid response", "", Some(97), "The remote host rejected the bootstrap request", true),
        ("invalid response", "", Some(96), "Could not install or verify the remote host binary", true),
        ("invalid response", "", Some(95), "The remote host binary is missing", true),
    ] {
        let failure = ssh::failure(error, stderr, code, true);
        assert_eq!(failure.cause, expected);
        assert_eq!(failure.breaker, breaker);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_shell_noise_and_stub_failure_do_not_open_the_breaker() {
    let server = Sshd::start().unwrap();
    fs::write(
        server.directory().join("command"),
        "#!/bin/sh\necho 'bash: module: command not found' >&2\nexit 98\n",
    )
    .unwrap();
    Store::open_at(server.directory().join("store"))
        .unwrap()
        .seed_engine_host(
            server.alias(),
            "~/engine-root",
            Some(HostClass::Key),
            &HostState::default(),
        )
        .unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    let failed = status(&conn, Status::Backoff).await;
    assert_eq!(alias_auth(&server).class, Some(HostClass::Key));
    assert_eq!(
        failed.cause.as_deref(),
        Some("Could not prepare the remote run directory: command not found")
    );
    assert_eq!(
        auth_attempts(&server)[&("publickey".into(), "Accepted".into())],
        1
    );
    clock.advance(6001);
    logged_attempts(&server, 4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropped_connection_backs_off_after_authentication() {
    let server = Sshd::start().unwrap();
    fs::write(
        server.directory().join("command"),
        "#!/bin/sh\necho 'Connection closed by 127.0.0.1 port 22' >&2\nexit 255\n",
    )
    .unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    let failed = status(&conn, Status::Backoff).await;
    assert_eq!(
        failed.cause.as_deref(),
        Some("Connection closed by 127.0.0.1 port 22")
    );
    assert_eq!(
        auth_attempts(&server)[&("publickey".into(), "Accepted".into())],
        1
    );
    clock.advance(6001);
    logged_attempts(&server, 4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_contact_network_allow_list_retries_without_authentication() {
    for message in [
        "Connection refused",
        "Connection timed out",
        "Could not resolve hostname x: unknown host",
        "No route to host",
        "Network is unreachable",
        "Connection reset by peer",
        "Host is down",
        "Network is down",
    ] {
        let server = Sshd::start().unwrap();
        let text = fs::read_to_string(server.config_path()).unwrap();
        let marker = server.directory().join("attempts");
        fs::write(
            server.config_path(),
            format!(
                "ProxyCommand /bin/sh -c 'echo x >> {}; echo {} >&2; exit 1'\n{text}",
                marker.display(),
                message
            ),
        )
        .unwrap();
        let clock = TestClock::new();
        let conn = start(&server, clock.clone());
        assert_eq!(
            {
                status(&conn, Status::Backoff).await;
                alias_auth(&server).class
            },
            None
        );
        assert!(auth_attempts(&server).is_empty());
        clock.advance(6001);
        tokio::time::timeout(Duration::from_secs(5), async {
            while fs::read_to_string(&marker).unwrap().lines().count() < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(auth_attempts(&server).is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_breaker_is_shared_across_roots_and_connect_clears_it() {
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let auth = Auth::new(&store, server.alias()).unwrap();
    let mut connections = Vec::new();
    for root in ["~/ssh-root", "~/slurm-root"] {
        let mut cfg = config(&server);
        cfg.root = root.into();
        let conn = HostConn::start_shared(
            cfg,
            Store::open_at(server.directory().join("store")).unwrap(),
            clock.clone(),
            auth.clone(),
        )
        .unwrap();
        status(&conn, Status::Live).await;
        connections.push(conn);
    }
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let before = auth_attempts(&server);
    server.proxy().reset().unwrap();
    for conn in &connections {
        status(conn, Status::Backoff).await;
    }
    clock.advance(6001);
    for conn in &connections {
        status(conn, Status::NeedsAuth).await;
    }
    let blocked = auth_attempts(&server);
    assert_eq!(
        blocked.values().sum::<usize>() - before.values().sum::<usize>(),
        1
    );
    let mut cfg = config(&server);
    cfg.root = "~/adopted-root".into();
    connections.push(
        HostConn::start_shared(
            cfg,
            Store::open_at(server.directory().join("store")).unwrap(),
            clock.clone(),
            auth,
        )
        .unwrap(),
    );
    status(&connections[2], Status::NeedsAuth).await;
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(auth_attempts(&server), blocked);
    fs::write(
        server.directory().join("authorized_keys"),
        fs::read(server.directory().join("client_key.pub")).unwrap(),
    )
    .unwrap();
    connections[0].retry().await.unwrap();
    for conn in &connections {
        status(conn, Status::Live).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_with_prompt_on_live_route_preserves_inflight_launch() {
    for keep_master in [false, true] {
        let server = Sshd::start().unwrap();
        let conn = start(&server, TestClock::new());
        status(&conn, Status::Live).await;
        let pids = ssh_pids(&server);
        let before = auth_attempts(&server);
        server
            .proxy()
            .set(Faults {
                latency: Duration::from_millis(400),
                ..Faults::default()
            })
            .unwrap();
        let calls = conn.clone();
        let request = tokio::spawn(async move {
            calls
                .request(Request::Launch(orx_host_proto::LaunchIntent {
                    run: "connect-inflight".into(),
                    digest: orx_host::sha(b"connect-inflight"),
                    run_sh: "true".into(),
                    env: Default::default(),
                    source: None,
                    cancel_sh: None,
                }))
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !server
                .home()
                .join("engine-root/runs/connect-inflight/started")
                .exists()
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        conn.connect_interactive(
            Arc::new(|_| Box::pin(async { panic!("Live host prompted") })),
            keep_master,
        )
        .await
        .unwrap();
        assert!(matches!(
            request.await.unwrap().unwrap(),
            Response::Launch { .. }
        ));
        assert_eq!(ssh_pids(&server), pids);
        if keep_master {
            let path = ssh::socket(&config(&server), &alias_auth(&server).route.unwrap()).unwrap();
            assert!(ssh::master_alive(&config(&server), &path).await.unwrap());
            let attempts = auth_attempts(&server);
            assert_eq!(
                attempts
                    .get(&("publickey".into(), "Accepted".into()))
                    .copied()
                    .unwrap_or(0),
                before
                    .get(&("publickey".into(), "Accepted".into()))
                    .copied()
                    .unwrap_or(0)
                    + 1
            );
        } else {
            assert_eq!(auth_attempts(&server), before);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_through_a_peer_root_updates_the_alias_authentication_class() {
    let server = Sshd::start().unwrap();
    let key = fs::read(server.directory().join("authorized_keys")).unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let auth = Auth::new(&store, server.alias()).unwrap();
    let first =
        HostConn::start_shared(config(&server), store, clock.clone(), auth.clone()).unwrap();
    assert_eq!(
        {
            status(&first, Status::NeedsAuth).await;
            alias_auth(&server).class
        },
        Some(HostClass::Interactive)
    );
    let mut cfg = config(&server);
    cfg.root = "~/peer-root".into();
    let peer = HostConn::start_shared(
        cfg,
        Store::open_at(server.directory().join("store")).unwrap(),
        clock,
        auth,
    )
    .unwrap();
    status(&peer, Status::NeedsAuth).await;
    let rejected = auth_attempts(&server)
        .get(&("publickey".into(), "Failed".into()))
        .copied()
        .unwrap_or(0);
    fs::write(server.directory().join("authorized_keys"), key).unwrap();
    peer.retry().await.unwrap();
    assert_eq!(
        {
            status(&peer, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
    assert_eq!(
        {
            status(&first, Status::Live).await;
            alias_auth(&server).class
        },
        Some(HostClass::Key)
    );
    assert_eq!(
        auth_attempts(&server)
            .get(&("publickey".into(), "Failed".into()))
            .copied()
            .unwrap_or(0),
        rejected
    );
}

#[test]
fn unmatched_ssh_diagnostics_keep_paths_context_and_bounds() {
    for (stderr, expected) in [
        (
            "/bin/bash: module: command not found",
            "/bin/bash: module: command not found",
        ),
        (r"C:\ssh.exe: broken", r"C:\ssh.exe: broken"),
        ("C: broken", "C: broken"),
        (
            "first useful message\nnoise\nlast useful message",
            "first useful message; last useful message",
        ),
    ] {
        let failure = ssh::failure("SSH failed", stderr, Some(255), true);
        let expected = format!("SSH failed: {expected}");
        assert_eq!(failure.cause, expected);
        assert!(failure.breaker);
    }
    assert!(ssh::failure("SSH connection failed", "Unclassified ProxyCommand failure\nkex_exchange_identification: Connection closed by remote host", None, true).breaker);
    let failure = ssh::failure("SSH failed", "channel 0: open failed: administratively prohibited\nkex_exchange_identification: Connection closed by UNKNOWN port 65535", Some(255), true);
    assert!(failure.breaker);
    assert_eq!(failure.cause, "SSH failed: channel 0: open failed: administratively prohibited; Connection closed by UNKNOWN port 65535");
    assert!(ssh::failure("SSH failed", "Corrupted MAC on input", None, true).breaker);
    for stderr in [
        "Host is down",
        "Network is down",
        "Connection closed by host",
        "kex_exchange_identification: connection closed",
        "ssh_exchange_identification: connection closed",
    ] {
        assert!(!ssh::failure("SSH failed", stderr, None, true).breaker);
    }
    let failure = ssh::failure("SSH failed", &"x".repeat(400), Some(255), true);
    assert_eq!(failure.cause.chars().count(), "SSH failed: ".len() + 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ambiguous_key_failures_open_the_breaker() {
    for message in ["Corrupted MAC on input", "Unclassified connection failure"] {
        let server = Sshd::start().unwrap();
        let store = Store::open_at(server.directory().join("store")).unwrap();
        store
            .seed_engine_host(
                server.alias(),
                "~/engine-root",
                Some(HostClass::Key),
                &HostState {
                    ..HostState::default()
                },
            )
            .unwrap();
        fs::write(
            server.directory().join("command"),
            format!("#!/bin/sh\necho '{message}' >&2\nexit 255\n"),
        )
        .unwrap();
        let clock = TestClock::new();
        let conn = start(&server, clock.clone());
        let failed = status(&conn, Status::NeedsAuth).await;
        assert_eq!(
            failed.cause.unwrap(),
            format!("SSH connection closed: {message}")
        );
        assert!(store
            .engine_host_auth(server.alias())
            .unwrap()
            .blocked
            .is_some());
        let attempts = auth_attempts(&server).values().sum::<usize>();
        clock.advance(30 * 60 * 1000);
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(auth_attempts(&server).values().sum::<usize>(), attempts);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn alias_authorization_survives_lazy_roots_and_restarts() {
    let server = Sshd::start().unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    for root in ["~/engine-root", "~/slurm-root"] {
        store
            .seed_engine_host(
                server.alias(),
                root,
                Some(HostClass::Key),
                &HostState {
                    status: Status::NeedsAuth,
                    cause: Some("Permission denied (publickey)".into()),
                    ..HostState::default()
                },
            )
            .unwrap();
    }
    for _ in 0..2 {
        let conn = start(&server, TestClock::new());
        status(&conn, Status::NeedsAuth).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(auth_attempts(&server).is_empty());
        drop(conn);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let conn = start(&server, TestClock::new());
    conn.retry().await.unwrap();
    status(&conn, Status::Live).await;
    assert!(store
        .engine_host_auth(server.alias())
        .unwrap()
        .blocked
        .is_none());
    drop(conn);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let auth = Auth::new(&store, server.alias()).unwrap();
    let mut peers = Vec::new();
    for root in ["~/engine-root", "~/slurm-root"] {
        let mut cfg = config(&server);
        cfg.root = root.into();
        let conn = HostConn::start_shared(
            cfg,
            Store::open_at(server.directory().join("store")).unwrap(),
            TestClock::new(),
            auth.clone(),
        )
        .unwrap();
        status(&conn, Status::Live).await;
        peers.push(conn);
    }
    drop(peers);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let auth = Auth::new(&store, server.alias()).unwrap();
    let primary = HostConn::start_shared(
        config(&server),
        Store::open_at(server.directory().join("store")).unwrap(),
        TestClock::new(),
        auth.clone(),
    )
    .unwrap();
    status(&primary, Status::Live).await;
    let old = fs::read_to_string(server.config_path()).unwrap();
    fs::write(
        server.config_path(),
        old.replace("HostName 127.0.0.1", "HostName localhost"),
    )
    .unwrap();
    let key = fs::read_to_string(server.directory().join("host_key.pub")).unwrap();
    let mut known = fs::read_to_string(server.directory().join("known_hosts")).unwrap();
    known.push_str(&format!("[localhost]:{} {key}", server.port()));
    fs::write(server.directory().join("known_hosts"), known).unwrap();
    primary.retry().await.unwrap();
    let pids = ssh_pids(&server);
    let mut cfg = config(&server);
    cfg.root = "~/slurm-root".into();
    let lazy = HostConn::start_shared(
        cfg,
        Store::open_at(server.directory().join("store")).unwrap(),
        TestClock::new(),
        auth,
    )
    .unwrap();
    status(&lazy, Status::Live).await;
    assert_eq!(primary.state().borrow().status, Status::Live);
    assert!(pids.iter().all(|pid| ssh_pids(&server).contains(pid)));
    assert!(store
        .engine_host_auth(server.alias())
        .unwrap()
        .blocked
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn broken_config_during_connect_leaves_the_actor_alive() {
    let server = Sshd::start().unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
    let original = fs::read_to_string(server.config_path()).unwrap();
    fs::write(server.config_path(), "InvalidOrxOption yes\n").unwrap();
    assert!(conn
        .connect_interactive(Arc::new(|_| Box::pin(async { Ok(None) })), false)
        .await
        .is_err());
    assert!(!conn.actions.is_closed());
    assert_eq!(conn.state().borrow().status, Status::NeedsAuth);
    fs::write(server.config_path(), original).unwrap();
    conn.retry().await.unwrap();
    status(&conn, Status::Live).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_first_success_cannot_clear_a_concurrent_breaker() {
    let server = Sshd::start().unwrap();
    server
        .proxy()
        .set(Faults {
            latency: Duration::from_millis(100),
            ..Faults::default()
        })
        .unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let auth = Auth::new(&store, server.alias()).unwrap();
    let conn = HostConn::start_shared(
        config(&server),
        Store::open_at(server.directory().join("store")).unwrap(),
        TestClock::new(),
        auth.clone(),
    )
    .unwrap();
    status(&conn, Status::Connecting).await;
    logged_attempts(&server, 2).await;
    auth.block(
        &store,
        server.alias(),
        123,
        "Peer root requires Connect".into(),
    )
    .unwrap();
    status(&conn, Status::NeedsAuth).await;
    assert_eq!(
        store.engine_host_auth(server.alias()).unwrap().blocked,
        Some((123, "Peer root requires Connect".into()))
    );
    assert_eq!(alias_auth(&server).class, Some(HostClass::Key));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn silent_proxy_failure_stays_behind_the_breaker_until_connect() {
    let server = Sshd::start().unwrap();
    let original = fs::read_to_string(server.config_path()).unwrap();
    let counter = server.directory().join("proxy-attempts");
    fs::write(
        server.config_path(),
        format!(
            "ProxyCommand /bin/sh -c 'echo attempt >> {}; exit 1'\n{original}",
            counter.display()
        ),
    )
    .unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    status(&conn, Status::NeedsAuth).await;
    assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 1);
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 1);
    assert!(conn
        .connect_interactive(
            Arc::new(|_| Box::pin(async { panic!("Silent proxy prompted") })),
            false
        )
        .await
        .is_err());
    assert_eq!(conn.state().borrow().status, Status::NeedsAuth);
    assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 2);
    assert!(auth_attempts(&server).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_key_mfa_deadline_opens_the_breaker_without_a_reconnect_storm() {
    if !paced_child() {
        return;
    }
    let server = Sshd::start().unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    store
        .seed_engine_host(
            server.alias(),
            "~/engine-root",
            Some(HostClass::Key),
            &HostState::default(),
        )
        .unwrap();
    fs::write(
        server.directory().join("command"),
        "#!/bin/sh\nprintf 'Approve MFA push: ' >&2\nexec sleep 60\n",
    )
    .unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    let failed = status(&conn, Status::NeedsAuth).await;
    assert!(failed
        .cause
        .as_deref()
        .unwrap()
        .contains("Approve MFA push"));
    assert_eq!(
        auth_attempts(&server)[&("publickey".into(), "Accepted".into())],
        1
    );
    let attempts = auth_attempts(&server);
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(auth_attempts(&server), attempts);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn proxy_jump_second_factor_fails_without_prompting() {
    let jump = SshdBuilder {
        authentication_methods: "publickey,publickey".into(),
        extra_client_keys: vec![Some("orx transport 'passphrase'".into())],
        identity_files: vec!["client_key".into(), "client_key_1".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    let server = Sshd::start().unwrap();
    let target = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("Host orx-test\n", "Host orx-test\n  ProxyJump orx-jump\n");
    let hop = fs::read_to_string(jump.config_path())
        .unwrap()
        .replace("Host orx-test\n", "Host orx-jump\n")
        .replace("  BatchMode yes\n", "");
    fs::write(server.config_path(), format!("{target}{hop}")).unwrap();
    let clock = TestClock::new();
    let conn = start(&server, clock.clone());
    let failed = status(&conn, Status::NeedsAuth).await;
    assert!(
        failed
            .cause
            .as_deref()
            .unwrap()
            .contains("Permission denied"),
        "{failed:?}"
    );
    let attempts = auth_attempts(&jump);
    assert!(!attempts.is_empty());
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(auth_attempts(&jump), attempts);
    assert!(auth_attempts(&server).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unpersisted_breaker_still_blocks_every_root() {
    let server = Sshd::start().unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let auth = Auth::new(&store, server.alias()).unwrap();
    let mut connections = Vec::new();
    for root in ["~/ssh-root", "~/slurm-root"] {
        let mut cfg = config(&server);
        cfg.root = root.into();
        let conn = HostConn::start_shared(
            cfg,
            Store::open_at(server.directory().join("store")).unwrap(),
            clock.clone(),
            auth.clone(),
        )
        .unwrap();
        status(&conn, Status::Live).await;
        connections.push(conn);
    }
    rusqlite::Connection::open(server.directory().join("store/orx.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_insert BEFORE INSERT ON engine_host_auth
               BEGIN SELECT RAISE(FAIL, 'injected write failure'); END;
             CREATE TRIGGER fail_update BEFORE UPDATE ON engine_host_auth
               BEGIN SELECT RAISE(FAIL, 'injected write failure'); END;",
        )
        .unwrap();
    fs::write(server.directory().join("authorized_keys"), "").unwrap();
    let before = auth_attempts(&server).values().sum::<usize>();
    server.proxy().reset().unwrap();
    for conn in &connections {
        status(conn, Status::Backoff).await;
    }
    clock.advance(6001);
    for conn in &connections {
        status(conn, Status::NeedsAuth).await;
    }
    assert!(store
        .engine_host_auth(server.alias())
        .unwrap()
        .blocked
        .is_none());
    let blocked = auth_attempts(&server);
    clock.advance(30 * 60 * 1000);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(auth_attempts(&server), blocked);
    assert_eq!(blocked.values().sum::<usize>(), before);
}

/// Pins and log reads wait in the upload and log lanes, so a host slow to answer them still
/// hears a cancel at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn slow_pins_and_log_reads_cannot_delay_a_cancel() {
    let server = Sshd::start().unwrap();
    let command = server.directory().join("command");
    let text = fs::read_to_string(&command).unwrap();
    fs::write(
        &command,
        text.replace("export HOME", "export HOME\nexport ORX_TEST_SLOW_RPC=3000"),
    )
    .unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
    let mut slow = Vec::new();
    for index in 0..30 {
        let pin = conn.clone();
        slow.push(tokio::spawn(async move {
            let _ = pin
                .request(Request::PinSource {
                    run: format!("pin-{index}"),
                    sha: "0".repeat(64),
                })
                .await;
        }));
        let conn = conn.clone();
        slow.push(tokio::spawn(async move {
            let _ = conn
                .request(Request::ReadLog {
                    target: orx_host_proto::LogTarget::Run(format!("read-{index}")),
                    file: orx_host_proto::LogFile::Log,
                    ident: None,
                    range: orx_host_proto::ByteRange { start: 0, end: 1 },
                    anchor: None,
                })
                .await;
        }));
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let asked = Instant::now();
    conn.request(Request::Cancel { run: "r1".into() })
        .await
        .unwrap();
    assert!(
        asked.elapsed() < Duration::from_millis(1500),
        "the cancel waited {:?} behind slow requests",
        asked.elapsed()
    );
    for task in slow {
        task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bashrc_banner_before_bootstrap_is_ignored() {
    let server = Sshd::start().unwrap();
    fs::write(
        server.home().join(".bashrc"),
        "printf 'Welcome to the cluster\\nLoading modules\\n'\n",
    )
    .unwrap();
    let path = server.directory().join("command");
    let original = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        original.replace("export HOME", "export HOME\n. \"$HOME/.bashrc\""),
    )
    .unwrap();
    let conn = start(&server, TestClock::new());
    status(&conn, Status::Live).await;
    assert_eq!(alias_auth(&server).class, Some(HostClass::Key));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_actor_recovers_after_store_writes_fail() {
    for table in ["engine_hosts", "engine_host_auth"] {
        let server = Sshd::start().unwrap();
        let clock = TestClock::new();
        let conn = start(&server, clock.clone());
        status(&conn, Status::Live).await;
        let db = rusqlite::Connection::open(server.directory().join("store/orx.db")).unwrap();
        db.execute_batch(&format!(
            "CREATE TRIGGER fail_write BEFORE INSERT ON {table}
            BEGIN SELECT RAISE(FAIL, 'injected write failure'); END;"
        ))
        .unwrap();
        let attempts = logged_attempts(&server, 2).await;
        server.proxy().reset().unwrap();
        status(&conn, Status::Backoff).await;
        clock.advance(6001);
        status(&conn, Status::NeedsAuth).await;
        assert!(!conn.actions.is_closed());
        assert!(conn.retry().await.is_err());
        assert_eq!(auth_attempts(&server), attempts);
        db.execute_batch("DROP TRIGGER fail_write").unwrap();
        conn.retry().await.unwrap();
        status(&conn, Status::Live).await;
        assert!(alias_auth(&server).blocked.is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_breaker_clear_stays_blocked_in_memory() {
    let server = Sshd::start().unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let auth = Auth::new(&store, server.alias()).unwrap();
    auth.block(&store, server.alias(), 1, "needs Connect".into())
        .unwrap();
    let db = rusqlite::Connection::open(server.directory().join("store/orx.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_clear BEFORE INSERT ON engine_host_auth
        WHEN json_extract(NEW.state_json, '$.blocked') IS NULL BEGIN SELECT RAISE(FAIL, 'injected write failure'); END;",
    )
    .unwrap();
    assert!(auth
        .update(&store, server.alias(), |state| state.blocked = None)
        .is_err());
    assert!(auth.state.borrow().blocked.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn breaker_opened_during_route_resolution_prevents_login() {
    let server = Sshd::start().unwrap();
    let entered = server.directory().join("route-entered");
    let release = server.directory().join("route-release");
    let script = server.directory().join("route-gate");
    fs::write(
        &script,
        format!(
            "touch {}; while [ ! -e {} ]; do sleep 0.01; done\n",
            entered.display(),
            release.display()
        ),
    )
    .unwrap();
    let original = fs::read_to_string(server.config_path()).unwrap();
    fs::write(
        server.config_path(),
        format!("Match exec \"/bin/sh {}\"\n{original}", script.display()),
    )
    .unwrap();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let auth = Auth::new(&store, server.alias()).unwrap();
    let conn = HostConn::start_shared(
        config(&server),
        Store::open_at(server.directory().join("store")).unwrap(),
        TestClock::new(),
        auth.clone(),
    )
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    auth.block(&store, server.alias(), 1, "Peer needs Connect".into())
        .unwrap();
    fs::write(release, "").unwrap();
    status(&conn, Status::NeedsAuth).await;
    assert!(conn.retry_key().await.is_err());
    assert!(auth_attempts(&server).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn engine_sharing_reuses_mfa_without_authentication() {
    use std::os::unix::fs::PermissionsExt;

    if !crate::config::in_test_child(&["CLAUDE_CONFIG_DIR", "CODEX_HOME"]) {
        return;
    }
    // Sharing derives the ordinary socket dir from `$HOME/.config`, so `$HOME` must hold the config dir.
    let home = crate::config::test_home();
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let shim = bin.join("ssh");
    fs::write(&shim, r#"#!/bin/sh
if [ -f "$HOME/.ssh/config" ]; then
    config="$HOME/.ssh/probe-$$"
    sed "s|^Include openresearch_config$|Include $HOME/.ssh/openresearch_config|" "$HOME/.ssh/config" > "$config"
    exec /usr/bin/ssh -F "$config" "$@"
else
    exec /usr/bin/ssh -F /dev/null "$@"
fi
"#).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    std::env::set_var("PATH", path);
    std::env::set_var("HOME", &home);

    let passphrase = "orx sharing passphrase";
    let server = SshdBuilder {
        authentication_methods: "publickey,publickey".into(),
        extra_client_keys: vec![Some(passphrase.into())],
        identity_files: vec!["client_key".into(), "client_key_1".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    fs::create_dir_all(home.join(".ssh")).unwrap();
    let user_config = fs::read_to_string(server.config_path())
        .unwrap()
        .lines()
        .filter(|line| !line.trim_start().starts_with("ControlPath "))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(home.join(".ssh/config"), &user_config).unwrap();
    let clock = TestClock::new();
    let store = Store::open_at(server.directory().join("store")).unwrap();
    let mut cfg = config(&server);
    cfg.sharing_home = Some(home.clone());
    let auth = Auth::new(&store, &cfg.alias).unwrap();
    let conn = HostConn::start_shared(cfg, store, clock.clone(), auth).unwrap();
    status(&conn, Status::NeedsAuth).await;
    let prompt: Prompt = Arc::new(move |_| Box::pin(async move { Ok(Some(passphrase.into())) }));
    conn.connect_interactive(prompt.clone(), false)
        .await
        .unwrap();
    status(&conn, Status::Live).await;
    let attempts = auth_attempts(&server);
    let registry = home.join(".ssh/openresearch_connections.json");
    assert!(fs::read_to_string(&registry).unwrap().contains("engine:"));
    let installed = fs::read_to_string(home.join(".ssh/config")).unwrap();
    assert_eq!(
        installed.matches("Include openresearch_config\n").count(),
        1
    );

    let plain = |scoped: bool| {
        let mut command = crate::process::tokio_command("ssh");
        command
            .args([
                "-oBatchMode=yes",
                "-oIdentityAgent=none",
                "-oIdentityFile=none",
                "-oIdentitiesOnly=yes",
                server.alias(),
                "true",
            ])
            .env_remove("SSH_AUTH_SOCK")
            .env("SSH_ASKPASS", "")
            .env("SSH_ASKPASS_REQUIRE", "never");
        if scoped {
            crate::jobs::ssh::prepare_sharing_env(&mut command);
        } else {
            command.env_remove("ORX_SSH_CONTROL_DIR");
        }
        command
    };
    for scoped in [true, false] {
        let output = plain(scoped).output().await.unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    assert_eq!(auth_attempts(&server), attempts);

    let pin = alias_auth(&server).route.unwrap();
    let path = ssh::socket(&config(&server), &pin).unwrap();
    let output = server
        .spawn(&["-S", path.to_str().unwrap(), "-O", "exit"], None)
        .unwrap()
        .wait(Duration::from_secs(5))
        .unwrap();
    assert!(output.status.success());
    status(&conn, Status::Backoff).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while fs::read_to_string(&registry).unwrap().contains("engine:") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let fallback = plain(true).output().await.unwrap();
    assert!(!fallback.status.success());
    assert!(String::from_utf8_lossy(&fallback.stderr).contains("Permission denied"));
    clock.advance(6001);
    status(&conn, Status::NeedsAuth).await;
    let before = auth_attempts(&server);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(auth_attempts(&server), before);
    assert_eq!(
        fs::read_to_string(home.join(".ssh/config")).unwrap(),
        installed
    );

    conn.connect_interactive(prompt, false).await.unwrap();
    let changed = installed.replace("IdentitiesOnly yes", "IdentitiesOnly no");
    fs::write(home.join(".ssh/config"), &changed).unwrap();
    let engine_config = fs::read_to_string(server.config_path())
        .unwrap()
        .replace("IdentitiesOnly yes", "IdentitiesOnly no");
    fs::write(server.config_path(), engine_config).unwrap();
    assert!(!plain(true).output().await.unwrap().status.success());
    unsafe {
        libc::kill(ssh_pids(&server)[0] as i32, libc::SIGKILL);
    }
    status(&conn, Status::Backoff).await;
    clock.advance(6001);
    status(&conn, Status::NeedsAuth).await;
    assert!(!fs::read_to_string(&registry).unwrap().contains("engine:"));
    assert_eq!(
        fs::read_to_string(home.join(".ssh/config")).unwrap(),
        changed
    );
    let mut cfg = config(&server);
    cfg.sharing_home = Some(home);
    ssh::stop_master(&cfg, &pin).await.unwrap();
}
