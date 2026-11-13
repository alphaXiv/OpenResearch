use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use orx_host_proto::{self as proto, *};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use super::super::transport::{HostConn, Status};
use super::super::{
    Backend, CancelOutcome, Fence, Handle, LaunchError, Observation, Recovery, RunIntent,
    WatchedRun,
};
use super::api::interrupted;
use crate::error::{anyhow, bail, Result};
use crate::jobs::ssh::sh_quote;
use crate::jobs::BackendDescriptor;

#[derive(Default, Serialize, Deserialize)]
pub struct LaunchOptions {
    pub run_sh: Option<String>,
    /// Secrets are added at launch and never stored with the intent.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cancel_sh: Option<String>,
    /// Runs inside this container, so run.sh and cancel.sh are built at launch.
    pub container: Option<crate::jobs::ssh::ContainerRun>,
    /// A saved container the runtime resolves into `container` before the intent is stored.
    pub container_reference: Option<String>,
    /// The engine root on the host, which selects the host connection.
    pub root: Option<String>,
}

#[derive(Default)]
struct Feed {
    summaries: HashMap<String, Summary>,
}

pub struct Ssh {
    pub(crate) host: String,
    pub(super) root: String,
    pub(crate) conn: HostConn,
    feed: tokio::sync::Mutex<Feed>,
    followers: Mutex<HashMap<String, usize>>,
}

/// Dropping the last subscription stops log-length checks in the host feed.
pub struct LogSubscription {
    adapter: std::sync::Arc<Ssh>,
    run: String,
}

impl Drop for LogSubscription {
    fn drop(&mut self) {
        let mut followers = self.adapter.followers.lock().unwrap();
        if let Some(count) = followers.get_mut(&self.run) {
            *count -= 1;
            if *count == 0 {
                followers.remove(&self.run);
            }
        }
    }
}

impl Ssh {
    pub fn new(host: String, root: String, conn: HostConn) -> Self {
        Self {
            host,
            root,
            conn,
            feed: tokio::sync::Mutex::new(Feed::default()),
            followers: Mutex::default(),
        }
    }

    pub fn follow_logs(self: &std::sync::Arc<Self>, run: &str) -> LogSubscription {
        *self
            .followers
            .lock()
            .unwrap()
            .entry(run.into())
            .or_default() += 1;
        LogSubscription {
            adapter: self.clone(),
            run: run.into(),
        }
    }

    pub(super) fn handle(&self) -> Handle {
        Handle::Ssh {
            host: self.host.clone(),
            root: self.root.clone(),
        }
    }

    pub(super) fn validate(&self, run: &WatchedRun) -> Result<()> {
        match &run.handle {
            Handle::Ssh { host, root } | Handle::Slurm { host, root, .. }
                if host == &self.host && root == &self.root =>
            {
                Ok(())
            }
            Handle::Legacy { descriptor }
                if matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job")
                    && descriptor.namespace.as_deref() == Some(&self.host) =>
            {
                Ok(())
            }
            _ => bail!("Run {} does not belong to host {}", run.id, self.host),
        }
    }

    pub(crate) async fn request(&self, request: Request) -> Result<Response> {
        match self.conn.request(request).await? {
            Response::Error { code, message } => bail!("orx-host {code:?}: {message}"),
            response => Ok(response),
        }
    }

    pub(super) fn disconnected(&self, cause: String) -> Observation {
        let state = self.conn.state();
        let state = state.borrow();
        let cause = state.cause.clone().unwrap_or(cause);
        if state.status == Status::NeedsAuth {
            Observation::NeedsAuth { cause }
        } else {
            Observation::Unreachable { cause }
        }
    }

