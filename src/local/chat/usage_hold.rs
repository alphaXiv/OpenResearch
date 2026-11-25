//! An account out of usage holds its chats: what they would send waits for the limit's reset,
//! then goes out on its own unless the user stopped that chat's timer.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::{session_json, ChatHost, RecoveryOverrides};
use crate::error::{anyhow, Result};
use crate::local::harness::{failure_cause, FailureCause};
use crate::store::{now_ms, Store, UsageLimit};

/// An agent and the profile it signs in with; None is the user's own sign-in.
pub(crate) type Account<'a> = (&'a str, Option<&'a str>);

impl ChatHost {
    /// The account refused a turn at `since` on its usage limit: hold its chats until `reset_at`.
    pub(crate) fn hold_for_usage_limit(
        self: &Arc<Self>,
        account: Account<'_>,
        reset_at: i64,
        since: i64,
    ) {
        let (harness, profile) = account;
        let recorded = Store::open().and_then(|store| {
            store.record_usage_limit(harness, profile, reset_at, since)?;
            store.usage_limit(harness, profile)
        });
        match recorded {
            Ok(Some(limit)) => {
                self.emit_event("usage.changed", json!({}));
                self.wait_for_reset(limit);
            }
            Ok(None) => {}
            Err(error) => eprintln!(
                "orx up: could not record that {harness} is out of usage until {reset_at}: {error:#}"
            ),
        }
    }

    /// The account served a turn or spent a reset, so its limit is gone: send what it held.
    pub(crate) fn lift_usage_limit(self: &Arc<Self>, account: Account<'_>) {
        self.lift(account, None);
    }

    /// Clears the account's limit, only the one resetting at `reset_at` when given.
    fn lift(self: &Arc<Self>, account: Account<'_>, reset_at: Option<i64>) {
        let (harness, profile) = account;
        let lifted =
            Store::open().and_then(|store| store.clear_usage_limit(harness, profile, reset_at));
        match lifted {
            Ok(Some(limit)) => {
                self.emit_event("usage.changed", json!({}));
                self.release_account(limit);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("orx up: could not clear {harness}'s usage limit: {error:#}")
            }
        }
    }

    /// Waits out every limit the last server left, sending at once what one already past holds.
    pub(crate) fn arm_usage_limits(self: &Arc<Self>) {
        match Store::open().and_then(|store| store.usage_limits()) {
            Ok(limits) => limits
                .into_iter()
                .for_each(|limit| self.wait_for_reset(limit)),
            Err(error) => eprintln!("orx up: could not read the accounts out of usage: {error:#}"),
        }
    }

    fn wait_for_reset(self: &Arc<Self>, limit: UsageLimit) {
        let host = self.clone();
        tokio::spawn(async move {
            // In slices, read against the wall clock: the monotonic one stops while the machine sleeps.
            loop {
                let left = limit.reset_at.saturating_sub(now_ms());
                if left <= 0 {
                    break;
                }
                let slice = Duration::from_millis(left.min(60_000) as u64);
                tokio::time::sleep(crate::config::paced(slice)).await;
            }
            // The next server sends what the limit held.
            if host.exiting.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            // A later refusal moved the reset and waits for it on its own.
            host.lift(
                (limit.harness.as_str(), limit.profile.as_deref()),
                Some(limit.reset_at),
            );
        });
    }

    /// Sends what each of the account's chats held since the limit began, skipping chats whose
    /// timer the user stopped.
    fn release_account(self: &Arc<Self>, limit: UsageLimit) {
        let sessions = Store::open().and_then(|store| {
            let mut held = Vec::new();
            for session in store.list_all_chat_sessions()? {
                if session.harness == limit.harness
                    && session.profile_id == limit.profile
                    && !session.archived
                    && !store.chat_limit_timer_stopped(&session.id)?
                {
                    held.push(session.id);
                }
            }
            Ok(held)
        });
        match sessions {
            Ok(sessions) => {
                for session_id in sessions {
                    let host = self.clone();
                    tokio::spawn(async move { host.release_chat(&session_id, limit.since).await });
                }
            }
            Err(error) => eprintln!(
                "orx up: could not find the chats {} held for its usage limit: {error:#}",
                limit.harness
            ),
        }
    }

