//! Provider-native state used by chats launched through OpenResearch.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{anyhow, Result};

#[path = "opencode_db.rs"]
pub mod opencode_database;

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

fn user_env(key: &str) -> Option<OsString> {
    crate::local::shell_env::var(key)
        .or_else(|| crate::config::synced_env_var(key).map(|value| value.trim().into()))
}

fn user_env_path(key: &str) -> Option<PathBuf> {
    user_env(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn opencode_db(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/opencode/opencode.db"),
        NativeStore::Legacy => {
            let data = user_env_path("XDG_DATA_HOME")
                .unwrap_or_else(|| home_dir().join(".local/share"))
                .join("opencode");
            data.join(user_env_path("OPENCODE_DB").unwrap_or_else(|| PathBuf::from("opencode.db")))
        }
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

pub fn claude_secure_storage_config_dir() -> OsString {
    // Empty intentionally selects Claude's unsuffixed default Keychain namespace.
    user_env("CLAUDE_SECURESTORAGE_CONFIG_DIR")
        .or_else(|| user_env("CLAUDE_CONFIG_DIR"))
        .unwrap_or_default()
}

pub fn codex_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/codex"),
        NativeStore::Legacy => {
            user_env_path("CODEX_HOME").unwrap_or_else(|| home_dir().join(".codex"))
        }
    }
}

pub fn cursor_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/cursor"),
        NativeStore::Legacy => user_env_path("CURSOR_CONFIG_DIR")
            .or_else(|| user_env_path("XDG_CONFIG_HOME").map(|root| root.join("cursor")))
            .unwrap_or_else(|| home_dir().join(".cursor")),
    }
}

pub fn opencode_session(native_id: &str) -> Result<Option<NativeSessionLocation>> {
    let isolated = opencode_db(NativeStore::Isolated);
    let legacy = opencode_db(NativeStore::Legacy);
    for (store, db) in [
        (NativeStore::Isolated, isolated.clone()),
        (NativeStore::Legacy, legacy),
    ] {
        if store == NativeStore::Legacy && db == isolated {
            continue;
        }
        let found = opencode_has_session(&db, native_id)?;
        if found {
            return Ok(Some(NativeSessionLocation { store, path: db }));
        }
    }
    Ok(None)
}

pub(crate) struct OpenCodeRelocation {
    database: PathBuf,
    sessions: Vec<(String, String)>,
    v2_sessions: Vec<(String, String)>,
    projects: Vec<(String, String)>,
}

