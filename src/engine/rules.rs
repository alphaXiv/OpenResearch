//! Stop rules: conditions an agent declares on an experiment, frozen on each run at launch and
//! checked by `orx up` on each report the run makes, with no model involved.

use serde::Deserialize;
use serde_json::{json, Map, Value};

const MAX_RULES: usize = 32;
const MAX_NAME: usize = 64;
const MAX_TEXT: usize = 280;
const MAX_REPORTS: u64 = 256;
const MAX_SECONDS: u64 = 30 * 24 * 3600;
const KINDS: &str =
    "nonFinite, compare, noImprovement, worseThanParent, noProgress, costAbove, projectedFinishPastLimit";
/// The rule `orx exp run --budget` adds.
pub(crate) const BUDGET: &str = "budget";

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Does {
    Stop,
    Ask,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Better {
    Lower,
    Higher,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Op {
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum When {
    /// The signal reported a non-finite value on `reports` reports in a row.
    NonFinite { signal: String, reports: u64 },
    /// The signal's value met `op value` on `reports` reports in a row.
    Compare {
        signal: String,
        op: Op,
        value: f64,
        reports: u64,
    },
    /// The best of the last `reports` reports beat the best before them by less than `min`.
    NoImprovement {
        signal: String,
        min: f64,
        reports: u64,
        better: Better,
    },
    /// The signal was worse than the parent's run at the same step by more than `by`, on
    /// `reports` reports in a row.
    WorseThanParent {
        signal: String,
        by: f64,
        better: Better,
        reports: u64,
    },
    /// No progress was reported for `seconds`.
    NoProgress { seconds: u64 },
    /// What the run has cost, or would cost by its end, passed `usd`.
    CostAbove { usd: f64 },
    /// The run would finish after its time limit.
    FinishPastLimit,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rule {
    pub name: String,
    /// The agent's words for the rule; none for the budget orx adds, which the page words.
    pub text: Option<String>,
    pub does: Does,
    pub when: When,
}

impl Rule {
    /// The rule as it is stored and read back.
    pub(crate) fn json(&self) -> Value {
        let mut when = match &self.when {
            When::NonFinite { signal, reports } => json!({"nonFinite": signal, "reports": reports}),
            When::Compare {
                signal,
                op,
                value,
                reports,
            } => json!({
                "compare": {"signal": signal, "op": op_name(*op), "value": value},
                "reports": reports,
            }),
            When::NoImprovement {
                signal,
                min,
                reports,
                better,
            } => json!({"noImprovement": {
                "signal": signal, "min": min, "reports": reports, "better": better_name(*better),
            }}),
            When::WorseThanParent {
                signal,
                by,
                better,
                reports,
            } => json!({
                "worseThanParent": {"signal": signal, "by": by, "better": better_name(*better)},
                "reports": reports,
            }),
            When::NoProgress { seconds } => json!({"noProgress": {"seconds": seconds}}),
            When::CostAbove { usd } => json!({"costAbove": {"usd": usd}}),
            When::FinishPastLimit => json!({"projectedFinishPastLimit": true}),
        };
        if let Value::Object(when) = &mut when {
            when.retain(|key, value| key != "reports" || *value != json!(1));
        }
        let mut rule = json!({
            "name": self.name,
            "does": match self.does { Does::Stop => "stop", Does::Ask => "ask" },
            "when": when,
        });
        if let Some(text) = &self.text {
            rule["text"] = json!(text);
        }
        rule
    }

    /// What the page and status show of the rule: its name, words and action.
    pub(crate) fn shown(&self) -> Value {
        let mut shown = self.json();
        shown.as_object_mut().map(|rule| rule.remove("when"));
        shown["text"] = json!(self.text);
        if let When::CostAbove { usd } = self.when {
            shown["budgetUsd"] = json!(usd);
        }
        shown
    }
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Lt => "lt",
        Op::Le => "le",
        Op::Gt => "gt",
        Op::Ge => "ge",
    }
}

fn better_name(better: Better) -> &'static str {
    match better {
        Better::Lower => "lower",
        Better::Higher => "higher",
    }
}

/// The rule `orx exp run --budget USD` adds, or why the amount cannot be one.
pub(crate) fn budget(usd: f64) -> Result<Rule, String> {
    if !(usd.is_finite() && usd > 0.0) {
        return Err(format!(
            "--budget must be a finite USD amount greater than zero (got {usd})"
        ));
    }
    Ok(Rule {
        name: BUDGET.into(),
        text: None,
        does: Does::Stop,
        when: When::CostAbove { usd },
    })
}

/// Rules as an agent declared them, each checked, or the first mistake in its own words.
pub(crate) fn parse(value: &Value) -> Result<Vec<Rule>, String> {
    let rules = value
        .as_array()
        .ok_or("\"rules\" must be a list of rules")?;
    if rules.len() > MAX_RULES {
        return Err(format!(
            "\"rules\" has {} rules; a run takes at most {MAX_RULES}",
            rules.len()
        ));
    }
    let mut parsed: Vec<Rule> = Vec::new();
    for (index, rule) in rules.iter().enumerate() {
        let rule = parse_rule(rule).map_err(|why| match rule["name"].as_str() {
            Some(name) => format!("rules[{index}] ({name}): {why}"),
            None => format!("rules[{index}]: {why}"),
        })?;
        if parsed.iter().any(|seen| seen.name == rule.name) {
            return Err(format!(
                "rules[{index}]: another rule is already named \"{}\"",
                rule.name
            ));
        }
        parsed.push(rule);
    }
    Ok(parsed)
}

fn parse_rule(value: &Value) -> Result<Rule, String> {
    let rule = value
        .as_object()
        .ok_or("give {\"name\", \"text\", \"does\", \"when\"}")?;
    if let Some(key) = rule
        .keys()
        .find(|key| !["name", "text", "does", "when"].contains(&key.as_str()))
    {
        return Err(format!(
            "\"{key}\" is not part of a rule; a rule takes name, text, does and when"
        ));
    }
    let name = rule
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty() && name.chars().count() <= MAX_NAME)
        .ok_or(format!(
            "\"name\" must be text of 1 to {MAX_NAME} characters"
        ))?;
    let text = match rule.get("text") {
        None | Some(Value::Null) if name == BUDGET => None,
        Some(Value::String(text))
            if !text.trim().is_empty() && text.chars().count() <= MAX_TEXT =>
        {
            Some(text.trim().to_owned())
        }
        _ => {
            return Err(format!(
                "\"text\" must say in 1 to {MAX_TEXT} characters what the rule checks"
            ))
        }
    };
    let does = match rule.get("does").and_then(Value::as_str) {
        Some("stop") => Does::Stop,
        Some("ask") => Does::Ask,
        _ => return Err("\"does\" must be \"stop\" or \"ask\"".into()),
    };
    let when = rule
        .get("when")
        .and_then(Value::as_object)
        .ok_or(format!("\"when\" must be an object with one of {KINDS}"))?;
    Ok(Rule {
        name: name.to_owned(),
        text,
        does,
        when: parse_when(when)?,
    })
}

fn parse_when(when: &Map<String, Value>) -> Result<When, String> {
    let kinds: Vec<&String> = when.keys().filter(|key| *key != "reports").collect();
    let kind = match kinds.as_slice() {
        [kind] => kind.as_str(),
        [] => return Err(format!("\"when\" must have one of {KINDS}")),
        [first, second, ..] => {
            return Err(format!(
                "\"when\" has both \"{first}\" and \"{second}\"; a rule checks one condition"
            ))
        }
    };
    let condition = &when[kind];
    let reports = match when.get("reports") {
        None => None,
        Some(reports) => Some(count(reports, "reports")?),
    };
    let counted = |reports: Option<u64>| reports.unwrap_or(1);
    let alone = |reports: Option<u64>| match reports {
        Some(_) => Err(format!("\"{kind}\" takes no \"reports\"")),
        None => Ok(()),
    };
    let field = |name: &str| {
        condition
            .get(name)
            .ok_or(format!("\"{kind}\" needs \"{name}\""))
    };
    let fields = |allowed: &[&str]| {
        let object = condition
            .as_object()
            .ok_or(format!("\"{kind}\" must be an object"))?;
        match object.keys().find(|key| !allowed.contains(&key.as_str())) {
            Some(key) => Err(format!(
                "\"{kind}\" takes {}, not \"{key}\"",
                allowed.join(", ")
            )),
            None => Ok(()),
        }
    };
    let signal = |value: &Value| {
        value
            .as_str()
            .filter(|name| orx_signals::identifier(name))
            .map(str::to_owned)
            .ok_or_else(|| format!("\"{kind}\" must name a signal the run reports"))
    };
    let finite = |value: &Value, name: &str| {
        value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("\"{name}\" must be a finite number"))
    };
    let better = |value: &Value| match value.as_str() {
        Some("lower") => Ok(Better::Lower),
        Some("higher") => Ok(Better::Higher),
        _ => Err("\"better\" must be \"lower\" or \"higher\"".to_owned()),
    };
    Ok(match kind {
        "nonFinite" => When::NonFinite {
            signal: signal(condition)?,
            reports: counted(reports),
        },
        "compare" => {
            fields(&["signal", "op", "value"])?;
            When::Compare {
                signal: signal(field("signal")?)?,
                op: match field("op")?.as_str() {
                    Some("lt") => Op::Lt,
                    Some("le") => Op::Le,
                    Some("gt") => Op::Gt,
                    Some("ge") => Op::Ge,
                    _ => return Err("\"op\" must be \"lt\", \"le\", \"gt\" or \"ge\"".into()),
                },
                value: finite(field("value")?, "value")?,
                reports: counted(reports),
            }
        }
        "noImprovement" => {
            alone(reports)?;
            fields(&["signal", "min", "reports", "better"])?;
            When::NoImprovement {
                signal: signal(field("signal")?)?,
                min: finite(field("min")?, "min")?.max(0.0),
                reports: count(field("reports")?, "reports")?,
                better: better(field("better")?)?,
            }
        }
        "worseThanParent" => {
            fields(&["signal", "by", "better"])?;
            When::WorseThanParent {
                signal: signal(field("signal")?)?,
                by: finite(field("by")?, "by")?.max(0.0),
                better: better(field("better")?)?,
                reports: counted(reports),
            }
        }
        "noProgress" => {
            alone(reports)?;
            fields(&["seconds"])?;
            When::NoProgress {
                seconds: field("seconds")?
                    .as_u64()
                    .filter(|seconds| (1..=MAX_SECONDS).contains(seconds))
                    .ok_or(format!(
                        "\"seconds\" must be a whole number from 1 to {MAX_SECONDS}"
                    ))?,
            }
        }
        "costAbove" => {
            alone(reports)?;
            fields(&["usd"])?;
            let usd = finite(field("usd")?, "usd")?;
            if usd <= 0.0 {
                return Err("\"usd\" must be greater than zero".into());
            }
            When::CostAbove { usd }
        }
        "projectedFinishPastLimit" => {
            alone(reports)?;
            if condition != &Value::Bool(true) {
                return Err("\"projectedFinishPastLimit\" must be true".into());
            }
            When::FinishPastLimit
        }
        other => {
            return Err(format!(
                "unknown condition \"{other}\"; this orx checks {KINDS}"
            ))
        }
    })
}

