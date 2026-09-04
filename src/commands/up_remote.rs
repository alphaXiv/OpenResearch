//! `orx up --remote <host>` — run `orx up` on a remote box and forward it here.
//!
//! This is the laptop-side half of remote access. Unlike a bare `orx up` on a
//! box you SSH'd into (which can only *print* an `ssh -L` command — see
//! `crate::remote`), here orx owns the SSH client, so it can set up the local
//! forward itself: it starts `orx up` on the remote, tunnels the port to this
//! machine, waits for the server to come up, and opens the browser.
//!
//! Transport is the `ssh` binary with the same ControlMaster/BatchMode options
//! the SSH job backend uses (`crate::jobs::ssh`); auth is the user's own
//! `~/.ssh/config` + agent/keys — orx never reads a key.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, RequestExt as _, Router};
use futures::TryStreamExt as _;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::{watch, Mutex, RwLock};

use crate::commands::remote_host::{
    ControlResponse, HostDescriptor, ATTACHED_MARKER, CONTROL_PROTOCOL, HOST_MARKER,
};
use crate::error::{anyhow, Result};
use crate::jobs::ssh::{HostKeyPolicy, SshTarget};
use crate::{browser, UpArgs};

/// How long to wait for the remote server to answer through the forward.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const DASHBOARD_PROTOCOL: u32 = 1;
pub(crate) const REMOTE_READY_MARKER: &str = "ORX_REMOTE_READY=1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RemoteSessionStatus {
    Checking,
    NeedsInstall,
    NeedsUpdate,
    Installing,
    Updating,
    Connecting,
    Connected,
    Reconnecting,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteUiPreferences {
    pub theme: Option<String>,
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteInstallPathsInfo {
    pub binary: String,
    pub database: String,
    pub cache: String,
}

impl From<&RemoteInstallPaths> for RemoteInstallPathsInfo {
    fn from(paths: &RemoteInstallPaths) -> Self {
        Self {
            binary: paths.binary.clone(),
            database: paths.database.clone(),
            cache: paths.cache.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteSessionInfo {
    pub id: String,
    pub host: String,
    pub user: Option<String>,
    pub status: RemoteSessionStatus,
    pub version: Option<String>,
    pub dashboard_protocol: Option<u32>,
    pub gateway_url: String,
    pub error: Option<String>,
    pub install_paths: Option<RemoteInstallPathsInfo>,
    pub ui_preferences: RemoteUiPreferences,
    pub can_start_new_host: bool,
}

#[derive(Clone)]
struct UpstreamRoute {
    port: u16,
    token: String,
}

struct ConnectionControl {
    cancel: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

struct RemoteSession {
    target: SshTarget,
    client: reqwest::Client,
    info: RwLock<RemoteSessionInfo>,
    route: RwLock<Option<UpstreamRoute>>,
    operation: Mutex<()>,
    connection: Mutex<Option<ConnectionControl>>,
    gateway_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    generation: AtomicU64,
    dev_origin: Option<String>,
    expected_instance: RwLock<Option<String>>,
}

#[derive(Default)]
struct RemoteSessionRegistry {
    by_id: HashMap<String, Arc<RemoteSession>>,
    by_host: HashMap<String, String>,
}

#[derive(Clone)]
pub(crate) struct RemoteSessionManager {
    registry: Arc<Mutex<RemoteSessionRegistry>>,
}

impl RemoteSessionManager {
    pub(crate) fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(RemoteSessionRegistry::default())),
        }
    }

    pub(crate) async fn list(&self) -> Vec<RemoteSessionInfo> {
        let sessions = {
            let registry = self.registry.lock().await;
            registry.by_id.values().cloned().collect::<Vec<_>>()
        };
        let mut infos = Vec::with_capacity(sessions.len());
        for session in sessions {
            infos.push(session.info.read().await.clone());
        }
        infos.sort_by(|left, right| left.host.cmp(&right.host));
        infos
    }

    pub(crate) async fn get(&self, id: &str) -> Option<RemoteSessionInfo> {
        let session = {
            let registry = self.registry.lock().await;
            registry.by_id.get(id).cloned()
        }?;
        let info = session.info.read().await.clone();
        Some(info)
    }

    pub(crate) async fn create(
        &self,
        host: String,
        preferences: RemoteUiPreferences,
        gateway_port: Option<u16>,
    ) -> Result<(bool, RemoteSessionInfo)> {
        let mut registry = self.registry.lock().await;
        if let Some(id) = registry.by_host.get(&host) {
            if let Some(session) = registry.by_id.get(id) {
                let mut info = session.info.write().await;
                info.ui_preferences = preferences;
                return Ok((false, info.clone()));
            }
        }

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", gateway_port.unwrap_or(0)))
            .await
            .map_err(|error| {
                if let Some(port) = gateway_port {
                    anyhow!("Could not bind 127.0.0.1:{port}: {error}. Pass a different --port.")
                } else {
                    anyhow!("Could not allocate a local remote-workspace port: {error}")
                }
            })?;
        let port = listener.local_addr()?.port();
        let id = uuid::Uuid::new_v4().to_string();
        let info = RemoteSessionInfo {
            id: id.clone(),
            host: host.clone(),
            user: None,
            status: RemoteSessionStatus::Checking,
            version: None,
            dashboard_protocol: None,
            gateway_url: format!("http://127.0.0.1:{port}"),
            error: None,
            install_paths: None,
            ui_preferences: preferences,
            can_start_new_host: false,
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .http1_only()
            .build()?;
        let session = Arc::new(RemoteSession {
            target: parse_remote_target(&host),
            client,
            info: RwLock::new(info.clone()),
            route: RwLock::new(None),
            operation: Mutex::new(()),
            connection: Mutex::new(None),
            gateway_task: Mutex::new(None),
            generation: AtomicU64::new(0),
            dev_origin: validated_dev_origin(),
            expected_instance: RwLock::new(None),
        });
        registry.by_host.insert(host, id.clone());
        registry.by_id.insert(id, session.clone());
        drop(registry);

        let gateway_session = session.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, gateway_router(gateway_session)).await {
                eprintln!("remote workspace gateway ended: {error}");
            }
        });
        *session.gateway_task.lock().await = Some(task);
        let preparing = session.clone();
        let generation = session.generation.fetch_add(1, Ordering::SeqCst) + 1;
        tokio::spawn(async move {
            prepare_remote_session(preparing, generation).await;
        });
        Ok((true, info))
    }

    async fn session(&self, id: &str) -> Result<Arc<RemoteSession>> {
        self.registry
            .lock()
            .await
            .by_id
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("Remote session not found."))
    }

    pub(crate) async fn reconnect(&self, id: &str) -> Result<RemoteSessionInfo> {
        let session = self.session(id).await?;
        Ok(reconnect_session(session).await)
    }

    pub(crate) async fn disconnect(&self, id: &str) -> Result<RemoteSessionInfo> {
        let session = self.session(id).await?;
        disconnect_session(&session).await;
        let info = session.info.read().await.clone();
        Ok(info)
    }

    pub(crate) async fn shutdown(&self) {
        let sessions = {
            let registry = self.registry.lock().await;
            registry.by_id.values().cloned().collect::<Vec<_>>()
        };
        for session in sessions {
            disconnect_session(&session).await;
            if let Some(task) = session.gateway_task.lock().await.take() {
                task.abort();
            }
        }
    }
}

fn validated_dev_origin() -> Option<String> {
    let value = std::env::var("ORX_UI_DEV_ORIGIN").ok()?;
    let url = reqwest::Url::parse(&value).ok()?;
    let loopback = url
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|ip| ip.is_loopback())
        || url.host_str() == Some("localhost");
    (url.scheme() == "http" && loopback && url.path() == "/")
        .then(|| value.trim_end_matches('/').to_string())
}

async fn set_session_state(
    session: &RemoteSession,
    status: RemoteSessionStatus,
    error: Option<String>,
) {
    let mut info = session.info.write().await;
    info.status = status;
    info.error = error;
}

async fn set_session_state_if_current(
    session: &RemoteSession,
    generation: u64,
    status: RemoteSessionStatus,
    error: Option<String>,
) -> bool {
    update_session_if_current(session, generation, |info| {
        info.status = status;
        info.error = error;
    })
    .await
}

async fn update_session_if_current(
    session: &RemoteSession,
    generation: u64,
    update: impl FnOnce(&mut RemoteSessionInfo),
) -> bool {
    let mut info = session.info.write().await;
    if session.generation.load(Ordering::SeqCst) != generation {
        return false;
    }
    update(&mut info);
    true
}

