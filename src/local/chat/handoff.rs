use std::{collections::HashSet, fmt::Write};

use super::{active_path, delegation, stored_to_wire, WirePart};
use crate::error::Result;
use crate::store::{Store, StoredChatMessage, StoredChatSession};

pub(super) const SWITCH_TOOL: &str = "provider_switch";
const MAX_TOKENS: usize = 24_000;

/// UTF-8 bytes upper-bound text tokens without assuming another provider's tokenizer.
pub(crate) fn budget(context_window: Option<u64>) -> usize {
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

/// Budget user words before answers; tool output stays behind message refs.
pub(crate) fn snapshot<'a>(
    messages: impl IntoIterator<Item = &'a StoredChatMessage>,
    bytes: usize,
) -> String {
    let messages: Vec<_> = messages.into_iter().collect();
    let users: Vec<_> = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.role == "user"
                || stored_to_wire(message)
                    .parts
                    .iter()
                    .any(|part| part.kind == "steer")
        })
        .map(|(index, message)| {
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
                index,
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
    let latest = messages.iter().enumerate().rev().find(|(_, message)| {
        message.role == "assistant" && !delegation::message_text(message).is_empty()
    });
    let mut entries: Vec<_> = users
        .iter()
        .map(|(index, label, text)| (*index, format!("{label}{text}\n\n")))
        .collect();
    let mut used = entries.iter().map(|(_, text)| text.len()).sum::<usize>();
    if used > bytes {
        let overhead = users
            .iter()
            .map(|(_, label, _)| label.len() + 2)
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
            .map(|(index, label, text)| {
                (*index, format!("{label}{}\n\n", shorten(text, allowance)))
            })
            .collect();
        used = entries.iter().map(|(_, text)| text.len()).sum();
    }
    if let Some((index, message)) = latest {
        let label = format!(
            "{} {} — latest answer · {} tools:\n",
            delegation::message_ref(&message.id),
            delegation::author(message),
            tool_counts(&stored_to_wire(message).parts)
        );
        let remaining = bytes.saturating_sub(used + label.len() + 2);
        if remaining > 0 {
            let answer = shorten(&delegation::message_text(message), remaining);
            let entry = format!("{label}{answer}\n\n");
            used += entry.len();
            entries.push((index, entry));
        }
    }
    for (index, message) in messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, message)| message.role == "assistant")
    {
        if latest.is_some_and(|(_, latest)| latest.id == message.id) {
            continue;
        }
        let parts = stored_to_wire(message).parts;
        let change = parts
            .iter()
            .find(|part| part.tool.as_deref() == Some(SWITCH_TOOL))
            .and_then(|part| part.state.as_ref()?.input.as_ref());
        let mut text = delegation::message_text(message);
        if let Some(change) = change {
            text = format!(
                "Agent switched: {} → {}.",
                change["fromHarness"]
                    .as_str()
                    .or(message.native_harness.as_deref())
                    .unwrap_or("unknown"),
                change["harness"].as_str().unwrap_or("unknown"),
            );
        } else if text.is_empty() {
            let error = parts
                .iter()
                .rev()
                .find_map(|part| part.state.as_ref()?.error.as_deref());
            text = match error {
                Some(error) => {
                    format!("No reply text; turn failed: {}", delegation::preview(error))
                }
                None => "No reply text recorded.".into(),
            };
        }
        let mut label = format!(
            "{} {}",
            delegation::message_ref(&message.id),
            if change.is_some() {
                "orx".into()
            } else {
                delegation::author(message)
            }
        );
        let tools = tool_counts(&parts);
        if tools > 0 && change.is_none() {
            let _ = write!(label, " · {tools} tools");
        }
        let full = format!("{label}:\n{text}\n\n");
        let line = if used + full.len() <= bytes {
            full
        } else {
            format!("{label}:\n{}\n\n", delegation::preview(&text))
        };
        if used + line.len() <= bytes {
            used += line.len();
            entries.push((index, line));
        }
    }
    entries.sort_by_key(|(index, _)| *index);
    entries.into_iter().map(|(_, text)| text).collect()
}

#[derive(Default)]
pub(crate) struct Position {
    pub since: Option<String>,
    pub context_window: Option<u64>,
    pub switched: bool,
}

