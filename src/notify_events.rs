//! What actually gets enqueued through `notify`'s outbox: T6's two Slack
//! events (a run submitted, a run's agent-driven outcome synthesized).
//!
//! `notify.rs` owns the transport (drain loop + Slack HTTP), agnostic of what
//! any particular feature wants to say. This module owns the message
//! building and the "should this even be enqueued" gate for those two
//! specific events, so every launcher and the chat turn-completion hook call
//! one small helper each instead of duplicating payload shape or the
//! config-gating check.
//!
//! **Enqueue-gated, not drain-gated**: both helpers check
//! `telemetry::slack_event_settings()` and `config::slack_webhook_url()`
//! *before* calling `Store::enqueue_notification`, rather than always
//! enqueueing and letting an idle drain loop (or one that never starts,
//! per `commands::up::run`'s webhook-gated `spawn_notifier_loop` call) sit on
//! the rows forever. Two reasons: a "job submitted" or "run synthesized"
//! notification is time-sensitive — sending it stale, days later, the moment
//! someone finally configures a webhook would be confusing rather than
//! useful — and gating here also keeps `notifications_outbox` from
//! accumulating dead rows for every run on every machine that has never
//! turned Slack on at all.

use std::sync::OnceLock;

use serde_json::{json, Value};

use crate::error::Result;
use crate::jobs::BackendDescriptor;
use crate::local::model::{LocalExperiment, LocalProject};
use crate::store::{RunWakeup, Store, StoredRun};
use crate::workspace_state::{ExperimentView, Pane};

static UP_PORT: OnceLock<u16> = OnceLock::new();

/// Recorded once by `orx up` right after it binds its port
/// (`commands::up::run`), so a deep link can be built from any launcher or
/// chat-turn call site without threading the port through each one. Unset
/// when a run is launched by a bare CLI invocation with no `orx up` running
/// (e.g. `orx exp run` outside the dashboard) — the notification still goes
/// out, just without a deep link.
pub fn set_up_port(port: u16) {
    let _ = UP_PORT.set(port);
}

/// Mirrors `ui/src/workspaceState.ts`'s `taskLocation()` exactly (path shape
/// and `encodeURIComponent`-equivalent escaping via `urlencoding`), pointed
/// at the run's experiment. `chat_session_id` is the run's own launching
/// session when known, else the wake-up's — falling back to `"new"` matches
/// what the dashboard itself does for a run with no session in hand.
fn deep_link(
    project_id: &str,
    experiment_id: &str,
    run_id: &str,
    chat_session_id: Option<&str>,
) -> Option<String> {
    let port = UP_PORT.get()?;
    let session = chat_session_id.unwrap_or("new");
    let pane = Pane::Experiment {
        experiment_id: experiment_id.to_string(),
        view: ExperimentView::Overview,
        run_id: Some(run_id.to_string()),
    };
    let pane_json = serde_json::to_string(&pane).ok()?;
    Some(format!(
        "http://127.0.0.1:{port}/projects/{}/tasks/{}?pane={}",
        urlencoding::encode(project_id),
        urlencoding::encode(session),
        urlencoding::encode(&pane_json),
    ))
}

/// Same run numbering the dashboard shows (`DetailDrawer.tsx`'s `runNumber`):
/// `list_runs_by_experiment` returns newest-first, so the oldest run is #1.
fn run_ordinal(store: &Store, experiment_id: &str, run_id: &str) -> Result<usize> {
    let runs = store.list_runs_by_experiment(experiment_id)?;
    let len = runs.len();
    Ok(match runs.iter().position(|r| r.id == run_id) {
        Some(idx) => len - idx,
        None => len,
    })
}

fn header_line(
    project_name: &str,
    experiment_title: &str,
    ordinal: usize,
    job_label: Option<&str>,
) -> String {
    match job_label {
        Some(label) => {
            format!("[{project_name}] {experiment_title} \u{b7} Run {ordinal} \u{b7} {label}")
        }
        None => format!("[{project_name}] {experiment_title} \u{b7} Run {ordinal}"),
    }
}

/// Slack's `mrkdwn` section text tops out well under this; a wake-up's
/// closing reply is already capped at `SPAWN_REPORT_LIMIT` (4000 chars) by
/// `chat::spawn_outcome`, which alone can still overflow Slack's ~3000-char
/// block limit, so this is a second, defensive cap on the rendered block
/// only — the fuller `text` fallback field has a much higher limit and is
/// left untruncated.
fn slack_block_text(text: &str) -> String {
    const LIMIT: usize = 2900;
    if text.chars().count() <= LIMIT {
        text.to_string()
    } else {
        text.chars().take(LIMIT).collect::<String>() + "\u{2026} (truncated)"
    }
}

