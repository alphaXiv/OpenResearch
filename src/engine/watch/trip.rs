//! Tripwires: what orx flags on its own, with no model, on every live run, watched or not. A
//! tripwire only flags, and raises each condition once a run.

use super::*;
use crate::store::Raise;

/// Words that make a kind of log line an error rather than a warning.
const ERROR_WORDS: [&str; 9] = [
    "error",
    "exception",
    "traceback",
    "fatal",
    "critical",
    "out of memory",
    "killed",
    "segmentation fault",
    "abort",
];
/// How long a run that reports no progress may print nothing before it is flagged.
const SILENT_S: u64 = 600;
const TAIL: u64 = 16 * 1024;
const TAIL_LINES: usize = 6;

/// One condition a run meets now.
struct Tripped {
    kind: &'static str,
    fingerprint: String,
    template: String,
    text: String,
    evidence: Evidence,
}

/// Which lines a tripwire cites.
enum Evidence {
    /// The log's last lines a person reads.
    Tail,
    /// The last lines that print a value that is not a number.
    NonFinite,
    Line(digest::Line),
}

/// Text orx writes into a sentence, with nothing the page would read as a token.
fn plain(text: &str) -> String {
    text.replace(['{', '}'], "").replace("**", "")
}

/// `covered` names the signals a rule the run was launched with already checks for a value that
/// is not a number: that rule decides, and the tripwire stays out of its way.
fn conditions(state: &State, run: &Now, covered: &BTreeSet<String>, now: i64) -> Vec<Tripped> {
    let mut tripped = Vec::new();
    let signals = &run.body["view"]["signals"];
    let label = |name: &str| signals[name]["label"].as_str().unwrap_or(name).to_owned();
    let stage = run.summary["stage"]["name"].as_str().unwrap_or_default();
    let done = run.summary["stage"]["done"].as_f64();
    let at_step = done.map_or_else(String::new, |done| format!(" at step {done}"));
    for (name, value) in run.body["latest"].as_object().into_iter().flatten() {
        if covered.contains(name) {
            continue;
        }
        let Some(spelled) = value
            .as_str()
            .filter(|v| ["NaN", "Infinity", "-Infinity"].contains(v))
        else {
            continue;
        };
        tripped.push(Tripped {
            kind: "nonFinite",
            fingerprint: format!("nonFinite:{name}:{stage}"),
            template: format!("{} became **{spelled}**{at_step}.", plain(&label(name))),
            text: format!("{} reported {spelled}{at_step}.", label(name)),
            evidence: Evidence::NonFinite,
        });
    }
    let scan = state.scans.get(&run.run.id);
    let silent_for = scan.map(|scan| now - scan.grew_at);
    if let Some(stall) = run.body["progress"]["stallMs"]
        .as_i64()
        .filter(|ms| *ms > 0)
    {
        // The last progress and the last line land moments apart: half the stall is silence.
        let log = if silent_for.is_some_and(|silent| silent >= stall / 2) {
            "its log is silent too"
        } else {
            "its log keeps printing"
        };
        let since = done.map_or_else(String::new, |done| format!(" since step {done}"));
        let of = match stage {
            "" => String::new(),
            stage => format!(" of {}", plain(stage)),
        };
        tripped.push(Tripped {
            kind: "stall",
            fingerprint: format!("stall:{stage}:{}", done.unwrap_or_default()),
            template: format!(
                "No progress for **{}**{since}{of}.",
                super::super::monitor::short(stall)
            ),
            text: format!(
                "no progress for {}{since}{of}; {log}.",
                super::super::monitor::short(stall)
            ),
            evidence: Evidence::Tail,
        });
    } else if run.body["progress"].is_null()
        && Phase::parse(&run.run.status) == Some(Phase::Running)
    {
        if let (Some(silent), Some(scan)) = (silent_for, scan) {
            if silent >= paced_ms(SILENT_S) {
                tripped.push(Tripped {
                    kind: "silence",
                    fingerprint: format!("silence:{}", scan.offset),
                    template: format!(
                        "Nothing printed for **{}** or more.",
                        super::super::monitor::short(silent)
                    ),
                    text: format!(
                        "its log has printed nothing for {} and it reports no progress.",
                        super::super::monitor::short(silent)
                    ),
                    evidence: Evidence::Tail,
                });
            }
        }
    }
    for (kind, line) in scan.into_iter().flat_map(|scan| &scan.firsts) {
        if !ERROR_WORDS.iter().any(|word| kind.contains(word)) {
            continue;
        }
        // The kind the line names, "torch.OutOfMemoryError", else its start; the evidence shows it whole.
        let text = match crate::local::share::find_secret(&line.text) {
            Some(kind) => digest::withheld_text(kind),
            None => line.text.trim().to_owned(),
        };
        let text = text.as_str();
        let shown: String = match text.split_once(": ") {
            Some((kind, _)) if kind.chars().count() <= 40 => kind.to_owned(),
            _ => text.chars().take(60).collect(),
        };
        tripped.push(Tripped {
            kind: "error",
            fingerprint: format!("error:{kind}"),
            template: format!("A new kind of error in the log: **{}**.", plain(&shown)),
            text: format!("it printed a kind of error it had not before: {text}"),
            evidence: Evidence::Line(line.clone()),
        });
    }
    tripped
}

