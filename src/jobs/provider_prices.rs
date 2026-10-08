//! What each compute provider charges, and how much credit is left where a
//! provider exposes it, so an agent picking `--backend`/`--flavor` can avoid
//! a provider whose balance would run out mid-experiment.
//!
//! Every source is best-effort: a provider that cannot be reached keeps its
//! row with an `error`, and published list prices stand in where a live rate
//! is unavailable (marked `estimated`).

use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::Mutex;

/// Prices barely move; the Colab balance is the only fast-moving field and
/// has its own one-minute cache.
const OVERVIEW_TTL: Duration = Duration::from_secs(5 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The Modal launcher starts a Python interpreter and dials Modal's API.
const MODAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Colab sells compute units at $9.99 per 100 (pay as you go).
const COLAB_USD_PER_UNIT: f64 = 0.0999;

/// Modal's published per-hour GPU prices (modal.com/pricing), used when the
/// workspace's own rates cannot be read. `(flavor, vram_gb, usd_per_hour)`.
const MODAL_LIST_PRICES: &[(&str, f64, f64)] = &[
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub providers: Vec<Provider>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// Backend id, as `--backend` takes it.
    pub id: &'static str,
    pub configured: bool,
    /// `credits` (a prepaid balance that can run out), `usage` (billed
    /// afterwards), `own` (the user's hardware, no per-hour charge), or
    /// `tokens` (billed per token rather than per GPU-hour).
    pub billing: &'static str,
    pub balance: Option<Balance>,
    /// This month's spend, where the provider reports it.
    pub spend: Option<Spend>,
    /// Cheapest first.
    pub offers: Vec<Offer>,
    pub note: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub amount: f64,
    /// `CU` (Colab compute units) or `USD`.
    pub unit: &'static str,
    /// Approximate value in US dollars.
    pub usd: Option<f64>,
    /// Units per hour the account's active runtimes are spending now.
    pub burning_per_hour: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spend {
    pub metered_usd: Option<f64>,
    pub billed_usd: Option<f64>,
    /// Plan credits and discounts applied this month.
    pub credits_usd: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    /// The `--flavor` value that requests it.
    pub flavor: String,
    /// GPU model, or `None` for a CPU-only offer.
    pub gpu: Option<String>,
    pub gpu_count: u32,
    pub vram_gb: Option<f64>,
    pub usd_per_hour: Option<f64>,
    /// Provider units per hour (Colab compute units).
    pub units_per_hour: Option<f64>,
    /// Hours the current balance pays for at this rate.
    pub runway_hours: Option<f64>,
    /// Whether the account can get it now, when the provider says.
    pub available: Option<bool>,
    /// A published or third-party price rather than this account's own rate.
    pub estimated: bool,
    /// The marketplace host behind the offer (OpenResearch only).
    pub host: Option<String>,
}

fn cache() -> &'static Mutex<Option<(Instant, Overview)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Overview)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Every backend's prices and balance. `fresh` skips the caches.
pub async fn overview(fresh: bool) -> Overview {
    let mut cached = cache().lock().await;
    if !fresh {
        if let Some((at, overview)) = cached.as_ref() {
            if at.elapsed() < OVERVIEW_TTL {
                return overview.clone();
            }
        }
    }
    let (colab, hf, modal, openresearch) =
        tokio::join!(colab(fresh), hf(), modal(), openresearch());
    let mut providers = vec![colab, hf, modal, openresearch];
    providers.extend(own_hardware());
    providers.push(Provider {
        note: Some("Billed per token of training and sampling, not per GPU-hour.".into()),
        ..empty(
            "tinker",
            "tokens",
            crate::jobs::tinker::resolve_api_key().is_ok(),
        )
    });
    let overview = Overview { providers };
    *cached = Some((Instant::now(), overview.clone()));
    overview
}

fn empty(id: &'static str, billing: &'static str, configured: bool) -> Provider {
    Provider {
        id,
        configured,
        billing,
        balance: None,
        spend: None,
        offers: Vec::new(),
        note: None,
        error: None,
    }
}

fn sort_offers(offers: &mut [Offer]) {
    offers.sort_by(|a, b| {
        let price = |o: &Offer| o.usd_per_hour.unwrap_or(f64::INFINITY);
        price(a)
            .total_cmp(&price(b))
            .then_with(|| a.flavor.cmp(&b.flavor))
    });
}

// --- Colab ----------------------------------------------------------------

async fn colab(fresh: bool) -> Provider {
    let configured = crate::jobs::colab::find_cli().is_some()
        && crate::jobs::colab::token_path().is_some_and(|path| path.is_file());
    let mut provider = empty("colab", "credits", configured);
    provider.note = Some(
        "Compute units are prepaid; a run stops when they run out. Dollar values assume \
         pay-as-you-go units ($9.99 per 100)."
            .into(),
    );
    let Some(account) = crate::jobs::colab_account::account(fresh).await else {
        provider.error = Some("Sign in to the Colab CLI to read the balance.".into());
        return provider;
    };
    provider.error = account.error.clone();
    provider.balance = account.balance.map(|amount| Balance {
        amount,
        unit: "CU",
        usd: Some(amount * COLAB_USD_PER_UNIT),
        burning_per_hour: account.rate_hourly,
    });
    provider.offers = colab_offers(&account);
    provider
}

fn colab_offers(account: &crate::jobs::colab_account::Account) -> Vec<Offer> {
    let mut offers: Vec<Offer> = account
        .rates
        .iter()
        .filter_map(|rate| {
            let accelerator = crate::jobs::colab::ACCELERATORS
                .iter()
                .find(|a| a.id == rate.id)?;
            let gpu = (accelerator.kind == "gpu").then(|| accelerator.cli_value.to_string());
            Some(Offer {
                flavor: rate.id.clone(),
                gpu_count: u32::from(gpu.is_some()),
                gpu,
                vram_gb: (accelerator.memory_gb > 0).then_some(accelerator.memory_gb as f64),
                usd_per_hour: Some(rate.cu_per_hour * COLAB_USD_PER_UNIT),
                units_per_hour: Some(rate.cu_per_hour),
                runway_hours: account
                    .balance
                    .filter(|_| rate.cu_per_hour > 0.0)
                    .map(|balance| balance / rate.cu_per_hour),
                available: account
                    .eligible
                    .as_ref()
                    .map(|eligible| eligible.iter().any(|id| id == &rate.id)),
                estimated: !rate.measured,
                host: None,
            })
        })
        .collect();
    sort_offers(&mut offers);
    offers
}

// --- Hugging Face ---------------------------------------------------------

async fn hf() -> Provider {
    let mut provider = empty(
        "hf",
        "usage",
        crate::jobs::huggingface::resolve_token().is_ok(),
    );
    provider.note = Some(
        "Billed per minute to the account's payment method; Hugging Face does not report a \
         balance."
            .into(),
    );
    match fetch_hf_hardware().await {
        Ok(body) => provider.offers = parse_hf_hardware(&body),
        Err(error) => provider.error = Some(error),
    }
    provider
}

async fn fetch_hf_hardware() -> Result<Value, String> {
    let client = crate::net::remote_client()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let url = format!("{}/api/jobs/hardware", crate::jobs::huggingface::endpoint());
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Could not reach Hugging Face: {}", e.without_url()))?;
    if !response.status().is_success() {
        return Err(format!(
            "Hugging Face did not list its hardware (HTTP {}).",
            response.status()
        ));
    }
    response
        .json()
        .await
        .map_err(|e| e.without_url().to_string())
}

/// Leading number of a value like `"16 GB"`, `"1"`, or `16`.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => {
            let digits: String = s
                .trim()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            digits.parse().ok()
        }
        _ => None,
    }
}

