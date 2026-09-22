//! Chats the user had in a coding agent's own CLI, outside orx.
//!
//! orx can already resume one: `native_store` resolves a session id in the
//! user's real agent home and the turn then runs against it. What is missing is
//! finding them, which each agent stores differently — Claude Code as one JSONL
//! transcript per session, Codex as a SQLite index beside its rollout files.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{anyhow, Result};
use crate::local::native_store::{self, NativeStore};

/// How much of a transcript is read to name it: the head carries the working
/// directory and usually the title, the tail a title the user changed later.
const PROBE_BYTES: u64 = 16 * 1024;

/// Chats offered by the picker, newest first.
pub const LISTING_LIMIT: usize = 200;

/// The newest messages kept when adopting a chat. The copy is for reading —
/// the agent itself resumes from its own session, which is not truncated.
const BACKFILL_MESSAGES: usize = 500;

/// Enough of a first prompt to recognize an unnamed chat by.
const PROMPT_TITLE_CHARS: usize = 80;

/// A chat in an agent's own store that orx does not have a session for.
pub struct NativeChat {
    pub harness: &'static str,
    pub native_id: String,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub updated_at: i64,
}

/// One message of an adopted chat, as far as orx can read it back.
pub struct NativeMessage {
    pub role: &'static str,
    pub text: String,
    pub created_at: i64,
}

/// Whether this harness keeps chats orx can read back at all.
pub fn is_importable(harness: &str) -> bool {
    matches!(harness, "claude-code" | "codex")
}

pub fn list(owned: &HashSet<String>, limit: usize) -> Vec<NativeChat> {
    let mut found: Vec<(i64, ChatSource)> = claude_sources()
        .into_iter()
        .chain(codex_sources())
        .filter(|(_, source)| !owned.contains(source.native_id()))
        .collect();
    // Sorted and cut before anything is read: probing every transcript to build
    // a list that keeps 200 of them reads the user's whole Claude history.
    found.sort_by_key(|(updated_at, _)| std::cmp::Reverse(*updated_at));
    // Over-fetch: describing drops orx's own throwaway children, and cutting to
    // the limit first would let a run of them shrink the list.
    found.truncate(limit * 2);
    let mut chats: Vec<NativeChat> = found
        .into_iter()
        .filter_map(|(updated_at, source)| source.describe(updated_at))
        .collect();
    chats.truncate(limit);
    chats
}

/// The transcript of one adopted chat: the text of each side's messages. Tool
/// calls and their output stay in the agent's own session, which is what the
/// resumed turn actually continues from.
pub fn transcript(harness: &str, native_id: &str) -> Result<Vec<NativeMessage>> {
    let path = match harness {
        "claude-code" => native_store::claude_session(native_id)?.map(|s| s.path),
        "codex" => native_store::codex_session(native_id)?.map(|s| s.path),
        _ => None,
    };
    let path = path.ok_or_else(|| anyhow!("that chat is no longer on disk"))?;
    // Streamed and bounded: a long-running chat's transcript reaches tens of
    // megabytes, and the newest turns are the ones worth showing.
    let file = std::io::BufReader::new(std::fs::File::open(&path)?);
    let mut spoken = std::collections::VecDeque::new();
    for line in std::io::BufRead::lines(file) {
        let Ok(line) = line else { continue };
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let parsed = match harness {
            "claude-code" => claude_message(&record),
            _ => codex_message(&record),
        };
        if let Some(message) = parsed {
            if spoken.len() == BACKFILL_MESSAGES {
                spoken.pop_front();
            }
            spoken.push_back(message);
        }
    }
    Ok(spoken.into())
}

/// A chat found on disk, before it is worth reading anything else about it.
enum ChatSource {
    Claude { native_id: String, path: PathBuf },
    Codex(NativeChat),
}

impl ChatSource {
    fn native_id(&self) -> &str {
        match self {
            ChatSource::Claude { native_id, .. } => native_id,
            ChatSource::Codex(chat) => &chat.native_id,
        }
    }

    fn describe(self, updated_at: i64) -> Option<NativeChat> {
        let (native_id, path) = match self {
            ChatSource::Codex(chat) => return Some(chat),
            ChatSource::Claude { native_id, path } => (native_id, path),
        };
        let head = read_window(&path, false)?;
        // Optional: a first record longer than the probe window leaves the cwd
        // unread, which is a row without a folder label, not a chat to hide.
        let cwd = pick_lines(&head, false, |record| {
            record
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string)
        });
        if cwd.as_deref().is_some_and(is_temporary) {
            return None;
        }
        Some(NativeChat {
            harness: "claude-code",
            native_id,
            // Claude titles a chat early and rewrites the record on a rename,
            // so a later title wins; an unnamed chat opens with what it was
            // asked to do.
            title: probe_tail(&path, claude_title)
                .or_else(|| pick_lines(&head, true, claude_title))
                .or_else(|| pick_lines(&head, false, first_prompt)),
            cwd,
            updated_at,
        })
    }
}

