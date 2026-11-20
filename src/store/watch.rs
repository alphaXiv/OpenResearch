use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

/// One look the watcher took, or is about to take, at a chat's runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WatchLook {
    pub id: i64,
    pub chat_session_id: String,
    /// `change`, `ceiling`, `manual` or `terminal`.
    pub kind: String,
    /// `prepared`, `sent`, `done`, `interrupted` or `error`.
    pub state: String,
    /// What made the look due, in words.
    pub reason: String,
    pub created_at: i64,
    pub done_at: Option<i64>,
    pub summary_template: Option<String>,
    pub summary_text: Option<String>,
    pub usage: Option<crate::local::harness::OneShotUsage>,
    pub error: Option<String>,
}

/// What one look did for one of its runs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WatchLookRun {
    pub run_id: String,
    /// The run's name in the look's digest and reply: `r1`, `r2`, …
    pub key: String,
    /// `subject` for a run the look is about, `rider` for a live sibling a terminal look takes along.
    pub role: String,
    /// `line`, `same` or `flag` for an answer, `error` or `interrupted` for none; None while pending.
    pub result: Option<String>,
    pub template: Option<String>,
    /// The template filled with the numbers of its moment.
    pub text: Option<String>,
    /// The bar's fraction at the look.
    pub whole: Option<f64>,
    /// What the sentence stood on, checked on every new point.
    pub basis: Value,
    /// How far the look read the run's log, and the kinds of warning it had seen.
    pub cursor: Value,
}

/// What a finished look wrote for one run.
pub(crate) struct LookResult {
    pub run_id: String,
    pub result: String,
    pub template: Option<String>,
    pub text: Option<String>,
    pub whole: Option<f64>,
    pub basis: Value,
    pub cursor: Value,
}

/// How many times a run's last look is taken before its failure stands.
const TERMINAL_TRIES: i64 = 2;

pub(crate) type WatchCount = (String, u64, u64, String);

const SELECT_LOOK: &str = "SELECT id, chat_session_id, kind, state, reason, created_at, done_at,
        summary_template, summary_text, usage_json, error FROM watch_looks";

fn row_to_look(row: &rusqlite::Row) -> rusqlite::Result<WatchLook> {
    Ok(WatchLook {
        id: row.get(0)?,
        chat_session_id: row.get(1)?,
        kind: row.get(2)?,
        state: row.get(3)?,
        reason: row.get(4)?,
        created_at: row.get(5)?,
        done_at: row.get(6)?,
        summary_template: row.get(7)?,
        summary_text: row.get(8)?,
        usage: row
            .get::<_, Option<String>>(9)?
            .and_then(|json| serde_json::from_str(&json).ok()),
        error: row.get(10)?,
    })
}

fn row_to_look_run(row: &rusqlite::Row, start: usize) -> rusqlite::Result<WatchLookRun> {
    let json = |index| -> rusqlite::Result<Value> {
        Ok(row
            .get::<_, Option<String>>(start + index)?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or(Value::Null))
    };
    Ok(WatchLookRun {
        run_id: row.get(start)?,
        key: row.get(start + 1)?,
        role: row.get(start + 2)?,
        result: row.get(start + 3)?,
        template: row.get(start + 4)?,
        text: row.get(start + 5)?,
        whole: row.get(start + 6)?,
        basis: json(7)?,
        cursor: json(8)?,
    })
}

