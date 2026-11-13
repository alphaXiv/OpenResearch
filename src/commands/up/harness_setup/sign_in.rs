//! A vendor sign-in's terminal output, read for the page the user must open
//! and for a pasted code the CLI turned down.

use serde::Deserialize;

/// Bounds what is kept of the output while the page has not appeared.
const HISTORY_LIMIT: usize = 65_536;
const URL_CHARS: &str = "-._~:/?#[]@!$&'()*+,;=%";

pub(in crate::commands::up) struct SignInPage {
    hosts: &'static [&'static str],
    history: String,
    url_sent: bool,
    code_sent: bool,
    failure: Option<String>,
}

/// What one chunk of output revealed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::commands::up) struct SignInUpdate {
    /// The page to open, the first time all of it has been printed.
    pub url: Option<String>,
    /// The CLI refused the pasted code, in words for the user.
    pub failure: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientMessage {
    Code { code: String },
}

impl SignInPage {
    /// Hosts a sign-in page may live on, so only the vendor's own page is offered.
    pub(in crate::commands::up) fn for_login(harness: &str) -> Option<Self> {
        let hosts: &'static [&'static str] = match harness {
            "claude-code" => &["claude.ai", "claude.com", "anthropic.com"],
            "codex" => &["auth.openai.com"],
            "cursor" => &["cursor.com"],
            _ => return None,
        };
        Some(Self {
            hosts,
            history: String::new(),
            url_sent: false,
            code_sent: false,
            failure: None,
        })
    }

    pub(in crate::commands::up) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub(in crate::commands::up) fn observe(&mut self, bytes: &[u8], cols: u16) -> SignInUpdate {
        self.history.push_str(&String::from_utf8_lossy(bytes));
        if self.history.len() > HISTORY_LIMIT {
            let cut = self
                .history
                .ceil_char_boundary(self.history.len() - HISTORY_LIMIT);
            self.history.drain(..cut);
        }
        let mut update = SignInUpdate::default();
        if !self.url_sent {
            update.url = self.link().or_else(|| self.plain_url(usize::from(cols)));
            self.url_sent = update.url.is_some();
        }
        if self.code_sent && self.failure.is_none() {
            self.failure = rejected_code(&screen_text(&self.history, true));
            update.failure = self.failure.clone();
        }
        update
    }

    /// The code to type for a client message, or `None` for any other message.
    pub(in crate::commands::up) fn code(&mut self, text: &str) -> Option<String> {
        let ClientMessage::Code { code } = serde_json::from_str(text).ok()?;
        let code = code.trim();
        if code.is_empty() || code.len() > 4096 || code.chars().any(char::is_control) {
            return None;
        }
        // Lines printed before this code can't be its rejection.
        self.history.clear();
        self.code_sent = true;
        Some(code.to_string())
    }

    fn allowed(&self, url: &str) -> bool {
        url::Url::parse(url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some_and(|host| {
                    self.hosts
                        .iter()
                        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
                })
        })
    }

    /// An OSC 8 hyperlink carries the whole URL however the text around it wraps.
    fn link(&self) -> Option<String> {
        let mut rest = self.history.as_str();
        while let Some(at) = rest.find("\x1b]8;") {
            rest = &rest[at + 4..];
            let target = &rest[rest.find(';')? + 1..];
            let end = target.find(['\x07', '\x1b'])?;
            if self.allowed(&target[..end]) {
                return Some(target[..end].to_string());
            }
            rest = &target[end..];
        }
        None
    }

    /// A URL printed as text. A line as wide as the terminal may have been wrapped
    /// onto the next, so the URL is joined until a line that can't continue it.
    fn plain_url(&self, cols: usize) -> Option<String> {
        let text = screen_text(&self.history, false);
        let wrapped = |line: &str| {
            let width = line.chars().count();
            cols > 0 && width + 1 >= cols && width <= cols
        };
        let mut from = 0;
        while let Some(found) = text[from..].find("https://") {
            let at = from + found;
            from = at + "https://".len();
            let end = text[at..]
                .find(char::is_whitespace)
                .map_or(text.len(), |end| at + end);
            if !self.allowed(&text[at..end]) {
                continue;
            }
            let mut url = text[at..end].to_string();
            if !text[end..].starts_with('\n') {
                // Something follows on the same line, or the line is still printing.
                return (end < text.len()).then_some(url);
            }
            let line_start = text[..at].rfind('\n').map_or(0, |start| start + 1);
            let lines: Vec<&str> = text[line_start..].split('\n').collect();
            let mut line = lines[0];
            for next in &lines[1..lines.len() - 1] {
                let part = next.trim();
                if !wrapped(line) || part.is_empty() || !part.chars().all(url_char) {
                    return Some(url);
                }
                url.push_str(part);
                line = next;
            }
            return (!wrapped(line)).then_some(url);
        }
        None
    }
}