fn count(value: &Value, name: &str) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|count| (1..=MAX_REPORTS).contains(count))
        .ok_or(format!(
            "\"{name}\" must be a whole number from 1 to {MAX_REPORTS}"
        ))
}

/// The experiment's declared rules as stored; none when none are declared.
pub(crate) fn stored(experiment: &str, raw: Option<&str>) -> crate::error::Result<Vec<Rule>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let settings: Value = serde_json::from_str(raw).map_err(|error| {
        crate::error::anyhow!("Experiment {experiment}'s run view settings are unreadable: {error}")
    })?;
    match settings.get("rules") {
        None => Ok(Vec::new()),
        Some(rules) => parse(rules).map_err(|why| {
            crate::error::anyhow!(
                "Experiment {experiment}'s rules: {why}. Update orx before launching this rule, or set the experiment's rules again with `orx exp view set {experiment} --config PATH`."
            )
        }),
    }
}

/// What a run launched now freezes: the experiment's rules, and the budget it was launched with.
/// `None` when it has no rules.
pub(crate) fn freeze(
    experiment: &str,
    settings: Option<&str>,
    budget_usd: Option<f64>,
) -> crate::error::Result<Option<String>> {
    let mut rules = stored(experiment, settings)?;
    if let Some(usd) = budget_usd {
        rules.retain(|rule| rule.name != BUDGET);
        rules.push(budget(usd).map_err(|why| crate::error::anyhow!("{why}"))?);
    }
    Ok((!rules.is_empty())
        .then(|| json!({"rules": rules.iter().map(Rule::json).collect::<Vec<_>>()}).to_string()))
}

