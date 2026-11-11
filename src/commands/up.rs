//! `orx up` — the local autoresearch dashboard server.
//!
//! One axum process on 127.0.0.1 that owns the run engine and serves:
//!   /            embedded SPA (rust-embed over ui/dist, index.html fallback)
//!   /api/*       JSON over the local SQLite store + run-log files
//!   /api/v2/*    the engine's API for the CLI and agents (`api_v2`)
//!   /api/events  SSE: run changes from the engine, a 500ms diff for everything else
//!
//! Fully local: no OpenResearch api anywhere on these paths (the /api/papers
//! routes proxy alphaXiv's public, token-free endpoints — needed because the
//! browser can't call api.alphaxiv.org cross-origin). The normal dashboard is
//! loopback-only; the hidden persistent-host mode also requires a session bearer.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::io::IsTerminal as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine as _;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;

use crate::commands::remote_host::{DashboardLock, HostDescriptor, RemoteAuth};
use crate::commands::up_remote::{self, RemoteSessionStatus};
use crate::error::{anyhow, Result};
use crate::local;
use crate::local::autonomy::Autonomy;
use crate::local::chat::ChatHost;
use crate::local::is_terminal;
use crate::local::opencode::AgentHost;
use crate::store::{
    now_ms, SshHostTest, Store, StoredAgentSelection, StoredChatSession, StoredRun,
};
use crate::updates;
use crate::workspace_state::{GlobalWorkspaceState, WorkspaceState};
use crate::{browser, UpArgs};

mod agent_tools;
pub(crate) mod api_v2;
pub(crate) mod compute_settings;
mod harness_setup;
mod network;
mod search;
use compute_settings::*;

pub async fn run(args: UpArgs) -> Result<()> {
    // orx's own clients reach the server on 127.0.0.1, which both of these binds serve.
    let reach = match args.host {
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) => up_remote::Reach::Loopback,
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED) => up_remote::Reach::Lan,
        _ => {
            return Err(anyhow!(
                "orx up: --host takes 127.0.0.1, or 0.0.0.0 to serve your private network"
            ))
        }
    };
    if !args.desktop_app && !args.remote_host && !args.headless {
        let data_dir = crate::commands::remote_host::canonical_data_dir()?;
        if let Some(server) =
            crate::commands::remote_host::ServerDescriptor::live(&data_dir).await?
        {
            if let up_remote::Reach::Lan = reach {
                return Err(anyhow!(
                    "orx up: already running on port {}; stop it to restart with --host",
                    server.port
                ));
            }
            let url = format!("http://127.0.0.1:{}", server.port);
            eprintln!("orx up: dashboard on {url}");
            if !args.no_browser {
                browser::open_dashboard(&url, crate::telemetry::UpLaunchMode::of(&args));
            }
            return Ok(());
        }
    }
    updates::note_startup_image();
    let port = args.port;
    let persistent_host = args.remote_host;
    // Agent sessions and the CLI present this bearer to the server's API.
    let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    local::chat::set_up_auth_token(token.clone());
    let remote_auth = persistent_host.then(|| RemoteAuth::new(&token));
    let data_dir = crate::commands::remote_host::canonical_data_dir()?;
    // The engine lock: one engine per data directory.
    let dashboard_lock = DashboardLock::acquire(&data_dir)?;
    // A laptop attached to an earlier host may have signed out since.
    if persistent_host {
        if let Err(error) = crate::alphaxiv::forget_forwarded() {
            eprintln!("orx up: could not forget the alphaXiv sign-in forwarded to the previous host: {error}");
        }
    }
    // Blocks only for a relaunched server, whose predecessor still holds the port.
    updates::await_replaced_parent();
    let backend_lock = tokio::task::spawn_blocking(updates::BackendLock::acquire)
        .await
        .ok()
        .flatten();
    // An install that finished while this process waited replaced the image it was started from.
    if backend_lock.is_some() && updates::updated_exe_on_disk().await {
        drop(backend_lock);
        let err = updates::relaunch(port);
        return Err(anyhow!("orx up: could not restart: {err}"));
    }
    let listener = tokio::net::TcpListener::bind((args.host, port))
        .await
        .map_err(|error| {
            anyhow!(
                "Could not bind {}: {}",
                SocketAddr::new(args.host, port),
                error
            )
        })?;
    let actual_port = listener.local_addr()?.port();
    // Open early so the schema exists before any request or agent spawn.
    {
        let store = Store::open()?;
        local::chat::reconcile_unfinished_turns(&store)?;
        // Under the engine lock, before any notice can be produced.
        store.migrate_run_wakeups()?;
    }
    let clock: Arc<dyn crate::engine::clock::Clock> =
        Arc::new(crate::engine::clock::SystemClock::new());
    let engine = Arc::new(crate::engine::Engine::new(Store::open()?, clock.clone()));
    let runtime = crate::engine::runtime::Runtime::new(
        engine.clone(),
        clock.clone(),
        crate::store::data_dir(),
        crate::config::ssh_config(),
    );
    let instance = uuid::Uuid::new_v4().to_string();
    let project_lifecycle = Arc::new(ProjectLifecycle::default());
    let data_dir_move_in_progress = Arc::new(AtomicBool::new(false));
    let stopping = Arc::new(AtomicBool::new(false));
    let api = Arc::new(api_v2::Api::new(
        instance.clone(),
        token.clone(),
        engine.clone(),
        runtime.clone(),
        launch_admission(
            project_lifecycle.clone(),
            data_dir_move_in_progress.clone(),
            stopping.clone(),
        ),
        crate::store::data_dir(),
    )?);
    api_v2::install(api.clone())?;

    // Harnesses spawn lazily on the first message to one of their sessions;
    // no eager agent bring-up. (--no-agent is now a no-op kept for compat.)
    let agent = Arc::new(AgentHost::new(args.model.clone()));
    let codex = Arc::new(local::codex::CodexHost::new());
    let claude = Arc::new(local::claude::ClaudeHost::new());
    claude.start_reaper();
    codex.start_reaper();
    let remote_instance_id = persistent_host.then(|| uuid::Uuid::new_v4().to_string());
    let chat = Arc::new(ChatHost::new(agent.clone(), codex.clone(), claude.clone()));
    let state = AppState {
        agent: agent.clone(),
        chat: chat.clone(),
        claude: claude.clone(),
        harnesses: chat.harnesses.clone(),
        project_lifecycle,
        project_creation_lock: Arc::new(tokio::sync::Mutex::new(())),
        operations: Default::default(),
        publication_locks: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        harness_fill_in_flight: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        claude_catalog_queue: Arc::new(std::sync::Mutex::new(ClaudeCatalogQueue::default())),
        data_dir_move_in_progress,
        data_dir_gate: Arc::new(tokio::sync::Mutex::new(())),
        remote_sessions: crate::commands::up_remote::RemoteSessionManager::new(),
        remote_instance_id: remote_instance_id.clone(),
        stopping: stopping.clone(),
        dashboard_lock: Arc::new(std::sync::Mutex::new(Some(dashboard_lock))),
        restart: Arc::new(tokio::sync::Notify::new()),
        reopen: Arc::default(),
        trackio: Arc::default(),
        opencode_signins: Arc::default(),
    };
    // Plan-mode turns hand this port to the `orx mcp` permission bridge.
    state.chat.set_up_port(actual_port);
    state.chat.resume_persisted_queues();
    {
        let chat = state.chat.clone();
        let moving = state.data_dir_move_in_progress.clone();
        let gate = state.data_dir_gate.clone();
        tokio::spawn(async move {
            let interval =
                crate::config::paced(crate::store::CHAT_TURN_LEASE_TTL + Duration::from_secs(1));
            loop {
                tokio::time::sleep(interval).await;
                if moving.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                let _gate = gate.lock().await;
                if moving.load(std::sync::atomic::Ordering::SeqCst) {
                    continue;
                }
                if let Err(err) = chat.reconcile_expired_turn_leases() {
                    eprintln!("orx up: could not reconcile expired chat turns: {err}");
                }
            }
        });
    }

    spawn_agent_preflight(state.clone());
    spawn_agent_updates(state.clone());
    search::spawn_backfill(state.clone());
    // A fresh install's demo worktree takes ~15 sequential git spawns — build
    // it while the onboarding screen is up instead of inside the confirm click.
    // On Windows those spawns run at idle priority (see `demo::prewarm`) so
    // they only take cores the catalog fill's probes leave free. The data-dir
    // gate keeps it from racing a mid-flight directory move.
    if !persistent_host {
        let move_in_progress = state.data_dir_move_in_progress.clone();
        let gate = state.data_dir_gate.clone();
        tokio::task::spawn_blocking(move || local::demo::prewarm(move_in_progress, gate));
    }
    tokio::spawn(local::chat::keep_turn_leases(
        state.chat.clone(),
        state.data_dir_move_in_progress.clone(),
        state.data_dir_gate.clone(),
    ));
    tokio::spawn(local::chat::watch_run_notices(
        state.chat.clone(),
        engine.bus.subscribe(),
        state.data_dir_move_in_progress.clone(),
        state.data_dir_gate.clone(),
    ));
    runtime.start();
    tokio::spawn(reclaim_orphaned_run_artifacts(
        state.data_dir_move_in_progress.clone(),
        state.data_dir_gate.clone(),
    ));
    tokio::spawn(reclaim_remote_run_dirs(
        state.data_dir_move_in_progress.clone(),
        state.data_dir_gate.clone(),
    ));
    spawn_claude_auth_monitor(
        state.chat.clone(),
        claude.clone(),
        state.harness_fill_in_flight.clone(),
    );
    spawn_background_tasks(remote_auth.is_none());
    if !persistent_host && !args.desktop_app {
        spawn_restart_when_idle(state.clone());
    }
    let live_events = state.chat.clone();
    local::overleaf_live::set_event_sink(Box::new(move |name, data| {
        live_events.emit_event(name, data)
    }));

    let app = router(state.clone(), remote_auth.clone(), reach)
        .into_make_service_with_connect_info::<SocketAddr>();
    let url = format!("http://127.0.0.1:{actual_port}");
    crate::commands::remote_host::ServerDescriptor {
        instance,
        pid: std::process::id(),
        starttime: crate::engine::process::identity(std::process::id())
            .map_or(0, |identity| identity.starttime),
        port: actual_port,
        version: env!("CARGO_PKG_VERSION").into(),
        proto: 2,
        token,
    }
    .publish(&data_dir)?;
    if args.headless {
        tokio::spawn(async move {
            let mut idle = api_v2::Idle::default();
            let started = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                // An unreadable store is not proof of idleness.
                if idle.check(&api, started.elapsed()).unwrap_or(false)
                    && stop_admitting_if_idle(&api).unwrap_or(false)
                {
                    eprintln!("orx up: idle for ten minutes; stopping");
                    SHUTDOWN_REQUESTED.notify_one();
                    return;
                }
            }
        });
    }
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let control_server = if persistent_host {
        Some(
            crate::commands::remote_host::start_control_server(
                HostDescriptor {
                    instance_id: remote_instance_id.clone().expect("persistent instance id"),
                    hostname: crate::commands::remote_host::hostname(),
                    port: actual_port,
                    version: env!("CARGO_PKG_VERSION").into(),
                    dashboard_protocol: crate::commands::up_remote::DASHBOARD_PROTOCOL,
                    control_protocol: crate::commands::remote_host::CONTROL_PROTOCOL,
                },
                remote_auth.clone().expect("persistent remote auth"),
                state.chat.clone(),
                stopping.clone(),
                stop_tx,
            )
            .await?,
        )
    } else {
        None
    };
    // In an SSH session the loopback URL only works on the remote box and there's
    // no local browser to open — print forwarding guidance instead of the bare
    // URL, and skip the (futile) browser-open. Otherwise, today's local flow.
    if let up_remote::Reach::Lan = reach {
        eprintln!(
            "orx up: serving your private network on port {actual_port}; it has no login, so anyone there can use it"
        );
    }
    if let Some(session) = crate::remote::detect_ssh_session() {
        eprint!("{}", session.instructions(actual_port));
    } else {
        eprintln!("orx up: dashboard on {url}");
        if let Some(warning) = crate::local::bash::missing_toolchain() {
            eprintln!("orx up: warning: {warning}");
        }
        if !args.desktop_app && !persistent_host && std::io::stderr().is_terminal() {
            eprintln!(
                "orx up: the OpenResearch desktop app opens these same projects: {}",
                crate::updates::desktop_app_download_url()
            );
        }
        if !args.no_browser {
            browser::open_dashboard(&url, crate::telemetry::UpLaunchMode::of(&args));
        }
    }

    // select! instead of graceful shutdown: open SSE streams never complete,
    // so waiting on connections would hang Ctrl-C forever.
    //
    // We wait on SIGTERM/SIGHUP as well as SIGINT: when this server is started
    // over SSH by `orx up --remote`, closing that tunnel (the launcher's Ctrl-C)
    // delivers SIGHUP here as the channel tears down — without handling it the
    // remote server would leak, staying bound to its port after the tunnel dies.
    let restart = state.restart.clone();
    let mut restarting = false;
    let explicit_stop = if persistent_host {
        tokio::select! {
            r = axum::serve(listener, app) => {
                r.map_err(|e| anyhow!("orx up: server error: {e}"))?;
                false
            }
            changed = stop_rx.changed() => changed.is_ok() && *stop_rx.borrow(),
            _ = restart.notified() => { restarting = true; false }
            _ = persistent_shutdown_signal() => false,
        }
    } else {
        tokio::select! {
            r = axum::serve(listener, app) => r.map_err(|e| anyhow!("orx up: server error: {e}"))?,
            _ = restart.notified() => restarting = true,
            _ = shutdown_signal() => eprintln!("orx up: shutting down"),
        }
        false
    };
    if persistent_host {
        stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // Before agents go down: a turn their shutdown ends must not start the next queued message.
    state.chat.stop_admitting();
    if explicit_stop {
        state.chat.interrupt_all().await;
    }
    state.opencode_signins.shutdown().await;
    state.remote_sessions.shutdown().await;
    agent.shutdown().await;
    codex.shutdown().await;
    claude.shutdown().await;
    state.chat.acp.shutdown().await;
    state.chat.release_turn_leases();
    if let Some(server) = control_server {
        server.shutdown().await;
    }
    state.dashboard_lock.lock().unwrap().take();
    if restarting {
        drop(backend_lock);
        if let Err(error) = crate::commands::remote_host::ServerDescriptor::withdraw(&data_dir) {
            eprintln!(
                "orx up: could not withdraw the server descriptor before restarting: {error:#}"
            );
        }
        if !state.reopen.load(Ordering::SeqCst) && !updates::updated_exe_on_disk().await {
            eprintln!("orx up: installing the update before restarting");
            let failures = updates::failure_count();
            if let Err(err) = updates::apply_now().await {
                eprintln!("orx up: {err} Restarting on this version.");
            }
            // Unless the updater counted it or still will, back off (deferred again, or it
            // never started), or the relaunch restarts straight back into this.
            if !updates::updated_exe_on_disk().await
                && updates::failure_count() == failures
                && !updates::updater_running()
            {
                updates::record_attempt(false);
            }
        }
        eprintln!("orx up: restarting");
        let err = updates::relaunch(actual_port);
        return Err(anyhow!("orx up: could not restart: {err}"));
    }
    Ok(())
}

/// The desktop app's quit. A stored permit covers a quit that lands before the
/// server is waiting for one.
static SHUTDOWN_REQUESTED: tokio::sync::Notify = tokio::sync::Notify::const_new();

#[cfg(desktop_app)]
pub(crate) fn request_shutdown() {
    SHUTDOWN_REQUESTED.notify_one();
}

/// Resolves when the process is asked to stop: SIGINT everywhere; on Unix also
/// SIGTERM and SIGHUP (SIGHUP is what an SSH tunnel delivers on disconnect, so
/// a `--remote`-launched server exits with its tunnel instead of leaking); and
/// [`request_shutdown`].
async fn shutdown_signal() {
    tokio::select! {
        _ = SHUTDOWN_REQUESTED.notified() => {}
        _ = os_shutdown_signal() => {}
    }
}

async fn os_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, Signal, SignalKind};
        // If a signal stream can't be installed, that arm simply never fires —
        // fall back to whatever handlers do register rather than aborting.
        async fn wait(s: &mut Option<Signal>) {
            match s {
                Some(s) => {
                    s.recv().await;
                }
                None => std::future::pending().await,
            }
        }
        let mut term = signal(SignalKind::terminate()).ok();
        let mut hup = signal(SignalKind::hangup()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = wait(&mut term) => {}
            _ = wait(&mut hup) => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

async fn persistent_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, Signal, SignalKind};
        async fn wait(signal: &mut Option<Signal>) {
            match signal {
                Some(signal) => {
                    signal.recv().await;
                }
                None => std::future::pending().await,
            }
        }
        let mut term = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = wait(&mut term) => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[derive(Clone)]
struct AppState {
    agent: Arc<AgentHost>,
    chat: Arc<ChatHost>,
    claude: Arc<local::claude::ClaudeHost>,
    /// Harness detection cache — detection shells out to CLIs, so it's rate-
    /// limited to once per TTL unless the UI asks for a refresh.
    harnesses: Arc<tokio::sync::Mutex<Option<(std::time::Instant, Value)>>>,
    project_lifecycle: Arc<ProjectLifecycle>,
    project_creation_lock: Arc<tokio::sync::Mutex<()>>,
    operations: Operations,
    publication_locks: Arc<tokio::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    /// Single-flight guard for the background catalog fill: without it every
    /// expired read would start its own full detection sweep.
    harness_fill_in_flight: Arc<std::sync::atomic::AtomicBool>,
    claude_catalog_queue: Arc<std::sync::Mutex<ClaudeCatalogQueue>>,
    /// Set while a data-dir move is running. New chat turns and run launches
    /// check it and refuse (409) so nothing starts writing the store mid-move —
    /// closing the window between the move's in-flight check and its completion.
    data_dir_move_in_progress: Arc<std::sync::atomic::AtomicBool>,
    /// Serializes wake-up store writes with a live data-directory move.
    data_dir_gate: Arc<tokio::sync::Mutex<()>>,
    remote_sessions: crate::commands::up_remote::RemoteSessionManager,
    remote_instance_id: Option<String>,
    stopping: Arc<AtomicBool>,
    dashboard_lock: Arc<std::sync::Mutex<Option<DashboardLock>>>,
    /// Fired by `POST /api/update/restart`; the serve loop relaunches on it.
    restart: Arc<tokio::sync::Notify>,
    /// Set with `restart` when the data directory changed: the engine reopens there, and no
    /// update is installed first.
    reopen: Arc<AtomicBool>,
    trackio: Arc<crate::local::trackio::LocalServer>,
    opencode_signins: Arc<local::opencode_signin::SignIns>,
}

async fn project_publication_lock(
    state: &AppState,
    project_id: &str,
) -> tokio::sync::OwnedMutexGuard<()> {
    let lock = {
        let mut locks = state.publication_locks.lock().await;
        locks
            .entry(project_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    lock.lock_owned().await
}

#[derive(Default)]
struct ProjectLifecycle {
    inner: Arc<std::sync::Mutex<ProjectLifecycleState>>,
}

#[derive(Default)]
struct ProjectLifecycleState {
    deleting: HashSet<String>,
    admissions: HashMap<String, usize>,
}

struct ProjectAdmissionLease {
    inner: Arc<std::sync::Mutex<ProjectLifecycleState>>,
    project_id: String,
}

impl Drop for ProjectAdmissionLease {
    fn drop(&mut self) {
        let mut state = self.inner.lock().unwrap();
        if let Some(count) = state.admissions.get_mut(&self.project_id) {
            *count -= 1;
            if *count == 0 {
                state.admissions.remove(&self.project_id);
            }
        }
    }
}

struct ProjectDeletionLease {
    inner: Arc<std::sync::Mutex<ProjectLifecycleState>>,
    project_id: String,
}

impl Drop for ProjectDeletionLease {
    fn drop(&mut self) {
        self.inner.lock().unwrap().deleting.remove(&self.project_id);
    }
}

impl ProjectLifecycle {
    fn admit(&self, project_id: &str) -> Option<ProjectAdmissionLease> {
        let mut state = self.inner.lock().unwrap();
        if state.deleting.contains(project_id)
            || (project_id == "__project_create__" && !state.deleting.is_empty())
        {
            return None;
        }
        *state.admissions.entry(project_id.to_string()).or_default() += 1;
        Some(ProjectAdmissionLease {
            inner: self.inner.clone(),
            project_id: project_id.to_string(),
        })
    }

    fn begin_delete(&self, project_id: &str) -> Option<ProjectDeletionLease> {
        let mut state = self.inner.lock().unwrap();
        if state.deleting.contains(project_id)
            || state
                .admissions
                .get("__project_create__")
                .copied()
                .unwrap_or_default()
                > 0
            || state
                .admissions
                .get(project_id)
                .copied()
                .unwrap_or_default()
                > 0
        {
            return None;
        }
        state.deleting.insert(project_id.to_string());
        Some(ProjectDeletionLease {
            inner: self.inner.clone(),
            project_id: project_id.to_string(),
        })
    }

    fn operation_count(&self) -> usize {
        let state = self.inner.lock().unwrap();
        state.admissions.values().copied().sum::<usize>() + state.deleting.len()
    }
}

/// Admits a launch the way the dashboard admits every project operation.
fn launch_admission(
    lifecycle: Arc<ProjectLifecycle>,
    moving: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
) -> api_v2::Admit {
    Box::new(move |project| {
        if stopping.load(Ordering::SeqCst) {
            return Err(anyhow!("The persistent OpenResearch host is stopping."));
        }
        let admission = lifecycle
            .admit(project)
            .ok_or_else(|| anyhow!("project deletion is in progress"))?;
        // Checked after admitting: a move that raised its flag first counts this admission.
        if moving.load(Ordering::SeqCst) {
            return Err(anyhow!(
                "A data-directory move is in progress. Try again once it finishes."
            ));
        }
        Ok(Box::new(admission))
    })
}

fn router(state: AppState, remote_auth: Option<RemoteAuth>, reach: up_remote::Reach) -> Router {
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/search", get(search::search))
        .route("/api/file-manager", get(file_manager))
        .route("/api/onboarding/complete", post(complete_onboarding))
        .route("/api/project-path/status", get(project_path_status))
        .route("/api/project-path/pick", post(pick_project_folder))
        .route("/api/git/install", post(install_git))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/starter-prompts/prewarm",
            post(prewarm_starter_prompts),
        )
        .route("/api/projects/activity", get(list_project_activity))
        .route(
            "/api/projects/{id}",
            get(get_project)
                .patch(update_project)
                .delete(delete_project),
        )
        .route("/api/projects/{id}/open", post(open_project))
        .route("/api/projects/{id}/git", get(project_git_status))
        .route(
            "/api/projects/{id}/starter-prompts",
            get(project_starter_prompts),
        )
        .route("/api/projects/{id}/git/init", post(initialize_project_git))
        .route("/api/projects/{id}/github", post(enable_project_github))
        .route(
            "/api/projects/{id}/github/disable",
            post(disable_project_github),
        )
        .route("/api/projects/{id}/github/push", post(push_project_github))
        .route("/api/projects/{id}/share", post(share_project))
        .route(
            "/api/projects/{id}/publish",
            get(get_project_link).post(publish_project_link),
        )
        .route("/api/shared-projects/{slug}", get(get_published_project))
        .route("/api/projects/import", post(import_shared_project))
        .route("/api/operations/{id}", get(get_operation))
        .route("/api/github/account", get(github_account))
        .route("/api/github/organizations", get(github_organizations))
        .route(
            "/api/github/project-repo-preview",
            get(github_project_repo_preview),
        )
        .route("/api/github/repo-access", get(github_repo_access))
        .route("/api/projects/{id}/experiments", get(list_experiments))
        .route("/api/projects/{id}/runs", get(list_project_runs))
        .route("/api/papers/search", get(search_papers_api))
        .route("/api/papers/resolve", get(resolve_paper_api))
        .route("/api/compute/backends", get(compute_backends))
        .route("/api/runs", post(create_run))
        .route("/api/runs/{id}", get(get_run))
        .route("/api/instances", get(list_instances))
        .route("/api/runs/{id}/cancel", post(cancel_run))
        .route("/api/runs/{id}/logs", get(run_logs))
        .route("/api/runs/{id}/diff", get(run_diff))
        .route("/api/experiments/{id}/diff", get(experiment_diff))
        .route(
            "/api/experiments/{id}",
            axum::routing::delete(delete_experiment),
        )
        .route(
            "/api/experiments/{id}/archive",
            axum::routing::patch(set_experiment_archive),
        )
        .route("/api/experiments/{id}/commits", get(experiment_commits))
        .route(
            "/api/experiments/{id}/commits/{sha}/diff",
            get(experiment_commit_diff),
        )
        .route("/api/projects/{id}/working-tree", get(project_working_tree))
        .route("/api/projects/{id}/code-tree", get(project_code_tree))
        .route(
            "/api/projects/{id}/file",
            get(project_file)
                .put(write_project_file)
                .patch(manage_project_file),
        )
        .route("/api/projects/{id}/file/raw", get(project_raw_file))
        .route("/api/projects/{id}/file/open", post(open_project_file))
        .route("/api/projects/{id}/file/reveal", post(reveal_project_file))
        .route("/api/projects/{id}/file/latex", post(compile_project_latex))
        .route("/api/latex/engine", get(latex_engine))
        .route(
            "/api/projects/{id}/file/overleaf",
            get(overleaf_link)
                .post(link_overleaf)
                .delete(unlink_overleaf),
        )
        .route("/api/projects/{id}/file/overleaf/sync", post(sync_overleaf))
        .route(
            "/api/projects/{id}/file/overleaf/status",
            get(overleaf_status),
        )
        .route(
            "/api/projects/{id}/file/overleaf/upload",
            get(overleaf_upload),
        )
        .route("/api/overleaf/settings", get(overleaf_settings))
        .route(
            "/api/overleaf/token",
            post(set_overleaf_token).delete(delete_overleaf_token),
        )
        .route(
            "/api/overleaf/session",
            post(set_overleaf_session).delete(delete_overleaf_session),
        )
        .route(
            "/api/overleaf/session/import",
            post(import_overleaf_session),
        )
        .route(
            "/api/projects/{id}/file/overleaf/live",
            post(start_overleaf_live).delete(stop_overleaf_live),
        )
        .route("/api/files/abs", get(absolute_file))
        .route("/api/files/abs/raw", get(absolute_raw_file))
        .route(
            "/api/settings/ssh/config",
            get(ssh_config).put(save_ssh_config),
        )
        .route(
            "/api/projects/{id}/files",
            get(list_artifacts)
                .patch(manage_artifact_file)
                .delete(delete_artifact),
        )
        .route("/api/projects/{id}/files/file", get(serve_artifact))
        .route(
            "/api/projects/{id}/files/reveal",
            post(reveal_artifact_file),
        )
        .route("/api/projects/{id}/terminal", get(project_terminal))
        .route("/api/events", get(events))
        .route(
            "/api/settings/run-engine",
            get(run_engine_settings).post(set_run_engine),
        )
        .route("/api/settings/hf", get(hf_settings).post(set_hf_token))
        .route(
            "/api/settings/tinker",
            get(tinker_settings).post(set_tinker_key),
        )
        .route(
            "/api/settings/k8s",
            get(k8s_settings).post(set_k8s_settings),
        )
        .route(
            "/api/settings/modal",
            get(modal_settings).post(set_modal_token),
        )
        .route(
            "/api/settings/vercel",
            get(vercel_settings).post(set_vercel),
        )
        .route("/api/settings/vercel/scopes", post(vercel_scopes_route))
        .route("/api/settings/env", get(env_settings).post(set_env_var))
        .route(
            "/api/settings/env/{key}",
            axum::routing::delete(delete_env_var),
        )
        .route(
            "/api/settings/data-dir",
            get(data_dir_settings).post(set_data_dir),
        )
        .route("/api/settings/data-dir/validate", post(validate_data_dir))
        .route("/api/settings/data-dir/move", post(move_data_dir))
        .route(
            "/api/settings/git",
            get(git_settings).post(set_git_settings),
        )
        .route(
            "/api/settings/projects",
            get(project_defaults).post(set_project_defaults),
        )
        .route(
            "/api/settings/telemetry",
            get(telemetry_settings).post(set_telemetry_settings),
        )
        .route("/api/telemetry/event", post(record_ui_event))
        .route("/api/telemetry/locale", post(set_dashboard_locale))
        .route(
            "/api/settings/profile",
            get(profile_settings).post(set_profile_settings),
        )
        .route("/api/pending-work", get(pending_work))
        .route("/api/update", get(update_status))
        .route("/api/update/notes", get(update_notes))
        .route("/api/update/apply", post(apply_update))
        .route("/api/update/restart", post(restart_after_update))
        .route("/api/update/auto", post(set_auto_update))
        .route("/api/update/channel", post(set_update_channel))
        .route("/api/update/install-cli", post(install_cli))
        .route("/api/settings/ui-state", get(ui_state).post(set_ui_state))
        .route("/api/settings/network", get(network::public_network))
        .route(
            "/api/projects/{id}/ui-state",
            get(project_ui_state).post(set_project_ui_state),
        )
        .route(
            "/api/settings/ssh",
            get(ssh_settings).post(save_ssh_settings),
        )
        .route("/api/settings/ssh/default", post(save_ssh_default))
        .route("/api/settings/ssh/master", get(ssh_master_status))
        .route("/api/settings/ssh/preflight", post(ssh_preflight))
        .route("/api/settings/ssh/connect", get(ssh_connect))
        .route(
            "/api/remote/sessions",
            get(remote_sessions).post(create_remote_session),
        )
        .route("/api/remote/sessions/{id}", get(remote_session))
        .route(
            "/api/remote/sessions/{id}/reconnect",
            post(reconnect_remote_session),
        )
        .route(
            "/api/remote/sessions/{id}/disconnect",
            post(disconnect_remote_session),
        )
        .route("/_orx/runtime", get(local_runtime))
        .route(
            "/_orx/workspaces/{id}/{*path}",
            axum::routing::any(remote_workspace),
        )
        .route(
            "/api/settings/slurm",
            get(slurm_settings).post(set_slurm_settings),
        )
        .route("/api/settings/slurm/preflight", post(slurm_preflight))
        .route(
            "/api/settings/ray",
            get(ray_settings).post(set_ray_settings),
        )
        .route("/api/settings/ray/preflight", post(ray_preflight))
        .route("/api/trackio/projects/{id}/runs", get(trackio_project_runs))
        .route("/api/settings/trackio/preflight", post(trackio_preflight))
        .route("/api/settings/compute", get(compute_settings))
        .route("/api/settings/compute/default", post(set_compute_default))
        .route("/api/settings/local", get(local_machine_settings))
        .route("/api/settings/openresearch", get(openresearch_settings))
        .route("/api/account/openresearch", get(openresearch_account))
        .route(
            "/api/settings/openresearch/logout",
            post(openresearch_logout),
        )
        .route("/api/settings/openresearch/login", get(openresearch_login))
        .route("/api/settings/alphaxiv/login", get(alphaxiv_login))
        .route("/api/alphaxiv/me", get(alphaxiv_me))
        .route("/api/alphaxiv/folders", get(alphaxiv_folders))
        .route("/api/alphaxiv/papers", get(alphaxiv_papers))
        .route("/api/settings/commands/run", get(run_settings_command))
        .route(
            "/api/settings/openresearch/ssh-key",
            post(add_openresearch_ssh_key),
        )
        .route(
            "/api/settings/lit-sources",
            get(lit_sources_settings).post(set_lit_sources_settings),
        )
        .route("/api/harnesses", get(list_harnesses))
        .route("/api/agents/{key}", axum::routing::patch(edit_agent))
        .route("/api/agent-profiles", post(create_agent_profile))
        .route(
            "/api/agent-profiles/{id}",
            axum::routing::delete(delete_agent_profile),
        )
        .route(
            "/api/harnesses/acp",
            get(list_acp_harnesses).post(create_acp_harness),
        )
        .route("/api/harnesses/acp/test", post(test_acp_harness))
        .route("/api/harnesses/acp/presets", get(list_acp_presets))
        .route(
            "/api/harnesses/acp/{id}",
            axum::routing::put(update_acp_harness).delete(delete_acp_harness),
        )
        .route("/api/harnesses/{id}/snapshot", get(harness_snapshot))
        .route(
            "/api/harnesses/{id}/install",
            axum::routing::delete(remove_harness_install),
        )
        .route(
            "/api/harnesses/setup/commands",
            get(harness_setup::commands),
        )
        .route("/api/harnesses/setup", get(harness_setup::connect))
        .route(
            "/api/local-models",
            get(list_local_models).post(connect_local_model),
        )
        .route("/api/local-models/discover", post(discover_local_models))
        .route("/api/local-models/{id}/check", post(check_local_model))
        .route(
            "/api/local-models/{id}",
            axum::routing::delete(remove_local_model),
        )
        .route(
            "/api/opencode/providers",
            get(list_opencode_providers).post(add_opencode_provider),
        )
        .route(
            "/api/opencode/providers/{id}",
            axum::routing::delete(remove_opencode_provider),
        )
        .route(
            "/api/opencode/providers/{id}/key",
            axum::routing::put(replace_opencode_key),
        )
        .route(
            "/api/opencode/integrations",
            get(list_opencode_integrations),
        )
        .route("/api/opencode/sign-in", post(start_opencode_signin))
        .route(
            "/api/opencode/sign-in/{attempt}",
            get(opencode_signin_status).delete(cancel_opencode_signin),
        )
        .route(
            "/api/opencode/sign-in/{attempt}/code",
            post(opencode_signin_code),
        )
        .route("/api/skills", get(list_skills))
        .route("/api/skills/{name}", get(get_skill))
        .route(
            "/api/user-skills",
            get(list_user_skills)
                .post(upload_user_skill)
                .delete(delete_user_skill),
        )
        .route(
            "/api/user-skills/{name}",
            get(get_user_skill).put(update_user_skill),
        )
        .route(
            "/api/latex-templates",
            get(list_latex_templates)
                .post(upload_latex_template)
                .delete(delete_latex_template),
        )
        .route(
            "/api/chat/sessions",
            get(list_chat_sessions).post(create_chat_session),
        )
        .route(
            "/api/chat/sessions/{id}",
            get(get_chat_session)
                .delete(delete_chat_session)
                .patch(update_chat_session),
        )
        .route("/api/chat/sessions/{id}/messages", get(chat_messages))
        .route("/api/chat/messages/{cited}", get(cited_chat_message))
        .route(
            "/api/chat/sessions/{id}/submissions/{client_id}",
            get(chat_submission),
        )
        .route(
            "/api/chat/sessions/{id}/experiments",
            get(chat_session_experiments),
        )
        .route("/api/chat/sessions/{id}/subagent", get(chat_subagent))
        .route(
            "/api/chat/sessions/{id}/configuration",
            post(set_chat_configuration),
        )
        .route("/api/chat/sessions/{id}/worktree", get(session_worktree))
        .route("/api/chat/sessions/{id}/message", post(send_chat_message))
        .route("/api/chat/sessions/{id}/shell", post(run_shell_command))
        .route(
            "/api/chat/sessions/{id}/compact",
            post(compact_chat_session),
        )
        .route("/api/chat/active-runs", get(chat_active_runs))
        .route("/api/chat/native-sessions", get(list_native_chats))
        .route("/api/chat/native-sessions/import", post(import_native_chat))
        .route(
            "/api/chat/sessions/{id}/turns/{turnId}/recover",
            post(recover_chat_turn),
        )
        .route("/api/chat/sessions/{id}/side", post(open_side_chat))
        .route("/api/chat/sessions/{id}/fork", post(fork_chat_turn))
        .route("/api/chat/sessions/{id}/branch", post(select_chat_branch))
        .route("/api/chat/sessions/{id}/interrupt", post(interrupt_chat))
        .route(
            "/api/chat/sessions/{id}/queue/{itemId}",
            axum::routing::delete(cancel_queued_chat).post(retry_queued_chat),
        )
        .route("/api/chat/sessions/{id}/respond", post(respond_chat))
        // Internal: the `orx mcp` permission bridge's long-poll (plan
        // mode). Token-authenticated in the handler; blocks until the surfaced
        // card is answered.
        .route("/api/internal/permissions", post(bridge_permission))
        // Internal: the orchestration tools `orx mcp` relays for an agent,
        // authenticated in the handler by that chat's agent token.
        .route("/api/internal/mcp", post(agent_tools::agent_tools))
        .route("/api/chat/attachments/{name}", get(chat_attachment))
        .route("/api/agent/status", get(agent_status))
        .fallback(spa)
        // Chat attachments (PDFs, images) ride as base64 in the send-message
        // JSON body; the 2 MB axum default rejects any real paper. Cap it well
        // above the client-side per-file limit so a full message still fits.
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(state);
    server_router(app.merge(api_v2::router()), remote_auth, reach)
}

fn server_router(app: Router, remote_auth: Option<RemoteAuth>, reach: up_remote::Reach) -> Router {
    // Outside `/api`, so no gateway forwards a browser's request to it.
    let forwarded_alphaxiv = axum::routing::put(keep_forwarded_alphaxiv)
        .delete(forget_forwarded_alphaxiv)
        .layer(DefaultBodyLimit::max(16 * 1024));
    let app = match remote_auth {
        Some(_) => app.route("/_orx/alphaxiv", forwarded_alphaxiv),
        None => app,
    };
    let app = app
        .layer(middleware::from_fn(track_active))
        .layer(middleware::from_fn_with_state(
            reach,
            up_remote::dashboard_guard,
        ));
    match remote_auth {
        Some(auth) => app.layer(middleware::from_fn_with_state(auth, require_remote_auth)),
        None => app,
    }
}

async fn require_remote_auth(
    State(auth): State<RemoteAuth>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if remote_route_forbidden(path) {
        return ApiError(
            StatusCode::FORBIDDEN,
            "This action is unavailable in an SSH workspace.".into(),
        )
        .into_response();
    }
    // V2 checks its descriptor credential at the route layer, including the hello challenge.
    if path.starts_with("/api/v2/")
        || path == "/api/internal/permissions"
        || path == "/api/internal/mcp"
    {
        return next.run(request).await;
    }
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|value| Sha256::digest(value.as_bytes()));
    let session = provided
        .as_ref()
        .is_some_and(|digest| auth.matches_attachment(digest));
    let callback = is_remote_callback_route(request.method(), path)
        && provided
            .as_ref()
            .is_some_and(|digest| auth.matches_callback(digest));
    if session || callback {
        next.run(request).await
    } else {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Remote session authentication required.".into(),
        )
        .into_response()
    }
}

/// An attached laptop's alphaXiv access token, for this host's agents and CLI.
async fn keep_forwarded_alphaxiv(body: axum::body::Bytes) -> Result<StatusCode, ApiError> {
    crate::alphaxiv::keep_forwarded(&body).map_err(|error| {
        match error.downcast_ref::<crate::alphaxiv::HttpFailure>() {
            Some(failure) => ApiError(failure.status, failure.message.clone()),
            None => error.into(),
        }
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn forget_forwarded_alphaxiv() -> Result<StatusCode, ApiError> {
    crate::alphaxiv::forget_forwarded()?;
    Ok(StatusCode::NO_CONTENT)
}

fn remote_route_forbidden(path: &str) -> bool {
    matches!(
        path,
        "/api/project-path/pick"
            | "/api/git/install"
            | "/api/update"
            | "/api/update/apply"
            | "/api/update/restart"
            | "/api/update/auto"
            | "/api/update/channel"
            | "/api/update/install-cli"
            | "/api/settings/data-dir"
            | "/api/settings/data-dir/move"
            | "/api/settings/openresearch/ssh-key"
            | "/api/settings/openresearch/login"
            | "/api/settings/openresearch/logout"
            | "/api/settings/alphaxiv/login"
            | "/api/alphaxiv/me"
            | "/api/alphaxiv/folders"
            | "/api/alphaxiv/papers"
            | "/api/settings/commands/run"
    ) || path.starts_with("/api/remote/")
        || (path.starts_with("/api/projects/")
            && (path.ends_with("/file/open")
                || path.ends_with("/file/reveal")
                || path.ends_with("/files/reveal")))
}

fn is_remote_callback_route(method: &Method, path: &str) -> bool {
    if method == Method::GET {
        // Older `orx agent spawn` clients' install preflight (read-only).
        return path
            .strip_prefix("/api/harnesses/")
            .and_then(|path| path.strip_suffix("/snapshot"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'));
    }
    if method != Method::POST {
        return false;
    }
    path == "/api/runs"
        || path
            .strip_prefix("/api/runs/")
            .and_then(|path| path.strip_suffix("/cancel"))
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
}

// --- error plumbing -------------------------------------------------------

/// JSON error responses: `{"error": "..."}` with an explicit status.
#[derive(Debug)]
pub(crate) struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<crate::error::Error> for ApiError {
    fn from(err: crate::error::Error) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

fn bad_request(err: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, err.to_string())
}

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

type ApiResult = std::result::Result<Json<Value>, ApiError>;

// --- wire types -----------------------------------------------------------

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ApiRun {
    id: String,
    experiment_id: String,
    project_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    commit_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result_markdown: Option<String>,
    created_at: i64,
    updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    started_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ended_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i64>,
    contact: Option<crate::store::Contact>,
    cancelling: bool,
    cancel_unconfirmed: Option<Vec<u32>>,
    waiting_reason: Option<String>,
    lost_reason: Option<String>,
    host: Option<String>,
    notice_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_collection: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    log_collection_error: Option<String>,
}

pub(crate) fn run_host(store: &Store, run: &StoredRun) -> Result<Option<String>> {
    use crate::engine::handle::Handle;
    Ok(match crate::engine::handle_of(run) {
        Some(Handle::Ssh { host, .. } | Handle::Slurm { host, .. }) => Some(host),
        Some(Handle::Legacy { descriptor })
            if matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job") =>
        {
            descriptor.namespace
        }
        _ => store
            .engine_run_state(&run.id)?
            .and_then(|state| state.launch)
            .filter(|launch| matches!(launch.kind.as_str(), "ssh_job" | "slurm_job"))
            .and_then(|launch| launch.target),
    })
}

impl ApiRun {
    fn new(store: &Store, run: &StoredRun) -> Result<Self> {
        let state = store.engine_run_state(&run.id)?;
        let state = state.filter(|_| crate::engine::is_engine_row(run));
        let accepted = state
            .as_ref()
            .is_some_and(|state| state.cancel_accepted_at.is_some());
        let launch = state.as_ref().and_then(|state| state.launch.as_ref());
        let collection = store.log_collection(&run.id)?;
        Ok(Self {
            id: run.id.clone(),
            experiment_id: run.experiment_id.clone(),
            project_id: run.project_id.clone(),
            status: run.status.clone(),
            backend: crate::store::public_backend(&run.backend_json, launch),
            command: Some(run.command.clone()).filter(|c| !c.is_empty()),
            commit_sha: run.commit_sha.clone(),
            result_markdown: run.result_markdown.clone(),
            created_at: run.created_at,
            updated_at: run.updated_at,
            started_at: state.as_ref().and_then(|state| state.started_at),
            ended_at: run.ended_at,
            exit_code: run.exit_code,
            cancel_unconfirmed: state
                .as_ref()
                .and_then(|state| state.cancel_unconfirmed.clone()),
            waiting_reason: state
                .as_ref()
                .and_then(|state| state.waiting_reason.clone()),
            contact: state
                .map(|state| state.contact)
                .filter(|contact| *contact != crate::store::Contact::Live),
            cancelling: !is_terminal(&run.status) && (run.cancel_requested || accepted),
            lost_reason: run
                .result_markdown
                .as_ref()
                .filter(|_| run.status == "lost")
                .map(|reason| {
                    if reason == crate::engine::reconcile::ABANDONED {
                        "abandoned".into()
                    } else {
                        reason.clone()
                    }
                }),
            host: run_host(store, run)?,
            notice_error: store.run_notice_error(&run.id)?,
            log_collection: collection
                .as_ref()
                .map(|(failed, _)| if *failed { "failed" } else { "pending" }),
            log_collection_error: collection.and_then(|(_, error)| error),
        })
    }
}

// --- basic routes ---------------------------------------------------------

async fn health(State(state): State<AppState>) -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "dashboardProtocol": crate::commands::up_remote::DASHBOARD_PROTOCOL,
        "instanceId": state.remote_instance_id,
    }))
}

async fn file_manager(
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    let locale = query.get("locale").cloned().unwrap_or_default();
    let name = tokio::task::spawn_blocking(move || crate::editors::file_manager_name(&locale))
        .await
        .ok()
        .flatten();
    Json(json!({ "os": std::env::consts::OS, "name": name }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteOnboardingReq {
    profile: Option<String>,
    harness: String,
    model: Option<String>,
    permission_mode: Option<String>,
    reasoning_level: Option<String>,
    #[serde(default)]
    research_areas: Vec<String>,
    #[serde(default)]
    other_area: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    papers: Vec<crate::telemetry::ProfilePaper>,
}

const RESEARCH_AREAS: [&str; 4] = ["AI/ML", "Biology", "Physics", "Other"];

fn preferred_permission_mode(harness: &str, mode: Option<String>) -> Option<String> {
    match mode.as_deref() {
        Some("plan") => local::harness::effective_permission_id(harness, None),
        _ => mode,
    }
}

fn normalize_research_profile(
    research_areas: Vec<String>,
    other_area: Option<String>,
    background: Option<String>,
    papers: Vec<crate::telemetry::ProfilePaper>,
) -> std::result::Result<crate::telemetry::ResearchProfile, ApiError> {
    let mut normalized_areas = Vec::with_capacity(research_areas.len());
    for area in research_areas {
        let area = area.trim().to_string();
        if !RESEARCH_AREAS.contains(&area.as_str()) {
            return Err(bad_request(format!("unknown research area: {area}")));
        }
        if normalized_areas.contains(&area) {
            return Err(bad_request(format!("duplicate research area: {area}")));
        }
        normalized_areas.push(area);
    }
    let other_area = other_area.filter(|value| !value.trim().is_empty());
    let includes_other = normalized_areas.iter().any(|area| area == "Other");
    if includes_other && other_area.is_none() {
        return Err(bad_request(
            "describe your research area when choosing Other",
        ));
    }
    if !includes_other && other_area.is_some() {
        return Err(bad_request(
            "choose Other before describing another research area",
        ));
    }
    Ok(crate::telemetry::ResearchProfile {
        research_areas: normalized_areas,
        other_area,
        background: background.filter(|value| !value.trim().is_empty()),
        papers,
    })
}

async fn complete_onboarding(
    State(state): State<AppState>,
    Json(req): Json<CompleteOnboardingReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    if !local::harness::is_chat_harness(&req.harness) {
        return Err(bad_request(format!("unknown agent: {}", req.harness)));
    }
    let nonempty = |value: Option<String>| value.filter(|item| !item.trim().is_empty());
    let permission_mode = nonempty(req.permission_mode);
    if permission_mode
        .as_deref()
        .is_some_and(|mode| local::harness::permission_mode_for(&req.harness, mode).is_none())
    {
        return Err(bad_request(
            "invalid permission mode for the selected agent",
        ));
    }
    if let Some(profile) = &req.profile {
        local::agent_profiles::home(profile, &req.harness).map_err(bad_request)?;
    }
    let model = selected_chat_model(
        &state,
        &req.harness,
        req.profile.as_deref(),
        req.model.as_deref(),
    )
    .await
    .map_err(bad_request)?;
    let selection = local::demo::DemoSelection {
        profile: req.profile,
        harness: req.harness,
        model,
        permission_mode,
        reasoning_level: nonempty(req.reasoning_level),
    };
    // A request without areas carries no profile and must not erase a saved one.
    let profile = (!req.research_areas.is_empty())
        .then(|| {
            normalize_research_profile(
                req.research_areas,
                req.other_area,
                req.background,
                req.papers,
            )
        })
        .transpose()?;
    let profile_for_event = profile.clone();
    let completion = tokio::task::spawn_blocking(move || -> Result<_> {
        if let Some(profile) = profile {
            let _ = crate::telemetry::set_profile(profile);
        }
        let completion = local::demo::complete_onboarding(selection)?;
        let store = Store::open()?;
        store.set_preferred_agent(&StoredAgentSelection {
            profile: completion.selection.profile.clone(),
            harness: completion.selection.harness.clone(),
            model: completion.selection.model.clone(),
            service_tier: None,
            permission_mode: preferred_permission_mode(
                &completion.selection.harness,
                completion.selection.permission_mode.clone(),
            ),
            reasoning_level: completion.selection.reasoning_level.clone(),
        })?;
        store.set_onboarding_completed(true)?;
        Ok(completion)
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("demo seed task failed: {e}")))??;
    if completion.newly_created {
        crate::telemetry::capture_onboarding_completed();
        if let Some(profile) = &profile_for_event {
            crate::telemetry::capture_onboarding_research_profile(profile);
        }
    }
    Ok(Json(json!({
        "project": project_json(&completion.project),
        "selection": completion.selection,
    })))
}

#[derive(Deserialize)]
struct SkillsQ {
    harness: Option<String>,
    /// The open project, so a built-in skill's instructions can account for it.
    project: Option<String>,
}

/// Slash-skills the composer's `/` dropdown offers (expanded server-side): the
/// built-in catalog, personal skills, and skills in the open project.
async fn list_skills(Query(q): Query<SkillsQ>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let project = q.project.as_deref().and_then(|id| {
            Store::open().ok()?.get_local_project(id).ok().flatten()
        });
        let project_skills = project
            .as_ref()
            .map(|project| {
                crate::local::user_skills::list_project_skills(
                    std::path::Path::new(&project.repo_path),
                    q.harness.as_deref(),
                )
            })
            .unwrap_or_default();
        let project_names: HashSet<_> = project_skills.iter().map(|skill| &skill.name).collect();
        let mut skills: Vec<Value> = crate::local::skills::CATALOG
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "description": s.description,
                    "source": "builtin",
                })
            })
            .collect();
        for s in crate::local::user_skills::list_for_harness(q.harness.as_deref()) {
            skills.push(json!({
                "name": if project_names.contains(&s.name) { format!("{}@u", s.name) } else { s.name },
                "description": s.description,
                "source": "user",
                "plugin": s.plugin,
                "harness": q.harness,
            }));
        }
        for s in project_skills {
            skills.push(json!({
                "name": format!("{}@p", s.name),
                "description": s.description,
                "source": "project",
                "harness": q.harness,
            }));
        }
        Json(json!({ "skills": skills }))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!(e)))
}

async fn get_skill(Path(name): Path<String>, Query(q): Query<SkillsQ>) -> ApiResult {
    if crate::local::user_skills::parse_selection(&name).is_none() {
        return Err(bad_request("invalid skill name"));
    }
    let github_enabled = if let Some(project_id) = q.project.as_deref() {
        Store::open()
            .ok()
            .and_then(|store| store.get_local_project(project_id).ok().flatten())
            .is_some_and(|project| project.github_enabled())
    } else {
        false
    };
    if let Some(content) = crate::local::skills::instructions(&name, false, github_enabled) {
        return Ok(Json(json!({ "name": name, "content": content })));
    }
    let skill_name = name.clone();
    let project_repo = q
        .project
        .as_deref()
        .and_then(|id| Store::open().ok()?.get_local_project(id).ok().flatten())
        .map(|project| project.repo_path);
    let content = tokio::task::spawn_blocking(move || {
        crate::local::user_skills::content(
            &skill_name,
            q.harness.as_deref(),
            project_repo.as_deref().map(std::path::Path::new),
        )
    })
    .await
    .map_err(|e| ApiError::from(anyhow!(e)))?
    .ok_or_else(|| not_found("skill"))?;
    Ok(Json(json!({ "name": name, "content": content })))
}

// --- user skills --------------------------------------------------------------

fn user_skill_json(s: &crate::local::user_skills::UserSkill) -> Value {
    json!({
        "name": s.name,
        "origin": s.origin,
        "bytes": s.bytes,
        "updatedAt": s.updated_at,
    })
}

/// Everything the Customize tab lists: uploads plus the skills mirrored from the
/// coding agents installed on this machine.
async fn list_user_skills() -> ApiResult {
    tokio::task::spawn_blocking(|| {
        let (skills, discovery) = crate::local::user_skills::list();
        let skills: Vec<Value> = skills.iter().map(user_skill_json).collect();
        Json(json!({ "skills": skills, "discovery": discovery }))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!(e)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadSkillReq {
    /// Original upload filename — its extension picks `.zip` vs single file.
    filename: String,
    /// The file bytes, base64 (same convention as chat attachments).
    content_base64: String,
}

async fn upload_user_skill(Json(req): Json<UploadSkillReq>) -> ApiResult {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(req.content_base64.trim())
        .map_err(|e| bad_request(format!("invalid file data: {e}")))?;

    let saved =
        crate::local::user_skills::save_upload(&req.filename, &bytes).map_err(bad_request)?;

    Ok(Json(json!({ "skill": user_skill_json(&saved) })))
}

#[derive(Deserialize)]
struct DeleteByNameQ {
    name: String,
}

async fn delete_user_skill(Query(q): Query<DeleteByNameQ>) -> ApiResult {
    tokio::task::spawn_blocking(move || crate::local::user_skills::delete(&q.name))
        .await
        .map_err(|e| ApiError::from(anyhow!(e)))?
        .map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

async fn get_user_skill(Path(name): Path<String>) -> ApiResult {
    let lookup = name.clone();
    let content =
        tokio::task::spawn_blocking(move || crate::local::user_skills::read_markdown(&lookup))
            .await
            .map_err(|e| ApiError::from(anyhow!(e)))?
            .map_err(bad_request)?
            .ok_or_else(|| not_found("skill"))?;
    Ok(Json(json!({ "name": name, "content": content })))
}

#[derive(Deserialize)]
struct UpdateSkillReq {
    content: String,
}

async fn update_user_skill(Path(name): Path<String>, Json(req): Json<UpdateSkillReq>) -> ApiResult {
    let saved = tokio::task::spawn_blocking(move || {
        crate::local::user_skills::update_markdown(&name, &req.content)
    })
    .await
    .map_err(|e| ApiError::from(anyhow!(e)))?
    .map_err(bad_request)?;
    Ok(Json(json!({ "skill": user_skill_json(&saved) })))
}

fn latex_template_json(t: &crate::local::latex_templates::LatexTemplate) -> Value {
    json!({
        "name": t.name,
        "entry": t.entry,
        "supportFiles": t.support_files,
        "bytes": t.bytes,
        "updatedAt": t.updated_at,
    })
}

async fn list_latex_templates() -> ApiResult {
    let templates: Vec<Value> = crate::local::latex_templates::list()
        .iter()
        .map(latex_template_json)
        .collect();
    Ok(Json(json!({ "templates": templates })))
}

async fn upload_latex_template(Json(req): Json<UploadSkillReq>) -> ApiResult {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(req.content_base64.trim())
        .map_err(|e| bad_request(format!("invalid file data: {e}")))?;
    let saved =
        crate::local::latex_templates::save_upload(&req.filename, &bytes).map_err(bad_request)?;
    Ok(Json(json!({ "template": latex_template_json(&saved) })))
}

async fn delete_latex_template(Query(q): Query<DeleteByNameQ>) -> ApiResult {
    crate::local::latex_templates::delete(&q.name).map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

/// Serialize a project for the UI, injecting the absolute artifacts directory
/// so the dashboard can recognize artifact paths in chat links. `filesDir` is
/// retained as a compatibility alias for older clients. Every project the UI
/// receives must go through this — the SSE `project.updated` diff (fired on
/// every visit, since `open_project` bumps `updated_at`) and `create_project`
/// upsert into the same `projects` state as `list_projects`.
fn project_json(p: &local::model::LocalProject) -> Value {
    project_json_with_artifacts_dir(p, local::files::files_dir_display(p))
}

fn project_json_with_artifacts_dir(p: &local::model::LocalProject, artifacts_dir: String) -> Value {
    // LocalProject is all String/Option/i64, so this can't realistically fail;
    // fail loud rather than emit a malformed `null` project if it ever does.
    let mut v = serde_json::to_value(p).expect("LocalProject serializes");
    if let Value::Object(map) = &mut v {
        let dir = Value::String(artifacts_dir);
        map.insert("artifactsDir".into(), dir.clone());
        map.insert("filesDir".into(), dir);
        map.insert("path".into(), Value::String(p.repo_path.clone()));
        map.insert("githubEnabled".into(), Value::Bool(p.github_enabled()));
        map.insert(
            "githubUrl".into(),
            p.github_url().map(Value::String).unwrap_or(Value::Null),
        );
    }
    v
}

async fn list_projects() -> ApiResult {
    let projects = Store::open()?.list_local_projects()?;
    let projects: Vec<Value> = projects.iter().map(project_json).collect();
    Ok(Json(json!({ "projects": projects })))
}

async fn list_project_activity(State(state): State<AppState>) -> ApiResult {
    let busy: HashSet<String> = state.chat.busy_sessions().await.into_iter().collect();
    tokio::task::spawn_blocking(move || -> Result<Json<Value>> {
        let store = Store::open()?;
        let mut active_agents = HashMap::<String, usize>::new();
        for (session_id, project_id) in store.list_chat_session_project_ids()? {
            // Side chats stay out of project agent counts.
            if busy.contains(&session_id)
                && store
                    .get_chat_session(&session_id)?
                    .is_some_and(|session| session.side_parent_session_id.is_none())
            {
                *active_agents.entry(project_id).or_default() += 1;
            }
        }
        let activity = store
            .list_project_activity_summaries()?
            .into_iter()
            .map(|summary| {
                let active_agents = active_agents.get(&summary.project_id).copied().unwrap_or(0);
                json!({
                    "projectId": summary.project_id,
                    "activeAgents": active_agents,
                    "totalAgents": summary.total_agents,
                    "runningExperiments": summary.running_experiments,
                    "totalExperiments": summary.total_experiments,
                    "lastMessageAt": summary.last_message_at,
                    "lastActivityAt": summary.last_activity_at,
                })
            })
            .collect::<Vec<_>>();
        Ok(Json(json!({ "activity": activity })))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("project activity task failed: {error}")))?
    .map_err(ApiError::from)
}

#[derive(Deserialize)]
struct ProjectPathStatusQ {
    path: Option<String>,
}

async fn install_git() -> ApiResult {
    // Spawned, so a page reload mid-install cannot orphan the extractor.
    tokio::spawn(local::portable_git::install())
        .await
        .map_err(|error| ApiError::from(anyhow!("Git install task failed: {error}")))?
        // `:#` keeps the cause (DNS, TLS, proxy) behind the outer context.
        .map_err(|error| {
            eprintln!("orx up: Git install failed: {error:#}");
            ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
        })?;
    Ok(Json(json!({})))
}

async fn project_path_status(Query(q): Query<ProjectPathStatusQ>) -> ApiResult {
    tokio::task::spawn_blocking(move || -> Result<Json<Value>> {
        let git_version = local::git::version();
        let Some(path) = q.path.filter(|path| !path.trim().is_empty()) else {
            return Ok(Json(json!({
                "gitVersion": git_version,
                "gitInstallable": cfg!(windows) && git_version.is_none(),
                "resolvedPath": null,
                "exists": null,
                "directory": null,
                "empty": null,
                "initialized": null,
                "gitState": null,
            })));
        };
        let resolved = local::projects::expand_path(&path)?;
        let exists = resolved.exists();
        let directory = resolved.is_dir();
        let empty = if directory {
            Some(std::fs::read_dir(&resolved)?.next().is_none())
        } else {
            None
        };
        let git_state =
            (git_version.is_some() && directory).then(|| local::git::repository_state(&resolved));
        let initialized = git_state.is_some_and(local::git::RepositoryState::is_initialized);
        let github_publication = initialized
            .then(|| local::git::github_publication(&resolved))
            .flatten();
        Ok(Json(json!({
            "gitVersion": git_version,
            "resolvedPath": resolved.to_string_lossy(),
            "exists": exists,
            "directory": directory,
            "empty": empty,
            "initialized": initialized,
            "gitState": git_state.map(local::git::RepositoryState::as_str),
            "githubOwner": github_publication.as_ref().map(|(owner, _)| owner),
            "githubRepo": github_publication.as_ref().map(|(_, repo)| repo),
        })))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("project path task failed: {error}")))?
    .map_err(bad_request)
}

async fn pick_project_folder() -> ApiResult {
    let path = tokio::task::spawn_blocking(crate::folder_picker::pick_folder)
        .await
        .map_err(|error| ApiError::from(anyhow!("folder picker task failed: {error}")))?
        .map_err(bad_request)?;
    Ok(Json(json!({
        "path": path.map(|path| path.to_string_lossy().into_owned()),
    })))
}

// --- papers (new-project "from a paper" flow; proxies alphaXiv) ------------

#[derive(Deserialize)]
struct PaperSearchQ {
    q: String,
}

async fn search_papers_api(Query(q): Query<PaperSearchQ>) -> ApiResult {
    let query = q.q.trim();
    if query.is_empty() {
        return Ok(Json(json!({ "papers": [] })));
    }
    let papers = crate::client::search_papers_fast(query)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "papers": papers })))
}

#[derive(Deserialize)]
struct PaperResolveQ {
    id: String,
}

async fn resolve_paper_api(Query(q): Query<PaperResolveQ>) -> ApiResult {
    let id = super::paper::parse_paper_id(&q.id);
    if id.is_empty() {
        return Err(bad_request("paper id is required"));
    }
    let paper = crate::client::resolve_paper(&id)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "paper": paper })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateProjectReq {
    creation_mode: Option<crate::telemetry::ProjectCreationMode>,
    name: String,
    path: String,
    run_command: Option<String>,
    paper_id: Option<String>,
    clone_url: Option<String>,
    #[serde(default)]
    create_folder: bool,
    #[serde(default)]
    require_new_folder: bool,
    #[serde(default)]
    initialize_git: bool,
    github_sync_enabled: Option<bool>,
    /// UI locale, for the starter prompts warmed up in the background.
    locale: Option<String>,
    starter_agent: Option<local::starter::Agent>,
}

async fn create_project(
    State(state): State<AppState>,
    Json(req): Json<CreateProjectReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let create_admission = state
        .project_lifecycle
        .admit("__project_create__")
        .ok_or_else(|| bad_request("project creation is unavailable"))?;
    reject_if_moving(&state)?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name is required"));
    }
    let locale = req.locale.unwrap_or_else(|| "en".to_string());
    let path = req.path;
    let create_folder = req.create_folder;
    let require_new_folder = req.require_new_folder;
    let initialize_git = req.initialize_git;
    let clone_url = req.clone_url.filter(|url| !url.trim().is_empty());
    let paper_id = req
        .paper_id
        .map(|paper_id| paper_id.trim().to_string())
        .filter(|paper_id| !paper_id.is_empty());
    // A paper with no linked repository starts blank, seeded with its PDF — the
    // only content such a project has, so a failed download fails the request.
    let paper_pdf = match (paper_id.as_deref(), clone_url.as_deref()) {
        (Some(id), None) => Some(
            crate::client::fetch_paper_pdf(id)
                .await
                .map_err(bad_request)?,
        ),
        _ => None,
    };
    let github_sync_enabled = req
        .github_sync_enabled
        .unwrap_or_else(crate::config::github_for_new_projects);
    let repo_size_kb = match clone_url.as_deref() {
        Some(url) => local::github::public_repo_size_kb(url).await,
        None => None,
    };
    let shallow_clone = local::github::should_shallow_clone(repo_size_kb);
    let run_command = req.run_command;
    let creation_guard = state.project_creation_lock.clone().lock_owned().await;
    let result = tokio::task::spawn_blocking(move || -> Result<_> {
        let _creation_guard = creation_guard;
        let store = Store::open()?;
        let project = local::projects::create_project(
            &store,
            &name,
            &path,
            local::projects::CreateProjectOptions {
                create_folder,
                require_new_folder,
                initialize_git,
                clone_url,
                shallow_clone,
                run_command,
                paper_id,
                paper_pdf,
            },
        )?;
        Ok((project, create_admission))
    })
    .await
    .map_err(|e| anyhow!("project task failed: {e}"))?;
    let (project, create_admission) = result.map_err(bad_request)?;
    // Starter prompts take a model call; start it now so the empty chat that
    // opens next usually finds them cached.
    local::starter::warm(project.clone(), locale, req.starter_agent);
    let _project_admission = state
        .project_lifecycle
        .admit(&project.id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    drop(create_admission);
    let (project, github_publication_error) = if github_sync_enabled {
        match push_project_for_sync(project.clone(), &state.chat, None).await {
            Ok((project, _)) => (project, None),
            Err(error) => {
                let project = Store::open()?
                    .get_local_project(&project.id)?
                    .unwrap_or(project);
                (project, Some(error.to_string()))
            }
        }
    } else {
        (project, None)
    };
    crate::telemetry::capture_project_created(true, req.creation_mode);
    Ok(Json(json!({
        "project": project_json(&project),
        "githubPublicationError": github_publication_error,
    })))
}

async fn get_project(Path(id): Path<String>) -> ApiResult {
    let project = Store::open()?
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!({ "project": project_json(&project) })))
}

fn project_git_json(
    project: &local::model::LocalProject,
    github_status: local::github::Status,
) -> Value {
    let path = std::path::Path::new(&project.repo_path);
    let initialized = local::git::is_repository(path);
    let branch = initialized
        .then(|| local::git::require_current_branch(path).ok())
        .flatten();
    let clean = initialized
        .then(|| local::git::is_clean(path).ok())
        .flatten();
    let remotes = if initialized {
        local::git::remotes(path)
            .unwrap_or_default()
            .into_iter()
            .map(|(name, url)| json!({ "name": name, "url": url }))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let (name, email, name_source, email_source) = if initialized {
        local::git::identity(path)
    } else {
        (None, None, None, None)
    };
    let sync_status = project.github_enabled().then(|| {
        local::git::publication_sync_status(
            path,
            &project.baseline_branch,
            &project.github_owner,
            &project.github_repo,
        )
    });
    json!({
        "path": project.repo_path,
        "gitVersion": local::git::version(),
        "initialized": initialized,
        "baselineBranch": project.baseline_branch,
        "currentBranch": branch,
        "clean": clean,
        "remotes": remotes,
        "identity": {
            "name": name,
            "email": email,
            "nameSource": name_source,
            "emailSource": email_source,
        },
        "github": {
            "ghInstalled": github_status.installed,
            "authenticated": github_status.authenticated,
            "enabled": project.github_enabled(),
            "owner": project.github_owner,
            "repo": project.github_repo,
            "url": project.github_url(),
            "syncStatus": sync_status,
        },
    })
}

async fn project_git_status(Path(id): Path<String>) -> ApiResult {
    let github_status = local::github::status().await;
    tokio::task::spawn_blocking(move || {
        let project = Store::open()?
            .get_local_project(&id)?
            .ok_or_else(|| anyhow!("project not found"))?;
        Ok(Json(project_git_json(&project, github_status)))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("git task failed: {error}")))?
}

async fn initialize_project_git(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let _admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    let _lock = project_publication_lock(&state, &id).await;
    let _creation_guard = state.project_creation_lock.lock().await;
    let github_status = local::github::status().await;
    tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        let mut project = store
            .get_local_project(&id)?
            .ok_or_else(|| anyhow!("project not found"))?;
        let path = std::path::Path::new(&project.repo_path);
        local::git::initialize_repository(path)?;
        local::git::validate_project_repository(path)?;
        project.baseline_branch = local::git::require_current_branch(path)?;
        store.update_local_project(&project)?;
        Ok(Json(project_git_json(&project, github_status)))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("git task failed: {error}")))?
}

fn push_project(project: &local::model::LocalProject) -> Result<()> {
    if !project.github_enabled() {
        return Err(anyhow!(
            "Enable GitHub syncing for this project before pushing."
        ));
    }
    let path = std::path::Path::new(&project.repo_path);
    local::git::add_github_remote(path, &project.github_owner, &project.github_repo)?;
    local::git::push_all(
        path,
        &project.baseline_branch,
        &project.github_owner,
        &project.github_repo,
    )
}

fn github_push_was_rejected(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("403")
        || error.contains("permission denied")
        || error.contains("write access")
        || error.contains("archived")
        || error.contains("read-only")
        || error.contains("repository not found")
        || error.contains("authentication failed")
        || error.contains("could not read username")
}

async fn create_independent_project_repository(
    mut project: local::model::LocalProject,
    chat: &ChatHost,
    organization: Option<&str>,
) -> Result<local::model::LocalProject> {
    let store = Store::open()?;
    let legacy = store
        .list_chat_sessions_by_project(&project.id)?
        .into_iter()
        .map(|session| session.id)
        .filter(|id| {
            local::git::existing_session_worktree_path(&project, id)
                != local::git::session_worktree_path(&project.id, id)
        })
        .collect::<Vec<_>>();
    // `git worktree move` would pull a running turn's checkout out from under it.
    chat.while_idle(&legacy, || {
        local::git::migrate_legacy_project_worktrees(&project, &legacy)
    })
    .await?;
    let source_repository = project
        .has_github_repository()
        .then(|| (project.github_owner.clone(), project.github_repo.clone()));
    let reroot_shallow = local::git::prepare_shallow_repository_for_publication(
        std::path::Path::new(&project.repo_path),
    )?;
    let (owner, repo) = local::github::create_project_repo(&project.slug, organization).await?;
    if reroot_shallow {
        local::git::reroot_shallow_repository(
            std::path::Path::new(&project.repo_path),
            &project.baseline_branch,
            source_repository.as_ref(),
        )?;
    }
    project.github_owner = owner;
    project.github_repo = repo;
    project.github_sync_enabled = false;
    store.update_local_project(&project)?;
    Ok(project)
}

async fn push_project_for_sync(
    mut project: local::model::LocalProject,
    chat: &ChatHost,
    organization: Option<&str>,
) -> Result<(local::model::LocalProject, local::github::Status)> {
    let github_status = local::github::status().await;
    if !github_status.installed {
        return Err(anyhow!(
            "GitHub CLI (`gh`) is required — install it from https://cli.github.com."
        ));
    }
    if !github_status.authenticated {
        return Err(anyhow!("Authenticate GitHub first with `gh auth login`."));
    }

    let mut using_existing_repository = project.has_github_repository();
    if using_existing_repository {
        let can_push = local::github::repo_meta(&project.github_owner, &project.github_repo)
            .await?
            .is_some_and(|meta| meta.can_push && !meta.archived);
        if !can_push {
            project = create_independent_project_repository(project, chat, organization).await?;
            using_existing_repository = false;
        }
    } else {
        project = create_independent_project_repository(project, chat, organization).await?;
        using_existing_repository = false;
    }

    let push_once = |project: &local::model::LocalProject| {
        let mut project = project.clone();
        project.github_sync_enabled = true;
        tokio::task::spawn_blocking(move || push_project(&project))
    };
    let first_push = push_once(&project)
        .await
        .map_err(|error| anyhow!("Git push task failed: {error}"))?;
    if let Err(error) = first_push {
        if !using_existing_repository || !github_push_was_rejected(&error.to_string()) {
            return Err(error);
        }
        project = create_independent_project_repository(project, chat, organization).await?;
        push_once(&project)
            .await
            .map_err(|error| anyhow!("Git push task failed: {error}"))??;
    }

    project.github_sync_enabled = true;
    Store::open()?.update_local_project(&project)?;
    Ok((project, github_status))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnableProjectGithubReq {
    organization: Option<String>,
}

async fn enable_project_github(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<EnableProjectGithubReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let _admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    let _lock = project_publication_lock(&state, &id).await;
    let store = Store::open()?;
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let organization = req
        .organization
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let (project, github_status) = push_project_for_sync(project, &state.chat, organization)
        .await
        .map_err(bad_request)?;
    let git_status = project_git_json(&project, github_status);
    Ok(Json(
        json!({ "project": project_json(&project), "git": git_status }),
    ))
}

async fn disable_project_github(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let _admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    let _lock = project_publication_lock(&state, &id).await;
    let store = Store::open()?;
    let mut project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    project.github_sync_enabled = false;
    store.update_local_project(&project)?;
    let github_status = local::github::status().await;
    Ok(Json(json!({
        "project": project_json(&project),
        "git": project_git_json(&project, github_status),
    })))
}

async fn push_project_github(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    let _admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    let _lock = project_publication_lock(&state, &id).await;
    let project = Store::open()?
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let project_for_push = project.clone();
    let github_status = local::github::status().await;
    let git_status = tokio::task::spawn_blocking(move || -> Result<Value> {
        push_project(&project_for_push)?;
        Ok(project_git_json(&project_for_push, github_status))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("git task failed: {error}")))?
    .map_err(bad_request)?;
    Ok(Json(
        json!({ "project": project_json(&project), "git": git_status }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShareProjectReq {
    #[serde(default)]
    chats: Vec<String>,
    #[serde(default = "default_true")]
    logs: bool,
    #[serde(default = "default_true")]
    artifacts: bool,
}

async fn share_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ShareProjectReq>,
) -> std::result::Result<axum::response::Response, ApiError> {
    reject_if_moving(&state)?;
    let admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    let project = Store::open()?
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    // The worker owns the lease: a dropped request must not let a delete or move start mid-export.
    let (name, scratch, out, skipped) = tokio::task::spawn_blocking(move || -> Result<_> {
        let _admission = admission;
        let (scratch, out, manifest) = export_shared_project(&project, req)?;
        Ok((
            local::share::file_name(&project),
            scratch,
            out,
            manifest.skipped,
        ))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("share task failed: {error}")))?
    .map_err(bad_request)?;
    let file = tokio::fs::File::open(out)
        .await
        .map_err(|error| ApiError::from(anyhow!("Could not read the shared project: {error}")))?;
    // Streamed from disk, since a project can outgrow memory; the stream keeps the scratch folder alive.
    let body = futures::StreamExt::map(tokio_util::io::ReaderStream::new(file), move |chunk| {
        let _keep = &scratch;
        chunk
    });
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", urlencoding::encode(&name)),
        )
        .header(
            "x-orx-share",
            urlencoding::encode(&json!({ "skipped": skipped }).to_string()).into_owned(),
        )
        .body(axum::body::Body::from_stream(body))
        .map_err(|error| ApiError::from(anyhow!("share response failed: {error}")))
}

/// Writes the project's `.orx` file into a scratch folder, which removes it when dropped.
fn export_shared_project(
    project: &local::model::LocalProject,
    req: ShareProjectReq,
) -> Result<(
    local::git::TemporaryDirectory,
    std::path::PathBuf,
    local::share::Manifest,
)> {
    use local::share::{ChatSelection, ShareOptions};
    let options = ShareOptions {
        chats: if req.chats.is_empty() {
            ChatSelection::None
        } else {
            ChatSelection::Only(req.chats)
        },
        logs: req.logs,
        artifacts: req.artifacts,
    };
    let scratch = local::git::TemporaryDirectory::new("orx-share")?;
    // A fixed name: project names can outgrow a file name's length limit.
    let out = scratch.path().join("project.orx");
    let manifest = local::share::export(&Store::open()?, project, &options, &out)?;
    Ok((scratch, out, manifest))
}

async fn openresearch_credentials() -> std::result::Result<crate::config::Credentials, ApiError> {
    crate::config::load_credentials().await?.ok_or_else(|| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Sign in to OpenResearch to publish a link.".into(),
        )
    })
}

/// Uploads the project as a `.orx` file to openresearch.sh; publishing again replaces the file behind the same link.
async fn publish_project_link(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<BackgroundQuery>,
    Json(req): Json<ShareProjectReq>,
) -> std::result::Result<Response, ApiError> {
    let creds = openresearch_credentials().await?;
    reject_if_moving(&state)?;
    let admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    let project = Store::open()?
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    run_operation(
        &state.operations,
        query.background,
        "preparing",
        move |progress| async move {
            // The worker owns the lease, released once the file is written; the upload needs no lease.
            let (_scratch, out, manifest) = tokio::task::spawn_blocking(move || {
                let _admission = admission;
                export_shared_project(&project, req)
            })
            .await
            .map_err(|error| anyhow!("Preparing the project stopped unexpectedly: {error}"))??;
            progress.phase("uploading");
            let reporter = progress.clone();
            let link =
                crate::client::publish_project(&creds, &id, &manifest, &out, move |sent, total| {
                    reporter.bytes(sent, Some(total))
                })
                .await?;
            Ok(json!({ "sharedProject": link, "skipped": manifest.skipped }))
        },
    )
    .await
}

/// Whether this machine can publish, and the project's link if it has one. In an SSH
/// workspace this answers for the remote, whose sign-in is not the laptop's.
async fn get_project_link(Path(id): Path<String>) -> ApiResult {
    let Some(creds) = crate::config::load_credentials().await? else {
        return Ok(Json(json!({ "signedIn": false, "sharedProject": null })));
    };
    // Only a hint: an older or unreachable server must not stand in the way of saving a file.
    match crate::client::find_published_project(&creds, &id).await {
        Err(error)
            if error
                .downcast_ref::<crate::client::Unauthorized>()
                .is_some() =>
        {
            Ok(Json(json!({ "signedIn": false, "sharedProject": null })))
        }
        link => Ok(Json(
            json!({ "signedIn": true, "sharedProject": link.unwrap_or(None) }),
        )),
    }
}

/// A published link's public details, shown before opening it.
async fn get_published_project(Path(slug): Path<String>) -> ApiResult {
    let slug =
        crate::client::published_project_slug(&slug).ok_or_else(|| not_found("project link"))?;
    let project =
        crate::client::get_public_project(&crate::config::openresearch_api_url().await, slug)
            .await
            .map_err(bad_request)?;
    Ok(Json(json!({ "sharedProject": project })))
}

/// Most a shared project upload may be; it streams to disk, outside the JSON request cap.
const SHARED_PROJECT_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Deserialize)]
struct ImportSharedProjectQuery {
    /// Folder to create the project in.
    path: String,
    /// A `.orx` file on this machine; without it or `shared` the request body is the file.
    file: Option<String>,
    /// The slug of a published project link to download.
    shared: Option<String>,
}

#[derive(Deserialize)]
struct BackgroundQuery {
    /// Answer at once with an `operationId` to follow; without it the request waits for the result.
    #[serde(default)]
    background: bool,
}

/// Work a request starts and the page follows with `GET /api/operations/{id}`, so a slow
/// network can neither time the request out nor cancel the work.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    phase: &'static str,
    bytes: u64,
    total_bytes: Option<u64>,
    result: Option<Value>,
    error: Option<String>,
    #[serde(skip)]
    finished: Option<std::time::Instant>,
}

/// How long a finished operation stays readable, so a lost poll can ask again.
const FINISHED_OPERATION_KEPT: Duration = Duration::from_secs(5 * 60);

type Operations = Arc<std::sync::Mutex<HashMap<String, Operation>>>;

/// Where background work reports how it is going; silent when the request waits instead.
#[derive(Clone)]
struct Progress(Option<(Operations, String)>);

impl Progress {
    fn phase(&self, phase: &'static str) {
        self.update(|operation| {
            operation.phase = phase;
            operation.bytes = 0;
            operation.total_bytes = None;
        });
    }

    fn bytes(&self, bytes: u64, total: Option<u64>) {
        self.update(|operation| {
            operation.bytes = bytes;
            operation.total_bytes = total;
        });
    }

    fn update(&self, change: impl FnOnce(&mut Operation)) {
        if let Some((operations, id)) = &self.0 {
            if let Some(operation) = operations.lock().unwrap().get_mut(id) {
                change(operation);
            }
        }
    }
}

/// Runs `work` in its own task: with `background` answers 202 with its `operationId`, else waits for its result.
async fn run_operation<F>(
    operations: &Operations,
    background: bool,
    phase: &'static str,
    work: impl FnOnce(Progress) -> F,
) -> std::result::Result<Response, ApiError>
where
    F: std::future::Future<Output = Result<Value>> + Send + 'static,
{
    let id = uuid::Uuid::new_v4().to_string();
    let progress = Progress(background.then(|| {
        let mut running = operations.lock().unwrap();
        running.retain(|_, operation| {
            operation
                .finished
                .is_none_or(|finished| finished.elapsed() < FINISHED_OPERATION_KEPT)
        });
        let operation = Operation {
            phase,
            bytes: 0,
            total_bytes: None,
            result: None,
            error: None,
            finished: None,
        };
        running.insert(id.clone(), operation);
        (operations.clone(), id.clone())
    }));
    let work = tokio::spawn(work(progress.clone()));
    // The page's request ends at once, so the work itself holds off a restart.
    let active = Active::new();
    let task = tokio::spawn(async move {
        let _active = active;
        let result = work
            .await
            .map_err(|error| anyhow!("The work stopped unexpectedly: {error}"))
            .and_then(|result| result);
        progress.update(|operation| {
            match &result {
                Ok(value) => operation.result = Some(value.clone()),
                Err(error) => operation.error = Some(format!("{error:#}")),
            }
            operation.finished = Some(std::time::Instant::now());
        });
        result
    });
    if background {
        return Ok((StatusCode::ACCEPTED, Json(json!({ "operationId": id }))).into_response());
    }
    let result = task
        .await
        .map_err(|error| ApiError::from(anyhow!("The work stopped unexpectedly: {error}")))?
        .map_err(bad_request)?;
    Ok(Json(result).into_response())
}

/// How background work is going; a finished one stays readable for a while.
async fn get_operation(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let operation = state.operations.lock().unwrap().get(&id).cloned();
    let operation = operation.ok_or_else(|| {
        ApiError(
            StatusCode::NOT_FOUND,
            "This is no longer running; OpenResearch may have restarted. Try again.".into(),
        )
    })?;
    Ok(Json(json!(operation)))
}

async fn import_shared_project(
    State(state): State<AppState>,
    Query(query): Query<ImportSharedProjectQuery>,
    Query(BackgroundQuery { background }): Query<BackgroundQuery>,
    body: axum::body::Body,
) -> std::result::Result<Response, ApiError> {
    use futures::StreamExt;
    reject_if_moving(&state)?;
    let admission = state
        .project_lifecycle
        .admit("__project_create__")
        .ok_or_else(|| bad_request("project creation is unavailable"))?;
    reject_if_moving(&state)?;
    let (file, slug, staged) = match (query.file, query.shared) {
        (Some(file), _) => (
            local::projects::expand_path(&file).map_err(bad_request)?,
            None,
            None,
        ),
        (None, Some(shared)) => {
            let slug = crate::client::published_project_slug(&shared)
                .ok_or_else(|| bad_request("That is not an OpenResearch project link."))?
                .to_owned();
            let download = local::git::TemporaryDirectory::new("orx-import")?;
            (
                download.path().join("download.orx"),
                Some(slug),
                Some(download),
            )
        }
        (None, None) => {
            let upload = local::git::TemporaryDirectory::new("orx-import")?;
            let path = upload.path().join("upload.orx");
            let mut out = tokio::fs::File::create(&path)
                .await
                .map_err(|error| ApiError::from(anyhow!("Could not stage the upload: {error}")))?;
            let mut stream = body.into_data_stream();
            let mut received: u64 = 0;
            while let Some(chunk) = stream.next().await {
                let chunk =
                    chunk.map_err(|error| bad_request(format!("Upload failed: {error}")))?;
                received += chunk.len() as u64;
                if received > SHARED_PROJECT_UPLOAD_BYTES {
                    return Err(bad_request("This project file is too large to open."));
                }
                tokio::io::AsyncWriteExt::write_all(&mut out, &chunk)
                    .await
                    .map_err(|error| {
                        ApiError::from(anyhow!("Could not stage the upload: {error}"))
                    })?;
            }
            // tokio writes in the background; the import must read the whole file.
            tokio::io::AsyncWriteExt::flush(&mut out)
                .await
                .map_err(|error| ApiError::from(anyhow!("Could not stage the upload: {error}")))?;
            drop(out);
            (path, None, Some(upload))
        }
    };
    let creation_lock = state.project_creation_lock.clone();
    let target = query.path;
    let phase = if slug.is_some() {
        "downloading"
    } else {
        "unpacking"
    };
    run_operation(
        &state.operations,
        background,
        phase,
        move |progress| async move {
            if let Some(slug) = &slug {
                crate::client::download_public_project(
                    &crate::config::openresearch_api_url().await,
                    slug,
                    &file,
                    |received, total| progress.bytes(received, total),
                )
                .await?;
                progress.phase("unpacking");
            }
            let creation_guard = creation_lock.lock_owned().await;
            tokio::task::spawn_blocking(move || -> Result<Value> {
                let (_creation_guard, _admission, _staged) = (creation_guard, admission, staged);
                let target = local::projects::expand_path(&target)?;
                let mut on_progress = |step| match step {
                    local::share::ImportProgress::Unpacking { done, total } => {
                        progress.bytes(done, Some(total))
                    }
                    local::share::ImportProgress::Opening => progress.phase("opening"),
                };
                let imported =
                    local::share::import(&Store::open()?, &file, &target, &mut on_progress)?;
                Ok(json!({
                    "project": project_json(&imported.project),
                    "missingBranches": imported.missing_branches,
                }))
            })
            .await
            .map_err(|error| anyhow!("Opening the project stopped unexpectedly: {error}"))?
        },
    )
    .await
}

async fn github_account() -> ApiResult {
    Ok(Json(
        json!({ "login": local::github::viewer_login().await.ok() }),
    ))
}

async fn github_organizations() -> ApiResult {
    let organizations = local::github::viewer_organizations()
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "organizations": organizations })))
}

#[derive(Deserialize)]
struct ProjectRepoPreviewQuery {
    name: String,
}

async fn github_project_repo_preview(Query(q): Query<ProjectRepoPreviewQuery>) -> ApiResult {
    let candidate = local::projects::project_slug_preview(&Store::open()?, q.name.trim())?;
    let repo = local::github::available_project_repo_name(&candidate)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "repo": repo })))
}

#[derive(Deserialize)]
struct RepoAccessQuery {
    owner: String,
    repo: String,
}

async fn github_repo_access(Query(q): Query<RepoAccessQuery>) -> ApiResult {
    let owner = q.owner.trim();
    let repo = q.repo.trim();
    if owner.is_empty() || repo.is_empty() {
        return Err(bad_request("owner and repo are required"));
    }
    let meta = local::github::repo_meta(owner, repo)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({
        "canPush": meta.is_some_and(|meta| meta.can_push && !meta.archived),
    })))
}

/// Mark a project visited: bumps updated_at, which drives the recency sort
/// and the SSE project.updated diff.
async fn open_project(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store.touch_local_project(&id)?;
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!({ "project": project_json(&project) })))
}

/// Present-vs-absent for PATCH fields: absent = leave, null = clear.
fn double_option<'de, D>(d: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProjectReq {
    name: Option<String>,
    #[serde(default, deserialize_with = "double_option")]
    run_command: Option<Option<String>>,
}

async fn update_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateProjectReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let _admission = state
        .project_lifecycle
        .admit(&id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    reject_if_moving(&state)?;
    if req.name.is_none() && req.run_command.is_none() {
        return Err(bad_request(
            "nothing to update: pass name and/or runCommand",
        ));
    }
    let store = Store::open()?;
    let mut project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(bad_request("name cannot be empty"));
        }
        project.name = name.trim().to_string();
    }
    if let Some(cmd) = req.run_command {
        project.run_command = cmd.filter(|c| !c.trim().is_empty());
    }
    store.update_local_project(&project)?;
    // Re-read: update bumps updated_at, which is also what fires the SSE
    // project.updated diff.
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!({ "project": project_json(&project) })))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteProjectQuery {
    #[serde(default)]
    delete_artifacts: bool,
}

/// Delete a project and everything hanging off it. Refuses while runs are in
/// flight (deleting their rows would strand the supervisor mid-job) — but
/// requests their cancellation, so a retry shortly after goes through. The
/// registered repository folder is left untouched.
async fn delete_project(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<DeleteProjectQuery>,
) -> ApiResult {
    // Finish destructive cleanup even if the HTTP request is canceled.
    tokio::spawn(delete_project_inner(state, id, query))
        .await
        .map_err(|error| ApiError::from(anyhow!("project deletion task failed: {error}")))?
}

async fn delete_project_inner(state: AppState, id: String, query: DeleteProjectQuery) -> ApiResult {
    reject_if_moving(&state)?;
    let _deleting_project = state
        .project_lifecycle
        .begin_delete(&id)
        .ok_or_else(|| bad_request("project has an operation or deletion in progress"))?;
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let project = store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let in_flight: Vec<_> = store
        .list_runs_by_project(&id)?
        .into_iter()
        .filter(|r| !is_terminal(&r.status))
        .collect();
    if !in_flight.is_empty() {
        let mut failures = Vec::new();
        for run in &in_flight {
            if let Err(err) = api_v2::cancel_run(
                run,
                "Cancel requested because the project is being deleted.",
            )
            .await
            {
                failures.push(format!("{}: {err}", run.id));
            }
        }
        if !failures.is_empty() {
            let requested = in_flight.len() - failures.len();
            return Err(bad_request(format!(
                "{requested} run(s) cancellation requested; cancellation failed for {}",
                failures.join(", ")
            )));
        }
        return Err(bad_request(format!(
            "{} run(s) still in flight — cancellation requested; retry once they stop",
            in_flight.len()
        )));
    }
    // Abort any in-flight chat turns before their rows disappear, and clean up
    // each session's serve child + worktree (the rows cascade with the project).
    let sessions = store.list_chat_sessions_by_project(&id)?;
    let mut _session_deletions = Vec::with_capacity(sessions.len());
    for session in &sessions {
        _session_deletions.push(
            state
                .chat
                .begin_session_delete(&session.id)
                .ok_or_else(|| bad_request("a chat session deletion is already in progress"))?,
        );
    }
    for session in &sessions {
        state.chat.clear_queue(&session.id)?;
        let _ = state.chat.interrupt(&session.id).await;
        state.chat.opencode.kill_session(&session.id).await;
        state.chat.codex.kill_session(&session.id).await;
        state.chat.claude.kill_session(&session.id).await;
        state.chat.acp.stop(&session.id).await;
    }
    let projects = store.list_local_projects()?;
    // Refuse before anything is deleted; remove only once the rows are gone.
    let artifacts =
        local::files::plan_project_artifact_cleanup(&project, query.delete_artifacts, &projects)?;
    let session_ids = sessions
        .iter()
        .map(|session| session.id.clone())
        .collect::<Vec<_>>();
    let project_id = id.clone();
    // Row deletion can wait up to RUN_ARTIFACTS_WAIT for the run-artifacts lock, so keep it off the async workers.
    tokio::task::spawn_blocking(move || {
        let native_purge = (!session_ids.is_empty()).then_some(|| {
            local::chat::ChatHost::purge_native_history_blocking(&store, &session_ids)
        });
        store.delete_local_project_with(&project_id, native_purge)
    })
    .await
    .map_err(|error| anyhow!("project row deletion task failed: {error}"))??;
    for session in &sessions {
        state.chat.claude.forget_session(&session.id).await;
        local::chat::cleanup_session_transcript_artifacts(&session.id);
        local::chat::cleanup_session_worktree(&project, &session.id);
    }
    let _ = std::fs::remove_dir(local::git::worktrees_root(&id));
    // The rows are gone, so a failure here is reported without failing the deletion.
    let artifacts_error = match tokio::task::spawn_blocking(move || artifacts.apply()).await {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(format!("The project was deleted. {error}")),
        Err(error) => Some(format!(
            "The project was deleted. Artifact cleanup failed: {error}"
        )),
    };
    Ok(Json(
        json!({ "ok": true, "artifactsError": artifacts_error }),
    ))
}

async fn list_experiments(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let experiments = store.list_experiments_by_project(&id)?;
    Ok(Json(json!({ "experiments": experiments })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveExperimentRequest {
    direction: String,
    archived: bool,
}

async fn set_experiment_archive(
    Path(id): Path<String>,
    Json(request): Json<ArchiveExperimentRequest>,
) -> ApiResult {
    let direction = match request.direction.as_str() {
        "ancestors" => local::experiments::ArchiveDirection::Ancestors,
        "descendants" => local::experiments::ArchiveDirection::Descendants,
        "only" => local::experiments::ArchiveDirection::Only,
        "region" => local::experiments::ArchiveDirection::Region,
        "taskRegion" => local::experiments::ArchiveDirection::TaskRegion,
        _ => {
            return Err(bad_request(
                "direction must be ancestors, descendants, only, region, or taskRegion",
            ))
        }
    };
    let mut store = Store::open()?;
    if store.get_local_experiment(&id)?.is_none() {
        return Err(not_found("experiment"));
    }
    let ids = local::experiments::set_archived(&mut store, &id, direction, request.archived)?;
    Ok(Json(json!({ "ids": ids })))
}

async fn delete_experiment(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    // Deletion can wait up to RUN_ARTIFACTS_WAIT for the run-artifacts lock.
    blocking_api(move || {
        let store = Store::open()?;
        let experiment = store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let _admission = state
            .project_lifecycle
            .admit(&experiment.project_id)
            .ok_or_else(|| bad_request("project deletion is in progress"))?;
        let deleted = local::experiments::delete_experiment(&store, &id).map_err(bad_request)?;
        Ok(Json(
            json!({ "ok": true, "branchCommit": deleted.branch_commit }),
        ))
    })
    .await
}

async fn list_project_runs(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    let runs = store
        .list_runs_by_project(&id)?
        .iter()
        .map(|run| ApiRun::new(&store, run))
        .collect::<Result<Vec<_>>>()?;
    Ok(Json(json!({ "runs": runs })))
}

async fn compute_backends() -> Json<Value> {
    Json(json!({ "backends": crate::compute::capabilities() }))
}

/// Dashboard requests omit `chat_session_id`; forwarded agent CLI requests use
/// it only for run attribution and wakeups.
#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateRunReq {
    invocation_context: Option<String>,
    #[serde(default)]
    telemetry_suppressed: bool,
    experiment_id: String,
    backend: Option<String>,
    cpus: Option<u32>,
    mem: Option<String>,
    partition: Option<String>,
    qos: Option<String>,
    nodelist: Option<String>,
    constraint: Option<String>,
    exclude: Option<String>,
    #[serde(default)]
    sbatch_args: Vec<String>,
    flavor: Option<String>,
    host: Option<String>,
    container: Option<String>,
    #[serde(default)]
    no_container: bool,
    manifest: Option<String>,
    image: Option<String>,
    timeout: Option<String>,
    org: Option<String>,
    provider: Option<String>,
    disk: Option<i64>,
    #[serde(default)]
    force: bool,
    chat_session_id: Option<String>,
    agent_origin: Option<String>,
    /// Absent from dashboard callers; checked against orx up's own config dir.
    caller_config_dir: Option<std::path::PathBuf>,
}

impl CreateRunReq {
    fn into_run_args(self) -> crate::ExpRunArgs {
        let mut backend = self.backend;
        let mut flavor = self.flavor;
        local::apply_compute_default(&mut backend, &mut flavor);
        crate::ExpRunArgs {
            invocation_context: self.invocation_context,
            telemetry_suppressed: self.telemetry_suppressed,
            exp_id: self.experiment_id,
            disk: self.disk,
            provider: self.provider,
            backend: Some(backend.unwrap_or_else(|| "local".to_string())),
            flavor,
            org: self.org,
            host: self.host,
            container: self.container,
            no_container: self.no_container,
            manifest: self.manifest,
            image: self.image,
            timeout: self.timeout,
            cpus: self.cpus,
            mem: self.mem,
            partition: self.partition,
            qos: self.qos,
            nodelist: self.nodelist,
            constraint: self.constraint,
            exclude: self.exclude,
            sbatch_args: self.sbatch_args,
            force: self.force,
            chat_session_id: self.chat_session_id,
            agent_origin: self.agent_origin,
            forwarded: true,
            caller_config_dir: self.caller_config_dir,
            caller_python_overrides: Default::default(),
            caller_compute_settings: None,
        }
    }
}

/// A relative `XDG_CONFIG_HOME` names a different dir in each process's working directory.
pub(crate) fn absolute_config_dir() -> std::path::PathBuf {
    let dir = crate::config::config_dir();
    std::path::absolute(&dir).unwrap_or(dir)
}

/// The settings in a config dir that pick where a run goes or which account it uses.
pub(crate) type LaunchSettings = std::collections::BTreeMap<String, Option<String>>;

/// The [`LaunchSettings`] a run of `args` would use from this process's config dir, and whether
/// that dir sets any of them; `None` for a backend that reads none from it.
pub(crate) async fn launch_settings(
    args: &crate::ExpRunArgs,
) -> Result<Option<(LaunchSettings, bool)>> {
    use crate::jobs::{kubernetes, ray, slurm};

    Ok(Some(match args.backend.as_deref().unwrap_or("local") {
        "k8s" => {
            let file = kubernetes::load_settings()?;
            let set = file.is_some();
            let file = file.unwrap_or_default();
            let settings = LaunchSettings::from([
                ("context".into(), file.context),
                ("namespace".into(), Some(file.namespace)),
            ]);
            (settings, set)
        }
        "slurm" => {
            let file = slurm::load_settings()?;
            let set = file.is_some();
            let file = file.unwrap_or_default();
            let settings = LaunchSettings::from([
                ("host".into(), args.host.clone().or(file.host)),
                (
                    "partition".into(),
                    args.partition.clone().or(file.partition),
                ),
                ("account".into(), file.account),
                ("remoteRoot".into(), file.remote_root),
                ("timeLimit".into(), args.timeout.clone().or(file.time_limit)),
                (
                    "cpusPerTask".into(),
                    args.cpus.or(file.cpus_per_task).map(|n| n.to_string()),
                ),
                ("mem".into(), args.mem.clone().or(file.mem)),
            ]);
            (settings, set)
        }
        "ray" => {
            let set = ray::load_settings()?
                .and_then(|file| file.address)
                .is_some_and(|address| !address.trim().is_empty());
            let address = Some(ray::resolve_address(None));
            (LaunchSettings::from([("address".into(), address)]), set)
        }
        "openresearch" => {
            let credentials = crate::config::load_credentials().await?;
            let set = credentials.is_some();
            let (url, token) = credentials
                .map(|credentials| {
                    let digest = format!("{:x}", Sha256::digest(credentials.token.as_bytes()));
                    (credentials.api_url, format!("sha256:{}", &digest[..12]))
                })
                .unzip();
            let settings = LaunchSettings::from([("apiUrl".into(), url), ("token".into(), token)]);
            (settings, set)
        }
        _ => return Ok(None),
    }))
}

/// Refuses a forwarded run that orx up would launch with other settings than the caller's,
/// such as another cluster or namespace than the one the caller tested.
pub(crate) async fn require_caller_settings(
    args: &crate::ExpRunArgs,
    own_dir: &std::path::Path,
) -> Result<()> {
    let Some(caller_dir) = args.caller_config_dir.as_deref() else {
        return Ok(());
    };
    let same_dir = caller_dir == own_dir
        || crate::paths::canonicalize(caller_dir)
            .ok()
            .zip(crate::paths::canonicalize(own_dir).ok())
            .is_some_and(|(a, b)| a == b);
    if same_dir {
        return Ok(());
    }
    let Some((own, _)) = launch_settings(args).await? else {
        return Ok(());
    };

    let base = own_dir.parent().unwrap_or(own_dir);
    let fix = format!(
        "Set XDG_CONFIG_HOME to {} and re-run, or restart orx up from this environment.",
        base.display()
    );
    // An orx older than this check sends only its config dir, which orx up may not be able to read.
    let Some(caller) = &args.caller_compute_settings else {
        return Err(anyhow!(
            "This command uses the compute settings in {}, but orx up launches runs with \
             the settings in {}. {fix}",
            caller_dir.display(),
            own_dir.display(),
        ));
    };
    let shown = |value: &Option<String>| value.as_deref().unwrap_or("unset").to_string();
    let differences: Vec<String> = caller
        .iter()
        .filter_map(|(name, value)| {
            let theirs = own.get(name).filter(|theirs| *theirs != value)?;
            Some(format!("{name} {} vs {}", shown(value), shown(theirs)))
        })
        .collect();
    if differences.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "This command's {} settings in {} differ from those orx up launches runs with, in {}: \
         {}. {fix}",
        args.backend.as_deref().unwrap_or("local"),
        caller_dir.display(),
        own_dir.display(),
        differences.join("; "),
    ))
}

async fn create_run(Json(req): Json<CreateRunReq>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_experiment(&req.experiment_id)?
        .ok_or_else(|| not_found("experiment"))?;
    // Dashboard callers may omit compute options; forwarded CLI requests arrive resolved.
    let run = api_v2::launch(req.into_run_args())
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "run": ApiRun::new(&store, &run)? })))
}

async fn get_run(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    if is_terminal(&run.status) {
        crate::compute::remove_hf_staging(&run).map_err(bad_request)?;
    }
    Ok(Json(json!({ "run": ApiRun::new(&store, &run)? })))
}

/// Newest-first cap for the cross-project instances list. Generous: the store
/// is a local single-user SQLite db, so this only bounds pathological history.
const INSTANCES_LIMIT: usize = 500;

/// Every run across all projects (running first on the client), each tagged
/// with its owning project's name — the "instances" view of compute the agents
/// have spun up (Modal / HF / SSH / K8s), regardless of which project launched
/// it. Includes finished runs as history; the client surfaces live ones first.
async fn list_instances() -> ApiResult {
    let store = Store::open()?;
    let names: HashMap<String, String> = store
        .list_local_projects()?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let mut instances: Vec<Value> = Vec::new();
    for run in store.list_runs(INSTANCES_LIMIT)? {
        // ApiRun is a plain serializable struct, so this can't realistically
        // fail; propagate rather than emit a malformed row if it ever does.
        let mut value = serde_json::to_value(ApiRun::new(&store, &run)?)
            .map_err(|e| anyhow!("serialize run {}: {e}", run.id))?;
        if let (Some(obj), Some(name)) = (value.as_object_mut(), names.get(&run.project_id)) {
            obj.insert("projectName".into(), json!(name));
        }
        instances.push(value);
    }
    Ok(Json(json!({ "instances": instances })))
}

/// Sent by `orx exp cancel`; the dashboard and older CLIs post no body.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelRunReq {
    chat_session_id: Option<String>,
}

async fn cancel_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    req: Option<Json<CancelRunReq>>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let run = local::local_run(&store, &id)?.ok_or_else(|| not_found("run"))?;
    // A terminal run must not gain a stale cancel_requested flag.
    if is_terminal(&run.status) {
        return Ok(Json(json!({ "ok": true, "alreadyTerminal": true })));
    }
    let reason = match req {
        Some(Json(req)) => crate::commands::exp::exp_cancel_reason(req.chat_session_id.as_deref()),
        None => "Cancel requested through orx up (dashboard or an older orx CLI).".into(),
    };
    api_v2::cancel_run(&run, &reason)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct LogsQuery {
    cursor: Option<u64>,
    generation: Option<String>,
}

async fn run_logs(Path(id): Path<String>, Query(q): Query<LogsQuery>) -> ApiResult {
    let store = Store::open()?;
    let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    let complete = is_terminal(&run.status) && !store.log_collection_pending(&id)?;
    let snapshot = crate::jobs::ssh::service::logs::read(
        &run.id,
        q.cursor.unwrap_or(0),
        256 * 1024,
        q.generation.as_deref(),
    )?;
    let next = snapshot.offset + snapshot.bytes.len() as u64;
    Ok(Json(json!({
        "generation": snapshot.generation,
        "offset": snapshot.offset,
        "dataBase64": base64::engine::general_purpose::STANDARD.encode(&snapshot.bytes),
        "nextCursor": next,
        "eof": complete && next >= snapshot.size,
    })))
}

// --- diffs ------------------------------------------------------------------
//
// Same payload shape as the OpenResearch api diff endpoints:
// `{diff, truncated, bytesRead, byteLimit}` with the raw unified-diff text.
// All of these shell out to git against the project's local clone, so they
// run on the blocking pool.

fn diff_json(d: local::git::DiffPayload) -> Value {
    json!({
        "diff": d.diff,
        "truncated": d.truncated,
        "bytesRead": d.bytes_read,
        "byteLimit": local::git::MAX_DIFF_BYTES,
    })
}

/// Off-worker helper for git-backed handlers.
async fn blocking_api<F>(f: F) -> ApiResult
where
    F: FnOnce() -> std::result::Result<Json<Value>, ApiError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

/// Cumulative diff of a run's commit vs its experiment's parent branch. A root
/// experiment on a distinct branch compares against the project's baseline.
async fn run_diff(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let run = store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
        let sha = run
            .commit_sha
            .clone()
            .ok_or_else(|| bad_request("run has no commit to diff"))?;
        let exp = store
            .get_local_experiment(&run.experiment_id)?
            .ok_or_else(|| not_found("experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let base = match exp.parent_experiment_id {
            Some(parent_id) => {
                store
                    .get_local_experiment(&parent_id)?
                    .ok_or_else(|| not_found("parent experiment"))?
                    .branch_name
            }
            None => project.baseline_branch.clone(),
        };
        let repo = std::path::Path::new(&project.repo_path);
        let payload = local::git::diff_range(repo, &base, &sha)?;
        Ok(Json(diff_json(payload)))
    })
    .await
}

/// Cumulative committed diff of an experiment branch vs its parent branch.
async fn experiment_diff(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let exp = store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let base = match &exp.parent_experiment_id {
            Some(parent_id) => {
                store
                    .get_local_experiment(parent_id)?
                    .ok_or_else(|| not_found("parent experiment"))?
                    .branch_name
            }
            None => project.baseline_branch.clone(),
        };
        let repo = std::path::Path::new(&project.repo_path);
        Ok(Json(diff_json(local::git::diff_range(
            repo,
            &base,
            &exp.branch_name,
        )?)))
    })
    .await
}

/// Commits on the experiment branch: child experiments list `parent..branch`,
/// the baseline lists the branch's recent history.
async fn experiment_commits(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let exp = store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let commits = match &exp.parent_experiment_id {
            Some(pid) => {
                let parent = store
                    .get_local_experiment(pid)?
                    .ok_or_else(|| not_found("parent experiment"))?;
                local::git::list_commits_between(repo, &parent.branch_name, &exp.branch_name, 100)?
            }
            None if exp.branch_name != project.baseline_branch => local::git::list_commits_between(
                repo,
                &project.baseline_branch,
                &exp.branch_name,
                100,
            )?,
            None => local::git::list_commits(repo, &exp.branch_name, 25)?,
        };
        let commits: Vec<Value> = commits
            .iter()
            .map(|c| json!({ "sha": c.sha, "subject": c.subject, "committedAt": c.committed_at }))
            .collect();
        Ok(Json(json!({ "commits": commits })))
    })
    .await
}

async fn experiment_commit_diff(Path((id, sha)): Path<(String, String)>) -> ApiResult {
    if !sha.chars().all(|c| c.is_ascii_hexdigit()) || sha.len() < 7 || sha.len() > 64 {
        return Err(bad_request("invalid commit sha"));
    }
    blocking_api(move || {
        let store = Store::open()?;
        let exp = store
            .get_local_experiment(&id)?
            .ok_or_else(|| not_found("experiment"))?;
        let project = store
            .get_local_project(&exp.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let payload = local::git::commit_diff(repo, &sha)?;
        Ok(Json(diff_json(payload)))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrewarmStarterPromptsReq {
    name: String,
    paper_id: Option<String>,
    /// An existing folder the project will be created from.
    path: Option<String>,
    locale: Option<String>,
    starter_agent: Option<local::starter::Agent>,
}

/// Start generating starter prompts for a project the user is still naming in
/// the new-project form, so they are cached before the project exists.
async fn prewarm_starter_prompts(Json(req): Json<PrewarmStarterPromptsReq>) -> ApiResult {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name is required"));
    }
    let paper_id = req
        .paper_id
        .as_deref()
        .map(super::paper::parse_paper_id)
        .filter(|id| !id.is_empty());
    let path = req
        .path
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());
    local::starter::prewarm(
        name,
        paper_id,
        path,
        req.locale.unwrap_or_else(|| "en".to_string()),
        req.starter_agent,
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct StarterPromptsQuery {
    profile: Option<String>,
    /// Chat harness whose one-shot child writes the prompts; the composer's
    /// current pick. Unknown or missing = no prompts.
    harness: Option<String>,
    /// The composer's model, so the child runs on what the chat will use.
    model: Option<String>,
    /// UI locale the prompts are written in.
    locale: Option<String>,
}

/// Four starter prompts for the empty chat, written by a model that has read
/// the project (paper, README, code). Slow on a cache miss — one headless
/// model call — so the UI shows a placeholder while it waits. A blank project
/// with no profile to tailor to, or whose tailored prompts fail, is flagged so
/// the UI shows its pre-written prompts.
async fn project_starter_prompts(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<StarterPromptsQuery>,
) -> ApiResult {
    let (project, experiment_count) = tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let experiment_count = store.list_experiments_by_project(&project.id)?.len();
        Ok::<_, ApiError>((project, experiment_count))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("starter task failed: {e}")))??;
    let harness = q
        .harness
        .as_deref()
        .map(str::trim)
        .filter(|h| local::harness::is_chat_harness(h));
    // Past "getting started" or no chat harness named: nothing to offer
    // (empty), as opposed to a harness that could not answer (null).
    let Some(harness) = harness.filter(|_| experiment_count == 0) else {
        return Ok(Json(json!({ "prompts": [], "blank": false })));
    };
    let locale = q.locale.as_deref().unwrap_or("en");
    let model = selected_chat_model(&state, harness, q.profile.as_deref(), q.model.as_deref())
        .await
        .map_err(bad_request)?;
    let agent = local::starter::Agent {
        profile: q.profile,
        harness: harness.to_string(),
        model,
    };
    let (prompts, blank) = match local::starter::prompts(&project, &agent, locale).await {
        local::starter::Starter::Blank => (Some(Vec::new()), true),
        local::starter::Starter::Generated(prompts) => (prompts, false),
    };
    Ok(Json(json!({ "prompts": prompts, "blank": blank })))
}

/// Live uncommitted changes in the project's clone (the agent's working
/// tree), mapped back to the experiment whose branch is checked out.
async fn project_working_tree(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let repo = std::path::Path::new(&project.repo_path);
        let (branch, payload) = local::git::working_tree_diff(repo)?;
        let experiment_id = match &branch {
            Some(b) => store
                .list_experiments_by_project(&project.id)?
                .into_iter()
                .find(|e| &e.branch_name == b)
                .map(|e| e.id),
            None => None,
        };
        Ok(Json(json!({
            "branch": branch,
            "experimentId": experiment_id,
            "diff": payload.diff,
            "truncated": payload.truncated,
        })))
    })
    .await
}

/// Live view of one chat session's private worktree — what the agent has
/// changed, before any run exists. Unlike `project_working_tree` (clone-scoped,
/// diffed against HEAD), the session worktree starts detached on the baseline
/// tip and the agent commits to experiment branches, so "what it changed" is
/// the working tree diffed against the merge-base of the baseline and HEAD; a
/// bare HEAD diff would hide every committed edit. Read-only throughout: no
/// index-touching (`git add -N`) that would mutate the agent's checkout.
///
/// A never-started session (worktree is lazy) or a pruned worktree degrades to
/// `resolve_checkout_root`'s clone fallback; we report `{ exists: false }`
/// rather than pass off the clone's contents as the session's work.
async fn session_worktree(Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let session = store
            .get_chat_session(&id)?
            .ok_or_else(|| not_found("chat session"))?;
        let project = store
            .get_local_project(&session.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let (root, root_kind) = resolve_checkout_root(&store, &project, Some(&id))?;
        if root_kind != "worktree" {
            return Ok(Json(json!({ "exists": false })));
        }
        let branch = local::git::current_branch(&root);
        // Diff against the merge-base of the baseline tip and HEAD — the fork
        // point of the agent's work. Every step that can't resolve (unrelated
        // histories, unborn HEAD) falls back to HEAD, so
        // the diff degrades to "uncommitted only" rather than erroring.
        let baseline = &project.baseline_branch;
        let base =
            local::git::merge_base(&root, baseline, "HEAD")?.unwrap_or_else(|| "HEAD".to_string());
        let files = local::git::changed_files(&root, &base)?;
        let payload = local::git::working_tree_diff_against(&root, Some(&base))?;
        Ok(Json(json!({
            "exists": true,
            "branch": branch,
            "baselineBranch": baseline,
            "baseSha": base,
            "files": files,
            "diff": diff_json(payload),
        })))
    })
    .await
}

/// Cap on file bytes served to the viewer (mirrors openresearch.sh).
const FILE_READ_LIMIT: u64 = 512_000;

/// Resolve which on-disk checkout answers a file/code request for a project.
///
/// The chat session's worktree is where the agent actually works, so it can
/// hold files the hub clone's checkout never sees. When `session_id` is given
/// it must be this project's session (the authorization boundary, which also
/// pins the worktree dir to a store-issued id); a missing worktree (pruned, or
/// never created) degrades to the clone rather than erroring, but any other
/// worktree failure is reported, not papered over. Returns the canonicalized
/// root and whether it is the `"worktree"` or the `"clone"`.
fn resolve_checkout_root(
    store: &Store,
    project: &local::model::LocalProject,
    session_id: Option<&str>,
) -> std::result::Result<(std::path::PathBuf, &'static str), ApiError> {
    let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
    let worktree = match session_id {
        Some(s) => {
            let session = store
                .get_chat_session(s)?
                .filter(|sess| sess.project_id == project.id)
                .ok_or_else(|| not_found("chat session"))?;
            let dir = local::git::existing_session_worktree_path(project, &session.id);
            match crate::paths::canonicalize(&dir) {
                Ok(p) => Some(p),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(ApiError::from(anyhow!("session worktree unavailable: {e}"))),
            }
        }
        None => None,
    };
    match worktree {
        Some(r) => Ok((r, "worktree")),
        None => Ok((
            crate::paths::canonicalize(&project.repo_path)
                .map_err(|e| ApiError::from(anyhow!("repo clone unavailable: {e}")))?,
            "clone",
        )),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodeTreeQuery {
    /// Branch to list the committed tree of; absent lists a live checkout.
    r#ref: Option<String>,
    /// Chat session whose worktree to list (the live view the Worktree tab's
    /// Files pane wants). Absent falls back to the hub clone's checkout.
    /// Mutually exclusive with `ref` — a committed tree has no live worktree.
    session_id: Option<String>,
}

/// Cap on entries returned by the code-tree listing.
const CODE_TREE_LIMIT: usize = 20_000;

/// Flat file listing for the UI code browser. With `ref`: the committed tree
/// of that branch (local ref first, then origin's), independent of any
/// checkout. Without: the hub clone's checkout via `git ls-files`, so
/// gitignored trees are excluded and untracked-but-new files are included.
/// Paths are repo-relative; the client builds the nested tree.
async fn project_code_tree(Path(id): Path<String>, Query(q): Query<CodeTreeQuery>) -> ApiResult {
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let ref_name = q.r#ref.as_deref().map(str::trim).filter(|s| !s.is_empty());
        let session_id = q
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if ref_name.is_some() && session_id.is_some() {
            return Err(bad_request("ref and sessionId are mutually exclusive"));
        }
        // Branch refs live in the shared object DB — any checkout resolves them;
        // for a live listing the session's worktree is the live view when given
        // (its untracked files the clone never sees), else the hub clone.
        let (root, root_kind) = resolve_checkout_root(&store, &project, session_id)?;
        let path = ref_name
            .is_none()
            .then(|| root.to_string_lossy().into_owned());
        let (root_kind, branch, mut entries) = match ref_name {
            Some(name) => {
                let sha = local::git::resolve_branch_commit(&root, name)?
                    .ok_or_else(|| not_found("branch"))?;
                let entries = local::git::list_tree_files(&root, &sha)?;
                ("branch", Some(name.to_string()), entries)
            }
            None => {
                let branch = local::git::current_branch(&root);
                let entries = local::git::list_worktree_files(&root)?;
                (root_kind, branch, entries)
            }
        };
        entries.sort();
        // During a merge conflict `ls-files --cached` emits an unmerged path
        // once per stage — collapse to one entry (they'd be duplicate keys).
        entries.dedup();
        let truncated = entries.len() > CODE_TREE_LIMIT;
        entries.truncate(CODE_TREE_LIMIT);
        Ok(Json(json!({
            "root": root_kind,
            "path": path,
            "branch": branch,
            "entries": entries,
            "truncated": truncated,
        })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFileQuery {
    path: String,
    /// Chat session whose worktree holds the file. Absent (or the worktree
    /// already pruned) falls back to the hub clone. Ignored when `ref` is given.
    session_id: Option<String>,
    /// Branch to read the committed file from, instead of a live checkout.
    r#ref: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectFileResponse {
    path: String,
    content: String,
    truncated: bool,
    binary: bool,
    not_found: bool,
    root: &'static str,
    presentation: local::files::FilePresentation,
    version: Option<String>,
}

impl ProjectFileResponse {
    fn missing(
        path: String,
        root: &'static str,
        presentation: local::files::FilePresentation,
    ) -> Self {
        Self {
            path,
            content: String::new(),
            truncated: false,
            binary: false,
            not_found: true,
            root,
            presentation,
            version: None,
        }
    }

    fn non_text(
        path: String,
        root: &'static str,
        presentation: local::files::FilePresentation,
    ) -> Self {
        Self {
            path,
            content: String::new(),
            truncated: false,
            binary: true,
            not_found: false,
            root,
            presentation,
            version: None,
        }
    }

    fn text(
        path: String,
        root: &'static str,
        content: String,
        truncated: bool,
        binary: bool,
        presentation: local::files::FilePresentation,
        version: Option<String>,
    ) -> Self {
        Self {
            path,
            content,
            truncated,
            binary,
            not_found: false,
            root,
            presentation,
            version,
        }
    }
}

fn file_version(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file_version_on_disk(path: &std::path::Path) -> std::result::Result<String, ApiError> {
    let mut file = std::fs::File::open(path)
        .map_err(|error| ApiError::from(anyhow!("save failed: {error}")))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)
        .map_err(|error| ApiError::from(anyhow!("save failed: {error}")))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn validated_project_file_path(
    path: &str,
) -> std::result::Result<(String, std::path::PathBuf), ApiError> {
    let rel = path.trim().trim_start_matches("./").to_string();
    if rel.is_empty() || rel.len() > 1024 {
        return Err(bad_request("invalid path"));
    }
    let rel_path = std::path::PathBuf::from(&rel);
    let traversal = rel_path.is_absolute()
        || rel_path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)));
    if traversal {
        return Err(bad_request("path must be repo-relative"));
    }
    Ok((rel, rel_path))
}

fn decode_project_file_text(bytes: Vec<u8>, truncated: bool) -> (String, bool) {
    if bytes.contains(&0) {
        return (String::new(), true);
    }
    match String::from_utf8(bytes) {
        Ok(content) => (content, false),
        Err(error) if truncated && error.utf8_error().error_len().is_none() => {
            let valid_up_to = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(valid_up_to);
            (String::from_utf8(bytes).unwrap_or_default(), false)
        }
        Err(_) => (String::new(), true),
    }
}

/// One file for the UI file viewer. With `ref`: the committed content on that
/// branch (a streamed, capped `git cat-file` read), independent of any
/// checkout. Without: the project's checkout — the chat session's worktree
/// when `sessionId` is given, else the hub clone. Path is repo-relative;
/// traversal outside the checkout is rejected. The response's `root` says
/// which source actually answered, so the UI can flag fallback.
async fn project_file(
    Path(id): Path<String>,
    Query(q): Query<ProjectFileQuery>,
) -> std::result::Result<Json<ProjectFileResponse>, ApiError> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read as _;
        let (rel, rel_path) = validated_project_file_path(&q.path)?;

        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let presentation = local::files::presentation_for_path(&rel);
        let ref_name = q.r#ref.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(name) = ref_name {
            let (root, _) = resolve_checkout_root(&store, &project, None)?;
            let sha = local::git::resolve_branch_commit(&root, name)?
                .ok_or_else(|| not_found("branch"))?;
            if !matches!(
                presentation,
                local::files::FilePresentation::Text | local::files::FilePresentation::Unknown
            ) {
                return if local::git::file_size_at(&root, &sha, &rel)?.is_some() {
                    Ok(Json(ProjectFileResponse::non_text(
                        rel,
                        "branch",
                        presentation,
                    )))
                } else {
                    Ok(Json(ProjectFileResponse::missing(
                        rel,
                        "branch",
                        presentation,
                    )))
                };
            }
            // Streamed + capped: a committed multi-GB blob must not become a
            // multi-GB allocation. Missing path is an exit-code check inside
            // the helper (`cat-file -e`) — no error-message parsing.
            return match local::git::file_bytes_at_capped(&root, &sha, &rel, FILE_READ_LIMIT)? {
                Some((bytes, truncated)) => {
                    let (content, binary) = decode_project_file_text(bytes, truncated);
                    let presentation = if binary {
                        local::files::FilePresentation::Download
                    } else {
                        local::files::FilePresentation::Text
                    };
                    Ok(Json(ProjectFileResponse::text(
                        rel,
                        "branch",
                        content,
                        truncated,
                        binary,
                        presentation,
                        None,
                    )))
                }
                None => Ok(Json(ProjectFileResponse::missing(
                    rel,
                    "branch",
                    presentation,
                ))),
            };
        }
        let (root, root_kind) = resolve_checkout_root(&store, &project, q.session_id.as_deref())?;
        // Canonicalize so symlinks can't escape the checkout.
        let full = match crate::paths::canonicalize(root.join(&rel_path)) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Json(ProjectFileResponse::missing(
                    rel,
                    root_kind,
                    presentation,
                )))
            }
            Err(e) => return Err(ApiError::from(anyhow!("read failed: {e}"))),
        };
        if !full.starts_with(&root) {
            return Err(bad_request("path escapes repository"));
        }
        if full.is_dir() {
            return Err(bad_request("path is a directory"));
        }
        if !matches!(
            presentation,
            local::files::FilePresentation::Text | local::files::FilePresentation::Unknown
        ) {
            return Ok(Json(ProjectFileResponse::non_text(
                rel,
                root_kind,
                presentation,
            )));
        }
        let file = match std::fs::File::open(&full) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Json(ProjectFileResponse::missing(
                    rel,
                    root_kind,
                    presentation,
                )))
            }
            Err(e) => return Err(ApiError::from(anyhow!("read failed: {e}"))),
        };
        let mut buf = Vec::new();
        std::io::Read::take(file, FILE_READ_LIMIT + 1)
            .read_to_end(&mut buf)
            .map_err(|e| ApiError::from(anyhow!("read failed: {e}")))?;
        let truncated = buf.len() as u64 > FILE_READ_LIMIT;
        buf.truncate(FILE_READ_LIMIT as usize);
        let (content, binary) = decode_project_file_text(buf, truncated);
        let version = (!truncated && !binary).then(|| file_version(content.as_bytes()));
        let presentation = if binary {
            local::files::FilePresentation::Download
        } else {
            local::files::FilePresentation::Text
        };
        Ok(Json(ProjectFileResponse::text(
            rel,
            root_kind,
            content,
            truncated,
            binary,
            presentation,
            version,
        )))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))?
}

/// Upper bound on a single save — generous room to grow past the read cap while
/// still bounding one write.
const FILE_WRITE_LIMIT: u64 = 8 * 1024 * 1024;

/// True when a repo-relative path steps into the `.git` metadata dir — writing
/// there (`config`, `hooks/*`) is an arbitrary-command vector. Case-insensitive
/// so `.GIT` can't slip past on macOS/Windows.
fn touches_git_dir(rel_path: &std::path::Path) -> bool {
    rel_path.components().any(
        |c| matches!(c, std::path::Component::Normal(name) if name.eq_ignore_ascii_case(".git")),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WriteProjectFileReq {
    path: String,
    content: String,
    /// Chat session whose worktree owns the file; absent writes the hub clone.
    session_id: Option<String>,
    /// Exact version returned by the read endpoint. Older clients may omit it.
    expected_version: Option<String>,
}

enum WriteProjectFileOutcome {
    Saved(Value),
    Conflict {
        current_version: Option<String>,
        exists: bool,
    },
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FileAction {
    Rename,
    Duplicate,
    Delete,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManageFileReq {
    path: String,
    action: FileAction,
    new_name: Option<String>,
    session_id: Option<String>,
}

fn validated_file_name(name: Option<&str>) -> std::result::Result<&str, ApiError> {
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    match name {
        Some(name)
            if name != "." && name != ".." && name.len() <= 255 && !name.contains(['/', '\\']) =>
        {
            Ok(name)
        }
        _ => Err(bad_request("invalid file name")),
    }
}

fn duplicate_file_name(name: &str, number: usize) -> String {
    let suffix = if number == 1 {
        " copy".to_string()
    } else {
        format!(" copy {number}")
    };
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem}{suffix}.{extension}"),
        _ => format!("{name}{suffix}"),
    }
}

/// API paths are `/`-separated; a Windows `PathBuf` would send backslashes back.
#[cfg(windows)]
fn api_rel_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(not(windows))]
fn api_rel_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
}

fn manage_local_file(
    root: &std::path::Path,
    rel: &str,
    action: FileAction,
    new_name: Option<&str>,
    protect_git_dir: bool,
) -> std::result::Result<String, ApiError> {
    let (rel, rel_path) = validated_project_file_path(rel)?;
    if protect_git_dir && touches_git_dir(&rel_path) {
        return Err(bad_request("cannot manage files under .git"));
    }
    let root = crate::paths::canonicalize(root)
        .map_err(|e| ApiError::from(anyhow!("file root unavailable: {e}")))?;
    let source = root.join(&rel_path);
    let resolved = crate::paths::canonicalize(&source).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => not_found("file"),
        _ => ApiError::from(anyhow!("file unavailable: {e}")),
    })?;
    if !resolved.starts_with(&root) {
        return Err(bad_request("path escapes file root"));
    }
    if protect_git_dir && resolved.strip_prefix(&root).is_ok_and(touches_git_dir) {
        return Err(bad_request("cannot manage files under .git"));
    }
    if resolved.is_dir() {
        return Err(bad_request("path is a directory"));
    }

    let parent = source.parent().ok_or_else(|| bad_request("invalid path"))?;
    let parent = crate::paths::canonicalize(parent)
        .map_err(|e| ApiError::from(anyhow!("parent directory unavailable: {e}")))?;
    if !parent.starts_with(&root) {
        return Err(bad_request("path escapes file root"));
    }
    if matches!(action, FileAction::Delete) {
        std::fs::remove_file(&source).map_err(|e| ApiError::from(anyhow!("delete failed: {e}")))?;
        return Ok(rel);
    }

    if matches!(action, FileAction::Duplicate)
        && std::fs::symlink_metadata(&source)
            .map_err(|e| ApiError::from(anyhow!("file unavailable: {e}")))?
            .file_type()
            .is_symlink()
    {
        return Err(bad_request("cannot duplicate a symbolic link"));
    }
    let old_name = rel_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| bad_request("invalid file name"))?;
    let destination_name = match action {
        FileAction::Rename => validated_file_name(new_name)?.to_string(),
        FileAction::Duplicate => {
            let mut number = 1;
            loop {
                let candidate = duplicate_file_name(old_name, number);
                match std::fs::symlink_metadata(parent.join(&candidate)) {
                    Ok(_) => number += 1,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break candidate,
                    Err(error) => {
                        return Err(ApiError::from(anyhow!(
                            "could not choose a copy name: {error}"
                        )))
                    }
                }
            }
        }
        FileAction::Delete => unreachable!(),
    };
    let destination_rel = rel_path.with_file_name(&destination_name);
    if protect_git_dir && touches_git_dir(&destination_rel) {
        return Err(bad_request("cannot manage files under .git"));
    }
    if destination_rel == rel_path {
        return Ok(rel);
    }
    let destination = parent.join(&destination_name);
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(bad_request("a file with that name already exists"));
        }
        Ok(_) => match crate::paths::canonicalize(&destination) {
            Ok(path) if path == resolved => {}
            Ok(_) | Err(_) => return Err(bad_request("a file with that name already exists")),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ApiError::from(anyhow!("destination unavailable: {error}"))),
    }
    match action {
        FileAction::Rename => std::fs::rename(&source, &destination)
            .map_err(|e| ApiError::from(anyhow!("rename failed: {e}")))?,
        FileAction::Duplicate => {
            std::fs::copy(&source, &destination)
                .map_err(|e| ApiError::from(anyhow!("copy failed: {e}")))?;
        }
        FileAction::Delete => unreachable!(),
    }
    Ok(api_rel_path(&destination_rel))
}

async fn manage_project_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ManageFileReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    blocking_api(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let (root, root_kind) = resolve_checkout_root(&store, &project, req.session_id.as_deref())?;
        if req.session_id.is_some() && root_kind == "clone" {
            return Err(bad_request(
                "this session's worktree is no longer available — reload the files",
            ));
        }
        let path = manage_local_file(&root, &req.path, req.action, req.new_name.as_deref(), true)?;
        Ok(Json(json!({ "ok": true, "path": path })))
    })
    .await
}

/// Overwrite an existing text file in the project's live checkout with edited
/// content. Only the worktree/clone is writable — committed branch trees have no
/// `ref` path here and stay read-only. Traversal and symlink escapes are
/// rejected by canonicalizing the target and confirming it stays under the root.
async fn write_project_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<WriteProjectFileReq>,
) -> std::result::Result<Response, ApiError> {
    reject_if_moving(&state)?;
    let outcome = tokio::task::spawn_blocking(move || {
        let (rel, rel_path) = validated_project_file_path(&req.path)?;
        if touches_git_dir(&rel_path) {
            return Err(bad_request("cannot edit files under .git"));
        }
        if req.content.len() as u64 > FILE_WRITE_LIMIT {
            return Err(bad_request("file too large to save"));
        }
        if !matches!(
            local::files::presentation_for_path(&rel),
            local::files::FilePresentation::Text | local::files::FilePresentation::Unknown
        ) {
            return Err(bad_request("not an editable text file"));
        }
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let (root, root_kind) = resolve_checkout_root(&store, &project, req.session_id.as_deref())?;
        // A session write that fell back to the clone means the worktree was
        // pruned mid-edit — refuse rather than silently write another checkout.
        if req.session_id.is_some() && root_kind == "clone" {
            return Err(bad_request(
                "this session's worktree is no longer available — reload the file",
            ));
        }
        // Canonicalize the existing target so a symlinked path can't escape the
        // checkout; a missing file means the editor's copy is stale.
        let full = match crate::paths::canonicalize(root.join(&rel_path)) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if req.expected_version.is_some() {
                    return Ok(WriteProjectFileOutcome::Conflict {
                        current_version: None,
                        exists: false,
                    });
                }
                return Err(not_found("file"));
            }
            Err(e) => return Err(ApiError::from(anyhow!("save failed: {e}"))),
        };
        if !full.starts_with(&root) {
            return Err(bad_request("path escapes repository"));
        }
        if full.is_dir() {
            return Err(bad_request("path is a directory"));
        }
        // ponytail: external writers do not share a lock; add platform file coordination if this race becomes observable.
        if let Some(expected) = req.expected_version.as_deref() {
            let current = file_version_on_disk(&full)?;
            if current != expected {
                return Ok(WriteProjectFileOutcome::Conflict {
                    current_version: Some(current),
                    exists: true,
                });
            }
        }
        std::fs::write(&full, req.content.as_bytes())
            .map_err(|e| ApiError::from(anyhow!("save failed: {e}")))?;
        // The live channel polls on a tick; a save through the dashboard is
        // told at once so a collaborator sees it without the wait.
        local::overleaf_live::nudge(&full);
        Ok(WriteProjectFileOutcome::Saved(json!({
            "ok": true,
            "root": root_kind,
            "bytesWritten": req.content.len(),
            "version": file_version(req.content.as_bytes()),
        })))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))??;

    Ok(match outcome {
        WriteProjectFileOutcome::Saved(value) => Json(value).into_response(),
        WriteProjectFileOutcome::Conflict {
            current_version,
            exists,
        } => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "file changed on disk",
                "code": "fileChanged",
                "currentVersion": current_version,
                "exists": exists,
            })),
        )
            .into_response(),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenProjectFileReq {
    path: String,
    session_id: Option<String>,
}

/// Canonical path of a checkout-relative file, confined to the checkout root.
fn confined_checkout_file(
    id: &str,
    req: &OpenProjectFileReq,
    verb: &str,
) -> Result<std::path::PathBuf, ApiError> {
    let (_, rel_path) = validated_project_file_path(&req.path)?;
    if touches_git_dir(&rel_path) {
        return Err(bad_request(format!("cannot {verb} files under .git")));
    }
    let store = Store::open()?;
    let project = store
        .get_local_project(id)?
        .ok_or_else(|| not_found("project"))?;
    let (root, _) = resolve_checkout_root(&store, &project, req.session_id.as_deref())?;
    let full = match crate::paths::canonicalize(root.join(&rel_path)) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found("file")),
        Err(e) => return Err(ApiError::from(anyhow!("{verb} failed: {e}"))),
    };
    if !full.starts_with(&root) {
        return Err(bad_request("path escapes repository"));
    }
    Ok(full)
}

/// Open a checkout file in the machine's default app for its type (the user's
/// editor for source files). Resolves the same worktree/clone the reader uses
/// and confirms the file is inside it before handing the path to the OS opener.
async fn open_project_file(
    Path(id): Path<String>,
    Json(req): Json<OpenProjectFileReq>,
) -> ApiResult {
    blocking_api(move || {
        let full = confined_checkout_file(&id, &req, "open")?;
        if full.is_dir() {
            return Err(bad_request("path is a directory"));
        }
        crate::editors::open_in_default_app(&full)
            .map_err(|e| ApiError::from(anyhow!("could not open file: {e}")))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Reveal a checkout file in the OS file manager; unlike open, directories are allowed.
async fn reveal_project_file(
    Path(id): Path<String>,
    Json(req): Json<OpenProjectFileReq>,
) -> ApiResult {
    blocking_api(move || {
        let full = if req.path == "." {
            let store = Store::open()?;
            let project = store
                .get_local_project(&id)?
                .ok_or_else(|| not_found("project"))?;
            resolve_checkout_root(&store, &project, req.session_id.as_deref())?.0
        } else {
            confined_checkout_file(&id, &req, "reveal")?
        };
        crate::editors::reveal_in_file_manager(&full)
            .map_err(|e| ApiError::from(anyhow!("could not reveal file: {e}")))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Whether this machine can compile at all, so the dashboard can render a real
/// document by default and fall back to its approximate preview when it cannot.
async fn latex_engine() -> ApiResult {
    blocking_api(move || {
        let engine = local::latex::find_engine();
        Ok(Json(json!({
            "engine": engine,
            "hint": engine.is_none().then(local::latex::install_hint),
            "installCommand": engine.is_none().then(local::latex::install_command).flatten(),
        })))
    })
    .await
}

// --- overleaf ------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverleafFileQ {
    path: String,
    session_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OverleafFileReq {
    path: String,
    session_id: Option<String>,
    /// Only on link: the project URL the user pasted.
    #[serde(default)]
    project: Option<String>,
    /// Only on sync: how the user settled files both sides changed, keyed by
    /// the checkout-relative path the panel showed them.
    #[serde(default)]
    resolve: std::collections::BTreeMap<String, String>,
    /// Only on live: open the channel again after Overleaf refused it.
    #[serde(default)]
    retry: bool,
}

fn overleaf_link_json(link: Option<&crate::store::OverleafLink>) -> Value {
    let Some(link) = link else {
        return Value::Null;
    };
    let project = link.project();
    json!({
        "projectId": project.id,
        "host": project.live_host(),
        "url": project.web_url(),
    })
}

fn overleaf_state_json(link: Option<&crate::store::OverleafLink>) -> Value {
    json!({
        "hasToken": local::overleaf::token().is_some(),
        "hasSession": local::overleaf_live::session().is_some(),
        "link": overleaf_link_json(link),
    })
}

async fn overleaf_settings() -> ApiResult {
    blocking_api(move || {
        Ok(Json(json!({
            "hasToken": local::overleaf::token().is_some(),
            "hasSession": local::overleaf_live::session().is_some(),
        })))
    })
    .await
}

#[derive(Deserialize)]
struct SetOverleafTokenReq {
    token: String,
}

/// Stored as given: the bridge is the only thing that can say whether a token
/// works, and it needs a project to say it about. `link_overleaf` is where a
/// bad token surfaces.
async fn set_overleaf_token(Json(req): Json<SetOverleafTokenReq>) -> ApiResult {
    blocking_api(move || {
        let token = req.token.trim().to_string();
        if token.is_empty() {
            return Err(bad_request("token is required"));
        }
        // A newline would add a second field to the credential line the helper
        // feeds git.
        if token
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || !c.is_ascii())
        {
            return Err(bad_request(
                "That does not look like an Overleaf token — it has spaces, line breaks, or characters a token does not contain. Copy it again from Overleaf's Account Settings.",
            ));
        }
        local::overleaf::set_token(&token)?;
        Ok(Json(json!({ "hasToken": true })))
    })
    .await
}

async fn delete_overleaf_token() -> ApiResult {
    blocking_api(move || {
        local::overleaf::clear_token()?;
        Ok(Json(json!({ "hasToken": false })))
    })
    .await
}

#[derive(Deserialize)]
struct SetOverleafSessionReq {
    session: String,
    /// The Overleaf site the cookie is for: the paper's linked host when the
    /// panel asks, www.overleaf.com from Settings.
    #[serde(default)]
    host: Option<String>,
}

/// The browser session cookie the live channel signs in with. Like the token,
/// only a connection can say whether it works; `start_overleaf_live` is where
/// a stale one surfaces.
async fn set_overleaf_session(Json(req): Json<SetOverleafSessionReq>) -> ApiResult {
    blocking_api(move || {
        let host = req.host.as_deref().unwrap_or(local::overleaf::CLOUD_HOST);
        local::overleaf_live::set_session(host, &req.session).map_err(bad_request)?;
        Ok(Json(json!({ "hasSession": true })))
    })
    .await
}

async fn delete_overleaf_session() -> ApiResult {
    blocking_api(move || {
        local::overleaf_live::clear_session()?;
        Ok(Json(json!({ "hasSession": false })))
    })
    .await
}

#[derive(Deserialize)]
struct ImportOverleafSessionReq {
    /// The site to find a cookie for; the paper's linked host, or the cloud.
    #[serde(default)]
    host: Option<String>,
}

/// Read the session cookie from a signed-in browser on this machine, so the
/// user need not paste it. Reading a Chromium store asks the Keychain for its
/// key, which is the one prompt the user sees; a store with no Overleaf cookie
/// is a 400 that tells them to sign in there or paste instead.
async fn import_overleaf_session(Json(req): Json<ImportOverleafSessionReq>) -> ApiResult {
    blocking_api(move || {
        let host = req
            .host
            .as_deref()
            .unwrap_or(local::overleaf::CLOUD_HOST)
            .to_string();
        if !local::browser_cookies::SUPPORTED {
            return Err(bad_request(
                "Importing from a browser works on macOS only. Paste the cookie instead.",
            ));
        }
        let imported = local::browser_cookies::import_session(&host)
            .map_err(bad_request)?
            .ok_or_else(|| {
                bad_request(
                    "No Overleaf session was found in a browser on this machine. Sign in to Overleaf in your browser, or paste the cookie.",
                )
            })?;
        local::overleaf_live::set_session(&host, &imported.cookie).map_err(bad_request)?;
        Ok(Json(json!({ "hasSession": true, "source": imported.source })))
    })
    .await
}

fn overleaf_live_json(key: &str, status: Option<local::overleaf_live::Status>) -> Value {
    json!({
        "key": key,
        "status": status.map(|status| status.json()),
    })
}

/// The live session a paper tab would use. Building it is also the
/// permission check: a paper that is not linked, or a path that is not a
/// paper, gets the same 400 the sync would.
struct LiveTarget {
    project: local::overleaf::Project,
    /// The paper's folder, canonical, as `resolve_project_tex` hands it out.
    dir: std::path::PathBuf,
    scope: local::overleaf_live::Scope,
}

impl LiveTarget {
    fn key(&self) -> String {
        local::overleaf_live::key_for(&self.project, &self.dir, &self.scope)
    }
}

/// The paper's folder — the Overleaf project's root, on disk.
fn paper_dir(full: &std::path::Path) -> std::result::Result<&std::path::Path, ApiError> {
    full.parent()
        .ok_or_else(|| ApiError::from(anyhow!("{} has no parent directory", full.display())))
}

fn overleaf_live_target(
    id: &str,
    path: &str,
    session_id: Option<&str>,
) -> std::result::Result<LiveTarget, ApiError> {
    let (rel, _, full) = resolve_project_tex(id, path, session_id)?;
    let link = Store::open()?
        .overleaf_link(id, &rel)?
        .ok_or_else(|| bad_request("This paper is not linked to an Overleaf project yet."))?;
    let dir = paper_dir(&full)?.to_path_buf();
    Ok(LiveTarget {
        project: link.project(),
        dir,
        scope: local::overleaf_live::Scope {
            project_id: id.to_string(),
            session_id: session_id.map(str::to_string),
            folder: local::overleaf::folder_of(&rel),
        },
    })
}

/// Open the live channel for this paper, or keep it open: the tab calls this
/// again while it stays on the file, and a session nobody asks after closes.
async fn start_overleaf_live(
    Path(id): Path<String>,
    Json(req): Json<OverleafFileReq>,
) -> ApiResult {
    blocking_api(move || {
        let target = overleaf_live_target(&id, &req.path, req.session_id.as_deref())?;
        let (host, cookie) = local::overleaf_live::session()
            .ok_or_else(|| bad_request("Add an Overleaf session cookie in Settings first."))?;
        // The cookie signs in to one site; it goes nowhere else.
        if host != target.project.live_host() {
            return Err(bad_request(format!(
                "The saved session cookie is for {host}, but this paper is linked to a project on {}. Add one for that site.",
                target.project.live_host()
            )));
        }
        let config = local::overleaf_live::Config {
            project: target.project,
            dir: target.dir,
            cookie,
            scope: target.scope,
        };
        let (key, status) = local::overleaf_live::start(config, req.retry);
        Ok(Json(overleaf_live_json(&key, Some(status))))
    })
    .await
}

async fn stop_overleaf_live(Path(id): Path<String>, Query(q): Query<OverleafFileQ>) -> ApiResult {
    blocking_api(move || {
        let key = overleaf_live_target(&id, &q.path, q.session_id.as_deref())?.key();
        local::overleaf_live::stop(&key);
        Ok(Json(overleaf_live_json(&key, None)))
    })
    .await
}

async fn overleaf_link(Path(id): Path<String>, Query(q): Query<OverleafFileQ>) -> ApiResult {
    blocking_api(move || {
        let (rel, ..) = resolve_project_tex(&id, &q.path, q.session_id.as_deref())?;
        let store = Store::open()?;
        let link = store.overleaf_link(&id, &rel)?;
        Ok(Json(overleaf_state_json(link.as_ref())))
    })
    .await
}

/// Link this `.tex` to an Overleaf project, proving the account can reach it
/// before the link is stored — so a plan without Git integration is reported
/// here, once, rather than on every later push.
async fn link_overleaf(Path(id): Path<String>, Json(req): Json<OverleafFileReq>) -> ApiResult {
    blocking_api(move || {
        let (rel, _, full) = resolve_project_tex(&id, &req.path, req.session_id.as_deref())?;
        let token = local::overleaf::token()
            .ok_or_else(|| bad_request("Add an Overleaf Git authentication token first."))?;
        let raw = req.project.unwrap_or_default();
        let project = local::overleaf::parse_project(&raw).map_err(bad_request)?;
        local::overleaf::probe(&project, &token).map_err(|e| bad_request(e.to_string()))?;
        let store = Store::open()?;
        let link = crate::store::OverleafLink {
            overleaf_project_id: project.id,
            host: project.host,
            head: String::new(),
            baseline: Default::default(),
            root: String::new(),
        };
        store.set_overleaf_link(&id, &rel, &link)?;
        // Relinked: nothing may keep writing the folder on the old project's
        // behalf, and what was agreed with it says nothing about the new one.
        local::overleaf_live::stop_dir(paper_dir(&full)?);
        Ok(Json(overleaf_state_json(Some(&link))))
    })
    .await
}

/// Validates the path but does not resolve it: unlinking a paper that has since
/// been deleted or renamed must still work, or the link would outlive any way
/// to remove it.
async fn unlink_overleaf(Path(id): Path<String>, Query(q): Query<OverleafFileQ>) -> ApiResult {
    blocking_api(move || {
        let (rel, _) = validated_project_file_path(&q.path)?;
        // An unlinked folder must stop following; the target only resolves
        // while the paper still exists, which is the only time it could be.
        if let Ok(target) = overleaf_live_target(&id, &rel, q.session_id.as_deref()) {
            local::overleaf_live::stop_dir(&target.dir);
        }
        let store = Store::open()?;
        store.clear_overleaf_link(&id, &rel)?;
        Ok(Json(overleaf_state_json(None)))
    })
    .await
}

/// Bring the paper and the linked Overleaf project into step, both ways. Like a
/// failed compile, a refusal is the answer the user came for, so it comes back
/// as a 400 carrying Overleaf's own words.
async fn sync_overleaf(Path(id): Path<String>, Json(req): Json<OverleafFileReq>) -> ApiResult {
    blocking_api(move || {
        let (rel, root, full) = resolve_project_tex(&id, &req.path, req.session_id.as_deref())?;
        let (token, link) = linked(&id, &rel)?;
        let root = root.to_string_lossy().to_string();
        // An agreement reached against another checkout says nothing about this
        // one: starting from none makes every difference a conflict, which is
        // the safe direction when we cannot tell who moved.
        let baseline = if link.root == root {
            link.baseline.clone()
        } else {
            Default::default()
        };
        let project = link.project();
        let folder = local::overleaf::folder_of(&rel);
        let mut resolutions = std::collections::BTreeMap::new();
        for (path, how) in &req.resolve {
            let how = match how.as_str() {
                "take-overleaf" => local::overleaf::Resolution::TakeOverleaf,
                "keep-local" => local::overleaf::Resolution::KeepLocal,
                other => return Err(bad_request(format!("unknown resolution {other:?}"))),
            };
            if let Some(path) = local::overleaf::from_checkout(folder.as_deref(), path) {
                resolutions.insert(path, how);
            }
        }
        // The live channel writes the same files; it waits while the sync
        // does, then starts again from what the sync agreed on.
        // Held so the channel is let go even if the sync unwinds; a folder
        // left paused would look live while syncing nothing.
        let paused = local::overleaf_live::pause(paper_dir(&full)?);
        let outcome = local::overleaf::collect(&full).and_then(|payload| {
            local::overleaf::sync(&payload, &project, &token, &baseline, &resolutions)
        });
        if let Ok(outcome) = &outcome {
            paused.synced(&outcome.baseline);
        }
        drop(paused);
        let outcome = outcome.map_err(|e| bad_request(e.to_string()))?;
        let store = Store::open()?;
        store.set_overleaf_link(
            &id,
            &rel,
            &crate::store::OverleafLink {
                head: outcome.head.clone(),
                baseline: outcome.baseline.clone(),
                root,
                ..link
            },
        )?;
        Ok(Json(json!({
            "ok": true,
            "pulled": local::overleaf::to_checkout(folder.as_deref(), &outcome.pulled),
            "pushed": local::overleaf::to_checkout(folder.as_deref(), &outcome.pushed),
            "conflicts": local::overleaf::to_checkout(folder.as_deref(), &outcome.conflicts),
            "note": outcome.note,
        })))
    })
    .await
}

/// Whether Overleaf has moved since the last sync — one `ls-remote`, no clone,
/// so a linked paper can be watched while its tab is open without a transfer
/// every time.
async fn overleaf_status(Path(id): Path<String>, Query(q): Query<OverleafFileQ>) -> ApiResult {
    blocking_api(move || {
        let (rel, ..) = resolve_project_tex(&id, &q.path, q.session_id.as_deref())?;
        let (token, link) = linked(&id, &rel)?;
        let head = local::overleaf::remote_head(&link.project(), &token)
            .map_err(|e| bad_request(e.to_string()))?;
        Ok(Json(json!({ "remoteChanged": head != link.head })))
    })
    .await
}

/// The token and link a sync needs, or the 400 that says which is missing.
fn linked(
    id: &str,
    path: &str,
) -> std::result::Result<(String, crate::store::OverleafLink), ApiError> {
    let token = local::overleaf::token()
        .ok_or_else(|| bad_request("Add an Overleaf Git authentication token first."))?;
    let link = Store::open()?
        .overleaf_link(id, path)?
        .ok_or_else(|| bad_request("This paper is not linked to an Overleaf project yet."))?;
    Ok((token, link))
}

/// The no-plan path: a page that posts the paper straight into a new Overleaf
/// project. Served rather than built in the browser because the files it
/// carries are read from the checkout, and returned as HTML because Overleaf
/// takes them as a form POST.
async fn overleaf_upload(Path(id): Path<String>, Query(q): Query<OverleafFileQ>) -> Response {
    let built = tokio::task::spawn_blocking(move || {
        let (_, _, full) = resolve_project_tex(&id, &q.path, q.session_id.as_deref())
            .map_err(|e| anyhow!("{}", e.1))?;
        let payload = local::overleaf::collect(&full)?;
        local::overleaf::upload_form_html(&payload)
    })
    .await;
    match built {
        Ok(Ok(html)) => Html(html).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Html(format!(
                "<!doctype html><meta charset=\"utf-8\"><p>{}</p>",
                local::overleaf::escape(&e.to_string())
            )),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("<!doctype html><meta charset=\"utf-8\"><p>{e}</p>")),
        )
            .into_response(),
    }
}

/// Resolve a checkout `.tex` to (normalized repo-relative path, checkout root,
/// canonical file). Confines the path exactly like `open_project_file`, and
/// refuses a session whose worktree is gone for the same reason
/// `write_project_file` does: both compiling and syncing act on the file the
/// user is actually editing.
fn resolve_project_tex(
    id: &str,
    path: &str,
    session_id: Option<&str>,
) -> std::result::Result<(String, std::path::PathBuf, std::path::PathBuf), ApiError> {
    let (rel, rel_path) = validated_project_file_path(path)?;
    if touches_git_dir(&rel_path) {
        return Err(bad_request("cannot read files under .git"));
    }
    if !rel.to_ascii_lowercase().ends_with(".tex") {
        return Err(bad_request("not a .tex file"));
    }
    let store = Store::open()?;
    let project = store
        .get_local_project(id)?
        .ok_or_else(|| not_found("project"))?;
    let (root, root_kind) = resolve_checkout_root(&store, &project, session_id)?;
    if session_id.is_some() && root_kind == "clone" {
        return Err(bad_request(
            "this session's worktree is no longer available — reload the file",
        ));
    }
    let full = match crate::paths::canonicalize(root.join(&rel_path)) {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found("file")),
        Err(e) => return Err(ApiError::from(anyhow!("could not read the file: {e}"))),
    };
    if !full.starts_with(&root) {
        return Err(bad_request("path escapes repository"));
    }
    if full.is_dir() {
        return Err(bad_request("path is a directory"));
    }
    Ok((rel, root, full))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompileLatexReq {
    path: String,
    session_id: Option<String>,
}

/// Compile a checkout `.tex` file to a PDF beside it with the machine's own
/// LaTeX engine, so the dashboard's approximate preview has an exact
/// counterpart. Resolves and confines the path exactly like `open_project_file`;
/// a compile failure is a 200 carrying the log, not an error — the log is the
/// answer the user came for.
async fn compile_project_latex(
    Path(id): Path<String>,
    Json(req): Json<CompileLatexReq>,
) -> ApiResult {
    blocking_api(move || {
        let (_, root, full) = resolve_project_tex(&id, &req.path, req.session_id.as_deref())?;
        // Asked here rather than inside `compile` so a machine that cannot build
        // *this* document gets a 400 with the install hint instead of a 500.
        if let Some(hint) = local::latex::missing_toolchain(&full) {
            return Err(bad_request(hint));
        }
        let result = local::latex::compile(&full)?;
        let pdf_path = match result.pdf.as_deref() {
            Some(pdf) => {
                Some(api_rel_path(pdf.strip_prefix(&root).map_err(|_| {
                    anyhow!("compiled PDF landed outside the checkout")
                })?))
            }
            None => None,
        };
        Ok(Json(json!({
            "ok": pdf_path.is_some(),
            "pdfPath": pdf_path,
            "engine": result.engine,
            "note": result.note,
            "hadErrors": result.had_errors,
            "log": result.log,
        })))
    })
    .await
}

enum RawProjectFileSource {
    Disk(std::fs::File),
    Git {
        repo: std::path::PathBuf,
        spec: String,
        size: u64,
    },
}

/// Byte-exact checkout file for browser-native media previews. It resolves the
/// same worktree/clone/branch source as `project_file`, but streams instead of
/// decoding or buffering the file in the API process.
async fn project_raw_file(
    Path(id): Path<String>,
    Query(q): Query<ProjectFileQuery>,
    method: Method,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    let source = tokio::task::spawn_blocking(move || {
        let (rel, rel_path) = validated_project_file_path(&q.path)?;
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let ref_name = q.r#ref.as_deref().map(str::trim).filter(|s| !s.is_empty());
        if let Some(name) = ref_name {
            let (root, _) = resolve_checkout_root(&store, &project, None)?;
            let sha = local::git::resolve_branch_commit(&root, name)?
                .ok_or_else(|| not_found("branch"))?;
            let size =
                local::git::file_size_at(&root, &sha, &rel)?.ok_or_else(|| not_found("file"))?;
            // `rel` is right here: git never follows a symlink in a tree, it
            // serves the blob at that path.
            return Ok((
                rel.clone(),
                RawProjectFileSource::Git {
                    repo: root,
                    spec: format!("{sha}:{rel}"),
                    size,
                },
            ));
        }

        let (root, _) = resolve_checkout_root(&store, &project, q.session_id.as_deref())?;
        let full = crate::paths::canonicalize(root.join(rel_path)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                not_found("file")
            } else {
                ApiError::from(anyhow!("read failed: {e}"))
            }
        })?;
        if !full.starts_with(&root) {
            return Err(bad_request("path escapes repository"));
        }
        if full.is_dir() {
            return Err(bad_request("path is a directory"));
        }
        let file =
            std::fs::File::open(&full).map_err(|e| ApiError::from(anyhow!("read failed: {e}")))?;
        // The resolved path, not `rel`: an in-repo symlink named `logo.png` must
        // not make `.env`'s bytes an image.
        Ok((
            full.to_string_lossy().into_owned(),
            RawProjectFileSource::Disk(file),
        ))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))??;

    let (type_path, source) = source;
    let presentation = local::files::presentation_for_path(&type_path);
    match source {
        RawProjectFileSource::Disk(file) => crate::commands::file_serve::disk_response(
            &type_path,
            file,
            presentation,
            &method,
            &headers,
            "no-cache",
        )
        .await
        .map_err(ApiError::from),
        RawProjectFileSource::Git { repo, spec, size } => {
            crate::commands::file_serve::git_response(
                &type_path,
                repo,
                spec,
                size,
                presentation,
                &method,
                &headers,
            )
            .await
            .map_err(ApiError::from)
        }
    }
}

#[derive(Deserialize)]
struct AbsoluteFileQuery {
    path: String,
}

/// Validate an absolute-path request and resolve a leading `~`/`~/` to the home
/// dir (the shell never expands it for us, and agents inline `~/…` paths). The
/// display string stays as typed — it's what the tab shows and what the agent
/// wrote; only the returned `PathBuf` is home-expanded. `~otheruser` is left
/// alone and simply fails the absolute check. The bind is loopback-only and the
/// server runs as the user, so any file the user could read is fair game — this
/// only rejects malformed input, not location.
fn validated_absolute_file_path(
    path: &str,
) -> std::result::Result<(String, std::path::PathBuf), ApiError> {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed.len() > 4096 {
        return Err(bad_request("invalid path"));
    }
    let resolved = match trimmed.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => crate::config::home_dir()
            .ok_or_else(|| bad_request("no home directory"))?
            .join(rest.trim_start_matches('/')),
        _ => std::path::PathBuf::from(trimmed),
    };
    if !resolved.is_absolute() {
        return Err(bad_request("path must be absolute"));
    }
    Ok((trimmed.to_string(), resolved))
}

/// One file by absolute path, for the UI file viewer — the escape hatch for a
/// file an agent references that lives outside the project's checkout and
/// artifacts (e.g. `/Users/me/.ssh/config`). Same decoded/capped body shape as
/// `project_file`; `root: "abs"`. Loopback-only and no auth, so it reads
/// whatever the user running `orx up` can read — matching how the raw variant
/// and the OS-open endpoint already expose the local disk.
async fn absolute_file(
    Query(q): Query<AbsoluteFileQuery>,
) -> std::result::Result<Json<ProjectFileResponse>, ApiError> {
    tokio::task::spawn_blocking(move || {
        use std::io::Read as _;
        let (display, abs) = validated_absolute_file_path(&q.path)?;
        let presentation = local::files::presentation_for_path(&display);
        let full = match crate::paths::canonicalize(&abs) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Json(ProjectFileResponse::missing(
                    display,
                    "abs",
                    presentation,
                )))
            }
            Err(e) => return Err(ApiError::from(anyhow!("read failed: {e}"))),
        };
        if full.is_dir() {
            return Err(bad_request("path is a directory"));
        }
        if !matches!(
            presentation,
            local::files::FilePresentation::Text | local::files::FilePresentation::Unknown
        ) {
            return Ok(Json(ProjectFileResponse::non_text(
                display,
                "abs",
                presentation,
            )));
        }
        let file = match std::fs::File::open(&full) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Json(ProjectFileResponse::missing(
                    display,
                    "abs",
                    presentation,
                )))
            }
            Err(e) => return Err(ApiError::from(anyhow!("read failed: {e}"))),
        };
        let mut buf = Vec::new();
        std::io::Read::take(file, FILE_READ_LIMIT + 1)
            .read_to_end(&mut buf)
            .map_err(|e| ApiError::from(anyhow!("read failed: {e}")))?;
        let truncated = buf.len() as u64 > FILE_READ_LIMIT;
        buf.truncate(FILE_READ_LIMIT as usize);
        let (content, binary) = decode_project_file_text(buf, truncated);
        let presentation = if binary {
            local::files::FilePresentation::Download
        } else {
            local::files::FilePresentation::Text
        };
        Ok(Json(ProjectFileResponse::text(
            display,
            "abs",
            content,
            truncated,
            binary,
            presentation,
            None,
        )))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))?
}

/// Byte-exact absolute-path file for browser-native media previews and
/// downloads — the streamed counterpart to `absolute_file`, mirroring
/// `project_raw_file` for arbitrary on-disk paths.
async fn absolute_raw_file(
    Query(q): Query<AbsoluteFileQuery>,
    method: Method,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    let (type_path, file) = tokio::task::spawn_blocking(
        move || -> std::result::Result<(String, std::fs::File), ApiError> {
            let (_, abs) = validated_absolute_file_path(&q.path)?;
            let full = crate::paths::canonicalize(&abs).map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    not_found("file")
                } else {
                    ApiError::from(anyhow!("read failed: {e}"))
                }
            })?;
            if full.is_dir() {
                return Err(bad_request("path is a directory"));
            }
            let file = std::fs::File::open(&full)
                .map_err(|e| ApiError::from(anyhow!("read failed: {e}")))?;
            // The resolved path: a symlink must not let the requested name
            // dictate the type of another file's contents.
            Ok((full.to_string_lossy().into_owned(), file))
        },
    )
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))??;
    let presentation = local::files::presentation_for_path(&type_path);
    crate::commands::file_serve::disk_response(
        &type_path,
        file,
        presentation,
        &method,
        &headers,
        "no-cache",
    )
    .await
    .map_err(ApiError::from)
}

// --- project artifacts ----------------------------------------------------

/// Listing of the project's artifacts dir — the filesystem is the source of
/// truth; this scans it fresh on every call (and creates it if missing).
async fn list_artifacts(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    blocking_api(move || {
        let _admission = state
            .project_lifecycle
            .admit(&id)
            .ok_or_else(|| bad_request("project deletion is in progress"))?;
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let listing = local::files::list(&project)?;
        Ok(Json(json!(listing)))
    })
    .await
}

async fn manage_artifact_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ManageFileReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    blocking_api(move || {
        let _admission = state
            .project_lifecycle
            .admit(&id)
            .ok_or_else(|| bad_request("project deletion is in progress"))?;
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let root = local::files::ensure_dir(&project)?;
        let path = manage_local_file(&root, &req.path, req.action, req.new_name.as_deref(), false)?;
        Ok(Json(json!({ "ok": true, "path": path })))
    })
    .await
}

#[derive(Deserialize)]
struct ArtifactPathQuery {
    path: String,
}

#[derive(Deserialize)]
struct ArtifactRevealReq {
    path: String,
}

/// Reveal an artifact file or folder in the OS file manager. Unlike checkout
/// reveals, this resolves inside the project's artifacts directory.
async fn reveal_artifact_file(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ArtifactRevealReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    blocking_api(move || {
        let _admission = state
            .project_lifecycle
            .admit(&id)
            .ok_or_else(|| bad_request("project deletion is in progress"))?;
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let full = local::files::file_path(&project, &req.path)
            .map_err(|e| ApiError::from(anyhow!("could not resolve artifact: {e}")))?;
        crate::editors::reveal_in_file_manager(&full)
            .map_err(|e| ApiError::from(anyhow!("could not reveal file: {e}")))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Delete a file or folder in the artifacts dir, by relative path.
async fn delete_artifact(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ArtifactPathQuery>,
) -> ApiResult {
    reject_if_moving(&state)?;
    blocking_api(move || {
        let _admission = state
            .project_lifecycle
            .admit(&id)
            .ok_or_else(|| bad_request("project deletion is in progress"))?;
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        local::files::delete_entry(&project, &q.path)?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

/// Raw artifact bytes, by directory-relative path. `no-cache`: the same path can
/// be rewritten in place on disk.
async fn serve_artifact(
    Path(id): Path<String>,
    Query(q): Query<ArtifactPathQuery>,
    method: Method,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    let (type_path, file) = tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        let project = store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        let path = local::files::file_path(&project, &q.path).map_err(|_| not_found("file"))?;
        let file = std::fs::File::open(&path).map_err(|_| not_found("file"))?;
        let metadata = file
            .metadata()
            .map_err(|e| ApiError::from(anyhow!("stat failed: {e}")))?;
        if !metadata.is_file() {
            return Err(not_found("file"));
        }
        // `file_path` canonicalized: type the response by what it resolved to.
        Ok((path.to_string_lossy().into_owned(), file))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("file task failed: {e}")))??;
    let presentation = local::files::presentation_for_path(&type_path);
    crate::commands::file_serve::disk_response(
        &type_path,
        file,
        presentation,
        &method,
        &headers,
        "no-cache",
    )
    .await
    .map_err(ApiError::from)
}

/// Keep update checks and telemetry delivery running for long-lived dashboards.
fn spawn_background_tasks(check_updates: bool) {
    if check_updates {
        tokio::spawn(async {
            loop {
                updates::periodic_update_pass().await;
                tokio::time::sleep(updates::PERIODIC_CHECK_INTERVAL).await;
            }
        });
    }
    tokio::spawn(async {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            crate::telemetry::retry_outbox();
        }
    });
}

#[cfg(test)]
static ACTIVE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
/// Requests, terminals and background operations in flight, which an automatic restart would cut off.
static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// An automatic restart is checking for idleness; new requests wait it out.
static DRAINING: AtomicBool = AtomicBool::new(false);
static IDLE_EXIT: AtomicBool = AtomicBool::new(false);

fn stop_admitting_if_idle(api: &api_v2::Api) -> Result<bool> {
    IDLE_EXIT.store(true, Ordering::SeqCst);
    let idle = api
        .busy()
        .map(|busy| !busy && ACTIVE.load(Ordering::SeqCst) == 0);
    if !matches!(idle, Ok(true)) {
        IDLE_EXIT.store(false, Ordering::SeqCst);
    }
    idle
}

struct Active;

impl Active {
    fn new() -> Self {
        ACTIVE.fetch_add(1, Ordering::SeqCst);
        Active
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) async fn track_active(request: axum::extract::Request, next: Next) -> Response {
    if IDLE_EXIT.load(Ordering::SeqCst) {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if matches!(
        request.uri().path(),
        "/api/events" | "/api/v2/events" | "/api/v2/hello"
    ) {
        return next.run(request).await;
    }
    // Count first, then check: paired with the drain's set-then-count, one side always sees the other.
    let active = loop {
        let active = Active::new();
        if IDLE_EXIT.load(Ordering::SeqCst) {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if !DRAINING.load(Ordering::SeqCst) {
            break active;
        }
        drop(active);
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    next.run(request).await.map(|body| {
        axum::body::Body::new(ActiveBody {
            body,
            _active: active,
        })
    })
}

/// A response body that stays `Active` until hyper has sent or dropped it, so a
/// file still streaming after its handler returned isn't cut off by a restart.
struct ActiveBody {
    body: axum::body::Body,
    _active: Active,
}

impl axum::body::HttpBody for ActiveBody {
    type Data = axum::body::Bytes;
    type Error = axum::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<std::result::Result<hyper::body::Frame<Self::Data>, Self::Error>>>
    {
        std::pin::Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.body.size_hint()
    }
}

const ORPHANED_RUN_SCAN_INTERVAL: Duration = Duration::from_secs(60);
const REMOTE_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Background task so a large first reclaim never delays startup; it releases the gate between passes.
async fn reclaim_orphaned_run_artifacts(
    moving: Arc<AtomicBool>,
    gate: Arc<tokio::sync::Mutex<()>>,
) {
    // Demo prewarm skips itself when the gate is contended at startup.
    tokio::time::sleep(Duration::from_secs(5)).await;
    // Orphans and sweep failures already logged, so a stuck one logs once.
    let mut reported = HashSet::new();
    loop {
        let mut more = false;
        if !moving.load(Ordering::SeqCst) {
            let _gate = gate.lock().await;
            if !moving.load(Ordering::SeqCst) {
                match tokio::task::spawn_blocking(|| {
                    Store::open()?.cleanup_orphaned_run_artifacts()
                })
                .await
                {
                    Ok(Ok(pass)) => {
                        more = pass.more;
                        for (run_id, error) in pass.kept {
                            if reported.insert(run_id.clone()) {
                                eprintln!("orx up: keeping run artifacts for {run_id}: {error}");
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        eprintln!("orx up: could not clean orphaned run artifacts: {error}")
                    }
                    Err(error) => {
                        eprintln!("orx up: orphaned run artifact cleanup failed: {error}")
                    }
                }
                let sweeps = [
                    (
                        "chat attachments",
                        Store::cleanup_orphaned_chat_attachments as fn(&Store) -> Result<bool>,
                    ),
                    (
                        "native chat history",
                        Store::cleanup_orphaned_native_sessions,
                    ),
                ];
                for (label, sweep) in sweeps {
                    match tokio::task::spawn_blocking(move || sweep(&Store::open()?)).await {
                        Ok(Ok(left)) => more |= left,
                        Ok(Err(error)) => {
                            if reported.insert(format!("{label}: {error}")) {
                                eprintln!("orx up: could not clean orphaned {label}: {error}");
                            }
                        }
                        Err(error) => eprintln!("orx up: orphaned {label} cleanup failed: {error}"),
                    }
                }
            }
        }
        let pause = if more {
            Duration::from_secs(1)
        } else {
            ORPHANED_RUN_SCAN_INTERVAL
        };
        tokio::time::sleep(pause).await;
    }
}

/// Runs `f` on a fresh store under the data-dir gate; `None` while the data dir is moving.
pub(crate) async fn with_gated_store<T: Send + 'static>(
    moving: &AtomicBool,
    gate: &tokio::sync::Mutex<()>,
    f: impl FnOnce(&Store) -> Result<T> + Send + 'static,
) -> Option<Result<T>> {
    if moving.load(Ordering::SeqCst) {
        return None;
    }
    let _gate = gate.lock().await;
    if moving.load(Ordering::SeqCst) {
        return None;
    }
    Some(
        tokio::task::spawn_blocking(move || f(&Store::open()?))
            .await
            .unwrap_or_else(|error| Err(anyhow!("store task failed: {error}"))),
    )
}

/// Removes queued remote run dirs; the gate is held only around store access, never over SSH.
async fn reclaim_remote_run_dirs(moving: Arc<AtomicBool>, gate: Arc<tokio::sync::Mutex<()>>) {
    // Startup's own SSH traffic (supervisor adoption, settings checks) goes first.
    tokio::time::sleep(Duration::from_secs(30)).await;
    let mut reported = HashSet::new();
    let mut plan_error = None;
    let mut report = |failures: Vec<(String, String)>| {
        for (run_id, error) in failures {
            if reported.insert(run_id.clone()) {
                eprintln!("orx up: remote cleanup for run {run_id}: {error}");
            }
        }
    };
    loop {
        match with_gated_store(&moving, &gate, local::remote_cleanup::plan).await {
            Some(Ok((batches, rejected))) => {
                plan_error = None;
                report(rejected);
                if !batches.is_empty() {
                    let results = local::remote_cleanup::clean(batches, &moving, &gate).await;
                    match with_gated_store(&moving, &gate, move |store| {
                        local::remote_cleanup::record(store, results)
                    })
                    .await
                    {
                        Some(Ok(failures)) => report(failures),
                        Some(Err(error)) => {
                            eprintln!("orx up: could not record remote cleanup: {error}")
                        }
                        // Unrecorded entries stay due and removal is idempotent.
                        None => {}
                    }
                }
            }
            Some(Err(error)) => {
                let error = error.to_string();
                if plan_error.as_ref() != Some(&error) {
                    eprintln!("orx up: could not plan remote cleanup: {error}");
                    plan_error = Some(error);
                }
            }
            None => {}
        }
        tokio::time::sleep(REMOTE_CLEANUP_INTERVAL).await;
    }
}

/// Relaunch into an update installed underneath this server once that interrupts nothing,
/// so a long-lived `orx up` stops launching runs and building sandboxes with old code.
fn spawn_restart_when_idle(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(UPDATE_SAMPLE_INTERVAL).await;
            if crate::commands::app::windowless() {
                continue;
            }
            let status = updates::status();
            // The cache can claim an install the exec target doesn't have; never restart in a loop.
            if !(status.auto_update
                && status.restart_required
                && (updates::deferred_update_due() || updates::updated_exe_on_disk().await))
            {
                continue;
            }
            if begin_restart(&state, 0, false).await {
                state.restart.notify_one();
                return;
            }
        }
    });
}

/// Serializes restart attempts; true once one committed, after which nothing reopens admission.
static RESTART_COMMITTED: tokio::sync::Mutex<bool> = tokio::sync::Mutex::const_new(false);

/// Stop admitting work and commit to a restart if nothing is in flight beyond
/// `own_requests` (the caller's), save for chat turns when `stop_agents`. Returns false, and admits again, otherwise.
async fn begin_restart(state: &AppState, own_requests: usize, stop_agents: bool) -> bool {
    let mut committed = RESTART_COMMITTED.lock().await;
    if *committed {
        return true;
    }
    DRAINING.store(true, Ordering::SeqCst);
    if ACTIVE.load(Ordering::SeqCst) <= own_requests
        && !state.data_dir_move_in_progress.load(Ordering::SeqCst)
        && state.remote_sessions.list().await.iter().all(|session| {
            matches!(
                session.status,
                RemoteSessionStatus::Disconnected
                    | RemoteSessionStatus::NeedsInstall
                    | RemoteSessionStatus::NeedsUpdate
            )
        })
        && (stop_agents || state.chat.stop_admitting_if_idle().await)
    {
        state.chat.stop_admitting();
        *committed = true;
        return true;
    }
    DRAINING.store(false, Ordering::SeqCst);
    false
}

/// Once a day, keep each agent orx can install current: through the user's own
/// updater, else with orx's copy (`harness_setup::keep_current`).
fn spawn_agent_updates(state: AppState) {
    tokio::spawn(async move {
        // Startup detection's probes go first.
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            update_agents(&state, harness_setup::keep_current).await;
            tokio::time::sleep(Duration::from_secs(24 * 60 * 60)).await;
        }
    });
}

/// One pass of [`spawn_agent_updates`]. An agent signed out on every account
/// is left alone, so its vendor's release feed hears only from agents in use;
/// one whose sign-in is unknown is still kept current. A runtime follows the
/// agent that installs it.
async fn update_agents<F>(state: &AppState, keep_current: impl Fn(&'static str) -> F)
where
    F: std::future::Future<Output = crate::error::Result<bool>>,
{
    let fresh = state
        .harnesses
        .lock()
        .await
        .as_ref()
        .is_some_and(|(at, _)| at.elapsed() < HARNESS_CACHE_TTL);
    let query = HarnessQuery {
        refresh: (!fresh).then_some(1),
        ..Default::default()
    };
    let payload = harnesses_payload(state, &query).await;
    let entries = payload["harnesses"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();

    for harness in local::harness::managed::harnesses() {
        let auth_states: Vec<&Value> = entries
            .iter()
            .filter(|entry| {
                entry["id"].as_str().is_some_and(|id| {
                    let agent = local::harness::managed::key(id);
                    agent == harness || local::harness::managed::runtime(&agent) == Some(harness)
                })
            })
            .map(|entry| &entry["authState"])
            .collect();
        let signed_out =
            !auth_states.is_empty() && auth_states.iter().all(|state| *state == "needsLogin");
        if signed_out {
            continue;
        }

        match keep_current(harness).await {
            Ok(false) => {}
            Ok(true) => {
                if let Some(info) = local::harness::detect_harness(harness).await {
                    publish_harness(state, &info).await;
                }
            }
            Err(error) => eprintln!("orx up: could not update {harness}: {error:#}"),
        }
    }
}

/// Startup summary of detected coding agents. Never blocks. It goes through
/// the same locked cache path as `/api/harnesses`, so the dashboard's first
/// call serves the preflight result instead of launching a second sweep.
fn spawn_agent_preflight(state: AppState) {
    tokio::spawn(async move {
        let payload = harnesses_payload(&state, &HarnessQuery::default()).await;
        let line: Vec<String> = payload["harnesses"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| {
                let name = h["name"].as_str().unwrap_or("agent");
                if h["agentReady"].as_bool() == Some(true) {
                    match h["account"].as_str() {
                        Some(acct) => format!("{name} ✓ ({acct})"),
                        None => format!("{name} ✓"),
                    }
                } else if h["installBroken"].as_bool() == Some(true) {
                    format!("{name} — installed but failed to run")
                } else if h["catalogPending"].as_bool() == Some(true) {
                    format!("{name} — checking…")
                } else if h["installed"].as_bool() == Some(true) {
                    format!(
                        "{name} — {}",
                        h["agentNote"].as_str().unwrap_or("not ready")
                    )
                } else {
                    format!("{name} — not installed")
                }
            })
            .collect();
        eprintln!("orx up: agents: {}", line.join(" · "));
        // The provisional pass can't vouch for readiness — this detached task
        // waits out the catalog fill before deciding there's really nothing
        // usable, so the warning reflects the settled answer.
        let mut settled = payload;
        for _ in 0..45 {
            if !payload_is_provisional(&settled) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            settled = harnesses_payload(&state, &HarnessQuery::default()).await;
        }
        let any_ready = settled["harnesses"]
            .as_array()
            .is_some_and(|all| all.iter().any(|h| h["agentReady"].as_bool() == Some(true)));
        // Still provisional after 45s means the fill never converged — stay
        // silent rather than guess "nothing ready" from clamped entries.
        if !payload_is_provisional(&settled) && !any_ready {
            eprintln!(
                "orx up: warning: no coding agent ready — install Claude Code, Codex, OpenCode, Cursor or Antigravity, then connect a local model or sign in."
            );
        }
    });
}

/// A signed-out harness is the only state that needs polling. Normal turns and
/// healthy idle sessions do no auth work; this loop merely notices a login the
/// user completed separately and wakes the UI immediately. The cache is left
/// alone: served payloads get the live auth snapshot overlaid on the way out,
/// the ready-claude gate re-detects a promoted login, and a wholesale clear
/// here used to discard in-flight catalog fills on every flap.
fn spawn_claude_auth_monitor(
    chat: Arc<ChatHost>,
    claude: Arc<local::claude::ClaudeHost>,
    fill_in_flight: Arc<std::sync::atomic::AtomicBool>,
) {
    tokio::spawn(async move {
        let mut delay = Duration::from_secs(5);
        let mut observed_generation = claude.auth_snapshot().generation;
        loop {
            tokio::time::sleep(delay).await;
            let before = claude.auth_snapshot();
            if before.generation != observed_generation {
                observed_generation = before.generation;
                if claude.claim_auth_announcement(before.generation) {
                    chat.emit_event(
                        "harness.auth",
                        json!({ "harness": "claude-code", "authState": before.state }),
                    );
                }
            }
            if fill_in_flight.load(std::sync::atomic::Ordering::Acquire)
                || before.runtime_rejected
                || matches!(
                    before.state,
                    local::harness::HarnessAuthState::Ready
                        | local::harness::HarnessAuthState::Unsupported
                ) && !before.auth_check_failed
            {
                delay = Duration::from_secs(5);
                continue;
            }
            let probe = local::harness::claude::current_auth_state().await;
            let changed = claude.observe_auth_probe(&probe);
            let after = claude.auth_snapshot();
            if after.generation != observed_generation {
                observed_generation = after.generation;
                if claude.claim_auth_announcement(after.generation) {
                    chat.emit_event(
                        "harness.auth",
                        json!({ "harness": "claude-code", "authState": after.state }),
                    );
                }
            } else if changed {
                chat.emit_event("harness.catalog", json!({}));
            }
            delay = if after.auth_check_failed {
                (delay * 2).min(Duration::from_secs(60))
            } else {
                Duration::from_secs(5)
            };
        }
    });
}

// --- env var settings -------------------------------------------------------

/// Everything in `~/.openresearch/env`, values masked. `inProcessEnv` flags
/// keys that are also set in orx up's own environment (which wins at runtime).
fn env_settings_json() -> Value {
    let vars: Vec<Value> = crate::config::list_synced_env()
        .iter()
        .map(|(key, value)| {
            let plain = crate::local::trackio::plain_value(key, value);
            json!({
                "key": key,
                "maskedValue": mask_token(plain.as_deref().unwrap_or(value)),
                "value": plain,
                "inProcessEnv": std::env::var_os(key).is_some(),
            })
        })
        .collect();
    json!({ "vars": vars })
}

async fn env_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(env_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("env task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetEnvVarReq {
    key: String,
    value: String,
}

fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

async fn set_env_var(Json(req): Json<SetEnvVarReq>) -> ApiResult {
    let key = req.key.trim().to_string();
    let value = req.value.trim().to_string();
    if !valid_env_key(&key) {
        return Err(bad_request(
            "key must be letters, digits or _, not starting with a digit",
        ));
    }
    if crate::local::shell_env::IMPORTED.contains(&key.as_str()) {
        return Err(bad_request(format!(
            "{key} is reserved by OpenResearch. To change it, set {key} in your shell and restart OpenResearch."
        )));
    }
    if value.is_empty() {
        return Err(bad_request("value is required"));
    }
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_var(&key, &value)?;
        Ok(Json(env_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("env task failed: {e}")))?
}

async fn delete_env_var(Path(key): Path<String>) -> ApiResult {
    if !valid_env_key(&key) {
        return Err(bad_request("invalid key"));
    }
    tokio::task::spawn_blocking(move || {
        crate::config::remove_synced_env_var(&key)?;
        Ok(Json(env_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("env task failed: {e}")))?
}

// --- data directory ---------------------------------------------------------

/// Current data-dir state for the Storage settings card: where it resolves,
/// whether that's the default, the path we'd fall back to, and *why* it resolves
/// where it does (so the UI can lock the field when `$ORX_DATA_DIR` forces it).
fn data_dir_json() -> Value {
    use crate::store::DataDirSource;
    let current = crate::store::data_dir();
    let default = crate::store::default_data_dir();
    let source = crate::store::data_dir_source();
    json!({
        "current": current.to_string_lossy(),
        "defaultPath": default.to_string_lossy(),
        // "On the fallback chain" = no explicit choice (env pin or saved config).
        // Env can happen to equal the default path but is still a forced override.
        "isDefault": matches!(source, DataDirSource::Xdg | DataDirSource::Default),
        // env | config | xdg | default — env means a forced override (read-only).
        "source": source,
    })
}

async fn data_dir_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(data_dir_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("data-dir task failed: {e}")))?
}

#[derive(Deserialize)]
struct DataDirReq {
    path: String,
}

/// Reject a mutation when `$ORX_DATA_DIR` is forcing the path — the config value
/// would be shadowed, so honoring the request would silently do nothing.
fn ensure_not_env_forced() -> std::result::Result<(), ApiError> {
    if crate::store::data_dir_source() == crate::store::DataDirSource::Env {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "The data directory is pinned by the ORX_DATA_DIR environment \
             variable, which overrides this setting. Unset it to choose a path here."
                .into(),
        ));
    }
    Ok(())
}

/// Pre-flight a candidate path for a **move** without committing: absolute? empty
/// target? room? Returns `{ ok, error?, treeBytes, freeBytes?, sameFilesystem }`.
async fn validate_data_dir(Json(req): Json<DataDirReq>) -> ApiResult {
    use crate::local::datadir::TargetIntent;
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(bad_request("path is required"));
    }
    tokio::task::spawn_blocking(move || {
        match crate::local::datadir::validate_target(
            std::path::Path::new(&path),
            TargetIntent::Move,
        ) {
            Ok(report) => Ok(Json(json!({
                "ok": true,
                "treeBytes": report.tree_bytes,
                "freeBytes": report.free_bytes,
                "sameFilesystem": report.same_filesystem,
            }))),
            Err(e) => Ok(Json(json!({ "ok": false, "error": e.to_string() }))),
        }
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("validate task failed: {e}")))?
}

/// Set the data dir *without moving* — for onboarding on an empty install, or
/// reconnecting to an already-populated location (a second machine, after config
/// loss). The UI routes here only when the current dir has nothing to migrate;
/// otherwise it calls `/move`. Uses `TargetIntent::Set`, which (unlike `Move`)
/// permits a populated existing dir since nothing is copied.
async fn set_data_dir(State(state): State<AppState>, Json(req): Json<DataDirReq>) -> ApiResult {
    use crate::local::datadir::TargetIntent;
    reject_if_moving(&state)?;
    ensure_not_env_forced()?;
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return Err(bad_request("path is required"));
    }
    // Validate before persisting so we never store a bad path.
    let validate_path = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::local::datadir::validate_target(
            std::path::Path::new(&validate_path),
            TargetIntent::Set,
        )
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("validate task failed: {e}")))?
    .map_err(bad_request)?;

    let lock_path = std::path::PathBuf::from(&path);
    let next_lock = tokio::task::spawn_blocking(move || DashboardLock::acquire(&lock_path))
        .await
        .map_err(|e| ApiError::from(anyhow!("dashboard lock task failed: {e}")))?
        .map_err(|error| ApiError(StatusCode::CONFLICT, error.to_string()))?;

    state.chat.shutdown_harnesses().await;
    tokio::task::spawn_blocking(move || crate::config::set_settings_data_dir(Some(path)))
        .await
        .map_err(|e| ApiError::from(anyhow!("settings task failed: {e}")))??;
    *state.dashboard_lock.lock().unwrap() = Some(next_lock);
    state.chat.shutdown_harnesses().await;
    state.reopen.store(true, Ordering::SeqCst);
    state.restart.notify_one();
    Ok(Json(data_dir_json()))
}

/// Relocate the data dir to `path`, streaming `datadir.move.*` progress events
/// over `/api/events`. Returns 202 immediately; the UI watches the SSE stream.
///
/// Concurrency safety: sets `data_dir_move_in_progress` *first*, then refuses
/// (409) if a run or chat turn is already active. The substantive store-mutating
/// handlers (`send`/`launch`, project/experiment/chat CRUD, file delete,
/// `set_data_dir`) check the flag on entry and back off, so once the move is
/// underway nothing new writes the store. Even the residual races don't lose
/// data: a request that passed its own flag check in the tiny window before this
/// one set the flag — or an unguarded incidental write (an `open_project`
/// timestamp touch, an `ssh_preflight` test row) — lands in the *old* dir, but
/// the cross-filesystem path never deletes it (it's returned as `oldPathLeft`),
/// so the write is preserved there; only the atomic same-filesystem rename
/// consumes the old dir, and that path has no copy window.
async fn move_data_dir(State(state): State<AppState>, Json(req): Json<DataDirReq>) -> Response {
    use crate::local::datadir::TargetIntent;
    use std::sync::atomic::Ordering;

    if let Err(e) = ensure_not_env_forced() {
        return e.into_response();
    }
    let path = req.path.trim().to_string();
    if path.is_empty() {
        return bad_request("path is required").into_response();
    }

    // Claim the move slot first (compare-exchange): only one move at a time, and
    // once claimed, new turns/launches see the flag and back off.
    if state
        .data_dir_move_in_progress
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return ApiError(
            StatusCode::CONFLICT,
            "A data-directory move is already in progress.".into(),
        )
        .into_response();
    }

    // Helper to release the slot on any early return.
    let release = |state: &AppState| {
        state
            .data_dir_move_in_progress
            .store(false, Ordering::SeqCst);
    };

    let data_dir_guard = state.data_dir_gate.clone().lock_owned().await;
    let source_data_dir = crate::store::data_dir();
    let move_token = uuid::Uuid::new_v4().to_string();
    if let Err(error) = crate::jobs::ssh::service::shutdown_idle().await {
        release(&state);
        return ApiError(StatusCode::CONFLICT, error.to_string()).into_response();
    }
    let (move_store, move_lock) = match Store::open().and_then(|store| {
        let lock = store.acquire_data_dir_move_lock()?;
        Ok((store, lock))
    }) {
        Ok(claim) => claim,
        Err(_) => {
            release(&state);
            return ApiError(
                StatusCode::CONFLICT,
                "Another dashboard is moving or cleaning up the data directory; retry shortly."
                    .into(),
            )
            .into_response();
        }
    };
    let move_claimed = move_store
        .claim_data_dir_move(&move_token, &state.chat.held_turn_leases())
        .unwrap_or(false);
    if !move_claimed {
        release(&state);
        return ApiError(
            StatusCode::CONFLICT,
            "Can't move while another dashboard has an active chat turn or storage move.".into(),
        )
        .into_response();
    }

    let release_move_claim = |token: &str| {
        if let Ok(store) = Store::open() {
            let _ = store.release_data_dir_move(token);
        }
    };

    // In-flight guard: block if a chat turn or a run is active right now. (The
    // flag we just set prevents *new* ones from starting past this point.)
    let busy = state.chat.busy_sessions().await;
    let active_runs = tokio::task::spawn_blocking(active_run_count)
        .await
        .unwrap_or(0);
    let active_operations = state.project_lifecycle.operation_count();
    if !busy.is_empty() || active_runs > 0 || active_operations > 0 {
        release_move_claim(&move_token);
        release(&state);
        return ApiError(
            StatusCode::CONFLICT,
            format!(
                "Can't move while work is in progress ({} active chat turn(s), \
                 {active_runs} active run(s), {active_operations} project operation(s)). \
                 Finish or stop them, then retry.",
                busy.len()
            ),
        )
        .into_response();
    }

    // Validate before kicking off the background move.
    let vpath = path.clone();
    let validated = tokio::task::spawn_blocking(move || {
        crate::local::datadir::validate_target(std::path::Path::new(&vpath), TargetIntent::Move)
    })
    .await;
    match validated {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            release_move_claim(&move_token);
            release(&state);
            return bad_request(e).into_response();
        }
        Err(e) => {
            release_move_claim(&move_token);
            release(&state);
            return ApiError::from(anyhow!("validate task failed: {e}")).into_response();
        }
    }

    let lock_target = std::path::PathBuf::from(&path);
    let next_lock = match tokio::task::spawn_blocking(move || DashboardLock::acquire(&lock_target))
        .await
    {
        Ok(Ok(lock)) => lock,
        Ok(Err(error)) => {
            release_move_claim(&move_token);
            release(&state);
            return ApiError(StatusCode::CONFLICT, error.to_string()).into_response();
        }
        Err(error) => {
            release_move_claim(&move_token);
            release(&state);
            return ApiError::from(anyhow!("dashboard lock task failed: {error}")).into_response();
        }
    };

    let chat = state.chat.clone();
    // Provider-native SQLite/session stores now live inside this directory.
    // Close every idle harness child before the filesystem begins moving it.
    chat.shutdown_harnesses().await;

    // Spawn the move on a blocking task (it does synchronous FS work); forward
    // throttled progress onto the SSE broadcast, clear the flag when done.
    let flag = state.data_dir_move_in_progress.clone();
    let dashboard_lock = state.dashboard_lock.clone();
    let (restart, reopen) = (state.restart.clone(), state.reopen.clone());
    let target = std::path::PathBuf::from(path);
    tokio::spawn(async move {
        let _data_dir_guard = data_dir_guard;
        let _move_lock = move_lock;
        use crate::local::datadir::MoveProgress;
        let chat_for_progress = chat.clone();
        // Throttle: forward at most one progress event per ~120ms of copy, but
        // always emit phase edges (copied==0 or ==total) so the first/last tick
        // of every phase gets through.
        let last = std::sync::Mutex::new(0i64);
        let on_progress = move |p: MoveProgress| {
            let now = crate::store::now_ms();
            let mut guard = last.lock().unwrap();
            let is_edge = p.copied_bytes == 0 || p.copied_bytes >= p.total_bytes;
            if is_edge || now - *guard >= 120 {
                *guard = now;
                chat_for_progress.emit_event("datadir.move.progress", json!(p));
            }
        };
        let target_for_move = target.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::local::datadir::move_data_dir(target_for_move, on_progress)
        })
        .await;

        match result {
            Ok(Ok(outcome)) => {
                *dashboard_lock.lock().unwrap() = Some(next_lock);
                // Close any child spawned while a cross-filesystem copy ran.
                chat.shutdown_harnesses().await;
                chat.emit_event("datadir.move.done", json!(outcome));
                reopen.store(true, Ordering::SeqCst);
            }
            Ok(Err(e)) => chat.emit_event("datadir.move.error", json!({ "error": e.to_string() })),
            Err(e) => chat.emit_event(
                "datadir.move.error",
                json!({ "error": format!("move task panicked: {e}") }),
            ),
        }
        // A cross-filesystem copy retains the source DB, while a rename removes
        // it. Avoid reopening a removed source path and recreating it.
        if source_data_dir.exists() {
            if let Ok(store) = Store::open_at(source_data_dir) {
                let _ = store.release_data_dir_move(&move_token);
            }
        }
        if let Ok(store) = Store::open() {
            let _ = store.release_data_dir_move(&move_token);
        }
        flag.store(false, Ordering::SeqCst);
        if reopen.load(Ordering::SeqCst) {
            restart.notify_one();
        }
    });

    (StatusCode::ACCEPTED, Json(json!({ "started": true }))).into_response()
}

/// Count runs currently in an active state (`starting`/`running`), for the
/// data-dir move's in-flight guard. SQL-side and unbounded (see
/// `Store::count_active_runs`).
fn active_run_count() -> usize {
    Store::open()
        .and_then(|s| s.count_active_runs())
        .unwrap_or(0)
}

/// Refuse an operation that would write the store while a data-dir move is in
/// progress — the move relies on nothing new touching the old dir mid-flight.
fn reject_if_moving(state: &AppState) -> std::result::Result<(), ApiError> {
    if state
        .data_dir_move_in_progress
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A data-directory move is in progress. Try again once it finishes.".into(),
        ));
    }
    Ok(())
}

fn reject_if_stopping(state: &AppState) -> std::result::Result<(), ApiError> {
    if state.stopping.load(Ordering::SeqCst) {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "The persistent OpenResearch host is stopping.".into(),
        ));
    }
    Ok(())
}

/// Older dashboards on a remote host still read and toggle this.
async fn run_engine_settings() -> ApiResult {
    Ok(Json(json!({"enabled": true, "forcedBy": null})))
}

async fn set_run_engine() -> ApiResult {
    Err(bad_request(
        "Every build now uses the new experiment engine; update this app to remove the switch.",
    ))
}

// --- git settings -----------------------------------------------------------

fn git_out(args: &[&str]) -> Option<String> {
    let out = local::git::git_command().args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn git_settings_json(github_status: local::github::Status) -> Value {
    json!({
        "gitVersion": git_out(&["--version"]),
        "userName": git_out(&["config", "--global", "user.name"]),
        "userEmail": git_out(&["config", "--global", "user.email"]),
        "ghInstalled": github_status.installed,
        "githubAuthenticated": github_status.authenticated,
    })
}

fn project_defaults_json(github_status: local::github::Status) -> Value {
    json!({
        "githubForNewProjects": crate::config::github_for_new_projects(),
        "githubDefaultPromptSeen": crate::config::github_default_prompt_seen(),
        "ghInstalled": github_status.installed,
        "githubAuthenticated": github_status.authenticated,
    })
}

async fn project_defaults() -> ApiResult {
    Ok(Json(project_defaults_json(local::github::status().await)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetProjectDefaultsReq {
    github_for_new_projects: bool,
    #[serde(default)]
    github_default_prompt_seen: Option<bool>,
}

async fn set_project_defaults(Json(req): Json<SetProjectDefaultsReq>) -> ApiResult {
    let github_status = local::github::status().await;
    if req.github_for_new_projects && !github_status.authenticated {
        return Err(bad_request(
            "Connect GitHub before enabling it by default for new projects.",
        ));
    }
    crate::config::set_github_for_new_projects(req.github_for_new_projects)?;
    if let Some(seen) = req.github_default_prompt_seen {
        crate::config::set_github_default_prompt_seen(seen)?;
    }
    Ok(Json(project_defaults_json(github_status)))
}

async fn git_settings() -> ApiResult {
    let github_status = local::github::status().await;
    tokio::task::spawn_blocking(move || Ok(Json(git_settings_json(github_status))))
        .await
        .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetGitSettingsReq {
    user_name: Option<String>,
    user_email: Option<String>,
}

async fn set_git_settings(Json(req): Json<SetGitSettingsReq>) -> ApiResult {
    let name = req.user_name.map(|s| s.trim().to_string());
    let email = req.user_email.map(|s| s.trim().to_string());
    if name.as_deref().is_none_or(str::is_empty) && email.as_deref().is_none_or(str::is_empty) {
        return Err(bad_request(
            "nothing to update: pass userName and/or userEmail",
        ));
    }
    let github_status = local::github::status().await;
    tokio::task::spawn_blocking(move || {
        for (key, value) in [("user.name", name), ("user.email", email)] {
            if let Some(v) = value.filter(|v| !v.is_empty()) {
                let ok = local::git::git_command()
                    .args(["config", "--global", key, &v])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !ok {
                    return Err(bad_request(format!("git config --global {key} failed")));
                }
            }
        }
        Ok(Json(git_settings_json(github_status)))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("git task failed: {e}")))?
}

// --- telemetry settings -----------------------------------------------------

/// Effective eligibility plus the saved preference controlled by the switch.
fn telemetry_settings_json() -> Value {
    let preference_enabled = crate::telemetry::preference_enabled();
    match crate::telemetry::effective_disabled_reason() {
        None => {
            json!({ "enabled": true, "preferenceEnabled": preference_enabled, "locked": false, "reason": null })
        }
        Some(r) => {
            let reason = r.as_str();
            let locked = !matches!(r, crate::telemetry::DisabledReason::Persisted);
            json!({ "enabled": false, "preferenceEnabled": preference_enabled, "locked": locked, "reason": reason })
        }
    }
}

async fn telemetry_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(telemetry_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("telemetry task failed: {e}")))?
}

/// A product event raised by the UI rather than by a command. Every field is
/// matched against a fixed allowlist in `telemetry`, so this local endpoint
/// cannot emit arbitrary telemetry.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiEventReq {
    name: String,
    #[serde(default)]
    choice: Option<String>,
    #[serde(default)]
    step: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    experiment: Option<String>,
    #[serde(default)]
    slot: Option<u8>,
    #[serde(default)]
    surface: Option<String>,
    #[serde(default)]
    action: Option<String>,
}

async fn record_ui_event(Json(req): Json<UiEventReq>) -> ApiResult {
    match req.name.as_str() {
        "demo_welcome_choice" => {
            if let Some(choice) = req.choice.as_deref() {
                crate::telemetry::capture_demo_welcome_choice(choice);
            }
        }
        "onboarding_step_viewed" => {
            if let Some(step) = req.step.as_deref() {
                crate::telemetry::capture_onboarding_step_viewed(step);
            }
        }
        "demo_experiment_started" => {
            if let (Some(kind), Some(experiment)) = (req.kind.as_deref(), req.experiment.as_deref())
            {
                crate::telemetry::capture_demo_experiment_started(kind, experiment);
            }
        }
        "project_starter_clicked" => {
            if let Some(slot) = req.slot {
                crate::telemetry::capture_project_starter_clicked(slot);
            }
        }
        "first_action" => {
            if let (Some(surface), Some(action)) = (req.surface.as_deref(), req.action.as_deref()) {
                crate::telemetry::capture_first_action(surface, action);
            }
        }
        _ => return Ok(Json(json!({ "ok": false }))),
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct SetTelemetryReq {
    enabled: bool,
}

async fn set_telemetry_settings(Json(req): Json<SetTelemetryReq>) -> ApiResult {
    let enabled = req.enabled;
    crate::telemetry::record_consent(enabled).await;
    tokio::task::spawn_blocking(move || {
        crate::telemetry::set_persisted_disabled(!enabled)
            .map_err(|e| ApiError::from(anyhow!("could not save telemetry setting: {e}")))?;
        Ok(Json(telemetry_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("telemetry task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetLocaleReq {
    locale: String,
}

async fn set_dashboard_locale(Json(SetLocaleReq { locale }): Json<SetLocaleReq>) -> ApiResult {
    // Attached to every later batch, so a value the server rejects would drop them all.
    let valid = (2..=16).contains(&locale.len())
        && locale
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !valid {
        return Err(bad_request(format!("invalid locale: {locale:?}")));
    }
    tokio::task::spawn_blocking(move || {
        crate::telemetry::set_dashboard_locale(&locale)
            .map_err(|e| ApiError::from(anyhow!("could not save the dashboard locale: {e}")))?;
        crate::telemetry::capture("dashboard_locale", json!({}));
        Ok(Json(json!({ "locale": locale })))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("locale task failed: {e}")))?
}

/// What a quit would interrupt, for the page's close dialog.
async fn pending_work() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(json!(Store::open()?.pending_work()?))))
        .await
        .map_err(|e| ApiError::from(anyhow!("pending work task failed: {e}")))?
}

// --- updates -----------------------------------------------------------------

async fn update_status() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(json!(updates::status()))))
        .await
        .map_err(|e| ApiError::from(anyhow!("update status task failed: {e}")))?
}

async fn update_notes() -> ApiResult {
    Ok(Json(json!(updates::fetch_build_notes().await?)))
}

#[derive(Deserialize)]
struct SetAutoUpdateReq {
    enabled: bool,
}

async fn set_auto_update(Json(req): Json<SetAutoUpdateReq>) -> ApiResult {
    let enabled = req.enabled;
    tokio::task::spawn_blocking(move || {
        crate::config::set_auto_update_enabled(enabled)
            .map_err(|e| ApiError::from(anyhow!("could not save the auto-update setting: {e}")))?;
        Ok(Json(json!(updates::status())))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("auto-update task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetUpdateChannelReq {
    channel: updates::ReleaseChannel,
}

async fn set_update_channel(Json(req): Json<SetUpdateChannelReq>) -> ApiResult {
    if !updates::current_channel().is_ok_and(|channel| channel.has_release_channels()) {
        return Err(bad_request(
            "Update channels are available for desktop apps.",
        ));
    }
    tokio::task::spawn_blocking(move || {
        let path = updates::updater_lock_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ApiError::from(anyhow!(e)))?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| ApiError::from(anyhow!(e)))?;
        let mut lock = fd_lock::RwLock::new(file);
        let _guard = lock.try_write().map_err(|_| {
            ApiError(
                StatusCode::CONFLICT,
                "An update is in progress. Try again when it finishes.".into(),
            )
        })?;
        if req.channel != crate::telemetry::update_channel() {
            if updates::channel_change_blocked_by_installed() {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "An update is already installed. Restart OpenResearch before changing channels."
                        .into(),
                ));
            }
            updates::clear_deferred_update();
        }
        crate::telemetry::set_update_channel(req.channel)
            .map_err(|e| ApiError::from(anyhow!("could not save update channel: {e}")))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("update channel task failed: {e}")))??;
    updates::periodic_update_pass().await;
    Ok(Json(json!(updates::status())))
}

/// Apply an update now, for the user who doesn't want to wait for the next
/// periodic pass. Runs the same detached updater, so it can't race one already
/// in flight — the updater's file lock settles that.
async fn apply_update() -> ApiResult {
    updates::apply_now().await?;
    Ok(Json(json!(updates::status())))
}

/// Relaunch into the copy the updater already installed. Answers first, then
/// restarts, so the dashboard learns the request landed before the connection
/// drops; it reloads once the new server is up.
/// Sent by the dashboard once the user agreed to stop the agents that are working; older pages post no body.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestartReq {
    stop_agents: bool,
}

async fn restart_after_update(
    State(state): State<AppState>,
    req: Option<Json<RestartReq>>,
) -> ApiResult {
    let stop_agents = req.is_some_and(|Json(req)| req.stop_agents);
    let status = tokio::task::spawn_blocking(updates::status)
        .await
        .map_err(|e| ApiError::from(anyhow!("update status task failed: {e}")))?;
    let Some(version) = status.installed_version else {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "no newer orx is installed to restart into".into(),
        ));
    };
    // A dashboard poll landing alongside the click is not work worth refusing over.
    let mut began = false;
    for _ in 0..10 {
        began = begin_restart(&state, 1, stop_agents).await;
        if began {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !began {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "OpenResearch is busy: a chat turn, queued message, approval, open terminal, \
             remote session, or request is still in progress. Try again once it finishes."
                .into(),
        ));
    }
    let restart = state.restart.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        restart.notify_one();
    });
    Ok(Json(json!({ "restarting": true, "version": version })))
}

#[derive(Deserialize)]
struct InstallCliReq {
    /// Replace an `orx` that is already on PATH. The card only sends this after
    /// showing the user what it would displace.
    #[serde(default)]
    force: bool,
}

/// Link the app's `orx` onto the user's PATH (Settings → Updates).
async fn install_cli(Json(req): Json<InstallCliReq>) -> ApiResult {
    let installed =
        tokio::task::spawn_blocking(move || crate::commands::install_cli::install(req.force))
            .await
            .map_err(|e| ApiError::from(anyhow!("install-cli task failed: {e}")))??;
    Ok(Json(json!({
        "link": installed.link.to_string_lossy(),
        "dir": installed.link.parent().unwrap_or(&installed.link).to_string_lossy(),
        "target": installed.target.to_string_lossy(),
        "onPath": installed.on_path,
        "alreadyCurrent": installed.already_current,
    })))
}

fn profile_settings_json() -> Value {
    json!(crate::telemetry::load_profile())
}

async fn profile_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(profile_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("profile task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetProfileReq {
    #[serde(default)]
    research_areas: Vec<String>,
    #[serde(default)]
    other_area: Option<String>,
    #[serde(default)]
    background: Option<String>,
    #[serde(default)]
    papers: Vec<crate::telemetry::ProfilePaper>,
}

fn profile_settings_update(
    req: SetProfileReq,
    current: crate::telemetry::ResearchProfile,
) -> std::result::Result<crate::telemetry::ResearchProfile, ApiError> {
    if req.research_areas.is_empty() {
        Ok(crate::telemetry::ResearchProfile {
            research_areas: current.research_areas,
            other_area: current.other_area,
            background: req.background.filter(|value| !value.trim().is_empty()),
            papers: req.papers,
        })
    } else {
        normalize_research_profile(
            req.research_areas,
            req.other_area,
            req.background,
            req.papers,
        )
    }
}

async fn set_profile_settings(Json(req): Json<SetProfileReq>) -> ApiResult {
    let profile = profile_settings_update(req, crate::telemetry::load_profile())?;
    tokio::task::spawn_blocking(move || {
        crate::telemetry::set_profile(profile)
            .map_err(|e| ApiError::from(anyhow!("could not save profile: {e}")))?;
        Ok(Json(profile_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("profile task failed: {e}")))?
}

async fn ui_state(State(state): State<AppState>) -> ApiResult {
    let mut preferences = Store::open()?.ui_state()?;
    if let Some(selection) = preferences.preferred_agent.as_mut() {
        if selection
            .model
            .as_deref()
            .is_none_or(|model| !local::harness::concrete_model(model))
        {
            if let Some((_, catalog)) = state.harnesses.lock().await.as_ref() {
                selection.model = local::harness::catalog_default(
                    catalog,
                    &selection.harness,
                    selection.profile.as_deref(),
                );
            }
        }
    }
    Ok(Json(json!(preferences)))
}

async fn project_ui_state(Path(id): Path<String>) -> ApiResult {
    tokio::task::spawn_blocking(move || -> ApiResult {
        let store = Store::open()?;
        store
            .get_local_project(&id)?
            .ok_or_else(|| not_found("project"))?;
        Ok(Json(json!(store.project_workspace_state(&id)?)))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("workspace task failed: {error}")))?
}

async fn set_project_ui_state(
    Path(id): Path<String>,
    Json(workspace): Json<WorkspaceState>,
) -> ApiResult {
    workspace.validate().map_err(bad_request)?;
    tokio::task::spawn_blocking(move || -> ApiResult {
        if !Store::open()?.set_project_workspace_state(&id, &workspace)? {
            return Err(not_found("project"));
        }
        Ok(Json(json!(workspace)))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("workspace task failed: {error}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetUiStateReq {
    #[serde(default)]
    tour_completed: Option<bool>,
    #[serde(default)]
    preferred_agent: Option<StoredAgentSelectionReq>,
    workspace: Option<GlobalWorkspaceState>,
    preferred_autonomy: Option<Autonomy>,
    nested_delegation: Option<bool>,
    opencode_free_models: Option<bool>,
    favorite_models: Option<Vec<String>>,
    hidden_models: Option<Vec<String>>,
    hidden_agents: Option<Vec<String>>,
    model_section_order: Option<Vec<String>>,
    collapsed_model_sections: Option<Vec<String>>,
    model_sightings: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAgentSelectionReq {
    profile: Option<String>,
    harness: String,
    model: Option<String>,
    service_tier: Option<String>,
    permission_mode: Option<String>,
    reasoning_level: Option<String>,
}

async fn set_ui_state(
    State(state): State<AppState>,
    Json(mut req): Json<SetUiStateReq>,
) -> ApiResult {
    if let Some(selection) = req.preferred_agent.as_mut() {
        selection.model = selected_chat_model(
            &state,
            &selection.harness,
            selection.profile.as_deref(),
            selection.model.as_deref(),
        )
        .await
        .map_err(bad_request)?;
    }
    if let Some(workspace) = &req.workspace {
        workspace.validate().map_err(bad_request)?;
    }
    let switched = req.hidden_agents.is_some();
    let lists = [
        (
            "favorite_models",
            req.favorite_models.take(),
            500,
            200,
            "Favorite models",
        ),
        (
            "hidden_models",
            req.hidden_models.take(),
            2000,
            200,
            "Hidden models",
        ),
        (
            "hidden_agents",
            req.hidden_agents.take(),
            2000,
            200,
            "Hidden agents",
        ),
        (
            "model_section_order",
            req.model_section_order.take(),
            200,
            80,
            "Model sections",
        ),
        (
            "collapsed_model_sections",
            req.collapsed_model_sections.take(),
            200,
            80,
            "Collapsed sections",
        ),
    ];
    for (_, list, entries, chars, name) in &lists {
        if list.as_ref().is_some_and(|list| {
            list.len() > *entries
                || list
                    .iter()
                    .any(|item| item.trim().is_empty() || item.len() > *chars)
        }) {
            return Err(bad_request(format!(
                "{name} must be up to {entries} names of up to {chars} characters"
            )));
        }
    }
    if req
        .model_sightings
        .as_ref()
        .is_some_and(|sightings| !sightings.is_object() || sightings.to_string().len() > 512 * 1024)
    {
        return Err(bad_request(
            "Model sightings must be a JSON object of up to 512 KiB",
        ));
    }
    let saved = tokio::task::spawn_blocking(move || -> Result<Json<Value>> {
        let store = Store::open()?;
        let selection = req
            .preferred_agent
            .map(|selection| {
                if !local::harness::is_chat_harness(&selection.harness) {
                    return Err(anyhow!("unknown agent: {}", selection.harness));
                }
                let nonempty = |value: Option<String>| value.filter(|item| !item.trim().is_empty());
                let permission_mode = nonempty(selection.permission_mode);
                let service_tier = nonempty(selection.service_tier);
                if permission_mode.as_deref().is_some_and(|mode| {
                    local::harness::permission_mode_for(&selection.harness, mode).is_none()
                }) {
                    return Err(anyhow!("invalid permission mode for the selected agent"));
                }
                if service_tier.as_deref().is_some_and(|tier| {
                    local::harness::service_tier_for(&selection.harness, tier).is_none()
                }) {
                    return Err(anyhow!("invalid speed for the selected agent"));
                }
                if let Some(id) = &selection.profile {
                    local::agent_profiles::home(id, &selection.harness)?;
                }
                Ok(StoredAgentSelection {
                    profile: selection.profile,
                    harness: selection.harness.clone(),
                    model: nonempty(selection.model),
                    service_tier,
                    permission_mode: preferred_permission_mode(&selection.harness, permission_mode),
                    reasoning_level: nonempty(selection.reasoning_level),
                })
            })
            .transpose()?;
        if let Some(completed) = req.tour_completed {
            store.set_tour_completed(completed)?;
        }
        if let Some(selection) = selection {
            store.set_preferred_agent(&selection)?;
        }
        if let Some(autonomy) = req.preferred_autonomy {
            store.set_preferred_autonomy(autonomy)?;
        }
        if let Some(allowed) = req.nested_delegation {
            store.set_nested_delegation(allowed)?;
        }
        if let Some(offered) = req.opencode_free_models {
            store.set_opencode_free_models(offered)?;
        }
        for (column, list, ..) in lists {
            if let Some(list) = list {
                store.set_ui_json(column, &list)?;
            }
        }
        if let Some(sightings) = &req.model_sightings {
            store.set_ui_json("model_sightings", sightings)?;
        }
        if let Some(workspace) = req.workspace {
            store.set_global_workspace_state(&workspace)?;
        }
        Ok(Json(json!(store.ui_state()?)))
    })
    .await
    .map_err(|error| ApiError::from(anyhow!("UI state task failed: {error}")))?
    .map_err(bad_request)?;
    // Each `/api/harnesses` entry says whether it is turned off.
    if switched {
        state.chat.emit_event("harness.catalog", json!({}));
    }
    Ok(saved)
}

/// The lit-source toggles as booleans (enabled = not in the disabled set).
fn lit_sources_json() -> Value {
    let disabled = crate::config::disabled_lit_sources();
    let enabled = |name: &str| !disabled.iter().any(|d| d == name);
    json!({
        "alphaxiv": enabled(crate::LitSource::Alphaxiv.as_str()),
        "openalex": enabled(crate::LitSource::Openalex.as_str()),
        "biorxiv": enabled(crate::LitSource::Biorxiv.as_str()),
        "pubmed": enabled(crate::LitSource::Pubmed.as_str()),
    })
}

async fn lit_sources_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(lit_sources_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("lit-sources task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetLitSourcesReq {
    alphaxiv: bool,
    openalex: bool,
    biorxiv: bool,
    pubmed: bool,
}

async fn set_lit_sources_settings(Json(req): Json<SetLitSourcesReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let mut disabled = Vec::new();
        for (enabled, source) in [
            (req.alphaxiv, crate::LitSource::Alphaxiv),
            (req.openalex, crate::LitSource::Openalex),
            (req.biorxiv, crate::LitSource::Biorxiv),
            (req.pubmed, crate::LitSource::Pubmed),
        ] {
            if !enabled {
                disabled.push(source.as_str().to_string());
            }
        }
        crate::telemetry::set_disabled_lit_sources(disabled)
            .map_err(|e| ApiError::from(anyhow!("could not save literature sources: {e}")))?;
        Ok(Json(lit_sources_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("lit-sources task failed: {e}")))?
}

// --- ssh hosts ----------------------------------------------------------------

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SshConnectBackend {
    Ssh,
    Slurm,
}

#[derive(Deserialize)]
pub(crate) struct SshConnectReq {
    pub(crate) host: String,
    pub(crate) backend: SshConnectBackend,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SshTerminalInput {
    Resize { cols: u16, rows: u16 },
}

enum PtyEvent {
    Output(Vec<u8>),
    Eof,
    Exit(std::result::Result<portable_pty::ExitStatus, String>),
}

struct PtySession {
    master: Box<dyn MasterPty + Send>,
    input: std::sync::mpsc::Sender<Vec<u8>>,
    events: mpsc::Receiver<PtyEvent>,
    kill: std::sync::mpsc::Sender<()>,
}

struct PtyChildGuard {
    kill: std::sync::mpsc::Sender<()>,
    running: bool,
}

impl Drop for PtyChildGuard {
    fn drop(&mut self) {
        if self.running {
            let _ = self.kill.send(());
        }
    }
}

fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    origin == format!("http://{host}") || origin == format!("https://{host}")
}

fn ssh_connect_failure(host: &str, error: String) -> SshHostTest {
    SshHostTest {
        host: host.to_string(),
        reachable: false,
        tools_found: false,
        missing_tools: Vec::new(),
        error: Some(error),
        tested_at: now_ms(),
    }
}

async fn send_ssh_connect_error(
    socket: &mut WebSocket,
    host: &str,
    backend: SshConnectBackend,
    error: String,
) {
    if matches!(backend, SshConnectBackend::Ssh) {
        record_ssh_host_test(&ssh_connect_failure(host, error.clone())).await;
    }
    let _ = socket
        .send(Message::Text(
            json!({ "type": "error", "error": error })
                .to_string()
                .into(),
        ))
        .await;
    close_socket(socket).await;
}

pub(crate) async fn ssh_connect(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    Query(req): Query<SshConnectReq>,
) -> Response {
    let target = crate::jobs::ssh::SshTarget::alias(req.host.trim());
    ssh_connect_to_target(headers, ws, req, target).await
}

pub(crate) async fn ssh_connect_to_target(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    req: SshConnectReq,
    target: crate::jobs::ssh::SshTarget,
) -> Response {
    if !same_origin(&headers) {
        return ApiError(StatusCode::FORBIDDEN, "SSH terminal origin rejected".into())
            .into_response();
    }
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return bad_request("host is required").into_response();
    }
    let active = Active::new();
    ws.on_upgrade(move |socket| async move {
        let _active = active;
        ssh_connect_socket(socket, host, req.backend, target).await
    })
}

/// Open a terminal that SSHes to `target` and runs a fixed `command`. Used by
/// the remote gateway's repair flow: the command is server-derived, so unlike
/// [`ssh_connect_to_target`] the client never supplies it, and no preflight
/// runs afterwards.
pub(crate) async fn ssh_run_to_target(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    host: String,
    target: crate::jobs::ssh::SshTarget,
    command: String,
) -> Response {
    if !same_origin(&headers) {
        return ApiError(StatusCode::FORBIDDEN, "SSH terminal origin rejected".into())
            .into_response();
    }
    let active = Active::new();
    ws.on_upgrade(move |socket| async move {
        let _active = active;
        ssh_run_socket(socket, host, target, command).await
    })
}

const DEFAULT_PTY_SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};

fn start_pty(program: &str, args: Vec<String>) -> Result<PtySession> {
    start_pty_with_env(program, args, &[], DEFAULT_PTY_SIZE, None)
}

fn start_pty_with_env(
    program: &str,
    args: Vec<String>,
    env: &[(&str, std::ffi::OsString)],
    size: PtySize,
    cwd: Option<&std::path::Path>,
) -> Result<PtySession> {
    use std::io::{Read as _, Write as _};

    let pair = native_pty_system().openpty(size)?;
    let mut command = CommandBuilder::new(program);
    command.args(args);
    // App mode never puts the imported shell env into the process env, so a
    // child sees launchd's PATH and config dirs unless they are exported here.
    if let Some(path) = local::shell_env::search_path() {
        command.env("PATH", path);
    }
    local::shell_env::export_to(|key, value| command.env(key, value));
    for (key, value) in local::shell_env::host_gui_env() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        }
    }
    command.env("TERM", "xterm-256color");
    command.env_remove("NO_COLOR");
    command.env_remove("FORCE_COLOR");
    if let Some(cwd) = cwd {
        command.cwd(cwd);
    }
    for (key, value) in env {
        command.env(key, value);
    }
    if program == "ssh" {
        command.env("ORX_SSH_PROBE", "1");
    }
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let (input_tx, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    let (kill_tx, kill_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel(64);

    // ConPTY asks where the cursor is and holds the child's output, and its own
    // exit, until a terminal answers; a client that leaves first is answered here.
    let cursor_query_pending = Arc::new(AtomicBool::new(false));
    let query_seen = cursor_query_pending.clone();
    let output_tx = event_tx.clone();
    std::thread::spawn(move || {
        let mut buf = [0; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if buf[..n].windows(4).any(|bytes| bytes == b"\x1b[6n") {
                        query_seen.store(true, Ordering::SeqCst);
                    }
                    if output_tx
                        .blocking_send(PtyEvent::Output(buf[..n].to_vec()))
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = output_tx.blocking_send(PtyEvent::Eof);
    });
    std::thread::spawn(move || {
        while let Ok(bytes) = input_rx.recv() {
            cursor_query_pending.store(false, Ordering::SeqCst);
            if writer
                .write_all(&bytes)
                .and_then(|_| writer.flush())
                .is_err()
            {
                return;
            }
        }
        if cursor_query_pending.load(Ordering::SeqCst) {
            let _ = writer.write_all(b"\x1b[1;1R").and_then(|_| writer.flush());
        }
    });
    std::thread::spawn(move || loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = event_tx.blocking_send(PtyEvent::Exit(Ok(status)));
                return;
            }
            Err(error) => {
                let _ = event_tx.blocking_send(PtyEvent::Exit(Err(error.to_string())));
                return;
            }
            Ok(None) => {}
        }
        match kill_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let status = child
                    .kill()
                    .and_then(|_| child.wait())
                    .map_err(|error| error.to_string());
                let _ = event_tx.blocking_send(PtyEvent::Exit(status));
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    });

    Ok(PtySession {
        master: pair.master,
        input: input_tx,
        events: event_rx,
        kill: kill_tx,
    })
}

/// The dashboard's Connect: the engine logs in through its own connection, and each
/// password, passphrase or MFA question is asked in this terminal.
async fn ssh_connect_socket(
    mut socket: WebSocket,
    host: String,
    backend: SshConnectBackend,
    _target: crate::jobs::ssh::SshTarget,
) {
    type Question = (String, tokio::sync::oneshot::Sender<String>);
    let (questions_tx, mut questions) = mpsc::channel::<Question>(1);
    let prompt: crate::engine::transport::Prompt = Arc::new(move |question| {
        let questions = questions_tx.clone();
        Box::pin(async move {
            let (reply, answer) = tokio::sync::oneshot::channel();
            questions
                .send((question, reply))
                .await
                .map_err(|_| anyhow!("The terminal closed"))?;
            Ok(answer.await.ok())
        })
    });
    let backend_name = match backend {
        SshConnectBackend::Ssh => "ssh",
        SshConnectBackend::Slurm => "slurm",
    };
    let connect = api_v2::check_compute(
        "connect",
        json!({"backend": backend_name, "host": host}),
        Some(prompt),
    );
    tokio::pin!(connect);
    let mut pending: Option<tokio::sync::oneshot::Sender<String>> = None;
    let mut answer = String::new();
    let result = loop {
        tokio::select! {
            result = &mut connect => break result,
            Some((question, reply)) = questions.recv() => {
                let clean: String = question.chars().filter(|c| !c.is_control()).collect();
                if socket
                    .send(Message::Binary(format!("\r\n{clean} ").into_bytes().into()))
                    .await
                    .is_err()
                {
                    return;
                }
                answer.clear();
                pending = Some(reply);
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Binary(bytes))) if pending.is_some() => {
                    for character in String::from_utf8_lossy(&bytes).chars() {
                        match character {
                            '\r' | '\n' => {
                                if let Some(reply) = pending.take() {
                                    let _ = reply.send(std::mem::take(&mut answer));
                                }
                                let _ = socket.send(Message::Binary(b"\r\n".to_vec().into())).await;
                                break;
                            }
                            '\u{8}' | '\u{7f}' => {
                                answer.pop();
                            }
                            character if !character.is_control() && answer.len() < 16384 => {
                                answer.push(character)
                            }
                            _ => {}
                        }
                    }
                }
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                _ => {}
            },
        }
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            send_ssh_connect_error(&mut socket, &host, backend, error.to_string()).await;
            return;
        }
    };
    if socket
        .send(Message::Text(
            json!({ "type": "complete", "backend": backend_name, "result": result })
                .to_string()
                .into(),
        ))
        .await
        .is_ok()
    {
        close_socket(&mut socket).await;
    }
}

async fn send_ws_error(socket: &mut WebSocket, error: String) {
    let _ = socket
        .send(Message::Text(
            json!({ "type": "error", "error": error })
                .to_string()
                .into(),
        ))
        .await;
    close_socket(socket).await;
}

async fn ssh_run_socket(
    mut socket: WebSocket,
    host: String,
    target: crate::jobs::ssh::SshTarget,
    command: String,
) {
    let connection = match crate::jobs::ssh::prepare_interactive_connection(
        &target,
        None,
        Some(command),
    )
    .await
    {
        Ok(connection) => connection,
        Err(error) => {
            send_ws_error(&mut socket, error.to_string()).await;
            return;
        }
    };
    let args = connection.args.clone();
    let session = match tokio::task::spawn_blocking(move || start_pty("ssh", args)).await {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            send_ws_error(&mut socket, error.to_string()).await;
            return;
        }
        Err(error) => {
            send_ws_error(&mut socket, format!("SSH terminal task failed: {error}")).await;
            return;
        }
    };
    let mut size = DEFAULT_PTY_SIZE;
    let Some(status) = relay_pty(&mut socket, session, None, &mut size, None).await else {
        return;
    };

    match status {
        Ok(status) if status.success() => {
            if let Some(warning) = crate::jobs::ssh::setup_connection_sharing(&target).await {
                let _ = socket
                    .send(Message::Binary(
                        format!("\r\norx: warning: {warning}\r\n")
                            .into_bytes()
                            .into(),
                    ))
                    .await;
            }
            if socket
                .send(Message::Text(
                    json!({ "type": "complete" }).to_string().into(),
                ))
                .await
                .is_ok()
            {
                close_socket(&mut socket).await;
            }
        }
        Ok(status) => {
            send_ws_error(
                &mut socket,
                format!("ssh {host} exited with code {}", status.exit_code()),
            )
            .await;
        }
        Err(error) => {
            send_ws_error(&mut socket, error).await;
        }
    }
}

/// Record a client resize message in `size`; false when it is not one.
fn apply_resize(size: &mut PtySize, text: &str) -> bool {
    match serde_json::from_str(text) {
        Ok(SshTerminalInput::Resize { cols, rows }) if cols > 0 && rows > 0 => {
            size.rows = rows;
            size.cols = cols;
            true
        }
        _ => false,
    }
}

/// Relay one PTY session; `size` follows the client's resizes so a session
/// started afterwards can open at the terminal's real dimensions.
async fn relay_pty(
    socket: &mut WebSocket,
    session: PtySession,
    mut output: Option<&mut String>,
    size: &mut PtySize,
    mut screen: Option<&mut harness_setup::TerminalScreen>,
) -> Option<std::result::Result<portable_pty::ExitStatus, String>> {
    let PtySession {
        master,
        input,
        mut events,
        kill,
    } = session;
    let mut child = PtyChildGuard {
        kill,
        running: true,
    };
    // A pasted code's Enter waits for the CLI to echo it, at most a second:
    // Claude Code reads a long chunk as a paste, where Enter does not submit.
    let mut enter_at: Option<tokio::time::Instant> = None;

    let status = loop {
        let enter = async move {
            match enter_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = events.recv() => match event {
                Some(PtyEvent::Output(bytes)) => {
                    if let Some(output) = output.as_deref_mut() { harness_setup::append_output(output, &bytes); }
                    if enter_at.take().is_some() && input.send(b"\r".to_vec()).is_err() {
                        break Err("Terminal input closed".into());
                    }
                    let page = screen.as_deref_mut().and_then(|screen| screen.sign_in.as_mut());
                    let update = page.map(|page| page.observe(&bytes, size.cols)).unwrap_or_default();
                    if socket.send(Message::Binary(bytes.into())).await.is_err() {
                        return None;
                    }
                    if let Some(url) = update.url {
                        let frame = json!({ "type": "signin", "url": url }).to_string();
                        if socket.send(Message::Text(frame.into())).await.is_err() {
                            return None;
                        }
                    }
                    if update.failure.is_some() {
                        return Some(Ok(portable_pty::ExitStatus::with_exit_code(1)));
                    }
                    let completed = screen.as_deref().and_then(|screen| screen.completed);
                    if let (Some(completed), Some(output)) = (completed, output.as_deref()) {
                        if completed(output) {
                            return Some(Ok(portable_pty::ExitStatus::with_exit_code(0)));
                        }
                    }
                }
                Some(PtyEvent::Eof) => {} // EOF alone is not the child's exit status.
                Some(PtyEvent::Exit(status)) => {
                    child.running = false;
                    break status;
                }
                None => break Err("Terminal ended without an exit status".into()),
            },
            message = socket.recv() => match message {
                Some(Ok(Message::Binary(bytes))) => {
                    if input.send(bytes.to_vec()).is_err() {
                        break Err("Terminal input closed".into());
                    }
                }
                Some(Ok(Message::Text(text))) => {
                    if apply_resize(size, &text) {
                        let _ = master.resize(*size);
                    } else if let Some(code) = screen
                        .as_deref_mut()
                        .and_then(|screen| screen.sign_in.as_mut())
                        .and_then(|page| page.code(&text))
                    {
                        if input.send(code.into_bytes()).is_err() {
                            break Err("Terminal input closed".into());
                        }
                        enter_at = Some(tokio::time::Instant::now() + Duration::from_secs(1));
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return None,
                Some(Ok(_)) => {}
            },
            () = enter => {
                enter_at = None;
                if input.send(b"\r".to_vec()).is_err() {
                    break Err("Terminal input closed".into());
                }
            }
        }
    };
    // The waiter and PTY reader run on separate threads. Drain the final bytes
    // briefly so the last diagnostic reaches the terminal before completion.
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(100), events.recv()).await
    {
        match event {
            PtyEvent::Output(bytes) => {
                if let Some(output) = output.as_deref_mut() {
                    harness_setup::append_output(output, &bytes);
                }
                if socket.send(Message::Binary(bytes.into())).await.is_err() {
                    return None;
                }
            }
            PtyEvent::Eof | PtyEvent::Exit(_) => break,
        }
    }

    Some(status)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectTerminalReq {
    session_id: Option<String>,
}

async fn project_terminal(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    Path(id): Path<String>,
    Query(req): Query<ProjectTerminalReq>,
) -> Response {
    if let Some(rejected) = reject_cross_origin(&headers) {
        return rejected;
    }
    let active = Active::new();
    ws.on_upgrade(move |mut socket| async move {
        let _active = active;
        let mut size = DEFAULT_PTY_SIZE;
        let started = tokio::task::spawn_blocking(move || {
            let root = project_terminal_root(&id, req.session_id.as_deref())?;
            let (shell, args) = interactive_shell();
            start_pty_with_env(&shell, args, &[], size, Some(&root))
        })
        .await
        .map_err(anyhow::Error::from)
        .and_then(|result| result);
        let session = match started {
            Ok(session) => session,
            Err(error) => {
                send_terminal_error(&mut socket, error).await;
                return;
            }
        };
        let Some(status) = relay_pty(&mut socket, session, None, &mut size, None).await else {
            return;
        };
        match status {
            Ok(status) => {
                let message = json!({ "type": "exit", "code": status.exit_code() });
                let _ = socket.send(Message::Text(message.to_string().into())).await;
                close_socket(&mut socket).await;
            }
            Err(error) => send_terminal_error(&mut socket, anyhow!(error)).await,
        }
    })
}

/// The session worktree or, without a session, the project clone.
fn project_terminal_root(project_id: &str, session_id: Option<&str>) -> Result<std::path::PathBuf> {
    let store = Store::open()?;
    let project = store
        .get_local_project(project_id)?
        .ok_or_else(|| anyhow!("project not found"))?;
    let session_id = session_id.map(str::trim).filter(|s| !s.is_empty());
    let root = match session_id {
        Some(session_id) => {
            let session = store
                .get_chat_session(session_id)?
                .filter(|session| session.project_id == project.id)
                .ok_or_else(|| anyhow!("chat session not found"))?;
            session_checkout_root(&store, &project, &session.id)
        }
        None => resolve_checkout_root(&store, &project, None).map(|(root, _)| root),
    };
    root.map_err(|ApiError(_, message)| anyhow!(message))
}

/// The worktree the harness will create on its first turn, so a command run
/// before any message acts on the same checkout the agent sees.
fn session_checkout_root(
    store: &Store,
    project: &local::model::LocalProject,
    session_id: &str,
) -> std::result::Result<std::path::PathBuf, ApiError> {
    match local::git::ensure_session_worktree(project, session_id) {
        Ok(dir) => Ok(crate::paths::canonicalize(&dir).unwrap_or(dir)),
        Err(error) => {
            eprintln!("orx up: session worktree unavailable, using the clone: {error}");
            resolve_checkout_root(store, project, Some(session_id)).map(|(root, _)| root)
        }
    }
}

pub(crate) async fn openresearch_login(headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let program = crate::paths::spawnable_exe()
        .map(|exe| exe.to_string_lossy().into_owned())
        .map_err(anyhow::Error::from);
    let mut args = vec!["login".into(), "--non-interactive".into()];
    if signs_in_with_code(&headers) {
        args.push("--device".into());
    }
    command_terminal(&headers, ws, program, args, Vec::new(), false).await
}

/// alphaXiv can send only this machine's own browser back to it, so a page
/// served elsewhere, or a server inside SSH, signs in with a code.
fn signs_in_with_code(headers: &HeaderMap) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    !up_remote::host_allowed(host, up_remote::Reach::Loopback)
        || crate::remote::detect_ssh_session().is_some()
}

/// Connects alphaXiv over the websocket: `authUrl` with the page that approves
/// it, then `complete` or `error`. Closing the socket abandons the sign-in.
pub(crate) async fn alphaxiv_login(headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if let Some(rejected) = reject_cross_origin(&headers) {
        return rejected;
    }
    let device = signs_in_with_code(&headers);
    let active = Active::new();
    ws.on_upgrade(move |mut socket| async move {
        let _active = active;
        let (prompts, mut prompted) = tokio::sync::mpsc::unbounded_channel();
        let login = crate::commands::login::connect_alphaxiv(device, move |prompt| {
            let (url, code) = match prompt {
                crate::commands::login::Prompt::Browser(url) => (url, None),
                crate::commands::login::Prompt::Device { url, code } => (url, Some(code)),
            };
            let _ = prompts.send(json!({ "type": "authUrl", "url": url, "code": code }));
        });
        tokio::pin!(login);

        let finished = loop {
            tokio::select! {
                biased;
                Some(prompt) = prompted.recv() => {
                    if socket.send(Message::Text(prompt.to_string().into())).await.is_err() {
                        return;
                    }
                }
                finished = &mut login => break finished,
                incoming = socket.recv() => {
                    if matches!(incoming, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                        return;
                    }
                }
            }
        };

        let message = match finished {
            Ok(_) => json!({ "type": "complete" }),
            Err(error) => json!({ "type": "error", "error": error.to_string() }),
        };
        if socket
            .send(Message::Text(message.to_string().into()))
            .await
            .is_ok()
        {
            close_socket(&mut socket).await;
        }
    })
}

fn alphaxiv_error(error: crate::error::Error) -> ApiError {
    let status = if error.is::<crate::alphaxiv::NotConnected>() {
        StatusCode::UNAUTHORIZED
    } else if let Some(failure) = error.downcast_ref::<crate::alphaxiv::HttpFailure>() {
        failure.status
    } else {
        StatusCode::BAD_GATEWAY
    };
    ApiError(status, error.to_string())
}

async fn alphaxiv_me() -> ApiResult {
    crate::alphaxiv::me()
        .await
        .map(Json)
        .map_err(alphaxiv_error)
}

pub(crate) async fn alphaxiv_folders() -> ApiResult {
    crate::alphaxiv::folders()
        .await
        .map(Json)
        .map_err(alphaxiv_error)
}

#[derive(Deserialize)]
pub(crate) struct AlphaXivPapersQuery {
    folder: Option<String>,
    cursor: Option<String>,
    search: Option<String>,
}

pub(crate) async fn alphaxiv_papers(Query(query): Query<AlphaXivPapersQuery>) -> ApiResult {
    crate::alphaxiv::papers(
        query.folder.as_deref(),
        query.cursor.as_deref(),
        query.search.as_deref(),
    )
    .await
    .map(Json)
    .map_err(alphaxiv_error)
}

/// Commands the settings page may run, keyed by the exact note text. The
/// loopback and origin guards are the security boundary (the session ends in
/// the user's shell anyway); this list only keeps the button honest.
const SETTINGS_COMMANDS: &[(&str, &[&str])] = &[
    ("gh auth login", &["gh", "auth", "login"]),
    ("hf auth login", &["hf", "auth", "login"]),
    ("claude auth status", &["claude", "auth", "status"]),
    ("kimi login", &["kimi", "login"]),
    (
        "mcode login --region global",
        &["mcode", "login", "--region", "global"],
    ),
    ("devin auth login", &["devin", "auth", "login"]),
    ("grok login", &["grok", "login"]),
    ("fx login", &["fx", "login"]),
    ("pi-acp --terminal-login", &["pi-acp", "--terminal-login"]),
    ("zcode login zai", &["zcode", "login", "zai"]),
    ("zcode login bigmodel", &["zcode", "login", "bigmodel"]),
];

fn settings_command(command: &str) -> Option<&'static [&'static str]> {
    SETTINGS_COMMANDS
        .iter()
        .find(|(text, _)| *text == command)
        .map(|(_, argv)| *argv)
}

#[derive(Deserialize)]
struct RunSettingsCommandReq {
    command: String,
}

async fn run_settings_command(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    Query(req): Query<RunSettingsCommandReq>,
) -> Response {
    // Before the allowlist reply, so a cross-origin page learns nothing.
    if let Some(rejected) = reject_cross_origin(&headers) {
        return rejected;
    }
    let Some(argv) = settings_command(req.command.trim()) else {
        return bad_request("command is not runnable from settings").into_response();
    };
    let mut args: Vec<String> = argv[1..].iter().map(|arg| arg.to_string()).collect();
    let mut env = Vec::new();
    let program = if argv[0] == "zcode" && local::acp::executable_path("zcode").is_none() {
        let mut candidates = Vec::new();
        if let Some(home) = crate::config::home_dir() {
            candidates.push(home.join("Applications/ZCode.app/Contents/Resources/glm/zcode.cjs"));
        }
        candidates.push(std::path::PathBuf::from(
            "/Applications/ZCode.app/Contents/Resources/glm/zcode.cjs",
        ));
        if let Some(data) = crate::config::data_local_dir() {
            candidates.push(data.join("Programs/ZCode/resources/glm/zcode.cjs"));
        }
        match (
            local::acp::executable_path("node"),
            candidates.into_iter().find(|path| path.is_file()),
        ) {
            (Some(node), Some(cli)) => {
                let home = local::shell_env::var("ZCODE_HOME")
                    .or_else(|| {
                        crate::config::synced_env_var("ZCODE_HOME").map(std::ffi::OsString::from)
                    })
                    .filter(|home| !home.is_empty())
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| {
                        crate::config::home_dir().unwrap_or_default().join(".zcode")
                    });
                env = zcode_provider_env(&cli, &home);
                env.push(("ZCODE_HOME", home.into_os_string()));
                args.insert(0, cli.to_string_lossy().into_owned());
                Ok(node.to_string_lossy().into_owned())
            }
            _ => Err(anyhow!(
                "ZCode CLI not found. Install ZCode and Node.js, then retry."
            )),
        }
    } else {
        local::acp::executable_path(argv[0])
            .map(|path| path.to_string_lossy().into_owned())
            .ok_or_else(|| anyhow!("{} is not installed", argv[0]))
    };
    command_terminal(&headers, ws, program, args, env, true).await
}

fn zcode_provider_env(
    cli: &std::path::Path,
    home: &std::path::Path,
) -> Vec<(&'static str, std::ffi::OsString)> {
    let Some(directory) = cli.parent() else {
        return Vec::new();
    };
    let provider = [
        directory.join("provider/zcode-builtin.json"),
        directory.join("../config/provider/zcode-builtin.json"),
    ]
    .into_iter()
    .find(|path| path.is_file());
    let Some(provider) = provider else {
        return Vec::new();
    };
    // ZCode uses the bundled table verbatim only when both paths are explicit.
    vec![
        (
            "ZCODE_BUILTIN_PROVIDER_CONFIG_FILE",
            provider.into_os_string(),
        ),
        (
            "ZCODE_PERSONAL_PROVIDER_CONFIG_FILE",
            home.join("v2/provider_config.json").into_os_string(),
        ),
    ]
}

/// The user's interactive login shell, so follow-up commands see the PATH a
/// fresh terminal would (an installer that just added `~/.local/bin`, say).
fn interactive_shell() -> (String, Vec<String>) {
    if cfg!(windows) {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        (shell, Vec::new())
    } else {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (shell, vec!["-i".to_string(), "-l".to_string()])
    }
}

fn reject_cross_origin(headers: &HeaderMap) -> Option<Response> {
    (!same_origin(headers))
        .then(|| ApiError(StatusCode::FORBIDDEN, "Terminal origin rejected".into()).into_response())
}

/// Run `program` in a PTY relayed over the websocket. With `shell_after`, a
/// command that ran is followed by the user's interactive shell in the same
/// terminal, at the size the client last reported.
async fn command_terminal(
    headers: &HeaderMap,
    ws: WebSocketUpgrade,
    program: Result<String>,
    args: Vec<String>,
    env: Vec<(&'static str, std::ffi::OsString)>,
    shell_after: bool,
) -> Response {
    if let Some(rejected) = reject_cross_origin(headers) {
        return rejected;
    }
    let active = Active::new();
    ws.on_upgrade(move |mut socket| async move {
        let _active = active;
        let mut size = DEFAULT_PTY_SIZE;
        let started = match program {
            Ok(program) => spawn_pty(program, args, env, size).await,
            Err(error) => Err(error),
        };
        let session = match started {
            Ok(session) => session,
            Err(error) => {
                send_terminal_error(&mut socket, error).await;
                return;
            }
        };
        let Some(status) = relay_pty(&mut socket, session, None, &mut size, None).await else {
            return;
        };
        let message = match status {
            Ok(status) if status.success() => json!({ "type": "complete" }),
            Ok(status) => json!({
                "type": "error",
                "error": format!("Command exited with code {}", status.exit_code()),
                "exitCode": status.exit_code(),
            }),
            Err(error) => json!({ "type": "error", "error": error }),
        };
        if socket
            .send(Message::Text(message.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        if shell_after {
            continue_in_shell(&mut socket, &mut size, Vec::new()).await;
        } else {
            close_socket(&mut socket).await;
        }
    })
}

/// WebKit drops the final frame of a socket closed without a close handshake,
/// so send Close and wait for the client's before dropping it.
async fn close_socket(socket: &mut WebSocket) {
    if socket.send(Message::Close(None)).await.is_err() {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
}

/// Hand the terminal to the user's interactive shell, with any env the command
/// before it needed (OpenCode's isolated store), so follow-ups land in the
/// same place.
async fn continue_in_shell(
    socket: &mut WebSocket,
    size: &mut PtySize,
    env: Vec<(&'static str, std::ffi::OsString)>,
) {
    let (shell, args) = interactive_shell();
    match spawn_pty(shell, args, env, *size).await {
        Ok(session) => {
            if relay_pty(socket, session, None, size, None).await.is_some() {
                close_socket(socket).await;
            }
        }
        Err(error) => send_terminal_error(socket, error).await,
    }
}

async fn spawn_pty(
    program: String,
    args: Vec<String>,
    env: Vec<(&'static str, std::ffi::OsString)>,
    size: PtySize,
) -> Result<PtySession> {
    tokio::task::spawn_blocking(move || start_pty_with_env(&program, args, &env, size, None))
        .await?
}

async fn send_terminal_error(socket: &mut WebSocket, error: anyhow::Error) {
    let _ = socket
        .send(Message::Text(
            json!({ "type": "error", "error": error.to_string() })
                .to_string()
                .into(),
        ))
        .await;
    close_socket(socket).await;
}

async fn remote_sessions(State(state): State<AppState>) -> Json<Value> {
    Json(json!({ "sessions": state.remote_sessions.list().await }))
}

async fn remote_workspace(
    State(state): State<AppState>,
    Path((id, _)): Path<(String, String)>,
    request: axum::extract::Request,
) -> axum::response::Response {
    state.remote_sessions.serve_workspace(&id, request).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRemoteSessionReq {
    host: String,
    #[serde(default)]
    ui_preferences: crate::commands::up_remote::RemoteUiPreferences,
}

async fn create_remote_session(
    State(state): State<AppState>,
    Json(req): Json<CreateRemoteSessionReq>,
) -> std::result::Result<Response, ApiError> {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    require_configured_ssh_host(&host).await?;
    let (created, session) = state
        .remote_sessions
        .create(host, req.ui_preferences, None)
        .await?;
    Ok((
        if created {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        },
        Json(json!(session)),
    )
        .into_response())
}

async fn remote_session(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let session = state
        .remote_sessions
        .get(&id)
        .await
        .ok_or_else(|| not_found("remote session"))?;
    Ok(Json(json!(session)))
}

async fn reconnect_remote_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    Ok(Json(json!(state.remote_sessions.reconnect(&id).await?)))
}

async fn disconnect_remote_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    Ok(Json(json!(state.remote_sessions.disconnect(&id).await?)))
}

async fn local_runtime() -> Json<Value> {
    Json(json!({
        "kind": "local",
        "version": env!("CARGO_PKG_VERSION"),
        "buildChannel": crate::telemetry::build_channel(),
    }))
}

enum OpenCodeError {
    V1,
    /// Why a model server listed no models, by a code the dashboard tells apart.
    Unlisted(&'static str, String),
    Api(ApiError),
}

impl IntoResponse for OpenCodeError {
    fn into_response(self) -> Response {
        match self {
            Self::V1 => (StatusCode::CONFLICT, Json(json!({"error":"Update OpenCode to add or change providers, or use OpenResearch's copy.","code":"opencode1"}))).into_response(),
            Self::Unlisted(code, message) => (StatusCode::BAD_REQUEST, Json(json!({"error":message,"code":code}))).into_response(),
            Self::Api(error) => error.into_response(),
        }
    }
}

impl From<crate::error::Error> for OpenCodeError {
    fn from(error: crate::error::Error) -> Self {
        Self::Api(error.into())
    }
}

impl From<ApiError> for OpenCodeError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

type OpenCodeResult = std::result::Result<Json<Value>, OpenCodeError>;

fn opencode_v2(protocol: local::opencode::Protocol) -> std::result::Result<(), OpenCodeError> {
    if protocol == local::opencode::Protocol::V1 {
        return Err(OpenCodeError::V1);
    }
    Ok(())
}

async fn require_opencode_v2() -> std::result::Result<local::opencode::ResolvedBinary, OpenCodeError>
{
    let binary = local::opencode::resolve_binary().await?;
    opencode_v2(binary.protocol)?;
    Ok(binary)
}

/// OpenCode 1 is read-only here, so a change needs OpenCode 2. Without OpenCode a
/// change still saves, since installing it brings OpenCode 2.
async fn opencode_for_change(
) -> std::result::Result<Option<local::opencode::ResolvedBinary>, OpenCodeError> {
    match local::opencode::resolve_binary().await {
        Ok(binary) => opencode_v2(binary.protocol).map(|()| Some(binary)),
        Err(_) => Ok(None),
    }
}

async fn list_local_models() -> OpenCodeResult {
    Ok(Json(local::local_models::list()?))
}

async fn discover_local_models(Json(req): Json<local::local_models::Probe>) -> OpenCodeResult {
    opencode_for_change().await?;
    let models =
        local::local_models::discover(&req)
            .await
            .map_err(
                |e| match e.downcast_ref::<local::local_models::Unlisted>() {
                    Some(unlisted) => {
                        OpenCodeError::Unlisted(unlisted.code(), unlisted.to_string())
                    }
                    None => bad_request(e.to_string()).into(),
                },
            )?;
    Ok(Json(json!({"models":models})))
}

async fn check_local_model(Path(id): Path<String>) -> OpenCodeResult {
    let connection = local::local_models::read()?
        .remove(&id)
        .ok_or_else(|| not_found("model connection"))?;
    let models = local::local_models::discover(&local::local_models::Probe {
        base_url: connection.base_url,
        api_key: connection.api_key,
    })
    .await
    .map_err(|e| bad_request(e.to_string()))?;
    Ok(Json(json!({"models":models})))
}

async fn connect_local_model(
    State(state): State<AppState>,
    Json(req): Json<local::local_models::Connect>,
) -> OpenCodeResult {
    opencode_for_change().await?;
    let (model, created) = local::local_models::connect(req)
        .await
        .map_err(|e| bad_request(e.to_string()))?;
    recheck_opencode(&state).await;
    Ok(Json(json!({"model":model,"created":created})))
}

#[derive(Deserialize)]
struct RemoveLocalModelQuery {
    model: Option<String>,
}

async fn remove_local_model(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<RemoveLocalModelQuery>,
) -> OpenCodeResult {
    local::local_models::remove(&id, query.model.as_deref())?;
    recheck_opencode(&state).await;
    Ok(Json(json!({"ok":true})))
}

/// Re-detects OpenCode alone after a change only its catalog shows: its entry reads as checking
/// meanwhile, and the other agents keep their answers.
async fn recheck_opencode(state: &AppState) {
    if let Some(opencode) = state
        .harnesses
        .lock()
        .await
        .as_mut()
        .and_then(|(_, payload)| payload["harnesses"].as_array_mut())
        .and_then(|all| all.iter_mut().find(|h| h["id"] == "opencode"))
    {
        opencode["catalogPending"] = json!(true);
    }
    let state = state.clone();
    tokio::spawn(async move {
        if let Some(info) = local::harness::detect_harness("opencode").await {
            publish_harness(&state, &info).await;
        }
        state.chat.emit_event("harness.catalog", json!({}));
    });
}

async fn list_opencode_providers() -> OpenCodeResult {
    Ok(Json(local::local_models::provider_list()?))
}

async fn add_opencode_provider(
    State(state): State<AppState>,
    Json(req): Json<local::local_models::AddProvider>,
) -> OpenCodeResult {
    let binary = opencode_for_change().await?;
    let id = local::local_models::add_provider(binary.as_ref(), req)
        .await
        .map_err(|e| bad_request(e.to_string()))?;
    recheck_opencode(&state).await;
    Ok(Json(json!({"id":id})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveOpenCodeKeyReq {
    api_key: String,
}

async fn replace_opencode_key(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SaveOpenCodeKeyReq>,
) -> OpenCodeResult {
    opencode_for_change().await?;
    if !local::local_models::providers()?.contains_key(&id) {
        return Err(not_found("provider").into());
    }
    local::local_models::replace_key(&id, &req.api_key).map_err(|e| bad_request(e.to_string()))?;
    recheck_opencode(&state).await;
    Ok(Json(json!({"ok":true})))
}

async fn remove_opencode_provider(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> OpenCodeResult {
    state.opencode_signins.cancel_integration(&id).await;
    if let Some(record) = local::local_models::providers()?.get(&id) {
        if matches!(
            record.credential,
            local::local_models::ProviderCredential::Oauth
        ) {
            local::opencode_signin::sign_out(&require_opencode_v2().await?, &id).await?;
        }
    }
    local::local_models::remove_provider(&id)?;
    recheck_opencode(&state).await;
    Ok(Json(json!({"ok":true})))
}

async fn list_opencode_integrations() -> OpenCodeResult {
    let binary = require_opencode_v2().await?;
    Ok(Json(local::opencode_signin::integrations(&binary).await?))
}

async fn start_opencode_signin(
    State(state): State<AppState>,
    Json(req): Json<local::opencode_signin::SignIn>,
) -> OpenCodeResult {
    let binary = require_opencode_v2().await?;
    Ok(Json(
        state
            .opencode_signins
            .start(&binary, req)
            .await
            .map_err(|e| bad_request(e.to_string()))?,
    ))
}

async fn opencode_signin_status(
    State(state): State<AppState>,
    Path(attempt): Path<String>,
) -> OpenCodeResult {
    let status = state
        .opencode_signins
        .status(&attempt)
        .ok_or_else(|| not_found("sign-in"))?;
    if status["status"] == "complete" {
        recheck_opencode(&state).await;
    }
    Ok(Json(status))
}

#[derive(Deserialize)]
struct OpenCodeSignInCode {
    code: String,
}

async fn opencode_signin_code(
    State(state): State<AppState>,
    Path(attempt): Path<String>,
    Json(req): Json<OpenCodeSignInCode>,
) -> OpenCodeResult {
    if state.opencode_signins.status(&attempt).is_none() {
        return Err(not_found("sign-in").into());
    }
    state
        .opencode_signins
        .code(&attempt, req.code)
        .await
        .map_err(|e| bad_request(e.to_string()))?;
    Ok(Json(json!({"ok":true})))
}

async fn cancel_opencode_signin(
    State(state): State<AppState>,
    Path(attempt): Path<String>,
) -> OpenCodeResult {
    state.opencode_signins.cancel(&attempt).await;
    Ok(Json(json!({"ok":true})))
}

// --- harnesses ---------------------------------------------------------------

const HARNESS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Minimum age before a still-pending or promotion-blocked entry may re-arm a
/// catalog fill — bounds how often a non-converging state can buy a sweep.
const FILL_RETRY_FLOOR: Duration = Duration::from_secs(5);

#[derive(Deserialize, Default)]
struct HarnessQuery {
    refresh: Option<u8>,
    retry: Option<u8>,
    /// With `refresh=1`, re-detect only this harness instead of every one.
    harness: Option<String>,
}

fn overlay_claude_auth(payload: &mut Value, snapshot: &local::claude::AuthSnapshot) {
    const FAILED_RECHECK_NOTE: &str = "Could not re-check Claude Code. The last verified configuration is still in use; re-check this agent.";
    let Some(harnesses) = payload.get_mut("harnesses").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(claude) = harnesses.iter_mut().find(|h| {
        h.get("id").and_then(Value::as_str) == Some("claude-code") && h.get("profile").is_none()
    }) else {
        return;
    };
    // Cached auth cannot make a missing or broken binary runnable.
    if claude.get("installed").and_then(Value::as_bool) == Some(false)
        || entry_install_broken(claude)
    {
        return;
    }
    // A provisional entry is never overlaid: the shared state would stamp an
    // auth verdict (or a "sign in" note) the fill is about to settle, under a
    // card that reads "checking" anyway.
    if claude.get("catalogPending").and_then(Value::as_bool) == Some(true) {
        return;
    }
    let entry_was_ready = claude.get("authState").and_then(Value::as_str) == Some("ready");
    claude["authState"] = json!(snapshot.state);
    claude["needsConfigRepair"] = json!(snapshot.credential_conflict);
    claude["authCheckFailed"] = json!(snapshot.auth_check_failed);
    claude["loginEligible"] = json!(
        !snapshot.auth_check_failed
            && !snapshot.runtime_rejected
            && local::harness::claude::login_eligible_state(
                snapshot.state,
                snapshot.method,
                snapshot.provider.as_deref(),
                snapshot.credential_conflict,
            )
    );
    if let Some(object) = claude.as_object_mut() {
        match snapshot.method {
            Some(method) => {
                object.insert("authMethod".into(), json!(method));
            }
            None => {
                object.remove("authMethod");
            }
        }
        match &snapshot.provider {
            Some(provider) => {
                object.insert("authProvider".into(), json!(provider));
            }
            None => {
                object.remove("authProvider");
            }
        }
        if snapshot.state != local::harness::HarnessAuthState::Ready
            || snapshot.method != Some("oauth")
            || snapshot
                .provider
                .as_deref()
                .is_some_and(|provider| provider != "firstParty")
            || !entry_was_ready
        {
            object.remove("account");
            object.remove("accountName");
            object.remove("org");
            object.remove("plan");
        }
    }
    if snapshot.state == local::harness::HarnessAuthState::Ready {
        claude["authenticated"] = json!(true);
        claude["agentReady"] = json!(true);
        if snapshot.auth_check_failed {
            claude["agentNote"] = json!(FAILED_RECHECK_NOTE);
        } else if !entry_was_ready
            || claude.get("agentNote").and_then(Value::as_str) == Some(FAILED_RECHECK_NOTE)
        {
            if let Some(object) = claude.as_object_mut() {
                object.remove("agentNote");
            }
        }
        return;
    }
    claude["authenticated"] = json!(false);
    claude["agentReady"] = json!(false);
    claude["models"] = json!([]);
    claude.as_object_mut().unwrap().remove("defaultModel");
    if snapshot.credential_conflict {
        claude["agentNote"] = json!("Claude Code reports a subscription login while an Anthropic environment credential is set. Fix or unset that credential, then re-check this agent.");
        return;
    }
    claude["agentNote"] = json!(if snapshot.runtime_rejected {
        local::harness::claude::auth_recovery_note(snapshot.method, snapshot.provider.as_deref())
    } else {
        match snapshot.state {
            local::harness::HarnessAuthState::NeedsLogin => {
                if local::harness::claude::login_eligible_state(
                    snapshot.state,
                    snapshot.method,
                    snapshot.provider.as_deref(),
                    false,
                ) {
                    "Sign in with `claude auth login`, then re-check this agent."
                } else {
                    "Check the configured Claude Code provider with `claude auth status`, then re-check this agent."
                }
            }
            local::harness::HarnessAuthState::Unknown => {
                "Open a terminal and run `claude auth status`, then re-check this agent."
            }
            local::harness::HarnessAuthState::Unsupported => {
                "Update Claude Code to 2.1.211 or newer, then re-check this agent."
            }
            local::harness::HarnessAuthState::Ready => unreachable!(),
        }
    });
}

struct ClaudeCatalogRequest {
    bin: std::path::PathBuf,
    ultracode: bool,
    method: Option<&'static str>,
    provider: Option<String>,
}

#[derive(Default)]
struct ClaudeCatalogQueue {
    running: bool,
    latest: Option<(std::time::Instant, ClaudeCatalogRequest)>,
}

fn claude_catalog_request(
    harnesses: &[local::harness::HarnessInfo],
) -> Option<ClaudeCatalogRequest> {
    let claude = harnesses
        .iter()
        .find(|h| h.id == "claude-code" && h.agent_ready)?;
    if !local::harness::claude::external_provider(
        claude.auth_method,
        claude.auth_provider.as_deref(),
    ) {
        return None;
    }
    Some(ClaudeCatalogRequest {
        bin: claude.bin_path.as_ref()?.into(),
        ultracode: claude.claude_ultracode,
        method: claude.auth_method,
        provider: claude.auth_provider.clone(),
    })
}

fn enqueue_claude_catalog(
    state: AppState,
    cached_at: std::time::Instant,
    request: ClaudeCatalogRequest,
) {
    let mut queue = state.claude_catalog_queue.lock().unwrap();
    queue.latest = Some((cached_at, request));
    if queue.running {
        return;
    }
    queue.running = true;
    drop(queue);
    tokio::spawn(async move {
        loop {
            let next = {
                let mut queue = state.claude_catalog_queue.lock().unwrap();
                match queue.latest.take() {
                    Some(next) => next,
                    None => {
                        queue.running = false;
                        return;
                    }
                }
            };
            let (cached_at, request) = next;
            let (models, default_model) =
                local::harness::claude::external_model_catalog(request.bin, request.ultracode)
                    .await;
            let empty = models.is_empty();
            let mut cache = state.harnesses.lock().await;
            let Some((at, payload)) = cache.as_mut() else {
                continue;
            };
            if *at != cached_at {
                continue;
            }
            let auth = state.claude.auth_snapshot();
            if auth.state != local::harness::HarnessAuthState::Ready
                || auth.runtime_rejected
                || auth.method != request.method
                || auth.provider != request.provider
            {
                continue;
            }
            let Some(claude) = payload["harnesses"].as_array_mut().and_then(|all| {
                all.iter_mut()
                    .find(|h| h["id"] == "claude-code" && h["agentReady"] == true)
            }) else {
                continue;
            };
            claude["models"] = json!(models);
            local::harness::resolve_entry(claude, default_model.as_deref())
                .expect("harness models");
            if empty {
                claude["agentNote"] = json!(
                    "Could not load Claude Code models. Re-check this agent or update Claude Code."
                );
            } else if let Some(object) = claude.as_object_mut() {
                object.remove("agentNote");
            }
            drop(cache);
            state.chat.emit_event("harness.catalog", json!({}));
        }
    });
}

fn claude_entry(payload: &Value) -> Option<&Value> {
    payload.get("harnesses")?.as_array()?.iter().find(|h| {
        h.get("id").and_then(Value::as_str) == Some("claude-code") && h.get("profile").is_none()
    })
}

fn entry_install_broken(entry: &Value) -> bool {
    entry.get("installBroken").and_then(Value::as_bool) == Some(true)
}

/// A claude the cached entry must never be restored over: one that can't run,
/// or isn't there at all. Both would resurrect `agentReady` for a binary no
/// turn can spawn.
fn claude_unspawnable(payload: &Value) -> bool {
    claude_entry(payload).is_none_or(|claude| {
        entry_install_broken(claude)
            || claude.get("installed").and_then(Value::as_bool) != Some(true)
    })
}

fn payload_has_ready_claude(payload: &Value) -> bool {
    claude_entry(payload)
        .and_then(|claude| claude.get("agentReady"))
        .and_then(Value::as_bool)
        == Some(true)
}

#[cfg(test)]
fn claude_entry_pending(payload: &Value) -> bool {
    claude_entry(payload)
        .and_then(|claude| claude.get("catalogPending"))
        .and_then(Value::as_bool)
        == Some(true)
}

async fn harness_snapshot(Path(id): Path<String>) -> ApiResult {
    let info = local::harness::detect_harness_snapshot(&id)
        .await
        .ok_or_else(|| not_found("harness"))?;
    Ok(Json(json!(info)))
}

async fn list_harnesses(State(state): State<AppState>, Query(q): Query<HarnessQuery>) -> ApiResult {
    let mut payload = harnesses_payload(&state, &q).await;
    let hidden = Store::open()?.ui_state()?.hidden_agents;
    for entry in payload["harnesses"].as_array_mut().into_iter().flatten() {
        entry["off"] = json!(local::model_routes::turned_off(entry, &hidden));
        let harness = entry["id"].as_str().unwrap_or_default().to_string();
        local::model_routes::name_models(&harness, &mut entry["models"]);
    }
    Ok(Json(payload))
}

#[derive(Deserialize)]
struct CreateAgentProfileReq {
    harness: String,
    name: String,
    icon: Option<Value>,
}

async fn create_agent_profile(
    State(state): State<AppState>,
    Json(req): Json<CreateAgentProfileReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let id = tokio::task::spawn_blocking(move || {
        local::agent_profiles::create(&req.harness, &req.name, req.icon)
    })
    .await
    .map_err(|e| anyhow!("Agent account creation failed: {e}"))?
    .map_err(bad_request)?;
    state.chat.emit_event("harness.catalog", json!({}));
    Ok(Json(json!({"id": id})))
}

async fn remove_harness_install(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    local::harness::managed::removable_key(&id).map_err(bad_request)?;
    let _data = state.data_dir_gate.lock().await;
    reject_if_moving(&state)?;
    let _setup = local::harness::managed::lock_installations()
        .map_err(|error| ApiError(StatusCode::CONFLICT, error.to_string()))?;
    if !state
        .chat
        .remove_harness_install(Store::open()?, &id, async {
            let result = local::harness::managed::uninstall(&id).await;

            *state.harnesses.lock().await = None;
            state.chat.emit_event("harness.catalog", json!({}));

            result
        })
        .await?
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Wait for this agent’s running chats to finish or stop them, then remove it again."
                .into(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn edit_agent(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(edit): Json<local::agent_profiles::Edit>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let relabels = tokio::task::spawn_blocking(move || local::agent_profiles::edit(&key, &edit))
        .await
        .map_err(|e| anyhow!("Agent update failed: {e}"))?
        .map_err(bad_request)?;
    if relabels {
        recheck_opencode(&state).await;
    } else {
        state.chat.emit_event("harness.catalog", json!({}));
    }
    Ok(Json(json!({})))
}

async fn delete_agent_profile(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    let harness = local::agent_profiles::harness_of(&id)
        .ok_or_else(|| bad_request("Choose an existing agent account"))?;
    if let Ok(env) = local::agent_profiles::login_env(&id) {
        let bin = if harness == "claude-code" {
            local::harness::claude::find_claude()
        } else {
            local::harness::codex::find_codex()
        };
        if let Some(bin) = bin {
            let mut command = crate::process::tokio_command(bin);
            if harness == "claude-code" {
                command.args(["auth", "logout"]);
            } else {
                command.arg("logout");
            }
            local::chat::prepare_env(&mut command);
            command
                .envs(env)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(10), command.status()).await;
        }
    }
    tokio::task::spawn_blocking(move || local::agent_profiles::remove(&id))
        .await
        .map_err(|e| anyhow!("Agent account removal failed: {e}"))?
        .map_err(bad_request)?;
    state.chat.emit_event("harness.catalog", json!({}));
    Ok(Json(json!({"ok": true})))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AcpHarnessInput {
    name: String,
    executable: String,
    #[serde(default)]
    arguments: Vec<String>,
}

impl AcpHarnessInput {
    fn definition(self, id: String) -> local::acp::Definition {
        local::acp::Definition {
            id,
            name: self.name,
            executable: self.executable,
            arguments: self.arguments,
            unknown: Default::default(),
        }
    }
}

async fn list_acp_harnesses() -> Json<Value> {
    Json(json!(local::acp::definitions()))
}

async fn list_acp_presets() -> Json<Value> {
    Json(json!(local::acp::presets()))
}

async fn create_acp_harness(
    State(state): State<AppState>,
    Json(input): Json<AcpHarnessInput>,
) -> ApiResult {
    let definition = input.definition(format!("acp:{}", uuid::Uuid::new_v4()));
    local::acp::save(definition.clone(), true).map_err(bad_request)?;
    *state.harnesses.lock().await = None;
    Ok(Json(json!(definition)))
}

async fn update_acp_harness(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<AcpHarnessInput>,
) -> ApiResult {
    let definition = input.definition(id);
    local::acp::save(definition.clone(), false).map_err(bad_request)?;
    *state.harnesses.lock().await = None;
    Ok(Json(json!(definition)))
}

async fn delete_acp_harness(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    local::acp::remove(&id).map_err(bad_request)?;
    *state.harnesses.lock().await = None;
    Ok(Json(json!({"ok": true})))
}

async fn test_acp_harness(Json(input): Json<AcpHarnessInput>) -> ApiResult {
    let definition = input.definition(format!("acp:{}", uuid::Uuid::new_v4()));
    let mut result = local::acp::test_connection(&definition)
        .await
        .map_err(bad_request)?;
    result["harnessId"] = json!(local::acp::registered_definitions()
        .iter()
        .find(|registered| local::acp::same_launch(registered, &definition))
        .map(|registered| &registered.id));
    Ok(Json(result))
}

/// The shared detection path behind `GET /api/harnesses` and the startup
/// preflight: the snapshot pass runs under the cache lock, so whoever arrives
/// first does the probing and everyone else in the TTL window reads the same
/// entry — no duplicate sweep at boot. `refresh=1` and the background fill
/// detect before taking the lock, so a slow probe never stalls a reader.
///
/// A plain request answers with the snapshot pass — install/auth/readiness,
/// without the model-catalog children that made cold calls take seconds — and
/// a background fill replaces the entry with the full catalog, then tells the
/// dashboard to re-read it. `refresh=1` still runs the full probe inline: an
/// explicit re-check wants the real answer, and the UI shows its own spinner.
/// With `harness=<id>` it probes only that harness (unless a fill is running)
/// and serves the cache with it patched in.
async fn harnesses_payload(state: &AppState, q: &HarnessQuery) -> Value {
    // `retry` only escalates to a full detect when it actually cleared a
    // runtime rejection — a bare retry is an ordinary cached read.
    let retried = q.retry == Some(1) && state.claude.clear_runtime_rejection();
    if let (Some(1), Some(id)) = (q.refresh, q.harness.as_deref()) {
        // Publishing re-stamps the cache, voiding an in-flight fill's other harnesses;
        // the fill commits this one itself and the auth monitor re-checks after it.
        if !state
            .harness_fill_in_flight
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if let Some(info) = local::harness::detect_harness(id).await {
                publish_harness(state, &info).await;
            }
        }
    } else if q.refresh == Some(1) || retried {
        // An explicit re-check detects outside the cache lock: a full sweep
        // takes seconds and must not stall every ordinary read queued behind
        // it. Last writer wins — concurrent refreshes may duplicate a sweep.
        state.claude.reserve_auth_check();
        let harnesses = local::harness::detect_harnesses().await;
        let catalog = claude_catalog_request(&harnesses);
        let (mut payload, announce) = finish_harnesses_payload(state, harnesses, false).await;
        let cached_at = std::time::Instant::now();
        spawn_cursor_account_details(state, &mut payload, cached_at);
        *state.harnesses.lock().await = Some((cached_at, payload.clone()));
        if let Some(catalog) = catalog {
            enqueue_claude_catalog(state.clone(), cached_at, catalog);
        }
        emit_auth_announcement(state, announce);
        return payload;
    }
    let mut cache = state.harnesses.lock().await;
    if let Some((at, payload)) = cache.as_ref() {
        let auth = state.claude.auth_snapshot();
        // Stale-while-revalidate serves the last answer while a fill
        // refreshes it — re-snapshotting on every TTL tick would flap
        // every card back to "checking". A provisional entry also asks
        // for a fill: the single-flight flag makes that idempotent, and
        // it re-arms a fill that lost the `snapshot_at` race so a
        // provisional payload can't sit unanswered for a whole TTL. The
        // same covers a live-verified login promotion (shared auth says
        // Ready but the payload's claude isn't).
        let promotable = auth.state == local::harness::HarnessAuthState::Ready
            && !payload_has_ready_claude(payload);
        // Provisional/promotable re-arms carry a floor: a state that cannot
        // converge (e.g. Ready auth over a broken install) must not buy a
        // full sweep on every read.
        let wants_fill = payload_is_provisional(payload) || promotable;
        if at.elapsed() >= HARNESS_CACHE_TTL || (wants_fill && at.elapsed() >= FILL_RETRY_FLOOR) {
            spawn_catalog_fill(state.clone(), *at, uuid::Uuid::new_v4());
        }
        let mut out = payload.clone();
        drop(cache);
        overlay_claude_auth(&mut out, &auth);
        if let Err(error) = local::agent_profiles::reconcile(&mut out) {
            eprintln!("orx up: agent profiles: {error}");
        }
        return out;
    }
    seed_harnesses_locked(state, &mut cache).await
}

/// Commit one harness's full detection after setup or a sign-in changed it, so
/// the dashboard need not wait on every sibling's probes to see the result.
async fn publish_harness(state: &AppState, info: &local::harness::HarnessInfo) {
    let mut info = info.clone();
    apply_model_override(state, &mut info);
    if info.id == "claude-code" {
        if let Some(probe) = info.auth_observation.as_ref() {
            state.claude.observe_auth_probe(probe);
        }
    }
    let catalog = claude_catalog_request(std::slice::from_ref(&info));
    let mut cache = state.harnesses.lock().await;
    // Bumping `at` voids a fill that probed before setup ran (and any cursor
    // lookup keyed to it); a fresh seed's fill probes after, so it keeps its `at`.
    let claim = cache.is_some();
    if !claim {
        seed_harnesses_locked(state, &mut cache).await;
    }
    let Some((at, payload)) = cache.as_mut() else {
        return;
    };
    let Some(slot) = payload["harnesses"].as_array_mut().and_then(|all| {
        all.iter_mut()
            .find(|h| h["id"].as_str() == Some(info.id.as_str()) && h.get("profile").is_none())
    }) else {
        return;
    };
    *slot = json!(info);
    if claim {
        *at = std::time::Instant::now();
    }
    let cached_at = *at;
    let lookup_voided = claim
        && payload["harnesses"].as_array().is_some_and(|all| {
            all.iter()
                .any(|h| h["id"] == "cursor" && h["accountLoading"] == true)
        });
    if lookup_voided || info.id == "cursor" {
        spawn_cursor_account_details(state, payload, cached_at);
    }
    let base = payload["harnesses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("profile").is_none())
        .cloned()
        .collect::<Vec<_>>();
    drop(cache);
    match local::agent_profiles::entries(&base).await {
        Ok(profiles) => {
            let mut cache = state.harnesses.lock().await;
            if let Some((at, payload)) = cache.as_mut() {
                if *at == cached_at {
                    if let Some(all) = payload["harnesses"].as_array_mut() {
                        all.retain(|entry| entry.get("profile").is_none());
                        all.extend(profiles);
                    }
                }
            }
        }
        Err(error) => eprintln!("orx up: agent profiles: {error}"),
    }
    if let Some(catalog) = catalog {
        enqueue_claude_catalog(state.clone(), cached_at, catalog);
    }
    state.chat.emit_event("harness.catalog", json!({}));
}

/// The spawn-free snapshot → provisional cache → background fill sequence,
/// shared by the first `/api/harnesses` request and the startup seed. The
/// boot call starts the fill's heavyweight probes during the user's first
/// page load instead of inside it — each spawn costs seconds on Windows, so
/// where the clock starts is most of the difference. Caller holds the
/// `harnesses` lock.
async fn seed_harnesses_locked(
    state: &AppState,
    cache: &mut Option<(std::time::Instant, Value)>,
) -> Value {
    let started = std::time::Instant::now();
    let harnesses = local::harness::detect_harnesses_snapshot().await;
    let installed = harnesses.iter().filter(|h| h.installed).count();
    // The snapshot row and the fill it seeds share one fillId so a boot's two
    // passes join on it.
    let fill_id = uuid::Uuid::new_v4();
    let snapshot_ms = started.elapsed().as_millis() as u64;
    let (mut payload, _) = finish_harnesses_payload(state, harnesses, true).await;
    let cached_at = std::time::Instant::now();
    spawn_cursor_account_details(state, &mut payload, cached_at);
    *cache = Some((cached_at, payload.clone()));
    // Off the lock-held request path and off a runtime worker: settings
    // load + outbox persist are sync IO on the same filesystems this pass
    // exists to stop blocking on.
    tokio::task::spawn_blocking(move || {
        crate::telemetry::harness::capture_detect(
            fill_id,
            "snapshot",
            snapshot_ms,
            None,
            installed,
            0,
            Vec::new(),
        );
    });
    spawn_catalog_fill(state.clone(), cached_at, fill_id);
    payload
}

fn apply_model_override(state: &AppState, info: &mut local::harness::HarnessInfo) {
    if info.id == "opencode" {
        if let Some(model) = state.agent.model_override.as_deref() {
            local::harness::resolve_model(info, Some(model));
        }
    }
}

/// The post-detection half of the harness answer: reconcile the result with
/// the ClaudeHost's tracked auth state, then overlay and report. Shared by
/// the snapshot and full passes, and by the background catalog fill.
///
/// `provisional` marks a discovery-only snapshot. Only full passes publish
/// live Claude auth observations and first-install telemetry.
/// The returned `Option` is a claimed auth announcement: the caller emits it
/// only after committing the payload, so a discarded pass never sends the
/// dashboard to re-probe Claude for results nobody will see.
async fn finish_harnesses_payload(
    state: &AppState,
    harnesses: Vec<local::harness::HarnessInfo>,
    provisional: bool,
) -> (Value, Option<local::claude::AuthSnapshot>) {
    if !provisional {
        if let Some(probe) = harnesses
            .iter()
            .find(|h| h.id == "claude-code")
            .and_then(|h| h.auth_observation.as_ref())
        {
            state.claude.observe_auth_probe(probe);
        }
    }
    let mut harnesses = harnesses;
    for info in &mut harnesses {
        apply_model_override(state, info);
    }
    let mut payload = json!({ "harnesses": harnesses });
    let mut snapshot = state.claude.auth_snapshot();
    // A claude that broke mid-session leaves the snapshot `Ready` — `detect`
    // skips the probe, and `observe_auth_state` won't downgrade on `Unknown` —
    // so drop it here instead, or the resurrection below would restore
    // `agentReady` for a binary that cannot start.
    if snapshot.state == local::harness::HarnessAuthState::Ready && claude_unspawnable(&payload) {
        state.claude.defer_auth_verification(snapshot.generation);
        snapshot = state.claude.auth_snapshot();
    }
    // A provisional pass must not announce: `harness.auth` makes the dashboard
    // re-probe Claude inline — the stall this whole design exists to remove —
    // fired by the snapshot's own adoption.
    // A real pass hands its auth snapshot back unclaimed; the caller claims at
    // emit time, after the payload commits, so a pass that loses the cache
    // race never burns a generation it can't announce.
    let announce = (!provisional).then_some(snapshot.clone());
    overlay_claude_auth(&mut payload, &snapshot);
    if let Some(base) = payload["harnesses"].as_array() {
        match local::agent_profiles::entries(base).await {
            Ok(profiles) => {
                let all = payload["harnesses"].as_array_mut().unwrap();
                all.extend(profiles);
            }
            Err(error) => {
                eprintln!("orx up: agent profiles: {error}");
            }
        }
    }
    // After the profiles join, so it adds none of its own beside them.
    if let Err(error) = local::agent_profiles::reconcile(&mut payload) {
        eprintln!("orx up: agent profiles: {error}");
    }
    if !provisional {
        crate::telemetry::harness::capture_initial(&payload);
    }
    (payload, announce)
}

/// Emit `harness.auth` for a generation this pass is entitled to announce.
/// The claim happens here — not in `finish_harnesses_payload` — so the
/// generation is only consumed when the event actually goes out.
fn emit_auth_announcement(state: &AppState, announce: Option<local::claude::AuthSnapshot>) {
    if let Some(snap) = announce {
        if state.claude.claim_auth_announcement(snap.generation) {
            state.chat.emit_event(
                "harness.auth",
                json!({ "harness": "claude-code", "authState": snap.state }),
            );
        }
    }
}

/// True while any entry still waits on the background catalog fill.
fn payload_is_provisional(payload: &Value) -> bool {
    payload["harnesses"].as_array().is_some_and(|all| {
        all.iter()
            .any(|h| h["catalogPending"].as_bool() == Some(true))
    })
}

/// Cursor's account lookup is a second child process deferred off the
/// detection answer; it patches the cache entry it was scheduled against.
fn spawn_cursor_account_details(
    state: &AppState,
    payload: &mut Value,
    cached_at: std::time::Instant,
) {
    // A provisional cursor entry keeps its lookup for the fill: spawning
    // `<binPath> about` here would put a child process on the path that exists
    // to avoid them — and the snapshot's binPath is only a first-candidate
    // guess.
    let cursor = payload["harnesses"].as_array_mut().and_then(|items| {
        items.iter_mut().find(|h| {
            h["id"] == "cursor"
                && h["authenticated"] == true
                && h["catalogPending"].as_bool() != Some(true)
        })
    });
    if let Some(cursor) = cursor {
        if let Some(bin) = cursor["binPath"].as_str().map(std::path::PathBuf::from) {
            cursor["accountLoading"] = json!(true);
            let cache = state.harnesses.clone();
            tokio::spawn(async move {
                let details = local::harness::cursor::account_details(&bin).await;
                let mut cache = cache.lock().await;
                let Some((at, payload)) = cache.as_mut() else {
                    return;
                };
                // A newer detection owns its own account lookup.
                if *at != cached_at {
                    return;
                }
                let Some(cursor) = payload["harnesses"]
                    .as_array_mut()
                    .and_then(|items| items.iter_mut().find(|h| h["id"] == "cursor"))
                else {
                    return;
                };
                cursor["accountLoading"] = json!(false);
                if let Some(details) = details {
                    for (source, target) in [("userEmail", "account"), ("subscriptionTier", "plan")]
                    {
                        if let Some(value) =
                            details[source].as_str().filter(|value| !value.is_empty())
                        {
                            cursor[target] = json!(value);
                        }
                    }
                }
            });
        }
    }
}

/// Complete the catalog a snapshot answer deferred: the full sweep runs off
/// the request path, replaces the cache entry it was scheduled against, and
/// the `harness.catalog` event tells the dashboard to re-read it. `snapshot_at`
/// guards the swap — a refresh or retry that landed in between owns the cache.
/// One fill at a time: expired reads each ask for one, but a second sweep
/// would only lose the `snapshot_at` race and throw its probes away.
fn spawn_catalog_fill(state: AppState, snapshot_at: std::time::Instant, fill_id: uuid::Uuid) {
    if state
        .harness_fill_in_flight
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    state.claude.reserve_auth_check();
    let probe_timings = local::harness::ProbeTimingSink::default();
    tokio::spawn(local::harness::probe_timing_scope(
        probe_timings.clone(),
        async move {
            let _fill = FillGuard(state.harness_fill_in_flight.clone());
            let started = std::time::Instant::now();
            // Commit each harness as its probes land: the onboarding gate is
            // per-entry (`agentReady && !catalogPending`), so a ready agent must
            // not wait for the slowest sibling's catalog child. The batch swap at
            // the end still applies the cross-entry reconciliation
            // (`finish_harnesses_payload`) to the complete set.
            let mut stream = std::pin::pin!(local::harness::detect_harnesses_each());
            let mut ordered = Vec::new();
            // `first_ready_ms` answers the churn question directly: how far into
            // the fill did a usable agent actually publish.
            let mut first_ready_ms = None;
            while let Some((index, mut info)) = futures::StreamExt::next(&mut stream).await {
                apply_model_override(&state, &mut info);
                // A progressive claude entry settles shared auth *now*: reads
                // overlay that state onto every non-pending entry, so a Ready
                // probe left unobserved until fill end would be stamped back to
                // Unknown on each answer — the stall this loop exists to remove.
                if info.id == "claude-code" {
                    if let Some(probe) = info.auth_observation.as_ref() {
                        state.claude.observe_auth_probe(probe);
                    }
                }
                let mut published = false;
                {
                    let mut cache = state.harnesses.lock().await;
                    // Same guard as the final swap: a refresh or retry that
                    // landed in between owns the cache.
                    if let Some((at, payload)) = cache.as_mut() {
                        if *at == snapshot_at {
                            if let Some(slot) =
                                payload["harnesses"].as_array_mut().and_then(|all| {
                                    all.iter_mut().find(|h| {
                                        h["id"].as_str() == Some(info.id.as_str())
                                            && h.get("profile").is_none()
                                    })
                                })
                            {
                                *slot = json!(info);
                                published = true;
                            }
                        }
                    }
                }
                // A superseded fill publishes nothing and must not announce: the
                // pass that owns the cache emits its own catalog events.
                if !published {
                    ordered.push((index, info));
                    continue;
                }
                state.chat.emit_event("harness.catalog", json!({}));
                // For claude, "ready" means the overlay won't stamp it back down
                // — its committed `agentReady` only stands once shared auth is
                // Ready (the observation above).
                if first_ready_ms.is_none()
                    && info.agent_ready
                    && (info.id != "claude-code"
                        || state.claude.auth_snapshot().state
                            == local::harness::HarnessAuthState::Ready)
                {
                    first_ready_ms = Some(started.elapsed().as_millis() as u64);
                }
                ordered.push((index, info));
            }
            ordered.sort_by_key(|(index, _)| *index);
            let harnesses: Vec<local::harness::HarnessInfo> =
                ordered.into_iter().map(|(_, info)| info).collect();
            let catalog = claude_catalog_request(&harnesses);
            // Finish before touching the cache: the Claude re-probe inside can
            // cost a child process, and holding the lock across it stalls every
            // reader the snapshot was meant to unblock.
            let (mut payload, announce) = finish_harnesses_payload(&state, harnesses, false).await;
            // The reconciled payload is authoritative — overlay applied — so
            // readiness is counted from it, not the raw probes.
            let installed = payload["harnesses"]
                .as_array()
                .map(|all| all.iter().filter(|h| h["installed"] == true).count())
                .unwrap_or(0);
            let ready = payload["harnesses"]
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter(|h| h["agentReady"] == true && h["catalogPending"] != true)
                        .count()
                })
                .unwrap_or(0);
            let mut catalog_at = None;
            let committed = {
                let mut cache = state.harnesses.lock().await;
                // A refresh or retry that landed in between owns the cache. No
                // event: clients holding a provisional payload already poll it at
                // 1 Hz, and the discarded pass must not announce auth it never
                // committed.
                if !matches!(cache.as_ref(), Some((at, _)) if *at == snapshot_at) {
                    false
                } else {
                    // Readiness can be *established* by the finish (a Ready
                    // shared-auth retry that ran no stream probe) — count it, but
                    // only once publication is confirmed: a superseded fill
                    // reports its probe timings with no invented readiness mark.
                    if first_ready_ms.is_none() && ready > 0 {
                        first_ready_ms = Some(started.elapsed().as_millis() as u64);
                    }
                    let filled_at = std::time::Instant::now();
                    spawn_cursor_account_details(&state, &mut payload, filled_at);
                    *cache = Some((filled_at, payload));
                    catalog_at = Some(filled_at);
                    true
                }
            };
            // Emit regardless of the race outcome: a fill that lost it still ran
            // real probes, and its timings are real data. Settings load +
            // outbox persist are sync IO — keep them off the runtime workers.
            let timings = std::mem::take(
                &mut *probe_timings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
            let duration_ms = started.elapsed().as_millis() as u64;
            tokio::task::spawn_blocking(move || {
                crate::telemetry::harness::capture_detect(
                    fill_id,
                    "full",
                    duration_ms,
                    first_ready_ms,
                    installed,
                    ready,
                    timings,
                );
            });
            if !committed {
                return;
            }
            if let (Some(catalog), Some(at)) = (catalog, catalog_at) {
                enqueue_claude_catalog(state.clone(), at, catalog);
            }
            state.chat.emit_event("harness.catalog", json!({}));
            emit_auth_announcement(&state, announce);
        },
    ));
}

/// Clears the single-flight flag however the fill task exits once polled —
/// including the superseded-swap early return and a dropped future. (A task
/// dropped before its first poll leaks the flag, but that only happens at
/// runtime shutdown, where no fill will ever be wanted again.)
struct FillGuard(Arc<std::sync::atomic::AtomicBool>);

impl Drop for FillGuard {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

// --- chat --------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionsQuery {
    project_id: Option<String>,
    /// `all` is the composer's `/resume` picker, which spans every project.
    /// Spelled out so a dropped `projectId` cannot silently widen the scope.
    scope: Option<String>,
    archived: Option<bool>,
    before_updated_at: Option<i64>,
    before_id: Option<String>,
}

fn sidebar_session_json(
    store: &Store,
    session: &StoredChatSession,
    busy: bool,
    catalog: &Value,
) -> Result<Value> {
    let mut value = local::chat::session_json_with(Some(store), session, busy);
    local::chat::project_model(&mut value, session, catalog);
    value["awaitingInput"] = json!(store.chat_session_awaiting_input(&session.id, busy)?);
    Ok(value)
}

fn chat_active_runs_json(store: &Store) -> Result<Value> {
    let mut rows = Vec::new();
    for (session_id, run) in store.chat_active_runs()? {
        let experiment_name = store
            .get_local_experiment(&run.experiment_id)?
            .map(|experiment| experiment.display_name().to_string());
        rows.push(json!({
            "sessionId": session_id,
            "run": ApiRun::new(store, &run)?,
            "experimentName": experiment_name,
        }));
    }
    Ok(json!(rows))
}

async fn chat_active_runs() -> ApiResult {
    Ok(Json(chat_active_runs_json(&Store::open()?)?))
}

async fn list_chat_sessions(
    State(state): State<AppState>,
    Query(q): Query<SessionsQuery>,
) -> ApiResult {
    let catalog = state
        .harnesses
        .lock()
        .await
        .as_ref()
        .map(|(_, catalog)| catalog.clone())
        .unwrap_or(Value::Null);
    let store = Store::open()?;
    if q.scope.as_deref() == Some("sidebar") {
        let before = match (q.before_updated_at, q.before_id.as_deref()) {
            (Some(at), Some(id)) => Some((at, id)),
            (None, None) => None,
            _ => return Err(bad_request("both cursor fields are required")),
        };
        let mut sessions = store.list_sidebar_chat_sessions(q.archived, before)?;
        let has_more = sessions.len() > 50;
        sessions.truncate(50);
        let next = sessions
            .last()
            .filter(|_| has_more)
            .map(|session| json!({ "updatedAt": session.updated_at, "id": session.id }));
        let busy = state.chat.busy_sessions().await;
        let sessions: Vec<Value> = sessions
            .iter()
            .map(|session| {
                sidebar_session_json(&store, session, busy.contains(&session.id), &catalog)
            })
            .collect::<Result<_>>()?;
        return Ok(Json(json!({ "sessions": sessions, "next": next })));
    }
    let sessions = match (q.project_id.as_deref(), q.scope.as_deref()) {
        (Some(project_id), _) => store.list_chat_sessions_by_project(project_id)?,
        (None, Some("all")) => store.list_all_chat_sessions()?,
        (None, _) => return Err(bad_request("projectId or scope=all is required")),
    };
    let busy = state.chat.busy_sessions().await;
    let sessions: Vec<Value> = sessions
        .iter()
        .map(|s| sidebar_session_json(&store, s, busy.contains(&s.id), &catalog))
        .collect::<Result<_>>()?;
    Ok(Json(json!({ "sessions": sessions })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateChatSessionReq {
    profile: Option<String>,
    project_id: String,
    harness: Option<String>,
    model: Option<String>,
    service_tier: Option<String>,
    permission_mode: Option<String>,
    #[serde(default)]
    plan_mode: bool,
    reasoning_level: Option<String>,
    autonomy: Option<Autonomy>,
}

async fn selected_chat_model(
    state: &AppState,
    harness: &str,
    profile: Option<&str>,
    model: Option<&str>,
) -> Result<Option<String>> {
    let payload = harnesses_payload(state, &HarnessQuery::default()).await;
    local::harness::selected_model(&payload, harness, profile, model)
}

fn new_chat_settings(
    project_id: String,
    preferences: crate::store::StoredUiState,
    catalog: &Value,
) -> Result<CreateChatSessionReq> {
    let harnesses = catalog
        .get("harnesses")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("OpenResearch could not load New Chat settings."))?;
    let saved = preferences.preferred_agent;
    let selected = match saved.as_ref() {
        Some(saved) => harnesses.iter().find(|h| {
            h.get("id").and_then(Value::as_str) == Some(&saved.harness)
                && h.get("profile")
                    .and_then(|p| p.get("id"))
                    .and_then(Value::as_str)
                    == saved.profile.as_deref()
        }),
        None => harnesses.iter().find(|h| {
            h.get("agentReady").and_then(Value::as_bool) == Some(true)
                && h["defaultModel"].is_string()
        }),
    }
    .ok_or_else(|| {
        anyhow!("Your New Chat agent is unavailable. Select an available agent in New Chat.")
    })?;
    if selected.get("agentReady").and_then(Value::as_bool) != Some(true) {
        return Err(anyhow!("Your New Chat agent is unavailable. Check its installation and sign-in in OpenResearch."));
    }
    let string = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
    let harness = string(selected.get("id"));
    let models = selected.get("models").and_then(Value::as_array);
    let model = saved
        .as_ref()
        .and_then(|s| s.model.clone())
        .filter(|model| local::harness::concrete_model(model))
        .or_else(|| string(selected.get("defaultModel")));
    let metadata = models.and_then(|models| {
        models
            .iter()
            .find(|m| m.get("id").and_then(Value::as_str) == model.as_deref())
    });
    let options = selected.get("options");
    let choices = metadata
        .and_then(|m| m.get("reasoningLevels"))
        .or_else(|| options.and_then(|o| o.get("reasoningLevels")))
        .and_then(Value::as_array);
    let has = |choices: Option<&Vec<Value>>, id: &str| {
        choices.is_some_and(|choices| {
            choices
                .iter()
                .any(|c| c.get("id").and_then(Value::as_str) == Some(id))
        })
    };
    let model_default = string(metadata.and_then(|m| m.get("defaultReasoningLevel")));
    let default_reasoning = model_default
        .filter(|id| has(choices, id))
        .or_else(|| has(choices, "default").then(|| "default".to_string()))
        .or_else(|| string(options.and_then(|o| o.get("defaultReasoningLevel"))))
        .or_else(|| string(choices.and_then(|c| c.first()).and_then(|c| c.get("id"))));
    let reasoning_level = if choices.is_none_or(Vec::is_empty) {
        Some("default".to_string())
    } else {
        saved
            .as_ref()
            .and_then(|s| s.reasoning_level.clone())
            .filter(|id| has(choices, id))
            .or(default_reasoning)
    };
    let tiers = metadata
        .and_then(|m| m.get("serviceTiers"))
        .and_then(Value::as_array);
    let service_tier = if harness.as_deref() == Some("codex") && tiers.is_some() {
        Some(
            saved
                .as_ref()
                .and_then(|s| s.service_tier.clone())
                .filter(|id| id == "default" || has(tiers, id))
                .unwrap_or_else(|| "default".to_string()),
        )
    } else {
        None
    };
    let permission_mode = match saved.as_ref() {
        Some(saved) => saved.permission_mode.clone(),
        None => string(options.and_then(|o| o.get("defaultPermissionMode"))),
    };
    Ok(CreateChatSessionReq {
        profile: string(selected.get("profile").and_then(|p| p.get("id"))),
        project_id,
        harness,
        model,
        service_tier,
        permission_mode,
        plan_mode: false,
        reasoning_level,
        autonomy: Some(preferences.preferred_autonomy.unwrap_or(Autonomy::Agentic)),
    })
}

async fn get_chat_session(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let session = Store::open()?
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    let busy = state.chat.is_busy(&id).await;
    Ok(Json(
        json!({ "session": state.chat.session_json(&session, busy).await }),
    ))
}

async fn chat_session_experiments(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    let experiments = store
        .list_experiments_by_chat_session(&id)?
        .into_iter()
        .map(|row| {
            let current_run = row
                .current_run
                .map(|run| {
                    let mut value = serde_json::to_value(ApiRun::new(&store, &run)?)?;
                    value["cancelRequested"] = json!(run.cancel_requested);
                    value["endedAt"] = json!(run.ended_at);
                    Ok::<_, crate::error::Error>(value)
                })
                .transpose()?;

            Ok(json!({
                "experiment": row.experiment,
                "projectName": row.project_name,
                "currentRun": current_run,
            }))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Json(json!({ "experiments": experiments })))
}

async fn create_chat_session(
    State(state): State<AppState>,
    Json(mut req): Json<CreateChatSessionReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let _admission = state
        .project_lifecycle
        .admit(&req.project_id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    let store = Store::open()?;
    store
        .get_local_project(&req.project_id)?
        .ok_or_else(|| not_found("project"))?;
    let catalog = harnesses_payload(&state, &HarnessQuery::default()).await;
    if req.harness.is_none() {
        if let Some(profile) = req.profile.as_deref() {
            req.harness = Some(
                local::agent_profiles::harness_of(profile)
                    .ok_or_else(|| bad_request("Choose an existing agent account"))?
                    .to_string(),
            );
        }
    }
    if req.harness.is_none() {
        let preferences = store.ui_state()?;
        req = new_chat_settings(req.project_id, preferences, &catalog).map_err(bad_request)?;
    }
    let harness = req
        .harness
        .ok_or_else(|| bad_request("no agent selected"))?;
    if !local::harness::is_chat_harness(&harness) {
        return Err(bad_request(format!("unknown harness: {harness}")));
    }
    let nonempty = |s: Option<String>| s.filter(|v| !v.trim().is_empty());
    let permission_mode = nonempty(req.permission_mode);
    let service_tier = nonempty(req.service_tier);
    if permission_mode
        .as_deref()
        .is_some_and(|mode| local::harness::permission_mode_for(&harness, mode).is_none())
    {
        return Err(bad_request(
            "invalid permission mode for the selected agent",
        ));
    }
    if service_tier
        .as_deref()
        .is_some_and(|tier| local::harness::service_tier_for(&harness, tier).is_none())
    {
        return Err(bad_request("invalid speed for the selected agent"));
    }
    if req.plan_mode && !local::harness::supports_command_plan(&harness) {
        return Err(bad_request("this agent activates Plan through permissions"));
    }
    if let Some(id) = &req.profile {
        local::agent_profiles::home(id, &harness).map_err(bad_request)?;
    }
    let model = local::harness::selected_model(
        &catalog,
        &harness,
        req.profile.as_deref(),
        req.model.as_deref(),
    )
    .map_err(bad_request)?;
    let session = StoredChatSession {
        profile_id: req.profile,
        id: format!("chat_{}", uuid::Uuid::new_v4()),
        project_id: req.project_id,
        harness,
        native_session_id: None,
        native_fork_at: None,
        title: None,
        title_source: None,
        model,
        service_tier,
        permission_mode,
        plan_mode: req.plan_mode,
        plan_reset_pending: false,
        reasoning_level: nonempty(req.reasoning_level),
        archived: false,
        context_usage_json: None,
        bootstrap_context: None,
        goal: None,
        autonomy: req.autonomy.map(|autonomy| autonomy.id().to_string()),
        active_leaf_id: None,
        side_parent_session_id: None,
        side_parent_leaf_id: None,
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    store.create_chat_session(&session)?;
    state.chat.emit_session(Some(session.clone())).await;
    Ok(Json(
        json!({ "session": state.chat.session_json(&session, false).await }),
    ))
}

/// Chats the user had in an agent's own CLI, for the composer's `/resume`
/// picker. Reading them walks the agent's store, so it stays off the executor.
async fn list_native_chats() -> ApiResult {
    let chats = tokio::task::spawn_blocking(|| {
        let owned = Store::open()?.native_ids_outside(&[])?;
        Ok::<_, crate::error::Error>(local::native_chats::list(
            &owned,
            local::native_chats::LISTING_LIMIT,
        ))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("native chat scan failed: {e}")))??;
    let chats: Vec<Value> = chats
        .iter()
        .map(|chat| {
            json!({
                "harness": chat.harness,
                "nativeId": chat.native_id,
                "title": chat.title,
                "cwd": chat.cwd,
                "updatedAt": chat.updated_at,
            })
        })
        .collect();
    Ok(Json(json!({ "chats": chats })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportNativeChatReq {
    project_id: String,
    harness: String,
    native_id: String,
    title: Option<String>,
}

/// Adopt one of those chats: a session bound to the agent's own id, with the
/// transcript backfilled so it does not open blank. Turns continue in the
/// agent's real home, since that is where the id resolves.
async fn import_native_chat(
    State(state): State<AppState>,
    Json(req): Json<ImportNativeChatReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    if !local::native_chats::is_importable(&req.harness) {
        return Err(bad_request("that agent's chats cannot be adopted"));
    }
    let catalog = harnesses_payload(&state, &HarnessQuery::default()).await;
    let native_model =
        local::native_chats::selection_model(&catalog, &req.harness, None, &req.native_id)?;
    let model =
        local::harness::selected_model(&catalog, &req.harness, None, native_model.as_deref())
            .map_err(bad_request)?;
    // As create does: a project whose delete is in flight takes no new sessions.
    let _admission = state
        .project_lifecycle
        .admit(&req.project_id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    let session = tokio::task::spawn_blocking(move || {
        local::chat::import_native_chat(
            &Store::open()?,
            &req.project_id,
            &req.harness,
            &req.native_id,
            req.title,
            model,
        )
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("import failed: {e}")))??;
    let session = state
        .chat
        .emit_session(Some(session))
        .await
        .ok_or_else(|| not_found("chat session"))?;
    Ok(Json(
        json!({ "session": state.chat.session_json(&session, false).await }),
    ))
}

async fn open_side_chat(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    let parent = Store::open()?
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    if parent.side_parent_session_id.is_some() {
        return Err(bad_request("a side chat cannot open its own side chat"));
    }
    let _admission = state
        .project_lifecycle
        .admit(&parent.project_id)
        .ok_or_else(|| bad_request("project deletion is in progress"))?;
    harnesses_payload(&state, &HarnessQuery::default()).await;
    let session = state.chat.open_side_chat(&parent).await.map_err(|error| {
        if error.is::<local::harness::ModelSelectionError>() {
            bad_request(error)
        } else {
            ApiError::from(error)
        }
    })?;
    Ok(Json(
        json!({ "session": state.chat.session_json(&session, false).await }),
    ))
}

async fn delete_chat_session(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_moving(&state)?;
    // Detached so a canceled request cannot stop between the native purge and the row delete.
    let chat = state.chat.clone();
    tokio::spawn(async move { chat.delete_session(&id).await })
        .await
        .map_err(|error| anyhow!("chat deletion task failed: {error}"))??;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateChatSessionReq {
    agent: Option<crate::store::StoredAgentSelection>,
    harness: Option<String>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    profile: Option<Option<String>>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    model: Option<Option<String>>,
    service_tier: Option<String>,
    reasoning_level: Option<String>,
    archived: Option<bool>,
    title: Option<String>,
    plan_mode: Option<bool>,
    permission_mode: Option<String>,
    /// Present-and-null clears the goal, which is why it is doubly wrapped.
    #[serde(default, deserialize_with = "present_nullable_string")]
    goal: Option<Option<String>>,
    autonomy: Option<Autonomy>,
    /// `false` keeps a delegate for the user: see `release_delegate`.
    notify_parent: Option<bool>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeConfigurationInput {
    option_id: String,
    value: String,
}

async fn set_chat_configuration(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<NativeConfigurationInput>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let session = store
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    let saved = store
        .acp_session_state(&id)?
        .ok_or_else(|| bad_request("This chat is not an ACP session"))?;
    let _reservation;
    let client = match state.chat.acp.connected(&id).await {
        Ok(client) => {
            _reservation = None;
            client
        }
        Err(_) => {
            _reservation = Some(
                local::chat::TurnGuard::claim(&state.chat, &id, None)
                    .await
                    .ok_or_else(|| {
                        bad_request(
                            "Chat is busy; try changing the option again after the turn finishes",
                        )
                    })?,
            );
            let native_id = session.native_session_id.as_deref().ok_or_else(|| {
                bad_request("Start the conversation before changing agent options")
            })?;
            let project = store
                .get_local_project(&session.project_id)?
                .ok_or_else(|| not_found("project"))?;
            let session_id = id.clone();
            let (cwd, _) = tokio::task::spawn_blocking(move || {
                local::opencode::ensure_playbook(
                    &project,
                    &session_id,
                    Some(local::harness::acp::SESSION_SKILLS_DIR),
                )
            })
            .await
            .map_err(bad_request)?
            .map_err(bad_request)?;
            let client = state
                .chat
                .acp
                .get(&id, &saved.launch, &cwd)
                .await
                .map_err(bad_request)?;
            let mcp_servers = local::harness::acp::orx_mcp_servers(&id, state.chat.up_port())
                .map_err(bad_request)?;
            match client.restore(native_id, &cwd, mcp_servers).await {
                Ok(configuration) => store.set_acp_configuration(&id, &configuration)?,
                Err(error) => {
                    state.chat.acp.stop(&id).await;
                    return Err(bad_request(error));
                }
            }
            client.end_turn();
            client
        }
    };
    client
        .configure(&id, &input.option_id, &input.value)
        .await
        .map_err(bad_request)?;
    let session = Store::open()?.get_chat_session(&id)?;
    let session = state
        .chat
        .emit_session(session)
        .await
        .ok_or_else(|| bad_request("Chat session no longer exists"))?;
    Ok(Json(
        state
            .chat
            .session_json(&session, state.chat.is_busy(&id).await)
            .await,
    ))
}

async fn update_chat_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateChatSessionReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    let session = if req.agent.is_some() || req.harness.is_some() || req.profile.is_some() {
        let current = Store::open()?
            .get_chat_session(&id)?
            .ok_or_else(|| not_found("chat session"))?;
        let mut selection = if let Some(agent) = req.agent {
            agent
        } else {
            let harness = req.harness.unwrap_or_else(|| current.harness.clone());
            if !local::harness::is_chat_harness(&harness) {
                return Err(bad_request("Choose an available agent"));
            }
            let same_harness = harness == current.harness;
            let profile = req.profile.unwrap_or_else(|| {
                if same_harness {
                    current.profile_id.clone()
                } else {
                    None
                }
            });
            let same_agent = same_harness && profile == current.profile_id;
            StoredAgentSelection {
                profile,
                harness: harness.clone(),
                model: req.model.unwrap_or_else(|| {
                    if same_agent {
                        current.model.clone()
                    } else {
                        None
                    }
                }),
                service_tier: req.service_tier.or_else(|| {
                    if same_harness {
                        current.service_tier.clone()
                    } else {
                        None
                    }
                }),
                permission_mode: req.permission_mode.or_else(|| {
                    if same_harness {
                        current.permission_mode.clone()
                    } else {
                        None
                    }
                }),
                reasoning_level: req.reasoning_level.or_else(|| {
                    if same_harness {
                        current.reasoning_level.clone()
                    } else {
                        None
                    }
                }),
            }
        };
        if let Some(profile) = &selection.profile {
            local::agent_profiles::home(profile, &selection.harness).map_err(bad_request)?;
        }
        let catalog = harnesses_payload(&state, &HarnessQuery::default()).await;
        selection.model = local::harness::selected_model(
            &catalog,
            &selection.harness,
            selection.profile.as_deref(),
            selection.model.as_deref(),
        )
        .map_err(bad_request)?;
        if selection.permission_mode.as_deref().is_some_and(|mode| {
            local::harness::permission_mode_for(&selection.harness, mode).is_none()
        }) {
            return Err(bad_request(
                "invalid permission mode for the selected agent",
            ));
        }
        if selection.service_tier.as_deref().is_some_and(|tier| {
            local::harness::service_tier_for(&selection.harness, tier).is_none()
        }) {
            return Err(bad_request("invalid speed for the selected agent"));
        }
        let window = catalog["harnesses"]
            .as_array()
            .and_then(|entries| {
                entries.iter().find(|entry| {
                    entry["id"].as_str() == Some(&selection.harness)
                        && entry["profile"]["id"].as_str() == selection.profile.as_deref()
                })
            })
            .and_then(|entry| entry["models"].as_array())
            .and_then(|models| {
                models
                    .iter()
                    .find(|model| model["id"].as_str() == selection.model.as_deref())
            })
            .and_then(|model| model["contextWindow"].as_u64());
        state
            .chat
            .set_agent(&id, selection, window)
            .await
            .map_err(bad_request)?
    } else if let Some(title) = req.title {
        let title = title.trim();
        if title.is_empty() {
            return Err(bad_request("title cannot be empty"));
        }
        state
            .chat
            .set_title(&id, title)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if let Some(archived) = req.archived {
        state
            .chat
            .set_archived(&id, archived)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if let Some(plan_mode) = req.plan_mode {
        state
            .chat
            .set_plan_mode(&id, plan_mode)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if let Some(goal) = req.goal {
        let goal = goal
            .map(|goal| goal.trim().to_string())
            .filter(|goal| !goal.is_empty());
        state
            .chat
            .set_goal(&id, goal.as_deref())
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if let Some(permission_mode) = req.permission_mode {
        state
            .chat
            .set_permission_mode(&id, &permission_mode)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if let Some(autonomy) = req.autonomy {
        state
            .chat
            .set_autonomy(&id, autonomy)
            .await?
            .ok_or_else(|| not_found("chat session"))?
    } else if req.notify_parent == Some(false) {
        let session = local::chat::delegation::release_delegate(&Store::open()?, &id)?
            .ok_or_else(|| not_found("chat session"))?;
        state.chat.emit_session(Some(session.clone())).await;
        session
    } else {
        return Err(bad_request("nothing to update"));
    };
    let busy = state.chat.is_busy(&id).await;
    Ok(Json(
        json!({ "session": state.chat.session_json(&session, busy).await }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentQuery {
    part_id: String,
}

async fn chat_subagent(Path(id): Path<String>, Query(query): Query<SubagentQuery>) -> ApiResult {
    let session = Store::open()?
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    let mut messages = local::chat::list_messages(&id)?;
    let spawn = messages
        .iter_mut()
        .find_map(|message| local::chat::find_part_mut(&mut message.parts, &query.part_id))
        .ok_or_else(|| not_found("subagent"))?;
    Ok(Json(json!(local::chat::subagent::snapshot(
        &session, spawn
    )?)))
}

/// The message an agent cited by ref: where it lives, the agent that wrote it
/// if one did, and its first words, for the dashboard's link to it.
async fn cited_chat_message(Path(cited): Path<String>) -> ApiResult {
    let store = Store::open()?;
    let message = local::chat::delegation::cited_message(&store, &cited)?
        .ok_or_else(|| not_found("message"))?;
    let source = local::chat::stored_to_wire(&message)
        .parts
        .into_iter()
        .find_map(|part| part.orx);
    // An agent wrote it: this chat's own, or another chat's for the user.
    let agent_chat = match &source {
        Some(source) => source.from.clone().filter(|from| from.starts_with("chat_")),
        None => (message.role == "assistant").then(|| message.session_id.clone()),
    };
    let harness = message
        .native_harness
        .clone()
        .filter(|_| message.role == "assistant")
        .or(agent_chat
            .map(|id| store.get_chat_session(&id))
            .transpose()?
            .flatten()
            .map(|session| session.harness));
    let preview =
        local::chat::delegation::preview(&local::chat::delegation::message_text(&message));
    let project_id = store
        .get_chat_session(&message.session_id)?
        .map(|session| session.project_id);
    Ok(Json(json!({
        "projectId": project_id,
        "sessionId": message.session_id,
        "messageId": message.id,
        "harness": harness,
        "preview": preview,
    })))
}

async fn chat_messages(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    // Messages first: these are two connections, so a turn writing between them
    // can only leave the leaf *ahead* of the list, which the client falls back on
    // safely — the other order hides a reply that is already in the list.
    let messages = local::chat::list_messages(&id)?;
    let session = Store::open()?
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    // The host restores its durable queue at startup; return the live snapshot
    // so dispatch progress and cancellation are reflected immediately.
    let queued = state.chat.queued_items(&id);
    Ok(Json(json!({
        "messages": messages,
        "queued": queued,
        "activeLeafId": session.active_leaf_id,
    })))
}

async fn chat_submission(Path((id, client_id)): Path<(String, String)>) -> ApiResult {
    let store = Store::open()?;
    if store.get_chat_session(&id)?.is_none() {
        return Err(not_found("chat session"));
    }
    Ok(Json(
        json!({ "accepted": store.has_chat_submission(&id, &client_id)? }),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendChatReq {
    text: String,
    client_turn_id: Option<String>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    model: Option<Option<String>>,
    service_tier: Option<String>,
    permission_mode: Option<String>,
    plan_mode: Option<bool>,
    reasoning_level: Option<String>,
    #[serde(default)]
    images: Vec<local::chat::ImageAttachment>,
    #[serde(default)]
    annotations: Vec<local::chat::TextAnnotation>,
    /// `"steer"` hands the message to a turn already running; omitted (an
    /// older client) keeps the parked-queue path.
    mode: Option<SendMode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum SendMode {
    Steer,
}

#[derive(Deserialize)]
struct ShellCommandReq {
    command: String,
}

/// Runs a composer `!` command where the agent works and records it as a
/// user-side message. Refused while a turn runs: a message landing mid-stream
/// would sit between the turn's own messages and never reach the agent. (A
/// turn started during the command itself is not caught; the card still shows.)
async fn run_shell_command(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ShellCommandReq>,
) -> ApiResult {
    reject_if_stopping(&state)?;
    reject_if_moving(&state)?;
    let command = req.command.trim().to_string();
    if command.is_empty() {
        return Err(bad_request("command is required"));
    }
    if state.chat.is_busy(&id).await {
        return Err(ApiError(StatusCode::CONFLICT, "session is busy".into()));
    }
    let session_id = id.clone();
    let root = tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        let session = store
            .get_chat_session(&session_id)?
            .ok_or_else(|| not_found("chat session"))?;
        let project = store
            .get_local_project(&session.project_id)?
            .ok_or_else(|| not_found("project"))?;
        session_checkout_root(&store, &project, &session.id)
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("shell task failed: {e}")))??;
    let message = state
        .chat
        .run_shell_command(&id, command, root)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "message": message })))
}

async fn compact_chat_session(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    reject_if_stopping(&state)?;
    reject_if_moving(&state)?;
    if state.chat.is_busy(&id).await {
        return Err(ApiError(StatusCode::CONFLICT, "session is busy".into()));
    }
    let probe = id.clone();
    tokio::task::spawn_blocking(move || {
        Store::open()?
            .get_chat_session(&probe)?
            .ok_or_else(|| not_found("chat session"))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("compact task failed: {e}")))??;
    let message = state
        .chat
        .compact_session(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(json!({ "message": message })))
}

async fn send_chat_message(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SendChatReq>,
) -> ApiResult {
    reject_if_stopping(&state)?;
    reject_if_moving(&state)?;
    let store = Store::open()?;
    let session = store
        .get_chat_session(&id)?
        .ok_or_else(|| not_found("chat session"))?;
    let project = store
        .get_local_project(&session.project_id)?
        .ok_or_else(|| not_found("project"))?;
    let harness = session.harness;
    let slash_skills = local::chat::builtin_slash_skill_names(&project, &req.text);
    let text = req.text.trim().to_string();
    let annotations = req
        .annotations
        .into_iter()
        .filter(|annotation| !annotation.text.trim().is_empty())
        .collect::<Vec<_>>();
    if text.is_empty() && req.images.is_empty() && annotations.is_empty() {
        return Err(bad_request("text is required"));
    }
    let overrides = local::chat::TurnOverrides {
        clear_model: req.model == Some(None),
        model: req.model.flatten(),
        service_tier: req.service_tier,
        permission_mode: req.permission_mode,
        permission_revision: None,
        plan_mode: req.plan_mode,
        plan_revision: None,
        reasoning_level: req.reasoning_level,
    };
    // The turn runs in the background; progress streams over /api/events.
    let (response, capture_message) = if matches!(req.mode, Some(SendMode::Steer)) {
        let result = state
            .chat
            .steer_message(
                &id,
                text,
                overrides,
                req.images,
                annotations,
                req.client_turn_id,
                None,
            )
            .await
            .map_err(|error| {
                if local::chat::is_client_turn_conflict(&error) {
                    ApiError(StatusCode::CONFLICT, error.to_string())
                } else {
                    // A failure can occur after the message was admitted.
                    ApiError::from(error)
                }
            })?;
        match result {
            Some(turn) => {
                let capture = !turn.existing;
                (json!({ "ok": true, "turn": turn }), capture)
            }
            None => (json!({ "ok": true, "steered": true }), true),
        }
    } else {
        let result = state
            .chat
            .send_message(
                &id,
                text,
                overrides,
                req.images,
                annotations,
                req.client_turn_id,
                None,
            )
            .await
            .map_err(|error| {
                if local::chat::is_client_turn_conflict(&error) {
                    ApiError(StatusCode::CONFLICT, error.to_string())
                } else {
                    ApiError::from(error)
                }
            })?;
        let capture = !result.existing;
        (json!({ "ok": true, "turn": result }), capture)
    };
    if capture_message {
        crate::telemetry::capture_chat_message_sent(&harness);
        for skill in slash_skills {
            crate::telemetry::capture_skill_invoked(skill, "slash", Some(&harness));
        }
    }
    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoverChatReq {
    action: String,
    #[serde(default, deserialize_with = "present_nullable_string")]
    model: Option<Option<String>>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    service_tier: Option<Option<String>>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    permission_mode: Option<Option<String>>,
    plan_mode: Option<bool>,
    #[serde(default, deserialize_with = "present_nullable_string")]
    reasoning_level: Option<Option<String>>,
}

fn present_nullable_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

async fn recover_chat_turn(
    State(state): State<AppState>,
    Path((id, turn_id)): Path<(String, String)>,
    Json(req): Json<RecoverChatReq>,
) -> ApiResult {
    reject_if_stopping(&state)?;
    reject_if_moving(&state)?;
    let result = state
        .chat
        .recover_turn(
            &id,
            &turn_id,
            &req.action,
            local::chat::RecoveryOverrides {
                model: req.model,
                service_tier: req.service_tier,
                permission_mode: req.permission_mode,
                plan_mode: req.plan_mode,
                reasoning_level: req.reasoning_level,
            },
        )
        .await
        .map_err(|error| ApiError(StatusCode::CONFLICT, error.to_string()))?;
    Ok(Json(json!({ "ok": true, "turn": result })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForkChatReq {
    /// The message being re-sampled: an assistant reply to retry, or a user
    /// message to re-ask with `text`.
    message_id: String,
    /// Edited prompt. Absent re-sends the original message unchanged.
    text: Option<String>,
}

async fn fork_chat_turn(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<ForkChatReq>,
) -> ApiResult {
    reject_if_stopping(&state)?;
    reject_if_moving(&state)?;
    let edit_text = match req.text {
        Some(text) if !text.trim().is_empty() => Some(text),
        Some(_) => return Err(bad_request("text is required")),
        None => None,
    };
    let invocation = if let Some(text) = edit_text.as_deref() {
        let store = Store::open()?;
        let session = store
            .get_chat_session(&id)?
            .ok_or_else(|| not_found("chat session"))?;
        let project = store
            .get_local_project(&session.project_id)?
            .ok_or_else(|| not_found("project"))?;
        let skills = local::chat::builtin_slash_skill_names(&project, text);
        Some((session.harness, skills))
    } else {
        None
    };
    let kind = edit_text
        .map(local::chat::ForkKind::Edit)
        .unwrap_or(local::chat::ForkKind::Retry);
    // A fork re-samples under the session's current settings, so it takes no
    // overrides — the turn runs in the background and streams over /api/events.
    let started = state
        .chat
        .fork_turn(&id, &req.message_id, kind)
        .await
        .map_err(bad_request)?;
    if started {
        if let Some((harness, skills)) = invocation {
            crate::telemetry::capture_chat_message_sent(&harness);
            for skill in skills {
                crate::telemetry::capture_skill_invoked(skill, "slash", Some(&harness));
            }
        }
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectBranchReq {
    /// The fork to show. Its whole branch comes with it.
    leaf_id: String,
}

async fn select_chat_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<SelectBranchReq>,
) -> ApiResult {
    reject_if_moving(&state)?;
    state
        .chat
        .select_branch(&id, &req.leaf_id)
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

/// Raw bytes of a chat attachment (image or PDF), by bare file name.
async fn chat_attachment(
    Path(name): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> std::result::Result<Response, ApiError> {
    // Names are server-minted (att-<uuid>__<name>.<ext>); anything else is rejected.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        || name.contains("..")
    {
        return Err(bad_request("invalid attachment name"));
    }
    let (type_path, file) = tokio::task::spawn_blocking(move || {
        let path = local::chat::attachments_dir()?.join(&name);
        let file = std::fs::File::open(&path).map_err(|_| not_found("attachment"))?;
        let metadata = file.metadata().map_err(|_| not_found("attachment"))?;
        if !metadata.is_file() {
            return Err(not_found("attachment"));
        }
        let resolved = crate::paths::canonicalize(&path).map_err(|_| not_found("attachment"))?;
        Ok((resolved.to_string_lossy().into_owned(), file))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("attachment task failed: {e}")))??;
    crate::commands::file_serve::disk_response(
        &type_path,
        file,
        local::files::presentation_for_path(&type_path),
        &method,
        &headers,
        "max-age=31536000, immutable",
    )
    .await
    .map_err(ApiError::from)
}

async fn interrupt_chat(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    state.chat.interrupt_by_user(&id).await?;
    Ok(Json(json!({ "ok": true })))
}

/// Cancel one message parked behind a running turn (the ✕ on a queued chip).
async fn cancel_queued_chat(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(String, String)>,
) -> ApiResult {
    let removed = state.chat.cancel_queued(&id, &item_id)?;
    Ok(Json(json!({ "ok": true, "removed": removed })))
}

/// Retry one queued message after its safe delivery budget was exhausted.
async fn retry_queued_chat(
    State(state): State<AppState>,
    Path((id, item_id)): Path<(String, String)>,
) -> ApiResult {
    reject_if_stopping(&state)?;
    reject_if_moving(&state)?;
    let retried = state.chat.retry_queued(&id, &item_id)?;
    if !retried {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "queued message is no longer available for retry".into(),
        ));
    }
    Ok(Json(json!({ "ok": true, "retried": retried })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RespondReq {
    prompt_id: String,
    #[serde(default = "default_true")]
    approve: bool,
    #[serde(default)]
    resume_mode: Option<String>,
    #[serde(default)]
    answers: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    annotations: Vec<local::chat::TextAnnotation>,
}

fn default_true() -> bool {
    true
}

/// Answer an interactive prompt (plan / permission / question) on a session.
async fn respond_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RespondReq>,
) -> ApiResult {
    reject_if_stopping(&state)?;
    state
        .chat
        .respond(local::chat::PromptAnswer {
            session_id: id,
            prompt_id: req.prompt_id,
            approve: req.approve,
            resume_mode: req.resume_mode,
            answers: req.answers,
            note: req.note,
            annotations: req
                .annotations
                .into_iter()
                .filter(|annotation| !annotation.text.trim().is_empty())
                .collect(),
            from: None,
        })
        .await
        .map_err(bad_request)?;
    Ok(Json(json!({ "ok": true })))
}

/// The `orx mcp` bridge relaying one blocked tool call from a plan-mode
/// claude turn. The response body is the permission decision verbatim
/// (`{"behavior":"allow",…}` / `{"behavior":"deny",…}`) — the bridge
/// stringifies it into the MCP tool result unchanged. Deliberately long-held:
/// it returns when the user answers the card (or policy/timeout decides).
async fn bridge_permission(
    State(state): State<AppState>,
    Json(req): Json<BridgePermissionReq>,
) -> ApiResult {
    let decision = state
        .chat
        .request_permission(&req.session_id, &req.token, &req.tool_name, req.tool_input)
        .await
        .map_err(bad_request)?;
    Ok(Json(serde_json::to_value(decision).map_err(bad_request)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BridgePermissionReq {
    session_id: String,
    token: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Value,
}

// --- agent ----------------------------------------------------------------

async fn agent_status(State(state): State<AppState>) -> Json<Value> {
    let agents = state.agent.status().await;
    Json(json!({ "running": !agents.is_empty(), "agents": agents }))
}

// --- /api/events SSE ------------------------------------------------------

async fn events(State(state): State<AppState>) -> Response {
    let Ok(client) = api_v2::event_client() else {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // Small buffer on purpose: run.log events can carry ~MB payloads, and a
    // stalled client must backpressure the loop, not queue hundreds of MB.
    let (tx, rx) = mpsc::channel::<Event>(16);
    // Subscribed before the snapshot, so no run change between them is lost.
    let runs = api_v2::run_events().ok();
    let chat = tx.clone();
    tokio::spawn(async move {
        event_loop(tx, runs).await;
        drop(client);
    });
    // Chat events ride the same stream: chat.session / chat.message / chat.busy.
    tokio::spawn(forward_chat_events(state.chat.subscribe(), chat));
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|ev| (Ok::<_, Infallible>(ev), rx))
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn forward_chat_events(
    mut chat_rx: tokio::sync::broadcast::Receiver<(&'static str, Value)>,
    tx: mpsc::Sender<Event>,
) {
    loop {
        let event = tokio::select! {
            _ = tx.closed() => return,
            event = chat_rx.recv() => event,
        };
        match event {
            Ok((name, data)) => {
                if tx.send(json_event(name, &data)).await.is_err() {
                    return;
                }
            }
            // The client must repair snapshots after missed edge-only events.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                if tx
                    .send(json_event("resync.required", &json!({})))
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// Diff state for one SSE subscriber.
#[derive(Default)]
struct EventCursor {
    projects: HashMap<String, i64>,
    experiments: HashMap<String, i64>,
    files: HashMap<String, u64>,
    runs: HashMap<String, String>,
    /// Runs whose local log file may still grow: in flight, or finished with a final
    /// collection not yet streamed.
    tailed: HashSet<String>,
    /// Live engine runs, whose hosts report log growth while this stream is open.
    followed: HashMap<String, Box<dyn Send>>,
    log_offsets: HashMap<String, u64>,
    log_generations: HashMap<String, String>,
    chat_active_runs: Option<Value>,
    /// Last update status sent. Unlike the rest of the cursor this isn't store
    /// state — the updater is a separate process, so its progress reaches the UI
    /// through the same diff the store changes do.
    update: Option<updates::UpdateStatus>,
    update_sampled_at: Option<std::time::Instant>,
}

/// How often the event loop re-reads update status. The 500ms store cadence is
/// there for run logs; nothing about an update needs that resolution.
const UPDATE_SAMPLE_INTERVAL: Duration = Duration::from_secs(10);

/// Run changes arrive from the engine's Bus; projects, experiments, files, updates and local
/// log files are diffed every 500ms. Ends when the subscriber disconnects.
async fn event_loop(
    tx: mpsc::Sender<Event>,
    mut runs: Option<tokio::sync::broadcast::Receiver<crate::engine::bus::RunEvent>>,
) {
    use tokio::sync::broadcast::error::RecvError;
    let mut cursor = EventCursor::default();
    let mut first = true;
    loop {
        if !first {
            // An idle store never sends, so a failed send can't be the only
            // disconnect signal — watch the receiver side too or the loop
            // leaks per closed EventSource.
            let event = tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(500)) => None,
                _ = tx.closed() => return,
                event = async {
                    match runs.as_mut() {
                        Some(receiver) => receiver.recv().await,
                        None => std::future::pending().await,
                    }
                } => Some(event),
            };
            let batch = match event {
                None => None,
                Some(Ok(event)) => Some(
                    Store::open().and_then(|store| run_events(&store, &mut cursor, Some(event))),
                ),
                // Lagged: resynchronize every run from the store.
                Some(Err(RecvError::Lagged(_))) => {
                    Some(Store::open().and_then(|store| run_events(&store, &mut cursor, None)))
                }
                Some(Err(RecvError::Closed)) => {
                    runs = None;
                    continue;
                }
            };
            if let Some(batch) = batch {
                for ev in batch.unwrap_or_default() {
                    if tx.send(ev).await.is_err() {
                        return;
                    }
                }
                continue;
            }
        }
        if tx.is_closed() {
            return;
        }
        // Store hiccups (locked db) just skip a tick.
        let mut batch = collect_events(&mut cursor).unwrap_or_default();
        if first {
            batch.extend(
                Store::open()
                    .and_then(|store| run_events(&store, &mut cursor, None))
                    .unwrap_or_default(),
            );
        }
        first = false;
        for ev in batch {
            if tx.send(ev).await.is_err() {
                return;
            }
        }
    }
}

fn json_event(name: &str, data: &Value) -> Event {
    Event::default().event(name).data(data.to_string())
}

/// One non-run diff pass. On the first pass everything is "changed", so a fresh
/// subscriber gets a full snapshot and needs no separate baseline fetches.
fn collect_events(cursor: &mut EventCursor) -> Result<Vec<Event>> {
    let mut out = Vec::new();

    // Sampled far below the 2Hz loop — the updater works on the scale of
    // minutes, and this reads files. `cursor.update` is committed only once the
    // batch is certain: every `?` below discards it (`event_loop` swallows the
    // error), and a cursor that had already moved would never re-emit, leaving
    // the restart banner permanently unshown for that subscriber.
    let sampled_update = cursor
        .update_sampled_at
        .is_none_or(|at| at.elapsed() >= UPDATE_SAMPLE_INTERVAL)
        .then(|| {
            cursor.update_sampled_at = Some(std::time::Instant::now());
            updates::status()
        })
        .filter(|update| cursor.update.as_ref() != Some(update));
    if let Some(update) = &sampled_update {
        out.push(json_event("update.status", &json!(update)));
    }

    let store = Store::open()?;
    for project in store.list_local_projects()? {
        if cursor.projects.get(&project.id) != Some(&project.updated_at) {
            cursor
                .projects
                .insert(project.id.clone(), project.updated_at);
            out.push(json_event(
                "project.updated",
                &json!({ "project": project_json(&project) }),
            ));
        }
        push_experiment_events(&store, &project.id, cursor, &mut out)?;
        // Artifacts appear live — anything written into the directory (by the
        // agent or the user) pings the UI to refetch the listing.
        let fp = local::files::fingerprint(&project);
        if cursor.files.get(&project.id) != Some(&fp) {
            cursor.files.insert(project.id.clone(), fp);
            out.push(json_event(
                "files.updated",
                &json!({ "projectId": project.id }),
            ));
        }
    }

    // Cap log bytes per tick so one pass never materializes a huge batch —
    // remainders (whole-log replays included) stream out on later ticks.
    let mut log_budget: u64 = 2_000_000;
    let tailed: Vec<String> = cursor.tailed.iter().cloned().collect();
    for run_id in tailed {
        let delta = push_log_delta(&run_id, cursor, &mut out, &mut log_budget);
        let live = cursor
            .runs
            .get(&run_id)
            .is_some_and(|status| !is_terminal(status));
        if !live && !delta {
            cursor.tailed.remove(&run_id);
        }
    }
    // Committed only now that the batch is certain to be returned.
    if let Some(update) = sampled_update {
        cursor.update = Some(update);
    }
    Ok(out)
}

/// Run events for one Bus event, or for every recent run when `event` is `None` (the first
/// pass, or after the subscriber lagged).
fn run_events(
    store: &Store,
    cursor: &mut EventCursor,
    event: Option<crate::engine::bus::RunEvent>,
) -> Result<Vec<Event>> {
    use crate::engine::bus::RunEvent;
    let first = cursor.chat_active_runs.is_none();
    let runs = match &event {
        None => store.list_runs(200)?,
        Some(RunEvent::Changed { run_id, .. } | RunEvent::LogCollection { run_id }) => {
            store.get_run(run_id)?.into_iter().collect()
        }
        Some(RunEvent::Grew { run_id, len, .. }) => {
            // A followed engine run's log grew on its host; the client refetches it.
            return Ok(vec![json_event(
                "run.log",
                &json!({"runId": run_id, "dataBase64": "", "offset": len}),
            )]);
        }
        Some(RunEvent::Host { alias, root, state }) => {
            return Ok(vec![json_event(
                "host.updated",
                &api_v2::host_view(store, alias, root, state)?,
            )])
        }
        Some(RunEvent::Notices { .. }) => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    if event.is_none() {
        for (alias, root, state) in store.engine_hosts()? {
            out.push(json_event(
                "host.updated",
                &api_v2::host_view(store, &alias, &root, &state)?,
            ));
        }
    }
    for run in runs {
        if !cursor.runs.contains_key(&run.id) {
            // Live runs replay their whole log through the stream (chunked per
            // tick); runs already finished start at EOF — backfill is /api/runs/{id}/log.
            let start = if first && is_terminal(&run.status) {
                log_size(&run.id)
            } else {
                0
            };
            cursor.log_offsets.insert(run.id.clone(), start);
        }
        cursor.runs.insert(run.id.clone(), run.status.clone());
        cursor.tailed.insert(run.id.clone());
        if crate::engine::is_engine_row(&run) && !is_terminal(&run.status) {
            if !cursor.followed.contains_key(&run.id) {
                if let Ok(follow) = api_v2::follow_log(&run.id) {
                    cursor.followed.insert(run.id.clone(), follow);
                }
            }
        } else {
            cursor.followed.remove(&run.id);
        }
        out.push(json_event(
            "run.updated",
            &json!({ "run": ApiRun::new(store, &run)? }),
        ));
    }
    let active_runs = chat_active_runs_json(store)?;
    if cursor.chat_active_runs.as_ref() != Some(&active_runs) {
        out.push(json_event("chat.runs", &json!({})));
        cursor.chat_active_runs = Some(active_runs);
    }
    Ok(out)
}

fn push_experiment_events(
    store: &Store,
    project_id: &str,
    cursor: &mut EventCursor,
    out: &mut Vec<Event>,
) -> Result<()> {
    for exp in store.list_experiments_by_project(project_id)? {
        if cursor.experiments.get(&exp.id) != Some(&exp.updated_at) {
            cursor.experiments.insert(exp.id.clone(), exp.updated_at);
            out.push(json_event(
                "experiment.updated",
                &json!({ "experiment": exp }),
            ));
        }
    }
    Ok(())
}

/// True when this pass sent or reset anything.
fn push_log_delta(
    run_id: &str,
    cursor: &mut EventCursor,
    out: &mut Vec<Event>,
    budget: &mut u64,
) -> bool {
    let offset = *cursor.log_offsets.entry(run_id.to_owned()).or_insert(0);
    let expected = cursor.log_generations.get(run_id).map(String::as_str);
    let Ok(snapshot) = crate::jobs::ssh::service::logs::read(run_id, offset, *budget, expected)
    else {
        return false;
    };
    let reset = expected.is_some_and(|generation| generation != snapshot.generation)
        || snapshot.offset != offset;
    if snapshot.bytes.is_empty() && !reset {
        return false;
    }
    *budget -= snapshot.bytes.len() as u64;
    cursor.log_offsets.insert(
        run_id.to_owned(),
        snapshot.offset + snapshot.bytes.len() as u64,
    );
    cursor
        .log_generations
        .insert(run_id.to_owned(), snapshot.generation.clone());
    out.push(json_event("run.log", &json!({
        "runId":run_id, "dataBase64":base64::engine::general_purpose::STANDARD.encode(&snapshot.bytes),
        "offset":snapshot.offset, "generation":snapshot.generation,
    })));
    true
}

fn log_size(run_id: &str) -> u64 {
    crate::jobs::ssh::service::logs::read(run_id, 0, 0, None).map_or(0, |snapshot| snapshot.size)
}

// --- embedded SPA ----------------------------------------------------------

/// ui/dist, embedded at release build time (debug builds read from disk).
#[derive(rust_embed::RustEmbed)]
#[folder = "ui/dist"]
#[cfg_attr(debug_assertions, allow_missing = true)]
struct UiDist;

#[cfg(not(debug_assertions))]
const _: () = assert!(
    !include_str!("../../ui/dist/index.html").is_empty(),
    "Run pnpm build in ui/ before a release build"
);

const NOT_BUILT_PAGE: &str = "<!doctype html><html><head><title>orx up</title></head>\
<body style=\"font-family:system-ui;background:#111;color:#ddd;display:grid;place-items:center;height:100vh;margin:0\">\
<div><h1>UI not built</h1><p>Run <code>pnpm build</code> in <code>ui/</code>, then rebuild orx.</p>\
<p>The API is live at <code>/api/health</code>.</p></div></body></html>";

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("json") | Some("map") => "application/json",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn asset_response(path: &str, file: rust_embed::EmbeddedFile) -> Response {
    // index.html must revalidate every load or browsers heuristically cache it
    // and keep loading a stale (hashed) bundle; the hashed assets themselves
    // are immutable by name. favicon.svg is likewise served under a fixed name.
    let cache = if path == "index.html" || path == "favicon.svg" {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    (
        [
            (header::CONTENT_TYPE, mime_for(path)),
            (header::CACHE_CONTROL, cache),
        ],
        file.data.into_owned(),
    )
        .into_response()
}

/// Every non-/api path: exact asset if it exists, index.html
/// otherwise (SPA client routing), friendly page when the UI isn't built.
pub(crate) async fn spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path == "api" {
        return not_found("route").into_response();
    }
    let candidate = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = UiDist::get(candidate) {
        return asset_response(candidate, file);
    }
    match UiDist::get("index.html") {
        Some(file) => asset_response("index.html", file),
        None => Html(NOT_BUILT_PAGE).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn event_text(events: Vec<Event>) -> String {
        let stream = futures::stream::iter(events.into_iter().map(Ok::<_, Infallible>));
        let bytes = axum::body::to_bytes(Sse::new(stream).into_response().into_body(), 8192)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn host_bus_events_serialize_as_dashboard_sse_and_initial_snapshot() {
        use crate::engine::bus::{Bus, RunEvent};
        use crate::engine::transport::{HostClass, HostState, Status};
        let directory = std::env::temp_dir().join(format!("orx-host-sse-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(directory.clone()).unwrap();
        let bus = Bus::new();
        let mut receiver = bus.subscribe();
        let mut cursor = EventCursor::default();
        for (status, expected) in [
            (Status::Live, "live"),
            (Status::Backoff, "backoff"),
            (Status::Live, "live"),
            (Status::NeedsAuth, "needs_auth"),
        ] {
            let state = HostState {
                status,
                since_ms: 123,
                next_retry_ms: Some(456),
                cause: Some("fixture".into()),
                ..HostState::default()
            };
            store
                .seed_engine_host("lab", "/engine", Some(HostClass::Key), &state)
                .unwrap();
            bus.publish(RunEvent::Host {
                alias: "lab".into(),
                root: "/engine".into(),
                state: state.clone(),
            });
            let events =
                run_events(&store, &mut cursor, Some(receiver.recv().await.unwrap())).unwrap();
            let text = event_text(events).await;
            assert!(text.contains("event: host.updated"), "{text}");
            let data = text
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap();
            let value: Value = serde_json::from_str(data).unwrap();
            assert_eq!(
                value,
                api_v2::host_view(&store, "lab", "/engine", &state).unwrap()
            );
            assert_eq!(value["status"], expected);
        }
        assert!(!run_events(&store, &mut cursor, None).unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn run_bus_events_emit_contact_and_cancel_without_a_timestamp_change() {
        use crate::engine::bus::RunEvent;
        use crate::engine::Phase;
        use crate::store::Contact;
        let directory = std::env::temp_dir().join(format!("orx-run-sse-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(directory.clone()).unwrap();
        let run = StoredRun {
            id: "r".into(),
            experiment_id: "e".into(),
            project_id: "p".into(),
            status: "running".into(),
            backend_json: r#"{"kind":"engine","jobId":"/r","handle":{"type":"Local","dir":"/r"}}"#
                .into(),
            command: "true".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        };
        store.upsert_run(&run).unwrap();
        store.accept_run_cancel("r", Some("user")).unwrap();
        let updated_at = store.get_run("r").unwrap().unwrap().updated_at;
        let mut cursor = EventCursor::default();
        run_events(&store, &mut cursor, None).unwrap();
        for contact in [
            Contact::Unreachable {
                since: 2,
                cause: "offline".into(),
            },
            Contact::NeedsAuth {
                since: 3,
                cause: "auth".into(),
            },
            Contact::Live,
        ] {
            store.set_run_contact("r", &contact).unwrap();
            assert_eq!(store.get_run("r").unwrap().unwrap().updated_at, updated_at);
            let event = RunEvent::Changed {
                run_id: "r".into(),
                phase: Phase::Running,
                contact: contact.clone(),
                cancelling: true,
            };
            let events = run_events(&store, &mut cursor, Some(event)).unwrap();
            let text = event_text(events).await;
            assert!(text.contains("event: run.updated"));
            let data = text
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap();
            let value: Value = serde_json::from_str(data).unwrap();
            assert_eq!(value["run"]["cancelling"], true);
            assert_eq!(
                value["run"]["contact"],
                if contact == Contact::Live {
                    Value::Null
                } else {
                    json!(contact)
                }
            );
        }
        store
            .set_engine_phase("r", "running", "done", Some(10), Some(0), None)
            .unwrap();
        store.queue_log_collection("r").unwrap();
        for attempt in 0..=8 {
            if attempt == 8 {
                store.finish_log_collection("r").unwrap();
            } else if attempt > 0 {
                store.defer_log_collection("r", "offline").unwrap();
            }
            let events = run_events(
                &store,
                &mut cursor,
                Some(RunEvent::LogCollection { run_id: "r".into() }),
            )
            .unwrap();
            let text = event_text(events).await;
            assert!(text.contains("event: run.updated"));
            let data = text
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .unwrap();
            let value: Value = serde_json::from_str(data).unwrap();
            if attempt == 8 {
                assert!(value["run"].get("logCollection").is_none());
                assert!(value["run"].get("logCollectionError").is_none());
            } else {
                assert_eq!(
                    value["run"]["logCollection"],
                    if attempt == 7 { "failed" } else { "pending" }
                );
                if attempt == 0 {
                    assert!(value["run"].get("logCollectionError").is_none());
                } else {
                    assert_eq!(value["run"]["logCollectionError"], "offline");
                }
            }
            assert_eq!(value["run"]["cancelling"], false);
        }
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn chat_active_runs_tracks_owners_and_watchers_until_terminal() {
        let dir = std::env::temp_dir().join(format!("orx-chat-runs-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let mut run = StoredRun {
            id: "run-1".into(),
            experiment_id: "experiment-1".into(),
            project_id: "project-1".into(),
            status: "running".into(),
            backend_json: "{}".into(),
            command: String::new(),
            created_at: 1,
            updated_at: 2,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: Some("owner".into()),
        };
        store.upsert_run(&run).unwrap();
        store.subscribe_run(&run.id, "owner", 1).unwrap();
        store.subscribe_run(&run.id, "watcher", 1).unwrap();
        let rows = chat_active_runs_json(&store).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
        assert_eq!(rows[0]["sessionId"], "owner");
        assert_eq!(rows[1]["sessionId"], "watcher");
        run.status = "done".into();
        store.upsert_run(&run).unwrap();
        assert_eq!(chat_active_runs_json(&store).unwrap(), json!([]));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_chat_uses_saved_preferences_and_normalizes_model_settings() {
        let preferences = crate::store::StoredUiState {
            onboarding_completed: true,
            tour_completed: true,
            workspace: None,
            preferred_autonomy: Some(Autonomy::Copilot),
            nested_delegation: false,
            opencode_free_models: true,
            favorite_models: Vec::new(),
            hidden_models: Vec::new(),
            hidden_agents: Vec::new(),
            model_section_order: Vec::new(),
            collapsed_model_sections: Vec::new(),
            model_sightings: None,
            preferred_agent: Some(StoredAgentSelection {
                profile: None,
                harness: "codex".into(),
                model: Some("custom-model".into()),
                reasoning_level: Some("ultra".into()),
                permission_mode: Some("full-access".into()),
                service_tier: Some("priority".into()),
            }),
        };
        let catalog = json!({"harnesses": [{
            "id": "codex", "agentReady": true,"defaultModel":"gpt-6-sol",
            "models": [{"id":"gpt-6-sol"}], "options": {"reasoningLevels": [{"id": "default"}, {"id": "high"}]}
        }]});
        let req = new_chat_settings("project".into(), preferences, &catalog).unwrap();
        assert_eq!(req.harness.as_deref(), Some("codex"));
        assert_eq!(req.model.as_deref(), Some("custom-model"));
        assert_eq!(req.reasoning_level.as_deref(), Some("default"));
        assert!(req.service_tier.is_none());
        assert_eq!(req.permission_mode.as_deref(), Some("full-access"));
        assert_eq!(req.autonomy, Some(Autonomy::Copilot));
        assert!(!req.plan_mode);
    }

    #[test]
    fn new_chat_falls_back_only_when_no_preference_exists() {
        let mut preferences = crate::store::StoredUiState {
            onboarding_completed: false,
            tour_completed: false,
            workspace: None,
            preferred_autonomy: None,
            preferred_agent: None,
            nested_delegation: false,
            opencode_free_models: true,
            favorite_models: Vec::new(),
            hidden_models: Vec::new(),
            hidden_agents: Vec::new(),
            model_section_order: Vec::new(),
            collapsed_model_sections: Vec::new(),
            model_sightings: None,
        };
        let catalog = json!({"harnesses": [{"id":"claude-code", "agentReady":true,"defaultModel":"opus",
            "models":[{"id":"opus"}], "options":{"defaultPermissionMode":"plan", "reasoningLevels":[{"id":"default"}]}}]});
        let req = new_chat_settings("project".into(), preferences.clone(), &catalog).unwrap();
        assert_eq!(req.harness.as_deref(), Some("claude-code"));
        assert_eq!(req.model.as_deref(), Some("opus"));
        assert_eq!(req.permission_mode.as_deref(), Some("plan"));
        assert_eq!(req.autonomy, Some(Autonomy::Agentic));
        preferences.preferred_agent = Some(StoredAgentSelection {
            profile: None,
            harness: "claude-code".into(),
            model: Some("custom-claude-model".into()),
            service_tier: None,
            permission_mode: Some("plan".into()),
            reasoning_level: Some("default".into()),
        });
        let req = new_chat_settings("project".into(), preferences.clone(), &catalog).unwrap();
        assert_eq!(req.harness.as_deref(), Some("claude-code"));
        assert_eq!(req.model.as_deref(), Some("custom-claude-model"));
        let unavailable = json!({"harnesses": [{"id":"claude-code", "agentReady":false}, {"id":"codex", "agentReady":true}]});
        assert!(new_chat_settings("project".into(), preferences.clone(), &unavailable).is_err());
        preferences.preferred_agent = Some(StoredAgentSelection {
            profile: None,
            harness: "codex".into(),
            model: Some("gpt-6-sol".into()),
            service_tier: None,
            permission_mode: None,
            reasoning_level: None,
        });
        assert!(new_chat_settings("project".into(), preferences, &catalog).is_err());
    }

    #[test]
    fn new_chat_matches_the_preferred_profile_without_falling_back() {
        let preferences = crate::store::StoredUiState {
            onboarding_completed: false,
            tour_completed: false,
            preferred_agent: Some(StoredAgentSelection {
                profile: Some("codex-012345abcd".into()),
                harness: "codex".into(),
                model: None,
                service_tier: None,
                permission_mode: None,
                reasoning_level: None,
            }),
            workspace: None,
            preferred_autonomy: None,
            nested_delegation: false,
            opencode_free_models: true,
            favorite_models: Vec::new(),
            hidden_models: Vec::new(),
            hidden_agents: Vec::new(),
            model_section_order: Vec::new(),
            collapsed_model_sections: Vec::new(),
            model_sightings: None,
        };
        let catalog = json!({"harnesses":[
            {"id":"codex", "agentReady":true, "models":[], "options":{}},
            {"id":"codex", "profile":{"id":"codex-012345abcd","name":"Work"}, "agentReady":true,"defaultModel":"gpt-6-sol","models":[{"id":"gpt-6-sol"}],"options":{}}
        ]});
        let selected = new_chat_settings("project".into(), preferences.clone(), &catalog).unwrap();
        assert_eq!(selected.profile.as_deref(), Some("codex-012345abcd"));
        let local_only =
            json!({"harnesses":[{"id":"codex","agentReady":true,"models":[],"options":{}}]});
        assert!(new_chat_settings("project".into(), preferences, &local_only).is_err());
    }

    #[test]
    fn missing_claude_keeps_install_guidance_when_auth_was_cached() {
        let host = local::claude::ClaudeHost::new();
        let ready = local::harness::claude::parse_auth_status(
            Some(0),
            br#"{"loggedIn":true,"authMethod":"oauth","apiProvider":"firstParty"}"#,
            local::harness::claude::auth_barrier_sequence(),
        );
        host.observe_auth_probe(&ready);
        let mut payload = json!({"harnesses": [{"id": "claude-code", "installed": false, "agentReady": false, "agentNote": "Install Claude Code"}]});
        let expected = payload.clone();
        overlay_claude_auth(&mut payload, &host.auth_snapshot());
        assert_eq!(payload, expected);
    }

    #[test]
    fn successful_recheck_clears_stale_claude_warning() {
        let host = local::claude::ClaudeHost::new();
        let ready = local::harness::claude::parse_auth_status(
            Some(0),
            br#"{"loggedIn":true,"authMethod":"third_party","apiProvider":"bedrock"}"#,
            local::harness::claude::auth_barrier_sequence(),
        );
        host.observe_auth_probe(&ready);
        let failed = local::harness::claude::parse_auth_status(
            Some(2),
            b"bad status",
            local::harness::claude::auth_barrier_sequence(),
        );
        host.observe_auth_probe(&failed);
        let mut payload =
            json!({"harnesses": [{"id": "claude-code", "authState": "ready", "agentReady": true}]});
        overlay_claude_auth(&mut payload, &host.auth_snapshot());
        assert_eq!(payload["harnesses"][0]["authCheckFailed"], true);
        assert!(payload["harnesses"][0]["agentNote"]
            .as_str()
            .unwrap()
            .contains("Could not re-check"));

        let recovered = local::harness::claude::parse_auth_status(
            Some(0),
            br#"{"loggedIn":true,"authMethod":"third_party","apiProvider":"bedrock"}"#,
            local::harness::claude::auth_barrier_sequence(),
        );
        host.observe_auth_probe(&recovered);
        overlay_claude_auth(&mut payload, &host.auth_snapshot());
        assert_eq!(payload["harnesses"][0]["authCheckFailed"], false);
        assert!(payload["harnesses"][0].get("agentNote").is_none());
    }

    #[test]
    fn send_model_distinguishes_cli_default_from_no_override() {
        let omitted: SendChatReq = serde_json::from_value(json!({ "text": "hello" })).unwrap();
        let default: SendChatReq =
            serde_json::from_value(json!({ "text": "hello", "model": null })).unwrap();
        assert_eq!(omitted.model, None);
        assert_eq!(default.model, Some(None));
    }

    #[test]
    fn harness_payload_predicates_read_the_wire_shape() {
        let provisional = json!({
            "harnesses": [
                {"id": "claude-code", "agentReady": false, "catalogPending": true},
                {"id": "codex", "agentReady": false},
            ]
        });
        assert!(payload_is_provisional(&provisional));
        assert!(claude_entry_pending(&provisional));
        assert!(!payload_has_ready_claude(&provisional));

        let filled = json!({
            "harnesses": [
                {"id": "claude-code", "agentReady": true},
                {"id": "codex", "agentReady": false},
            ]
        });
        assert!(!payload_is_provisional(&filled));
        assert!(!claude_entry_pending(&filled));
        assert!(payload_has_ready_claude(&filled));

        // `catalogPending` is skip-serialized when false — its absence must
        // read as settled, not pending.
        let no_claude = json!({ "harnesses": [{"id": "codex", "agentReady": false}] });
        assert!(!payload_is_provisional(&no_claude));
        assert!(!claude_entry_pending(&no_claude));
    }

    #[tokio::test]
    async fn closed_dashboard_releases_idle_chat_receiver() {
        let (sender, receiver) = tokio::sync::broadcast::channel(1);
        let (tx, rx) = mpsc::channel(1);
        drop(rx);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            forward_chat_events(receiver, tx),
        )
        .await
        .unwrap();
        assert_eq!(sender.receiver_count(), 0);
    }

    #[tokio::test]
    async fn v2_events_and_hello_bypass_update_drain_without_counting_active_requests() {
        use tower::ServiceExt;

        let _guard = ACTIVE_TEST_LOCK.lock().await;
        let app = Router::new()
            .route("/api/v2/hello", get(|| async { "ready" }))
            .route("/api/v2/events", get(|| async { "events" }))
            .layer(middleware::from_fn(track_active));
        DRAINING.store(true, Ordering::SeqCst);
        let result = tokio::time::timeout(Duration::from_secs(1), async {
            for path in ["hello", "events"] {
                let request = axum::http::Request::builder()
                    .uri(format!("/api/v2/{path}"))
                    .body(axum::body::Body::empty())
                    .unwrap();
                let response = app.clone().oneshot(request).await.unwrap();
                assert!(response.status().is_success());
                axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap();
            }
        })
        .await;
        DRAINING.store(false, Ordering::SeqCst);
        result.unwrap();
        assert_eq!(ACTIVE.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_streaming_response_stays_active_until_its_body_is_sent() {
        let _guard = ACTIVE_TEST_LOCK.lock().await;
        let gate = Arc::new(tokio::sync::Notify::new());
        let release = gate.clone();
        let app = Router::new()
            .route(
                "/file",
                get(move || async move {
                    axum::body::Body::from_stream(futures::stream::once(async move {
                        gate.notified().await;
                        Ok::<_, Infallible>("done")
                    }))
                }),
            )
            .layer(middleware::from_fn(track_active));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await });

        let response = crate::net::loopback_client()
            .build()
            .unwrap()
            .get(url)
            .send()
            .await
            .unwrap();
        assert_eq!(ACTIVE.load(Ordering::SeqCst), 1);
        release.notify_one();
        assert_eq!(response.text().await.unwrap(), "done");
        for _ in 0..100 {
            if ACTIVE.load(Ordering::SeqCst) == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("a sent response body stayed active");
    }

    #[tokio::test]
    async fn a_background_operation_holds_off_a_restart_until_it_finishes() {
        let _guard = ACTIVE_TEST_LOCK.lock().await;
        let operations = Operations::default();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        let started = run_operation(&operations, true, "uploading", |_| async move {
            finished.await.ok();
            Ok(json!({}))
        })
        .await
        .unwrap();
        assert_eq!(started.status(), StatusCode::ACCEPTED);
        assert_eq!(ACTIVE.load(Ordering::SeqCst), 1);

        finish.send(()).unwrap();
        for _ in 0..100 {
            if ACTIVE.load(Ordering::SeqCst) == 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("a finished operation stayed active");
    }

    #[tokio::test]
    async fn lagged_chat_stream_requests_resync_and_continues() {
        use axum::response::IntoResponse;
        let (sender, receiver) = tokio::sync::broadcast::channel(1);
        sender.send(("chat.busy", json!({"busy": true}))).unwrap();
        sender.send(("chat.busy", json!({"busy": false}))).unwrap();
        drop(sender);
        let (tx, mut rx) = mpsc::channel(4);
        forward_chat_events(receiver, tx).await;
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(Ok::<_, Infallible>(event));
        }
        let response = Sse::new(futures::stream::iter(events)).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = std::str::from_utf8(&bytes).unwrap();
        assert!(body.starts_with("event: resync.required\ndata: {}\n\n"));
        assert!(body.contains("event: chat.busy\ndata: {\"busy\":false}"));
    }

    #[tokio::test]
    async fn workspace_requests_validate_metadata_and_allow_independent_preferences() {
        let preferences: SetUiStateReq =
            serde_json::from_value(json!({"tourCompleted":true})).unwrap();
        assert!(preferences.workspace.is_none());
        let workspace: SetUiStateReq = serde_json::from_value(json!({"workspace":{
            "lastLocation":"/projects/project/tasks/new", "railOpen":true,
            "panelWidth":500, "experimentsView":"tree"
        }}))
        .unwrap();
        assert!(workspace.preferred_agent.is_none());
        workspace.workspace.unwrap().validate().unwrap();
        assert!(serde_json::from_value::<SetUiStateReq>(json!({"workspace":{
            "lastLocation":null, "railOpen":true, "panelWidth":500,
            "experimentsView":"grid"
        }}))
        .is_err());
        let invalid: GlobalWorkspaceState = serde_json::from_value(json!({
            "lastLocation":"/projects/project", "railOpen":true,
            "panelWidth":500, "experimentsView":"tree"
        }))
        .unwrap();
        assert!(invalid.validate().is_err());
        let result = set_ui_state(
            State(test_state()),
            Json(SetUiStateReq {
                tour_completed: Some(true),
                preferred_agent: None,
                workspace: Some(invalid),
                preferred_autonomy: None,
                nested_delegation: None,
                opencode_free_models: None,
                favorite_models: None,
                hidden_models: None,
                hidden_agents: None,
                model_section_order: None,
                collapsed_model_sections: None,
                model_sightings: None,
            }),
        )
        .await;
        assert_eq!(result.err().unwrap().0, StatusCode::BAD_REQUEST);
        for invalid in [
            json!({"hiddenModels": [" "]}),
            json!({"hiddenAgents": [" "]}),
            json!({"hiddenAgents": ["x".repeat(201)]}),
            json!({"hiddenAgents": vec!["codex"; 2001]}),
            json!({"modelSectionOrder": ["x".repeat(81)]}),
            json!({"modelSightings": ["gpt-6"]}),
        ] {
            let result = set_ui_state(
                State(test_state()),
                Json(serde_json::from_value(invalid).unwrap()),
            )
            .await;
            assert_eq!(result.err().unwrap().0, StatusCode::BAD_REQUEST);
        }
        let unsupported: WorkspaceState = serde_json::from_value(json!({
            "version":2, "lastLocation":null, "tasks":{}
        }))
        .unwrap();
        let result = set_project_ui_state(Path("project".into()), Json(unsupported)).await;
        assert_eq!(result.err().unwrap().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn ssh_workspace_blocks_local_machine_actions_but_keeps_config_editing() {
        for path in [
            "/api/project-path/pick",
            "/api/update",
            "/api/update/apply",
            "/api/update/restart",
            "/api/settings/data-dir",
            "/api/settings/data-dir/move",
            "/api/settings/openresearch/login",
            "/api/settings/openresearch/logout",
            "/api/settings/openresearch/ssh-key",
            "/api/settings/commands/run",
            "/api/remote/sessions",
            "/api/projects/p1/file/open",
            "/api/projects/p1/file/reveal",
            "/api/projects/p1/files/reveal",
        ] {
            assert!(remote_route_forbidden(path), "{path}");
        }
        assert!(!remote_route_forbidden("/api/settings/ssh/config"));
        assert!(!remote_route_forbidden("/api/settings/ssh/master"));
        assert!(!remote_route_forbidden("/api/settings/ssh/preflight"));
        assert!(!remote_route_forbidden("/api/settings/ssh/connect"));
        assert!(!remote_route_forbidden("/api/harnesses/setup"));
        assert!(!remote_route_forbidden("/api/settings/network"));
        assert!(!remote_route_forbidden("/api/projects/p1/file"));
    }

    #[test]
    fn remote_callback_token_is_limited_to_runs_and_the_spawn_preflight() {
        assert!(is_remote_callback_route(&Method::POST, "/api/runs"));
        assert!(is_remote_callback_route(
            &Method::POST,
            "/api/runs/run-1/cancel"
        ));
        assert!(!is_remote_callback_route(&Method::GET, "/api/runs"));
        assert!(!is_remote_callback_route(
            &Method::POST,
            "/api/runs/run-1/extra/cancel"
        ));
        assert!(!is_remote_callback_route(
            &Method::POST,
            "/api/chat/sessions/s1/message"
        ));
        assert!(is_remote_callback_route(
            &Method::GET,
            "/api/harnesses/codex/snapshot"
        ));
        assert!(!is_remote_callback_route(
            &Method::POST,
            "/api/harnesses/codex/snapshot"
        ));
        assert!(!is_remote_callback_route(&Method::GET, "/api/harnesses"));
        assert!(!is_remote_callback_route(
            &Method::GET,
            "/api/harnesses/setup/commands"
        ));
    }

    #[test]
    fn ssh_terminal_requires_the_dashboard_origin() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "127.0.0.1:4791".parse().unwrap());
        headers.insert(header::ORIGIN, "http://127.0.0.1:4791".parse().unwrap());
        assert!(same_origin(&headers));

        headers.insert(header::ORIGIN, "https://example.com".parse().unwrap());
        assert!(!same_origin(&headers));
        headers.remove(header::ORIGIN);
        assert!(!same_origin(&headers));
    }

    #[test]
    fn ssh_terminal_resize_message_deserializes() {
        let input: SshTerminalInput =
            serde_json::from_str(r#"{"type":"resize","cols":120,"rows":40}"#).unwrap();
        let SshTerminalInput::Resize { cols, rows } = input;
        assert_eq!((cols, rows), (120, 40));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_ssh_status_reports_missing_persistent_connection() {
        let response = ssh_master_status(Query(SshPreflightReq {
            host: "unused-host".into(),
            container: None,
        }))
        .await
        .unwrap_or_else(|error| panic!("{}", error.1));
        assert_eq!(response.0["running"], false);
    }

    #[test]
    fn ssh_connection_failure_is_a_persistable_preflight_result() {
        let test = ssh_connect_failure("cluster", "authentication failed".into());
        assert_eq!(test.host, "cluster");
        assert!(!test.reachable);
        assert!(!test.tools_found);
        assert_eq!(test.error.as_deref(), Some("authentication failed"));
        assert!(test.tested_at > 0);
    }

    /// ConPTY waits on its cursor query until answered, and closing it waits on that.
    #[tokio::test]
    async fn closing_a_pty_whose_client_left_returns_promptly() {
        let (program, args) = if cfg!(windows) {
            ("cmd", vec!["/c".into(), "echo ready & pause".into()])
        } else {
            ("sh", vec!["-c".into(), "echo ready; read _".into()])
        };
        let PtySession {
            master,
            input,
            mut events,
            kill,
        } = start_pty(program, args).unwrap();
        let first = tokio::time::timeout(Duration::from_secs(10), events.recv()).await;
        assert!(matches!(first, Ok(Some(PtyEvent::Output(_)))));

        // The order `relay_pty` tears down in when its socket closes.
        let _ = kill.send(());
        drop((events, input));
        let (dropped, done) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(master);
            let _ = dropped.send(());
        });
        done.recv_timeout(Duration::from_secs(10))
            .expect("closing the PTY blocked its caller");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn pty_relays_input_output_resize_exit_and_cancel() {
        let mut session = start_pty(
            "sh",
            vec![
                "-c".into(),
                "read value; printf 'reply:%s\\n' \"$value\"".into(),
            ],
        )
        .unwrap();
        session
            .master
            .resize(PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        session.input.send(b"hello\n".to_vec()).unwrap();

        let mut output = Vec::new();
        let mut exit = None;
        // Exit and the last output come from different threads; the channel closes after both.
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = session.events.recv().await {
                match event {
                    PtyEvent::Output(bytes) => output.extend(bytes),
                    PtyEvent::Exit(status) => exit = Some(status),
                    PtyEvent::Eof => {}
                }
            }
        })
        .await
        .expect("PTY did not exit");
        let status = exit
            .expect("PTY ended without an exit status")
            .unwrap_or_else(|error| panic!("PTY wait failed: {error}"));
        assert!(status.success());
        assert!(String::from_utf8_lossy(&output).contains("reply:hello"));

        let mut cancelled =
            start_pty("sh", vec!["-c".into(), "trap '' HUP; sleep 30".into()]).unwrap();
        cancelled.kill.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = cancelled.events.recv().await {
                if matches!(event, PtyEvent::Exit(_)) {
                    return;
                }
            }
            panic!("cancelled PTY ended without an exit event");
        })
        .await
        .expect("cancelled PTY did not exit");
    }

    #[test]
    fn client_resizes_update_the_tracked_size_only_when_valid() {
        let mut size = DEFAULT_PTY_SIZE;
        assert!(apply_resize(
            &mut size,
            r#"{"type":"resize","cols":111,"rows":33}"#
        ));
        assert_eq!((size.cols, size.rows), (111, 33));
        assert!(!apply_resize(
            &mut size,
            r#"{"type":"resize","cols":0,"rows":9}"#
        ));
        assert!(!apply_resize(&mut size, "not json"));
        assert_eq!((size.cols, size.rows), (111, 33));
    }

    #[test]
    fn bundled_zcode_login_has_both_provider_paths() {
        let root = std::env::temp_dir().join(format!("orx-zcode-login-{}", uuid::Uuid::new_v4()));
        let cli = root.join("Resources/glm/zcode.cjs");
        let provider = root.join("Resources/config/provider/zcode-builtin.json");
        std::fs::create_dir_all(cli.parent().unwrap()).unwrap();
        std::fs::create_dir_all(provider.parent().unwrap()).unwrap();
        std::fs::write(&provider, "{}").unwrap();
        let home = root.join("test-home");
        let env = zcode_provider_env(&cli, &home);
        assert_eq!(env.len(), 2);
        assert_eq!(
            std::fs::canonicalize(&env[0].1).unwrap(),
            std::fs::canonicalize(provider).unwrap()
        );
        assert_eq!(
            env[1],
            (
                "ZCODE_PERSONAL_PROVIDER_CONFIG_FILE",
                home.join("v2/provider_config.json").into_os_string()
            )
        );
        assert!(zcode_provider_env(&root.join("missing/zcode.cjs"), &home).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn settings_commands_are_an_exact_allowlist() {
        assert_eq!(
            settings_command("gh auth login"),
            Some(&["gh", "auth", "login"][..])
        );
        assert_eq!(settings_command("gh auth login; rm -rf ~"), None);
        assert_eq!(settings_command("gh"), None);
        assert_eq!(
            settings_command("zcode login zai"),
            Some(&["zcode", "login", "zai"][..])
        );
        assert_eq!(settings_command("zcode login zai; echo token"), None);
    }

    #[tokio::test]
    async fn project_path_status_reports_an_unborn_repository_as_importable() {
        let path =
            std::env::temp_dir().join(format!("orx-project-path-status-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let status = crate::process::command("git")
            .current_dir(&path)
            .args(["init", "-q", "-b", "main"])
            .status()
            .unwrap();
        assert!(status.success());

        let response = project_path_status(Query(ProjectPathStatusQ {
            path: Some(path.to_string_lossy().into_owned()),
        }))
        .await;
        let Json(body) = match response {
            Ok(body) => body,
            Err(error) => panic!("unexpected path status error: {}", error.1),
        };

        assert_eq!(body["gitState"], "unborn");
        assert_eq!(body["initialized"], true);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn plan_never_becomes_the_preferred_mode_for_new_sessions() {
        assert_eq!(
            preferred_permission_mode("claude-code", Some("plan".into())).as_deref(),
            Some("auto")
        );
        assert_eq!(
            preferred_permission_mode("codex", Some("plan".into())).as_deref(),
            Some("approve-for-me")
        );
        assert_eq!(
            preferred_permission_mode("opencode", Some("plan".into())).as_deref(),
            Some("default")
        );
    }

    fn expect_profile(
        result: std::result::Result<crate::telemetry::ResearchProfile, ApiError>,
    ) -> crate::telemetry::ResearchProfile {
        match result {
            Ok(profile) => profile,
            Err(error) => panic!("unexpected profile error: {}", error.1),
        }
    }

    #[test]
    fn research_profile_requires_an_other_description() {
        let missing_other =
            normalize_research_profile(vec!["Other".into()], Some("   ".into()), None, vec![])
                .unwrap_err();
        assert_eq!(
            missing_other.1,
            "describe your research area when choosing Other"
        );
    }

    #[test]
    fn research_profile_preserves_disclosed_free_text() {
        let profile = expect_profile(normalize_research_profile(
            vec!["AI/ML".into(), "Other".into()],
            Some("  AI for theorem proving  ".into()),
            Some("  I study RL.\nSecond line.  ".into()),
            vec![crate::telemetry::ProfilePaper {
                paper_id: "1706.03762".into(),
                title: Some("Attention Is All You Need".into()),
                unknown: Default::default(),
            }],
        ));

        assert_eq!(
            profile.other_area.as_deref(),
            Some("  AI for theorem proving  ")
        );
        assert_eq!(
            profile.background.as_deref(),
            Some("  I study RL.\nSecond line.  ")
        );
        assert_eq!(
            profile.papers[0].title.as_deref(),
            Some("Attention Is All You Need")
        );
    }

    #[test]
    fn legacy_profile_update_preserves_saved_research_areas() {
        let current = crate::telemetry::ResearchProfile {
            research_areas: vec!["Physics".into(), "Other".into()],
            other_area: Some("Quantum information".into()),
            ..crate::telemetry::ResearchProfile::default()
        };
        let updated = expect_profile(profile_settings_update(
            SetProfileReq {
                research_areas: vec![],
                other_area: None,
                background: Some("Updated background".into()),
                papers: vec![],
            },
            current,
        ));

        assert_eq!(updated.research_areas, vec!["Physics", "Other"]);
        assert_eq!(updated.other_area.as_deref(), Some("Quantum information"));
        assert_eq!(updated.background.as_deref(), Some("Updated background"));
    }

    #[test]
    fn launches_hold_their_project_against_moves_and_deletion() {
        let lifecycle = Arc::new(ProjectLifecycle::default());
        let moving = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let admit = launch_admission(lifecycle.clone(), moving.clone(), stopping.clone());
        let admission = admit("p").unwrap();
        assert_eq!(lifecycle.operation_count(), 1);
        assert!(lifecycle.begin_delete("p").is_none());
        drop(admission);
        assert_eq!(lifecycle.operation_count(), 0);
        moving.store(true, Ordering::SeqCst);
        assert!(admit("p").err().unwrap().to_string().contains("move"));
        assert_eq!(lifecycle.operation_count(), 0);
        moving.store(false, Ordering::SeqCst);
        let deleting = lifecycle.begin_delete("p").unwrap();
        assert!(admit("p").is_err());
        drop(deleting);
        stopping.store(true, Ordering::SeqCst);
        assert!(admit("p").is_err());
    }

    #[test]
    fn create_run_request_preserves_forwarded_slurm_resources_and_attribution() {
        let request: CreateRunReq = serde_json::from_value(json!({
            "experimentId": "experiment-1",
            "backend": "slurm",
            "cpus": 8,
            "mem": "64G",
            "partition": "gpu",
            "qos": "acc_ehpc",
            "nodelist": "node[01-02]",
            "constraint": "a100",
            "exclude": "node07",
            "sbatchArgs": ["--rsc g=1"],
            "chatSessionId": "session-1",
            "telemetrySuppressed": true,
            "force": true,
        }))
        .unwrap();
        let forwarded = request.into_run_args();
        assert_eq!(forwarded.exp_id, "experiment-1");
        assert_eq!(forwarded.backend.as_deref(), Some("slurm"));
        assert_eq!(forwarded.cpus, Some(8));
        assert_eq!(forwarded.mem.as_deref(), Some("64G"));
        assert_eq!(forwarded.partition.as_deref(), Some("gpu"));
        assert_eq!(forwarded.qos.as_deref(), Some("acc_ehpc"));
        assert_eq!(forwarded.nodelist.as_deref(), Some("node[01-02]"));
        assert_eq!(forwarded.constraint.as_deref(), Some("a100"));
        assert_eq!(forwarded.exclude.as_deref(), Some("node07"));
        assert_eq!(forwarded.sbatch_args, ["--rsc g=1"]);
        assert_eq!(forwarded.chat_session_id.as_deref(), Some("session-1"));
        assert!(forwarded.forwarded);
        assert!(forwarded.telemetry_suppressed);
        assert!(forwarded.force);
    }

    #[test]
    fn api_run_exposes_cancel_intent_contact_and_the_launched_backend() {
        let mut backend =
            crate::jobs::BackendDescriptor::parse(r#"{"kind":"openresearch_job"}"#).unwrap();
        backend.rental_lease = Some(crate::jobs::rental::Rental {
            api_url: "https://example.invalid".into(),
            sandbox_id: "synthetic-only".into(),
            token: "private-rental-token".into(),
            unknown: Default::default(),
        });
        let run = StoredRun {
            id: "run-1".into(),
            experiment_id: "experiment-1".into(),
            project_id: "project-1".into(),
            status: "running".into(),
            backend_json: backend.to_json(),
            command: String::new(),
            created_at: 1,
            updated_at: 2,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: true,
            cancel_reason: None,
            chat_session_id: None,
        };

        let dir = std::env::temp_dir().join(format!("orx-api-run-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let value = serde_json::to_value(ApiRun::new(&store, &run).unwrap()).unwrap();
        assert!(value.get("cancelRequested").is_none());
        assert_eq!(value["backend"]["kind"], "openresearch_job");
        assert!(value["contact"].is_null());
        assert!(!value.to_string().contains("private-rental-token"));
        let legacy = serde_json::to_value(&run).unwrap();
        assert!(!legacy.to_string().contains("private-rental-token"));
        assert!(run.backend_json.contains("private-rental-token"));

        // An engine run shows what it launched, its reachability, and an accepted cancel.
        let metadata = crate::jobs::BackendDescriptor::parse(r#"{"kind":"engine"}"#).unwrap();
        let launch = crate::store::LaunchSpec {
            kind: "ssh_job".into(),
            target: Some("lab".into()),
            options: json!({"container": {"reference": "dev", "id": "c1", "startedAt": "t", "runDir": "/root/.orx/runs/e"}}),
        };
        store
            .insert_submitting_run(crate::store::SubmittingRun {
                id: "e",
                experiment_id: "experiment-1",
                project_id: "project-1",
                metadata: &metadata,
                command: "train",
                created_at: 1,
                commit_sha: None,
                chat_session_id: None,
                spec_digest: "digest",
                launch: &launch,
            })
            .unwrap();
        store
            .set_engine_phase("e", "submitting", "running", None, None, Some(1234))
            .unwrap();
        store.accept_run_cancel("e", Some("user")).unwrap().unwrap();
        let outage = crate::store::Contact::Unreachable {
            since: 3,
            cause: "eof".into(),
        };
        store.set_run_contact("e", &outage).unwrap();
        let engine = store.get_run("e").unwrap().unwrap();
        let value = serde_json::to_value(ApiRun::new(&store, &engine).unwrap()).unwrap();
        assert!(value.get("cancelRequested").is_none());
        assert_eq!(value["contact"]["type"], "unreachable");
        assert_eq!(value["backend"]["kind"], "ssh_job");
        assert_eq!(value["backend"]["namespace"], "lab");
        assert_eq!(value["backend"]["sshContainer"]["reference"], "dev");
        assert_eq!(value["cancelling"], true);
        assert_eq!(value["startedAt"], 1234);
        assert!(value["lostReason"].is_null());
        assert_eq!(value["host"], "lab");
        assert!(value["noticeError"].is_null());
        store
            .insert_run_notice("e", "chat", crate::store::TERMINAL, "terminal", "done", 1)
            .unwrap();
        let batch = store.run_notice_batch_at("chat", 2).unwrap();
        store
            .record_run_notice_failure(
                "chat",
                &batch[0].client_turn_id,
                "Chat harness is no longer configured",
                true,
                3,
            )
            .unwrap();
        let value = serde_json::to_value(ApiRun::new(&store, &engine).unwrap()).unwrap();
        assert_eq!(value["noticeError"], "Chat harness is no longer configured");
        let mut engine = engine;
        engine.backend_json = crate::engine::handle::backend_json(
            "e",
            Some(&crate::engine::handle::Handle::Ssh {
                host: "lab".into(),
                root: "/engine".into(),
            }),
            &metadata,
        )
        .unwrap();
        for status in [
            "submitting",
            "starting",
            "running",
            "done",
            "failed",
            "cancelled",
            "lost",
        ] {
            engine.status = status.into();
            engine.result_markdown = (status == "lost").then(|| "abandoned".into());
            for contact in [
                crate::store::Contact::Live,
                outage.clone(),
                crate::store::Contact::NeedsAuth {
                    since: 4,
                    cause: "auth".into(),
                },
                crate::store::Contact::Unreconciled {
                    since: 5,
                    detail: "unknown".into(),
                },
            ] {
                store.set_run_contact("e", &contact).unwrap();
                let value = serde_json::to_value(ApiRun::new(&store, &engine).unwrap()).unwrap();
                assert_eq!(value["status"], status);
                assert_eq!(value["host"], "lab");
                assert_eq!(value["cancelling"], !is_terminal(status));
                assert_eq!(
                    value["lostReason"],
                    if status == "lost" {
                        json!("abandoned")
                    } else {
                        Value::Null
                    }
                );
                assert_eq!(
                    value["contact"],
                    if contact == crate::store::Contact::Live {
                        Value::Null
                    } else {
                        json!(contact)
                    }
                );
            }
        }
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn compute_settings_registers_tinker_without_exposing_its_key() {
        let settings = compute_settings_json(SshReadiness::NoUsableKey);
        let tinker = settings["targets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|target| target["id"] == "tinker")
            .unwrap();
        assert!(tinker["configured"].is_boolean());
        assert!(tinker.get("key").is_none());
        assert!(tinker.get("apiKey").is_none());
        let ids: Vec<_> = settings["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|target| target["id"].as_str().unwrap())
            .collect();
        let vercel = ids.iter().position(|id| *id == "vercel").unwrap();
        assert_eq!(ids[vercel + 1], "openresearch");
        assert!(ids.iter().position(|id| *id == "ray").unwrap() < vercel);
    }

    #[tokio::test]
    async fn a_caller_launches_only_with_the_settings_orx_up_would_use() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let root = crate::config::test_home();
        let own = absolute_config_dir();
        let caller = root.join("agent/openresearch");
        std::fs::create_dir_all(&caller).unwrap();
        let mut namespace = crate::jobs::kubernetes::K8sSettings::default();
        namespace.namespace = "lab".into();
        crate::jobs::kubernetes::save_settings(&namespace).unwrap();
        let k8s = |dir: Option<&std::path::Path>, sent: Option<&[(&str, Option<&str>)]>| {
            let mut args = crate::compute::tests::tinker_args();
            args.backend = Some("k8s".into());
            args.caller_config_dir = dir.map(std::path::Path::to_path_buf);
            args.caller_compute_settings = sent.map(|sent| {
                sent.iter()
                    .map(|(name, value)| (name.to_string(), value.map(str::to_string)))
                    .collect()
            });
            args
        };
        let lab: &[_] = &[("context", None), ("namespace", Some("lab"))];
        let elsewhere: &[_] = &[("context", None), ("namespace", Some("default"))];

        let own = own.as_path();
        let ok = |args: crate::ExpRunArgs| async move { require_caller_settings(&args, own).await };
        assert!(ok(k8s(None, None)).await.is_ok());
        assert!(ok(k8s(Some(own), None)).await.is_ok());
        assert!(ok(k8s(Some(&own.join("../openresearch")), None))
            .await
            .is_ok());
        assert!(ok(k8s(Some(&caller), Some(&[]))).await.is_ok());
        assert!(ok(k8s(Some(&caller), Some(lab))).await.is_ok());
        assert!(ok(k8s(Some(&caller), Some(&[("newerSetting", Some("x"))])))
            .await
            .is_ok());
        let mut ssh = k8s(Some(&caller), None);
        ssh.backend = Some("ssh".into());
        assert!(ok(ssh).await.is_ok());

        for args in [
            k8s(Some(&caller), Some(elsewhere)),
            k8s(Some(&caller), None),
        ] {
            let error = ok(args).await.unwrap_err().to_string();
            assert!(error.contains(&caller.display().to_string()), "{error}");
            assert!(error.contains(&own.display().to_string()), "{error}");
        }
        let error = ok(k8s(Some(&caller), Some(elsewhere)))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("namespace"), "{error}");
        assert!(!error.contains("context"), "{error}");
    }

    #[test]
    fn project_json_exposes_artifacts_dir_with_legacy_alias() {
        let project = local::model::LocalProject {
            id: "p1".into(),
            name: "Demo".into(),
            slug: "demo".into(),
            github_owner: "o".into(),
            github_repo: "r".into(),
            github_sync_enabled: true,
            baseline_branch: "main".into(),
            repo_path: "/tmp/r".into(),
            run_command: None,
            paper_id: None,
            created_at: 0,
            updated_at: 0,
        };
        let json = project_json_with_artifacts_dir(
            &project,
            "/tmp/openresearch-test/files/demo".to_string(),
        );
        assert_eq!(json["artifactsDir"], json["filesDir"]);
        assert!(json["artifactsDir"]
            .as_str()
            .unwrap()
            .ends_with("files/demo"));
    }

    #[test]
    fn project_deletion_preserves_artifacts_unless_explicitly_requested() {
        for (uri, delete_artifacts) in [
            ("/api/projects/p1", false),
            ("/api/projects/p1?deleteArtifacts=true", true),
        ] {
            let Query(query) =
                Query::<DeleteProjectQuery>::try_from_uri(&uri.parse().unwrap()).unwrap();
            assert_eq!(query.delete_artifacts, delete_artifacts);
        }
    }

    #[test]
    fn project_deletion_excludes_artifact_requests_and_project_creation() {
        let lifecycle = ProjectLifecycle::default();
        let artifact_request = lifecycle.admit("p1").unwrap();
        assert!(lifecycle.begin_delete("p1").is_none());
        drop(artifact_request);

        let deletion = lifecycle.begin_delete("p1").unwrap();
        assert!(lifecycle.admit("p1").is_none());
        assert!(lifecycle.admit("__project_create__").is_none());
        drop(deletion);

        let creation = lifecycle.admit("__project_create__").unwrap();
        assert!(lifecycle.begin_delete("p1").is_none());
        drop(creation);
        assert!(lifecycle.begin_delete("p1").is_some());
    }

    #[tokio::test]
    async fn opencode_v1_provider_routes_report_upgrade() {
        let response = opencode_v2(local::opencode::Protocol::V1)
            .err()
            .unwrap()
            .into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["code"], "opencode1");
        assert!(body["error"].as_str().unwrap().contains("Update OpenCode"));
        assert!(opencode_v2(local::opencode::Protocol::V2).is_ok());
    }

    #[tokio::test]
    async fn daily_update_pass_skips_agents_signed_out_on_every_account() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let state = test_state();
        let work = local::agent_profiles::create("codex", "Work", None).unwrap();
        let preset = |executable: &str| {
            local::acp::presets()
                .into_iter()
                .find(|preset| preset.launch.executable == executable)
                .unwrap()
                .launch
                .id
        };
        *state.harnesses.lock().await = Some((
            std::time::Instant::now(),
            json!({"harnesses":[
                {"id":"opencode","authState":"needsLogin"},
                {"id":preset("dsh"),"authState":"needsLogin"},
                {"id":preset("pi-acp"),"authState":"needsLogin"},
                {"id":"codex","authState":"needsLogin"},
                {"id":"codex","profile":{"id":work},"authState":"ready"},
                {"id":"cursor","authState":"ready"},
                {"id":"antigravity","authState":"unknown"},
            ]}),
        ));
        let checked = std::sync::Mutex::new(Vec::new());

        update_agents(&state, |harness| {
            checked.lock().unwrap().push(harness);
            async { Ok(false) }
        })
        .await;

        let expected: Vec<_> = local::harness::managed::harnesses()
            .filter(|harness| !["opencode", "dsh", "pi-acp", "pi"].contains(harness))
            .collect();
        assert_eq!(checked.into_inner().unwrap(), expected);
    }

    #[tokio::test]
    async fn daily_update_pass_rechecks_an_expired_sign_in() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let empty = std::env::temp_dir().join("bin");
        std::fs::create_dir(&empty).unwrap();
        std::env::set_var("PATH", &empty);
        let state = test_state();
        let expired = std::time::Instant::now()
            .checked_sub(HARNESS_CACHE_TTL)
            .unwrap();
        *state.harnesses.lock().await = Some((
            expired,
            json!({"harnesses":[{"id":"opencode","authState":"needsLogin"}]}),
        ));
        let checked = std::sync::Mutex::new(Vec::new());

        update_agents(&state, |harness| {
            checked.lock().unwrap().push(harness);
            async { Ok(false) }
        })
        .await;

        assert!(checked.into_inner().unwrap().contains(&"opencode"));
    }

    #[tokio::test]
    async fn discarded_detection_cannot_change_committed_model_selection() {
        let state = test_state();
        *state.harnesses.lock().await = Some((
            std::time::Instant::now(),
            json!({"harnesses":[{
                "id":"codex", "name":"Codex", "agentReady":true, "authState":"ready", "defaultModel":"custom-provider-model", "models":[{"id":"custom-provider-model"}]
            }]}),
        ));
        for mut discarded in [
            json!({"id":"codex","catalogPending":true,"models":[]}),
            json!({"id":"codex","authState":"needsLogin","models":[]}),
        ] {
            local::harness::resolve_entry(&mut discarded, None).unwrap();
        }
        for model in [None, Some("explicit-model")] {
            assert_eq!(
                selected_chat_model(&state, "codex", None, model)
                    .await
                    .unwrap()
                    .as_deref(),
                Some(model.unwrap_or("custom-provider-model"))
            );
        }
    }

    fn test_state() -> AppState {
        let agent = Arc::new(AgentHost::new(None));
        let codex = Arc::new(local::codex::CodexHost::new());
        let claude = Arc::new(local::claude::ClaudeHost::new());
        let chat = Arc::new(ChatHost::new(agent.clone(), codex, claude.clone()));
        AppState {
            agent,
            chat: chat.clone(),
            claude,
            harnesses: chat.harnesses.clone(),
            project_lifecycle: Default::default(),
            project_creation_lock: Default::default(),
            operations: Default::default(),
            publication_locks: Default::default(),
            harness_fill_in_flight: Default::default(),
            claude_catalog_queue: Default::default(),
            data_dir_move_in_progress: Default::default(),
            data_dir_gate: Default::default(),
            remote_sessions: crate::commands::up_remote::RemoteSessionManager::new(),
            remote_instance_id: None,
            stopping: Default::default(),
            dashboard_lock: Default::default(),
            restart: Default::default(),
            reopen: Default::default(),
            trackio: Default::default(),
            opencode_signins: Arc::default(),
        }
    }

    #[tokio::test]
    async fn side_chat_reports_selection_as_400_and_store_faults_as_500() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let state = test_state();
        let _store = Store::open().unwrap();
        let connection =
            rusqlite::Connection::open(crate::store::data_dir().join("orx.db")).unwrap();
        connection.execute_batch(
            "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, repo_path, created_at, updated_at)
             VALUES ('side-project', 'Side project', 'side-project', '', '', '/unused', 1, 1);
             INSERT INTO chat_sessions (id, project_id, harness, created_at, updated_at)
             VALUES ('side-parent', 'side-project', 'codex', 1, 1);",
        ).unwrap();
        *state.harnesses.lock().await = Some((
            std::time::Instant::now(),
            json!({"harnesses":[{
                "id":"codex","agentReady":false,"authState":"needsLogin"
            }]}),
        ));
        let response = open_side_chat(State(state.clone()), Path("side-parent".into()))
            .await
            .err()
            .unwrap()
            .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(state.harnesses.try_lock().is_ok());
        *state.harnesses.lock().await = Some((
            std::time::Instant::now(),
            json!({"harnesses":[{
                "id":"codex","agentReady":true,"defaultModel":"concrete-model"
            }]}),
        ));
        let Json(response) = open_side_chat(State(state.clone()), Path("side-parent".into()))
            .await
            .unwrap_or_else(|error| panic!("{}", error.1));
        assert_eq!(response["session"]["model"], "concrete-model");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_side_chat BEFORE INSERT ON chat_sessions
                BEGIN SELECT RAISE(FAIL, 'side chat insert failed'); END;",
            )
            .unwrap();
        let response = open_side_chat(State(state.clone()), Path("side-parent".into()))
            .await
            .err()
            .unwrap()
            .into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(state.harnesses.try_lock().is_ok());
    }

    async fn serve(app: Router) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
        });
        address
    }

    /// A dashboard whose openresearch.sh is a fake that serves one project's `.orx` file under
    /// `slug` and accepts publishes; returns the project's id and a scratch folder.
    async fn dashboard_with_fake_openresearch(
        slug: &'static str,
    ) -> (std::net::SocketAddr, std::path::PathBuf, String) {
        use axum::routing::put;
        let root = std::env::temp_dir().join(format!("orx-open-link-{}", uuid::Uuid::new_v4()));
        let repo = root.join("sharer");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.com",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "start",
            ],
        ] {
            let status = crate::process::command("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success());
        }
        let store = Store::open().unwrap();
        let project = local::projects::create_project(
            &store,
            "Tiny LM",
            &repo.to_string_lossy(),
            Default::default(),
        )
        .unwrap();
        let file = root.join("tiny-lm.orx");
        let options = local::share::ShareOptions {
            chats: local::share::ChatSelection::None,
            logs: false,
            artifacts: false,
        };
        local::share::export(&store, &project, &options, &file).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service = listener.local_addr().unwrap();
        let link = json!({
            "id": "link-1", "slug": slug, "url": format!("https://openresearch.sh/p/{slug}"),
            "sourceProjectId": project.id, "name": "Tiny LM", "byteSize": bytes.len(),
            "publishedAt": "2026-11-11T00:00:00.000Z",
        });
        let fake = Router::new()
            .route(
                "/shared-projects/by-slug/{slug}/file",
                get(move |Path(requested): Path<String>| {
                    let bytes = bytes.clone();
                    async move {
                        if requested == slug {
                            bytes.into_response()
                        } else {
                            StatusCode::NOT_FOUND.into_response()
                        }
                    }
                }),
            )
            .route(
                "/shared-projects",
                post(move || async move {
                    Json(json!({
                        "sharedProjectId": "link-1",
                        "uploadId": "upload-1",
                        "upload": { "url": format!("http://{service}/blob"), "headers": {} },
                    }))
                }),
            )
            // Reads the upload first; answering before it resets the connection on Windows.
            .route(
                "/blob",
                put(|_: axum::body::Bytes| async { StatusCode::OK }),
            )
            .route(
                "/shared-projects/{id}/publish",
                post(move || {
                    let link = link.clone();
                    async move { Json(json!({ "sharedProject": link })) }
                }),
            );
        tokio::spawn(async move { axum::serve(listener, fake).await });
        std::env::set_var("OPENRESEARCH_API_URL", format!("http://{service}"));
        let dashboard = serve(router(test_state(), None, up_remote::Reach::Loopback)).await;
        (dashboard, root, project.id)
    }

    async fn post_to(
        address: std::net::SocketAddr,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let request = crate::net::loopback_client()
            .build()
            .unwrap()
            .post(format!("http://{address}{path}"))
            .header(header::ORIGIN, format!("http://{address}"));
        match body {
            Some(body) => request.json(&body),
            None => request,
        }
        .send()
        .await
        .unwrap()
    }

    async fn operation_id(started: reqwest::Response) -> String {
        assert_eq!(started.status(), StatusCode::ACCEPTED);
        let started: Value = started.json().await.unwrap();
        started["operationId"].as_str().unwrap().to_owned()
    }

    async fn get_operation_status(address: std::net::SocketAddr, id: &str) -> reqwest::Response {
        crate::net::loopback_client()
            .build()
            .unwrap()
            .get(format!("http://{address}/api/operations/{id}"))
            .send()
            .await
            .unwrap()
    }

    async fn finish_operation(address: std::net::SocketAddr, id: &str) -> Value {
        loop {
            let status: Value = get_operation_status(address, id)
                .await
                .json()
                .await
                .unwrap();
            if !status["result"].is_null() || !status["error"].is_null() {
                return status;
            }
            tokio::time::sleep(crate::config::paced(Duration::from_millis(50))).await;
        }
    }

    #[tokio::test]
    async fn a_published_link_opens_in_the_background() {
        if !crate::config::in_test_child(&["OPENRESEARCH_API_URL"]) {
            return;
        }
        let (address, root, _) = dashboard_with_fake_openresearch("abcdefghijkl").await;
        let target = root.join("opened");
        let path = format!(
            "/api/projects/import?path={}&shared=abcdefghijkl&background=true",
            target.display()
        );
        let id = operation_id(post_to(address, &path, None).await).await;

        let status = finish_operation(address, &id).await;
        assert_eq!(status["result"]["project"]["name"], "Tiny LM", "{status}");
        assert!(target.join(".git").exists());
        let again: Value = get_operation_status(address, &id)
            .await
            .json()
            .await
            .unwrap();
        assert_eq!(again, status, "a lost poll can read the result again");
    }

    #[tokio::test]
    async fn a_link_that_no_longer_downloads_says_why() {
        if !crate::config::in_test_child(&["OPENRESEARCH_API_URL"]) {
            return;
        }
        let (address, root, _) = dashboard_with_fake_openresearch("abcdefghijkl").await;
        let path = format!(
            "/api/projects/import?path={}&shared=unpublished0&background=true",
            root.join("opened").display()
        );
        let id = operation_id(post_to(address, &path, None).await).await;

        let status = finish_operation(address, &id).await;
        assert!(
            status["error"].as_str().unwrap().contains("unpublished"),
            "{status}"
        );
        assert!(!root.join("opened").exists());
    }

    #[tokio::test]
    async fn an_older_dashboard_still_waits_for_the_opened_project() {
        if !crate::config::in_test_child(&["OPENRESEARCH_API_URL"]) {
            return;
        }
        let (address, root, _) = dashboard_with_fake_openresearch("abcdefghijkl").await;
        let path = format!(
            "/api/projects/import?path={}&shared=abcdefghijkl",
            root.join("opened").display()
        );
        let opened = post_to(address, &path, None).await;
        assert_eq!(opened.status(), StatusCode::OK);
        let opened: Value = opened.json().await.unwrap();
        assert_eq!(opened["project"]["name"], "Tiny LM", "{opened}");
        assert!(opened["missingBranches"].is_array());
    }

    #[tokio::test]
    async fn a_project_publishes_in_the_background() {
        if !crate::config::in_test_child(&["OPENRESEARCH_API_URL"]) {
            return;
        }
        let (address, _, project) = dashboard_with_fake_openresearch("a1B2c3D4e5F6").await;
        let choice = json!({ "chats": [], "logs": false, "artifacts": false });
        let path = format!("/api/projects/{project}/publish?background=true");
        let signed_out = post_to(address, &path, Some(choice.clone())).await;
        assert_eq!(signed_out.status(), StatusCode::UNAUTHORIZED);

        crate::config::save_credentials(&crate::config::Credentials {
            api_url: std::env::var("OPENRESEARCH_API_URL").unwrap(),
            token: "pat".into(),
        })
        .await
        .unwrap();
        let id = operation_id(post_to(address, &path, Some(choice.clone())).await).await;
        let status = finish_operation(address, &id).await;
        assert_eq!(
            status["result"]["sharedProject"]["slug"], "a1B2c3D4e5F6",
            "{status}"
        );

        let waited = post_to(
            address,
            &format!("/api/projects/{project}/publish"),
            Some(choice),
        )
        .await;
        assert_eq!(waited.status(), StatusCode::OK);
        let waited: Value = waited.json().await.unwrap();
        assert_eq!(waited["sharedProject"]["slug"], "a1B2c3D4e5F6", "{waited}");
    }

    /// The status of a request written byte for byte, so the test owns every header.
    async fn status(address: std::net::SocketAddr, request: &str, headers: &[&str]) -> u16 {
        use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};

        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let head: String = headers
            .iter()
            .map(|header| format!("{header}\r\n"))
            .collect();
        stream
            .write_all(format!("{request} HTTP/1.1\r\n{head}Content-Length: 0\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        tokio::io::BufReader::new(stream)
            .read_line(&mut line)
            .await
            .unwrap();
        line.split(' ').nth(1).unwrap().parse().unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_browser_on_another_machine_connects_alphaxiv_with_a_code() {
        use futures::StreamExt as _;
        use std::sync::atomic::AtomicUsize;
        use std::sync::Mutex;
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest as _, Message as Frame};

        if !crate::config::in_test_child(&[
            "ALPHAXIV_API_URL",
            "ALPHAXIV_WEB_URL",
            "SSH_CONNECTION",
            "SSH_TTY",
            "SSH_CLIENT",
        ]) {
            return;
        }
        std::env::set_var("ORX_TEST_SPEEDUP", "100");

        // alphaXiv approves the second code after five polls; OpenResearch links it.
        let issued = Arc::new(AtomicUsize::new(0));
        let polls = Arc::new(Mutex::new(HashMap::<String, usize>::new()));
        let fake = {
            let (issued, polls) = (issued.clone(), polls.clone());
            Router::new()
                .route(
                    "/auth/device/code",
                    axum::routing::post(move |headers: HeaderMap| async move {
                        let n = issued.fetch_add(1, Ordering::SeqCst) + 1;
                        let site = format!("http://{}", headers["host"].to_str().unwrap());
                        Json(json!({
                            "device_code": format!("device-{n}"),
                            "user_code": format!("WXYZ-000{n}"),
                            "verification_uri": format!("{site}/device"),
                            "verification_uri_complete": format!("{site}/device?user_code=WXYZ-000{n}"),
                            "expires_in": 600,
                            "interval": 1,
                        }))
                    }),
                )
                .route(
                    "/auth/oauth2/token",
                    axum::routing::post(
                        move |axum::Form(form): axum::Form<HashMap<String, String>>| async move {
                            let code = form["device_code"].clone();
                            let count = {
                                let mut polls = polls.lock().unwrap();
                                let count = polls.entry(code.clone()).or_default();
                                *count += 1;
                                *count
                            };
                            if code != "device-2" || count <= 5 {
                                let pending = Json(json!({ "error": "authorization_pending" }));
                                return (StatusCode::BAD_REQUEST, pending).into_response();
                            }
                            Json(json!({
                                "access_token": "access-1",
                                "refresh_token": "refresh-1",
                                "expires_in": 3600,
                                "id_token": "e30.e30.sig",
                            }))
                            .into_response()
                        },
                    ),
                )
                .route(
                    "/auth/alphaxiv/link",
                    axum::routing::post(|| async { StatusCode::NO_CONTENT }),
                )
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, fake).await });
        std::env::set_var("ALPHAXIV_API_URL", &api);
        std::env::set_var("ALPHAXIV_WEB_URL", &api);
        crate::config::save_credentials(&crate::config::Credentials {
            api_url: api.clone(),
            token: "pat-1".into(),
        })
        .await
        .unwrap();

        let address = serve(router(test_state(), None, up_remote::Reach::Lan)).await;
        let lan = format!("192.168.1.5:{}", address.port());
        let connect = || async {
            let mut request = format!("ws://{address}/api/settings/alphaxiv/login")
                .into_client_request()
                .unwrap();
            let headers = request.headers_mut();
            headers.insert("host", lan.parse().unwrap());
            headers.insert("origin", format!("http://{lan}").parse().unwrap());
            tokio_tungstenite::connect_async(request).await.unwrap().0
        };
        async fn next<S, E>(socket: &mut S) -> Value
        where
            S: futures::Stream<Item = Result<Frame, E>> + Unpin,
            E: std::fmt::Debug,
        {
            match socket.next().await {
                Some(Ok(Frame::Text(text))) => serde_json::from_str(&text).unwrap(),
                other => panic!("expected a control message, got {other:?}"),
            }
        }

        // Closing the page's socket abandons the sign-in.
        let mut abandoned = connect().await;
        assert_eq!(
            next(&mut abandoned).await,
            json!({ "type": "authUrl", "url": format!("{api}/device?user_code=WXYZ-0001"), "code": "WXYZ-0001" })
        );
        abandoned.close(None).await.unwrap();
        drop(abandoned);
        let polls_when_closed = polls.lock().unwrap().get("device-1").copied().unwrap_or(0);

        let mut socket = connect().await;
        assert_eq!(next(&mut socket).await["code"], "WXYZ-0002");
        assert_eq!(next(&mut socket).await, json!({ "type": "complete" }));
        let stored: Value = serde_json::from_str(
            &std::fs::read_to_string(crate::config::config_dir().join("alphaxiv.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored["refreshToken"], "refresh-1");
        let device_1 = polls.lock().unwrap().get("device-1").copied().unwrap_or(0);
        assert!(device_1 <= polls_when_closed + 1, "{device_1} polls");
    }

    #[tokio::test]
    async fn a_lan_dashboard_answers_private_hosts_and_peers_only() {
        let app = router(test_state(), None, up_remote::Reach::Lan);
        let address = serve(app.clone()).await;
        let cases: &[(&str, &[&str], u16)] = &[
            ("GET /api/health", &["Host: 192.168.1.5:4791"], 200),
            ("GET /api/health", &["Host: 100.92.54.38:4791"], 200),
            ("GET /api/health", &["Host: 8.8.8.8:4791"], 400),
            ("GET /api/health", &["Host: workstation.local:4791"], 400),
            (
                "POST /api/health",
                &["Host: 192.168.1.5:4791", "Origin: http://192.168.1.5:4791"],
                405,
            ),
            (
                "POST /api/health",
                &["Host: 192.168.1.5:4791", "Origin: http://attacker.example"],
                403,
            ),
        ];
        for (request, headers, expected) in cases {
            assert_eq!(
                status(address, request, headers).await,
                *expected,
                "{request} {headers:?}"
            );
        }

        let mut public_peer = axum::http::Request::get("/api/health")
            .header(header::HOST, "192.168.1.5:4791")
            .body(axum::body::Body::empty())
            .unwrap();
        public_peer
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [8, 8, 8, 8],
                50000,
            ))));
        let response = tower::ServiceExt::oneshot(app, public_peer).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn the_dashboard_answers_only_its_own_loopback_pages() {
        let address = serve(router(test_state(), None, up_remote::Reach::Loopback)).await;
        let host = format!("Host: {address}");
        let origin = format!("Origin: http://{address}");
        let websocket = ["Connection: Upgrade", "Upgrade: websocket"];
        let cases: &[(&str, Vec<&str>, u16)] = &[
            ("GET /api/health", vec![&host], 200),
            ("GET /api/health", vec!["Host: localhost"], 200),
            ("GET /api/health", vec!["Host: [::1]:9"], 200),
            ("GET /api/health", vec!["Host: attacker.example"], 400),
            (
                "GET /api/health",
                vec!["Host: 127.0.0.1.attacker.example"],
                400,
            ),
            ("GET /api/health", vec![], 400),
            // The router answers 405 once a POST to this GET route clears the guard.
            ("POST /api/health", vec![&host], 405),
            ("POST /api/health", vec![&host, &origin], 405),
            (
                "POST /api/health",
                vec![&host, "Origin: http://attacker.example"],
                403,
            ),
            ("POST /api/health", vec![&host, "Origin: null"], 403),
            (
                "POST /api/health",
                vec![&host, &origin, "Sec-Fetch-Site: cross-site"],
                403,
            ),
            (
                "POST /api/health",
                vec![&host, &origin, "Sec-Fetch-Site: same-origin"],
                405,
            ),
            (
                "GET /api/health",
                vec![&host, "Sec-Fetch-Site: cross-site"],
                200,
            ),
            (
                "GET /api/health",
                [&[host.as_str()][..], &websocket].concat(),
                403,
            ),
            (
                "GET /api/health",
                [&[host.as_str(), origin.as_str()][..], &websocket].concat(),
                200,
            ),
            (
                "GET /api/health",
                [
                    &[host.as_str(), "Origin: http://attacker.example"][..],
                    &websocket,
                ]
                .concat(),
                403,
            ),
            (
                "GET /api/health",
                [
                    &[host.as_str(), origin.as_str(), "Sec-Fetch-Site: cross-site"][..],
                    &websocket,
                ]
                .concat(),
                403,
            ),
        ];
        for (request, headers, expected) in cases {
            assert_eq!(
                status(address, request, headers).await,
                *expected,
                "{request} {headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn no_route_answers_a_rebinding_host_or_a_cross_site_write() {
        let address = serve(router(test_state(), None, up_remote::Reach::Loopback)).await;
        let host = format!("Host: {address}");
        let routes = [
            "/",
            "/index.html",
            "/projects/p1/chat",
            "/api/health",
            "/api/events",
            "/api/projects",
            "/api/projects/p1/files/raw",
            "/api/chat/sessions/s1/messages",
            "/api/chat/sessions/s1/respond",
            "/api/internal/permissions",
            "/api/settings/data-dir/move",
            "/api/update/apply",
            "/api/ssh/connect",
            "/api/not-a-route",
        ];
        for path in routes {
            for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                let request = format!("{method} {path}");
                assert_eq!(
                    status(address, &request, &["Host: attacker.example"]).await,
                    400,
                    "{request}"
                );
                if method != "GET" {
                    assert_eq!(
                        status(
                            address,
                            &request,
                            &[&host, "Origin: http://attacker.example"]
                        )
                        .await,
                        403,
                        "{request}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn the_dev_server_origin_is_trusted_only_when_it_is_loopback_http() {
        if !crate::config::in_test_child(&["ORX_UI_DEV_ORIGIN"]) {
            return;
        }
        let address = serve(router(test_state(), None, up_remote::Reach::Loopback)).await;
        let host = format!("Host: {address}");
        for (dev_origin, expected) in [
            ("http://127.0.0.1:5173", 405),
            ("http://localhost:5173/", 405),
            ("https://127.0.0.1:5173", 403),
            ("http://attacker.example:5173", 403),
        ] {
            std::env::set_var("ORX_UI_DEV_ORIGIN", dev_origin);
            let origin = format!("Origin: {}", dev_origin.trim_end_matches('/'));
            assert_eq!(
                status(address, "POST /api/health", &[&host, &origin]).await,
                expected,
                "{dev_origin}"
            );
        }
    }

    #[tokio::test]
    async fn a_remote_host_admits_only_attached_sessions_and_its_own_callbacks() {
        const ATTACHMENT: &str = "abcdefghijklmnopqrstuvwxyz-attachment";
        const CALLBACK: &str = "abcdefghijklmnopqrstuvwxyz-callback";
        let auth = RemoteAuth::new(CALLBACK);
        auth.register(ATTACHMENT);
        let address = serve(router(
            test_state(),
            Some(auth.clone()),
            up_remote::Reach::Loopback,
        ))
        .await;
        let host = format!("Host: {address}");
        let attachment = format!("Authorization: Bearer {ATTACHMENT}");
        let callback = format!("Authorization: Bearer {CALLBACK}");
        let cases = [
            ("GET /api/health", None, 401),
            ("GET /api/health", Some("Authorization: Bearer guess"), 401),
            ("GET /api/health", Some(attachment.as_str()), 200),
            ("GET /api/health", Some(callback.as_str()), 401),
            ("POST /api/update/apply", Some(attachment.as_str()), 403),
            (
                "POST /api/settings/data-dir/move",
                Some(attachment.as_str()),
                403,
            ),
            ("GET /api/remote/sessions", Some(attachment.as_str()), 403),
            (
                "POST /api/projects/p1/file/open",
                Some(attachment.as_str()),
                403,
            ),
            // Past authentication, an empty body is the handler's to refuse.
            ("POST /api/runs", Some(callback.as_str()), 415),
            ("POST /api/runs/r1/cancel", None, 401),
            ("POST /api/internal/permissions", None, 415),
        ];
        for (request, authorization, expected) in cases {
            let headers: Vec<&str> = [host.as_str()].into_iter().chain(authorization).collect();
            assert_eq!(
                status(address, request, &headers).await,
                expected,
                "{request} {authorization:?}"
            );
        }

        auth.unregister(ATTACHMENT);
        assert_eq!(
            status(address, "GET /api/health", &[&host, &attachment]).await,
            401
        );
    }

    #[tokio::test]
    async fn only_an_attached_laptop_forwards_its_alphaxiv_sign_in() {
        if !crate::config::in_test_child(&["ALPHAXIV_API_URL"]) {
            return;
        }
        const ATTACHMENT: &str = "abcdefghijklmnopqrstuvwxyz-attachment";
        const CALLBACK: &str = "abcdefghijklmnopqrstuvwxyz-callback";
        let auth = RemoteAuth::new(CALLBACK);
        auth.register(ATTACHMENT);
        let host = serve(router(test_state(), Some(auth), up_remote::Reach::Loopback)).await;
        let laptop = serve(router(test_state(), None, up_remote::Reach::Loopback)).await;
        let forwarded = crate::config::config_dir().join("alphaxiv-forwarded.json");
        let body = json!({
            "apiUrl": crate::config::alphaxiv_api_url(),
            "accessToken": "forwarded",
            "expiresAtMs": crate::store::now_ms() + 3_600_000,
        })
        .to_string();
        let client = crate::net::loopback_client().build().unwrap();
        let put = |address: std::net::SocketAddr, bearer: &str, body: String| {
            client
                .put(format!("http://{address}/_orx/alphaxiv"))
                .bearer_auth(bearer)
                .body(body)
                .send()
        };

        for bearer in ["guess", CALLBACK] {
            let response = put(host, bearer, body.clone()).await.unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
        let response = put(laptop, ATTACHMENT, body.clone()).await.unwrap();
        assert_ne!(response.status(), StatusCode::NO_CONTENT);
        assert!(!forwarded.exists());

        let response = put(host, ATTACHMENT, body.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(std::fs::read_to_string(&forwarded).unwrap(), body);
        let elsewhere = body.replace(
            &crate::config::alphaxiv_api_url(),
            "https://alphaxiv.invalid",
        );
        let response = put(host, ATTACHMENT, elsewhere).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = put(host, ATTACHMENT, "x".repeat(32 * 1024)).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(std::fs::read_to_string(&forwarded).unwrap(), body);

        let response = client
            .delete(format!("http://{host}/_orx/alphaxiv"))
            .bearer_auth(ATTACHMENT)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(!forwarded.exists());
    }

    #[tokio::test]
    async fn removal_rejects_unknown_and_protected_agents_as_json() {
        let state = test_state();
        for id in [
            "../x",
            "cursor/..",
            "/tmp/x",
            "",
            "unknown",
            "acp:unknown",
            "claude-code",
            "codex",
            "opencode",
        ] {
            let error = remove_harness_install(State(state.clone()), Path(id.into()))
                .await
                .unwrap_err();
            assert_eq!(error.0, StatusCode::BAD_REQUEST);
            let response = error.into_response();
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert!(serde_json::from_slice::<Value>(&bytes).unwrap()["error"].is_string());
        }
    }

    #[tokio::test]
    async fn artifact_delete_rejects_during_project_cleanup() {
        let state = test_state();
        let _deletion = state.project_lifecycle.begin_delete("p1").unwrap();
        let error = delete_artifact(
            State(state),
            Path("p1".into()),
            Query(ArtifactPathQuery {
                path: "report.md".into(),
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, StatusCode::BAD_REQUEST);
        assert_eq!(error.1, "project deletion is in progress");
    }

    #[tokio::test]
    async fn project_cleanup_leases_survive_canceled_request_waiters() {
        let lifecycle = ProjectLifecycle::default();
        let deletion = lifecycle.begin_delete("p1").unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let (finished, completion) = tokio::sync::oneshot::channel();
        let cleanup = tokio::spawn(async move {
            let _deletion = deletion;
            tokio::task::spawn_blocking(move || blocked.recv().unwrap())
                .await
                .unwrap();
            drop(_deletion);
            finished.send(()).unwrap();
        });
        let request = tokio::spawn(async move { cleanup.await.unwrap() });
        request.abort();
        assert!(request.await.err().unwrap().is_cancelled());
        assert!(lifecycle.admit("p1").is_none());
        assert!(lifecycle.admit("__project_create__").is_none());
        release.send(()).unwrap();
        completion.await.unwrap();

        let creation = lifecycle.admit("__project_create__").unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            blocked.recv().unwrap();
            creation
        });
        let request = tokio::spawn(async move { worker.await.unwrap() });
        request.abort();
        assert!(request.await.err().unwrap().is_cancelled());
        assert!(lifecycle.begin_delete("p1").is_none());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while lifecycle.operation_count() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(lifecycle.begin_delete("p1").is_some());
    }

    #[test]
    fn project_file_text_never_lossily_decodes_binary_bytes() {
        let (content, binary) = decode_project_file_text(b"plain text\n".to_vec(), false);
        assert_eq!(content, "plain text\n");
        assert!(!binary);

        for bytes in [b"header\0payload".to_vec(), vec![0x89, b'P', b'N', b'G']] {
            let (content, binary) = decode_project_file_text(bytes, false);
            assert!(content.is_empty());
            assert!(binary);
        }

        let mut split_utf8 = b"prefix".to_vec();
        split_utf8.push(0xe2);
        let (content, binary) = decode_project_file_text(split_utf8, true);
        assert_eq!(content, "prefix");
        assert!(!binary);
    }

    #[test]
    fn project_file_versions_track_exact_bytes() {
        let first = file_version(b"same-size-a");
        let second = file_version(b"same-size-b");
        assert_ne!(first, second);

        let path = std::env::temp_dir().join(format!("orx-file-version-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"same-size-a").unwrap();
        assert_eq!(
            file_version_on_disk(&path)
                .map_err(|error| error.1)
                .unwrap(),
            first
        );
        std::fs::write(&path, b"same-size-b").unwrap();
        assert_eq!(
            file_version_on_disk(&path)
                .map_err(|error| error.1)
                .unwrap(),
            second
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn project_file_paths_reject_traversal() {
        assert_eq!(
            validated_project_file_path("./figures/chart.png")
                .map_err(|error| error.1)
                .unwrap()
                .0,
            "figures/chart.png"
        );
        for path in ["", "../secret", "/etc/passwd", "figures/../secret"] {
            assert!(
                validated_project_file_path(path).is_err(),
                "accepted {path:?}"
            );
        }
    }

    #[test]
    fn local_file_actions_rename_duplicate_and_delete() {
        let root = std::env::temp_dir().join(format!("orx-file-actions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("reports")).unwrap();
        std::fs::write(root.join("reports/result.md"), "result").unwrap();

        let renamed = manage_local_file(
            &root,
            "reports/result.md",
            FileAction::Rename,
            Some("summary.md"),
            true,
        )
        .map_err(|error| error.1)
        .unwrap();
        assert_eq!(renamed, "reports/summary.md");

        let duplicated = manage_local_file(&root, &renamed, FileAction::Duplicate, None, true)
            .map_err(|error| error.1)
            .unwrap();
        assert_eq!(duplicated, "reports/summary copy.md");
        assert_eq!(
            std::fs::read_to_string(root.join(&duplicated)).unwrap(),
            "result"
        );

        manage_local_file(&root, &duplicated, FileAction::Delete, None, true)
            .map_err(|error| error.1)
            .unwrap();
        assert!(!root.join(duplicated).exists());
        assert!(manage_local_file(
            &root,
            &renamed,
            FileAction::Rename,
            Some("../escape.md"),
            true,
        )
        .is_err());
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "private").unwrap();
        assert!(manage_local_file(&root, ".git/config", FileAction::Delete, None, true,).is_err());
        #[cfg(unix)]
        {
            let outside = std::env::temp_dir()
                .join(format!("orx-file-actions-outside-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, root.join("jump")).unwrap();
            std::os::unix::fs::symlink(root.join(&renamed), outside.join("back")).unwrap();
            assert!(
                manage_local_file(&root, "jump/back", FileAction::Delete, None, true,).is_err()
            );
            assert!(outside.join("back").exists());
            std::os::unix::fs::symlink(root.join(&renamed), root.join("reports/summary-link"))
                .unwrap();
            assert!(manage_local_file(
                &root,
                &renamed,
                FileAction::Rename,
                Some("summary-link"),
                true,
            )
            .is_err());
            assert!(root.join(&renamed).exists());
            assert!(std::fs::symlink_metadata(root.join("reports/summary-link"))
                .unwrap()
                .file_type()
                .is_symlink());
            assert!(manage_local_file(
                &root,
                "reports/summary-link",
                FileAction::Duplicate,
                None,
                true,
            )
            .is_err());
            std::os::unix::fs::symlink(root.join(".git"), root.join("git-link")).unwrap();
            assert!(
                manage_local_file(&root, "git-link/config", FileAction::Delete, None, true,)
                    .is_err()
            );
            let _ = std::fs::remove_dir_all(outside);
        }
        let case_renamed = manage_local_file(
            &root,
            &renamed,
            FileAction::Rename,
            Some("SUMMARY.md"),
            true,
        )
        .map_err(|error| error.1)
        .unwrap();
        assert_eq!(case_renamed, "reports/SUMMARY.md");
        assert!(root.join(case_renamed).exists());
        assert!(std::fs::read_dir(root.join("reports"))
            .unwrap()
            .any(|entry| entry.unwrap().file_name() == "SUMMARY.md"));
        let _ = std::fs::remove_dir_all(root);
    }

    // ApiError has no Debug, so `.unwrap()` on the Err path won't compile; drop
    // the error to its message string to make the Result assertion-friendly.
    fn abs_path(path: &str) -> std::result::Result<(String, std::path::PathBuf), String> {
        validated_absolute_file_path(path).map_err(|error| error.1)
    }

    #[test]
    fn absolute_file_paths_require_an_absolute_path() {
        #[cfg(windows)]
        let absolute = r"C:\Windows\System32\drivers\etc\hosts";
        #[cfg(not(windows))]
        let absolute = "/etc/hosts";

        assert_eq!(
            abs_path(&format!("  {absolute}  ")),
            Ok((absolute.to_string(), std::path::PathBuf::from(absolute))),
        );
        for path in ["", "   ", "relative/path", "../secret", &"/x".repeat(3000)] {
            assert!(abs_path(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn absolute_file_paths_expand_a_leading_tilde() {
        let home = crate::config::home_dir().expect("home dir");
        // `~/x` and bare `~` resolve under home; the display stays as typed.
        assert_eq!(
            abs_path("~/.ssh/config"),
            Ok(("~/.ssh/config".to_string(), home.join(".ssh/config"))),
        );
        assert_eq!(abs_path("~").map(|(_, p)| p), Ok(home));
        // `~otheruser` isn't expanded, so it stays relative and is rejected.
        assert!(abs_path("~otheruser/x").is_err());
    }

    #[test]
    fn signed_out_beats_every_key_state() {
        assert!(openresearch_summary(false, &SshReadiness::Ready).contains("orx login"));
        assert!(openresearch_summary(false, &SshReadiness::NoUsableKey).contains("orx login"));
    }

    #[test]
    fn unverified_says_why_it_could_not_check() {
        let s = openresearch_summary(
            true,
            &SshReadiness::Unverified {
                reason: "timed out".to_string(),
            },
        );
        assert!(s.contains("timed out"), "surfaces the cause: {s}");
    }
}
