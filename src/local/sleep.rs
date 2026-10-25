//! Sleep mode: a chat that keeps researching while nobody watches the
//! machine, typically overnight.
//!
//! The agent owns the research decisions. orx owns what an unattended agent
//! cannot be trusted to keep by itself:
//!
//! - **A hard GPU budget.** Every run an agent launches in the project while a
//!   plan exists is charged its hourly rate from submission. A launch that
//!   would not fit is refused, and paid runs are cancelled before the ledger
//!   reaches the budget.
//! - **No stalls.** A watchdog in `orx up` wakes the agent when its chat sits
//!   idle, backs off while every harness is out of quota, interrupts a turn
//!   that hangs, and keeps the computer from suspending.
//! - **Harness failover.** A turn that stops on a usage limit continues on
//!   another account or harness even when failover is off in Settings.
//! - **Stop conditions.** A deadline, and an optional target such as "beat the
//!   baseline by 15% on `val_acc`", checked by orx against imported run
//!   metrics rather than the agent's own claim. A target written in plain
//!   words is judged by the agent instead, which finishes the plan with
//!   `orx sleep done` once the evidence shows it is reached.
//! - **Escalation.** When finished runs stop improving the best result, the
//!   agent is told to fan out cheap research subagents across sources before
//!   trying again, and to reassess whether the target is still feasible.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};
use crate::store::run_metrics::RunMetrics;
use crate::store::sleep::{SleepCharge, StoredSleep};
use crate::store::{Store, StoredRun};

/// Paid runs are cancelled once the ledger plus this much of their combined
/// burn would reach the budget, which covers the watchdog interval and the
/// time a provider takes to release the machine.
pub const CANCEL_MARGIN_SECS: f64 = 180.0;
/// A paid launch must leave the running jobs, itself included, this long.
pub const MIN_RUNWAY_SECS: f64 = 15.0 * 60.0;
/// The longest the watchdog waits between nudges, however often they failed.
pub const MAX_BACKOFF_SECS: u64 = 60 * 60;
/// While runs are in flight the agent is woken by their results, so the
/// watchdog only checks in this rarely.
pub const ACTIVE_RUN_CHECK_SECS: u64 = 30 * 60;
/// The longest target the user can write; it is sent with every turn.
pub const MAX_GOAL_CHARS: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Max,
    Min,
}

impl Direction {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "max" | "higher" => Some(Self::Max),
            "min" | "lower" => Some(Self::Min),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Max => "max",
            Self::Min => "min",
        }
    }

    pub fn better(self, a: f64, b: f64) -> bool {
        match self {
            Self::Max => a > b,
            Self::Min => a < b,
        }
    }
}

/// The metric the night is judged on, as imported with `orx exp metrics`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Objective {
    pub metric: String,
    #[serde(default)]
    pub direction: Direction,
    /// Stop once the best run beats the baseline by this many percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub improve_pct: Option<f64>,
    /// A known baseline value, when the baseline was not run in orx.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_value: Option<f64>,
    /// The run whose metrics are the baseline; default: the newest finished
    /// run of a root experiment that reported the metric.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_run: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepPlan {
    /// Hard cap, in US dollars, on GPU spend for runs agents launch.
    pub gpu_budget_usd: f64,
    /// What the user wants done, in their words. The agent judges when it is
    /// reached and then finishes the plan itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    /// Stop at this time (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<Objective>,
    /// Wake an idle chat after this long.
    #[serde(default = "default_idle_secs")]
    pub idle_secs: u64,
    /// Finished runs without a new best before the agent must research.
    #[serde(default = "default_stall_runs")]
    pub stall_runs: u32,
    /// Interrupt a single turn that runs longer than this.
    #[serde(default = "default_max_turn_secs")]
    pub max_turn_secs: u64,
    /// Keep the computer from sleeping while the plan is active.
    #[serde(default = "default_true")]
    pub keep_awake: bool,
    /// Let the agent revise its strategy, kept only when runs show it does
    /// better. See [`crate::local::strategy`].
    #[serde(default = "default_true")]
    pub self_improve: bool,
}

fn default_idle_secs() -> u64 {
    5 * 60
}
fn default_stall_runs() -> u32 {
    3
}
fn default_max_turn_secs() -> u64 {
    2 * 60 * 60
}
fn default_true() -> bool {
    true
}

impl Default for SleepPlan {
    fn default() -> Self {
        Self {
            gpu_budget_usd: 0.0,
            goal: None,
            until: None,
            objective: None,
            idle_secs: default_idle_secs(),
            stall_runs: default_stall_runs(),
            max_turn_secs: default_max_turn_secs(),
            keep_awake: true,
            self_improve: true,
        }
    }
}

