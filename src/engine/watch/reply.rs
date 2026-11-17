//! What a look's model may answer, and the checks every sentence passes before anyone reads it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

/// The longest run sentence, in displayed characters, at the numbers it reads now.
pub(super) const MAX_GRAPHEMES: usize = 48;
const MAX_TOKENS: usize = 12;
const MAX_NEW_COUNTS_PER_RUN: usize = 2;
const MAX_NEW_COUNTS: usize = 4;
const MAX_PATTERN: usize = 200;
const MAX_EVIDENCE: usize = 12;
const MAX_EVIDENCE_BYTES: u64 = 3 * 1024;
const MAX_NOTES: usize = 1024;
const LIVE: char = '\u{fffc}';

/// What one run's tokens can bind in this look.
pub(super) struct Bindable<'a> {
    pub key: &'a str,
    /// The view's signals, with their specs.
    pub signals: &'a Value,
    /// Signals compared with the parent now.
    pub versus: &'a BTreeSet<String>,
    /// Patterns already counted for this run.
    pub counts: &'a BTreeSet<String>,
    /// Numbers that move: no sentence may write one of these as it reads now.
    pub moving: &'a [f64],
    /// The step it is at, which is all a number written as a step can collide with.
    pub step: Option<f64>,
    /// Byte ranges of the log excerpts the digest showed.
    pub excerpts: &'a [(u64, u64)],
    /// Whether the run has a sentence a `same` can keep.
    pub sentence: Option<&'a str>,
    pub setup: bool,
}

/// One run's accepted answer.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Answer {
    Line {
        flag: bool,
        template: String,
        evidence: Vec<(u64, u64)>,
    },
    Same,
    Silent,
}

/// What a reply settled, and what it got wrong, in words its model is shown on a retry.
#[derive(Debug, Default)]
pub(super) struct Read {
    pub runs: BTreeMap<String, Answer>,
    /// Flags retained despite invalid replies; a retry's `same` cannot dismiss a possible problem.
    pub salvaged: BTreeSet<String>,
    pub summary: Option<Answer>,
    pub notes: Option<String>,
    pub errors: Vec<String>,
}

/// A token inside `{…}`: whose, and what it reads.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Token {
    /// `{r2}` in a summary: that run's label.
    Label(String),
    Signal(Option<String>, String),
    Versus(Option<String>, String),
    Count(Option<String>, String),
}

impl Token {
    fn run(&self) -> Option<&str> {
        match self {
            Self::Label(run) => Some(run),
            Self::Signal(run, _) | Self::Versus(run, _) | Self::Count(run, _) => run.as_deref(),
        }
    }

    /// The key a sentence's bindings carry this token's value under, as the page fills it.
    pub(super) fn binding(&self) -> String {
        match self {
            Self::Label(_) => String::new(),
            Self::Signal(_, name) => name.clone(),
            Self::Versus(_, name) => format!("{name}.vs"),
            Self::Count(_, pattern) => format!("count:{}", Value::from(pattern.as_str())),
        }
    }
}

/// Each `{…}` of a template and where it sits, or why one cannot be read.
pub(super) fn tokens(
    template: &str,
    summary: bool,
) -> Result<Vec<(std::ops::Range<usize>, Token)>, String> {
    let mut found = Vec::new();
    let mut rest = 0;
    while let Some(open) = template[rest..].find('{').map(|at| at + rest) {
        let close = template[open..]
            .find('}')
            .map(|at| at + open)
            .ok_or_else(|| format!("\"{}\" opens a token it never closes", &template[open..]))?;
        let inner = &template[open + 1..close];
        let (run, what) = match inner.split_once(':') {
            Some((run, what)) if summary && is_key(run) => (Some(run.to_owned()), what),
            _ if summary && is_key(inner) => {
                found.push((open..close + 1, Token::Label(inner.to_owned())));
                rest = close + 1;
                continue;
            }
            _ if summary => {
                return Err(format!(
                    "{{{inner}}} names no run: in the summary a token starts with the run's key, as {{r1:loss}}"
                ))
            }
            _ => (None, inner),
        };
        let token = if let Some(pattern) = what.strip_prefix("count:") {
            let pattern: String = serde_json::from_str(pattern).map_err(|_| {
                format!(
                    "{{{inner}}}: a count's pattern is a quoted string, as {{count:\"ERR 429\"}}"
                )
            })?;
            Token::Count(run, pattern)
        } else if let Some(name) = what.strip_suffix(".vs") {
            Token::Versus(run, name.to_owned())
        } else {
            Token::Signal(run, what.to_owned())
        };
        found.push((open..close + 1, token));
        rest = close + 1;
    }
    Ok(found)
}

