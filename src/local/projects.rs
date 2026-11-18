//! Local project creation — clone the repo and insert the project row. Used by
//! the `orx up` HTTP API (`POST /api/projects`).
//!
//! The project starts with an empty experiment tree. The first experiment
//! created without a parent via `orx create-experiment` becomes the baseline
//! root — the control every variant is measured against.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::store::{now_ms, Store};

use super::model::LocalProject;
use super::{git, slugify};

fn unique_project_slug(store: &Store, base: &str) -> Result<String> {
    let taken: HashSet<String> = store
        .list_local_projects()?
        .into_iter()
        .map(|p| p.slug)
        .collect();
    if base != super::demo::PROJECT_SLUG && !taken.contains(base) {
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

pub fn project_slug_preview(store: &Store, name: &str) -> Result<String> {
    unique_project_slug(store, &slugify(name))
}

pub(crate) fn expand_path(path: &str) -> Result<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(crate::error::anyhow!("project path is required"));
    }
    if trimmed == "~" || trimmed.starts_with("~/") {
        let home = crate::config::home_dir()
            .ok_or_else(|| crate::error::anyhow!("Could not resolve the home directory"))?;
        return Ok(if trimmed == "~" {
            home
        } else {
            home.join(&trimmed[2..])
        });
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(relative_base()?.join(path))
    }
}

/// Where a relative path is anchored: the working directory, except in the Linux
/// AppImage, whose working directory is its read-only mount (see linux/AppRun).
pub fn relative_base() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let in_image = crate::paths::in_appimage_mount(&cwd, std::env::var_os("APPDIR").as_deref());
    Ok(match crate::config::home_dir() {
        Some(home) if in_image => home,
        _ => cwd,
    })
}

const PAPER_PDF_NAME: &str = "paper.pdf";

fn prepare_path(
    path: &str,
    create_folder: bool,
    require_new_folder: bool,
    initialize_git: bool,
    clone_url: Option<&str>,
    shallow_clone: bool,
    paper_pdf: Option<&[u8]>,
) -> Result<PathBuf> {
    let path = expand_path(path)?;
    if let Some(url) = clone_url.map(str::trim).filter(|url| !url.is_empty()) {
        if path.exists() {
            let mut entries = std::fs::read_dir(&path)?;
            if entries.next().is_some() {
                return Err(crate::error::anyhow!(
                    "{} must be empty before cloning the paper repository",
                    path.display()
                ));
            }
        } else if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        git::clone_public(url, &path, shallow_clone)?;
        git::rename_origin_to_upstream(&path)?;
    } else if require_new_folder && path.exists() {
        return Err(crate::error::anyhow!(
            "{} already exists; choose a new folder for a blank project",
            path.display()
        ));
    } else if !path.exists() {
        if !create_folder {
            return Err(crate::error::anyhow!(
                "{} does not exist; choose an existing folder or allow OpenResearch to create it",
                path.display()
            ));
        }
        std::fs::create_dir_all(&path)?;
    } else if !path.is_dir() {
        return Err(crate::error::anyhow!("{} is not a folder", path.display()));
    }

    if paper_pdf.is_some() && std::fs::read_dir(&path)?.next().is_some() {
        return Err(crate::error::anyhow!(
            "{} must be empty to start a paper project",
            path.display()
        ));
    }

    // A blank or paper project owns the folder it was given — it was empty or
    // freshly created — so it gets a repository of its own even when it sits
    // inside an unrelated checkout, such as a dotfiles repository at `$HOME`.
    // Adopting an existing folder keeps looking outwards, where the enclosing
    // repository is the project the user means.
    let owns_folder = require_new_folder || paper_pdf.is_some();
    let repository_state = if owns_folder {
        git::own_repository_state(&path)
    } else {
        git::repository_state(&path)
    };
    if repository_state == git::RepositoryState::Invalid {
        return Err(crate::error::anyhow!(
            "{} is not a valid Git repository",
            path.display()
        ));
    }
    if matches!(
        repository_state,
        git::RepositoryState::NotRepository | git::RepositoryState::Unborn
    ) {
        if !initialize_git {
            return Err(crate::error::anyhow!(
                "Experiments need a local Git repository. Confirm initialization for {} and try again.",
                path.display()
            ));
        }
        // Written before initialization so the paper lands in the initial commit.
        if let Some(pdf) = paper_pdf {
            let pdf_path = path.join(PAPER_PDF_NAME);
            std::fs::write(&pdf_path, pdf).map_err(|e| {
                crate::error::anyhow!("Could not write {}: {}", pdf_path.display(), e)
            })?;
        }
        if owns_folder {
            git::initialize_own_repository(&path)?;
        } else {
            git::initialize_repository(&path)?;
        }
    }
    let root = git::repository_root(&path)?;
    git::validate_project_repository(&root)?;
    Ok(root)
}

