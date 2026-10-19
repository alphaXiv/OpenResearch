//! `orx strategy`: the versioned way of working that sleep mode improves.
//!
//!   orx strategy show [--version N] [--json]   the version in force, or one
//!   orx strategy stats [--runs]                each version's judged runs
//!   orx strategy log                           every proposal and decision
//!   orx strategy diff [--from A] [--to B]      what a version changed
//!   orx strategy propose --rationale … --file  open a trial (agents too)
//!   orx strategy keep --reason …               close a retrospective unchanged
//!   orx strategy withdraw --reason …           close the open trial
//!   orx strategy promote | reject | rollback   the user's own decisions
//!
//! Every verb acts on the current chat's project unless `--project` names one.
//! Agents can propose, keep, and withdraw; only orx's trial or the user puts
//! a version in force. See `crate::local::strategy`.

use std::io::Read;

use clap::{Args, Subcommand};

use crate::error::{anyhow, Result};
use crate::local::strategy::{self, VersionStatus};
use crate::store::{now_ms, Store};

#[derive(Debug, Args)]
pub struct StrategyArgs {
    #[command(subcommand)]
    pub command: StrategyCommand,
    /// The project (default: the current chat's).
    #[arg(long, global = true)]
    pub project: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum StrategyCommand {
    /// Show the strategy in force, the one on trial, or any version.
    Show {
        #[arg(long)]
        version: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Each version's runs: wins, losses, pending, and spend.
    Stats {
        /// List every attributed run with its outcome.
        #[arg(long)]
        runs: bool,
        #[arg(long)]
        json: bool,
    },
    /// The change log: proposals, trials, promotions, rejections, rollbacks.
    Log {
        #[arg(long)]
        json: bool,
    },
    /// Line diff between two versions (default: the newest against its parent).
    Diff {
        #[arg(long)]
        from: Option<u32>,
        #[arg(long)]
        to: Option<u32>,
    },
    /// Propose a revision; orx trials it against the version in force.
    Propose {
        /// JSON with any of `approach`, `research`, `retrospective`, `knobs`;
        /// `-` reads stdin. Omitted fields keep the current values.
        #[arg(long)]
        file: String,
        /// The evidence behind the change.
        #[arg(long)]
        rationale: String,
    },
    /// Close a due retrospective without changing the strategy.
    Keep {
        #[arg(long)]
        reason: String,
    },
    /// Withdraw the strategy on trial.
    Withdraw {
        #[arg(long)]
        reason: String,
    },
    /// Put the strategy on trial in force now (not available to agents).
    Promote,
    /// Reject the strategy on trial (not available to agents).
    Reject {
        #[arg(long)]
        reason: Option<String>,
    },
    /// Put an earlier version back in force (not available to agents).
    Rollback {
        /// Default: the version the one in force replaced.
        #[arg(long)]
        to: Option<u32>,
    },
}

fn from_agent() -> bool {
    crate::local::chat::in_local_session()
}

fn user_only(verb: &str) -> Result<()> {
    if from_agent() {
        return Err(anyhow!(
            "Only the user can {verb} a strategy. orx puts a candidate in force when its trial \
             shows it does better."
        ));
    }
    Ok(())
}

fn project_id(store: &Store, explicit: Option<String>) -> Result<String> {
    if let Some(project) = explicit {
        return Ok(project);
    }
    let session = crate::local::chat::launching_chat_session()
        .ok_or_else(|| anyhow!("Pass --project <id>; this is not an OpenResearch chat."))?;
    Ok(store
        .get_chat_session(&session)?
        .ok_or_else(|| anyhow!("Chat {session} was not found."))?
        .project_id)
}

/// The sleep plan whose runs are judged: the current chat's, else any in the
/// project.
fn sleep_for(store: &Store, project: &str) -> Result<Option<crate::store::sleep::StoredSleep>> {
    let session = crate::local::chat::launching_chat_session().unwrap_or_default();
    crate::local::sleep::governing_plan(store, project, &session)
}

fn author() -> &'static str {
    if from_agent() {
        "agent"
    } else {
        "user"
    }
}

pub async fn run(args: StrategyArgs) -> Result<()> {
    let store = Store::open()?;
    let project = project_id(&store, args.project)?;
    match args.command {
        StrategyCommand::Show { version, json } => show(&store, &project, version, json),
        StrategyCommand::Stats { runs, json } => stats(&store, &project, runs, json),
        StrategyCommand::Log { json } => log(&store, &project, json),
        StrategyCommand::Diff { from, to } => diff(&store, &project, from, to),
        StrategyCommand::Propose { file, rationale } => {
            let text = if file == "-" {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text)?;
                text
            } else {
                std::fs::read_to_string(&file)
                    .map_err(|err| anyhow!("Could not read {file}: {err}"))?
            };
            let change: serde_json::Value = serde_json::from_str(&text)
                .map_err(|err| anyhow!("The strategy change is not JSON: {err}"))?;
            let base = strategy::lineup(&store, &project)?.incumbent;
            let candidate = strategy::merged(&base.strategy, change)?;
            let version = strategy::propose(&store, &project, &candidate, &rationale, author())?;
            println!(
                "\u{2713} Strategy v{version} is on trial against v{}. Turns now alternate \
                 between them; orx keeps v{version} only if its runs win more often.",
                base.version
            );
            println!(
                "{}",
                strategy::diff(
                    &strategy::as_text(&base.strategy),
                    &strategy::as_text(&candidate)
                )
            );
            Ok(())
        }
        StrategyCommand::Keep { reason } => {
            let reason = reason.trim();
            if reason.is_empty() {
                return Err(anyhow!("Explain why the strategy stays in --reason."));
            }
            let lineup = strategy::lineup(&store, &project)?;
            if lineup.candidate.is_some() {
                return Err(anyhow!(
                    "A trial is open; there is no retrospective to close."
                ));
            }
            store.log_strategy_event(
                &project,
                "kept",
                lineup.incumbent.version,
                &format!("by {}: {reason}", author()),
            )?;
            println!(
                "\u{2713} Strategy v{} stays in force.",
                lineup.incumbent.version
            );
            Ok(())
        }
        StrategyCommand::Withdraw { reason } => {
            let note = format!("withdrawn by {}: {}", author(), reason.trim());
            match strategy::close_candidate(&store, &project, VersionStatus::Withdrawn, &note)? {
                Some(v) => println!("\u{2713} Strategy v{v} withdrawn."),
                None => println!("No strategy is on trial."),
            }
            Ok(())
        }
        StrategyCommand::Promote => {
            user_only("promote")?;
            let v = strategy::promote(&store, &project, "promoted by the user")?;
            println!("\u{2713} Strategy v{v} is in force.");
            Ok(())
        }
        StrategyCommand::Reject { reason } => {
            user_only("reject")?;
            let note = match reason
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
            {
                Some(reason) => format!("rejected by the user: {reason}"),
                None => "rejected by the user".to_string(),
            };
            match strategy::close_candidate(&store, &project, VersionStatus::Rejected, &note)? {
                Some(v) => println!("\u{2713} Strategy v{v} rejected."),
                None => println!("No strategy is on trial."),
            }
            Ok(())
        }
        StrategyCommand::Rollback { to } => {
            user_only("roll back")?;
            let v = strategy::rollback(&store, &project, to, "user")?;
            println!("\u{2713} Strategy v{v} is in force (a copy of the earlier version).");
            Ok(())
        }
    }
}

