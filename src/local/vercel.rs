//! Vercel Sandbox launch intents. Credentials are added only at launch.

use std::collections::HashMap;

use crate::compute::SourceSnapshot;
use crate::error::Result;
use crate::jobs::{huggingface as hf, vercel};
use crate::store::LaunchSpec;

pub async fn launch_spec(
    args: &crate::ExpRunArgs,
    source: &SourceSnapshot,
    run_id: &str,
    command: &str,
) -> Result<LaunchSpec> {
    let vcpus = vercel::resolve_flavor(args.flavor.as_deref())?;
    let limits = vercel::launch_limits().await;
    let timeout_seconds = match &args.timeout {
        Some(t) => hf::parse_timeout(t)?,
        None => limits.default_timeout(),
    };
    vercel::check_timeout(timeout_seconds, &limits)?;
    vercel::check_vcpus(vcpus, &limits)?;

    let image = args
        .image
        .clone()
        .unwrap_or_else(|| vercel::DEFAULT_IMAGE.to_string());
    let spec = vercel::VercelJobSpec {
        name: vercel::sandbox_name(run_id),
        script: crate::compute::gated_script("/tmp/orx-source.tar", command),
        image,
        vcpus,
        env: HashMap::new(),
        timeout_seconds,
        session_cap: limits.session_cap(),
        region: std::env::var("ORX_VERCEL_REGION")
            .ok()
            .filter(|region| !region.trim().is_empty()),
        tags: super::run_labels(&args.exp_id, run_id)?,
        source_archive: Some(source.path.clone()),
    };
    Ok(LaunchSpec {
        kind: "vercel_job".into(),
        target: None,
        options: serde_json::to_value(spec)?,
    })
}
