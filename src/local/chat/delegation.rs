//! Delegated chats. An agent hands a task to another model through the orx
//! MCP server; the delegate runs as a side chat of the delegating chat, or in
//! a worktree of its own, and the delegator subscribes to it (see `news`).
//! Agents cite messages by ref and read each other's state through here.

use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use std::sync::OnceLock;

use super::{
    attachments_dir, stored_to_wire, MessagePhase, OrxKind, OrxSource, WirePart, WirePrompt,
    ORX_MESSAGE_PREFIX,
};
use crate::error::{anyhow, Result};
use crate::local::harness::{self, PermissionMode};
use crate::local::model_routes::Route;
use crate::store::{now_ms, Store, StoredChatMessage, StoredChatSession};

/// A delegate's answer rides inline up to this size; `status` reads the rest.
const ANSWER_INLINE_BYTES: usize = 16 * 1024;

/// The first line of a delegate's answer, as the dashboard previews it.
const ANSWER_PREVIEW_CHARS: usize = 200;

/// The longest `wait`. Harnesses give the orx server a tool timeout past it,
/// so a wait ends on its own terms rather than the harness's.
pub const WAIT_MAX_SECONDS: u64 = 1800;
pub const TOOL_TIMEOUT_SECONDS: u64 = WAIT_MAX_SECONDS + 60;

