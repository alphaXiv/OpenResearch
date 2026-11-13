//! Local identifier resolution for CLI project, experiment, and run commands.

use crate::error::{anyhow, Result};
use crate::local::model::{LocalExperiment, LocalProject};
use crate::store::{Store, StoredRun};

pub fn resolve_project(store: &Store, project_id: &str) -> Result<LocalProject> {
    store.get_local_project(project_id)?.ok_or_else(|| {
        anyhow!(
            "Project {project_id} is not registered in the orx store at {}. Run `orx up` in its repository first.",
            store.data_dir().display()
        )
    })
}

pub fn resolve_experiment(store: &Store, exp_id: &str) -> Result<LocalExperiment> {
    store.get_local_experiment(exp_id)?.ok_or_else(|| {
        anyhow!(
            "Experiment {exp_id} was not found in the orx store at {}. Find local experiment ids with `orx project view <projectId>`.",
            store.data_dir().display()
        )
    })
}

pub fn resolve_run(store: &Store, run_id: &str) -> Result<StoredRun> {
    crate::local::local_run(store, run_id)?.ok_or_else(|| {
        anyhow!(
            "Run {run_id} was not found in the orx store at {}. Find local run ids with `orx runs <projectId>`.",
            store.data_dir().display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::model::{LocalExperiment, LocalProject};
    use crate::store::{now_ms, StoredRun};

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!("orx-resolve-{}", uuid::Uuid::new_v4()));
        Store::open_at(dir).expect("open temp store")
    }

    fn project(id: &str) -> LocalProject {
        let now = now_ms();
        LocalProject {
            id: id.to_string(),
            name: "P".to_string(),
            slug: format!("slug-{id}"),
            github_owner: "o".to_string(),
            github_repo: "r".to_string(),
            github_sync_enabled: true,
            baseline_branch: "main".to_string(),
            repo_path: "/tmp/repo".to_string(),
            run_command: None,
            paper_id: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn experiment(id: &str, project_id: &str) -> LocalExperiment {
        let now = now_ms();
        LocalExperiment {
            id: id.to_string(),
            project_id: project_id.to_string(),
            parent_experiment_id: None,
            slug: format!("exp-{id}"),
            branch_name: format!("orx/{id}"),
            title: None,
            description: None,
            run_command: "echo hi".to_string(),
            agent_status: "idle".to_string(),
            created_at: now,
            updated_at: now,
            chat_session_id: None,
            archived: false,
        }
    }

    fn run(id: &str, experiment_id: &str, project_id: &str) -> StoredRun {
        let now = now_ms();
        StoredRun {
            id: id.to_string(),
            experiment_id: experiment_id.to_string(),
            project_id: project_id.to_string(),
            status: "running".to_string(),
            backend_json: "{}".to_string(),
            command: "echo hi".to_string(),
            created_at: now,
            updated_at: now,
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
    fn resolves_local_records() {
        let store = temp_store();
        store.create_local_project(&project("p1")).unwrap();
        store
            .create_local_experiment(&experiment("e1", "p1"))
            .unwrap();
        store.upsert_run(&run("r1", "e1", "p1")).unwrap();

        assert_eq!(resolve_project(&store, "p1").unwrap().id, "p1");
        assert_eq!(resolve_experiment(&store, "e1").unwrap().id, "e1");
        assert_eq!(resolve_run(&store, "r1").unwrap().id, "r1");
    }

    #[test]
    fn unknown_ids_name_the_store_they_were_looked_up_in() {
        let store = temp_store();
        let dir = store.data_dir().display().to_string();
        for error in [
            resolve_project(&store, "nope").map(|_| ()),
            resolve_experiment(&store, "nope").map(|_| ()),
            resolve_run(&store, "nope").map(|_| ()),
        ] {
            let error = error.unwrap_err().to_string();
            assert!(error.contains("nope") && error.contains(&dir), "{error}");
        }
    }

    #[test]
    fn orphaned_run_is_not_a_local_run() {
        let store = temp_store();
        store
            .upsert_run(&run("r1", "missing-experiment", "old-project"))
            .unwrap();
        assert!(resolve_run(&store, "r1").is_err());
    }
}
