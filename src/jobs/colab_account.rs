//! The signed-in Colab account: plan, compute-unit balance, which accelerators
//! the plan can assign, and what each one costs per hour.
//!
//! Reuses the OAuth token google-colab-cli stores after its sign-in. The token
//! is refreshed in memory only; the CLI's token file is never written. Every
//! call is best-effort, so a missing scope or an offline machine leaves the
//! affected fields empty instead of failing the whole probe.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

use super::colab::{token_path, ACCELERATORS};

pub(super) const COLAB_DOMAIN: &str = "https://colab.research.google.com";
const COLAB_API: &str = "https://colaboratory.googleapis.com/v1beta";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const DEFAULT_TOKEN_URI: &str = "https://oauth2.googleapis.com/token";
pub(super) const XSSI_PREFIX: &str = ")]}'";
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The settings page re-reads the account on every open; Colab's balance moves
/// slowly enough that a minute-old answer is fine.
const ACCOUNT_TTL: Duration = Duration::from_secs(60);

/// Compute units per hour that Colab does not publish. Third-party
/// measurements (March 2026); a rate measured on this account replaces them.
const ESTIMATED_RATES: &[(&str, f64)] = &[("t4", 1.19), ("l4", 1.71), ("a100", 5.40), ("g4", 8.71)];

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub email: Option<String>,
    /// `free`, `pro`, or `pro_plus`.
    pub tier: Option<String>,
    /// Compute units left on the account.
    pub balance: Option<f64>,
    /// Units per hour the account's active runtimes are burning right now.
    pub rate_hourly: Option<f64>,
    pub active_runtimes: Option<u32>,
    /// Accelerator ids (see `colab::ACCELERATORS`) the plan can assign now.
    /// `None` when Colab did not say.
    pub eligible: Option<Vec<String>>,
    pub rates: Vec<Rate>,
    /// Why some or all fields are missing.
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rate {
    pub id: String,
    pub cu_per_hour: f64,
    /// Measured on this account rather than estimated.
    pub measured: bool,
}

/// Drop the cached account so the next read shows a released runtime.
pub(super) async fn invalidate() {
    *cache().lock().await = None;
}

fn cache() -> &'static Mutex<Option<(Instant, Account)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Account)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn access_token_cache() -> &'static Mutex<Option<(Instant, String)>> {
    static TOKEN: OnceLock<Mutex<Option<(Instant, String)>>> = OnceLock::new();
    TOKEN.get_or_init(|| Mutex::new(None))
}

/// The account, or `None` when the Colab CLI has not signed in.
pub async fn account(force: bool) -> Option<Account> {
    if !token_path().is_some_and(|path| path.is_file()) {
        return None;
    }
    let mut cached = cache().lock().await;
    if !force {
        if let Some((at, account)) = cached.as_ref() {
            if at.elapsed() < ACCOUNT_TTL {
                return Some(account.clone());
            }
        }
    }
    let account = fetch().await;
    *cached = Some((Instant::now(), account.clone()));
    Some(account)
}

async fn fetch() -> Account {
    let mut account = Account {
        rates: rates(&load_measured()),
        ..Account::default()
    };
    let token = match access_token().await {
        Ok(token) => token,
        Err(error) => {
            account.error = Some(error);
            return account;
        }
    };
    let client = match crate::net::remote_client().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            account.error = Some(error.to_string());
            return account;
        }
    };
    let tun = |path: &str| format!("{COLAB_DOMAIN}/tun/m/{path}?authuser=0");
    let (ccu_url, assignments_url) = (tun("ccu-info"), tun("assignments"));
    let subscription_url = format!("{COLAB_API}/subscription");
    let specs_url = format!("{COLAB_API}/runtimespecs");
    let (ccu, subscription, specs, assignments, userinfo) = tokio::join!(
        get_json(&client, &token, &ccu_url),
        get_json(&client, &token, &subscription_url),
        get_json(&client, &token, &specs_url),
        get_json(&client, &token, &assignments_url),
        get_json(&client, &token, USERINFO_URL),
    );

    match &ccu {
        Ok(ccu) => {
            account.balance = ccu["currentBalance"].as_f64();
            account.rate_hourly = ccu["consumptionRateHourly"].as_f64();
            account.active_runtimes = ccu["assignmentsCount"].as_u64().map(|n| n as u32);
        }
        Err(error) => account.error = Some(format!("Could not read compute units: {error}")),
    }
    if let Ok(subscription) = &subscription {
        account.tier = subscription["tier"].as_str().and_then(tier_name);
    }
    account.eligible = specs
        .as_ref()
        .ok()
        .and_then(eligible_from_specs)
        .or_else(|| ccu.as_ref().ok().and_then(eligible_from_ccu));
    if let Ok(userinfo) = &userinfo {
        account.email = userinfo["email"].as_str().map(str::to_string);
    }

    // With one runtime up, the account's burn rate is that accelerator's rate.
    if let (Ok(assignments), Some(rate)) = (&assignments, account.rate_hourly) {
        if let Some(id) = single_assignment(assignments).filter(|_| rate > 0.0) {
            let mut measured = load_measured();
            measured.insert(id, rate);
            save_measured(&measured);
            account.rates = rates(&measured);
        }
    }
    account
}