/// Proves which chat a call to the orx MCP server speaks for. Derived from a
/// key that lives only in this process, so a respawned agent keeps its token
/// and nothing needs revoking: a deleted chat's token names no chat.
pub fn agent_token(session_id: &str) -> String {
    static KEY: OnceLock<String> = OnceLock::new();
    let key = KEY.get_or_init(|| format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()));
    Sha256::digest(format!("{key}:{session_id}").as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The orx MCP server one chat's agent spawns: this binary's `mcp` over
/// stdio, with the env that names the chat to the `orx up` on `up_port`.
pub struct McpServer {
    pub command: String,
    pub args: [&'static str; 1],
    pub env: [(&'static str, String); 3],
}

pub fn mcp_server(session_id: &str, up_port: u16) -> Result<McpServer> {
    let command = crate::paths::spawnable_exe()
        .map_err(|e| anyhow!("cannot resolve the orx binary for its MCP server: {e}"))?;
    Ok(McpServer {
        command: command.to_string_lossy().into_owned(),
        args: ["mcp"],
        env: [
            ("ORX_UP_PORT", up_port.to_string()),
            ("ORX_SESSION_ID", session_id.to_string()),
            ("ORX_AGENT_TOKEN", agent_token(session_id)),
        ],
    })
}

/// Claude Code caches a delegate like a main conversation (1h on a
/// subscription, at 2× the 5m write price). It gets the lifetime a native
/// subagent would instead, unless the user pinned the main-conversation TTL.
pub fn delegate_prompt_cache_ttl() -> Option<String> {
    let var = |key: &str| {
        crate::local::shell_env::var(key)
            .map(|value| value.to_string_lossy().into_owned())
            .or_else(|| {
                crate::config::list_synced_env()
                    .into_iter()
                    .find_map(|(name, value)| (name == key).then_some(value))
            })
    };
    if var("CLAUDE_CODE_PROMPT_CACHE_TTL").is_some() {
        return None;
    }
    Some(
        var("CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL").unwrap_or_else(|| {
            let one_hour = var("ENABLE_PROMPT_CACHING_1H").is_some_and(|v| v == "1" || v == "true");
            if one_hour { "1h" } else { "5m" }.to_string()
        }),
    )
}

/// How much a chat may do unasked, from asking (0) to bypassing (3). Plan
/// asks. `None` for a harness without permission modes.
fn permission_rank(session: &StoredChatSession) -> Option<u8> {
    let mode =
        harness::effective_permission_id(&session.harness, session.permission_mode.as_deref())
            .and_then(|id| harness::permission_mode_for(&session.harness, &id))?;
    Some(if session.plan_mode {
        0
    } else {
        mode_rank(mode)
    })
}

fn mode_rank(mode: PermissionMode) -> u8 {
    match mode {
        PermissionMode::Plan | PermissionMode::Ask => 0,
        PermissionMode::AcceptEdits => 1,
        PermissionMode::Auto => 2,
        PermissionMode::Bypass => 3,
    }
}

/// Whether `caller` may message `target` without widening what it can do: a
/// target that may do more unasked than its caller, or whose modes orx cannot
/// read, is the user's to steer.
pub fn within_reach(caller: &StoredChatSession, target: &StoredChatSession) -> bool {
    match (permission_rank(caller), permission_rank(target)) {
        (Some(caller), Some(target)) => target <= caller,
        _ => false,
    }
}

/// The broadest mode the delegate's harness offers that is no broader than
/// the delegator's. Plan narrows to asking: a delegate is sent to do its task.
/// A delegator whose modes orx cannot read gets a delegate that asks.
pub fn delegate_permission(parent: &StoredChatSession, harness_id: &str) -> Result<Option<String>> {
    let ceiling = permission_rank(parent).unwrap_or(0);
    let child =
        harness::chat_harness(harness_id).ok_or_else(|| anyhow!("unknown agent {harness_id}"))?;
    let choices = child.options().permission_modes;
    if choices.is_empty() {
        // Its own policy decides what it may do, which only a chat that may
        // do anything can stand behind.
        return if ceiling == mode_rank(PermissionMode::Bypass) {
            Ok(None)
        } else {
            Err(anyhow!(
                "{} has no permission modes orx can hold to this chat's; pick another `route`, or ask the user.",
                child.name()
            ))
        };
    }
    choices
        .iter()
        .filter_map(|choice| {
            PermissionMode::from_id(&choice.id)
                .filter(|mode| *mode != PermissionMode::Plan)
                .map(|mode| (mode_rank(mode), choice.id.clone()))
        })
        .filter(|(rank, _)| *rank <= ceiling)
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, id)| Some(id))
        .ok_or_else(|| {
            anyhow!(
                "{} has no permission mode as narrow as this chat's",
                child.name()
            )
        })
}

/// A delegate's chat: a side chat of its delegator, unless it gets a
/// worktree of its own and stands as a chat in the sidebar.
/// The id of a chat an agent starts. Given the agent's `clientRequestId` it
/// is fixed, so a retry finds the chat the first attempt started.
pub fn started_chat_id(parent: &StoredChatSession, request: Option<&str>) -> String {
    let uuid = match request {
        Some(request) => request_key(&parent.id, request),
        None => uuid::Uuid::new_v4(),
    };
    format!("chat_{uuid}")
}

/// What an agent's `clientRequestId` names: the same across retries and
/// restarts, and its own to each calling chat.
pub fn request_key(caller: &str, request: &str) -> uuid::Uuid {
    let digest = Sha256::digest(format!("{caller}:{request}").as_bytes());
    uuid::Uuid::from_bytes(
        digest[..16]
            .try_into()
            .expect("a SHA-256 digest has 16 bytes"),
    )
}

pub fn delegate_session(
    id: String,
    parent: &StoredChatSession,
    route: &Route,
    permission_mode: Option<String>,
    effort: Option<String>,
    title: Option<String>,
    own_worktree: bool,
) -> StoredChatSession {
    let now = now_ms();
    StoredChatSession {
        profile_id: route.profile.clone().or_else(|| {
            (route.harness == parent.harness)
                .then(|| parent.profile_id.clone())
                .flatten()
        }),
        id,
        project_id: parent.project_id.clone(),
        harness: route.harness.clone(),
        native_session_id: None,
        native_fork_at: None,
        title_source: title.is_some().then(|| "user".to_string()),
        title,
        model: Some(route.model.clone()),
        service_tier: None,
        permission_mode,
        plan_mode: false,
        plan_reset_pending: false,
        reasoning_level: effort,
        archived: false,
        context_usage_json: None,
        bootstrap_context: None,
        goal: None,
        autonomy: parent.autonomy.clone(),
        active_leaf_id: None,
        side_parent_session_id: (!own_worktree).then(|| parent.id.clone()),
        side_parent_leaf_id: None,
        created_at: now,
        updated_at: now,
    }
}

/// What a message says to another agent: a user's text, or an assistant's
/// final-answer parts, else its trailing text.
pub fn message_text(message: &StoredChatMessage) -> String {
    let parts = stored_to_wire(message).parts;
    let text = |part: &WirePart| part.text.clone().filter(|_| part.kind == "text");
    let chosen: Vec<String> = if message.role != "assistant" {
        parts.iter().filter_map(text).collect()
    } else {
        let finals: Vec<String> = parts
            .iter()
            .filter(|part| part.phase == Some(MessagePhase::FinalAnswer))
            .filter_map(text)
            .collect();
        if finals.is_empty() {
            let mut tail: Vec<String> = parts
                .iter()
                .rev()
                .take_while(|part| matches!(part.kind.as_str(), "text" | "reasoning"))
                .filter_map(text)
                .collect();
            tail.reverse();
            tail
        } else {
            finals
        }
    };
    chosen.join("\n\n").trim().to_string()
}

/// The text a turn ended with.
pub fn answer(store: &Store, assistant_message_id: &str) -> Result<String> {
    Ok(store
        .get_chat_message(assistant_message_id)?
        .map(|message| message_text(&message))
        .unwrap_or_default())
}

/// How many hex digits of a message's random id its ref carries.
const REF_HEX: usize = 8;

/// How an agent cites a message: the start of its id, like a short commit hash.
pub fn message_ref(message_id: &str) -> String {
    let len = "msg_".len() + REF_HEX;
    message_id.get(..len).unwrap_or(message_id).to_string()
}

/// Replace each ref an agent wrote with the message it cites, quoted with its
/// chat and attachments, so the receiving agent reads the exact words. One that
/// cites no message stays as written.
pub fn expand_refs(store: &Store, text: &str) -> Result<String> {
    let mut expanded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("msg_") {
        let end = start + "msg_".len() + REF_HEX;
        let cited = rest
            .get(start..end)
            .filter(|cited| is_ref(rest, start, cited))
            .map(|cited| cited_message(store, cited))
            .transpose()?
            .flatten()
            .map(|message| quote(&message))
            .transpose()?;
        match cited {
            Some(quote) => {
                expanded.push_str(rest[..start].trim_end());
                expanded.push_str(&quote);
                rest = rest[end..].trim_start();
            }
            None => {
                expanded.push_str(&rest[..start + 1]);
                rest = &rest[start + 1..];
            }
        }
    }
    expanded.push_str(rest);
    Ok(expanded.trim().to_string())
}

/// Whether `cited`, found at `start` of `text`, is a whole ref rather than
/// part of a longer id or word.
fn is_ref(text: &str, start: usize, cited: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let hex = &cited["msg_".len()..];
    hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && !text[..start].ends_with(word)
        && !text[start + cited.len()..].starts_with(word)
}

/// The one message a ref cites.
pub fn cited_message(store: &Store, cited: &str) -> Result<Option<StoredChatMessage>> {
    let Some(hex) = cited.strip_prefix("msg_") else {
        return Ok(None);
    };
    if let Some(message) = store.get_chat_message(cited)? {
        return Ok(Some(message));
    }
    let ids = store.chat_message_ids_with_prefix(hex)?;
    let [id] = ids.as_slice() else {
        return Ok(None);
    };
    store.get_chat_message(id)
}

fn quote(message: &StoredChatMessage) -> Result<String> {
    let mut body = message_text(message);
    let images: Vec<String> = stored_to_wire(message)
        .parts
        .into_iter()
        .filter(|part| part.kind == "image")
        .filter_map(|part| part.text)
        .collect();
    if !images.is_empty() {
        let dir = attachments_dir()?;
        body.push_str("\n\nAttached files, saved on disk at:");
        for name in images {
            body.push_str(&format!("\n- {}", dir.join(name).display()));
        }
    }
    let quoted: Vec<String> = body
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".into()
            } else {
                format!("> {line}")
            }
        })
        .collect();
    Ok(format!(
        "\n\n{}\n>\n> — {}, in chat `{}` ({})\n\n",
        quoted.join("\n"),
        author(message),
        message.session_id,
        message_ref(&message.id),
    ))
}

