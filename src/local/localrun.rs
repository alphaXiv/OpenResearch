//! Launch intents for runs on this machine: `local`, and `tinker` (a local controller using
//! remote Tinker model compute). Each run extracts the recorded revision into its own run
//! dir, never the agent's worktree.

use crate::error::{anyhow, Result};
use crate::store::LaunchSpec;

pub fn launch_spec(args: &crate::ExpRunArgs, kind: &str) -> Result<LaunchSpec> {
    let backend = kind.trim_end_matches("_job");
    if args.flavor.is_some() {
        return Err(anyhow!("--backend {backend} does not take --flavor."));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend {backend} — the controller uses this machine's own environment."
        ));
    }
    Ok(LaunchSpec {
        kind: kind.into(),
        target: None,
        options: serde_json::json!({}),
    })
}
