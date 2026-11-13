use crate::{
    crash_point, exec, legacy, lock, log, platform, publish, publish_bytes, read_json, run_dir,
    wrapper,
};
use orx_host_proto::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[derive(Deserialize, Serialize)]
struct SchedulerTerminal {
    job: String,
    state: String,
    since: SystemTime,
}

pub struct Host {
    root: PathBuf,
    binary: PathBuf,
    binary_sha: String,
    /// Set when the client is gone, so long waits and commands end.
    stop: Arc<AtomicBool>,
    blobs: Mutex<()>,
    /// Per run: when the wrapper may be spawned again, the delay before that and how many
    /// wrappers this host spawned for it.
    recovery: Mutex<HashMap<RunId, (Instant, Duration, u32)>>,
}

impl Host {
    pub fn new(root: PathBuf, binary: PathBuf) -> io::Result<Self> {
        for path in [&root, &root.join("runs"), &root.join("sources")] {
            fs::create_dir_all(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
        let binary_sha = file_sha(&binary)?;
        Ok(Self {
            root,
            binary,
            binary_sha,
            stop: Arc::default(),
            blobs: Mutex::new(()),
            recovery: Mutex::new(HashMap::new()),
        })
    }

    pub fn handle(&self, request: Request) -> Response {
        self.request(request).unwrap_or_else(|error| {
            Response::Error {
                code: match error.kind() {
                    // A full disk refuses every retry until the user frees space.
                    io::ErrorKind::InvalidInput
                    | io::ErrorKind::StorageFull
                    | io::ErrorKind::QuotaExceeded => ErrorCode::InvalidRequest,
                    io::ErrorKind::AlreadyExists => ErrorCode::Conflict,
                    io::ErrorKind::NotFound => ErrorCode::NotFound,
                    io::ErrorKind::Unsupported => ErrorCode::Unsupported,
                    io::ErrorKind::WouldBlock => ErrorCode::Busy,
                    _ => ErrorCode::Io,
                },
                message: error.to_string(),
            }
        })
    }

    fn exec(
        &self,
        command: &mut Command,
        input: &[u8],
        timeout: Duration,
    ) -> io::Result<ExecResult> {
        exec::run(command, input, timeout, &|| {
            self.stop.load(Ordering::Relaxed)
        })
    }

    /// Runs a Slurm command; one the SSH session can't find or run is the host's setup to fix,
    /// never an unknown outcome.
    fn scheduler(
        &self,
        program: &str,
        args: &[String],
        input: &[u8],
        timeout: Duration,
    ) -> io::Result<ExecResult> {
        self.exec(Command::new(program).args(args), input, timeout)
            .map_err(|error| match error.kind() {
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "{program} can't run in the non-interactive SSH session orx uses on \
                         this host ({error}). Add the directory holding the Slurm commands to \
                         PATH in a file such sessions read, such as ~/.bashrc."
                    ),
                ),
                _ => error,
            })
    }