fn parse_hf_hardware(body: &Value) -> Vec<Offer> {
    let mut offers: Vec<Offer> = body
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|flavor| {
            let name = flavor["name"].as_str()?.to_string();
            let per_unit = number(&flavor["unitCostUSD"])
                .or_else(|| number(&flavor["unitCostMicroUSD"]).map(|micro| micro / 1e6))?;
            let per_hour = match flavor["unitLabel"].as_str().unwrap_or("minute") {
                "second" => per_unit * 3600.0,
                "hour" => per_unit,
                _ => per_unit * 60.0,
            };
            let accelerator = &flavor["accelerator"];
            let gpu = accelerator["model"].as_str().map(str::to_string);
            let count = number(&accelerator["quantity"]).unwrap_or(1.0) as u32;
            Some(Offer {
                flavor: name,
                gpu_count: if gpu.is_some() { count.max(1) } else { 0 },
                vram_gb: gpu.as_ref().and_then(|_| number(&accelerator["vram"])),
                gpu,
                usd_per_hour: Some(per_hour),
                units_per_hour: None,
                runway_hours: None,
                available: None,
                estimated: false,
                host: None,
            })
        })
        .collect();
    sort_offers(&mut offers);
    offers
}

// --- Modal ----------------------------------------------------------------

async fn modal() -> Provider {
    let configured = crate::jobs::modal::token_source().is_some();
    let mut provider = empty("modal", "usage", configured);
    provider.note =
        Some("Billed per second; plan credits cover usage first, then the payment method.".into());
    let mut offers = modal_list_offers();
    if configured {
        match tokio::time::timeout(MODAL_TIMEOUT, crate::jobs::modal::billing()).await {
            Ok(Ok(billing)) => {
                apply_modal_rates(&mut offers, &billing["rates"]);
                provider.spend = modal_spend(&billing);
                provider.error = billing["error"]
                    .as_str()
                    .or(billing["ratesError"].as_str())
                    .map(str::to_string);
            }
            Ok(Err(error)) => provider.error = Some(error.to_string()),
            Err(_) => provider.error = Some("Modal did not answer in time.".into()),
        }
    }
    sort_offers(&mut offers);
    provider.offers = offers;
    provider
}

