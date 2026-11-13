//! Local CLI operations shared by project, experiment, run, and log commands.

use crate::error::{anyhow, Result};
use crate::store::{Store, StoredRun};

pub(crate) static LABELS: std::sync::LazyLock<std::collections::HashMap<String, String>> =
    std::sync::LazyLock::new(|| {
        serde_json::from_str(include_str!("../ui/messages/en.json"))
            .expect("English message catalog")
    });

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub experiment_id: String,
    pub status: String,
    pub contact: crate::store::Contact,
    #[serde(default)]
    pub cancelling: bool,
    #[serde(default)]
    pub cancel_unconfirmed: Option<Vec<u32>>,
    #[serde(default)]
    pub waiting_reason: Option<String>,
    pub host: Option<String>,
    pub commit_sha: Option<String>,
    pub duration_secs: i64,
    pub updated_display: String,
    pub result_markdown: Option<String>,
    pub cancel_reason: Option<String>,
}

impl From<&StoredRun> for Run {
    fn from(run: &StoredRun) -> Self {
        Run {
            id: run.id.clone(),
            experiment_id: run.experiment_id.clone(),
            status: run.status.clone(),
            contact: crate::store::Contact::Live,
            cancelling: run.cancel_requested
                && !crate::engine::Phase::parse(&run.status)
                    .is_some_and(crate::engine::Phase::is_terminal),
            cancel_unconfirmed: None,
            waiting_reason: None,
            host: None,
            commit_sha: run.commit_sha.clone(),
            duration_secs: crate::local::run_duration_secs(run),
            updated_display: crate::local::fmt_ago(run.updated_at),
            result_markdown: run.result_markdown.clone(),
            cancel_reason: run.cancel_reason.clone(),
        }
    }
}

impl Run {
    pub(crate) fn from_store(store: &Store, stored: &StoredRun) -> Result<Self> {
        let mut run = Self::from(stored);
        if let Some(state) = store.engine_run_state(&stored.id)? {
            run.cancelling |= state.cancel_accepted_at.is_some()
                && !crate::engine::Phase::parse(&stored.status)
                    .is_some_and(crate::engine::Phase::is_terminal);
            run.contact = state.contact;
            run.cancel_unconfirmed = state.cancel_unconfirmed;
            run.waiting_reason = state.waiting_reason;
        }
        run.host = crate::commands::up::run_host(store, stored)?;
        Ok(run)
    }

    pub fn contact_detail(&self) -> Option<String> {
        use crate::store::Contact;
        let (contact, since, cause) = match &self.contact {
            Contact::Live | Contact::Stopping { .. } => return None,
            Contact::Unreachable { since, cause } => ("unreachable", since, cause),
            Contact::NeedsAuth { since, cause } => ("needs_auth", since, cause),
            Contact::Unreconciled { since, detail } => ("unreconciled", since, detail),
        };
        Some(format!(
            "contact: {contact}, since {}: {cause}",
            crate::telemetry::iso8601_utc(*since)
        ))
    }

    pub fn waiting_detail(&self) -> Option<String> {
        let reason = self.waiting_reason.as_deref()?;
        Some(format!("waiting: {reason}"))
    }

    pub fn display_status(&self) -> std::borrow::Cow<'_, str> {
        if self.status == "cancelled" {
            if let crate::store::Contact::Stopping { count, .. } = self.contact {
                let host = self.host.as_deref().unwrap_or("localhost");
                let key = if count == 1 {
                    "status_stopping_one"
                } else {
                    "status_stopping_other"
                };
                return LABELS[key]
                    .replace("{count}", &count.to_string())
                    .replace("{host}", host)
                    .into();
            }
            if self.cancel_unconfirmed.is_some() {
                return LABELS["status_cancelled_unconfirmed"].as_str().into();
            }
            return LABELS["status_cancelled"].as_str().into();
        }
        if self.cancelling {
            let pending = self.status == "submitting"
                || matches!(
                    self.contact,
                    crate::store::Contact::Unreachable { .. }
                        | crate::store::Contact::NeedsAuth { .. }
                );
            let key = if pending {
                "status_cancellation_pending"
            } else {
                "status_cancelling"
            };
            return LABELS[key].as_str().into();
        }
        self.status.as_str().into()
    }

    pub fn failure_detail(&self) -> Option<String> {
        if self.status == "cancelled" {
            let reason = self
                .cancel_reason
                .iter()
                .chain(self.result_markdown.iter())
                .filter(|text| !text.trim().is_empty())
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            return (!reason.is_empty()).then(|| format!("reason: {reason}"));
        }
        if self.status != "failed" {
            return None;
        }
        match self.result_markdown.as_deref().map(str::trim) {
            Some(reason) if !reason.is_empty() => Some(format!("reason: {reason}")),
            _ => Some(format!(
                "reason: — (no message recorded — see `orx logs {}`)",
                self.id
            )),
        }
    }
}

