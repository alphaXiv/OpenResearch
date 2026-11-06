// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

use anyhow::{bail, ensure, Context, Result};
use orx_test_support::{Faults, Process, Sshd, SshdBuilder};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::num::NonZeroU64;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

struct Fixture {
    server: Option<Process>,
    hosts: Vec<(&'static str, Sshd)>,
    root: PathBuf,
    client: reqwest::Client,
    url: String,
}

impl Fixture {
    fn control(&mut self, line: &str) -> Result<()> {
        let words: Vec<_> = line.split_whitespace().collect();
        match words.as_slice() {
            ["quit"] => {
                STOP.store(true, Ordering::Relaxed);
                Ok(())
            }
            ["blackhole", alias] => self.fault(
                alias,
                Faults {
                    black_hole: true,
                    ..Default::default()
                },
            ),
            ["restore", alias] => self.fault(alias, Faults::default()),
            ["restart-sshd", alias] => self
                .hosts
                .iter_mut()
                .find(|(name, _)| name == alias)
                .ok_or_else(|| anyhow::anyhow!("unknown host"))
                .and_then(|(_, host)| host.restart()),
            _ => Err(anyhow::anyhow!("unknown control: {}", line.trim())),
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_orx"));
        let root = self.root.as_path();
        cmd.env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("ORX_LOCAL_SESSION")
            .env_remove("ORX_UP_AUTH_TOKEN")
            .env_remove("ORX_UP_PORT")
            .env_remove("SSH_AUTH_SOCK")
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_DATA_HOME", root.join("xdg-data"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("ORX_DATA_DIR", root.join("data"))
            .env("ORX_SSH_CONFIG", root.join(".ssh/config"))
            .env(
                "ORX_HOST_BIN_DIR",
                std::env::var_os("ORX_FIXTURE_HOST_BIN_DIR").unwrap(),
            )
            .env("ORX_NO_UPDATE_CHECK", "1")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "UI fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "UI fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .arg("--no-telemetry");
        cmd
    }

    async fn request(&self, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{path}", self.url);
        let request = match body {
            Some(body) => self.client.post(url).json(&body),
            None => self.client.get(url),
        };
        let response = request.send().await?;
        let status = response.status();
        let value: Value = response.json().await?;
        ensure!(status.is_success(), "{path}: {status} {value}");
        Ok(value)
    }

    async fn wait(&self, path: &str, predicate: impl Fn(&Value) -> bool) -> Result<Value> {
        let deadline = Instant::now() + Duration::from_secs(180);
        let mut last = Value::Null;
        while Instant::now() < deadline && !STOP.load(Ordering::Relaxed) {
            if let Ok(value) = self.request(path, None).await {
                if predicate(&value) {
                    return Ok(value);
                }
                last = value;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!(
            "timed out waiting for {path}: {last}; server: {}",
            self.server.as_ref().unwrap().stderr()?
        );
    }

    async fn run(&self, project: &str, title: &str, command: &str, host: &str) -> Result<String> {
        let output = Process::spawn(self.command().args([
            "create-experiment",
            project,
            "--title",
            title,
            "--baseline",
            "--run-command",
            command,
        ]))?
        .wait(Duration::from_secs(30))?;
        ensure!(
            output.status.success(),
            "create experiment: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let experiments = self
            .request(&format!("/api/projects/{project}/experiments"), None)
            .await?;
        let experiment = experiments["experiments"]
            .as_array()
            .context("experiments array")?
            .iter()
            .find(|e| e["title"] == title)
            .context("created experiment")?;
        let run = self.request("/api/runs", Some(json!({"experimentId":experiment["id"],"backend":"ssh","host":host,"force":true}))).await?;
        let id = run["run"]["id"]
            .as_str()
            .or_else(|| run["id"].as_str())
            .context(format!("run id: {run}"))?
            .to_owned();
        println!("{title}: run={id}, experiment={}", experiment["id"]);
        Ok(id)
    }

    fn fault(&self, alias: &str, faults: Faults) -> Result<()> {
        let host = &self
            .hosts
            .iter()
            .find(|(name, _)| *name == alias)
            .context("unknown host")?
            .1;
        host.proxy().set(faults)?;
        host.proxy().reset()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.take();
        for (_, host) in &self.hosts {
            let runs = host.home().join(".orx/engine/runs");
            if let Ok(entries) = fs::read_dir(runs) {
                for entry in entries.flatten() {
                    if let Ok(bytes) = fs::read(entry.path().join("wrapper.json")) {
                        if let Ok(identity) =
                            serde_json::from_slice::<orx_host_proto::ProcessIdentity>(&bytes)
                        {
                            if orx_host::platform::identity(identity.pid).ok().as_ref()
                                == Some(&identity)
                            {
                                unsafe {
                                    libc::kill(-(identity.pid as i32), libc::SIGKILL);
                                }
                            }
                        }
                    }
                }
            }
        }
        let paths: Vec<PathBuf> = std::iter::once(self.root.as_path().to_path_buf())
            .chain(
                self.hosts
                    .iter()
                    .map(|(_, host)| host.directory().to_path_buf()),
            )
            .collect();
        self.hosts.clear();
        for path in &paths {
            if let Ok(output) = Command::new("pgrep")
                .args(["-f", &path.to_string_lossy()])
                .output()
            {
                for pid in String::from_utf8_lossy(&output.stdout).split_whitespace() {
                    if let Ok(pid) = pid.parse::<i32>() {
                        if pid != std::process::id() as i32 {
                            unsafe {
                                libc::kill(pid, libc::SIGKILL);
                            }
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        for path in &paths {
            if let Ok(output) = Command::new("pgrep")
                .args(["-af", &path.to_string_lossy()])
                .output()
            {
                eprintln!(
                    "cleanup pgrep {}: {}",
                    path.display(),
                    String::from_utf8_lossy(&output.stdout).trim()
                );
                if !std::thread::panicking() {
                    assert!(!output.status.success(), "fixture process survived cleanup");
                }
            }
        }
        let _ = fs::remove_dir_all(&self.root);
        eprintln!("fixture stopped; isolated homes and large logs removed");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live browser fixture; run tests/fixtures/engine-ui.sh and interrupt to stop"]
async fn live_engine_dashboard() -> Result<()> {
    ensure!(
        std::env::var_os("ORX_FIXTURE_HOST_BIN_DIR").is_some(),
        "start with sh tests/fixtures/engine-ui.sh"
    );
    unsafe {
        libc::signal(libc::SIGINT, stop as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, stop as *const () as libc::sighandler_t);
    }
    let reservation = std::net::TcpListener::bind("127.0.0.1:4930")
        .context("fixture requires unused localhost port 4930")?;
    let root = PathBuf::from(format!("/tmp/orx-ui-fixture-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root)?;
    #[allow(clippy::disallowed_methods)]
    // Integration tests cannot access the binary's loopback client.
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(20))
        .build()?;
    let mut f = Fixture {
        server: None,
        hosts: vec![
            ("gpu-box", Sshd::start()?),
            ("lab-login", Sshd::start()?),
            ("expired-key", Sshd::start()?),
            (
                "mfa-box",
                SshdBuilder {
                    extra_client_keys: vec![Some("fixture-passphrase".into())],
                    identity_files: vec!["client_key_1".into()],
                    ..Default::default()
                }
                .start()?,
            ),
        ],
        root,
        client,
        url: "http://localhost:4930".into(),
    };
    fs::create_dir(f.root.as_path().join(".ssh"))?;
    fs::write(f.hosts[1].1.directory().join("authorized_keys"), "")?;
    let mut config = String::new();
    for (alias, host) in &f.hosts {
        config.push_str(
            &fs::read_to_string(host.config_path())?
                .replace("Host orx-test\n", &format!("Host {alias}\n")),
        );
    }
    fs::write(f.root.as_path().join(".ssh/config"), config)?;
    println!(
        "fixture pid={} home={}",
        std::process::id(),
        f.root.as_path().display()
    );
    for (alias, host) in &f.hosts {
        println!(
            "{alias}: sshd pid={} home={}",
            host.pid(),
            host.home().display()
        );
    }
    drop(reservation);
    f.server = Some(Process::spawn(f.command().args([
        "up",
        "--no-browser",
        "--port",
        "4930",
    ]))?);
    f.wait("/api/health", |_| true).await?;
    let repo = f.root.as_path().join("project");
    fs::create_dir(&repo)?;
    fs::write(repo.join("README.md"), "# Live engine dashboard fixture\n")?;
    let project = f.request("/api/projects", Some(json!({"name":"Engine live fixture","path":repo,"initializeGit":true,"githubSyncEnabled":false}))).await?;
    let project = project["project"]["id"]
        .as_str()
        .context("project id")?
        .to_owned();
    let steady = f
        .run(
            &project,
            "Steady output",
            "while :; do date; echo 'gpu-box training step'; sleep 1; done",
            "gpu-box",
        )
        .await?;
    f.wait(&format!("/api/runs/{steady}"), |r| {
        r["run"]["status"] == "running" || r["status"] == "running"
    })
    .await?;
    let done = f
        .run(
            &project,
            "Finished successfully",
            "echo 'training complete'",
            "gpu-box",
        )
        .await?;
    let failed = f
        .run(
            &project,
            "Finished with failure",
            "echo 'training failed' >&2; exit 7",
            "gpu-box",
        )
        .await?;
    let auth = f
        .run(
            &project,
            "Key rejected: NeedsAuth",
            "echo 'resumes after host authentication is fixed'",
            "lab-login",
        )
        .await?;
    let mfa = f
        .run(
            &project,
            "Interactive Connect",
            "while :; do echo 'mfa connected'; sleep 2; done",
            "mfa-box",
        )
        .await?;
    let expired = f
        .run(
            &project,
            "Established run: NeedsAuth",
            "while :; do echo 'key expired'; sleep 2; done",
            "expired-key",
        )
        .await?;
    f.wait(&format!("/api/runs/{expired}"), |r| {
        r["run"]["status"] == "running"
    })
    .await?;
    fs::write(f.hosts[2].1.directory().join("authorized_keys"), "")?;
    f.hosts[2].1.proxy().reset()?;
    let cancelled = f
        .run(
            &project,
            "Cancel pending during outage",
            "while :; do echo 'waiting for cancel'; sleep 1; done",
            "gpu-box",
        )
        .await?;
    f.wait(&format!("/api/projects/{project}/runs"), |v| {
        v["runs"].as_array().is_some_and(|r| {
            r.iter()
                .any(|r| r["id"] == cancelled && r["status"] == "running")
                && r.iter()
                    .any(|r| r["id"] == auth && r["contact"]["type"] == "needs_auth")
                && r.iter()
                    .any(|r| r["id"] == mfa && r["contact"]["type"] == "needs_auth")
                && r.iter()
                    .any(|r| r["id"] == expired && r["contact"]["type"] == "needs_auth")
                && r.iter().any(|r| r["id"] == done && r["status"] == "done")
                && r.iter()
                    .any(|r| r["id"] == failed && r["status"] == "failed")
        })
    })
    .await?;
    let large = f
        .run(
            &project,
            "Large log with host-only middle",
            "yes '0123456789 fixture training output' | head -c 1074790400 || [ \"$?\" = 141 ]",
            "gpu-box",
        )
        .await?;
    f.wait(&format!("/api/runs/{large}/log?start=0&end=16"), |p| {
        p["len"].as_u64().is_some_and(|n| n > 1 << 30)
            && p["kept"].as_array().is_some_and(|r| r.len() == 2)
    })
    .await?;
    let page = f
        .request(
            &format!("/api/runs/{large}/log?start=536870912&end=536870928"),
            None,
        )
        .await?;
    ensure!(
        page["kind"] == "data" && page["start"] == 536870912u64,
        "host middle page: {page}"
    );
    f.hosts[0].1.proxy().set(Faults {
        bytes_per_second: NonZeroU64::new(128 * 1024),
        ..Default::default()
    })?;
    let pending = f
        .run(
            &project,
            "Final log collection pending",
            "yes 'final collection output' | head -c 8388608 || [ \"$?\" = 141 ]",
            "gpu-box",
        )
        .await?;
    f.wait(&format!("/api/projects/{project}/runs"), |v| {
        v["runs"].as_array().is_some_and(|rs| {
            rs.iter().any(|r| {
                r["id"] == pending && r["status"] == "done" && r["logCollection"] == "pending"
            })
        })
    })
    .await?;
    f.fault(
        "gpu-box",
        Faults {
            black_hole: true,
            ..Default::default()
        },
    )?;
    f.request(&format!("/api/runs/{cancelled}/cancel"), Some(json!({})))
        .await?;
    let runs = f
        .wait(&format!("/api/projects/{project}/runs"), |v| {
            v["runs"].as_array().is_some_and(|rs| {
                rs.iter().any(|r| {
                    r["id"] == cancelled
                        && r["cancelling"] == true
                        && r["contact"]["type"] == "unreachable"
                })
            })
        })
        .await?;
    let hosts = f
        .wait("/api/compute/hosts", |v| {
            v.as_array().is_some_and(|hs| {
                hs.iter()
                    .any(|h| h["alias"] == "gpu-box" && h["status"] == "backoff")
                    && hs
                        .iter()
                        .any(|h| h["alias"] == "lab-login" && h["status"] == "needs_auth")
            })
        })
        .await?;
    let unavailable = f
        .client
        .get(format!(
            "{}/api/runs/{large}/log?start=629145600&end=629145616",
            f.url
        ))
        .send()
        .await?;
    ensure!(
        unavailable.status() == 503,
        "middle page outage: {}",
        unavailable.status()
    );
    ensure!(unavailable.json::<Value>().await?["contact"] == "unreachable");
    let fifo = f.root.as_path().join("controls");
    let cpath = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes())?;
    ensure!(
        unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) } == 0,
        "create control FIFO"
    );
    let mut control = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)?;
    let disk = Command::new("du")
        .arg("-sk")
        .arg(f.root.as_path())
        .args(f.hosts.iter().map(|(_, h)| h.directory()))
        .output()?;
    let report = json!({"url":f.url,"project":project,"runs":{"steady":steady,"done":done,"failed":failed,"large":large,"needsAuth":expired,"firstContactNeedsAuth":auth,"connect":mfa,"cancelPending":cancelled,"collectionPending":pending},"hosts":hosts,"snapshot":runs,"fifo":fifo,"diskKiB":String::from_utf8_lossy(&disk.stdout)});
    fs::write(
        f.root.as_path().join("ready.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("READY {} project={project}\ncontrols: echo 'restore gpu-box' > {}\nblackhole gpu-box | restore gpu-box | restart-sshd gpu-box | quit\nmfa-box Connect passphrase: fixture-passphrase\nreport: {}\ndisk KiB:\n{}",f.url,fifo.display(),f.root.as_path().join("ready.json").display(),String::from_utf8_lossy(&disk.stdout));
    let mut buffer = String::new();
    while !STOP.load(Ordering::Relaxed) {
        match control.read_to_string(&mut buffer) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.into()),
        }
        while let Some(end) = buffer.find('\n') {
            let line = buffer.drain(..=end).collect::<String>();
            let result = f.control(&line);
            println!("control {}: {result:?}", line.trim());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[allow(clippy::disallowed_methods)]
fn empty_fixture() -> Fixture {
    let root = PathBuf::from(format!(
        "/tmp/orx-ui-fixture-check-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&root).unwrap();
    Fixture {
        server: None,
        hosts: Vec::new(),
        root,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        url: "http://localhost:4930".into(),
    }
}

#[test]
fn unknown_fixture_control_keeps_the_fixture_alive() {
    let mut fixture = empty_fixture();
    assert_eq!(
        fixture
            .control("restart-sshd gpu-bxo")
            .unwrap_err()
            .to_string(),
        "unknown host"
    );
    assert!(fixture.root.exists());
    assert!(!STOP.load(Ordering::Relaxed));
}

#[test]
fn fixture_cleanup_does_not_abort_during_unwind() {
    if std::env::var_os("ORX_FIXTURE_UNWIND_PROBE").is_some() {
        let _fixture = empty_fixture();
        panic!("fixture unwind probe");
    }
    let fixture = empty_fixture();
    let bin = fixture.root.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("pgrep"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(bin.join("pgrep"), fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "fixture_cleanup_does_not_abort_during_unwind",
            "--nocapture",
        ])
        .env("ORX_FIXTURE_UNWIND_PROBE", "1")
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(101), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture stopped"));
}