/// Who wrote a message, as an agent reads it.
pub fn author(message: &StoredChatMessage) -> String {
    let source = stored_to_wire(message)
        .parts
        .into_iter()
        .find_map(|part| part.orx);
    match source {
        Some(OrxSource {
            from: Some(from),
            kind: OrxKind::Task | OrxKind::Message | OrxKind::Answer | OrxKind::Question,
            ..
        }) => format!("chat `{from}`'s agent"),
        Some(_) => "orx".into(),
        None if message.role == "user" => "the user".into(),
        None => message
            .native_harness
            .as_ref()
            .map(|harness| format!("assistant ({harness})"))
            .unwrap_or_else(|| "assistant".into()),
    }
}

/// A question or approval a chat's latest turn waits on.
#[derive(Clone)]
pub struct PendingPrompt {
    pub id: String,
    pub prompt: WirePrompt,
}

impl PendingPrompt {
    /// The card's kind. OpenCode asks to leave Plan with a question card,
    /// which is the user's approval all the same.
    pub fn kind(&self) -> &str {
        if self.prompt.plan_exit {
            "plan"
        } else {
            &self.prompt.kind
        }
    }
}

/// Claude's approval bridge puts each card in an assistant message of its own
/// beside the turn's, so every message since the turn began is searched.
pub fn pending_prompt(store: &Store, session_id: &str) -> Result<Option<PendingPrompt>> {
    let Some(turn) = store.latest_chat_turn(session_id)? else {
        return Ok(None);
    };
    let mut messages = store.list_chat_messages_since(session_id, turn.created_at)?;
    messages
        .retain(|message| message.role == "assistant" && message.id != turn.assistant_message_id);
    messages.extend(store.get_chat_message(&turn.assistant_message_id)?);
    Ok(messages.iter().rev().find_map(|message| {
        stored_to_wire(message).parts.into_iter().find_map(|part| {
            let prompt = part.prompt.filter(|prompt| !prompt.resolved)?;
            Some(PendingPrompt {
                id: part.id,
                prompt,
            })
        })
    }))
}

