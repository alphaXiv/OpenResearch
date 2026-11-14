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
/// Comparisons in a row a signal is far worse than the parent's before it is flagged.
const WORSE_IN_A_ROW: u32 = 3;

/// A signal's comparisons with the parent: the step last compared, and at how many steps in a
/// row it was far worse.
pub(super) type Compared = (f64, u32);

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

/// `covered` names the tripwires and signals a rule the run was launched with already checks: that
/// rule decides, and the tripwire stays out of its way.
fn conditions(
    seen: &Seen,
    run: &Now,
    covered: &BTreeSet<(&str, String)>,
    now: i64,
) -> Vec<Tripped> {
    let mut tripped = Vec::new();
    let signals = &run.body["view"]["signals"];
    let label = |name: &str| signals[name]["label"].as_str().unwrap_or(name).to_owned();
    let stage = run.summary["stage"]["name"].as_str().unwrap_or_default();
    let done = run.summary["stage"]["done"].as_f64();
    let at_step = done.map_or_else(String::new, |done| format!(" at step {done}"));
    for (name, value) in run.body["latest"].as_object().into_iter().flatten() {
        if covered.contains(&("nonFinite", name.clone())) {
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
    for (name, vs) in run.body["versus"]["step"].as_object().into_iter().flatten() {
        let held = seen.compared.get(name).map_or(0, |(_, held)| *held);
        let spec = &signals[name];
        if covered.contains(&("parent", name.clone()))
            || held < WORSE_IN_A_ROW
            || !far_worse(vs, spec)
        {
            continue;
        }
        let mine = super::super::monitor::value_text(&run.body["latest"][name], spec);
        let theirs = super::super::monitor::value_text(&vs["parent"], spec);
        let at = vs["step"]
            .as_f64()
            .map_or_else(String::new, |step| format!(" at step {step}"));
        tripped.push(Tripped {
            kind: "parent",
            fingerprint: format!("parent:{name}:{stage}"),
            template: format!(
                "{} **{mine}**, {}{at}.",
                plain(&label(name)),
                versus_text(Some(vs), spec)
            ),
            text: format!(
                "{} is {mine} against the parent's {theirs}{at}, far worse on {held} comparisons in a row.",
                label(name)
            ),
            evidence: Evidence::Tail,
        });
    }
    let scan = seen.scan.as_ref();
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
            None => digest::clip(line.text.trim(), digest::MAX_LINE),
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

/// Counts, for each signal compared with the parent, the comparisons in a row at a new step that
/// were far worse.
fn count_worse(seen: &mut Seen, run: &Now) {
    let signals = &run.body["view"]["signals"];
    for (name, vs) in run.body["versus"]["step"].as_object().into_iter().flatten() {
        let Some(step) = vs["step"].as_f64() else {
            continue;
        };
        let (last, held) = seen
            .compared
            .entry(name.clone())
            .or_insert((f64::NEG_INFINITY, 0));
        if step > *last {
            *last = step;
            *held = if far_worse(vs, &signals[name]) {
                *held + 1
            } else {
                0
            };
        }
    }
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
    let (ident, _, _, len) = bounded(read_lines(run, None, 0, 1)).await.ok()?;
    let from = len.unwrap_or(0).saturating_sub(TAIL);
    let (ident, mut lines, ..) = bounded(read_lines(run, Some(&ident), from, TAIL))
        .await
        .ok()?;
    // A read that starts mid-line starts with the rest of that line.
    if from > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    Some((ident, lines))
}

/// Raises each condition the run meets that it has not raised yet, while it has no open flag.
pub(super) async fn check(seen: &mut Seen, run: &Now, now: i64) {
    // What orx cannot see now flags nothing: lost contact says so on the run already.
    if !run.live() || run.body["availability"]["state"] != "live" {
        return;
    }
    count_worse(seen, run);
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
    let covered: BTreeSet<(&str, String)> = Store::open()
        .and_then(|store| {
            super::super::rules::frozen(
                &run.run.id,
                store.run_monitor_config(&run.run.id)?.as_deref(),
            )
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|rule| match rule.when {
            super::super::rules::When::NonFinite { signal, .. } => Some(("nonFinite", signal)),
            super::super::rules::When::WorseThanParent { signal, .. } => Some(("parent", signal)),
            _ => None,
        })
        .collect();
    for tripped in conditions(seen, run, &covered, now) {
        if seen.tripped.contains(&tripped.fingerprint) {
            continue;
        }
        let evidence = match &tripped.evidence {
            Evidence::Line(line) => {
                let ident = seen.scan.as_ref().and_then(|scan| scan.ident.clone());
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
                seen.tripped.insert(tripped.fingerprint);
                return;
            }
            Ok(Some(Raise::Waits)) => return,
            Ok(_) => {
                seen.tripped.insert(tripped.fingerprint);
            }
            Err(error) => eprintln!(
                "orx up: tripwire {} on run {}: {error:#}",
                tripped.fingerprint, run.run.id
            ),
        }
    }
}
