//! The `exp` command group: operate on a single experiment node by id.
//!
//!   orx exp status <expId>            inspect status, run command, latest run
//!   orx exp run    <expId> …          launch a local orx-supervised run
//!   orx exp cancel <expId>            cancel the in-flight run
//!   orx exp wake   <expId>            send this chat the run's result when it finishes
//!   orx exp archive <expId> --ancestors|--only|--descendants   hide the chosen scope
//!   orx exp unarchive <expId> --ancestors|--only|--descendants restore the same nodes
//!   orx exp move <expId> --parent <expId>|--baseline          re-parent a subtree
//!   orx exp delete <expId>            permanently delete a leaf node, its runs and branch
//!
//! Unlike the project-scoped data commands, every verb here takes an
//! *experiment* id from `orx project view <projectId>`.

use std::time::{Duration, Instant};

use crate::error::{anyhow, Result};
use crate::plane::{resolve_experiment, resolve_project};
use crate::store::Store;
use crate::ExpCommand;

pub async fn run(args: crate::ExpArgs) -> Result<()> {
    if let ExpCommand::Abandon { run } = &args.command {
        return crate::plane::local_plane::ApiClient::discover()
            .await?
            .abandon(run)
            .await;
    }
    // `exp wait` opens its store itself: a transiently locked orx.db is
    // retried inside the wait timeout rather than dying at startup.
    if let ExpCommand::Wait {
        exp_id,
        project,
        timeout,
        interval,
    } = args.command
    {
        return wait(exp_id, project, timeout, interval).await;
    }
    let mut store = match &args.command {
        ExpCommand::Status { .. }
        | ExpCommand::Desc {
            set: None,
            stdin: false,
            ..
        } => Store::open_for_reading()?,
        _ => Store::open()?,
    };
    match args.command {
        ExpCommand::Archive {
            exp_id,
            ancestors,
            only,
            descendants,
            ..
        } => archive(&mut store, &exp_id, ancestors, only, descendants, true),
        ExpCommand::Unarchive {
            exp_id,
            ancestors,
            only,
            descendants,
            ..
        } => archive(&mut store, &exp_id, ancestors, only, descendants, false),
        ExpCommand::Delete { exp_id } => delete(&store, &exp_id),
        ExpCommand::Move { exp_id, parent, .. } => reparent(&store, &exp_id, parent.as_deref()),
        ExpCommand::Status { exp_id, scheduler } => {
            crate::local::chat::record_chat_target("experiments", &exp_id);
            resolve_experiment(store, &exp_id)?
                .experiment_status(scheduler)
                .await
        }
        ExpCommand::Desc { exp_id, set, stdin } => {
            crate::local::chat::record_chat_target("experiments", &exp_id);
            resolve_experiment(store, &exp_id)?
                .experiment_desc(set, stdin)
                .await
        }
        ExpCommand::Run(run_args) => {
            let run_args = *run_args;
            resolve_experiment(store, &run_args.exp_id)?
                .launch(run_args)
                .await
        }
        ExpCommand::Cancel { exp_id, run } => match exp_id {
            Some(exp_id) => {
                resolve_experiment(store, &exp_id)?
                    .cancel(run.as_deref())
                    .await
            }
            None => {
                crate::plane::local_plane::ApiClient::discover()
                    .await?
                    .cancel(None, run.as_deref())
                    .await
            }
        },
        ExpCommand::Wake { exp_id } => wake(&store, &exp_id),
        ExpCommand::Wait { .. } | ExpCommand::Abandon { .. } => unreachable!("handled above"),
    }
}

fn archive(
    store: &mut Store,
    id: &str,
    ancestors: bool,
    only: bool,
    descendants: bool,
    archived: bool,
) -> Result<()> {
    let direction = if ancestors {
        crate::local::experiments::ArchiveDirection::Ancestors
    } else if only {
        crate::local::experiments::ArchiveDirection::Only
    } else if descendants {
        crate::local::experiments::ArchiveDirection::Descendants
    } else {
        unreachable!("clap requires an archive scope")
    };
    let ids = crate::local::experiments::set_archived(store, id, direction, archived)?;
    println!(
        "{} {} experiment(s).",
        if archived { "Archived" } else { "Restored" },
        ids.len()
    );
    Ok(())
}

