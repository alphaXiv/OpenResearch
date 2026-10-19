//! Self-improvement: the agent revises the way it works, and orx keeps a
//! revision only when real experiments show it does better.
//!
//! The way of working is a versioned *strategy* per project: the research
//! approach, how research rounds use subagents and sources, how compute is
//! routed, and how the retrospective that writes the next strategy is run
//! (so the improver improves itself too). The loop, while sleep mode runs:
//!
//! 1. **Measure.** Every run an agent launches is attributed to the version
//!    its turn ran under. A finished run is a *win* when its objective value
//!    beats the best comparable result that existed when it was launched, a
//!    *loss* when it does not, fails, is cancelled, or never reports the
//!    metric. orx judges this from imported run metrics, never from the
//!    agent's own account.
//! 2. **Reflect.** After [`Knobs::reflect_every_runs`] judged runs, the agent
//!    is asked for a retrospective and may propose a candidate with
//!    `orx strategy propose`, or keep the current one with `orx strategy keep`.
//! 3. **Trial.** The candidate does not replace anything. Turns alternate
//!    between it and the incumbent, so both arms face the same, ever harder,
//!    experiment tree, and the comparison is not skewed by the tree maturing.
//! 4. **Decide.** The watchdog compares the arms' smoothed win rates. A clear
//!    gain promotes the candidate; a clear loss, or no gain within
//!    [`MAX_TRIAL_RUNS`], rejects it, which leaves the incumbent untouched (the
//!    automatic rollback). The user can promote, reject, or roll back to any
//!    earlier version at any time.
//!
//! Out of the strategy's reach by construction: the GPU budget, the deadline
//! and target, the watchdog's timings, permissions and autonomy. Those live in
//! the sleep plan and the chat settings, which agents cannot loosen, and the
//! strategy's schema rejects any field it does not know. Every proposal and
//! decision is written to the change log (`orx strategy log`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{anyhow, Result};
use crate::local::sleep::{self, Objective, ProjectEvidence, RunEvidence};
use crate::store::sleep::StoredSleep;
use crate::store::strategy::{StoredStrategy, StrategyArm, StrategyEvent, StrategyRun};
use crate::store::Store;

/// Judged runs a candidate needs before it can win or lose.
pub const MIN_TRIAL_RUNS: usize = 4;
/// Judged incumbent runs during the trial before arms are compared.
pub const MIN_CONTROL_RUNS: usize = 2;
/// A trial that has not shown a clear gain by now is decided on what it has.
pub const MAX_TRIAL_RUNS: usize = 10;
/// Standard errors by which the candidate must lead to be promoted early.
pub const PROMOTE_Z: f64 = 1.0;
/// Standard errors by which it may trail before it is rejected early.
pub const REJECT_Z: f64 = 1.0;
/// The lead that still promotes a candidate at the end of a full trial.
pub const FINAL_Z: f64 = 0.5;
/// A finished run that has not reported the metric by now counts as a loss.
pub const METRIC_GRACE_MS: i64 = 60 * 60 * 1000;
/// Each text section of a strategy is capped so it stays a set of rules.
pub const MAX_SECTION_CHARS: usize = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    /// The version in force.
    Incumbent,
    /// On trial against the incumbent.
    Candidate,
    /// Was in force, replaced by a better one or rolled back.
    Retired,
    /// Lost its trial.
    Rejected,
    /// Taken back before a decision.
    Withdrawn,
    /// Sleep mode ended before its trial decided.
    Inconclusive,
}

impl VersionStatus {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "incumbent" => Self::Incumbent,
            "candidate" => Self::Candidate,
            "retired" => Self::Retired,
            "rejected" => Self::Rejected,
            "withdrawn" => Self::Withdrawn,
            "inconclusive" => Self::Inconclusive,
            _ => return None,
        })
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Incumbent => "incumbent",
            Self::Candidate => "candidate",
            Self::Retired => "retired",
            Self::Rejected => "rejected",
            Self::Withdrawn => "withdrawn",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// The numeric parts of the way of working.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Knobs {
    /// Finished runs without a new best before a research round; overrides
    /// the sleep plan's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_runs: Option<u32>,
    /// Judged runs between retrospectives.
    #[serde(default = "default_reflect_every_runs")]
    pub reflect_every_runs: u32,
    /// Share of each round's runs that depart from the current approach.
    #[serde(default = "default_explore_share")]
    pub explore_share: f64,
    /// Research subagents per research round.
    #[serde(default = "default_subagents")]
    pub subagents: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
    /// Compute backends to try first, in order. The budget still applies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_backends: Vec<String>,
}

fn default_reflect_every_runs() -> u32 {
    6
}
fn default_explore_share() -> f64 {
    0.25
}
fn default_subagents() -> u32 {
    3
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            stall_runs: None,
            reflect_every_runs: default_reflect_every_runs(),
            explore_share: default_explore_share(),
            subagents: default_subagents(),
            subagent_harness: None,
            subagent_model: None,
            prefer_backends: Vec::new(),
        }
    }
}

