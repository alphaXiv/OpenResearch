//! Ray Jobs client — REST surface for `orx exp run --backend ray`.
//!
//! Talks to a Ray cluster's Jobs / Dashboard API (default
//! `http://127.0.0.1:8265`). Address resolution:
//!   1. Settings file (`$XDG_CONFIG_HOME/openresearch/ray.json`)
//!   2. `ASTROAI_RAY_JOBS_ADDRESS`
//!   3. `RAY_DASHBOARD_URL`
//!   4. `http://127.0.0.1:8265`
//!
//! Paths (Ray 2.x):
//!   POST   {address}/api/jobs/                 submit
//!   GET    {address}/api/jobs/{submission_id}  inspect
//!   GET    {address}/api/jobs/{submission_id}/logs
//!   POST   {address}/api/jobs/{submission_id}/stop

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::{anyhow, Result};

const DEFAULT_ADDRESS: &str = "http://127.0.0.1:8265";

// --- settings ---------------------------------------------------------------

/// User-tunable Ray Jobs defaults at `$XDG_CONFIG_HOME/openresearch/ray.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RaySettings {
    /// Jobs / Dashboard base URL (no trailing slash).
    #[serde(default)]
    pub address: Option<String>,
}

fn settings_path() -> std::path::PathBuf {
    crate::config::config_dir().join("ray.json")
}

pub fn load_settings() -> Result<Option<RaySettings>> {
    let raw = match std::fs::read_to_string(settings_path()) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    match serde_json::from_str::<RaySettings>(&raw) {
        Ok(s) => Ok(Some(s)),
        Err(e) => Err(anyhow!(
            "Unreadable {} ({}). Fix or delete it and reconfigure.",
            settings_path().display(),
            e
        )),
    }
}

pub fn save_settings(settings: &RaySettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(settings)?);
    std::fs::write(&path, body)?;
    Ok(())
}

/// Resolve the Jobs API base URL (no trailing slash).
pub fn resolve_address(explicit: Option<&str>) -> String {
    if let Some(a) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return a.trim_end_matches('/').to_string();
    }
    if let Ok(Some(s)) = load_settings() {
        if let Some(a) = s.address.map(|x| x.trim().to_string()).filter(|s| !s.is_empty()) {
            return a.trim_end_matches('/').to_string();
        }
    }
    for key in ["ASTROAI_RAY_JOBS_ADDRESS", "RAY_DASHBOARD_URL"] {
        if let Ok(a) = std::env::var(key) {
            let a = a.trim().to_string();
            if !a.is_empty() {
                return a.trim_end_matches('/').to_string();
            }
        }
    }
    DEFAULT_ADDRESS.to_string()
}

/// Where the address came from (settings UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSource {
    Settings,
    AstroaiEnv,
    RayEnv,
    Default,
}

pub fn resolve_address_with_source(explicit: Option<&str>) -> (String, AddressSource) {
    if let Some(a) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return (a.trim_end_matches('/').to_string(), AddressSource::Settings);
    }
    if let Ok(Some(s)) = load_settings() {
        if let Some(a) = s
            .address
            .as_ref()
            .map(|x| x.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return (a.trim_end_matches('/').to_string(), AddressSource::Settings);
        }
    }
    if let Ok(a) = std::env::var("ASTROAI_RAY_JOBS_ADDRESS") {
        let a = a.trim().to_string();
        if !a.is_empty() {
            return (
                a.trim_end_matches('/').to_string(),
                AddressSource::AstroaiEnv,
            );
        }
    }
    if let Ok(a) = std::env::var("RAY_DASHBOARD_URL") {
        let a = a.trim().to_string();
        if !a.is_empty() {
            return (a.trim_end_matches('/').to_string(), AddressSource::RayEnv);
        }
    }
    (DEFAULT_ADDRESS.to_string(), AddressSource::Default)
}

// --- flavor → resources -----------------------------------------------------

#[derive(Debug, Clone)]
pub struct RayResources {
    pub cpus: f64,
    pub gpus: f64,
    pub memory_bytes: Option<u64>,
}

impl Default for RayResources {
    fn default() -> Self {
        // cpus=0: do not reserve entrypoint CPUs (avoids Pending on small heads).
        Self {
            cpus: 0.0,
            gpus: 0.0,
            memory_bytes: None,
        }
    }
}

