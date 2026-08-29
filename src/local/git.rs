//! Git operations for local mode — shell out to the `git` binary (already a
//! hard dependency of the workflow; no libgit2). Clones live at
//! `~/.cache/openresearch/repos/<owner>/<repo>`, the same convention SKILL.md
//! documents for manual diffing.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, process::CommandExt};

use crate::error::{anyhow, Result};

pub const GITHUB_REMOTE: &str = "github";
pub const INITIAL_SNAPSHOT_MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
pub const INITIAL_SNAPSHOT_MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

const MANAGED_IGNORE_START: &str = "# >>> OpenResearch large-file exclusions >>>";
const MANAGED_IGNORE_END: &str = "# <<< OpenResearch large-file exclusions <<<";
const PROJECT_TOO_LARGE: &str = "This project is too large to import. After excluding files 50 MB or larger, the remaining files exceed OpenResearch's 1 GB limit.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryState {
    NotRepository,
    Unborn,
    Ready,
    Detached,
    Invalid,
}

impl RepositoryState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRepository => "notRepository",
            Self::Unborn => "unborn",
            Self::Ready => "ready",
            Self::Detached => "detached",
            Self::Invalid => "invalid",
        }
    }

    pub fn is_initialized(self) -> bool {
        matches!(self, Self::Unborn | Self::Ready | Self::Detached)
    }
}

pub fn clones_root() -> PathBuf {
    cache_root().join("repos")
}

pub fn clone_path(owner: &str, repo: &str) -> PathBuf {
    clones_root().join(owner).join(repo)
}

fn cache_root() -> PathBuf {
    std::env::var_os("ORX_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cache")
                .join("openresearch")
        })
}

/// Root for per-chat-session worktrees of a project repository.
pub fn worktrees_root(project_id: &str) -> PathBuf {
    cache_root().join("worktrees").join(project_id)
}

pub fn session_worktree_path(project_id: &str, session_id: &str) -> PathBuf {
    worktrees_root(project_id).join(session_id)
}

fn legacy_worktrees_root(owner: &str, repo: &str) -> PathBuf {
    cache_root().join("worktrees").join(owner).join(repo)
}

fn legacy_session_worktree_path(owner: &str, repo: &str, session_id: &str) -> PathBuf {
    legacy_worktrees_root(owner, repo).join(session_id)
}

pub fn migrate_legacy_project_worktrees(
    project: &crate::local::model::LocalProject,
    session_ids: &[String],
) -> Result<()> {
    if !project.has_github_repository() {
        return Ok(());
    }
    let legacy_root = legacy_worktrees_root(&project.github_owner, &project.github_repo);
    if !legacy_root.is_dir() {
        return Ok(());
    }
    let current_root = worktrees_root(&project.id);
    std::fs::create_dir_all(&current_root)?;
    for session_id in session_ids {
        let source = legacy_root.join(session_id);
        if !source.is_dir() {
            continue;
        }
        let target = current_root.join(session_id);
        if target.exists() {
            continue;
        }
        let source = source
            .to_str()
            .ok_or_else(|| anyhow!("Legacy worktree path is not valid UTF-8."))?;
        let target = target
            .to_str()
            .ok_or_else(|| anyhow!("Project worktree path is not valid UTF-8."))?;
        git(
            Some(Path::new(&project.repo_path)),
            &["worktree", "move", source, target],
        )?;
    }
    Ok(())
}

pub fn existing_session_worktree_path(
    project: &crate::local::model::LocalProject,
    session_id: &str,
) -> PathBuf {
    let current = session_worktree_path(&project.id, session_id);
    if current.exists() || !project.has_github_repository() {
        return current;
    }
    let legacy =
        legacy_session_worktree_path(&project.github_owner, &project.github_repo, session_id);
    if legacy.exists() {
        legacy
    } else {
        current
    }
}

/// Run git with `args`, returning trimmed stdout; failures carry git's stderr.
/// Headless: git must fail fast rather than prompt on /dev/tty (these calls
/// run under a server, where a prompt would hang a worker forever).
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes");
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn version() -> Option<String> {
    git(None, &["--version"]).ok()
}

pub fn is_repository(path: &Path) -> bool {
    git(Some(path), &["rev-parse", "--is-inside-work-tree"])
        .map(|value| value == "true")
        .unwrap_or(false)
}

