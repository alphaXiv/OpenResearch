pub mod askpass;
mod ssh;

#[cfg(unix)]
pub(crate) use ssh::local_binary;
pub(crate) use ssh::read_stderr;

use anyhow::{anyhow, bail, Result};
use orx_host_proto::{Message, Request, Response};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::engine::clock::{Clock, JumpDetector};
use tokio::sync::{mpsc, oneshot, watch};

use crate::store::Store;
pub use askpass::Prompt;

fn deadline() -> Duration {
    crate::config::paced(Duration::from_secs(35))
}

/// How long orx-host may work on one request before answering: sbatch then squeue take up to
/// 40 s, and a request on a run may first wait for another doing that. Probes keep checking the
/// session meanwhile, so a dead host is still noticed by `deadline`.
const HOST_WORK: Duration = Duration::from_secs(80);

/// Blob requests share one host lock, which checking another blob holds while it hashes: up to
/// 5 GiB at `hash_time`'s rate.
const BLOB_WORK: Duration = Duration::from_secs(320);

/// Hashing a staged blob on a slow disk, at a pessimistic 16 MiB/s.
fn hash_time(len: u64) -> Duration {
    Duration::from_secs(len / (16 << 20))
}

fn reply_deadline(body: &Request) -> Duration {
    deadline()
        + match body {
            Request::Exec { timeout_ms, .. } => Duration::from_millis(u64::from(*timeout_ms)),
            Request::Changes { wait_ms, .. } => {
                Duration::from_millis(u64::from(*wait_ms)) + HOST_WORK
            }
            Request::HasBlob { len, .. } => BLOB_WORK + hash_time(*len),
            Request::PutBlob { .. } => BLOB_WORK,
            Request::Launch(_)
            | Request::SlurmSubmit { .. }
            | Request::SlurmQueue { .. }
            | Request::PinSource { .. }
            | Request::Cancel { .. }
            | Request::CancelLegacy { .. }
            | Request::Cleanup { .. } => HOST_WORK,
            Request::Hello { .. }
            | Request::Inspect { .. }
            | Request::ReadLog { .. }
            | Request::SlurmCancelLegacy { .. }
            | Request::InspectLegacy { .. } => Duration::ZERO,
        }
}
const CHUNK: usize = 256 * 1024;

/// sshd and the org key sync can lag a freshly online box, so its key refusals back off this
/// long before they open the breaker.
const KEY_SYNC_GRACE_MS: i64 = 2 * 60 * 1000;

/// An OpenResearch box: a key-only host orx rents, with no Connect to close a breaker.
pub(crate) fn managed(alias: &str) -> bool {
    alias.starts_with("orx-box-")
}

/// The host refused every key ssh offered.
pub(crate) fn key_refused(cause: &str) -> bool {
    cause
        .to_ascii_lowercase()
        .starts_with("permission denied (publickey")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostClass {
    Key,
    Interactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Idle,
    Connecting,
    Live,
    Backoff,
    NeedsAuth,
}

impl Status {
    /// Ranks how much a root needs attention when one state stands for an alias.
    pub fn severity(&self) -> u8 {
        match self {
            Status::Idle => 0,
            Status::Live => 1,
            Status::Connecting => 2,
            Status::Backoff => 3,
            Status::NeedsAuth => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostState {
    pub status: Status,
    pub failures: usize,
    pub next_retry_ms: Option<i64>,
    pub since_ms: i64,
    pub last_live_ms: Option<i64>,
    pub cause: Option<String>,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            status: Status::Idle,
            failures: 0,
            next_retry_ms: None,
            since_ms: 0,
            last_live_ms: None,
            cause: None,
        }
    }
}

#[derive(Debug)]
pub struct Unavailable {
    pub state: HostState,
    /// A write was attempted, so the request may have reached the host.
    pub sent: bool,
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.state.cause.as_deref().unwrap_or("Connection closed"))
    }
}

impl std::error::Error for Unavailable {}

