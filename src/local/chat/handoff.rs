use std::{collections::HashSet, fmt::Write};

use super::{active_path, delegation, stored_to_wire, WirePart};
use crate::error::Result;
use crate::store::{Store, StoredChatMessage, StoredChatSession};

pub(super) const SWITCH_TOOL: &str = "provider_switch";
const MAX_TOKENS: usize = 24_000;

/// UTF-8 bytes upper-bound text tokens without assuming another provider's tokenizer.
pub(super) fn budget(context_window: Option<u64>) -> usize {
    context_window
        .map(|window| (window / 10) as usize)
        .unwrap_or(MAX_TOKENS)
        .min(MAX_TOKENS)
}

fn shorten(text: &str, bytes: usize) -> String {
    if text.len() <= bytes {
        return text.to_string();
    }
    let marker = "\n… [read this message by ref for the rest] …\n";
    if bytes < marker.len() {
        return String::new();
    }
    let retained = bytes - marker.len();
    let mut head = retained / 2;
    let mut tail = text.len() - (retained - head);
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    format!("{}{marker}{}", &text[..head], &text[tail..])
}

fn tool_counts(parts: &[WirePart]) -> usize {
    parts
        .iter()
        .map(|part| usize::from(part.kind == "tool") + tool_counts(&part.children))
        .sum()
}

/// User words survive before answer text; tool output stays behind its message ref.
pub(crate) fn snapshot<'a>(
    messages: impl IntoIterator<Item = &'a StoredChatMessage>,
    bytes: usize,
) -> String {
    let messages: Vec<_> = messages.into_iter().collect();
    let users: Vec<_> = messages
        .iter()
        .filter(|message| {
            message.role == "user"
                || stored_to_wire(message)
                    .parts
                    .iter()
                    .any(|part| part.kind == "steer")
        })
        .map(|message| {
            let wire = stored_to_wire(message);
            let mut text = if message.role == "user" {
                delegation::message_text(message)
            } else {
                String::new()
            };
            for part in &wire.parts {
                if part.kind == "steer" {
                    if let Some(steer) = &part.text {
                        let _ = write!(text, "\n{steer}");
                    }
                }
                if part.kind == "image" {
                    if let Some(name) = &part.text {
                        let _ = write!(text, "\nAttachment: {name} (read this ref for its path)");
                    }
                }
            }
            (
                format!(
                    "{} {}:\n",
                    delegation::message_ref(&message.id),
                    if message.role == "user" {
                        delegation::author(message)
                    } else {
                        "the user, during this turn".into()
                    }
                ),
                text,
            )
        })
        .collect();
    let latest = messages.iter().rev().find(|message| {
        message.role == "assistant" && !delegation::message_text(message).is_empty()
    });
    let mut entries: Vec<_> = users
        .iter()
        .map(|(label, text)| format!("{label}{text}\n\n"))
        .collect();
    let mut used = entries.iter().map(String::len).sum::<usize>();
    if used > bytes {
        let overhead = users
            .iter()
            .map(|(label, _)| label.len() + 2)
            .sum::<usize>();
        if overhead > bytes {
            let index = messages
                .iter()
                .filter(|message| message.role == "user")
                .map(|message| delegation::message_ref(&message.id))
                .collect::<Vec<_>>()
                .join(" ");
            let label = "User request refs (read each for the original words):\n";
            if label.len() + index.len() <= bytes {
                return format!("{label}{index}");
            }
            let cursor = messages
                .last()
                .map(|message| delegation::message_ref(&message.id))
                .unwrap_or_default();
            return shorten(&format!("Read the full conversation with the orx MCP tool `read {{\"before\":\"{cursor}\",\"limit\":50}}`, then keep paging backward. The user request index exceeds this handoff's budget."), bytes);
        }
        let allowance = bytes.saturating_sub(overhead) / users.len().max(1);
        entries = users
            .iter()
            .map(|(label, text)| format!("{label}{}\n\n", shorten(text, allowance)))
            .collect();
        used = entries.iter().map(String::len).sum();
    }
    if let Some(message) = latest {
        let label = format!(
            "Latest answer — {} · {} tools:\n",
            delegation::message_ref(&message.id),
            tool_counts(&stored_to_wire(message).parts)
        );
        let remaining = bytes.saturating_sub(used + label.len() + 2);
        if remaining > 0 {
            let answer = shorten(&delegation::message_text(message), remaining);
            let entry = format!("{label}{answer}\n\n");
            used += entry.len();
            entries.push(entry);
        }
    }
    let mut turns = Vec::new();
    for message in messages
        .iter()
        .rev()
        .filter(|message| message.role == "assistant")
    {
        if latest.is_some_and(|latest| latest.id == message.id) {
            continue;
        }
        let parts = stored_to_wire(message).parts;
        if parts
            .iter()
            .any(|part| part.tool.as_deref() == Some(SWITCH_TOOL))
        {
            continue;
        }
        let text = delegation::message_text(message);
        let tools = tool_counts(&parts);
        let full = format!(
            "{}  {} · {} tools\n",
            delegation::message_ref(&message.id),
            text,
            tools
        );
        let line = if used + full.len() <= bytes {
            full
        } else {
            format!(
                "{}  {} · {} tools\n",
                delegation::message_ref(&message.id),
                delegation::preview(&text),
                tools
            )
        };
        if used + line.len() <= bytes {
            used += line.len();
            turns.push(line);
        }
    }
    turns.reverse();
    entries.extend(turns);
    entries.concat()
}