async fn prepare_remote_session(session: Arc<RemoteSession>, generation: u64) {
    let _operation = session.operation.lock().await;
    if session.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    if !matches!(
        session.info.read().await.status,
        RemoteSessionStatus::Checking
            | RemoteSessionStatus::Disconnected
            | RemoteSessionStatus::Failed
    ) {
        return;
    }
    if !set_session_state_if_current(&session, generation, RemoteSessionStatus::Checking, None)
        .await
    {
        return;
    }
    *session.route.write().await = None;

    let host = session.info.read().await.host.clone();
    let paths = match remote_install_paths(&session.target, &host).await {
        Ok(paths) => paths,
        Err(error) => {
            set_session_state_if_current(
                &session,
                generation,
                RemoteSessionStatus::Failed,
                Some(error.to_string()),
            )
            .await;
            return;
        }
    };
    if !update_session_if_current(&session, generation, |info| {
        info.user = Some(paths.user.clone());
    })
    .await
    {
        return;
    }
    let remote_orx_result = find_remote_orx(&session.target, &host).await;
    if session.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let remote_orx = match remote_orx_result {
        Ok(Some(orx)) => orx,
        Ok(None) => {
            let guidance = (crate::telemetry::build_channel() != "production").then(|| {
                "This development build cannot install an unreleased public version. Build this branch for the remote machine and place the binary at the path shown below."
                    .to_string()
            });
            update_session_if_current(&session, generation, |info| {
                info.status = RemoteSessionStatus::NeedsInstall;
                info.error = guidance;
                info.install_paths = Some((&paths).into());
            })
            .await;
            return;
        }
        Err(error) => {
            set_session_state_if_current(
                &session,
                generation,
                RemoteSessionStatus::Failed,
                Some(error.to_string()),
            )
            .await;
            return;
        }
    };
    let expected_instance = session.expected_instance.read().await.clone();
    let running = remote_host_status(&session.target, &remote_orx.path, &paths).await;
    if session.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let running = match running {
        Ok(running) => running,
        Err(error) => {
            set_session_state_if_current(
                &session,
                generation,
                RemoteSessionStatus::Failed,
                Some(error.to_string()),
            )
            .await;
            return;
        }
    };
    let descriptor = if let Some(descriptor) = running {
        if expected_instance
            .as_deref()
            .is_some_and(|expected| expected != descriptor.instance_id)
        {
            set_session_state_if_current(
                &session,
                generation,
                RemoteSessionStatus::Failed,
                Some(format!(
                    "A different OpenResearch host is running on {}. Start a new connection explicitly.",
                    descriptor.hostname
                )),
            )
            .await;
            return;
        }
        descriptor
    } else {
        let protocol =
            match remote_dashboard_protocol(&session.target, &host, &remote_orx.path).await {
                Ok(protocol) => protocol,
                Err(error) => {
                    set_session_state_if_current(
                        &session,
                        generation,
                        RemoteSessionStatus::Failed,
                        Some(error.to_string()),
                    )
                    .await;
                    return;
                }
            };
        if protocol != Some(DASHBOARD_PROTOCOL) {
            let message = protocol
                .filter(|protocol| *protocol > DASHBOARD_PROTOCOL)
                .map(|_| "This remote OpenResearch is newer than the local app. Update OpenResearch locally, then reconnect.".to_string())
                .or_else(|| (crate::telemetry::build_channel() != "production").then(|| {
                    "This development build cannot install an unreleased public version. Build this branch for the remote machine and replace the binary at the path shown below."
                        .to_string()
                }));
            update_session_if_current(&session, generation, |info| {
                info.status = RemoteSessionStatus::NeedsUpdate;
                info.error = message;
                info.install_paths = Some((&paths).into());
            })
            .await;
            return;
        }
        match ensure_remote_host(
            &session.target,
            &remote_orx.path,
            &paths,
            expected_instance.as_deref(),
        )
        .await
        {
            Ok(descriptor) => descriptor,
            Err(error) => {
                let error = error.to_string();
                update_session_if_current(&session, generation, |info| {
                    info.status = RemoteSessionStatus::Failed;
                    info.can_start_new_host = expected_instance.is_some()
                        && error.contains("The expected OpenResearch host is no longer running.");
                    info.error = Some(error);
                })
                .await;
                return;
            }
        }
    };
    if descriptor.control_protocol != CONTROL_PROTOCOL {
        set_session_state_if_current(
            &session,
            generation,
            RemoteSessionStatus::Failed,
            Some("The running OpenResearch host uses an incompatible control protocol.".into()),
        )
        .await;
        return;
    }
    let protocol = Some(descriptor.dashboard_protocol);
    if !update_session_if_current(&session, generation, |info| {
        info.version = Some(descriptor.version.clone());
        info.dashboard_protocol = protocol;
    })
    .await
    {
        return;
    }
    if protocol != Some(DASHBOARD_PROTOCOL) {
        let message = protocol
            .filter(|protocol| *protocol > DASHBOARD_PROTOCOL)
            .map(|_| "This remote OpenResearch is newer than the local app. Update OpenResearch locally, then reconnect.".to_string())
            .or_else(|| Some("The running OpenResearch host uses an older dashboard protocol. Stop it before updating the remote binary.".to_string()));
        update_session_if_current(&session, generation, |info| {
            info.status = RemoteSessionStatus::NeedsUpdate;
            info.error = message;
            info.install_paths = None;
        })
        .await;
        return;
    }
    *session.expected_instance.write().await = Some(descriptor.instance_id.clone());
    update_session_if_current(&session, generation, |info| {
        info.install_paths = None;
        info.can_start_new_host = false;
    })
    .await;
    start_connection(
        session.clone(),
        remote_orx,
        paths,
        descriptor,
        generation,
        false,
    )
    .await;
}

async fn install_and_connect(
    session: Arc<RemoteSession>,
    requested: RemoteInstallPathsInfo,
) -> Result<RemoteSessionInfo> {
    let _operation = session.operation.lock().await;
    let previous = session.info.read().await.status;
    if !matches!(
        previous,
        RemoteSessionStatus::NeedsInstall | RemoteSessionStatus::NeedsUpdate
    ) {
        return Ok(session.info.read().await.clone());
    }
    if crate::telemetry::build_channel() != "production" {
        return Err(anyhow!(
            "This development build cannot install an unreleased remote protocol. Build this branch for the remote machine and place the binary at the selected path."
        ));
    }
    let generation = session.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let host = session.info.read().await.host.clone();
    if !set_session_state_if_current(
        &session,
        generation,
        if previous == RemoteSessionStatus::NeedsUpdate {
            RemoteSessionStatus::Updating
        } else {
            RemoteSessionStatus::Installing
        },
        None,
    )
    .await
    {
        return Ok(session.info.read().await.clone());
    }
    let result = async {
        let paths = remote_install_paths(&session.target, &host)
            .await?
            .with_requested(requested.binary, requested.database, requested.cache);
        save_remote_install_paths(&session.target, &host, &paths).await?;
        let remote_orx = install_remote_orx(&session.target, &host, &paths).await?;
        let protocol = remote_dashboard_protocol(&session.target, &host, &remote_orx.path).await?;
        Ok::<_, crate::error::Error>((paths, remote_orx, protocol))
    }
    .await;
    let (paths, remote_orx, protocol) = match result {
        Ok(result) => result,
        Err(error) => {
            set_session_state_if_current(
                &session,
                generation,
                RemoteSessionStatus::Failed,
                Some(error.to_string()),
            )
            .await;
            return Err(error);
        }
    };
    if session.generation.load(Ordering::SeqCst) != generation {
        return Ok(session.info.read().await.clone());
    }
    if protocol != Some(DASHBOARD_PROTOCOL) {
        set_session_state_if_current(
            &session,
            generation,
            RemoteSessionStatus::Failed,
            Some("The installed remote binary does not support this dashboard protocol.".into()),
        )
        .await;
        return Err(anyhow!("The installed remote binary is incompatible."));
    }
    {
        let mut info = session.info.write().await;
        if session.generation.load(Ordering::SeqCst) != generation {
            return Ok(info.clone());
        }
        info.version = Some(remote_orx.version.clone());
        info.dashboard_protocol = protocol;
        info.install_paths = None;
    }
    let descriptor = ensure_remote_host(&session.target, &remote_orx.path, &paths, None).await?;
    *session.expected_instance.write().await = Some(descriptor.instance_id.clone());
    start_connection(
        session.clone(),
        remote_orx,
        paths,
        descriptor,
        generation,
        false,
    )
    .await;
    Ok(session.info.read().await.clone())
}

async fn start_connection(
    session: Arc<RemoteSession>,
    remote_orx: RemoteOrx,
    paths: RemoteInstallPaths,
    descriptor: HostDescriptor,
    generation: u64,
    reconnecting: bool,
) {
    if session.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let mut connection = session.connection.lock().await;
    if let Some(control) = connection.take() {
        stop_connection(control).await;
    }
    if !set_session_state_if_current(
        &session,
        generation,
        if reconnecting {
            RemoteSessionStatus::Reconnecting
        } else {
            RemoteSessionStatus::Connecting
        },
        None,
    )
    .await
    {
        return;
    }
    let (cancel, cancelled) = watch::channel(false);
    let task_session = session.clone();
    let task = tokio::spawn(async move {
        supervise_connection(
            task_session,
            remote_orx,
            paths,
            descriptor,
            generation,
            cancelled,
        )
        .await;
    });
    *connection = Some(ConnectionControl { cancel, task });
}

async fn disconnect_session(session: &Arc<RemoteSession>) {
    session.generation.fetch_add(1, Ordering::SeqCst);
    *session.route.write().await = None;
    if let Some(control) = session.connection.lock().await.take() {
        stop_connection(control).await;
    }
    set_session_state(session, RemoteSessionStatus::Disconnected, None).await;
}

async fn reconnect_session(session: Arc<RemoteSession>) -> RemoteSessionInfo {
    let generation = {
        let mut info = session.info.write().await;
        if !matches!(
            info.status,
            RemoteSessionStatus::Disconnected | RemoteSessionStatus::Failed
        ) {
            return info.clone();
        }
        info.status = RemoteSessionStatus::Checking;
        info.error = None;
        session.generation.fetch_add(1, Ordering::SeqCst) + 1
    };
    let reconnecting = session.clone();
    tokio::spawn(async move {
        prepare_remote_session(reconnecting, generation).await;
    });
    session.info.read().await.clone()
}

async fn stop_connection(control: ConnectionControl) {
    let _ = control.cancel.send(true);
    let mut task = control.task;
    if tokio::time::timeout(Duration::from_secs(10), &mut task)
        .await
        .is_err()
    {
        task.abort();
    }
}

async fn supervise_connection(
    session: Arc<RemoteSession>,
    remote_orx: RemoteOrx,
    paths: RemoteInstallPaths,
    descriptor: HostDescriptor,
    generation: u64,
    cancelled: watch::Receiver<bool>,
) {
    const BACKOFF: [u64; 5] = [1, 2, 5, 10, 20];
    let mut retry = 0_usize;
    loop {
        if *cancelled.borrow() || session.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if retry > 0
            && !set_session_state_if_current(
                &session,
                generation,
                RemoteSessionStatus::Reconnecting,
                None,
            )
            .await
        {
            return;
        }
        match connect_once(
            &session,
            &remote_orx,
            &paths,
            &descriptor,
            generation,
            cancelled.clone(),
            retry > 0,
        )
        .await
        {
            ConnectionEnd::Cancelled => return,
            ConnectionEnd::Terminal(error) => {
                *session.route.write().await = None;
                set_session_state_if_current(
                    &session,
                    generation,
                    RemoteSessionStatus::Failed,
                    Some(error),
                )
                .await;
                return;
            }
            ConnectionEnd::Retryable(error) => {
                *session.route.write().await = None;
                if !set_session_state_if_current(
                    &session,
                    generation,
                    RemoteSessionStatus::Reconnecting,
                    (!error.is_empty()).then_some(error),
                )
                .await
                {
                    return;
                }
                let delay = BACKOFF[retry.min(BACKOFF.len() - 1)];
                retry += 1;
                let mut wait_cancel = cancelled.clone();
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
                    changed = wait_cancel.changed() => {
                        if changed.is_err() || *wait_cancel.borrow() { return; }
                    }
                }
            }
        }
    }
}

