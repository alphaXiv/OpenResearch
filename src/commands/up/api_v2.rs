use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::broadcast;

use crate::engine::bus::RunEvent;
use crate::engine::{handle_of, Backend, ByteRange, Engine, LogFile, LogPage, Phase, WatchedRun};
use crate::error::{anyhow, Context, Result};
use crate::store::{Contact, Store, StoredRun};

static SERVER: OnceLock<(Arc<Api>, Router)> = OnceLock::new();

/// The adapter registry and launch preparation the server owns (`engine::runtime`).
#[async_trait]
pub(crate) trait Commands: Send + Sync {
    /// Holds `admission` until the run is durable.
    async fn run(&self, args: crate::ExpRunArgs, admission: Box<dyn Send>) -> Result<String>;
    fn backend(&self, run: &StoredRun) -> Result<Arc<dyn Backend>>;
    async fn cancel(
        &self,
        run: &StoredRun,
        reason: &str,
        stop: Option<&crate::store::StopContext>,
    ) -> Result<()>;
    async fn cleanup(&self, run: &WatchedRun) -> Result<Value>;
    async fn compute(
        &self,
        action: &str,
        args: Value,
        prompt: Option<crate::engine::transport::Prompt>,
    ) -> Result<Value>;
    /// Keeps the host feed following this run and publishing `RunEvent::Grew` until drop.
    fn follow_log(&self, run: &str) -> Result<Box<dyn Send>>;
    fn host_states(&self, alias: &str) -> Vec<(String, crate::engine::transport::HostState)>;
    async fn retry_host(&self, alias: &str);
    /// Commands on this target run over the engine's connection to the host.
    fn host_target(&self, host: &str, slurm: bool) -> Result<crate::jobs::ssh::SshTarget>;
}

/// Holds a project open against deletion and data-directory moves until dropped.
pub(crate) type Admit = Box<dyn Fn(&str) -> Result<Box<dyn Send>> + Send + Sync>;

pub(crate) struct Api {
    instance: String,
    token: String,
    engine: Arc<Engine>,
    commands: Arc<dyn Commands>,
    admit: Admit,
    data_dir: PathBuf,
    store: Mutex<Store>,
    clients: AtomicUsize,
}

impl Api {
    pub fn new(
        instance: String,
        token: String,
        engine: Arc<Engine>,
        commands: Arc<dyn Commands>,
        admit: Admit,
        data_dir: PathBuf,
    ) -> Result<Self> {
        let store = Mutex::new(Store::open_at(data_dir.clone())?);
        Ok(Self {
            instance,
            token,
            engine,
            commands,
            admit,
            data_dir,
            store,
            clients: AtomicUsize::new(0),
        })
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap()
    }

    /// Admitted before preparation and held until the run is durable, so a deletion or move
    /// cannot pull storage out from under it.
    async fn launch(&self, mut args: crate::ExpRunArgs) -> Result<String> {
        let project = self
            .store()
            .get_local_experiment(&args.exp_id)?
            .ok_or_else(|| anyhow!("Local experiment {} not found.", args.exp_id))?
            .project_id;
        let admission = (self.admit)(&project)?;
        args.forwarded = true;
        crate::local::apply_compute_default(&mut args.backend, &mut args.flavor);
        args.backend.get_or_insert_with(|| "local".into());
        super::require_caller_settings(&args, &super::absolute_config_dir()).await?;
        let experiment = args.exp_id.clone();
        let id = self.commands.run(args, admission).await?;
        // Its first collection may fold before this lands; the next folds again with the patterns.
        let store = self.store();
        let raw = store.run_monitor_json(&experiment)?;
        let config = crate::engine::view::stored_patterns(&experiment, raw.as_deref())?;
        if let Some(run) = store.get_run(&id)? {
            crate::engine::view::configure_run(&store, &run, &config)?;
        }
        Ok(id)
    }

    fn run(&self, id: &str) -> Result<StoredRun> {
        self.store()
            .get_run(id)?
            .ok_or_else(|| anyhow!("Run {id} not found"))
    }

    fn view(&self, run: &StoredRun) -> Result<Value> {
        let state = self.store().engine_run_state(&run.id)?;
        let cancelling = !Phase::parse(&run.status).is_some_and(Phase::is_terminal)
            && (run.cancel_requested
                || state
                    .as_ref()
                    .is_some_and(|state| state.cancel_accepted_at.is_some()));
        let mut value = serde_json::to_value(run)?;
        value["startedAt"] = json!(state.as_ref().and_then(|state| state.started_at));
        let launch = state.as_ref().and_then(|state| state.launch.as_ref());
        if let Some(backend) = crate::store::public_backend(&run.backend_json, launch) {
            value["backendJson"] = json!(backend.to_string());
        }
        value["cancelUnconfirmed"] = json!(state
            .as_ref()
            .and_then(|state| state.cancel_unconfirmed.as_ref()));
        value["waitingReason"] = json!(state
            .as_ref()
            .and_then(|state| state.waiting_reason.as_ref()));
        value["contact"] =
            serde_json::to_value(state.map_or(Contact::Live, |state| state.contact))?;
        if let Some((failed, error)) = self.store().log_collection(&run.id)? {
            value["logCollection"] = json!(if failed { "failed" } else { "pending" });
            value["logCollectionError"] = json!(error);
        }
        value["host"] = json!(super::run_host(&self.store(), run)?);
        value["cancelling"] = json!(cancelling);
        value["cancelError"] = json!(self.engine.cancel_error(&run.id));
        value["durationSecs"] = json!(crate::local::run_duration_secs(run));
        value["updatedDisplay"] = json!(crate::local::fmt_ago(run.updated_at));
        value["monitor"] = json!(super::run_monitor_summary(&self.store(), run));
        value["outcome"] = json!(super::run_outcome(&self.store(), run));
        Ok(value)
    }

    /// Asks each run that needs the app open to stop, including runs launched meanwhile, for at most `wait`.
    #[cfg_attr(not(desktop_app), allow(dead_code))]
    pub(crate) async fn force_stop_runs(&self, wait: Duration) {
        use futures::StreamExt;

        let mut asked = std::collections::HashSet::new();
        let mut stops = futures::stream::FuturesUnordered::new();
        let stopping = async {
            loop {
                let runs = match self.store().pending_work() {
                    Ok(work) => work.runs,
                    Err(error) => {
                        eprintln!(
                            "openresearch app: could not check which runs to stop: {error:#}"
                        );
                        return;
                    }
                };
                let mut unsettled = false;
                for run in runs {
                    if run.after_quit != crate::store::AfterQuit::NeedsApp {
                        continue;
                    }
                    let stored = match self.run(&run.run_id) {
                        Ok(stored) => stored,
                        Err(error) => {
                            eprintln!(
                                "openresearch app: could not stop run {}: {error:#}",
                                run.run_id
                            );
                            continue;
                        }
                    };
                    // A run still launching takes its stop only once it has a handle, so it is asked again then;
                    // the stop its launch delivers, or one already on its way, is waited for here.
                    let launched = crate::engine::handle_of(&stored).is_some();
                    unsettled |= !launched || self.engine.delivering_cancel(&stored.id);
                    if !asked.insert((run.run_id, launched)) {
                        continue;
                    }
                    stops.push(async move {
                        if let Err(error) = self
                            .commands
                            .cancel(&stored, "Stopped when OpenResearch was force quit.", None)
                            .await
                        {
                            eprintln!(
                                "openresearch app: could not stop run {}: {error:#}",
                                stored.id
                            );
                        }
                    });
                }
                if stops.is_empty() && !unsettled {
                    return;
                }

                // Agents keep working until the quit, so a run can start while others stop.
                let poll = tokio::time::sleep(crate::config::paced(Duration::from_millis(250)));
                tokio::select! {
                    Some(()) = stops.next() => {}
                    () = poll => {}
                }
            }
        };
        if tokio::time::timeout(wait, stopping).await.is_err() {
            eprintln!("openresearch app: quitting before every run answered its stop");
        }
    }

