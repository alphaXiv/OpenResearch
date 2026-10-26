//! Direct SSH observations. Scheduling belongs to the supervisor or its caller.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{service, SshTarget};
use crate::error::{anyhow, Result};
use crate::jobs::{is_terminal_stage, observed_run_status, BackendDescriptor};
use crate::store::{log_path, now_ms, RunStatus, Store, StoredRun};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const LOG_CHUNKS: usize = 4;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    #[default]
    Auto,
    OnDemand,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Monitoring {
    pub mode: Mode,
    pub last_checked_at: Option<i64>,
    pub last_observed_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_log_sync_at: Option<i64>,
    pub logs_complete: bool,
    pub missing_checks: u32,
}

impl Monitoring {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }
}

pub fn direct(descriptor: &BackendDescriptor) -> bool {
    descriptor.kind == "ssh_job"
        && descriptor.ssh_container.is_none()
        && descriptor.rental_lease.is_none()
}

pub fn on_demand(descriptor: &BackendDescriptor) -> bool {
    direct(descriptor)
        && descriptor
            .monitoring
            .as_ref()
            .is_some_and(|m| m.mode == Mode::OnDemand)
}

pub fn automatic_intervals(descriptor: &BackendDescriptor) -> (Duration, Duration) {
    match descriptor
        .monitor_interval_secs
        .filter(|_| direct(descriptor))
    {
        Some(seconds) => (
            Duration::from_secs(seconds.max(1)),
            Duration::from_secs(seconds.max(1)),
        ),
        None => (Duration::from_secs(5), Duration::from_secs(2)),
    }
}

fn handle(run: &StoredRun) -> Result<(BackendDescriptor, SshTarget, String)> {
    let descriptor = BackendDescriptor::parse(&run.backend_json)?;
    if !direct(&descriptor) {
        return Err(anyhow!(
            "On-demand refresh currently supports direct SSH runs only."
        ));
    }
    let (host, dir) = descriptor.ssh_ref()?;
    let target = SshTarget::alias(host).for_run(&run.id);
    let dir = dir.to_string();
    Ok((descriptor, target, dir))
}

fn record_error(store: &Store, id: &str, error: &str) -> Result<()> {
    store.patch_backend(id, |descriptor| {
        let m = descriptor
            .monitoring
            .get_or_insert_with(Monitoring::default);
        m.last_checked_at = Some(now_ms());
        m.last_error = Some(error.to_owned());
    })
}

/// One bounded control attempt. A saved intent survives failure or caller exit.
pub async fn deliver_cancel(run_id: &str) -> Result<()> {
    let store = Store::open()?;
    let run = crate::compute::load_run_with_submission_handle(&store, run_id)?;
    let (descriptor, target, dir) = handle(&run)?;
    if crate::local::is_terminal(&run.status)
        || !run.cancel_requested
        || descriptor.cancellation_accepted
    {
        return Ok(());
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(log_path(run_id).with_extension("ssh-control.lock"))?;
    let mut lock = fd_lock::RwLock::new(file);
    let Ok(_guard) = lock.try_write() else {
        return Ok(());
    };
    // Recheck after acquiring the lock: another caller may have delivered it.
    let current = store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("Run {run_id} not found."))?;
    if crate::local::is_terminal(&current.status)
        || BackendDescriptor::parse(&current.backend_json)?.cancellation_accepted
    {
        return Ok(());
    }
    let result = tokio::time::timeout(REQUEST_TIMEOUT, super::cancel_job(&target, &dir, None))
        .await
        .map_err(|_| anyhow!("SSH cancellation timed out; cancellation remains pending."))
        .and_then(|result| result);
    match result {
        Ok(()) => store.patch_backend(run_id, |d| d.cancellation_accepted = true),
        Err(error) => {
            record_error(&store, run_id, &error.to_string())?;
            Err(error)
        }
    }
}

