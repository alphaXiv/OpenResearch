//! Decision model: a small OpenAI model that picks where work runs, so the
//! harnesses' own quota goes further.
//!
//! The model sees what orx already knows (each harness account's quota per
//! pool, which harnesses are ready, which compute targets are configured)
//! together with a description of the work, and answers with a route: the
//! harness, model, account, and compute target to use, each one of the
//! options it was given or `null`. It is called through the Responses API
//! with a strict JSON schema, so the answer is always parseable.
//!
//! The decision model only advises. Every caller keeps its own choice and
//! uses it whenever the decision model is off, has no key, fails, runs out
//! of quota, times out, or answers with something that was not offered.
//!
//! The key and model live in `decision-model.json` under the orx data dir,
//! readable by the user only, and never in the synced Environment file:
//! that file reaches every harness and run, where an `OPENAI_API_KEY` would
//! switch Codex to API billing.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{anyhow, Result};

/// OpenAI's most efficient model for focused, high-volume tasks, and the
/// only one that can skip reasoning, which keeps a routing call fast.
pub const DEFAULT_MODEL: &str = "gpt-6-luna";

/// Overrides the stored key, for testing without saving one.
pub const API_KEY_ENV: &str = "ORX_DECISION_OPENAI_API_KEY";

const API_BASE: &str = "https://api.openai.com/v1";
const TIMEOUT: Duration = Duration::from_secs(12);
/// How long to stop asking after OpenAI refused the key or reported the
/// account out of quota, so each turn does not wait on a call that fails.
const COOLDOWN: Duration = Duration::from_secs(15 * 60);
/// Longest work description sent, in characters.
const TASK_CHARS: usize = 4000;
const MAX_MODELS: usize = 16;

static WRITE_LOCK: Mutex<()> = Mutex::new(());
static PAUSED_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

// --- settings ---------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Stored {
    #[serde(default)]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    api_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeySource {
    Env,
    Settings,
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub enabled: bool,
    pub model: String,
    key: Option<(String, KeySource)>,
}

impl Settings {
    pub fn key_source(&self) -> Option<KeySource> {
        self.key.as_ref().map(|(_, source)| *source)
    }

    pub fn masked_key(&self) -> Option<String> {
        self.key.as_ref().map(|(key, _)| mask(key))
    }

    /// On, with a key to call with.
    pub fn active(&self) -> bool {
        self.enabled && self.key.is_some()
    }
}

fn path() -> PathBuf {
    crate::store::data_dir().join("decision-model.json")
}

fn read_stored() -> Stored {
    std::fs::read(path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_stored(stored: &Stored) -> Result<()> {
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(stored)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

pub fn settings() -> Settings {
    let stored = read_stored();
    let env = std::env::var(API_KEY_ENV)
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    let key = env.map(|key| (key, KeySource::Env)).or_else(|| {
        let key = stored.api_key.trim();
        (!key.is_empty()).then(|| (key.to_string(), KeySource::Settings))
    });
    Settings {
        enabled: stored.enabled,
        model: stored
            .model
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        key,
    }
}

/// A change to the stored settings; `None` leaves a field as it is, and an
/// empty `api_key` removes the stored key.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
}

pub fn save(patch: SettingsPatch) -> Result<Settings> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut stored = read_stored();
    if let Some(enabled) = patch.enabled {
        stored.enabled = enabled;
    }
    if let Some(model) = patch.model {
        let model = model.trim();
        if !model.is_empty() && !valid_model_id(model) {
            return Err(anyhow!("`{model}` is not an OpenAI model ID."));
        }
        stored.model = (!model.is_empty()).then(|| model.to_string());
    }
    if let Some(key) = patch.api_key {
        stored.api_key = key.trim().to_string();
    }
    write_stored(&stored)?;
    // A new key or model deserves a fresh try.
    resume();
    Ok(settings())
}

fn valid_model_id(model: &str) -> bool {
    model.len() <= 64
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | ':'))
}

fn mask(key: &str) -> String {
    let tail: String = key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let head: String = key.chars().take(3).collect();
    format!("{head}…{tail}")
}

fn paused() -> bool {
    let mut until = PAUSED_UNTIL.lock().unwrap_or_else(|e| e.into_inner());
    match *until {
        Some(at) if at > Instant::now() => true,
        Some(_) => {
            *until = None;
            false
        }
        None => false,
    }
}