/// Context on an `Unavailable` whose last connection failed in a way no retry fixes: a platform
/// orx-host doesn't support, or a bootstrap the host refuses. A launch that was never sent
/// fails with it instead of waiting.
#[derive(Debug)]
pub struct Unsupported(pub String);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone)]
pub struct Config {
    pub alias: String,
    pub root: String,
    pub ssh_config: Option<PathBuf>,
    #[cfg(unix)]
    pub control_dir: PathBuf,
    #[cfg(unix)]
    pub sharing_home: Option<PathBuf>,
    pub binary_dir: Option<PathBuf>,
    pub askpass_executable: PathBuf,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AuthState {
    pub blocked: Option<(i64, String)>,
    pub class: Option<HostClass>,
    pub route: Option<String>,
}

pub(crate) struct Auth {
    gate: tokio::sync::Mutex<()>,
    state: watch::Sender<AuthState>,
}

impl Auth {
    pub(crate) fn new(store: &Store, alias: &str) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            gate: tokio::sync::Mutex::new(()),
            state: watch::channel(store.engine_host_auth(alias)?).0,
        }))
    }

    /// Failed writes still block peers, but cannot clear their breaker.
    fn update(
        &self,
        store: &Store,
        alias: &str,
        update: impl FnOnce(&mut AuthState),
    ) -> Result<()> {
        let mut result = Ok(());
        self.state.send_if_modified(|current| {
            let mut next = current.clone();
            update(&mut next);
            result = store.save_engine_host_auth(alias, &next);
            if result.is_err() && current.blocked.is_some() && next.blocked.is_none() {
                return false;
            }
            let changed = next != *current;
            *current = next;
            changed
        });
        result
    }

    fn block(&self, store: &Store, alias: &str, since: i64, cause: String) -> Result<()> {
        self.update(store, alias, |state| {
            state.blocked.get_or_insert((since, cause));
        })
    }
}

#[derive(Clone)]
pub struct HostConn {
    calls: mpsc::Sender<Call>,
    actions: mpsc::Sender<Action>,
    state: watch::Receiver<HostState>,
}

struct Action {
    key_only: bool,
    keep_master: bool,
    prompt: Option<Prompt>,
    reply: oneshot::Sender<Result<()>>,
}

struct Call {
    body: Request,
    reply: oneshot::Sender<Result<Response>>,
}
struct Pending {
    reply: Reply,
    deadline: Duration,
}

enum Reply {
    Call(oneshot::Sender<Result<Response>>),
    Probe,
}

impl HostConn {
    pub(crate) fn start_shared(
        config: Config,
        store: Store,
        clock: Arc<dyn Clock>,
        auth: Arc<Auth>,
    ) -> Result<Self> {
        let mut state = store
            .engine_root(&config.alias, &config.root)?
            .unwrap_or_default();
        let class = auth.state.borrow().class;
        if state.status == Status::Connecting && class.is_none() && !managed(&config.alias) {
            state.status = Status::NeedsAuth;
            state.cause = Some("First SSH attempt was interrupted; press Connect".into());
            if let Err(error) = auth.block(
                &store,
                &config.alias,
                state.since_ms,
                state.cause.clone().unwrap(),
            ) {
                eprintln!(
                    "Host {}: could not persist SSH auth state: {error:#}",
                    config.alias
                );
            }
        } else if matches!(state.status, Status::Live | Status::Connecting)
            || state.status == Status::Backoff && class != Some(HostClass::Interactive)
        {
            state.status = Status::Idle;
        }
        if let Some((since, cause)) = auth.state.borrow().blocked.as_ref() {
            state.status = Status::NeedsAuth;
            state.since_ms = *since;
            state.cause = Some(cause.clone());
            state.next_retry_ms = None;
        }
        if let Err(error) = store.save_engine_host(&config.alias, &config.root, &state) {
            eprintln!(
                "Host {}: could not persist SSH connection state: {error:#}",
                config.alias
            );
        }
        let (publish, state_rx) = watch::channel(state.clone());
        let (calls, incoming) = mpsc::channel(256);
        let (actions, action_rx) = mpsc::channel(8);
        let actor = Actor {
            auth,
            config,
            store,
            clock,
            state,
            publish,
            incoming,
            unsupported: None,
        };
        tokio::spawn(actor.run(action_rx));
        Ok(Self {
            calls,
            actions,
            state: state_rx,
        })
    }

    pub fn state(&self) -> watch::Receiver<HostState> {
        self.state.clone()
    }

    pub async fn request(&self, body: Request) -> Result<Response> {
        match &body {
            Request::PutBlob { bytes, .. } if bytes.len() > CHUNK => {
                bail!("Upload chunks must be at most 256 KiB")
            }
            Request::ReadLog { range, .. }
                if range.end.saturating_sub(range.start) > CHUNK as u64 =>
            {
                bail!("Log chunks must be at most 256 KiB")
            }
            _ => {}
        }
        let (reply, receive) = oneshot::channel();
        self.calls.send(Call { body, reply }).await.map_err(|_| {
            anyhow!(Unavailable {
                state: self.state.borrow().clone(),
                sent: false
            })
        })?;
        receive.await.map_err(|_| {
            anyhow!(Unavailable {
                state: self.state.borrow().clone(),
                sent: true
            })
        })?
    }