/// Wakes the run's Agentic chats with its open flag, when none was woken with it yet: the chat
/// was Copilot when it was raised.
fn route(store: &Store, run: &StoredRun) -> Result<()> {
    let flags = super::super::flags::read(store, run)?;
    let Some(flag) = super::super::flags::open(&flags) else {
        return Ok(());
    };
    let id = flag["id"].as_str().unwrap_or_default();
    let mut chats = Vec::new();
    for (chat, body) in super::super::flags::wakes(store, run, flag)? {
        let dedupe = format!("{}:{id}", crate::store::WATCH_FLAG);
        let at = crate::store::now_ms();
        if store.insert_run_notice(&run.id, &chat, crate::store::WATCH_FLAG, &dedupe, &body, at)? {
            chats.push(chat);
        }
    }
    crate::commands::up::api_v2::notices_queued(chats);
    Ok(())
}

/// The run's log's last lines, and its identity.
async fn tail(run: &str) -> Option<(String, Vec<digest::Line>)> {
    let (ident, _, _, len) = read_lines(run, None, 0, 1).await.ok()?;
    let from = len.unwrap_or(0).saturating_sub(TAIL);
    let (ident, mut lines, ..) = read_lines(run, Some(&ident), from, TAIL).await.ok()?;
    // A read that starts mid-line starts with the rest of that line.
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Some((ident, lines))
}

/// Raises each condition the run meets that it has not raised yet, while it has no open flag.
pub(super) async fn check(state: &mut State, run: &Now, now: i64) {
    // What orx cannot see now flags nothing: lost contact says so on the run already.
    if !run.live() || run.body["availability"]["state"] != "live" {
        return;
    }
    if run.summary["flag"]["state"] == "raised" {
        if let Err(error) = Store::open().and_then(|store| route(&store, &run.run)) {
            eprintln!(
                "orx up: waking run {}'s chats with its flag: {error:#}",
                run.run.id
            );
        }
    }
    if !run.summary["flag"].is_null() {
        return;
    }
    let covered: BTreeSet<String> = Store::open()
        .and_then(|store| {
            super::super::rules::frozen(
                &run.run.id,
                store.run_monitor_config(&run.run.id)?.as_deref(),
            )
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|rule| match rule.when {
            super::super::rules::When::NonFinite { signal, .. } => Some(signal),
            _ => None,
        })
        .collect();
    for tripped in conditions(state, run, &covered, now) {
        let key = (run.run.id.clone(), tripped.fingerprint.clone());
        if state.tripped.contains(&key) {
            continue;
        }
        let evidence = match &tripped.evidence {
            Evidence::Line(line) => {
                let ident = state
                    .scans
                    .get(&run.run.id)
                    .and_then(|scan| scan.ident.clone());
                evidence_of(&run.run.id, ident.as_deref(), &[line]).await
            }
            wanted => match tail(&run.run.id).await {
                Some((ident, lines)) => {
                    let human = |line: &&digest::Line| {
                        !line.text.trim_start().starts_with("::orx::")
                            && !line.text.trim().is_empty()
                    };
                    let odd = |line: &&digest::Line| {
                        let lower = line.text.to_lowercase();
                        lower.contains("nan") || lower.contains("infinity")
                    };
                    let mut chosen: Vec<&digest::Line> = match wanted {
                        Evidence::NonFinite => lines.iter().filter(odd).collect(),
                        _ => lines.iter().filter(human).collect(),
                    };
                    chosen.drain(..chosen.len().saturating_sub(TAIL_LINES));
                    evidence_of(&run.run.id, Some(&ident), &chosen).await
                }
                None => Value::Null,
            },
        };
        let body = json!({
            "id": super::super::flags::new_id(),
            "source": "tripwire",
            "kind": tripped.kind,
            "fingerprint": tripped.fingerprint,
            "at": now,
            "whole": run.summary["whole"],
            "template": tripped.template,
            "detail": tripped.template,
            "text": tripped.text,
            "evidence": evidence,
        });
        let raised = Store::open().and_then(|store| {
            let flag = super::super::flags::new_flag(&store, &run.run, body, true)?;
            let chats = flag.wakes.iter().map(|(chat, _)| chat.clone()).collect();
            let raised = store.raise_flags(&[flag], now)?;
            if raised.first() == Some(&Raise::Raised) {
                super::super::monitor::advance(&store, &run.run, now)?;
                crate::commands::up::api_v2::notices_queued(chats);
            }
            Ok(raised.first().copied())
        });
        match raised {
            // One flag at a time: the next condition waits until this one is settled.
            Ok(Some(Raise::Raised)) => {
                state.tripped.insert(key);
                return;
            }
            Ok(Some(Raise::Waits)) => return,
            Ok(_) => {
                state.tripped.insert(key);
            }
            Err(error) => eprintln!(
                "orx up: tripwire {} on run {}: {error:#}",
                tripped.fingerprint, run.run.id
            ),
        }
    }
}
