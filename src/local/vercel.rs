//! Vercel Sandbox launch intents. Credentials are added only at launch.

use std::collections::HashMap;

use crate::commands::exp::spawn_detached_supervise;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{huggingface as hf, vercel, BackendDescriptor};
use crate::store::{now_ms, LaunchSpec, Store, StoredRun};

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

pub async fn submit_local_vercel_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
) -> Result<StoredRun> {
    let vcpus = vercel::resolve_flavor(args.flavor.as_deref())?;
    vercel::preflight().await?;
    // One team lookup when a token and team id are available. OIDC-only and a
    // failed lookup keep the wide limits; sandbox create then reports a plan cap.
    let limits = vercel::launch_limits().await;
    let timeout_seconds = match &args.timeout {
        Some(t) => hf::parse_timeout(t)?,
        None => limits.default_timeout(),
    };
    vercel::check_timeout(timeout_seconds, &limits)?;
    vercel::check_vcpus(vcpus, &limits)?;

    let store = Store::open()?;
    let exp = store
        .get_local_experiment(&args.exp_id)?
        .ok_or_else(|| anyhow!("Local experiment {} not found.", args.exp_id))?;
    let project = store
        .get_local_project(&exp.project_id)?
        .ok_or_else(|| anyhow!("Local project {} not found.", exp.project_id))?;
    if let Some(w) = crate::local::experiments::legacy_root_warning(&project, &exp) {
        eprintln!("{w}");
    }
    let run_command = Some(exp.run_command.clone())
        .filter(|c| !c.trim().is_empty())
        .or_else(|| project.run_command.clone().filter(|c| !c.trim().is_empty()))
        .ok_or_else(|| anyhow!("{}", crate::invocation::no_run_command(&project.id)))?;

    let image = args
        .image
        .clone()
        .unwrap_or_else(|| vercel::DEFAULT_IMAGE.to_string());
    let script = crate::compute::gated_script("/tmp/orx-source.tar", &run_command);

    let mut env = crate::config::run_env(&run_id);
    if let Ok(hf_token) = hf::resolve_token() {
        env.entry("HF_TOKEN".to_string()).or_insert(hf_token);
    }
    vercel::strip_credentials(&mut env);
    let mut tags = HashMap::new();
    tags.insert("or_run".to_string(), run_id.clone());
    tags.insert("or_experiment".to_string(), exp.id.clone());
    tags.insert("or_project".to_string(), project.id.clone());
    let flavor = args.flavor.clone().unwrap_or_else(|| "cpu".to_string());

    let submitted = vercel::run_job(&vercel::VercelJobSpec {
        name: vercel::sandbox_name(&run_id),
        script,
        image: image.clone(),
        vcpus,
        env,
        timeout_seconds,
        session_cap: limits.session_cap(),
        region: std::env::var("ORX_VERCEL_REGION")
            .ok()
            .filter(|r| !r.trim().is_empty()),
        tags,
        source_archive: Some(source.path.clone()),
    })
    .await?;

    let mut descriptor = BackendDescriptor {
        ssh_container: None,
        monitoring_error: None,
        cancellation_accepted: false,
        rental_lease: None,
        kind: "vercel_job".to_string(),
        namespace: Some(submitted.sandbox_name.clone()),
        job_id: Some(submitted.process_id),
        flavor: Some(flavor),
        image: Some(image),
        url: None,
        context: None,
        manifest: None,
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: Some(timeout_seconds),
        cpus_per_task: None,
        mem: None,
        source_digest: None,
        source_path: None,
        source_size: None,
        remote_root: None,
        unknown: Default::default(),
    };
    source.apply_to_descriptor(&mut descriptor);
    if let Err(error) = crate::compute::record_submission_handle(&run_id, &descriptor) {
        let _ = vercel::finish_job(&submitted.sandbox_name).await;
        return Err(error);
    }
    let run = StoredRun {
        id: run_id.clone(),
        experiment_id: exp.id.clone(),
        project_id: project.id.clone(),
        status: "starting".to_string(),
        backend_json: descriptor.to_json(),
        command: run_command,
        created_at: now_ms(),
        updated_at: now_ms(),
        ended_at: None,
        exit_code: None,
        commit_sha: Some(source.revision),
        result_markdown: None,
        cancel_requested: store
            .get_run(&run_id)?
            .is_some_and(|run| run.cancel_requested),
        cancel_reason: None,
        chat_session_id: args.launching_chat_session(),
    };
    store.upsert_run(&run)?;

    spawn_detached_supervise(&run_id)?;
    Ok(run)
}
