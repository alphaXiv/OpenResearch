// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]
mod common;
use common::until;
use orx_host::{platform, read_json, wrapper};
use orx_host_proto::*;
use orx_test_support::Sshd;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufReader;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Remote {
    server: Sshd,
    child: Child,
    input: Option<ChildStdin>,
    replies: Receiver<std::io::Result<Message<Response>>>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
}

impl Remote {
    fn new() -> Self {
        let server = Sshd::start().unwrap();
        let bin = server.home().join("bin");
        fs::create_dir(&bin).unwrap();
        let stderr = File::create(server.home().join("serve.stderr")).unwrap();
        let command = format!(
            "ORX_TEST_FAST_STOPS=1 PATH='{}':\"$PATH\" exec '{}' serve --root '{}'",
            bin.display(),
            env!("CARGO_BIN_EXE_orx-host"),
            server.home().join("host").display()
        );
        let mut child = Command::new("ssh")
            .args(server.ssh_args())
            .arg(command)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (tx, replies) = mpsc::channel();
        let reader = thread::spawn(move || loop {
            match read_frame::<Message<Response>>(&mut stdout) {
                Ok(Some(reply)) => {
                    if tx.send(Ok(reply)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = tx.send(Err(error));
                    break;
                }
            }
        });
        let mut remote = Self {
            server,
            child,
            input,
            replies,
            reader: Some(reader),
            next_id: 1,
        };
        assert!(matches!(
            remote.request(Request::Hello {
                proto: PROTOCOL_VERSION
            }),
            Response::Hello { .. }
        ));
        remote
    }
    fn root(&self) -> PathBuf {
        self.server.home().join("host")
    }
    fn send(&mut self, body: Request) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        write_frame(self.input.as_mut().unwrap(), &Message { id, body }).unwrap();
        id
    }
    fn receive(&self) -> Message<Response> {
        self.replies
            .recv_timeout(Duration::from_secs(40))
            .unwrap_or_else(|error| {
                panic!(
                    "{error}: {}",
                    fs::read_to_string(self.server.home().join("serve.stderr")).unwrap()
                )
            })
            .unwrap()
    }
    fn request(&mut self, body: Request) -> Response {
        let id = self.send(body);
        let reply = self.receive();
        assert_eq!(reply.id, id);
        assert!(
            !matches!(reply.body, Response::Error { .. }),
            "{:?}",
            reply.body
        );
        reply.body
    }
    fn shim(&self, name: &str, script: &str) {
        let path = self.server.home().join("bin").join(name);
        orx_test_support::write_executable(path, format!("#!/bin/sh\n{script}\n"), 0o700).unwrap();
    }
    fn launch(&mut self, run: &str, script: &str) -> Summary {
        let Response::Launch { summary } = self.request(Request::Launch(LaunchIntent {
            run: run.into(),
            digest: orx_host::sha(script.as_bytes()),
            run_sh: script.into(),
            env: BTreeMap::new(),
            source: None,
            cancel_sh: None,
        })) else {
            panic!();
        };
        summary
    }
    fn finish(&mut self, run: &str, mut summary: Summary) -> ExitReceipt {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let RunState::Exited { receipt } = summary.state {
                return receipt;
            }
            assert!(Instant::now() < deadline, "{summary:?}");
            let Response::Changes { runs } = self.request(Request::Changes {
                watch: vec![(run.into(), summary.clone())],
                follow_logs: vec![run.into()],
                wait_ms: 2000,
            }) else {
                panic!();
            };
            if let Some((_, changed)) = runs.into_iter().next() {
                summary = changed;
            }
        }
    }
}
impl Drop for Remote {
    fn drop(&mut self) {
        if let Ok(runs) = fs::read_dir(self.root().join("runs")) {
            for run in runs.flatten() {
                if let Ok(identity) = read_json::<ProcessIdentity>(&run.path().join("wrapper.json"))
                {
                    if platform::alive(&identity).unwrap_or(false) {
                        let _ = wrapper::cancel(&run.path());
                        let _ = wrapper::cleanup(&run.path());
                        let deadline = Instant::now() + Duration::from_secs(5);
                        while platform::alive(&identity).unwrap_or(false)
                            && Instant::now() < deadline
                        {
                            thread::sleep(Duration::from_millis(20));
                        }
                    }
                }
            }
        }
        self.input = None;
        let end = Instant::now() + Duration::from_secs(3);
        while self.child.try_wait().unwrap().is_none() && Instant::now() < end {
            thread::sleep(Duration::from_millis(10));
        }
        if self.child.try_wait().unwrap().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            reader.join().unwrap();
        }
    }
}