fn payload(header: &str, details: &str) -> Value {
    json!({
        "text": format!("{header}\n\n{details}"),
        "blocks": [
            {
                "type": "header",
                "text": { "type": "plain_text", "text": header, "emoji": true },
            },
            {
                "type": "section",
                "text": { "type": "mrkdwn", "text": slack_block_text(details) },
            },
        ],
    })
}

fn slack_ready(event_enabled: bool) -> bool {
    event_enabled && crate::config::slack_webhook_url().is_some()
}

/// Enqueue the "a run was submitted" notification, right after the launcher
/// has upserted the run row with its final backend descriptor. A no-op
/// (`Ok(())`, nothing enqueued) unless `slack_events.job_submitted` is on
/// and a webhook is saved — see the module doc for why that's checked here
/// rather than left to the drain loop.
pub fn enqueue_job_submitted(
    store: &Store,
    project: &LocalProject,
    experiment: &LocalExperiment,
    run: &StoredRun,
    descriptor: &BackendDescriptor,
) -> Result<()> {
    if !slack_ready(crate::telemetry::slack_event_settings().job_submitted) {
        return Ok(());
    }
    let ordinal = run_ordinal(store, &experiment.id, &run.id)?;
    let header = header_line(
        &project.name,
        experiment.display_name(),
        ordinal,
        descriptor.job_label().as_deref(),
    );
    let mut details = vec![format!("Command: `{}`", run.command)];
    if let Some(link) = deep_link(
        &project.id,
        &experiment.id,
        &run.id,
        run.chat_session_id.as_deref(),
    ) {
        details.push(format!("<{link}|Open in orx>"));
    }
    store.enqueue_notification(
        "job_submitted",
        &payload(&header, &details.join("\n")).to_string(),
    )?;
    Ok(())
}