fn delete(store: &Store, id: &str) -> Result<()> {
    let deleted = crate::local::experiments::delete_experiment(store, id)?;
    println!(
        "Deleted experiment {} ({}) and {} run(s).",
        deleted.experiment.id,
        deleted.experiment.display_name(),
        deleted.runs
    );
    if let Some(sha) = &deleted.branch_commit {
        let branch = &deleted.experiment.branch_name;
        println!("  Deleted branch {branch} (restore with `git branch {branch} {sha}`).");
    }
    Ok(())
}

fn reparent(store: &Store, id: &str, parent: Option<&str>) -> Result<()> {
    let before = crate::local::experiments::move_experiment(store, id, parent)?;
    println!(
        "Moved {} ({}) from {} to {}.",
        before.id,
        before.display_name(),
        before.parent_experiment_id.as_deref().unwrap_or("a root"),
        parent.unwrap_or("a new root")
    );
    println!("  Git branch {} is unchanged.", before.branch_name);
    Ok(())
}

fn wake(store: &Store, exp_id: &str) -> Result<()> {
    if !crate::local::chat::in_local_session() {
        return Err(anyhow!(
            "`orx exp wake` is only available inside a local `orx up` agent session."
        ));
    }
    let session_id = crate::local::chat::launching_chat_session()
        .ok_or_else(|| anyhow!("This agent session has no chat id to wake."))?;
    if store.get_chat_session(&session_id)?.is_none() {
        return Err(anyhow!("The current chat session no longer exists."));
    }
    let run = store
        .latest_run_for_experiment(exp_id)?
        .ok_or_else(|| anyhow!("Experiment {exp_id} has no runs to wake for."))?;
    if run.status == "cancelled" {
        println!("Run {} was cancelled; no wake-up scheduled.", run.id);
        return Ok(());
    }
    let now = crate::store::now_ms();
    store.subscribe_run(&run.id, &session_id, now)?;
    if !crate::local::is_terminal(&run.status) {
        println!("Wake-up scheduled for run {}.", run.id);
        return Ok(());
    }
    let body = store.terminal_notice_body(&run)?;
    let terminal = crate::store::TERMINAL;
    if store.insert_run_notice(&run.id, &session_id, terminal, terminal, &body, now)? {
        println!(
            "Run {} already finished; its result is queued for this chat.",
            run.id
        );
    } else {
        println!("Run {} already delivered its result to this chat.", run.id);
    }
    Ok(())
}

/// `orx exp wait …` — block on run state, one server event stream at a time.
///
/// Two modes, picked by argument:
///   - `<expId>` — level trigger: wait until the experiment's latest run reaches a terminal state (done/failed/cancelled).
///   - `--project` — edge trigger: snapshot every run in the project and return when the first run *completes* — i.e. transitions into a terminal state (done/failed/cancelled). A cancelled result can still report surviving processes; run starts and queued→running transitions are intentionally ignored.
///
/// Gives up after `--timeout` seconds (default 1800) with a non-zero exit so callers can
/// branch on it.
async fn wait(
    exp_id: Option<String>,
    project: Option<String>,
    timeout: Option<u64>,
    interval: Option<u64>,
) -> Result<()> {
    let interval = Duration::from_secs(interval.unwrap_or(5).max(1));
    let deadline = Instant::now() + Duration::from_secs(timeout.unwrap_or(1800));

    match (exp_id, project) {
        (Some(_), Some(_)) => Err(anyhow!("Pass either <expId> or --project, not both.")),
        (None, None) => Err(anyhow!(
            "Specify what to wait on: `orx exp wait <expId>` (one run) or \
             `orx exp wait --project <projectId>` (any run in a project)."
        )),
        (Some(exp_id), None) => {
            let plane = crate::plane::local_plane::retry_on_db_lock(
                || {
                    let store = Store::open_for_reading_within(
                        crate::plane::local_plane::busy_timeout_left(deadline),
                    )?;
                    resolve_experiment(store, &exp_id)
                },
                interval,
                deadline,
            )
            .await?;
            plane.wait(deadline).await
        }
        (None, Some(project_id)) => {
            let plane = crate::plane::local_plane::retry_on_db_lock(
                || {
                    let store = Store::open_for_reading_within(
                        crate::plane::local_plane::busy_timeout_left(deadline),
                    )?;
                    resolve_project(store, &project_id)
                },
                interval,
                deadline,
            )
            .await?;
            plane.wait(deadline).await
        }
    }
}

