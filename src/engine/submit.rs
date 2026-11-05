//! §2: submission with fences and recovery, cancel at every point of it, and Abandon.

use super::clock::backoff;
use super::reconcile::{is_tombstone, Change, ABANDONED};
use super::{
    handle_of, phase_of, process, Backend, Engine, Fence, Handle, LaunchError, Phase, Recovery,
    RunIntent,
};
use crate::error::{anyhow, Result};
use crate::store::{Contact, EngineRunState, SubmissionFence, SubmittingRun};

/// Ends `attempt` if it is still the current one: Cancelled when a cancel was accepted, else
/// Failed with `rejected`. A superseded attempt's result changes nothing.
fn end_attempt(
    attempt: u32,
    phase: Phase,
    state: &EngineRunState,
    rejected: Option<String>,
) -> Change {
    if phase != Phase::Submitting || state.submission_attempt != attempt {
        return Change::default();
    }
    match (state.cancel_accepted_at, rejected) {
        (Some(_), _) => Change::terminal(Phase::Cancelled, None),
        (None, Some(reason)) => Change::terminal(Phase::Failed, Some(reason)),
        (None, None) => Change::default(),
    }
}

impl Engine {
    /// Commits the intent before any side effect, then launches attempt 1.
    #[cfg(test)]
    pub async fn submit(&self, backend: &dyn Backend, run: SubmittingRun<'_>) -> Result<()> {
        let run_id = run.id.to_string();
        self.insert(run)?;
        self.launch_inserted(backend, &run_id).await
    }

    /// The durable half of [`Self::submit`]: from here recovery owns the run.
    pub fn insert(&self, run: SubmittingRun<'_>) -> Result<()> {
        self.store().insert_submitting_run(run)?;
        #[cfg(all(test, unix))]
        super::runtime::chaos::crash_point("engine_intent");
        Ok(())
    }

    pub async fn launch_inserted(&self, backend: &dyn Backend, run_id: &str) -> Result<()> {
        let run_id = run_id.to_string();
        if !self.own(&run_id)? {
            return Err(anyhow!("Run {run_id} is locked by another process"));
        }
        let _gate = self.gate(&run_id).lock_owned().await;
        if let Some(error) = self.launch(backend, &run_id, 1).await? {
            self.recover_gated(backend, &run_id, Some(error)).await?;
        }
        Ok(())
    }

    /// Fences and launches one attempt; the launch error when its outcome is unknown and needs
    /// recovery.
    async fn launch(
        &self,
        backend: &dyn Backend,
        run_id: &str,
        attempt: u32,
    ) -> Result<Option<String>> {
        let taken_at = self.clock.wall_ms();
        let fence = SubmissionFence {
            taken_at,
            submitter: None,
        };
        if !self.store().set_submission_fence(run_id, attempt, &fence)? {
            // Refused before any side effect; an accepted cancel ends the attempt.
            self.commit(run_id, |phase, state| {
                end_attempt(attempt, phase, state, None)
            })?;
            return Ok(None);
        }
        #[cfg(all(test, unix))]
        super::runtime::chaos::crash_point("engine_fence");
        let (run, state) = self.load(run_id)?;
        let intent = RunIntent::new(run, &state)?;
        let fence = Fence {
            store: &self.store,
            run_id,
            attempt,
            taken_at,
        };
        match backend.launch(&intent, &fence).await {
            Ok(handle) => {
                #[cfg(all(test, unix))]
                super::runtime::chaos::crash_point("engine_response");
                self.found(backend, run_id, attempt, handle).await?;
            }
            Err(LaunchError::Rejected { reason }) => {
                self.commit(run_id, |phase, state| {
                    end_attempt(attempt, phase, state, Some(reason))
                })?;
            }
            Err(LaunchError::Blocked { contact }) => {
                if self.store().clear_submission_fence(run_id, attempt)? {
                    self.commit(run_id, |phase, state| {
                        if phase != Phase::Submitting || state.submission_attempt != attempt {
                            return Change::default();
                        }
                        if state.cancel_accepted_at.is_some() {
                            end_attempt(attempt, phase, state, None)
                        } else {
                            Change::contact(contact)
                        }
                    })?;
                }
            }
            Err(LaunchError::Interrupted { detail }) => return Ok(Some(detail)),
        }
        Ok(None)
    }

    async fn found(
        &self,
        backend: &dyn Backend,
        run_id: &str,
        attempt: u32,
        handle: Handle,
    ) -> Result<()> {
        self.commit(run_id, |phase, state| Change {
            handle: Some((attempt, handle)),
            contact: (!is_tombstone(phase, &state.contact)).then_some(Contact::Live),
            ..Change::default()
        })?;
        #[cfg(all(test, unix))]
        super::runtime::chaos::crash_point("engine_handle");
        self.deliver_cancel(backend, run_id).await
    }

    /// §2 recovery a–c for a submitting run, or an abandoned one that never got a handle.
    /// Unknown leaves it Unreconciled and retries with backoff; it is never failed for it.
    /// A run another task is already working on is left to it.
    pub async fn recover(&self, backend: &dyn Backend, run_id: &str) -> Result<()> {
        let Some(_gate) = self.try_gate(run_id) else {
            return Ok(());
        };
        self.recover_gated(backend, run_id, None).await
    }

