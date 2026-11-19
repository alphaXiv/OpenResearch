use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

pub const WATCH_FLAG: &str = "watch_flag";
pub(crate) const FLAG_DELIVERY_UNCONFIRMED: &str =
    "The chat turn was accepted without confirming that the flag reached the agent";

pub(crate) fn flag_ask_dedupe(request: Option<&str>) -> String {
    format!("ask:{}", request.unwrap_or_default())
}

/// A flag to append to a run's journal, with the notice each chat it wakes is sent.
pub(crate) struct NewFlag {
    pub run_id: String,
    /// The flag as the journal keeps it: id, source, fingerprint, sentence, evidence.
    pub body: Value,
    /// Chat, then the frozen wake text.
    pub wakes: Vec<(String, String)>,
    /// What wakes the same chats instead when a look's flag is noted under one still open.
    pub noted: Vec<(String, String)>,
    /// Raised only while the run has no flag still open, so one trouble wakes once.
    pub only_if_clear: bool,
}

/// What became of a flag asked to be raised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Raise {
    Raised,
    /// The run was cancelled or its stop was accepted.
    Stopped,
    /// The run raised this fingerprint before.
    Repeated,
    /// The run has a flag still open.
    Waits,
}

/// What a run's journal holds of one flag or choice.
#[derive(Debug, Clone)]
pub(crate) struct FlagEvent {
    pub seq: i64,
    pub kind: String,
    pub at: i64,
    pub body: Value,
}

/// How far a notice that wakes a chat with a flag got.
#[derive(Debug, Clone)]
pub(crate) struct FlagNotice {
    pub id: i64,
    pub flag_id: String,
    pub chat_session_id: String,
    pub dedupe: String,
    pub claimed: bool,
    pub delivered: bool,
    pub retired: bool,
    /// The state of the turn that carries it, once there is one.
    pub turn: Option<String>,
}

/// A choice on one run's flag, with the notices it wakes chats with: chat, then frozen text.
pub(crate) struct FlagChoice {
    pub run_id: String,
    pub body: Value,
    pub wakes: Vec<(String, String)>,
}

/// What recording a choice did.
#[derive(Debug, PartialEq)]
pub(crate) enum Chosen {
    Recorded,
    /// A choice with this request id is already kept.
    Repeated,
    /// Another choice settled the flag first.
    Settled,
}

fn settles(event: &FlagEvent) -> bool {
    event.kind == "choice" && matches!(event.body["c"].as_str(), Some("keep" | "stop"))
}

/// The id of the run's newest flag no choice settled.
pub(super) fn open_flag(events: &[FlagEvent]) -> Option<&str> {
    let settled: BTreeSet<&str> = events
        .iter()
        .filter(|event| settles(event))
        .filter_map(|event| event.body["flagId"].as_str())
        .collect();
    events
        .iter()
        .rev()
        .filter(|event| event.kind == "flag")
        .filter_map(|event| event.body["id"].as_str())
        .find(|id| !settled.contains(id))
}