/// Register a local folder as a project. No experiments are created —
/// the tree starts empty and the baseline is created lazily (first no-parent
/// `create_experiment`).
pub fn create_project(
    store: &Store,
    name: &str,
    path: &str,
    options: CreateProjectOptions,
) -> Result<LocalProject> {
    let CreateProjectOptions {
        create_folder,
        require_new_folder,
        initialize_git,
        clone_url,
        shallow_clone,
        run_command,
        paper_id,
        paper_pdf,
    } = options;
    let slug = unique_project_slug(store, &slugify(name))?;
    let repo_path = prepare_path(
        path,
        create_folder,
        require_new_folder,
        initialize_git,
        clone_url.as_deref(),
        shallow_clone,
        paper_pdf.as_deref(),
    )?;
    if store
        .list_local_projects()?
        .iter()
        .any(|project| Path::new(&project.repo_path) == repo_path)
    {
        return Err(crate::error::anyhow!(
            "{} is already registered as an OpenResearch project",
            repo_path.display()
        ));
    }
    let baseline_branch = git::require_current_branch(&repo_path)?;
    let publication = git::github_publication(&repo_path);
    let (github_owner, github_repo) = publication.unwrap_or_default();

    let now = now_ms();
    let project = LocalProject {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.to_string(),
        slug,
        github_owner,
        github_repo,
        github_sync_enabled: false,
        baseline_branch,
        repo_path: repo_path.to_string_lossy().to_string(),
        run_command: run_command.filter(|c| !c.trim().is_empty()),
        paper_id: paper_id.filter(|p| !p.trim().is_empty()),
        icon: None,
        created_at: now,
        updated_at: now,
    };
    store.create_local_project(&project)?;
    Ok(project)
}

pub fn scratch_root() -> PathBuf {
    crate::store::data_dir().join("scratch")
}

/// The project a new chat without a project starts in. A scratch project no
/// chat uses yet is reused, so opening the draft again leaves no new folder.
pub fn scratch_project(store: &Store) -> Result<LocalProject> {
    for project in store.list_local_projects()? {
        if project.is_scratch() && store.list_chat_sessions_by_project(&project.id)?.is_empty() {
            return Ok(project);
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let repo = scratch_root().join(&id);
    crate::paths::create_dir_all(&repo)?;
    git::initialize_own_repository(&repo)?;
    let now = now_ms();
    let project = LocalProject {
        slug: unique_project_slug(store, &format!("scratch-{}", &id[..8]))?,
        id,
        name: "No project".to_string(),
        github_owner: String::new(),
        github_repo: String::new(),
        github_sync_enabled: false,
        baseline_branch: git::require_current_branch(&repo)?,
        repo_path: repo.to_string_lossy().into_owned(),
        run_command: None,
        paper_id: None,
        icon: None,
        created_at: now,
        updated_at: now,
    };
    store.create_local_project(&project)?;
    Ok(project)
}

/// Turn a scratch project into a real one at `path`: bring the chat's commits
/// onto the baseline, move the repository there, and give it `name`. What the
/// chat left uncommitted stays in its worktree. The caller keeps every chat of
/// the project idle meanwhile.
pub fn promote_scratch_project(
    store: &Store,
    project: &LocalProject,
    worktree: Option<&Path>,
    name: &str,
    path: &str,
) -> Result<LocalProject> {
    if !project.is_scratch() {
        return Err(crate::error::anyhow!(
            "{} is already a project, at {}.",
            project.name,
            project.repo_path
        ));
    }
    let (name, target) = new_project_place(name, path)?;
    let name = name.as_str();

    let repo = PathBuf::from(&project.repo_path);
    if let Some(dir) = worktree.filter(|dir| dir.join(".git").exists()) {
        let head = git::git(Some(dir), &["rev-parse", "HEAD"])?;
        git::git(Some(&repo), &["merge", "--ff-only", "--quiet", &head]).map_err(|error| {
            crate::error::anyhow!(
                "Could not bring the chat's work onto {}: {error}",
                project.baseline_branch
            )
        })?;
    }

    move_repository(&repo, &target)?;
    let moved = (|| -> Result<LocalProject> {
        git::git(Some(&target), &["worktree", "repair"])?;
        let mut promoted = project.clone();
        promoted.name = name.to_string();
        promoted.repo_path = crate::paths::canonicalize(&target)?
            .to_string_lossy()
            .into_owned();
        store.update_local_project(&promoted)?;
        store.restamp_local_project_created(&promoted.id)?;
        Ok(promoted)
    })();
    let promoted = match moved {
        Ok(promoted) => promoted,
        Err(error) => {
            let restored = move_repository(&target, &repo)
                .and_then(|()| git::git(Some(&repo), &["worktree", "repair"]).map(drop));
            return Err(match restored {
                Ok(()) => crate::error::anyhow!(
                    "Could not move the project to {}: {error}",
                    target.display()
                ),
                Err(restore) => crate::error::anyhow!(
                    "Could not move the project to {}: {error}. Moving it back also failed: {restore}",
                    target.display()
                ),
            });
        }
    };

    // The slug, and the artifacts folder named by it, stay: the chat's earlier
    // messages link to files there.
    Ok(store.get_local_project(&project.id)?.unwrap_or(promoted))
}

/// The trimmed name and the folder for a project made from a chat, which must
/// be new or empty and outside the data directory.
pub fn new_project_place(name: &str, path: &str) -> Result<(String, PathBuf)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(crate::error::anyhow!("Give the project a name."));
    }
    let target = expand_path(path)?;
    if target.starts_with(crate::store::data_dir()) {
        return Err(crate::error::anyhow!(
            "{} is inside OpenResearch's data folder; choose a folder of your own.",
            target.display()
        ));
    }
    if target.exists() && (!target.is_dir() || std::fs::read_dir(&target)?.next().is_some()) {
        return Err(crate::error::anyhow!(
            "{} already exists; choose a new folder for the project.",
            target.display()
        ));
    }
    Ok((name.to_string(), target))
}

