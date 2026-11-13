pub mod adapters;
pub mod bus;
pub mod clock;
pub mod handle;
pub mod legacy;
pub mod locks;
pub mod notify;
pub mod process;
pub mod reconcile;
pub mod runtime;
pub mod ssh_config_cleanup;
pub mod submit;
pub mod transport;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::OwnedMutexGuard;

use crate::error::{anyhow, Result};
use crate::jobs::BackendDescriptor;
use crate::store::{Contact, EngineRunState, LaunchSpec, Store, StoredRun, SubmissionFence};
use bus::Bus;
use clock::Clock;
use handle::Handle;
use locks::RunLocks;
pub use orx_host_proto::{ByteRange, FileIdent, LogFile, LogPage};

/// `runs.status`. Phase only moves forward: Submitting → Starting → Running → one terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Submitting,
    Starting,
    Running,
    Done,
    Failed,
    Cancelled,
    Lost,
}

impl Phase {
    pub fn parse(status: &str) -> Option<Self> {
        Some(match status {
            "submitting" => Self::Submitting,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "lost" => Self::Lost,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitting => "submitting",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Submitting => 0,
            Self::Starting => 1,
            Self::Running => 2,
            Self::Done | Self::Failed | Self::Cancelled | Self::Lost => 3,
        }
    }

    pub fn is_terminal(self) -> bool {
        self.rank() == 3
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    NotFound,
    Gone {
        reason: String,
    },
    /// Not running yet; `detail` is the backend's own word on why, if it gave one.
    Queued {
        detail: Option<String>,
    },
    Running {
        started_at: Option<i64>,
    },
    Exited {
        finished_at: Option<i64>,
        code: Option<i32>,
        signal: Option<i32>,
    },
    Cancelled {
        finished_at: Option<i64>,
        code: Option<i32>,
        signal: Option<i32>,
    },
    CancelReceipt {
        finished_at: Option<i64>,
        code: Option<i32>,
        remaining: usize,
        note: Option<Vec<u32>>,
    },
    Failed {
        finished_at: Option<i64>,
        reason: String,
    },
    Lost {
        reason: String,
    },
    Unreachable {
        cause: String,
    },
    NeedsAuth {
        cause: String,
    },
    Unreconciled {
        detail: String,
    },
    Notify {
        len: u64,
    },
    /// The log length of a followed or local run.
    Log {
        len: u64,
    },
    /// Latest process proof; an empty list settles a tombstone.
    Lingering {
        pids: Vec<u32>,
    },
}

impl Observation {
    pub fn ends_run(&self) -> bool {
        matches!(
            self,
            Self::Exited { .. }
                | Self::Cancelled { .. }
                | Self::CancelReceipt { .. }
                | Self::Failed { .. }
                | Self::Lost { .. }
                | Self::Gone { .. }
        )
    }
}

#[derive(Debug)]
pub enum Recovery {
    Found(Handle),
    /// Proven: no job exists for this attempt and none can appear.
    Absent,
    Unknown {
        detail: String,
    },
}

#[derive(Debug)]
pub enum CancelOutcome {
    Verified,
    Survivors {
        count: usize,
    },
    /// Accepted; the outcome arrives as an observation.
    Pending,
    /// No acknowledgement; retain the intent for another delivery attempt.
    Undelivered {
        detail: String,
    },
}

#[derive(Debug)]
pub enum LaunchError {
    /// Proven: nothing was created.
    Rejected { reason: String },
    /// No launch was sent; contact must recover before retrying.
    Blocked { contact: crate::store::Contact },
    /// The outcome is unknown; recovery decides.
    Interrupted { detail: String },
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::store::Contact;
        match self {
            Self::Rejected { reason: text } | Self::Interrupted { detail: text } => {
                f.write_str(text)
            }
            Self::Blocked {
                contact: Contact::Unreachable { cause, .. } | Contact::NeedsAuth { cause, .. },
            } => f.write_str(cause),
            Self::Blocked { .. } => f.write_str("The launch was not sent; it is retried."),
        }
    }
}

#[derive(Debug)]
pub struct RunIntent {
    pub run: StoredRun,
    pub launch: LaunchSpec,
    pub spec_digest: String,
    /// Lets fakes report which attempt they were asked to launch or recover.
    #[cfg(test)]
    pub attempt: u32,
}

impl RunIntent {
    fn new(run: StoredRun, state: &EngineRunState) -> Result<Self> {
        let launch = state
            .launch
            .clone()
            .ok_or_else(|| anyhow!("Run {} has no launch intent", run.id))?;
        Ok(Self {
            run,
            launch,
            spec_digest: state.spec_digest.clone().unwrap_or_default(),
            #[cfg(test)]
            attempt: state.submission_attempt,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WatchedRun {
    pub id: String,
    pub handle: Handle,
}

/// The outstanding submitter of one attempt, recorded before its side effect.
pub struct Fence<'a> {
    store: &'a Mutex<Store>,
    run_id: &'a str,
    attempt: u32,
    taken_at: i64,
}

impl Fence<'_> {
    /// Whether a cancel was accepted for the run; a launch checks before each step it takes.
    pub fn cancelled(&self) -> bool {
        lock(self.store)
            .engine_run_state(self.run_id)
            .ok()
            .flatten()
            .is_some_and(|state| state.cancel_accepted_at.is_some())
    }

    /// For a launch done by a child process. Record it before the child can act (e.g. have it
    /// wait on stdin), so recovery after a crash waits for it instead of racing it.
    pub fn record_submitter(&self, pid: u32) -> Result<()> {
        let submitter = process::identity(pid)
            .ok_or_else(|| anyhow!("Could not read submitter {pid}'s identity"))?;
        let fence = SubmissionFence {
            taken_at: self.taken_at,
            submitter: Some(submitter),
        };
        if lock(self.store).record_submitter(self.run_id, self.attempt, &fence)? {
            Ok(())
        } else {
            Err(anyhow!(
                "Run {} no longer awaits attempt {}",
                self.run_id,
                self.attempt
            ))
        }
    }
}

#[async_trait]
pub trait Backend: Send + Sync {
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError>;
    async fn recover(&self, intent: &RunIntent) -> Recovery;
    async fn recover_with_cancel(
        &self,
        intent: &RunIntent,
        _cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Recovery {
        self.recover(intent).await
    }

    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)>;
    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome;
    async fn cleanup(&self, run: &WatchedRun) -> CancelOutcome {
        self.cancel(run).await
    }
    /// Releases provider resources after output is kept; failures retry with collection.
    async fn finish(&self, _run: &WatchedRun) -> Result<()> {
        Ok(())
    }
    async fn unwatch(&self, _run: &WatchedRun) {}
    /// Releases what is held for reading the logs of runs `forgotten` names, such as deleted
    /// ones.
    fn forget_logs(&self, _forgotten: &dyn Fn(&str) -> bool) {}
    /// Before a finished run's log is kept: fetch it through its end where reads are cached.
    async fn drain_log(&self, _run: &WatchedRun) -> Result<()> {
        Ok(())
    }
    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage>;
}

pub(crate) const ORPHANED_RUN_SETTLE_MS: i64 = 5 * 60 * 1000;

/// Process-local facts; everything durable lives in the store.
#[derive(Default)]
struct Memory {
    locks: RunLocks,
    /// Serializes submission, recovery and notify delivery per run; dropping a future releases it.
    gates: HashMap<String, Arc<tokio::sync::Mutex<()>>>,
    /// Monotonic start of each Unreachable outage seen by this process.
    outages: HashMap<String, Duration>,
    /// Failed recoveries and when the next one is due.
    retries: HashMap<String, (u32, Duration)>,
    /// Cancels delivered on the current connection; cleared when contact is lost.
    cancels_sent: HashSet<String>,
    /// Cancels awaiting the backend's verdict, alive while their delivery is; the verdict, not
    /// a death seen meanwhile, ends the run.
    verifying: HashMap<String, Weak<()>>,
    #[cfg(test)]
    after_legacy_snapshot: Option<Box<dyn FnOnce() + Send>>,
    #[cfg(test)]
    before_observed_commit: Option<Box<dyn FnOnce() + Send>>,
    /// Notify-file lengths reported but not yet turned into a notice.
    notify: HashMap<String, u64>,
    /// Last published log length per run.
    logs: HashMap<String, u64>,
}

pub struct Engine {
    store: Mutex<Store>,
    clock: Arc<dyn Clock>,
    pub bus: Bus,
    memory: Mutex<Memory>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Engine {
    pub fn new(store: Store, clock: Arc<dyn Clock>) -> Self {
        Self {
            store: Mutex::new(store),
            clock,
            bus: Bus::new(),
            memory: Mutex::default(),
        }
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        lock(&self.store)
    }

    fn memory(&self) -> MutexGuard<'_, Memory> {
        lock(&self.memory)
    }

    fn gate(&self, run_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.memory()
            .gates
            .entry(run_id.into())
            .or_default()
            .clone()
    }

    /// None while another task is submitting, recovering or notifying for this run.
    fn try_gate(&self, run_id: &str) -> Option<OwnedMutexGuard<()>> {
        self.gate(run_id).try_lock_owned().ok()
    }

    /// Takes both per-run locks unless already held. Every action on a run goes through here,
    /// so a run an old supervisor holds is never written.
    fn own(&self, run_id: &str) -> Result<bool> {
        let paths = self.store().supervisor_lock_paths(run_id)?;
        self.memory().locks.acquire(run_id, paths)
    }

    /// True while this engine holds the run's supervisor locks.
    pub fn holds(&self, run_id: &str) -> bool {
        self.memory().locks.holds(run_id)
    }

    #[cfg(test)]
    pub async fn observe_runs(
        &self,
        backend: &dyn Backend,
        runs: &[WatchedRun],
        budget: Duration,
    ) -> Result<()> {
        let observations = backend.observe(runs, budget).await;
        self.apply_observations(backend, runs, observations).await
    }

    /// Applies what `Backend::observe` reported and delivers pending cancels.
    pub async fn apply_observations(
        &self,
        backend: &dyn Backend,
        runs: &[WatchedRun],
        observations: Vec<(String, Observation)>,
    ) -> Result<()> {
        for (id, observation) in observations {
            self.observe(&id, observation)?;
        }
        for watched in runs {
            let (run, state) = self.load(&watched.id)?;
            let phase = phase_of(&run)?;
            let tombstone = reconcile::is_tombstone(phase, &state.contact);
            if phase.is_terminal() && !tombstone {
                self.memory().logs.remove(&watched.id);
            } else if run.cancel_requested || state.cancel_accepted_at.is_some() || tombstone {
                self.deliver_cancel(backend, &watched.id).await?;
            }
        }
        Ok(())
    }

    /// Whether a stop is on its way to the run's backend.
    pub fn delivering_cancel(&self, run_id: &str) -> bool {
        self.memory()
            .verifying
            .get(run_id)
            .is_some_and(|attempt| attempt.strong_count() > 0)
    }

    /// Delivers a pending stop once per connection; rented boxes can be released without SSH.
    pub async fn deliver_cancel(&self, backend: &dyn Backend, run_id: &str) -> Result<()> {
        if !self.own(run_id)? {
            return Ok(());
        }
        let (run, state) = self.load(run_id)?;
        let phase = phase_of(&run)?;
        // Intent is read under `cancel.lock`, accepting a legacy `cancel_requested` for good.
        let wanted = reconcile::is_tombstone(phase, &state.contact)
            || (matches!(phase, Phase::Starting | Phase::Running)
                && self.store().accept_run_cancel(run_id, None)?.is_some());
        let Some(handle) = handle_of(&run) else {
            return Ok(());
        };
        let deliverable = matches!(
            state.contact,
            Contact::Live | Contact::Unreconciled { .. } | Contact::Stopping { .. }
        ) || matches!(&handle, Handle::OpenResearch { .. })
            || matches!(&handle, Handle::Legacy { descriptor } if descriptor.kind == "openresearch_job");
        if !wanted || !deliverable || self.memory().cancels_sent.contains(run_id) {
            return Ok(());
        }
        let watched = WatchedRun {
            id: run.id.clone(),
            handle,
        };
        let verifying = Arc::new(());
        {
            // Serialized with terminal decisions, which commit under the store lock.
            let _decisions = self.store();
            let mut memory = self.memory();
            memory.cancels_sent.insert(run_id.to_string());
            memory
                .verifying
                .insert(run_id.to_string(), Arc::downgrade(&verifying));
        }
        let outcome = if reconcile::is_tombstone(phase, &state.contact) {
            backend.cleanup(&watched).await
        } else {
            backend.cancel(&watched).await
        };
        if matches!(outcome, CancelOutcome::Undelivered { .. }) {
            self.memory().cancels_sent.remove(run_id);
        }
        self.apply_cancel(&run, outcome)
    }

    fn load(&self, run_id: &str) -> Result<(StoredRun, EngineRunState)> {
        load(&self.store(), run_id)
    }
}

fn load(store: &Store, run_id: &str) -> Result<(StoredRun, EngineRunState)> {
    let missing = || anyhow!("Run {run_id} not found");
    let run = store.get_run(run_id)?.ok_or_else(missing)?;
    let state = store.engine_run_state(run_id)?.ok_or_else(missing)?;
    Ok((run, state))
}

fn phase_of(run: &StoredRun) -> Result<Phase> {
    Phase::parse(&run.status).ok_or_else(|| anyhow!("Unknown run status: {}", run.status))
}

/// Rows the engine created, as opposed to rows orx 0.2.x left behind.
pub fn is_engine_row(run: &StoredRun) -> bool {
    serde_json::from_str::<serde_json::Value>(&run.backend_json)
        .is_ok_and(|value| value["kind"] == "engine")
}

/// Runner-2 rows carry `handle`; an adopted runner-1 row is its whole old descriptor.
pub fn handle_of(run: &StoredRun) -> Option<Handle> {
    let value: serde_json::Value = serde_json::from_str(&run.backend_json).ok()?;
    if value["kind"] == "engine" {
        return serde_json::from_value(value.get("handle")?.clone()).ok();
    }
    let descriptor = BackendDescriptor::parse(&run.backend_json).ok()?;
    descriptor.job_id.is_some().then(|| Handle::Legacy {
        descriptor: Box::new(descriptor),
    })
}

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
