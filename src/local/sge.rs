//! Local Sun Grid Engine launch — the SGE twin of `local/slurm.rs`: submit the
//! experiment as a batch job on a Grid Engine cluster reached via its login
//! node. `--host` names an `~/.ssh/config` alias (defaultable in the sge
//! settings); `--flavor` asks for GPUs. The run row lives in the local store
//! only; a detached `orx supervise` watches the job.
//!
//! Two things differ structurally from the Slurm launcher: the run dir is an
//! ABSOLUTE path on a project filesystem (SCC home dirs are quota'd), and the
//! login node requires a live ControlMaster because its sshd demands a second
//! factor no batch-mode ssh can answer.

use std::collections::HashMap;

use crate::commands::exp::spawn_detached_supervise;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{huggingface, sge, ssh, BackendDescriptor};
use crate::store::{now_ms, Store, StoredRun};

/// CLI wrapper around `submit_local_sge`: submit, then print the summary.
pub async fn launch_local_sge(args: &crate::ExpRunArgs) -> Result<()> {
    let run = submit_local_sge(args).await?;
    let backend = BackendDescriptor::parse(&run.backend_json)?;
    println!("\u{2713} Grid Engine job submitted.");
    println!(
        "  host {}  (job {})",
        backend.namespace.as_deref().unwrap_or(""),
        backend.job_id.as_deref().unwrap_or("")
    );
    if let Some(dir) = backend.run_dir.as_deref() {
        println!("  dir  {dir}");
    }
    println!("  run  {}", run.id);
    println!(
        "{}",
        crate::invocation::follow_up(&run.experiment_id, &run.id)
    );
    Ok(())
}

pub async fn submit_local_sge(args: &crate::ExpRunArgs) -> Result<StoredRun> {
    crate::compute::submit(args).await
}

pub async fn submit_local_sge_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
) -> Result<StoredRun> {
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend sge — the job runs in your cluster \
             environment (modules/conda), not a container."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest only applies with --backend k8s."));
    }

    let settings = sge::load_settings()?.unwrap_or_default();
    let host = args
        .host
        .clone()
        .or_else(|| settings.host.clone())
        .ok_or_else(|| {
            anyhow!(
                "--backend sge needs a login node: pass --host <alias> (an ~/.ssh/config \
                 alias) or configure a default Grid Engine host."
            )
        })?;

    // Resolve the resource request before touching the network — an unknown
    // gpu_type should fail instantly, not after a source upload.
    let resources = sge::resolve_resources(args.flavor.as_deref(), &settings)?;

    // `--timeout` beats the settings default, which itself defaults to 12h.
    let time_limit_secs = Some(huggingface::parse_timeout(
        args.timeout
            .as_deref()
            .unwrap_or(&settings.time_limit_or_default()),
    )?);

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

    // The job env: everything the user synced (API keys), plus the tokens the
    // run step expects. Exported in job.qsub.
    let mut env: HashMap<String, String> = crate::config::list_synced_env().into_iter().collect();
    if let Ok(hf_token) = huggingface::resolve_token() {
        env.entry("HF_TOKEN".to_string()).or_insert(hf_token);
    }

    // Per-user leaf under the configured base: /projectnb is a shared group
    // filesystem and our run dirs are 0700, so without this a labmate's staging
    // would trip over our unreadable cache entries.
    let work_dir = sge::resolve_user_work_dir(&host, &settings.resolved_work_dir()?)
        .await
        .map_err(|e| connect_hint(&host, e))?;
    sge::ensure_work_dir(&host, &work_dir)
        .await
        .map_err(|e| connect_hint(&host, e))?;

    // Redirect pip/conda/HF/compile caches off $HOME, which is capped at 10 GB
    // on SCC — one `pip install torch` plus a model download would spend most
    // of it. Defaults only: anything the author synced themselves still wins.
    let env = sge::default_cache_env(&env, &work_dir);

    let dir = ssh::stage_source_under(
        &sge::login(&host),
        Some(&work_dir),
        &run_id,
        &source.path,
        &source.digest,
    )
    .await
    .map_err(|e| connect_hint(&host, e))?;

    let slots = settings.slots_or_default();
    let job_id = sge::run_job(&sge::SgeJobSpec {
        host: host.clone(),
        run_id: run_id.clone(),
        dir: dir.clone(),
        command: run_command.clone(),
        env,
        resources,
        scc_project: Some(settings.scc_project_or_default()),
        pe: Some((settings.pe_or_default(), slots)),
        time_limit_secs,
    })
    .await
    .map_err(|e| connect_hint(&host, e))?;

    let mut descriptor = BackendDescriptor {
        kind: "sge_job".to_string(),
        namespace: Some(host.clone()),
        job_id: Some(job_id.clone()),
        flavor: args.flavor.clone(),
        image: None,
        url: None,
        context: None,
        manifest: None,
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: None,
        source_digest: None,
        source_path: None,
        source_size: None,
        run_dir: Some(dir),
    };
    source.apply_to_descriptor(&mut descriptor);
    if let Err(error) = crate::compute::record_submission_handle(&run_id, &descriptor) {
        let _ = sge::cancel_job(&host, &job_id).await;
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
        chat_session_id: args.launching_chat_session(),
    };
    store.upsert_run(&run)?;

    spawn_detached_supervise(&run_id)?;
    Ok(run)
}

/// Turn a bare transport failure into the one instruction that fixes it.
///
/// The launch path is where users actually hit a lapsed second factor, and
/// `Permission denied (publickey,keyboard-interactive)` tells them nothing
/// actionable — least of all that a plain `ssh <host>` will NOT help, because
/// orx multiplexes over its own private ControlPath.
pub(crate) fn connect_hint(host: &str, error: crate::error::Error) -> crate::error::Error {
    if error.downcast_ref::<ssh::MasterRequired>().is_some() {
        return error;
    }
    if ssh::is_second_factor_failure(&error.to_string()) {
        return anyhow!(
            "{host} refused the connection: it requires a second factor (Duo) that orx's \
             background connections cannot answer.\n\nRun `{orx} ssh connect {host}` once, \
             approve the prompt, then launch again. A plain `ssh {host}` will not do — orx \
             uses its own private ControlPath.",
            orx = crate::invocation::orx()
        );
    }
    error
}
