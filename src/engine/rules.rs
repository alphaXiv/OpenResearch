//! Stop rules: conditions an agent declares on an experiment, frozen on each run at launch and
//! checked by `orx up` on each report the run makes, with no model involved.

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
