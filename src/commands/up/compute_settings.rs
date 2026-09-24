//! Shared compute settings operations for the dashboard and standalone CLI.

use super::*;

// --- HF token settings ------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HfSettings {
    configured: bool,
    source: Option<&'static str>,
    masked_token: Option<String>,
    validation_status: &'static str,
    validation_error: Option<String>,
    username: Option<String>,
    jobs_write: Option<bool>,
}

/// Never the full token: first 3 chars + ellipsis + last 4.
pub(super) fn mask_token(token: &str) -> String {
    let chars: Vec<char> = token.chars().collect();
    if chars.len() < 8 {
        return "…".to_string();
    }
    format!(
        "{}…{}",
        chars[..3].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

/// Re-resolve the token and check it against whoami-v2. Uncached — cheap, and
/// the UI calls it rarely.
pub(super) async fn hf_token_status() -> HfSettings {
    use crate::jobs::huggingface::{self, TokenSource};
    let Ok((token, source)) = huggingface::resolve_token_with_source() else {
        return HfSettings {
            configured: false,
            source: None,
            masked_token: None,
            validation_status: "missing",
            validation_error: None,
            username: None,
            jobs_write: None,
        };
    };
    let source = match source {
        TokenSource::Env => "env",
        TokenSource::OpenresearchEnv => "openresearchEnv",
        TokenSource::HfCache => "hfCache",
    };
    match huggingface::whoami_details(&token).await {
        Ok(details) => HfSettings {
            configured: true,
            source: Some(source),
            masked_token: Some(mask_token(&token)),
            validation_status: "valid",
            validation_error: None,
            username: Some(details.name),
            jobs_write: details.jobs_write,
        },
        Err(error) => {
            let validation_status = if error
                .downcast_ref::<reqwest::Error>()
                .is_some_and(|error| error.status() == Some(reqwest::StatusCode::UNAUTHORIZED))
            {
                "invalid"
            } else {
                "unreachable"
            };
            HfSettings {
                configured: true,
                source: Some(source),
                masked_token: Some(mask_token(&token)),
                validation_status,
                validation_error: Some(error.to_string()),
                username: None,
                jobs_write: None,
            }
        }
    }
}

pub(super) async fn hf_settings() -> Json<Value> {
    Json(json!(hf_token_status().await))
}

pub(super) async fn tinker_settings() -> ApiResult {
    use crate::jobs::tinker;
    let Ok((key, source)) = tinker::resolve_api_key_with_source() else {
        return Ok(Json(
            json!({ "validationStatus": tinker::KeyStatus::Missing, "processEnv": false, "maskedKey": null }),
        ));
    };
    let status = tinker::validate_api_key(&key).await.map_err(bad_request)?;
    Ok(Json(json!({
        "validationStatus": status,
        "processEnv": source == tinker::ApiKeySource::Env,
        "maskedKey": mask_token(&key),
    })))
}

#[derive(Deserialize)]
pub(super) struct SetTinkerKeyReq {
    key: String,
}

pub(super) async fn set_tinker_key(Json(req): Json<SetTinkerKeyReq>) -> ApiResult {
    use crate::jobs::tinker;
    let key = req.key.trim().to_string();
    if key.is_empty() {
        return Err(bad_request("API key is required"));
    }
    let status = tinker::validate_api_key(&key).await.map_err(bad_request)?;
    if status == tinker::KeyStatus::Invalid {
        return Err(bad_request(
            "Invalid Tinker API key. The existing key was not changed.",
        ));
    }
    let process_key = std::env::var(tinker::API_KEY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let effective_status = if let Some(ref process_key) = process_key {
        if process_key.trim() == key {
            status
        } else {
            tinker::validate_api_key(process_key.trim())
                .await
                .map_err(bad_request)?
        }
    } else {
        status
    };
    let masked_key = mask_token(process_key.as_deref().map(str::trim).unwrap_or(&key));
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_var(tinker::API_KEY_ENV, &key)
    })
    .await
    .map_err(|e| anyhow!("env write task failed: {e}"))??;
    Ok(Json(
        json!({ "validationStatus": effective_status, "processEnv": process_key.is_some(), "maskedKey": masked_key }),
    ))
}

#[derive(Deserialize)]
pub(super) struct SetHfTokenReq {
    token: String,
}

pub(super) async fn set_hf_token(Json(req): Json<SetHfTokenReq>) -> ApiResult {
    let token = req.token.trim().to_string();
    if token.is_empty() {
        return Err(bad_request("token is required"));
    }
    crate::jobs::huggingface::whoami_details(&token)
        .await
        .map_err(bad_request)?;
    tokio::task::spawn_blocking(move || crate::config::write_synced_env_var("HF_TOKEN", &token))
        .await
        .map_err(|e| anyhow!("env write task failed: {e}"))??;
    // Freshly re-resolved: if HF_TOKEN is set in this process env, env still
    // wins over the file — source says "env" and the UI explains it.
    Ok(Json(json!(hf_token_status().await)))
}

// --- modal settings -----------------------------------------------------------

use crate::jobs::modal;

pub(super) fn modal_settings_json() -> crate::error::Result<Value> {
    let token = modal::resolve_token()?;
    Ok(json!({
        "tokenConfigured": token.is_some(),
        "tokenSource": token.as_ref().map(|token| token.source),
        "maskedTokenId": token.as_ref().map(|token| mask_token(&token.id)),
        "maskedTokenSecret": token.as_ref().map(|token| mask_token(&token.secret)),
        "processEnv": std::env::var_os("MODAL_TOKEN_ID").is_some() || std::env::var_os("MODAL_TOKEN_SECRET").is_some(),
    }))
}

pub(super) async fn modal_settings() -> ApiResult {
    Ok(Json(modal_settings_json().map_err(bad_request)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetModalTokenReq {
    token_id: String,
    token_secret: String,
}

pub(super) async fn set_modal_token(Json(req): Json<SetModalTokenReq>) -> ApiResult {
    let id = req.token_id.trim().to_string();
    let secret = req.token_secret.trim().to_string();
    if id.is_empty() || secret.is_empty() {
        return Err(bad_request(
            "Both the Modal token ID and secret are required.",
        ));
    }
    if std::env::var_os("MODAL_TOKEN_ID").is_some()
        || std::env::var_os("MODAL_TOKEN_SECRET").is_some()
    {
        return Err(bad_request("Modal credentials are overridden by the process environment. Remove those overrides before replacing the token here."));
    }
    // Modal loads and validates its profile file even when credentials come from env.
    modal::resolve_token().map_err(bad_request)?;
    let masked_id = mask_token(&id);
    let masked_secret = mask_token(&secret);
    tokio::task::spawn_blocking(move || {
        crate::config::write_synced_env_vars(&[
            ("MODAL_TOKEN_ID", &id),
            ("MODAL_TOKEN_SECRET", &secret),
        ])
    })
    .await
    .map_err(|e| anyhow!("env write task failed: {e}"))??;
    Ok(Json(json!({
        "tokenConfigured": true, "tokenSource": "syncedEnv", "processEnv": false,
        "maskedTokenId": masked_id, "maskedTokenSecret": masked_secret,
    })))
}

// --- kubernetes settings ------------------------------------------------------

use crate::jobs::kubernetes as k8s;

/// One payload powers the whole settings card: stored config plus live
/// cluster health. Contexts come from the local kubeconfig. Resource shapes
/// live in each experiment's committed manifest, not in settings.
pub(super) async fn k8s_settings_json() -> Value {
    let settings = k8s::load_settings().ok().flatten();
    let configured = settings.is_some();
    let settings = settings.unwrap_or_default();
    let (contexts, current) = k8s::list_contexts().await.unwrap_or((Vec::new(), None));
    let preflight = k8s::preflight(settings.context.as_deref(), &settings.namespace).await;
    json!({
        "configured": configured,
        "contexts": contexts,
        "currentContext": current,
        "context": settings.context,
        "namespace": settings.namespace,
        "preflight": preflight,
    })
}

pub(super) async fn k8s_settings() -> ApiResult {
    Ok(Json(k8s_settings_json().await))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetK8sSettingsReq {
    /// `None` leaves the field alone; `Some("")` clears it (kubectl default).
    context: Option<String>,
    namespace: Option<String>,
}

pub(super) async fn set_k8s_settings(Json(req): Json<SetK8sSettingsReq>) -> ApiResult {
    let mut settings = k8s::load_settings()?.unwrap_or_default();
    if let Some(ctx) = req.context {
        settings.context = Some(ctx.trim().to_string()).filter(|c| !c.is_empty());
    }
    if let Some(ns) = req.namespace {
        let ns = ns.trim().to_string();
        settings.namespace = if ns.is_empty() {
            "default".to_string()
        } else {
            ns
        };
    }
    k8s::save_settings(&settings)?;
    Ok(Json(k8s_settings_json().await))
}

/// Concrete Host entries from `~/.ssh/config` (wildcard patterns skipped) —
/// read-only groundwork for an SSH compute backend. No keys are read.
pub(super) fn list_ssh_hosts() -> Vec<Value> {
    let Some(path) = dirs::home_dir().map(|h| h.join(".ssh").join("config")) else {
        return Vec::new();
    };
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut hosts: Vec<Value> = Vec::new();
    // Indices into `hosts` for the Host block currently being filled.
    let mut current: Vec<usize> = Vec::new();
    for line in raw.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = match line.split_once([' ', '\t', '=']) {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"')),
            None => continue,
        };
        if key == "host" {
            current = value
                .split_whitespace()
                .filter(|name| !name.contains(['*', '?', '!']))
                .map(|name| {
                    hosts.push(json!({ "host": name }));
                    hosts.len() - 1
                })
                .collect();
            continue;
        }
        let field = match key.as_str() {
            "hostname" => "hostname",
            "user" => "user",
            "port" => "port",
            "identityfile" => "identityFile",
            _ => continue,
        };
        for &i in &current {
            // First value wins, like ssh itself.
            if hosts[i].get(field).is_none() {
                hosts[i][field] = json!(value);
            }
        }
    }
    hosts
}

pub(super) async fn ssh_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| {
        let mut hosts = list_ssh_hosts();
        let settings = crate::config::ssh_settings()?;
        for host in &mut hosts {
            let options = host
                .get("host")
                .and_then(Value::as_str)
                .and_then(|host| settings.hosts.get(host))
                .cloned()
                .unwrap_or_default();
            host["container"] = json!(options.container);
        }
        // Best-effort, like the preflight write: a store hiccup shouldn't take
        // out the host listing — hosts just render as never tested.
        let tests: HashMap<String, SshHostTest> = Store::open()
            .and_then(|s| s.list_ssh_host_tests())
            .unwrap_or_else(|e| {
                eprintln!("orx up: could not load ssh test history: {e}");
                Vec::new()
            })
            .into_iter()
            .map(|t| (t.host.clone(), t))
            .collect();
        for h in &mut hosts {
            let Some(t) = h
                .get("host")
                .and_then(Value::as_str)
                .and_then(|a| tests.get(a))
            else {
                continue;
            };
            h["lastTest"] = json!(t);
        }
        Ok(Json(
            json!({ "hosts": hosts, "defaultHost": settings.default_host }),
        ))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("ssh task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveSshSettingsReq {
    host: String,
    container: Option<String>,
}

pub(super) async fn save_ssh_settings(Json(req): Json<SaveSshSettingsReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    require_configured_ssh_host(&host)?;
    let options = crate::config::SshHostSettings {
        container: req.container,
    };
    crate::jobs::ssh::validate_host_options(&options).map_err(bad_request)?;
    blocking_api(move || {
        crate::config::set_ssh_host(host, options)?;
        Ok(Json(json!({"ok": true})))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct SaveSshDefaultReq {
    host: Option<String>,
}

pub(super) async fn save_ssh_default(Json(req): Json<SaveSshDefaultReq>) -> ApiResult {
    let host = req.host.map(|host| host.trim().to_string());
    if let Some(host) = &host {
        require_configured_ssh_host(host)?;
    }
    blocking_api(move || {
        crate::config::set_ssh_default(host)?;
        Ok(Json(json!({"ok": true})))
    })
    .await
}

pub(super) fn ssh_config_path() -> Result<std::path::PathBuf> {
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("no home directory"))?
        .join(".ssh")
        .join("config"))
}

pub(super) async fn ssh_config() -> ApiResult {
    blocking_api(move || {
        let path = ssh_config_path()?;
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(ApiError::from(anyhow!(
                    "could not read SSH config: {error}"
                )))
            }
        };
        Ok(Json(json!({ "path": "~/.ssh/config", "content": content })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SaveSshConfigReq {
    content: String,
    previous_content: String,
}

pub(super) async fn save_ssh_config(Json(req): Json<SaveSshConfigReq>) -> ApiResult {
    blocking_api(move || {
        if req.content.len() as u64 > FILE_WRITE_LIMIT {
            return Err(bad_request("SSH config is too large to save"));
        }
        let path = ssh_config_path()?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("SSH config has no parent"))?;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent).map_err(anyhow::Error::from)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(".orx-config.lock"))
            .map_err(anyhow::Error::from)?;
        let mut lock = fd_lock::RwLock::new(file);
        let _guard = lock.write().map_err(anyhow::Error::from)?;
        let current = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(ApiError::from(anyhow!(
                    "could not read SSH config: {error}"
                )))
            }
        };
        if current != req.previous_content {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "SSH config changed on disk. Reload it before saving.".into(),
            ));
        }

        crate::local::git::atomic_write_with_mode(&path, req.content.as_bytes(), Some(0o600))
            .map_err(|error| ApiError::from(anyhow!("could not save SSH config: {error}")))?;
        Ok(Json(json!({ "ok": true })))
    })
    .await
}

#[derive(Deserialize)]
pub(super) struct SshPreflightReq {
    host: String,
    container: Option<String>,
}

pub(super) async fn ssh_master_status(Query(req): Query<SshPreflightReq>) -> ApiResult {
    let host = req.host.trim();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    // A missing master is meaningful only on platforms that support multiplexing.
    // Windows opens a new SSH connection for each command; reporting false here
    // makes a successful preflight immediately look disconnected in the dashboard.
    let running = if cfg!(unix) {
        Some(crate::jobs::ssh::master_is_running(&crate::jobs::ssh::SshTarget::alias(host)).await?)
    } else {
        None
    };
    Ok(Json(json!({ "running": running })))
}

/// Live check for one host: can we reach it and run bash/tar snapshots?
pub(super) async fn ssh_preflight(Json(req): Json<SshPreflightReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    require_configured_ssh_host(&host)?;
    if let Some(reference) = &req.container {
        crate::jobs::ssh::validate_container_reference(reference).map_err(bad_request)?;
    }
    let test = run_ssh_host_preflight(host.clone()).await;
    let container = if let Some(reference) = req.container {
        let result = crate::jobs::ssh::resolve_container(
            &crate::jobs::ssh::SshTarget::alias(&host),
            &reference,
        )
        .await;
        Some(
            json!({"reference": reference, "ready": result.is_ok(), "error": result.err().map(|error| error.to_string())}),
        )
    } else {
        None
    };
    let mut result =
        serde_json::to_value(test).map_err(|error| ApiError::from(anyhow!("{error}")))?;
    result["container"] = json!(container);
    Ok(Json(result))
}

pub(super) fn require_configured_ssh_host(host: &str) -> Result<(), ApiError> {
    if list_ssh_hosts()
        .iter()
        .any(|candidate| candidate.get("host").and_then(Value::as_str) == Some(host))
    {
        Ok(())
    } else {
        Err(bad_request("Choose a host from ~/.ssh/config."))
    }
}

pub(super) async fn run_ssh_host_preflight(host: String) -> SshHostTest {
    let test = probe_ssh_host_preflight(host).await;
    record_ssh_host_test(&test).await;
    test
}

pub(super) async fn probe_ssh_host_preflight(host: String) -> SshHostTest {
    let p = crate::jobs::ssh::preflight(&crate::jobs::ssh::SshTarget::alias(&host)).await;
    SshHostTest {
        host,
        reachable: p.reachable,
        tools_found: p.tools_found,
        missing_tools: p.missing_tools,
        error: p.error,
        tested_at: now_ms(),
    }
}

pub(super) async fn record_ssh_host_test(test: &SshHostTest) {
    // Best-effort persistence — the UI shows "last tested" across restarts,
    // but a store hiccup shouldn't hide a test result that already ran.
    let record = test.clone();
    if let Err(e) =
        tokio::task::spawn_blocking(move || Store::open()?.upsert_ssh_host_test(&record))
            .await
            .map_err(|e| anyhow!("ssh task failed: {e}"))
            .and_then(|r| r)
    {
        eprintln!("orx up: could not record ssh test for {}: {e}", test.host);
    }
}

// --- slurm --------------------------------------------------------------------

use crate::jobs::slurm;

/// One payload powers the whole settings card: stored cluster defaults plus
/// the ssh hosts to pick a login node from (same `~/.ssh/config` source as
/// the ssh backend — a Slurm login node is just an ssh host).
pub(super) fn slurm_settings_json() -> Value {
    let settings = slurm::load_settings().ok().flatten().unwrap_or_default();
    json!({
        "host": settings.host,
        "partition": settings.partition,
        "account": settings.account,
        "timeLimit": settings.time_limit,
        "hosts": list_ssh_hosts(),
    })
}

pub(super) async fn slurm_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(slurm_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("slurm task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetSlurmSettingsReq {
    /// `None` leaves the field alone; `Some("")` clears it (cluster default).
    host: Option<String>,
    partition: Option<String>,
    account: Option<String>,
    time_limit: Option<String>,
}

pub(super) async fn set_slurm_settings(Json(req): Json<SetSlurmSettingsReq>) -> ApiResult {
    // One spawn_blocking around the whole load→mutate→save→respond body
    // (settings + ~/.ssh/config are sync fs I/O), like the git handlers.
    tokio::task::spawn_blocking(move || {
        let mut settings = slurm::load_settings()?.unwrap_or_default();
        let norm = |v: String| Some(v.trim().to_string()).filter(|s| !s.is_empty());
        if let Some(h) = req.host {
            settings.host = norm(h);
        }
        if let Some(p) = req.partition {
            settings.partition = norm(p);
        }
        if let Some(a) = req.account {
            settings.account = norm(a);
        }
        if let Some(t) = req.time_limit {
            // Reject a default that would fail every later launch.
            let t = norm(t);
            if let Some(t) = &t {
                crate::jobs::huggingface::parse_timeout(t).map_err(bad_request)?;
            }
            settings.time_limit = t;
        }
        slurm::save_settings(&settings)?;
        Ok(Json(slurm_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("slurm task failed: {e}")))?
}

#[derive(Deserialize)]
pub(super) struct SlurmPreflightReq {
    host: String,
}

/// Live check for one login node: reachable, Slurm CLI + snapshot tools, and
/// which partitions exist (feeds the partition picker).
pub(super) async fn slurm_preflight(Json(req): Json<SlurmPreflightReq>) -> ApiResult {
    let host = req.host.trim().to_string();
    if host.is_empty() {
        return Err(bad_request("host is required"));
    }
    let p = slurm::preflight(&host).await;
    Ok(Json(slurm_preflight_value(&p)))
}

pub(super) fn slurm_preflight_value(p: &slurm::SlurmPreflight) -> Value {
    json!({
        "reachable": p.reachable,
        "slurmFound": p.slurm_found,
        "toolsFound": p.tools_found,
        "partitions": p.partitions,
        "error": p.error,
    })
}

// --- ray --------------------------------------------------------------------

use crate::jobs::ray;

pub(super) fn ray_settings_json() -> Value {
    let settings = ray::load_settings().ok().flatten().unwrap_or_default();
    let (resolved, source) = ray::resolve_address_with_source();
    let source_label = match source {
        ray::AddressSource::Settings => "settings",
        ray::AddressSource::AstroaiEnv => "ASTROAI_RAY_JOBS_ADDRESS",
        ray::AddressSource::RayEnv => "RAY_DASHBOARD_URL",
        ray::AddressSource::Default => "default",
    };
    json!({
        "address": settings.address,
        "resolvedAddress": resolved,
        "source": source_label,
    })
}

pub(super) async fn ray_settings() -> ApiResult {
    tokio::task::spawn_blocking(|| Ok(Json(ray_settings_json())))
        .await
        .map_err(|e| ApiError::from(anyhow!("ray task failed: {e}")))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetRaySettingsReq {
    /// `None` leaves alone; `Some("")` clears (fall back to env / default).
    address: Option<String>,
}

pub(super) async fn set_ray_settings(Json(req): Json<SetRaySettingsReq>) -> ApiResult {
    tokio::task::spawn_blocking(move || {
        let mut settings = ray::load_settings()?.unwrap_or_default();
        if let Some(a) = req.address {
            let a = Some(a.trim().to_string()).filter(|s| !s.is_empty());
            if let Some(a) = &a {
                // Reject a default that would fail every later launch.
                let url = reqwest::Url::parse(a)
                    .map_err(|e| bad_request(anyhow!("Invalid Jobs URL {a:?}: {e}")))?;
                if !matches!(url.scheme(), "http" | "https") {
                    return Err(bad_request(anyhow!(
                        "The Jobs URL must be http(s), e.g. http://127.0.0.1:8265 (got {a:?})."
                    )));
                }
            }
            settings.address = a;
        }
        ray::save_settings(&settings)?;
        Ok(Json(ray_settings_json()))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("ray task failed: {e}")))?
}

#[derive(Deserialize)]
pub(super) struct RayPreflightReq {
    address: Option<String>,
}

/// Live check for a Ray Jobs / Dashboard endpoint.
pub(super) async fn ray_preflight(Json(req): Json<RayPreflightReq>) -> ApiResult {
    let address = ray::resolve_address(req.address.as_deref());
    match ray::preflight(&address).await {
        Ok(ray_version) => Ok(Json(json!({
            "reachable": true,
            "address": address,
            "rayVersion": ray_version,
            "error": null,
        }))),
        Err(e) => Ok(Json(json!({
            "reachable": false,
            "address": address,
            "rayVersion": null,
            "error": e.to_string(),
        }))),
    }
}

// --- compute targets (unified settings list + default) --------------------------

/// The whole payload for the Compute tab's collapsed list, in one round trip.
/// CHEAP probes only — env vars and file reads, never a network call, kubectl,
/// or the modal python import. `configured` means "worth trying", not
/// "healthy"; deep health stays in each backend's own settings endpoint,
/// fetched when a row is expanded.
/// Whether a box would accept this machine. Three-valued on purpose: the check
/// needs the api, so "we couldn't ask" is a different answer from "no" and the
/// badge shouldn't have to pick one of the two. Each arm carries what the row
/// needs to say — guessing a key path would send the user to a file that may
/// not exist.
#[derive(Clone, PartialEq)]
pub(super) enum SshReadiness {
    Ready,
    /// The `.pub` on this machine worth registering, if there is one.
    NoUsableKey {
        pub_path: Option<String>,
    },
    Unverified {
        reason: String,
    },
}

pub(super) async fn openresearch_ssh_readiness() -> SshReadiness {
    use crate::local::ssh_identity::{preferred_local, tilde, KeyStatus};
    let Ok(Some(creds)) = crate::config::load_credentials().await else {
        // Signed-out is reported by the row's own `or_logged_in`.
        return SshReadiness::NoUsableKey { pub_path: None };
    };
    let named = |local: &[crate::local::ssh_identity::LocalKey]| SshReadiness::NoUsableKey {
        pub_path: preferred_local(local)
            .and_then(|k| k.path.as_deref())
            .map(tilde),
    };
    match crate::local::ssh_identity::check(&creds).await {
        KeyStatus::Matched => SshReadiness::Ready,
        KeyStatus::NoLocalMatch { local, .. } | KeyStatus::NoneRegistered { local } => {
            named(&local)
        }
        KeyStatus::Unknown { reason } => SshReadiness::Unverified { reason },
    }
}

/// The openresearch row's one-line status. Every branch that tells the user to
/// run something names a path we actually found — never a guessed one.
pub(super) fn openresearch_summary(logged_in: bool, ssh: &SshReadiness) -> String {
    if !logged_in {
        return "Not signed in — run orx login".to_string();
    }
    match ssh {
        SshReadiness::Ready => "Signed in — ephemeral boxes billed to your org".to_string(),
        SshReadiness::NoUsableKey {
            pub_path: Some(path),
        } => format!("No usable SSH key — run orx ssh-key add {path}"),
        SshReadiness::NoUsableKey { pub_path: None } => {
            "No SSH key on this computer — run ssh-keygen -t ed25519, then orx ssh-key add"
                .to_string()
        }
        SshReadiness::Unverified { reason } => {
            format!("Signed in — couldn't check your SSH key ({reason})")
        }
    }
}

pub(super) fn compute_settings_json(ssh: SshReadiness) -> Value {
    let default = crate::config::compute_default();
    let (default_backend, default_flavor) = match &default {
        Some((b, f)) => (Some(b.as_str()), f.as_deref()),
        None => (None, None),
    };

    let hf = crate::jobs::huggingface::resolve_token_with_source().ok();
    let tinker = crate::jobs::tinker::resolve_api_key_with_source().ok();
    let modal_source = crate::jobs::modal::token_source();
    let k8s_settings = k8s::load_settings().ok().flatten();
    let ssh_hosts = list_ssh_hosts().len();
    let slurm_settings = crate::jobs::slurm::load_settings().ok().flatten();
    let slurm_host = slurm_settings.as_ref().and_then(|s| s.host.clone());
    let (ray_resolved, ray_source) = crate::jobs::ray::resolve_address_with_source();
    let ray_configured = !matches!(ray_source, crate::jobs::ray::AddressSource::Default);
    let ray_source_label = match ray_source {
        crate::jobs::ray::AddressSource::Settings => "Saved address",
        crate::jobs::ray::AddressSource::AstroaiEnv => "ASTROAI_RAY_JOBS_ADDRESS",
        crate::jobs::ray::AddressSource::RayEnv => "RAY_DASHBOARD_URL",
        crate::jobs::ray::AddressSource::Default => "Default localhost:8265",
    };
    // Presence of the credentials file only — whether the token still works is
    // the expanded row's (network) question.
    let or_logged_in = crate::config::credentials_present();

    // Same spellings as the expanded rows' SOURCE_LABELS/MODAL_TOKEN_LABELS
    // in the UI — the collapsed head stays visible above the open row, so the
    // same fact must not read two different ways.
    let source_label = |s: &crate::jobs::huggingface::TokenSource| match s {
        crate::jobs::huggingface::TokenSource::Env => "HF_TOKEN env var",
        crate::jobs::huggingface::TokenSource::OpenresearchEnv => "Token from Environment tab",
        crate::jobs::huggingface::TokenSource::HfCache => "Token from ~/.cache/huggingface/token",
    };
    let mut targets = json!([
        {
            "id": "local",
            "configured": true,
            "summary": "Runs as a detached process on this machine",
        },
        {
            "id": "ssh",
            "configured": ssh_hosts > 0,
            "summary": match ssh_hosts {
                0 => "No hosts in ~/.ssh/config".to_string(),
                1 => "1 host in ~/.ssh/config".to_string(),
                n => format!("{n} hosts in ~/.ssh/config"),
            },
        },
        {
            "id": "tinker",
            "configured": tinker.is_some(),
            "fromEnvironmentTab": matches!(tinker.as_ref().map(|(_, source)| source), Some(crate::jobs::tinker::ApiKeySource::OpenresearchEnv)),
            "summary": match tinker.map(|(_, source)| source) {
                Some(crate::jobs::tinker::ApiKeySource::Env) => "TINKER_API_KEY env var",
                Some(crate::jobs::tinker::ApiKeySource::OpenresearchEnv) => "Key from Environment tab",
                None => "No API key",
            },
        },
        {
            "id": "hf",
            "configured": hf.is_some(),
            "fromEnvironmentTab": matches!(hf.as_ref().map(|(_, source)| source), Some(crate::jobs::huggingface::TokenSource::OpenresearchEnv)),
            "summary": hf.as_ref().map_or_else(
                || "No token".to_string(),
                |(_, s)| source_label(s).to_string(),
            ),
        },
        {
            "id": "modal",
            "configured": modal_source.is_some(),
            "fromEnvironmentTab": modal_source == Some("syncedEnv"),
            "summary": match modal_source {
                Some("env") => "MODAL_TOKEN_ID + MODAL_TOKEN_SECRET env vars",
                Some("syncedEnv") => "Token from Environment tab",
                Some("modalToml") => "Token from ~/.modal.toml",
                _ => "No token",
            },
        },
        {
            "id": "k8s",
            "configured": k8s_settings.is_some(),
            "summary": k8s_settings.as_ref().map_or_else(
                || "No context selected".to_string(),
                |s| format!(
                    "Context {} / namespace {}",
                    s.context.as_deref().unwrap_or("(kubectl default)"),
                    s.namespace,
                ),
            ),
        },
        {
            "id": "slurm",
            "configured": slurm_host.is_some(),
            "summary": slurm_host.as_ref().map_or_else(
                || "No login node configured".to_string(),
                |h| match slurm_settings.as_ref().and_then(|s| s.partition.as_deref()) {
                    Some(partition) => format!("Login node {h} / partition {partition}"),
                    None => format!("Login node {h}"),
                },
            ),
        },
        {
            "id": "ray",
            "configured": ray_configured,
            "summary": if ray_configured {
                format!("{ray_source_label} ({ray_resolved})")
            } else {
                ray_source_label.to_string()
            },
        },
        {
            "id": "openresearch",
            // Signed in alone would be a green light on a backend that can't
            // connect — the box authorizes your *registered* keys, so one of
            // them has to be on this machine too.
            "configured": or_logged_in && ssh == SshReadiness::Ready,
            "unverified": or_logged_in && matches!(ssh, SshReadiness::Unverified { .. }),
            "summary": openresearch_summary(or_logged_in, &ssh),
        },
    ]);
    if let Some(targets) = targets.as_array_mut() {
        for target in targets {
            if let Some(target) = target.as_object_mut() {
                target.insert("enabled".to_string(), Value::Bool(true));
                target.insert("disabledReason".to_string(), Value::Null);
            }
        }
    }
    json!({
        "defaultBackend": default_backend.unwrap_or("local"),
        "defaultFlavor": default_flavor,
        "configuredDefaultBackend": default_backend,
        "configuredDefaultFlavor": default_flavor,
        "targets": targets,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ComputeSettingsQuery {
    project_id: Option<String>,
}

pub(super) async fn compute_settings(Query(query): Query<ComputeSettingsQuery>) -> ApiResult {
    let ssh = openresearch_ssh_readiness().await;
    let _project_id = query.project_id;
    // fs/env probes only, but keep them off the async runtime anyway.
    let payload =
        tokio::task::spawn_blocking(move || -> Result<Value> { Ok(compute_settings_json(ssh)) })
            .await
            .map_err(|e| ApiError::from(anyhow!("compute settings task failed: {e}")))??;
    Ok(Json(payload))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SetComputeDefaultReq {
    /// `None`/absent clears the default (and its flavor with it).
    backend: Option<String>,
    flavor: Option<String>,
    project_id: Option<String>,
}

/// Persist the default compute target. An *unconfigured* backend is allowed
/// (config state fluctuates outside orx; the UI warns instead) — only unknown
/// backends and meaningless flavors are rejected.
pub(super) async fn set_compute_default(Json(req): Json<SetComputeDefaultReq>) -> ApiResult {
    let _project_id = req.project_id;
    let backend = req
        .backend
        .map(|b| b.trim().to_string())
        .filter(|b| !b.is_empty());
    let flavor = req
        .flavor
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty());
    if let Some(b) = &backend {
        local::validate_compute_default(b, flavor.as_deref()).map_err(bad_request)?;
    }
    // Picking openresearch as the default is the moment to answer "will this
    // actually work?", so the row that comes back is honest about the SSH key.
    let ssh = openresearch_ssh_readiness().await;
    // Validation already ran above, so a failure in here is a server-side
    // fault (io error, corrupt settings.json refusal) — surface it as 500 via
    // the plain ApiError conversion, not as a 400 blaming the request.
    let payload = tokio::task::spawn_blocking(move || -> Result<Value> {
        crate::config::set_compute_default(backend, flavor)?;
        Ok(compute_settings_json(ssh))
    })
    .await
    .map_err(|e| ApiError::from(anyhow!("compute default task failed: {e}")))??;
    Ok(Json(payload))
}

/// The "This machine" row's expanded detail: detected hardware. Subprocess
/// probes (hostname, sysctl, nvidia-smi) — blocking, so spawned.
pub(super) async fn local_machine_settings() -> ApiResult {
    let hw = tokio::task::spawn_blocking(crate::jobs::localbox::hardware_info)
        .await
        .map_err(|e| ApiError::from(anyhow!("hardware probe task failed: {e}")))?;
    Ok(Json(json!(hw)))
}

/// The OpenResearch row's expanded detail. Network calls are fine here (the
/// row is open) but each is individually best-effort — an offline machine
/// still renders "signed in, status unknown" instead of an error page.
pub(super) async fn openresearch_settings() -> ApiResult {
    let Some(creds) = crate::config::load_credentials().await? else {
        return Ok(Json(json!({
            "loggedIn": false,
            "apiUrl": null,
            "orgs": [],
            "sshKeyStatus": "unknown",
            "error": null,
        })));
    };
    let mut error: Option<String> = None;
    let orgs = match crate::client::list_orgs(&creds).await {
        Ok(o) => o.orgs.into_iter().map(|o| o.name).collect::<Vec<_>>(),
        Err(e) => {
            error = Some(e.to_string());
            Vec::new()
        }
    };
    // "Registered" alone is a misleading green: a key registered from another
    // laptop leaves this machine unable to reach any box. Report whether the
    // private half is actually here.
    use crate::local::ssh_identity::{preferred_local, tilde, KeyStatus};
    // Hand back the .pub we actually found so the note can name a real file
    // rather than guessing at ~/.ssh/id_ed25519.pub.
    let mut ssh_key_path: Option<String> = None;
    let mut note_key = |local: &[crate::local::ssh_identity::LocalKey]| {
        ssh_key_path = preferred_local(local)
            .and_then(|k| k.path.as_deref())
            .map(tilde);
    };
    let ssh_key_status = match crate::local::ssh_identity::check(&creds).await {
        KeyStatus::Matched => "matched",
        KeyStatus::NoLocalMatch { local, .. } => {
            note_key(&local);
            "no_local_match"
        }
        KeyStatus::NoneRegistered { local } => {
            note_key(&local);
            "none_registered"
        }
        KeyStatus::Unknown { reason } => {
            error.get_or_insert(reason);
            "unknown"
        }
    };
    Ok(Json(json!({
        "loggedIn": true,
        "apiUrl": creds.api_url,
        "orgs": orgs,
        "sshKeyStatus": ssh_key_status,
        "sshKeyPath": ssh_key_path,
        "error": error,
    })))
}

fn cli_result(result: ApiResult) -> Result<Value> {
    result
        .map(|Json(value)| value)
        .map_err(|ApiError(_, message)| anyhow!(message))
}

pub(crate) async fn status() -> Result<Value> {
    cli_result(compute_settings(Query(ComputeSettingsQuery { project_id: None })).await)
}

pub(crate) async fn show(backend: &str) -> Result<Value> {
    let mut value = cli_result(match backend {
        "local" => local_machine_settings().await,
        "ssh" => ssh_settings().await,
        "slurm" => slurm_settings().await,
        "ray" => ray_settings().await,
        "hf" => Ok(hf_settings().await),
        "modal" => modal_settings().await,
        "tinker" => tinker_settings().await,
        "k8s" => k8s_settings().await,
        "openresearch" => openresearch_settings().await,
        _ => return Err(anyhow!("Unknown compute backend: {backend}")),
    })?;
    if backend == "ssh" {
        if let Some(hosts) = value["hosts"].as_array_mut() {
            for host in hosts {
                if let Some(alias) = host["host"].as_str() {
                    host["masterRunning"] = ssh_master_status(Query(SshPreflightReq {
                        host: alias.to_owned(),
                        container: None,
                    }))
                    .await
                    .ok()
                    .map(|Json(value)| value["running"].clone())
                    .unwrap_or(Value::Null);
                }
            }
        }
    }
    Ok(value)
}

pub(crate) async fn default(backend: Option<String>, flavor: Option<String>) -> Result<Value> {
    cli_result(
        set_compute_default(Json(SetComputeDefaultReq {
            backend,
            flavor,
            project_id: None,
        }))
        .await,
    )
}

pub(crate) async fn configure(backend: &str, body: Value) -> Result<Value> {
    if body.get("clearCredentials") == Some(&Value::Bool(true)) {
        let keys: &[&str] = match backend {
            "hf" => &["HF_TOKEN"],
            "tinker" => &["TINKER_API_KEY"],
            "modal" => &["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"],
            _ => return Err(anyhow!("No saved credentials for {backend}.")),
        };
        for key in keys {
            crate::config::remove_synced_env_var(key)?;
        }
        let mut value = show(backend)
            .await
            .unwrap_or_else(|error| json!({"error": error.to_string()}));
        value["clearedSavedCredentials"] = json!(true);
        return Ok(value);
    }
    cli_result(match backend {
        "ssh" => {
            // Validate both changes before persisting either.
            if let Some(host) = body.get("defaultHost").and_then(Value::as_str) {
                require_configured_ssh_host(host)
                    .map_err(|ApiError(_, message)| anyhow!(message))?;
            }
            if body.get("container").is_some() {
                let req: SaveSshSettingsReq = serde_json::from_value(
                    json!({"host": body["host"], "container": body["container"]}),
                )?;
                cli_result(save_ssh_settings(Json(req)).await)?;
            }
            if let Some(host) = body.get("defaultHost") {
                cli_result(
                    save_ssh_default(Json(SaveSshDefaultReq {
                        host: serde_json::from_value(host.clone())?,
                    }))
                    .await,
                )?;
            }
            ssh_settings().await
        }
        "slurm" => set_slurm_settings(Json(serde_json::from_value(body)?)).await,
        "ray" => set_ray_settings(Json(serde_json::from_value(body)?)).await,
        "k8s" => set_k8s_settings(Json(serde_json::from_value(body)?)).await,
        "hf" => set_hf_token(Json(serde_json::from_value(body)?)).await,
        "modal" => set_modal_token(Json(serde_json::from_value(body)?)).await,
        "tinker" => set_tinker_key(Json(serde_json::from_value(body)?)).await,
        _ => {
            return Err(anyhow!(
                "{backend} has no saved configuration; use compute connect or default set."
            ))
        }
    })
}

pub(crate) async fn check(
    backend: &str,
    host: Option<String>,
    container: Option<String>,
    no_container: bool,
    address: Option<String>,
) -> Result<Value> {
    let mut value = match backend {
        "ssh" => {
            let settings = crate::config::ssh_settings()?;
            let host = host
                .or(settings.default_host)
                .ok_or_else(|| anyhow!("Pass --host or configure a default SSH host."))?;
            let container = if no_container {
                None
            } else {
                container.or_else(|| settings.hosts.get(&host).and_then(|s| s.container.clone()))
            };
            cli_result(ssh_preflight(Json(SshPreflightReq { host, container })).await)?
        }
        "slurm" => {
            let host = host
                .or(slurm::load_settings()?.unwrap_or_default().host)
                .ok_or_else(|| anyhow!("Pass --host or configure a Slurm host."))?;
            cli_result(slurm_preflight(Json(SlurmPreflightReq { host })).await)?
        }
        "ray" => cli_result(ray_preflight(Json(RayPreflightReq { address })).await)?,
        "modal" => match modal::preflight().await {
            Ok(()) => json!({"ready": true}),
            Err(error) => json!({"ready": false, "error": error.to_string()}),
        },
        _ => show(backend)
            .await
            .unwrap_or_else(|error| json!({"error": error.to_string()})),
    };
    let ready = match backend {
        "local" => true,
        "ssh" => {
            value["reachable"] == true
                && value["toolsFound"] == true
                && (value["container"].is_null() || value["container"]["ready"] == true)
        }
        "slurm" => {
            value["reachable"] == true && value["toolsFound"] == true && value["slurmFound"] == true
        }
        "ray" => value["reachable"] == true,
        "k8s" => {
            value["preflight"]["reachable"] == true && value["preflight"]["canCreateJobs"] == true
        }
        "hf" => value["validationStatus"] == "valid" && value["jobsWrite"] != false,
        "tinker" => value["validationStatus"] == "valid",
        "openresearch" => {
            value["loggedIn"] == true
                && value["sshKeyStatus"] == "matched"
                && value["error"].is_null()
        }
        "modal" => value["ready"] == true,
        _ => false,
    };
    value["ready"] = json!(ready);
    Ok(value)
}

pub(crate) async fn read_ssh_config() -> Result<Value> {
    cli_result(ssh_config().await)
}
pub(crate) async fn write_ssh_config(content: String, previous_content: String) -> Result<Value> {
    cli_result(
        save_ssh_config(Json(SaveSshConfigReq {
            content,
            previous_content,
        }))
        .await,
    )
}
