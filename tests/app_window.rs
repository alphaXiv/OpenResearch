// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

//! `orx up --app-window`, as a desktop window starts and controls it (src/commands/app/FROZEN.md).

#[path = "common/home.rs"]
mod home;

use serde_json::{json, Value};
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::os::fd::AsRawFd as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Window {
    root: PathBuf,
    server: Child,
    lease: Option<ChildStdin>,
    /// Copies the server's stderr, a pipe as a window's is, into `log` until told to stop.
    reader: Option<(Arc<AtomicBool>, JoinHandle<()>)>,
    log: PathBuf,
    url: String,
    client: reqwest::Client,
}

impl Window {
    /// Starts the server the way a window does, holding its stdin, and waits for it to answer.
    async fn open() -> Self {
        Self::open_from(Path::new(env!("CARGO_BIN_EXE_orx"))).await
    }

    async fn open_from(exe: &Path) -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let window = Self::start(exe, port);
        window.state().await;
        window
    }

    fn start(exe: &Path, port: u16) -> Self {
        let root = std::env::temp_dir().join(format!("orx-app-window-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("config/openresearch")).unwrap();
        fs::create_dir_all(root.join(".ssh")).unwrap();
        fs::write(root.join(".ssh/config"), "").unwrap();
        let log = root.join("server.log");
        let mut command = Command::new(exe);
        home::isolate_home(&mut command, &root)
            .env_remove("ORX_LOCAL_SESSION")
            .env_remove("ORX_UP_PORT")
            .env_remove("ORX_UP_AUTH_TOKEN")
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("xdg-data"))
            .env("ORX_DATA_DIR", root.join("data"))
            .env("ORX_CACHE_DIR", root.join("cache"))
            .env("ORX_SSH_CONFIG", root.join(".ssh/config"))
            .env("ORX_HOST_BIN_DIR", orx_test_support::host_binaries())
            .env("ORX_NO_UPDATE_CHECK", "1")
            .args(["--no-telemetry", "up", "--app-window", "--port"])
            .arg(port.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut server = command.spawn().unwrap();
        let lease = server.stdin.take();
        let reader = server.stderr.take().map(|stderr| read_stderr(stderr, &log));
        Self {
            root,
            server,
            lease,
            reader,
            log,
            url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// `GET /api/app-window`, waiting out a server that is starting or restarting.
    async fn state(&self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Ok(response) = self
                .client
                .get(format!("{}/api/app-window", self.url))
                .send()
                .await
            {
                if let Ok(state) = response.json::<Value>().await {
                    return state;
                }
            }
            assert!(Instant::now() < deadline, "{}", self.log());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn instance(&self) -> Value {
        let status: Value = self
            .client
            .get(format!("{}/api/update", self.url))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        status["instance"].clone()
    }

    fn exited_within(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.server.try_wait().unwrap().is_some() {
                self.stop_reading();
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Reads what the server has written so far, then closes the window's end of its stderr.
    fn stop_reading(&mut self) {
        if let Some((stop, reader)) = self.reader.take() {
            stop.store(true, Ordering::SeqCst);
            reader.join().unwrap();
        }
    }

    /// The window goes away, closing its end of the server's stderr along with the lease.
    fn close(&mut self) {
        self.stop_reading();
        drop(self.lease.take());
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        let _ = self.server.kill();
        let _ = self.server.wait();
        self.stop_reading();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn read_stderr(stderr: ChildStderr, log: &Path) -> (Arc<AtomicBool>, JoinHandle<()>) {
    // Nonblocking, so the reader can stop while the server writes nothing.
    // SAFETY: fcntl on a descriptor `stderr` owns.
    unsafe { libc::fcntl(stderr.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) };
    let mut log = fs::File::create(log).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let reader = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut buffer = [0; 4096];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => log.write_all(&buffer[..read]).unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if stopping.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("reading the server's stderr: {error}"),
            }
        }
    });
    (stop, reader)
}

fn config(root: &Path) -> PathBuf {
    root.join("config/openresearch")
}

#[tokio::test]
async fn a_window_that_goes_away_leaves_its_server_to_hand_off_even_after_an_update() {
    let mut window = Window::open().await;
    assert_eq!(window.state().await, json!({ "busy": false }));
    let pid = window.server.id();
    let before = window.instance().await;

    fs::write(config(&window.root).join("update-check-unknown.json"), json!({"checked_at":0,"latest":"0.2.99","installed_version":"0.2.99","installed_tag":"v0.2.99"}).to_string()).unwrap();
    let response = window
        .client
        .post(format!("{}/api/update/restart", window.url))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", window.log());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        window.state().await;
        if window.instance().await != before {
            break;
        }
        assert!(Instant::now() < deadline, "{}", window.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        window.server.id(),
        pid,
        "the restart kept the process the window waits on"
    );
    assert!(window.server.try_wait().unwrap().is_none());

    window.close();
    assert!(
        window.exited_within(Duration::from_secs(30)),
        "{}",
        window.log()
    );
    let status = window.server.try_wait().unwrap().unwrap();
    assert!(status.success(), "{status}\n{}", window.log());
}

#[tokio::test]
async fn a_quit_stops_the_server_rather_than_handing_off() {
    let mut window = Window::open().await;
    let response = window
        .client
        .post(format!("{}/api/app-window/quit", window.url))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT);
    assert!(
        window.exited_within(Duration::from_secs(30)),
        "{}",
        window.log()
    );
    assert!(
        window.log().contains("orx up: shutting down"),
        "{}",
        window.log()
    );
    assert!(!window.log().contains("the app window closed"));
}

#[tokio::test]
async fn a_server_that_cannot_start_says_why_last_for_its_window_to_show() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let mut window = Window::start(Path::new(env!("CARGO_BIN_EXE_orx")), port);
    assert!(
        window.exited_within(Duration::from_secs(60)),
        "{}",
        window.log()
    );
    assert_eq!(window.server.try_wait().unwrap().unwrap().code(), Some(1));
    let log = window.log();
    let last = log.lines().rev().find(|line| !line.trim().is_empty());
    assert!(
        last.is_some_and(|last| last.contains(&format!("port {port}"))),
        "{log}"
    );
}

#[tokio::test]
async fn a_restart_whose_successor_cannot_start_says_why_last_for_its_window_to_show() {
    let bin = std::env::temp_dir().join(format!("orx-app-window-bin-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("orx");
    fs::hard_link(env!("CARGO_BIN_EXE_orx"), &exe)
        .or_else(|_| fs::copy(env!("CARGO_BIN_EXE_orx"), &exe).map(drop))
        .unwrap();
    let mut window = Window::open_from(&exe).await;
    fs::remove_dir_all(&bin).unwrap();

    fs::write(config(&window.root).join("update-check-unknown.json"), json!({"checked_at":0,"latest":"0.2.99","installed_version":"0.2.99","installed_tag":"v0.2.99"}).to_string()).unwrap();
    let response = window
        .client
        .post(format!("{}/api/update/restart", window.url))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success(), "{}", window.log());
    assert!(
        window.exited_within(Duration::from_secs(60)),
        "{}",
        window.log()
    );
    assert_eq!(window.server.try_wait().unwrap().unwrap().code(), Some(1));
    let log = window.log();
    let last = log.lines().rev().find(|line| !line.trim().is_empty());
    assert!(
        last.is_some_and(|last| last.contains("could not restart")),
        "{log}"
    );
}
