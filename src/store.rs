//! Local run store — orx's own truth for externally-executed runs.
//!
//! Mirrors the opencode model: state lives in a SQLite db beside the work
//! (`orx.db` under the data dir), and `orx serve` exposes it over loopback
//! HTTP/SSE. Run logs are plain append-only files under `run-logs/<runId>.log`
//! so tailing (serve) and appending (supervise) never contend on the db.
//!
//! Data dir: `$ORX_DATA_DIR`, else `$XDG_DATA_HOME/openresearch`, else
//! `~/.local/share/openresearch`.

use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

use crate::error::{anyhow, Result};
use crate::local::model::{LocalExperiment, LocalProject};

pub fn data_dir() -> PathBuf {
    // Resolution order (most to least authoritative):
    //   1. $ORX_DATA_DIR — explicit imperative override (launch.json, tests,
    //      the Codex sandbox pin). Stays on top so a forced path always wins.
    //   2. persisted user choice (config_dir()/settings.json `dataDir`) — set
    //      from the UI's Storage settings. Read fresh every call (no cache) so a
    //      just-completed data-dir move is picked up by the next Store::open().
    //   3. $XDG_DATA_HOME/openresearch — ambient system default *base*; an
    //      explicit UI choice rightly beats it, so it sits below (2).
    //   4. ~/.local/share/openresearch — hardcoded default.
    if let Some(dir) = env_path("ORX_DATA_DIR") {
        return dir;
    }
    if let Some(dir) = crate::config::settings_data_dir() {
        return dir;
    }
    xdg_default_data_dir()
}

pub(crate) fn open_lifecycle_lock() -> Result<fd_lock::RwLock<std::fs::File>> {
    // The config dir stays put while the user can move the live data directory.
    open_lifecycle_lock_at(&lifecycle_lock_path())
}

pub(crate) fn lifecycle_lock_path() -> PathBuf {
    crate::config::config_dir().join("orx.lifecycle.lock")
}

pub(crate) fn open_lifecycle_lock_at(
    path: &std::path::Path,
) -> Result<fd_lock::RwLock<std::fs::File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    Ok(fd_lock::RwLock::new(file))
}

fn data_dir_move_lock_path() -> PathBuf {
    crate::config::config_dir().join("orx.data-dir-move.lock")
}

pub(crate) struct DataDirMoveLock {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for DataDirMoveLock {
    fn drop(&mut self) {
        self.release.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn acquire_data_dir_move_lock(path: PathBuf) -> Result<DataDirMoveLock> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut lock = open_lifecycle_lock_at(&path)?;
            let _guard = lock.try_write()?;
            ready_tx
                .send(Ok(()))
                .map_err(|_| anyhow!("data-directory move lock receiver closed"))?;
            let _ = release_rx.recv();
            Ok(())
        })();
        if let Err(error) = result {
            let _ = ready_tx.send(Err(error.to_string()));
        }
    });
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(DataDirMoveLock {
            release: Some(release_tx),
            thread: Some(thread),
        }),
        Ok(Err(error)) => {
            let _ = thread.join();
            Err(anyhow!(error))
        }
        Err(_) => {
            let _ = thread.join();
            Err(anyhow!("data-directory move lock thread exited"))
        }
    }
}

/// Read an env var as a path, treating unset **and empty** the same (an empty
/// `export ORX_DATA_DIR=` is a shell footgun that must not resolve to `""`).
fn env_path(key: &str) -> Option<PathBuf> {
    crate::local::shell_env::var(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `$XDG_DATA_HOME/openresearch` else `~/.local/share/openresearch` — the tail
/// of the resolution chain, shared by `data_dir()` and `default_data_dir()`.
fn xdg_default_data_dir() -> PathBuf {
    let base = env_path("XDG_DATA_HOME").unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local")
            .join("share")
    });
    base.join("openresearch")
}

/// The data dir ignoring any persisted user choice — where resolution would
/// land if `settings.json` had no `dataDir`. Used by the Storage UI to show the
/// "(default)" path and offer resetting to it. `$ORX_DATA_DIR` still wins, since
/// it's a forced override.
pub fn default_data_dir() -> PathBuf {
    if let Some(dir) = env_path("ORX_DATA_DIR") {
        return dir;
    }
    xdg_default_data_dir()
}

/// Where `data_dir()`'s answer came from — surfaced by the Storage settings API
/// so the UI can explain a forced env override (read-only) vs. a user choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataDirSource {
    /// `$ORX_DATA_DIR` is set — forces the path, UI field is read-only.
    Env,
    /// Persisted user choice in `settings.json`.
    Config,
    /// Derived from `$XDG_DATA_HOME` (no user choice).
    Xdg,
    /// Hardcoded `~/.local/share/openresearch`.
    Default,
}

/// Classify the current `data_dir()` resolution for the Storage settings UI.
pub fn data_dir_source() -> DataDirSource {
    if env_path("ORX_DATA_DIR").is_some() {
        return DataDirSource::Env;
    }
    if crate::config::settings_data_dir().is_some() {
        return DataDirSource::Config;
    }
    if env_path("XDG_DATA_HOME").is_some() {
        return DataDirSource::Xdg;
    }
    DataDirSource::Default
}

/// Compact human-readable byte size (e.g. `1.2 KB`, `3.4 MB`). Shared by the
/// artifacts listing and the data-dir move so the two don't drift.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut size = n as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    format!("{:.1} {}", size, UNITS[unit])
}

pub fn log_path(run_id: &str) -> PathBuf {
    // Sanitize the id so malformed local data cannot escape the log directory.
    let safe: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    data_dir().join("run-logs").join(format!("{safe}.log"))
}

/// A locally tracked run. `backend_json` stores the backend descriptor used by
/// the detached supervisor.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredRun {
    pub id: String,
    pub experiment_id: String,
    pub project_id: String,
    pub status: String,
    pub backend_json: String,
    pub command: String,
    /// Unix millis.
    pub created_at: i64,
    pub updated_at: i64,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i64>,
    pub commit_sha: Option<String>,
    pub result_markdown: Option<String>,
    /// Cancel intent polled by the detached supervisor.
    pub cancel_requested: bool,
    /// The `orx up` chat session that launched this run, when it was started by
    /// an agent harness child (which exports `ORX_CHAT_SESSION_ID`). `None` for
    /// CLI-launched runs. This records attribution; wake-ups are
    /// separately and explicitly registered in `chat_run_wakeups`.
    pub chat_session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProjectActivitySummary {
    pub project_id: String,
    pub total_agents: usize,
    pub running_experiments: usize,
    pub total_experiments: usize,
    pub last_message_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RunWakeup {
    pub run: StoredRun,
    pub chat_session_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunWakeupRegistration {
    Scheduled,
    AlreadyPending,
    AlreadyDelivered,
}

/// A helper session an agent created with `orx agent spawn`, and the task it
/// was spawned to do. The CLI only writes the row; the resident `orx up`
/// watcher is what starts the child's first turn and reports back.
#[derive(Debug, Clone)]
pub struct ChatSpawn {
    pub session_id: String,
    pub parent_session_id: String,
    pub prompt: String,
    /// Whether the parent gets a wake-up once the helper finishes.
    pub wake_parent: bool,
    /// Failed attempts to start the helper's first turn.
    pub attempts: i64,
    /// Set by `finish_turn` when the helper's turn ends. Absent on a row whose
    /// turn is still running — or whose `orx up` died mid-turn, which is how a
    /// crash is told from a completion.
    pub finished_at: Option<i64>,
}

/// Lifecycle of a [`ChatSpawn`]: `Pending → Starting → Running → Waking →
/// Done`. The two `-ing` states are claims — one watcher owns the transition
/// and holds a token, so a crash mid-step is reclaimable rather than lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatSpawnState {
    /// Row written; the child's first turn has not been delivered yet.
    Pending,
    /// Claimed for delivery of the brief to the helper.
    Starting,
    /// The helper is working on its task.
    Running,
    /// Claimed for delivery of the wake-up to the parent.
    Waking,
    /// Terminal: the parent has been told, or never asked to be.
    Done,
}

impl ChatSpawnState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Waking => "waking",
            Self::Done => "done",
        }
    }
}

const RUN_WAKEUP_CLAIM_TTL_MS: i64 = 60 * 1000;
pub(crate) const CHAT_TURN_LEASE_TTL_MS: i64 = 60 * 1000;
const CHAT_SPAWN_CLAIM_TTL_MS: i64 = 60 * 1000;

pub struct Store {
    conn: Connection,
    data_dir_move_lock_path: PathBuf,
}

impl Store {
    /// Open (creating dirs/schema as needed). WAL so the supervise writers and
    /// the serve readers never block each other.
    pub fn open() -> Result<Self> {
        Self::open_at_with_move_lock(data_dir(), data_dir_move_lock_path())
    }

    /// Open a store rooted at an explicit directory, bypassing `data_dir()`
    /// resolution. For tests: a throwaway temp dir here avoids mutating the
    /// process-global `$ORX_DATA_DIR`, which the localbox lifecycle test owns
    /// (tests in different modules share env under the parallel runner).
    pub fn open_at(dir: PathBuf) -> Result<Self> {
        let move_lock_path = dir.join(".orx-data-dir-move.lock");
        Self::open_at_with_move_lock(dir, move_lock_path)
    }

