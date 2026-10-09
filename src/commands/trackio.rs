//! The `trackio` command — show the configured Trackio connection and the
//! dashboard link for a run.
//!
//! The sibling of `orx wandb`, for the self-hosted case: where `wandb` asks the
//! api which W&B runs are linked, Trackio is a direct connection this machine
//! holds, so everything here is local and no credentials are needed.
//!
//! The write token is never printed — only whether one is set, and whether the
//! server accepted it.

use crate::error::Result;
use crate::jobs::{BackendDescriptor, TrackingDescriptor};
use crate::local::trackio;
use crate::store::Store;

pub async fn run(args: crate::TrackioArgs) -> Result<()> {
    if let Some(run_id) = args.run_id.as_deref() {
        return recorded(run_id);
    }
    let Some(config) = trackio::config() else {
        println!("Trackio is not configured.");
        eprintln!(
            "\nSet TRACKIO_SERVER_URL (and TRACKIO_PROJECT, TRACKIO_WRITE_TOKEN) in \
             `orx up` Settings → Environment, pointing at a Trackio server you already run."
        );
        return Ok(());
    };

    println!("Server   {}", config.server_url);
    println!("Project  {}", config.project.as_deref().unwrap_or("—"));
    println!(
        "Token    {}",
        if config.has_token { "set" } else { "not set" }
    );

    println!("{}", config.dashboard_url());

    let probe = trackio::probe(
        &config.server_url,
        crate::config::trackio_write_token().as_deref(),
    )
    .await;
    let verdict = trackio::verdict(Some(&config), Some(&probe), false);
    println!("Usable   {}", if verdict.usable { "yes" } else { "no" });
    match (&probe.error, probe.version.as_deref(), probe.write_access) {
        (Some(_), _, _) => eprintln!(
            "\n{}",
            verdict
                .reason
                .as_deref()
                .unwrap_or("Trackio is not usable.")
        ),
        (None, version, write_access) => {
            let version = version.unwrap_or("unknown version");
            match write_access {
                Some(true) => eprintln!("\nReachable (Trackio {version}); the token can write."),
                Some(false) => eprintln!(
                    "\nReachable (Trackio {version}), but the token cannot write — runs will \
                     fail to log. Check TRACKIO_WRITE_TOKEN against the server's."
                ),
                None => eprintln!("\nReachable (Trackio {version}); write access unknown."),
            }
        }
    }
    if probe.error.is_none() {
        if let Some(reason) = verdict.reason {
            eprintln!("\n{reason}");
        }
    }
    Ok(())
}

fn recorded(run_id: &str) -> Result<()> {
    let store = Store::open()?;
    let Some(run) = store.get_run(run_id)? else {
        println!("Run {run_id} was not found in the local store.");
        return Ok(());
    };
    let descriptor = BackendDescriptor::parse(&run.backend_json)?;
    let Some((server_url, project, recorded_run)) =
        descriptor.tracking.iter().find_map(|record| match record {
            TrackingDescriptor::Trackio {
                server_url,
                project,
                run,
            } => Some((server_url.as_str(), project.as_deref(), run.as_str())),
            TrackingDescriptor::Tensorboard { .. } => None,
        })
    else {
        println!("Run {run_id} has no recorded Trackio connection.");
        return Ok(());
    };
    println!("Server   {server_url}");
    println!("Project  {}", project.unwrap_or("—"));
    println!("Run      {recorded_run}");
    println!(
        "{}",
        crate::config::trackio_dashboard_url(server_url, project)
    );
    Ok(())
}
