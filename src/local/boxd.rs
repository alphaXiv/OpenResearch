//! boxd launch intents. Credentials are added only at launch.

use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{boxd, huggingface as hf};
use crate::store::LaunchSpec;

pub async fn launch_spec(
    args: &crate::ExpRunArgs,
    source: &SourceSnapshot,
    run_id: &str,
    command: &str,
) -> Result<LaunchSpec> {
    if args.image.is_some() {
        return Err(anyhow!(
            "--image does not apply to --backend boxd: each run gets a fresh Ubuntu 24.04 machine \
             with Python, uv, Node and Docker. Install what the run needs in its command."
        ));
    }
    let vcpus = boxd::resolve_flavor(args.flavor.as_deref())?;
    let timeout = args.timeout.as_deref().map(hf::parse_timeout).transpose()?;
    let defaults = boxd::Account::from_env()?
        .client("")?
        .org_defaults()
        .await?;
    boxd::check_size(vcpus, &defaults)?;
    let region = boxd::configured_region()?.or(Some(defaults.region).filter(|r| !r.is_empty()));
    let spec = boxd::BoxdJobSpec {
        name: boxd::machine_name(run_id),
        script: crate::compute::gated_script(boxd::SOURCE_ARCHIVE, command),
        vcpus,
        env: Default::default(),
        timeout_seconds: timeout.unwrap_or(boxd::DEFAULT_TIMEOUT_SECS),
        region,
        labels: super::run_labels(&args.exp_id, run_id)?,
        source_archive: Some(source.path.clone()),
    };
    Ok(LaunchSpec {
        kind: "boxd_job".into(),
        target: None,
        options: serde_json::to_value(spec)?,
    })
}
