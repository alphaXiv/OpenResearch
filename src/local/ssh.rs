//! SSH launch intents: the run executes as a detached process on one of your own boxes.
//! `--host` names an `~/.ssh/config` host alias (there's no hardware scheduler on a plain
//! server).

use crate::commands::exp::spawn_detached_supervise;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{ssh, BackendDescriptor};
use crate::store::{now_ms, LaunchSpec, Store, StoredRun};

/// The saved container is resolved by the engine over its own connection to the host.
pub fn launch_spec(args: &crate::ExpRunArgs, command: &str) -> Result<LaunchSpec> {
    let settings = crate::config::ssh_settings()?;
    let (host, options) = crate::jobs::ssh::resolve_options(args, &settings)?;
    let launch = crate::engine::adapters::ssh::LaunchOptions {
        run_sh: Some(format!(
            "{} {}",
            crate::compute::TENSORBOARD_LOGDIR_EXPORT,
            crate::compute::staged_script(command)
        )),
        container_reference: options.container,
        root: Some(crate::engine::runtime::engine_root(
            options.remote_root.as_deref(),
        )),
        ..Default::default()
    };
    Ok(LaunchSpec {
        kind: "ssh_job".into(),
        target: Some(host),
        options: serde_json::to_value(launch)?,
    })
}

pub async fn submit_local_ssh_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
    launch: &ssh::ResolvedLaunch,
) -> Result<StoredRun> {
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

    let target = launch.target.clone().for_run(&run_id);
    let container = launch
        .container
        .as_ref()
        .map(|container| container.for_run(&run_id));
    let remote_dir = ssh::stage_source(
        &target,
        &run_id,
        &source.path,
        &source.digest,
        container.as_ref(),
        launch.remote_root.as_deref(),
    )
    .await?;
    let script = format!(
        "{} {}",
        crate::compute::TENSORBOARD_LOGDIR_EXPORT,
        crate::compute::staged_script(&run_command)
    );

    // The remote env: everything the user synced (API keys), plus the tokens
    // the run script expects. Exported inside run.sh (written owner-only).
    let mut env = crate::config::run_env(&run_id);
    if let Ok(hf_token) = crate::jobs::huggingface::resolve_token() {
        env.entry("HF_TOKEN".to_string()).or_insert(hf_token);
    }

    let mut descriptor = BackendDescriptor {
        ssh_container: container.clone(),
        monitoring_error: None,
        cancellation_accepted: false,
        rental_lease: None,
        kind: "ssh_job".to_string(),
        namespace: Some(target.dest.clone()),
        job_id: Some(remote_dir.clone()),
        flavor: None,
        image: None,
        url: None,
        context: None,
        manifest: None,
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: None,
        cpus_per_task: None,
        mem: None,
        source_digest: None,
        source_path: None,
        source_size: None,
        remote_root: launch.remote_root.clone(),
        unknown: Default::default(),
    };
    source.apply_to_descriptor(&mut descriptor);
    if container.is_some() {
        crate::compute::record_submission_handle(&run_id, &descriptor)?;
    }
    let submission = ssh::run_job(&ssh::SshJobSpec {
        target: target.clone(),
        run_id: run_id.clone(),
        remote_root: launch.remote_root.clone(),
        script,
        env,
        container: container.clone(),
    })
    .await;
    match submission {
        Err(error) if container.is_some() && error.is::<ssh::LaunchUncertain>() => {
            eprintln!("SSH launch response failed for run {run_id}: {error:#}. Reconciling the saved container handle.");
        }
        Err(error) => {
            store.update_status(
                &run_id,
                crate::store::RunStatus::Failed,
                Some(now_ms()),
                None,
            )?;
            store.set_result_markdown(&run_id, &format!("Compute submission failed: {error:#}"))?;
            return Err(error);
        }
        Ok(_) if container.is_none() => {
            if let Err(error) = crate::compute::record_submission_handle(&run_id, &descriptor) {
                let _ = ssh::cancel_job(&target, &remote_dir, None).await;
                return Err(error);
            }
        }
        Ok(_) => {}
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
