//! Trackio for orx: where runs log, the Settings check, which runs logged and their metrics,
//! and the local dashboard when no Space or server is saved.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::synced_env_var;

pub const SERVER_URL: &str = "TRACKIO_SERVER_URL";
pub const SPACE_ID: &str = "TRACKIO_SPACE_ID";
pub const PROJECT: &str = "TRACKIO_PROJECT";
pub const WRITE_TOKEN: &str = "TRACKIO_WRITE_TOKEN";
const DIR: &str = "TRACKIO_DIR";
/// 0.43 is the release the dashboard's `runs`, `sidebar` and `navbar` params were checked against.
const LOCAL_REQUIREMENT: &str = "trackio>=0.43,<1";

/// Where runs log, in Trackio's own precedence: a Space wins over a server URL.
pub enum Target {
    Space(String),
    Server(String),
    Local,
}

pub fn target() -> Target {
    if let Some(id) = synced_env_var(SPACE_ID) {
        return Target::Space(id);
    }
    match synced_env_var(SERVER_URL).and_then(|raw| base_url(&raw)) {
        Some(url) => Target::Server(url),
        None => Target::Local,
    }
}

/// The Trackio project an orx project's runs log to: the saved one, else the orx project's slug.
pub fn project_for(orx_project_slug: &str) -> String {
    synced_env_var(PROJECT).unwrap_or_else(|| orx_project_slug.to_string())
}

/// The value the Environment page may show and edit in the clear; `None` keeps it masked.
pub fn plain_value(key: &str, value: &str) -> Option<String> {
    match key {
        // An unparseable URL is shown as typed unless it might carry a token.
        SERVER_URL => {
            base_url(value).or_else(|| (!value.contains(['?', '@'])).then(|| value.to_string()))
        }
        SPACE_ID | PROJECT => Some(value.to_string()),
        _ => None,
    }
}

/// Drops the query (`trackio.show()` URLs carry `?write_token=`), fragment and
/// credentials, so the result is safe to show, link to, and log.
fn base_url(raw: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    url.set_username("").ok()?;
    url.set_password(None).ok()?;
    Some(url.as_str().trim_end_matches('/').to_string())
}

/// True for an address only this machine can reach, including Gradio's `0.0.0.0` bind address.
pub fn is_local_only(url: &str) -> bool {
    crate::local::local_models::is_loopback_url(url)
        || reqwest::Url::parse(url).is_ok_and(|url| {
            url.host_str()
                .and_then(|host| {
                    host.trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .ok()
                })
                .is_some_and(|ip| ip.is_unspecified())
        })
}

/// Where Trackio's client keeps local runs: `TRACKIO_DIR`, else `$HF_HOME/trackio`.
pub fn local_dir() -> Option<PathBuf> {
    let saved_or_env = |key: &str| {
        synced_env_var(key)
            .or_else(|| std::env::var(key).ok())
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = saved_or_env(DIR) {
        return Some(dir);
    }
    let hf_home = saved_or_env("HF_HOME").or_else(|| {
        saved_or_env("XDG_CACHE_HOME")
            .or_else(|| dirs::home_dir().map(|home| home.join(".cache")))
            .map(|cache| cache.join("huggingface"))
    })?;
    Some(hf_home.join("trackio"))
}

/// Pins a local run's `TRACKIO_DIR` to the directory the local dashboard reads.
pub fn local_run_env() -> Option<(String, String)> {
    if !matches!(target(), Target::Local) {
        return None;
    }
    Some((DIR.to_string(), local_dir()?.to_string_lossy().into_owned()))
}

/// Something has logged to this machine's local store, so the user relies on Trackio.
pub fn used_locally() -> bool {
    local_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .is_some_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.path().extension().is_some_and(|ext| ext == "db"))
        })
}

/// Trackio's on-disk project name (`trackio.utils.canonical_project_name`).
fn canonical_project_name(project: &str) -> String {
    let safe: String = project
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .collect();
    if safe.is_empty() {
        "default".to_string()
    } else {
        safe
    }
}

fn has_local_project(project: &str) -> bool {
    local_dir().is_some_and(|dir| {
        dir.join(format!("{}.db", canonical_project_name(project)))
            .is_file()
    })
}

fn hf_api() -> String {
    format!("{}/api", crate::jobs::huggingface::endpoint())
}

/// No redirects: tokens must only ever reach the host they were sent to.
fn client(base: &str) -> Result<reqwest::Client, String> {
    let builder = if is_local_only(base) {
        crate::net::loopback_client()
    } else {
        crate::net::remote_client()
    };
    builder
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())
}

