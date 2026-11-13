use std::mem::discriminant;
use std::time::Duration;

use super::bus::RunEvent;
use super::{load, phase_of, CancelOutcome, Engine, Handle, Observation, Phase};
use crate::error::Result;
use crate::store::{Contact, EngineRunState, StoredRun, ATTENTION, RECOVERED, TERMINAL};

const OUTAGE_ATTENTION: Duration = Duration::from_secs(10 * 60);

pub(crate) const ABANDONED: &str =
    "Abandoned; the cancel stays pending until the job is confirmed gone.";

const WAITING_REASON_CHARS: usize = 500;

/// What one fact does to a run. The phase applies only if it moves forward.
#[derive(Debug, Default)]
pub(super) struct Change {
    pub phase: Option<Phase>,
    /// Records the launched job; the only way out of Submitting other than a terminal phase.
    pub handle: Option<(u32, Handle)>,
    pub exit_code: Option<i64>,
    pub finished_at: Option<i64>,
    pub started_at: Option<i64>,
    pub reason: Option<String>,
    pub cancel_unconfirmed: Option<Option<Vec<u32>>>,
    /// The backend's latest word on why the run waits; `Some(None)` means it gave none.
    pub waiting: Option<Option<String>>,
    pub contact: Option<Contact>,
    /// Raise the outage's attention notice even though the contact did not change.
    pub attention: bool,
}

impl Change {
    pub fn contact(contact: Contact) -> Self {
        Self {
            contact: Some(contact),
            ..Self::default()
        }
    }

    pub fn terminal(phase: Phase, reason: Option<String>) -> Self {
        Self {
            phase: Some(phase),
            reason,
            contact: Some(Contact::Live),
            ..Self::default()
        }
    }
}

/// A terminal run with non-Live contact keeps its kill pending.
pub(crate) fn is_tombstone(phase: Phase, contact: &Contact) -> bool {
    phase.is_terminal() && *contact != Contact::Live
}

fn observed(phase: Phase, contact: &Contact, observation: Observation, now: i64) -> Change {
    use Observation::*;

    if let CancelReceipt {
        finished_at,
        code,
        remaining,
        note,
    } = observation
    {
        if phase == Phase::Submitting || (phase.is_terminal() && !is_tombstone(phase, contact)) {
            return Change::default();
        }
        let contact = if remaining == 0 {
            Contact::Live
        } else {
            Contact::Stopping {
                since: now,
                count: remaining,
            }
        };
        return Change {
            phase: Some(Phase::Cancelled),
            finished_at,
            exit_code: code.map(i64::from),
            reason: note.as_ref().map(|unreadable| {
                if unreadable.is_empty() {
                    crate::plane::LABELS["run_notice_cancel_uncovered"].clone()
                } else {
                    let pids = unreadable
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    crate::plane::LABELS["run_notice_cancel_unreadable"].replace("{pids}", &pids)
                }
            }),
            cancel_unconfirmed: Some(note),
            contact: Some(contact),
            ..Change::default()
        };
    }
    if phase.is_terminal() {
        if !is_tombstone(phase, contact) {
            return Change::default();
        }
        let finished_at = match &observation {
            Exited { finished_at, .. }
            | Cancelled { finished_at, .. }
            | Failed { finished_at, .. } => *finished_at,
            _ => None,
        };
        let mut change = match observation {
            Cancelled { .. } | Gone { .. } | NotFound => Change::contact(Contact::Live),
            _ if phase == Phase::Cancelled => Change::default(),
            Unreachable { cause } => Change::contact(Contact::Unreachable { since: now, cause }),
            NeedsAuth { cause } => Change::contact(Contact::NeedsAuth { since: now, cause }),
            Queued { .. }
            | Running { .. }
            | Exited { .. }
            | Failed { .. }
            | Lost { .. }
            | Unreconciled { .. } => Change::contact(Contact::Unreconciled {
                since: now,
                detail: ABANDONED.into(),
            }),
            Notify { .. } | Log { .. } | Lingering { .. } | CancelReceipt { .. } => {
                Change::default()
            }
        };
        change.finished_at = finished_at;
        return change;
    }
    // Until it has a handle, a run moves only through the submission protocol.
    if phase == Phase::Submitting {
        return Change::default();
    }
    let cancelled = matches!(observation, Cancelled { .. });
    let progressed = |to| Change {
        phase: Some(to),
        contact: Some(Contact::Live),
        ..Change::default()
    };
    match observation {
        Queued { detail } => Change {
            waiting: Some(detail.as_deref().and_then(waiting_reason)),
            ..progressed(Phase::Starting)
        },
        Running { started_at } => Change {
            started_at: Some(started_at.unwrap_or(now)),
            ..progressed(Phase::Running)
        },
        Exited {
            code,
            signal,
            finished_at,
        }
        | Cancelled {
            code,
            signal,
            finished_at,
        } => Change {
            finished_at,
            exit_code: code.map(i64::from),
            ..Change::terminal(
                if cancelled {
                    Phase::Cancelled
                } else if code == Some(0) && signal.is_none() {
                    Phase::Done
                } else {
                    Phase::Failed
                },
                signal.map(|signal| format!("Killed by signal {signal}.")),
            )
        },
        Failed {
            reason,
            finished_at,
        } => Change {
            finished_at,
            ..Change::terminal(Phase::Failed, Some(reason))
        },
        Lost { reason } | Gone { reason } => Change::terminal(Phase::Lost, Some(reason)),
        Unreachable { cause } => Change::contact(Contact::Unreachable { since: now, cause }),
        NeedsAuth { cause } => Change::contact(Contact::NeedsAuth { since: now, cause }),
        Unreconciled { detail } => Change::contact(Contact::Unreconciled { since: now, detail }),
        NotFound => Change::contact(Contact::Unreconciled {
            since: now,
            detail: "The backend reports no such job.".into(),
        }),
        Notify { .. } | Log { .. } | Lingering { .. } | CancelReceipt { .. } => Change::default(),
    }
}

