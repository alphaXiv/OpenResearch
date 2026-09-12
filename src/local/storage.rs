//! Move legacy repository storage once, using existing paths rather than a migration marker.

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{anyhow, Result};
use crate::store::Store;

use super::git;

pub async fn prepare() -> Result<()> {
    tokio::task::spawn_blocking(|| {
        let source = normalize(&git::legacy_cache_root())?;
        let target = normalize(&crate::store::data_dir())?;
        if source == target {
            return Ok(());
        }
        let mappings = mappings(&source, &target);
        let mut lock = crate::store::open_lifecycle_lock()?;
        let needed = {
            // ponytail: O(repository metadata) for marker-free recovery; reuse startup inventory if this grows.
            let _guard = lock.read()?;
            mappings.iter().any(|(from, _)| from.exists())
                || !references(&target, &mappings)?.is_empty()
        };
        if !needed {
            return Ok(());
        }
        let _guard = lock.try_write().map_err(|_| {
            anyhow!("Repository storage needs to move. Close other OpenResearch processes and retry; no live files were moved.")
        })?;
        migrate(&source, &target, false)
    })
    .await
    .map_err(|error| anyhow!("Repository migration failed: {error}"))?
}

fn mappings(source: &Path, target: &Path) -> [(PathBuf, PathBuf); 2] {
    ["repos", "worktrees"].map(|name| (source.join(name), target.join(name)))
}

fn normalize(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        missing.push(
            ancestor
                .file_name()
                .ok_or_else(|| anyhow!("Invalid storage path"))?,
        );
        ancestor = ancestor
            .parent()
            .ok_or_else(|| anyhow!("Invalid storage path"))?;
    }
    let mut result = crate::paths::canonicalize(ancestor)?;
    for name in missing.into_iter().rev() {
        result.push(name);
    }
    Ok(result)
}

fn mapped(path: &Path, mappings: &[(PathBuf, PathBuf)]) -> PathBuf {
    if mappings.iter().any(|(_, target)| path.starts_with(target)) {
        return path.to_path_buf();
    }
    let normalized = normalize(path).unwrap_or_else(|_| path.to_path_buf());
    mappings
        .iter()
        .find_map(|(from, to)| {
            let from_normalized = normalize(from).unwrap_or_else(|_| from.clone());
            path.strip_prefix(from)
                .or_else(|_| normalized.strip_prefix(&from_normalized))
                .ok()
                .map(|suffix| to.join(suffix))
        })
        .filter(|target| target.exists())
        .unwrap_or_else(|| path.to_path_buf())
}

fn migrate(source: &Path, target: &Path, copy: bool) -> Result<()> {
    if target.is_dir() {
        for entry in std::fs::read_dir(target)? {
            let entry = entry?;
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".orx-copy-")
            {
                return Err(anyhow!("An interrupted repository copy exists at {}. Files have been preserved; resolve it before retrying.", entry.path().display()));
            }
        }
    }
    let mappings = mappings(source, target);
    for (from, to) in &mappings {
        if !from.exists() {
            continue;
        }
        if from.starts_with(to) || to.starts_with(from) {
            return Err(anyhow!(
                "Repository storage paths overlap: {} and {}",
                from.display(),
                to.display()
            ));
        }
        if !std::fs::symlink_metadata(from)?.is_dir() {
            return Err(anyhow!(
                "Repository storage is not a directory: {}",
                from.display()
            ));
        }
        if to.is_symlink() || to.try_exists()? {
            return Err(anyhow!("Both {} and {} exist. Both have been preserved; resolve the conflict before retrying.", from.display(), to.display()));
        }
    }
    let mut copied = Vec::new();
    for (from, to) in &mappings {
        if !from.exists() {
            continue;
        }
        std::fs::create_dir_all(target)?;
        if !copy {
            match std::fs::rename(from, to) {
                Ok(()) => {
                    sync_directory(target)?;
                    sync_directory(source)?;
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {}
                Err(error) => return Err(error.into()),
            }
        }
        let temporary = target.join(format!(".orx-copy-{}", uuid::Uuid::new_v4()));
        copy_verified(from, &temporary)?;
        std::fs::rename(&temporary, to)?;
        sync_directory(target)?;
        copied.push(from);
    }
    repair_paths(target, &mappings)?;
    for source in copied {
        std::fs::remove_dir_all(source)?;
    }
    Ok(())
}