/// `bearer` is the HF token, which a private Space needs even to answer.
async fn version(
    client: &reqwest::Client,
    base: &str,
    bearer: Option<&str>,
) -> Result<String, String> {
    let mut req = client.get(format!("{base}/version"));
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let res = req
        .send()
        .await
        .map_err(|error| format!("Could not reach {base}: {}", error.without_url()))?;
    if !res.status().is_success() {
        return Err(format!(
            "{base}/version answered {}; is this a Trackio server?",
            res.status()
        ));
    }
    res.json::<Value>()
        .await
        .ok()
        .and_then(|body| body["version"].as_str().map(str::to_string))
        .ok_or_else(|| format!("{base}/version did not identify a Trackio server"))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub reachable: bool,
    pub version: Option<String>,
    /// `None` when no token is saved or the check was inconclusive.
    pub write_access: Option<bool>,
    pub error: Option<String>,
    /// The Space's runtime stage (e.g. `SLEEPING`); `None` for a self-hosted server.
    pub space_stage: Option<String>,
}

impl Preflight {
    fn failed(error: String) -> Self {
        Self {
            reachable: false,
            version: None,
            write_access: None,
            error: Some(error),
            space_stage: None,
        }
    }
}

/// The Settings check for whichever destination is saved.
pub async fn check() -> Result<Preflight, String> {
    match target() {
        Target::Space(id) => Ok(space_preflight(&id).await),
        Target::Server(base) => Ok(preflight(&base, synced_env_var(WRITE_TOKEN).as_deref()).await),
        Target::Local => Err(format!("Save {SPACE_ID} or an http(s) {SERVER_URL} first.")),
    }
}

/// `GET /version`, then (with a token) Trackio's read-only write-access check.
async fn preflight(base: &str, token: Option<&str>) -> Preflight {
    let client = match client(base) {
        Ok(client) => client,
        Err(error) => return Preflight::failed(error),
    };
    let version = match version(&client, base, None).await {
        Ok(version) => version,
        Err(error) => return Preflight::failed(error),
    };
    let reachable = |write_access, error| Preflight {
        reachable: true,
        version: Some(version.clone()),
        write_access,
        error,
        space_stage: None,
    };
    let Some(token) = token else {
        return reachable(None, None);
    };
    let check = async {
        let res = client
            .post(format!("{base}/api/get_run_mutation_status"))
            .header("X-Trackio-Write-Token", token)
            .json(&json!({}))
            .send()
            .await
            .map_err(|error| error.without_url().to_string())?;
        if !res.status().is_success() {
            return Err(format!("it answered {}", res.status()));
        }
        res.json::<Value>()
            .await
            .ok()
            .and_then(|body| body["data"]["allowed"].as_bool())
            .ok_or_else(|| "its answer had no `allowed` field".to_string())
    };
    match check.await {
        Ok(allowed) => reachable(Some(allowed), None),
        Err(error) => reachable(
            None,
            Some(format!("Could not check the write token: {error}")),
        ),
    }
}

#[derive(Clone)]
struct Space {
    repo: String,
    host: String,
    stage: Option<String>,
    private: bool,
}

/// `owner/name`; a bare name is the token owner's, as Trackio expands it.
async fn space_repo(id: &str, token: Option<&str>) -> Result<String, String> {
    let id = id.trim();
    let valid = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match id.split_once('/') {
        Some((owner, name)) if valid(owner) && valid(name) => Ok(id.to_string()),
        None if valid(id) => {
            let token = token.ok_or_else(|| {
                format!("{SPACE_ID} has no owner and no Hugging Face token is set; use owner/name")
            })?;
            let owner = crate::jobs::huggingface::whoami(token)
                .await
                .map_err(|error| error.to_string())?;
            Ok(format!("{owner}/{id}"))
        }
        _ => Err(format!("{SPACE_ID} must look like owner/name, not {id:?}")),
    }
}

/// Only an `https://*.hf.space` host may receive the Hugging Face token.
fn space_host(raw: &str) -> Option<String> {
    let url = reqwest::Url::parse(raw).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https" && host.ends_with(".hf.space")).then(|| format!("https://{host}"))
}