/// The rules a run was launched with; none for a run launched without any, or before rules.
pub(crate) fn frozen(run: &str, config: Option<&str>) -> crate::error::Result<Vec<Rule>> {
    let Some(config) = config else {
        return Ok(Vec::new());
    };
    let config: Value = serde_json::from_str(config)
        .map_err(|error| crate::error::anyhow!("Run {run}'s rules are unreadable: {error}"))?;
    parse(&config["rules"])
        .map_err(|why| crate::error::anyhow!("Run {run}'s rules: {why}. Update orx to check them."))
}

/// What a rule has counted on one run, kept in the run's monitor state so a restart neither
/// counts a report twice nor acts twice.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    /// Where on the signal's axis the last report this rule counted was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<f64>,
    /// The checkpoint revision a rule on the estimate last counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rev: Option<u64>,
    /// Reports, or revisions, in a row that met the condition, from `since`.
    #[serde(default)]
    held: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    since: Option<f64>,
    /// No improvement: the reports in its window, and the best before them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    window: Vec<(f64, Option<f64>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    best: Option<f64>,
    /// The rule acted; it never acts again on this run.
    #[serde(default)]
    tripped: bool,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

/// What a live run's rules read: its checkpoint and its parent's, and the monitor projected
/// from them.
pub(crate) struct Facts<'a> {
    pub snapshot: &'a orx_signals::Snapshot,
    pub parent: Option<&'a orx_signals::Snapshot>,
    /// The monitor's body: its axis, progress, estimate and cost.
    pub body: &'a Value,
    /// Slices of progress the estimate has timed.
    pub slices: usize,
    pub started_at: i64,
    pub now: i64,
}

