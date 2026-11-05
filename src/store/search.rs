use std::collections::{HashMap, HashSet};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Store;
use crate::error::{anyhow, Result};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum SearchScope {
    Chat,
    Project,
    All,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchRequest {
    q: String,
    scope: SearchScope,
    session_id: Option<String>,
    project_id: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

enum SearchFilter {
    Chat(String),
    Project(String),
    All,
}

pub(crate) struct SearchQuery {
    terms: Vec<String>,
    filter: SearchFilter,
    limit: usize,
    after: Option<SearchCursor>,
    fingerprint: String,
}

#[derive(Clone, Deserialize, Serialize)]
struct SearchCursor {
    rowid: i64,
    fingerprint: String,
}

impl SearchRequest {
    pub(crate) fn prepare(self) -> Result<SearchQuery> {
        if self.q.chars().count() > 256 {
            return Err(anyhow!("Search is limited to 256 characters."));
        }
        // FTS5 folds case itself; lowercasing first breaks letters like `İ` that Rust and FTS5 fold differently.
        let terms: Vec<_> = self
            .q
            .split_whitespace()
            .take(16)
            .map(str::to_owned)
            .collect();
        let filter = match self.scope {
            SearchScope::Chat => SearchFilter::Chat(
                self.session_id
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow!("Choose a chat to search."))?,
            ),
            SearchScope::Project => SearchFilter::Project(
                self.project_id
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| anyhow!("Choose a project to search."))?,
            ),
            SearchScope::All => SearchFilter::All,
        };
        let scope_id = match &filter {
            SearchFilter::Chat(id) | SearchFilter::Project(id) => Some(id.as_str()),
            SearchFilter::All => None,
        };
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&terms, self.scope, scope_id))?)
        );
        let after = self
            .cursor
            .map(|encoded| {
                let bytes = if encoded.len() <= 1024 {
                    URL_SAFE_NO_PAD.decode(encoded).ok()
                } else {
                    None
                };
                let cursor = bytes
                    .and_then(|bytes| serde_json::from_slice::<SearchCursor>(&bytes).ok())
                    .filter(|cursor| cursor.fingerprint == fingerprint);
                cursor.ok_or_else(|| anyhow!("Search changed. Start from the first result."))
            })
            .transpose()?;
        let limit = self.limit.unwrap_or(50);
        if !(1..=100).contains(&limit) {
            return Err(anyhow!("Request between 1 and 100 search results."));
        }
        Ok(SearchQuery {
            terms,
            filter,
            limit,
            after,
            fingerprint,
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SearchPage {
    hits: Vec<SearchHit>,
    cursor: Option<String>,
    indexing: bool,
    requires_longer_query: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchHit {
    session_id: String,
    project_id: String,
    message_id: String,
    part_id: Option<String>,
    spawn_part_id: Option<String>,
    role: String,
    created_at: i64,
    chat_title: Option<String>,
    archived: bool,
    on_active_branch: bool,
    snippet: String,
}

pub(super) fn initialize(conn: &Connection) -> Result<()> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    tx.execute_batch(include_str!("search.sql"))?;
    tx.commit()?;
    Ok(())
}

impl Store {
    /// Reindex one bounded batch; the committed cursor survives a dashboard restart.
    pub(crate) fn backfill_chat_search(&self) -> Result<bool> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let through: Option<i64> = tx
            .query_row(
                "SELECT through_rowid FROM chat_search_backfill WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(through) = through else {
            return Ok(false);
        };
        let end: Option<i64> = tx.query_row(
            "WITH pending AS (
                SELECT rowid, octet_length(parts_json) AS bytes FROM chat_messages
                WHERE rowid > ?1 ORDER BY rowid LIMIT 100
             ), bounded AS (
                SELECT rowid, sum(bytes) OVER (ORDER BY rowid) AS total FROM pending
             ) SELECT coalesce(max(CASE WHEN total <= 131072 THEN rowid END), min(rowid)) FROM bounded",
            [through], |row| row.get(0),
        )?;
        if let Some(end) = end {
            tx.execute(
                "DELETE FROM chat_search WHERE rowid > ?1 AND rowid <= ?2",
                params![through, end],
            )?;
            tx.execute(
                "INSERT INTO chat_search(rowid, text) SELECT message_rowid, text FROM chat_search_text WHERE message_rowid > ?1 AND message_rowid <= ?2",
                params![through, end],
            )?;
            tx.execute(
                "UPDATE chat_search_backfill SET through_rowid = ?1 WHERE id = 1",
                [end],
            )?;
        } else {
            tx.execute("DELETE FROM chat_search_backfill", [])?;
            let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            tx.pragma_update(None, "user_version", version.max(2))?;
        }
        tx.commit()?;
        Ok(end.is_some())
    }

    pub(crate) fn search_chat_messages(&self, query: &SearchQuery) -> Result<SearchPage> {
        let snapshot = self.conn.unchecked_transaction()?;
        let mut page = SearchPage {
            hits: Vec::new(),
            cursor: None,
            indexing: snapshot.query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_search_backfill)",
                [],
                |row| row.get(0),
            )?,
            requires_longer_query: false,
        };
        if query.terms.is_empty() {
            return Ok(page);
        }
        let lowered: Vec<_> = query.terms.iter().map(|term| term.to_lowercase()).collect();
        let indexed_terms: Vec<_> = query
            .terms
            .iter()
            .filter(|term| term.chars().count() >= 3)
            .collect();
        let scan = indexed_terms.is_empty();
        if scan && !matches!(query.filter, SearchFilter::Chat(_)) {
            page.requires_longer_query = true;
            return Ok(page);
        }
        let expression = indexed_terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        let (scope_column, scope_id) = match &query.filter {
            SearchFilter::Chat(id) => ("m.session_id", Some(id.as_str())),
            SearchFilter::Project(id) => ("s.project_id", Some(id.as_str())),
            SearchFilter::All => ("NULL", None),
        };
        let (lower, upper) = if scan || matches!(query.filter, SearchFilter::All) {
            (i64::MIN, i64::MAX)
        } else {
            let sql = format!(
                "SELECT min(m.rowid), max(m.rowid) FROM chat_messages m
                 JOIN chat_sessions s ON s.id = m.session_id WHERE {scope_column} = ?1"
            );
            let bounds: (Option<i64>, Option<i64>) =
                snapshot.query_row(&sql, [scope_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            let (Some(lower), Some(upper)) = bounds else {
                return Ok(page);
            };
            (lower, upper)
        };
        let (source, predicate, order) = if scan {
            ("chat_messages m", "?1 = ''", "m.rowid")
        } else {
            (
                "chat_search JOIN chat_messages m ON m.rowid = chat_search.rowid",
                "chat_search MATCH ?1",
                "chat_search.rowid",
            )
        };
        let sql = format!(
            "SELECT m.rowid, m.id, m.session_id, s.project_id, m.role, m.created_at, s.title, s.archived
             FROM {source} JOIN chat_sessions s ON s.id = m.session_id
             WHERE {predicate} AND {scope_column} IS ?2
               AND {order} >= ?3 AND {order} <= ?4
             ORDER BY {order} DESC LIMIT ?5"
        );
        let mut statement = snapshot.prepare(&sql)?;
        let mut text_statement = snapshot.prepare(
            "SELECT text, part_id, spawn_part_id FROM chat_search_parts WHERE message_rowid = ?1",
        )?;
        let mut after = query.after.clone();
        let mut last_hit = None;
        let mut branches: HashMap<String, HashSet<String>> = HashMap::new();
        'pages: loop {
            if after.as_ref().is_some_and(|cursor| cursor.rowid <= lower) {
                break;
            }
            let upper = after
                .as_ref()
                .map_or(upper, |cursor| upper.min(cursor.rowid - 1));
            let candidates = statement
                .query_map(
                    params![expression, scope_id, lower, upper, query.limit + 1,],
                    |row| {
                        Ok((
                            SearchCursor {
                                rowid: row.get(0)?,
                                fingerprint: query.fingerprint.clone(),
                            },
                            SearchHit {
                                message_id: row.get(1)?,
                                part_id: None,
                                spawn_part_id: None,
                                session_id: row.get(2)?,
                                project_id: row.get(3)?,
                                role: row.get(4)?,
                                created_at: row.get(5)?,
                                chat_title: row.get(6)?,
                                archived: row.get(7)?,
                                on_active_branch: false,
                                snippet: String::new(),
                            },
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let count = candidates.len();
            for (cursor, mut hit) in candidates {
                after = Some(cursor.clone());
                let parts = text_statement
                    .query_map([cursor.rowid], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                let text = parts
                    .iter()
                    .map(|part| part.0.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                if scan || indexed_terms.len() != query.terms.len() {
                    let lower = text.to_lowercase();
                    if !lowered.iter().all(|term| lower.contains(term)) {
                        continue;
                    }
                }
                if page.hits.len() == query.limit {
                    page.cursor = Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&last_hit)?));
                    break 'pages;
                }
                let target = parts
                    .iter()
                    .find(|part| {
                        let text = part.0.to_lowercase();
                        lowered.iter().all(|term| text.contains(term))
                    })
                    .or_else(|| {
                        parts.iter().find(|part| {
                            let text = part.0.to_lowercase();
                            lowered.iter().any(|term| text.contains(term))
                        })
                    });
                if let Some(part) = target {
                    hit.part_id.clone_from(&part.1);
                    hit.spawn_part_id.clone_from(&part.2);
                }
                if !branches.contains_key(&hit.session_id) {
                    let mut stmt = snapshot.prepare(
                        "WITH RECURSIVE branch(id, parent_id) AS (
                            SELECT id, parent_id FROM chat_messages WHERE session_id = ?1 AND id = coalesce(
                                (SELECT active_leaf_id FROM chat_sessions WHERE id = ?1),
                                (SELECT id FROM chat_messages WHERE session_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1))
                            UNION
                            SELECT m.id, m.parent_id FROM chat_messages m JOIN branch b ON m.id = b.parent_id WHERE m.session_id = ?1
                         ) SELECT id FROM branch"
                    )?;
                    let ids = stmt
                        .query_map([&hit.session_id], |row| row.get::<_, String>(0))?
                        .collect::<std::result::Result<HashSet<_>, _>>()?;
                    branches.insert(hit.session_id.clone(), ids);
                }
                hit.on_active_branch = branches[&hit.session_id].contains(&hit.message_id);
                hit.snippet = snippet(
                    target.map_or(text.as_str(), |part| part.0.as_str()),
                    &lowered,
                );
                page.hits.push(hit);
                last_hit = Some(cursor);
            }
            if count < query.limit + 1 {
                break;
            }
        }
        Ok(page)
    }
}

fn snippet(text: &str, terms: &[String]) -> String {
    let chars: Vec<_> = text.chars().collect();
    let mut lower = String::new();
    let mut source = Vec::new();
    for (index, ch) in chars.iter().enumerate() {
        let folded = ch.to_lowercase().to_string();
        source.extend(std::iter::repeat_n(index, folded.len()));
        lower.push_str(&folded);
    }
    let mut anchor = None;
    for term in terms {
        let mut matches = lower.match_indices(term.as_str());
        if let Some((start, matched)) = matches.next() {
            let count = matches.count() + 1;
            if anchor.is_none_or(|(fewest, _, _)| count < fewest) {
                anchor = Some((count, source[start], source[start + matched.len() - 1] + 1));
            }
        }
    }
    let start = anchor.map_or(0, |(_, start, _)| start.saturating_sub(60));
    let end = (start + 240)
        .max(anchor.map_or(0, |(_, _, end)| end))
        .min(chars.len());
    let mut snippet = String::new();
    if start > 0 {
        snippet.push('…');
    }
    snippet.extend(&chars[start..end]);
    if end < chars.len() {
        snippet.push('…');
    }
    snippet
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoredChatMessage;

    fn with_store(test: impl FnOnce(Store)) {
        let dir = std::env::temp_dir().join(format!("orx-search-{}", uuid::Uuid::new_v4()));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            test(Store::open_at(dir.clone()).unwrap());
        }));
        std::fs::remove_dir_all(dir).unwrap();
        if let Err(error) = result {
            std::panic::resume_unwind(error);
        }
    }

    fn session(store: &Store, id: &str, project: &str) {
        store.conn.execute(
            "INSERT INTO chat_sessions(id, project_id, harness, title, created_at, updated_at) VALUES (?1, ?2, 'codex', ?1, 1, 1)",
            params![id, project],
        ).unwrap();
    }

    fn message(id: &str, session: &str, text: &str) -> StoredChatMessage {
        StoredChatMessage {
            id: id.into(),
            session_id: session.into(),
            role: "user".into(),
            parts_json: serde_json::json!([{"id":"text", "type":"text", "text":text}]).to_string(),
            created_at: 1,
            completed_at: None,
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
        }
    }

    fn request(q: &str, scope: SearchScope, id: &str) -> SearchRequest {
        SearchRequest {
            q: q.into(),
            scope,
            session_id: Some(id.into()),
            project_id: Some(id.into()),
            limit: Some(2),
            cursor: None,
        }
    }

    #[test]
    fn raw_sql_writers_and_streaming_upserts_keep_the_index_current() {
        with_store(|store| {
            session(&store, "chat", "project");
            let mut msg = message("message", "chat", "warmup schedule");
            store.conn.execute("INSERT INTO chat_messages(id, session_id, role, parts_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![msg.id, msg.session_id, msg.role, msg.parts_json, msg.created_at]).unwrap();
            let search = |q| {
                store
                    .search_chat_messages(&request(q, SearchScope::All, "").prepare().unwrap())
                    .unwrap()
            };
            assert_eq!(search("warmup").hits.len(), 1);
            let rowid: i64 = store
                .conn
                .query_row("SELECT rowid FROM chat_messages", [], |row| row.get(0))
                .unwrap();
            msg.parts_json = message("message", "chat", "cooldown schedule").parts_json;
            store.upsert_chat_message(&msg).unwrap();
            assert!(search("warmup").hits.is_empty());
            assert_eq!(search("cooldown").hits.len(), 1);
            assert_eq!(
                rowid,
                store
                    .conn
                    .query_row("SELECT rowid FROM chat_messages", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap()
            );
            store
                .conn
                .execute_batch("BEGIN; UPDATE chat_messages SET parts_json = '[]'; ROLLBACK;")
                .unwrap();
            assert_eq!(search("cooldown").hits.len(), 1);
            store.conn.execute("DELETE FROM chat_messages", []).unwrap();
            assert!(search("cooldown").hits.is_empty());
            assert_eq!(
                store
                    .conn
                    .query_row("SELECT count(*) FROM chat_search", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        });
    }

    #[test]
    fn nested_visible_parts_are_searchable_and_tool_output_is_excluded() {
        with_store(|store| {
            session(&store, "chat", "project");
            let mut msg = message("message", "chat", "");
            msg.parts_json = serde_json::json!([
                {"type":"text", "text":"commentary"},
                {"type":"steer", "text":"steering"},
                {"type":"reasoning", "text":"reasoning"},
                {"id":"outer", "type":"tool", "state":{"input":{"command":"cargo verify", "path":"src/optimizer.rs"}, "output":"invisible-output", "error":"invisible-error"}, "children":[
                    {"id":"inner", "type":"tool", "children":[{"id":"nested", "type":"text", "text":"nested-signal"}]}
                ]},
                {"type":"prompt", "prompt":{"question":"choose precision", "plan":"compare optimizers"}},
                {"id":"failure", "type":"tool", "state":{"status":"error", "error":"visible-failure"}},
                {"id":"report", "type":"tool", "tool":"Task", "state":{"status":"completed", "output":"visible-report"}},
                {"type":"tool", "tool":"Task", "state":{"status":"completed", "output":"Async agent launched hidden-acknowledgment"}},
                {"type":"tool", "tool":"Task", "state":{"status":"completed", "output":"hidden-duplicate"}, "children":[{"type":"text", "text":"visible-child"}]},
                {"type":"image", "text":"invisible-image"}
            ]).to_string();
            store.upsert_chat_message(&msg).unwrap();
            for q in [
                "commentary",
                "steering",
                "cargo verify",
                "optimizer.rs",
                "nested-signal",
                "precision",
                "optimizers",
                "visible-failure",
                "visible-report",
            ] {
                assert_eq!(
                    store
                        .search_chat_messages(&request(q, SearchScope::All, "").prepare().unwrap())
                        .unwrap()
                        .hits
                        .len(),
                    1,
                    "{q}"
                );
            }
            let nested = store
                .search_chat_messages(
                    &request("nested-signal", SearchScope::All, "")
                        .prepare()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(nested.hits[0].part_id.as_deref(), Some("nested"));
            assert_eq!(nested.hits[0].spawn_part_id.as_deref(), Some("inner"));
            let report = store
                .search_chat_messages(
                    &request("visible-report", SearchScope::All, "")
                        .prepare()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(report.hits[0].part_id.as_deref(), Some("report"));
            assert_eq!(report.hits[0].spawn_part_id.as_deref(), Some("report"));
            for q in [
                "reasoning",
                "invisible-output",
                "invisible-error",
                "invisible-image",
                "hidden-acknowledgment",
                "hidden-duplicate",
            ] {
                assert!(
                    store
                        .search_chat_messages(&request(q, SearchScope::All, "").prepare().unwrap())
                        .unwrap()
                        .hits
                        .is_empty(),
                    "{q}"
                );
            }
            msg.id = "shell".into();
            msg.role = "user".into();
            msg.parts_json = serde_json::json!([
                {"id":"shell-output", "type":"tool", "tool":"bash", "state":{"status":"completed", "output":"visible-shell"}}
            ]).to_string();
            store.upsert_chat_message(&msg).unwrap();
            let shell = store
                .search_chat_messages(
                    &request("visible-shell", SearchScope::All, "")
                        .prepare()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(shell.hits[0].part_id.as_deref(), Some("shell-output"));
        });
    }

    #[test]
    fn scopes_archive_and_branch_membership_follow_live_session_state() {
        with_store(|store| {
            for (id, project) in [
                ("chat", "project"),
                ("delegate", "project"),
                ("elsewhere", "other"),
            ] {
                session(&store, id, project);
            }
            for (id, chat) in [
                ("root", "chat"),
                ("active", "chat"),
                ("fork", "chat"),
                ("delegated", "delegate"),
                ("other", "elsewhere"),
            ] {
                let mut msg = message(id, chat, "searchable");
                if id == "active" || id == "fork" {
                    msg.parent_id = Some("root".into());
                }
                store.upsert_chat_message(&msg).unwrap();
            }
            store.conn.execute_batch("UPDATE chat_sessions SET active_leaf_id = 'active' WHERE id = 'chat'; UPDATE chat_sessions SET parent_session_id = 'chat', archived = 1 WHERE id = 'delegate';").unwrap();
            let search = |scope, id| {
                let mut req = request("searchable", scope, id);
                req.limit = Some(100);
                store.search_chat_messages(&req.prepare().unwrap()).unwrap()
            };
            let chat = search(SearchScope::Chat, "chat");
            assert_eq!(chat.hits.len(), 3);
            for hit in chat.hits {
                assert_eq!(hit.on_active_branch, hit.message_id != "fork");
            }
            let project = search(SearchScope::Project, "project");
            assert_eq!(project.hits.len(), 4);
            assert!(
                project
                    .hits
                    .iter()
                    .find(|hit| hit.session_id == "delegate")
                    .unwrap()
                    .archived
            );
            assert_eq!(search(SearchScope::All, "").hits.len(), 5);
            store.conn.execute("UPDATE chat_sessions SET project_id = 'other', archived = 0 WHERE id = 'delegate'", []).unwrap();
            assert_eq!(search(SearchScope::Project, "project").hits.len(), 3);
            assert!(!search(SearchScope::Project, "other")
                .hits
                .iter()
                .any(|hit| hit.archived));
        });
    }

    #[test]
    fn cursor_pages_ties_without_duplicates_and_rejects_another_search() {
        with_store(|store| {
            session(&store, "chat", "project");
            for i in 0..9 {
                store
                    .upsert_chat_message(&message(
                        &format!("message-{i}"),
                        "chat",
                        "learning schedule",
                    ))
                    .unwrap();
            }
            let mut cursor = None;
            let mut ids = Vec::new();
            loop {
                let mut req = request("learning", SearchScope::All, "");
                req.cursor = cursor;
                let page = store.search_chat_messages(&req.prepare().unwrap()).unwrap();
                ids.extend(page.hits.into_iter().map(|hit| hit.message_id));
                cursor = page.cursor;
                if cursor.is_none() {
                    break;
                }
                let mut changed = request("schedule", SearchScope::All, "");
                changed.cursor = cursor.clone();
                assert!(changed.prepare().is_err());
                assert!(ids.len() <= 9);
            }
            assert_eq!(
                ids,
                (0..9)
                    .rev()
                    .map(|i| format!("message-{i}"))
                    .collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn short_terms_filter_candidates_without_truncating_pages() {
        with_store(|store| {
            session(&store, "chat", "project");
            for i in 0..12 {
                store
                    .upsert_chat_message(&message(
                        &format!("message-{i}"),
                        "chat",
                        if i % 4 == 0 {
                            "learning z"
                        } else {
                            "learning q"
                        },
                    ))
                    .unwrap();
            }
            let query = request("learning z", SearchScope::All, "")
                .prepare()
                .unwrap();
            let first = store.search_chat_messages(&query).unwrap();
            assert_eq!(first.hits.len(), 2);
            let mut next = request("learning z", SearchScope::All, "");
            next.cursor = first.cursor;
            let last = store
                .search_chat_messages(&next.prepare().unwrap())
                .unwrap();
            assert_eq!(last.hits.len(), 1);
            assert!(last.cursor.is_none());
            assert_eq!(
                first
                    .hits
                    .iter()
                    .chain(&last.hits)
                    .map(|hit| hit.message_id.as_str())
                    .collect::<Vec<_>>(),
                ["message-8", "message-4", "message-0"]
            );
        });
    }

    #[test]
    fn scoped_cursor_handles_interleaved_chats_and_streaming_updates() {
        with_store(|store| {
            for (chat, project) in [("chat", "project"), ("other", "elsewhere")] {
                session(&store, chat, project);
            }
            for i in 0..20 {
                for chat in ["chat", "other"] {
                    store
                        .upsert_chat_message(&message(
                            &format!("{chat}-{i}"),
                            chat,
                            "learning schedule",
                        ))
                        .unwrap();
                }
            }
            let first = store
                .search_chat_messages(
                    &request("learning", SearchScope::Chat, "chat")
                        .prepare()
                        .unwrap(),
                )
                .unwrap();
            let mut ids: Vec<_> = first.hits.into_iter().map(|hit| hit.message_id).collect();
            store
                .upsert_chat_message(&message("new", "chat", "learning schedule"))
                .unwrap();
            store
                .upsert_chat_message(&message(
                    "chat-0",
                    "chat",
                    "learning schedule with much more context",
                ))
                .unwrap();
            let mut cursor = first.cursor;
            while cursor.is_some() {
                let mut req = request("learning", SearchScope::Chat, "chat");
                req.cursor = cursor;
                let page = store.search_chat_messages(&req.prepare().unwrap()).unwrap();
                ids.extend(page.hits.into_iter().map(|hit| hit.message_id));
                cursor = page.cursor;
                assert!(ids.len() <= 20);
            }
            assert_eq!(
                ids,
                (0..20)
                    .rev()
                    .map(|i| format!("chat-{i}"))
                    .collect::<Vec<_>>()
            );
        });
    }

    #[test]
    fn backfill_advances_past_a_message_larger_than_its_byte_budget() {
        with_store(|store| {
            session(&store, "chat", "project");
            store
                .upsert_chat_message(&message("large", "chat", &"large reply ".repeat(20_000)))
                .unwrap();
            store
                .upsert_chat_message(&message("small", "chat", "small reply"))
                .unwrap();
            assert!(store.backfill_chat_search().unwrap());
            assert_eq!(
                store
                    .conn
                    .query_row(
                        "SELECT through_rowid FROM chat_search_backfill",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                1
            );
            assert!(store.backfill_chat_search().unwrap());
            assert!(!store.backfill_chat_search().unwrap());
            assert_eq!(
                store
                    .search_chat_messages(
                        &request("reply", SearchScope::All, "").prepare().unwrap()
                    )
                    .unwrap()
                    .hits
                    .len(),
                2
            );
        });
    }

    #[test]
    fn literal_punctuation_and_unicode_queries_do_not_become_fts_syntax() {
        with_store(|store| {
            session(&store, "chat", "project");
            store
                .upsert_chat_message(&message(
                    "message",
                    "chat",
                    "ÉTÉ 中文测试 foo\"bar a*b NEAR(foo) src/optimizer.rs İstanbul",
                ))
                .unwrap();
            for q in [
                "İstanbul",
                "été",
                "中文测",
                "foo\"bar",
                "a*b",
                "NEAR(foo)",
                "optimizer.rs",
                "é",
                "中文",
            ] {
                let page = store
                    .search_chat_messages(&request(q, SearchScope::Chat, "chat").prepare().unwrap())
                    .unwrap();
                assert_eq!(page.hits.len(), 1, "{q}");
                assert!(
                    page.hits[0]
                        .snippet
                        .to_lowercase()
                        .contains(&q.to_lowercase()),
                    "{q}"
                );
            }
            assert!(
                store
                    .search_chat_messages(&request("中文", SearchScope::All, "").prepare().unwrap())
                    .unwrap()
                    .requires_longer_query
            );
            let empty = store
                .search_chat_messages(&request("  ", SearchScope::All, "").prepare().unwrap())
                .unwrap();
            assert!(empty.hits.is_empty());
            assert!(!empty.requires_longer_query);
        });
    }

    #[test]
    fn backfill_resumes_after_reopening_and_preserves_interleaved_writes() {
        with_store(|store| {
            session(&store, "chat", "project");
            store.conn.execute_batch("DROP TRIGGER chat_search_insert; DROP TRIGGER chat_search_update; DROP TRIGGER chat_search_delete;").unwrap();
            for i in 0..250 {
                store
                    .upsert_chat_message(&message(&format!("message-{i}"), "chat", "older history"))
                    .unwrap();
            }
            initialize(&store.conn).unwrap();
            assert!(store.backfill_chat_search().unwrap());
            let through: i64 = store
                .conn
                .query_row(
                    "SELECT through_rowid FROM chat_search_backfill",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(through, 100);
            let dir = store.data_dir.clone();
            drop(store);
            let reopened = Store::open_at(dir).unwrap();
            assert_eq!(
                reopened
                    .conn
                    .query_row(
                        "SELECT through_rowid FROM chat_search_backfill",
                        [],
                        |row| row.get::<_, i64>(0)
                    )
                    .unwrap(),
                through
            );
            reopened
                .upsert_chat_message(&message("message-0", "chat", "updated history"))
                .unwrap();
            reopened
                .upsert_chat_message(&message("message-249", "chat", "updated history"))
                .unwrap();
            reopened
                .conn
                .execute("DELETE FROM chat_messages WHERE id = 'message-101'", [])
                .unwrap();
            while reopened.backfill_chat_search().unwrap() {}
            let result = reopened
                .search_chat_messages(&request("updated", SearchScope::All, "").prepare().unwrap())
                .unwrap();
            assert_eq!(result.hits.len(), 2);
            assert!(!result.indexing);
            assert_eq!(
                reopened
                    .conn
                    .query_row("SELECT count(*) FROM chat_search", [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                249
            );
            initialize(&reopened.conn).unwrap();
            assert!(!reopened.backfill_chat_search().unwrap());
            assert_eq!(
                reopened
                    .conn
                    .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                    .unwrap(),
                2
            );
        });
    }

    #[test]
    fn snippets_preserve_unicode_boundaries_and_center_the_rarest_term() {
        let text = format!(
            "{}İstanbul <script> warmup schedule{}",
            "中".repeat(300),
            "文".repeat(300)
        );
        let plain = snippet(
            &text,
            &["i\u{307}stanbul".into(), "warmup".into(), "arm".into()],
        );
        assert!(plain.contains("İstanbul"));
        assert!(plain.contains("warmup"));
        assert!(plain.starts_with('…'));
        assert!(plain.ends_with('…'));
        assert!(plain.contains("<script>"));
        assert!(plain.chars().count() <= 242);
        let plan = format!(
            "{}The plan aurora beacon identifies the final step.",
            "Training plan. ".repeat(300)
        );
        let plain = snippet(&plan, &["plan".into(), "aurora".into()]);
        assert!(plain.contains("plan aurora"));
        assert!(plain.chars().count() <= 242);
        let long_term = "中".repeat(256);
        assert!(snippet(&long_term, std::slice::from_ref(&long_term)).contains(&long_term));
    }

    #[test]
    fn requests_bound_work_and_require_the_selected_scope() {
        let mut req = request("warmup", SearchScope::Chat, "");
        assert!(req.prepare().is_err());
        req = request(&"x".repeat(257), SearchScope::All, "");
        assert!(req.prepare().is_err());
        req = request("warmup", SearchScope::All, "");
        req.limit = Some(101);
        assert!(req.prepare().is_err());
        req = request("warmup", SearchScope::All, "");
        req.cursor = Some("bad-cursor".into());
        assert!(req.prepare().is_err());
        let selection = request(&"word ".repeat(17), SearchScope::All, "");
        assert_eq!(selection.prepare().unwrap().terms.len(), 16);
    }

    #[test]
    fn malformed_stored_json_neither_fails_writes_nor_stalls_backfill() {
        with_store(|store| {
            session(&store, "chat", "project");
            store
                .conn
                .execute_batch("DROP TRIGGER chat_search_insert;")
                .unwrap();
            store
                .upsert_chat_message(&message("legacy", "chat", "older history"))
                .unwrap();
            store
                .conn
                .execute("UPDATE chat_messages SET parts_json = 'not json'", [])
                .unwrap();
            initialize(&store.conn).unwrap();
            for (id, parts) in [
                ("string", "[\"abc\"]"),
                ("children", "[{\"type\":\"tool\",\"children\":\"x\"}]"),
            ] {
                store.conn.execute(
                    "INSERT INTO chat_messages(id, session_id, role, parts_json, created_at) VALUES (?1, 'chat', 'user', ?2, 1)",
                    params![id, parts],
                ).unwrap();
            }
            while store.backfill_chat_search().unwrap() {}
            assert!(
                !store
                    .search_chat_messages(
                        &request("older", SearchScope::All, "").prepare().unwrap()
                    )
                    .unwrap()
                    .indexing
            );
        });
    }

    #[test]
    #[ignore = "seeds one million messages; run explicitly for search latency"]
    fn benchmark_million_message_search() {
        with_store(|store| {
            let start = std::time::Instant::now();
            let tx = store.conn.unchecked_transaction().unwrap();
            for project in 0..100 {
                for chat in 0..10 {
                    session(
                        &store,
                        &format!("chat-{project}-{chat}"),
                        &format!("project-{project}"),
                    );
                }
            }
            tx.commit().unwrap();
            for batch in 0..1000 {
                let tx = store.conn.unchecked_transaction().unwrap();
                tx.execute(
                    "WITH RECURSIVE seq(n) AS (VALUES (?1) UNION ALL SELECT n + 1 FROM seq WHERE n < ?1 + 999)
                     INSERT INTO chat_messages(id, session_id, role, parts_json, created_at)
                     SELECT 'message-' || n, 'chat-' || (n / 10000) || '-' || ((n / 1000) % 10), 'user',
                        json_array(json_object('type', 'text', 'text', 'Learning rate schedule for experiment ' || n || CASE WHEN n % 1000 = 0 THEN ' raremarker' ELSE '' END)), n FROM seq",
                    [batch * 1000],
                ).unwrap();
                tx.commit().unwrap();
            }
            eprintln!("seeded in {:?}", start.elapsed());
            for (q, scope, id) in [
                ("raremarker", SearchScope::All, ""),
                ("learning", SearchScope::Chat, "chat-50-5"),
                ("learning", SearchScope::Project, "project-50"),
                ("learning", SearchScope::All, ""),
            ] {
                let mut req = request(q, scope, id);
                req.limit = Some(50);
                let query = req.prepare().unwrap();
                let mut timings = Vec::new();
                for _ in 0..20 {
                    let interrupt = store.conn.get_interrupt_handle();
                    let (done, completion) = std::sync::mpsc::channel();
                    let deadline = std::thread::spawn(move || {
                        if completion
                            .recv_timeout(std::time::Duration::from_secs(10))
                            .is_err()
                        {
                            interrupt.interrupt();
                        }
                    });
                    let start = std::time::Instant::now();
                    let result = store.search_chat_messages(&query);
                    timings.push(start.elapsed());
                    let _ = done.send(());
                    deadline.join().unwrap();
                    assert_eq!(result.unwrap().hits.len(), 50);
                }
                timings.sort_unstable();
                eprintln!("{scope:?} {q:?} p95 {:?}", timings[18]);
            }
        });
    }

    #[test]
    #[ignore = "measures streaming write and backfill lock duration"]
    fn benchmark_search_write_cost() {
        with_store(|store| {
            session(&store, "chat", "project");
            let text = "Compare learning rate schedules and optimizer settings. ".repeat(600);
            let tx = store.conn.unchecked_transaction().unwrap();
            store
                .conn
                .execute_batch("DROP TRIGGER chat_search_insert;")
                .unwrap();
            for i in 0..100 {
                store
                    .upsert_chat_message(&message(&format!("message-{i}"), "chat", &text))
                    .unwrap();
            }
            tx.commit().unwrap();
            initialize(&store.conn).unwrap();
            let start = std::time::Instant::now();
            let mut longest = std::time::Duration::ZERO;
            loop {
                let batch = std::time::Instant::now();
                let more = store.backfill_chat_search().unwrap();
                longest = longest.max(batch.elapsed());
                if !more {
                    break;
                }
            }
            eprintln!(
                "backfill 100 replies of {} bytes: {:?}, longest batch {:?}",
                text.len(),
                start.elapsed(),
                longest
            );
            let mut times = Vec::new();
            for i in 0..50 {
                let msg = message("message-0", "chat", &format!("{text} {i}"));
                let start = std::time::Instant::now();
                store.upsert_chat_message(&msg).unwrap();
                times.push(start.elapsed());
            }
            times.sort_unstable();
            eprintln!("streaming update p95: {:?}", times[47]);
        });
    }
}
