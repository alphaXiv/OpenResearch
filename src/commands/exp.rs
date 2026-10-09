//! The `exp` command group: operate on a single experiment node by id.
//!
//!   orx exp status <expId>            inspect status, run command, latest run
//!   orx exp run    <expId> …          launch a local orx-supervised run
//!   orx exp cancel <expId>            cancel the in-flight run
//!   orx exp wake   <expId>            resume this agent when the run succeeds or fails
//!   orx exp archive <expId> --ancestors|--only|--descendants   hide the chosen scope
//!   orx exp unarchive <expId> --ancestors|--only|--descendants restore the same nodes
//!
//! Unlike the project-scoped data commands, every verb here takes an
//! *experiment* id from `orx project view <projectId>`.

use std::time::{Duration, Instant};

use crate::error::{anyhow, Result};
use crate::plane::{resolve_experiment, resolve_project};
use crate::store::Store;
use crate::ExpCommand;

pub async fn run(args: crate::ExpArgs) -> Result<()> {
    let mut store = Store::open()?;
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
        ExpCommand::Cancel { exp_id } => resolve_experiment(store, &exp_id)?.cancel().await,
        ExpCommand::Wake { exp_id } => wake(&store, &exp_id),
        ExpCommand::Wait {
            exp_id,
            project,
            timeout,
            interval,
        } => wait(store, exp_id, project, timeout, interval).await,
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
        store.remove_run_wakeup(&run.id, &session_id)?;
        println!("Run {} was cancelled; no wake-up scheduled.", run.id);
        return Ok(());
    }
    match store.register_run_wakeup(&run.id, &session_id)? {
        crate::store::RunWakeupRegistration::Scheduled => {
            println!("Wake-up scheduled for run {}.", run.id);
        }
        crate::store::RunWakeupRegistration::AlreadyPending => {
            println!("Wake-up already scheduled for run {}.", run.id);
        }
        crate::store::RunWakeupRegistration::AlreadyDelivered => {
            println!("Run {} already delivered its wake-up.", run.id);
        }
    }
    Ok(())
}

