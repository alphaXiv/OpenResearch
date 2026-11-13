//! Provider-native state used by chats launched through OpenResearch.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{anyhow, Context, Result};

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
    crate::config::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub fn opencode_db(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/opencode/opencode.db"),
        NativeStore::Legacy => {
            let data = user_env_path("XDG_DATA_HOME")
                .unwrap_or_else(|| home_dir().join(".local/share"))
                .join("opencode");
            crate::config::test_contained(
                data.join(
                    user_env_path("OPENCODE_DB").unwrap_or_else(|| PathBuf::from("opencode.db")),
                ),
            )
        }
    }
}

pub fn claude_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/claude"),
        NativeStore::Legacy => crate::config::test_contained(
            user_env_path("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home_dir().join(".claude")),
        ),
    }
}

pub fn claude_secure_storage_config_dir() -> OsString {
    let namespace =
        user_env("CLAUDE_SECURESTORAGE_CONFIG_DIR").or_else(|| user_env("CLAUDE_CONFIG_DIR"));
    if cfg!(test) {
        // A test never reaches the user's Keychain namespace.
        let namespace = namespace
            .filter(|namespace| !namespace.is_empty())
            .map_or_else(|| claude_home(NativeStore::Legacy), PathBuf::from);
        return crate::config::test_contained(namespace).into_os_string();
    }
    // Empty intentionally selects Claude's unsuffixed default Keychain namespace.
    namespace.unwrap_or_default()
}

pub fn codex_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/codex"),
        NativeStore::Legacy => crate::config::test_contained(
            user_env_path("CODEX_HOME").unwrap_or_else(|| home_dir().join(".codex")),
        ),
    }
}

pub fn cursor_home(store: NativeStore) -> PathBuf {
    match store {
        NativeStore::Isolated => crate::store::data_dir().join("agents/cursor"),
        NativeStore::Legacy => crate::config::test_contained(
            user_env_path("CURSOR_CONFIG_DIR")
                .or_else(|| user_env_path("XDG_CONFIG_HOME").map(|root| root.join("cursor")))
                .unwrap_or_else(|| home_dir().join(".cursor")),
        ),
    }
}

pub fn cursor_skills_dir() -> PathBuf {
    crate::config::test_contained(cursor_skills_dir_in(
        &home_dir(),
        user_env_path("CURSOR_CONFIG_DIR"),
    ))
}

fn cursor_skills_dir_in(home: &Path, config: Option<PathBuf>) -> PathBuf {
    // XDG redirects Cursor state, but is not a native skill search location.
    config
        .unwrap_or_else(|| home.join(".cursor"))
        .join("skills")
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

pub fn delete_opencode_sessions(
    ids: &std::collections::BTreeSet<String>,
    retained: &std::collections::BTreeSet<String>,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let isolated = opencode_db(NativeStore::Isolated);
    let legacy = opencode_db(NativeStore::Legacy);
    opencode_database::delete_sessions(&[isolated, legacy], ids, retained)
}

/// Removes Claude Code, Codex and Cursor history from orx's isolated homes only, never the user's own.
pub fn delete_isolated_sessions(
    ids: &std::collections::BTreeSet<String>,
    surviving: &std::collections::HashSet<String>,
) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    remove_isolated_sessions(&crate::store::data_dir().join("agents"), ids, surviving)
}