    pub(super) fn busy(&self) -> Result<bool> {
        Ok(!self.store().pending_work()?.is_empty() || self.sse_clients() != 0)
    }

    pub fn sse_clients(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }
}

pub(crate) fn install(api: Arc<Api>) -> Result<()> {
    SERVER
        .set((
            api.clone(),
            routes(api.clone()).merge(dashboard_routes(api)),
        ))
        .map_err(|_| anyhow!("API v2 is already installed"))
}

pub(crate) fn router() -> Router {
    Router::new().route("/api/{*path}", axum::routing::any(dispatch))
}

async fn dispatch(mut request: Request) -> Response {
    use tower::ServiceExt;
    match SERVER.get() {
        Some((_, app)) => {
            // The inner router must not inherit the outer wildcard's path captures.
            request.extensions_mut().clear();
            app.clone().oneshot(request).await.unwrap()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Run engine is not initialized",
        )
            .into_response(),
    }
}

fn dashboard_routes(api: Arc<Api>) -> Router {
    Router::new()
        .route("/api/runs/{id}/log", get(log))
        .route("/api/runs/{id}/monitor", get(monitor))
        .route("/api/runs/{id}/look", post(look_now))
        .route("/api/runs/{id}/view/check", post(check_view))
        .route("/api/experiments/{id}/view", get(view).put(set_view))
        .route("/api/runs/{id}/abandon", post(dashboard_abandon))
        .route("/api/runs/{id}/cleanup", post(cleanup))
        .route("/api/compute/hosts", get(hosts))
        .route("/api/compute/hosts/{alias}/retry", post(retry_host))
        .with_state(api)
}

/// The run's log in `range`, read the way the page's log view reads it: the laptop's kept copy,
/// else the run's backend; outside `orx up`, the kept copy alone. `expected` names the log a
/// cursor was read from; another is a reset.
pub(crate) async fn run_log(
    id: &str,
    range: ByteRange,
    expected: Option<crate::engine::FileIdent>,
) -> Result<LogPage> {
    let Ok(api) = installed() else {
        let data_dir = crate::store::data_dir();
        return kept_page(&kept_log(&data_dir, id)?, &range, expected.as_ref())?
            .ok_or_else(|| anyhow!("This computer keeps none of run {id}'s log"));
    };
    read_log(api, id, range, expected)
        .await
        .map_err(|failure| match failure {
            Failure::Message(_, message) => anyhow!("{message}"),
            Failure::Unavailable { host, .. } => anyhow!(
                "The log is on {}, which is out of contact",
                host.as_deref().unwrap_or("the run's machine")
            ),
        })
}

/// `POST /api/runs/:id/look`: a look at the run's chat now, under the chat's least gap between
/// looks. 202 with the look; 409 when one ran too lately or the run is not watched.
async fn look_now(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Response {
    let run = match api.store().get_run(&id) {
        Ok(Some(run)) => run,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({"error": format!("Run {id} not found.")})),
            )
                .into_response()
        }
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": format!("Couldn't read run {id}: {error:#}")})),
            )
                .into_response()
        }
    };
    let asked = tokio::task::spawn_blocking(move || {
        let store = Store::open()?;
        Ok::<_, crate::error::Error>(crate::engine::watch::look_now(&store, &run))
    })
    .await;
    use crate::engine::watch::LookNow;
    match asked {
        Ok(Ok(Ok(look))) => (StatusCode::ACCEPTED, Json(look)).into_response(),
        Ok(Ok(Err(LookNow::TooSoon(at)))) => (
            StatusCode::CONFLICT,
            Json(json!({"error": "A look ran under two minutes ago. Try again after it.", "notBefore": at})),
        )
            .into_response(),
        Ok(Ok(Err(LookNow::NotWatched(why)))) => {
            (StatusCode::CONFLICT, Json(json!({"error": why}))).into_response()
        }
        Ok(Ok(Err(LookNow::Failed(error))) | Err(error)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Couldn't ask for a look: {error:#}")})),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("Couldn't ask for a look: {error}")})),
        )
            .into_response(),
    }
}

/// See [`Engine::cancel_error`]; none when the engine isn't running in this process.
pub(crate) fn cancel_error(run_id: &str) -> Option<String> {
    installed().ok()?.engine.cancel_error(run_id)
}

fn installed() -> Result<&'static Arc<Api>> {
    SERVER
        .get()
        .map(|(api, _)| api)
        .ok_or_else(|| anyhow!("Run engine is not initialized"))
}

#[cfg(desktop_app)]
pub(crate) fn busy() -> Result<bool> {
    installed()?.busy()
}

#[cfg(desktop_app)]
pub(crate) fn pending_work() -> Result<crate::store::PendingWork> {
    installed()?.store().pending_work()
}

/// A force quit's stops; the quit goes ahead once all have answered or ten seconds have passed.
#[cfg(desktop_app)]
pub(crate) async fn force_stop_runs() {
    match installed() {
        Ok(api) => api.force_stop_runs(Duration::from_secs(10)).await,
        Err(error) => eprintln!("openresearch app: could not stop runs before quitting: {error:#}"),
    }
}

/// For the dashboard's `/api/runs`: the same launch as `POST /api/v2/runs`.
pub(crate) async fn launch(args: crate::ExpRunArgs) -> Result<StoredRun> {
    let api = installed()?;
    let id = api.launch(args).await?;
    api.run(&id)
}

pub(crate) async fn cancel_run(
    run: &StoredRun,
    reason: &str,
    stop: Option<&crate::store::StopContext>,
) -> Result<()> {
    installed()?.commands.cancel(run, reason, stop).await
}

pub(crate) fn host_target(host: &str, slurm: bool) -> Result<crate::jobs::ssh::SshTarget> {
    installed()?.commands.host_target(host, slurm)
}

/// `compute test|connect` as the dashboard asks for it.
pub(crate) async fn check_compute(
    action: &str,
    args: Value,
    prompt: Option<crate::engine::transport::Prompt>,
) -> Result<Value> {
    installed()?.commands.compute(action, args, prompt).await
}

pub(crate) fn follow_log(run: &str) -> Result<Box<dyn Send>> {
    installed()?.commands.follow_log(run)
}

pub(crate) fn run_events() -> Result<broadcast::Receiver<RunEvent>> {
    Ok(installed()?.engine.bus.subscribe())
}

/// Counts a dashboard event stream as an idle blocker until dropped.
pub(crate) fn event_client() -> Result<impl Send> {
    let api = installed()?.clone();
    api.clients.fetch_add(1, Ordering::SeqCst);
    let subscription = Subscription { api, _log: None };
    if super::IDLE_EXIT.load(Ordering::SeqCst) {
        return Err(anyhow!("Run engine is shutting down"));
    }
    Ok(subscription)
}

