//! Local project creation — clone the repo and insert the project row. Used by
//! the `orx up` HTTP API (`POST /api/projects`).
//!
//! The project starts with an empty experiment tree. The first experiment
//! created without a parent via `orx create-experiment` becomes the baseline
//! root — the control every variant is measured against.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::store::{now_ms, ChatProjectMove, Store};

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
    if let Some(rest) = under_home(trimmed) {
        let home = crate::config::home_dir()
            .ok_or_else(|| crate::error::anyhow!("Could not resolve the home directory"))?;
        return Ok(if rest.is_empty() {
            home
        } else {
            home.join(rest)
        });
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(relative_base()?.join(path))
    }
}

/// The rest of a path written from the home folder, as `~` or `~/…`.
fn under_home(path: &str) -> Option<&str> {
    if path == "~" {
        return Some("");
    }
    path.strip_prefix("~/")
        .or_else(|| path.strip_prefix("~\\").filter(|_| cfg!(windows)))
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
        proposal: None,
        created_at: now,
        updated_at: now,
    };
    store.create_local_project(&project)?;
    Ok(project)
}

pub fn scratch_root() -> PathBuf {
    crate::store::data_dir().join("scratch")
}

/// What a scratch project is called until the user names it; older versions show it as a project of this name.
pub const UNNAMED_SCRATCH: &str = "No project";

/// The project a new chat without a project starts in. An unnamed scratch
/// project no chat uses yet is reused, so opening the draft again leaves no new folder.
pub fn scratch_project(store: &Store) -> Result<LocalProject> {
    for project in store.list_local_projects()? {
        if project.is_unnamed_scratch()
            && store.list_chat_sessions_by_project(&project.id)?.is_empty()
        {
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
        name: UNNAMED_SCRATCH.to_string(),
        github_owner: String::new(),
        github_repo: String::new(),
        github_sync_enabled: false,
        baseline_branch: git::require_current_branch(&repo)?,
        repo_path: repo.to_string_lossy().into_owned(),
        run_command: None,
        paper_id: None,
        icon: None,
        proposal: None,
        created_at: now,
        updated_at: now,
    };
    store.create_local_project(&project)?;
    Ok(project)
}

/// A move of a project's files, recorded before it starts so a start after a
/// crash can finish or undo it ([`recover_project_moves`]). `request` is the
/// waiting move it carries out, given back to `requester` if it is undone.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum MoveInFlight {
    /// A scratch project's repository moving from `source` to `target`;
    /// `tip` is its baseline before the chat's commits came onto it.
    #[serde(rename_all = "camelCase")]
    Promote {
        requester: String,
        request: ChatProjectMove,
        source: PathBuf,
        target: PathBuf,
        tip: String,
    },
    /// A chat's worktree moving to `moved` in a new repository at `target`,
    /// recorded under the new project's id; `old_link` is its `.git` file before.
    #[serde(rename_all = "camelCase")]
    Split {
        requester: String,
        request: ChatProjectMove,
        source_repo: PathBuf,
        target: PathBuf,
        worktree: PathBuf,
        moved: PathBuf,
        old_link: Option<String>,
    },
}

/// What a move that lost its request to a cancel says.
pub const MOVE_CANCELLED: &str = "The move was cancelled.";

/// Records the move and takes its request in one write, so a crash leaves one or the other.
fn begin_move(store: &Store, project_id: &str, state: &MoveInFlight) -> Result<()> {
    let (MoveInFlight::Promote {
        requester, request, ..
    }
    | MoveInFlight::Split {
        requester, request, ..
    }) = state;
    let json = serde_json::to_string(state)?;
    if !store.begin_project_move(project_id, &json, requester, request.requested_at)? {
        return Err(crate::error::anyhow!(MOVE_CANCELLED));
    }
    Ok(())
}

/// A finished move needs no recovery; a record left behind only costs a check at the next start.
fn end_move(store: &Store, project_id: &str) {
    if let Err(error) = store.end_project_move(project_id) {
        eprintln!("orx: could not clear the record of moving project {project_id}: {error}");
    }
}

