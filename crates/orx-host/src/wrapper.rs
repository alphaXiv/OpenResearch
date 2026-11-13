use crate::{crash_point, lock, nonce, platform, publish, read_json};
use orx_host_proto::{
    CancelAck, CancelOutcome, CleanupAck, ExitReason, ExitReceipt, LaunchIntent, ProcessIdentity,
    ProcessOutcome, RunState, Survivor,
};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// `keep_awake` holds a sleep prevention from before the payload starts until its receipt.
pub fn wrap(dir: &Path, keep_awake: bool) -> io::Result<()> {
    let Some(_ownership) = lock(&dir.join("wrapper.lock"))? else {
        return Ok(());
    };
    if dir.join("exit.json").try_exists()? || dir.join("started").try_exists()? {
        return Ok(());
    }
    crash_point("wrapper_locked");
    // A detached wrapper must survive ssh hangup; only its payload receives the run tag.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGCHLD, wake as *const () as libc::sighandler_t);
        libc::signal(libc::SIGUSR1, wake as *const () as libc::sighandler_t);
    }
    if unsafe { libc::getsid(0) } != std::process::id() as i32 && unsafe { libc::setsid() } == -1 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EPERM) {
            return Err(error);
        }
    }
    let identity = platform::identity(std::process::id())?;
    publish(&dir.join("wrapper.json"), &identity)?;
    crash_point("wrapper_identity");
    let intent: LaunchIntent = read_json(&dir.join("launch.json"))?;
    let mut awake = keep_awake
        .then(|| crate::awake::hold(&intent.run))
        .flatten();
    let subreaper = platform::subreaper()?;
    let _rental = crate::rental::Duty::start(dir)?;
    let tag = format!("{}:{}", intent.run, nonce()?);
    let gate = start_gate(dir)?;
    if dir.join("cancel.req").try_exists()? {
        publish(&dir.join("exit.json"), &cancelled_before_start())?;
        return Ok(());
    }
    let log = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(dir.join("log"))?;
    let error_log = log.try_clone()?;
    let mut started = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("started"))?;
    writeln!(started, "{tag}")?;
    started.sync_all()?;
    File::open(dir)?.sync_all()?;
    drop(started);
    drop(gate);

    crash_point("wrapper_started");
    let mut command = Command::new("bash");
    command
        .arg("run.sh")
        .current_dir(dir)
        .envs(&intent.env)
        .env("ORX_RUN_TAG", &tag)
        .env("ORX_NOTIFY_FILE", dir.join("notify"))
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(error_log);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            publish(
                &dir.join("exit.json"),
                &ExitReceipt {
                    finished_at: Some(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64,
                    ),
                    code: None,
                    signal: None,
                    reason: ExitReason::SpawnFailed,
                    processes: vec![],
                    outcome: CancelOutcome::Verified,
                },
            )?;
            return Err(error);
        }
    };
    let child_pid = child.id();
    drop(command);
    drop(child);
    crash_point("wrapper_spawned");
    let mut direct_status = None;
    let mut lingering: Option<(Vec<Survivor>, bool)> = None;
    let mut stopping = false;
    let mut renewal_delay = crate::stop_duration(Duration::from_secs(1));
    loop {
        let no_children = retry(dir, child_pid, &mut direct_status, |empty, _| Ok(empty))?;
        let cancel = lingering.is_none()
            && retry(dir, child_pid, &mut direct_status, |_, _| {
                dir.join("cancel.req").try_exists()
            })?;
        let cleanup = lingering.is_some()
            && retry(dir, child_pid, &mut direct_status, |_, _| {
                dir.join("cleanup.req").try_exists()
            })?;
        if cancel || cleanup || stopping {
            let (scan, no_children) = stop(
                dir,
                &identity,
                &tag,
                subreaper,
                child_pid,
                &mut direct_status,
                !stopping,
            )?;
            let outcome = scan.outcome(no_children);
            let processes: Vec<_> = scan
                .processes
                .iter()
                .map(platform::Process::survivor)
                .collect();
            if cancel {
                retry(dir, child_pid, &mut direct_status, |_, status| {
                    publish(
                        &dir.join("exit.json"),
                        &exit_receipt(
                            status,
                            ExitReason::Cancelled,
                            processes.clone(),
                            outcome.clone(),
                        ),
                    )
                })?;
                drop(awake.take());
                if std::env::var_os("SLURM_JOB_ID").is_some()
                    || (processes.is_empty() && no_children)
                {
                    return Ok(());
                }
            } else {
                retry(dir, child_pid, &mut direct_status, |_, _| {
                    publish(
                        &dir.join("cleanup.json"),
                        &ProcessOutcome {
                            processes: processes.clone(),
                            outcome: outcome.clone(),
                        },
                    )
                })?;
                retry(
                    dir,
                    child_pid,
                    &mut direct_status,
                    |_, _| match fs::remove_file(dir.join("cleanup.req")) {
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                        result => result,
                    },
                )?;
            }
            lingering = Some((processes, scan.complete));
            stopping = true;
            if scan.processes.is_empty() && no_children {
                return Ok(());
            }
            unsafe {
                libc::poll(
                    std::ptr::null_mut(),
                    0,
                    renewal_delay.as_millis() as libc::c_int,
                );
            }
            renewal_delay = (renewal_delay * 2).min(crate::stop_duration(Duration::from_secs(30)));
            continue;
        } else if let Some((processes, complete)) = &mut lingering {
            retry(dir, child_pid, &mut direct_status, |_, _| {
                let mut alive = Vec::new();
                for identity in processes.iter() {
                    match platform::process(identity.pid) {
                        Ok(process)
                            if process.starttime == identity.starttime && process.live() =>
                        {
                            alive.push(identity.clone())
                        }
                        Ok(_) => {}
                        Err(error) if missing_process(&error) => {}
                        Err(error) => return Err(error),
                    }
                }
                *processes = alive;
                Ok(())
            })?;
            if processes.is_empty() && no_children {
                let result = ProcessOutcome {
                    processes: vec![],
                    outcome: if *complete {
                        CancelOutcome::Verified
                    } else {
                        CancelOutcome::Unverifiable { unreadable: vec![] }
                    },
                };
                retry(dir, child_pid, &mut direct_status, |_, _| {
                    publish(&dir.join("cleanup.json"), &result)
                })?;
                return Ok(());
            }
        } else if direct_status.is_some() {
            let (scan, no_children) = retry(dir, child_pid, &mut direct_status, |empty, _| {
                let mut scan = platform::scan(Some(&identity), &tag, subreaper)?;
                scan.complete &= !dir.join("cancel.sh").try_exists()?;
                Ok((scan, empty))
            })?;
            let processes: Vec<_> = scan
                .processes
                .iter()
                .map(platform::Process::survivor)
                .collect();
            let outcome = scan.outcome(no_children);
            crash_point("wrapper_before_receipt");
            retry(dir, child_pid, &mut direct_status, |_, status| {
                publish(
                    &dir.join("exit.json"),
                    &exit_receipt(
                        status,
                        ExitReason::Exited,
                        processes.clone(),
                        outcome.clone(),
                    ),
                )
            })?;
            crash_point("wrapper_receipt");
            drop(awake.take());
            if std::env::var_os("SLURM_JOB_ID").is_some() || (processes.is_empty() && no_children) {
                return Ok(());
            }
            lingering = Some((processes, scan.complete));
        }
        unsafe {
            libc::poll(std::ptr::null_mut(), 0, 1000);
        }
    }
}

