//! The tools behind `orx mcp`: the stdio MCP server every agent harness
//! is given relays `tools/list` and `tools/call` here, authenticated as the
//! chat that harness runs.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{harnesses_payload, reject_if_moving, ApiError, ApiResult, AppState, HarnessQuery};
use crate::error::{anyhow, bail, Result};
use crate::local::chat::{
    delegation, news, OrxDisplay, OrxKind, OrxSource, PromptAnswer, TurnOverrides,
    ORX_MESSAGE_PREFIX,
};
use crate::local::model_routes;
use crate::store::{Store, StoredChatSession};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentToolRequest {
    session_id: String,
    token: String,
    method: String,
    #[serde(default)]
    params: Value,
}

pub(super) async fn agent_tools(
    State(state): State<AppState>,
    Json(request): Json<AgentToolRequest>,
) -> ApiResult {
    if delegation::agent_token(&request.session_id) != request.token {
        return Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "unknown agent token".into(),
        ));
    }
    let caller = Store::open()?
        .get_chat_session(&request.session_id)?
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "unknown agent token".into()))?;
    match request.method.as_str() {
        "tools/list" => Ok(Json(json!({ "tools": tools(&caller.harness) }))),
        "tools/call" => {
            let name = request
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let args = request
                .params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            reject_if_moving(&state)?;
            let (text, is_error) = match call(&state, &caller, name, &args).await {
                Ok(text) => (text, false),
                Err(error) => (error.to_string(), true),
            };
            Ok(Json(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": is_error,
            })))
        }
        other => Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("unknown method {other}"),
        )),
    }
}