pub(super) fn question_text(id: &str, model: &str, pending: &PendingPrompt) -> String {
    format!(
        "{ORX_MESSAGE_PREFIX}Chat `{id}` ({model}) asks a question and waits for an answer:\n\n{}\n\nAnswer with `answer` (chatId `{id}`, promptId `{}`), or ask the user.",
        question_body(&pending.prompt),
        pending.id
    )
}

/// A question as its asker put it: header, question and options.
pub(super) fn question_body(prompt: &WirePrompt) -> String {
    let mut lines = Vec::new();
    if let Some(header) = prompt.header.as_deref().filter(|h| !h.trim().is_empty()) {
        lines.push(format!("**{}**\n", header.trim()));
    }
    if let Some(question) = prompt.question.as_deref().filter(|q| !q.trim().is_empty()) {
        lines.push(format!("{}\n", question.trim()));
    }
    for option in &prompt.options {
        match option.description.as_deref().filter(|d| !d.is_empty()) {
            Some(description) => lines.push(format!("- {} — {description}", option.label)),
            None => lines.push(format!("- {}", option.label)),
        }
    }
    if prompt.multi_select {
        lines.push("(More than one may be chosen.)".into());
    }
    lines.join("\n").trim().to_string()
}

/// One chat as `viewer` sees it: working, asking, or idle with its answer.
pub fn status(
    store: &Store,
    target: &StoredChatSession,
    working: bool,
    viewer: &str,
) -> Result<String> {
    let model = target.model.clone().unwrap_or_default();
    let pending = pending_prompt(store, &target.id)?;
    let turn = store.latest_chat_turn(&target.id)?;
    let mut text = match (&pending, working, &turn) {
        (Some(pending), _, _) if pending.kind() == "question" => {
            question_text(&target.id, &model, pending)
        }
        (Some(pending), _, _) => format!(
            "Chat `{}` ({model}) waits for the user to approve {} in that chat.",
            target.id,
            pending.prompt.tool.as_deref().unwrap_or("a step")
        ),
        (None, true, _) => format!("Chat `{}` ({model}) is working.", target.id),
        (None, false, Some(turn)) => {
            let mut text = format!(
                "Chat `{}` ({model}) is idle; its latest turn {}.",
                target.id, turn.state
            );
            if let Some(error) = turn
                .error_message
                .as_deref()
                .filter(|_| turn.state != "completed")
            {
                text.push_str(&format!("\nError: {error}"));
            }
            text
        }
        (None, false, None) => format!("Chat `{}` ({model}) has not run a turn.", target.id),
    };
    let paused = store
        .list_chat_subscriptions(Some(viewer))?
        .into_iter()
        .any(|sub| sub.target_session_id == target.id && sub.paused);
    if paused {
        text.push_str(
            "\nThe user turned off its notifications to you; read it with `status` when you need it.",
        );
    }
    if let Some(turn) = turn.filter(|_| !working) {
        let answer = answer(store, &turn.assistant_message_id)?;
        if !answer.is_empty() {
            // In full: this is where a truncated answer points.
            text.push_str(&format!(
                "\n\nIts answer {}:\n\n{answer}",
                message_ref(&turn.assistant_message_id),
            ));
        }
    }
    Ok(text)
}