    pub(super) async fn launch_request(&self, request: Request) -> Result<Response, LaunchError> {
        let frame_len = serde_json::to_vec(&Message {
            id: u64::MAX,
            body: &request,
        })
        .map_err(rejected)?
        .len();
        if frame_len > MAX_FRAME_LEN {
            return Err(rejected(
                "Launch request exceeds the 8 MiB frame limit. Shorten the script or environment.",
            ));
        }
        let submitting = matches!(&request, Request::Launch(_) | Request::SlurmSubmit { .. });
        let pinning = matches!(&request, Request::PinSource { .. });
        match self.conn.request(request).await {
            // Staging sends no launch: a busy lock or a blob swept before its pin retries next
            // tick, and any other refusal fails the run with the host's cause.
            Ok(Response::Error {
                code: ErrorCode::Busy,
                ..
            }) if !submitting => Err(LaunchError::Blocked {
                contact: crate::store::Contact::Live,
            }),
            Ok(Response::Error {
                code: ErrorCode::NotFound,
                ..
            }) if pinning => Err(LaunchError::Blocked {
                contact: crate::store::Contact::Live,
            }),
            Ok(Response::Error { message, .. }) if !submitting => Err(rejected(format!(
                "Couldn't stage the source on {}: {message}",
                self.host
            ))),
            Ok(Response::Error {
                code: ErrorCode::InvalidRequest | ErrorCode::Conflict,
                message,
            }) => Err(rejected(message)),
            Err(error)
                if !submitting
                    || error
                        .downcast_ref::<super::super::transport::Unavailable>()
                        .is_some_and(|error| !error.sent) =>
            {
                let state = error
                    .downcast_ref::<super::super::transport::Unavailable>()
                    .map(|error| error.state.clone())
                    .unwrap_or_else(|| self.conn.state().borrow().clone());
                let since = state.since_ms;
                let cause = state.cause.unwrap_or_else(|| error.to_string());
                let contact = if state.status == Status::NeedsAuth {
                    crate::store::Contact::NeedsAuth { since, cause }
                } else {
                    crate::store::Contact::Unreachable { since, cause }
                };
                Err(LaunchError::Blocked { contact })
            }
            Ok(Response::Error { message, .. }) => Err(interrupted(message)),
            Err(error) => Err(interrupted(error)),
            Ok(response) => Ok(response),
        }
    }

    pub async fn stage_blob(&self, sha: &str, path: &Path, len: u64) -> Result<(), LaunchError> {
        if let Response::HasBlob { present: true } = self
            .launch_request(Request::HasBlob {
                sha: sha.into(),
                len,
            })
            .await?
        {
            return Ok(());
        }
        let unreadable = |error| {
            rejected(format!(
                "Couldn't read the source snapshot {}: {error}",
                path.display()
            ))
        };
        let mut file = tokio::fs::File::open(path).await.map_err(unreadable)?;
        if file.metadata().await.map_err(unreadable)?.len() != len {
            return Err(rejected("Source snapshot length changed"));
        }
        let mut offset = 0;
        let mut bytes = vec![0; 256 * 1024];
        loop {
            let read = file.read(&mut bytes).await.map_err(unreadable)?;
            if read == 0 && len != 0 {
                break;
            }
            match self
                .launch_request(Request::PutBlob {
                    sha: sha.into(),
                    offset,
                    bytes: bytes[..read].to_vec(),
                })
                .await?
            {
                Response::PutBlob { len: remote }
                    if remote >= offset + read as u64 && remote <= len =>
                {
                    offset = remote;
                    file.seek(std::io::SeekFrom::Start(offset))
                        .await
                        .map_err(unreadable)?;
                }
                _ => return Err(rejected(UNEXPECTED)),
            }
            if offset == len {
                break;
            }
        }
        if offset != len
            || !matches!(
                self.launch_request(Request::HasBlob {
                    sha: sha.into(),
                    len
                })
                .await?,
                Response::HasBlob { present: true }
            )
        {
            return Err(rejected("Source snapshot digest or length differs"));
        }
        Ok(())
    }

