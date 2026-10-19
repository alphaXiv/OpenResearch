//! Settings routes owned by the tuandang304 fork: research tools, harness
//! accounts, the decision model, cross-harness review, compute routing and
//! auto-compact, plus the compute routes in `fork_compute`. `up.rs` mounts them all with one
//! `.merge(fork_settings::routes())`. See `docs/fork-maintenance.md`.

use super::*;
use crate::commands::discover::extra_sources;

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
            "/api/settings/auto-compact",
            get(auto_compact_settings).post(set_auto_compact_settings),
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
        .route(
            "/api/settings/fork-lit-sources",
            get(fork_lit_sources_settings).post(set_fork_lit_sources_settings),
        )
        .route(
            "/api/chat/sessions/{id}/sleep",
            get(sleep_mode::get_sleep)
                .put(sleep_mode::put_sleep)
                .delete(sleep_mode::delete_sleep),
        )
        .route(
            "/api/chat/sessions/{id}/sleep/stop",
            post(sleep_mode::stop_sleep),
        )
        .merge(fork_compute::routes())
}

/// arXiv and Semantic Scholar toggles, kept apart from upstream's four so the
/// upstream `/api/settings/lit-sources` request shape stays unchanged.
fn fork_lit_sources_json() -> Value {
    let disabled = crate::config::disabled_lit_sources();
    let enabled = |name: &str| !disabled.iter().any(|d| d == name);
    json!({
        "arxiv": enabled(extra_sources::ARXIV),
        "semanticscholar": enabled(extra_sources::SEMANTIC_SCHOLAR),
    })
}

async fn fork_lit_sources_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(fork_lit_sources_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("lit-sources task failed: {e}")))?
}

#[derive(Deserialize)]
struct SetForkLitSourcesReq {
    arxiv: bool,
    semanticscholar: bool,
}

async fn set_fork_lit_sources_settings(Json(req): Json<SetForkLitSourcesReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let fork = [
            (req.arxiv, extra_sources::ARXIV),
            (req.semanticscholar, extra_sources::SEMANTIC_SCHOLAR),
        ];
        let mut disabled: Vec<String> = crate::config::disabled_lit_sources()
            .into_iter()
            .filter(|name| !fork.iter().any(|(_, source)| source == name))
            .collect();
        disabled.extend(
            fork.iter()
                .filter(|(enabled, _)| !enabled)
                .map(|(_, source)| source.to_string()),
        );
        crate::telemetry::set_disabled_lit_sources(disabled)
            .map_err(|e| ApiError::from(anyhow!("could not save literature sources: {e}")))?;
        Ok(Json(fork_lit_sources_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("lit-sources task failed: {e}")))?
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

async fn auto_compact_settings() -> ApiResult {
    Ok(Json(json!(crate::local::auto_compact::settings())))
}

#[derive(Deserialize)]
struct SetAutoCompactReq {
    enabled: Option<bool>,
    percent: Option<u8>,
}

/// Read at the end of every turn, so a change applies from the next one.
async fn set_auto_compact_settings(Json(req): Json<SetAutoCompactReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        crate::local::auto_compact::set_settings(req.enabled, req.percent)
            .map(|settings| Json(json!(settings)))
            .map_err(|error| {
                ApiError::from(anyhow!("could not save the auto-compact setting: {error}"))
            })
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("auto-compact task failed: {e}")))?
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