fn is_key(text: &str) -> bool {
    text.strip_prefix('r')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A count pattern this orx will keep counting, or why not.
pub(super) fn pattern(text: &str) -> Result<regex::Regex, String> {
    if text.is_empty() || text.len() > MAX_PATTERN {
        return Err(format!(
            "count pattern \"{text}\" must be 1 to {MAX_PATTERN} bytes"
        ));
    }
    regex::RegexBuilder::new(text)
        .size_limit(64 * 1024)
        .build()
        .map_err(|error| format!("count pattern \"{text}\" is not a regex this orx reads: {error}"))
}

/// Checks `reply` against the runs it was asked about. A run's valid answer is kept even when
/// another's is not; each mistake is said so the model can correct it.
pub(super) fn read(
    reply: &str,
    runs: &[Bindable],
    summary: Option<&SummaryBounds>,
    render: &dyn Fn(Option<&str>, &Token) -> String,
) -> Read {
    let mut read = Read::default();
    let value = match parse(reply) {
        Ok(value) => value,
        Err(error) => {
            read.errors.push(error);
            return read;
        }
    };
    let Some(object) = value.as_object() else {
        read.errors.push("Reply with one JSON object.".into());
        return read;
    };
    for key in object.keys() {
        if !["runs", "summary", "notes"].contains(&key.as_str()) {
            read.errors.push(format!(
                "\"{key}\" is not part of a reply; give runs, summary and notes only"
            ));
        }
    }
    if let Some(notes) = object.get("notes") {
        match notes.as_str() {
            Some(text) if text.chars().count() <= MAX_NOTES => read.notes = Some(text.to_owned()),
            _ => read.errors.push(format!(
                "notes must be text of at most {MAX_NOTES} characters"
            )),
        }
    }
    let given = object.get("runs").and_then(Value::as_object);
    if let Some(keys) = given {
        for key in keys.keys() {
            if !runs.iter().any(|run| run.key == key) {
                read.errors
                    .push(format!("there is no run {key} in this look"));
            }
        }
    }
    let mut new_counts = 0;
    for run in runs {
        let answer = given.and_then(|given| given.get(run.key));
        // A run's own sentence may name the run as the summary does; it reads the same without.
        let mut answer = answer.cloned();
        if let Some(answer) = answer.as_mut() {
            if let Some(line) = answer.get("line").and_then(Value::as_str) {
                answer["line"] = Value::from(line.replace(&format!("{{{}:", run.key), "{"));
            }
        }
        let mut line_valid = false;
        match answer_of(answer.as_ref(), run.sentence, run.setup, &mut |template| {
            let checked = check_line(template, false, runs, Some(run), render)?;
            let fresh = checked
                .iter()
                .filter(|pattern| !run.counts.contains(*pattern))
                .count();
            if fresh > MAX_NEW_COUNTS_PER_RUN || new_counts + fresh > MAX_NEW_COUNTS {
                return Err(format!(
                    "counts at most {MAX_NEW_COUNTS_PER_RUN} new patterns a run and {MAX_NEW_COUNTS} a look"
                ));
            }
            new_counts += fresh;
            line_valid = true;
            Ok(())
        }) {
            Ok(Answer::Line {
                flag,
                template,
                evidence,
            }) => match check_evidence(&evidence, run.excerpts) {
                Ok(evidence) => {
                    read.runs.insert(
                        run.key.to_owned(),
                        Answer::Line {
                            flag,
                            template,
                            evidence,
                        },
                    );
                }
                Err(error) => read.errors.push(format!("{}: {error}", run.key)),
            },
            Ok(answer @ (Answer::Same | Answer::Silent)) => {
                read.runs.insert(run.key.to_owned(), answer);
            }
            Err(error) => read.errors.push(format!("{}: {error}", run.key)),
        }
        let refused = !read.runs.contains_key(run.key);
        let flagged = answer
            .as_ref()
            .is_some_and(|answer| answer["status"] == "flag");
        if let Some(line) = answer
            .as_ref()
            .and_then(|answer| answer["line"].as_str())
            .filter(|_| refused && flagged)
        {
            let kept = if line_valid {
                line.to_owned()
            } else {
                line.replace("**", "").replace(['{', '}'], "")
            };
            read.runs.insert(
                run.key.to_owned(),
                Answer::Line {
                    flag: true,
                    template: kept.trim().to_owned(),
                    evidence: Vec::new(),
                },
            );
            read.salvaged.insert(run.key.to_owned());
        }
    }
    if let Some(bounds) = summary {
        match answer_of(
            object.get("summary"),
            bounds.sentence,
            bounds.setup,
            &mut |template| check_line(template, true, runs, None, render).map(|_| ()),
        ) {
            Ok(answer) => read.summary = Some(answer),
            Err(error) => read.errors.push(format!("summary: {error}")),
        }
    }

    read
}

/// The summary's own state: whether it has a sentence to keep.
pub(super) struct SummaryBounds<'a> {
    pub sentence: Option<&'a str>,
    pub setup: bool,
}