    fn request(&self, request: Request) -> io::Result<Response> {
        match request {
            Request::Hello { proto } => {
                if proto != PROTOCOL_VERSION {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "unsupported protocol version",
                    ));
                }
                let kernel = platform::kernel()?;
                if cfg!(target_os = "linux") && !platform::supported_kernel(&kernel) {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!(
                            "orx-host needs Linux kernel 3.2 or newer, and this host runs \
                             {kernel}. Choose another host."
                        ),
                    ));
                }
                Ok(Response::Hello {
                    proto: PROTOCOL_VERSION,
                    binary_sha: self.binary_sha.clone(),
                    boot_id: platform::boot_id()?,
                    os: std::env::consts::OS.into(),
                    arch: std::env::consts::ARCH.into(),
                    kernel,
                })
            }
            Request::Launch(intent) => {
                self.launch(&intent, None)?;
                Ok(Response::Launch {
                    summary: self.summary(&intent.run, true, true)?,
                })
            }
            Request::Inspect { run } => {
                let summary = self.summary(&run, false, false)?;
                if !matches!(summary.state, RunState::Missing | RunState::Staging) {
                    return Ok(Response::Inspect { summary });
                }
                let dir = run_dir(&self.root, &run)?;
                if !dir.try_exists()? {
                    return Ok(Response::Inspect { summary });
                }
                let published = unless_preparing(&dir, || {
                    Ok(dir.join("launch.json").try_exists()?
                        || dir.join("job.sbatch").try_exists()?)
                })?
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "A launch of this run is still being prepared on the host.",
                    )
                })?;
                Ok(Response::Inspect {
                    summary: if summary.state == RunState::Missing && published {
                        self.summary(&run, false, false)?
                    } else {
                        summary
                    },
                })
            }
            Request::Changes {
                watch,
                follow_logs,
                wait_ms,
            } => {
                if wait_ms > MAX_WAIT_MS {
                    return invalid("wait exceeds 30 seconds");
                }
                let deadline = Instant::now() + Duration::from_millis(wait_ms.into());
                let names: BTreeSet<_> = watch.iter().map(|(run, _)| run.clone()).collect();
                for name in &names {
                    run_dir(&self.root, name)?;
                }
                for name in &follow_logs {
                    run_dir(&self.root, name)?;
                }
                loop {
                    let mut changes = Vec::new();
                    for name in &names {
                        let summary = self
                            .summary(name, follow_logs.contains(name), true)
                            .unwrap_or_else(|error| {
                                let mut summary = watch
                                    .iter()
                                    .find(|(run, _)| run == name)
                                    .map(|(_, summary)| summary.clone())
                                    .unwrap_or(Summary {
                                        started_at: None,
                                        state: RunState::Missing,
                                        log_len: None,
                                        notify_len: 0,
                                        cleanup: None,
                                    });
                                summary.state = wrapper::read_error(error);
                                summary
                            });
                        if !watch
                            .iter()
                            .any(|(run, old)| run == name && old == &summary)
                        {
                            changes.push((name.clone(), summary));
                        }
                    }
                    if !changes.is_empty()
                        || Instant::now() >= deadline
                        || self.stop.load(Ordering::Relaxed)
                    {
                        return Ok(Response::Changes { runs: changes });
                    }
                    std::thread::sleep(
                        Duration::from_secs(1)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
            }
            Request::ReadLog {
                target,
                file,
                ident,
                range,
                anchor,
            } => {
                let dir = match target {
                    LogTarget::Run(run) => run_dir(&self.root, &run)?,
                    LogTarget::Dir(dir) => legacy::directory(&dir)?,
                };
                crate::slow_request();
                let page = log::read(
                    &dir.join(log_name(file)),
                    ident.as_ref(),
                    &range,
                    anchor.as_ref(),
                )?;
                Ok(Response::ReadLog { page })
            }
            Request::Cancel { run } => {
                let dir = run_dir(&self.root, &run)?;
                // scancel would kill a started wrapper before its receipt; it reads cancel.req.
                // Before it starts, the queued job goes first, so a cancelled receipt never
                // stands for a job that still holds its place in the queue. The marker goes
                // first: published under start.lock, it keeps an unstarted wrapper from starting.
                let slurm = dir.join("job.sbatch").try_exists()?;
                if slurm {
                    wrapper::request(&dir, "cancel.req")?;
                }
                if slurm && !dir.join("started").try_exists()? && wrapper::receipt(&dir)?.is_none()
                {
                    let result = self.scheduler(
                        "scancel",
                        &[format!("--name={run}")],
                        &[],
                        Duration::from_secs(10),
                    )?;
                    if result.code != Some(0) || result.timed_out {
                        return Err(io::Error::other(format!(
                            "scancel --name={run} did not acknowledge the cancel: {}",
                            String::from_utf8_lossy(&result.stderr).trim()
                        )));
                    }
                }
                Ok(Response::Cancel {
                    outcome: wrapper::cancel(&dir)?,
                })
            }
            Request::Cleanup { run } => Ok(Response::Cleanup {
                outcome: wrapper::cleanup(&run_dir(&self.root, &run)?)?,
            }),
            Request::CancelLegacy { dir, container } => Ok(Response::CancelLegacy {
                outcome: legacy::cancel(
                    &legacy::directory(&dir)?,
                    container.as_ref(),
                    Some(&self.stop),
                )?,
            }),
            Request::Exec {
                command,
                stdin,
                timeout_ms,
            } => {
                if timeout_ms == 0 || timeout_ms > 300_000 || stdin.len() > MAX_PAGE_LEN as usize {
                    return invalid("invalid command timeout or input length");
                }
                Ok(Response::Exec {
                    result: self.exec(
                        Command::new("/bin/sh")
                            .args(["-c", &command])
                            .env("ORX_HOST_BINARY", &self.binary),
                        &stdin,
                        Duration::from_millis(timeout_ms.into()),
                    )?,
                })
            }
            Request::HasBlob { sha, len } => Ok(Response::HasBlob {
                present: self.has_blob(&sha, len).map_err(|error| {
                    staging(error, "checking the source in", &self.root.join("sources"))
                })?,
            }),
            Request::PinSource { run, sha } => {
                check_sha(&sha)?;
                let dir = run_dir(&self.root, &run)?;
                Ok(Response::PinSource {
                    pinned: self
                        .pin(&dir, &sha)
                        .map_err(|error| staging(error, "copying the source into", &dir))?,
                })
            }
            Request::PutBlob { sha, offset, bytes } => Ok(Response::PutBlob {
                len: self.put_blob(&sha, offset, &bytes).map_err(|error| {
                    staging(error, "writing the source to", &self.root.join("sources"))
                })?,
            }),
            Request::SlurmSubmit { intent, directives } => {
                let run = intent.run.clone();
                let dir = run_dir(&self.root, &run)?;
                let submit = self.launch(&intent, Some(&directives))?;
                match fs::read_to_string(dir.join("slurm.job")) {
                    Ok(job) => return Ok(Response::SlurmSubmit { job }),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                if !submit {
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "Slurm submission outcome unknown; recover by job name",
                    ));
                }
                let sbatch = fs::read(dir.join("job.sbatch"))?;
                // The launch was never started: retract it whole.
                let retract = || -> io::Result<()> {
                    fs::remove_file(dir.join("launch.json"))?;
                    fs::remove_file(dir.join("job.sbatch"))?;
                    File::open(&dir)?.sync_all()
                };
                let result = match self.scheduler(
                    "sbatch",
                    &[
                        "--parsable".into(),
                        "--no-requeue".into(),
                        format!("--job-name={run}"),
                        format!("--output={}", dir.join("slurm.out").display()),
                    ],
                    &sbatch,
                    Duration::from_secs(30),
                ) {
                    Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                        retract()?;
                        return Err(error);
                    }
                    result => result?,
                };
                let job = if result.code == Some(0) && !result.timed_out {
                    String::from_utf8_lossy(&result.stdout)
                        .trim()
                        .split(';')
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                } else {
                    let queued = self.scheduler(
                        "squeue",
                        &[
                            "--noheader".into(),
                            "--format=%i|%j|%T".into(),
                            format!("--name={run}"),
                        ],
                        &[],
                        Duration::from_secs(10),
                    )?;
                    if queued.code != Some(0) || queued.timed_out || queued.stdout_truncated {
                        return Err(io::Error::other(
                            "sbatch outcome unknown; scheduler query unavailable",
                        ));
                    }
                    let found = String::from_utf8_lossy(&queued.stdout)
                        .lines()
                        .find_map(|line| {
                            let fields: Vec<_> = line.trim().split('|').collect();
                            if fields.len() == 3
                                && fields[1] == run
                                && !fields[0].is_empty()
                                && fields[0].bytes().all(|byte| byte.is_ascii_digit())
                            {
                                Some(fields[0].to_owned())
                            } else {
                                None
                            }
                        });
                    if let Some(job) = found {
                        job
                    } else {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        let lower = stderr.to_ascii_lowercase();
                        let connectivity =
                            ["socket", "controller", "connection", "timed out", "timeout"]
                                .iter()
                                .any(|cause| lower.contains(cause));
                        if result.timed_out || result.code.is_none() || connectivity {
                            return Err(io::Error::other(format!(
                                "sbatch outcome unknown: {}",
                                stderr.trim()
                            )));
                        }
                        retract()?;
                        return invalid(&format!("sbatch failed: {}", stderr.trim()));
                    }
                };
                if job.is_empty() || !job.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(io::Error::other("invalid sbatch job id"));
                }
                publish_bytes(&dir.join("slurm.job"), job.as_bytes())?;
                Ok(Response::SlurmSubmit { job })
            }
            Request::SlurmQueue { names, legacy } => self.slurm_queue(&names, &legacy),
            Request::SlurmCancelLegacy { job } => {
                if job.is_empty() || !job.bytes().all(|b| b.is_ascii_digit()) {
                    return invalid("invalid Slurm job id");
                }
                let result = self.scheduler(
                    "scancel",
                    std::slice::from_ref(&job),
                    &[],
                    Duration::from_secs(10),
                )?;
                if result.code != Some(0) || result.timed_out {
                    return Err(io::Error::other("scancel did not acknowledge cancellation"));
                }
                Ok(Response::SlurmCancelLegacy)
            }
            Request::InspectLegacy { dir } => Ok(Response::InspectLegacy {
                inspection: legacy::inspect(&legacy::directory(&dir)?)?,
            }),
        }
    }

    fn summary(&self, run: &str, follow: bool, repair: bool) -> io::Result<Summary> {
        let dir = run_dir(&self.root, run)?;
        if dir.join("job.sbatch").try_exists()? {
            let fallback = if !slurm_execution(&dir)?
                && (!dir.join("launch.json").try_exists()? || !dir.join("run.sh").try_exists()?)
            {
                RunState::Staging
            } else {
                RunState::Starting { reason: None }
            };
            let mut summary = slurm_summary(&dir, None, fallback);
            if follow {
                summary.log_len = Some(length(&dir.join("log"))?);
            }
            return Ok(summary);
        }
        let mut state = wrapper::state(&dir)?;
        if matches!(state, RunState::Starting { .. }) {
            let intent: LaunchIntent = read_json(&dir.join("launch.json"))?;
            if !repair || (intent.source.is_some() && !dir.join("source.tar").try_exists()?) {
                return Ok(Summary {
                    started_at: None,
                    state: RunState::Staging,
                    log_len: if follow {
                        Some(length(&dir.join("log"))?)
                    } else {
                        None
                    },
                    notify_len: length(&dir.join("notify"))?,
                    cleanup: wrapper::cleanup_result(&dir)?,
                });
            }
            // A cancel that reached no wrapper is honoured here, never respawned past.
            let launched = if dir.join("cancel.req").try_exists()? {
                wrapper::cancel(&dir).map(drop)
            } else {
                self.launch(&intent, None).map(drop)
            };
            match launched {
                Ok(()) => state = wrapper::state(&dir)?,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }
        if !matches!(state, RunState::Starting { .. }) {
            self.recovery
                .lock()
                .map_err(|_| io::Error::other("recovery lock poisoned"))?
                .remove(run);
        }
        Ok(Summary {
            started_at: wrapper::started_at(&dir),
            state,
            log_len: if follow {
                Some(length(&dir.join("log"))?)
            } else {
                None
            },
            notify_len: length(&dir.join("notify"))?,
            cleanup: wrapper::cleanup_result(&dir)?,
        })
    }

    /// True with directives means the accepted batch script still needs submission.
    fn launch(&self, intent: &LaunchIntent, directives: Option<&str>) -> io::Result<bool> {
        let dir = run_dir(&self.root, &intent.run)?;
        check_sha(&intent.digest)?;
        fs::create_dir_all(&dir)?;
        let mutex = crate::run_mutex(&dir)?;
        let _local = mutex
            .lock()
            .map_err(|_| io::Error::other("run lock poisoned"))?;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        let Some(_ownership) = lock(&dir.join("launch.lock"))? else {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "launch is being staged",
            ));
        };
        let batch = dir.join("job.sbatch");
        let mut existing_batch = match fs::read_to_string(&batch) {
            Ok(script) => Some(script),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        let launch = dir.join("launch.json");
        let existing = match read_json::<LaunchIntent>(&launch) {
            Ok(existing) if existing.run != intent.run || existing.digest != intent.digest => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "run already has a different launch intent",
                ));
            }
            Ok(existing) => Some(existing),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        match (directives, &existing_batch, &existing) {
            (None, Some(_), _) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "run belongs to Slurm",
                ))
            }
            (Some(_), None, Some(_)) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "run already has a direct launch",
                ))
            }
            (Some(_), Some(_), None) => {
                if slurm_execution(&dir)? {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Slurm execution has no launch identity",
                    ));
                }
                fs::remove_file(&batch)?;
                existing_batch = None;
            }
            _ => {}
        }
        let intent = existing.as_ref().unwrap_or(intent);
        if intent
            .cancel_sh
            .as_ref()
            .is_some_and(|script| script.len() > MAX_PAGE_LEN as usize)
            || intent.run_sh.len() > MAX_PAGE_LEN as usize
            || intent.env.iter().any(|(key, value)| {
                key.is_empty() || key.contains(['=', '\0']) || value.contains('\0')
            })
        {
            return invalid("invalid payload or environment");
        }
        if let Some(source) = &intent.source {
            check_sha(source)?;
        }
        if existing_batch.is_some() && dir.join("run.sh").try_exists()? {
            return Ok(false);
        }
        // PinSource and launch share ownership so accepted source bytes cannot change.
        if intent.source.is_some() && !dir.join("source.tar").try_exists()? {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "the run's source is not pinned",
            ));
        }
        if existing.is_none() {
            if let Some(directives) = directives {
                if directives.lines().any(|line| !line.starts_with("#SBATCH ")) {
                    return invalid("invalid Slurm directives");
                }
                let sbatch = format!(
                    "#!/bin/bash\n{directives}exec {} wrap {}\n",
                    quote(&self.binary),
                    quote(&dir),
                );
                publish_bytes(&batch, sbatch.as_bytes())?;
            }
            crash_point("before_launch");
            let temporary = dir.join(format!("launch-{}.tmp", crate::nonce()?));
            publish(&temporary, intent)?;
            let result = fs::hard_link(&temporary, &launch);
            fs::remove_file(temporary)?;
            result?;
            File::open(&dir)?.sync_all()?;
            crash_point("launch_published");
        }
        let Some(wrapper_lock) = lock(&dir.join("wrapper.lock"))? else {
            return Ok(false);
        };
        // Under wrapper.lock, so a wrapper that just finished can't have its receipt replaced.
        if dir.join("started").try_exists()? || dir.join("exit.json").try_exists()? {
            return Ok(false);
        }

        if directives.is_none() {
            let mut recovery = self
                .recovery
                .lock()
                .map_err(|_| io::Error::other("recovery lock poisoned"))?;
            let now = Instant::now();
            let previous = recovery.get(&intent.run).copied();
            if previous.is_some_and(|(next, _, _)| next > now) {
                return Ok(false);
            }
            let spawns = previous.map_or(0, |(_, _, spawns)| spawns);
            if spawns >= MAX_SPAWNS {
                // Holding wrapper.lock, no wrapper can start while this receipt is published.
                publish(
                    &dir.join("exit.json"),
                    &ExitReceipt {
                        finished_at: SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|elapsed| elapsed.as_millis() as i64),
                        code: None,
                        signal: None,
                        reason: ExitReason::SpawnFailed,
                        processes: Vec::new(),
                        outcome: CancelOutcome::Verified,
                        detail: Some(format!(
                            "its wrapper exited {spawns} times without starting it; \
                             wrapper.log in {} may say why",
                            dir.display()
                        )),
                    },
                )?;
                return Ok(false);
            }
            let delay = previous.map_or(Duration::from_secs(1), |(_, delay, _)| {
                (delay * 2).min(Duration::from_secs(30))
            });
            recovery.insert(
                intent.run.clone(),
                (now + crate::stop_duration(delay), delay, spawns + 1),
            );
        }
        drop(wrapper_lock);
        if !dir.join("run.sh").try_exists()? {
            publish_bytes(&dir.join("run.sh"), intent.run_sh.as_bytes())?;
        }
        if let Some(script) = &intent.cancel_sh {
            if !dir.join("cancel.sh").try_exists()? {
                publish_bytes(&dir.join("cancel.sh"), script.as_bytes())?;
            }
        }
        if directives.is_some() {
            return Ok(true);
        }
        let diagnostics = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(dir.join("wrapper.log"))?;
        let mut child = Command::new(&self.binary)
            .arg("wrap")
            .arg(&dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(diagnostics)
            .spawn()?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(true)
    }

    fn has_blob(&self, hash: &str, len: u64) -> io::Result<bool> {
        check_sha(hash)?;
        let _ownership = self
            .blobs
            .lock()
            .map_err(|_| io::Error::other("blob lock poisoned"))?;
        let Some(_file_lock) = lock(&self.root.join("sources").join(format!("{hash}.lock")))?
        else {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "blob is being staged",
            ));
        };
        let path = self.root.join("sources").join(hash);
        if path.try_exists()? {
            return Ok(fs::metadata(&path)?.len() == len && file_sha(&path)? == hash);
        }
        let part = path.with_extension("part");
        if !part.try_exists()? || fs::metadata(&part)?.len() != len {
            return Ok(false);
        }
        if file_sha(&part)? != hash {
            fs::remove_file(part)?;
            return Ok(false);
        }
        File::open(&part)?.sync_all()?;
        fs::rename(part, &path)?;
        File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("missing sources"))?,
        )?
        .sync_all()?;
        Ok(true)
    }

    fn pin(&self, dir: &Path, sha: &str) -> io::Result<bool> {
        fs::create_dir_all(dir)?;
        let mutex = crate::run_mutex(dir)?;
        let _local = mutex
            .lock()
            .map_err(|_| io::Error::other("run lock poisoned"))?;
        let Some(_ownership) = lock(&dir.join("launch.lock"))? else {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "launch is being staged",
            ));
        };
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        crate::slow_request();
        let cross_device = crate::cross_device();
        pin_source(
            &self.root.join("sources"),
            sha,
            dir,
            |blob, target| match cross_device {
                true => Err(io::Error::from_raw_os_error(libc::EXDEV)),
                false => fs::hard_link(blob, target),
            },
            if cross_device {
                Duration::ZERO
            } else {
                PIN_SLICE
            },
        )
    }

    fn put_blob(&self, hash: &str, offset: u64, bytes: &[u8]) -> io::Result<u64> {
        check_sha(hash)?;
        if bytes.len() > 256 * 1024 || offset.checked_add(bytes.len() as u64).is_none() {
            return invalid("invalid upload chunk");
        }
        let _ownership = self
            .blobs
            .lock()
            .map_err(|_| io::Error::other("blob lock poisoned"))?;
        let Some(_file_lock) = lock(&self.root.join("sources").join(format!("{hash}.lock")))?
        else {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "blob is being staged",
            ));
        };
        let path = self.root.join("sources").join(hash);
        let complete = path.try_exists()?;
        let path = if complete {
            path
        } else {
            path.with_extension("part")
        };
        let file = OpenOptions::new()
            .read(true)
            .write(!complete)
            .create(!complete)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        let len = file.metadata()?.len();
        if offset > len {
            return invalid("upload offset exceeds staged length");
        }
        let overlap = (len - offset).min(bytes.len() as u64) as usize;
        let mut existing = vec![0; overlap];
        file.read_exact_at(&mut existing, offset)?;
        if existing != bytes[..overlap] {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "upload retry differs from staged bytes",
            ));
        }
        if complete && overlap < bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "completed blob is immutable",
            ));
        }
        if overlap < bytes.len() {
            file.write_all_at(&bytes[overlap..], offset + overlap as u64)?;
        }
        file.sync_data()?;
        Ok(file.metadata()?.len())
    }

    fn slurm_queue(&self, names: &[RunId], legacy: &[SlurmLegacyRun]) -> io::Result<Response> {
        for run in legacy {
            run_dir(&self.root, &run.run)?;
            legacy::directory(&run.dir)?;
        }
        let mut jobs = BTreeMap::new();
        let mut pending = Vec::new();
        for name in names {
            let dir = run_dir(&self.root, name)?;
            let old = legacy.iter().find(|old| old.run == *name);
            if old.is_some_and(|old| {
                old.job.is_empty() || !old.job.bytes().all(|b| b.is_ascii_digit())
            }) {
                continue;
            }
            let summary = slurm_summary(&dir, old, RunState::Missing);
            let survivors = matches!(&summary.state, RunState::Exited { receipt }
                if receipt.reason == ExitReason::Cancelled
                    && !summary.cleanup.as_ref().map_or(&receipt.processes, |proof| &proof.processes).is_empty());
            if summary.state != RunState::Missing && !survivors {
                jobs.insert(
                    name.clone(),
                    SlurmJob {
                        run: name.clone(),
                        job: old.map(|old| old.job.clone()).unwrap_or_else(|| {
                            fs::read_to_string(dir.join("slurm.job")).unwrap_or_default()
                        }),
                        state: "RECEIPT".into(),
                        summary,
                    },
                );
            } else {
                pending.push(name.clone());
            }
        }
        let mut known_jobs = BTreeMap::new();
        for run in std::mem::take(&mut pending) {
            let job = match legacy.iter().find(|old| old.run == run) {
                Some(old) => old.job.clone(),
                None => match fs::read_to_string(run_dir(&self.root, &run)?.join("slurm.job")) {
                    Ok(job) => job.trim().to_owned(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        pending.push(run);
                        continue;
                    }
                    Err(error) => return Err(error),
                },
            };
            if job.is_empty() || !job.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            known_jobs.insert(run.clone(), job);
            pending.push(run);
        }
        if pending.is_empty() {
            return Ok(Response::SlurmQueue {
                jobs: jobs.into_values().collect(),
            });
        }
        let job_ids = known_jobs.values().cloned().collect::<Vec<_>>().join(",");
        let names = pending
            .iter()
            .filter(|run| !known_jobs.contains_key(*run))
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let filters: Vec<_> = [("name", names), ("jobs", job_ids)]
            .into_iter()
            .filter(|(_, values)| !values.is_empty())
            .map(|(argument, values)| format!("--{argument}={values}"))
            .collect();
        let find_run = |job: &str, name: &str| -> Option<String> {
            pending
                .iter()
                .find(|run| match known_jobs.get(*run) {
                    Some(id) => id == job,
                    None => **run == name,
                })
                .cloned()
        };
        let elapsed = |run: &str, job: &str, state: &str| -> io::Result<Duration> {
            let dir = run_dir(&self.root, run)?;
            fs::create_dir_all(&dir)?;
            let mutex = crate::run_mutex(&dir)?;
            let _local = mutex
                .lock()
                .map_err(|_| io::Error::other("run lock poisoned"))?;
            let path = dir.join("slurm-terminal.json");
            let state = state
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_end_matches('+');
            match scheduler_state(state, Duration::MAX) {
                RunState::Starting { .. } | RunState::Running => {
                    match fs::remove_file(&path) {
                        Ok(()) => File::open(&dir)?.sync_all()?,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                    Ok(Duration::ZERO)
                }
                RunState::Failed { .. } | RunState::Lost { .. } | RunState::Cancelled { .. } => {
                    let previous = match read_json::<SchedulerTerminal>(&path) {
                        Ok(fact) => Some(fact),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                        Err(error) => return Err(error),
                    };
                    match previous {
                        Some(fact) if fact.job == job && fact.state == state => {
                            Ok(fact.since.elapsed().unwrap_or_default())
                        }
                        // A slow fsync must not count toward the grace of a state first seen now.
                        _ => {
                            let fact = SchedulerTerminal {
                                job: job.into(),
                                state: state.into(),
                                since: SystemTime::now(),
                            };
                            publish(&path, &fact)?;
                            Ok(Duration::ZERO)
                        }
                    }
                }
                _ => Ok(Duration::ZERO),
            }
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let budget = || {
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_secs(10))
        };
        let queued: Vec<_> = filters
            .iter()
            .map(|filter| {
                self.exec(
                    Command::new("squeue").args([
                        "--noheader",
                        "--states=all",
                        "--format=%i|%j|%T|%r",
                        filter,
                    ]),
                    &[],
                    budget(),
                )
            })
            .collect();
        let mut unanswered = None;
        for result in queued {
            let result = match result {
                Ok(result)
                    if result.code == Some(0) && !result.timed_out && !result.stdout_truncated =>
                {
                    result
                }
                other => {
                    unanswered = Some(match other {
                        Ok(result) => String::from_utf8_lossy(&result.stderr).trim().to_owned(),
                        Err(error) => error.to_string(),
                    });
                    continue;
                }
            };
            for line in String::from_utf8_lossy(&result.stdout).lines() {
                let fields: Vec<_> = line.trim().splitn(4, '|').collect();
                if fields.len() < 3 {
                    unanswered = Some(line.to_owned());
                    continue;
                }
                let Some(run) = find_run(fields[0], fields[1]) else {
                    continue;
                };
                let state = match scheduler_state(fields[2], elapsed(&run, fields[0], fields[2])?) {
                    RunState::Starting { .. } => RunState::Starting {
                        reason: fields
                            .get(3)
                            .map(|reason| reason.trim())
                            .filter(|reason| !reason.is_empty() && *reason != "None")
                            .map(str::to_owned),
                    },
                    state => state,
                };
                jobs.insert(
                    run.clone(),
                    SlurmJob {
                        run: run.clone(),
                        job: fields[0].into(),
                        state: fields[2].into(),
                        summary: slurm_summary(
                            &run_dir(&self.root, &run)?,
                            legacy.iter().find(|old| old.run == run),
                            state,
                        ),
                    },
                );
            }
        }
        let mut start = None;
        for run in &pending {
            let dir = match legacy.iter().find(|old| old.run == *run) {
                Some(old) => legacy::directory(&old.dir)?,
                None => run_dir(&self.root, run)?,
            };
            for name in ["launch.json", "job.sbatch"] {
                let modified = match fs::metadata(dir.join(name)) {
                    Ok(metadata) => metadata.modified()?,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                start = Some(start.map_or(modified, |earliest: SystemTime| earliest.min(modified)));
                break;
            }
        }
        let start = start
            .map(|time| -> io::Result<String> {
                let seconds = time
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(io::Error::other)?
                    .as_secs()
                    .try_into()
                    .map_err(io::Error::other)?;
                let mut local = unsafe { std::mem::zeroed::<libc::tm>() };
                if unsafe { libc::localtime_r(&seconds, &mut local) }.is_null() {
                    return Err(io::Error::last_os_error());
                }
                Ok(format!(
                    "--starttime={:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                    local.tm_year + 1900,
                    local.tm_mon + 1,
                    local.tm_mday,
                    local.tm_hour,
                    local.tm_min,
                    local.tm_sec
                ))
            })
            .transpose()?;
        let accounting: Vec<_> = filters
            .iter()
            .map(|filter| {
                self.exec(
                    Command::new("sacct")
                        .args([
                            "--noheader",
                            "--parsable2",
                            "--format=JobIDRaw,JobName,State",
                            filter,
                        ])
                        .args(start.as_ref()),
                    &[],
                    budget(),
                )
            })
            .collect();
        let mut accounting_answered = accounting.iter().all(|result| {
            matches!(result, Ok(result) if result.code == Some(0) && !result.timed_out && !result.stdout_truncated)
        });
        for result in accounting.into_iter().flatten() {
            if result.code == Some(0) && !result.timed_out && !result.stdout_truncated {
                for line in String::from_utf8_lossy(&result.stdout).lines() {
                    let fields: Vec<_> = line.trim().split('|').collect();
                    if fields.len() < 3 {
                        accounting_answered = false;
                        continue;
                    }
                    if fields[0].contains('.') {
                        continue;
                    }
                    let Some(run) = find_run(fields[0], fields[1]) else {
                        continue;
                    };
                    if jobs.contains_key(&run) {
                        continue;
                    }
                    let state = scheduler_state(fields[2], elapsed(&run, fields[0], fields[2])?);
                    jobs.insert(
                        run.clone(),
                        SlurmJob {
                            run: run.clone(),
                            job: fields[0].into(),
                            state: fields[2].into(),
                            summary: slurm_summary(
                                &run_dir(&self.root, &run)?,
                                legacy.iter().find(|old| old.run == run),
                                state,
                            ),
                        },
                    );
                }
            }
        }
        let mut absent = BTreeSet::new();
        for run in pending {
            if !jobs.contains_key(&run) {
                let path = run_dir(&self.root, &run)?.join("slurm-terminal.json");
                let terminal = match read_json::<SchedulerTerminal>(&path) {
                    Ok(fact) => Some(fact),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
                if terminal.is_none() && unanswered.is_none() && accounting_answered {
                    absent.insert(run.clone());
                }
                let fallback = match &terminal {
                    Some(fact) => {
                        scheduler_state(&fact.state, fact.since.elapsed().unwrap_or_default())
                    }
                    None => RunState::Unreconciled {
                        detail: match &unanswered {
                            Some(error) => format!("scheduler unanswered: {error}"),
                            None => "scheduler has no record and no receipt".into(),
                        },
                    },
                };
                let summary = slurm_summary(
                    &run_dir(&self.root, &run)?,
                    legacy.iter().find(|old| old.run == run),
                    fallback,
                );
                jobs.insert(
                    run.clone(),
                    SlurmJob {
                        run,
                        job: terminal
                            .as_ref()
                            .map(|fact| fact.job.clone())
                            .unwrap_or_default(),
                        state: terminal
                            .map(|fact| fact.state)
                            .unwrap_or_else(|| "UNKNOWN".into()),
                        summary,
                    },
                );
            }
        }
        for job in jobs.values_mut() {
            let stopped = matches!(
                scheduler_state(&job.state, Duration::MAX),
                RunState::Failed { .. } | RunState::Lost { .. } | RunState::Cancelled { .. }
            ) || absent.contains(&job.run);
            if stopped
                && matches!(&job.summary.state, RunState::Exited { receipt } if receipt.reason == ExitReason::Cancelled)
            {
                let proof = ProcessOutcome {
                    processes: Vec::new(),
                    outcome: CancelOutcome::Verified,
                };
                publish(&run_dir(&self.root, &job.run)?.join("cleanup.json"), &proof)?;
                job.summary.cleanup = Some(proof);
            }
        }
        Ok(Response::SlurmQueue {
            jobs: jobs.into_values().collect(),
        })
    }
}

/// An OS error while staging a source, naming the step and directory, with the fix for a cause
/// the user can clear.
fn staging(error: io::Error, step: &str, dir: &Path) -> io::Error {
    if error.raw_os_error().is_none() {
        return error;
    }
    let fix = match error.kind() {
        io::ErrorKind::QuotaExceeded => {
            " Free space within your disk quota (`quota -s` shows it), then run the experiment again.".into()
        }
        io::ErrorKind::StorageFull => format!(
            " Free space on its disk (`df -h {}`), then run the experiment again.",
            quote(dir)
        ),
        io::ErrorKind::PermissionDenied => format!(
            " Make it writable by you (`ls -ld {}`), then run the experiment again.",
            quote(dir)
        ),
        _ => String::new(),
    };
    io::Error::new(
        error.kind(),
        format!("{error} {step} {}.{fix}", dir.display()),
    )
}

/// Wrappers one host spawns for a run that keep dying before starting the payload, before it
/// gives up with a spawn_failed receipt.
const MAX_SPAWNS: u32 = 5;

/// Runs `then` holding the run's launch ownership, or returns None while a launch of it is
/// being prepared, here or by another orx-host.
fn unless_preparing<T>(dir: &Path, then: impl FnOnce() -> io::Result<T>) -> io::Result<Option<T>> {
    let mutex = crate::run_mutex(dir)?;
    let _local = match mutex.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
        Err(std::sync::TryLockError::Poisoned(_)) => {
            return Err(io::Error::other("run lock poisoned"))
        }
    };
    let Some(_ownership) = lock(&dir.join("launch.lock"))? else {
        return Ok(None);
    };
    then().map(Some)
}

fn invalid<T>(message: &str) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message))
}