/// How the agent works in sleep mode. Hard limits are deliberately absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Strategy {
    /// How ideas are chosen, explored, and run.
    pub approach: String,
    /// How a research round works when results stall.
    pub research: String,
    /// How the retrospective that proposes the next strategy is run.
    pub retrospective: String,
    #[serde(default)]
    pub knobs: Knobs,
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            approach: "\
Explore as well as exploit: in every round of experiments, include ideas that depart from the \
obvious next step (a different mechanism, an idea borrowed from another field, an assumption of \
the baseline removed). Surprising results are the goal; record negative ones too.
Prefer free hardware and the cheapest GPU that fits, check `orx compute prices`, and keep spend \
proportional to the expected information gain. Size a run before scaling it: a short smoke run \
catches crashes and out-of-memory errors cheaply."
                .into(),
            research: "\
Run a research round before more compute: use your harness's native subagents on its cheapest, \
fastest model (or `orx agent spawn --harness <h> --model <cheap model>` with an explicit ban on \
compute launches; check `orx agent quota` first). Give each a distinct source: papers via \
`orx discover`/`orx paper` (alphaXiv, arXiv, Semantic Scholar, OpenAlex, bioRxiv, PubMed), code \
and issues on GitHub, model and dataset cards on Hugging Face, benchmark leaderboards, and the \
web. Ask each for concrete, citable techniques with expected gains; synthesize them into ranked \
hypotheses yourself."
                .into(),
            retrospective: "\
Judge the strategy by the evidence in `orx strategy stats --runs`, not by how the night felt: \
which kinds of ideas produced new bests, which runs were lost (crashes, out of memory, \
mis-sized GPUs, duplicated ideas, metrics never imported), what each win cost, and where turns \
stalled. Change only what the evidence points at, one or two things at a time, so the trial can \
tell what helped, and keep what works. The rationale states the evidence and the change it \
motivates. This section is part of the strategy too: improve how you reflect when it misses \
things."
                .into(),
            knobs: Knobs::default(),
        }
    }
}

impl Strategy {
    pub fn validated(self) -> Result<Self> {
        for (name, text) in [
            ("approach", &self.approach),
            ("research", &self.research),
            ("retrospective", &self.retrospective),
        ] {
            if text.trim().is_empty() {
                return Err(anyhow!("The strategy's `{name}` section is empty."));
            }
            if text.chars().count() > MAX_SECTION_CHARS {
                return Err(anyhow!(
                    "The strategy's `{name}` section is over {MAX_SECTION_CHARS} characters; \
                     keep it to the rules that change behavior."
                ));
            }
        }
        let k = &self.knobs;
        if k.stall_runs.is_some_and(|n| !(1..=10).contains(&n)) {
            return Err(anyhow!("knobs.stallRuns is between 1 and 10."));
        }
        if !(3..=30).contains(&k.reflect_every_runs) {
            return Err(anyhow!("knobs.reflectEveryRuns is between 3 and 30."));
        }
        if !k.explore_share.is_finite() || !(0.0..=1.0).contains(&k.explore_share) {
            return Err(anyhow!("knobs.exploreShare is between 0 and 1."));
        }
        if !(1..=8).contains(&k.subagents) {
            return Err(anyhow!("knobs.subagents is between 1 and 8."));
        }
        if k.prefer_backends.len() > 8 {
            return Err(anyhow!("knobs.preferBackends lists at most 8 backends."));
        }
        Ok(self)
    }

    /// The strategy as the agent reads it at the start of a turn.
    pub fn render(&self) -> String {
        let k = &self.knobs;
        let mut knobs = vec![format!(
            "- About {:.0}% of each round's runs depart from the current approach.",
            k.explore_share * 100.0
        )];
        let mut research = format!("- Research rounds use {} subagent(s)", k.subagents);
        if let Some(harness) = &k.subagent_harness {
            research.push_str(&format!(" on `{harness}`"));
        }
        if let Some(model) = &k.subagent_model {
            research.push_str(&format!(" with model `{model}`"));
        }
        research.push('.');
        knobs.push(research);
        if !k.prefer_backends.is_empty() {
            knobs.push(format!(
                "- Try these compute backends first, in order: {}.",
                k.prefer_backends.join(", ")
            ));
        }
        if let Some(n) = k.stall_runs {
            knobs.push(format!(
                "- A research round starts after {n} finished run(s) without a new best."
            ));
        }
        format!(
            "Approach:\n{}\n\nWhen results stall:\n{}\n\nSettings:\n{}",
            self.approach.trim(),
            self.research.trim(),
            knobs.join("\n")
        )
    }
}

/// Applies a partial JSON change to `base`, one level deep and into `knobs`,
/// and validates the result. A full strategy works as well.
pub fn merged(base: &Strategy, change: Value) -> Result<Strategy> {
    let Value::Object(change) = change else {
        return Err(anyhow!("A strategy change is a JSON object."));
    };
    let mut value = serde_json::to_value(base)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("strategy is not an object"))?;
    for (key, field) in change {
        match (key.as_str(), field, object.get_mut("knobs")) {
            ("knobs", Value::Object(knobs), Some(Value::Object(current))) => {
                for (knob, v) in knobs {
                    current.insert(knob, v);
                }
            }
            (_, field, _) => {
                object.insert(key, field);
            }
        }
    }
    let strategy: Strategy = serde_json::from_value(value).map_err(|err| {
        anyhow!(
            "Not a valid strategy change: {err}. The strategy holds `approach`, `research`, \
             `retrospective`, and `knobs` only; budgets, deadlines, targets and permissions \
             are not part of it."
        )
    })?;
    strategy.validated()
}

