//! Settings → Harnesses → Accounts: several accounts per harness, their
//! quota, and signing a new account in through an embedded terminal.

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::extract::Query;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{bad_request, ApiResult};
use crate::local::harness::accounts;

#[derive(Deserialize)]
pub(super) struct AccountsQuery {
    #[serde(default)]
    harness: Option<String>,
    /// Ask each provider what its accounts have left before answering.
    #[serde(default)]
    refresh: Option<u8>,
}

/// Every account-capable harness with its pools and accounts.
pub(crate) async fn accounts_payload(harness: Option<&str>, refresh: bool) -> ApiResult {
    let mut read_errors = serde_json::Map::new();
    if refresh {
        let targets: Vec<(String, String)> = accounts::statuses(harness)?
            .into_iter()
            .filter(|account| account.signed_in && !account.paused)
            .map(|account| (account.harness, account.id))
            .collect();
        let reads = futures::future::join_all(targets.iter().map(|(harness, id)| async move {
            (
                format!("{harness}/{id}"),
                accounts::refresh(harness, id).await,
            )
        }))
        .await;
        for (key, read) in reads {
            if let Err(error) = read {
                read_errors.insert(key, Value::String(error));
            }
        }
    }
    let statuses = accounts::statuses(harness)?;
    let harnesses: Vec<Value> = accounts::ACCOUNT_HARNESSES
        .iter()
        .filter(|id| harness.is_none_or(|only| only == **id))
        .map(|id| {
            json!({
                "id": id,
                "pools": accounts::pools(id),
                "quotaSource": accounts::quota_source(id),
                "accounts": statuses
                    .iter()
                    .filter(|account| account.harness == *id)
                    .map(|account| {
                        let mut value = json!(account);
                        if let Some(error) = read_errors.get(&format!("{id}/{}", account.id)) {
                            value["readError"] = error.clone();
                        }
                        value
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(Json(json!({ "harnesses": harnesses })))
}

pub(super) async fn list(Query(query): Query<AccountsQuery>) -> ApiResult {
    accounts_payload(query.harness.as_deref(), query.refresh.unwrap_or(0) == 1).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewAccount {
    harness: String,
    id: String,
    #[serde(default)]
    label: Option<String>,
}

pub(super) async fn add(Json(req): Json<NewAccount>) -> ApiResult {
    accounts::add(&req.harness, &req.id, req.label).map_err(bad_request)?;
    accounts_payload(None, false).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccountUpdate {
    harness: String,
    id: String,
    #[serde(flatten)]
    patch: accounts::AccountPatch,
}

pub(super) async fn update(Json(req): Json<AccountUpdate>) -> ApiResult {
    accounts::update(&req.harness, &req.id, req.patch).map_err(bad_request)?;
    accounts_payload(None, false).await
}

#[derive(Deserialize)]
pub(super) struct AccountRef {
    harness: String,
    id: String,
}

pub(super) async fn remove(Json(req): Json<AccountRef>) -> ApiResult {
    accounts::remove(&req.harness, &req.id).map_err(bad_request)?;
    accounts_payload(None, false).await
}

/// The harness's own sign-in command, run with the account's home.
fn login_program(harness: &str) -> Option<(std::path::PathBuf, Vec<String>)> {
    use crate::local::harness::{antigravity, claude, codex};
    match harness {
        "claude-code" => Some((claude::find_claude()?, vec!["auth".into(), "login".into()])),
        "codex" => Some((codex::find_codex()?, vec!["login".into()])),
        "antigravity" => Some((antigravity::find_agy()?, Vec::new())),
        _ => None,
    }
}

/// Sign an extra account in: the harness's login runs in a terminal whose
/// config home is the account's folder.
pub(super) async fn login(
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    Query(req): Query<AccountRef>,
) -> Response {
    if let Some(rejected) = super::reject_cross_origin(&headers) {
        return rejected;
    }
    if accounts::find(&req.harness, &req.id).is_none() || req.id == accounts::DEFAULT {
        return bad_request("Unknown account").into_response();
    }
    ws.on_upgrade(move |mut socket| async move {
        let mut size = super::DEFAULT_PTY_SIZE;
        let result = run_login(&mut socket, &req, &mut size).await;
        let message = match result {
            Ok(()) => json!({ "type": "complete" }),
            Err(error) => json!({ "type": "error", "error": error }),
        };
        let _ = socket.send(Message::Text(message.to_string().into())).await;
        super::close_socket(&mut socket).await;
    })
}

async fn run_login(
    socket: &mut axum::extract::ws::WebSocket,
    req: &AccountRef,
    size: &mut portable_pty::PtySize,
) -> std::result::Result<(), String> {
    let (program, args) = login_program(&req.harness)
        .ok_or_else(|| "Install this harness first, then add accounts.".to_string())?;
    let (harness, id) = (req.harness.clone(), req.id.clone());
    let env = tokio::task::spawn_blocking(move || accounts::login_env(&harness, &id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    let program = program.to_string_lossy().into_owned();
    let pty_size = *size;
    let session = tokio::task::spawn_blocking(move || {
        super::start_pty_with_env(&program, args, &env, pty_size, None)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    let mut output = String::new();
    let completed = (req.harness == "antigravity")
        .then_some(super::harness_setup::antigravity_prompt_ready as fn(&str) -> bool);
    match super::relay_pty(socket, session, Some(&mut output), size, completed).await {
        Some(Ok(_)) | Some(Err(_)) if accounts::signed_in(&req.harness, &req.id) => Ok(()),
        Some(Ok(status)) if status.success() => Err(
            "The sign-in finished, but no saved credentials were found for this account.".into(),
        ),
        Some(Ok(status)) => Err(format!("Sign-in exited with code {}", status.exit_code())),
        Some(Err(error)) => Err(error),
        None => Err("Terminal disconnected".into()),
    }
}