impl Store {
    /// Records a look before it is taken: its runs, each under its key, are fixed from here on.
    pub(crate) fn prepare_watch_look(
        tx: &rusqlite::Transaction<'_>,
        chat_session_id: &str,
        kind: &str,
        reason: &str,
        runs: &[(&str, &str, &str)],
        now: i64,
    ) -> Result<i64> {
        let chat_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_sessions WHERE id = ?1)",
            [chat_session_id],
            |row| row.get(0),
        )?;
        if !chat_exists {
            return Err(anyhow!(
                "Cannot record a watcher look for deleted chat {chat_session_id}"
            ));
        }

        let mut exists = tx.prepare_cached("SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)")?;
        for (run, _, _) in runs {
            if !exists.query_row([run], |row| row.get::<_, bool>(0))? {
                return Err(anyhow!(
                    "Cannot record a watcher look for deleted run {run}"
                ));
            }
        }

        tx.execute(
            "INSERT INTO watch_looks (chat_session_id, kind, state, reason, created_at)
             VALUES (?1, ?2, 'prepared', ?3, ?4)",
            params![chat_session_id, kind, reason, now],
        )?;
        let id = tx.last_insert_rowid();
        for (run_id, key, role) in runs {
            tx.execute(
                "INSERT INTO watch_look_runs (look_id, run_id, key, role) VALUES (?1, ?2, ?3, ?4)",
                params![id, run_id, key, role],
            )?;
        }
        Ok(id)
    }

    /// Claims the first send only while the look is prepared and watching is enabled.
    pub(crate) fn watch_look_sent(&self, id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE watch_looks SET state = 'sent'
             WHERE id = ?1 AND state = 'prepared'
               AND EXISTS (SELECT 1 FROM ui_state WHERE id = 1 AND run_watching)",
            [id],
        )? > 0)
    }

    /// Settles a look that was sent: its state, what it cost, its summary, what it wrote for
    /// each run and the flags it raised, in one transaction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finish_watch_look(
        &self,
        id: i64,
        state: &str,
        now: i64,
        usage: Option<&crate::local::harness::OneShotUsage>,
        error: Option<&str>,
        summary: Option<(&str, &str, usize)>,
        results: &[LookResult],
        flags: &[NewFlag],
    ) -> Result<()> {
        let tx = self.begin_immediate()?;
        let runs = self.watch_look_runs(id)?;
        let summary = summary.filter(|&(_, _, size)| size == runs.len());
        let updated = tx.execute(
            "UPDATE watch_looks SET state = ?2, done_at = ?3, usage_json = ?4, error = ?5,
                    summary_template = ?6, summary_text = ?7
             WHERE id = ?1",
            params![
                id,
                state,
                now,
                usage.map(serde_json::to_string).transpose()?,
                error,
                summary.map(|s| s.0),
                summary.map(|s| s.1),
            ],
        )?;
        if updated == 0 {
            return Ok(());
        }

        let flags = flags
            .iter()
            .filter(|flag| runs.iter().any(|row| row.run_id == flag.run_id));
        // A look's flag on a run with one still open joins that one, as what the look noted.
        let raised = self.append_flags(flags.clone(), now)?;
        let mut noted = BTreeSet::new();
        for (flag, raise) in flags.zip(raised) {
            if raise == Raise::Stopped {
                noted.insert(flag.run_id.as_str());
                continue;
            }
            if raise != Raise::Waits {
                continue;
            }
            let events = self.run_flag_events(&flag.run_id)?;
            let open =
                super::flags::open_flag(&events).expect("Raise::Waits requires an open flag");
            let body = serde_json::json!({
                "flagId": open,
                "lookId": flag.body["lookId"],
                "text": flag.body["text"],
                "evidence": flag.body["evidence"],
            });
            tx.execute(
                "INSERT INTO run_monitor_events (run_id, seq, kind, created_at, body_json)
                 VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM run_monitor_events WHERE run_id = ?1),
                         'noted', ?2, ?3)",
                params![flag.run_id, now, body.to_string()],
            )?;
            for (chat, wake) in &flag.noted {
                let dedupe = format!("noted:{id}");
                self.insert_flag_wake(&flag.run_id, chat, open, &dedupe, wake, now)?;
            }
            noted.insert(flag.run_id.as_str());
        }
        for result in results {
            tx.execute(
                "UPDATE watch_look_runs SET result = ?3, template = ?4, text = ?5, whole = ?6,
                        basis_json = ?7, cursor_json = ?8
                 WHERE look_id = ?1 AND run_id = ?2",
                params![
                    id,
                    result.run_id,
                    match result.result.as_str() {
                        "flag" if noted.contains(result.run_id.as_str()) => "line",
                        said => said,
                    },
                    result.template,
                    result.text,
                    result.whole,
                    serde_json::to_string(&result.basis)?,
                    serde_json::to_string(&result.cursor)?,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Looks a stopped `orx up` left behind. Each sent one is recorded as interrupted, since its
    /// model may have answered and is never asked again; the prepared ones are returned, to take.
    pub(crate) fn recover_watch_looks(&self, now: i64) -> Result<Vec<WatchLook>> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE watch_look_runs SET result = 'interrupted'
             WHERE result IS NULL AND look_id IN (SELECT id FROM watch_looks WHERE state = 'sent')",
            [],
        )?;
        tx.execute(
            "UPDATE watch_looks SET state = 'interrupted', done_at = ?1 WHERE state = 'sent'",
            [now],
        )?;
        tx.commit()?;
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_LOOK} WHERE state = 'prepared' ORDER BY id"
        ))?;
        let rows = stmt.query_map([], row_to_look)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The chat's looks still to take, oldest first.
    pub(crate) fn prepared_watch_looks(&self, chat_session_id: &str) -> Result<Vec<WatchLook>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_LOOK} WHERE chat_session_id = ?1 AND state = 'prepared' ORDER BY id"
        ))?;
        let rows = stmt.query_map([chat_session_id], row_to_look)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Every chat with a look still to take.
    pub(crate) fn prepared_watch_looks_chats(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT chat_session_id FROM watch_looks WHERE state = 'prepared'")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn watch_look(&self, id: i64) -> Result<Option<WatchLook>> {
        Ok(self
            .conn
            .query_row(&format!("{SELECT_LOOK} WHERE id = ?1"), [id], row_to_look)
            .optional()?)
    }

    pub(crate) fn watch_look_runs(&self, look_id: i64) -> Result<Vec<WatchLookRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, key, role, result, template, text, whole, basis_json, cursor_json
             FROM watch_look_runs WHERE look_id = ?1 ORDER BY key",
        )?;
        let rows = stmt.query_map([look_id], |row| row_to_look_run(row, 0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The run's looks, oldest first, at most the newest `limit`.
    pub(crate) fn run_watch_looks(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<(WatchLook, WatchLookRun)>> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.chat_session_id, l.kind, l.state, l.reason, l.created_at,
                    l.done_at, l.summary_template, l.summary_text, l.usage_json, l.error,
                    r.run_id, r.key, r.role, r.result, r.template, r.text, r.whole,
                    r.basis_json, r.cursor_json
             FROM watch_look_runs r JOIN watch_looks l ON l.id = r.look_id
             WHERE r.run_id = ?1 ORDER BY l.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id, limit as i64], |row| {
            Ok((row_to_look(row)?, row_to_look_run(row, 11)?))
        })?;
        let mut looks = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        looks.reverse();
        Ok(looks)
    }

    /// Each of the run's looks with the other runs it covered, by their experiments' names.
    pub(crate) fn watch_look_company(&self, run_id: &str) -> Result<Vec<(i64, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT o.look_id, COALESCE(NULLIF(TRIM(e.title), ''), e.slug, x.experiment_id)
             FROM watch_look_runs r
             JOIN watch_look_runs o ON o.look_id = r.look_id AND o.run_id != r.run_id
             LEFT JOIN runs x ON x.id = o.run_id
             LEFT JOIN local_experiments e ON e.id = x.experiment_id
             WHERE r.run_id = ?1 ORDER BY o.look_id, o.key",
        )?;
        let rows = stmt.query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Changes whenever a look settles for the run or one of its counts moves.
    pub(crate) fn run_watch_version(&self, run_id: &str) -> Result<String> {
        Ok(self.conn.query_row(
            "SELECT (SELECT IFNULL(MAX(look_id), 0) || ':' || COUNT(result) FROM watch_look_runs WHERE run_id = ?1)
                    || ':' || (SELECT IFNULL(SUM(lines), 0) || ':' || COUNT(*) FROM watch_counts WHERE run_id = ?1)",
            [run_id],
            |row| row.get(0),
        )?)
    }

    /// The chat's pending looks and those from `since` on, oldest first.
    pub(crate) fn chat_has_watch_look_since(&self, chat: &str, since: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM watch_looks WHERE chat_session_id = ?1
               AND (created_at >= ?2 OR state IN ('prepared', 'sent')))",
            params![chat, since],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn chat_watch_looks(
        &self,
        chat_session_id: &str,
        since: i64,
    ) -> Result<Vec<WatchLook>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_LOOK} WHERE chat_session_id = ?1
               AND (created_at >= ?2 OR state IN ('prepared', 'sent')) ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![chat_session_id, since], row_to_look)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn watch_auth_refusal(
        &self,
        harness: &str,
        profile: Option<&str>,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT revision FROM watch_auth_refusals WHERE harness = ?1 AND profile = ?2",
                params![harness, profile.unwrap_or_default()],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn record_watch_auth_refusal(
        &self,
        harness: &str,
        profile: Option<&str>,
        revision: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO watch_auth_refusals (harness, profile, revision) VALUES (?1, ?2, ?3)
             ON CONFLICT(harness, profile) DO UPDATE SET revision = excluded.revision",
            params![harness, profile.unwrap_or_default(), revision],
        )?;
        Ok(())
    }

    pub(crate) fn last_chat_watch_at(&self, chat: &str) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT MAX(created_at) FROM watch_looks WHERE chat_session_id = ?1",
            [chat],
            |row| row.get(0),
        )?)
    }

    /// The chat that last looked at the run, which keeps watching it.
    pub(crate) fn run_watch_owner(&self, run_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT l.chat_session_id FROM watch_look_runs r JOIN watch_looks l ON l.id = r.look_id
                 WHERE r.run_id = ?1 ORDER BY l.id DESC LIMIT 1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// How far each counted pattern has read the run's log: its count, the next byte, the log's identity.
    pub(crate) fn watch_counts(&self, run_id: &str) -> Result<Vec<WatchCount>> {
        let mut stmt = self.conn.prepare(
            "SELECT pattern, lines, offset, ident FROM watch_counts WHERE run_id = ?1 ORDER BY pattern",
        )?;
        let rows = stmt.query_map([run_id], |row| {
            Ok((
                row.get(0)?,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, i64>(2)? as u64,
                row.get(3)?,
            ))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// A look's backfill may have advanced the cursor while the watcher read its log.
    pub(crate) fn advance_watch_count(
        &self,
        run_id: &str,
        before: &WatchCount,
        lines: u64,
        offset: u64,
        ident: &str,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE watch_counts SET lines = ?6, offset = ?7, ident = ?8
             WHERE run_id = ?1 AND pattern = ?2 AND lines = ?3 AND offset = ?4 AND ident = ?5",
            params![
                run_id,
                before.0,
                before.1 as i64,
                before.2 as i64,
                before.3,
                lines as i64,
                offset as i64,
                ident
            ],
        )?;
        Ok(())
    }

    pub(crate) fn set_watch_count(
        &self,
        run_id: &str,
        pattern: &str,
        lines: u64,
        offset: u64,
        ident: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO watch_counts (run_id, pattern, lines, offset, ident)
             SELECT id, ?2, ?3, ?4, ?5 FROM runs WHERE id = ?1
             ON CONFLICT(run_id, pattern) DO UPDATE SET lines = ?3, offset = ?4, ident = ?5",
            params![run_id, pattern, lines as i64, offset as i64, ident],
        )?;
        Ok(())
    }
}

impl Store {
    /// Runs that ended at `since` or later, newest first.
    pub(crate) fn runs_ended_since(&self, since: i64) -> Result<Vec<StoredRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_RUN} WHERE ended_at >= ?1 ORDER BY ended_at DESC LIMIT 256"
        ))?;
        let rows = stmt.query_map([since], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The chats that follow a run, the one that launched it first, then by when they subscribed.
    pub(crate) fn run_followers(&self, run_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT chat_session_id FROM (
                 SELECT chat_session_id, 0 AS at FROM runs WHERE id = ?1 AND chat_session_id IS NOT NULL
                 UNION ALL SELECT chat_session_id, created_at FROM run_subscriptions WHERE run_id = ?1)
             GROUP BY chat_session_id ORDER BY MIN(at), chat_session_id",
        )?;
        let rows = stmt.query_map([run_id], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Whether the run needs no more looks after it ended: one answered, or is still to answer,
    /// or [`TERMINAL_TRIES`] of them failed.
    pub(crate) fn run_watch_terminal_done(&self, run_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT COUNT(CASE WHEN r.result IS NULL OR r.result IN ('line', 'same', 'flag') THEN 1 END) > 0
                    OR COUNT(CASE WHEN r.result IN ('error', 'interrupted') THEN 1 END) >= ?2
             FROM watch_look_runs r JOIN watch_looks l ON l.id = r.look_id
             WHERE r.run_id = ?1 AND l.kind = 'terminal' AND r.role = 'subject'",
            params![run_id, TERMINAL_TRIES],
            |row| row.get(0),
        )?)
    }
}
