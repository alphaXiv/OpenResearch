//! Compute settings routes owned by the tuandang304 fork: provider prices,
//! Colab, Google Cloud and Kaggle. They live here, behind one `.merge()` in
//! `up.rs`, so upstream edits to `compute_settings.rs` and the router merge
//! cleanly. See `docs/fork-maintenance.md`.

use super::*;

pub(super) fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/settings/compute/prices", get(compute_prices))
        .route("/api/settings/colab", get(colab_settings))
        .route(
            "/api/settings/colab/runtimes/stop",
            post(stop_colab_runtime),
        )
        .route(
            "/api/settings/gcp",
            get(gcp_settings).post(set_gcp_settings),
        )
        .route("/api/settings/gcp/instances/stop", post(stop_gcp_instance))
        .route("/api/settings/kaggle", get(kaggle_settings))
        .route("/api/settings/kaggle/accounts", post(add_kaggle_account))
        .route("/api/settings/kaggle/kernels", get(kaggle_live_kernels))
        .route(
            "/api/settings/kaggle/accounts/import",
            post(import_kaggle_account),
        )
        .route(
            "/api/settings/kaggle/accounts/active",
            post(set_kaggle_active_account),
        )
        .route(
            "/api/settings/kaggle/accounts/remove",
            post(remove_kaggle_account),
        )
}

#[derive(Deserialize)]
pub(super) struct ComputePricesQuery {
    /// Skip the caches so Refresh shows current balances.
    fresh: Option<bool>,
}

/// Every provider's GPU prices and remaining credit, cheapest offers first.
pub(super) async fn compute_prices(Query(query): Query<ComputePricesQuery>) -> ApiResult {
    let overview = crate::jobs::provider_prices::overview(query.fresh.unwrap_or(false)).await;
    Ok(Json(json!(overview)))
}

/// The Colab row's expanded detail: CLI install, sign-in, the accelerators a
/// run can request, and the signed-in account's plan and compute units. Never
/// starts a sign-in.
pub(super) async fn colab_settings(Query(query): Query<ColabSettingsQuery>) -> ApiResult {
    colab_settings_with(query.fresh.unwrap_or(false)).await
}

#[derive(serde::Deserialize)]
pub(super) struct ColabSettingsQuery {
    /// Skip the cached account so Refresh shows the current balance.
    fresh: Option<bool>,
}

pub(super) async fn colab_settings_with(fresh: bool) -> ApiResult {
    let mut value = json!(crate::jobs::colab::status().await);
    let account = crate::jobs::colab_account::account(fresh).await;
    let runtimes = if account.is_some() {
        crate::jobs::colab_runtimes::runtimes().await.ok()
    } else {
        None
    };
    value["account"] = json!(account);
    value["runtimes"] = json!(runtimes);
    Ok(Json(value))
}

#[derive(serde::Deserialize)]
pub(super) struct StopColabRuntimeReq {
    endpoint: String,
}

/// Release one Colab runtime so it stops spending compute units.
pub(super) async fn stop_colab_runtime(Json(req): Json<StopColabRuntimeReq>) -> ApiResult {
    crate::jobs::colab_runtimes::stop(&req.endpoint)
        .await
        .map_err(|error| ApiError::from(anyhow!("{error}")))?;
    Ok(Json(json!({ "ok": true })))
}

// --- Kaggle ---------------------------------------------------------------------

#[derive(serde::Deserialize)]
pub(super) struct KaggleSettingsQuery {
    /// Re-read every account's quota instead of the cached one.
    fresh: Option<bool>,
}

/// The Kaggle row's expanded detail: CLI install (with install commands when
/// missing), saved accounts, and each account's weekly quota. Credentials are
/// never part of the payload.
pub(super) async fn kaggle_settings(Query(query): Query<KaggleSettingsQuery>) -> ApiResult {
    kaggle_settings_with(query.fresh.unwrap_or(false)).await
}