fn slurm_execution(dir: &Path) -> io::Result<bool> {
    for name in ["slurm.job", "wrapper.json", "started", "exit.json"] {
        if dir.join(name).try_exists()? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn log_name(file: LogFile) -> &'static str {
    match file {
        LogFile::Log => "log",
        LogFile::Notify => "notify",
    }
}
fn length(path: &Path) -> io::Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn check_sha(hash: &str) -> io::Result<()> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(())
    } else {
        invalid("invalid SHA-256 digest")
    }
}
fn file_sha(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let len = file.read(&mut bytes)?;
        if len == 0 {
            break;
        }
        hash.update(&bytes[..len]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn slurm_summary(dir: &Path, old: Option<&SlurmLegacyRun>, fallback: RunState) -> Summary {
    let result = (|| -> io::Result<Summary> {
        if let Some(old) = old {
            let dir = legacy::directory(&old.dir)?;
            let inspection = legacy::inspect(&dir)?;
            let state = match inspection.exit_code {
                Some(code) => RunState::Exited {
                    receipt: ExitReceipt {
                        finished_at: None,
                        code: Some(code),
                        signal: None,
                        reason: ExitReason::Exited,
                        processes: vec![],
                        outcome: CancelOutcome::Unverifiable { unreadable: vec![] },
                        detail: None,
                    },
                },
                None if inspection.receipt_unreadable => RunState::Unreconciled {
                    detail: "legacy receipt unreadable".into(),
                },
                None => fallback,
            };
            return Ok(Summary {
                started_at: None,
                state,
                log_len: None,
                notify_len: length(&dir.join("notify"))?,
                cleanup: None,
            });
        }
        // Refresh NFS visibility before reopening the receipt.
        match fs::metadata(dir.join("exit.json")) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let state = match wrapper::receipt(dir) {
            Ok(Some(receipt)) => RunState::Exited { receipt },
            Ok(None) => fallback,
            Err(error) => wrapper::read_error(error),
        };
        Ok(Summary {
            started_at: wrapper::started_at(dir),
            state,
            log_len: None,
            notify_len: length(&dir.join("notify"))?,
            cleanup: wrapper::cleanup_result(dir)?,
        })
    })();
    result.unwrap_or_else(|error| Summary {
        started_at: None,
        state: wrapper::read_error(error),
        log_len: None,
        notify_len: 0,
        cleanup: None,
    })
}

fn scheduler_state(state: &str, elapsed: Duration) -> RunState {
    let state = state
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_end_matches('+');
    if matches!(state, "PENDING" | "CONFIGURING") {
        return RunState::Starting { reason: None };
    }
    if matches!(state, "RUNNING" | "COMPLETING" | "SUSPENDED") {
        return RunState::Running;
    }
    if !matches!(
        state,
        "COMPLETED"
            | "CANCELLED"
            | "FAILED"
            | "TIMEOUT"
            | "OUT_OF_MEMORY"
            | "NODE_FAIL"
            | "PREEMPTED"
            | "BOOT_FAIL"
            | "DEADLINE"
            | "REVOKED"
    ) {
        return RunState::Unreconciled {
            detail: format!("unknown scheduler state: {state}"),
        };
    }
    if elapsed < crate::stop_duration(Duration::from_secs(120)) {
        return RunState::Unreconciled {
            detail: "awaiting receipt visibility after scheduler terminal".into(),
        };
    }
    if state == "CANCELLED" {
        RunState::Cancelled {
            reason: "scheduler cancelled; no receipt".into(),
        }
    } else if matches!(
        state,
        "FAILED"
            | "TIMEOUT"
            | "OUT_OF_MEMORY"
            | "NODE_FAIL"
            | "PREEMPTED"
            | "BOOT_FAIL"
            | "DEADLINE"
            | "REVOKED"
    ) {
        RunState::Failed {
            reason: format!("scheduler {state}; no receipt"),
        }
    } else {
        RunState::Lost {
            reason: "no receipt".into(),
        }
    }
}

fn write_response(writer: &mut impl Write, response: &Message<Response>) -> io::Result<()> {
    let mut frame = Vec::new();
    if let Err(error) = write_frame(&mut frame, response) {
        frame.clear();
        write_frame(
            &mut frame,
            &Message {
                id: response.id,
                body: Response::Error {
                    code: ErrorCode::Io,
                    message: error.to_string(),
                },
            },
        )?;
    }
    writer.write_all(&frame)?;
    writer.flush()
}

/// How long one `PinSource` request may copy across filesystems before answering.
const PIN_SLICE: Duration = Duration::from_secs(5);

/// Holds blob `sha` from `sources` in run dir `dir` as `source.tar`, against source sweeps: a
/// hard link, or across filesystems a copy built in a part named for the blob, a megabyte at
/// a time for about `budget` per call. Only what was synced counts, recorded beside the part,
/// so a retry cuts off whatever an interrupted writer left after it and continues. True once
/// the whole part is published. A blob already swept fails with NotFound, so the client
/// stages it again.
fn pin_source(
    sources: &Path,
    sha: &str,
    dir: &Path,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
    budget: Duration,
) -> io::Result<bool> {
    let target = dir.join("source.tar");
    if target.try_exists()? {
        return Ok(true);
    }
    let blob = sources.join(sha);
    match link(&blob, &target) {
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {}
        result => return result.map(|()| true),
    }
    let part = dir.join(format!("source.tar.{sha}.part"));
    let synced = dir.join(format!("source.tar.{sha}.synced"));
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().and_then(|name| name.to_str());
        let ours = path == part || path == synced;
        if !ours && name.is_some_and(|name| name.starts_with("source.tar.")) {
            fs::remove_file(path)?;
        }
    }
    let source = File::open(&blob)?;
    let total = source.metadata()?.len();
    let mut len = fs::read_to_string(&synced)
        .ok()
        .and_then(|len| len.parse::<u64>().ok())
        .unwrap_or(0)
        .min(total);
    let copy = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&part)?;
    copy.set_len(len)?;
    let mut buffer = vec![0; 1 << 20];
    let started = Instant::now();
    while len < total {
        let step = (total - len).min(buffer.len() as u64) as usize;
        source.read_exact_at(&mut buffer[..step], len)?;
        copy.write_all_at(&buffer[..step], len)?;
        len += step as u64;
        if started.elapsed() >= budget {
            break;
        }
    }
    copy.sync_all()?;
    if len < total {
        crate::publish_bytes(&synced, len.to_string().as_bytes())?;
        return Ok(false);
    }
    fs::rename(&part, &target)?;
    let _ = fs::remove_file(&synced);
    File::open(dir)?.sync_all()?;
    Ok(true)
}

