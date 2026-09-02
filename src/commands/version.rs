//! The `version` command — print the CLI version, optionally comparing it to
//! the latest GitHub release.
//!
//! `--json` (which implies `--check`) is the agent-facing form: one stable
//! JSON object on stdout, exit 0 whether or not an update is available, so
//! harnesses can poll deliberately instead of scraping the stderr warning.

use std::time::Duration;

use crate::error::Result;
use crate::updates;

pub async fn run(args: crate::VersionArgs) -> Result<()> {
    if args.build_channel {
        println!("{}", crate::telemetry::build_channel());
        return Ok(());
    }

    let current = updates::current_version();

    if !args.check && !args.json {
        println!("orx {}", current);
        return Ok(());
    }

    // Channel-aware: the macOS app installs from a different manifest than the
    // CLI, and reporting a version this install cannot move to would have a
    // harness chase an update that never lands.
    let latest = updates::fetch_latest_for_channel(Duration::from_secs(10)).await?;
    if let Some(latest) = &latest {
        // Keep the update-check cache in sync with this explicit check.
        updates::write_check_cache(&latest.to_string());
    }
    let update_available = latest
        .as_ref()
        .is_some_and(|latest| updates::is_outdated(&current, latest));

    if args.json {
        let status = updates::status();
        println!(
            "{}",
            serde_json::json!({
                "current": current.to_string(),
                "latest": latest.as_ref().map(|v| v.to_string()),
                "updateAvailable": update_available,
                // How this copy was installed, and whether it keeps itself
                // current — so a harness can tell "will fix itself" from
                // "needs a human to run brew upgrade".
                "channel": status.channel,
                "autoUpdate": status.self_updates && status.auto_update,
            })
        );
        return Ok(());
    }

    println!("orx {}", current);
    match latest.filter(|_| update_available) {
        Some(latest) => println!(
            "A new release is available: {} → {}. Run `{} update` to upgrade.",
            current,
            latest,
            crate::invocation::orx()
        ),
        None => println!("orx is up to date."),
    }
    Ok(())
}