enum ConnectionEnd {
    Cancelled,
    Retryable(String),
    Terminal(String),
}

async fn connect_once(
    session: &RemoteSession,
    remote_orx: &RemoteOrx,
    paths: &RemoteInstallPaths,
    expected: &HostDescriptor,
    generation: u64,
    mut cancelled: watch::Receiver<bool>,
    retrying: bool,
) -> ConnectionEnd {
    let descriptor = match ensure_remote_host(
        &session.target,
        &remote_orx.path,
        paths,
        Some(&expected.instance_id),
    )
    .await
    {
        Ok(descriptor) => descriptor,
        Err(error) => {
            let error = error.to_string();
            return if retrying && retryable_ssh_transport_error(&error) {
                ConnectionEnd::Retryable(error)
            } else {
                ConnectionEnd::Terminal(error)
            };
        }
    };
    let local_port = match reserve_loopback_port() {
        Ok(port) => port,
        Err(error) => return ConnectionEnd::Terminal(error.to_string()),
    };
    let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let forward = forward_spec(local_port, descriptor.port);
    let remote_cmd = match remote_attach_cmd(&remote_orx.path, &descriptor.instance_id, paths) {
        Ok(command) => command,
        Err(error) => return ConnectionEnd::Terminal(error.to_string()),
    };
    let mut command = match ssh_forward_command(&session.target, &forward, &remote_cmd) {
        Ok(command) => command,
        Err(error) => return ConnectionEnd::Terminal(error.to_string()),
    };
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let error = ssh_spawn_error(error, "remote access").to_string();
            return ConnectionEnd::Terminal(error);
        }
    };
    let mut stdin = match child.stdin.take() {
        Some(stdin) => stdin,
        None => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return ConnectionEnd::Terminal("Could not open the SSH session input.".into());
        }
    };
    if tokio::io::AsyncWriteExt::write_all(&mut stdin, format!("{token}\n").as_bytes())
        .await
        .is_err()
        || tokio::io::AsyncWriteExt::flush(&mut stdin).await.is_err()
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return ConnectionEnd::Terminal(
            "Could not authenticate the remote OpenResearch process.".into(),
        );
    }
    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return ConnectionEnd::Terminal("Could not read remote OpenResearch startup.".into());
        }
    };
    let mut reader = tokio::io::BufReader::new(stdout);
    let ready = async {
        let mut bytes = 0_usize;
        loop {
            let mut line = String::new();
            let read = tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut line).await?;
            if read == 0 {
                return Ok::<bool, std::io::Error>(false);
            }
            bytes += read;
            if bytes > 64 * 1024 {
                return Ok(false);
            }
            if line.trim() == ATTACHED_MARKER {
                return Ok(true);
            }
        }
    };
    let ready = tokio::select! {
        result = tokio::time::timeout(HEALTH_TIMEOUT, ready) => matches!(result, Ok(Ok(true))),
        changed = cancelled.changed() => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return if changed.is_ok() && *cancelled.borrow() { ConnectionEnd::Cancelled } else { ConnectionEnd::Terminal("Remote session cancelled.".into()) };
        }
    };
    if !ready {
        let _ = child.start_kill();
        let _ = child.wait().await;
        let error = "SSH closed before the remote dashboard became ready.".into();
        return if retrying {
            ConnectionEnd::Retryable(error)
        } else {
            ConnectionEnd::Terminal(error)
        };
    }
    if let Err(error) = authenticated_health(&session.client, local_port, &token, &descriptor).await
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return ConnectionEnd::Terminal(error.to_string());
    }
    if session.generation.load(Ordering::SeqCst) != generation {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return ConnectionEnd::Cancelled;
    }
    tokio::spawn(async move {
        let mut sink = tokio::io::sink();
        let _ = tokio::io::copy(&mut reader, &mut sink).await;
    });
    *session.route.write().await = Some(UpstreamRoute {
        port: local_port,
        token,
    });
    if !set_session_state_if_current(session, generation, RemoteSessionStatus::Connected, None)
        .await
    {
        *session.route.write().await = None;
        let _ = child.start_kill();
        let _ = child.wait().await;
        return ConnectionEnd::Cancelled;
    }

    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            status = child.wait() => {
                return ConnectionEnd::Retryable(match status {
                    Ok(status) => format!("SSH connection ended ({status})."),
                    Err(error) => format!("Could not wait for SSH: {error}"),
                });
            }
            _ = heartbeat.tick() => {
                if tokio::io::AsyncWriteExt::write_all(&mut stdin, b"ping\n").await.is_err()
                    || tokio::io::AsyncWriteExt::flush(&mut stdin).await.is_err()
                {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    return ConnectionEnd::Retryable("The SSH heartbeat failed.".into());
                }
            }
            changed = cancelled.changed() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return if changed.is_ok() && *cancelled.borrow() {
                    ConnectionEnd::Cancelled
                } else {
                    ConnectionEnd::Retryable("Remote session cancelled.".into())
                };
            }
        }
    }
}

fn retryable_ssh_transport_error(error: &str) -> bool {
    error.contains("failed (exit 255)")
        && ![
            "Permission denied",
            "Host key verification failed",
            "REMOTE HOST IDENTIFICATION HAS CHANGED",
            "No matching host key",
            "no matching host key",
            "Too many authentication failures",
        ]
        .iter()
        .any(|terminal| error.contains(terminal))
}

fn reserve_loopback_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

async fn authenticated_health(
    client: &reqwest::Client,
    port: u16,
    token: &str,
    expected: &HostDescriptor,
) -> Result<()> {
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client
            .get(format!("http://127.0.0.1:{port}/api/health"))
            .bearer_auth(token)
            .send(),
    )
    .await
    .map_err(|_| anyhow!("Timed out checking the remote OpenResearch process."))??;
    let health = response.json::<serde_json::Value>().await?;
    if health.get("ok").and_then(serde_json::Value::as_bool) != Some(true)
        || health.get("version").and_then(serde_json::Value::as_str)
            != Some(expected.version.as_str())
        || health
            .get("dashboardProtocol")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(DASHBOARD_PROTOCOL))
        || health.get("instanceId").and_then(serde_json::Value::as_str)
            != Some(expected.instance_id.as_str())
    {
        return Err(anyhow!(
            "The remote OpenResearch health response is incompatible."
        ));
    }
    Ok(())
}

fn gateway_router(session: Arc<RemoteSession>) -> Router {
    Router::new()
        .route("/_orx/runtime", get(gateway_runtime))
        .route("/_orx/install", post(gateway_install))
        .route("/_orx/reconnect", post(gateway_reconnect))
        .route("/_orx/start-host", post(gateway_start_new_host))
        .route("/_orx/disconnect", post(gateway_disconnect))
        .route(
            "/_orx/stop-host",
            get(gateway_stop_host_preview).post(gateway_stop_host),
        )
        .fallback(gateway_fallback)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(middleware::from_fn(gateway_loopback_guard))
        .with_state(session)
}

pub(crate) async fn loopback_guard(request: Request, next: Next) -> Response {
    loopback_guard_inner(request, next, true).await
}

async fn gateway_loopback_guard(request: Request, next: Next) -> Response {
    loopback_guard_inner(request, next, false).await
}

async fn loopback_guard_inner(request: Request, next: Next, allow_dev_origin: bool) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());
    let Some(host) = host.filter(|value| loopback_host(value)) else {
        return secure_response(gateway_error(
            StatusCode::BAD_REQUEST,
            "Invalid Host header.".into(),
        ));
    };
    let websocket = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let unsafe_method = !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    if (unsafe_method || websocket)
        && request
            .headers()
            .get("sec-fetch-site")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("cross-site"))
    {
        return secure_response(gateway_error(
            StatusCode::FORBIDDEN,
            "Cross-site requests are not allowed.".into(),
        ));
    }
    if unsafe_method || websocket {
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok());
        let expected = format!("http://{host}");
        let allowed_dev = allow_dev_origin.then(validated_dev_origin).flatten();
        let valid = origin.is_none() && !websocket
            || origin
                .is_some_and(|value| value == expected || allowed_dev.as_deref() == Some(value));
        if !valid {
            return secure_response(gateway_error(
                StatusCode::FORBIDDEN,
                "Invalid Origin header.".into(),
            ));
        }
    }
    secure_response(next.run(request).await)
}

fn secure_response(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
}

fn loopback_host(value: &str) -> bool {
    let host = if value.starts_with('[') {
        value
            .split_once(']')
            .map(|(host, _)| host.trim_start_matches('['))
            .unwrap_or(value)
    } else {
        value.split_once(':').map(|(host, _)| host).unwrap_or(value)
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

async fn gateway_runtime(State(session): State<Arc<RemoteSession>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "kind": "ssh",
        "version": env!("CARGO_PKG_VERSION"),
        "session": session.info.read().await.clone(),
    }))
}

async fn gateway_install(
    State(session): State<Arc<RemoteSession>>,
    Json(paths): Json<RemoteInstallPathsInfo>,
) -> Response {
    match install_and_connect(session, paths).await {
        Ok(info) => Json(serde_json::json!(info)).into_response(),
        Err(error) => gateway_error(StatusCode::BAD_REQUEST, error.to_string()),
    }
}

