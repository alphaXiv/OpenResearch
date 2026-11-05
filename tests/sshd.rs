// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

use orx_test_support::{FaultProxy, Faults, Process, Sshd, SshdBuilder};
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::num::NonZeroU64;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

fn until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "condition timed out after {timeout:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn master(server: &Sshd) -> Process {
    let process = server.spawn(&["-M", "-N"], None).unwrap();
    until(Duration::from_secs(5), || {
        server
            .spawn(&["-O", "check"], None)
            .unwrap()
            .wait(Duration::from_secs(1))
            .unwrap()
            .status
            .success()
    });
    process
}

fn live_session(server: &Sshd, marker: &str, options: &[&str]) -> Process {
    let process = server
        .spawn(
            options,
            Some(&format!(
                "touch {marker}; while printf .; do sleep 0.1; done"
            )),
        )
        .unwrap();
    until(Duration::from_secs(5), || {
        server.home().join(marker).exists()
    });
    process
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

fn assert_auth_attempts(server: &Sshd, expected: &[(&str, &str, usize)]) {
    let expected = expected
        .iter()
        .map(|&(method, outcome, count)| ((method.to_owned(), outcome.to_owned()), count))
        .collect();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        let actual = server.auth_attempts().unwrap();
        if actual == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "auth attempts: {actual:?}, expected {expected:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn command_through_generated_alias() {
    let server = Sshd::start().unwrap();
    let mut command = Command::new("ssh");
    command
        .args(server.ssh_args())
        .arg("printf '%s\\n' \"$USER\"; pwd");
    let output = Process::spawn(&mut command)
        .unwrap()
        .wait(Duration::from_secs(5))
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n{}\n", server.user(), server.home().display())
    );
    assert!(server.port() > 0);
    assert_eq!(server.alias(), "orx-test");
}

#[test]
fn missing_managed_include_is_harmless_to_config_and_connect() {
    let server = Sshd::start().unwrap();
    let managed = server.directory().join("openresearch_config");
    fs::write(&managed, "# retired connection sharing\n").unwrap();
    let original = fs::read_to_string(server.config_path()).unwrap();
    fs::write(
        server.config_path(),
        format!("Include {}\n{original}", managed.display()),
    )
    .unwrap();
    fs::remove_file(&managed).unwrap();

    let mut command = Command::new("ssh");
    command.arg("-G").args(server.ssh_args());
    let output = Process::spawn(&mut command)
        .unwrap()
        .wait(Duration::from_secs(5))
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("hostname 127.0.0.1"));
    assert!(server.run("true").unwrap().status.success());
}

#[test]
fn remote_home_is_isolated_and_real_orx_is_untouched() {
    let server = Sshd::start().unwrap();
    let real_orx = server.real_home().join(".orx");
    let before = fs::metadata(&real_orx)
        .map(|m| (m.is_dir(), m.len(), m.modified().unwrap()))
        .ok();
    let marker = server.directory().file_name().unwrap().to_str().unwrap();
    let real_marker = real_orx.join(marker);
    assert!(!real_marker.exists());
    let output = server
        .run(&format!(
        "printf '%s' \"$HOME\"; mkdir -p \"$HOME/.orx\"; printf isolated >\"$HOME/.orx/{marker}\""
    ))
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        server.home().to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(server.home().join(".orx").join(marker)).unwrap(),
        "isolated"
    );
    assert!(!real_marker.exists());
    assert_eq!(
        before,
        fs::metadata(&real_orx)
            .map(|m| (m.is_dir(), m.len(), m.modified().unwrap()))
            .ok()
    );
}

#[test]
fn black_hole_is_detected_by_client_keepalives() {
    let server = Sshd::start().unwrap();
    let mut client = live_session(
        &server,
        "black-hole-ready",
        &[
            "-o",
            "ServerAliveInterval=1",
            "-o",
            "ServerAliveCountMax=2",
            "-o",
            "LogLevel=INFO",
        ],
    );
    let start = Instant::now();
    server
        .proxy()
        .set(Faults {
            black_hole: true,
            ..Faults::default()
        })
        .unwrap();
    let output = client.wait(Duration::from_secs(8)).unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Timeout, server"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(start.elapsed() < Duration::from_secs(8));
    server.proxy().set(Faults::default()).unwrap();
    assert!(server.run("true").unwrap().status.success());
}

#[test]
fn reset_disconnects_live_clients_and_allows_reconnect() {
    let server = Sshd::start().unwrap();
    let mut clients = [
        live_session(&server, "reset-a", &[]),
        live_session(&server, "reset-b", &[]),
    ];
    server.proxy().reset().unwrap();
    for client in &mut clients {
        assert!(!client
            .wait(Duration::from_secs(3))
            .unwrap()
            .status
            .success());
    }
    assert!(server.run("true").unwrap().status.success());
}