/// `None` when the Space does not exist yet (or is private and no token can see it).
async fn space(id: &str, token: Option<&str>) -> Result<Option<Space>, String> {
    let repo = space_repo(id, token).await?;
    let api = hf_api();
    let mut req = client(&api)?.get(format!("{api}/spaces/{repo}"));
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let res = req
        .send()
        .await
        .map_err(|error| format!("Could not reach Hugging Face: {}", error.without_url()))?;
    match res.status().as_u16() {
        404 => return Ok(None),
        401 if token.is_none() => return Ok(None),
        401 => return Err("Hugging Face rejected HF_TOKEN; save a current token.".to_string()),
        300..=399 => {
            return Err(format!(
                "Space {repo} moved on Hugging Face; update {SPACE_ID}."
            ))
        }
        _ if !res.status().is_success() => {
            return Err(format!(
                "Hugging Face answered {} for Space {repo}",
                res.status()
            ))
        }
        _ => {}
    }
    let body: Value = res
        .json()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let host = body["host"]
        .as_str()
        .and_then(space_host)
        .ok_or_else(|| format!("Space {repo} has no hf.space address"))?;
    Ok(Some(Space {
        repo,
        host,
        stage: body["runtime"]["stage"].as_str().map(str::to_string),
        private: body["private"].as_bool().unwrap_or(false),
    }))
}

/// Caches are keyed by which token answered, so saving another account's token starts afresh.
fn token_key(token: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut hasher);
    hasher.finish()
}

/// Long enough that dashboard polls never reach `whoami`, which Hugging Face rate-limits hard.
const SPACE_REUSE: Duration = Duration::from_secs(60);
/// Shorter than a poll, so a Space created by the first run shows up on the next check.
const MISSING_SPACE_REUSE: Duration = Duration::from_secs(10);

/// Failures are cached too, so a rejected token does not reach `whoami` on every poll.
async fn cached_space(id: &str, token: Option<&str>) -> Result<Option<Space>, String> {
    type Spaces = Mutex<HashMap<(String, u64), (Result<Option<Space>, String>, Instant)>>;
    static SPACES: OnceLock<Spaces> = OnceLock::new();
    let key = (id.to_string(), token_key(token));
    let spaces = SPACES.get_or_init(Mutex::default);
    if let Some((space, at)) = spaces.lock().unwrap().get(&key) {
        let reuse = if matches!(space, Ok(None)) {
            MISSING_SPACE_REUSE
        } else {
            SPACE_REUSE
        };
        if at.elapsed() < reuse {
            return space.clone();
        }
    }
    let space = space(id, token).await;
    spaces
        .lock()
        .unwrap()
        .insert(key, (space.clone(), Instant::now()));
    space
}

/// Refetched well before Hugging Face's 24-hour expiry, and stable in between so an open embed never reloads.
const SPACE_SIGN_REUSE: Duration = Duration::from_secs(12 * 3600);

