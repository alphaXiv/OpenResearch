//! The engine as `orx up` runs it: one adapter per host or account, a loop observing each,
//! a 5 s supervision tick (0.2.x adoption, submission recovery, outage and notify notices),
//! final log collection, and the commands behind API v2.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use async_trait::async_trait;
use once_cell::sync::OnceCell;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{watch, Notify};

use super::adapters::openresearch::LegacyTransport;
use super::adapters::{api, local::Local, openresearch, slurm::Slurm, ssh::Ssh};
use super::clock::Clock;
use super::handle::ApiAccount;
use super::transport::{Auth, Config, HostConn, Prompt, Status};
use super::{
    handle_of, is_engine_row, Backend, ByteRange, Engine, FileIdent, Handle, LogFile, LogPage,
    Observation, Phase, WatchedRun,
};
use crate::commands::up::api_v2::{Commands, KeptRange};
use crate::error::{anyhow, bail, Result};
use crate::jobs::ssh::SshTarget;
use crate::store::{LaunchSpec, Store, StoredRun, SubmittingRun};
use orx_host_proto::LogInfo;

const DEFAULT_ROOT: &str = "~/.orx/engine";
/// Logs up to this size are kept whole; larger ones keep the head and the tail.
const KEEP_WHOLE: u64 = 1 << 30;
const KEEP_HEAD: u64 = 64 << 20;
const KEEP_TAIL: u64 = 256 << 20;
const COLLECTION_BUDGET: Duration = Duration::from_secs(120);
/// How long finishing runs' logs may hold up an adapter's observations so their notices can
/// quote them.
const EARLY_COLLECTION_BUDGET: Duration = Duration::from_secs(3);
pub(super) const PREPARATION_BUDGET: Duration = Duration::from_secs(10 * 60);

/// Engine run directories live beside, never inside, the 0.2.x `<root>/runs`.
pub fn engine_root(remote_root: Option<&str>) -> String {
    remote_root.map_or_else(|| DEFAULT_ROOT.into(), |root| format!("{root}/engine"))
}

fn box_alias(target: &SshTarget) -> Result<String> {
    let identity =
        serde_json::to_vec(&(&target.dest, &target.extra_opts, &target.managed_generation))?;
    Ok(format!("orx-box-{}", &hex(&Sha256::digest(identity))[..16]))
}

/// The alias an OpenResearch run's box is reached through, once the box has an endpoint.
fn box_of(run: &StoredRun) -> Option<String> {
    match handle_of(run)? {
        Handle::OpenResearch { descriptor } | Handle::Legacy { descriptor }
            if descriptor.kind == "openresearch_job" =>
        {
            box_alias(&descriptor.openresearch_ssh_target()?).ok()
        }
        _ => None,
    }
}

/// What one adapter instance serves.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Ssh {
        host: String,
        root: String,
    },
    Slurm {
        host: String,
        root: String,
    },
    Local,
    Api {
        provider: String,
        account: ApiAccount,
    },
    OpenResearch {
        rental: Option<String>,
    },
}

impl Key {
    /// The adapter serving runs of `kind` on `target`: their host, namespace or address.
    fn of_kind(
        kind: &str,
        target: Option<String>,
        root: String,
        context: Option<String>,
    ) -> Result<Self> {
        let required_target = || {
            target
                .clone()
                .ok_or_else(|| anyhow!("The {kind} run has no target"))
        };
        Ok(match kind {
            "ssh_job" => Self::Ssh {
                host: required_target()?,
                root,
            },
            "slurm_job" => Self::Slurm {
                host: required_target()?,
                root,
            },
            "local_job" | "tinker_job" => Self::Local,
            "openresearch_job" => Self::OpenResearch { rental: None },
            kind => {
                api::provider(kind)?;
                Self::Api {
                    provider: kind.into(),
                    account: ApiAccount { target, context },
                }
            }
        })
    }

    fn of_launch(launch: &LaunchSpec) -> Result<Self> {
        let root = launch.options["root"].as_str().unwrap_or(DEFAULT_ROOT);
        let context = launch.options["context"].as_str().map(str::to_owned);
        Self::of_kind(&launch.kind, launch.target.clone(), root.into(), context)
    }

    fn of_handle(handle: &Handle) -> Result<Self> {
        Ok(match handle {
            Handle::OpenResearch { descriptor } => Self::OpenResearch {
                rental: descriptor.job_id.clone(),
            },
            Handle::Ssh { host, root } => Self::Ssh {
                host: host.clone(),
                root: root.clone(),
            },
            Handle::Slurm { host, root, .. } => Self::Slurm {
                host: host.clone(),
                root: root.clone(),
            },
            Handle::Local { .. } | Handle::Tinker { .. } => Self::Local,
            Handle::Api {
                provider, account, ..
            } => Self::Api {
                provider: provider.clone(),
                account: account.clone(),
            },
            Handle::Legacy { descriptor } if descriptor.kind == "openresearch_job" => {
                Self::OpenResearch {
                    rental: descriptor.job_id.clone(),
                }
            }
            Handle::Legacy { descriptor } => Self::of_kind(
                &descriptor.kind,
                descriptor.namespace.clone(),
                engine_root(descriptor.remote_root.as_deref()),
                descriptor.context.clone(),
            )?,
        })
    }

    /// How long one `observe` may wait: host feeds long-poll, API accounts pace themselves.
    fn budget(&self) -> Duration {
        match self {
            Self::Ssh { .. } | Self::Slurm { .. } => Duration::from_secs(25),
            Self::Local => Duration::from_secs(1),
            _ => Duration::from_secs(10),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

type HostCell = Arc<OnceCell<HostConn>>;

pub struct Runtime {
    engine: Arc<Engine>,
    clock: Arc<dyn Clock>,
    data_dir: PathBuf,
    store: OnceCell<Mutex<Store>>,
    /// `ssh -F` for every configured host; tests point it at a hermetic sshd.
    ssh_config: Option<PathBuf>,
    me: Weak<Runtime>,
    conns: Mutex<HashMap<(String, String), HostCell>>,
    auth: Mutex<HashMap<String, Arc<Auth>>>,
    hosts: Mutex<HashMap<Key, Arc<Ssh>>>,
    backends: Mutex<HashMap<Key, Arc<dyn Backend>>>,
    boxes: Mutex<HashMap<String, Arc<LegacyTransport>>>,
    watched: Mutex<HashMap<Key, watch::Sender<Vec<WatchedRun>>>>,
    changed: Notify,
    collect_now: Notify,
    /// Held while a local run is unfinished; a failure is kept so it is logged once.
    #[cfg(windows)]
    awake: Mutex<Option<std::io::Result<std::os::windows::io::OwnedHandle>>>,
}

impl Runtime {
    pub fn new(
        engine: Arc<Engine>,
        clock: Arc<dyn Clock>,
        data_dir: PathBuf,
        ssh_config: Option<PathBuf>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            engine,
            clock,
            data_dir,
            store: OnceCell::new(),
            ssh_config,
            me: me.clone(),
            conns: Mutex::default(),
            auth: Mutex::default(),
            hosts: Mutex::default(),
            backends: Mutex::default(),
            boxes: Mutex::default(),
            watched: Mutex::default(),
            changed: Notify::new(),
            collect_now: Notify::new(),
            #[cfg(windows)]
            awake: Mutex::default(),
        })
    }

    pub fn start(self: &Arc<Self>) {
        tokio::spawn(self.clone().supervise());
        tokio::spawn(self.clone().collect_pending());
    }

    fn store(&self) -> Result<MutexGuard<'_, Store>> {
        let store = self
            .store
            .get_or_try_init(|| Store::open_at(self.data_dir.clone()).map(Mutex::new))?;
        Ok(lock(store))
    }

