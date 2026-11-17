use std::sync::Mutex as SyncMutex;

use async_trait::async_trait;
use tokio::io::AsyncWriteExt as _;

use super::super::{DeliveryState, WirePart};
use super::*;
use crate::local::harness::{TurnFailure, TurnOutcome, TurnResult, TEST_HARNESSES};
use crate::store::StoredChatSession;

/// What each fake harness was handed back, in order.
static ADOPTED: SyncMutex<Vec<Adoption>> = SyncMutex::new(Vec::new());

/// Resumes a turn on a kept `cat`: each line it echoes is a text part, and once it has
/// resumed it writes `done`, which ends the turn. Refuses an agent whose state says so.
struct Resumes;

#[async_trait]
impl Harness for Resumes {
    fn id(&self) -> &str {
        "test-resume"
    }

    fn name(&self) -> &str {
        "Resumes"
    }

    fn supports_resume(&self) -> bool {
        true
    }

    async fn adopt(&self, _chat: &Arc<ChatHost>, adoption: Adoption) -> Result<()> {
        let refuse = adoption.state["refuse"] == true;
        ADOPTED.lock().unwrap().push(adoption);
        if refuse {
            bail!("this agent is refused");
        }
        Ok(())
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        let failure = |message: String| TurnFailure {
            kind: "test",
            message,
            delivery: DeliveryState::Accepted,
        };
        let Some(resume) = ctx.resume.clone() else {
            return Err(failure("the prompt was sent again".into()));
        };
        let mut kept = keep::attach(&resume.keep_id)
            .await
            .map_err(|error| failure(format!("{error:#}")))?;
        let handle = kept.handle();
        let mut stdin = kept.take_stdin().unwrap();
        let mut lines = kept.take_stdout().unwrap().lines();
        lines.skip_to(resume.offset).unwrap();
        let mut seen = resume.state["seen"].as_u64().unwrap();
        stdin.write_all(b"done\n").await.unwrap();
        stdin.flush().await.unwrap();
        loop {
            let (line, offset) = lines
                .next_line()
                .await
                .unwrap()
                .ok_or_else(|| failure("the agent's output ended".into()))?;
            if line == "done" {
                return Ok(TurnOutcome::Completed);
            }
            seen += 1;
            ctx.upsert_part(WirePart::text(&line, &line));
            ctx.flush_at(&handle, offset, &json!({ "seen": seen }))
                .map_err(|error| failure(format!("{error:#}")))?;
        }
    }
}

/// Keeps its agents across a restart but cannot resume a turn on one.
struct Plain;

#[async_trait]
impl Harness for Plain {
    fn id(&self) -> &str {
        "test-plain"
    }

    fn name(&self) -> &str {
        "Plain"
    }

    async fn adopt(&self, _chat: &Arc<ChatHost>, adoption: Adoption) -> Result<()> {
        ADOPTED.lock().unwrap().push(adoption);
        Ok(())
    }
}

fn register() {
    let mut harnesses = TEST_HARNESSES.lock().unwrap();
    harnesses.push(("test-resume", || Box::new(Resumes)));
    harnesses.push(("test-plain", || Box::new(Plain)));
}

fn host() -> Arc<ChatHost> {
    Arc::new(ChatHost::new(
        Arc::new(crate::local::opencode::AgentHost::new(None)),
        Arc::new(crate::local::codex::CodexHost::new()),
        Arc::new(crate::local::claude::ClaudeHost::new()),
    ))
}