// --- measurement --------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Beat the best comparable result that existed when it was launched.
    Win,
    /// Finished without doing so: no gain, failed, cancelled, or no metric.
    Loss,
    /// Still running, or finished and waiting for its metrics.
    Pending,
    /// Nothing to compare against: no baseline and no earlier result.
    Unjudged,
}

impl Outcome {
    pub fn id(self) -> &'static str {
        match self {
            Self::Win => "win",
            Self::Loss => "loss",
            Self::Pending => "pending",
            Self::Unjudged => "unjudged",
        }
    }
}

/// Judges every run in `evidence` against the objective.
pub fn outcomes(
    objective: &Objective,
    evidence: &[RunEvidence<'_>],
    now: i64,
) -> HashMap<String, Outcome> {
    let (baseline, scope) = sleep::baseline_and_scope(objective, evidence);
    let results: Vec<(i64, f64)> = evidence
        .iter()
        .filter_map(|e| {
            let value = sleep::comparable_value(objective, scope.as_ref(), e)?;
            Some((e.run.ended_at.unwrap_or(e.run.updated_at), value))
        })
        .collect();
    let direction = objective.direction;
    evidence
        .iter()
        .map(|e| {
            let run = e.run;
            let outcome = match run.status.as_str() {
                "failed" | "cancelled" => Outcome::Loss,
                "done" => match sleep::comparable_value(objective, scope.as_ref(), e) {
                    Some(value) => {
                        let prior = results
                            .iter()
                            .filter(|(ended, _)| *ended < run.created_at)
                            .map(|(_, v)| *v)
                            .chain(baseline.as_ref().map(|b| b.value))
                            .reduce(|a, b| if direction.better(b, a) { b } else { a });
                        match prior {
                            Some(prior) if direction.better(value, prior) => Outcome::Win,
                            Some(_) => Outcome::Loss,
                            None => Outcome::Unjudged,
                        }
                    }
                    None if now - run.ended_at.unwrap_or(run.updated_at) > METRIC_GRACE_MS => {
                        Outcome::Loss
                    }
                    None => Outcome::Pending,
                },
                _ => Outcome::Pending,
            };
            (run.id.clone(), outcome)
        })
        .collect()
}

/// One version's record over a set of its runs.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmStats {
    pub launched: usize,
    pub wins: usize,
    pub losses: usize,
    pub pending: usize,
    pub spend_usd: f64,
}

impl ArmStats {
    pub fn judged(&self) -> usize {
        self.wins + self.losses
    }

    /// The win rate with one imagined win and one imagined loss, so a few
    /// lucky runs do not read as certainty.
    pub fn smoothed_rate(&self) -> f64 {
        (self.wins as f64 + 1.0) / (self.judged() as f64 + 2.0)
    }

    fn variance(&self) -> f64 {
        let p = self.smoothed_rate();
        p * (1.0 - p) / (self.judged() as f64 + 2.0)
    }

    fn add(&mut self, outcome: Option<Outcome>, spend: f64) {
        self.launched += 1;
        self.spend_usd += spend;
        match outcome {
            Some(Outcome::Win) => self.wins += 1,
            Some(Outcome::Loss) => self.losses += 1,
            Some(Outcome::Pending) | None => self.pending += 1,
            Some(Outcome::Unjudged) => {}
        }
    }
}

