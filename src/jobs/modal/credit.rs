//! Modal credit tracking owned by the tuandang304 fork: billing report,
//! saved starting credit, remaining balance and list prices per flavor. Kept
//! out of `modal.rs` so upstream edits there merge cleanly.

use super::*;

/// The workspace's billing rates and this month's spend, as the launcher's
/// `billing` subcommand reports them, plus `spentSince` (metered cost from
/// `since_ms` on, in whole hours) when asked. Never provisions the managed
/// env: a machine without the `modal` package gets an error instead.
pub async fn billing(since_ms: Option<i64>) -> Result<Value> {
    let since = since_ms.map(|ms| ms.to_string());
    let mut args = vec!["billing"];
    args.extend(since.as_deref());
    let out = launcher_capture(&args, None).await?;
    serde_json::from_slice(&out).map_err(|e| anyhow!("Modal billing output was not JSON: {e}"))
}

// --- credit ---------------------------------------------------------------------

/// Modal's API reports spend but not the credit left on the workspace, so the
/// user copies it once from the Modal dashboard (Settings → Usage & Billing)
/// and orx subtracts what the workspace has spent since.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModalSettings {
    /// Credit the workspace had left, in USD, when it was entered.
    #[serde(default)]
    pub credit_usd: Option<f64>,
    /// When `credit_usd` was entered (Unix ms); spend is counted from here.
    #[serde(default)]
    pub credit_as_of: Option<i64>,
}

fn settings_path() -> PathBuf {
    crate::config::config_dir().join("modal.json")
}

pub fn load_settings() -> Result<Option<ModalSettings>> {
    let raw = match std::fs::read_to_string(settings_path()) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    serde_json::from_str(&raw).map(Some).map_err(|e| {
        anyhow!(
            "Unreadable {} ({}). Fix or delete it and reconfigure.",
            settings_path().display(),
            e
        )
    })
}

pub fn save_settings(settings: &ModalSettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(settings)?),
    )?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModalBalance {
    /// What is left to spend: the entered credit minus spend since.
    pub available_usd: f64,
    pub credit_usd: f64,
    /// Unix ms when the credit was entered.
    pub as_of: i64,
    pub spent_usd: f64,
    /// `report` (Modal's own billing report) or `estimate` (orx's runs at
    /// list price, when the report could not be read).
    pub source: &'static str,
    /// Why the report could not be read, when it fell back to the estimate.
    pub error: Option<String>,
}

/// The workspace's remaining credit, when the user has entered one. `billing`
/// is the launcher's answer for `billing(Some(as_of))`, or the error reading it.
pub fn balance(
    settings: &ModalSettings,
    billing: std::result::Result<&Value, String>,
) -> Option<ModalBalance> {
    let credit = settings.credit_usd?;
    let as_of = settings.credit_as_of.unwrap_or(0);
    let reported = billing.as_ref().ok().and_then(|billing| {
        billing["spentSince"]
            .as_str()
            .and_then(|s| s.trim().parse::<f64>().ok())
    });
    let (spent, source, error) = match reported {
        Some(spent) => (spent, "report", None),
        None => (
            estimated_cost(as_of),
            "estimate",
            match billing {
                Ok(billing) => billing["reportError"]
                    .as_str()
                    .or(billing["error"].as_str())
                    .map(str::to_string),
                Err(error) => Some(error),
            },
        ),
    };
    Some(ModalBalance {
        available_usd: credit - spent,
        credit_usd: credit,
        as_of,
        spent_usd: spent,
        source,
        error,
    })
}

/// What orx's own Modal runs since `as_of` cost at list price.
fn estimated_cost(as_of: i64) -> f64 {
    let Ok(runs) = crate::store::Store::open().and_then(|store| store.list_runs(1000)) else {
        return 0.0;
    };
    let now = crate::store::now_ms();
    runs.iter()
        .filter_map(|run| {
            let backend: Value = serde_json::from_str(&run.backend_json).ok()?;
            if backend["kind"] != "modal_job" {
                return None;
            }
            let start = run.created_at.max(as_of);
            let end = run.ended_at.unwrap_or(now);
            let hours = (end - start).max(0) as f64 / 3_600_000.0;
            Some(hours * flavor_usd_per_hour(backend["flavor"].as_str().unwrap_or("cpu")))
        })
        .sum()
}

/// Modal's published per-hour GPU prices (modal.com/pricing), used when the
/// workspace's own rates cannot be read. `(flavor, vram_gb, usd_per_hour)`.
pub const LIST_PRICES: &[(&str, f64, f64)] = &[
    ("T4", 16.0, 0.59),
    ("L4", 24.0, 0.80),
    ("A10G", 24.0, 1.10),
    ("L40S", 48.0, 1.95),
    ("A100-40GB", 40.0, 2.10),
    ("A100-80GB", 80.0, 2.50),
    ("H100", 80.0, 3.95),
    ("H200", 141.0, 4.54),
    ("B200", 180.0, 6.25),
];

/// A sandbox's default CPU and memory, roughly, per hour.
const CPU_USD_PER_HOUR: f64 = 0.10;

/// List price per hour for a `--flavor` value such as `a100-80gb` or `h100:2`.
pub fn flavor_usd_per_hour(flavor: &str) -> f64 {
    let Some(gpu) = resolve_flavor(flavor).gpu else {
        return CPU_USD_PER_HOUR;
    };
    let (model, count) = match gpu.split_once(':') {
        Some((model, count)) => (model, count.trim().parse::<f64>().unwrap_or(1.0)),
        None => (gpu.as_str(), 1.0),
    };
    let model = match model {
        "A100" => "A100-40GB",
        "A10" => "A10G",
        other => other,
    };
    LIST_PRICES
        .iter()
        .find(|(name, _, _)| *name == model)
        .map_or(0.0, |(_, _, price)| price * count)
        + CPU_USD_PER_HOUR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_prices_cover_counts_aliases_and_cpu() {
        assert!((flavor_usd_per_hour("cpu") - CPU_USD_PER_HOUR).abs() < 1e-9);
        assert!((flavor_usd_per_hour("h100:2") - (2.0 * 3.95 + CPU_USD_PER_HOUR)).abs() < 1e-9);
        assert!((flavor_usd_per_hour("a100") - (2.10 + CPU_USD_PER_HOUR)).abs() < 1e-9);
        assert!((flavor_usd_per_hour("a10") - (1.10 + CPU_USD_PER_HOUR)).abs() < 1e-9);
    }

    #[test]
    fn balance_subtracts_reported_spend() {
        let settings = ModalSettings {
            credit_usd: Some(30.0),
            credit_as_of: Some(1),
        };
        let billing = json!({"spentSince": "4.25"});
        let left = balance(&settings, Ok(&billing)).unwrap();
        assert_eq!(left.source, "report");
        assert!((left.available_usd - 25.75).abs() < 1e-9);
        assert!(left.error.is_none());
        assert!(balance(&ModalSettings::default(), Ok(&billing)).is_none());
    }
}
