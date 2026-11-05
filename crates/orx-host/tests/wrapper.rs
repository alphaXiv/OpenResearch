// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]
mod common;
use common::{until, Directory, Run};
use orx_host::{platform, publish, read_json, wrapper};
use orx_host_proto::*;
use std::fs;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::Duration;

// An unreadable environment makes every concurrent same-UID scan incomplete.
static CANCELLATIONS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn cancellation_before_started_never_spawns_the_payload() {
    let mut run = Run::new("touch should-not-run\n");
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    run.spawn();
    run.wait();
    assert!(!run.directory.0.join("started").exists());
    assert!(!run.directory.0.join("should-not-run").exists());
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    assert_eq!(receipt.outcome, CancelOutcome::Verified);
    assert!(receipt.processes.is_empty());

    let mut run = Run::new("touch should-not-run\n");
    fs::write(run.directory.0.join("cancel.req"), "").unwrap();
    run.spawn();
    run.wait();
    assert!(!run.directory.0.join("started").exists());
    assert!(!run.directory.0.join("should-not-run").exists());
    assert_eq!(
        wrapper::receipt(&run.directory.0).unwrap().unwrap().outcome,
        CancelOutcome::Verified
    );
}

#[test]
fn process_group_leader_can_be_a_wrapper() {
    let mut run = Run::new("printf completed\n");
    run.children.push(
        Command::new(env!("CARGO_BIN_EXE_orx-host"))
            .args(["wrap", run.directory.0.to_str().unwrap()])
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    run.wait();
    assert_eq!(
        wrapper::receipt(&run.directory.0).unwrap().unwrap().code,
        Some(0)
    );
}

#[test]
fn term_is_delivered_once_and_checkpoint_completion_returns_before_the_grace_deadline() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new(
        r#"exec python3 - <<'PY'
import signal, time
def checkpoint(signum, frame):
    with open('signals', 'a') as f: f.write('TERM\n')
    time.sleep(0.35)
    raise SystemExit(0)
signal.signal(signal.SIGTERM, checkpoint)
open('ready', 'w').close()
while True: time.sleep(1)
PY
"#,
    );
    run.children.push(
        Command::new(env!("CARGO_BIN_EXE_orx-host"))
            .env("ORX_TEST_FAST_STOPS", "0")
            .args(["wrap", run.directory.0.to_str().unwrap()])
            .spawn()
            .unwrap(),
    );
    run.ready();
    until(Duration::from_secs(3), || {
        run.directory.0.join("ready").exists()
    });
    let start = std::time::Instant::now();
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    run.wait();
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "cancel took {:?}",
        start.elapsed()
    );
    assert_eq!(
        fs::read_to_string(run.directory.0.join("signals")).unwrap(),
        "TERM\n"
    );
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.code, Some(0));
    assert_eq!(receipt.outcome, CancelOutcome::Verified);
}

#[test]
fn transient_request_and_receipt_errors_keep_the_wrapper_and_lock_alive() {
    let mut run = Run::new("touch ready\nwhile [ ! -f release ]; do sleep 0.01; done\nexit 7\n");
    run.spawn();
    run.ready();
    until(Duration::from_secs(3), || {
        run.directory.0.join("ready").exists()
    });
    let cancel = run.directory.0.join("cancel.req");
    std::os::unix::fs::symlink("cancel.req", &cancel).unwrap();
    let identity: ProcessIdentity = read_json(&run.directory.0.join("wrapper.json")).unwrap();
    platform::signal(&platform::process(identity.pid).unwrap(), libc::SIGUSR1).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let alive = platform::alive(&identity).unwrap();
    let locked = orx_host::lock(&run.directory.0.join("wrapper.lock"))
        .unwrap()
        .is_none();
    fs::remove_file(cancel).unwrap();
    assert!(alive && locked);
    let receipt_path = run.directory.0.join("exit.json");
    fs::create_dir(&receipt_path).unwrap();
    fs::write(run.directory.0.join("release"), "").unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let alive = platform::alive(&identity).unwrap();
    let locked = orx_host::lock(&run.directory.0.join("wrapper.lock"))
        .unwrap()
        .is_none();
    fs::remove_dir(receipt_path).unwrap();
    assert!(alive && locked);
    run.wait();
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.code, Some(7));
    assert_eq!(receipt.reason, ExitReason::Exited);
    assert!(!fs::read_dir(&run.directory.0).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".tmp-")));
}