impl SleepPlan {
    pub fn validated(self) -> Result<Self> {
        if !self.gpu_budget_usd.is_finite() || self.gpu_budget_usd < 0.0 {
            return Err(anyhow!("The GPU budget must be zero or more US dollars."));
        }
        let goal = self
            .goal
            .as_deref()
            .map(str::trim)
            .filter(|goal| !goal.is_empty())
            .map(str::to_string);
        if goal
            .as_ref()
            .is_some_and(|goal| goal.chars().count() > MAX_GOAL_CHARS)
        {
            return Err(anyhow!(
                "Keep the target under {MAX_GOAL_CHARS} characters."
            ));
        }
        if let Some(objective) = &self.objective {
            if objective.metric.trim().is_empty() {
                return Err(anyhow!("Name the metric the target is measured on."));
            }
            if objective
                .improve_pct
                .is_some_and(|pct| !pct.is_finite() || pct <= 0.0)
            {
                return Err(anyhow!(
                    "The improvement target must be a positive percentage."
                ));
            }
            if objective.baseline_value.is_some_and(|v| !v.is_finite()) {
                return Err(anyhow!("The baseline value must be a finite number."));
            }
        }
        if self.idle_secs < 60 {
            return Err(anyhow!("Wake an idle chat after one minute or more."));
        }
        if self.max_turn_secs < 10 * 60 {
            return Err(anyhow!("Allow a turn at least ten minutes."));
        }
        Ok(Self {
            stall_runs: self.stall_runs.max(1),
            goal,
            ..self
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FinishReason {
    TargetMet,
    /// The agent judged, with evidence, that the user's written target is reached.
    GoalMet,
    Deadline,
    /// The agent concluded, with evidence, that it should stop.
    Agent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Finish {
    pub reason: FinishReason,
    pub detail: String,
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Feasible,
    Uncertain,
    Infeasible,
}

impl Verdict {
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "feasible" => Some(Self::Feasible),
            "uncertain" => Some(Self::Uncertain),
            "infeasible" => Some(Self::Infeasible),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Feasible => "feasible",
            Self::Uncertain => "uncertain",
            Self::Infeasible => "infeasible",
        }
    }
}

/// The agent's own analysis of whether the target can be reached.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Assessment {
    pub verdict: Verdict,
    pub note: String,
    /// A lower target the agent committed to instead, when the user's one is
    /// out of reach. The stop condition then uses it; the user's stays on record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revised_improve_pct: Option<f64>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished: Option<Finish>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assessment: Option<Assessment>,
    /// Nudges since the agent last launched a run; drives the backoff.
    #[serde(default)]
    pub nudges: u32,
    #[serde(default)]
    pub kicked_off: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub busy_since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_nudge_at: Option<i64>,
    /// Runs launched in the project when the watchdog last looked.
    #[serde(default)]
    pub seen_runs: usize,
    /// Which stall the agent was last told to research, so each stall is
    /// escalated once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub research_key: Option<String>,
    #[serde(default)]
    pub research_rounds: u32,
    #[serde(default)]
    pub budget_alerted: bool,
    #[serde(default)]
    pub finish_notified: bool,
    #[serde(default)]
    pub interrupts: u32,
    /// Which retrospective window the agent was last asked to reflect on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrospective_key: Option<String>,
    #[serde(default)]
    pub retrospectives: u32,
}

// --- progress -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub run_id: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Progress {
    pub baseline: Option<Measured>,
    pub best: Option<Measured>,
    /// Signed improvement of the best over the baseline, in percent, where
    /// positive is better in the objective's direction.
    pub improvement_pct: Option<f64>,
    /// The target in force: the agent's revised one, else the user's.
    pub target_pct: Option<f64>,
    pub met: bool,
    /// Runs that finished after the current best (or since the plan started,
    /// when the best predates it) without beating it.
    pub runs_since_best: usize,
}

/// The run with its imported measurements, if any.
pub struct RunEvidence<'a> {
    pub run: &'a StoredRun,
    pub metrics: Option<&'a RunMetrics>,
    /// Whether the run's experiment is a root (baseline) node.
    pub root: bool,
}

pub fn improvement_pct(direction: Direction, baseline: f64, value: f64) -> f64 {
    let delta = match direction {
        Direction::Max => value - baseline,
        Direction::Min => baseline - value,
    };
    // A zero baseline has no relative scale; read the delta as the fraction.
    if baseline.abs() < f64::EPSILON {
        delta * 100.0
    } else {
        delta / baseline.abs() * 100.0
    }
}

fn value_of(objective: &Objective, e: &RunEvidence<'_>) -> Option<f64> {
    e.metrics
        .and_then(|m| m.metrics.get(&objective.metric).copied())
}

/// The dataset and split a result must share with the baseline to compare.
pub type Scope = (String, String);