async fn gateway_reconnect(State(session): State<Arc<RemoteSession>>) -> Response {
    Json(serde_json::json!(reconnect_session(session).await)).into_response()
}

async fn gateway_start_new_host(State(session): State<Arc<RemoteSession>>) -> Response {
    *session.expected_instance.write().await = None;
    {
        let mut info = session.info.write().await;
        info.can_start_new_host = false;
    }
    Json(serde_json::json!(reconnect_session(session).await)).into_response()
}

async fn gateway_disconnect(State(session): State<Arc<RemoteSession>>) -> Response {
    disconnect_session(&session).await;
    Json(serde_json::json!(session.info.read().await.clone())).into_response()
}

async fn gateway_stop_host_preview(State(session): State<Arc<RemoteSession>>) -> Response {
    match remote_stop_context(&session).await {
        Ok((descriptor, preview, _, _)) => Json(serde_json::json!({
            "instanceId": descriptor.instance_id,
            "activeTurnCount": preview.active_turn_count,
            "queuedMessageCount": preview.queued_message_count,
            "pendingPermissionCount": preview.pending_permission_count,
            "activeRunCount": preview.active_run_count,
            "attachmentCount": preview.attachment_count,
        }))
        .into_response(),
        Err(error) => gateway_error(StatusCode::CONFLICT, error.to_string()),
    }
}

async fn gateway_stop_host(
    State(session): State<Arc<RemoteSession>>,
    Json(request): Json<crate::commands::remote_host::StopRequest>,
) -> Response {
    let result = async {
        let (_, _, remote_orx, paths) = remote_stop_context(&session).await?;
        let body = format!("{}\n", serde_json::to_string(&request)?);
        let output = crate::jobs::ssh::ssh_run(
            &session.target,
            &remote_host_cmd(&remote_orx.path, &paths, "stop")?,
            Some(&body),
        )
        .await?;
        parse_control_response(&output)
            .ok_or_else(|| anyhow!("The remote OpenResearch host returned an invalid response."))
    }
    .await;
    match result {
        Ok(ControlResponse::Accepted) => {
            disconnect_session(&session).await;
            session.info.write().await.can_start_new_host = true;
            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "accepted": true })),
            )
                .into_response()
        }
        Ok(ControlResponse::Conflict {
            error,
            descriptor,
            preview,
        }) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": error,
                "instanceId": descriptor.instance_id,
                "activeTurnCount": preview.active_turn_count,
                "queuedMessageCount": preview.queued_message_count,
                "pendingPermissionCount": preview.pending_permission_count,
                "activeRunCount": preview.active_run_count,
                "attachmentCount": preview.attachment_count,
            })),
        )
            .into_response(),
        Ok(ControlResponse::Error { error }) => gateway_error(StatusCode::CONFLICT, error),
        Ok(_) => gateway_error(
            StatusCode::BAD_GATEWAY,
            "Unexpected remote OpenResearch stop response.".into(),
        ),
        Err(error) => gateway_error(StatusCode::BAD_GATEWAY, error.to_string()),
    }
}

async fn remote_stop_context(
    session: &RemoteSession,
) -> Result<(
    HostDescriptor,
    crate::commands::remote_host::StopPreview,
    RemoteOrx,
    RemoteInstallPaths,
)> {
    let host = session.info.read().await.host.clone();
    let paths = remote_install_paths(&session.target, &host).await?;
    let remote_orx = find_remote_orx(&session.target, &host)
        .await?
        .ok_or_else(|| anyhow!("OpenResearch is no longer installed on '{host}'."))?;
    let (descriptor, preview) =
        remote_host_control_status(&session.target, &remote_orx.path, &paths)
            .await?
            .ok_or_else(|| anyhow!("The persistent OpenResearch host is not running."))?;
    Ok((descriptor, preview, remote_orx, paths))
}

fn gateway_error(status: StatusCode, error: String) -> Response {
    (status, Json(serde_json::json!({ "error": error }))).into_response()
}

async fn gateway_fallback(State(session): State<Arc<RemoteSession>>, request: Request) -> Response {
    let path = request.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        let Some(route) = session.route.read().await.clone() else {
            return gateway_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "The remote OpenResearch connection is unavailable.".into(),
            );
        };
        return proxy_request(
            &session.client,
            request,
            &format!("http://127.0.0.1:{}", route.port),
            Some(&route.token),
        )
        .await;
    }
    if let Some(origin) = &session.dev_origin {
        return proxy_request(&session.client, request, origin, None).await;
    }
    crate::commands::up::spa(request.uri().clone()).await
}

async fn proxy_request(
    client: &reqwest::Client,
    mut request: Request,
    origin: &str,
    bearer: Option<&str>,
) -> Response {
    let browser_origin = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|host| format!("http://{host}"))
        .unwrap_or_default();
    let websocket = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"));
    let downstream_upgrade = websocket.then(|| hyper::upgrade::on(&mut request));
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");
    let url = format!("{origin}{path}");
    let method = request.method().clone();
    let (parts, body) = request.with_limited_body().into_parts();
    let mut headers = sanitized_request_headers(parts.headers, origin, bearer);
    if websocket {
        headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));
    }
    let mut upstream = client.request(method, &url).headers(headers);
    if !websocket {
        upstream = upstream.body(reqwest::Body::wrap_stream(body.into_data_stream()));
    }
    let response = match upstream.send().await {
        Ok(response) => response,
        Err(error) => {
            return gateway_error(
                StatusCode::BAD_GATEWAY,
                format!("Remote request failed: {error}"),
            )
        }
    };
    if websocket {
        return proxy_websocket(
            response,
            downstream_upgrade.expect("upgrade present"),
            &browser_origin,
            origin,
        )
        .await;
    }
    proxy_http_response(response, &browser_origin, origin)
}

fn connection_header_names(headers: &HeaderMap) -> Vec<HeaderName> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|value| HeaderName::from_bytes(value.trim().as_bytes()).ok())
        .collect()
}

fn hop_by_hop(name: &HeaderName, nominated: &[HeaderName]) -> bool {
    nominated.iter().any(|candidate| candidate == name)
        || matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "transfer-encoding"
                | "upgrade"
        )
}

fn sanitized_request_headers(source: HeaderMap, origin: &str, bearer: Option<&str>) -> HeaderMap {
    let had_referer = source.contains_key(header::REFERER);
    let nominated = connection_header_names(&source);
    let mut headers = HeaderMap::new();
    for (name, value) in &source {
        if hop_by_hop(name, &nominated)
            || matches!(
                name.as_str(),
                "host"
                    | "authorization"
                    | "cookie"
                    | "forwarded"
                    | "x-forwarded-for"
                    | "x-forwarded-host"
                    | "x-forwarded-proto"
                    | "origin"
                    | "referer"
            )
        {
            continue;
        }
        headers.append(name, value.clone());
    }
    if let Ok(value) = HeaderValue::from_str(origin) {
        headers.insert(header::ORIGIN, value);
    }
    if let Ok(url) = reqwest::Url::parse(origin) {
        if let Some(authority) = url.host_str().map(|host| match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_string(),
        }) {
            if let Ok(value) = HeaderValue::from_str(&authority) {
                headers.insert(header::HOST, value);
            }
        }
    }
    if had_referer {
        if let Ok(value) = HeaderValue::from_str(&format!("{origin}/")) {
            headers.insert(header::REFERER, value);
        }
    }
    if let Some(token) = bearer {
        if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
            headers.insert(header::AUTHORIZATION, value);
        }
    }
    headers
}

fn proxy_http_response(
    response: reqwest::Response,
    browser_origin: &str,
    upstream_origin: &str,
) -> Response {
    let status = response.status();
    let nominated = connection_header_names(response.headers());
    let mut builder = Response::builder().status(status);
    for (name, value) in response.headers() {
        if hop_by_hop(name, &nominated)
            || name == header::SET_COOKIE
            || name.as_str().starts_with("access-control-")
        {
            continue;
        }
        if name == header::LOCATION {
            if let Ok(location) = value.to_str() {
                if let Some(rewritten) =
                    rewrite_loopback_location(location, browser_origin, upstream_origin)
                {
                    builder = builder.header(name, rewritten);
                    continue;
                }
            }
        }
        builder = builder.header(name, value);
    }
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    builder
        .body(Body::from_stream(stream))
        .unwrap_or_else(|error| gateway_error(StatusCode::BAD_GATEWAY, error.to_string()))
}

fn rewrite_loopback_location(
    location: &str,
    browser_origin: &str,
    upstream_origin: &str,
) -> Option<String> {
    let url = reqwest::Url::parse(location).ok()?;
    let upstream = reqwest::Url::parse(upstream_origin).ok()?;
    if url.port_or_known_default() != upstream.port_or_known_default()
        || !url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
    {
        return None;
    }
    let mut rewritten = format!("{}{}", browser_origin, url.path());
    if let Some(query) = url.query() {
        rewritten.push('?');
        rewritten.push_str(query);
    }
    if let Some(fragment) = url.fragment() {
        rewritten.push('#');
        rewritten.push_str(fragment);
    }
    Some(rewritten)
}

async fn proxy_websocket(
    response: reqwest::Response,
    downstream_upgrade: hyper::upgrade::OnUpgrade,
    browser_origin: &str,
    upstream_origin: &str,
) -> Response {
    if response.status() != StatusCode::SWITCHING_PROTOCOLS {
        return proxy_http_response(response, browser_origin, upstream_origin);
    }
    let headers = response.headers().clone();
    let upstream_upgrade = response.upgrade();
    tokio::spawn(async move {
        let (Ok(downstream), Ok(upstream)) = (downstream_upgrade.await, upstream_upgrade.await)
        else {
            return;
        };
        let mut downstream = hyper_util::rt::TokioIo::new(downstream);
        let mut upstream = upstream;
        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
    });
    let mut builder = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    for name in [
        header::CONNECTION,
        header::UPGRADE,
        header::SEC_WEBSOCKET_PROTOCOL,
    ] {
        if let Some(value) = headers.get(&name) {
            builder = builder.header(name, value);
        }
    }
    builder
        .body(Body::empty())
        .unwrap_or_else(|error| gateway_error(StatusCode::BAD_GATEWAY, error.to_string()))
}

