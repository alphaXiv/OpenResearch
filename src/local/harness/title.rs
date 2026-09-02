//! Shared pieces of harness auto-titling: the prompt every one-shot title child
//! is given, and the sanitizer that turns whatever the model actually said into
//! a title worth showing in Recents.
//!
//! Kept free of process spawning so both halves stay unit-testable — the
//! per-harness files hold only the thin transport around them (the same split
//! as `parse_claude_model_list` and its spawner).

use std::time::Duration;

/// Chars of the first message fed to the model. A title only needs the opening
/// intent, and an unbounded paste would make the cheap child expensive.
const TITLE_INPUT_CAP: usize = 2000;

/// Longest title we'll store; anything past this is the model ignoring the
/// word limit, and the sidebar truncates it anyway.
const TITLE_MAX_CHARS: usize = 80;

/// Wall-clock budget for a one-shot title child. Generous enough for a cold
/// CLI start, short enough that a wedged child doesn't linger — the placeholder
/// title is already on screen either way.
const TITLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The one-shot every harness runs for a title (the trait's default
/// `generate_title`), around an already-built [`title_prompt`]: cheapest
/// model, no tools, one short system line.
pub(crate) fn title_request(prompt: &str) -> super::OneShot<'_> {
    super::OneShot {
        system: "You generate short chat titles.",
        prompt,
        quality: super::OneShotQuality::Cheap,
        timeout: TITLE_TIMEOUT,
    }
}

pub(crate) fn title_prompt(first_message: &str) -> String {
    let input: String = first_message.chars().take(TITLE_INPUT_CAP).collect();
    format!(
        "Generate a short title for a chat session that starts with the user \
         message below. At most 6 words. Reply with the title text ONLY — no \
         quotes, no trailing punctuation, no explanation, nothing else.\n\n\
         User message:\n{input}"
    )
}

/// Model output → usable title, or `None` (caller keeps the placeholder).
///
/// Defensive on purpose: a chatty model may wrap the title in quotes, add
/// trailing periods, or prefix a blank line, and none of that should reach the
/// sidebar.
pub(crate) fn sanitize_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    // Trailing periods twice: the first strip catches a period *outside* the
    // quotes ("Fix the redirect".), the second one *inside* them ("Fix the
    // redirect.") once the quotes are gone. Models produce both.
    let line = strip_wrapping_quotes(line.trim_end_matches('.').trim())
        .trim_end_matches('.')
        .trim();
    let title: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    // A title with nothing alphanumeric in it is punctuation debris, not a
    // name: a lone or unbalanced quote, a run of dots, a zero-width space.
    // Better to keep the placeholder than to show it.
    if !title.chars().any(char::is_alphanumeric) {
        return None;
    }
    Some(
        title
            .chars()
            .take(TITLE_MAX_CHARS)
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

/// Strip one layer of matching wrapping quotes/backticks, if any.
///
/// Only a *genuinely* wrapping pair: the same char at both ends with no further
/// instance between them. Otherwise a title that quotes two phrases of its own
/// (`"draft" vs "published" states`) would lose its outer delimiters and keep
/// the inner ones.
fn strip_wrapping_quotes(s: &str) -> &str {
    for q in ['"', '\'', '`'] {
        if let Some(inner) = s.strip_prefix(q).and_then(|i| i.strip_suffix(q)) {
            if !inner.contains(q) {
                return inner.trim();
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_quotes_and_trailing_period() {
        assert_eq!(
            sanitize_title("\"Fix the login redirect\""),
            Some("Fix the login redirect".into())
        );
        assert_eq!(
            sanitize_title("`Fix the login redirect`"),
            Some("Fix the login redirect".into())
        );
        assert_eq!(
            sanitize_title("'Fix the login redirect'."),
            Some("Fix the login redirect".into())
        );
        assert_eq!(
            sanitize_title("Fix the login redirect."),
            Some("Fix the login redirect".into())
        );
    }

    #[test]
    fn sanitize_only_strips_a_genuinely_wrapping_quote_pair() {
        assert_eq!(sanitize_title("\"Fix login\""), Some("Fix login".into()));
        // The quotes here belong to the title: stripping the outer pair would
        // leave the inner ones stranded.
        assert_eq!(
            sanitize_title("\"draft\" vs \"published\" states"),
            Some("\"draft\" vs \"published\" states".into())
        );
        assert_eq!(
            sanitize_title("'Auth' vs 'session'"),
            Some("'Auth' vs 'session'".into())
        );
        // An apostrophe inside blocks the strip too — keeping the outer quotes
        // beats stranding it.
        assert_eq!(
            sanitize_title("'Don't break it'"),
            Some("'Don't break it'".into())
        );
    }

    #[test]
    fn sanitize_takes_first_nonempty_line_and_collapses_whitespace() {
        assert_eq!(
            sanitize_title("\n\n  Rename\tthe   store  columns \nHere's why: …"),
            Some("Rename the store columns".into())
        );
    }

    #[test]
    fn sanitize_rejects_empty_output() {
        assert_eq!(sanitize_title(""), None);
        assert_eq!(sanitize_title("   \n\t "), None);
        assert_eq!(sanitize_title("\"\""), None);
        assert_eq!(sanitize_title("."), None);
    }

    #[test]
    fn sanitize_rejects_punctuation_only_output() {
        // A lone quote survives the quote-stripping pass (nothing to match).
        assert_eq!(sanitize_title("\""), None);
        assert_eq!(sanitize_title("''"), None);
        assert_eq!(sanitize_title("..."), None);
        assert_eq!(sanitize_title("— — —"), None);
        assert_eq!(sanitize_title("\u{200b}"), None);
    }

    #[test]
    fn sanitize_caps_at_eighty_chars() {
        let title = sanitize_title(&"word ".repeat(50)).unwrap();
        assert!(title.chars().count() <= TITLE_MAX_CHARS);
        assert!(!title.ends_with(' '));
    }

    #[test]
    fn prompt_truncates_long_messages() {
        let long = "x".repeat(TITLE_INPUT_CAP + 500);
        let prompt = title_prompt(&long);
        assert!(prompt.contains(&"x".repeat(TITLE_INPUT_CAP)));
        assert!(!prompt.contains(&"x".repeat(TITLE_INPUT_CAP + 1)));
    }
}
