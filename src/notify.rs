//! Outbound notifications (Slack today, other providers later) queued in
//! `notifications_outbox` and drained by one background loop in `orx up`.
//!
//! Any process holding a `Store` — including a detached `orx supervise` —
//! can call [`crate::store::Store::enqueue_notification`] to drop a message
//! in the queue. Delivery itself only ever happens from the single `orx up`
//! process that owns the drain loop, so two writers never race a send.
//!
//! This module is the drain mechanics and the Slack transport. Deciding
//! *when* to enqueue (a run starting, a run's outcome getting synthesized,
//! a usage limit hit) is each feature's own job — see `notify_events` —
//! as is where a webhook URL comes from (`config_dir()/slack.json`, the
//! same shape as `overleaf.json` in `src/config.rs`). [`spawn_notifier_loop`]
//! is started from `commands::up::run`, only when a webhook is configured.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::store::Store;

/// How one delivery attempt turned out. A rejected payload (a revoked or
/// disabled webhook, a malformed request) is pointless to retry; a network
/// hiccup or a 5xx is worth trying again on the next drain pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Sent,
    Rejected,
    Retryable,
}

/// A place to deliver notifications to. `kind` is the notification's own
/// tag (`"job_submitted"`, `"run_synthesized"`, …); `payload` is whatever
/// the enqueuing code put in `notifications_outbox.payload_json`, parsed.
/// A provider owns its own formatting — this trait does not assume Slack's
/// `text`/`blocks` shape belongs to every future provider.
#[async_trait]
pub trait NotificationProvider: Send + Sync {
    async fn send(&self, kind: &str, payload: &Value) -> DeliveryOutcome;
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// Posts to a Slack Incoming Webhook. The payload's `"text"` field is sent
/// verbatim; a caller that wants Slack `blocks` formatting puts them under
/// `"blocks"` in the same payload and this provider forwards them as-is.
pub struct SlackWebhookProvider {
    webhook_url: String,
}

impl SlackWebhookProvider {
    pub fn new(webhook_url: String) -> Self {
        Self { webhook_url }
    }
}

#[async_trait]
impl NotificationProvider for SlackWebhookProvider {
    async fn send(&self, _kind: &str, payload: &Value) -> DeliveryOutcome {
        let mut body = serde_json::Map::new();
        if let Some(text) = payload.get("text") {
            body.insert("text".to_string(), text.clone());
        }
        if let Some(blocks) = payload.get("blocks") {
            body.insert("blocks".to_string(), blocks.clone());
        }
        let Ok(response) = http()
            .post(&self.webhook_url)
            .timeout(Duration::from_secs(5))
            .json(&Value::Object(body))
            .send()
            .await
        else {
            return DeliveryOutcome::Retryable;
        };
        if response.status().is_success() {
            DeliveryOutcome::Sent
        } else if matches!(response.status().as_u16(), 400 | 404 | 410) {
            // Slack answers a revoked/disabled webhook with 404 or 410, and
            // a malformed payload with 400 — none of those improve on retry.
            DeliveryOutcome::Rejected
        } else {
            DeliveryOutcome::Retryable
        }
    }
}

/// Attempts a notification is allowed before the drain loop gives up on it
/// (it stays in the outbox, unsent, rather than being deleted — visible for
/// debugging, harmless since nothing else reads unsent rows as "pending
/// work" outside this module). Matches "a few attempts with backoff": the
/// backoff is simply the drain loop's own poll interval between attempts.
const MAX_ATTEMPTS: i64 = 3;

/// One drain pass: attempt delivery of every currently-pending notification
/// (up to `MAX_ATTEMPTS` each) through `provider`, oldest first.
///
/// Takes `store` by value, not `&Store`: `rusqlite::Connection` isn't
/// `Sync`, so a `&Store` held across the `provider.send(...).await` below
/// would make this function's future `!Send` — fatal for `tokio::spawn` in
/// [`spawn_notifier_loop`], which needs to hand the task to any worker
/// thread. An owned `Store` has no such borrow and is `Send`. Callers reopen
/// a fresh connection each time (cheap: WAL mode, a short busy timeout)
/// rather than share one across calls.
pub async fn drain_once(
    store: Store,
    provider: &dyn NotificationProvider,
) -> crate::error::Result<()> {
    for notification in store.list_pending_notifications(20)? {
        if notification.attempts >= MAX_ATTEMPTS {
            continue;
        }
        let payload: Value = match serde_json::from_str(&notification.payload_json) {
            Ok(value) => value,
            Err(err) => {
                // Never sendable — record it and let MAX_ATTEMPTS retire the
                // row like any other permanent failure, rather than looping.
                store.mark_notification_attempt_failed(&notification.id, &err.to_string())?;
                continue;
            }
        };
        match provider.send(&notification.kind, &payload).await {
            DeliveryOutcome::Sent => store.mark_notification_sent(&notification.id)?,
            DeliveryOutcome::Rejected => {
                store.mark_notification_attempt_failed(&notification.id, "rejected by provider")?
            }
            DeliveryOutcome::Retryable => store.mark_notification_attempt_failed(
                &notification.id,
                "delivery attempt failed, will retry",
            )?,
        }
    }
    Ok(())
}

/// Drains the outbox on an interval for as long as `orx up` runs. Exactly
/// one of these should run per process — a detached `orx supervise` only
/// ever enqueues (`Store::enqueue_notification`), never drains, so no two
/// writers can race a send. Opens a fresh `Store` each tick (cheap: WAL
/// mode, a short busy timeout) rather than holding one connection across
/// every await in between, matching `watch_runs`'s loop.
pub fn spawn_notifier_loop(provider: Arc<dyn NotificationProvider>) {
    tokio::spawn(async move {
        loop {
            match Store::open() {
                Ok(store) => {
                    if let Err(err) = drain_once(store, provider.as_ref()).await {
                        eprintln!("orx up: notification drain failed (will retry): {err}");
                    }
                }
                Err(err) => eprintln!("orx up: notification drain could not open the store: {err}"),
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every `(kind, payload)` it was asked to send and returns a
    /// scripted outcome per call, so tests can drive multi-attempt behavior
    /// without a real HTTP server.
    struct ScriptedProvider {
        outcomes: Mutex<Vec<DeliveryOutcome>>,
        calls: Mutex<Vec<(String, Value)>>,
    }

    impl ScriptedProvider {
        fn new(outcomes: Vec<DeliveryOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl NotificationProvider for ScriptedProvider {
        async fn send(&self, kind: &str, payload: &Value) -> DeliveryOutcome {
            self.calls
                .lock()
                .unwrap()
                .push((kind.to_string(), payload.clone()));
            let mut outcomes = self.outcomes.lock().unwrap();
            if outcomes.is_empty() {
                DeliveryOutcome::Sent
            } else {
                outcomes.remove(0)
            }
        }
    }

    /// A fresh connection to the same on-disk database each call — exactly
    /// how `spawn_notifier_loop` uses `drain_once` in production, and
    /// necessary here since `drain_once` now consumes its `Store`.
    fn open(dir: &std::path::Path) -> Store {
        Store::open_at(dir.to_path_buf()).unwrap()
    }

    fn temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("orx-notify-{}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn a_sent_notification_leaves_the_pending_list() {
        let dir = temp_dir();
        open(&dir)
            .enqueue_notification("job_submitted", "{\"text\":\"hello\"}")
            .unwrap();
        let provider = ScriptedProvider::new(vec![]);

        drain_once(open(&dir), &provider).await.unwrap();

        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 0);
        assert_eq!(
            provider.calls.lock().unwrap().as_slice(),
            [(
                "job_submitted".to_string(),
                serde_json::json!({"text": "hello"})
            )]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_retryable_failure_stays_pending_for_the_next_drain_pass() {
        let dir = temp_dir();
        let id = open(&dir)
            .enqueue_notification("run_synthesized", "{\"text\":\"done\"}")
            .unwrap();
        let provider = ScriptedProvider::new(vec![DeliveryOutcome::Retryable]);

        drain_once(open(&dir), &provider).await.unwrap();
        let pending = open(&dir).list_pending_notifications(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].attempts, 1);

        // A second pass succeeds and the row finally clears.
        drain_once(open(&dir), &provider).await.unwrap();
        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 0);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_notification_stops_being_attempted_after_max_attempts() {
        let dir = temp_dir();
        open(&dir)
            .enqueue_notification("job_submitted", "{\"text\":\"x\"}")
            .unwrap();
        let provider = ScriptedProvider::new(vec![
            DeliveryOutcome::Retryable,
            DeliveryOutcome::Retryable,
            DeliveryOutcome::Retryable,
            DeliveryOutcome::Retryable,
        ]);

        for _ in 0..(MAX_ATTEMPTS + 2) {
            drain_once(open(&dir), &provider).await.unwrap();
        }

        // Exactly MAX_ATTEMPTS calls reached the provider, not MAX_ATTEMPTS + 2.
        assert_eq!(provider.calls.lock().unwrap().len(), MAX_ATTEMPTS as usize);
        // Still sitting in the outbox, unsent — visible, not silently dropped.
        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_unparseable_payload_is_recorded_and_not_retried_forever() {
        let dir = temp_dir();
        open(&dir)
            .enqueue_notification("job_submitted", "not json")
            .unwrap();
        let provider = ScriptedProvider::new(vec![]);

        for _ in 0..(MAX_ATTEMPTS + 1) {
            drain_once(open(&dir), &provider).await.unwrap();
        }

        // The provider was never called — the payload never parsed.
        assert_eq!(provider.calls.lock().unwrap().len(), 0);
        assert_eq!(open(&dir).list_pending_notifications(10).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(dir);
    }

    /// `SlackWebhookProvider` against a real local HTTP server — never a live
    /// `hooks.slack.com` URL, per this track's credential-handling rule.
    /// Doubles as coverage for the `/api/settings/slack/preflight` route,
    /// which sends through the exact same provider.
    #[tokio::test]
    async fn slack_webhook_provider_delivers_through_a_local_axum_stub() {
        use axum::extract::State;
        use axum::http::StatusCode;
        use axum::routing::post;
        use std::sync::atomic::{AtomicU16, Ordering};

        async fn respond(
            State(status): State<Arc<AtomicU16>>,
            body: axum::body::Bytes,
        ) -> StatusCode {
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["text"], "hello from orx");
            StatusCode::from_u16(status.load(Ordering::SeqCst)).unwrap()
        }

        let status = Arc::new(AtomicU16::new(200));
        let app = axum::Router::new()
            .route("/webhook", post(respond))
            .with_state(status.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = SlackWebhookProvider::new(format!("http://{addr}/webhook"));
        let message = serde_json::json!({ "text": "hello from orx" });

        status.store(200, Ordering::SeqCst);
        assert_eq!(
            provider.send("preflight", &message).await,
            DeliveryOutcome::Sent
        );

        status.store(404, Ordering::SeqCst);
        assert_eq!(
            provider.send("preflight", &message).await,
            DeliveryOutcome::Rejected
        );

        status.store(500, Ordering::SeqCst);
        assert_eq!(
            provider.send("preflight", &message).await,
            DeliveryOutcome::Retryable
        );
    }
}
