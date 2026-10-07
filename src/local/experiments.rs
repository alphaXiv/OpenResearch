//! Local experiment creation — slug, branch, row. Used by
//! `orx create-experiment` through the local execution plane.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::error::{anyhow, Result};
use crate::store::{now_ms, Store};

use super::git;
use super::model::{LocalExperiment, LocalProject};
use super::slugify;

/// First free slug within the project: `base`, `base-2`, `base-3`, …
fn unique_slug(store: &Store, project: &LocalProject, base: &str) -> Result<String> {
    let mut taken: HashSet<String> = store
        .list_experiments_by_project(&project.id)?
        .into_iter()
        .map(|e| e.slug)
        .collect();
    for branch in git::local_branches(Path::new(&project.repo_path))? {
        if let Some(slug) = branch.strip_prefix("orx/") {
            taken.insert(slug.to_string());
        }
    }
    if !taken.contains(base) {
        return Ok(base.to_string());
    }
    let mut n = 2;
    loop {
        let candidate = format!("{base}-{n}");
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
        n += 1;
    }
}

/// The project's active root experiment (parent NULL) — oldest first when several
/// exist. `None` on a fresh project: the tree starts empty and the first
/// no-parent create becomes the baseline. CLI creates without a parent attach
/// here once a root exists (pass `--baseline` to add another root).
pub fn project_root(store: &Store, project_id: &str) -> Result<Option<LocalExperiment>> {
    // list is ordered created_at ASC, so `find` picks the oldest root.
    Ok(store
        .list_experiments_by_project(project_id)?
        .into_iter()
        .find(|e| e.parent_experiment_id.is_none() && !e.archived))
}

/// Warning for pre-orx/<slug>-baseline rows: roots created before baselines
/// got their own branch ride the project's base branch, so that branch is an
/// immutable experiment node. `None` for experiments created since.
pub fn legacy_root_warning(project: &LocalProject, experiment: &LocalExperiment) -> Option<String> {
    (experiment.parent_experiment_id.is_none() && experiment.branch_name == project.baseline_branch)
        .then(|| {
            format!(
                "warning: root experiment {} rides the project's base branch '{}' \
                 (created before baselines got their own orx/* branch). Treat '{}' \
                 as immutable — publish READMEs/notebooks elsewhere, or recreate the \
                 tree with `orx create-experiment --baseline`.",
                experiment.id, project.baseline_branch, project.baseline_branch
            )
        })
}

#[derive(Clone, Copy)]
pub enum ArchiveDirection {
    Ancestors,
    Descendants,
    Only,
    Region,
    TaskRegion,
}

pub fn set_archived(
    store: &mut Store,
    id: &str,
    direction: ArchiveDirection,
    archived: bool,
) -> Result<Vec<String>> {
    let selected = store
        .get_local_experiment(id)?
        .ok_or_else(|| anyhow!("Experiment {id} not found."))?;
    let experiments = store.list_experiments_by_project(&selected.project_id)?;
    let ids = archive_ids(&experiments, &selected, direction);
    store.set_experiments_archived(&ids, archived)
}

fn archive_ids(
    experiments: &[LocalExperiment],
    selected: &LocalExperiment,
    direction: ArchiveDirection,
) -> Vec<String> {
    match direction {
        ArchiveDirection::Only => vec![selected.id.clone()],
        ArchiveDirection::Region | ArchiveDirection::TaskRegion => {
            let in_scope = |experiment: &LocalExperiment| {
                !matches!(direction, ArchiveDirection::TaskRegion)
                    || selected.chat_session_id.is_some()
                        && experiment.chat_session_id == selected.chat_session_id
            };
            if !selected.archived || !in_scope(selected) {
                return Vec::new();
            }
            let by_id: HashMap<_, _> = experiments.iter().map(|e| (e.id.as_str(), e)).collect();
            let mut root = selected;
            while let Some(parent) = root
                .parent_experiment_id
                .as_deref()
                .and_then(|id| by_id.get(id))
            {
                if !parent.archived || !in_scope(parent) {
                    break;
                }
                root = parent;
            }
            let mut children: HashMap<&str, Vec<&LocalExperiment>> = HashMap::new();
            for experiment in experiments {
                if let Some(parent) = experiment.parent_experiment_id.as_deref() {
                    children.entry(parent).or_default().push(experiment);
                }
            }
            let mut ids = Vec::new();
            let mut stack = vec![root];
            while let Some(experiment) = stack.pop() {
                ids.push(experiment.id.clone());
                if let Some(next) = children.get(experiment.id.as_str()) {
                    stack.extend(
                        next.iter()
                            .copied()
                            .filter(|child| child.archived && in_scope(child)),
                    );
                }
            }
            ids
        }
        ArchiveDirection::Ancestors => {
            let by_id: HashMap<_, _> = experiments.iter().map(|e| (e.id.as_str(), e)).collect();
            let mut ids = Vec::new();
            let mut parent = selected.parent_experiment_id.as_deref();
            while let Some(id) = parent {
                let Some(experiment) = by_id.get(id) else {
                    break;
                };
                ids.push(experiment.id.clone());
                parent = experiment.parent_experiment_id.as_deref();
            }
            ids
        }
        ArchiveDirection::Descendants => {
            let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
            for experiment in experiments {
                if let Some(parent) = experiment.parent_experiment_id.as_deref() {
                    children.entry(parent).or_default().push(&experiment.id);
                }
            }
            let mut ids = Vec::new();
            let mut stack = vec![selected.id.as_str()];
            while let Some(id) = stack.pop() {
                if let Some(next) = children.get(id) {
                    for child in next {
                        ids.push((*child).to_string());
                        stack.push(*child);
                    }
                }
            }
            ids
        }
    }
}

