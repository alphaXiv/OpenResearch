//! How much a Google Cloud project can still spend.
//!
//! Google exposes no API for a billing account's remaining credit, so the user
//! copies it once from Billing → Credits in the console. orx then subtracts what
//! the project has spent since: the exact cost from a BigQuery billing export
//! when one is configured, otherwise an estimate from the Google Cloud runs orx
//! launched (run time × list price).

use serde::Serialize;
use serde_json::Value;
use tokio::process::Command;

use super::gcp::{self, GcpSettings};
use crate::error::{anyhow, Result};

/// Spot VMs bill at roughly a third of on-demand.
const SPOT_FACTOR: f64 = 0.35;
/// Per vCPU-hour for a CPU machine type orx has no table entry for.
const CPU_USD_PER_VCPU_HOUR: f64 = 0.04;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingAccount {
    pub id: String,
    pub name: Option<String>,
    pub open: Option<bool>,
    /// Billing is turned on for the project.
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    /// What is left to spend: the entered credit minus spend since.
    pub available_usd: f64,
    pub credit_usd: f64,
    /// Unix ms when the credit was entered.
    pub as_of: i64,
    pub spent_usd: f64,
    /// `export` (BigQuery billing export, exact) or `estimate` (orx's runs).
    pub source: &'static str,
    /// Why the export could not be read, when it fell back to the estimate.
    pub error: Option<String>,
}

/// The billing account paying for the project, as far as the user can see it.
pub async fn billing_account(project: &str) -> Option<BillingAccount> {
    let info: Value = serde_json::from_str(
        &gcp::gcloud(&[
            "billing".into(),
            "projects".into(),
            "describe".into(),
            project.into(),
            "--format=json".into(),
        ])
        .await
        .ok()?,
    )
    .ok()?;
    let id = info["billingAccountName"]
        .as_str()
        .and_then(|name| name.strip_prefix("billingAccounts/"))
        .unwrap_or_default()
        .to_string();
    let mut account = BillingAccount {
        enabled: info["billingEnabled"].as_bool().unwrap_or(false),
        id,
        name: None,
        open: None,
    };
    if !account.id.is_empty() {
        if let Some(details) = gcp::gcloud(&[
            "billing".into(),
            "accounts".into(),
            "describe".into(),
            account.id.clone(),
            "--format=json".into(),
        ])
        .await
        .ok()
        .and_then(|out| serde_json::from_str::<Value>(&out).ok())
        {
            account.name = details["displayName"].as_str().map(str::to_string);
            account.open = details["open"].as_bool();
        }
    }
    Some(account)
}

/// The project's remaining credit, when the user has entered one.
pub async fn balance(settings: &GcpSettings, project: &str) -> Option<Balance> {
    let credit = settings.credit_usd?;
    let as_of = settings.credit_as_of.unwrap_or(0);
    let (spent, source, error) = match &settings.billing_export_table {
        Some(table) => match exported_cost(table, project, as_of).await {
            Ok(cost) => (cost, "export", None),
            Err(error) => (
                estimated_cost(settings, as_of),
                "estimate",
                Some(error.to_string()),
            ),
        },
        None => (estimated_cost(settings, as_of), "estimate", None),
    };
    Some(Balance {
        available_usd: credit - spent,
        credit_usd: credit,
        as_of,
        spent_usd: spent,
        source,
        error,
    })
}

/// `project.dataset.table`, or `project:dataset.table`.
pub fn valid_export_table(table: &str) -> bool {
    let parts = table.replace(':', ".");
    parts.split('.').count() == 3
        && parts.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        })
}

