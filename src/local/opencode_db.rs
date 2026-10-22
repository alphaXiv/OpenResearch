use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatabaseState {
    Empty,
    V1,
    V2Pending,
    V2Ready,
}

fn open_readonly(path: &Path) -> Result<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(connection)
}

fn has_columns(connection: &Connection, table: &str, columns: &[&str]) -> Result<bool> {
    for column in columns {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
            [table, column],
            |row| row.get(0),
        )?;
        if !exists {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn inspect(path: &Path) -> Result<DatabaseState> {
    inspect_file(path).map_err(|error| {
        anyhow!(
            "Could not inspect OpenCode database {}: {error}",
            path.display()
        )
    })
}

fn inspect_file(path: &Path) -> Result<DatabaseState> {
    match std::fs::metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DatabaseState::Empty);
        }
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.len() == 0 => return Ok(DatabaseState::Empty),
        Ok(_) => {}
    }
    inspect_connection(&open_readonly(path)?)
}

fn inspect_connection(connection: &Connection) -> Result<DatabaseState> {
    let v2_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'session_v2')",
        [],
        |row| row.get(0),
    )?;
    if v2_exists {
        if !has_columns(connection, "session_v2", &["id", "directory"])?
            || !has_columns(connection, "session_message", &["id", "session_id", "data"])?
            || !has_columns(connection, "kv", &["key", "value"])?
            || !has_columns(connection, "migration", &["id", "time_completed"])?
        {
            return Err(anyhow!(
                "Unsupported or incomplete OpenCode V2 database schema"
            ));
        }
        let schema_applied: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM migration WHERE id = '20260910120000_clear_v1_session_permission' AND time_completed IS NOT NULL)",
            [],
            |row| row.get(0),
        )?;
        if !schema_applied {
            return Ok(DatabaseState::V2Pending);
        }
        let has_legacy = has_columns(connection, "session", &["id"])?;
        let marker: Option<String> = connection
            .query_row(
                "SELECT value FROM kv WHERE key = 'migration.v1-v2'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        return match marker {
            None if !has_legacy => Ok(DatabaseState::V2Ready),
            None => Ok(DatabaseState::V2Pending),
            Some(marker) => {
                let value: serde_json::Value = serde_json::from_str(&marker)?;
                // OpenCode 2.0.1 uses only these phases; unknown formats must not admit turns.
                match value.get("phase").and_then(serde_json::Value::as_str) {
                    Some("completed") => Ok(DatabaseState::V2Ready),
                    Some("sessions") => Ok(DatabaseState::V2Pending),
                    _ => Err(anyhow!("Unsupported OpenCode V2 migration marker")),
                }
            }
        };
    }
    if has_columns(connection, "session", &["id", "directory"])?
        && has_columns(connection, "message", &["id", "session_id", "data"])?
        && has_columns(
            connection,
            "part",
            &["id", "message_id", "session_id", "data"],
        )?
    {
        return Ok(DatabaseState::V1);
    }
    let tables: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )?;
    if tables == 0 {
        Ok(DatabaseState::Empty)
    } else {
        Err(anyhow!("Unsupported OpenCode database schema"))
    }
}

fn table_has_session(connection: &Connection, table: &str, id: &str) -> Result<bool> {
    if !has_columns(connection, table, &["id"])? {
        return Ok(false);
    }
    Ok(connection.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE id = ?1)"),
        [id],
        |row| row.get(0),
    )?)
}

