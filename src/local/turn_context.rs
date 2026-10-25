//! Context blocks that ride every turn: the goal, the copilot autonomy level,
//! the sleep-mode rules, and the strategy in force.
//!
//! A harness that keeps native history (Claude Code `--resume`, Codex threads,
//! OpenCode sessions) keeps every copy of such a block for the rest of the
//! session, so a block repeated verbatim each turn grows the context by its
//! full size every turn and is re-read on every request after it. Overnight
//! sleep mode is the worst case: dozens of hidden turns each carried ~1.5k
//! tokens of rules and strategy the agent already had.
//!
//! A block is therefore sent in full once, then as a one-line reference while
//! the native session still holds that exact text: any of the last
//! [`REFRESH_TURNS`] turns on the active branch, in the same native session,
//! delivered it. A new or lost native session, a compaction, a branch switch,
//! a harness failover, or an edited block all send it in full again, and so
//! does a long session every [`REFRESH_TURNS`] turns, in case the harness
//! summarized its own history.

use sha2::{Digest as _, Sha256};

use crate::store::{Store, StoredChatSession};

/// How many earlier turns a full copy may lie back before it is resent.
pub const REFRESH_TURNS: usize = 10;

/// What the native session already holds: the prepared inputs of its recent
/// turns on the active branch.
#[derive(Debug, Default)]
pub struct Delivered {
    inputs: Vec<String>,
}

impl Delivered {
    /// Nothing delivered yet: every block goes out in full.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn from_inputs(inputs: Vec<String>) -> Self {
        Self { inputs }
    }

    /// The recent turns of `session`'s live native session on its active
    /// branch. Best-effort: on any store error nothing counts as delivered.
    pub fn load(store: &Store, session: &StoredChatSession) -> Self {
        let Some(native) = session.native_session_id.as_deref() else {
            return Self::none();
        };
        let Ok(messages) = store.list_chat_messages(&session.id) else {
            return Self::none();
        };
        let mut assistants = Vec::new();
        let mut current = session
            .active_leaf_id
            .as_deref()
            .and_then(|id| messages.iter().find(|m| m.id == id));
        while let Some(message) = current {
            if message.role == "assistant" {
                // An older native session (before a compaction, a failover or
                // a lost session) holds nothing the live one can see.
                if message.result_native_session_id.as_deref() != Some(native) {
                    break;
                }
                assistants.push(message.id.clone());
                if assistants.len() >= REFRESH_TURNS {
                    break;
                }
            }
            current = message
                .parent_id
                .as_deref()
                .and_then(|parent| messages.iter().find(|m| m.id == parent));
        }
        store
            .chat_turn_inputs_for_assistants(&session.id, &assistants)
            .map(Self::from_inputs)
            .unwrap_or_default()
    }

    /// `body` in full under a `[key #id]` header, or just that header with a
    /// pointer back when the native session already holds this exact text.
    pub fn once(&self, key: &str, body: &str) -> String {
        let header = format!("[{key} #{}]", digest(body));
        let full = format!("{header}\n{body}");
        if self.inputs.iter().any(|input| input.contains(&full)) {
            format!(
                "{header} Unchanged since it was last sent in this chat; it still applies in full."
            )
        } else {
            full
        }
    }
}

/// The language the user reads the dashboard in, when agents should reply in
/// it rather than in English. Only interface languages whose users asked for
/// it are listed; the rest keep the harness's own default.
pub fn reply_language() -> Option<&'static str> {
    language_for_locale(crate::telemetry::dashboard_locale().as_deref()?)
}

fn language_for_locale(locale: &str) -> Option<&'static str> {
    match locale {
        "vi" => Some("Vietnamese"),
        _ => None,
    }
}

/// The `<orx-language>` block asking for replies in `language`. Written in
/// English like the rest of the prompt: Vietnamese and other non-Latin-heavy
/// scripts take several times the tokens for the same instruction.
pub fn language_block(delivered: &Delivered, language: &str) -> String {
    let body = format!(
        "The user reads this workspace in {language}. Write every reply, question, plan, \
         and report meant for the user in {language}, with correct spelling and \
         diacritics, and keep it concise. Keep code, code comments, commands, file \
         paths, identifiers, metric names, commit messages, and quotations in their \
         original language."
    );
    format!(
        "<orx-language>\n{}\n</orx-language>",
        delivered.once("language", &body)
    )
}

