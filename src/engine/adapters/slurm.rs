use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orx_host_proto::*;
use serde::Deserialize;
use tokio::sync::Mutex;

use super::super::clock::Clock;
use super::super::{
    Backend, CancelOutcome, Fence, Handle, LaunchError, Observation, Recovery, RunIntent,
    WatchedRun,
};
use super::api::interrupted;
use super::ssh::{observations, LaunchOptions, Ssh, CANCELLED, UNEXPECTED};
use crate::error::{bail, Result};

#[derive(Deserialize)]
struct Options {
    #[serde(default)]
    directives: BTreeMap<String, String>,
    /// The user's own `#SBATCH` arguments, written verbatim after `directives`.
    #[serde(default)]
    sbatch_args: Vec<String>,
}

#[derive(Default)]
struct Queue {
    next: Duration,
    jobs: HashMap<String, SlurmJob>,
}

pub struct Slurm {
    ssh: Arc<Ssh>,
    clock: Arc<dyn Clock>,
    queue: Mutex<Queue>,
}

impl Slurm {
    pub fn new(ssh: Arc<Ssh>, clock: Arc<dyn Clock>) -> Self {
        Self {
            ssh,
            clock,
            queue: Mutex::default(),
        }
    }

    fn handle(&self, job: Option<String>) -> Handle {
        Handle::Slurm {
            host: self.ssh.host.clone(),
            root: self.ssh.root.clone(),
            job,
        }
    }

    async fn submit(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let payload_options: LaunchOptions = serde_json::from_value(intent.launch.options.clone())
            .map_err(|error| LaunchError::Rejected {
                reason: error.to_string(),
            })?;
        let metadata =
            crate::jobs::BackendDescriptor::parse(&intent.run.backend_json).map_err(|error| {
                LaunchError::Rejected {
                    reason: error.to_string(),
                }
            })?;
        if payload_options.cancel_sh.is_some()
            || payload_options.container.is_some()
            || metadata.ssh_container.is_some()
        {
            return Err(LaunchError::Rejected {
                reason: "Slurm container runs are not supported. Use SSH for container runs."
                    .into(),
            });
        }
        let payload = self.ssh.payload(intent, &|| fence.cancelled()).await?;
        if fence.cancelled() {
            return Err(interrupted(CANCELLED));
        }
        let options: Options =
            serde_json::from_value(intent.launch.options.clone()).map_err(|error| {
                LaunchError::Rejected {
                    reason: error.to_string(),
                }
            })?;
        let mut directives = String::new();
        for (name, value) in options.directives {
            if !matches!(
                name.as_str(),
                "partition"
                    | "ntasks"
                    | "account"
                    | "qos"
                    | "gres"
                    | "cpus-per-task"
                    | "mem"
                    | "time"
                    | "nodes"
                    | "nodelist"
                    | "constraint"
                    | "exclude"
            ) || value.is_empty()
                || value.contains(['\r', '\n', '\0'])
            {
                return Err(LaunchError::Rejected {
                    reason: format!("Invalid Slurm directive {name}"),
                });
            }
            directives.push_str(&format!("#SBATCH --{name}={value}\n"));
        }
        for arg in options.sbatch_args {
            if !arg.starts_with('-') || arg.contains(['\r', '\n', '\0']) {
                return Err(LaunchError::Rejected {
                    reason: format!("Invalid Slurm argument {arg:?}"),
                });
            }
            directives.push_str(&format!("#SBATCH {arg}\n"));
        }
        match self
            .ssh
            .launch_request(Request::SlurmSubmit {
                intent: payload,
                directives,
            })
            .await?
        {
            Response::SlurmSubmit { job } => Ok(self.handle(Some(job))),
            _ => Err(interrupted(UNEXPECTED)),
        }
    }

    async fn query(
        &self,
        names: Vec<String>,
        legacy: Vec<SlurmLegacyRun>,
    ) -> Result<Vec<SlurmJob>> {
        match self
            .ssh
            .request(Request::SlurmQueue { names, legacy })
            .await?
        {
            Response::SlurmQueue { jobs, .. } => Ok(jobs),
            response => bail!("Unexpected Slurm queue reply: {response:?}"),
        }
    }
}