pub(super) fn has_session(path: &Path, id: &str) -> Result<bool> {
    let state = inspect(path)?;
    if state == DatabaseState::Empty {
        return Ok(false);
    }
    let canonical = crate::paths::canonicalize(path)?;
    let path = canonical.as_path();
    let connection = open_readonly(path)?;
    let legacy = table_has_session(&connection, "session", id)?;
    match state {
        DatabaseState::V1 => Ok(legacy),
        // Locating a session allows startup to finish migration; it does not admit a turn.
        DatabaseState::V2Pending => Ok(legacy || table_has_session(&connection, "session_v2", id)?),
        DatabaseState::V2Ready => {
            let found = table_has_session(&connection, "session_v2", id)?;
            if legacy && !found {
                let journal = read_journal(path)?;
                if journal.as_ref().is_some_and(|journal| {
                    journal.completed && journal.expected_sessions.contains(id)
                }) {
                    return Ok(false);
                }
                let recovery = journal
                    .map(|journal| journal.recovery_details(path))
                    .unwrap_or_else(|| {
                        "No OpenResearch migration journal or backup is recorded for this database. Recover the session in OpenCode, or start a new OpenResearch chat. This chat's saved transcript remains available.".to_string()
                    });
                return Err(anyhow!(
                    "OpenCode session {id} was deleted or was not migrated to V2 in {}; its prior migration was not validated, so OpenResearch cannot safely replace it. {recovery}",
                    path.display()
                ));
            }
            Ok(found)
        }
        DatabaseState::Empty => Ok(false),
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

pub(super) fn delete_sessions(
    paths: &[PathBuf],
    ids: &BTreeSet<String>,
    retained: &BTreeSet<String>,
) -> Result<()> {
    let mut leases = Vec::new();
    for path in paths.iter().collect::<BTreeSet<_>>() {
        let major = match inspect(path)? {
            DatabaseState::Empty => continue,
            DatabaseState::V1 => 1,
            DatabaseState::V2Pending | DatabaseState::V2Ready => 2,
        };
        let lease = DatabaseLease::acquire(path, major)?;
        if lease.requires_migration() {
            return Err(anyhow!(
                "Complete the OpenCode V2 upgrade before deleting chats in {}",
                path.display()
            ));
        }
        leases.push(lease);
    }
    for lease in &leases {
        delete_session_rows(lease.path(), ids, retained).map_err(|error| {
            anyhow!("Could not finish native cleanup in {}: {error}. Cleanup may be partial; retry deleting the chat or project to finish", lease.path().display())
        })?;
    }
    Ok(())
}

fn delete_session_rows(
    path: &Path,
    ids: &BTreeSet<String>,
    retained: &BTreeSet<String>,
) -> Result<()> {
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;
    let transaction =
        connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut deleting = ids.difference(retained).cloned().collect::<BTreeSet<_>>();
    let mut children = Vec::new();
    for table in ["session", "session_v2"] {
        if !has_columns(&transaction, table, &["id", "parent_id"])? {
            continue;
        }
        let mut query = transaction.prepare(&format!(
            "SELECT id, parent_id FROM {table} WHERE parent_id IS NOT NULL"
        ))?;
        children.extend(
            query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?,
        );
    }
    loop {
        let before = deleting.len();
        for (child, parent) in &children {
            if deleting.contains(parent) && !retained.contains(child) {
                deleting.insert(child.clone());
            }
        }
        if before == deleting.len() {
            break;
        }
    }
    for id in deleting {
        // Explicit deletes also cover older stores without foreign-key cascades.
        for (table, column) in [
            ("event", "aggregate_id"),
            ("event_sequence", "aggregate_id"),
            ("part", "session_id"),
            ("message", "session_id"),
            ("session_message", "session_id"),
            ("session_input", "session_id"),
            ("session_context_epoch", "session_id"),
            ("todo", "session_id"),
            ("session_share", "session_id"),
            ("session_v2", "id"),
            ("session", "id"),
        ] {
            if has_columns(&transaction, table, &[column])? {
                transaction.execute(&format!("DELETE FROM {table} WHERE {column} = ?1"), [&id])?;
            }
        }
    }
    transaction.commit()?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct MigrationJournal {
    source_state: DatabaseState,
    backup: Option<PathBuf>,
    expected_sessions: BTreeSet<String>,
    completed: bool,
    error: Option<String>,
}

impl MigrationJournal {
    fn backup_path(&self, database: &Path) -> Option<PathBuf> {
        self.backup.as_ref().map(|backup| {
            database
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(backup)
        })
    }

    fn recovery_details(&self, database: &Path) -> String {
        let backup = match self.backup_path(database) {
            Some(path) if self.source_state == DatabaseState::V1 => {
                format!("Pre-upgrade backup: {}", path.display())
            }
            Some(path) => format!("Backup captured after V2 upgrade began: {}", path.display()),
            None if self.source_state == DatabaseState::Empty => {
                "No backup was needed because the database was empty".to_string()
            }
            None => "The backup was removed after successful migration".to_string(),
        };
        format!(
            "{backup}. Migration journal: {}",
            sidecar(database, ".orx-migration.json").display()
        )
    }
}

fn read_journal(path: &Path) -> Result<Option<MigrationJournal>> {
    let journal = sidecar(path, ".orx-migration.json");
    match std::fs::read(&journal) {
        Ok(bytes) => {
            let decoded: MigrationJournal = serde_json::from_slice(&bytes).map_err(|error| {
                anyhow!(
                    "Could not read OpenCode migration journal {}: {error}",
                    journal.display()
                )
            })?;
            if let Some(backup) = &decoded.backup {
                let prefix = sidecar(path, ".orx-backup-");
                let id = backup.to_str().and_then(|name| {
                    name.strip_prefix(prefix.file_name()?.to_str()?)?
                        .strip_suffix(".db")
                });
                if id.is_none_or(|id| uuid::Uuid::parse_str(id).is_err()) {
                    return Err(anyhow!(
                        "Invalid OpenCode backup filename in {}",
                        journal.display()
                    ));
                }
            }
            Ok(Some(decoded))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(anyhow!(
            "Could not read OpenCode migration journal {}: {error}",
            journal.display()
        )),
    }
}

fn write_journal(path: &Path, journal: &MigrationJournal) -> Result<()> {
    crate::local::git::atomic_write_with_mode(
        &sidecar(path, ".orx-migration.json"),
        &serde_json::to_vec(journal)?,
        Some(0o600),
    )?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<()> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub struct DatabaseLease {
    path: PathBuf,
    lock: File,
    state: DatabaseState,
    migration: bool,
}

#[derive(Debug)]
pub struct DatabaseBusy {
    pub path: PathBuf,
}

impl std::fmt::Display for DatabaseBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "OpenCode database {} is in use or migrating. Retry when chats are idle. To complete an upgrade, send a message in an OpenCode chat or restart orx", self.path.display())
    }
}

impl std::error::Error for DatabaseBusy {}

pub fn normalize_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("OpenCode database has no parent"))?;
    std::fs::create_dir_all(parent)?;
    if path.exists() {
        Ok(crate::paths::canonicalize(path)?)
    } else {
        Ok(crate::paths::canonicalize(parent)?.join(
            path.file_name()
                .ok_or_else(|| anyhow!("OpenCode database has no filename"))?,
        ))
    }
}