/// The reply as JSON. A model that fences it in a code block still means the JSON inside.
fn parse(reply: &str) -> Result<Value, String> {
    let text = reply.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|inner| inner.strip_suffix("```"))
        .unwrap_or(text)
        .trim();
    serde_json::from_str(text).map_err(|error| {
        format!("The reply was not one JSON object ({error}); reply with JSON only.")
    })
}

fn answer_of(
    value: Option<&Value>,
    previous: Option<&str>,
    setup: bool,
    check: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<Answer, String> {
    if value.is_none() && setup {
        return Ok(Answer::Silent);
    }
    let same = serde_json::json!({"same": true});
    let object = value.unwrap_or(&same).as_object().ok_or("give an object")?;
    if object.get("same") == Some(&Value::Bool(true)) {
        if let Some(key) = object
            .keys()
            .find(|key| !["same", "notes"].contains(&key.as_str()))
        {
            return Err(format!("\"same\" takes no \"{key}\""));
        }
        let previous = previous.ok_or("there is no sentence yet to keep; write one")?;
        check(previous)?;
        return Ok(Answer::Same);
    }
    if let Some(key) = object
        .keys()
        .find(|key| !["status", "line", "evidence", "notes"].contains(&key.as_str()))
    {
        return Err(format!(
            "\"{key}\" is not part of an answer; give status, line and evidence, or same"
        ));
    }
    let flag = match object.get("status").and_then(Value::as_str) {
        Some("ok") => false,
        Some("flag") => true,
        _ => return Err("status is \"ok\" or \"flag\"".into()),
    };
    if !flag && object.get("line") == Some(&Value::Null) {
        return Ok(Answer::Silent);
    }
    let template = object
        .get("line")
        .and_then(Value::as_str)
        .ok_or("line is the sentence, as text")?
        .trim()
        .to_owned();
    check(&template)?;
    let evidence = match object.get("evidence") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                let from = item["from"].as_u64();
                let to = item["to"].as_u64();
                from.zip(to).ok_or_else(|| {
                    "evidence is a list of {\"from\": BYTE, \"to\": BYTE}".to_owned()
                })
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("evidence is a list of {\"from\": BYTE, \"to\": BYTE}".into()),
    };
    Ok(Answer::Line {
        flag,
        template,
        evidence,
    })
}

/// The shown excerpts a citation points at: each cited range stands for the lines it overlaps,
/// so evidence only ever names bytes the look was shown, at most 12 lines and 3 KiB of them. A
/// range that overlaps none is wrong.
fn check_evidence(
    evidence: &[(u64, u64)],
    excerpts: &[(u64, u64)],
) -> Result<Vec<(u64, u64)>, String> {
    let mut cited = Vec::new();
    for (from, to) in evidence {
        let overlapped: Vec<(u64, u64)> = excerpts
            .iter()
            .copied()
            .filter(|(a, b)| from < b && a < to)
            .collect();
        if from >= to || overlapped.is_empty() {
            return Err(format!(
                "evidence {from}..{to} is not a log excerpt this look showed"
            ));
        }
        cited.extend(overlapped);
    }
    cited.sort_unstable();
    cited.dedup();
    // Past the cap a flood of cited lines keeps its first ones: they say where it began.
    let mut bytes = 0;
    cited.retain(|(from, to)| {
        bytes += to - from;
        bytes <= MAX_EVIDENCE_BYTES
    });
    cited.truncate(MAX_EVIDENCE);
    Ok(cited)
}

