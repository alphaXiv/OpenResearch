use super::*;
use crate::engine::runtime::chaos::rng::Rng;

fn stub(bytes: &[u8]) -> Session {
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut child = crate::process::tokio_command("python3")
        .args(["-c", "import os,sys; os.write(1,bytes.fromhex(sys.argv[1])); os.close(1); sys.stdin.buffer.read()", &hex])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
    let output: Box<dyn AsyncRead + Send + Unpin> = Box::new(child.stdout.take().unwrap());
    Session {
        input: Box::new(child.stdin.take().unwrap()),
        output: Some(BufReader::new(output)),
        child: Some(child),
        #[cfg(windows)]
        held: None,
        stderr: None,
        stderr_tail: Arc::new(Mutex::new(Vec::new())),
        bootstrapped: false,
        authenticated: Arc::new(AtomicBool::new(false)),
    }
}

#[tokio::test]
async fn seeded_bootstrap_stub_responses_are_bounded_and_validated() {
    let seed = 0x7b6c9d189f049321;
    let mut rng = Rng(seed);
    for case in 0..256 {
        let bytes = match rng.next() % 4 {
            0 => format!("ORX1 OS-{} arch-{}\n", rng.next(), rng.next()).into_bytes(),
            1 => b"ORX1 Linux x86_64 extra\n".to_vec(),
            2 => vec![b'x'; 300],
            _ => (0..rng.next() % 600).map(|_| rng.next() as u8).collect(),
        };
        let expected = bytes
            .split_inclusive(|&byte| byte == b'\n')
            .find(|line| line.starts_with(b"ORX1 "))
            .filter(|line| line.ends_with(b"\n"))
            .and_then(|line| std::str::from_utf8(line).ok())
            .and_then(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                (fields.len() == 3).then(|| (fields[1].to_owned(), fields[2].to_owned()))
            });
        let mut session = stub(&bytes);
        let actual = tokio::time::timeout(Duration::from_secs(2), session.handshake())
            .await
            .expect("handshake hung")
            .ok();
        assert_eq!(actual, expected, "seed={seed} case={case}");
        session.close().await;

        let binary = Binary {
            sha: "a".repeat(64),
            bytes: vec![1, 2, 3],
        };
        let mode = rng.next() % 8;
        let mut output = match mode {
            0 => b"ORX1 missing\nORX1 ready\n".to_vec(),
            1..=3 => b"ORX1 ready\n".to_vec(),
            4 => b"ORX1 ready \n".to_vec(),
            5 => vec![b'R'; 300],
            _ => (0..rng.next() % 600).map(|_| rng.next() as u8).collect(),
        };
        orx_host_proto::write_frame(
            &mut output,
            &Message {
                id: 0,
                body: Response::Hello {
                    proto: if mode == 2 {
                        PROTOCOL_VERSION + 1
                    } else {
                        PROTOCOL_VERSION
                    },
                    binary_sha: if mode == 3 {
                        "wrong".into()
                    } else {
                        binary.sha.clone()
                    },
                    boot_id: "boot".into(),
                    os: "Linux".into(),
                    arch: "x86_64".into(),
                    kernel: "test".into(),
                },
            },
        )
        .unwrap();
        let mut session = stub(&output);
        let actual = tokio::time::timeout(Duration::from_secs(2), session.bootstrap(&binary))
            .await
            .expect("bootstrap hung");
        assert_eq!(
            actual.is_ok(),
            mode <= 1,
            "seed={seed} case={case} mode={mode}: {actual:?}"
        );
        session.close().await;
    }
}

#[tokio::test]
async fn a_held_login_outlives_an_orx_host_update_but_not_a_protocol_change() {
    for (proto, resumes) in [(PROTOCOL_VERSION, true), (PROTOCOL_VERSION + 1, false)] {
        let mut hello = Vec::new();
        let body = Response::Hello {
            proto,
            binary_sha: "an older orx-host".into(),
            boot_id: "boot".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            kernel: "test".into(),
        };
        orx_host_proto::write_frame(&mut hello, &Message { id: 0, body }).unwrap();
        let mut session = stub(&hello);
        let resumed = session.resume().await;
        assert_eq!(resumed.is_ok(), resumes, "proto {proto}: {resumed:?}");
        if let Err(error) = resumed {
            assert!(error.to_string().contains("press Connect"), "{error}");
        }
        session.close().await;
    }
}

