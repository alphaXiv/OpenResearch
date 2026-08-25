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
        let home = dirs::home_dir()
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
        Ok(std::env::current_dir()?.join(path))
    }
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
    if require_new_folder && !path.exists() {
        let existing_parent = path
            .parent()
            .and_then(|parent| parent.ancestors().find(|ancestor| ancestor.is_dir()));
        if let Some(parent) = existing_parent {
            if git::repository_state(parent) != git::RepositoryState::NotRepository {
                return Err(crate::error::anyhow!(
                    "{} is already inside a Git repository; a blank project needs a folder of its own",
                    path.display()
                ));
            }
        }
    }
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

    let repository_state = git::repository_state(&path);
    if repository_state == git::RepositoryState::Invalid {
        return Err(crate::error::anyhow!(
            "{} is not a valid Git repository",
            path.display()
        ));
    }
    // Seeding an existing repository would leave the paper uncommitted, and in a
    // subdirectory it would land outside the project root.
    if paper_pdf.is_some() && repository_state != git::RepositoryState::NotRepository {
        return Err(crate::error::anyhow!(
            "{} is already inside a Git repository; a paper project needs a folder of its own",
            path.display()
        ));
    }
    if require_new_folder && repository_state != git::RepositoryState::NotRepository {
        return Err(crate::error::anyhow!(
            "{} is already inside a Git repository; a blank project needs a folder of its own",
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
        git::initialize_repository(&path)?;
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
        created_at: now,
        updated_at: now,
    };
    store.create_local_project(&project)?;
    Ok(project)
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
    use std::process::Command;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("orx-local-project-{}", uuid::Uuid::new_v4()))
    }

    fn run_git(path: &Path, args: &[&str]) {
        let status = Command::new("git")
            .current_dir(path)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {}", args.join(" "));
    }

    fn git_output(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
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
        std::fs::remove_dir_all(root).unwrap();
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
            std::fs::canonicalize(&project_path).unwrap()
        );
        assert!(git::remotes(&project_path).unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
            std::fs::canonicalize(project_path).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_blank_project_nested_in_an_existing_repository() {
        let root = root();
        let repository = root.join("repository");
        initialized(&repository);
        let project_path = repository.join("blank-project");
        let store = Store::open_at(root.join("data")).unwrap();

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

        assert!(error.to_string().contains("folder of its own"));
        assert!(store.list_local_projects().unwrap().is_empty());
        assert!(!project_path.exists());
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refuses_to_seed_a_paper_project_inside_an_existing_repository() {
        let root = root();
        let store = Store::open_at(root.join("data")).unwrap();
        let repository = root.join("repository");
        initialized(&repository);
        let nested = repository.join("nested");
        let error = paper_project(&store, &nested).unwrap_err().to_string();
        assert!(error.contains("already inside a Git repository"), "{error}");
        assert!(!nested.join(PAPER_PDF_NAME).exists());

        let error = paper_project(&store, &repository).unwrap_err().to_string();
        assert!(error.contains("must be empty"), "{error}");
        assert!(!repository.join(PAPER_PDF_NAME).exists());
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn managed_initial_commit_bypasses_signing_and_hooks() {
        use std::os::unix::fs::PermissionsExt;

        let root = root();
        let project_path = root.join("project");
        std::fs::create_dir_all(&project_path).unwrap();
        run_git(&project_path, &["init", "-b", "main"]);
        run_git(&project_path, &["config", "commit.gpgsign", "true"]);
        run_git(&project_path, &["config", "gpg.program", "false"]);
        std::fs::write(project_path.join("README.md"), "# test\n").unwrap();
        let hook = project_path.join(".git/hooks/post-commit");
        std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&hook, permissions).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
            std::fs::canonicalize(&dirty).unwrap()
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
        std::fs::remove_dir_all(root).unwrap();
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
            std::fs::canonicalize(project_path).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
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
            std::fs::canonicalize(&project_path).unwrap()
        );
        assert!(project_path.join(".gitignore").exists());
        assert_eq!(
            git_output(&project_path, &["ls-tree", "-r", "--name-only", "HEAD"]),
            ".gitignore\nREADME.md"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
