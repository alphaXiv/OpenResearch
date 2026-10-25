//! `orx sleep`: run a chat unattended (overnight) under a hard GPU budget.
//!
//!   orx sleep start --budget 20 [--for 8h] [--goal "…"] [--metric acc --target 15]
//!   orx sleep status                  plan, spend, progress, verdict
//!   orx sleep assess --verdict …      the agent's feasibility analysis
//!   orx sleep done --evidence "…"     the agent reached the target; finish
//!   orx sleep stop --reason "…"       finish early; paid launches stay blocked
//!   orx sleep off                     remove the plan (the user only)
//!
//! Every verb acts on the current chat (`ORX_CHAT_SESSION_ID`) unless
//! `--session` names one. An agent may start sleep mode when there is no plan
//! yet, but never loosen one: changing a plan and turning it off are the
//! user's, from a terminal or the dashboard. See `crate::local::sleep`.

use clap::{Args, Subcommand};

use crate::error::{anyhow, Result};
use crate::local::sleep::{
    self, Assessment, Budget, Direction, Finish, FinishReason, Objective, SleepPlan, Verdict,
};
use crate::store::{now_ms, Store};

#[derive(Debug, Args)]
pub struct SleepArgs {
    #[command(subcommand)]
    pub command: SleepCommand,
}