    pub async fn retry(&self) -> Result<()> {
        self.action(None, false, false).await
    }

    pub async fn retry_key(&self) -> Result<()> {
        self.action(None, true, false).await
    }

    pub async fn connect_interactive(&self, prompt: Prompt, keep_master: bool) -> Result<()> {
        self.action(Some(prompt), false, keep_master).await
    }

    async fn action(
        &self,
        prompt: Option<Prompt>,
        key_only: bool,
        keep_master: bool,
    ) -> Result<()> {
        let (reply, receive) = oneshot::channel();
        self.actions
            .send(Action {
                prompt,
                key_only,
                keep_master,
                reply,
            })
            .await
            .map_err(|_| anyhow!("Host transport stopped"))?;
        receive.await?
    }
}

struct Actor {
    auth: Arc<Auth>,
    config: Config,
    store: Store,
    clock: Arc<dyn Clock>,
    state: HostState,
    publish: watch::Sender<HostState>,
    incoming: mpsc::Receiver<Call>,
    unsupported: Option<String>,
}

fn lane(body: &Request) -> usize {
    match body {
        Request::Changes { .. } | Request::SlurmQueue { .. } => 1,
        Request::ReadLog { .. } => 2,
        Request::PutBlob { .. } | Request::HasBlob { .. } | Request::PinSource { .. } => 3,
        _ => 0,
    }
}