fn copy_verified(source: &Path, target: &Path) -> Result<()> {
    std::fs::create_dir(target)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_verified(&from, &to)?;
        } else if kind.is_symlink() {
            super::native_store::copy_symlink(&from, &to)?;
            if std::fs::read_link(&from)? != std::fs::read_link(&to)? {
                return Err(anyhow!(
                    "Symlink copy verification failed: {}",
                    from.display()
                ));
            }
        } else if kind.is_file() {
            let file = std::fs::File::create_new(&to)?;
            // Keep a writable handle for Windows after copy applies read-only source permissions.
            std::fs::copy(&from, &to)?;
            verify_file(&from, &to)?;
            file.sync_all()?;
        } else {
            return Err(anyhow!("Cannot migrate special file: {}", from.display()));
        }
    }
    std::fs::set_permissions(target, std::fs::metadata(source)?.permissions())?;
    sync_directory(target)?;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn verify_file(source: &Path, target: &Path) -> Result<()> {
    if hash(source)? != hash(target)? {
        return Err(anyhow!("Copy verification failed: {}", source.display()));
    }
    Ok(())
}

fn hash(path: &Path) -> Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(digest.finalize().to_vec());
        }
        digest.update(&buffer[..count]);
    }
}

#[derive(Default)]
struct References {
    projects: Vec<(String, String)>,
    worktrees: BTreeSet<(PathBuf, PathBuf, PathBuf)>,
    remotes: Vec<(PathBuf, String, String)>,
    opencode: Vec<super::native_store::OpenCodeRelocation>,
}

impl References {
    fn is_empty(&self) -> bool {
        self.projects.is_empty()
            && self.worktrees.is_empty()
            && self.remotes.is_empty()
            && self.opencode.is_empty()
    }
}

fn repositories(root: &Path, found: &mut BTreeSet<PathBuf>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    if root.join(".git").exists() {
        found.insert(root.to_path_buf());
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            repositories(&entry.path(), found)?;
        }
    }
    Ok(())
}

fn config_may_need_repair(repo: &Path, mappings: &[(PathBuf, PathBuf)]) -> Result<bool> {
    let config = match std::fs::read_to_string(repo.join(".git/config")) {
        Ok(config) => config,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => return Ok(true),
        Err(error) => return Err(error.into()),
    };
    // Let Git interpret includes and escapes rather than duplicating its config parser.
    if config.contains('\\') || config.to_ascii_lowercase().contains("[include") {
        return Ok(true);
    }
    Ok(config
        .lines()
        .filter_map(|line| line.split_once('='))
        .any(|(_, value)| {
            let value = value.trim().trim_matches('"');
            value.contains(['#', ';', '"'])
                || mapped(Path::new(value), mappings) != Path::new(value)
        }))
}

