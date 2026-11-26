use super::*;

/// Notes a report keeps for its next update; an agent that marks more drops its oldest.
pub(crate) const NOTES_KEPT: usize = 20;

/// A chat's status report, kept once the agent noted a milestone, it opened or the user chose:
/// the newest version its watching model wrote, whether it is open, the last update that failed,
/// and what the next update is due for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChatReport {
    pub body: Option<String>,
    /// The id of the newest version, which wrote `body`.
    pub version: Option<i64>,
    /// Whether the user marked the newest version read, so the next one leaves out what it says.
    pub read: bool,
    /// When the inputs of the update that last wrote or confirmed the body were read, in ms.
    pub updated_at: Option<i64>,
    /// None until the report opens or the user chooses; false once the user turned it off.
    pub enabled: Option<bool>,
    /// Why the last update failed or could not start, until one succeeds or the user retries.
    pub error: Option<String>,
    /// When the last update failed or could not start; a retry keeps it, and so the gap.
    pub failed_at: Option<i64>,
    /// What the agent said changed since the last update, by id, oldest first.
    pub notes: Vec<(i64, String)>,
    /// The latest milestone that waits for nothing.
    pub due_at: Option<i64>,
    /// The latest milestone that waits for the chat's turns running at it to end.
    pub turn_due_at: Option<i64>,
}

/// What an update took of its report when it started: when it read its inputs, the newest note it
/// read and the milestones it covers, which it settles when it ends unless newer ones replaced them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReportTaken {
    pub read_at: i64,
    pub last_note: Option<i64>,
    pub due_at: Option<i64>,
    pub turn_due_at: Option<i64>,
}