pub async fn run(host: &str, args: UpArgs) -> Result<()> {
    let manager = RemoteSessionManager::new();
    eprintln!("orx up --remote: preparing {host}…");
    let (_, session) = manager
        .create(
            host.to_string(),
            RemoteUiPreferences::default(),
            Some(args.port),
        )
        .await?;
    let session = loop {
        let current = manager
            .get(&session.id)
            .await
            .ok_or_else(|| anyhow!("Remote session ended before it started."))?;
        match current.status {
            RemoteSessionStatus::Connected => break current,
            RemoteSessionStatus::NeedsInstall => {
                return Err(anyhow!(
                    "OpenResearch is not installed for {} on {host}. Open the local dashboard to install it with configurable paths.",
                    current.user.as_deref().unwrap_or("the selected user")
                ));
            }
            RemoteSessionStatus::NeedsUpdate => {
                return Err(anyhow!(
                    "OpenResearch on {host} does not support dashboard protocol {DASHBOARD_PROTOCOL}. Update it from the local dashboard."
                ));
            }
            RemoteSessionStatus::Failed => {
                return Err(anyhow!(
                    "Could not open {host}: {}",
                    current
                        .error
                        .as_deref()
                        .unwrap_or("remote connection failed")
                ));
            }
            _ => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    };
    eprintln!("orx up --remote: dashboard on {}", session.gateway_url);
    if !args.no_browser {
        browser::open_browser(&session.gateway_url);
    }
    eprintln!("orx up --remote: press Ctrl-C to stop.");
    let _ = tokio::signal::ctrl_c().await;
    manager.shutdown().await;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteOrx {
    pub path: String,
    pub version: String,
}

const REMOTE_PATH_MARKER: &str = "ORX_REMOTE_PATH=";
const REMOTE_VERSION_MARKER: &str = "ORX_REMOTE_VERSION=";
const REMOTE_INSTALL_PATH_MARKER: &str = "ORX_REMOTE_INSTALL_PATH=";
const REMOTE_DATABASE_PATH_MARKER: &str = "ORX_REMOTE_DATABASE_PATH=";
const REMOTE_CACHE_PATH_MARKER: &str = "ORX_REMOTE_CACHE_PATH=";
const REMOTE_SETTINGS_PATH_MARKER: &str = "ORX_REMOTE_SETTINGS_PATH=";
const REMOTE_USER_MARKER: &str = "ORX_REMOTE_USER=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteInstallPaths {
    pub user: String,
    pub binary: String,
    pub database: String,
    pub cache: String,
    settings: String,
}

impl RemoteInstallPaths {
    pub(crate) fn with_requested(
        mut self,
        binary: String,
        database: String,
        cache: String,
    ) -> Self {
        self.binary = binary;
        self.database = database;
        self.cache = cache;
        self
    }
}

fn parse_remote_orx(output: &str) -> Option<RemoteOrx> {
    let path = output
        .lines()
        .find_map(|line| line.strip_prefix(REMOTE_PATH_MARKER))?
        .to_string();
    if !path.starts_with('/') {
        return None;
    }
    let version = output
        .lines()
        .find_map(|line| line.strip_prefix(REMOTE_VERSION_MARKER))?;
    let version = version.strip_prefix("orx ").unwrap_or(version).to_string();
    (!version.is_empty()).then_some(RemoteOrx { path, version })
}

fn marker_value(output: &str, marker: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(marker))
        .filter(|path| path.starts_with('/'))
        .map(str::to_string)
}

fn marker_text(output: &str, marker: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(marker))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_remote_install_paths(output: &str) -> Option<RemoteInstallPaths> {
    Some(RemoteInstallPaths {
        user: marker_text(output, REMOTE_USER_MARKER)?,
        binary: marker_value(output, REMOTE_INSTALL_PATH_MARKER)?,
        database: marker_value(output, REMOTE_DATABASE_PATH_MARKER)?,
        cache: marker_value(output, REMOTE_CACHE_PATH_MARKER)?,
        settings: marker_value(output, REMOTE_SETTINGS_PATH_MARKER)?,
    })
}

fn storage_root(path: &str, filename: &str, label: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return Err(anyhow!("{label} must be an absolute path without . or .."));
    }
    if path.file_name().and_then(|name| name.to_str()) != Some(filename) {
        return Err(anyhow!("{label} must end in /{filename}"));
    }
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("{label} has no parent directory"))
}

fn validated_roots(paths: &RemoteInstallPaths) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let bin = storage_root(&paths.binary, "orx", "OpenResearch binary")?;
    if bin.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return Err(anyhow!("OpenResearch binary must end in /bin/orx"));
    }
    let cargo = bin
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("OpenResearch binary has no install directory"))?;
    let data = storage_root(&paths.database, "orx.db", "Database")?;
    let cache = storage_root(&paths.cache, "repos", "Repository cache")?;
    Ok((cargo, data, cache))
}

async fn read_remote_settings(
    target: &SshTarget,
    host: &str,
    path: &str,
) -> Result<serde_json::Value> {
    let quoted = crate::jobs::ssh::sh_quote(path);
    let command = remote_orx_cmd(&format!(
        "if [ -f {quoted} ]; then cat {quoted}; else printf '{{}}'; fi"
    ));
    let raw = crate::jobs::ssh::ssh_run(target, &command, None)
        .await
        .map_err(|error| anyhow!("Could not read OpenResearch settings on '{host}': {error}"))?;
    let settings: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|error| anyhow!("OpenResearch settings on '{host}' are invalid: {error}"))?;
    if !settings.is_object() {
        return Err(anyhow!("OpenResearch settings on '{host}' are invalid."));
    }
    Ok(settings)
}

async fn write_remote_json(
    target: &SshTarget,
    host: &str,
    path: &str,
    value: &serde_json::Value,
) -> Result<()> {
    let body = format!("{}\n", serde_json::to_string_pretty(value)?);
    let quoted_path = crate::jobs::ssh::sh_quote(path);
    let parent = Path::new(path)
        .parent()
        .ok_or_else(|| anyhow!("Remote settings path has no parent directory"))?;
    let parent = crate::jobs::ssh::sh_quote(&parent.to_string_lossy());
    let command = remote_orx_cmd(&format!(
        "mkdir -p {parent} && umask 077 && tmp={quoted_path}.tmp.$$ && \
         trap 'rm -f \"$tmp\"' EXIT && cat > \"$tmp\" && chmod 600 \"$tmp\" && \
         mv \"$tmp\" {quoted_path} && trap - EXIT"
    ));
    crate::jobs::ssh::ssh_run(target, &command, Some(&body))
        .await
        .map_err(|error| anyhow!("Could not save OpenResearch settings on '{host}': {error}"))?;
    Ok(())
}

pub(crate) async fn save_remote_install_paths(
    target: &SshTarget,
    host: &str,
    paths: &RemoteInstallPaths,
) -> Result<()> {
    let (_, data, cache) = validated_roots(paths)?;
    let mut settings = read_remote_settings(target, host, &paths.settings).await?;
    let object = settings
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenResearch settings on '{host}' are invalid."))?;
    object.insert("orxBinaryPath".into(), paths.binary.clone().into());
    object.insert("dataDir".into(), data.to_string_lossy().into_owned().into());
    object.insert(
        "cacheDir".into(),
        cache.to_string_lossy().into_owned().into(),
    );
    write_remote_json(target, host, &paths.settings, &settings).await
}

pub(crate) async fn remote_install_paths(
    target: &SshTarget,
    host: &str,
) -> Result<RemoteInstallPaths> {
    let probe = remote_login_orx_cmd(&format!(
        "abs_path() {{ case \"$1\" in /*) printf '%s' \"$1\" ;; *) printf '%s/%s' \"$HOME\" \"$1\" ;; esac; }}; \
         cargo_root=${{CARGO_HOME:-$HOME/.cargo}}; \
         data_root=${{ORX_DATA_DIR:-${{XDG_DATA_HOME:-$HOME/.local/share}}/openresearch}}; \
         cache_root=${{ORX_CACHE_DIR:-$HOME/.cache/openresearch}}; \
         settings_root=${{XDG_CONFIG_HOME:-$HOME/.config}}; \
         printf '{}%s\n{}%s/bin/orx\n{}%s/orx.db\n{}%s/repos\n{}%s/openresearch/settings.json\n' \
         \"$(id -un)\" \"$(abs_path \"$cargo_root\")\" \"$(abs_path \"$data_root\")\" \
         \"$(abs_path \"$cache_root\")\" \"$(abs_path \"$settings_root\")\"",
        REMOTE_USER_MARKER,
        REMOTE_INSTALL_PATH_MARKER,
        REMOTE_DATABASE_PATH_MARKER,
        REMOTE_CACHE_PATH_MARKER,
        REMOTE_SETTINGS_PATH_MARKER,
    ));
    let output = crate::jobs::ssh::ssh_run(target, &probe, None)
        .await
        .map_err(|error| anyhow!("Can't resolve OpenResearch paths on '{host}': {error}"))?;
    let mut paths = parse_remote_install_paths(&output)
        .ok_or_else(|| anyhow!("Could not resolve OpenResearch paths on '{host}'."))?;
    let settings = read_remote_settings(target, host, &paths.settings).await?;
    if let Some(binary) = settings
        .get("orxBinaryPath")
        .and_then(|value| value.as_str())
    {
        if Path::new(binary).is_absolute() {
            paths.binary = binary.to_string();
        }
    }
    if let Some(data) = settings.get("dataDir").and_then(|value| value.as_str()) {
        if Path::new(data).is_absolute() {
            paths.database = Path::new(data)
                .join("orx.db")
                .to_string_lossy()
                .into_owned();
        }
    }
    if let Some(cache) = settings.get("cacheDir").and_then(|value| value.as_str()) {
        if Path::new(cache).is_absolute() {
            paths.cache = Path::new(cache)
                .join("repos")
                .to_string_lossy()
                .into_owned();
        }
    }
    Ok(paths)
}