fn pause() {
    *PAUSED_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() + COOLDOWN);
}

fn resume() {
    *PAUSED_UNTIL.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// --- checking a key ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyStatus {
    Missing,
    Valid,
    Invalid,
    /// The key works but cannot use the chosen model.
    ModelUnavailable,
}

/// Whether `key` can call `model`: one metadata request, no tokens spent.
pub async fn check_key(key: &str, model: &str) -> Result<KeyStatus> {
    let response = client()?
        .get(format!("{API_BASE}/models/{model}"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| anyhow!("Could not reach OpenAI to check the key. Try again."))?;
    match response.status().as_u16() {
        200 => Ok(KeyStatus::Valid),
        401 => Ok(KeyStatus::Invalid),
        403 | 404 => Ok(KeyStatus::ModelUnavailable),
        code => Err(anyhow!(
            "Could not check the OpenAI key (HTTP {code}). Try again."
        )),
    }
}

pub async fn current_key_status(settings: &Settings) -> Result<KeyStatus> {
    match &settings.key {
        Some((key, _)) => check_key(key, &settings.model).await,
        None => Ok(KeyStatus::Missing),
    }
}

fn client() -> Result<reqwest::Client> {
    Ok(crate::net::remote_client()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

// --- the question -----------------------------------------------------------------

/// What the route is for, which changes what a good answer looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Purpose {
    /// The next turn of a chat whose harness and model the user chose:
    /// only the account is open.
    Turn,
    /// A turn stopped on a usage limit: pick the harness to continue on.
    Failover,
    /// Work not started yet: harness, model, account, and compute are open.
    Plan,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Current {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessOption {
    pub id: String,
    pub vendor: String,
    pub models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended_model: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOption {
    pub harness: String,
    pub account: String,
    pub pool: String,
    pub pool_label: String,
    pub state: String,
    /// Share of quota left, 0–1, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    /// Minutes until the tightest provider window resets, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_in_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked_for_minutes: Option<i64>,
    pub turns_last_24h: u64,
}

impl AccountOption {
    pub fn from_status(
        harness: &str,
        account: &str,
        pool: &crate::local::harness::accounts::PoolStatus,
        now: i64,
    ) -> Self {
        let minutes = |at: i64| ((at - now).max(0) + 59_999) / 60_000;
        Self {
            harness: harness.to_string(),
            account: account.to_string(),
            pool: pool.pool.to_string(),
            pool_label: pool.label.to_string(),
            state: format!("{:?}", pool.state).to_lowercase(),
            remaining: pool.remaining.map(|left| (left * 100.0).round() / 100.0),
            resets_in_minutes: pool
                .windows
                .iter()
                .filter_map(|window| window.resets_at)
                .filter(|at| *at > now)
                .min()
                .map(minutes),
            blocked_for_minutes: pool.blocked_until.map(minutes),
            turns_last_24h: pool.tracked_turns,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputeOption {
    pub id: String,
    pub summary: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRequest {
    pub purpose: Purpose,
    pub task: String,
    pub current: Current,
    /// Ready harnesses the route may pick; empty when the harness is fixed.
    pub harnesses: Vec<HarnessOption>,
    /// Signed-in, unpaused accounts with their quota per pool.
    pub accounts: Vec<AccountOption>,
    /// Configured compute targets; empty when compute is not in question.
    pub compute: Vec<ComputeOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_gpu: Option<String>,
}

impl RouteRequest {
    pub fn new(purpose: Purpose, task: &str) -> Self {
        Self {
            purpose,
            task: truncate(task.trim(), TASK_CHARS),
            current: Current::default(),
            harnesses: Vec::new(),
            accounts: Vec::new(),
            compute: Vec::new(),
            default_gpu: None,
        }
    }
}

fn truncate(text: &str, chars: usize) -> String {
    match text.char_indices().nth(chars) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The decision model's answer, reduced to what was actually offered.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub compute: Option<String>,
    pub gpu: Option<String>,
    pub reason: String,
}

const INSTRUCTIONS: &str = "You route work for OpenResearch, a research tool that runs coding agents (harnesses) on the user's subscription accounts and runs experiments on compute targets. Your goal is to finish the work while spending the scarcest quota last.

Rules:
- Answer only with options listed in the input; use null for anything not offered, not needed, or when no option fits.
- Never pick an account whose state is \"blocked\". Prefer \"ok\" over \"low\"; \"unknown\" means nobody reported the quota.
- An account's pool must match the model: Antigravity meters Gemini models (pool \"gemini\") apart from its Claude and GPT models (pool \"partner\").
- Spend quota that is about to reset first: between accounts with room, prefer the one whose window resets sooner, since unused quota is lost at the reset.
- Size the choice to the work: short or simple tasks (questions, small edits, summaries) go to cheaper models and accounts with less room; long or hard tasks (multi-file changes, research, debugging) need a strong model and an account with plenty of room.
- For a \"turn\", keep the current account unless it is low or blocked or another account clearly saves quota; switching mid-chat costs context.
- For a \"failover\", pick the harness most likely to finish the task with room left.
- Pick compute only when the work runs code or experiments: \"local\" for light CPU work, a GPU target for training or large inference. Suggest a GPU type in gpu only when a GPU target is picked; otherwise null.
- reason: one short sentence a user can read.";

fn schema() -> Value {
    let nullable =
        |description: &str| json!({ "type": ["string", "null"], "description": description });
    json!({
        "type": "object",
        "properties": {
            "harness": nullable("Harness id from the input, or null."),
            "model": nullable("Model id from the picked harness's models, or null for its default."),
            "account": nullable("Account id of the picked harness with room in the model's pool, or null."),
            "compute": nullable("Compute target id from the input, or null."),
            "gpu": nullable("GPU type for a GPU compute target, or null."),
            "reason": { "type": "string", "description": "One short sentence." },
        },
        "required": ["harness", "model", "account", "compute", "gpu", "reason"],
        "additionalProperties": false,
    })
}

/// The Responses API request for `request` on `model`.
fn request_body(model: &str, request: &RouteRequest) -> Value {
    let mut body = json!({
        "model": model,
        "instructions": INSTRUCTIONS,
        "input": [{
            "role": "user",
            "content": serde_json::to_string(request).unwrap_or_default(),
        }],
        "text": {
            "format": {
                "type": "json_schema",
                "name": "route",
                "schema": schema(),
                "strict": true,
            }
        },
        "max_output_tokens": 2000,
        "store": false,
    });
    // Luna answers a routing question without reasoning; other models keep
    // the lowest effort they all accept.
    body["reasoning"] = json!({ "effort": if model.contains("luna") { "none" } else { "low" } });
    body
}

/// The route in a Responses API reply, or why there is none.
fn parse_reply(reply: &Value) -> Result<Route> {
    if reply["status"] == "incomplete" {
        let why = reply
            .pointer("/incomplete_details/reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(anyhow!("the decision model stopped early ({why})"));
    }
    let content = reply["output"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten());
    for part in content {
        match part["type"].as_str() {
            Some("refusal") => return Err(anyhow!("the decision model declined to answer")),
            Some("output_text") => {
                let text = part["text"].as_str().unwrap_or_default();
                return serde_json::from_str(text)
                    .map_err(|e| anyhow!("the decision model's answer is not a route ({e})"));
            }
            _ => {}
        }
    }
    Err(anyhow!("the decision model returned no answer"))
}

/// `route` with every field that names something not offered set to `None`.
fn validate(mut route: Route, request: &RouteRequest) -> Route {
    use crate::local::harness::accounts::pool_for;
    let fixed_harness = request.harnesses.is_empty();
    if fixed_harness {
        route.harness = None;
        route.model = None;
    } else if !route
        .harness
        .as_ref()
        .is_some_and(|id| request.harnesses.iter().any(|option| &option.id == id))
    {
        route.harness = None;
    }
    let harness = route
        .harness
        .clone()
        .or_else(|| request.current.harness.clone());
    if let (Some(model), Some(harness)) = (&route.model, &route.harness) {
        let offered = request
            .harnesses
            .iter()
            .find(|option| &option.id == harness)
            .is_some_and(|option| option.models.contains(model));
        if !offered {
            route.model = None;
        }
    } else {
        route.model = None;
    }
    let model = route.model.clone().or_else(|| {
        if route.harness.is_some() && route.harness != request.current.harness {
            None
        } else {
            request.current.model.clone()
        }
    });
    let usable = |account: &String| {
        let Some(harness) = harness.as_deref() else {
            return false;
        };
        let pool = pool_for(harness, model.as_deref());
        request.accounts.iter().any(|option| {
            option.harness == harness
                && &option.account == account
                && option.pool == pool
                && option.state != "blocked"
        })
    };
    if !route.account.as_ref().is_some_and(usable) {
        route.account = None;
    }
    if !route
        .compute
        .as_ref()
        .is_some_and(|id| request.compute.iter().any(|option| &option.id == id))
    {
        route.compute = None;
    }
    if route.compute.is_none() || route.compute.as_deref() == Some("local") {
        route.gpu = None;
    }
    route.gpu = route
        .gpu
        .map(|gpu| truncate(gpu.trim(), 40))
        .filter(|gpu| !gpu.is_empty());
    route.reason = truncate(route.reason.trim(), 300);
    route
}

/// Ask the decision model for a route. `Ok(None)` when it is off, has no
/// key, or is paused after OpenAI refused it; `Err` when the call failed.
/// Either way the caller keeps its own choice.
pub async fn decide(request: &RouteRequest) -> Result<Option<Route>> {
    let settings = settings();
    if !settings.enabled || paused() {
        return Ok(None);
    }
    let Some((key, _)) = settings.key.as_ref() else {
        return Ok(None);
    };
    let response = client()?
        .post(format!("{API_BASE}/responses"))
        .bearer_auth(key)
        .json(&request_body(&settings.model, request))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                anyhow!("the decision model did not answer in time")
            } else {
                anyhow!("could not reach OpenAI")
            }
        })?;
    let status = response.status().as_u16();
    if status != 200 {
        let body: Value = response.json().await.unwrap_or(Value::Null);
        let code = body
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Err(refusal(status, &code));
    }
    let reply: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("OpenAI returned an unexpected response"))?;
    Ok(Some(validate(parse_reply(&reply)?, request)))
}

/// The error for a refused call, pausing the decision model when retrying
/// soon would fail the same way.
fn refusal(status: u16, code: &str) -> crate::error::Error {
    match (status, code) {
        (401, _) => {
            pause();
            anyhow!("OpenAI rejected the decision model's key")
        }
        (429, "insufficient_quota") => {
            pause();
            anyhow!("the OpenAI account behind the decision model is out of quota")
        }
        (429, _) => anyhow!("OpenAI rate-limited the decision model"),
        (403 | 404, _) => {
            pause();
            anyhow!("the decision model's key cannot use the chosen model")
        }
        (code, _) => anyhow!("OpenAI answered HTTP {code}"),
    }
}

/// `models`, capped so a harness with a long catalog does not crowd the
/// request.
pub fn model_ids(models: &[crate::local::harness::ModelInfo]) -> Vec<String> {
    models
        .iter()
        .take(MAX_MODELS)
        .map(|model| model.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(harness: &str, id: &str, pool: &str, state: &str) -> AccountOption {
        AccountOption {
            harness: harness.into(),
            account: id.into(),
            pool: pool.into(),
            pool_label: pool.into(),
            state: state.into(),
            remaining: None,
            resets_in_minutes: None,
            blocked_for_minutes: None,
            turns_last_24h: 0,
        }
    }

    fn plan() -> RouteRequest {
        let mut request = RouteRequest::new(Purpose::Plan, "Fine-tune a small model");
        request.harnesses = vec![
            HarnessOption {
                id: "antigravity".into(),
                vendor: "Google".into(),
                models: vec!["gemini-3-pro".into(), "claude-sonnet-4-6".into()],
                recommended_model: None,
            },
            HarnessOption {
                id: "codex".into(),
                vendor: "OpenAI".into(),
                models: vec!["gpt-6.1-sol".into()],
                recommended_model: None,
            },
        ];
        request.accounts = vec![
            account("antigravity", "default", "gemini", "ok"),
            account("antigravity", "default", "partner", "blocked"),
            account("antigravity", "work", "partner", "low"),
            account("codex", "default", "openai", "ok"),
        ];
        request.compute = vec![
            ComputeOption {
                id: "local".into(),
                summary: String::new(),
                is_default: true,
            },
            ComputeOption {
                id: "modal".into(),
                summary: String::new(),
                is_default: false,
            },
        ];
        request
    }

    fn route(harness: &str, model: &str, account: &str) -> Route {
        Route {
            harness: Some(harness.into()),
            model: Some(model.into()),
            account: Some(account.into()),
            compute: Some("modal".into()),
            gpu: Some("H100".into()),
            reason: "fits".into(),
        }
    }

    #[test]
    fn an_offered_route_passes_through() {
        let checked = validate(route("antigravity", "gemini-3-pro", "default"), &plan());
        assert_eq!(checked, route("antigravity", "gemini-3-pro", "default"));
    }

    #[test]
    fn accounts_must_have_room_in_the_models_pool() {
        // `default` is blocked for Claude models on Antigravity.
        let checked = validate(
            route("antigravity", "claude-sonnet-4-6", "default"),
            &plan(),
        );
        assert_eq!(checked.account, None);
        assert_eq!(checked.model.as_deref(), Some("claude-sonnet-4-6"));
        let checked = validate(route("antigravity", "claude-sonnet-4-6", "work"), &plan());
        assert_eq!(checked.account.as_deref(), Some("work"));
        // An account of another harness is not an option.
        let checked = validate(route("codex", "gpt-6.1-sol", "work"), &plan());
        assert_eq!(checked.account, None);
    }

    #[test]
    fn anything_not_offered_is_dropped() {
        let mut answer = route("cursor", "gpt-6.1-sol", "default");
        answer.compute = Some("slurm".into());
        let checked = validate(answer, &plan());
        assert_eq!(checked.harness, None);
        assert_eq!(checked.model, None);
        assert_eq!(checked.compute, None);
        assert_eq!(checked.gpu, None);
        let checked = validate(route("codex", "gemini-3-pro", "default"), &plan());
        assert_eq!(checked.model, None);
        assert_eq!(checked.account.as_deref(), Some("default"));
    }

    #[test]
    fn a_turn_keeps_its_harness_and_model() {
        let mut request = RouteRequest::new(Purpose::Turn, "fix the typo");
        request.current = Current {
            harness: Some("antigravity".into()),
            model: Some("claude-sonnet-4-6".into()),
            account: Some("default".into()),
        };
        request.accounts = plan().accounts;
        let checked = validate(route("codex", "gpt-6.1-sol", "work"), &request);
        assert_eq!(checked.harness, None);
        assert_eq!(checked.model, None);
        assert_eq!(checked.account.as_deref(), Some("work"));
    }

    #[test]
    fn local_compute_takes_no_gpu() {
        let mut answer = route("codex", "gpt-6.1-sol", "default");
        answer.compute = Some("local".into());
        assert_eq!(validate(answer, &plan()).gpu, None);
    }

    #[test]
    fn the_request_is_strict_and_skips_reasoning_on_luna() {
        let body = request_body(DEFAULT_MODEL, &plan());
        assert_eq!(body["text"]["format"]["type"], "json_schema");
        assert_eq!(body["text"]["format"]["strict"], true);
        assert_eq!(body["reasoning"]["effort"], "none");
        assert_eq!(body["store"], false);
        let schema = &body["text"]["format"]["schema"];
        assert_eq!(schema["additionalProperties"], false);
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let properties: Vec<&String> = schema["properties"].as_object().unwrap().keys().collect();
        assert_eq!(required.len(), properties.len());
        assert_eq!(
            request_body("gpt-6.1-sol", &plan())["reasoning"]["effort"],
            "low"
        );
    }

    #[test]
    fn replies_parse_or_explain_why_not() {
        let text = serde_json::to_string(&route("codex", "gpt-6.1-sol", "default")).unwrap();
        let reply = json!({
            "status": "completed",
            "output": [
                { "type": "reasoning", "summary": [] },
                { "type": "message", "content": [{ "type": "output_text", "text": text }] },
            ],
        });
        assert_eq!(
            parse_reply(&reply).unwrap(),
            route("codex", "gpt-6.1-sol", "default")
        );
        let refused = json!({ "status": "completed", "output": [
            { "type": "message", "content": [{ "type": "refusal", "refusal": "no" }] },
        ]});
        assert!(parse_reply(&refused).is_err());
        let cut = json!({ "status": "incomplete", "incomplete_details": { "reason": "max_output_tokens" } });
        assert!(parse_reply(&cut)
            .unwrap_err()
            .to_string()
            .contains("max_output_tokens"));
    }

    #[test]
    fn long_tasks_are_cut_and_keys_masked() {
        let request = RouteRequest::new(Purpose::Plan, &"x".repeat(TASK_CHARS + 50));
        assert_eq!(request.task.chars().count(), TASK_CHARS + 1);
        assert_eq!(mask("sk-abcdefgh1234"), "sk-…1234");
        assert!(valid_model_id("gpt-6-luna"));
        assert!(!valid_model_id("gpt 6/../x"));
    }
}
