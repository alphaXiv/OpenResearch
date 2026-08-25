//! Provider-native state used by chats launched through OpenResearch.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStore {
    Isolated,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeSessionLocation {
    pub store: NativeStore,
    pub path: PathBuf,
}

static LINK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn user_env_path(key: &str) -> Option<PathBuf> {
    crate::local::shell_env::var(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::config::synced_env_var(key)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn opencode_db(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/opencode/opencode.db"),
        NativeStore::Legacy => user_env_path("OPENCODE_DB").unwrap_or_else(|| {
            user_env_path("XDG_DATA_HOME")
                .unwrap_or_else(|| home_dir().join(".local/share"))
                .join("opencode/opencode.db")
        }),
    }
}

pub fn claude_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/claude"),
        NativeStore::Legacy => {
            user_env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home_dir().join(".claude"))
        }
    }
}

pub fn codex_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/codex"),
        NativeStore::Legacy => {
            user_env_path("CODEX_HOME").unwrap_or_else(|| home_dir().join(".codex"))
        }
    }
}

pub fn opencode_session(native_id: &str) -> Result<Option<NativeSessionLocation>> {
    let isolated = opencode_db(NativeStore::Isolated);
    let legacy = opencode_db(NativeStore::Legacy);
    for (store, db) in [
        (NativeStore::Isolated, isolated.clone()),
        (NativeStore::Legacy, legacy),
    ] {
        if (store == NativeStore::Isolated || db != isolated)
            && opencode_has_session(&db, native_id)?
        {
            return Ok(Some(NativeSessionLocation { store, path: db }));
        }
    }
    Ok(None)
}

fn opencode_has_session(db: &Path, native_id: &str) -> Result<bool> {
    match std::fs::metadata(db) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    }
    let connection =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    match connection.query_row("SELECT 1 FROM session WHERE id = ?1", [native_id], |_| {
        Ok(())
    }) {
        Ok(()) => Ok(true),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub fn codex_sqlite_override(store: NativeStore, home: &Path) -> Option<String> {
    (store == NativeStore::Isolated).then(|| format!("sqlite_home={}", toml_string(home)))
}

pub fn claude_session(native_id: &str) -> Result<Option<NativeSessionLocation>> {
    session_location(claude_home, &["projects"], native_id)
}

pub fn codex_session(native_id: &str) -> Result<Option<NativeSessionLocation>> {
    session_location(codex_home, &["sessions", "archived_sessions"], native_id)
}

fn session_location(
    home: impl Fn(NativeStore) -> PathBuf,
    trees: &[&str],
    native_id: &str,
) -> Result<Option<NativeSessionLocation>> {
    let isolated = home(NativeStore::Isolated);
    for (store, root) in [
        (NativeStore::Isolated, isolated.clone()),
        (NativeStore::Legacy, home(NativeStore::Legacy)),
    ] {
        if store == NativeStore::Legacy && root == isolated {
            continue;
        }
        for tree in trees {
            if let Some(path) = tree_session_path(&root.join(tree), native_id)? {
                return Ok(Some(NativeSessionLocation { store, path }));
            }
        }
    }
    Ok(None)
}

fn tree_session_path(root: &Path, native_id: &str) -> Result<Option<PathBuf>> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            if let Some(path) = tree_session_path(&path, native_id)? {
                return Ok(Some(path));
            }
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".jsonl"))
            .is_some_and(|name| {
                name == native_id
                    || name
                        .strip_suffix(native_id)
                        .is_some_and(|prefix| prefix.ends_with('-'))
            })
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// Quote a path for a TOML `-c` override; JSON escaping is compatible except for DEL.
pub fn toml_string(path: &Path) -> String {
    serde_json::to_string(&path.to_string_lossy())
        .unwrap_or_else(|_| "\"\"".to_string())
        .replace('\u{7f}', "\\u007F")
}

pub fn prepare_opencode(store: NativeStore) -> Result<PathBuf> {
    let db = opencode_db(store);
    if let Some(parent) = db.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(db)
}

pub fn prepare_claude(store: NativeStore) -> Result<PathBuf> {
    let root = claude_home(store);
    let legacy = claude_home(NativeStore::Legacy);
    if store == NativeStore::Legacy || root == legacy {
        std::fs::create_dir_all(&root)?;
        return Ok(root);
    }
    prepare_links(
        &root,
        &[
            legacy.join("settings.json"),
            legacy.join("settings.local.json"),
            legacy.join("CLAUDE.md"),
            legacy.join(".credentials.json"),
            legacy.join("plugins"),
            legacy.join("skills"),
        ],
    )?;
    Ok(root)
}

pub fn prepare_codex(store: NativeStore) -> Result<PathBuf> {
    let root = codex_home(store);
    let legacy = codex_home(NativeStore::Legacy);
    if store == NativeStore::Legacy || root == legacy {
        std::fs::create_dir_all(&root)?;
        return Ok(root);
    }
    let mut sources = vec![
        legacy.join("auth.json"),
        legacy.join("config.toml"),
        legacy.join("AGENTS.md"),
        legacy.join("plugins"),
        legacy.join("skills"),
        legacy.join("prompts"),
        legacy.join("packages"),
    ];
    if let Ok(entries) = std::fs::read_dir(&legacy) {
        sources.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".config.toml"))
        }));
    }
    prepare_links(&root, &sources)?;
    Ok(root)
}

fn prepare_links(root: &Path, sources: &[PathBuf]) -> Result<()> {
    let _guard = LINK_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    std::fs::create_dir_all(root)?;
    let mut process_lock = sources
        .iter()
        .find(|source| source.exists())
        .and_then(|source| source.parent())
        .map(|parent| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(parent.join(".openresearch-native-links.lock"))
                .map(fd_lock::RwLock::new)
        })
        .transpose()?;
    let _process_guard = process_lock.as_mut().map(|lock| lock.write()).transpose()?;
    for source in sources {
        let Some(name) = source.file_name() else {
            continue;
        };
        reconcile_link(source, &root.join(name))?;
    }
    Ok(())
}

