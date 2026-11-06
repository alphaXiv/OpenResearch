use crate::engine::clock::Clock;
use crate::engine::handle::ApiAccount;
use crate::engine::*;
use crate::error::{anyhow, Result};
use async_trait::async_trait;
use futures::{stream, StreamExt};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

type ProviderFactory = fn(&ApiAccount, Arc<dyn Clock>, PathBuf) -> Result<Arc<dyn Backend>>;

pub struct Provider {
    pub kind: &'static str,
    pub build: ProviderFactory,
}

pub fn provider(kind: &str) -> Result<&'static Provider> {
    super::PROVIDERS
        .iter()
        .find(|provider| provider.kind == kind)
        .ok_or_else(|| anyhow!("Unknown API provider {kind}"))
}

pub fn handle(intent: &RunIntent, ids: Vec<String>) -> Handle {
    Handle::Api {
        provider: intent.launch.kind.clone(),
        account: ApiAccount {
            target: intent.launch.target.clone(),
            context: intent.launch.options["context"].as_str().map(str::to_owned),
        },
        ids,
    }
}

#[cfg(test)]
pub(crate) fn test_handle(
    kind: &str,
    target: Option<&str>,
    context: Option<&str>,
    ids: Vec<String>,
) -> Handle {
    Handle::Api {
        provider: kind.into(),
        account: ApiAccount {
            target: target.map(str::to_owned),
            context: context.map(str::to_owned),
        },
        ids,
    }
}

pub fn id<'a>(
    run: &'a WatchedRun,
    kind: &str,
    target: Option<&str>,
    context: Option<&str>,
) -> Result<&'a str> {
    let id = match &run.handle {
        Handle::Api {
            provider,
            account,
            ids,
        } if provider == kind
            && (target.is_none() || account.target.as_deref() == target)
            && (target.is_none() || account.context.as_deref() == context) =>
        {
            ids.first().map(String::as_str)
        }
        Handle::Legacy { descriptor }
            if descriptor.kind == kind
                && (target.is_none() || descriptor.namespace.as_deref() == target)
                && (target.is_none() || descriptor.context.as_deref() == context) =>
        {
            descriptor.job_id.as_deref()
        }
        _ => return Err(anyhow!("Run does not belong to this {kind} account")),
    };
    id.filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow!("API handle has no job id"))
}

#[async_trait]
pub trait Api: Send + Sync {
    type Cursor: Clone + Default + Send + 'static;
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError>;
    async fn recover(&self, intent: &RunIntent) -> Recovery;
    async fn inspect(&self, run: &WatchedRun) -> Result<Observation>;
    async fn cancel(&self, run: &WatchedRun) -> Result<()>;
    async fn finish(&self, _run: &WatchedRun) -> Result<()> {
        Ok(())
    }
    /// Hands `sink` the bytes past `cursor` as they arrive, and says whether the stream closed
    /// before `window` ran out. An error means nothing reached `sink`.
    async fn logs(
        &self,
        run: &WatchedRun,
        cursor: &mut Self::Cursor,
        window: Duration,
        sink: &mut (dyn for<'b> FnMut(&'b [u8]) + Send),
    ) -> Result<bool>;
}

const PAGE_STREAM_WAIT: Duration = Duration::from_millis(20);
const DRAIN_WINDOW: Duration = Duration::from_secs(60);
/// How long one draining stream may stay open across callers' windows.
const DRAIN_STREAM_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);

/// Where a provider run's log is spooled as it arrives; pages and final collection read it.
pub fn spool_path(directory: &Path, run: &str) -> Result<PathBuf> {
    if run.is_empty()
        || !run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(anyhow!("Invalid run id {run}"));
    }
    Ok(directory.join(format!("{run}.spool.log")))
}

/// A provider log on disk, from the stream position `cursor` reached. It lasts one engine:
/// a new one starts empty and reads the provider from the start.
struct Spool<C> {
    path: PathBuf,
    /// `None` once the run is forgotten, so an aborted stream writes nothing more.
    file: Option<File>,
    len: u64,
    cursor: C,
    error: Option<String>,
    complete: bool,
    /// Final collection needs a pass started after the terminal observation.
    generation: u64,
    drain: Option<tokio::task::AbortHandle>,
    #[cfg(test)]
    fail_after: Option<usize>,
}

