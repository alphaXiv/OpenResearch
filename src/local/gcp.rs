//! Local Google Cloud launch — create a Compute Engine VM for this one run. As
//! with `local/openresearch.rs`, submit only creates the VM and records the run
//! as `starting`; the detached supervisor waits for it, launches over ssh,
//! watches, and deletes the VM at terminal state.

use crate::commands::exp::spawn_detached_supervise;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{gcp, BackendDescriptor};
use crate::store::{now_ms, Store, StoredRun};

/// The flavor a launch without `--flavor` gets: the cheapest GPU.
pub const DEFAULT_FLAVOR: &str = "t4";

/// CLI wrapper: submit, then print the summary.
pub async fn launch_local_gcp(args: &crate::ExpRunArgs) -> Result<()> {
    let run = crate::compute::submit(args).await?;
    let backend = BackendDescriptor::parse(&run.backend_json)?;
    println!("\u{2713} Google Cloud VM requested.");
    println!("  flavor  {}", backend.flavor.as_deref().unwrap_or(""));
    println!(
        "  vm      {} ({}, {})",
        backend.job_id.as_deref().unwrap_or(""),
        backend.namespace.as_deref().unwrap_or(""),
        backend.context.as_deref().unwrap_or("")
    );
    println!("  run     {}", run.id);
    println!("  The VM is starting; the supervisor launches the run once it is up and deletes the VM when the run ends.");
    println!(
        "{}",
        crate::invocation::follow_up(&run.experiment_id, &run.id)
    );
    Ok(())
}

pub async fn submit_local_gcp_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
) -> Result<StoredRun> {
    if args.host.is_some() {
        return Err(anyhow!(
            "--host doesn't apply to --backend gcp — the VM is created for you (use \
             --backend ssh to run on a machine you already have)."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest is k8s-only."));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend gcp. Set the VM image family in the Google \
             Cloud compute settings; install project dependencies in the run command."
        ));
    }
    let flavor = args
        .flavor
        .clone()
        .unwrap_or_else(|| DEFAULT_FLAVOR.to_string());
    let shape = gcp::parse_flavor(&flavor)?;
    let timeout_secs = match &args.timeout {
        Some(t) => crate::jobs::huggingface::parse_timeout(t)?,
        None => 4 * 3600,
    };
    let settings = gcp::load_settings()?.unwrap_or_default();
    let (project, zone) = gcp::resolve_location(&settings).await;
    let project = project.ok_or_else(|| {
        anyhow!(
            "No Google Cloud project. Run `orx compute configure gcp --project <id>` or \
             `gcloud config set project <id>`."
        )
    })?;
    let public_key = gcp::ensure_ssh_key().await?;

    let store = Store::open()?;
    let exp = store
        .get_local_experiment(&args.exp_id)?
        .ok_or_else(|| anyhow!("Local experiment {} not found.", args.exp_id))?;
    let project_row = store
        .get_local_project(&exp.project_id)?
        .ok_or_else(|| anyhow!("Local project {} not found.", exp.project_id))?;
    let run_command = Some(exp.run_command.clone())
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            project_row
                .run_command
                .clone()
                .filter(|c| !c.trim().is_empty())
        })
        .ok_or_else(|| anyhow!("{}", crate::invocation::no_run_command(&project_row.id)))?;

    let name = gcp::instance_name(&run_id);
    let mut descriptor = BackendDescriptor {
        ssh_container: None,
        monitoring_error: None,
        cancellation_accepted: false,
        kind: "gcp_job".to_string(),
        namespace: Some(project.clone()),
        job_id: Some(name.clone()),
        flavor: Some(flavor),
        image: None,
        url: Some(format!(
            "https://console.cloud.google.com/compute/instancesDetail/zones/{zone}/instances/{name}?project={project}"
        )),
        context: Some(zone.clone()),
        manifest: None,
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: Some(timeout_secs),
        source_digest: None,
        source_path: None,
        source_size: None,
    };
    source.apply_to_descriptor(&mut descriptor);
    // Record the handle first so an interrupted submit can still find the VM.
    crate::compute::record_submission_handle(&run_id, &descriptor)?;
    gcp::create(&gcp::CreateSpec {
        project: &project,
        zone: &zone,
        name: &name,
        run_id: &run_id,
        shape: &shape,
        settings: &settings,
        ssh_public_key: &public_key,
        timeout_secs,
    })
    .await?;

    let run = StoredRun {
        id: run_id.clone(),
        experiment_id: exp.id.clone(),
        project_id: project_row.id.clone(),
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
    // From here the VM is billing: never leak it behind an error the store
    // doesn't know about.
    let persisted = store
        .upsert_run(&run)
        .and_then(|()| spawn_detached_supervise(&run_id));
    if let Err(err) = persisted {
        eprintln!("submit failed after the VM was created — deleting VM {name}");
        if let Err(delete_error) = gcp::delete(&project, &zone, &name).await {
            eprintln!(
                "warning: VM {name} could not be deleted ({delete_error}). Delete it with \
                 `gcloud compute instances delete {name} --project={project} --zone={zone}`."
            );
        }
        return Err(err);
    }
    Ok(run)
}
