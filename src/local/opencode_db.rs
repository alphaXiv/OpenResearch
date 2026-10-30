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

/// The tables that carry the user's OpenCode sign-ins, in foreign-key order
/// (`account_state` points at `account`).
const CREDENTIAL_TABLES: [&str; 4] = ["credential", "account", "account_state", "control_account"];

/// Copy the user's OpenCode sign-ins into orx's isolated database.
///
/// WHY THIS EXISTS. orx runs OpenCode against its own database and passes it as
/// `OPENCODE_DB`, but OpenCode does not honour that variable for credentials:
/// `opencode auth list` prints the same accounts with and without it, because
/// they live in the `credential` table of the user's own database
/// (`~/.local/share/opencode/opencode.db`). So the isolated database starts with
/// none, the server orx spawns reports no connected integration, the harness
/// reads as signed out while the user is plainly signed in, and its catalog
/// collapses to the anonymous free models. `source` must therefore be that
/// default database — not the `OPENCODE_DB` override, which OpenCode ignores for
/// credentials even though it honours it for sessions. Signing in from the harness cannot
/// repair it either, because that login writes back to the user's own database
/// and the isolated one stays empty.
///
/// One-way and additive. The user's database is opened read-only and never
/// written. A row already present in the isolated store is updated in place,
/// never deleted and re-inserted: a replacement that half-succeeds would drop a
/// working sign-in. What "present" means is the table's own primary key
/// (`control_account` is keyed by `(email, url)`, not by `id`), read from the
/// schema rather than assumed.
///
/// Update policy is per table, because the tables do not agree on what "newer"
/// means:
///
/// * with a `time_updated` column (`credential`, `account`,
///   `control_account`), the user's copy wins only when it is strictly newer, so
///   a token the isolated server refreshed during a turn is not reverted by the
///   next startup while a deliberate re-login still lands;
/// * without one (`account_state`, the singleton pointer to the active account),
///   the user's database is mirrored — switching accounts must not leave the
///   isolated store pointing at the old one.
///
/// Repeated calls on every startup and detection sweep are therefore cheap and
/// idempotent.
///
/// Only an isolated database that already exists is touched. OpenCode creates the
/// schema itself on first boot, so a fresh install is seeded by the detection
/// sweep that follows the one which created it — never by pre-creating tables
/// in a database OpenCode is about to migrate, which would collide with its own
/// migrations.
///
/// Schema drift is tolerated rather than assumed away: only the columns both
/// sides have are copied, and a table whose key the two schemas disagree on is
/// skipped. A database written by another OpenCode version must not be able to
/// fail startup here.
pub fn inherit_credentials(database: &Path, source: &Path) -> Result<()> {
    if database == source || !source.is_file() || !database.is_file() {
        return Ok(());
    }
    let reader = open_readonly(source)?;
    // One snapshot across every table. Reading them independently lets a
    // concurrent sign-in commit `account` between the two reads, so
    // `account_state` could name an account this copy never saw and the isolated
    // store would point at a missing row.
    let snapshot = reader.unchecked_transaction()?;
    let mut writer = Connection::open(database)?;
    writer.busy_timeout(std::time::Duration::from_secs(5))?;
    let transaction = writer.transaction()?;
    for table in CREDENTIAL_TABLES {
        let columns = shared_columns(&transaction, &snapshot, table)?;
        if columns.is_empty() {
            continue;
        }
        // Without an agreed key every row would look new on every start, and the
        // isolated store would silently accumulate duplicates.
        let keys = key_columns(&transaction, &reader, table, &columns)?;
        if keys.is_empty() {
            continue;
        }
        let updated = columns.iter().position(|column| column == "time_updated");
        let insert = format!(
            "INSERT OR IGNORE INTO {table} ({}) VALUES ({})",
            columns
                .iter()
                .map(|column| quote_identifier(column))
                .collect::<Vec<_>>()
                .join(", "),
            (1..=columns.len())
                .map(|index| format!("?{index}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let settable: Vec<(usize, &String)> = columns
            .iter()
            .enumerate()
            .filter(|(index, _)| !keys.contains(index))
            .collect();
        let update = (!settable.is_empty()).then(|| {
            format!(
                "UPDATE {table} SET {} WHERE {}",
                settable
                    .iter()
                    .enumerate()
                    .map(|(position, (_, column))| format!(
                        "{} = ?{}",
                        quote_identifier(column),
                        position + 1
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                keys.iter()
                    .enumerate()
                    .map(|(position, index)| {
                        format!(
                            "{} = ?{}",
                            quote_identifier(&columns[*index]),
                            settable.len() + position + 1
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ")
            )
        });
        let lookup = format!(
            "SELECT {} FROM {table} WHERE {}",
            updated
                .map(|index| quote_identifier(&columns[index]))
                .unwrap_or_else(|| "1".to_string()),
            keys.iter()
                .enumerate()
                .map(|(position, index)| {
                    format!("{} = ?{}", quote_identifier(&columns[*index]), position + 1)
                })
                .collect::<Vec<_>>()
                .join(" AND ")
        );
        for row in read_rows(&snapshot, table, &columns)? {
            let current: Option<Option<i64>> = transaction
                .query_row(
                    &lookup,
                    rusqlite::params_from_iter(keys.iter().map(|index| &row[*index])),
                    |row| row.get(0),
                )
                .optional()?;
            let Some(current) = current else {
                // Absent: nothing to lose, and a row the target schema rejects is
                // left out rather than failing the whole copy.
                transaction.execute(&insert, rusqlite::params_from_iter(row))?;
                continue;
            };
            let is_newer = updated.is_none_or(|index| match &row[index] {
                // A malformed timestamp must not outrank a real one; treat it as
                // the oldest possible copy.
                rusqlite::types::Value::Integer(value) => *value > current.unwrap_or_default(),
                _ => false,
            });
            if !is_newer {
                continue;
            }
            let Some(update) = &update else { continue };
            let mut values: Vec<&rusqlite::types::Value> =
                settable.iter().map(|(index, _)| &row[*index]).collect();
            values.extend(keys.iter().map(|index| &row[*index]));
            transaction.execute(update, rusqlite::params_from_iter(values))?;
        }
    }
    transaction.commit()?;
    // The snapshot is read-only; committing it just releases the read lock.
    snapshot.commit()?;
    Ok(())
}

/// The indexes into `columns` of this table's primary key — the columns both
/// schemas declare as key, in key order. Empty when they disagree or declare
/// none, which skips the table instead of guessing a key that could duplicate or
/// overwrite the wrong row.
fn key_columns(
    target: &Connection,
    source: &Connection,
    table: &str,
    columns: &[String],
) -> Result<Vec<usize>> {
    let source_key = primary_key(source, table)?;
    if source_key.is_empty() || source_key != primary_key(target, table)? {
        return Ok(Vec::new());
    }
    Ok(source_key
        .iter()
        .filter_map(|name| columns.iter().position(|column| column == name))
        .collect())
}

fn primary_key(connection: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement =
        connection.prepare("SELECT name FROM pragma_table_info(?1) WHERE pk > 0 ORDER BY pk")?;
    let key = statement
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(key)
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement =
        connection.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid")?;
    let columns = statement
        .query_map([table], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns)
}

/// Columns both sides have, in the source's order. Empty when either side lacks
/// the table, which is how a table that only one OpenCode version writes is
/// skipped instead of failing the copy.
fn shared_columns(target: &Connection, source: &Connection, table: &str) -> Result<Vec<String>> {
    let source_columns = table_columns(source, table)?;
    if source_columns.is_empty() {
        return Ok(Vec::new());
    }
    let target_columns = table_columns(target, table)?;
    Ok(source_columns
        .into_iter()
        .filter(|column| target_columns.contains(column))
        .collect())
}

fn read_rows(
    connection: &Connection,
    table: &str,
    columns: &[String],
) -> Result<Vec<Vec<rusqlite::types::Value>>> {
    let mut statement = connection.prepare(&format!(
        "SELECT {} FROM {table}",
        columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let rows = statement
        .query_map([], |row| {
            (0..columns.len())
                .map(|index| row.get::<_, rusqlite::types::Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
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
    // Zero deleted content in place so chat text does not linger in free pages.
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA secure_delete = ON;")?;
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
    // Older WAL frames still hold the deleted text; waiting out a live reader would stall
    // OpenCode's writers, so the periodic `compact` of orx's own database retries it.
    connection.busy_timeout(std::time::Duration::ZERO)?;
    let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    Ok(())
}

/// Top-level sessions untouched for `settle`; children go with their parent.
pub(super) fn stale_root_sessions(
    path: &Path,
    settle: std::time::Duration,
) -> Result<BTreeSet<String>> {
    let mut ids = BTreeSet::new();
    if inspect(path)? == DatabaseState::Empty {
        return Ok(ids);
    }
    let connection = open_readonly(path)?;
    let cutoff = crate::store::now_ms() - i64::try_from(settle.as_millis())?;
    for table in ["session", "session_v2"] {
        if !has_columns(&connection, table, &["id", "parent_id", "time_updated"])? {
            continue;
        }
        let mut query = connection.prepare(&format!(
            "SELECT id FROM {table} WHERE parent_id IS NULL AND time_updated < ?1"
        ))?;
        for id in query.query_map([cutoff], |row| row.get::<_, String>(0))? {
            ids.insert(id?);
        }
    }
    Ok(ids)
}

/// Truncates the WAL without waiting on readers, then VACUUMs once free pages are a tenth of the file and no OpenCode holds it.
pub(super) fn compact(path: &Path) -> Result<()> {
    if inspect(path)? == DatabaseState::Empty {
        return Ok(());
    }
    let path = normalize_path(path)?;
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(std::time::Duration::ZERO)?;
    let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    let (free, pages): (i64, i64) = connection.query_row(
        "SELECT freelist_count, page_count FROM pragma_freelist_count(), pragma_page_count()",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if free == 0 || free * 10 < pages {
        return Ok(());
    }
    // orx's OpenCode children hold a shared lease, so an exclusive one means none is running.
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(sidecar(&path, ".orx-lock"))?;
    // Windows' exclusive-open check would see this process's own connection as another user.
    drop(connection);
    if lock.try_lock().is_err() || ensure_no_external_users(&path).is_err() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute_batch("VACUUM;")?;
    let _ = connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
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
    fn deleted_session_text_does_not_linger_in_the_file_or_wal() {
        let fixture = Fixture::new();
        let path = fixture.path();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(&format!(
                "PRAGMA journal_mode=WAL; {V1}
                 INSERT INTO message VALUES ('msg', 'history', 'deleted-secret-marker');"
            ))
            .unwrap();
        delete_sessions(
            std::slice::from_ref(&path),
            &BTreeSet::from(["history".into()]),
            &BTreeSet::new(),
        )
        .unwrap();
        drop(connection);
        for suffix in ["", "-wal"] {
            let bytes = std::fs::read(sidecar(&path, suffix)).unwrap_or_default();
            assert!(
                !bytes
                    .windows(b"deleted-secret-marker".len())
                    .any(|window| window == b"deleted-secret-marker"),
                "{suffix}"
            );
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

    const CREDENTIALS: &str = "CREATE TABLE credential (
            id TEXT PRIMARY KEY,
            integration_id TEXT,
            label TEXT NOT NULL,
            value TEXT NOT NULL,
            method_id TEXT,
            active INTEGER,
            time_created INTEGER NOT NULL,
            time_updated INTEGER NOT NULL
        );
        CREATE TABLE account (id TEXT PRIMARY KEY, email TEXT NOT NULL, url TEXT NOT NULL,
            access_token TEXT NOT NULL, refresh_token TEXT NOT NULL, token_expiry INTEGER,
            time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);
        CREATE TABLE account_state (id INTEGER PRIMARY KEY NOT NULL, active_account_id TEXT);
        CREATE TABLE control_account (email TEXT NOT NULL, url TEXT NOT NULL,
            access_token TEXT NOT NULL, refresh_token TEXT NOT NULL, token_expiry INTEGER,
            active INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL,
            CONSTRAINT control_account_pk PRIMARY KEY(email, url));";

    fn sign_in(path: &Path, id: &str, value: &str, updated: i64) {
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO credential
                   (id, integration_id, label, value, method_id, active, time_created, time_updated)
                 VALUES (?1, 'anthropic', 'OAuth', ?2, NULL, NULL, ?3, ?3)",
                rusqlite::params![id, value, updated],
            )
            .unwrap();
    }

    fn credential_value(path: &Path, id: &str) -> Option<String> {
        open_readonly(path)
            .unwrap()
            .query_row("SELECT value FROM credential WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .optional()
            .unwrap()
    }

    #[test]
    fn sign_ins_are_copied_into_the_isolated_database_once() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        sign_in(&legacy, "anthropic", "access-1", 100);
        sign_in(&legacy, "opencode-go", "access-2", 100);

        inherit_credentials(&isolated, &legacy).unwrap();
        inherit_credentials(&isolated, &legacy).unwrap();

        assert_eq!(
            credential_value(&isolated, "anthropic").as_deref(),
            Some("access-1")
        );
        assert_eq!(
            credential_value(&isolated, "opencode-go").as_deref(),
            Some("access-2")
        );
        let count: i64 = open_readonly(&isolated)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM credential", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn a_retried_copy_does_not_revert_a_token_the_isolated_store_refreshed() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        sign_in(&legacy, "anthropic", "stale", 100);
        inherit_credentials(&isolated, &legacy).unwrap();
        // The isolated server refreshed the credential during a turn.
        sign_in(&isolated, "anthropic", "refreshed", 200);

        inherit_credentials(&isolated, &legacy).unwrap();

        assert_eq!(
            credential_value(&isolated, "anthropic").as_deref(),
            Some("refreshed")
        );
    }

    #[test]
    fn a_deliberate_relogin_replaces_the_credential_it_shares_an_id_with() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        sign_in(&legacy, "anthropic", "old", 100);
        inherit_credentials(&isolated, &legacy).unwrap();

        sign_in(&legacy, "anthropic", "new", 300);
        inherit_credentials(&isolated, &legacy).unwrap();

        assert_eq!(
            credential_value(&isolated, "anthropic").as_deref(),
            Some("new")
        );
    }

    #[test]
    fn a_source_without_credential_tables_is_skipped() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        // A V1 database has no credential table at all; the copy must not turn
        // that into a startup failure.
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(V1)
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();

        inherit_credentials(&isolated, &legacy).unwrap();

        let count: i64 = open_readonly(&isolated)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM credential", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn a_missing_user_database_is_not_an_error() {
        let fixture = Fixture::new();
        let isolated = fixture.path();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();

        inherit_credentials(&isolated, &fixture.0.path().join("absent.db")).unwrap();
        inherit_credentials(&isolated, &isolated).unwrap();
    }

    #[test]
    fn the_active_account_pointer_is_mirrored_because_it_carries_no_timestamp() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        for path in [&legacy, &isolated] {
            Connection::open(path)
                .unwrap()
                .execute_batch(CREDENTIALS)
                .unwrap();
        }
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(
                "INSERT INTO account_state VALUES (1, 'new-account');
                 INSERT INTO account VALUES ('new-account', 'a@b', 'https://console', 'a', 'r', 0, 1, 1);",
            )
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch("INSERT INTO account_state VALUES (1, 'old-account');")
            .unwrap();

        inherit_credentials(&isolated, &legacy).unwrap();

        let active: Option<String> = open_readonly(&isolated)
            .unwrap()
            .query_row("SELECT active_account_id FROM account_state", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(active.as_deref(), Some("new-account"));
    }

    #[test]
    fn a_console_relogin_updates_the_row_it_shares_its_composite_key_with() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        for path in [&legacy, &isolated] {
            Connection::open(path)
                .unwrap()
                .execute_batch(CREDENTIALS)
                .unwrap();
        }
        let control = |path: &Path, token: &str, updated: i64| {
            Connection::open(path)
                .unwrap()
                .execute(
                    "INSERT OR REPLACE INTO control_account VALUES
                       ('a@b', 'https://console', ?1, 'refresh', 0, 1, 1, ?2)",
                    rusqlite::params![token, updated],
                )
                .unwrap();
        };
        control(&legacy, "old", 100);
        inherit_credentials(&isolated, &legacy).unwrap();
        control(&legacy, "new", 300);

        inherit_credentials(&isolated, &legacy).unwrap();

        let token: String = open_readonly(&isolated)
            .unwrap()
            .query_row(
                "SELECT access_token FROM control_account WHERE email = 'a@b'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(token, "new");
    }

    #[test]
    fn updating_a_row_keeps_columns_the_source_does_not_have() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        // The isolated schema gained a required column the user's older database
        // does not have. Copying by delete-then-insert would drop the row when the
        // insert is refused; the update path must keep it and its extra value.
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(
                "CREATE TABLE credential (id TEXT PRIMARY KEY, integration_id TEXT,
                   label TEXT NOT NULL, value TEXT NOT NULL, time_created INTEGER NOT NULL,
                   time_updated INTEGER NOT NULL, sealed TEXT NOT NULL);",
            )
            .unwrap();
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute(
                "INSERT INTO credential VALUES ('anthropic', 'anthropic', 'OAuth', 'old', 1, 100, 'keep')",
                [],
            )
            .unwrap();
        sign_in(&legacy, "anthropic", "new", 300);

        inherit_credentials(&isolated, &legacy).unwrap();

        let (value, sealed): (String, String) = open_readonly(&isolated)
            .unwrap()
            .query_row(
                "SELECT value, sealed FROM credential WHERE id = 'anthropic'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(value, "new");
        assert_eq!(sealed, "keep");
    }

    #[test]
    fn columns_only_one_side_has_are_not_copied() {
        let fixture = Fixture::new();
        let legacy = fixture.0.path().join("legacy.db");
        let isolated = fixture.path();
        // An older schema without `method_id`/`active` must still contribute the
        // columns it does share instead of failing the whole copy.
        Connection::open(&legacy)
            .unwrap()
            .execute_batch(
                "CREATE TABLE credential (id TEXT PRIMARY KEY, integration_id TEXT,
                   label TEXT NOT NULL, value TEXT NOT NULL,
                   time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);",
            )
            .unwrap();
        Connection::open(&isolated)
            .unwrap()
            .execute_batch(CREDENTIALS)
            .unwrap();
        Connection::open(&legacy)
            .unwrap()
            .execute(
                "INSERT INTO credential VALUES ('google', 'google', 'API key', 'key-1', 10, 20)",
                [],
            )
            .unwrap();

        inherit_credentials(&isolated, &legacy).unwrap();

        assert_eq!(
            credential_value(&isolated, "google").as_deref(),
            Some("key-1")
        );
    }
}