#[test]
fn run_feed_paged_logs_and_double_fork_cancel_over_real_ssh() {
    let mut remote = Remote::new();
    let script = r#"printf 'first\n'
python3 - <<'PY'
import os,time
if os.fork() == 0:
    os.setsid()
    if os.fork() != 0: os._exit(0)
    with open('escaped.pid.tmp','w') as f: f.write(str(os.getpid()))
    os.rename('escaped.pid.tmp','escaped.pid')
    while True: time.sleep(1)
while not os.path.exists('escaped.pid'): time.sleep(0.01)
PY
printf 'second\n'
touch ready
sleep 60
"#;
    let summary = remote.launch("run1", script);
    until(Duration::from_secs(5), || {
        remote.root().join("runs/run1/ready").exists()
    });
    let pid: u32 = fs::read_to_string(remote.root().join("runs/run1/escaped.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let process = platform::process(pid).unwrap();
    let Response::Changes { runs } = remote.request(Request::Changes {
        watch: vec![("run1".into(), summary)],
        follow_logs: vec!["run1".into()],
        wait_ms: 1000,
    }) else {
        panic!();
    };
    assert_eq!(runs.len(), 1);
    assert!(matches!(runs[0].1.state, RunState::Running));
    assert_eq!(runs[0].1.log_len, Some(13));
    let Response::ReadLog {
        page: LogPage::Reset { info: Some(info) },
    } = remote.request(Request::ReadLog {
        target: LogTarget::Run("run1".into()),
        file: LogFile::Log,
        ident: None,
        range: ByteRange {
            start: u64::MAX - 1,
            end: u64::MAX,
        },
        anchor: None,
    })
    else {
        panic!();
    };
    let mut actual = vec![];
    for start in (0..info.len).step_by(4) {
        let Response::ReadLog {
            page: LogPage::Data { bytes, .. },
        } = remote.request(Request::ReadLog {
            target: LogTarget::Run("run1".into()),
            file: LogFile::Log,
            ident: Some(info.ident.clone()),
            range: ByteRange {
                start,
                end: start + 4,
            },
            anchor: None,
        })
        else {
            panic!();
        };
        actual.extend(bytes);
    }
    assert_eq!(actual, b"first\nsecond\n");
    assert_eq!(
        remote.request(Request::Cancel { run: "run1".into() }),
        Response::Cancel {
            outcome: CancelAck::Accepted
        }
    );
    let receipt = remote.finish("run1", runs[0].1.clone());
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    assert!(receipt.processes.is_empty());
    assert_eq!(receipt.outcome, CancelOutcome::Verified);
    assert!(!platform::process(pid).is_ok_and(|p| p.starttime == process.starttime && p.live()));
}

#[test]
fn entire_256_mib_log_is_identical_in_shuffled_random_access_pages_over_ssh() {
    let mut remote = Remote::new();
    let script = "python3 - <<'PY'\nimport sys\nblock=bytearray(bytes(range(256))*4096)\nfor i in range(256):\n    block[:8]=i.to_bytes(8,'little')\n    sys.stdout.buffer.write(block)\nPY\n";
    let summary = remote.launch("large", script);
    assert_eq!(remote.finish("large", summary).code, Some(0));
    let Response::ReadLog {
        page: LogPage::Reset { info: Some(info) },
    } = remote.request(Request::ReadLog {
        target: LogTarget::Run("large".into()),
        file: LogFile::Log,
        ident: None,
        range: ByteRange {
            start: u64::MAX - 1,
            end: u64::MAX,
        },
        anchor: None,
    })
    else {
        panic!();
    };
    assert_eq!(info.len, 256 * 1024 * 1024);
    let mut pages: Vec<u64> = (0..256).collect();
    let mut seed = 0x12345678u64;
    for n in (1..pages.len()).rev() {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        pages.swap(n, seed as usize % (n + 1));
    }
    let mut expected: Vec<u8> = (0..MAX_PAGE_LEN).map(|offset| offset as u8).collect();
    for batch in pages.chunks(4) {
        let mut pending = Vec::new();
        for &page in batch {
            let start = page * MAX_PAGE_LEN;
            let id = remote.send(Request::ReadLog {
                target: LogTarget::Run("large".into()),
                file: LogFile::Log,
                ident: Some(info.ident.clone()),
                range: ByteRange {
                    start,
                    end: start + MAX_PAGE_LEN,
                },
                anchor: if start >= 64 {
                    Some(std::array::from_fn(|i| (start - 64 + i as u64) as u8))
                } else {
                    None
                },
            });
            pending.push((id, page));
        }
        while !pending.is_empty() {
            let response = remote.receive();
            let index = pending
                .iter()
                .position(|(id, _)| *id == response.id)
                .unwrap();
            let (_, page) = pending.swap_remove(index);
            let Response::ReadLog {
                page: LogPage::Data {
                    bytes, eof, start, ..
                },
            } = response.body
            else {
                panic!("page {page}");
            };
            assert_eq!(start, page * MAX_PAGE_LEN);
            assert_eq!(bytes.len() as u64, MAX_PAGE_LEN);
            expected[..8].copy_from_slice(&page.to_le_bytes());
            assert!(bytes == expected, "page {page}");
            assert_eq!(eof, page == 255);
        }
    }
    let start = info.len - 17;
    assert!(
        matches!(remote.request(Request::ReadLog { target: LogTarget::Run("large".into()), file: LogFile::Log, ident: Some(info.ident), range: ByteRange { start, end: start + MAX_PAGE_LEN }, anchor: None }), Response::ReadLog { page: LogPage::Data { bytes, eof: true, .. } } if bytes.len() == 17)
    );
}

#[test]
fn control_lane_multiplexes_with_feed_and_exec_is_bounded_over_ssh() {
    let mut remote = Remote::new();
    let feed = remote.send(Request::Changes {
        watch: vec![],
        follow_logs: vec![],
        wait_ms: 500,
    });
    let hello = remote.send(Request::Hello {
        proto: PROTOCOL_VERSION,
    });
    let reply = remote.receive();
    assert_eq!(reply.id, hello);
    assert!(matches!(reply.body, Response::Hello { .. }));
    assert_eq!(remote.receive().id, feed);
    let Response::Exec { result } = remote.request(Request::Exec {
        command: "cat; head -c 300000 /dev/zero; head -c 20000 /dev/zero >&2".into(),
        stdin: b"input".to_vec(),
        timeout_ms: 2000,
    }) else {
        panic!();
    };
    assert_eq!(result.code, Some(0));
    assert_eq!(result.stdout.len(), 256 * 1024);
    assert_eq!(result.stderr.len(), 16 * 1024);
    assert!(result.stdout.starts_with(b"input"));
    assert!(result.stdout_truncated && result.stderr_truncated);
    let Response::Exec { result } = remote.request(Request::Exec {
        command: "sleep 60 & wait".into(),
        stdin: vec![],
        timeout_ms: 50,
    }) else {
        panic!();
    };
    assert!(result.timed_out);
}

#[test]
fn eof_stops_feed_and_exec_without_leaving_command_children() {
    let mut remote = Remote::new();
    let marker = remote.server.home().join("exec.pid");
    remote.send(Request::Exec {
        command: format!("echo $$ > '{}'; sleep 60 & wait", marker.display()),
        stdin: vec![],
        timeout_ms: 60_000,
    });
    remote.send(Request::Changes {
        watch: vec![],
        follow_logs: vec![],
        wait_ms: 30_000,
    });
    until(Duration::from_secs(3), || {
        fs::read_to_string(&marker).is_ok_and(|text| text.trim().parse::<u32>().is_ok())
    });
    let pid = fs::read_to_string(marker).unwrap().trim().parse().unwrap();
    let identity = platform::process(pid).unwrap();
    let start = Instant::now();
    remote.input = None;
    until(Duration::from_secs(3), || {
        remote.child.try_wait().unwrap().is_some()
    });
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(!platform::process(pid).is_ok_and(|p| p.starttime == identity.starttime && p.live()));
}

#[test]
fn fake_slurm_uses_receipts_and_keeps_unknown_jobs_unreconciled() {
    let mut remote = Remote::new();
    remote.shim("sbatch", "cat >/dev/null; printf '123;cluster\\n'");
    remote.shim("squeue", "exit 0");
    remote.shim("sacct", "printf '123|job|COMPLETED|0:0|\\n'");
    let intent = LaunchIntent {
        run: "job".into(),
        digest: orx_host::sha(b"true"),
        run_sh: "true".into(),
        env: Default::default(),
        source: None,
        cancel_sh: None,
    };
    assert_eq!(
        remote.request(Request::SlurmSubmit {
            intent,
            directives: "#SBATCH --job-name=job\n".into()
        }),
        Response::SlurmSubmit { job: "123".into() }
    );
    let Response::SlurmQueue { jobs } = remote.request(Request::SlurmQueue {
        names: vec!["job".into()],
        legacy: Vec::new(),
    }) else {
        panic!();
    };
    assert!(matches!(
        jobs[0].summary.state,
        RunState::Unreconciled { .. }
    ));
    remote.request(Request::Changes {
        watch: vec![],
        follow_logs: vec![],
        wait_ms: 0,
    });
    assert!(!remote.root().join("runs/job/started").exists());
    let receipt = ExitReceipt {
        finished_at: None,
        code: Some(0),
        signal: None,
        reason: ExitReason::Exited,
        processes: vec![],
        outcome: CancelOutcome::Verified,
        detail: None,
    };
    orx_host::publish(&remote.root().join("runs/job/exit.json"), &receipt).unwrap();
    remote.shim("squeue", "echo 'controller down' >&2; exit 1");
    let Response::SlurmQueue { jobs, .. } = remote.request(Request::SlurmQueue {
        names: vec!["job".into()],
        legacy: Vec::new(),
    }) else {
        panic!();
    };
    assert_eq!(jobs[0].summary.state, RunState::Exited { receipt });
    remote.shim("sacct", "exit 1");
    let Response::SlurmQueue { jobs } = remote.request(Request::SlurmQueue {
        names: vec!["unknown".into()],
        legacy: Vec::new(),
    }) else {
        panic!();
    };
    assert_eq!(
        jobs[0].summary.state,
        RunState::Unreconciled {
            detail: "scheduler unanswered: controller down".into()
        }
    );
}

#[test]
fn slurm_submit_resumes_staging_cut_short_after_the_batch_script() {
    let mut remote = Remote::new();
    remote.shim("sbatch", "cat >/dev/null; printf '123;cluster\\n'");
    let submit = || Request::SlurmSubmit {
        intent: LaunchIntent {
            run: "job".into(),
            digest: orx_host::sha(b"true"),
            run_sh: "true".into(),
            env: Default::default(),
            source: None,
            cancel_sh: None,
        },
        directives: "#SBATCH --job-name=job\n".into(),
    };
    let submitted = Response::SlurmSubmit { job: "123".into() };
    assert_eq!(remote.request(submit()), submitted);
    let dir = remote.root().join("runs/job");
    for interrupted in [
        &["slurm.job", "run.sh"][..],
        &["slurm.job", "run.sh", "launch.json"][..],
    ] {
        for staged in interrupted {
            fs::remove_file(dir.join(staged)).unwrap();
        }
        assert_eq!(remote.request(submit()), submitted);
        assert!(dir.join("run.sh").exists());
    }
    fs::remove_file(dir.join("launch.json")).unwrap();
    let id = remote.send(submit());
    let reply = remote.receive();
    assert_eq!(reply.id, id);
    assert!(matches!(
        reply.body,
        Response::Error {
            code: ErrorCode::Io,
            ..
        }
    ));
    assert!(dir.join("job.sbatch").exists());
    assert_eq!(fs::read_to_string(dir.join("slurm.job")).unwrap(), "123");
}

#[test]
fn legacy_container_cancel_checks_generation_and_both_kill_results() {
    let mut remote = Remote::new();
    let dir = remote.server.home().join("legacy");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("pid"), "99999999").unwrap();
    let container = ContainerRun {
        reference: "fixture".into(),
        id: "abc123".into(),
        started_at: "generation".into(),
        run_dir: "/tmp/container-run".into(),
    };
    let exec_marker = remote.server.home().join("docker-exec");
    remote.shim("docker", &format!("if [ \"$1\" = container ]; then printf 'abc123|generation|running\\n'; else printf '%s' \"$*\" > '{}'; fi", exec_marker.display()));
    let request = |container| Request::CancelLegacy {
        dir: dir.to_str().unwrap().into(),
        container: Some(container),
    };
    assert_eq!(
        remote.request(request(container.clone())),
        Response::CancelLegacy {
            outcome: LegacyCancelOutcome::GroupGone
        }
    );
    let called = fs::read_to_string(&exec_marker).unwrap();
    assert!(
        called.contains("identity")
            && called.contains("kill -KILL")
            && called.contains("/tmp/container-run")
    );
    fs::remove_file(&exec_marker).unwrap();
    remote.shim("docker", "printf 'abc123|new-generation|running\\n'");
    assert_eq!(
        remote.request(request(container.clone())),
        Response::CancelLegacy {
            outcome: LegacyCancelOutcome::GroupGone
        }
    );
    assert!(!exec_marker.exists());
    let host_group = common::Group(
        Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    fs::write(dir.join("pid"), host_group.0.id().to_string()).unwrap();
    remote.shim("docker", "printf 'abc123|generation|paused\\n'");
    let id = remote.send(request(container.clone()));
    let response = remote.receive();
    assert_eq!(response.id, id);
    assert!(matches!(response.body, Response::Error { message, .. }
        if message == "Container 'fixture' is paused. Unpause it to complete cancellation."));
    remote.shim("docker", "if [ \"$1\" = container ]; then printf 'abc123|generation|running\\n'; else echo 'Process identity changed; refusing to signal it' >&2; exit 1; fi");
    let id = remote.send(request(container));
    let response = remote.receive();
    assert_eq!(response.id, id);
    assert!(matches!(response.body, Response::Error { message, .. }
        if message == "Process identity changed; refusing to signal it"));
    assert!(platform::process(host_group.0.id()).unwrap().live());
    assert_eq!(
        fs::read_to_string(dir.join("pid")).unwrap(),
        host_group.0.id().to_string()
    );
    fs::write(dir.join("pid"), "99999999").unwrap();
    drop(host_group);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    let quoted_dir = remote.server.home().join("container's run");
    fs::create_dir(&quoted_dir).unwrap();
    let container = ContainerRun {
        reference: "fixture".into(),
        id: "abc123".into(),
        started_at: "generation".into(),
        run_dir: quoted_dir.to_str().unwrap().into(),
    };
    remote.shim("docker", "if [ \"$1\" = container ]; then printf 'abc123|generation|running\\n'; else shift 3; exec \"$@\"; fi");
    assert_eq!(
        remote.request(request(container)),
        Response::CancelLegacy {
            outcome: LegacyCancelOutcome::GroupGone
        }
    );
    assert!(quoted_dir.join("cancel").exists());
}

#[test]
fn completed_run_reports_lingering_and_cleanup_through_the_feed_over_ssh() {
    let mut remote = Remote::new();
    let summary = remote.launch("daemon", r#"python3 - <<'PY'
import os, sys, time
if os.fork() == 0:
    os.setsid()
    code = "import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); f=open('daemon.pid.tmp','w'); f.write(str(os.getpid())); f.close(); os.rename('daemon.pid.tmp','daemon.pid'); time.sleep(60)"
    os.execv(sys.executable, [sys.executable, '-c', code])
while not os.path.exists('daemon.pid'): time.sleep(0.01)
PY
exit 0
"#);
    let receipt = remote.finish("daemon", summary);
    let pid = fs::read_to_string(remote.root().join("runs/daemon/daemon.pid"))
        .unwrap()
        .parse()
        .unwrap();
    assert!(receipt.processes.iter().any(|process| process.pid == pid));
    assert_eq!(receipt.reason, ExitReason::Exited);
    assert_eq!(
        remote.request(Request::Cleanup {
            run: "daemon".into()
        }),
        Response::Cleanup {
            outcome: CleanupAck::Accepted
        }
    );
    let mut watch = Summary {
        started_at: None,
        state: RunState::Exited { receipt },
        log_len: None,
        notify_len: 0,
        cleanup: None,
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline);
        let Response::Changes { runs } = remote.request(Request::Changes {
            watch: vec![("daemon".into(), watch.clone())],
            follow_logs: vec![],
            wait_ms: 1000,
        }) else {
            panic!();
        };
        if let Some((_, changed)) = runs.into_iter().next() {
            watch = changed;
        }
        if watch.cleanup.is_some() {
            break;
        }
    }
    assert_eq!(
        watch.cleanup.as_ref().unwrap().outcome,
        CancelOutcome::Verified
    );
    assert!(matches!(
        watch.state,
        RunState::Exited {
            receipt: ExitReceipt {
                code: Some(0),
                reason: ExitReason::Exited,
                ..
            }
        }
    ));
    let identity: ProcessIdentity =
        read_json(&remote.root().join("runs/daemon/wrapper.json")).unwrap();
    until(Duration::from_secs(3), || {
        !platform::alive(&identity).unwrap()
    });
    assert!(!platform::process(pid).is_ok_and(|process| process.live()));
}

#[test]
fn slurm_accounting_uses_recorded_ids_and_an_explicit_history_window() {
    let mut remote = Remote::new();
    remote.shim(
        "sbatch",
        "printf '%s' \"$*\" > \"$HOME/sbatch-args\"; cat >/dev/null; printf '123\\n'",
    );
    remote.shim("squeue", "exit 0");
    remote.shim(
        "sacct",
        r#"printf '%s\n' "$*" >> "$HOME/sacct-args"
case " $* " in
*' --jobs=123 '*) printf '123|historical|PREEMPTED|1:0\n';;
*' --name=unrecorded '*) printf '456|unrecorded|NODE_FAIL|1:0\n';;
esac"#,
    );
    remote.request(Request::SlurmSubmit {
        intent: LaunchIntent {
            run: "historical".into(),
            digest: orx_host::sha(b"true"),
            run_sh: "true".into(),
            env: Default::default(),
            source: None,
            cancel_sh: None,
        },
        directives: String::new(),
    });
    let launch = remote.root().join("runs/historical/launch.json");
    let accepted = std::time::UNIX_EPOCH + Duration::from_secs(1_577_934_245);
    File::options()
        .write(true)
        .open(&launch)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(accepted))
        .unwrap();
    assert!(fs::read_to_string(remote.server.home().join("sbatch-args"))
        .unwrap()
        .contains("--no-requeue"));
    let query = || Request::SlurmQueue {
        names: vec!["historical".into()],
        legacy: vec![],
    };
    let Response::SlurmQueue { jobs, .. } = remote.request(query()) else {
        panic!()
    };
    assert_eq!((&*jobs[0].job, &*jobs[0].state), ("123", "PREEMPTED"));
    let arguments = fs::read_to_string(remote.server.home().join("sacct-args")).unwrap();
    let start = arguments
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("--starttime="))
        .unwrap();
    let timestamp = Command::new("python3").args([
        "-c", "import datetime,sys; print(int(datetime.datetime.strptime(sys.argv[1], '%Y-%m-%dT%H:%M:%S').timestamp()))", start,
    ]).output().unwrap();
    assert!(timestamp.status.success());
    assert_eq!(
        String::from_utf8(timestamp.stdout).unwrap().trim(),
        "1577934245"
    );

    assert!(matches!(
        jobs[0].summary.state,
        RunState::Unreconciled { .. }
    ));
    let Response::SlurmQueue { jobs, .. } = remote.request(Request::SlurmQueue {
        names: vec!["unrecorded".into()],
        legacy: vec![],
    }) else {
        panic!()
    };
    assert_eq!((&*jobs[0].job, &*jobs[0].state), ("456", "NODE_FAIL"));
    remote.shim("sacct", "exit 1");
    thread::sleep(Duration::from_millis(150));
    let Response::SlurmQueue { jobs, .. } = remote.request(query()) else {
        panic!()
    };
    assert_eq!((&*jobs[0].job, &*jobs[0].state), ("123", "PREEMPTED"));
    assert!(
        matches!(&jobs[0].summary.state, RunState::Failed { reason } if reason == "scheduler PREEMPTED; no receipt")
    );
    let retained = remote.root().join("runs/historical/slurm-terminal.json");
    let bytes = fs::read(&retained).unwrap();
    let command = format!(
        "ORX_TEST_FAST_STOPS=1 PATH='{}':\"$PATH\" exec '{}' serve --root '{}'",
        remote.server.home().join("bin").display(),
        env!("CARGO_BIN_EXE_orx-host"),
        remote.root().display(),
    );
    let mut replacement = Command::new("ssh")
        .args(remote.server.ssh_args())
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    write_frame(
        replacement.stdin.as_mut().unwrap(),
        &Message {
            id: 1,
            body: query(),
        },
    )
    .unwrap();
    let reply: Message<Response> =
        read_frame(&mut BufReader::new(replacement.stdout.take().unwrap()))
            .unwrap()
            .unwrap();
    let Response::SlurmQueue { jobs } = reply.body else {
        panic!()
    };
    assert_eq!((&*jobs[0].job, &*jobs[0].state), ("123", "PREEMPTED"));
    assert!(
        matches!(&jobs[0].summary.state, RunState::Failed { reason } if reason == "scheduler PREEMPTED; no receipt")
    );
    replacement.stdin.take();
    assert!(replacement.wait().unwrap().success());
    assert_eq!(fs::read(&retained).unwrap(), bytes);
    remote.shim("squeue", "printf '123|historical|RUNNING\\n'");
    remote.request(query());
    assert!(!retained.exists());
    remote.shim("squeue", "exit 0");
    let Response::SlurmQueue { jobs, .. } = remote.request(query()) else {
        panic!()
    };
    assert_eq!(jobs[0].state, "UNKNOWN");
}