/// A chat's recent messages, oldest first, each with the ref that cites it.
pub fn transcript(
    store: &Store,
    session_id: &str,
    limit: usize,
    before: Option<&str>,
) -> Result<String> {
    let messages = store.list_chat_messages(session_id)?;
    let session = store.get_chat_session(session_id)?;
    let path = super::active_path(
        &messages,
        session
            .as_ref()
            .and_then(|session| session.active_leaf_id.as_deref()),
    );
    let end = match before {
        Some(id) => path
            .iter()
            .position(|message| message.id == id)
            .ok_or_else(|| anyhow!("The message is not on this chat's active branch."))?,
        None => path.len(),
    };
    let selected: Vec<_> = path[..end]
        .iter()
        .copied()
        .filter(|message| matches!(message.role.as_str(), "user" | "assistant"))
        .rev()
        .take(limit)
        .collect();
    let recent: Vec<String> = selected
        .iter()
        .rev()
        .map(|message| super::handoff::snapshot([*message], 8 * 1024))
        .collect();
    Ok(if recent.is_empty() {
        format!("Chat `{session_id}` has no messages yet.")
    } else {
        let mut text = recent.join("\n\n");
        if end > selected.len() {
            if let Some(oldest) = selected.last() {
                text.push_str(&format!(
                    "\n\nOlder messages: `read {{\"before\":\"{}\",\"limit\":{limit}}}`.",
                    message_ref(&oldest.id)
                ));
            }
        }
        text
    })
}

pub fn read_message(message: &StoredChatMessage) -> Result<String> {
    fn visible_parts(parts: Vec<WirePart>) -> Vec<WirePart> {
        parts
            .into_iter()
            .filter(|part| part.kind != "reasoning")
            .map(|mut part| {
                part.children = visible_parts(part.children);
                part
            })
            .collect()
    }

    let parts = visible_parts(stored_to_wire(message).parts);
    let mut text = format!(
        "{} {}, in chat `{}`:\n{}",
        message_ref(&message.id),
        author(message),
        message.session_id,
        serde_json::to_string(&parts)?
    );
    if parts.iter().any(|part| part.kind == "image") {
        text.push_str(&quote(message)?);
    }
    Ok(text)
}

/// A delegate's link to the chat that started it, for the dashboard: whether
/// it tells that chat when it finishes, where it stands, and its answer.
pub fn session_links(store: &Store, session: &StoredChatSession) -> Result<Value> {
    let delegation = store.delegation(&session.id)?;
    let Some(parent) = delegation.parent else {
        return Ok(Value::Null);
    };
    let notifies = store
        .list_chat_subscriptions(Some(&parent))?
        .into_iter()
        .any(|sub| sub.target_session_id == session.id && !sub.paused);
    let turn = store.latest_chat_turn(&session.id)?;
    let answer = turn
        .as_ref()
        .filter(|turn| turn.state != "running")
        .map(|turn| answer(store, &turn.assistant_message_id))
        .transpose()?
        .filter(|answer| !answer.is_empty())
        .map(|answer| preview(&answer));
    let error = turn
        .as_ref()
        .filter(|turn| turn.state == "failed")
        .and_then(|turn| turn.error_message.as_deref())
        .map(preview);
    let asking = pending_prompt(store, &session.id)?.map(|pending| pending.kind().to_string());
    Ok(json!({
        "parentId": parent,
        "depth": delegation.depth,
        "notifies": notifies,
        "state": turn.map(|turn| turn.state),
        "answer": answer,
        "error": error,
        "asking": asking,
    }))
}

