//! `orx telemetry status | on | off` — inspect and control anonymous usage
//! analytics.

use crate::error::{anyhow, Result};
use crate::telemetry;

pub async fn run(args: crate::TelemetryArgs) -> Result<()> {
    match args.command {
        crate::TelemetryCommand::Status => status(),
        crate::TelemetryCommand::On => set_enabled(true).await,
        crate::TelemetryCommand::Off => set_enabled(false).await,
    }
}

fn status() -> Result<()> {
    match telemetry::effective_disabled_reason() {
        None => {
            println!("Anonymous usage analytics: on");
            println!("  No code, prompts, file contents, or identifiers are ever sent.");
        }
        Some(reason) => {
            println!("Anonymous usage analytics: off ({})", reason.as_str());
        }
    }
    println!("  Build channel: {}", telemetry::build_channel());

    match telemetry::load_settings().and_then(|s| s.install_id) {
        Some(id) => println!("  Anonymous install id: {id}"),
        None => println!("  Anonymous install id: (not yet generated)"),
    }
    println!();
    println!("Set your preference with `orx telemetry off` or `orx telemetry on`.");
    Ok(())
}

async fn set_enabled(enabled: bool) -> Result<()> {
    // Eligible official builds record the decision even for an opt-out.
    telemetry::record_consent(enabled).await;
    telemetry::set_persisted_disabled(!enabled)
        .map_err(|e| anyhow!("Could not save telemetry setting: {e}"))?;
    if enabled {
        match telemetry::effective_disabled_reason() {
            None => {
                println!("\u{2713} Anonymous usage analytics enabled.");
                println!("  (The --no-telemetry flag still disables it for a single run.)");
            }
            Some(reason) => println!(
                "\u{2713} Analytics preference enabled, but analytics remain off ({}).",
                reason.as_str()
            ),
        }
    } else {
        println!("\u{2713} Anonymous usage analytics disabled on this machine.");
    }
    Ok(())
}