fn tier_name(tier: &str) -> Option<String> {
    match tier {
        "SUBSCRIPTION_TIER_FREE" => Some("free".into()),
        "SUBSCRIPTION_TIER_PRO" => Some("pro".into()),
        "SUBSCRIPTION_TIER_PRO_PLUS" => Some("pro_plus".into()),
        _ => None,
    }
}

/// Map a Colab accelerator name (`T4`, `NONE`, `V5E1`) to an orx flavor id.
pub(super) fn accelerator_id(name: &str) -> Option<String> {
    let id = if name.eq_ignore_ascii_case("none") {
        "cpu".to_string()
    } else {
        name.to_ascii_lowercase()
    };
    ACCELERATORS.iter().any(|a| a.id == id).then_some(id)
}

/// `runtimespecs` lists every spec with an `eligible` flag; an accelerator is
/// usable when any of its machine shapes is.
fn eligible_from_specs(body: &Value) -> Option<Vec<String>> {
    let specs = body["runtimeSpecs"].as_array()?;
    let mut ids: Vec<String> = specs
        .iter()
        .filter(|spec| spec["eligible"].as_bool() == Some(true))
        .filter_map(|spec| spec["key"]["accelerator"].as_str().and_then(accelerator_id))
        .collect();
    ids.sort_by_key(|id| ACCELERATORS.iter().position(|a| a.id == id));
    ids.dedup();
    Some(ids)
}

/// Older `ccu-info` responses carry `eligibleGpus` / `eligibleTpus` name lists.
fn eligible_from_ccu(body: &Value) -> Option<Vec<String>> {
    let mut found = false;
    let mut ids = Vec::new();
    for key in ["eligibleGpus", "eligibleTpus"] {
        if let Some(names) = body[key].as_array() {
            found = true;
            ids.extend(
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .filter_map(accelerator_id),
            );
        }
    }
    found.then_some(ids)
}

fn single_assignment(body: &Value) -> Option<String> {
    match body["assignments"].as_array()?.as_slice() {
        [only] => only["accelerator"].as_str().and_then(accelerator_id),
        _ => None,
    }
}

fn rates(measured: &BTreeMap<String, f64>) -> Vec<Rate> {
    ACCELERATORS
        .iter()
        .filter_map(|a| {
            if let Some(rate) = measured.get(a.id) {
                return Some(Rate {
                    id: a.id.into(),
                    cu_per_hour: *rate,
                    measured: true,
                });
            }
            ESTIMATED_RATES
                .iter()
                .find(|(id, _)| *id == a.id)
                .map(|(id, rate)| Rate {
                    id: (*id).into(),
                    cu_per_hour: *rate,
                    measured: false,
                })
        })
        .collect()
}

fn measured_path() -> PathBuf {
    crate::store::data_dir().join("colab").join("rates.json")
}