fn modal_list_offers() -> Vec<Offer> {
    MODAL_LIST_PRICES
        .iter()
        .map(|(flavor, vram, price)| Offer {
            flavor: (*flavor).to_string(),
            gpu: Some(flavor.split('-').next().unwrap_or(flavor).to_string()),
            gpu_count: 1,
            vram_gb: Some(*vram),
            usd_per_hour: Some(*price),
            units_per_hour: None,
            runway_hours: None,
            available: None,
            estimated: true,
            host: None,
        })
        .collect()
}

/// The Modal flavor a billing-rate key prices, e.g. `gpu.h100.per_second`.
fn modal_flavor_for_key(key: &str) -> Option<&'static str> {
    let key = key.to_ascii_lowercase().replace(['_', '.', ' ', ':'], "-");
    let has = |token: &str| key.contains(token);
    Some(if has("b200") {
        "B200"
    } else if has("h200") {
        "H200"
    } else if has("h100") {
        "H100"
    } else if has("a100") && has("80") {
        "A100-80GB"
    } else if has("a100") {
        "A100-40GB"
    } else if has("l40s") {
        "L40S"
    } else if has("a10") {
        "A10G"
    } else if has("l4") {
        "L4"
    } else if has("t4") {
        "T4"
    } else {
        return None;
    })
}

/// Modal quotes GPUs per second; a key may also say its unit.
fn modal_rate_per_hour(key: &str, value: f64) -> f64 {
    let key = key.to_ascii_lowercase();
    if key.contains("hour") || key.contains("hr") {
        value
    } else if key.contains("sec") || value < 0.05 {
        value * 3600.0
    } else {
        value
    }
}

fn apply_modal_rates(offers: &mut [Offer], rates: &Value) {
    let Some(rates) = rates.as_object() else {
        return;
    };
    for (key, value) in rates {
        let (Some(flavor), Some(value)) = (modal_flavor_for_key(key), number(value)) else {
            continue;
        };
        if value <= 0.0 {
            continue;
        }
        if let Some(offer) = offers.iter_mut().find(|o| o.flavor == flavor) {
            offer.usd_per_hour = Some(modal_rate_per_hour(key, value));
            offer.estimated = false;
        }
    }
}

