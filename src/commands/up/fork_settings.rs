//! Settings routes owned by the tuandang304 fork: research tools, harness
//! accounts, the decision model, cross-harness review and compute routing,
//! plus the compute routes in `fork_compute`. `up.rs` mounts them all with one
//! `.merge(fork_settings::routes())`. See `docs/fork-maintenance.md`.

use super::*;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/projects/{id}/metrics",
            get(research_tools::project_metrics),
        )
        .route(
            "/api/runs/{id}/metrics",
            post(research_tools::import_metrics),
        )
        .route(
            "/api/settings/compute/doctor/{backend}",
            post(research_tools::compute_doctor),
        )
        .route(
            "/api/settings/cross-harness",
            get(cross_harness_settings).post(set_cross_harness_settings),
        )
        .route(
            "/api/settings/compute-routing",
            get(compute_routing_settings).post(set_compute_routing_settings),
        )
        .route(
            "/api/harnesses/accounts",
            get(harness_accounts::list).post(harness_accounts::add),
        )
        .route(
            "/api/harnesses/accounts/update",
            post(harness_accounts::update),
        )
        .route(
            "/api/harnesses/accounts/remove",
            post(harness_accounts::remove),
        )
        .route(
            "/api/harnesses/accounts/login",
            get(harness_accounts::login),
        )
        .route(
            "/api/settings/decision-model",
            get(harness_accounts::decision_settings).post(harness_accounts::set_decision_settings),
        )
        .merge(fork_compute::routes())
}

fn cross_harness_settings_json() -> Value {
    json!({
        "enabled": crate::config::cross_harness_review(),
        "excluded": crate::config::cross_harness_excluded(),
    })
}

async fn cross_harness_settings() -> ApiResult {
    Ok(Json(cross_harness_settings_json()))
}

#[derive(Deserialize)]
struct SetCrossHarnessReq {
    enabled: Option<bool>,
    /// Replaces the whole list of harnesses turned off for review.
    excluded: Option<Vec<String>>,
}

/// Takes effect for each session at its next playbook rewrite.
async fn set_cross_harness_settings(Json(req): Json<SetCrossHarnessReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let save = |error: crate::error::Error| {
            ApiError::from(anyhow!("could not save the cross-harness setting: {error}"))
        };
        if let Some(excluded) = req.excluded {
            if let Some(unknown) = excluded
                .iter()
                .find(|id| crate::local::harness::chat_harness(id).is_none())
            {
                return Err(ApiError::from(anyhow!("Unknown harness {unknown}.")));
            }
            crate::config::set_cross_harness_excluded(excluded).map_err(save)?;
        }
        if let Some(enabled) = req.enabled {
            crate::config::set_cross_harness_review(enabled).map_err(save)?;
        }
        Ok(Json(cross_harness_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("cross-harness task failed: {e}")))?
}

fn compute_routing_settings_json() -> Value {
    json!({
        "enabled": crate::config::compute_routing(),
        "excluded": crate::config::compute_routing_excluded(),
    })
}

async fn compute_routing_settings() -> ApiResult {
    Ok(Json(compute_routing_settings_json()))
}

#[derive(Deserialize)]
struct SetComputeRoutingReq {
    enabled: Option<bool>,
    /// Replaces the whole list of backends kept out of routing.
    excluded: Option<Vec<String>>,
}

/// Takes effect for each session at its next playbook rewrite.
async fn set_compute_routing_settings(Json(req): Json<SetComputeRoutingReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let save = |error: crate::error::Error| {
            ApiError::from(anyhow!(
                "could not save the compute routing setting: {error}"
            ))
        };
        if let Some(excluded) = req.excluded {
            if let Some(unknown) = excluded
                .iter()
                .find(|id| !crate::local::BACKENDS.contains(&id.as_str()))
            {
                return Err(ApiError::from(anyhow!(
                    "Unknown compute backend {unknown}."
                )));
            }
            crate::config::set_compute_routing_excluded(excluded).map_err(save)?;
        }
        if let Some(enabled) = req.enabled {
            crate::config::set_compute_routing(enabled).map_err(save)?;
        }
        Ok(Json(compute_routing_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("compute routing task failed: {e}")))?
}