/// How many standard errors the candidate's win rate leads the control's.
pub fn lead(candidate: &ArmStats, control: &ArmStats) -> f64 {
    let spread = (candidate.variance() + control.variance()).sqrt();
    (candidate.smoothed_rate() - control.smoothed_rate()) / spread.max(1e-9)
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrialDecision {
    Continue,
    Promote(String),
    Reject(String),
    Inconclusive(String),
}

fn record(stats: &ArmStats) -> String {
    format!(
        "{}/{} wins, ${:.2}",
        stats.wins,
        stats.judged(),
        stats.spend_usd
    )
}

/// Decides a trial. `control` is the incumbent during the trial; `history`
/// its whole record, used only when the trial filled up without enough
/// control runs. `ended`: sleep mode is over and no run is in flight.
pub fn evaluate_trial(
    candidate: &ArmStats,
    control: &ArmStats,
    history: &ArmStats,
    ended: bool,
) -> TrialDecision {
    let full = candidate.judged() >= MAX_TRIAL_RUNS;
    let control = if control.judged() < MIN_CONTROL_RUNS && full {
        history
    } else {
        control
    };
    let ready = candidate.judged() >= MIN_TRIAL_RUNS && control.judged() >= MIN_CONTROL_RUNS;
    if !ready {
        return if ended {
            TrialDecision::Inconclusive(format!(
                "sleep mode ended with too few runs to judge (candidate {}, incumbent {})",
                record(candidate),
                record(control)
            ))
        } else {
            TrialDecision::Continue
        };
    }
    let z = lead(candidate, control);
    let summary = format!(
        "candidate {} vs incumbent {}, lead {z:+.2} SE",
        record(candidate),
        record(control)
    );
    if z >= PROMOTE_Z && candidate.wins > 0 {
        return TrialDecision::Promote(summary);
    }
    if z <= -REJECT_Z {
        return TrialDecision::Reject(summary);
    }
    if full {
        return if z >= FINAL_Z && candidate.wins > 0 {
            TrialDecision::Promote(summary)
        } else {
            TrialDecision::Reject(format!("no clear gain after a full trial: {summary}"))
        };
    }
    if ended {
        return TrialDecision::Inconclusive(format!("sleep mode ended mid-trial: {summary}"));
    }
    TrialDecision::Continue
}

// --- versions in the store ----------------------------------------------------

/// The project's versions, with the built-in default as v0 the first time.
pub fn versions(store: &Store, project_id: &str) -> Result<Vec<StoredStrategy>> {
    let versions = store.strategy_versions(project_id)?;
    if versions
        .iter()
        .any(|v| v.status == VersionStatus::Incumbent)
    {
        return Ok(versions);
    }
    let parent = versions.last().map(|v| v.version);
    let version = store.insert_strategy_version(
        project_id,
        parent,
        VersionStatus::Incumbent,
        &Strategy::default(),
        "built-in default",
        "default",
    )?;
    store.log_strategy_event(project_id, "created", version, "built-in default strategy")?;
    store.strategy_versions(project_id)
}

pub struct Lineup {
    pub incumbent: StoredStrategy,
    pub candidate: Option<StoredStrategy>,
}

pub fn lineup(store: &Store, project_id: &str) -> Result<Lineup> {
    let all = versions(store, project_id)?;
    let candidate = all
        .iter()
        .rev()
        .find(|v| v.status == VersionStatus::Candidate)
        .cloned();
    let incumbent = all
        .into_iter()
        .rev()
        .find(|v| v.status == VersionStatus::Incumbent)
        .ok_or_else(|| anyhow!("project {project_id} has no strategy in force"))?;
    Ok(Lineup {
        incumbent,
        candidate,
    })
}

/// Opens a trial for `strategy` against the incumbent.
pub fn propose(
    store: &Store,
    project_id: &str,
    strategy: &Strategy,
    rationale: &str,
    author: &str,
) -> Result<u32> {
    let rationale = rationale.trim();
    if rationale.is_empty() {
        return Err(anyhow!(
            "Give the evidence behind the change in --rationale."
        ));
    }
    let lineup = lineup(store, project_id)?;
    if let Some(open) = &lineup.candidate {
        return Err(anyhow!(
            "v{} is already on trial. Let it finish, or withdraw it with `orx strategy \
             withdraw --reason \"…\"`.",
            open.version
        ));
    }
    if &lineup.incumbent.strategy == strategy {
        return Err(anyhow!(
            "That is the strategy already in force; to keep it, run `orx strategy keep \
             --reason \"…\"`."
        ));
    }
    let version = store.insert_strategy_version(
        project_id,
        Some(lineup.incumbent.version),
        VersionStatus::Candidate,
        strategy,
        rationale,
        author,
    )?;
    store.log_strategy_event(
        project_id,
        "proposed",
        version,
        &format!(
            "by {author} against v{}: {rationale}",
            lineup.incumbent.version
        ),
    )?;
    Ok(version)
}

/// Closes the open trial without promoting it.
pub fn close_candidate(
    store: &Store,
    project_id: &str,
    status: VersionStatus,
    note: &str,
) -> Result<Option<u32>> {
    let Some(candidate) = lineup(store, project_id)?.candidate else {
        return Ok(None);
    };
    store.set_strategy_status(project_id, candidate.version, status, Some(note))?;
    store.log_strategy_event(project_id, status.id(), candidate.version, note)?;
    Ok(Some(candidate.version))
}

/// Puts the open candidate in force.
pub fn promote(store: &Store, project_id: &str, note: &str) -> Result<u32> {
    let lineup = lineup(store, project_id)?;
    let candidate = lineup
        .candidate
        .ok_or_else(|| anyhow!("No strategy is on trial."))?;
    store.set_strategy_status(
        project_id,
        lineup.incumbent.version,
        VersionStatus::Retired,
        Some(&format!("replaced by v{}", candidate.version)),
    )?;
    store.set_strategy_status(
        project_id,
        candidate.version,
        VersionStatus::Incumbent,
        Some(note),
    )?;
    store.log_strategy_event(
        project_id,
        "promoted",
        candidate.version,
        &format!("replaces v{}: {note}", lineup.incumbent.version),
    )?;
    Ok(candidate.version)
}

/// Puts an earlier version back in force as a new version, closing any trial.
/// `to` defaults to the version the incumbent replaced.
pub fn rollback(store: &Store, project_id: &str, to: Option<u32>, author: &str) -> Result<u32> {
    let lineup = lineup(store, project_id)?;
    let target = match to {
        Some(version) => version,
        None => lineup
            .incumbent
            .parent
            .ok_or_else(|| anyhow!("v{} has no earlier version.", lineup.incumbent.version))?,
    };
    if target == lineup.incumbent.version {
        return Err(anyhow!("v{target} is already in force."));
    }
    let source = store
        .get_strategy_version(project_id, target)?
        .ok_or_else(|| anyhow!("There is no v{target}."))?;
    close_candidate(
        store,
        project_id,
        VersionStatus::Withdrawn,
        &format!("closed by a rollback to v{target}"),
    )?;
    store.set_strategy_status(
        project_id,
        lineup.incumbent.version,
        VersionStatus::Retired,
        Some(&format!("rolled back to v{target}")),
    )?;
    let version = store.insert_strategy_version(
        project_id,
        Some(lineup.incumbent.version),
        VersionStatus::Incumbent,
        &source.strategy,
        &format!("rollback to v{target}"),
        author,
    )?;
    store.log_strategy_event(
        project_id,
        "rolled_back",
        version,
        &format!(
            "by {author}: v{} replaced by a copy of v{target}",
            lineup.incumbent.version
        ),
    )?;
    Ok(version)
}

// --- the loop in sleep mode ---------------------------------------------------

/// The judged record of every version, and of the incumbent during the open
/// trial.
pub struct Scoreboard {
    pub by_version: Vec<(u32, ArmStats)>,
    pub control: ArmStats,
    pub outcomes: HashMap<String, Outcome>,
    pub runs: Vec<StrategyRun>,
}

impl Scoreboard {
    pub fn of(&self, version: u32) -> ArmStats {
        self.by_version
            .iter()
            .find(|(v, _)| *v == version)
            .map(|(_, s)| *s)
            .unwrap_or_default()
    }
}

pub fn scoreboard(
    store: &Store,
    sleep: &StoredSleep,
    lineup: &Lineup,
    now: i64,
) -> Result<Option<Scoreboard>> {
    let Some(objective) = &sleep.plan.objective else {
        return Ok(None);
    };
    let loaded = ProjectEvidence::load(store, &sleep.project_id)?;
    let outcomes = outcomes(objective, &loaded.evidence(), now);
    let spend: HashMap<String, f64> = store
        .sleep_charges(&sleep.session_id)?
        .into_iter()
        .map(|c| {
            let usd = sleep::spent_usd(std::slice::from_ref(&c), now);
            (c.run_id, usd)
        })
        .collect();
    let runs = store.strategy_runs(&sleep.project_id)?;
    let mut by_version: Vec<(u32, ArmStats)> = Vec::new();
    let mut control = ArmStats::default();
    for run in &runs {
        let outcome = outcomes.get(&run.run_id).copied();
        let usd = spend.get(&run.run_id).copied().unwrap_or_default();
        match by_version.iter_mut().find(|(v, _)| *v == run.version) {
            Some((_, stats)) => stats.add(outcome, usd),
            None => {
                let mut stats = ArmStats::default();
                stats.add(outcome, usd);
                by_version.push((run.version, stats));
            }
        }
        if let Some(candidate) = &lineup.candidate {
            if run.version == lineup.incumbent.version && run.created_at >= candidate.created_at {
                control.add(outcome, usd);
            }
        }
    }
    Ok(Some(Scoreboard {
        by_version,
        control,
        outcomes,
        runs,
    }))
}

/// Whether self-improvement runs for this plan: it is on and has an objective
/// to measure by.
pub fn active(sleep: &StoredSleep) -> bool {
    sleep.plan.self_improve && sleep.plan.objective.is_some()
}

/// The version a sleep chat's next turn runs under, advancing the
/// alternation: during a trial, every other turn runs the candidate.
pub fn next_turn(store: &Store, sleep: &StoredSleep) -> Result<(StoredStrategy, bool)> {
    let lineup = lineup(store, &sleep.project_id)?;
    let previous = store.get_strategy_arm(&sleep.session_id)?;
    let turns = previous.map_or(0, |arm| arm.turns).wrapping_add(1);
    let (strategy, on_trial) = match lineup.candidate {
        Some(candidate) if active(sleep) && turns % 2 == 1 => (candidate, true),
        _ => (lineup.incumbent, false),
    };
    store.set_strategy_arm(
        &sleep.session_id,
        &sleep.project_id,
        &StrategyArm {
            version: strategy.version,
            turns,
        },
    )?;
    Ok((strategy, on_trial))
}

/// The version the sleep chat's current turn runs under.
pub fn current(store: &Store, sleep: &StoredSleep) -> Result<StoredStrategy> {
    let lineup = lineup(store, &sleep.project_id)?;
    if let Some(arm) = store.get_strategy_arm(&sleep.session_id)? {
        if let Some(candidate) = lineup.candidate.filter(|c| c.version == arm.version) {
            return Ok(candidate);
        }
    }
    Ok(lineup.incumbent)
}

/// Attributes a launch governed by `sleep` to the version its turn runs under.
pub fn attribute_run(
    store: &Store,
    sleep: &StoredSleep,
    run_id: &str,
    created_at: i64,
) -> Result<()> {
    let version = current(store, sleep)?.version;
    store.record_strategy_run(&sleep.project_id, run_id, version, created_at)
}

/// When the last retrospective window closed: a proposal, a keep, or a
/// trial's end. Runs judged after this count toward the next one.
fn window_start(events: &[StrategyEvent], sleep: &StoredSleep) -> i64 {
    events
        .iter()
        .rev()
        .find(|e| e.kind != "created")
        .map_or(sleep.created_at, |e| e.at.max(sleep.created_at))
}

/// A retrospective that is due, with what the agent needs to run it.
#[derive(Debug, Clone, PartialEq)]
pub struct Retrospective {
    /// Identifies the window, so each one is nudged once.
    pub key: String,
    pub summary: String,
}

pub fn retrospective_due(
    store: &Store,
    sleep: &StoredSleep,
    now: i64,
) -> Result<Option<Retrospective>> {
    if !active(sleep) || sleep.state.finished.is_some() {
        return Ok(None);
    }
    let lineup = lineup(store, &sleep.project_id)?;
    if lineup.candidate.is_some() {
        return Ok(None);
    }
    let Some(board) = scoreboard(store, sleep, &lineup, now)? else {
        return Ok(None);
    };
    let since = window_start(&store.strategy_events(&sleep.project_id)?, sleep);
    let judged = board
        .runs
        .iter()
        .filter(|r| r.created_at >= since)
        .filter(|r| {
            matches!(
                board.outcomes.get(&r.run_id),
                Some(Outcome::Win | Outcome::Loss)
            )
        })
        .count();
    let every = lineup.incumbent.strategy.knobs.reflect_every_runs as usize;
    if judged < every {
        return Ok(None);
    }
    let stats = board.of(lineup.incumbent.version);
    Ok(Some(Retrospective {
        key: format!("{}@{since}", lineup.incumbent.version),
        summary: format!(
            "{judged} run(s) judged since the last retrospective; strategy v{} in force has \
             {} overall.",
            lineup.incumbent.version,
            record(&stats)
        ),
    }))
}

/// One watchdog step: decide the open trial from the evidence so far.
/// Returns the change log line when something was decided.
pub fn advance(
    store: &Store,
    sleep: &StoredSleep,
    active_runs: usize,
    now: i64,
) -> Result<Option<String>> {
    if !active(sleep) {
        return Ok(None);
    }
    let lineup = lineup(store, &sleep.project_id)?;
    let Some(candidate) = &lineup.candidate else {
        return Ok(None);
    };
    let Some(board) = scoreboard(store, sleep, &lineup, now)? else {
        return Ok(None);
    };
    let ended = sleep.state.finished.is_some() && active_runs == 0;
    let decision = evaluate_trial(
        &board.of(candidate.version),
        &board.control,
        &board.of(lineup.incumbent.version),
        ended,
    );
    let v = candidate.version;
    Ok(match decision {
        TrialDecision::Continue => None,
        TrialDecision::Promote(note) => {
            promote(store, &sleep.project_id, &note)?;
            Some(format!("v{v} promoted: {note}"))
        }
        TrialDecision::Reject(note) => {
            close_candidate(store, &sleep.project_id, VersionStatus::Rejected, &note)?;
            Some(format!(
                "v{v} rejected, v{} stays: {note}",
                lineup.incumbent.version
            ))
        }
        TrialDecision::Inconclusive(note) => {
            close_candidate(store, &sleep.project_id, VersionStatus::Inconclusive, &note)?;
            Some(format!("v{v} inconclusive: {note}"))
        }
    })
}

/// The `<orx-strategy>` block for a sleep turn, advancing the alternation.
pub fn turn_context(store: &Store, sleep: &StoredSleep) -> Result<String> {
    let (strategy, on_trial) = next_turn(store, sleep)?;
    let mut notes = Vec::new();
    if on_trial {
        notes.push(format!(
            "This turn runs candidate v{} on trial against v{}; turns alternate between them and \
             orx keeps the one whose runs win more often. Follow it faithfully so the trial is \
             fair.",
            strategy.version,
            strategy.parent.unwrap_or_default()
        ));
    } else if let Some(candidate) = lineup(store, &sleep.project_id)?
        .candidate
        .filter(|_| active(sleep))
    {
        notes.push(format!(
            "This turn runs v{} in force; candidate v{} runs on the other turns. Follow this \
             version faithfully so the trial is fair.",
            strategy.version, candidate.version
        ));
    }
    if active(sleep) {
        if let Some(event) = store
            .strategy_events(&sleep.project_id)?
            .into_iter()
            .rev()
            .find(|e| {
                matches!(
                    e.kind.as_str(),
                    "promoted" | "rejected" | "inconclusive" | "rolled_back"
                )
            })
            .filter(|e| e.at >= sleep.created_at)
        {
            notes.push(format!(
                "Last strategy decision: v{} {}: {}",
                event.version,
                event.kind.replace('_', " "),
                event.detail
            ));
        }
        if let Some(due) = retrospective_due(store, sleep, crate::store::now_ms())? {
            notes.push(format!(
                "A retrospective is due: {} Run it before planning the next round; see the \
                 retrospective rules below.",
                due.summary
            ));
        }
    }
    let mut text = format!(
        "<orx-strategy version=\"{}\">\nYour way of working, version {}. It shapes how you \
         research; the sleep-mode rules and limits above always take precedence.\n",
        strategy.version, strategy.version
    );
    for note in notes {
        text.push_str(&format!("{note}\n"));
    }
    text.push('\n');
    text.push_str(&strategy.strategy.render());
    if active(sleep) {
        text.push_str(&format!(
            "\n\nSelf-improvement (retrospectives):\n{}\nPropose a revision with `orx strategy \
             propose --rationale \"…\" --file -` (JSON with any of `approach`, `research`, \
             `retrospective`, `knobs`; omitted fields stay), or keep this one with `orx strategy \
             keep --reason \"…\"`. orx then trials it against the version in force on real \
             runs and keeps whichever wins; you cannot promote it yourself.",
            strategy.strategy.retrospective.trim()
        ));
    }
    text.push_str("\n</orx-strategy>");
    Ok(text)
}

/// The sleep plan as the watchdog applies it: the strategy's stall threshold
/// in place of the plan's default, when it sets one.
pub fn effective_plan(store: &Store, sleep: &StoredSleep) -> sleep::SleepPlan {
    let mut plan = sleep.plan.clone();
    if let Some(n) = current(store, sleep)
        .ok()
        .and_then(|p| p.strategy.knobs.stall_runs)
    {
        plan.stall_runs = n;
    }
    plan
}

// --- reporting ----------------------------------------------------------------

fn version_json(v: &StoredStrategy) -> Value {
    json!({
        "version": v.version,
        "parent": v.parent,
        "status": v.status.id(),
        "author": v.author,
        "rationale": v.rationale,
        "createdAt": v.created_at,
        "decidedAt": v.decided_at,
        "decision": v.decision,
    })
}

/// The strategy state as the dashboard and `orx strategy show --json` show it.
pub fn status_json(store: &Store, sleep: &StoredSleep) -> Result<Value> {
    let lineup = lineup(store, &sleep.project_id)?;
    let board = scoreboard(store, sleep, &lineup, crate::store::now_ms())?;
    let stats = |version: u32| board.as_ref().map(|b| b.of(version));
    let events = store.strategy_events(&sleep.project_id)?;
    let recent =
        events.iter().rev().take(8).map(
            |e| json!({ "at": e.at, "kind": e.kind, "version": e.version, "detail": e.detail }),
        );
    let with = |v: &StoredStrategy, extra: Value| {
        let mut object = version_json(v);
        if let (Value::Object(object), Value::Object(extra)) = (&mut object, extra) {
            object.extend(extra);
        }
        object
    };
    Ok(json!({
        "selfImprove": sleep.plan.self_improve,
        "measurable": sleep.plan.objective.is_some(),
        "incumbent": with(&lineup.incumbent, json!({ "stats": stats(lineup.incumbent.version) })),
        "candidate": lineup.candidate.as_ref().map(|c| with(c, json!({
            "stats": stats(c.version),
            "control": board.as_ref().map(|b| b.control),
        }))),
        "events": recent.collect::<Vec<_>>(),
    }))
}

/// A line diff of two texts, `-` for removed and `+` for added lines.
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(format!("  {}", a[i]));
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            out.push(format!("- {}", a[i]));
            i += 1;
        } else {
            out.push(format!("+ {}", b[j]));
            j += 1;
        }
    }
    out.join("\n")
}