/// Parse an optional flavor into Ray entrypoint resources.
///
/// Examples: `cpu`, `cpu:2`, `gpu`, `gpu:1`, `gpu:1,cpu:4`, `gpu:1,mem:8GiB`.
pub fn parse_flavor(flavor: Option<&str>) -> Result<RayResources> {
    let Some(raw) = flavor.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(RayResources::default());
    };
    let mut out = RayResources::default();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, val) = match part.split_once(':') {
            Some((k, v)) => (k.trim().to_ascii_lowercase(), Some(v.trim())),
            None => (part.to_ascii_lowercase(), None),
        };
        match key.as_str() {
            "cpu" | "cpus" => {
                out.cpus = match val {
                    None | Some("") => 1.0,
                    Some(v) => v.parse::<f64>().map_err(|_| {
                        anyhow!("Invalid Ray flavor CPU count in {raw:?} (got {v:?})")
                    })?,
                };
            }
            "gpu" | "gpus" => {
                out.gpus = match val {
                    None | Some("") => 1.0,
                    Some(v) => v.parse::<f64>().map_err(|_| {
                        anyhow!("Invalid Ray flavor GPU count in {raw:?} (got {v:?})")
                    })?,
                };
            }
            "mem" | "memory" => {
                let v = val.ok_or_else(|| {
                    anyhow!("Ray flavor memory needs a size, e.g. mem:8GiB (got {raw:?})")
                })?;
                out.memory_bytes = Some(parse_memory(v)?);
            }
            other => {
                return Err(anyhow!(
                    "Unknown Ray flavor token {other:?} in {raw:?}. \
                     Use cpu[:N], gpu[:N], and/or mem:<size> (e.g. gpu:1,cpu:4,mem:8GiB)."
                ));
            }
        }
    }
    if out.cpus < 0.0 || out.gpus < 0.0 {
        return Err(anyhow!("Ray flavor cpus/gpus must be non-negative"));
    }
    Ok(out)
}

fn parse_memory(value: &str) -> Result<u64> {
    let value = value.trim();
    let digits: String = value
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let unit: String = value[digits.len()..].trim().to_ascii_lowercase();
    let amount: f64 = digits
        .parse()
        .map_err(|_| anyhow!("Invalid memory size {value:?}"))?;
    let factor: f64 = match unit.as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1000.0,
        "ki" | "kib" => 1024.0,
        "m" | "mb" => 1000f64.powi(2),
        "mi" | "mib" => 1024f64.powi(2),
        "g" | "gb" => 1000f64.powi(3),
        "gi" | "gib" => 1024f64.powi(3),
        "t" | "tb" => 1000f64.powi(4),
        "ti" | "tib" => 1024f64.powi(4),
        _ => {
            return Err(anyhow!(
                "Unknown memory unit in {value:?} (try 8GiB or 512MiB)"
            ))
        }
    };
    let nbytes = (amount * factor) as u64;
    if nbytes == 0 {
        return Err(anyhow!("memory must be positive"));
    }
    Ok(nbytes)
}

// --- HTTP --------------------------------------------------------------------

fn http() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .expect("reqwest client")
    })
}

async fn check(res: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    Err(anyhow!(
        "Ray Jobs {} failed ({}): {}",
        what,
        status.as_u16(),
        body
    ))
}

/// Probe the Jobs API (used by Settings preflight).
pub async fn preflight(address: &str) -> Result<Preflight> {
    let address = address.trim_end_matches('/');
    let version_url = format!("{address}/api/version");
    let res = http()
        .get(&version_url)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Ray Jobs at {address}: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Ray Jobs at {address} returned HTTP {}: {body}",
            status.as_u16()
        ));
    }
    let body: serde_json::Value = res.json().await.unwrap_or(json!({}));
    let ray_version = body
        .get("ray_version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Ok(Preflight {
        reachable: true,
        address: address.to_string(),
        ray_version,
    })
}

#[derive(Debug, Clone)]
pub struct Preflight {
    pub reachable: bool,
    pub address: String,
    pub ray_version: Option<String>,
}

