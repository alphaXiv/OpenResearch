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
    if args.build_id {
        println!("{}", updates::current_build_id());
        return Ok(());
    }
    if args.dashboard_protocol {
        println!(
            "ORX_DASHBOARD_PROTOCOL={}",
            crate::commands::up_remote::DASHBOARD_PROTOCOL
        );
        return Ok(());
    }
    if args.build_channel {
        println!("{}", crate::telemetry::build_channel());
        return Ok(());
    }

    let current = updates::current_version();
    let current_display =
        if updates::current_channel().is_ok_and(|channel| channel.has_release_channels()) {
            updates::current_build_id().to_string()
        } else {
            current.to_string()
        };

    if !args.check && !args.json {
        println!("orx {}", current_display);
        return Ok(());
    }

    // Channel-aware: the macOS app installs from a different manifest than the
    // CLI, and reporting a version this install cannot move to would have a
    // harness chase an update that never lands.
    let latest = updates::fetch_latest_for_channel(Duration::from_secs(10)).await?;
    if let Some(latest) = &latest {
        // Keep the update-check cache in sync with this explicit check.
        updates::record_checked_release(latest);
    }
    let update_available = latest.as_ref().is_some_and(|latest| {
        latest.compatible && updates::release_available(&current, &latest.version, &latest.tag)
    });

    if args.json {
        let status = updates::status();
        println!(
            "{}",
            serde_json::json!({
                "current": current_display,
                "latest": latest.as_ref().map(|release| if release.tag.starts_with("canary-") { release.tag.clone() } else { release.version.to_string() }),
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

    println!("orx {}", current_display);
    match latest.as_ref() {
        Some(latest) if update_available => println!(
            "A new release is available: {} → {}. Run `{} update` to upgrade. Release notes: {}/releases/tag/{}",
            current_display,
            if latest.tag.starts_with("canary-") { latest.tag.clone() } else { latest.version.to_string() },
            crate::invocation::orx(),
            updates::REPO_URL,
            if latest.tag.starts_with("canary-") { "canary" } else { &latest.tag },
        ),
        Some(latest) if !latest.compatible => {
            println!("{}", updates::compatibility_wait_message(&latest.tag));
        }
        _ => println!("orx is up to date."),
    }
    Ok(())
}
