//! Hugging Face Jobs launch intents. The engine adds the job's tokens as secrets at launch.

use std::collections::HashMap;

use crate::commands::exp::default_hf_image;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::huggingface as hf;
use crate::store::LaunchSpec;

/// Requires `--flavor`; the namespace is the token's account.
pub async fn launch_spec(
    args: &crate::ExpRunArgs,
    _source: &SourceSnapshot,
    run_id: &str,
    command: &str,
) -> Result<LaunchSpec> {
    let flavor = args.flavor.clone().ok_or_else(|| {
        anyhow!(
            "--backend hf requires --flavor: t4-small, a10g-small/large, l4x1, \
             l40sx1, a100-large, h200, … (cpu-basic/cpu-upgrade for CPU). \
             Priced per minute on your Hugging Face account."
        )
    })?;
    // HF's own 30m default is a footgun.
    let timeout_seconds = match &args.timeout {
        Some(t) => hf::parse_timeout(t)?,
        None => 4 * 3600,
    };
    let namespace = hf::whoami(&hf::resolve_token()?).await?;
    let image = args
        .image
        .clone()
        .unwrap_or_else(|| default_hf_image(&flavor));
    let labels = super::run_labels(&args.exp_id, run_id)?;
    let submission = hf::JobSubmission {
        command: vec![
            "bash".to_string(),
            "-c".to_string(),
            crate::compute::snapshot_script("/orx-source/source.tar", command),
        ],
        docker_image: image,
        flavor,
        environment: HashMap::new(),
        secrets: HashMap::new(),
        timeout_seconds,
        labels,
    };
    Ok(LaunchSpec {
        kind: "hf_job".into(),
        target: Some(namespace),
        options: serde_json::to_value(submission)?,
    })
}