fn url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || URL_CHARS.contains(c)
}

/// What a terminal shows of `raw`: carriage returns dropped and a cursor move
/// (how Ink spaces words) read as a space. Other escape sequences are dropped, or
/// with `split` end the line, as a redraw puts what follows elsewhere.
fn screen_text(raw: &str, split: bool) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if split {
                text.push('\n');
            }
            continue;
        }
        if c != '\x1b' {
            text.push(c);
            continue;
        }
        let mut final_byte = None;
        match chars.next() {
            Some('[') => {
                final_byte = chars.by_ref().find(|c| ('\x40'..='\x7e').contains(c));
            }
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            Some(c) if ('\x20'..='\x2f').contains(&c) => {
                while chars.next_if(|c| ('\x20'..='\x2f').contains(c)).is_some() {}
                chars.next();
            }
            _ => {}
        }
        if matches!(final_byte, Some('C' | 'G')) {
            text.push(' ');
        } else if split {
            text.push('\n');
        }
    }
    text
}

/// How Claude Code says it turned a pasted code down, once the line is complete.
fn rejected_code(lines: &str) -> Option<String> {
    let complete = &lines[..lines.rfind('\n')?];
    let reason = complete.lines().find_map(|line| {
        let line = line.trim();
        ["OAuth error:", "Login failed:"]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix).map(str::trim))
            .or_else(|| line.starts_with("Invalid code.").then_some(line))
            .filter(|reason| !reason.is_empty())
    })?;
    Some(format!(
        "Claude didn't accept the code: {}. Try again for a new link and code.",
        reason.trim_end_matches('.')
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // From real Claude Code 2.1.301 runs in an 80-column terminal (The Knife's fixtures).
    const CLAUDE_URL: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=user%3Ainference&code_challenge=V_mpkuwkjehz88ztIzUkJbp9v6mmPEGvHirBeba720Y&code_challenge_method=S256&state=QAOMEi61u0DUpuJDrizIybKtYVgy9w15cT4H8d6SmtY";

    fn ink_prompt(linked: bool) -> String {
        let mut out = "\r\x1b[1C\x1b[1ABrowser didn't open?\x1b[23GUse the url\x1b[35Gbelow\x1b[41Gto\x1b[44Gsign\x1b[49Gin\x1b[52G(c\x1b[55Gto\x1b[58Gcopy)\r\r\n\r\r\n".to_string();
        for part in CLAUDE_URL.as_bytes().chunks(80) {
            let part = std::str::from_utf8(part).unwrap();
            if linked {
                out.push_str(&format!(
                    "\x1b]8;id=1cxriz3;{CLAUDE_URL}\x07{part}\x1b]8;;\x07\r\r\n"
                ));
            } else {
                out.push_str(&format!("{part}\r\r\n"));
            }
        }
        out.push_str("\r\r\n\r\r\n\x1b[2GPaste\x1b[8Gcode\x1b[13Ghere\x1b[18Gif\x1b[21Gprompted\x1b[30G>\r\r\n");
        out
    }

    fn feed(page: &mut SignInPage, chunks: &[&str]) -> (Option<String>, Option<String>) {
        let mut found = (None, None);
        for chunk in chunks {
            let update = page.observe(chunk.as_bytes(), 80);
            found.0 = found.0.or(update.url);
            found.1 = found.1.or(update.failure);
        }
        found
    }

    #[test]
    fn reads_the_claude_page_from_its_link_or_its_wrapped_text() {
        let login = format!(
            "Opening browser to sign in…\r\nIf the browser didn't open, visit: \x1b]8;;{CLAUDE_URL}\x07{CLAUDE_URL}\x1b]8;;\x07\r\nPaste code here if prompted > "
        );
        for output in [ink_prompt(true), ink_prompt(false), login] {
            let mut page = SignInPage::for_login("claude-code").unwrap();
            assert_eq!(feed(&mut page, &[&output]).0.as_deref(), Some(CLAUDE_URL));
        }
        // Up to a full-width line: the next line may still continue the URL.
        let plain = ink_prompt(false);
        let cut = plain.find(&CLAUDE_URL[80..160]).unwrap() + 82;
        let mut page = SignInPage::for_login("claude-code").unwrap();
        assert_eq!(feed(&mut page, &[&plain[..cut]]).0, None);
        assert_eq!(
            feed(&mut page, &[&plain[cut..]]).0.as_deref(),
            Some(CLAUDE_URL)
        );
        // The page is reported once.
        assert_eq!(feed(&mut page, &[&plain]).0, None);
    }

    #[test]
    fn reads_a_long_url_printed_on_one_line() {
        let url = format!(
            "https://auth.openai.com/oauth/authorize?response_type=code&client_id=app_EMoamEEZ73f0CkXaXp7hrann&redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&scope=openid&state={}",
            "x".repeat(60)
        );
        let output = format!("Starting local login server on http://localhost:1455.\nIf your browser did not open, navigate to this URL to authenticate:\n\n{url}\n");
        let mut page = SignInPage::for_login("codex").unwrap();
        assert_eq!(feed(&mut page, &[&output]).0, Some(url));
    }

    #[test]
    fn offers_only_the_vendors_own_page() {
        let mut page = SignInPage::for_login("codex").unwrap();
        let output = "Open \x1b]8;;https://evil.example/oauth\x07here\x1b]8;;\x07\nor https://auth.openai.com.evil.example/x\n\n";
        assert_eq!(feed(&mut page, &[output]).0, None);
        assert!(SignInPage::for_login("opencode").is_none());
    }

    #[test]
    fn a_turned_down_code_ends_the_sign_in_once_its_line_is_complete() {
        let mut page = SignInPage::for_login("claude-code").unwrap();
        feed(&mut page, &["Invalid code. Earlier noise.\r\n"]);
        assert_eq!(
            page.code(r#"{"type":"code","code":" abc#123 "}"#)
                .as_deref(),
            Some("abc#123")
        );
        let rejected = "Invalid code. Please make sure the full code was copied.\r\n";
        assert_eq!(feed(&mut page, &[&rejected[..30]]).1, None);
        assert_eq!(
            feed(&mut page, &[&rejected[30..]]).1.as_deref(),
            Some("Claude didn't accept the code: Invalid code. Please make sure the full code was copied. Try again for a new link and code.")
        );
        let mut page = SignInPage::for_login("claude-code").unwrap();
        page.code(r#"{"type":"code","code":"abc"}"#);
        let setup_token = "\x1b[2K\x1b[G\x1b[1A\r\x1b[1C\x1b[4A\x1b[38;5;211mOAuth error: Request failed with status code 400\x1b[39m\x1b[K\r\x1b[2B\x1b[K\r\x1b[1B Press Enter to retry.\r\n";
        assert_eq!(
            feed(&mut page, &[setup_token]).1.as_deref(),
            Some("Claude didn't accept the code: Request failed with status code 400. Try again for a new link and code.")
        );
    }

    #[test]
    fn only_a_plain_code_is_typed() {
        let mut page = SignInPage::for_login("claude-code").unwrap();
        assert_eq!(page.code(r#"{"type":"resize","cols":80,"rows":24}"#), None);
        assert_eq!(page.code(r#"{"type":"code","code":"   "}"#), None);
        assert_eq!(page.code(r#"{"type":"code","code":"a\u001b[2Jb"}"#), None);
        assert_eq!(page.code(r#"{"type":"code","code":"a\rb"}"#), None);
    }
}