/// Hugging Face's read-only JWT for one Space: a private Space's browser URLs need it as `__sign`,
/// as the Space page on huggingface.co does, or they 404 even for a signed-in owner.
async fn space_sign(repo: &str, token: &str) -> Result<String, String> {
    type Signs = Mutex<HashMap<(String, u64), (String, Instant)>>;
    static SIGNS: OnceLock<Signs> = OnceLock::new();
    let key = (repo.to_string(), token_key(Some(token)));
    let signs = SIGNS.get_or_init(Mutex::default);
    if let Some((sign, at)) = signs.lock().unwrap().get(&key) {
        if at.elapsed() < SPACE_SIGN_REUSE {
            return Ok(sign.clone());
        }
    }
    let api = hf_api();
    let res = client(&api)?
        .get(format!("{api}/spaces/{repo}/jwt"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| format!("Could not reach Hugging Face: {}", error.without_url()))?;
    if !res.status().is_success() {
        return Err(format!(
            "Hugging Face answered {} when signing Space {repo}'s dashboard link",
            res.status()
        ));
    }
    let body: Value = res
        .json()
        .await
        .map_err(|error| error.without_url().to_string())?;
    let sign = body["token"]
        .as_str()
        .ok_or_else(|| format!("Hugging Face did not sign Space {repo}'s dashboard link"))?
        .to_string();
    signs
        .lock()
        .unwrap()
        .insert(key, (sign.clone(), Instant::now()));
    Ok(sign)
}

async fn space_preflight(id: &str) -> Preflight {
    let token = crate::jobs::huggingface::resolve_token().ok();
    let space = match space(id, token.as_deref()).await {
        Ok(Some(space)) => space,
        Ok(None) => {
            return Preflight::failed(format!(
                "Space {id} does not exist yet, or HF_TOKEN cannot see it. Trackio creates it on the first logged run."
            ))
        }
        Err(error) => return Preflight::failed(error),
    };
    let write_access = match token.as_deref() {
        Some(token) => space_write_access(&space.repo, token).await,
        None => None,
    };
    let bearer = token.as_deref().filter(|_| space.private);
    let (version, error) = match client(&space.host) {
        Ok(client) => match version(&client, &space.host, bearer).await {
            Ok(version) => (Some(version), None),
            Err(error) => (None, Some(error)),
        },
        Err(error) => (None, Some(error)),
    };
    Preflight {
        reachable: version.is_some(),
        version,
        write_access,
        error,
        space_stage: space.stage,
    }
}

/// Hugging Face's own write check, the one Trackio runs before logging to a Space.
async fn space_write_access(repo: &str, token: &str) -> Option<bool> {
    let api = hf_api();
    let res = client(&api)
        .ok()?
        .get(format!("{api}/spaces/{repo}/auth-check/write"))
        .bearer_auth(token)
        .send()
        .await
        .ok()?;
    match res.status().as_u16() {
        200..=299 => Some(true),
        401 | 403 => Some(false),
        _ => None,
    }
}

/// One of Trackio's dashboard API calls (`POST /api/<name>`), returning its `data`.
async fn call(base: &str, bearer: Option<&str>, name: &str, body: Value) -> Result<Value, String> {
    let mut req = client(base)?.post(format!("{base}/api/{name}")).json(&body);
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    let res = req
        .send()
        .await
        .map_err(|error| format!("Could not reach {base}: {}", error.without_url()))?;
    let status = res.status();
    let body: Value = res.json().await.unwrap_or_default();
    if !status.is_success() {
        let detail = body["error"]
            .as_str()
            .map(|e| format!(": {e}"))
            .unwrap_or_default();
        return Err(format!("{base} answered {status} to {name}{detail}"));
    }
    match body.get("data") {
        Some(data) => Ok(data.clone()),
        None => Err(format!("{base} sent no data for {name}")),
    }
}

async fn run_names(base: &str, project: &str, bearer: Option<&str>) -> Result<Vec<String>, String> {
    let data = call(
        base,
        bearer,
        "get_runs_for_project",
        json!({ "project": project }),
    )
    .await?;
    let runs = data
        .as_array()
        .ok_or_else(|| format!("{base} did not list the project's runs"))?;
    Ok(runs
        .iter()
        .flat_map(|run| [run["id"].as_str(), run["name"].as_str()])
        .flatten()
        .map(str::to_string)
        .collect())
}

struct Dashboard {
    base: String,
    /// The HF token, only for a private Space; a public Space's code never sees it.
    bearer: Option<String>,
    /// A private Space's `owner/name`, which its browser links are signed for.
    private_repo: Option<String>,
}

/// The dashboard `project` logs to; `None` when nothing has logged yet (no local data, or no Space yet).
async fn dashboard(local: &Arc<LocalServer>, project: &str) -> Result<Option<Dashboard>, String> {
    match target() {
        Target::Space(id) => {
            let token = crate::jobs::huggingface::resolve_token().ok();
            // Not created yet is the normal first-use state, not a failure.
            let Some(space) = cached_space(&id, token.as_deref()).await? else {
                return Ok(None);
            };
            Ok(Some(Dashboard {
                base: space.host,
                bearer: token.filter(|_| space.private),
                private_repo: space.private.then_some(space.repo),
            }))
        }
        Target::Server(base) => Ok(Some(Dashboard {
            base,
            bearer: None,
            private_repo: None,
        })),
        Target::Local if has_local_project(project) => Ok(Some(Dashboard {
            base: local.url().await?,
            bearer: None,
            private_repo: None,
        })),
        Target::Local => Ok(None),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRuns {
    /// `None` until something has logged (to the local store, or creating the Space).
    pub dashboard_url: Option<String>,
    pub project: String,
    /// Run ids and names that have logged, matched against `ORX_RUN_ID`.
    pub runs: Vec<String>,
    /// A private Space's `__sign` for browser URLs: read-only, for that Space only.
    pub dashboard_sign: Option<String>,
}

/// The dashboard for `project` and which runs have logged to it.
pub async fn project_runs(
    local: &Arc<LocalServer>,
    project: String,
) -> Result<ProjectRuns, String> {
    let Some(dashboard) = dashboard(local, &project).await? else {
        return Ok(ProjectRuns {
            dashboard_url: None,
            project,
            runs: Vec::new(),
            dashboard_sign: None,
        });
    };
    let dashboard_sign = match (&dashboard.private_repo, dashboard.bearer.as_deref()) {
        (Some(repo), Some(token)) => Some(space_sign(repo, token).await?),
        _ => None,
    };
    let runs = run_names(&dashboard.base, &project, dashboard.bearer.as_deref()).await?;
    Ok(ProjectRuns {
        dashboard_url: Some(dashboard.base),
        project,
        runs,
        dashboard_sign,
    })
}

/// How many evenly spaced points the statistics are computed over.
const STATS_POINTS: usize = 2000;

#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub step: i64,
    pub value: f64,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MetricSummary {
    pub name: String,
    /// Points the statistics cover; at most `STATS_POINTS` of the run's logged steps.
    pub points: usize,
    pub first: Point,
    pub last: Point,
    pub min: Point,
    pub max: Point,
    /// Mean over the last tenth of the points: the value the run settled at.
    pub tail_mean: f64,
    /// Evenly spaced points, first and last included, for reading the curve's shape.
    pub series: Vec<Point>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMetrics {
    pub run: String,
    pub project: String,
    /// `None` until something has logged (to the local store, or creating the Space).
    pub dashboard_url: Option<String>,
    /// Steps the run logged; `None` when it never logged to Trackio.
    pub logged_steps: Option<u64>,
    pub last_step: Option<i64>,
    pub group: Option<String>,
    /// The run's `config`, without Trackio's own `_`-prefixed fields.
    pub config: Option<serde_json::Map<String, Value>>,
    pub alerts: Vec<Value>,
    pub metrics: Vec<MetricSummary>,
    /// Metrics with no numeric values, such as images or tables.
    pub other_metrics: Vec<String>,
    /// What could not be read; the rest of the report still stands.
    pub errors: Vec<String>,
}

/// `rows` as Trackio's `get_metric_values` returns them; `None` when none is numeric.
fn summarize(name: &str, rows: &[Value], series_points: usize) -> Option<MetricSummary> {
    let values: Vec<Point> = rows
        .iter()
        .filter_map(|row| {
            let value = row["value"].as_f64().filter(|value| value.is_finite())?;
            let step = row["step"]
                .as_i64()
                .or_else(|| row["step"].as_f64().map(|step| step as i64))?;
            Some(Point { step, value })
        })
        .collect();
    let first = *values.first()?;
    let last = *values.last()?;
    let by_value = |a: &&Point, b: &&Point| a.value.total_cmp(&b.value);
    let min = *values.iter().min_by(by_value)?;
    let max = *values.iter().max_by(by_value)?;
    let tail = &values[values.len() - values.len().div_ceil(10)..];
    let tail_mean = tail.iter().map(|point| point.value).sum::<f64>() / tail.len() as f64;
    let series = if values.len() <= series_points.max(2) {
        values.clone()
    } else {
        let end = values.len() - 1;
        let picks = series_points.max(2) - 1;
        let mut indices: Vec<usize> = (0..=picks).map(|i| (i * end + picks / 2) / picks).collect();
        indices.dedup();
        indices.into_iter().map(|i| values[i]).collect()
    };
    Some(MetricSummary {
        name: name.to_string(),
        points: values.len(),
        first,
        last,
        min,
        max,
        tail_mean,
        series,
    })
}

/// What `run` logged to Trackio, summarized for an agent to read.
pub async fn run_metrics(
    local: &Arc<LocalServer>,
    project: String,
    run: String,
    filters: &[String],
    series_points: usize,
) -> RunMetrics {
    let mut out = RunMetrics {
        run,
        project,
        dashboard_url: None,
        logged_steps: None,
        last_step: None,
        group: None,
        config: None,
        alerts: Vec::new(),
        metrics: Vec::new(),
        other_metrics: Vec::new(),
        errors: Vec::new(),
    };
    let dashboard = match dashboard(local, &out.project).await {
        Ok(Some(dashboard)) => dashboard,
        Ok(None) => return out,
        Err(error) => {
            out.errors.push(error);
            return out;
        }
    };
    let (base, bearer) = (dashboard.base.as_str(), dashboard.bearer.as_deref());
    out.dashboard_url = Some(base.to_string());
    let target = json!({ "project": out.project, "run": out.run });
    let summary = match call(base, bearer, "get_run_summary", target.clone()).await {
        Ok(summary) => summary,
        Err(error) => {
            out.errors.push(error);
            return out;
        }
    };
    let logged = summary["num_logs"].as_u64().unwrap_or(0);
    if logged == 0 {
        return out;
    }
    out.logged_steps = Some(logged);
    out.last_step = summary["last_step"].as_i64();
    if let Some(config) = summary["config"].as_object() {
        out.group = config
            .get("_Group")
            .and_then(Value::as_str)
            .map(str::to_string);
        out.config = Some(
            config
                .iter()
                .filter(|(key, _)| !key.starts_with('_'))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
    }
    match call(base, bearer, "get_alerts", target.clone()).await {
        Ok(alerts) => out.alerts = alerts.as_array().cloned().unwrap_or_default(),
        Err(error) => out.errors.push(format!("alerts: {error}")),
    }
    let names = summary["metrics"].as_array().cloned().unwrap_or_default();
    let wanted = names.iter().filter_map(Value::as_str).filter(|name| {
        filters.is_empty()
            || filters
                .iter()
                .any(|filter| name.to_lowercase().contains(&filter.to_lowercase()))
    });
    for name in wanted {
        let rows = call(
            base,
            bearer,
            "get_metric_values",
            json!({ "project": out.project, "run": out.run, "metric_name": name, "max_points": STATS_POINTS }),
        )
        .await;
        match rows {
            Ok(rows) => match summarize(
                name,
                rows.as_array().map(Vec::as_slice).unwrap_or_default(),
                series_points,
            ) {
                Some(summary) => out.metrics.push(summary),
                None => out.other_metrics.push(name.to_string()),
            },
            Err(error) => out.errors.push(format!("{name}: {error}")),
        }
    }
    out
}

/// The `trackio show` that `orx up` starts on first use, so local runs need no setup.
#[derive(Default)]
pub struct LocalServer {
    state: Mutex<LocalState>,
}

type Started = Option<Result<String, String>>;

#[derive(Default)]
enum LocalState {
    #[default]
    Idle,
    Starting(tokio::sync::watch::Receiver<Started>),
    /// Boxed: a Windows `Child` dwarfs the other variants.
    Running {
        child: Box<LocalChild>,
        url: String,
    },
    /// Keeps a missing install from respawning on every dashboard poll.
    Failed {
        at: Instant,
        error: String,
    },
}

/// `uvx` runs Trackio in a child of its own, so stopping takes the whole process group.
struct LocalChild(tokio::process::Child);

impl Drop for LocalChild {
    fn drop(&mut self) {
        crate::local::chat::kill_shell_group(self.0.id());
    }
}

const LOCAL_RETRY_AFTER: Duration = Duration::from_secs(60);
/// `uvx` may download Trackio on the first start.
const LOCAL_START_TIMEOUT: Duration = Duration::from_secs(180);

impl LocalServer {
    /// Starting runs in its own task, so a caller that gives up neither kills it nor blocks the others.
    pub async fn url(self: &Arc<Self>) -> Result<String, String> {
        let mut started = {
            let mut state = self.state.lock().unwrap();
            if let LocalState::Running { child, url } = &mut *state {
                if matches!(child.0.try_wait(), Ok(None)) {
                    return Ok(url.clone());
                }
            }
            match &*state {
                LocalState::Failed { at, error } if at.elapsed() < LOCAL_RETRY_AFTER => {
                    return Err(error.clone())
                }
                LocalState::Starting(started) => started.clone(),
                _ => {
                    let (done, started) = tokio::sync::watch::channel(None);
                    *state = LocalState::Starting(started.clone());
                    let server = Arc::clone(self);
                    tokio::spawn(async move {
                        let result = start_local().await;
                        let outcome = result
                            .as_ref()
                            .map(|(_, url)| url.clone())
                            .map_err(String::clone);
                        *server.state.lock().unwrap() = match result {
                            Ok((child, url)) => LocalState::Running {
                                child: Box::new(child),
                                url,
                            },
                            Err(error) => LocalState::Failed {
                                at: Instant::now(),
                                error,
                            },
                        };
                        let _ = done.send(Some(outcome));
                    });
                    started
                }
            }
        };
        let outcome = started
            .wait_for(Option::is_some)
            .await
            .ok()
            .and_then(|outcome| outcome.clone());
        outcome.unwrap_or_else(|| Err("Trackio's dashboard stopped while starting".to_string()))
    }
}

/// Exits when its stdin closes, so the dashboard dies with orx however orx ends (restart, crash, kill).
const SHOW_SCRIPT: &str = "import os, sys, threading, trackio; threading.Thread(target=lambda: (sys.stdin.read(), os._exit(0)), daemon=True).start(); trackio.show(open_browser=False, block_thread=True, footer=False, server_port=int(sys.argv[1]))";

/// Windows' Store stub for a missing `python`.
const WINDOWS_STUB_EXIT: i32 = 9009;

async fn start_local() -> Result<(LocalChild, String), String> {
    use crate::local::shell_env::find_on_path;
    let install = "Install uv, or run `pip install trackio`, to see local runs in Trackio.";
    let dir = local_dir().ok_or("No home directory for Trackio's local store")?;
    let port = crate::local::opencode::free_port()
        .map_err(|error| error.to_string())?
        .to_string();
    let mut cmd = if let Some(uvx) = find_on_path("uvx") {
        let mut cmd = tokio::process::Command::new(uvx);
        cmd.args([
            "--from",
            LOCAL_REQUIREMENT,
            "python",
            "-c",
            SHOW_SCRIPT,
            &port,
        ]);
        cmd
    } else if let Some(python) = find_on_path(if cfg!(windows) { "python" } else { "python3" }) {
        let mut cmd = tokio::process::Command::new(python);
        cmd.args(["-c", SHOW_SCRIPT, &port]);
        cmd
    } else {
        return Err(install.to_string());
    };
    let log_path = crate::store::data_dir().join("trackio.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| error.to_string())?;
    cmd.envs(crate::config::list_synced_env())
        .env(DIR, &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(
            log.try_clone().map_err(|error| error.to_string())?,
        ))
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = LocalChild(
        cmd.spawn()
            .map_err(|error| format!("Could not start Trackio: {error}"))?,
    );
    let url = format!("http://127.0.0.1:{port}");
    let client = client(&url)?;
    let deadline = Instant::now() + LOCAL_START_TIMEOUT;
    loop {
        if let Ok(Some(status)) = child.0.try_wait() {
            if status.code() == Some(WINDOWS_STUB_EXIT) {
                return Err(install.to_string());
            }
            return Err(format!(
                "Trackio exited ({status}) before its dashboard started; see {}",
                log_path.display()
            ));
        }
        if version(&client, &url, None).await.is_ok() {
            return Ok((child, url));
        }
        if Instant::now() > deadline {
            return Err(format!(
                "Trackio's dashboard did not start within {}s; see {}",
                LOCAL_START_TIMEOUT.as_secs(),
                log_path.display()
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_drops_tokens_and_credentials() {
        assert_eq!(
            base_url(" https://user:pw@trackio.example:7860/?write_token=secret#x ").as_deref(),
            Some("https://trackio.example:7860")
        );
        assert_eq!(
            base_url("http://10.0.0.5:7860/trackio/").as_deref(),
            Some("http://10.0.0.5:7860/trackio")
        );
        assert_eq!(base_url("localhost:7860"), None);
        assert_eq!(
            plain_value(SERVER_URL, "http://h:7860/?write_token=secret").as_deref(),
            Some("http://h:7860")
        );
        assert_eq!(plain_value(WRITE_TOKEN, "secret"), None);
        assert_eq!(
            plain_value(SERVER_URL, "localhost:7860").as_deref(),
            Some("localhost:7860")
        );
        assert_eq!(plain_value(SERVER_URL, "h:7860?write_token=secret"), None);
        assert_eq!(base_url("ftp://trackio.example"), None);
        assert_eq!(
            plain_value(SPACE_ID, "user/trackio").as_deref(),
            Some("user/trackio")
        );
    }

    #[test]
    fn project_names_match_trackio_files() {
        assert_eq!(canonical_project_name("my.model v2"), "mymodelv2");
        assert_eq!(canonical_project_name("nano-chat_1"), "nano-chat_1");
        assert_eq!(canonical_project_name("..."), "default");
    }

    #[test]
    fn only_hf_space_hosts_get_the_token() {
        assert_eq!(
            space_host("https://user-trackio.hf.space").as_deref(),
            Some("https://user-trackio.hf.space")
        );
        for host in [
            "http://user-trackio.hf.space",
            "https://hf.space.evil.example",
            "https://evil.example/.hf.space",
        ] {
            assert_eq!(space_host(host), None, "{host}");
        }
    }

    #[tokio::test]
    async fn space_ids_are_validated_before_any_request() {
        assert_eq!(
            space_repo(" user/trackio ", None).await.as_deref(),
            Ok("user/trackio")
        );
        for id in ["user/../x/y", "a/b/c", "user/", "../spaces"] {
            assert!(space_repo(id, None).await.is_err(), "{id}");
        }
        assert!(space_repo("trackio", None).await.is_err());
    }

    #[test]
    fn summaries_keep_extremes_and_the_curve_ends() {
        let rows: Vec<Value> = (0..100)
            .map(|step| json!({ "step": step, "value": if step == 40 { 9.0 } else { 1.0 / f64::from(step + 1) } }))
            .chain([json!({ "step": 100, "value": "image" })])
            .collect();
        let summary = summarize("loss", &rows, 5).unwrap();
        assert_eq!(summary.points, 100);
        assert_eq!(
            summary.first,
            Point {
                step: 0,
                value: 1.0
            }
        );
        assert_eq!(summary.last.step, 99);
        assert_eq!(
            summary.max,
            Point {
                step: 40,
                value: 9.0
            }
        );
        assert_eq!(summary.min.step, 99);
        let steps: Vec<i64> = summary.series.iter().map(|point| point.step).collect();
        assert_eq!(steps, [0, 25, 50, 74, 99]);
        assert!(
            (summary.tail_mean - (90..100).map(|s| 1.0 / f64::from(s + 1)).sum::<f64>() / 10.0)
                .abs()
                < 1e-12
        );
        assert!(summarize("media", &[json!({ "step": 0, "value": "image" })], 5).is_none());
    }

    #[tokio::test]
    async fn run_names_lists_ids_and_names() {
        use axum::routing::post;
        let runs = |axum::Json(body): axum::Json<Value>| async move {
            assert_eq!(body["project"], "p");
            axum::Json(
                json!({ "data": [{ "id": "r1", "name": "run-a" }, { "id": null, "name": "r2" }] }),
            )
        };
        let base = stub(axum::Router::new().route("/api/get_runs_for_project", post(runs))).await;
        assert_eq!(
            run_names(&base, "p", None).await.unwrap(),
            ["r1", "run-a", "r2"]
        );
    }

    #[test]
    fn local_only_addresses() {
        for url in [
            "http://localhost:7860",
            "http://127.0.0.1:7860",
            "http://0.0.0.0:7860",
            "http://[::]:7860",
            "http://[::1]:7860",
        ] {
            assert!(is_local_only(url), "{url}");
        }
        for url in ["http://10.0.0.5:7860", "https://trackio.example"] {
            assert!(!is_local_only(url), "{url}");
        }
    }

    async fn stub(write_check: axum::Router) -> String {
        use axum::routing::get;
        let app = write_check.route(
            "/version",
            get(|| async { axum::Json(json!({ "version": "0.42.0" })) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        base
    }

    #[tokio::test]
    async fn preflight_reports_the_write_check() {
        use axum::http::{HeaderMap, StatusCode};
        use axum::routing::post;
        let allowed = |headers: HeaderMap| async move {
            let ok = headers
                .get("x-trackio-write-token")
                .is_some_and(|t| t == "right");
            axum::Json(json!({ "data": { "allowed": ok } }))
        };
        let base =
            stub(axum::Router::new().route("/api/get_run_mutation_status", post(allowed))).await;
        let accepted = preflight(&base, Some("right")).await;
        assert_eq!(
            (accepted.reachable, accepted.write_access),
            (true, Some(true))
        );
        assert_eq!(accepted.version.as_deref(), Some("0.42.0"));
        assert_eq!(
            preflight(&base, Some("wrong")).await.write_access,
            Some(false)
        );
        let unchecked = preflight(&base, None).await;
        assert_eq!((unchecked.write_access, unchecked.error), (None, None));

        let silent = || async { axum::Json(json!({})) };
        let base =
            stub(axum::Router::new().route("/api/get_run_mutation_status", post(silent))).await;
        let silent = preflight(&base, Some("right")).await;
        assert!(silent.error.is_some_and(|e| e.contains("allowed")));

        let failing = || async { StatusCode::INTERNAL_SERVER_ERROR };
        let base =
            stub(axum::Router::new().route("/api/get_run_mutation_status", post(failing))).await;
        let failed = preflight(&base, Some("right")).await;
        assert!(failed.reachable);
        assert_eq!(failed.write_access, None);
        assert!(failed.error.is_some_and(|e| e.contains("500")));
    }
}
