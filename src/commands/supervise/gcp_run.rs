//! Compute Engine runs, owned by the tuandang304 fork.

use super::*;
use crate::jobs::gcp;

// --- gcp ---------------------------------------------------------------------
//
// The openresearch flow on a Compute Engine VM: wait for it to run, launch over
// ssh, watch with the shared ssh loop, and delete the VM at the end.

pub(super) async fn run_gcp(
    store: Store,
    stored: crate::store::StoredRun,
    mut descriptor: BackendDescriptor,
    run_id: String,
) -> Result<()> {
    let (project, zone, name) = descriptor.gcp_ref()?;
    let (project, zone, name) = (project.to_string(), zone.to_string(), name.to_string());
    let fail = |reason: String| -> Result<()> {
        if store.update_status(&run_id, RunStatus::Failed, Some(now_ms()), None)? {
            store.set_result_markdown(&run_id, &reason)?;
        }
        Ok(())
    };

    let target = match descriptor.gcp_ssh_target() {
        Some(target) => target,
        None => {
            eprintln!("supervise {run_id}: waiting for Google Cloud VM {name} to start");
            let ip = match gcp::wait_running(&project, &zone, &name, || {
                local_cancel_requested(&store, &run_id)
            })
            .await
            {
                gcp::WaitOutcome::Running(ip) => ip,
                gcp::WaitOutcome::Cancelled => {
                    store.update_status(&run_id, RunStatus::Cancelled, Some(now_ms()), None)?;
                    teardown_gcp(&store, &project, &zone, &name, &run_id).await;
                    return Ok(());
                }
                gcp::WaitOutcome::Failed(reason) => {
                    fail(format!("Provisioning failed: {reason}."))?;
                    teardown_gcp(&store, &project, &zone, &name, &run_id).await;
                    return Ok(());
                }
            };
            descriptor.ssh_host = Some(ip);
            descriptor.ssh_user = Some(gcp::SSH_USER.to_string());
            descriptor.ssh_port = Some(22);
            store.set_backend_json(&run_id, &descriptor.to_json())?;
            descriptor
                .gcp_ssh_target()
                .ok_or_else(|| anyhow!("VM {name} started without a reachable SSH target"))?
        }
    };

    let already_launched = openresearch::launched(&target, &run_id)
        .await
        .unwrap_or(false);
    if !already_launched {
        let source = match crate::compute::SourceSnapshot::from_run(&stored, &descriptor) {
            Ok(source) => source,
            Err(error) => {
                fail(format!(
                    "The recorded source snapshot could not be loaded: {error}"
                ))?;
                teardown_gcp(&store, &project, &zone, &name, &run_id).await;
                return Ok(());
            }
        };
        let mut script = crate::compute::staged_script(&stored.command);
        let is_gpu = descriptor
            .flavor
            .as_deref()
            .and_then(|flavor| gcp::parse_flavor(flavor).ok())
            .is_some_and(|shape| shape.gpu.is_some());
        if is_gpu {
            script = gcp::wait_for_gpu(&script);
        }
        let script =
            openresearch::wrap_with_timeout(&script, descriptor.timeout_secs.unwrap_or(4 * 3600));
        let mut env: std::collections::HashMap<String, String> =
            crate::config::list_synced_env().into_iter().collect();
        if let Ok(hf_token) = hf::resolve_token() {
            env.entry("HF_TOKEN".to_string()).or_insert(hf_token);
        }
        // sshd and the metadata key agent come up a little after RUNNING.
        let mut launch_err = None;
        for backoff_secs in [0u64, 10, 15, 20, 30, 30, 45, 60, 60] {
            if backoff_secs > 0 {
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            }
            if local_cancel_requested(&store, &run_id) {
                store.update_status(&run_id, RunStatus::Cancelled, Some(now_ms()), None)?;
                teardown_gcp(&store, &project, &zone, &name, &run_id).await;
                return Ok(());
            }
            if let Err(err) =
                ssh::stage_source(&target, &run_id, &source.path, &source.digest, None, None).await
            {
                eprintln!("supervise {run_id}: source staging failed (will retry): {err}");
                launch_err = Some(err);
                continue;
            }
            match ssh::run_job(&ssh::SshJobSpec {
                container: None,
                target: target.clone(),
                run_id: run_id.clone(),
                script: script.clone(),
                env: env.clone(),
                remote_root: None,
            })
            .await
            {
                Ok(_) => {
                    launch_err = None;
                    break;
                }
                Err(err) => {
                    eprintln!("supervise {run_id}: launch failed (will retry): {err}");
                    launch_err = Some(err);
                }
            }
        }
        if let Some(err) = launch_err {
            let err = err.to_string();
            let hint = if err.contains("Permission denied") || err.contains("publickey") {
                "\n\nIf the project enforces OS Login through an organization policy, orx's \
                 SSH key is ignored; use another project or the ssh backend."
            } else {
                ""
            };
            fail(format!(
                "Could not start the run on Google Cloud VM {name}: {err}{hint}"
            ))?;
            teardown_gcp(&store, &project, &zone, &name, &run_id).await;
            return Ok(());
        }
    }

    eprintln!(
        "supervise {run_id}: watching Google Cloud VM {name} ({})",
        target.dest
    );
    let watch = watch_ssh_job(
        &store,
        status_of(&stored)?,
        target,
        openresearch::run_dir(&run_id),
        &run_id,
        &mut descriptor,
        None,
    )
    .await;
    teardown_gcp(&store, &project, &zone, &name, &run_id).await;
    watch?;
    Ok(())
}

/// Delete the run's VM; on failure leave a cleanup hint on the run.
async fn teardown_gcp(store: &Store, project: &str, zone: &str, name: &str, run_id: &str) {
    match gcp::delete(project, zone, name).await {
        Ok(()) => eprintln!("supervise {run_id}: Google Cloud VM {name} deleted"),
        Err(err) => {
            eprintln!("supervise {run_id}: Google Cloud VM {name} could NOT be deleted: {err}");
            let existing = store
                .get_run(run_id)
                .ok()
                .flatten()
                .and_then(|r| r.result_markdown)
                .unwrap_or_default();
            let hint = format!(
                "\n\n> **Warning**: Google Cloud VM {name} could not be deleted ({err}) and is \
                 still billing. Delete it with `gcloud compute instances delete {name} \
                 --project={project} --zone={zone}`. Compute Engine deletes it on its own once \
                 the run's time limit plus one hour has passed."
            );
            let _ = store.set_result_markdown(run_id, &format!("{existing}{hint}"));
        }
    }
}