#[test]
fn queued_slurm_intent_keeps_its_original_executable_and_directives() {
    let mut remote = Remote::new();
    remote.shim("sbatch", "cat > \"$HOME/accepted-batch\"; printf '123\\n'");
    let intent = LaunchIntent {
        run: "pinned".into(),
        digest: orx_host::sha(b"true"),
        run_sh: "true".into(),
        env: Default::default(),
        source: None,
        cancel_sh: None,
    };
    remote.request(Request::SlurmSubmit {
        intent: intent.clone(),
        directives: "#SBATCH --mem=1G\n".into(),
    });
    let dir = remote.root().join("runs/pinned");
    let batch = fs::read_to_string(dir.join("job.sbatch"))
        .unwrap()
        .replace(env!("CARGO_BIN_EXE_orx-host"), "/queued/orx-host-original");
    fs::write(dir.join("job.sbatch"), &batch).unwrap();
    let mut retry = intent;
    retry.run_sh = "false".into();
    retry.env.insert("CHANGED".into(), "value".into());
    remote.request(Request::SlurmSubmit {
        intent: retry.clone(),
        directives: "invalid regenerated directives".into(),
    });
    assert_eq!(fs::read_to_string(dir.join("job.sbatch")).unwrap(), batch);
    assert!(batch.contains("/queued/orx-host-original"));
    assert_eq!(fs::read(dir.join("run.sh")).unwrap(), b"true");
    retry.digest = orx_host::sha(b"other");
    let id = remote.send(Request::SlurmSubmit {
        intent: retry,
        directives: String::new(),
    });
    let response = remote.receive();
    assert_eq!(response.id, id);
    assert!(matches!(
        response.body,
        Response::Error {
            code: ErrorCode::Conflict,
            ..
        }
    ));
}