/// The user keeps a delegate for themselves: it stops telling the chat that
/// started it, and stands as that chat's side chat, or on its own when it has
/// its own worktree. The delegator's agent can still read it.
pub fn release_delegate(store: &Store, id: &str) -> Result<Option<StoredChatSession>> {
    let Some(session) = store.get_chat_session(id)? else {
        return Ok(None);
    };
    let parent = store
        .delegation(id)?
        .parent
        .ok_or_else(|| anyhow!("This chat is not a delegate."))?;
    store.pause_chat_subscription(&parent, id)?;
    store.remove_chat_delegate(id)?;
    Ok(Some(session))
}

/// The first line of text, without the Markdown markers that lead it.
pub fn preview(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let line = line
        .trim_start_matches(['#', '>', '-', '*', '_', ' '])
        .trim_end_matches(['#', '*', '_', ' ']);
    line.chars().take(ANSWER_PREVIEW_CHARS).collect()
}

/// A started chat's first message: who sent it and, for a delegate, where it
/// sits, so nesting stays deliberate, then the task it was given.
pub fn delegate_task(task: &str, parent: &str, delegate: Option<(u32, bool)>) -> String {
    let Some((depth, may_delegate)) = delegate else {
        return format!(
            "{ORX_MESSAGE_PREFIX}Task from chat `{parent}`, which does not hear from you unless it subscribes; `send` it what it must know.\n\n{task}"
        );
    };
    let levels = if depth == 1 {
        "1 level".to_string()
    } else {
        format!("{depth} levels")
    };
    let nesting = if may_delegate {
        "You may start delegates of your own with `delegate`, but keep nesting shallow unless the task calls for it."
    } else {
        "You cannot start delegates of your own; do the work yourself."
    };
    format!(
        "{ORX_MESSAGE_PREFIX}Task from chat `{parent}`. You are its delegate, {levels} below a chat the user started, and your final message is your answer to it. {nesting}\n\n{task}"
    )
}