/// Enqueue the "a run's agent-driven outcome was synthesized" notification —
/// called only from the wake-up turn's own completion hook
/// (`local::chat::mod`), never for an ordinary chat turn or a failed one. A
/// no-op unless `slack_events.run_synthesized` is on and a webhook is saved.
pub fn enqueue_run_synthesized(
    store: &Store,
    wakeup: &RunWakeup,
    project: &LocalProject,
    experiment: &LocalExperiment,
    outcome_text: Option<&str>,
    description_changed: bool,
) -> Result<()> {
    if !slack_ready(crate::telemetry::slack_event_settings().run_synthesized) {
        return Ok(());
    }
    let run = &wakeup.run;
    let ordinal = run_ordinal(store, &experiment.id, &run.id)?;
    let job_label = BackendDescriptor::parse(&run.backend_json)
        .ok()
        .and_then(|d| d.job_label());
    let header = header_line(
        &project.name,
        experiment.display_name(),
        ordinal,
        job_label.as_deref(),
    );
    let mut details = vec![format!("Status: *{}*", run.status)];
    if description_changed {
        details.push("The experiment's description changed during this turn.".to_string());
    }
    if let Some(text) = outcome_text {
        details.push(text.to_string());
    }
    if let Some(link) = deep_link(
        &project.id,
        &experiment.id,
        &run.id,
        Some(&wakeup.chat_session_id),
    ) {
        details.push(format!("<{link}|Open in orx>"));
    }
    store.enqueue_notification(
        "run_synthesized",
        &payload(&header, &details.join("\n\n")).to_string(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points `config_dir()` at a fresh temp directory for the duration of
    /// `f`, serialized through `telemetry::XDG_CONFIG_HOME_TEST_LOCK` — see
    /// that static's doc comment: both `slack_webhook_url()` and
    /// `slack_event_settings()` bottom out in `config_dir()`, so any test
    /// that touches them must hold this lock, not a module-local one.
    fn with_isolated_config_dir<T>(f: impl FnOnce() -> T) -> T {
        let _lock = crate::telemetry::XDG_CONFIG_HOME_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let saved = std::env::var("XDG_CONFIG_HOME").ok();
        let dir =
            std::env::temp_dir().join(format!("orx-notify-events-config-{}", uuid::Uuid::new_v4()));
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let result = f();
        match saved {
            Some(val) => std::env::set_var("XDG_CONFIG_HOME", val),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn store_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("orx-notify-events-{name}-{}", uuid::Uuid::new_v4()))
    }

    fn project() -> LocalProject {
        LocalProject {
            id: "proj_1".into(),
            name: "My Project".into(),
            slug: "my-project".into(),
            github_owner: String::new(),
            github_repo: String::new(),
            github_sync_enabled: true,
            baseline_branch: "main".into(),
            repo_path: "/tmp/proj".into(),
            run_command: None,
            paper_id: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn experiment() -> LocalExperiment {
        LocalExperiment {
            id: "exp_1".into(),
            project_id: "proj_1".into(),
            parent_experiment_id: None,
            slug: "baseline".into(),
            branch_name: "orx/baseline".into(),
            title: Some("Baseline".into()),
            description: None,
            run_command: "echo hi".into(),
            agent_status: "idle".into(),
            created_at: 1,
            updated_at: 1,
            chat_session_id: None,
        }
    }

    fn descriptor() -> BackendDescriptor {
        BackendDescriptor {
            kind: "sge_job".into(),
            namespace: Some("login1".into()),
            job_id: Some("12345".into()),
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
            source_digest: None,
            source_path: None,
            source_size: None,
            run_dir: None,
        }
    }

    fn run() -> StoredRun {
        StoredRun {
            id: "run_1".into(),
            experiment_id: "exp_1".into(),
            project_id: "proj_1".into(),
            status: "starting".into(),
            backend_json: descriptor().to_json(),
            command: "python train.py".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            chat_session_id: None,
        }
    }

    #[test]
    fn job_submitted_is_a_no_op_without_a_webhook() {
        with_isolated_config_dir(|| {
            let dir = store_dir("job-submitted-unconfigured");
            let store = Store::open_at(dir.clone()).unwrap();
            let r = run();
            store.upsert_run(&r).unwrap();
            enqueue_job_submitted(&store, &project(), &experiment(), &r, &descriptor()).unwrap();
            assert_eq!(store.list_pending_notifications(10).unwrap().len(), 0);
            drop(store);
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn job_submitted_enqueues_once_a_webhook_is_configured() {
        with_isolated_config_dir(|| {
            crate::config::set_slack_webhook_url("https://hooks.slack.com/services/T0/B0/xyz")
                .unwrap();
            let dir = store_dir("job-submitted-configured");
            let store = Store::open_at(dir.clone()).unwrap();
            let r = run();
            store.upsert_run(&r).unwrap();
            enqueue_job_submitted(&store, &project(), &experiment(), &r, &descriptor()).unwrap();
            let pending = store.list_pending_notifications(10).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].kind, "job_submitted");
            let payload: Value = serde_json::from_str(&pending[0].payload_json).unwrap();
            assert!(payload["text"].as_str().unwrap().contains("My Project"));
            assert!(payload["text"].as_str().unwrap().contains("Run 1"));
            drop(store);
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn run_synthesized_is_a_no_op_without_a_webhook() {
        with_isolated_config_dir(|| {
            let dir = store_dir("run-synthesized-unconfigured");
            let store = Store::open_at(dir.clone()).unwrap();
            let r = run();
            store.upsert_run(&r).unwrap();
            let wakeup = RunWakeup {
                run: r,
                chat_session_id: "chat_A".into(),
                state: "delivered".into(),
            };
            enqueue_run_synthesized(
                &store,
                &wakeup,
                &project(),
                &experiment(),
                Some("All done."),
                false,
            )
            .unwrap();
            assert_eq!(store.list_pending_notifications(10).unwrap().len(), 0);
            drop(store);
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn run_synthesized_enqueues_once_a_webhook_is_configured() {
        with_isolated_config_dir(|| {
            crate::config::set_slack_webhook_url("https://hooks.slack.com/services/T0/B0/xyz")
                .unwrap();
            let dir = store_dir("run-synthesized-configured");
            let store = Store::open_at(dir.clone()).unwrap();
            let r = run();
            store.upsert_run(&r).unwrap();
            let wakeup = RunWakeup {
                run: r,
                chat_session_id: "chat_A".into(),
                state: "delivered".into(),
            };
            enqueue_run_synthesized(
                &store,
                &wakeup,
                &project(),
                &experiment(),
                Some("All done."),
                true,
            )
            .unwrap();
            let pending = store.list_pending_notifications(10).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].kind, "run_synthesized");
            let payload: Value = serde_json::from_str(&pending[0].payload_json).unwrap();
            let text = payload["text"].as_str().unwrap();
            assert!(text.contains("All done."));
            assert!(text.contains("description changed"));
            drop(store);
            let _ = std::fs::remove_dir_all(dir);
        });
    }

    #[test]
    fn run_ordinal_numbers_the_oldest_run_first() {
        let dir = store_dir("ordinal");
        let store = Store::open_at(dir.clone()).unwrap();
        let mut r1 = run();
        r1.id = "run_1".into();
        r1.created_at = 1;
        store.upsert_run(&r1).unwrap();
        let mut r2 = run();
        r2.id = "run_2".into();
        r2.created_at = 2;
        store.upsert_run(&r2).unwrap();

        assert_eq!(run_ordinal(&store, "exp_1", "run_1").unwrap(), 1);
        assert_eq!(run_ordinal(&store, "exp_1", "run_2").unwrap(), 2);

        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