/// A rule that met its condition, with what orx saw.
pub(crate) struct Trip {
    pub rule: Rule,
    pub evidence: String,
}

/// What checking a run's rules changed: every rule's count, and the rules that tripped now.
pub(crate) struct Checked {
    pub counts: Value,
    pub trips: Vec<Trip>,
}

/// An estimate rule acts only on a pace measured over this many slices, seen on two revisions.
const SLICES_BEFORE_PROJECTING: usize = 4;
const REVISIONS_BEFORE_PROJECTING: u64 = 2;

/// Counts the reports the run made since the rules last looked, and trips each rule whose
/// condition holds once they are counted, so a value that has already recovered stops nothing.
/// `counts` is what the run's monitor state kept. `None` when nothing changed.
pub(crate) fn check(rules: &[Rule], counts: Option<&Value>, facts: &Facts) -> Option<Checked> {
    let mut kept: Map<String, Value> = counts
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut trips = Vec::new();
    let mut changed = false;
    for rule in rules {
        // A count this orx cannot read is left as it is: acting on it could stop a run twice.
        let was: Count = match kept.get(&rule.name).map(Count::deserialize) {
            None => Count::default(),
            Some(Ok(count)) => count,
            Some(Err(_)) => continue,
        };
        if was.tripped {
            continue;
        }
        let mut count = was.clone();
        if let Some(evidence) = step(rule, &mut count, facts) {
            count.tripped = true;
            trips.push(Trip {
                rule: rule.clone(),
                evidence,
            });
        }
        if count != was {
            changed = true;
            kept.insert(rule.name.clone(), json!(count));
        }
    }
    changed.then(|| Checked {
        counts: Value::Object(kept),
        trips,
    })
}