fn load_measured() -> BTreeMap<String, f64> {
    std::fs::read(measured_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_measured(rates: &BTreeMap<String, f64>) {
    let path = measured_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(rates) {
        let _ = std::fs::write(path, bytes);
    }
}

pub(super) async fn get_json(
    client: &reqwest::Client,
    token: &str,
    url: &str,
) -> Result<Value, String> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/json")
        .header("X-Colab-Client-Agent", "colab-cli")
        .send()
        .await
        .map_err(|e| e.without_url().to_string())?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| e.without_url().to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let body = text.strip_prefix(XSSI_PREFIX).unwrap_or(&text);
    serde_json::from_str(body.trim_start()).map_err(|e| e.to_string())
}

/// A fresh access token from the CLI's refresh token, kept in memory only.
pub(super) async fn access_token() -> Result<String, String> {
    let mut cached = access_token_cache().lock().await;
    if let Some((expires, token)) = cached.as_ref() {
        if Instant::now() < *expires {
            return Ok(token.clone());
        }
    }
    let path = token_path().ok_or("No home directory.")?;
    let stored: Value = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or("The Colab CLI's sign-in could not be read. Sign in again with `orx compute connect colab`.")?;
    let field = |name: &str| stored[name].as_str().filter(|v| !v.is_empty());
    let (Some(refresh), Some(client_id)) = (field("refresh_token"), field("client_id")) else {
        // No refresh token: use the stored access token while it lasts.
        return field("token")
            .map(str::to_string)
            .ok_or_else(|| "The Colab CLI's sign-in has no token. Sign in again with `orx compute connect colab`.".into());
    };
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
    ];
    if let Some(secret) = field("client_secret") {
        form.push(("client_secret", secret));
    }
    let client = crate::net::remote_client()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .post(field("token_uri").unwrap_or(DEFAULT_TOKEN_URI))
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("Could not refresh the Colab sign-in: {}", e.without_url()))?;
    if !response.status().is_success() {
        return Err(format!(
            "Google refused the Colab sign-in (HTTP {}). Sign in again with `orx compute connect colab`.",
            response.status()
        ));
    }
    let body: Value = response.json().await.map_err(|e| e.to_string())?;
    let token = body["access_token"]
        .as_str()
        .ok_or("Google returned no access token.")?
        .to_string();
    let lifetime = body["expires_in"]
        .as_u64()
        .unwrap_or(3600)
        .saturating_sub(120);
    *cached = Some((
        Instant::now() + Duration::from_secs(lifetime),
        token.clone(),
    ));
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn eligibility_comes_from_runtime_specs_in_accelerator_order() {
        let body = json!({"runtimeSpecs": [
            {"key": {"variant": "VARIANT_GPU", "accelerator": "A100", "shape": "SHAPE_STANDARD"}, "eligible": true},
            {"key": {"variant": "VARIANT_GPU", "accelerator": "H100", "shape": "SHAPE_STANDARD"}, "eligible": false},
            {"key": {"variant": "VARIANT_GPU", "accelerator": "T4", "shape": "SHAPE_HIGHMEM"}, "eligible": true},
            {"key": {"variant": "VARIANT_GPU", "accelerator": "T4", "shape": "SHAPE_STANDARD"}, "eligible": true},
            {"key": {"variant": "VARIANT_CPU", "accelerator": "NONE", "shape": "SHAPE_STANDARD"}, "eligible": true},
            {"key": {"variant": "VARIANT_GPU", "accelerator": "X9", "shape": "SHAPE_STANDARD"}, "eligible": true}
        ]});
        assert_eq!(eligible_from_specs(&body).unwrap(), ["cpu", "t4", "a100"]);
    }

    #[test]
    fn eligibility_falls_back_to_ccu_name_lists() {
        let body = json!({"eligibleGpus": ["T4", "L4"], "eligibleTpus": ["V5E1"]});
        assert_eq!(eligible_from_ccu(&body).unwrap(), ["t4", "l4", "v5e1"]);
        assert!(eligible_from_ccu(&json!({"currentBalance": 3.0})).is_none());
    }

    #[test]
    fn a_measured_rate_replaces_the_estimate() {
        let measured = BTreeMap::from([("t4".to_string(), 1.5), ("h100".to_string(), 9.0)]);
        let rates = rates(&measured);
        let t4 = rates.iter().find(|r| r.id == "t4").unwrap();
        assert!(t4.measured && t4.cu_per_hour == 1.5);
        assert!(rates.iter().any(|r| r.id == "h100" && r.measured));
        assert!(rates.iter().any(|r| r.id == "l4" && !r.measured));
    }

    #[test]
    fn only_a_single_runtime_attributes_the_burn_rate() {
        assert_eq!(
            single_assignment(&json!({"assignments": [{"accelerator": "L4"}]})).as_deref(),
            Some("l4")
        );
        assert!(single_assignment(
            &json!({"assignments": [{"accelerator": "L4"}, {"accelerator": "T4"}]})
        )
        .is_none());
    }
}