#[cfg(unix)]
async fn publish_master(config: &Config, route: &str) {
    let result = async {
        if let Some(home) = &config.sharing_home {
            let path = ssh::socket(config, route)?;
            crate::jobs::ssh::sharing::publish_engine(
                home,
                &config.alias,
                config.ssh_config.as_deref(),
                &path,
                route,
            )
            .await?;
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        eprintln!(
            "Host {}: connection sharing is unavailable: {error:#}",
            config.alias
        );
    }
}

impl Actor {
    fn unavailable(&self, sent: bool) -> anyhow::Error {
        let error = anyhow!(Unavailable {
            state: self.state.clone(),
            sent,
        });
        match &self.unsupported {
            Some(cause) => error.context(Unsupported(cause.clone())),
            None => error,
        }
    }

    fn save(&self) {
        if let Err(error) =
            self.store
                .save_engine_host(&self.config.alias, &self.config.root, &self.state)
        {
            eprintln!(
                "Host {}: could not persist SSH connection state: {error:#}",
                self.config.alias
            );
        }
        self.publish.send_replace(self.state.clone());
    }

    fn classify(&mut self, class: HostClass) {
        if let Err(error) = self.auth.update(&self.store, &self.config.alias, |state| {
            state.class = Some(class)
        }) {
            eprintln!(
                "Host {}: could not persist SSH auth state: {error:#}",
                self.config.alias
            );
        }
    }

    fn failed(&mut self, breaker: bool, cause: String) {
        let managed = managed(&self.config.alias);
        let now = self.clock.wall_ms();
        let refusing = self.state.cause.as_deref().is_some_and(key_refused);
        if self.state.cause.is_none() || managed && key_refused(&cause) && !refusing {
            self.state.since_ms = now;
        }
        let breaker = breaker
            && !(managed
                && key_refused(&cause)
                && now.saturating_sub(self.state.since_ms) < KEY_SYNC_GRACE_MS);
        if breaker && !managed && self.auth.state.borrow().class.is_none() {
            self.classify(HostClass::Interactive);
        }
        self.state.cause = Some(cause);
        if breaker {
            if let Err(error) = self.auth.block(
                &self.store,
                &self.config.alias,
                now,
                self.state.cause.clone().unwrap(),
            ) {
                eprintln!(
                    "Host {}: could not persist SSH auth breaker: {error:#}",
                    self.config.alias
                );
            }
        }
        let blocked = self.auth.state.borrow().blocked.clone();
        if let Some((since, cause)) = blocked {
            self.state.since_ms = since;
            self.state.cause = Some(cause);
            self.state.status = Status::NeedsAuth;
            self.state.next_retry_ms = None;
        } else {
            let steps = [5_000, 15_000, 60_000, 300_000, 900_000];
            let base = steps[self.state.failures.min(steps.len() - 1)];
            let noise =
                u16::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..2].try_into().unwrap()) as i64;
            let max_delay = if managed { 60_000 } else { 900_000 };
            let delay = (base * (800 + noise % 401) / 1000).min(max_delay);
            self.state.failures = self.state.failures.saturating_add(1);
            self.state.next_retry_ms = Some(now.saturating_add(delay));
            self.state.status = Status::Backoff;
        }
        self.save();
    }

    async fn connect(
        &mut self,
        prompt: Option<Prompt>,
        approve_route: bool,
        #[cfg_attr(windows, allow(unused_variables))] keep_master: bool,
    ) -> Result<ssh::Session> {
        let auth = self.auth.clone();
        let _guard = auth.gate.lock().await;
        self.unsupported = None;
        let blocked = auth.state.borrow().blocked.clone();
        if !approve_route {
            if let Some((_, cause)) = blocked {
                self.failed(true, cause.clone());
                bail!("{cause}");
            }
        }
        if approve_route {
            let now = self.clock.wall_ms();
            auth.update(&self.store, &self.config.alias, |state| {
                state.blocked = None;
            })?;
            self.state.cause = None;
            self.state.since_ms = now;
        }
        self.state.status = Status::Connecting;
        self.store
            .save_engine_host(&self.config.alias, &self.config.root, &self.state)?;
        self.publish.send_replace(self.state.clone());
        let route = match ssh::route(&self.config).await {
            Ok(route) => route,
            Err(error) => {
                let failure = ssh::failure(&error.to_string(), "", None, false);
                self.failed(failure.breaker, failure.cause);
                return Err(error);
            }
        };
        let retry_closure = !route
            .lines()
            .any(|line| line.starts_with("proxycommand ") || line.starts_with("proxyjump "));
        let pinned = auth.state.borrow().route.clone();
        if !approve_route && pinned.as_ref().is_some_and(|pin| pin != &route) {
            #[cfg(unix)]
            if let Some(previous) = &pinned {
                ssh::withdraw(&self.config, previous).await;
            }
            self.failed(
                true,
                "SSH route changed; press Connect to approve it".into(),
            );
            bail!("SSH route changed");
        }
        #[cfg(unix)]
        if let Some(previous) = pinned.filter(|previous| *previous != route) {
            ssh::stop_master(&self.config, &previous).await?;
        }
        #[cfg(windows)]
        if let Some(previous) = pinned.filter(|previous| *previous != route) {
            ssh::release(&self.config, &previous).await;
        }
        auth.update(&self.store, &self.config.alias, |state| {
            state.route = Some(route.clone())
        })?;
        #[cfg(unix)]
        let mut attach = false;
        #[cfg(windows)]
        let attach = false;
        let askpass = if let Some(prompt) = prompt {
            let askpass = askpass::Askpass::start(prompt).await?;
            #[cfg(unix)]
            {
                if self.auth.state.borrow().class.is_none() {
                    ssh::stop_master(&self.config, &route).await?;
                }
                let result =
                    ssh::master(&self.config, &ssh::socket(&self.config, &route)?, &askpass).await;
                if askpass.auth_prompted() {
                    self.classify(HostClass::Interactive);
                }
                match result {
                    Ok(authenticated) => {
                        if authenticated && !askpass.auth_prompted() {
                            self.classify(HostClass::Key);
                        }
                        if self.auth.state.borrow().class == Some(HostClass::Key) && !keep_master {
                            ssh::stop_master(&self.config, &route).await?;
                        } else {
                            attach = true;
                        }
                    }
                    Err(error) => {
                        let failure = ssh::failure(
                            "SSH connection failed",
                            &error.to_string(),
                            None,
                            retry_closure,
                        );
                        self.failed(failure.breaker, failure.cause);
                        return Err(error);
                    }
                }
            }
            Some(askpass)
        } else {
            #[cfg(unix)]
            if self.auth.state.borrow().class == Some(HostClass::Interactive) {
                let alive =
                    ssh::master_alive(&self.config, &ssh::socket(&self.config, &route)?).await?;
                if !alive {
                    ssh::withdraw(&self.config, &route).await;
                }
                if !alive && !approve_route {
                    self.failed(true, "SSH master ended; press Connect".into());
                    bail!("SSH master ended");
                }
                attach = alive;
            }
            None
        };
        // Windows has no ControlMaster; the SSH service may still hold this root's login.
        #[cfg(windows)]
        let resumed = if askpass.is_none()
            && self.auth.state.borrow().class == Some(HostClass::Interactive)
        {
            match ssh::attach(&self.config, &route).await {
                Ok(session) => Some(session),
                Err(_) if !approve_route => {
                    self.failed(true, "Interactive SSH requires Connect".into());
                    bail!("Interactive SSH requires Connect");
                }
                Err(_) => None,
            }
        } else {
            None
        };

        #[cfg(unix)]
        if attach {
            publish_master(&self.config, &route).await;
        }
        if !approve_route {
            if let Some((_, cause)) = auth.state.borrow().blocked.clone() {
                bail!("{cause}");
            }
        }
        #[cfg(windows)]
        let resuming = resumed.is_some();
        #[cfg(windows)]
        let mut session = match (resumed, &askpass) {
            (Some(session), _) => session,
            (None, Some(bridge)) => match ssh::hold(&self.config, &route, bridge).await {
                Ok(session) => session,
                Err(error) => {
                    eprintln!(
                        "Host {}: the SSH service cannot hold this login, so it ends with this server: {error:#}",
                        self.config.alias
                    );
                    ssh::spawn(&self.config, attach, &route, Some(bridge))?
                }
            },
            (None, None) => ssh::spawn(&self.config, attach, &route, None)?,
        };
        #[cfg(unix)]
        let mut session = ssh::spawn(&self.config, attach, &route, askpass.as_ref())?;
        #[cfg(windows)]
        let deadline = if askpass.is_some() {
            Duration::from_secs(300)
        } else {
            deadline()
        };
        #[cfg(unix)]
        let deadline = deadline();
        let result = tokio::time::timeout(deadline, async {
            #[cfg(windows)]
            if resuming {
                return session.resume().await;
            }
            let (os, arch) = session.handshake().await?;
            if askpass
                .as_ref()
                .is_some_and(|bridge| bridge.auth_prompted())
            {
                self.classify(HostClass::Interactive);
            } else if !attach {
                self.classify(HostClass::Key);
            }
            let binary = ssh::binary(self.config.binary_dir.as_deref(), &os, &arch)?;
            session.bootstrap(&binary).await
        })
        .await
        .map_err(|_| anyhow!("SSH did not respond within {} s", deadline.as_secs()))
        .and_then(|result| result);
        match result {
            Ok(()) => {
                let blocked = self.auth.state.borrow().blocked.clone();
                if !approve_route {
                    if let Some((_, cause)) = blocked {
                        self.failed(true, cause.clone());
                        bail!("{cause}");
                    }
                }
                if approve_route {
                    self.auth.update(&self.store, &self.config.alias, |state| {
                        state.blocked = None;
                    })?;
                }
                self.state.status = Status::Live;
                self.state.failures = 0;
                self.state.next_retry_ms = None;
                self.state.cause = None;
                self.state.since_ms = self.clock.wall_ms();
                self.state.last_live_ms = Some(self.state.since_ms);
                self.save();
                Ok(session)
            }
            Err(error) => {
                #[cfg(windows)]
                let resumed_older = resuming;
                #[cfg(unix)]
                let resumed_older = false;
                let unsupported = error.downcast_ref::<Unsupported>().is_some() && !resumed_older;
                let failure = session.failure(error, retry_closure).await;
                if unsupported || failure.unsupported {
                    self.unsupported = Some(failure.cause.clone());
                }
                if askpass
                    .as_ref()
                    .is_some_and(|bridge| bridge.auth_prompted())
                {
                    self.classify(HostClass::Interactive);
                }
                self.failed(failure.breaker, failure.cause.clone());
                bail!("{}", failure.cause)
            }
        }
    }

    async fn run(mut self, mut actions: mpsc::Receiver<Action>) {
        let mut auth = self.auth.state.subscribe();
        auth.borrow_and_update();
        let mut session: Option<ssh::Session> = None;
        let mut reader: Option<tokio::task::JoinHandle<()>> = None;
        let mut masters: tokio::task::JoinSet<(oneshot::Sender<Result<()>>, Result<()>, bool)> =
            tokio::task::JoinSet::new();
        let (events, mut responses) = mpsc::channel(64);
        let mut pending: BTreeMap<u64, Pending> = BTreeMap::new();
        let mut queues: [VecDeque<Call>; 4] = std::array::from_fn(|_| VecDeque::new());
        // Unique across restarts: a resumed Windows login can still carry replies to the last server.
        let mut id = self.clock.wall_ms().max(0) as u64 * 1000;
        let mut first = id;
        let mut jumps = JumpDetector::new(self.clock.as_ref());
        let mut last_reply = self.clock.monotonic();
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            let mut disconnect: Option<String> = None;
            let mut action = None;
            tokio::select! {
                Some(completed) = masters.join_next(), if !masters.is_empty() => {
                    if let Ok((reply, result, prompted)) = completed {
                        if prompted {
                            self.classify(HostClass::Interactive);
                        }
                        let _ = reply.send(result);
                    }
                }
                changed = auth.changed() => {
                    if changed.is_err() { break; }
                    let authorization = auth.borrow_and_update().clone();
                    match authorization.blocked {
                        Some((_, cause)) => { self.failed(true, cause.clone()); disconnect = Some(cause); }
                        None => {
                            if self.state.status == Status::NeedsAuth
                                && !(cfg!(windows) && authorization.class == Some(HostClass::Interactive)) {
                                self.state.status = Status::Idle;
                                self.state.cause = None;
                            }
                            self.save();
                        }
                    }
                }
                next = actions.recv() => {
                    match next { Some(next) => action = Some(next), None => break }
                }
                event = responses.recv(), if session.is_some() => {
                    match event {
                        Some(Ok(Message { id, body })) => {
                            last_reply = self.clock.monotonic();
                            self.state.last_live_ms = Some(self.clock.wall_ms());
                            if let Some(call) = pending.remove(&id) {
                                if let Reply::Call(reply) = call.reply { let _ = reply.send(Ok(body)); }
                            } else if id >= first { disconnect = Some("Unexpected orx-host response id".into()); }
                        }
                        Some(Err(_)) => disconnect = Some("Connection closed".into()),
                        None => disconnect = Some("Connection closed".into()),
                    }
                }
                call = self.incoming.recv(), if queues.iter().map(VecDeque::len).sum::<usize>() < 256 => {
                    match call {
                        Some(call) if session.is_some() || matches!(self.state.status, Status::Idle | Status::Connecting) => queues[lane(&call.body)].push_back(call),
                        Some(call) => { let _ = call.reply.send(Err(self.unavailable(false))); }
                        None => break,
                    }
                }
                _ = tick.tick() => {
                    let now = self.clock.monotonic();
                    if jumps.jumped(self.clock.as_ref()) && (session.is_some() || self.auth.state.borrow().class == Some(HostClass::Key) && self.state.status == Status::Backoff) {
                        disconnect = Some("Clock jump exceeded 30 seconds".into());
                    } else if session.is_some() && now >= last_reply + deadline()
                        || pending.values().any(|call| now >= call.deadline) {
                        disconnect = Some("Connection timed out".into());
                    } else if let Some(current) = session.as_mut().filter(|_| now >= last_reply + crate::config::paced(Duration::from_secs(15)) && !pending.values().any(|call| matches!(call.reply, Reply::Probe))) {
                        let body = Request::Hello { proto: orx_host_proto::PROTOCOL_VERSION };
                        let remaining = (last_reply + deadline()).saturating_sub(now);
                        let result = tokio::time::timeout(remaining, ssh::write(&mut current.input, &Message { id, body })).await;
                        if matches!(result, Ok(Ok(()))) {
                            pending.insert(id, Pending { reply: Reply::Probe, deadline: now + deadline() });
                            id += 1;
                        } else { disconnect = Some("Connection closed".into()); }
                    }
                }
            }
            if action.as_ref().is_some_and(|action| !action.key_only)
                && self.state.status == Status::Live
                && self.auth.state.borrow().blocked.is_none()
                && ssh::route(&self.config).await.ok().as_ref()
                    == self.auth.state.borrow().route.as_ref()
            {
                if let Some(action) = action.take() {
                    #[cfg(unix)]
                    if let (true, Some(prompt), Some(route)) = (
                        action.keep_master,
                        action.prompt.clone(),
                        self.auth.state.borrow().route.clone(),
                    ) {
                        let config = self.config.clone();
                        let auth = self.auth.clone();
                        masters.spawn(async move {
                            let mut reply = action.reply;
                            let mut prompted = false;
                            let connect = async {
                                let _guard = auth.gate.lock().await;
                                let askpass = askpass::Askpass::start(prompt).await?;
                                let result =
                                    ssh::master(&config, &ssh::socket(&config, &route)?, &askpass)
                                        .await;
                                prompted = askpass.auth_prompted();
                                result?;
                                publish_master(&config, &route).await;
                                Ok(())
                            };
                            let result = tokio::select! {
                                result = connect => result,
                                _ = reply.closed() => Err(anyhow!("SSH authentication cancelled.")),
                            };
                            (reply, result, prompted)
                        });
                    } else {
                        let _ = action.reply.send(Ok(()));
                    }
                    #[cfg(windows)]
                    let _ = action.reply.send(Ok(()));
                }
            }
            if action.as_ref().is_some_and(|action| {
                action.key_only && matches!(self.state.status, Status::Live | Status::Connecting)
            }) {
                if let Some(action) = action.take() {
                    let _ = action.reply.send(Ok(()));
                }
            }
            if action.as_ref().is_some_and(|action| {
                action.key_only
                    && (self.state.status == Status::NeedsAuth
                        || self.auth.state.borrow().class != Some(HostClass::Key))
            }) {
                if let Some(action) = action.take() {
                    let _ = action.reply.send(Err(anyhow!("Host requires Connect")));
                }
            }
            if let Some(current) = session
                .as_mut()
                .filter(|_| disconnect.is_none() && action.is_none())
            {
                while pending.len() < 24 {
                    while queues.iter().map(VecDeque::len).sum::<usize>() < 256 {
                        let Ok(call) = self.incoming.try_recv() else {
                            break;
                        };
                        queues[lane(&call.body)].push_back(call);
                    }
                    let call = queues
                        .iter_mut()
                        .zip([24, 20, 16, 8])
                        .find_map(|(queue, limit)| {
                            if pending.len() < limit {
                                queue.pop_front()
                            } else {
                                None
                            }
                        });
                    let Some(call) = call else {
                        break;
                    };
                    if call.reply.is_closed() {
                        continue;
                    }
                    let timeout = reply_deadline(&call.body);
                    let remaining = pending
                        .values()
                        .map(|call| call.deadline.saturating_sub(self.clock.monotonic()))
                        .min()
                        .unwrap_or(deadline())
                        .min((last_reply + deadline()).saturating_sub(self.clock.monotonic()));
                    let result = tokio::time::timeout(
                        remaining,
                        ssh::write(
                            &mut current.input,
                            &Message {
                                id,
                                body: call.body,
                            },
                        ),
                    )
                    .await;
                    pending.insert(
                        id,
                        Pending {
                            reply: Reply::Call(call.reply),
                            deadline: self.clock.monotonic() + timeout,
                        },
                    );
                    if matches!(result, Ok(Ok(()))) {
                        id += 1;
                    } else {
                        disconnect = Some("Connection closed".into());
                        break;
                    }
                }
            }
            if disconnect.is_some() || action.is_some() {
                #[cfg(unix)]
                let route = self.auth.state.borrow().route.clone().filter(|_| {
                    self.config.sharing_home.is_some()
                        && self.auth.state.borrow().class == Some(HostClass::Interactive)
                });
                #[cfg(unix)]
                if let Some(route) = route {
                    if let Ok(path) = ssh::socket(&self.config, &route) {
                        if !ssh::master_alive(&self.config, &path)
                            .await
                            .unwrap_or(false)
                        {
                            ssh::withdraw(&self.config, &route).await;
                        }
                    }
                }
                if let Some(mut old) = session.take() {
                    old.close().await;
                }
                if let Some(task) = reader.take() {
                    task.abort();
                    let _ = task.await;
                }
                while responses.try_recv().is_ok() {}
                if let Some(cause) = disconnect {
                    let jump = cause.starts_with("Clock jump");
                    self.failed(false, cause);
                    if jump && self.auth.state.borrow().class == Some(HostClass::Key) {
                        self.state.next_retry_ms = Some(self.clock.wall_ms());
                        self.save();
                    }
                }
                for (_, call) in std::mem::take(&mut pending) {
                    if let Reply::Call(reply) = call.reply {
                        let _ = reply.send(Err(self.unavailable(true)));
                    }
                }
                for queue in &mut queues {
                    for call in queue.drain(..) {
                        let _ = call.reply.send(Err(self.unavailable(false)));
                    }
                }
            }
            let now = self.clock.wall_ms();
            let automatic = matches!(self.state.status, Status::Idle)
                || (self.state.status == Status::Backoff
                    && self.state.next_retry_ms.is_some_and(|at| now >= at));
            if session.is_none() && (automatic || action.is_some()) {
                let mut next_action = action;
                let mut replies = Vec::new();
                let result = loop {
                    if next_action.as_ref().is_some_and(|action| action.key_only) {
                        self.state.failures = 0;
                        self.state.next_retry_ms = None;
                    }
                    let approve_route = next_action.as_ref().is_some_and(|action| !action.key_only);
                    let keep_master = next_action
                        .as_ref()
                        .is_some_and(|action| action.keep_master);
                    let prompt = next_action.take().and_then(|action| {
                        replies.push(action.reply);
                        action.prompt
                    });
                    let mut queued_action = None;
                    let mut enqueue = |action: Action| {
                        if !approve_route && queued_action.is_none() {
                            queued_action = Some(action);
                        } else {
                            replies.push(action.reply);
                        }
                    };
                    let result = {
                        let attempt = self.connect(prompt, approve_route, keep_master);
                        tokio::pin!(attempt);
                        loop {
                            tokio::select! {
                                result = &mut attempt => break result,
                                Some(action) = actions.recv(), if !actions.is_closed() => {
                                    if action.key_only { let _ = action.reply.send(Ok(())); }
                                    else { enqueue(action); }
                                },
                            }
                        }
                    };
                    while let Ok(action) = actions.try_recv() {
                        enqueue(action);
                    }
                    if queued_action.as_ref().is_some_and(|action| {
                        action.key_only
                            && (self.state.status == Status::NeedsAuth
                                || self.auth.state.borrow().class != Some(HostClass::Key))
                    }) {
                        if let Some(action) = queued_action.take() {
                            let _ = action.reply.send(Err(anyhow!("Host requires Connect")));
                        }
                    }
                    // A Connect pressed during an automatic attempt approves only the current route.
                    let unapproved = !approve_route
                        && result.is_ok()
                        && queued_action
                            .as_ref()
                            .is_some_and(|action| !action.key_only)
                        && ssh::route(&self.config).await.ok().as_ref()
                            != self.auth.state.borrow().route.as_ref();
                    if queued_action.is_some()
                        && (result.is_err()
                            || unapproved
                            || cfg!(unix)
                                && !keep_master
                                && queued_action
                                    .as_ref()
                                    .is_some_and(|action| action.keep_master))
                    {
                        next_action = queued_action;
                        continue;
                    }
                    if let Some(action) = queued_action {
                        replies.push(action.reply);
                    }
                    break result;
                };
                auth.borrow_and_update();
                let error = result.as_ref().err().map(|error| format!("{error:#}"));
                match result {
                    Ok(mut connected) => {
                        // The reader owns the pipe so cancelling select branches cannot lose frame bytes.
                        let mut output = connected.output.take().unwrap();
                        let sender = events.clone();
                        reader = Some(tokio::spawn(async move {
                            loop {
                                let result = ssh::read(&mut output).await;
                                let failed = result.is_err();
                                if sender.send(result).await.is_err() || failed {
                                    break;
                                }
                            }
                        }));
                        session = Some(connected);
                        first = id;
                        last_reply = self.clock.monotonic();
                    }
                    Err(error) => {
                        if self.state.status != Status::NeedsAuth
                            && self.state.status != Status::Backoff
                        {
                            self.failed(true, format!("{error:#}"));
                        }
                        for queue in &mut queues {
                            for call in queue.drain(..) {
                                let _ = call.reply.send(Err(self.unavailable(false)));
                            }
                        }
                    }
                }
                for reply in replies {
                    let result = match &error {
                        Some(error) => Err(anyhow!("{error}")),
                        None => Ok(()),
                    };
                    let _ = reply.send(result);
                }
            }
        }
        if let Some(task) = reader {
            task.abort();
            let _ = task.await;
        }
    }
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(test)]
mod state_tests {
    use super::*;