fn routes(api: Arc<Api>) -> Router {
    Router::new()
        .route("/api/v2/runs", get(runs).post(run))
        .route("/api/v2/runs/{id}", get(get_run))
        .route("/api/v2/runs/{id}/cancel", post(cancel))
        .route("/api/v2/runs/{id}/abandon", post(abandon))
        .route("/api/v2/runs/{id}/revise", post(revise))
        .route("/api/v2/runs/{id}/cleanup", post(cleanup))
        .route("/api/v2/runs/{id}/log", get(log))
        .route("/api/v2/runs/{id}/monitor", get(monitor))
        .route("/api/v2/runs/{id}/view/check", post(check_view))
        .route("/api/v2/experiments/{id}/view", get(view).put(set_view))
        .route("/api/v2/events", get(events))
        .route("/api/v2/compute/{action}", post(compute))
        .route_layer(middleware::from_fn_with_state(api.clone(), authenticate))
        .route("/api/v2/hello", get(hello))
        .with_state(api)
}

async fn authenticate(State(api): State<Arc<Api>>, request: Request, next: Next) -> Response {
    use sha2::{Digest, Sha256};
    let provided = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let authorized = !api.token.is_empty()
        && provided.is_some_and(|provided| {
            let left = Sha256::digest(provided.as_bytes());
            let right = Sha256::digest(api.token.as_bytes());
            left.iter()
                .zip(right)
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0
        });
    if authorized {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "Server bearer token required").into_response()
    }
}

type ApiResult<T> = std::result::Result<T, Failure>;
enum Failure {
    Message(StatusCode, String),
    Unavailable {
        host: Option<String>,
        since: i64,
        contact: &'static str,
    },
}
impl From<crate::error::Error> for Failure {
    fn from(error: crate::error::Error) -> Self {
        Self::Message(StatusCode::BAD_REQUEST, error.to_string())
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        match self {
            Self::Message(status, message) => (status, message).into_response(),
            Self::Unavailable { host, since, contact } => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({
                "error": "Page not on this computer", "host": host, "since": since, "contact": contact,
            }))).into_response(),
        }
    }
}

#[derive(Deserialize)]
struct HelloQuery {
    nonce: String,
}
async fn hello(
    State(api): State<Arc<Api>>,
    Query(query): Query<HelloQuery>,
) -> ApiResult<Json<Value>> {
    if query.nonce.len() > 256 || query.nonce.is_empty() {
        return Err(Failure::Message(
            StatusCode::BAD_REQUEST,
            "Supply a discovery nonce".into(),
        ));
    }
    let proof = crate::commands::remote_host::hello_proof(&api.token, &api.instance, &query.nonce);
    Ok(Json(
        json!({"instance":api.instance, "proto":2, "version":env!("CARGO_PKG_VERSION"), "proof":proof}),
    ))
}

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct Filter {
    pub project: Option<String>,
    pub experiment: Option<String>,
    pub run: Option<String>,
    #[serde(default)]
    pub wait: bool,
    #[serde(default)]
    pub follow: bool,
}
impl Filter {
    fn matches(&self, run: &StoredRun) -> bool {
        self.project.as_ref().is_none_or(|id| *id == run.project_id)
            && self
                .experiment
                .as_ref()
                .is_none_or(|id| *id == run.experiment_id)
            && self.run.as_ref().is_none_or(|id| *id == run.id)
    }
    fn rows(&self, api: &Api) -> Result<Vec<StoredRun>> {
        let store = api.store();
        let rows = if let Some(id) = &self.run {
            store.get_run(id)?.into_iter().collect()
        } else if let Some(id) = &self.experiment {
            if self.wait {
                store.latest_run_for_experiment(id)?.into_iter().collect()
            } else {
                store.list_runs_by_experiment(id)?
            }
        } else if let Some(id) = &self.project {
            store.list_runs_by_project(id)?
        } else {
            store.list_runs(usize::MAX)?
        };
        Ok(rows.into_iter().filter(|run| self.matches(run)).collect())
    }
}

async fn runs(State(api): State<Arc<Api>>, Query(filter): Query<Filter>) -> ApiResult<Json<Value>> {
    let titles: HashMap<_, _> = if let Some(project) = &filter.project {
        api.store()
            .list_experiments_by_project(project)?
            .into_iter()
            .map(|exp| (exp.id.clone(), exp.display_name().to_string()))
            .collect()
    } else {
        HashMap::new()
    };
    let runs: Result<Vec<_>> = filter.rows(&api)?.iter().map(|run| api.view(run)).collect();
    Ok(Json(json!({"runs":runs?, "titles":titles})))
}
async fn get_run(State(api): State<Arc<Api>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    Ok(Json(api.view(&api.run(&id)?)?))
}
async fn run(
    State(api): State<Arc<Api>>,
    Json(args): Json<crate::ExpRunArgs>,
) -> ApiResult<Json<Value>> {
    let id = api.launch(args).await?;
    Ok(Json(api.view(&api.run(&id)?)?))
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelRequest {
    chat_session_id: Option<String>,
    reason: Option<String>,
}

async fn cancel(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    request: Option<Json<CancelRequest>>,
) -> ApiResult<Json<Value>> {
    let run = api.run(&id)?;
    let Json(request) = request.unwrap_or_default();
    let (reason, stop) = crate::commands::exp::exp_cancel_stop(
        request.chat_session_id.as_deref(),
        request.reason.as_deref(),
    )
    .map_err(|why| Failure::Message(StatusCode::BAD_REQUEST, why))?;
    api.commands.cancel(&run, &reason, Some(&stop)).await?;
    Ok(Json(api.view(&api.run(&id)?)?))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviseRequest {
    experiment_id: String,
    chat_session_id: Option<String>,
}

/// `orx exp revise`: the chat's agent is repairing the experiment after its stopped run, for as
/// long as the turn it said so in goes on and no run of the experiment starts.
async fn revise(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    Json(request): Json<ReviseRequest>,
) -> ApiResult<Json<Value>> {
    let refused = |status, why: String| Failure::Message(status, why);
    let run = api.run(&id)?;
    let experiment = &request.experiment_id;
    let Some(chat) = request.chat_session_id.as_deref() else {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            format!("`orx exp revise` marks run {id} for the agent repairing its experiment; run it from that agent's chat."),
        ));
    };
    if run.experiment_id != *experiment {
        return Err(refused(
            StatusCode::BAD_REQUEST,
            format!(
                "Run {id} belongs to experiment {}, not {experiment}.",
                run.experiment_id
            ),
        ));
    }
    match Phase::parse(&run.status) {
        Some(Phase::Cancelled) => {}
        Some(phase) if !phase.is_terminal() => {
            return Err(refused(
                StatusCode::CONFLICT,
                format!("Run {id} is still {}; stop it first with `orx exp cancel --run {id} --reason '…'`, then mark it.", run.status),
            ))
        }
        _ => {
            return Err(refused(
                StatusCode::CONFLICT,
                format!("Run {id} ended {}, not stopped, so there is nothing to revise; launch the next run with `orx exp run {experiment}`.", run.status),
            ))
        }
    }
    let store = api.store();
    if let Some(newest) = store
        .latest_run_for_experiment(experiment)?
        .filter(|newest| newest.id != id)
    {
        return Err(refused(
            StatusCode::CONFLICT,
            format!(
                "Run {id} is not experiment {experiment}'s newest run; {} is.",
                newest.id
            ),
        ));
    }
    let Some(turn) = store.active_chat_turn(chat)? else {
        return Err(refused(
            StatusCode::CONFLICT,
            format!("Chat {chat} has no turn going, so nothing would be revising run {id}."),
        ));
    };
    store.record_revision(&id, chat, &turn)?;
    // Projected now so this answer and every list says Revising; the monitor task then follows
    // the run until its revision ends.
    crate::engine::monitor::advance(&store, &run, crate::store::now_ms())?;
    drop(store);
    let state = api.store().engine_run_state(&id)?;
    api.engine
        .bus
        .publish(crate::engine::bus::RunEvent::Changed {
            run_id: id.clone(),
            phase: Phase::Cancelled,
            contact: state.map_or(crate::store::Contact::Live, |state| state.contact),
            cancelling: false,
        });
    let run = api.run(&id)?;
    let revising = api.store().run_revising(&run)?;
    Ok(Json(json!({"run": api.view(&run)?, "revising": revising})))
}