impl Store {
    /// A claim can still be withdrawn until a chat turn has frozen its payload.
    pub(crate) fn retire_unadmitted_flag_wakes(&self, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE flag_wakes AS w SET retired_at = ?1, client_turn_id = NULL,
                    next_delivery_at = NULL, delivery_error = NULL
             WHERE delivered_at IS NULL AND retired_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM chat_turns t WHERE t.session_id = w.chat_session_id
                               AND t.client_turn_id = w.client_turn_id)
               AND NOT EXISTS (
                   SELECT 1 FROM runs r JOIN chat_sessions s ON s.id = w.chat_session_id
                   WHERE r.id = w.run_id AND r.cancel_requested = 0 AND r.cancel_accepted_at IS NULL
                     AND r.status NOT IN ('cancelled', 'stopping')
                     AND NOT EXISTS (SELECT 1 FROM run_notices n WHERE n.run_id = r.id
                                     AND n.chat_session_id = s.id AND n.dedupe = 'chat-done')
                     AND CASE WHEN w.dedupe GLOB 'ask:*' THEN (
                         SELECT json_extract(e.body_json, '$.c') = 'ask'
                             AND json_extract(e.body_json, '$.by') = 'user'
                             AND w.dedupe = 'ask:' || COALESCE(json_extract(e.body_json, '$.requestId'), '')
                         FROM run_monitor_events e WHERE e.run_id = r.id AND e.kind = 'choice'
                           AND json_extract(e.body_json, '$.flagId') = w.flag_id
                         ORDER BY e.seq DESC LIMIT 1
                     ) ELSE s.autonomy IS NOT 'copilot' AND s.archived = 0
                         AND (r.status NOT IN ('done', 'failed', 'lost')
                              OR COALESCE(r.ended_at, r.updated_at) < w.created_at)
                         AND NOT EXISTS (SELECT 1 FROM run_monitor_events e
                             WHERE e.run_id = r.id AND e.kind = 'choice'
                               AND json_extract(e.body_json, '$.flagId') = w.flag_id)
                     END
               )",
            [now],
        )?;
        Ok(())
    }

    /// Revalidates and reads one claim inside the transaction that admits its chat turn.
    pub(crate) fn admit_flag_wakes(&self, chat: &str, turn: &str, now: i64) -> Result<Vec<String>> {
        self.retire_unadmitted_flag_wakes(now)?;
        self.conn.execute(
            "UPDATE flag_wakes SET client_turn_id = NULL
             WHERE chat_session_id = ?1 AND client_turn_id = ?2
               AND delivered_at IS NULL AND retired_at IS NULL
               AND id NOT IN (SELECT id FROM pending_chat_run_outbox
                              WHERE chat_session_id = ?1 AND client_turn_id = ?2 AND is_flag)",
            params![chat, turn],
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT w.id, w.run_id, w.flag_id, w.body,
                    w.dedupe GLOB 'ask:*' AND r.status IN ('done', 'failed', 'lost')
             FROM flag_wakes w JOIN runs r ON r.id = w.run_id
             WHERE w.chat_session_id = ?1 AND w.client_turn_id = ?2
               AND w.delivered_at IS NULL AND w.retired_at IS NULL ORDER BY w.id",
        )?;
        let rows = stmt
            .query_map(params![chat, turn], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut bodies = Vec::with_capacity(rows.len());
        for (id, run, flag, mut body, ended) in rows {
            if ended {
                let run = self
                    .get_run(&run)?
                    .ok_or_else(|| anyhow!("Run {run} was deleted"))?;
                let event = self
                    .run_flag_events(&run.id)?
                    .into_iter()
                    .find(|event| event.kind == "flag" && event.body["id"] == flag)
                    .ok_or_else(|| anyhow!("Run {} has no flag {flag}", run.id))?;
                body = crate::engine::flags::wake(&run, &event.body, true);
                self.conn.execute(
                    "UPDATE flag_wakes SET body = ?2 WHERE id = ?1",
                    params![id, body],
                )?;
            }
            bodies.push(body);
        }
        Ok(bodies)
    }

    /// The run's flags, the choices made on them and what looks noted while one was open,
    /// oldest first.
    pub(crate) fn run_flag_events(&self, run_id: &str) -> Result<Vec<FlagEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, kind, created_at, body_json FROM run_monitor_events
             WHERE run_id = ?1 AND kind IN ('flag', 'choice', 'noted') ORDER BY seq",
        )?;
        let rows = stmt.query_map([run_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (seq, kind, at, body) = row?;
            let body = serde_json::from_str(&body)
                .map_err(|e| anyhow!("Run {run_id}'s flag event {seq} is unreadable: {e}"))?;
            events.push(FlagEvent {
                seq,
                kind,
                at,
                body,
            });
        }
        Ok(events)
    }

    /// The notices that woke, or will wake, chats with the run's flags.
    pub(crate) fn run_flag_notices(&self, run_id: &str) -> Result<Vec<FlagNotice>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.chat_session_id, n.dedupe, n.client_turn_id IS NOT NULL,
                    n.delivered_at IS NOT NULL,
                    n.retired_at IS NOT NULL, t.state, n.id, n.flag_id
             FROM flag_wakes n
             LEFT JOIN chat_turns t ON t.session_id = n.chat_session_id AND t.client_turn_id = n.client_turn_id
             WHERE n.run_id = ?1 ORDER BY n.id",
        )?;
        let rows = stmt.query_map([run_id], |row| {
            Ok(FlagNotice {
                chat_session_id: row.get(0)?,
                dedupe: row.get(1)?,
                claimed: row.get(2)?,
                delivered: row.get(3)?,
                retired: row.get(4)?,
                turn: row.get(5)?,
                id: row.get(6)?,
                flag_id: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn flag_notice_runs_for_turn(&self, turn_id: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT n.run_id FROM chat_turns t
             JOIN flag_wakes n ON n.chat_session_id = t.session_id AND n.client_turn_id = t.client_turn_id
             WHERE t.id = ?1 AND n.retired_at IS NULL",
        )?;
        let runs = stmt.query_map([turn_id], |row| row.get(0))?;
        Ok(runs.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(super) fn confirm_flag_wake_delivery(&self, turn_id: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE flag_wakes SET delivered_at = ?2, next_delivery_at = NULL
             WHERE delivered_at IS NULL AND retired_at IS NULL
               AND (chat_session_id, client_turn_id) = (
                   SELECT session_id, client_turn_id FROM chat_turns WHERE id = ?1
               )",
            params![turn_id, now],
        )?;
        Ok(())
    }

    /// Released Retry can complete the carrying turn with only its base prompt.
    pub(crate) fn retire_unconfirmed_flag_attachments(
        &self,
        chat: Option<&str>,
        now: i64,
    ) -> Result<BTreeSet<String>> {
        let mut stmt = self.conn.prepare(
            "UPDATE flag_wakes AS w SET retired_at = ?2, delivery_error = ?3
             WHERE w.dedupe GLOB 'resume:*' AND w.delivered_at IS NULL AND w.retired_at IS NULL
               AND (?1 IS NULL OR w.chat_session_id = ?1)
               AND EXISTS (
                   SELECT 1 FROM chat_turns t
                   WHERE t.session_id = w.chat_session_id AND t.client_turn_id = w.client_turn_id
                     AND t.state = 'completed'
               )
             RETURNING run_id",
        )?;
        let runs = stmt.query_map(params![chat, now, FLAG_DELIVERY_UNCONFIRMED], |row| {
            row.get(0)
        })?;
        Ok(runs.collect::<rusqlite::Result<_>>()?)
    }

    /// Runs with a flag no one chose on whose wake ended: retired, or its turn finished or failed.
    pub(crate) fn flag_follow_up_runs(&self, turn: Option<(&str, &str)>) -> Result<Vec<String>> {
        let (chat, client_turn_id) = turn.unzip();
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT n.run_id FROM flag_wakes n
             JOIN runs r ON r.id = n.run_id
             LEFT JOIN chat_turns t ON t.session_id = n.chat_session_id AND t.client_turn_id = n.client_turn_id
             WHERE r.cancel_requested = 0 AND r.cancel_accepted_at IS NULL
               AND ((?1 IS NULL AND ((t.state = 'completed' AND n.delivered_at IS NOT NULL)
                        OR (n.retired_at IS NOT NULL AND n.client_turn_id IS NULL
                            AND r.status IN ('done', 'failed', 'lost')
                            AND COALESCE(r.ended_at, r.updated_at) >= n.created_at)))
                    OR (n.chat_session_id = ?1 AND (n.retired_at IS NOT NULL
                        OR t.state = 'interrupted' OR (t.state = 'failed'
                            AND n.delivered_at IS NOT NULL))
                        AND NOT EXISTS (SELECT 1 FROM run_notices result
                            WHERE result.chat_session_id = ?1 AND (result.dedupe = 'chat-done' AND result.run_id = n.run_id
                                OR result.client_turn_id = ?2
                                OR (result.run_id = n.run_id AND result.kind = 'terminal'
                                    AND result.delivered_at IS NULL AND result.retired_at IS NULL)))))
               AND NOT EXISTS (
                   SELECT 1 FROM run_monitor_events e WHERE e.run_id = n.run_id AND e.kind = 'choice'
                     AND json_extract(e.body_json, '$.flagId') = n.flag_id)",
        )?;
        let runs = stmt.query_map(params![chat, client_turn_id], |row| row.get(0))?;
        Ok(runs.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub(crate) fn queue_flag_follow_up(
        &self,
        run: &str,
        latest: &FlagNotice,
        dedupe: &str,
        body: &str,
        now: i64,
    ) -> Result<bool> {
        let tx = self.begin_immediate()?;
        let available: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs r JOIN chat_sessions s ON s.id = ?3 WHERE r.id = ?1
                AND r.cancel_requested = 0 AND r.cancel_accepted_at IS NULL
                AND s.archived = 0 AND s.autonomy IS NOT 'copilot'
                AND ?4 = (SELECT n.id FROM flag_wakes n WHERE n.run_id = r.id
                    AND n.chat_session_id = s.id AND n.flag_id = ?2
                    AND (n.dedupe IN ('initial', 'remind', 'result') OR n.dedupe GLOB 'resume:*')
                    ORDER BY n.id DESC LIMIT 1)
                AND NOT EXISTS (SELECT 1 FROM run_notices n WHERE n.run_id = r.id
                    AND n.chat_session_id = s.id AND n.dedupe = 'chat-done')
                AND NOT EXISTS (SELECT 1 FROM run_monitor_events e
                    WHERE e.run_id = r.id AND e.kind = 'choice'
                        AND json_extract(e.body_json, '$.flagId') = ?2))",
            params![run, latest.flag_id, latest.chat_session_id, latest.id],
            |row| row.get(0),
        )?;
        let inserted = available
            && self.insert_flag_wake(
                run,
                &latest.chat_session_id,
                &latest.flag_id,
                dedupe,
                body,
                now,
            )?;
        tx.commit()?;
        Ok(inserted)
    }

    pub(crate) fn insert_flag_wake(
        &self,
        run: &str,
        chat: &str,
        flag: &str,
        dedupe: &str,
        body: &str,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "INSERT INTO flag_wakes (run_id, chat_session_id, flag_id, dedupe, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(run_id, chat_session_id, flag_id, dedupe) DO NOTHING",
            params![run, chat, flag, dedupe, body, now],
        )? == 1)
    }

    /// An attached flag can only be sent as part of its owning turn.
    pub(crate) fn attach_flag_notice(
        &self,
        run: &str,
        chat: &str,
        flag: &str,
        body: &str,
        client_turn_id: &str,
        now: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO flag_wakes (run_id, chat_session_id, flag_id, dedupe, body, created_at,
                                      client_turn_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(run_id, chat_session_id, flag_id, dedupe) DO NOTHING",
            params![
                run,
                chat,
                flag,
                format!("resume:{client_turn_id}"),
                body,
                now,
                client_turn_id
            ],
        )?;
        Ok(())
    }

    pub(crate) fn pending_flag_review_runs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT w.run_id FROM flag_wakes w
             WHERE w.retired_at IS NULL AND (
                 (w.delivered_at IS NULL AND w.dedupe NOT GLOB 'resume:*')
                 OR EXISTS (
                     SELECT 1 FROM chat_turns t
                     WHERE t.session_id = w.chat_session_id AND t.client_turn_id = w.client_turn_id
                       AND t.state IN ('preparing', 'retrying', 'running')
                 )
             )",
        )?;
        let runs = stmt.query_map([], |row| row.get(0))?;
        Ok(runs.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Appends each flag that still applies, with the notices that wake its chats, in one
    /// transaction. Returns what became of each.
    pub(crate) fn raise_flags(&self, flags: &[NewFlag], now: i64) -> Result<Vec<Raise>> {
        let tx = self.begin_immediate()?;
        let raised = self.append_flags(flags, now)?;
        tx.commit()?;
        Ok(raised)
    }

    /// [`Self::raise_flags`] inside a caller's transaction. A flag whose fingerprint the run has
    /// raised before is not raised again; another run of the experiment raises it afresh.
    pub(crate) fn append_flags<'a>(
        &self,
        flags: impl IntoIterator<Item = &'a NewFlag>,
        now: i64,
    ) -> Result<Vec<Raise>> {
        let mut raised = Vec::new();
        for flag in flags {
            let stopped: bool = self.conn.query_row(
                "SELECT status IN ('cancelled', 'stopping') OR cancel_requested = 1 OR cancel_accepted_at IS NOT NULL FROM runs WHERE id = ?1",
                [&flag.run_id],
                |row| row.get(0),
            )?;
            if stopped {
                raised.push(Raise::Stopped);
                continue;
            }
            let events = self.run_flag_events(&flag.run_id)?;
            let open = open_flag(&events).is_some();
            let fingerprint = flag.body["fingerprint"].as_str();
            let repeated = fingerprint.is_some_and(|fingerprint| {
                events
                    .iter()
                    .any(|event| event.kind == "flag" && event.body["fingerprint"] == fingerprint)
            });
            if repeated {
                raised.push(Raise::Repeated);
                continue;
            }
            if flag.only_if_clear && open {
                raised.push(Raise::Waits);
                continue;
            }
            self.conn.execute(
                "INSERT INTO run_monitor_events (run_id, seq, kind, created_at, body_json)
                 VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM run_monitor_events WHERE run_id = ?1),
                         'flag', ?2, ?3)",
                params![flag.run_id, now, flag.body.to_string()],
            )?;
            let id = flag.body["id"].as_str().unwrap_or_default();
            for (chat, body) in &flag.wakes {
                self.insert_flag_wake(&flag.run_id, chat, id, "initial", body, now)?;
            }
            raised.push(Raise::Raised);
        }
        Ok(raised)
    }

    /// Keeps a choice on a run's flag, and on the other flags of its look an Ask takes along,
    /// with the notices each wakes chats with, in one transaction, so they wake as one turn.
    /// The first choice is the one asked for: nothing is kept when another choice settled its
    /// flag first, the run's stop was accepted first, or this request was kept already; a flag
    /// taken along is skipped if settled or stopping. A Stop's `stop` accepts the run's cancel with who
    /// asked in the same transaction, so the choice and the stop's words are one winner's.
    pub(crate) fn record_flag_choices(
        &self,
        choices: &[FlagChoice],
        stop: Option<(&str, &StopContext)>,
        now: i64,
    ) -> Result<Chosen> {
        let mut lock = match (stop, choices.first()) {
            (Some(_), Some(first)) => Some(open_lifecycle_lock_at(
                &self.run_lock_path(CANCEL_LOCK, &first.run_id)?,
            )?),
            _ => None,
        };
        let _guard = lock.as_mut().map(|lock| lock.write()).transpose()?;
        let tx = self.begin_immediate()?;
        for (index, choice) in choices.iter().enumerate() {
            let flag = choice.body["flagId"].as_str().unwrap_or_default();
            let events = self.run_flag_events(&choice.run_id)?;
            let mine = |event: &&FlagEvent| event.kind == "choice" && event.body["flagId"] == flag;
            let request = choice.body["requestId"].as_str();
            if index == 0
                && request.is_some()
                && events
                    .iter()
                    .filter(mine)
                    .any(|event| event.body["requestId"].as_str() == request)
            {
                return Ok(Chosen::Repeated);
            }
            let stopping: bool = self.conn.query_row(
                "SELECT cancel_requested = 1 OR cancel_accepted_at IS NOT NULL FROM runs WHERE id = ?1",
                [&choice.run_id],
                |row| row.get(0),
            )?;
            if stopping
                || events
                    .iter()
                    .filter(mine)
                    .any(|event| settles(event) || choice.body["unsettled"] == true)
            {
                if index == 0 {
                    return Ok(Chosen::Settled);
                }
                continue;
            }
            if let Some((reason, stop)) = stop.filter(|_| index == 0) {
                if self
                    .accept_run_stop_locked(&choice.run_id, Some(reason), Some(stop))?
                    .is_none()
                {
                    return Ok(Chosen::Settled);
                }
            }
            self.conn.execute(
                "INSERT INTO run_monitor_events (run_id, seq, kind, created_at, body_json)
                 VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM run_monitor_events WHERE run_id = ?1),
                         'choice', ?2, ?3)",
                params![choice.run_id, now, choice.body.to_string()],
            )?;
            if matches!(choice.body["c"].as_str(), Some("keep" | "stop")) {
                self.conn.execute(
                    "UPDATE flag_wakes SET retired_at = ?3
                     WHERE run_id = ?1 AND flag_id = ?2
                       AND client_turn_id IS NULL AND delivered_at IS NULL AND retired_at IS NULL",
                    params![choice.run_id, flag, now],
                )?;
            }
            for (chat, body) in &choice.wakes {
                let dedupe = flag_ask_dedupe(request);
                self.insert_flag_wake(&choice.run_id, chat, flag, &dedupe, body, now)?;
            }
        }
        tx.commit()?;
        Ok(Chosen::Recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::git::TemporaryDirectory;
    use crate::store::tests::run_fixture;
    use serde_json::json;

    #[test]
    fn released_drain_cannot_claim_a_flag_reminder() {
        let dir = TemporaryDirectory::new("orx-flag-downgrade").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store.put_chat_session("owner", "claude-code", None);
        let run = run_fixture("run", "running", Some("owner"));
        store.upsert_run(&run).unwrap();
        let flag = crate::engine::flags::new_flag(&store, &run, json!({"id": "f1"}), true).unwrap();
        assert_eq!(store.raise_flags(&[flag], 1).unwrap(), [Raise::Raised]);
        let initial = store.run_notice_batch_at("owner", 2).unwrap();
        let turn = &initial[0].client_turn_id;
        store.put_chat_turn("owner", turn, "completed", 2);
        store
            .mark_run_notice_batch_delivered("owner", turn, 2)
            .unwrap();
        crate::engine::flags::follow_up_flags(&store, 3).unwrap();

        let old = Connection::open(dir.path().join("orx.db")).unwrap();
        let due = |now| {
            old.prepare(include_str!(
                "../../tests/contracts/store-0.2.28-due-notices.sql"
            ))
            .unwrap()
            .query_map([now], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
        };
        assert!(due(3).is_empty());
        assert_eq!(store.pending_run_notice_sessions().unwrap(), ["owner"]);

        store
            .insert_run_notice(
                "other",
                "owner",
                TERMINAL,
                TERMINAL,
                "Other run finished",
                4,
            )
            .unwrap();
        assert_eq!(due(4), ["owner"]);
        assert_eq!(
            old.execute(
                "UPDATE run_notices SET client_turn_id = ?2 WHERE chat_session_id = ?1
             AND delivered_at IS NULL AND retired_at IS NULL AND client_turn_id IS NULL",
                params!["owner", "run-notice:older"],
            )
            .unwrap(),
            1
        );
        assert_eq!(
            old.query_row(
                "SELECT body FROM run_notices WHERE chat_session_id = ?1 AND client_turn_id = ?2
             AND delivered_at IS NULL ORDER BY id",
                params!["owner", "run-notice:older"],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "Other run finished"
        );
        old.execute(
            "UPDATE run_notices SET delivered_at = ?3, next_delivery_at = NULL WHERE chat_session_id = ?1
             AND client_turn_id = ?2 AND delivered_at IS NULL",
            params!["owner", "run-notice:older", 5],
        ).unwrap();
        assert!(due(5).is_empty());

        let reminder = store.run_notice_batch_at("owner", 6).unwrap();
        assert_eq!(reminder.len(), 1);
        assert!(reminder[0].body.contains("orx exp flag run --flag f1"));
        assert!(reminder[0].body.contains("ended without settling flag"));
        assert!(due(6).is_empty());
        store
            .mark_run_notice_batch_delivered("owner", &reminder[0].client_turn_id, 6)
            .unwrap();
        assert!(store.run_notice_batch_at("owner", 7).unwrap().is_empty());
    }

    #[test]
    fn released_result_resumes_with_its_original_turn_before_flag_wakes() {
        let dir = TemporaryDirectory::new("orx-notice-upgrade").unwrap();
        let old = Connection::open(dir.path().join("orx.db")).unwrap();
        old.execute_batch(include_str!(
            "../../tests/contracts/store-0.2.28-notices.sql"
        ))
        .unwrap();
        drop(old);

        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store.put_chat_session("owner", "claude-code", None);
        let mut run = run_fixture("released-run", "done", Some("owner"));
        run.ended_at = Some(1);
        store.upsert_run(&run).unwrap();
        let flag = crate::engine::flags::new_flag(&store, &run, json!({"id": "f1"}), true).unwrap();
        store.raise_flags(&[flag], 3).unwrap();

        assert!(store
            .run_notice_batch_at("owner", 60_001)
            .unwrap()
            .is_empty());
        let result = store.run_notice_batch_at("owner", 60_002).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].body, "Run finished");
        assert_eq!(
            result[0].client_turn_id,
            "run-notice:00000000-0000-4000-8000-000000000028"
        );
        store
            .mark_run_notice_batch_delivered("owner", &result[0].client_turn_id, 60_003)
            .unwrap();

        let wake = store.run_notice_batch_at("owner", 60_004).unwrap();
        assert_eq!(wake.len(), 1);
        assert!(wake[0].body.contains("orx exp flag released-run --flag f1"));
        assert_ne!(wake[0].client_turn_id, result[0].client_turn_id);
    }

    #[test]
    fn released_batch_keeps_partially_retired_members_and_resumes_by_id() {
        let dir = TemporaryDirectory::new("orx-frozen-results").unwrap();
        let old = Connection::open(dir.path().join("orx.db")).unwrap();
        old.execute_batch(include_str!(
            "../../tests/contracts/store-0.2.28-notices.sql"
        ))
        .unwrap();
        old.execute_batch(include_str!(
            "../../tests/contracts/store-0.2.28-frozen-batch.sql"
        ))
        .unwrap();
        drop(old);

        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store
            .dismiss_run_notices("discarded", "partial", 12)
            .unwrap();
        let batch = store.run_notice_batch_at("partial", 13).unwrap();
        assert_eq!(
            batch
                .iter()
                .map(|row| row.body.as_str())
                .collect::<Vec<_>>(),
            ["Discarded run finished", "Surviving run finished"]
        );
        assert!(batch
            .iter()
            .all(|row| row.client_turn_id == "run-notice:frozen"));
        store
            .mark_run_notice_batch_delivered("partial", "run-notice:frozen", 13)
            .unwrap();
        let next = store.run_notice_batch_at("partial", 14).unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].client_turn_id, "run-notice:later-id");
    }

    #[test]
    fn admission_withdraws_claimed_automatic_wakes_when_their_basis_changes() {
        for change in [
            "copilot",
            "done",
            "dismiss",
            "keep",
            "delete-chat",
            "delete-run",
            "result",
        ] {
            let dir = TemporaryDirectory::new("orx-flag-admission").unwrap();
            let store = Store::open_at(dir.path().to_path_buf()).unwrap();
            store.put_chat_session("owner", "claude-code", None);
            let mut run = run_fixture("run", "running", Some("owner"));
            store.upsert_run(&run).unwrap();
            let flag =
                crate::engine::flags::new_flag(&store, &run, json!({"id": "f1"}), true).unwrap();
            store.raise_flags(&[flag], 1).unwrap();
            let batch = store.run_notice_batch_at("owner", 2).unwrap();
            assert_eq!(batch.len(), 1);
            let claim = &batch[0].client_turn_id;

            match change {
                "copilot" => store
                    .set_chat_session_autonomy("owner", Autonomy::Copilot)
                    .unwrap(),
                "done" => store.set_chat_session_archived("owner", true).unwrap(),
                "dismiss" => store.dismiss_run_notices("run", "owner", 3).unwrap(),
                "keep" => {
                    let choice = FlagChoice {
                        run_id: "run".into(),
                        body: json!({"flagId": "f1", "c": "keep", "by": "user"}),
                        wakes: Vec::new(),
                    };
                    assert_eq!(
                        store.record_flag_choices(&[choice], None, 3).unwrap(),
                        Chosen::Recorded
                    );
                }
                "delete-chat" => store.delete_chat_session("owner").unwrap(),
                "delete-run" => {
                    store
                        .conn
                        .execute("DELETE FROM runs WHERE id = 'run'", [])
                        .unwrap();
                }
                "result" => {
                    run.status = "done".into();
                    run.ended_at = Some(3);
                    store.upsert_run(&run).unwrap();
                    store.conn.execute(
                        "INSERT INTO run_notices (run_id, chat_session_id, kind, dedupe, body, created_at, delivered_at)
                         VALUES ('run', 'owner', 'terminal', 'terminal', 'Finished', 3, 4)", [],
                    ).unwrap();
                }
                _ => unreachable!(),
            }
            let tx = store.begin_immediate().unwrap();
            assert!(
                store
                    .admit_flag_wakes("owner", claim, 5)
                    .unwrap()
                    .is_empty(),
                "{change}"
            );
            tx.commit().unwrap();
            assert_eq!(store.run_notice_error("run").unwrap(), None);
        }
    }

    #[test]
    fn an_ask_claimed_before_the_run_ends_is_rewritten_after_its_result() {
        let dir = TemporaryDirectory::new("orx-ended-ask").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store.put_chat_session("owner", "claude-code", None);
        let mut run = run_fixture("run", "running", Some("owner"));
        store.upsert_run(&run).unwrap();
        let body = json!({"id": "f1", "text": "Review the loss"});
        let flag = crate::engine::flags::new_flag(&store, &run, body.clone(), true).unwrap();
        store.raise_flags(&[flag], 1).unwrap();
        let ask = FlagChoice {
            run_id: run.id.clone(),
            body: json!({"flagId": "f1", "c": "ask", "by": "user", "requestId": "ask-1"}),
            wakes: vec![(
                "owner".into(),
                crate::engine::flags::wake(&run, &body, true),
            )],
        };
        assert_eq!(
            store.record_flag_choices(&[ask], None, 2).unwrap(),
            Chosen::Recorded
        );
        let batch = store.run_notice_batch_at("owner", 3).unwrap();
        assert_eq!(batch.len(), 1);
        assert!(batch[0].body.contains("--stop"));
        store
            .set_chat_session_autonomy("owner", Autonomy::Copilot)
            .unwrap();

        store.watch_legacy_run("run", Some(("running", 1))).unwrap();
        run.status = "done".into();
        run.ended_at = Some(1);
        store.upsert_run(&run).unwrap();
        let tx = store.begin_immediate().unwrap();
        assert!(store
            .admit_flag_wakes("owner", &batch[0].client_turn_id, 5)
            .unwrap()
            .is_empty());
        tx.commit().unwrap();
        assert!(store.run_notice_batch_at("owner", 5).unwrap().is_empty());

        let tx = store.begin_immediate().unwrap();
        store
            .insert_run_notice("run", "owner", TERMINAL, TERMINAL, "Finished", 5)
            .unwrap();
        store.unwatch_legacy_run("run").unwrap();
        tx.commit().unwrap();
        let result = store.run_notice_batch_at("owner", 5).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].body, "Finished");
        store
            .mark_run_notice_batch_delivered("owner", &result[0].client_turn_id, 5)
            .unwrap();
        let wake = store.run_notice_batch_at("owner", 6).unwrap();
        assert_eq!(wake.len(), 1);
        assert_eq!(wake[0].id, batch[0].id);
        assert_eq!(wake[0].dedupe, "ask:ask-1");
        let tx = store.begin_immediate().unwrap();
        let bodies = store
            .admit_flag_wakes("owner", &wake[0].client_turn_id, 6)
            .unwrap();
        assert_eq!(bodies, [crate::engine::flags::wake(&run, &body, true)]);
        assert!(bodies[0].contains("has ended"));
        assert!(!bodies[0].contains("--stop"));
        tx.commit().unwrap();
    }

    #[test]
    fn a_follow_up_cannot_queue_after_the_latest_wake_was_carried_into_a_user_turn() {
        let dir = TemporaryDirectory::new("orx-flag-follow-up").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store.put_chat_session("owner", "claude-code", None);
        let mut run = run_fixture("run", "running", Some("owner"));
        store.upsert_run(&run).unwrap();
        let flag = crate::engine::flags::new_flag(&store, &run, json!({"id": "f1"}), true).unwrap();
        store.raise_flags(&[flag], 1).unwrap();

        run.status = "done".into();
        run.ended_at = Some(2);
        store.upsert_run(&run).unwrap();
        store.conn.execute(
            "INSERT INTO run_notices (run_id, chat_session_id, kind, dedupe, body, created_at, delivered_at)
             VALUES ('run', 'owner', 'terminal', 'terminal', 'Finished', 2, 3)", [],
        ).unwrap();
        store.retire_unadmitted_flag_wakes(3).unwrap();
        let previous = store.run_flag_notices("run").unwrap().pop().unwrap();
        assert!(previous.retired);

        let chat = store.get_chat_session("owner").unwrap().unwrap();
        let tx = store.begin_immediate().unwrap();
        let carried = crate::engine::flags::carry_cut_flags(&store, &chat, "user-turn", 4).unwrap();
        assert_eq!(carried.len(), 1);
        tx.commit().unwrap();
        assert!(!store
            .queue_flag_follow_up("run", &previous, "result", "Duplicate wake", 4)
            .unwrap());
        let wakes = store.run_flag_notices("run").unwrap();
        assert_eq!(wakes.len(), 2);
        assert_eq!(wakes[1].dedupe, "resume:user-turn");
    }

    #[test]
    fn a_settled_flag_withdraws_a_queued_ask() {
        let dir = TemporaryDirectory::new("orx-flag-ask-retirement").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store.put_chat_session("owner", "claude-code", Some("copilot"));
        let run = run_fixture("run", "running", Some("owner"));
        store.upsert_run(&run).unwrap();
        let flag = crate::engine::flags::new_flag(&store, &run, json!({"id": "f1"}), true).unwrap();
        store.raise_flags(&[flag], 1).unwrap();

        let ask = FlagChoice {
            run_id: run.id.clone(),
            body: json!({"flagId": "f1", "c": "ask", "by": "user", "requestId": "ask-1"}),
            wakes: vec![("owner".into(), "Review the flag".into())],
        };
        assert_eq!(
            store.record_flag_choices(&[ask], None, 2).unwrap(),
            Chosen::Recorded
        );
        assert_eq!(store.pending_run_notice_sessions().unwrap(), ["owner"]);

        let keep = FlagChoice {
            run_id: run.id.clone(),
            body: json!({"flagId": "f1", "c": "keep", "by": "user"}),
            wakes: Vec::new(),
        };
        assert_eq!(
            store.record_flag_choices(&[keep], None, 3).unwrap(),
            Chosen::Recorded
        );
        assert!(store.run_notice_batch_at("owner", 4).unwrap().is_empty());
        assert!(store.run_flag_notices(&run.id).unwrap()[0].retired);
        assert_eq!(store.run_notice_error(&run.id).unwrap(), None);
    }

    #[test]
    fn a_flag_superseded_by_the_result_is_not_a_delivery_error() {
        let dir = TemporaryDirectory::new("orx-flag-retirement").unwrap();
        let store = Store::open_at(dir.path().to_path_buf()).unwrap();
        store.put_chat_session("owner", "claude-code", None);
        let mut run = run_fixture("run", "running", Some("owner"));
        store.upsert_run(&run).unwrap();
        let flag = NewFlag {
            run_id: run.id.clone(),
            body: json!({"id": "f1"}),
            wakes: vec![("owner".into(), "Review the flag".into())],
            noted: Vec::new(),
            only_if_clear: true,
        };
        assert_eq!(store.raise_flags(&[flag], 2).unwrap(), [Raise::Raised]);

        run.status = "done".into();
        run.ended_at = Some(3);
        run.updated_at = 3;
        store.upsert_run(&run).unwrap();
        store
            .insert_run_notice(&run.id, "owner", TERMINAL, TERMINAL, "Finished", 3)
            .unwrap();
        let chat = store.get_chat_session("owner").unwrap().unwrap();
        store.retire_unadmitted_flag_wakes(4).unwrap();
        crate::engine::flags::follow_up_flags(&store, 4).unwrap();
        assert!(
            crate::engine::flags::carry_cut_flags(&store, &chat, "unrelated", 4)
                .unwrap()
                .is_empty()
        );
        let batch = store.run_notice_batch_at("owner", 4).unwrap();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].kind, TERMINAL);
        let turn = &batch[0].client_turn_id;
        let carried = crate::engine::flags::carry_cut_flags(&store, &chat, turn, 4).unwrap();
        assert!(carried.is_empty());
        store.put_chat_turn("owner", turn, "running", 4);
        store
            .update_chat_turn_progress(turn, "running", "accepted", 4, None)
            .unwrap();
        store
            .mark_run_notice_batch_delivered("owner", turn, 4)
            .unwrap();
        let notices = store.run_flag_notices(&run.id).unwrap();
        assert!(notices[0].retired);
        assert_eq!(store.run_notice_error(&run.id).unwrap(), None);
        let wake = store.run_notice_batch_at("owner", 5).unwrap();
        assert_eq!(wake.len(), 1);
        assert!(wake[0].body.contains("has ended"));
        assert_ne!(wake[0].client_turn_id, *turn);

        let ask = FlagChoice {
            run_id: run.id.clone(),
            body: json!({"flagId": "f1", "c": "ask", "by": "user", "requestId": "result"}),
            wakes: vec![("owner".into(), "Asked to review".into())],
        };
        assert_eq!(
            store.record_flag_choices(&[ask], None, 6).unwrap(),
            Chosen::Recorded
        );
        store
            .mark_run_notice_batch_delivered("owner", &wake[0].client_turn_id, 7)
            .unwrap();
        let asked = store.run_notice_batch_at("owner", 8).unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].body, "Asked to review");
    }
}
