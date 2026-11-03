//! Ray Jobs launch intents, submitted through the Ray Jobs / Dashboard API.

use std::collections::HashMap;

use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::ray;
use crate::jobs::ssh::sh_quote;
use crate::store::LaunchSpec;

/// Uploads the working_dir package now; it is content-addressed, so a relaunch reuses it.
pub async fn launch_spec(
    args: &crate::ExpRunArgs,
    source: &SourceSnapshot,
    run_id: &str,
    command: &str,
) -> Result<LaunchSpec> {
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend ray — the job runs in the cluster's \
             runtime environment, not a per-job container."
        ));
    }
    if args.host.is_some() {
        return Err(anyhow!(
            "--host only applies with --backend ssh/slurm. Save the Ray Jobs URL in \
             OpenResearch or set ASTROAI_RAY_JOBS_ADDRESS / RAY_DASHBOARD_URL."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest only applies with --backend k8s."));
    }
    if args.timeout.is_some() {
        return Err(anyhow!(
            "--timeout isn't supported on --backend ray — Ray Jobs have no time limit; \
             the job runs until the command exits. Bound the run in the command itself."
        ));
    }
    let resources = ray::parse_flavor(args.flavor.as_deref())?;
    let address = ray::resolve_address(None);
    let (package_digest, package_path) = source
        .ray_package
        .as_ref()
        .ok_or_else(|| anyhow!("Ray source package was not created."))?;
    let working_dir = ray::stage_working_dir(&address, package_digest, package_path).await?;
    let submission = ray::JobSubmission {
        entrypoint: format!("bash -c {}", sh_quote(command)),
        submission_id: run_id.to_string(),
        resources,
        env: HashMap::new(),
        metadata: super::run_labels(&args.exp_id, run_id)?,
        working_dir: Some(working_dir),
    };
    Ok(LaunchSpec {
        kind: "ray_job".into(),
        target: Some(address),
        options: serde_json::to_value(submission)?,
    })
}