    /// Continues the chat's last turn if a usage limit failed it after `since`, else sends its
    /// queue.
    async fn release_chat(self: &Arc<Self>, session_id: &str, since: i64) {
        let limited = Store::open().and_then(|store| store.latest_chat_turn(session_id));
        let turn = match limited {
            Ok(turn) => turn.filter(|turn| {
                turn.state == "failed"
                    && turn.recovered_by_turn_id.is_none()
                    && turn.updated_at >= since
                    && failure_cause(
                        turn.error_kind.as_deref().unwrap_or_default(),
                        turn.error_message.as_deref().unwrap_or_default(),
                    ) == Some(FailureCause::UsageLimit)
            }),
            Err(error) => {
                eprintln!("orx up: could not read chat {session_id}'s last turn: {error:#}");
                None
            }
        };
        let recovery = turn.as_ref().and_then(|turn| {
            let action = turn.recovery_action.as_deref()?;
            Some((turn.id.as_str(), action))
        });
        match recovery {
            Some((turn_id, action)) => {
                if let Err(error) = self
                    .recover_turn(session_id, turn_id, action, RecoveryOverrides::default())
                    .await
                {
                    eprintln!(
                        "orx up: chat {session_id} could not {action} after its usage reset: {error:#}"
                    );
                }
            }
            None => self.kick_queue(session_id.to_string(), Duration::ZERO),
        }
    }

    /// Whether the chat's queue waits: its account is out of usage, or the user stopped its timer.
    pub(super) fn holds_queue(&self, session_id: &str) -> bool {
        let held = || -> Result<bool> {
            let store = Store::open()?;
            let Some(session) = store.get_chat_session(session_id)? else {
                return Ok(false);
            };
            Ok(store
                .usage_limit(&session.harness, session.profile_id.as_deref())?
                .is_some()
                || store.chat_limit_timer_stopped(session_id)?)
        };
        held().unwrap_or_else(|error| {
            eprintln!("orx up: could not tell whether chat {session_id} waits for a usage reset, so its queue waits: {error:#}");
            true
        })
    }

    /// Stops the chat's wait for its account's reset, or starts it again; started once the
    /// limit is gone, it sends what the chat held now.
    pub async fn set_limit_timer(self: &Arc<Self>, session_id: &str, stopped: bool) -> Result<()> {
        let store = Store::open()?;
        let session = store
            .get_chat_session(session_id)?
            .ok_or_else(|| anyhow!("chat session not found"))?;
        store.set_chat_limit_timer_stopped(session_id, stopped)?;
        self.emit(
            "chat.session",
            json!({ "session": session_json(&session, self.is_busy(session_id).await) }),
        );
        let limited = store
            .usage_limit(&session.harness, session.profile_id.as_deref())?
            .is_some();
        if !stopped && !limited {
            self.release_chat(session_id, 0).await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::TurnOverrides;
    use crate::store::StoredChatSession;

    const AGENT: &str = "acp:00000000-0000-4000-8000-000000000002";

    /// An ACP agent that exits at once, so a turn the hold lets go of ends quickly.
    const EXITS: &str = "raise SystemExit(1)";

    /// An ACP agent that refuses its first prompt on a usage limit and answers the rest; argv[1]
    /// marks the refusal.
    const REFUSES_ONCE: &str = r#"
import json, os, sys
refused = sys.argv[1]
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    if method == 'initialize':
        reply = {'result': {'protocolVersion': 1, 'agentCapabilities': {'loadSession': True}}}
    elif method == 'session/new':
        reply = {'result': {'sessionId': 'limited'}}
    elif method == 'session/prompt' and not os.path.exists(refused):
        open(refused, 'w').close()
        reply = {'error': {'code': -32000, 'message': "You've hit your usage limit."}}
    elif method == 'session/prompt':
        reply = {'result': {'stopReason': 'end_turn'}}
    else:
        reply = {'result': {}}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], **reply}), flush=True)