#[test]
fn latency_applies_to_an_established_ssh_connection() {
    let server = Sshd::start().unwrap();
    let _master = master(&server);
    let baseline = Instant::now();
    assert!(server.run("true").unwrap().status.success());
    let baseline = baseline.elapsed();
    server
        .proxy()
        .set(Faults {
            latency: Duration::from_millis(150),
            ..Faults::default()
        })
        .unwrap();
    let delayed = Instant::now();
    assert!(server.run("true").unwrap().status.success());
    let delayed = delayed.elapsed();
    assert!(
        delayed >= Duration::from_millis(280),
        "latency was only {delayed:?}"
    );
    assert!(
        delayed > baseline + Duration::from_millis(150),
        "baseline={baseline:?}, delayed={delayed:?}"
    );
    server.proxy().set(Faults::default()).unwrap();
    assert!(server.run("true").unwrap().status.success());
}

#[test]
fn max_sessions_is_enforced_over_a_control_master() {
    let server = SshdBuilder {
        max_sessions: 1,
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    let _master = master(&server);
    let mut first = live_session(&server, "session-ready", &["-o", "ProxyCommand=false"]);
    let output = server
        .spawn(&["-o", "ProxyCommand=false"], Some("true"))
        .unwrap()
        .wait(Duration::from_secs(3))
        .unwrap();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Session open refused"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    server.proxy().reset().unwrap();
    assert!(!first.wait(Duration::from_secs(3)).unwrap().status.success());
}

#[test]
fn panic_cleans_up_daemon_master_and_directory() {
    let mut recorded = None;
    let mut kept_master = None;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let server = Sshd::start().unwrap();
        let process = master(&server);
        recorded = Some((
            server.pid(),
            process.pid(),
            server.directory().to_path_buf(),
        ));
        // Keep the process guard outside the unwind: only Sshd's socket cleanup can stop it.
        kept_master = Some(process);
        panic!("intentional harness cleanup test");
    }));
    assert!(result.is_err());
    let (sshd_pid, master_pid, directory) = recorded.unwrap();
    assert!(!alive(sshd_pid), "sshd {sshd_pid} survived unwind");
    assert!(!directory.exists(), "temporary directory survived unwind");
    let output = kept_master
        .as_mut()
        .unwrap()
        .wait(Duration::from_secs(3))
        .unwrap();
    assert!(
        output.status.code().is_some(),
        "master was terminated by a signal"
    );
    assert!(!alive(master_pid), "master {master_pid} survived unwind");
}

#[test]
fn proxy_pause_bandwidth_and_refusal_are_reversible() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let proxy = FaultProxy::start(listener.local_addr().unwrap()).unwrap();
    let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, proxy.port())).unwrap();
    client
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    let (mut remote, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "proxy did not connect to target");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept proxy connection: {error}"),
        }
    };
    remote.set_nonblocking(false).unwrap();
    remote
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    proxy
        .set(Faults {
            paused: true,
            ..Faults::default()
        })
        .unwrap();
    client.write_all(b"paused").unwrap();
    remote.write_all(b"return").unwrap();
    let mut bytes = [0; 6];
    assert!(remote.read(&mut bytes).is_err());
    assert!(client.read(&mut bytes).is_err());
    proxy.set(Faults::default()).unwrap();
    remote.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"paused");
    client.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"return");

    proxy
        .set(Faults {
            bytes_per_second: NonZeroU64::new(4096),
            ..Faults::default()
        })
        .unwrap();
    remote
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let start = Instant::now();
    client.write_all(&[7; 4096]).unwrap();
    let mut payload = [0; 4096];
    remote.read_exact(&mut payload).unwrap();
    assert_eq!(payload, [7; 4096]);
    assert!(start.elapsed() >= Duration::from_millis(900));

    proxy
        .set(Faults {
            refuse_new: true,
            ..Faults::default()
        })
        .unwrap();
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, proxy.port()));
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err());
    client.write_all(b"alive!").unwrap();
    remote.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"alive!");
    proxy.set(Faults::default()).unwrap();
    assert!(TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok());
}

#[test]
fn invalid_server_configuration_fails_with_diagnostics() {
    let result = SshdBuilder {
        max_startups: "invalid".into(),
        ..SshdBuilder::default()
    }
    .start();
    let error = result.err().expect("invalid MaxStartups must fail");
    assert!(error.to_string().contains("MaxStartups"), "{error:#}");
}