/// Move a chat out of `project` into a new project at `path`, seeded with the
/// project's history up to the chat's latest commit. The chat's worktree moves
/// whole, uncommitted and ignored files included. `sessions` are the chat and
/// the side chats sharing its worktree; the caller keeps them idle meanwhile.
pub fn split_chat_project(
    store: &Store,
    project: &LocalProject,
    owner: &str,
    sessions: &[String],
    name: &str,
    path: &str,
) -> Result<LocalProject> {
    let (name, target) = new_project_place(name, path)?;
    let worktree = git::existing_session_worktree_path(project, owner);
    let has_worktree = worktree.join(".git").exists();
    // The new repository can't hold staged changes the chat never committed.
    if has_worktree && !git::git(Some(&worktree), &["diff", "--cached", "--name-only"])?.is_empty()
    {
        return Err(crate::error::anyhow!(
            "This chat has staged changes that aren't committed. Commit or unstage them, then try again."
        ));
    }
    let source = if has_worktree {
        worktree.clone()
    } else {
        PathBuf::from(&project.repo_path)
    };
    let head = git::git(Some(&source), &["rev-parse", "HEAD"])?;

    crate::paths::create_dir_all(&target)?;
    let seeded = (|| -> Result<()> {
        git::git(Some(&target), &["init", "--quiet", "-b", "main"])?;
        // Lets the push below check out what it brings into the empty folder.
        git::git(
            Some(&target),
            &["config", "receive.denyCurrentBranch", "updateInstead"],
        )?;
        let into = target.to_string_lossy();
        git::git(
            Some(&source),
            &["push", "--quiet", &into, &format!("{head}:refs/heads/main")],
        )?;
        git::git(
            Some(&target),
            &["config", "--unset", "receive.denyCurrentBranch"],
        )?;
        Ok(())
    })();
    if let Err(error) = seeded {
        let _ = std::fs::remove_dir_all(&target);
        return Err(crate::error::anyhow!(
            "Could not start the project at {}: {error}",
            target.display()
        ));
    }

    let now = now_ms();
    let created = LocalProject {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.clone(),
        slug: unique_project_slug(store, &slugify(&name))?,
        github_owner: String::new(),
        github_repo: String::new(),
        github_sync_enabled: false,
        baseline_branch: "main".into(),
        repo_path: crate::paths::canonicalize(&target)?
            .to_string_lossy()
            .into_owned(),
        run_command: project.run_command.clone(),
        paper_id: project.paper_id.clone(),
        icon: None,
        created_at: now,
        updated_at: now,
    };
    let moved = git::session_worktree_path(&created.id, owner);
    let relinked = if has_worktree {
        relink_worktree(&worktree, &moved, Path::new(&created.repo_path))
    } else {
        Ok(None)
    };
    let done = relinked.and_then(|undo| {
        store
            .create_local_project_for_chats(&created, sessions)
            .inspect_err(|_| {
                if let Some(undo) = undo {
                    undo();
                }
            })
    });
    if let Err(error) = done {
        let _ = std::fs::remove_dir_all(&target);
        return Err(crate::error::anyhow!(
            "Could not move the chat to {}: {error}",
            target.display()
        ));
    }

    if has_worktree {
        relocate_opencode_sessions(&worktree, &moved);
        // The old registration only points at a folder that has moved.
        if let Err(error) = git::git(
            Some(Path::new(&project.repo_path)),
            &["worktree", "remove", "--force", &worktree.to_string_lossy()],
        ) {
            eprintln!(
                "orx: left a stale worktree entry in {}: {error}",
                project.repo_path
            );
        }
    }
    Ok(created)
}

/// Moves a chat's worktree, files untouched, to `moved` at the same commit in
/// `repo`, which registers it. Returns how to put it back.
fn relink_worktree(worktree: &Path, moved: &Path, repo: &Path) -> Result<Option<impl FnOnce()>> {
    let head = git::git(Some(worktree), &["rev-parse", "HEAD"])?;
    crate::paths::create_dir_all(moved.parent().unwrap_or(moved))?;
    git::git(
        Some(repo),
        &[
            "worktree",
            "add",
            "--no-checkout",
            "--detach",
            &moved.to_string_lossy(),
            &head,
        ],
    )?;
    let new_link = std::fs::read(moved.join(".git"))?;
    let old_link = std::fs::read(worktree.join(".git"))?;
    std::fs::remove_file(moved.join(".git"))?;
    std::fs::remove_dir(moved)?;
    std::fs::rename(worktree, moved)?;
    let undo = {
        let (worktree, moved) = (worktree.to_path_buf(), moved.to_path_buf());
        move || {
            let restored = std::fs::rename(&moved, &worktree)
                .and_then(|()| std::fs::write(worktree.join(".git"), &old_link));
            if let Err(error) = restored {
                eprintln!(
                    "orx: could not put the chat's files back at {}; they are at {}: {error}",
                    worktree.display(),
                    moved.display()
                );
            }
        }
    };
    // The new registration starts with an empty index; this matches it to HEAD
    // and leaves every file as the chat had it.
    let linked = std::fs::write(moved.join(".git"), new_link)
        .map_err(crate::error::Error::from)
        .and_then(|()| git::git(Some(moved), &["reset", "--quiet"]).map(drop));
    match linked {
        Ok(()) => Ok(Some(undo)),
        Err(error) => {
            undo();
            Err(error)
        }
    }
}