    /// Holds the staged blob in the run dir against source sweeps, one bounded slice per
    /// request when it must be copied. A cancel accepted meanwhile fails as Interrupted, so
    /// recovery decides what comes next; a blob swept since staging is staged again next tick.
    pub(super) async fn pin_source(
        &self,
        run: &str,
        sha: &str,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<(), LaunchError> {
        loop {
            if cancelled() {
                return Err(interrupted(CANCELLED));
            }
            match self
                .launch_request(Request::PinSource {
                    run: run.into(),
                    sha: sha.into(),
                })
                .await?
            {
                Response::PinSource { pinned: true } => return Ok(()),
                Response::PinSource { pinned: false } => {}
                _ => return Err(rejected(UNEXPECTED)),
            }
        }
    }

    pub(super) async fn payload(
        &self,
        intent: &RunIntent,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<LaunchIntent, LaunchError> {
        if intent.launch.target.as_deref() != Some(&self.host) {
            return Err(rejected(format!(
                "Launch target does not match host {}",
                self.host
            )));
        }
        let options: LaunchOptions =
            serde_json::from_value(intent.launch.options.clone()).map_err(rejected)?;
        let metadata = BackendDescriptor::parse(&intent.run.backend_json).map_err(rejected)?;
        if let Some(sha) = &metadata.source_digest {
            let path = metadata
                .source_path
                .as_deref()
                .ok_or_else(|| rejected("Source path missing"))?;
            let len = metadata
                .source_size
                .ok_or_else(|| rejected("Source length missing"))?;
            self.stage_blob(sha, Path::new(path), len).await?;
            self.pin_source(&intent.run.id, sha, cancelled).await?;
        }
        let mut env = crate::config::launch_env(&intent.run.id);
        env.extend(options.env);
        let env: BTreeMap<_, _> = crate::jobs::default_python_env(&env).into_iter().collect();
        let run_sh = options.run_sh.unwrap_or_else(|| intent.run.command.clone());
        let (run_sh, env, cancel_sh) = match options.container {
            Some(container) => {
                let exports = env
                    .iter()
                    .map(|(key, value)| format!("export {key}={}", sh_quote(value)))
                    .collect::<Vec<_>>()
                    .join("\n");
                // orx-host links the staged archive into the run dir as source.tar.
                let source = metadata.source_digest.is_some().then_some("source.tar");
                (
                    container.engine_run_sh(&exports, &run_sh, source),
                    BTreeMap::new(),
                    Some(container.engine_cancel_sh()),
                )
            }
            None => (run_sh, env, options.cancel_sh),
        };
        Ok(LaunchIntent {
            run: intent.run.id.clone(),
            digest: intent.spec_digest.clone(),
            run_sh,
            env,
            source: metadata.source_digest,
            cancel_sh,
        })
    }

    pub async fn cleanup(&self, run: &WatchedRun) -> Result<CleanupAck> {
        self.validate(run)?;
        match self
            .request(Request::Cleanup {
                run: run.id.clone(),
            })
            .await?
        {
            Response::Cleanup { outcome } => Ok(outcome),
            response => bail!("Unexpected cleanup reply: {response:?}"),
        }
    }

    #[cfg(test)]
    pub async fn summary(&self, run: &str) -> Option<Summary> {
        self.feed.lock().await.summaries.get(run).cloned()
    }

    pub(super) async fn changes(
        &self,
        runs: &[String],
        budget: Duration,
    ) -> Result<Vec<(String, Summary)>> {
        let mut feed = self.feed.lock().await;
        let watched: HashSet<_> = runs.iter().collect();
        let watch = runs
            .iter()
            .map(|run| {
                (
                    run.clone(),
                    feed.summaries.get(run).cloned().unwrap_or_else(missing),
                )
            })
            .collect();
        let follow_logs = self
            .followers
            .lock()
            .unwrap()
            .keys()
            .filter(|run| watched.contains(run))
            .cloned()
            .collect();
        let wait_ms = budget.min(Duration::from_secs(25)).as_millis() as u32;
        let response = self
            .request(Request::Changes {
                watch,
                follow_logs,
                wait_ms,
            })
            .await?;
        let Response::Changes { runs: changed } = response else {
            bail!("Unexpected feed reply: {response:?}");
        };
        for (run, summary) in changed {
            if watched.contains(&run) {
                feed.summaries.insert(run, summary);
            }
        }
        // Replay current facts after reconnect, including when the remote summary did not change.
        Ok(runs
            .iter()
            .map(|run| {
                (
                    run.clone(),
                    feed.summaries.get(run).cloned().unwrap_or_else(missing),
                )
            })
            .collect())
    }
}

/// A 0.2.x run dir as `orx-host` addresses it; 0.2.x stored HOME-relative dirs without `~/`.
pub(super) fn legacy_dir(dir: &str) -> String {
    if dir.starts_with('/') || dir.starts_with("~/") {
        dir.into()
    } else {
        format!("~/{dir}")
    }
}

pub(super) const CANCELLED: &str = "A cancel was accepted during the launch.";
pub(super) const UNEXPECTED: &str = "The host sent an unexpected response.";

fn rejected(error: impl std::fmt::Display) -> LaunchError {
    LaunchError::Rejected {
        reason: error.to_string(),
    }
}

pub(super) fn missing() -> Summary {
    Summary {
        started_at: None,
        state: RunState::Missing,
        log_len: None,
        notify_len: 0,
        cleanup: None,
    }
}

pub(super) fn observations(run: String, summary: &Summary) -> Vec<(String, Observation)> {
    let observation = match &summary.state {
        RunState::Missing => Observation::NotFound,
        RunState::Staging => Observation::Queued { detail: None },
        RunState::Starting { reason } => Observation::Queued {
            detail: reason.clone(),
        },
        RunState::Running => Observation::Running {
            started_at: summary.started_at,
        },
        RunState::Exited { receipt } if receipt.reason == ExitReason::Cancelled => {
            let proof = summary.cleanup.as_ref();
            let processes = proof.map_or(&receipt.processes, |proof| &proof.processes);
            let outcome = proof.map_or(&receipt.outcome, |proof| &proof.outcome);
            let note = match outcome {
                proto::CancelOutcome::Verified | proto::CancelOutcome::Survivors => None,
                proto::CancelOutcome::Unverifiable { unreadable } => Some(unreadable.clone()),
            };
            Observation::CancelReceipt {
                finished_at: receipt.finished_at,
                code: receipt.code,
                remaining: processes.len(),
                note,
            }
        }
        RunState::Exited { receipt } => Observation::Exited {
            finished_at: receipt.finished_at,
            code: receipt.code,
            signal: receipt.signal,
        },
        RunState::Failed { reason } => Observation::Failed {
            finished_at: None,
            reason: reason.clone(),
        },
        RunState::Lost { reason } => Observation::Lost {
            reason: reason.clone(),
        },
        RunState::Cancelled { .. } => Observation::Cancelled {
            finished_at: None,
            code: None,
            signal: None,
        },
        RunState::Unreachable { cause } => Observation::Unreachable {
            cause: cause.clone(),
        },
        RunState::InvalidData { detail } | RunState::Unreconciled { detail } => {
            Observation::Unreconciled {
                detail: detail.clone(),
            }
        }
    };
    let mut result = Vec::new();
    let processes = summary
        .cleanup
        .as_ref()
        .map(|cleanup| &cleanup.processes)
        .or(match &summary.state {
            RunState::Exited { receipt } => Some(&receipt.processes),
            _ => None,
        });
    let settled = processes.map_or_else(
        || {
            matches!(
                &summary.state,
                RunState::Lost { .. } | RunState::Cancelled { .. }
            )
        },
        |processes| processes.is_empty(),
    );
    if let (Some(processes), RunState::Exited { receipt }) = (processes, &summary.state) {
        if receipt.reason == ExitReason::Exited && !processes.is_empty() {
            result.push((
                run.clone(),
                Observation::Lingering {
                    pids: processes.iter().map(|process| process.pid).collect(),
                },
            ));
        }
    }
    if let Some(len) = summary.log_len {
        result.push((run.clone(), Observation::Log { len }));
    }
    if let Some(started_at) = summary
        .started_at
        .filter(|_| matches!(&summary.state, RunState::Exited { .. }))
    {
        result.push((
            run.clone(),
            Observation::Running {
                started_at: Some(started_at),
            },
        ));
    }
    // Persist attention before terminal runs leave the watch set.
    result.push((run.clone(), observation));
    if settled {
        result.push((run.clone(), Observation::Lingering { pids: Vec::new() }));
    }
    if summary.notify_len > 0 {
        result.push((
            run,
            Observation::Notify {
                len: summary.notify_len,
            },
        ));
    }
    result
}

#[async_trait]
impl Backend for Ssh {
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let payload = self.payload(intent, &|| fence.cancelled()).await?;
        if fence.cancelled() {
            return Err(interrupted(CANCELLED));
        }
        match self.launch_request(Request::Launch(payload)).await? {
            Response::Launch { .. } => Ok(self.handle()),
            _ => Err(interrupted(UNEXPECTED)),
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
        match self
            .request(Request::Inspect {
                run: intent.run.id.clone(),
            })
            .await
        {
            Ok(Response::Inspect { summary }) => match summary.state {
                RunState::Missing => Recovery::Absent,
                RunState::Staging if !cancelled() => {
                    let payload = match self.payload(intent, cancelled).await {
                        Ok(payload) => payload,
                        Err(_) if cancelled() => return Recovery::Found(self.handle()),
                        Err(error) => {
                            return Recovery::Unknown {
                                detail: error.to_string(),
                            }
                        }
                    };
                    if cancelled() {
                        return Recovery::Found(self.handle());
                    }
                    match self.request(Request::Launch(payload)).await {
                        Ok(Response::Launch { .. }) => Recovery::Found(self.handle()),
                        Ok(_) => Recovery::Unknown {
                            detail: UNEXPECTED.into(),
                        },
                        Err(error) => Recovery::Unknown {
                            detail: error.to_string(),
                        },
                    }
                }
                _ => Recovery::Found(self.handle()),
            },
            Ok(response) => Recovery::Unknown {
                detail: format!("Unexpected inspection reply: {response:?}"),
            },
            Err(error) => Recovery::Unknown {
                detail: error.to_string(),
            },
        }
    }

    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)> {
        if !runs.is_empty()
            && runs
                .iter()
                .all(|run| matches!(run.handle, Handle::Legacy { .. }))
        {
            tokio::time::sleep(budget).await;
        }
        let mut result = Vec::new();
        let mut native = Vec::new();
        for run in runs {
            if let Err(error) = self.validate(run) {
                result.push((
                    run.id.clone(),
                    Observation::Unreconciled {
                        detail: error.to_string(),
                    },
                ));
                continue;
            }
            if let Handle::Legacy { descriptor } = &run.handle {
                let request = Request::InspectLegacy {
                    dir: legacy_dir(descriptor.job_id.as_deref().unwrap_or_default()),
                };
                let observation = match self.request(request).await {
                    Ok(Response::InspectLegacy { inspection }) => {
                        if inspection.pid.is_some() && !inspection.alive {
                            result.push((run.id.clone(), Observation::Lingering { pids: vec![] }));
                        }
                        if let Some(code) = inspection.exit_code {
                            Observation::Exited {
                                finished_at: None,
                                code: Some(code),
                                signal: None,
                            }
                        } else if inspection.alive {
                            Observation::Running { started_at: None }
                        } else {
                            Observation::Unreconciled {
                                detail: if inspection.receipt_unreadable {
                                    "Legacy receipt unreadable"
                                } else {
                                    "Legacy process gone without a receipt"
                                }
                                .into(),
                            }
                        }
                    }
                    Err(error) => self.disconnected(error.to_string()),
                    response => Observation::Unreconciled {
                        detail: format!("Unexpected legacy reply: {response:?}"),
                    },
                };
                result.push((run.id.clone(), observation));
            } else {
                native.push(run.id.clone());
            }
        }
        if !native.is_empty() {
            match self.changes(&native, budget).await {
                Ok(changed) => result.extend(
                    changed
                        .into_iter()
                        .flat_map(|(run, summary)| observations(run, &summary)),
                ),
                Err(error) => result.extend(
                    native
                        .into_iter()
                        .map(|run| (run, self.disconnected(error.to_string()))),
                ),
            }
        }
        result
    }

    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        if let Err(error) = self.validate(run) {
            return CancelOutcome::Undelivered {
                detail: error.to_string(),
            };
        }
        if let Handle::Legacy { descriptor } = &run.handle {
            let container =
                descriptor
                    .ssh_container
                    .as_ref()
                    .map(|container| proto::ContainerRun {
                        reference: container.reference.clone(),
                        id: container.id.clone(),
                        started_at: container.started_at.clone(),
                        run_dir: container.run_dir.clone(),
                    });
            return match self
                .request(Request::CancelLegacy {
                    dir: legacy_dir(descriptor.job_id.as_deref().unwrap_or_default()),
                    container,
                })
                .await
            {
                Ok(Response::CancelLegacy {
                    outcome: LegacyCancelOutcome::GroupGone,
                }) => CancelOutcome::Verified,
                Ok(Response::CancelLegacy {
                    outcome: LegacyCancelOutcome::Survivors { pids },
                }) => CancelOutcome::Survivors { count: pids.len() },
                result => CancelOutcome::Undelivered {
                    detail: format!("Legacy cancel was not acknowledged: {result:?}"),
                },
            };
        }
        match self
            .request(Request::Cancel {
                run: run.id.clone(),
            })
            .await
        {
            Ok(Response::Cancel { .. }) => CancelOutcome::Pending,
            result => CancelOutcome::Undelivered {
                detail: format!("Cancel was not acknowledged: {result:?}"),
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
        match Ssh::cleanup(self, run).await {
            Ok(_) => outcome,
            Err(error) if error.to_string() == "orx-host InvalidRequest: run has not finished" => {
                CancelOutcome::Pending
            }
            Err(error) => CancelOutcome::Undelivered {
                detail: error.to_string(),
            },
        }
    }

    async fn unwatch(&self, run: &WatchedRun) {
        self.feed.lock().await.summaries.remove(&run.id);
    }

    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage> {
        if range.end < range.start || range.end - range.start > MAX_PAGE_LEN {
            bail!("Invalid log range");
        }
        self.validate(run)?;
        let target = match &run.handle {
            Handle::Legacy { descriptor } if descriptor.kind == "slurm_job" => {
                LogTarget::Dir(legacy_dir(&crate::jobs::slurm::run_dir(
                    descriptor.remote_root.as_deref(),
                    &run.id,
                )))
            }
            Handle::Legacy { descriptor } => LogTarget::Dir(legacy_dir(
                descriptor
                    .job_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("Legacy run directory missing"))?,
            )),
            _ => LogTarget::Run(run.id.clone()),
        };
        // Sub-ranges small enough for one frame, all of the same log file.
        let mut bytes = Vec::new();
        let mut identity = ident.cloned();
        let mut start = range.start;
        loop {
            let end = range.end.min(start.saturating_add(256 * 1024));
            let response = self
                .request(Request::ReadLog {
                    target: target.clone(),
                    file,
                    ident: identity.clone(),
                    range: ByteRange { start, end },
                    anchor: None,
                })
                .await?;
            match response {
                Response::ReadLog {
                    page:
                        LogPage::Data {
                            len,
                            ident,
                            start: actual,
                            bytes: page,
                            eof,
                        },
                } if actual == start && page.len() as u64 <= end - start => {
                    identity = Some(ident.clone());
                    start += page.len() as u64;
                    let short = page.is_empty();
                    bytes.extend(page);
                    if eof || start == range.end || short {
                        return Ok(LogPage::Data {
                            len,
                            ident,
                            start: range.start,
                            bytes,
                            eof,
                        });
                    }
                }
                Response::ReadLog {
                    page: reset @ LogPage::Reset { .. },
                } => return Ok(reset),
                response => bail!("Unexpected log reply: {response:?}"),
            }
        }
    }
}