pub(super) fn position(
    session: &StoredChatSession,
    path: &[&StoredChatMessage],
    unconfirmed: &HashSet<String>,
) -> Position {
    let seen = path.iter().rposition(|message| {
        message.role == "assistant"
            && message
                .native_harness
                .as_deref()
                .unwrap_or(&session.harness)
                == session.harness
            && session.native_session_id.is_some()
            && message.result_native_session_id == session.native_session_id
            && !unconfirmed.contains(&message.id)
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
    let context_window = tail.iter().rev().find_map(|message| {
        stored_to_wire(message)
            .parts
            .into_iter()
            .find(|part| part.tool.as_deref() == Some(SWITCH_TOOL))
            .and_then(|part| part.state)
            .and_then(|state| state.input)
            .filter(|input| input["harness"].as_str() == Some(&session.harness))
            .and_then(|input| input["contextWindow"].as_u64())
    });
    Position {
        since: seen.map(|index| path[index].id.clone()),
        context_window,
        switched,
    }
}

pub(crate) fn position_for(store: &Store, session: &StoredChatSession) -> Result<Position> {
    let Some(leaf) = session.active_leaf_id.as_deref() else {
        return Ok(Position::default());
    };
    let messages = store.list_chat_messages(&session.id)?;
    let path = active_path(&messages, Some(leaf));

    Ok(position(
        session,
        &path,
        &store.unconfirmed_chat_replies(&session.id)?,
    ))
}

pub(crate) fn context(
    store: &Store,
    session: &StoredChatSession,
    excluded_message: Option<&str>,
    since: Option<&str>,
    context_window: Option<u64>,
) -> Result<String> {
    let messages = store.list_chat_messages(&session.id)?;
    let path = session
        .active_leaf_id
        .as_deref()
        .map(|leaf| active_path(&messages, Some(leaf)))
        .unwrap_or_default();
    let seen = since.and_then(|id| path.iter().position(|message| message.id == id));
    let start = seen.map_or(0, |index| index + 1);
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
    let mut state = format!("Chat: `{}`\n", session.id);
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
    let header = format!(
        "<orx-handoff>\n\
         You are the {} agent, continuing the user's conversation in the same worktree. \
         Earlier assistant replies name the agent that wrote them; an agent switch \
         changes who is answering, not the conversation. Use the messages below to \
         answer the current user message after this block. Prior tool actions \
         already happened. Read history only for details missing below: \
         `read {{\"ref\":\"msg_…\"}}` returns that message and its tools; \
         `read {{\"before\":\"msg_…\"}}` returns older messages, excluding that message.\n\
         If no orx MCP tool is exposed, run `{cli_read}` in this chat's shell, \
         replacing the ref. The same arguments support paging.\n\n{}\
         State now:\n{state}\nConversation (oldest first):\n",
        session.harness,
        if let Some(index) = seen {
            let message = path[index];
            format!(
                "Your native session already contains the conversation through {}, your last reply as {}. \
                 The assistant replies in that native session are yours; this is what happened since.\n\n\
                 Your last reply, already seen ({}):\n> {}\n\n",
                delegation::message_ref(&message.id),
                session.harness,
                session.harness,
                delegation::preview(&delegation::message_text(message)),
            )
        } else {
            String::new()
        }
    );
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
    fn a_short_handoff_identifies_who_answered_without_a_history_read() {
        let request = message("cafe0001", "user", "What's your favorite video game?");
        let mut answer = message("cafe0002", "assistant", "Outer Wilds.");
        answer.native_harness = Some("claude-code".into());
        let change = switched("cafe0003", "claude-code", "codex");
        let text = snapshot([&request, &answer, &change], 4000);

        assert!(text.contains("assistant (claude-code)"));
        assert_eq!(text.matches("Outer Wilds.").count(), 1);
        assert!(text.contains("Agent switched: claude-code → codex."));
        assert!(text.find("What's your favorite").unwrap() < text.find("Outer Wilds.").unwrap());
        assert!(text.find("Outer Wilds.").unwrap() < text.find("Agent switched").unwrap());
    }

    #[test]
    fn an_unfinished_turn_has_a_source_and_failure_in_the_conversation() {
        let request = message("cafe0011", "user", "What's your favorite video game?");
        let mut answer = message("cafe0012", "assistant", "Portal 2.");
        answer.native_harness = Some("codex".into());
        let change = switched("cafe0013", "codex", "opencode");
        let followup = message("cafe0014", "user", "What did the other agent say?");
        let mut failed = message("cafe0015", "assistant", "");
        failed.native_harness = Some("opencode".into());
        let error = WirePart::tool(
            "error",
            "error",
            "error",
            Some("Usage limit reached.".into()),
        );
        failed.parts_json =
            serde_json::to_string(&[WirePart::reasoning("thinking", "PRIVATE_SENTINEL"), error])
                .unwrap();
        let back = switched("cafe0016", "opencode", "claude-code");
        let text = snapshot(
            [&request, &answer, &change, &followup, &failed, &back],
            4000,
        );

        assert!(text.contains("assistant (codex)"));
        assert!(text.contains("assistant (opencode)"));
        assert!(text.contains("No reply text; turn failed: Usage limit reached."));
        assert!(!text.contains("PRIVATE_SENTINEL"));
        let positions: Vec<_> = [
            "cafe0011", "cafe0012", "cafe0013", "cafe0014", "cafe0015", "cafe0016",
        ]
        .iter()
        .map(|id| text.find(id).unwrap())
        .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn reference_reads_keep_public_parts_and_tools_without_nested_reasoning() {
        let mut answer = message("cafe0021", "assistant", "");
        answer.native_harness = Some("opencode".into());
        let mut tool = WirePart::tool("run", "execute", "completed", None);
        tool.state.as_mut().unwrap().input = Some(json!({"command":"cargo test"}));
        tool.state.as_mut().unwrap().output = Some("42 tests passed.".into());
        let mut nested = WirePart::tool("delegate", "delegate", "completed", None);
        nested.children = vec![
            WirePart::reasoning("nested-thinking", "NESTED_PRIVATE_SENTINEL"),
            WirePart::text("nested-text", "Visible delegate answer."),
        ];
        tool.children = vec![
            WirePart::reasoning("child-thinking", "CHILD_PRIVATE_SENTINEL"),
            nested,
        ];
        answer.parts_json = serde_json::to_string(&[
            WirePart::reasoning("thinking", "PRIVATE_SENTINEL"),
            tool,
            WirePart::text("answer", "Visible answer."),
        ])
        .unwrap();
        let text = delegation::read_message(&answer).unwrap();

        assert!(text.contains("assistant (opencode)"));
        for visible in [
            "Visible answer.",
            "Visible delegate answer.",
            "cargo test",
            "42 tests passed.",
        ] {
            assert!(text.contains(visible));
        }
        assert!(!text.contains("PRIVATE_SENTINEL"));
        assert!(!text.contains("\"reasoning\""));
        assert!(answer.parts_json.contains("PRIVATE_SENTINEL"));
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
        let missed = position(&session, &path, &HashSet::new());
        assert_eq!(missed.since.as_deref(), Some("msg_55555555"));
        assert_eq!(missed.context_window, Some(100_000));
        assert!(missed.switched);
        let mut reply = message("aaaaaaaa", "assistant", "Continued.");
        reply.native_harness = Some("claude-code".into());
        reply.result_native_session_id = session.native_session_id.clone();
        let mut continued = path.to_vec();
        continued.push(&reply);
        let caught_up = position(&session, &continued, &HashSet::new());
        assert_eq!(caught_up.since.as_deref(), Some("msg_aaaaaaaa"));
        assert!(!caught_up.switched);
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
        let unconfirmed = HashSet::from([rejected.id.clone()]);
        let retry = position(&session, &[&request, &rejected], &unconfirmed);
        assert!(retry.since.is_none());
        assert!(!retry.switched);
        let path = [&request, &rejected, &out, &back];
        let returned = position(&session, &path, &unconfirmed);
        assert!(returned.since.is_none());
        assert_eq!(returned.context_window, Some(100_000));
        assert!(returned.switched);
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
        let position = position(&side, &path, &HashSet::new());
        let handoff = context(
            &store,
            &side,
            None,
            position.since.as_deref(),
            position.context_window,
        )
        .unwrap();
        assert!(handoff.contains("Compare the two parser designs."));
        assert!(handoff.contains("Explain the second design."));
        assert!(!handoff.contains("LATER PARENT REQUEST"));
    }

    #[test]
    fn an_empty_branch_does_not_reintroduce_discarded_requests_or_forks() {
        let dir = crate::local::git::TemporaryDirectory::new("handoff-empty-branch").unwrap();
        let store = Store::open_at(dir.path().into()).unwrap();
        let session = session();
        store.create_chat_session(&session).unwrap();
        for row in [
            message("feed0001", "user", "DISCARDED REQUEST"),
            switched("feed0002", "codex", "claude-code"),
            message("feed0003", "assistant", "DISCARDED ANSWER"),
            message("feed0004", "user", "OTHER FORK"),
        ] {
            store.upsert_chat_message_on_branch(&row).unwrap();
        }
        store
            .set_chat_session_active_leaf(&session.id, None)
            .unwrap();
        let empty = store.get_chat_session(&session.id).unwrap().unwrap();
        let position = position_for(&store, &empty).unwrap();
        assert!(position.since.is_none());
        assert!(!position.switched);
        assert!(context(&store, &empty, None, None, None)
            .unwrap()
            .is_empty());

        let replacement = message("feed0005", "user", "REPLACEMENT REQUEST");
        store.upsert_chat_message_on_branch(&replacement).unwrap();
        let edited = store.get_chat_session(&session.id).unwrap().unwrap();
        let fresh = context(&store, &edited, None, None, None).unwrap();
        assert!(fresh.contains("REPLACEMENT REQUEST"));
        assert!(!fresh.contains("DISCARDED"));
        assert!(!fresh.contains("OTHER FORK"));
    }

    #[test]
    fn a_compaction_summary_stays_on_its_branch_while_parent_context_survives() {
        for side in [false, true] {
            let dir =
                crate::local::git::TemporaryDirectory::new("handoff-compacted-branch").unwrap();
            let store = Store::open_at(dir.path().into()).unwrap();
            let mut parent = session();
            parent.id = "chat_parent".into();
            store.create_chat_session(&parent).unwrap();
            let mut parent_message = message("cafe0001", "user", "FROZEN_PARENT_FACT");
            parent_message.session_id = parent.id.clone();
            store
                .upsert_chat_message_on_branch(&parent_message)
                .unwrap();

            let mut session = session();
            session.bootstrap_context = Some("DISCARDED_COMPACTION_SUMMARY".into());
            if side {
                session.side_parent_session_id = Some(parent.id);
                session.side_parent_leaf_id = Some(parent_message.id);
            }
            store.create_chat_session(&session).unwrap();
            let request = message("cafe0002", "user", "DISCARDED_REQUEST");
            store.upsert_chat_message_on_branch(&request).unwrap();
            let mut compacted = message("cafe0003", "assistant", "");
            compacted.parts_json = serde_json::to_string(&[WirePart::tool(
                "compacted",
                super::super::COMPACTED_TOOL,
                "completed",
                None,
            )])
            .unwrap();
            store.upsert_chat_message_on_branch(&compacted).unwrap();

            let saved = store.get_chat_session(&session.id).unwrap().unwrap();
            assert!(context(&store, &saved, None, None, None)
                .unwrap()
                .contains("DISCARDED_COMPACTION_SUMMARY"));
            for leaf in [None, Some(request.id.as_str())] {
                store
                    .set_chat_session_active_leaf(&session.id, leaf)
                    .unwrap();
                let rewound = store.get_chat_session(&session.id).unwrap().unwrap();
                let handoff = context(&store, &rewound, None, None, None).unwrap();
                assert!(!handoff.contains("DISCARDED_COMPACTION_SUMMARY"));
                assert_eq!(handoff.contains("FROZEN_PARENT_FACT"), side);
                if leaf.is_none() {
                    assert!(!handoff.contains("DISCARDED_REQUEST"));
                }
            }
            store
                .set_chat_session_active_leaf(&session.id, Some(&compacted.id))
                .unwrap();
            let restored = store.get_chat_session(&session.id).unwrap().unwrap();
            assert!(context(&store, &restored, None, None, None)
                .unwrap()
                .contains("DISCARDED_COMPACTION_SUMMARY"));
        }
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