impl OpenCodeRelocation {
    pub(crate) fn apply(self) -> Result<()> {
        let mut connection = rusqlite::Connection::open_with_flags(
            &self.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let transaction = connection.transaction()?;
        for (table, column, paths) in [
            ("session", "directory", self.sessions),
            ("session_v2", "directory", self.v2_sessions),
            ("project", "worktree", self.projects),
        ] {
            for (old, new) in paths {
                transaction.execute(
                    &format!("UPDATE {table} SET {column} = ?2 WHERE {column} = ?1"),
                    rusqlite::params![old, new],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

pub(crate) fn opencode_relocation(
    database: &Path,
    relocate: impl Fn(&Path) -> PathBuf,
) -> Result<Option<OpenCodeRelocation>> {
    if !database.is_file() {
        return Ok(None);
    }
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut changes = OpenCodeRelocation {
        database: database.to_path_buf(),
        sessions: Vec::new(),
        v2_sessions: Vec::new(),
        projects: Vec::new(),
    };
    for (table, column, paths) in [
        ("session", "directory", &mut changes.sessions),
        ("session_v2", "directory", &mut changes.v2_sessions),
        ("project", "worktree", &mut changes.projects),
    ] {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            rusqlite::params![table, column],
            |row| row.get(0),
        )?;
        if !exists {
            continue;
        }
        let mut query = connection.prepare(&format!("SELECT DISTINCT {column} FROM {table}"))?;
        for path in query.query_map([], |row| row.get::<_, String>(0))? {
            let old = path?;
            let new = relocate(Path::new(&old));
            if new != Path::new(&old) {
                paths.push((old, new.to_string_lossy().into_owned()));
            }
        }
    }
    Ok((!changes.sessions.is_empty()
        || !changes.v2_sessions.is_empty()
        || !changes.projects.is_empty())
    .then_some(changes))
}

pub(crate) fn opencode_has_session(db: &Path, native_id: &str) -> Result<bool> {
    opencode_database::has_session(db, native_id)
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

pub fn cursor_session(native_id: &str) -> Result<Option<NativeSessionLocation>> {
    let isolated = cursor_home(NativeStore::Isolated);
    for (store, root) in [
        (NativeStore::Isolated, isolated.clone()),
        (NativeStore::Legacy, cursor_home(NativeStore::Legacy)),
    ] {
        if store == NativeStore::Legacy && root == isolated {
            continue;
        }
        let path = match cursor_session_path(&root, native_id) {
            Ok(path) => path,
            Err(_) if store == NativeStore::Legacy => continue,
            Err(error) => return Err(error),
        };
        if let Some(path) = path {
            return Ok(Some(NativeSessionLocation { store, path }));
        }
    }
    Ok(None)
}

/// Cursor CLI print-mode chats live at `chats/<workspace-md5>/<uuid>/store.db`
/// (and ACP sessions at `acp-sessions/<uuid>/`). Walk those trees only — never
/// `projects/`, which is the IDE's transcript dump and can be huge.
fn cursor_session_path(root: &Path, native_id: &str) -> Result<Option<PathBuf>> {
    for tree in ["chats", "acp-sessions"] {
        if let Some(path) = named_dir_session(&root.join(tree), native_id, 3)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn named_dir_session(root: &Path, native_id: &str, depth: usize) -> Result<Option<PathBuf>> {
    if depth == 0 {
        return Ok(None);
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_dir() {
            continue;
        }
        if path.file_name().and_then(|name| name.to_str()) == Some(native_id) {
            return Ok(Some(path));
        }
        if let Some(path) = named_dir_session(&path, native_id, depth - 1)? {
            return Ok(Some(path));
        }
    }
    Ok(None)
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
            let path = match tree_session_path(&root.join(tree), native_id) {
                Ok(path) => path,
                Err(_) if store == NativeStore::Legacy => continue,
                Err(error) => return Err(error),
            };
            if let Some(path) = path {
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

/// Remove the isolated native sessions `doomed` selects. Only stores orx owns
/// are touched: legacy homes hold the user's own (and imported) chats, and ACP
/// agents run in their global homes, so neither is ever purged. Cursor chats
/// run in the isolated `CURSOR_DATA_DIR`; only its config is read from legacy.
pub fn purge_isolated_sessions(doomed: impl Fn(&str) -> bool) -> Result<()> {
    let claude = claude_home(NativeStore::Isolated);
    for tree in ["projects", "file-history", "session-env", "todos", "debug"] {
        purge_session_tree(&claude.join(tree), &doomed)?;
    }
    purge_history(&claude.join("history.jsonl"), "sessionId", &doomed)?;

    let codex = codex_home(NativeStore::Isolated);
    let databases = match std::fs::read_dir(&codex) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let databases: Vec<_> = databases
        .into_iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("sqlite"))
        .collect();
    let (threads, children) = purge_databases(
        &databases,
        &[
            "threads.id",
            "thread_id",
            "parent_thread_id",
            "child_thread_id",
        ],
        &[],
        &[("thread_spawn_edges", "child_thread_id", "parent_thread_id")],
        &doomed,
    )?;
    let doomed_thread = |id: &str| threads.contains(id) || (doomed(id) && !children.contains(id));
    for tree in ["sessions", "archived_sessions", "shell_snapshots"] {
        purge_session_tree(&codex.join(tree), &doomed_thread)?;
    }
    purge_history(&codex.join("history.jsonl"), "session_id", &doomed_thread)?;

    purge_databases(
        &[opencode_db(NativeStore::Isolated)],
        &["session.id", "session_v2.id", "session_id"],
        &["aggregate_id"],
        &[
            ("session", "id", "parent_id"),
            ("session_v2", "id", "parent_id"),
        ],
        &doomed,
    )?;

    let cursor = cursor_home(NativeStore::Isolated);
    for tree in ["chats", "acp-sessions"] {
        purge_session_tree(&cursor.join(tree), &doomed)?;
    }
    Ok(())
}

/// The native id a session file or directory is named after: `<id>/`,
/// `<id>.jsonl`, `<id>-agent-….json`, or Codex's `rollout-<time>-<id>.jsonl`.
fn session_entry_id(name: &str, is_dir: bool) -> Option<&str> {
    // Hyphenated only: Cursor's 32-hex workspace dirs also parse as UUIDs.
    let is_id = |id: &str| id.len() == 36 && uuid::Uuid::parse_str(id).is_ok();
    if is_dir {
        return is_id(name).then_some(name);
    }
    if let Some(id) = name
        .get(..36)
        .filter(|id| is_id(id) && name[36..].starts_with(['.', '-']))
    {
        return Some(id);
    }
    let stem = name.strip_suffix(".jsonl")?;
    let id = stem.get(stem.len().checked_sub(36)?..)?;
    (stem[..stem.len() - 36].ends_with('-') && is_id(id)).then_some(id)
}

fn purge_session_tree(root: &Path, doomed: &impl Fn(&str) -> bool) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let is_dir = entry.file_type()?.is_dir();
        let name = entry.file_name();
        match name
            .to_str()
            .and_then(|name| session_entry_id(name, is_dir))
        {
            Some(id) if doomed(id) && is_dir => std::fs::remove_dir_all(&path)?,
            Some(id) if doomed(id) => std::fs::remove_file(&path)?,
            _ if is_dir => purge_session_tree(&path, doomed)?,
            _ => {}
        }
    }
    Ok(())
}

/// Drop the prompt-history lines a doomed session wrote.
fn purge_history(path: &Path, key: &str, doomed: &impl Fn(&str) -> bool) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let kept: String = text
        .lines()
        .filter(|line| {
            !serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|entry| entry.get(key)?.as_str().map(doomed))
                .unwrap_or(false)
        })
        .map(|line| format!("{line}\n"))
        .collect();
    if kept.len() != text.len() {
        std::fs::write(path, kept)?;
    }
    Ok(())
}

/// Delete every row of the doomed sessions (and their descendants via
/// `parents`: `(table, child, parent)`) from a native SQLite store. `ids`
/// columns (`column` or `table.column`) hold session ids; `also` columns are
/// only cleared. Returns the purged ids and every known child id. Rows are zeroed and the WAL truncated so no copy remains.
fn purge_databases(
    paths: &[PathBuf],
    ids: &[&str],
    also: &[&str],
    parents: &[(&str, &str, &str)],
    doomed: &impl Fn(&str) -> bool,
) -> Result<(
    std::collections::HashSet<String>,
    std::collections::HashSet<String>,
)> {
    let named = |table: &str, column: &str, names: &[&str]| {
        names
            .iter()
            .any(|name| *name == column || *name == format!("{table}.{column}"))
    };
    let mut databases = Vec::new();
    let mut candidates = std::collections::HashSet::new();
    let mut edges = Vec::new();
    for path in paths.iter().filter(|path| path.exists()) {
        let connection = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.pragma_update(None, "secure_delete", true)?;
        let columns = connection
            .prepare(
                "SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p
                 WHERE m.type = 'table'",
            )?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (table, column) in columns.iter().filter(|(t, c)| named(t, c, ids)) {
            let mut query = connection.prepare(&format!(
                "SELECT DISTINCT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL"
            ))?;
            for id in query.query_map([], |row| row.get::<_, String>(0))? {
                candidates.insert(id?);
            }
        }
        for (table, child, parent) in parents {
            if columns.iter().any(|(t, c)| t == table && c == parent) {
                let mut query = connection.prepare(&format!(
                    "SELECT \"{child}\", \"{parent}\" FROM \"{table}\" WHERE \"{parent}\" IS NOT NULL"
                ))?;
                for edge in query.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })? {
                    edges.push(edge?);
                }
            }
        }
        databases.push((connection, columns));
    }
    let parent_of: std::collections::HashMap<&str, &str> = edges
        .iter()
        .filter(|(_, parent)| candidates.contains(parent))
        .map(|(child, parent)| (child.as_str(), parent.as_str()))
        .collect();
    // orx records only top-level ids, so a child goes with its parent, never alone.
    let mut purged: std::collections::HashSet<String> = candidates
        .iter()
        .filter(|id| !parent_of.contains_key(id.as_str()) && doomed(id))
        .cloned()
        .collect();
    loop {
        let before = purged.len();
        for (child, parent) in &edges {
            if parent_of.contains_key(child.as_str()) && purged.contains(parent) {
                purged.insert(child.clone());
            }
        }
        if purged.len() == before {
            break;
        }
    }
    for (connection, columns) in databases {
        for (table, column) in columns
            .iter()
            .filter(|(t, c)| named(t, c, ids) || named(t, c, also))
        {
            for id in &purged {
                connection.execute(
                    &format!("DELETE FROM \"{table}\" WHERE \"{column}\" = ?1"),
                    [id],
                )?;
            }
        }
        // Every pass, so a checkpoint an open reader blocked last time is retried.
        let busy: bool =
            connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
        if busy {
            return Err(anyhow::anyhow!(
                "{} is busy; its WAL may still hold purged rows",
                connection.path().unwrap_or_default()
            ));
        }
    }
    let children = parent_of.into_keys().map(String::from).collect();
    Ok((purged, children))
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
        &legacy,
        &[
            home_dir().join(".claude.json"),
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
    match std::fs::read_dir(&legacy) {
        Ok(entries) => sources.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".config.toml"))
        })),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    prepare_links(&root, &legacy, &sources)?;
    Ok(crate::paths::canonicalize(&root).unwrap_or(root))
}