#[test]
fn deleting_the_run_directory_ends_retries_without_signalling_the_payload() {
    let mut run = Run::new("echo $$ > payload.pid\nexec sleep 60\n");
    let diagnostics = fs::File::create(run.directory.0.join("wrapper.log")).unwrap();
    run.children.push(
        Command::new(env!("CARGO_BIN_EXE_orx-host"))
            .env("ORX_TEST_FAST_STOPS", "1")
            .arg("wrap")
            .arg(&run.directory.0)
            .stderr(diagnostics.try_clone().unwrap())
            .spawn()
            .unwrap(),
    );
    run.ready();
    until(Duration::from_secs(3), || {
        fs::read_to_string(run.directory.0.join("payload.pid"))
            .is_ok_and(|pid| pid.trim().parse::<u32>().is_ok())
    });
    let pid = fs::read_to_string(run.directory.0.join("payload.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let payload = platform::process(pid).unwrap();
    run.extra.push(payload.clone());
    std::os::unix::fs::symlink("cancel.req", run.directory.0.join("cancel.req")).unwrap();
    let identity: ProcessIdentity = read_json(&run.directory.0.join("wrapper.json")).unwrap();
    platform::signal(&platform::process(identity.pid).unwrap(), libc::SIGUSR1).unwrap();
    until(Duration::from_secs(3), || {
        diagnostics.metadata().unwrap().len() > 0
    });
    fs::remove_dir_all(&run.directory.0).unwrap();
    until(Duration::from_secs(2), || {
        run.children[0].try_wait().unwrap().is_some()
    });
    run.children[0].wait().unwrap();
    assert!(!platform::alive(&identity).unwrap());
    let current = platform::process(pid).unwrap();
    assert_eq!(current.starttime, payload.starttime);
    assert!(current.live());
}

#[test]
fn duplicate_wrapper_loser_writes_nothing() {
    let mut run = Run::new("printf once >> count\nwhile [ ! -f release ]; do sleep 0.01; done\n");
    run.spawn();
    run.ready();
    let identity = fs::read(run.directory.0.join("wrapper.json")).unwrap();
    let started = fs::read(run.directory.0.join("started")).unwrap();
    run.spawn();
    until(Duration::from_secs(2), || {
        run.children[1].try_wait().unwrap().is_some()
    });
    assert_eq!(
        fs::read(run.directory.0.join("wrapper.json")).unwrap(),
        identity
    );
    assert_eq!(fs::read(run.directory.0.join("started")).unwrap(), started);
    fs::write(run.directory.0.join("release"), "").unwrap();
    run.wait();
    assert_eq!(
        fs::read_to_string(run.directory.0.join("count")).unwrap(),
        "once"
    );
    let before = fs::read_dir(&run.directory.0).unwrap().count();
    run.spawn();
    run.wait();
    assert_eq!(fs::read_dir(&run.directory.0).unwrap().count(), before);
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.code, Some(0));
}

#[test]
fn crash_windows_never_reexecute_after_started() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new("printf once >> count\n");
    assert_eq!(
        wrapper::state(&run.directory.0).unwrap(),
        RunState::Starting
    );
    let mut forged = platform::identity(std::process::id()).unwrap();
    forged.starttime += 1;
    publish(&run.directory.0.join("wrapper.json"), &forged).unwrap();
    assert_eq!(
        wrapper::state(&run.directory.0).unwrap(),
        RunState::Starting
    );
    run.spawn();
    run.wait();
    fs::remove_file(run.directory.0.join("exit.json")).unwrap();
    until(Duration::from_secs(5), || {
        matches!(
            wrapper::state(&run.directory.0).unwrap(),
            RunState::Lost { .. }
        )
    });
    run.spawn();
    run.wait();
    assert_eq!(
        fs::read_to_string(run.directory.0.join("count")).unwrap(),
        "once"
    );
    assert!(!run.directory.0.join("exit.json").exists());
}

#[test]
fn malformed_receipts_remain_unreconciled() {
    let dir = Directory::new();
    for bytes in ["", "{", "{}", "null", "{\"code\":"] {
        fs::write(dir.0.join("exit.json"), bytes).unwrap();
        assert!(matches!(
            wrapper::state(&dir.0).unwrap(),
            RunState::InvalidData { .. }
        ));
    }
}

#[test]
fn wrapper_gone_with_a_complete_empty_tag_scan_is_lost() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let dir = Directory::new();
    fs::write(dir.0.join("started"), "test:nonexistent").unwrap();
    let mut identity = platform::identity(std::process::id()).unwrap();
    identity.starttime += 1;
    publish(&dir.0.join("wrapper.json"), &identity).unwrap();
    assert_eq!(wrapper::cancel(&dir.0).unwrap(), CancelAck::WrapperGone);
    until(Duration::from_secs(5), || {
        matches!(wrapper::state(&dir.0).unwrap(), RunState::Lost { .. })
    });
}

