use std::fs::{self, OpenOptions};
#[cfg(windows)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;

use super::super::{
    Backend, ByteRange, CancelOutcome, Fence, FileIdent, Handle, LaunchError, LogFile, LogPage,
    Observation, Recovery, RunIntent, WatchedRun,
};
use super::ssh::LaunchOptions;
use crate::error::{anyhow, bail, Result};
use crate::jobs::localbox;

#[path = "../../../crates/orx-host/src/log.rs"]
pub(super) mod log;

pub struct Local {
    root: PathBuf,
}

impl Local {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn directory(&self, intent: &RunIntent) -> Result<PathBuf> {
        if intent.run.id.is_empty()
            || !intent
                .run
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            bail!("Invalid local run id");
        }
        Ok(self.root.join(&intent.run.id))
    }

    fn handle(&self, intent: &RunIntent, dir: &Path) -> Handle {
        let dir = dir.to_string_lossy().into_owned();
        if matches!(intent.launch.kind.as_str(), "tinker" | "tinker_job") {
            Handle::Tinker { dir }
        } else {
            Handle::Local { dir }
        }
    }

    fn dir<'a>(&self, run: &'a WatchedRun) -> Result<&'a Path> {
        let dir = match &run.handle {
            Handle::Local { dir } | Handle::Tinker { dir } => dir,
            Handle::Legacy { descriptor }
                if matches!(descriptor.kind.as_str(), "local_job" | "tinker_job") =>
            {
                descriptor
                    .job_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("Local directory missing"))?
            }
            _ => bail!("Run {} is not local", run.id),
        };
        Ok(Path::new(dir))
    }

    pub(crate) fn cleanup(&self, run: &WatchedRun) -> Result<orx_host_proto::CleanupAck> {
        #[cfg(unix)]
        return Ok(orx_host::wrapper::cleanup(self.dir(run)?)?);
        #[cfg(windows)]
        {
            localbox::cancel_job(self.dir(run)?)?;
            Ok(orx_host_proto::CleanupAck::NothingLingering)
        }
    }

    async fn start(
        &self,
        intent: &RunIntent,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<Handle> {
        #[cfg(windows)]
        let _ = cancelled;
        let dir = self.directory(intent)?;
        let options: LaunchOptions = serde_json::from_value(intent.launch.options.clone())?;
        fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
        }
        let ownership = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("launch.lock"))?;
        ownership.lock()?;
        let launch = dir.join("launch.json");
        #[cfg(unix)]
        let existing = match orx_host::read_json::<orx_host_proto::LaunchIntent>(&launch) {
            Ok(existing)
                if existing.run == intent.run.id && existing.digest == intent.spec_digest =>
            {
                Some((existing.run_sh, existing.env))
            }
            Ok(_) => bail!("Local launch intent differs"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        #[cfg(windows)]
        match fs::read_to_string(&launch) {
            Ok(digest) if digest != intent.spec_digest => bail!("Local launch intent differs"),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let temporary = dir.join(format!("launch-{}.tmp", uuid::Uuid::new_v4()));
                let mut file = std::fs::File::create(&temporary)?;
                file.write_all(intent.spec_digest.as_bytes())?;
                file.sync_all()?;
                let published = fs::hard_link(&temporary, &launch);
                fs::remove_file(temporary)?;
                published?;
            }
            Err(error) => return Err(error.into()),
        }
        if dir.join("started").try_exists()?
            || dir.join("exit_code").try_exists()?
            || dir.join("exit.json").try_exists()?
        {
            return Ok(self.handle(intent, &dir));
        }
        let prepare = || -> Result<(String, std::collections::BTreeMap<String, String>)> {
            let payload = match options.run_sh {
                Some(script) => script,
                None => {
                    let metadata = crate::jobs::BackendDescriptor::parse(&intent.run.backend_json)?;
                    match metadata.source_path {
                        Some(path) => crate::compute::snapshot_script(
                            &crate::local::bash::bash_path(Path::new(&path)),
                            &crate::local::bash::local_run_command(&intent.run.command),
                        ),
                        None => intent.run.command.clone(),
                    }
                }
            };
            let mut env = crate::config::launch_env(&intent.run.id);
            env.insert(
                crate::compute::TENSORBOARD_LOGDIR.into(),
                dir.join("tensorboard").to_string_lossy().into_owned(),
            );
            env.extend(crate::local::trackio::local_run_env());
            crate::local::shell_env::export_to(|key, value| {
                env.insert(key.into(), value.to_string_lossy().into_owned());
            });
            env.extend(options.env);
            if matches!(intent.launch.kind.as_str(), "tinker" | "tinker_job") {
                env.insert(
                    crate::jobs::tinker::API_KEY_ENV.into(),
                    crate::jobs::tinker::resolve_api_key()?,
                );
            }
            let mut env = crate::jobs::default_python_env(&env);
            if let Some(path) =
                crate::local::bash::path_with_toolchain(crate::local::shell_env::search_path())
            {
                env.entry("PATH".into())
                    .or_insert_with(|| path.to_string_lossy().into_owned());
            }
            Ok((payload, env.into_iter().collect()))
        };
        #[cfg(unix)]
        let (payload, env) = match existing {
            Some(accepted) => accepted,
            None => prepare()?,
        };
        #[cfg(windows)]
        let (payload, env) = prepare()?;
        #[cfg(unix)]
        {
            if !launch.try_exists()? {
                orx_host::publish(
                    &launch,
                    &orx_host_proto::LaunchIntent {
                        run: intent.run.id.clone(),
                        digest: intent.spec_digest.clone(),
                        run_sh: payload.clone(),
                        env: env.clone(),
                        source: None,
                        cancel_sh: None,
                    },
                )?;
            }
            if !dir.join("run.sh").try_exists()? {
                orx_host::publish_bytes(&dir.join("run.sh"), payload.as_bytes())?;
            }
            let binary = crate::engine::transport::local_binary(&self.root)?;
            if cancelled() {
                orx_host::wrapper::cancel(&dir)?;
                return Ok(self.handle(intent, &dir));
            }
            let diagnostics = OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("wrapper.log"))?;
            let status = crate::process::command(crate::local::bash::program())
                .args(["-c", "\"$@\" &", "orx-local"])
                .arg(binary)
                .arg("wrap")
                .arg(crate::paths::canonicalize(&dir)?)
                .envs(&env)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(diagnostics)
                .status()?;
            if !status.success() {
                bail!("Could not detach the local wrapper: {status}");
            }
            drop(ownership);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while !dir.join("started").try_exists()? && !dir.join("exit.json").try_exists()? {
                if cancelled() {
                    orx_host::wrapper::cancel(&dir)?;
                    return Ok(self.handle(intent, &dir));
                }
                if tokio::time::Instant::now() >= deadline {
                    bail!(
                        "Local supervisor start is unconfirmed; see {}",
                        dir.join("wrapper.log").display()
                    );
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        #[cfg(windows)]
        {
            let payload = format!("{}{}", localbox::python::prelude(&dir)?, payload);
            fs::write(dir.join("payload.sh"), payload)?;
            let script = "#!/usr/bin/env bash\nread -r ready || exit 98\nbash payload.sh > log 2>&1\ncode=$?\nprintf '%s\\n' \"$code\" > exit_code.part\nmv exit_code.part exit_code\nexit \"$code\"\n";
            fs::write(dir.join("run.sh"), script)?;
            let script_path = crate::paths::canonicalize(dir.join("run.sh"))?;
            fs::write(
                dir.join("pid_script"),
                script_path.to_string_lossy().as_bytes(),
            )?;
            let mut command = crate::process::command(crate::local::bash::program());
            command
                .arg(crate::local::bash::bash_path(&script_path))
                .envs(&env)
                .env("ORX_NOTIFY_FILE", dir.join("notify"))
                .current_dir(&dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut child = localbox::windows::spawn(&mut command, &dir)?;
            let released = (|| -> Result<()> {
                fs::write(dir.join("pid"), child.id().to_string())?;
                let started = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dir.join("started"))?;
                started.sync_all()?;
                child
                    .stdin
                    .take()
                    .ok_or_else(|| anyhow!("Local launch gate missing"))?
                    .write_all(b"start\n")?;
                Ok(())
            })();
            if let Err(error) = released {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Ok(self.handle(intent, &dir))
    }
}

#[async_trait]
impl Backend for Local {
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        self.directory(intent)
            .map_err(|error| LaunchError::Rejected {
                reason: error.to_string(),
            })?;
        match self.start(intent, &|| fence.cancelled()).await {
            Ok(handle) => Ok(handle),
            Err(error) => match self.recover(intent).await {
                Recovery::Absent => Err(LaunchError::Rejected {
                    reason: error.to_string(),
                }),
                _ => Err(super::api::interrupted(error)),
            },
        }
    }

    async fn recover(&self, intent: &RunIntent) -> Recovery {
        self.recover_with_cancel(intent, &|| intent.run.cancel_requested)
            .await
    }

    async fn recover_with_cancel(
        &self,
        intent: &RunIntent,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Recovery {
        #[cfg(windows)]
        let _ = cancelled;
        let dir = match self.directory(intent) {
            Ok(dir) => dir,
            Err(error) => {
                return Recovery::Unknown {
                    detail: error.to_string(),
                }
            }
        };
        #[cfg(unix)]
        return match orx_host::read_json::<orx_host_proto::LaunchIntent>(&dir.join("launch.json")) {
            Ok(accepted)
                if accepted.run != intent.run.id || accepted.digest != intent.spec_digest =>
            {
                Recovery::Unknown {
                    detail: "Local launch intent differs".into(),
                }
            }
            Ok(_) if cancelled() => Recovery::Found(self.handle(intent, &dir)),
            Ok(_) => match self.start(intent, cancelled).await {
                Ok(handle) => Recovery::Found(handle),
                Err(error) => Recovery::Unknown {
                    detail: error.to_string(),
                },
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Recovery::Absent,
            Err(error) => Recovery::Unknown {
                detail: error.to_string(),
            },
        };
        #[cfg(windows)]
        match fs::read_to_string(dir.join("launch.json")) {
            Ok(digest) if digest != intent.spec_digest => Recovery::Unknown {
                detail: "Local launch intent differs".into(),
            },
            Ok(_) if dir.join("started").exists() || dir.join("exit_code").exists() => {
                Recovery::Found(self.handle(intent, &dir))
            }
            Ok(_) => Recovery::Absent,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Recovery::Absent,
            Err(error) => Recovery::Unknown {
                detail: error.to_string(),
            },
        }
    }

    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)> {
        if !runs.is_empty() {
            tokio::time::sleep(budget).await;
        }
        let mut result = Vec::new();
        for run in runs {
            let dir = match self.dir(run) {
                Ok(dir) => dir,
                Err(error) => {
                    let detail = error.to_string();
                    result.push((run.id.clone(), Observation::Unreconciled { detail }));
                    continue;
                }
            };
            #[cfg(unix)]
            if !matches!(run.handle, Handle::Legacy { .. }) {
                let summary = (|| -> std::io::Result<orx_host_proto::Summary> {
                    let len = |file: &str| match fs::metadata(dir.join(file)) {
                        Ok(metadata) => Ok(metadata.len()),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
                        Err(error) => Err(error),
                    };
                    Ok(orx_host_proto::Summary {
                        started_at: orx_host::wrapper::started_at(dir),
                        state: orx_host::wrapper::state(dir)?,
                        log_len: Some(len("log")?),
                        notify_len: len("notify")?,
                        cleanup: orx_host::wrapper::cleanup_result(dir)?,
                    })
                })();
                match summary {
                    Ok(summary) => {
                        result.extend(super::ssh::observations(run.id.clone(), &summary))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::InvalidData => result.push((
                        run.id.clone(),
                        Observation::Unreconciled {
                            detail: error.to_string(),
                        },
                    )),
                    Err(error) => result.push((
                        run.id.clone(),
                        Observation::Unreachable {
                            cause: error.to_string(),
                        },
                    )),
                }
                continue;
            }
            #[cfg(unix)]
            if orx_host::legacy::inspect(dir)
                .is_ok_and(|inspection| inspection.pid.is_some() && !inspection.alive)
            {
                result.push((run.id.clone(), Observation::Lingering { pids: vec![] }));
            }
            let job = localbox::inspect_job(dir);
            let observation = if let Some(code) = job.exit_code {
                Observation::Exited {
                    finished_at: None,
                    code: Some(code),
                    signal: None,
                }
            } else if job.stage == "ERROR" && !localbox::process_group_alive(dir) {
                Observation::Failed {
                    finished_at: None,
                    reason: job
                        .message
                        .unwrap_or_else(|| "Local process died without an exit code".into()),
                }
            } else {
                // A killed launcher whose payload survives in its group is still running.
                Observation::Running { started_at: None }
            };
            result.push((run.id.clone(), observation));
            let len = |file| fs::metadata(dir.join(file)).map_or(0, |metadata| metadata.len());
            let (log, notify) = (len("log"), len("notify"));
            if log > 0 {
                result.push((run.id.clone(), Observation::Log { len: log }));
            }
            if notify > 0 {
                result.push((run.id.clone(), Observation::Notify { len: notify }));
            }
        }
        result
    }

    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        let dir = match self.dir(run) {
            Ok(dir) => dir.to_path_buf(),
            Err(error) => {
                return CancelOutcome::Undelivered {
                    detail: error.to_string(),
                }
            }
        };
        #[cfg(unix)]
        if !matches!(run.handle, Handle::Legacy { .. }) {
            return match orx_host::wrapper::cancel(&dir) {
                Ok(_) => CancelOutcome::Pending,
                Err(error) => CancelOutcome::Undelivered {
                    detail: format!("Local cancel: {error}"),
                },
            };
        }
        let result = tokio::task::spawn_blocking(move || -> Result<CancelOutcome> {
            localbox::cancel_job(&dir)?;
            #[cfg(unix)]
            {
                let pid: i32 = fs::read_to_string(dir.join("pid"))?.trim().parse()?;
                if pid <= 1 {
                    bail!("Invalid local process group");
                }
                for poll in 0..110 {
                    if !localbox::process_group_alive(&dir) {
                        return Ok(CancelOutcome::Verified);
                    }
                    if poll == 100 {
                        // The still-existing group retains its id after its leader exits.
                        unsafe {
                            libc::kill(-pid, libc::SIGKILL);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let output = crate::process::command("ps")
                    .args(["-axo", "pgid=,stat="])
                    .output()?;
                if !output.status.success() {
                    bail!("Could not inspect the local process group");
                }
                let remaining = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| {
                        let mut fields = line.split_whitespace();
                        fields.next().and_then(|group| group.parse::<i32>().ok()) == Some(pid)
                            && fields.next().is_some_and(|state| !state.starts_with('Z'))
                    })
                    .count();
                Ok(if remaining == 0 {
                    CancelOutcome::Verified
                } else {
                    CancelOutcome::Survivors { count: remaining }
                })
            }
            #[cfg(windows)]
            Ok(CancelOutcome::Verified)
        })
        .await;
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => CancelOutcome::Undelivered {
                detail: format!("Local cancel: {error}"),
            },
            Err(error) => CancelOutcome::Undelivered {
                detail: format!("Local cancel: {error}"),
            },
        }
    }

    async fn cleanup(&self, run: &WatchedRun) -> CancelOutcome {
        let outcome = self.cancel(run).await;
        if matches!(run.handle, Handle::Legacy { .. })
            || matches!(
                outcome,
                CancelOutcome::Verified | CancelOutcome::Undelivered { .. }
            )
        {
            return outcome;
        }
        match Local::cleanup(self, run) {
            Ok(_) => outcome,
            Err(error) if error.to_string() == "run has not finished" => CancelOutcome::Pending,
            Err(error) => CancelOutcome::Undelivered {
                detail: error.to_string(),
            },
        }
    }

    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage> {
        let dir = self.dir(run)?;
        let path = dir.join(match file {
            LogFile::Log => "log",
            LogFile::Notify => "notify",
        });
        Ok(log::read(&path, ident, &range, None)?)
    }
}
