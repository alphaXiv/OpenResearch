use tokio::sync::broadcast::error::RecvError;

use super::*;
use crate::local::harness::codex::AccountError;
use crate::local::usage::{ChatSpend, Reader, Sweep, UsageReport, PLANS};

/// How long a sweep waits for the harness list's first full check.
const SIGN_IN_CHECK: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
pub(super) struct UsageQuery {
    chat: Option<String>,
}

/// The cached report at once. A stale re-read of the plans goes on in the
/// background, and its end emits `usage.changed`.
pub(super) async fn usage(state: State<AppState>, query: Query<UsageQuery>) -> ApiResult {
    report(state, query, false).await
}

/// [`usage`], re-reading every plan however fresh: each read starts an agent
/// that reaches its provider with the user's sign-in.
pub(super) async fn refresh(state: State<AppState>, query: Query<UsageQuery>) -> ApiResult {
    report(state, query, true).await
}

async fn report(
    State(state): State<AppState>,
    Query(query): Query<UsageQuery>,
    force: bool,
) -> ApiResult {
    if let Some(sweep) = PLANS.begin_sweep(std::time::Instant::now(), force) {
        tokio::spawn(read_plans(state.clone(), force, sweep));
    }

    let plans = PLANS.report();
    let now = crate::store::now_ms();
    let report = tokio::task::spawn_blocking(move || -> Result<UsageReport> {
        let store = Store::open()?;
        let chat = query
            .chat
            .map(|id| -> Result<ChatSpend> {
                Ok(ChatSpend {
                    agent: store.chat_cost(&id)?,
                    compute: local::spend::chat_compute(&store, &id, now)?,
                })
            })
            .transpose()?;
        Ok(UsageReport {
            plans,
            chat,
            ..local::spend::seven_days(&store, now)?
        })
    })
    .await
    .map_err(|error| anyhow!("Reading usage stopped: {error}"))??;
    Ok(Json(json!(report)))
}

async fn read_plans(state: AppState, force: bool, sweep: Sweep<'static>) {
    PLANS.sync(readers(&state).await);
    PLANS
        .read(
            PLANS.claim(None, std::time::Instant::now(), force),
            &state.chat.codex,
        )
        .await;
    drop(sweep);
    state.chat.emit_event("usage.changed", json!({}));
}

/// The machine's Claude Code and Codex accounts, once detection has checked
/// their sign-ins: a provisional list can't yet tell an API key from a plan.
async fn readers(state: &AppState) -> Vec<Reader> {
    let mut events = state.chat.subscribe();
    let deadline = tokio::time::Instant::now() + SIGN_IN_CHECK;
    let query = HarnessQuery::default();
    let mut payload = harnesses_payload(state, &query).await;
    while payload_is_provisional(&payload) {
        let checked = async {
            loop {
                match events.recv().await {
                    Ok(("harness.catalog", _)) | Err(RecvError::Lagged(_)) => return true,
                    Ok(_) => {}
                    Err(RecvError::Closed) => return false,
                }
            }
        };
        if tokio::time::timeout_at(deadline, checked).await != Ok(true) {
            break;
        }
        payload = harnesses_payload(state, &query).await;
    }
    let hidden = match Store::open().and_then(|store| store.ui_state()) {
        Ok(ui) => ui.hidden_agents,
        Err(error) => {
            eprintln!(
                "orx up: could not read which agents are turned off, so every plan is read: {error:#}"
            );
            Default::default()
        }
    };
    let entries = payload["harnesses"].as_array_mut().into_iter().flatten();
    entries
        .filter_map(|entry| {
            entry["off"] = json!(local::model_routes::turned_off(entry, &hidden));
            Reader::from_entry(entry)
        })
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResetRequest {
    profile: Option<String>,
    credit_id: Option<String>,
    /// The client's key for one confirmed click, sent again when it retries.
    attempt: String,
}

/// Spends one of a Codex account's resets, then re-reads that account's plan
/// and emits `usage.changed`, whatever the answer: a reset that timed out may
/// still have happened.
pub(super) async fn reset(
    State(state): State<AppState>,
    Json(request): Json<ResetRequest>,
) -> ApiResult {
    if request.attempt.trim().is_empty() {
        return Err(bad_request("A reset needs an attempt key."));
    }
    let profile = request.profile.as_deref();
    let Some(reader) = PLANS.reader("codex", profile) else {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "This Codex account isn't signed in on this machine anymore.".into(),
        ));
    };
    let home = reader.home()?;
    let consumed = local::harness::codex::consume_reset(
        &state.chat.codex,
        profile,
        &reader.bin,
        home.as_deref(),
        request.credit_id.as_deref(),
        &request.attempt,
    )
    .await;

    let claimed = PLANS.claim(Some(("codex", profile)), std::time::Instant::now(), true);
    PLANS.read(claimed, &state.chat.codex).await;
    state.chat.emit_event("usage.changed", json!({}));

    let outcome = consumed.map_err(|error| match error {
        AccountError::Unsupported => ApiError(
            StatusCode::CONFLICT,
            match &reader.version {
                Some(version) => {
                    format!("Codex {version} can't use resets. Update Codex, then try again.")
                }
                None => {
                    "This version of Codex can't use resets. Update Codex, then try again.".into()
                }
            },
        ),
        AccountError::Rejected(message) => ApiError(
            StatusCode::BAD_GATEWAY,
            format!("Codex couldn't use the reset: {message}"),
        ),
        AccountError::Unanswered(message) => ApiError(
            StatusCode::BAD_GATEWAY,
            format!("{message} Try again; it won't use a second reset."),
        ),
    })?;
    Ok(Json(json!({ "outcome": outcome })))
}