    fn conn(&self, alias: &str, root: &str, ssh_config: Option<PathBuf>) -> Result<HostConn> {
        let key = (alias.to_owned(), root.to_owned());
        let cell = lock(&self.conns).entry(key).or_default().clone();
        cell.get_or_try_init(|| {
            let auth = {
                let mut aliases = lock(&self.auth);
                match aliases.entry(alias.into()) {
                    std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(Auth::new(&*self.store()?, alias)?).clone()
                    }
                }
            };
            let conn = HostConn::start_shared(
                Config {
                    alias: alias.into(),
                    root: root.into(),
                    ssh_config,
                    #[cfg(unix)]
                    control_dir: control_dir(&self.data_dir),
                    binary_dir: None,
                    #[cfg(unix)]
                    sharing_home: crate::config::home_dir(),
                    askpass_executable: crate::paths::spawnable_exe()?,
                },
                Store::open_at(self.data_dir.clone())?,
                self.clock.clone(),
                auth,
            )?;
            let mut state = conn.state();
            let engine = self.engine.clone();
            let me = self.me.clone();
            let alias = alias.to_owned();
            let root = root.to_owned();
            tokio::spawn(async move {
                let mut live = false;
                loop {
                    let snapshot = state.borrow_and_update().clone();
                    let now = snapshot.status == Status::Live;
                    if now && !live {
                        if let Some(Err(error)) =
                            me.upgrade().map(|me| me.host_returned(&alias, &root))
                        {
                            eprintln!("orx up: {alias}: log collection: {error:#}");
                        }
                    }
                    live = now;
                    engine.bus.publish(super::bus::RunEvent::Host {
                        alias: alias.clone(),
                        root: root.clone(),
                        state: snapshot,
                    });
                    if state.changed().await.is_err() {
                        break;
                    }
                }
            });
            Ok::<_, crate::error::Error>(conn)
        })
        .cloned()
    }

    /// Ssh and Slurm keep separate feeds over the one connection to a host.
    fn ssh(&self, key: &Key) -> Result<Arc<Ssh>> {
        let (Key::Ssh { host, root } | Key::Slurm { host, root }) = key else {
            bail!("{key:?} is not an SSH host");
        };
        if let Some(ssh) = lock(&self.hosts).get(key) {
            return Ok(ssh.clone());
        }
        let conn = self.conn(host, root, self.ssh_config.clone())?;
        let ssh = Arc::new(Ssh::new(host.clone(), root.clone(), conn));
        Ok(lock(&self.hosts).entry(key.clone()).or_insert(ssh).clone())
    }

    fn backend_for(&self, key: &Key) -> Result<Arc<dyn Backend>> {
        if let Some(backend) = lock(&self.backends).get(key) {
            return Ok(backend.clone());
        }
        let clock = self.clock.clone();
        let spools = self.data_dir.join("run-logs");
        let backend: Arc<dyn Backend> = match key {
            Key::Ssh { .. } => self.ssh(key)?,
            Key::Slurm { .. } => Arc::new(Slurm::new(self.ssh(key)?, clock)),
            Key::Local => Arc::new(Local::new(self.data_dir.join("local-runs"))),
            Key::Api { provider, account } => {
                (api::provider(provider)?.build)(account, clock, spools)?
            }
            Key::OpenResearch { .. } => {
                let me = self.me.clone();
                Arc::new(openresearch::OpenResearch::new(
                    self.engine.clone(),
                    Arc::new(move |target: &SshTarget| {
                        me.upgrade()
                            .ok_or_else(|| anyhow!("The run engine stopped"))?
                            .box_transport(target)
                    }),
                ))
            }
        };
        Ok(lock(&self.backends)
            .entry(key.clone())
            .or_insert(backend)
            .clone())
    }

    fn key(&self, run: &StoredRun) -> Result<Key> {
        if let Some(handle) = handle_of(run) {
            return Key::of_handle(&handle);
        }
        let launch = self
            .store()?
            .engine_run_state(&run.id)?
            .and_then(|state| state.launch)
            .ok_or_else(|| anyhow!("Run {} has neither a handle nor a launch intent", run.id))?;
        Key::of_launch(&launch)
    }

    /// An OpenResearch box is reached through a generated `ssh -F` file that layers its
    /// endpoint over the user's own configuration.
    fn box_transport(&self, target: &SshTarget) -> Result<Arc<LegacyTransport>> {
        let alias = box_alias(target)?;
        if let Some(transport) = lock(&self.boxes).get(&alias) {
            return Ok(transport.clone());
        }
        let (user, hostname) = target
            .dest
            .split_once('@')
            .ok_or_else(|| anyhow!("Box endpoint {} has no user", target.dest))?;
        let mut config = format!("Host {alias}\n  HostName {hostname}\n  User {user}\n");
        let mut options = target.extra_opts.iter();
        while let Some(flag) = options.next() {
            let value = options
                .next()
                .ok_or_else(|| anyhow!("Box SSH option {flag} has no value"))?;
            match flag.as_str() {
                "-p" => config.push_str(&format!("  Port {value}\n")),
                "-o" => {
                    let (name, value) = value
                        .split_once('=')
                        .ok_or_else(|| anyhow!("Unexpected box SSH option {value}"))?;
                    config.push_str(&format!("  {name} {value}\n"));
                }
                _ => bail!("Unexpected box SSH option {flag}"),
            }
        }
        if crate::config::home_dir().is_some_and(|home| home.join(".ssh/config").is_file()) {
            config.push_str("Host *\n  Include ~/.ssh/config\n");
        }
        let directory = self.data_dir.join("engine-ssh");
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{alias}.config"));
        crate::local::git::atomic_write_with_mode(&path, config.as_bytes(), Some(0o600))?;
        let conn = self.conn(&alias, DEFAULT_ROOT, Some(path))?;
        let ssh = Arc::new(Ssh::new(alias.clone(), DEFAULT_ROOT.into(), conn));
        let transport = Arc::new(LegacyTransport::new(ssh, self.data_dir.join("run-logs")));
        Ok(lock(&self.boxes).entry(alias).or_insert(transport).clone())
    }

    async fn supervise(self: Arc<Self>) {
        let home = crate::config::home_dir();
        let mut retired = home.is_none();
        let mut retire_after = tokio::time::Instant::now();
        loop {
            if !retired && tokio::time::Instant::now() >= retire_after {
                match self
                    .engine
                    .retire_ssh_config(home.as_deref().unwrap_or(Path::new("/")))
                {
                    Ok(done) => retired = done,
                    Err(error) => {
                        retire_after = tokio::time::Instant::now() + Duration::from_secs(300);
                        eprintln!(
                            "orx up: could not retire the 0.2.x SSH configuration: {error:#}"
                        );
                    }
                }
            }
            if let Err(error) = self.tick() {
                eprintln!("orx up: run engine: {error:#}");
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = self.changed.notified() => {}
            }
        }
    }

    fn tick(self: &Arc<Self>) -> Result<()> {
        self.engine.scan_legacy()?;
        self.engine.check_outages()?;
        let store = self.store()?;
        // Deleted from the dashboard or the CLI alike.
        let deleted = |run: &str| store.get_run(run).is_ok_and(|row| row.is_none());
        let backends: Vec<_> = lock(&self.backends).values().cloned().collect();
        for backend in backends {
            backend.forget_logs(&deleted);
        }
        let runs = store.list_engine_runs()?;
        drop(store);
        let mut watch: HashMap<Key, Vec<WatchedRun>> = HashMap::new();
        #[cfg(windows)]
        let mut local_unfinished = false;
        for run in runs {
            // 0.2.x rows are touched only once adoption holds their locks.
            if !is_engine_row(&run) && !self.engine.holds(&run.id) {
                continue;
            }
            let key = match self.key(&run) {
                Ok(key) => key,
                Err(error) => {
                    self.engine.unreconciled(&run.id, &error)?;
                    continue;
                }
            };
            #[cfg(windows)]
            {
                local_unfinished |= key == Key::Local && !crate::local::is_terminal(&run.status);
            }
            match handle_of(&run) {
                Some(handle) => watch
                    .entry(key)
                    .or_default()
                    .push(WatchedRun { id: run.id, handle }),
                None => {
                    let me = self.clone();
                    tokio::spawn(async move {
                        let result = match me.backend_for(&key) {
                            Ok(backend) => me.engine.recover(backend.as_ref(), &run.id).await,
                            Err(error) => me.engine.unreconciled(&run.id, &error),
                        };
                        if let Err(error) = result {
                            eprintln!("orx up: run {}: recovery: {error:#}", run.id);
                        }
                    });
                }
            }
        }
        #[cfg(windows)]
        {
            let mut awake = lock(&self.awake);
            if !local_unfinished {
                *awake = None;
            } else {
                // Renewed every tick: Windows ends power requests on lid close or Start → Sleep.
                let held = super::adapters::local::keep_awake();
                if let Err(error) = &held {
                    if !matches!(*awake, Some(Err(_))) {
                        eprintln!(
                            "orx up: could not keep this machine awake for local runs: {error}"
                        );
                    }
                }
                *awake = Some(held);
            }
        }
        for run_id in self.engine.notify_due()? {
            let row = self.store()?.get_run(&run_id)?;
            let Some(key) = row.and_then(|run| self.key(&run).ok()) else {
                continue;
            };
            let me = self.clone();
            tokio::spawn(async move {
                let result = match me.backend_for(&key) {
                    Ok(backend) => me
                        .engine
                        .deliver_notify(backend.as_ref(), &run_id)
                        .await
                        .map(|_| ()),
                    Err(error) => Err(error),
                };
                if let Err(error) = result {
                    eprintln!("orx up: run {run_id}: notify: {error:#}");
                }
            });
        }

        let mut senders = lock(&self.watched);
        senders.retain(|key, sender| {
            if watch.contains_key(key) {
                return true;
            }
            sender.send_if_modified(|runs| !std::mem::take(runs).is_empty());
            !matches!(key, Key::OpenResearch { rental: Some(_) })
        });
        lock(&self.backends).retain(|key, _| {
            !matches!(key, Key::OpenResearch { rental: Some(_) }) || watch.contains_key(key)
        });
        for (key, runs) in watch {
            let ids =
                |runs: &[WatchedRun]| runs.iter().map(|run| run.id.clone()).collect::<Vec<_>>();
            match senders.get(&key) {
                Some(sender) => {
                    sender.send_if_modified(|current| {
                        let modified = ids(current) != ids(&runs);
                        *current = runs;
                        modified
                    });
                }
                None => {
                    let (sender, receiver) = watch::channel(runs);
                    tokio::spawn(self.clone().observe(key.clone(), receiver));
                    senders.insert(key, sender);
                }
            }
        }

        drop(senders);
        self.retire_boxes()?;
        self.engine.store().migrate_run_wakeups()
    }

    /// Stops dialing a box no unfinished run or pending log collection needs, and forgets its
    /// connection state.
    fn retire_boxes(&self) -> Result<()> {
        // Taken before reading runs: a box connection exists only once its run's endpoint is stored.
        let open: Vec<String> = lock(&self.conns)
            .keys()
            .map(|(alias, _)| alias.clone())
            .collect();
        let store = self.store()?;
        let mut needed = std::collections::HashSet::new();
        for run in store.list_engine_runs()? {
            needed.extend(box_of(&run));
        }
        for run_id in store.pending_log_collections()? {
            needed.extend(store.get_run(&run_id)?.as_ref().and_then(box_of));
        }
        let retired: std::collections::HashSet<String> = open
            .into_iter()
            .chain(store.engine_hosts()?.into_iter().map(|(alias, ..)| alias))
            .filter(|alias| super::transport::managed(alias) && !needed.contains(alias))
            .collect();
        drop(store);
        if retired.is_empty() {
            return Ok(());
        }
        lock(&self.boxes).retain(|alias, _| !retired.contains(alias));
        lock(&self.conns).retain(|(alias, _), _| !retired.contains(alias));
        lock(&self.auth).retain(|alias, _| !retired.contains(alias));
        // A closing connection may save once more; the next tick forgets it again.
        let store = self.store()?;
        for alias in &retired {
            store.forget_engine_host(alias)?;
        }
        Ok(())
    }

    /// One loop per adapter. A changed watch set interrupts a pending long poll.
    async fn observe(self: Arc<Self>, key: Key, mut runs: watch::Receiver<Vec<WatchedRun>>) {
        loop {
            let watched = runs.borrow_and_update().clone();
            if watched.is_empty() {
                if runs.changed().await.is_err() {
                    return;
                }
                continue;
            }
            let backend = match self.backend_for(&key) {
                Ok(backend) => backend,
                Err(error) => {
                    for run in &watched {
                        if let Err(error) = self.engine.unreconciled(&run.id, &error) {
                            eprintln!("orx up: run {}: {error:#}", run.id);
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    continue;
                }
            };
            let started = tokio::time::Instant::now();
            let observations = tokio::select! {
                observations = backend.observe(&watched, key.budget()) => observations,
                changed = runs.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    continue;
                }
            };
            let finished = observations
                .iter()
                .any(|(_, observation)| observation.ends_run());
            if finished {
                let collecting = self.collect_logs(backend.as_ref(), &watched, &observations);
                // Other runs' phases wait on this; what is left resumes in `collect_pending`.
                let _ = tokio::time::timeout(EARLY_COLLECTION_BUDGET, collecting).await;
            }
            self.engine
                .apply_observations(backend.as_ref(), &watched, observations)
                .await;
            if finished {
                self.changed.notify_one();
            }
            // Adapters that are not yet due answer at once; never spin on them.
            tokio::time::sleep_until(started + Duration::from_secs(1)).await;
        }
    }

    /// Starts keeping the log of every run about to become terminal, so later reads need no
    /// host. What the early budget leaves is finished by [`Self::collect_pending`].
    async fn collect_logs(
        &self,
        backend: &dyn Backend,
        watched: &[WatchedRun],
        observations: &[(String, Observation)],
    ) {
        for run in watched {
            if !observations
                .iter()
                .any(|(id, observation)| *id == run.id && observation.ends_run())
            {
                continue;
            }
            let terminal = self
                .store()
                .and_then(|store| store.get_run(&run.id))
                .ok()
                .flatten()
                .and_then(|row| Phase::parse(&row.status))
                .is_none_or(Phase::is_terminal);
            if terminal {
                continue;
            }
            if let Err(error) = self.collect(backend, run, COLLECTION_BUDGET).await {
                eprintln!("orx up: run {}: log collection: {error:#}", run.id);
            }
        }
    }

    /// Retries, across restarts, the logs of finished runs that could not be kept in time.
    /// A starting engine retries them all at once.
    async fn collect_pending(self: Arc<Self>) {
        if let Err(error) = self
            .store()
            .and_then(|store| store.retry_log_collections_now(None))
        {
            eprintln!("orx up: log collection: {error:#}");
        }
        loop {
            match self.store().and_then(|store| store.due_log_collections(10)) {
                Ok(due) => {
                    let full = due.len() == 10;
                    for run_id in &due {
                        if let Err(error) = self.collect_finished(run_id).await {
                            eprintln!("orx up: run {run_id}: log collection: {error:#}");
                        }
                    }
                    if full
                        && self
                            .store()
                            .and_then(|store| store.due_log_collections(10))
                            .is_ok_and(|next| due.iter().any(|id| !next.contains(id)))
                    {
                        continue;
                    }
                }
                Err(error) => eprintln!("orx up: log collection: {error:#}"),
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(30)) => {}
                _ = self.collect_now.notified() => {}
            }
        }
    }

    async fn collect_finished(&self, run_id: &str) -> Result<()> {
        let Some(run) = self.store()?.get_run(run_id)? else {
            return Ok(());
        };
        let collected = async {
            let Some(handle) = handle_of(&run) else {
                return self.store()?.finish_log_collection(run_id);
            };
            let run = WatchedRun { id: run.id, handle };
            let backend = self.backend_for(&Key::of_handle(&run.handle)?)?;
            if !self.store()?.log_output_collected(run_id)? {
                if !self
                    .collect(backend.as_ref(), &run, COLLECTION_BUDGET)
                    .await?
                {
                    return Ok(());
                }
                let (_, state) = self.engine.load(run_id)?;
                if state.contact != crate::store::Contact::Live {
                    return Ok(());
                }
                if !self.engine.deliver_notify(backend.as_ref(), run_id).await? {
                    bail!("Final notify collection is still pending");
                }
                self.store()?.checkpoint_log_collection(run_id)?;
            }
            backend.finish(&run).await?;
            {
                let store = self.store()?;
                let tx = store.begin_immediate()?;
                store.finish_log_collection(run_id)?;
                store.queue_finished_remote_cleanup(run_id)?;
                tx.commit()?;
            }
            backend.unwatch(&run).await;
            Ok(())
        }
        .await;
        if let Err(error) = &collected {
            let error = format!("{error:#}");
            if self.store()?.defer_log_collection(run_id, &error)? {
                let body = format!(
                    "[orx] Run `{run_id}`'s output collection could not finish ({error}). orx keeps retrying hourly, and at once when its host reconnects."
                );
                self.engine.attention(run_id, "log-collection", &body)?;
            }
        }
        self.engine
            .bus
            .publish(super::bus::RunEvent::LogCollection {
                run_id: run_id.into(),
            });
        collected
    }

    fn host_returned(&self, alias: &str, root: &str) -> Result<()> {
        let store = self.store()?;
        for run_id in store.pending_log_collections()? {
            let on_host = store
                .get_run(&run_id)?
                .is_some_and(|run| match box_of(&run) {
                    Some(host) => host == alias && root == DEFAULT_ROOT,
                    None => handle_of(&run)
                        .and_then(|handle| Key::of_handle(&handle).ok())
                        .is_some_and(|key| {
                            matches!(&key, Key::Ssh { host, root: r } | Key::Slurm { host, root: r }
                            if host == alias && r == root)
                        }),
                });
            if on_host {
                store.retry_log_collections_now(Some(&run_id))?;
                self.collect_now.notify_one();
            }
        }
        Ok(())
    }

    async fn collect(
        &self,
        backend: &dyn Backend,
        run: &WatchedRun,
        budget: Duration,
    ) -> Result<bool> {
        let mut progressed = false;
        let collected = async {
            backend.drain_log(run).await?;
            Self::collect_log(&self.data_dir, backend, run, &mut progressed).await
        };
        match tokio::time::timeout(budget, collected).await {
            Ok(result) => result?,
            Err(_) if progressed => return Ok(false),
            Err(_) => bail!("timed out without keeping more log bytes"),
        }
        Ok(true)
    }

    /// Keeps the log by the keep policy, up to the length the host reported when collection
    /// started. A partial range resumes from the length its marker records as synced, when it
    /// was started for this generation at this offset; every page checks the generation.
    pub(crate) async fn collect_log(
        data_dir: &Path,
        backend: &dyn Backend,
        run: &WatchedRun,
        progressed: &mut bool,
    ) -> Result<()> {
        let Some(info) = Self::log_info(backend, run).await? else {
            return Ok(());
        };
        let ident = info.ident;
        let directory = data_dir.join("run-logs");
        for suffix in ["log", "tail.log"] {
            let part = directory.join(format!("{}.{suffix}.part", run.id));
            let marker = directory.join(format!("{}.{suffix}.part.json", run.id));
            let obsolete = std::fs::read(&marker)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<(FileIdent, u64, u64)>(&bytes).ok())
                .is_some_and(|(generation, _, _)| generation != ident);
            if obsolete {
                for path in [part, marker] {
                    match std::fs::remove_file(path) {
                        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                            return Err(error.into());
                        }
                        _ => {}
                    }
                }
            }
        }
        let kept = crate::commands::up::api_v2::kept_ranges(data_dir, &run.id)?;
        if kept.last().is_some_and(|last| {
            last.eof && last.ident == ident && last.offset + last.len == info.len
        }) {
            return Ok(());
        }
        let ranges = if info.len > KEEP_WHOLE {
            vec![
                (0, KEEP_HEAD, "log"),
                (info.len - KEEP_TAIL, info.len, "tail.log"),
            ]
        } else {
            vec![(0, info.len, "log")]
        };
        let last = ranges.len() - 1;
        std::fs::create_dir_all(&directory)?;
        for (index, (start, end, suffix)) in ranges.into_iter().enumerate() {
            let path = directory.join(format!("{}.{suffix}", run.id));
            let part = directory.join(format!("{}.{suffix}.part", run.id));
            let marker = directory.join(format!("{}.{suffix}.part.json", run.id));
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&part)?;
            let recorded = std::fs::read(&marker)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<(FileIdent, u64, u64)>(&bytes).ok())
                .filter(|(kept, at, synced)| {
                    *kept == ident && *at == start && *synced <= end - start
                });
            let synced = recorded.map_or(0, |(_, _, synced)| synced);
            let synced = synced.min(file.metadata()?.len());
            file.set_len(synced)?;
            file.seek(SeekFrom::Start(synced))?;
            let mut cursor = start + synced;
            let mut eof;
            loop {
                let range = ByteRange {
                    start: cursor,
                    end: end.min(cursor + orx_host_proto::MAX_PAGE_LEN),
                };
                match backend
                    .read_log(run, LogFile::Log, range, Some(&ident))
                    .await?
                {
                    LogPage::Data {
                        start: at,
                        bytes,
                        eof: at_end,
                        ..
                    } if at == cursor => {
                        if !bytes.is_empty() {
                            std::io::Write::write_all(&mut file, &bytes)?;
                            file.sync_data()?;
                            cursor += bytes.len() as u64;
                            let progress = serde_json::to_vec(&(&ident, start, cursor - start))?;
                            crate::local::git::atomic_write_with_mode(
                                &marker,
                                &progress,
                                Some(0o600),
                            )?;
                            *progressed = true;
                        }
                        eof = at_end;
                    }
                    page => bail!("The log changed during collection: {page:?}"),
                }
                if cursor >= end || eof {
                    break;
                }
            }
            if cursor < end {
                bail!("The log ended at byte {cursor} of {end}");
            }
            drop(file);
            std::fs::rename(&part, &path)?;
            let mut kept = crate::commands::up::api_v2::kept_ranges(data_dir, &run.id)?;
            kept.retain(|range| {
                range.ident == ident
                    && range.file != path
                    && (index != last || range.offset < start)
            });
            for range in &mut kept {
                range.eof = false;
            }
            kept.push(KeptRange {
                offset: start,
                len: cursor - start,
                file: path,
                ident: ident.clone(),
                eof: index == last && eof,
            });
            kept.sort_by_key(|range| range.offset);
            crate::commands::up::api_v2::keep_ranges(data_dir, &run.id, kept)?;
            match std::fs::remove_file(&marker) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(error.into())
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn log_info(backend: &dyn Backend, run: &WatchedRun) -> Result<Option<LogInfo>> {
        let probe = ByteRange {
            start: u64::MAX - 1,
            end: u64::MAX,
        };
        match backend.read_log(run, LogFile::Log, probe, None).await? {
            LogPage::Reset { info } => Ok(info),
            LogPage::Data { .. } => bail!("The host answered a length probe with data"),
        }
    }

    /// The connection a host's runs use: SSH and Slurm keep their own engine roots.
    fn host_key(host: &str, slurm: bool) -> Result<Key> {
        Ok(if slurm {
            let settings = crate::jobs::slurm::load_settings()?.unwrap_or_default();
            Key::Slurm {
                host: host.into(),
                root: engine_root(settings.remote_root.as_deref()),
            }
        } else {
            let saved = crate::config::ssh_settings()?.hosts.remove(host);
            Key::Ssh {
                host: host.into(),
                root: engine_root(saved.and_then(|saved| saved.remote_root).as_deref()),
            }
        })
    }

    fn target(&self, key: &Key) -> Result<SshTarget> {
        let ssh = self.ssh(key)?;
        let mut target = SshTarget::alias(&ssh.host);
        target.transport = Some(Arc::new(LegacyTransport::new(
            ssh,
            self.data_dir.join("run-logs"),
        )));
        Ok(target)
    }
}