pub(super) fn pending<'a>(
    session: &StoredChatSession,
    path: &[&'a StoredChatMessage],
    undelivered: &HashSet<String>,
) -> Option<(Option<&'a str>, Option<u64>)> {
    let seen = path.iter().rposition(|message| {
        message.role == "assistant"
            && message.native_harness.as_deref() == Some(&session.harness)
            && session.native_session_id.is_some()
            && message.result_native_session_id == session.native_session_id
            && !undelivered.contains(&message.id)
    });
    let tail = &path[seen.map_or(0, |index| index + 1)..];
    let switched = tail.iter().any(|message| {
        message
            .native_harness
            .as_deref()
            .is_some_and(|harness| harness != session.harness)
            || stored_to_wire(message)
                .parts
                .iter()
                .any(|part| part.tool.as_deref() == Some(SWITCH_TOOL))
    });
    if !switched {
        return None;
    }
    let window = tail.iter().rev().find_map(|message| {
        stored_to_wire(message)
            .parts
            .into_iter()
            .find(|part| part.tool.as_deref() == Some(SWITCH_TOOL))
            .and_then(|part| part.state)
            .and_then(|state| state.input)
            .filter(|input| input["harness"].as_str() == Some(&session.harness))
            .and_then(|input| input["contextWindow"].as_u64())
    });
    Some((seen.map(|index| path[index].id.as_str()), window))
}

