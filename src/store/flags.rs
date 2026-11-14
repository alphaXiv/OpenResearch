use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

pub const WATCH_FLAG: &str = "watch_flag";

/// A flag to append to a run's journal, with the notice each chat it wakes is sent.
pub(crate) struct NewFlag {
    pub run_id: String,
    /// The flag as the journal keeps it: id, source, fingerprint, sentence, evidence.
    pub body: Value,
    /// Chat, then the frozen wake text.
    pub wakes: Vec<(String, String)>,
    /// Raised only while the run has no flag still open, so one trouble wakes once.
    pub only_if_clear: bool,
}

/// What became of a flag asked to be raised.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Raise {
    Raised,
    /// The run raised this fingerprint before, or the experiment kept going after it.
    Repeated,
    /// The run has a flag still open.
    Waits,
}

/// What a run's journal holds of one flag or choice.
#[derive(Debug, Clone)]
pub(crate) struct FlagEvent {
    pub kind: String,
    pub at: i64,
    pub body: Value,
}

/// How far a notice that wakes a chat with a flag got.
#[derive(Debug, Clone)]
pub(crate) struct FlagNotice {
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

impl Store {
    /// Withdraws each flag's wake not yet claimed whose run ended after it was raised: the run's
    /// end reaches the chat as its own notice, and the wake would ask to stop what has stopped.
    pub(crate) fn retire_stale_flag_wakes(&self, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE run_notices SET retired_at = ?1, next_delivery_at = NULL,
                    delivery_error = 'The run ended before its flag reached the chat'
             WHERE kind = ?2 AND delivered_at IS NULL AND retired_at IS NULL AND client_turn_id IS NULL
               AND EXISTS(SELECT 1 FROM runs r WHERE r.id = run_notices.run_id
                          AND r.ended_at IS NOT NULL AND r.ended_at >= run_notices.created_at)",
            params![now, WATCH_FLAG],
        )?;
        Ok(())
    }