"#;

    /// A chat on an ACP agent running `script`.
    fn chat(store: &Store, script: &str) -> String {
        let launch = crate::local::acp::Definition {
            id: AGENT.into(),
            name: "Fixture".into(),
            executable: crate::local::acp::fake_agent_interpreter("python3"),
            arguments: vec![
                "-u".into(),
                "-c".into(),
                script.into(),
                crate::store::data_dir()
                    .join("refused")
                    .to_string_lossy()
                    .into_owned(),
            ],
            unknown: Default::default(),
        };
        crate::local::acp::save(launch, true).unwrap();
        let repo = crate::store::data_dir().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("README.md"), "demo\n").unwrap();
        crate::local::git::initialize_repository(&repo).unwrap();
        let mut project = crate::local::chat::TurnCtx::test_stub().project;
        project.repo_path = repo.to_string_lossy().into_owned();
        store.create_local_project(&project).unwrap();
        let id = "held".to_string();
        store
            .create_chat_session(&StoredChatSession {
                profile_id: None,
                id: id.clone(),
                project_id: project.id,
                harness: AGENT.into(),
                native_session_id: None,
                native_fork_at: None,
                title: Some("Held".into()),
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
                active_leaf_id: None,
                side_parent_session_id: None,
                side_parent_leaf_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        id
    }

    fn host() -> Arc<ChatHost> {
        Arc::new(ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            Arc::new(crate::local::claude::ClaudeHost::new()),
        ))
    }

    async fn send(host: &Arc<ChatHost>, id: &str) -> bool {
        host.send_message(
            id,
            "after the reset".into(),
            TurnOverrides::default(),
            vec![],
            vec![],
            None,
            None,
        )
        .await
        .unwrap()
        .queued
    }

    async fn sent(host: &Arc<ChatHost>, id: &str) {
        let mut events = host.subscribe();
        tokio::time::timeout(Duration::from_secs(60), async {
            while !host.queued_items(id).is_empty() {
                let _ = events.recv().await;
            }
        })
        .await
        .expect("the held message was sent");
    }

    /// The account's last plan read says its 5-hour window is used up until `reset_at`.
    fn plan_used_up_until(reset_at: i64) {
        use crate::local::usage::{PlanWindow, Read, Reader, Snapshot, PLANS};
        let reader = Reader {
            harness: AGENT.into(),
            profile: None,
            bin: "/bin/agent".into(),
            version: None,
            plan: None,
            email: None,
            subscription: true,
        };
        PLANS.sync(vec![reader.clone()]);
        let sweep = PLANS.begin_sweep(std::time::Instant::now(), true);
        let window = PlanWindow {
            minutes: Some(300),
            model: None,
            remaining_percent: 0.0,
            resets_at: Some(crate::telemetry::iso8601_utc(reset_at)),
        };
        let snapshot = Snapshot {
            plan: None,
            windows: vec![window],
            credits: None,
            extra_usage: None,
            resets: None,
        };
        PLANS.finish(
            &reader,
            Read::Plan(snapshot),
            std::time::Instant::now(),
            String::new(),
        );
        drop(sweep);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_turn_that_meets_the_limit_continues_at_the_reset() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let store = Store::open().unwrap();
        let id = chat(&store, REFUSES_ONCE);
        let host = host();
        plan_used_up_until(now_ms() + 10_000);
        let mut events = host.subscribe();
        let sent = host
            .send_message(
                &id,
                "go".into(),
                TurnOverrides::default(),
                vec![],
                vec![],
                None,
                None,
            )
            .await
            .unwrap();
        assert!(!sent.queued, "nothing holds the chat before the refusal");
        assert!(
            send(&host, &id).await,
            "a message behind the refused turn waits"
        );
        let continued = || {
            store
                .get_chat_turn(&id, &sent.turn_id)
                .unwrap()
                .is_some_and(|turn| turn.recovered_by_turn_id.is_some())
        };
        let done = || {
            continued()
                && host.queued_items(&id).is_empty()
                && store
                    .latest_chat_turn(&id)
                    .unwrap()
                    .is_some_and(|turn| turn.state == "completed")
        };
        tokio::time::timeout(Duration::from_secs(60), async {
            while !done() {
                let _ = events.recv().await;
            }
        })
        .await
        .expect("the refused turn continued at the reset, then its queue was sent");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_reset_sends_what_an_account_out_of_usage_held() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let store = Store::open().unwrap();
        let id = chat(&store, EXITS);
        let host = host();
        host.hold_for_usage_limit((AGENT, None), now_ms() + 300, now_ms());
        assert!(send(&host, &id).await, "an idle chat waits for the reset");
        sent(&host, &id).await;
        assert_eq!(store.usage_limit(AGENT, None).unwrap(), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_stopped_timer_keeps_the_chat_waiting_past_the_reset() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let store = Store::open().unwrap();
        let id = chat(&store, EXITS);
        let host = host();
        host.hold_for_usage_limit((AGENT, None), now_ms() + 3_600_000, now_ms());
        assert!(send(&host, &id).await);
        host.set_limit_timer(&id, true).await.unwrap();
        let session = store.get_chat_session(&id).unwrap().unwrap();
        assert_eq!(session_json(&session, false)["limitTimerStopped"], true);

        host.lift_usage_limit((AGENT, None));
        assert_eq!(
            host.queued_items(&id).len(),
            1,
            "a stopped timer outlasts the limit"
        );

        host.set_limit_timer(&id, false).await.unwrap();
        sent(&host, &id).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_later_refusal_moves_the_reset_and_keeps_when_the_limit_began() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let store = Store::open().unwrap();
        store
            .record_usage_limit("codex", Some("work"), 10, 1)
            .unwrap();
        store
            .record_usage_limit("codex", Some("work"), 20, 2)
            .unwrap();
        let limit = store.usage_limit("codex", Some("work")).unwrap().unwrap();
        assert_eq!((limit.reset_at, limit.since), (20, 1));
        assert_eq!(store.usage_limit("codex", None).unwrap(), None);
        // The timer of a reset that a later refusal moved forgets nothing.
        assert_eq!(
            store
                .clear_usage_limit("codex", Some("work"), Some(10))
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .clear_usage_limit("codex", Some("work"), None)
                .unwrap(),
            Some(limit)
        );
        assert_eq!(
            store
                .clear_usage_limit("codex", Some("work"), None)
                .unwrap(),
            None
        );
    }
}
