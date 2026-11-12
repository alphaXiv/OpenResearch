// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]
mod common;
use common::{until, Directory, Group};
use orx_host::{legacy, log, platform, server::Host, sha};
use orx_host_proto::*;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn host(dir: &Directory) -> Host {
    Host::new(dir.0.clone(), env!("CARGO_BIN_EXE_orx-host").into()).unwrap()
}

fn log_info(path: &std::path::Path) -> LogInfo {
    let probe = ByteRange {
        start: u64::MAX - 1,
        end: u64::MAX,
    };
    let LogPage::Reset { info: Some(info) } = log::read(path, None, &probe, None).unwrap() else {
        panic!("no log at {}", path.display())
    };
    info
}

#[test]
fn random_log_pages_identity_anchor_and_short_eof() {
    let dir = Directory::new();
    let path = dir.0.join("log");
    let bytes: Vec<_> = (0..60_123).map(|n| (n % 251) as u8).collect();
    fs::write(&path, &bytes).unwrap();
    let info = log_info(&path);
    for n in 0..100 {
        let start = 64 + (n * 7357 % 60_059) as u64;
        let end = start + 1000;
        let anchor = bytes[start as usize - 64..start as usize]
            .try_into()
            .unwrap();
        let page = log::read(
            &path,
            Some(&info.ident),
            &ByteRange { start, end },
            Some(&anchor),
        )
        .unwrap();
        let LogPage::Data {
            bytes: actual, eof, ..
        } = page
        else {
            panic!("unexpected reset");
        };
        assert_eq!(
            actual,
            bytes[start as usize..end.min(bytes.len() as u64) as usize]
        );
        assert_eq!(eof, end >= bytes.len() as u64);
    }
    let bad_anchor = [0; 64];
    assert!(matches!(
        log::read(
            &path,
            Some(&info.ident),
            &ByteRange {
                start: 100,
                end: 200
            },
            Some(&bad_anchor)
        )
        .unwrap(),
        LogPage::Reset { .. }
    ));
    fs::write(dir.0.join("new"), &bytes).unwrap();
    fs::rename(dir.0.join("new"), &path).unwrap();
    assert!(matches!(
        log::read(
            &path,
            Some(&info.ident),
            &ByteRange {
                start: 100,
                end: 200
            },
            None
        )
        .unwrap(),
        LogPage::Reset { .. }
    ));
    let info = log_info(&path);
    fs::write(&path, b"short").unwrap();
    assert!(matches!(
        log::read(
            &path,
            Some(&info.ident),
            &ByteRange {
                start: 100,
                end: 200
            },
            None
        )
        .unwrap(),
        LogPage::Reset { .. }
    ));
    assert!(log::read(
        &path,
        None,
        &ByteRange {
            start: 0,
            end: MAX_PAGE_LEN + 1
        },
        None
    )
    .is_err());
}

#[test]
fn small_log_identity_survives_appends_from_empty() {
    let dir = Directory::new();
    let path = dir.0.join("log");
    fs::write(&path, b"").unwrap();
    let ident = log_info(&path).ident;
    for bytes in [b"first".as_slice(), b"second", &[3; 4096]] {
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        assert_eq!(log_info(&path).ident, ident);
    }
}

/// A watched run the engine has not seen yet, so whatever the host reports is a change.
fn unseen(run: &str) -> (RunId, Summary) {
    let summary = Summary {
        started_at: None,
        state: RunState::Unreconciled {
            detail: "unseen".into(),
        },
        log_len: None,
        notify_len: 0,
        cleanup: None,
    };
    (run.into(), summary)
}

#[test]
fn change_feed_isolates_unreadable_intent() {
    let dir = Directory::new();
    let host = host(&dir);
    fs::create_dir(dir.0.join("runs/bad")).unwrap();
    fs::write(dir.0.join("runs/bad/launch.json"), "{").unwrap();
    fs::create_dir(dir.0.join("runs/good")).unwrap();
    let Response::Changes { runs } = host.handle(Request::Changes {
        watch: vec![unseen("bad"), unseen("good")],
        follow_logs: vec![],
        wait_ms: 0,
    }) else {
        panic!()
    };
    assert!(matches!(
        runs.iter().find(|(run, _)| run == "bad").unwrap().1.state,
        RunState::InvalidData { .. }
    ));
    assert!(runs.iter().any(|(run, _)| run == "good"));
}