async fn abandon(State(api): State<Arc<Api>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    abandon_run(&api, &id)?;
    Ok(Json(api.view(&api.run(&id)?)?))
}
fn abandon_run(api: &Api, id: &str) -> ApiResult<()> {
    let ended = |run: &StoredRun| {
        Failure::Message(
            StatusCode::CONFLICT,
            format!(
                "Run {id} already ended ({}), so there is nothing to abandon.",
                run.status
            ),
        )
    };
    let elsewhere = || {
        Failure::Message(
            StatusCode::CONFLICT,
            format!("Run {id} is supervised by another orx process, such as an orx 0.2 run, so it can't be abandoned. Cancel it instead: `orx exp cancel --run {id}`."),
        )
    };

    let run = api.run(id)?;
    if Phase::parse(&run.status).is_some_and(Phase::is_terminal) {
        return Err(ended(&run));
    }
    if !crate::engine::is_engine_row(&run) && !api.engine.holds(id) {
        return Err(elsewhere());
    }
    let contact = api
        .store()
        .engine_run_state(id)?
        .map_or(Contact::Live, |state| state.contact);
    // A stop the backend keeps refusing is as stuck as one that cannot reach it.
    if contact == Contact::Live && api.engine.cancel_error(id).is_none() {
        return Err(Failure::Message(
            StatusCode::CONFLICT,
            format!("OpenResearch can reach run {id} again. Cancel it instead: `orx exp cancel --run {id}`."),
        ));
    }

    if !api.engine.abandon(id)? {
        let run = api.run(id)?;
        return Err(
            if Phase::parse(&run.status).is_some_and(Phase::is_terminal) {
                ended(&run)
            } else {
                elsewhere()
            },
        );
    }
    Ok(())
}