/// Where a repository waits beside `target` until it is whole: a copy across
/// volumes, or a split's new repository.
fn staged_copy(target: &Path, project_id: &str) -> PathBuf {
    target
        .parent()
        .unwrap_or(target)
        .join(format!(".orx-copy-{project_id}"))
}

/// Marks a repository as moved by `project_id`'s move, so undoing it never
/// removes a folder another move or the user put at the destination.
fn ownership_mark(repo: &Path, project_id: &str) -> PathBuf {
    repo.join(".git").join(format!("orx-move-{project_id}"))
}

fn owned(repo: &Path, project_id: &str) -> bool {
    ownership_mark(repo, project_id).exists()
}

/// A mark left behind is an empty file in `.git` that nothing reads.
fn unmark(repo: &Path, project_id: &str) {
    let _ = std::fs::remove_file(ownership_mark(repo, project_id));
}

/// Turn a scratch project into a real one as `request` asks: bring the chat's
/// commits onto the baseline, move the repository to its folder, and name it.
/// What the chat left uncommitted stays in its worktree. The caller keeps every
/// chat of the project idle meanwhile; `requester` is the chat that asked.
pub fn promote_scratch_project(
    store: &Store,
    project: &LocalProject,
    requester: &str,
    worktree: Option<&Path>,
    request: &ChatProjectMove,
) -> Result<LocalProject> {
    if !project.is_scratch() {
        return Err(crate::error::anyhow!(
            "{} is already a project, at {}.",
            project.name,
            project.repo_path
        ));
    }
    let (name, target) = new_project_place(&request.name, &request.path)?;
    let repo = PathBuf::from(&project.repo_path);
    let branch = git::require_current_branch(&repo)?;
    if branch != project.baseline_branch {
        return Err(crate::error::anyhow!(
            "{} is on {branch}, not {}; switch it back, then try again.",
            repo.display(),
            project.baseline_branch
        ));
    }
    let tip = git::git(Some(&repo), &["rev-parse", "HEAD"])?;
    begin_move(
        store,
        &project.id,
        &MoveInFlight::Promote {
            requester: requester.to_string(),
            request: request.clone(),
            source: repo.clone(),
            target: target.clone(),
            tip: tip.clone(),
        },
    )?;

    let promoted = (|| -> Result<LocalProject> {
        std::fs::write(ownership_mark(&repo, &project.id), "")?;
        if let Some(dir) = worktree.filter(|dir| dir.join(".git").exists()) {
            let head = git::git(Some(dir), &["rev-parse", "HEAD"])?;
            git::git(Some(&repo), &["merge", "--ff-only", "--quiet", &head]).map_err(|error| {
                crate::error::anyhow!(
                    "Could not bring the chat's work onto {}: {error}",
                    project.baseline_branch
                )
            })?;
        }
        move_repository(&repo, &target, &staged_copy(&target, &project.id))?;
        git::git(Some(&target), &["worktree", "repair"])?;
        record_promotion(store, project, &name, &target, request.proposal.as_deref())
    })();
    match promoted {
        Ok(promoted) => {
            unmark(&target, &project.id);
            remove_copied_source(&repo);
            end_move(store, &project.id);
            Ok(promoted)
        }
        Err(error) => match undo_promotion(&repo, &target, &project.id, &tip) {
            Ok(()) => {
                end_move(store, &project.id);
                Err(crate::error::anyhow!(
                    "Could not move the project to {}: {error}",
                    target.display()
                ))
            }
            // Still recorded, so the next start tries to put it back again.
            Err(restore) => Err(crate::error::anyhow!(
                "Could not move the project to {}: {error}. Moving it back also failed: {restore}",
                target.display()
            )),
        },
    }
}

/// Names the project and points it at `target`, created as of now. The slug,
/// and the artifacts folder named by it, stay: the chat's earlier messages link
/// to files there.
fn record_promotion(
    store: &Store,
    project: &LocalProject,
    name: &str,
    target: &Path,
    proposal: Option<&str>,
) -> Result<LocalProject> {
    let now = now_ms();
    let promoted = LocalProject {
        name: name.to_string(),
        repo_path: crate::paths::canonicalize(target)?
            .to_string_lossy()
            .into_owned(),
        proposal: proposal.map(str::to_string),
        created_at: now,
        updated_at: now,
        ..project.clone()
    };
    store.promote_local_project(&promoted)?;
    Ok(promoted)
}

