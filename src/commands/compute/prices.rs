//! `orx compute prices`, owned by the tuandang304 fork.

use super::*;

#[derive(Debug, Args)]
pub struct PricesArgs {
    /// Re-read balances and prices instead of using the five-minute cache.
    #[arg(long)]
    pub fresh: bool,
    /// Only offers with this GPU model (e.g. `a100`). Case-insensitive.
    #[arg(long)]
    pub gpu: Option<String>,
    /// Include providers that are not connected.
    #[arg(long)]
    pub all: bool,
}

/// Remaining credit and GPU prices across providers, so a run can go where it
/// is cheapest and will not run out of credit midway.
pub(super) async fn prices(args: PricesArgs, json: bool) -> Result<()> {
    use crate::jobs::provider_prices::overview;
    let mut overview = overview(args.fresh).await;
    overview
        .providers
        .retain(|provider| args.all || provider.configured);
    if let Some(gpu) = &args.gpu {
        let gpu = gpu.to_ascii_lowercase();
        for provider in &mut overview.providers {
            provider.offers.retain(|offer| {
                offer
                    .gpu
                    .as_ref()
                    .is_some_and(|model| model.to_ascii_lowercase().starts_with(&gpu))
            });
        }
    }
    if json {
        println!("{}", serde_json::to_string(&overview)?);
        return Ok(());
    }
    let money = |value: f64| format!("${value:.2}");
    let mut balances = Vec::new();
    let mut rows = Vec::new();
    for provider in &overview.providers {
        let credit = match (&provider.balance, &provider.spend) {
            (Some(balance), _) => {
                let mut text = format!("{:.1} {}", balance.amount, balance.unit);
                if balance.unit == "USD" {
                    text = format!("≈{} available", money(balance.amount));
                } else if let Some(usd) = balance.usd {
                    text.push_str(&format!(" (≈{})", money(usd)));
                }
                if let Some(burn) = balance.burning_per_hour.filter(|burn| *burn > 0.0) {
                    text.push_str(&format!(", spending {burn:.2}/h now"));
                }
                text
            }
            (None, Some(spend)) => format!(
                "{} used this month, {} billed{}",
                spend.metered_usd.map_or("?".into(), money),
                spend.billed_usd.map_or("?".into(), money),
                spend.credits_usd.filter(|credits| *credits > 0.0).map_or(
                    String::new(),
                    |credits| format!(", {} covered by credits", money(credits))
                ),
            ),
            (None, None) => match provider.billing {
                "own" => "own hardware".into(),
                "tokens" => "billed per token".into(),
                "credits" => "prepaid, balance unknown".into(),
                _ => "pay as you go".into(),
            },
        };
        let status = if provider.configured {
            ""
        } else {
            " (not connected)"
        };
        balances.push(vec![
            format!("{}{status}", provider.id),
            credit,
            provider.error.clone().unwrap_or_default(),
        ]);
        for offer in provider.offers.iter().filter(|offer| offer.gpu.is_some()) {
            let mut price = offer.usd_per_hour.map_or("—".into(), money);
            if let Some(units) = offer.units_per_hour {
                price = format!("{price} ({units:.2} CU)");
            }
            if offer.estimated {
                price = format!("≈{price}");
            }
            rows.push(vec![
                provider.id.to_string(),
                offer.flavor.clone(),
                format!(
                    "{}{}",
                    offer.gpu.clone().unwrap_or_default(),
                    if offer.gpu_count > 1 {
                        format!(" ×{}", offer.gpu_count)
                    } else {
                        String::new()
                    }
                ),
                offer
                    .vram_gb
                    .map_or("—".into(), |vram| format!("{vram:.0}")),
                price,
                offer
                    .runway_hours
                    .map_or("—".into(), |hours| format!("{hours:.1} h")),
                match offer.available {
                    Some(false) => "not on plan".into(),
                    _ => offer.host.clone().unwrap_or_default(),
                },
            ]);
        }
    }
    print_table(&["PROVIDER", "CREDIT", "NOTE"], &balances);
    println!();
    rows.sort_by(|a, b| price_key(&a[4]).total_cmp(&price_key(&b[4])));
    if rows.is_empty() {
        println!("No GPU prices to show.");
    } else {
        print_table(
            &[
                "PROVIDER", "FLAVOR", "GPU", "VRAM(GB)", "$/HR", "RUNWAY", "NOTE",
            ],
            &rows,
        );
        println!("\n≈ marks a published or estimated price rather than the account's own rate.");
    }
    Ok(())
}

fn price_key(cell: &str) -> f64 {
    cell.trim_start_matches(['≈', '$'])
        .split(' ')
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(f64::INFINITY)
}