fn chat(store: &Store, id: &str, harness: &str) {
    let project = TurnCtx::test_stub().project;
    if store.get_local_project(&project.id).unwrap().is_none() {
        store.create_local_project(&project).unwrap();
    }
    store
        .create_chat_session(&StoredChatSession {
            profile_id: None,
            id: id.into(),
            project_id: project.id,
            harness: harness.into(),
            native_session_id: None,
            native_fork_at: None,
            title: None,
            title_source: None,
            model: None,
            service_tier: None,
            permission_mode: None,
            plan_mode: false,
            plan_reset_pending: false,
            reasoning_level: None,
            archived: false,
            context_usage_json: None,
            bootstrap_context: None,
            goal: None,
            autonomy: None,
            active_leaf_id: None,
            side_parent_session_id: None,
            side_parent_leaf_id: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
}

/// A turn the agent accepted and is running, with no lease: its server let go of it.
fn running_turn(store: &Store, session_id: &str) -> StoredChatTurn {
    let turn = StoredChatTurn {
        id: format!("turn-{session_id}"),
        session_id: session_id.into(),
        user_message_id: None,
        assistant_message_id: format!("answer-{session_id}"),
        client_turn_id: format!("client-{session_id}"),
        request_hash: "task".into(),
        prepared_input: "task".into(),
        settings_json: "{}".into(),
        state: "running".into(),
        delivery_state: "accepted".into(),
        attempt_count: 1,
        next_retry_at: None,
        error_kind: None,
        error_message: None,
        recovery_action: None,
        recovered_by_turn_id: None,
        created_at: crate::store::now_ms(),
        updated_at: crate::store::now_ms(),
    };
    store.admit_chat_turn(None, &turn).unwrap();
    turn
}

/// What a previous server left: a kept `cat` for `session_id` that echoed `a` and `b`, with
/// `a` in the turn's transcript and checkpoint, if it has a running turn.
async fn left_running(store: &Store, harness: &str, session_id: &str, state: Value) -> String {
    let command = crate::process::command("cat");
    let mut kept = keep::spawn(keep::Keep {
        command: &command,
        io: keep::Io::Pipes { stderr: None },
        retain: keep::LOSSLESS,
        owner: Owner::Agent {
            harness: harness.into(),
            session_id: session_id.into(),
            state,
        }
        .to_value(),
    })
    .await
    .unwrap();
    let handle = kept.handle();
    let mut stdin = kept.take_stdin().unwrap();
    let mut lines = kept.take_stdout().unwrap().lines();
    stdin.write_all(b"a\nb\n").await.unwrap();
    let (line, offset) = lines.next_line().await.unwrap().unwrap();
    assert_eq!((line.as_str(), offset), ("a", 2));
    let turn = store
        .latest_chat_turn(session_id)
        .ok()
        .flatten()
        .filter(unfinished);
    if let Some(turn) = turn {
        let mut ctx = resumable_ctx(store, &turn);
        ctx.upsert_part(WirePart::text("a", "a"));
        ctx.flush_at(&handle, offset, &json!({ "seen": 1 }))
            .unwrap();
    }
    handle.detach();
    handle.id().to_string()
}

fn resumable_ctx(store: &Store, turn: &StoredChatTurn) -> TurnCtx {
    let session = store.get_chat_session(&turn.session_id).unwrap().unwrap();
    let project = store
        .get_local_project(&session.project_id)
        .unwrap()
        .unwrap();
    let assistant = WireMessage {
        id: turn.assistant_message_id.clone(),
        role: "assistant".into(),
        parts: Vec::new(),
        created_at: turn.created_at,
        completed_at: None,
        parent_id: None,
    };
    let mut ctx = turn_ctx_from_stored(host(), store, &session, project, turn, assistant).unwrap();
    ctx.delivery_state = DeliveryState::Accepted;
    ctx
}

async fn settled(store: &Store, turn: &StoredChatTurn) -> StoredChatTurn {
    let settled = async {
        loop {
            let stored = store
                .get_chat_turn(&turn.session_id, &turn.id)
                .unwrap()
                .unwrap();
            if !unfinished(&stored) {
                return stored;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), settled)
        .await
        .expect("the turn settles")
}

async fn gone(id: &str) {
    let stopped = async {
        while keep::entries()
            .iter()
            .any(|entry| entry.id == id && entry.alive)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), stopped)
        .await
        .expect("the kept child is stopped");
}

fn texts(store: &Store, turn: &StoredChatTurn) -> Vec<String> {
    let message = store
        .get_chat_message(&turn.assistant_message_id)
        .unwrap()
        .unwrap();
    stored_to_wire(&message)
        .parts
        .iter()
        .filter(|part| part.kind == "text")
        .filter_map(|part| part.text.clone())
        .collect()
}

fn recovery(turn: &StoredChatTurn) -> (&str, Option<&str>) {
    (turn.state.as_str(), turn.recovery_action.as_deref())
}

#[cfg(unix)]
#[test]
fn a_restart_mid_turn_resumes_on_the_kept_agent_without_the_prompt() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        register();
        let store = Store::open().unwrap();
        chat(&store, "chat", "test-resume");
        let turn = running_turn(&store, "chat");
        let keep_id = left_running(&store, "test-resume", "chat", json!({})).await;

        let next = host();
        adopt_kept_work(&next, true).await.unwrap();

        let adopted = ADOPTED.lock().unwrap().clone();
        assert_eq!(adopted.len(), 1);
        assert_eq!(
            (adopted[0].keep_id.as_str(), adopted[0].resume_at),
            (keep_id.as_str(), Some(2))
        );
        let stored = settled(&store, &turn).await;
        assert_eq!(stored.state, "completed", "{:?}", stored.error_message);
        // `a` was applied before the restart and `b` after it, each once.
        assert_eq!(texts(&store, &turn), ["a", "b"]);
        assert!(store.chat_turn_checkpoint(&turn.id).unwrap().is_none());
        gone(&keep_id).await;
    });
}