fn show(store: &Store, project: &str, version: Option<u32>, json: bool) -> Result<()> {
    let lineup = strategy::lineup(store, project)?;
    if json {
        let value = match sleep_for(store, project)? {
            Some(sleep) if version.is_none() => strategy::status_json(store, &sleep)?,
            _ => serde_json::Value::Null,
        };
        let shown = match version {
            Some(v) => store
                .get_strategy_version(project, v)?
                .ok_or_else(|| anyhow!("There is no v{v}."))?,
            None => lineup.incumbent.clone(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "version": shown.version,
                "status": shown.status.id(),
                "strategy": shown.strategy,
                "trial": value,
            }))?
        );
        return Ok(());
    }
    let shown = match version {
        Some(v) => store
            .get_strategy_version(project, v)?
            .ok_or_else(|| anyhow!("There is no v{v}."))?,
        None => lineup.incumbent.clone(),
    };
    println!(
        "Strategy v{} ({}, by {}{})",
        shown.version,
        shown.status.id(),
        shown.author,
        shown
            .parent
            .map(|p| format!(", from v{p}"))
            .unwrap_or_default()
    );
    if !shown.rationale.is_empty() {
        println!("Rationale: {}", shown.rationale);
    }
    if let Some(decision) = &shown.decision {
        println!("Decision: {decision}");
    }
    println!();
    println!("{}", strategy::as_text(&shown.strategy));
    if version.is_none() {
        if let Some(candidate) = &lineup.candidate {
            println!(
                "\nv{} is on trial (`orx strategy show --version {}`, `orx strategy diff`).",
                candidate.version, candidate.version
            );
        }
    }
    Ok(())
}