/// Counts what is new for `rule`; what orx saw when its condition holds now.
fn step(rule: &Rule, count: &mut Count, facts: &Facts) -> Option<String> {
    let body = facts.body;
    let at = |x: f64| match body["x"]["name"].as_str() {
        Some("step") => format!("step {}", super::monitor::number(x)),
        _ => format!("{} in", super::monitor::short((x * 1000.0) as i64)),
    };
    let span = |count: &Count, last: f64| match count.since {
        Some(since) if count.held > 1 => format!(
            "on {} reports in a row, {} to {}",
            count.held,
            at(since),
            at(last).trim_start_matches("step ")
        ),
        _ => format!("at {}", at(last)),
    };
    let value = |value: f64| super::monitor::measure(value, &Value::Null);
    match &rule.when {
        When::NonFinite { signal, reports } => {
            let fresh = fresh(facts, signal, count);
            let last = fresh.last()?.0;
            for (x, y) in fresh {
                held(count, x, y.is_none());
            }
            let word = facts.snapshot.latest[signal.as_str()]
                .as_str()
                .unwrap_or("not a finite number");
            (count.held >= *reports).then(|| format!("{signal} was {word} {}", span(count, last)))
        }
        When::Compare {
            signal,
            op,
            value: bound,
            reports,
        } => {
            let fresh = fresh(facts, signal, count);
            let (last, newest) = *fresh.last()?;
            for (x, y) in fresh {
                let holds = y.is_some_and(|y| match op {
                    Op::Lt => y < *bound,
                    Op::Le => y <= *bound,
                    Op::Gt => y > *bound,
                    Op::Ge => y >= *bound,
                });
                held(count, x, holds);
            }
            let word = match op {
                Op::Lt => "below",
                Op::Le => "at or below",
                Op::Gt => "above",
                Op::Ge => "at or above",
            };
            (count.held >= *reports).then(|| {
                format!(
                    "{signal} was {} ({word} {}) {}",
                    newest.map_or("not a number".into(), value),
                    value(*bound),
                    span(count, last)
                )
            })
        }
        When::WorseThanParent {
            signal,
            by,
            better,
            reports,
        } => {
            let theirs = facts.parent.and_then(|parent| parent.series.get(signal));
            let fresh = fresh(facts, signal, count);
            let last = fresh.last()?.0;
            let mut seen = None;
            for (x, y) in fresh {
                // Where the parent has no value at this step, the report counts neither way.
                let Some((mine, parent)) =
                    y.zip(theirs.and_then(|theirs| super::monitor::parent_at(theirs, x)))
                else {
                    continue;
                };
                let worse = match better {
                    Better::Lower => mine - parent,
                    Better::Higher => parent - mine,
                };
                held(count, x, worse > *by);
                seen = Some((mine, parent, worse));
            }
            let (mine, parent, worse) = seen?;
            (count.held >= *reports).then(|| {
                format!(
                    "{signal} was {} worse than the parent's at the same step ({} against {}) {}",
                    value(worse),
                    value(mine),
                    value(parent),
                    span(count, last)
                )
            })
        }
        When::NoImprovement {
            signal,
            min,
            reports,
            better,
        } => {
            let fresh = fresh(facts, signal, count);
            let last = fresh.last()?.0;
            let best = |values: &mut dyn Iterator<Item = f64>| match better {
                Better::Lower => values.filter(|v| v.is_finite()).min_by(f64::total_cmp),
                Better::Higher => values.filter(|v| v.is_finite()).max_by(f64::total_cmp),
            };
            for point in fresh {
                count.window.push(point);
                if count.window.len() as u64 > *reports {
                    let (_, left) = count.window.remove(0);
                    count.best = best(&mut count.best.into_iter().chain(left));
                }
            }
            let before = count.best?;
            if (count.window.len() as u64) < *reports {
                return None;
            }
            let since = best(&mut count.window.iter().filter_map(|point| point.1));
            let gained = since.map_or(0.0, |since| match better {
                Better::Lower => before - since,
                Better::Higher => since - before,
            });
            (gained < *min).then(|| {
                format!(
                    "{signal} improved by {} over its last {reports} reports, {} to {}, on its best of {} before them; the rule needs {}",
                    value(gained.max(0.0)),
                    at(count.window[0].0),
                    at(last).trim_start_matches("step "),
                    value(before),
                    value(*min)
                )
            })
        }
        When::NoProgress { seconds } => {
            let progress = &body["progress"];
            let since = if progress.is_null() {
                facts.started_at
            } else {
                progress["lastAt"].as_i64()?
            };
            let silent = facts.now - since;
            (silent >= *seconds as i64 * 1000).then(|| {
                let from = match progress["done"].as_f64() {
                    Some(done) if !progress.is_null() => format!("since {}", at(done)),
                    _ => "since it started".into(),
                };
                format!(
                    "no progress was reported for {} {from}",
                    super::monitor::short(silent)
                )
            })
        }
        When::CostAbove { usd } => {
            let cost = &body["cost"];
            let spent = cost["soFar"].as_f64().or(cost["spend"].as_f64());
            if let Some(spent) = spent.filter(|spent| spent >= usd) {
                return Some(format!(
                    "it has cost ${spent:.2}, past its ${usd:.2} budget"
                ));
            }
            let projected = cost["projectedRange"][1]
                .as_f64()
                .or(cost["projected"].as_f64());
            projecting(count, facts, projected.is_some_and(|p| p > *usd)).then(|| {
                format!(
                    "it would cost up to ${:.2} by its end, past its ${usd:.2} budget",
                    projected.unwrap_or_default()
                )
            })
        }
        When::FinishPastLimit => {
            let past = body["estimate"]["pastLimitMs"]
                .as_i64()
                .filter(|ms| *ms > 0);
            projecting(count, facts, past.is_some()).then(|| {
                format!(
                    "it would finish up to {} past its time limit",
                    super::monitor::short(past.unwrap_or_default())
                )
            })
        }
    }
}