#[test]
fn starting_run_recovery_backs_off_and_preserves_the_staged_script() {
    let dir = Directory::new();
    let binary = dir.0.join("fake-host");
    orx_test_support::write_executable(&binary, "#!/bin/sh\nprintf x >> \"$2/attempts\"\n", 0o700)
        .unwrap();
    let host = Host::new(dir.0.clone(), binary).unwrap();
    let intent = LaunchIntent {
        run: "retry".into(),
        digest: sha(b"true\n"),
        run_sh: "true\n".into(),
        env: Default::default(),
        source: None,
        cancel_sh: None,
    };
    assert!(matches!(
        host.handle(Request::Launch(intent.clone())),
        Response::Launch { .. }
    ));
    let run = dir.0.join("runs/retry");
    until(Duration::from_secs(2), || {
        fs::read(run.join("attempts")).is_ok_and(|bytes| bytes == b"x")
    });
    let script = fs::metadata(run.join("run.sh"))
        .unwrap()
        .modified()
        .unwrap();
    for _ in 0..5 {
        assert!(matches!(
            host.handle(Request::Launch(intent.clone())),
            Response::Launch { .. }
        ));
    }
    assert_eq!(fs::read(run.join("attempts")).unwrap(), b"x");
    assert!(!run.join("started").exists());
    std::thread::sleep(Duration::from_millis(1050));
    host.handle(Request::Launch(intent));
    until(Duration::from_secs(2), || {
        fs::read(run.join("attempts")).is_ok_and(|bytes| bytes == b"xx")
    });
    assert_eq!(
        fs::metadata(run.join("run.sh"))
            .unwrap()
            .modified()
            .unwrap(),
        script
    );
}

#[test]
fn change_feed_returns_on_change_timeout_and_ignores_unfollowed_log_growth() {
    let dir = Directory::new();
    let host = Arc::new(host(&dir));
    let request = |watch| Request::Changes {
        watch,
        follow_logs: vec![],
        wait_ms: 0,
    };
    assert_eq!(
        host.handle(request(vec![])),
        Response::Changes { runs: vec![] }
    );
    fs::create_dir(dir.0.join("runs/r1")).unwrap();
    let Response::Changes { runs } = host.handle(request(vec![unseen("r1")])) else {
        panic!();
    };
    assert_eq!(runs[0].1.log_len, None);
    fs::write(dir.0.join("runs/r1/log"), b"hello").unwrap();
    let start = Instant::now();
    assert_eq!(
        host.handle(Request::Changes {
            watch: runs.clone(),
            follow_logs: vec![],
            wait_ms: 40
        }),
        Response::Changes { runs: vec![] }
    );
    assert!(start.elapsed() >= Duration::from_millis(35));
    let feed = {
        let host = host.clone();
        std::thread::spawn(move || {
            host.handle(Request::Changes {
                watch: runs,
                follow_logs: vec![],
                wait_ms: 2000,
            })
        })
    };
    std::thread::sleep(Duration::from_millis(50));
    fs::write(dir.0.join("runs/r1/notify"), b"done").unwrap();
    assert!(
        matches!(feed.join().unwrap(), Response::Changes { runs } if runs[0].1.notify_len == 4)
    );
}

#[test]
fn blob_retries_are_checked_and_only_verified_blobs_are_published() {
    let dir = Directory::new();
    let host = host(&dir);
    let bytes = b"a source archive";
    let hash = sha(bytes);
    assert_eq!(
        host.handle(Request::HasBlob {
            sha: hash.clone(),
            len: bytes.len() as u64
        }),
        Response::HasBlob { present: false }
    );
    for (offset, chunk) in [(0, &bytes[..5]), (0, &bytes[..5]), (5, &bytes[5..])] {
        assert!(matches!(
            host.handle(Request::PutBlob {
                sha: hash.clone(),
                offset,
                bytes: chunk.to_vec()
            }),
            Response::PutBlob { .. }
        ));
    }
    assert!(matches!(
        host.handle(Request::PutBlob {
            sha: hash.clone(),
            offset: 0,
            bytes: b"wrong".to_vec()
        }),
        Response::Error {
            code: ErrorCode::Conflict,
            ..
        }
    ));
    assert_eq!(
        host.handle(Request::HasBlob {
            sha: hash.clone(),
            len: 1
        }),
        Response::HasBlob { present: false }
    );
    assert!(!dir.0.join("sources").join(&hash).exists());
    assert_eq!(
        host.handle(Request::HasBlob {
            sha: hash.clone(),
            len: bytes.len() as u64
        }),
        Response::HasBlob { present: true }
    );
    assert_eq!(fs::read(dir.0.join("sources").join(&hash)).unwrap(), bytes);
    let wrong_hash = sha(b"different");
    host.handle(Request::PutBlob {
        sha: wrong_hash.clone(),
        offset: 0,
        bytes: bytes.to_vec(),
    });
    assert_eq!(
        host.handle(Request::HasBlob {
            sha: wrong_hash.clone(),
            len: bytes.len() as u64
        }),
        Response::HasBlob { present: false }
    );
    assert!(!dir.0.join("sources").join(wrong_hash).exists());
}

