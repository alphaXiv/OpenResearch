//! Stop rules: conditions an agent declares on an experiment, frozen on each run at launch and
//! checked by `orx up` on each report the run makes, with no model involved.

use orx_signals::{MetricReport, MAX_RULE_REPORTS};
use serde::Deserialize;
use serde_json::{json, Map, Value};

const MAX_RULES: usize = 32;
const MAX_NAME: usize = 64;
const MAX_TEXT: usize = 280;
const MAX_SECONDS: u64 = 30 * 24 * 3600;
const KINDS: &str =
    "nonFinite, compare, noImprovement, worseThanParent, noProgress, costAbove, projectedFinishPastLimit";
/// The rule `orx exp run --budget` adds.
const BUDGET: &str = "budget";

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
    /// What the run has cost reached `usd`, or what it would cost by its end passed it.
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
            When::NonFinite { signal, reports } => json!({
                "kind": "nonFinite", "signal": signal, "reports": reports,
            }),
            When::Compare {
                signal,
                op,
                value,
                reports,
            } => json!({
                "kind": "compare",
                "signal": signal,
                "op": op_name(*op),
                "value": value,
                "reports": reports,
            }),
            When::NoImprovement {
                signal,
                min,
                reports,
                better,
            } => json!({
                "kind": "noImprovement",
                "signal": signal, "min": min, "reports": reports, "better": better_name(*better),
            }),
            When::WorseThanParent {
                signal,
                by,
                better,
                reports,
            } => json!({
                "kind": "worseThanParent",
                "signal": signal,
                "by": by,
                "better": better_name(*better),
                "reports": reports,
            }),
            When::NoProgress { seconds } => json!({"kind": "noProgress", "seconds": seconds}),
            When::CostAbove { usd } => json!({"kind": "costAbove", "usd": usd}),
            When::FinishPastLimit => json!({"kind": "projectedFinishPastLimit"}),
        };
        if let Value::Object(when) = &mut when {
            when.retain(|key, value| {
                key != "reports"
                    || value.as_u64() != Some(1)
                    || matches!(self.when, When::NoImprovement { .. })
            });
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
    parse_rules(value, false)
}