/// The checks of one sentence; returns the count patterns it reads.
fn check_line(
    template: &str,
    summary: bool,
    runs: &[Bindable],
    own: Option<&Bindable>,
    render: &dyn Fn(Option<&str>, &Token) -> String,
) -> Result<Vec<String>, String> {
    if template.is_empty() {
        return Err("the sentence is empty".into());
    }
    if template.contains(['<', '>', '`', '\n'])
        || template.contains("](")
        || template.contains("://")
    {
        return Err("write plain words: no markup, links, code or line breaks".into());
    }
    let found = tokens(template, summary)?;
    if found.len() > MAX_TOKENS {
        return Err(format!("use at most {MAX_TOKENS} tokens"));
    }
    let mut patterns = Vec::new();
    for (_, token) in &found {
        let run = match token.run() {
            Some(key) => runs
                .iter()
                .find(|run| run.key == key)
                .ok_or_else(|| format!("{{{key}…}} names no run in this look"))?,
            None => own.expect("a run's own sentence"),
        };
        match token {
            Token::Label(_) => {}
            Token::Signal(_, name) => {
                if run.signals.get(name).is_none() {
                    return Err(format!(
                        "{{{name}}} is not a signal of {}; its signals are {}",
                        run.key,
                        run.signals.as_object().map_or_else(String::new, |s| s
                            .keys()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "))
                    ));
                }
            }
            Token::Versus(_, name) => {
                if !run.versus.contains(name) {
                    return Err(format!(
                        "{{{name}.vs}} compares with the parent, which {} has no comparison for",
                        run.key
                    ));
                }
            }
            Token::Count(_, text) => {
                pattern(text)?;
                patterns.push(text.clone());
            }
        }
    }
    let bold: Vec<_> = template.match_indices("**").collect();
    let has_figure = template.chars().any(|c| c.is_numeric())
        || found
            .iter()
            .any(|(_, token)| !matches!(token, Token::Label(_)));
    if bold.len() != 2 && !(bold.is_empty() && (summary || !has_figure)) {
        return Err("mark exactly one figure in **bold**".into());
    }
    if !bold.is_empty() {
        let inside = &template[bold[0].0 + 2..bold[1].0];
        let in_bold =
            |range: &std::ops::Range<usize>| range.start >= bold[0].0 + 2 && range.end <= bold[1].0;
        if inside.trim().is_empty()
            || !(inside.chars().any(|c| c.is_numeric())
                || found.iter().any(|(range, _)| in_bold(range)))
        {
            return Err("the **bold** part is the figure that matters: a number or a token".into());
        }
    }
    let mut rendered = String::new();
    let mut literal = String::new();
    let mut at = 0;
    for (range, token) in &found {
        rendered.push_str(&template[at..range.start]);
        literal.push_str(&template[at..range.start]);
        let bound = token
            .run()
            .and_then(|key| runs.iter().find(|run| run.key == key))
            .or(own);
        let live =
            !matches!(token, Token::Label(_)) && bound.is_some_and(|run| !run.moving.is_empty());
        literal.push(if live { LIVE } else { ' ' });
        let run = token.run().or(own.map(|run| run.key));
        rendered.push_str(&render(run, token));
        at = range.end;
    }
    rendered.push_str(&template[at..]);
    literal.push_str(&template[at..]);
    if literal.contains(LIVE) {
        static PINNED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let pinned = PINNED.get_or_init(|| regex::Regex::new(
            r"\b(?:step|epoch|round)\s+\d[\d.,]*|\bat\s+(?:time\s+\d[\d.,]*|\d+(?::\d+)+|\d+(?:[.,]\d+)?\s*(?:ms|s|seconds?|minutes?|hours?)\b)|\b(?:first|last|initial|final)\s+reading\b"
        ).expect("fixed timing pattern"));
        let literal = literal.to_lowercase();
        for place in pinned.find_iter(&literal) {
            let before = literal[..place.start()]
                .rsplit(';')
                .next()
                .unwrap_or_default();
            let after = literal[place.end()..].split(';').next().unwrap_or_default();
            let trend = before
                .split_whitespace()
                .last()
                .is_some_and(|word| ["since", "from", "after", "before"].contains(&word));
            if trend {
                continue;
            }
            let figure = before
                .chars()
                .rev()
                .find(|c| c.is_numeric() || *c == LIVE)
                .or_else(|| after.chars().find(|c| c.is_numeric() || *c == LIVE));
            if figure == Some(LIVE) {
                return Err("a live token describes now, a comparison or a trend; a fixed step, time or reading needs a fixed value".into());
            }
        }
    }
    let shown = rendered.replace("**", "").graphemes(true).count();
    let limit = if summary { 90 } else { MAX_GRAPHEMES };
    if shown > limit {
        return Err(format!(
            "the sentence reads {shown} characters with its numbers filled; keep it to {limit}"
        ));
    }
    let moving: Vec<f64> = runs
        .iter()
        .flat_map(|run| run.moving.iter().copied())
        .collect();
    let steps: Vec<f64> = runs.iter().filter_map(|run| run.step).collect();
    if let Some(number) = literal_moving(&literal, &moving, &steps) {
        return Err(format!(
            "\"{number}\" is a number that still moves; write it as its token so it stays current"
        ));
    }
    Ok(patterns)
}

/// The first number written in `text` that reads as one of `moving` does now. A number written
/// as a step ("since step 20") is a place in the run, which moves only when it is the step now.
fn literal_moving<'a>(text: &'a str, moving: &[f64], steps: &[f64]) -> Option<&'a str> {
    let mut start = None;
    let ends = text
        .char_indices()
        .map(|(at, c)| (at, Some(c)))
        .chain(std::iter::once((text.len(), None)));
    for (at, c) in ends {
        let part = c.is_some_and(|c| {
            ascii_digit(c).is_some() || (start.is_some() && matches!(c, '.' | ',' | '٫' | '٬'))
        });
        match (start, part) {
            (None, true) => start = Some(at),
            (Some(from), false) => {
                let lexeme = text[from..at].trim_end_matches(['.', ',', '٫', '٬']);
                let before = text[..from].trim_end().to_lowercase();
                let place = ["step", "steps", "epoch", "round"]
                    .iter()
                    .any(|word| before.ends_with(word));
                if matches_moving(lexeme, if place { steps } else { moving }) {
                    return Some(lexeme);
                }
                start = None;
            }
            _ => {}
        }
    }
    None
}