pub(super) fn inline(answer: &str) -> String {
    if answer.len() <= ANSWER_INLINE_BYTES {
        return answer.to_string();
    }
    let mut end = ANSWER_INLINE_BYTES;
    while !answer.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[Truncated; call `status` for the full answer.]",
        &answer[..end]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::news::{chat_news, News};
    use crate::store::StoredChatTurn;

    #[test]
    fn delegates_inherit_the_profile_or_use_the_explicit_route_profile() {
        let mut parent = chat("claude-code", "auto");
        parent.profile_id = Some("claude-012345abcd".into());
        let mut route = Route {
            profile: None,
            name: "claude-code".into(),
            harness: "claude-code".into(),
            model: "sonnet".into(),
            free: false,
            efforts: Vec::new(),
        };
        let child = delegate_session("child".into(), &parent, &route, None, None, None, false);
        assert_eq!(child.profile_id, parent.profile_id);
        route.profile = Some("claude-abcdefghij".into());
        assert_eq!(
            delegate_session("child".into(), &parent, &route, None, None, None, false).profile_id,
            route.profile
        );
        route.harness = "codex".into();
        route.profile = None;
        assert!(
            delegate_session("child".into(), &parent, &route, None, None, None, false)
                .profile_id
                .is_none()
        );
    }

    fn chat(harness: &str, permission_mode: &str) -> StoredChatSession {
        let route = Route {
            profile: None,
            name: harness.into(),
            harness: harness.into(),
            model: "m".into(),
            free: false,
            efforts: Vec::new(),
        };
        let root = StoredChatSession {
            profile_id: None,
            id: "chat_root".into(),
            project_id: "proj".into(),
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
            created_at: 0,
            updated_at: 0,
        };
        delegate_session(
            "chat_child".into(),
            &root,
            &route,
            Some(permission_mode.into()),
            None,
            None,
            false,
        )
    }

    #[test]
    fn a_delegate_never_gets_a_broader_mode_than_its_delegator() {
        for (parent, mode, child, expected) in [
            ("claude-code", "acceptEdits", "codex", "ask"),
            ("claude-code", "auto", "codex", "approve-for-me"),
            ("claude-code", "auto", "opencode", "auto-approve"),
            ("claude-code", "plan", "codex", "ask"),
            ("codex", "full-access", "claude-code", "bypassPermissions"),
            ("opencode", "default", "claude-code", "manual"),
        ] {
            let got = delegate_permission(&chat(parent, mode), child).unwrap();
            assert_eq!(got.as_deref(), Some(expected), "{parent} {mode} → {child}");
        }
    }

    #[test]
    fn previews_are_the_first_line_without_markdown_markers() {
        assert_eq!(
            preview("\n# The History of the Abacus\n\nBody"),
            "The History of the Abacus"
        );
        assert_eq!(
            preview("**The Slipstick: A History**"),
            "The Slipstick: A History"
        );
        assert_eq!(preview("> quoted"), "quoted");
        assert_eq!(preview(&"x".repeat(500)).len(), ANSWER_PREVIEW_CHARS);
    }

    fn save_message(store: &Store, id: &str, session: &str, role: &str, parts: Vec<WirePart>) {
        store
            .upsert_chat_message(&StoredChatMessage {
                id: id.into(),
                session_id: session.into(),
                role: role.into(),
                parts_json: serde_json::to_string(&parts).unwrap(),
                created_at: 0,
                completed_at: None,
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: None,
                result_native_message_id: None,
                native_harness: None,
            })
            .unwrap();
    }

    #[test]
    fn a_ref_expands_to_the_cited_message_and_its_files() {
        let dir = std::env::temp_dir().join(format!("orx-refs-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let id = "msg_3fa9c1d2-0000-4000-8000-000000000000";
        save_message(
            &store,
            id,
            "chat_user",
            "user",
            vec![
                WirePart::text("p0", "Plot loss against width."),
                WirePart::image("img0", "shot.png"),
            ],
        );
        assert_eq!(message_ref(id), "msg_3fa9c1d2");
        let expanded = expand_refs(&store, "Do this: msg_3fa9c1d2 Thanks.").unwrap();
        assert!(expanded.starts_with("Do this:\n\n> Plot loss against width.\n>\n> Attached files"));
        assert!(expanded
            .ends_with("shot.png\n>\n> — the user, in chat `chat_user` (msg_3fa9c1d2)\n\nThanks."));
        assert_eq!(
            cited_message(&store, "msg_3fa9c1d2")
                .unwrap()
                .map(|message| message.session_id),
            Some("chat_user".into())
        );
        for untouched in [
            "Colour it #3fa9c1d2.",
            "msg_deadbeef",
            "msg_3fa9c1d2b",
            "msg_3fa9c1d2-0000-4000-8000-000000000000",
            "my_msg_3fa9c1d2",
            "msg_3fa9c1",
        ] {
            assert_eq!(expand_refs(&store, untouched).unwrap(), untouched);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_released_delegate_stops_telling_its_parent_and_stands_alone() {
        let dir = std::env::temp_dir().join(format!("orx-release-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let child = chat("opencode", "default");
        let mut parent = child.clone();
        parent.id = "chat_root".into();
        parent.side_parent_session_id = None;
        store.create_chat_session(&parent).unwrap();
        store.create_chat_session(&child).unwrap();
        store
            .record_chat_delegate(&child.id, &parent.id, false)
            .unwrap();
        store.subscribe_chat(&parent.id, &child.id, None).unwrap();
        assert_eq!(session_links(&store, &child).unwrap()["notifies"], true);

        release_delegate(&store, &child.id).unwrap();
        assert_eq!(session_links(&store, &child).unwrap(), Value::Null);
        let delegation = store.delegation(&child.id).unwrap();
        assert_eq!((delegation.depth, delegation.parent), (0, None));
        store.subscribe_chat(&parent.id, &child.id, None).unwrap();
        assert!(store.list_chat_subscriptions(Some(&parent.id)).unwrap()[0].paused);
        assert!(release_delegate(&store, &child.id).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_question_is_news_before_the_turn_that_asked_it() {
        let dir = std::env::temp_dir().join(format!("orx-news-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let child = chat("opencode", "default");
        let mut parent = child.clone();
        parent.id = "chat_root".into();
        parent.side_parent_session_id = None;
        store.create_chat_session(&parent).unwrap();
        store.create_chat_session(&child).unwrap();
        store
            .record_chat_delegate(&child.id, &parent.id, false)
            .unwrap();
        store.subscribe_chat(&parent.id, &child.id, None).unwrap();
        let turn = StoredChatTurn {
            id: "turn_1".into(),
            session_id: child.id.clone(),
            user_message_id: None,
            assistant_message_id: "msg_answer".into(),
            client_turn_id: "ct_1".into(),
            request_hash: "hash".into(),
            prepared_input: "task".into(),
            settings_json: "{}".into(),
            state: "preparing".into(),
            delivery_state: "not_sent".into(),
            attempt_count: 0,
            next_retry_at: None,
            error_kind: None,
            error_message: None,
            recovery_action: None,
            recovered_by_turn_id: None,
            created_at: 1,
            updated_at: 1,
        };
        store.admit_chat_turn(None, &turn).unwrap();
        let question = |resolved: bool| {
            let mut part = WirePart::text("q1", "");
            part.kind = "prompt".into();
            part.prompt = Some(WirePrompt {
                kind: "question".into(),
                question: Some("Which backend?".into()),
                resolved,
                ..Default::default()
            });
            save_message(
                &store,
                "msg_answer",
                &child.id,
                "assistant",
                vec![WirePart::text("t", "Before I start:"), part],
            );
        };
        let kinds = || -> Vec<&'static str> {
            chat_news(&store, Some(&parent.id))
                .unwrap()
                .iter()
                .map(|item| match item {
                    News::Answer(_) => "answer",
                    News::Question { .. } => "question",
                })
                .collect()
        };

        question(false);
        assert_eq!(kinds(), ["question"], "a held question is news mid-turn");
        store.complete_chat_turn("turn_1").unwrap();
        let asked = chat_news(&store, Some(&parent.id)).unwrap();
        assert_eq!(
            asked.len(),
            1,
            "the turn that ended asking is not its answer"
        );
        let token = asked[0].claim(&store).unwrap().unwrap();
        asked[0].settle(&store, &token, true).unwrap();
        assert!(kinds().is_empty(), "a told question waits for its answer");
        question(true);
        assert_eq!(kinds(), ["answer"]);
        let mut card = WirePart::text("perm", "");
        card.kind = "prompt".into();
        card.prompt = Some(WirePrompt {
            kind: "question".into(),
            question: Some("Which dataset?".into()),
            ..Default::default()
        });
        store
            .upsert_chat_message(&StoredChatMessage {
                id: "msg_perm".into(),
                session_id: child.id.clone(),
                role: "assistant".into(),
                parts_json: serde_json::to_string(&[card]).unwrap(),
                created_at: 2,
                completed_at: None,
                parent_id: None,
                base_native_session_id: None,
                result_native_session_id: None,
                result_native_message_id: None,
                native_harness: None,
            })
            .unwrap();
        assert_eq!(
            pending_prompt(&store, &child.id)
                .unwrap()
                .map(|pending| pending.id),
            Some("perm".into()),
            "Claude's bridge asks from a message of its own"
        );
        let plan_exit = WirePrompt {
            kind: "question".into(),
            plan_exit: true,
            ..Default::default()
        };
        let pending = PendingPrompt {
            id: "exit".into(),
            prompt: plan_exit,
        };
        assert_eq!(
            pending.kind(),
            "plan",
            "leaving Plan is the user's to approve"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_answer_is_the_final_text_or_else_the_trailing_text() {
        let dir = std::env::temp_dir().join(format!("orx-delegate-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let save = |id: &str, parts: Vec<WirePart>| {
            save_message(&store, id, "chat_child", "assistant", parts)
        };
        let mut commentary = WirePart::text("a", "Looking around.");
        commentary.phase = Some(MessagePhase::Commentary);
        let mut done = WirePart::text("c", "The answer is 42.");
        done.phase = Some(MessagePhase::FinalAnswer);
        let tool = WirePart::tool("b", "Bash", "completed", None);
        save("phased", vec![commentary.clone(), tool.clone(), done]);
        save(
            "plain",
            vec![commentary, tool, WirePart::text("d", "Plain ending.")],
        );
        assert_eq!(answer(&store, "phased").unwrap(), "The answer is 42.");
        assert_eq!(answer(&store, "plain").unwrap(), "Plain ending.");
        assert_eq!(answer(&store, "missing").unwrap(), "");
        let _ = std::fs::remove_dir_all(dir);
    }
}