    #[tokio::test]
    async fn interrupted_first_contact_requires_user_action_even_after_restart() {
        let directory =
            std::env::temp_dir().join(format!("orx-transport-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(directory.clone()).unwrap();
        store
            .seed_engine_host(
                "interrupted",
                "/unused",
                None,
                &HostState {
                    status: Status::Connecting,
                    ..HostState::default()
                },
            )
            .unwrap();
        let config = Config {
            alias: "interrupted".into(),
            root: "/unused".into(),
            ssh_config: Some(directory.join("missing-ssh-config")),
            binary_dir: None,
            #[cfg(unix)]
            sharing_home: None,
            askpass_executable: std::env::current_exe().unwrap(),
            #[cfg(unix)]
            control_dir: directory.join("cm"),
        };
        let auth = Auth::new(&store, &config.alias).unwrap();
        let conn = HostConn::start_shared(
            config,
            store,
            Arc::new(crate::engine::clock::SystemClock::new()),
            auth,
        )
        .unwrap();
        assert_eq!(conn.state().borrow().status, Status::NeedsAuth);
        assert!(conn
            .request(Request::Hello {
                proto: orx_host_proto::PROTOCOL_VERSION
            })
            .await
            .is_err());
        assert!(conn.retry().await.is_err());
        let prompt: Prompt = Arc::new(|_| Box::pin(async { bail!("unexpected prompt") }));
        assert!(conn.connect_interactive(prompt, false).await.is_err());
        drop(conn);
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::remove_dir_all(directory).unwrap();
    }
}