/// A short, stable fingerprint of a block's text.
fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(5)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{StoredChatMessage, StoredChatTurn};

    #[test]
    fn a_block_is_sent_in_full_once_then_referenced() {
        let first = Delivered::none().once("rules", "Be careful.");
        assert!(first.ends_with("\nBe careful."));
        let header = first.lines().next().unwrap().to_string();
        assert!(header.starts_with("[rules #"));

        let later = Delivered::from_inputs(vec![format!("<x>\n{first}\n</x>\n\nhello")])
            .once("rules", "Be careful.");
        assert!(later.starts_with(&header));
        assert!(!later.contains("Be careful."));
        // A brief reference is not a full copy: the turn after it still
        // finds the original, and never mistakes the reference for it.
        assert!(!Delivered::from_inputs(vec![later])
            .once("rules", "Be careful.")
            .contains("Unchanged"));
    }

    #[test]
    fn vietnamese_interfaces_get_vietnamese_replies() {
        assert_eq!(language_for_locale("vi"), Some("Vietnamese"));
        assert_eq!(language_for_locale("en"), None);
        let block = language_block(&Delivered::none(), "Vietnamese");
        assert!(block.starts_with("<orx-language>\n[language #"));
        assert!(block.contains("in Vietnamese"));
    }

    #[test]
    fn an_edited_block_is_sent_in_full_again() {
        let first = Delivered::none().once("goal", "Reach 90%.");
        let edited = Delivered::from_inputs(vec![first]).once("goal", "Reach 95%.");
        assert!(edited.ends_with("\nReach 95%."));
    }

    fn message(id: &str, role: &str, parent: Option<&str>, native: &str) -> StoredChatMessage {
        StoredChatMessage {
            id: id.into(),
            session_id: "chat".into(),
            role: role.into(),
            parts_json: "[]".into(),
            created_at: 0,
            completed_at: None,
            parent_id: parent.map(Into::into),
            base_native_session_id: None,
            result_native_session_id: (role == "assistant").then(|| native.into()),
        }
    }

    fn turn(id: &str, user: Option<&str>, assistant: &str, input: &str) -> StoredChatTurn {
        StoredChatTurn {
            id: id.into(),
            session_id: "chat".into(),
            user_message_id: user.map(Into::into),
            assistant_message_id: assistant.into(),
            client_turn_id: id.into(),
            request_hash: String::new(),
            prepared_input: input.into(),
            settings_json: "{}".into(),
            state: "completed".into(),
            delivery_state: "accepted".into(),
            attempt_count: 0,
            next_retry_at: None,
            error_kind: None,
            error_message: None,
            recovery_action: None,
            recovered_by_turn_id: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// The store side: only turns on the active branch of the live native
    /// session count, hidden turns (no user message) included.
    #[test]
    fn load_reads_the_live_native_sessions_branch() {
        let root = std::env::temp_dir().join(format!("orx-turn-ctx-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(root.clone()).unwrap();
            let mut session = StoredChatSession {
                id: "chat".into(),
                project_id: "p".into(),
                harness: "claude".into(),
                native_session_id: Some("n1".into()),
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
                created_at: 0,
                updated_at: 0,
            };
            store.create_chat_session(&session).unwrap();
            let full = Delivered::none().once("goal", "Reach 90%.");
            let u1 = message("u1", "user", None, "n1");
            store
                .admit_chat_turn(Some(&u1), &turn("t1", Some("u1"), "a1", &full))
                .unwrap();
            store
                .upsert_chat_message(&message("a1", "assistant", Some("u1"), "n1"))
                .unwrap();
            // A hidden turn (sleep nudge, run wake-up) after it.
            store
                .admit_chat_turn(None, &turn("t2", None, "a2", "[orx] check-in"))
                .unwrap();
            store
                .upsert_chat_message(&message("a2", "assistant", Some("a1"), "n1"))
                .unwrap();
            session.active_leaf_id = Some("a2".into());

            let brief = Delivered::load(&store, &session).once("goal", "Reach 90%.");
            assert!(brief.contains("Unchanged"), "{brief}");
            // A fresh native session (compaction, failover) holds nothing.
            session.native_session_id = Some("n2".into());
            assert_eq!(
                Delivered::load(&store, &session).once("goal", "Reach 90%."),
                full
            );
            session.native_session_id = None;
            assert_eq!(
                Delivered::load(&store, &session).once("goal", "Reach 90%."),
                full
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