pub(crate) fn context(
    store: &Store,
    session: &StoredChatSession,
    excluded_message: Option<&str>,
    since: Option<&str>,
    context_window: Option<u64>,
) -> Result<String> {
    let messages = store.list_chat_messages(&session.id)?;
    let path = active_path(&messages, session.active_leaf_id.as_deref());
    let start = since
        .and_then(|id| path.iter().position(|message| message.id == id))
        .map_or(0, |index| index + 1);
    let selected: Vec<_> = path[start..]
        .iter()
        .copied()
        .filter(|message| Some(message.id.as_str()) != excluded_message)
        .collect();
    let prior = if since.is_none() {
        super::effective_bootstrap_context(store, session)?
    } else {
        None
    };
    if selected.is_empty() && prior.is_none() {
        return Ok(String::new());
    }
    let mut state = format!("Chat: `{}`\nProvider: {}\n", session.id, session.harness);
    if let Some(project) = store.get_local_project(&session.project_id)? {
        let cwd = crate::local::git::existing_session_worktree_path(&project, &session.id);
        let _ = writeln!(state, "Worktree: {}", cwd.display());
        if cwd.exists() {
            let branch = crate::local::git::current_branch(&cwd);
            let _ = writeln!(state, "Branch: {}", branch.as_deref().unwrap_or("detached"));
            if let Ok(files) = crate::local::git::changed_files(&cwd, "HEAD") {
                let _ = writeln!(
                    state,
                    "Uncommitted files: {}",
                    files
                        .iter()
                        .take(15)
                        .map(|file| file.path.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            if let Ok(commits) = crate::local::git::list_commits(&cwd, "HEAD", 15) {
                for commit in commits
                    .iter()
                    .filter(|commit| commit.committed_at * 1000 >= session.created_at)
                {
                    let _ = writeln!(state, "Commit: {} {}", commit.sha, commit.subject);
                }
            }
        }
    }
    for run in store
        .list_runs_by_project(&session.project_id)?
        .iter()
        .filter(|run| run.chat_session_id.as_deref() == Some(&session.id))
        .take(15)
    {
        let _ = writeln!(state, "Run: {} {}", run.id, run.status);
    }
    for child in store.chat_delegates_of(&session.id)? {
        let _ = writeln!(state, "Delegate: {child}");
    }
    for queued in store.list_queued_chat_messages_for_session(&session.id)? {
        let _ = writeln!(state, "Queued message: {}", queued.id);
    }
    if let Some(prompt) = delegation::pending_prompt(store, &session.id)? {
        if let Some(message) = path.iter().find(|message| {
            stored_to_wire(message)
                .parts
                .iter()
                .any(|part| part.id == prompt.id)
        }) {
            let _ = writeln!(
                state,
                "Unresolved {}: read {}",
                prompt.kind(),
                delegation::message_ref(&message.id)
            );
        }
    }
    if let Some(goal) = &session.goal {
        let _ = writeln!(state, "Goal: {goal}");
    }
    if let Some(prior) = prior {
        let _ = writeln!(state, "Earlier saved context:\n{prior}");
    }
    let state = shorten(&state, budget(context_window) / 3);
    let cli_read = r#"printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read","arguments":{"ref":"msg_…"}}}' | orx mcp"#;
    let header = format!("<orx-handoff>\nYou are continuing this chat. Same user, same worktree, same conversation. Answer the current user message after this block.{} Prior tool actions already happened; inspect their results before repeating work.\nRead history with the orx MCP tool `read {{\"ref\":\"msg_…\"}}` for a complete message and its tools, or `read {{\"before\":\"msg_…\"}}` to page backward.\nIf no orx MCP tool is exposed, run `{cli_read}` in this chat's shell, replacing the ref. The same arguments support paging.\n\nState now:\n{state}\nConversation:\n", if since.is_some() { " Your native session already contains the earlier conversation; this is what happened since." } else { "" });
    let footer = "</orx-handoff>";
    let transcript = snapshot(
        selected,
        budget(context_window).saturating_sub(header.len() + footer.len()),
    );
    Ok(shorten(
        &format!("{header}{transcript}{footer}"),
        budget(context_window),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::{MessagePhase, WireToolState};
    use serde_json::json;

    fn message(id: &str, role: &str, text: &str) -> StoredChatMessage {
        StoredChatMessage {
            id: format!("msg_{id}"),
            session_id: "chat_test".into(),
            role: role.into(),
            parts_json: serde_json::to_string(&[WirePart::text(id, text)]).unwrap(),
            created_at: 1,
            completed_at: Some(1),
            parent_id: None,
            base_native_session_id: None,
            result_native_session_id: None,
            native_harness: None,
        }
    }

    fn session() -> StoredChatSession {
        StoredChatSession {
            id: "chat_test".into(),
            project_id: "project_test".into(),
            harness: "claude-code".into(),
            native_session_id: None,
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
        }
    }

    fn switched(id: &str, from: &str, target: &str) -> StoredChatMessage {
        let mut message = message(id, "assistant", "");
        let mut part = WirePart::tool(id, SWITCH_TOOL, "completed", None);
        part.state.as_mut().unwrap().input =
            Some(json!({"harness":target,"contextWindow":100_000}));
        message.parts_json = serde_json::to_string(&[part]).unwrap();
        message.native_harness = Some(from.into());
        message
    }

    #[test]
    fn cut_order_preserves_user_words_and_latest_answer_before_turn_lines() {
        let users = [
            message("00000001", "user", "Keep the tests."),
            message("00000003", "user", "Only change the parser."),
        ];
        let old = message("00000002", "assistant", "Older plan.");
        let latest = message("00000004", "assistant", &"Latest answer. ".repeat(100));
        let path = [&users[0], &old, &users[1], &latest];
        let text = snapshot(path, 400);
        assert!(text.len() <= 400);
        assert!(text.contains("Keep the tests."));
        assert!(text.contains("Only change the parser."));
        assert!(text.contains("Latest answer."));
        assert!(!text.contains("Older plan."));
    }

    #[test]
    fn older_answer_details_are_available_without_reading_refs_when_they_fit() {
        let request = message("cccc0001", "user", "Keep the parser API.");
        let decision = message(
            "cccc0002",
            "assistant",
            "Both options passed.\nChoose the streaming parser to keep memory bounded.",
        );
        let latest = message("cccc0003", "assistant", "Ready to implement.");
        let text = snapshot([&request, &decision, &latest], 4000);
        assert!(text.contains("Choose the streaming parser to keep memory bounded."));
        assert_eq!(text.matches("Ready to implement.").count(), 1);
        assert!(text.len() <= 4000);
    }

    #[test]
    fn long_requests_keep_their_refs_and_utf8_head_and_tail() {
        let one = message(
            "11111111",
            "user",
            &format!("START {} END", "🦀研究".repeat(1000)),
        );
        let two = message(
            "22222222",
            "user",
            &format!("CORRECTION {} DONE", "é".repeat(5000)),
        );
        let text = snapshot([&one, &two], 600);
        assert!(text.len() <= 600);
        for value in [
            "msg_11111111",
            "msg_22222222",
            "START",
            "END",
            "CORRECTION",
            "DONE",
        ] {
            assert!(text.contains(value), "{value}: {text}");
        }
        assert_eq!(budget(Some(100_000)), 10_000);
        assert_eq!(budget(Some(1_000_000)), 24_000);
    }

    #[test]
    fn logs_stay_behind_refs_and_steering_survives() {
        let mut answer = message("33333333", "assistant", "");
        let mut tool = WirePart::tool("test", "bash", "completed", None);
        tool.state = Some(WireToolState {
            status: "completed".into(),
            input: Some(json!({"command":"cargo test"})),
            output: Some("NOISY OUTPUT".repeat(5000)),
            error: None,
            title: None,
        });
        let mut final_text = WirePart::text("final", "Tests passed.");
        final_text.phase = Some(MessagePhase::FinalAnswer);
        let mut steering = WirePart::text("steer", "Keep the public API.");
        steering.kind = "steer".into();
        answer.parts_json = serde_json::to_string(&[tool, steering, final_text]).unwrap();
        let text = snapshot([&answer], 2000);
        assert!(!text.contains("NOISY OUTPUT"));
        assert!(text.contains("1 tools"));
        assert!(text.contains("Keep the public API."));
        let full = delegation::read_message(&answer).unwrap();
        assert!(full.contains("NOISY OUTPUT"));
        assert!(full.contains("cargo test"));
    }

    #[test]
    fn returning_provider_gets_only_what_happened_since_its_last_reply() {
        let mut session = session();
        session.native_session_id = Some("claude_native".into());
        let user = message("44444444", "user", "original request");
        let mut claude = message("55555555", "assistant", "original answer");
        claude.native_harness = Some("claude-code".into());
        claude.result_native_session_id = session.native_session_id.clone();
        let out = switched("66666666", "claude-code", "codex");
        let changed = message("77777777", "user", "Use the new parser.");
        let mut codex = message("88888888", "assistant", "Parser updated.");
        codex.native_harness = Some("codex".into());
        codex.result_native_session_id = Some("codex_native".into());
        let back = switched("99999999", "codex", "claude-code");
        let path = [&user, &claude, &out, &changed, &codex, &back];
        assert_eq!(
            pending(&session, &path, &HashSet::new()),
            Some((Some("msg_55555555"), Some(100_000)))
        );
        let mut reply = message("aaaaaaaa", "assistant", "Continued.");
        reply.native_harness = Some("claude-code".into());
        reply.result_native_session_id = session.native_session_id.clone();
        let mut continued = path.to_vec();
        continued.push(&reply);
        assert_eq!(pending(&session, &continued, &HashSet::new()), None);
    }

    #[test]
    fn a_thread_created_before_delivery_is_not_a_history_cursor() {
        let mut session = session();
        session.native_session_id = Some("empty_thread".into());
        let request = message("eeee0001", "user", "Original task that was rejected.");
        let mut rejected = message("eeee0002", "assistant", "Unable to send.");
        rejected.native_harness = Some(session.harness.clone());
        rejected.result_native_session_id = session.native_session_id.clone();
        let out = switched("eeee0003", "claude-code", "codex");
        let back = switched("eeee0004", "codex", "claude-code");
        let path = [&request, &rejected, &out, &back];
        let undelivered = HashSet::from([rejected.id.clone()]);
        assert_eq!(
            pending(&session, &path, &undelivered),
            Some((None, Some(100_000)))
        );
    }

    #[test]
    fn a_side_chat_keeps_its_frozen_parent_context_when_switching_agents() {
        let dir = crate::local::git::TemporaryDirectory::new("handoff-side-chat").unwrap();
        let store = Store::open_at(dir.path().into()).unwrap();
        let mut parent = session();
        store.create_chat_session(&parent).unwrap();
        let original = message("bbbb0001", "user", "Compare the two parser designs.");
        store.upsert_chat_message_on_branch(&original).unwrap();
        parent.active_leaf_id = Some(original.id.clone());
        let mut side = super::super::side_chat_session(&parent, std::slice::from_ref(&original));
        side.harness = "codex".into();
        store.create_chat_session(&side).unwrap();

        let mut later = message("bbbb0002", "user", "LATER PARENT REQUEST");
        later.parent_id = Some(original.id);
        store.upsert_chat_message_on_branch(&later).unwrap();
        let fresh = context(&store, &side, None, None, Some(100_000)).unwrap();
        assert!(fresh.contains("Compare the two parser designs."));
        assert!(!fresh.contains("LATER PARENT REQUEST"));

        let mut request = message("bbbb0003", "user", "Explain the second design.");
        request.session_id = side.id.clone();
        store.upsert_chat_message_on_branch(&request).unwrap();
        let mut marker = switched("bbbb0004", "claude-code", "codex");
        marker.session_id = side.id.clone();
        store.upsert_chat_message_on_branch(&marker).unwrap();
        let side = store.get_chat_session(&side.id).unwrap().unwrap();
        let messages = store.list_chat_messages(&side.id).unwrap();
        let path = active_path(&messages, side.active_leaf_id.as_deref());
        let (since, window) = pending(&side, &path, &HashSet::new()).unwrap();
        let handoff = context(&store, &side, None, since, window).unwrap();
        assert!(handoff.contains("Compare the two parser designs."));
        assert!(handoff.contains("Explain the second design."));
        assert!(!handoff.contains("LATER PARENT REQUEST"));
    }

    #[test]
    fn history_reads_page_the_active_branch_and_expand_complete_messages() {
        let dir = crate::local::git::TemporaryDirectory::new("handoff-read").unwrap();
        let store = Store::open_at(dir.path().into()).unwrap();
        let session = session();
        store.create_chat_session(&session).unwrap();
        let first = message("aaaa0001", "user", "Original request.");
        let mut second = message("aaaa0002", "assistant", "Complete answer.");
        second.parent_id = Some(first.id.clone());
        let mut correction = message("aaaa0003", "user", "Correction on this branch.");
        correction.parent_id = Some(second.id.clone());
        let mut other = message("aaaa0004", "user", "WRONG BRANCH");
        other.parent_id = Some(first.id.clone());
        for row in [&first, &second, &correction, &other] {
            store.upsert_chat_message(row).unwrap();
        }
        store
            .set_chat_session_active_leaf(&session.id, Some(&correction.id))
            .unwrap();
        let page = delegation::transcript(&store, &session.id, 1, None).unwrap();
        assert!(page.contains("Correction on this branch."));
        assert!(page.contains("before"));
        assert!(!page.contains("WRONG BRANCH"));
        let older = delegation::transcript(&store, &session.id, 2, Some(&correction.id)).unwrap();
        assert!(older.contains("Original request."));
        assert!(older.contains("Complete answer."));
        let saved = store.get_chat_session(&session.id).unwrap().unwrap();
        let handoff = context(&store, &saved, None, Some(&second.id), Some(100_000)).unwrap();
        assert!(handoff.contains("Correction on this branch."));
        assert!(!handoff.contains("Original request."));
        assert!(!handoff.contains("WRONG BRANCH"));
        assert!(handoff.len() <= 10_000);
    }
}