impl DatabaseLease {
    pub fn acquire(path: &Path, major: u64) -> Result<Self> {
        if !matches!(major, 1 | 2) {
            return Err(anyhow!("Unsupported OpenCode major version {major}"));
        }
        let path = normalize_path(path)?;
        let state = inspect(&path)?;
        let journal = read_journal(&path)?;
        let migration = major == 2
            && (state != DatabaseState::V2Ready
                || journal.as_ref().is_some_and(|journal| !journal.completed));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(sidecar(&path, ".orx-lock"))?;
        let locked = if migration {
            lock.try_lock()
        } else {
            lock.try_lock_shared()
        };
        locked.map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => {
                anyhow::Error::new(DatabaseBusy { path: path.clone() })
            }
            std::fs::TryLockError::Error(error) => anyhow!(
                "Could not lock OpenCode database {}: {error}",
                path.display()
            ),
        })?;
        let current = inspect(&path)?;
        let journal = read_journal(&path)?;
        if current != state
            || (!migration
                && major == 2
                && journal.as_ref().is_some_and(|journal| !journal.completed))
        {
            return Err(anyhow!("OpenCode database changed during startup; retry"));
        }
        if major == 1
            && (matches!(state, DatabaseState::V2Pending | DatabaseState::V2Ready)
                || journal.is_some())
        {
            if matches!(state, DatabaseState::V1 | DatabaseState::Empty) {
                if let Some(journal) = journal {
                    return Err(anyhow!("An OpenCode V2 upgrade was prepared for this database. Use OpenCode V2 to resume, or review the saved migration state before a deliberate rollback. {}", journal.recovery_details(&path)));
                }
            }
            return Err(anyhow!("This OpenCode database has begun upgrading to V2. Use OpenCode V2 to continue; downgrading would split its history."));
        }
        if major == 2 && !migration {
            if let Some(mut journal) = journal {
                cleanup_backup(&path, &mut journal);
            }
        }
        Ok(Self {
            path,
            lock,
            state,
            migration,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn requires_migration(&self) -> bool {
        self.migration
    }

    pub fn prepare_migration(&self) -> Result<Option<PathBuf>> {
        if !self.migration {
            return Ok(None);
        }
        if self.state != DatabaseState::Empty {
            ensure_no_external_users(&self.path)?;
        }
        let expected = referenced_sessions(&self.path)?;
        self.prepare_with_sessions(expected)
    }

    fn prepare_with_sessions(
        &self,
        expected_sessions: BTreeSet<String>,
    ) -> Result<Option<PathBuf>> {
        if let Some(mut journal) = read_journal(&self.path)? {
            let mut backup = journal.backup_path(&self.path);
            if backup.as_ref().is_some_and(|backup| !backup.is_file()) {
                return Err(anyhow!(
                    "The OpenCode migration backup is missing; repair it before retrying. {}",
                    journal.recovery_details(&self.path)
                ));
            }
            if let Some(backup) = &backup {
                check_integrity(backup)?;
            }
            if !journal.expected_sessions.is_empty() {
                let current = open_readonly(&self.path)?;
                for id in &journal.expected_sessions {
                    if !table_has_session(&current, "session", id)?
                        && !table_has_session(&current, "session_v2", id)?
                    {
                        return Err(anyhow!("OpenCode database {} no longer contains saved session {id}; check whether the database was replaced before resuming migration. {}", self.path.display(), journal.recovery_details(&self.path)));
                    }
                }
            }
            if self.state == DatabaseState::V1 {
                backup = self.create_backup()?;
                journal.backup = backup
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .map(PathBuf::from);
                journal.source_state = DatabaseState::V1;
            }
            journal.expected_sessions.extend(expected_sessions);
            journal.completed = false;
            journal.error = None;
            write_journal(&self.path, &journal)?;
            return Ok(backup);
        }
        let backup = self.create_backup()?;
        write_journal(
            &self.path,
            &MigrationJournal {
                source_state: self.state,
                backup: backup
                    .as_ref()
                    .and_then(|backup| backup.file_name())
                    .map(PathBuf::from),
                expected_sessions,
                completed: false,
                error: None,
            },
        )?;
        Ok(backup)
    }

    fn create_backup(&self) -> Result<Option<PathBuf>> {
        let backup = if self.state == DatabaseState::Empty {
            None
        } else {
            let backup = sidecar(
                &self.path,
                &format!(".orx-backup-{}.db", uuid::Uuid::new_v4()),
            );
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(&backup)?;
            open_readonly(&self.path)?.backup(rusqlite::DatabaseName::Main, &backup, None)?;
            check_integrity(&backup)?;
            file.sync_all()?;
            sync_parent(&backup)?;
            Some(backup)
        };
        Ok(backup)
    }

    pub fn complete_migration(&mut self) -> Result<()> {
        if !self.migration {
            return Ok(());
        }
        let mut journal = read_journal(&self.path)?.ok_or_else(|| {
            anyhow!(
                "OpenCode migration was not prepared; journal missing: {}",
                sidecar(&self.path, ".orx-migration.json").display()
            )
        })?;
        if inspect(&self.path)? != DatabaseState::V2Ready {
            return Err(anyhow!(
                "OpenCode has not completed its native V2 migration for {}. {}",
                self.path.display(),
                journal.recovery_details(&self.path)
            ));
        }
        let connection = open_readonly(&self.path)?;
        for id in &journal.expected_sessions {
            if !table_has_session(&connection, "session_v2", id)? {
                let error = format!("OpenCode V2 did not migrate session {id} in {}; continuation is blocked until its history is repaired. {}", self.path.display(), journal.recovery_details(&self.path));
                journal.error = Some(error.clone());
                write_journal(&self.path, &journal)?;
                return Err(anyhow!(error));
            }
        }
        journal.completed = true;
        journal.error = None;
        write_journal(&self.path, &journal)?;
        self.lock.unlock()?;
        self.lock
            .try_lock_shared()
            .map_err(|error| anyhow!("OpenCode database became busy: {error}"))?;
        self.state = DatabaseState::V2Ready;
        self.migration = false;
        cleanup_backup(&self.path, &mut journal);
        Ok(())
    }

    pub fn fail_migration(&self, error: &str) -> Result<()> {
        if let Some(mut journal) = read_journal(&self.path)? {
            journal.error = Some(error.to_string());
            write_journal(&self.path, &journal)?;
        }
        Ok(())
    }
}

fn cleanup_backup(path: &Path, journal: &mut MigrationJournal) {
    if let Some(backup) = journal.backup_path(path) {
        for suffix in ["", "-wal", "-shm"] {
            let file = sidecar(&backup, suffix);
            if let Err(error) = std::fs::remove_file(&file) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("warning: OpenCode migration succeeded but backup file {} could not be removed: {error}", file.display());
                    return;
                }
            }
        }
        journal.backup = None;
        if let Err(error) = write_journal(path, journal) {
            eprintln!("warning: could not record OpenCode backup cleanup: {error}");
        }
    }
}

