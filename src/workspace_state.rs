use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceState {
    pub version: u32,
    pub last_task_id: Option<String>,
    pub last_location: Option<String>,
    pub tasks: BTreeMap<String, TaskWorkspace>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GlobalWorkspaceState {
    pub last_location: Option<String>,
    pub rail_open: bool,
    pub panel_width: f64,
    pub experiments_view: ExperimentsView,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskWorkspace {
    tabs: Vec<Pane>,
    active: Option<Pane>,
    preview_key: Option<String>,
    history: Vec<String>,
    expanded: BTreeMap<String, Vec<String>>,
    scroll: BTreeMap<String, ScrollPosition>,
    source_modes: BTreeMap<String, bool>,
    files_view: FilesView,
    scope: Scope,
    panel_max: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ScrollPosition {
    top: f64,
    left: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Pane {
    Home {
        view: HomeView,
    },
    Experiment {
        experiment_id: String,
        view: ExperimentView,
        #[serde(skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
    },
    File {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        source: Option<FileSource>,
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        branch_label: Option<String>,
    },
    Code {
        experiment_id: String,
        branch: String,
        view: FilesView,
    },
    Plan {
        session_id: String,
        prompt_id: String,
    },
    Subagent {
        session_id: String,
        spawn_part_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum HomeView {
    Experiments,
    Files,
    Artifacts,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ExperimentView {
    Overview,
    Terminal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FileSource {
    Repo,
    Artifacts,
    Abs,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum FilesView {
    Files,
    Changes,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Agent,
    Project,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ExperimentsView {
    Tree,
    Table,
}

impl Pane {
    fn valid(&self) -> bool {
        let nonempty = |value: &String| !value.is_empty();
        match self {
            Self::Home { .. } => true,
            Self::Experiment {
                experiment_id,
                run_id,
                ..
            } => nonempty(experiment_id) && run_id.as_ref().is_none_or(nonempty),
            Self::File {
                path,
                session_id,
                r#ref,
                line,
                branch_label,
                ..
            } => {
                nonempty(path)
                    && session_id.as_ref().is_none_or(nonempty)
                    && r#ref.as_ref().is_none_or(nonempty)
                    && branch_label.as_ref().is_none_or(|label| !label.is_empty())
                    && line.is_none_or(|line| line > 0 && line <= 9_007_199_254_740_991)
            }
            Self::Code {
                experiment_id,
                branch,
                ..
            } => nonempty(experiment_id) && nonempty(branch),
            Self::Plan {
                session_id,
                prompt_id,
            } => nonempty(session_id) && nonempty(prompt_id),
            Self::Subagent {
                session_id,
                spawn_part_id,
            } => nonempty(session_id) && nonempty(spawn_part_id),
        }
    }
}

impl WorkspaceState {
    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.last_task_id.as_ref().is_some_and(|id| id.is_empty())
            || !self.last_location.as_deref().is_none_or(valid_location)
            || self.tasks.iter().any(|(id, task)| {
                id.is_empty()
                    || !task.tabs.iter().all(Pane::valid)
                    || !task.active.as_ref().is_none_or(Pane::valid)
                    || task.scroll.values().any(|position| {
                        !position.top.is_finite()
                            || !position.left.is_finite()
                            || position.top < 0.0
                            || position.left < 0.0
                    })
            })
        {
            return Err(anyhow!("invalid workspace state"));
        }
        Ok(())
    }

    pub fn from_stored(json: &str) -> Option<Self> {
        let state: Self = serde_json::from_str(json).ok()?;
        state.validate().ok()?;
        Some(state)
    }
}

impl GlobalWorkspaceState {
    pub fn validate(&self) -> Result<()> {
        if !self.last_location.as_deref().is_none_or(valid_location)
            || !self.panel_width.is_finite()
            || self.panel_width <= 0.0
        {
            return Err(anyhow!("invalid global workspace state"));
        }
        Ok(())
    }

    pub fn from_stored(json: &str) -> Option<Self> {
        let state: Self = serde_json::from_str(json).ok()?;
        state.validate().ok()?;
        Some(state)
    }
}

fn valid_location(location: &str) -> bool {
    if !location.starts_with('/')
        || location.starts_with("//")
        || location.contains(['\\', '#'])
        || location.chars().any(char::is_control)
    {
        return false;
    }
    let (path, query) = location.split_once('?').unwrap_or((location, ""));
    let parts: Vec<_> = path.split('/').collect();
    let valid_id = |id: &str| {
        urlencoding::decode(id).is_ok_and(|id| {
            !id.is_empty()
                && id != "."
                && id != ".."
                && !id.contains(['/', '\\', '?', '#'])
                && !id.chars().any(char::is_control)
        })
    };
    let valid_path = match parts.as_slice() {
        ["", "projects"] => true,
        ["", "projects", project, "skills"] => valid_id(project),
        ["", "projects", project, "tasks", task] => valid_id(project) && valid_id(task),
        ["", "projects", project, "settings", tab] => {
            valid_id(project)
                && urlencoding::decode(tab).is_ok_and(|tab| {
                    matches!(
                        tab.as_ref(),
                        "settings"
                            | "harnesses"
                            | "projects"
                            | "compute"
                            | "instances"
                            | "environment"
                            | "git"
                            | "storage"
                    )
                })
        }
        _ => false,
    };
    if !valid_path || query.is_empty() {
        return valid_path;
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://localhost/?{query}")) else {
        return false;
    };
    let mut pairs = url.query_pairs();
    let Some((key, json)) = pairs.next() else {
        return true;
    };
    key == "pane"
        && pairs.next().is_none()
        && serde_json::from_str::<Pane>(&json)
            .ok()
            .is_some_and(|pane| pane.valid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn workspace_schema_and_resume_locations_are_validated() {
        let state = json!({"version":1,"lastLocation":"/projects/demo/tasks/new","tasks":{}});
        assert!(WorkspaceState::from_stored(&state.to_string()).is_some());
        for invalid in [
            json!({"version":2,"lastLocation":null,"tasks":{}}),
            json!({"version":1,"lastLocation":"https://example.com","tasks":{}}),
            json!({"version":1,"lastLocation":null,"tasks":{"new":{"tabs":[]}}}),
            json!({"version":1,"lastLocation":null,"tasks":{},"content":"not metadata"}),
        ] {
            assert!(WorkspaceState::from_stored(&invalid.to_string()).is_none());
        }
        for path in [
            "/",
            "/projects/demo",
            "//evil.test/projects",
            "/projects/../tasks/new",
            "/projects/demo/settings/unknown",
            "/projects/demo/tasks/new?pane=garbage",
            "/projects/demo/tasks/new#fragment",
        ] {
            assert!(!valid_location(path), "{path}");
        }
        let pane = json!({"kind":"file","path":"研究/figure +100%?draft#１.tex","source":"repo","line":9_007_199_254_740_991_u64});
        let location = format!(
            "/projects/demo/tasks/new?pane={}",
            urlencoding::encode(&pane.to_string())
        );
        assert!(valid_location(&location));
        assert!(valid_location("/projects/demo/settings/%67it"));
        assert!(valid_location(&format!(
            "/projects/demo/tasks/new?%70ane={}&",
            urlencoding::encode(&pane.to_string())
        )));
        assert!(valid_location("/projects/demo/tasks/new?pane=%7B%22kind%22%3A%22file%22%2C%22path%22%3A%22paper.tex%22%2C%22line%22%3Anull%7D"));
        let invalid_line =
            json!({"kind":"file","path":"paper.tex","line":9_007_199_254_740_992_u64});
        assert!(!serde_json::from_value::<Pane>(invalid_line)
            .unwrap()
            .valid());
        assert!(!Pane::File {
            path: "paper.tex".into(),
            source: None,
            session_id: None,
            r#ref: None,
            line: Some(0),
            branch_label: None,
        }
        .valid());
    }
}