/// Resolve the authenticated remote user's existing binary once, then launch
/// that exact path so a different non-interactive PATH cannot select another.
pub(crate) async fn find_remote_orx(target: &SshTarget, host: &str) -> Result<Option<RemoteOrx>> {
    let probe = remote_login_orx_cmd(&format!(
        "p=$(command -v orx 2>/dev/null || true); \
         p=$(readlink -f \"$p\" 2>/dev/null || realpath \"$p\" 2>/dev/null || true); \
         if [ -n \"$p\" ] && [ -x \"$p\" ]; then \
         owner=$(find \"$p\" -prune \\( -user \"$(id -u)\" -o -user 0 \\) -print 2>/dev/null); \
         unsafe=$(find \"$p\" \"$(dirname \"$p\")\" -prune -perm -022 -print 2>/dev/null); \
         if [ -n \"$owner\" ] && [ -z \"$unsafe\" ]; then \
         v=$(\"$p\" --version 2>/dev/null || true); \
         printf '{}%s\\n{}%s\\n' \"$p\" \"$v\"; fi; fi",
        REMOTE_PATH_MARKER, REMOTE_VERSION_MARKER
    ));
    let output = crate::jobs::ssh::ssh_run(target, &probe, None)
        .await
        .map_err(|e| anyhow!("Can't reach '{host}' over SSH: {e}"))?;
    if let Some(orx) = parse_remote_orx(&output) {
        return Ok(Some(orx));
    }
    let paths = remote_install_paths(target, host).await?;
    probe_remote_orx_path(target, host, &paths.binary).await
}

async fn probe_remote_orx_path(
    target: &SshTarget,
    host: &str,
    path: &str,
) -> Result<Option<RemoteOrx>> {
    let path = crate::jobs::ssh::sh_quote(path);
    let probe = remote_login_orx_cmd(&format!(
        "p={path}; p=$(readlink -f \"$p\" 2>/dev/null || realpath \"$p\" 2>/dev/null || true); \
         if [ -n \"$p\" ] && [ -x \"$p\" ]; then \
         owner=$(find \"$p\" -prune \\( -user \"$(id -u)\" -o -user 0 \\) -print 2>/dev/null); \
         unsafe=$(find \"$p\" \"$(dirname \"$p\")\" -prune -perm -022 -print 2>/dev/null); \
         if [ -n \"$owner\" ] && [ -z \"$unsafe\" ]; then \
         v=$(\"$p\" --version 2>/dev/null || true); \
         printf '{}%s\\n{}%s\\n' \"$p\" \"$v\"; fi; fi",
        REMOTE_PATH_MARKER, REMOTE_VERSION_MARKER,
    ));
    let output = crate::jobs::ssh::ssh_run(target, &probe, None)
        .await
        .map_err(|error| anyhow!("Can't check OpenResearch on '{host}': {error}"))?;
    Ok(parse_remote_orx(&output))
}

pub(crate) async fn install_remote_orx(
    target: &SshTarget,
    host: &str,
    paths: &RemoteInstallPaths,
) -> Result<RemoteOrx> {
    let (cargo, _, _) = validated_roots(paths)?;
    let cargo = crate::jobs::ssh::sh_quote(&cargo.to_string_lossy());
    let version = env!("CARGO_PKG_VERSION");
    let installer = remote_installer(version);
    let command = remote_login_orx_cmd(&format!("export CARGO_HOME={cargo}; {installer}"));
    crate::jobs::ssh::ssh_run(target, &command, None)
        .await
        .map_err(|error| anyhow!("Could not install OpenResearch on '{host}': {error}"))?;
    probe_remote_orx_path(target, host, &paths.binary)
        .await?
        .ok_or_else(|| {
            anyhow!("The installer finished, but no working `orx` binary was found on '{host}'.")
        })
}

fn remote_installer(version: &str) -> String {
    format!(
        "curl --proto '=https' --tlsv1.2 -LsSf {}/releases/download/v{version}/openresearch-cli-installer.sh | sh",
        crate::updates::REPO_URL
    )
}

/// Turn the `--remote` value into an [`SshTarget`], supporting a trailing
/// `:PORT` so boxes on a non-standard SSH port (RunPod / openresearch dev nodes,
/// reached as `root@1.2.3.4:38455`) work with no `~/.ssh/config` entry.
///
/// - `alias` / `user@host` (no port) → a bare alias: `~/.ssh/config` alone
///   decides everything, exactly as before.
/// - `user@<hostname>:PORT` → `-p PORT` only; the user's own config/known_hosts
///   still govern host-key checking, since a name they typed may be one they've
///   pinned. We must not silently weaken verification for it.
/// - `user@<ip>:PORT` → `-p PORT` plus `StrictHostKeyChecking=accept-new`
///   (trust-on-first-use against the real `known_hosts`). A raw IP is the
///   freshly-provisioned-box case (RunPod/openresearch): nothing is pinned yet,
///   so first-use auto-accept lets `orx up --remote` connect without a prompt,
///   while a later key change is still caught. Caveat: if a provider recycles
///   that IP:port onto a *different* box, the pin now mismatches and ssh refuses
///   until the user runs `ssh-keygen -R`; that loud failure is the safe choice
///   over silently trusting whatever answers.
///
/// A trailing `:PORT` is only recognized when it's `1..=65535` and the address
/// isn't a raw (unbracketed) IPv6 literal — so IPv6 hosts and aliases containing
/// colons are left untouched rather than mis-split into host + bogus port.
fn parse_remote_target(host: &str) -> SshTarget {
    match split_host_port(host) {
        Some((dest, port)) => {
            let policy = if host_is_ip_literal(&dest) {
                HostKeyPolicy::AcceptNew
            } else {
                HostKeyPolicy::UserConfig
            };
            SshTarget::host_port(dest, port, policy)
        }
        None => SshTarget::alias(host),
    }
}

/// `(host, PORT)` when `host` ends in `:<port>` with `port` in `1..=65535` and
/// `host` is not a raw (unbracketed) IPv6 literal. Returns `None` for aliases,
/// bare `user@host`, `host:0`, out-of-range ports, and `2001:db8::1`-style
/// addresses, so those keep their exact prior behavior instead of being
/// mis-parsed into a host + spurious port.
fn split_host_port(host: &str) -> Option<(String, u16)> {
    let (head, tail) = host.rsplit_once(':')?;
    // Reject if the port isn't purely numeric and in range (0 and >65535 are
    // not usable ports, so treat them as "no port here", not a silent -p 0).
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port = tail.parse::<u16>().ok().filter(|&p| p != 0)?;
    // Disambiguate IPv6: a `:` remaining in the host means the split colon was
    // *inside* an address (raw `2001:db8::1`, or an unclosed `[…`), not a port
    // separator. Only a bracketed literal (`[2001:db8::1]`) is a valid host half
    // here — mirroring how ssh requires brackets for an IPv6 host with a port.
    // Check the part *after* any `user@`, so `user@[::1]:2222` isn't mistaken for
    // an unbracketed literal (its `head` starts with `user@`, not `[`).
    let host_only = strip_user(head);
    let bracketed_v6 = host_only.starts_with('[') && host_only.ends_with(']');
    if host_only.contains(':') && !bracketed_v6 {
        return None;
    }
    Some((head.to_string(), port))
}

/// The host portion of a destination, dropping an optional leading `user@`.
fn strip_user(dest: &str) -> &str {
    dest.rsplit_once('@').map_or(dest, |(_, h)| h)
}

/// Whether `dest` (an alias or `user@host`) resolves to a bare IP literal — the
/// signal that this is a freshly-provisioned box (nothing pinned) rather than a
/// hostname the user may already trust. Strips an optional `user@` and matched
/// IPv6 brackets before parsing.
fn host_is_ip_literal(dest: &str) -> bool {
    use std::net::IpAddr;
    let host = strip_user(dest);
    // Strip brackets only as a matched pair, so a malformed one-sided `[…`
    // doesn't parse as an IP.
    let host = match (host.strip_prefix('['), host.strip_suffix(']')) {
        (Some(_), Some(_)) => &host[1..host.len() - 1],
        _ => host,
    };
    host.parse::<IpAddr>().is_ok()
}

/// Dirs prepended to the remote `PATH` so `orx` is found on a non-interactive
/// shell. These are *POSIX shell words* expanded on the remote (`$CARGO_HOME`,
/// `$HOME`), not locally — so they adapt to the remote user and a relocated
/// `CARGO_HOME`, whether that's `root` or `runpod`.
///
/// `${CARGO_HOME:-$HOME/.cargo}/bin` is where our installer lands `orx`
/// (dist-workspace.toml sets `install-path = "CARGO_HOME"`, i.e. `~/.cargo/bin`
/// unless the box overrides `$CARGO_HOME`); `$HOME/.local/bin` covers pip/uv-style
/// drops. The "not installed" error above names these as `~/.cargo/bin` and
/// `~/.local/bin` — keep the two in step.
const REMOTE_ORX_PATH: &str = "${CARGO_HOME:-$HOME/.cargo}/bin:$HOME/.local/bin";

fn remote_host_cmd(path: &str, paths: &RemoteInstallPaths, operation: &str) -> Result<String> {
    let (_, data, cache) = validated_roots(paths)?;
    Ok(remote_orx_cmd(&format!(
        "export ORX_DATA_DIR={} ORX_CACHE_DIR={}; exec {} remote-host {operation}",
        crate::jobs::ssh::sh_quote(&data.to_string_lossy()),
        crate::jobs::ssh::sh_quote(&cache.to_string_lossy()),
        crate::jobs::ssh::sh_quote(path),
    )))
}

fn remote_attach_cmd(path: &str, instance_id: &str, paths: &RemoteInstallPaths) -> Result<String> {
    remote_host_cmd(
        path,
        paths,
        &format!(
            "attach --expected-instance {}",
            crate::jobs::ssh::sh_quote(instance_id)
        ),
    )
}

async fn remote_host_status(
    target: &SshTarget,
    path: &str,
    paths: &RemoteInstallPaths,
) -> Result<Option<HostDescriptor>> {
    Ok(remote_host_control_status(target, path, paths)
        .await?
        .map(|(descriptor, _)| descriptor))
}

