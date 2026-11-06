//! Local run store — orx's own truth for externally-executed runs.
//!
//! Mirrors the opencode model: state lives in a SQLite db beside the work
//! (`orx.db` under the data dir), and `orx serve` exposes it over loopback
//! HTTP/SSE. Run logs are plain append-only files under `run-logs/<runId>.log`
//! so tailing (serve) and appending (supervise) never contend on the db.
//!
//! Data dir: `$ORX_DATA_DIR`, else `$XDG_DATA_HOME/openresearch`, else
//! `~/.local/share/openresearch`.

mod acp;
pub(crate) mod search;
mod telemetry;
pub(crate) use telemetry::{InvocationIdentity, TokenUsage};

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};
use crate::local::autonomy::Autonomy;
use crate::local::model::{LocalExperiment, LocalProject};
use crate::workspace_state::{GlobalWorkspaceState, WorkspaceState};

/// An experiment a chat created, launched a run of, or asked to be woken for.
/// Listed live first, then unarchived before archived, each newest activity first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionExperiment {
    pub experiment: LocalExperiment,
    pub project_name: String,
    /// Its newest live run, else its newest run; `None` until it has been run.
    /// A forced relaunch can leave an older run live, so the newest run alone
    /// could hide work still in flight.
    pub current_run: Option<StoredRun>,
}

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
    if let Some(dir) = data_dir_override() {
        return dir;
    }
    if let Some(dir) = crate::config::settings_data_dir() {
        return dir;
    }
    xdg_default_data_dir()
}

pub(crate) fn open_lifecycle_lock() -> Result<fd_lock::RwLock<std::fs::File>> {
    open_lifecycle_lock_in(&crate::config::config_dir())
}

pub(crate) fn open_lifecycle_lock_in(config_dir: &Path) -> Result<fd_lock::RwLock<std::fs::File>> {
    // The config dir stays put while the user can move the live data directory.
    let path = lifecycle_lock_path_in(config_dir);
    let legacy = config_dir.join("orx.lifecycle.lock");
    if !path.exists() {
        drop(open_lifecycle_lock_at(&legacy)?);
        // Keep the legacy lock if this filesystem cannot share its inode.
        if std::fs::create_dir_all(path.parent().unwrap()).is_err() {
            return open_lifecycle_lock_at(&legacy);
        }
        match std::fs::hard_link(&legacy, &path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return open_lifecycle_lock_at(&legacy),
        }
    }
    if !same_file::is_same_file(&legacy, &path)? {
        return Err(anyhow!(
            "Lifecycle lock paths differ; refusing to run without a shared lock"
        ));
    }
    open_lifecycle_lock_at(&path)
}