fn tools(harness: &str) -> Value {
    let longest = longest_wait(harness);
    let wait_timeout = format!(
        "Default {}, at most {longest}. Chats still working when it expires keep working.",
        WAIT_DEFAULT_SECONDS.min(longest)
    );
    let chat_id = json!({ "type": "string", "description": "A chat's id, such as `chat_…`." });
    let chat_ids = json!({ "type": "array", "items": { "type": "string" }, "description": "Chat ids, such as `chat_…`." });
    let request_id = json!({ "type": "string", "description": "Optional key you keep across retries of this call: a retry with the same key returns what the first call did instead of doing it again." });
    json!([
        {
            "name": "models",
            "description": "List the versioned models you can delegate to. Each line is a model, the routes that can run it in the order orx prefers, and the effort levels its first route takes.",
            "inputSchema": { "type": "object", "properties": {} },
        },
        {
            "name": "delegate",
            "description": "Start a new chat on another model to do a task. Name a versioned model from `models`; orx picks the route unless you name one. The delegate sees only `task`, so include everything it needs; a message's ref, such as `msg_3fa9c1d2`, in `task` passes that message on word for word, with its attachments. This returns at once and subscribes you to the delegate: its answer arrives in this chat as a new message when it finishes, or sooner through `wait`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "model": { "type": "string", "description": "A versioned model id from `models`, such as `deepseek-v4.1-flash`." },
                    "task": { "type": "string", "description": "The complete, standalone instructions for the delegate." },
                    "route": { "type": "string", "description": "Optional route from `models`, such as `openrouter`. Omit to use orx's order." },
                    "effort": { "type": "string", "description": "Optional effort level from `models`. Omit for the route's default." },
                    "title": { "type": "string", "description": "Optional short name for the delegate's chat." },
                    "worktree": { "type": "string", "enum": ["shared", "own"], "description": "`shared` (the default) works in this chat's worktree. `own` gets a fresh worktree from the project's baseline and appears in the sidebar; commit first and name the commit in `task` if it should start from your work." },
                    "notify": { "type": "boolean", "description": "Default true. False starts a chat that is not your delegate: a side chat of yours, or with `worktree: own` a chat of its own, which you do not hear from unless you `subscribe`." },
                    "canDelegate": { "type": "boolean", "description": "Whether the delegate may start delegates of its own. Omit to use the user's setting, which is off unless they turned it on; set it only when the task needs nested delegates." },
                    "clientRequestId": request_id,
                },
                "required": ["model", "task"],
            },
        },
        {
            "name": "subscribe",
            "description": "Hear from chats, not only the ones you delegated to: each time one finishes a turn or asks a question, it arrives in this chat as a new message, or through `wait`. Returns each chat's state now; news starts from there.",
            "inputSchema": { "type": "object", "properties": { "chatIds": chat_ids }, "required": ["chatIds"] },
        },
        {
            "name": "unsubscribe",
            "description": "Stop hearing from chats.",
            "inputSchema": { "type": "object", "properties": { "chatIds": chat_ids }, "required": ["chatIds"] },
        },
        {
            "name": "wait",
            "description": "Block until chats you subscribe to have news, and return it: a finished turn's answer, or a question to `answer`. Without `chatIds`, waits on every subscribed chat still working; naming a chat subscribes you to it. News that `wait` returns does not arrive again as a message. When you have nothing else to do, ending your turn works as well: the news arrives as a new message.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "chatIds": chat_ids,
                    "until": { "type": "string", "enum": ["any", "all"], "description": "`any` (the default) returns at the first news; `all` waits for an answer from every chat. A question returns at once either way." },
                    "timeoutSeconds": { "type": "integer", "description": wait_timeout },
                },
            },
        },
        {
            "name": "status",
            "description": "Read whether a chat is working, waiting on a question or an approval, and the answer its latest turn ended with.",
            "inputSchema": { "type": "object", "properties": { "chatId": chat_id }, "required": ["chatId"] },
        },
        {
            "name": "read",
            "description": "Read recent conversation messages with their authors, page backward with `before`, or read one message and its tool calls with `ref`. Message refs identify the chat, so chatId is optional with ref or before.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "chatId": chat_id,
                    "ref": { "type": "string", "description": "A message ref, such as msg_3fa9c1d2. Returns its visible message parts, tools and attachment paths." },
                    "before": { "type": "string", "description": "Read messages older than this ref on its active branch." },
                    "limit": { "type": "integer", "description": "How many messages, newest last. Default 10, at most 50." }
                },
            },
        },
        {
            "name": "send",
            "description": "Send a message to a chat you delegated to, or to any chat whose permissions are no broader than yours, such as the chat that delegated to you or a side chat's parent. It runs as that chat's next turn, or joins its running turn when its agent accepts steering. Refs such as `msg_3fa9c1d2` in the message expand to the messages they cite.",
            "inputSchema": {
                "type": "object",
                "properties": { "chatId": chat_id, "message": { "type": "string" }, "clientRequestId": request_id },
                "required": ["chatId", "message"],
            },
        },
        {
            "name": "answer",
            "description": "Answer a question a chat asked: choose among its option labels, write an answer, or both.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "chatId": chat_id,
                    "promptId": { "type": "string", "description": "The question's prompt id, from its news or `status`." },
                    "choices": { "type": "array", "items": { "type": "string" }, "description": "Option labels, exactly as given." },
                    "text": { "type": "string", "description": "A written answer, or a note beside the choices." },
                },
                "required": ["chatId", "promptId"],
            },
        },
        {
            "name": "cancel",
            "description": "Stop a chat you delegated to, and every chat it delegated to in turn. Their queued messages stay queued.",
            "inputSchema": { "type": "object", "properties": { "chatId": chat_id }, "required": ["chatId"] },
        },
    ])
}

/// How long `wait` blocks unless told otherwise.
const WAIT_DEFAULT_SECONDS: u64 = 600;

/// The longest `wait` under `harness`. Antigravity ends every MCP call after three minutes and
/// takes no longer timeout.
fn longest_wait(harness: &str) -> u64 {
    if harness == "antigravity" {
        170
    } else {
        delegation::WAIT_MAX_SECONDS
    }
}

