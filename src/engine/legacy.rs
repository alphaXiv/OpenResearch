//! Runner-1 rows remain read-only until their launch has settled and no old supervisor holds them.

use super::bus::RunEvent;
use super::{handle_of, phase_of, Engine};
use crate::error::Result;
use crate::jobs::ssh::ContainerRun;
use crate::jobs::BackendDescriptor;
use crate::store::{Contact, RunStatus, SeenRow, StoredRun, ATTENTION, RECOVERED, TERMINAL};

/// 0.2.x launched without wrapper or tags, so `CancelLegacy` only reaches the recorded group.
const LEGACY_CANCEL_NOTE: &str = "Cancelled. This run was launched by orx 0.2.x, so only its \
     process group was stopped; workers it started in new sessions are not covered (OR-469).";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adoption {
    /// An old supervisor holds the run: read its row until terminal, never write it.
    Watch,
    /// `ssh_job`: `InspectLegacy` for state, `CancelLegacy` with the shipped container step.
    Inspect {
        dir: String,
        container: Option<ContainerRun>,
    },
    /// Other kinds: the native adapter reading the old descriptor fields.
    Native { kind: String },
}

fn adoption(descriptor: BackendDescriptor) -> Option<Adoption> {
    Some(match descriptor.kind.as_str() {
        "ssh_job" => Adoption::Inspect {
            dir: descriptor.job_id?,
            container: descriptor.ssh_container,
        },
        kind if matches!(
            kind,
            "slurm_job" | "local_job" | "tinker_job" | "openresearch_job"
        ) || super::adapters::api::provider(kind).is_ok() =>
        {
            Adoption::Native {
                kind: descriptor.kind,
            }
        }
        _ => return None,
    })
}

pub(super) fn cancel_note(run: &StoredRun) -> Option<String> {
    match handle_of(run)? {
        super::Handle::Legacy { descriptor } if descriptor.kind == "ssh_job" => {
            Some(LEGACY_CANCEL_NOTE.into())
        }
        _ => None,
    }
}

impl Engine {
    pub fn scan_legacy(&self) -> Result<Vec<(String, Adoption)>> {
        let (candidates, interrupted, tombstones) = {
            let store = self.store();
            let mut candidates = Vec::new();
            let mut interrupted = Vec::new();
            // Only the engine writes contact, so a 0.2.x tombstone was already adopted once.
            let mut tombstones = Vec::new();
            for run in store.list_engine_runs()? {
                let Ok(descriptor) = BackendDescriptor::parse(&run.backend_json) else {
                    continue;
                };
                let tombstone = super::Phase::parse(&run.status)
                    .is_some_and(super::Phase::is_terminal)
                    && store
                        .engine_run_state(&run.id)?
                        .is_some_and(|state| state.contact != Contact::Live);
                if descriptor.kind != "engine" && tombstone {
                    if let Some(adoption) = adoption(descriptor) {
                        tombstones.push((run.id, adoption));
                    }
                }
            }
            for run in store.list_active_runs()? {
                let Ok(descriptor) = BackendDescriptor::parse(&run.backend_json) else {
                    continue;
                };
                if descriptor.kind == "engine"
                    || store.get_local_experiment(&run.experiment_id)?.is_none()
                {
                    continue;
                }
                if descriptor.job_id.is_none() {
                    interrupted.push(run);
                } else if let Some(adoption) = adoption(descriptor) {
                    candidates.push((run, adoption));
                }
            }
            (candidates, interrupted, tombstones)
        };

        // Without a handle a 0.2.x CLI may still be submitting it, so wait out the same settle.
        for run in interrupted {
            if self.memory().locks.holds(&run.id)
                || self.clock.wall_ms() - run.updated_at < super::ORPHANED_RUN_SETTLE_MS
                || !self.own(&run.id)?
            {
                continue;
            }
            let settled = self.settle_interrupted_submission(&run.id);
            self.memory().locks.release(&run.id);
            settled?;
        }

        #[cfg(test)]
        {
            let hook = self.memory().after_legacy_snapshot.take();
            if let Some(hook) = hook {
                hook();
            }
        }

        let mut decisions = Vec::new();
        for (id, adoption) in tombstones {
            if !self.memory().locks.holds(&id) && self.own(&id)? {
                decisions.push((id, adoption));
            }
        }
        for (run, adoption) in candidates {
            if self.memory().locks.holds(&run.id) {
                continue;
            }
            if self.clock.wall_ms() - run.updated_at < super::ORPHANED_RUN_SETTLE_MS
                || !self.own(&run.id)?
            {
                self.store().watch_legacy_run(&run.id, None)?;
                decisions.push((run.id, Adoption::Watch));
            } else if self.reread_watch(&run.id, None, true)?.is_some() {
                decisions.push((run.id, adoption));
            } else {
                // Its supervisor finished it between the snapshot and the lock.
                self.memory().locks.release(&run.id);
            }
        }
        let watches = self.store().legacy_watches()?;
        for (id, seen) in watches {
            self.reread_watch(&id, seen.as_ref(), false)?;
        }
        Ok(decisions)
    }