/// OpenCode remembers each session's folder, in orx's database or, for older
/// chats, the user's own; point both at the moved worktree. A chat whose
/// session can't be found again starts a new one from orx's transcript.
fn relocate_opencode_sessions(from: &Path, to: &Path) {
    use super::native_store::{opencode_db, opencode_relocation, NativeStore};
    for database in [
        opencode_db(NativeStore::Isolated),
        opencode_db(NativeStore::Legacy),
    ] {
        let relocated = opencode_relocation(&database, |path| {
            path.strip_prefix(from)
                .map(|rest| to.join(rest))
                .unwrap_or_else(|_| path.to_path_buf())
        })
        .and_then(|changes| changes.map_or(Ok(()), |changes| changes.apply()));
        if let Err(error) = relocated {
            eprintln!(
                "orx: {} still names {} for this chat: {error}",
                database.display(),
                from.display()
            );
        }
    }
}

fn move_repository(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        crate::paths::create_dir_all(parent)?;
    }
    // An empty folder the user picked is replaced, which `rename` refuses on Windows.
    if to.is_dir() {
        std::fs::remove_dir(to)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(error) => {
            return Err(crate::error::anyhow!(
                "Could not move {} to {}: {error}",
                from.display(),
                to.display()
            ))
        }
    }
    let parent = to.parent().unwrap_or(to);
    let temporary = parent.join(format!(".orx-copy-{}", uuid::Uuid::new_v4()));
    if let Err(error) = super::storage::copy_verified(from, &temporary) {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(crate::error::anyhow!(
            "Could not copy {} to {}: {error}",
            from.display(),
            to.display()
        ));
    }
    std::fs::rename(&temporary, to)?;
    // The copy is complete; a source that won't go away is only wasted space.
    if let Err(error) = std::fs::remove_dir_all(from) {
        eprintln!(
            "orx: could not remove {} after copying it: {error}",
            from.display()
        );
    }
    Ok(())
}