/// A throughput floor holds only on an idle machine; run with `--ignored`.
#[test]
#[ignore = "benchmark"]
fn pass_through_upload_exceeds_fifty_megabytes_per_second() {
    const BYTES: u64 = 256 * 1024 * 1024;
    let server = Sshd::start().unwrap();
    let upload = server.directory().join("upload");
    fs::File::create(&upload).unwrap().set_len(BYTES).unwrap();
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            r#"exec ssh -o Compression=no "$@" 'cat > /dev/null' < "$ORX_SSH_UPLOAD""#,
            "upload",
        ])
        .args(server.ssh_args())
        .env("ORX_SSH_UPLOAD", upload);
    let start = Instant::now();
    let output = Process::spawn(&mut command)
        .unwrap()
        .wait(Duration::from_secs(20))
        .unwrap();
    let elapsed = start.elapsed();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let megabytes_per_second = BYTES as f64 / elapsed.as_secs_f64() / 1_000_000.0;
    println!("256 MiB SSH upload: {megabytes_per_second:.1} MB/s in {elapsed:.3?}");
    assert!(
        megabytes_per_second > 50.0,
        "pass-through was only {megabytes_per_second:.1} MB/s"
    );
}

#[test]
fn interactive_key_counts_one_batch_probe_and_askpass_unlocks_it() {
    let server = SshdBuilder {
        extra_client_keys: vec![Some("a 'quoted' passphrase $with `shell` syntax".into())],
        identity_files: vec!["client_key_1".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    assert!(server.auth_attempts().unwrap().is_empty());
    let output = server.run("true").unwrap();
    assert!(!output.status.success());
    assert_auth_attempts(&server, &[("publickey", "Postponed", 1)]);
    let attempts = server.auth_attempts().unwrap();
    thread::sleep(Duration::from_millis(100));
    assert_eq!(server.auth_attempts().unwrap(), attempts);

    let askpass = server
        .askpass("a 'quoted' passphrase $with `shell` syntax")
        .unwrap();
    let mut command = Command::new("ssh");
    command
        .args(["-o", "BatchMode=no"])
        .args(server.ssh_args())
        .arg("true")
        .env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "orx-test");
    let output = Process::spawn(&mut command)
        .unwrap()
        .wait(Duration::from_secs(3))
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prompts = fs::read_to_string(server.directory().join("askpass-prompts")).unwrap();
    assert_eq!(prompts.lines().count(), 1, "{prompts}");
    assert!(prompts.contains("client_key_1"), "{prompts}");
    assert_auth_attempts(
        &server,
        &[("publickey", "Postponed", 2), ("publickey", "Accepted", 1)],
    );
}

#[test]
fn mfa_requires_two_distinct_keys_and_a_passphrase() {
    let server = SshdBuilder {
        authentication_methods: "publickey,publickey".into(),
        extra_client_keys: vec![Some("second factor".into())],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    assert!(!server.run("true").unwrap().status.success());
    assert_auth_attempts(
        &server,
        &[("publickey", "Postponed", 1), ("publickey", "Partial", 1)],
    );
    let mut config = fs::read_to_string(server.config_path()).unwrap();
    config.push_str(&format!(
        "  IdentityFile {}\n",
        server.directory().join("client_key_1").display()
    ));
    fs::write(server.config_path(), config).unwrap();
    assert!(!server.run("true").unwrap().status.success());
    assert_auth_attempts(
        &server,
        &[("publickey", "Postponed", 3), ("publickey", "Partial", 2)],
    );

    let askpass = server.askpass("second factor").unwrap();
    let mut command = Command::new("ssh");
    command
        .args(["-o", "BatchMode=no"])
        .args(server.ssh_args())
        .arg("true")
        .env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", "orx-test");
    let output = Process::spawn(&mut command)
        .unwrap()
        .wait(Duration::from_secs(3))
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_auth_attempts(
        &server,
        &[
            ("publickey", "Postponed", 5),
            ("publickey", "Partial", 3),
            ("publickey", "Accepted", 1),
        ],
    );
    let prompts = fs::read_to_string(server.directory().join("askpass-prompts")).unwrap();
    assert_eq!(prompts.lines().count(), 1, "{prompts}");
    assert!(prompts.contains("client_key_1"), "{prompts}");
}

#[test]
fn clear_restores_pass_through_after_black_hole_and_reset() {
    let server = Sshd::start().unwrap();
    let mut session = live_session(&server, "before-black-hole", &[]);
    server
        .proxy()
        .set(Faults {
            black_hole: true,
            ..Faults::default()
        })
        .unwrap();
    server.proxy().reset().unwrap();
    assert!(!session
        .wait(Duration::from_secs(3))
        .unwrap()
        .status
        .success());
    let mut blocked = server.spawn(&[], Some("true")).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(blocked.wait(Duration::from_millis(50)).is_err());
    server.proxy().reset().unwrap();
    assert!(!blocked
        .wait(Duration::from_secs(3))
        .unwrap()
        .status
        .success());
    server.proxy().clear().unwrap();
    assert!(server.run("true").unwrap().status.success());
}

#[test]
fn rejected_key_is_counted_as_a_failed_publickey_attempt() {
    let server = SshdBuilder {
        identity_files: vec!["host_key".into()],
        ..SshdBuilder::default()
    }
    .start()
    .unwrap();
    assert!(!server.run("true").unwrap().status.success());
    assert_auth_attempts(&server, &[("publickey", "Failed", 1)]);
}