pub(crate) enum ReportOutcome<'a> {
    Written(&'a str),
    /// The model confirmed the newest version is current, which writes no version.
    Unchanged,
    Failed(&'a str),
}

impl Store {
    pub(crate) fn chat_report(&self, chat: &str) -> Result<Option<ChatReport>> {
        let report = self
            .conn
            .query_row(
                "SELECT v.body, MAX(COALESCE(v.written_at, r.confirmed_at),
                                    COALESCE(r.confirmed_at, v.written_at)),
                        r.error, r.error_at, r.due_at, r.turn_due_at, r.enabled,
                        v.id, r.read_version IS NOT NULL AND r.read_version = v.id
                 FROM chat_reports r
                 LEFT JOIN chat_report_versions v ON v.id = (
                     SELECT MAX(id) FROM chat_report_versions
                     WHERE chat_session_id = r.chat_session_id)
                 WHERE r.chat_session_id = ?1",
                [chat],
                |row| {
                    Ok(ChatReport {
                        body: row.get(0)?,
                        updated_at: row.get(1)?,
                        error: row.get(2)?,
                        failed_at: row.get(3)?,
                        due_at: row.get(4)?,
                        turn_due_at: row.get(5)?,
                        enabled: row.get(6)?,
                        version: row.get(7)?,
                        read: row.get::<_, Option<bool>>(8)?.unwrap_or(false),
                        notes: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut report) = report else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT id, note FROM chat_report_notes WHERE chat_session_id = ?1 ORDER BY id",
        )?;
        let notes = stmt.query_map([chat], |row| Ok((row.get(0)?, row.get(1)?)))?;
        report.notes = notes.collect::<std::result::Result<_, _>>()?;
        Ok(Some(report))
    }

    /// Marks the chat's report read up to `version`; false when the chat has no such version.
    pub(crate) fn mark_chat_report_read(&self, chat: &str, version: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE chat_reports SET read_version = MAX(COALESCE(read_version, 0), ?2)
             WHERE chat_session_id = ?1 AND EXISTS (
                 SELECT 1 FROM chat_report_versions WHERE chat_session_id = ?1 AND id = ?2)",
            params![chat, version],
        )? > 0)
    }

    /// Every version of the chat's report, oldest first, as when it was written and its body.
    #[cfg(test)]
    pub(crate) fn chat_report_versions(&self, chat: &str) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT written_at, body FROM chat_report_versions WHERE chat_session_id = ?1
             ORDER BY id",
        )?;
        let rows = stmt.query_map([chat], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Keeps `note` for the chat's next update unless the user turned its report off, and makes an
    /// open report due now.
    pub(crate) fn chat_report_due(&self, chat: &str, note: Option<&str>, now: i64) -> Result<()> {
        let tx = self.begin_immediate()?;
        self.conn.execute(
            "INSERT INTO chat_reports (chat_session_id) VALUES (?1)
             ON CONFLICT(chat_session_id) DO NOTHING",
            [chat],
        )?;
        if let Some(note) = note {
            self.conn.execute(
                "INSERT INTO chat_report_notes (chat_session_id, note)
                 SELECT ?1, ?2 FROM chat_reports WHERE chat_session_id = ?1 AND enabled IS NOT 0",
                params![chat, note],
            )?;
            self.conn.execute(
                "DELETE FROM chat_report_notes WHERE chat_session_id = ?1 AND id NOT IN (
                     SELECT id FROM chat_report_notes WHERE chat_session_id = ?1
                     ORDER BY id DESC LIMIT ?2)",
                params![chat, NOTES_KEPT as i64],
            )?;
        }
        self.conn.execute(
            "UPDATE chat_reports
             SET due_at = CASE WHEN enabled THEN MAX(COALESCE(due_at, 0), ?2) ELSE due_at END
             WHERE chat_session_id = ?1",
            params![chat, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Makes the chat's open report due once the turns it has running now end.
    pub(crate) fn chat_report_due_after_turn(&self, chat: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_reports SET turn_due_at = MAX(COALESCE(turn_due_at, 0), ?2)
             WHERE chat_session_id = ?1 AND enabled",
            params![chat, now],
        )?;
        Ok(())
    }

    /// Opens the chat's report unless it is open or turned off; its first update waits for the turns
    /// running now to end, as a result's reaction does.
    pub(crate) fn open_chat_report(&self, chat: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat_reports (chat_session_id, enabled, turn_due_at) VALUES (?1, 1, ?2)
             ON CONFLICT(chat_session_id) DO UPDATE SET enabled = 1, turn_due_at = ?2
                 WHERE enabled IS NULL",
            params![chat, now],
        )?;
        Ok(())
    }

    /// The user's choice: on makes the report due now, off drops whatever was due.
    pub(crate) fn set_chat_report_enabled(
        &self,
        chat: &str,
        enabled: bool,
        now: i64,
    ) -> Result<()> {
        let tx = self.begin_immediate()?;
        self.conn.execute(
            "INSERT INTO chat_reports (chat_session_id, enabled, due_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(chat_session_id) DO UPDATE SET enabled = ?2, due_at = ?3,
                 turn_due_at = NULL",
            params![chat, enabled, enabled.then_some(now)],
        )?;
        if !enabled {
            self.conn.execute(
                "DELETE FROM chat_report_notes WHERE chat_session_id = ?1",
                [chat],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The chats whose report has not opened, and whose user has not chosen, that went on working
    /// past one of their runs' end: the chat launched another run, or some turn kept going `after`
    /// ms past the later of the run's end and its own start. Only turns going now or ended, and
    /// launches made, within `recent` ms are read.
    pub(crate) fn chats_opening_reports(
        &self,
        now: i64,
        after: i64,
        recent: i64,
    ) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.session_id FROM chat_turns t JOIN runs r
                 ON r.chat_session_id = t.session_id
                 OR r.id IN (SELECT run_id FROM run_subscriptions s
                             WHERE s.chat_session_id = t.session_id)
             WHERE r.ended_at IS NOT NULL
               AND NOT EXISTS (SELECT 1 FROM chat_reports c
                               WHERE c.chat_session_id = t.session_id AND c.enabled IS NOT NULL)
               AND (t.state IN ('preparing', 'retrying', 'running') OR t.updated_at > ?1 - ?3)
               AND CASE WHEN t.state IN ('preparing', 'retrying', 'running') THEN ?1
                        ELSE t.updated_at END >= MAX(r.ended_at, t.created_at) + ?2
             UNION
             SELECT later.chat_session_id FROM runs later JOIN runs r
                 ON r.chat_session_id = later.chat_session_id
                 OR r.id IN (SELECT run_id FROM run_subscriptions s
                             WHERE s.chat_session_id = later.chat_session_id)
             WHERE later.chat_session_id IS NOT NULL AND later.created_at > ?1 - ?3
               AND r.ended_at IS NOT NULL AND later.created_at > r.ended_at
               AND later.id <> r.id
               AND NOT EXISTS (SELECT 1 FROM chat_reports c
                               WHERE c.chat_session_id = later.chat_session_id
                                 AND c.enabled IS NOT NULL)",
        )?;
        let rows = stmt.query_map(params![now, after, recent], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The chats whose report has a milestone waiting.
    pub(crate) fn due_chat_reports(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT chat_session_id FROM chat_reports
             WHERE enabled AND (due_at IS NOT NULL OR turn_due_at IS NOT NULL)",
        )?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Whether a turn of the chat that started at `at` or earlier is still going.
    pub(crate) fn chat_turn_running_from(&self, chat: &str, at: i64) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM chat_turns WHERE session_id = ?1 AND created_at <= ?2
                              AND state IN ('preparing', 'retrying', 'running'))",
            params![chat, at],
            |row| row.get(0),
        )?)
    }

    /// Records how an update ended and settles what it took.
    pub(crate) fn finish_chat_report(
        &self,
        chat: &str,
        outcome: ReportOutcome,
        taken: ReportTaken,
        now: i64,
    ) -> Result<()> {
        let settled = "due_at = CASE WHEN due_at = ?2 THEN NULL ELSE due_at END,
                       turn_due_at = CASE WHEN turn_due_at = ?3 THEN NULL ELSE turn_due_at END";
        let tx = self.begin_immediate()?;
        match outcome {
            ReportOutcome::Written(body) => {
                let changed = self.conn.execute(
                    &format!(
                        "UPDATE chat_reports SET {settled}, error = NULL, error_at = NULL
                         WHERE chat_session_id = ?1"
                    ),
                    params![chat, taken.due_at, taken.turn_due_at],
                )?;
                if changed > 0 {
                    self.conn.execute(
                        "INSERT INTO chat_report_versions (chat_session_id, written_at, body)
                         VALUES (?1, ?2, ?3)",
                        params![chat, taken.read_at, body],
                    )?;
                }
            }
            ReportOutcome::Unchanged => {
                self.conn.execute(
                    &format!(
                        "UPDATE chat_reports SET {settled}, error = NULL, error_at = NULL,
                             confirmed_at = ?4
                         WHERE chat_session_id = ?1"
                    ),
                    params![chat, taken.due_at, taken.turn_due_at, taken.read_at],
                )?;
            }
            ReportOutcome::Failed(message) => {
                self.conn.execute(
                    &format!(
                        "UPDATE chat_reports SET {settled}, error = ?4, error_at = ?5
                         WHERE chat_session_id = ?1"
                    ),
                    params![chat, taken.due_at, taken.turn_due_at, message, now],
                )?;
            }
        }
        if !matches!(outcome, ReportOutcome::Failed(_)) {
            self.conn.execute(
                "DELETE FROM chat_report_notes WHERE chat_session_id = ?1 AND id <= ?2",
                params![chat, taken.last_note],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records why the report cannot update, leaving its milestones due; false when it already
    /// said so.
    pub(crate) fn chat_report_blocked(&self, chat: &str, message: &str, now: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE chat_reports SET error = ?2, error_at = ?3
             WHERE chat_session_id = ?1 AND error IS NOT ?2",
            params![chat, message, now],
        )? > 0)
    }

    /// The user's Retry: drops the recorded error, keeping when it happened.
    pub(crate) fn clear_chat_report_error(&self, chat: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE chat_reports SET error = NULL WHERE chat_session_id = ?1",
            [chat],
        )?;
        Ok(())
    }

    /// The runs the chat launched or follows that changed after `since`, and every one still
    /// going, newest first.
    pub(crate) fn chat_report_runs(
        &self,
        chat: &str,
        since: i64,
        limit: usize,
    ) -> Result<Vec<StoredRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "{SELECT_RUN} WHERE (chat_session_id = ?1
                     OR id IN (SELECT run_id FROM run_subscriptions WHERE chat_session_id = ?1))
                 AND (updated_at > ?2 OR status IN ('submitting', 'starting', 'running'))
             ORDER BY created_at DESC LIMIT ?3"
        ))?;
        let rows = stmt.query_map(params![chat, since, limit as i64], row_to_run)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The run notices delivered to the chat after `since`, as each run's id and the notice,
    /// newest first.
    pub(crate) fn chat_report_notices(
        &self,
        chat: &str,
        since: i64,
        limit: usize,
    ) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT run_id, body FROM run_notices WHERE chat_session_id = ?1 AND delivered_at > ?2
             ORDER BY delivered_at DESC, id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![chat, since, limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}