/// A decimal digit in any of the scripts the app is written in, as ASCII.
fn ascii_digit(c: char) -> Option<char> {
    let zero = ['0', '٠', '۰', '०', '০', '０']
        .into_iter()
        .find(|zero| (*zero..=char::from_u32(*zero as u32 + 9).unwrap_or(*zero)).contains(&c))?;
    char::from_digit(c as u32 - zero as u32, 10)
}

/// Whether `lexeme`, read with `.` or `,` as its decimal point or as grouping, is one of
/// `moving` rounded to the places it shows.
fn matches_moving(lexeme: &str, moving: &[f64]) -> bool {
    let digits: String = lexeme
        .chars()
        .map(|c| match c {
            '٫' => '.',
            '٬' => ',',
            c => ascii_digit(c).unwrap_or(c),
        })
        .collect();
    let readings = [
        digits.replace(',', ""),
        digits.replace('.', "").replace(',', "."),
        digits.replace(',', "."),
    ];
    readings.iter().any(|reading| {
        let Ok(value) = reading.parse::<f64>() else {
            return false;
        };
        // Zero is where a count or a failed check rests, and a sentence says so in words.
        if value == 0.0 {
            return false;
        }
        let places = reading
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        let half = 0.5 * 10f64.powi(-(places as i32));
        moving
            .iter()
            .any(|moving| moving.is_finite() && (moving - value).abs() <= half * (1.0 + 1e-9))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Run {
        signals: Value,
        versus: BTreeSet<String>,
        counts: BTreeSet<String>,
        moving: Vec<f64>,
        excerpts: Vec<(u64, u64)>,
    }

    fn run() -> Run {
        Run {
            signals: json!({"loss": {"label": "Loss"}, "val_loss": {"label": "Val loss", "better": "lower"}}),
            versus: ["val_loss".to_owned()].into(),
            counts: BTreeSet::new(),
            moving: vec![2.3104, 2.412, 340.0],
            excerpts: vec![(100, 900)],
        }
    }

    fn bind<'a>(key: &'a str, run: &'a Run, has_sentence: bool) -> Bindable<'a> {
        Bindable {
            key,
            signals: &run.signals,
            versus: &run.versus,
            counts: &run.counts,
            moving: &run.moving,
            step: run.moving.last().copied(),
            excerpts: &run.excerpts,
            sentence: has_sentence.then_some("Loss **{loss}**."),
            setup: false,
        }
    }

    fn render(_: Option<&str>, token: &Token) -> String {
        match token {
            Token::Label(key) => format!("Depth {key}"),
            Token::Count(..) => "12".into(),
            _ => "2.31".into(),
        }
    }

    #[test]
    fn numberless_sentences_and_setup_silence_pass() {
        let run = run();
        for setup in [false, true] {
            for line in [Value::Null, json!("Loss tracks the parent.")] {
                let mut binding = bind("r1", &run, false);
                binding.setup = setup;
                let reply = json!({"runs":{"r1":{"status":"ok","line":line}}}).to_string();
                let read = read(&reply, &[binding], None, &render);
                assert!(read.errors.is_empty(), "{:?}", read.errors);
                assert!(read.runs.contains_key("r1"));
            }
        }
        for line in ["Loss {loss}.", "There are 2 warnings."] {
            let reply = json!({"runs":{"r1":{"status":"ok","line":line}}}).to_string();
            assert!(!read(&reply, &[bind("r1", &run, false)], None, &render)
                .errors
                .is_empty());
        }
    }

    #[test]
    fn a_sentence_with_tokens_and_one_bold_figure_is_kept() {
        let run = run();
        let read = read(
            r#"{"runs": {"r1": {"status": "ok", "line": "Val loss **{val_loss}**, {val_loss.vs}."}}}"#,
            &[bind("r1", &run, false)],
            None,
            &render,
        );
        assert!(read.errors.is_empty(), "{:?}", read.errors);
        assert!(matches!(&read.runs["r1"], Answer::Line { flag: false, .. }));
    }

    #[test]
    fn a_flag_that_breaks_the_sentence_rules_is_kept_in_plain_words() {
        let run = run();
        for line in [
            "Most cases report **wrong output** from n 10000 up.",
            "Cases report correct {signal} from n 10000 up; loss **2.41**.",
        ] {
            let reply = json!({"runs": {"r1": {"status": "flag", "line": line}}}).to_string();
            let read = read(&reply, &[bind("r1", &run, true)], None, &render);
            assert!(!read.errors.is_empty(), "{line}");
            let Some(Answer::Line {
                flag: true,
                template,
                ..
            }) = &read.runs.get("r1")
            else {
                panic!("{line}: {:?}", read.runs);
            };
            assert!(
                !template.contains("**") && !template.contains('{'),
                "{template}"
            );
            assert!(read.salvaged.contains("r1"));
        }
        let refused = r#"{"runs": {"r1": {"status": "ok", "line": "Loss is **fine**."}}}"#;
        let read = read(refused, &[bind("r1", &run, true)], None, &render);
        assert!(
            read.runs.is_empty() && read.salvaged.is_empty(),
            "only a flag is kept"
        );
    }

    #[test]
    fn a_refused_citation_keeps_a_flags_valid_tokens() {
        let run = run();
        let line = "Loss **{loss}** rose.";
        let reply = json!({"runs": {"r1": {"status": "flag", "line": line,
            "evidence": [{"from": 99999, "to": 100000}]}}})
        .to_string();
        let read = read(&reply, &[bind("r1", &run, true)], None, &render);
        assert!(!read.errors.is_empty());
        assert!(read.salvaged.contains("r1"));
        assert!(
            matches!(&read.runs["r1"], Answer::Line {flag: true, template, evidence}
            if template == line && evidence.is_empty())
        );
    }

    #[test]
    fn every_broken_rule_is_said_in_words_the_model_can_fix() {
        let run = run();
        let cases = [
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "Val loss {val_loss} falling."}}}"#,
                "bold",
            ),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "Val loss **2.41** and falling."}}}"#,
                "\"2.41\" is a number that still moves",
            ),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "At step **340** of 600."}}}"#,
                "\"340\"",
            ),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "Loss **{los}**."}}}"#,
                "{los} is not a signal",
            ),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "Loss **{loss.vs}**."}}}"#,
                "has no comparison",
            ),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "Loss **{loss}** <b>"}}}"#,
                "plain words",
            ),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "Loss **{loss}** and some very long words that go on and on and on and on and on and on and on past the end."}}}"#,
                "characters",
            ),
            (r#"{"runs": {"r1": {"same": true}}}"#, "no sentence yet"),
            (
                r#"{"runs": {"r1": {"status": "maybe", "line": "x"}}}"#,
                "status",
            ),
            (
                r#"{"runs": {"r1": {"status": "flag", "line": "Loss **{loss}**.", "evidence": [{"from": 10, "to": 20}]}}}"#,
                "not a log excerpt",
            ),
            (r#"{"runs": {"r2": {"same": true}}}"#, "no run r2"),
            (r#"{"cancel": true}"#, "not part of a reply"),
            ("I think it looks fine", "not one JSON object"),
            (
                r#"{"runs": {"r1": {"status": "ok", "line": "**{count:\"(\"}** errors."}}}"#,
                "not a regex",
            ),
        ];
        for (reply, said) in cases {
            let read = read(reply, &[bind("r1", &run, false)], None, &render);
            assert!(
                read.errors.iter().any(|error| error.contains(said)),
                "{reply}: {:?}",
                read.errors
            );
        }
    }

    #[test]
    fn a_literal_of_any_sibling_moving_value_is_refused() {
        let first = run();
        let mut second = run();
        second.moving = vec![1.207];
        let read = read(
            r#"{"runs": {"r1": {"status": "ok", "line": "Loss **{loss}**, behind Depth 6 at 1.21."}, "r2": {"same": true}}}"#,
            &[bind("r1", &first, false), bind("r2", &second, true)],
            Some(&SummaryBounds {
                sentence: Some("{r1} loss **{r1:loss}**."),
                setup: false,
            }),
            &render,
        );
        assert!(
            read.errors.iter().any(|error| error.contains("\"1.21\"")),
            "{:?}",
            read.errors
        );
        assert_eq!(read.runs.get("r2"), Some(&Answer::Same));
    }

    #[test]
    fn a_step_can_anchor_a_trend_without_pinning_a_live_value() {
        let mut run = run();
        run.moving = vec![2.3104, 20.0, 340.0];
        let line = |text: &str| {
            let reply = json!({"runs": {"r1": {"status": "ok", "line": text}}}).to_string();
            read(&reply, &[bind("r1", &run, false)], None, &render).errors
        };
        assert!(line("Loss **{loss}**, climbing since step 20.").is_empty());
        assert!(line("Loss **{loss}** with 20 warnings.")[0].contains("\"20\""));
        assert!(!line("Loss **{loss}** at step 340.").is_empty());
    }

    #[test]
    fn evidence_names_the_lines_it_overlaps_and_a_run_may_name_itself() {
        let mut run = run();
        run.excerpts = vec![(100, 140), (140, 200), (300, 340)];
        // From the middle of one line to past the next, across a line the look left out.
        let kept = read(
            r#"{"runs": {"r1": {"status": "flag", "line": "Loss **{r1:loss}**.", "evidence": [{"from": 110, "to": 320}]}},
               "summary": {"status": "ok", "line": "unasked"}}"#,
            &[bind("r1", &run, false)],
            None,
            &render,
        );
        assert!(kept.errors.is_empty(), "{:?}", kept.errors);
        assert!(
            matches!(&kept.runs["r1"], Answer::Line { template, evidence, .. }
                if template == "Loss **{loss}**." && *evidence == [(100, 140), (140, 200), (300, 340)])
        );
        assert_eq!(kept.summary, None);
        let gap = read(
            r#"{"runs": {"r1": {"status": "flag", "line": "Loss **{loss}**.", "evidence": [{"from": 210, "to": 290}]}}}"#,
            &[bind("r1", &run, false)],
            None,
            &render,
        );
        assert!(
            gap.errors
                .iter()
                .any(|error| error.contains("not a log excerpt")),
            "{:?}",
            gap.errors
        );
    }

    #[test]
    fn a_flood_of_cited_lines_keeps_its_first_twelve() {
        let lines: Vec<(u64, u64)> = (0..40).map(|i| (i * 50, i * 50 + 50)).collect();
        assert_eq!(check_evidence(&[(0, 2000)], &lines).unwrap(), lines[..12]);
    }

    #[test]
    fn localized_digits_and_separators_still_read_as_the_number() {
        assert!(matches_moving("٢٫٣١", &[2.31]));
        assert_eq!(literal_moving("خسارة ٢٫٣١ الآن", &[2.31], &[]), Some("٢٫٣١"));
        assert!(matches_moving("2,31", &[2.31]));
        assert!(matches_moving("1,140", &[1140.0]));
        assert!(!matches_moving("600", &[340.0]));
        assert!(
            !matches_moving("0", &[0.0]),
            "zero is a resting point, said in words"
        );
    }

    #[test]
    fn summary_allows_plain_tokens_but_rejects_live_literals() {
        let a = run();
        let runs = [bind("r1", &a, true)];
        let bounds = SummaryBounds {
            sentence: None,
            setup: false,
        };
        let good = read(
            r#"{"summary":{"status":"ok","line":"{r1} leads at {r1:val_loss}."}}"#,
            &runs,
            Some(&bounds),
            &render,
        );
        assert!(good.errors.is_empty(), "{:?}", good.errors);
        let bad = read(
            r#"{"summary":{"status":"ok","line":"{r1} leads at 2.31."}}"#,
            &runs,
            Some(&bounds),
            &render,
        );
        assert!(!bad.errors.is_empty());
    }

    #[test]
    fn the_summary_names_each_run_by_its_key() {
        let a = run();
        let b = run();
        let runs = [bind("r1", &a, true), bind("r2", &b, true)];
        let bounds = SummaryBounds {
            sentence: None,
            setup: false,
        };
        let good = read(
            r#"{"summary": {"status": "ok", "line": "{r2} leads at val loss **{r2:val_loss}**; {r1} is close."}}"#,
            &runs,
            Some(&bounds),
            &render,
        );
        assert!(good.errors.is_empty(), "{:?}", good.errors);
        let bad = read(
            r#"{"summary": {"status": "ok", "line": "Depth leads at **{val_loss}**."}}"#,
            &runs,
            Some(&bounds),
            &render,
        );
        assert!(
            bad.errors
                .iter()
                .any(|error| error.contains("names no run")),
            "{:?}",
            bad.errors
        );
    }

    #[test]
    fn a_fenced_reply_is_read_and_counts_are_bounded() {
        let run = run();
        let read_fenced = read(
            "```json\n{\"runs\": {\"r1\": {\"status\": \"ok\", \"line\": \"**{count:\\\"timed out\\\"}** retries since step 300.\"}}}\n```",
            &[bind("r1", &run, false)],
            None,
            &render,
        );
        assert!(read_fenced.errors.is_empty(), "{:?}", read_fenced.errors);
        let many = read(
            r#"{"runs": {"r1": {"status": "ok", "line": "**{count:\"a\"}** {count:\"b\"} {count:\"c\"}."}}}"#,
            &[bind("r1", &run, false)],
            None,
            &render,
        );
        assert!(
            many.errors
                .iter()
                .any(|error| error.contains("new patterns")),
            "{:?}",
            many.errors
        );
    }

    #[test]
    fn live_values_cannot_be_pinned_to_a_fixed_reading_or_kept_that_way() {
        let run = run();
        for line in [
            "Loss **{loss}** at step 0.",
            "Step 0: loss **{loss}**.",
            "Loss **{loss}**, the first reading.",
            "Loss **{loss}** at 00:00.",
        ] {
            let answer = serde_json::json!({"runs": {"r1": {"status": "ok", "line": line}}});
            let read_line = read(
                &answer.to_string(),
                &[bind("r1", &run, false)],
                None,
                &render,
            );
            assert!(!read_line.errors.is_empty(), "{line}");
            let mut bound = bind("r1", &run, true);
            bound.sentence = Some(line);
            let kept = read(r#"{"runs":{"r1":{"same":true}}}"#, &[bound], None, &render);
            assert!(!kept.errors.is_empty(), "{line}");
        }
        for line in [
            "Loss **1.75** at step 0.",
            "Loss **{loss}** now.",
            "Loss **{loss}** lower since step 0.",
            "Loss **1.75** at step 0; now {loss}.",
        ] {
            let answer = serde_json::json!({"runs": {"r1": {"status": "ok", "line": line}}});
            let read = read(
                &answer.to_string(),
                &[bind("r1", &run, false)],
                None,
                &render,
            );
            assert!(read.errors.is_empty(), "{line}: {:?}", read.errors);
        }
    }
}