impl<C> Spool<C> {
    /// A failed append leaves none of its bytes behind.
    fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        let Some(file) = &mut self.file else {
            return Ok(());
        };
        file.seek(SeekFrom::Start(self.len))?;
        #[cfg(test)]
        if let Some(limit) = self.fail_after.take() {
            file.write_all(&bytes[..limit])?;
            file.set_len(self.len)?;
            return Err(std::io::Error::other("injected write failure"));
        }
        if let Err(error) = file.write_all(bytes) {
            file.set_len(self.len)?;
            return Err(error);
        }
        self.len += bytes.len() as u64;
        Ok(())
    }

    /// Drops what was appended past `len`.
    fn truncate(&mut self, len: u64) -> std::io::Result<()> {
        if let Some(file) = &self.file {
            file.set_len(len)?;
        }
        self.len = len;
        Ok(())
    }

    /// Stops the drain; it writes nothing more, as it may be mid-write when aborted.
    fn forget(&mut self) {
        self.file = None;
        if let Some(drain) = self.drain.take() {
            drain.abort();
        }
    }
}

type Shared<C> = Arc<std::sync::Mutex<Spool<C>>>;
type Drain = tokio::task::JoinHandle<Result<bool>>;

/// One run's spool, and the stream draining it through the end. The stream outlives a
/// caller's window, since reopening would replay the history and meet the same pauses again.
struct Log<C> {
    spool: Shared<C>,
    drain: tokio::sync::Mutex<Option<Drain>>,
}

impl<C> Drop for Log<C> {
    fn drop(&mut self) {
        lock(&self.spool).forget();
    }
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct Adapter<A: Api> {
    account: Arc<A>,
    clock: Arc<dyn Clock>,
    spools: PathBuf,
    next_observe: tokio::sync::Mutex<Duration>,
    logs: std::sync::Mutex<HashMap<String, Arc<Log<A::Cursor>>>>,
    drain_window: Duration,
}

impl<A: Api + 'static> Adapter<A> {
    pub fn new(account: A, clock: Arc<dyn Clock>, spools: PathBuf) -> Self {
        Self {
            account: Arc::new(account),
            clock,
            spools,
            next_observe: tokio::sync::Mutex::new(Duration::ZERO),
            logs: std::sync::Mutex::default(),
            drain_window: DRAIN_WINDOW,
        }
    }

    fn log(&self, run: &WatchedRun) -> Result<Arc<Log<A::Cursor>>> {
        let mut logs = lock(&self.logs);
        if let Some(log) = logs.get(&run.id) {
            return Ok(log.clone());
        }
        let path = spool_path(&self.spools, &run.id)?;
        std::fs::create_dir_all(&self.spools)?;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        // A fresh inode per engine, so pages kept from an earlier spool never match this one.
        match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        let file = options.open(&path)?;
        let log = Arc::new(Log {
            spool: Arc::new(std::sync::Mutex::new(Spool {
                path,
                len: 0,
                file: Some(file),
                cursor: A::Cursor::default(),
                error: None,
                complete: false,
                generation: 0,
                drain: None,
                #[cfg(test)]
                fail_after: None,
            })),
            drain: tokio::sync::Mutex::new(None),
        });
        logs.insert(run.id.clone(), log.clone());
        Ok(log)
    }
}