/// Counts one report toward a run of reports in a row, or ends the run when it does not hold.
fn held(count: &mut Count, x: f64, holds: bool) {
    if !holds {
        count.held = 0;
        count.since = None;
        return;
    }
    if count.held == 0 {
        count.since = Some(x);
    }
    count.held += 1;
}

/// The run's reports of `signal` that `count` has not counted, which it now has. A run that
/// went back to an earlier step, as a relaunch from a checkpoint does, counts its newest report.
fn fresh(facts: &Facts, signal: &str, count: &mut Count) -> Vec<(f64, Option<f64>)> {
    let Some(points) = facts.snapshot.series.get(signal) else {
        return Vec::new();
    };
    let Some(&last) = points.last() else {
        return Vec::new();
    };
    let fresh: Vec<_> = match count.after {
        Some(after) if last.0 < after => vec![last],
        Some(after) => points.iter().copied().filter(|p| p.0 > after).collect(),
        None => points.clone(),
    };
    if !fresh.is_empty() {
        count.after = Some(last.0);
    }
    fresh
}

/// Whether an estimate rule acts: `holds` on this revision and the one before it, over a pace
/// measured on enough slices that one slow first step cannot stop a run.
fn projecting(count: &mut Count, facts: &Facts, holds: bool) -> bool {
    let rev = facts.snapshot.rev;
    if count.rev.is_some_and(|seen| seen >= rev) {
        return false;
    }
    count.rev = Some(rev);
    let measured = facts.slices >= SLICES_BEFORE_PROJECTING;
    count.held = if holds && measured { count.held + 1 } else { 0 };
    count.held >= REVISIONS_BEFORE_PROJECTING
}

