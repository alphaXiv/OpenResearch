//! Local launch — the on-this-machine twin of `local/ssh.rs`: run the
//! experiment as a detached process on the machine running orx. Same snapshot
//! contract as every backend — the run extracts the recorded revision into
//! its own run dir, never the agent's worktree. The run row lives in the
//! local store only; a detached `orx supervise` watches the process.

use std::collections::HashMap;

use crate::commands::exp::spawn_detached_supervise;
use crate::compute::SourceSnapshot;
use crate::error::{anyhow, Result};
use crate::jobs::{localbox, BackendDescriptor};
use crate::store::{now_ms, Store, StoredRun};

pub(crate) fn mount_namespace(backend: &str) -> Result<Option<String>> {
    if backend != "local" {
        return Ok(None);
    }
    #[cfg(target_os = "linux")]
    {
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(|error| anyhow!("Could not identify the local run's Linux boot: {error}"))?;
        let namespace = std::fs::read_link("/proc/self/ns/mnt").map_err(|error| {
            anyhow!("Could not identify the local run's mount namespace: {error}")
        })?;
        Ok(Some(format!("{}/{}", boot.trim(), namespace.display())))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

pub(crate) fn check_mount_namespace(
    store: &Store,
    project_id: &str,
    command: &str,
    namespace: Option<&str>,
) -> Result<()> {
    let Some((boot, current)) = namespace.and_then(|namespace| namespace.split_once('/')) else {
        return Ok(());
    };
    for run in store.list_runs_by_project(project_id)? {
        if !matches!(run.status.as_str(), "starting" | "running" | "done") || run.command != command
        {
            continue;
        }
        let descriptor = BackendDescriptor::parse(&run.backend_json)?;
        if descriptor.kind != "local_job" {
            continue;
        }
        let Some((previous_boot, previous)) = descriptor
            .namespace
            .as_deref()
            .and_then(|namespace| namespace.split_once('/'))
        else {
            continue;
        };
        if previous_boot == boot && previous != current {
            return Err(anyhow!(
                "Local mount namespace mismatch: this session uses {current}, but run {} \
                 uses {previous} for the same project and command. Data, Conda and GPU mounts \
                 may not be visible here. Launch from the same mounted environment as that run; \
                 no run was created.",
                run.id
            ));
        }
    }
    Ok(())
}

/// Submit a local controller and print its run directory.
pub async fn launch_local_run(args: &crate::ExpRunArgs) -> Result<()> {
    let run = crate::compute::submit(args).await?;
    let backend = BackendDescriptor::parse(&run.backend_json)?;
    let label = if backend.kind == "tinker_job" {
        "Tinker"
    } else {
        "Local"
    };
    println!("\u{2713} {label} run started.");
    println!("  dir  {}", backend.job_id.as_deref().unwrap_or(""));
    println!("  run  {}", run.id);
    println!(
        "{}",
        crate::invocation::follow_up(&run.experiment_id, &run.id)
    );
    Ok(())
}

pub async fn submit_local_run_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
) -> Result<StoredRun> {
    submit_controller_run(args, source, run_id, "local_job").await
}

pub async fn submit_tinker_run_with_source(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
) -> Result<StoredRun> {
    submit_controller_run(args, source, run_id, "tinker_job").await
}

async fn submit_controller_run(
    args: &crate::ExpRunArgs,
    source: SourceSnapshot,
    run_id: String,
    kind: &str,
) -> Result<StoredRun> {
    let backend = kind.trim_end_matches("_job");
    if args.flavor.is_some() {
        return Err(anyhow!("--backend {backend} does not take --flavor."));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend {backend} — the controller uses this machine's own environment."
        ));
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

    let script = crate::compute::snapshot_script(
        &crate::local::bash::bash_path(&source.path),
        &crate::local::bash::local_run_command(&run_command),
    );

    // The run's env: everything the user synced (API keys), plus the tokens
    // the run script expects. Passed to the launcher process.
    let mut env = crate::config::run_env(&run_id);
    env.insert(
        crate::compute::TENSORBOARD_LOGDIR.to_string(),
        crate::jobs::localbox::run_dir(&run_id)
            .join("tensorboard")
            .to_string_lossy()
            .into_owned(),
    );
    if let Ok(hf_token) = crate::jobs::huggingface::resolve_token() {
        env.entry("HF_TOKEN".to_string()).or_insert(hf_token);
    }
    // run.sh executes the user's own script, so it needs the shell's PATH — a
    // run launched from the macOS app would otherwise have no python/uv/conda.
    // Not on Windows: bash splits PATH on `:`, so a `C:` value collapses there.
    #[cfg(not(windows))]
    if let Some(path) = crate::local::shell_env::search_path() {
        env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    }
    crate::local::shell_env::export_to(|key, value| {
        env.insert(key.to_string(), value.to_string_lossy().into_owned());
    });
    let mut secret_env = HashMap::new();
    // Every local controller inherits this key without persisting it in run.sh.
    let tinker_key = if kind == "tinker_job" {
        Some(crate::jobs::tinker::resolve_api_key()?)
    } else {
        env.get(crate::jobs::tinker::API_KEY_ENV).cloned()
    };
    env.remove(crate::jobs::tinker::API_KEY_ENV);
    if let Some(key) = tinker_key {
        secret_env.insert(crate::jobs::tinker::API_KEY_ENV.to_string(), key);
    }

    let namespace = mount_namespace(backend)?;
    let dir = localbox::run_job(&localbox::LocalJobSpec {
        run_id: run_id.clone(),
        script,
        env,
        secret_env,
    })?;

    let mut descriptor = BackendDescriptor {
        ssh_container: None,
        monitoring_error: None,
        cancellation_accepted: false,
        rental_lease: None,
        kind: kind.to_string(),
        namespace,
        job_id: Some(dir.to_string_lossy().into_owned()),
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
    };
    source.apply_to_descriptor(&mut descriptor);
    if let Err(error) = crate::compute::record_submission_handle(&run_id, &descriptor) {
        let _ = localbox::cancel_job(&dir);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespace_preflight_rejects_only_comparable_local_runs() {
        let dir = std::env::temp_dir().join(format!("orx-namespace-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for (index, (kind, status, namespace, command, blocked)) in [
            (
                "local_job",
                "starting",
                Some("boot/mnt:[1]"),
                "source /data/conda.sh",
                true,
            ),
            (
                "local_job",
                "running",
                Some("boot/mnt:[1]"),
                "source /data/conda.sh",
                true,
            ),
            (
                "local_job",
                "done",
                Some("boot/mnt:[1]"),
                "source /data/conda.sh",
                true,
            ),
            (
                "local_job",
                "done",
                Some("boot/mnt:[2]"),
                "source /data/conda.sh",
                false,
            ),
            (
                "local_job",
                "done",
                Some("old-boot/mnt:[1]"),
                "source /data/conda.sh",
                false,
            ),
            ("local_job", "done", None, "source /data/conda.sh", false),
            (
                "local_job",
                "failed",
                Some("boot/mnt:[1]"),
                "source /data/conda.sh",
                false,
            ),
            (
                "local_job",
                "cancelled",
                Some("boot/mnt:[1]"),
                "source /data/conda.sh",
                false,
            ),
            (
                "ssh_job",
                "done",
                Some("boot/mnt:[1]"),
                "source /data/conda.sh",
                false,
            ),
            (
                "local_job",
                "done",
                Some("boot/mnt:[1]"),
                "new command",
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let project_id = format!("project-{index}");
            let run = StoredRun {
                id: format!("run-{index}"),
                experiment_id: "other-experiment".into(),
                project_id: project_id.clone(),
                status: status.into(),
                backend_json: serde_json::json!({"kind": kind, "namespace": namespace}).to_string(),
                command: command.into(),
                created_at: 1,
                updated_at: 1,
                ended_at: None,
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: None,
            };
            store.upsert_run(&run).unwrap();
            let result = check_mount_namespace(
                &store,
                &project_id,
                "source /data/conda.sh",
                Some("boot/mnt:[2]"),
            );
            assert_eq!(result.is_err(), blocked, "case {index}: {result:?}");
            if let Err(error) = result {
                let message = error.to_string();
                assert!(message.contains(&run.id));
                assert!(message.contains("no run was created"));
            }
            assert!(check_mount_namespace(&store, &project_id, command, None).is_ok());
            assert_eq!(store.list_runs_by_project(&project_id).unwrap().len(), 1);
        }
        assert!(check_mount_namespace(
            &store,
            "unrelated-project",
            "source /data/conda.sh",
            Some("boot/mnt:[2]"),
        )
        .is_ok());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_linux_local_runs_record_a_mount_namespace() {
        assert!(mount_namespace("ssh").unwrap().is_none());
        assert!(mount_namespace("tinker").unwrap().is_none());
        let namespace = mount_namespace("local").unwrap();
        #[cfg(target_os = "linux")]
        assert!(namespace.unwrap().contains("/mnt:["));
        #[cfg(not(target_os = "linux"))]
        assert!(namespace.is_none());
    }
}