#[async_trait]
impl<A: Api + 'static> Backend for Adapter<A> {
    async fn launch(&self, intent: &RunIntent, fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        self.account.launch(intent, fence).await
    }

    async fn recover(&self, intent: &RunIntent) -> Recovery {
        self.account.recover(intent).await
    }

    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)> {
        let mut next = self.next_observe.lock().await;
        let now = self.clock.monotonic();
        if now < *next || runs.is_empty() {
            return vec![];
        }
        *next = now + Duration::from_secs(10);
        drop(next);
        let work: Vec<_> = runs
            .iter()
            .map(|run| async move {
                let result = tokio::time::timeout(budget, self.account.inspect(run)).await;
                let observation = match result {
                    Ok(Ok(observation)) => observation,
                    Ok(Err(error)) => observation_error(error),
                    Err(_) => Observation::Unreachable {
                        cause: "Provider observation timed out".into(),
                    },
                };
                let settled = observation.ends_run();
                let mut observations = vec![(run.id.clone(), observation)];
                if settled {
                    observations
                        .push((run.id.clone(), Observation::Lingering { pids: Vec::new() }));
                }
                let log = lock(&self.logs).get(&run.id).cloned();
                if let Some(log) = log {
                    {
                        let mut spool = lock(&log.spool);
                        spool.generation += 1;
                        spool.complete = false;
                    }
                    let _ = tokio::time::timeout(PAGE_STREAM_WAIT, self.drain_log(run)).await;
                    observations.push((
                        run.id.clone(),
                        Observation::Log {
                            len: lock(&log.spool).len,
                        },
                    ));
                }
                observations
            })
            .collect();
        stream::iter(work)
            .buffer_unordered(8)
            .flat_map(stream::iter)
            .collect()
            .await
    }

    async fn finish(&self, run: &WatchedRun) -> Result<()> {
        self.account.finish(run).await
    }

    /// A finished run's spool goes with it; a later read starts a new one.
    async fn unwatch(&self, run: &WatchedRun) {
        self.forget_logs(&|id| id == run.id);
        if let Ok(path) = spool_path(&self.spools, &run.id) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Their drains stop and write nothing more; spool files stay for whoever keeps or deletes
    /// them.
    fn forget_logs(&self, forgotten: &dyn Fn(&str) -> bool) {
        lock(&self.logs).retain(|id, log| {
            if !forgotten(id) {
                return true;
            }
            lock(&log.spool).forget();
            false
        });
    }

    async fn drain_log(&self, run: &WatchedRun) -> Result<()> {
        let log = self.log(run)?;
        let mut drain = log.drain.lock().await;
        loop {
            if lock(&log.spool).complete {
                return Ok(());
            }
            let task = match &mut *drain {
                Some(task) => task,
                None => {
                    let (account, run, spool) =
                        (self.account.clone(), run.clone(), log.spool.clone());
                    let (mut cursor, start, generation) = {
                        let spool = lock(&spool);
                        (spool.cursor.clone(), spool.len, spool.generation)
                    };
                    let task = tokio::spawn(async move {
                        let mut failed = None;
                        let mut sink = |chunk: &[u8]| {
                            if failed.is_none() {
                                failed = lock(&spool).append(chunk).err();
                            }
                        };
                        let closed = account
                            .logs(&run, &mut cursor, DRAIN_STREAM_LIMIT, &mut sink)
                            .await;
                        let mut spool = lock(&spool);
                        match (closed, failed) {
                            (Ok(closed), None) => {
                                spool.cursor = cursor;
                                spool.complete = closed && generation == spool.generation;
                                spool.error = None;
                                Ok(closed)
                            }
                            // The stream's position is unknown; what it appended goes.
                            (closed, failed) => {
                                spool.truncate(start)?;
                                Err(failed.map_or_else(|| closed.unwrap_err(), Into::into))
                            }
                        }
                    });
                    lock(&log.spool).drain = Some(task.abort_handle());
                    drain.insert(task)
                }
            };
            let Ok(joined) = tokio::time::timeout(self.drain_window, task).await else {
                return Err(anyhow!("The provider log is still streaming its history"));
            };
            *drain = None;
            let mut spool = lock(&log.spool);
            spool.drain = None;
            match joined {
                Ok(Ok(true)) => {}
                outcome => {
                    let error = match outcome {
                        Ok(Ok(_)) => anyhow!("The provider log stopped short of its end"),
                        Ok(Err(error)) => error,
                        Err(error) => anyhow!("Provider log drain stopped: {error}"),
                    };
                    spool.error = Some(error.to_string());
                    return Err(error);
                }
            }
        }
    }

    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        match self.account.cancel(run).await {
            Ok(()) => CancelOutcome::Pending,
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
        if file != LogFile::Log {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "Provider has no notify file",
            )
            .into());
        }
        let log = self.log(run)?;
        let deadline = tokio::time::Instant::now() + crate::config::paced(Duration::from_secs(2));
        loop {
            let _ = tokio::time::timeout(PAGE_STREAM_WAIT, self.drain_log(run)).await;
            let (len, error, path, complete) = {
                let spool = lock(&log.spool);
                (
                    spool.len,
                    spool.error.clone(),
                    spool.path.clone(),
                    spool.complete,
                )
            };
            if let Some(error) = error.filter(|_| len == 0) {
                return Err(anyhow!("{error}"));
            }
            if len > 0 || complete {
                return Ok(super::local::log::read(&path, ident, &range, None)?);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(anyhow!(
                    "Provider log stream timed out before receiving any bytes"
                ));
            }
        }
    }
}