#[derive(Debug, Subcommand)]
pub enum SleepCommand {
    /// Start sleep mode, or change its plan (from a terminal).
    Start(StartArgs),
    /// Show the plan, the GPU spend, and progress toward the target.
    Status {
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Record whether the target is reachable within the budget and deadline.
    Assess {
        /// feasible, uncertain, or infeasible.
        #[arg(long)]
        verdict: String,
        /// The reasoning, in a few sentences.
        #[arg(long)]
        note: String,
        /// The most ambitious improvement (percent over the baseline) judged
        /// reachable, which then becomes the stop condition.
        #[arg(long)]
        revised_target: Option<f64>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Finish sleep mode because the target is reached. No paid run starts
    /// afterwards.
    Done {
        /// What was reached, with run ids and numbers.
        #[arg(long)]
        evidence: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Finish sleep mode early. No paid run starts afterwards.
    Stop {
        #[arg(long)]
        reason: String,
        #[arg(long)]
        session: Option<String>,
    },
    /// Remove the plan and its budget (not available to agents).
    Off {
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// Hard cap on GPU spend for agent-launched runs, in US dollars.
    #[arg(long)]
    pub budget: f64,
    /// Stop after this long: 90m, 8h, 1d.
    #[arg(long = "for")]
    pub duration: Option<String>,
    /// What you want done, in your own words. The agent judges when it is
    /// reached and then finishes sleep mode (`orx sleep done`).
    #[arg(long)]
    pub goal: Option<String>,
    /// The run metric the target is measured on (as imported with `orx exp metrics`).
    #[arg(long)]
    pub metric: Option<String>,
    /// max (higher is better) or min (lower is better).
    #[arg(long, default_value = "max", requires = "metric")]
    pub direction: String,
    /// Stop once the best run beats the baseline by this many percent.
    #[arg(long, requires = "metric")]
    pub target: Option<f64>,
    /// The baseline's value, when it was not measured in orx.
    #[arg(long, requires = "metric", conflicts_with = "baseline_run")]
    pub baseline: Option<f64>,
    /// The run whose metrics are the baseline (default: the newest finished
    /// run of a root experiment that reported the metric).
    #[arg(long, requires = "metric")]
    pub baseline_run: Option<String>,
    /// Wake the chat after it has been idle this long (default 5m).
    #[arg(long)]
    pub idle: Option<String>,
    /// Finished runs without a new best before a research round (default 3).
    #[arg(long)]
    pub stall_runs: Option<u32>,
    /// Interrupt a single turn after this long (default 2h).
    #[arg(long)]
    pub max_turn: Option<String>,
    /// Let the computer sleep as usual.
    #[arg(long)]
    pub no_keep_awake: bool,
    /// Keep the agent's strategy fixed: no retrospectives or trials
    /// (`orx strategy`).
    #[arg(long)]
    pub no_self_improve: bool,
    #[arg(long)]
    pub session: Option<String>,
}

fn session_id(explicit: Option<String>) -> Result<String> {
    explicit
        .or_else(crate::local::chat::launching_chat_session)
        .ok_or_else(|| anyhow!("Pass --session <chat id>; this is not an OpenResearch chat."))
}

fn seconds(value: &str, flag: &str) -> Result<u64> {
    crate::jobs::huggingface::parse_timeout(value)
        .map_err(|_| anyhow!("Bad {flag} '{value}': use e.g. 30m, 8h, 1d."))
}

fn from_agent() -> bool {
    crate::local::chat::in_local_session()
}

pub async fn run(args: SleepArgs) -> Result<()> {
    let store = Store::open()?;
    match args.command {
        SleepCommand::Start(args) => start(&store, args),
        SleepCommand::Status { session, json } => status(&store, &session_id(session)?, json),
        SleepCommand::Assess {
            verdict,
            note,
            revised_target,
            session,
        } => {
            let session = session_id(session)?;
            let mut stored = store
                .get_sleep(&session)?
                .ok_or_else(|| anyhow!("Sleep mode is not on for this chat."))?;
            let verdict = Verdict::from_id(&verdict)
                .ok_or_else(|| anyhow!("--verdict is feasible, uncertain, or infeasible."))?;
            if revised_target.is_some_and(|pct| !pct.is_finite() || pct <= 0.0) {
                return Err(anyhow!("--revised-target must be a positive percentage."));
            }
            if note.trim().is_empty() {
                return Err(anyhow!("Explain the verdict in --note."));
            }
            stored.state.assessment = Some(Assessment {
                verdict,
                note: note.trim().to_string(),
                revised_improve_pct: revised_target,
                at: now_ms(),
            });
            store.set_sleep_state(&session, &stored.state)?;
            println!("\u{2713} Feasibility recorded: {}.", verdict.id());
            status(&store, &session, false)
        }
        SleepCommand::Done { evidence, session } => {
            if evidence.trim().is_empty() {
                return Err(anyhow!(
                    "Say in --evidence what was reached, with run ids and numbers."
                ));
            }
            finish(
                &store,
                &session_id(session)?,
                FinishReason::GoalMet,
                format!("target reached: {}", evidence.trim()),
            )
        }
        SleepCommand::Stop { reason, session } => finish(
            &store,
            &session_id(session)?,
            FinishReason::Agent,
            format!("stopped: {}", reason.trim()),
        ),
        SleepCommand::Off { session } => {
            if from_agent() {
                return Err(anyhow!(
                    "Only the user can turn sleep mode off. Use `orx sleep stop --reason` to \
                     finish early."
                ));
            }
            let session = session_id(session)?;
            if store.delete_sleep(&session)? {
                println!("\u{2713} Sleep mode is off for this chat.");
            } else {
                println!("Sleep mode was not on for this chat.");
            }
            Ok(())
        }
    }
}

/// Finishes the plan unless it already has; the first reason stays on record.
fn finish(store: &Store, session: &str, reason: FinishReason, detail: String) -> Result<()> {
    let mut stored = store
        .get_sleep(session)?
        .ok_or_else(|| anyhow!("Sleep mode is not on for this chat."))?;
    if stored.state.finished.is_none() {
        stored.state.finished = Some(Finish {
            reason,
            detail,
            at: now_ms(),
        });
        // The caller is already writing the report.
        stored.state.finish_notified = from_agent();
        store.set_sleep_state(session, &stored.state)?;
    }
    println!(
        "\u{2713} Sleep mode finished. No paid run starts until the user turns it off \
         (`orx sleep off`)."
    );
    Ok(())
}

fn start(store: &Store, args: StartArgs) -> Result<()> {
    let session = session_id(args.session.clone())?;
    let chat = store
        .get_chat_session(&session)?
        .ok_or_else(|| anyhow!("Chat {session} was not found."))?;
    let existing = store.get_sleep(&session)?;
    if existing.is_some() && from_agent() {
        return Err(anyhow!(
            "Sleep mode is already on, and only the user can change its plan or budget."
        ));
    }
    let objective = args
        .metric
        .map(|metric| -> Result<Objective> {
            Ok(Objective {
                metric,
                direction: Direction::from_id(&args.direction)
                    .ok_or_else(|| anyhow!("--direction is max or min."))?,
                improve_pct: args.target,
                baseline_value: args.baseline,
                baseline_run: args.baseline_run,
            })
        })
        .transpose()?;
    let defaults = SleepPlan::default();
    let plan = SleepPlan {
        gpu_budget_usd: args.budget,
        goal: args.goal,
        until: args
            .duration
            .as_deref()
            .map(|d| seconds(d, "--for"))
            .transpose()?
            .map(|secs| now_ms() + secs as i64 * 1000),
        objective,
        idle_secs: match args.idle.as_deref() {
            Some(d) => seconds(d, "--idle")?,
            None => defaults.idle_secs,
        },
        stall_runs: args.stall_runs.unwrap_or(defaults.stall_runs),
        max_turn_secs: match args.max_turn.as_deref() {
            Some(d) => seconds(d, "--max-turn")?,
            None => defaults.max_turn_secs,
        },
        keep_awake: !args.no_keep_awake,
        self_improve: !args.no_self_improve,
    }
    .validated()?;
    if sleep::apply_plan(store, &session, &chat.project_id, &plan)? {
        println!("\u{2713} Sleep mode plan updated.");
    } else {
        println!(
            "\u{2713} Sleep mode is on. OpenResearch (`orx up`) must keep running; it wakes \
             the chat, enforces the budget, and keeps the computer awake."
        );
    }
    if sleep::chat_prompts(&chat) {
        eprintln!(
            "! This chat asks before some actions, and nobody will answer while you sleep; \
             a turn that waits is interrupted after the turn limit. Switch the chat to Auto \
             or Bypass permissions for an unattended night."
        );
    }
    status(store, &session, false)
}

fn status(store: &Store, session: &str, json: bool) -> Result<()> {
    let stored = store
        .get_sleep(session)?
        .ok_or_else(|| anyhow!("Sleep mode is not on for this chat."))?;
    let now = now_ms();
    let charges = store.sleep_charges(session)?;
    let budget = Budget::of(&stored.plan, &charges, now);
    let progress = sleep::progress_from_store(store, &stored)?;
    if json {
        let value = sleep::status_json(store, &stored)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    println!(
        "Sleep mode {} since {}",
        if stored.state.finished.is_some() {
            "finished"
        } else {
            "on"
        },
        sleep::format_time(stored.created_at)
    );
    println!(
        "  GPU     ${:.2} of ${:.2} used, ${:.2} left{}",
        budget.spent,
        budget.limit,
        budget.left(),
        if budget.burn > 0.0 {
            format!(", ${:.2}/h running", budget.burn)
        } else {
            String::new()
        }
    );
    if let Some(until) = stored.plan.until {
        println!("  until   {}", sleep::format_time(until));
    }
    if let Some(goal) = &stored.plan.goal {
        println!("  goal    {goal}");
    }
    if let (Some(objective), Some(progress)) = (&stored.plan.objective, &progress) {
        println!(
            "  target  {} {}{}",
            objective.direction.id(),
            objective.metric,
            progress
                .target_pct
                .map(|pct| format!(", {pct:+.1}% over the baseline"))
                .unwrap_or_default()
        );
        match (&progress.baseline, &progress.best) {
            (Some(b), Some(best)) => println!(
                "  result  baseline {:.6}, best {:.6} ({}, {:+.2}%), {} run(s) since the best",
                b.value,
                best.value,
                best.run_id,
                progress.improvement_pct.unwrap_or_default(),
                progress.runs_since_best
            ),
            (Some(b), None) => println!("  result  baseline {:.6}, no comparable run yet", b.value),
            (None, _) => println!("  result  no baseline measurement yet"),
        }
    }
    if let Some(a) = &stored.state.assessment {
        println!("  verdict {} — {}", a.verdict.id(), a.note);
    }
    if let Some(finish) = &stored.state.finished {
        println!("  done    {}", finish.detail);
    }
    if let Ok(lineup) = crate::local::strategy::lineup(store, &stored.project_id) {
        let mut line = format!("  strategy v{}", lineup.incumbent.version);
        match (&lineup.candidate, crate::local::strategy::active(&stored)) {
            (Some(c), true) => line.push_str(&format!(", v{} on trial", c.version)),
            (_, false) if !stored.plan.self_improve => line.push_str(", self-improvement off"),
            (_, false) => line.push_str(", self-improvement needs a metric to judge runs by"),
            _ => {}
        }
        println!("{line}  (orx strategy show)");
    }
    if stored.state.research_rounds > 0 || stored.state.interrupts > 0 {
        println!(
            "  watch   {} research round(s), {} interrupted turn(s)",
            stored.state.research_rounds, stored.state.interrupts
        );
    }
    Ok(())
}
