//! Local CLI operations shared by project, experiment, run, and log commands.

use crate::error::{anyhow, Result};
use crate::store::{Store, StoredRun};

pub struct Run {
    pub id: String,
    pub experiment_id: String,
    pub status: String,
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
            commit_sha: run.commit_sha.clone(),
            duration_secs: crate::local::run_duration_secs(run),
            updated_display: crate::local::fmt_ago(run.updated_at),
            result_markdown: run.result_markdown.clone(),
            cancel_reason: run.cancel_reason.clone(),
        }
    }
}

impl Run {
    pub fn failure_detail(&self) -> Option<String> {
        if self.status == "cancelled" {
            return self.cancel_reason.as_ref().map(|r| format!("reason: {r}"));
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

mod local_plane;

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