/// Provider text on one line, short enough for a status.
fn waiting_reason(detail: &str) -> Option<String> {
    let line = detail.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= WAITING_REASON_CHARS {
        return Some(line).filter(|line| !line.is_empty());
    }
    let mut short: String = line.chars().take(WAITING_REASON_CHARS - 1).collect();
    short.push('…');
    Some(short)
}

/// A continuing outage keeps its start; its cause or detail follows the latest report.
fn merged(old: &Contact, new: Contact) -> Contact {
    match (old, new) {
        (Contact::Unreachable { since, .. }, Contact::Unreachable { cause, .. }) => {
            Contact::Unreachable {
                since: *since,
                cause,
            }
        }
        (Contact::NeedsAuth { since, .. }, Contact::NeedsAuth { cause, .. }) => {
            Contact::NeedsAuth {
                since: *since,
                cause,
            }
        }
        (Contact::Unreconciled { since, .. }, Contact::Unreconciled { detail, .. }) => {
            Contact::Unreconciled {
                since: *since,
                detail,
            }
        }
        (Contact::Stopping { since, .. }, Contact::Stopping { count, .. }) => Contact::Stopping {
            since: *since,
            count,
        },
        (_, new) => new,
    }
}

fn attention_body(run_id: &str, contact: &Contact) -> String {
    match contact {
        Contact::Unreachable { since, cause } => format!(
            "[orx] Run `{run_id}` has been unreachable since {} ({cause}). Its status is unchanged and orx keeps retrying.",
            crate::telemetry::iso8601_utc(*since)
        ),
        Contact::NeedsAuth { cause, .. } => format!(
            "[orx] Run `{run_id}` needs authentication ({cause}). Its status is unchanged."
        ),
        Contact::Unreconciled { detail, .. } => {
            format!("[orx] Run `{run_id}` needs attention: {detail}")
        }
        Contact::Stopping { .. } | Contact::Live => unreachable!("terminal contact is not an outage"),
    }
}

