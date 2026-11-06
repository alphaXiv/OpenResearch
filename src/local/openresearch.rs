//! OpenResearch launch intents: an ephemeral org-billed box for one run. The engine
//! provisions it at launch; the legacy loop waits for it, runs the payload over SSH, collects
//! the log, and releases the box.

use crate::client::{create_sandbox, list_orgs, CreateSandboxBody};
use crate::commands::exp::spawn_detached_supervise;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{openresearch, BackendDescriptor};
use crate::local::ssh_identity;
use crate::store::{now_ms, LaunchSpec, Store, StoredRun};

/// Requires `--flavor <shape>` (`h100_sxm[:count]` or `cpu5c|cpu5g|cpu5m[:vcpus]`) and
/// `orx login`. The box is provisioned by the engine at launch.
pub async fn launch_spec(args: &crate::ExpRunArgs) -> Result<LaunchSpec> {
    if args.host.is_some() {
        return Err(anyhow!(
            "--host doesn't apply to --backend openresearch — the box is provisioned \
             for you (use --backend ssh to run on your own machine)."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest is k8s-only."));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend openresearch — provider base images are fixed \
             by OpenResearch; install project dependencies from the project's lockfile in the \
             run command."
        ));
    }
    let flavor = args.flavor.clone().ok_or_else(|| {
        anyhow!(
            "--backend openresearch requires --flavor: a GPU id like h100_sxm[:count] \
             or a CPU flavor like cpu5c[:vcpus]. See `orx compute` for the catalog."
        )
    })?;
    // Parsed now so a typo fails before any billing starts.
    openresearch::parse_flavor(&flavor, args.disk.unwrap_or(100), args.provider.clone())?;
    let timeout_seconds = match &args.timeout {
        Some(t) => crate::jobs::huggingface::parse_timeout(t)?,
        None => 4 * 3600,
    };

    let creds = crate::config::load_credentials()
        .await?
        .ok_or_else(|| anyhow!("OpenResearch requires `orx login`."))?;

    // Which org pays for the box.
    let org_id = match &args.org {
        Some(org) => org.clone(),
        None => {
            let orgs = list_orgs(&creds).await?.orgs;
            match orgs.len() {
                0 => {
                    return Err(anyhow!(
                        "You belong to no organization — create an OpenResearch organization first."
                    ))
                }
                1 => orgs[0].id.clone(),
                _ => {
                    let rows = orgs
                        .iter()
                        .map(|o| format!("  {}  {}", o.id, o.name))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Err(anyhow!(
                        "You belong to several orgs — pass --org <id>:\n{rows}"
                    ));
                }
            }
        }
    };

    // Boxes authorize org members' *registered* SSH keys, and orx authenticates
    // with whatever this machine's ssh offers — so a key registered from another
    // computer leaves the box online but unreachable.
    match ssh_identity::check(&creds).await {
        ssh_identity::KeyStatus::Matched => {}
        ssh_identity::KeyStatus::Unknown { reason } => {
            eprintln!("warning: could not check your registered SSH keys: {reason}");
        }
        // No keys at all is the api's own answer, so blocking is safe.
        ssh_identity::KeyStatus::NoneRegistered { local } => {
            return Err(anyhow!(
                "{}\n\n{}",
                ssh_identity::diagnosis(&[]),
                ssh_identity::remediation(&[], &local)
            ));
        }
        // We only enumerate the agent and ~/.ssh/*.pub, so ssh can still succeed
        // via an IdentityFile elsewhere or a key whose .pub isn't on disk. Warn
        // loudly rather than block a launch that would have worked.
        ssh_identity::KeyStatus::NoLocalMatch { registered, local } => {
            eprintln!(
                "warning: {}\n\n{}",
                ssh_identity::diagnosis(&registered),
                ssh_identity::remediation(&registered, &local)
            );
        }
    }

    Ok(LaunchSpec {
        kind: "openresearch_job".into(),
        target: Some(org_id),
        options: serde_json::json!({
            "flavor": flavor,
            "disk_gb": args.disk.unwrap_or(100),
            "provider": args.provider,
            "timeout_seconds": timeout_seconds,
        }),
    })
}