#[derive(Default)]
pub struct CreateProjectOptions {
    pub create_folder: bool,
    pub require_new_folder: bool,
    pub initialize_git: bool,
    pub clone_url: Option<String>,
    pub shallow_clone: bool,
    pub run_command: Option<String>,
    pub paper_id: Option<String>,
    pub paper_pdf: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("orx-local-project-{}", uuid::Uuid::new_v4()))
    }

    fn run_git(path: &Path, args: &[&str]) {
        let status = crate::process::command("git")
            .current_dir(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {}", args.join(" "));
    }

    fn git_output(path: &Path, args: &[&str]) -> String {
        let output = crate::process::command("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {}", args.join(" "));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn sparse_file(path: &Path, bytes: u64) {
        File::create(path).unwrap().set_len(bytes).unwrap();
    }

    fn initialized(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
        run_git(path, &["init", "-b", "main"]);
        std::fs::write(path.join("README.md"), "# test\n").unwrap();
        run_git(path, &["add", "-A"]);
        run_git(
            path,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "initial",
            ],
        );
    }

    #[test]
    fn reserves_the_demo_slug_for_onboarding() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        assert_eq!(
            project_slug_preview(&store, "nanochat").unwrap(),
            "nanochat-2"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn creates_local_only_project_without_a_remote() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let project_path = root.join("project");
        let project = create_project(
            &store,
            "Local project",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                create_folder: true,
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(project.github_owner.is_empty());
        assert!(project.github_repo.is_empty());
        assert_eq!(project.baseline_branch, "main");
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&project_path).unwrap()
        );
        assert!(git::remotes(&project_path).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_an_existing_folder_for_a_blank_project() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();

        let error = create_project(
            &store,
            "Blank project",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                create_folder: true,
                require_new_folder: true,
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("choose a new folder"));
        assert!(store.list_local_projects().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn create_folder_permission_still_accepts_an_existing_folder() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();

        let project = create_project(
            &store,
            "Existing project",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                create_folder: true,
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(project_path).unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn creates_a_blank_project_nested_in_an_existing_repository() {
        let root = root();
        let repository = root.join("repository");
        initialized(&repository);
        let project_path = repository.join("blank-project");
        let store = Store::open_at(root.join("data")).unwrap();

        let project = create_project(
            &store,
            "Blank project",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                create_folder: true,
                require_new_folder: true,
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        // The nested folder is its own repository, not the enclosing checkout.
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&project_path).unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn paper_project(store: &Store, path: &Path) -> Result<LocalProject> {
        create_project(
            store,
            "Paper project",
            path.to_str().unwrap(),
            CreateProjectOptions {
                create_folder: true,
                initialize_git: true,
                paper_pdf: Some(b"%PDF-1.7 test".to_vec()),
                ..Default::default()
            },
        )
    }

    #[test]
    fn commits_the_paper_pdf_into_a_blank_paper_project() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let project = paper_project(&store, &root.join("project")).unwrap();
        let repo_path = Path::new(&project.repo_path);
        assert_eq!(
            std::fs::read(repo_path.join(PAPER_PDF_NAME)).unwrap(),
            b"%PDF-1.7 test"
        );
        assert_eq!(
            git_output(repo_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            PAPER_PDF_NAME
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn seeds_a_paper_project_in_a_folder_inside_an_existing_repository() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let repository = root.join("repository");
        initialized(&repository);
        let nested = repository.join("nested");
        let project = paper_project(&store, &nested).unwrap();
        // The paper is committed to the nested folder's own repository.
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&nested).unwrap()
        );
        assert_eq!(
            git_output(&nested, &["ls-tree", "-r", "--name-only", "HEAD"]),
            PAPER_PDF_NAME
        );

        let error = paper_project(&store, &repository).unwrap_err().to_string();
        assert!(error.contains("must be empty"), "{error}");
        assert!(!repository.join(PAPER_PDF_NAME).exists());
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn imports_files_inside_plain_subdirectories() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(project_path.join("src/components")).unwrap();
        std::fs::write(project_path.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(
            project_path.join("src/components/mod.rs"),
            "pub struct Component;\n",
        )
        .unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Nested source files",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            "src/components/mod.rs\nsrc/main.rs"
        );
        assert!(!project_path.join(".gitignore").exists());
        assert!(git::is_clean(&project_path).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn imports_an_existing_repository_without_commits() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        run_git(&project_path, &["init", "-b", "trunk"]);
        std::fs::write(project_path.join("README.md"), "# zero commits\n").unwrap();
        run_git(&project_path, &["add", "README.md"]);
        std::fs::write(project_path.join("README.md"), "# working tree wins\n").unwrap();
        std::fs::write(project_path.join("notes.md"), "untracked\n").unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        let project = create_project(
            &store,
            "Zero commits",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(project.baseline_branch, "trunk");
        assert_eq!(
            git_output(&project_path, &["rev-list", "--count", "HEAD"]),
            "1"
        );
        assert_eq!(
            git_output(&project_path, &["log", "-1", "--format=%an <%ae>"]),
            "OpenResearch <local@openresearch.sh>"
        );
        assert_eq!(
            git_output(&project_path, &["show", "HEAD:README.md"]),
            "# working tree wins"
        );
        assert_eq!(
            git_output(&project_path, &["show", "HEAD:notes.md"]),
            "untracked"
        );
        assert!(git::is_clean(&project_path).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_an_invalid_repository_without_reinitializing_it() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(project_path.join(".git")).unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        let error = create_project(
            &store,
            "Invalid",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("not a valid Git repository"));
        assert!(!project_path.join(".git/HEAD").exists());
        assert!(store.list_local_projects().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn excludes_files_at_the_fifty_megabyte_boundary() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(project_path.join("data set")).unwrap();
        std::fs::write(project_path.join("README.md"), "# test\n").unwrap();
        std::fs::write(project_path.join(".gitignore"), "ignored.bin\n").unwrap();
        sparse_file(&project_path.join("ignored.bin"), 2 * 1024 * 1024 * 1024);
        let large_file = project_path.join("data set/checkpoint[1].bin");
        sparse_file(&large_file, git::INITIAL_SNAPSHOT_MAX_FILE_BYTES);
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Large local data",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(large_file.exists());
        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        let gitignore = std::fs::read_to_string(project_path.join(".gitignore")).unwrap();
        assert!(gitignore.starts_with("ignored.bin\n"));
        assert!(gitignore.contains("/data\\ set/checkpoint\\[1\\].bin"));
        assert_eq!(
            gitignore
                .matches("/data\\ set/checkpoint\\[1\\].bin")
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn excludes_a_staged_large_file_even_when_a_nested_ignore_negates_the_exclusion() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(project_path.join("data")).unwrap();
        run_git(&project_path, &["init", "-b", "main"]);
        std::fs::write(project_path.join("README.md"), "# test\n").unwrap();
        std::fs::write(project_path.join("data/.gitignore"), "!checkpoint.bin\n").unwrap();
        let large_file = project_path.join("data/checkpoint.bin");
        sparse_file(&large_file, git::INITIAL_SNAPSHOT_MAX_FILE_BYTES);
        run_git(&project_path, &["add", "-A"]);
        let store = Store::open_at(root.join("data-store")).unwrap();

        create_project(
            &store,
            "Staged large file",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        let committed = git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]);
        assert!(committed.contains("README.md"));
        assert!(committed.contains("data/.gitignore"));
        assert!(!committed.contains("checkpoint.bin"));
        assert!(large_file.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn does_not_commit_a_large_file_that_was_force_staged_despite_gitignore() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        run_git(&project_path, &["init", "-b", "main"]);
        std::fs::write(project_path.join(".gitignore"), "checkpoint.bin\n").unwrap();
        let large_file = project_path.join("checkpoint.bin");
        sparse_file(&large_file, git::INITIAL_SNAPSHOT_MAX_FILE_BYTES);
        run_git(&project_path, &["add", ".gitignore"]);
        run_git(&project_path, &["add", "-f", "checkpoint.bin"]);
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Ignored staged file",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore"
        );
        assert!(large_file.exists());
        assert!(git::is_clean(&project_path).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn preserves_a_symlinked_gitignore_while_excluding_large_files() {
        use std::os::unix::fs::symlink;

        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        let shared_ignore = root.join("shared-ignore");
        std::fs::write(&shared_ignore, "shared rule\n").unwrap();
        symlink(&shared_ignore, project_path.join(".gitignore")).unwrap();
        std::fs::write(project_path.join("README.md"), "# test\n").unwrap();
        let large_file = project_path.join("checkpoint.bin");
        sparse_file(&large_file, git::INITIAL_SNAPSHOT_MAX_FILE_BYTES);
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Symlinked ignore",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(std::fs::symlink_metadata(project_path.join(".gitignore"))
            .unwrap()
            .is_symlink());
        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        assert!(large_file.exists());
        assert!(git::is_clean(&project_path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&shared_ignore).unwrap(),
            "shared rule\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_gitignore_does_not_block_excluding_a_nested_unborn_repository() {
        use std::os::unix::fs::symlink;

        let root = root();
        let project_path = root.join("project");
        let nested = project_path.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        run_git(&nested, &["init", "-b", "main"]);
        sparse_file(
            &nested.join("checkpoint.bin"),
            git::INITIAL_SNAPSHOT_MAX_FILE_BYTES,
        );
        let shared_ignore = root.join("shared-ignore");
        std::fs::write(&shared_ignore, "shared rule\n").unwrap();
        symlink(&shared_ignore, project_path.join(".gitignore")).unwrap();
        std::fs::write(project_path.join("README.md"), "# parent\n").unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Symlinked ignore and nested repo",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(std::fs::symlink_metadata(project_path.join(".gitignore"))
            .unwrap()
            .is_symlink());
        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        assert!(nested.join(".git").exists());
        assert!(nested.join("checkpoint.bin").exists());
        assert!(git::is_clean(&project_path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&shared_ignore).unwrap(),
            "shared rule\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn excludes_a_nested_unborn_repository_without_blocking_import() {
        let root = root();
        let project_path = root.join("project");
        let nested = project_path.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        run_git(&nested, &["init", "-b", "main"]);
        std::fs::write(nested.join("draft.md"), "nested work\n").unwrap();
        std::fs::write(project_path.join("README.md"), "# parent\n").unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Nested unborn repository",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        let gitignore = std::fs::read_to_string(project_path.join(".gitignore")).unwrap();
        assert!(gitignore.contains("/nested/"));
        assert!(nested.join(".git").exists());
        assert!(nested.join("draft.md").exists());
        assert!(git::is_clean(&project_path).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn commits_a_nested_ready_repository_as_a_gitlink() {
        let root = root();
        let project_path = root.join("project");
        let nested = project_path.join("nested");
        initialized(&nested);
        std::fs::write(project_path.join("README.md"), "# parent\n").unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Nested ready repository",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert!(
            git_output(&project_path, &["ls-tree", "HEAD", "nested"]).starts_with("160000 commit ")
        );
        assert!(nested.join("README.md").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn excludes_an_invalid_nested_repository() {
        let root = root();
        let project_path = root.join("project");
        let nested = project_path.join("nested");
        std::fs::create_dir_all(nested.join(".git")).unwrap();
        sparse_file(
            &nested.join("checkpoint.bin"),
            git::INITIAL_SNAPSHOT_MAX_FILE_BYTES,
        );
        std::fs::write(project_path.join("README.md"), "# parent\n").unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Invalid nested repository",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        assert!(nested.join(".git").exists());
        assert!(nested.join("checkpoint.bin").exists());
        assert!(git::is_clean(&project_path).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn blocks_a_one_gigabyte_initial_snapshot_without_mutating_the_folder() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        for index in 0..20 {
            sparse_file(
                &project_path.join(format!("part-{index}.bin")),
                49 * 1024 * 1024,
            );
        }
        sparse_file(&project_path.join("remainder.bin"), 44 * 1024 * 1024);
        let store = Store::open_at(root.join("data")).unwrap();

        let error = create_project(
            &store,
            "Too large",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "This project is too large to import. After excluding files 50 MB or larger, the remaining files exceed OpenResearch's 1 GB limit."
        );
        assert!(!project_path.join(".git").exists());
        assert!(!project_path.join(".gitignore").exists());
        assert!(store.list_local_projects().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn restores_gitignore_when_the_managed_block_crosses_the_total_limit() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        let original_gitignore = b"custom\n";
        std::fs::write(project_path.join(".gitignore"), original_gitignore).unwrap();
        for index in 0..20 {
            sparse_file(
                &project_path.join(format!("part-{index}.bin")),
                49 * 1024 * 1024,
            );
        }
        let base_bytes = 20 * 49 * 1024 * 1024 + original_gitignore.len() as u64;
        sparse_file(
            &project_path.join("remainder.bin"),
            git::INITIAL_SNAPSHOT_MAX_TOTAL_BYTES - base_bytes - 1,
        );
        sparse_file(
            &project_path.join("large.bin"),
            git::INITIAL_SNAPSHOT_MAX_FILE_BYTES,
        );
        let store = Store::open_at(root.join("data")).unwrap();

        let error = create_project(
            &store,
            "Managed ignore overflow",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("too large to import"));
        assert_eq!(
            std::fs::read(project_path.join(".gitignore")).unwrap(),
            original_gitignore
        );
        assert!(!project_path.join(".git").exists());
        assert!(store.list_local_projects().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn leaves_existing_repositories_with_commits_untouched() {
        let root = root();
        let project_path = root.join("project");
        initialized(&project_path);
        let original_head = git_output(&project_path, &["rev-parse", "HEAD"]);
        sparse_file(
            &project_path.join("local-checkpoint.bin"),
            git::INITIAL_SNAPSHOT_MAX_FILE_BYTES,
        );
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Existing",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            git_output(&project_path, &["rev-parse", "HEAD"]),
            original_head
        );
        assert!(!project_path.join(".gitignore").exists());
        assert!(project_path.join("local-checkpoint.bin").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn managed_initial_commit_bypasses_signing_and_hooks() {
        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        run_git(&project_path, &["init", "-b", "main"]);
        run_git(&project_path, &["config", "commit.gpgsign", "true"]);
        run_git(&project_path, &["config", "gpg.program", "false"]);
        std::fs::write(project_path.join("README.md"), "# test\n").unwrap();
        let hook = project_path.join(".git/hooks/post-commit");
        orx_test_support::write_executable(&hook, "#!/bin/sh\nexit 1\n", 0o755).unwrap();
        let store = Store::open_at(root.join("data")).unwrap();

        create_project(
            &store,
            "Managed commit",
            project_path.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            git_output(&project_path, &["rev-list", "--count", "HEAD"]),
            "1"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_dirty_but_rejects_detached_repositories() {
        let root = root();
        let dirty = root.join("dirty");
        initialized(&dirty);
        std::fs::write(dirty.join("README.md"), "changed\n").unwrap();
        let store = Store::open_at(root.join("data")).unwrap();
        let project = create_project(
            &store,
            "Dirty",
            dirty.to_str().unwrap(),
            CreateProjectOptions::default(),
        )
        .unwrap();
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&dirty).unwrap()
        );
        assert!(!git::is_clean(&dirty).unwrap());

        let detached = root.join("detached");
        initialized(&detached);
        run_git(&detached, &["checkout", "--detach"]);
        let error = create_project(
            &store,
            "Detached",
            detached.to_str().unwrap(),
            CreateProjectOptions::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("detached HEAD"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn paper_clone_keeps_source_as_upstream() {
        let root = root();
        let source = root.join("source");
        initialized(&source);
        let store = Store::open_at(root.join("data")).unwrap();
        let destination = root.join("paper");
        let project = create_project(
            &store,
            "Paper",
            destination.to_str().unwrap(),
            CreateProjectOptions {
                create_folder: true,
                clone_url: Some(source.to_string_lossy().into_owned()),
                paper_id: Some("2401.12345".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        let remotes = git::remotes(Path::new(&project.repo_path)).unwrap();
        assert_eq!(remotes[0].0, "upstream");
        assert!(!remotes.iter().any(|(name, _)| name == "origin"));
        assert!(project.github_owner.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ordinary_github_origin_remains_opt_in() {
        let root = root();
        let project_path = root.join("project");
        initialized(&project_path);
        run_git(
            &project_path,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:example/research.git",
            ],
        );
        let store = Store::open_at(root.join("data")).unwrap();
        let project = create_project(
            &store,
            "Existing",
            project_path.to_str().unwrap(),
            CreateProjectOptions::default(),
        )
        .unwrap();
        assert!(!project.github_enabled());
        assert_eq!(project.github_owner, "example");
        assert_eq!(project.github_repo, "research");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dedicated_github_remote_is_recognized_but_remains_opt_in() {
        let root = root();
        let project_path = root.join("project");
        initialized(&project_path);
        run_git(
            &project_path,
            &[
                "remote",
                "add",
                "github",
                "git@github.com:example/research.git",
            ],
        );
        let store = Store::open_at(root.join("data")).unwrap();
        let project = create_project(
            &store,
            "Existing",
            project_path.to_str().unwrap(),
            CreateProjectOptions::default(),
        )
        .unwrap();
        assert!(!project.github_enabled());
        assert_eq!(project.github_owner, "example");
        assert_eq!(project.github_repo, "research");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nested_folder_resolves_to_repository_root() {
        let root = root();
        let project_path = root.join("project");
        initialized(&project_path);
        let nested = project_path.join("src/nested");
        std::fs::create_dir_all(&nested).unwrap();
        let store = Store::open_at(root.join("data")).unwrap();
        let project = create_project(
            &store,
            "Nested",
            nested.to_str().unwrap(),
            CreateProjectOptions::default(),
        )
        .unwrap();
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(project_path).unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nested_folder_in_an_unborn_repository_initializes_the_repository_root() {
        let root = root();
        let project_path = root.join("project");
        let nested = project_path.join("src/nested");
        std::fs::create_dir_all(&nested).unwrap();
        run_git(&project_path, &["init", "-b", "main"]);
        std::fs::write(project_path.join("README.md"), "# test\n").unwrap();
        sparse_file(
            &project_path.join("checkpoint.bin"),
            git::INITIAL_SNAPSHOT_MAX_FILE_BYTES,
        );
        let store = Store::open_at(root.join("data")).unwrap();

        let project = create_project(
            &store,
            "Nested unborn",
            nested.to_str().unwrap(),
            CreateProjectOptions {
                initialize_git: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&project_path).unwrap()
        );
        assert!(project_path.join(".gitignore").exists());
        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_new_chat_without_a_project_reuses_an_unused_scratch_project() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();

        let first = scratch_project(&store).unwrap();
        assert!(first.is_scratch());
        assert_eq!(scratch_project(&store).unwrap().id, first.id);

        let _ = std::fs::remove_dir_all(&first.repo_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn creating_a_project_moves_the_repository_and_leaves_unsaved_work_in_the_chat() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let scratch = scratch_project(&store).unwrap();
        let worktree = root.join("worktree");
        run_git(
            Path::new(&scratch.repo_path),
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(worktree.join("fib.py"), "print(1)\n").unwrap();
        run_git(&worktree, &["add", "fib.py"]);
        run_git(
            &worktree,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e",
                "commit",
                "-qm",
                "fib",
            ],
        );
        std::fs::write(worktree.join("draft.md"), "unsaved\n").unwrap();
        let artifacts = crate::local::files::files_dir(&scratch);
        std::fs::create_dir_all(&artifacts).unwrap();
        std::fs::write(artifacts.join("report.md"), "# report\n").unwrap();

        let target = root.join("projects").join("fib");
        let project = promote_scratch_project(
            &store,
            &scratch,
            Some(&worktree),
            "Fibonacci sandbox",
            target.to_str().unwrap(),
        )
        .unwrap();

        assert!(!project.is_scratch());
        assert_eq!(project.id, scratch.id);
        assert_eq!(project.name, "Fibonacci sandbox");
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&target).unwrap()
        );
        assert!(!Path::new(&scratch.repo_path).exists());
        // The folder holds what the chat committed, and nothing orx committed for it.
        assert_eq!(git_output(&target, &["show", "HEAD:fib.py"]), "print(1)");
        assert!(target.join("fib.py").exists());
        assert_eq!(git_output(&target, &["log", "-1", "--format=%s"]), "fib");
        // The chat keeps working in its worktree, its unsaved work intact.
        assert_eq!(
            std::fs::read_to_string(worktree.join("draft.md")).unwrap(),
            "unsaved\n"
        );
        assert_eq!(
            git_output(&worktree, &["rev-parse", "HEAD"]),
            git_output(&target, &["rev-parse", "HEAD"])
        );
        // Artifacts stay where the chat's messages link to them.
        assert_eq!(project.slug, scratch.slug);
        assert!(crate::local::files::files_dir(&project)
            .join("report.md")
            .exists());

        let again = promote_scratch_project(&store, &project, None, "Other", "/nowhere");
        assert!(again
            .err()
            .unwrap()
            .to_string()
            .contains("already a project"));
        let _ = std::fs::remove_dir_all(crate::local::files::files_dir(&project));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn creating_a_project_refuses_a_folder_with_files() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let scratch = scratch_project(&store).unwrap();
        let occupied = root.join("occupied");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("notes.txt"), "mine").unwrap();

        let error =
            promote_scratch_project(&store, &scratch, None, "Mine", occupied.to_str().unwrap())
                .err()
                .unwrap()
                .to_string();

        assert!(error.contains("choose a new folder"), "{error}");
        assert!(Path::new(&scratch.repo_path).exists());
        assert!(store
            .get_local_project(&scratch.id)
            .unwrap()
            .unwrap()
            .is_scratch());
        let _ = std::fs::remove_dir_all(&scratch.repo_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn moving_a_chat_to_a_new_project_keeps_its_history_and_its_worktree() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let source = root.join("source");
        initialized(&source);
        std::fs::write(source.join(".gitignore"), "data/\n").unwrap();
        run_git(&source, &["add", "-A"]);
        run_git(
            &source,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e",
                "commit",
                "-qm",
                "ignore data",
            ],
        );
        let project = create_project(
            &store,
            "Source",
            source.to_str().unwrap(),
            Default::default(),
        )
        .unwrap();
        let session = format!("chat_{}", uuid::Uuid::new_v4());
        let worktree = git::session_worktree_path(&project.id, &session);
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        run_git(
            &source,
            &[
                "worktree",
                "add",
                "--detach",
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        std::fs::write(worktree.join("probe.py"), "print(2)\n").unwrap();
        std::fs::create_dir_all(worktree.join("data")).unwrap();
        std::fs::write(worktree.join("data").join("shard.bin"), "bytes").unwrap();

        run_git(&worktree, &["add", "probe.py"]);
        run_git(
            &worktree,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e",
                "commit",
                "-qm",
                "probe",
            ],
        );
        std::fs::write(worktree.join("README.md"), "# edited\n").unwrap();

        // Staged changes would be lost to the new repository, so they block the move.
        run_git(&worktree, &["add", "README.md"]);
        let staged = split_chat_project(
            &store,
            &project,
            &session,
            std::slice::from_ref(&session),
            "Probe",
            root.join("staged").to_str().unwrap(),
        );
        assert!(staged.err().unwrap().to_string().contains("staged changes"));
        assert!(!root.join("staged").exists());
        run_git(&worktree, &["reset", "--quiet"]);

        let target = root.join("split");
        let created = split_chat_project(
            &store,
            &project,
            &session,
            std::slice::from_ref(&session),
            "Probe",
            target.to_str().unwrap(),
        )
        .unwrap();

        assert_ne!(created.id, project.id);
        assert!(!created.is_scratch());
        // Git's own copy: a Windows checkout ends lines with CRLF.
        assert_eq!(git_output(&target, &["show", "HEAD:probe.py"]), "print(2)");
        assert_eq!(
            git_output(&target, &["log", "--format=%s"]),
            "probe\nignore data\ninitial"
        );
        // The chat's worktree moves whole: ignored files and unsaved edits stay as they were.
        let moved = git::session_worktree_path(&created.id, &session);
        assert_eq!(
            std::fs::read_to_string(moved.join("data").join("shard.bin")).unwrap(),
            "bytes"
        );
        assert_eq!(
            std::fs::read_to_string(moved.join("README.md")).unwrap(),
            "# edited\n"
        );
        assert_eq!(
            git_output(&moved, &["status", "--porcelain"]),
            "M README.md"
        );
        assert_eq!(
            git_output(&moved, &["rev-parse", "HEAD"]),
            git_output(&target, &["rev-parse", "HEAD"])
        );
        assert!(!worktree.exists());
        // The source project keeps its own history.
        assert_eq!(
            git_output(&source, &["log", "-1", "--format=%s"]),
            "ignore data"
        );
        let _ = std::fs::remove_dir_all(git::worktrees_root(&created.id));
        let _ = std::fs::remove_dir_all(git::worktrees_root(&project.id));
        let _ = std::fs::remove_dir_all(root);
    }
}