async fn dashboard_abandon(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Value>> {
    abandon_run(&api, &id)?;
    let run = api.run(&id)?;
    Ok(Json(
        serde_json::to_value(super::ApiRun::new(&api.store(), &run)?)
            .map_err(crate::error::Error::from)?,
    ))
}

pub(crate) fn host_view(
    store: &Store,
    alias: &str,
    root: &str,
    state: &crate::engine::transport::HostState,
) -> Result<Value> {
    use crate::engine::transport::{HostClass, Status};
    let class = store.engine_host_auth(alias)?.class;
    let connect_required = cfg!(windows)
        && crate::compute::supervised_ssh_needed(&store.list_active_runs()?, alias, |id| {
            installed().is_ok_and(|api| api.engine.holds(id))
        });
    Ok(json!({
        "alias": alias, "root": root,
        "status": match state.status { Status::Idle => "idle", Status::Connecting => "connecting", Status::Live => "live", Status::Backoff => "backoff", Status::NeedsAuth => "needs_auth" },
        "class": class.map(|class| match class { HostClass::Key => "key", HostClass::Interactive => "interactive" }),
        "connectRequired": connect_required,
        "sinceMs": state.since_ms, "nextRetryMs": state.next_retry_ms, "cause": state.cause,
    }))
}

async fn hosts(State(api): State<Arc<Api>>) -> ApiResult<Json<Value>> {
    let store = api.store();
    let hosts: Result<Vec<_>> = store
        .engine_hosts()?
        .into_iter()
        .map(|(alias, root, state)| host_view(&store, &alias, &root, &state))
        .collect();
    Ok(Json(json!(hosts?)))
}

async fn retry_host(
    State(api): State<Arc<Api>>,
    Path(alias): Path<String>,
) -> ApiResult<Json<Value>> {
    use crate::engine::transport::{HostClass, Status};
    let states = api.commands.host_states(&alias);
    if states.is_empty() {
        return Err(Failure::Message(
            StatusCode::NOT_FOUND,
            "Host not in use".into(),
        ));
    }
    let key = api.store().engine_host_auth(&alias)?.class == Some(HostClass::Key);
    if states.iter().any(|(_, state)| {
        !matches!(state.status, Status::Live | Status::Connecting)
            && (state.status == Status::NeedsAuth || !key)
    }) {
        return Err(Failure::Message(
            StatusCode::CONFLICT,
            "Host requires Connect".into(),
        ));
    }
    api.commands.retry_host(&alias).await;
    let states = api.commands.host_states(&alias);
    let (root, state) = states
        .iter()
        .max_by_key(|(_, state)| state.status.severity())
        .ok_or_else(|| Failure::Message(StatusCode::NOT_FOUND, "Host not in use".into()))?;
    Ok(Json(host_view(&api.store(), &alias, root, state)?))
}

async fn cleanup(State(api): State<Arc<Api>>, Path(id): Path<String>) -> ApiResult<Json<Value>> {
    let run = api.run(&id)?;
    if !Phase::parse(&run.status).is_some_and(Phase::is_terminal) {
        return Err(Failure::Message(
            StatusCode::CONFLICT,
            "Cleanup requires a finished run".into(),
        ));
    }
    let handle = handle_of(&run).ok_or_else(|| {
        Failure::Message(
            StatusCode::CONFLICT,
            "This run never started, so there is nothing to clean up".into(),
        )
    })?;
    Ok(Json(
        api.commands.cleanup(&WatchedRun { id, handle }).await?,
    ))
}
async fn compute(
    State(api): State<Arc<Api>>,
    Path(action): Path<String>,
    Json(mut args): Json<Value>,
) -> ApiResult<Json<Value>> {
    if !matches!(action.as_str(), "connect" | "test") {
        return Err(Failure::Message(
            StatusCode::NOT_FOUND,
            "Unknown compute action".into(),
        ));
    }
    #[derive(Deserialize)]
    struct Endpoint {
        port: u16,
        token: String,
    }
    let endpoint: Option<Endpoint> = args
        .as_object_mut()
        .and_then(|args| args.remove("prompt"))
        .map(serde_json::from_value)
        .transpose()
        .map_err(crate::error::Error::from)?;
    let prompt: Option<crate::engine::transport::Prompt> = if let Some(endpoint) = endpoint {
        if action != "connect"
            || endpoint.port == 0
            || endpoint.token.len() < 32
            || endpoint.token.len() > 256
        {
            return Err(Failure::Message(
                StatusCode::BAD_REQUEST,
                "Incorrect prompt callback".into(),
            ));
        }
        let client = crate::net::loopback_client()
            .build()
            .map_err(crate::error::Error::from)?;
        Some(Arc::new(move |question| {
            let client = client.clone();
            let token = endpoint.token.clone();
            let port = endpoint.port;
            Box::pin(async move {
                Ok(client
                    .post(format!("http://127.0.0.1:{port}/prompt"))
                    .bearer_auth(token)
                    .json(&question)
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Option<String>>()
                    .await?)
            })
        }))
    } else {
        None
    };
    Ok(Json(api.commands.compute(&action, args, prompt).await?))
}

#[derive(Deserialize)]
struct LogQuery {
    start: u64,
    end: u64,
    ident: Option<String>,
}

/// Kept ranges retain host offsets even when the middle of a large log is discarded.
#[derive(Serialize, Deserialize)]
pub(crate) struct KeptRange {
    pub offset: u64,
    pub len: u64,
    pub file: PathBuf,
    pub ident: crate::engine::FileIdent,
    pub eof: bool,
}

pub(crate) fn kept_ranges(data_dir: &std::path::Path, run: &str) -> Result<Vec<KeptRange>> {
    let path = data_dir.join("run-logs").join(format!("{run}.ranges.json"));
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// Replaces a run's kept ranges with a collected log, removing page files it no longer uses.
pub(crate) fn keep_ranges(
    data_dir: &std::path::Path,
    run: &str,
    ranges: Vec<KeptRange>,
) -> Result<()> {
    let manifest = data_dir.join("run-logs").join(format!("{run}.ranges.json"));
    let old = kept_ranges(data_dir, run)?;
    crate::local::git::atomic_write_with_mode(
        &manifest,
        &serde_json::to_vec(&ranges)?,
        Some(0o600),
    )?;
    for range in old {
        if !ranges.iter().any(|kept| kept.file == range.file) {
            std::fs::remove_file(range.file)?;
        }
    }
    Ok(())
}

/// The laptop's copy of a run's log: its kept ranges and, while collection is under way, the
/// bytes its `.part` files have synced. Partial files of a newer log replace the older log.
pub(crate) fn kept_log(data_dir: &std::path::Path, run: &str) -> Result<Vec<KeptRange>> {
    let directory = data_dir.join("run-logs");
    let mut kept = kept_ranges(data_dir, run)?;
    let mut partials = Vec::new();
    for suffix in ["log", "tail.log"] {
        let marker = directory.join(format!("{run}.{suffix}.part.json"));
        let bytes = match std::fs::read(&marker) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let (ident, offset, synced): (crate::engine::FileIdent, u64, u64) =
            serde_json::from_slice(&bytes)?;
        let part = directory.join(format!("{run}.{suffix}.part"));
        let written = match std::fs::metadata(&part) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let len = synced.min(written);
        if len != 0 {
            let modified = std::fs::metadata(&marker)?.modified()?;
            partials.push((
                modified,
                suffix,
                KeptRange {
                    offset,
                    len,
                    file: part,
                    ident,
                    eof: false,
                },
            ));
        }
    }
    let newest = partials
        .iter()
        .max_by_key(|partial| partial.0)
        .map(|p| p.2.ident.clone());
    if let Some(generation) = newest {
        kept.retain(|range| range.ident == generation);
        for (_, suffix, partial) in partials.into_iter().filter(|p| p.2.ident == generation) {
            let whole = directory.join(format!("{run}.{suffix}"));
            let superseded = |range: &KeptRange| {
                range.file == whole && range.offset == partial.offset && range.len >= partial.len
            };
            if kept.iter().any(superseded) {
                continue;
            }
            kept.retain(|range| range.file != whole);
            kept.push(partial);
        }
        kept.sort_by_key(|range| range.offset);
    }
    Ok(kept)
}

/// Reads the laptop's copy of a run's log from `range.start`, across file boundaries; `None` when
/// no kept byte is there. A different identity than `expected` is a reset.
pub(crate) fn kept_page(
    kept: &[KeptRange],
    range: &ByteRange,
    expected: Option<&crate::engine::FileIdent>,
) -> Result<Option<LogPage>> {
    use std::io::{Read, Seek, SeekFrom};
    let known_len = kept
        .iter()
        .filter(|range| range.eof)
        .map(|range| range.offset + range.len)
        .max();
    let mut cursor = range.start;
    let mut bytes = Vec::new();
    let mut identity = None;
    let mut eof = false;
    while let Some(kept) = kept
        .iter()
        .filter(|kept| {
            kept.offset <= cursor
                && (cursor < kept.offset.saturating_add(kept.len)
                    || kept.eof && cursor == kept.offset.saturating_add(kept.len))
        })
        .max_by_key(|kept| kept.offset)
    {
        if identity.as_ref().is_some_and(|ident| *ident != kept.ident) {
            break;
        }
        if expected.is_some_and(|ident| *ident != kept.ident) {
            return Ok(Some(LogPage::Reset {
                info: Some(orx_host_proto::LogInfo {
                    ident: kept.ident.clone(),
                    len: known_len.unwrap_or(kept.offset + kept.len),
                }),
            }));
        }
        identity = Some(kept.ident.clone());
        let end = range.end.min(kept.offset + kept.len);
        let mut file = std::fs::File::open(&kept.file)
            .with_context(|| format!("Open kept log {}", kept.file.display()))?;
        file.seek(SeekFrom::Start(cursor - kept.offset))?;
        let previous = bytes.len();
        bytes.resize(previous + (end - cursor) as usize, 0);
        file.read_exact(&mut bytes[previous..])?;
        cursor = end;
        eof = kept.eof && cursor == kept.offset + kept.len;
        if cursor == range.end || eof {
            break;
        }
    }
    Ok(identity.map(|ident| LogPage::Data {
        len: known_len,
        ident,
        start: range.start,
        bytes,
        eof,
    }))
}

/// A view request's answer: its body with the experiment's `ETag`, or the refusal in the words
/// the user reads.
fn view_answer(answer: std::result::Result<Value, crate::engine::view::Refusal>) -> Response {
    use crate::engine::view::Refusal;
    match answer {
        Ok(body) => {
            let etag = body["etag"].as_str().unwrap_or_default().to_owned();
            ([(axum::http::header::ETAG, etag)], Json(body)).into_response()
        }
        Err(refusal) => {
            let (status, message) = match refusal {
                Refusal::Missing(message) => (StatusCode::NOT_FOUND, message),
                Refusal::Invalid(message) => (StatusCode::BAD_REQUEST, message),
                Refusal::Changed(message) => (StatusCode::CONFLICT, message),
                Refusal::Failed(message) => {
                    eprintln!("orx up: view: {message}");
                    (StatusCode::INTERNAL_SERVER_ERROR, message)
                }
            };
            (status, Json(json!({ "error": message }))).into_response()
        }
    }
}

async fn blocking_view(
    work: impl FnOnce() -> std::result::Result<Value, crate::engine::view::Refusal> + Send + 'static,
) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(answer) => view_answer(answer),
        Err(cause) => view_answer(Err(crate::engine::view::Refusal::Failed(cause.to_string()))),
    }
}

/// `GET /api/experiments/:id/view`: the experiment's saved view, or the default its runs draw.
async fn view(State(api): State<Arc<Api>>, Path(id): Path<String>) -> Response {
    blocking_view(move || crate::engine::view::described(&api.store(), &id)).await
}