/// Reword a 402 from `POST /sandboxes` (org has no available compute balance)
/// into the top-up action, surfacing the checkout URL the API attaches.
pub(crate) fn billing_friendly(err: crate::error::Error) -> crate::error::Error {
    let text = err.to_string();
    if !text.contains("(402 ") {
        return err;
    }
    let url = text
        .split("checkoutUrl\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|u| format!(" Top up at {u}"))
        .unwrap_or_default();
    anyhow!("Your org has no available compute balance, so no box was provisioned.{url}")
}

pub async fn submit_local_openresearch_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
) -> Result<StoredRun> {
    if args.host.is_some() {
        return Err(anyhow!(
            "--host doesn't apply to --backend openresearch — the box is provisioned \
             for you (use --backend ssh to run on your own machine)."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest is k8s-only."));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend openresearch — provider base images are fixed \
             by OpenResearch; install project dependencies from the project's lockfile in the \
             run command."
        ));
    }
    let flavor = args.flavor.clone().ok_or_else(|| {
        anyhow!(
            "--backend openresearch requires --flavor: a GPU id like h100_sxm[:count] \
             or a CPU flavor like cpu5c[:vcpus]. See `orx compute` for the catalog."
        )
    })?;
    let target =
        openresearch::parse_flavor(&flavor, args.disk.unwrap_or(100), args.provider.clone())?;
    // Enforced later (the supervisor wraps the payload) — parsed now so a
    // typo fails before any billing starts. Same 4h default as hf/modal.
    let timeout_secs = match &args.timeout {
        Some(t) => crate::jobs::huggingface::parse_timeout(t)?,
        None => 4 * 3600,
    };

    let creds = crate::config::load_credentials()
        .await?
        .ok_or_else(|| anyhow!("OpenResearch requires `orx login`."))?;

    // Which org pays for the box.
    let org_id = match &args.org {
        Some(org) => org.clone(),
        None => {
            let orgs = list_orgs(&creds).await?.orgs;
            match orgs.len() {
                0 => {
                    return Err(anyhow!(
                        "You belong to no organization — create an OpenResearch organization first."
                    ))
                }
                1 => orgs[0].id.clone(),
                _ => {
                    let rows = orgs
                        .iter()
                        .map(|o| format!("  {}  {}", o.id, o.name))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Err(anyhow!(
                        "You belong to several orgs — pass --org <id>:\n{rows}"
                    ));
                }
            }
        }
    };

    // Boxes authorize org members' *registered* SSH keys, and orx authenticates
    // with whatever this machine's ssh offers — so a key registered from another
    // computer leaves the box online but unreachable.
    match ssh_identity::check(&creds).await {
        ssh_identity::KeyStatus::Matched => {}
        ssh_identity::KeyStatus::Unknown { reason } => {
            eprintln!("warning: could not check your registered SSH keys: {reason}");
        }
        // No keys at all is the api's own answer, so blocking is safe.
        ssh_identity::KeyStatus::NoneRegistered { local } => {
            return Err(anyhow!(
                "{}\n\n{}",
                ssh_identity::diagnosis(&[]),
                ssh_identity::remediation(&[], &local)
            ));
        }
        // We only enumerate the agent and ~/.ssh/*.pub, so ssh can still succeed
        // via an IdentityFile elsewhere or a key whose .pub isn't on disk. Warn
        // loudly rather than block a launch that would have worked.
        ssh_identity::KeyStatus::NoLocalMatch { registered, local } => {
            eprintln!(
                "warning: {}\n\n{}",
                ssh_identity::diagnosis(&registered),
                ssh_identity::remediation(&registered, &local)
            );
        }
    }

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

    let created = create_sandbox(
        &creds,
        &CreateSandboxBody {
            organization_id: org_id.clone(),
            target,
            rental_lease: Some(crate::client::CreateRentalLease {
                version: 1,
                max_active_seconds: timeout_secs,
            }),
        },
    )
    .await
    .map_err(billing_friendly)?;
    let sandbox = created.sandbox;
    let capability = match created.rental_lease.filter(|lease| lease.version == 1) {
        Some(capability) => capability,
        None => {
            let _ = openresearch::teardown(&creds, &sandbox.id).await;
            return Err(anyhow!("The server does not support rental supervision; update the service before launching."));
        }
    };
    let rental = crate::jobs::rental::Rental {
        api_url: creds.api_url.clone(),
        sandbox_id: sandbox.id.clone(),
        token: capability.token,
        unknown: Default::default(),
    };

    let mut descriptor = BackendDescriptor {
        ssh_container: None,
        monitoring_error: None,
        cancellation_accepted: false,
        rental_lease: Some(rental),
        kind: "openresearch_job".to_string(),
        namespace: Some(org_id),
        job_id: Some(sandbox.id.clone()),
        flavor: Some(flavor),
        image: None,
        url: None,
        context: None,
        manifest: None,
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: Some(timeout_secs),
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
        let _ = openresearch::teardown(&creds, &sandbox.id).await;
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

    // From here the box is billing: never leak it behind an error the store
    // doesn't know about.
    let persisted = store
        .upsert_run(&run)
        .and_then(|()| spawn_detached_supervise(&run_id));
    if let Err(err) = persisted {
        eprintln!(
            "submit failed after the box was provisioned — deleting box {}",
            sandbox.id
        );
        if let Err(td) = openresearch::teardown(&creds, &sandbox.id).await {
            eprintln!(
                "warning: box {} could not be torn down ({td}) — delete it with \
                 `orx instance delete {}` or through OpenResearch.",
                sandbox.id, sandbox.id
            );
        }
        return Err(err);
    }
    Ok(run)
}