/// orx spawns throwaway children (its auto-titler) in a temp directory; their
/// transcripts are not chats the user had. Compared canonically because macOS
/// records the cwd as `/private/var/...` while `temp_dir()` reports `/var/...`.
fn is_temporary(cwd: &str) -> bool {
    let temp = std::env::temp_dir();
    let canonical = std::fs::canonicalize(&temp).unwrap_or_else(|_| temp.clone());
    let path = Path::new(cwd);
    path.starts_with(&temp) || path.starts_with(&canonical)
}

fn claude_title(record: &Value) -> Option<String> {
    match record.get("type").and_then(Value::as_str)? {
        "custom-title" => record.get("customTitle"),
        "ai-title" => record.get("aiTitle"),
        _ => None,
    }
    .and_then(Value::as_str)
    .map(str::to_string)
}

/// A chat with no title of its own is named by what it was asked to do.
fn first_prompt(record: &Value) -> Option<String> {
    let message = claude_message(record).filter(|message| message.role == "user")?;
    let line = message
        .text
        .lines()
        .map(str::trim)
        // The CLIs prepend context the user did not type.
        .find(|line| !line.is_empty() && !line.starts_with('<'))?;
    Some(line.chars().take(PROMPT_TITLE_CHARS).collect())
}

/// Wrappers the CLIs inject around what the user actually typed. They are
/// context for the agent, not part of the conversation being read back.
const INJECTED_BLOCKS: [&str; 4] = [
    "system-reminder",
    "local-command-caveat",
    "command-message",
    "task-notification",
];

fn without_injected_blocks(text: String) -> String {
    let mut text = text;
    for tag in INJECTED_BLOCKS {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        while let Some(start) = text.find(&open) {
            // An unterminated block runs to the end — dropping the tail beats
            // importing a wrapper as something the user said.
            let end = match text[start..].find(&close) {
                Some(end) => start + end + close.len(),
                None => text.len(),
            };
            text.replace_range(start..end, "");
        }
    }
    text.trim().to_string()
}

/// Claude writes one record per event; `user`/`assistant` carry the message.
/// Sidechains are a sub-agent's own conversation, not this one's.
fn claude_message(record: &Value) -> Option<NativeMessage> {
    if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let role = match record.get("type").and_then(Value::as_str)? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let text = without_injected_blocks(content_text(record.get("message")?.get("content")?));
    (!text.is_empty()).then(|| NativeMessage {
        role,
        text,
        created_at: iso_ms(record.get("timestamp")),
    })
}

/// Codex rollouts interleave protocol events; `response_item` messages are the
/// conversation. `developer` items are orx's own preamble, not the user's.
fn codex_message(record: &Value) -> Option<NativeMessage> {
    if record.get("type").and_then(Value::as_str)? != "response_item" {
        return None;
    }
    let payload = record.get("payload")?;
    if payload.get("type").and_then(Value::as_str)? != "message" {
        return None;
    }
    let role = match payload.get("role").and_then(Value::as_str)? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let text = without_injected_blocks(content_text(payload.get("content")?));
    (!text.is_empty()).then(|| NativeMessage {
        role,
        text,
        created_at: iso_ms(record.get("timestamp")),
    })
}

/// Both agents stamp records as `YYYY-MM-DDTHH:MM:SS[.mmm]Z`, and the copy is
/// worth dating honestly — a year-old chat should not read as if it happened
/// just now. Anything unparseable falls back to the time it was read.
fn iso_ms(timestamp: Option<&Value>) -> i64 {
    timestamp
        .and_then(Value::as_str)
        .and_then(parse_iso_ms)
        .unwrap_or_else(crate::store::now_ms)
}

fn parse_iso_ms(stamp: &str) -> Option<i64> {
    let (date, rest) = stamp.split_once('T')?;
    let mut date = date.split('-');
    let (year, month, day) = (
        date.next()?.parse::<i64>().ok()?,
        date.next()?.parse::<i64>().ok()?,
        date.next()?.parse::<i64>().ok()?,
    );
    // `Z` only: a numeric offset would date the message wrong rather than not
    // at all, which is worse than falling back to the time it was read.
    let time = rest.strip_suffix('Z').unwrap_or(rest);
    let (time, millis) = match time.split_once('.') {
        Some((time, fraction)) => {
            let digits: String = fraction.chars().take(3).collect();
            if !fraction.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            (time, format!("{digits:0<3}").parse::<i64>().ok()?)
        }
        None => (time, 0),
    };
    if !time.chars().all(|c| c.is_ascii_digit() || c == ':') {
        return None;
    }
    let mut time = time.split(':');
    let (hour, minute, second) = (
        time.next()?.parse::<i64>().ok()?,
        time.next()?.parse::<i64>().ok()?,
        time.next()?.parse::<i64>().ok()?,
    );
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Days since 1970-01-01, by Howard Hinnant's civil-date algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Both agents write content as a string or as typed parts.
fn content_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n\n")
        })
        .unwrap_or_default()
}