pub enum DescInput {
    Set(String),
    Get,
}

impl DescInput {
    pub async fn resolve(set: Option<String>, stdin: bool) -> Result<Self> {
        use tokio::io::AsyncReadExt as _;
        match (set, stdin) {
            (Some(_), true) => Err(anyhow!("Pass either --set or --stdin, not both.")),
            (Some(text), false) => Ok(DescInput::Set(text)),
            (None, true) => {
                let mut buffer = String::new();
                tokio::io::stdin().read_to_string(&mut buffer).await?;
                Ok(DescInput::Set(buffer))
            }
            (None, false) => Ok(DescInput::Get),
        }
    }
}

pub struct ProjectEdit {
    pub name: Option<String>,
    pub run_command: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct RunListing {
    pub runs: Vec<Run>,
    pub titles: std::collections::HashMap<String, String>,
}

pub struct CreateExperimentSpec {
    pub title: String,
    pub parent: Option<String>,
    pub baseline: bool,
    pub description: Option<String>,
    pub run_command: Option<String>,
}

pub(crate) fn resolve_project(store: Store, project_id: &str) -> Result<LocalPlane> {
    let project = crate::local::resolve::resolve_project(&store, project_id)?;
    Ok(LocalPlane {
        store,
        project: Some(project),
        experiment: None,
        id: project_id.to_string(),
    })
}

pub(crate) fn resolve_experiment(store: Store, exp_id: &str) -> Result<LocalPlane> {
    let experiment = crate::local::resolve::resolve_experiment(&store, exp_id)?;
    Ok(LocalPlane {
        store,
        project: None,
        experiment: Some(experiment),
        id: exp_id.to_string(),
    })
}

pub(crate) fn resolve_run(store: Store, run_id: &str) -> Result<LocalPlane> {
    crate::local::resolve::resolve_run(&store, run_id)?;
    Ok(LocalPlane {
        store,
        project: None,
        experiment: None,
        id: run_id.to_string(),
    })
}

mod checkout_hint;
pub(crate) mod local_plane;

pub(crate) use local_plane::LocalPlane;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{now_ms, StoredRun};

    fn stored_run(status: &str, result_markdown: Option<&str>) -> StoredRun {
        let now = now_ms();
        StoredRun {
            id: "r1".to_string(),
            experiment_id: "e1".to_string(),
            project_id: "p1".to_string(),
            status: status.to_string(),
            backend_json: "{}".to_string(),
            command: "echo hi".to_string(),
            created_at: now,
            updated_at: now,
            ended_at: Some(now),
            exit_code: None,
            commit_sha: Some("abcdef1234567890".to_string()),
            result_markdown: result_markdown.map(str::to_string),
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }
    }

    #[test]
    fn contact_details_decode_the_v2_shape() {
        for (contact, reason) in [
            (
                serde_json::json!({"type":"unreachable", "since":1000, "cause":"offline"}),
                "offline",
            ),
            (
                serde_json::json!({"type":"needs_auth", "since":1000, "cause":"key rejected"}),
                "key rejected",
            ),
            (
                serde_json::json!({"type":"unreconciled", "since":1000, "detail":"receipt unreadable"}),
                "receipt unreadable",
            ),
            (serde_json::json!({"type":"live"}), ""),
        ] {
            let mut view = serde_json::to_value(stored_run("running", None)).unwrap();
            view["durationSecs"] = 1.into();
            view["updatedDisplay"] = "now".into();
            view["contact"] = contact.clone();
            let run: Run = serde_json::from_value(view).unwrap();
            if reason.is_empty() {
                assert_eq!(run.contact_detail(), None);
            } else {
                let detail = run.contact_detail().unwrap();
                assert!(detail.contains(contact["type"].as_str().unwrap()));
                assert!(detail.contains("1970-01-01T00:00:01"));
                assert!(detail.contains(reason));
            }
        }
    }

    #[test]
    fn stored_run_maps_to_cli_run() {
        let run = Run::from(&stored_run("done", None));
        assert_eq!(run.id, "r1");
        assert_eq!(run.experiment_id, "e1");
        assert_eq!(run.status, "done");
        assert_eq!(run.commit_sha.as_deref(), Some("abcdef1234567890"));
        assert!(run.updated_display.ends_with("ago"));
    }