pub fn repository_state(path: &Path) -> RepositoryState {
    if !is_repository(path) {
        return if path.join(".git").exists() || git(Some(path), &["rev-parse", "--git-dir"]).is_ok()
        {
            RepositoryState::Invalid
        } else {
            RepositoryState::NotRepository
        };
    }

    let has_head = git(Some(path), &["rev-parse", "--verify", "HEAD"]).is_ok();
    let has_branch = git(Some(path), &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .is_ok_and(|branch| !branch.is_empty());
    match (has_head, has_branch) {
        (true, true) => RepositoryState::Ready,
        (true, false) => RepositoryState::Detached,
        (false, true) => RepositoryState::Unborn,
        (false, false) => RepositoryState::Invalid,
    }
}

pub fn repository_root(path: &Path) -> Result<PathBuf> {
    let root = git(Some(path), &["rev-parse", "--show-toplevel"])?;
    std::fs::canonicalize(root).map_err(Into::into)
}

pub fn common_git_dir(path: &Path) -> Result<PathBuf> {
    let value = git(Some(path), &["rev-parse", "--git-common-dir"])?;
    let value = PathBuf::from(value);
    let resolved = if value.is_absolute() {
        value
    } else {
        path.join(value)
    };
    std::fs::canonicalize(resolved).map_err(Into::into)
}

pub(crate) struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    pub(crate) fn new(prefix: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn repository_git_dir(path: &Path) -> Result<PathBuf> {
    let value = PathBuf::from(git(Some(path), &["rev-parse", "--git-dir"])?);
    let resolved = if value.is_absolute() {
        value
    } else {
        path.join(value)
    };
    std::fs::canonicalize(resolved).map_err(Into::into)
}

fn git_context_bytes(
    work_tree: &Path,
    git_dir: &Path,
    index_file: &Path,
    args: &[&str],
) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .current_dir(work_tree)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_DIR", git_dir)
        .env("GIT_WORK_TREE", work_tree)
        .env("GIT_INDEX_FILE", index_file)
        .args(args)
        .output()
        .map_err(|error| anyhow!("Could not run git: {error}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn scan_git_dir(work_tree: &Path, operation: &TemporaryDirectory) -> Result<PathBuf> {
    if is_repository(work_tree) {
        return repository_git_dir(work_tree);
    }
    let git_dir = operation.0.join("scan.git");
    let git_dir_arg = git_dir
        .to_str()
        .ok_or_else(|| anyhow!("Temporary Git path is not valid UTF-8."))?;
    git(None, &["init", "--bare", git_dir_arg])?;
    Ok(git_dir)
}

#[derive(Debug)]
struct InitialSnapshot {
    excluded_paths: Vec<Vec<u8>>,
    included_paths: Vec<Vec<u8>>,
    included_bytes: u64,
}

#[cfg(unix)]
fn path_from_git_bytes(path: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(path)))
}

#[cfg(not(unix))]
fn path_from_git_bytes(path: &[u8]) -> Result<PathBuf> {
    Ok(PathBuf::from(std::str::from_utf8(path).map_err(|_| {
        anyhow!("A filename that is not valid UTF-8 cannot be imported safely.")
    })?))
}

fn initial_snapshot(path: &Path) -> Result<InitialSnapshot> {
    let operation = TemporaryDirectory::new("orx-initial-snapshot-scan")?;
    let git_dir = scan_git_dir(path, &operation)?;
    let index_file = operation.0.join("index");
    git_context_bytes(path, &git_dir, &index_file, &["read-tree", "--empty"])?;
    let bytes = git_context_bytes(
        path,
        &git_dir,
        &index_file,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut excluded_paths = Vec::new();
    let mut included_paths = Vec::new();
    let mut included_bytes = 0_u64;
    for relative in bytes.split(|byte| *byte == 0).filter(|raw| !raw.is_empty()) {
        if relative.ends_with(b"/") {
            let nested = path.join(path_from_git_bytes(&relative[..relative.len() - 1])?);
            if matches!(
                repository_state(&nested),
                RepositoryState::Ready | RepositoryState::Detached
            ) {
                included_paths.push(relative.to_vec());
            } else {
                excluded_paths.push(relative.to_vec());
            }
            continue;
        }
        let file = path.join(path_from_git_bytes(relative)?);
        let metadata = std::fs::symlink_metadata(&file)?;
        let size = if metadata.file_type().is_symlink() {
            std::fs::read_link(&file)?
                .as_os_str()
                .to_string_lossy()
                .len() as u64
        } else if metadata.is_file() {
            metadata.len()
        } else {
            continue;
        };
        if metadata.is_file() && size >= INITIAL_SNAPSHOT_MAX_FILE_BYTES {
            excluded_paths.push(relative.to_vec());
        } else {
            included_paths.push(relative.to_vec());
            included_bytes = included_bytes
                .checked_add(size)
                .ok_or_else(|| anyhow!(PROJECT_TOO_LARGE))?;
        }
    }
    excluded_paths.sort();
    included_paths.sort();
    Ok(InitialSnapshot {
        excluded_paths,
        included_paths,
        included_bytes,
    })
}

fn escaped_gitignore_path(path: &[u8]) -> Result<Vec<u8>> {
    if path.contains(&b'\n') || path.contains(&b'\r') {
        return Err(anyhow!(
            "The file {:?} has a newline in its name and cannot be excluded automatically.",
            String::from_utf8_lossy(path)
        ));
    }
    let mut escaped = vec![b'/'];
    for byte in path {
        if matches!(
            *byte,
            b'\\' | b'!' | b'#' | b'[' | b']' | b'*' | b'?' | b' '
        ) {
            escaped.push(b'\\');
        }
        escaped.push(*byte);
    }
    Ok(escaped)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn include_following_newline(contents: &[u8], index: usize) -> usize {
    if contents.get(index) == Some(&b'\r') && contents.get(index + 1) == Some(&b'\n') {
        index + 2
    } else if contents.get(index) == Some(&b'\n') {
        index + 1
    } else {
        index
    }
}

fn managed_gitignore(existing: &[u8], excluded_paths: &[Vec<u8>]) -> Result<Vec<u8>> {
    let mut updated = existing.to_vec();
    let start = MANAGED_IGNORE_START.as_bytes();
    let end = MANAGED_IGNORE_END.as_bytes();
    while let Some(start_index) = find_bytes(&updated, start) {
        let after_start = start_index + start.len();
        let remove_end = match find_bytes(&updated[after_start..], end) {
            Some(index) => include_following_newline(&updated, after_start + index + end.len()),
            None => include_following_newline(&updated, after_start),
        };
        updated.drain(start_index..remove_end);
    }
    if excluded_paths.is_empty() {
        return Ok(updated);
    }
    if !updated.is_empty() && !updated.ends_with(b"\n") {
        updated.push(b'\n');
    }
    updated.extend_from_slice(MANAGED_IGNORE_START.as_bytes());
    updated.push(b'\n');
    for path in excluded_paths {
        updated.extend_from_slice(&escaped_gitignore_path(path)?);
        updated.push(b'\n');
    }
    updated.extend_from_slice(MANAGED_IGNORE_END.as_bytes());
    updated.push(b'\n');
    Ok(updated)
}

#[derive(Debug)]
enum FileBackup {
    Missing,
    Contents(Vec<u8>),
}

fn file_backup(path: &Path) -> Result<FileBackup> {
    match std::fs::read(path) {
        Ok(contents) => Ok(FileBackup::Contents(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileBackup::Missing),
        Err(error) => Err(error.into()),
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", path.display()))?;
    let temporary = parent.join(format!(".openresearch-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents)?;
        file.sync_all()?;
        if let Ok(metadata) = std::fs::metadata(path) {
            std::fs::set_permissions(&temporary, metadata.permissions())?;
        }
        if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
            return Err(anyhow!(
                "Refusing to replace the symlink at {}.",
                path.display()
            ));
        }
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn restore_file(path: &Path, backup: &FileBackup) -> Result<()> {
    match backup {
        FileBackup::Missing => {
            if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_symlink()) {
                return Err(anyhow!(
                    "Refusing to remove the symlink at {} during rollback.",
                    path.display()
                ));
            }
            match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
        FileBackup::Contents(contents) => atomic_write(path, contents),
    }
}

fn replace_managed_ignore_file(path: &Path, excluded_paths: &[Vec<u8>]) -> Result<()> {
    let existing = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    atomic_write(path, &managed_gitignore(&existing, excluded_paths)?)
}

fn replace_managed_gitignore(path: &Path, excluded_paths: &[Vec<u8>]) -> Result<()> {
    replace_managed_ignore_file(&path.join(".gitignore"), excluded_paths)
}

struct PreparedInitialSnapshot {
    gitignore_backup: Option<FileBackup>,
    excluded_paths: Vec<Vec<u8>>,
    included_paths: Vec<Vec<u8>>,
    write_git_exclude: bool,
}

fn prepare_initial_snapshot(path: &Path) -> Result<PreparedInitialSnapshot> {
    let snapshot = initial_snapshot(path)?;
    if snapshot.included_bytes >= INITIAL_SNAPSHOT_MAX_TOTAL_BYTES {
        return Err(anyhow!(PROJECT_TOO_LARGE));
    }
    if snapshot.excluded_paths.is_empty() {
        return Ok(PreparedInitialSnapshot {
            gitignore_backup: None,
            excluded_paths: Vec::new(),
            included_paths: snapshot.included_paths,
            write_git_exclude: false,
        });
    }

    let gitignore = path.join(".gitignore");
    if std::fs::symlink_metadata(&gitignore).is_ok_and(|metadata| metadata.is_symlink()) {
        let verified = initial_snapshot(path)?;
        if verified.included_bytes >= INITIAL_SNAPSHOT_MAX_TOTAL_BYTES {
            return Err(anyhow!(PROJECT_TOO_LARGE));
        }
        return Ok(PreparedInitialSnapshot {
            gitignore_backup: None,
            excluded_paths: verified.excluded_paths,
            included_paths: verified.included_paths,
            write_git_exclude: true,
        });
    }
    let backup = file_backup(&gitignore)?;
    let mut excluded_paths = snapshot.excluded_paths;
    let result = (|| -> Result<Vec<Vec<u8>>> {
        loop {
            replace_managed_gitignore(path, &excluded_paths)?;
            let updated = initial_snapshot(path)?;
            if updated.included_bytes >= INITIAL_SNAPSHOT_MAX_TOTAL_BYTES {
                return Err(anyhow!(PROJECT_TOO_LARGE));
            }
            let previous_count = excluded_paths.len();
            for excluded in updated.excluded_paths {
                if !excluded_paths.contains(&excluded) {
                    excluded_paths.push(excluded);
                }
            }
            if excluded_paths.len() == previous_count {
                return Ok(updated.included_paths);
            }
            excluded_paths.sort();
        }
    })();
    let included_paths = match result {
        Ok(included_paths) => included_paths,
        Err(error) => {
            if let Err(restore_error) = restore_file(&gitignore, &backup) {
                return Err(anyhow!(
                    "{error}; additionally failed to restore {}: {restore_error}",
                    gitignore.display()
                ));
            }
            return Err(error);
        }
    };
    Ok(PreparedInitialSnapshot {
        gitignore_backup: Some(backup),
        excluded_paths,
        included_paths,
        write_git_exclude: false,
    })
}

fn repository_index_path(path: &Path) -> Result<PathBuf> {
    let value = PathBuf::from(git(Some(path), &["rev-parse", "--git-path", "index"])?);
    Ok(if value.is_absolute() {
        value
    } else {
        path.join(value)
    })
}

fn repository_exclude_path(path: &Path) -> Result<PathBuf> {
    let value = PathBuf::from(git(
        Some(path),
        &["rev-parse", "--git-path", "info/exclude"],
    )?);
    Ok(if value.is_absolute() {
        value
    } else {
        path.join(value)
    })
}

fn stage_initial_snapshot(path: &Path, included_paths: &[Vec<u8>]) -> Result<()> {
    if included_paths.is_empty() {
        return Ok(());
    }

    let mut child = Command::new("git")
        .current_dir(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["add", "-f", "--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow!("Could not run git: {error}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Could not open git add input."))?;
    for included in included_paths {
        stdin.write_all(b":(top,literal)")?;
        stdin.write_all(included)?;
        stdin.write_all(b"\0")?;
    }
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| anyhow!("Could not run git: {error}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "Could not stage the initial project snapshot: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn remove_excluded_paths_from_index(path: &Path, excluded_paths: &[Vec<u8>]) -> Result<()> {
    for paths in excluded_paths.chunks(128) {
        let mut command = Command::new("git");
        command
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "--literal-pathspecs",
                "update-index",
                "--force-remove",
                "--",
            ]);
        for path in paths {
            command.arg(path_from_git_bytes(path)?);
        }
        let output = command
            .output()
            .map_err(|error| anyhow!("Could not run git: {error}"))?;
        if !output.status.success() {
            return Err(anyhow!(
                "Could not exclude large files from the initial commit: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(())
}

fn create_initial_commit(
    path: &Path,
    included_paths: &[Vec<u8>],
    excluded_paths: &[Vec<u8>],
) -> Result<()> {
    let operation = TemporaryDirectory::new("orx-initial-snapshot-commit")?;
    let hooks_dir = operation.0.join("hooks");
    std::fs::create_dir(&hooks_dir)?;
    let index_path = repository_index_path(path)?;
    let index_backup = file_backup(&index_path)?;
    let hooks_config = format!("core.hooksPath={}", hooks_dir.display());
    let result = (|| -> Result<()> {
        git(Some(path), &["read-tree", "--empty"])?;
        stage_initial_snapshot(path, included_paths)?;
        remove_excluded_paths_from_index(path, excluded_paths)?;
        git(
            Some(path),
            &[
                "-c",
                &hooks_config,
                "-c",
                "commit.gpgSign=false",
                "-c",
                "user.name=OpenResearch",
                "-c",
                "user.email=local@openresearch.sh",
                "commit",
                "--no-verify",
                "--no-gpg-sign",
                "--allow-empty",
                "-m",
                "Initialize OpenResearch project",
            ],
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(restore_error) = restore_file(&index_path, &index_backup) {
            return Err(anyhow!(
                "{error}; additionally failed to restore the Git index: {restore_error}"
            ));
        }
        return Err(error);
    }
    Ok(())
}

pub fn initialize_repository(path: &Path) -> Result<()> {
    let state = repository_state(path);
    if state == RepositoryState::Ready {
        return Ok(());
    }
    if state == RepositoryState::Detached {
        return Err(anyhow!(
            "The repository is on a detached HEAD. Check out a branch first."
        ));
    }
    if state == RepositoryState::Invalid {
        return Err(anyhow!("{} is not a valid Git repository", path.display()));
    }

    let root = if state == RepositoryState::Unborn {
        repository_root(path)?
    } else {
        std::fs::canonicalize(path)?
    };
    let snapshot = prepare_initial_snapshot(&root)?;

    let initialized_here = state == RepositoryState::NotRepository;
    let mut created_git_dir = None;
    let mut git_exclude_backup = None;
    let result = (|| -> Result<()> {
        if initialized_here {
            let git_dir = root.join(".git");
            std::fs::create_dir(&git_dir)?;
            created_git_dir = Some(git_dir);
            git(Some(&root), &["init", "-b", "main"])?;
        }
        if snapshot.write_git_exclude {
            let exclude_path = repository_exclude_path(&root)?;
            let backup = file_backup(&exclude_path)?;
            git_exclude_backup = Some((exclude_path.clone(), backup));
            // Keep these path exclusions durable, matching the managed .gitignore behavior.
            replace_managed_ignore_file(&exclude_path, &snapshot.excluded_paths)?;
        }
        create_initial_commit(&root, &snapshot.included_paths, &snapshot.excluded_paths)
    })();

    if let Err(error) = result {
        let mut cleanup_errors = Vec::new();
        if let Some((exclude_path, backup)) = &git_exclude_backup {
            if let Err(restore_error) = restore_file(exclude_path, backup) {
                cleanup_errors.push(format!(
                    "failed to restore {}: {restore_error}",
                    exclude_path.display()
                ));
            }
        }
        if let Some(backup) = &snapshot.gitignore_backup {
            if let Err(restore_error) = restore_file(&root.join(".gitignore"), backup) {
                cleanup_errors.push(format!(
                    "failed to restore .gitignore, which may still contain OpenResearch exclusions: {restore_error}"
                ));
            }
        }
        if let Some(git_dir) = created_git_dir {
            if let Err(cleanup_error) = std::fs::remove_dir_all(&git_dir) {
                if cleanup_error.kind() != std::io::ErrorKind::NotFound {
                    cleanup_errors.push(format!(
                        "failed to remove the incomplete repository at {}: {cleanup_error}",
                        git_dir.display()
                    ));
                }
            }
        }
        if !cleanup_errors.is_empty() {
            return Err(anyhow!(
                "{error}; additionally {}",
                cleanup_errors.join("; ")
            ));
        }
        return Err(error);
    }
    Ok(())
}

fn public_clone_history_args(shallow: bool) -> &'static [&'static str] {
    if shallow {
        &["--depth=1", "--single-branch"]
    } else {
        &[]
    }
}

pub fn clone_public(url: &str, path: &Path, shallow: bool) -> Result<()> {
    let url = public_clone_url(url)?;
    let empty_config = std::env::temp_dir().join(format!(
        "orx-public-clone-{}.gitconfig",
        uuid::Uuid::new_v4()
    ));
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&empty_config)?;
    let mut command = Command::new("git");
    command
        .current_dir(std::env::temp_dir())
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &empty_config)
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env("GIT_SSH_COMMAND", "false")
        .args(["-c", "credential.helper=", "-c", "core.askPass=", "clone"])
        .args(public_clone_history_args(shallow))
        .arg(&url)
        .arg(path);
    let output = command.output();
    let _ = std::fs::remove_file(&empty_config);
    let out = output.map_err(|error| anyhow!("Could not run git clone: {error}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "Public git clone failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn public_clone_url(url: &str) -> Result<String> {
    let trimmed = url.trim();
    #[cfg(test)]
    if Path::new(trimmed).exists() {
        return Ok(trimmed.to_string());
    }
    if trimmed.contains(['?', '#']) {
        return Err(anyhow!(
            "Paper repository URLs cannot contain a query string or fragment."
        ));
    }
    let rest = trimmed
        .strip_prefix("https://")
        .ok_or_else(|| anyhow!("Paper repositories must use a public https:// URL."))?;
    let (authority, path) = rest
        .split_once('/')
        .ok_or_else(|| anyhow!("Paper repository URL is incomplete."))?;
    if authority.is_empty() || authority.contains('@') || authority.contains(':') || path.is_empty()
    {
        return Err(anyhow!(
            "Paper repository URLs cannot contain credentials or a custom port."
        ));
    }
    Ok(format!("https://{authority}/{path}"))
}

pub fn rename_origin_to_upstream(path: &Path) -> Result<()> {
    if git(Some(path), &["remote", "get-url", "origin"]).is_ok() {
        git(Some(path), &["remote", "rename", "origin", "upstream"])?;
    }
    Ok(())
}

/// Set the URL of a remote, adding it if it does not yet exist.
pub fn set_remote_url(path: &Path, name: &str, url: &str) -> Result<()> {
    if git(Some(path), &["remote", "get-url", name]).is_ok() {
        git(Some(path), &["remote", "set-url", name, url])?;
    } else {
        git(Some(path), &["remote", "add", name, url])?;
    }
    Ok(())
}

pub fn require_current_branch(path: &Path) -> Result<String> {
    let branch = git(Some(path), &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .map_err(|_| anyhow!("The repository is on a detached HEAD. Check out a branch first."))?;
    if branch.trim().is_empty() {
        return Err(anyhow!("The repository has no current branch."));
    }
    Ok(branch)
}

pub fn is_clean(path: &Path) -> Result<bool> {
    Ok(git(Some(path), &["status", "--porcelain"])?.is_empty())
}

pub fn validate_project_repository(path: &Path) -> Result<()> {
    if !is_repository(path) {
        return Err(anyhow!("{} is not a Git repository", path.display()));
    }
    require_current_branch(path)?;
    git(Some(path), &["rev-parse", "--verify", "HEAD"]).map_err(|_| {
        anyhow!("The repository has no commits. Create an initial commit before continuing.")
    })?;
    Ok(())
}

pub fn local_head_sha(path: &Path, branch: &str) -> Result<String> {
    git(
        Some(path),
        &[
            "rev-parse",
            "--verify",
            &format!("refs/heads/{branch}^{{commit}}"),
        ],
    )
}

pub fn remotes(path: &Path) -> Result<Vec<(String, String)>> {
    let out = git(Some(path), &["remote", "-v"])?;
    let mut remotes = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else { continue };
        let Some(url) = parts.next() else { continue };
        if !remotes.iter().any(|(existing, _)| existing == name) {
            remotes.push((name.to_string(), sanitize_remote_url(url)));
        }
    }
    Ok(remotes)
}

fn sanitize_remote_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let Some(separator) = lower.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..separator];
    let rest = &url[separator + 3..];
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .split(['?', '#'])
        .next()
        .unwrap_or(authority);
    if path.is_empty() {
        return format!("{scheme}://{host}");
    }
    let path = path.split(['?', '#']).next().unwrap_or(path);
    format!("{scheme}://{host}/{path}")
}

pub fn github_publication(path: &Path) -> Option<(String, String)> {
    [GITHUB_REMOTE, "origin", "upstream"]
        .into_iter()
        .find_map(|remote| {
            let url = git(Some(path), &["remote", "get-url", remote]).ok()?;
            let (owner, repo) = parse_github_url(&url)?;
            remote_matches_publication(path, remote, &owner, &repo).then_some((owner, repo))
        })
}

fn parse_github_url(url: &str) -> Option<(String, String)> {
    let path = if let Some(path) = url.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = url.strip_prefix("git://github.com/") {
        path
    } else {
        let rest = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))?;
        let (authority, path) = rest.split_once('/')?;
        if authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host)
            != "github.com"
        {
            return None;
        }
        path
    };
    let (owner, repo) = path.trim_end_matches('/').split_once('/')?;
    let repo = repo.trim_end_matches(".git");
    (!owner.is_empty() && !repo.is_empty() && !repo.contains('/'))
        .then(|| (owner.to_string(), repo.to_string()))
}

pub fn github_repository(url: &str) -> Option<(String, String)> {
    parse_github_url(url)
}

fn github_repository_matches(url: &str, owner: &str, repo: &str) -> bool {
    parse_github_url(url).is_some_and(|(remote_owner, remote_repo)| {
        remote_owner.eq_ignore_ascii_case(owner) && remote_repo.eq_ignore_ascii_case(repo)
    })
}

fn remote_matches_publication(repo_path: &Path, remote: &str, owner: &str, repo: &str) -> bool {
    let Ok(fetch_url) = git(Some(repo_path), &["remote", "get-url", remote]) else {
        return false;
    };
    let Ok(push_urls) = git(
        Some(repo_path),
        &["remote", "get-url", "--push", "--all", remote],
    ) else {
        return false;
    };
    github_repository_matches(&fetch_url, owner, repo)
        && !push_urls.is_empty()
        && push_urls
            .lines()
            .all(|url| github_repository_matches(url, owner, repo))
}

fn config_value(path: &Path, scope: &str, key: &str) -> Option<String> {
    git(Some(path), &["config", scope, "--get", key])
        .ok()
        .filter(|value| !value.trim().is_empty())
}

pub fn identity(
    path: &Path,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let local_name = config_value(path, "--local", "user.name");
    let local_email = config_value(path, "--local", "user.email");
    let name = local_name
        .clone()
        .or_else(|| config_value(path, "--global", "user.name"));
    let email = local_email
        .clone()
        .or_else(|| config_value(path, "--global", "user.email"));
    let name_source = name.as_ref().map(|_| {
        if local_name.is_some() {
            "local"
        } else {
            "global"
        }
        .to_string()
    });
    let email_source = email.as_ref().map(|_| {
        if local_email.is_some() {
            "local"
        } else {
            "global"
        }
        .to_string()
    });
    (name, email, name_source, email_source)
}

/// Fail early on a typo'd baseline branch — otherwise it only surfaces much
/// later as an opaque `git push` refspec error on the first run.
fn assert_branch_exists(dir: &Path, owner: &str, repo: &str, branch: &str) -> Result<()> {
    let remote = format!("refs/remotes/origin/{branch}");
    if git(Some(dir), &["rev-parse", "--verify", "--quiet", &remote]).is_err() {
        return Err(anyhow!(
            "Branch '{branch}' not found in {owner}/{repo} — check the project's baseline branch."
        ));
    }
    Ok(())
}

/// The remote's default branch, over git's own credentials (ssh, then https).
/// The GitHub API can't answer this without a token, but a project created by
/// an SSH-only user still needs a baseline that exists — otherwise the clone
/// dies on a hardcoded "main" that a master/dev repo doesn't have.
pub fn remote_default_branch(owner: &str, repo: &str) -> Option<String> {
    for url in [
        format!("git@github.com:{owner}/{repo}.git"),
        format!("https://github.com/{owner}/{repo}.git"),
    ] {
        let Ok(out) = git(None, &["ls-remote", "--symref", &url, "HEAD"]) else {
            continue;
        };
        // "ref: refs/heads/main\tHEAD"
        if let Some(branch) = out.lines().find_map(|l| {
            l.strip_prefix("ref: refs/heads/")
                .and_then(|r| r.split_whitespace().next())
        }) {
            return Some(branch.to_string());
        }
    }
    None
}

/// Clone `owner/repo` into the cache (ssh first, then https) or, when the
/// clone already exists, fetch. Validates that `baseline_branch` exists on
/// the remote. Returns the clone path.
pub fn ensure_clone(owner: &str, repo: &str, baseline_branch: &str) -> Result<PathBuf> {
    let dir = clone_path(owner, repo);
    if dir.join(".git").is_dir() {
        git(Some(&dir), &["fetch", "origin"])?;
        assert_branch_exists(&dir, owner, repo, baseline_branch)?;
        return Ok(dir);
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    if let Some(origin) = super::demo::installed_origin(owner, repo) {
        restore_local_repository(&dir, &origin, baseline_branch)?;
        assert_branch_exists(&dir, owner, repo, baseline_branch)?;
        return Ok(dir);
    }
    let target = dir.to_string_lossy().to_string();
    // Test seam: ORX_GIT_REMOTE_BASE=file:///some/root clones <base>/<owner>/<repo>.
    if let Ok(base) = std::env::var("ORX_GIT_REMOTE_BASE") {
        let url = format!("{}/{owner}/{repo}", base.trim_end_matches('/'));
        git(None, &["clone", &url, &target])?;
        assert_branch_exists(&dir, owner, repo, baseline_branch)?;
        return Ok(dir);
    }
    let ssh = format!("git@github.com:{owner}/{repo}.git");
    let https = format!("https://github.com/{owner}/{repo}.git");
    // ssh covers private repos with keys; https covers public repos and
    // credential-helper setups. Surface the https error (the common path).
    if git(None, &["clone", &ssh, &target]).is_err() {
        if let Err(err) = git(None, &["clone", &https, &target]) {
            return Err(anyhow!(
                "Could not clone {owner}/{repo} (tried ssh and https): {err}"
            ));
        }
    }
    assert_branch_exists(&dir, owner, repo, baseline_branch)?;
    Ok(dir)
}

pub(crate) fn restore_local_repository(
    dir: &Path,
    origin: &Path,
    baseline_branch: &str,
) -> Result<()> {
    if dir.exists() {
        return Err(anyhow!(
            "Refusing to overwrite the invalid repository cache at {}.",
            dir.display()
        ));
    }
    let parent = dir
        .parent()
        .ok_or_else(|| anyhow!("Repository cache path has no parent."))?;
    std::fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(".orx-repository-{}", uuid::Uuid::new_v4()));
    let tmp_arg = tmp.to_string_lossy().into_owned();
    let origin_arg = origin.to_string_lossy().into_owned();
    let result: Result<()> = (|| {
        git(None, &["init", "--quiet", &tmp_arg])?;
        git(Some(&tmp), &["remote", "add", "origin", &origin_arg])?;
        git(
            Some(&tmp),
            &[
                "fetch",
                "--quiet",
                "origin",
                "+refs/heads/*:refs/remotes/origin/*",
            ],
        )?;
        let branches = git(
            Some(&tmp),
            &[
                "for-each-ref",
                "--format=%(refname:strip=3)",
                "refs/remotes/origin",
            ],
        )?;
        for branch in branches.lines().filter(|branch| *branch != "HEAD") {
            git(
                Some(&tmp),
                &[
                    "update-ref",
                    &format!("refs/heads/{branch}"),
                    &format!("refs/remotes/origin/{branch}"),
                ],
            )?;
        }
        git(Some(&tmp), &["checkout", "--quiet", baseline_branch])?;
        std::fs::rename(&tmp, dir)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&tmp);
    }
    result
}

/// Ensure a private worktree of the hub clone for one chat session, so
/// parallel agents on the same project never share (or stomp) a checkout.
/// Worktrees share the hub's object store and refs: a branch created in one
/// is immediately visible in all, one `fetch` updates everyone, and git
/// refuses to check out a branch that another worktree already holds.
///
/// The worktree starts **detached** on the baseline tip — checking out the
/// baseline branch itself would claim it and block every sibling; the agent
/// checks out its own experiment branch from there.
pub fn ensure_session_worktree(
    project: &crate::local::model::LocalProject,
    session_id: &str,
) -> Result<PathBuf> {
    let repo_path = Path::new(&project.repo_path);
    if !is_repository(repo_path) {
        return Err(anyhow!("{} is not a Git repository", repo_path.display()));
    }
    let dir = existing_session_worktree_path(project, session_id);
    let start_ref =
        super::demo::session_start_ref(&project.github_owner, &project.github_repo, session_id)
            .unwrap_or(&project.baseline_branch);
    git(Some(repo_path), &["rev-parse", "--verify", start_ref])?;
    ensure_worktree_from(repo_path, dir, start_ref)
}

pub(crate) fn ensure_session_worktree_in(
    repo: &Path,
    dir: &Path,
    owner: &str,
    repo_name: &str,
    baseline_branch: &str,
    session_id: &str,
) -> Result<PathBuf> {
    let start_ref =
        super::demo::session_start_ref(owner, repo_name, session_id).unwrap_or(baseline_branch);
    ensure_worktree_from(repo, dir.to_path_buf(), start_ref)
}

pub fn ensure_worktree_at(repo: &Path, dir: &Path, start_ref: &str) -> Result<PathBuf> {
    if dir.join(".git").exists() && git(Some(dir), &["rev-parse", "--is-inside-work-tree"]).is_ok()
    {
        let expected = git(Some(repo), &["rev-parse", start_ref])?;
        let actual = git(Some(dir), &["rev-parse", "HEAD"])?;
        let status = git(Some(dir), &["status", "--porcelain"])?;
        if actual != expected || !status.is_empty() {
            return Err(anyhow!(
                "The seeded nanochat worktree at {} is not clean at the expected experiment commit; move it aside and retry onboarding.",
                dir.display()
            ));
        }
    }
    ensure_worktree_from(repo, dir.to_path_buf(), start_ref)
}

fn ensure_worktree_from(repo: &Path, dir: PathBuf, start_ref: &str) -> Result<PathBuf> {
    if dir.join(".git").exists() {
        if git(Some(&dir), &["rev-parse", "--is-inside-work-tree"]).is_ok() {
            return Ok(dir);
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|e| anyhow!("Could not remove stale worktree {}: {}", dir.display(), e))?;
    }
    // A manually deleted worktree dir leaves a stale registration behind that
    // would make `worktree add` at the same path fail.
    let _ = git(Some(repo), &["worktree", "prune"]);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("Could not create {}: {}", parent.display(), e))?;
    }
    let target = dir.to_string_lossy().to_string();
    git(
        Some(repo),
        &["worktree", "add", "--detach", &target, start_ref],
    )?;
    Ok(dir)
}

/// Remove a session's worktree (on session/project delete). Uncommitted
/// scratch is discarded deliberately — real work is committed per the
/// playbook contract. Best-effort: cleanup must never block the delete.
pub fn remove_session_worktree(project: &crate::local::model::LocalProject, session_id: &str) {
    let repo_path = Path::new(&project.repo_path);
    let dir = existing_session_worktree_path(project, session_id);
    if !dir.exists() {
        return;
    }
    if is_repository(repo_path) {
        let _ = git(
            Some(repo_path),
            &["worktree", "remove", "--force", &dir.to_string_lossy()],
        );
        let _ = git(Some(repo_path), &["worktree", "prune"]);
    }
    // Hub gone (cache wiped) or `worktree remove` refused: take the dir anyway.
    let _ = std::fs::remove_dir_all(&dir);
}

/// Seed a fresh (empty) GitHub repo from the tip of another repo — the
/// fork-by-copy the platform does on import. Shallow-clones the source
/// (`src_branch`, or its default branch), re-roots the snapshot as a single
/// orphan commit (a shallow tip's parents aren't in the clone, so pushing it
/// as-is would be rejected), and pushes it as the new repo's `main`.
pub fn seed_copy(
    src_owner: &str,
    src_repo: &str,
    src_branch: Option<&str>,
    dst_owner: &str,
    dst_repo: &str,
) -> Result<()> {
    let tmp = std::env::temp_dir().join(format!("orx-seed-{}", uuid::Uuid::new_v4()));
    let result = seed_copy_in(&tmp, src_owner, src_repo, src_branch, dst_owner, dst_repo);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn seed_copy_in(
    dir: &Path,
    src_owner: &str,
    src_repo: &str,
    src_branch: Option<&str>,
    dst_owner: &str,
    dst_repo: &str,
) -> Result<()> {
    let target = dir.to_string_lossy().to_string();
    let mut args = vec!["clone", "--depth=1", "--single-branch"];
    if let Some(branch) = src_branch {
        args.extend(["--branch", branch]);
    }
    // ssh first, https fallback — same auth order as ensure_clone.
    let ssh = format!("git@github.com:{src_owner}/{src_repo}.git");
    let https = format!("https://github.com/{src_owner}/{src_repo}.git");
    let mut ssh_args = args.clone();
    ssh_args.extend([ssh.as_str(), target.as_str()]);
    if git(None, &ssh_args).is_err() {
        let mut https_args = args;
        https_args.extend([https.as_str(), target.as_str()]);
        if let Err(err) = git(None, &https_args) {
            return Err(anyhow!(
                "Could not clone {src_owner}/{src_repo} (tried ssh and https): {err}"
            ));
        }
    }
    git(Some(dir), &["checkout", "--orphan", "orx-seed"])?;
    git(Some(dir), &["add", "-A"])?;
    // An empty source stages nothing; seed the stub a blank project gets.
    if git(Some(dir), &["status", "--porcelain"])?.is_empty() {
        std::fs::write(dir.join("README.md"), format!("# {dst_repo}\n"))
            .map_err(|e| anyhow!("Could not write README.md: {}", e))?;
        git(Some(dir), &["add", "-A"])?;
    }
    git(
        Some(dir),
        &[
            "-c",
            "user.name=orx",
            "-c",
            "user.email=orx@openresearch.sh",
            "commit",
            "-m",
            &format!("orx: import {src_owner}/{src_repo}"),
        ],
    )?;
    let dst_ssh = format!("git@github.com:{dst_owner}/{dst_repo}.git");
    let dst_https = format!("https://github.com/{dst_owner}/{dst_repo}.git");
    if git(Some(dir), &["push", &dst_ssh, "HEAD:main"]).is_err() {
        git(Some(dir), &["push", &dst_https, "HEAD:main"])?;
    }
    Ok(())
}

pub fn local_branches(repo_path: &Path) -> Result<Vec<String>> {
    Ok(git(
        Some(repo_path),
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?
    .lines()
    .filter(|branch| !branch.is_empty())
    .map(str::to_string)
    .collect())
}

fn publication_remote(repo_path: &Path, owner: &str, repo: &str) -> Result<String> {
    for remote in [GITHUB_REMOTE, "origin", "upstream"] {
        if remote_matches_publication(repo_path, remote, owner, repo) {
            return Ok(remote.to_string());
        }
    }
    Err(anyhow!(
        "This project has no remote matching github.com/{owner}/{repo}. Retry enabling GitHub syncing for this project."
    ))
}

pub fn is_shallow_repository(repo_path: &Path) -> Result<bool> {
    Ok(git(Some(repo_path), &["rev-parse", "--is-shallow-repository"])? == "true")
}

pub fn reroot_shallow_repository(
    repo_path: &Path,
    baseline_branch: &str,
    source: Option<&(String, String)>,
) -> Result<()> {
    if !is_shallow_repository(repo_path)? {
        return Ok(());
    }
    if !is_clean(repo_path)? {
        return Err(anyhow!(
            "Cannot prepare a shallow paper import for publication because the working tree has changes."
        ));
    }
    let temporary = format!("orx-import-{}", uuid::Uuid::new_v4().simple());
    git(Some(repo_path), &["checkout", "--orphan", &temporary])?;
    git(Some(repo_path), &["add", "-A"])?;
    let source_name = source
        .map(|(owner, repo)| format!("{owner}/{repo}"))
        .unwrap_or_else(|| "paper repository".to_string());
    git(
        Some(repo_path),
        &[
            "-c",
            "user.name=OpenResearch",
            "-c",
            "user.email=local@openresearch.sh",
            "commit",
            "--allow-empty",
            "-m",
            &format!("Import snapshot from {source_name}"),
        ],
    )?;
    git(Some(repo_path), &["branch", "-M", baseline_branch])?;
    Ok(())
}

pub fn prepare_shallow_repository_for_publication(repo_path: &Path) -> Result<bool> {
    if !is_shallow_repository(repo_path)? {
        return Ok(false);
    }
    if local_branches(repo_path)?
        .iter()
        .any(|branch| branch.starts_with("orx/"))
    {
        let remote = ["upstream", "origin"]
            .into_iter()
            .find(|remote| git(Some(repo_path), &["remote", "get-url", remote]).is_ok())
            .ok_or_else(|| anyhow!("The shallow project has no source remote to deepen."))?;
        git(Some(repo_path), &["fetch", "--unshallow", remote])?;
        return Ok(false);
    }
    Ok(true)
}

const GITHUB_CREDENTIAL_HELPER: &str = "!gh auth git-credential";

fn redact_remote_urls(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let bare = word.trim_matches(|ch: char| "'\"`()[]{}<>,".contains(ch));
            if bare.to_ascii_lowercase().contains("://") {
                word.replace(bare, &sanitize_remote_url(bare))
            } else {
                word.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn authenticated_git_command(repo_path: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(repo_path)
        .env("GH_HOST", "github.com")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(paths) = super::shell_env::search_path() {
        command.env("PATH", paths);
    }
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && std::env::var_os("GIT_SSH").is_none() {
        command.env("GIT_SSH_COMMAND", "ssh -oBatchMode=yes -oConnectTimeout=15");
    }
    command
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_CONFIG_COUNT", "3")
        .env("GIT_CONFIG_KEY_0", "credential.helper")
        .env("GIT_CONFIG_VALUE_0", "")
        .env("GIT_CONFIG_KEY_1", "credential.helper")
        .env("GIT_CONFIG_VALUE_1", GITHUB_CREDENTIAL_HELPER)
        .env("GIT_CONFIG_KEY_2", "core.hooksPath")
        .env("GIT_CONFIG_VALUE_2", "/dev/null");
    #[cfg(unix)]
    command.process_group(0);
    command
}

fn authenticated_git(repo_path: &Path, args: &[&str], timeout: Duration) -> Result<String> {
    let mut command = authenticated_git_command(repo_path);
    let mut child = command
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| anyhow!("Could not run git: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Could not capture git stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Could not capture git stderr"))?;
    let stdout_reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if Instant::now() >= deadline {
            terminate_git_process_tree(&mut child);
            break (child.wait()?, true);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("Could not collect git stdout"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("Could not collect git stderr"))??;
    if timed_out {
        return Err(anyhow!(
            "git {} timed out after {} seconds",
            args.first().copied().unwrap_or("command"),
            timeout.as_secs()
        ));
    }
    if !status.success() {
        let error = String::from_utf8_lossy(&stderr).trim().to_string();
        return Err(anyhow!(
            "git {} failed: {}",
            args.first().copied().unwrap_or("command"),
            redact_remote_urls(&error)
        ));
    }
    Ok(String::from_utf8_lossy(&stdout).trim().to_string())
}

fn terminate_git_process_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        // Git owns this process group, including credential-bearing transport children.
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(not(unix))]
    let _ = child.kill();
}

fn push(repo_path: &Path, args: &[&str]) -> Result<()> {
    let mut command = vec!["push"];
    command.extend_from_slice(args);
    authenticated_git(repo_path, &command, Duration::from_secs(600))?;
    Ok(())
}

pub fn add_github_remote(repo_path: &Path, owner: &str, repo: &str) -> Result<()> {
    let url = format!("https://github.com/{owner}/{repo}.git");
    if let Ok(existing) = git(Some(repo_path), &["remote", "get-url", GITHUB_REMOTE]) {
        if github_repository_matches(&existing, owner, repo) {
            let _ = git(
                Some(repo_path),
                &["config", "--unset-all", "remote.github.pushurl"],
            );
            git(
                Some(repo_path),
                &["remote", "set-url", "--add", "--push", GITHUB_REMOTE, &url],
            )?;
            return Ok(());
        }
        let remotes = git(Some(repo_path), &["remote"])?;
        let mut suffix = 1;
        let backup = loop {
            let candidate = if suffix == 1 {
                "upstream".to_string()
            } else {
                format!("upstream-{suffix}")
            };
            if !remotes.lines().any(|remote| remote == candidate) {
                break candidate;
            }
            suffix += 1;
        };
        git(
            Some(repo_path),
            &["remote", "rename", GITHUB_REMOTE, &backup],
        )?;
    }
    git(Some(repo_path), &["remote", "add", GITHUB_REMOTE, &url])?;
    Ok(())
}

pub fn push_all(repo_path: &Path, baseline_branch: &str, owner: &str, repo: &str) -> Result<()> {
    let remote = publication_remote(repo_path, owner, repo)?;
    let mut branches = local_branches(repo_path)?
        .into_iter()
        .filter(|branch| branch == baseline_branch || branch.starts_with("orx/"))
        .collect::<Vec<_>>();
    branches.sort_by_key(|branch| branch != baseline_branch);
    for branch in branches {
        push(repo_path, &["-u", &remote, &branch])?;
    }
    Ok(())
}

pub fn push_branch(repo_path: &Path, branch: &str, owner: &str, repo: &str) -> Result<()> {
    let remote = publication_remote(repo_path, owner, repo)?;
    push(repo_path, &["-u", &remote, branch])
}

pub fn spawn_branch_publication(
    repo_path: &Path,
    branch: &str,
    owner: &str,
    repo: &str,
) -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg("publish-branch")
        .arg(repo_path)
        .arg(branch)
        .arg(owner)
        .arg(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(paths) = super::shell_env::search_path() {
        command.env("PATH", paths);
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| anyhow!("Could not start GitHub publication worker: {error}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub fn publication_sync_status(
    repo_path: &Path,
    baseline_branch: &str,
    owner: &str,
    repo: &str,
) -> &'static str {
    let Ok(remote) = publication_remote(repo_path, owner, repo) else {
        return "not configured";
    };
    let Ok(remote_refs) = authenticated_git(
        repo_path,
        &["ls-remote", "--heads", &remote],
        Duration::from_secs(30),
    ) else {
        return "unknown";
    };
    let Ok(branches) = local_branches(repo_path) else {
        return "unknown";
    };
    for branch in branches
        .into_iter()
        .filter(|branch| branch == baseline_branch || branch.starts_with("orx/"))
    {
        let Ok(local) = local_head_sha(repo_path, &branch) else {
            return "unknown";
        };
        let reference = format!("refs/heads/{branch}");
        if !remote_refs
            .lines()
            .any(|line| line == format!("{local}\t{reference}"))
        {
            return if remote_refs.lines().any(|line| line.ends_with(&reference)) {
                "local changes to push"
            } else {
                "not pushed"
            };
        }
    }
    "synced"
}

/// Create `new_branch` from `parent_branch`'s local tip.
pub fn create_experiment_branch(
    repo_path: &Path,
    parent_branch: &str,
    new_branch: &str,
) -> Result<()> {
    git(
        Some(repo_path),
        &["branch", "--no-track", new_branch, parent_branch],
    )?;
    Ok(())
}

/// A file's content at a specific commit (`git show <sha>:<path>`), i.e.
/// exactly what a job cloning that sha will see — not the working tree.
pub fn file_at(repo_path: &Path, sha: &str, path: &str) -> Result<String> {
    git(Some(repo_path), &["show", &format!("{sha}:{path}")])
}

/// Whether the repo tracks `path` (local check, no network).
pub fn is_tracked(repo_path: &Path, path: &str) -> bool {
    git(
        Some(repo_path),
        &["ls-files", "--error-unmatch", "--", path],
    )
    .is_ok()
}

// --- diffs ------------------------------------------------------------------

/// Whole-diff cap, mirroring the OpenResearch api's MAX_DIFF_BYTES.
pub const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

pub struct DiffPayload {
    pub diff: String,
    pub truncated: bool,
    pub bytes_read: usize,
}

pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    /// Unix seconds.
    pub committed_at: i64,
}

/// Like `git` but raw stdout bytes, no trim, and extra tolerated exit codes
/// (`git diff --no-index` exits 1 when the files differ).
fn git_bytes(dir: &Path, args: &[&str], ok_codes: &[i32]) -> Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir);
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    let code = out.status.code().unwrap_or(-1);
    if !out.status.success() && !ok_codes.contains(&code) {
        return Err(anyhow!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

fn cap_diff(mut bytes: Vec<u8>) -> DiffPayload {
    let truncated = bytes.len() > MAX_DIFF_BYTES;
    if truncated {
        bytes.truncate(MAX_DIFF_BYTES);
    }
    let bytes_read = bytes.len();
    // lossy: the cap can land mid multibyte char
    DiffPayload {
        diff: String::from_utf8_lossy(&bytes).into_owned(),
        truncated,
        bytes_read,
    }
}

/// Resolve a branch name or sha to something git can diff. Prefers the local
/// ref (where the agent works) and falls back to origin.
fn resolve_commitish(repo: &Path, name: &str) -> Result<String> {
    for cand in [name.to_string(), format!("refs/remotes/origin/{name}")] {
        let probe = format!("{cand}^{{commit}}");
        if git(Some(repo), &["rev-parse", "--verify", "--quiet", &probe]).is_ok() {
            return Ok(cand);
        }
    }
    Err(anyhow!("unknown git ref: {name}"))
}

/// Cumulative diff `base...head` (merge-base semantics, same as the cloud
/// compare endpoint).
pub fn diff_range(repo: &Path, base: &str, head: &str) -> Result<DiffPayload> {
    let base = resolve_commitish(repo, base)?;
    let head = resolve_commitish(repo, head)?;
    let range = format!("{base}...{head}");
    Ok(cap_diff(git_bytes(
        repo,
        &["--no-pager", "diff", &range],
        &[],
    )?))
}

/// Single-commit diff. `git show` handles root commits, unlike `sha~1..sha`.
pub fn commit_diff(repo: &Path, sha: &str) -> Result<DiffPayload> {
    Ok(cap_diff(git_bytes(
        repo,
        &["--no-pager", "show", "--format=", "--patch", sha],
        &[],
    )?))
}

fn parse_commit_lines(out: &str) -> Vec<CommitInfo> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.split('\u{1f}');
            Some(CommitInfo {
                sha: parts.next()?.to_string(),
                subject: parts.next()?.to_string(),
                committed_at: parts.next()?.parse().ok()?,
            })
        })
        .collect()
}

/// Commits on `head` that aren't on `base`, newest first.
pub fn list_commits_between(
    repo: &Path,
    base: &str,
    head: &str,
    limit: usize,
) -> Result<Vec<CommitInfo>> {
    let base = resolve_commitish(repo, base)?;
    let head = resolve_commitish(repo, head)?;
    let range = format!("{base}..{head}");
    let out = git(
        Some(repo),
        &[
            "log",
            "--format=%H%x1f%s%x1f%ct",
            "-n",
            &limit.to_string(),
            &range,
        ],
    )?;
    Ok(parse_commit_lines(&out))
}

/// Latest commits on a branch, newest first.
pub fn list_commits(repo: &Path, branch: &str, limit: usize) -> Result<Vec<CommitInfo>> {
    let branch = resolve_commitish(repo, branch)?;
    let out = git(
        Some(repo),
        &[
            "log",
            "--format=%H%x1f%s%x1f%ct",
            "-n",
            &limit.to_string(),
            &branch,
        ],
    )?;
    Ok(parse_commit_lines(&out))
}

/// Uncommitted changes in the clone: tracked edits vs HEAD plus untracked
/// files rendered as new-file diffs. Returns (current branch, diff).
pub fn working_tree_diff(repo: &Path) -> Result<(Option<String>, DiffPayload)> {
    let branch = current_branch(repo);
    let payload = working_tree_diff_against(repo, None)?;
    Ok((branch, payload))
}

/// The working tree diffed against `base` (a merge-base sha for a session
/// worktree) instead of `HEAD` — the agent commits its work to experiment
/// branches, so a bare `HEAD` diff would hide everything it committed since
/// forking from the baseline. `None` diffs against `HEAD` — the clone-scoped
/// behaviour `working_tree_diff` wraps. Tracked edits come
/// from one `git diff`; untracked files are appended as new-file diffs, both
/// under the shared `MAX_DIFF_BYTES` cap.
pub fn working_tree_diff_against(repo: &Path, base: Option<&str>) -> Result<DiffPayload> {
    let base = base.unwrap_or("HEAD");
    let mut bytes = git_bytes(repo, &["--no-pager", "diff", base], &[1])?;
    let untracked = git(Some(repo), &["ls-files", "--others", "--exclude-standard"])?;
    for f in untracked.lines().filter(|l| !l.is_empty()) {
        if bytes.len() > MAX_DIFF_BYTES {
            break;
        }
        if let Ok(chunk) = git_bytes(
            repo,
            &["--no-pager", "diff", "--no-index", "--", "/dev/null", f],
            &[1],
        ) {
            bytes.extend_from_slice(&chunk);
        }
    }
    Ok(cap_diff(bytes))
}

/// The merge-base of `a` and `b` (`git merge-base`), or `Ok(None)` when git
/// can't compute one — an unresolvable ref, unrelated histories, or a fresh
/// worktree whose baseline ref is momentarily missing. Callers treat `None` as
/// "fall back to HEAD" rather than an error: a missing merge-base is a routine
/// state, not a failure.
pub fn merge_base(repo: &Path, a: &str, b: &str) -> Result<Option<String>> {
    match git(Some(repo), &["merge-base", a, b]) {
        Ok(sha) if !sha.is_empty() => Ok(Some(sha)),
        _ => Ok(None),
    }
}

/// How a changed file differs from the diff base. Serialized lowercase to
/// match the single-letter status badges the UI renders; `renamed` and
/// `untracked` have no git single-letter porcelain equivalent (rename is `R`
/// with a score, untracked comes from a separate `ls-files` pass).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangedStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

/// One entry in the session worktree's change list. `old_path` is only set for
/// renames (the pre-rename path); the list is complete even when the unified
/// diff truncates, because it comes from a separate name-status pass.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: ChangedStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
}

/// The set of files that differ between `base` and the worktree — tracked
/// changes from `git diff --name-status -z` (so rename pairs and non-ASCII
/// paths survive intact) plus untracked-not-ignored files from `git ls-files
/// --others`. This is the authoritative change list the Changes view renders:
/// it stays complete even when the unified diff hits `MAX_DIFF_BYTES`. Sorted
/// by path for a stable ordering.
pub fn changed_files(repo: &Path, base: &str) -> Result<Vec<ChangedFile>> {
    let bytes = git_bytes(
        repo,
        &["--no-pager", "diff", "--name-status", "-z", base],
        &[1],
    )?;
    // -z output is NUL-delimited fields (not lines): a status field followed by
    // one path, except renames/copies which emit `R<score>`/`C<score>` and two
    // paths (old, new). Walk the fields with an explicit cursor so the extra
    // rename path is consumed in lockstep.
    let fields = split_nul(&bytes);
    let mut files = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let code = &fields[i];
        let first = code.chars().next().unwrap_or(' ');
        match first {
            'R' | 'C' => {
                // Rename/copy: <old> then <new>. A copy leaves the original in
                // place, so surface only the new path (as a rename) — the old
                // file is unchanged and needs no row.
                let old = fields.get(i + 1).cloned();
                let new = fields.get(i + 2).cloned();
                if let Some(path) = new {
                    files.push(ChangedFile {
                        path,
                        status: ChangedStatus::Renamed,
                        old_path: old,
                    });
                }
                i += 3;
            }
            _ => {
                if let Some(path) = fields.get(i + 1).cloned() {
                    let status = match first {
                        'A' => ChangedStatus::Added,
                        'D' => ChangedStatus::Deleted,
                        _ => ChangedStatus::Modified,
                    };
                    files.push(ChangedFile {
                        path,
                        status,
                        old_path: None,
                    });
                }
                i += 2;
            }
        }
    }
    let untracked = git_bytes(
        repo,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        &[],
    )?;
    for path in split_nul(&untracked) {
        // `--others` reports a nested git repo as `dir/` — not a file; drop it.
        if path.ends_with('/') {
            continue;
        }
        files.push(ChangedFile {
            path,
            status: ChangedStatus::Untracked,
            old_path: None,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    // The two passes cover disjoint states (tracked vs untracked), but the UI
    // keys rows on the path — dedupe defensively rather than trust git's edge
    // cases (e.g. index-only states) forever.
    files.dedup_by(|a, b| a.path == b.path);
    Ok(files)
}

/// The checked-out branch name, or `None` when detached (`rev-parse
/// --abbrev-ref HEAD` prints the literal `HEAD` — e.g. a fresh worktree
/// before the agent checks out its branch) or when rev-parse fails outright
/// (unborn HEAD in an empty repo). Never errors — no branch is an answer.
pub fn current_branch(repo: &Path) -> Option<String> {
    git(Some(repo), &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .filter(|b| b != "HEAD" && !b.is_empty())
}

/// NUL-separated git output (`-z` flags) → lossy-decoded strings, empties
/// dropped.
fn split_nul(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect()
}

/// Every path in the checkout that git would show as tracked or
/// untracked-but-not-ignored (`git ls-files --cached --others
/// --exclude-standard -z`). NUL-separated so non-ASCII paths aren't quoted;
/// gitignored trees (`target/`, `node_modules/`, `.git/`) drop out for free.
/// Repo-relative, unsorted.
pub fn list_worktree_files(repo: &Path) -> Result<Vec<String>> {
    let bytes = git_bytes(
        repo,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
        &[],
    )?;
    let mut entries = split_nul(&bytes);
    // `--others` reports a nested git repo as `dir/` — a directory, nothing
    // servable as a file; drop those here so every client benefits.
    entries.retain(|e| !e.ends_with('/'));
    Ok(entries)
}

/// Resolve a branch name to its commit sha — the *local* ref first, then
/// origin's: the code browser wants the agent's latest work, which lives
/// locally before any push (the opposite preference of `branch_head_sha`,
/// which serves jobs that clone from the remote; `resolve_commitish` is the
/// diff-side sibling that also accepts raw shas). `Ok(None)` when neither
/// exists. Only real branch names are accepted: rev-suffix expressions
/// (`@{...}`, `^`, `~`, `:`, whitespace) are rejected up front, and the
/// leading-`-` check is belt-and-braces — the `refs/heads/` prefix already
/// keeps the name out of option position.
pub fn resolve_branch_commit(repo: &Path, name: &str) -> Result<Option<String>> {
    let suspicious = name.is_empty()
        || name.starts_with('-')
        || name.contains("@{")
        || name.contains(['^', '~', ':'])
        || name.chars().any(char::is_whitespace);
    if suspicious {
        return Ok(None);
    }
    for prefix in ["refs/heads/", "refs/remotes/origin/"] {
        let full = format!("{prefix}{name}");
        if let Ok(sha) = git(Some(repo), &["rev-parse", "--verify", "--quiet", &full]) {
            if !sha.is_empty() {
                return Ok(Some(sha));
            }
        }
    }
    Ok(None)
}

/// Every path in the tree of a commit (`git ls-tree -r -z --name-only`) —
/// the committed state, independent of any checkout. Repo-relative, unsorted.
pub fn list_tree_files(repo: &Path, sha: &str) -> Result<Vec<String>> {
    let bytes = git_bytes(
        repo,
        &["ls-tree", "-r", "-z", "--name-only", sha, "--"],
        &[],
    )?;
    Ok(split_nul(&bytes))
}

/// Whether `<sha>:<path>` is a blob in the committed tree.
pub fn file_exists_at(repo: &Path, sha: &str, path: &str) -> Result<bool> {
    Ok(file_size_at(repo, sha, path)?.is_some())
}

/// Size of a committed blob at `<sha>:<path>`, or `None` when the path does
/// not name a blob.
pub fn file_size_at(repo: &Path, sha: &str, path: &str) -> Result<Option<u64>> {
    use std::process::Stdio;
    let spec = format!("{sha}:{path}");
    let kind = Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["cat-file", "-t", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    if !kind.status.success() || kind.stdout != b"blob\n" {
        return Ok(None);
    }
    let size = Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["cat-file", "-s", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    if !size.status.success() {
        return Err(anyhow!("git cat-file -s {spec} failed"));
    }
    let value = String::from_utf8(size.stdout)
        .map_err(|e| anyhow!("git cat-file returned invalid size: {e}"))?;
    let value = value
        .trim()
        .parse::<u64>()
        .map_err(|e| anyhow!("git cat-file returned invalid size: {e}"))?;
    Ok(Some(value))
}

/// A file's committed bytes at `<sha>:<path>`, read from a streamed
/// `git cat-file blob` and capped at `limit` bytes — a multi-GB committed
/// blob costs one pipe buffer, not one allocation. Returns byte-exact content
/// so callers can decide whether it is text or media without lossy decoding.
pub fn file_bytes_at_capped(
    repo: &Path,
    sha: &str,
    path: &str,
    limit: u64,
) -> Result<Option<(Vec<u8>, bool)>> {
    use std::process::Stdio;
    let spec = format!("{sha}:{path}");
    if !file_exists_at(repo, sha, path)? {
        return Ok(None);
    }
    let mut child = Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["cat-file", "blob", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("Could not run git: {}", e))?;
    let mut buf = Vec::new();
    let read = {
        use std::io::Read as _;
        let stdout = child.stdout.take().expect("stdout was piped");
        stdout.take(limit + 1).read_to_end(&mut buf)
    };
    let truncated = buf.len() as u64 > limit;
    // Reap the child before propagating any read error — no zombies. Kill
    // only when it may still be streaming (read error, or we stopped at the
    // cap): after a complete read EOF means git closed stdout and exits on
    // its own, and killing it then could race its natural exit into a bogus
    // signal-death status.
    if read.is_err() || truncated {
        let _ = child.kill();
    }
    let status = child.wait();
    read.map_err(|e| anyhow!("read failed: {}", e))?;
    if !truncated {
        // A cat-file failure after the `-e` probe (the path names a tree via
        // a crafted request, or the object vanished) must not masquerade as
        // an empty file. When truncated we killed it — any status goes.
        let status = status.map_err(|e| anyhow!("git cat-file blob: {}", e))?;
        if !status.success() {
            return Err(anyhow!("git cat-file blob {spec} failed"));
        }
    }
    buf.truncate(limit as usize);
    Ok(Some((buf, truncated)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_git_uses_only_the_command_scoped_gh_credential_helper() {
        use std::ffi::OsStr;

        let command = authenticated_git_command(Path::new("."));
        let search_path = crate::local::shell_env::search_path();
        let env = |key: &str| {
            command
                .get_envs()
                .find(|(name, _)| *name == OsStr::new(key))
                .and_then(|(_, value)| value)
        };

        assert_eq!(env("GIT_CONFIG_COUNT"), Some(OsStr::new("3")));
        assert_eq!(env("GH_HOST"), Some(OsStr::new("github.com")));
        assert_eq!(env("PATH"), search_path.as_deref());
        assert_eq!(env("GIT_CONFIG_VALUE_0"), Some(OsStr::new("")));
        assert_eq!(
            env("GIT_CONFIG_VALUE_1"),
            Some(OsStr::new("!gh auth git-credential"))
        );
    }

    #[test]
    fn managed_large_file_ignores_are_anchored_escaped_and_idempotent() {
        let paths = vec![b"data set/checkpoint[1].bin".to_vec()];
        let first = managed_gitignore(b"target/\n", &paths).unwrap();
        let second = managed_gitignore(&first, &paths).unwrap();

        assert_eq!(first, second);
        let text = String::from_utf8(first).unwrap();
        assert!(text.starts_with("target/\n"));
        assert!(text.contains("/data\\ set/checkpoint\\[1\\].bin\n"));
        assert_eq!(text.matches(MANAGED_IGNORE_START).count(), 1);
    }

    #[test]
    fn managed_gitignore_preserves_rules_after_an_orphaned_start_marker() {
        let existing = format!("before\n{MANAGED_IGNORE_START}\nsecret.env\nafter\n");

        let updated =
            managed_gitignore(existing.as_bytes(), &[b"checkpoint.bin".to_vec()]).unwrap();
        let text = String::from_utf8(updated).unwrap();

        assert!(text.starts_with("before\nsecret.env\nafter\n"));
        assert!(text.contains("/checkpoint.bin\n"));
        assert_eq!(text.matches(MANAGED_IGNORE_START).count(), 1);
        assert_eq!(text.matches(MANAGED_IGNORE_END).count(), 1);
    }

    #[test]
    fn repository_state_distinguishes_unborn_ready_and_detached() {
        let dir = std::env::temp_dir().join(format!("orx-git-state-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(repository_state(&dir), RepositoryState::NotRepository);
        let invalid = dir.join("invalid");
        std::fs::create_dir_all(invalid.join(".git")).unwrap();
        assert_eq!(repository_state(&invalid), RepositoryState::Invalid);
        run(&dir, &["init", "-q", "-b", "main"]);
        assert_eq!(repository_state(&dir), RepositoryState::Unborn);
        write(&dir, "seed.txt", "seed\n");
        run(&dir, &["add", "-A"]);
        run(
            &dir,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "seed",
            ],
        );
        assert_eq!(repository_state(&dir), RepositoryState::Ready);
        run(&dir, &["checkout", "-q", "--detach"]);
        assert_eq!(repository_state(&dir), RepositoryState::Detached);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn initial_snapshot_measures_a_symlink_instead_of_its_target() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!("orx-symlink-scan-{}", uuid::Uuid::new_v4()));
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let target = root.join("checkpoint.bin");
        std::fs::File::create(&target)
            .unwrap()
            .set_len(INITIAL_SNAPSHOT_MAX_TOTAL_BYTES)
            .unwrap();
        symlink(&target, project.join("checkpoint-link")).unwrap();

        let snapshot = initial_snapshot(&project).unwrap();

        assert!(snapshot.excluded_paths.is_empty());
        assert_eq!(
            snapshot.included_bytes,
            target.as_os_str().to_string_lossy().len() as u64
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn initial_snapshot_preserves_non_utf8_paths_for_exclusion() {
        use std::os::unix::ffi::OsStringExt;

        let root =
            std::env::temp_dir().join(format!("orx-byte-path-scan-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let raw_name = vec![
            b'c', b'h', b'e', b'c', b'k', b'p', b'o', b'i', b'n', b't', 0xff,
        ];
        std::fs::File::create(root.join(std::ffi::OsString::from_vec(raw_name.clone())))
            .unwrap()
            .set_len(INITIAL_SNAPSHOT_MAX_FILE_BYTES)
            .unwrap();

        let snapshot = initial_snapshot(&root).unwrap();

        assert_eq!(snapshot.excluded_paths, vec![raw_name]);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A throwaway git repo under the temp dir with one seed commit on `main`.
    /// Configured with a fixed identity and no signing/hooks so `commit`
    /// succeeds regardless of the host's global git config.
    fn temp_repo() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orx-git-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create repo dir");
        run(&dir, &["init", "-q", "-b", "main"]);
        run(&dir, &["config", "user.name", "orx-test"]);
        run(&dir, &["config", "user.email", "orx-test@example.com"]);
        run(&dir, &["config", "commit.gpgsign", "false"]);
        write(&dir, "seed.txt", "seed\n");
        run(&dir, &["add", "-A"]);
        run(&dir, &["commit", "-q", "-m", "seed"]);
        dir
    }

    fn run(dir: &Path, args: &[&str]) -> String {
        git(Some(dir), args).unwrap_or_else(|e| panic!("git {args:?} failed: {e}"))
    }

    fn write(dir: &Path, rel: &str, contents: &str) {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(path, contents).expect("write file");
    }

    #[test]
    fn committed_file_reads_preserve_binary_bytes() {
        let dir = temp_repo();
        let bytes = [0x89, b'P', b'N', b'G', 0, 0xff];
        std::fs::write(dir.join("figure.png"), bytes).unwrap();
        run(&dir, &["add", "figure.png"]);
        run(&dir, &["commit", "-q", "-m", "binary"]);
        let sha = run(&dir, &["rev-parse", "HEAD"]);

        let (actual, truncated) = file_bytes_at_capped(&dir, &sha, "figure.png", 1024)
            .unwrap()
            .unwrap();
        assert_eq!(actual, bytes);
        assert!(!truncated);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn statuses(files: &[ChangedFile]) -> Vec<(String, ChangedStatus)> {
        files
            .iter()
            .map(|f| (f.path.clone(), f.status.clone()))
            .collect()
    }

    #[test]
    fn merge_base_normal_and_missing() {
        let repo = temp_repo();
        let seed = run(&repo, &["rev-parse", "HEAD"]);
        // A second commit on main: merge-base(HEAD, seed) is the seed itself.
        write(&repo, "a.txt", "a\n");
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "second"]);
        assert_eq!(merge_base(&repo, "HEAD", &seed).unwrap(), Some(seed));
        // An unknown ref yields None, never an error.
        assert_eq!(merge_base(&repo, "HEAD", "no-such-ref").unwrap(), None);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn changed_files_committed_ahead_uncommitted_untracked() {
        let repo = temp_repo();
        let base = run(&repo, &["rev-parse", "HEAD"]);
        // Committed-ahead: a new file committed on top of the base.
        write(&repo, "committed.txt", "c\n");
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "add committed"]);
        // Uncommitted: edit a tracked file in the working tree.
        write(&repo, "seed.txt", "seed edited\n");
        // Untracked: a brand-new unstaged file.
        write(&repo, "untracked.txt", "u\n");

        let got = statuses(&changed_files(&repo, &base).unwrap());
        assert_eq!(
            got,
            vec![
                ("committed.txt".to_string(), ChangedStatus::Added),
                ("seed.txt".to_string(), ChangedStatus::Modified),
                ("untracked.txt".to_string(), ChangedStatus::Untracked),
            ]
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn changed_files_rename_and_delete() {
        let repo = temp_repo();
        write(
            &repo,
            "old.txt",
            "the quick brown fox jumps over the lazy dog\n",
        );
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "add old"]);
        write(&repo, "gone.txt", "delete me\n");
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "add gone"]);
        let base = run(&repo, &["rev-parse", "HEAD"]);
        // Rename old.txt -> new.txt (identical content: a pure rename) and
        // delete gone.txt, then commit so they land in the name-status diff.
        run(&repo, &["mv", "old.txt", "new.txt"]);
        run(&repo, &["rm", "-q", "gone.txt"]);
        run(&repo, &["commit", "-q", "-m", "rename and delete"]);

        let files = changed_files(&repo, &base).unwrap();
        let renamed = files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("rename row");
        assert_eq!(renamed.status, ChangedStatus::Renamed);
        assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
        let deleted = files
            .iter()
            .find(|f| f.path == "gone.txt")
            .expect("delete row");
        assert_eq!(deleted.status, ChangedStatus::Deleted);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn working_tree_diff_against_includes_committed_ahead() {
        let repo = temp_repo();
        let base = run(&repo, &["rev-parse", "HEAD"]);
        // Content committed after the base must appear in the diff — a bare HEAD
        // diff (base = None) would show nothing, since the tree is clean.
        write(&repo, "ahead.txt", "committed line\n");
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "ahead"]);

        assert!(working_tree_diff_against(&repo, None)
            .unwrap()
            .diff
            .is_empty());
        let payload = working_tree_diff_against(&repo, Some(&base)).unwrap();
        assert!(payload.diff.contains("ahead.txt"));
        assert!(payload.diff.contains("committed line"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn detached_zero_change_is_empty() {
        // A fresh detached checkout at the baseline tip with no edits: base and
        // HEAD coincide, so there are no changed files and an empty diff — the
        // "detached, No changes yet" state the UI renders.
        let repo = temp_repo();
        let head = run(&repo, &["rev-parse", "HEAD"]);
        run(&repo, &["checkout", "-q", "--detach", &head]);
        let base = merge_base(&repo, "main", "HEAD")
            .unwrap()
            .expect("merge-base");
        assert!(changed_files(&repo, &base).unwrap().is_empty());
        assert!(working_tree_diff_against(&repo, Some(&base))
            .unwrap()
            .diff
            .is_empty());
        assert_eq!(current_branch(&repo), None);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn experiment_branch_collision_never_rewrites_existing_work() {
        let repo = temp_repo();
        run(&repo, &["branch", "orx/existing"]);
        let before = run(&repo, &["rev-parse", "orx/existing"]);
        write(&repo, "later.txt", "later\n");
        run(&repo, &["add", "later.txt"]);
        run(&repo, &["commit", "-q", "-m", "later"]);

        assert!(create_experiment_branch(&repo, "main", "orx/existing").is_err());
        assert_eq!(run(&repo, &["rev-parse", "orx/existing"]), before);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn public_clone_urls_reject_credentials_and_secret_suffixes() {
        assert!(public_clone_url("https://user:token@example.com/repo.git").is_err());
        assert!(public_clone_url("https://example.com/repo.git?token=secret").is_err());
        assert!(public_clone_url("ssh://git@example.com/repo.git").is_err());
        assert_eq!(
            sanitize_remote_url("https://user:token@example.com/repo.git?token=secret"),
            "https://example.com/repo.git"
        );
        assert_eq!(
            sanitize_remote_url("SSH://user:password@example.com/repo.git"),
            "SSH://example.com/repo.git"
        );
    }

    #[test]
    fn public_clone_history_is_shallow_only_when_requested() {
        assert_eq!(
            public_clone_history_args(true),
            &["--depth=1", "--single-branch"]
        );
        assert!(public_clone_history_args(false).is_empty());
    }
}