fn claude_sources() -> Vec<(i64, ChatSource)> {
    let projects = native_store::claude_home(NativeStore::Legacy).join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    let mut sources = Vec::new();
    for project in entries.flatten() {
        let Ok(transcripts) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in transcripts.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(native_id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else { continue };
            sources.push((
                modified_ms(&meta),
                ChatSource::Claude {
                    native_id: native_id.to_string(),
                    path,
                },
            ));
        }
    }
    sources
}

fn codex_sources() -> Vec<(i64, ChatSource)> {
    let db = native_store::codex_home(NativeStore::Legacy).join("state_5.sqlite");
    codex_threads(&db)
        .unwrap_or_default()
        .into_iter()
        // Both agents run orx's throwaway children in a temp directory.
        .filter(|chat| !chat.cwd.as_deref().is_some_and(is_temporary))
        .map(|chat| (chat.updated_at, ChatSource::Codex(chat)))
        .collect()
}

/// Codex's thread index, read without disturbing it. `mode=ro` sees the WAL —
/// where the newest threads live until codex checkpoints — so `immutable=1`,
/// which ignores the WAL entirely, is only the fallback.
fn codex_threads(db: &Path) -> Result<Vec<NativeChat>> {
    if !db.exists() {
        return Ok(Vec::new());
    }
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let conn =
        rusqlite::Connection::open_with_flags(format!("file:{}?mode=ro", db.display()), flags)
            .or_else(|_| {
                rusqlite::Connection::open_with_flags(
                    format!("file:{}?immutable=1", db.display()),
                    flags,
                )
            })?;
    let mut stmt = conn.prepare(&format!(
        "SELECT id, title, cwd, updated_at FROM threads
         WHERE archived = 0 AND COALESCE(thread_source, '') != 'subagent'
         ORDER BY updated_at DESC LIMIT {LISTING_LIMIT}"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok(NativeChat {
            harness: "codex",
            native_id: row.get(0)?,
            title: row.get(1)?,
            cwd: row.get(2)?,
            // Codex stores whole seconds here; orx timestamps are milliseconds.
            updated_at: row.get::<_, i64>(3)? * 1000,
        })
    })?;
    Ok(rows.flatten().collect())
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// The newest match in the tail — a title is rewritten as the chat goes on.
fn probe_tail(path: &Path, pick: impl Fn(&Value) -> Option<String>) -> Option<String> {
    read_window(path, true).and_then(|text| pick_lines(&text, true, pick))
}

fn pick_lines(
    text: &str,
    newest_first: bool,
    pick: impl Fn(&Value) -> Option<String>,
) -> Option<String> {
    let parse = |line: &str| {
        serde_json::from_str::<Value>(line)
            .ok()
            .as_ref()
            .and_then(&pick)
    };
    if newest_first {
        text.lines().rev().find_map(parse)
    } else {
        text.lines().find_map(parse)
    }
}