#[test]
fn a_staging_write_the_host_refuses_names_its_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = Directory::new();
    let host = host(&dir);
    let sources = dir.0.join("sources");
    fs::set_permissions(&sources, fs::Permissions::from_mode(0o500)).unwrap();
    let response = host.handle(Request::PutBlob {
        sha: sha(b"x"),
        offset: 0,
        bytes: b"x".to_vec(),
    });
    fs::set_permissions(&sources, fs::Permissions::from_mode(0o700)).unwrap();
    let Response::Error {
        code: ErrorCode::Io,
        message,
    } = response
    else {
        panic!("unexpected reply: {response:?}");
    };
    assert!(
        message.contains(&sources.display().to_string()),
        "{message}"
    );
}

#[test]
fn legacy_paths_are_validated_and_dir_pages_use_the_same_identity() {
    for path in ["/scratch/user/orx", "~/orx/runs/a", "$HOME/.orx/runs/a"] {
        assert!(legacy::directory(path).unwrap().is_absolute());
    }
    for path in [
        "",
        "relative/path",
        "/tmp/../etc",
        "~/../etc",
        "/tmp/a;whoami",
        "/tmp/$(id)",
        "/tmp/a b",
        "/tmp//a",
        "/tmp/a/",
    ] {
        assert!(legacy::directory(path).is_err(), "{path}");
    }
    let dir = Directory::new();
    let host = host(&dir);
    fs::write(dir.0.join("log"), b"legacy log").unwrap();
    let target = LogTarget::Dir(dir.0.to_str().unwrap().into());
    let request = |ident| Request::ReadLog {
        target: target.clone(),
        file: LogFile::Log,
        ident,
        range: ByteRange { start: 0, end: 100 },
        anchor: None,
    };
    let Response::ReadLog {
        page:
            LogPage::Data {
                ident,
                bytes,
                eof: true,
                ..
            },
    } = host.handle(request(None))
    else {
        panic!();
    };
    assert_eq!(bytes, b"legacy log");
    fs::write(dir.0.join("replacement"), b"changed").unwrap();
    fs::rename(dir.0.join("replacement"), dir.0.join("log")).unwrap();
    assert!(matches!(
        host.handle(request(Some(ident))),
        Response::ReadLog {
            page: LogPage::Reset { .. }
        }
    ));
    assert!(matches!(
        host.handle(Request::ReadLog {
            target: LogTarget::Dir("/tmp/../etc".into()),
            file: LogFile::Log,
            ident: None,
            range: ByteRange { start: 0, end: 1 },
            anchor: None,
        }),
        Response::Error {
            code: ErrorCode::InvalidRequest,
            ..
        }
    ));
}

#[test]
fn legacy_cancel_terminates_a_real_group_without_rewriting_receipts() {
    let dir = Directory::new();
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("touch ready; sleep 60 & wait")
        .current_dir(&dir.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let group = Group(command.spawn().unwrap());
    until(Duration::from_secs(3), || dir.0.join("ready").exists());
    fs::write(dir.0.join("pid"), group.0.id().to_string()).unwrap();
    fs::write(dir.0.join("exit_code"), b"").unwrap();
    let before: Vec<_> = fs::read_dir(&dir.0)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (path.clone(), fs::read(path).unwrap())
        })
        .collect();
    assert!(legacy::inspect(&dir.0).unwrap().receipt_unreadable);
    assert_eq!(
        legacy::cancel(&dir.0, None, None).unwrap(),
        LegacyCancelOutcome::GroupGone
    );
    for (path, bytes) in before {
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    assert!(!platform::process(group.0.id()).is_ok_and(|p| p.live()));
}