async fn remote_host_control_status(
    target: &SshTarget,
    path: &str,
    paths: &RemoteInstallPaths,
) -> Result<Option<(HostDescriptor, crate::commands::remote_host::StopPreview)>> {
    let command = format!(
        "{} 2>/dev/null || true",
        remote_host_cmd(path, paths, "status")?
    );
    let output = crate::jobs::ssh::ssh_run(target, &command, None).await?;
    Ok(
        parse_control_response(&output).and_then(|response| match response {
            ControlResponse::Status {
                descriptor,
                preview,
            } => Some((descriptor, preview)),
            _ => None,
        }),
    )
}

async fn ensure_remote_host(
    target: &SshTarget,
    path: &str,
    paths: &RemoteInstallPaths,
    expected_instance: Option<&str>,
) -> Result<HostDescriptor> {
    let operation = expected_instance.map_or_else(
        || "ensure".to_string(),
        |instance| {
            format!(
                "ensure --expected-instance {}",
                crate::jobs::ssh::sh_quote(instance)
            )
        },
    );
    let output =
        crate::jobs::ssh::ssh_run(target, &remote_host_cmd(path, paths, &operation)?, None).await?;
    output
        .lines()
        .find_map(|line| line.strip_prefix(HOST_MARKER))
        .and_then(|json| serde_json::from_str(json).ok())
        .ok_or_else(|| anyhow!("The remote OpenResearch host returned an invalid descriptor."))
}

fn parse_control_response(output: &str) -> Option<ControlResponse> {
    output
        .lines()
        .find_map(|line| line.strip_prefix(HOST_MARKER))
        .and_then(|json| serde_json::from_str(json).ok())
}

async fn remote_dashboard_protocol(
    target: &SshTarget,
    host: &str,
    path: &str,
) -> Result<Option<u32>> {
    let path = crate::jobs::ssh::sh_quote(path);
    let command = remote_orx_cmd(&format!(
        "{path} version --dashboard-protocol 2>/dev/null || true"
    ));
    let output = crate::jobs::ssh::ssh_run(target, &command, None)
        .await
        .map_err(|error| {
            anyhow!("Could not check OpenResearch compatibility on '{host}': {error}")
        })?;
    Ok(output.lines().find_map(|line| {
        line.strip_prefix("ORX_DASHBOARD_PROTOCOL=")
            .and_then(|value| value.parse().ok())
    }))
}

