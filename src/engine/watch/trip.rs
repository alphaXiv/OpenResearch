//! Tripwires: what orx flags on its own, with no model, on every live run, watched or not. A
//! tripwire only flags, and raises each condition once a run.

use super::*;
use crate::store::Raise;

/// Words that make a kind of log line an error rather than a warning.
const ERROR_WORDS: [&str; 8] = [
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
/// The comparisons with the parent a verdict takes the median of, so one odd point decides nothing.
const SMOOTHED: usize = 5;
/// Comparisons kept to judge the parent's own scatter by.
const KEPT: usize = 14;
/// How far into a stage a run is before it is judged against its parent: a healthy run may
/// start slower.
const JUDGED_FROM: f64 = 0.1;
/// How many times the parent's own scatter a gap must pass to be more than noise.
const SPREADS: f64 = 3.0;

/// A signal's comparisons with the parent in one stage, oldest first: step, the run's value,
/// the parent's.
#[derive(Default)]
pub(super) struct Compared {
    stage: (String, usize),
    points: Vec<(f64, f64, f64)>,
}

impl Compared {
    /// Whether the run is far worse than its parent: over its last few comparisons, at least
    /// 1.5 times worse, unless the better value is within noise of zero, and by a gap past the
    /// parent's own scatter; never in the stage's first tenth.
    fn far_worse(&self, lower_is_better: bool, of: Option<f64>) -> bool {
        let recent = &self.points[self.points.len().saturating_sub(SMOOTHED)..];
        let Some(&(step, ..)) = recent.last().filter(|_| recent.len() == SMOOTHED) else {
            return false;
        };
        if of.is_some_and(|of| step < JUDGED_FROM * of) {
            return false;
        }
        let theirs: Vec<(f64, f64)> = self.points.iter().map(|(x, _, y)| (*x, *y)).collect();
        let Some((_, scatter, _)) = trend(&theirs) else {
            return false;
        };
        let middle = |mut values: Vec<f64>| median(&mut values);
        let mine = middle(recent.iter().map(|point| point.1).collect());
        let parent = middle(recent.iter().map(|point| point.2).collect());
        let (worse, better) = if lower_is_better {
            (mine, parent)
        } else {
            (parent, mine)
        };
        let noise = SPREADS * scatter;
        worse - better > noise && (better <= noise || worse >= 1.5 * better)
    }
}

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
    let prominent: BTreeSet<&str> = run.body["view"]["headline"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .chain(
            run.body["view"]["blocks"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|block| {
                    block["chart"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                }),
        )
        .collect();
    let of = run.summary["stage"]["of"].as_f64();
    for (name, vs) in run.body["versus"]["step"].as_object().into_iter().flatten() {
        let spec = &signals[name];
        let far = seen
            .compared
            .get(name)
            .is_some_and(|compared| compared.far_worse(spec["better"] == "lower", of));
        // The latest comparison is the one the sentence shows, so it must read worse too.
        if !prominent.contains(name.as_str())
            || covered.contains(&("parent", name.clone()))
            || vs["good"] != false
            || !far
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
                "{} is {mine} against the parent's {theirs}{at}, far worse over its last {SMOOTHED} comparisons.",
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
        .filter(|_| {
            run.body["phases"]
                .as_array()
                .and_then(|phases| phases.get(run.body["progress"]["phase"].as_u64()? as usize))
                .is_none_or(|phase| phase["endedAt"].is_null())
        })
        .filter(|stall| {
            scan.and_then(|scan| scan.progress.as_ref())
                .is_none_or(|(stage, _, at)| *stage != run.stage() || now - at >= stall / 2)
        })
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
    static ERROR: OnceLock<regex::Regex> = OnceLock::new();
    static TALLY: OnceLock<regex::Regex> = OnceLock::new();
    let error = ERROR.get_or_init(|| {
        regex::Regex::new(r"\b(?:[Ee][Rr][Rr][Oo][Rr]|[A-Z][A-Za-z]*Error)\b").unwrap()
    });
    let tally = TALLY.get_or_init(|| {
        regex::Regex::new(r"^\s*(?:\d+\s+errors?|errors?\s+(?:so far\s+)?[=:]?\s*\d+)\s*[.!]?\s*$")
            .unwrap()
    });
    for (kind, line) in scan.into_iter().flat_map(|scan| &scan.firsts) {
        let severe = ERROR_WORDS.iter().any(|word| kind.contains(word));
        let named_error =
            error.is_match(&line.text) && !tally.is_match(&line.text.to_ascii_lowercase());
        if !(severe || named_error) {
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

/// Keeps each comparison with the parent at a new step, afresh in each stage and whenever the
/// steps go back, as a relaunch from a checkpoint does.
fn compare(seen: &mut Seen, run: &Now) {
    let stage = run.stage();
    for (name, vs) in run.body["versus"]["step"].as_object().into_iter().flatten() {
        let (Some(step), Some(d), Some(theirs)) =
            (vs["step"].as_f64(), vs["d"].as_f64(), vs["parent"].as_f64())
        else {
            continue;
        };
        let compared = seen.compared.entry(name.clone()).or_default();
        let last = compared.points.last().map(|(x, ..)| *x);
        if compared.stage != stage || last.is_some_and(|last| step < last) {
            *compared = Compared {
                stage: stage.clone(),
                points: Vec::new(),
            };
        }
        if last.is_none_or(|last| step > last) || compared.points.is_empty() {
            compared.points.push((step, theirs + d, theirs));
            let extra = compared.points.len().saturating_sub(KEPT);
            compared.points.drain(..extra);
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
pub(super) async fn check(store: &std::sync::Mutex<Store>, seen: &mut Seen, run: &Now, now: i64) {
    // What orx cannot see now flags nothing: lost contact says so on the run already.
    if !run.live() || run.body["availability"]["state"] != "live" {
        return;
    }
    compare(seen, run);
    if run.summary["flag"]["state"] == "raised" {
        if let Err(error) = route(&super::lock(store), &run.run) {
            eprintln!(
                "orx up: waking run {}'s chats with its flag: {error:#}",
                run.run.id
            );
        }
    }
    if !run.summary["flag"].is_null() {
        return;
    }
    let config = super::lock(store).run_monitor_config(&run.run.id);
    let covered: BTreeSet<(&str, String)> = config
        .and_then(|config| super::super::rules::frozen(&run.run.id, config.as_deref()))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|rule| match rule.when {
            super::super::rules::When::NonFinite { signal, .. } => Some(("nonFinite", signal)),
            super::super::rules::When::WorseThanParent { signal, .. } => Some(("parent", signal)),
            _ => None,
        })
        .collect();
    let tripped = conditions(seen, run, &covered, now);
    let patterns = if tripped
        .iter()
        .any(|trip| matches!(trip.evidence, Evidence::Line(_)))
        && run.settings["patterns"]
            .as_array()
            .is_some_and(|patterns| !patterns.is_empty())
    {
        let patterns = super::super::view::stored_patterns(
            &run.run.experiment_id,
            Some(&run.settings.to_string()),
        )
        .and_then(|config| {
            orx_signals::Patterns::compile(&config)
                .map_err(|error| anyhow!("Pattern {}: {}", error.index + 1, error.reason))
        });
        match patterns {
            Ok(patterns) => Some(patterns),
            Err(error) => {
                eprintln!(
                    "orx up: checking run {}'s error lines against its patterns: {error:#}",
                    run.run.id
                );
                None
            }
        }
    } else {
        None
    };
    for tripped in tripped {
        if seen.tripped.contains(&tripped.fingerprint) {
            continue;
        }
        let evidence = match &tripped.evidence {
            Evidence::Line(line) => {
                if patterns
                    .as_ref()
                    .is_some_and(|patterns| patterns.reads_row(&line.text))
                {
                    continue;
                }
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
            "text": tripped.text,
            "evidence": evidence,
        });
        let raised = (|| {
            let store = super::lock(store);
            let flag = super::super::flags::new_flag(&store, &run.run, body, true)?;
            let chats = flag.wakes.iter().map(|(chat, _)| chat.clone()).collect();
            let raised = store.raise_flags(&[flag], now)?;
            if raised.first() == Some(&Raise::Raised) {
                super::super::monitor::advance(&store, &run.run, now)?;
                crate::commands::up::api_v2::notices_queued(chats);
            }
            Ok::<_, crate::error::Error>(raised.first().copied())
        })();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A run in `stage` (its name, and how many stages ended before it) of `of` steps, compared
    /// with its parent at `step`.
    fn at(stage: (&str, usize), of: f64, step: f64, mine: f64, theirs: f64) -> Now {
        let phases: Vec<Value> = (0..=stage.1)
            .map(|index| {
                let ended = (index < stage.1).then_some(1);
                json!({"name": stage.0, "startedAt": 1, "endedAt": ended})
            })
            .collect();
        Now {
            run: StoredRun {
                id: "r".into(),
                experiment_id: "e".into(),
                project_id: "p".into(),
                status: "running".into(),
                backend_json: "{}".into(),
                command: "train".into(),
                created_at: 0,
                updated_at: 0,
                ended_at: None,
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: None,
            },
            summary: json!({"stage": {"name": stage.0, "done": step, "of": of}}),
            body: json!({"phases": phases, "versus": {"step": {"loss": {
                "step": step, "d": mine - theirs, "parent": theirs, "good": mine < theirs}}}}),
            label: "r".into(),
            slug: "e".into(),
            settings: Value::Null,
            looks: Vec::new(),
            folded: None,
            noted: Vec::new(),
        }
    }

    /// A parent's loss, falling with a little noise.
    fn parent(step: f64) -> f64 {
        2.0 / (1.0 + step / 50.0) + 0.01 * (step * 1.7).sin()
    }

    fn far(seen: &Seen, of: f64) -> bool {
        seen.compared["loss"].far_worse(true, Some(of))
    }

    #[test]
    fn advancing_log_counters_defer_a_stall_but_repeated_or_old_ones_do_not() {
        let mut seen = Seen {
            scan: Some(Scan::default()),
            ..Seen::default()
        };
        let mut run = at(("tokenizer", 0), 2.0, 0.0, 0.0, 0.0);
        run.body["progress"] = json!({"phase": 0, "stallMs": 120_000});
        let stalled = |seen: &Seen, run: &Now, now| {
            conditions(seen, run, &BTreeSet::new(), now)
                .iter()
                .any(|c| c.kind == "stall")
        };
        let scan = seen.scan.as_mut().unwrap();
        scan.observe_progress(
            &run.stage(),
            "INFO - Progress: 99% (7848/7927 merges)",
            110_000,
        );
        scan.observe_progress(
            &run.stage(),
            "INFO - Progress: 100% (7927/7927 merges)",
            120_000,
        );
        scan.observe_progress(
            &run.stage(),
            "INFO - Finished training: 7927 merges completed",
            120_001,
        );
        assert!(!stalled(&seen, &run, 120_001));
        seen.scan.as_mut().unwrap().observe_progress(
            &run.stage(),
            "INFO - Progress: 100% (7927/7927 merges)",
            240_000,
        );
        assert!(stalled(&seen, &run, 240_000));
        seen.scan.as_mut().unwrap().observe_progress(
            &run.stage(),
            "Progress: 10% (10/100 batches)",
            240_000,
        );
        assert!(!stalled(&seen, &run, 240_001));
        run.summary["stage"]["name"] = json!("train");
        assert!(stalled(&seen, &run, 120_001));
    }

    #[test]
    fn an_ended_stage_is_never_stalled() {
        let seen = Seen::default();
        let mut run = at(("tokenizer", 0), 2.0, 0.0, 0.0, 0.0);
        run.body["progress"] = json!({"phase": 0, "stallMs": 120_000});
        run.body["phases"][0]["endedAt"] = json!(1);
        run.body["phases"]
            .as_array_mut()
            .unwrap()
            .push(json!({"name":"train","endedAt":null}));
        assert!(conditions(&seen, &run, &BTreeSet::new(), 120_000)
            .iter()
            .all(|c| c.kind != "stall"));
    }

    #[test]
    fn stats_only_parent_gaps_do_not_raise_flags() {
        let mut seen = Seen::default();
        let mut run = at(("train", 0), 200.0, 0.0, 5.0, 1.0);
        run.body["view"] = json!({"headline":["val_bpb"],"signals":{"loss":{"better":"lower"}},"blocks":[{"stats":["loss"]}]});
        for step in 20..30 {
            run.body["versus"]["step"]["loss"]["step"] = json!(step);
            compare(&mut seen, &run);
        }
        assert!(conditions(&seen, &run, &BTreeSet::new(), 0).is_empty());
        run.body["view"]["blocks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"chart":["loss"]}));
        assert!(conditions(&seen, &run, &BTreeSet::new(), 0)
            .iter()
            .any(|c| c.kind == "parent"));
    }

    #[test]
    fn a_collapse_is_far_worse_once_a_tenth_of_its_stage_is_in() {
        let mut seen = Seen::default();
        let mut first = None;
        for step in 1..=40 {
            let step = f64::from(step);
            compare(
                &mut seen,
                &at(("train", 0), 200.0, step, 3.0 * parent(step), parent(step)),
            );
            if far(&seen, 200.0) {
                first.get_or_insert(step);
            }
        }
        assert_eq!(first, Some(20.0), "judged from step 20 of 200");
    }

    #[test]
    fn a_new_stage_or_a_relaunch_starts_its_comparisons_over() {
        let mut seen = Seen::default();
        for step in 1..=100 {
            let step = f64::from(step);
            compare(
                &mut seen,
                &at(
                    ("pretrain", 0),
                    100.0,
                    step,
                    3.0 * parent(step),
                    parent(step),
                ),
            );
        }
        assert!(far(&seen, 100.0));

        compare(
            &mut seen,
            &at(("finetune", 1), 100.0, 1.0, 3.0 * parent(1.0), parent(1.0)),
        );
        assert!(!far(&seen, 100.0), "nothing carries into the next stage");
        for step in 2..=20 {
            let step = f64::from(step);
            compare(
                &mut seen,
                &at(
                    ("finetune", 1),
                    100.0,
                    step,
                    3.0 * parent(step),
                    parent(step),
                ),
            );
        }
        assert!(
            far(&seen, 100.0),
            "a collapse in the second stage, whose steps start at 0, counts"
        );

        compare(
            &mut seen,
            &at(
                ("finetune", 1),
                100.0,
                12.0,
                3.0 * parent(12.0),
                parent(12.0),
            ),
        );
        assert_eq!(
            seen.compared["loss"].points.len(),
            1,
            "a relaunch starts over"
        );
    }

    #[test]
    fn a_slower_start_or_a_gap_within_noise_of_zero_is_not_far_worse() {
        let mut slower = Seen::default();
        for step in 1..=60 {
            let step = f64::from(step);
            // Twice the parent's loss early on, closing to level by step 40.
            let lag = 1.0 + (1.0 - step / 40.0).max(0.0);
            compare(
                &mut slower,
                &at(("train", 0), 400.0, step, lag * parent(step), parent(step)),
            );
            assert!(!far(&slower, 400.0), "step {step}");
        }

        let noisy = |step: f64| 0.002 + 0.001 * (step * 2.3).sin();
        let mut near_zero = Seen::default();
        for step in 1..=60 {
            let step = f64::from(step);
            compare(
                &mut near_zero,
                &at(("train", 0), 100.0, step, noisy(step) + 0.002, noisy(step)),
            );
        }
        assert!(
            !far(&near_zero, 100.0),
            "twice a loss near zero, within its noise"
        );
        for step in 61..=70 {
            let step = f64::from(step);
            compare(
                &mut near_zero,
                &at(("train", 0), 100.0, step, 0.05, noisy(step)),
            );
        }
        assert!(
            far(&near_zero, 100.0),
            "a jump far past its noise is, ratio or not"
        );
    }
}