#[test]
fn payload_exiting_during_cancel_publishes_one_receipt() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new("touch ready\ntrap 'exit 0' TERM\nwhile :; do sleep 0.1; done\n");
    run.spawn();
    run.ready();
    until(Duration::from_secs(3), || {
        run.directory.0.join("ready").exists()
    });
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    run.wait();
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    assert!(receipt.processes.is_empty());
    assert_eq!(receipt.outcome, CancelOutcome::Verified);
    let bytes = fs::read(run.directory.0.join("exit.json")).unwrap();
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        if receipt.outcome == CancelOutcome::Verified {
            CancelAck::NotRunning
        } else {
            CancelAck::WrapperGone
        }
    );
    run.spawn();
    run.wait();
    assert_eq!(fs::read(run.directory.0.join("exit.json")).unwrap(), bytes);
    assert_eq!(
        fs::read_dir(&run.directory.0)
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("tmp-"))
            .count(),
        0
    );
}

#[test]
fn detached_double_fork_and_nohup_children_are_killed() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new(
        r#"python3 - <<'PY'
import os, time
if os.fork() == 0:
    os.setsid()
    if os.fork() != 0: os._exit(0)
    with open('escaped.pid.tmp', 'w') as f: f.write(str(os.getpid()))
    os.rename('escaped.pid.tmp', 'escaped.pid')
    while True: time.sleep(1)
while not os.path.exists('escaped.pid'): time.sleep(0.01)
PY
nohup sleep 60 >/dev/null 2>&1 &
echo $! > nohup.pid
touch ready
sleep 60
"#,
    );
    run.spawn();
    run.ready();
    until(Duration::from_secs(5), || {
        run.directory.0.join("ready").exists()
    });
    for file in ["escaped.pid", "nohup.pid"] {
        let pid: u32 = fs::read_to_string(run.directory.0.join(file))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        run.extra.push(platform::process(pid).unwrap());
    }
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    run.wait();
    for process in &run.extra {
        assert!(!platform::process(process.pid)
            .is_ok_and(|found| found.starttime == process.starttime && found.live()));
    }
    let receipt: ExitReceipt = read_json(&run.directory.0.join("exit.json")).unwrap();
    assert_eq!(receipt.outcome, CancelOutcome::Verified);
    assert_eq!(receipt.reason, ExitReason::Cancelled);
}