#[test]
fn cancelled_slurm_receipts_keep_querying_until_scheduler_proves_teardown() {
    let mut remote = Remote::new();
    remote.shim(
        "squeue",
        "echo query >> \"$HOME/queries\"; printf '123|survivor|RUNNING\\n'",
    );
    remote.shim("sacct", "exit 1");
    let dir = remote.root().join("runs/survivor");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("slurm.job"), "123\n").unwrap();
    fs::write(dir.join("job.sbatch"), "batch").unwrap();
    fs::write(dir.join("started"), "survivor:tag\n").unwrap();
    let mut identity = platform::identity(std::process::id()).unwrap();
    identity.node = "compute-node".into();
    orx_host::publish(&dir.join("wrapper.json"), &identity).unwrap();
    orx_host::publish(
        &dir.join("exit.json"),
        &ExitReceipt {
            finished_at: None,
            code: None,
            signal: None,
            reason: ExitReason::Cancelled,
            processes: vec![Survivor {
                pid: 123456,
                starttime: 1,
                state: "D".into(),
            }],
            outcome: CancelOutcome::Survivors,
            detail: None,
        },
    )
    .unwrap();
    let bytes = fs::read(dir.join("exit.json")).unwrap();
    let query = || Request::SlurmQueue {
        names: vec!["survivor".into()],
        legacy: vec![],
    };
    for _ in 0..2 {
        let Response::SlurmQueue { jobs } = remote.request(query()) else {
            panic!()
        };
        assert_eq!(jobs[0].state, "RUNNING");
        assert!(jobs[0].summary.cleanup.is_none());
    }
    assert_eq!(
        fs::read_to_string(remote.server.home().join("queries"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    for (queue, accounting, settled) in [
        ("exit 1", "exit 0", false),
        ("exit 0", "exit 1", false),
        ("printf 'malformed\\n'", "exit 0", false),
        ("exit 0", "printf 'malformed\\n'", false),
        ("printf '123|survivor|UNKNOWN\\n'", "exit 0", false),
        ("exit 0", "exit 0", true),
        ("printf '123|survivor|COMPLETING\\n'", "exit 0", false),
        ("printf '123|survivor|CANCELLED\\n'", "exit 1", true),
        ("exit 1", "exit 1", true),
    ] {
        let _ = fs::remove_file(dir.join("cleanup.json"));
        remote.shim("squeue", queue);
        remote.shim("sacct", accounting);
        let Response::SlurmQueue { jobs } = remote.request(query()) else {
            panic!()
        };
        assert_eq!(
            jobs[0].summary.cleanup.is_some(),
            settled,
            "{queue} / {accounting}"
        );
        if settled {
            let proof = jobs[0].summary.cleanup.as_ref().unwrap();
            assert!(proof.processes.is_empty());
            assert_eq!(proof.outcome, CancelOutcome::Verified);
            assert_eq!(wrapper::cleanup_result(&dir).unwrap().as_ref(), Some(proof));
        }
        assert_eq!(fs::read(dir.join("exit.json")).unwrap(), bytes);
    }
    remote.shim("squeue", "exit 99");
    let Response::SlurmQueue { jobs } = remote.request(query()) else {
        panic!()
    };
    assert_eq!(jobs[0].state, "RECEIPT");
}

#[test]
fn malformed_slurm_ids_skip_only_the_affected_rows() {
    let mut remote = Remote::new();
    remote.shim(
        "squeue",
        "printf '%s\\n' \"$*\" >> \"$HOME/queries\"; printf '123|healthy|RUNNING\\n'",
    );
    remote.shim("sacct", "exit 0");
    for (run, job) in [("healthy", "123"), ("empty", ""), ("malformed", "bad;456")] {
        let dir = remote.root().join("runs").join(run);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("slurm.job"), job).unwrap();
    }
    let legacy_dir = remote.server.home().join("legacy");
    fs::create_dir(&legacy_dir).unwrap();
    let legacy = SlurmLegacyRun {
        run: "legacy".into(),
        job: "not-a-job".into(),
        dir: legacy_dir.to_string_lossy().into_owned(),
    };
    let names = vec![
        "empty".into(),
        "malformed".into(),
        "legacy".into(),
        "healthy".into(),
    ];
    let Response::SlurmQueue { jobs } = remote.request(Request::SlurmQueue {
        names,
        legacy: vec![legacy.clone()],
    }) else {
        panic!()
    };
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].run, "healthy");
    assert_eq!(jobs[0].state, "RUNNING");
    let arguments = fs::read_to_string(remote.server.home().join("queries")).unwrap();
    assert_eq!(arguments.lines().count(), 1);
    assert!(arguments.contains("--jobs=123"));
    assert!(!arguments.contains("--name="));
    let Response::SlurmQueue { jobs } = remote.request(Request::SlurmQueue {
        names: vec!["empty".into(), "malformed".into(), "legacy".into()],
        legacy: vec![legacy],
    }) else {
        panic!()
    };
    assert!(jobs.is_empty());
    assert_eq!(
        fs::read_to_string(remote.server.home().join("queries")).unwrap(),
        arguments
    );
}