    fn open_at_with_move_lock(dir: PathBuf, data_dir_move_lock_path: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(dir.join("run-logs"))
            .map_err(|e| anyhow!("Could not create {}: {}", dir.display(), e))?;
        let conn = Connection::open(dir.join("orx.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                id           TEXT PRIMARY KEY,
                experiment_id TEXT NOT NULL,
                project_id   TEXT NOT NULL,
                status       TEXT NOT NULL,
                backend_json TEXT NOT NULL,
                command      TEXT NOT NULL DEFAULT '',
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL,
                ended_at     INTEGER,
                exit_code    INTEGER
            );
            CREATE TABLE IF NOT EXISTS local_projects (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                slug            TEXT NOT NULL UNIQUE,
                github_owner    TEXT NOT NULL,
                github_repo     TEXT NOT NULL,
                github_sync_enabled INTEGER NOT NULL DEFAULT 1,
                baseline_branch TEXT NOT NULL DEFAULT 'main',
                repo_path       TEXT NOT NULL,
                run_command     TEXT,
                paper_id        TEXT,
                created_at      INTEGER NOT NULL,
                updated_at      INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS local_experiments (
                id                   TEXT PRIMARY KEY,
                project_id           TEXT NOT NULL,
                parent_experiment_id TEXT,
                slug                 TEXT NOT NULL,
                branch_name          TEXT NOT NULL,
                title                TEXT,
                description          TEXT,
                run_command          TEXT NOT NULL,
                agent_status         TEXT NOT NULL DEFAULT 'idle',
                created_at           INTEGER NOT NULL,
                updated_at           INTEGER NOT NULL,
                chat_session_id      TEXT,
                UNIQUE(project_id, slug)
            );
            DROP TABLE IF EXISTS local_reports;
            CREATE TABLE IF NOT EXISTS chat_sessions (
                id                TEXT PRIMARY KEY,
                project_id        TEXT NOT NULL,
                harness           TEXT NOT NULL,
                native_session_id TEXT,
                title             TEXT,
                title_source      TEXT,
                model             TEXT,
                service_tier      TEXT,
                permission_mode   TEXT,
                plan_mode         INTEGER NOT NULL DEFAULT 0,
                plan_reset_pending INTEGER NOT NULL DEFAULT 0,
                reasoning_level   TEXT,
                archived          INTEGER NOT NULL DEFAULT 0,
                context_usage_json TEXT,
                bootstrap_context TEXT,
                parent_session_id TEXT,
                created_at        INTEGER NOT NULL,
                updated_at        INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_messages (
                id         TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role       TEXT NOT NULL,
                parts_json TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                parent_id                TEXT,
                base_native_session_id   TEXT,
                result_native_session_id TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_chat_messages_session
                ON chat_messages(session_id, created_at);
            CREATE TABLE IF NOT EXISTS chat_turns (
                id                   TEXT PRIMARY KEY,
                session_id           TEXT NOT NULL,
                user_message_id      TEXT,
                assistant_message_id TEXT NOT NULL,
                client_turn_id       TEXT NOT NULL,
                request_hash         TEXT NOT NULL,
                prepared_input       TEXT NOT NULL,
                settings_json        TEXT NOT NULL,
                state                TEXT NOT NULL,
                delivery_state       TEXT NOT NULL,
                attempt_count        INTEGER NOT NULL DEFAULT 0,
                next_retry_at        INTEGER,
                error_kind           TEXT,
                error_message        TEXT,
                recovery_action      TEXT,
                recovered_by_turn_id TEXT,
                created_at           INTEGER NOT NULL,
                updated_at           INTEGER NOT NULL,
                UNIQUE(session_id, client_turn_id)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_turns_session
                ON chat_turns(session_id, created_at);
            CREATE TABLE IF NOT EXISTS chat_queued_messages (
                id             TEXT PRIMARY KEY,
                session_id     TEXT NOT NULL,
                client_turn_id TEXT NOT NULL,
                request_hash   TEXT NOT NULL,
                payload_json   TEXT NOT NULL,
                created_at     INTEGER NOT NULL,
                UNIQUE(session_id, client_turn_id)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_queued_messages_session
                ON chat_queued_messages(session_id, created_at);
            CREATE TABLE IF NOT EXISTS chat_run_wakeups (
                run_id          TEXT NOT NULL,
                chat_session_id TEXT NOT NULL,
                requested_at    INTEGER NOT NULL,
                state           TEXT NOT NULL DEFAULT 'pending',
                claim_token     TEXT,
                claimed_at      INTEGER,
                delivered_at    INTEGER,
                PRIMARY KEY(run_id, chat_session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_run_wakeups_requested
                ON chat_run_wakeups(requested_at);
            CREATE TABLE IF NOT EXISTS chat_spawns (
                session_id        TEXT PRIMARY KEY,
                parent_session_id TEXT NOT NULL,
                prompt            TEXT NOT NULL,
                wake_parent       INTEGER NOT NULL DEFAULT 1,
                state             TEXT NOT NULL DEFAULT 'pending',
                requested_at      INTEGER NOT NULL,
                claim_token       TEXT,
                claimed_at        INTEGER,
                attempts          INTEGER NOT NULL DEFAULT 0,
                finished_at       INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_chat_spawns_state
                ON chat_spawns(state, requested_at);
            CREATE TABLE IF NOT EXISTS chat_turn_leases (
                chat_session_id TEXT PRIMARY KEY,
                claim_token     TEXT NOT NULL,
                heartbeat_at    INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS data_dir_move_lease (
                id           INTEGER PRIMARY KEY CHECK (id = 1),
                claim_token  TEXT NOT NULL,
                heartbeat_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ssh_host_tests (
                host      TEXT PRIMARY KEY,
                reachable INTEGER NOT NULL,
                git_found INTEGER NOT NULL,
                tools_found INTEGER NOT NULL DEFAULT 0,
                missing_tools TEXT NOT NULL DEFAULT '',
                error     TEXT,
                tested_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS overleaf_links (
                project_id TEXT NOT NULL,
                tex_path   TEXT NOT NULL,
                overleaf_project_id TEXT NOT NULL,
                host       TEXT NOT NULL,
                head       TEXT NOT NULL DEFAULT '',
                baseline   TEXT NOT NULL DEFAULT '{}',
                root       TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (project_id, tex_path)
            );
            CREATE TABLE IF NOT EXISTS ui_state (
                id                       INTEGER PRIMARY KEY CHECK (id = 1),
                onboarding_completed     INTEGER NOT NULL DEFAULT 0,
                tour_completed           INTEGER NOT NULL DEFAULT 0,
                preferred_harness        TEXT,
                preferred_model          TEXT,
                preferred_service_tier   TEXT,
                preferred_permission_mode TEXT,
                preferred_reasoning_level TEXT
            );",
        )?;
        // Best-effort migrations for pre-existing dbs; re-runs fail with
        // "duplicate column name", which is exactly the no-op we want.
        for ddl in [
            "ALTER TABLE runs ADD COLUMN commit_sha TEXT",
            "ALTER TABLE runs ADD COLUMN result_markdown TEXT",
            "ALTER TABLE runs ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE runs ADD COLUMN chat_session_id TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN permission_mode TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN service_tier TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN plan_mode INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN plan_reset_pending INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN reasoning_level TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_sessions ADD COLUMN context_usage_json TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN bootstrap_context TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN title_source TEXT",
            "ALTER TABLE local_projects ADD COLUMN paper_id TEXT",
            "ALTER TABLE local_projects ADD COLUMN github_sync_enabled INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE local_experiments ADD COLUMN chat_session_id TEXT",
            "ALTER TABLE ssh_host_tests ADD COLUMN tools_found INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE ssh_host_tests ADD COLUMN missing_tools TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE chat_run_wakeups ADD COLUMN state TEXT NOT NULL DEFAULT 'pending'",
            "ALTER TABLE chat_run_wakeups ADD COLUMN claim_token TEXT",
            "ALTER TABLE chat_run_wakeups ADD COLUMN claimed_at INTEGER",
            "ALTER TABLE chat_run_wakeups ADD COLUMN delivered_at INTEGER",
            "ALTER TABLE chat_messages ADD COLUMN parent_id TEXT",
            "ALTER TABLE chat_messages ADD COLUMN base_native_session_id TEXT",
            "ALTER TABLE chat_messages ADD COLUMN result_native_session_id TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN active_leaf_id TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN parent_session_id TEXT",
            "ALTER TABLE ui_state ADD COLUMN preferred_service_tier TEXT",
            "ALTER TABLE chat_spawns ADD COLUMN wake_parent INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE chat_spawns ADD COLUMN attempts INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_spawns ADD COLUMN finished_at INTEGER",
        ] {
            let _ = conn.execute(ddl, []);
        }
        // Legacy tool failures cannot identify the missing dependency, so require one fresh check.
        conn.execute(
            "DELETE FROM ssh_host_tests
             WHERE reachable = 1 AND tools_found = 0 AND missing_tools = ''",
            [],
        )?;
        // Chat messages became a tree (forked turns). Legacy rows are one linear
        // chain each; after this a NULL parent_id genuinely means "branch root".
        // All-or-nothing, because a half-applied backfill that still bumped the
        // marker would leave those transcripts unparented for good.
        let schema_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if schema_version < 1 {
            let tx = conn.unchecked_transaction()?;
            tx.execute(
                "UPDATE chat_messages AS m SET parent_id = (
                     SELECT p.id FROM chat_messages p
                     WHERE p.session_id = m.session_id
                       AND (p.created_at, p.rowid) < (m.created_at, m.rowid)
                     ORDER BY p.created_at DESC, p.rowid DESC LIMIT 1
                 ) WHERE m.parent_id IS NULL",
                [],
            )?;
            tx.execute(
                "UPDATE chat_sessions SET active_leaf_id = (
                     SELECT m.id FROM chat_messages m WHERE m.session_id = chat_sessions.id
                     ORDER BY m.created_at DESC, m.rowid DESC LIMIT 1
                 ) WHERE active_leaf_id IS NULL",
                [],
            )?;
            // Legacy turns never recorded where the harness stood, so hand the
            // session's live id to its newest message: without it, switching
            // back to a pre-upgrade branch would resume from nothing and strand
            // that conversation's harness history.
            tx.execute(
                "UPDATE chat_messages SET result_native_session_id = (
                     SELECT s.native_session_id FROM chat_sessions s WHERE s.id = session_id
                 ) WHERE id IN (
                     SELECT active_leaf_id FROM chat_sessions WHERE active_leaf_id IS NOT NULL
                 )",
                [],
            )?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        // Older builds of this branch created a one-root-per-project unique
        // index; multiple baselines are allowed, so make sure it's gone.
        let _ = conn.execute(
            "DROP INDEX IF EXISTS uidx_local_experiments_project_baseline",
            [],
        );
        // Provider-owned permission ids + the independent Codex/OpenCode Plan
        // axis. These updates are idempotent and intentionally do not touch
        // Claude's native `plan`, Manual, or Accept edits modes.
        let _ = conn.execute(
            "UPDATE chat_sessions
             SET plan_mode = 1, permission_mode = 'ask'
             WHERE harness = 'codex' AND permission_mode = 'plan'",
            [],
        );
        let _ = conn.execute(
            "UPDATE chat_sessions
             SET plan_mode = 1, permission_mode = 'default'
             WHERE harness = 'opencode' AND permission_mode = 'plan'",
            [],
        );
        for (harness, old, new) in [
            ("claude-code", "ask", "manual"),
            ("claude-code", "default", "manual"),
            ("claude-code", "accept-edits", "acceptEdits"),
            ("claude-code", "bypass", "bypassPermissions"),
            ("codex", "auto", "approve-for-me"),
            ("codex", "bypass", "full-access"),
            ("codex", "accept-edits", "ask"),
            ("codex", "acceptEdits", "ask"),
            ("opencode", "auto", "default"),
            ("opencode", "bypass", "auto-approve"),
            ("opencode", "ask", "default"),
            ("opencode", "accept-edits", "default"),
            ("opencode", "acceptEdits", "default"),
        ] {
            let _ = conn.execute(
                "UPDATE chat_sessions SET permission_mode = ?3
                 WHERE harness = ?1 AND permission_mode = ?2",
                params![harness, old, new],
            );
        }
        let _ = conn.execute(
            "UPDATE chat_sessions SET permission_mode = 'auto'
             WHERE harness = 'claude-code'
               AND (permission_mode IS NULL OR permission_mode NOT IN
                    ('manual', 'acceptEdits', 'plan', 'auto', 'bypassPermissions'))",
            [],
        );
        let _ = conn.execute(
            "UPDATE chat_sessions SET permission_mode = 'approve-for-me'
             WHERE harness = 'codex'
               AND (permission_mode IS NULL OR permission_mode NOT IN
                    ('ask', 'approve-for-me', 'full-access'))",
            [],
        );
        let _ = conn.execute(
            "UPDATE chat_sessions SET permission_mode = 'default'
             WHERE harness = 'opencode'
               AND (permission_mode IS NULL OR permission_mode NOT IN
                    ('default', 'auto-approve'))",
            [],
        );
        // Seed before normalizing preferences: on the first open of an older
        // database the latest session may itself carry a retired mode or Plan.
        conn.execute(
            "INSERT OR IGNORE INTO ui_state (
                 id, onboarding_completed, tour_completed,
                 preferred_harness, preferred_model,
                 preferred_service_tier, preferred_permission_mode, preferred_reasoning_level
             )
             SELECT 1,
                    EXISTS(SELECT 1 FROM local_projects),
                    EXISTS(SELECT 1 FROM local_projects),
                    harness, model, service_tier, permission_mode, reasoning_level
             FROM (SELECT 1) seed
             LEFT JOIN chat_sessions ON chat_sessions.id = (
                 SELECT id FROM chat_sessions ORDER BY updated_at DESC LIMIT 1
             )",
            [],
        )?;
        // Preferred-agent state never carries Plan for command-activated
        // harnesses: new sessions start in Build/Default until `/plan` is used.
        let _ = conn.execute(
            "UPDATE ui_state SET preferred_permission_mode = 'ask'
             WHERE preferred_harness = 'codex' AND preferred_permission_mode = 'plan'",
            [],
        );
        let _ = conn.execute(
            "UPDATE ui_state SET preferred_permission_mode = 'default'
             WHERE preferred_harness = 'opencode' AND preferred_permission_mode = 'plan'",
            [],
        );
        let _ = conn.execute(
            "UPDATE ui_state SET preferred_permission_mode = 'auto'
             WHERE preferred_harness = 'claude-code' AND preferred_permission_mode = 'plan'",
            [],
        );
        for (harness, old, new) in [
            ("claude-code", "ask", "manual"),
            ("claude-code", "default", "manual"),
            ("claude-code", "accept-edits", "acceptEdits"),
            ("claude-code", "bypass", "bypassPermissions"),
            ("codex", "auto", "approve-for-me"),
            ("codex", "bypass", "full-access"),
            ("opencode", "auto", "default"),
            ("opencode", "bypass", "auto-approve"),
        ] {
            let _ = conn.execute(
                "UPDATE ui_state SET preferred_permission_mode = ?3
                 WHERE preferred_harness = ?1 AND preferred_permission_mode = ?2",
                params![harness, old, new],
            );
        }
        let _ = conn.execute(
            "UPDATE ui_state SET preferred_permission_mode = 'auto'
             WHERE preferred_harness = 'claude-code'
               AND preferred_permission_mode NOT IN
                   ('manual', 'acceptEdits', 'plan', 'auto', 'bypassPermissions')",
            [],
        );
        let _ = conn.execute(
            "UPDATE ui_state SET preferred_permission_mode = 'approve-for-me'
             WHERE preferred_harness = 'codex'
               AND preferred_permission_mode NOT IN ('ask', 'approve-for-me', 'full-access')",
            [],
        );
        let _ = conn.execute(
            "UPDATE ui_state SET preferred_permission_mode = 'default'
             WHERE preferred_harness = 'opencode'
               AND preferred_permission_mode NOT IN ('default', 'auto-approve')",
            [],
        );
        // NOTE: `reasoning_level` deliberately has NO migration for issue #123,
        // unlike the permission modes above. Rows written by older builds carry
        // an implicit effort (`high`), but every value the old builds wrote is
        // still a value the picker offers, so a blanket reset here would be
        // indistinguishable from — and would silently destroy — a level the user
        // just chose, on the very next open. That is the failure mode the NOTE
        // above warns about. Stale levels are reconciled where the information
        // to do it safely exists: `reconcileReasoning` in `ui/src/api.ts` drops
        // one the selected model doesn't offer, and each harness's mapper drops
        // it again before it can reach a CLI.
        Ok(Self {
            conn,
            data_dir_move_lock_path,
        })
    }

    /// Short write transaction over this connection; rolls back when dropped
    /// without `commit()`. Keep network I/O out of the closure it guards.
    pub fn begin(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.conn.unchecked_transaction()?)
    }

    /// Coalesce the WAL back into the main `orx.db` file and truncate it, so a
    /// filesystem-level copy of `orx.db` alone captures all committed data.
    /// Best-effort — used before relocating the data dir. Errors are returned so
    /// the caller can decide, but a busy checkpoint is non-fatal (the WAL sidecar
    /// gets copied too when present).
    pub fn checkpoint(&self) -> Result<()> {
        self.conn
            .pragma_update(None, "wal_checkpoint", "TRUNCATE")?;
        Ok(())
    }

    pub fn ui_state(&self) -> Result<StoredUiState> {
        Ok(self.conn.query_row(
            "SELECT onboarding_completed, tour_completed, preferred_harness,
                    preferred_model, preferred_service_tier,
                    preferred_permission_mode, preferred_reasoning_level
             FROM ui_state WHERE id = 1",
            [],
            |row| {
                let harness = row.get::<_, Option<String>>(2)?;
                let model = row.get::<_, Option<String>>(3)?;
                let service_tier = row.get::<_, Option<String>>(4)?;
                let permission_mode = row.get::<_, Option<String>>(5)?;
                let reasoning_level = row.get::<_, Option<String>>(6)?;
                Ok(StoredUiState {
                    onboarding_completed: row.get(0)?,
                    tour_completed: row.get(1)?,
                    preferred_agent: harness.map(|harness| StoredAgentSelection {
                        harness,
                        model,
                        service_tier,
                        permission_mode,
                        reasoning_level,
                    }),
                })
            },
        )?)
    }

    pub fn set_onboarding_completed(&self, completed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE ui_state SET onboarding_completed = ?1 WHERE id = 1",
            params![completed],
        )?;
        Ok(())
    }

    pub fn set_tour_completed(&self, completed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE ui_state SET tour_completed = ?1 WHERE id = 1",
            params![completed],
        )?;
        Ok(())
    }

    pub fn set_preferred_agent(&self, selection: &StoredAgentSelection) -> Result<()> {
        self.conn.execute(
            "UPDATE ui_state
             SET preferred_harness = ?1, preferred_model = ?2,
                 preferred_service_tier = ?3,
                 preferred_permission_mode = ?4, preferred_reasoning_level = ?5
             WHERE id = 1",
            params![
                selection.harness,
                selection.model,
                selection.service_tier,
                selection.permission_mode,
                selection.reasoning_level,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_run(&self, run: &StoredRun) -> Result<()> {
        self.conn.execute(
            "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command,
                               created_at, updated_at, ended_at, exit_code,
                               commit_sha, result_markdown, cancel_requested,
                               chat_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
               status = excluded.status,
               backend_json = excluded.backend_json,
               updated_at = excluded.updated_at,
               ended_at = excluded.ended_at,
               exit_code = excluded.exit_code,
               commit_sha = excluded.commit_sha,
               result_markdown = excluded.result_markdown",
            // chat_session_id is deliberately absent from the DO UPDATE SET:
            // run ownership is immutable, so a later status upsert never
            // rewrites (or clears) the session that launched the run.
            params![
                run.id,
                run.experiment_id,
                run.project_id,
                run.status,
                run.backend_json,
                run.command,
                run.created_at,
                run.updated_at,
                run.ended_at,
                run.exit_code,
                run.commit_sha,
                run.result_markdown,
                run.cancel_requested,
                run.chat_session_id,
            ],
        )?;
        Ok(())
    }

    pub fn update_status(
        &self,
        run_id: &str,
        status: &str,
        ended_at: Option<i64>,
        exit_code: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET status = ?2, updated_at = ?3, ended_at = COALESCE(?4, ended_at),
                             exit_code = COALESCE(?5, exit_code)
             WHERE id = ?1",
            params![run_id, status, now_ms(), ended_at, exit_code],
        )?;
        Ok(())
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<StoredRun>> {
        let run = self
            .conn
            .query_row(
                &format!("{SELECT_RUN} WHERE id = ?1"),
                params![run_id],
                row_to_run,
            )
            .optional()?;
        Ok(run)
    }

    /// Newest first (creation time).
    pub fn list_runs(&self, limit: usize) -> Result<Vec<StoredRun>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{SELECT_RUN} ORDER BY created_at DESC LIMIT ?1"))?;
        let rows = stmt.query_map(params![limit as i64], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Count runs in an active state (`starting`/`running`) — SQL-side and
    /// unbounded, so a long-running job older than the newest N rows still
    /// counts. Used by the data-dir move's in-flight guard.
    pub fn count_active_runs(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM runs WHERE status IN ('starting', 'running')",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }

    pub fn list_active_runs(&self) -> Result<Vec<StoredRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_RUN} WHERE status IN ('starting', 'running') ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map([], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_runs_by_project(&self, project_id: &str) -> Result<Vec<StoredRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_RUN} WHERE project_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![project_id], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    // Consumed by later local-mode stages (supervise + `orx up` API).
    #[allow(dead_code)]
    pub fn list_runs_by_experiment(&self, experiment_id: &str) -> Result<Vec<StoredRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_RUN} WHERE experiment_id = ?1 ORDER BY created_at DESC"
        ))?;
        let rows = stmt.query_map(params![experiment_id], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn latest_run_for_experiment(&self, experiment_id: &str) -> Result<Option<StoredRun>> {
        let run = self
            .conn
            .query_row(
                &format!("{SELECT_RUN} WHERE experiment_id = ?1 ORDER BY created_at DESC LIMIT 1"),
                params![experiment_id],
                row_to_run,
            )
            .optional()?;
        Ok(run)
    }

    pub fn register_run_wakeup(
        &self,
        run_id: &str,
        chat_session_id: &str,
    ) -> Result<RunWakeupRegistration> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO chat_run_wakeups (run_id, chat_session_id, requested_at)
             VALUES (?1, ?2, ?3)",
            params![run_id, chat_session_id, now_ms()],
        )?;
        if inserted == 1 {
            return Ok(RunWakeupRegistration::Scheduled);
        }
        let state: String = self.conn.query_row(
            "SELECT state FROM chat_run_wakeups WHERE run_id = ?1 AND chat_session_id = ?2",
            params![run_id, chat_session_id],
            |row| row.get(0),
        )?;
        Ok(if state == "delivered" {
            RunWakeupRegistration::AlreadyDelivered
        } else {
            RunWakeupRegistration::AlreadyPending
        })
    }

    pub fn remove_run_wakeup(&self, run_id: &str, chat_session_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM chat_run_wakeups WHERE run_id = ?1 AND chat_session_id = ?2",
            params![run_id, chat_session_id],
        )?;
        Ok(())
    }

    pub fn prune_run_wakeups(&self) -> Result<()> {
        self.clear_stale_data_dir_move_lease()?;
        self.conn.execute(
            "UPDATE chat_run_wakeups
             SET state = 'pending', claim_token = NULL, claimed_at = NULL
             WHERE state = 'claimed' AND claimed_at < ?1
               AND NOT EXISTS (SELECT 1 FROM data_dir_move_lease WHERE id = 1)",
            params![now_ms() - RUN_WAKEUP_CLAIM_TTL_MS],
        )?;
        self.conn.execute(
            "DELETE FROM chat_run_wakeups
             WHERE NOT EXISTS (SELECT 1 FROM data_dir_move_lease WHERE id = 1)
               AND (
                 NOT EXISTS (SELECT 1 FROM runs WHERE runs.id = chat_run_wakeups.run_id)
                OR NOT EXISTS (
                    SELECT 1 FROM chat_sessions
                    WHERE chat_sessions.id = chat_run_wakeups.chat_session_id
                )
                OR EXISTS (
                    SELECT 1 FROM runs
                    WHERE runs.id = chat_run_wakeups.run_id
                      AND runs.status = 'cancelled'
                )
               )",
            [],
        )?;
        Ok(())
    }

    pub fn list_ready_run_wakeups(&self) -> Result<Vec<RunWakeup>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.id, r.experiment_id, r.project_id, r.status, r.backend_json, r.command,
                    r.created_at, r.updated_at, r.ended_at, r.exit_code,
                    r.commit_sha, r.result_markdown, r.cancel_requested, r.chat_session_id,
                    w.chat_session_id, w.state
             FROM chat_run_wakeups w
             JOIN runs r ON r.id = w.run_id
             WHERE w.state IN ('pending', 'claimed') AND r.status IN ('done', 'failed')
             ORDER BY COALESCE(r.ended_at, r.updated_at), w.requested_at, r.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(RunWakeup {
                run: row_to_run(row)?,
                chat_session_id: row.get(14)?,
                state: row.get(15)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn claim_run_wakeup(&self, run_id: &str, chat_session_id: &str) -> Result<Option<String>> {
        let token = uuid::Uuid::new_v4().to_string();
        let claimed = self.conn.execute(
            "UPDATE chat_run_wakeups
             SET state = 'claimed', claim_token = ?3, claimed_at = ?4
             WHERE run_id = ?1 AND chat_session_id = ?2 AND state = 'pending'",
            params![run_id, chat_session_id, token, now_ms()],
        )?;
        Ok((claimed == 1).then_some(token))
    }

    pub fn renew_run_wakeup_claim(
        &self,
        run_id: &str,
        chat_session_id: &str,
        token: &str,
    ) -> Result<bool> {
        let renewed = self.conn.execute(
            "UPDATE chat_run_wakeups SET claimed_at = ?4
             WHERE run_id = ?1 AND chat_session_id = ?2
               AND state = 'claimed' AND claim_token = ?3",
            params![run_id, chat_session_id, token, now_ms()],
        )?;
        Ok(renewed == 1)
    }

    pub fn release_run_wakeup(
        &self,
        run_id: &str,
        chat_session_id: &str,
        token: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_run_wakeups
             SET state = 'pending', claim_token = NULL, claimed_at = NULL
             WHERE run_id = ?1 AND chat_session_id = ?2
               AND state = 'claimed' AND claim_token = ?3",
            params![run_id, chat_session_id, token],
        )?;
        Ok(())
    }

    pub fn mark_run_wakeup_delivered(
        &self,
        run_id: &str,
        chat_session_id: &str,
        token: &str,
    ) -> Result<bool> {
        let delivered = self.conn.execute(
            "UPDATE chat_run_wakeups
             SET state = 'delivered', claim_token = NULL, claimed_at = NULL, delivered_at = ?4
             WHERE run_id = ?1 AND chat_session_id = ?2
               AND state = 'claimed' AND claim_token = ?3",
            params![run_id, chat_session_id, token, now_ms()],
        )?;
        Ok(delivered == 1)
    }

    pub fn create_chat_spawn(&self, spawn: &ChatSpawn) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat_spawns
                 (session_id, parent_session_id, prompt, wake_parent, state, requested_at)
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![
                spawn.session_id,
                spawn.parent_session_id,
                spawn.prompt,
                spawn.wake_parent,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    /// Return abandoned claims to their prior state and drop rows whose child
    /// session is gone. A missing *parent* is not pruned here: the child keeps
    /// working, and the wake step retires the row on its own.
    pub fn prune_chat_spawns(&self) -> Result<()> {
        self.clear_stale_data_dir_move_lease()?;
        let stale = now_ms() - CHAT_SPAWN_CLAIM_TTL_MS;
        self.conn.execute(
            "UPDATE chat_spawns
             SET state = CASE state WHEN 'starting' THEN 'pending' ELSE 'running' END,
                 attempts = attempts + CASE state WHEN 'starting' THEN 1 ELSE 0 END,
                 claim_token = NULL, claimed_at = NULL
             WHERE state IN ('starting', 'waking') AND claimed_at < ?1
               AND NOT EXISTS (SELECT 1 FROM data_dir_move_lease WHERE id = 1)",
            params![stale],
        )?;
        self.conn.execute(
            "DELETE FROM chat_spawns
             WHERE NOT EXISTS (SELECT 1 FROM data_dir_move_lease WHERE id = 1)
               AND NOT EXISTS (
                   SELECT 1 FROM chat_sessions WHERE chat_sessions.id = chat_spawns.session_id
               )",
            [],
        )?;
        Ok(())
    }

    /// Spawns sitting in one state, oldest request first.
    pub fn list_chat_spawns(&self, state: ChatSpawnState) -> Result<Vec<ChatSpawn>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, parent_session_id, prompt, wake_parent, attempts, finished_at
             FROM chat_spawns WHERE state = ?1 ORDER BY requested_at, session_id",
        )?;
        let rows = stmt.query_map(params![state.as_str()], row_to_chat_spawn)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_chat_spawn(&self, session_id: &str) -> Result<Option<ChatSpawn>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id, parent_session_id, prompt, wake_parent, attempts, finished_at
             FROM chat_spawns WHERE session_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![session_id], row_to_chat_spawn)?;
        Ok(rows.next().transpose()?)
    }

    /// Take ownership of a spawn's next step, moving it into the claimed state
    /// and returning the token that proves the claim. `None` means another
    /// watcher got there first.
    pub fn claim_chat_spawn(
        &self,
        session_id: &str,
        from: ChatSpawnState,
        to: ChatSpawnState,
    ) -> Result<Option<String>> {
        let token = uuid::Uuid::new_v4().to_string();
        let claimed = self.conn.execute(
            "UPDATE chat_spawns SET state = ?3, claim_token = ?4, claimed_at = ?5
             WHERE session_id = ?1 AND state = ?2",
            params![session_id, from.as_str(), to.as_str(), token, now_ms()],
        )?;
        Ok((claimed == 1).then_some(token))
    }

    /// Finish (or hand back) a claimed step. `false` means the claim expired
    /// and someone else reclaimed the row, so the caller's work is NOT recorded
    /// and must not be treated as done.
    pub fn settle_chat_spawn(
        &self,
        session_id: &str,
        token: &str,
        to: ChatSpawnState,
    ) -> Result<bool> {
        let settled = self.conn.execute(
            "UPDATE chat_spawns SET state = ?3, claim_token = NULL, claimed_at = NULL
             WHERE session_id = ?1 AND claim_token = ?2",
            params![session_id, token, to.as_str()],
        )?;
        Ok(settled == 1)
    }

    /// Whether any `orx up` currently holds a turn lease on this session.
    ///
    /// The durable counterpart to `ChatHost::is_busy`, which reads a map this
    /// process owns: a helper started by a different `orx up` — or by one that
    /// has since restarted — is busy without this process knowing it.
    pub fn chat_turn_leased(&self, chat_session_id: &str) -> Result<bool> {
        let leased = self.conn.query_row(
            "SELECT 1 FROM chat_turn_leases WHERE chat_session_id = ?1 AND heartbeat_at >= ?2",
            params![chat_session_id, now_ms() - CHAT_TURN_LEASE_TTL_MS],
            |_| Ok(()),
        );
        Ok(matches!(leased, Ok(())))
    }

    /// Record that a spawned helper's turn ended. An unstamped row whose lease
    /// has lapsed is how the watcher recognizes an `orx up` that died mid-turn,
    /// so the parent hears "interrupted" rather than a false completion.
    pub fn mark_chat_spawn_finished(&self, chat_session_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_spawns SET finished_at = ?2
             WHERE session_id = ?1 AND finished_at IS NULL",
            params![chat_session_id, now_ms()],
        )?;
        Ok(())
    }

    pub fn record_chat_spawn_attempt(&self, chat_session_id: &str) -> Result<i64> {
        self.conn.execute(
            "UPDATE chat_spawns SET attempts = attempts + 1 WHERE session_id = ?1",
            params![chat_session_id],
        )?;
        Ok(self.conn.query_row(
            "SELECT attempts FROM chat_spawns WHERE session_id = ?1",
            params![chat_session_id],
            |row| row.get(0),
        )?)
    }

    /// Helpers this parent has in flight, for the fan-out cap.
    pub fn count_live_chat_spawns(&self, parent_session_id: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM chat_spawns
             WHERE parent_session_id = ?1 AND state != 'done'",
            params![parent_session_id],
            |row| row.get(0),
        )?)
    }

