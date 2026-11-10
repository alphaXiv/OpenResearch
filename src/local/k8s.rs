//! Kubernetes launch intents. Cluster/namespace come from the settings file (`orx up`
//! Settings → Compute, or `~/.config/openresearch/k8s.json`).
//!
//! There are no flavors: the run's shape is a **manifest committed on the
//! experiment branch** (default `.orx/k8s.yaml`, or `--manifest <path>`),
//! read at the recorded revision — the same commit the job receives — so
//! uncommitted manifest edits never run. See `jobs/kubernetes.rs` for the contract orx
//! enforces on it.

use std::collections::HashMap;

use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{huggingface as hf, kubernetes as k8s};
use crate::local::git;
use crate::store::{LaunchSpec, Store};

/// Manifest path on the experiment branch when `--manifest` is omitted.
const DEFAULT_MANIFEST: &str = ".orx/k8s.yaml";

pub async fn launch_spec(
    args: &crate::ExpRunArgs,
    source: &SourceSnapshot,
    run_id: &str,
    command: &str,
) -> Result<LaunchSpec> {
    if args.flavor.is_some() {
        return Err(anyhow!(
            "--backend k8s has no flavors — the manifest on the experiment branch \
             (default {DEFAULT_MANIFEST}, or --manifest <path>) declares the resources."
        ));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend k8s — set the image in the manifest."
        ));
    }
    let settings = k8s::load_settings()?.unwrap_or_default();
    let manifest_path = args
        .manifest
        .clone()
        .unwrap_or_else(|| DEFAULT_MANIFEST.to_string());
    // Same default as the HF path — no-timeout jobs are a footgun. The
    // manifest's own activeDeadlineSeconds wins when set.
    let timeout_seconds = match &args.timeout {
        Some(t) => hf::parse_timeout(t)?,
        None => 4 * 3600,
    };
    let store = Store::open()?;
    let experiment = store
        .get_local_experiment(&args.exp_id)?
        .ok_or_else(|| anyhow!("Local experiment {} not found.", args.exp_id))?;
    let project = store
        .get_local_project(&experiment.project_id)?
        .ok_or_else(|| anyhow!("Local project {} not found.", experiment.project_id))?;
    let manifest = {
        let repo_path = project.repo_path.clone();
        let revision = source.revision.clone();
        let path = manifest_path.clone();
        tokio::task::spawn_blocking(move || -> Result<String> {
            git::file_at(std::path::Path::new(&repo_path), &revision, &path).map_err(|_| {
                    anyhow!(
                        "No manifest at '{path}' in committed revision {revision} — write one \
                         and commit it before running. Pass --manifest <path> if it lives elsewhere."
                    )
                })
        })
        .await
        .map_err(|e| anyhow!("git task failed: {e}"))??
    };
    // DNS-safe, run-unique token for {{ORX_RUN}} in resource names.
    let run_token = run_id
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(10)
        .collect::<String>();
    let spec = k8s::ManifestSpec {
        manifest,
        script: crate::compute::gated_script("/tmp/orx-source/source.tar", command),
        run_token,
        env: HashMap::new(),
        timeout_seconds,
        labels: super::run_labels(&args.exp_id, run_id)?,
        pod_env: Vec::new(),
        source_archive: source.path.clone(),
    };
    let mut options = serde_json::to_value(spec)?;
    options["context"] = serde_json::json!(settings.context);
    Ok(LaunchSpec {
        kind: "k8s_job".into(),
        target: Some(settings.namespace),
        options,
    })
}