/// `surviving` holds every id other chats still record; their Codex sub-agents stay too.
fn remove_isolated_sessions(
    agents: &Path,
    ids: &std::collections::BTreeSet<String>,
    surviving: &std::collections::HashSet<String>,
) -> Result<()> {
    let mut deleting = ids
        .iter()
        .filter(|id| !surviving.contains(*id))
        .cloned()
        .collect::<std::collections::HashSet<_>>();
    let codex = agents.join("codex");
    // Unreadable spawn edges leave sub-agents to the sweep, which skips Codex until they can be read.
    if let (Ok(spawned), Ok(kept)) = (
        codex_spawned_threads(&codex, &deleting),
        codex_spawned_threads(&codex, surviving),
    ) {
        deleting = spawned
            .into_iter()
            .filter(|id| !kept.contains(id))
            .collect();
    }
    let files = isolated_session_files(agents)?;
    for id in &deleting {
        for path in files.get(id).into_iter().flatten() {
            remove_session_path(path)?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum IsolatedHome {
    Claude,
    Codex,
    Cursor,
}

const ISOLATED_HOMES: [IsolatedHome; 3] = [
    IsolatedHome::Claude,
    IsolatedHome::Codex,
    IsolatedHome::Cursor,
];

#[derive(Clone, Copy)]
enum SweepTarget {
    Home(IsolatedHome),
    OpenCode,
}

const SWEEP_ORDER: [SweepTarget; 4] = [
    SweepTarget::Home(IsolatedHome::Claude),
    SweepTarget::Home(IsolatedHome::Codex),
    SweepTarget::Home(IsolatedHome::Cursor),
    SweepTarget::OpenCode,
];

/// Rotates which target a sweep pass starts with, so one slow target cannot starve the others.
static NEXT_SWEEP_TARGET: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Remove orx-launched sessions no chat records once they have settled; returns whether the budget ran out.
pub(crate) fn sweep_isolated_sessions(
    agents: &Path,
    mut referenced: std::collections::HashSet<String>,
    settle: std::time::Duration,
    budget: std::time::Duration,
) -> Result<bool> {
    let deadline = std::time::Instant::now() + budget;
    // Without the spawn edges a referenced thread's sub-agents would look orphaned.
    let codex_readable = match codex_spawned_threads(&agents.join("codex"), &referenced) {
        Ok(spawned) => {
            referenced.extend(spawned);
            true
        }
        Err(_) => false,
    };
    let first = NEXT_SWEEP_TARGET.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut failure = None;
    for step in 0..SWEEP_ORDER.len() {
        // More work keeps the fast re-run; a failure is reported by a pass that finishes.
        if std::time::Instant::now() >= deadline {
            return Ok(true);
        }
        // Each target fails on its own, so an unhealthy one never holds the others back.
        let swept = match SWEEP_ORDER[(first + step) % SWEEP_ORDER.len()] {
            SweepTarget::Home(IsolatedHome::Codex) if !codex_readable => Ok(false),
            SweepTarget::Home(home) => {
                sweep_isolated_home(agents, home, &referenced, settle, deadline)
            }
            SweepTarget::OpenCode => sweep_opencode(agents, &referenced, settle).map(|()| false),
        };
        match swept {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error) => {
                failure.get_or_insert(error);
            }
        }
    }
    failure.map_or(Ok(false), Err)
}

fn sweep_isolated_home(
    agents: &Path,
    home: IsolatedHome,
    referenced: &std::collections::HashSet<String>,
    settle: std::time::Duration,
    deadline: std::time::Instant,
) -> Result<bool> {
    for (id, paths) in home_session_files(agents, home)? {
        if referenced.contains(&id)
            || !paths.iter().all(|path| {
                newest_modified(path).is_some_and(|at| at.elapsed().unwrap_or_default() >= settle)
            })
        {
            continue;
        }
        for path in &paths {
            // A path that cannot be removed is retried next pass without stalling the rest.
            let _ = remove_session_path(path);
        }
        // Checked after a removal, so a home whose walk outlasts the budget still progresses.
        if std::time::Instant::now() >= deadline {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sweep_opencode(
    agents: &Path,
    referenced: &std::collections::HashSet<String>,
    settle: std::time::Duration,
) -> Result<()> {
    let database = agents.join("opencode/opencode.db");
    if same_home(&database, &opencode_db(NativeStore::Legacy)) {
        return Ok(());
    }
    let orphans = opencode_database::stale_root_sessions(&database, settle)?
        .into_iter()
        .filter(|id| !referenced.contains(id))
        .collect::<std::collections::BTreeSet<_>>();
    if !orphans.is_empty() {
        let retained = referenced.iter().cloned().collect();
        let purged = opencode_database::delete_sessions(
            std::slice::from_ref(&database),
            &orphans,
            &retained,
        );
        // A busy database is retried next pass.
        if let Err(error) = purged {
            if error
                .downcast_ref::<opencode_database::DatabaseBusy>()
                .is_none()
            {
                return Err(error);
            }
        }
    }
    opencode_database::compact(&database)
}

/// A directory's own mtime misses writes to files already inside it, so the newest one counts.
fn newest_modified(path: &Path) -> Option<std::time::SystemTime> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    let mut newest = metadata.modified().ok()?;
    if metadata.is_dir() {
        for child in child_paths(path).ok()? {
            newest = newest.max(newest_modified(&child)?);
        }
    }
    Some(newest)
}

/// A user home pointed at orx's isolated one is the user's, so it is never purged.
fn same_home(isolated: &Path, legacy: &Path) -> bool {
    isolated == legacy
        || matches!(
            (crate::paths::canonicalize(isolated), crate::paths::canonicalize(legacy)),
            (Ok(isolated), Ok(legacy)) if isolated == legacy
        )
}

/// Session files in orx's isolated Claude Code, Codex and Cursor homes, by native id.
fn isolated_session_files(
    agents: &Path,
) -> Result<std::collections::BTreeMap<String, Vec<PathBuf>>> {
    let mut sessions = std::collections::BTreeMap::<String, Vec<PathBuf>>::new();
    for home in ISOLATED_HOMES {
        for (id, paths) in home_session_files(agents, home)? {
            sessions.entry(id).or_default().extend(paths);
        }
    }
    Ok(sessions)
}

fn home_session_files(
    agents: &Path,
    home: IsolatedHome,
) -> Result<std::collections::BTreeMap<String, Vec<PathBuf>>> {
    let mut sessions = std::collections::BTreeMap::<String, Vec<PathBuf>>::new();
    let mut add = |id: Option<&str>, path: PathBuf| {
        // Hyphenated only: Claude, Codex and Cursor ids, and nothing else in those trees.
        if let Some(id) = id.filter(|id| id.len() == 36 && uuid::Uuid::parse_str(id).is_ok()) {
            sessions.entry(id.to_string()).or_default().push(path);
        }
    };
    let name = |path: &Path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string)
    };
    match home {
        IsolatedHome::Claude => {
            let claude = agents.join("claude");
            if same_home(&claude, &claude_home(NativeStore::Legacy)) {
                return Ok(sessions);
            }
            for project in child_dirs(&claude.join("projects"))? {
                // `<id>.jsonl` is the transcript; a sibling `<id>/` holds its subagents and tool results.
                for path in child_paths(&project)? {
                    let name = name(&path);
                    add(
                        name.as_deref()
                            .map(|name| name.strip_suffix(".jsonl").unwrap_or(name)),
                        path,
                    );
                }
            }
            for tree in ["session-env", "file-history"] {
                for path in child_paths(&claude.join(tree))? {
                    add(name(&path).as_deref(), path);
                }
            }
        }
        IsolatedHome::Codex => {
            let codex = agents.join("codex");
            if same_home(&codex, &codex_home(NativeStore::Legacy)) {
                return Ok(sessions);
            }
            for tree in ["sessions", "archived_sessions"] {
                for path in tree_files(&codex.join(tree))? {
                    // Rollouts are `rollout-<timestamp>-<thread id>.jsonl`.
                    let stem = name(&path)
                        .and_then(|name| name.strip_suffix(".jsonl").map(str::to_string));
                    add(
                        stem.as_deref()
                            .and_then(|stem| stem.get(stem.len().saturating_sub(36)..)),
                        path,
                    );
                }
            }
        }
        IsolatedHome::Cursor => {
            let cursor = agents.join("cursor");
            if same_home(&cursor, &cursor_home(NativeStore::Legacy)) {
                return Ok(sessions);
            }
            for workspace in child_dirs(&cursor.join("chats"))? {
                for path in child_paths(&workspace)? {
                    add(name(&path).as_deref(), path);
                }
            }
            for path in child_paths(&cursor.join("acp-sessions"))? {
                add(name(&path).as_deref(), path);
            }
        }
    }
    Ok(sessions)
}

/// The given threads plus every sub-agent Codex spawned from them, transitively.
fn codex_spawned_threads(
    codex: &Path,
    parents: &std::collections::HashSet<String>,
) -> Result<std::collections::HashSet<String>> {
    let database = codex.join("state_5.sqlite");
    if !database.exists() {
        return Ok(parents.clone());
    }
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut query = connection.prepare(
        "WITH RECURSIVE spawned(id) AS (
            SELECT value FROM json_each(?1)
            UNION SELECT child_thread_id FROM thread_spawn_edges JOIN spawned ON parent_thread_id = spawned.id
         ) SELECT id FROM spawned",
    )?;
    let spawned = query
        .query_map([serde_json::to_string(parents)?], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(spawned)
}

/// Entries that vanish while listing are skipped: a harness may be moving them.
fn child_paths(root: &Path) -> Result<Vec<PathBuf>> {
    match std::fs::read_dir(root) {
        Ok(entries) => Ok(entries.flatten().map(|entry| entry.path()).collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn child_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    Ok(child_paths(root)?
        .into_iter()
        .filter(|path| std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir()))
        .collect())
}

fn tree_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for path in child_paths(root)? {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            files.extend(tree_files(&path)?);
        } else if metadata.is_file() {
            files.push(path);
        }
    }
    Ok(files)
}

/// Symlinks are removed, never followed.
fn remove_session_path(path: &Path) -> Result<()> {
    let result = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match result {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(anyhow!(
            "Could not remove native chat history {}: {error}",
            path.display()
        )),
        _ => Ok(()),
    }
}

pub fn codex_sqlite_override(store: NativeStore, home: &Path, profile: bool) -> Option<String> {
    let home = if profile {
        codex_home(NativeStore::Isolated)
    } else {
        home.to_path_buf()
    };
    (store == NativeStore::Isolated || profile)
        .then(|| format!("sqlite_home={}", toml_string(&home)))
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
            let path = tree_session_path(&root.join(tree), native_id).with_context(|| {
                format!(
                    "Could not read native sessions in {}",
                    root.join(tree).display()
                )
            })?;
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
    prepare_claude_links(&root, &legacy, Some(&claude_config_file()))?;
    Ok(root)
}

pub(crate) fn claude_config_file() -> PathBuf {
    if user_env_path("CLAUDE_CONFIG_DIR").is_some() {
        claude_home(NativeStore::Legacy).join(".claude.json")
    } else {
        home_dir().join(".claude.json")
    }
}

pub(crate) fn prepare_claude_links(
    root: &Path,
    legacy: &Path,
    config: Option<&Path>,
) -> Result<()> {
    let mut sources = [
        "settings.json",
        "settings.local.json",
        "CLAUDE.md",
        "plugins",
        "skills",
    ]
    .map(|name| legacy.join(name))
    .to_vec();
    if let Some(config) = config {
        sources.extend([config.to_path_buf(), legacy.join(".credentials.json")]);
    }
    prepare_links(root, legacy, &sources)
}

pub fn prepare_codex(store: NativeStore) -> Result<PathBuf> {
    let root = codex_home(store);
    let legacy = codex_home(NativeStore::Legacy);
    if store == NativeStore::Legacy || same_home(&root, &legacy) {
        std::fs::create_dir_all(&root)?;
        return Ok(root);
    }
    prepare_codex_links(&root, &legacy, true)?;
    Ok(crate::paths::canonicalize(&root).unwrap_or(root))
}

pub(crate) fn prepare_codex_links(root: &Path, legacy: &Path, share_auth: bool) -> Result<()> {
    std::fs::create_dir_all(root)?;
    let isolated = crate::paths::canonicalize(root)?;
    let mut sources = child_paths(legacy)?;
    sources.extend(child_paths(root)?.into_iter().filter_map(|path| {
        (path.extension()? == "orx-managed-link").then_some(legacy.join(path.file_stem()?))
    }));
    sources.retain(|path| {
        let name = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();

        // CODEX_HOME also holds private transcripts, runtime files and desktop state.
        let private = matches!(
            name.as_str(),
            "sessions"
                | "archived_sessions"
                | "log"
                | "logs"
                | "memories"
                | "memories_v2"
                | "shell_snapshots"
                | "thread-writer-locks"
                | "generated_images"
                | "process_manager"
                | "ipc"
                | "worktrees"
                | "automations"
                | "cache"
                | "tmp"
                | ".tmp"
                | "models_cache.json"
                | "version.json"
        ) || name.contains(".sqlite")
            || name.starts_with("history.jsonl")
            || name.starts_with("session_index.jsonl")
            || name.starts_with("db-backup-")
            || name.starts_with(".sandbox")
            || name
                .trim_start_matches('.')
                .starts_with("codex-global-state")
            || name.starts_with(".openresearch-")
            || name.contains(".orx-");
        let special = std::fs::symlink_metadata(path).is_ok_and(|metadata| {
            !metadata.is_file() && !metadata.is_dir() && !metadata.file_type().is_symlink()
        });

        (share_auth || name != "auth.json")
            && !private
            && !special
            && !crate::paths::canonicalize(path).is_ok_and(|source| isolated.starts_with(source))
    });
    sources.sort_unstable();
    sources.dedup();
    prepare_links(root, legacy, &sources)
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
            cursor_skills_dir(),
            legacy.join("skills-cursor"),
            legacy.join("plugins"),
        ],
    )?;
    Ok(root)
}