#[test]
fn hello_and_self_verification_match_the_binary() {
    let dir = Directory::new();
    let host = host(&dir);
    let hash = sha(&fs::read(env!("CARGO_BIN_EXE_orx-host")).unwrap());
    assert!(
        matches!(host.handle(Request::Hello { proto: PROTOCOL_VERSION }), Response::Hello { binary_sha, .. } if binary_sha == hash)
    );
    assert!(matches!(
        host.handle(Request::Hello { proto: 0 }),
        Response::Error {
            code: ErrorCode::Unsupported,
            ..
        }
    ));
    assert!(Command::new(env!("CARGO_BIN_EXE_orx-host"))
        .args(["verify-self", &hash])
        .status()
        .unwrap()
        .success());
    let output = Command::new(env!("CARGO_BIN_EXE_orx-host"))
        .args(["verify-self", "wrong"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn source_staging_resumes_and_launch_intent_cannot_be_replaced() {
    let dir = Directory::new();
    let source = Directory::new();
    fs::write(source.0.join("input"), b"source data").unwrap();
    let archive = Command::new("tar")
        .args(["-cf", "-", "-C"])
        .arg(&source.0)
        .arg("input")
        .output()
        .unwrap();
    assert!(archive.status.success());
    let hash = sha(&archive.stdout);
    let host = host(&dir);
    let script = "cat repo/input";
    let request = || {
        Request::Launch(LaunchIntent {
            run: "staged".into(),
            digest: sha(script.as_bytes()),
            run_sh: script.into(),
            env: Default::default(),
            source: Some(hash.clone()),
            cancel_sh: None,
        })
    };
    // An unpinned (or swept) source fails the launch, so the client stages it again.
    assert!(matches!(
        host.handle(request()),
        Response::Error {
            code: ErrorCode::NotFound,
            ..
        }
    ));
    assert!(!dir.0.join("runs/staged/started").exists());
    host.handle(Request::PutBlob {
        sha: hash.clone(),
        offset: 0,
        bytes: archive.stdout.clone(),
    });
    assert_eq!(
        host.handle(Request::HasBlob {
            sha: hash.clone(),
            len: archive.stdout.len() as u64
        }),
        Response::HasBlob { present: true }
    );
    assert_eq!(
        host.handle(Request::PinSource {
            run: "staged".into(),
            sha: hash.clone(),
        }),
        Response::PinSource { pinned: true }
    );
    assert!(matches!(host.handle(request()), Response::Launch { .. }));
    until(Duration::from_secs(5), || {
        dir.0.join("runs/staged/exit.json").exists()
    });
    assert_eq!(
        fs::read(dir.0.join("runs/staged/log")).unwrap(),
        b"source data"
    );
    assert!(matches!(
        host.handle(request()),
        Response::Launch {
            summary: Summary {
                started_at: Some(started_at),
                state: RunState::Exited { .. },
                ..
            }
        } if Some(started_at) == orx_host::wrapper::started_at(&dir.0.join("runs/staged"))
    ));
    assert!(matches!(
        host.handle(Request::Launch(LaunchIntent {
            run: "staged".into(),
            digest: sha(b"other"),
            run_sh: "exit 99".into(),
            env: Default::default(),
            source: Some(hash),
            cancel_sh: None,
        })),
        Response::Error {
            code: ErrorCode::Conflict,
            ..
        }
    ));
    assert_eq!(
        fs::read_to_string(dir.0.join("runs/staged/run.sh")).unwrap(),
        script
    );
}

#[test]
fn legacy_pid_files_cannot_overflow_into_a_different_signal_target() {
    let dir = Directory::new();
    for pid in ["0", "1", "4294967295", "not-a-pid"] {
        fs::write(dir.0.join("pid"), pid).unwrap();
        assert_eq!(legacy::inspect(&dir.0).unwrap().pid, None);
        assert_eq!(
            legacy::cancel(&dir.0, None, None).unwrap(),
            LegacyCancelOutcome::GroupGone
        );
    }
}

#[test]
fn legacy_inspection_keeps_a_zombie_leader_live_while_its_group_runs() {
    let dir = Directory::new();
    let child = Command::new("/bin/sh")
        .args(["-c", "sleep 60 & exit 0"])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let group = Group(child);
    let pid = group.0.id();
    fs::write(dir.0.join("pid"), pid.to_string()).unwrap();
    until(Duration::from_secs(3), || {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        assert_eq!(result, 0);
        unsafe { info.si_pid() != 0 }
    });
    assert!(legacy::inspect(&dir.0).unwrap().alive);
    assert_eq!(
        legacy::cancel(&dir.0, None, None).unwrap(),
        LegacyCancelOutcome::GroupGone
    );
    assert!(!legacy::inspect(&dir.0).unwrap().alive);
}

#[test]
fn receipt_read_errors_keep_their_type_and_recover() {
    let root = Directory::new();
    let host = host(&root);
    let dir = root.0.join("runs/errors");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("exit.json");
    for bytes in [b"{".as_slice(), br#"{"reason":"future"}"#] {
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            orx_host::wrapper::state(&dir).unwrap(),
            RunState::InvalidData { .. }
        ));
    }
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        orx_host::wrapper::state(&dir).unwrap(),
        RunState::Unreachable { .. }
    ));
    let Response::Changes { runs } = host.handle(Request::Changes {
        watch: vec![unseen("errors")],
        follow_logs: vec![],
        wait_ms: 0,
    }) else {
        panic!()
    };
    assert!(matches!(runs[0].1.state, RunState::Unreachable { .. }));
    fs::remove_dir(&path).unwrap();
    let receipt = ExitReceipt {
        finished_at: None,
        code: Some(0),
        signal: None,
        reason: ExitReason::Exited,
        processes: vec![],
        outcome: CancelOutcome::Verified,
        detail: None,
    };
    orx_host::publish(&path, &receipt).unwrap();
    assert_eq!(
        orx_host::wrapper::state(&dir).unwrap(),
        RunState::Exited { receipt }
    );
}