// --- job-launch helpers shared with the src/local/* backends -----------------

/// Default docker image per flavor family: plain python for CPU flavors, a
/// CUDA-ready pytorch image for GPU flavors. Override with --image.
pub(crate) fn default_hf_image(flavor: &str) -> String {
    if flavor.starts_with("cpu") {
        "python:3.12".to_string()
    } else {
        "pytorch/pytorch:2.6.0-cuda12.4-cudnn9-runtime".to_string()
    }
}

/// Who asked `orx exp cancel` to stop a run, recorded on the run.
pub(crate) fn exp_cancel_reason(chat_session_id: Option<&str>) -> String {
    match chat_session_id {
        Some(id) => format!("Cancel requested by agent session {id} with `orx exp cancel`."),
        None => "Cancel requested with `orx exp cancel`.".into(),
    }
}

/// Records cancel intent for a run a 0.2.x supervisor owns, under the `cancel.lock` it reads
/// the intent under.
pub(crate) fn request_legacy_run_cancel(store: &Store, run_id: &str, reason: &str) -> Result<()> {
    let lock_path =
        crate::store::log_path(&crate::store::data_dir(), run_id).with_extension("cancel.lock");
    request_legacy_run_cancel_at(store, run_id, reason, &lock_path)
}

fn request_legacy_run_cancel_at(
    store: &Store,
    run_id: &str,
    reason: &str,
    lock_path: &std::path::Path,
) -> Result<()> {
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    let mut cancel_lock = fd_lock::RwLock::new(lock_file);
    let _cancel_guard = cancel_lock.write()?;
    store.get_run(run_id)?.ok_or_else(|| {
        anyhow!(
            "Run {run_id} was not found in the orx store at {}.",
            store.data_dir().display()
        )
    })?;
    store.request_cancel(run_id, reason)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn wait_validates_both_and_neither_targets_before_opening_the_store() {
        assert!(
            super::wait(Some("exp".into()), Some("project".into()), Some(1), None)
                .await
                .unwrap_err()
                .to_string()
                .contains("either")
        );
        assert!(super::wait(None, None, Some(1), None)
            .await
            .unwrap_err()
            .to_string()
            .contains("Specify"));
    }

    use super::*;
    use crate::store::StoredRun;

    fn run_fixture() -> StoredRun {
        StoredRun {
            id: "run-1".into(),
            experiment_id: "experiment-1".into(),
            project_id: "project-1".into(),
            status: "running".into(),
            backend_json: "{}".into(),
            command: String::new(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }
    }

    #[test]
    fn cancel_reason_keeps_the_first_requester_and_shows_only_when_cancelled() {
        use crate::store::RunStatus;
        let dir =
            std::env::temp_dir().join(format!("orx-cancel-reason-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let lock_path = dir.join("cancel.lock");
        let starting = StoredRun {
            status: "starting".into(),
            ..run_fixture()
        };
        let finishing = StoredRun {
            id: "run-done".into(),
            ..run_fixture()
        };
        store.upsert_run(&starting).unwrap();
        store.upsert_run(&finishing).unwrap();

        for (run_id, reason) in [
            ("run-1", "first"),
            ("run-1", "second"),
            ("run-done", "first"),
        ] {
            request_legacy_run_cancel_at(&store, run_id, reason, &lock_path).unwrap();
        }
        // A backend's submit upsert lands after the cancel request.
        store.upsert_run(&starting).unwrap();
        store
            .update_status("run-1", RunStatus::Cancelled, Some(2), None)
            .unwrap();
        store
            .update_status("run-done", RunStatus::Done, Some(2), Some(0))
            .unwrap();

        let detail =
            |id| crate::plane::Run::from(&store.get_run(id).unwrap().unwrap()).failure_detail();
        assert_eq!(detail("run-1").as_deref(), Some("reason: first"));
        assert_eq!(detail("run-done"), None);
        // The reason never leaks into a finished run's result.
        assert_eq!(
            store.get_run("run-done").unwrap().unwrap().result_markdown,
            None
        );

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
