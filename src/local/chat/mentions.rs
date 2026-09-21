//! Grammar for `@`-mention tokens in chat composer text.
//!
//! Three forms: `@path/to/file.py` (a file mention), `@session:<id>` (pull
//! another session's context into this message), and `@message:<id>`
//! (forward this message to that session). This module only finds and
//! classifies the tokens in raw text — resolving a file path against a
//! worktree, building a session digest, or routing a forwarded message are
//! later steps (T5, T7) with their own providers and their own store/git
//! access. Keeping the grammar provider-free means it can be unit tested
//! with no store, no filesystem, and no session lookups, and the two later
//! tracks share one tokenizer instead of writing their own.
//!
//! Nothing outside this module's tests calls it yet — see the
//! `#![allow(dead_code)]` below — until T5 wires a file provider into
//! `send_message_showing`'s prepared-input step.

#![allow(dead_code)]

/// What a mention token refers to, with the id/path exactly as typed — case,
/// extension, and all. Resolving it against real files or sessions is the
/// caller's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionKind {
    /// `@path/to/file.py` — a repo- (or worktree-)relative path.
    File(String),
    /// `@session:<id>` — pull that session's context into this message.
    Session(String),
    /// `@message:<id>` — forward this message to that session.
    Message(String),
}

/// One `@`-mention found in composer text, with its byte-offset span in the
/// original string so a caller can highlight it, splice it out, or rewrite
/// it in place without re-searching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub kind: MentionKind,
    /// Byte offset of the leading `@` in the original text.
    pub start: usize,
    /// Byte offset one past the last token character (exclusive).
    pub end: usize,
}

const SESSION_PREFIX: &str = "session:";
const MESSAGE_PREFIX: &str = "message:";

/// Trailing punctuation trimmed off the *end* of a token — sentence
/// punctuation and closing brackets/quotes — so a mention can sit inside
/// ordinary prose ("see @src/foo.rs.") or be wrapped in punctuation
/// ("(@src/a.py)", "\"@src/b.py\"") without either riding along. Applied
/// only after classification decides whether a `session:`/`message:`
/// prefix is present (see [`classify`]), so a bare `@session:` doesn't lose
/// its own colon to this trim before that check ever sees it.
fn is_trailing_punct(c: char) -> bool {
    matches!(
        c,
        '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '"' | '\''
    )
}

/// A token may start only where a mention plausibly begins a word: the
/// start of the text, after whitespace, or after an opening bracket or
/// quote. This is what keeps `user@example.com` from being read as a
/// mention of `example.com`.
fn starts_token(prev: char) -> bool {
    prev.is_whitespace() || "([{\"'".contains(prev)
}

/// Classify one whitespace-delimited word (the `@` already stripped, still
/// carrying whatever trailing punctuation ended the word) into a mention
/// kind, and how many of its bytes belong to the mention — the rest is
/// trailing punctuation the caller excludes from the token's span.
/// `None` for a bare `session:`/`message:` prefix with no id, or a path
/// that trims away to nothing.
fn classify(word: &str) -> Option<(MentionKind, usize)> {
    if let Some(rest) = word.strip_prefix(SESSION_PREFIX) {
        let id = rest.trim_end_matches(is_trailing_punct);
        return (!id.is_empty()).then(|| {
            (
                MentionKind::Session(id.to_string()),
                SESSION_PREFIX.len() + id.len(),
            )
        });
    }
    if let Some(rest) = word.strip_prefix(MESSAGE_PREFIX) {
        let id = rest.trim_end_matches(is_trailing_punct);
        return (!id.is_empty()).then(|| {
            (
                MentionKind::Message(id.to_string()),
                MESSAGE_PREFIX.len() + id.len(),
            )
        });
    }
    let path = word.trim_end_matches(is_trailing_punct);
    (!path.is_empty()).then(|| (MentionKind::File(path.to_string()), path.len()))
}