/// A repository copied across volumes leaves its source until the store names the copy.
fn remove_copied_source(source: &Path) {
    if source.exists() {
        if let Err(error) = std::fs::remove_dir_all(source) {
            eprintln!(
                "orx: could not remove {} after copying it: {error}",
                source.display()
            );
        }
    }
}

/// Puts a scratch repository back at `source` with its baseline at `tip`,
/// whichever step the move reached: a copy is dropped while the source is
/// whole, a renamed repository goes back. Safe to repeat.
fn undo_promotion(source: &Path, target: &Path, project_id: &str, tip: &str) -> Result<()> {
    let staged = staged_copy(target, project_id);
    if staged.exists() {
        std::fs::remove_dir_all(&staged)?;
    }
    let ours = owned(target, project_id);
    if source.join(".git").exists() {
        if ours {
            std::fs::remove_dir_all(target)?;
        }
    } else if ours {
        move_repository(target, source, &staged)?;
        remove_copied_source(target);
    } else {
        return Err(crate::error::anyhow!(
            "the repository is at neither {} nor {}",
            source.display(),
            target.display()
        ));
    }
    unmark(source, project_id);
    git::git(Some(source), &["worktree", "repair"])?;
    if git::git(Some(source), &["rev-parse", "HEAD"])? != tip {
        git::git(Some(source), &["reset", "--keep", "--quiet", tip])?;
    }
    Ok(())
}