#[derive(Deserialize)]
struct ViewBody {
    markup: Option<String>,
    /// The file the markup came from, named in errors.
    file: Option<String>,
    patterns: Option<Value>,
    #[serde(rename = "patternFile")]
    pattern_file: Option<String>,
    /// What else the experiment's runs are drawn with: `{"plan": [...]}`.
    config: Option<Value>,
    #[serde(rename = "configFile")]
    config_file: Option<String>,
}

/// `PUT /api/experiments/:id/view` with `If-Match`: saves the view and its patterns, and draws
/// the experiment's runs with them. New patterns have each run's log read again: a live one at
/// its next collection, an ended one from the log kept on this machine.
async fn set_view(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
    Json(body): Json<ViewBody>,
) -> Response {
    use crate::engine::view::Refusal;
    let Some(expected) = headers
        .get(axum::http::header::IF_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
    else {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({"error": format!("Send If-Match with the etag GET /api/experiments/{id}/view returned.")})),
        )
            .into_response();
    };
    let refold = api.clone();
    let repatterned = body.patterns.is_some();
    let answer = tokio::task::spawn_blocking(move || {
        let store = api.store();
        let source = crate::engine::view::Source {
            file: body.file.as_deref(),
            patterns: body.pattern_file.as_deref(),
            config: body.config_file.as_deref(),
        };
        let saved = crate::engine::view::set(
            &store,
            &id,
            &expected,
            body.markup.as_deref(),
            body.patterns.as_ref(),
            body.config.as_ref(),
            &source,
        )?;
        let runs = if repatterned {
            crate::engine::view::configure_runs(&store, &id)?
        } else {
            store.list_runs_by_experiment(&id).map_err(Refusal::from)?
        };
        let now = crate::store::now_ms();
        for run in &runs {
            if let Err(error) = crate::engine::monitor::advance(&store, run, now) {
                eprintln!("orx up: run {} after its view changed: {error:#}", run.id);
            }
        }
        let ended = runs
            .into_iter()
            .filter(|run| repatterned && Phase::parse(&run.status).is_some_and(Phase::is_terminal))
            .collect::<Vec<_>>();
        Ok((saved, ended))
    })
    .await
    .unwrap_or_else(|cause| Err(Refusal::Failed(cause.to_string())));
    let (saved, ended) = match answer {
        Ok(answer) => answer,
        Err(refusal) => return view_answer(Err(refusal)),
    };
    tokio::spawn(async move {
        for run in ended {
            refold_kept(&refold, &run).await;
        }
    });
    view_answer(Ok(saved))
}

/// Reads an ended run's kept log again with its patterns, a few seconds a pass, and draws it.
async fn refold_kept(api: &Api, run: &StoredRun) {
    let Some(handle) = handle_of(run) else {
        return;
    };
    let watched = WatchedRun {
        id: run.id.clone(),
        handle,
    };
    let as_of = run.ended_at.unwrap_or(run.updated_at);
    for _ in 0..1000 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        let now = crate::store::now_ms();
        match crate::engine::adapters::signals::collect_kept(
            &api.data_dir,
            &watched,
            now,
            as_of,
            deadline,
        )
        .await
        {
            Ok(false) => continue,
            Ok(true) => break,
            Err(error) => {
                eprintln!(
                    "orx up: run {} could not be read again with its patterns: {error:#}",
                    run.id
                );
                break;
            }
        }
    }
    if let Err(error) = crate::engine::monitor::advance(&api.store(), run, crate::store::now_ms()) {
        eprintln!(
            "orx up: run {} after its patterns changed: {error:#}",
            run.id
        );
    }
}

#[derive(Default, Deserialize)]
struct CheckBody {
    markup: Option<String>,
    file: Option<String>,
    patterns: Option<Value>,
    #[serde(rename = "patternFile")]
    pattern_file: Option<String>,
}

/// The most of a run's log `orx exp view check` reads with patterns.
const CHECK_BYTES: u64 = 64 * 1024 * 1024;

/// `POST /api/runs/:id/view/check`: checks a view, or the saved one, against what the run
/// reported; with patterns, given or saved, against what they read from the run's log now.
async fn check_view(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    body: Option<Json<CheckBody>>,
) -> Response {
    use crate::engine::view::Refusal;
    let Json(body) = body.unwrap_or_default();
    let body = Arc::new(body);
    let (given, run) = (body.clone(), id.clone());
    let reading = api.clone();
    let patterns = tokio::task::spawn_blocking(move || {
        let store = reading.store();
        let row = store
            .get_run(&run)?
            .ok_or_else(|| Refusal::Missing(format!("Run {run} not found.")))?;
        let source = crate::engine::view::Source {
            file: given.file.as_deref(),
            patterns: given.pattern_file.as_deref(),
            config: None,
        };
        let fix = format!(
            "Run `orx exp view check {run} --pattern {}`.",
            given.pattern_file.as_deref().unwrap_or("PATH")
        );
        let list = match &given.patterns {
            Some(list) => list.clone(),
            None => {
                let raw = store.run_monitor_json(&row.experiment_id)?;
                json!(
                    crate::engine::view::stored_patterns(&row.experiment_id, raw.as_deref())?
                        .patterns
                )
            }
        };
        Ok::<_, Refusal>(crate::engine::view::patterns(&list, &source, &fix)?.1)
    })
    .await
    .unwrap_or_else(|cause| Err(Refusal::Failed(cause.to_string())));
    let patterns = match patterns {
        Ok(patterns) => patterns,
        Err(refusal) => return view_answer(Err(refusal)),
    };
    let folded = match patterns {
        Some(patterns) => match fold_for_check(&api, &id, patterns).await {
            Ok(folded) => Some(folded),
            Err(error) => {
                let message =
                    format!("Run {id}'s log could not be read to check its patterns: {error:#}");
                return view_answer(Err(Refusal::Failed(message)));
            }
        },
        None => None,
    };
    blocking_view(move || {
        let source = crate::engine::view::Source {
            file: body.file.as_deref(),
            patterns: body.pattern_file.as_deref(),
            config: None,
        };
        crate::engine::view::check(
            &api.store(),
            &id,
            body.markup.as_deref(),
            folded.as_ref(),
            &source,
        )
    })
    .await
}

/// The run's log as it reads now, folded afresh with `patterns`, counting what each matched.
async fn fold_for_check(
    api: &Api,
    id: &str,
    patterns: Arc<orx_signals::Patterns>,
) -> Result<orx_signals::Snapshot> {
    let mut folded: Option<orx_signals::Snapshot> = None;
    let mut ident = None;
    loop {
        let start = folded.as_ref().map_or(0, |f| f.cursor.offset);
        let range = ByteRange {
            start,
            end: start + orx_host_proto::MAX_PAGE_LEN,
        };
        let page =
            read_log(api, id, range, ident.clone())
                .await
                .map_err(|failure| match failure {
                    Failure::Message(_, message) => anyhow!("{message}"),
                    Failure::Unavailable { host, .. } => anyhow!(
                        "the host {} holding it cannot be reached",
                        host.unwrap_or_default()
                    ),
                })?;
        let LogPage::Data {
            ident: page_ident,
            bytes,
            eof,
            start: at,
            ..
        } = page
        else {
            return folded.ok_or_else(|| anyhow!("it has no log yet"));
        };
        let snapshot = folded.get_or_insert_with(|| {
            let mut snapshot =
                orx_signals::Snapshot::new(id, &String::from(page_ident.clone()), "check");
            snapshot.tally = Some(Default::default());
            snapshot.configure(Some(patterns.clone()));
            snapshot
        });
        ident = Some(page_ident);
        if at > start {
            snapshot.coverage.add_reason("missing_middle");
            snapshot.cursor.offset = at;
        }
        let last = eof || bytes.is_empty() || at + bytes.len() as u64 >= CHECK_BYTES;
        snapshot
            .feed(&bytes, eof, crate::store::now_ms(), false)
            .map_err(|error| anyhow!(error))?;
        if last {
            return Ok(folded.unwrap());
        }
    }
}