fn reconcile_link(source: &Path, destination: &Path) -> Result<()> {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let marker = destination.with_file_name(format!("{name}.orx-managed-link"));
    let source_metadata = match std::fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if std::fs::symlink_metadata(destination)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
                && std::fs::read_link(destination).is_ok_and(|target| target == source)
            {
                remove_link(destination)?;
            }
            if marker.is_file() {
                std::fs::remove_file(marker)?;
            }
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            if std::fs::read_link(destination).is_ok_and(|target| target == source) {
                write_marker(&marker, source)?;
                return Ok(());
            }
            remove_link(destination)?;
        }
        Ok(metadata) if metadata.is_file() && source_metadata.is_file() && marker.is_file() => {
            if std::fs::read_to_string(&marker).ok().as_deref() == file_hash(source)?.as_deref() {
                adopt_managed_file(destination, source)?;
            } else {
                preserve_conflict(destination)?;
            }
        }
        Ok(_) => {
            preserve_conflict(destination)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    create_symlink(source, destination).map_err(|error| {
        anyhow!(
            "could not link {} to {}: {error}",
            destination.display(),
            source.display()
        )
    })?;
    write_marker(&marker, source)?;
    Ok(())
}

fn adopt_managed_file(from: &Path, to: &Path) -> Result<()> {
    let name = to
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    let id = uuid::Uuid::new_v4();
    let backup = to.with_file_name(format!(".{name}.orx-backup"));
    let staged = to.with_file_name(format!(".{name}.orx-staged-{id}"));
    std::fs::copy(from, &staged)?;
    if backup.exists() {
        std::fs::remove_file(&backup)?;
    }
    std::fs::rename(to, &backup)?;
    if let Err(error) = std::fs::rename(&staged, to) {
        std::fs::rename(&backup, to)?;
        return Err(error.into());
    }
    std::fs::remove_file(from)?;
    Ok(())
}

fn preserve_conflict(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config");
    std::fs::rename(
        path,
        path.with_file_name(format!("{name}.orx-conflict-{}", uuid::Uuid::new_v4())),
    )?;
    Ok(())
}

fn file_hash(path: &Path) -> Result<Option<String>> {
    if path.is_file() {
        Ok(Some(format!("{:x}", Sha256::digest(std::fs::read(path)?))))
    } else {
        Ok(None)
    }
}

fn write_marker(marker: &Path, source: &Path) -> Result<()> {
    std::fs::write(marker, file_hash(source)?.unwrap_or_default())?;
    Ok(())
}

#[cfg(not(windows))]
fn remove_link(path: &Path) -> std::io::Result<()> {
    std::fs::remove_file(path)
}

#[cfg(windows)]
fn remove_link(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::FileTypeExt;

    if std::fs::symlink_metadata(path)?
        .file_type()
        .is_symlink_dir()
    {
        std::fs::remove_dir(path)
    } else {
        std::fs::remove_file(path)
    }
}

#[cfg(unix)]
fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

pub(crate) fn copy_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    let target = std::fs::read_link(source)?;
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, destination);
    #[cfg(windows)]
    if source.metadata()?.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

#[cfg(windows)]
fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(source, destination)
    } else {
        std::os::windows::fs::symlink_file(source, destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn native_store_smoke_test() {
        let root = std::env::temp_dir().join(format!("orx-native-store-{}", uuid::Uuid::new_v4()));
        let source = root.join("legacy/config.toml");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "old").unwrap();
        prepare_links(&isolated, std::slice::from_ref(&source)).unwrap();
        std::fs::remove_file(isolated.join("config.toml")).unwrap();
        std::fs::write(isolated.join("config.toml"), "new").unwrap();

        prepare_links(&isolated, std::slice::from_ref(&source)).unwrap();

        assert_eq!(std::fs::read_to_string(&source).unwrap(), "new");
        assert!(source
            .parent()
            .unwrap()
            .read_dir()
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".orx-backup")));
        assert!(std::fs::symlink_metadata(isolated.join("config.toml"))
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_file(isolated.join("config.toml")).unwrap();
        std::fs::write(isolated.join("config.toml"), "isolated change").unwrap();
        std::fs::write(&source, "legacy change").unwrap();
        prepare_links(&isolated, std::slice::from_ref(&source)).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "legacy change");
        let auth = root.join("legacy/auth.json");
        std::fs::write(&auth, "legacy").unwrap();
        std::fs::write(isolated.join("auth.json"), "isolated").unwrap();
        prepare_links(&isolated, std::slice::from_ref(&auth)).unwrap();
        assert_eq!(std::fs::read_to_string(&auth).unwrap(), "legacy");
        assert!(std::fs::read_dir(&isolated)
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains(".orx-conflict-")));
        let session = root.join("sessions/2026/08/25/rollout-session-id.jsonl");
        std::fs::create_dir_all(session.parent().unwrap()).unwrap();
        std::fs::write(session, "{}").unwrap();
        assert!(tree_session_path(&root.join("sessions"), "session-id")
            .unwrap()
            .is_some());
        assert!(tree_session_path(&root.join("sessions"), "ession-id")
            .unwrap()
            .is_none());
        let db = root.join("opencode.db");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch("CREATE TABLE session (id TEXT); INSERT INTO session VALUES ('id');")
            .unwrap();
        assert!(opencode_has_session(&db, "id").unwrap());
        assert!(!opencode_has_session(&db, "missing").unwrap());
        std::fs::remove_dir_all(root).ok();
    }
}