    #[test]
    fn failure_detail_points_to_local_logs() {
        let run = Run::from(&stored_run("failed", None));
        assert_eq!(
            run.failure_detail().as_deref(),
            Some("reason: — (no message recorded — see `orx logs r1`)")
        );
    }

    #[test]
    fn cancelled_run_reports_who_requested_the_cancel() {
        let mut run = Run::from(&stored_run("cancelled", None));
        assert_eq!(run.failure_detail(), None);
        run.cancel_reason = Some("Cancel requested with `orx exp cancel`.".into());
        assert_eq!(
            run.failure_detail().as_deref(),
            Some("reason: Cancel requested with `orx exp cancel`.")
        );
    }

    #[test]
    fn exp_run_plane_tags_demo_launches_by_owning_project() {
        use crate::local::model::{LocalExperiment, LocalProject};
        let dir = std::env::temp_dir().join(format!("orx-plane-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for project_id in [crate::local::demo::PROJECT_ID, "p1"] {
            store
                .create_local_project(&LocalProject {
                    id: project_id.into(),
                    name: project_id.into(),
                    slug: project_id.into(),
                    github_owner: "o".into(),
                    github_repo: "r".into(),
                    github_sync_enabled: false,
                    baseline_branch: "main".into(),
                    repo_path: "/tmp/repo".into(),
                    run_command: None,
                    paper_id: None,
                    created_at: 0,
                    updated_at: 0,
                })
                .unwrap();
        }
        for (id, project_id) in [
            ("demo_nanochat_lr_probe_v1", crate::local::demo::PROJECT_ID),
            ("agent_made", crate::local::demo::PROJECT_ID),
            ("user_exp", "p1"),
        ] {
            store
                .create_local_experiment(&LocalExperiment {
                    id: id.into(),
                    project_id: project_id.into(),
                    parent_experiment_id: None,
                    slug: id.into(),
                    branch_name: format!("orx/{id}"),
                    title: None,
                    description: None,
                    run_command: "echo hi".into(),
                    agent_status: "idle".into(),
                    created_at: 0,
                    updated_at: 0,
                    chat_session_id: None,
                    archived: false,
                })
                .unwrap();
        }
        let label = |id: &str| {
            resolve_experiment(Store::open_at(dir.clone()).unwrap(), id)
                .unwrap()
                .demo_run_label()
        };
        assert_eq!(label("demo_nanochat_lr_probe_v1"), Some("lr_probe"));
        assert_eq!(label("agent_made"), Some("other"));
        assert_eq!(label("user_exp"), None);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    #[test]
    fn cli_labels_decode_the_v2_cancel_fields_without_replacing_the_phase() {
        let mut value = serde_json::json!({
            "id": "run", "experimentId": "exp", "status": "running", "commitSha": null,
            "durationSecs": 1, "updatedDisplay": "now", "resultMarkdown": null, "cancelReason": null,
            "contact": {"type":"live"}, "cancelling": true, "host": "gpu-box",
        });
        let run: Run = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(run.status, "running");
        assert_eq!(run.display_status(), "Cancelling…");
        for contact in [
            serde_json::json!({"type":"unreachable", "since":1, "cause":"offline"}),
            serde_json::json!({"type":"needs_auth", "since":1, "cause":"connect"}),
        ] {
            value["contact"] = contact;
            let run: Run = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(run.display_status(), "Cancellation pending");
        }
        value["status"] = serde_json::json!("cancelled");
        value["cancelling"] = serde_json::json!(false);
        value["contact"] = serde_json::json!({"type":"stopping", "since":1, "count":2});
        let run: Run = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(run.status, "cancelled");
        assert_eq!(
            run.display_status(),
            "Not fully stopped · 2 processes still running on gpu-box"
        );
        value["contact"] = serde_json::json!({"type":"stopping", "since":1, "count":1});
        let run: Run = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(
            run.display_status(),
            "Not fully stopped · 1 process still running on gpu-box"
        );
        assert_eq!(run.contact_detail(), None);
        value["contact"] = serde_json::json!({"type":"live"});
        assert_eq!(
            serde_json::from_value::<Run>(value.clone())
                .unwrap()
                .display_status(),
            "Cancelled"
        );
        value["cancelUnconfirmed"] = serde_json::json!([789]);
        value["resultMarkdown"] = "The host couldn't inspect process 789.".into();
        let run: Run = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(run.display_status(), "Cancelled · couldn't confirm");
        assert!(run.failure_detail().unwrap().contains("process 789"));
        value["resultMarkdown"] = "Edited".into();
        assert_eq!(
            serde_json::from_value::<Run>(value)
                .unwrap()
                .display_status(),
            "Cancelled · couldn't confirm"
        );
    }
}
