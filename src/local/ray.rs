//! Local Ray Jobs launch — submit via the Ray Jobs / Dashboard API, then
//! detach `orx supervise` to poll status and mirror logs.

use std::collections::HashMap;

use crate::commands::exp::{hf_clone_script, spawn_detached_supervise};
use crate::error::{anyhow, Result};
use crate::jobs::{ray, BackendDescriptor};
use crate::local::git;
use crate::store::{now_ms, Store, StoredRun};

/// CLI wrapper: submit, then print the summary.
pub async fn launch_local_ray(args: &crate::ExpRunArgs) -> Result<()> {
    let run = submit_local_ray(args).await?;
    let backend = BackendDescriptor::parse(&run.backend_json)?;
    println!("\u{2713} Ray job submitted.");
    println!("  run    {}", run.id);
    println!(
        "  job    {} ({})",
        backend.job_id.as_deref().unwrap_or(""),
        backend.flavor.as_deref().unwrap_or("default")
    );
    println!("  watch  {}", backend.url.as_deref().unwrap_or(""));
    println!(
        "  Follow it with `orx exp wait {}` or `orx logs {}`.",
        run.experiment_id, run.id
    );
    Ok(())
}

/// Submit the local experiment's run as a Ray Job and detach a supervisor.
pub async fn submit_local_ray(args: &crate::ExpRunArgs) -> Result<StoredRun> {
    if args.sandbox.is_some() || args.gpu.is_some() || args.cpu.is_some() {
        return Err(anyhow!(
            "Local experiments run on Ray Jobs; drop --gpu/--cpu/--sandbox \
             and pass --backend ray [--flavor cpu:2|gpu:1|…]."
        ));
    }

    let resources = ray::parse_flavor(args.flavor.as_deref())?;
    let address = ray::resolve_address(None);

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
        .ok_or_else(|| {
            anyhow!(
                "No run command set for this experiment or its project. Set the project \
                 default with `orx project edit {} --run-command '<cmd>'`, pass \
                 `--run-command '<cmd>'` to `orx create-experiment`, or set it in the \
                 dashboard — then relaunch.",
                project.id
            )
        })?;

    if !args.force {
        if let Some(r) = store
            .list_runs_by_experiment(&exp.id)?
            .into_iter()
            .find(|r| !crate::local::is_terminal(&r.status))
        {
            return Err(anyhow!(
                "Run {} is already in flight for this experiment ({}). \
                 Cancel it with `orx exp cancel {}` or pass --force to launch anyway.",
                r.id,
                r.status,
                exp.id
            ));
        }
    }

    // Reachability check before we touch git / allocate a run id.
    ray::preflight(&address).await.map_err(|e| {
        anyhow!(
            "{e}\n\
             Set the Jobs URL in Settings → Compute → Ray, or export \
             ASTROAI_RAY_JOBS_ADDRESS / RAY_DASHBOARD_URL."
        )
    })?;

    let commit_sha = {
        let (owner, repo, baseline, branch) = (
            project.github_owner.clone(),
            project.github_repo.clone(),
            project.baseline_branch.clone(),
            exp.branch_name.clone(),
        );
        tokio::task::spawn_blocking(move || -> Result<String> {
            let repo_path = git::ensure_clone(&owner, &repo, &baseline)?;
            if !git::branch_on_remote(&repo_path, &branch)? {
                git::push_branch(&repo_path, &branch)?;
            }
            git::branch_head_sha(&repo_path, &branch)
        })
        .await
        .map_err(|e| anyhow!("git task failed: {e}"))??
    };

    let run_id = uuid::Uuid::new_v4().to_string();
    // Ray submission ids: letters, digits, dashes, underscores.
    let submission_id = format!("orx-{}", run_id.replace('-', ""));
    let script = hf_clone_script(
        &exp.branch_name,
        &project.github_owner,
        &project.github_repo,
        &run_command,
    );
    // Prefer an explicit image only as documentation on the descriptor —
    // Ray Jobs run in the cluster's runtime env, not a per-job Docker image
    // unless the cluster is configured for it. We still record --image if set.
    let image = args.image.clone();

    let mut env = HashMap::new();
    if let Some(gh) = git::resolve_github_token() {
        env.insert("GITHUB_TOKEN".to_string(), gh);
    }
    let mut metadata = HashMap::new();
    metadata.insert("or_run".to_string(), run_id.clone());
    metadata.insert("or_experiment".to_string(), exp.id.clone());
    metadata.insert("or_project".to_string(), project.id.clone());

    let job = ray::run_job(
        &address,
        &ray::JobSubmission {
            entrypoint: format!("bash -c {}", shell_single_quote(&script)),
            submission_id: submission_id.clone(),
            resources,
            env,
            metadata,
        },
    )
    .await?;

    let job_id = job
        .submission_id
        .or(job.job_id)
        .unwrap_or(submission_id);
    let watch = ray::job_url(&address, &job_id);

    let descriptor = BackendDescriptor {
        kind: "ray_job".to_string(),
        namespace: Some(address.clone()),
        job_id: Some(job_id),
        flavor: args.flavor.clone(),
        image,
        url: Some(watch),
        context: None,
        manifest: None,
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: None,
    };
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
        commit_sha: Some(commit_sha),
        result_markdown: None,
        cancel_requested: false,
        chat_session_id: crate::local::chat::launching_chat_session(),
    };
    store.upsert_run(&run)?;

    spawn_detached_supervise(&run_id)?;
    Ok(run)
}

/// Wrap `s` in single quotes for `bash -c '…'`, escaping embedded `'`.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