#[async_trait]
impl Backend for Slurm {
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        self.submit(intent, fence).await
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
        match self
            .ssh
            .request(Request::Inspect {
                run: intent.run.id.clone(),
            })
            .await
        {
            Ok(Response::Inspect { summary })
                if matches!(summary.state, RunState::Missing | RunState::Staging) =>
            {
                return Recovery::Absent
            }
            Ok(Response::Inspect { .. }) => {}
            Ok(response) => {
                return Recovery::Unknown {
                    detail: format!("Unexpected inspection reply: {response:?}"),
                }
            }
            Err(error) => return self.ssh.unanswered(error),
        }
        match self.query(vec![intent.run.id.clone()], Vec::new()).await {
            Ok(jobs) => match jobs.into_iter().find(|job| {
                job.run == intent.run.id && (job.state != "UNKNOWN" || !job.job.is_empty())
            }) {
                Some(job) => {
                    let id = (!job.job.is_empty()).then_some(job.job);
                    Recovery::Found(self.handle(id))
                }
                // The host cancels whatever the submission left: scancel by the run's name, and
                // a cancelled receipt once no wrapper holds the run, which then ends it.
                None if cancelled() => match self
                    .ssh
                    .request(Request::Cancel {
                        run: intent.run.id.clone(),
                    })
                    .await
                {
                    Ok(Response::Cancel { .. }) => Recovery::Found(self.handle(None)),
                    Ok(response) => Recovery::Unknown {
                        detail: format!("Unexpected cancel reply: {response:?}"),
                    },
                    Err(error) => self.ssh.unanswered(error),
                },
                None => Recovery::Unknown {
                    detail: "Slurm has no record of this run's submission and no receipt; it \
                             stays unconfirmed until the scheduler or a receipt answers. If the \
                             job is gone for good, abandon the run."
                        .into(),
                },
            },
            Err(error) => self.ssh.unanswered(error),
        }
    }

    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)> {
        let mut result = Vec::new();
        let mut names = Vec::new();
        let mut legacy = Vec::new();
        for run in runs {
            match self.ssh.validate(run) {
                Ok(()) => {
                    names.push(run.id.clone());
                    if let Handle::Legacy { descriptor } = &run.handle {
                        if let Some(job) = &descriptor.job_id {
                            legacy.push(SlurmLegacyRun {
                                run: run.id.clone(),
                                job: job.clone(),
                                dir: super::ssh::legacy_dir(&crate::jobs::slurm::run_dir(
                                    descriptor.remote_root.as_deref(),
                                    &run.id,
                                )),
                            });
                        }
                    }
                }
                Err(error) => result.push((
                    run.id.clone(),
                    Observation::Unreconciled {
                        detail: error.to_string(),
                    },
                )),
            }
        }
        if names.is_empty() {
            return result;
        }
        let mut queue = self.queue.lock().await;
        let now = self.clock.monotonic();
        let due = now >= queue.next || names.iter().any(|name| !queue.jobs.contains_key(name));
        if due {
            match self.query(names.clone(), legacy.clone()).await {
                Ok(jobs) => {
                    queue.jobs = jobs.into_iter().map(|job| (job.run.clone(), job)).collect();
                    queue.next = now + Duration::from_secs(30);
                }
                Err(error) => {
                    result.extend(
                        names
                            .into_iter()
                            .map(|name| (name, self.ssh.disconnected(error.to_string()))),
                    );
                    return result;
                }
            }
        }
        let wait = if due {
            Duration::ZERO
        } else {
            budget.min(queue.next.saturating_sub(now))
        };
        match self.ssh.changes(&names, wait).await {
            Ok(changed) => {
                for (name, mut summary) in changed {
                    let receipt = matches!(
                        &summary.state,
                        RunState::Exited { .. }
                            | RunState::InvalidData { .. }
                            | RunState::Unreachable { .. }
                    );
                    if !receipt {
                        if let Some(job) = queue.jobs.get(&name) {
                            summary.state = job.summary.state.clone();
                            if legacy.iter().any(|old| old.run == name) {
                                summary.notify_len = job.summary.notify_len;
                            }
                        } else {
                            summary.state = RunState::Unreconciled {
                                detail: "Scheduler returned no job".into(),
                            };
                        }
                    }
                    if !receipt && matches!(summary.state, RunState::Failed { .. }) {
                        result.push((name.clone(), Observation::Lingering { pids: Vec::new() }));
                    }
                    let mut observed = observations(name.clone(), &summary);
                    if matches!(&summary.state, RunState::Exited { receipt }
                        if receipt.reason != orx_host_proto::ExitReason::Cancelled)
                    {
                        observed.retain(|(_, observation)| {
                            !matches!(observation, Observation::Lingering { .. })
                        });
                        observed.push((name, Observation::Lingering { pids: Vec::new() }));
                    }
                    result.extend(observed);
                }
            }
            Err(error) => result.extend(
                names
                    .into_iter()
                    .map(|name| (name, self.ssh.disconnected(error.to_string()))),
            ),
        }
        result
    }

    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        if let Err(error) = self.ssh.validate(run) {
            return CancelOutcome::Undelivered {
                detail: error.to_string(),
            };
        }
        let request = match &run.handle {
            Handle::Legacy { descriptor } => Request::SlurmCancelLegacy {
                job: descriptor.job_id.clone().unwrap_or_default(),
            },
            _ => Request::Cancel {
                run: run.id.clone(),
            },
        };
        if let Err(error) = self.ssh.request(request).await {
            return CancelOutcome::Undelivered {
                detail: error.to_string(),
            };
        }

        self.queue.lock().await.next = Duration::ZERO;
        CancelOutcome::Pending
    }

    async fn cleanup(&self, run: &WatchedRun) -> CancelOutcome {
        if matches!(run.handle, Handle::Legacy { .. }) {
            return self.cancel(run).await;
        }
        Backend::cleanup(self.ssh.as_ref(), run).await
    }

    async fn unwatch(&self, run: &WatchedRun) {
        self.queue.lock().await.jobs.remove(&run.id);
        self.ssh.unwatch(run).await;
    }

    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage> {
        self.ssh.read_log(run, file, range, ident).await
    }
}
