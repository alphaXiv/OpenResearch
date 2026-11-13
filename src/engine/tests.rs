use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::Receiver;

use super::bus::RunEvent;
use super::clock::TestClock;
use super::fake::{FakeBackend, HostFile};
use super::legacy::Adoption;
use super::*;
use crate::local::model::LocalExperiment;
use crate::store::{
    LaunchSpec, RunNotice, StoredRun, SubmissionFence, SubmittingRun, ATTENTION, NOTIFY, RECOVERED,
    TERMINAL,
};

const CHAT: &str = "chat";

struct Fixture {
    dir: PathBuf,
    clock: Arc<TestClock>,
    engine: Engine,
    backend: FakeBackend,
    events: Receiver<RunEvent>,
}

impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("orx-engine-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .create_local_experiment(&LocalExperiment {
                id: "exp".into(),
                project_id: "proj".into(),
                parent_experiment_id: None,
                slug: "exp".into(),
                branch_name: "orx/exp".into(),
                title: None,
                description: None,
                run_command: "train".into(),
                agent_status: "idle".into(),
                created_at: 1,
                updated_at: 1,
                chat_session_id: None,
                archived: false,
            })
            .unwrap();
        let clock = Arc::new(TestClock::new());
        let engine = Engine::new(store, clock.clone());
        let events = engine.bus.subscribe();
        Self {
            dir,
            clock,
            engine,
            backend: FakeBackend::default(),
            events,
        }
    }

    /// A second connection, as another process would have.
    fn store(&self) -> Store {
        Store::open_at(self.dir.clone()).unwrap()
    }

    /// Simulates a crash: a fresh engine over the same data, with nothing in memory, once the
    /// old one's locks are gone as they would be with its process.
    fn restart(&mut self) {
        let held: Vec<String> = std::fs::read_dir(self.dir.join("run-logs"))
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let name = entry.unwrap().file_name().into_string().unwrap();
                name.strip_suffix(".supervisor.lock").map(str::to_owned)
            })
            .filter(|id| self.engine.holds(id))
            .collect();
        let engine = Engine::new(self.store(), self.clock.clone());
        self.events = engine.bus.subscribe();
        self.engine = engine;
        for id in held {
            self.released(&id);
        }
    }

    async fn submit(&self, id: &str) -> Result<()> {
        let metadata = BackendDescriptor::parse(r#"{"kind":"ssh_job","sourceDigest":"abc"}"#)?;
        self.engine
            .submit(
                &self.backend,
                SubmittingRun {
                    id,
                    experiment_id: "exp",
                    project_id: "proj",
                    metadata: &metadata,
                    command: "train",
                    created_at: 1,
                    commit_sha: None,
                    chat_session_id: Some(CHAT),
                    spec_digest: "spec",
                    launch: &ssh_launch(),
                },
            )
            .await
    }

    async fn running(&self, id: &str) {
        self.backend.then_launch(Ok(handle()));
        self.submit(id).await.unwrap();
        self.engine
            .observe(id, Observation::Running { started_at: None })
            .unwrap();
        assert_eq!(self.status(id), "running");
    }

    fn status(&self, id: &str) -> String {
        self.store().get_run(id).unwrap().unwrap().status
    }

    fn contact(&self, id: &str) -> Contact {
        self.store().engine_run_state(id).unwrap().unwrap().contact
    }

    /// Freezes, reads and acknowledges the chat's next notice batch.
    fn deliver(&self) -> Vec<RunNotice> {
        let store = self.store();
        let batch = store.run_notice_batch(CHAT).unwrap();
        if let Some(first) = batch.first() {
            store
                .mark_run_notice_batch_delivered(CHAT, &first.client_turn_id, 1)
                .unwrap();
        }
        batch
    }

    fn kinds(&self) -> Vec<String> {
        self.deliver()
            .into_iter()
            .map(|notice| notice.kind)
            .collect()
    }

    fn events(&mut self) -> Vec<RunEvent> {
        std::iter::from_fn(|| self.events.try_recv().ok()).collect()
    }

    fn phases(&mut self) -> Vec<Phase> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                RunEvent::Changed { phase, .. } => Some(phase),
                _ => None,
            })
            .collect()
    }

    /// Waits until no process holds the run's locks. A dropped lock lingers in any child a
    /// parallel test forked meanwhile, until that child execs, which load can stretch.
    fn released(&self, id: &str) {
        released(&self.store().supervisor_lock_paths(id).unwrap());
    }

    fn locked(&self, id: &str) -> bool {
        self.store()
            .supervisor_lock_paths(id)
            .unwrap()
            .iter()
            .all(|path| {
                matches!(
                    lock_file(path).try_lock(),
                    Err(std::fs::TryLockError::WouldBlock)
                )
            })
    }

    fn legacy_run(&self, id: &str, backend_json: &str) {
        self.store()
            .upsert_run(&StoredRun {
                id: id.into(),
                experiment_id: "exp".into(),
                project_id: "proj".into(),
                status: "running".into(),
                backend_json: backend_json.into(),
                command: "train".into(),
                created_at: 1,
                updated_at: 1,
                ended_at: None,
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: Some(CHAT.into()),
            })
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn released(paths: &[PathBuf]) {
    let free = || paths.iter().all(|path| lock_file(path).try_lock().is_ok());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !free() {
        assert!(
            std::time::Instant::now() < deadline,
            "{paths:?} stayed locked"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn lock_file(path: &std::path::Path) -> std::fs::File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .unwrap()
}

fn ssh_launch() -> LaunchSpec {
    LaunchSpec {
        kind: "ssh_job".into(),
        target: Some("lab".into()),
        options: serde_json::json!({ "root": "/orx", "container": null }),
    }
}

fn handle() -> Handle {
    Handle::Ssh {
        host: "lab".into(),
        root: "/orx".into(),
    }
}

fn unreconciled(contact: &Contact) -> bool {
    matches!(contact, Contact::Unreconciled { .. })
}

#[tokio::test]
async fn phase_moves_only_forward_and_one_terminal_notice_survives_a_crash() {
    let mut f = Fixture::new();
    f.backend.then_launch(Ok(handle()));
    f.submit("run").await.unwrap();
    assert_eq!(f.status("run"), "starting");
    assert!(f.locked("run"));
    let row = f.store().get_run("run").unwrap().unwrap();
    assert!(matches!(handle_of(&row), Some(Handle::Ssh { .. })));

    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    f.engine
        .observe(
            "run",
            Observation::Queued {
                detail: Some("late".into()),
            },
        )
        .unwrap();
    assert_eq!(f.status("run"), "running");
    f.engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(3),
                signal: None,
            },
        )
        .unwrap();
    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    f.engine
        .observe(
            "run",
            Observation::Cancelled {
                finished_at: None,
                code: None,
                signal: None,
            },
        )
        .unwrap();
    let row = f.store().get_run("run").unwrap().unwrap();
    assert_eq!((row.status.as_str(), row.exit_code), ("failed", Some(3)));
    assert_eq!(f.phases(), [Phase::Starting, Phase::Running, Phase::Failed]);
    f.released("run");

    // Crash after the transition committed, before the notice was delivered.
    f.restart();
    f.engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    let batch = f.store().run_notice_batch(CHAT).unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(batch[0].kind, TERMINAL);
    assert!(batch[0].body.contains("**failed**"));
    // Crash after the batch was frozen and sent, before it was marked delivered.
    f.restart();
    f.engine
        .observe("run", Observation::Lost { reason: "?".into() })
        .unwrap();
    assert_eq!(f.deliver(), batch);
    assert!(f.deliver().is_empty());
    assert!(f.phases().is_empty());
}

#[tokio::test]
async fn a_frozen_batch_keeps_its_members_and_later_notices_wait_for_the_next() {
    let f = Fixture::new();
    f.running("a").await;
    f.running("b").await;
    f.engine
        .observe(
            "a",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    let store = f.store();
    let frozen = store.run_notice_batch(CHAT).unwrap();
    f.engine
        .observe(
            "b",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    assert_eq!(store.run_notice_batch(CHAT).unwrap(), frozen);
    assert_eq!(f.deliver(), frozen);
    let next = f.deliver();
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].run_id, "b");
}

#[tokio::test]
async fn contact_never_moves_the_phase_and_raises_one_attention_per_outage() {
    let f = Fixture::new();
    f.running("run").await;
    let start = f.clock.wall_ms();
    f.engine
        .observe(
            "run",
            Observation::Unreachable {
                cause: "eof".into(),
            },
        )
        .unwrap();
    f.clock.advance(Duration::from_secs(9 * 60));
    f.engine
        .observe(
            "run",
            Observation::Unreachable {
                cause: "timeout".into(),
            },
        )
        .unwrap();
    f.engine.check_outages().unwrap();
    assert_eq!(
        f.contact("run"),
        Contact::Unreachable {
            since: start,
            cause: "timeout".into()
        }
    );
    assert!(f.kinds().is_empty());
    f.clock.advance(Duration::from_secs(60));
    f.engine.check_outages().unwrap();
    f.engine.check_outages().unwrap();
    f.engine
        .observe(
            "run",
            Observation::NeedsAuth {
                cause: "denied".into(),
            },
        )
        .unwrap();
    assert_eq!(f.status("run"), "running");
    assert_eq!(f.kinds(), [ATTENTION]);

    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    assert_eq!(f.contact("run"), Contact::Live);
    assert_eq!(f.kinds(), [RECOVERED]);

    // The next outage is a new one; an attention resolved before it was sent is withdrawn.
    f.engine
        .observe(
            "run",
            Observation::NeedsAuth {
                cause: "denied".into(),
            },
        )
        .unwrap();
    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    assert!(f.kinds().is_empty());
    f.engine
        .observe(
            "run",
            Observation::Unreconciled {
                detail: "wrapper died".into(),
            },
        )
        .unwrap();
    let attention = f.deliver();
    assert_eq!(attention.len(), 1);
    assert!(attention[0].body.contains("wrapper died"));
    assert_eq!(f.status("run"), "running");
}

#[tokio::test]
async fn cancel_before_any_side_effect_is_immediate() {
    let f = Fixture::new();
    let metadata = BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap();
    for id in ["api", "refused"] {
        f.store()
            .insert_submitting_run(SubmittingRun {
                id,
                experiment_id: "exp",
                project_id: "proj",
                metadata: &metadata,
                command: "train",
                created_at: 1,
                commit_sha: None,
                chat_session_id: Some(CHAT),
                spec_digest: "spec",
                launch: &ssh_launch(),
            })
            .unwrap();
    }
    assert!(f.engine.cancel(&f.backend, "api", "user").await.unwrap());
    assert_eq!(f.status("api"), "cancelled");
    // An engine restarting between the intent and the fence finds the cancel and never launches.
    f.store()
        .accept_run_cancel("refused", Some("user"))
        .unwrap();
    f.engine.recover(&f.backend, "refused").await.unwrap();
    assert_eq!(f.status("refused"), "cancelled");
    assert!(f.backend.calls().is_empty());
    assert!(!f.engine.cancel(&f.backend, "api", "again").await.unwrap());
}

#[tokio::test]
async fn cancel_during_submission_rides_the_launch() {
    let f = Fixture::new();
    let dir = f.dir.clone();
    *f.backend.during_launch.lock().unwrap() = Some(Box::new(move || {
        let store = Store::open_at(dir).unwrap();
        store
            .accept_run_cancel("run", Some("user"))
            .unwrap()
            .unwrap();
    }));
    f.backend
        .then_launch(Ok(handle()))
        .then_cancel(CancelOutcome::Pending);
    f.submit("run").await.unwrap();
    assert_eq!(f.status("run"), "starting");
    assert_eq!(f.backend.calls(), ["launch run #1", "cancel run Ssh"]);
    f.engine
        .observe(
            "run",
            Observation::Cancelled {
                finished_at: None,
                code: None,
                signal: None,
            },
        )
        .unwrap();
    assert_eq!(f.status("run"), "cancelled");
}

#[tokio::test]
async fn an_unknown_submission_stays_unreconciled_with_backoff_until_recovery_decides() {
    let f = Fixture::new();
    f.backend
        .then_launch(Err(LaunchError::Interrupted {
            detail: "lost".into(),
        }))
        .then_recover(Recovery::Unknown {
            detail: "host down".into(),
        });
    f.submit("run").await.unwrap();
    assert_eq!(f.status("run"), "submitting");
    assert!(unreconciled(&f.contact("run")));
    assert_eq!(f.kinds(), [ATTENTION]);

    assert!(f.engine.cancel(&f.backend, "run", "user").await.unwrap());
    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.backend.calls().len(), 2);
    f.clock.advance(Duration::from_secs(5));
    f.backend.then_recover(Recovery::Unknown {
        detail: "still down".into(),
    });
    f.engine.recover(&f.backend, "run").await.unwrap();
    f.clock.advance(Duration::from_secs(14));
    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.backend.calls().len(), 3);
    assert_eq!(f.status("run"), "submitting");

    f.clock.advance(Duration::from_secs(1));
    f.backend
        .then_recover(Recovery::Found(handle()))
        .then_cancel(CancelOutcome::Verified);
    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.status("run"), "cancelled");
    assert_eq!(f.contact("run"), Contact::Live);
    assert_eq!(
        f.backend.calls(),
        [
            "launch run #1",
            "recover run #1",
            "recover run #1",
            "recover run #1",
            "cancel run Ssh"
        ]
    );
}