    /// Takes the handle a 0.2.x launch kept in its recovery file, or fails the run as its
    /// supervisor did when the launch died before recording one.
    fn settle_interrupted_submission(&self, id: &str) -> Result<()> {
        {
            let store = self.store();
            let tx = store.begin_immediate()?;
            let Some(run) = store.get_run(id)? else {
                return Ok(());
            };
            let descriptor = BackendDescriptor::parse(&run.backend_json)?;
            if phase_of(&run)?.is_terminal() || descriptor.job_id.is_some() {
                return Ok(());
            }
            let backend = descriptor.kind.trim_end_matches("_job");
            let recovered = store.submission_handle(id).and_then(|json| {
                json.map(|json| BackendDescriptor::parse(&json).map(|parsed| (json, parsed)))
                    .transpose()
            });
            let reason = match recovered {
                Ok(Some((json, parsed))) if parsed.job_id.is_some() => {
                    store.set_backend_json(id, &json)?;
                    tx.commit()?;
                    return Ok(());
                }
                Err(error) => format!(
                    "orx 0.2 kept this run's {backend} handle in a recovery file that could not be read ({error})."
                ),
                _ => format!("Submission was interrupted before the {backend} handle was recorded."),
            };
            if store.update_status(id, RunStatus::Failed, Some(self.clock.wall_ms()), None)? {
                store.set_result_markdown(
                    id,
                    &format!(
                        "{reason} Inspect the provider for resources labelled or_run={id} before retrying."
                    ),
                )?;
            }
            tx.commit()?;
        }
        self.reread_watch(id, None, false)?;
        Ok(())
    }

    /// Rereads a watched run and records what it found in one transaction: a finished run's
    /// chat results are queued as its watch ends. Returns the row while it is still active;
    /// `adopting` ends the watch of an active run too.
    fn reread_watch(
        &self,
        id: &str,
        seen: Option<&SeenRow>,
        adopting: bool,
    ) -> Result<Option<StoredRun>> {
        let now = self.clock.wall_ms();
        let (run, chats, terminal, changed) = {
            let store = self.store();
            let tx = store.begin_immediate()?;
            let Some(run) = store.get_run(id)? else {
                store.unwatch_legacy_run(id)?;
                tx.commit()?;
                return Ok(None);
            };
            let terminal = phase_of(&run)?.is_terminal();
            let mut chats = Vec::new();
            if terminal {
                let body = store.terminal_notice_body(&run)?;
                for chat in store.run_subscribers(id)? {
                    if store.insert_run_notice(id, &chat, TERMINAL, TERMINAL, &body, now)? {
                        chats.push(chat);
                    }
                }
            } else if !adopting {
                let descriptor = BackendDescriptor::parse(&run.backend_json)?;
                for chat in store.run_subscribers(id)? {
                    let (kind, body) = if descriptor.monitoring_error.is_some() {
                        let body = format!(
                            "[orx] orx can no longer monitor these live runs:\n\
                             - run `{id}` of experiment `{}` (still **{}**)\n\
                             Run `orx exp status <expId>` for the reason and tell the user so they \
                             can fix it. orx wakes you once it can see a run finish; until then \
                             each run keeps its current status.",
                            run.experiment_id, run.status
                        );
                        (ATTENTION, body)
                    } else if store.settle_outage_notice(id, &chat)? {
                        (
                            RECOVERED,
                            format!(
                                "[orx] Run `{id}` is reachable again; status {}.",
                                run.status
                            ),
                        )
                    } else {
                        continue;
                    };
                    let dedupe = store.outage_dedupe(id, &chat, kind)?;
                    if store.insert_run_notice(id, &chat, kind, &dedupe, &body, now)? {
                        chats.push(chat);
                    }
                }
            }
            let changed = seen != Some(&(run.status.clone(), run.updated_at));
            if terminal || adopting {
                store.unwatch_legacy_run(id)?;
            } else if changed {
                store.watch_legacy_run(id, Some((&run.status, run.updated_at)))?;
            }
            tx.commit()?;
            (run, chats, terminal, changed)
        };
        if terminal || (changed && !adopting) {
            let phase = phase_of(&run)?;
            self.bus.publish(RunEvent::Changed {
                run_id: id.into(),
                phase,
                contact: Contact::Live,
                cancelling: run.cancel_requested && !terminal,
            });
        }
        if !chats.is_empty() {
            self.bus.publish(RunEvent::Notices { chats });
        }
        Ok((!terminal).then_some(run))
    }
}