fn stats(store: &Store, project: &str, runs: bool, json: bool) -> Result<()> {
    let Some(sleep) = sleep_for(store, project)? else {
        return Err(anyhow!(
            "Runs are judged against a sleep-mode objective, and no sleep plan exists here."
        ));
    };
    let lineup = strategy::lineup(store, project)?;
    let Some(board) = strategy::scoreboard(store, &sleep, &lineup, now_ms())? else {
        return Err(anyhow!(
            "This sleep plan has no target metric, so runs cannot be judged."
        ));
    };
    if json {
        let value = serde_json::json!({
            "versions": board.by_version.iter().map(|(v, s)| serde_json::json!({
                "version": v,
                "stats": s,
                "smoothedRate": s.smoothed_rate(),
            })).collect::<Vec<_>>(),
            "control": lineup.candidate.as_ref().map(|_| board.control),
            "runs": board.runs.iter().map(|r| serde_json::json!({
                "runId": r.run_id,
                "version": r.version,
                "outcome": board.outcomes.get(&r.run_id).map(|o| o.id()),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if board.by_version.is_empty() {
        println!("No agent-launched runs are attributed to a strategy yet.");
    }
    for (version, s) in &board.by_version {
        let role = if *version == lineup.incumbent.version {
            " (in force)"
        } else if lineup
            .candidate
            .as_ref()
            .is_some_and(|c| c.version == *version)
        {
            " (on trial)"
        } else {
            ""
        };
        println!(
            "v{version}{role}: {} launched, {} won, {} lost, {} pending, ${:.2}; win rate {:.0}% \
             (smoothed)",
            s.launched,
            s.wins,
            s.losses,
            s.pending,
            s.spend_usd,
            s.smoothed_rate() * 100.0
        );
    }
    if let Some(candidate) = &lineup.candidate {
        let c = board.control;
        println!(
            "Trial of v{}: v{} during the trial {} won, {} lost; it needs {} judged runs to \
             decide early, and is decided at {}.",
            candidate.version,
            lineup.incumbent.version,
            c.wins,
            c.losses,
            strategy::MIN_TRIAL_RUNS,
            strategy::MAX_TRIAL_RUNS
        );
    }
    if runs {
        for r in &board.runs {
            println!(
                "  {}  v{}  {}",
                r.run_id,
                r.version,
                board.outcomes.get(&r.run_id).map_or("unknown", |o| o.id())
            );
        }
    }
    Ok(())
}

fn log(store: &Store, project: &str, json: bool) -> Result<()> {
    strategy::versions(store, project)?;
    let events = store.strategy_events(project)?;
    if json {
        let value: Vec<_> = events
            .iter()
            .map(|e| {
                serde_json::json!({ "at": e.at, "kind": e.kind, "version": e.version, "detail": e.detail })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    for e in events {
        println!(
            "{}  v{:<3} {:<12} {}",
            crate::local::sleep::format_time(e.at),
            e.version,
            e.kind,
            e.detail
        );
    }
    Ok(())
}

fn diff(store: &Store, project: &str, from: Option<u32>, to: Option<u32>) -> Result<()> {
    let all = strategy::versions(store, project)?;
    let to = match to {
        Some(v) => all.iter().find(|p| p.version == v),
        None => all.last(),
    }
    .ok_or_else(|| anyhow!("There is no such version."))?;
    let from = match from.or(to.parent) {
        Some(v) => all
            .iter()
            .find(|p| p.version == v)
            .ok_or_else(|| anyhow!("There is no v{v}."))?,
        None => {
            println!("v{} is the first version.", to.version);
            return Ok(());
        }
    };
    println!("--- v{}\n+++ v{}", from.version, to.version);
    println!(
        "{}",
        strategy::diff(
            &strategy::as_text(&from.strategy),
            &strategy::as_text(&to.strategy)
        )
    );
    Ok(())
}