/// Gross cost since `as_of` from a BigQuery billing export — what credits pay
/// for before the card is charged.
async fn exported_cost(table: &str, project: &str, as_of: i64) -> Result<f64> {
    if !valid_export_table(table) {
        return Err(anyhow!("'{table}' is not a BigQuery table name."));
    }
    let bq = crate::local::shell_env::find_on_path("bq")
        .or_else(|| gcp::find_cli().and_then(|cli| Some(cli.parent()?.join("bq"))))
        .filter(|path| path.is_file())
        .ok_or_else(|| anyhow!("The `bq` tool from the Google Cloud CLI is not installed."))?;
    let sql = format!(
        "SELECT IFNULL(SUM(cost), 0) AS cost FROM `{}` \
         WHERE project.id = @project AND usage_start_time >= TIMESTAMP_MILLIS(@since)",
        table.replace(':', ".")
    );
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        Command::new(bq)
            .args([
                "--format=json",
                "--quiet",
                "query",
                "--use_legacy_sql=false",
            ])
            .arg(format!("--parameter=project::{project}"))
            .arg(format!("--parameter=since:INT64:{as_of}"))
            .arg(sql)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow!("BigQuery did not answer within a minute."))?
    .map_err(|e| anyhow!("Could not run bq: {e}"))?;
    if !output.status.success() {
        let text = String::from_utf8_lossy(&output.stderr);
        let text = if text.trim().is_empty() {
            String::from_utf8_lossy(&output.stdout)
        } else {
            text
        };
        return Err(anyhow!("BigQuery: {}", text.trim()));
    }
    parse_cost(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| anyhow!("BigQuery returned no cost."))
}

fn parse_cost(out: &str) -> Option<f64> {
    let rows: Value = serde_json::from_str(out.trim()).ok()?;
    let cost = &rows.as_array()?.first()?["cost"];
    cost.as_f64().or_else(|| cost.as_str()?.parse().ok())
}

/// Hourly list price orx assumes for a flavor.
pub fn flavor_usd_per_hour(flavor: &str, spot: bool) -> f64 {
    let Ok(shape) = gcp::parse_flavor(flavor) else {
        return 0.0;
    };
    let on_demand = match shape.gpu {
        Some(id) => gcp::GPUS
            .iter()
            .find(|gpu| gpu.id == id)
            .map_or(0.0, |gpu| gpu.usd_per_hour * f64::from(shape.gpu_count)),
        None => {
            let vcpus = shape
                .machine_type
                .rsplit('-')
                .next()
                .and_then(|n| n.parse::<f64>().ok())
                .unwrap_or(8.0);
            vcpus * CPU_USD_PER_VCPU_HOUR
        }
    };
    if spot {
        on_demand * SPOT_FACTOR
    } else {
        on_demand
    }
}

/// What orx's own Google Cloud runs since `as_of` cost at list price.
fn estimated_cost(settings: &GcpSettings, as_of: i64) -> f64 {
    let Ok(runs) = crate::store::Store::open().and_then(|store| store.list_runs(1000)) else {
        return 0.0;
    };
    let now = crate::store::now_ms();
    runs.iter()
        .filter_map(|run| {
            let backend: Value = serde_json::from_str(&run.backend_json).ok()?;
            if backend["kind"] != "gcp_job" {
                return None;
            }
            let start = run.created_at.max(as_of);
            let end = run.ended_at.unwrap_or(now);
            let hours = (end - start).max(0) as f64 / 3_600_000.0;
            let flavor = backend["flavor"].as_str().unwrap_or("t4");
            Some(hours * flavor_usd_per_hour(flavor, settings.spot))
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavor_prices_scale_with_count_and_spot() {
        assert!((flavor_usd_per_hour("t4", false) - 0.73).abs() < 1e-9);
        assert!((flavor_usd_per_hour("a100:2", false) - 7.34).abs() < 1e-9);
        assert!((flavor_usd_per_hour("t4", true) - 0.73 * SPOT_FACTOR).abs() < 1e-9);
        assert!((flavor_usd_per_hour("n2-standard-16", false) - 0.64).abs() < 1e-9);
        assert_eq!(flavor_usd_per_hour("nonsense", false), 0.0);
    }

    #[test]
    fn export_tables_and_bq_output() {
        assert!(valid_export_table(
            "my-proj.billing.gcp_billing_export_v1_0123"
        ));
        assert!(valid_export_table("my-proj:billing.export"));
        assert!(!valid_export_table("billing.export"));
        assert!(!valid_export_table("p.d.t`; DROP TABLE x; --"));
        assert_eq!(parse_cost(r#"[{"cost":"12.5"}]"#), Some(12.5));
        assert_eq!(parse_cost(r#"[{"cost":3.25}]"#), Some(3.25));
        assert_eq!(parse_cost("[]"), None);
    }
}