pub(super) async fn kaggle_settings_with(fresh: bool) -> ApiResult {
    let status = crate::jobs::kaggle::status().await;
    let accounts = if status.cli_path.is_some() {
        crate::jobs::kaggle::accounts(fresh).await
    } else {
        Vec::new()
    };
    let mut value = json!(status);
    value["accounts"] = json!(accounts);
    Ok(Json(value))
}

/// Kernels queued or running on any saved Kaggle account, including ones orx
/// no longer follows and ones started outside orx.
pub(super) async fn kaggle_live_kernels(Query(query): Query<KaggleSettingsQuery>) -> ApiResult {
    let kernels = crate::jobs::kaggle::live_kernels(query.fresh.unwrap_or(false)).await;
    Ok(Json(json!({ "kernels": kernels })))
}

/// Validate pasted credentials with the Kaggle CLI and save them as an account.
pub(super) async fn add_kaggle_account(
    Json(req): Json<crate::jobs::kaggle::NewAccount>,
) -> ApiResult {
    crate::jobs::kaggle::add_account(req)
        .await
        .map_err(bad_request)?;
    kaggle_settings_with(false).await
}

/// Save the sign-in the Kaggle CLI already has in ~/.kaggle as an account.
pub(super) async fn import_kaggle_account() -> ApiResult {
    crate::jobs::kaggle::import_cli_account()
        .await
        .map_err(bad_request)?;
    kaggle_settings_with(false).await
}

#[derive(serde::Deserialize)]
pub(super) struct KaggleAccountReq {
    username: String,
}

pub(super) async fn set_kaggle_active_account(Json(req): Json<KaggleAccountReq>) -> ApiResult {
    crate::jobs::kaggle::set_active(&req.username)
        .await
        .map_err(bad_request)?;
    kaggle_settings_with(false).await
}

pub(super) async fn remove_kaggle_account(Json(req): Json<KaggleAccountReq>) -> ApiResult {
    crate::jobs::kaggle::remove_account(&req.username)
        .await
        .map_err(bad_request)?;
    kaggle_settings_with(false).await
}

/// `orx compute configure kaggle`: add an account from `token`/`username`/`key`,
/// make `account` the default, or delete `removeAccount`.
pub(super) async fn configure_kaggle(body: Value) -> ApiResult {
    if let Some(name) = body.get("removeAccount").and_then(Value::as_str) {
        return remove_kaggle_account(Json(KaggleAccountReq {
            username: name.to_string(),
        }))
        .await;
    }
    let new: crate::jobs::kaggle::NewAccount =
        serde_json::from_value(body.clone()).map_err(|e| bad_request(anyhow!("{e}")))?;
    if new.token.is_some() || new.key.is_some() || new.kaggle_json.is_some() {
        crate::jobs::kaggle::add_account(new)
            .await
            .map_err(bad_request)?;
    }
    if let Some(name) = body.get("account").and_then(Value::as_str) {
        crate::jobs::kaggle::set_active(name)
            .await
            .map_err(bad_request)?;
    }
    kaggle_settings_with(true).await
}

// --- Google Cloud ---------------------------------------------------------------

use crate::jobs::gcp;