fn references(data: &Path, mappings: &[(PathBuf, PathBuf)]) -> Result<References> {
    let mut result = References::default();
    let native = data.join("agents/opencode/opencode.db");
    let mut repos = BTreeSet::new();
    repositories(&data.join("repos"), &mut repos)?;
    repositories(&data.join("worktrees"), &mut repos)?;
    if data.join("orx.db").exists() {
        let connection = rusqlite::Connection::open_with_flags(
            data.join("orx.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let mut projects = connection.prepare("SELECT repo_path FROM local_projects")?;
        for path in projects.query_map([], |row| row.get::<_, String>(0))? {
            let path = path?;
            let old = PathBuf::from(&path);
            let new = mapped(&old, mappings);
            if new != old {
                result
                    .projects
                    .push((path, new.to_string_lossy().into_owned()));
            }
            repos.insert(new);
        }
        let legacy = super::native_store::opencode_db(super::native_store::NativeStore::Legacy);
        if legacy != native {
            let changes =
                super::native_store::opencode_relocation(&legacy, |path| mapped(path, mappings));
            if !matches!(changes, Ok(None)) {
                let mut sessions = connection.prepare(
                    "SELECT native_session_id FROM chat_sessions WHERE harness = 'opencode' AND native_session_id IS NOT NULL",
                )?;
                for id in sessions.query_map([], |row| row.get::<_, String>(0))? {
                    let id = id?;
                    if !super::native_store::opencode_has_session(&native, &id)?
                        && super::native_store::opencode_has_session(&legacy, &id).unwrap_or(false)
                    {
                        if let Some(changes) = changes? {
                            result.opencode.push(changes);
                        }
                        break;
                    }
                }
            }
        }
    }
    if let Some(changes) =
        super::native_store::opencode_relocation(&native, |path| mapped(path, mappings))?
    {
        result.opencode.push(changes);
    }
    // Registrations also find moved worktrees whose project/database was deleted.
    let mut discovered = Vec::new();
    for repo in &repos {
        if repo.join(".git").is_dir() && config_may_need_repair(repo, mappings)? {
            for remote in git::git(Some(repo), &["remote"])
                .unwrap_or_default()
                .lines()
            {
                let Ok(url) = git::git(Some(repo), &["remote", "get-url", remote]) else {
                    continue;
                };
                let target = mapped(Path::new(&url), mappings);
                if target != Path::new(&url) {
                    result.remotes.push((
                        repo.clone(),
                        remote.to_string(),
                        target.to_string_lossy().into_owned(),
                    ));
                }
            }
        }
        let registrations = repo.join(".git/worktrees");
        if registrations.is_dir() {
            for entry in std::fs::read_dir(registrations)? {
                let entry = entry?;
                let backlink = entry.path().join("gitdir");
                if backlink.is_file() {
                    let old = std::fs::read_to_string(backlink)?;
                    let path = mapped(Path::new(old.trim()), mappings);
                    if let Some(worktree) = path.parent().filter(|_| path.is_file()) {
                        discovered.push(worktree.to_path_buf());
                    }
                }
            }
        }
    }
    repos.extend(discovered);
    for repo in repos {
        let gitfile = repo.join(".git");
        if !gitfile.is_file() {
            continue;
        }
        let Ok(contents) = std::fs::read_to_string(&gitfile) else {
            continue;
        };
        let Some(old) = contents.trim().strip_prefix("gitdir: ") else {
            continue;
        };
        let old = repo.join(old);
        let admin = mapped(&old, mappings);
        let backlink = admin.join("gitdir");
        let actual = match std::fs::read_to_string(&backlink) {
            Ok(actual) => admin.join(actual.trim()),
            Err(_) if admin == old => continue,
            Err(error) => {
                return Err(anyhow!(
                    "Cannot repair {}: {error}; worktree files preserved",
                    gitfile.display()
                ))
            }
        };
        if admin != old || mapped(&actual, mappings) != actual {
            let common = admin
                .parent()
                .filter(|p| p.file_name().is_some_and(|n| n == "worktrees"))
                .and_then(Path::parent)
                .filter(|p| p.file_name().is_some_and(|n| n == ".git"))
                .ok_or_else(|| {
                    anyhow!(
                        "Unsupported Git link at {}; files preserved",
                        gitfile.display()
                    )
                })?;
            let main = common
                .parent()
                .ok_or_else(|| anyhow!("Git directory has no parent"))?;
            result.worktrees.insert((main.to_path_buf(), repo, admin));
        }
    }
    Ok(result)
}

pub(crate) fn repair_paths(data: &Path, mappings: &[(PathBuf, PathBuf)]) -> Result<()> {
    let refs = references(data, mappings)?;
    for (repo, worktree, admin) in &refs.worktrees {
        // A copied worktree still points to a valid old repo; steer repair to the verified copy.
        git::atomic_write(
            &worktree.join(".git"),
            format!("gitdir: {}\n", admin.display()).as_bytes(),
        )?;
        git::git(
            Some(repo),
            &["worktree", "repair", &worktree.to_string_lossy()],
        )?;
        if git::common_git_dir(worktree)? != git::common_git_dir(repo)? {
            return Err(anyhow!(
                "Worktree {} points to the wrong repository",
                worktree.display()
            ));
        }
    }
    if !refs.projects.is_empty() {
        Store::open_at(data.to_path_buf())?.relocate_project_paths(&refs.projects)?;
    }
    for changes in refs.opencode {
        changes.apply()?;
    }
    for (repo, remote, target) in refs.remotes {
        git::git(Some(&repo), &["remote", "set-url", &remote, &target])?;
    }
    if !references(data, mappings)?.is_empty() {
        return Err(anyhow!(
            "Repository paths could not be fully updated; files have been preserved."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init(repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        git::git(Some(repo), &["init", "-b", "main"]).unwrap();
        std::fs::write(repo.join("tracked"), "original").unwrap();
        std::fs::write(repo.join(".gitignore"), "ignored\n").unwrap();
        git::git(Some(repo), &["add", "."]).unwrap();
        git::git(
            Some(repo),
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "local only",
            ],
        )
        .unwrap();
    }

    fn checkout(repo: &Path, path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        git::git(
            Some(repo),
            &["worktree", "add", "--detach", &path.to_string_lossy()],
        )
        .unwrap();
        std::fs::write(path.join("tracked"), "staged").unwrap();
        git::git(Some(path), &["add", "tracked"]).unwrap();
        std::fs::write(path.join("tracked"), "unstaged").unwrap();
        std::fs::write(path.join("untracked"), "only copy").unwrap();
        std::fs::write(path.join("ignored"), "result").unwrap();
    }

    #[test]
    fn move_and_verified_copy_preserve_projects_and_worktrees() {
        for copy in [false, true] {
            let tmp = git::TemporaryDirectory::new("orx-storage").unwrap();
            let root = normalize(tmp.path()).unwrap();
            let source = root.join("cache");
            let data = root.join("data");
            let repo = source.join("repos/owner/project");
            init(&repo);
            let store = Store::open_at(data.clone()).unwrap();
            let project = super::super::projects::create_project(
                &store,
                "test",
                &repo.to_string_lossy(),
                Default::default(),
            )
            .unwrap();
            let worktree = source.join("worktrees").join(&project.id).join("session");
            checkout(&repo, &worktree);

            let head = git::git(Some(&repo), &["rev-parse", "HEAD"]).unwrap();
            let external = root.join("external");
            init(&external);
            checkout(
                &external,
                &source.join("worktrees/owner/external/old-session"),
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::os::unix::fs::symlink("untracked", worktree.join("link")).unwrap();
                std::fs::set_permissions(
                    worktree.join("untracked"),
                    std::fs::Permissions::from_mode(0o700),
                )
                .unwrap();
                std::fs::set_permissions(
                    worktree.join("ignored"),
                    std::fs::Permissions::from_mode(0o400),
                )
                .unwrap();
            }
            let status = git::git(Some(&worktree), &["status", "--porcelain"]).unwrap();

            migrate(&source, &data, copy).unwrap();
            assert!(!source.join("repos").exists());
            assert!(!source.join("worktrees").exists());
            let moved = data.join("worktrees").join(&project.id).join("session");
            let moved_repo = data.join("repos/owner/project");
            assert_eq!(
                PathBuf::from(
                    store
                        .get_local_project(&project.id)
                        .unwrap()
                        .unwrap()
                        .repo_path
                ),
                moved_repo
            );
            assert_eq!(
                git::git(Some(&moved_repo), &["rev-parse", "HEAD"]).unwrap(),
                head
            );
            assert_eq!(
                std::fs::read_to_string(moved.join("untracked")).unwrap(),
                "only copy"
            );
            assert_eq!(
                std::fs::read_to_string(moved.join("ignored")).unwrap(),
                "result"
            );
            assert!(git::git(Some(&moved), &["status", "--porcelain"])
                .unwrap()
                .eq(&status));
            assert_eq!(
                git::common_git_dir(&moved).unwrap(),
                moved_repo.join(".git")
            );
            assert_eq!(
                git::common_git_dir(&data.join("worktrees/owner/external/old-session")).unwrap(),
                external.join(".git")
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::read_link(moved.join("link")).unwrap(),
                    Path::new("untracked")
                );
                assert_eq!(
                    std::fs::metadata(moved.join("untracked"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700
                );
                assert_eq!(
                    std::fs::metadata(moved.join("ignored"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o400
                );
            }
            assert!(references(&data, &mappings(&source, &data))
                .unwrap()
                .is_empty());
            migrate(&source, &data, copy).unwrap();
        }
    }

    #[test]
    fn interrupted_renames_repair_without_a_database_or_marker() {
        for moved_worktrees in [false, true] {
            let tmp = git::TemporaryDirectory::new("orx-interrupted").unwrap();
            let root = normalize(tmp.path()).unwrap();
            let source = root.join("cache");
            let data = root.join("data");
            let repo = source.join("repos/owner/project");
            init(&repo);
            checkout(&repo, &source.join("worktrees/owner/project/session"));
            std::fs::create_dir(&data).unwrap();
            std::fs::rename(source.join("repos"), data.join("repos")).unwrap();
            if moved_worktrees {
                std::fs::rename(source.join("worktrees"), data.join("worktrees")).unwrap();
            }
            migrate(&source, &data, false).unwrap();
            let worktree = data.join("worktrees/owner/project/session");
            assert!(git::is_repository(&worktree));
            assert_eq!(
                std::fs::read_to_string(worktree.join("untracked")).unwrap(),
                "only copy"
            );
            assert!(!data.join("orx.db").exists());
            assert!(references(&data, &mappings(&source, &data))
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn conflicts_preserve_both_copies_and_nested_dev_roots_work() {
        let tmp = git::TemporaryDirectory::new("orx-conflict").unwrap();
        let data = normalize(tmp.path()).unwrap();
        let source = data.join("cache");
        init(&source.join("repos/owner/project"));
        checkout(
            &source.join("repos/owner/project"),
            &source.join("worktrees/id/session"),
        );
        std::fs::create_dir(data.join("worktrees")).unwrap();
        std::fs::write(data.join("worktrees/keep"), "keep").unwrap();
        assert!(migrate(&source, &data, false).is_err());
        assert!(source.join("repos/owner/project/.git").exists());
        assert_eq!(
            std::fs::read_to_string(data.join("worktrees/keep")).unwrap(),
            "keep"
        );
        std::fs::remove_dir_all(data.join("worktrees")).unwrap();
        migrate(&source, &data, false).unwrap();
        assert!(git::is_repository(&data.join("worktrees/id/session")));
    }

    #[test]
    fn settled_references_are_read_only_and_skip_git_remote_commands() {
        let tmp = git::TemporaryDirectory::new("orx-settled-storage").unwrap();
        let root = normalize(tmp.path()).unwrap();
        let source = root.join("cache");
        let data = root.join("data");
        let repo = source.join("repos/o/r");
        init(&repo);
        git::git(
            Some(&repo),
            &["remote", "add", "origin", "https://example.com/o/r"],
        )
        .unwrap();
        let store = Store::open_at(data.clone()).unwrap();
        super::super::projects::create_project(
            &store,
            "test",
            &repo.to_string_lossy(),
            Default::default(),
        )
        .unwrap();
        checkout(&repo, &source.join("worktrees/id/session"));
        drop(store);
        migrate(&source, &data, false).unwrap();
        let database = data.join("orx.db");
        let before = std::fs::read(&database).unwrap();
        let modified = std::fs::metadata(&database).unwrap().modified().unwrap();
        std::fs::remove_dir(data.join("run-logs")).unwrap();
        assert!(
            !config_may_need_repair(&data.join("repos/o/r"), &mappings(&source, &data)).unwrap()
        );
        assert!(references(&data, &mappings(&source, &data))
            .unwrap()
            .is_empty());
        assert!(!data.join("run-logs").exists());
        assert_eq!(before, std::fs::read(&database).unwrap());
        assert_eq!(
            modified,
            std::fs::metadata(&database).unwrap().modified().unwrap()
        );
    }

    #[test]
    fn config_filter_defers_quoted_paths_and_non_utf8_to_git() {
        let tmp = git::TemporaryDirectory::new("orx-config-filter").unwrap();
        let root = normalize(tmp.path()).unwrap();
        let source = root.join("cache");
        let data = root.join("data");
        let repo = source.join("repos/o/r");
        init(&repo);
        let config = repo.join(".git/config");
        let mut contents = std::fs::read(&config).unwrap();
        let prefix = source.to_string_lossy().replace('\\', "/");
        contents.extend_from_slice(
            format!("\n[remote \"origin\"]\nurl = {prefix}/repos/\"o\"/r\n").as_bytes(),
        );
        std::fs::write(&config, contents).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::rename(source.join("repos"), data.join("repos")).unwrap();
        let moved_repo = data.join("repos/o/r");
        let paths = mappings(&source, &data);
        assert!(config_may_need_repair(&moved_repo, &paths).unwrap());
        repair_paths(&data, &paths).unwrap();
        assert_eq!(
            PathBuf::from(git::git(Some(&moved_repo), &["remote", "get-url", "origin"]).unwrap()),
            moved_repo
        );
        let config = moved_repo.join(".git/config");
        let mut contents = std::fs::read(&config).unwrap();
        contents.extend_from_slice(b"\n# comment: \xff\n");
        std::fs::write(config, contents).unwrap();
        assert!(config_may_need_repair(&moved_repo, &paths).unwrap());
        assert!(references(&data, &paths).unwrap().is_empty());
    }

    #[test]
    fn subsequent_data_move_repairs_git_and_database_paths() {
        let tmp = git::TemporaryDirectory::new("orx-data-move").unwrap();
        let root = normalize(tmp.path()).unwrap();
        let source = root.join("old-data");
        let target = root.join("new-data");
        let repo = source.join("repos/owner/project");
        init(&repo);
        let origin = source.join("demo-repos/local.git");
        std::fs::create_dir_all(&origin).unwrap();
        git::git(Some(&origin), &["init", "--bare"]).unwrap();
        git::git(
            Some(&repo),
            &["remote", "add", "origin", &origin.to_string_lossy()],
        )
        .unwrap();
        let store = Store::open_at(source.clone()).unwrap();
        let project = super::super::projects::create_project(
            &store,
            "test",
            &repo.to_string_lossy(),
            Default::default(),
        )
        .unwrap();
        checkout(&repo, &source.join("worktrees/id/session"));
        store.checkpoint().unwrap();
        drop(store);
        std::fs::rename(&source, &target).unwrap();
        #[cfg(unix)]
        let old_path = {
            let alias = root.join("alias");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            alias.join("old-data")
        };
        #[cfg(not(unix))]
        let old_path = source;
        let moved_repo = target.join("repos/owner/project");
        let moved_paths = [(old_path, target.clone())];
        assert!(config_may_need_repair(&moved_repo, &moved_paths).unwrap());
        repair_paths(&target, &moved_paths).unwrap();
        #[cfg(unix)]
        assert!(!config_may_need_repair(&moved_repo, &moved_paths).unwrap());
        assert!(git::is_repository(&target.join("worktrees/id/session")));
        assert_eq!(
            git::git(
                Some(&target.join("repos/owner/project")),
                &["remote", "get-url", "origin"]
            )
            .unwrap(),
            target.join("demo-repos/local.git").to_string_lossy()
        );
        assert_eq!(
            PathBuf::from(
                Store::open_at(target.clone())
                    .unwrap()
                    .get_local_project(&project.id)
                    .unwrap()
                    .unwrap()
                    .repo_path
            ),
            target.join("repos/owner/project")
        );
    }

    #[test]
    fn interrupted_reference_updates_need_no_marker() {
        for phase in ["renamed", "gitfile", "git", "database"] {
            let tmp = git::TemporaryDirectory::new("orx-reference-interruption").unwrap();
            let root = normalize(tmp.path()).unwrap();
            let source = root.join("cache");
            let data = root.join("data");
            let repo = source.join("repos/o/r");
            init(&repo);
            let store = Store::open_at(data.clone()).unwrap();
            super::super::projects::create_project(
                &store,
                "test",
                &repo.to_string_lossy(),
                Default::default(),
            )
            .unwrap();
            checkout(&repo, &source.join("worktrees/id/session"));
            for (from, to) in mappings(&source, &data) {
                std::fs::rename(from, to).unwrap();
            }
            let refs = references(&data, &mappings(&source, &data)).unwrap();
            if phase == "database" {
                store.relocate_project_paths(&refs.projects).unwrap();
            } else if phase != "renamed" {
                for (main, worktree, admin) in refs.worktrees {
                    std::fs::write(
                        worktree.join(".git"),
                        format!("gitdir: {}\n", admin.display()),
                    )
                    .unwrap();
                    if phase == "git" {
                        git::git(
                            Some(&main),
                            &["worktree", "repair", &worktree.to_string_lossy()],
                        )
                        .unwrap();
                    }
                }
            }
            migrate(&source, &data, false).unwrap();
            assert!(references(&data, &mappings(&source, &data))
                .unwrap()
                .is_empty());
            assert!(git::is_repository(&data.join("worktrees/id/session")));
        }
    }

    #[test]
    fn verification_detects_equal_length_corruption_and_leftovers_preserve_originals() {
        let tmp = git::TemporaryDirectory::new("orx-copy-failure").unwrap();
        let source = tmp.path().join("cache");
        let data = tmp.path().join("data");
        init(&source.join("repos/o/r"));
        std::fs::create_dir_all(data.join(".orx-copy-interrupted")).unwrap();
        let copied = data.join(".orx-copy-interrupted/tracked");
        std::fs::write(&copied, "corrupt!").unwrap();
        assert!(verify_file(&source.join("repos/o/r/tracked"), &copied).is_err());
        assert!(migrate(&source, &data, true)
            .unwrap_err()
            .to_string()
            .contains("interrupted"));
        assert_eq!(std::fs::read_to_string(copied).unwrap(), "corrupt!");
        assert!(git::is_repository(&source.join("repos/o/r")));
    }

    #[test]
    fn native_directories_repair_after_rename_without_rewriting_history() {
        let tmp = git::TemporaryDirectory::new("orx-native-paths").unwrap();
        let root = normalize(tmp.path()).unwrap();
        let source = root.join("cache");
        let data = root.join("data");
        init(&data.join("repos/o/r"));
        checkout(&data.join("repos/o/r"), &data.join("worktrees/id/session"));
        let db = data.join("agents/opencode/opencode.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection.execute_batch("CREATE TABLE session (id TEXT, directory TEXT); CREATE TABLE project (id TEXT, worktree TEXT); CREATE TABLE message (text TEXT); INSERT INTO session VALUES ('unrelated', '/unrelated'); INSERT INTO message VALUES ('original history');").unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES ('native', ?1)",
                [source.join("worktrees/id/session").to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO project VALUES ('native', ?1)",
                [source.join("repos/o/r").to_string_lossy()],
            )
            .unwrap();
        assert!(!references(&data, &mappings(&source, &data))
            .unwrap()
            .is_empty());
        migrate(&source, &data, false).unwrap();
        let directory: String = connection
            .query_row(
                "SELECT directory FROM session WHERE id = 'native'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(PathBuf::from(directory), data.join("worktrees/id/session"));
        assert!(references(&data, &mappings(&source, &data))
            .unwrap()
            .is_empty());
        drop(connection);

        let target = root.join("new-data");
        std::fs::rename(&data, &target).unwrap();
        repair_paths(&target, &[(data, target.clone())]).unwrap();
        let connection =
            rusqlite::Connection::open(target.join("agents/opencode/opencode.db")).unwrap();
        let directory: String = connection
            .query_row(
                "SELECT directory FROM session WHERE id = 'native'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            PathBuf::from(directory),
            target.join("worktrees/id/session")
        );
        let project: String = connection
            .query_row("SELECT worktree FROM project", [], |row| row.get(0))
            .unwrap();
        assert_eq!(PathBuf::from(project), target.join("repos/o/r"));
        let history: String = connection
            .query_row("SELECT text FROM message", [], |row| row.get(0))
            .unwrap();
        assert_eq!(history, "original history");
        let unrelated: String = connection
            .query_row(
                "SELECT directory FROM session WHERE id = 'unrelated'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unrelated, "/unrelated");
    }

    #[cfg(unix)]
    #[test]
    fn aliases_and_unrelated_broken_worktrees_do_not_strand_projects() {
        let tmp = git::TemporaryDirectory::new("orx-alias").unwrap();
        let root = normalize(tmp.path()).unwrap();
        let real = root.join("real");
        std::fs::create_dir(&real).unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let source = real.join("cache");
        let data = real.join("data");
        let repo = source.join("repos/o/r");
        init(&repo);
        let store = Store::open_at(data.clone()).unwrap();
        super::super::projects::create_project(
            &store,
            "test",
            &repo.to_string_lossy(),
            Default::default(),
        )
        .unwrap();
        store
            .relocate_project_paths(&[(
                repo.to_string_lossy().into_owned(),
                alias.join("cache/repos/o/r").to_string_lossy().into_owned(),
            )])
            .unwrap();
        let external = root.join("external");
        init(&external);
        checkout(&external, &source.join("worktrees/external/session"));
        std::fs::remove_dir_all(&external).unwrap();
        migrate(&source, &data, false).unwrap();
        assert_eq!(
            store.list_local_projects().unwrap()[0].repo_path,
            data.join("repos/o/r").to_string_lossy()
        );
        assert!(references(&data, &mappings(&source, &data))
            .unwrap()
            .is_empty());
        let broken = data.join("worktrees/external/session");
        assert!(git::ensure_worktree_at(&data.join("repos/o/r"), &broken, "HEAD").is_err());
        assert_eq!(
            std::fs::read_to_string(broken.join("untracked")).unwrap(),
            "only copy"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_copy_preserves_original_and_partial_payload() {
        let tmp = git::TemporaryDirectory::new("orx-special-file").unwrap();
        let source = tmp.path().join("cache");
        let data = tmp.path().join("data");
        init(&source.join("repos/o/r"));
        assert!(std::process::Command::new("mkfifo")
            .arg(source.join("repos/pipe"))
            .status()
            .unwrap()
            .success());
        assert!(migrate(&source, &data, true).is_err());
        assert!(source.join("repos/o/r/tracked").exists());
        assert!(migrate(&source, &data, true)
            .unwrap_err()
            .to_string()
            .contains("interrupted"));
    }
}
