//! Modal Sandbox launch intents. Auth is Modal's own (MODAL_TOKEN_ID/SECRET or
//! `~/.modal.toml`); the sandbox runs on the user's Modal account and scales to zero when the
//! run ends.

use std::collections::HashMap;

use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{huggingface as hf, modal};
use crate::store::LaunchSpec;

/// Modal app all orx sandboxes are grouped under (visible in the Modal dashboard).
const MODAL_APP: &str = "openresearch";

/// Requires `--flavor <name>`: a Modal GPU (t4, l4, a10g, a100, a100-80gb, l40s, h100, h200,
/// …) or `cpu` / `cpu-large` for CPU-only.
pub fn launch_spec(
    args: &crate::ExpRunArgs,
    source: &SourceSnapshot,
    run_id: &str,
    command: &str,
) -> Result<LaunchSpec> {
    let flavor = args.flavor.clone().ok_or_else(|| {
        anyhow!(
            "--backend modal requires --flavor: a Modal GPU (t4, l4, a10g, a100, \
             a100-80gb, l40s, h100, h200, or e.g. h100:2 for a count) — or cpu / cpu-large \
             for CPU-only. Priced per second on your Modal account."
        )
    })?;
    let resources = modal::resolve_flavor(&flavor);
    // Same default as the HF/k8s paths — no-timeout jobs are a footgun.
    let timeout_seconds = match &args.timeout {
        Some(t) => hf::parse_timeout(t)?,
        None => 4 * 3600,
    };
    let image = args
        .image
        .clone()
        .unwrap_or_else(|| modal::default_image(resources.gpu.is_some()));
    let spec = modal::ModalJobSpec {
        script: crate::compute::gated_script("/tmp/orx-source.tar", command),
        image,
        gpu: resources.gpu,
        cpu: resources.cpu,
        memory: resources.memory,
        env: HashMap::new(),
        timeout_seconds,
        app: MODAL_APP.to_string(),
        tags: super::run_labels(&args.exp_id, run_id)?,
        source_archive: Some(source.path.clone()),
    };
    Ok(LaunchSpec {
        kind: "modal_job".into(),
        target: None,
        options: serde_json::to_value(spec)?,
    })
}