fn check_integrity(path: &Path) -> Result<()> {
    let result: String =
        open_readonly(path)?.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result != "ok" {
        return Err(anyhow!(
            "OpenCode migration backup {} failed integrity check: {result}",
            path.display()
        ));
    }
    Ok(())
}

fn ensure_no_external_users(database: &Path) -> Result<()> {
    let paths = [
        database.to_path_buf(),
        sidecar(database, "-wal"),
        sidecar(database, "-shm"),
    ];
    #[cfg(unix)]
    {
        let output = std::process::Command::new("lsof")
            .args(["-w", "-nP", "-Fpc", "--"])
            .args(paths.iter().filter(|path| path.exists()))
            .output()
            .map_err(|error| anyhow!("Could not check OpenCode database users with lsof: {error}. Install lsof and retry."))?;
        if !output.status.success()
            && (output.status.code() != Some(1) || !output.stderr.is_empty())
        {
            return Err(anyhow!(
                "Could not check OpenCode database users: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let own_pid = std::process::id();
        if String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.strip_prefix('p'))
            .filter_map(|pid| pid.parse::<u32>().ok())
            .any(|pid| pid != own_pid)
        {
            return Err(anyhow!("OpenCode database is open in another process. Close other OpenCode processes and retry."));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        for path in paths.iter().filter(|path| path.exists()) {
            OpenOptions::new().read(true).write(true).share_mode(0).open(path)
                .map_err(|error| anyhow!("OpenCode database is open in another process. Close other OpenCode processes and retry: {error}"))?;
        }
    }
    Ok(())
}

fn referenced_sessions(database: &Path) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    if inspect(database)? == DatabaseState::Empty {
        return Ok(ids);
    }
    let store = crate::store::Store::open()?;
    let connection = open_readonly(database)?;
    for (id, _) in store.list_chat_session_project_ids()? {
        let Some(session) = store
            .get_chat_session(&id)?
            .filter(|session| session.harness == "opencode")
        else {
            continue;
        };
        let mut candidates = Vec::new();
        candidates.extend(session.native_session_id);
        for message in store.list_chat_messages(&id)? {
            candidates.extend(message.base_native_session_id);
            candidates.extend(message.result_native_session_id);
        }
        for id in candidates {
            if table_has_session(&connection, "session", &id)?
                || table_has_session(&connection, "session_v2", &id)?
            {
                ids.insert(id);
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT);
        CREATE TABLE message (id TEXT, session_id TEXT, data TEXT);
        CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT);
        INSERT INTO session VALUES ('history', '/tmp/project');";
    const V2: &str = "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, directory TEXT);
        CREATE TABLE session_message (id TEXT, session_id TEXT, data TEXT);
        CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT);
        CREATE TABLE migration (id TEXT PRIMARY KEY, time_completed INTEGER);
        INSERT INTO migration VALUES ('20260910120000_clear_v1_session_permission', 1);";

    struct Fixture(crate::local::git::TemporaryDirectory);

    impl Fixture {
        fn new() -> Self {
            Self(crate::local::git::TemporaryDirectory::new("orx-opencode-db").unwrap())
        }

        fn path(&self) -> PathBuf {
            self.0.path().join("opencode.db")
        }
    }

    #[test]
    fn deleting_sessions_purges_content_and_children_but_preserves_other_chats() {
        for schema in [V1.to_string(), V2.to_string(), format!("{V1}{V2}")] {
            let fixture = Fixture::new();
            let path = fixture.path();
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(&schema).unwrap();
            if schema.contains("session_v2") && schema.contains("CREATE TABLE session (") {
                connection
                    .execute_batch(
                        "INSERT INTO kv VALUES ('migration.v1-v2', '{\"phase\":\"completed\"}');",
                    )
                    .unwrap();
            }
            connection.execute_batch(
                "CREATE TABLE event_sequence (aggregate_id TEXT PRIMARY KEY);
                 CREATE TABLE event (aggregate_id TEXT REFERENCES event_sequence(aggregate_id), data TEXT);",
            ).unwrap();
            for table in ["session", "session_v2"] {
                if !has_columns(&connection, table, &["id"]).unwrap() {
                    continue;
                }
                connection
                    .execute_batch(&format!(
                        "ALTER TABLE {table} ADD COLUMN parent_id TEXT;
                     INSERT OR REPLACE INTO {table} (id, directory, parent_id) VALUES
                       ('history', '/deleted', NULL), ('child', '/deleted', 'history'),
                       ('grandchild', '/deleted', 'child'),
                       ('retained', '/kept', 'history'), ('other', '/other', NULL);"
                    ))
                    .unwrap();
            }
            for id in ["history", "child", "grandchild", "retained", "other"] {
                for table in ["message", "part", "session_message"] {
                    if has_columns(&connection, table, &["session_id"]).unwrap() {
                        connection.execute(
                            &format!("INSERT INTO {table} (id, session_id, data) VALUES (?1, ?1, 'private content')"),
                            [id],
                        ).unwrap();
                    }
                }
                connection
                    .execute("INSERT INTO event_sequence VALUES (?1)", [id])
                    .unwrap();
                connection
                    .execute("INSERT INTO event VALUES (?1, 'private event')", [id])
                    .unwrap();
            }
            let ids = BTreeSet::from(["history".into()]);
            let retained = BTreeSet::from(["retained".into()]);
            delete_sessions(std::slice::from_ref(&path), &ids, &retained).unwrap();
            delete_sessions(std::slice::from_ref(&path), &ids, &retained).unwrap();
            for (table, column) in [
                ("session", "id"),
                ("session_v2", "id"),
                ("message", "session_id"),
                ("part", "session_id"),
                ("session_message", "session_id"),
                ("event", "aggregate_id"),
                ("event_sequence", "aggregate_id"),
            ] {
                if !has_columns(&connection, table, &[column]).unwrap() {
                    continue;
                }
                let mut query = connection
                    .prepare(&format!("SELECT {column} FROM {table} ORDER BY {column}"))
                    .unwrap();
                let remaining = query
                    .query_map([], |row| row.get::<_, String>(0))
                    .unwrap()
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(remaining, ["other", "retained"], "{table}");
            }
        }
    }

    #[test]
    fn native_deletion_rolls_back_on_failure_and_does_not_create_missing_databases() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let ids = BTreeSet::from(["history".into()]);
        delete_sessions(std::slice::from_ref(&path), &ids, &BTreeSet::new()).unwrap();
        assert!(!path.exists());
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(V1).unwrap();
        connection
            .execute_batch(
                "INSERT INTO message VALUES ('msg', 'history', 'private content');
             INSERT INTO part VALUES ('part', 'msg', 'history', 'private content');
             CREATE TRIGGER fail_delete BEFORE DELETE ON session
             BEGIN SELECT RAISE(ABORT, 'cannot purge'); END;",
            )
            .unwrap();
        assert!(delete_sessions(std::slice::from_ref(&path), &ids, &BTreeSet::new()).is_err());
        for table in ["session", "message", "part"] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
    }

    #[test]
    fn native_deletion_defers_to_active_and_interrupted_migrations() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(V1).unwrap();
        let ids = BTreeSet::from(["history".into()]);
        let lease = DatabaseLease::acquire(&path, 2).unwrap();
        lease.prepare_with_sessions(ids.clone()).unwrap();
        assert!(delete_sessions(std::slice::from_ref(&path), &ids, &BTreeSet::new()).is_err());
        drop(lease);
        assert!(delete_sessions(std::slice::from_ref(&path), &ids, &BTreeSet::new()).is_err());
        connection.execute_batch(V2).unwrap();
        assert!(delete_sessions(std::slice::from_ref(&path), &ids, &BTreeSet::new()).is_err());
        let lease = DatabaseLease::acquire(&path, 2).unwrap();
        lease.prepare_with_sessions(ids).unwrap();
        assert!(table_has_session(&connection, "session", "history").unwrap());
    }

    #[test]
    fn native_deletion_preflights_stores_and_can_retry_partial_cleanup() {
        let fixture = Fixture::new();
        let paths = [fixture.0.path().join("a.db"), fixture.0.path().join("b.db")];
        let first = Connection::open(&paths[0]).unwrap();
        let second = Connection::open(&paths[1]).unwrap();
        first.execute_batch(V1).unwrap();
        second
            .execute_batch("CREATE TABLE unsupported (id TEXT);")
            .unwrap();
        let ids = BTreeSet::from(["history".into()]);
        assert!(delete_sessions(&paths, &ids, &BTreeSet::new()).is_err());
        assert!(table_has_session(&first, "session", "history").unwrap());
        second.execute_batch("DROP TABLE unsupported;").unwrap();
        second.execute_batch(V1).unwrap();
        second.execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON session BEGIN SELECT RAISE(ABORT, 'cannot purge'); END;").unwrap();
        let error = delete_sessions(&paths, &ids, &BTreeSet::new())
            .unwrap_err()
            .to_string();
        assert!(error.contains("retry deleting"));
        assert!(!table_has_session(&first, "session", "history").unwrap());
        assert!(table_has_session(&second, "session", "history").unwrap());
        second.execute_batch("DROP TRIGGER fail_delete;").unwrap();
        delete_sessions(&paths, &ids, &BTreeSet::new()).unwrap();
        assert!(!table_has_session(&second, "session", "history").unwrap());
    }

    #[test]
    fn migration_backup_includes_wal_and_resume_validates_history() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        connection.execute_batch(V1).unwrap();
        assert_eq!(inspect(&path).unwrap(), DatabaseState::V1);
        let expected = BTreeSet::from(["history".to_string()]);
        let lease = DatabaseLease::acquire(&path, 2).unwrap();
        let backup = lease
            .prepare_with_sessions(expected.clone())
            .unwrap()
            .unwrap();
        assert!(table_has_session(&open_readonly(&backup).unwrap(), "session", "history").unwrap());
        assert!(DatabaseLease::acquire(&path, 1).is_err());
        assert!(DatabaseLease::acquire(&path, 2).is_err());
        connection.execute_batch(V2).unwrap();
        assert_eq!(inspect(&path).unwrap(), DatabaseState::V2Pending);
        assert!(has_session(&path, "history").unwrap());
        drop(lease);

        let mut resumed = DatabaseLease::acquire(&path, 2).unwrap();
        assert_eq!(
            resumed.prepare_with_sessions(expected).unwrap(),
            Some(backup.clone())
        );
        connection
            .execute_batch(
                "INSERT INTO kv VALUES ('migration.v1-v2', '{\"phase\":\"completed\"}');",
            )
            .unwrap();
        let error = resumed.complete_migration().unwrap_err().to_string();
        assert!(error.contains("history"));
        assert!(error.contains(&backup.display().to_string()));
        assert!(error.contains(".orx-migration.json"));
        assert!(has_session(&path, "history").is_err());
        assert!(backup.is_file());
        connection
            .execute_batch("INSERT INTO session_v2 SELECT * FROM session;")
            .unwrap();
        resumed.complete_migration().unwrap();
        assert!(!backup.exists());
        let journal = read_journal(&path).unwrap().unwrap();
        assert!(journal.completed);
        assert!(journal.backup.is_none());
        assert!(!resumed.requires_migration());
        assert!(has_session(&path, "history").unwrap());
        assert!(!has_session(&path, "missing").unwrap());
        let second = DatabaseLease::acquire(&path, 2).unwrap();
        assert!(!second.requires_migration());
        assert!(DatabaseLease::acquire(&path, 1).is_err());
        connection
            .execute_batch("DELETE FROM session_v2 WHERE id = 'history';")
            .unwrap();
        assert!(!has_session(&path, "history").unwrap());
        std::fs::remove_file(sidecar(&path, ".orx-migration.json")).unwrap();
        let error = has_session(&path, "history").unwrap_err().to_string();
        assert!(error.contains("deleted or was not migrated"));
        assert!(error.contains("No OpenResearch migration journal or backup is recorded"));
        assert!(error.contains("start a new OpenResearch chat"));
        assert!(!error.contains("Migration journal not found"));
    }

    #[test]
    fn completed_migration_resumes_backup_cleanup() {
        for already_removed in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.path();
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(V1).unwrap();
            let lease = DatabaseLease::acquire(&path, 2).unwrap();
            let backup = lease
                .prepare_with_sessions(BTreeSet::new())
                .unwrap()
                .unwrap();
            connection.execute_batch(V2).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO kv VALUES ('migration.v1-v2', '{\"phase\":\"completed\"}');",
                )
                .unwrap();
            let mut journal = read_journal(&path).unwrap().unwrap();
            journal.completed = true;
            write_journal(&path, &journal).unwrap();
            for suffix in ["-wal", "-shm"] {
                std::fs::write(sidecar(&backup, suffix), []).unwrap();
            }
            if already_removed {
                std::fs::remove_file(&backup).unwrap();
            }
            drop(lease);
            assert!(!DatabaseLease::acquire(&path, 2)
                .unwrap()
                .requires_migration());
            for suffix in ["", "-wal", "-shm"] {
                assert!(!sidecar(&backup, suffix).exists());
            }
            assert!(read_journal(&path).unwrap().unwrap().backup.is_none());
        }
    }

    #[test]
    fn journal_rejects_paths_outside_generated_backup_names() {
        let fixture = Fixture::new();
        let path = fixture.path();
        Connection::open(&path).unwrap().execute_batch(V1).unwrap();
        let lease = DatabaseLease::acquire(&path, 2).unwrap();
        let backup = lease
            .prepare_with_sessions(BTreeSet::new())
            .unwrap()
            .unwrap();
        let mut journal = read_journal(&path).unwrap().unwrap();
        for invalid in [
            backup.clone(),
            PathBuf::from("../other.db"),
            PathBuf::from("opencode.db"),
        ] {
            journal.backup = Some(invalid);
            write_journal(&path, &journal).unwrap();
            assert!(read_journal(&path).is_err());
        }
        assert!(backup.is_file());
        assert_eq!(inspect(&path).unwrap(), DatabaseState::V1);
    }

    #[test]
    fn backup_cleanup_failure_does_not_block_v2() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(V1).unwrap();
        let mut lease = DatabaseLease::acquire(&path, 2).unwrap();
        let backup = lease
            .prepare_with_sessions(BTreeSet::new())
            .unwrap()
            .unwrap();
        connection.execute_batch(V2).unwrap();
        connection
            .execute_batch(
                "INSERT INTO kv VALUES ('migration.v1-v2', '{\"phase\":\"completed\"}');",
            )
            .unwrap();
        std::fs::remove_file(&backup).unwrap();
        std::fs::create_dir(&backup).unwrap();
        lease.complete_migration().unwrap();
        assert!(backup.is_dir());
        let journal = read_journal(&path).unwrap().unwrap();
        assert!(journal.completed);
        assert_eq!(journal.backup_path(lease.path()), Some(backup));
        assert!(!DatabaseLease::acquire(&path, 2)
            .unwrap()
            .requires_migration());
    }

    #[test]
    fn fresh_database_and_incompatible_schema_never_bypass_gate() {
        let fixture = Fixture::new();
        let path = fixture.path();
        assert_eq!(inspect(&path).unwrap(), DatabaseState::Empty);
        let mut lease = DatabaseLease::acquire(&path, 2).unwrap();
        assert!(lease.requires_migration());
        assert!(lease
            .prepare_with_sessions(BTreeSet::new())
            .unwrap()
            .is_none());
        assert!(lease.complete_migration().is_err());
        drop(lease);
        assert!(DatabaseLease::acquire(&path, 1).is_err());
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(V2).unwrap();
        assert_eq!(inspect(&path).unwrap(), DatabaseState::V2Ready);
        connection.execute_batch("INSERT INTO kv VALUES ('migration.v1-v2', '{\"phase\":\"sessions\",\"cursor\":\"old\"}');").unwrap();
        assert_eq!(inspect(&path).unwrap(), DatabaseState::V2Pending);
        connection
            .execute_batch("UPDATE kv SET value = '{\"phase\":\"unexpected\"}';")
            .unwrap();
        assert!(inspect(&path)
            .unwrap_err()
            .to_string()
            .contains(&path.display().to_string()));
        assert!(DatabaseLease::acquire(&path, 1).is_err());
        let unsupported = DatabaseLease::acquire(&path, 2).err().unwrap();
        assert!(unsupported.downcast_ref::<DatabaseBusy>().is_none());
    }

    #[test]
    fn v1_readers_share_a_lease_and_block_upgrade() {
        let fixture = Fixture::new();
        let path = fixture.path();
        Connection::open(&path).unwrap().execute_batch(V1).unwrap();
        let first = DatabaseLease::acquire(&path, 1).unwrap();
        let second = DatabaseLease::acquire(&path, 1).unwrap();
        assert!(!first.requires_migration());
        let busy = DatabaseLease::acquire(&path, 2).err().unwrap();
        assert!(busy.downcast_ref::<DatabaseBusy>().is_some());
        drop((first, second));
        assert!(DatabaseLease::acquire(&path, 2).is_ok());
    }

    #[test]
    fn v1_resume_refreshes_backup_after_new_history_and_directory_relocation() {
        let fixture = Fixture::new();
        let old = fixture.0.path().join("old");
        std::fs::create_dir(&old).unwrap();
        let path = old.join("opencode.db");
        Connection::open(&path).unwrap().execute_batch(V1).unwrap();
        let lease = DatabaseLease::acquire(&path, 2).unwrap();
        let backup = lease
            .prepare_with_sessions(BTreeSet::from(["history".into()]))
            .unwrap()
            .unwrap();
        drop(lease);
        Connection::open(&path)
            .unwrap()
            .execute_batch("INSERT INTO session VALUES ('later', '/tmp/project');")
            .unwrap();
        let moved = fixture.0.path().join("moved");
        std::fs::rename(&old, &moved).unwrap();
        let lease = DatabaseLease::acquire(&moved.join("opencode.db"), 2).unwrap();
        let old_backup = crate::paths::canonicalize(&moved)
            .unwrap()
            .join(backup.file_name().unwrap());
        let refreshed = lease
            .prepare_with_sessions(BTreeSet::from(["later".into()]))
            .unwrap()
            .unwrap();
        assert_ne!(refreshed, old_backup);
        assert!(old_backup.is_file());
        assert!(
            !table_has_session(&open_readonly(&old_backup).unwrap(), "session", "later").unwrap()
        );
        assert!(
            table_has_session(&open_readonly(&refreshed).unwrap(), "session", "later").unwrap()
        );
        assert!(
            table_has_session(&open_readonly(&refreshed).unwrap(), "session", "history").unwrap()
        );
    }
}