impl Engine {
    pub fn observe(&self, run_id: &str, observation: Observation) -> Result<()> {
        if let Observation::Notify { len } = observation {
            self.memory().notify.insert(run_id.into(), len);
            self.bus.publish(RunEvent::Grew {
                file: super::LogFile::Notify,
                run_id: run_id.into(),
                len,
            });
            return Ok(());
        }
        if let Observation::Log { len } = observation {
            if self.memory().logs.insert(run_id.into(), len) != Some(len) {
                self.bus.publish(RunEvent::Grew {
                    file: super::LogFile::Log,
                    run_id: run_id.into(),
                    len,
                });
            }
            return Ok(());
        }
        if let Observation::Lingering { pids } = observation {
            if !self.own(run_id)? {
                return Ok(());
            }
            if pids.is_empty() {
                return self.commit(run_id, |phase, state| {
                    if is_tombstone(phase, &state.contact) {
                        Change::contact(Contact::Live)
                    } else {
                        Change::default()
                    }
                });
            }
            let body = format!("[orx] Run `{run_id}` finished with lingering processes {pids:?}. Cleanup can stop them.");
            return self.attention(run_id, "lingering", &body);
        }
        if !self.own(run_id)? {
            return Ok(());
        }
        #[cfg(test)]
        {
            let hook = self.memory().before_observed_commit.take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let hosted = match super::handle_of(&load(&self.store(), run_id)?.0) {
            Some(Handle::Api { .. }) => true,
            Some(Handle::Legacy { descriptor }) => {
                super::adapters::api::provider(&descriptor.kind).is_ok()
            }
            _ => false,
        };
        let now = self.clock.wall_ms();
        // Decided under the store lock that cancel registration also takes.
        self.commit(run_id, |phase, state| {
            let verifying = self
                .memory()
                .verifying
                .get(run_id)
                .is_some_and(|cancel| cancel.strong_count() > 0);
            if verifying && observation.ends_run() {
                return Change::default();
            }
            let observation = match observation {
                Observation::Exited {
                    finished_at,
                    code,
                    signal,
                } if hosted
                    && matches!(phase, Phase::Starting | Phase::Running)
                    && state.cancel_accepted_at.is_some()
                    && (code.is_some_and(|code| code != 0) || signal.is_some()) =>
                {
                    Observation::Cancelled {
                        finished_at,
                        code,
                        signal,
                    }
                }
                Observation::Gone { .. } if state.cancel_accepted_at.is_some() => {
                    Observation::Cancelled {
                        finished_at: None,
                        code: None,
                        signal: None,
                    }
                }
                observation => observation,
            };
            if phase == Phase::Cancelled
                && matches!(
                    observation,
                    Observation::Unreachable { .. } | Observation::NeedsAuth { .. }
                )
            {
                self.memory().cancels_sent.remove(run_id);
            }
            observed(phase, &state.contact, observation, now)
        })
    }

    /// A live run whose adapter cannot be built is surfaced, never skipped.
    pub fn unreconciled(&self, run_id: &str, error: &crate::error::Error) -> Result<()> {
        if !self.own(run_id)? {
            return Ok(());
        }
        let since = self.clock.wall_ms();
        let detail = format!("{error:#}");
        self.commit(run_id, |phase, _| {
            if phase.is_terminal() {
                return Change::default();
            }
            Change::contact(Contact::Unreconciled { since, detail })
        })
    }

    /// One attention notice per subscribed chat and `dedupe` key.
    pub fn attention(&self, run_id: &str, dedupe: &str, body: &str) -> Result<()> {
        let chats = {
            let store = self.store();
            let tx = store.begin_immediate()?;
            let mut chats = Vec::new();
            for chat in store.run_subscribers(run_id)? {
                if store.insert_run_notice(
                    run_id,
                    &chat,
                    ATTENTION,
                    dedupe,
                    body,
                    self.clock.wall_ms(),
                )? {
                    chats.push(chat);
                }
            }
            tx.commit()?;
            chats
        };
        if !chats.is_empty() {
            self.bus.publish(RunEvent::Notices { chats });
        }
        Ok(())
    }

    pub(super) fn apply_cancel(&self, run: &StoredRun, outcome: CancelOutcome) -> Result<()> {
        let now = self.clock.wall_ms();
        let legacy_note = super::legacy::cancel_note(run);
        self.commit(&run.id, |phase, _| match outcome {
            CancelOutcome::Verified if phase.is_terminal() => Change::contact(Contact::Live),
            CancelOutcome::Verified => Change::terminal(Phase::Cancelled, legacy_note),
            CancelOutcome::Survivors { count } => Change {
                contact: Some(Contact::Stopping { since: now, count }),
                ..Change::terminal(Phase::Cancelled, legacy_note)
            },
            CancelOutcome::Undelivered { detail } => {
                eprintln!("Cancel pending for {}: {detail}", run.id);
                Change::default()
            }
            CancelOutcome::Pending => Change::default(),
        })
    }

    /// Raises the attention notice for every outage that has lasted long enough.
    pub fn check_outages(&self) -> Result<()> {
        let now = self.clock.monotonic();
        let due: Vec<String> = self
            .memory()
            .outages
            .iter()
            .filter(|(_, start)| now.saturating_sub(**start) >= OUTAGE_ATTENTION)
            .map(|(run_id, _)| run_id.clone())
            .collect();
        for run_id in due {
            self.commit(&run_id, |_, _| Change {
                attention: true,
                ..Change::default()
            })?;
        }
        Ok(())
    }

    /// Writes a change and its notices in one transaction, then publishes it.
    pub(super) fn commit(
        &self,
        run_id: &str,
        decide: impl FnOnce(Phase, &EngineRunState) -> Change,
    ) -> Result<()> {
        let now = self.clock.wall_ms();
        let (phase, contact, cancelling, chats, changed) = {
            let store = self.store();
            let tx = store.begin_immediate()?;
            let (run, state) = load(&store, run_id)?;
            let old_phase = phase_of(&run)?;
            let change = decide(old_phase, &state);

            let recorded = match &change.handle {
                Some((attempt, handle)) => store.record_run_handle(run_id, *attempt, handle)?,
                None => false,
            };
            let phase = if recorded && old_phase == Phase::Submitting {
                Phase::Starting
            } else {
                change
                    .phase
                    .filter(|to| to.rank() > old_phase.rank())
                    .filter(|to| old_phase != Phase::Submitting || to.is_terminal())
                    .unwrap_or(old_phase)
            };
            let moved = phase != old_phase;
            if moved || (old_phase == Phase::Cancelled && is_tombstone(old_phase, &state.contact)) {
                if change.cancel_unconfirmed.is_some()
                    || old_phase == Phase::Cancelled && change.contact == Some(Contact::Live)
                {
                    let unconfirmed = change
                        .cancel_unconfirmed
                        .as_ref()
                        .and_then(Option::as_ref)
                        .map(serde_json::to_string)
                        .transpose()?;
                    tx.execute(
                        "UPDATE runs SET cancel_unconfirmed = ?2 WHERE id = ?1",
                        rusqlite::params![run_id, unconfirmed],
                    )?;
                }
                if let Some(reason) = &change.reason {
                    store.set_result_markdown(run_id, reason)?;
                }
            }
            let ended_at = phase.is_terminal().then(|| {
                change
                    .finished_at
                    .unwrap_or(now)
                    .min(now)
                    .max(run.created_at)
            });
            if moved && !recorded {
                let started_at = change
                    .started_at
                    .filter(|_| phase == Phase::Running && super::is_engine_row(&run));
                if phase.is_terminal() && super::handle_of(&run).is_some() {
                    store.queue_log_collection(run_id)?;
                }
                store.set_engine_phase(
                    run_id,
                    old_phase.as_str(),
                    phase.as_str(),
                    ended_at,
                    change.exit_code,
                    started_at,
                )?;
            } else if is_tombstone(old_phase, &state.contact) && change.finished_at.is_some() {
                tx.execute(
                    "UPDATE runs SET ended_at = ?2, exit_code = COALESCE(?3, exit_code) WHERE id = ?1",
                    rusqlite::params![run_id, ended_at, change.exit_code],
                )?;
            }

            let waiting = match phase {
                Phase::Submitting | Phase::Starting => change
                    .waiting
                    .clone()
                    .unwrap_or_else(|| state.waiting_reason.clone()),
                _ => None,
            };
            let waiting_changed = waiting != state.waiting_reason;
            if waiting_changed {
                tx.execute(
                    "UPDATE runs SET waiting_reason = ?2 WHERE id = ?1",
                    rusqlite::params![run_id, waiting],
                )?;
            }

            let settled = phase.is_terminal() && !moved && state.contact == Contact::Live;
            let contact = match change.contact {
                Some(new) if !settled => merged(&state.contact, new),
                _ => state.contact.clone(),
            };
            let contact_changed = contact != state.contact;
            if contact_changed {
                store.set_run_contact(run_id, &contact)?;
            }

            let terminal_body = match moved && phase.is_terminal() {
                true => Some(store.terminal_notice_body(&load(&store, run_id)?.0)?),
                false => None,
            };
            let mut chats = Vec::new();
            for chat in store.run_subscribers(run_id)? {
                let notice = if let Some(body) = &terminal_body {
                    Some((TERMINAL, body.clone()))
                } else if phase == Phase::Cancelled && contact_changed && contact == Contact::Live {
                    if store.queue_cancel_settled_notice(run_id, &chat, now)? {
                        chats.push(chat);
                    }
                    continue;
                } else if phase.is_terminal() {
                    None
                } else if contact == Contact::Live {
                    (contact_changed && store.settle_outage_notice(run_id, &chat)?).then(|| {
                        let status = phase.as_str();
                        let body =
                            format!("[orx] Run `{run_id}` is reachable again; status {status}.");
                        (RECOVERED, body)
                    })
                } else {
                    let raise = change.attention
                        || (discriminant(&contact) != discriminant(&state.contact)
                            && !matches!(contact, Contact::Unreachable { .. }));
                    raise.then(|| {
                        let mut body = attention_body(run_id, &contact);
                        if matches!(contact, Contact::NeedsAuth { .. } | Contact::Unreachable { .. })
                            && (matches!(super::handle_of(&run), Some(Handle::OpenResearch { .. }))
                                || matches!(super::handle_of(&run), Some(Handle::Legacy { descriptor }) if descriptor.kind == "openresearch_job"))
                        {
                            body.push(' ');
                            body.push_str(&crate::plane::LABELS["run_notice_rental_billing"]);
                        }
                        (ATTENTION, body)
                    })
                };
                let Some((kind, body)) = notice else {
                    continue;
                };
                let dedupe = if kind == TERMINAL {
                    TERMINAL.to_string()
                } else {
                    store.outage_dedupe(run_id, &chat, kind)?
                };
                if store.insert_run_notice(run_id, &chat, kind, &dedupe, &body, now)? {
                    chats.push(chat);
                }
            }
            tx.commit()?;
            let cancelling = (run.cancel_requested || state.cancel_accepted_at.is_some())
                && !phase.is_terminal();
            (
                phase,
                contact,
                cancelling,
                chats,
                moved || contact_changed || recorded || waiting_changed,
            )
        };

        {
            let mut memory = self.memory();
            if matches!(contact, Contact::Unreachable { .. }) && !phase.is_terminal() {
                let now = self.clock.monotonic();
                memory.outages.entry(run_id.into()).or_insert(now);
            } else {
                memory.outages.remove(run_id);
            }
            if matches!(
                contact,
                Contact::Unreachable { .. } | Contact::NeedsAuth { .. }
            ) {
                memory.cancels_sent.remove(run_id);
            }
            let tombstone = is_tombstone(phase, &contact);
            if phase != Phase::Submitting && !tombstone {
                memory.retries.remove(run_id);
            }
            if phase.is_terminal() && !tombstone {
                memory.cancels_sent.remove(run_id);
                memory.verifying.remove(run_id);
                memory.locks.release(run_id);
            }
        }
        if changed {
            self.bus.publish(RunEvent::Changed {
                run_id: run_id.into(),
                phase,
                contact,
                cancelling,
            });
        }
        if !chats.is_empty() {
            self.bus.publish(RunEvent::Notices { chats });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_observations_never_move_the_phase() {
        for phase in [Phase::Starting, Phase::Running] {
            for observation in [
                Observation::Unreachable {
                    cause: "eof".into(),
                },
                Observation::NeedsAuth {
                    cause: "denied".into(),
                },
                Observation::Unreconciled { detail: "?".into() },
                Observation::NotFound,
                Observation::Notify { len: 3 },
            ] {
                assert_eq!(observed(phase, &Contact::Live, observation, 1).phase, None);
            }
        }
    }

    #[test]
    fn submitting_runs_ignore_observations_and_settled_runs_ignore_everything() {
        for observation in [
            Observation::Running { started_at: None },
            Observation::Cancelled {
                finished_at: None,
                code: None,
                signal: None,
            },
        ] {
            for phase in [Phase::Submitting, Phase::Done] {
                let change = observed(phase, &Contact::Live, observation.clone(), 1);
                assert!(change.phase.is_none() && change.contact.is_none());
            }
        }
    }

    #[test]
    fn a_continuing_outage_keeps_its_start() {
        let old = Contact::Unreachable {
            since: 1,
            cause: "eof".into(),
        };
        let next = Contact::Unreachable {
            since: 9,
            cause: "timeout".into(),
        };
        assert_eq!(
            merged(&old, next),
            Contact::Unreachable {
                since: 1,
                cause: "timeout".into()
            }
        );
        let auth = Contact::NeedsAuth {
            since: 9,
            cause: "denied".into(),
        };
        assert_eq!(merged(&old, auth.clone()), auth);
    }
}