/// The command a launch would run: the experiment's own, else the project default.
pub fn effective_run_command<'a>(
    experiment: &'a LocalExperiment,
    project_command: Option<&'a str>,
) -> Option<&'a str> {
    Some(experiment.run_command.as_str())
        .filter(|c| !c.trim().is_empty())
        .or_else(|| project_command.filter(|c| !c.trim().is_empty()))
}

/// Create a local experiment. Every node gets its own `orx/<slug>` branch:
/// a child forks off its parent's tip, a baseline/root off
/// the project's base branch. The base branch itself is never an experiment
/// node — it stays mutable (README, notebooks, publication surface) while
/// `orx/*` branches hold the experiment nodes' recorded code.
pub fn create_experiment(
    store: &Store,
    project: &LocalProject,
    parent: Option<&LocalExperiment>,
    slug: Option<&str>,
    title: Option<String>,
    description: Option<String>,
    run_command: Option<String>,
) -> Result<LocalExperiment> {
    if let Some(p) = parent {
        if p.project_id != project.id {
            return Err(anyhow!(
                "Parent experiment {} belongs to a different project.",
                p.id
            ));
        }
    }
    let base = match slug {
        Some(s) => slugify(s),
        None => slugify(title.as_deref().unwrap_or("experiment")),
    };
    let slug = unique_slug(store, project, &base)?;

    let repo = Path::new(&project.repo_path);
    let fork_point = parent
        .map(|p| p.branch_name.as_str())
        .unwrap_or(&project.baseline_branch);
    let branch_name = format!("orx/{slug}");
    git::create_experiment_branch(repo, fork_point, &branch_name)?;

    // Inherit: explicit > parent's command > project default > "".
    let run_command = run_command
        .filter(|c| !c.trim().is_empty())
        .or_else(|| {
            parent
                .map(|p| p.run_command.clone())
                .filter(|c| !c.trim().is_empty())
        })
        .or_else(|| project.run_command.clone())
        .unwrap_or_default();

    let now = now_ms();
    let experiment = LocalExperiment {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: project.id.clone(),
        parent_experiment_id: parent.map(|p| p.id.clone()),
        slug,
        branch_name,
        title,
        description,
        run_command,
        agent_status: "idle".to_string(),
        created_at: now,
        updated_at: now,
        chat_session_id: crate::local::chat::launching_chat_session(),
        archived: false,
    };
    store.create_local_experiment(&experiment)?;
    Ok(experiment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(base: &str) -> LocalProject {
        LocalProject {
            id: "p1".into(),
            name: "Demo".into(),
            slug: "demo".into(),
            github_owner: "o".into(),
            github_repo: "r".into(),
            github_sync_enabled: true,
            baseline_branch: base.into(),
            repo_path: "/tmp/r".into(),
            run_command: None,
            paper_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn experiment(parent: Option<&str>, branch: &str) -> LocalExperiment {
        LocalExperiment {
            id: "e1".into(),
            project_id: "p1".into(),
            parent_experiment_id: parent.map(String::from),
            slug: "baseline".into(),
            branch_name: branch.into(),
            title: None,
            description: None,
            run_command: String::new(),
            agent_status: "idle".into(),
            created_at: 0,
            updated_at: 0,
            chat_session_id: None,
            archived: false,
        }
    }

    #[test]
    fn warns_only_for_legacy_roots_on_the_base_branch() {
        let p = project("main");
        // Legacy root riding main: warn.
        let w = legacy_root_warning(&p, &experiment(None, "main")).unwrap();
        assert!(w.contains("rides the project's base branch 'main'"));
        // Current-scheme root on its own branch: silent.
        assert!(legacy_root_warning(&p, &experiment(None, "orx/baseline")).is_none());
        // Child, even on the base branch name (not a root): silent.
        assert!(legacy_root_warning(&p, &experiment(Some("root"), "main")).is_none());
    }

    #[test]
    fn unset_experiment_command_follows_the_project_default() {
        let mut e = experiment(None, "orx/baseline");
        assert_eq!(effective_run_command(&e, None), None);
        assert_eq!(effective_run_command(&e, Some(" ")), None);
        let train = Some("python train.py");
        assert_eq!(effective_run_command(&e, train), train);
        e.run_command = "  ".into();
        assert_eq!(effective_run_command(&e, train), train);
        e.run_command = "python eval.py".into();
        assert_eq!(effective_run_command(&e, train), Some("python eval.py"));
    }

    #[test]
    fn project_is_available_as_an_experiment_slug() {
        let root =
            std::env::temp_dir().join(format!("orx-experiment-slug-test-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(root.clone()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        assert!(std::process::Command::new("git")
            .current_dir(&repo)
            .args(["init", "-b", "main"])
            .status()
            .unwrap()
            .success());
        let mut project = project("main");
        project.repo_path = repo.to_string_lossy().into_owned();
        assert_eq!(unique_slug(&store, &project, "project").unwrap(), "project");
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn archive_directions_respect_selection_and_siblings() {
        let mut root = experiment(None, "root");
        root.id = "root".into();
        let mut selected = experiment(Some("root"), "selected");
        selected.id = "selected".into();
        let mut child = experiment(Some("selected"), "child");
        child.id = "child".into();
        let mut sibling = experiment(Some("root"), "sibling");
        sibling.id = "sibling".into();
        let all = [root, selected.clone(), child, sibling];
        assert_eq!(
            archive_ids(&all, &selected, ArchiveDirection::Ancestors),
            ["root"]
        );
        assert_eq!(
            archive_ids(&all, &selected, ArchiveDirection::Only),
            ["selected"]
        );
        assert_eq!(
            archive_ids(&all, &selected, ArchiveDirection::Descendants),
            ["child"]
        );
    }

    #[test]
    fn archive_region_stops_at_active_nodes() {
        let mut root = experiment(None, "root");
        root.id = "root".into();
        root.archived = true;
        let mut selected = experiment(Some("root"), "selected");
        selected.id = "selected".into();
        selected.archived = true;
        let mut sibling = experiment(Some("root"), "sibling");
        sibling.id = "sibling".into();
        sibling.archived = true;
        let mut active = experiment(Some("selected"), "active");
        active.id = "active".into();
        let mut separate = experiment(Some("active"), "separate");
        separate.id = "separate".into();
        separate.archived = true;
        let ids = archive_ids(
            &[root, selected.clone(), sibling, active, separate],
            &selected,
            ArchiveDirection::Region,
        );
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"root".to_string()));
        assert!(ids.contains(&"selected".to_string()));
        assert!(ids.contains(&"sibling".to_string()));
    }

    #[test]
    fn task_region_does_not_restore_other_sessions() {
        let mut foreign_root = experiment(None, "foreign-root");
        foreign_root.archived = true;
        foreign_root.chat_session_id = Some("other".into());
        let mut selected = experiment(Some(&foreign_root.id), "selected");
        selected.id = "selected".into();
        selected.archived = true;
        selected.chat_session_id = Some("mine".into());
        let mut mine = experiment(Some(&selected.id), "mine");
        mine.id = "mine".into();
        mine.archived = true;
        mine.chat_session_id = Some("mine".into());
        let mut foreign_sibling = experiment(Some(&foreign_root.id), "foreign-sibling");
        foreign_sibling.id = "foreign-sibling".into();
        foreign_sibling.archived = true;
        foreign_sibling.chat_session_id = Some("other".into());
        let ids = archive_ids(
            &[foreign_root, selected.clone(), mine, foreign_sibling],
            &selected,
            ArchiveDirection::TaskRegion,
        );
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"selected".to_string()));
        assert!(ids.contains(&"mine".to_string()));
    }

    #[test]
    fn default_parent_skips_archived_roots() {
        let dir = std::env::temp_dir().join(format!("orx-archive-root-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let mut archived = experiment(None, "old");
        archived.archived = true;
        store.create_local_experiment(&archived).unwrap();
        assert!(project_root(&store, "p1").unwrap().is_none());
        let mut active = experiment(None, "new");
        active.id = "e2".into();
        active.slug = "new".into();
        store.create_local_experiment(&active).unwrap();
        assert_eq!(project_root(&store, "p1").unwrap().unwrap().id, "e2");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