async fn call(
    state: &AppState,
    caller: &StoredChatSession,
    name: &str,
    args: &Value,
) -> Result<String> {
    match name {
        "models" => {
            let models = model_routes::models(&catalog(state).await);
            if models.is_empty() {
                bail!("No agent is ready to run a delegate. The user can sign in to one in OpenResearch.");
            }
            Ok(model_routes::listing(&models))
        }
        "delegate" => delegate(state, caller, args).await,
        "subscribe" => {
            let mut lines = Vec::new();
            for id in chat_ids(args)? {
                let store = Store::open()?;
                let target = chat(&store, &id)?;
                if target.id == caller.id {
                    bail!("A chat cannot subscribe to itself.");
                }
                let told = store
                    .latest_chat_turn(&target.id)?
                    .filter(|turn| {
                        matches!(turn.state.as_str(), "completed" | "failed" | "interrupted")
                    })
                    .map(|turn| turn.id);
                store.subscribe_chat(&caller.id, &target.id, told.as_deref())?;
                let working = state.chat.is_busy(&target.id).await;
                lines.push(delegation::status(&store, &target, working, &caller.id)?);
            }
            Ok(format!("Subscribed.\n\n{}", lines.join("\n\n---\n\n")))
        }
        "unsubscribe" => {
            let store = Store::open()?;
            let ids = chat_ids(args)?;
            for id in &ids {
                store.unsubscribe_chat(&caller.id, id)?;
            }
            Ok(format!("Unsubscribed from {}.", ids.join(", ")))
        }
        "wait" => wait(state, caller, args).await,
        "status" => {
            let store = Store::open()?;
            let target = chat(&store, text(args, "chatId")?)?;
            let working = state.chat.is_busy(&target.id).await;
            delegation::status(&store, &target, working, &caller.id)
        }
        "read" => {
            let store = Store::open()?;
            let reference = optional_text(args, "ref");
            let before = optional_text(args, "before");
            if reference.is_some() && before.is_some() {
                bail!("Use either `ref` or `before`.");
            }
            let cited = reference
                .or(before)
                .map(|cited| {
                    delegation::cited_message(&store, cited)?
                        .ok_or_else(|| anyhow!("Message ref `{cited}` is missing or ambiguous."))
                })
                .transpose()?;
            if let Some(message) = &cited {
                if optional_text(args, "chatId").is_some_and(|id| id != message.session_id) {
                    bail!("The message belongs to a different chat.");
                }
                if reference.is_some() {
                    return delegation::read_message(message);
                }
            }
            let id = cited
                .as_ref()
                .map(|message| message.session_id.as_str())
                .or(optional_text(args, "chatId"))
                .unwrap_or(&caller.id);
            let target = chat(&store, id)?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(10)
                .clamp(1, 50);
            delegation::transcript(
                &store,
                &target.id,
                limit as usize,
                cited.as_ref().map(|message| message.id.as_str()),
            )
        }
        "send" => {
            let store = Store::open()?;
            let target = chat(&store, text(args, "chatId")?)?;
            reachable(&store, caller, &target)?;
            let written = text(args, "message")?;
            let message = format!(
                "{ORX_MESSAGE_PREFIX}Message from chat `{}`:\n\n{}",
                caller.id,
                delegation::expand_refs(&store, written)?
            );
            let shown = OrxDisplay {
                text: written.to_string(),
                source: OrxSource {
                    kind: OrxKind::Message,
                    from: Some(caller.id.clone()),
                    status: None,
                },
            };
            let client_turn_id = optional_text(args, "clientRequestId")
                .map(|key| format!("ct_send_{}", delegation::request_key(&caller.id, key)));
            let sent = state
                .chat
                .steer_message(
                    &target.id,
                    message,
                    TurnOverrides::default(),
                    Vec::new(),
                    Vec::new(),
                    client_turn_id,
                    Some(shown),
                )
                .await?;
            Ok(match sent {
                None => format!("Sent into chat `{}`'s running turn.", target.id),
                Some(result) if result.queued => {
                    format!("Queued for chat `{}`'s next turn.", target.id)
                }
                Some(_) => format!("Chat `{}` started a turn with your message.", target.id),
            })
        }
        "answer" => {
            let store = Store::open()?;
            let target = chat(&store, text(args, "chatId")?)?;
            reachable(&store, caller, &target)?;
            let prompt_id = text(args, "promptId")?;
            let pending = delegation::pending_prompt(&store, &target.id)?
                .filter(|pending| pending.id == prompt_id)
                .ok_or_else(|| anyhow!("Chat `{}` has no open question `{prompt_id}`; call `status` for its current one.", target.id))?;
            if pending.kind() != "question" {
                bail!("Only the user can approve a step; `answer` answers questions.");
            }
            let choices: Vec<String> = args
                .get("choices")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if let Some(unknown) = choices.iter().find(|choice| {
                !pending
                    .prompt
                    .options
                    .iter()
                    .any(|option| &option.label == *choice)
            }) {
                bail!("`{unknown}` is not one of the question's options.");
            }
            let note = optional_text(args, "text").map(str::to_string);
            if choices.is_empty() && note.is_none() {
                bail!("Give `choices`, `text`, or both.");
            }
            state
                .chat
                .respond(PromptAnswer {
                    session_id: target.id.clone(),
                    prompt_id: prompt_id.to_string(),
                    approve: true,
                    resume_mode: None,
                    answers: choices,
                    note,
                    annotations: Vec::new(),
                    from: Some(caller.id.clone()),
                })
                .await?;
            Ok(format!("Answered chat `{}`; it continues.", target.id))
        }
        "cancel" => {
            let store = Store::open()?;
            let target = chat(&store, text(args, "chatId")?)?;
            if store.delegation(&target.id)?.parent.as_deref() != Some(caller.id.as_str()) {
                bail!(
                    "Only the chat that delegated to `{}` can stop it.",
                    target.id
                );
            }
            let mut stopped = Vec::new();
            if state.chat.interrupt(&target.id).await? {
                stopped.push(target.id.clone());
                news::mark_told(&store, &caller.id, &target.id)?;
            }
            stopped.extend(state.chat.stop_delegates(&target.id).await?);
            Ok(if stopped.is_empty() {
                format!("Chat `{}` and its delegates were not running.", target.id)
            } else {
                format!(
                    "Stopped {}.",
                    stopped
                        .iter()
                        .map(|id| format!("`{id}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        }
        other => bail!("Unknown tool `{other}`."),
    }
}

async fn delegate(state: &AppState, caller: &StoredChatSession, args: &Value) -> Result<String> {
    let store = Store::open()?;
    let delegation = store.delegation(&caller.id)?;
    if !delegation.may_delegate {
        bail!("This chat cannot start delegates: the chat that delegated to it did not allow nesting. Do the work yourself, or `send` the chat that delegated to you.");
    }
    let request = optional_text(args, "clientRequestId");
    let id = delegation::started_chat_id(caller, request);
    // A retry's first attempt succeeded once its task is in; until then the
    // retry finishes starting the chat that attempt created.
    if request.is_some() && store.latest_chat_turn(&id)?.is_some() {
        return Ok(format!(
            "Already started chat `{id}` for this clientRequestId."
        ));
    }
    let notify = args.get("notify").and_then(Value::as_bool).unwrap_or(true);
    let may_delegate = match args.get("canDelegate").and_then(Value::as_bool) {
        Some(allowed) => allowed,
        None => store.ui_state()?.nested_delegation,
    };
    let own_worktree = match optional_text(args, "worktree") {
        None | Some("shared") => false,
        Some("own") => true,
        Some(other) => bail!("`worktree` is `shared` or `own`, not `{other}`."),
    };
    let written = text(args, "task")?;
    let task = delegation::delegate_task(
        &delegation::expand_refs(&store, written)?,
        &caller.id,
        notify.then_some((delegation.depth + 1, may_delegate)),
    );
    let models = model_routes::models(&catalog(state).await);
    let route = model_routes::resolve(&models, text(args, "model")?, optional_text(args, "route"))
        .map_err(|e| anyhow!(e))?;
    let effort = optional_text(args, "effort");
    if let Some(effort) = effort.filter(|effort| !route.efforts.iter().any(|e| e == effort)) {
        bail!(if route.efforts.is_empty() {
            format!(
                "`{}` on {} takes no effort level; omit `effort`.",
                route.model, route.name
            )
        } else {
            format!(
                "`{effort}` is not an effort `{}` takes on {}: {}.",
                route.model,
                route.name,
                route.efforts.join(", ")
            )
        });
    }
    let _admission = state
        .project_lifecycle
        .admit(&caller.project_id)
        .ok_or_else(|| anyhow!("This project is being deleted."))?;
    let permission = delegation::delegate_permission(caller, &route.harness)?;
    let title = optional_text(args, "title").map(str::to_string);
    let child = delegation::delegate_session(
        id,
        caller,
        &route,
        permission,
        effort.map(str::to_string),
        title,
        own_worktree,
    );
    let created = match store.create_chat_session(&child) {
        Ok(()) => true,
        Err(_) if request.is_some() && store.get_chat_session(&child.id)?.is_some() => false,
        Err(error) => return Err(error),
    };
    // A chat its starter does not hear from stands as the user's, like a
    // delegate the user kept.
    if notify {
        if let Err(error) = store
            .record_chat_delegate(&child.id, &caller.id, may_delegate)
            .and_then(|()| store.subscribe_chat(&caller.id, &child.id, None))
        {
            if created {
                store.delete_chat_session(&child.id)?;
            }
            return Err(error);
        }
    }
    state.chat.emit_session(Some(child.clone())).await;
    let sent = state
        .chat
        .send_message(
            &child.id,
            task,
            TurnOverrides::default(),
            Vec::new(),
            Vec::new(),
            request.map(|_| format!("ct_{}", child.id)),
            Some(OrxDisplay {
                text: written.to_string(),
                source: OrxSource {
                    kind: OrxKind::Task,
                    from: Some(caller.id.clone()),
                    status: None,
                },
            }),
        )
        .await;
    if let Err(error) = sent {
        // A delegate that never got its task would never answer.
        if created {
            let _ = state.chat.delete_session(&child.id).await;
        }
        return Err(error);
    }
    let effort = effort
        .map(|effort| format!(" at {effort} effort"))
        .unwrap_or_default();
    Ok(if notify {
        format!(
            "Delegated to chat `{}` running `{}` on {}{effort}. Its answer arrives here as a new message when it finishes, or sooner through `wait`.",
            child.id, route.model, route.name
        )
    } else {
        format!(
            "Started chat `{}` running `{}` on {}{effort}. It is not your delegate and you are not subscribed to it; `subscribe` to hear from it.",
            child.id, route.model, route.name
        )
    })
}

/// Blocks until the caller's subscriptions have news, then tells it here
/// instead of in a wake: the news is marked told as it is returned.
async fn wait(state: &AppState, caller: &StoredChatSession, args: &Value) -> Result<String> {
    let all = match optional_text(args, "until") {
        None | Some("any") => false,
        Some("all") => true,
        Some(other) => bail!("`until` is `any` or `all`, not `{other}`."),
    };
    let timeout = args
        .get("timeoutSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(WAIT_DEFAULT_SECONDS)
        .clamp(1, longest_wait(&caller.harness));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout);
    let named = args
        .get("chatIds")
        .is_some()
        .then(|| chat_ids(args))
        .transpose()?;
    let mut waiting: Vec<String> = {
        let store = Store::open()?;
        if let Some(ids) = &named {
            for id in ids {
                chat(&store, id)?;
                if id == &caller.id {
                    bail!("A chat cannot wait on itself.");
                }
                store.subscribe_chat(&caller.id, id, None)?;
            }
        }
        store
            .list_chat_subscriptions(Some(&caller.id))?
            .into_iter()
            .filter(|sub| sub.outstanding && !sub.paused)
            .map(|sub| sub.target_session_id)
            .filter(|id| named.as_ref().is_none_or(|ids| ids.contains(id)))
            .collect()
    };
    if waiting.is_empty() {
        return Ok("Nothing to wait for: every chat you subscribe to has told you its latest answer, or the user turned off its notifications.".into());
    }
    let mut collected = Vec::new();
    let mut paused: Vec<String> = Vec::new();
    let mut gone: Vec<String> = Vec::new();
    loop {
        let store = Store::open()?;
        // A turn interrupted mid-wait reads nothing more; what it would have
        // been told arrives as a new message instead.
        if !state.chat.is_busy(&caller.id).await {
            return Ok("This turn ended before any news arrived.".into());
        }
        // Collected news is marked told only once returned, so a turn that
        // ends first still gets it as a new message.
        let mut asked = false;
        for item in news::chat_news(&store, Some(&caller.id))? {
            if !waiting.iter().any(|id| id == item.target())
                || (matches!(item, news::News::Answer(_))
                    && state.chat.is_busy(item.target()).await)
            {
                continue;
            }
            asked |= matches!(item, news::News::Question { .. });
            if matches!(item, news::News::Answer(_)) {
                waiting.retain(|id| id != item.target());
            }
            collected.push(item);
        }
        // A target is done waiting on when the user stopped it telling this
        // chat, another `wait` was told its news, or it is gone.
        let subscriptions = store.list_chat_subscriptions(Some(&caller.id))?;
        waiting.retain(|id| {
            let Some(sub) = subscriptions
                .iter()
                .find(|sub| &sub.target_session_id == id)
            else {
                gone.push(id.clone());
                return false;
            };
            if sub.paused {
                paused.push(id.clone());
            }
            sub.outstanding && !sub.paused
        });
        let done = if all {
            waiting.is_empty()
        } else {
            !collected.is_empty()
        };
        if asked || done || waiting.is_empty() || std::time::Instant::now() >= deadline {
            let mut told = Vec::new();
            for item in collected {
                if let Some(token) = item.claim(&store)? {
                    item.settle(&store, &token, true)?;
                    told.push(item);
                }
            }
            let mut text = if told.is_empty() {
                format!("No news after {timeout} seconds.")
            } else {
                news::messages(&store, &told)?
                    .into_iter()
                    .map(|(text, _)| text)
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n")
            };
            let list = |ids: &[String]| {
                ids.iter()
                    .map(|id| format!("`{id}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            if !paused.is_empty() {
                text.push_str(&format!(
                    "\n\nThe user turned off notifications from {}; read it with `status` when you need it.",
                    list(&paused)
                ));
            }
            if !gone.is_empty() {
                text.push_str(&format!(
                    "\n\nYou no longer subscribe to {}: it was deleted or unsubscribed.",
                    list(&gone)
                ));
            }
            if !waiting.is_empty() {
                text.push_str(&format!(
                    "\n\nStill working: {}. Wait again, or end your turn: their news arrives as new messages.",
                    list(&waiting)
                ));
            }
            return Ok(text);
        }
        drop(store);
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

async fn catalog(state: &AppState) -> Value {
    let query = HarnessQuery {
        refresh: None,
        retry: None,
        harness: None,
    };
    harnesses_payload(state, &query).await
}

fn optional_text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    optional_text(args, key).ok_or_else(|| anyhow!("`{key}` is required."))
}

fn chat_ids(args: &Value) -> Result<Vec<String>> {
    let ids: Vec<String> = args
        .get("chatIds")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() {
        bail!("`chatIds` needs at least one chat id.");
    }
    Ok(ids)
}

fn chat(store: &Store, id: &str) -> Result<StoredChatSession> {
    store
        .get_chat_session(id)?
        .ok_or_else(|| anyhow!("No chat `{id}` exists."))
}

/// Chats `caller` may change: those it is linked to by a delegation or a
/// side chat, and any other whose permissions are no broader than its own.
/// A chat may steer its own delegates, and any chat that can do no more
/// unasked than it can; a narrower chat steering a broader one would widen it.
fn reachable(store: &Store, caller: &StoredChatSession, target: &StoredChatSession) -> Result<()> {
    let delegated = store.delegation(&target.id)?.parent.as_deref() == Some(caller.id.as_str());
    if delegated || delegation::within_reach(caller, target) {
        Ok(())
    } else {
        bail!(
            "Chat `{}` may do more without asking than this chat, so only the user can steer it.",
            target.id
        )
    }
}