/// Find every `@`-mention in `text`, in the order they appear.
///
/// A token runs from a qualifying `@` (see [`starts_token`]) to the next
/// whitespace; [`classify`] then decides its kind and trims trailing
/// punctuation off the id/path portion. A bare `@` with nothing
/// non-whitespace after it, or a `session:`/`message:` token with no id, is
/// not a mention at all — neither an error nor a `File` mention of the
/// literal text.
pub fn find_mentions(text: &str) -> Vec<Mention> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut mentions = Vec::new();
    let mut idx = 0;
    while idx < chars.len() {
        let (byte_pos, ch) = chars[idx];
        if ch != '@' {
            idx += 1;
            continue;
        }
        let ok_start = idx == 0 || starts_token(chars[idx - 1].1);
        if !ok_start {
            idx += 1;
            continue;
        }

        let body_start = idx + 1;
        let mut word_end = body_start;
        while word_end < chars.len() && !chars[word_end].1.is_whitespace() {
            word_end += 1;
        }
        if word_end == body_start {
            // Bare "@" — nothing to mention.
            idx = word_end.max(idx + 1);
            continue;
        }

        let word_start_byte = chars[body_start].0;
        let word_end_byte = chars
            .get(word_end)
            .map(|(pos, _)| *pos)
            .unwrap_or(text.len());
        let word = &text[word_start_byte..word_end_byte];

        if let Some((kind, consumed_bytes)) = classify(word) {
            mentions.push(Mention {
                kind,
                start: byte_pos,
                end: word_start_byte + consumed_bytes,
            });
        }
        idx = word_end;
    }
    mentions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> MentionKind {
        MentionKind::File(path.to_string())
    }

    #[test]
    fn plain_file_mention_is_found_with_its_exact_span() {
        let text = "look at @src/foo.rs please";
        let found = find_mentions(text);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, file("src/foo.rs"));
        assert_eq!(&text[found[0].start..found[0].end], "@src/foo.rs");
    }

    #[test]
    fn session_and_message_prefixes_are_classified() {
        let found = find_mentions("see @session:chat_abc and @message:chat_xyz now");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, MentionKind::Session("chat_abc".to_string()));
        assert_eq!(found[1].kind, MentionKind::Message("chat_xyz".to_string()));
    }

    #[test]
    fn trailing_sentence_punctuation_is_not_part_of_the_mention() {
        let found = find_mentions("see @src/foo.rs. it's broken");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].kind, file("src/foo.rs"));
    }

    #[test]
    fn a_mention_can_sit_inside_brackets_or_quotes() {
        let found = find_mentions("(@src/a.py) and \"@src/b.py\"");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].kind, file("src/a.py"));
        assert_eq!(found[1].kind, file("src/b.py"));
    }

    #[test]
    fn email_addresses_are_never_mentions() {
        assert_eq!(find_mentions("email user@example.com about it"), vec![]);
    }

    #[test]
    fn a_bare_at_or_an_empty_keyword_id_is_not_a_mention() {
        assert_eq!(find_mentions("just an @ sign"), vec![]);
        assert_eq!(find_mentions("@session: missing id"), vec![]);
        assert_eq!(find_mentions("@message: missing id"), vec![]);
    }

    #[test]
    fn a_mention_at_the_very_start_of_the_text_is_found() {
        let found = find_mentions("@session:chat_1 do the thing");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].start, 0);
        assert_eq!(found[0].kind, MentionKind::Session("chat_1".to_string()));
    }

    #[test]
    fn multibyte_text_before_a_mention_keeps_byte_offsets_correct() {
        let text = "café @src/menu.rs";
        let found = find_mentions(text);
        assert_eq!(found.len(), 1);
        // "café" is 5 bytes (é is 2 bytes) + 1 space = offset 6.
        assert_eq!(found[0].start, 6);
        assert_eq!(&text[found[0].start..found[0].end], "@src/menu.rs");
    }

    #[test]
    fn multiple_mentions_are_found_in_order() {
        let found = find_mentions("@a.py then @b.py then @c.py");
        assert_eq!(
            found.iter().map(|m| m.kind.clone()).collect::<Vec<_>>(),
            vec![file("a.py"), file("b.py"), file("c.py")]
        );
    }
}
