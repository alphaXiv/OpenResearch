//! Sleep-mode routes for the chat's Sleep mode dialog. The plan, ledger and
//! watchdog live in `crate::local::sleep`; `orx sleep` is the CLI over the same.

use super::*;
use crate::local::sleep::{self, Direction, Finish, FinishReason, Objective, SleepPlan};

fn chat(store: &Store, id: &str) -> std::result::Result<crate::store::StoredChatSession, ApiError> {
    store
        .get_chat_session(id)?
        .ok_or_else(|| not_found("chat session"))
}

fn status(store: &Store, id: &str) -> ApiResult {
    let sleep = match store.get_sleep(id)? {
        Some(stored) => sleep::status_json(store, &stored)?,
        None => Value::Null,
    };
    Ok(Json(json!({ "sleep": sleep })))
}

pub(super) async fn get_sleep(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    chat(&store, &id)?;
    status(&store, &id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SleepReq {
    budget_usd: f64,
    hours: Option<f64>,
    metric: Option<String>,
    direction: Option<String>,
    target_pct: Option<f64>,
    baseline_value: Option<f64>,
    keep_awake: Option<bool>,
}

pub(super) async fn put_sleep(Path(id): Path<String>, Json(req): Json<SleepReq>) -> ApiResult {
    let store = Store::open()?;
    let session = chat(&store, &id)?;
    if req.hours.is_some_and(|h| !h.is_finite() || h <= 0.0) {
        return Err(bad_request(anyhow!("Hours must be a positive number.")));
    }
    let objective = match req
        .metric
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
    {
        Some(metric) => Some(Objective {
            metric,
            direction: match req.direction.as_deref() {
                Some(d) => Direction::from_id(d)
                    .ok_or_else(|| bad_request(anyhow!("Direction is max or min.")))?,
                None => Direction::Max,
            },
            improve_pct: req.target_pct,
            baseline_value: req.baseline_value,
            baseline_run: None,
        }),
        None => None,
    };
    // A changed plan keeps the settings the dialog does not show.
    let previous = store.get_sleep(&id)?.map(|s| s.plan).unwrap_or_default();
    let plan = SleepPlan {
        gpu_budget_usd: req.budget_usd,
        until: req
            .hours
            .map(|h| crate::store::now_ms() + (h * 3_600_000.0) as i64),
        objective,
        keep_awake: req.keep_awake.unwrap_or(previous.keep_awake),
        ..previous
    }
    .validated()
    .map_err(bad_request)?;
    sleep::apply_plan(&store, &id, &session.project_id, &plan)?;
    status(&store, &id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StopReq {
    reason: Option<String>,
}

pub(super) async fn stop_sleep(Path(id): Path<String>, Json(req): Json<StopReq>) -> ApiResult {
    let store = Store::open()?;
    let mut stored = store
        .get_sleep(&id)?
        .ok_or_else(|| not_found("sleep mode"))?;
    if stored.state.finished.is_none() {
        let reason = req
            .reason
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "the user".to_string());
        stored.state.finished = Some(Finish {
            reason: FinishReason::Agent,
            detail: format!("stopped by {reason}"),
            at: crate::store::now_ms(),
        });
        store.set_sleep_state(&id, &stored.state)?;
    }
    status(&store, &id)
}

pub(super) async fn delete_sleep(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store.delete_sleep(&id)?;
    status(&store, &id)
}