#[tokio::test]
async fn absent_relaunches_the_next_attempt_unless_cancelled() {
    let f = Fixture::new();
    f.backend
        .then_launch(Err(LaunchError::Interrupted {
            detail: "lost".into(),
        }))
        .then_recover(Recovery::Absent)
        .then_launch(Ok(handle()));
    f.submit("relaunched").await.unwrap();
    assert_eq!(f.status("relaunched"), "starting");
    assert_eq!(
        f.backend.calls(),
        [
            "launch relaunched #1",
            "recover relaunched #1",
            "launch relaunched #2"
        ]
    );

    f.backend
        .then_launch(Err(LaunchError::Interrupted {
            detail: "lost".into(),
        }))
        .then_recover(Recovery::Unknown { detail: "?".into() });
    f.submit("cancelled").await.unwrap();
    f.engine
        .cancel(&f.backend, "cancelled", "user")
        .await
        .unwrap();
    f.clock.advance(Duration::from_secs(5));
    f.backend.then_recover(Recovery::Absent);
    f.engine.recover(&f.backend, "cancelled").await.unwrap();
    assert_eq!(f.status("cancelled"), "cancelled");

    f.backend.then_launch(Err(LaunchError::Rejected {
        reason: "no such host".into(),
    }));
    f.submit("rejected").await.unwrap();
    let row = f.store().get_run("rejected").unwrap().unwrap();
    assert_eq!(row.status, "failed");
    assert_eq!(row.result_markdown.as_deref(), Some("no such host"));
    assert_eq!(
        f.backend
            .calls()
            .iter()
            .filter(|c| c.starts_with("launch"))
            .count(),
        4
    );
}