/// The trimmed name and the folder for a project made from a chat, which must
/// be new or empty and outside the data directory.
pub fn new_project_place(name: &str, path: &str) -> Result<(String, PathBuf)> {
    let name = name.trim();
    if name.is_empty() {
        return Err(crate::error::anyhow!("Give the project a name."));
    }
    // A relative path would land wherever the dashboard happens to run, not where the agent or user meant.
    if under_home(path.trim()).is_none() && !Path::new(path.trim()).is_absolute() {
        return Err(crate::error::anyhow!(
            "{} is not a full path; give the folder as one, such as ~/OpenResearch/{}.",
            path.trim(),
            slugify(name)
        ));
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

/// Move a chat out of `project` into a new project as `request` asks, seeded
/// with the project's history up to the chat's latest commit. The chat's
/// worktree moves whole, uncommitted and ignored files included. `sessions` are
/// the chat and the side chats sharing `owner`'s worktree; the caller keeps them
/// idle meanwhile. `requester` is the chat that asked.
pub fn split_chat_project(
    store: &Store,
    project: &LocalProject,
    requester: &str,
    owner: &str,
    sessions: &[String],
    request: &ChatProjectMove,
) -> Result<LocalProject> {
    let (name, target) = new_project_place(&request.name, &request.path)?;
    let worktree = git::existing_session_worktree_path(store, project, owner)?;
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
    let id = uuid::Uuid::new_v4().to_string();
    let moved = git::session_worktree_path(&id, owner);
    let old_link = if has_worktree {
        Some(std::fs::read_to_string(worktree.join(".git"))?)
    } else {
        None
    };
    begin_move(
        store,
        &id,
        &MoveInFlight::Split {
            requester: requester.to_string(),
            request: request.clone(),
            source_repo: PathBuf::from(&project.repo_path),
            target: target.clone(),
            worktree: worktree.clone(),
            moved: moved.clone(),
            old_link: old_link.clone(),
        },
    )?;
    let created = (|| -> Result<LocalProject> {
        // Seeded beside the folder and renamed onto it, which fails rather than
        // mixing with anything that arrived there since it was checked.
        let staged = staged_copy(&target, &id);
        let seeded = (|| -> Result<()> {
            crate::paths::create_dir_all(staged.parent().unwrap_or(&staged))?;
            std::fs::create_dir(&staged)?;
            git::git(Some(&staged), &["init", "--quiet", "-b", "main"])?;
            std::fs::write(ownership_mark(&staged, &id), "")?;
            // Lets the push below check out what it brings into the empty folder.
            git::git(
                Some(&staged),
                &["config", "receive.denyCurrentBranch", "updateInstead"],
            )?;
            let into = staged.to_string_lossy();
            git::git(
                Some(&source),
                &["push", "--quiet", &into, &format!("{head}:refs/heads/main")],
            )?;
            git::git(
                Some(&staged),
                &["config", "--unset", "receive.denyCurrentBranch"],
            )?;
            // An empty folder the user picked is replaced, which `rename` refuses on Windows.
            if target.is_dir() {
                std::fs::remove_dir(&target)?;
            }
            std::fs::rename(&staged, &target)?;
            Ok(())
        })();
        if let Err(error) = seeded {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(crate::error::anyhow!(
                "Could not start the project at {}: {error}",
                target.display()
            ));
        }

        let now = now_ms();
        let created = LocalProject {
            id: id.clone(),
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
            proposal: request.proposal.clone(),
            created_at: now,
            updated_at: now,
        };
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
            // Files that couldn't go back still link to the new repository, so it
            // stays, and so does the record the next start puts them back from.
            let linked_back = old_link.as_ref().is_none_or(|link| {
                std::fs::read_to_string(worktree.join(".git")).ok().as_ref() == Some(link)
            });
            if moved.exists() || !linked_back {
                let files = if moved.exists() { &moved } else { &worktree };
                return Err(crate::error::anyhow!(
                    "Could not move the chat to {}: {error}. Its files are at {}, and the next start puts them back.",
                    target.display(),
                    files.display()
                ));
            }
            if owned(&target, &id) {
                let _ = std::fs::remove_dir_all(&target);
            }
            return Err(crate::error::anyhow!(
                "Could not move the chat to {}: {error}",
                target.display()
            ));
        }

        unmark(&target, &id);
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
    })();
    // A chat whose files stayed in the new repository keeps its record for the next start.
    if created.is_ok() || !target.join(".git").exists() || !owned(&target, &id) {
        end_move(store, &id);
    }
    created
}

/// Finishes or undoes each move a crash interrupted; run before any chat can
/// start. A move whose files all reached the new folder is recorded there; any
/// other goes back and its request waits to be carried out again.
pub fn recover_project_moves(store: &Store) {
    let moves = match store.project_moves_in_flight() {
        Ok(moves) => moves,
        Err(error) => {
            eprintln!("orx up: could not read interrupted project moves: {error}");
            return;
        }
    };
    for (project_id, state) in moves {
        let recovered = serde_json::from_str::<MoveInFlight>(&state)
            .map_err(crate::error::Error::from)
            .and_then(|state| recover_move(store, &project_id, state));
        match recovered {
            Ok(()) => end_move(store, &project_id),
            // Kept, so the next start tries again and says so again.
            Err(error) => eprintln!(
                "orx up: could not finish or undo the interrupted move of project {project_id}: {error}"
            ),
        }
    }
}

/// Each step checks what is on disk first, so a start cut short here can run it again.
fn recover_move(store: &Store, project_id: &str, state: MoveInFlight) -> Result<()> {
    let (requester, request) = match state {
        MoveInFlight::Promote {
            requester,
            request,
            source,
            target,
            tip,
        } => {
            let project = store
                .get_local_project(project_id)?
                .ok_or_else(|| crate::error::anyhow!("project {project_id} was not found"))?;
            if !project.is_scratch() {
                unmark(&target, project_id);
                remove_copied_source(&source);
                return Ok(());
            }
            if !source.join(".git").exists() && owned(&target, project_id) {
                git::git(Some(&target), &["worktree", "repair"])?;
                record_promotion(
                    store,
                    &project,
                    request.name.trim(),
                    &target,
                    request.proposal.as_deref(),
                )?;
                unmark(&target, project_id);
                return Ok(());
            }
            undo_promotion(&source, &target, project_id, &tip)?;
            (requester, request)
        }
        MoveInFlight::Split {
            requester,
            request,
            source_repo,
            target,
            worktree,
            moved,
            old_link,
        } => {
            if store.get_local_project(project_id)?.is_some() {
                relocate_opencode_sessions(&worktree, &moved);
                git::git(Some(&source_repo), &["worktree", "prune"])?;
                unmark(&target, project_id);
                return Ok(());
            }
            if moved.exists() {
                if !worktree.exists() {
                    std::fs::rename(&moved, &worktree)?;
                } else if only_git_link(&moved)? {
                    // The registration made before the chat's files moved into it.
                    std::fs::remove_dir_all(&moved)?;
                } else {
                    return Err(crate::error::anyhow!(
                        "the chat's files are in both {} and {}",
                        worktree.display(),
                        moved.display()
                    ));
                }
            }
            if let Some(link) = old_link {
                let current = std::fs::read_to_string(worktree.join(".git")).ok();
                if current.as_deref() != Some(link.as_str()) {
                    std::fs::write(worktree.join(".git"), link)?;
                }
            }
            if let Some(parent) = moved.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            let staged = staged_copy(&target, project_id);
            if staged.exists() {
                std::fs::remove_dir_all(&staged)?;
            }
            if owned(&target, project_id) {
                std::fs::remove_dir_all(&target)?;
            }
            (requester, request)
        }
    };
    // A request the chat made since takes precedence over the one cut short.
    if store.chat_project_move(&requester)?.is_some() {
        return Ok(());
    }
    store.set_chat_project_move(
        &requester,
        &request.name,
        &request.path,
        request.proposal.as_deref(),
        request.scratch,
    )?;
    Ok(())
}

/// Whether `dir` holds nothing but a `.git` link.
fn only_git_link(dir: &Path) -> Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        if entry?.file_name() != ".git" {
            return Ok(false);
        }
    }
    Ok(true)
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