#[async_trait]
impl Commands for Runtime {
    async fn run(&self, args: crate::ExpRunArgs, admission: Box<dyn Send>) -> Result<String> {
        let chat_session = args.launching_chat_session();
        let (durable, inserted) = tokio::sync::oneshot::channel::<()>();
        let submitted = crate::compute::submit(&args, |mut submission| async move {
            // A saved container is resolved over the engine's own connection, so first
            // contact follows the engine's authentication rules.
            if submission.launch.kind == "ssh_job" {
                let mut options: super::adapters::ssh::LaunchOptions =
                    serde_json::from_value(submission.launch.options.clone())?;
                if let Some(reference) = options.container_reference.take() {
                    let target = self.target(&Key::of_launch(&submission.launch)?)?;
                    let container =
                        crate::jobs::ssh::resolve_container(&target, &reference).await?;
                    options.container = Some(container.for_run(&submission.run_id));
                    submission.launch.options = serde_json::to_value(options)?;
                }
            }
            let backend = self.backend_for(&Key::of_launch(&submission.launch)?)?;
            let mut metadata = crate::jobs::BackendDescriptor::parse(r#"{"kind":"engine"}"#)?;
            submission.source.apply_to_descriptor(&mut metadata);
            let digest = hex(&Sha256::digest(
                serde_json::to_vec(&json!([
                    submission.launch,
                    submission.command,
                    submission.source.digest,
                ]))?
                .as_slice(),
            ));
            self.engine.insert(SubmittingRun {
                id: &submission.run_id,
                experiment_id: &submission.experiment.id,
                project_id: &submission.experiment.project_id,
                metadata: &metadata,
                command: &submission.command,
                created_at: crate::store::now_ms(),
                commit_sha: Some(&submission.source.revision),
                chat_session_id: chat_session.as_deref(),
                spec_digest: &digest,
                launch: &submission.launch,
            })?;
            drop(admission);
            let _ = durable.send(());
            self.engine
                .launch_inserted(backend.as_ref(), &submission.run_id)
                .await
        });
        // Only preparation is bounded; once the run is durable, recovery owns its launch.
        let unprepared = async {
            if tokio::time::timeout(PREPARATION_BUDGET, inserted)
                .await
                .is_ok()
            {
                std::future::pending::<()>().await;
            }
        };
        let run_id = tokio::select! {
            run_id = submitted => run_id?,
            () = unprepared => bail!(
                "Preparing the run took over {} minutes; check the compute backend and retry.",
                PREPARATION_BUDGET.as_secs() / 60
            ),
        };
        self.changed.notify_one();
        Ok(run_id)
    }

    fn backend(&self, run: &StoredRun) -> Result<Arc<dyn Backend>> {
        self.backend_for(&self.key(run)?)
    }

    /// Accepts a cancel for a run the engine owns, or records it for the 0.2.x supervisor
    /// that does.
    async fn cancel(&self, run: &StoredRun, reason: &str) -> Result<()> {
        if !is_engine_row(run)
            && !self.engine.holds(&run.id)
            && !crate::local::is_terminal(&run.status)
        {
            return crate::commands::exp::request_legacy_run_cancel(
                &*self.store()?,
                &run.id,
                reason,
            );
        }
        match self.key(run).and_then(|key| self.backend_for(&key)) {
            Ok(backend) => {
                self.engine
                    .cancel(backend.as_ref(), &run.id, reason)
                    .await?;
            }
            // The tick shows the run unreconciled with this error; delivery waits for an adapter.
            Err(_) => {
                self.engine.accept_cancel(&run.id, reason)?;
            }
        }
        self.changed.notify_one();
        Ok(())
    }

    async fn cleanup(&self, run: &WatchedRun) -> Result<Value> {
        if matches!(run.handle, Handle::Local { .. } | Handle::Tinker { .. }) {
            return Ok(serde_json::to_value(
                Local::new(self.data_dir.join("local-runs")).cleanup(run)?,
            )?);
        }
        if matches!(&run.handle, Handle::OpenResearch { .. }) {
            let backend = self.backend_for(&Key::of_handle(&run.handle)?)?;
            return match backend.cleanup(run).await {
                super::CancelOutcome::Verified => Ok(serde_json::to_value(
                    orx_host_proto::CleanupAck::NothingLingering,
                )?),
                super::CancelOutcome::Undelivered { detail } => bail!("{detail}"),
                _ => Ok(serde_json::to_value(orx_host_proto::CleanupAck::Accepted)?),
            };
        }
        let key = Key::of_handle(&run.handle)?;
        let ssh = self.ssh(&key)?;
        Ok(serde_json::to_value(ssh.cleanup(run).await?)?)
    }

    async fn compute(&self, action: &str, args: Value, prompt: Option<Prompt>) -> Result<Value> {
        let backend = args["backend"].as_str().unwrap_or_default();
        let host = args["host"].as_str().map(str::to_owned);
        if action == "connect" && matches!(backend, "ssh" | "slurm") {
            let slurm = backend == "slurm";
            let host = match &host {
                Some(host) => host.clone(),
                None if slurm => crate::jobs::slurm::load_settings()?
                    .unwrap_or_default()
                    .host
                    .ok_or_else(|| anyhow!("Pass --host or configure a default host."))?,
                None => crate::config::ssh_settings()?
                    .default_host
                    .ok_or_else(|| anyhow!("Pass --host or configure a default host."))?,
            };
            let supervised = || -> Result<bool> {
                Ok(crate::compute::supervised_ssh_needed(
                    &self.store()?.list_active_runs()?,
                    &host,
                    |id| self.engine.holds(id),
                ))
            };
            let keep_master = supervised()?;
            let target = SshTarget::alias(&host);
            #[cfg(unix)]
            if keep_master && prompt.is_some() {
                drop(crate::jobs::ssh::prepare_interactive_connection(&target, None, None).await?);
            }
            self.ssh(&Self::host_key(&host, slurm)?)?;
            let connections: Vec<_> = lock(&self.conns)
                .iter()
                .filter(|((alias, _), _)| alias == &host)
                .filter_map(|(_, cell)| cell.get().cloned())
                .collect();
            let mut worst: Option<(u8, anyhow::Error)> = None;
            for conn in connections {
                let result = match &prompt {
                    Some(prompt) => conn.connect_interactive(prompt.clone(), keep_master).await,
                    None => conn.retry().await,
                };
                if let Err(error) = result {
                    let severity = conn.state().borrow().status.severity();
                    if worst.as_ref().is_none_or(|(worst, _)| severity > *worst) {
                        worst = Some((severity, error));
                    }
                }
            }
            if let Err(error) = crate::jobs::ssh::service::reset_existing_backoff(&target).await {
                eprintln!("Host {host}: could not reset supervisor SSH backoff: {error:#}");
            }
            let connected = worst.map_or(Ok(()), |(_, error)| Err(error));
            // The engine may have adopted the host's last 0.2.x run while the user authenticated.
            #[cfg(windows)]
            let connected = if supervised()? {
                connect_supervised(connected, prompt.as_ref(), |operation| {
                    crate::jobs::ssh::service::request(&target, operation)
                })
                .await
            } else {
                connected
            };
            connected?;
        }
        crate::commands::up::compute_settings::check(
            backend,
            host,
            args["container"].as_str().map(str::to_owned),
            args["noContainer"] == true,
            args["address"].as_str().map(str::to_owned),
            &serde_json::from_value::<Option<_>>(args["callerPythonOverrides"].clone())?
                .unwrap_or_default(),
        )
        .await
    }

    fn host_states(&self, alias: &str) -> Vec<(String, super::transport::HostState)> {
        let mut states: Vec<_> = lock(&self.conns)
            .iter()
            .filter(|((host, _), _)| host == alias)
            .filter_map(|((_, root), cell)| {
                cell.get()
                    .map(|conn| (root.clone(), conn.state().borrow().clone()))
            })
            .collect();
        states.sort_by(|a, b| a.0.cmp(&b.0));
        states
    }

    async fn retry_host(&self, alias: &str) {
        let connections: Vec<_> = lock(&self.conns)
            .iter()
            .filter(|((host, _), _)| host == alias)
            .filter_map(|(_, cell)| cell.get().cloned())
            .collect();
        futures::future::join_all(connections.iter().map(|conn| conn.retry_key())).await;
    }

    fn host_target(&self, host: &str, slurm: bool) -> Result<SshTarget> {
        self.target(&Self::host_key(host, slurm)?)
    }

    fn follow_log(&self, run: &str) -> Result<Box<dyn Send>> {
        let row = self
            .store()?
            .get_run(run)?
            .ok_or_else(|| anyhow!("Run {run} not found"))?;
        let key = self.key(&row)?;
        Ok(match key {
            Key::Ssh { .. } | Key::Slurm { .. } => Box::new(self.ssh(&key)?.follow_logs(run)),
            _ => Box::new(()),
        })
    }
}

#[cfg(any(windows, test))]
async fn connect_supervised<F, Fut>(
    connected: Result<()>,
    prompt: Option<&Prompt>,
    mut request: F,
) -> Result<()>
where
    F: FnMut(Value) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    let login = async {
        if request(json!({"op": "status"})).await?["connected"] == true {
            return Ok(());
        }
        let prompt = prompt.ok_or_else(|| {
            anyhow!("SSH disconnected. Use Connect to authenticate; background login is disabled.")
        })?;
        let start = request(json!({"op": "connect_start"})).await?;
        let attempt = start["attempt"].as_str().ok_or_else(|| {
            anyhow!("SSH Connect returned no authentication attempt. Retry Connect.")
        })?;
        let result = async {
            loop {
                let status = request(json!({"op": "connect_status", "attempt": attempt})).await?;
                if status["done"] == true {
                    if let Some(error) = status["error"].as_str() {
                        bail!("{error}");
                    }
                    return Ok(());
                }
                if let Some(question) = status["prompt"].as_str() {
                    let answer = prompt(question.to_owned())
                        .await?
                        .ok_or_else(|| anyhow!("SSH authentication cancelled."))?;
                    request(json!({"op": "connect_answer", "attempt": attempt, "answer": answer}))
                        .await?;
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        }
        .await;
        let _ = request(json!({"op": "connect_cancel", "attempt": attempt})).await;
        result
    }
    .await;
    connected.and(login)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Master sockets need a short path; `/tmp` keeps them under the Unix socket limit.
#[cfg(unix)]
fn control_dir(data_dir: &Path) -> PathBuf {
    let uid = unsafe { libc::geteuid() };
    let digest = hex(&Sha256::digest(data_dir.as_os_str().as_encoded_bytes()));
    PathBuf::from("/tmp").join(format!("orx-cm-{uid}-{}", &digest[..8]))
}

#[cfg(all(test, unix))]
pub(crate) mod chaos;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::TestClock;

    #[tokio::test]
    async fn supervisor_connect_skips_a_live_service_and_relays_each_authentication_prompt() {
        use std::collections::VecDeque;
        let mut calls = Vec::new();
        connect_supervised(Ok(()), None, |request| {
            calls.push(request);
            std::future::ready(Ok(json!({"connected": true})))
        })
        .await
        .unwrap();
        assert_eq!(calls, [json!({"op": "status"})]);

        let prompt: Prompt =
            Arc::new(|question| Box::pin(async move { Ok(Some(format!("answer: {question}"))) }));
        let mut replies = VecDeque::from([
            json!({"connected": false}),
            json!({"attempt": "login"}),
            json!({"prompt": "Password"}),
            json!({}),
            json!({"prompt": "MFA code"}),
            json!({}),
            json!({"done": true}),
            json!({}),
        ]);
        calls.clear();
        connect_supervised(Ok(()), Some(&prompt), |request| {
            calls.push(request);
            std::future::ready(Ok(replies.pop_front().unwrap()))
        })
        .await
        .unwrap();
        assert!(replies.is_empty());
        assert_eq!(
            calls
                .iter()
                .map(|call| call["op"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "status",
                "connect_start",
                "connect_status",
                "connect_answer",
                "connect_status",
                "connect_answer",
                "connect_status",
                "connect_cancel",
            ]
        );
        assert_eq!(calls[3]["answer"], "answer: Password");
        assert_eq!(calls[5]["answer"], "answer: MFA code");
        assert!(calls[2..].iter().all(|call| call["attempt"] == "login"));
    }

    #[tokio::test]
    async fn supervisor_connect_attempts_login_after_the_engine_fails() {
        let prompt: Prompt = Arc::new(|_| Box::pin(async { Ok(Some("answer".into())) }));
        let mut replies = std::collections::VecDeque::from([
            json!({"connected": false}),
            json!({"attempt": "login"}),
            json!({"done": true}),
            json!({}),
        ]);
        let mut calls = Vec::new();
        let error =
            connect_supervised(Err(anyhow!("Unsupported host")), Some(&prompt), |request| {
                calls.push(request);
                std::future::ready(Ok(replies.pop_front().unwrap()))
            })
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Unsupported host");
        assert!(calls.iter().any(|call| call["op"] == "connect_start"));
        assert!(replies.is_empty());
    }

    #[tokio::test]
    async fn supervisor_connect_cancels_the_attempt_after_errors_or_cancelled_prompts() {
        let prompt: Prompt = Arc::new(|_| Box::pin(async { Ok(None) }));
        for status in [
            Ok(json!({"done": true, "error": "Login rejected"})),
            Ok(json!({"prompt": "Password"})),
            Err(anyhow!("Service unavailable")),
        ] {
            let mut replies = std::collections::VecDeque::from([
                Ok(json!({"connected": false})),
                Ok(json!({"attempt": "login"})),
                status,
                Ok(json!({})),
            ]);
            let mut calls = Vec::new();
            assert!(connect_supervised(Ok(()), Some(&prompt), |request| {
                calls.push(request);
                std::future::ready(replies.pop_front().unwrap())
            })
            .await
            .is_err());
            assert_eq!(
                calls.last().unwrap(),
                &json!({"op": "connect_cancel", "attempt": "login"})
            );
            assert!(replies.is_empty());
        }
    }

    use crate::engine::fake::{FakeBackend, HostFile};

    /// A host log that counts the bytes collection pages from it and, when `grows`, grows
    /// before answering each page.
    #[derive(Default)]
    struct Host {
        log: FakeBackend,
        paged: std::sync::atomic::AtomicU64,
        grows: std::sync::atomic::AtomicBool,
        destroyed: std::sync::atomic::AtomicBool,
        fail_finish: std::sync::atomic::AtomicBool,
        stall_after: Option<u64>,
    }

    #[async_trait]
    impl Backend for Host {
        async fn launch(
            &self,
            _: &super::super::RunIntent,
            _: &super::super::Fence<'_>,
        ) -> std::result::Result<Handle, super::super::LaunchError> {
            unreachable!()
        }
        async fn recover(&self, _: &super::super::RunIntent) -> super::super::Recovery {
            unreachable!()
        }
        async fn observe(&self, _: &[WatchedRun], _: Duration) -> Vec<(String, Observation)> {
            unreachable!()
        }
        async fn cancel(&self, _: &WatchedRun) -> super::super::CancelOutcome {
            unreachable!()
        }
        async fn finish(&self, _: &WatchedRun) -> Result<()> {
            use std::sync::atomic::Ordering;
            self.destroyed.store(true, Ordering::SeqCst);
            if self.fail_finish.load(Ordering::SeqCst) {
                bail!("provider destroyed the sandbox but its response was lost");
            }
            Ok(())
        }
        async fn read_log(
            &self,
            run: &WatchedRun,
            file: LogFile,
            range: ByteRange,
            ident: Option<&FileIdent>,
        ) -> Result<LogPage> {
            use std::sync::atomic::Ordering;
            if self.destroyed.load(Ordering::SeqCst) {
                bail!("sandbox was destroyed");
            }
            let probe = range.start == u64::MAX - 1;
            if !probe
                && self
                    .stall_after
                    .is_some_and(|limit| self.paged.load(Ordering::SeqCst) >= limit)
            {
                std::future::pending::<()>().await;
            }
            if !probe && self.grows.load(Ordering::SeqCst) {
                let mut log = self.log.log.lock().unwrap();
                log.bytes.extend_from_slice(b"more\n");
            }
            let page = self.log.read_log(run, file, range, ident).await?;
            if let LogPage::Data { bytes, .. } = &page {
                self.paged.fetch_add(bytes.len() as u64, Ordering::SeqCst);
            }
            Ok(page)
        }
    }

    fn collector(dir: &Path) -> Arc<Runtime> {
        let clock = Arc::new(TestClock::new());
        let engine = Arc::new(Engine::new(
            Store::open_at(dir.to_owned()).unwrap(),
            clock.clone(),
        ));
        Runtime::new(engine, clock, dir.to_owned(), None)
    }

    #[tokio::test]
    async fn round_two_retired_rental_observer_and_adapter_are_dropped() {
        let dir = std::env::temp_dir().join(format!("orx-retired-rental-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let metadata = crate::jobs::BackendDescriptor::parse(
            r#"{"kind":"openresearch_job","namespace":"org","jobId":"finished"}"#,
        )
        .unwrap();
        let run = WatchedRun {
            id: "finished".into(),
            handle: Handle::OpenResearch {
                descriptor: Box::new(metadata),
            },
        };
        let key = Key::of_handle(&run.handle).unwrap();
        let backend = Arc::new(FakeBackend::default());
        let weak = Arc::downgrade(&backend);
        lock(&runtime.backends).insert(key.clone(), backend);
        let (sender, receiver) = watch::channel(vec![run]);
        lock(&runtime.watched).insert(key.clone(), sender);
        let observer = tokio::spawn(runtime.clone().observe(key.clone(), receiver));
        tokio::time::timeout(Duration::from_secs(2), async {
            while weak.strong_count() < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        runtime.tick().unwrap();
        tokio::time::timeout(Duration::from_secs(2), observer)
            .await
            .expect("retired observer must exit")
            .unwrap();
        assert!(!lock(&runtime.watched).contains_key(&key));
        assert!(!lock(&runtime.backends).contains_key(&key));
        assert!(weak.upgrade().is_none());

        let backend = runtime.backend_for(&key).unwrap();
        let weak = Arc::downgrade(&backend);
        drop(backend);
        runtime.tick().unwrap();
        assert!(!lock(&runtime.backends).contains_key(&key));
        assert!(weak.upgrade().is_none());
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A box stays dialed while a run or its log collection needs it; then orx stops dialing it
    /// and forgets its connection state.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_box_no_run_needs_is_no_longer_dialed_or_listed() {
        let dir = std::env::temp_dir().join(format!("orx-retired-box-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let store = Store::open_at(dir.clone()).unwrap();
        let mut boxes = Vec::new();
        for (port, id, status) in [
            (1, "running", "running"),
            (2, "collecting", "done"),
            (3, "finished", "done"),
        ] {
            let descriptor = crate::jobs::BackendDescriptor::parse(
                &json!({
                    "kind":"openresearch_job", "namespace":"org", "jobId":id,
                    "sshHost":"127.0.0.1", "sshPort":port, "sshUser":"root"
                })
                .to_string(),
            )
            .unwrap();
            let target = descriptor.openresearch_ssh_target().unwrap();
            let handle = Handle::OpenResearch {
                descriptor: Box::new(descriptor),
            };
            lock(&runtime.backends)
                .entry(Key::of_handle(&handle).unwrap())
                .or_insert_with(|| Arc::new(FakeBackend::default()));
            store
                .upsert_run(&StoredRun {
                    id: id.into(),
                    experiment_id: "exp".into(),
                    project_id: "project".into(),
                    status: status.into(),
                    backend_json: json!({"kind":"engine","handle":handle}).to_string(),
                    command: "train".into(),
                    created_at: 1,
                    updated_at: 1,
                    ended_at: None,
                    exit_code: None,
                    commit_sha: None,
                    result_markdown: None,
                    cancel_requested: false,
                    cancel_reason: None,
                    chat_session_id: None,
                })
                .unwrap();
            let transport = runtime.box_transport(&target).unwrap();
            boxes.push((box_alias(&target).unwrap(), transport.ssh.conn.state()));
        }
        store.queue_log_collection("collecting").unwrap();
        let hosts = || {
            store
                .engine_hosts()
                .unwrap()
                .into_iter()
                .map(|(alias, ..)| alias)
                .collect::<std::collections::BTreeSet<_>>()
        };
        let stopped = |state: &watch::Receiver<crate::engine::transport::HostState>| {
            let mut state = state.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(10), async {
                    while state.changed().await.is_ok() {}
                })
                .await
                .is_ok()
            }
        };
        assert_eq!(hosts().len(), 3);

        runtime.tick().unwrap();
        assert!(
            stopped(&boxes[2].1).await,
            "a finished run's box is still dialed"
        );
        runtime.tick().unwrap();
        assert_eq!(hosts(), [boxes[0].0.clone(), boxes[1].0.clone()].into());
        assert!(
            boxes[1].1.has_changed().is_ok(),
            "a box with pending collection was dropped"
        );

        store.finish_log_collection("collecting").unwrap();
        runtime.tick().unwrap();
        assert!(
            stopped(&boxes[1].1).await,
            "a collected run's box is still dialed"
        );
        runtime.tick().unwrap();
        assert_eq!(hosts(), [boxes[0].0.clone()].into());
        assert!(
            boxes[0].1.has_changed().is_ok(),
            "a running run's box was dropped"
        );
        for (_, sender) in lock(&runtime.watched).drain() {
            sender.send_replace(Vec::new());
        }
        let events = runtime.engine.bus.subscribe();
        drop((runtime, store));
        engine_released(events).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn round_one_rentals_collect_independently_of_stalled_staging() {
        struct Boxes(FakeBackend);
        #[async_trait]
        impl Backend for Boxes {
            async fn launch(
                &self,
                _: &super::super::RunIntent,
                _: &super::super::Fence<'_>,
            ) -> std::result::Result<Handle, super::super::LaunchError> {
                unreachable!()
            }
            async fn recover(&self, _: &super::super::RunIntent) -> super::super::Recovery {
                unreachable!()
            }
            async fn cancel(&self, _: &WatchedRun) -> super::super::CancelOutcome {
                unreachable!()
            }
            async fn observe(
                &self,
                runs: &[WatchedRun],
                budget: Duration,
            ) -> Vec<(String, Observation)> {
                if runs.iter().any(|run| run.id == "staging") {
                    std::future::pending::<()>().await;
                }
                self.0.observe(runs, budget).await
            }
            async fn read_log(
                &self,
                run: &WatchedRun,
                file: LogFile,
                range: ByteRange,
                ident: Option<&FileIdent>,
            ) -> Result<LogPage> {
                self.0.read_log(run, file, range, ident).await
            }
        }
        let dir =
            std::env::temp_dir().join(format!("orx-independent-boxes-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let backend = Arc::new(Boxes(FakeBackend::default()));
        backend.0.log.lock().unwrap().bytes = b"result=42\n".to_vec();
        backend.0.feed.lock().unwrap().push_back((
            "finished".into(),
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        ));
        for id in ["staging", "finished"] {
            let metadata = crate::jobs::BackendDescriptor::parse(
                &json!({"kind":"openresearch_job","namespace":"org","jobId":id}).to_string(),
            )
            .unwrap();
            let handle = Handle::OpenResearch {
                descriptor: Box::new(metadata.clone()),
            };
            runtime
                .engine
                .insert(SubmittingRun {
                    id,
                    experiment_id: "exp",
                    project_id: "project",
                    metadata: &metadata,
                    command: "true",
                    created_at: 1,
                    commit_sha: None,
                    chat_session_id: None,
                    spec_digest: "digest",
                    launch: &LaunchSpec {
                        kind: "openresearch_job".into(),
                        target: Some("org".into()),
                        options: json!({}),
                    },
                })
                .unwrap();
            runtime
                .engine
                .store()
                .set_submission_fence(
                    id,
                    1,
                    &crate::store::SubmissionFence {
                        taken_at: 1,
                        submitter: None,
                    },
                )
                .unwrap();
            assert!(runtime
                .engine
                .store()
                .record_run_handle(id, 1, &handle)
                .unwrap());
            lock(&runtime.backends).insert(Key::of_handle(&handle).unwrap(), backend.clone());
        }
        runtime.tick().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while runtime.engine.load("finished").unwrap().0.status != "done" {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            runtime.collect_finished("finished").await.unwrap();
        })
        .await
        .unwrap();
        assert_eq!(runtime.engine.load("staging").unwrap().0.status, "starting");
        assert_eq!(
            std::fs::read(dir.join("run-logs/finished.log")).unwrap(),
            b"result=42\n"
        );
        assert!(!runtime
            .store()
            .unwrap()
            .log_collection_pending("finished")
            .unwrap());
        for (_, sender) in lock(&runtime.watched).drain() {
            sender.send_replace(Vec::new());
        }
        let weak = Arc::downgrade(&runtime);
        drop(runtime);
        tokio::time::timeout(Duration::from_secs(3), async {
            while weak.upgrade().is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("rental observers must release the runtime");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn legacy_wakeups_accepted_while_running_are_imported_once() {
        let dir = std::env::temp_dir().join(format!("orx-live-wakeups-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        runtime.tick().unwrap();
        let store = Store::open_at(dir.clone()).unwrap();
        let legacy = rusqlite::Connection::open(dir.join("orx.db")).unwrap();
        legacy
            .execute_batch(
                "INSERT INTO runs
                     (id, experiment_id, project_id, status, backend_json, created_at, updated_at)
                 VALUES ('legacy', 'exp', 'project', 'done', '{\"kind\":\"ssh_job\",\"jobId\":\"old\"}', 1, 2);",
            )
            .unwrap();
        store.register_run_wakeup("legacy", "chat").unwrap();
        runtime.tick().unwrap();
        let notices = store.run_notice_batch("chat").unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].run_id, "legacy");
        assert_eq!(notices[0].kind, crate::store::TERMINAL);
        runtime.tick().unwrap();
        assert_eq!(store.run_notice_batch("chat").unwrap(), notices);
        drop(legacy);
        drop(store);
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn ssh_retirement_failures_wait_five_minutes_before_retrying() {
        const CHILD: &str = "ORX_TEST_RETIREMENT_BACKOFF";
        if std::env::var_os(CHILD).is_none() {
            let server = orx_test_support::Sshd::start().unwrap();
            let output = crate::process::command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "engine::runtime::tests::ssh_retirement_failures_wait_five_minutes_before_retrying",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("HOME", server.home())
                .env("ORX_DATA_DIR", server.directory().join("store"))
                .env("ORX_NO_TELEMETRY", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stderr)
                    .matches("could not retire the 0.2.x SSH configuration")
                    .count(),
                2
            );
            return;
        }
        let home = crate::config::home_dir().unwrap();
        let ssh = home.join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let registry = ssh.join("openresearch_connections.json");
        std::fs::write(&registry, b"malformed").unwrap();
        let marker = ssh.join(".orx-config.lock");
        let runtime = collector(&crate::store::data_dir());
        let supervisor = tokio::spawn(runtime.clone().supervise());
        tokio::task::yield_now().await;
        assert!(marker.exists());
        std::fs::remove_file(&marker).unwrap();
        for _ in 0..3 {
            runtime.changed.notify_one();
            tokio::task::yield_now().await;
            assert!(!marker.exists());
        }
        for attempt in 0..2 {
            tokio::time::advance(Duration::from_secs(299)).await;
            runtime.changed.notify_one();
            tokio::task::yield_now().await;
            assert!(!marker.exists());
            if attempt == 1 {
                std::fs::write(&registry, b"{}").unwrap();
            }
            tokio::time::advance(Duration::from_secs(1)).await;
            runtime.changed.notify_one();
            tokio::task::yield_now().await;
            assert!(marker.exists());
            std::fs::remove_file(&marker).unwrap();
        }
        tokio::time::advance(Duration::from_secs(600)).await;
        runtime.changed.notify_one();
        tokio::task::yield_now().await;
        assert!(!marker.exists());
        supervisor.abort();
    }

    fn local_run() -> WatchedRun {
        WatchedRun {
            id: "run".into(),
            handle: Handle::Local {
                dir: "/fake".into(),
            },
        }
    }

    fn kept_ranges(dir: &Path) -> Vec<KeptRange> {
        serde_json::from_slice(&std::fs::read(dir.join("run-logs/run.ranges.json")).unwrap())
            .unwrap()
    }

    #[tokio::test]
    async fn collection_budget_yields_only_after_durable_progress() {
        for stall_after in [0, 1] {
            let dir = std::env::temp_dir()
                .join(format!("orx-collection-budget-{}", uuid::Uuid::new_v4()));
            let runtime = collector(&dir);
            let host = Host {
                stall_after: Some(stall_after),
                ..Host::default()
            };
            host.log.log.lock().unwrap().bytes =
                vec![b'x'; 2 * orx_host_proto::MAX_PAGE_LEN as usize];
            runtime
                .store()
                .unwrap()
                .queue_log_collection("run")
                .unwrap();
            let result = runtime
                .collect(&host, &local_run(), Duration::from_millis(100))
                .await;
            if stall_after > 0 {
                assert!(!result.unwrap());
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                runtime.store().unwrap().log_collection("run").unwrap(),
                Some((false, None))
            );
            assert!(!dir.join("run-logs/run.ranges.json").exists());
            if stall_after > 0 {
                let marker: (FileIdent, u64, u64) = serde_json::from_slice(
                    &std::fs::read(dir.join("run-logs/run.log.part.json")).unwrap(),
                )
                .unwrap();
                assert!(marker.2 > 0);
                let resumed = Host::default();
                resumed.log.log.lock().unwrap().bytes = host.log.log.lock().unwrap().bytes.clone();
                assert!(runtime
                    .collect(&resumed, &local_run(), Duration::from_secs(1))
                    .await
                    .unwrap());
                assert_eq!(
                    resumed.paged.load(std::sync::atomic::Ordering::SeqCst),
                    orx_host_proto::MAX_PAGE_LEN
                );
                assert!(runtime
                    .store()
                    .unwrap()
                    .log_collection_pending("run")
                    .unwrap());
                assert_eq!(
                    std::fs::metadata(dir.join("run-logs/run.log"))
                        .unwrap()
                        .len(),
                    2 * orx_host_proto::MAX_PAGE_LEN
                );
            }
            drop(runtime);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn runtime_reuses_its_store_without_reopening_migrations() {
        let dir =
            std::env::temp_dir().join(format!("orx-collection-budget-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        drop(runtime.store().unwrap());
        let db = rusqlite::Connection::open(dir.join("orx.db")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_ui_insert BEFORE INSERT ON ui_state
            BEGIN SELECT RAISE(FAIL, 'store reopened'); END;",
        )
        .unwrap();
        assert!(Store::open_at(dir.clone()).is_err());
        assert!(runtime
            .store()
            .unwrap()
            .list_engine_runs()
            .unwrap()
            .is_empty());
        drop(db);
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A partial range resumes from the length its marker records as synced, so only the
    /// rest is paged from the host again.
    #[tokio::test]
    async fn a_partial_range_resumes_from_its_synced_length() {
        use std::sync::atomic::Ordering;
        let dir = std::env::temp_dir().join(format!("orx-collect-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let host = Host::default();
        let log: Vec<u8> = (0..3 * 4096).map(|i| (i % 251) as u8).collect();
        host.log.log.lock().unwrap().bytes = log.clone();
        let run = local_run();
        let info = Runtime::log_info(&host, &run).await.unwrap().unwrap();
        let logs = dir.join("run-logs");
        std::fs::create_dir_all(&logs).unwrap();
        // Bytes past the synced length may not have reached the disk.
        let mut partial = log[..9000].to_vec();
        partial[8500] ^= 1;
        std::fs::write(logs.join("run.log.part"), &partial).unwrap();
        std::fs::write(
            logs.join("run.log.part.json"),
            serde_json::to_vec(&(&info.ident, 0u64, 8192u64)).unwrap(),
        )
        .unwrap();
        Runtime::collect_log(&dir, &host, &run, &mut false)
            .await
            .unwrap();
        assert_eq!(std::fs::read(logs.join("run.log")).unwrap(), log);
        assert_eq!(host.paged.load(Ordering::SeqCst), 4096);
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A log is kept to the length it had when collection started; growth since then leaves
    /// it short of the end, so readers ask the host for the rest.
    #[tokio::test]
    async fn a_log_growing_during_collection_is_kept_to_its_starting_length() {
        use std::sync::atomic::Ordering;
        let dir = std::env::temp_dir().join(format!("orx-collect-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let host = Host::default();
        host.log.log.lock().unwrap().bytes = b"start\n".to_vec();
        host.grows.store(true, Ordering::SeqCst);
        Runtime::collect_log(&dir, &host, &local_run(), &mut false)
            .await
            .unwrap();
        let kept = std::fs::read(dir.join("run-logs/run.log")).unwrap();
        assert_eq!(kept, b"start\n");
        let range = &kept_ranges(&dir)[0];
        assert_eq!((range.len, range.eof), (6, false));

        host.grows.store(false, Ordering::SeqCst);
        Runtime::collect_log(&dir, &host, &local_run(), &mut false)
            .await
            .unwrap();
        let range = &kept_ranges(&dir)[0];
        assert_eq!((range.len, range.eof), (11, true));
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Once a log is kept, what the adapter held for reading it goes; runs deleted by any path
    /// are forgotten on the next tick.
    #[tokio::test]
    async fn kept_and_deleted_runs_release_their_logs() {
        let dir = std::env::temp_dir().join(format!("orx-collect-{}", uuid::Uuid::new_v4()));
        let clock = Arc::new(TestClock::new());
        let store = Store::open_at(dir.clone()).unwrap();
        let run = StoredRun {
            id: "kept".into(),
            experiment_id: "exp".into(),
            project_id: "project".into(),
            status: "done".into(),
            backend_json:
                r#"{"kind":"engine","jobId":"/fake","handle":{"type":"Local","dir":"/fake"}}"#
                    .into(),
            command: "train".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: Some(1),
            exit_code: Some(0),
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        };
        store.upsert_run(&run).unwrap();
        let engine = Arc::new(Engine::new(store, clock.clone()));
        let runtime = Runtime::new(engine, clock, dir.clone(), None);
        let backend = Arc::new(FakeBackend::default());
        backend.log.lock().unwrap().bytes = b"output\n".to_vec();
        *backend.held.lock().unwrap() = vec!["kept".into(), "deleted".into()];
        lock(&runtime.backends).insert(Key::Local, backend.clone());

        runtime
            .store()
            .unwrap()
            .queue_log_collection("kept")
            .unwrap();
        runtime
            .store()
            .unwrap()
            .set_run_contact(
                "kept",
                &crate::store::Contact::Unreconciled {
                    since: 1,
                    detail: "kill pending".into(),
                },
            )
            .unwrap();
        for _ in 0..10 {
            runtime.collect_finished("kept").await.unwrap();
        }
        assert_eq!(
            runtime.store().unwrap().log_collection("kept").unwrap(),
            Some((false, None))
        );
        let failures: i64 = runtime
            .store()
            .unwrap()
            .begin()
            .unwrap()
            .query_row(
                "SELECT attempts FROM log_collections WHERE run_id = 'kept'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(failures, 0);
        assert!(backend.unwatched.lock().unwrap().is_empty());
        runtime
            .store()
            .unwrap()
            .set_run_contact("kept", &crate::store::Contact::Live)
            .unwrap();
        runtime.collect_finished("kept").await.unwrap();
        assert_eq!(*backend.unwatched.lock().unwrap(), ["kept"]);
        runtime.tick().unwrap();
        assert_eq!(*backend.held.lock().unwrap(), ["kept"]);
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn finalization_retries_after_restart_without_reopening_destroyed_logs() {
        use std::sync::atomic::Ordering;
        let dir = std::env::temp_dir().join(format!("orx-finalize-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let metadata = crate::jobs::BackendDescriptor::parse(r#"{"kind":"local_job"}"#).unwrap();
        runtime
            .engine
            .insert(SubmittingRun {
                id: "run",
                experiment_id: "exp",
                project_id: "project",
                metadata: &metadata,
                command: "true",
                created_at: 1,
                commit_sha: None,
                chat_session_id: None,
                spec_digest: "digest",
                launch: &LaunchSpec {
                    kind: "local_job".into(),
                    target: None,
                    options: json!({}),
                },
            })
            .unwrap();
        {
            let store = runtime.store().unwrap();
            store
                .set_submission_fence(
                    "run",
                    1,
                    &crate::store::SubmissionFence {
                        taken_at: 1,
                        submitter: None,
                    },
                )
                .unwrap();
            store
                .record_run_handle("run", 1, &local_run().handle)
                .unwrap();
        }
        runtime
            .engine
            .observe(
                "run",
                Observation::Exited {
                    finished_at: None,
                    code: Some(0),
                    signal: None,
                },
            )
            .unwrap();
        let backend = Arc::new(Host::default());
        backend.log.log.lock().unwrap().bytes = b"final output\n".to_vec();
        backend.fail_finish.store(true, Ordering::SeqCst);
        lock(&runtime.backends).insert(Key::Local, backend.clone());
        assert!(runtime.collect_finished("run").await.is_err());
        assert!(runtime
            .store()
            .unwrap()
            .log_output_collected("run")
            .unwrap());
        assert!(runtime
            .store()
            .unwrap()
            .log_collection_pending("run")
            .unwrap());
        assert!(backend.destroyed.load(Ordering::SeqCst));
        drop(runtime);

        let runtime = collector(&dir);
        let backend = Arc::new(Host::default());
        backend.destroyed.store(true, Ordering::SeqCst);
        lock(&runtime.backends).insert(Key::Local, backend);
        runtime.collect_finished("run").await.unwrap();
        assert!(!runtime
            .store()
            .unwrap()
            .log_collection_pending("run")
            .unwrap());
        assert_eq!(
            std::fs::read(dir.join("run-logs/run.log")).unwrap(),
            b"final output\n"
        );
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A partial range resumes only for the log generation and offset it was started for.
    #[tokio::test]
    async fn partial_collection_restarts_for_a_new_generation_or_offset() {
        let dir = std::env::temp_dir().join(format!("orx-collect-{}", uuid::Uuid::new_v4()));
        let clock = Arc::new(TestClock::new());
        let engine = Arc::new(Engine::new(
            Store::open_at(dir.clone()).unwrap(),
            clock.clone(),
        ));
        let runtime = Runtime::new(engine, clock, dir.clone(), None);
        let backend = FakeBackend::default();
        let run = WatchedRun {
            id: "run".into(),
            handle: Handle::Local {
                dir: "/fake".into(),
            },
        };
        let probe = ByteRange {
            start: u64::MAX - 1,
            end: u64::MAX,
        };
        let logs = dir.join("run-logs");
        std::fs::create_dir_all(&logs).unwrap();

        backend.log.lock().unwrap().bytes = b"OLD:tail\n".to_vec();
        let LogPage::Reset { info: Some(old) } = backend
            .read_log(&run, LogFile::Log, probe.clone(), None)
            .await
            .unwrap()
        else {
            panic!("no length probe")
        };
        std::fs::write(logs.join("run.log.part"), "OLD:").unwrap();
        std::fs::write(
            logs.join("run.log.part.json"),
            serde_json::to_vec(&(&old.ident, 0u64, 4u64)).unwrap(),
        )
        .unwrap();
        *backend.log.lock().unwrap() = HostFile {
            bytes: b"NEW:tail\n".to_vec(),
            ino: 1,
        };
        Runtime::collect_log(&dir, &backend, &run, &mut false)
            .await
            .unwrap();
        assert_eq!(std::fs::read(logs.join("run.log")).unwrap(), b"NEW:tail\n");
        assert!(!logs.join("run.log.part.json").exists());

        // The same generation recorded for another offset starts over too.
        let LogPage::Reset { info: Some(new) } = backend
            .read_log(&run, LogFile::Log, probe.clone(), None)
            .await
            .unwrap()
        else {
            panic!("no length probe")
        };
        std::fs::write(logs.join("run.log.part"), "tail\n").unwrap();
        std::fs::write(
            logs.join("run.log.part.json"),
            serde_json::to_vec(&(&new.ident, 4u64, 5u64)).unwrap(),
        )
        .unwrap();
        Runtime::collect_log(&dir, &backend, &run, &mut false)
            .await
            .unwrap();
        assert_eq!(std::fs::read(logs.join("run.log")).unwrap(), b"NEW:tail\n");
        drop(runtime);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn early_collection_and_failed_final_notify_do_not_finish_output_debt() {
        let dir = std::env::temp_dir().join(format!("orx-final-notify-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let local = dir.join("local-runs/run");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("log"), "finished\n").unwrap();
        std::fs::create_dir(local.join("notify")).unwrap();
        let metadata = crate::jobs::BackendDescriptor::parse(r#"{"kind":"local_job"}"#).unwrap();
        runtime
            .engine
            .insert(SubmittingRun {
                id: "run",
                experiment_id: "exp",
                project_id: "project",
                metadata: &metadata,
                command: "true",
                created_at: 1,
                commit_sha: None,
                chat_session_id: Some("chat"),
                spec_digest: "spec",
                launch: &LaunchSpec {
                    kind: "local_job".into(),
                    target: None,
                    options: json!({}),
                },
            })
            .unwrap();
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .set_submission_fence(
                "run",
                1,
                &crate::store::SubmissionFence {
                    taken_at: 1,
                    submitter: None,
                },
            )
            .unwrap();
        let handle = Handle::Local {
            dir: local.to_string_lossy().into_owned(),
        };
        store.record_run_handle("run", 1, &handle).unwrap();
        let watched = WatchedRun {
            id: "run".into(),
            handle,
        };
        let backend = Local::new(dir.clone());
        runtime
            .collect(&backend, &watched, COLLECTION_BUDGET)
            .await
            .unwrap();
        assert!(store.log_collection("run").unwrap().is_none());
        runtime
            .engine
            .observe(
                "run",
                Observation::Exited {
                    finished_at: None,
                    code: Some(0),
                    signal: None,
                },
            )
            .unwrap();
        assert!(store.log_collection_pending("run").unwrap());
        assert!(runtime.collect_finished("run").await.is_err());
        assert!(store.log_collection_pending("run").unwrap());
        drop(runtime);
        let runtime = collector(&dir);
        assert_eq!(store.pending_log_collections().unwrap(), ["run"]);
        std::fs::remove_dir(local.join("notify")).unwrap();
        std::fs::write(local.join("notify"), "final evaluation\n").unwrap();
        runtime.collect_finished("run").await.unwrap();
        assert!(!store.log_collection_pending("run").unwrap());
        let notices = store.run_notice_batch("chat").unwrap();
        assert!(notices
            .iter()
            .any(|notice| notice.kind == crate::store::NOTIFY
                && notice.body.ends_with("final evaluation\n")));
        drop(runtime);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_provider_without_notify_input_completes_that_obligation() {
        let dir = std::env::temp_dir().join(format!("orx-no-notify-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&StoredRun {
                id: "run".into(),
                experiment_id: "exp".into(),
                project_id: "project".into(),
                status: "done".into(),
                backend_json: r#"{"kind":"engine","handle":{"type":"Api","provider":"hf_job","account":{"target":"test","context":null},"ids":["job"]}}"#
                    .into(),
                command: "true".into(),
                created_at: 1,
                updated_at: 1,
                ended_at: Some(1),
                exit_code: Some(0),
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: None,
            })
            .unwrap();
        let backend = runtime
            .backend_for(&Key::Api {
                provider: "hf_job".into(),
                account: ApiAccount {
                    target: Some("test".into()),
                    context: None,
                },
            })
            .unwrap();
        assert!(runtime
            .engine
            .deliver_notify(backend.as_ref(), "run")
            .await
            .unwrap());
        drop(runtime);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A run no adapter can be built for is surfaced as unreconciled, with one notice, rather
    /// than skipped every tick.
    #[tokio::test]
    async fn a_run_without_an_adapter_is_unreconciled_and_told_once() {
        let dir = std::env::temp_dir().join(format!("orx-adapter-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        runtime
            .engine
            .insert(crate::store::SubmittingRun {
                id: "run",
                experiment_id: "exp",
                project_id: "project",
                metadata: &crate::jobs::BackendDescriptor::parse(r#"{"kind":"local"}"#).unwrap(),
                command: "train",
                created_at: 1,
                commit_sha: None,
                chat_session_id: None,
                spec_digest: "digest",
                launch: &LaunchSpec {
                    kind: "retired_job".into(),
                    target: None,
                    options: json!({}),
                },
            })
            .unwrap();
        let store = Store::open_at(dir.clone()).unwrap();
        store.subscribe_run("run", "chat", 1).unwrap();
        for _ in 0..2 {
            runtime.tick().unwrap();
        }
        let state = store.engine_run_state("run").unwrap().unwrap();
        assert!(
            matches!(&state.contact, crate::store::Contact::Unreconciled { detail, .. } if detail.contains("retired_job")),
            "{:?}",
            state.contact
        );
        let notices = store.run_notice_batch("chat").unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, crate::store::ATTENTION);
        // Nothing launched, so the cancel needs no adapter.
        let run = store.get_run("run").unwrap().unwrap();
        runtime.cancel(&run, "user").await.unwrap();
        assert_eq!(store.get_run("run").unwrap().unwrap().status, "cancelled");
        drop(runtime);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn collection_failures_tell_the_chat_once_and_reconnect_retries_owed_logs() {
        let dir = std::env::temp_dir().join(format!("orx-collect-{}", uuid::Uuid::new_v4()));
        let clock = Arc::new(TestClock::new());
        let store = Store::open_at(dir.clone()).unwrap();
        let descriptor = r#"{"kind":"vanished_job","jobId":"j"}"#;
        store
            .upsert_run(&StoredRun {
                id: "run".into(),
                experiment_id: "exp".into(),
                project_id: "project".into(),
                status: "done".into(),
                backend_json: descriptor.into(),
                command: "train".into(),
                created_at: 1,
                updated_at: 1,
                ended_at: Some(1),
                exit_code: Some(0),
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: Some("chat".into()),
            })
            .unwrap();
        store.subscribe_run("run", "chat", 1).unwrap();
        store.queue_log_collection("run").unwrap();
        let engine = Arc::new(Engine::new(store, clock.clone()));
        let runtime = Runtime::new(engine, clock, dir.clone(), None);
        let attention = || {
            runtime
                .store()
                .unwrap()
                .run_notice_batch("chat")
                .unwrap()
                .into_iter()
                .filter(|notice| notice.kind == crate::store::ATTENTION)
                .count()
        };
        let mut events = runtime.engine.bus.subscribe();
        for attempt in 1..=10 {
            assert!(runtime.collect_finished("run").await.is_err());
            let mut updates = 0;
            while let Ok(event) = events.try_recv() {
                if matches!(event, super::super::bus::RunEvent::LogCollection { run_id } if run_id == "run")
                {
                    updates += 1;
                }
            }
            assert_eq!(updates, 1);
            let (failed, error) = runtime
                .store()
                .unwrap()
                .log_collection("run")
                .unwrap()
                .unwrap();
            assert_eq!(failed, attempt >= 7, "attempt {attempt}");
            assert!(error.unwrap().contains("vanished_job"));
            assert_eq!(attention(), usize::from(attempt >= 7), "attempt {attempt}");
        }

        // Reconnecting a host retries its runs' collections at once.
        let store = Store::open_at(dir.clone()).unwrap();
        let mut on_host = store.get_run("run").unwrap().unwrap();
        on_host.id = "on-host".into();
        on_host.backend_json =
            r#"{"kind":"engine","jobId":"x","handle":{"type":"Ssh","host":"lab","root":"/r"}}"#
                .into();
        store.upsert_run(&on_host).unwrap();
        store.queue_log_collection("on-host").unwrap();
        for _ in 0..3 {
            store
                .defer_log_collection("on-host", "unreachable")
                .unwrap();
        }
        runtime.host_returned("other", "/r").unwrap();
        runtime.host_returned("lab", "/elsewhere").unwrap();
        assert!(store.due_log_collections(10).unwrap().is_empty());
        runtime.host_returned("lab", "/r").unwrap();
        assert_eq!(store.due_log_collections(10).unwrap(), ["on-host"]);
        for _ in 3..7 {
            store
                .defer_log_collection("on-host", "unreachable")
                .unwrap();
        }
        assert!(store.due_log_collections(10).unwrap().is_empty());
        runtime.host_returned("other", "/r").unwrap();
        runtime.host_returned("lab", "/elsewhere").unwrap();
        assert!(store.due_log_collections(10).unwrap().is_empty());
        runtime.host_returned("lab", "/r").unwrap();
        assert_eq!(store.due_log_collections(10).unwrap(), ["on-host"]);
        drop(runtime);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn rental_live_at_eight_minutes_wakes_collection_after_seven_minute_failure() {
        let dir =
            std::env::temp_dir().join(format!("orx-rental-collection-{}", uuid::Uuid::new_v4()));
        let runtime = collector(&dir);
        let store = Store::open_at(dir.clone()).unwrap();
        let descriptor = crate::jobs::BackendDescriptor::parse(
            r#"{"kind":"openresearch_job","namespace":"org","jobId":"rental","sshHost":"box","sshPort":22,"sshUser":"root"}"#,
        )
        .unwrap();
        let alias = box_alias(&descriptor.openresearch_ssh_target().unwrap()).unwrap();
        let handle = Handle::OpenResearch {
            descriptor: Box::new(descriptor),
        };
        store
            .upsert_run(&StoredRun {
                id: "run".into(),
                experiment_id: "exp".into(),
                project_id: "project".into(),
                status: "done".into(),
                backend_json: json!({"kind":"engine","handle":handle}).to_string(),
                command: "train".into(),
                created_at: 1,
                updated_at: 1,
                ended_at: Some(1),
                exit_code: Some(0),
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: None,
            })
            .unwrap();
        store.queue_log_collection("run").unwrap();
        for _ in 0..3 {
            store.defer_log_collection("run", "unreachable").unwrap();
        }
        let key = Key::of_handle(&handle).unwrap();
        let stalled = Arc::new(Host {
            stall_after: Some(0),
            ..Host::default()
        });
        let output = b"final result=42\n";
        stalled.log.log.lock().unwrap().bytes = output.to_vec();
        lock(&runtime.backends).insert(key.clone(), stalled);
        let start = tokio::time::Instant::now();
        tokio::time::advance(Duration::from_secs(5 * 60)).await;
        let mut events = runtime.engine.bus.subscribe();
        let task = tokio::spawn(runtime.clone().collect_pending());
        tokio::time::timeout(Duration::from_secs(3 * 60), async {
            while !matches!(events.recv().await.unwrap(), super::super::bus::RunEvent::LogCollection { run_id } if run_id == "run") {}
        })
        .await
        .unwrap();
        assert_eq!(start.elapsed(), Duration::from_secs(7 * 60));
        assert!(store
            .log_collection("run")
            .unwrap()
            .unwrap()
            .1
            .unwrap()
            .contains("timed out"));
        assert!(store.due_log_collections(10).unwrap().is_empty());

        tokio::time::advance(Duration::from_secs(60)).await;
        let recovered = Arc::new(Host::default());
        recovered.log.log.lock().unwrap().bytes = output.to_vec();
        lock(&runtime.backends).insert(key, recovered);
        runtime.host_returned("other", DEFAULT_ROOT).unwrap();
        runtime.host_returned(&alias, "/wrong-root").unwrap();
        assert!(store.due_log_collections(10).unwrap().is_empty());
        runtime.host_returned(&alias, DEFAULT_ROOT).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !matches!(events.recv().await.unwrap(), super::super::bus::RunEvent::LogCollection { run_id } if run_id == "run") {}
        })
        .await
        .expect("Live must wake collection before its backoff expires");
        assert!(!store.log_collection_pending("run").unwrap());
        assert_eq!(std::fs::read(dir.join("run-logs/run.log")).unwrap(), output);
        assert!(start.elapsed() < Duration::from_secs(9 * 60));
        task.abort();
        let _ = task.await;
        drop(runtime);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rented_boxes_keep_separate_transports_for_ports_and_rental_generations() {
        let directory =
            std::env::temp_dir().join(format!("orx-rental-transports-{}", uuid::Uuid::new_v4()));
        let clock = Arc::new(TestClock::new());
        let engine = Arc::new(Engine::new(
            Store::open_at(directory.clone()).unwrap(),
            clock.clone(),
        ));
        let runtime = Runtime::new(
            engine,
            clock,
            directory.clone(),
            Some(directory.join("missing-config")),
        );
        let mut first = SshTarget::host_port(
            "root@shared".into(),
            2200,
            crate::jobs::ssh::HostKeyPolicy::Ephemeral,
        );
        first.managed_generation = Some("first-rental".into());
        let transport = runtime.box_transport(&first).unwrap();
        assert!(Arc::ptr_eq(
            &transport,
            &runtime.box_transport(&first).unwrap()
        ));

        let mut other_port = SshTarget::host_port(
            "root@shared".into(),
            2201,
            crate::jobs::ssh::HostKeyPolicy::Ephemeral,
        );
        other_port.managed_generation = first.managed_generation.clone();
        assert!(!Arc::ptr_eq(
            &transport,
            &runtime.box_transport(&other_port).unwrap()
        ));

        let mut next_rental = first;
        next_rental.managed_generation = Some("next-rental".into());
        assert!(!Arc::ptr_eq(
            &transport,
            &runtime.box_transport(&next_rental).unwrap()
        ));
        assert_eq!(lock(&runtime.auth).len(), 3);
        let events = runtime.engine.bus.subscribe();
        drop(transport);
        drop(runtime);
        engine_released(events).await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    /// Windows cannot remove the data directory while a host watcher still holds the store.
    async fn engine_released(
        mut events: tokio::sync::broadcast::Receiver<crate::engine::bus::RunEvent>,
    ) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while !matches!(
                events.recv().await,
                Err(tokio::sync::broadcast::error::RecvError::Closed)
            ) {}
        })
        .await
        .expect("host watchers retained the engine after its runtime stopped");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_host_initialization_shares_one_cell_per_alias_and_root() {
        let directory =
            std::env::temp_dir().join(format!("orx-host-cells-{}", uuid::Uuid::new_v4()));
        let clock = Arc::new(super::super::clock::SystemClock::new());
        let engine = Arc::new(Engine::new(
            Store::open_at(directory.clone()).unwrap(),
            clock.clone(),
        ));
        let runtime = Runtime::new(
            engine,
            clock,
            directory.clone(),
            Some(directory.join("missing-config")),
        );
        let barrier = Arc::new(std::sync::Barrier::new(8));
        let handle = tokio::runtime::Handle::current();
        let connections = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let (runtime, barrier, handle) =
                        (runtime.clone(), barrier.clone(), handle.clone());
                    scope.spawn(move || {
                        let _entered = handle.enter();
                        barrier.wait();
                        runtime
                            .conn("host", "/first", runtime.ssh_config.clone())
                            .unwrap()
                    })
                })
                .collect();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<Vec<_>>()
        });
        for conn in &connections {
            assert!(connections[0].state().same_channel(&conn.state()));
        }
        let other = runtime
            .conn("host", "/second", runtime.ssh_config.clone())
            .unwrap();
        assert!(!other.state().same_channel(&connections[0].state()));
        assert_eq!(runtime.store().unwrap().engine_hosts().unwrap().len(), 2);
        assert_eq!(runtime.host_states("host").len(), 2);
        assert!(runtime.host_states("unused").is_empty());
        runtime.retry_host("unused").await;
        assert_eq!(lock(&runtime.conns).len(), 2);
        assert_eq!(lock(&runtime.auth).len(), 1);
        let events = runtime.engine.bus.subscribe();
        drop(other);
        drop(connections);
        drop(runtime);
        engine_released(events).await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn owed_collections_without_progress_wait_instead_of_spinning() {
        let dir = std::env::temp_dir().join(format!("orx-owed-stalled-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for n in 0..10 {
            store.queue_log_collection(&format!("missing-{n}")).unwrap();
        }
        let clock = Arc::new(super::super::clock::TestClock::new());
        let runtime = Runtime::new(
            Arc::new(Engine::new(
                Store::open_at(dir.clone()).unwrap(),
                clock.clone(),
            )),
            clock,
            dir.clone(),
            None,
        );
        let task = tokio::spawn(runtime.collect_pending());
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(store.due_log_collections(10).unwrap().len(), 10);
        task.abort();
        let _ = task.await;
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn owed_collections_drain_full_batches_without_sleeping() {
        let dir = std::env::temp_dir().join(format!("orx-owed-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for n in 0..37 {
            let id = format!("owed-{n}");
            store
                .upsert_run(&StoredRun {
                    id: id.clone(),
                    experiment_id: "exp".into(),
                    project_id: "p".into(),
                    status: "done".into(),
                    backend_json: r#"{"kind":"engine"}"#.into(),
                    command: "true".into(),
                    created_at: 1,
                    updated_at: 1,
                    ended_at: Some(1),
                    exit_code: Some(0),
                    commit_sha: None,
                    result_markdown: None,
                    cancel_requested: false,
                    cancel_reason: None,
                    chat_session_id: None,
                })
                .unwrap();
            store.queue_log_collection(&id).unwrap();
        }
        let clock = Arc::new(super::super::clock::TestClock::new());
        let runtime = Runtime::new(
            Arc::new(Engine::new(
                Store::open_at(dir.clone()).unwrap(),
                clock.clone(),
            )),
            clock,
            dir.clone(),
            None,
        );
        let task = tokio::spawn(runtime.clone().collect_pending());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if store.due_log_collections(40).unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        for n in 0..37 {
            assert!(store
                .log_collection(&format!("owed-{n}"))
                .unwrap()
                .is_none());
        }
        task.abort();
        let _ = task.await;
        drop(runtime);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