/// Wrap a remote command so `orx` is found even though ssh runs it in a shell
/// that skips the box's `~/.bashrc`/`~/.profile`, and so it runs under POSIX
/// `sh` regardless of the remote user's login shell.
///
/// Two problems this solves, both hit on real boxes:
/// 1. **PATH.** ssh runs the command non-interactively, so `~/.bashrc`/`~/.profile`
///    aren't sourced; on a minimal image (RunPod's Ubuntu, etc.) `orx`'s install
///    dir isn't on the default `PATH`, so a bare `command -v orx` / `orx up` fails
///    even when `which orx` works when you're logged in. We prepend
///    [`REMOTE_ORX_PATH`].
/// 2. **Login shell.** ssh runs the command under the remote user's *login* shell,
///    which may be csh/tcsh/fish — none of which understand `export VAR=val`. We
///    hand the whole POSIX body to `sh -c` so it's interpreted by `/bin/sh`
///    whatever the login shell is. (This mirrors how the job backends run their
///    payload through an explicit `bash`/`sh`, never the bare login shell.)
///
/// The PATH dirs are left as literal `$…`/`${…}` shell words for the *inner* `sh`
/// to expand on the remote, so they adapt to that box's user and `CARGO_HOME`.
fn remote_orx_cmd(cmd: &str) -> String {
    let body = format!(r#"export PATH="{REMOTE_ORX_PATH}:$PATH"; {cmd}"#);
    // `sh -c '<body>'`: the login shell sees three plain words and execs /bin/sh,
    // which parses the POSIX body. sh_quote wraps the body so its own quotes,
    // `$…`, and `;` reach `sh` intact rather than being eaten by the login shell.
    format!("sh -c {}", crate::jobs::ssh::sh_quote(&body))
}

/// Discover through the account's login environment, then hand the probe back
/// to POSIX sh so custom bash/zsh/fish startup syntax cannot change its meaning.
fn remote_login_orx_cmd(cmd: &str) -> String {
    let body = format!(r#"export PATH="{REMOTE_ORX_PATH}:$PATH"; {cmd}"#);
    let login_command = format!("exec /bin/sh -c {}", crate::jobs::ssh::sh_quote(&body));
    let outer = format!(
        "exec \"${{SHELL:-/bin/sh}}\" -ilc {}",
        crate::jobs::ssh::sh_quote(&login_command)
    );
    format!("sh -c {}", crate::jobs::ssh::sh_quote(&outer))
}

/// The `-L` forward value. Local bind pinned to `127.0.0.1` (see the call site).
pub(crate) fn forward_spec(local_port: u16, remote_port: u16) -> String {
    format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}")
}

/// Build `ssh <opts> -L <forward> -- <dest> <remote_cmd>` over the settings
/// ControlMaster. The session launcher replaces stdin with its credential pipe.
pub(crate) fn ssh_forward_command(
    target: &SshTarget,
    forward: &str,
    remote_cmd: &str,
) -> Result<Command> {
    let mut cmd = Command::new("ssh");
    cmd.args(crate::jobs::ssh::forward_args(target, forward, remote_cmd)?)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    Ok(cmd)
}

pub(crate) fn ssh_spawn_error(error: std::io::Error, purpose: &str) -> crate::error::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow!("`ssh` not found on PATH — {purpose} needs the OpenSSH client.")
    } else {
        anyhow!("Could not run ssh: {error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_paths() -> RemoteInstallPaths {
        RemoteInstallPaths {
            user: "me".into(),
            binary: "/home/me/.cargo/bin/orx".into(),
            database: "/home/me/.local/share/openresearch/orx.db".into(),
            cache: "/scratch/me/openresearch/repos".into(),
            settings: "/home/me/.config/openresearch/settings.json".into(),
        }
    }

    #[test]
    fn forward_and_remote_attach_use_the_daemon_port() {
        assert_eq!(forward_spec(4899, 4900), "127.0.0.1:4899:127.0.0.1:4900");
        let command =
            remote_attach_cmd("/home/me/.cargo/bin/orx", "instance-1", &sample_paths()).unwrap();
        assert!(command.contains("/home/me/.cargo/bin/orx"));
        assert!(command.contains("remote-host attach --expected-instance"));
        assert!(command.contains("instance-1"));
        assert!(command.contains("ORX_DATA_DIR="));
        assert!(command.contains("ORX_CACHE_DIR="));
    }

    #[test]
    fn reconnect_retries_transport_but_not_authentication_failures() {
        assert!(retryable_ssh_transport_error(
            "ssh ini failed (exit 255): Connection timed out"
        ));
        assert!(!retryable_ssh_transport_error(
            "ssh ini failed (exit 255): Permission denied (publickey)"
        ));
        assert!(!retryable_ssh_transport_error(
            "ssh ini failed (exit 1): expected host is no longer running"
        ));
    }

    #[test]
    fn remote_orx_cmd_forces_posix_sh_and_keeps_expansions_literal() {
        let wrapped = remote_orx_cmd("orx up");
        // Runs under an explicit POSIX `sh -c`, not the remote's login shell —
        // so a csh/tcsh/fish login shell can't choke on `export VAR=val`.
        assert!(wrapped.starts_with("sh -c "));
        // The installer dir (CARGO_HOME-aware) and ~/.local/bin are both on PATH,
        // and every `$…`/`${…}` stays LITERAL for the *remote* sh to expand — so
        // it adapts to the box's user (root or `runpod`) and its CARGO_HOME. The
        // presence of the raw tokens proves nothing was expanded on the laptop.
        assert!(wrapped.contains("${CARGO_HOME:-$HOME/.cargo}/bin"));
        assert!(wrapped.contains("$HOME/.local/bin"));
        assert!(wrapped.contains("orx up"));
    }

    #[test]
    fn discovery_probe_is_also_wrapped() {
        let probe = remote_login_orx_cmd("command -v orx");
        assert!(probe.starts_with("sh -c "));
        assert!(probe.contains("${SHELL:-/bin/sh}"));
        assert!(probe.contains("-ilc"));
        assert!(probe.contains("${CARGO_HOME:-$HOME/.cargo}/bin"));
        assert!(probe.contains("command -v orx"));
    }

    #[test]
    fn remote_install_uses_the_release_installer_in_the_login_environment() {
        let command = remote_login_orx_cmd(&remote_installer("1.2.3"));
        assert!(command.contains("openresearch-cli-installer.sh"));
        assert!(command.contains("/releases/download/v1.2.3/"));
        assert!(!command.contains("releases/latest"));
        assert!(command.contains("${CARGO_HOME:-$HOME/.cargo}/bin"));
    }

    #[test]
    fn loopback_hosts_are_narrow() {
        for host in [
            "localhost",
            "localhost:4791",
            "127.0.0.1:4791",
            "[::1]:4791",
        ] {
            assert!(loopback_host(host), "{host}");
        }
        for host in ["example.com", "127.0.0.2.evil.test", "[2001:db8::1]:4791"] {
            assert!(!loopback_host(host), "{host}");
        }
    }

    #[test]
    fn proxy_headers_strip_browser_credentials_and_connection_nominees() {
        let mut source = HeaderMap::new();
        source.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("browser secret"),
        );
        source.insert(header::COOKIE, HeaderValue::from_static("browser cookie"));
        source.insert(header::HOST, HeaderValue::from_static("127.0.0.1:5000"));
        source.insert(header::CONNECTION, HeaderValue::from_static("x-secret"));
        source.insert("x-secret", HeaderValue::from_static("remove me"));
        source.insert(
            header::REFERER,
            HeaderValue::from_static("http://127.0.0.1:5000/project"),
        );

        let headers = sanitized_request_headers(source, "http://127.0.0.1:6000", Some("session"));
        assert_eq!(
            headers.get(header::AUTHORIZATION).unwrap(),
            "Bearer session"
        );
        assert_eq!(
            headers.get(header::ORIGIN).unwrap(),
            "http://127.0.0.1:6000"
        );
        assert_eq!(
            headers.get(header::REFERER).unwrap(),
            "http://127.0.0.1:6000/"
        );
        assert!(!headers.contains_key(header::COOKIE));
        assert_eq!(headers.get(header::HOST).unwrap(), "127.0.0.1:6000");
        assert!(!headers.contains_key("x-secret"));
    }

    #[test]
    fn loopback_redirects_only_rewrite_the_selected_upstream_port() {
        assert_eq!(
            rewrite_loopback_location(
                "http://localhost:6000/project?p=1#files",
                "http://127.0.0.1:5000",
                "http://127.0.0.1:6000",
            )
            .as_deref(),
            Some("http://127.0.0.1:5000/project?p=1#files")
        );
        assert!(rewrite_loopback_location(
            "http://localhost:7000/project",
            "http://127.0.0.1:5000",
            "http://127.0.0.1:6000",
        )
        .is_none());
    }

    #[test]
    fn discovery_ignores_shell_noise_and_returns_the_exact_binary() {
        let found = parse_remote_orx(
            "welcome\nORX_REMOTE_PATH=/srv/users/me/bin/orx\nORX_REMOTE_VERSION=orx 1.2.3\n",
        )
        .unwrap();
        assert_eq!(found.path, "/srv/users/me/bin/orx");
        assert_eq!(found.version, "1.2.3");
    }

    #[test]
    fn install_paths_ignore_shell_noise_and_require_absolute_paths() {
        let paths = parse_remote_install_paths(
            "welcome\nORX_REMOTE_USER=me\n\
             ORX_REMOTE_INSTALL_PATH=/home/me/.cargo/bin/orx\n\
             ORX_REMOTE_DATABASE_PATH=/home/me/.local/share/openresearch/orx.db\n\
             ORX_REMOTE_CACHE_PATH=/home/me/.cache/openresearch/repos\n\
             ORX_REMOTE_SETTINGS_PATH=/home/me/.config/openresearch/settings.json\n",
        )
        .unwrap();
        assert_eq!(paths.user, "me");
        assert_eq!(paths.binary, "/home/me/.cargo/bin/orx");
        assert_eq!(paths.database, "/home/me/.local/share/openresearch/orx.db");
        assert_eq!(paths.cache, "/home/me/.cache/openresearch/repos");

        assert!(parse_remote_install_paths(
            "ORX_REMOTE_USER=me\n\
             ORX_REMOTE_INSTALL_PATH=.cargo/bin/orx\n\
             ORX_REMOTE_DATABASE_PATH=/data/orx.db\n\
             ORX_REMOTE_CACHE_PATH=/cache/repos\n\
             ORX_REMOTE_SETTINGS_PATH=/home/me/.config/openresearch/settings.json\n"
        )
        .is_none());

        assert!(validated_roots(&sample_paths()).is_ok());
        assert!(validated_roots(&sample_paths().with_requested(
            "/home/me/orx".into(),
            "/data/orx.db".into(),
            "/cache/repos".into(),
        ))
        .is_err());
    }

    #[test]
    fn tunnel_reuses_control_master_and_exits_on_forward_failure() {
        let opts = crate::jobs::ssh::forward_args(
            &SshTarget::alias("mybox"),
            "127.0.0.1:7:localhost:7",
            "orx up",
        )
        .unwrap();
        let joined = opts.join(" ");
        assert!(joined.contains("-o ExitOnForwardFailure=yes"));
        assert!(joined.contains("-o BatchMode=yes"));
        assert!(joined.contains("-o ControlMaster=auto"));
        assert!(opts.contains(&"-T".to_string()));
    }

    #[test]
    fn ssh_args_are_ordered_opts_then_forward_then_dest_then_cmd() {
        let target = SshTarget::alias("mybox");
        let args =
            crate::jobs::ssh::forward_args(&target, "127.0.0.1:7:localhost:7", "orx up").unwrap();
        // -L and its value are adjacent and precede the `--` separator.
        let l = args.iter().position(|a| a == "-L").unwrap();
        assert_eq!(args[l + 1], "127.0.0.1:7:localhost:7");
        let sep = args.iter().position(|a| a == "--").unwrap();
        assert!(l < sep, "-L must come before --");
        // dest then the remote command follow the separator, in that order.
        assert_eq!(args[sep + 1], "mybox");
        assert_eq!(args[sep + 2], "orx up");
    }

    #[test]
    fn extra_opts_land_before_the_separator() {
        let target = SshTarget {
            dest: "mybox".into(),
            extra_opts: vec!["-p".into(), "2222".into()],
        };
        let args =
            crate::jobs::ssh::forward_args(&target, "127.0.0.1:7:localhost:7", "orx up").unwrap();
        let sep = args.iter().position(|a| a == "--").unwrap();
        let p = args.iter().position(|a| a == "-p").unwrap();
        assert!(p < sep, "extra_opts must precede the -- separator");
        assert_eq!(args[p + 1], "2222");
    }

    #[test]
    fn bare_alias_and_userhost_stay_plain_aliases() {
        // No port, no imposed opts — `~/.ssh/config` keeps full control.
        for h in ["mybox", "root@example.com"] {
            let t = parse_remote_target(h);
            assert_eq!(t.dest, h);
            assert!(t.extra_opts.is_empty(), "{h} should get no extra opts");
        }
    }

    #[test]
    fn ip_with_port_gets_accept_new_tofu_not_devnull() {
        // The RunPod / openresearch case: root@<ip>:38455 → -p 38455 plus genuine
        // trust-on-first-use (accept-new against real known_hosts). It must NOT
        // use UserKnownHostsFile=/dev/null (that's accept-every-time, reserved
        // for provider-managed boxes).
        let t = parse_remote_target("root@38.128.232.245:38455");
        assert_eq!(t.dest, "root@38.128.232.245");
        let joined = t.extra_opts.join(" ");
        assert!(joined.contains("-p 38455"));
        assert!(joined.contains("StrictHostKeyChecking=accept-new"));
        assert!(
            !joined.contains("/dev/null"),
            "user-typed host must keep its real known_hosts"
        );
        // And it flows all the way into the ssh argv, before the `--`.
        let args = crate::jobs::ssh::forward_args(&t, "127.0.0.1:7:localhost:7", "orx up").unwrap();
        let sep = args.iter().position(|a| a == "--").unwrap();
        let p = args.iter().position(|a| a == "-p").unwrap();
        assert!(p < sep && args[p + 1] == "38455");
        assert_eq!(args[sep + 1], "root@38.128.232.245");
    }

    #[test]
    fn hostname_with_port_gets_dash_p_only_no_hostkey_override() {
        // A *name* the user typed may be one they've pinned — appending :PORT
        // must not silently downgrade host-key verification. Only `-p` is added.
        let t = parse_remote_target("root@example.com:2222");
        assert_eq!(t.dest, "root@example.com");
        assert_eq!(t.extra_opts, vec!["-p".to_string(), "2222".to_string()]);
    }

    #[test]
    fn ipv6_and_odd_trailing_colons_are_not_treated_as_ports() {
        // Bracketed IPv6 without a port: colon is inside the literal.
        assert_eq!(parse_remote_target("[::1]").dest, "[::1]");
        assert!(parse_remote_target("[::1]").extra_opts.is_empty());
        // RAW (unbracketed) IPv6 must NOT be mis-split into host + port.
        assert_eq!(parse_remote_target("2001:db8::5").dest, "2001:db8::5");
        assert!(parse_remote_target("2001:db8::5").extra_opts.is_empty());
        // A trailing colon with no digits, non-numeric, or :0 isn't a port.
        assert!(parse_remote_target("host:").extra_opts.is_empty());
        assert!(parse_remote_target("host:abc").extra_opts.is_empty());
        assert!(parse_remote_target("host:0").extra_opts.is_empty());
        assert_eq!(parse_remote_target("host:0").dest, "host:0");
        // >u16 falls through to a bare alias (ssh will surface the bad host).
        assert!(parse_remote_target("host:99999").extra_opts.is_empty());
        // Bracketed IPv6 *with* a port is honored (and an IP literal → accept-new).
        let t = parse_remote_target("[2001:db8::1]:2222");
        assert_eq!(t.dest, "[2001:db8::1]");
        let joined = t.extra_opts.join(" ");
        assert!(joined.contains("-p 2222"));
        assert!(joined.contains("StrictHostKeyChecking=accept-new"));
    }

    #[test]
    fn user_prefixed_bracketed_ipv6_keeps_its_port() {
        // Regression: `user@[::1]:2222` must NOT drop the port (the bracket check
        // has to look past the `user@` prefix, or it silently connects on :22).
        let t = parse_remote_target("root@[::1]:2222");
        assert_eq!(t.dest, "root@[::1]");
        let joined = t.extra_opts.join(" ");
        assert!(joined.contains("-p 2222"), "port must survive: {joined}");
        // `[::1]` is a loopback IP literal → accept-new TOFU.
        assert!(joined.contains("StrictHostKeyChecking=accept-new"));
    }

    #[test]
    fn host_is_ip_literal_classifies_forms() {
        // IPs in every shape we can be handed → true.
        for d in [
            "1.2.3.4",
            "root@1.2.3.4",
            "[2001:db8::1]",
            "root@[::1]",
            "::ffff:1.2.3.4",
        ] {
            assert!(host_is_ip_literal(d), "{d} should be an IP literal");
        }
        // Hostnames (incl. a numeric-looking one) → false.
        for d in ["example.com", "root@example.com", "1.2.3.4.example.com"] {
            assert!(!host_is_ip_literal(d), "{d} should be a hostname");
        }
        // A malformed one-sided bracket must NOT parse as an IP.
        assert!(!host_is_ip_literal("[1.2.3.4"));
    }
}