#[derive(Deserialize)]
struct MonitorQuery {
    cursor: Option<String>,
    series: Option<String>,
    after: Option<f64>,
    limit: Option<usize>,
}

/// `GET /api/runs/:id/monitor`: the run's whole monitor at one `rev`, bounded and paged by
/// cursor, or one series' history with `?series=`.
async fn monitor(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    Query(query): Query<MonitorQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let error = |status: StatusCode, message: String| {
        (status, Json(json!({ "error": message }))).into_response()
    };
    let read = tokio::task::spawn_blocking(move || -> Result<Option<_>> {
        let store = api.store();
        let Some(run) = store.get_run(&id)? else {
            return Ok(None);
        };
        crate::engine::monitor::monitor(&store, &run).map(Some)
    })
    .await;
    let (projection, monitor) = match read {
        Ok(Ok(Some(read))) => read,
        Ok(Ok(None)) => return error(StatusCode::NOT_FOUND, "This run no longer exists.".into()),
        Ok(Err(cause)) => {
            eprintln!("orx up: monitor: {cause:#}");
            return error(StatusCode::SERVICE_UNAVAILABLE, cause.to_string());
        }
        Err(cause) => return error(StatusCode::INTERNAL_SERVER_ERROR, cause.to_string()),
    };
    let etag = projection.etag();
    if let Some(name) = query.series {
        let page = crate::engine::monitor::series_page(
            &monitor,
            &name,
            query.after,
            query.limit.unwrap_or(2048),
        );
        return match page {
            Some(page) => ([(axum::http::header::ETAG, etag)], Json(page)).into_response(),
            None => error(
                StatusCode::NOT_FOUND,
                format!("This run has no series named \"{name}\"."),
            ),
        };
    }
    let basis = format!(
        "{}.{}",
        monitor["generation"].as_str().unwrap_or_default(),
        monitor["seriesRev"]
    );
    let page = match query
        .cursor
        .as_deref()
        .map(|cursor| cursor.rsplit_once('.'))
    {
        None => 0,
        Some(Some((pinned, page))) if pinned == basis => page.parse().unwrap_or(0),
        Some(_) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "This run's series changed since the first page; read it again.",
                    "reset": true,
                    "generation": monitor["generation"],
                    "rev": monitor["rev"],
                })),
            )
                .into_response()
        }
    };
    if page == 0
        && headers
            .get(axum::http::header::IF_NONE_MATCH)
            .is_some_and(|tag| tag.as_bytes() == etag.as_bytes())
    {
        return (StatusCode::NOT_MODIFIED, [(axum::http::header::ETAG, etag)]).into_response();
    }
    let rev = monitor["rev"].clone();
    let (mut body, next) = crate::engine::monitor::bounded(monitor, page);
    if next.is_some() || page > 0 {
        body["page"] = json!({"next": next.map(|next| format!("{basis}.{next}")), "rev": rev});
    }
    ([(axum::http::header::ETAG, etag)], Json(body)).into_response()
}

/// A page, and which of its ranges are kept for the identity it reads.
async fn log(
    State(api): State<Arc<Api>>,
    Path(id): Path<String>,
    Query(query): Query<LogQuery>,
) -> ApiResult<Json<Value>> {
    let expected = query
        .ident
        .map(crate::engine::FileIdent::try_from)
        .transpose()
        .map_err(|error| Failure::Message(StatusCode::BAD_REQUEST, error))?;
    let range = ByteRange {
        start: query.start,
        end: query.end,
    };
    let page = read_log(&api, &id, range, expected).await?;
    let mut value = serde_json::to_value(&page).map_err(crate::error::Error::from)?;
    if let LogPage::Data { ident, .. } = &page {
        let kept: Vec<_> = kept_log(&api.data_dir, &id)?
            .iter()
            .filter(|range| range.ident == *ident)
            .map(|range| [range.offset, range.offset + range.len])
            .collect();
        value["kept"] = json!(kept);
    }
    Ok(Json(value))
}

async fn read_log(
    api: &Api,
    id: &str,
    range: ByteRange,
    expected: Option<crate::engine::FileIdent>,
) -> ApiResult<LogPage> {
    if range.end < range.start || range.end - range.start > orx_host_proto::MAX_PAGE_LEN {
        return Err(Failure::Message(
            StatusCode::BAD_REQUEST,
            "Choose a log range of at most 1 MiB".into(),
        ));
    }
    let run = api.run(id)?;
    // Until its log is kept through the end, a finished run is read like a live one.
    let terminal = Phase::parse(&run.status).is_some_and(Phase::is_terminal)
        && !api.store().log_collection_pending(id)?;
    let kept = kept_log(&api.data_dir, id)?;
    match kept_page(&kept, &range, expected.as_ref())? {
        Some(LogPage::Data {
            len,
            ident,
            start,
            bytes,
            eof,
        }) => {
            let filled = start + bytes.len() as u64 == range.end;
            if !bytes.is_empty() || filled || eof && terminal {
                return Ok(LogPage::Data {
                    len,
                    ident,
                    start,
                    bytes,
                    eof: eof && (terminal || filled),
                });
            }
        }
        Some(reset) => return Ok(reset),
        None => {}
    }

    // A 0.2.x supervisor kept its log here; the host may no longer have it.
    let legacy_log = api.data_dir.join("run-logs").join(format!("{id}.log"));
    if !crate::engine::is_engine_row(&run)
        && (legacy_log.is_file() || legacy_log.with_extension("cursor.json").is_file())
    {
        let snapshot = crate::jobs::ssh::service::logs::read_from(
            &api.data_dir,
            id,
            range.start,
            range.end - range.start,
            None,
        )?;
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(snapshot.generation.as_bytes());
        let ident = crate::engine::FileIdent {
            dev: 0,
            ino: u64::from_be_bytes(digest[..8].try_into().unwrap()),
        };
        if snapshot.offset != range.start || expected.as_ref().is_some_and(|e| *e != ident) {
            return Ok(LogPage::Reset {
                info: Some(orx_host_proto::LogInfo {
                    ident,
                    len: snapshot.size,
                }),
            });
        }
        let eof = snapshot.offset + snapshot.bytes.len() as u64 >= snapshot.size;
        return Ok(LogPage::Data {
            len: Some(snapshot.size),
            ident,
            start: range.start,
            bytes: snapshot.bytes,
            eof,
        });
    }

    let Some(handle) = handle_of(&run) else {
        let message = if terminal {
            "This run never started, so it has no log"
        } else {
            "This run hasn't started yet, so it has no log"
        };
        return Err(Failure::Message(StatusCode::NOT_FOUND, message.into()));
    };
    let fetch = async {
        let backend = api.commands.backend(&run)?;
        let watched = WatchedRun {
            id: id.to_owned(),
            handle,
        };
        let page = backend
            .read_log(&watched, LogFile::Log, range, expected.as_ref())
            .await;
        // A finished run's log is kept; what reading the rest set up goes again.
        if terminal {
            backend.unwatch(&watched).await;
        }
        page
    }
    .await;
    let page = fetch.map_err(|error| {
        let transport = error.chain().any(|cause| {
            cause.downcast_ref::<std::io::Error>().is_some_and(|error| {
                matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::NotConnected
                        | std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::TimedOut
                )
            }) || cause
                .downcast_ref::<reqwest::Error>()
                .is_some_and(|error| error.is_connect() || error.is_timeout())
        });
        let state = error
            .downcast_ref::<crate::engine::transport::Unavailable>()
            .map(|error| &error.state);
        if state.is_some() || transport {
            let contact = if state
                .is_some_and(|state| state.status == crate::engine::transport::Status::NeedsAuth)
            {
                "needs_auth"
            } else {
                "unreachable"
            };
            let since = state
                .map(|state| state.since_ms)
                .filter(|since| *since > 0)
                .unwrap_or_else(crate::store::now_ms);
            let host = super::run_host(&api.store(), &run).ok().flatten();
            Failure::Unavailable {
                host,
                since,
                contact,
            }
        } else {
            Failure::Message(StatusCode::BAD_REQUEST, error.to_string())
        }
    })?;
    Ok(page)
}