pub fn prepare_cursor(store: NativeStore) -> Result<PathBuf> {
    let root = cursor_home(store);
    let legacy = cursor_home(NativeStore::Legacy);
    if store == NativeStore::Legacy || root == legacy {
        std::fs::create_dir_all(&root)?;
        return Ok(root);
    }
    prepare_links(
        &root,
        &legacy,
        &[
            legacy.join("cli-config.json"),
            legacy.join("skills"),
            legacy.join("skills-cursor"),
            legacy.join("plugins"),
        ],
    )?;
    Ok(root)
}

fn prepare_links(root: &Path, lock_root: &Path, sources: &[PathBuf]) -> Result<()> {
    let _guard = LINK_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    std::fs::create_dir_all(root)?;
    let mut process_lock = sources
        .iter()
        .any(|source| source.exists())
        .then(|| {
            std::fs::create_dir_all(lock_root)?;
            Ok::<_, std::io::Error>(fd_lock::RwLock::new(
                std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(lock_root.join(".openresearch-native-links.lock"))?,
            ))
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
            // Windows' hard-link fallback is the source itself: nothing to adopt.
            if same_file::is_same_file(destination, source).unwrap_or(false) {
                write_marker(&marker, source)?;
                return Ok(());
            }
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
pub(crate) fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
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

/// Without Developer Mode, Windows refuses symlinks; a junction reads back like one,
/// a hard link does not (see `reconcile_link`).
#[cfg(windows)]
pub(crate) fn create_symlink(source: &Path, destination: &Path) -> std::io::Result<()> {
    let directory = source.is_dir();
    let attempt = if directory {
        std::os::windows::fs::symlink_dir(source, destination)
    } else {
        std::os::windows::fs::symlink_file(source, destination)
    };
    match attempt {
        Err(error)
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_PRIVILEGE_NOT_HELD as i32) => {}
        result => return result,
    }
    if directory {
        create_junction(source, destination)
    } else {
        std::fs::hard_link(source, destination)
    }
}

/// `mklink /J` is the only junction maker short of reparse-point FFI. Both paths are quoted,
/// and a Windows path cannot contain the `"` that would end its quoting.
#[cfg(windows)]
fn create_junction(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;

    let output = std::process::Command::new("cmd")
        .arg("/d")
        .arg("/c")
        .raw_arg(format!(
            "mklink /J \"{}\" \"{}\"",
            destination.display(),
            source.display()
        ))
        .output()?;
    if output.status.success() {
        return Ok(());
    }
    let detail = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    Err(std::io::Error::other(format!(
        "mklink /J: {}",
        String::from_utf8_lossy(detail).trim()
    )))
}

/// Outside the unix-only `tests`: hard links exist everywhere.
#[cfg(test)]
mod hard_link_tests {
    use super::*;

    #[test]
    fn a_hard_linked_destination_survives_an_edit_through_the_source() {
        let root = std::env::temp_dir().join(format!("orx-hard-link-{}", uuid::Uuid::new_v4()));
        let legacy = root.join("legacy");
        let isolated = root.join("isolated");
        let source = legacy.join("config.toml");
        let destination = isolated.join("config.toml");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(&source, "first").unwrap();
        std::fs::hard_link(&source, &destination).unwrap();
        write_marker(
            &destination.with_file_name("config.toml.orx-managed-link"),
            &source,
        )
        .unwrap();

        std::fs::write(&source, "second").unwrap();
        prepare_links(&isolated, &legacy, std::slice::from_ref(&source)).unwrap();

        assert!(same_file::is_same_file(&destination, &source).unwrap());
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), "second");
        assert!(!std::fs::read_dir(&isolated)
            .unwrap()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .contains(".orx-conflict-")));

        // A launch with nothing edited must not adopt the link back as a copy.
        prepare_links(&isolated, &legacy, std::slice::from_ref(&source)).unwrap();
        assert!(same_file::is_same_file(&destination, &source).unwrap());
        assert!(!legacy.join(".config.toml.orx-backup").exists());
        assert!(!std::fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn purge_removes_only_doomed_sessions() {
        let root = std::env::temp_dir().join(format!("orx-native-purge-{}", uuid::Uuid::new_v4()));
        let doomed = "01a160a3-b998-7ec2-8a30-d4aebfea7643";
        let child = "11a160a3-b998-7ec2-8a30-d4aebfea7643";
        let kept = "36bd48a2-7757-4640-b698-cbffd6adf96c";
        let project = root.join(format!("projects/-worktrees-chat-{kept}"));
        let workspace = root.join("chats/0123456789abcdef0123456789abcdef");
        for dir in [
            project.join(doomed).join("subagents"),
            root.join("sessions/2026/10"),
            root.join("todos"),
            workspace.join(kept),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for file in [
            project.join(format!("{doomed}.jsonl")),
            project.join(format!("{kept}.jsonl")),
            root.join(format!("todos/{doomed}-agent-{doomed}.json")),
            root.join(format!(
                "sessions/2026/10/rollout-2026-10-21T22-45-05-{doomed}.jsonl"
            )),
            root.join(format!(
                "sessions/2026/10/rollout-2026-10-21T22-45-05-{child}.jsonl"
            )),
            root.join(format!(
                "sessions/2026/10/rollout-2026-10-21T22-45-05-{kept}.jsonl"
            )),
        ] {
            std::fs::write(file, "secret").unwrap();
        }
        let history = root.join("history.jsonl");
        std::fs::write(
            &history,
            format!("{{\"session_id\":\"{doomed}\",\"text\":\"secret\"}}\n{{\"session_id\":\"{kept}\"}}\n"),
        )
        .unwrap();
        let db = root.join("state_5.sqlite");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE threads (id TEXT, title TEXT);
                 CREATE TABLE thread_items (thread_id TEXT, item_json TEXT);
                 CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, child_thread_id TEXT);
                 INSERT INTO thread_spawn_edges VALUES ('{doomed}', '{child}');"
            ))
            .unwrap();
        for id in [doomed, child, kept] {
            connection
                .execute("INSERT INTO threads VALUES (?1, 'secret')", [id])
                .unwrap();
            connection
                .execute("INSERT INTO thread_items VALUES (?1, 'secret')", [id])
                .unwrap();
        }
        let is_doomed = |id: &str| id != kept;
        let only_doomed = |id: &str| id == doomed;

        let (threads, _) = purge_databases(
            std::slice::from_ref(&db),
            &[
                "threads.id",
                "thread_id",
                "parent_thread_id",
                "child_thread_id",
            ],
            &[],
            &[("thread_spawn_edges", "child_thread_id", "parent_thread_id")],
            &only_doomed,
        )
        .unwrap();
        assert_eq!(threads, [doomed, child].map(String::from).into());
        let doomed_thread = |id: &str| threads.contains(id);
        for tree in ["projects", "todos", "sessions", "chats"] {
            purge_session_tree(&root.join(tree), &doomed_thread).unwrap();
        }
        purge_history(&history, "session_id", &doomed_thread).unwrap();
        // The sweep form never mistakes a workspace hash dir for a session.
        purge_session_tree(&root.join("chats"), &is_doomed).unwrap();

        assert!(project.join(format!("{kept}.jsonl")).exists());
        assert!(!project.join(format!("{doomed}.jsonl")).exists());
        assert!(!project.join(doomed).exists());
        assert!(workspace.join(kept).exists());
        assert_eq!(std::fs::read_dir(root.join("todos")).unwrap().count(), 0);
        let rollouts: Vec<_> = std::fs::read_dir(root.join("sessions/2026/10"))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            rollouts,
            [format!("rollout-2026-10-21T22-45-05-{kept}.jsonl")]
        );
        assert_eq!(
            std::fs::read_to_string(&history).unwrap(),
            format!("{{\"session_id\":\"{kept}\"}}\n")
        );
        for (table, column) in [("threads", "id"), ("thread_items", "thread_id")] {
            let ids: Vec<String> = connection
                .prepare(&format!("SELECT {column} FROM {table}"))
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .flatten()
                .collect();
            assert_eq!(ids, [kept]);
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn database_purge_takes_children_but_the_sweep_spares_live_ones() {
        let db = std::env::temp_dir().join(format!("orx-db-purge-{}.db", uuid::Uuid::new_v4()));
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (id TEXT, parent_id TEXT);
                 CREATE TABLE message (session_id TEXT, data TEXT);
                 CREATE TABLE event (aggregate_id TEXT, data TEXT);
                 INSERT INTO session VALUES ('gone', NULL), ('gone-child', 'gone'),
                   ('live', NULL), ('live-child', 'live');
                 INSERT INTO message VALUES ('gone', 'secret'), ('gone-child', 'secret'),
                   ('live', 'x'), ('live-child', 'x');
                 INSERT INTO event VALUES ('gone', 'secret'), ('live', 'x'), ('other', 'x');",
            )
            .unwrap();
        let purge = |doomed: &dyn Fn(&str) -> bool| {
            purge_databases(
                std::slice::from_ref(&db),
                &["session.id", "session_v2.id", "session_id"],
                &["aggregate_id"],
                &[
                    ("session", "id", "parent_id"),
                    ("session_v2", "id", "parent_id"),
                ],
                &doomed,
            )
            .unwrap()
        };
        let set = |ids: &[&str]| ids.iter().map(|id| id.to_string()).collect();

        let (purged, children) = purge(&|id| id == "gone");
        assert_eq!(purged, set(&["gone", "gone-child"]));
        assert_eq!(children, set(&["gone-child", "live-child"]));
        // The sweep only knows top-level ids; a live chat's child must survive it.
        assert_eq!(purge(&|id| id != "live").0, set(&[]));

        for (table, column, expected) in [
            ("session", "id", ["live", "live-child"]),
            ("message", "session_id", ["live", "live-child"]),
            ("event", "aggregate_id", ["live", "other"]),
        ] {
            let ids: Vec<String> = connection
                .prepare(&format!("SELECT {column} FROM {table} ORDER BY rowid"))
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .flatten()
                .collect();
            assert_eq!(ids, expected);
        }
        std::fs::remove_file(db).ok();
    }

    #[test]
    fn native_store_smoke_test() {
        let root = std::env::temp_dir().join(format!("orx-native-store-{}", uuid::Uuid::new_v4()));
        let source = root.join("legacy/config.toml");
        let legacy = source.parent().unwrap().to_path_buf();
        let isolated = root.join("isolated");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(&source, "old").unwrap();
        prepare_links(&isolated, &legacy, std::slice::from_ref(&source)).unwrap();
        std::fs::remove_file(isolated.join("config.toml")).unwrap();
        std::fs::write(isolated.join("config.toml"), "new").unwrap();

        prepare_links(&isolated, &legacy, std::slice::from_ref(&source)).unwrap();

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
        prepare_links(&isolated, &legacy, std::slice::from_ref(&source)).unwrap();
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "legacy change");
        let auth = root.join("legacy/auth.json");
        std::fs::write(&auth, "legacy").unwrap();
        std::fs::write(isolated.join("auth.json"), "isolated").unwrap();
        prepare_links(&isolated, &legacy, std::slice::from_ref(&auth)).unwrap();
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
        let chat_id = "e0ad13d3-d977-43a7-9994-e739975e82ec";
        let chat = root.join("chats").join("abc123def456").join(chat_id);
        std::fs::create_dir_all(&chat).unwrap();
        std::fs::write(chat.join("store.db"), []).unwrap();
        assert_eq!(
            cursor_session_path(&root, chat_id).unwrap().as_deref(),
            Some(chat.as_path())
        );
        assert!(cursor_session_path(&root, "missing-id").unwrap().is_none());
        let db = root.join("opencode.db");
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch("CREATE TABLE session (id TEXT, directory TEXT); CREATE TABLE message (id TEXT, session_id TEXT, data TEXT); CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT); INSERT INTO session VALUES ('id', '/tmp');")
            .unwrap();
        assert!(opencode_has_session(&db, "id").unwrap());
        assert!(!opencode_has_session(&db, "missing").unwrap());
        std::fs::remove_dir_all(root).ok();
    }
}