#[cfg(feature = "test-hooks")]
#[test]
fn real_crashes_before_and_after_exactly_once_marker() {
    use std::process::Command;
    for point in [
        "wrapper_locked",
        "wrapper_identity",
        "wrapper_started",
        "wrapper_spawned",
        "wrapper_before_receipt",
        "wrapper_receipt",
    ] {
        let mut run = Run::new("printf once >> count\n");
        let status = Command::new(env!("CARGO_BIN_EXE_orx-host"))
            .arg("wrap")
            .arg(&run.directory.0)
            .env("ORX_TEST_CRASH_AT", point)
            .status()
            .unwrap();
        assert!(!status.success());
        let started = run.directory.0.join("started").exists();
        run.spawn();
        run.wait();
        let count = fs::read_to_string(run.directory.0.join("count")).unwrap_or_default();
        assert!(
            count == "once" || (started && count.is_empty()),
            "{point}: {count}"
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn an_older_nondumpable_same_uid_process_does_not_block_verified_cancellation() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let dir = Directory::new();
    let old = common::Group(Command::new("python3")
        .arg("-c")
        .arg("import ctypes,time; assert ctypes.CDLL(None).prctl(4,0,0,0,0)==0; open('ready','w').close(); time.sleep(60)")
        .current_dir(&dir.0).process_group(0).spawn().unwrap());
    until(Duration::from_secs(3), || dir.0.join("ready").exists());
    let predecessor = platform::process(old.0.id()).unwrap();
    assert_eq!(predecessor.uid, unsafe { libc::getuid() });
    assert!(platform::scan(None, "", true)
        .unwrap()
        .unreadable
        .contains(&predecessor.pid));
    std::thread::sleep(Duration::from_millis(30));
    let mut run = Run::new("touch ready; exec sleep 60\n");
    run.spawn();
    run.ready();
    let identity: ProcessIdentity = read_json(&run.directory.0.join("wrapper.json")).unwrap();
    assert!(predecessor.starttime < identity.starttime);
    until(Duration::from_secs(3), || {
        run.directory.0.join("ready").exists()
    });
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    run.wait();
    assert_eq!(
        wrapper::receipt(&run.directory.0).unwrap().unwrap().outcome,
        CancelOutcome::Verified
    );
    assert!(platform::process(predecessor.pid).unwrap().live());
}

#[cfg(target_os = "linux")]
#[test]
fn unreadable_environment_requires_a_fresh_scan_after_proven_death() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new("touch ready; exec sleep 60");
    run.spawn();
    run.ready();
    let identity: ProcessIdentity = read_json(&run.directory.0.join("wrapper.json")).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    let mut unreadable = common::Group(Command::new("python3")
        .arg("-c")
        .arg("import ctypes,time; assert ctypes.CDLL(None).prctl(4,0,0,0,0)==0; open('unreadable-ready','w').close(); time.sleep(60)")
        .current_dir(&run.directory.0).process_group(0).spawn().unwrap());
    until(Duration::from_secs(3), || {
        run.directory.0.join("unreadable-ready").exists()
    });
    let process = platform::process(unreadable.0.id()).unwrap();
    assert_eq!(process.uid, unsafe { libc::getuid() });
    assert!(process.starttime >= identity.starttime);
    let tag = fs::read_to_string(run.directory.0.join("started")).unwrap();
    let scan = platform::scan(Some(&identity), tag.trim(), true).unwrap();
    assert!(scan.unreadable.contains(&process.pid));
    assert!(!scan.complete);
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    run.wait();
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert!(
        matches!(receipt.outcome, CancelOutcome::Unverifiable { unreadable } if unreadable.contains(&process.pid))
    );
    assert!(platform::process(process.pid).unwrap().live());
    let bytes = fs::read(run.directory.0.join("exit.json")).unwrap();
    assert_eq!(
        wrapper::cleanup(&run.directory.0).unwrap(),
        CleanupAck::Accepted
    );
    assert!(
        matches!(wrapper::cleanup_result(&run.directory.0).unwrap().unwrap().outcome,
        CancelOutcome::Unverifiable { unreadable } if unreadable.contains(&process.pid))
    );
    unreadable.0.kill().unwrap();
    unreadable.0.wait().unwrap();
    assert!(platform::process(process.pid).is_err());
    assert_eq!(
        wrapper::cleanup(&run.directory.0).unwrap(),
        CleanupAck::Accepted
    );
    let proof = wrapper::cleanup_result(&run.directory.0).unwrap().unwrap();
    assert!(proof.processes.is_empty());
    assert_eq!(proof.outcome, CancelOutcome::Verified);
    assert_eq!(fs::read(run.directory.0.join("exit.json")).unwrap(), bytes);
}

#[test]
fn reboot_only_loses_a_run_after_the_exactly_once_marker() {
    let run = Run::new("printf once >> count\n");
    let mut identity = platform::identity(std::process::id()).unwrap();
    identity.boot_id.push_str("-previous-boot");
    publish(&run.directory.0.join("wrapper.json"), &identity).unwrap();
    assert_eq!(
        wrapper::state(&run.directory.0).unwrap(),
        RunState::Starting
    );
    fs::write(run.directory.0.join("started"), "test:old-tag").unwrap();
    assert_eq!(
        wrapper::state(&run.directory.0).unwrap(),
        RunState::Lost {
            reason: "host rebooted".into()
        }
    );
}

#[test]
fn killed_wrapper_with_scrubbed_orphan_remains_unreconciled() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new(
        r#"python3 - <<'PY'
import os, sys, time
if os.fork() == 0:
    os.setsid()
    if os.fork() != 0: os._exit(0)
    code = "import os,time; f=open('escaped.pid.tmp','w'); f.write(str(os.getpid())); f.close(); os.rename('escaped.pid.tmp','escaped.pid'); time.sleep(60)"
    os.execve(sys.executable, [sys.executable, '-c', code], {})
while True: time.sleep(1)
PY
"#,
    );
    run.spawn();
    run.ready();
    until(Duration::from_secs(5), || {
        run.directory.0.join("escaped.pid").exists()
    });
    let pid = fs::read_to_string(run.directory.0.join("escaped.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let escaped = platform::process(pid).unwrap();
    run.extra.push(escaped.clone());
    let identity: ProcessIdentity = read_json(&run.directory.0.join("wrapper.json")).unwrap();
    let tag = fs::read_to_string(run.directory.0.join("started")).unwrap();
    run.extra.extend(
        platform::scan(Some(&identity), tag.trim(), false)
            .unwrap()
            .processes,
    );
    run.children[0].kill().unwrap();
    assert!(!run.children[0].wait().unwrap().success());
    assert!(matches!(
        wrapper::state(&run.directory.0).unwrap(),
        RunState::Unreconciled { .. }
    ));
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::WrapperGone
    );
    assert!(platform::process(pid)
        .is_ok_and(|process| process.starttime == escaped.starttime && process.live()));
    assert!(!run.directory.0.join("exit.json").exists());
}

#[test]
fn direct_exit_is_published_before_lingering_daemon_cleanup() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new(
        r#"python3 - <<'PY'
import os, sys, time
if os.fork() == 0:
    os.setsid()
    code = "import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); f=open('daemon.pid.tmp','w'); f.write(str(os.getpid())); f.close(); os.rename('daemon.pid.tmp','daemon.pid'); time.sleep(60)"
    os.execv(sys.executable, [sys.executable, '-c', code])
while not os.path.exists('daemon.pid'): time.sleep(0.01)
PY
exit 23
"#,
    );
    run.spawn();
    until(Duration::from_secs(3), || {
        run.directory.0.join("exit.json").exists()
    });
    let pid = fs::read_to_string(run.directory.0.join("daemon.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let daemon = platform::process(pid).unwrap();
    run.extra.push(daemon.clone());
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(
        (receipt.code, receipt.reason),
        (Some(23), ExitReason::Exited)
    );
    assert!(receipt.processes.iter().any(|process| process.pid == pid));
    assert!(run.children[0].try_wait().unwrap().is_none());
    assert!(matches!(
        wrapper::state(&run.directory.0).unwrap(),
        RunState::Exited { .. }
    ));
    let bytes = fs::read(run.directory.0.join("exit.json")).unwrap();
    assert_eq!(
        wrapper::cleanup(&run.directory.0).unwrap(),
        CleanupAck::Accepted
    );
    run.wait();
    let outcome = wrapper::cleanup_result(&run.directory.0).unwrap().unwrap();
    assert_eq!(outcome.outcome, CancelOutcome::Verified);
    assert!(!platform::process(pid)
        .is_ok_and(|process| process.starttime == daemon.starttime && process.live()));
    assert_eq!(fs::read(run.directory.0.join("exit.json")).unwrap(), bytes);
}

#[test]
fn natural_daemon_exit_reaps_and_resolves_lingering_without_changing_the_result() {
    let mut run = Run::new("bash -c 'echo $$ > daemon.pid; touch ready; while [ ! -f release ]; do sleep 0.05; done' &\nwhile [ ! -f ready ]; do sleep 0.01; done\nexit 0\n");
    run.spawn();
    until(Duration::from_secs(3), || {
        run.directory.0.join("exit.json").exists()
    });
    let pid = fs::read_to_string(run.directory.0.join("daemon.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    run.extra.push(platform::process(pid).unwrap());
    let receipt = fs::read(run.directory.0.join("exit.json")).unwrap();
    assert!(!wrapper::receipt(&run.directory.0)
        .unwrap()
        .unwrap()
        .processes
        .is_empty());
    fs::write(run.directory.0.join("release"), "").unwrap();
    run.wait();
    assert_eq!(
        fs::read(run.directory.0.join("exit.json")).unwrap(),
        receipt
    );
    assert!(wrapper::cleanup_result(&run.directory.0).unwrap().is_some());
}

#[test]
fn a_timed_out_hook_retries_before_the_receipt_then_recovers() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new("touch ready; sleep 60\n");
    fs::write(
        run.directory.0.join("cancel.sh"),
        "echo attempt >> hook-called; [ -f hook-recovered ] || sleep 60\n",
    )
    .unwrap();
    run.spawn();
    run.ready();
    until(Duration::from_secs(3), || {
        run.directory.0.join("ready").exists()
    });
    let start = std::time::Instant::now();
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    until(Duration::from_secs(3), || {
        fs::read(run.directory.0.join("hook-called")).is_ok_and(|bytes| bytes.len() >= 16)
    });
    assert!(!run.directory.0.join("exit.json").exists());
    assert!(run.children[0].try_wait().unwrap().is_none());
    fs::write(run.directory.0.join("hook-recovered"), "").unwrap();
    run.wait();
    assert!(start.elapsed() < Duration::from_secs(3));
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    assert!(matches!(
        receipt.outcome,
        CancelOutcome::Unverifiable { .. }
    ));
}

#[test]
fn a_hook_prevents_lingering_from_upgrading_to_verified() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new("bash -c 'touch ready; while [ ! -f release ]; do sleep .01; done' &\nwhile [ ! -f ready ]; do sleep .01; done\n");
    fs::write(run.directory.0.join("cancel.sh"), "true").unwrap();
    run.spawn();
    until(Duration::from_secs(3), || {
        run.directory.0.join("exit.json").exists()
    });
    let receipt = fs::read(run.directory.0.join("exit.json")).unwrap();
    assert_ne!(
        wrapper::receipt(&run.directory.0).unwrap().unwrap().outcome,
        CancelOutcome::Verified
    );
    fs::write(run.directory.0.join("release"), "").unwrap();
    run.wait();
    let cleanup = wrapper::cleanup_result(&run.directory.0).unwrap().unwrap();
    assert!(cleanup.processes.is_empty());
    assert!(matches!(
        cleanup.outcome,
        CancelOutcome::Unverifiable { .. }
    ));
    assert_eq!(
        fs::read(run.directory.0.join("exit.json")).unwrap(),
        receipt
    );
    assert_ne!(
        wrapper::cleanup(&run.directory.0).ok(),
        Some(CleanupAck::NothingLingering)
    );
}

#[test]
fn an_empty_unverifiable_receipt_does_not_skip_cleanup() {
    let run = Run::new("true");
    publish(
        &run.directory.0.join("exit.json"),
        &ExitReceipt {
            finished_at: None,
            code: Some(0),
            signal: None,
            reason: ExitReason::Exited,
            processes: vec![],
            outcome: CancelOutcome::Unverifiable { unreadable: vec![] },
        },
    )
    .unwrap();
    assert_ne!(
        wrapper::cleanup(&run.directory.0).ok(),
        Some(CleanupAck::NothingLingering)
    );
}

#[test]
fn slurm_wrapper_releases_the_allocation_after_the_receipt() {
    let mut run = Run::new("bash -c 'echo $$ > daemon.pid; touch ready; while [ ! -f release ]; do sleep .01; done' &\nwhile [ ! -f ready ]; do sleep .01; done\n");
    run.children.push(
        Command::new(env!("CARGO_BIN_EXE_orx-host"))
            .env("SLURM_JOB_ID", "123")
            .arg("wrap")
            .arg(&run.directory.0)
            .spawn()
            .unwrap(),
    );
    until(Duration::from_secs(3), || {
        run.directory.0.join("exit.json").exists()
    });
    let pid = fs::read_to_string(run.directory.0.join("daemon.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    run.extra.push(platform::process(pid).unwrap());
    run.wait();
    assert!(!wrapper::receipt(&run.directory.0)
        .unwrap()
        .unwrap()
        .processes
        .is_empty());
    fs::write(run.directory.0.join("release"), "").unwrap();
}

#[test]
fn node_or_boot_mismatch_never_authorizes_a_fallback_signal() {
    for foreign_node in [true, false] {
        let run = Run::new("sleep 60\n");
        let mut worker = Command::new("sleep").arg("60").spawn().unwrap();
        let mut identity = platform::identity(worker.id()).unwrap();
        if foreign_node {
            identity.node.push_str("-elsewhere");
        } else {
            identity.boot_id.push_str("-old");
        }
        publish(&run.directory.0.join("wrapper.json"), &identity).unwrap();
        fs::write(run.directory.0.join("started"), "scope-test").unwrap();
        assert_eq!(
            wrapper::cancel(&run.directory.0).unwrap(),
            CancelAck::Accepted
        );
        assert!(worker.try_wait().unwrap().is_none());
        assert!(run.directory.0.join("cancel.req").exists());
        if foreign_node {
            assert_eq!(
                wrapper::state(&run.directory.0).unwrap_err().kind(),
                std::io::ErrorKind::NotConnected
            );
            assert!(wrapper::has_live_processes(&run.directory.0).is_err());
        } else {
            assert!(matches!(
                wrapper::state(&run.directory.0).unwrap(),
                RunState::Lost { .. }
            ));
            assert!(!wrapper::has_live_processes(&run.directory.0).unwrap());
        }
        worker.kill().unwrap();
        worker.wait().unwrap();
    }
}

#[test]
fn missing_log_can_appear_after_a_lookup() {
    let dir = Directory::new();
    let path = dir.0.join("log");
    let range = ByteRange { start: 0, end: 100 };
    assert!(matches!(
        orx_host::log::read(&path, None, &range, None).unwrap(),
        LogPage::Reset { info: None }
    ));
    fs::write(&path, b"Traceback\n").unwrap();
    let LogPage::Data { bytes, .. } = orx_host::log::read(&path, None, &range, None).unwrap()
    else {
        panic!("log missing");
    };
    assert_eq!(bytes, b"Traceback\n");
}

#[test]
fn wrapper_receipt_records_completion_time() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let mut run = Run::new("exit 3\n");
    run.spawn();
    run.wait();
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert_eq!(receipt.code, Some(3));
    assert!((before..=after).contains(&receipt.finished_at.unwrap()));
}

#[test]
fn cancellation_waits_for_a_slow_survivor_inside_the_kill_window() {
    let _serial = CANCELLATIONS.lock().unwrap();
    let mut run = Run::new("touch ready; exec sleep 60");
    let hold = run.directory.0.join("hold");
    fs::write(&hold, "once").unwrap();
    run.children.push(
        Command::new(env!("CARGO_BIN_EXE_orx-host"))
            .env("ORX_TEST_FAST_STOPS", "1")
            .env("ORX_TEST_DEFER_SIGNALS", &hold)
            .arg("wrap")
            .arg(&run.directory.0)
            .spawn()
            .unwrap(),
    );
    until(Duration::from_secs(5), || {
        run.directory.0.join("ready").exists()
    });
    wrapper::cancel(&run.directory.0).unwrap();
    run.wait();
    let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
    assert_eq!(receipt.reason, ExitReason::Cancelled);
    assert!(receipt.processes.is_empty());
    assert_eq!(receipt.outcome, CancelOutcome::Verified);
}

#[test]
fn survivors_after_the_window_are_rekilled_and_the_proof_is_renewed() {
    let _serial = CANCELLATIONS.lock().unwrap();
    for slurm in [false, true] {
        let mut run = Run::new("touch ready; exec sleep 60");
        let hold = run.directory.0.join("hold");
        fs::write(&hold, "hold").unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_orx-host"));
        command
            .env("ORX_TEST_FAST_STOPS", "1")
            .env("ORX_TEST_DEFER_SIGNALS", &hold)
            .arg("wrap")
            .arg(&run.directory.0);
        if slurm {
            command.env("SLURM_JOB_ID", "123");
        }
        run.children.push(command.spawn().unwrap());
        until(Duration::from_secs(5), || {
            run.directory.0.join("ready").exists()
        });
        wrapper::cancel(&run.directory.0).unwrap();
        until(Duration::from_secs(5), || {
            run.directory.0.join("exit.json").exists()
        });
        let bytes = fs::read(run.directory.0.join("exit.json")).unwrap();
        let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
        assert_eq!(receipt.outcome, CancelOutcome::Survivors);
        assert!(!receipt.processes.is_empty());
        if slurm {
            for survivor in receipt.processes {
                run.extra.push(platform::process(survivor.pid).unwrap());
            }
            run.wait();
            assert!(!run.directory.0.join("cleanup.json").exists());
        } else {
            until(Duration::from_secs(5), || {
                wrapper::cleanup_result(&run.directory.0).unwrap().is_some()
            });
            assert!(!wrapper::cleanup_result(&run.directory.0)
                .unwrap()
                .unwrap()
                .processes
                .is_empty());
            fs::remove_file(hold).unwrap();
            run.wait();
            let proof = wrapper::cleanup_result(&run.directory.0).unwrap().unwrap();
            assert!(proof.processes.is_empty());
            assert_eq!(proof.outcome, CancelOutcome::Verified);
        }
        assert_eq!(fs::read(run.directory.0.join("exit.json")).unwrap(), bytes);
    }
}

#[test]
fn a_dead_wrapper_reterms_receipted_survivors_even_after_their_tag_is_gone() {
    let run = Run::new("true");
    let mut worker = common::Group(
        Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap(),
    );
    let survivor = platform::process(worker.0.id()).unwrap().survivor();
    let mut identity = platform::identity(std::process::id()).unwrap();
    identity.starttime += 1;
    publish(&run.directory.0.join("wrapper.json"), &identity).unwrap();
    fs::write(run.directory.0.join("started"), "test:no-matching-tag").unwrap();
    publish(
        &run.directory.0.join("exit.json"),
        &ExitReceipt {
            code: None,
            signal: None,
            reason: ExitReason::Cancelled,
            finished_at: None,
            processes: vec![survivor],
            outcome: CancelOutcome::Survivors,
        },
    )
    .unwrap();
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::WrapperGone
    );
    until(Duration::from_secs(5), || {
        worker.0.try_wait().unwrap().is_some()
    });
}

#[test]
fn cancellation_never_replaces_a_receipt_even_without_started() {
    let run = Run::new("true");
    publish(
        &run.directory.0.join("exit.json"),
        &ExitReceipt {
            finished_at: Some(1234),
            code: None,
            signal: None,
            reason: ExitReason::Cancelled,
            processes: vec![Survivor {
                pid: 123456,
                starttime: 1,
                state: "D".into(),
            }],
            outcome: CancelOutcome::Survivors,
        },
    )
    .unwrap();
    let bytes = fs::read(run.directory.0.join("exit.json")).unwrap();
    assert_eq!(
        wrapper::cancel(&run.directory.0).unwrap(),
        CancelAck::Accepted
    );
    assert_eq!(fs::read(run.directory.0.join("exit.json")).unwrap(), bytes);
}

#[test]
fn dead_wrapper_cleanup_renews_proof_without_changing_the_receipt() {
    let _serial = CANCELLATIONS.lock().unwrap();
    for reboot in [false, true] {
        let run = Run::new("true");
        let mut worker = common::Group(
            Command::new("sleep")
                .arg("60")
                .process_group(0)
                .spawn()
                .unwrap(),
        );
        let process = platform::process(worker.0.id()).unwrap();
        let mut identity = platform::identity(std::process::id()).unwrap();
        identity.starttime += 1;
        if reboot {
            identity.boot_id.push_str("-previous-boot");
        }
        publish(&run.directory.0.join("wrapper.json"), &identity).unwrap();
        fs::write(run.directory.0.join("started"), "test:no-matching-tag").unwrap();
        publish(
            &run.directory.0.join("exit.json"),
            &ExitReceipt {
                finished_at: Some(1234),
                code: None,
                signal: None,
                reason: ExitReason::Cancelled,
                processes: vec![process.survivor()],
                outcome: CancelOutcome::Survivors,
            },
        )
        .unwrap();
        let bytes = fs::read(run.directory.0.join("exit.json")).unwrap();
        if reboot {
            assert_eq!(
                wrapper::cleanup(&run.directory.0).unwrap(),
                CleanupAck::Accepted
            );
            assert!(platform::process(process.pid).unwrap().live());
        } else {
            until(Duration::from_secs(3), || {
                assert!(matches!(
                    wrapper::state(&run.directory.0).unwrap(),
                    RunState::Exited { .. }
                ));
                wrapper::cleanup_result(&run.directory.0)
                    .unwrap()
                    .is_some_and(|proof| proof.outcome == CancelOutcome::Verified)
            });
            assert!(worker.0.try_wait().unwrap().is_some());
        }
        let proof = wrapper::cleanup_result(&run.directory.0).unwrap().unwrap();
        assert!(proof.processes.is_empty());
        assert_eq!(proof.outcome, CancelOutcome::Verified);
        assert_eq!(
            wrapper::cleanup(&run.directory.0).unwrap(),
            CleanupAck::NothingLingering
        );
        assert_eq!(fs::read(run.directory.0.join("exit.json")).unwrap(), bytes);
    }
}

#[test]
fn rentals_release_after_receipts_with_or_without_a_collector_and_retry_release() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };

    let _serial = CANCELLATIONS.lock().unwrap();
    for (code, acknowledge, cancel) in [
        (0, true, false),
        (7, true, false),
        (0, false, false),
        (0, true, true),
    ] {
        let mut run = Run::new(&format!(
            "printf 'private experiment output\\n'; sleep 0.3; exit {code}\n"
        ));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        publish(&run.directory.0.join("rental.json"), &serde_json::json!({
            "rental": {"apiUrl":format!("http://{address}"), "sandboxId":"rental", "token":"capability"},
            "directory":run.directory.0,
        })).unwrap();
        if acknowledge {
            fs::write(run.directory.0.join("final_collected"), "").unwrap();
        }
        if cancel {
            fs::write(run.directory.0.join("cancel.req"), "").unwrap();
        }
        let requests = Arc::new(Mutex::new(Vec::<String>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let server_requests = requests.clone();
        let server_stop = stop.clone();
        let dir = run.directory.0.clone();
        let server = std::thread::spawn(move || {
            let mut releases = 0;
            while !server_stop.load(Ordering::SeqCst) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                let mut byte = [0];
                while !bytes.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let headers = String::from_utf8(bytes).unwrap();
                assert!(headers
                    .to_ascii_lowercase()
                    .contains("authorization: bearer capability\r\n"));
                let mut body = [0; 2];
                stream.read_exact(&mut body).unwrap();
                assert_eq!(&body, b"{}");
                let operation = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_owned();
                if operation == "activate" {
                    assert!(!dir.join("started").exists());
                    assert!(matches!(wrapper::state(&dir).unwrap(), RunState::Starting));
                }
                if operation == "release" {
                    assert!(dir.join("exit.json").exists());
                    releases += 1;
                }
                server_requests.lock().unwrap().push(operation.clone());
                let status = if operation == "release" && releases == 1 {
                    "503 Service Unavailable"
                } else {
                    "204 No Content"
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        run.spawn();
        run.wait();
        stop.store(true, Ordering::SeqCst);
        server.join().unwrap();
        let receipt = wrapper::receipt(&run.directory.0).unwrap().unwrap();
        assert_eq!(receipt.code, (!cancel).then_some(code));
        assert_eq!(
            receipt.reason,
            if cancel {
                ExitReason::Cancelled
            } else {
                ExitReason::Exited
            }
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.first().unwrap(), "activate");
        assert_eq!(requests.last().unwrap(), "release");
        assert_eq!(requests.iter().filter(|op| *op == "release").count(), 2);
        if !cancel {
            assert!(requests.iter().any(|op| op == "heartbeat"));
        }
    }
}