pub fn serve(root: PathBuf, binary: PathBuf) -> io::Result<()> {
    #[derive(Default)]
    struct InFlight {
        ids: BTreeSet<u64>,
        running: usize,
    }

    let host = Arc::new(Host::new(root, binary)?);
    let output = Arc::new(Mutex::new(io::stdout()));
    let inflight = Arc::new(Mutex::new(InFlight::default()));
    let finished = Arc::new(Condvar::new());
    let mut input = io::stdin().lock();
    let result = (|| -> io::Result<()> {
        while let Some(message) = read_frame::<Message<Request>>(&mut input)? {
            let mut active = inflight
                .lock()
                .map_err(|_| io::Error::other("request set poisoned"))?;
            if active.ids.len() >= 32 || !active.ids.insert(message.id) {
                write_frame(
                    &mut *output
                        .lock()
                        .map_err(|_| io::Error::other("output poisoned"))?,
                    &Message {
                        id: message.id,
                        body: Response::Error {
                            code: ErrorCode::Busy,
                            message: "too many requests or duplicate request id".into(),
                        },
                    },
                )?;
                continue;
            }
            active.running += 1;
            drop(active);
            let (host, output, inflight, finished) = (
                host.clone(),
                output.clone(),
                inflight.clone(),
                finished.clone(),
            );
            std::thread::spawn(move || {
                let response = Message {
                    id: message.id,
                    body: host.handle(message.body),
                };
                if let Ok(mut active) = inflight.lock() {
                    active.running -= 1;
                    finished.notify_all();
                }
                if !host.stop.load(Ordering::Relaxed) {
                    if let Ok(mut output) = output.lock() {
                        if let Err(error) = write_response(&mut *output, &response) {
                            let _ = writeln!(
                                io::stderr(),
                                "orx-host: response {}: {error}",
                                response.id
                            );
                        }
                    }
                }
                if let Ok(mut active) = inflight.lock() {
                    active.ids.remove(&message.id);
                }
            });
        }
        Ok(())
    })();
    host.stop.store(true, Ordering::Relaxed);
    let mut active = inflight
        .lock()
        .map_err(|_| io::Error::other("request set poisoned"))?;
    while active.running > 0 {
        active = finished
            .wait(active)
            .map_err(|_| io::Error::other("request set poisoned"))?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Across mounts the link fails with EXDEV. A copy slower than one request continues
    /// across requests from what was synced; whatever an interrupted writer left past that is
    /// cut off, never published; strays from other copies go.
    #[test]
    fn a_source_on_another_filesystem_is_pinned_by_resumable_copy() {
        let root = std::env::temp_dir().join(format!("orx-host-{}", crate::nonce().unwrap()));
        let (sources, dir) = (root.join("sources"), root.join("runs/r1"));
        fs::create_dir_all(&sources).unwrap();
        fs::create_dir_all(&dir).unwrap();
        let archive: Vec<u8> = (0..3 << 20).map(|i: u32| (i % 251) as u8).collect();
        let sha = crate::sha(&archive);
        fs::write(sources.join(&sha), &archive).unwrap();
        let cross_device = |_: &Path, _: &Path| Err(io::Error::from_raw_os_error(libc::EXDEV));
        let pin = || pin_source(&sources, &sha, &dir, cross_device, Duration::ZERO).unwrap();
        let part = dir.join(format!("source.tar.{sha}.part"));
        let stray = dir.join(format!("source.tar.{}.part", "0".repeat(64)));
        fs::write(&stray, b"another copy").unwrap();

        // Each zero-budget request copies one megabyte, as a copy slower than the deadline would.
        assert!(!pin());
        assert!(!stray.exists());
        // An interrupted writer left garbage past what was synced.
        let mut torn = fs::read(&part).unwrap();
        torn.extend_from_slice(&[0xff; 4096]);
        fs::write(&part, &torn).unwrap();
        assert!(!pin());
        assert_eq!(fs::metadata(&part).unwrap().len(), 2 << 20);
        assert!(pin());
        assert_eq!(fs::read(dir.join("source.tar")).unwrap(), archive);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "a part was left behind"
        );
        assert!(pin());

        let other = root.join("runs/r2");
        fs::create_dir_all(&other).unwrap();
        let swept = pin_source(&sources, &"1".repeat(64), &other, cross_device, PIN_SLICE);
        assert_eq!(swept.unwrap_err().kind(), io::ErrorKind::NotFound);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn launch_cancel_and_cleanup_share_a_run_mutex_across_host_instances_and_path_aliases() {
        let root = std::env::temp_dir().join(format!("orx-host-{}", crate::nonce().unwrap()));
        let run = root.join("runs/r1");
        fs::create_dir_all(&run).unwrap();
        let binary = PathBuf::from("/usr/bin/true");
        let hosts: Vec<_> = (0..3)
            .map(|_| Host::new(root.clone(), binary.clone()).unwrap())
            .collect();
        let mutex = crate::run_mutex(&run).unwrap();
        let alias = crate::run_mutex(&run.join(".")).unwrap();
        assert!(Arc::ptr_eq(&mutex, &alias));
        let other = root.join("runs/other");
        fs::create_dir(&other).unwrap();
        let other = crate::run_mutex(&other).unwrap();
        let held = mutex.lock().unwrap();
        assert!(other.try_lock().is_ok());
        let requests = [
            Request::Launch(LaunchIntent {
                run: "r1".into(),
                digest: crate::sha(b"true"),
                run_sh: "true".into(),
                env: BTreeMap::new(),
                source: None,
                cancel_sh: None,
            }),
            Request::Cancel { run: "r1".into() },
            Request::Cleanup { run: "r1".into() },
        ];
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let (sent, received) = std::sync::mpsc::channel();
        let workers: Vec<_> = hosts
            .into_iter()
            .zip(requests)
            .map(|(host, request)| {
                let barrier = barrier.clone();
                let sent = sent.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    sent.send(host.handle(request)).unwrap();
                })
            })
            .collect();
        barrier.wait();
        let blocked = received.recv_timeout(Duration::from_millis(50)).is_err();
        drop(held);
        for worker in workers {
            worker.join().unwrap();
        }
        fs::remove_dir_all(root).unwrap();
        assert!(blocked, "a run mutation bypassed the in-process lock");
        assert_eq!(received.try_iter().count(), 3);
    }

    #[test]
    fn oversized_response_returns_an_error_for_the_same_id() {
        let mut output = Vec::new();
        write_response(
            &mut output,
            &Message {
                id: 17,
                body: Response::Error {
                    code: ErrorCode::Io,
                    message: "x".repeat(MAX_FRAME_LEN + 1),
                },
            },
        )
        .unwrap();
        let response: Message<Response> = read_frame(&mut output.as_slice()).unwrap().unwrap();
        assert_eq!(response.id, 17);
        assert!(
            matches!(response.body, Response::Error { message, .. } if message.contains("frame"))
        );
    }

    #[test]
    fn retained_scheduler_fact_preserves_its_disk_shape() {
        let fixture = include_str!("../tests/fixtures/slurm-terminal.json");
        let fact: SchedulerTerminal = serde_json::from_str(fixture).unwrap();
        assert_eq!(fact.job, "123");
        assert_eq!(fact.state, "PREEMPTED");
        assert_eq!(
            serde_json::to_value(&fact).unwrap(),
            serde_json::from_str::<serde_json::Value>(fixture).unwrap()
        );
    }

    #[test]
    fn scheduler_terminal_requires_visibility_grace_and_never_guesses_success() {
        assert!(matches!(
            scheduler_state("COMPLETED", Duration::from_secs(119)),
            RunState::Unreconciled { .. }
        ));
        assert!(
            matches!(scheduler_state("COMPLETED", Duration::from_secs(120)), RunState::Lost { reason } if reason == "no receipt")
        );
        assert!(matches!(
            scheduler_state("CANCELLED by 501", Duration::from_secs(120)),
            RunState::Cancelled { .. }
        ));
        assert!(matches!(
            scheduler_state("OUT_OF_MEMORY+", Duration::from_secs(120)),
            RunState::Failed { .. }
        ));
        for state in ["PREEMPTED", "NODE_FAIL"] {
            assert_eq!(
                scheduler_state(state, Duration::from_secs(120)),
                RunState::Failed {
                    reason: format!("scheduler {state}; no receipt"),
                }
            );
        }
        assert_eq!(
            scheduler_state("RUNNING", Duration::from_secs(500)),
            RunState::Running
        );
        assert_eq!(
            scheduler_state("PENDING", Duration::from_secs(500)),
            RunState::Starting { reason: None }
        );
    }
}