#[cfg(unix)]
#[test]
fn a_crashed_servers_turn_resumes_only_once_its_lease_expires() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        register();
        let store = Store::open().unwrap();
        chat(&store, "chat", "test-resume");
        let turn = running_turn(&store, "chat");
        left_running(&store, "test-resume", "chat", json!({})).await;
        // The crashed server's lease, still fresh.
        assert!(store.claim_chat_turn("chat", "crashed", &[]).unwrap());

        let next = host();
        adopt_kept_work(&next, true).await.unwrap();
        next.reconcile_expired_turn_leases().await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let held = store.get_chat_turn("chat", &turn.id).unwrap().unwrap();
        assert_eq!(held.state, "running");
        assert!(!next.is_busy("chat").await);

        let tx = store.begin().unwrap();
        tx.execute("UPDATE chat_turn_leases SET heartbeat_at = 0", [])
            .unwrap();
        tx.commit().unwrap();
        next.reconcile_expired_turn_leases().await.unwrap();
        assert_eq!(settled(&store, &turn).await.state, "completed");
        assert_eq!(texts(&store, &turn), ["a", "b"]);
    });
}

#[cfg(unix)]
#[test]
fn a_turn_that_cannot_resume_fails_with_continue_and_its_agent_stops() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        register();
        let store = Store::open().unwrap();
        chat(&store, "plain", "test-plain");
        chat(&store, "refused", "test-resume");
        chat(&store, "unsaved", "test-resume");
        let plain = running_turn(&store, "plain");
        let refused = running_turn(&store, "refused");
        let plain_agent = left_running(&store, "test-plain", "plain", json!({})).await;
        let refused_agent =
            left_running(&store, "test-resume", "refused", json!({ "refuse": true })).await;
        // A turn whose agent started but has no checkpoint yet.
        let unsaved_agent = left_running(&store, "test-resume", "unsaved", json!({})).await;
        let unsaved = running_turn(&store, "unsaved");

        let next = host();
        adopt_kept_work(&next, true).await.unwrap();

        for turn in [&plain, &refused, &unsaved] {
            let stored = store
                .get_chat_turn(&turn.session_id, &turn.id)
                .unwrap()
                .unwrap();
            assert_eq!(
                recovery(&stored),
                ("failed", Some("continue")),
                "{}",
                turn.id
            );
        }
        for agent in [&plain_agent, &refused_agent, &unsaved_agent] {
            gone(agent).await;
        }
        let adopted = ADOPTED.lock().unwrap().clone();
        assert_eq!(adopted.len(), 1, "only the refusing agent was offered back");
        assert_eq!(adopted[0].keep_id, refused_agent);
        assert!(!next.is_busy("refused").await);
    });
}