/// The Google Cloud row's expanded detail: CLI and sign-in, project and zone,
/// saved VM options, and the VMs orx created that still exist.
pub(super) async fn gcp_settings() -> ApiResult {
    let settings = gcp::load_settings()
        .map_err(|e| ApiError::from(anyhow!("{e}")))?
        .unwrap_or_default();
    let preflight = gcp::preflight(&settings).await;
    let (instances, billing, balance) = match (&preflight.project, preflight.compute_ready) {
        (Some(project), true) => tokio::join!(
            async { gcp::list_orx_instances(project).await.ok() },
            crate::jobs::gcp_billing::billing_account(project),
            crate::jobs::gcp_billing::balance(&settings, project),
        ),
        _ => (None, None, None),
    };
    let gpus: Vec<Value> = gcp::GPUS
        .iter()
        .map(|gpu| {
            json!({
                "id": gpu.id,
                "label": gpu.label,
                "vramGb": gpu.vram_gb,
                "usdPerHour": gpu.usd_per_hour,
            })
        })
        .collect();
    Ok(Json(json!({
        "settings": settings,
        "preflight": preflight,
        "ready": preflight.compute_ready,
        "instances": instances,
        "billing": billing,
        "balance": balance,
        "gpus": gpus,
    })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetGcpSettingsReq {
    /// `None` leaves a field alone; `Some("")` clears it.
    project: Option<String>,
    zone: Option<String>,
    spot: Option<bool>,
    image_family: Option<String>,
    image_project: Option<String>,
    disk_gb: Option<u64>,
    /// Remaining credit in USD from the console; counting restarts now.
    credit_usd: Option<f64>,
    clear_credit: Option<bool>,
    /// `""` clears it.
    billing_export_table: Option<String>,
}

pub(super) async fn set_gcp_settings(Json(req): Json<SetGcpSettingsReq>) -> ApiResult {
    let mut settings = gcp::load_settings()
        .map_err(|e| ApiError::from(anyhow!("{e}")))?
        .unwrap_or_default();
    let norm = |v: String| Some(v.trim().to_string()).filter(|s| !s.is_empty());
    let valid = |v: &str| {
        v.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    };
    for value in [
        &req.project,
        &req.zone,
        &req.image_family,
        &req.image_project,
    ]
    .into_iter()
    .flatten()
    {
        if !valid(value.trim()) {
            return Err(bad_request(format!(
                "'{}' is not a valid Google Cloud name.",
                value.trim()
            )));
        }
    }
    if let Some(project) = req.project {
        settings.project = norm(project);
    }
    if let Some(zone) = req.zone {
        settings.zone = norm(zone);
    }
    if let Some(spot) = req.spot {
        settings.spot = spot;
    }
    if let Some(family) = req.image_family {
        settings.image_family = norm(family);
    }
    if let Some(image_project) = req.image_project {
        settings.image_project = norm(image_project);
    }
    if let Some(disk) = req.disk_gb {
        if !(10..=10_000).contains(&disk) {
            return Err(bad_request("The boot disk must be 10–10000 GB."));
        }
        settings.disk_gb = Some(disk);
    }
    if let Some(credit) = req.credit_usd {
        if !credit.is_finite() || credit < 0.0 {
            return Err(bad_request("The credit must be a positive amount in USD."));
        }
        settings.credit_usd = Some(credit);
        settings.credit_as_of = Some(crate::store::now_ms());
    }
    if req.clear_credit == Some(true) {
        settings.credit_usd = None;
        settings.credit_as_of = None;
    }
    if let Some(table) = req.billing_export_table {
        let table = norm(table);
        if let Some(table) = &table {
            if !crate::jobs::gcp_billing::valid_export_table(table) {
                return Err(bad_request(
                    "The billing export table must look like project.dataset.table.",
                ));
            }
        }
        settings.billing_export_table = table;
    }
    gcp::save_settings(&settings).map_err(|e| ApiError::from(anyhow!("{e}")))?;
    crate::jobs::provider_prices::invalidate().await;
    gcp_settings().await
}

#[derive(Deserialize)]
pub(super) struct StopGcpInstanceReq {
    zone: String,
    name: String,
}

/// Delete one VM orx created, so it stops billing.
pub(super) async fn stop_gcp_instance(Json(req): Json<StopGcpInstanceReq>) -> ApiResult {
    let settings = gcp::load_settings()
        .map_err(|e| ApiError::from(anyhow!("{e}")))?
        .unwrap_or_default();
    let (Some(project), _) = gcp::resolve_location(&settings).await else {
        return Err(bad_request("No Google Cloud project is set."));
    };
    // Only VMs orx labelled for a run; never the user's other machines.
    let owned = gcp::describe(&project, &req.zone, &req.name)
        .await
        .map_err(|e| ApiError::from(anyhow!("{e}")))?
        .is_some_and(|instance| instance.run_id.is_some());
    if !owned {
        return Err(bad_request("orx only deletes VMs it created for a run."));
    }
    gcp::delete(&project, &req.zone, &req.name)
        .await
        .map_err(|e| ApiError::from(anyhow!("{e}")))?;
    Ok(Json(json!({ "ok": true })))
}