/// Hands `sink` each line of a stream, counting them into `received`.
pub fn lines<'a>(
    sink: &'a mut (dyn for<'b> FnMut(&'b [u8]) + Send),
    received: &'a mut u64,
) -> impl FnMut(&str) + Send + 'a {
    |line| {
        sink(format!("{line}\n").as_bytes());
        *received += 1;
    }
}

/// An error means none of the stream's `received` lines arrived.
pub fn closed<T>(
    outcome: Result<Result<T>, tokio::time::error::Elapsed>,
    received: u64,
) -> Result<Option<T>> {
    match outcome {
        Ok(Ok(value)) => Ok(Some(value)),
        Ok(Err(error)) if received == 0 => Err(error),
        _ if received == 0 => Err(anyhow!(
            "Provider log stream timed out before receiving any bytes"
        )),
        _ => Ok(None),
    }
}

fn observation_error(error: crate::error::Error) -> Observation {
    let cause = error.to_string();
    let detail = cause.to_ascii_lowercase();
    let http_auth = error
        .downcast_ref::<reqwest::Error>()
        .and_then(reqwest::Error::status)
        .is_some_and(|status| matches!(status.as_u16(), 401 | 403));
    let sdk_auth = [
        "http 401:",
        "http 403:",
        "not authorized",
        "modal.exception.autherror:",
        "status.unauthenticated",
        "status.permission_denied",
        "error from server (unauthorized)",
        "error from server (forbidden)",
        "you must be logged in to the server",
    ]
    .iter()
    .any(|rejection| detail.contains(rejection));

    if http_auth || sdk_auth {
        Observation::NeedsAuth { cause }
    } else {
        Observation::Unreachable { cause }
    }
}

pub fn stage(
    stage: &str,
    message: Option<String>,
    exit_code: Option<i32>,
    started_at: Option<i64>,
    finished_at: Option<i64>,
) -> Observation {
    match stage {
        "COMPLETED" | "ERROR" | "FAILED" if exit_code.is_some() => Observation::Exited {
            finished_at,
            code: exit_code,
            signal: None,
        },
        "COMPLETED" => Observation::Exited {
            finished_at,
            code: Some(0),
            signal: None,
        },
        "ERROR" | "FAILED" => Observation::Failed {
            finished_at,
            reason: message.unwrap_or_else(|| stage.into()),
        },
        "CANCELED" | "CANCELLED" => Observation::Cancelled {
            finished_at,
            code: exit_code,
            signal: None,
        },
        "DELETED" | "GONE" => Observation::Unreconciled {
            detail: "Provider no longer knows the job".into(),
        },
        "RUNNING" => Observation::Running { started_at },
        _ => Observation::Queued {
            detail: message.unwrap_or_else(|| stage.into()),
        },
    }
}

pub fn rejected(error: impl std::fmt::Display) -> LaunchError {
    LaunchError::Rejected {
        reason: error.to_string(),
    }
}

pub fn interrupted(error: impl std::fmt::Display) -> LaunchError {
    LaunchError::Interrupted {
        detail: error.to_string(),
    }
}

