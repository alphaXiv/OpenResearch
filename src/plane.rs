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
        }
    }
}

impl Run {
    pub fn failure_detail(&self) -> Option<String> {
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

pub struct RunLog {
    pub content: Vec<u8>,
    pub start_byte: i64,
    pub end_byte: i64,
    pub total_bytes: i64,
    pub source: String,
    pub truncated_before: bool,
    pub truncated_after: bool,
    pub missing_local: bool,
}

impl RunLog {
    pub fn footer(&self) -> String {
        let mut more = Vec::new();
        if self.truncated_before {
            more.push("more above");
        }
        if self.truncated_after {
            more.push("more below");
        }
        let suffix = if more.is_empty() {
            String::new()
        } else {
            format!(" ({})", more.join(", "))
        };
        format!(
            "[{}] bytes {}–{} of {}{}",
            self.source, self.start_byte, self.end_byte, self.total_bytes, suffix
        )
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

pub struct LogRequest {
    pub mode: String,
    pub max_bytes: Option<i64>,
    pub start_byte: Option<i64>,
    pub end_byte: Option<i64>,
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
}