fn parse_rules(value: &Value, allow_generated_budget: bool) -> Result<Vec<Rule>, String> {
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
        let rule = parse_rule(rule, allow_generated_budget).map_err(|why| {
            match rule["name"].as_str() {
                Some(name) => format!("rules[{index}] ({name}): {why}"),
                None => format!("rules[{index}]: {why}"),
            }
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

pub(crate) fn parse_rule(value: &Value, allow_generated_budget: bool) -> Result<Rule, String> {
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
    let does = match rule.get("does").and_then(Value::as_str) {
        Some("stop") => Does::Stop,
        Some("ask") => Does::Ask,
        _ => return Err("\"does\" must be \"stop\" or \"ask\"".into()),
    };
    let when = rule
        .get("when")
        .and_then(Value::as_object)
        .ok_or("\"when\" must be an object with a \"kind\" field")?;
    let when = parse_when(when)?;
    let generated_budget = allow_generated_budget
        && name == BUDGET
        && does == Does::Stop
        && matches!(when, When::CostAbove { .. });
    let text = match rule.get("text") {
        None | Some(Value::Null) if generated_budget => None,
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
    Ok(Rule {
        name: name.to_owned(),
        text,
        does,
        when,
    })
}

fn parse_when(when: &Map<String, Value>) -> Result<When, String> {
    let kind = when
        .get("kind")
        .ok_or("\"when\" needs a \"kind\" field")?
        .as_str()
        .ok_or("\"when.kind\" must be text")?;

    let field = |name: &str| {
        when.get(name)
            .ok_or_else(|| format!("\"{kind}\" needs \"{name}\""))
    };
    let fields = |allowed: &[&str]| {
        let unexpected = when
            .keys()
            .find(|key| key.as_str() != "kind" && !allowed.contains(&key.as_str()));
        match unexpected {
            Some(key) => Err(format!("\"{kind}\" has an unexpected field \"{key}\"")),
            None => Ok(()),
        }
    };
    let streak = || {
        when.get("reports")
            .map_or(Ok(1), |value| count(value, "reports"))
    };
    let signal = || {
        field("signal")?
            .as_str()
            .filter(|name| orx_signals::identifier(name))
            .map(str::to_owned)
            .ok_or_else(|| format!("\"{kind}\" must name a signal the run reports"))
    };
    let finite = |name: &str| {
        field(name)?
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("\"{name}\" must be a finite number"))
    };
    let better = || match field("better")?.as_str() {
        Some("lower") => Ok(Better::Lower),
        Some("higher") => Ok(Better::Higher),
        _ => Err("\"better\" must be \"lower\" or \"higher\"".to_owned()),
    };
    Ok(match kind {
        "nonFinite" => {
            fields(&["signal", "reports"])?;
            When::NonFinite {
                signal: signal()?,
                reports: streak()?,
            }
        }
        "compare" => {
            fields(&["signal", "op", "value", "reports"])?;
            When::Compare {
                signal: signal()?,
                op: match field("op")?.as_str() {
                    Some("lt") => Op::Lt,
                    Some("le") => Op::Le,
                    Some("gt") => Op::Gt,
                    Some("ge") => Op::Ge,
                    _ => return Err("\"op\" must be \"lt\", \"le\", \"gt\" or \"ge\"".into()),
                },
                value: finite("value")?,
                reports: streak()?,
            }
        }
        "noImprovement" => {
            fields(&["signal", "min", "reports", "better"])?;
            When::NoImprovement {
                signal: signal()?,
                min: finite("min")?.max(0.0),
                reports: count(field("reports")?, "reports")?,
                better: better()?,
            }
        }
        "worseThanParent" => {
            fields(&["signal", "by", "better", "reports"])?;
            When::WorseThanParent {
                signal: signal()?,
                by: finite("by")?.max(0.0),
                better: better()?,
                reports: streak()?,
            }
        }
        "noProgress" => {
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
            fields(&["usd"])?;
            let usd = finite("usd")?;
            if usd <= 0.0 {
                return Err("\"usd\" must be greater than zero".into());
            }
            When::CostAbove { usd }
        }
        "projectedFinishPastLimit" => {
            fields(&[])?;
            When::FinishPastLimit
        }
        other => return Err(format!("unknown condition \"{other}\". Use one of {KINDS}")),
    })
}

fn count(value: &Value, name: &str) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|count| (1..=MAX_RULE_REPORTS).contains(count))
        .ok_or(format!(
            "\"{name}\" must be a whole number from 1 to {MAX_RULE_REPORTS}"
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
    has_time_limit: bool,
) -> crate::error::Result<Option<String>> {
    let mut rules = stored(experiment, settings)?;
    if let Some(usd) = budget_usd {
        rules.retain(|rule| rule.name != BUDGET);
        let budget = budget(usd).map_err(|why| crate::error::anyhow!("{why}"))?;
        if rules.len() == MAX_RULES {
            crate::error::bail!(
                "Experiment {experiment} already has {MAX_RULES} rules. Remove a rule with `orx exp view set {experiment} --config PATH` or launch without `--budget`."
            );
        }
        rules.push(budget);
    }
    if let Some(rule) = rules
        .iter()
        .find(|rule| !has_time_limit && rule.when == When::FinishPastLimit)
    {
        crate::error::bail!(
            "Rule \"{}\" requires a time limit and this run has none. Run on a backend with a time limit or remove the rule with `orx exp view set {experiment} --config PATH`.",
            rule.name
        );
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
    parse_rules(&config["rules"], true)
        .map_err(|why| crate::error::anyhow!("Run {run}'s rules: {why}. Update orx to check them."))
}

/// What a rule has counted on one run, kept in the run's monitor state so a restart neither
/// counts a report twice nor acts twice.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Count {
    /// How many reports of this signal have been read in the epoch.
    #[serde(default)]
    after: u64,
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
    window: Vec<(Option<f64>, Option<f64>)>,
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
pub(crate) fn check(rules: &[Rule], state: Option<&Value>, facts: &Facts) -> Option<Checked> {
    let mut state = match state {
        Some(state) => state.as_object()?.clone(),
        None => Map::new(),
    };
    let mut kept = match state.remove("counts") {
        Some(Value::Object(counts)) => counts,
        Some(_) => return None,
        None => Map::new(),
    };
    let basis = json!([facts.snapshot.generation, facts.snapshot.report_epoch]);
    let reset = state.get("basis") != Some(&basis);
    let mut trips = Vec::new();
    let mut changed = reset;
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
        let mut count = if reset {
            Count {
                extra: was.extra.clone(),
                ..Count::default()
            }
        } else {
            was.clone()
        };
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
    changed.then(|| {
        state.insert("basis".into(), basis);
        state.insert("counts".into(), Value::Object(kept));
        Checked {
            counts: Value::Object(state),
            trips,
        }
    })
}

/// Counts what is new for `rule`; what orx saw when its condition holds now.
fn step(rule: &Rule, count: &mut Count, facts: &Facts) -> Option<String> {
    let body = facts.body;
    let at = |x: f64| match body["x"]["name"].as_str() {
        Some("step") => format!("step {}", super::monitor::number(x)),
        _ => format!("{} in", super::monitor::short((x * 1000.0) as i64)),
    };
    let span = |count: &Count, last: Option<f64>| {
        if count.held > 1 {
            let streak = format!("on {} reports in a row", count.held);
            return match count.since.zip(last) {
                Some((first, last)) => {
                    format!(
                        "{streak}, {} to {}",
                        at(first),
                        at(last).trim_start_matches("step ")
                    )
                }
                None => streak,
            };
        }
        last.map_or_else(
            || "on its latest report".into(),
            |last| format!("at {}", at(last)),
        )
    };
    let value = |value: f64| super::monitor::measure(value, &Value::Null);
    match &rule.when {
        When::NonFinite { signal, reports } => {
            let (last, fresh) = fresh(facts, signal, count)?;
            for report in fresh {
                held(count, report.x, report.non_finite);
            }
            let word = facts.snapshot.latest[signal.as_str()]
                .as_str()
                .unwrap_or("not a finite number");
            (count.held >= *reports).then(|| format!("{signal} was {word} {}", span(count, last.x)))
        }
        When::Compare {
            signal,
            op,
            value: bound,
            reports,
        } => {
            let (last, fresh) = fresh(facts, signal, count)?;
            for report in fresh {
                let holds = report.value.is_some_and(|y| match op {
                    Op::Lt => y < *bound,
                    Op::Le => y <= *bound,
                    Op::Gt => y > *bound,
                    Op::Ge => y >= *bound,
                });
                held(count, report.x, holds);
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
                    last.value.map_or("not a number".into(), value),
                    value(*bound),
                    span(count, last.x)
                )
            })
        }
        When::WorseThanParent {
            signal,
            by,
            better,
            reports,
        } => {
            let theirs = facts
                .parent
                .filter(|parent| super::monitor::same_stage(facts.snapshot, parent, signal))
                .and_then(|parent| parent.series.get(signal));
            let (last, fresh) = fresh(facts, signal, count)?;
            let mut seen = None;
            for report in fresh {
                // Where the parent has no value at this step, the report counts neither way.
                let paired = report
                    .x
                    .zip(theirs)
                    .and_then(|(x, theirs)| report.value.zip(super::monitor::parent_at(theirs, x)));
                let Some((mine, parent)) = paired else {
                    seen = None;
                    continue;
                };
                let worse = match better {
                    Better::Lower => mine - parent,
                    Better::Higher => parent - mine,
                };
                held(count, report.x, worse > *by);
                seen = Some((mine, parent, worse));
            }
            let (mine, parent, worse) = seen?;
            (count.held >= *reports).then(|| {
                format!(
                    "{signal} was {} worse than the parent's at the same step ({} against {}) {}",
                    value(worse),
                    value(mine),
                    value(parent),
                    span(count, last.x)
                )
            })
        }
        When::NoImprovement {
            signal,
            min,
            reports,
            better,
        } => {
            let (last, fresh) = fresh(facts, signal, count)?;
            let best = |values: &mut dyn Iterator<Item = f64>| match better {
                Better::Lower => values.filter(|v| v.is_finite()).min_by(f64::total_cmp),
                Better::Higher => values.filter(|v| v.is_finite()).max_by(f64::total_cmp),
            };
            for report in fresh {
                count.window.push((report.x, report.value));
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
            let range = count.window[0]
                .0
                .zip(last.x)
                .map_or_else(String::new, |(first, last)| {
                    format!(
                        ", {} to {}",
                        at(first),
                        at(last).trim_start_matches("step ")
                    )
                });
            (gained < *min).then(|| {
                format!(
                    "{signal} improved by {} over its last {reports} reports{range}, on its best of {} before them; the rule needs {}",
                    value(gained.max(0.0)),
                    value(before),
                    value(*min)
                )
            })
        }
        When::NoProgress { seconds } => {
            let progress = &body["progress"];
            let last_at = progress["lastAt"].as_i64();
            let silent = facts.now - last_at.unwrap_or(facts.started_at);
            (silent >= *seconds as i64 * 1000).then(|| {
                let from = match last_at.and(progress["done"].as_f64()) {
                    Some(done) => format!("since {}", at(done)),
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
                    "it has cost ${spent:.2} and reached its ${usd:.2} budget"
                ));
            }
            let projected = cost["projected"].as_f64();
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
fn held(count: &mut Count, x: Option<f64>, holds: bool) {
    if !holds {
        count.held = 0;
        count.since = None;
        return;
    }
    if count.held == 0 {
        count.since = x;
    }
    count.held += 1;
}

fn fresh<'a>(
    facts: &Facts<'a>,
    signal: &str,
    count: &mut Count,
) -> Option<(&'a MetricReport, impl Iterator<Item = &'a MetricReport>)> {
    let reports = facts.snapshot.metric_reports.get(signal)?;
    if count.after >= reports.seen {
        return None;
    }
    let first = reports.seen.checked_sub(reports.tail.len() as u64)?;
    let skip = count.after.saturating_sub(first) as usize;
    if count.after < first {
        // The missing reports could have broken the streak or changed the plateau baseline.
        count.held = 0;
        count.since = None;
        count.window.clear();
        count.best = None;
    }
    count.after = reports.seen;
    Some((reports.tail.back()?, reports.tail.iter().skip(skip)))
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
            json!({"kind": "nonFinite", "signal": "loss", "reports": 3}),
            json!({"kind": "compare", "signal": "acc", "op": "ge", "value": 0.9, "reports": 2}),
            json!({"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}),
            json!({"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 1, "better": "lower"}),
            json!({"kind": "worseThanParent", "signal": "loss", "by": 1.0, "better": "lower", "reports": 3}),
            json!({"kind": "noProgress", "seconds": 600}),
            json!({"kind": "costAbove", "usd": 5.0}),
            json!({"kind": "projectedFinishPastLimit"}),
        ] {
            let rule = one(when.clone()).unwrap();
            assert_eq!(rule.json()["when"], when, "{when}");
            assert_eq!(parse(&json!([rule.json()])).unwrap(), vec![rule]);
        }
    }

    #[test]
    fn streak_rules_write_one_report_as_the_default() {
        for when in [
            json!({"kind": "nonFinite", "signal": "loss"}),
            json!({"kind": "compare", "signal": "eval_score", "op": "lt", "value": 0.1}),
            json!({"kind": "worseThanParent", "signal": "loss", "by": 1.0, "better": "lower"}),
        ] {
            let mut explicit = when.clone();
            explicit["reports"] = json!(1);
            let rule = one(when.clone()).unwrap();
            assert_eq!(one(explicit).unwrap(), rule);
            assert_eq!(rule.json()["when"], when);
        }
    }

    #[test]
    fn each_mistake_is_refused_in_its_own_words() {
        let refused = |rules: Value| parse(&rules).unwrap_err();
        let when = |when: Value| one(when).unwrap_err();
        assert_eq!(refused(json!({})), "\"rules\" must be a list of rules");
        assert!(refused(json!(vec![json!({"name": "r"}); 33])).contains("at most 32"));
        let rule = json!({"name": "nan", "text": "t", "does": "stop", "when": {"kind": "nonFinite", "signal": "loss"}});
        assert_eq!(
            refused(json!([rule, rule])),
            "rules[1]: another rule is already named \"nan\""
        );
        assert!(
            refused(json!([{"name": "nan", "does": "stop", "when": {"kind": "nonFinite", "signal": "loss"}}]))
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
        assert!(when(json!({"kind": "lossAbove", "value": 3}))
            .contains("unknown condition \"lossAbove\""));
        assert!(when(json!({"nonFinite": "loss"})).contains("needs a \"kind\" field"));
        assert!(when(json!({"kind": true})).contains("\"when.kind\" must be text"));
        assert!(
            when(json!({"kind": "nonFinite", "signal": "loss", "noProgress": {"seconds": 1}}))
                .contains("unexpected field \"noProgress\"")
        );
        assert!(when(json!({"kind": "nonFinite", "signal": "a b"})).contains("must name a signal"));
        assert!(
            when(json!({"kind": "nonFinite", "signal": "loss", "reports": 0}))
                .contains("from 1 to 256")
        );
        assert!(
            when(json!({"kind": "nonFinite", "signal": "loss", "reports": 257}))
                .contains("from 1 to 256")
        );
        assert!(
            when(json!({"kind": "compare", "signal": "a", "op": "eq", "value": 1}))
                .contains("\"op\" must be")
        );
        assert!(
            when(json!({"kind": "compare", "signal": "a", "op": "lt"})).contains("needs \"value\"")
        );
        assert!(
            when(json!({"kind": "compare", "signal": "a", "op": "lt", "value": "NaN"}))
                .contains("finite number")
        );
        assert!(
            when(json!({"kind": "compare", "signal": "a", "op": "lt", "value": 1, "x": 1}))
                .contains("unexpected field \"x\"")
        );
        assert!(when(
            json!({"kind": "noImprovement", "signal": "a", "min": 0, "reports": 3, "better": "up"})
        )
        .contains("\"better\""));
        assert!(
            when(json!({"kind": "noImprovement", "signal": "a", "min": 0, "better": "lower"}))
                .contains("needs \"reports\"")
        );
        assert!(when(json!({"kind": "noProgress", "seconds": 0})).contains("\"seconds\" must be"));
        assert!(when(json!({"kind": "costAbove", "usd": 0})).contains("greater than zero"));
        for mut when_with_reports in [
            json!({"kind": "noProgress", "seconds": 600}),
            json!({"kind": "costAbove", "usd": 5}),
            json!({"kind": "projectedFinishPastLimit"}),
        ] {
            when_with_reports["reports"] = json!(1);
            assert!(when(when_with_reports).contains("unexpected field \"reports\""));
        }
    }

    #[test]
    fn a_declared_budget_rule_requires_text() {
        let rule =
            json!({"name": "budget", "does": "stop", "when": {"kind": "costAbove", "usd": 5}});
        let missing = parse(&json!([rule])).unwrap_err();
        assert!(missing.contains("\"text\""), "{missing}");

        let mut rule = rule;
        rule["text"] = Value::Null;
        let null = parse(&json!([rule])).unwrap_err();
        assert!(null.contains("\"text\""), "{null}");
    }

    #[test]
    fn a_budget_is_a_rule_that_stops_the_run_and_replaces_a_declared_one() {
        assert_eq!(
            budget(f64::NAN).unwrap_err(),
            "--budget must be a finite USD amount greater than zero (got NaN)"
        );
        assert!(budget(0.0).unwrap_err().contains("(got 0)"));
        let settings = json!({"rules": [
            {"name": "nan", "text": "Stop at NaN.", "does": "stop", "when": {"kind": "nonFinite", "signal": "loss"}},
            {"name": "budget", "text": "Old.", "does": "ask", "when": {"kind": "costAbove", "usd": 1}},
        ]})
        .to_string();
        let frozen_rules = freeze("exp", Some(&settings), Some(5.0), false)
            .unwrap()
            .unwrap();
        let rules = frozen("run", Some(&frozen_rules)).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1], budget(5.0).unwrap());
        assert_eq!(rules[1].shown()["budgetUsd"], 5.0);
        assert_eq!(freeze("exp", None, None, false).unwrap(), None);
        assert_eq!(frozen("run", None).unwrap(), vec![]);
    }

    #[test]
    fn a_budget_cannot_push_a_launch_over_the_rule_limit() {
        let mut rules: Vec<_> = (0..MAX_RULES)
            .map(|i| {
                json!({"name": format!("rule-{i}"), "text": "Check loss.", "does": "stop",
                "when": {"kind": "nonFinite", "signal": "loss"}})
            })
            .collect();
        let settings = json!({"rules": rules}).to_string();
        assert!(freeze("exp", Some(&settings), None, false).is_ok());
        assert!(freeze("exp", Some(&settings), Some(5.0), false)
            .unwrap_err()
            .to_string()
            .contains("--budget"));

        rules[0]["name"] = json!("budget");
        let settings = json!({"rules": rules}).to_string();
        let frozen_rules = freeze("exp", Some(&settings), Some(5.0), false).unwrap();
        assert_eq!(
            frozen("run", frozen_rules.as_deref()).unwrap().len(),
            MAX_RULES
        );
    }

    #[test]
    fn a_rule_a_newer_orx_declared_stops_a_launch_with_what_to_do() {
        let settings = json!({"rules": [{"name": "r", "text": "t", "does": "stop", "when": {"kind": "lossAbove", "value": 3}}]});
        let error = freeze("exp", Some(&settings.to_string()), None, false)
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

    #[test]
    fn a_time_limit_rule_is_frozen_when_the_run_has_a_limit() {
        let settings = json!({"rules": [{
            "name": "deadline", "text": "This would run too long.", "does": "stop",
            "when": {"kind": "projectedFinishPastLimit"}
        }]});
        let config = freeze("exp", Some(&settings.to_string()), None, true).unwrap();
        let rules = frozen("run", config.as_deref()).unwrap();
        assert!(matches!(&rules[..], [rule] if rule.when == When::FinishPastLimit));
    }
}