/// Refresh status once, optionally copying at most 1 MiB of log data in 20s.
/// The observation lock is separate from control, so slow reads cannot block cancellation.
pub async fn refresh(run_id: &str, collect_logs: bool) -> Result<StoredRun> {
    let store = Store::open()?;
    let run = crate::compute::load_run_with_submission_handle(&store, run_id)?;
    let (descriptor, target, dir) = handle(&run)?;
    if crate::local::is_terminal(&run.status)
        && descriptor
            .monitoring
            .as_ref()
            .is_some_and(|m| m.logs_complete)
    {
        return Ok(run);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(log_path(run_id).with_extension("observation.lock"))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock
        .try_write()
        .map_err(|_| anyhow!("A refresh for this run is already in progress."))?;
    if !crate::local::is_terminal(&run.status) {
        deliver_cancel(run_id).await?;
        let result =
            tokio::time::timeout(REQUEST_TIMEOUT, super::inspect_direct_job(&target, &dir))
                .await
                .map_err(|_| {
                    anyhow!("SSH status check timed out; the last observed outcome is unchanged.")
                })
                .and_then(|result| result);
        let (job, exit_code) = match result {
            Ok(job) => job,
            Err(error) => {
                record_error(&store, run_id, &error.to_string())?;
                return Err(error);
            }
        };
        let missing = job.message.as_deref() == Some(super::RUN_DIR_MISSING);
        store.patch_backend(run_id, |d| {
            let m = d.monitoring.get_or_insert_with(Monitoring::default);
            m.last_checked_at = Some(now_ms());
            m.missing_checks = if missing {
                m.missing_checks.saturating_add(1)
            } else {
                0
            };
            if !missing || m.missing_checks >= 3 {
                m.last_observed_at = m.last_checked_at;
                m.last_error = None;
            } else {
                m.last_error = Some(
                    "Remote run directory is missing; refresh again to confirm its outcome.".into(),
                );
            }
            if !is_terminal_stage(&job.stage) {
                m.logs_complete = false;
            }
        })?;
        let current = store
            .get_run(run_id)?
            .ok_or_else(|| anyhow!("Run {run_id} not found."))?;
        let d = BackendDescriptor::parse(&current.backend_json)?;
        if !missing || d.monitoring.as_ref().is_some_and(|m| m.missing_checks >= 3) {
            let status = observed_run_status(
                &job.stage,
                current.cancel_requested,
                d.cancellation_accepted,
            );
            let applied = store.update_status(
                run_id,
                status,
                is_terminal_stage(&job.stage).then(now_ms),
                exit_code,
            )?;
            if applied && status == RunStatus::Failed {
                if let Some(message) = job.message {
                    store.set_result_markdown(run_id, &format!("Job failed: {message}"))?;
                }
            }
        }
    }
    if collect_logs {
        collect(run_id, &target, &dir).await?;
    }
    store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("Run {run_id} not found."))
}

async fn collect(run_id: &str, target: &SshTarget, dir: &str) -> Result<()> {
    let result = tokio::time::timeout(Duration::from_secs(20), async {
        for _ in 0..LOG_CHUNKS {
            let value = service::request(
                target,
                serde_json::json!({"op":"collect", "run":run_id, "dir":dir}),
            )
            .await?;
            if value["eof"] == true {
                return Ok::<_, crate::error::Error>(true);
            }
        }
        Ok(false)
    })
    .await
    .map_err(|_| anyhow!("Log collection paused after 20 seconds; refresh again to continue."))
    .and_then(|result| result);
    let store = Store::open()?;
    let eof = match result {
        Ok(eof) => eof,
        Err(error) => {
            record_error(&store, run_id, &error.to_string())?;
            return Err(error);
        }
    };
    let terminal = store
        .get_run(run_id)?
        .is_some_and(|r| crate::local::is_terminal(&r.status));
    store.patch_backend(run_id, |d| {
        let m = d.monitoring.get_or_insert_with(Monitoring::default);
        m.last_log_sync_at = Some(now_ms());
        m.logs_complete = terminal && eof;
        m.last_error = None;
    })?;
    if terminal && eof {
        store.queue_finished_remote_cleanup(run_id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_monitoring_scope_preserves_container_rental_and_slurm_intervals() {
        for json in [
            r#"{"kind":"ssh_job","monitorIntervalSecs":600,"sshContainer":{"reference":"research","id":"id","startedAt":"now","runDir":"/tmp/run"}}"#,
            r#"{"kind":"ssh_job","monitorIntervalSecs":600,"rentalLease":{"apiUrl":"http://unused","sandboxId":"id","token":"fixture"}}"#,
            r#"{"kind":"slurm_job","monitorIntervalSecs":600}"#,
        ] {
            let d = BackendDescriptor::parse(json).unwrap();
            assert!(!direct(&d));
            assert_eq!(
                automatic_intervals(&d),
                (Duration::from_secs(5), Duration::from_secs(2))
            );
        }
    }

    #[test]
    fn older_runs_keep_automatic_defaults_and_modes_survive_restart() {
        let mut d = BackendDescriptor::parse(
            r#"{"kind":"ssh_job","namespace":"lab","jobId":".orx/runs/id"}"#,
        )
        .unwrap();
        assert!(!on_demand(&d));
        assert_eq!(
            automatic_intervals(&d),
            (Duration::from_secs(5), Duration::from_secs(2))
        );
        d.monitor_interval_secs = Some(600);
        assert_eq!(
            automatic_intervals(&d),
            (Duration::from_secs(600), Duration::from_secs(600))
        );
        d.monitoring = Some(Monitoring::new(Mode::OnDemand));
        let restored = BackendDescriptor::parse(&d.to_json()).unwrap();
        assert!(on_demand(&restored));
        d.kind = "slurm_job".into();
        assert!(!on_demand(&d));
        assert!(!direct(&d));
    }
}