    /// The run's flags and the choices made on them, oldest first.
    pub(crate) fn run_flag_events(&self, run_id: &str) -> Result<Vec<FlagEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT seq, kind, created_at, body_json FROM run_monitor_events
             WHERE run_id = ?1 AND kind IN ('flag', 'choice') ORDER BY seq",
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
            events.push(FlagEvent { kind, at, body });
        }
        Ok(events)
    }

    /// The notices that woke, or will wake, chats with the run's flags.
    pub(crate) fn run_flag_notices(&self, run_id: &str) -> Result<Vec<FlagNotice>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.chat_session_id, n.dedupe, n.client_turn_id IS NOT NULL, n.delivered_at IS NOT NULL,
                    n.retired_at IS NOT NULL, t.state
             FROM run_notices n
             LEFT JOIN chat_turns t ON t.session_id = n.chat_session_id AND t.client_turn_id = n.client_turn_id
             WHERE n.run_id = ?1 AND n.kind = ?2 ORDER BY n.id",
        )?;
        let rows = stmt.query_map(params![run_id, WATCH_FLAG], |row| {
            Ok(FlagNotice {
                chat_session_id: row.get(0)?,
                dedupe: row.get(1)?,
                claimed: row.get(2)?,
                delivered: row.get(3)?,
                retired: row.get(4)?,
                turn: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// The flags and choices of every run of the experiment, by run.
    pub(crate) fn experiment_flag_events(
        &self,
        experiment_id: &str,
    ) -> Result<Vec<(String, FlagEvent)>> {
        let runs: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM runs WHERE experiment_id = ?1 ORDER BY created_at")?;
            let rows = stmt.query_map([experiment_id], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut events = Vec::new();
        for run in runs {
            for event in self.run_flag_events(&run)? {
                events.push((run.clone(), event));
            }
        }
        Ok(events)
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
    /// raised before, or someone chose to keep going after on any run of its experiment, is not
    /// raised again.
    pub(crate) fn append_flags(&self, flags: &[NewFlag], now: i64) -> Result<Vec<Raise>> {
        let mut raised = Vec::new();
        for flag in flags {
            let events = self.run_flag_events(&flag.run_id)?;
            let settled: BTreeSet<&str> = events
                .iter()
                .filter(|event| settles(event))
                .filter_map(|event| event.body["flagId"].as_str())
                .collect();
            let open = events.iter().any(|event| {
                event.kind == "flag"
                    && !event.body["id"]
                        .as_str()
                        .is_some_and(|id| settled.contains(id))
            });
            let fingerprint = flag.body["fingerprint"].as_str();
            let repeated = match fingerprint {
                Some(fingerprint) => {
                    events.iter().any(|event| {
                        event.kind == "flag" && event.body["fingerprint"] == fingerprint
                    }) || self.kept_fingerprints(&flag.run_id)?.contains(fingerprint)
                }
                None => false,
            };
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
                self.insert_run_notice(
                    &flag.run_id,
                    chat,
                    WATCH_FLAG,
                    &format!("{WATCH_FLAG}:{id}"),
                    body,
                    now,
                )?;
            }
            raised.push(Raise::Raised);
        }
        Ok(raised)
    }

    /// The fingerprints of flags someone chose to keep going after, on any run of `run_id`'s
    /// experiment.
    fn kept_fingerprints(&self, run_id: &str) -> Result<BTreeSet<String>> {
        let Some(run) = self.get_run(run_id)? else {
            return Ok(BTreeSet::new());
        };
        let events = self.experiment_flag_events(&run.experiment_id)?;
        let kept: BTreeSet<(&str, &str)> = events
            .iter()
            .filter(|(_, event)| event.kind == "choice" && event.body["c"] == "keep")
            .filter_map(|(run, event)| Some((run.as_str(), event.body["flagId"].as_str()?)))
            .collect();
        Ok(events
            .iter()
            .filter(|(run, event)| {
                event.kind == "flag"
                    && event.body["id"]
                        .as_str()
                        .is_some_and(|id| kept.contains(&(run.as_str(), id)))
            })
            .filter_map(|(_, event)| event.body["fingerprint"].as_str().map(str::to_owned))
            .collect())
    }

    /// Keeps a choice on a run's flag, and on the other flags of its look an Ask takes along,
    /// with the notices each wakes chats with, in one transaction, so they wake as one turn.
    /// The first choice is the one asked for: nothing is kept when another choice settled its
    /// flag first, or this request was kept already; a flag taken along that is settled is left.
    pub(crate) fn record_flag_choices(&self, choices: &[FlagChoice], now: i64) -> Result<Chosen> {
        let tx = self.begin_immediate()?;
        for (index, choice) in choices.iter().enumerate() {
            let flag = choice.body["flagId"].as_str().unwrap_or_default();
            let events = self.run_flag_events(&choice.run_id)?;
            let mine = |event: &&FlagEvent| event.kind == "choice" && event.body["flagId"] == flag;
            let request = choice.body["requestId"].as_str();
            if index == 0 {
                if request.is_some()
                    && events
                        .iter()
                        .filter(mine)
                        .any(|event| event.body["requestId"].as_str() == request)
                {
                    return Ok(Chosen::Repeated);
                }
                if events.iter().filter(mine).any(settles) {
                    return Ok(Chosen::Settled);
                }
            } else if events.iter().filter(mine).any(settles) {
                continue;
            }
            self.conn.execute(
                "INSERT INTO run_monitor_events (run_id, seq, kind, created_at, body_json)
                 VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM run_monitor_events WHERE run_id = ?1),
                         'choice', ?2, ?3)",
                params![choice.run_id, now, choice.body.to_string()],
            )?;
            for (chat, body) in &choice.wakes {
                let dedupe = format!("{WATCH_FLAG}:{flag}:{}", request.unwrap_or("ask"));
                self.insert_run_notice(&choice.run_id, chat, WATCH_FLAG, &dedupe, body, now)?;
            }
        }
        tx.commit()?;
        Ok(Chosen::Recorded)
    }
}