pub(crate) fn prepare_links(root: &Path, lock_root: &Path, sources: &[PathBuf]) -> Result<()> {
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
    let mut marker_name = destination.file_name().unwrap_or_default().to_os_string();
    marker_name.push(".orx-managed-link");
    let marker = destination.with_file_name(marker_name);
    let source_metadata = match std::fs::symlink_metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Ok(metadata) = std::fs::symlink_metadata(destination) {
                if metadata.file_type().is_symlink()
                    && (marker.is_file()
                        || std::fs::read_link(destination).is_ok_and(|target| target == source))
                {
                    remove_link(destination)?;
                } else if metadata.is_file() && marker.is_file() {
                    if std::fs::read_to_string(&marker).ok().as_deref()
                        == file_hash(destination)?.as_deref()
                    {
                        std::fs::remove_file(destination)?;
                    } else {
                        preserve_conflict(destination)?;
                    }
                }
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
            if !untrusted_link(destination)
                && std::fs::read_link(destination).is_ok_and(|target| target == source)
            {
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
            let previous = std::fs::read_to_string(&marker).ok();
            if previous.as_deref() == file_hash(destination)?.as_deref() {
                std::fs::remove_file(destination)?;
            } else if previous.as_deref() == file_hash(source)?.as_deref() {
                adopt_managed_file(destination, source)?;
            } else {
                preserve_conflict(destination)?;
            }
        }
        Ok(metadata)
            if cfg!(windows) && metadata.is_dir() && source.is_dir() && marker.is_file() =>
        {
            return Ok(copy_tree(source, destination)?);
        }
        Ok(_) => {
            preserve_conflict(destination)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    create_symlink(source, destination)
        .and_then(|()| {
            if !untrusted_link(destination) {
                return Ok(());
            }
            remove_link(destination)?;
            if source.is_dir() {
                copy_tree(source, destination)
            } else {
                // Hard links cannot cross volumes; the marker reconciles a copy just the same.
                std::fs::hard_link(source, destination)
                    .or_else(|_| std::fs::copy(source, destination).map(drop))
            }
        })
        .map_err(|error| {
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

/// Windows' Redirection Guard refuses to traverse reparse points a non-admin created (error 448);
/// hard links and plain copies are not reparse points.
#[cfg(windows)]
fn untrusted_link(path: &Path) -> bool {
    std::fs::metadata(path)
        .err()
        .and_then(|error| error.raw_os_error())
        == Some(windows_sys::Win32::Foundation::ERROR_UNTRUSTED_MOUNT_POINT as i32)
}

#[cfg(not(windows))]
fn untrusted_link(_path: &Path) -> bool {
    false
}

/// shortcut: merges the source over the copy on each launch, so deletions in the source are not
/// mirrored; replace with a real link if Windows ever trusts user-made reparse points again.
fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(destination)?;
    for entry in std::fs::read_dir(source)? {
        let from = entry?.path();
        let to = destination.join(from.file_name().unwrap_or_default());
        // Links the guard refuses or that dangle are unreadable to the agent anyway.
        let Ok(new) = std::fs::metadata(&from) else {
            continue;
        };
        if new.is_dir() {
            copy_tree(&from, &to)?;
            continue;
        }
        let Ok(old) = std::fs::metadata(&to) else {
            std::fs::copy(&from, &to)?;
            continue;
        };
        if old.len() != new.len() || old.modified()? != new.modified()? {
            std::fs::copy(&from, &to)?;
        }
    }
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

    let output = crate::process::command("cmd")
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

#[cfg(test)]
mod link_tests {
    use super::*;

    #[test]
    fn codex_links_relative_configuration_assets_and_reconciles_changes() {
        let root = std::env::temp_dir().join(format!("orx-codex-config-{}", uuid::Uuid::new_v4()));
        let legacy = root.join("custom-codex-home");
        let isolated = root.join("isolated");
        prepare_codex_links(&isolated, &legacy, true).unwrap();
        assert!(!legacy.exists());

        std::fs::create_dir_all(legacy.join("catalogs")).unwrap();
        for (name, contents) in [
            ("config.toml", "model_catalog_json = 'provider-models.json'"),
            ("provider-models.json", "{\"models\": []}"),
            ("catalogs/extra.json", "nested catalog"),
            ("hooks.json", "{}"),
            ("requirements.toml", ""),
            ("custom.config.toml", "model = 'custom'"),
            (".custom-instructions.md", "instructions"),
        ] {
            std::fs::write(legacy.join(name), contents).unwrap();
        }

        prepare_codex_links(&isolated, &legacy, true).unwrap();

        let config: toml::Value = std::fs::read_to_string(isolated.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let catalog = isolated.join(config["model_catalog_json"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(&catalog).unwrap(),
            "{\"models\": []}"
        );
        assert!(same_file::is_same_file(&catalog, legacy.join("provider-models.json")).unwrap());
        assert!(
            same_file::is_same_file(isolated.join("catalogs"), legacy.join("catalogs")).unwrap()
        );
        for name in [
            "hooks.json",
            "requirements.toml",
            "custom.config.toml",
            ".custom-instructions.md",
        ] {
            assert!(same_file::is_same_file(isolated.join(name), legacy.join(name)).unwrap());
        }

        std::fs::write(legacy.join("catalogs/added.json"), "new nested file").unwrap();
        assert_eq!(
            std::fs::read_to_string(isolated.join("catalogs/added.json")).unwrap(),
            "new nested file"
        );
        std::fs::write(legacy.join("new-provider.json"), "another provider").unwrap();
        std::fs::remove_file(legacy.join("provider-models.json")).unwrap();
        std::fs::write(legacy.join("provider-models.json"), "replacement catalog").unwrap();
        prepare_codex_links(&isolated, &legacy, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(&catalog).unwrap(),
            "replacement catalog"
        );
        assert_eq!(
            std::fs::read_to_string(isolated.join("new-provider.json")).unwrap(),
            "another provider"
        );

        std::fs::remove_file(legacy.join("provider-models.json")).unwrap();
        prepare_codex_links(&isolated, &legacy, true).unwrap();
        assert!(std::fs::symlink_metadata(catalog).is_err());
        assert!(!isolated
            .join("provider-models.json.orx-managed-link")
            .exists());
        assert!(!isolated.join(".openresearch-native-links.lock").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_keeps_native_and_isolated_runtime_state_separate() {
        let root = std::env::temp_dir().join(format!("orx-codex-state-{}", uuid::Uuid::new_v4()));
        let legacy = root.join("legacy");
        let isolated = root.join("isolated");
        let private = [
            "sessions/2026/11/04/rollout.jsonl",
            "archived_sessions/rollout.jsonl",
            "history.jsonl",
            "session_index.jsonl",
            "session_index.jsonl.lock",
            "state_5.sqlite",
            "state_5.sqlite-wal",
            "logs_2.sqlite-shm",
            "memories/raw_memories.md",
            "memories_v2/memory_summary.md",
            "shell_snapshots/thread.sh",
            "thread-writer-locks/thread.lock",
            "log/codex-tui.log",
            "generated_images/image.png",
            "process_manager/chat_processes.json",
            "ipc/socket",
            "worktrees/project/code.rs",
            "automations/task/automation.toml",
            "cache/data",
            ".tmp/lock",
            "tmp/arg0/launcher",
            "models_cache.json",
            "version.json",
            ".codex-global-state.json",
            "..codex-global-state.json.tmp-123",
            ".sandbox-secrets/credentials.json",
        ];
        for name in private {
            for (home, contents) in [(&legacy, "native state"), (&isolated, "orx state")] {
                let path = home.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, contents).unwrap();
            }
        }
        std::fs::write(legacy.join("state_42.sqlite-wal"), "future database").unwrap();
        std::fs::write(legacy.join("auth.json"), "sign-in").unwrap();
        std::fs::write(legacy.join("catalog.json.orx-conflict-123"), "conflict").unwrap();
        #[cfg(unix)]
        let _socket = {
            let alias = PathBuf::from("/tmp").join(format!("orx-socket-{}", uuid::Uuid::new_v4()));
            create_symlink(&legacy, &alias).unwrap();
            let socket =
                std::os::unix::net::UnixListener::bind(alias.join("runtime.sock")).unwrap();
            remove_link(&alias).unwrap();
            socket
        };

        prepare_codex_links(&isolated, &legacy, true).unwrap();
        prepare_codex_links(&isolated, &legacy, true).unwrap();

        for name in private {
            assert_eq!(
                std::fs::read_to_string(legacy.join(name)).unwrap(),
                "native state"
            );
            assert_eq!(
                std::fs::read_to_string(isolated.join(name)).unwrap(),
                "orx state"
            );
        }
        assert!(!isolated.join("state_42.sqlite-wal").exists());
        assert!(!isolated.join("catalog.json.orx-conflict-123").exists());
        assert!(!isolated.join("runtime.sock").exists());
        assert!(
            same_file::is_same_file(isolated.join("auth.json"), legacy.join("auth.json")).unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_does_not_link_an_ancestor_of_its_isolated_home() {
        let legacy =
            std::env::temp_dir().join(format!("orx-codex-nested-{}", uuid::Uuid::new_v4()));
        let isolated = legacy.join("orx/agents/codex");
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(legacy.join("config.toml"), "").unwrap();
        create_symlink(&legacy.join("orx"), &legacy.join("alias")).unwrap();

        prepare_codex_links(&isolated, &legacy, true).unwrap();

        assert!(isolated.join("config.toml").exists());
        assert!(!isolated.join("orx").exists());
        assert!(!isolated.join("alias").exists());
        std::fs::remove_dir_all(legacy).unwrap();
    }

    #[test]
    fn managed_hard_links_follow_source_replacements_and_deletions() {
        let root =
            std::env::temp_dir().join(format!("orx-codex-hard-link-{}", uuid::Uuid::new_v4()));
        let legacy = root.join("legacy");
        let isolated = root.join("isolated");
        let source = legacy.join("provider-models.json");
        let destination = isolated.join("provider-models.json");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::create_dir_all(&isolated).unwrap();
        std::fs::write(&source, "original catalog").unwrap();
        std::fs::hard_link(&source, &destination).unwrap();
        write_marker(
            &isolated.join("provider-models.json.orx-managed-link"),
            &source,
        )
        .unwrap();
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&source, "replacement catalog").unwrap();

        prepare_codex_links(&isolated, &legacy, true).unwrap();

        assert!(same_file::is_same_file(&source, &destination).unwrap());
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "replacement catalog"
        );
        assert!(!child_paths(&isolated).unwrap().iter().any(|path| path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(".orx-conflict-")));

        remove_link(&destination).unwrap();
        std::fs::hard_link(&source, &destination).unwrap();
        std::fs::remove_file(&source).unwrap();
        prepare_codex_links(&isolated, &legacy, true).unwrap();
        assert!(std::fs::symlink_metadata(&destination).is_err());

        std::fs::write(&source, "native catalog").unwrap();
        std::fs::hard_link(&source, &destination).unwrap();
        write_marker(
            &isolated.join("provider-models.json.orx-managed-link"),
            &source,
        )
        .unwrap();
        std::fs::remove_file(&source).unwrap();
        std::fs::write(&destination, "local edits").unwrap();
        prepare_codex_links(&isolated, &legacy, true).unwrap();
        assert!(!destination.exists());
        let conflict = child_paths(&isolated)
            .unwrap()
            .into_iter()
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("provider-models.json.orx-conflict-")
            })
            .unwrap();
        assert_eq!(std::fs::read_to_string(conflict).unwrap(), "local edits");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cursor_roots_and_retired_managed_links() {
        let root = std::env::temp_dir().join(format!("orx-cursor-skills-{}", uuid::Uuid::new_v4()));
        let native = cursor_skills_dir_in(&root, None);
        assert_eq!(native, root.join(".cursor/skills"));
        assert_eq!(
            cursor_skills_dir_in(&root, Some(root.join("custom"))),
            root.join("custom/skills")
        );
        let old = root.join("xdg/cursor/skills");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(&old).unwrap();
        prepare_links(&isolated, &root, std::slice::from_ref(&old)).unwrap();
        let link = isolated.join("skills");
        assert!(same_file::is_same_file(&link, &old).unwrap());
        reconcile_link(&native, &link).unwrap();
        assert!(std::fs::symlink_metadata(&link).is_err());
        assert!(old.is_dir());
        create_symlink(&old, &link).unwrap();
        reconcile_link(&native, &link).unwrap();
        assert!(same_file::is_same_file(&link, &old).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

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

    #[cfg(windows)]
    #[test]
    fn a_copied_directory_picks_up_new_source_files_and_keeps_its_own() {
        let root = std::env::temp_dir().join(format!("orx-dir-copy-{}", uuid::Uuid::new_v4()));
        let legacy = root.join("legacy");
        let isolated = root.join("isolated");
        let source = legacy.join("skills");
        let destination = isolated.join("skills");
        std::fs::create_dir_all(source.join("a")).unwrap();
        std::fs::write(source.join("a/SKILL.md"), "first").unwrap();
        copy_tree(&source, &destination).unwrap();
        std::fs::write(isolated.join("skills.orx-managed-link"), "").unwrap();
        std::fs::write(destination.join("installed.md"), "mine").unwrap();

        std::fs::write(source.join("a/SKILL.md"), "second!").unwrap();
        std::fs::write(source.join("new.md"), "new").unwrap();
        prepare_links(&isolated, &legacy, std::slice::from_ref(&source)).unwrap();

        assert!(!std::fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(destination.join("a/SKILL.md")).unwrap(),
            "second!"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("new.md")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(destination.join("installed.md")).unwrap(),
            "mine"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// Re-runs itself under the Redirection Guard mitigation, which is process-wide and permanent.
    /// Links an admin creates stay trusted, so the fallback itself only runs for non-admin users.
    #[cfg(windows)]
    #[test]
    fn untrusted_links_are_replaced_with_hard_links_and_copies() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let root = std::env::temp_dir().join(format!("orx-untrusted-{}", uuid::Uuid::new_v4()));
        let legacy = root.join("legacy");
        let isolated = root.join("isolated");
        std::fs::create_dir_all(legacy.join("skills")).unwrap();
        std::fs::write(legacy.join("config.toml"), "config").unwrap();
        std::fs::write(legacy.join("skills/SKILL.md"), "skill").unwrap();
        let sources = [legacy.join("config.toml"), legacy.join("skills")];
        // Links made before the guard was on stand in for a store an older build created.
        prepare_links(&isolated, &legacy, &sources).unwrap();
        let enforce: u32 = 1;
        assert_ne!(
            unsafe {
                windows_sys::Win32::System::Threading::SetProcessMitigationPolicy(
                    windows_sys::Win32::System::Threading::ProcessRedirectionTrustPolicy,
                    std::ptr::from_ref(&enforce).cast(),
                    std::mem::size_of_val(&enforce),
                )
            },
            0
        );
        prepare_links(&isolated, &legacy, &sources).unwrap();

        for name in ["config.toml", "skills"] {
            let path = isolated.join(name);
            assert!(!untrusted_link(&path));
        }

        assert_eq!(
            std::fs::read_to_string(isolated.join("config.toml")).unwrap(),
            "config"
        );
        assert_eq!(
            std::fs::read_to_string(isolated.join("skills/SKILL.md")).unwrap(),
            "skill"
        );
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod home_guard_tests {
    use super::*;

    const OVERRIDES: [&str; 7] = [
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
        "CLAUDE_SECURESTORAGE_CONFIG_DIR",
        "OPENCODE_DB",
        "XDG_DATA_HOME",
        "XDG_CONFIG_HOME",
        "CURSOR_CONFIG_DIR",
    ];

    fn resolve_all() -> Vec<PathBuf> {
        vec![
            codex_home(NativeStore::Legacy),
            claude_home(NativeStore::Legacy),
            PathBuf::from(claude_secure_storage_config_dir()),
            opencode_db(NativeStore::Legacy),
            cursor_home(NativeStore::Legacy),
            cursor_skills_dir(),
            crate::local::harness::xdg_config_home(),
        ]
    }

    /// A path outside every test root that this test never creates.
    fn outside() -> PathBuf {
        let temp = std::env::temp_dir();
        let filesystem_root = temp.ancestors().last().unwrap();
        filesystem_root.join(format!("orx-home-guard-{}", uuid::Uuid::new_v4()))
    }

    fn assert_panics(what: &str, resolve: impl FnOnce() -> PathBuf) {
        let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(resolve));
        assert!(resolved.is_err(), "{what} resolved {:?}", resolved.unwrap());
    }

    #[test]
    fn unset_and_empty_overrides_resolve_inside_the_test_home() {
        if !crate::config::in_test_child(&OVERRIDES) {
            return;
        }
        let home = crate::config::test_home();
        for path in resolve_all() {
            assert!(path.starts_with(&home), "{}", path.display());
        }
        for var in OVERRIDES {
            std::env::set_var(var, "");
        }
        for path in resolve_all() {
            assert!(path.starts_with(&home), "{}", path.display());
        }
    }

    #[test]
    fn the_child_environment_already_points_inside_its_temp_dir() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let temp = std::env::temp_dir();
        for var in OVERRIDES.iter().chain(&["HOME", "USERPROFILE"]) {
            let value = std::env::var_os(var).unwrap_or_else(|| panic!("{var} is unset"));
            assert!(PathBuf::from(value).starts_with(&temp), "{var}");
        }
        resolve_all();
    }

    #[test]
    fn overrides_outside_the_test_root_panic() {
        if !crate::config::in_test_child(&OVERRIDES) {
            return;
        }
        let outside = outside();
        for (var, resolve) in [
            (
                "CODEX_HOME",
                (|| codex_home(NativeStore::Legacy)) as fn() -> PathBuf,
            ),
            ("CLAUDE_CONFIG_DIR", || claude_home(NativeStore::Legacy)),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", || {
                claude_secure_storage_config_dir().into()
            }),
            ("XDG_DATA_HOME", || opencode_db(NativeStore::Legacy)),
            ("OPENCODE_DB", || opencode_db(NativeStore::Legacy)),
            ("CURSOR_CONFIG_DIR", cursor_skills_dir),
            ("XDG_CONFIG_HOME", crate::local::harness::xdg_config_home),
        ] {
            std::env::set_var(var, &outside);
            assert_panics(var, resolve);
            std::env::remove_var(var);
        }
        std::env::set_var("OPENCODE_DB", "../../../../../../../../opencode.db");
        assert_panics("a climbing OPENCODE_DB", || {
            opencode_db(NativeStore::Legacy)
        });
        std::env::remove_var("OPENCODE_DB");

        let synced = crate::config::test_home().join(".openresearch");
        std::fs::create_dir_all(&synced).unwrap();
        std::fs::write(
            synced.join("env"),
            format!("export CODEX_HOME='{}'\n", outside.display()),
        )
        .unwrap();
        assert_panics("a synced CODEX_HOME", || codex_home(NativeStore::Legacy));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_test_root_panics() {
        if !crate::config::in_test_child(&OVERRIDES) {
            return;
        }
        let temp = std::env::temp_dir();
        let escape = temp
            .parent()
            .unwrap()
            .join(format!("orx-home-guard-escape-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&escape).unwrap();
        let link = temp.join("codex-link");
        std::os::unix::fs::symlink(&escape, &link).unwrap();
        std::env::set_var("CODEX_HOME", link.join("home"));
        let escaped = std::panic::catch_unwind(|| codex_home(NativeStore::Legacy)).is_err();
        std::os::unix::fs::symlink(escape.join("gone"), temp.join("dangling")).unwrap();
        std::env::set_var("CODEX_HOME", temp.join("dangling").join("home"));
        let dangled = std::panic::catch_unwind(|| codex_home(NativeStore::Legacy)).is_err();
        std::fs::remove_dir(&escape).unwrap();
        assert!(escaped, "a symlinked CODEX_HOME escaped the test root");
        assert!(dangled, "a dangling CODEX_HOME link escaped the test root");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn copied_directories_skip_dangling_links() {
        let root = std::env::temp_dir().join(format!("orx-copy-tree-{}", uuid::Uuid::new_v4()));
        let source = root.join("skills");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "skill").unwrap();
        std::os::unix::fs::symlink(root.join("gone"), source.join("broken")).unwrap();

        copy_tree(&source, &root.join("copy")).unwrap();

        assert_eq!(
            std::fs::read_to_string(root.join("copy/SKILL.md")).unwrap(),
            "skill"
        );
        assert!(!root.join("copy/broken").exists());
        std::fs::remove_dir_all(root).unwrap();
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

    fn write_aged(path: &Path, age: std::time::Duration) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "private transcript").unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - age)
            .unwrap();
    }

    #[test]
    fn isolated_history_is_purged_by_id_and_swept_only_when_unreferenced_and_settled() {
        let root = std::env::temp_dir().join(format!("orx-native-sweep-{}", uuid::Uuid::new_v4()));
        let agents = root.join("agents");
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        let id = |n: u8| format!("00000000-0000-4000-8000-{n:012}");
        let project = agents.join("claude/projects/-tmp-chat");
        let rollout = |dir: &str, n: u8| {
            agents.join(format!(
                "codex/{dir}/rollout-2026-09-29T16-57-41-{}.jsonl",
                id(n)
            ))
        };
        let deleted = [
            project.join(format!("{}.jsonl", id(1))),
            project.join(id(1)).join("subagents/agent-a.jsonl"),
            agents.join("claude/session-env").join(id(1)).join("env"),
            agents
                .join("cursor/chats/abc123")
                .join(id(1))
                .join("store.db"),
        ];
        let orphan = project.join(format!("{}.jsonl", id(2)));
        let fresh = project.join(format!("{}.jsonl", id(3)));
        let referenced_claude = project.join(format!("{}.jsonl", id(4)));
        let orphan_rollout = rollout("sessions/2026/09/29", 5);
        let referenced_rollout = rollout("archived_sessions", 6);
        let spawned_rollout = rollout("sessions/2026/09/29", 7);
        let not_a_session = project.join("notes.jsonl");
        for path in deleted.iter().chain([
            &orphan,
            &referenced_claude,
            &orphan_rollout,
            &referenced_rollout,
            &spawned_rollout,
            &not_a_session,
        ]) {
            write_aged(path, 2 * day);
        }
        write_aged(&fresh, std::time::Duration::ZERO);
        let active_chat = agents.join("cursor/chats/abc123").join(id(8));
        write_aged(&active_chat.join("store.db"), std::time::Duration::ZERO);
        std::fs::File::open(&active_chat)
            .unwrap()
            .set_modified(std::time::SystemTime::now() - 2 * day)
            .unwrap();
        let (parent, child, kept_child) = (
            rollout("sessions/2026/09/29", 9),
            rollout("sessions/2026/09/29", 10),
            rollout("sessions/2026/09/29", 11),
        );
        for path in [&parent, &child, &kept_child] {
            write_aged(path, 2 * day);
        }
        let state = rusqlite::Connection::open(agents.join("codex/state_5.sqlite")).unwrap();
        state
            .execute_batch("CREATE TABLE thread_spawn_edges (parent_thread_id TEXT, child_thread_id TEXT, status TEXT);")
            .unwrap();
        for (parent, child) in [(6, 7), (9, 10), (9, 11)] {
            state
                .execute(
                    "INSERT INTO thread_spawn_edges VALUES (?1, ?2, 'done')",
                    [id(parent), id(child)],
                )
                .unwrap();
        }
        drop(state);

        let none = std::collections::HashSet::new();
        remove_isolated_sessions(&agents, &[id(1)].into(), &none).unwrap();
        assert!(deleted.iter().all(|path| !path.exists()));
        assert!(!project.join(id(1)).exists());
        assert!(orphan.exists());
        remove_isolated_sessions(&agents, &[id(9)].into(), &[id(11)].into()).unwrap();
        assert!(
            !parent.exists() && !child.exists(),
            "sub-agents go with their parent"
        );
        assert!(kept_child.exists(), "a surviving chat still records it");

        let referenced = || [id(4), id(6)].into_iter().collect();
        assert!(
            sweep_isolated_sessions(&agents, referenced(), day, std::time::Duration::ZERO).unwrap()
        );
        assert!(orphan.exists(), "an exhausted budget removes nothing");
        assert!(!sweep_isolated_sessions(&agents, referenced(), day, day).unwrap());
        assert!(!orphan.exists() && !orphan_rollout.exists());
        for kept in [
            &fresh,
            &referenced_claude,
            &referenced_rollout,
            &spawned_rollout,
            &not_a_session,
            &active_chat,
        ] {
            assert!(kept.exists(), "{}", kept.display());
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn opencode_sweep_purges_settled_unreferenced_roots_and_compacts() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("orx-opencode-sweep-{}", uuid::Uuid::new_v4()));
        let agents = root.join("agents");
        let db = agents.join("opencode/opencode.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let day = std::time::Duration::from_secs(24 * 60 * 60);
        let old = crate::store::now_ms() - 2 * 24 * 60 * 60 * 1000;
        let connection = rusqlite::Connection::open(&db).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, parent_id TEXT, time_updated INTEGER);
                 CREATE TABLE message (id TEXT, session_id TEXT, data TEXT);
                 CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT);
                 INSERT INTO session VALUES ('orphan', '/tmp', NULL, {old}), ('child', '/tmp', 'orphan', {old}),
                   ('kept', '/tmp', NULL, {old}), ('fresh', '/tmp', NULL, {now});",
                now = crate::store::now_ms()
            ))
            .unwrap();
        for id in ["orphan", "child", "kept", "fresh"] {
            connection
                .execute(
                    "INSERT INTO message VALUES (?1, ?1, ?2)",
                    rusqlite::params![id, "x".repeat(64 * 1024)],
                )
                .unwrap();
        }
        drop(connection);
        let before = std::fs::metadata(&db).unwrap().len();
        let referenced = ["kept".to_string()].into_iter().collect();
        sweep_isolated_sessions(&agents, referenced, day, day).unwrap();
        let connection = rusqlite::Connection::open(&db).unwrap();
        let mut query = connection
            .prepare("SELECT id FROM session ORDER BY id")
            .unwrap();
        let remaining = query
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(remaining, ["fresh", "kept"]);
        assert!(std::fs::metadata(&db).unwrap().len() < before);
        std::fs::remove_dir_all(root).ok();
    }
}