/// The baseline measurement, and the scope of the run it came from.
pub fn baseline_and_scope(
    objective: &Objective,
    evidence: &[RunEvidence<'_>],
) -> (Option<Measured>, Option<Scope>) {
    let baseline_evidence = match &objective.baseline_run {
        Some(id) => evidence.iter().find(|e| &e.run.id == id),
        None => evidence
            .iter()
            .filter(|e| e.root && e.run.status == "done" && value_of(objective, e).is_some())
            .max_by_key(|e| e.run.ended_at.unwrap_or(e.run.created_at)),
    };
    let baseline = match objective.baseline_value {
        Some(value) => Some(Measured {
            run_id: String::new(),
            value,
        }),
        None => baseline_evidence.and_then(|e| {
            value_of(objective, e).map(|value| Measured {
                run_id: e.run.id.clone(),
                value,
            })
        }),
    };
    // Only measurements on the baseline's dataset and split are comparable.
    let scope = baseline_evidence
        .filter(|_| objective.baseline_value.is_none())
        .and_then(|e| e.metrics)
        .map(|m| (m.dataset.clone(), m.split.clone()));
    (baseline, scope)
}

/// The run's objective value, when it finished and is comparable.
pub fn comparable_value(
    objective: &Objective,
    scope: Option<&Scope>,
    e: &RunEvidence<'_>,
) -> Option<f64> {
    let in_scope = scope.is_none_or(|(dataset, split)| {
        e.metrics
            .is_some_and(|m| &m.dataset == dataset && &m.split == split)
    });
    value_of(objective, e).filter(|_| e.run.status == "done" && in_scope)
}

pub fn progress(
    objective: &Objective,
    assessment: Option<&Assessment>,
    started_at: i64,
    evidence: &[RunEvidence<'_>],
) -> Progress {
    let (baseline, scope) = baseline_and_scope(objective, evidence);
    let mut ordered: Vec<&RunEvidence<'_>> = evidence.iter().collect();
    ordered.sort_by_key(|e| e.run.ended_at.unwrap_or(e.run.created_at));
    let mut best: Option<(&RunEvidence<'_>, f64)> = None;
    let mut runs_since_best = 0;
    for e in ordered {
        let finished = matches!(e.run.status.as_str(), "done" | "failed");
        if !finished {
            continue;
        }
        let value = comparable_value(objective, scope.as_ref(), e);
        match (value, best) {
            (Some(value), None) => {
                best = Some((e, value));
                runs_since_best = 0;
            }
            (Some(value), Some((_, current))) if objective.direction.better(value, current) => {
                best = Some((e, value));
                runs_since_best = 0;
            }
            _ => {
                if e.run.created_at >= started_at {
                    runs_since_best += 1;
                }
            }
        }
    }
    let best = best.map(|(e, value)| Measured {
        run_id: e.run.id.clone(),
        value,
    });
    let improvement = baseline
        .as_ref()
        .zip(best.as_ref())
        .map(|(b, best)| improvement_pct(objective.direction, b.value, best.value));
    let target_pct = assessment
        .and_then(|a| a.revised_improve_pct)
        .or(objective.improve_pct);
    let met = improvement
        .zip(target_pct)
        .is_some_and(|(have, want)| have + 1e-9 >= want);
    Progress {
        baseline,
        best,
        improvement_pct: improvement,
        target_pct,
        met,
        runs_since_best,
    }
}

/// A project's runs with their measurements, loaded once.
pub struct ProjectEvidence {
    runs: Vec<StoredRun>,
    metrics: std::collections::BTreeMap<String, RunMetrics>,
    roots: HashSet<String>,
}

impl ProjectEvidence {
    pub fn load(store: &Store, project_id: &str) -> Result<Self> {
        Ok(Self {
            runs: store.list_runs_by_project(project_id)?,
            metrics: store.project_run_metrics(project_id)?,
            roots: store
                .list_experiments_by_project(project_id)?
                .into_iter()
                .filter(|e| e.parent_experiment_id.is_none())
                .map(|e| e.id)
                .collect(),
        })
    }

    pub fn evidence(&self) -> Vec<RunEvidence<'_>> {
        self.runs
            .iter()
            .map(|run| RunEvidence {
                run,
                metrics: self.metrics.get(&run.id),
                root: self.roots.contains(&run.experiment_id),
            })
            .collect()
    }
}

/// The plan's progress from the store.
pub fn progress_from_store(store: &Store, sleep: &StoredSleep) -> Result<Option<Progress>> {
    let Some(objective) = &sleep.plan.objective else {
        return Ok(None);
    };
    let loaded = ProjectEvidence::load(store, &sleep.project_id)?;
    Ok(Some(progress(
        objective,
        sleep.state.assessment.as_ref(),
        sleep.created_at,
        &loaded.evidence(),
    )))
}

/// Starts the session's plan, or changes a running one in place so its
/// ledger keeps counting. Returns whether a running plan was updated.
pub fn apply_plan(
    store: &Store,
    session_id: &str,
    project_id: &str,
    plan: &SleepPlan,
) -> Result<bool> {
    match store.get_sleep(session_id)? {
        Some(existing) if existing.state.finished.is_none() => {
            store.update_sleep_plan(session_id, plan)?;
            Ok(true)
        }
        _ => {
            store.put_sleep(session_id, project_id, plan)?;
            Ok(false)
        }
    }
}

/// Whether the chat's permission mode would stop a turn to ask the user.
pub fn chat_prompts(chat: &crate::store::StoredChatSession) -> bool {
    use crate::local::harness::PermissionMode;
    matches!(
        crate::local::harness::effective_permission_id(
            &chat.harness,
            chat.permission_mode.as_deref()
        )
        .and_then(|id| PermissionMode::from_id(&id)),
        Some(PermissionMode::Ask | PermissionMode::AcceptEdits | PermissionMode::Plan)
    )
}

/// The plan, ledger, and progress as the dashboard and `orx sleep status --json` show them.
pub fn status_json(store: &Store, stored: &StoredSleep) -> Result<serde_json::Value> {
    use serde_json::json;
    let charges = store.sleep_charges(&stored.session_id)?;
    let budget = Budget::of(&stored.plan, &charges, crate::store::now_ms());
    let progress = progress_from_store(store, stored)?;
    let measured = |m: &Measured| json!({ "runId": m.run_id, "value": m.value });
    let prompts = store
        .get_chat_session(&stored.session_id)?
        .is_some_and(|chat| chat_prompts(&chat));
    Ok(json!({
        "sessionId": stored.session_id,
        "projectId": stored.project_id,
        "startedAt": stored.created_at,
        "plan": stored.plan,
        "state": stored.state,
        "chatPrompts": prompts,
        "budget": {
            "limitUsd": budget.limit,
            "spentUsd": budget.spent,
            "leftUsd": budget.left(),
            "burnUsdPerHour": budget.burn,
        },
        "progress": progress.as_ref().map(|p| json!({
            "baseline": p.baseline.as_ref().map(measured),
            "best": p.best.as_ref().map(measured),
            "improvementPct": p.improvement_pct,
            "targetPct": p.target_pct,
            "met": p.met,
            "runsSinceBest": p.runs_since_best,
        })),
        "strategy": crate::local::strategy::status_json(store, stored)
            .map_err(|err| eprintln!("orx: strategy: {err}"))
            .ok(),
        "charges": charges.iter().map(|c| json!({
            "runId": c.run_id,
            "usdPerHour": c.usd_per_hour,
            "startedAt": c.started_at,
            "endedAt": c.ended_at,
            "status": c.status,
        })).collect::<Vec<_>>(),
    }))
}

// --- budget -------------------------------------------------------------------

fn hours(from: i64, to: i64) -> f64 {
    (to - from).max(0) as f64 / 3_600_000.0
}

/// Dollars charged so far.
pub fn spent_usd(charges: &[SleepCharge], now: i64) -> f64 {
    charges
        .iter()
        .map(|c| c.usd_per_hour * hours(c.started_at, c.ended_at.unwrap_or(now)))
        .fold(0.0, |total, usd| total + usd)
}

/// Dollars per hour the still-running paid runs add.
pub fn burn_per_hour(charges: &[SleepCharge]) -> f64 {
    charges
        .iter()
        .filter(|c| c.ended_at.is_none() && !crate::local::is_terminal(&c.status))
        .map(|c| c.usd_per_hour)
        .fold(0.0, |total, usd| total + usd)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    pub limit: f64,
    pub spent: f64,
    pub burn: f64,
}

impl Budget {
    pub fn of(plan: &SleepPlan, charges: &[SleepCharge], now: i64) -> Self {
        Self {
            limit: plan.gpu_budget_usd,
            spent: spent_usd(charges, now),
            burn: burn_per_hour(charges),
        }
    }

    pub fn left(&self) -> f64 {
        (self.limit - self.spent).max(0.0)
    }

    /// Paid runs must stop now to stay under the limit.
    pub fn must_cancel(&self) -> bool {
        self.burn > 0.0 && self.spent + self.burn * CANCEL_MARGIN_SECS / 3600.0 >= self.limit
    }

    /// Whether a new run at `rate` fits beside the ones already burning:
    /// together they must have [`MIN_RUNWAY_SECS`] before the cancel margin.
    pub fn admits(&self, rate: f64) -> bool {
        rate <= 0.0
            || self.spent + (self.burn + rate) * (CANCEL_MARGIN_SECS + MIN_RUNWAY_SECS) / 3600.0
                <= self.limit
    }
}

/// The hourly price of a launch, from the provider price table. An unknown
/// flavor on a priced backend is charged at the backend's dearest offer, so
/// the ledger never undercounts. `None` when the backend cannot be priced.
pub fn rate_from_overview(
    overview: &crate::jobs::provider_prices::Overview,
    backend: &str,
    flavor: Option<&str>,
) -> Option<f64> {
    let Some(provider) = overview.providers.iter().find(|p| p.id == backend) else {
        return matches!(backend, "local" | "ssh" | "slurm" | "k8s" | "ray").then_some(0.0);
    };
    if provider.billing == "own" {
        return Some(0.0);
    }
    let priced: Vec<(String, f64)> = provider
        .offers
        .iter()
        .filter_map(|o| Some((o.flavor.to_ascii_lowercase(), o.usd_per_hour?)))
        .collect();
    let dearest = priced.iter().map(|(_, rate)| *rate).reduce(f64::max);
    let default_flavor = match backend {
        "colab" | "kaggle" => Some("t4"),
        _ => None,
    };
    let Some(flavor) = flavor.or(default_flavor) else {
        return dearest;
    };
    let flavor = flavor
        .split('@')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if let Some((_, rate)) = priced.iter().find(|(f, _)| *f == flavor) {
        return Some(*rate);
    }
    if let Some((base, suffix)) = flavor.split_once(':') {
        let count = suffix.parse::<f64>().ok().filter(|n| *n >= 1.0);
        if let Some((_, rate)) = priced.iter().find(|(f, _)| f == base) {
            return Some(rate * count.unwrap_or(1.0));
        }
    }
    dearest
}

/// A run an agent wants to launch, against the project's sleep plan.
pub struct Admission {
    pub session_id: String,
    pub usd_per_hour: f64,
}

/// The plan that governs launches in `project_id`: the launching chat's own,
/// else any other chat's in the project.
pub fn governing_plan(
    store: &Store,
    project_id: &str,
    chat_session_id: &str,
) -> Result<Option<StoredSleep>> {
    let mut plans: Vec<StoredSleep> = store
        .list_sleeps()?
        .into_iter()
        .filter(|s| s.project_id == project_id)
        .collect();
    if let Some(i) = plans.iter().position(|s| s.session_id == chat_session_id) {
        return Ok(Some(plans.swap_remove(i)));
    }
    Ok(plans.into_iter().next())
}

/// Refuses an agent launch that would break the plan's budget, or `None`
/// when no plan governs it. Runs launched from the dashboard or a terminal
/// are the user's own and are never gated.
pub async fn admit_run(project_id: &str, args: &crate::ExpRunArgs) -> Result<Option<Admission>> {
    let Some(chat_session_id) = args.chat_session_id.as_deref() else {
        return Ok(None);
    };
    let Some(sleep) = governing_plan(&Store::open()?, project_id, chat_session_id)? else {
        return Ok(None);
    };
    let backend = args.backend.as_deref().unwrap_or("local");
    let rate = match backend {
        "local" | "ssh" | "slurm" | "k8s" | "ray" => Some(0.0),
        _ => {
            let overview = crate::jobs::provider_prices::overview(false).await;
            rate_from_overview(&overview, backend, args.flavor.as_deref())
        }
    };
    let Some(rate) = rate else {
        return Err(anyhow!(
            "Sleep mode keeps a hard GPU budget, and orx cannot price a `{backend}` run. \
             Use a backend and flavor with a listed price (`orx compute prices`) or your own \
             hardware (local, ssh, slurm, k8s, ray)."
        ));
    };
    let now = crate::store::now_ms();
    if let Some(finish) = sleep.state.finished.as_ref().filter(|_| rate > 0.0) {
        return Err(anyhow!(
            "Sleep mode has finished ({}), so no more paid runs start. Free hardware is still \
             allowed; the user decides on more GPU time when they are back.",
            finish.detail
        ));
    }
    let charges = Store::open()?.sleep_charges(&sleep.session_id)?;
    let budget = Budget::of(&sleep.plan, &charges, now);
    if !budget.admits(rate) {
        return Err(anyhow!(
            "Sleep mode GPU budget: ${:.2} of ${:.2} used, ${:.2}/h already running. A run at \
             ${rate:.2}/h does not fit. Use a cheaper flavor, free hardware (local, ssh, \
             slurm, k8s, ray, or Kaggle's free quota), or wait for a running job to finish. \
             Only the user can raise the budget.",
            budget.spent,
            budget.limit,
            budget.burn
        ));
    }
    Ok(Some(Admission {
        session_id: sleep.session_id,
        usd_per_hour: rate,
    }))
}

// --- watchdog -----------------------------------------------------------------

/// What the watchdog saw this tick.
#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub now: i64,
    pub busy: bool,
    /// Runs launched in the project so far.
    pub total_runs: usize,
    /// The project's runs still starting or running.
    pub active_runs: usize,
    pub budget: Option<Budget>,
    pub progress: Option<Progress>,
    /// A self-improvement retrospective that is due.
    pub retrospective: Option<crate::local::strategy::Retrospective>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Cancel the plan's paid runs with this reason.
    CancelPaidRuns(String),
    /// Stop the turn that has run past the limit.
    Interrupt,
    /// Wake the agent with a hidden message.
    Nudge(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NudgeKind {
    Kickoff,
    Continue,
    Research,
    Retrospective,
    Finished,
}

/// How long an idle chat waits before the next nudge: the plan's idle time,
/// doubled for every nudge that led to no new run, capped at an hour. Failed
/// turns (every harness out of quota) therefore retry ever less often, but
/// never stop retrying.
pub fn nudge_delay_secs(plan: &SleepPlan, nudges: u32, active_runs: usize) -> u64 {
    let base = if active_runs > 0 {
        plan.idle_secs.max(ACTIVE_RUN_CHECK_SECS)
    } else {
        plan.idle_secs
    };
    base.saturating_mul(1u64 << nudges.min(10))
        .min(MAX_BACKOFF_SECS.max(base))
}

pub fn tick(plan: &SleepPlan, state: &mut SleepState, obs: &Observation) -> Vec<Action> {
    let mut actions = Vec::new();
    if let Some(budget) = obs.budget {
        if budget.must_cancel() {
            actions.push(Action::CancelPaidRuns(format!(
                "Sleep mode GPU budget reached (${:.2} of ${:.2}).",
                budget.spent, budget.limit
            )));
            state.budget_alerted = true;
        }
    }
    if state.finished.is_none() {
        if let Some(until) = plan.until.filter(|until| obs.now >= *until) {
            state.finished = Some(Finish {
                reason: FinishReason::Deadline,
                detail: format!("deadline {} reached", format_time(until)),
                at: obs.now,
            });
        } else if let Some(progress) = obs.progress.as_ref().filter(|p| p.met) {
            state.finished = Some(Finish {
                reason: FinishReason::TargetMet,
                detail: format!(
                    "target met: {:+.1}% over the baseline (target {:+.1}%)",
                    progress.improvement_pct.unwrap_or_default(),
                    progress.target_pct.unwrap_or_default()
                ),
                at: obs.now,
            });
        }
    }
    if obs.busy {
        state.idle_since = None;
        let since = *state.busy_since.get_or_insert(obs.now);
        if obs.now - since >= plan.max_turn_secs as i64 * 1000 {
            state.busy_since = None;
            state.interrupts += 1;
            actions.push(Action::Interrupt);
        }
        return actions;
    }
    state.busy_since = None;
    if obs.total_runs > state.seen_runs {
        state.seen_runs = obs.total_runs;
        state.nudges = 0;
    }
    let kind = if state.finished.is_some() {
        if state.finish_notified {
            return actions;
        }
        NudgeKind::Finished
    } else if !state.kicked_off {
        NudgeKind::Kickoff
    } else {
        let idle_since = *state.idle_since.get_or_insert(obs.now);
        let delay = nudge_delay_secs(plan, state.nudges, obs.active_runs) as i64 * 1000;
        if obs.now - idle_since < delay {
            return actions;
        }
        match stall_key(plan, obs.progress.as_ref()) {
            Some(key) if state.research_key.as_deref() != Some(key.as_str()) => {
                state.research_key = Some(key);
                state.research_rounds += 1;
                NudgeKind::Research
            }
            _ => match &obs.retrospective {
                Some(due) if state.retrospective_key.as_deref() != Some(due.key.as_str()) => {
                    state.retrospective_key = Some(due.key.clone());
                    state.retrospectives += 1;
                    NudgeKind::Retrospective
                }
                _ => NudgeKind::Continue,
            },
        }
    };
    match kind {
        NudgeKind::Kickoff => state.kicked_off = true,
        NudgeKind::Finished => state.finish_notified = true,
        NudgeKind::Continue | NudgeKind::Research | NudgeKind::Retrospective => state.nudges += 1,
    }
    state.idle_since = None;
    state.last_nudge_at = Some(obs.now);
    actions.push(Action::Nudge(nudge_text(kind, state, obs)));
    actions
}

/// Identifies the current stall: the same best with another `stall_runs`
/// finished runs behind it is a new stall.
fn stall_key(plan: &SleepPlan, progress: Option<&Progress>) -> Option<String> {
    let progress = progress?;
    let stall = progress.runs_since_best / plan.stall_runs.max(1) as usize;
    (stall > 0).then(|| {
        format!(
            "{}#{stall}",
            progress.best.as_ref().map_or("", |b| b.run_id.as_str())
        )
    })
}

/// The nudge itself. The live status rides every sleep turn in its
/// `<orx-sleep>` block (see [`turn_context`]), so it is not repeated here.
fn nudge_text(kind: NudgeKind, state: &SleepState, obs: &Observation) -> String {
    match kind {
        NudgeKind::Kickoff => {
            "[orx] Sleep mode is on: the user is away and you drive the research until the plan \
             finishes.\nStart with the feasibility analysis the sleep-mode rules ask \
             for, record it with `orx sleep assess`, then plan and launch the first runs."
                .to_string()
        }
        NudgeKind::Continue => {
            "[orx] Sleep mode check-in: this chat has been idle with nothing waking it.\n\
             Continue the research: inspect finished runs (`orx runs`), import their metrics, and \
             launch the next experiments. If you are waiting on runs, register `orx exp wake` \
             for each and end your turn."
                .to_string()
        }
        NudgeKind::Research => format!(
            "[orx] Sleep mode: the last {} finished runs did not beat the best result.\n\
             Stop iterating on the same idea. Run a research round now, as the sleep-mode rules \
             describe: fan out cheap subagents across sources, synthesize what they find into \
             new hypotheses, reassess feasibility with `orx sleep assess`, then launch the most \
             promising ones, including at least one that departs from the current approach.",
            obs.progress.as_ref().map_or(0, |p| p.runs_since_best)
        ),
        NudgeKind::Retrospective => format!(
            "[orx] Sleep mode: a self-improvement retrospective is due. {}\n\
             Review how you have been working, as the retrospective rules in your strategy \
             describe (`orx strategy stats --runs`, `orx strategy show`). Then either propose a \
             revised strategy with `orx strategy propose --rationale \"…\" --file -`, which orx \
             trials against the current one on the next runs, or keep it with `orx strategy keep \
             --reason \"…\"`. Then continue the research.",
            obs.retrospective
                .as_ref()
                .map_or("", |due| due.summary.as_str())
        ),
        NudgeKind::Finished => format!(
            "[orx] Sleep mode finished: {}.\nDo not start new paid runs. Let runs in \
             flight finish, import their metrics, and write the morning report the sleep-mode \
             rules describe.",
            state
                .finished
                .as_ref()
                .map_or("", |finish| finish.detail.as_str())
        ),
    }
}

fn status_lines(
    plan: &SleepPlan,
    state: &SleepState,
    budget: Option<Budget>,
    progress: Option<&Progress>,
) -> String {
    let mut lines = Vec::new();
    if let Some(goal) = &plan.goal {
        lines.push(format!("- Target (the user's words): {goal}"));
    }
    if let Some(budget) = budget {
        lines.push(format!(
            "- GPU budget: ${:.2} of ${:.2} used, ${:.2} left{}",
            budget.spent,
            budget.limit,
            budget.left(),
            if budget.burn > 0.0 {
                format!(", ${:.2}/h running now", budget.burn)
            } else {
                String::new()
            }
        ));
    }
    if let Some(until) = plan.until {
        lines.push(format!("- Stops at {}", format_time(until)));
    }
    if let Some(objective) = &plan.objective {
        let mut line = format!(
            "- Objective: {} `{}`",
            match objective.direction {
                Direction::Max => "maximize",
                Direction::Min => "minimize",
            },
            objective.metric
        );
        if let Some(target) = progress.and_then(|p| p.target_pct) {
            line.push_str(&format!(", stop at {target:+.1}% over the baseline"));
        }
        lines.push(line);
        if let Some(progress) = progress {
            match (&progress.baseline, &progress.best) {
                (None, _) => lines.push(format!(
                    "- Baseline: none measured yet. Run the baseline and import its `{}` with \
                     `orx exp metrics`.",
                    objective.metric
                )),
                (Some(baseline), None) => {
                    lines.push(format!("- Baseline {:.6}; no comparable result yet", baseline.value))
                }
                (Some(baseline), Some(best)) => lines.push(format!(
                    "- Baseline {:.6}, best {:.6} (run `{}`, {:+.2}%), {} finished run(s) since the best",
                    baseline.value,
                    best.value,
                    best.run_id,
                    progress.improvement_pct.unwrap_or_default(),
                    progress.runs_since_best
                )),
            }
        }
    }
    match &state.assessment {
        Some(a) => lines.push(format!(
            "- Your feasibility verdict: {} ({})",
            a.verdict.id(),
            a.note
        )),
        None if plan.objective.is_some() || plan.goal.is_some() => {
            lines.push("- Feasibility: not assessed yet".to_string())
        }
        None => {}
    }
    if let Some(finish) = &state.finished {
        lines.push(format!("- Finished: {}", finish.detail));
    }
    lines.join("\n")
}

pub fn format_time(ms: i64) -> String {
    httpdate::fmt_http_date(
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms.max(0) as u64),
    )
}

/// The sleep-mode rules. Static, so a native session that already holds them
/// gets a one-line reference instead (see [`crate::local::turn_context`]).
const RULES: &str = "\
- Never ask the user a question or wait for approval: decide, write down why in the experiment \
description, and continue. Avoid actions that would need a permission prompt.\n\
- The GPU budget is a hard limit. orx refuses launches that do not fit and cancels paid runs \
before it is exceeded; plan around it.\n\
- Never block a turn: launch with `orx exp run`, register `orx exp wake <expId>`, and end your \
turn. orx wakes you when runs finish and checks in when the chat is idle.\n\
- If your harness runs out of quota, orx continues the chat on another account or harness; \
keep notes in the experiment descriptions so the next harness can pick up.\n\
- Import every finished run's measurements with `orx exp metrics <expId> --run <runId> --file -` \
(version-1 JSON with dataset, split, and metrics). orx judges the stop condition from these \
alone, on the baseline's dataset and split.\n\
- Feasibility: before the first paid run, analyze whether the target is reachable within the \
budget and deadline. Consider the baseline's headroom, the best published results on this \
task, the compute each idea needs, and the noise between seeds. Record it with `orx sleep assess \
--verdict feasible|uncertain|infeasible --note \"…\"`; when the target is out of reach, commit \
to the most ambitious target you judge reachable with `--revised-target <pct>` and pursue that. \
Reassess after every research round.\n\
- When finished runs stop improving the best result, run a research round before more compute, \
as your strategy describes.\n\
- If the plan has a target, check it against your evidence after every finished run. Once the \
evidence shows it is reached, cancel runs that are no longer needed, run `orx sleep done \
--evidence \"…\"` (what was reached, with run ids and numbers), write the morning report, and end \
your turn without starting new work. Never claim the target before the evidence shows it.\n\
- If you conclude with evidence that nothing more is worth spending on, stop with `orx sleep stop \
--reason \"…\"`.\n\
- When sleep mode finishes, write a morning report in the artifacts directory: what was tried, \
the best result against the baseline with run ids, spend, the feasibility verdict, surprising \
findings, the strategy changes and trial results (`orx strategy log`), and recommended next \
steps.";

/// The sleep-mode block, sent with every turn while a plan exists. It replaces
/// the autonomy level: the user is not there to ask. The status is live and
/// always sent; the rules and the strategy text go out in full only when the
/// native session does not already hold them.
pub fn turn_context(
    store: &Store,
    session_id: &str,
    delivered: &crate::local::turn_context::Delivered,
) -> Option<String> {
    let sleep = store.get_sleep(session_id).ok()??;
    let now = crate::store::now_ms();
    let budget = store
        .sleep_charges(session_id)
        .ok()
        .map(|charges| Budget::of(&sleep.plan, &charges, now));
    let progress = progress_from_store(store, &sleep).ok().flatten();
    let status = status_lines(&sleep.plan, &sleep.state, budget, progress.as_ref());
    // The strategy is how the agent works; a broken one must not cost the turn
    // its hard rules.
    let strategy = crate::local::strategy::turn_context(store, &sleep, delivered)
        .map_err(|err| eprintln!("orx: strategy: {err}"))
        .ok();
    Some(format!(
        "<orx-sleep>\n\
Sleep mode is on. The user is away (often asleep) and cannot answer, approve, or unblock anything. \
You own the research decisions; orx enforces the limits below.\n\
{status}\n\n\
{}\n\
</orx-sleep>{}",
        delivered.once("sleep-mode rules", RULES),
        strategy
            .map(|block| format!("\n{block}"))
            .unwrap_or_default()
    ))
}

// --- keep awake ---------------------------------------------------------------

/// Holds the computer awake while any plan is active: a `systemd-inhibit`
/// lock on Linux, `caffeinate` on macOS, and the system-required execution
/// state on Windows, refreshed every tick.
pub mod keep_awake {
    use std::sync::Mutex;

    static HOLDER: Mutex<Option<tokio::process::Child>> = Mutex::new(None);

    pub fn set(active: bool) {
        #[cfg(windows)]
        if active {
            // Resets the system idle timer; the tick runs far more often than
            // any sleep timeout.
            // SAFETY: a plain flag call with no pointers.
            unsafe {
                windows_sys::Win32::System::Power::SetThreadExecutionState(
                    windows_sys::Win32::System::Power::ES_SYSTEM_REQUIRED,
                );
            }
        }
        let mut holder = HOLDER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(child) = holder.as_mut() {
            if !active || child.try_wait().ok().flatten().is_some() {
                let _ = child.start_kill();
                *holder = None;
            }
        }
        if active && holder.is_none() {
            *holder = spawn();
        }
    }

    fn spawn() -> Option<tokio::process::Child> {
        let mut command = if cfg!(target_os = "linux") {
            let mut command = tokio::process::Command::new("systemd-inhibit");
            command.args([
                "--what=sleep:idle",
                "--who=OpenResearch",
                "--why=Sleep mode is running research",
                "--mode=block",
                "sleep",
                "infinity",
            ]);
            command
        } else if cfg!(target_os = "macos") {
            let mut command = tokio::process::Command::new("caffeinate");
            command.args(["-i", "-s"]);
            command
        } else {
            return None;
        };
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        command.spawn().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            updated_at: created,
            ended_at: ended,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }
    }

    fn metrics(dataset: &str, value: f64) -> RunMetrics {
        RunMetrics {
            version: 1,
            dataset: dataset.into(),
            split: "test".into(),
            metrics: BTreeMap::from([("acc".to_string(), value)]),
            parameters: BTreeMap::new(),
            seed: None,
        }
    }

    fn objective(pct: f64) -> Objective {
        Objective {
            metric: "acc".into(),
            direction: Direction::Max,
            improve_pct: Some(pct),
            baseline_value: None,
            baseline_run: None,
        }
    }

    /// An overnight session in one native session: the rules and the
    /// strategy go out in full on the first turn and every
    /// `REFRESH_TURNS + 1` turns, the live status on every one.
    #[test]
    fn sleep_turns_repeat_only_the_live_status() {
        use crate::local::turn_context::{Delivered, REFRESH_TURNS};
        let root = std::env::temp_dir().join(format!("orx-sleep-ctx-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(root.clone()).unwrap();
            let plan = SleepPlan {
                objective: Some(objective(15.0)),
                ..SleepPlan::default()
            };
            store.put_sleep("chat", "p", &plan).unwrap();
            let turns = 40;
            let mut sent: Vec<String> = Vec::new();
            let (mut before, mut after) = (0, 0);
            for _ in 0..turns {
                let full = turn_context(&store, "chat", &Delivered::none()).unwrap();
                let recent = sent.iter().rev().take(REFRESH_TURNS).cloned().collect();
                let once = turn_context(&store, "chat", &Delivered::from_inputs(recent)).unwrap();
                assert!(once.contains("Sleep mode is on") && once.contains("- GPU budget"));
                before += full.len();
                after += once.len();
                sent.push(once);
            }
            assert!(sent[0].contains("Never ask the user a question"));
            assert!(!sent[1].contains("Never ask the user a question"));
            assert!(sent[REFRESH_TURNS + 1].contains("Never ask the user a question"));
            println!(
                "sleep context over {turns} turns: {before} bytes before, {after} after (~{} -> ~{} tokens)",
                before / 4,
                after / 4
            );
            assert!(after * 3 < before);
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn progress_compares_only_the_baselines_dataset_and_counts_stalls() {
        let runs = [
            run("base", "done", 0, Some(10)),
            run("a", "done", 20, Some(30)),
            run("other", "done", 40, Some(50)),
            run("b", "failed", 60, Some(70)),
            run("c", "done", 80, Some(90)),
        ];
        let m = [
            metrics("d1", 0.60),
            metrics("d1", 0.66),
            metrics("d2", 0.99),
            metrics("d1", 0.0),
            metrics("d1", 0.65),
        ];
        let evidence: Vec<RunEvidence<'_>> = runs
            .iter()
            .enumerate()
            .map(|(i, run)| RunEvidence {
                run,
                metrics: (run.status == "done").then(|| &m[i]),
                root: i == 0,
            })
            .collect();
        let p = progress(&objective(15.0), None, 15, &evidence);
        assert_eq!(p.baseline.as_ref().unwrap().run_id, "base");
        assert_eq!(p.best.as_ref().unwrap().run_id, "a");
        assert!((p.improvement_pct.unwrap() - 10.0).abs() < 1e-9);
        assert!(!p.met);
        // `other` is on another dataset, `b` failed, `c` did not beat `a`.
        assert_eq!(p.runs_since_best, 3);

        let assessment = Assessment {
            verdict: Verdict::Infeasible,
            note: String::new(),
            revised_improve_pct: Some(8.0),
            at: 0,
        };
        let p = progress(&objective(15.0), Some(&assessment), 15, &evidence);
        assert_eq!(p.target_pct, Some(8.0));
        assert!(p.met);
    }

    #[test]
    fn minimizing_and_explicit_baselines() {
        assert!((improvement_pct(Direction::Min, 2.0, 1.5) - 25.0).abs() < 1e-9);
        assert!((improvement_pct(Direction::Max, 0.0, 0.2) - 20.0).abs() < 1e-9);
        let runs = [run("a", "done", 0, Some(1))];
        let m = metrics("d", 0.9);
        let evidence = [RunEvidence {
            run: &runs[0],
            metrics: Some(&m),
            root: false,
        }];
        let mut o = objective(10.0);
        o.baseline_value = Some(0.8);
        let p = progress(&o, None, 0, &evidence);
        assert!(p.met);
    }

    fn charge(rate: f64, start: i64, end: Option<i64>, status: &str) -> SleepCharge {
        SleepCharge {
            run_id: "r".into(),
            usd_per_hour: rate,
            started_at: start,
            ended_at: end,
            status: status.into(),
        }
    }

    #[test]
    fn budget_charges_elapsed_time_and_keeps_a_margin() {
        let hour = 3_600_000;
        let charges = [
            charge(2.0, 0, Some(hour), "done"),
            charge(4.0, 0, None, "running"),
        ];
        let plan = SleepPlan {
            gpu_budget_usd: 10.0,
            ..SleepPlan::default()
        };
        let budget = Budget::of(&plan, &charges, hour);
        assert!((budget.spent - 6.0).abs() < 1e-9);
        assert_eq!(budget.burn, 4.0);
        assert!(!budget.must_cancel());
        assert!(budget.admits(0.0));
        assert!(budget.admits(4.0));
        assert!(!budget.admits(12.0));
        let late = Budget::of(&plan, &charges, hour * 2 - 60_000);
        assert!(late.must_cancel());
    }

    #[test]
    fn prices_fall_back_to_the_dearest_offer() {
        use crate::jobs::provider_prices::{Offer, Overview, Provider};
        let offer = |flavor: &str, rate: f64| Offer {
            flavor: flavor.into(),
            gpu: Some(flavor.into()),
            gpu_count: 1,
            vram_gb: None,
            usd_per_hour: Some(rate),
            units_per_hour: None,
            runway_hours: None,
            available: None,
            estimated: true,
            host: None,
        };
        let provider = |id: &'static str, billing: &'static str, offers| Provider {
            id,
            configured: true,
            billing,
            balance: None,
            spend: None,
            offers,
            note: None,
            error: None,
        };
        let overview = Overview {
            providers: vec![
                provider("modal", "usage", vec![offer("t4", 0.6), offer("h100", 4.0)]),
                provider(
                    "colab",
                    "credits",
                    vec![offer("t4", 0.18), offer("a100", 1.2)],
                ),
                provider("local", "own", vec![]),
                provider("tinker", "tokens", vec![]),
            ],
        };
        assert_eq!(
            rate_from_overview(&overview, "modal", Some("T4")),
            Some(0.6)
        );
        assert_eq!(
            rate_from_overview(&overview, "modal", Some("h100:2")),
            Some(8.0)
        );
        assert_eq!(
            rate_from_overview(&overview, "modal", Some("b200")),
            Some(4.0)
        );
        assert_eq!(rate_from_overview(&overview, "modal", None), Some(4.0));
        assert_eq!(rate_from_overview(&overview, "colab", None), Some(0.18));
        assert_eq!(
            rate_from_overview(&overview, "colab", Some("a100:highmem@me")),
            Some(1.2)
        );
        assert_eq!(rate_from_overview(&overview, "local", None), Some(0.0));
        assert_eq!(rate_from_overview(&overview, "ssh", None), Some(0.0));
        assert_eq!(rate_from_overview(&overview, "tinker", None), None);
        assert_eq!(rate_from_overview(&overview, "hf", None), None);
    }

    #[test]
    fn watchdog_kicks_off_backs_off_and_escalates_stalls() {
        let plan = SleepPlan {
            gpu_budget_usd: 1.0,
            idle_secs: 60,
            stall_runs: 2,
            ..SleepPlan::default()
        };
        let mut state = SleepState::default();
        let obs = |now: i64| Observation {
            now,
            ..Observation::default()
        };
        let nudge = |actions: Vec<Action>| {
            actions
                .into_iter()
                .find_map(|a| match a {
                    Action::Nudge(text) => Some(text),
                    _ => None,
                })
                .unwrap_or_default()
        };
        assert!(nudge(tick(&plan, &mut state, &obs(0))).contains("Sleep mode is on"));
        assert!(tick(&plan, &mut state, &obs(1_000)).is_empty());
        assert!(nudge(tick(&plan, &mut state, &obs(61_000))).contains("check-in"));
        assert_eq!(state.nudges, 1);
        // The next nudge waits twice as long.
        assert!(tick(&plan, &mut state, &obs(62_000)).is_empty());
        assert!(tick(&plan, &mut state, &obs(150_000)).is_empty());
        assert!(!tick(&plan, &mut state, &obs(183_000)).is_empty());
        // A new run resets the backoff.
        let mut seen = obs(200_000);
        seen.total_runs = 1;
        tick(&plan, &mut state, &seen);
        assert_eq!(state.nudges, 0);

        let mut stalled = obs(400_000);
        stalled.total_runs = 1;
        stalled.progress = Some(Progress {
            best: Some(Measured {
                run_id: "a".into(),
                value: 1.0,
            }),
            runs_since_best: 2,
            ..Progress::default()
        });
        state.idle_since = Some(0);
        assert!(nudge(tick(&plan, &mut state, &stalled)).contains("research round"));
        state.idle_since = Some(0);
        stalled.now = 10_000_000;
        assert!(nudge(tick(&plan, &mut state, &stalled)).contains("check-in"));
    }

    #[test]
    fn watchdog_asks_for_each_due_retrospective_once() {
        let plan = SleepPlan {
            idle_secs: 60,
            ..SleepPlan::default()
        };
        let mut state = SleepState {
            kicked_off: true,
            ..SleepState::default()
        };
        let mut obs = Observation {
            now: 0,
            retrospective: Some(crate::local::strategy::Retrospective {
                key: "0@0".into(),
                summary: "6 run(s) judged".into(),
            }),
            ..Observation::default()
        };
        assert!(tick(&plan, &mut state, &obs).is_empty());
        obs.now = 61_000;
        let actions = tick(&plan, &mut state, &obs);
        assert!(
            matches!(&actions[0], Action::Nudge(text) if text.contains("retrospective is due"))
        );
        assert_eq!(state.retrospectives, 1);
        // The same window is not asked about again.
        obs.now = 100_000;
        assert!(tick(&plan, &mut state, &obs).is_empty());
        obs.now = 10_000_000;
        let actions = tick(&plan, &mut state, &obs);
        assert!(matches!(&actions[0], Action::Nudge(text) if text.contains("check-in")));
    }

    #[test]
    fn watchdog_finishes_cancels_and_interrupts() {
        let plan = SleepPlan {
            gpu_budget_usd: 1.0,
            until: Some(100_000),
            ..SleepPlan::default()
        };
        let mut state = SleepState {
            kicked_off: true,
            ..SleepState::default()
        };
        let mut obs = Observation {
            now: 50_000,
            busy: true,
            budget: Some(Budget {
                limit: 1.0,
                spent: 0.99,
                burn: 2.0,
            }),
            ..Observation::default()
        };
        let actions = tick(&plan, &mut state, &obs);
        assert!(matches!(actions[0], Action::CancelPaidRuns(_)));
        obs.now = 50_000 + plan.max_turn_secs as i64 * 1000;
        assert!(tick(&plan, &mut state, &obs).contains(&Action::Interrupt));
        obs.busy = false;
        obs.budget = None;
        obs.now = 200_000;
        let actions = tick(&plan, &mut state, &obs);
        assert_eq!(
            state.finished.as_ref().unwrap().reason,
            FinishReason::Deadline
        );
        assert!(matches!(&actions[0], Action::Nudge(text) if text.contains("finished")));
        assert!(tick(&plan, &mut state, &obs).is_empty());
    }

    #[test]
    fn plans_are_validated() {
        assert!(SleepPlan {
            gpu_budget_usd: -1.0,
            ..SleepPlan::default()
        }
        .validated()
        .is_err());
        assert!(SleepPlan {
            objective: Some(Objective {
                improve_pct: Some(0.0),
                ..objective(1.0)
            }),
            ..SleepPlan::default()
        }
        .validated()
        .is_err());
        assert!(SleepPlan::default().validated().is_ok());
    }

    #[test]
    fn written_targets_are_trimmed_bounded_and_shown_to_the_agent() {
        let plan = |goal: &str| SleepPlan {
            goal: Some(goal.to_string()),
            ..SleepPlan::default()
        };
        assert_eq!(plan("   ").validated().unwrap().goal, None);
        let plan = plan("  reproduce Table 2 within 1 point \n")
            .validated()
            .unwrap();
        assert_eq!(
            plan.goal.as_deref(),
            Some("reproduce Table 2 within 1 point")
        );
        assert!(SleepPlan {
            goal: Some("x".repeat(MAX_GOAL_CHARS + 1)),
            ..SleepPlan::default()
        }
        .validated()
        .is_err());
        let lines = status_lines(&plan, &SleepState::default(), None, None);
        assert!(lines.contains("Target (the user's words): reproduce Table 2 within 1 point"));
        assert!(lines.contains("Feasibility: not assessed yet"));
    }
}