    pub fn claim_chat_turn(&self, chat_session_id: &str, token: &str) -> Result<bool> {
        self.clear_stale_data_dir_move_lease()?;
        self.conn.execute(
            "DELETE FROM chat_turn_leases
             WHERE heartbeat_at < ?1
                OR NOT EXISTS (
                    SELECT 1 FROM chat_sessions
                    WHERE chat_sessions.id = chat_turn_leases.chat_session_id
                )",
            params![now_ms() - CHAT_TURN_LEASE_TTL_MS],
        )?;
        let claimed = self.conn.execute(
            "INSERT OR IGNORE INTO chat_turn_leases
                 (chat_session_id, claim_token, heartbeat_at)
             SELECT ?1, ?2, ?3
             WHERE NOT EXISTS (SELECT 1 FROM data_dir_move_lease WHERE id = 1)",
            params![chat_session_id, token, now_ms()],
        )?;
        Ok(claimed == 1)
    }

    pub fn renew_chat_turn(&self, chat_session_id: &str, token: &str) -> Result<bool> {
        let renewed = self.conn.execute(
            "UPDATE chat_turn_leases SET heartbeat_at = ?3
             WHERE chat_session_id = ?1 AND claim_token = ?2",
            params![chat_session_id, token, now_ms()],
        )?;
        Ok(renewed == 1)
    }