#[cfg(unix)]
#[test]
fn keepers_with_no_chat_or_no_child_are_cleared_and_idle_agents_adopted() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        register();
        let store = Store::open().unwrap();
        chat(&store, "idle", "test-resume");
        chat(&store, "dead", "test-resume");
        let dead_turn = running_turn(&store, "dead");
        let idle_agent = left_running(&store, "test-resume", "idle", json!({ "kept": 1 })).await;
        let deleted_agent = left_running(&store, "test-resume", "deleted", json!({})).await;
        let dead_agent = left_running(&store, "test-resume", "dead", json!({})).await;
        // A keeper that died with its child leaves only its directory.
        keep::attach(&dead_agent)
            .await
            .unwrap()
            .handle()
            .stop(STOP_TIMEOUT)
            .await
            .unwrap();
        std::fs::create_dir_all(keep::root().join(&dead_agent)).unwrap();

        let next = host();
        adopt_kept_work(&next, true).await.unwrap();

        let adopted = ADOPTED.lock().unwrap().clone();
        assert_eq!(adopted.len(), 1);
        assert_eq!(
            (adopted[0].keep_id.as_str(), adopted[0].resume_at),
            (idle_agent.as_str(), None)
        );
        assert_eq!(adopted[0].state, json!({ "kept": 1 }));
        gone(&deleted_agent).await;
        assert!(!keep::root().join(&dead_agent).exists());
        let stored = store.get_chat_turn("dead", &dead_turn.id).unwrap().unwrap();
        assert_eq!(recovery(&stored), ("failed", Some("continue")));
        assert!(keep::entries()
            .iter()
            .any(|entry| entry.id == idle_agent && entry.alive));
        keep::attach(&idle_agent)
            .await
            .unwrap()
            .handle()
            .stop(STOP_TIMEOUT)
            .await;
    });
}

#[cfg(unix)]
#[test]
fn agents_whose_server_moved_port_are_stopped_not_adopted() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        register();
        let store = Store::open().unwrap();
        chat(&store, "chat", "test-resume");
        let turn = running_turn(&store, "chat");
        let agent = left_running(&store, "test-resume", "chat", json!({})).await;

        adopt_kept_work(&host(), false).await.unwrap();

        assert!(ADOPTED.lock().unwrap().is_empty());
        gone(&agent).await;
        let stored = store.get_chat_turn("chat", &turn.id).unwrap().unwrap();
        assert_eq!(recovery(&stored), ("failed", Some("continue")));
    });
}

#[cfg(unix)]
#[test]
fn a_restart_waits_only_for_work_it_would_stop() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        register();
        let store = Store::open().unwrap();
        chat(&store, "resumes", "test-resume");
        chat(&store, "plain", "test-plain");
        let resumes = running_turn(&store, "resumes");
        let plain = running_turn(&store, "plain");
        left_running(&store, "test-resume", "resumes", json!({})).await;
        left_running(&store, "test-plain", "plain", json!({})).await;
        let chat = host();
        for turn in [&resumes, &plain] {
            chat.turns.lock().await.insert(
                turn.session_id.clone(),
                TurnState::Active(super::super::ActiveTurn {
                    handle: tokio::spawn(std::future::pending()),
                    message_id: turn.assistant_message_id.clone(),
                    turn_id: turn.id.clone(),
                }),
            );
        }

        assert_eq!(
            chat.stopped_by_restart().await,
            HashSet::from(["plain".to_string()])
        );
        assert!(!chat.stop_admitting_unless_restart_stops().await);
        chat.turns.lock().await.remove("plain");
        assert!(chat.stop_admitting_unless_restart_stops().await);

        // A turn whose agent's keeper is gone would not resume either.
        store
            .write_chat_turn_checkpoint(crate::store::TurnCheckpointWrite::Set {
                turn_id: &resumes.id,
                keep_id: "gone",
                offset: 0,
                state_json: "{}",
            })
            .unwrap();
        assert_eq!(
            chat.stopped_by_restart().await,
            HashSet::from(["resumes".to_string()])
        );
        super::stop_kept(|_| true, "the test ended").await;
    });
}