struct Subscription {
    api: Arc<Api>,
    _log: Option<Box<dyn Send>>,
}
impl Drop for Subscription {
    fn drop(&mut self) {
        self.api.clients.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn events(
    State(api): State<Arc<Api>>,
    Query(filter): Query<Filter>,
) -> ApiResult<Sse<impl Stream<Item = std::result::Result<Event, Infallible>>>> {
    // Subscribe before taking the snapshot so a concurrent commit cannot be lost.
    let receiver = api.engine.bus.subscribe();
    let rows = filter.rows(&api)?;
    let snapshot = if filter.wait {
        let values: Result<Vec<_>> = rows.iter().map(|run| api.view(run)).collect();
        Some(Ok(Event::default()
            .event("snapshot")
            .data(json!({"snapshot":values?}).to_string())))
    } else {
        None
    };
    let completed: HashSet<_> = rows
        .iter()
        .filter(|run| Phase::parse(&run.status).is_some_and(Phase::is_terminal))
        .map(|run| run.id.clone())
        .collect();
    let initial = if filter.wait && filter.project.is_some() && filter.experiment.is_none() {
        rows.iter()
            .all(|run| Phase::parse(&run.status).is_some_and(Phase::is_terminal))
            .then(|| json!({"drained":true}))
    } else if filter.wait {
        rows.first()
            .filter(|run| Phase::parse(&run.status).is_some_and(Phase::is_terminal))
            .map(|run| api.view(run))
            .transpose()?
    } else if filter.run.is_some() {
        rows.first().map(|run| api.view(run)).transpose()?
    } else {
        None
    };
    let log = if filter.follow {
        Some(
            api.commands.follow_log(
                filter
                    .run
                    .as_deref()
                    .ok_or_else(|| anyhow!("Following logs requires a run id"))?,
            )?,
        )
    } else {
        None
    };
    api.clients.fetch_add(1, Ordering::SeqCst);
    let subscription = Subscription { api, _log: log };
    if super::IDLE_EXIT.load(Ordering::SeqCst) {
        return Err(anyhow!("Run engine is shutting down").into());
    }
    let stream = futures::stream::unfold(
        (subscription, receiver, filter, initial, false, completed),
        |(subscription, mut receiver, filter, initial, done, completed)| async move {
            if done {
                return None;
            }
            if let Some(initial) = initial {
                let done = filter.wait;
                return Some((
                    Ok(Event::default().event("run").data(initial.to_string())),
                    (subscription, receiver, filter, None, done, completed),
                ));
            }
            loop {
                let api = &subscription.api;
                let result = match receiver.recv().await {
                    Ok(event @ (RunEvent::Changed { .. } | RunEvent::Grew { .. })) => {
                        let run_id = match &event {
                            RunEvent::Changed { run_id, .. } | RunEvent::Grew { run_id, .. } => {
                                run_id
                            }
                            _ => unreachable!(),
                        };
                        api.run(run_id).and_then(|run| {
                            if !filter.matches(&run) || filter.wait && completed.contains(run_id) {
                                return Ok(None);
                            }
                            if filter.wait && filter.experiment.is_some() {
                                let rows = filter.rows(api)?;
                                if rows.first().is_none_or(|latest| latest.id != run.id) {
                                    return Ok(None);
                                }
                            }
                            let mut value = api.view(&run)?;
                            match &event {
                                RunEvent::Changed {
                                    phase,
                                    contact,
                                    cancelling,
                                    ..
                                } => {
                                    value["status"] = json!(phase.as_str());
                                    value["contact"] = serde_json::to_value(contact)?;
                                    value["cancelling"] = json!(cancelling);
                                    value["event"] = json!("changed");
                                }
                                RunEvent::Grew { file, len, .. } => {
                                    if filter.wait {
                                        return Ok(None);
                                    }
                                    value["file"] = serde_json::to_value(file)?;
                                    value["len"] = json!(len);
                                }
                                _ => unreachable!(),
                            }
                            Ok(Some(value))
                        })
                    }
                    Ok(
                        RunEvent::Notices { .. }
                        | RunEvent::Host { .. }
                        | RunEvent::Monitor { .. }
                        | RunEvent::LogCollection { .. },
                    ) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if filter.wait {
                            filter.rows(api).and_then(|rows| {
                                rows.iter()
                                    .find(|run| {
                                        Phase::parse(&run.status).is_some_and(Phase::is_terminal)
                                            && !completed.contains(&run.id)
                                    })
                                    .map(|run| api.view(run))
                                    .transpose()
                            })
                        } else {
                            Ok(Some(json!({"resync":true})))
                        }
                    }
                };
                match result {
                    Ok(Some(value)) => {
                        let done = filter.wait
                            && Phase::parse(value["status"].as_str().unwrap_or_default())
                                .is_some_and(Phase::is_terminal);
                        return Some((
                            Ok(Event::default().event("run").data(value.to_string())),
                            (subscription, receiver, filter, None, done, completed),
                        ));
                    }
                    Ok(None) => continue,
                    Err(error) => {
                        return Some((
                            Ok(Event::default().event("error").data(error.to_string())),
                            (subscription, receiver, filter, None, true, completed),
                        ))
                    }
                }
            }
        },
    );
    let stream = futures::stream::iter(snapshot).chain(stream);
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[derive(Default)]
pub(crate) struct Idle {
    since: Option<Duration>,
}
impl Idle {
    pub fn should_exit(&mut self, now: Duration, busy: bool) -> bool {
        if busy {
            self.since = None;
            return false;
        }
        now.saturating_sub(*self.since.get_or_insert(now)) >= Duration::from_secs(600)
    }

    pub fn check(&mut self, api: &Api, now: Duration) -> Result<bool> {
        Ok(self.should_exit(
            now,
            api.busy()? || super::ACTIVE.load(Ordering::SeqCst) != 0,
        ))
    }
}

#[cfg(test)]
mod tests;