#[tokio::test]
async fn a_relaunch_interrupted_again_waits_out_the_backoff_with_the_launch_error() {
    let f = Fixture::new();
    let interrupted = || {
        Err(LaunchError::Interrupted {
            detail: "disk full".into(),
        })
    };
    f.backend
        .then_launch(interrupted())
        .then_recover(Recovery::Absent)
        .then_launch(interrupted());
    f.submit("run").await.unwrap();
    assert!(
        matches!(f.contact("run"), Contact::Unreconciled { detail, .. } if detail == "The launch failed: disk full")
    );
    assert_eq!(f.kinds(), [ATTENTION]);

    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.backend.calls().len(), 3);

    f.clock.advance(Duration::from_secs(5));
    f.backend
        .then_recover(Recovery::Absent)
        .then_launch(Ok(handle()));
    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.status("run"), "starting");
    assert_eq!(
        f.backend.calls(),
        [
            "launch run #1",
            "recover run #1",
            "launch run #2",
            "recover run #2",
            "launch run #3"
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn recovery_waits_out_a_recorded_submitter_before_asking_the_backend() {
    let f = Fixture::new();
    let mut child = crate::process::command("sleep").arg("60").spawn().unwrap();
    *f.backend.submitter.lock().unwrap() = Some(child.id());
    f.backend
        .then_launch(Err(LaunchError::Interrupted {
            detail: "lost".into(),
        }))
        .then_recover(Recovery::Found(handle()));
    f.submit("run").await.unwrap();
    let fence = f
        .store()
        .engine_run_state("run")
        .unwrap()
        .unwrap()
        .submission_fence;
    assert_eq!(fence.unwrap().submitter.unwrap().pid, child.id());
    assert!(child.try_wait().unwrap().is_some());
    assert_eq!(f.status("run"), "starting");
}

#[tokio::test]
async fn a_death_seen_while_a_cancel_is_verified_waits_for_the_verdict() {
    let f = Fixture::new();
    f.running("run").await;
    let mut events = f.engine.bus.subscribe();
    f.backend.then_cancel(CancelOutcome::Verified);
    let cancel = f.engine.cancel(&f.backend, "run", "user");
    tokio::pin!(cancel);
    assert!(futures::poll!(&mut cancel).is_pending());
    // Dashboards see the accepted cancel before the verdict.
    assert!(matches!(
        events.try_recv(),
        Ok(RunEvent::Changed {
            phase: Phase::Running,
            cancelling: true,
            ..
        })
    ));
    let died = Observation::Failed {
        finished_at: None,
        reason: "process died without an exit code".into(),
    };
    f.engine.observe("run", died.clone()).unwrap();
    assert_eq!(f.status("run"), "running");
    assert!(cancel.await.unwrap());
    assert_eq!(f.status("run"), "cancelled");

    // A delivery dropped before its verdict leaves observation in charge.
    f.running("dropped").await;
    f.backend.then_cancel(CancelOutcome::Verified);
    {
        let cancel = f.engine.cancel(&f.backend, "dropped", "user");
        tokio::pin!(cancel);
        assert!(futures::poll!(&mut cancel).is_pending());
    }
    f.engine.observe("dropped", died).unwrap();
    assert_eq!(f.status("dropped"), "failed");
}

/// A cancel registered between an observation's arrival and its terminal decision still wins.
#[tokio::test]
async fn a_cancel_registered_just_before_a_terminal_commit_is_honored() {
    use futures::FutureExt;
    let f = Fixture::new();
    f.running("run").await;
    f.backend.then_cancel(CancelOutcome::Verified);
    let (arrived, at_commit) = std::sync::mpsc::channel();
    let (proceed, go) = std::sync::mpsc::channel::<()>();
    f.engine.memory().before_observed_commit = Some(Box::new(move || {
        arrived.send(()).unwrap();
        go.recv().unwrap();
    }));
    let cancel = f.engine.cancel(&f.backend, "run", "user");
    tokio::pin!(cancel);
    std::thread::scope(|scope| {
        let observer = scope.spawn(|| {
            let died = Observation::Failed {
                finished_at: None,
                reason: "process died without an exit code".into(),
            };
            f.engine.observe("run", died).unwrap()
        });
        at_commit.recv().unwrap();
        // The cancel registers and parks in the backend while the observation waits to commit.
        assert!(cancel.as_mut().now_or_never().is_none());
        proceed.send(()).unwrap();
        observer.join().unwrap();
    });
    assert_eq!(f.status("run"), "running");
    assert!(cancel.await.unwrap());
    assert_eq!(f.status("run"), "cancelled");
}

#[tokio::test]
async fn cancel_while_unreachable_is_held_until_reconnect_and_sent_once_per_connection() {
    let mut f = Fixture::new();
    f.running("run").await;
    f.engine
        .observe(
            "run",
            Observation::Unreachable {
                cause: "eof".into(),
            },
        )
        .unwrap();
    f.events();
    assert!(f.engine.cancel(&f.backend, "run", "user").await.unwrap());
    f.engine.deliver_cancel(&f.backend, "run").await.unwrap();
    assert!(f
        .backend
        .calls()
        .iter()
        .all(|call| !call.starts_with("cancel")));

    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    assert!(f.events().iter().any(|event| matches!(
        event,
        RunEvent::Changed {
            cancelling: true,
            contact: Contact::Live,
            ..
        }
    )));
    f.backend.then_cancel(CancelOutcome::Survivors { count: 1 });
    f.engine.deliver_cancel(&f.backend, "run").await.unwrap();
    f.engine.deliver_cancel(&f.backend, "run").await.unwrap();
    assert_eq!(f.status("run"), "cancelled");
    assert!(matches!(
        f.contact("run"),
        Contact::Stopping { count: 1, .. }
    ));
    assert_eq!(f.kinds(), [TERMINAL]);

    f.engine
        .observe(
            "run",
            Observation::Unreachable {
                cause: "eof".into(),
            },
        )
        .unwrap();
    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    f.backend.then_cancel(CancelOutcome::Verified);
    f.engine.deliver_cancel(&f.backend, "run").await.unwrap();
    assert_eq!(f.status("run"), "cancelled");
    let cancels = f
        .backend
        .calls()
        .iter()
        .filter(|c| c.starts_with("cancel"))
        .count();
    assert_eq!(cancels, 2);
    assert_eq!(f.kinds(), [ATTENTION]);
}

#[tokio::test]
async fn a_verified_cancel_receipt_keeps_its_exit_code() {
    let f = Fixture::new();
    f.running("run").await;
    f.backend.then_cancel(CancelOutcome::Pending);
    f.engine
        .cancel(&f.backend, "run", "Cancel requested with `orx exp cancel`.")
        .await
        .unwrap();
    f.engine
        .observe(
            "run",
            Observation::Cancelled {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    let row = f.store().get_run("run").unwrap().unwrap();
    assert_eq!((row.status.as_str(), row.exit_code), ("cancelled", Some(0)));
    let body = &f.deliver()[0].body;
    assert!(body.contains("Exit code: 0\n"));
    assert!(body.contains("Reason: Cancel requested with `orx exp cancel`.\n"));
}

#[tokio::test]
async fn accepted_cancels_classify_nonzero_exits_after_restart() {
    for handle in [
        super::adapters::api::test_handle("ray_job", None, None, vec!["job".into()]),
        Handle::Legacy {
            descriptor: Box::new(
                BackendDescriptor::parse(r#"{"kind":"ray_job","jobId":"job"}"#).unwrap(),
            ),
        },
    ] {
        for (requested, code, signal, status) in [
            (true, Some(137), None, "cancelled"),
            (false, Some(137), None, "failed"),
            (true, Some(0), None, "done"),
            (true, None, Some(9), "cancelled"),
        ] {
            let mut f = Fixture::new();
            f.backend.then_launch(Ok(handle.clone()));
            f.submit("run").await.unwrap();
            f.engine
                .observe("run", Observation::Running { started_at: None })
                .unwrap();
            if requested {
                assert!(f
                    .store()
                    .accept_run_cancel("run", Some("Dashboard stop"))
                    .unwrap()
                    .is_some());
            }
            f.restart();
            f.engine
                .observe(
                    "run",
                    Observation::Exited {
                        finished_at: Some(123),
                        code,
                        signal,
                    },
                )
                .unwrap();

            let row = f.store().get_run("run").unwrap().unwrap();
            assert_eq!(row.status, status);
            assert_eq!(row.exit_code, code.map(i64::from));
            assert_eq!(row.ended_at, Some(123));
        }
    }
}

#[tokio::test]
async fn accepted_cancels_do_not_replace_wrapper_or_legacy_stop_proof() {
    for legacy in [false, true] {
        let f = Fixture::new();
        if legacy {
            f.legacy_run("run", r#"{"kind":"ssh_job","jobId":"/orx/run"}"#);
        } else {
            f.running("run").await;
        }
        f.store().accept_run_cancel("run", Some("stop")).unwrap();
        f.engine
            .observe("run", Observation::Lingering { pids: vec![42] })
            .unwrap();
        f.engine
            .observe(
                "run",
                Observation::Exited {
                    finished_at: None,
                    code: Some(137),
                    signal: None,
                },
            )
            .unwrap();
        assert_eq!(f.status("run"), "failed");
        let notices = f.deliver();
        assert_eq!(notices.len(), 2);
        assert!(notices
            .iter()
            .any(|notice| notice.body.contains("lingering processes [42]")));
        assert!(notices
            .iter()
            .all(|notice| !notice.body.contains("Cancelled")));
    }
}

#[tokio::test]
async fn nonzero_exit_after_abandon_keeps_cleanup_pending() {
    let mut f = Fixture::new();
    f.running("run").await;
    assert!(f.engine.abandon("run").unwrap());
    f.restart();
    f.engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(137),
                signal: None,
            },
        )
        .unwrap();

    assert_eq!(f.status("run"), "lost");
    assert!(unreconciled(&f.contact("run")));
}

#[tokio::test]
async fn an_unreconciled_run_keeps_its_since_across_a_restart() {
    let mut f = Fixture::new();
    f.running("run").await;
    let unverified = || Observation::Unreconciled {
        detail: "wrapper gone".into(),
    };
    f.engine.observe("run", unverified()).unwrap();
    let before = f.contact("run");
    assert!(unreconciled(&before));
    f.clock.advance(Duration::from_secs(60));
    f.restart();
    f.engine.observe("run", unverified()).unwrap();
    assert_eq!(f.contact("run"), before);
}

#[tokio::test]
async fn abandon_tells_the_chat_once_and_keeps_the_kill_pending_until_the_job_is_gone() {
    let f = Fixture::new();
    f.backend
        .then_launch(Err(LaunchError::Interrupted {
            detail: "lost".into(),
        }))
        .then_recover(Recovery::Unknown {
            detail: "host down".into(),
        });
    f.submit("unsubmitted").await.unwrap();
    f.deliver();
    assert!(f.engine.abandon("unsubmitted").unwrap());
    assert!(!f.engine.abandon("unsubmitted").unwrap());
    assert_eq!(f.status("unsubmitted"), "lost");
    let abandoned = f.store().get_run("unsubmitted").unwrap().unwrap();
    assert_eq!(
        abandoned.result_markdown.as_deref(),
        Some("Abandoned; the cancel stays pending until the job is confirmed gone.")
    );
    assert!(f
        .store()
        .terminal_notice_body(&abandoned)
        .unwrap()
        .contains("Reason: Abandoned; the cancel stays pending until the job is confirmed gone."));
    assert!(f.locked("unsubmitted"));
    f.clock.advance(Duration::from_secs(5));
    f.backend
        .then_recover(Recovery::Found(handle()))
        .then_cancel(CancelOutcome::Verified);
    f.engine.recover(&f.backend, "unsubmitted").await.unwrap();
    assert_eq!(f.contact("unsubmitted"), Contact::Live);
    f.released("unsubmitted");

    f.running("submitted").await;
    assert!(f.engine.abandon("submitted").unwrap());
    f.backend.then_cancel(CancelOutcome::Undelivered {
        detail: "environ unreadable".into(),
    });
    f.engine
        .deliver_cancel(&f.backend, "submitted")
        .await
        .unwrap();
    assert!(
        matches!(f.contact("submitted"), Contact::Unreconciled { detail, .. } if detail == super::reconcile::ABANDONED)
    );
    // Terminal evidence is not proof of absence: exits and failures keep the kill pending.
    for observation in [
        Observation::Running { started_at: None },
        Observation::Exited {
            finished_at: None,
            code: Some(0),
            signal: None,
        },
        Observation::Failed {
            finished_at: None,
            reason: "receipt unreadable".into(),
        },
        Observation::Lost {
            reason: "no receipt".into(),
        },
    ] {
        f.engine.observe("submitted", observation).unwrap();
        assert!(unreconciled(&f.contact("submitted")));
        assert!(f.locked("submitted"));
    }
    f.engine
        .observe(
            "submitted",
            Observation::Cancelled {
                finished_at: None,
                code: None,
                signal: None,
            },
        )
        .unwrap();
    let row = f.store().get_run("submitted").unwrap().unwrap();
    assert_eq!(row.status, "lost");
    assert_eq!(f.contact("submitted"), Contact::Live);
    f.released("submitted");

    let terminal: Vec<_> = f
        .deliver()
        .into_iter()
        .map(|notice| notice.run_id)
        .collect();
    assert_eq!(terminal, ["unsubmitted", "submitted"]);
}

#[tokio::test]
async fn runs_an_old_supervisor_holds_are_watched_and_never_written() {
    let f = Fixture::new();
    f.legacy_run("held", r#"{"kind":"ssh_job","jobId":"/orx/runs/held"}"#);
    let [supervisor, shared] = f.store().supervisor_lock_paths("held").unwrap();
    let old_supervisor = [supervisor, shared].map(|path| {
        let file = lock_file(&path);
        file.try_lock().unwrap();
        file
    });
    f.legacy_run(
        "ssh",
        r#"{"kind":"ssh_job","jobId":"/orx/runs/ssh","sshContainer":{"reference":"img","id":"c1","startedAt":"t","runDir":"/orx/runs/ssh"}}"#,
    );
    f.legacy_run(
        "box",
        r#"{"kind":"openresearch_job","namespace":"org","jobId":"sb"}"#,
    );
    f.legacy_run("hf", r#"{"kind":"hf_job","namespace":"lab","jobId":"j"}"#);
    f.legacy_run("mystery", r#"{"kind":"future_job","jobId":"x"}"#);
    f.running("engine").await;

    let mut decisions = f.engine.scan_legacy().unwrap();
    decisions.sort_by(|a, b| a.0.cmp(&b.0));
    let container = match &decisions[3].1 {
        Adoption::Inspect { container, .. } => container.clone(),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        decisions,
        [
            (
                "box".to_string(),
                Adoption::Native {
                    kind: "openresearch_job".into()
                }
            ),
            ("held".to_string(), Adoption::Watch),
            (
                "hf".to_string(),
                Adoption::Native {
                    kind: "hf_job".into()
                }
            ),
            (
                "ssh".to_string(),
                Adoption::Inspect {
                    dir: "/orx/runs/ssh".into(),
                    container
                }
            ),
        ]
    );
    assert!(f.locked("ssh") && f.locked("box") && f.locked("hf"));
    assert!(f
        .engine
        .scan_legacy()
        .unwrap()
        .iter()
        .all(|(id, _)| id == "held"));

    let before = f.store().get_run("held").unwrap().unwrap();
    f.engine
        .observe(
            "held",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    assert!(!f.engine.abandon("held").unwrap());
    f.store().request_cancel("held", "old CLI").unwrap();
    f.engine.deliver_cancel(&f.backend, "held").await.unwrap();
    let after = f.store().get_run("held").unwrap().unwrap();
    assert_eq!(after.status, before.status);
    assert!(f
        .store()
        .engine_run_state("held")
        .unwrap()
        .unwrap()
        .cancel_accepted_at
        .is_none());
    assert!(f.backend.calls().iter().all(|call| !call.contains("held")));

    // The old supervisor finishes the run; the engine only tells the chat.
    f.store()
        .update_status("held", crate::store::RunStatus::Done, Some(5), Some(0))
        .unwrap();
    drop(old_supervisor);
    f.released("held");
    f.engine.scan_legacy().unwrap();
    let notices = f.deliver();
    assert_eq!(notices.len(), 1);
    assert_eq!(
        (notices[0].run_id.as_str(), notices[0].kind.as_str()),
        ("held", TERMINAL)
    );
    f.engine.scan_legacy().unwrap();
    assert!(f.deliver().is_empty());
    f.released("held");
}

#[tokio::test]
async fn an_old_cli_cancel_is_accepted_once_under_its_lock_and_survives_withdrawal() {
    let f = Fixture::new();
    for id in ["pending", "verified"] {
        f.legacy_run(
            id,
            &format!(r#"{{"kind":"ssh_job","jobId":"/orx/runs/{id}"}}"#),
        );
    }
    f.engine.scan_legacy().unwrap();
    let old_cli = |id: &str, act: &dyn Fn(&Store)| {
        let path = f.dir.join(format!("run-logs/{id}.cancel.lock"));
        let mut lock = crate::store::open_lifecycle_lock_at(&path).unwrap();
        let _guard = lock.write().unwrap();
        act(&f.store());
    };

    old_cli("pending", &|store| {
        assert!(store.request_cancel("pending", "old CLI").unwrap());
    });
    f.backend.then_cancel(CancelOutcome::Pending);
    f.engine
        .deliver_cancel(&f.backend, "pending")
        .await
        .unwrap();
    // The old CLI's supervisor spawn failed, so it withdraws; the engine already started.
    old_cli("pending", &|store| {
        store.withdraw_cancel("pending").unwrap()
    });
    for _ in 0..3 {
        f.engine
            .deliver_cancel(&f.backend, "pending")
            .await
            .unwrap();
    }
    assert_eq!(f.backend.calls(), ["cancel pending Legacy"]);
    f.engine
        .observe(
            "pending",
            Observation::Cancelled {
                finished_at: None,
                code: None,
                signal: None,
            },
        )
        .unwrap();
    assert_eq!(f.status("pending"), "cancelled");

    old_cli("verified", &|store| {
        assert!(store.request_cancel("verified", "old CLI").unwrap());
    });
    f.backend.then_cancel(CancelOutcome::Verified);
    f.engine
        .deliver_cancel(&f.backend, "verified")
        .await
        .unwrap();
    let row = f.store().get_run("verified").unwrap().unwrap();
    assert_eq!(row.status, "cancelled");
    assert!(row.result_markdown.unwrap().contains("OR-469"));
}

#[tokio::test]
async fn reordered_feed_reports_never_move_a_phase_backwards() {
    let f = Fixture::new();
    f.running("run").await;
    f.running("other").await;
    f.backend.feed.lock().unwrap().extend([
        (
            "run".to_string(),
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        ),
        (
            "unwatched".to_string(),
            Observation::Running { started_at: None },
        ),
        ("run".to_string(), Observation::Running { started_at: None }),
        (
            "other".to_string(),
            Observation::Failed {
                finished_at: None,
                reason: "OOM".into(),
            },
        ),
        (
            "run".to_string(),
            Observation::Queued {
                detail: Some("stale".into()),
            },
        ),
    ]);
    let watched: Vec<_> = ["run", "other"]
        .iter()
        .map(|id| WatchedRun {
            id: id.to_string(),
            handle: handle_of(&f.store().get_run(id).unwrap().unwrap()).unwrap(),
        })
        .collect();
    let feed = f.backend.observe(&watched, Duration::from_secs(25)).await;
    assert_eq!(feed.len(), 4);
    for (id, observation) in feed {
        f.engine.observe(&id, observation).unwrap();
    }
    assert_eq!(f.status("run"), "done");
    let other = f.store().get_run("other").unwrap().unwrap();
    assert_eq!(
        (other.status.as_str(), other.result_markdown.as_deref()),
        ("failed", Some("OOM"))
    );
}

#[tokio::test]
async fn a_restart_between_fence_and_launch_recovers_from_the_persisted_intent() {
    let mut f = Fixture::new();
    let metadata = BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap();
    let store = f.store();
    store
        .insert_submitting_run(SubmittingRun {
            id: "run",
            experiment_id: "exp",
            project_id: "proj",
            metadata: &metadata,
            command: "train",
            created_at: 1,
            commit_sha: None,
            chat_session_id: Some(CHAT),
            spec_digest: "spec",
            launch: &ssh_launch(),
        })
        .unwrap();
    let fence = SubmissionFence {
        taken_at: 1,
        submitter: None,
    };
    assert!(store.set_submission_fence("run", 1, &fence).unwrap());

    f.restart();
    f.backend
        .then_recover(Recovery::Absent)
        .then_launch(Ok(handle()));
    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.backend.calls(), ["recover run #1", "launch run #2"]);
    assert_eq!(
        *f.backend.intents.lock().unwrap(),
        [ssh_launch(), ssh_launch()]
    );
    assert_eq!(f.status("run"), "starting");
}

#[tokio::test]
async fn notify_bytes_become_at_most_one_notice_per_ten_minutes_and_coalesce() {
    let mut f = Fixture::new();
    f.running("run").await;
    let write = |f: &Fixture, text: &str| {
        let mut notify = f.backend.notify.lock().unwrap();
        notify.bytes.extend_from_slice(text.as_bytes());
        let len = notify.bytes.len() as u64;
        drop(notify);
        f.engine
            .observe("run", Observation::Notify { len })
            .unwrap();
    };

    write(&f, "epoch 1: loss 0.9");
    assert_eq!(f.engine.notify_due().unwrap(), ["run"]);
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    write(&f, "\nepoch 2: loss 0.5");
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    f.clock.advance(Duration::from_secs(9 * 60));
    write(&f, "\nepoch 3: loss 0.2");
    assert!(f.engine.notify_due().unwrap().is_empty());
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    let first = f.deliver();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].kind, NOTIFY);
    assert!(first[0].body.ends_with("epoch 1: loss 0.9"));

    f.clock.advance(Duration::from_secs(60));
    assert_eq!(f.engine.notify_due().unwrap(), ["run"]);
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    let coalesced = f.deliver();
    assert_eq!(coalesced.len(), 1);
    assert!(coalesced[0]
        .body
        .ends_with("wrote to its notify file:\n\nepoch 2: loss 0.5\nepoch 3: loss 0.2"));

    // The cursor is durable: a restarted engine re-reported the same length sends nothing.
    f.restart();
    f.clock.advance(Duration::from_secs(10 * 60));
    let len = f.backend.notify.lock().unwrap().bytes.len() as u64;
    f.engine
        .observe("run", Observation::Notify { len })
        .unwrap();
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    assert!(f.engine.notify_due().unwrap().is_empty());

    // A replaced file starts over, and only its tail is quoted.
    let long = "x".repeat(5000) + "tail";
    *f.backend.notify.lock().unwrap() = HostFile {
        bytes: Vec::new(),
        ino: 1,
    };
    write(&f, &long);
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    let restarted = f.deliver();
    assert_eq!(restarted.len(), 1);
    assert!(restarted[0]
        .body
        .contains("tail\n(908 earlier bytes omitted; see the file.)"));

    // A same-size replacement and a truncation below the cursor are both new files.
    let replaced = "y".repeat(5000) + "tail";
    f.clock.advance(Duration::from_secs(10 * 60));
    *f.backend.notify.lock().unwrap() = HostFile {
        bytes: Vec::new(),
        ino: 2,
    };
    write(&f, &replaced);
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    let same_size = f.deliver();
    assert_eq!(same_size.len(), 1);
    assert!(same_size[0].body.contains("yyyytail"));
    assert_ne!(same_size[0].dedupe, restarted[0].dedupe);

    f.clock.advance(Duration::from_secs(10 * 60));
    *f.backend.notify.lock().unwrap() = HostFile {
        bytes: Vec::new(),
        ino: 3,
    };
    write(&f, "short");
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    let truncated = f.deliver();
    assert_eq!(truncated.len(), 1);
    assert!(truncated[0].body.ends_with("notify file:\nshort"));
    assert!(f.deliver().is_empty());
}

#[tokio::test]
async fn concurrent_notify_deliveries_send_one_notice() {
    let f = Fixture::new();
    f.running("run").await;
    f.backend.notify.lock().unwrap().bytes = b"done".to_vec();
    f.engine
        .observe("run", Observation::Notify { len: 4 })
        .unwrap();
    let (a, b) = tokio::join!(
        f.engine.deliver_notify(&f.backend, "run"),
        f.engine.deliver_notify(&f.backend, "run"),
    );
    a.unwrap();
    b.unwrap();
    f.backend
        .notify
        .lock()
        .unwrap()
        .bytes
        .extend_from_slice(b" again");
    f.engine
        .observe("run", Observation::Notify { len: 10 })
        .unwrap();
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    assert_eq!(f.deliver().len(), 1);
    let cursor = f
        .store()
        .engine_run_state("run")
        .unwrap()
        .unwrap()
        .notify_cursor;
    assert_eq!(cursor.unwrap().offset, 4);
}

#[tokio::test]
async fn concurrent_recoveries_never_act_on_a_stale_attempt() {
    let f = Fixture::new();
    f.backend
        .then_launch(Err(LaunchError::Interrupted {
            detail: "lost".into(),
        }))
        .then_recover(Recovery::Unknown { detail: "?".into() });
    f.submit("run").await.unwrap();
    f.clock.advance(Duration::from_secs(5));
    let dir = f.dir.clone();
    *f.backend.during_launch.lock().unwrap() = Some(Box::new(move || {
        let store = Store::open_at(dir).unwrap();
        store
            .accept_run_cancel("run", Some("user"))
            .unwrap()
            .unwrap();
    }));
    f.backend
        .then_recover(Recovery::Absent)
        .then_recover(Recovery::Absent)
        .then_launch(Ok(handle()))
        .then_cancel(CancelOutcome::Pending);
    let (a, b) = tokio::join!(
        f.engine.recover(&f.backend, "run"),
        f.engine.recover(&f.backend, "run"),
    );
    a.unwrap();
    b.unwrap();
    assert_eq!(f.status("run"), "starting");
    assert!(handle_of(&f.store().get_run("run").unwrap().unwrap()).is_some());
    assert_eq!(
        f.backend.calls(),
        [
            "launch run #1",
            "recover run #1",
            "recover run #1",
            "launch run #2",
            "cancel run Ssh"
        ]
    );
}

#[tokio::test]
async fn a_dropped_launch_leaves_the_run_recoverable() {
    let f = Fixture::new();
    f.backend
        .hang_launch
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let dropped = tokio::time::timeout(Duration::from_millis(50), f.submit("run")).await;
    assert!(dropped.is_err());
    assert_eq!(f.status("run"), "submitting");
    f.backend.then_recover(Recovery::Found(handle()));
    f.engine.recover(&f.backend, "run").await.unwrap();
    assert_eq!(f.status("run"), "starting");
}

#[test]
fn legacy_adoption_waits_for_the_launch_to_settle() {
    let f = Fixture::new();
    f.legacy_run("recent", r#"{"kind":"local_job","jobId":"job"}"#);
    let mut run = f.store().get_run("recent").unwrap().unwrap();
    run.updated_at = f.clock.wall_ms();
    f.store().upsert_run(&run).unwrap();
    assert_eq!(
        f.engine.scan_legacy().unwrap(),
        [("recent".into(), Adoption::Watch)]
    );
    assert!(!f.engine.holds("recent"));
    f.clock.advance(Duration::from_millis(
        u64::try_from(ORPHANED_RUN_SETTLE_MS).unwrap() - 1,
    ));
    assert_eq!(
        f.engine.scan_legacy().unwrap(),
        [("recent".into(), Adoption::Watch)]
    );
    assert!(!f.engine.holds("recent"));
    f.clock.advance(Duration::from_millis(1));
    assert_eq!(
        f.engine.scan_legacy().unwrap(),
        [(
            "recent".into(),
            Adoption::Native {
                kind: "local_job".into()
            }
        )]
    );
    assert!(f.engine.holds("recent"));
    assert!(f.engine.scan_legacy().unwrap().is_empty());
    assert!(f.engine.holds("recent"));
}

#[test]
fn a_legacy_launch_without_a_handle_takes_its_recovery_file_or_fails_once_settled() {
    let f = Fixture::new();
    f.legacy_run("lost", r#"{"kind":"modal_job"}"#);
    f.legacy_run("kept", r#"{"kind":"local_job"}"#);
    f.legacy_run("recent", r#"{"kind":"hf_job"}"#);
    f.legacy_run("unreadable", r#"{"kind":"modal_job"}"#);
    let handles = f.dir.join("submission-handles");
    std::fs::create_dir_all(handles.join("unreadable.json")).unwrap();
    std::fs::write(
        handles.join("kept.json"),
        r#"{"kind":"local_job","jobId":"/runs/kept"}"#,
    )
    .unwrap();
    let mut recent = f.store().get_run("recent").unwrap().unwrap();
    recent.updated_at = f.clock.wall_ms();
    f.store().upsert_run(&recent).unwrap();

    f.engine.scan_legacy().unwrap();
    let lost = f.store().get_run("lost").unwrap().unwrap();
    assert_eq!(lost.status, "failed");
    assert!(lost
        .result_markdown
        .unwrap()
        .contains("Inspect the provider for resources labelled or_run=lost"));
    let kept = f.store().get_run("kept").unwrap().unwrap();
    assert_eq!(kept.status, "running");
    assert_eq!(
        BackendDescriptor::parse(&kept.backend_json)
            .unwrap()
            .job_id
            .as_deref(),
        Some("/runs/kept")
    );
    assert_eq!(f.status("recent"), "running");
    let unreadable = f.store().get_run("unreadable").unwrap().unwrap();
    assert_eq!(unreadable.status, "failed");
    assert!(unreadable
        .result_markdown
        .unwrap()
        .contains("recovery file that could not be read"));
    assert!(!f.engine.holds("lost") && !f.engine.holds("kept"));
    let mut notices: Vec<_> = f
        .deliver()
        .into_iter()
        .map(|notice| (notice.run_id, notice.kind))
        .collect();
    notices.sort();
    assert_eq!(
        notices,
        [
            ("lost".to_string(), TERMINAL.to_string()),
            ("unreadable".to_string(), TERMINAL.to_string())
        ]
    );

    f.engine.scan_legacy().unwrap();
    assert!(f.deliver().is_empty());
    assert_eq!(f.status("recent"), "running");
}

#[tokio::test]
async fn an_old_supervisor_finishing_while_the_engine_is_down_still_reaches_the_chat() {
    let mut f = Fixture::new();
    f.legacy_run("held", r#"{"kind":"ssh_job","jobId":"/orx/runs/held"}"#);
    let old_supervisor = f
        .store()
        .supervisor_lock_paths("held")
        .unwrap()
        .map(|path| {
            let file = lock_file(&path);
            file.try_lock().unwrap();
            file
        });
    assert_eq!(
        f.engine.scan_legacy().unwrap(),
        [("held".to_string(), Adoption::Watch)]
    );

    f.restart();
    f.store()
        .update_status("held", crate::store::RunStatus::Failed, Some(5), Some(1))
        .unwrap();
    drop(old_supervisor);
    f.released("held");
    assert!(f.engine.scan_legacy().unwrap().is_empty());
    let notices = f.deliver();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].body.contains("**failed**"));
    f.restart();
    f.engine.scan_legacy().unwrap();
    assert!(f.deliver().is_empty());
}

#[tokio::test]
async fn a_supervisor_finishing_between_snapshot_and_adoption_still_reaches_the_chat() {
    let f = Fixture::new();
    f.legacy_run("held", r#"{"kind":"ssh_job","jobId":"/orx/runs/held"}"#);
    let old_supervisor = f
        .store()
        .supervisor_lock_paths("held")
        .unwrap()
        .map(|path| {
            let file = lock_file(&path);
            file.try_lock().unwrap();
            file
        });
    assert_eq!(
        f.engine.scan_legacy().unwrap(),
        [("held".to_string(), Adoption::Watch)]
    );

    let dir = f.dir.clone();
    let paths = f.store().supervisor_lock_paths("held").unwrap();
    f.engine.memory().after_legacy_snapshot = Some(Box::new(move || {
        Store::open_at(dir)
            .unwrap()
            .update_status("held", crate::store::RunStatus::Done, Some(5), Some(0))
            .unwrap();
        drop(old_supervisor);
        released(&paths);
    }));
    assert!(f.engine.scan_legacy().unwrap().is_empty());
    let notices = f.deliver();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].body.contains("**done**"));
    assert!(f.store().legacy_watches().unwrap().is_empty());
    f.released("held");
    f.engine.scan_legacy().unwrap();
    assert!(f.deliver().is_empty());
}

/// `echo msg > $ORX_NOTIFY_FILE` rewrites the file under the same inode: the shorter file is
/// read from its start, never as a tail of the old one, and nothing stays pending.
#[tokio::test]
async fn a_shorter_rewrite_in_place_is_read_from_its_start() {
    let f = Fixture::new();
    f.running("run").await;
    for text in ["epoch 1: loss 0.9\n", "done\n"] {
        f.backend.notify.lock().unwrap().bytes = text.as_bytes().to_vec();
        let len = text.len() as u64;
        f.engine
            .observe("run", Observation::Notify { len })
            .unwrap();
        f.engine.deliver_notify(&f.backend, "run").await.unwrap();
        assert!(f.engine.notify_due().unwrap().is_empty());
        f.clock.advance(Duration::from_secs(10 * 60));
    }
    let bodies: Vec<_> = f.deliver().into_iter().map(|notice| notice.body).collect();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].ends_with("notify file:\ndone\n"), "{}", bodies[1]);
}

#[tokio::test]
async fn a_truncation_after_a_long_report_is_sent_once() {
    let f = Fixture::new();
    f.running("run").await;
    f.backend.notify.lock().unwrap().bytes = vec![b'x'; 5004];
    f.engine
        .observe("run", Observation::Notify { len: 5004 })
        .unwrap();
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    f.clock.advance(Duration::from_secs(10 * 60));
    f.engine
        .observe("run", Observation::Notify { len: 5004 })
        .unwrap();
    // Shrinks after the report and before the read.
    f.backend.notify.lock().unwrap().bytes = b"short".to_vec();
    f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    for _ in 0..3 {
        f.clock.advance(Duration::from_secs(10 * 60));
        assert!(f.engine.notify_due().unwrap().is_empty());
        f.engine.deliver_notify(&f.backend, "run").await.unwrap();
    }
    let bodies: Vec<_> = f.deliver().into_iter().map(|notice| notice.body).collect();
    assert_eq!(bodies.len(), 2);
    assert!(bodies[1].ends_with("notify file:\nshort"));
}

#[tokio::test]
async fn a_cancel_without_acknowledgement_can_be_delivered_again() {
    let f = Fixture::new();
    f.running("run").await;
    f.backend.then_cancel(CancelOutcome::Undelivered {
        detail: "scheduler unavailable".into(),
    });
    f.engine.cancel(&f.backend, "run", "user").await.unwrap();
    assert_eq!(f.status("run"), "running");
    assert_eq!(f.contact("run"), Contact::Live);
    f.backend.then_cancel(CancelOutcome::Verified);
    f.engine.deliver_cancel(&f.backend, "run").await.unwrap();
    assert_eq!(f.status("run"), "cancelled");
    assert_eq!(
        f.backend
            .calls()
            .iter()
            .filter(|call| call.starts_with("cancel"))
            .count(),
        2
    );
}

#[tokio::test]
async fn observing_a_run_without_cancel_intent_does_not_take_the_cancel_lock() {
    let f = Fixture::new();
    f.running("run").await;
    let path = f.dir.join("run-logs/run.cancel.lock");
    let (ready, locked) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let mut lock = crate::store::open_lifecycle_lock_at(&path).unwrap();
        let _guard = lock.write().unwrap();
        ready.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(1));
    });
    locked.recv().unwrap();
    let began = std::time::Instant::now();
    f.engine
        .observe_runs(
            &f.backend,
            &[WatchedRun {
                id: "run".into(),
                handle: handle(),
            }],
            Duration::ZERO,
        )
        .await
        .unwrap();
    let elapsed = began.elapsed();
    let _ = release.send(());
    holder.join().unwrap();
    assert!(
        elapsed < Duration::from_millis(500),
        "cancel lock blocked observation for {elapsed:?}"
    );
}

#[tokio::test]
async fn launch_blocked_before_send_keeps_contact_and_retries_without_recovery() {
    for contact in [
        Contact::NeedsAuth {
            since: 1,
            cause: "Permission denied (publickey)".into(),
        },
        Contact::Unreachable {
            since: 1,
            cause: "Connection refused".into(),
        },
    ] {
        let mut f = Fixture::new();
        f.backend.then_launch(Err(LaunchError::Blocked {
            contact: contact.clone(),
        }));
        f.submit("blocked").await.unwrap();
        assert_eq!(f.status("blocked"), "submitting");
        assert_eq!(f.contact("blocked"), contact);
        assert!(f
            .store()
            .engine_run_state("blocked")
            .unwrap()
            .unwrap()
            .submission_fence
            .is_none());
        assert_eq!(f.backend.calls(), ["launch blocked #1"]);
        f.restart();
        f.backend.then_launch(Ok(handle()));
        f.engine.recover(&f.backend, "blocked").await.unwrap();
        assert_eq!(f.status("blocked"), "starting");
        assert_eq!(f.contact("blocked"), Contact::Live);
        assert_eq!(
            f.backend.calls(),
            ["launch blocked #1", "launch blocked #1"]
        );
    }
}

#[tokio::test]
async fn cancelling_a_launch_blocked_before_send_needs_no_backend_confirmation() {
    let f = Fixture::new();
    f.backend.then_launch(Err(LaunchError::Blocked {
        contact: Contact::NeedsAuth {
            since: 1,
            cause: "Permission denied (publickey)".into(),
        },
    }));
    f.submit("blocked").await.unwrap();
    f.engine
        .cancel(&f.backend, "blocked", "user")
        .await
        .unwrap();
    assert_eq!(f.status("blocked"), "cancelled");
    assert_eq!(f.backend.calls(), ["launch blocked #1"]);
}

#[tokio::test]
async fn final_notify_survives_a_restart_inside_the_rate_window() {
    let mut f = Fixture::new();
    f.running("run").await;
    f.backend.notify.lock().unwrap().bytes = b"epoch 1\n".to_vec();
    f.engine
        .observe("run", Observation::Notify { len: 8 })
        .unwrap();
    assert!(f.engine.deliver_notify(&f.backend, "run").await.unwrap());
    assert_eq!(f.kinds(), [NOTIFY]);
    f.backend
        .notify
        .lock()
        .unwrap()
        .bytes
        .extend_from_slice(b"final score 0.93\n");
    f.engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    assert!(f.store().log_collection_pending("run").unwrap());
    f.restart();
    assert!(f.engine.memory().notify.is_empty());
    assert!(f.engine.deliver_notify(&f.backend, "run").await.unwrap());
    let notices = f.deliver();
    assert_eq!(
        notices
            .iter()
            .filter(|notice| notice.kind == TERMINAL)
            .count(),
        1
    );
    let final_notice = notices.iter().find(|notice| notice.kind == NOTIFY).unwrap();
    assert!(final_notice.body.ends_with("final score 0.93\n"));
    assert!(f.engine.deliver_notify(&f.backend, "run").await.unwrap());
    assert!(f.deliver().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn a_submitter_without_a_readable_identity_cannot_cross_the_fence() {
    let f = Fixture::new();
    let metadata = BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap();
    f.engine
        .insert(SubmittingRun {
            id: "run",
            experiment_id: "exp",
            project_id: "proj",
            metadata: &metadata,
            command: "train",
            created_at: 1,
            commit_sha: None,
            chat_session_id: None,
            spec_digest: "spec",
            launch: &ssh_launch(),
        })
        .unwrap();
    let fence = SubmissionFence {
        taken_at: 1,
        submitter: None,
    };
    f.store().set_submission_fence("run", 1, &fence).unwrap();
    let fence = Fence {
        store: &f.engine.store,
        run_id: "run",
        attempt: 1,
        taken_at: 1,
    };
    assert!(fence.record_submitter(u32::MAX).is_err());
    assert!(f
        .store()
        .engine_run_state("run")
        .unwrap()
        .unwrap()
        .submission_fence
        .unwrap()
        .submitter
        .is_none());
}

#[tokio::test]
async fn running_records_its_first_source_or_observation_time_durably() {
    let mut f = Fixture::new();
    for (id, source) in [("source", Some(1234)), ("fallback", None)] {
        f.backend.then_launch(Ok(handle()));
        f.submit(id).await.unwrap();
        let expected = source.unwrap_or_else(|| f.clock.wall_ms());
        f.engine
            .observe(id, Observation::Running { started_at: source })
            .unwrap();
        assert_eq!(
            f.store().engine_run_state(id).unwrap().unwrap().started_at,
            Some(expected)
        );
        f.clock.advance(Duration::from_secs(10));
        f.engine
            .observe(
                id,
                Observation::Running {
                    started_at: Some(expected + 5000),
                },
            )
            .unwrap();
        assert_eq!(
            f.store().engine_run_state(id).unwrap().unwrap().started_at,
            Some(expected)
        );
        f.restart();
        assert_eq!(
            f.store().engine_run_state(id).unwrap().unwrap().started_at,
            Some(expected)
        );
    }
    f.legacy_run("legacy", r#"{"kind":"local_job","jobId":"job"}"#);
    f.engine
        .observe(
            "legacy",
            Observation::Running {
                started_at: Some(1234),
            },
        )
        .unwrap();
    assert_eq!(
        f.store()
            .engine_run_state("legacy")
            .unwrap()
            .unwrap()
            .started_at,
        None
    );
}

#[tokio::test]
async fn receipt_time_uses_the_source_clock_and_falls_back_to_observation_time() {
    let f = Fixture::new();
    let now = f.clock.wall_ms();
    for (id, finished_at, expected) in [
        ("dated", Some(1234), 1234),
        ("old", None, now),
        ("past", Some(-100), 1),
        ("future", Some(now + 1000), now),
    ] {
        f.running(id).await;
        f.engine
            .observe(
                id,
                Observation::Exited {
                    code: Some(0),
                    signal: None,
                    finished_at,
                },
            )
            .unwrap();
        assert_eq!(
            f.store().get_run(id).unwrap().unwrap().ended_at,
            Some(expected)
        );
    }
    f.running("provider-failed").await;
    f.engine
        .observe(
            "provider-failed",
            Observation::Failed {
                finished_at: Some(5678),
                reason: "provider failure".into(),
            },
        )
        .unwrap();
    assert_eq!(
        f.store()
            .get_run("provider-failed")
            .unwrap()
            .unwrap()
            .ended_at,
        Some(5678)
    );
    f.running("abandoned").await;
    f.engine.abandon("abandoned").unwrap();
    f.engine
        .observe(
            "abandoned",
            Observation::CancelReceipt {
                finished_at: Some(2468),
                code: Some(0),
                remaining: 0,
                note: None,
            },
        )
        .unwrap();
    assert_eq!(
        f.store().get_run("abandoned").unwrap().unwrap().ended_at,
        Some(2468)
    );
    f.running("rollback").await;
    f.clock.jump_wall(100 - now);
    f.engine
        .observe(
            "rollback",
            Observation::Exited {
                code: Some(0),
                signal: None,
                finished_at: Some(-100),
            },
        )
        .unwrap();
    assert_eq!(
        f.store().get_run("rollback").unwrap().unwrap().ended_at,
        Some(1)
    );
}

#[tokio::test]
async fn vanished_job_is_lost_unless_cancellation_was_requested() {
    let f = Fixture::new();
    for (id, cancel) in [("expired", false), ("deleted", true)] {
        f.running(id).await;
        if cancel {
            f.backend.then_cancel(CancelOutcome::Pending);
            f.engine.cancel(&f.backend, id, "test").await.unwrap();
        }
        f.engine
            .observe(
                id,
                Observation::Gone {
                    reason: "Job deleted before its outcome was recorded".into(),
                },
            )
            .unwrap();
        assert_eq!(f.status(id), if cancel { "cancelled" } else { "lost" });
        if !cancel {
            assert!(f
                .store()
                .get_run(id)
                .unwrap()
                .unwrap()
                .result_markdown
                .unwrap()
                .contains("Job deleted"));
        }
    }
}

#[tokio::test]
async fn cancelled_survivors_settle_once_after_a_restart_and_only_notify_affected_chats() {
    let mut f = Fixture::new();
    f.running("survivors").await;
    let proof = |remaining| Observation::CancelReceipt {
        finished_at: Some(1234),
        code: None,
        remaining,
        note: None,
    };
    f.engine.observe("survivors", proof(2)).unwrap();
    assert_eq!(f.status("survivors"), "cancelled");
    assert_eq!(
        f.store().get_run("survivors").unwrap().unwrap().ended_at,
        Some(1234)
    );
    assert!(matches!(
        f.contact("survivors"),
        Contact::Stopping { count: 2, .. }
    ));
    let terminal = f.deliver();
    assert_eq!(terminal.len(), 1);
    let known = f.contact("survivors");
    for observation in [
        Observation::Unreachable {
            cause: "eof".into(),
        },
        Observation::NeedsAuth {
            cause: "MFA".into(),
        },
    ] {
        f.engine.observe("survivors", observation).unwrap();
        assert_eq!(f.contact("survivors"), known);
    }

    assert!(terminal[0]
        .body
        .contains("Not fully stopped · 2 processes still running on lab"));
    f.store().begin().unwrap().execute(
        "UPDATE run_notices SET body = 'Earlier cancellation wording' WHERE run_id = 'survivors' AND kind = ?1",
        [TERMINAL],
    ).unwrap();
    f.store()
        .subscribe_run("survivors", "late-subscriber", 2)
        .unwrap();
    f.restart();
    f.engine.observe("survivors", proof(1)).unwrap();
    assert!(f.deliver().is_empty());
    f.engine.observe("survivors", proof(0)).unwrap();
    assert_eq!(f.contact("survivors"), Contact::Live);
    assert_eq!(
        f.store().get_run("survivors").unwrap().unwrap().ended_at,
        Some(1234)
    );
    let followup = f.deliver();
    assert_eq!(followup.len(), 1);
    assert_eq!(followup[0].dedupe, "cancel-stopped");
    assert!(f
        .store()
        .run_notice_batch("late-subscriber")
        .unwrap()
        .is_empty());
    assert_eq!(
        followup[0].body,
        "[orx] Run `survivors`: Cancelled. All remaining processes have stopped."
    );
    f.engine.observe("survivors", proof(0)).unwrap();
    f.restart();
    f.engine.observe("survivors", proof(0)).unwrap();
    assert!(f.deliver().is_empty());

    f.running("empty").await;
    f.engine.observe("empty", proof(0)).unwrap();
    assert_eq!(f.deliver().len(), 1);
    f.engine
        .observe("empty", Observation::Lingering { pids: vec![] })
        .unwrap();
    assert!(f.deliver().is_empty());
}

#[tokio::test]
async fn cancelled_without_containment_proof_is_live_with_an_explicit_note() {
    let mut f = Fixture::new();
    for (id, unreadable, note) in [
        (
            "container",
            vec![],
            "In-container processes or processes outside the host scan couldn't be checked.",
        ),
        (
            "mac",
            vec![789],
            "The host couldn't inspect processes 789 to confirm the stop.",
        ),
    ] {
        f.running(id).await;
        f.engine
            .observe(
                id,
                Observation::CancelReceipt {
                    finished_at: None,
                    code: None,
                    remaining: 0,
                    note: Some(unreadable.clone()),
                },
            )
            .unwrap();
        assert_eq!(f.status(id), "cancelled");
        assert_eq!(f.contact(id), Contact::Live);
        assert_eq!(
            f.store()
                .get_run(id)
                .unwrap()
                .unwrap()
                .result_markdown
                .as_deref(),
            Some(note)
        );
        assert_eq!(
            f.store()
                .engine_run_state(id)
                .unwrap()
                .unwrap()
                .cancel_unconfirmed,
            Some(unreadable)
        );
        assert!(!f
            .store()
            .list_engine_runs()
            .unwrap()
            .iter()
            .any(|run| run.id == id));
        f.restart();
        let run = f.store().get_run(id).unwrap().unwrap();
        assert_eq!(
            crate::plane::Run::from_store(&f.store(), &run)
                .unwrap()
                .display_status(),
            "Cancelled · couldn't confirm"
        );
        let notices = f.deliver();
        assert_eq!(notices.len(), 1);
        assert!(notices[0]
            .body
            .contains("status **Cancelled · couldn't confirm**"));
        assert!(notices[0].body.contains(note));
    }
}

#[tokio::test]
async fn emptied_survivors_without_proof_do_not_claim_a_verified_stop_in_the_followup() {
    let mut f = Fixture::new();
    f.running("opaque").await;
    f.engine
        .observe(
            "opaque",
            Observation::CancelReceipt {
                finished_at: Some(10),
                code: None,
                remaining: 2,
                note: None,
            },
        )
        .unwrap();
    assert_eq!(f.deliver().len(), 1);
    f.restart();
    let uncertain = || Observation::CancelReceipt {
        finished_at: Some(10),
        code: None,
        remaining: 0,
        note: Some(vec![]),
    };
    f.engine.observe("opaque", uncertain()).unwrap();
    assert_eq!(f.contact("opaque"), Contact::Live);
    let notices = f.deliver();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].body, "[orx] Run `opaque`: Cancelled · couldn't confirm. In-container processes or processes outside the host scan couldn't be checked.");
    f.restart();
    f.engine.observe("opaque", uncertain()).unwrap();
    assert!(f.deliver().is_empty());
}

#[tokio::test]
async fn supervised_monitoring_outages_and_recoveries_are_delivered_once() {
    let mut f = Fixture::new();
    f.legacy_run("run", r#"{"kind":"ssh_job","jobId":"/orx/run"}"#);
    let _old_supervisor = f.store().supervisor_lock_paths("run").unwrap().map(|path| {
        let file = lock_file(&path);
        file.try_lock().unwrap();
        file
    });
    f.engine.scan_legacy().unwrap();
    assert!(f.deliver().is_empty());
    for _ in 0..2 {
        let mut run = f.store().get_run("run").unwrap().unwrap();
        let mut descriptor = BackendDescriptor::parse(&run.backend_json).unwrap();
        descriptor.monitoring_error = Some("Host unreachable".into());
        run.backend_json = descriptor.to_json();
        f.store().upsert_run(&run).unwrap();
        f.engine.scan_legacy().unwrap();
        f.engine.scan_legacy().unwrap();
        let notices = f.deliver();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].kind, ATTENTION);
        assert!(notices[0].body.contains("orx can no longer monitor"));
        f.restart();
        f.engine.scan_legacy().unwrap();
        assert!(f.deliver().is_empty());
        descriptor.monitoring_error = None;
        run.backend_json = descriptor.to_json();
        f.store().upsert_run(&run).unwrap();
        f.engine.scan_legacy().unwrap();
        f.engine.scan_legacy().unwrap();
        assert_eq!(f.kinds(), [RECOVERED]);
        f.restart();
        f.engine.scan_legacy().unwrap();
        assert!(f.deliver().is_empty());
    }
}

#[tokio::test]
async fn tombstone_finish_time_does_not_stage_terminal_effects_again() {
    let f = Fixture::new();
    f.running("run").await;
    f.store()
        .reserve_run_telemetry(
            "run",
            None,
            Some(&(
                "terminal".into(),
                serde_json::json!({
                    "events": [{"eventId": "terminal", "properties": {}}]
                }),
            )),
        )
        .unwrap();
    f.engine.abandon("run").unwrap();
    assert_eq!(f.store().pending_telemetry().unwrap().len(), 1);
    f.store().acknowledge_pending_telemetry("terminal").unwrap();
    let updated_at = f.store().get_run("run").unwrap().unwrap().updated_at;
    let now = f.clock.wall_ms();
    for (remaining, finished_at, expected) in [(2, now + 1000, now), (1, -100, 1), (0, 1234, 1234)]
    {
        f.engine
            .observe(
                "run",
                Observation::CancelReceipt {
                    finished_at: Some(finished_at),
                    code: Some(137),
                    remaining,
                    note: None,
                },
            )
            .unwrap();
        let run = f.store().get_run("run").unwrap().unwrap();
        assert_eq!(run.ended_at, Some(expected));
        assert_eq!(run.exit_code, Some(137));
        assert_eq!(run.updated_at, updated_at);
        assert!(f.store().pending_telemetry().unwrap().is_empty());
    }
}

#[tokio::test]
async fn a_starting_run_shows_the_backends_latest_waiting_reason_until_it_runs() {
    let mut f = Fixture::new();
    f.backend.then_launch(Ok(handle()));
    f.submit("run").await.unwrap();
    f.events();
    let waiting = |f: &Fixture| {
        f.store()
            .engine_run_state("run")
            .unwrap()
            .unwrap()
            .waiting_reason
    };
    let queued = |detail: &str| Observation::Queued {
        detail: Some(detail.into()),
    };
    let unscheduled = "0/4 nodes are available: 4 Insufficient nvidia.com/gpu.";

    f.engine
        .observe(
            "run",
            queued("0/4 nodes are available:\n  4 Insufficient nvidia.com/gpu."),
        )
        .unwrap();
    assert_eq!(waiting(&f).as_deref(), Some(unscheduled));
    assert_eq!(f.phases(), [Phase::Starting]);

    // Each poll repeats the reason; only a new one is written and published.
    let observer = rusqlite::Connection::open(f.dir.join("orx.db")).unwrap();
    let version = || -> i64 {
        observer
            .query_row("PRAGMA data_version", [], |row| row.get(0))
            .unwrap()
    };
    let before = version();
    f.engine.observe("run", queued(unscheduled)).unwrap();
    assert_eq!(version(), before);
    assert!(f.events().is_empty());

    f.engine
        .observe(
            "run",
            Observation::Unreachable {
                cause: "eof".into(),
            },
        )
        .unwrap();
    assert_eq!(waiting(&f).as_deref(), Some(unscheduled));

    f.engine
        .observe("run", Observation::Queued { detail: None })
        .unwrap();
    assert_eq!(waiting(&f), None);

    f.engine.observe("run", queued(&"x".repeat(2000))).unwrap();
    let long = waiting(&f).unwrap();
    assert!(long.chars().count() <= 500 && long.ends_with('…'));

    f.engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    assert_eq!(waiting(&f), None);
    f.engine.observe("run", queued("late")).unwrap();
    assert_eq!(waiting(&f), None);
    assert_eq!(f.status("run"), "running");
}

#[tokio::test]
async fn an_ended_run_drops_its_waiting_reason() {
    let f = Fixture::new();
    f.backend.then_launch(Ok(handle()));
    f.submit("run").await.unwrap();
    f.engine
        .observe(
            "run",
            Observation::Queued {
                detail: Some("Provisioning box".into()),
            },
        )
        .unwrap();
    f.engine
        .observe(
            "run",
            Observation::Failed {
                finished_at: None,
                reason: "Box provisioning failed".into(),
            },
        )
        .unwrap();
    assert_eq!(f.status("run"), "failed");
    let stored: Option<String> = rusqlite::Connection::open(f.dir.join("orx.db"))
        .unwrap()
        .query_row(
            "SELECT waiting_reason FROM runs WHERE id = 'run'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, None);
}