    pub fn release_chat_turn(&self, chat_session_id: &str, token: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM chat_turn_leases
             WHERE chat_session_id = ?1 AND claim_token = ?2",
            params![chat_session_id, token],
        )?;
        Ok(())
    }

    pub fn claim_data_dir_move(&self, token: &str) -> Result<bool> {
        self.conn.execute(
            "DELETE FROM chat_turn_leases WHERE heartbeat_at < ?1",
            params![now_ms() - CHAT_TURN_LEASE_TTL_MS],
        )?;
        self.clear_stale_data_dir_move_lease()?;
        let claimed = self.conn.execute(
            "INSERT OR IGNORE INTO data_dir_move_lease (id, claim_token, heartbeat_at)
             SELECT 1, ?1, ?2
             WHERE NOT EXISTS (SELECT 1 FROM chat_turn_leases)",
            params![token, now_ms()],
        )?;
        Ok(claimed == 1)
    }

    pub(crate) fn acquire_data_dir_move_lock(&self) -> Result<DataDirMoveLock> {
        acquire_data_dir_move_lock(self.data_dir_move_lock_path.clone())
    }

    fn clear_stale_data_dir_move_lease(&self) -> Result<()> {
        let token = self
            .conn
            .query_row(
                "SELECT claim_token FROM data_dir_move_lease WHERE id = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(token) = token else {
            return Ok(());
        };
        let mut lock = open_lifecycle_lock_at(&self.data_dir_move_lock_path)?;
        match lock.try_write() {
            Ok(_guard) => {
                self.conn.execute(
                    "DELETE FROM data_dir_move_lease WHERE id = 1 AND claim_token = ?1",
                    params![token],
                )?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub fn release_data_dir_move(&self, token: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM data_dir_move_lease WHERE id = 1 AND claim_token = ?1",
            params![token],
        )?;
        Ok(())
    }

    pub fn set_cancel_requested(&self, run_id: &str, requested: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET cancel_requested = ?2, updated_at = ?3 WHERE id = ?1",
            params![run_id, requested, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_result_markdown(&self, run_id: &str, markdown: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET result_markdown = ?2, updated_at = ?3 WHERE id = ?1",
            params![run_id, markdown, now_ms()],
        )?;
        Ok(())
    }

    /// Update only the run's backend descriptor — for a supervisor learning
    /// more about its job mid-flight (e.g. the openresearch box's SSH
    /// endpoint) without clobbering status/markdown/cancel state.
    pub fn set_backend_json(&self, run_id: &str, backend_json: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET backend_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![run_id, backend_json, now_ms()],
        )?;
        Ok(())
    }

    // --- local projects (orx up) ---

    /// Atomically install a fully-materialized demo project. The project id is
    /// the idempotency key: a completed prior seed is left byte-for-byte intact.
    pub fn create_demo_snapshot(
        &self,
        project: &LocalProject,
        experiments: &[crate::local::model::LocalExperiment],
        run: &StoredRun,
        sessions: &[StoredChatSession],
        messages: &[StoredChatMessage],
    ) -> Result<bool> {
        let tx = self.begin()?;
        let inserted = tx.execute(
            &format!("INSERT OR IGNORE INTO local_projects ({PROJECT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
            params![
                project.id,
                project.name,
                project.slug,
                project.github_owner,
                project.github_repo,
                project.github_sync_enabled,
                project.baseline_branch,
                project.repo_path,
                project.run_command,
                project.paper_id,
                project.created_at,
                project.updated_at,
            ],
        )?;
        if inserted == 0 {
            tx.commit()?;
            return Ok(false);
        }
        for experiment in experiments {
            tx.execute(
                &format!("INSERT INTO local_experiments ({EXPERIMENT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
                params![
                    experiment.id,
                    experiment.project_id,
                    experiment.parent_experiment_id,
                    experiment.slug,
                    experiment.branch_name,
                    experiment.title,
                    experiment.description,
                    experiment.run_command,
                    experiment.agent_status,
                    experiment.created_at,
                    experiment.updated_at,
                    experiment.chat_session_id,
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command,
                               created_at, updated_at, ended_at, exit_code, commit_sha,
                               result_markdown, cancel_requested, chat_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                run.id,
                run.experiment_id,
                run.project_id,
                run.status,
                run.backend_json,
                run.command,
                run.created_at,
                run.updated_at,
                run.ended_at,
                run.exit_code,
                run.commit_sha,
                run.result_markdown,
                run.cancel_requested,
                run.chat_session_id,
            ],
        )?;
        for session in sessions {
            tx.execute(
                "INSERT INTO chat_sessions (id, project_id, harness, native_session_id, title,
                                            title_source, model, service_tier, permission_mode, plan_mode,
                                            plan_reset_pending, reasoning_level,
                                            archived, context_usage_json, bootstrap_context,
                                            active_leaf_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                params![
                    session.id,
                    session.project_id,
                    session.harness,
                    session.native_session_id,
                    session.title,
                    session.title_source,
                    session.model,
                    session.service_tier,
                    session.permission_mode,
                    session.plan_mode,
                    session.plan_reset_pending,
                    session.reasoning_level,
                    session.archived,
                    session.context_usage_json,
                    session.bootstrap_context,
                    session.active_leaf_id,
                    session.created_at,
                    session.updated_at,
                ],
            )?;
        }
        for message in messages {
            tx.execute(
                "INSERT INTO chat_messages (id, session_id, role, parts_json, created_at,
                                            parent_id, base_native_session_id,
                                            result_native_session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    message.id,
                    message.session_id,
                    message.role,
                    message.parts_json,
                    message.created_at,
                    message.parent_id,
                    message.base_native_session_id,
                    message.result_native_session_id,
                ],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    pub fn create_local_project(&self, p: &LocalProject) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO local_projects ({PROJECT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
            params![
                p.id, p.name, p.slug, p.github_owner, p.github_repo,
                p.github_sync_enabled, p.baseline_branch, p.repo_path, p.run_command, p.paper_id,
                p.created_at, p.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_local_project(&self, id: &str) -> Result<Option<LocalProject>> {
        let p = self
            .conn
            .query_row(
                &format!("SELECT {PROJECT_COLS} FROM local_projects WHERE id = ?1"),
                params![id],
                LocalProject::from_row,
            )
            .optional()?;
        Ok(p)
    }

    #[allow(dead_code)]
    pub fn get_local_project_by_slug(&self, slug: &str) -> Result<Option<LocalProject>> {
        let p = self
            .conn
            .query_row(
                &format!("SELECT {PROJECT_COLS} FROM local_projects WHERE slug = ?1"),
                params![slug],
                LocalProject::from_row,
            )
            .optional()?;
        Ok(p)
    }

    pub fn list_local_projects(&self) -> Result<Vec<LocalProject>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PROJECT_COLS} FROM local_projects ORDER BY updated_at DESC"
        ))?;
        let rows = stmt.query_map([], LocalProject::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Delete a project and everything hanging off it (chats, runs,
    /// experiments) in one transaction. GitHub repo and cache clone are kept.
    pub fn delete_local_project(&self, id: &str) -> Result<()> {
        let tx = self.begin()?;
        tx.execute(
            "DELETE FROM overleaf_links WHERE project_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM chat_queued_messages WHERE session_id IN
               (SELECT id FROM chat_sessions WHERE project_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM chat_turns WHERE session_id IN
               (SELECT id FROM chat_sessions WHERE project_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM chat_spawns WHERE session_id IN
               (SELECT id FROM chat_sessions WHERE project_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM chat_messages WHERE session_id IN
               (SELECT id FROM chat_sessions WHERE project_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM chat_sessions WHERE project_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM runs WHERE project_id = ?1", params![id])?;
        self.conn.execute(
            "DELETE FROM local_experiments WHERE project_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM local_projects WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// Bump updated_at only — records a visit for the recency sort and fires
    /// the SSE project.updated diff.
    pub fn touch_local_project(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE local_projects SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Full-row update by id (name / run_command / branch edits).
    pub fn update_local_project(&self, p: &LocalProject) -> Result<()> {
        self.conn.execute(
            "UPDATE local_projects SET name = ?2, slug = ?3, github_owner = ?4, github_repo = ?5,
                    github_sync_enabled = ?6, baseline_branch = ?7, repo_path = ?8,
                    run_command = ?9, paper_id = ?10, updated_at = ?11
             WHERE id = ?1",
            params![
                p.id,
                p.name,
                p.slug,
                p.github_owner,
                p.github_repo,
                p.github_sync_enabled,
                p.baseline_branch,
                p.repo_path,
                p.run_command,
                p.paper_id,
                now_ms(),
            ],
        )?;
        Ok(())
    }

    // --- local experiments (orx up) ---

    pub fn create_local_experiment(&self, e: &LocalExperiment) -> Result<()> {
        self.conn.execute(
            &format!("INSERT INTO local_experiments ({EXPERIMENT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"),
            params![
                e.id, e.project_id, e.parent_experiment_id, e.slug, e.branch_name,
                e.title, e.description, e.run_command, e.agent_status, e.created_at, e.updated_at,
                e.chat_session_id,
            ],
        )?;
        Ok(())
    }

    pub fn get_local_experiment(&self, id: &str) -> Result<Option<LocalExperiment>> {
        let e = self
            .conn
            .query_row(
                &format!("SELECT {EXPERIMENT_COLS} FROM local_experiments WHERE id = ?1"),
                params![id],
                LocalExperiment::from_row,
            )
            .optional()?;
        Ok(e)
    }

    pub fn list_experiments_by_project(&self, project_id: &str) -> Result<Vec<LocalExperiment>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EXPERIMENT_COLS} FROM local_experiments WHERE project_id = ?1 ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![project_id], LocalExperiment::from_row)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Full-row update by id (title / description / run_command / agent_status).
    ///
    /// `chat_session_id` is deliberately omitted: session ownership is stamped
    /// once at creation and is immutable thereafter.
    pub fn update_local_experiment(&self, e: &LocalExperiment) -> Result<()> {
        self.conn.execute(
            "UPDATE local_experiments SET parent_experiment_id = ?2, slug = ?3, branch_name = ?4,
                    title = ?5, description = ?6, run_command = ?7, agent_status = ?8, updated_at = ?9
             WHERE id = ?1",
            params![
                e.id, e.parent_experiment_id, e.slug, e.branch_name,
                e.title, e.description, e.run_command, e.agent_status, now_ms(),
            ],
        )?;
        Ok(())
    }

    // --- chat sessions / messages ------------------------------------------

    pub fn create_chat_session(&self, s: &StoredChatSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat_sessions (id, project_id, harness, native_session_id, title, title_source, model,
                                        service_tier, permission_mode, plan_mode, plan_reset_pending, reasoning_level, archived, bootstrap_context,
                                        active_leaf_id, parent_session_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                s.id,
                s.project_id,
                s.harness,
                s.native_session_id,
                s.title,
                s.title_source,
                s.model,
                s.service_tier,
                s.permission_mode,
                s.plan_mode,
                s.plan_reset_pending,
                s.reasoning_level,
                s.archived,
                s.bootstrap_context,
                s.active_leaf_id,
                s.parent_session_id,
                s.created_at,
                s.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_chat_session(&self, id: &str) -> Result<Option<StoredChatSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_SESSION_COLS} FROM chat_sessions WHERE id = ?1"
        ))?;
        let mut rows = stmt.query_map(params![id], row_to_chat_session)?;
        Ok(rows.next().transpose()?)
    }

    pub fn list_chat_sessions_by_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<StoredChatSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_SESSION_COLS} FROM chat_sessions WHERE project_id = ?1
             ORDER BY updated_at DESC"
        ))?;
        let rows = stmt.query_map(params![project_id], row_to_chat_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_chat_session_project_ids(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, project_id FROM chat_sessions")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Project totals include archived sessions so the agent count reflects
    /// every agent that has worked on the project, not only the visible rail.
    pub fn list_project_activity_summaries(&self) -> Result<Vec<ProjectActivitySummary>> {
        let mut stmt = self.conn.prepare(
            "WITH agent_counts AS (
                 SELECT project_id, COUNT(*) AS total_agents
                 FROM chat_sessions
                 GROUP BY project_id
             ),
             experiment_counts AS (
                 SELECT project_id, COUNT(*) AS total_experiments
                 FROM local_experiments
                 GROUP BY project_id
             ),
             running_experiment_counts AS (
                 SELECT runs.project_id, COUNT(DISTINCT runs.experiment_id) AS running_experiments
                 FROM runs
                 JOIN local_experiments
                   ON local_experiments.id = runs.experiment_id
                  AND local_experiments.project_id = runs.project_id
                 WHERE runs.status IN ('starting', 'running')
                 GROUP BY runs.project_id
             ),
             latest_messages AS (
                 SELECT chat_sessions.project_id, MAX(chat_messages.created_at) AS last_message_at
                 FROM chat_messages
                 JOIN chat_sessions ON chat_sessions.id = chat_messages.session_id
                 WHERE chat_messages.role IN ('user', 'assistant')
                 GROUP BY chat_sessions.project_id
             )
             SELECT local_projects.id,
                    COALESCE(agent_counts.total_agents, 0),
                    COALESCE(running_experiment_counts.running_experiments, 0),
                    COALESCE(experiment_counts.total_experiments, 0),
                    latest_messages.last_message_at
             FROM local_projects
             LEFT JOIN agent_counts ON agent_counts.project_id = local_projects.id
             LEFT JOIN experiment_counts ON experiment_counts.project_id = local_projects.id
             LEFT JOIN running_experiment_counts
               ON running_experiment_counts.project_id = local_projects.id
             LEFT JOIN latest_messages ON latest_messages.project_id = local_projects.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectActivitySummary {
                project_id: row.get(0)?,
                total_agents: row.get::<_, i64>(1)? as usize,
                running_experiments: row.get::<_, i64>(2)? as usize,
                total_experiments: row.get::<_, i64>(3)? as usize,
                last_message_at: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn delete_chat_session(&self, id: &str) -> Result<()> {
        let tx = self.begin()?;
        tx.execute(
            "DELETE FROM chat_queued_messages WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM chat_turns WHERE session_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM chat_messages WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM chat_spawns WHERE session_id = ?1", params![id])?;
        tx.execute("DELETE FROM chat_sessions WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// `None` clears it, which a fork of the session's very first turn needs:
    /// that turn ran with no harness session to resume from.
    pub fn set_chat_session_native_id(&self, id: &str, native_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET native_session_id = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, native_id, now_ms()],
        )?;
        Ok(())
    }

    /// Deliberately leaves `updated_at` alone: paging between forks is not
    /// activity, and would otherwise reorder the Recents list.
    pub fn set_chat_session_active_leaf(&self, id: &str, leaf_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET active_leaf_id = ?2 WHERE id = ?1",
            params![id, leaf_id],
        )?;
        Ok(())
    }

    pub fn set_chat_session_model(&self, id: &str, model: &str) -> Result<()> {
        self.set_chat_session_model_value(id, Some(model))
    }

    pub fn set_chat_session_model_value(&self, id: &str, model: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET model = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, model, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_chat_session_service_tier(&self, id: &str, tier: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET service_tier = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, tier, now_ms()],
        )?;
        Ok(())
    }

    /// Persist the latest context-window usage (serialized `ContextUsage`).
    /// Does not bump `updated_at` — usage is a passive by-product of a turn that
    /// already bumped it, and re-ordering the session on every token report would
    /// be noise.
    pub fn set_chat_session_context_usage(&self, id: &str, json: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET context_usage_json = ?2 WHERE id = ?1",
            params![id, json],
        )?;
        Ok(())
    }

    pub fn set_chat_session_permission_mode(&self, id: &str, mode: &str) -> Result<()> {
        self.set_chat_session_permission_mode_value(id, Some(mode))
    }

    pub fn set_chat_session_permission_mode_value(
        &self,
        id: &str,
        mode: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET permission_mode = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, mode, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_chat_session_plan_state(
        &self,
        id: &str,
        plan_mode: bool,
        reset_pending: bool,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions
             SET plan_mode = ?2, plan_reset_pending = ?3, updated_at = ?4
             WHERE id = ?1",
            params![id, plan_mode, reset_pending, now_ms()],
        )?;
        Ok(())
    }

    pub fn clear_chat_session_plan_reset(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET plan_reset_pending = 0 WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn set_chat_session_reasoning_level(&self, id: &str, level: &str) -> Result<()> {
        self.set_chat_session_reasoning_level_value(id, Some(level))
    }

    pub fn set_chat_session_reasoning_level_value(
        &self,
        id: &str,
        level: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET reasoning_level = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, level, now_ms()],
        )?;
        Ok(())
    }

    /// Archive/unarchive. Doesn't bump `updated_at`, so the session keeps its
    /// place in the recency ordering when it comes back.
    pub fn set_chat_session_archived(&self, id: &str, archived: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET archived = ?2 WHERE id = ?1",
            params![id, archived],
        )?;
        Ok(())
    }

    /// Unconditional title write. `source` records who wrote it — see
    /// [`StoredChatSession::title_source`] for the vocabulary — which is what
    /// later lets auto-titling tell a placeholder from a title worth keeping.
    pub fn set_chat_session_title(&self, id: &str, title: &str, source: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET title = ?2, title_source = ?3, updated_at = ?4 WHERE id = ?1",
            params![id, title, source, now_ms()],
        )?;
        Ok(())
    }

    /// Adopt a generated title only while the title is still unset or the
    /// first-line placeholder. Atomic check-and-set: a user Rename (`'user'`)
    /// and a legacy row (NULL source with a non-blank title) are never
    /// overwritten, and a session that already has a `'generated'` title is
    /// never re-titled. Returns true if a row was written.
    pub fn set_chat_session_title_if_placeholder(&self, id: &str, title: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE chat_sessions SET title = ?2, title_source = 'generated', updated_at = ?3 \
             WHERE id = ?1 AND (title IS NULL OR trim(title) = '' OR title_source = 'fallback')",
            params![id, title, now_ms()],
        )?;
        Ok(n > 0)
    }

    pub fn touch_chat_session(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET updated_at = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Insert or replace a message's parts — assistant messages are rewritten
    /// as their parts stream in.
    pub fn upsert_chat_message(&self, m: &StoredChatMessage) -> Result<()> {
        upsert_chat_message_with(&self.conn, m)
    }

    /// Persist a message on the session's active branch and report its parent.
    /// `m.parent_id` is derived from the session's leaf, not read. Immediate: a
    /// torn read-leaf/append would make a permission card and its own reply
    /// siblings, hiding one of them.
    pub fn upsert_chat_message_on_branch(&self, m: &StoredChatMessage) -> Result<Option<String>> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        // Outer None = no such row yet; inner None = a row that is a branch root.
        let known: Option<Option<String>> = tx
            .query_row(
                "SELECT parent_id FROM chat_messages WHERE id = ?1",
                params![m.id],
                |row| row.get(0),
            )
            .optional()?;
        let parent = match &known {
            Some(parent) => parent.clone(),
            None => tx
                .query_row(
                    "SELECT active_leaf_id FROM chat_sessions WHERE id = ?1",
                    params![m.session_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten(),
        };
        upsert_chat_message_with(
            &tx,
            &StoredChatMessage {
                parent_id: parent.clone(),
                ..m.clone()
            },
        )?;
        if known.is_none() {
            tx.execute(
                "UPDATE chat_sessions SET active_leaf_id = ?2 WHERE id = ?1",
                params![m.session_id, m.id],
            )?;
        }
        tx.commit()?;
        Ok(parent)
    }

    pub fn upsert_chat_recovery_message_if_actionable(
        &self,
        turn_id: &str,
        message: &StoredChatMessage,
    ) -> Result<bool> {
        let transaction = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let actionable = transaction.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM chat_turns
               WHERE id = ?1 AND state = 'failed' AND recovery_action IS NOT NULL
                 AND recovered_by_turn_id IS NULL
             )",
            params![turn_id],
            |row| row.get::<_, bool>(0),
        )?;
        if actionable {
            let exists = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE id = ?1)",
                params![message.id],
                |row| row.get::<_, bool>(0),
            )?;
            upsert_chat_message_with(&transaction, message)?;
            if !exists {
                transaction.execute(
                    "UPDATE chat_sessions SET active_leaf_id = ?2 WHERE id = ?1",
                    params![message.session_id, message.id],
                )?;
            }
        }
        transaction.commit()?;
        Ok(actionable)
    }

    /// Admit one user turn and its transcript bubble as one durable operation.
    /// The `(session_id, client_turn_id)` pair is the browser-to-server
    /// idempotency key. A duplicate with the same request hash returns the
    /// existing row; a different payload is rejected without changing either
    /// the transcript or the turn.
    pub fn admit_chat_turn(
        &self,
        user_message: Option<&StoredChatMessage>,
        turn: &StoredChatTurn,
    ) -> Result<ChatTurnAdmission> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        let session_exists = tx
            .query_row(
                "SELECT 1 FROM chat_sessions WHERE id = ?1",
                params![turn.session_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !session_exists {
            return Err(anyhow!("chat session not found"));
        }
        let existing = tx
            .query_row(
                &format!(
                    "SELECT {CHAT_TURN_COLS} FROM chat_turns
                     WHERE session_id = ?1 AND client_turn_id = ?2"
                ),
                params![turn.session_id, turn.client_turn_id],
                row_to_chat_turn,
            )
            .optional()?;
        if let Some(existing) = existing {
            tx.commit()?;
            return Ok(if existing.request_hash == turn.request_hash {
                ChatTurnAdmission::Existing(Box::new(existing))
            } else {
                ChatTurnAdmission::Conflict
            });
        }
        if let Some(message) = user_message {
            tx.execute(
                "INSERT INTO chat_messages
                 (id, session_id, role, parts_json, created_at, parent_id,
                  base_native_session_id, result_native_session_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    message.id,
                    message.session_id,
                    message.role,
                    message.parts_json,
                    message.created_at,
                    message.parent_id,
                    message.base_native_session_id,
                    message.result_native_session_id,
                ],
            )?;
            tx.execute(
                "UPDATE chat_sessions SET active_leaf_id = ?2 WHERE id = ?1",
                params![message.session_id, message.id],
            )?;
        }
        tx.execute(
            "INSERT INTO chat_turns
             (id, session_id, user_message_id, assistant_message_id, client_turn_id,
              request_hash, prepared_input, settings_json, state, delivery_state,
              attempt_count, next_retry_at, error_kind, error_message, recovery_action,
              recovered_by_turn_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?16, ?17, ?18)",
            params![
                turn.id,
                turn.session_id,
                turn.user_message_id,
                turn.assistant_message_id,
                turn.client_turn_id,
                turn.request_hash,
                turn.prepared_input,
                turn.settings_json,
                turn.state,
                turn.delivery_state,
                turn.attempt_count,
                turn.next_retry_at,
                turn.error_kind,
                turn.error_message,
                turn.recovery_action,
                turn.recovered_by_turn_id,
                turn.created_at,
                turn.updated_at,
            ],
        )?;
        tx.commit()?;
        Ok(ChatTurnAdmission::Inserted)
    }

    pub fn get_chat_turn(&self, session_id: &str, turn_id: &str) -> Result<Option<StoredChatTurn>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CHAT_TURN_COLS} FROM chat_turns WHERE id = ?1 AND session_id = ?2"
                ),
                params![turn_id, session_id],
                row_to_chat_turn,
            )
            .optional()?)
    }

    pub fn get_chat_turn_by_client_id(
        &self,
        session_id: &str,
        client_turn_id: &str,
    ) -> Result<Option<StoredChatTurn>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CHAT_TURN_COLS} FROM chat_turns
                     WHERE session_id = ?1 AND client_turn_id = ?2"
                ),
                params![session_id, client_turn_id],
                row_to_chat_turn,
            )
            .optional()?)
    }

    pub fn insert_queued_chat_message(&self, message: &StoredQueuedChatMessage) -> Result<()> {
        let changed = self.conn.execute(
            "INSERT INTO chat_queued_messages
             (id, session_id, client_turn_id, request_hash, payload_json, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6
             WHERE EXISTS (SELECT 1 FROM chat_sessions WHERE id = ?2)",
            params![
                message.id,
                message.session_id,
                message.client_turn_id,
                message.request_hash,
                message.payload_json,
                message.created_at,
            ],
        )?;
        if changed == 0 {
            return Err(anyhow!("chat session not found"));
        }
        Ok(())
    }

    pub fn update_queued_chat_message(&self, id: &str, payload_json: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_queued_messages SET payload_json = ?2 WHERE id = ?1",
            params![id, payload_json],
        )?;
        Ok(())
    }

    pub fn delete_queued_chat_message(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM chat_queued_messages WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn delete_queued_chat_messages_for_session(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM chat_queued_messages WHERE session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    pub fn list_queued_chat_messages(&self) -> Result<Vec<StoredQueuedChatMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, client_turn_id, request_hash, payload_json, created_at
             FROM chat_queued_messages ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredQueuedChatMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                client_turn_id: row.get(2)?,
                request_hash: row.get(3)?,
                payload_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_queued_chat_messages_for_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<StoredQueuedChatMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, client_turn_id, request_hash, payload_json, created_at
             FROM chat_queued_messages WHERE session_id = ?1
             ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(StoredQueuedChatMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                client_turn_id: row.get(2)?,
                request_hash: row.get(3)?,
                payload_json: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn update_chat_turn_progress(
        &self,
        id: &str,
        state: &str,
        delivery_state: &str,
        attempt_count: i64,
        next_retry_at: Option<i64>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_turns SET state = ?2, delivery_state = ?3, attempt_count = ?4,
                 next_retry_at = ?5, updated_at = ?6
             WHERE id = ?1 AND state IN ('preparing', 'retrying', 'running')",
            params![
                id,
                state,
                delivery_state,
                attempt_count,
                next_retry_at,
                now_ms()
            ],
        )?;
        Ok(())
    }

    pub fn complete_chat_turn(&self, id: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE chat_turns SET state = 'completed', next_retry_at = NULL,
                 error_kind = NULL, error_message = NULL, recovery_action = NULL,
                 updated_at = ?2 WHERE id = ?1
                   AND state IN ('preparing', 'retrying', 'running')",
            params![id, now_ms()],
        )?;
        Ok(changed > 0)
    }

    pub fn fail_chat_turn(
        &self,
        id: &str,
        delivery_state: &str,
        error_kind: &str,
        error_message: &str,
        recovery_action: Option<&str>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE chat_turns SET state = 'failed', delivery_state = ?2,
                 next_retry_at = NULL, error_kind = ?3, error_message = ?4,
                 recovery_action = ?5, updated_at = ?6 WHERE id = ?1
                   AND state IN ('preparing', 'retrying', 'running')",
            params![
                id,
                delivery_state,
                error_kind,
                error_message,
                recovery_action,
                now_ms(),
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn interrupt_chat_turn(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_turns SET state = 'interrupted', next_retry_at = NULL,
                 recovery_action = NULL, error_kind = 'cancelled', updated_at = ?2
             WHERE id = ?1 AND state IN ('preparing', 'retrying', 'running', 'failed')
               AND recovered_by_turn_id IS NULL",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Claim a terminal recovery action exactly once. `Retry` reuses this row;
    /// `Continue` records the newly-created recovery turn after admission.
    pub fn mark_chat_turn_recovered(&self, id: &str, recovered_by: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE chat_turns SET recovered_by_turn_id = ?2, recovery_action = NULL,
                 updated_at = ?3
             WHERE id = ?1 AND recovered_by_turn_id IS NULL AND state = 'failed'",
            params![id, recovered_by, now_ms()],
        )?;
        Ok(changed > 0)
    }

    pub fn mark_chat_turn_recovered_with_message(
        &self,
        id: &str,
        recovered_by: &str,
        message: &StoredChatMessage,
    ) -> Result<bool> {
        let transaction = self.conn.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE chat_turns SET recovered_by_turn_id = ?2, recovery_action = NULL,
                 updated_at = ?3
             WHERE id = ?1 AND recovered_by_turn_id IS NULL AND state = 'failed'",
            params![id, recovered_by, now_ms()],
        )?;
        if changed > 0 {
            upsert_chat_message_with(&transaction, message)?;
        }
        transaction.commit()?;
        Ok(changed > 0)
    }

    pub fn reset_chat_turn_for_retry(&self, id: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE chat_turns SET state = 'preparing', delivery_state = 'not_sent',
                 attempt_count = 0, next_retry_at = NULL, error_kind = NULL,
                 error_message = NULL, recovery_action = NULL, updated_at = ?2
             WHERE id = ?1 AND state = 'failed' AND recovery_action = 'retry'
                   AND recovered_by_turn_id IS NULL",
            params![id, now_ms()],
        )?;
        Ok(changed > 0)
    }

    pub fn set_chat_turn_settings(&self, id: &str, settings_json: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_turns SET settings_json = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, settings_json, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_chat_session_recovery_settings(
        &self,
        id: &str,
        model: Option<&str>,
        service_tier: Option<&str>,
        permission_mode: Option<&str>,
        plan_state: Option<(bool, bool)>,
        reasoning_level: Option<&str>,
    ) -> Result<()> {
        let transaction = self.conn.unchecked_transaction()?;
        let (plan_mode, plan_reset_pending) = plan_state
            .map(|(mode, reset)| (Some(mode), Some(reset)))
            .unwrap_or((None, None));
        transaction.execute(
            "UPDATE chat_sessions
             SET model = ?2, service_tier = ?3, permission_mode = ?4,
                 plan_mode = COALESCE(?5, plan_mode),
                 plan_reset_pending = COALESCE(?6, plan_reset_pending),
                 reasoning_level = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                model,
                service_tier,
                permission_mode,
                plan_mode,
                plan_reset_pending,
                reasoning_level,
                now_ms()
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// On server startup no in-flight task survives. Convert unfinished rows
    /// into explicit, user-recoverable terminal states without replaying them.
    pub fn reconcile_unfinished_chat_turns(&self) -> Result<Vec<StoredChatTurn>> {
        self.reconcile_unfinished_chat_turns_inner(true)
    }

    pub fn reconcile_expired_unfinished_chat_turns(&self) -> Result<Vec<StoredChatTurn>> {
        self.reconcile_unfinished_chat_turns_inner(false)
    }

    fn reconcile_unfinished_chat_turns_inner(
        &self,
        include_existing_failures: bool,
    ) -> Result<Vec<StoredChatTurn>> {
        let now = now_ms();
        let transaction = self.conn.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM chat_turn_leases WHERE heartbeat_at < ?1",
            params![now - CHAT_TURN_LEASE_TTL_MS],
        )?;
        let newly_reconciled = {
            let mut statement = transaction.prepare(
                "SELECT id FROM chat_turns
                 WHERE state IN ('preparing', 'retrying', 'running')
                   AND NOT EXISTS (
                     SELECT 1 FROM chat_turn_leases
                     WHERE chat_turn_leases.chat_session_id = chat_turns.session_id
                   )",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<std::result::Result<std::collections::HashSet<_>, _>>()?
        };
        transaction.execute(
            "UPDATE chat_turns
             SET state = 'failed', error_kind = 'server_restarted',
                 error_message = 'ORX restarted before this turn finished',
                 recovery_action = CASE
                   WHEN delivery_state IN ('not_sent', 'rejected') THEN 'retry'
                   ELSE 'continue'
                 END,
                 next_retry_at = NULL, updated_at = ?1
             WHERE state IN ('preparing', 'retrying', 'running')
               AND NOT EXISTS (
                 SELECT 1 FROM chat_turn_leases
                 WHERE chat_turn_leases.chat_session_id = chat_turns.session_id
               )",
            params![now],
        )?;
        transaction.commit()?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_TURN_COLS} FROM chat_turns
             WHERE state = 'failed' AND recovery_action IS NOT NULL
               AND recovered_by_turn_id IS NULL
             ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map([], row_to_chat_turn)?;
        let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if include_existing_failures {
            Ok(rows)
        } else {
            Ok(rows
                .into_iter()
                .filter(|turn| newly_reconciled.contains(&turn.id))
                .collect())
        }
    }

    pub fn list_chat_messages(&self, session_id: &str) -> Result<Vec<StoredChatMessage>> {
        let mut stmt = self.conn.prepare(
            // rowid tiebreak: a user message and its reply can share a millisecond.
            "SELECT id, session_id, role, parts_json, created_at, parent_id,
                    base_native_session_id, result_native_session_id
             FROM chat_messages
             WHERE session_id = ?1 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(StoredChatMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                parts_json: row.get(3)?,
                created_at: row.get(4)?,
                parent_id: row.get(5)?,
                base_native_session_id: row.get(6)?,
                result_native_session_id: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn has_chat_messages(&self, session_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_messages WHERE session_id = ?1)",
            params![session_id],
            |row| row.get(0),
        )?)
    }

    /// A single chat message by id (used to reconcile a message's persisted
    /// state against an in-memory copy mid-turn). `None` if it doesn't exist.
    pub fn get_chat_message(&self, id: &str) -> Result<Option<StoredChatMessage>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, session_id, role, parts_json, created_at, parent_id,
                        base_native_session_id, result_native_session_id
                 FROM chat_messages WHERE id = ?1",
                params![id],
                |row| {
                    Ok(StoredChatMessage {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        role: row.get(2)?,
                        parts_json: row.get(3)?,
                        created_at: row.get(4)?,
                        parent_id: row.get(5)?,
                        base_native_session_id: row.get(6)?,
                        result_native_session_id: row.get(7)?,
                    })
                },
            )
            .optional()?)
    }

    /// The Overleaf project a `.tex` in this project pushes to, if one is linked.
    pub fn overleaf_link(&self, project_id: &str, tex_path: &str) -> Result<Option<OverleafLink>> {
        Ok(self
            .conn
            .query_row(
                "SELECT overleaf_project_id, host, head, baseline, root FROM overleaf_links
                 WHERE project_id = ?1 AND tex_path = ?2",
                params![project_id, tex_path],
                |row| {
                    let baseline: String = row.get(3)?;
                    Ok(OverleafLink {
                        overleaf_project_id: row.get(0)?,
                        host: row.get(1)?,
                        head: row.get(2)?,
                        root: row.get(4)?,
                        // A baseline that will not parse is one we cannot trust
                        // to say who changed what; empty makes the next sync
                        // treat every difference as a conflict rather than
                        // guess a direction.
                        baseline: serde_json::from_str(&baseline).unwrap_or_default(),
                    })
                },
            )
            .optional()?)
    }

    pub fn set_overleaf_link(
        &self,
        project_id: &str,
        tex_path: &str,
        link: &OverleafLink,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO overleaf_links
               (project_id, tex_path, overleaf_project_id, host, head, baseline, root)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(project_id, tex_path) DO UPDATE SET
               overleaf_project_id = excluded.overleaf_project_id,
               host = excluded.host,
               head = excluded.head,
               baseline = excluded.baseline,
               root = excluded.root",
            params![
                project_id,
                tex_path,
                link.overleaf_project_id,
                link.host,
                link.head,
                serde_json::to_string(&link.baseline)?,
                link.root
            ],
        )?;
        Ok(())
    }

    pub fn clear_overleaf_link(&self, project_id: &str, tex_path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM overleaf_links WHERE project_id = ?1 AND tex_path = ?2",
            params![project_id, tex_path],
        )?;
        Ok(())
    }

    pub fn upsert_ssh_host_test(&self, t: &SshHostTest) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ssh_host_tests (host, reachable, git_found, tools_found, missing_tools, error, tested_at)
             VALUES (?1, ?2, 0, ?3, ?4, ?5, ?6)
             ON CONFLICT(host) DO UPDATE SET
               reachable = excluded.reachable,
               tools_found = excluded.tools_found,
               missing_tools = excluded.missing_tools,
               error = excluded.error,
               tested_at = excluded.tested_at",
            params![
                t.host,
                t.reachable,
                t.tools_found,
                t.missing_tools.join(","),
                t.error,
                t.tested_at
            ],
        )?;
        Ok(())
    }

    pub fn list_ssh_host_tests(&self) -> Result<Vec<SshHostTest>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT host, reachable, tools_found, missing_tools, error, tested_at FROM ssh_host_tests",
            )?;
        let rows = stmt.query_map([], |row| {
            let missing_tools = row.get::<_, String>(3)?;
            Ok(SshHostTest {
                host: row.get(0)?,
                reachable: row.get(1)?,
                tools_found: row.get(2)?,
                missing_tools: missing_tools
                    .split(',')
                    .filter(|tool| !tool.is_empty())
                    .map(str::to_string)
                    .collect(),
                error: row.get(4)?,
                tested_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

/// The Overleaf project one `.tex` file pushes to. Per file, not per project:
/// a checkout can hold a paper and a rebuttal, and they are different Overleaf
/// projects.
#[derive(Debug, Clone)]
pub struct OverleafLink {
    pub overleaf_project_id: String,
    /// `www.overleaf.com`, or a Server Pro site.
    pub host: String,
    /// Overleaf's HEAD at the last sync. A poll that sees the same sha knows
    /// nothing moved there, and can skip the clone.
    pub head: String,
    /// What both sides agreed on at the last sync (path to content hash) — the
    /// only thing that gives a later difference a direction.
    pub baseline: std::collections::BTreeMap<String, String>,
    /// The checkout the baseline was taken from. A session worktree and the hub
    /// clone hold different copies of the same path, and an agreement made
    /// against one says nothing about the other.
    pub root: String,
}

/// Most recent preflight result per ssh host alias (Settings → Compute → SSH).
/// Serializes to the wire shape the UI's `SshPreflight` type expects; `host`
/// is the row key only (the API embeds results under their host entry).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SshHostTest {
    #[serde(skip_serializing)]
    pub host: String,
    pub reachable: bool,
    pub tools_found: bool,
    pub missing_tools: Vec<String>,
    pub error: Option<String>,
    /// Unix millis.
    pub tested_at: i64,
}

/// One chat thread with a harness. `native_session_id` is the harness's own
/// session/rollout id (set after the first turn for CLIs that mint it lazily).
#[derive(Debug, Clone)]
pub struct StoredChatSession {
    pub id: String,
    pub project_id: String,
    pub harness: String,
    pub native_session_id: Option<String>,
    pub title: Option<String>,
    /// Who wrote `title`: `"fallback"` (first-line placeholder), `"generated"`
    /// (harness auto-title), `"user"` (explicitly chosen — a rename, or an
    /// agent's `orx agent spawn --title`, neither of which auto-titling may
    /// overwrite). NULL on legacy rows, which the conditional setter treats as
    /// "unknown, don't overwrite".
    pub title_source: Option<String>,
    pub model: Option<String>,
    /// Codex processing tier (`"default"` or `"priority"`); None uses CLI configuration.
    pub service_tier: Option<String>,
    /// Permission-mode wire id (`"auto"` / `"plan"` / …); None = harness default.
    pub permission_mode: Option<String>,
    /// Independent Plan state for Codex/OpenCode. Claude keeps Plan in
    /// `permission_mode`, matching its native permission-mode model.
    pub plan_mode: bool,
    /// Codex must send one `collaborationMode: default` after leaving Plan;
    /// persisted so a restart cannot strand the native thread in Plan.
    pub plan_reset_pending: bool,
    /// Reasoning-level wire id (`"low"` / `"medium"` / `"high"`); None = default.
    pub reasoning_level: Option<String>,
    /// Hidden from the default Recents list, but fully intact and resumable.
    pub archived: bool,
    /// Serialized `ContextUsage` for the latest turn; None until first reported.
    pub context_usage_json: Option<String>,
    /// Hidden context prepended only when a seeded transcript starts its first
    /// real native harness session. Never serialized to the UI.
    pub bootstrap_context: Option<String>,
    /// Tip of the branch the UI is currently showing. Forked turns make the
    /// transcript a tree; this picks which path through it is live.
    pub active_leaf_id: Option<String>,
    /// Session that spawned this one with `orx agent spawn`. `None` for
    /// sessions the user started from the dashboard.
    pub parent_session_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredAgentSelection {
    pub harness: String,
    pub model: Option<String>,
    pub service_tier: Option<String>,
    pub permission_mode: Option<String>,
    pub reasoning_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredUiState {
    pub onboarding_completed: bool,
    pub tour_completed: bool,
    pub preferred_agent: Option<StoredAgentSelection>,
}

/// Normalized transcript entry; `parts_json` is the wire-format parts array
/// the UI renders (orx is the system of record for transcripts, not the
/// harness's own storage).
#[derive(Debug, Clone)]
pub struct StoredChatMessage {
    pub id: String,
    pub session_id: String,
    pub role: String,
    pub parts_json: String,
    pub created_at: i64,
    /// Message this one follows on its branch. NULL only for a branch root.
    pub parent_id: Option<String>,
    /// Harness session id current *before* this turn ran. Re-sampling a turn
    /// resumes from it, so a fork branches the harness's own history instead of
    /// appending onto the sibling's.
    pub base_native_session_id: Option<String>,
    /// Harness session id this turn ended at, recorded on the assistant reply.
    /// Selecting a branch restores it, so the next message continues the branch
    /// on screen rather than whichever fork ran most recently.
    pub result_native_session_id: Option<String>,
}

fn upsert_chat_message_with(conn: &Connection, m: &StoredChatMessage) -> Result<()> {
    conn.execute(
        // A streaming message is written many times; only its parts and a
        // late-arriving harness session id may change, never its place in
        // the tree.
        "INSERT INTO chat_messages
                 (id, session_id, role, parts_json, created_at, parent_id,
                  base_native_session_id, result_native_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 parts_json = excluded.parts_json,
                 result_native_session_id = COALESCE(
                     excluded.result_native_session_id, result_native_session_id)",
        params![
            m.id,
            m.session_id,
            m.role,
            m.parts_json,
            m.created_at,
            m.parent_id,
            m.base_native_session_id,
            m.result_native_session_id
        ],
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredChatTurn {
    pub id: String,
    pub session_id: String,
    pub user_message_id: Option<String>,
    pub assistant_message_id: String,
    pub client_turn_id: String,
    pub request_hash: String,
    pub prepared_input: String,
    pub settings_json: String,
    pub state: String,
    pub delivery_state: String,
    pub attempt_count: i64,
    pub next_retry_at: Option<i64>,
    pub error_kind: Option<String>,
    pub error_message: Option<String>,
    pub recovery_action: Option<String>,
    pub recovered_by_turn_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct StoredQueuedChatMessage {
    pub id: String,
    pub session_id: String,
    pub client_turn_id: String,
    pub request_hash: String,
    pub payload_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatTurnAdmission {
    Inserted,
    Existing(Box<StoredChatTurn>),
    Conflict,
}

const CHAT_TURN_COLS: &str = "id, session_id, user_message_id, assistant_message_id, \
    client_turn_id, request_hash, prepared_input, settings_json, state, delivery_state, \
    attempt_count, next_retry_at, error_kind, error_message, recovery_action, \
    recovered_by_turn_id, created_at, updated_at";

fn row_to_chat_turn(
    row: &rusqlite::Row<'_>,
) -> std::result::Result<StoredChatTurn, rusqlite::Error> {
    Ok(StoredChatTurn {
        id: row.get(0)?,
        session_id: row.get(1)?,
        user_message_id: row.get(2)?,
        assistant_message_id: row.get(3)?,
        client_turn_id: row.get(4)?,
        request_hash: row.get(5)?,
        prepared_input: row.get(6)?,
        settings_json: row.get(7)?,
        state: row.get(8)?,
        delivery_state: row.get(9)?,
        attempt_count: row.get(10)?,
        next_retry_at: row.get(11)?,
        error_kind: row.get(12)?,
        error_message: row.get(13)?,
        recovery_action: row.get(14)?,
        recovered_by_turn_id: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

const CHAT_SESSION_COLS: &str = "id, project_id, harness, native_session_id, title, model, service_tier, \
     permission_mode, plan_mode, plan_reset_pending, reasoning_level, archived, context_usage_json, \
     created_at, updated_at, title_source, bootstrap_context, active_leaf_id, parent_session_id";

fn row_to_chat_spawn(row: &rusqlite::Row<'_>) -> std::result::Result<ChatSpawn, rusqlite::Error> {
    Ok(ChatSpawn {
        session_id: row.get(0)?,
        parent_session_id: row.get(1)?,
        prompt: row.get(2)?,
        wake_parent: row.get(3)?,
        attempts: row.get(4)?,
        finished_at: row.get(5)?,
    })
}

fn row_to_chat_session(
    row: &rusqlite::Row<'_>,
) -> std::result::Result<StoredChatSession, rusqlite::Error> {
    Ok(StoredChatSession {
        id: row.get(0)?,
        project_id: row.get(1)?,
        harness: row.get(2)?,
        native_session_id: row.get(3)?,
        title: row.get(4)?,
        model: row.get(5)?,
        service_tier: row.get(6)?,
        permission_mode: row.get(7)?,
        plan_mode: row.get(8)?,
        plan_reset_pending: row.get(9)?,
        reasoning_level: row.get(10)?,
        archived: row.get(11)?,
        context_usage_json: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        title_source: row.get(15)?,
        bootstrap_context: row.get(16)?,
        active_leaf_id: row.get(17)?,
        parent_session_id: row.get(18)?,
    })
}

const SELECT_RUN: &str = "SELECT id, experiment_id, project_id, status, backend_json, command,
                                 created_at, updated_at, ended_at, exit_code,
                                 commit_sha, result_markdown, cancel_requested,
                                 chat_session_id FROM runs";

const PROJECT_COLS: &str = "id, name, slug, github_owner, github_repo, github_sync_enabled, \
                            baseline_branch, repo_path, run_command, paper_id, created_at, updated_at";

const EXPERIMENT_COLS: &str = "id, project_id, parent_experiment_id, slug, branch_name, \
                               title, description, run_command, agent_status, created_at, \
                               updated_at, chat_session_id";

fn row_to_run(row: &rusqlite::Row<'_>) -> std::result::Result<StoredRun, rusqlite::Error> {
    Ok(StoredRun {
        id: row.get(0)?,
        experiment_id: row.get(1)?,
        project_id: row.get(2)?,
        status: row.get(3)?,
        backend_json: row.get(4)?,
        command: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        ended_at: row.get(8)?,
        exit_code: row.get(9)?,
        commit_sha: row.get(10)?,
        result_markdown: row.get(11)?,
        cancel_requested: row.get(12)?,
        chat_session_id: row.get(13)?,
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_scales() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn legacy_ssh_tool_failures_require_a_fresh_check() {
        let dir = std::env::temp_dir().join(format!(
            "orx-store-legacy-ssh-tests-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("orx.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE ssh_host_tests (
                 host TEXT PRIMARY KEY,
                 reachable INTEGER NOT NULL,
                 git_found INTEGER NOT NULL,
                 tools_found INTEGER NOT NULL DEFAULT 0,
                 error TEXT,
                 tested_at INTEGER NOT NULL
             );
             INSERT INTO ssh_host_tests VALUES ('legacy-tools', 1, 0, 0, NULL, 1);
             INSERT INTO ssh_host_tests VALUES ('legacy-connect', 0, 0, 0, 'timed out', 2);",
        )
        .unwrap();
        drop(conn);

        let store = Store::open_at(dir.clone()).unwrap();
        let tests = store.list_ssh_host_tests().unwrap();
        assert_eq!(tests.len(), 1);
        assert_eq!(tests[0].host, "legacy-connect");

        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ui_state_roundtrips_functional_preferences() {
        let dir = std::env::temp_dir().join(format!("orx-store-ui-state-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        assert_eq!(
            store.ui_state().unwrap(),
            StoredUiState {
                onboarding_completed: false,
                tour_completed: false,
                preferred_agent: None,
            }
        );

        let selection = StoredAgentSelection {
            harness: "codex".into(),
            model: Some("gpt-5.6".into()),
            service_tier: Some("priority".into()),
            permission_mode: Some("plan".into()),
            reasoning_level: Some("high".into()),
        };
        store.set_onboarding_completed(true).unwrap();
        store.set_tour_completed(true).unwrap();
        store.set_preferred_agent(&selection).unwrap();

        assert_eq!(
            store.ui_state().unwrap(),
            StoredUiState {
                onboarding_completed: true,
                tour_completed: true,
                preferred_agent: Some(selection),
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_state_singleton_seeds_existing_projects_and_latest_session() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-ui-migrate-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_local_project(&LocalProject {
                id: "project".into(),
                name: "Project".into(),
                slug: "project".into(),
                github_owner: String::new(),
                github_repo: String::new(),
                github_sync_enabled: false,
                baseline_branch: "main".into(),
                repo_path: dir.join("project").to_string_lossy().into_owned(),
                run_command: None,
                paper_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let mut older = chat_session_fixture("older");
        older.harness = "codex".into();
        store.create_chat_session(&older).unwrap();
        let mut session = chat_session_fixture("latest");
        session.harness = "claude-code".into();
        session.model = Some("model".into());
        session.permission_mode = Some("plan".into());
        session.updated_at = 2;
        store.create_chat_session(&session).unwrap();
        store.conn.execute("DELETE FROM ui_state", []).unwrap();
        drop(store);

        let migrated = Store::open_at(dir.clone()).unwrap().ui_state().unwrap();
        assert!(migrated.onboarding_completed);
        assert!(migrated.tour_completed);
        let preferred = migrated.preferred_agent.unwrap();
        assert_eq!(preferred.harness, "claude-code");
        assert_eq!(preferred.permission_mode.as_deref(), Some("auto"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_session_context_usage_roundtrips() {
        let dir = std::env::temp_dir().join(format!("orx-store-ctxusage-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        // Fresh session: no usage yet.
        assert!(store
            .get_chat_session("chat_1")
            .unwrap()
            .unwrap()
            .context_usage_json
            .is_none());
        // Set, then read it back verbatim.
        let json = r#"{"usedTokens":27564,"contextWindow":200000}"#;
        store
            .set_chat_session_context_usage("chat_1", json)
            .unwrap();
        assert_eq!(
            store
                .get_chat_session("chat_1")
                .unwrap()
                .unwrap()
                .context_usage_json
                .as_deref(),
            Some(json)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn permission_and_plan_migration_preserves_each_harness_native_contract() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-plan-migrate-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(dir.clone()).unwrap();
            for (id, harness, mode) in [
                ("claude_manual", "claude-code", "ask"),
                ("claude_edits", "claude-code", "accept-edits"),
                ("claude_plan", "claude-code", "plan"),
                ("codex_plan", "codex", "plan"),
                ("codex_auto", "codex", "auto"),
                ("codex_bypass", "codex", "bypass"),
                ("codex_invalid", "codex", "retired-mode"),
                ("open_plan", "opencode", "plan"),
                ("open_auto", "opencode", "auto"),
                ("open_bypass", "opencode", "bypass"),
            ] {
                let mut session = chat_session_fixture(id);
                session.harness = harness.into();
                session.permission_mode = Some(mode.into());
                store.create_chat_session(&session).unwrap();
            }
        }
        let store = Store::open_at(dir.clone()).unwrap();
        let state = |id: &str| {
            let session = store.get_chat_session(id).unwrap().unwrap();
            (session.permission_mode.unwrap(), session.plan_mode)
        };
        assert_eq!(state("claude_manual"), ("manual".into(), false));
        assert_eq!(state("claude_edits"), ("acceptEdits".into(), false));
        assert_eq!(state("claude_plan"), ("plan".into(), false));
        assert_eq!(state("codex_plan"), ("ask".into(), true));
        assert_eq!(state("codex_auto"), ("approve-for-me".into(), false));
        assert_eq!(state("codex_bypass"), ("full-access".into(), false));
        assert_eq!(state("codex_invalid"), ("approve-for-me".into(), false));
        assert_eq!(state("open_plan"), ("default".into(), true));
        assert_eq!(state("open_auto"), ("default".into(), false));
        assert_eq!(state("open_bypass"), ("auto-approve".into(), false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn codex_plan_reset_marker_roundtrips_and_clears() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-plan-reset-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let mut session = chat_session_fixture("codex");
        session.harness = "codex".into();
        session.permission_mode = Some("approve-for-me".into());
        store.create_chat_session(&session).unwrap();
        store
            .set_chat_session_plan_state("codex", false, true)
            .unwrap();
        assert!(
            store
                .get_chat_session("codex")
                .unwrap()
                .unwrap()
                .plan_reset_pending
        );
        store.clear_chat_session_plan_reset("codex").unwrap();
        assert!(
            !store
                .get_chat_session("codex")
                .unwrap()
                .unwrap()
                .plan_reset_pending
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_message_existence_tracks_persisted_messages() {
        let dir = std::env::temp_dir().join(format!("orx-store-messages-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        assert!(!store.has_chat_messages("chat_1").unwrap());

        store
            .upsert_chat_message(&StoredChatMessage {
                id: "msg_1".into(),
                session_id: "chat_1".into(),
                role: "user".into(),
                parts_json: "[]".into(),
                created_at: 1,
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: None,
            })
            .unwrap();
        assert!(store.has_chat_messages("chat_1").unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn chat_turn_fixture(id: &str, client_turn_id: &str) -> StoredChatTurn {
        StoredChatTurn {
            id: id.into(),
            session_id: "chat_1".into(),
            user_message_id: Some(format!("msg_{id}")),
            assistant_message_id: format!("assistant_{id}"),
            client_turn_id: client_turn_id.into(),
            request_hash: "hash-a".into(),
            prepared_input: "prepared".into(),
            settings_json: "{}".into(),
            state: "preparing".into(),
            delivery_state: "not_sent".into(),
            attempt_count: 0,
            next_retry_at: None,
            error_kind: None,
            error_message: None,
            recovery_action: None,
            recovered_by_turn_id: None,
            created_at: 2,
            updated_at: 2,
        }
    }

    #[test]
    fn chat_turn_admission_is_atomic_and_idempotent() {
        let dir = std::env::temp_dir().join(format!("orx-store-turns-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let turn = chat_turn_fixture("one", "client-1");
        let user = StoredChatMessage {
            id: turn.user_message_id.clone().unwrap(),
            session_id: "chat_1".into(),
            role: "user".into(),
            parts_json: "[]".into(),
            created_at: 2,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
        };
        assert_eq!(
            store.admit_chat_turn(Some(&user), &turn).unwrap(),
            ChatTurnAdmission::Inserted
        );
        assert!(matches!(
            store.admit_chat_turn(Some(&user), &turn).unwrap(),
            ChatTurnAdmission::Existing(existing) if existing.id == turn.id
        ));
        assert_eq!(store.list_chat_messages("chat_1").unwrap().len(), 1);
        assert_eq!(
            store
                .get_chat_session("chat_1")
                .unwrap()
                .unwrap()
                .active_leaf_id
                .as_deref(),
            Some(user.id.as_str())
        );

        let mut conflict = turn.clone();
        conflict.id = "two".into();
        conflict.request_hash = "hash-b".into();
        assert_eq!(
            store.admit_chat_turn(None, &conflict).unwrap(),
            ChatTurnAdmission::Conflict
        );
        assert_eq!(
            store
                .get_chat_turn_by_client_id("chat_1", "client-1")
                .unwrap()
                .unwrap()
                .id,
            "one"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unfinished_turns_reconcile_by_delivery_certainty() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-reconcile-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let retry = chat_turn_fixture("retry", "client-retry");
        store.admit_chat_turn(None, &retry).unwrap();
        let mut keep_going = chat_turn_fixture("continue", "client-continue");
        keep_going.delivery_state = "unknown".into();
        keep_going.state = "running".into();
        store.admit_chat_turn(None, &keep_going).unwrap();

        let reconciled = store.reconcile_unfinished_chat_turns().unwrap();
        assert_eq!(reconciled.len(), 2);
        assert_eq!(
            store
                .get_chat_turn("chat_1", "retry")
                .unwrap()
                .unwrap()
                .recovery_action
                .as_deref(),
            Some("retry")
        );
        assert_eq!(
            store
                .get_chat_turn("chat_1", "continue")
                .unwrap()
                .unwrap()
                .recovery_action
                .as_deref(),
            Some("continue")
        );
        assert!(store
            .mark_chat_turn_recovered("continue", "recovery-turn")
            .unwrap());
        let reconciled_again = store.reconcile_unfinished_chat_turns().unwrap();
        assert!(reconciled_again.iter().all(|turn| turn.id != "continue"));
        store.delete_chat_session("chat_1").unwrap();
        assert!(store.get_chat_turn("chat_1", "retry").unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unfinished_turn_reconciliation_respects_live_leases() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-live-turn-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let mut turn = chat_turn_fixture("running", "client-running");
        turn.state = "running".into();
        store.admit_chat_turn(None, &turn).unwrap();
        assert!(store.claim_chat_turn("chat_1", "owner").unwrap());

        assert!(store.reconcile_unfinished_chat_turns().unwrap().is_empty());
        assert_eq!(
            store
                .get_chat_turn("chat_1", "running")
                .unwrap()
                .unwrap()
                .state,
            "running"
        );

        store
            .conn
            .execute(
                "UPDATE chat_turn_leases SET heartbeat_at = ?1 WHERE chat_session_id = 'chat_1'",
                params![now_ms() - CHAT_TURN_LEASE_TTL_MS - 1],
            )
            .unwrap();
        assert_eq!(
            store
                .reconcile_expired_unfinished_chat_turns()
                .unwrap()
                .len(),
            1
        );
        assert!(store
            .reconcile_expired_unfinished_chat_turns()
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_chains_a_legacy_transcript_into_one_branch() {
        let dir = std::env::temp_dir().join(format!("orx-store-tree-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        for (i, (id, role)) in [("m1", "user"), ("m2", "assistant"), ("m3", "user")]
            .into_iter()
            .enumerate()
        {
            store
                .upsert_chat_message(&StoredChatMessage {
                    id: id.into(),
                    session_id: "chat_1".into(),
                    role: role.into(),
                    parts_json: "[]".into(),
                    created_at: i as i64,
                    parent_id: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                })
                .unwrap();
        }
        // Rewind the marker so reopening replays the backfill against rows that
        // look exactly like a pre-fork database: every parent NULL.
        store.conn.pragma_update(None, "user_version", 0).unwrap();
        store
            .conn
            .execute(
                "UPDATE chat_sessions SET active_leaf_id = NULL, native_session_id = 'sess-abc'",
                [],
            )
            .unwrap();
        drop(store);

        let store = Store::open_at(dir.clone()).unwrap();
        let parents = store
            .list_chat_messages("chat_1")
            .unwrap()
            .into_iter()
            .map(|m| (m.id, m.parent_id))
            .collect::<Vec<_>>();
        assert_eq!(
            parents,
            vec![
                ("m1".to_string(), None),
                ("m2".to_string(), Some("m1".into())),
                ("m3".to_string(), Some("m2".into())),
            ]
        );
        // The session resumes where it left off, not at the start.
        let session = store.get_chat_session("chat_1").unwrap().unwrap();
        assert_eq!(session.active_leaf_id.as_deref(), Some("m3"));
        // The newest message inherits the live harness id, so paging back to a
        // pre-upgrade branch resumes that conversation instead of starting over.
        let tip = store.get_chat_message("m3").unwrap().unwrap();
        assert_eq!(tip.result_native_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(
            store
                .get_chat_message("m1")
                .unwrap()
                .unwrap()
                .result_native_session_id,
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retry_reuses_the_turn_without_duplicating_the_user_message() {
        let dir = std::env::temp_dir().join(format!("orx-store-retry-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let turn = chat_turn_fixture("turn_1", "client_1");
        let user = StoredChatMessage {
            id: turn.user_message_id.clone().unwrap(),
            session_id: "chat_1".into(),
            role: "user".into(),
            parts_json: "[]".into(),
            created_at: 1,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
        };
        store.admit_chat_turn(Some(&user), &turn).unwrap();
        assert!(store
            .fail_chat_turn("turn_1", "not_sent", "setup", "failed", Some("retry"))
            .unwrap());
        assert!(store.reset_chat_turn_for_retry("turn_1").unwrap());
        assert_eq!(store.list_chat_messages("chat_1").unwrap().len(), 1);
        assert_eq!(
            store
                .get_chat_turn("chat_1", "turn_1")
                .unwrap()
                .unwrap()
                .state,
            "preparing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_interrupt_clears_a_racing_terminal_recovery() {
        let dir = std::env::temp_dir().join(format!(
            "orx-store-interrupt-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let turn = chat_turn_fixture("turn_1", "client_1");
        store.admit_chat_turn(None, &turn).unwrap();
        assert!(store
            .fail_chat_turn("turn_1", "not_sent", "setup", "failed", Some("retry"))
            .unwrap());

        store.interrupt_chat_turn("turn_1").unwrap();

        let interrupted = store.get_chat_turn("chat_1", "turn_1").unwrap().unwrap();
        assert_eq!(interrupted.state, "interrupted");
        assert!(interrupted.recovery_action.is_none());
        assert!(!store.reset_chat_turn_for_retry("turn_1").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn continue_recovery_claim_and_message_cleanup_are_idempotent() {
        let dir = std::env::temp_dir().join(format!("orx-store-continue-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let mut turn = chat_turn_fixture("turn_1", "client_1");
        turn.state = "failed".into();
        turn.delivery_state = "unknown".into();
        turn.recovery_action = Some("continue".into());
        store.admit_chat_turn(None, &turn).unwrap();
        let assistant = StoredChatMessage {
            id: turn.assistant_message_id.clone(),
            session_id: "chat_1".into(),
            role: "assistant".into(),
            parts_json: "[]".into(),
            created_at: 1,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
        };
        assert!(store
            .mark_chat_turn_recovered_with_message("turn_1", "recovery_1", &assistant)
            .unwrap());
        assert!(!store
            .mark_chat_turn_recovered_with_message("turn_1", "recovery_2", &assistant)
            .unwrap());
        let recovered = store.get_chat_turn("chat_1", "turn_1").unwrap().unwrap();
        assert_eq!(
            recovered.recovered_by_turn_id.as_deref(),
            Some("recovery_1")
        );
        assert!(recovered.recovery_action.is_none());
        assert_eq!(
            store
                .get_chat_message(&assistant.id)
                .unwrap()
                .unwrap()
                .parts_json,
            "[]"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn synthetic_recovery_message_stays_on_the_failed_turn_branch() {
        let dir = std::env::temp_dir().join(format!(
            "orx-store-recovery-branch-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let mut turn = chat_turn_fixture("turn_1", "client_1");
        turn.state = "failed".into();
        turn.recovery_action = Some("retry".into());
        let user = StoredChatMessage {
            id: turn.user_message_id.clone().unwrap(),
            session_id: "chat_1".into(),
            role: "user".into(),
            parts_json: "[]".into(),
            created_at: 1,
            parent_id: None,
            base_native_session_id: Some("native_1".into()),
            result_native_session_id: None,
        };
        store.admit_chat_turn(Some(&user), &turn).unwrap();
        let recovery = StoredChatMessage {
            id: turn.assistant_message_id.clone(),
            session_id: "chat_1".into(),
            role: "assistant".into(),
            parts_json: "[]".into(),
            created_at: 2,
            parent_id: Some(user.id.clone()),
            base_native_session_id: Some("native_1".into()),
            result_native_session_id: None,
        };
        assert!(store
            .upsert_chat_recovery_message_if_actionable(&turn.id, &recovery)
            .unwrap());
        assert_eq!(
            store
                .get_chat_message(&recovery.id)
                .unwrap()
                .unwrap()
                .parent_id
                .as_deref(),
            Some(user.id.as_str())
        );
        assert_eq!(
            store
                .get_chat_session("chat_1")
                .unwrap()
                .unwrap()
                .active_leaf_id
                .as_deref(),
            Some(recovery.id.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_settings_replace_nullable_axes_atomically() {
        let dir = std::env::temp_dir().join(format!("orx-store-settings-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let mut session = chat_session_fixture("chat_1");
        session.model = Some("old-model".into());
        session.service_tier = Some("priority".into());
        session.permission_mode = Some("ask".into());
        session.reasoning_level = Some("high".into());
        store.create_chat_session(&session).unwrap();

        store
            .set_chat_session_recovery_settings(
                "chat_1",
                None,
                Some("default"),
                None,
                Some((true, false)),
                Some("low"),
            )
            .unwrap();
        let recovered = store.get_chat_session("chat_1").unwrap().unwrap();
        assert!(recovered.model.is_none());
        assert_eq!(recovered.service_tier.as_deref(), Some("default"));
        assert!(recovered.permission_mode.is_none());
        assert!(recovered.plan_mode);
        assert_eq!(recovered.reasoning_level.as_deref(), Some("low"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queued_chat_messages_survive_reopen_and_delete_with_session() {
        let dir = std::env::temp_dir().join(format!("orx-store-queue-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(dir.clone()).unwrap();
            store
                .create_chat_session(&chat_session_fixture("chat_1"))
                .unwrap();
            store
                .insert_queued_chat_message(&StoredQueuedChatMessage {
                    id: "queue-1".into(),
                    session_id: "chat_1".into(),
                    client_turn_id: "client-1".into(),
                    request_hash: "hash-1".into(),
                    payload_json: "{\"text\":\"hello\"}".into(),
                    created_at: 1,
                })
                .unwrap();
            store
                .insert_queued_chat_message(&StoredQueuedChatMessage {
                    id: "queue-2".into(),
                    session_id: "chat_1".into(),
                    client_turn_id: "client-2".into(),
                    request_hash: "hash-2".into(),
                    payload_json: "{\"text\":\"world\"}".into(),
                    created_at: 1,
                })
                .unwrap();
        }
        let store = Store::open_at(dir.clone()).unwrap();
        let queued = store.list_queued_chat_messages().unwrap();
        assert_eq!(
            queued
                .iter()
                .map(|message| message.client_turn_id.as_str())
                .collect::<Vec<_>>(),
            ["client-1", "client-2"]
        );
        store.delete_chat_session("chat_1").unwrap();
        assert!(store.list_queued_chat_messages().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_branched_message_keeps_its_parent_across_streaming_rewrites() {
        let dir = std::env::temp_dir().join(format!("orx-store-branch-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let message = |id: &str, role: &str, parts: &str| StoredChatMessage {
            id: id.into(),
            session_id: "chat_1".into(),
            role: role.into(),
            parts_json: parts.into(),
            created_at: 0,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
        };
        assert_eq!(
            store
                .upsert_chat_message_on_branch(&message("m1", "user", "[]"))
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .upsert_chat_message_on_branch(&message("m2", "assistant", "[1]"))
                .unwrap(),
            Some("m1".to_string())
        );
        // A later flush of the same message refreshes its parts without
        // re-parenting it or advancing the branch past itself.
        assert_eq!(
            store
                .upsert_chat_message_on_branch(&message("m2", "assistant", "[1,2]"))
                .unwrap(),
            Some("m1".to_string())
        );
        let stored = store.get_chat_message("m2").unwrap().unwrap();
        assert_eq!(stored.parts_json, "[1,2]");
        assert_eq!(stored.parent_id.as_deref(), Some("m1"));
        assert_eq!(
            store
                .get_chat_session("chat_1")
                .unwrap()
                .unwrap()
                .active_leaf_id
                .as_deref(),
            Some("m2")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn chat_session_fixture(id: &str) -> StoredChatSession {
        StoredChatSession {
            id: id.into(),
            project_id: "proj_1".into(),
            harness: "claude-code".into(),
            native_session_id: None,
            title: None,
            title_source: None,
            model: None,
            service_tier: None,
            permission_mode: None,
            plan_mode: false,
            plan_reset_pending: false,
            reasoning_level: None,
            archived: false,
            context_usage_json: None,
            bootstrap_context: None,
            active_leaf_id: None,
            parent_session_id: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn chat_spawn_fixture(session_id: &str, parent: &str) -> ChatSpawn {
        ChatSpawn {
            session_id: session_id.into(),
            parent_session_id: parent.into(),
            prompt: "Sweep the literature for LoRA rank ablations".into(),
            wake_parent: true,
            attempts: 0,
            finished_at: None,
        }
    }

    #[test]
    fn spawned_sessions_record_their_parent() {
        let dir = std::env::temp_dir().join(format!("orx-store-spawnrow-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        store
            .create_chat_session(&chat_session_fixture("chat_parent"))
            .unwrap();
        let mut child = chat_session_fixture("chat_child");
        child.parent_session_id = Some("chat_parent".into());
        store.create_chat_session(&child).unwrap();

        assert!(store
            .get_chat_session("chat_parent")
            .unwrap()
            .unwrap()
            .parent_session_id
            .is_none());
        assert_eq!(
            store
                .get_chat_session("chat_child")
                .unwrap()
                .unwrap()
                .parent_session_id
                .as_deref(),
            Some("chat_parent")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_one_claimant_advances_a_spawn() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-spawnclaim-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_child"))
            .unwrap();
        store
            .create_chat_spawn(&chat_spawn_fixture("chat_child", "chat_parent"))
            .unwrap();

        let pending = store.list_chat_spawns(ChatSpawnState::Pending).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].parent_session_id, "chat_parent");
        assert!(pending[0].wake_parent);

        let token = store
            .claim_chat_spawn(
                "chat_child",
                ChatSpawnState::Pending,
                ChatSpawnState::Starting,
            )
            .unwrap()
            .expect("first claim wins");
        assert!(store
            .claim_chat_spawn(
                "chat_child",
                ChatSpawnState::Pending,
                ChatSpawnState::Starting
            )
            .unwrap()
            .is_none());
        // A settle under the wrong token must not move the row.
        assert!(!store
            .settle_chat_spawn("chat_child", "not-the-token", ChatSpawnState::Running)
            .unwrap());
        assert!(store
            .settle_chat_spawn("chat_child", &token, ChatSpawnState::Running)
            .unwrap());
        assert_eq!(
            store
                .list_chat_spawns(ChatSpawnState::Running)
                .unwrap()
                .len(),
            1
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_reclaims_stale_spawns_and_drops_deleted_sessions() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-spawnprune-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["chat_stuck", "chat_gone"] {
            store
                .create_chat_session(&chat_session_fixture(id))
                .unwrap();
            store
                .create_chat_spawn(&chat_spawn_fixture(id, "chat_parent"))
                .unwrap();
        }

        // A watcher that claimed the start and then died.
        store
            .claim_chat_spawn(
                "chat_stuck",
                ChatSpawnState::Pending,
                ChatSpawnState::Starting,
            )
            .unwrap()
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE chat_spawns SET claimed_at = ?1 WHERE session_id = 'chat_stuck'",
                params![now_ms() - CHAT_SPAWN_CLAIM_TTL_MS - 1],
            )
            .unwrap();
        // A session removed WITHOUT its spawn row (the `delete_local_project`
        // path) is what prune's orphan sweep is actually for.
        store
            .conn
            .execute("DELETE FROM chat_sessions WHERE id = 'chat_gone'", [])
            .unwrap();

        store.prune_chat_spawns().unwrap();
        let pending = store.list_chat_spawns(ChatSpawnState::Pending).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_id, "chat_stuck");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn title_source_roundtrips_and_defaults_to_none() {
        let dir = std::env::temp_dir().join(format!("orx-store-titlesrc-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let fresh = store.get_chat_session("chat_1").unwrap().unwrap();
        assert!(fresh.title.is_none());
        assert!(fresh.title_source.is_none());

        store
            .set_chat_session_title("chat_1", "First line…", "fallback")
            .unwrap();
        let after = store.get_chat_session("chat_1").unwrap().unwrap();
        assert_eq!(after.title.as_deref(), Some("First line…"));
        assert_eq!(after.title_source.as_deref(), Some("fallback"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn title_if_placeholder_respects_provenance() {
        let dir = std::env::temp_dir().join(format!("orx-store-titleph-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["untitled", "fallback", "renamed", "legacy"] {
            store
                .create_chat_session(&chat_session_fixture(id))
                .unwrap();
        }

        // No title at all (a harness-native title arriving before any user
        // message) → filled.
        assert!(store
            .set_chat_session_title_if_placeholder("untitled", "Generated one")
            .unwrap());
        assert_eq!(
            store
                .get_chat_session("untitled")
                .unwrap()
                .unwrap()
                .title_source
                .as_deref(),
            Some("generated")
        );
        // Already generated → never re-titled.
        assert!(!store
            .set_chat_session_title_if_placeholder("untitled", "Generated two")
            .unwrap());
        assert_eq!(
            store
                .get_chat_session("untitled")
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Generated one")
        );
        // ...but an explicit Rename still overrides it.
        store
            .set_chat_session_title("untitled", "My name", "user")
            .unwrap();
        let renamed = store.get_chat_session("untitled").unwrap().unwrap();
        assert_eq!(renamed.title.as_deref(), Some("My name"));
        assert_eq!(renamed.title_source.as_deref(), Some("user"));

        // The first-line placeholder → replaced.
        store
            .set_chat_session_title("fallback", "Hey can you look at…", "fallback")
            .unwrap();
        assert!(store
            .set_chat_session_title_if_placeholder("fallback", "Review the parser")
            .unwrap());
        assert_eq!(
            store
                .get_chat_session("fallback")
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Review the parser")
        );

        // A user Rename → never clobbered, whichever order the race resolves in.
        store
            .set_chat_session_title("renamed", "Mine", "user")
            .unwrap();
        assert!(!store
            .set_chat_session_title_if_placeholder("renamed", "Generated")
            .unwrap());
        assert_eq!(
            store
                .get_chat_session("renamed")
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Mine")
        );

        // Legacy row: a title with no recorded source is "unknown, don't touch".
        store
            .conn
            .execute(
                "UPDATE chat_sessions SET title = 'Old title' WHERE id = 'legacy'",
                [],
            )
            .unwrap();
        assert!(!store
            .set_chat_session_title_if_placeholder("legacy", "Generated")
            .unwrap());
        assert_eq!(
            store
                .get_chat_session("legacy")
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Old title")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn run_fixture(id: &str, status: &str, chat_session_id: Option<&str>) -> StoredRun {
        StoredRun {
            id: id.into(),
            experiment_id: "exp_1".into(),
            project_id: "proj_1".into(),
            status: status.into(),
            backend_json: "{}".into(),
            command: "echo hi".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            chat_session_id: chat_session_id.map(str::to_string),
        }
    }

    #[test]
    fn run_chat_session_id_roundtrips() {
        let dir = std::env::temp_dir().join(format!("orx-store-runsess-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        store
            .upsert_run(&run_fixture("run_owned", "starting", Some("chat_A")))
            .unwrap();
        store
            .upsert_run(&run_fixture("run_orphan", "starting", None))
            .unwrap();

        assert_eq!(
            store.get_run("run_owned").unwrap().unwrap().chat_session_id,
            Some("chat_A".to_string())
        );
        assert_eq!(
            store
                .get_run("run_orphan")
                .unwrap()
                .unwrap()
                .chat_session_id,
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_wakeups_are_idempotent_ready_only_at_success_or_failure_and_pruned() {
        let dir = std::env::temp_dir().join(format!("orx-store-wake-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_A"))
            .unwrap();
        for (id, status) in [
            ("run_active", "running"),
            ("run_done", "done"),
            ("run_failed", "failed"),
            ("run_cancelled", "cancelled"),
        ] {
            store
                .upsert_run(&run_fixture(id, status, Some("chat_A")))
                .unwrap();
            assert_eq!(
                store.register_run_wakeup(id, "chat_A").unwrap(),
                RunWakeupRegistration::Scheduled
            );
            assert_eq!(
                store.register_run_wakeup(id, "chat_A").unwrap(),
                RunWakeupRegistration::AlreadyPending
            );
        }
        store.register_run_wakeup("missing_run", "chat_A").unwrap();
        store
            .register_run_wakeup("run_done", "missing_chat")
            .unwrap();

        store.prune_run_wakeups().unwrap();
        let ready = store.list_ready_run_wakeups().unwrap();
        assert_eq!(
            ready
                .iter()
                .map(|wakeup| wakeup.run.id.as_str())
                .collect::<Vec<_>>(),
            ["run_done", "run_failed"]
        );

        store
            .update_status("run_active", "done", Some(2), Some(0))
            .unwrap();
        assert_eq!(store.list_ready_run_wakeups().unwrap().len(), 3);
        let token = store
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .unwrap();
        assert!(store
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .is_none());
        assert!(store
            .list_ready_run_wakeups()
            .unwrap()
            .iter()
            .any(|wakeup| wakeup.run.id == "run_done" && wakeup.state == "claimed"));
        store
            .release_run_wakeup("run_done", "chat_A", &token)
            .unwrap();
        assert!(store
            .list_ready_run_wakeups()
            .unwrap()
            .iter()
            .any(|wakeup| wakeup.run.id == "run_done"));
        let token = store
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .unwrap();
        assert!(store
            .mark_run_wakeup_delivered("run_done", "chat_A", &token)
            .unwrap());
        assert_eq!(
            store.register_run_wakeup("run_done", "chat_A").unwrap(),
            RunWakeupRegistration::AlreadyDelivered
        );
        assert!(store
            .list_ready_run_wakeups()
            .unwrap()
            .iter()
            .all(|wakeup| wakeup.run.id != "run_done"));

        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_wakeup_claim_is_atomic_across_store_connections_and_recoverable() {
        let dir = std::env::temp_dir().join(format!("orx-store-claim-{}", uuid::Uuid::new_v4()));
        let first = Store::open_at(dir.clone()).unwrap();
        first
            .create_chat_session(&chat_session_fixture("chat_A"))
            .unwrap();
        first
            .upsert_run(&run_fixture("run_done", "done", Some("chat_A")))
            .unwrap();
        first.register_run_wakeup("run_done", "chat_A").unwrap();
        let second = Store::open_at(dir.clone()).unwrap();

        let stale_token = first
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .unwrap();
        assert!(second
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .is_none());
        first
            .conn
            .execute(
                "UPDATE chat_run_wakeups SET claimed_at = 0 WHERE run_id = 'run_done'",
                [],
            )
            .unwrap();
        second.prune_run_wakeups().unwrap();
        let current_token = second
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .unwrap();
        first
            .release_run_wakeup("run_done", "chat_A", &stale_token)
            .unwrap();
        assert!(!first
            .mark_run_wakeup_delivered("run_done", "chat_A", &stale_token)
            .unwrap());
        assert!(second
            .renew_run_wakeup_claim("run_done", "chat_A", &current_token)
            .unwrap());
        second
            .release_run_wakeup("run_done", "chat_A", &current_token)
            .unwrap();
        assert!(first
            .claim_run_wakeup("run_done", "chat_A")
            .unwrap()
            .is_some());

        drop(second);
        drop(first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ready_run_wakeups_are_ordered_by_terminal_time() {
        let dir = std::env::temp_dir().join(format!("orx-store-order-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_A"))
            .unwrap();
        for (id, ended_at) in [("run_late", 20), ("run_early", 10)] {
            let mut run = run_fixture(id, "done", Some("chat_A"));
            run.ended_at = Some(ended_at);
            store.upsert_run(&run).unwrap();
            store.register_run_wakeup(id, "chat_A").unwrap();
        }

        assert_eq!(
            store
                .list_ready_run_wakeups()
                .unwrap()
                .iter()
                .map(|wakeup| wakeup.run.id.as_str())
                .collect::<Vec<_>>(),
            ["run_early", "run_late"]
        );

        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn chat_turn_lease_is_cross_process_and_token_scoped() {
        let dir = std::env::temp_dir().join(format!("orx-store-turn-{}", uuid::Uuid::new_v4()));
        let first = Store::open_at(dir.clone()).unwrap();
        first
            .create_chat_session(&chat_session_fixture("chat_A"))
            .unwrap();
        let second = Store::open_at(dir.clone()).unwrap();

        assert!(first.claim_chat_turn("chat_A", "token_A").unwrap());
        assert!(!second.claim_chat_turn("chat_A", "token_B").unwrap());
        assert!(!second.renew_chat_turn("chat_A", "token_B").unwrap());
        assert!(!second.claim_data_dir_move("move_A").unwrap());
        second.release_chat_turn("chat_A", "token_B").unwrap();
        assert!(!second.claim_chat_turn("chat_A", "token_B").unwrap());
        first.release_chat_turn("chat_A", "token_A").unwrap();
        let move_lock = first.acquire_data_dir_move_lock().unwrap();
        assert!(first.claim_data_dir_move("move_A").unwrap());
        assert!(!second.claim_chat_turn("chat_A", "token_B").unwrap());
        second.release_data_dir_move("move_B").unwrap();
        assert!(!second.claim_chat_turn("chat_A", "token_B").unwrap());
        drop(move_lock);
        assert!(second.claim_chat_turn("chat_A", "token_B").unwrap());

        drop(second);
        drop(first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn active_run_listing_excludes_terminal_history() {
        let dir = std::env::temp_dir().join(format!("orx-store-active-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&run_fixture("run_starting", "starting", None))
            .unwrap();
        store
            .upsert_run(&run_fixture("run_running", "running", None))
            .unwrap();
        store
            .upsert_run(&run_fixture("run_done", "done", None))
            .unwrap();

        let mut ids: Vec<_> = store
            .list_active_runs()
            .unwrap()
            .into_iter()
            .map(|run| run.id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["run_running", "run_starting"]);

        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn run_ownership_is_immutable_across_upserts() {
        let dir = std::env::temp_dir().join(format!("orx-store-runimmut-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        // Created by chat_A.
        store
            .upsert_run(&run_fixture("run_1", "starting", Some("chat_A")))
            .unwrap();
        // A later status upsert that carries a *different* (or absent) session
        // must NOT rewrite the owner — ownership is immutable.
        store
            .upsert_run(&run_fixture("run_1", "failed", Some("chat_B")))
            .unwrap();
        store
            .upsert_run(&run_fixture("run_1", "done", None))
            .unwrap();

        let run = store.get_run("run_1").unwrap().unwrap();
        assert_eq!(run.status, "done", "status still updates on conflict");
        assert_eq!(
            run.chat_session_id,
            Some("chat_A".to_string()),
            "the launching session is never overwritten by a later upsert"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn experiment_fixture(id: &str, chat_session_id: Option<&str>) -> LocalExperiment {
        LocalExperiment {
            id: id.into(),
            project_id: "proj_1".into(),
            parent_experiment_id: None,
            slug: format!("exp-{id}"),
            branch_name: format!("orx/exp-{id}"),
            title: None,
            description: None,
            run_command: "echo hi".into(),
            agent_status: "idle".into(),
            created_at: 1,
            updated_at: 1,
            chat_session_id: chat_session_id.map(str::to_string),
        }
    }

    #[test]
    fn project_activity_summaries_aggregate_lifetime_work() {
        let dir = std::env::temp_dir().join(format!(
            "orx-store-project-activity-{}",
            uuid::Uuid::new_v4()
        ));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_local_project(&LocalProject {
                id: "proj_1".into(),
                name: "Project".into(),
                slug: "project".into(),
                github_owner: String::new(),
                github_repo: String::new(),
                github_sync_enabled: false,
                baseline_branch: "main".into(),
                repo_path: dir.join("project").to_string_lossy().into_owned(),
                run_command: None,
                paper_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();

        let mut archived = chat_session_fixture("archived");
        archived.archived = true;
        store.create_chat_session(&archived).unwrap();
        store
            .create_chat_session(&chat_session_fixture("current"))
            .unwrap();
        for (id, role, created_at) in [
            ("user", "user", 10),
            ("assistant", "assistant", 20),
            ("system", "system", 30),
        ] {
            store
                .upsert_chat_message(&StoredChatMessage {
                    id: id.into(),
                    session_id: "current".into(),
                    role: role.into(),
                    parts_json: "[]".into(),
                    created_at,
                    parent_id: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                })
                .unwrap();
        }

        store
            .create_local_experiment(&experiment_fixture("exp_1", None))
            .unwrap();
        store
            .create_local_experiment(&experiment_fixture("exp_2", None))
            .unwrap();
        for (id, experiment_id, status) in [
            ("run_1", "exp_1", "starting"),
            ("run_2", "exp_2", "running"),
            ("run_done", "exp_1", "done"),
            ("run_orphan", "missing", "running"),
        ] {
            let mut run = run_fixture(id, status, None);
            run.experiment_id = experiment_id.into();
            store.upsert_run(&run).unwrap();
        }

        let summaries = store.list_project_activity_summaries().unwrap();
        assert_eq!(summaries.len(), 1);
        let summary = &summaries[0];
        assert_eq!(summary.project_id, "proj_1");
        assert_eq!(summary.total_agents, 2);
        assert_eq!(summary.running_experiments, 2);
        assert_eq!(summary.total_experiments, 2);
        assert_eq!(summary.last_message_at, Some(20));
        assert_eq!(store.list_chat_session_project_ids().unwrap().len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn experiment_chat_session_id_roundtrips() {
        let dir = std::env::temp_dir().join(format!("orx-store-expsess-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        store
            .create_local_experiment(&experiment_fixture("exp_owned", Some("chat_x")))
            .unwrap();
        store
            .create_local_experiment(&experiment_fixture("exp_orphan", None))
            .unwrap();

        assert_eq!(
            store
                .get_local_experiment("exp_owned")
                .unwrap()
                .unwrap()
                .chat_session_id,
            Some("chat_x".to_string())
        );
        assert_eq!(
            store
                .get_local_experiment("exp_orphan")
                .unwrap()
                .unwrap()
                .chat_session_id,
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn experiment_ownership_is_immutable_across_updates() {
        let dir = std::env::temp_dir().join(format!("orx-store-expimm-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        store
            .create_local_experiment(&experiment_fixture("exp_owned", Some("chat_x")))
            .unwrap();

        // A later full-row update must not rewrite the owning session.
        let mut updated = experiment_fixture("exp_owned", None);
        updated.title = Some("renamed".into());
        store.update_local_experiment(&updated).unwrap();

        let stored = store.get_local_experiment("exp_owned").unwrap().unwrap();
        assert_eq!(stored.title.as_deref(), Some("renamed"), "title updates");
        assert_eq!(
            stored.chat_session_id,
            Some("chat_x".to_string()),
            "the creating session is never overwritten by a later update"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