fn missing_process(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ESRCH)
}

fn exit_receipt(
    status: Option<i32>,
    reason: ExitReason,
    processes: Vec<Survivor>,
    outcome: CancelOutcome,
) -> ExitReceipt {
    ExitReceipt {
        finished_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ),
        code: status
            .filter(|&status| libc::WIFEXITED(status))
            .map(|status| libc::WEXITSTATUS(status)),
        signal: status
            .filter(|&status| libc::WIFSIGNALED(status))
            .map(|status| libc::WTERMSIG(status)),
        reason,
        processes,
        outcome,
    }
}

fn cancelled_before_start() -> ExitReceipt {
    exit_receipt(None, ExitReason::Cancelled, vec![], CancelOutcome::Verified)
}

fn start_gate(dir: &Path) -> io::Result<File> {
    loop {
        if let Some(file) = lock(&dir.join("start.lock"))? {
            return Ok(file);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn retry<T>(
    dir: &Path,
    child: u32,
    status: &mut Option<i32>,
    mut operation: impl FnMut(bool, Option<i32>) -> io::Result<T>,
) -> io::Result<T> {
    let mut delay = crate::stop_duration(Duration::from_millis(100));
    loop {
        let result = match fs::metadata(dir) {
            Ok(_) => reap(child, status).and_then(|empty| operation(empty, *status)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => return Ok(value),
            Err(error) => {
                let _ = writeln!(io::stderr(), "orx-host: supervision retry: {error}");
            }
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(crate::stop_duration(Duration::from_secs(5)));
    }
}

fn stop(
    dir: &Path,
    identity: &ProcessIdentity,
    tag: &str,
    subreaper: bool,
    child: u32,
    direct_status: &mut Option<i32>,
    initial: bool,
) -> io::Result<(platform::Scan, bool)> {
    let mut known = retry(dir, child, direct_status, |_, _| {
        Ok(latest_proof(dir)?.map_or_else(Vec::new, |proof| proof.processes))
    })?;
    let has_hook = retry(dir, child, direct_status, |_, _| {
        dir.join("cancel.sh").try_exists()
    })?;
    if initial && has_hook {
        retry(dir, child, direct_status, |_, _| {
            let result = crate::exec::run(
                Command::new("bash").arg("cancel.sh").current_dir(dir),
                &[],
                crate::stop_duration(crate::HOOK_TIMEOUT),
                None,
            )?;
            if result.code == Some(0) && !result.timed_out {
                Ok(())
            } else {
                Err(io::Error::other("cancel hook did not complete"))
            }
        })?;
    }
    let kill_at = Instant::now()
        + if initial {
            crate::stop_duration(crate::TERM_GRACE)
        } else {
            Duration::ZERO
        };
    let kill_window = Duration::from_secs(30);
    #[cfg(all(feature = "test-hooks", debug_assertions))]
    let kill_window = if std::env::var("ORX_TEST_FAST_STOPS").as_deref() == Ok("1") {
        Duration::from_secs(1)
    } else {
        kill_window
    };
    let mut killed_at = None;
    let mut unreadable_since = None;
    let mut termed = std::collections::BTreeSet::new();
    let mut delay = crate::stop_duration(Duration::from_millis(250));
    loop {
        let (mut scan, no_children) = retry(dir, child, direct_status, |empty, _| {
            let mut scan = platform::scan(Some(identity), tag, subreaper)?;
            scan.include(&known)?;
            Ok((scan, empty))
        })?;
        scan.complete &= !has_hook;
        known = scan
            .processes
            .iter()
            .map(platform::Process::survivor)
            .collect();
        if scan.processes.is_empty() && no_children {
            // An unreadable process is usually mid-exec; give it a moment before the proof gives up.
            let settled = scan.unreadable.is_empty()
                || !initial
                || unreadable_since.get_or_insert_with(Instant::now).elapsed()
                    >= Duration::from_secs(2);
            if settled {
                return Ok((scan, no_children));
            }
        }
        let killing = Instant::now() >= kill_at;
        if killed_at.is_some_and(|since: Instant| since.elapsed() >= kill_window) {
            return Ok((scan, no_children));
        }
        for process in &scan.processes {
            let key = (process.pid, process.starttime);
            if killing || !termed.contains(&key) {
                let signal = if killing {
                    libc::SIGKILL
                } else {
                    libc::SIGTERM
                };
                match platform::signal(process, signal) {
                    Ok(()) => {
                        termed.insert(key);
                    }
                    Err(error) if missing_process(&error) => {}
                    Err(error) => {
                        let _ = writeln!(io::stderr(), "orx-host: signal {}: {error}", process.pid);
                    }
                }
            }
        }
        if killing {
            killed_at.get_or_insert_with(Instant::now);
            if !initial {
                return retry(dir, child, direct_status, |empty, _| {
                    let mut scan = platform::scan(Some(identity), tag, subreaper)?;
                    scan.include(&known)?;
                    scan.complete &= !has_hook;
                    Ok((scan, empty))
                });
            }
            std::thread::sleep(delay);
            delay = (delay * 2).min(crate::stop_duration(Duration::from_secs(2)));
        } else {
            std::thread::sleep(crate::stop_duration(Duration::from_millis(100)));
        }
    }
}

extern "C" fn wake(_: libc::c_int) {}

fn reap(direct: u32, status: &mut Option<i32>) -> io::Result<bool> {
    loop {
        let mut value = 0;
        let pid = unsafe { libc::waitpid(-1, &mut value, libc::WNOHANG) };
        if pid > 0 {
            if pid as u32 == direct {
                *status = Some(value);
            }
            continue;
        }
        if pid == 0 {
            return Ok(false);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ECHILD) {
            return Ok(true);
        }
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

pub fn receipt(dir: &Path) -> io::Result<Option<ExitReceipt>> {
    match read_json(&dir.join("exit.json")) {
        Ok(receipt) => Ok(Some(receipt)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn cleanup_result(dir: &Path) -> io::Result<Option<ProcessOutcome>> {
    match read_json(&dir.join("cleanup.json")) {
        Ok(outcome) => Ok(Some(outcome)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn latest_proof(dir: &Path) -> io::Result<Option<ProcessOutcome>> {
    match cleanup_result(dir)? {
        Some(proof) => Ok(Some(proof)),
        None => Ok(receipt(dir)?.map(|receipt| ProcessOutcome {
            processes: receipt.processes,
            outcome: receipt.outcome,
        })),
    }
}

pub fn cleanup(dir: &Path) -> io::Result<CleanupAck> {
    let mutex = crate::run_mutex(dir)?;
    let _local = mutex
        .lock()
        .map_err(|_| io::Error::other("run lock poisoned"))?;
    let receipt = receipt(dir)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "run has not finished"))?;
    let proof = cleanup_result(dir)?.unwrap_or(ProcessOutcome {
        processes: receipt.processes,
        outcome: receipt.outcome,
    });
    if proof.processes.is_empty() && proof.outcome == CancelOutcome::Verified {
        return Ok(CleanupAck::NothingLingering);
    }
    let identity: ProcessIdentity = read_json(&dir.join("wrapper.json"))?;
    if identity.node == platform::node()? && !platform::alive(&identity)? {
        if let Some(_ownership) = lock(&dir.join("wrapper.lock"))? {
            let proof = if identity.boot_id != platform::boot_id()? {
                ProcessOutcome {
                    processes: Vec::new(),
                    outcome: CancelOutcome::Verified,
                }
            } else {
                let tag = fs::read_to_string(dir.join("started"))?;
                if tag.trim().is_empty() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "empty run tag"));
                }
                let scan = |known: &[Survivor]| -> io::Result<platform::Scan> {
                    let mut scan = platform::scan(Some(&identity), tag.trim(), false)?;
                    scan.include(known)?;
                    scan.complete =
                        scan.unreadable.is_empty() && !dir.join("cancel.sh").try_exists()?;
                    Ok(scan)
                };
                let before = scan(&proof.processes)?;
                let known: Vec<_> = before
                    .processes
                    .iter()
                    .map(platform::Process::survivor)
                    .collect();
                for process in &before.processes {
                    if let Err(error) = platform::signal(process, libc::SIGKILL) {
                        if !missing_process(&error) {
                            return Err(error);
                        }
                    }
                }
                let after = scan(&known)?;
                ProcessOutcome {
                    outcome: after.outcome(true),
                    processes: after
                        .processes
                        .iter()
                        .map(platform::Process::survivor)
                        .collect(),
                }
            };
            publish(&dir.join("cleanup.json"), &proof)?;
            return Ok(CleanupAck::Accepted);
        }
    }
    request(dir, "cleanup.req")?;
    Ok(CleanupAck::Accepted)
}

fn request(dir: &Path, name: &str) -> io::Result<bool> {
    let _gate = start_gate(dir)?;
    let marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join(name));
    match marker {
        Ok(file) => {
            file.sync_all()?;
            File::open(dir)?.sync_all()?;
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    if let Ok(identity) = read_json::<ProcessIdentity>(&dir.join("wrapper.json")) {
        if identity.node == platform::node()? && platform::alive(&identity)? {
            match platform::process(identity.pid) {
                Ok(process) if process.starttime == identity.starttime => {
                    match platform::signal(&process, libc::SIGUSR1) {
                        Ok(()) => return Ok(true),
                        Err(error)
                            if error.raw_os_error() == Some(libc::ESRCH)
                                || error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(_) => {}
                Err(error)
                    if error.raw_os_error() == Some(libc::ESRCH)
                        || error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(false)
}

pub(crate) fn read_error(error: io::Error) -> RunState {
    if error.kind() == io::ErrorKind::InvalidData {
        RunState::InvalidData {
            detail: error.to_string(),
        }
    } else {
        RunState::Unreachable {
            cause: error.to_string(),
        }
    }
}

pub fn started_at(dir: &Path) -> Option<i64> {
    fs::metadata(dir.join("started"))
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()
}

pub fn state(dir: &Path) -> io::Result<RunState> {
    let first = receipt(dir);
    let identity = read_json::<ProcessIdentity>(&dir.join("wrapper.json"));
    let alive = match &identity {
        Ok(identity) => platform::alive(identity),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Ok(false),
    };
    let second = receipt(dir);
    let receipt = match second {
        Ok(None) => first,
        other => other,
    };
    match receipt {
        Ok(Some(receipt)) => {
            if receipt.reason == ExitReason::Cancelled && matches!(alive, Ok(false)) {
                if let Ok(identity) = &identity {
                    if identity.node == platform::node()? {
                        cleanup(dir)?;
                    }
                }
            }
            return Ok(RunState::Exited { receipt });
        }
        Err(error) => return Ok(read_error(error)),
        Ok(None) => {}
    }
    if alive? {
        return Ok(if dir.join("started").try_exists()? {
            RunState::Running
        } else {
            RunState::Starting { reason: None }
        });
    }
    if dir.join("started").try_exists()? {
        if let Ok(identity) = identity {
            if identity.node == platform::node()? {
                if identity.boot_id != platform::boot_id()? {
                    return Ok(RunState::Lost {
                        reason: "host rebooted".into(),
                    });
                }
                if !dir.join("cancel.sh").try_exists()? {
                    let tag = fs::read_to_string(dir.join("started"))?;
                    if !tag.trim().is_empty() {
                        let scan = platform::scan(Some(&identity), tag.trim(), false)?;
                        if scan.processes.is_empty() && scan.unreadable.is_empty() {
                            return Ok(RunState::Lost {
                                reason:
                                    "its supervisor died; no run process remains that orx can see"
                                        .into(),
                            });
                        }
                    }
                }
            }
        }
        return Ok(RunState::Unreconciled {
            detail: "wrapper gone after started; containment unverifiable".into(),
        });
    }
    if dir.join("launch.json").try_exists()? {
        return Ok(RunState::Starting { reason: None });
    }
    Ok(RunState::Missing)
}

pub fn cancel(dir: &Path) -> io::Result<CancelAck> {
    let mutex = crate::run_mutex(dir)?;
    let _local = mutex
        .lock()
        .map_err(|_| io::Error::other("run lock poisoned"))?;
    if receipt(dir)?.is_some_and(|receipt| {
        receipt.reason != ExitReason::Cancelled || receipt.outcome == CancelOutcome::Verified
    }) {
        return Ok(CancelAck::NotRunning);
    }
    let identity = read_json::<ProcessIdentity>(&dir.join("wrapper.json")).ok();
    if request(dir, "cancel.req")? {
        return Ok(CancelAck::Accepted);
    }
    if receipt(dir)?.is_some_and(|receipt| {
        receipt.reason != ExitReason::Cancelled || receipt.outcome == CancelOutcome::Verified
    }) {
        return Ok(CancelAck::NotRunning);
    }
    if !dir.join("started").try_exists()? {
        let _gate = start_gate(dir)?;
        if let Some(_ownership) = lock(&dir.join("wrapper.lock"))? {
            if !dir.join("started").try_exists()? && receipt(dir)?.is_none() {
                publish(&dir.join("exit.json"), &cancelled_before_start())?;
            }
        }
        return Ok(CancelAck::Accepted);
    }
    let identity = identity.ok_or_else(|| io::Error::other("wrapper identity missing"))?;
    if identity.node != platform::node()? || identity.boot_id != platform::boot_id()? {
        return Ok(CancelAck::Accepted);
    }
    if let Ok(tag) = fs::read_to_string(dir.join("started")) {
        if !tag.trim().is_empty() {
            let mut scan = platform::scan(Some(&identity), tag.trim(), false)?;
            let known = latest_proof(dir)?.map_or_else(Vec::new, |proof| proof.processes);
            scan.include(&known)?;
            for process in &scan.processes {
                let _ = platform::signal(process, libc::SIGTERM);
            }
        }
    }
    Ok(CancelAck::WrapperGone)
}

pub fn has_live_processes(dir: &Path) -> io::Result<bool> {
    let identity = match read_json::<ProcessIdentity>(&dir.join("wrapper.json")) {
        Ok(identity) => identity,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if dir.join("started").try_exists()? {
                return Err(io::Error::other("started run has no wrapper identity"));
            }
            return Ok(receipt(dir)?.is_none() && dir.join("launch.json").try_exists()?);
        }
        Err(error) => return Err(error),
    };
    if platform::alive(&identity)? {
        return Ok(true);
    }
    if identity.boot_id != platform::boot_id()? {
        return Ok(false);
    }
    let proof = latest_proof(dir)?;
    if proof
        .as_ref()
        .is_some_and(|proof| proof.processes.is_empty() && proof.outcome == CancelOutcome::Verified)
    {
        return Ok(false);
    }
    let tag = match fs::read_to_string(dir.join("started")) {
        Ok(tag) if !tag.trim().is_empty() => tag,
        Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidData, "empty run tag")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(receipt(dir)?.is_none()),
        Err(error) => return Err(error),
    };
    let mut scan = platform::scan(Some(&identity), tag.trim(), false)?;
    let known = proof.map_or_else(Vec::new, |proof| proof.processes);
    scan.include(&known)?;
    Ok(!scan.processes.is_empty() || !scan.unreadable.is_empty())
}