#[tokio::test]
async fn seeded_base64_pages_round_trip_through_async_frames() {
    let mut rng = Rng(0xef528d041a9bb673);
    for case in 0..256 {
        let bytes: Vec<_> = (0..rng.next() % 8192).map(|_| rng.next() as u8).collect();
        let page = orx_host_proto::LogPage::Data {
            len: None,
            ident: orx_host_proto::FileIdent {
                dev: rng.next(),
                ino: rng.next(),
            },
            start: rng.next(),
            bytes,
            eof: rng.next() & 1 == 0,
        };
        let message = Message {
            id: rng.next(),
            body: Response::ReadLog { page },
        };
        let json = serde_json::to_value(&message).unwrap();
        assert!(json["body"]["page"]["data"].is_string());
        assert!(json["body"]["page"]["ident"].is_string());
        let mut frame = Vec::new();
        orx_host_proto::write_frame(&mut frame, &message).unwrap();
        let actual = read(&mut frame.as_slice()).await.unwrap();
        assert_eq!(actual, message, "case={case}");
    }
}

#[tokio::test]
async fn bootstrap_banner_limits_count_lines_and_bytes() {
    let handshake = b"ORX1 Linux x86_64\n";
    for (banner, expected) in [
        (b"banner\n".repeat(63), true),
        (b"banner\n".repeat(64), false),
        (
            [vec![b'x'; 16 * 1024 - handshake.len() - 1], b"\n".to_vec()].concat(),
            true,
        ),
        ([vec![b'x'; 16 * 1024], b"\n".to_vec()].concat(), false),
        (b"\xff\xfe\n".to_vec(), true),
        (b"ORX1 invalid\n".to_vec(), false),
    ] {
        let mut session = stub(&[banner, handshake.to_vec()].concat());
        let result = tokio::time::timeout(Duration::from_secs(2), session.handshake())
            .await
            .unwrap();
        assert_eq!(result.is_ok(), expected, "{result:?}");
        assert_eq!(session.bootstrapped, expected);
        session.close().await;
    }
}

#[tokio::test]
async fn inner_proxy_jump_authentication_is_not_target_authentication() {
    let jump = orx_test_support::Sshd::start().unwrap();
    let target = orx_test_support::Sshd::start().unwrap();
    let target_config = std::fs::read_to_string(target.config_path())
        .unwrap()
        .replace("Host orx-test\n", "Host orx-test\n  ProxyJump orx-jump\n")
        .replace("HostName 127.0.0.1", "HostName localhost");
    let jump_config = std::fs::read_to_string(jump.config_path())
        .unwrap()
        .replace("Host orx-test\n", "Host orx-jump\n  LogLevel VERBOSE\n");
    std::fs::write(
        target.config_path(),
        format!("{target_config}{jump_config}"),
    )
    .unwrap();
    let config = Config {
        alias: target.alias().into(),
        root: "~/engine-root".into(),
        ssh_config: Some(target.config_path()),
        control_dir: target.directory().join("cm"),
        binary_dir: None,
        #[cfg(unix)]
        sharing_home: None,
        askpass_executable: "/unused".into(),
    };
    let route = route(&config).await.unwrap();
    let mut session = spawn(&config, false, &route, None).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), session.handshake())
        .await
        .unwrap()
        .unwrap_err();
    let failure = session.failure(error, false).await;
    assert!(failure.breaker);
    let tail = session.stderr_tail.lock().unwrap();
    let stderr = String::from_utf8_lossy(&tail);
    assert!(stderr.contains("Authenticated to 127.0.0.1 ("), "{stderr}");
    assert!(!session.authenticated.load(Ordering::Relaxed));
}

#[tokio::test]
async fn target_authentication_latch_survives_diagnostic_tail_eviction() {
    let server = orx_test_support::Sshd::start().unwrap();
    std::fs::write(server.directory().join("command"),
        "#!/bin/sh\nhead -c 20000 /dev/zero | tr '\\0' '\\n' >&2\nprintf '\\nremote directory unavailable\\n' >&2\nexit 98\n").unwrap();
    let config = Config {
        alias: server.alias().into(),
        root: "~/engine-root".into(),
        ssh_config: Some(server.config_path()),
        control_dir: server.directory().join("cm"),
        binary_dir: None,
        #[cfg(unix)]
        sharing_home: None,
        askpass_executable: "/unused".into(),
    };
    let route = route(&config).await.unwrap();
    let mut session = spawn(&config, false, &route, None).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), session.handshake())
        .await
        .unwrap()
        .unwrap_err();
    let failure = session.failure(error, false).await;
    assert!(session.authenticated.load(Ordering::Relaxed));
    assert!(!failure.breaker);
    let tail = session.stderr_tail.lock().unwrap();
    assert!(tail.len() <= 16 * 1024);
    let stderr = String::from_utf8_lossy(&tail);
    assert!(!stderr.contains("Authenticated to "));
    assert!(super::failure("SSH connection closed", &stderr, Some(98), false).breaker);
}
