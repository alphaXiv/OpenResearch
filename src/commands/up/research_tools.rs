//! Additive routes for diagnostics and structured measurements.
use super::*;
use crate::store::run_metrics::RunMetrics;

pub(super) async fn compute_doctor(Path(backend): Path<String>) -> ApiResult {
    use clap::ValueEnum;
    let backend =
        crate::commands::compute::Backend::from_str(&backend, false).map_err(bad_request)?;
    Ok(Json(json!(
        crate::commands::compute_doctor::diagnose(backend.name(), None, None, false, None,).await
    )))
}

pub(super) async fn project_metrics(Path(id): Path<String>) -> ApiResult {
    let store = Store::open()?;
    store
        .get_local_project(&id)?
        .ok_or_else(|| not_found("project"))?;
    Ok(Json(json!(store.project_run_metrics(&id)?)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ImportMetrics {
    metrics: RunMetrics,
    #[serde(default)]
    replace: bool,
}

pub(super) async fn import_metrics(
    Path(id): Path<String>,
    Json(body): Json<ImportMetrics>,
) -> ApiResult {
    body.metrics.validate().map_err(bad_request)?;
    let store = Store::open()?;
    store.get_run(&id)?.ok_or_else(|| not_found("run"))?;
    store
        .put_run_metrics(&id, &body.metrics, body.replace)
        .map_err(bad_request)?;
    Ok(Json(json!(body.metrics)))
}