fn modal_spend(billing: &Value) -> Option<Spend> {
    let metered = number(&billing["meteredCost"]);
    let billed = number(&billing["billedCost"]);
    if metered.is_none() && billed.is_none() {
        return None;
    }
    let credits = billing["adjustments"].as_object().map(|adjustments| {
        adjustments
            .values()
            .filter_map(|value| value.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
            .filter(|value| *value < 0.0)
            .map(f64::abs)
            .sum()
    });
    Some(Spend {
        metered_usd: metered,
        billed_usd: billed,
        credits_usd: credits,
    })
}

// --- OpenResearch ---------------------------------------------------------

async fn openresearch() -> Provider {
    let mut provider = empty(
        "openresearch",
        "usage",
        crate::config::credentials_present(),
    );
    provider.note = Some("Marketplace GPUs billed per hour to the OpenResearch account.".into());
    let creds = match crate::config::load_credentials().await {
        Ok(Some(creds)) => creds,
        Ok(None) => {
            provider.error = Some("Sign in to OpenResearch to list its GPU prices.".into());
            return provider;
        }
        Err(error) => {
            provider.error = Some(error.to_string());
            return provider;
        }
    };
    match tokio::time::timeout(REQUEST_TIMEOUT, crate::client::list_catalog(&creds)).await {
        Ok(Ok(catalog)) => provider.offers = cheapest_catalog_offers(catalog.offers),
        Ok(Err(error)) => provider.error = Some(error.to_string()),
        Err(_) => provider.error = Some("OpenResearch did not answer in time.".into()),
    }
    provider
}

/// The cheapest offer for each GPU model and count.
fn cheapest_catalog_offers(catalog: Vec<crate::client::GpuOffer>) -> Vec<Offer> {
    let mut best: BTreeMap<(String, i64), crate::client::GpuOffer> = BTreeMap::new();
    for offer in catalog {
        let key = (offer.gpu.to_ascii_uppercase(), offer.gpu_count);
        if best
            .get(&key)
            .is_none_or(|kept| offer.price_per_hour < kept.price_per_hour)
        {
            best.insert(key, offer);
        }
    }
    let mut offers: Vec<Offer> = best
        .into_values()
        .map(|offer| {
            let id = offer.gpu.to_ascii_lowercase();
            Offer {
                flavor: if offer.gpu_count > 1 {
                    format!("{id}:{}", offer.gpu_count)
                } else {
                    id
                },
                gpu: Some(offer.gpu),
                gpu_count: offer.gpu_count.max(0) as u32,
                vram_gb: None,
                usd_per_hour: Some(offer.price_per_hour),
                units_per_hour: None,
                runway_hours: None,
                available: Some(true),
                estimated: false,
                host: Some(offer.provider),
            }
        })
        .collect();
    sort_offers(&mut offers);
    offers
}

// --- The user's own hardware ------------------------------------------------

fn own_hardware() -> Vec<Provider> {
    let ssh_hosts = ssh_config_has_host();
    let note = |text: &str| Some(text.to_string());
    vec![
        Provider {
            note: note("This machine; no per-hour charge. Check free RAM before heavy runs."),
            ..empty("local", "own", true)
        },
        Provider {
            note: note("The user's servers; no per-hour charge from OpenResearch."),
            ..empty("ssh", "own", ssh_hosts)
        },
        Provider {
            note: note("Cluster allocation; limits come from the account's quota."),
            ..empty(
                "slurm",
                "own",
                crate::jobs::slurm::load_settings()
                    .ok()
                    .flatten()
                    .is_some_and(|s| s.host.is_some()),
            )
        },
        Provider {
            note: note("Cluster quota; no per-hour charge from OpenResearch."),
            ..empty(
                "k8s",
                "own",
                crate::jobs::kubernetes::load_settings()
                    .ok()
                    .flatten()
                    .is_some(),
            )
        },
        Provider {
            note: note("The user's Ray cluster; no per-hour charge from OpenResearch."),
            ..empty(
                "ray",
                "own",
                !matches!(
                    crate::jobs::ray::resolve_address_with_source().1,
                    crate::jobs::ray::AddressSource::Default
                ),
            )
        },
    ]
}

/// Whether `~/.ssh/config` names at least one concrete host.
fn ssh_config_has_host() -> bool {
    dirs::home_dir()
        .and_then(|home| std::fs::read_to_string(home.join(".ssh/config")).ok())
        .is_some_and(|raw| {
            raw.lines().any(|line| {
                let line = line.split('#').next().unwrap_or("").trim();
                line.split_once([' ', '\t', '='])
                    .is_some_and(|(key, value)| {
                        key.eq_ignore_ascii_case("host")
                            && value
                                .split_whitespace()
                                .any(|name| !name.contains(['*', '?', '!']))
                    })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hf_hardware_becomes_hourly_offers_cheapest_first() {
        let body = json!([
            {"name": "a100-large", "accelerator": {"type": "gpu", "model": "A100", "quantity": "1", "vram": "80 GB"},
             "unitCostUSD": 0.041667, "unitLabel": "minute"},
            {"name": "cpu-basic", "accelerator": null, "unitCostMicroUSD": 167, "unitLabel": "minute"},
            {"name": "t4-small", "accelerator": {"type": "gpu", "model": "T4", "quantity": "1", "vram": "16 GB"},
             "unitCostUSD": 0.006667, "unitLabel": "minute"},
            {"name": "l4x4", "accelerator": {"type": "gpu", "model": "L4", "quantity": "4", "vram": "96 GB"},
             "unitCostUSD": 0.063333, "unitLabel": "minute"}
        ]);
        let offers = parse_hf_hardware(&body);
        let flavors: Vec<_> = offers.iter().map(|o| o.flavor.as_str()).collect();
        assert_eq!(flavors, ["cpu-basic", "t4-small", "a100-large", "l4x4"]);
        let t4 = &offers[1];
        assert!((t4.usd_per_hour.unwrap() - 0.40).abs() < 0.001);
        assert_eq!(
            (t4.gpu.as_deref(), t4.gpu_count, t4.vram_gb),
            (Some("T4"), 1, Some(16.0))
        );
        assert_eq!(offers[0].gpu_count, 0);
        assert_eq!(offers[3].gpu_count, 4);
    }

    #[test]
    fn modal_rates_override_list_prices() {
        let mut offers = modal_list_offers();
        apply_modal_rates(
            &mut offers,
            &json!({"gpu.h100.per_second": "0.001097", "gpu_a100_80gb": "0.000694",
                    "cpu.per_core_second": "0.0000131", "gpu.t4.hourly": "0.59"}),
        );
        let find = |flavor: &str| offers.iter().find(|o| o.flavor == flavor).unwrap();
        assert!((find("H100").usd_per_hour.unwrap() - 3.949).abs() < 0.01);
        assert!(!find("H100").estimated);
        assert!((find("A100-80GB").usd_per_hour.unwrap() - 2.498).abs() < 0.01);
        assert!((find("T4").usd_per_hour.unwrap() - 0.59).abs() < 1e-9);
        assert!(!find("T4").estimated);
        assert!(find("L4").estimated);
        assert_eq!(
            modal_flavor_for_key("gpu.a100.per_second"),
            Some("A100-40GB")
        );
        assert_eq!(modal_flavor_for_key("gpu.l40s"), Some("L40S"));
        assert_eq!(modal_flavor_for_key("cpu.per_core_second"), None);
    }

    #[test]
    fn modal_spend_counts_negative_adjustments_as_credits() {
        let spend = modal_spend(&json!({
            "meteredCost": "12.50", "billedCost": "0",
            "adjustments": {"plan_credits": "-12.50", "other": "0"}
        }))
        .unwrap();
        assert_eq!(spend.metered_usd, Some(12.5));
        assert_eq!(spend.billed_usd, Some(0.0));
        assert_eq!(spend.credits_usd, Some(12.5));
    }

    #[test]
    fn colab_offers_carry_runway_and_availability() {
        use crate::jobs::colab_account::{Account, Rate};
        let account = Account {
            balance: Some(100.0),
            eligible: Some(vec!["t4".into()]),
            rates: vec![
                Rate {
                    id: "a100".into(),
                    cu_per_hour: 5.0,
                    measured: false,
                },
                Rate {
                    id: "t4".into(),
                    cu_per_hour: 2.0,
                    measured: true,
                },
            ],
            ..Account::default()
        };
        let offers = colab_offers(&account);
        assert_eq!(offers[0].flavor, "t4");
        assert_eq!(offers[0].runway_hours, Some(50.0));
        assert_eq!(offers[0].available, Some(true));
        assert!(!offers[0].estimated);
        assert_eq!(offers[1].runway_hours, Some(20.0));
        assert_eq!(offers[1].available, Some(false));
        assert!(offers[1].estimated);
    }
}