pub struct JobSubmission {
    pub entrypoint: String,
    pub submission_id: String,
    pub resources: RayResources,
    pub env: HashMap<String, String>,
    pub metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitResponse {
    #[serde(default)]
    pub submission_id: Option<String>,
    #[serde(default)]
    pub job_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct JobInfo {
    #[allow(dead_code)]
    pub submission_id: String,
    /// Shared stage vocabulary (`SCHEDULING` / `RUNNING` / `COMPLETED` / …).
    pub stage: String,
    pub message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawJobStatus {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    submission_id: Option<String>,
}

fn map_ray_status(raw: &str) -> String {
    match raw.to_ascii_uppercase().as_str() {
        "PENDING" => "SCHEDULING".into(),
        "RUNNING" => "RUNNING".into(),
        "SUCCEEDED" | "COMPLETED" => "COMPLETED".into(),
        "FAILED" | "ERROR" => "ERROR".into(),
        "STOPPED" | "STOPPING" | "CANCELLED" | "CANCELED" => "CANCELED".into(),
        other => other.to_string(),
    }
}

pub async fn run_job(address: &str, spec: &JobSubmission) -> Result<SubmitResponse> {
    let address = address.trim_end_matches('/');
    let env = super::default_unbuffered(&spec.env);
    let mut body = json!({
        "entrypoint": spec.entrypoint,
        "submission_id": spec.submission_id,
        "runtime_env": { "env_vars": env },
        "metadata": spec.metadata,
        "entrypoint_num_cpus": spec.resources.cpus,
        "entrypoint_num_gpus": spec.resources.gpus,
    });
    if let Some(mem) = spec.resources.memory_bytes {
        body["entrypoint_memory"] = json!(mem);
    }
    let res = http()
        .post(format!("{address}/api/jobs/"))
        .json(&body)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Ray Jobs at {address}: {e}"))?;
    let job: SubmitResponse = check(res, "job submit").await?.json().await?;
    Ok(job)
}

pub async fn inspect_job(address: &str, submission_id: &str) -> Result<JobInfo> {
    let address = address.trim_end_matches('/');
    let res = http()
        .get(format!("{address}/api/jobs/{submission_id}"))
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Ray Jobs at {address}: {e}"))?;
    let raw: RawJobStatus = check(res, "job inspect").await?.json().await?;
    let status = raw.status.unwrap_or_else(|| "PENDING".into());
    Ok(JobInfo {
        submission_id: raw
            .submission_id
            .unwrap_or_else(|| submission_id.to_string()),
        stage: map_ray_status(&status),
        message: raw.message,
    })
}

pub async fn stop_job(address: &str, submission_id: &str) -> Result<()> {
    let address = address.trim_end_matches('/');
    let res = http()
        .post(format!("{address}/api/jobs/{submission_id}/stop"))
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Ray Jobs at {address}: {e}"))?;
    check(res, "job stop").await?;
    Ok(())
}

/// Fetch the full driver log text (Ray returns JSON `{"logs":"..."}` or plain text).
pub async fn fetch_logs(address: &str, submission_id: &str) -> Result<String> {
    let address = address.trim_end_matches('/');
    let res = http()
        .get(format!("{address}/api/jobs/{submission_id}/logs"))
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Ray Jobs at {address}: {e}"))?;
    let res = check(res, "job logs").await?;
    let ct = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.contains("json") {
        let body: serde_json::Value = res.json().await?;
        Ok(body
            .get("logs")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string())
    } else {
        Ok(res.text().await.unwrap_or_default())
    }
}

pub fn job_url(address: &str, submission_id: &str) -> String {
    let address = address.trim_end_matches('/');
    format!("{address}/#/jobs/{submission_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavor_defaults_and_gpu() {
        let d = parse_flavor(None).unwrap();
        assert_eq!(d.cpus, 0.0);
        assert_eq!(d.gpus, 0.0);
        let g = parse_flavor(Some("gpu:2,cpu:4,mem:8GiB")).unwrap();
        assert_eq!(g.gpus, 2.0);
        assert_eq!(g.cpus, 4.0);
        assert_eq!(g.memory_bytes, Some(8 * 1024 * 1024 * 1024));
    }

    #[test]
    fn address_prefers_explicit() {
        let (a, src) = resolve_address_with_source(Some("http://example:8265/"));
        assert_eq!(a, "http://example:8265");
        assert_eq!(src, AddressSource::Settings);
    }
}