#[test]
fn matching_launch_key_adopts_the_accepted_intent_during_staging() {
    let root = Directory::new();
    let host = host(&root);
    let dir = root.0.join("runs/adopt");
    fs::create_dir(&dir).unwrap();
    let accepted = LaunchIntent {
        run: "adopt".into(),
        digest: sha(b"accepted"),
        run_sh: "printf '%s' \"$TOKEN\"".into(),
        env: [("TOKEN".into(), "accepted".into())].into(),
        source: None,
        cancel_sh: Some("true".into()),
    };
    orx_host::publish(&dir.join("launch.json"), &accepted).unwrap();
    let mut retry = accepted.clone();
    retry.run_sh = "touch duplicate".into();
    retry.env.insert("TOKEN".into(), "changed".into());
    retry.cancel_sh = Some("false".into());
    retry.source = Some(sha(b"different source"));
    assert!(matches!(
        host.handle(Request::Launch(retry)),
        Response::Launch { .. }
    ));
    until(Duration::from_secs(5), || dir.join("exit.json").exists());
    assert_eq!(fs::read(dir.join("log")).unwrap(), b"accepted");
    assert_eq!(
        orx_host::read_json::<LaunchIntent>(&dir.join("launch.json")).unwrap(),
        accepted
    );
    assert_eq!(fs::read_to_string(dir.join("cancel.sh")).unwrap(), "true");
    assert!(!dir.join("duplicate").exists());
}

#[test]
fn source_pinning_respects_cross_process_launch_ownership() {
    let root = Directory::new();
    let host = host(&root);
    let bytes = vec![42; 2 << 20];
    let sha = sha(&bytes);
    fs::write(root.0.join("sources").join(&sha), &bytes).unwrap();
    let dir = root.0.join("runs/pin-lock");
    fs::create_dir(&dir).unwrap();
    let mut owner = Group(Command::new("python3").arg("-c").arg(
        "import fcntl,pathlib,sys,time; d=pathlib.Path(sys.argv[1]); f=open(d/'launch.lock','w'); fcntl.lockf(f,fcntl.LOCK_EX); (d/'locked').touch();\nwhile not (d/'release').exists(): time.sleep(.01)"
    ).arg(&dir).process_group(0).spawn().unwrap());
    until(Duration::from_secs(3), || dir.join("locked").exists());
    let pin = || Request::PinSource {
        run: "pin-lock".into(),
        sha: sha.clone(),
    };
    assert!(matches!(
        host.handle(pin()),
        Response::Error {
            code: ErrorCode::Busy,
            ..
        }
    ));
    assert!(!dir.join("source.tar").exists());
    fs::write(dir.join("release"), "").unwrap();
    assert!(owner.0.wait().unwrap().success());
    assert_eq!(host.handle(pin()), Response::PinSource { pinned: true });
    assert_eq!(fs::read(dir.join("source.tar")).unwrap(), bytes);
}