/// Renames `from` to `to`, or across volumes copies it there through `staged`
/// and leaves `from` for the caller to remove once nothing can fail.
fn move_repository(from: &Path, to: &Path, staged: &Path) -> Result<()> {
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
    let copied = super::storage::copy_verified(from, staged)
        .and_then(|()| std::fs::rename(staged, to).map_err(Into::into));
    if let Err(error) = copied {
        let _ = std::fs::remove_dir_all(staged);
        return Err(crate::error::anyhow!(
            "Could not copy {} to {}: {error}",
            from.display(),
            to.display()
        ));
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

/// A waiting move for `session` to carry out, as a request would leave it.
#[cfg(test)]
pub(crate) fn ask(
    store: &Store,
    session: &str,
    name: &str,
    path: &str,
    proposal: Option<&str>,
) -> ChatProjectMove {
    store
        .set_chat_project_move(session, name, path, proposal, None)
        .unwrap();
    store.chat_project_move(session).unwrap().unwrap()
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
    fn naming_a_scratch_project_keeps_it_from_the_next_new_chat() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();

        let named = scratch_project(&store).unwrap();
        assert!(named.is_unnamed_scratch());
        store
            .edit_local_project(&named.id, Some("Tokenizer probes"), None, None)
            .unwrap();

        let next = scratch_project(&store).unwrap();
        assert_ne!(next.id, named.id);
        assert!(next.is_unnamed_scratch());
        let kept = store.get_local_project(&named.id).unwrap().unwrap();
        assert!(kept.is_scratch() && !kept.is_unnamed_scratch());

        let _ = std::fs::remove_dir_all(&named.repo_path);
        let _ = std::fs::remove_dir_all(&next.repo_path);
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
            "owner",
            Some(&worktree),
            &ask(
                &store,
                "owner",
                "Fibonacci sandbox",
                target.to_str().unwrap(),
                Some("toolu_create"),
            ),
        )
        .unwrap();

        assert!(!project.is_scratch());
        assert_eq!(project.id, scratch.id);
        assert_eq!(project.name, "Fibonacci sandbox");
        assert_eq!(
            store
                .get_local_project(&project.id)
                .unwrap()
                .unwrap()
                .proposal,
            Some("toolu_create".into())
        );
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

        let again = promote_scratch_project(
            &store,
            &project,
            "owner",
            None,
            &ask(&store, "owner", "Other", "/nowhere", None),
        );
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

        let error = promote_scratch_project(
            &store,
            &scratch,
            "owner",
            None,
            &ask(&store, "owner", "Mine", occupied.to_str().unwrap(), None),
        )
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
    fn creating_a_project_needs_a_full_path() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let scratch = scratch_project(&store).unwrap();

        for path in ["probes", "$HOME/probes", "~probes"] {
            let error = promote_scratch_project(
                &store,
                &scratch,
                "owner",
                None,
                &ask(&store, "owner", "Probes", path, None),
            )
            .err()
            .unwrap()
            .to_string();
            assert!(error.contains("not a full path"), "{path}: {error}");
        }
        assert!(Path::new(&scratch.repo_path).join(".git").is_dir());
        let _ = std::fs::remove_dir_all(&scratch.repo_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn creating_a_project_that_cannot_be_saved_leaves_the_chat_where_it_was() {
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
        store.conn_execute_for_tests(
            "CREATE TRIGGER fail_promotion BEFORE UPDATE OF created_at ON local_projects
             BEGIN SELECT RAISE(FAIL, 'disk I/O error'); END",
        );
        let target = root.join("projects").join("fib");

        let error = promote_scratch_project(
            &store,
            &scratch,
            "owner",
            Some(&worktree),
            &ask(
                &store,
                "owner",
                "Fibonacci sandbox",
                target.to_str().unwrap(),
                Some("toolu_create"),
            ),
        )
        .err()
        .unwrap()
        .to_string();

        assert!(error.contains("disk I/O error"), "{error}");
        assert!(Path::new(&scratch.repo_path).join(".git").is_dir());
        assert!(!target.exists());
        let kept = store.get_local_project(&scratch.id).unwrap().unwrap();
        assert_eq!(kept.repo_path, scratch.repo_path);
        assert_eq!(kept.name, scratch.name);
        assert_eq!(kept.created_at, scratch.created_at);
        assert_eq!(kept.proposal, None);
        assert_eq!(
            git_output(&worktree, &["rev-parse", "HEAD"]),
            git_output(Path::new(&scratch.repo_path), &["rev-parse", "HEAD"])
        );
        let _ = std::fs::remove_dir_all(&scratch.repo_path);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A scratch project with a chat worktree, as a crash would find it.
    fn scratch_with_worktree(root: &Path, store: &Store) -> (LocalProject, PathBuf) {
        let scratch = scratch_project(store).unwrap();
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
        (scratch, worktree)
    }

    fn promotion(store: &Store, scratch: &LocalProject, target: &Path) -> MoveInFlight {
        let path = target.to_string_lossy();
        MoveInFlight::Promote {
            requester: "owner".into(),
            request: ask(store, "owner", "Probes", &path, Some("toolu_create")),
            source: PathBuf::from(&scratch.repo_path),
            target: target.to_path_buf(),
            tip: git_output(Path::new(&scratch.repo_path), &["rev-parse", "HEAD"]),
        }
    }

    #[test]
    fn a_start_after_a_crash_finishes_a_promotion_whose_repository_moved() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let (scratch, worktree) = scratch_with_worktree(&root, &store);
        let target = root.join("projects").join("probes");
        begin_move(&store, &scratch.id, &promotion(&store, &scratch, &target)).unwrap();
        // Another try before the start that recovers it would lose what it needs.
        let again = begin_move(&store, &scratch.id, &promotion(&store, &scratch, &target));
        assert!(again.unwrap_err().to_string().contains("didn't finish"));
        store.take_chat_project_move("owner", None).unwrap();
        let source = PathBuf::from(&scratch.repo_path);
        std::fs::write(ownership_mark(&source, &scratch.id), "").unwrap();
        move_repository(&source, &target, &staged_copy(&target, &scratch.id)).unwrap();

        recover_project_moves(&store);

        let project = store.get_local_project(&scratch.id).unwrap().unwrap();
        assert_eq!(project.name, "Probes");
        assert_eq!(project.proposal.as_deref(), Some("toolu_create"));
        assert_eq!(
            Path::new(&project.repo_path),
            crate::paths::canonicalize(&target).unwrap()
        );
        assert_eq!(
            git_output(&worktree, &["rev-parse", "HEAD"]),
            git_output(&target, &["rev-parse", "HEAD"])
        );
        assert!(store.project_moves_in_flight().unwrap().is_empty());
        assert_eq!(store.chat_project_move("owner").unwrap(), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_start_after_a_crash_undoes_a_copy_and_asks_for_the_move_again() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let (scratch, worktree) = scratch_with_worktree(&root, &store);
        let target = root.join("projects").join("probes");
        begin_move(&store, &scratch.id, &promotion(&store, &scratch, &target)).unwrap();
        let source = PathBuf::from(&scratch.repo_path);
        std::fs::write(ownership_mark(&source, &scratch.id), "").unwrap();
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        // Copied across volumes and repaired there, cut short before the store named it.
        crate::local::storage::copy_verified(&source, &target).unwrap();
        run_git(&target, &["worktree", "repair"]);
        crate::local::storage::copy_verified(&source, &staged_copy(&target, &scratch.id)).unwrap();

        recover_project_moves(&store);

        assert!(!target.exists());
        assert!(!staged_copy(&target, &scratch.id).exists());
        assert!(store
            .get_local_project(&scratch.id)
            .unwrap()
            .unwrap()
            .is_scratch());
        assert_eq!(
            git_output(&worktree, &["rev-parse", "HEAD"]),
            git_output(&source, &["rev-parse", "HEAD"])
        );
        let waiting = store.chat_project_move("owner").unwrap().unwrap();
        assert_eq!(
            (waiting.name.as_str(), Path::new(&waiting.path)),
            ("Probes", target.as_path())
        );
        assert_eq!(waiting.proposal.as_deref(), Some("toolu_create"));
        assert!(store.project_moves_in_flight().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&scratch.repo_path);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_start_after_a_crash_puts_back_a_chat_moved_into_an_unrecorded_project() {
        // Cut short with the chat's files moved, after they went back but before their
        // link did (a recovery itself cut short), and with only Git's registration made.
        for cut in ["relinked", "put back", "registered"] {
            split_cut_short(cut);
        }
    }

    fn split_cut_short(cut: &str) {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let source = root.join("source");
        initialized(&source);
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
        std::fs::write(worktree.join("draft.md"), "unsaved\n").unwrap();
        let target = root.join("split");
        let id = uuid::Uuid::new_v4().to_string();
        let moved = git::session_worktree_path(&id, &session);
        begin_move(
            &store,
            &id,
            &MoveInFlight::Split {
                requester: session.clone(),
                request: ask(
                    &store,
                    &session,
                    "Probe",
                    &target.to_string_lossy(),
                    Some("toolu_split"),
                ),
                source_repo: source.clone(),
                target: target.clone(),
                worktree: worktree.clone(),
                moved: moved.clone(),
                old_link: Some(std::fs::read_to_string(worktree.join(".git")).unwrap()),
            },
        )
        .unwrap();
        // Seeded and relinked, cut short before the store recorded the project.
        std::fs::create_dir_all(&target).unwrap();
        run_git(&target, &["init", "--quiet", "-b", "main"]);
        std::fs::write(ownership_mark(&target, &id), "").unwrap();
        run_git(
            &source,
            &[
                "push",
                "--quiet",
                target.to_str().unwrap(),
                "main:refs/heads/side",
            ],
        );
        run_git(&target, &["branch", "-m", "side", "main"]);
        let head = git_output(&worktree, &["rev-parse", "HEAD"]);
        if cut == "registered" {
            run_git(
                &target,
                &[
                    "worktree",
                    "add",
                    "--no-checkout",
                    "--detach",
                    moved.to_str().unwrap(),
                    &head,
                ],
            );
        } else {
            std::mem::forget(relink_worktree(&worktree, &moved, &target).unwrap());
        }
        if cut == "put back" {
            std::fs::rename(&moved, &worktree).unwrap();
        }

        recover_project_moves(&store);

        assert!(!moved.exists(), "{cut}");
        assert!(!target.exists(), "{cut}");
        assert_eq!(
            std::fs::read_to_string(worktree.join("draft.md")).unwrap(),
            "unsaved\n"
        );
        assert_eq!(
            git_output(&worktree, &["rev-parse", "HEAD"]),
            git_output(&source, &["rev-parse", "HEAD"])
        );
        assert!(store.get_local_project(&id).unwrap().is_none());
        assert!(store.project_moves_in_flight().unwrap().is_empty(), "{cut}");
        let waiting = store.chat_project_move(&session).unwrap().unwrap();
        assert_eq!(waiting.name, "Probe");
        assert_eq!(waiting.proposal.as_deref(), Some("toolu_split"));
        let _ = std::fs::remove_dir_all(worktree.parent().unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn undoing_a_promotion_leaves_a_folder_another_move_filled() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let first = scratch_project(&store).unwrap();
        store
            .edit_local_project(&first.id, Some("First"), None, None)
            .unwrap();
        let second = scratch_project(&store).unwrap();
        assert_ne!(first.id, second.id);
        // Both checked the same empty folder; the first got there first.
        let target = root.join("projects").join("shared");
        let path = target.to_string_lossy();
        let kept = promote_scratch_project(
            &store,
            &first,
            "first",
            None,
            &ask(&store, "first", "First", &path, None),
        )
        .unwrap();
        let tip = git_output(Path::new(&second.repo_path), &["rev-parse", "HEAD"]);

        undo_promotion(Path::new(&second.repo_path), &target, &second.id, &tip).unwrap();

        assert!(target.join(".git").is_dir());
        assert_eq!(
            Path::new(&kept.repo_path),
            crate::paths::canonicalize(&target).unwrap()
        );
        assert!(Path::new(&second.repo_path).join(".git").is_dir());
        let _ = std::fs::remove_dir_all(&second.repo_path);
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
            &session,
            std::slice::from_ref(&session),
            &ask(
                &store,
                &session,
                "Probe",
                root.join("staged").to_str().unwrap(),
                None,
            ),
        );
        assert!(staged.err().unwrap().to_string().contains("staged changes"));
        assert!(!root.join("staged").exists());
        run_git(&worktree, &["reset", "--quiet"]);

        // A store that can't record the project puts the chat's files back as they were.
        store.conn_execute_for_tests(
            "CREATE TRIGGER fail_split BEFORE INSERT ON local_projects
             BEGIN SELECT RAISE(FAIL, 'disk I/O error'); END",
        );
        let unsaved = split_chat_project(
            &store,
            &project,
            &session,
            &session,
            std::slice::from_ref(&session),
            &ask(
                &store,
                &session,
                "Probe",
                root.join("unsaved").to_str().unwrap(),
                None,
            ),
        );
        assert!(unsaved
            .err()
            .unwrap()
            .to_string()
            .contains("disk I/O error"));
        assert!(!root.join("unsaved").exists());
        assert_eq!(
            std::fs::read_to_string(worktree.join("README.md")).unwrap(),
            "# edited\n"
        );
        assert_eq!(
            git_output(&worktree, &["status", "--porcelain"]),
            "M README.md"
        );
        store.conn_execute_for_tests("DROP TRIGGER fail_split");

        let target = root.join("split");
        let created = split_chat_project(
            &store,
            &project,
            &session,
            &session,
            std::slice::from_ref(&session),
            &ask(
                &store,
                &session,
                "Probe",
                target.to_str().unwrap(),
                Some(""),
            ),
        )
        .unwrap();

        assert_ne!(created.id, project.id);
        assert!(!created.is_scratch());
        assert_eq!(
            store
                .get_local_project(&created.id)
                .unwrap()
                .unwrap()
                .proposal,
            Some(String::new())
        );
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