/// A bounded read of one end of a transcript: these files reach hundreds of
/// megabytes, and the picker only needs to name them.
fn read_window(path: &Path, tail: bool) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if tail && len > PROBE_BYTES {
        file.seek(SeekFrom::End(-(PROBE_BYTES as i64))).ok()?;
    }
    let mut buffer = Vec::with_capacity(PROBE_BYTES.min(len) as usize);
    file.take(PROBE_BYTES).read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(message: Option<NativeMessage>) -> Option<(&'static str, String)> {
        message.map(|message| (message.role, message.text))
    }

    #[test]
    fn the_clis_own_injected_blocks_are_not_part_of_the_conversation() {
        let record = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content":
                "<system-reminder>\nworktree\n</system-reminder>\n\nrun the sweep" },
        });
        assert_eq!(
            text_of(claude_message(&record)),
            Some(("user", "run the sweep".into()))
        );
        // A message that was nothing but injected context is not a message.
        let only_context = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": "<system-reminder>ctx</system-reminder>" },
        });
        assert!(claude_message(&only_context).is_none());
        // An unterminated block takes the rest with it rather than leaking.
        assert_eq!(
            without_injected_blocks("keep <system-reminder>and the rest".into()),
            "keep"
        );
    }

    #[test]
    fn a_claude_record_is_read_as_its_speaker_and_text() {
        let user = serde_json::json!({
            "type": "user",
            "timestamp": "2026-09-04T14:28:28.000Z",
            "message": { "role": "user", "content": "run the sweep" },
        });
        let message = claude_message(&user).expect("a user message");
        assert_eq!(
            (message.role, message.text.as_str()),
            ("user", "run the sweep")
        );
        // The copy keeps when it was said, not when it was read.
        assert_eq!(message.created_at, 1788532108000);

        let assistant = serde_json::json!({
            "type": "assistant",
            "message": { "role": "assistant", "content": [
                { "type": "text", "text": "on it" },
                { "type": "tool_use", "name": "Bash" },
            ]},
        });
        assert_eq!(
            text_of(claude_message(&assistant)),
            Some(("assistant", "on it".into()))
        );

        // A sub-agent's own conversation is not this chat's.
        let sidechain = serde_json::json!({
            "type": "user", "isSidechain": true,
            "message": { "role": "user", "content": "inner" },
        });
        assert!(claude_message(&sidechain).is_none());
        assert!(claude_message(&serde_json::json!({ "type": "queue-operation" })).is_none());
    }

    #[test]
    fn record_timestamps_are_read_as_epoch_millis() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2026-09-04T14:28:28.000Z"),
            Some(1788532108000)
        );
        // A leap day, a fraction shorter than three digits, and no fraction.
        assert_eq!(parse_iso_ms("2024-02-29T00:00:00Z"), Some(1709164800000));
        assert_eq!(parse_iso_ms("2026-09-04T14:28:28.5Z"), Some(1788532108500));
        assert_eq!(parse_iso_ms("not a timestamp"), None);
        // An offset is not a fraction, and a fraction is not bytes.
        assert_eq!(parse_iso_ms("2026-09-04T14:28:28.123+02:00"), None);
        assert_eq!(parse_iso_ms("2026-01-01T00:00:00.𝄞Z"), None);
    }

    #[test]
    fn a_codex_rollout_record_is_read_as_its_speaker_and_text() {
        let user = serde_json::json!({
            "type": "response_item",
            "payload": { "type": "message", "role": "user", "content": [
                { "type": "input_text", "text": "ship it" },
            ]},
        });
        assert_eq!(
            text_of(codex_message(&user)),
            Some(("user", "ship it".into()))
        );

        // orx's own preamble rides as a developer message.
        let developer = serde_json::json!({
            "type": "response_item",
            "payload": { "type": "message", "role": "developer", "content": [
                { "type": "input_text", "text": "<app-context>" },
            ]},
        });
        assert!(codex_message(&developer).is_none());
        assert!(
            codex_message(&serde_json::json!({ "type": "event_msg", "payload": {} })).is_none()
        );
    }

    #[test]
    fn a_chat_is_named_by_its_newest_title_else_by_its_first_prompt() {
        let titled =
            serde_json::json!({ "type": "ai-title", "aiTitle": "Named by the agent" }).to_string();
        assert_eq!(
            pick_lines(&titled, true, claude_title),
            Some("Named by the agent".into())
        );
        let unnamed = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": "<system-reminder>ctx</system-reminder>\nthe ask" },
        })
        .to_string();
        assert_eq!(
            pick_lines(&unnamed, false, first_prompt),
            Some("the ask".into())
        );
    }

    #[test]
    fn a_throwaway_child_in_a_temp_directory_is_not_a_chat() {
        let temp = std::env::temp_dir();
        assert!(is_temporary(&temp.join("orx-probe").display().to_string()));
        // macOS records the cwd canonically, which `temp_dir()` does not.
        let canonical = std::fs::canonicalize(&temp).unwrap_or_else(|_| temp.clone());
        assert!(is_temporary(
            &canonical.join("orx-probe").display().to_string()
        ));
        assert!(!is_temporary("/Users/someone/repo"));
    }

    #[test]
    fn codex_threads_skip_archived_and_subagent_rows_and_read_seconds() {
        let dir = std::env::temp_dir().join(format!("orx-codex-threads-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let db = dir.join("state_5.sqlite");
        {
            let conn = rusqlite::Connection::open(&db).expect("open");
            conn.execute_batch(
                "CREATE TABLE threads (id TEXT, title TEXT, cwd TEXT, updated_at INTEGER,
                     archived INTEGER, thread_source TEXT);
                 INSERT INTO threads VALUES ('keep', 'Kept', '/repo', 1700000000, 0, '');
                 INSERT INTO threads VALUES ('old', 'Archived', '/repo', 1700000001, 1, '');
                 INSERT INTO threads VALUES ('sub', 'Subagent', '/repo', 1700000002, 0, 'subagent');",
            )
            .expect("seed");
        }
        let threads = codex_threads(&db).expect("read");
        assert_eq!(
            threads
                .iter()
                .map(|thread| thread.native_id.as_str())
                .collect::<Vec<_>>(),
            vec!["keep"]
        );
        assert_eq!(threads[0].updated_at, 1700000000 * 1000);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