    /// A relaunch after Absent that is interrupted again backs off rather than looping.
    async fn recover_gated(
        &self,
        backend: &dyn Backend,
        run_id: &str,
        mut launch_error: Option<String>,
    ) -> Result<()> {
        if !self.own(run_id)? {
            return Ok(());
        }
        loop {
            let now = self.clock.monotonic();
            let backing_off = self
                .memory()
                .retries
                .get(run_id)
                .is_some_and(|(_, due)| *due > now);
            if backing_off {
                return Ok(());
            }
            let (run, state) = self.load(run_id)?;
            let phase = phase_of(&run)?;
            let tombstone = is_tombstone(phase, &state.contact) && handle_of(&run).is_none();
            if phase != Phase::Submitting && !tombstone {
                return Ok(());
            }
            let attempt = state.submission_attempt;

            let Some(fence) = &state.submission_fence else {
                // The fence precedes every side effect, so this attempt never launched.
                if tombstone {
                    return self.commit(run_id, |_, _| Change::contact(Contact::Live));
                }
                launch_error = self.launch(backend, run_id, attempt).await?;
                if launch_error.is_none() {
                    return Ok(());
                }
                continue;
            };
            if let Some(submitter) = &fence.submitter {
                if !process::ensure_gone(submitter).await {
                    return self.unknown(run_id, "The previous submitter could not be stopped.");
                }
            }
            let intent = RunIntent::new(run, &state)?;
            let recovery_fence = Fence {
                store: &self.store,
                run_id,
                attempt,
                taken_at: fence.taken_at,
            };
            match backend
                .recover_with_cancel(&intent, &|| recovery_fence.cancelled())
                .await
            {
                Recovery::Found(handle) => {
                    return self.found(backend, run_id, attempt, handle).await
                }
                Recovery::Absent if tombstone => {
                    return self.commit(run_id, |_, _| Change::contact(Contact::Live))
                }
                Recovery::Absent => {
                    if !self.store().advance_submission_attempt(run_id, attempt)? {
                        return self.commit(run_id, |phase, state| {
                            end_attempt(attempt, phase, state, None)
                        });
                    }
                    return match self.launch(backend, run_id, attempt + 1).await? {
                        Some(error) => self.unknown(run_id, format!("The launch failed: {error}")),
                        None => Ok(()),
                    };
                }
                Recovery::Unknown { detail } => {
                    let detail = match launch_error {
                        Some(error) => format!("{detail}; the launch failed: {error}"),
                        None => detail,
                    };
                    return self.unknown(run_id, detail);
                }
            }
        }
    }

    fn unknown(&self, run_id: &str, detail: impl Into<String>) -> Result<()> {
        {
            let mut memory = self.memory();
            let failures = memory.retries.get(run_id).map_or(0, |(n, _)| *n) + 1;
            let due = self.clock.monotonic() + backoff(failures);
            memory.retries.insert(run_id.into(), (failures, due));
        }
        let now = self.clock.wall_ms();
        let detail = detail.into();
        self.commit(run_id, |phase, state| {
            if is_tombstone(phase, &state.contact) {
                return Change::default();
            }
            Change::contact(Contact::Unreconciled { since: now, detail })
        })
    }

    /// Before the fence cancel is immediate; after it, the backend's stop proof decides the result.
    pub async fn cancel(&self, backend: &dyn Backend, run_id: &str, reason: &str) -> Result<bool> {
        if !self.own(run_id)?
            || self
                .store()
                .accept_run_cancel(run_id, Some(reason))?
                .is_none()
        {
            return Ok(false);
        }
        // A busy gate means a launch or recovery is running; it will see the accepted cancel.
        if let Some(_gate) = self.try_gate(run_id) {
            self.commit(run_id, |phase, state| {
                if phase == Phase::Submitting && state.submission_fence.is_none() {
                    Change::terminal(Phase::Cancelled, None)
                } else {
                    Change::default()
                }
            })?;
        }
        let (run, state) = self.load(run_id)?;
        let phase = phase_of(&run)?;
        if !phase.is_terminal() {
            self.bus.publish(super::bus::RunEvent::Changed {
                run_id: run_id.into(),
                phase,
                contact: state.contact,
                cancelling: true,
            });
        }
        self.deliver_cancel(backend, run_id).await?;
        Ok(true)
    }

    /// Lost now, with the chat told; the fence and the cancel stay alive for if the job returns.
    pub fn abandon(&self, run_id: &str) -> Result<bool> {
        if !self.own(run_id)?
            || self
                .store()
                .accept_run_cancel(run_id, Some("Abandoned."))?
                .is_none()
        {
            return Ok(false);
        }
        let now = self.clock.wall_ms();
        self.commit(run_id, |_, _| Change {
            contact: Some(Contact::Unreconciled {
                since: now,
                detail: ABANDONED.into(),
            }),
            ..Change::terminal(Phase::Lost, Some(ABANDONED.into()))
        })?;
        Ok(true)
    }
}