pub(crate) fn lifecycle_lock_path_in(config_dir: &Path) -> PathBuf {
    config_dir.join("locks/orx.lifecycle.lock")
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

fn acquire_data_dir_lock(path: PathBuf, exclusive: bool) -> Result<DataDirMoveLock> {
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let result = (|| -> Result<()> {
            let mut lock = open_lifecycle_lock_at(&path)?;
            let wait = || -> Result<()> {
                ready_tx
                    .send(Ok(()))
                    .map_err(|_| anyhow!("data-directory move lock receiver closed"))?;
                let _ = release_rx.recv();
                Ok(())
            };
            if exclusive {
                let _guard = lock.try_write()?;
                wait()
            } else {
                let _guard = lock.try_read()?;
                wait()
            }
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

pub(crate) fn acquire_data_dir_read_lock() -> Result<DataDirMoveLock> {
    acquire_data_dir_lock(data_dir_move_lock_path(), false)
}

/// Read an env var as a path, treating unset **and empty** the same (an empty
/// `export ORX_DATA_DIR=` is a shell footgun that must not resolve to `""`).
fn env_path(key: &str) -> Option<PathBuf> {
    crate::local::shell_env::var(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `$ORX_DATA_DIR`. Under test only a fixture in the temp dir counts: one
/// inherited from the developer's shell points at their real data.
fn data_dir_override() -> Option<PathBuf> {
    let dir = env_path("ORX_DATA_DIR")?;
    #[cfg(test)]
    if !dir.starts_with(std::env::temp_dir()) {
        return None;
    }
    Some(dir)
}

/// `$XDG_DATA_HOME/openresearch` else `~/.local/share/openresearch` — the tail
/// of the resolution chain, shared by `data_dir()` and `default_data_dir()`.
fn xdg_default_data_dir() -> PathBuf {
    #[cfg(test)]
    let base = crate::config::test_home().join(".local").join("share");
    #[cfg(not(test))]
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
    if let Some(dir) = data_dir_override() {
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
    if data_dir_override().is_some() {
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

pub fn log_path(data_dir: &Path, run_id: &str) -> PathBuf {
    // Sanitize the id so malformed local data cannot escape the log directory.
    let safe: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    data_dir.join("run-logs").join(format!("{safe}.log"))
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
    #[serde(serialize_with = "serialize_public_backend")]
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
    pub cancel_reason: Option<String>,
    /// The `orx up` chat session that launched this run, when it was started by
    /// an agent harness child (which exports `ORX_CHAT_SESSION_ID`). `None` for
    /// CLI-launched runs. This records attribution; wake-ups are
    /// separately and explicitly registered in `chat_run_wakeups`.
    pub chat_session_id: Option<String>,
}

pub const TERMINAL: &str = "terminal";
pub const ATTENTION: &str = "attention";
pub const RECOVERED: &str = "recovered";
pub const NOTIFY: &str = "notify";
const TERMINAL_LOG_BYTES: u64 = 4 * 1024;
const TERMINAL_LOG_LINES: usize = 40;

pub struct SubmittingRun<'a> {
    pub id: &'a str,
    pub experiment_id: &'a str,
    pub project_id: &'a str,
    pub metadata: &'a crate::jobs::BackendDescriptor,
    pub command: &'a str,
    pub created_at: i64,
    pub commit_sha: Option<&'a str>,
    pub chat_session_id: Option<&'a str>,
    pub spec_digest: &'a str,
    pub launch: &'a LaunchSpec,
}

/// Where and how a run launches, durable from its first transaction so recovery after a
/// restart can find the job.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LaunchSpec {
    /// Backend kind, e.g. `ssh_job`.
    pub kind: String,
    /// Host alias, cluster, namespace or account, as the backend names it.
    pub target: Option<String>,
    pub options: serde_json::Value,
}

/// A run row's `(status, updated_at)` as last seen.
pub type SeenRow = (String, i64);

/// Notify-file bytes already turned into notices, with the file they came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NotifyCursor {
    pub ident: orx_host_proto::FileIdent,
    pub offset: u64,
    /// Bumped whenever the file is replaced or truncated, so offsets never collide across files.
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Contact {
    Live,
    Unreachable { since: i64, cause: String },
    NeedsAuth { since: i64, cause: String },
    Unreconciled { since: i64, detail: String },
    Stopping { since: i64, count: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubmitterIdentity {
    pub pid: u32,
    pub starttime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SubmissionFence {
    pub taken_at: i64,
    pub submitter: Option<SubmitterIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineRunState {
    pub started_at: Option<i64>,
    pub contact: Contact,
    pub cancel_unconfirmed: Option<Vec<u32>>,
    pub spec_digest: Option<String>,
    pub submission_attempt: u32,
    pub submission_fence: Option<SubmissionFence>,
    pub cancel_accepted_at: Option<i64>,
    /// Present on every runner-2 row.
    pub launch: Option<LaunchSpec>,
    pub notify_cursor: Option<NotifyCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunNotice {
    pub id: i64,
    pub run_id: String,
    pub chat_session_id: String,
    pub kind: String,
    pub dedupe: String,
    pub body: String,
    pub created_at: i64,
    pub client_turn_id: String,
}

/// The descriptor a run shows, without secrets. An engine run reads as the 0.2.x descriptor of
/// what it launched: its OpenResearch box's own, else the fields its launch intent decided.
pub fn public_backend(value: &str, launch: Option<&LaunchSpec>) -> Option<serde_json::Value> {
    let mut backend = serde_json::from_str::<serde_json::Value>(value).ok()?;
    let Some(fields) = backend.as_object_mut() else {
        return Some(backend);
    };
    if fields.get("kind").and_then(serde_json::Value::as_str) == Some("engine") {
        let descriptor = fields
            .get("handle")
            .and_then(|handle| handle.get("descriptor"))
            .and_then(serde_json::Value::as_object)
            .cloned();
        if let Some(descriptor) = descriptor {
            for (field, value) in descriptor {
                if field == "kind" || !fields.contains_key(&field) {
                    fields.insert(field, value);
                }
            }
        } else if let Some(launch) = launch {
            let options = &launch.options;
            let directives = &options["directives"];
            let cpus = directives["cpus-per-task"]
                .as_str()
                .and_then(|cpus| cpus.parse::<u64>().ok());
            let derived = [
                ("kind", serde_json::json!(launch.kind)),
                ("namespace", serde_json::json!(launch.target)),
                ("flavor", options["flavor"].clone()),
                ("sshContainer", options["container"].clone()),
                ("context", options["context"].clone()),
                ("timeoutSecs", options["timeout_seconds"].clone()),
                ("cpusPerTask", serde_json::json!(cpus)),
                ("mem", directives["mem"].clone()),
            ];
            for (field, value) in derived {
                if !value.is_null() {
                    fields.insert(field.into(), value);
                }
            }
        }
    }
    fields.remove("rentalLease");
    if let Some(descriptor) = fields
        .get_mut("handle")
        .and_then(|handle| handle.get_mut("descriptor"))
        .and_then(serde_json::Value::as_object_mut)
    {
        descriptor.remove("rentalLease");
    }
    Some(backend)
}

fn serialize_public_backend<S: serde::Serializer>(
    value: &str,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(
        &public_backend(value, None)
            .map(|value| value.to_string())
            .unwrap_or_default(),
    )
}

/// Run lifecycle: active states can finish once; terminal outcomes never change.
/// Both status updates and submission upserts enforce these transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Submitted to the backend; not yet observed running.
    Starting,
    /// The backend has confirmed the job is under way.
    Running,
    /// Terminal: the job ran to completion with a successful exit.
    Done,
    /// Terminal: the job errored, or was never observed as launched.
    Failed,
    /// Terminal: cancellation was requested and honored.
    Cancelled,
    /// Terminal: the rental ended before its execution outcome could be retrieved.
    Lost,
}

impl RunStatus {
    #[cfg(test)]
    const ALL: [RunStatus; 6] = [
        Self::Starting,
        Self::Running,
        Self::Done,
        Self::Failed,
        Self::Cancelled,
        Self::Lost,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Lost => "lost",
        }
    }

    /// Parses the stored/wire vocabulary. `None` for anything else — callers
    /// that only need "is this finished" should prefer [`is_terminal_status`],
    /// which treats an unrecognized string the same as a non-terminal one.
    pub fn parse(status: &str) -> Option<Self> {
        Some(match status {
            "starting" => Self::Starting,
            "running" => Self::Running,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "lost" => Self::Lost,
            _ => return None,
        })
    }

    /// A run in a terminal state is finished and will never change again.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Lost
        )
    }

    fn can_transition_to(self, to: RunStatus) -> bool {
        use RunStatus::*;
        match self {
            Starting => true,
            Running => matches!(to, Running | Done | Failed | Cancelled | Lost),
            Done | Failed | Cancelled | Lost => false,
        }
    }
}

impl std::fmt::Display for RunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a stored run status string denotes a finished run. An unrecognized
/// string is treated as non-terminal (the conservative choice: it keeps a run
/// visible as active rather than silently dropping it from "in flight" views).
pub fn is_terminal_status(status: &str) -> bool {
    RunStatus::parse(status).is_some_and(RunStatus::is_terminal)
}

#[derive(Debug, Clone)]
pub struct ProjectActivitySummary {
    pub project_id: String,
    pub total_agents: usize,
    pub running_experiments: usize,
    pub total_experiments: usize,
    pub last_message_at: Option<i64>,
    pub last_activity_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegation {
    pub depth: u32,
    /// The chat that delegated to this one; `None` for a chat the user started.
    pub parent: Option<String>,
    pub may_delegate: bool,
}

/// One chat's standing request to hear from another. A paused one, which the
/// user stopped, tells nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatSubscription {
    pub subscriber_session_id: String,
    pub target_session_id: String,
    pub paused: bool,
    pub told_prompt_id: Option<String>,
    /// The target is working, or idle after a turn the subscriber was not told.
    pub outstanding: bool,
}

/// What a subscription tells: a finished turn, or a question its target asked.
#[derive(Debug, Clone, Copy)]
pub enum ToldEvent<'a> {
    Turn(&'a str),
    Prompt(&'a str),
}

/// A chat whose latest turn has ended since its subscriber was last told.
#[derive(Debug, Clone)]
pub struct ReadyChatSubscription {
    pub subscriber_session_id: String,
    pub target_session_id: String,
    pub turn_id: String,
    pub turn_state: String,
    pub assistant_message_id: String,
    subscribed_at: i64,
    told_turn_id: Option<String>,
}

const CHAT_SUBSCRIPTION_CLAIM_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Each subscription with its target's latest turn and whether the target is
/// idle after it: done, and with nothing queued unless a stop held the queue.
const LATEST_SUBSCRIBED_TURNS: &str = "subscribed AS (
    SELECT s.subscriber_session_id, s.target_session_id, s.paused, s.told_turn_id,
           s.told_prompt_id, s.claim_token, s.claimed_at, s.created_at,
           t.id AS turn_id, t.state,
           t.assistant_message_id, t.updated_at,
           t.state = 'interrupted'
               OR (t.state IN ('completed', 'failed') AND NOT EXISTS (
                   SELECT 1 FROM chat_queued_messages q
                   WHERE q.session_id = s.target_session_id
               )) AS idle
    FROM chat_subscriptions s
    LEFT JOIN chat_turns t ON t.id = (
        SELECT id FROM chat_turns WHERE session_id = s.target_session_id
        ORDER BY created_at DESC, rowid DESC LIMIT 1
    )
),
latest AS (
    SELECT *, NOT (idle IS 1 AND told_turn_id IS turn_id) AS pending FROM subscribed
)";

pub(crate) const CHAT_TURN_LEASE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// Heartbeats and claims older than this belong to an owner that is gone.
fn expired_before(ttl: std::time::Duration) -> i64 {
    now_ms() - crate::config::paced(ttl).as_millis() as i64
}

fn migrate_chat_spawns(conn: &Connection) -> Result<()> {
    let tx = rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)?;
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'chat_spawns')",
        [],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(());
    }
    let pending = {
        let mut statement = tx.prepare(
            "SELECT sp.session_id, sp.prompt, sp.requested_at FROM chat_spawns sp
             JOIN chat_sessions s ON s.id = sp.session_id
             WHERE sp.state IN ('pending', 'starting')
               AND NOT EXISTS (SELECT 1 FROM chat_turns t WHERE t.session_id = sp.session_id)
             ORDER BY sp.requested_at, sp.session_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (session_id, prompt, requested_at) in pending {
        let leased: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_turn_leases WHERE chat_session_id = ?1 AND heartbeat_at > ?2)",
            params![session_id, expired_before(CHAT_TURN_LEASE_TTL)], |row| row.get(0),
        )?;
        if leased {
            return Err(anyhow!(
                "Stop the older OpenResearch process before migrating its pending agent chats."
            ));
        }
        let parts = {
            let mut statement = tx.prepare("SELECT role, parts_json FROM chat_messages WHERE session_id = ?1 ORDER BY created_at, rowid")?;
            let rows = statement.query_map(params![session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let brief_recorded = match parts.as_slice() {
            [] => false,
            [(role, parts)] if role == "user" => {
                let parts: Vec<crate::local::chat::WirePart> = serde_json::from_str(parts)?;
                let text = parts
                    .iter()
                    .filter(|part| part.kind == "text")
                    .filter_map(|part| part.text.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n\n");
                if text.trim() != prompt.trim() {
                    continue;
                }
                true
            }
            _ => continue,
        };
        let message = crate::local::chat::legacy_opening_message(
            &session_id,
            prompt,
            requested_at,
            brief_recorded,
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO chat_queued_messages (id, session_id, client_turn_id, request_hash, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![message.id, message.session_id, message.client_turn_id, message.request_hash, message.payload_json, message.created_at],
        )?;
    }
    tx.execute_batch("DROP TABLE chat_spawns")?;
    tx.commit()?;
    Ok(())
}

/// Default SQLite busy_timeout for every store connection (5s). Long enough
/// to ride out ordinary writer bursts, short enough that a dead writer doesn't
/// stall a command for long.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);
/// How long submission and project deletion wait out each other or an orphan scan before failing as busy.
#[cfg(not(test))]
pub(crate) const RUN_ARTIFACTS_WAIT: Duration = Duration::from_secs(60);
#[cfg(test)]
pub(crate) const RUN_ARTIFACTS_WAIT: Duration = Duration::from_secs(1);
pub(crate) const RUN_ARTIFACTS_POLL: Duration = Duration::from_millis(250);
/// Bounds removal time per orphan pass; classification is unbudgeted but cheap for finished runs.
const ORPHAN_PASS_BUDGET: Duration = Duration::from_secs(5);
/// Finished runs older than each project's newest few lose their rebuildable environments
/// (local runs) or their remote dirs (SSH and Slurm runs).
const FULL_RUNS_PER_PROJECT: i64 = 3;
/// Uploads are written before the message naming them, so the sweep spares recent files.
const CHAT_ATTACHMENT_SETTLE: Duration = Duration::from_secs(24 * 60 * 60);
/// A harness writes its session before orx records the id, so the sweep spares recent ones.
const NATIVE_SESSION_SETTLE: Duration = Duration::from_secs(24 * 60 * 60);
const SLIMMED_RUN_PATHS: [&[&str]; 2] = [&["repo", ".venv"], &["repo", ".cache", "uv"]];

const SUBMISSION_HANDLE: (&str, &str) = ("submission-handles", ".json");
const SUPERVISOR_LOCK: (&str, &str) = ("run-logs", ".supervisor.lock");
const CANCEL_LOCK: (&str, &str) = ("run-logs", ".cancel.lock");
const SHARED_SSH_LOCK: (&str, &str) = ("run-logs", ".supervisor.shared-ssh.lock");
/// The kept end of an engine run's log too large to keep whole.
const TAIL_LOG: (&str, &str) = ("run-logs", ".tail.log");
/// Per-run files as (data-dir root, name suffix); the supervisor lock goes last so it excludes until the end.
const RUN_ARTIFACTS: [(&str, &str); 18] = [
    ("local-runs", ""),
    ("run-logs", ".log"),
    ("run-logs", ".spool.log"),
    ("run-logs", ".log.part"),
    ("run-logs", ".log.part.json"),
    TAIL_LOG,
    ("run-logs", ".tail.log.part"),
    ("run-logs", ".tail.log.part.json"),
    ("run-logs", ".ranges.json"),
    ("run-logs", ".cursor.json"),
    ("run-logs", ".pending.json"),
    ("run-logs", ".cursor.lock"),
    SHARED_SSH_LOCK,
    ("run-logs", ".supervisor.log"),
    ("run-logs", ".supervisor.log.1"),
    CANCEL_LOCK,
    SUBMISSION_HANDLE,
    SUPERVISOR_LOCK,
];

#[derive(Default)]
pub(crate) struct OrphanPass {
    /// The pass stopped at [`ORPHAN_PASS_BUDGET`] with artifacts left to reclaim.
    pub more: bool,
    /// Runs whose artifacts were kept because they could not be classified or removed.
    pub kept: Vec<(String, crate::error::Error)>,
}

enum OrphanFate {
    Reclaim,
    Keep { snapshot: Option<String> },
    KeepWithAllSnapshots,
}

/// Remote dirs waiting for removal; `deleted` entries ignore a target's keep setting.
pub(crate) struct RemoteCleanup {
    pub run_id: String,
    pub backend_json: String,
    pub deleted: bool,
}

/// Runs whose job ran in `<data dir>/local-runs/<id>`, from 0.2.x or the engine.
const LOCAL_RUNS: &str = "CASE WHEN json_valid(backend_json) THEN
    json_extract(backend_json, '$.kind') IN ('local_job', 'tinker_job')
    OR json_extract(backend_json, '$.handle.type') IN ('Local', 'Tinker') END";
/// Runs that left a run dir on an SSH or Slurm host, from 0.2.x or the engine.
const REMOTE_RUNS: &str = "CASE WHEN json_valid(backend_json) THEN
    json_extract(backend_json, '$.kind') IN ('ssh_job', 'slurm_job')
    OR json_extract(backend_json, '$.handle.type') IN ('Ssh', 'Slurm') END";

/// `(id, status)` of runs matching `runs` past each project's newest [`FULL_RUNS_PER_PROJECT`].
fn runs_past_newest(runs: &'static str) -> String {
    format!(
        "SELECT id, status FROM (
           SELECT id, status, ROW_NUMBER() OVER
             (PARTITION BY project_id ORDER BY created_at DESC, id DESC) AS position
           FROM runs
           WHERE {runs})
         WHERE position > {FULL_RUNS_PER_PROJECT}"
    )
}

fn remote_cleanup_eligibility() -> String {
    format!("NOT EXISTS (SELECT 1 FROM runs WHERE id = run_id AND
          (contact IS NOT NULL OR status IN ('submitting','starting','running')))
        AND NOT EXISTS (SELECT 1 FROM log_collections WHERE run_id = remote_cleanups.run_id AND done = 0)
        AND (deleted = 1 OR run_id IN (SELECT id FROM ({})))", runs_past_newest(REMOTE_RUNS))
}

/// Queues the SSH/Slurm runs whose `column` equals `value`; deletion wins over a pending finish.
/// An engine run's entry carries its container from the launch intent, which a deletion removes.
fn queue_remote_cleanups(
    conn: &Connection,
    column: &'static str,
    value: &str,
    deleted: bool,
) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO remote_cleanups (run_id, backend_json, deleted, next_attempt_at)
             SELECT id, CASE WHEN json_extract(launch_spec, '$.options.container') IS NULL
                 THEN backend_json ELSE json_set(backend_json, '$.sshContainer',
                   json(json_extract(launch_spec, '$.options.container'))) END, ?2, ?3
             FROM runs WHERE {column} = ?1 AND {REMOTE_RUNS}
             ON CONFLICT(run_id) DO UPDATE SET backend_json = excluded.backend_json,
               deleted = MAX(deleted, excluded.deleted), attempts = 0,
               next_attempt_at = excluded.next_attempt_at"
        ),
        params![value, deleted, now_ms()],
    )?;
    Ok(())
}

fn validate_deleted_runs(runs: &[StoredRun]) -> Result<()> {
    for run in runs {
        if run.id.is_empty()
            || !run
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(anyhow!("Invalid run id for artifact cleanup: {}", run.id));
        }
    }
    Ok(())
}

/// Log collection retries after a minute, doubling to an hour, then hourly.
const LOG_COLLECTION_RETRY_MS: i64 = 60 * 1000;
const LOG_COLLECTION_MAX_RETRY_MS: i64 = 60 * 60 * 1000;
/// Failures before retries reach the cap and the collection reads as failed.
const LOG_COLLECTION_FAILED_AFTER: u32 = 7;
/// Failed remote cleanups retry after 5 minutes, doubling up to a day.
const REMOTE_CLEANUP_RETRY_MS: i64 = 5 * 60 * 1000;
const REMOTE_CLEANUP_MAX_RETRY_MS: i64 = 24 * 60 * 60 * 1000;
/// Just past the source cache's idle window, so tars a pass had to skip are removable.
const SOURCE_SWEEP_DELAY_MS: i64 = (crate::jobs::ssh::cleanup::SOURCE_IDLE_MINUTES + 1) * 60 * 1000;

pub struct Store {
    conn: Connection,
    data_dir: PathBuf,
    data_dir_move_lock_path: PathBuf,
}

impl Store {
    pub(crate) fn relocate_project_paths(&self, paths: &[(String, String)]) -> Result<()> {
        let tx = self.begin()?;
        for (old, new) in paths {
            tx.execute(
                "UPDATE local_projects SET repo_path = ?2 WHERE repo_path = ?1",
                params![old, new],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Open (creating dirs/schema as needed). WAL so the supervise writers and
    /// the serve readers never block each other.
    pub fn open() -> Result<Self> {
        Self::open_at_with_move_lock(data_dir(), data_dir_move_lock_path(), BUSY_TIMEOUT)
    }

    /// Open like `open`, but cap SQLite's lock wait — `orx exp wait` shrinks
    /// this to its deadline so one blocked open can't overshoot it.
    pub fn open_with_busy_timeout(busy_timeout: Duration) -> Result<Self> {
        Self::open_at_with_move_lock(data_dir(), data_dir_move_lock_path(), busy_timeout)
    }

    /// Open a store rooted at an explicit directory, bypassing `data_dir()`
    /// resolution.
    pub fn open_at(dir: PathBuf) -> Result<Self> {
        let move_lock_path = dir.join(".orx-data-dir-move.lock");
        Self::open_at_with_move_lock(dir, move_lock_path, BUSY_TIMEOUT)
    }

    fn open_at_with_move_lock(
        dir: PathBuf,
        data_dir_move_lock_path: PathBuf,
        busy_timeout: Duration,
    ) -> Result<Self> {
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // SQLite and its sidecars contain scoped rental capabilities.
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::create_dir_all(dir.join("run-logs"))
            .map_err(|e| anyhow!("Could not create {}: {}", dir.display(), e))?;
        let conn = Connection::open(dir.join("orx.db"))?;
        let has_floor: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'store_metadata')",
            [],
            |row| row.get(0),
        )?;
        if has_floor {
            let floor: Option<i64> = conn
                .query_row(
                    "SELECT value FROM store_metadata WHERE key = 'min_reader_version'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(floor) = floor.filter(|floor| *floor > 1) {
                return Err(anyhow!("This database requires a newer OpenResearch reader (min_reader_version {}). Update orx before opening it.", floor));
            }
        }
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "busy_timeout", busy_timeout.as_millis() as u64)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS store_metadata (
                key TEXT PRIMARY KEY,
                value INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO store_metadata (key, value) VALUES ('min_reader_version', 1);
            CREATE TABLE IF NOT EXISTS runs (
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
            CREATE TABLE IF NOT EXISTS engine_host_auth (
                alias TEXT PRIMARY KEY,
                state_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS engine_hosts (
                alias TEXT NOT NULL,
                root TEXT NOT NULL,
                state_json TEXT NOT NULL,
                PRIMARY KEY(alias, root)
            );
            CREATE TABLE IF NOT EXISTS run_subscriptions (
                run_id TEXT NOT NULL,
                chat_session_id TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY(run_id, chat_session_id)
            );
            CREATE TABLE IF NOT EXISTS run_notices (
                id INTEGER PRIMARY KEY,
                run_id TEXT NOT NULL,
                chat_session_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                dedupe TEXT NOT NULL,
                body TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                client_turn_id TEXT,
                delivered_at INTEGER,
                delivery_failures INTEGER NOT NULL DEFAULT 0,
                first_failure_at INTEGER,
                next_delivery_at INTEGER,
                delivery_error TEXT,
                retired_at INTEGER,
                UNIQUE(run_id, chat_session_id, dedupe)
            );
            CREATE INDEX IF NOT EXISTS idx_run_notices_pending
                ON run_notices(chat_session_id, client_turn_id, id) WHERE delivered_at IS NULL;
            CREATE TABLE IF NOT EXISTS legacy_watches (
                run_id TEXT PRIMARY KEY,
                seen_status TEXT,
                seen_updated_at INTEGER
            );
            CREATE TABLE IF NOT EXISTS native_invocation_identities (
                harness TEXT NOT NULL,
                call_id TEXT NOT NULL,
                identity_json TEXT NOT NULL,
                PRIMARY KEY (harness, call_id)
            );
            CREATE TABLE IF NOT EXISTS run_telemetry (
                run_id TEXT PRIMARY KEY,
                identity_json TEXT,
                report_json TEXT
            );
            CREATE TABLE IF NOT EXISTS telemetry_pending_events (
                event_id TEXT PRIMARY KEY,
                payload_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS native_usage_totals (
                harness TEXT NOT NULL,
                native_scope TEXT NOT NULL,
                totals_json TEXT NOT NULL,
                PRIMARY KEY (harness, native_scope)
            );
            CREATE TABLE IF NOT EXISTS native_usage_baselines (
                execution_id TEXT NOT NULL,
                prefix TEXT NOT NULL,
                totals_json TEXT,
                PRIMARY KEY (execution_id, prefix)
            );
            CREATE TABLE IF NOT EXISTS chat_usage_executions (
                execution_id TEXT PRIMARY KEY,
                turn_id TEXT NOT NULL,
                harness TEXT NOT NULL,
                report_id TEXT NOT NULL,
                suppressed INTEGER NOT NULL,
                outcome TEXT
            );
            CREATE TABLE IF NOT EXISTS chat_usage_samples (
                execution_id TEXT NOT NULL,
                sample_id TEXT NOT NULL,
                harness TEXT NOT NULL,
                model TEXT,
                provider TEXT,
                usage_json TEXT NOT NULL,
                complete INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (execution_id, sample_id)
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
                archived             INTEGER NOT NULL DEFAULT 0,
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
            CREATE TABLE IF NOT EXISTS chat_steer_receipts (
                session_id TEXT NOT NULL,
                client_turn_id TEXT NOT NULL,
                request_hash TEXT NOT NULL,
                delivered INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (session_id, client_turn_id)
            );
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
                monitoring_alerted INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(run_id, chat_session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_run_wakeups_requested
                ON chat_run_wakeups(requested_at);
            CREATE TABLE IF NOT EXISTS chat_delegates (
                session_id        TEXT PRIMARY KEY,
                parent_session_id TEXT NOT NULL,
                may_delegate      INTEGER NOT NULL,
                created_at        INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_subscriptions (
                subscriber_session_id TEXT NOT NULL,
                target_session_id     TEXT NOT NULL,
                created_at            INTEGER NOT NULL,
                paused                INTEGER NOT NULL DEFAULT 0,
                told_turn_id          TEXT,
                told_prompt_id        TEXT,
                claim_token           TEXT,
                claimed_at            INTEGER,
                PRIMARY KEY (subscriber_session_id, target_session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_chat_subscriptions_target
                ON chat_subscriptions(target_session_id);
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
            CREATE TABLE IF NOT EXISTS remote_cleanups (
                run_id          TEXT PRIMARY KEY,
                backend_json    TEXT NOT NULL,
                deleted         INTEGER NOT NULL,
                attempts        INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL,
                last_error      TEXT
            );
            CREATE TABLE IF NOT EXISTS log_collections (
                run_id          TEXT PRIMARY KEY,
                collected       INTEGER NOT NULL DEFAULT 0,
                done            INTEGER NOT NULL DEFAULT 0,
                attempts        INTEGER NOT NULL DEFAULT 0,
                next_attempt_at INTEGER NOT NULL DEFAULT 0,
                last_error      TEXT
            );
            CREATE TABLE IF NOT EXISTS remote_source_sweeps (
                host   TEXT NOT NULL,
                root   TEXT NOT NULL,
                due_at INTEGER NOT NULL,
                PRIMARY KEY (host, root)
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
            CREATE TABLE IF NOT EXISTS acp_sessions (
                session_id TEXT PRIMARY KEY REFERENCES chat_sessions(id) ON DELETE CASCADE,
                launch_json TEXT NOT NULL,
                configuration_json TEXT NOT NULL DEFAULT '[]'
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
            "ALTER TABLE native_invocation_identities ADD COLUMN session_id TEXT",
            "ALTER TABLE native_invocation_identities ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_usage_samples ADD COLUMN complete INTEGER NOT NULL DEFAULT 0",
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
            "ALTER TABLE local_projects ADD COLUMN workspace_state_json TEXT",
            "ALTER TABLE local_experiments ADD COLUMN chat_session_id TEXT",
            "ALTER TABLE local_experiments ADD COLUMN archived INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE ssh_host_tests ADD COLUMN tools_found INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE ssh_host_tests ADD COLUMN missing_tools TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE chat_run_wakeups ADD COLUMN state TEXT NOT NULL DEFAULT 'pending'",
            "ALTER TABLE chat_run_wakeups ADD COLUMN claim_token TEXT",
            "ALTER TABLE chat_run_wakeups ADD COLUMN claimed_at INTEGER",
            "ALTER TABLE chat_run_wakeups ADD COLUMN delivered_at INTEGER",
            "ALTER TABLE chat_run_wakeups ADD COLUMN monitoring_alerted INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_run_wakeups ADD COLUMN engine_migrated INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_messages ADD COLUMN parent_id TEXT",
            "ALTER TABLE chat_messages ADD COLUMN base_native_session_id TEXT",
            "ALTER TABLE chat_messages ADD COLUMN result_native_session_id TEXT",
            "ALTER TABLE chat_messages ADD COLUMN native_harness TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN active_leaf_id TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN parent_session_id TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN goal TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN autonomy TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN side_parent_session_id TEXT",
            "ALTER TABLE chat_sessions ADD COLUMN side_parent_leaf_id TEXT",
            "ALTER TABLE ui_state ADD COLUMN preferred_service_tier TEXT",
            "ALTER TABLE ui_state ADD COLUMN workspace_state_json TEXT",
            "ALTER TABLE ui_state ADD COLUMN preferred_autonomy TEXT",
            "ALTER TABLE ui_state ADD COLUMN nested_delegation INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE chat_messages ADD COLUMN completed_at INTEGER",
            "ALTER TABLE runs ADD COLUMN cancel_reason TEXT",
            "ALTER TABLE runs ADD COLUMN cancel_unconfirmed TEXT",
            "ALTER TABLE runs ADD COLUMN contact TEXT",
            "ALTER TABLE runs ADD COLUMN spec_digest TEXT",
            "ALTER TABLE runs ADD COLUMN submission_attempt INTEGER NOT NULL DEFAULT 1",
            "ALTER TABLE runs ADD COLUMN submission_fence TEXT",
            "ALTER TABLE runs ADD COLUMN cancel_accepted_at INTEGER",
            "ALTER TABLE runs ADD COLUMN launch_spec TEXT",
            "ALTER TABLE runs ADD COLUMN notify_cursor TEXT",
            "ALTER TABLE runs ADD COLUMN started_at INTEGER",
        ] {
            let _ = conn.execute(ddl, []);
        }
        conn.execute_batch(
            "CREATE TRIGGER IF NOT EXISTS delete_run_outbox AFTER DELETE ON runs BEGIN
                 DELETE FROM run_notices WHERE run_id = OLD.id;
                 DELETE FROM run_subscriptions WHERE run_id = OLD.id;
             END;
             CREATE TRIGGER IF NOT EXISTS delete_chat_outbox AFTER DELETE ON chat_sessions BEGIN
                 DELETE FROM run_notices WHERE chat_session_id = OLD.id;
                 DELETE FROM run_subscriptions WHERE chat_session_id = OLD.id;
             END;",
        )?;
        // Covers the periodic native-id scan so it never reads the large parts_json.
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_chat_messages_native_ids
             ON chat_messages(base_native_session_id, result_native_session_id, session_id);",
        )?;
        conn.execute_batch("CREATE TRIGGER IF NOT EXISTS delete_log_collection AFTER DELETE ON runs BEGIN DELETE FROM log_collections WHERE run_id = OLD.id; END;")?;
        conn.execute_batch("CREATE TRIGGER IF NOT EXISTS delete_chat_steer_receipts AFTER DELETE ON chat_sessions BEGIN DELETE FROM chat_steer_receipts WHERE session_id = OLD.id; END;")?;
        conn.execute_batch("CREATE TRIGGER IF NOT EXISTS delete_chat_invocation_identities AFTER DELETE ON chat_sessions BEGIN DELETE FROM native_invocation_identities WHERE session_id = OLD.id; END;")?;
        conn.execute_batch("CREATE TRIGGER IF NOT EXISTS delete_acp_session AFTER DELETE ON chat_sessions BEGIN DELETE FROM acp_sessions WHERE session_id = OLD.id; END;")?;
        conn.execute_batch("CREATE TRIGGER IF NOT EXISTS delete_chat_subscriptions AFTER DELETE ON chat_sessions BEGIN DELETE FROM chat_subscriptions WHERE subscriber_session_id = OLD.id OR target_session_id = OLD.id; END;")?;
        // A delegate that outlives its delegator, in a worktree of its own, stands as the user's.
        conn.execute_batch("CREATE TRIGGER IF NOT EXISTS delete_chat_delegates AFTER DELETE ON chat_sessions BEGIN DELETE FROM chat_delegates WHERE session_id = OLD.id OR parent_session_id = OLD.id; END;")?;
        conn.execute(
            "DELETE FROM native_invocation_identities WHERE session_id IS NULL AND created_at < ?1",
            [now_ms() - 7 * 24 * 60 * 60 * 1000],
        )?;
        // Legacy tool failures cannot identify the missing dependency, so require one fresh check.
        conn.execute(
            "DELETE FROM ssh_host_tests
             WHERE reachable = 1 AND tools_found = 0 AND missing_tools = ''",
            [],
        )?;
        migrate_chat_spawns(&conn)?;
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
        search::initialize(&conn)?;
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
            data_dir: dir,
            data_dir_move_lock_path,
        })
    }

    /// Short write transaction over this connection; rolls back when dropped
    /// without `commit()`. Keep network I/O out of the closure it guards.
    pub fn begin(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(self.conn.unchecked_transaction()?)
    }

    /// `begin` that takes the write lock up front, so a check made inside it
    /// cannot be raced by another writer between the check and the insert.
    pub fn begin_immediate(&self) -> Result<rusqlite::Transaction<'_>> {
        Ok(rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?)
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
                    preferred_permission_mode, preferred_reasoning_level, workspace_state_json,
                    preferred_autonomy, nested_delegation
             FROM ui_state WHERE id = 1",
            [],
            |row| {
                let harness = row.get::<_, Option<String>>(2)?;
                let model = row.get::<_, Option<String>>(3)?;
                let service_tier = row.get::<_, Option<String>>(4)?;
                let permission_mode = row.get::<_, Option<String>>(5)?;
                let reasoning_level = row.get::<_, Option<String>>(6)?;
                let workspace_json = row.get::<_, Option<String>>(7)?;
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
                    workspace: workspace_json
                        .as_deref()
                        .and_then(GlobalWorkspaceState::from_stored),
                    preferred_autonomy: row
                        .get::<_, Option<String>>(8)?
                        .as_deref()
                        .and_then(Autonomy::from_id),
                    nested_delegation: row.get(9)?,
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

    pub fn set_global_workspace_state(&self, workspace: &GlobalWorkspaceState) -> Result<()> {
        workspace.validate()?;
        self.conn.execute(
            "UPDATE ui_state SET workspace_state_json = ?1 WHERE id = 1",
            params![serde_json::to_string(workspace)?],
        )?;
        Ok(())
    }

    pub fn project_workspace_state(&self, id: &str) -> Result<Option<WorkspaceState>> {
        let json: Option<String> = self.conn.query_row(
            "SELECT workspace_state_json FROM local_projects WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(json.as_deref().and_then(WorkspaceState::from_stored))
    }

    pub fn set_project_workspace_state(
        &self,
        id: &str,
        workspace: &WorkspaceState,
    ) -> Result<bool> {
        workspace.validate()?;
        Ok(self.conn.execute(
            "UPDATE local_projects SET workspace_state_json = ?2 WHERE id = ?1",
            params![id, serde_json::to_string(workspace)?],
        )? > 0)
    }

    pub fn set_tour_completed(&self, completed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE ui_state SET tour_completed = ?1 WHERE id = 1",
            params![completed],
        )?;
        Ok(())
    }

    pub fn set_nested_delegation(&self, allowed: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE ui_state SET nested_delegation = ?1 WHERE id = 1",
            params![allowed],
        )?;
        Ok(())
    }

    pub fn set_preferred_autonomy(&self, autonomy: Autonomy) -> Result<()> {
        self.conn.execute(
            "UPDATE ui_state SET preferred_autonomy = ?1 WHERE id = 1",
            params![autonomy.id()],
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
        let status = RunStatus::parse(&run.status)
            .ok_or_else(|| anyhow!("Unknown run status: {}", run.status))?;
        let tx = self
            .conn
            .is_autocommit()
            .then(|| self.begin())
            .transpose()?;
        self.conn.execute(
            "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command,
                               created_at, updated_at, ended_at, exit_code,
                               commit_sha, result_markdown, cancel_requested,
                               chat_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
               backend_json = excluded.backend_json,
               commit_sha = excluded.commit_sha",
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
        // Late submission handles must survive even when their status update is stale.
        if self.update_status(&run.id, status, run.ended_at, run.exit_code)? {
            self.conn.execute(
                "UPDATE runs SET result_markdown = ?2, updated_at = ?3 WHERE id = ?1",
                params![run.id, run.result_markdown, run.updated_at],
            )?;
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(())
    }

    /// Atomically apply a legal transition; false leaves the outcome untouched.
    pub fn update_status(
        &self,
        run_id: &str,
        status: RunStatus,
        ended_at: Option<i64>,
        exit_code: Option<i64>,
    ) -> Result<bool> {
        let tx = self
            .conn
            .is_autocommit()
            .then(|| self.begin())
            .transpose()?;
        let applied = self.conn.execute(
            "UPDATE runs SET status = ?2, updated_at = ?3, ended_at = COALESCE(?4, ended_at),
                             exit_code = COALESCE(?5, exit_code)
             WHERE id = ?1 AND ((status = 'starting' AND ?6) OR (status = 'running' AND ?7))",
            params![
                run_id,
                status.as_str(),
                now_ms(),
                ended_at,
                exit_code,
                RunStatus::Starting.can_transition_to(status),
                RunStatus::Running.can_transition_to(status),
            ],
        )?;
        if applied == 1 && status.is_terminal() {
            self.stage_run_terminal(run_id, status)?;
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(applied == 1)
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

    pub(crate) fn engine_runs_block_idle(&self) -> Result<bool> {
        let mut statement = self.conn.prepare("SELECT status FROM runs")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for status in rows {
            let status = status?;
            let phase = crate::engine::Phase::parse(&status)
                .ok_or_else(|| anyhow!("Unknown run status: {status}"))?;
            if !phase.is_terminal() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn engine_idle_blocked(&self) -> Result<bool> {
        if self.engine_runs_block_idle()? {
            return Ok(true);
        }
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM run_notices WHERE delivered_at IS NULL AND retired_at IS NULL)
                OR EXISTS(SELECT 1 FROM chat_turns WHERE state IN ('preparing','retrying','running'))",
            [], |row| row.get(0),
        )?)
    }

    /// Runs or local processes that prevent moving their working directory.
    pub fn count_active_runs(&self) -> Result<usize> {
        let mut statement = self
            .conn
            .prepare("SELECT id FROM runs WHERE status IN ('submitting', 'starting', 'running')")?;
        let active = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<std::collections::HashSet<_>, _>>()?;
        let mut count = active.len();
        let entries = match std::fs::read_dir(self.data_dir.join("local-runs")) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(count),
            Err(_) => return Ok(count + 1),
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => return Ok(count + 1),
            };
            if active.contains(entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            if crate::jobs::localbox::orphaned_job_is_running(&entry.path()).unwrap_or(true) {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Runs the engine may act on: in flight, or terminal with a kill still pending.
    pub fn list_engine_runs(&self) -> Result<Vec<StoredRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_RUN} WHERE status IN ('submitting', 'starting', 'running') OR contact IS NOT NULL"
        ))?;
        let rows = stmt.query_map([], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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

    /// The active runs each chat launched or subscribed to.
    pub fn chat_active_runs(&self) -> Result<Vec<(String, StoredRun)>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT * FROM (
                 SELECT chat_session_id AS chat, r.* FROM ({SELECT_RUN}) r
                 WHERE chat_session_id IS NOT NULL
                 UNION SELECT s.chat_session_id AS chat, r.* FROM run_subscriptions s
                 JOIN ({SELECT_RUN}) r ON r.id = s.run_id
             ) WHERE status IN ('submitting', 'starting', 'running')
             ORDER BY chat, id"
        ))?;
        let rows = stmt.query_map([], |row| {
            let chat: String = row.get(0)?;
            let run = row_to_run_at(row, 1)?;
            Ok((chat, run))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// A 0.2.x wake-up row, for migration fixtures.
    #[cfg(test)]
    pub fn register_run_wakeup(&self, run_id: &str, chat_session_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO chat_run_wakeups (run_id, chat_session_id, requested_at)
             VALUES (?1, ?2, ?3)",
            params![run_id, chat_session_id, now_ms()],
        )?;
        Ok(())
    }

    #[cfg(test)]
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

    /// Record `child` as a delegate of `parent`; recording it again, as a
    /// retried start does, changes nothing.
    pub fn record_chat_delegate(
        &self,
        child: &str,
        parent: &str,
        may_delegate: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO chat_delegates (session_id, parent_session_id, may_delegate, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![child, parent, may_delegate, now_ms()],
        )?;
        Ok(())
    }

    /// Forget who delegated to `id`; it stands as a chat the user started.
    pub fn remove_chat_delegate(&self, id: &str) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM chat_delegates WHERE session_id = ?1", [id])?
            == 1)
    }

    /// The chats `parent` delegated to, oldest first.
    pub fn chat_delegates_of(&self, parent: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM chat_delegates WHERE parent_session_id = ?1
             ORDER BY created_at, session_id",
        )?;
        let rows = stmt.query_map([parent], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// How many delegations deep `id` is, who delegated to it, and whether it
    /// may delegate. A chat the user started is at depth 0 and may.
    pub fn delegation(&self, id: &str) -> Result<Delegation> {
        Ok(self.conn.query_row(
            "WITH RECURSIVE chain(id, depth) AS (
                 SELECT ?1, 0
                 UNION ALL
                 SELECT d.parent_session_id, chain.depth + 1 FROM chain
                 JOIN chat_delegates d ON d.session_id = chain.id
             )
             SELECT (SELECT MAX(depth) FROM chain), d.parent_session_id,
                    COALESCE(d.may_delegate, 1)
             FROM (SELECT 1) LEFT JOIN chat_delegates d ON d.session_id = ?1",
            params![id],
            |row| {
                Ok(Delegation {
                    depth: row.get(0)?,
                    parent: row.get(1)?,
                    may_delegate: row.get(2)?,
                })
            },
        )?)
    }

    /// Subscribe `subscriber` to `target`; an existing subscription, paused
    /// or not, stays as it is. `told_turn_id` names a turn of `target` the
    /// subscriber already knows.
    pub fn subscribe_chat(
        &self,
        subscriber: &str,
        target: &str,
        told_turn_id: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO chat_subscriptions
             (subscriber_session_id, target_session_id, told_turn_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![subscriber, target, told_turn_id, now_ms()],
        )?;
        Ok(())
    }

    /// Stop `target` telling `subscriber` anything; subscribing again keeps it
    /// stopped.
    pub fn pause_chat_subscription(&self, subscriber: &str, target: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE chat_subscriptions SET paused = 1
             WHERE subscriber_session_id = ?1 AND target_session_id = ?2",
            params![subscriber, target],
        )? == 1)
    }

    /// A subscription the user paused stays, so subscribing again cannot undo
    /// the user's choice.
    pub fn unsubscribe_chat(&self, subscriber: &str, target: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM chat_subscriptions
             WHERE subscriber_session_id = ?1 AND target_session_id = ?2 AND paused = 0",
            params![subscriber, target],
        )? == 1)
    }

    /// Every subscription, or one subscriber's.
    pub fn list_chat_subscriptions(
        &self,
        subscriber: Option<&str>,
    ) -> Result<Vec<ChatSubscription>> {
        let mut stmt = self.conn.prepare(&format!(
            "WITH {LATEST_SUBSCRIBED_TURNS}
             SELECT subscriber_session_id, target_session_id, paused, told_prompt_id, pending
             FROM latest
             WHERE ?1 IS NULL OR subscriber_session_id = ?1
             ORDER BY created_at, target_session_id"
        ))?;
        let rows = stmt.query_map([subscriber], |row| {
            Ok(ChatSubscription {
                subscriber_session_id: row.get(0)?,
                target_session_id: row.get(1)?,
                paused: row.get(2)?,
                told_prompt_id: row.get(3)?,
                outstanding: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Subscriptions whose target is idle after a turn the subscriber has not
    /// been told about. A delegate is still working while delegates of its own
    /// have not answered it: a turn that ends waiting on them is not its answer.
    pub fn list_ready_chat_subscriptions(&self) -> Result<Vec<ReadyChatSubscription>> {
        let mut stmt = self.conn.prepare(&format!(
            "WITH {LATEST_SUBSCRIBED_TURNS}
             SELECT subscriber_session_id, target_session_id, turn_id, state,
                    assistant_message_id, created_at, told_turn_id
             FROM latest l
             WHERE l.idle AND l.paused = 0
               AND l.told_turn_id IS NOT l.turn_id
               AND (l.claim_token IS NULL OR l.claimed_at < ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM latest own
                   JOIN chat_delegates d ON d.session_id = own.target_session_id
                       AND d.parent_session_id = own.subscriber_session_id
                   WHERE own.subscriber_session_id = l.target_session_id
                     AND own.pending
               )
             ORDER BY l.updated_at"
        ))?;
        let rows = stmt.query_map([expired_before(CHAT_SUBSCRIPTION_CLAIM_TTL)], |row| {
            Ok(ReadyChatSubscription {
                subscriber_session_id: row.get(0)?,
                target_session_id: row.get(1)?,
                turn_id: row.get(2)?,
                turn_state: row.get(3)?,
                assistant_message_id: row.get(4)?,
                subscribed_at: row.get(5)?,
                told_turn_id: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The answers of turns the target finished since the subscriber was last
    /// told, before the one it is told now: a later turn started before the
    /// subscriber was free to hear them. Oldest first.
    pub fn untold_chat_answers(&self, ready: &ReadyChatSubscription) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT e.assistant_message_id FROM chat_turns e
             JOIN chat_turns now ON now.id = ?2
             WHERE e.session_id = ?1 AND e.id != now.id
               AND e.state IN ('completed', 'failed', 'interrupted')
               AND e.updated_at >= ?3
               AND (e.created_at, e.rowid) < (now.created_at, now.rowid)
               AND NOT EXISTS (
                   SELECT 1 FROM chat_turns told WHERE told.id = ?4
                     AND (e.created_at, e.rowid) <= (told.created_at, told.rowid)
               )
             ORDER BY e.created_at, e.rowid",
        )?;
        let rows = stmt.query_map(
            params![
                ready.target_session_id,
                ready.turn_id,
                ready.subscribed_at,
                ready.told_turn_id
            ],
            |row| row.get(0),
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Lease a subscription to tell `event`, unless it was told already or
    /// another sweep holds the lease.
    pub fn claim_chat_subscription(
        &self,
        subscriber: &str,
        target: &str,
        event: ToldEvent,
    ) -> Result<Option<String>> {
        let token = uuid::Uuid::new_v4().to_string();
        // A turn older than the one told already would rewind the cursor and
        // tell the newer one twice.
        let (told, id, newer) = match event {
            ToldEvent::Turn(id) => (
                "told_turn_id",
                id,
                "AND NOT EXISTS (
                     SELECT 1 FROM chat_turns e JOIN chat_turns t ON t.id = told_turn_id
                     WHERE e.id = ?3 AND (e.created_at, e.rowid) <= (t.created_at, t.rowid)
                 )",
            ),
            ToldEvent::Prompt(id) => ("told_prompt_id", id, ""),
        };
        let claimed = self.conn.execute(
            &format!(
                "UPDATE chat_subscriptions SET claim_token = ?4, claimed_at = ?5
                 WHERE subscriber_session_id = ?1 AND target_session_id = ?2
                   AND {told} IS NOT ?3 {newer}
                   AND (claim_token IS NULL OR claimed_at < ?6)"
            ),
            params![
                subscriber,
                target,
                id,
                token,
                now_ms(),
                expired_before(CHAT_SUBSCRIPTION_CLAIM_TTL)
            ],
        )?;
        Ok((claimed == 1).then_some(token))
    }

    /// Release a claim. `told` records the event as told; without it the
    /// event is retried on a later sweep.
    pub fn settle_chat_subscription(
        &self,
        subscriber: &str,
        target: &str,
        token: &str,
        told: Option<ToldEvent>,
    ) -> Result<bool> {
        let (turn, prompt) = match told {
            Some(ToldEvent::Turn(id)) => (Some(id), None),
            Some(ToldEvent::Prompt(id)) => (None, Some(id)),
            None => (None, None),
        };
        let settled = self.conn.execute(
            "UPDATE chat_subscriptions
             SET claim_token = NULL, claimed_at = NULL,
                 told_turn_id = COALESCE(?4, told_turn_id),
                 told_prompt_id = COALESCE(?5, told_prompt_id)
             WHERE subscriber_session_id = ?1 AND target_session_id = ?2 AND claim_token = ?3",
            params![subscriber, target, token, turn, prompt],
        )?;
        Ok(settled == 1)
    }

    pub fn chat_turn_leased(&self, chat_session_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_turn_leases WHERE chat_session_id = ?1 AND heartbeat_at >= ?2)",
            params![chat_session_id, expired_before(CHAT_TURN_LEASE_TTL)], |row| row.get(0),
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
            params![expired_before(CHAT_TURN_LEASE_TTL)],
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
            params![expired_before(CHAT_TURN_LEASE_TTL)],
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
        acquire_data_dir_lock(self.data_dir_move_lock_path.clone(), true)
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
        let lock = open_lifecycle_lock_at(&self.data_dir_move_lock_path)?;
        match lock.try_read() {
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

    /// First requester wins; true when this call set the cancel intent.
    pub fn request_cancel(&self, run_id: &str, reason: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE runs SET cancel_requested = 1, cancel_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND cancel_requested = 0 AND status IN ('starting', 'running')",
            params![run_id, reason, now_ms()],
        )?;
        Ok(changed == 1)
    }

    /// What a 0.2.x CLI does when its supervisor spawn fails; the reason stays.
    #[cfg(test)]
    pub fn withdraw_cancel(&self, run_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET cancel_requested = 0, updated_at = ?2 WHERE id = ?1",
            params![run_id, now_ms()],
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
                &format!("INSERT INTO local_experiments ({EXPERIMENT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"),
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
                    experiment.archived,
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
                                            active_leaf_id, created_at, updated_at, side_parent_session_id, side_parent_leaf_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
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
                    session.side_parent_session_id,
                    session.side_parent_leaf_id,
                ],
            )?;
            if let Some(state) = self.acp_launch_for_session(session)? {
                self.save_acp_session_state(&session.id, &state)?;
            }
        }
        for message in messages {
            tx.execute(
                "INSERT INTO chat_messages (id, session_id, role, parts_json, created_at,
                                            parent_id, base_native_session_id,
                                            result_native_session_id, completed_at, native_harness)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                         COALESCE(?10, (SELECT harness FROM chat_sessions WHERE id = ?2
                             AND (?7 IS NOT NULL OR ?8 IS NOT NULL))))",
                params![
                    message.id,
                    message.session_id,
                    message.role,
                    message.parts_json,
                    message.created_at,
                    message.parent_id,
                    message.base_native_session_id,
                    message.result_native_session_id,
                    message.completed_at,
                    message.native_harness,
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

    pub(crate) fn run_artifacts_lock(&self) -> Result<fd_lock::RwLock<std::fs::File>> {
        open_lifecycle_lock_at(&self.data_dir.join("run-artifacts.lock"))
    }

    fn run_artifact_path(&self, (root, suffix): (&str, &str), run_id: &str) -> PathBuf {
        self.data_dir.join(root).join(format!("{run_id}{suffix}"))
    }

    fn validate_run_artifact_roots(&self) -> Result<()> {
        for name in RUN_ARTIFACTS
            .iter()
            .map(|(root, _)| *root)
            .chain(["source-snapshots"])
        {
            let path = self.data_dir.join(name);
            match std::fs::metadata(&path) {
                Ok(metadata) if !metadata.file_type().is_dir() => {
                    return Err(anyhow!(
                        "Refusing artifact cleanup through {}",
                        path.display()
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn remove_run_artifacts(&self, run_id: &str) -> Result<()> {
        for entry in artifact_entries(&self.data_dir.join("run-logs"))? {
            if entry.file_name().to_str().and_then(log_generation_run_id) == Some(run_id) {
                remove_run_artifact(&entry.path())?;
            }
        }
        for artifact in RUN_ARTIFACTS {
            remove_run_artifact(&self.run_artifact_path(artifact, run_id))?;
        }
        Ok(())
    }

    /// Reclaim artifacts of deleted runs, then slim finished runs past each project's newest few.
    pub(crate) fn cleanup_orphaned_run_artifacts(&self) -> Result<OrphanPass> {
        let mut lock = self.run_artifacts_lock()?;
        let _guard = match lock.try_write() {
            Ok(guard) => guard,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Ok(OrphanPass::default())
            }
            Err(error) => return Err(error.into()),
        };
        // Another dashboard may be copying this data directory.
        if self.data_dir_move_claimed()? {
            return Ok(OrphanPass::default());
        }
        let started = std::time::Instant::now();
        let mut pass = self.cleanup_run_artifacts_locked(None)?;
        if !pass.more {
            self.slim_finished_runs_locked(started, &mut pass)?;
        }
        Ok(pass)
    }

    fn slim_finished_runs_locked(
        &self,
        started: std::time::Instant,
        pass: &mut OrphanPass,
    ) -> Result<()> {
        let mut statement = self.conn.prepare(&runs_past_newest(LOCAL_RUNS))?;
        let runs = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut remaining = runs.len();
        for (id, status) in runs {
            remaining -= 1;
            if !is_terminal_status(&status) || uuid::Uuid::parse_str(&id).is_err() {
                continue;
            }
            match self.slim_run(&id) {
                Ok(false) => continue,
                Ok(true) => {}
                Err(error) => pass.kept.push((id, error)),
            }
            if remaining > 0 && started.elapsed() >= ORPHAN_PASS_BUDGET {
                pass.more = true;
                return Ok(());
            }
        }
        Ok(())
    }

    /// Returns whether anything was removed.
    fn slim_run(&self, id: &str) -> Result<bool> {
        let root = self.data_dir.join("local-runs");
        let mut targets = Vec::new();
        for parts in SLIMMED_RUN_PATHS {
            targets.extend(real_path_under(
                &root,
                std::iter::once(id).chain(parts.iter().copied()),
            )?);
        }
        if targets.is_empty() {
            return Ok(false);
        }
        if crate::jobs::localbox::orphaned_job_is_running(&root.join(id))?
            || self.supervisor_is_held(id)?
        {
            return Ok(false);
        }
        for target in &targets {
            remove_run_artifact(target)?;
        }
        Ok(true)
    }

    fn cleanup_run_artifacts_locked(
        &self,
        deleting_runs: Option<&std::collections::HashSet<String>>,
    ) -> Result<OrphanPass> {
        let started = std::time::Instant::now();
        let mut pass = OrphanPass::default();
        self.validate_run_artifact_roots()?;
        let mut retained_runs = std::collections::HashSet::new();
        let mut retained_snapshots = std::collections::HashSet::new();
        let mut deleted_runs = std::collections::HashSet::new();
        let mut retain_all_snapshots = false;
        let mut statement = self.conn.prepare("SELECT id, backend_json FROM runs")?;
        for row in statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })? {
            let (id, json) = row?;
            if deleting_runs.is_some_and(|ids| ids.contains(&id)) {
                deleted_runs.insert(id);
                continue;
            }
            retained_runs.insert(id);
            match crate::jobs::BackendDescriptor::parse(&json) {
                Ok(descriptor) => retained_snapshots.extend(descriptor.source_digest),
                // An unparseable row could reference any snapshot.
                Err(_) => retain_all_snapshots = true,
            }
        }
        let mut candidates = deleted_runs.clone();
        for (root, suffix) in RUN_ARTIFACTS {
            for entry in artifact_entries(&self.data_dir.join(root))? {
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(id) = name.strip_suffix(suffix) else {
                    continue;
                };
                let id = if root == "run-logs" && suffix == ".log" {
                    log_generation_run_id(name).unwrap_or(id)
                } else {
                    id
                };
                if uuid::Uuid::parse_str(id).is_ok() && !retained_runs.contains(id) {
                    candidates.insert(id.to_string());
                }
            }
        }
        let mut reclaimable = Vec::new();
        for id in &candidates {
            let deleting = deleted_runs.contains(id);
            let fate = match self.orphan_fate(id, deleting) {
                Ok(fate) => fate,
                Err(error) if deleting => return Err(error),
                // One unreadable orphan must not stall reclaiming the rest.
                Err(error) => {
                    pass.kept.push((id.clone(), error));
                    OrphanFate::KeepWithAllSnapshots
                }
            };
            match fate {
                // Deletion leaves unrelated orphans to the periodic scan.
                OrphanFate::Reclaim if deleting || deleting_runs.is_none() => reclaimable.push(id),
                OrphanFate::Reclaim => {}
                OrphanFate::Keep { snapshot } => retained_snapshots.extend(snapshot),
                OrphanFate::KeepWithAllSnapshots => retain_all_snapshots = true,
            }
        }
        let mut remaining = reclaimable.len();
        for id in reclaimable {
            match self.remove_run_artifacts(id) {
                Ok(()) => {}
                Err(error) if deleted_runs.contains(id) => return Err(error),
                Err(error) => pass.kept.push((id.clone(), error)),
            }
            remaining -= 1;
            // Checked after a removal so every pass makes progress.
            if deleting_runs.is_none() && remaining > 0 && started.elapsed() >= ORPHAN_PASS_BUDGET {
                pass.more = true;
                return Ok(pass);
            }
        }
        if !retain_all_snapshots {
            for entry in artifact_entries(&self.data_dir.join("source-snapshots"))? {
                let name = entry.file_name();
                let Some((digest, extension)) =
                    name.to_str().and_then(|name| name.rsplit_once('.'))
                else {
                    continue;
                };
                if digest.len() == 64
                    && digest.bytes().all(|c| c.is_ascii_hexdigit())
                    && matches!(extension, "tar" | "hf" | "zip")
                    && !retained_snapshots.contains(digest)
                {
                    // ZIPs are upload-only; submit holds the shared lock until the upload finishes.
                    remove_run_artifact(&entry.path())?;
                }
            }
        }
        Ok(pass)
    }

    /// Only a run being deleted turns a live job into an error; other errors are the caller's to absorb.
    fn orphan_fate(&self, id: &str, deleting: bool) -> Result<OrphanFate> {
        let live = |reason: &str| {
            if deleting {
                Err(anyhow!(
                    "Run {id} {reason}; retry the deletion once it stops."
                ))
            } else {
                Ok(OrphanFate::KeepWithAllSnapshots)
            }
        };
        let mut fate = OrphanFate::Reclaim;
        if !deleting {
            match std::fs::read_to_string(self.run_artifact_path(SUBMISSION_HANDLE, id)) {
                Ok(json) => {
                    let descriptor = crate::jobs::BackendDescriptor::parse(&json)?;
                    // Rowless remote handles may be the only way to cancel a still-live provider job.
                    if !matches!(descriptor.kind.as_str(), "local_job" | "tinker_job") {
                        fate = OrphanFate::Keep {
                            snapshot: descriptor.source_digest,
                        };
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        let dir = self.data_dir.join("local-runs").join(id);
        if dir.exists() && crate::jobs::localbox::orphaned_job_is_running(&dir)? {
            return live("still has running processes");
        }
        let forgetting_kill = deleting && self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1 AND contact IS NOT NULL AND {REMOTE_RUNS})"),
            [id],
            |row| row.get::<_, bool>(0),
        )?;
        if !forgetting_kill && self.supervisor_is_held(id)? {
            return live("still has a supervisor");
        }
        Ok(fate)
    }

    fn supervisor_is_held(&self, id: &str) -> Result<bool> {
        let path = self.run_artifact_path(SUPERVISOR_LOCK, id);
        match std::fs::symlink_metadata(&path) {
            // Probing must not create lock files for runs that never had a supervisor.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
            Ok(_) => {}
        }
        let mut supervisor = open_lifecycle_lock_at(&path)?;
        let probe = supervisor.try_write().map(drop);
        match probe {
            Ok(()) => Ok(false),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    fn cleanup_deleted_run_artifacts(&self, runs: &[StoredRun], noun: &str) -> Result<()> {
        if runs.is_empty() {
            return Ok(());
        }
        if runs
            .iter()
            .any(|run| !crate::local::is_terminal(&run.status))
        {
            return Err(anyhow!(
                "Cannot delete this {noun} while its runs are still in flight."
            ));
        }
        validate_deleted_runs(runs)?;
        let ids = runs.iter().map(|run| run.id.clone()).collect();
        self.cleanup_run_artifacts_locked(Some(&ids))?;
        Ok(())
    }

    /// Runs `f` holding run artifacts shared, excluding deletes and orphan scans; waits up to
    /// [`RUN_ARTIFACTS_WAIT`].
    pub(crate) fn with_run_artifacts_shared<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let lock = self.run_artifacts_lock()?;
        let deadline = std::time::Instant::now() + RUN_ARTIFACTS_WAIT;
        let _guard = loop {
            match lock.try_read() {
                Ok(guard) => break guard,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(RUN_ARTIFACTS_POLL);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(anyhow!(
                        "A deletion or artifact cleanup is in progress; retry shortly."
                    ))
                }
                Err(error) => return Err(error.into()),
            }
        };
        f()
    }

    /// Runs `delete` with exclusive use of run artifacts, waiting up to [`RUN_ARTIFACTS_WAIT`].
    fn while_deleting(&self, noun: &str, delete: impl FnOnce() -> Result<()>) -> Result<()> {
        let mut lock = self.run_artifacts_lock()?;
        let deadline = std::time::Instant::now() + RUN_ARTIFACTS_WAIT;
        let _guard = loop {
            match lock.try_write() {
                Ok(guard) => break guard,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(RUN_ARTIFACTS_POLL);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Err(anyhow!("Run artifacts are busy; retry {noun} deletion."))
                }
                Err(error) => return Err(error.into()),
            }
        };
        // A move can start while this waits; its copy must not race the removals below.
        if self.data_dir_move_claimed()? {
            return Err(anyhow!(
                "The data directory is being moved; retry {noun} deletion afterwards."
            ));
        }
        delete()
    }

    /// Delete a childless experiment with its runs' rows and local artifacts, queueing their
    /// remote dirs. `before_rows` runs once the checks pass, so a refusal leaves it undone.
    pub fn delete_local_experiment(
        &self,
        id: &str,
        before_rows: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        self.while_deleting("experiment", || {
            self.delete_local_experiment_locked(id, before_rows)
        })
    }

    fn delete_local_experiment_locked(
        &self,
        id: &str,
        before_rows: impl FnOnce() -> Result<()>,
    ) -> Result<()> {
        // A concurrent delete may have won, and a new experiment may now own its branch.
        if self.get_local_experiment(id)?.is_none() {
            return Err(anyhow!("Experiment {id} not found."));
        }
        let children: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM local_experiments WHERE parent_experiment_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if children > 0 {
            return Err(anyhow!(
                "Experiment {id} has {children} child experiment(s); delete them or move them with `orx exp move` first."
            ));
        }
        let runs = self.list_runs_by_experiment(id)?;
        if runs
            .iter()
            .any(|run| !crate::local::is_terminal(&run.status))
        {
            return Err(anyhow!(
                "Cannot delete this experiment while its runs are still in flight."
            ));
        }
        // Refuse what cleanup would, such as a supervisor still winding down, before `before_rows` acts.
        validate_deleted_runs(&runs)?;
        self.validate_run_artifact_roots()?;
        for run in &runs {
            self.orphan_fate(&run.id, true)?;
        }
        before_rows()?;
        // Keep rows on filesystem errors so deletion can be retried with the same handles.
        self.cleanup_deleted_run_artifacts(&runs, "experiment")?;
        let tx = self.begin()?;
        queue_remote_cleanups(&tx, "experiment_id", id, true)?;
        tx.execute("DELETE FROM runs WHERE experiment_id = ?1", params![id])?;
        tx.execute("DELETE FROM local_experiments WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// A terminal run owes final log and notify collection.
    pub(crate) fn queue_log_collection(&self, run_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO log_collections (run_id, next_attempt_at) VALUES (?1, ?2)",
            params![run_id, now_ms()],
        )?;
        Ok(())
    }

    pub(crate) fn log_output_collected(&self, run_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT collected FROM log_collections WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub(crate) fn checkpoint_log_collection(&self, run_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE log_collections SET collected = 1 WHERE run_id = ?1",
            [run_id],
        )?;
        Ok(())
    }

    /// The terminal run's log and notify have both been consumed.
    pub(crate) fn finish_log_collection(&self, run_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO log_collections (run_id, done) VALUES (?1, 1)
             ON CONFLICT(run_id) DO UPDATE SET done = 1, last_error = NULL",
            params![run_id],
        )?;
        Ok(())
    }

    /// The engine owes this finished run its kept log: local copies may stop short of the end.
    pub(crate) fn log_collection_pending(&self, run_id: &str) -> Result<bool> {
        Ok(self.log_collection(run_id)?.is_some())
    }

    /// An unfinished collection: whether retries have reached their cap, and the last error.
    pub(crate) fn log_collection(&self, run_id: &str) -> Result<Option<(bool, Option<String>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT attempts >= ?2, last_error FROM log_collections
                 WHERE run_id = ?1 AND done = 0",
                params![run_id, LOG_COLLECTION_FAILED_AFTER],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub(crate) fn due_log_collections(&self, limit: usize) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT run_id FROM log_collections WHERE done = 0 AND next_attempt_at <= ?1
             ORDER BY next_attempt_at LIMIT ?2",
        )?;
        let rows = statement.query_map(params![now_ms(), limit as i64], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn pending_log_collections(&self) -> Result<Vec<String>> {
        let mut statement = self
            .conn
            .prepare("SELECT run_id FROM log_collections WHERE done = 0")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Makes one run's unfinished collection, or every one, due now.
    pub(crate) fn retry_log_collections_now(&self, run_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE log_collections SET next_attempt_at = 0
             WHERE done = 0 AND (?1 IS NULL OR run_id = ?1)",
            [run_id],
        )?;
        Ok(())
    }

    /// Schedules the next attempt; true exactly when this failure brings retries to their cap.
    pub(crate) fn defer_log_collection(&self, run_id: &str, error: &str) -> Result<bool> {
        let attempts: u32 = self.conn.query_row(
            "UPDATE log_collections SET attempts = attempts + 1, last_error = ?2,
               next_attempt_at = ?3 + MIN(?4 * (1 << MIN(attempts, 10)), ?5)
             WHERE run_id = ?1 RETURNING attempts",
            params![
                run_id,
                error,
                now_ms(),
                LOG_COLLECTION_RETRY_MS,
                LOG_COLLECTION_MAX_RETRY_MS
            ],
            |row| row.get(0),
        )?;
        Ok(attempts == LOG_COLLECTION_FAILED_AFTER)
    }

    /// Queues a finished run's remote dir for removal; non-SSH/Slurm runs are ignored.
    pub(crate) fn queue_finished_remote_cleanup(&self, run_id: &str) -> Result<()> {
        queue_remote_cleanups(&self.conn, "id", run_id, false)
    }

    /// Due entries; a finished run waits until newer runs push it past its project's newest few.
    pub(crate) fn due_remote_cleanups(&self, limit: usize) -> Result<Vec<RemoteCleanup>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT run_id, backend_json, deleted FROM remote_cleanups
             WHERE next_attempt_at <= ?1 AND {}
             ORDER BY next_attempt_at LIMIT ?2",
            remote_cleanup_eligibility()
        ))?;
        let rows = statement.query_map(params![now_ms(), limit as i64], |row| {
            Ok(RemoteCleanup {
                run_id: row.get(0)?,
                backend_json: row.get(1)?,
                deleted: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn remote_cleanup_eligible(&self, run_id: &str, deleted: bool) -> Result<bool> {
        Ok(self.conn.query_row(&format!(
            "SELECT EXISTS(SELECT 1 FROM remote_cleanups WHERE run_id = ?1 AND deleted = ?2 AND {})",
            remote_cleanup_eligibility()), params![run_id, deleted], |row| row.get(0))?)
    }

    /// `deleted` is what the pass acted on; a deletion queued meanwhile stays for the next pass.
    pub(crate) fn finish_remote_cleanup(&self, run_id: &str, deleted: bool) -> Result<()> {
        self.conn.execute(
            "DELETE FROM remote_cleanups WHERE run_id = ?1 AND deleted = ?2",
            params![run_id, deleted],
        )?;
        Ok(())
    }

    pub(crate) fn defer_remote_cleanup(
        &self,
        run_id: &str,
        deleted: bool,
        error: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE remote_cleanups SET attempts = attempts + 1, last_error = ?3,
               next_attempt_at = ?4 + MIN(?5 * (1 << MIN(attempts, 10)), ?6)
             WHERE run_id = ?1 AND deleted = ?2",
            params![
                run_id,
                deleted,
                error,
                now_ms(),
                REMOTE_CLEANUP_RETRY_MS,
                REMOTE_CLEANUP_MAX_RETRY_MS
            ],
        )?;
        Ok(())
    }

    /// Revisits a host's source cache once its tars from this pass have gone idle.
    pub(crate) fn schedule_source_sweep(&self, host: &str, root: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_source_sweeps (host, root, due_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(host, root) DO UPDATE SET due_at = excluded.due_at",
            params![
                host,
                root.unwrap_or_default(),
                now_ms() + SOURCE_SWEEP_DELAY_MS
            ],
        )?;
        Ok(())
    }

    /// Due (host, root) source-cache sweeps; `None` is the default root.
    pub(crate) fn due_source_sweeps(&self) -> Result<Vec<(String, Option<String>)>> {
        let mut statement = self
            .conn
            .prepare("SELECT host, root FROM remote_source_sweeps WHERE due_at <= ?1")?;
        let rows = statement.query_map(params![now_ms()], |row| {
            let root: String = row.get(1)?;
            Ok((row.get(0)?, Some(root).filter(|root| !root.is_empty())))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn finish_source_sweep(&self, host: &str, root: Option<&str>) -> Result<()> {
        self.conn.execute(
            "DELETE FROM remote_source_sweeps WHERE host = ?1 AND root = ?2",
            params![host, root.unwrap_or_default()],
        )?;
        Ok(())
    }

    fn data_dir_move_claimed(&self) -> Result<bool> {
        self.clear_stale_data_dir_move_lease()?;
        self.data_dir_move_lease_exists()
    }

    fn data_dir_move_lease_exists(&self) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM data_dir_move_lease WHERE id = 1)",
            [],
            |row| row.get(0),
        )?)
    }

    #[cfg(test)]
    pub fn delete_local_project(&self, id: &str) -> Result<()> {
        self.delete_local_project_with(id, None::<fn() -> Result<()>>)
    }

    /// Delete a project, its rows, run artifacts and chat attachments; its artifacts folder is the caller's. GitHub repo and cache clone are kept.
    /// Blocks up to [`RUN_ARTIFACTS_WAIT`] while a submission or orphan scan holds the artifacts lock.
    /// `native_purge` runs under the native-deletion lock after run-artifact cleanup, just before the rows go.
    pub fn delete_local_project_with(
        &self,
        id: &str,
        native_purge: Option<impl FnOnce() -> Result<()>>,
    ) -> Result<()> {
        self.while_deleting("project", || {
            self.delete_local_project_locked(id, native_purge)
        })
    }

    fn delete_local_project_locked(
        &self,
        id: &str,
        native_purge: Option<impl FnOnce() -> Result<()>>,
    ) -> Result<()> {
        let _native_deletion = native_purge
            .is_some()
            .then(|| self.lock_native_deletion())
            .transpose()?;
        // Keep rows on filesystem errors so deletion can be retried with the same handles.
        self.cleanup_deleted_run_artifacts(&self.list_runs_by_project(id)?, "project")?;
        if let Some(purge) = native_purge {
            purge()?;
        }
        let tx = self.begin()?;
        let attachments = self
            .chat_attachment_names_where(
                "m.session_id IN (SELECT id FROM chat_sessions WHERE project_id = ?1)",
                [id],
            )
            .unwrap_or_default();
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
            "DELETE FROM chat_messages WHERE session_id IN
               (SELECT id FROM chat_sessions WHERE project_id = ?1)",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM chat_sessions WHERE project_id = ?1",
            params![id],
        )?;
        queue_remote_cleanups(&tx, "project_id", id, true)?;
        self.conn
            .execute("DELETE FROM runs WHERE project_id = ?1", params![id])?;
        self.conn.execute(
            "DELETE FROM local_experiments WHERE project_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM local_projects WHERE id = ?1", params![id])?;
        tx.commit()?;
        self.remove_unreferenced_chat_attachments(attachments);
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
            &format!("INSERT INTO local_experiments ({EXPERIMENT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"),
            params![
                e.id, e.project_id, e.parent_experiment_id, e.slug, e.branch_name,
                e.title, e.description, e.run_command, e.agent_status, e.created_at, e.updated_at,
                e.chat_session_id, e.archived,
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

    pub fn list_experiments_by_chat_session(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionExperiment>> {
        let mut stmt = self.conn.prepare(&format!(
            "WITH related AS (
                 SELECT id AS related_id FROM local_experiments WHERE chat_session_id = ?1
                 UNION SELECT experiment_id FROM runs WHERE chat_session_id = ?1
                 UNION SELECT runs.experiment_id FROM chat_run_wakeups w
                       JOIN runs ON runs.id = w.run_id WHERE w.chat_session_id = ?1
                 UNION SELECT runs.experiment_id FROM run_subscriptions s
                       JOIN runs ON runs.id = s.run_id WHERE s.chat_session_id = ?1
             ), headline AS (
                 SELECT experiment_id AS current_experiment_id, id AS current_run_id, status AS current_status,
                        created_at AS current_created_at, ended_at AS current_ended_at,
                        ROW_NUMBER() OVER (
                            PARTITION BY experiment_id
                            ORDER BY status IN ('submitting', 'starting', 'running') DESC, created_at DESC, id DESC
                        ) AS current_rank
                 FROM runs WHERE experiment_id IN (SELECT related_id FROM related)
             )
             SELECT {EXPERIMENT_COLS}, project_name, current_run_id
             FROM local_experiments e
             JOIN related ON related_id = e.id
             JOIN (SELECT id AS owner_id, name AS project_name FROM local_projects)
                  ON owner_id = e.project_id
             LEFT JOIN headline ON current_experiment_id = e.id AND current_rank = 1
             ORDER BY COALESCE(current_status IN ('submitting', 'starting', 'running'), 0) DESC,
                      e.archived ASC,
                      COALESCE(current_ended_at, current_created_at, e.created_at) DESC,
                      e.id ASC"
        ))?;
        let rows = stmt.query_map(params![session_id], |row| {
            Ok((
                LocalExperiment::from_row(row)?,
                row.get::<_, String>(13)?,
                row.get::<_, Option<String>>(14)?,
            ))
        })?;
        let rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(experiment, project_name, run_id)| {
                let current_run = run_id.map(|id| self.get_run(&id)).transpose()?.flatten();
                Ok(SessionExperiment {
                    experiment,
                    project_name,
                    current_run,
                })
            })
            .collect()
    }

    /// Full-row update by id (title / description / run_command / agent_status).
    ///
    /// `chat_session_id` is deliberately omitted: session ownership is stamped
    /// once at creation and is immutable thereafter. The parent changes only
    /// through `set_experiment_parent`, so a stale row cannot undo a move.
    pub fn update_local_experiment(&self, e: &LocalExperiment) -> Result<()> {
        self.conn.execute(
            "UPDATE local_experiments SET slug = ?2, branch_name = ?3,
                    title = ?4, description = ?5, run_command = ?6, agent_status = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                e.id, e.slug, e.branch_name,
                e.title, e.description, e.run_command, e.agent_status, now_ms(),
            ],
        )?;
        Ok(())
    }

    pub fn set_experiment_parent(&self, id: &str, parent: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE local_experiments SET parent_experiment_id = ?2, updated_at = MAX(updated_at + 1, ?3) WHERE id = ?1",
            params![id, parent, now_ms()],
        )?;
        Ok(())
    }

    pub fn set_experiments_archived(
        &mut self,
        ids: &[String],
        archived: bool,
    ) -> Result<Vec<String>> {
        let tx = self.conn.transaction()?;
        let mut changed = Vec::new();
        for id in ids {
            let rows = tx.execute(
                "UPDATE local_experiments SET archived = ?2, updated_at = MAX(updated_at + 1, ?3) WHERE id = ?1 AND archived != ?2",
                params![id, archived, now_ms()],
            )?;
            if rows > 0 {
                changed.push(id.clone());
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    // --- chat sessions / messages ------------------------------------------

    pub fn create_chat_session(&self, s: &StoredChatSession) -> Result<()> {
        let acp_state = self.acp_launch_for_session(s)?;
        let transaction = if acp_state.is_some() && self.conn.is_autocommit() {
            Some(self.conn.unchecked_transaction()?)
        } else {
            None
        };
        self.conn.execute(
            "INSERT INTO chat_sessions (id, project_id, harness, native_session_id, title, title_source, model,
                                        service_tier, permission_mode, plan_mode, plan_reset_pending, reasoning_level, archived, bootstrap_context,
                                        active_leaf_id, created_at, updated_at, autonomy,
                                        side_parent_session_id, side_parent_leaf_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
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
                s.created_at,
                s.updated_at,
                s.autonomy,
                s.side_parent_session_id,
                s.side_parent_leaf_id,
            ],
        )?;
        if let Some(state) = acp_state {
            self.save_acp_session_state(&s.id, &state)?;
        }
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(())
    }

    pub fn get_chat_session(&self, id: &str) -> Result<Option<StoredChatSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_SESSION_COLS} FROM chat_sessions WHERE id = ?1"
        ))?;
        let mut rows = stmt.query_map(params![id], row_to_chat_session)?;
        Ok(rows.next().transpose()?)
    }

    pub fn chat_session_awaiting_input(&self, session_id: &str, busy: bool) -> Result<bool> {
        Ok(self.conn.query_row(
            "WITH RECURSIVE active(id, parent_id, parts_json) AS (
                SELECT m.id, m.parent_id, m.parts_json FROM chat_messages m
                JOIN chat_sessions s ON s.active_leaf_id = m.id AND s.id = m.session_id
                WHERE s.id = ?1
                UNION ALL
                SELECT m.id, m.parent_id, m.parts_json FROM chat_messages m
                JOIN active a ON a.parent_id = m.id WHERE m.session_id = ?1
             )
             SELECT EXISTS (
                SELECT 1 FROM active a, json_each(a.parts_json) p
                WHERE json_extract(p.value, '$.type') = 'prompt'
                  AND COALESCE(json_extract(p.value, '$.prompt.resolved'), 0) = 0
                  AND CASE WHEN json_extract(p.value, '$.prompt.nativeId') IS NOT NULL
                    THEN ?2
                    ELSE json_extract(p.value, '$.prompt.kind') = 'question'
                  END
             )",
            params![session_id, busy],
            |row| row.get(0),
        )?)
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

    /// Every project's sessions for the composer's `/resume` picker, capped so a
    /// long-lived install cannot turn one dialog open into a multi-megabyte read.
    pub fn list_all_chat_sessions(&self) -> Result<Vec<StoredChatSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_SESSION_COLS} FROM chat_sessions WHERE side_parent_session_id IS NULL
             ORDER BY updated_at DESC LIMIT 500"
        ))?;
        let rows = stmt.query_map([], row_to_chat_session)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn list_sidebar_chat_sessions(
        &self,
        archived: Option<bool>,
        before: Option<(i64, &str)>,
    ) -> Result<Vec<StoredChatSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_SESSION_COLS} FROM chat_sessions
             WHERE side_parent_session_id IS NULL AND (?1 IS NULL OR archived = ?1)
               AND (?2 IS NULL OR updated_at < ?2 OR (updated_at = ?2 AND id < ?3))
             ORDER BY updated_at DESC, id DESC LIMIT 51"
        ))?;
        let rows = stmt.query_map(
            params![
                archived,
                before.map(|cursor| cursor.0),
                before.map(|cursor| cursor.1)
            ],
            row_to_chat_session,
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn side_chats_of(&self, parent_id: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM chat_sessions WHERE side_parent_session_id = ?1")?;
        let rows = stmt.query_map(params![parent_id], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The session whose worktree `id` runs in: the top of its chain of
    /// existing side-chat parents (a delegate can delegate again), else itself.
    pub fn chat_worktree_owner(&self, id: &str) -> Result<String> {
        Ok(self.conn.query_row(
            "WITH RECURSIVE chain(id, depth) AS (
                 SELECT ?1, 0
                 UNION ALL
                 SELECT parent.id, chain.depth + 1 FROM chain
                 JOIN chat_sessions side ON side.id = chain.id
                 JOIN chat_sessions parent ON parent.id = side.side_parent_session_id
             )
             SELECT id FROM chain ORDER BY depth DESC LIMIT 1",
            params![id],
            |row| row.get(0),
        )?)
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
                 WHERE side_parent_session_id IS NULL
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
             , latest_activity AS (
                 SELECT project_id, MAX(updated_at) AS last_activity_at FROM (
                     SELECT id AS project_id, updated_at FROM local_projects
                     UNION ALL SELECT project_id, updated_at FROM chat_sessions
                     UNION ALL SELECT project_id, updated_at FROM local_experiments
                     UNION ALL SELECT project_id, updated_at FROM runs
                     UNION ALL
                     SELECT chat_sessions.project_id, chat_messages.created_at
                     FROM chat_messages JOIN chat_sessions ON chat_sessions.id = chat_messages.session_id
                 ) GROUP BY project_id
             )
             SELECT local_projects.id,
                    COALESCE(agent_counts.total_agents, 0),
                    COALESCE(running_experiment_counts.running_experiments, 0),
                    COALESCE(experiment_counts.total_experiments, 0),
                    latest_messages.last_message_at,
                    latest_activity.last_activity_at
             FROM local_projects
             LEFT JOIN agent_counts ON agent_counts.project_id = local_projects.id
             LEFT JOIN experiment_counts ON experiment_counts.project_id = local_projects.id
             LEFT JOIN running_experiment_counts
               ON running_experiment_counts.project_id = local_projects.id
             LEFT JOIN latest_messages ON latest_messages.project_id = local_projects.id
             JOIN latest_activity ON latest_activity.project_id = local_projects.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectActivitySummary {
                project_id: row.get(0)?,
                total_agents: row.get::<_, i64>(1)? as usize,
                running_experiments: row.get::<_, i64>(2)? as usize,
                total_experiments: row.get::<_, i64>(3)? as usize,
                last_message_at: row.get(4)?,
                last_activity_at: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Hold through ownership lookup, native purge and local row deletion, across dashboard processes.
    pub fn lock_native_deletion(&self) -> Result<std::fs::File> {
        let path = self
            .conn
            .path()
            .ok_or_else(|| anyhow!("Store has no database path"))?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(Path::new(path).with_extension("native-deletion.lock"))?;
        lock.try_lock().map_err(|error| {
            anyhow!(
                "Could not lock native chat deletion; retry when other deletions finish: {error}"
            )
        })?;
        Ok(lock)
    }

    pub fn native_sessions_for_deletion(
        &self,
        chat_ids: &[String],
        harnesses: &[&str],
    ) -> Result<(
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
    )> {
        let mut query = self.conn.prepare(
            "WITH refs AS (
                SELECT id AS chat_id, native_session_id AS native_id FROM chat_sessions
                WHERE harness IN (SELECT value FROM json_each(?2))
                UNION
                SELECT s.id, m.base_native_session_id FROM chat_sessions s JOIN chat_messages m
                ON m.session_id = s.id WHERE COALESCE(m.native_harness, s.harness) IN (SELECT value FROM json_each(?2))
                UNION
                SELECT s.id, m.result_native_session_id FROM chat_sessions s JOIN chat_messages m
                ON m.session_id = s.id WHERE COALESCE(m.native_harness, s.harness) IN (SELECT value FROM json_each(?2))
             )
             SELECT native_id, MAX(chat_id IN (SELECT value FROM json_each(?1))),
                 MAX(chat_id NOT IN (SELECT value FROM json_each(?1))) FROM refs
             WHERE native_id IS NOT NULL GROUP BY native_id",
        )?;
        let mut deleting = std::collections::BTreeSet::new();
        let mut retained = std::collections::BTreeSet::new();
        for row in query.query_map(
            [
                serde_json::to_string(chat_ids)?,
                serde_json::to_string(harnesses)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )? {
            let (native_id, belongs_to_chat, shared) = row?;
            if shared {
                retained.insert(native_id);
            } else if belongs_to_chat {
                deleting.insert(native_id);
            }
        }
        Ok((deleting, retained))
    }

    pub fn delete_chat_session(&self, id: &str) -> Result<()> {
        let tx = self.begin_immediate()?;
        let has_children: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE side_parent_session_id = ?1)",
            [id],
            |row| row.get(0),
        )?;
        if has_children {
            return Err(anyhow!("delete side chats before deleting their parent"));
        }
        let attachments = self
            .chat_attachment_names_where("m.session_id = ?1", [id])
            .unwrap_or_default();
        tx.execute(
            "DELETE FROM chat_queued_messages WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM chat_turns WHERE session_id = ?1", params![id])?;
        tx.execute(
            "DELETE FROM chat_messages WHERE session_id = ?1",
            params![id],
        )?;
        tx.execute("DELETE FROM chat_sessions WHERE id = ?1", params![id])?;
        tx.commit()?;
        self.remove_unreferenced_chat_attachments(attachments);
        Ok(())
    }

    /// Attachment file names that image parts of matching messages show; only user messages carry them.
    fn chat_attachment_names_where(
        &self,
        filter: &'static str,
        params: impl rusqlite::Params,
    ) -> Result<std::collections::HashSet<String>> {
        let mut query = self.conn.prepare(&format!(
            "SELECT DISTINCT json_extract(p.value, '$.text')
             FROM chat_messages m, json_each(m.parts_json) p
             WHERE m.role = 'user' AND json_extract(p.value, '$.type') = 'image'
               AND json_extract(p.value, '$.text') IS NOT NULL AND {filter}"
        ))?;
        let names = query
            .query_map(params, |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(names)
    }

    /// Best effort once the rows are gone; the periodic sweep retries what is left.
    fn remove_unreferenced_chat_attachments(&self, names: std::collections::HashSet<String>) {
        if names.is_empty() {
            return;
        }
        let Ok(Some(_move_hold)) = self.hold_off_data_dir_move() else {
            return;
        };
        let Ok(referenced) = self.chat_attachment_names_where("1", []) else {
            return;
        };
        let dir = self.data_dir.join(crate::local::chat::ATTACHMENTS_DIR);
        for name in names.difference(&referenced) {
            if crate::local::chat::is_attachment_name(name) {
                let _ = std::fs::remove_file(dir.join(name));
            }
        }
    }

    /// Keeps any dashboard from starting a data-dir move until dropped; `None` while a move holds or claims it.
    fn hold_off_data_dir_move(&self) -> Result<Option<std::fs::File>> {
        if self.data_dir_move_claimed()? {
            return Ok(None);
        }
        // A move takes this lock exclusively before claiming its lease.
        let hold = open_lifecycle_lock_at(&self.data_dir_move_lock_path)?.into_inner();
        match hold.try_lock_shared() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
        Ok((!self.data_dir_move_lease_exists()?).then_some(hold))
    }

    /// Remove attachment files no message shows; returns whether the budget ran out first.
    pub(crate) fn cleanup_orphaned_chat_attachments(&self) -> Result<bool> {
        self.cleanup_orphaned_chat_attachments_within(ORPHAN_PASS_BUDGET)
    }

    /// The budget starts at the first removal, so every pass makes progress; removed files leave the listing.
    fn cleanup_orphaned_chat_attachments_within(&self, budget: Duration) -> Result<bool> {
        let Some(_move_hold) = self.hold_off_data_dir_move()? else {
            return Ok(false);
        };
        let entries =
            match std::fs::read_dir(self.data_dir.join(crate::local::chat::ATTACHMENTS_DIR)) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.into()),
            };
        let mut started = None;
        let mut referenced = None;
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let settled = entry.metadata().ok().filter(|metadata| {
                metadata.is_file()
                    && metadata
                        .modified()
                        .is_ok_and(|at| at.elapsed().unwrap_or_default() >= CHAT_ATTACHMENT_SETTLE)
            });
            if !crate::local::chat::is_attachment_name(&name) || settled.is_none() {
                continue;
            }
            let referenced = match &mut referenced {
                Some(referenced) => referenced,
                None => referenced.insert(self.chat_attachment_names_where("1", [])?),
            };
            if referenced.contains(&name) {
                continue;
            }
            let started = *started.get_or_insert_with(std::time::Instant::now);
            // A file that cannot be removed is retried next pass without stalling the rest.
            let _ = std::fs::remove_file(entry.path());
            if started.elapsed() >= budget {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Reclaim orx-launched agent history no chat records; returns whether the budget ran out first.
    pub(crate) fn cleanup_orphaned_native_sessions(&self) -> Result<bool> {
        let Some(_move_hold) = self.hold_off_data_dir_move()? else {
            return Ok(false);
        };
        crate::local::native_store::sweep_isolated_sessions(
            &self.data_dir.join("agents"),
            self.native_ids_outside(&[])?,
            NATIVE_SESSION_SETTLE,
            ORPHAN_PASS_BUDGET,
        )
    }

    /// Every native id recorded by chats other than `chat_ids`.
    pub fn native_ids_outside(
        &self,
        chat_ids: &[String],
    ) -> Result<std::collections::HashSet<String>> {
        let mut query = self.conn.prepare(
            "SELECT native_session_id FROM chat_sessions
             WHERE native_session_id IS NOT NULL AND id NOT IN (SELECT value FROM json_each(?1))
             UNION SELECT base_native_session_id FROM chat_messages
             WHERE base_native_session_id IS NOT NULL
               AND session_id NOT IN (SELECT value FROM json_each(?1))
             UNION SELECT result_native_session_id FROM chat_messages
             WHERE result_native_session_id IS NOT NULL
               AND session_id NOT IN (SELECT value FROM json_each(?1))",
        )?;
        let ids = query
            .query_map([serde_json::to_string(chat_ids)?], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<std::result::Result<_, _>>()?;
        Ok(ids)
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

    pub fn switch_chat_agent(
        &self,
        id: &str,
        agent: &StoredAgentSelection,
        native_id: Option<&str>,
        marker: &StoredChatMessage,
        plan_mode: bool,
    ) -> Result<()> {
        let tx = rusqlite::Transaction::new_unchecked(
            &self.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        tx.execute(
            "UPDATE chat_messages SET native_harness = (SELECT harness FROM chat_sessions WHERE id = ?1)
             WHERE session_id = ?1 AND native_harness IS NULL
               AND (role = 'user' OR base_native_session_id IS NOT NULL OR result_native_session_id IS NOT NULL)",
            [id],
        )?;
        upsert_chat_message_with(&tx, marker)?;
        tx.execute(
            "UPDATE chat_sessions SET harness = ?2, model = ?3, service_tier = ?4,
             permission_mode = ?5, reasoning_level = ?6, native_session_id = ?7,
             context_usage_json = NULL, active_leaf_id = ?8,
             plan_mode = ?9,
             plan_reset_pending = (?2 = 'codex' AND ?7 IS NOT NULL AND NOT ?9),
             updated_at = ?10 WHERE id = ?1",
            params![
                id,
                agent.harness,
                agent.model,
                agent.service_tier,
                agent.permission_mode,
                agent.reasoning_level,
                native_id,
                marker.id,
                plan_mode,
                now_ms()
            ],
        )?;
        tx.commit()?;
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

    /// The session bound to an agent's own chat, if one was already adopted.
    pub fn chat_session_for_native_id(&self, native_id: &str) -> Result<Option<StoredChatSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CHAT_SESSION_COLS} FROM chat_sessions WHERE native_session_id = ?1
             OR id IN (SELECT session_id FROM chat_messages
                 WHERE base_native_session_id = ?1 OR result_native_session_id = ?1)
             ORDER BY created_at LIMIT 1"
        ))?;
        let mut rows = stmt.query_map(params![native_id], row_to_chat_session)?;
        Ok(rows.next().transpose()?)
    }

    pub fn set_chat_session_autonomy(&self, id: &str, autonomy: Autonomy) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET autonomy = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, autonomy.id(), now_ms()],
        )?;
        Ok(())
    }

    pub fn set_chat_session_goal(&self, id: &str, goal: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET goal = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, goal, now_ms()],
        )?;
        Ok(())
    }

    /// Cleared after a compaction: the pre-compaction total would otherwise be
    /// inherited by the next turn and keep the meter pinned high.
    pub fn clear_chat_session_context_usage(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET context_usage_json = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn set_chat_session_bootstrap_context(
        &self,
        id: &str,
        context: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_sessions SET bootstrap_context = ?2 WHERE id = ?1",
            params![id, context],
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
        let steer_exists: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM chat_steer_receipts WHERE session_id = ?1 AND client_turn_id = ?2)",
            params![turn.session_id, turn.client_turn_id],
            |row| row.get(0),
        )?;
        if steer_exists {
            tx.commit()?;
            return Ok(ChatTurnAdmission::Conflict);
        }
        if let Some(message) = user_message {
            tx.execute(
                "INSERT INTO chat_messages
                 (id, session_id, role, parts_json, created_at, parent_id,
                  base_native_session_id, result_native_session_id, completed_at, native_harness)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                         COALESCE(?10, (SELECT harness FROM chat_sessions WHERE id = ?2
                             AND (?7 IS NOT NULL OR ?8 IS NOT NULL))))",
                params![
                    message.id,
                    message.session_id,
                    message.role,
                    message.parts_json,
                    message.created_at,
                    message.parent_id,
                    message.base_native_session_id,
                    message.result_native_session_id,
                    message.completed_at,
                    message.native_harness,
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

    /// The turn a user message started.
    pub fn chat_turn_for_user_message(
        &self,
        session_id: &str,
        message_id: &str,
    ) -> Result<Option<StoredChatTurn>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CHAT_TURN_COLS} FROM chat_turns
                     WHERE session_id = ?1 AND user_message_id = ?2
                     ORDER BY created_at, rowid LIMIT 1"
                ),
                params![session_id, message_id],
                row_to_chat_turn,
            )
            .optional()?)
    }

    pub fn latest_chat_turn(&self, session_id: &str) -> Result<Option<StoredChatTurn>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {CHAT_TURN_COLS} FROM chat_turns WHERE session_id = ?1
                     ORDER BY created_at DESC, rowid DESC LIMIT 1"
                ),
                params![session_id],
                row_to_chat_turn,
            )
            .optional()?)
    }

    pub(crate) fn steer_receipt(
        &self,
        session_id: &str,
        client_turn_id: &str,
    ) -> Result<Option<(String, bool)>> {
        Ok(self.conn.query_row(
            "SELECT request_hash, delivered FROM chat_steer_receipts WHERE session_id = ?1 AND client_turn_id = ?2",
            params![session_id, client_turn_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?)
    }

    pub(crate) fn claim_steer(
        &self,
        session_id: &str,
        client_turn_id: &str,
        request_hash: &str,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "INSERT OR IGNORE INTO chat_steer_receipts (session_id, client_turn_id, request_hash)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM chat_sessions WHERE id = ?1)
             AND NOT EXISTS (SELECT 1 FROM chat_turns WHERE session_id = ?1 AND client_turn_id = ?2)
             AND NOT EXISTS (SELECT 1 FROM chat_queued_messages WHERE session_id = ?1 AND client_turn_id = ?2)",
            params![session_id, client_turn_id, request_hash],
        )? == 1)
    }

    pub(crate) fn finish_steer(&self, session_id: &str, client_turn_id: &str) -> Result<()> {
        self.conn.execute("UPDATE chat_steer_receipts SET delivered = 1 WHERE session_id = ?1 AND client_turn_id = ?2", params![session_id, client_turn_id])?;
        Ok(())
    }

    pub(crate) fn discard_steer(&self, session_id: &str, client_turn_id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM chat_steer_receipts WHERE session_id = ?1 AND client_turn_id = ?2 AND delivered = 0", params![session_id, client_turn_id])?;
        Ok(())
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

    pub fn has_chat_submission(&self, session_id: &str, client_turn_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_queued_messages WHERE session_id = ?1 AND client_turn_id = ?2)
                 OR EXISTS(SELECT 1 FROM chat_turns WHERE session_id = ?1 AND client_turn_id = ?2)
                 OR EXISTS(SELECT 1 FROM chat_steer_receipts WHERE session_id = ?1 AND client_turn_id = ?2 AND delivered = 1)",
            params![session_id, client_turn_id],
            |row| row.get(0),
        )?)
    }

    pub fn chat_native_thread_has_named_model(
        &self,
        session_id: &str,
        native_session_id: &str,
    ) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (
                SELECT 1 FROM chat_turns t
                JOIN chat_messages m ON m.id = t.assistant_message_id
                WHERE t.session_id = ?1 AND m.result_native_session_id = ?2
                  AND json_extract(t.settings_json, '$.model') IS NOT NULL
            )",
            params![session_id, native_session_id],
            |row| row.get(0),
        )?)
    }

    pub fn insert_queued_chat_message(&self, message: &StoredQueuedChatMessage) -> Result<()> {
        let changed = self.conn.execute(
            "INSERT INTO chat_queued_messages
             (id, session_id, client_turn_id, request_hash, payload_json, created_at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6
             WHERE EXISTS (SELECT 1 FROM chat_sessions WHERE id = ?2)
             AND NOT EXISTS (SELECT 1 FROM chat_steer_receipts WHERE session_id = ?2 AND client_turn_id = ?3)",
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
            return Err(anyhow!(
                "chat session not found or message already admitted as steering"
            ));
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

    pub fn reset_chat_turn_for_retry(&self, id: &str, started_at: i64) -> Result<bool> {
        let transaction = self.conn.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE chat_turns SET state = 'preparing', delivery_state = 'not_sent',
                 attempt_count = 0, next_retry_at = NULL, error_kind = NULL,
                 error_message = NULL, recovery_action = NULL, updated_at = ?2
             WHERE id = ?1 AND state = 'failed' AND recovery_action = 'retry'
                   AND recovered_by_turn_id IS NULL",
            params![id, started_at],
        )?;
        if changed > 0 {
            transaction.execute(
                "UPDATE chat_messages SET completed_at = NULL, created_at = ?2 WHERE id =
                 (SELECT assistant_message_id FROM chat_turns WHERE id = ?1)",
                params![id, started_at],
            )?;
        }
        transaction.commit()?;
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
        // Sub-agent usage outliving its parent turn: its native connection died with the server.
        self.finalize_usage_turns("codex-late:", "cancelled")?;
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
            params![expired_before(CHAT_TURN_LEASE_TTL)],
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
        self.recover_terminal_usage()?;
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
        self.list_chat_messages_since(session_id, i64::MIN)
    }

    pub fn unconfirmed_chat_replies(
        &self,
        session_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let mut query = self.conn.prepare(
            "SELECT assistant_message_id FROM chat_turns WHERE session_id = ?1
             AND state != 'completed' AND delivery_state != 'accepted'",
        )?;
        let replies = query
            .query_map([session_id], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        Ok(replies)
    }

    /// A chat's messages created at or after `since`, oldest first.
    pub fn list_chat_messages_since(
        &self,
        session_id: &str,
        since: i64,
    ) -> Result<Vec<StoredChatMessage>> {
        let mut stmt = self.conn.prepare(
            // rowid tiebreak: a user message and its reply can share a millisecond.
            "SELECT id, session_id, role, parts_json, created_at, parent_id,
                    base_native_session_id, result_native_session_id, completed_at, native_harness
             FROM chat_messages
             WHERE session_id = ?1 AND created_at >= ?2 ORDER BY created_at ASC, rowid ASC",
        )?;
        let rows = stmt.query_map(params![session_id, since], |row| {
            Ok(StoredChatMessage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                role: row.get(2)?,
                parts_json: row.get(3)?,
                created_at: row.get(4)?,
                completed_at: row.get(8)?,
                parent_id: row.get(5)?,
                base_native_session_id: row.get(6)?,
                result_native_session_id: row.get(7)?,
                native_harness: row.get(9)?,
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

    /// Message ids that start with `msg_` and `hex`, at most two: enough to
    /// tell a unique match from an ambiguous one.
    pub fn chat_message_ids_with_prefix(&self, hex: &str) -> Result<Vec<String>> {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM chat_messages WHERE id GLOB ?1 LIMIT 2")?;
        let rows = stmt.query_map([format!("msg_{hex}*")], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// A single chat message by id (used to reconcile a message's persisted
    /// state against an in-memory copy mid-turn). `None` if it doesn't exist.
    pub fn get_chat_message(&self, id: &str) -> Result<Option<StoredChatMessage>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, session_id, role, parts_json, created_at, parent_id,
                        base_native_session_id, result_native_session_id, completed_at, native_harness
                 FROM chat_messages WHERE id = ?1",
                params![id],
                |row| {
                    Ok(StoredChatMessage {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        role: row.get(2)?,
                        parts_json: row.get(3)?,
                        created_at: row.get(4)?,
                        completed_at: row.get(8)?,
                        parent_id: row.get(5)?,
                        base_native_session_id: row.get(6)?,
                        result_native_session_id: row.get(7)?,
                        native_harness: row.get(9)?,
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

impl OverleafLink {
    pub fn project(&self) -> crate::local::overleaf::Project {
        crate::local::overleaf::Project {
            id: self.overleaf_project_id.clone(),
            host: self.host.clone(),
        }
    }
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
    /// (harness auto-title), `"user"` (explicitly chosen — a rename, or the
    /// title a delegating agent gave, neither of which auto-titling may
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
    /// What the user asked the agent to keep working toward (`/goal`), carried
    /// into every turn until they clear it.
    pub goal: Option<String>,
    /// Research autonomy level id (`crate::local::autonomy`); `None` is the default.
    pub autonomy: Option<String>,
    /// Tip of the branch the UI is currently showing. Forked turns make the
    /// transcript a tree; this picks which path through it is live.
    pub active_leaf_id: Option<String>,
    /// Chat this side chat branched from. Side chats share that chat's
    /// worktree, stay out of history, and are deleted with it.
    pub side_parent_session_id: Option<String>,
    /// Parent branch tip captured when this side chat was created.
    pub side_parent_leaf_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredAgentSelection {
    pub harness: String,
    pub model: Option<String>,
    pub service_tier: Option<String>,
    pub permission_mode: Option<String>,
    pub reasoning_level: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StoredUiState {
    pub onboarding_completed: bool,
    pub tour_completed: bool,
    pub preferred_agent: Option<StoredAgentSelection>,
    pub workspace: Option<GlobalWorkspaceState>,
    pub preferred_autonomy: Option<Autonomy>,
    /// Whether delegates may delegate in turn unless a `delegate` call says
    /// otherwise. Off: only chats the user started delegate.
    pub nested_delegation: bool,
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
    pub completed_at: Option<i64>,
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
    /// The harness that owns both native session ids on this message.
    pub native_harness: Option<String>,
}

fn upsert_chat_message_with(conn: &Connection, m: &StoredChatMessage) -> Result<()> {
    conn.execute(
        // A streaming message is written many times; only its parts and a
        // late-arriving harness session id may change, never its place in
        // the tree.
        "INSERT INTO chat_messages
                 (id, session_id, role, parts_json, created_at, parent_id,
                  base_native_session_id, result_native_session_id, completed_at, native_harness)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                     COALESCE(?10, (SELECT harness FROM chat_sessions WHERE id = ?2
                         AND (?7 IS NOT NULL OR ?8 IS NOT NULL))))
             ON CONFLICT(id) DO UPDATE SET
                 parts_json = excluded.parts_json,
                 completed_at = COALESCE(excluded.completed_at, completed_at),
                 result_native_session_id = COALESCE(
                     excluded.result_native_session_id, result_native_session_id),
                 native_harness = COALESCE(excluded.native_harness, native_harness)",
        params![
            m.id,
            m.session_id,
            m.role,
            m.parts_json,
            m.created_at,
            m.parent_id,
            m.base_native_session_id,
            m.result_native_session_id,
            m.completed_at,
            m.native_harness
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
     created_at, updated_at, title_source, bootstrap_context, active_leaf_id, \
     goal, autonomy, side_parent_session_id, side_parent_leaf_id";

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
        goal: row.get(18)?,
        autonomy: row.get(19)?,
        side_parent_session_id: row.get(20)?,
        side_parent_leaf_id: row.get(21)?,
    })
}

const RUN_NOTICE_RETRY_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

impl Store {
    pub(crate) fn engine_host_auth(
        &self,
        alias: &str,
    ) -> Result<crate::engine::transport::AuthState> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT state_json FROM engine_host_auth WHERE alias = ?1",
                [alias],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    pub(crate) fn save_engine_host_auth(
        &self,
        alias: &str,
        state: &crate::engine::transport::AuthState,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO engine_host_auth (alias, state_json) VALUES (?1, ?2) ON CONFLICT(alias) DO UPDATE SET state_json = excluded.state_json",
            params![alias, serde_json::to_string(state)?],
        )?;
        Ok(())
    }

    pub fn engine_hosts(
        &self,
    ) -> Result<Vec<(String, String, crate::engine::transport::HostState)>> {
        let mut query = self
            .conn
            .prepare("SELECT alias, root FROM engine_hosts ORDER BY alias, root")?;
        let rows = query.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (alias, root) = row?;
            let state = self.engine_host(&alias, &root)?.unwrap();
            Ok((alias, root, state))
        })
        .collect()
    }

    pub(crate) fn engine_root(
        &self,
        alias: &str,
        root: &str,
    ) -> Result<Option<crate::engine::transport::HostState>> {
        let json: Option<String> = self
            .conn
            .query_row(
                "SELECT state_json FROM engine_hosts WHERE alias = ?1 AND root = ?2",
                params![alias, root],
                |row| row.get(0),
            )
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    pub fn engine_host(
        &self,
        alias: &str,
        root: &str,
    ) -> Result<Option<crate::engine::transport::HostState>> {
        let Some(mut state) = self.engine_root(alias, root)? else {
            return Ok(None);
        };
        if let Some((since, cause)) = self.engine_host_auth(alias)?.blocked {
            state.status = crate::engine::transport::Status::NeedsAuth;
            state.since_ms = since;
            state.cause = Some(cause);
            state.next_retry_ms = None;
        }
        Ok(Some(state))
    }

    pub fn save_engine_host(
        &self,
        alias: &str,
        root: &str,
        state: &crate::engine::transport::HostState,
    ) -> Result<()> {
        use crate::engine::transport::Status;
        let blocked = state.status == Status::NeedsAuth;
        let json = serde_json::json!({
            "status": if blocked { &Status::Idle } else { &state.status },
            "since_ms": state.since_ms, "failures": state.failures,
            "last_live_ms": state.last_live_ms,
            "next_retry_ms": state.next_retry_ms,
            "cause": if blocked { None } else { state.cause.as_ref() },
        });
        self.conn.execute(
            "INSERT INTO engine_hosts (alias, root, state_json) VALUES (?1, ?2, ?3) ON CONFLICT(alias, root) DO UPDATE SET state_json = excluded.state_json",
            params![alias, root, json.to_string()],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn seed_engine_host(
        &self,
        alias: &str,
        root: &str,
        class: Option<crate::engine::transport::HostClass>,
        state: &crate::engine::transport::HostState,
    ) -> Result<()> {
        self.save_engine_host_auth(
            alias,
            &crate::engine::transport::AuthState {
                class,
                route: None,
                blocked: (state.status == crate::engine::transport::Status::NeedsAuth)
                    .then(|| (state.since_ms, state.cause.clone().unwrap_or_default())),
            },
        )?;
        self.save_engine_host(alias, root, state)
    }

    pub fn insert_submitting_run(&self, run: SubmittingRun<'_>) -> Result<()> {
        let backend_json = crate::engine::handle::backend_json(run.id, None, run.metadata)?;
        self.conn.execute(
            "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command,
                               created_at, updated_at, commit_sha, chat_session_id, spec_digest,
                               launch_spec)
             VALUES (?1, ?2, ?3, 'submitting', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run.id,
                run.experiment_id,
                run.project_id,
                backend_json,
                run.command,
                run.created_at,
                run.created_at,
                run.commit_sha,
                run.chat_session_id,
                run.spec_digest,
                serde_json::to_string(run.launch)?,
            ],
        )?;
        Ok(())
    }

    pub fn engine_run_state(&self, run_id: &str) -> Result<Option<EngineRunState>> {
        fn json<T: serde::de::DeserializeOwned>(value: Option<String>) -> Result<Option<T>> {
            Ok(value.map(|json| serde_json::from_str(&json)).transpose()?)
        }

        let row = self
            .conn
            .query_row(
                "SELECT contact, spec_digest, submission_attempt, submission_fence,
                        cancel_accepted_at, launch_spec, notify_cursor, cancel_unconfirmed, started_at
                 FROM runs WHERE id = ?1",
                [run_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, Option<i64>>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            contact,
            spec_digest,
            submission_attempt,
            fence,
            cancel_accepted_at,
            launch,
            notify,
            cancel_unconfirmed,
            started_at,
        )) = row
        else {
            return Ok(None);
        };
        Ok(Some(EngineRunState {
            started_at,
            contact: json(contact)?.unwrap_or(Contact::Live),
            cancel_unconfirmed: json(cancel_unconfirmed)?,
            spec_digest,
            submission_attempt,
            submission_fence: json(fence)?,
            cancel_accepted_at,
            launch: json(launch)?,
            notify_cursor: json(notify)?,
        }))
    }

    pub fn set_notify_cursor(&self, run_id: &str, cursor: &NotifyCursor) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET notify_cursor = ?2 WHERE id = ?1",
            params![run_id, serde_json::to_string(cursor)?],
        )?;
        Ok(())
    }

    /// Runner-1 runs an old supervisor holds, with the row as the engine last saw it. Durable,
    /// so a run that finishes while the engine is down still gets its chat result.
    pub fn legacy_watches(&self) -> Result<Vec<(String, Option<SeenRow>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, seen_status, seen_updated_at FROM legacy_watches ORDER BY run_id",
        )?;
        let rows = stmt.query_map([], |row| {
            let seen = match (row.get(1)?, row.get(2)?) {
                (Some(status), Some(updated_at)) => Some((status, updated_at)),
                _ => None,
            };
            Ok((row.get(0)?, seen))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// `None` starts watching without overwriting what was already seen.
    pub fn watch_legacy_run(&self, run_id: &str, seen: Option<(&str, i64)>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO legacy_watches (run_id, seen_status, seen_updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(run_id) DO UPDATE SET
               seen_status = COALESCE(excluded.seen_status, seen_status),
               seen_updated_at = COALESCE(excluded.seen_updated_at, seen_updated_at)",
            params![run_id, seen.map(|seen| seen.0), seen.map(|seen| seen.1)],
        )?;
        Ok(())
    }

    pub fn unwatch_legacy_run(&self, run_id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM legacy_watches WHERE run_id = ?1", [run_id])?;
        Ok(())
    }

    pub fn last_notice_at(&self, run_id: &str, kind: &str) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT MAX(created_at) FROM run_notices WHERE run_id = ?1 AND kind = ?2",
            params![run_id, kind],
            |row| row.get(0),
        )?)
    }

    pub fn set_run_contact(&self, run_id: &str, contact: &Contact) -> Result<bool> {
        let json = match contact {
            Contact::Live => None,
            _ => Some(serde_json::to_string(contact)?),
        };
        Ok(self.conn.execute(
            "UPDATE runs SET contact = ?2 WHERE id = ?1",
            params![run_id, json],
        )? == 1)
    }

    pub fn set_submission_fence(
        &self,
        run_id: &str,
        attempt: u32,
        fence: &SubmissionFence,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE runs SET submission_fence = ?3
             WHERE id = ?1 AND status = 'submitting' AND submission_attempt = ?2
               AND submission_fence IS NULL AND cancel_accepted_at IS NULL",
            params![run_id, attempt, serde_json::to_string(fence)?],
        )? == 1)
    }

    /// Release only when the launch was never sent.
    pub fn clear_submission_fence(&self, run_id: &str, attempt: u32) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE runs SET submission_fence = NULL
             WHERE id = ?1 AND status = 'submitting' AND submission_attempt = ?2",
            params![run_id, attempt],
        )? == 1)
    }

    /// Call only after recovery proves absence and the previous submitter is gone.
    pub fn advance_submission_attempt(&self, run_id: &str, attempt: u32) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE runs SET submission_attempt = submission_attempt + 1, submission_fence = NULL
             WHERE id = ?1 AND status = 'submitting' AND submission_attempt = ?2
               AND submission_attempt < ?3 AND cancel_accepted_at IS NULL",
            params![run_id, attempt, u32::MAX],
        )? == 1)
    }

    pub fn record_run_handle(
        &self,
        run_id: &str,
        attempt: u32,
        handle: &crate::engine::handle::Handle,
    ) -> Result<bool> {
        let tx = self
            .conn
            .is_autocommit()
            .then(|| self.begin_immediate())
            .transpose()?;
        let run = self
            .get_run(run_id)?
            .ok_or_else(|| anyhow!("Run {run_id} not found"))?;
        let metadata = crate::jobs::BackendDescriptor::parse(&run.backend_json)?;
        let json = crate::engine::handle::backend_json(run_id, Some(handle), &metadata)?;
        let changed = self.conn.execute(
            "UPDATE runs SET backend_json = ?3,
               status = CASE WHEN status = 'submitting' THEN 'starting' ELSE status END,
               updated_at = ?4
             WHERE id = ?1 AND submission_attempt = ?2 AND submission_fence IS NOT NULL
               AND json_extract(backend_json, '$.handle') IS NULL
               AND (status = 'submitting' OR
                 (status IN ('done','failed','cancelled','lost') AND contact IS NOT NULL))",
            params![run_id, attempt, json, now_ms()],
        )?;
        if changed == 1
            && crate::engine::Phase::parse(&run.status)
                .is_some_and(crate::engine::Phase::is_terminal)
        {
            self.queue_log_collection(run_id)?;
        }
        if let Some(tx) = tx {
            tx.commit()?;
        }
        Ok(changed == 1)
    }

    /// For a submitter subprocess of the fenced attempt, recorded before it can act.
    pub fn record_submitter(
        &self,
        run_id: &str,
        attempt: u32,
        fence: &SubmissionFence,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE runs SET submission_fence = ?3
             WHERE id = ?1 AND status = 'submitting' AND submission_attempt = ?2
               AND submission_fence IS NOT NULL",
            params![run_id, attempt, serde_json::to_string(fence)?],
        )? == 1)
    }

    /// Compare-and-set on `status`; the engine decides which moves are legal.
    pub fn set_engine_phase(
        &self,
        run_id: &str,
        from: &str,
        to: &str,
        ended_at: Option<i64>,
        exit_code: Option<i64>,
        started_at: Option<i64>,
    ) -> Result<bool> {
        let changed = self.conn.execute(
            "UPDATE runs SET status = ?3, updated_at = ?4, ended_at = COALESCE(?5, ended_at),
                             exit_code = COALESCE(?6, exit_code),
                             started_at = COALESCE(started_at, ?7)
             WHERE id = ?1 AND status = ?2",
            params![run_id, from, to, now_ms(), ended_at, exit_code, started_at],
        )? == 1;
        if let Some(status) = RunStatus::parse(to).filter(|status| changed && status.is_terminal())
        {
            self.stage_run_terminal(run_id, status)?;
        }
        Ok(changed)
    }

    pub fn supervisor_lock_paths(&self, run_id: &str) -> Result<[PathBuf; 2]> {
        Ok([
            self.run_lock_path(SUPERVISOR_LOCK, run_id)?,
            self.run_lock_path(SHARED_SSH_LOCK, run_id)?,
        ])
    }

    fn run_lock_path(&self, artifact: (&str, &str), run_id: &str) -> Result<PathBuf> {
        if run_id.is_empty()
            || !run_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(anyhow!("Invalid run id: {run_id}"));
        }
        Ok(self.run_artifact_path(artifact, run_id))
    }

    /// Ends an outage for a chat: withdraws its attention notice if it was never sent, and
    /// returns whether one was sent (or is in flight), so a recovery notice is owed.
    pub fn settle_outage_notice(&self, run_id: &str, chat: &str) -> Result<bool> {
        let dedupe = self.outage_dedupe(run_id, chat, ATTENTION)?;
        let withdrawn = self.conn.execute(
            "DELETE FROM run_notices WHERE run_id = ?1 AND chat_session_id = ?2 AND dedupe = ?3
               AND client_turn_id IS NULL",
            params![run_id, chat, dedupe],
        )?;
        // Retired attention keeps its audit row without reserving the next outage's key.
        self.conn.execute(
            "UPDATE run_notices SET dedupe = 'retired:' || client_turn_id || ':' || dedupe
             WHERE run_id = ?1 AND chat_session_id = ?2 AND dedupe = ?3 AND retired_at IS NOT NULL",
            params![run_id, chat, dedupe],
        )?;
        Ok(withdrawn == 0
            && self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM run_notices
                   WHERE run_id = ?1 AND chat_session_id = ?2 AND dedupe = ?3)",
                params![run_id, chat, dedupe],
                |row| row.get(0),
            )?)
    }

    /// `None` accepts legacy intent; a reason accepts an API cancel, including during submission.
    pub fn accept_run_cancel(&self, run_id: &str, reason: Option<&str>) -> Result<Option<i64>> {
        let path = self.run_lock_path(CANCEL_LOCK, run_id)?;
        let mut lock = open_lifecycle_lock_at(&path)?;
        let _guard = lock.write()?;
        Ok(self.conn.query_row(
            "UPDATE runs SET cancel_accepted_at = COALESCE(cancel_accepted_at, ?2),
                             updated_at = CASE WHEN cancel_accepted_at IS NULL THEN ?2
                                               ELSE updated_at END,
                             cancel_reason = CASE WHEN cancel_requested = 0 AND cancel_accepted_at IS NULL
                                                  THEN COALESCE(?3, cancel_reason)
                                                  ELSE cancel_reason END
             WHERE id = ?1 AND status IN ('submitting', 'starting', 'running')
               AND (cancel_requested = 1 OR cancel_accepted_at IS NOT NULL OR ?3 IS NOT NULL)
             RETURNING cancel_accepted_at",
            params![run_id, now_ms(), reason],
            |row| row.get(0),
        ).optional()?)
    }

    pub fn subscribe_run(
        &self,
        run_id: &str,
        chat_session_id: &str,
        created_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO run_subscriptions (run_id, chat_session_id, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(run_id, chat_session_id) DO NOTHING",
            params![run_id, chat_session_id, created_at],
        )?;
        Ok(())
    }

    pub fn run_subscribers(&self, run_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT chat_session_id FROM runs WHERE id = ?1 AND chat_session_id IS NOT NULL
             UNION SELECT chat_session_id FROM run_subscriptions WHERE run_id = ?1
             ORDER BY chat_session_id",
        )?;
        let rows = stmt.query_map([run_id], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn insert_run_notice(
        &self,
        run_id: &str,
        chat_session_id: &str,
        kind: &str,
        dedupe: &str,
        body: &str,
        created_at: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "INSERT INTO run_notices (run_id, chat_session_id, kind, dedupe, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(run_id, chat_session_id, dedupe) DO NOTHING",
            params![run_id, chat_session_id, kind, dedupe, body, created_at],
        )? == 1)
    }

    pub(crate) fn queue_cancel_settled_notice(
        &self,
        run_id: &str,
        chat: &str,
        now: i64,
    ) -> Result<bool> {
        let notified: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM run_notices WHERE run_id = ?1 AND chat_session_id = ?2
             AND kind = ?3)",
            params![run_id, chat, TERMINAL],
            |row| row.get(0),
        )?;
        if !notified {
            return Ok(false);
        }
        let run = self
            .get_run(run_id)?
            .ok_or_else(|| anyhow!("Run {run_id} not found"))?;
        let view = crate::plane::Run::from_store(self, &run)?;
        let status = view.display_status();
        let detail = if view.cancel_unconfirmed.is_some() {
            run.result_markdown.as_deref().unwrap_or_default()
        } else {
            "All remaining processes have stopped."
        };
        self.insert_run_notice(
            run_id,
            chat,
            ATTENTION,
            "cancel-stopped",
            &format!("[orx] Run `{run_id}`: {status}. {detail}"),
            now,
        )
    }

    pub fn pending_run_notice_sessions(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT chat_session_id FROM run_notices
             WHERE delivered_at IS NULL AND retired_at IS NULL
             AND (next_delivery_at IS NULL OR next_delivery_at <= ?1) ORDER BY chat_session_id",
        )?;
        let rows = stmt.query_map([now_ms()], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn record_run_notice_failure(
        &self,
        chat: &str,
        client_turn_id: &str,
        reason: &str,
        permanent: bool,
        now: i64,
    ) -> Result<Option<i64>> {
        let tx = self.begin_immediate()?;
        let prior: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT delivery_failures, first_failure_at FROM run_notices
             WHERE chat_session_id = ?1 AND client_turn_id = ?2
             AND delivered_at IS NULL AND retired_at IS NULL LIMIT 1",
                params![chat, client_turn_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((failures, first)) = prior else {
            tx.commit()?;
            return Ok(None);
        };
        let first = first.unwrap_or(now);
        let deadline = first.saturating_add(RUN_NOTICE_RETRY_WINDOW_MS);
        let retired = (permanent || now >= deadline).then_some(now);
        let delay_minutes = match failures {
            0 => 1,
            1 => 5,
            2 => 15,
            _ => 60,
        };
        let next = retired
            .is_none()
            .then(|| now.saturating_add(delay_minutes * 60 * 1000).min(deadline));
        let reason = if !permanent && now >= deadline {
            format!("Retry window expired after 24 hours: {reason}")
        } else {
            reason.to_string()
        };
        tx.execute(
            "UPDATE run_notices SET delivery_failures = ?3, first_failure_at = ?4,
             next_delivery_at = ?5, delivery_error = ?6, retired_at = ?7
             WHERE chat_session_id = ?1 AND client_turn_id = ?2 AND delivered_at IS NULL AND retired_at IS NULL",
            params![chat, client_turn_id, failures + 1, first, next, reason, retired],
        )?;
        tx.commit()?;
        Ok(Some(failures + 1))
    }

    /// Why the run's latest retired notice batch never reached its chat.
    pub(crate) fn run_notice_error(&self, run_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT delivery_error FROM run_notices WHERE run_id = ?1 AND retired_at IS NOT NULL
                 ORDER BY retired_at DESC, id DESC LIMIT 1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten())
    }

    pub fn expire_run_notice_batches(&self, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE run_notices SET retired_at = ?1, next_delivery_at = NULL,
             delivery_error = 'Retry window expired after 24 hours: ' || delivery_error
             WHERE delivered_at IS NULL AND retired_at IS NULL
             AND first_failure_at <= ?1 - ?2",
            params![now, RUN_NOTICE_RETRY_WINDOW_MS],
        )?;
        Ok(())
    }

    pub fn run_notice_batch(&self, chat_session_id: &str) -> Result<Vec<RunNotice>> {
        self.run_notice_batch_at(chat_session_id, now_ms())
    }

    pub fn run_notice_batch_at(&self, chat_session_id: &str, now: i64) -> Result<Vec<RunNotice>> {
        let tx = self.begin_immediate()?;
        self.expire_run_notice_batches(now)?;
        let assigned: Option<String> = tx
            .query_row(
                "SELECT client_turn_id FROM run_notices
             WHERE chat_session_id = ?1 AND delivered_at IS NULL AND client_turn_id IS NOT NULL
             AND retired_at IS NULL AND (next_delivery_at IS NULL OR next_delivery_at <= ?2)
             ORDER BY id LIMIT 1",
                params![chat_session_id, now],
                |row| row.get(0),
            )
            .optional()?;
        let client_turn_id = match assigned {
            Some(id) => id,
            None => {
                let id = format!("run-notice:{}", uuid::Uuid::new_v4());
                let assigned = tx.execute(
                    "UPDATE run_notices SET client_turn_id = ?2 WHERE chat_session_id = ?1
                     AND delivered_at IS NULL AND retired_at IS NULL AND client_turn_id IS NULL",
                    params![chat_session_id, id],
                )?;
                if assigned == 0 {
                    tx.commit()?;
                    return Ok(Vec::new());
                }
                id
            }
        };
        let notices = {
            let mut stmt = tx.prepare(
                "SELECT id, run_id, chat_session_id, kind, dedupe, body, created_at, client_turn_id
                 FROM run_notices WHERE chat_session_id = ?1 AND client_turn_id = ?2 AND delivered_at IS NULL ORDER BY id",
            )?;
            let rows = stmt.query_map(params![chat_session_id, client_turn_id], |row| {
                Ok(RunNotice {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    chat_session_id: row.get(2)?,
                    kind: row.get(3)?,
                    dedupe: row.get(4)?,
                    body: row.get(5)?,
                    created_at: row.get(6)?,
                    client_turn_id: row.get(7)?,
                })
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        tx.commit()?;
        Ok(notices)
    }

    pub fn mark_run_notice_batch_delivered(
        &self,
        chat_session_id: &str,
        client_turn_id: &str,
        delivered_at: i64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE run_notices SET delivered_at = ?3, next_delivery_at = NULL WHERE chat_session_id = ?1
             AND client_turn_id = ?2 AND delivered_at IS NULL",
            params![chat_session_id, client_turn_id, delivered_at],
        )?;
        Ok(())
    }

    /// Run under the engine lock after legacy delivery stops and before notice production.
    pub fn migrate_run_wakeups(&self) -> Result<()> {
        let tx = self.begin_immediate()?;
        let wakeups = {
            let mut stmt = tx.prepare(
                "SELECT run_id, chat_session_id, state, requested_at, delivered_at, monitoring_alerted
                 FROM chat_run_wakeups WHERE engine_migrated = 0 ORDER BY run_id, chat_session_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, bool>(5)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        for (run_id, chat, state, requested_at, delivered_at, alerted) in wakeups {
            let Some(run) = self.get_run(&run_id)? else {
                continue;
            };
            if matches!(state.as_str(), "pending" | "claimed") {
                if !is_terminal_status(&run.status) {
                    self.subscribe_run(&run_id, &chat, requested_at)?;
                } else if matches!(run.status.as_str(), "done" | "failed" | "lost") {
                    let body = self.terminal_notice_body(&run)?;
                    self.insert_run_notice(
                        &run_id,
                        &chat,
                        TERMINAL,
                        TERMINAL,
                        &body,
                        requested_at,
                    )?;
                }
            } else if state == "delivered" {
                self.insert_run_notice(&run_id, &chat, TERMINAL, TERMINAL, "", requested_at)?;
                tx.execute(
                    "UPDATE run_notices SET delivered_at = ?4 WHERE run_id = ?1 AND chat_session_id = ?2 AND dedupe = ?3",
                    params![run_id, chat, TERMINAL, delivered_at.unwrap_or(requested_at)],
                )?;
            }
            if alerted {
                let dedupe = self.outage_dedupe(&run_id, &chat, ATTENTION)?;
                self.insert_run_notice(&run_id, &chat, ATTENTION, &dedupe, "", requested_at)?;
                tx.execute(
                    "UPDATE run_notices SET delivered_at = ?4 WHERE run_id = ?1 AND chat_session_id = ?2 AND dedupe = ?3",
                    params![run_id, chat, dedupe, delivered_at.unwrap_or(requested_at)],
                )?;
            }
            tx.execute(
                "UPDATE chat_run_wakeups SET engine_migrated = 1
                 WHERE run_id = ?1 AND chat_session_id = ?2",
                params![run_id, chat],
            )?;
        }
        tx.pragma_update(None, "user_version", 2)?;
        tx.commit()?;
        Ok(())
    }

    /// Recovery notices advance the outage generation, including outages begun by 0.2.x.
    pub fn outage_dedupe(&self, run_id: &str, chat: &str, kind: &str) -> Result<String> {
        let generation: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM run_notices WHERE run_id = ?1 AND chat_session_id = ?2 AND kind = ?3",
            params![run_id, chat, RECOVERED], |row| row.get(0),
        )?;
        Ok(format!("{kind}:{generation}"))
    }

    pub fn terminal_notice_body(&self, run: &StoredRun) -> Result<String> {
        use std::io::{Read, Seek, SeekFrom};

        let mut bytes = Vec::new();
        let ranges = crate::commands::up::api_v2::kept_ranges(&self.data_dir, &run.id)?;
        let tail = self.run_artifact_path(TAIL_LOG, &run.id);
        let current = if let Some(range) = ranges.iter().max_by_key(|range| range.offset) {
            let file = std::fs::File::open(&range.file)?;
            Ok((file, range.file.clone(), range.len))
        } else if tail.exists() {
            let file = std::fs::File::open(&tail)?;
            let size = file.metadata()?.len();
            Ok((file, tail, size))
        } else {
            crate::jobs::ssh::service::logs::open_current(
                self.run_artifact_path(("run-logs", ".log"), &run.id),
            )
        };
        match current {
            Ok((mut file, _, size)) => {
                let start = size.saturating_sub(TERMINAL_LOG_BYTES);
                file.seek(SeekFrom::Start(start))?;
                bytes.resize(usize::try_from(size - start)?, 0);
                file.read_exact(&mut bytes)?;
            }
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
            Err(error) => return Err(error),
        }
        let first_character = bytes
            .iter()
            .position(|byte| byte & 0xc0 != 0x80)
            .unwrap_or(bytes.len());
        let log = String::from_utf8_lossy(&bytes[first_character..]);
        let mut start = log
            .len()
            .saturating_sub(usize::try_from(TERMINAL_LOG_BYTES)?);
        while !log.is_char_boundary(start) {
            start += 1;
        }
        let mut lines: Vec<_> = log[start..]
            .lines()
            .rev()
            .take(TERMINAL_LOG_LINES)
            .collect();
        lines.reverse();
        let tail = lines.join("\n");
        let code = run
            .exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unavailable".into());
        let duration = run
            .ended_at
            .map(|ended| format!("{} ms", ended.saturating_sub(run.created_at).max(0)))
            .unwrap_or_else(|| "unavailable".into());
        let cancel_reason = run
            .cancel_reason
            .as_deref()
            .filter(|_| run.status == "cancelled");
        let reason = match (cancel_reason, run.result_markdown.as_deref()) {
            (Some(cancel), Some(result)) => format!("{cancel} {result}"),
            (cancel, result) => cancel.or(result).unwrap_or("unavailable").into(),
        };
        let view = crate::plane::Run::from_store(self, run)?;
        let status = view.display_status();
        Ok(format!(
            "[orx] Run `{}` finished with status **{}**.\nExit code: {code}\nDuration: {duration}\nReason: {reason}\n\nRecent log:\n{tail}",
            run.id, status,
        ))
    }
}

const SELECT_RUN: &str = "SELECT id, experiment_id, project_id, status, backend_json, command,
                                 created_at, updated_at, ended_at, exit_code,
                                 commit_sha, result_markdown, cancel_requested,
                                 chat_session_id, cancel_reason FROM runs";

const PROJECT_COLS: &str = "id, name, slug, github_owner, github_repo, github_sync_enabled, \
                            baseline_branch, repo_path, run_command, paper_id, created_at, updated_at";

const EXPERIMENT_COLS: &str = "id, project_id, parent_experiment_id, slug, branch_name, \
                               title, description, run_command, agent_status, created_at, \
                               updated_at, chat_session_id, archived";

fn row_to_run(row: &rusqlite::Row<'_>) -> std::result::Result<StoredRun, rusqlite::Error> {
    row_to_run_at(row, 0)
}

/// A [`SELECT_RUN`] row whose columns start at `first`.
fn row_to_run_at(
    row: &rusqlite::Row<'_>,
    first: usize,
) -> std::result::Result<StoredRun, rusqlite::Error> {
    Ok(StoredRun {
        id: row.get(first)?,
        experiment_id: row.get(first + 1)?,
        project_id: row.get(first + 2)?,
        status: row.get(first + 3)?,
        backend_json: row.get(first + 4)?,
        command: row.get(first + 5)?,
        created_at: row.get(first + 6)?,
        updated_at: row.get(first + 7)?,
        ended_at: row.get(first + 8)?,
        exit_code: row.get(first + 9)?,
        commit_sha: row.get(first + 10)?,
        result_markdown: row.get(first + 11)?,
        cancel_requested: row.get(first + 12)?,
        chat_session_id: row.get(first + 13)?,
        cancel_reason: row.get(first + 14)?,
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn log_generation_run_id(name: &str) -> Option<&str> {
    let (run, generation) = name.strip_suffix(".log")?.split_once('.')?;
    uuid::Uuid::parse_str(generation).ok()?;
    Some(run)
}

fn artifact_entries(path: &Path) -> Result<Vec<std::fs::DirEntry>> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(entries.collect::<std::io::Result<Vec<_>>>()?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn remove_run_artifact(path: &Path) -> Result<()> {
    let result = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow!(
            "Could not remove run artifact {}: {error}",
            path.display()
        )),
    }
}

/// Resolves `parts` under `root` through real directories only, so symlinks cannot redirect removal.
fn real_path_under<'a>(
    root: &Path,
    parts: impl IntoIterator<Item = &'a str>,
) -> Result<Option<PathBuf>> {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_reader_version_refuses_before_migrations_on_every_open() {
        let dir = std::env::temp_dir().join(format!("orx-reader-floor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("orx.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE store_metadata (key TEXT PRIMARY KEY, value INTEGER NOT NULL);
            INSERT INTO store_metadata VALUES ('min_reader_version', 2);",
        )
        .unwrap();
        let error = Store::open_at(dir.clone()).err().unwrap();
        assert!(error.to_string().contains("min_reader_version 2"));
        let migrated: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'runs')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!migrated);
        conn.execute("UPDATE store_metadata SET value = 1", [])
            .unwrap();
        drop(Store::open_at(dir.clone()).unwrap());
        conn.execute("UPDATE store_metadata SET value = 2", [])
            .unwrap();
        assert!(Store::open_at(dir.clone()).is_err());
        drop(conn);
        std::fs::remove_dir_all(dir).unwrap();
    }

    // Pinned from 0.2.22 main: store.rs, commands/{supervise,up}.rs and jobs/ssh/service/mod.rs.
    const LEGACY_SELECT_RUN: &str =
        "SELECT id, experiment_id, project_id, status, backend_json, command,
                                 created_at, updated_at, ended_at, exit_code,
                                 commit_sha, result_markdown, cancel_requested,
                                 chat_session_id, cancel_reason FROM runs";
    const LEGACY_ACTIVE_RUNS: &str =
        "SELECT id, experiment_id, project_id, status, backend_json, command,
                                 created_at, updated_at, ended_at, exit_code,
                                 commit_sha, result_markdown, cancel_requested,
                                 chat_session_id, cancel_reason FROM runs
         WHERE status IN ('starting', 'running') ORDER BY created_at DESC";
    const LEGACY_REQUEST_CANCEL: &str =
        "UPDATE runs SET cancel_requested = 1, cancel_reason = ?2, updated_at = ?3
             WHERE id = ?1 AND cancel_requested = 0 AND status IN ('starting', 'running')";
    const LEGACY_WITHDRAW_CANCEL: &str =
        "UPDATE runs SET cancel_requested = 0, updated_at = ?2 WHERE id = ?1";
    const LEGACY_SUPERVISE_DISPATCH: &[&str] = &[
        "k8s_job",
        "modal_job",
        "ssh_job",
        "slurm_job",
        "ray_job",
        "openresearch_job",
        "local_job",
        "tinker_job",
    ];
    const LEGACY_SUPERVISE_FALLTHROUGH: &str = "hf_job";
    const LEGACY_SSH_SERVICE_SCAN: fn(&str, bool, bool) -> bool =
        |kind, supervisor_running, uses_shared_ssh| {
            matches!(kind, "ssh_job" | "slurm_job" | "openresearch_job")
                && supervisor_running
                && !uses_shared_ssh
        };
    const LEGACY_SUPERVISOR_LOCK: (&str, &str) = ("run-logs", ".supervisor.lock");
    const LEGACY_ADOPTION: fn(bool, bool, bool, bool) -> bool =
        |submitted, settled, local_experiment, supervisor_running| {
            submitted && settled && local_experiment && !supervisor_running
        };

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LegacyDescriptor {
        kind: String,
        job_id: Option<String>,
        source_digest: Option<String>,
        source_path: Option<String>,
        source_size: Option<u64>,
    }

    fn legacy_rows(conn: &Connection, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
        conn.prepare(sql)
            .unwrap()
            .query_map([], |row| {
                (0..row.as_ref().column_count())
                    .map(|i| row.get(i))
                    .collect()
            })
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap()
    }

    fn legacy_active_runs(conn: &Connection) -> Vec<(String, LegacyDescriptor)> {
        conn.prepare(LEGACY_ACTIVE_RUNS)
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(4)?))
            })
            .unwrap()
            .map(|row| {
                let (id, json) = row.unwrap();
                (id, serde_json::from_str(&json).unwrap())
            })
            .collect()
    }

    fn test_launch() -> LaunchSpec {
        LaunchSpec {
            kind: "ssh_job".into(),
            target: Some("lab".into()),
            options: serde_json::json!({}),
        }
    }

    fn engine_fixture() -> (PathBuf, Store) {
        let dir = std::env::temp_dir().join(format!("orx-engine-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = Connection::open(dir.join("orx.db")).unwrap();
        conn.execute_batch(include_str!("../tests/fixtures/store-0.2.22.sql"))
            .unwrap();
        assert!(conn.prepare("SELECT contact FROM runs").is_err());
        drop(conn);
        let store = Store::open_at(dir.clone()).unwrap();
        (dir, store)
    }

    #[test]
    fn engine_migration_enqueues_every_finished_pending_or_claimed_result_except_cancelled() {
        let (dir, store) = engine_fixture();
        for phase in ["done", "failed", "lost", "cancelled"] {
            for state in ["pending", "claimed"] {
                let id = format!("{phase}_{state}");
                let mut run = run_fixture(&id, phase, Some("owner"));
                run.ended_at = Some(101);
                run.exit_code = Some(7);
                run.result_markdown = Some("payload ended".into());
                store.upsert_run(&run).unwrap();
                store.register_run_wakeup(&id, "foreign").unwrap();
                if state == "claimed" {
                    store.claim_run_wakeup(&id, "foreign").unwrap().unwrap();
                }
                std::fs::write(dir.join(format!("run-logs/{id}.log")), "result line\n").unwrap();
            }
        }
        store.migrate_run_wakeups().unwrap();
        for phase in ["done", "failed", "lost", "cancelled"] {
            for state in ["pending", "claimed"] {
                let id = format!("{phase}_{state}");
                assert_eq!(
                    store
                        .conn
                        .query_row(
                            "SELECT COUNT(*) FROM run_subscriptions WHERE run_id = ?1",
                            [&id],
                            |row| row.get::<_, i64>(0)
                        )
                        .unwrap(),
                    0
                );
                let notice: Option<(String, String, Option<i64>)> = store
                    .conn
                    .query_row(
                        "SELECT body, dedupe, delivered_at FROM run_notices WHERE run_id = ?1",
                        [&id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                    .optional()
                    .unwrap();
                if phase == "cancelled" {
                    assert!(notice.is_none());
                    continue;
                }
                let (body, dedupe, delivered) = notice.unwrap();
                assert_eq!(
                    body,
                    store
                        .terminal_notice_body(&store.get_run(&id).unwrap().unwrap())
                        .unwrap()
                );
                assert!(body.contains(&format!("**{phase}**")));
                assert!(body.contains("Exit code: 7\nDuration: 100 ms\nReason: payload ended"));
                assert!(body.ends_with("Recent log:\nresult line"));
                assert_eq!(dedupe, TERMINAL);
                assert!(delivered.is_none());
            }
        }
        let frozen = store.run_notice_batch("foreign").unwrap();
        assert_eq!(frozen.len(), 7);
        store.migrate_run_wakeups().unwrap();
        assert_eq!(store.run_notice_batch("foreign").unwrap(), frozen);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn terminal_notice_of_a_cancelled_run_gives_the_recorded_cancel_reason() {
        let (dir, store) = engine_fixture();
        let mut run = run_fixture("cancelled", "cancelled", None);
        run.cancel_reason = Some("Cancel requested with `orx exp cancel`.".into());
        let body = store.terminal_notice_body(&run).unwrap();
        assert!(body.contains("\nReason: Cancel requested with `orx exp cancel`.\n"));
        run.result_markdown = Some("Killed by signal 15.".into());
        let body = store.terminal_notice_body(&run).unwrap();
        assert!(body
            .contains("\nReason: Cancel requested with `orx exp cancel`. Killed by signal 15.\n"));
        run.status = "done".into();
        let body = store.terminal_notice_body(&run).unwrap();
        assert!(body.contains("\nReason: Killed by signal 15.\n"));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_terminal_renderer_bounds_log_bytes_and_lines_without_splitting_utf8() {
        let (dir, store) = engine_fixture();
        let run = store.get_run("failed").unwrap().unwrap();
        let path = dir.join("run-logs/failed.log");
        let lines: String = (0..100).map(|i| format!("line-{i:03}\n")).collect();
        std::fs::write(&path, &lines).unwrap();
        let body = store.terminal_notice_body(&run).unwrap();
        let tail = body.split("Recent log:\n").nth(1).unwrap();
        assert_eq!(tail.lines().count(), 40);
        assert!(tail.starts_with("line-060\n"));
        assert!(tail.ends_with("line-099"));
        std::fs::write(&path, format!("{}a", "🦀".repeat(1500))).unwrap();
        let body = store.terminal_notice_body(&run).unwrap();
        let tail = body.split("Recent log:\n").nth(1).unwrap();
        assert!(tail.len() <= 4096);
        assert!(!tail.contains('\u{fffd}'));
        assert!(tail.ends_with('a'));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_wakeup_migration_imports_new_legacy_rows_after_downgrade() {
        let (dir, store) = engine_fixture();
        store.migrate_run_wakeups().unwrap();
        let delivered = store.run_notice_batch("foreign").unwrap();
        assert_eq!(delivered.len(), 1);
        store
            .mark_run_notice_batch_delivered("foreign", &delivered[0].client_turn_id, 600)
            .unwrap();
        drop(store);

        let legacy = Connection::open(dir.join("orx.db")).unwrap();
        legacy
            .execute_batch(
                "INSERT INTO chat_run_wakeups (run_id, chat_session_id, requested_at)
                 VALUES ('slurm_job', 'foreign', 700), ('lost', 'foreign', 701);
                 INSERT INTO chat_run_wakeups
                     (run_id, chat_session_id, requested_at, state, delivered_at)
                 VALUES ('done', 'foreign', 702, 'delivered', 703);",
            )
            .unwrap();
        drop(legacy);

        let store = Store::open_at(dir.clone()).unwrap();
        store.migrate_run_wakeups().unwrap();
        assert!(store
            .run_subscribers("slurm_job")
            .unwrap()
            .iter()
            .any(|chat| chat == "foreign"));
        let notices = store.run_notice_batch("foreign").unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].run_id, "lost");
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT delivered_at FROM run_notices
                     WHERE run_id = 'failed' AND chat_session_id = 'foreign'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            600
        );
        store.migrate_run_wakeups().unwrap();
        assert_eq!(store.run_notice_batch("foreign").unwrap(), notices);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn terminal_notice_prefers_kept_ranges_to_the_published_legacy_log_generation() {
        let (dir, store) = engine_fixture();
        let run = store.get_run("failed").unwrap().unwrap();
        let path = dir.join("run-logs/failed.log");
        std::fs::write(&path, "obsolete generation\n").unwrap();
        let generation = uuid::Uuid::new_v4();
        let published = "UPGRADE_BEGIN\nFINAL_METRIC=0.862\nUPGRADE_END\n";
        std::fs::write(
            path.with_extension(format!("{generation}.log")),
            format!("{published}unpublished bytes\n"),
        )
        .unwrap();
        std::fs::write(
            path.with_extension("cursor.json"),
            serde_json::to_vec(&serde_json::json!({
                "generation": generation.to_string(),
                "identity": "legacy-log",
                "boundary": "boundary",
                "bytes": published.len(),
            }))
            .unwrap(),
        )
        .unwrap();
        let body = store.terminal_notice_body(&run).unwrap();
        assert_eq!(
            body.split_once("Recent log:\n").unwrap().1,
            published.trim_end()
        );
        let collected = "collected after adoption\nUPGRADE_END\n";
        std::fs::write(&path, format!("{collected}unpublished bytes\n")).unwrap();
        let manifest = path.with_extension("ranges.json");
        std::fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!([{
                "offset": 0,
                "len": collected.len(),
                "file": &path,
                "ident": "1-2",
                "eof": true,
            }]))
            .unwrap(),
        )
        .unwrap();
        let body = store.terminal_notice_body(&run).unwrap();
        assert_eq!(
            body.split_once("Recent log:\n").unwrap().1,
            collected.trim_end()
        );
        std::fs::write(&path, "truncated").unwrap();
        assert!(store.terminal_notice_body(&run).is_err());
        std::fs::write(&path, collected).unwrap();
        std::fs::remove_file(manifest).unwrap();
        let engine_cursor = path.with_extension("engine-cursor.json");
        std::fs::write(
            &engine_cursor,
            serde_json::to_vec(&serde_json::json!({"offset": collected.len()})).unwrap(),
        )
        .unwrap();
        let body = store.terminal_notice_body(&run).unwrap();
        assert_eq!(
            body.split_once("Recent log:\n").unwrap().1,
            collected.trim_end()
        );
        std::fs::remove_file(engine_cursor).unwrap();
        std::fs::write(dir.join("run-logs/failed.tail.log"), "retained tail\n").unwrap();
        let body = store.terminal_notice_body(&run).unwrap();
        assert!(body.ends_with("Recent log:\nretained tail"));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_wakeup_migration_rolls_back_on_an_unreadable_completion_log() {
        let (dir, store) = engine_fixture();
        let path = dir.join("run-logs/failed.log");
        std::fs::create_dir(&path).unwrap();
        assert!(store.migrate_run_wakeups().is_err());
        for table in ["run_notices", "run_subscriptions"] {
            assert_eq!(
                store
                    .conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM chat_run_wakeups WHERE engine_migrated != 0",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        std::fs::remove_dir(&path).unwrap();
        store.migrate_run_wakeups().unwrap();
        assert_eq!(store.run_notice_batch("foreign").unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_absent_recovery_advances_and_refences_only_the_current_uncancelled_attempt() {
        let (dir, store) = engine_fixture();
        let metadata = crate::jobs::BackendDescriptor::parse(r#"{"kind":"hf_job"}"#).unwrap();
        store
            .insert_submitting_run(SubmittingRun {
                id: "retry",
                experiment_id: "exp_1",
                project_id: "proj_1",
                metadata: &metadata,
                command: "train",
                created_at: 1,
                commit_sha: None,
                chat_session_id: None,
                spec_digest: "digest",
                launch: &test_launch(),
            })
            .unwrap();
        let fence = SubmissionFence {
            taken_at: 1,
            submitter: Some(SubmitterIdentity {
                pid: 4,
                starttime: 5,
            }),
        };
        assert!(store.set_submission_fence("retry", 1, &fence).unwrap());
        assert!(store.advance_submission_attempt("retry", 1).unwrap());
        let state = store.engine_run_state("retry").unwrap().unwrap();
        assert_eq!(state.submission_attempt, 2);
        assert!(state.submission_fence.is_none());
        assert!(!store.advance_submission_attempt("retry", 1).unwrap());
        assert!(!store.set_submission_fence("retry", 1, &fence).unwrap());
        assert!(store.set_submission_fence("retry", 2, &fence).unwrap());
        let handle = crate::engine::adapters::api::test_handle(
            "hf_job",
            Some("lab"),
            None,
            vec!["job".into()],
        );
        assert!(!store.record_run_handle("retry", 1, &handle).unwrap());
        assert!(!store.advance_submission_attempt("ssh_job", 1).unwrap());
        assert!(!store.advance_submission_attempt("done", 1).unwrap());
        store
            .conn
            .execute(
                "UPDATE runs SET submission_attempt = ?1 WHERE id = 'retry'",
                [u32::MAX],
            )
            .unwrap();
        assert!(!store.advance_submission_attempt("retry", u32::MAX).unwrap());
        store
            .conn
            .execute(
                "UPDATE runs SET submission_attempt = 2 WHERE id = 'retry'",
                [],
            )
            .unwrap();
        store
            .accept_run_cancel("retry", Some("cancel"))
            .unwrap()
            .unwrap();
        assert!(!store.advance_submission_attempt("retry", 2).unwrap());
        assert_eq!(
            store
                .engine_run_state("retry")
                .unwrap()
                .unwrap()
                .submission_fence,
            Some(fence)
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_run_and_chat_deletion_remove_only_their_outbox_rows_transactionally() {
        let (dir, store) = engine_fixture();
        for (run, chat) in [("done", "owner"), ("done", "foreign"), ("failed", "owner")] {
            store.subscribe_run(run, chat, 1).unwrap();
            store
                .insert_run_notice(run, chat, TERMINAL, TERMINAL, "result", 1)
                .unwrap();
        }
        let tx = store.begin().unwrap();
        tx.execute("DELETE FROM runs WHERE id = 'done'", [])
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM run_notices", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        tx.rollback().unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM run_notices", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        store.delete_chat_session("foreign").unwrap();
        for table in ["run_notices", "run_subscriptions"] {
            assert_eq!(
                store
                    .conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        store
            .conn
            .execute("DELETE FROM runs WHERE id = 'done'", [])
            .unwrap();
        for table in ["run_notices", "run_subscriptions"] {
            let rows: Vec<(String, String)> = store
                .conn
                .prepare(&format!("SELECT run_id, chat_session_id FROM {table}"))
                .unwrap()
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<std::result::Result<_, _>>()
                .unwrap();
            assert_eq!(rows, [("failed".into(), "owner".into())]);
        }
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_schema_preserves_022_runs_and_legacy_readers_and_cleanup() {
        let old = Connection::open_in_memory().unwrap();
        old.execute_batch(include_str!("../tests/fixtures/store-0.2.22.sql"))
            .unwrap();
        assert!(old.prepare("SELECT started_at FROM runs").is_err());
        let before = legacy_rows(&old, &format!("{LEGACY_SELECT_RUN} ORDER BY id"));
        let (dir, store) = engine_fixture();
        let after = legacy_rows(&store.conn, &format!("{LEGACY_SELECT_RUN} ORDER BY id"));
        assert_eq!(before, after);
        assert_eq!(store.count_active_runs().unwrap(), 9);
        assert!(LEGACY_SSH_SERVICE_SCAN("ssh_job", true, false));
        assert!(!LEGACY_SSH_SERVICE_SCAN("ssh_job", true, true));
        let active = legacy_active_runs(&store.conn);
        assert_eq!(active.len(), 9);
        assert_eq!(active.first().unwrap().0, "openresearch_job");
        for run in store.list_runs(100).unwrap() {
            assert_eq!(
                store.engine_run_state(&run.id).unwrap().unwrap(),
                EngineRunState {
                    started_at: None,
                    contact: Contact::Live,
                    cancel_unconfirmed: None,
                    spec_digest: None,
                    submission_attempt: 1,
                    submission_fence: None,
                    cancel_accepted_at: None,
                    launch: None,
                    notify_cursor: None,
                }
            );
        }
        let snapshots = dir.join("source-snapshots");
        std::fs::create_dir_all(&snapshots).unwrap();
        for (_, descriptor) in active {
            assert!(
                LEGACY_SUPERVISE_DISPATCH.contains(&descriptor.kind.as_str())
                    || descriptor.kind == LEGACY_SUPERVISE_FALLTHROUGH
            );
            assert!(descriptor.job_id.is_some());
            let digest = descriptor.source_digest.unwrap();
            for extension in ["tar", "hf", "zip"] {
                std::fs::write(snapshots.join(format!("{digest}.{extension}")), "source").unwrap();
            }
        }
        let orphan = snapshots.join(format!("{}.tar", "f".repeat(64)));
        std::fs::write(&orphan, "unreferenced").unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(!orphan.exists());
        assert_eq!(std::fs::read_dir(&snapshots).unwrap().count(), 27);
        assert_eq!(
            store
                .conn
                .execute(
                    LEGACY_REQUEST_CANCEL,
                    params!["slurm_job", "old CLI", now_ms()]
                )
                .unwrap(),
            1
        );
        store
            .conn
            .execute(LEGACY_WITHDRAW_CANCEL, params!["slurm_job", now_ms()])
            .unwrap();
        assert!(
            !store
                .get_run("slurm_job")
                .unwrap()
                .unwrap()
                .cancel_requested
        );
        assert_eq!(
            store.get_run("slurm_job").unwrap().unwrap().status,
            "starting"
        );
        assert!(!store.request_cancel("done", "old CLI").unwrap());
        drop(store);
        let reopened = Store::open_at(dir.clone()).unwrap();
        assert_eq!(legacy_active_runs(&reopened.conn).len(), 9);
        assert_eq!(
            reopened
                .conn
                .query_row("SELECT COUNT(*) FROM run_notices", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_submitting_is_hidden_and_owned_handles_skip_legacy_adoption() {
        use crate::engine::handle::Handle;
        let (dir, store) = engine_fixture();
        let mut run = run_fixture("engine_run", "submitting", Some("owner"));
        run.backend_json = store.get_run("ssh_job").unwrap().unwrap().backend_json;
        store
            .insert_submitting_run(SubmittingRun {
                id: &run.id,
                experiment_id: &run.experiment_id,
                project_id: &run.project_id,
                metadata: &crate::jobs::BackendDescriptor::parse(&run.backend_json).unwrap(),
                command: &run.command,
                created_at: run.created_at,
                commit_sha: run.commit_sha.as_deref(),
                chat_session_id: run.chat_session_id.as_deref(),
                spec_digest: "spec-sha",
                launch: &test_launch(),
            })
            .unwrap();
        assert!(store
            .insert_submitting_run(SubmittingRun {
                id: &run.id,
                experiment_id: &run.experiment_id,
                project_id: &run.project_id,
                metadata: &crate::jobs::BackendDescriptor::parse(&run.backend_json).unwrap(),
                command: &run.command,
                created_at: run.created_at,
                commit_sha: run.commit_sha.as_deref(),
                chat_session_id: run.chat_session_id.as_deref(),
                spec_digest: "spec-sha",
                launch: &test_launch(),
            })
            .is_err());
        assert_eq!(store.count_active_runs().unwrap(), 10);
        assert!(!legacy_active_runs(&store.conn)
            .iter()
            .any(|(id, _)| id == &run.id));
        assert_eq!(
            store
                .conn
                .execute(LEGACY_REQUEST_CANCEL, params![run.id, "old CLI", now_ms()])
                .unwrap(),
            0
        );
        let submitting = store.get_run(&run.id).unwrap().unwrap();
        let descriptor = crate::jobs::BackendDescriptor::parse(&submitting.backend_json).unwrap();
        assert_eq!(descriptor.kind, "engine");
        assert!(descriptor.job_id.is_none());
        let handle = Handle::Ssh {
            host: "lab".into(),
            root: "/orx".into(),
        };
        assert!(!store.record_run_handle(&run.id, 1, &handle).unwrap());
        let fence = SubmissionFence {
            taken_at: 1000,
            submitter: Some(SubmitterIdentity {
                pid: 42,
                starttime: 5678,
            }),
        };
        assert!(!store.set_submission_fence(&run.id, 2, &fence).unwrap());
        assert!(store.set_submission_fence(&run.id, 1, &fence).unwrap());
        assert!(!store.set_submission_fence(&run.id, 1, &fence).unwrap());
        assert!(!store.record_run_handle(&run.id, 2, &handle).unwrap());
        let mut supervisor =
            open_lifecycle_lock_at(&dir.join("run-logs/engine_run.supervisor.lock")).unwrap();
        let mut shared =
            open_lifecycle_lock_at(&dir.join("run-logs/engine_run.supervisor.shared-ssh.lock"))
                .unwrap();
        let supervisor_guard = supervisor.write().unwrap();
        let shared_guard = shared.write().unwrap();
        assert!(store.record_run_handle(&run.id, 1, &handle).unwrap());
        assert!(!store.record_run_handle(&run.id, 1, &handle).unwrap());
        assert!(store.supervisor_is_held(&run.id).unwrap());
        let mut old_shared_probe =
            open_lifecycle_lock_at(&dir.join("run-logs/engine_run.supervisor.shared-ssh.lock"))
                .unwrap();
        assert!(
            matches!(old_shared_probe.try_write(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
        );
        let active = legacy_active_runs(&store.conn);
        assert_eq!(active.len(), 10);
        for (id, descriptor) in &active {
            assert!(descriptor.job_id.is_some());
            let mut probe =
                open_lifecycle_lock_at(&store.run_artifact_path(LEGACY_SUPERVISOR_LOCK, id))
                    .unwrap();
            let held = matches!(probe.try_write(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock);
            if id == &run.id {
                assert!(!LEGACY_ADOPTION(
                    descriptor.job_id.is_some(),
                    true,
                    true,
                    held
                ));
                assert!(!LEGACY_SUPERVISE_DISPATCH.contains(&descriptor.kind.as_str()));
                assert_ne!(descriptor.kind, LEGACY_SUPERVISE_FALLTHROUGH);
                assert!(!LEGACY_SSH_SERVICE_SCAN(&descriptor.kind, held, true));
                assert_eq!(
                    descriptor.source_digest.as_deref(),
                    Some("0000000000000000000000000000000000000000000000000000000000000000")
                );
                assert_eq!(descriptor.source_path.as_deref(), Some("/sources/0.tar"));
                assert_eq!(descriptor.source_size, Some(123));
            }
        }
        let row = store.get_run(&run.id).unwrap().unwrap();
        let descriptor = crate::jobs::BackendDescriptor::parse(&row.backend_json).unwrap();
        assert_eq!(descriptor.job_id.as_deref(), Some("/orx/runs/engine_run"));
        assert_eq!(
            descriptor.source_digest.as_deref(),
            Some("0000000000000000000000000000000000000000000000000000000000000000")
        );
        assert_eq!(descriptor.source_path.as_deref(), Some("/sources/0.tar"));
        assert_eq!(descriptor.source_size, Some(123));
        let digest = descriptor.source_digest.clone().unwrap();
        let snapshot = dir.join(format!("source-snapshots/{digest}.tar"));
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&snapshot, "keep").unwrap();
        store
            .conn
            .execute("DELETE FROM runs WHERE id = 'ssh_job'", [])
            .unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(snapshot.exists());
        let handle =
            |json: &str| serde_json::from_str::<serde_json::Value>(json).unwrap()["handle"].take();
        assert!(handle(&row.backend_json).is_object());
        assert_eq!(handle(&descriptor.to_json()), handle(&row.backend_json));
        assert!(store
            .update_status(&run.id, RunStatus::Running, None, None)
            .unwrap());
        assert!(legacy_active_runs(&store.conn)
            .iter()
            .any(|(id, _)| id == &run.id));
        drop(shared_guard);
        drop(supervisor_guard);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_contact_and_submission_fence_survive_reopen() {
        let (dir, store) = engine_fixture();
        assert!(store.engine_run_state("missing").unwrap().is_none());
        let mut run = run_fixture("engine_contact", "submitting", None);
        run.backend_json = r#"{"kind":"hf_job"}"#.into();
        store
            .insert_submitting_run(SubmittingRun {
                id: &run.id,
                experiment_id: &run.experiment_id,
                project_id: &run.project_id,
                metadata: &crate::jobs::BackendDescriptor::parse(&run.backend_json).unwrap(),
                command: &run.command,
                created_at: run.created_at,
                commit_sha: run.commit_sha.as_deref(),
                chat_session_id: run.chat_session_id.as_deref(),
                spec_digest: "digest",
                launch: &test_launch(),
            })
            .unwrap();
        let fence = SubmissionFence {
            taken_at: 123,
            submitter: Some(SubmitterIdentity {
                pid: 1234,
                starttime: u64::MAX,
            }),
        };
        assert!(store.set_submission_fence(&run.id, 1, &fence).unwrap());
        let contacts = [
            Contact::Unreachable {
                since: 100,
                cause: "transport".into(),
            },
            Contact::NeedsAuth {
                since: 200,
                cause: "key rejected".into(),
            },
            Contact::Unreconciled {
                since: 300,
                detail: "submit unknown".into(),
            },
            Contact::Live,
        ];
        for contact in contacts {
            assert!(store.set_run_contact(&run.id, &contact).unwrap());
            let reopened = Store::open_at(dir.clone()).unwrap();
            let state = reopened.engine_run_state(&run.id).unwrap().unwrap();
            assert_eq!(state.contact, contact);
            assert_eq!(state.spec_digest.as_deref(), Some("digest"));
            assert_eq!(state.submission_attempt, 1);
            assert_eq!(state.submission_fence.as_ref(), Some(&fence));
            assert!(state.cancel_accepted_at.is_none());
            assert_eq!(
                reopened.get_run(&run.id).unwrap().unwrap().status,
                "submitting"
            );
        }
        assert!(store
            .conn
            .query_row(
                "SELECT contact IS NULL FROM runs WHERE id = ?1",
                [&run.id],
                |row| row.get::<_, bool>(0)
            )
            .unwrap());
        assert!(!store.set_run_contact("missing", &Contact::Live).unwrap());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_notice_backoff_survives_restart_and_expires_after_one_day() {
        let (dir, store) = engine_fixture();
        for run in ["ssh_job", "slurm_job"] {
            store
                .insert_run_notice(run, "owner", TERMINAL, TERMINAL, "result", 1)
                .unwrap();
        }
        let start = 1_000_000;
        let frozen = store.run_notice_batch_at("owner", start).unwrap();
        assert_eq!(frozen.len(), 2);
        drop(store);

        let deadline = start + RUN_NOTICE_RETRY_WINDOW_MS;
        let mut now = start;
        let mut failures = 0;
        while now < deadline {
            let store = Store::open_at(dir.clone()).unwrap();
            assert_eq!(store.run_notice_batch_at("owner", now).unwrap(), frozen);
            failures += 1;
            assert_eq!(
                store
                    .record_run_notice_failure(
                        "owner",
                        &frozen[0].client_turn_id,
                        "rate limited",
                        false,
                        now
                    )
                    .unwrap(),
                Some(failures)
            );
            let expected_minutes = match failures {
                1 => 1,
                2 => 5,
                3 => 15,
                _ => 60,
            };
            let due = (now + expected_minutes * 60_000).min(deadline);
            let recorded: (i64, i64, Option<i64>) = store.conn.query_row(
                "SELECT first_failure_at, next_delivery_at, retired_at FROM run_notices WHERE id = ?1",
                [frozen[0].id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).unwrap();
            assert_eq!(recorded, (start, due, None));
            assert!(store
                .run_notice_batch_at("owner", due - 1)
                .unwrap()
                .is_empty());
            drop(store);
            now = due;
        }
        let store = Store::open_at(dir.clone()).unwrap();
        assert!(store
            .run_notice_batch_at("owner", deadline)
            .unwrap()
            .is_empty());
        let retired: (i64, String) = store
            .conn
            .query_row(
                "SELECT retired_at, delivery_error FROM run_notices WHERE id = ?1",
                [frozen[0].id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            retired,
            (
                deadline,
                "Retry window expired after 24 hours: rate limited".into()
            )
        );
        assert_eq!(
            store
                .record_run_notice_failure(
                    "owner",
                    &frozen[0].client_turn_id,
                    "again",
                    false,
                    deadline
                )
                .unwrap(),
            None
        );
        assert!(store.pending_run_notice_sessions().unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_retired_notice_batch_does_not_block_idle_and_is_reported_on_its_run() {
        let dir = std::env::temp_dir().join(format!("orx-retired-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .insert_run_notice("run", "owner", TERMINAL, TERMINAL, "done", 1)
            .unwrap();
        assert!(store.engine_idle_blocked().unwrap());
        assert_eq!(store.run_notice_error("run").unwrap(), None);
        let batch = store.run_notice_batch_at("owner", 10).unwrap();
        store
            .record_run_notice_failure(
                "owner",
                &batch[0].client_turn_id,
                "harness removed",
                true,
                20,
            )
            .unwrap();
        assert!(!store.engine_idle_blocked().unwrap());
        assert_eq!(
            store.run_notice_error("run").unwrap().as_deref(),
            Some("harness removed")
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_permanent_notice_failure_keeps_reason_and_unblocks_later_batches() {
        let (dir, store) = engine_fixture();
        store
            .insert_run_notice("ssh_job", "owner", ATTENTION, "attention:0", "outage", 1)
            .unwrap();
        let first = store.run_notice_batch_at("owner", 10).unwrap();
        assert_eq!(
            store
                .record_run_notice_failure(
                    "owner",
                    &first[0].client_turn_id,
                    "harness removed",
                    true,
                    20
                )
                .unwrap(),
            Some(1)
        );
        store
            .insert_run_notice("ssh_job", "owner", NOTIFY, "later", "later", 21)
            .unwrap();
        let later = store.run_notice_batch_at("owner", 21).unwrap();
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].body, "later");
        store
            .mark_run_notice_batch_delivered("owner", &later[0].client_turn_id, 22)
            .unwrap();
        assert!(!store.settle_outage_notice("ssh_job", "owner").unwrap());
        let retired: (i64, String) = store
            .conn
            .query_row(
                "SELECT retired_at, delivery_error FROM run_notices WHERE id = ?1",
                [first[0].id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(retired, (20, "harness removed".into()));
        assert!(store
            .insert_run_notice(
                "ssh_job",
                "owner",
                ATTENTION,
                "attention:0",
                "new outage",
                23
            )
            .unwrap());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_notices_dedupe_freeze_and_retry_across_connections() {
        let (dir, store) = engine_fixture();
        let second = Store::open_at(dir.clone()).unwrap();
        assert!(store.run_notice_batch("owner").unwrap().is_empty());
        assert_eq!(store.run_subscribers("ssh_job").unwrap(), ["owner"]);
        store.subscribe_run("ssh_job", "foreign", 1).unwrap();
        store.subscribe_run("ssh_job", "foreign", 2).unwrap();
        assert_eq!(
            store.run_subscribers("ssh_job").unwrap(),
            ["foreign", "owner"]
        );
        store.subscribe_run("ssh_job", "owner", 3).unwrap();
        assert_eq!(
            store.run_subscribers("ssh_job").unwrap(),
            ["foreign", "owner"]
        );
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT created_at FROM run_subscriptions WHERE chat_session_id = 'foreign'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        {
            let tx = store.begin().unwrap();
            store
                .insert_run_notice("ssh_job", "owner", "terminal", "terminal", "rolled back", 1)
                .unwrap();
            tx.rollback().unwrap();
        }
        assert!(store.run_notice_batch("owner").unwrap().is_empty());
        assert!(store
            .insert_run_notice("ssh_job", "owner", "terminal", "terminal", "finished", 10)
            .unwrap());
        assert!(store
            .insert_run_notice(
                "slurm_job",
                "owner",
                "terminal",
                "terminal",
                "also finished",
                11
            )
            .unwrap());
        let frozen = store.run_notice_batch("owner").unwrap();
        assert_eq!(frozen.len(), 2);
        uuid::Uuid::parse_str(
            frozen[0]
                .client_turn_id
                .strip_prefix("run-notice:")
                .unwrap(),
        )
        .unwrap();
        assert!(frozen
            .iter()
            .all(|row| row.client_turn_id == frozen[0].client_turn_id));
        assert!(!second
            .insert_run_notice("ssh_job", "owner", "terminal", "terminal", "changed", 12)
            .unwrap());
        assert!(second
            .insert_run_notice("ssh_job", "owner", "notify", "notify:1", "new bytes", 13)
            .unwrap());
        assert!(second
            .insert_run_notice(
                "ssh_job",
                "foreign",
                "terminal",
                "terminal",
                "foreign result",
                14
            )
            .unwrap());
        assert_eq!(second.run_notice_batch("owner").unwrap(), frozen);
        assert_eq!(store.run_notice_batch("owner").unwrap(), frozen);
        let foreign = second.run_notice_batch("foreign").unwrap();
        assert_eq!(foreign.len(), 1);
        assert_ne!(foreign[0].client_turn_id, frozen[0].client_turn_id);
        let turn = frozen[0].client_turn_id.as_str();
        second
            .mark_run_notice_batch_delivered("foreign", turn, 15)
            .unwrap();
        assert_eq!(store.run_notice_batch("owner").unwrap(), frozen);
        second
            .mark_run_notice_batch_delivered("owner", turn, 16)
            .unwrap();
        store
            .mark_run_notice_batch_delivered("owner", turn, 17)
            .unwrap();
        assert_eq!(
            store
                .conn
                .query_row(
                    "SELECT MIN(delivered_at) FROM run_notices WHERE client_turn_id = ?1",
                    [turn],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            16
        );
        let next = store.run_notice_batch("owner").unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].body, "new bytes");
        assert_ne!(next[0].client_turn_id, frozen[0].client_turn_id);
        drop(second);
        drop(store);
        let reopened = Store::open_at(dir.clone()).unwrap();
        assert_eq!(reopened.run_notice_batch("owner").unwrap(), next);
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_wakeup_migration_never_refires_and_preserves_foreign_chats() {
        let (dir, store) = engine_fixture();
        let claimed = store
            .claim_run_wakeup("failed", "foreign")
            .unwrap()
            .unwrap();
        assert!(!claimed.is_empty());
        store.subscribe_run("ssh_job", "foreign", 1).unwrap();
        assert_eq!(
            store
                .conn
                .query_row("SELECT COUNT(*) FROM run_notices", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        store.migrate_run_wakeups().unwrap();
        let subscriptions: Vec<(String, String)> = store
            .conn
            .prepare("SELECT run_id, chat_session_id FROM run_subscriptions ORDER BY 1,2")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            subscriptions,
            [
                ("ssh_job".into(), "foreign".into()),
                ("ssh_job".into(), "owner".into())
            ]
        );
        let delivered: (String, String, i64) = store
            .conn
            .query_row(
                "SELECT kind, dedupe, delivered_at FROM run_notices WHERE run_id = 'done'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(delivered, ("terminal".into(), "terminal".into(), 500));
        let alert: (String, String, i64) = store.conn.query_row(
            "SELECT kind, chat_session_id, delivered_at FROM run_notices WHERE run_id = 'ssh_job'", [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).unwrap();
        assert_eq!(alert, ("attention".into(), "foreign".into(), 400));
        assert!(store.run_notice_batch("owner").unwrap().is_empty());
        let completion = store.run_notice_batch("foreign").unwrap();
        assert_eq!(completion.len(), 1);
        assert_eq!(completion[0].run_id, "failed");
        assert_eq!(
            completion[0].body,
            store
                .terminal_notice_body(&store.get_run("failed").unwrap().unwrap())
                .unwrap()
        );
        assert_eq!(completion[0].dedupe, TERMINAL);
        let key = store
            .outage_dedupe("ssh_job", "foreign", ATTENTION)
            .unwrap();
        assert_eq!(key, "attention:0");
        assert!(!store
            .insert_run_notice(
                "ssh_job",
                "foreign",
                ATTENTION,
                &key,
                "duplicate outage",
                500
            )
            .unwrap());
        let recovered_key = store
            .outage_dedupe("ssh_job", "foreign", RECOVERED)
            .unwrap();
        store
            .insert_run_notice(
                "ssh_job",
                "foreign",
                RECOVERED,
                &recovered_key,
                "recovered",
                501,
            )
            .unwrap();
        let next_key = store
            .outage_dedupe("ssh_job", "foreign", ATTENTION)
            .unwrap();
        assert_eq!(next_key, "attention:1");
        assert!(store
            .insert_run_notice(
                "ssh_job",
                "foreign",
                ATTENTION,
                &next_key,
                "next outage",
                502
            )
            .unwrap());
        assert!(!store
            .insert_run_notice(
                "done",
                "owner",
                "terminal",
                "terminal",
                "must not re-fire",
                600
            )
            .unwrap());
        let legacy_rows: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM chat_run_wakeups", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(legacy_rows, 4);
        store
            .conn
            .execute(
                "DELETE FROM run_subscriptions WHERE chat_session_id = 'foreign'",
                [],
            )
            .unwrap();
        drop(store);
        let reopened = Store::open_at(dir.clone()).unwrap();
        reopened.migrate_run_wakeups().unwrap();
        assert_eq!(
            reopened
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM run_subscriptions WHERE chat_session_id = 'foreign'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            reopened
                .conn
                .query_row("SELECT COUNT(*) FROM run_notices", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            reopened
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_cancel_acceptance_survives_legacy_withdraw_and_handles_submitting() {
        let (dir, store) = engine_fixture();
        let path = dir.join("run-logs/slurm_job.cancel.lock");
        let mut old_lock = open_lifecycle_lock_at(&path).unwrap();
        let old_guard = old_lock.write().unwrap();
        assert!(store.request_cancel("slurm_job", "old CLI").unwrap());
        store.withdraw_cancel("slurm_job").unwrap();
        drop(old_guard);
        assert_eq!(store.accept_run_cancel("slurm_job", None).unwrap(), None);
        let old_guard = old_lock.write().unwrap();
        assert!(store.request_cancel("slurm_job", "old CLI").unwrap());
        drop(old_guard);
        let accepted = store.accept_run_cancel("slurm_job", None).unwrap().unwrap();
        assert!(accepted > 0);
        let old_guard = old_lock.write().unwrap();
        store.withdraw_cancel("slurm_job").unwrap();
        drop(old_guard);
        assert_eq!(
            store.accept_run_cancel("slurm_job", None).unwrap(),
            Some(accepted)
        );
        let state = store.engine_run_state("slurm_job").unwrap().unwrap();
        assert_eq!(state.cancel_accepted_at, Some(accepted));
        assert!(
            !store
                .get_run("slurm_job")
                .unwrap()
                .unwrap()
                .cancel_requested
        );
        let mut run = run_fixture("engine_cancel", "submitting", None);
        run.backend_json = r#"{"kind":"modal_job"}"#.into();
        store
            .insert_submitting_run(SubmittingRun {
                id: &run.id,
                experiment_id: &run.experiment_id,
                project_id: &run.project_id,
                metadata: &crate::jobs::BackendDescriptor::parse(&run.backend_json).unwrap(),
                command: &run.command,
                created_at: run.created_at,
                commit_sha: run.commit_sha.as_deref(),
                chat_session_id: run.chat_session_id.as_deref(),
                spec_digest: "digest",
                launch: &test_launch(),
            })
            .unwrap();
        assert!(!store.request_cancel(&run.id, "old CLI").unwrap());
        assert!(store
            .accept_run_cancel(&run.id, Some("API"))
            .unwrap()
            .is_some());
        let fence = SubmissionFence {
            taken_at: 1,
            submitter: None,
        };
        assert!(!store.set_submission_fence(&run.id, 1, &fence).unwrap());
        store
            .accept_run_cancel(&run.id, Some("later API request"))
            .unwrap();
        assert_eq!(
            store
                .get_run(&run.id)
                .unwrap()
                .unwrap()
                .cancel_reason
                .as_deref(),
            Some("API")
        );
        assert_eq!(store.accept_run_cancel("done", Some("API")).unwrap(), None);
        assert_eq!(store.accept_run_cancel("missing", None).unwrap(), None);
        assert!(store.accept_run_cancel("../escape", None).is_err());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn engine_cancel_acceptance_waits_for_legacy_writer_to_commit_or_withdraw() {
        let (dir, store) = engine_fixture();
        let second = Store::open_at(dir.clone()).unwrap();
        let mut old_lock =
            open_lifecycle_lock_at(&dir.join("run-logs/slurm_job.cancel.lock")).unwrap();
        let guard = old_lock.write().unwrap();
        store.request_cancel("slurm_job", "old CLI").unwrap();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            result_tx
                .send(second.accept_run_cancel("slurm_job", None).unwrap())
                .unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(
            result_rx.recv_timeout(Duration::from_millis(100)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        store.withdraw_cancel("slurm_job").unwrap();
        drop(guard);
        assert_eq!(
            result_rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            None
        );
        thread.join().unwrap();
        assert!(store
            .engine_run_state("slurm_job")
            .unwrap()
            .unwrap()
            .cancel_accepted_at
            .is_none());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn store_protects_existing_data_and_sqlite_sidecars() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("orx-private-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let store = Store::open_at(dir.clone()).unwrap();
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Released binaries keep opening the store, so a column one knows keeps its
    /// type, nullability and default, and a column it doesn't know accepts its inserts.
    #[test]
    fn schema_only_grows_from_the_last_release() {
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("../tests/contracts/store-schema.json")).unwrap();
        let release = snapshot["release"].as_str().unwrap();
        let dir = std::env::temp_dir().join(format!("orx-store-schema-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        for (table, released) in snapshot["tables"].as_object().unwrap() {
            let mut statement = store
                .conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap();
            let current: Vec<serde_json::Value> = statement
                .query_map([], |row| {
                    Ok(serde_json::json!({
                        "name": row.get::<_, String>(1)?,
                        "type": row.get::<_, String>(2)?,
                        "notnull": row.get::<_, bool>(3)?,
                        "default": row.get::<_, Option<String>>(4)?,
                        "pk": row.get::<_, i64>(5)?,
                    }))
                })
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            assert!(!current.is_empty(), "{table} from {release} was dropped");

            let released = released.as_array().unwrap();
            for old in released {
                let name = &old["name"];
                let new = current
                    .iter()
                    .find(|column| &column["name"] == name)
                    .unwrap_or_else(|| panic!("{table}.{name} from {release} was dropped"));
                assert_eq!(new["type"], old["type"], "{table}.{name} type");
                assert_eq!(new["pk"], old["pk"], "{table}.{name} primary key");
                assert!(
                    new["notnull"] == false || old["notnull"] == true,
                    "{table}.{name} became NOT NULL"
                );
                assert!(
                    old["default"].is_null() || !new["default"].is_null(),
                    "{table}.{name} lost its default"
                );
            }
            for new in &current {
                let added = !released.iter().any(|old| old["name"] == new["name"]);
                let insertable = new["notnull"] == false || !new["default"].is_null();
                assert!(
                    !added || insertable,
                    "{table}.{} rejects inserts from {release}",
                    new["name"]
                );
            }
        }
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn project_with_run_artifacts(store: &Store, project: &str, run: &str, digest: &str) {
        let repo = store.data_dir.join(format!("repo-{project}"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("keep"), "repository").unwrap();
        store.conn.execute(
            "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, repo_path, created_at, updated_at)
             VALUES (?1, ?1, ?1, '', '', ?2, 1, 1)",
            params![project, repo.to_string_lossy()],
        ).unwrap();
        let descriptor = serde_json::json!({
            "kind": "local_job", "sourceDigest": digest,
            "jobId": repo, "sourcePath": repo.join("keep"),
        });
        store.conn.execute(
            "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at)
             VALUES (?1, 'experiment', ?2, 'done', ?3, 1, 1)",
            params![run, project, descriptor.to_string()],
        ).unwrap();
        for path in [
            store
                .data_dir
                .join("local-runs")
                .join(run)
                .join("repo/output"),
            store.data_dir.join("run-logs").join(format!("{run}.log")),
            store
                .data_dir
                .join("submission-handles")
                .join(format!("{run}.json")),
            store
                .data_dir
                .join("source-snapshots")
                .join(format!("{digest}.tar")),
            store
                .data_dir
                .join("source-snapshots")
                .join(format!("{digest}.hf/payload")),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "artifact").unwrap();
        }
        std::fs::write(
            store
                .data_dir
                .join("submission-handles")
                .join(format!("{run}.json")),
            descriptor.to_string(),
        )
        .unwrap();
    }

    #[test]
    fn orphan_cleanup_reclaims_rowless_local_artifacts_and_preserves_other_runs() {
        let dir =
            std::env::temp_dir().join(format!("orx-orphan-artifacts-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let orphan = uuid::Uuid::new_v4().to_string();
        let shared_orphan = uuid::Uuid::new_v4().to_string();
        let surviving = uuid::Uuid::new_v4().to_string();
        let abandoned = uuid::Uuid::new_v4().to_string();
        let tinker = uuid::Uuid::new_v4().to_string();
        let unreferenced = "a".repeat(64);
        let shared = "b".repeat(64);
        project_with_run_artifacts(&store, "old", &orphan, &unreferenced);
        project_with_run_artifacts(&store, "old-shared", &shared_orphan, &shared);
        project_with_run_artifacts(&store, "surviving", &surviving, &shared);
        project_with_run_artifacts(&store, "abandoned", &abandoned, &unreferenced);
        project_with_run_artifacts(&store, "tinker", &tinker, &unreferenced);
        let tinker_handle = dir
            .join("submission-handles")
            .join(format!("{tinker}.json"));
        let mut descriptor: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&tinker_handle).unwrap()).unwrap();
        descriptor["kind"] = serde_json::json!("tinker_job");
        std::fs::write(tinker_handle, descriptor.to_string()).unwrap();
        let abandoned_dir = dir.join("local-runs").join(&abandoned);
        std::fs::write(abandoned_dir.join("run.sh"), "echo never started").unwrap();
        store
            .conn
            .execute("DELETE FROM runs WHERE project_id != 'surviving'", [])
            .unwrap();
        for id in [&orphan, &shared_orphan, &tinker] {
            std::fs::write(dir.join("local-runs").join(id).join("exit_code"), "0").unwrap();
        }
        let zip = dir
            .join("source-snapshots")
            .join(format!("{}.zip", "c".repeat(64)));
        std::fs::write(&zip, "upload-only").unwrap();
        std::fs::write(dir.join("source-snapshots/keep.txt"), "not an archive").unwrap();
        let sidecar_only = uuid::Uuid::new_v4().to_string();
        for name in [
            format!("{orphan}.supervisor.log"),
            format!("{orphan}.cancel.lock"),
            format!("{orphan}.supervisor.lock"),
            format!("{sidecar_only}.supervisor.log"),
        ] {
            std::fs::write(dir.join("run-logs").join(name), "").unwrap();
        }
        let staging_lock = store.run_artifacts_lock().unwrap();
        let staging = staging_lock.read().unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(dir.join("local-runs").join(&orphan).is_dir());
        drop(staging);
        drop(staging_lock);
        store.cleanup_orphaned_run_artifacts().unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert_eq!(
            std::fs::read_dir(dir.join("run-logs"))
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>(),
            [std::ffi::OsString::from(format!("{surviving}.log"))]
        );
        for id in [&orphan, &shared_orphan, &abandoned, &tinker] {
            assert!(!dir.join("local-runs").join(id).exists());
            assert!(!dir.join("run-logs").join(format!("{id}.log")).exists());
            assert!(!dir
                .join("submission-handles")
                .join(format!("{id}.json"))
                .exists());
        }
        for extension in ["tar", "hf"] {
            assert!(!dir
                .join("source-snapshots")
                .join(format!("{unreferenced}.{extension}"))
                .exists());
            assert!(dir
                .join("source-snapshots")
                .join(format!("{shared}.{extension}"))
                .exists());
        }
        assert!(!zip.exists());
        assert!(dir.join("source-snapshots/keep.txt").is_file());
        assert!(dir
            .join("local-runs")
            .join(&surviving)
            .join("repo/output")
            .is_file());
        assert!(dir.join("repo-old/keep").is_file());
        assert!(dir.join("repo-surviving/keep").is_file());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn orphan_cleanup_slims_only_finished_runs_beyond_each_projects_newest() {
        let dir = std::env::temp_dir().join(format!("orx-slim-runs-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let run_dir = |id: &str| dir.join("local-runs").join(id);
        let outside = dir.join("outside");
        std::fs::create_dir_all(outside.join("uv")).unwrap();
        let mut runs = Vec::new();
        for (created_at, project, status, kind) in [
            (1, "busy", "done", "local_job"),
            (2, "busy", "running", "local_job"),
            (3, "busy", "failed", "local_job"),
            (4, "busy", "cancelled", "local_job"),
            (5, "busy", "done", "local_job"),
            (6, "busy", "done", "local_job"),
            (7, "busy", "done", "local_job"),
            (8, "busy", "done", "ssh_job"),
            (1, "quiet", "done", "local_job"),
        ] {
            let id = uuid::Uuid::new_v4().to_string();
            store.conn.execute(
                "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at)
                 VALUES (?1, 'experiment', ?2, ?3, json_object('kind', ?4), ?5, ?5)",
                params![id, project, status, kind, created_at],
            ).unwrap();
            for path in ["repo/.venv/bin", "repo/.cache/nanochat", "tensorboard"] {
                std::fs::create_dir_all(run_dir(&id).join(path)).unwrap();
            }
            std::fs::write(run_dir(&id).join("exit_code"), "0").unwrap();
            runs.push(id);
        }
        let (oldest, running, symlinked, supervised) = (&runs[0], &runs[1], &runs[2], &runs[3]);
        std::fs::create_dir_all(run_dir(oldest).join("repo/.cache/uv")).unwrap();
        let symlinked_repo = run_dir(symlinked).join("repo");
        std::fs::remove_dir_all(symlinked_repo.join(".cache")).unwrap();
        std::os::unix::fs::symlink(&outside, symlinked_repo.join(".cache")).unwrap();
        let mut supervisor =
            open_lifecycle_lock_at(&store.run_artifact_path(SUPERVISOR_LOCK, supervised)).unwrap();
        let held = supervisor.write().unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        drop(held);
        assert!(!run_dir(oldest).join("repo/.venv").exists());
        assert!(!run_dir(oldest).join("repo/.cache/uv").exists());
        for kept in ["repo/.cache/nanochat", "tensorboard", "exit_code"] {
            assert!(run_dir(oldest).join(kept).exists(), "{kept}");
        }
        assert!(!symlinked_repo.join(".venv").exists());
        assert!(outside.join("uv").is_dir());
        let newest_and_quiet = &runs[4..];
        for kept in [running, supervised].into_iter().chain(newest_and_quiet) {
            assert!(run_dir(kept).join("repo/.venv").is_dir(), "{kept}");
        }
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shared_ssh_log_cleanup_preserves_retained_runs_and_removes_deleted_generations() {
        let dir =
            std::env::temp_dir().join(format!("orx-ssh-log-cleanup-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let deleted = uuid::Uuid::new_v4().to_string();
        let retained = uuid::Uuid::new_v4().to_string();
        project_with_run_artifacts(&store, "deleted", &deleted, &"a".repeat(64));
        project_with_run_artifacts(&store, "retained", &retained, &"b".repeat(64));
        let suffixes = [
            "cursor.json".to_string(),
            "pending.json".to_string(),
            "cursor.lock".to_string(),
            "supervisor.shared-ssh.lock".to_string(),
            format!("{}.log", uuid::Uuid::new_v4()),
            format!("{}.log", uuid::Uuid::new_v4()),
        ];
        for run in [&deleted, &retained] {
            for suffix in &suffixes {
                std::fs::write(dir.join("run-logs").join(format!("{run}.{suffix}")), "log")
                    .unwrap();
            }
        }
        let orphan = dir.join("run-logs").join(format!(
            "{}.{}.log",
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&orphan, "interrupted generation").unwrap();
        let unrelated = dir.join("run-logs").join(format!("{deleted}.notes.log"));
        std::fs::write(&unrelated, "unrecognized file").unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(!orphan.exists());
        for run in [&deleted, &retained] {
            for suffix in &suffixes {
                assert!(dir
                    .join("run-logs")
                    .join(format!("{run}.{suffix}"))
                    .is_file());
            }
        }
        store.delete_local_project("deleted").unwrap();
        for suffix in &suffixes {
            assert!(!dir
                .join("run-logs")
                .join(format!("{deleted}.{suffix}"))
                .exists());
            assert!(dir
                .join("run-logs")
                .join(format!("{retained}.{suffix}"))
                .is_file());
        }
        assert!(unrelated.is_file());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn orphan_cleanup_skips_malformed_rows_and_keeps_every_snapshot() {
        let dir = std::env::temp_dir().join(format!("orx-malformed-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let orphan = uuid::Uuid::new_v4().to_string();
        let unreferenced = "f".repeat(64);
        project_with_run_artifacts(&store, "old", &orphan, &unreferenced);
        project_with_run_artifacts(&store, "malformed", "malformed-run", &"0".repeat(64));
        store
            .conn
            .execute("DELETE FROM runs WHERE project_id = 'old'", [])
            .unwrap();
        store
            .conn
            .execute("UPDATE runs SET backend_json = 'not json'", [])
            .unwrap();
        std::fs::write(dir.join("local-runs").join(&orphan).join("exit_code"), "0").unwrap();
        let unreadable = uuid::Uuid::new_v4().to_string();
        let unreadable_handle = dir
            .join("submission-handles")
            .join(format!("{unreadable}.json"));
        std::fs::create_dir_all(&unreadable_handle).unwrap();
        let pass = store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(!pass.more);
        assert_eq!(pass.kept.len(), 1);
        assert_eq!(pass.kept[0].0, unreadable);
        assert!(!dir.join("local-runs").join(&orphan).exists());
        assert!(unreadable_handle.is_dir());
        assert!(dir
            .join("source-snapshots")
            .join(format!("{unreferenced}.tar"))
            .is_file());
        store.delete_local_project("malformed").unwrap();
        assert!(!dir.join("local-runs/malformed-run").exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn experiment_delete_reclaims_its_run_artifacts_only_after_checks_pass() {
        let dir = std::env::temp_dir().join(format!("orx-exp-artifacts-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let run = uuid::Uuid::new_v4().to_string();
        let sibling = uuid::Uuid::new_v4().to_string();
        let (digest, shared) = ("d".repeat(64), "e".repeat(64));
        project_with_run_artifacts(&store, "p", &run, &digest);
        project_with_run_artifacts(&store, "q", &sibling, &shared);
        store
            .conn
            .execute(
                "INSERT INTO local_experiments (id, project_id, slug, branch_name, title, description,
                   run_command, agent_status, created_at, updated_at)
                 VALUES ('experiment', 'p', 'e', 'orx/e', NULL, NULL, '', 'idle', 1, 1)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE runs SET experiment_id = 'other' WHERE id = ?1",
                params![sibling],
            )
            .unwrap();
        let refused = |store: &Store| {
            let mut called = false;
            assert!(store
                .delete_local_experiment("experiment", || {
                    called = true;
                    Ok(())
                })
                .is_err());
            assert!(!called);
        };
        store
            .conn
            .execute(
                "UPDATE runs SET status = 'running' WHERE id = ?1",
                params![run],
            )
            .unwrap();
        refused(&store);
        store
            .conn
            .execute(
                "UPDATE runs SET status = 'done' WHERE id = ?1",
                params![run],
            )
            .unwrap();
        // A terminal run whose supervisor is still finishing is refused before anything is undone.
        let mut supervisor =
            open_lifecycle_lock_at(&dir.join("run-logs").join(format!("{run}.supervisor.lock")))
                .unwrap();
        let held = supervisor.write().unwrap();
        refused(&store);
        drop(held);
        drop(supervisor);
        store
            .conn
            .execute(
                "UPDATE runs SET backend_json = ?2 WHERE id = ?1",
                params![
                    run,
                    r#"{"kind":"engine","handle":{"type":"Ssh","host":"dead","root":"/orx"}}"#
                ],
            )
            .unwrap();
        store
            .set_run_contact(
                &run,
                &Contact::Unreachable {
                    since: 1,
                    cause: "host gone".into(),
                },
            )
            .unwrap();
        let mut supervisor =
            open_lifecycle_lock_at(&dir.join("run-logs").join(format!("{run}.supervisor.lock")))
                .unwrap();
        let held = supervisor.write().unwrap();
        store
            .delete_local_experiment("experiment", || Ok(()))
            .unwrap();
        drop(held);
        drop(supervisor);
        // A repeat delete is refused before touching anything.
        refused(&store);
        assert!(!dir.join("local-runs").join(&run).exists());
        assert!(!dir.join("run-logs").join(format!("{run}.log")).exists());
        assert!(!dir
            .join("source-snapshots")
            .join(format!("{digest}.tar"))
            .exists());
        assert!(store.get_run(&run).unwrap().is_none());
        assert!(dir
            .join("local-runs")
            .join(&sibling)
            .join("repo/output")
            .is_file());
        assert!(dir
            .join("source-snapshots")
            .join(format!("{shared}.tar"))
            .is_file());
        assert!(store.get_run(&sibling).unwrap().is_some());
        assert!(store.get_local_project("p").unwrap().is_some());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn remote_cleanups_keep_each_projects_newest_runs_until_project_delete() {
        let dir = std::env::temp_dir().join(format!("orx-remote-cleanup-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        project_with_run_artifacts(&store, "p", "local", &"d".repeat(64));
        store
            .conn
            .execute("UPDATE runs SET created_at = 10 WHERE id = 'local'", [])
            .unwrap();
        let insert = |id: &str, status: &str, kind: &str, created_at: i64| {
            let descriptor =
                serde_json::json!({"kind": kind, "namespace": "lab", "sourceDigest": id});
            store.conn.execute(
                "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at)
                 VALUES (?1, 'experiment', 'p', ?2, ?3, ?4, ?4)",
                params![id, status, descriptor.to_string(), created_at],
            ).unwrap();
        };
        let due = |store: &Store| {
            let mut entries = store
                .due_remote_cleanups(10)
                .unwrap()
                .into_iter()
                .map(|entry| (entry.run_id, entry.deleted))
                .collect::<Vec<_>>();
            entries.sort();
            entries
        };
        let finished = |ids: &[&str]| {
            ids.iter()
                .map(|id| (id.to_string(), false))
                .collect::<Vec<_>>()
        };
        insert("r1", "done", "ssh_job", 1);
        insert("r2", "failed", "slurm_job", 2);
        insert("r3", "done", "ssh_job", 3);
        insert("box", "done", "openresearch_job", 4);
        for id in ["r1", "r2", "r3", "box", "local"] {
            store.queue_finished_remote_cleanup(id).unwrap();
        }
        // The newest three remote runs stay whole; local and other kinds don't count.
        assert!(due(&store).is_empty());
        insert("r4", "done", "ssh_job", 5);
        store.queue_finished_remote_cleanup("r4").unwrap();
        assert_eq!(due(&store), finished(&["r1"]));
        // As locally, an unfinished newer run takes one of the slots.
        insert("r5", "running", "slurm_job", 6);
        assert_eq!(due(&store), finished(&["r1", "r2"]));
        store
            .defer_remote_cleanup("r1", false, "unreachable")
            .unwrap();
        assert_eq!(due(&store), finished(&["r2"]));
        store
            .conn
            .execute("UPDATE runs SET status = 'done'", [])
            .unwrap();
        store.delete_local_project("p").unwrap();
        // Deletion marks every SSH/Slurm run, including the newest and one waiting to retry.
        assert_eq!(
            due(&store),
            ["r1", "r2", "r3", "r4", "r5"]
                .iter()
                .map(|id| (id.to_string(), true))
                .collect::<Vec<_>>()
        );
        // A pass that started before the deletion queued only finishes what it acted on.
        store.finish_remote_cleanup("r2", false).unwrap();
        store.defer_remote_cleanup("r2", false, "stale").unwrap();
        store.finish_remote_cleanup("r1", true).unwrap();
        assert_eq!(due(&store).len(), 4);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unfinished_log_collection_survives_retries_and_a_collected_log_stays_collected() {
        let dir = std::env::temp_dir().join(format!("orx-collection-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        assert!(!store.log_collection_pending("legacy").unwrap());
        // Kept before its terminal commit queues it: the queue leaves it kept.
        store.finish_log_collection("early").unwrap();
        store.queue_log_collection("early").unwrap();
        assert!(!store.log_collection_pending("early").unwrap());

        store.queue_log_collection("late").unwrap();
        assert_eq!(store.log_collection("late").unwrap(), Some((false, None)));
        assert_eq!(store.due_log_collections(10).unwrap(), ["late"]);
        assert!(!store
            .defer_log_collection("late", "host in backoff")
            .unwrap());
        assert!(store.due_log_collections(10).unwrap().is_empty());
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        assert!(store.log_collection_pending("late").unwrap());
        // Retries reach their cap once, then go on hourly; it never stops being owed.
        let capped: Vec<bool> = (1..LOG_COLLECTION_FAILED_AFTER + 3)
            .map(|_| store.defer_log_collection("late", "gone").unwrap())
            .collect();
        assert_eq!(capped.iter().filter(|capped| **capped).count(), 1);
        assert_eq!(
            store.log_collection("late").unwrap(),
            Some((true, Some("gone".into())))
        );
        let next: i64 = store
            .conn
            .query_row(
                "SELECT next_attempt_at FROM log_collections WHERE run_id = 'late'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(next - now_ms() <= LOG_COLLECTION_MAX_RETRY_MS);
        store.retry_log_collections_now(None).unwrap();
        assert_eq!(store.due_log_collections(10).unwrap(), ["late"]);
        store.finish_log_collection("late").unwrap();
        assert!(!store.log_collection_pending("late").unwrap());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_engine_slurm_run_reads_as_the_descriptor_exp_status_shows() {
        let launch = LaunchSpec {
            kind: "slurm_job".into(),
            target: Some("login".into()),
            options: serde_json::json!({
                "timeout_seconds": 3600,
                "directives": {"cpus-per-task": "4", "mem": "32G", "ntasks": "1"},
            }),
        };
        let stored = r#"{"kind":"engine","jobId":"7","handle":{"type":"Slurm","host":"login","root":"~/.orx/engine","job":"7"}}"#;
        let public = public_backend(stored, Some(&launch)).unwrap();
        let descriptor = crate::jobs::BackendDescriptor::parse(&public.to_string()).unwrap();
        assert_eq!(descriptor.kind, "slurm_job");
        assert_eq!(descriptor.slurm_ref().unwrap(), ("login", "7"));
        assert_eq!(descriptor.timeout_secs, Some(3600));
        assert_eq!(
            crate::local::slurm::status_resources(&descriptor),
            "requested cpus 4  mem 32G"
        );
        assert_eq!(public_backend(stored, None).unwrap()["kind"], "engine");
    }

    #[test]
    fn engine_runs_queue_their_host_dir_and_container_for_cleanup() {
        let dir = std::env::temp_dir().join(format!("orx-engine-cleanup-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let container = serde_json::json!({
            "reference": "dev", "id": "c1", "startedAt": "t", "runDir": "/root/.orx/runs/e1"
        });
        for (id, handle, launch, created_at) in [
            (
                "e1",
                r#"{"type":"Ssh","host":"lab","root":"~/.orx/engine"}"#,
                serde_json::json!({"kind":"ssh_job","target":"lab","options":{"container":container}}),
                1,
            ),
            (
                "e2",
                r#"{"type":"Slurm","host":"login","root":"~/.orx/engine","job":"7"}"#,
                serde_json::json!({"kind":"slurm_job","target":"login","options":{}}),
                2,
            ),
            (
                "e3",
                r#"{"type":"Api","provider":"hf_job","account":{"target":"org","context":null},"ids":["j"]}"#,
                serde_json::json!({"kind":"hf_job","target":"org","options":{}}),
                3,
            ),
        ] {
            store.conn.execute(
                "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, launch_spec, created_at, updated_at)
                 VALUES (?1, 'experiment', 'p', 'done', ?2, ?3, ?4, ?4)",
                params![id, format!(r#"{{"kind":"engine","handle":{handle}}}"#), launch.to_string(), created_at],
            ).unwrap();
            store.queue_finished_remote_cleanup(id).unwrap();
        }
        store.delete_local_project("p").unwrap();
        let mut due = store.due_remote_cleanups(10).unwrap();
        due.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        assert_eq!(
            due.iter()
                .map(|entry| entry.run_id.as_str())
                .collect::<Vec<_>>(),
            ["e1", "e2"]
        );
        let queued: serde_json::Value = serde_json::from_str(&due[0].backend_json).unwrap();
        assert_eq!(queued["sshContainer"], container);
        assert_eq!(queued["handle"]["host"], "lab");
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retention_rechecks_tombstones_and_output_debt_until_explicit_deletion() {
        let (dir, store) = engine_fixture();
        for n in 0..5 {
            let mut run = run_fixture(&format!("retention-{n}"), "done", None);
            run.backend_json =
                r#"{"kind":"engine","handle":{"type":"Ssh","host":"lab","root":"/orx"}}"#.into();
            run.created_at = n;
            run.project_id = "retention".into();
            store.upsert_run(&run).unwrap();
            store.queue_finished_remote_cleanup(&run.id).unwrap();
        }
        let run = "retention-0";
        assert!(store.remote_cleanup_eligible(run, false).unwrap());
        store
            .set_run_contact(
                run,
                &Contact::Unreconciled {
                    since: 1,
                    detail: "kill pending".into(),
                },
            )
            .unwrap();
        assert!(!store.remote_cleanup_eligible(run, false).unwrap());
        assert!(!store
            .due_remote_cleanups(100)
            .unwrap()
            .iter()
            .any(|entry| entry.run_id == run));
        store.set_run_contact(run, &Contact::Live).unwrap();
        store.queue_log_collection(run).unwrap();
        assert!(!store.remote_cleanup_eligible(run, false).unwrap());
        store.finish_log_collection(run).unwrap();
        assert!(store.remote_cleanup_eligible(run, false).unwrap());
        store
            .conn
            .execute(
                "UPDATE remote_cleanups SET deleted = 1 WHERE run_id = ?1",
                [run],
            )
            .unwrap();
        store
            .set_run_contact(
                run,
                &Contact::NeedsAuth {
                    since: 1,
                    cause: "offline".into(),
                },
            )
            .unwrap();
        assert!(!store.remote_cleanup_eligible(run, true).unwrap());
        store
            .conn
            .execute("DELETE FROM runs WHERE id = ?1", [run])
            .unwrap();
        assert!(store.remote_cleanup_eligible(run, true).unwrap());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn source_sweeps_come_due_after_the_idle_window() {
        let dir = std::env::temp_dir().join(format!("orx-source-sweep-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store.schedule_source_sweep("lab", None).unwrap();
        store
            .schedule_source_sweep("lab", Some("/scratch/orx"))
            .unwrap();
        assert!(store.due_source_sweeps().unwrap().is_empty());
        store
            .conn
            .execute("UPDATE remote_source_sweeps SET due_at = 0", [])
            .unwrap();
        let mut sweeps = store.due_source_sweeps().unwrap();
        sweeps.sort();
        assert_eq!(
            sweeps,
            [
                ("lab".to_string(), None),
                ("lab".to_string(), Some("/scratch/orx".to_string()))
            ]
        );
        store.finish_source_sweep("lab", None).unwrap();
        assert_eq!(store.due_source_sweeps().unwrap().len(), 1);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn live_wrapper_runs_survive_slimming_orphan_reclaim_and_block_data_dir_moves() {
        let dir = std::env::temp_dir().join(format!("orx-live-wrapper-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        project_with_run_artifacts(&store, "wrapper", &id, &"d".repeat(64));
        let run_dir = dir.join("local-runs").join(&id);
        std::fs::create_dir_all(run_dir.join("repo/.venv")).unwrap();
        let script = "touch ready; sleep 60";
        std::fs::write(run_dir.join("run.sh"), script).unwrap();
        orx_host::publish(
            &run_dir.join("launch.json"),
            &orx_host_proto::LaunchIntent {
                run: id.clone(),
                digest: orx_host::sha(script.as_bytes()),
                run_sh: script.into(),
                env: Default::default(),
                source: None,
                cancel_sh: None,
            },
        )
        .unwrap();
        let binary = orx_test_support::host_binaries().join(format!(
            "orx-host-{}-{}",
            if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            },
            std::env::consts::ARCH
        ));
        let mut wrapper = orx_test_support::Process::spawn(
            crate::process::command(binary)
                .arg("wrap")
                .arg(&run_dir)
                .env("ORX_TEST_FAST_STOPS", "1"),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !run_dir.join("ready").exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let identity: orx_host_proto::ProcessIdentity =
            orx_host::read_json(&run_dir.join("wrapper.json")).unwrap();
        assert!(!store.slim_run(&id).unwrap());
        assert!(run_dir.join("repo/.venv").is_dir());
        assert_eq!(store.count_active_runs().unwrap(), 1);
        std::fs::remove_file(dir.join("submission-handles").join(format!("{id}.json"))).unwrap();
        store
            .conn
            .execute("DELETE FROM runs WHERE id=?1", [&id])
            .unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(run_dir.join("repo/.venv").is_dir());
        assert!(run_dir.join("repo/output").is_file());
        assert_eq!(store.count_active_runs().unwrap(), 1);
        let mut elsewhere = identity.clone();
        elsewhere.node.push_str("-elsewhere");
        orx_host::publish(&run_dir.join("wrapper.json"), &elsewhere).unwrap();
        assert!(store.slim_run(&id).is_err());
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(run_dir.join("repo/.venv").is_dir());
        assert_eq!(store.count_active_runs().unwrap(), 1);
        orx_host::publish(&run_dir.join("wrapper.json"), &identity).unwrap();
        orx_host::wrapper::cancel(&run_dir).unwrap();
        wrapper.wait(Duration::from_secs(5)).unwrap();
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn orphan_cleanup_preserves_live_local_runs_and_remote_recovery_handles() {
        let dir = std::env::temp_dir().join(format!("orx-live-orphan-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let live = uuid::Uuid::new_v4().to_string();
        let remote = uuid::Uuid::new_v4().to_string();
        let digest = "d".repeat(64);
        project_with_run_artifacts(&store, "live", &live, &digest);
        let run_dir = dir.join("local-runs").join(&live);
        std::fs::write(run_dir.join("pid"), std::process::id().to_string()).unwrap();
        let remote_handle = dir
            .join("submission-handles")
            .join(format!("{remote}.json"));
        std::fs::write(
            &remote_handle,
            serde_json::json!({"kind": "ssh_job", "sourceDigest": "e".repeat(64)}).to_string(),
        )
        .unwrap();
        let remote_snapshot = dir
            .join("source-snapshots")
            .join(format!("{}.tar", "e".repeat(64)));
        std::fs::write(&remote_snapshot, "provider recovery source").unwrap();
        store.conn.execute("DELETE FROM runs", []).unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(run_dir.join("repo/output").is_file());
        assert!(dir.join("run-logs").join(format!("{live}.log")).is_file());
        assert!(dir
            .join("source-snapshots")
            .join(format!("{digest}.tar"))
            .is_file());
        project_with_run_artifacts(&store, "deleting", "deleted-run", &digest);
        store.delete_local_project("deleting").unwrap();
        assert!(dir
            .join("source-snapshots")
            .join(format!("{digest}.tar"))
            .is_file());
        project_with_run_artifacts(&store, "deleting-remote", "deleted-remote", &"e".repeat(64));
        store.delete_local_project("deleting-remote").unwrap();
        assert!(remote_snapshot.is_file());
        std::fs::write(run_dir.join("exit_code"), "0").unwrap();
        // The pid is this test process, whose own group the probe would read as live.
        std::fs::remove_file(run_dir.join("pid")).unwrap();
        let path = dir.join("run-logs").join(format!("{live}.supervisor.lock"));
        let mut supervisor = open_lifecycle_lock_at(&path).unwrap();
        let guard = supervisor.write().unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(run_dir.is_dir());
        drop(guard);
        drop(supervisor);
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(!run_dir.exists());
        assert!(!dir
            .join("source-snapshots")
            .join(format!("{digest}.tar"))
            .exists());
        assert!(!path.exists());
        assert!(remote_handle.is_file());
        assert!(remote_snapshot.is_file());
        assert!(!dir
            .join("run-logs")
            .join(format!("{remote}.supervisor.lock"))
            .exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn orphan_cleanup_keeps_a_finished_run_while_its_process_group_lives() {
        let dir = std::env::temp_dir().join(format!("orx-group-orphan-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let run = uuid::Uuid::new_v4().to_string();
        project_with_run_artifacts(&store, "cancelled", &run, &"f".repeat(64));
        let run_dir = dir.join("local-runs").join(&run);
        let mut survivor = crate::jobs::localbox::spawn_lingering_group(&run_dir);
        std::fs::write(run_dir.join("exit_code"), "143").unwrap();
        let refused = store.delete_local_project("cancelled").unwrap_err();
        assert!(
            refused.to_string().contains("still has running processes"),
            "{refused}"
        );
        store.conn.execute("DELETE FROM runs", []).unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(run_dir.join("repo/output").is_file());
        survivor.kill().unwrap();
        survivor.wait().unwrap();
        store.cleanup_orphaned_run_artifacts().unwrap();
        assert!(!run_dir.exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_deletion_cleans_run_artifacts_and_only_unreferenced_snapshots() {
        let dir =
            std::env::temp_dir().join(format!("orx-delete-artifacts-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let shared = "a".repeat(64);
        project_with_run_artifacts(&store, "first", "run-first", &shared);
        project_with_run_artifacts(&store, "second", "run-second", &shared);
        let zip = dir
            .join("source-snapshots")
            .join(format!("{}.zip", "f".repeat(64)));
        std::fs::write(
            &zip,
            "Ray upload package with a different digest from the TAR",
        )
        .unwrap();
        store.delete_local_project("first").unwrap();
        assert!(!zip.exists());
        for name in [
            "local-runs/run-first",
            "run-logs/run-first.log",
            "submission-handles/run-first.json",
        ] {
            assert!(!dir.join(name).exists(), "{name}");
        }
        assert!(store.get_run("run-first").unwrap().is_none());
        assert!(store.get_local_project("first").unwrap().is_none());
        for name in [
            "local-runs/run-second/repo/output",
            "run-logs/run-second.log",
            "submission-handles/run-second.json",
            "repo-first/keep",
            "repo-second/keep",
        ] {
            assert!(dir.join(name).is_file(), "{name}");
        }
        let snapshot = dir.join("source-snapshots").join(format!("{shared}.tar"));
        let staging = dir.join("source-snapshots").join(format!("{shared}.hf"));
        assert!(snapshot.is_file());
        assert!(staging.is_dir());
        // Partial previous cleanup and retries are harmless.
        std::fs::remove_file(dir.join("run-logs/run-second.log")).unwrap();
        store.delete_local_project("second").unwrap();
        store.delete_local_project("second").unwrap();
        assert!(!snapshot.exists());
        assert!(!staging.exists());
        assert!(!dir.join("local-runs/run-second").exists());
        assert!(!dir.join("submission-handles/run-second.json").exists());
        assert!(dir.join("repo-second/keep").is_file());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_deletion_preserves_active_runs_and_retries_after_staging() {
        let dir = std::env::temp_dir().join(format!("orx-delete-busy-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        project_with_run_artifacts(&store, "project", "run", &"b".repeat(64));
        store
            .conn
            .execute("UPDATE runs SET status = 'running'", [])
            .unwrap();
        assert!(store.delete_local_project("project").is_err());
        assert!(store.get_run("run").unwrap().is_some());
        assert!(dir.join("local-runs/run/repo/output").exists());
        store
            .conn
            .execute("UPDATE runs SET status = 'done'", [])
            .unwrap();
        let staging_lock = store.run_artifacts_lock().unwrap();
        let staging = staging_lock.read().unwrap();
        assert!(store
            .delete_local_project("project")
            .unwrap_err()
            .to_string()
            .contains("busy"));
        assert!(store.get_local_project("project").unwrap().is_some());
        assert!(dir.join("local-runs/run/repo/output").exists());
        drop(staging);
        drop(staging_lock);
        let lock_path = dir.join("run-artifacts.lock");
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let lock = open_lifecycle_lock_at(&lock_path).unwrap();
            let _staging = lock.read().unwrap();
            held_tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });
        held_rx.recv().unwrap();
        let started = std::time::Instant::now();
        // Deletion waits out a submission that finishes within RUN_ARTIFACTS_WAIT.
        store.delete_local_project("project").unwrap();
        assert!(started.elapsed() >= Duration::from_millis(200));
        holder.join().unwrap();
        assert!(!dir.join("local-runs/run").exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn project_deletion_removes_artifacts_through_linked_roots() {
        let dir = std::env::temp_dir().join(format!("orx-linked-roots-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        project_with_run_artifacts(&store, "project", "run", &"e".repeat(64));
        let external = dir.join("external");
        std::fs::create_dir_all(&external).unwrap();
        for name in ["local-runs", "submission-handles"] {
            std::fs::rename(dir.join(name), external.join(name)).unwrap();
            std::os::unix::fs::symlink(external.join(name), dir.join(name)).unwrap();
        }

        store.delete_local_project("project").unwrap();

        assert!(!external.join("local-runs/run").exists());
        assert!(!external.join("submission-handles/run.json").exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn refused_project_deletion_never_reaches_the_native_purge() {
        let dir = std::env::temp_dir().join(format!("orx-delete-order-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        project_with_run_artifacts(&store, "project", "run", &"e".repeat(64));
        let mut session = chat_session_fixture("chat");
        session.project_id = "project".into();
        session.harness = "opencode".into();
        session.native_session_id = Some("native".into());
        store.create_chat_session(&session).unwrap();
        let purged = std::cell::Cell::new(false);
        let purge = || {
            purged.set(true);
            Ok(())
        };
        let refuse = |reason: &str| {
            let error = store
                .delete_local_project_with("project", Some(purge))
                .unwrap_err();
            assert!(error.to_string().contains(reason), "{error}");
            assert!(!purged.get());
            assert!(store.get_chat_session("chat").unwrap().is_some());
        };

        store
            .conn
            .execute("UPDATE runs SET status = 'running'", [])
            .unwrap();
        refuse("in flight");
        store
            .conn
            .execute("UPDATE runs SET status = 'done'", [])
            .unwrap();

        let staging_lock = store.run_artifacts_lock().unwrap();
        let staging = staging_lock.read().unwrap();
        refuse("busy");
        drop(staging);

        let moving = store.acquire_data_dir_move_lock().unwrap();
        assert!(store.claim_data_dir_move("move").unwrap());
        refuse("being moved");
        store.release_data_dir_move("move").unwrap();
        drop(moving);

        let native_deletion = store.lock_native_deletion().unwrap();
        refuse("native chat deletion");
        drop(native_deletion);
        assert!(dir.join("local-runs/run/repo/output").is_file());

        std::fs::rename(dir.join("local-runs"), dir.join("outside")).unwrap();
        std::fs::write(dir.join("local-runs"), "").unwrap();
        refuse("Refusing artifact cleanup");
        std::fs::remove_file(dir.join("local-runs")).unwrap();
        std::fs::rename(dir.join("outside"), dir.join("local-runs")).unwrap();

        assert!(store
            .delete_local_project_with("project", Some(|| Err(anyhow!("purge failed"))))
            .is_err());
        assert!(store.get_chat_session("chat").unwrap().is_some());

        store
            .delete_local_project_with(
                "project",
                Some(|| {
                    assert!(store.run_artifacts_lock()?.try_read().is_err());
                    assert!(store.lock_native_deletion().is_err());
                    purge()
                }),
            )
            .unwrap();
        assert!(purged.get());
        assert!(store.get_chat_session("chat").unwrap().is_none());
        assert!(store.get_local_project("project").unwrap().is_none());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleting_chats_removes_only_attachments_no_other_message_shows() {
        let dir = std::env::temp_dir().join(format!("orx-attachments-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        project_with_run_artifacts(&store, "project", "run", &"a".repeat(64));
        let attachments = dir.join(crate::local::chat::ATTACHMENTS_DIR);
        std::fs::create_dir_all(&attachments).unwrap();
        let chats: [(&str, &str, &[&str]); 3] = [
            (
                "first",
                "project",
                &["att-1__own.png", "att-2__shared.pdf", "keep.txt"],
            ),
            ("second", "project", &["att-3__own.png"]),
            ("other", "elsewhere", &["att-2__shared.pdf"]),
        ];
        for (chat, project, files) in chats {
            let mut session = chat_session_fixture(chat);
            session.project_id = project.into();
            store.create_chat_session(&session).unwrap();
            let parts = files
                .iter()
                .enumerate()
                .map(|(i, file)| serde_json::json!({ "id": format!("img{i}"), "type": "image", "text": file }))
                .collect::<Vec<_>>();
            store
                .upsert_chat_message(&StoredChatMessage {
                    id: format!("msg_{chat}"),
                    session_id: chat.into(),
                    role: "user".into(),
                    parts_json: serde_json::Value::from(parts).to_string(),
                    created_at: 1,
                    completed_at: None,
                    parent_id: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                    native_harness: None,
                })
                .unwrap();
        }
        for file in [
            "att-1__own.png",
            "att-2__shared.pdf",
            "att-3__own.png",
            "keep.txt",
        ] {
            std::fs::write(attachments.join(file), "bytes").unwrap();
        }

        store.delete_chat_session("first").unwrap();
        assert!(!attachments.join("att-1__own.png").exists());
        assert!(attachments.join("att-2__shared.pdf").exists());
        store.delete_local_project("project").unwrap();
        assert!(!attachments.join("att-3__own.png").exists());
        assert!(attachments.join("att-2__shared.pdf").exists());
        store.delete_chat_session("other").unwrap();
        assert!(!attachments.join("att-2__shared.pdf").exists());
        assert!(attachments.join("keep.txt").exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn attachment_sweep_spares_shown_recent_and_foreign_files() {
        let dir =
            std::env::temp_dir().join(format!("orx-attachment-sweep-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        assert!(!store.cleanup_orphaned_chat_attachments().unwrap());
        store
            .create_chat_session(&chat_session_fixture("chat"))
            .unwrap();
        store
            .upsert_chat_message(&StoredChatMessage {
                id: "msg".into(),
                session_id: "chat".into(),
                role: "user".into(),
                parts_json: r#"[{"id":"img0","type":"image","text":"att-1__shown.png"}]"#.into(),
                created_at: 1,
                completed_at: None,
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: None,
                native_harness: None,
            })
            .unwrap();
        let attachments = dir.join(crate::local::chat::ATTACHMENTS_DIR);
        std::fs::create_dir_all(&attachments).unwrap();
        let old = std::time::SystemTime::now() - 2 * CHAT_ATTACHMENT_SETTLE;
        for file in ["att-1__shown.png", "att-2__orphan.png", "notes.txt"] {
            let file = std::fs::File::create(attachments.join(file)).unwrap();
            file.set_modified(old).unwrap();
        }
        std::fs::write(attachments.join("att-3__unsent.png"), "bytes").unwrap();

        let moving = store.acquire_data_dir_move_lock().unwrap();
        assert!(store.claim_data_dir_move("move").unwrap());
        store.cleanup_orphaned_chat_attachments().unwrap();
        assert!(attachments.join("att-2__orphan.png").exists());
        store.release_data_dir_move("move").unwrap();
        drop(moving);

        let hold = store.hold_off_data_dir_move().unwrap().unwrap();
        assert!(store.acquire_data_dir_move_lock().is_err());
        drop(hold);

        assert!(!store.cleanup_orphaned_chat_attachments().unwrap());
        assert!(!attachments.join("att-2__orphan.png").exists());
        for kept in ["att-1__shown.png", "att-3__unsent.png", "notes.txt"] {
            assert!(attachments.join(kept).exists(), "{kept}");
        }

        let orphans = ["att-4__a.png", "att-5__b.png", "att-6__c.png"];
        for file in orphans {
            let file = std::fs::File::create(attachments.join(file)).unwrap();
            file.set_modified(old).unwrap();
        }
        let passes = (0..=orphans.len())
            .take_while(|_| {
                store
                    .cleanup_orphaned_chat_attachments_within(Duration::ZERO)
                    .unwrap()
            })
            .count();
        assert_eq!(
            passes,
            orphans.len(),
            "each exhausted pass still removes one"
        );
        assert!(orphans.iter().all(|file| !attachments.join(file).exists()));
        assert!(attachments.join("att-1__shown.png").exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn project_deletion_rejects_unsafe_artifact_identifiers() {
        let dir = std::env::temp_dir().join(format!("orx-delete-unsafe-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        project_with_run_artifacts(&store, "project", "run", &"c".repeat(64));
        store
            .conn
            .execute("UPDATE runs SET id = '../repo-project'", [])
            .unwrap();
        assert!(store.delete_local_project("project").is_err());
        store
            .conn
            .execute(
                "UPDATE runs SET id = 'run', backend_json = ?1",
                [r#"{"kind":"local_job","sourceDigest":"../repo-project"}"#],
            )
            .unwrap();
        // Snapshot removal works from directory names, so a bad digest cannot steer it.
        store.delete_local_project("project").unwrap();
        assert!(dir.join("repo-project/keep").is_file());
        assert!(!dir.join("local-runs/run").exists());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lifecycle_lock_keeps_legacy_inode() {
        let config_dir = std::env::temp_dir().join(format!("orx-lock-{}", uuid::Uuid::new_v4()));
        let lock = open_lifecycle_lock_in(&config_dir).unwrap();
        let legacy = config_dir.join("orx.lifecycle.lock");
        let path = lifecycle_lock_path_in(&config_dir);
        assert!(same_file::is_same_file(legacy, &path).unwrap());
        assert!(path.parent().unwrap().is_dir());
        assert!(!config_dir
            .join("credentials.json")
            .starts_with(path.parent().unwrap()));
        drop(lock);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn lifecycle_lock_rejects_replaced_entry() {
        let config_dir = std::env::temp_dir().join(format!("orx-lock-{}", uuid::Uuid::new_v4()));
        let lock = open_lifecycle_lock_in(&config_dir).unwrap();
        let path = lifecycle_lock_path_in(&config_dir);
        drop(lock);
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        assert!(open_lifecycle_lock_in(&config_dir).is_err());
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn lifecycle_lock_uses_legacy_when_dedicated_dir_cannot_be_created() {
        let config_dir = std::env::temp_dir().join(format!("orx-lock-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("locks"), "not a directory").unwrap();
        assert!(open_lifecycle_lock_in(&config_dir).is_ok());
        std::fs::remove_dir_all(config_dir).unwrap();
    }

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
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn workspace_state_roundtrips_without_touching_project_activity_or_preferences() {
        let dir = std::env::temp_dir().join(format!("orx-workspace-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["first", "second"] {
            store
                .create_local_project(&LocalProject {
                    id: id.into(),
                    name: id.into(),
                    slug: id.into(),
                    github_owner: String::new(),
                    github_repo: String::new(),
                    github_sync_enabled: false,
                    baseline_branch: "main".into(),
                    repo_path: dir.join(id).to_string_lossy().into_owned(),
                    run_command: None,
                    paper_id: None,
                    created_at: 1,
                    updated_at: 2,
                })
                .unwrap();
        }
        let value = serde_json::json!({
            "version": 1,
            "lastLocation": "/projects/first/tasks/new",
            "lastTaskId": null,
            "tasks": {"new": {
                "tabs": [
                    {"kind":"home", "view":"files"},
                    {"kind":"experiment", "experimentId":"experiment", "view":"terminal", "runId":"run"},
                    {"kind":"file", "path":"paper.tex", "source":"repo", "sessionId":"session", "ref":"main", "line":12},
                    {"kind":"code", "experimentId":"experiment", "branch":"main", "view":"changes"},
                    {"kind":"plan", "sessionId":"session", "promptId":"prompt"},
                    {"kind":"subagent", "sessionId":"session", "spawnPartId":"part"}
                ],
                "active":{"kind":"home", "view":"files"}, "previewKey":"file:paper.tex",
                "history":["home", "file:paper.tex"], "expanded":{"files":["src"]},
                "scroll":{"file:paper.tex":{"top":23.5,"left":2.0}}, "sourceModes":{"file:paper.tex":true},
                "filesView":"changes", "scope":"agent", "panelMax":true,
                "treeViewport":{"x":-24.0,"y":18.0,"zoom":1.4}
            }}
        });
        let workspace: WorkspaceState = serde_json::from_value(value.clone()).unwrap();
        assert!(store.project_workspace_state("first").unwrap().is_none());
        assert!(store
            .set_project_workspace_state("first", &workspace)
            .unwrap());
        assert!(!store
            .set_project_workspace_state("missing", &workspace)
            .unwrap());
        assert!(store.project_workspace_state("second").unwrap().is_none());
        assert_eq!(
            store
                .get_local_project("first")
                .unwrap()
                .unwrap()
                .updated_at,
            2
        );
        let preferences = store.ui_state().unwrap();
        let global: GlobalWorkspaceState = serde_json::from_value(serde_json::json!({
            "lastLocation":"/projects/first/settings/storage", "railOpen":false,
            "panelWidth":620, "railWidth":280, "experimentsView":"table"
        }))
        .unwrap();
        store.set_global_workspace_state(&global).unwrap();
        assert_eq!(
            store.ui_state().unwrap().preferred_agent,
            preferences.preferred_agent
        );
        assert_eq!(
            store.ui_state().unwrap().onboarding_completed,
            preferences.onboarding_completed
        );
        store.set_tour_completed(true).unwrap();
        store.set_onboarding_completed(true).unwrap();
        store
            .set_preferred_agent(&StoredAgentSelection {
                harness: "codex".into(),
                model: None,
                service_tier: None,
                permission_mode: None,
                reasoning_level: None,
            })
            .unwrap();
        assert_eq!(store.ui_state().unwrap().workspace, Some(global.clone()));
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(store.project_workspace_state("first").unwrap()).unwrap(),
            value
        );
        assert_eq!(store.ui_state().unwrap().workspace, Some(global));
        for corrupt in [
            "not json",
            "{\"version\":2,\"lastLocation\":null,\"tasks\":{}}",
        ] {
            store
                .conn
                .execute(
                    "UPDATE local_projects SET workspace_state_json = ?1 WHERE id = 'first'",
                    params![corrupt],
                )
                .unwrap();
            assert!(store.project_workspace_state("first").unwrap().is_none());
        }
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
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
                workspace: None,
                preferred_autonomy: None,
                nested_delegation: false,
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
        store.set_preferred_autonomy(Autonomy::Copilot).unwrap();

        assert_eq!(
            store.ui_state().unwrap(),
            StoredUiState {
                onboarding_completed: true,
                tour_completed: true,
                preferred_agent: Some(selection),
                workspace: None,
                preferred_autonomy: Some(Autonomy::Copilot),
                nested_delegation: false,
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
                completed_at: None,
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: None,
                native_harness: None,
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
    fn named_model_history_is_scoped_to_the_native_thread() {
        let dir = std::env::temp_dir().join(format!("orx-store-model-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let mut turn = chat_turn_fixture("one", "client-1");
        turn.settings_json = r#"{"model":"gpt-6-astra-canary-invalid"}"#.into();
        store.admit_chat_turn(None, &turn).unwrap();
        store
            .upsert_chat_message(&StoredChatMessage {
                id: turn.assistant_message_id,
                session_id: "chat_1".into(),
                role: "assistant".into(),
                parts_json: "[]".into(),
                created_at: 2,
                completed_at: None,
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: Some("old-thread".into()),
                native_harness: None,
            })
            .unwrap();
        assert!(store
            .chat_native_thread_has_named_model("chat_1", "old-thread")
            .unwrap());
        assert!(!store
            .chat_native_thread_has_named_model("chat_1", "fresh-thread")
            .unwrap());
        let _ = std::fs::remove_dir_all(dir);
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
            completed_at: None,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
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
    fn submission_lookup_uses_client_ids_for_queued_and_completed_turns() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-submissions-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        store
            .insert_queued_chat_message(&StoredQueuedChatMessage {
                id: "q_queue-id".into(),
                session_id: "chat_1".into(),
                client_turn_id: "ct_queue-id".into(),
                request_hash: "hash".into(),
                payload_json: "{}".into(),
                created_at: 1,
            })
            .unwrap();
        assert!(store.has_chat_submission("chat_1", "ct_queue-id").unwrap());
        assert!(!store.has_chat_submission("chat_1", "q_queue-id").unwrap());
        assert!(!store.has_chat_submission("another", "ct_queue-id").unwrap());
        let mut turn = chat_turn_fixture("finished", "ct_finished");
        turn.state = "completed".into();
        store.admit_chat_turn(None, &turn).unwrap();
        assert!(store.has_chat_submission("chat_1", "ct_finished").unwrap());
        assert!(store.claim_steer("chat_1", "ct_steer", "hash").unwrap());
        assert!(!store.has_chat_submission("chat_1", "ct_steer").unwrap());
        store.finish_steer("chat_1", "ct_steer").unwrap();
        assert!(store.has_chat_submission("chat_1", "ct_steer").unwrap());
        assert!(!store.has_chat_submission("another", "ct_steer").unwrap());
        store.delete_queued_chat_message("q_queue-id").unwrap();
        assert!(!store.has_chat_submission("chat_1", "ct_queue-id").unwrap());
        let _ = std::fs::remove_dir_all(dir);
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
                params![expired_before(CHAT_TURN_LEASE_TTL) - 1],
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
    fn chat_completion_time_survives_reload_and_partial_updates() {
        let dir = std::env::temp_dir().join(format!("orx-chat-time-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        let mut message = StoredChatMessage {
            id: "answer".into(),
            session_id: "chat_1".into(),
            role: "assistant".into(),
            parts_json: "[]".into(),
            created_at: 1000,
            completed_at: Some(165000),
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
        };
        store.upsert_chat_message(&message).unwrap();
        message.completed_at = None;
        store.upsert_chat_message(&message).unwrap();
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        assert_eq!(
            store
                .get_chat_message("answer")
                .unwrap()
                .unwrap()
                .completed_at,
            Some(165000)
        );
        assert_eq!(
            store.list_chat_messages("chat_1").unwrap()[0].completed_at,
            Some(165000)
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
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
                    completed_at: None,
                    parent_id: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                    native_harness: None,
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
            completed_at: None,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
        };
        store.admit_chat_turn(Some(&user), &turn).unwrap();
        assert!(store
            .fail_chat_turn("turn_1", "not_sent", "setup", "failed", Some("retry"))
            .unwrap());
        store
            .upsert_chat_message(&StoredChatMessage {
                id: turn.assistant_message_id.clone(),
                role: "assistant".into(),
                completed_at: Some(2000),
                ..user.clone()
            })
            .unwrap();
        assert!(store.reset_chat_turn_for_retry("turn_1", 3000).unwrap());
        assert_eq!(
            store
                .get_chat_message(&turn.assistant_message_id)
                .unwrap()
                .unwrap()
                .completed_at,
            None
        );
        assert_eq!(
            store
                .get_chat_message(&turn.assistant_message_id)
                .unwrap()
                .unwrap()
                .created_at,
            3000
        );
        assert_eq!(store.list_chat_messages("chat_1").unwrap().len(), 2);
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
        assert!(!store.reset_chat_turn_for_retry("turn_1", 3000).unwrap());
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
            completed_at: None,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
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
            completed_at: None,
            parent_id: None,
            base_native_session_id: Some("native_1".into()),
            result_native_session_id: None,
            native_harness: None,
        };
        store.admit_chat_turn(Some(&user), &turn).unwrap();
        let recovery = StoredChatMessage {
            id: turn.assistant_message_id.clone(),
            session_id: "chat_1".into(),
            role: "assistant".into(),
            parts_json: "[]".into(),
            created_at: 2,
            completed_at: None,
            parent_id: Some(user.id.clone()),
            base_native_session_id: Some("native_1".into()),
            result_native_session_id: None,
            native_harness: None,
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
    fn steer_and_turn_admission_cannot_claim_the_same_client_id() {
        let dir = std::env::temp_dir().join(format!("orx-steer-claim-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        assert!(store.claim_steer("chat_1", "ct1", "hash").unwrap());
        let turn = chat_turn_fixture("turn1", "ct1");
        assert!(matches!(
            store.admit_chat_turn(None, &turn).unwrap(),
            ChatTurnAdmission::Conflict
        ));
        let queued = StoredQueuedChatMessage {
            id: "q1".into(),
            session_id: "chat_1".into(),
            client_turn_id: "ct1".into(),
            request_hash: "hash".into(),
            payload_json: "{}".into(),
            created_at: 1,
        };
        assert!(store.insert_queued_chat_message(&queued).is_err());
        let other = chat_turn_fixture("turn2", "ct2");
        assert!(matches!(
            store.admit_chat_turn(None, &other).unwrap(),
            ChatTurnAdmission::Inserted
        ));
        assert!(!store.claim_steer("chat_1", "ct2", "hash").unwrap());
        let queued_first = StoredQueuedChatMessage {
            id: "q3".into(),
            session_id: "chat_1".into(),
            client_turn_id: "ct3".into(),
            request_hash: "hash".into(),
            payload_json: "{}".into(),
            created_at: 1,
        };
        store.insert_queued_chat_message(&queued_first).unwrap();
        assert!(!store.claim_steer("chat_1", "ct3", "hash").unwrap());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_steer_and_turn_admission_have_one_winner() {
        let dir = std::env::temp_dir().join(format!("orx-steer-race-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_1"))
            .unwrap();
        for i in 0..10 {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let steer_dir = dir.clone();
            let turn_dir = dir.clone();
            let steer_barrier = barrier.clone();
            let id = format!("client-{i}");
            let steer_id = id.clone();
            let turn_id = id.clone();
            let steer = std::thread::spawn(move || {
                let store = Store::open_at(steer_dir).unwrap();
                steer_barrier.wait();
                store.claim_steer("chat_1", &steer_id, "hash").unwrap()
            });
            let turn = std::thread::spawn(move || {
                let store = Store::open_at(turn_dir).unwrap();
                let turn = chat_turn_fixture(&format!("turn-{i}"), &turn_id);
                barrier.wait();
                matches!(
                    store.admit_chat_turn(None, &turn).unwrap(),
                    ChatTurnAdmission::Inserted
                )
            });
            assert_ne!(steer.join().unwrap(), turn.join().unwrap());
        }
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
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
            completed_at: None,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
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

    #[test]
    fn native_deletion_includes_inactive_branches_and_preserves_shared_sessions() {
        let dir = crate::local::git::TemporaryDirectory::new("orx-native-delete-refs").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        for (id, harness, native_id) in [
            ("delete", "opencode", "current"),
            ("keep", "claude-code", "shared"),
            ("cursor", "cursor", "current"),
        ] {
            let mut session = chat_session_fixture(id);
            session.harness = harness.into();
            session.native_session_id = Some(native_id.into());
            store.create_chat_session(&session).unwrap();
        }
        store
            .conn
            .execute_batch(
                "INSERT INTO chat_messages (id, session_id, role, parts_json, created_at,
                 base_native_session_id, result_native_session_id)
             VALUES ('branch', 'delete', 'assistant', '[]', 1, 'old', 'shared'),
                    ('other-branch', 'keep', 'assistant', '[]', 1, NULL, 'kept-branch');",
            )
            .unwrap();
        let (deleting, retained) = store
            .native_sessions_for_deletion(&["delete".into()], &["opencode", "claude-code"])
            .unwrap();
        assert_eq!(deleting, ["current".into(), "old".into()].into());
        assert_eq!(retained, ["kept-branch".into(), "shared".into()].into());
        let (deleting, retained) = store
            .native_sessions_for_deletion(
                &["delete".into(), "keep".into()],
                &["opencode", "claude-code"],
            )
            .unwrap();
        assert_eq!(
            deleting,
            [
                "current".into(),
                "old".into(),
                "kept-branch".into(),
                "shared".into()
            ]
            .into()
        );
        assert!(retained.is_empty());
    }

    #[test]
    fn native_ids_outside_skips_only_the_given_chats() {
        let dir = crate::local::git::TemporaryDirectory::new("orx-native-outside").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        for (id, native_id) in [("gone", "own"), ("kept", "other")] {
            let mut session = chat_session_fixture(id);
            session.native_session_id = Some(native_id.into());
            store.create_chat_session(&session).unwrap();
        }
        store
            .conn
            .execute_batch(
                "INSERT INTO chat_messages (id, session_id, role, parts_json, created_at,
                 base_native_session_id, result_native_session_id)
             VALUES ('a', 'gone', 'assistant', '[]', 1, 'own-base', 'own-result'),
                    ('b', 'kept', 'assistant', '[]', 1, 'other-base', 'other-result');",
            )
            .unwrap();
        let outside = store.native_ids_outside(&["gone".into()]).unwrap();
        assert_eq!(
            outside,
            ["other", "other-base", "other-result"]
                .map(String::from)
                .into()
        );
        assert_eq!(store.native_ids_outside(&[]).unwrap().len(), 6);
    }

    #[test]
    fn native_deletion_lock_serializes_shared_owners_across_processes() {
        const CHILD_DIR: &str = "ORX_TEST_NATIVE_DELETE_DIR";
        const CHILD_PHASE: &str = "ORX_TEST_NATIVE_DELETE_PHASE";
        if let Some(dir) = std::env::var_os(CHILD_DIR) {
            let store = Store::open_at(dir.into()).unwrap();
            if std::env::var(CHILD_PHASE).unwrap() == "blocked" {
                assert!(store.lock_native_deletion().is_err());
            } else {
                // A released lock lingers in children the parent's other tests forked, until
                // they exec.
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                let _lock = loop {
                    match store.lock_native_deletion() {
                        Ok(lock) => break lock,
                        Err(error) => assert!(std::time::Instant::now() < deadline, "{error}"),
                    }
                    std::thread::sleep(Duration::from_millis(1));
                };
                let (deleting, retained) = store
                    .native_sessions_for_deletion(&["second".into()], &["opencode", "claude-code"])
                    .unwrap();
                assert_eq!(deleting, ["shared".into()].into());
                assert!(retained.is_empty());
                store.delete_chat_session("second").unwrap();
            }
            return;
        }
        let dir = crate::local::git::TemporaryDirectory::new("orx-native-delete-lock").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        for id in ["first", "second"] {
            let mut session = chat_session_fixture(id);
            session.harness = "opencode".into();
            session.native_session_id = Some("shared".into());
            store.create_chat_session(&session).unwrap();
        }
        let run_child = |phase| {
            let output = crate::process::command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "store::tests::native_deletion_lock_serializes_shared_owners_across_processes",
                    "--nocapture",
                ])
                .env(CHILD_DIR, dir.path())
                .env(CHILD_PHASE, phase)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        };
        let lock = store.lock_native_deletion().unwrap();
        let (deleting, retained) = store
            .native_sessions_for_deletion(&["first".into()], &["opencode", "claude-code"])
            .unwrap();
        assert!(deleting.is_empty());
        assert_eq!(retained, ["shared".into()].into());
        run_child("blocked");
        store.delete_chat_session("first").unwrap();
        drop(lock);
        run_child("retry");
        let _lock = store.lock_native_deletion().unwrap();
        assert!(store.get_chat_session("second").unwrap().is_none());
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
            goal: None,
            autonomy: None,
            active_leaf_id: None,
            side_parent_session_id: None,
            side_parent_leaf_id: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn acp_first_turn_repairs_missing_state_without_redirecting_native_sessions() {
        let dir = std::env::temp_dir().join(format!("orx-acp-repair-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let mut session = chat_session_fixture("legacy-demo");
        session.harness = crate::local::acp::presets()[0].launch.id.clone();
        store.create_chat_session(&session).unwrap();
        store
            .conn
            .execute(
                "DELETE FROM acp_sessions WHERE session_id = ?1",
                [&session.id],
            )
            .unwrap();
        let repaired = store
            .ensure_acp_session_state(&session.id)
            .unwrap()
            .unwrap();
        assert_eq!(repaired.launch.id, session.harness);
        store
            .conn
            .execute(
                "UPDATE chat_sessions SET native_session_id = 'native' WHERE id = ?1",
                [&session.id],
            )
            .unwrap();
        store
            .conn
            .execute(
                "DELETE FROM acp_sessions WHERE session_id = ?1",
                [&session.id],
            )
            .unwrap();
        assert!(store.ensure_acp_session_state(&session.id).is_err());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn acp_side_chat_keeps_launch_snapshot_and_deletes_its_state() {
        let dir = std::env::temp_dir().join(format!("orx-acp-store-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let parent = chat_session_fixture("parent");
        store.create_chat_session(&parent).unwrap();
        let launch = crate::local::acp::Definition {
            id: format!("acp:{}", uuid::Uuid::new_v4()),
            name: "Saved agent".into(),
            executable: "original-agent".into(),
            arguments: vec!["original argument".into()],
            unknown: Default::default(),
        };
        store.conn.execute(
            "INSERT INTO acp_sessions(session_id, launch_json, configuration_json) VALUES (?1, ?2, ?3)",
            params![parent.id, serde_json::to_string(&launch).unwrap(), "[]"],
        ).unwrap();
        let mut side = chat_session_fixture("side");
        side.harness = launch.id.clone();
        side.side_parent_session_id = Some(parent.id.clone());
        store.create_chat_session(&side).unwrap();
        assert_eq!(
            store.acp_session_state("side").unwrap().unwrap().launch,
            launch
        );
        assert!(store
            .get_chat_session("side")
            .unwrap()
            .unwrap()
            .native_session_id
            .is_none());
        let configuration = serde_json::json!([{"id":"model", "currentValue":"native/model"}]);
        store.set_acp_configuration("side", &configuration).unwrap();
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        assert_eq!(
            store
                .acp_session_state("side")
                .unwrap()
                .unwrap()
                .configuration,
            configuration
        );
        store.delete_chat_session("side").unwrap();
        assert!(store.acp_session_state("side").unwrap().is_none());
        assert!(store.acp_session_state("parent").unwrap().is_some());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pending_input_summary_survives_reopen_and_follows_the_active_branch() {
        let dir = std::env::temp_dir().join(format!("orx-store-pending-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_pending"))
            .unwrap();
        for (id, parent_id, parts) in [
            ("user", None, serde_json::json!([])),
            (
                "question",
                Some("user"),
                serde_json::json!([{"type":"prompt","prompt":{"kind":"question","resolved":false}}]),
            ),
            (
                "retry",
                Some("user"),
                serde_json::json!([{"type":"text","text":"Done"}]),
            ),
        ] {
            store
                .upsert_chat_message(&StoredChatMessage {
                    id: id.into(),
                    session_id: "chat_pending".into(),
                    role: "assistant".into(),
                    parts_json: parts.to_string(),
                    parent_id: parent_id.map(str::to_string),
                    created_at: 1,
                    completed_at: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                    native_harness: None,
                })
                .unwrap();
        }
        store
            .set_chat_session_active_leaf("chat_pending", Some("question"))
            .unwrap();
        assert!(store
            .chat_session_awaiting_input("chat_pending", false)
            .unwrap());
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        assert!(store
            .chat_session_awaiting_input("chat_pending", false)
            .unwrap());
        store
            .set_chat_session_active_leaf("chat_pending", Some("retry"))
            .unwrap();
        assert!(!store
            .chat_session_awaiting_input("chat_pending", false)
            .unwrap());
        store
            .set_chat_session_active_leaf("chat_pending", Some("question"))
            .unwrap();
        store.conn.execute("UPDATE chat_messages SET parts_json = ?1 WHERE id = 'question'", params![r#"[{"type":"prompt","prompt":{"kind":"question","resolved":false,"nativeId":"rpc-1"}}]"#]).unwrap();
        assert!(!store
            .chat_session_awaiting_input("chat_pending", false)
            .unwrap());
        assert!(store
            .chat_session_awaiting_input("chat_pending", true)
            .unwrap());
        store
            .conn
            .execute(
                "UPDATE chat_messages SET parts_json = ?1 WHERE id = 'question'",
                params![r#"[{"type":"prompt","prompt":{"kind":"question","resolved":true}}]"#],
            )
            .unwrap();
        assert!(!store
            .chat_session_awaiting_input("chat_pending", true)
            .unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chat_session_setters_survive_a_reopen() {
        let dir = std::env::temp_dir().join(format!("orx-store-setters-{}", uuid::Uuid::new_v4()));
        const USAGE: &str = r#"{"usedTokens":27564,"contextWindow":200000}"#;
        let mut session = chat_session_fixture("chat");
        session.autonomy = Some("copilot".into());
        Store::open_at(dir.clone())
            .unwrap()
            .create_chat_session(&session)
            .unwrap();
        let read = || {
            let store = Store::open_at(dir.clone()).unwrap();
            store.get_chat_session("chat").unwrap().unwrap()
        };
        let fresh = read();
        assert_eq!(fresh.autonomy.as_deref(), Some("copilot"));
        assert_eq!(fresh.context_usage_json, None);
        assert_eq!((fresh.title, fresh.title_source), (None, None));
        assert!(!fresh.plan_reset_pending);

        type Case = (fn(&Store), fn(&StoredChatSession) -> bool);
        let cases: [Case; 5] = [
            (
                |store| {
                    store
                        .set_chat_session_autonomy("chat", Autonomy::Agentic)
                        .unwrap()
                },
                |chat| chat.autonomy.as_deref() == Some("agentic"),
            ),
            (
                |store| store.set_chat_session_context_usage("chat", USAGE).unwrap(),
                |chat| chat.context_usage_json.as_deref() == Some(USAGE),
            ),
            (
                |store| {
                    store
                        .set_chat_session_title("chat", "First line…", "fallback")
                        .unwrap()
                },
                |chat| {
                    chat.title.as_deref() == Some("First line…")
                        && chat.title_source.as_deref() == Some("fallback")
                },
            ),
            (
                |store| {
                    store
                        .set_chat_session_plan_state("chat", false, true)
                        .unwrap()
                },
                |chat| chat.plan_reset_pending,
            ),
            (
                |store| store.clear_chat_session_plan_reset("chat").unwrap(),
                |chat| !chat.plan_reset_pending,
            ),
        ];
        for (i, (set, check)) in cases.into_iter().enumerate() {
            set(&Store::open_at(dir.clone()).unwrap());
            assert!(check(&read()), "case {i}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn side_chats_stay_out_of_history_and_share_their_parents_worktree() {
        let dir = std::env::temp_dir().join(format!("orx-store-sidechat-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_chat_session(&chat_session_fixture("chat_parent"))
            .unwrap();
        let mut side = chat_session_fixture("chat_side");
        side.side_parent_session_id = Some("chat_parent".into());
        store.create_chat_session(&side).unwrap();

        let all: Vec<String> = store
            .list_all_chat_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(all, ["chat_parent"]);
        assert_eq!(store.side_chats_of("chat_parent").unwrap(), ["chat_side"]);
        assert_eq!(
            store.chat_worktree_owner("chat_side").unwrap(),
            "chat_parent"
        );
        let mut nested = chat_session_fixture("chat_nested");
        nested.side_parent_session_id = Some("chat_side".into());
        store.create_chat_session(&nested).unwrap();
        assert_eq!(
            store.chat_worktree_owner("chat_nested").unwrap(),
            "chat_parent"
        );
        store.delete_chat_session("chat_nested").unwrap();
        assert_eq!(store.chat_worktree_owner("chat_gone").unwrap(), "chat_gone");
        assert!(store.delete_chat_session("chat_parent").is_err());
        assert!(store.get_chat_session("chat_parent").unwrap().is_some());
        store.delete_chat_session("chat_side").unwrap();
        store.delete_chat_session("chat_parent").unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_resume_picker_sees_every_project_newest_first() {
        let dir = std::env::temp_dir().join(format!("orx-store-allchats-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();

        for (id, project, updated_at) in [
            ("chat_old", "proj_1", 10),
            ("chat_other_project", "proj_2", 30),
            ("chat_middle", "proj_1", 20),
        ] {
            let mut session = chat_session_fixture(id);
            session.project_id = project.into();
            session.updated_at = updated_at;
            store.create_chat_session(&session).unwrap();
        }

        let all = store.list_all_chat_sessions().unwrap();
        assert_eq!(
            all.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["chat_other_project", "chat_middle", "chat_old"]
        );
        assert_eq!(
            store.list_chat_sessions_by_project("proj_1").unwrap().len(),
            2
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sidebar_pages_across_projects_with_ties_and_archive_filter() {
        let dir = std::env::temp_dir().join(format!("orx-store-sidebar-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for n in 0..65 {
            let mut session = chat_session_fixture(&format!("chat_{n:03}"));
            session.project_id = format!("proj_{}", n % 3);
            session.updated_at = 10;
            session.archived = n >= 60;
            store.create_chat_session(&session).unwrap();
        }
        let mut side = chat_session_fixture("side");
        side.side_parent_session_id = Some("chat_000".into());
        side.updated_at = 20;
        store.create_chat_session(&side).unwrap();
        let first = store.list_sidebar_chat_sessions(Some(false), None).unwrap();
        assert_eq!(first.len(), 51);
        assert_eq!(first[0].id, "chat_059");
        let cursor = &first[49];
        let second = store
            .list_sidebar_chat_sessions(Some(false), Some((cursor.updated_at, &cursor.id)))
            .unwrap();
        assert_eq!(second.len(), 10);
        assert_eq!(second[0].id, "chat_009");
        assert_eq!(second.last().unwrap().id, "chat_000");
        assert_eq!(
            store
                .list_sidebar_chat_sessions(Some(true), None)
                .unwrap()
                .len(),
            5
        );
        assert_eq!(
            store.list_sidebar_chat_sessions(None, None).unwrap()[0].id,
            "chat_064"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_spawns_migrate_only_unadmitted_opening_messages() {
        let dir =
            std::env::temp_dir().join(format!("orx-spawn-migration-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store.conn.execute_batch("CREATE TABLE chat_spawns (session_id TEXT PRIMARY KEY, prompt TEXT NOT NULL, requested_at INTEGER NOT NULL, state TEXT NOT NULL)").unwrap();
        for (id, state) in [
            ("pending", "pending"),
            ("brief", "starting"),
            ("admitted", "starting"),
            ("running", "running"),
            ("waking", "waking"),
            ("done", "done"),
        ] {
            store
                .create_chat_session(&chat_session_fixture(id))
                .unwrap();
            store
                .conn
                .execute(
                    "INSERT INTO chat_spawns VALUES (?1, 'Opening task', 1, ?2)",
                    params![id, state],
                )
                .unwrap();
        }
        let parts =
            serde_json::to_string(&[crate::local::chat::WirePart::text("part", "Opening task")])
                .unwrap();
        store.conn.execute("INSERT INTO chat_messages (id, session_id, role, parts_json, created_at) VALUES ('brief-user', 'brief', 'user', ?1, 1)", params![parts]).unwrap();
        store.conn.execute("INSERT INTO chat_turns (id, session_id, assistant_message_id, client_turn_id, request_hash, prepared_input, settings_json, state, delivery_state, created_at, updated_at) VALUES ('turn', 'admitted', 'assistant', 'ct', 'hash', 'task', '{}', 'interrupted', 'accepted', 1, 1)", []).unwrap();
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        let queued = store.list_queued_chat_messages().unwrap();
        assert_eq!(queued.len(), 2);
        let brief = queued.iter().find(|m| m.session_id == "brief").unwrap();
        assert_eq!(brief.client_turn_id, "legacy_spawn_brief");
        let payload: serde_json::Value = serde_json::from_str(&brief.payload_json).unwrap();
        assert_eq!(payload["transcriptText"], "");
        assert_eq!(store.list_chat_messages("brief").unwrap().len(), 1);
        assert_eq!(store.list_all_chat_sessions().unwrap().len(), 6);
        let exists: bool = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'chat_spawns')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!exists);
        drop(store);
        assert_eq!(
            Store::open_at(dir.clone())
                .unwrap()
                .list_queued_chat_messages()
                .unwrap()
                .len(),
            2
        );
        std::fs::remove_dir_all(dir).unwrap();
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
            cancel_reason: None,
            chat_session_id: chat_session_id.map(str::to_string),
        }
    }

    /// Every (from, to) pair, checked against the lifecycle diagram on
    /// [`RunStatus`] rather than the implementation, so this fails if
    /// `can_transition_to` and the documented state machine ever drift apart.
    #[test]
    fn run_status_transition_table_matches_the_documented_lifecycle() {
        use RunStatus::*;
        let legal: &[(RunStatus, RunStatus)] = &[
            (Starting, Starting),
            (Starting, Running),
            (Starting, Done),
            (Starting, Failed),
            (Starting, Cancelled),
            (Starting, Lost),
            (Running, Running),
            (Running, Done),
            (Running, Failed),
            (Running, Cancelled),
            (Running, Lost),
        ];
        for from in RunStatus::ALL {
            for to in RunStatus::ALL {
                let expected = legal.contains(&(from, to));
                assert_eq!(
                    from.can_transition_to(to),
                    expected,
                    "{from} -> {to} should be {}",
                    if expected { "legal" } else { "illegal" }
                );
            }
        }
    }

    #[test]
    fn status_writers_enforce_every_transition_and_preserve_terminal_outcomes() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-transitions-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for from in RunStatus::ALL {
            for to in RunStatus::ALL {
                for upsert in [false, true] {
                    let id = format!("{from}-{to}-{upsert}");
                    let mut original = run_fixture(&id, from.as_str(), Some("owner"));
                    original.result_markdown = Some("original result".into());
                    original.ended_at = from.is_terminal().then_some(10);
                    original.exit_code = from.is_terminal().then_some(0);
                    original.cancel_requested = true;
                    store.upsert_run(&original).unwrap();
                    let before = store.get_run(&id).unwrap().unwrap();
                    let expected = from == RunStatus::Starting
                        || (from == RunStatus::Running && to != RunStatus::Starting);
                    if upsert {
                        let mut incoming = run_fixture(&id, to.as_str(), None);
                        incoming.backend_json = "{\"job_id\":\"late-handle\"}".into();
                        incoming.ended_at = Some(20);
                        incoming.exit_code = Some(1);
                        incoming.result_markdown = Some("new result".into());
                        store.upsert_run(&incoming).unwrap();
                    } else {
                        assert_eq!(
                            store.update_status(&id, to, Some(20), Some(1)).unwrap(),
                            expected,
                        );
                    }
                    let after = store.get_run(&id).unwrap().unwrap();
                    assert_eq!(
                        after.status,
                        if expected { to } else { from }.as_str(),
                        "{id}"
                    );
                    assert_eq!(after.chat_session_id, before.chat_session_id);
                    assert!(after.cancel_requested);
                    if expected {
                        assert_eq!(after.ended_at, Some(20));
                        assert_eq!(after.exit_code, Some(1));
                    } else {
                        assert_eq!(after.ended_at, before.ended_at);
                        assert_eq!(after.exit_code, before.exit_code);
                        assert_eq!(after.updated_at, before.updated_at);
                        assert_eq!(after.result_markdown, before.result_markdown);
                    }
                    if upsert {
                        assert!(after.backend_json.contains("late-handle"));
                        if expected {
                            assert_eq!(after.result_markdown.as_deref(), Some("new result"));
                        }
                    }
                }
            }
        }
        assert!(store
            .upsert_run(&run_fixture("invalid", "unknown", None))
            .is_err());
        assert!(store.get_run("invalid").unwrap().is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn terminal_statuses_are_exactly_the_absorbing_ones() {
        for from in RunStatus::ALL {
            let absorbing = RunStatus::ALL
                .into_iter()
                .all(|to| !from.can_transition_to(to));
            assert_eq!(from.is_terminal(), absorbing, "{from}");
        }
    }

    /// Rows written by every shipped version hold these strings.
    #[test]
    fn run_status_stored_strings_are_stable() {
        use RunStatus::*;
        let stored = [
            (Starting, "starting"),
            (Running, "running"),
            (Done, "done"),
            (Failed, "failed"),
            (Cancelled, "cancelled"),
            (Lost, "lost"),
        ];
        assert_eq!(stored.map(|(status, _)| status), RunStatus::ALL);
        for (status, text) in stored {
            assert_eq!(status.as_str(), text);
            assert_eq!(RunStatus::parse(text), Some(status));
        }
        assert_eq!(RunStatus::parse("not-a-real-status"), None);
        assert!(!is_terminal_status("not-a-real-status"));
    }

    /// Test scenario from the state-machine design doc: "duplicate completion
    /// callbacks". The second write must be silently rejected rather than
    /// erroring or re-stamping `ended_at`/`exit_code`.
    #[test]
    fn duplicate_terminal_writes_are_idempotent_no_ops() {
        let dir = std::env::temp_dir().join(format!("orx-store-dup-term-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&run_fixture("run_1", "running", None))
            .unwrap();

        assert!(store
            .update_status("run_1", RunStatus::Done, Some(100), Some(0))
            .unwrap());
        let first = store.get_run("run_1").unwrap().unwrap();
        assert_eq!(first.status, "done");
        assert_eq!(first.ended_at, Some(100));
        assert_eq!(first.exit_code, Some(0));

        // A second, later completion callback for the same run (e.g. a
        // retried webhook, or a stale poll landing after a fresher one) must
        // not be applied, and must not disturb the first write's timestamp
        // or exit code.
        assert!(!store
            .update_status("run_1", RunStatus::Done, Some(200), Some(1))
            .unwrap());
        let second = store.get_run("run_1").unwrap().unwrap();
        assert_eq!(second.status, "done");
        assert_eq!(
            second.ended_at,
            Some(100),
            "ended_at must not be re-stamped"
        );
        assert_eq!(
            second.exit_code,
            Some(0),
            "exit_code must not be overwritten"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test scenario: "completion arriving after cancellation" — once a run
    /// has been recorded as cancelled, a late success/failure report from the
    /// backend must not resurrect it.
    #[test]
    fn completion_after_cancellation_is_rejected() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-cancel-race-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&run_fixture("run_1", "running", None))
            .unwrap();

        assert!(store
            .update_status("run_1", RunStatus::Cancelled, Some(100), None)
            .unwrap());
        assert!(!store
            .update_status("run_1", RunStatus::Done, Some(200), Some(0))
            .unwrap());

        let run = store.get_run("run_1").unwrap().unwrap();
        assert_eq!(run.status, "cancelled");
        assert_eq!(run.ended_at, Some(100));
        assert_eq!(run.exit_code, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test scenario: "invalid backward transitions" — a backend cannot
    /// regress an already-`running` job back to `starting`.
    #[test]
    fn running_cannot_regress_to_starting() {
        let dir = std::env::temp_dir().join(format!("orx-store-backward-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&run_fixture("run_1", "running", None))
            .unwrap();

        assert!(!store
            .update_status("run_1", RunStatus::Starting, None, None)
            .unwrap());
        assert_eq!(store.get_run("run_1").unwrap().unwrap().status, "running");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test scenarios: "cancel while preparing" and "cancel while running" —
    /// both non-terminal states can move directly to `cancelled`.
    #[test]
    fn cancel_is_legal_from_either_non_terminal_state() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-cancel-both-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&run_fixture("run_starting", "starting", None))
            .unwrap();
        store
            .upsert_run(&run_fixture("run_running", "running", None))
            .unwrap();

        assert!(store
            .update_status("run_starting", RunStatus::Cancelled, Some(1), None)
            .unwrap());
        assert!(store
            .update_status("run_running", RunStatus::Cancelled, Some(1), None)
            .unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test scenario: "retry after a partially completed run" — a run that
    /// failed can be re-launched under a fresh run id (retries are new rows,
    /// never a resurrection of the old one), and the old terminal row is left
    /// untouched by anything trying to touch it afterward.
    #[test]
    fn retry_after_failure_is_a_new_row_not_a_resurrection() {
        let dir = std::env::temp_dir().join(format!("orx-store-retry-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .upsert_run(&run_fixture("run_1", "running", None))
            .unwrap();
        assert!(store
            .update_status("run_1", RunStatus::Failed, Some(1), Some(1))
            .unwrap());

        // The retry is a distinct run id; the failed row is untouched.
        store
            .upsert_run(&run_fixture("run_1_retry", "starting", None))
            .unwrap();
        assert!(store
            .update_status("run_1_retry", RunStatus::Running, None, None)
            .unwrap());

        assert_eq!(store.get_run("run_1").unwrap().unwrap().status, "failed");
        assert_eq!(
            store.get_run("run_1_retry").unwrap().unwrap().status,
            "running"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test scenario: "concurrent status updates" / "agent wake while a run
    /// is finishing" — many independent connections (as separate supervisor
    /// processes would be) race to finalize the same run with different
    /// terminal outcomes. Exactly one write must win, and the row must land
    /// on a real terminal state rather than a torn/partial one.
    #[test]
    fn concurrent_terminal_writes_from_separate_connections_settle_on_exactly_one_outcome() {
        let dir =
            std::env::temp_dir().join(format!("orx-store-concurrent-{}", uuid::Uuid::new_v4()));
        // Open once first so schema creation isn't itself racing below.
        let setup = Store::open_at(dir.clone()).unwrap();
        setup
            .upsert_run(&run_fixture("run_1", "running", None))
            .unwrap();
        drop(setup);

        let outcomes = [RunStatus::Done, RunStatus::Failed, RunStatus::Cancelled];
        let handles: Vec<_> = outcomes
            .into_iter()
            .map(|outcome| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    // Each thread is its own connection, like independent
                    // supervisor processes sharing one SQLite file (WAL +
                    // busy_timeout, set in `open_at_with_move_lock`).
                    let store = Store::open_at(dir).unwrap();
                    store
                        .update_status("run_1", outcome, Some(1), None)
                        .unwrap()
                })
            })
            .collect();
        let applied: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        assert_eq!(
            applied.iter().filter(|ok| **ok).count(),
            1,
            "exactly one racing writer should win: {applied:?}"
        );
        let final_status = Store::open_at(dir.clone())
            .unwrap()
            .get_run("run_1")
            .unwrap()
            .unwrap()
            .status;
        assert!(
            RunStatus::parse(&final_status).is_some_and(RunStatus::is_terminal),
            "run must settle on a real terminal state, got {final_status:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn news_of_a_turn_names_the_answers_it_overtook() {
        let dir = std::env::temp_dir().join(format!("orx-store-untold-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["chat_parent", "chat_child"] {
            store
                .create_chat_session(&chat_session_fixture(id))
                .unwrap();
        }
        let finish = |id: &str, created_at: i64| {
            let mut turn = chat_turn_fixture(id, &format!("client-{id}"));
            turn.session_id = "chat_child".into();
            turn.created_at = created_at;
            store.admit_chat_turn(None, &turn).unwrap();
            store.complete_chat_turn(id).unwrap();
        };
        finish("history", 1);
        store
            .subscribe_chat("chat_parent", "chat_child", Some("history"))
            .unwrap();
        finish("one", 2);
        finish("two", 3);

        let ready = store.list_ready_chat_subscriptions().unwrap();
        assert_eq!(ready[0].turn_id, "two");
        assert_eq!(
            store.untold_chat_answers(&ready[0]).unwrap(),
            ["assistant_one"],
            "the subscriber was never told of `one`, and `history` came before it subscribed"
        );
        let told = store
            .claim_chat_subscription("chat_parent", "chat_child", ToldEvent::Turn("two"))
            .unwrap()
            .unwrap();
        store
            .settle_chat_subscription(
                "chat_parent",
                "chat_child",
                &told,
                Some(ToldEvent::Turn("two")),
            )
            .unwrap();
        assert!(
            store
                .claim_chat_subscription("chat_parent", "chat_child", ToldEvent::Turn("one"))
                .unwrap()
                .is_none(),
            "a turn older than the one told cannot rewind the subscription"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn chat_subscriptions_tell_each_finished_turn_once_after_the_queue_drains() {
        let dir = std::env::temp_dir().join(format!("orx-store-sub-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["chat_parent", "chat_child"] {
            store
                .create_chat_session(&chat_session_fixture(id))
                .unwrap();
        }
        store
            .record_chat_delegate("chat_child", "chat_parent", false)
            .unwrap();
        store
            .subscribe_chat("chat_parent", "chat_child", None)
            .unwrap();
        store
            .subscribe_chat("chat_parent", "chat_child", None)
            .unwrap();
        assert_eq!(store.list_chat_subscriptions(None).unwrap().len(), 1);
        let ready = || store.list_ready_chat_subscriptions().unwrap();
        let outstanding = || {
            store
                .list_chat_subscriptions(Some("chat_parent"))
                .unwrap()
                .into_iter()
                .filter(|sub| sub.outstanding)
                .map(|sub| sub.target_session_id)
                .collect::<Vec<_>>()
        };
        let turn = |session: &str, id: &str, created_at: i64| {
            let mut turn = chat_turn_fixture(id, &format!("client-{id}"));
            turn.session_id = session.into();
            turn.created_at = created_at;
            store.admit_chat_turn(None, &turn).unwrap();
        };
        let claim = |ready: &ReadyChatSubscription| {
            store.claim_chat_subscription(
                &ready.subscriber_session_id,
                &ready.target_session_id,
                ToldEvent::Turn(&ready.turn_id),
            )
        };
        let tell = |ready: &ReadyChatSubscription| {
            let token = claim(ready).unwrap().unwrap();
            assert!(store
                .settle_chat_subscription(
                    &ready.subscriber_session_id,
                    &ready.target_session_id,
                    &token,
                    Some(ToldEvent::Turn(&ready.turn_id)),
                )
                .unwrap());
        };

        assert_eq!(
            outstanding(),
            ["chat_child"],
            "a new delegate owes an answer"
        );
        turn("chat_child", "one", 2);
        assert!(ready().is_empty(), "a running turn has nothing to tell");
        store.complete_chat_turn("one").unwrap();
        let first = ready();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].turn_id, "one");
        let token = claim(&first[0]).unwrap().unwrap();
        assert!(claim(&first[0]).unwrap().is_none());
        assert!(ready().is_empty(), "a claimed answer is in flight");
        store
            .settle_chat_subscription("chat_parent", "chat_child", &token, None)
            .unwrap();
        assert_eq!(ready().len(), 1, "an untold answer is retried");
        tell(&first[0]);
        assert!(ready().is_empty(), "each turn is told once");
        assert!(outstanding().is_empty());

        turn("chat_child", "two", 3);
        store.complete_chat_turn("two").unwrap();
        store
            .insert_queued_chat_message(&StoredQueuedChatMessage {
                id: "queued".into(),
                session_id: "chat_child".into(),
                client_turn_id: "client-queued".into(),
                request_hash: "hash".into(),
                payload_json: "{}".into(),
                created_at: 4,
            })
            .unwrap();
        assert!(ready().is_empty(), "a queued message means more work first");
        store.delete_queued_chat_message("queued").unwrap();
        assert_eq!(ready()[0].turn_id, "two");
        tell(&ready()[0]);

        // The child delegates in turn: its turn ending while its own delegate
        // works is not an answer for the parent. An unrelated subscription of
        // the child does not hold its answer back.
        for id in ["chat_grandchild", "chat_elsewhere"] {
            store
                .create_chat_session(&chat_session_fixture(id))
                .unwrap();
            store.subscribe_chat("chat_child", id, None).unwrap();
        }
        store
            .record_chat_delegate("chat_grandchild", "chat_child", false)
            .unwrap();
        turn("chat_child", "three", 5);
        store.complete_chat_turn("three").unwrap();
        assert!(ready().is_empty(), "its delegate has not run yet");
        turn("chat_grandchild", "four", 6);
        assert!(ready().is_empty(), "its delegate is still working");
        store.complete_chat_turn("four").unwrap();
        let waiting = ready();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].subscriber_session_id, "chat_child");
        tell(&waiting[0]);
        assert_eq!(
            ready()[0].turn_id,
            "three",
            "answered, the child's turn is its answer"
        );

        // The user stopped the child telling its parent: neither subscribing
        // again nor unsubscribing first undoes that.
        assert!(store
            .pause_chat_subscription("chat_parent", "chat_child")
            .unwrap());
        assert!(ready()
            .iter()
            .all(|ready| ready.subscriber_session_id != "chat_parent"));
        store.unsubscribe_chat("chat_parent", "chat_child").unwrap();
        store
            .subscribe_chat("chat_parent", "chat_child", None)
            .unwrap();
        assert!(ready()
            .iter()
            .all(|ready| ready.subscriber_session_id != "chat_parent"));

        store.delete_chat_session("chat_child").unwrap();
        assert!(store
            .list_chat_subscriptions(None)
            .unwrap()
            .iter()
            .all(|sub| sub.subscriber_session_id != "chat_child"
                && sub.target_session_id != "chat_child"));
        assert!(store.chat_delegates_of("chat_parent").unwrap().is_empty());
        assert_eq!(
            store.delegation("chat_grandchild").unwrap().parent,
            None,
            "a delegate outliving its delegator is the user's"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delegation_depth_follows_delegate_links_not_side_chats() {
        let dir = std::env::temp_dir().join(format!("orx-store-depth-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let chat = |id: &str, side_parent: Option<&str>| {
            let mut session = chat_session_fixture(id);
            session.side_parent_session_id = side_parent.map(str::to_string);
            store.create_chat_session(&session).unwrap();
        };
        chat("chat_root", None);
        chat("chat_side", Some("chat_root"));
        chat("chat_one", Some("chat_side"));
        chat("chat_two", None);
        store
            .record_chat_delegate("chat_one", "chat_side", true)
            .unwrap();
        store
            .record_chat_delegate("chat_two", "chat_one", false)
            .unwrap();
        store
            .subscribe_chat("chat_root", "chat_side", None)
            .unwrap();
        let at = |id: &str| store.delegation(id).unwrap();
        let user = Delegation {
            depth: 0,
            parent: None,
            may_delegate: true,
        };
        assert_eq!(at("chat_root"), user);
        assert_eq!(
            at("chat_side"),
            user,
            "a side chat the user opened is not a delegate, subscribed or not"
        );
        assert_eq!(
            at("chat_one"),
            Delegation {
                depth: 1,
                parent: Some("chat_side".into()),
                may_delegate: true
            }
        );
        assert_eq!(
            at("chat_two"),
            Delegation {
                depth: 2,
                parent: Some("chat_one".into()),
                may_delegate: false
            },
            "a delegate in its own worktree is still one level deeper"
        );
        assert_eq!(store.chat_worktree_owner("chat_one").unwrap(), "chat_root");
        assert_eq!(store.chat_worktree_owner("chat_two").unwrap(), "chat_two");
        let _ = std::fs::remove_dir_all(dir);
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
        let service_lock =
            acquire_data_dir_lock(first.data_dir_move_lock_path.clone(), false).unwrap();
        assert!(first.acquire_data_dir_move_lock().is_err());
        assert!(second.claim_chat_turn("chat_A", "token_B").unwrap());

        drop(service_lock);
        drop(second);
        drop(first);
        let _ = std::fs::remove_dir_all(dir);
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
        let _ = std::fs::remove_dir_all(dir);
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
        assert_eq!(
            run.status, "failed",
            "terminal outcome survives later upserts"
        );
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
            archived: false,
        }
    }

    #[test]
    fn session_experiments_include_creation_runs_and_wakeups_across_projects() {
        let dir =
            std::env::temp_dir().join(format!("orx-session-experiments-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["proj_1", "proj_2"] {
            store
                .create_local_project(&LocalProject {
                    id: id.into(),
                    name: format!("name of {id}"),
                    slug: id.into(),
                    github_owner: String::new(),
                    github_repo: String::new(),
                    github_sync_enabled: false,
                    baseline_branch: "main".into(),
                    repo_path: dir.join(id).to_string_lossy().into_owned(),
                    run_command: None,
                    paper_id: None,
                    created_at: 1,
                    updated_at: 1,
                })
                .unwrap();
        }
        for (id, owner, project) in [
            ("created", Some("chat_a"), "proj_1"),
            ("created_no_runs", Some("chat_a"), "proj_1"),
            ("run", Some("chat_b"), "proj_2"),
            ("both", Some("chat_a"), "proj_1"),
            ("watched", Some("chat_b"), "proj_1"),
            ("concurrent", Some("chat_a"), "proj_1"),
            ("archived", Some("chat_a"), "proj_1"),
            ("unrelated", Some("chat_b"), "proj_1"),
            ("missing_project", Some("chat_a"), "missing"),
            ("deleted", Some("chat_a"), "proj_1"),
        ] {
            let mut experiment = experiment_fixture(id, owner);
            experiment.project_id = project.into();
            experiment.archived = id == "archived";
            store.create_local_experiment(&experiment).unwrap();
        }
        for (id, experiment, session, status, created_at, ended_at) in [
            ("run_1", "run", "chat_a", "failed", 5, Some(6)),
            ("run_2", "run", "chat_a", "done", 7, Some(9)),
            ("run_3", "both", "chat_a", "done", 1, Some(2)),
            ("run_4", "created", "chat_b", "done", 10, Some(20)),
            ("run_5", "created", "chat_b", "running", 30, None),
            ("run_6", "watched", "chat_b", "starting", 3, None),
            ("run_7", "unrelated", "chat_b", "done", 4, Some(5)),
            ("run_8", "missing_experiment", "chat_a", "done", 1, Some(2)),
            ("run_9", "deleted", "chat_a", "done", 1, Some(2)),
            ("run_10", "archived", "chat_a", "done", 50, Some(100)),
            ("run_11", "concurrent", "chat_a", "running", 5, None),
            ("run_12", "concurrent", "chat_a", "failed", 9, Some(10)),
        ] {
            let mut run = run_fixture(id, status, Some(session));
            run.experiment_id = experiment.into();
            run.created_at = created_at;
            run.ended_at = ended_at;
            store.upsert_run(&run).unwrap();
        }
        store.register_run_wakeup("run_6", "chat_a").unwrap();
        store
            .conn
            .execute("DELETE FROM local_experiments WHERE id = 'deleted'", [])
            .unwrap();
        let rows = store.list_experiments_by_chat_session("chat_a").unwrap();
        let summary: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.experiment.id.as_str(),
                    row.current_run.as_ref().map(|run| run.status.as_str()),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("created", Some("running")),
                ("concurrent", Some("running")),
                ("watched", Some("starting")),
                ("run", Some("done")),
                ("both", Some("done")),
                ("created_no_runs", None),
                ("archived", Some("done")),
            ]
        );
        assert_eq!(rows[3].experiment.project_id, "proj_2");
        assert_eq!(rows[3].project_name, "name of proj_2");
        // An older run still in flight outranks the newer run that already failed.
        assert_eq!(rows[1].current_run.as_ref().unwrap().created_at, 5);
        let response = serde_json::json!({ "experiments": rows });
        assert_eq!(
            response["experiments"][0]["currentRun"]["status"],
            "running"
        );
        assert_eq!(response["experiments"][0]["currentRun"]["createdAt"], 30);
        assert_eq!(
            response["experiments"][0]["currentRun"]["endedAt"],
            serde_json::Value::Null
        );
        assert_eq!(
            response["experiments"][0]["experiment"]["chatSessionId"],
            "chat_a"
        );
        assert_eq!(
            response["experiments"][5]["currentRun"],
            serde_json::Value::Null
        );
        assert!(store
            .list_experiments_by_chat_session("unrelated")
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
                    completed_at: None,
                    parent_id: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                    native_harness: None,
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
        assert_eq!(summary.last_activity_at, 30);
        assert_eq!(store.list_chat_session_project_ids().unwrap().len(), 2);

        for (table, timestamp) in [
            ("local_projects", 40),
            ("chat_sessions", 50),
            ("local_experiments", 60),
            ("runs", 70),
        ] {
            store
                .conn
                .execute(&format!("UPDATE {table} SET updated_at = ?1"), [timestamp])
                .unwrap();
            assert_eq!(
                store.list_project_activity_summaries().unwrap()[0].last_activity_at,
                timestamp
            );
        }

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
        updated.parent_experiment_id = Some("stale-parent".into());
        store.update_local_experiment(&updated).unwrap();

        let stored = store.get_local_experiment("exp_owned").unwrap().unwrap();
        assert_eq!(stored.title.as_deref(), Some("renamed"), "title updates");
        assert_eq!(
            stored.chat_session_id,
            Some("chat_x".to_string()),
            "the creating session is never overwritten by a later update"
        );
        assert_eq!(stored.parent_experiment_id, None, "only a move re-parents");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn host_roots_never_persist_alias_authorization() {
        let dir = std::env::temp_dir().join(format!("orx-alias-store-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        use crate::engine::transport::{AuthState, HostClass, HostState, Status};
        let auth = AuthState {
            class: Some(HostClass::Key),
            route: Some("approved route".into()),
            blocked: Some((123, "auth rejected".into())),
        };
        store.save_engine_host_auth("lab", &auth).unwrap();
        for root in ["ssh", "slurm"] {
            store
                .save_engine_host(
                    "lab",
                    root,
                    &HostState {
                        status: Status::NeedsAuth,
                        ..HostState::default()
                    },
                )
                .unwrap();
        }
        assert!(store
            .engine_hosts()
            .unwrap()
            .iter()
            .all(|(_, _, state)| state.status == Status::NeedsAuth));
        let mut rows = store
            .conn
            .prepare("SELECT state_json FROM engine_hosts")
            .unwrap();
        for json in rows.query_map([], |row| row.get::<_, String>(0)).unwrap() {
            let json: serde_json::Value = serde_json::from_str(&json.unwrap()).unwrap();
            assert!(json.get("class").is_none());
            assert!(json.get("route").is_none());
            assert_eq!(json["status"], "Idle");
            assert!(json["cause"].is_null());
        }
        drop(rows);
        store
            .save_engine_host_auth(
                "lab",
                &AuthState {
                    blocked: None,
                    ..auth
                },
            )
            .unwrap();
        assert!(store
            .engine_hosts()
            .unwrap()
            .iter()
            .all(|(_, _, state)| state.status == Status::Idle && state.cause.is_none()));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn log_path_keeps_the_explicit_directory_and_sanitizes_the_run_id() {
        assert_eq!(
            log_path(Path::new("/isolated/data"), "../run_A-1/../../x"),
            Path::new("/isolated/data/run-logs/run_A-1x.log")
        );
    }
}
