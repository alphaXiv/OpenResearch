//! The tools behind `orx mcp`: the stdio MCP server every agent harness
//! is given relays `tools/list` and `tools/call` here, authenticated as the
//! chat that harness runs.

use std::collections::{HashMap, HashSet};

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{harnesses_payload, reject_if_moving, ApiError, ApiResult, AppState, HarnessQuery};
use crate::error::{anyhow, bail, Result};
use crate::local::chat::{
    delegation, env_request, news, OrxDisplay, OrxKind, OrxSource, PromptAnswer, TurnOverrides,
    ORX_MESSAGE_PREFIX,
};
use crate::local::model_routes;
use crate::store::search::SearchRequest;
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
                    "worktree": { "type": "string", "enum": ["shared", "own"], "description": "`shared` (the default) works in this chat's worktree. `own` gets a fresh worktree from the project's baseline and stays your delegate; commit first and name the commit in `task` if it should start from your work." },
                    "notify": { "type": "boolean", "description": "Default true. False starts a chat that is not your delegate, which you do not hear from unless you `subscribe`: a side chat of yours, or with `worktree: own` a top-level chat in the user's sidebar. Start a top-level chat only when the user asks for one." },
                    "canDelegate": { "type": "boolean", "description": "Whether the delegate may start delegates of its own. Omit to use the user's setting, which is off unless they turned it on; set it only when the task needs nested delegates." },
                    "clientRequestId": request_id,
                },
                "required": ["model", "task"],
            },
        },
        {
            "name": "subscribe",
            "description": "Hear from chats, not only the ones you delegated to: each time one finishes a turn, or a delegate asks a question, it arrives in this chat as a new message, or through `wait`. Returns each chat's state now; news starts from there.",
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
            "name": "chats",
            "description": "Find chats around this one, one line each: id, title, project, model and what it is doing. Give one of: `view` for the top of the user's sidebar, `query` to search every chat's messages, Done ones included, or `relation` for the chats linked to `of` in the tree of chats that started one another. Walk the tree by passing a listed chat as `of`. `status` and `read` tell more about one chat.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "view": { "type": "string", "enum": ["inbox", "working"], "description": "`inbox`: chats not at work, such as those waiting on the user or finished, newest first. `working`: chats whose agent, delegates or experiments are at work." },
                    "query": { "type": "string", "description": "Words to find in chats' messages, at least one of 3 or more characters." },
                    "relation": { "type": "string", "enum": ["parent", "children", "siblings", "tree"], "description": "`parent`: the chat that started `of`. `children`: the chats `of` started. `siblings`: the other chats its parent started. `tree`: every chat descended from the chat the user started, indented." },
                    "of": { "type": "string", "description": "The chat `relation` starts from, such as `chat_…`. Default this chat." },
                    "limit": { "type": "integer", "description": "How many chats. Default 20, at most 50." },
                },
            },
        },
        {
            "name": "send",
            "description": "Send a message to any chat, such as one you delegated to, the chat that delegated to you, or a side chat's parent. It runs as that chat's next turn, or joins its running turn when its agent accepts steering. Refs such as `msg_3fa9c1d2` in the message expand to the messages they cite.",
            "inputSchema": {
                "type": "object",
                "properties": { "chatId": chat_id, "message": { "type": "string" }, "clientRequestId": request_id },
                "required": ["chatId", "message"],
            },
        },
        {
            "name": "answer",
            "description": "Answer a question a delegate chat asked: choose among its option labels, write an answer, or both.",
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
            "name": "request_env",
            "description": format!("Ask the user to set environment variables, such as an API key or a setting your code reads. The user types the values on a card in this chat and chooses where to save them for your shell, your runs and later chats. The tool result omits secret values. Waits until the user answers, at most {} minutes. Use this instead of asking for a key in chat or reading one from a file.", longest / 60),
            "inputSchema": {
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Why you need them, in a sentence the user reads on the card." },
                    "variables": {
                        "type": "array",
                        "maxItems": env_request::MAX_VARIABLES,
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "The variable's name, such as `WANDB_API_KEY`." },
                                "description": { "type": "string", "description": "What it is and where the user finds it, such as \"From wandb.ai/authorize\". Saved with the variable." },
                                "secret": { "type": "boolean", "description": "Default true. False for a setting that is not sensitive, such as a project name: the user sees it as typed and you are told its value." },
                                "pattern": { "type": "string", "description": "Optional regex the value should match, such as `^[0-9a-f]{40}$`. A value that does not match only gets a warning." },
                                "example": { "type": "string", "description": "Optional placeholder showing the value's form, never a real value." },
                            },
                            "required": ["name"],
                        },
                    },
                },
                "required": ["variables"],
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
            let models = models(state, None).await?;
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
        "chats" => chats(state, caller, args).await,
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
                Some(result)
                    if result.queued
                        && !crate::local::harness::supports_steering(&target.harness) =>
                {
                    format!(
                        "Queued for chat `{}`'s next turn: its agent takes no messages mid-turn.",
                        target.id
                    )
                }
                Some(result) if result.queued => {
                    format!("Queued for chat `{}`'s next turn.", target.id)
                }
                Some(_) => format!("Chat `{}` started a turn with your message.", target.id),
            })
        }
        "answer" => {
            let store = Store::open()?;
            let target = chat(&store, text(args, "chatId")?)?;
            let prompt_id = text(args, "promptId")?;
            let pending = delegation::pending_prompt(&store, &target.id)?
                .filter(|pending| pending.id == prompt_id)
                .ok_or_else(|| anyhow!("Chat `{}` has no open question `{prompt_id}`; call `status` for its current one.", target.id))?;
            match pending.kind() {
                "question" if delegation::asks_agents(&store, &target.id, &pending)? => {}
                "question" => bail!(
                    "Chat `{}` is not a delegate, so only the user answers its questions.",
                    target.id
                ),
                "env" => bail!(
                    "Only the user can set the variables chat `{}` asks for.",
                    target.id
                ),
                _ => bail!("Only the user can approve a step; `answer` answers questions."),
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
                    env: Vec::new(),
                })
                .await?;
            Ok(format!("Answered chat `{}`; it continues.", target.id))
        }
        "request_env" => {
            let variables = env_request::parse_variables(args)?;
            if let Some(error) = variables
                .iter()
                .find_map(|var| super::env_key_error(&var.name))
            {
                bail!("{error}");
            }
            state
                .chat
                .request_env(
                    &caller.id,
                    optional_text(args, "reason").map(str::to_string),
                    variables,
                    std::time::Duration::from_secs(longest_wait(&caller.harness)),
                )
                .await
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
            if state.chat.interrupt_and_hold(&target.id).await? {
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
    let models = models(state, optional_text(args, "route")).await?;
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
    let started = crate::store::now_ms();
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
        // The agent reads a message steered into this turn only once this call returns.
        let steered = state.chat.steered_since(&caller.id, started);
        if asked || done || steered || waiting.is_empty() || std::time::Instant::now() >= deadline {
            let mut told = Vec::new();
            for item in collected {
                if let Some(token) = item.claim(&store)? {
                    item.settle(&store, &token, true)?;
                    told.push(item);
                }
            }
            let mut text = if told.is_empty() && steered {
                "A message for this chat arrived; it follows this result.".to_string()
            } else if told.is_empty() {
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

async fn chats(state: &AppState, caller: &StoredChatSession, args: &Value) -> Result<String> {
    let view = optional_text(args, "view");
    let query = optional_text(args, "query");
    let relation = optional_text(args, "relation");
    if [view, query, relation].iter().flatten().count() != 1 {
        bail!("Give one of `view`, `query` or `relation`.");
    }
    if relation.is_none() && optional_text(args, "of").is_some() {
        bail!("`of` goes with `relation`.");
    }
    let limit = args
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .clamp(1, 50) as usize;

    let activity = Activity::read(state).await?;
    let store = Store::open()?;
    let listing = Listing {
        activity,
        projects: store
            .list_local_projects()?
            .into_iter()
            .map(|project| (project.id, project.name))
            .collect(),
        caller: &caller.id,
        store: &store,
    };

    let mut lines = Vec::new();
    let heading = match (view, query, relation.unwrap_or_default()) {
        (Some("inbox"), _, _) => {
            let mut before: Option<(i64, String)> = None;
            'pages: loop {
                let page = store.list_sidebar_chat_sessions(
                    Some(false),
                    before.as_ref().map(|(at, id)| (*at, id.as_str())),
                )?;
                let more = page.len() > 50;
                for session in page.into_iter().take(50) {
                    before = Some((session.updated_at, session.id.clone()));
                    let (line, working) = listing.line(&session, None)?;
                    if !working {
                        lines.push(line);
                    }
                    if lines.len() > limit {
                        break 'pages;
                    }
                }
                if !more {
                    break;
                }
            }
            "The inbox, newest first:".to_string()
        }
        (Some("working"), _, _) => {
            let live: Vec<&str> = listing.activity.live().collect();
            let mut sessions = store.sidebar_chat_sessions_at_work(Some(false), &live)?;
            // Newest first by creation, as the sidebar holds Working still while chats stream.
            sessions.sort_by_key(|session| std::cmp::Reverse(session.created_at));
            for session in &sessions {
                let (line, working) = listing.line(session, None)?;
                if working {
                    lines.push(line);
                }
            }
            "At work, newest first:".to_string()
        }
        (Some(other), _, _) => bail!("`view` is `inbox` or `working`, not `{other}`."),
        (_, Some(query), _) => {
            let mut seen = HashSet::new();
            let mut cursor = None;
            let mut indexing = false;
            // One chat can match many messages, so page until enough distinct chats.
            while lines.len() <= limit {
                let request: SearchRequest = serde_json::from_value(
                    json!({ "q": query, "scope": "all", "limit": 100, "cursor": cursor }),
                )?;
                let page = store.search_chat_messages(&request.prepare()?)?;
                if page.requires_longer_query {
                    bail!("Search with at least one word of 3 or more characters.");
                }
                indexing |= page.indexing;
                for hit in page.hits {
                    if !seen.insert(hit.session_id.clone()) {
                        continue;
                    }
                    let Some(session) = store.get_chat_session(&hit.session_id)? else {
                        continue;
                    };
                    let snippet = hit.snippet.split_whitespace().collect::<Vec<_>>().join(" ");
                    let (line, _) = listing.line(&session, None)?;
                    lines.push(format!("{line}\n  > {snippet}"));
                }
                cursor = page.cursor;
                if cursor.is_none() {
                    break;
                }
            }
            if indexing {
                "Chats matching, newest first. Older chats are still being indexed, so some may be missing:".to_string()
            } else {
                "Chats matching, newest first:".to_string()
            }
        }
        (None, None, relation) => {
            let of = chat(&store, optional_text(args, "of").unwrap_or(&caller.id))?;
            match relation {
                "parent" => match parent(&store, &of.id)? {
                    Some((id, kind)) => {
                        let (line, _) = listing.line(&chat(&store, &id)?, None)?;
                        lines.push(line);
                        format!("Chat `{}` is a {kind} of:", of.id)
                    }
                    None => format!(
                        "Chat `{}` was started by the user; it has no parent.",
                        of.id
                    ),
                },
                "children" => {
                    for (id, kind) in children(&store, &of.id)? {
                        if let Some(child) = store.get_chat_session(&id)? {
                            lines.push(listing.line(&child, Some(kind))?.0);
                        }
                    }
                    format!("Chats `{}` started:", of.id)
                }
                "siblings" => {
                    let Some((parent, _)) = parent(&store, &of.id)? else {
                        return Ok(format!(
                            "Chat `{}` was started by the user; it has no siblings.",
                            of.id
                        ));
                    };
                    for (id, kind) in children(&store, &parent)? {
                        if id == of.id {
                            continue;
                        }
                        if let Some(sibling) = store.get_chat_session(&id)? {
                            lines.push(listing.line(&sibling, Some(kind))?.0);
                        }
                    }
                    format!("Other chats `{parent}` started:")
                }
                "tree" => {
                    let mut root = of.id.clone();
                    let mut seen = HashSet::from([root.clone()]);
                    while let Some((id, _)) = parent(&store, &root)? {
                        if !seen.insert(id.clone()) {
                            break;
                        }
                        root = id;
                    }
                    let mut stack = vec![(root, 0, None)];
                    let mut seen = HashSet::new();
                    while let Some((id, depth, kind)) = stack.pop() {
                        if !seen.insert(id.clone()) {
                            continue;
                        }
                        let Some(session) = store.get_chat_session(&id)? else {
                            continue;
                        };
                        let (line, _) = listing.line(&session, kind)?;
                        lines.push(format!("{}{line}", "  ".repeat(depth)));
                        let below = children(&store, &id)?;
                        stack.extend(
                            below
                                .into_iter()
                                .rev()
                                .map(|(child, kind)| (child, depth + 1, Some(kind))),
                        );
                    }
                    format!("The tree of chats `{}` belongs to:", of.id)
                }
                other => bail!(
                    "`relation` is `parent`, `children`, `siblings` or `tree`, not `{other}`."
                ),
            }
        }
    };

    if lines.is_empty() {
        return Ok(format!("{heading}\n\nNone."));
    }
    let more = lines.len() > limit;
    lines.truncate(limit);
    let mut text = format!("{heading}\n\n{}", lines.join("\n"));
    if more {
        text.push_str("\n\nMore chats follow; raise `limit` to see them.");
    }
    Ok(text)
}

/// What the sidebar knows of every chat's work, read once per listing.
struct Activity {
    busy: HashSet<String>,
    background: HashSet<String>,
    runs: HashSet<String>,
}

impl Activity {
    async fn read(state: &AppState) -> Result<Self> {
        Ok(Self {
            busy: state.chat.busy_sessions().await.into_iter().collect(),
            background: state.chat.waiting_on().await?.into_keys().collect(),
            runs: Store::open()?
                .chat_active_runs()?
                .into_iter()
                .map(|(chat, _)| chat)
                .collect(),
        })
    }

    /// Chats whose agent or delegates are at work; experiments the store finds itself.
    fn live(&self) -> impl Iterator<Item = &str> {
        self.busy.union(&self.background).map(String::as_str)
    }

    /// What `session` is doing, and whether that puts it in the sidebar's
    /// Working section: a chat waiting on the user stays in the inbox.
    fn state(&self, store: &Store, session: &StoredChatSession) -> Result<(String, bool)> {
        let busy = self.busy.contains(&session.id);
        if store.chat_session_awaiting_input(&session.id, busy)? {
            let text = match delegation::pending_prompt(store, &session.id)? {
                Some(pending) if pending.kind() == "question" => {
                    format!("asks a question, prompt `{}`", pending.id)
                }
                Some(pending) => format!("waits for the user to {}", pending.awaits()),
                None => "waits for the user".to_string(),
            };
            return Ok((text, false));
        }
        let at_work = if self.runs.contains(&session.id) {
            Some("running an experiment")
        } else if busy {
            Some("working")
        } else if self.background.contains(&session.id) {
            Some("waiting on background work or its delegates")
        } else {
            None
        };
        if let Some(text) = at_work {
            return Ok((text.to_string(), true));
        }
        let text = match store.latest_chat_turn(&session.id)? {
            _ if session.archived => "done",
            Some(turn) if turn.state == "failed" => "failed",
            Some(_) => "idle",
            None => "not started",
        };
        Ok((text.to_string(), false))
    }
}

struct Listing<'a> {
    activity: Activity,
    projects: HashMap<String, String>,
    caller: &'a str,
    store: &'a Store,
}

impl Listing<'_> {
    /// One chat's line, and whether it is at work.
    fn line(&self, session: &StoredChatSession, kind: Option<&str>) -> Result<(String, bool)> {
        let (state, working) = self.activity.state(self.store, session)?;
        let title = session
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map_or_else(|| "untitled".to_string(), |title| format!("\"{title}\""));
        let kind = kind.map(|kind| format!(" ({kind})")).unwrap_or_default();
        let project = self
            .projects
            .get(&session.project_id)
            .map_or(session.project_id.as_str(), String::as_str);
        let model = session.model.as_deref().unwrap_or(&session.harness);
        let you = if session.id == self.caller {
            " (you)"
        } else {
            ""
        };
        Ok((
            format!(
                "- `{}` {title}{kind} in {project}, {model}: {state}{you}",
                session.id
            ),
            working,
        ))
    }
}

/// The chat that started `id`, and whether `id` is its delegate or its side chat.
fn parent(store: &Store, id: &str) -> Result<Option<(String, &'static str)>> {
    if let Some(parent) = store.delegation(id)?.parent {
        return Ok(Some((parent, "delegate")));
    }
    Ok(store
        .get_chat_session(id)?
        .and_then(|session| session.side_parent_session_id)
        .map(|parent| (parent, "side chat")))
}

/// The chats `id` started; a delegate sharing its worktree counts as a delegate.
fn children(store: &Store, id: &str) -> Result<Vec<(String, &'static str)>> {
    let delegates = store.chat_delegates_of(id)?;
    let sides: Vec<String> = store
        .side_chats_of(id)?
        .into_iter()
        .filter(|side| !delegates.contains(side))
        .collect();
    Ok(delegates
        .into_iter()
        .map(|id| (id, "delegate"))
        .chain(sides.into_iter().map(|id| (id, "side chat")))
        .collect())
}

async fn models(state: &AppState, requested: Option<&str>) -> Result<Vec<model_routes::Model>> {
    let preferences = Store::open()?.ui_state()?;
    let query = HarnessQuery {
        refresh: None,
        retry: None,
        harness: None,
    };
    let mut catalog = harnesses_payload(state, &query).await;
    model_routes::filter_agents(&mut catalog, &preferences.hidden_agents, requested)
        .map_err(|error| anyhow!(error))?;
    Ok(model_routes::models(&catalog, &preferences.hidden_models))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::SteerMessage;
    use crate::store::StoredChatMessage;

    fn add_chat(store: &Store, id: &str, title: &str, side_parent: Option<&str>, archived: bool) {
        store
            .create_chat_session(&StoredChatSession {
                profile_id: None,
                id: id.into(),
                project_id: "proj_tree".into(),
                harness: "codex".into(),
                native_session_id: None,
                native_fork_at: None,
                title: Some(title.into()),
                title_source: Some("user".into()),
                model: Some("gpt-test".into()),
                service_tier: None,
                permission_mode: None,
                plan_mode: false,
                plan_reset_pending: false,
                reasoning_level: None,
                archived,
                context_usage_json: None,
                bootstrap_context: None,
                goal: None,
                autonomy: None,
                active_leaf_id: None,
                side_parent_session_id: side_parent.map(str::to_string),
                side_parent_leaf_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
    }

    /// `root` started `a` (a delegate in its worktree), `b` (a delegate in its own) and
    /// `side` (a side chat); `a` started `leaf`.
    fn tree(store: &Store) {
        store
            .create_local_project(&crate::store::tests::local_project_fixture("proj_tree"))
            .unwrap();
        add_chat(store, "root", "Root", None, false);
        add_chat(store, "a", "Delegate A", Some("root"), false);
        add_chat(store, "b", "Delegate B", None, false);
        add_chat(store, "side", "Side", Some("root"), false);
        add_chat(store, "leaf", "Leaf", Some("a"), false);
        store.record_chat_delegate("a", "root", true).unwrap();
        store.record_chat_delegate("b", "root", true).unwrap();
        store.record_chat_delegate("leaf", "a", false).unwrap();
    }

    async fn list(state: &AppState, caller: &str, args: Value) -> Result<String> {
        let caller = Store::open()
            .unwrap()
            .get_chat_session(caller)
            .unwrap()
            .unwrap();
        call(state, &caller, "chats", &args).await
    }

    fn ids(text: &str) -> Vec<&str> {
        text.lines()
            .filter_map(|line| line.trim_start().strip_prefix("- `"))
            .filter_map(|line| line.split('`').next())
            .collect()
    }

    #[tokio::test]
    async fn relations_walk_the_tree_of_chats_that_started_one_another() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let state = super::super::tests::test_state();
        tree(&Store::open().unwrap());

        let parent = list(&state, "leaf", json!({ "relation": "parent" }))
            .await
            .unwrap();
        assert_eq!(ids(&parent), ["a"]);
        assert!(parent.contains("delegate of"), "{parent}");

        let children = list(
            &state,
            "leaf",
            json!({ "relation": "children", "of": "root" }),
        )
        .await
        .unwrap();
        assert_eq!(ids(&children), ["a", "b", "side"]);
        assert!(
            children.contains("`side` \"Side\" (side chat)"),
            "{children}"
        );

        let siblings = list(&state, "leaf", json!({ "relation": "siblings", "of": "a" }))
            .await
            .unwrap();
        assert_eq!(ids(&siblings), ["b", "side"]);

        let tree = list(&state, "leaf", json!({ "relation": "tree" }))
            .await
            .unwrap();
        assert_eq!(ids(&tree), ["root", "a", "leaf", "b", "side"]);
        let leaf = tree.lines().find(|line| line.contains("- `leaf`")).unwrap();
        assert!(
            leaf.starts_with("    - ") && leaf.ends_with("(you)"),
            "{tree}"
        );

        let orphan = list(&state, "root", json!({ "relation": "siblings" }))
            .await
            .unwrap();
        assert!(
            ids(&orphan).is_empty() && orphan.contains("started by the user"),
            "{orphan}"
        );
    }

    #[tokio::test]
    async fn views_split_open_chats_into_inbox_and_working_and_search_reaches_done() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let state = super::super::tests::test_state();
        let store = Store::open().unwrap();
        tree(&store);
        add_chat(&store, "idle", "Idle", None, false);
        add_chat(&store, "busy", "Busy", None, false);
        add_chat(&store, "done", "Finished work", None, true);
        state.chat.hold_turn("busy").await;
        store
            .upsert_chat_message(&StoredChatMessage {
                id: "msg_done".into(),
                session_id: "done".into(),
                role: "user".into(),
                parts_json:
                    json!([{ "id": "p", "type": "text", "text": "tune the flamingo scheduler" }])
                        .to_string(),
                created_at: 1,
                completed_at: Some(1),
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: None,
                native_harness: None,
                result_native_message_id: None,
            })
            .unwrap();
        // More newer matches than one search page holds, all in one chat.
        for i in 0..120 {
            store
                .upsert_chat_message(&StoredChatMessage {
                    id: format!("msg_busy_{i}"),
                    session_id: "busy".into(),
                    role: "user".into(),
                    parts_json: json!([{ "id": "p", "type": "text", "text": "feed the flamingo" }])
                        .to_string(),
                    created_at: 10 + i,
                    completed_at: Some(10 + i),
                    parent_id: None,
                    base_native_session_id: None,
                    result_native_session_id: None,
                    native_harness: None,
                    result_native_message_id: None,
                })
                .unwrap();
        }

        let inbox = list(&state, "root", json!({ "view": "inbox" }))
            .await
            .unwrap();
        let listed = ids(&inbox);
        assert!(
            listed.contains(&"idle") && listed.contains(&"root"),
            "{inbox}"
        );
        assert!(
            !listed
                .iter()
                .any(|id| ["busy", "done", "a", "leaf"].contains(id)),
            "{inbox}"
        );

        let working = list(&state, "root", json!({ "view": "working" }))
            .await
            .unwrap();
        assert_eq!(ids(&working), ["busy"]);
        assert!(working.contains(": working"), "{working}");

        let found = list(&state, "root", json!({ "query": "flamingo" }))
            .await
            .unwrap();
        assert_eq!(ids(&found), ["busy", "done"]);
        assert!(
            found.contains(": done") && found.contains("flamingo scheduler"),
            "{found}"
        );

        let error = list(&state, "root", json!({ "view": "inbox", "query": "x" }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("one of"), "{error}");
        let error = list(&state, "root", json!({ "query": "ab" }))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("3 or more"), "{error}");
    }

    #[tokio::test]
    async fn wait_returns_when_a_message_is_steered_into_the_waiting_turn() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let state = super::super::tests::test_state();
        let store = Store::open().unwrap();
        tree(&store);
        store.subscribe_chat("root", "a", None).unwrap();
        state.chat.hold_turn("root").await;
        state.chat.hold_turn("a").await;

        let caller = store.get_chat_session("root").unwrap().unwrap();
        let waiting = tokio::spawn({
            let state = state.clone();
            async move { wait(&state, &caller, &json!({ "timeoutSeconds": 30 })).await }
        });
        tokio::task::yield_now().await;
        let steer = SteerMessage {
            id: "steer_1".into(),
            display: "stop".into(),
            text: "stop".into(),
            client_turn_id: None,
            orx: None,
            // Stamped before the wait began: what counts is when the steer was queued.
            sent_at: 0,
        };
        state.chat.queue_steer("root", &steer, &store).unwrap();
        // Codex reads a steer as soon as it accepts it, before `wait` looks again.
        assert!(state.chat.forget_steer("root", &steer.id));

        let text = tokio::time::timeout(std::time::Duration::from_secs(10), waiting)
            .await
            .expect("wait should return once a steer arrives")
            .unwrap()
            .unwrap();
        assert!(text.contains("A message for this chat arrived"), "{text}");
        assert!(text.contains("Still working: `a`"), "{text}");
    }
}