/// The strategy as reviewable text: its sections and knobs, one per line.
pub fn as_text(strategy: &Strategy) -> String {
    let knobs = serde_json::to_string_pretty(&strategy.knobs).unwrap_or_default();
    format!(
        "## approach\n{}\n\n## research\n{}\n\n## retrospective\n{}\n\n## knobs\n{knobs}",
        strategy.approach.trim(),
        strategy.research.trim(),
        strategy.retrospective.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::sleep::{Direction, SleepPlan};
    use crate::store::run_metrics::RunMetrics;
    use crate::store::StoredRun;
    use std::collections::BTreeMap;

    fn run(id: &str, status: &str, created: i64, ended: Option<i64>) -> StoredRun {
        StoredRun {
            id: id.into(),
            experiment_id: format!("exp_{id}"),
            project_id: "p".into(),
            status: status.into(),
            backend_json: "{}".into(),
            command: String::new(),
            created_at: created,
            updated_at: ended.unwrap_or(created),
            ended_at: ended,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }
    }

    fn metrics(value: f64) -> RunMetrics {
        RunMetrics {
            version: 1,
            dataset: "d".into(),
            split: "test".into(),
            metrics: BTreeMap::from([("acc".to_string(), value)]),
            parameters: BTreeMap::new(),
            seed: None,
        }
    }

    fn objective() -> Objective {
        Objective {
            metric: "acc".into(),
            direction: Direction::Max,
            improve_pct: None,
            baseline_value: None,
            baseline_run: None,
        }
    }

    fn stats(wins: usize, losses: usize) -> ArmStats {
        ArmStats {
            launched: wins + losses,
            wins,
            losses,
            ..ArmStats::default()
        }
    }

    #[test]
    fn wins_beat_the_best_that_existed_at_launch() {
        let runs = [
            run("base", "done", 0, Some(10)),
            // Launched before `a` finished: judged against the baseline only.
            run("a", "done", 20, Some(40)),
            run("b", "done", 30, Some(50)),
            // Launched after `a` finished, does not beat it.
            run("c", "done", 45, Some(60)),
            run("d", "failed", 46, Some(61)),
            run("e", "done", 47, Some(62)),
            run("f", "running", 48, None),
            run("late", "done", 49, Some(METRIC_GRACE_MS + 100)),
        ];
        let m = [
            Some(metrics(0.50)),
            Some(metrics(0.70)),
            Some(metrics(0.60)),
            Some(metrics(0.65)),
            None,
            None,
            None,
            None,
        ];
        let evidence: Vec<RunEvidence<'_>> = runs
            .iter()
            .zip(&m)
            .enumerate()
            .map(|(i, (run, m))| RunEvidence {
                run,
                metrics: m.as_ref(),
                root: i == 0,
            })
            .collect();
        let now = METRIC_GRACE_MS + 200;
        let o = outcomes(&objective(), &evidence, now);
        assert_eq!(o["base"], Outcome::Loss);
        assert_eq!(o["a"], Outcome::Win);
        assert_eq!(o["b"], Outcome::Win);
        assert_eq!(o["c"], Outcome::Loss);
        assert_eq!(o["d"], Outcome::Loss);
        // Finished long ago without metrics.
        assert_eq!(o["e"], Outcome::Loss);
        assert_eq!(o["f"], Outcome::Pending);
        assert_eq!(o["late"], Outcome::Pending);
    }

    #[test]
    fn trials_promote_clear_gains_and_reject_losses() {
        // Too early to tell.
        assert_eq!(
            evaluate_trial(&stats(3, 0), &stats(0, 1), &stats(0, 1), false),
            TrialDecision::Continue
        );
        assert!(matches!(
            evaluate_trial(&stats(3, 1), &stats(0, 3), &stats(0, 3), false),
            TrialDecision::Promote(_)
        ));
        assert!(matches!(
            evaluate_trial(&stats(0, 6), &stats(3, 1), &stats(3, 1), false),
            TrialDecision::Reject(_)
        ));
        // Level after a full trial: the incumbent stays.
        assert!(matches!(
            evaluate_trial(&stats(2, 8), &stats(2, 8), &stats(2, 8), false),
            TrialDecision::Reject(_)
        ));
        // A full trial with no control runs falls back to the incumbent's history.
        assert!(matches!(
            evaluate_trial(&stats(5, 5), &stats(0, 0), &stats(1, 9), false),
            TrialDecision::Promote(_)
        ));
        assert!(matches!(
            evaluate_trial(&stats(1, 1), &stats(0, 0), &stats(0, 0), true),
            TrialDecision::Inconclusive(_)
        ));
    }

    #[test]
    fn changes_merge_and_hard_limits_are_not_fields() {
        let base = Strategy::default();
        let changed = merged(
            &base,
            json!({ "approach": "Smoke-test first.", "knobs": { "exploreShare": 0.5 } }),
        )
        .unwrap();
        assert_eq!(changed.approach, "Smoke-test first.");
        assert_eq!(changed.knobs.explore_share, 0.5);
        assert_eq!(changed.knobs.subagents, base.knobs.subagents);
        assert_eq!(changed.research, base.research);
        for bad in [
            json!({ "gpuBudgetUsd": 100 }),
            json!({ "knobs": { "idleSecs": 1 } }),
            json!({ "knobs": { "exploreShare": 2.0 } }),
            json!({ "approach": " " }),
        ] {
            assert!(merged(&base, bad).is_err());
        }
    }

    #[test]
    fn diffs_mark_changed_lines() {
        assert_eq!(diff("a\nb\nc", "a\nx\nc"), "  a\n- b\n+ x\n  c");
    }

    #[test]
    fn trials_alternate_turns_and_decisions_are_logged() {
        let root = std::env::temp_dir().join(format!("orx-strategy-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(root.clone()).unwrap();
            let sleep = StoredSleep {
                session_id: "chat".into(),
                project_id: "p".into(),
                plan: SleepPlan {
                    objective: Some(objective()),
                    ..SleepPlan::default()
                },
                state: Default::default(),
                created_at: 0,
                updated_at: 0,
            };
            assert_eq!(lineup(&store, "p").unwrap().incumbent.version, 0);
            let change = merged(&Strategy::default(), json!({ "approach": "New." })).unwrap();
            assert!(propose(&store, "p", &change, " ", "agent").is_err());
            let v1 = propose(&store, "p", &change, "evidence", "agent").unwrap();
            assert!(propose(&store, "p", &change, "again", "agent").is_err());
            let arms: Vec<(u32, bool)> = (0..4)
                .map(|_| {
                    let (p, trial) = next_turn(&store, &sleep).unwrap();
                    (p.version, trial)
                })
                .collect();
            assert_eq!(arms, [(1, true), (0, false), (1, true), (0, false)]);
            assert!(turn_context(&store, &sleep)
                .unwrap()
                .contains("candidate v1"));
            assert_eq!(current(&store, &sleep).unwrap().version, v1);

            let promoted = promote(&store, "p", "won").unwrap();
            assert_eq!(lineup(&store, "p").unwrap().incumbent.version, promoted);
            let back = rollback(&store, "p", None, "user").unwrap();
            let lineup = lineup(&store, "p").unwrap();
            assert_eq!(lineup.incumbent.version, back);
            assert_eq!(lineup.incumbent.strategy, Strategy::default());
            let kinds: Vec<String> = store
                .strategy_events("p")
                .unwrap()
                .into_iter()
                .map(|e| e.kind)
                .collect();
            assert_eq!(kinds, ["created", "proposed", "promoted", "rolled_back"]);
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