/// The flag a rule that asks raises: its sentence names the rule and what orx saw.
pub(crate) fn flag(trip: &Trip, whole: Value, now: i64) -> Value {
    let plain = |text: &str| text.replace(['{', '}', '*'], "");
    let name = plain(&trip.rule.name);
    json!({
        "id": super::flags::new_id(),
        "source": "rule",
        "rule": trip.rule.name,
        "ruleText": trip.rule.text,
        "fingerprint": format!("rule:{}", trip.rule.name),
        "at": now,
        "whole": whole,
        "template": format!("Rule **{name}** asks: {}.", plain(&trip.evidence)),
        "detail": format!("Rule **{name}** asks: {}.", plain(&trip.evidence)),
        "text": format!("{}.", trip.evidence),
        "evidence": null,
    })
}

/// The reason a rule that stops a run gives.
pub(crate) fn stopped(trip: &Trip) -> String {
    format!(
        "Stopped by rule {}: {}. No model was involved.",
        trip.rule.name, trip.evidence
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(when: Value) -> Result<Rule, String> {
        parse(&json!([{"name": "r", "text": "Checks.", "does": "stop", "when": when}]))
            .map(|mut rules| rules.remove(0))
    }

    #[test]
    fn each_condition_reads_back_as_it_was_written() {
        for when in [
            json!({"nonFinite": "loss", "reports": 3}),
            json!({"nonFinite": "loss"}),
            json!({"compare": {"signal": "eval_score", "op": "lt", "value": 0.1}}),
            json!({"compare": {"signal": "acc", "op": "ge", "value": 0.9}, "reports": 2}),
            json!({"noImprovement": {"signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}}),
            json!({"worseThanParent": {"signal": "loss", "by": 1.0, "better": "lower"}, "reports": 3}),
            json!({"noProgress": {"seconds": 600}}),
            json!({"costAbove": {"usd": 5.0}}),
            json!({"projectedFinishPastLimit": true}),
        ] {
            let rule = one(when.clone()).unwrap();
            assert_eq!(rule.json()["when"], when, "{when}");
            assert_eq!(parse(&json!([rule.json()])).unwrap(), vec![rule]);
        }
    }

    #[test]
    fn each_mistake_is_refused_in_its_own_words() {
        let refused = |rules: Value| parse(&rules).unwrap_err();
        let when = |when: Value| one(when).unwrap_err();
        assert_eq!(refused(json!({})), "\"rules\" must be a list of rules");
        assert!(refused(json!(vec![json!({"name": "r"}); 33])).contains("at most 32"));
        let rule =
            json!({"name": "nan", "text": "t", "does": "stop", "when": {"nonFinite": "loss"}});
        assert_eq!(
            refused(json!([rule, rule])),
            "rules[1]: another rule is already named \"nan\""
        );
        assert!(
            refused(json!([{"name": "nan", "does": "stop", "when": {"nonFinite": "loss"}}]))
                .starts_with("rules[0] (nan): \"text\" must say")
        );
        assert!(
            refused(json!([{"name": "", "text": "t", "does": "stop", "when": {}}]))
                .contains("\"name\" must be text")
        );
        assert!(
            refused(json!([{"name": "n", "text": "t", "does": "halt", "when": {}}]))
                .contains("\"does\" must be \"stop\" or \"ask\"")
        );
        assert!(
            refused(json!([{"name": "n", "text": "t", "does": "stop", "when": {}, "if": 1}]))
                .contains("\"if\" is not part of a rule")
        );
        assert!(when(json!({"lossAbove": 3}))
            .contains("unknown condition \"lossAbove\"; this orx checks nonFinite"));
        assert!(when(json!({})).contains("must have one of"));
        assert!(
            when(json!({"nonFinite": "loss", "noProgress": {"seconds": 1}}))
                .contains("checks one condition")
        );
        assert!(when(json!({"nonFinite": "a b"})).contains("must name a signal"));
        assert!(when(json!({"nonFinite": "loss", "reports": 0})).contains("from 1 to 256"));
        assert!(when(json!({"nonFinite": "loss", "reports": 257})).contains("from 1 to 256"));
        assert!(
            when(json!({"compare": {"signal": "a", "op": "eq", "value": 1}}))
                .contains("\"op\" must be")
        );
        assert!(when(json!({"compare": {"signal": "a", "op": "lt"}})).contains("needs \"value\""));
        assert!(
            when(json!({"compare": {"signal": "a", "op": "lt", "value": "NaN"}}))
                .contains("finite number")
        );
        assert!(
            when(json!({"compare": {"signal": "a", "op": "lt", "value": 1, "x": 1}}))
                .contains("not \"x\"")
        );
        assert!(when(
            json!({"noImprovement": {"signal": "a", "min": 0, "reports": 3, "better": "up"}})
        )
        .contains("\"better\""));
        assert!(when(json!({"noImprovement": {"signal": "a", "min": 0, "reports": 3, "better": "lower"}, "reports": 2})).contains("takes no \"reports\""));
        assert!(when(json!({"noProgress": {"seconds": 0}})).contains("\"seconds\" must be"));
        assert!(when(json!({"costAbove": {"usd": 0}})).contains("greater than zero"));
        assert!(when(json!({"projectedFinishPastLimit": false})).contains("must be true"));
    }

    #[test]
    fn a_budget_is_a_rule_that_stops_the_run_and_replaces_a_declared_one() {
        assert_eq!(
            budget(f64::NAN).unwrap_err(),
            "--budget must be a finite USD amount greater than zero (got NaN)"
        );
        assert!(budget(0.0).unwrap_err().contains("(got 0)"));
        let settings = json!({"rules": [
            {"name": "nan", "text": "Stop at NaN.", "does": "stop", "when": {"nonFinite": "loss"}},
            {"name": "budget", "text": "Old.", "does": "ask", "when": {"costAbove": {"usd": 1}}},
        ]})
        .to_string();
        let frozen_rules = freeze("exp", Some(&settings), Some(5.0)).unwrap().unwrap();
        let rules = frozen("run", Some(&frozen_rules)).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1], budget(5.0).unwrap());
        assert_eq!(rules[1].shown()["budgetUsd"], 5.0);
        assert_eq!(freeze("exp", None, None).unwrap(), None);
        assert_eq!(frozen("run", None).unwrap(), vec![]);
    }

    #[test]
    fn a_rule_a_newer_orx_declared_stops_a_launch_with_what_to_do() {
        let settings = json!({"rules": [{"name": "r", "text": "t", "does": "stop", "when": {"lossAbove": 3}}]});
        let error = freeze("exp", Some(&settings.to_string()), None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("rules[0] (r): unknown condition \"lossAbove\""),
            "{error}"
        );
        assert!(
            error.contains("Update orx before launching this rule"),
            "{error}"
        );
    }
}