pub fn unknown(error: impl std::fmt::Display) -> Recovery {
    Recovery::Unknown {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::TestClock;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[test]
    fn api_identity_rejects_foreign_accounts_and_keeps_legacy_ids() {
        let mut run = WatchedRun {
            id: "run".into(),
            handle: test_handle("hf_job", Some("lab"), None, vec!["job".into()]),
        };
        assert_eq!(id(&run, "hf_job", Some("lab"), None).unwrap(), "job");
        assert!(id(&run, "ray_job", Some("lab"), None).is_err());
        assert!(id(&run, "hf_job", Some("other"), None).is_err());
        assert!(id(&run, "hf_job", Some("lab"), Some("other")).is_err());
        for (kind, namespace, target) in [
            ("hf_job", "lab", Some("lab")),
            ("modal_job", "app", None),
            ("ray_job", "http://ray", Some("http://ray")),
            ("k8s_job", "lab", Some("lab")),
        ] {
            run.handle = Handle::Legacy {
                descriptor: Box::new(
                    crate::jobs::BackendDescriptor::parse(
                        &serde_json::json!({"kind":kind, "namespace":namespace, "jobId":"job"})
                            .to_string(),
                    )
                    .unwrap(),
                ),
            };
            assert_eq!(id(&run, kind, target, None).unwrap(), "job");
        }
        run.handle = test_handle("hf_job", Some("lab"), None, vec![]);
        assert!(id(&run, "hf_job", Some("lab"), None).is_err());
    }

    #[derive(Default)]
    struct Fake {
        text: std::sync::Mutex<Vec<u8>>,
        downloads: AtomicUsize,
        inspecting: AtomicUsize,
        peak: AtomicUsize,
        block_logs: AtomicBool,
        terminal: AtomicBool,
        log_started: tokio::sync::Notify,
        /// Draining passes pause after what they have sent until released, like a stream
        /// paused mid-history.
        pause_drains: AtomicBool,
        drain_paused: tokio::sync::Notify,
        drain_resumed: tokio::sync::Notify,
        open_streams: Arc<AtomicUsize>,
        peak_streams: AtomicUsize,
        /// History before this was rotated away: a stream from the start begins here.
        rotated: AtomicUsize,
    }

    struct Open(Arc<AtomicUsize>);
    impl Drop for Open {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait]
    impl Api for Arc<Fake> {
        type Cursor = usize;
        async fn launch(&self, _: &RunIntent, _: &Fence<'_>) -> Result<Handle, LaunchError> {
            unreachable!()
        }
        async fn recover(&self, _: &RunIntent) -> Recovery {
            unreachable!()
        }
        async fn inspect(&self, _: &WatchedRun) -> Result<Observation> {
            let active = self.inspecting.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(active, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            self.inspecting.fetch_sub(1, Ordering::SeqCst);
            Ok(if self.terminal.load(Ordering::SeqCst) {
                Observation::Exited {
                    code: Some(0),
                    signal: None,
                    finished_at: None,
                }
            } else {
                Observation::Running { started_at: None }
            })
        }
        async fn cancel(&self, _: &WatchedRun) -> Result<()> {
            Ok(())
        }
        async fn logs(
            &self,
            _: &WatchedRun,
            cursor: &mut usize,
            _: Duration,
            sink: &mut (dyn for<'b> FnMut(&'b [u8]) + Send),
        ) -> Result<bool> {
            self.downloads.fetch_add(1, Ordering::SeqCst);
            *cursor = (*cursor).max(self.rotated.load(Ordering::SeqCst));
            let open = self.open_streams.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak_streams.fetch_max(open, Ordering::SeqCst);
            let _open = Open(self.open_streams.clone());
            if self.block_logs.load(Ordering::SeqCst) {
                self.log_started.notify_one();
                self.drain_resumed.notified().await;
            }
            let mut send = |sink: &mut (dyn for<'b> FnMut(&'b [u8]) + Send)| {
                let text = self.text.lock().unwrap().clone();
                sink(&text[*cursor..]);
                *cursor = text.len();
            };
            send(sink);
            if self.pause_drains.load(Ordering::SeqCst) {
                self.drain_paused.notify_one();
                self.drain_resumed.notified().await;
                send(sink);
            }
            Ok(true)
        }
    }

    fn adapter(account: &Arc<Fake>, clock: Arc<TestClock>) -> (Adapter<Arc<Fake>>, PathBuf) {
        let spools = std::env::temp_dir().join(format!("orx-spools-{}", uuid::Uuid::new_v4()));
        (Adapter::new(account.clone(), clock, spools.clone()), spools)
    }

    fn provider_run() -> WatchedRun {
        WatchedRun {
            id: "run".into(),
            handle: crate::engine::adapters::api::test_handle(
                "ray_job",
                Some("fake"),
                None,
                vec!["provider-id".into()],
            ),
        }
    }

    const WHOLE: ByteRange = ByteRange {
        start: 0,
        end: orx_host_proto::MAX_PAGE_LEN,
    };

    async fn page(backend: &Adapter<Arc<Fake>>) -> Vec<u8> {
        let page = tokio::time::timeout(
            Duration::from_millis(500),
            backend.read_log(&provider_run(), LogFile::Log, WHOLE, None),
        )
        .await
        .expect("a page waited on the stream")
        .unwrap();
        let LogPage::Data { bytes, .. } = page else {
            panic!("no page: {page:?}")
        };
        bytes
    }

    #[tokio::test]
    async fn live_eof_refreshes_on_observation_and_keeps_its_identity() {
        let clock = Arc::new(TestClock::new());
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        let (backend, spools) = adapter(&account, clock.clone());
        let run = provider_run();
        let LogPage::Data { ident, bytes, .. } = backend
            .read_log(&run, LogFile::Log, ByteRange { start: 0, end: 6 }, None)
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(bytes, b"first\n");
        for _ in 0..50 {
            backend
                .read_log(
                    &run,
                    LogFile::Log,
                    ByteRange { start: 0, end: 6 },
                    Some(&ident),
                )
                .await
                .unwrap();
        }
        assert_eq!(account.downloads.load(Ordering::SeqCst), 1);
        backend
            .observe(std::slice::from_ref(&run), Duration::from_secs(1))
            .await;
        account.text.lock().unwrap().extend_from_slice(b"second\n");
        clock.advance(Duration::from_secs(10));
        backend
            .observe(std::slice::from_ref(&run), Duration::from_secs(1))
            .await;
        let LogPage::Data {
            ident: grown,
            bytes,
            ..
        } = backend
            .read_log(
                &run,
                LogFile::Log,
                ByteRange { start: 6, end: 13 },
                Some(&ident),
            )
            .await
            .unwrap()
        else {
            panic!("growing log reset")
        };
        assert_eq!(grown, ident);
        assert_eq!(bytes, b"second\n");
        assert_eq!(account.downloads.load(Ordering::SeqCst), 3);
        backend.unwatch(&run).await;
        assert!(lock(&backend.logs).is_empty());
        assert!(!spool_path(&spools, "run").unwrap().exists());
        std::fs::remove_dir_all(spools).unwrap();
    }

    #[tokio::test]
    async fn live_provider_uses_one_stream_across_observation_ticks() {
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        account.pause_drains.store(true, Ordering::SeqCst);
        let clock = Arc::new(TestClock::new());
        let (backend, spools) = adapter(&account, clock.clone());
        assert_eq!(page(&backend).await, b"first\n");
        for _ in 0..3 {
            clock.advance(Duration::from_secs(10));
            backend
                .observe(&[provider_run()], Duration::from_secs(1))
                .await;
            assert_eq!(page(&backend).await, b"first\n");
        }
        assert_eq!(account.downloads.load(Ordering::SeqCst), 1);
        account.text.lock().unwrap().extend_from_slice(b"last\n");
        account.pause_drains.store(false, Ordering::SeqCst);
        account.drain_resumed.notify_one();
        backend.drain_log(&provider_run()).await.unwrap();
        assert_eq!(page(&backend).await, b"first\nlast\n");
        assert_eq!(account.downloads.load(Ordering::SeqCst), 2);
        assert_eq!(account.peak_streams.load(Ordering::SeqCst), 1);
        backend.unwatch(&provider_run()).await;
        std::fs::remove_dir_all(spools).unwrap();
    }

    #[tokio::test]
    async fn terminal_observation_requires_a_pass_after_the_active_stream_finishes() {
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        account.pause_drains.store(true, Ordering::SeqCst);
        let (backend, spools) = adapter(&account, Arc::new(TestClock::new()));
        assert_eq!(page(&backend).await, b"first\n");
        account.terminal.store(true, Ordering::SeqCst);
        let observations = backend
            .observe(&[provider_run()], Duration::from_secs(1))
            .await;
        assert!(observations
            .iter()
            .any(|(_, observation)| observation.ends_run()));
        assert_eq!(account.downloads.load(Ordering::SeqCst), 1);
        account.text.lock().unwrap().extend_from_slice(b"last\n");
        account.pause_drains.store(false, Ordering::SeqCst);
        account.drain_resumed.notify_one();
        backend.drain_log(&provider_run()).await.unwrap();
        assert_eq!(page(&backend).await, b"first\nlast\n");
        assert_eq!(account.downloads.load(Ordering::SeqCst), 2);
        assert_eq!(account.peak_streams.load(Ordering::SeqCst), 1);
        backend.unwatch(&provider_run()).await;
        std::fs::remove_dir_all(spools).unwrap();
    }

    /// A history paused longer than a caller's window is read on by the same stream on the
    /// next call; reopening would meet the same pause forever.
    #[tokio::test]
    async fn a_drain_paused_past_its_window_keeps_one_stream_open() {
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        account.pause_drains.store(true, Ordering::SeqCst);
        let (mut backend, spools) = adapter(&account, Arc::new(TestClock::new()));
        backend.drain_window = Duration::from_millis(20);
        let run = provider_run();
        for _ in 0..3 {
            assert!(backend.drain_log(&run).await.is_err());
        }
        assert_eq!(account.downloads.load(Ordering::SeqCst), 1);
        account.text.lock().unwrap().extend_from_slice(b"last\n");
        account.drain_resumed.notify_one();
        backend.drain_log(&run).await.unwrap();
        assert_eq!(page(&backend).await, b"first\nlast\n");
        assert_eq!(account.downloads.load(Ordering::SeqCst), 1);
        std::fs::remove_dir_all(spools).unwrap();
    }

    /// What an open drain has received is on disk: a viewer reads it at once. A restarted engine
    /// starts over from the provider's history, so what rotated away meanwhile is not kept.
    #[tokio::test]
    async fn an_open_drain_spools_what_it_received_for_viewers_and_restarts() {
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        account.pause_drains.store(true, Ordering::SeqCst);
        let (backend, spools) = adapter(&account, Arc::new(TestClock::new()));
        let backend = Arc::new(backend);
        let draining = {
            let backend = backend.clone();
            tokio::spawn(async move { backend.drain_log(&provider_run()).await })
        };
        account.drain_paused.notified().await;
        assert_eq!(page(&backend).await, b"first\n");

        // The engine stops mid-drain, and the provider rotates its history.
        draining.abort();
        let _ = draining.await;
        drop(backend);
        account.pause_drains.store(false, Ordering::SeqCst);
        account.text.lock().unwrap().extend_from_slice(b"last\n");
        account.rotated.store(6, Ordering::SeqCst);
        account.drain_resumed.notify_waiters();
        tokio::time::timeout(Duration::from_secs(1), async {
            while account.open_streams.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the drain outlived its engine");
        let spool = spool_path(&spools, "run").unwrap();
        assert_eq!(std::fs::read(&spool).unwrap(), b"first\n");
        let restarted = Adapter::new(account.clone(), Arc::new(TestClock::new()), spools.clone());
        restarted.drain_log(&provider_run()).await.unwrap();
        assert_eq!(std::fs::read(&spool).unwrap(), b"last\n");
        assert_eq!(page(&restarted).await, b"last\n");
        std::fs::remove_dir_all(spools).unwrap();
    }

    /// An append that fails part way leaves none of its bytes, so the next follows the last
    /// good one directly.
    #[tokio::test]
    async fn a_failed_append_leaves_nothing_behind() {
        let account = Arc::new(Fake::default());
        let (backend, spools) = adapter(&account, Arc::new(TestClock::new()));
        let log = backend.log(&provider_run()).unwrap();
        let mut spool = lock(&log.spool);
        spool.append(b"head").unwrap();
        spool.fail_after = Some(2);
        assert_eq!(
            spool.append(b"tail").unwrap_err().to_string(),
            "injected write failure"
        );
        spool.append(b"-123").unwrap();
        let path = spool.path.clone();
        drop(spool);
        assert_eq!(std::fs::read(path).unwrap(), b"head-123");
        std::fs::remove_dir_all(spools).unwrap();
    }

    /// Forgetting a run, as collection and deletion do, stops its open drain; nothing more
    /// is written for it.
    #[tokio::test]
    async fn forgetting_a_run_stops_its_drain() {
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        account.pause_drains.store(true, Ordering::SeqCst);
        let (mut backend, spools) = adapter(&account, Arc::new(TestClock::new()));
        backend.drain_window = Duration::from_millis(20);
        let run = provider_run();
        assert!(backend.drain_log(&run).await.is_err());
        assert_eq!(account.open_streams.load(Ordering::SeqCst), 1);
        backend.forget_logs(&|id| id == "run");
        tokio::time::timeout(Duration::from_secs(1), async {
            while account.open_streams.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the drain outlived its run");
        assert!(lock(&backend.logs).is_empty());
        account.text.lock().unwrap().extend_from_slice(b"late\n");
        account.drain_resumed.notify_one();
        tokio::task::yield_now().await;
        assert_eq!(
            std::fs::read(spool_path(&spools, "run").unwrap()).unwrap(),
            b"first\n"
        );
        std::fs::remove_dir_all(spools).unwrap();
    }

    #[tokio::test]
    async fn account_observations_never_exceed_eight_in_flight() {
        let account = Arc::new(Fake::default());
        let (backend, spools) = adapter(&account, Arc::new(TestClock::new()));
        let runs: Vec<_> = (0..50)
            .map(|id| WatchedRun {
                id: id.to_string(),
                handle: crate::engine::adapters::api::test_handle(
                    "modal_job",
                    None,
                    None,
                    vec![id.to_string()],
                ),
            })
            .collect();
        assert_eq!(
            backend.observe(&runs, Duration::from_secs(1)).await.len(),
            50
        );
        assert_eq!(account.peak.load(Ordering::SeqCst), 8);
        let _ = std::fs::remove_dir_all(spools);
    }
    #[tokio::test]
    async fn stalled_stream_does_not_block_observations_or_cached_pages() {
        let account = Arc::new(Fake::default());
        *account.text.lock().unwrap() = b"first\n".to_vec();
        let clock = Arc::new(TestClock::new());
        let (backend, spools) = adapter(&account, clock.clone());
        let run = provider_run();
        backend
            .read_log(&run, LogFile::Log, ByteRange { start: 0, end: 6 }, None)
            .await
            .unwrap();
        account.block_logs.store(true, Ordering::SeqCst);
        account.text.lock().unwrap().extend_from_slice(b"second\n");
        let runs = [run];
        {
            let observing = backend.observe(&runs, Duration::from_millis(100));
            tokio::pin!(observing);
            tokio::select! {
                _ = account.log_started.notified() => {}
                result = &mut observing => panic!("snapshot stream did not start: {result:?}"),
            }
            let page = tokio::time::timeout(
                Duration::from_millis(200),
                backend.read_log(&runs[0], LogFile::Log, ByteRange { start: 0, end: 6 }, None),
            )
            .await
            .expect("cached page was locked behind the network")
            .unwrap();
            assert!(matches!(page, LogPage::Data { bytes, .. } if bytes == b"first\n"));
            let observations = tokio::time::timeout(Duration::from_millis(200), observing)
                .await
                .expect("observation exceeded its log-fetch budget");
            assert_eq!(
                observations,
                vec![
                    ("run".into(), Observation::Running { started_at: None }),
                    ("run".into(), Observation::Log { len: 6 })
                ]
            );
        }
        let log = lock(&backend.logs).get("run").unwrap().clone();
        assert!(lock(&log.spool).error.is_none());
        assert_eq!(lock(&log.spool).cursor, 6);
        account.block_logs.store(false, Ordering::SeqCst);
        account.drain_resumed.notify_one();
        clock.advance(Duration::from_secs(10));
        backend.observe(&runs, Duration::from_secs(1)).await;
        let page = backend
            .read_log(
                &runs[0],
                LogFile::Log,
                ByteRange { start: 6, end: 13 },
                None,
            )
            .await
            .unwrap();
        assert!(matches!(page, LogPage::Data { bytes, .. } if bytes == b"second\n"));
        std::fs::remove_dir_all(spools).unwrap();
    }
}