/// `orx exp wait …` — block on run state, for agents driving a research loop.
///
/// Two modes, picked by argument:
///   - `<expId>` — level trigger: poll the experiment's latest run until it reaches a terminal state (done/failed/cancelled).
///   - `--project` — edge trigger: snapshot every run in the project and return when the first run *completes* — i.e. transitions into a terminal state (done/failed/cancelled). This is the "a slot just freed" signal a budget-saturation loop wants; run starts and queued→running transitions are intentionally ignored.
///
/// Polls every `--interval` seconds (default 5), gives up after `--timeout`
/// seconds (default 1800) with a non-zero exit so callers can branch on it. The
/// Polling reads only the local run store.
async fn wait(
    store: Store,
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
            resolve_experiment(store, &exp_id)?
                .wait_experiment(interval, deadline)
                .await
        }
        (None, Some(project_id)) => {
            resolve_project(store, &project_id)?
                .wait_project(interval, deadline)
                .await
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

const SUPERVISOR_LOG_MAX_BYTES: u64 = 1024 * 1024;

/// Spawn `orx supervise <runId>` fully detached (own process group, stderr to a log),
/// so it outlives this command and any SSH session that launched it.
pub(crate) fn spawn_detached_supervise(run_id: &str) -> Result<()> {
    let exe = crate::paths::spawnable_exe().map_err(|e| {
        anyhow!(
            "Could not locate the orx binary to spawn the supervisor: {}",
            e
        )
    })?;
    // Supervisor diagnostics (retries, transitions) exist only on stderr; keep them per run.
    let path = crate::store::log_path(run_id).with_extension("supervisor.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() >= SUPERVISOR_LOG_MAX_BYTES) {
        // Rename rather than truncate: a still-running supervisor keeps writing to its handle.
        let _ = std::fs::rename(&path, path.with_extension("log.1"));
    }
    let mut child = match supervise_command(&exe, run_id, &path).spawn() {
        // `/proc/self/exe` can name a path this process cannot exec (OR-334); the `orx` on PATH still runs.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match crate::local::shell_env::find_on_path("orx").filter(|orx| *orx != exe) {
                Some(orx) => {
                    let tried = format!("`{}`{}: {error}", exe.display(), current_exe_note(&exe));
                    let child = supervise_command(&orx, run_id, &path)
                        .spawn()
                        .map_err(|e| {
                            anyhow!(
                                "{} First tried {tried}.",
                                supervise_spawn_error(&orx, run_id, e)
                            )
                        })?;
                    let warning = format!(
                        "warning: could not run {tried}; started the supervisor with `{}`.",
                        orx.display()
                    );
                    eprintln!("{warning}");
                    // A forwarded launch prints this on `orx up`'s stderr; keep it beside the run too.
                    if let Ok(mut log) = std::fs::OpenOptions::new().append(true).open(&path) {
                        use std::io::Write as _;
                        let _ = log.write_all(format!("{warning}\n").as_bytes());
                    }
                    child
                }
                None => return Err(supervise_spawn_error(&exe, run_id, error)),
            }
        }
        result => result.map_err(|e| supervise_spawn_error(&exe, run_id, e))?,
    };
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn supervise_command(
    exe: &std::path::Path,
    run_id: &str,
    log_path: &std::path::Path,
) -> std::process::Command {
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_or_else(|_| std::process::Stdio::null(), std::process::Stdio::from);
    // A long-lived `orx up` may be running a replaced binary; spawn the new file at its path.
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("supervise")
        .arg(run_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(stderr);
    // The supervisor re-resolves its directories from its own environment, so
    // without this a run launched from the macOS app is tracked in a different
    // store than the app is reading.
    if let Some(path) = crate::local::shell_env::search_path() {
        cmd.env("PATH", path);
    }
    crate::local::shell_env::export_to(|key, value| {
        cmd.env(key, value);
    });
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // A console-less parent would otherwise give the supervisor a visible console window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    cmd
}

/// Names the binary that failed and how to attach a supervisor by hand, since
/// the run stays `starting` without one.
fn supervise_spawn_error(
    exe: &std::path::Path,
    run_id: &str,
    error: std::io::Error,
) -> crate::error::Error {
    let missing = if exe.exists() {
        ""
    } else {
        " (no file at that path)"
    };
    let current = if crate::paths::spawnable_exe().is_ok_and(|own| own == exe) {
        current_exe_note(exe)
    } else {
        String::new()
    };
    anyhow!(
        "Could not spawn `{} supervise {run_id}`{missing}{current}: {error}. A running `orx up` retries \
         within about 6 minutes; otherwise start `{} supervise {run_id}` in the background.",
        exe.display(),
        crate::invocation::orx(),
    )
}

/// The kernel's own name for this binary when it differs from the spawned path, e.g. `… (deleted)`.
fn current_exe_note(exe: &std::path::Path) -> String {
    std::env::current_exe()
        .ok()
        .filter(|raw| raw != exe)
        .map(|raw| format!(" (current_exe: {})", raw.display()))
        .unwrap_or_default()
}

/// Who asked `orx exp cancel` to stop a run, recorded on the run.
pub(crate) fn exp_cancel_reason(chat_session_id: Option<&str>) -> String {
    match chat_session_id {
        Some(id) => format!("Cancel requested by agent session {id} with `orx exp cancel`."),
        None => "Cancel requested with `orx exp cancel`.".into(),
    }
}

/// Persist cancel intent and ensure an orphaned run gets a fresh supervisor.
pub(crate) fn request_local_run_cancel(store: &Store, run_id: &str, reason: &str) -> Result<()> {
    let lock_path = crate::store::log_path(run_id).with_extension("cancel.lock");
    request_local_run_cancel_with(
        store,
        run_id,
        reason,
        &lock_path,
        || {},
        spawn_detached_supervise,
    )
}

fn request_local_run_cancel_with(
    store: &Store,
    run_id: &str,
    reason: &str,
    lock_path: &std::path::Path,
    before_lock: impl FnOnce(),
    spawn: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)?;
    let mut cancel_lock = fd_lock::RwLock::new(lock_file);
    before_lock();
    let _cancel_guard = cancel_lock.write()?;
    store
        .get_run(run_id)?
        .ok_or_else(|| anyhow!("Run {run_id} not found in the local store."))?;
    let requested = store.request_cancel(run_id, reason)?;
    if let Err(spawn_err) = spawn(run_id) {
        if requested {
            if let Err(rollback_err) = store.withdraw_cancel(run_id) {
                return Err(anyhow!(
                    "Could not recover the supervisor: {spawn_err}; could not restore retryable cancel state: {rollback_err}"
                ));
            }
        }
        return Err(spawn_err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoredRun;

    #[test]
    fn supervise_spawn_error_names_the_binary_and_the_manual_attach() {
        let message = supervise_spawn_error(
            std::path::Path::new("/nonexistent/orx"),
            "run-1",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        )
        .to_string();
        assert!(message.contains("`/nonexistent/orx supervise run-1` (no file at that path)"));
        assert!(message.ends_with(" supervise run-1` in the background."));
    }

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
    fn failed_supervisor_spawn_restores_cancel_retry() {
        let dir =
            std::env::temp_dir().join(format!("orx-cancel-spawn-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let run = run_fixture();
        store.upsert_run(&run).unwrap();
        let lock_path = dir.join("cancel.lock");

        let result = request_local_run_cancel_with(
            &store,
            &run.id,
            "test",
            &lock_path,
            || {},
            |_| Err(anyhow!("synthetic spawn failure")),
        );
        assert!(result.is_err());
        let stored = store.get_run(&run.id).unwrap().unwrap();
        assert!(!stored.cancel_requested);
        // A supervisor that already sent the cancel still ends the run with its requester.
        store
            .update_status(&run.id, crate::store::RunStatus::Cancelled, Some(1), None)
            .unwrap();
        let cancelled = crate::plane::Run::from(&store.get_run(&run.id).unwrap().unwrap());
        assert_eq!(cancelled.failure_detail().as_deref(), Some("reason: test"));

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
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
            request_local_run_cancel_with(&store, run_id, reason, &lock_path, || {}, |_| Ok(()))
                .unwrap();
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

    #[test]
    fn concurrent_spawn_failure_preserves_successful_cancel_intent() {
        let dir = std::env::temp_dir().join(format!(
            "orx-cancel-concurrency-test-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::open_at(dir.clone()).unwrap();
        let run = run_fixture();
        store.upsert_run(&run).unwrap();
        drop(store);
        let lock_path = dir.join("cancel.lock");
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (attempted_tx, attempted_rx) = std::sync::mpsc::channel();
        let (completed_tx, completed_rx) = std::sync::mpsc::channel();

        let first_dir = dir.clone();
        let first_lock = lock_path.clone();
        let first = std::thread::spawn(move || {
            let store = Store::open_at(first_dir).unwrap();
            request_local_run_cancel_with(
                &store,
                "run-1",
                "first",
                &first_lock,
                || {},
                |_| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Err(anyhow!("synthetic spawn failure"))
                },
            )
            .unwrap_err();
        });
        entered_rx.recv().unwrap();

        let second_dir = dir.clone();
        let second_lock = lock_path.clone();
        let second = std::thread::spawn(move || {
            let store = Store::open_at(second_dir).unwrap();
            request_local_run_cancel_with(
                &store,
                "run-1",
                "second",
                &second_lock,
                || attempted_tx.send(()).unwrap(),
                |_| Ok(()),
            )
            .unwrap();
            completed_tx.send(()).unwrap();
        });
        attempted_rx.recv().unwrap();
        let completed_while_locked = completed_rx
            .recv_timeout(std::time::Duration::from_millis(250))
            .is_ok();
        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();

        let store = Store::open_at(dir.clone()).unwrap();
        assert!(!completed_while_locked);
        let run = store.get_run("run-1").unwrap().unwrap();
        assert!(run.cancel_requested);
        assert_eq!(run.cancel_reason.as_deref(), Some("second"));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
