//! What OpenResearch's chats and runs spent: agent cost per day, and compute
//! cost from what the provider billed, else from the price each run was quoted.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use crate::error::Result;
use crate::jobs::modal;
use crate::local::usage::{AgentSpend, BackendSpend, ChatCompute, PlanUsage, UsageReport};
use crate::store::{BilledRun, Cost, Store};

/// Backends that bill for running time and have a price to read. The user's own machines and
/// clusters cost nothing orx can see; Vercel and Tinker have no per-run price.
pub(crate) const BILLED: [&str; 4] = ["openresearch_job", "hf_job", "modal_job", "boxd_job"];

/// Billed backends whose price is a list price, still readable after a run ends; a box's price
/// went with the box.
pub(crate) const LIST_PRICED: [&str; 3] = ["hf_job", "modal_job", "boxd_job"];

const HOUR: i64 = 3_600_000;
/// How long after an hour ends Modal's report is read for it, since it can lag.
const BILL_SETTLE: i64 = HOUR;
/// The longest span Modal's hourly report covers.
const REPORT_SPAN: i64 = 7 * 24 * HOUR;
/// Runs that ended longer ago keep their estimate, since a sandbox runs at most a day.
const BILL_WINDOW: i64 = REPORT_SPAN - 24 * HOUR;

/// The currency of a row with nothing priced in it: every price source orx reads is in dollars.
const UNPRICED_CURRENCY: &str = "USD";

/// The seven days up to `now`'s and what was spent in them; the plans and the chat are the
/// caller's to fill. Agent spend leaves out accounts on one of `plans`, whose cost is only
/// what the tokens would have cost at API prices, and is empty while `plans` are unknown.
pub fn seven_days(store: &Store, now: i64, plans: Option<&[PlanUsage]>) -> Result<UsageReport> {
    let mut days = Vec::new();
    let mut midnights = Vec::new();
    for offset in -6..=1 {
        let (day, midnight) = store.local_day(now, offset)?;
        days.push(day);
        midnights.push(midnight);
    }
    days.pop();
    let (from, to) = (midnights[0], midnights[7]);

    let mut agent_days: BTreeMap<(String, String), [u64; 7]> = BTreeMap::new();
    let mut unpriced_days: BTreeSet<(String, usize)> = BTreeSet::new();
    let billed = store.spent_samples(from, to)?.into_iter().filter(|sample| {
        plans.is_some_and(|plans| {
            !plans
                .iter()
                .any(|plan| plan.harness == sample.harness && plan.profile == sample.profile)
        })
    });
    for sample in billed {
        let Some(day) = days.iter().position(|day| *day == sample.day) else {
            continue;
        };
        match sample.cost {
            Some(cost) => {
                agent_days
                    .entry((sample.harness, cost.currency))
                    .or_default()[day] += cost.micros;
            }
            None if sample.usage.total().is_some_and(|total| total > 0) => {
                unpriced_days.insert((sample.harness, day));
            }
            None => {}
        }
    }
    for (harness, _) in &unpriced_days {
        if !agent_days.keys().any(|(priced, _)| priced == harness) {
            agent_days.insert((harness.clone(), UNPRICED_CURRENCY.into()), [0; 7]);
        }
    }
    let mut agents: Vec<AgentSpend> = agent_days
        .into_iter()
        .map(|((harness, currency), micros)| AgentSpend {
            unpriced: unpriced_days
                .iter()
                .filter(|(unpriced, _)| *unpriced == harness)
                .count() as u32,
            day_micros: micros.to_vec(),
            harness,
            currency,
        })
        .collect();
    agents.sort_by_key(|agent| std::cmp::Reverse(agent.day_micros.iter().sum::<u64>()));

    let mut backends: BTreeMap<(String, String), [u64; 7]> = BTreeMap::new();
    let mut unpriced: BTreeMap<String, u32> = BTreeMap::new();
    for run in store.billed_runs(&BILLED, from, None)? {
        let running = running_time(&run, now);
        let Some((start, end)) = running.or(run.ended_at.map(|end| (end, end))) else {
            continue;
        };
        if start >= to || end <= from {
            continue;
        }
        let (currency, micros) = match (&run.billed, running, &run.quote) {
            (Some(billed), ..) => (billed.currency.clone(), billed.micros),
            (None, Some(_), Some(quote)) => (quote.currency.clone(), cost(quote, end - start)),
            _ => {
                *unpriced.entry(run.backend).or_default() += 1;
                continue;
            }
        };
        let row = backends.entry((run.backend, currency)).or_default();
        for (day, spent) in row.iter_mut().enumerate() {
            *spent += share(micros, (start, end), (midnights[day], midnights[day + 1]));
        }
    }
    let mut compute: Vec<BackendSpend> = backends
        .into_iter()
        .map(|((backend, currency), micros)| BackendSpend {
            backend,
            currency,
            day_micros: micros.to_vec(),
            unpriced: 0,
        })
        .collect();
    for (backend, count) in unpriced {
        match compute.iter_mut().find(|row| row.backend == backend) {
            Some(row) => row.unpriced = count,
            None => compute.push(BackendSpend {
                backend,
                currency: UNPRICED_CURRENCY.into(),
                day_micros: vec![0; 7],
                unpriced: count,
            }),
        }
    }
    compute.sort_by_key(|row| std::cmp::Reverse(row.day_micros.iter().sum::<u64>()));

    Ok(UsageReport {
        days,
        agents,
        compute,
        ..Default::default()
    })
}

/// The runs `chat_id` launched on backends that bill, over their whole running time.
pub fn chat_compute(store: &Store, chat_id: &str, now: i64) -> Result<Option<ChatCompute>> {
    let runs = store.billed_runs(&BILLED, i64::MIN, Some(chat_id))?;
    if runs.is_empty() {
        return Ok(None);
    }
    let mut spent: BTreeMap<String, u64> = BTreeMap::new();
    let mut unpriced = 0;
    for run in &runs {
        match (&run.billed, running_time(run, now), &run.quote) {
            (Some(billed), ..) => {
                *spent.entry(billed.currency.clone()).or_default() += billed.micros
            }
            (None, Some((start, end)), Some(quote)) => {
                *spent.entry(quote.currency.clone()).or_default() += cost(quote, end - start)
            }
            (None, None, _) if !run.finished => {}
            _ => unpriced += 1,
        }
    }
    // The contract carries one currency; runs priced in another are left out of the sum.
    let largest = spent.into_iter().max_by_key(|(_, micros)| *micros);
    Ok(Some(ChatCompute {
        runs: runs.len() as u32,
        cost_micros: largest.as_ref().map_or(0, |(_, micros)| *micros),
        currency: largest.map_or_else(|| UNPRICED_CURRENCY.into(), |(currency, _)| currency),
        running: runs.iter().any(|run| !run.finished),
        unpriced,
    }))
}

/// From the payload's start to its end, or to now while it runs; None before it starts, and for
/// a run first seen after it finished, whose running time is unknown.
fn running_time(run: &BilledRun, now: i64) -> Option<(i64, i64)> {
    let start = run.started_at?;
    let end = if run.finished { run.ended_at? } else { now };
    Some((start, end.max(start)))
}

/// The part of `micros`, spent evenly over `[start, end)`, that falls in `[from, to)`; all of it
/// at `start` when the run took no time orx saw.
fn share(micros: u64, (start, end): (i64, i64), (from, to): (i64, i64)) -> u64 {
    if start == end {
        return if (from..to).contains(&start) {
            micros
        } else {
            0
        };
    }
    let overlap = (end.min(to) - start.max(from)).max(0) as u128;
    let span = (end - start) as u128;
    ((u128::from(micros) * overlap + span / 2) / span) as u64
}

/// Reads what Modal billed each finished run that had an app of its own, once Modal has
/// settled its last hour. A run whose app the report doesn't name, as when the token now reaches
/// another workspace, keeps its estimate.
pub(crate) async fn read_modal_bills(data_dir: &Path, now: i64) -> Result<()> {
    let settled = (now - BILL_SETTLE).div_euclid(HOUR) * HOUR;
    let runs: Vec<_> = Store::open_at(data_dir.to_path_buf())?
        .unbilled_runs("modal_job", now - BILL_WINDOW)?
        .into_iter()
        .filter(|run| run.ended_at < settled && run.app == Some(modal::run_app(&run.id)))
        .collect();
    let Some(since) = runs.iter().map(|run| run.created_at).min() else {
        return Ok(());
    };
    let billed = modal::billed_by_app(since.max(now - REPORT_SPAN + HOUR)).await?;
    let store = Store::open_at(data_dir.to_path_buf())?;
    for run in runs {
        let Some(&micros) = run.app.and_then(|app| billed.get(&app)) else {
            continue;
        };
        let cost = Cost {
            micros,
            currency: "USD".into(),
        };
        store.record_run_bill(&run.id, &cost)?;
    }
    Ok(())
}

/// Reads Modal's bills while `orx up` runs; a plan without billing reports keeps estimates.
pub(crate) async fn keep_modal_bills(data_dir: std::path::PathBuf) {
    loop {
        let wait = match read_modal_bills(&data_dir, crate::store::now_ms()).await {
            Ok(()) => Duration::from_secs(15 * 60),
            Err(error) => {
                eprintln!("orx up: could not read what Modal billed: {error:#}");
                Duration::from_secs(6 * 60 * 60)
            }
        };
        tokio::time::sleep(crate::config::paced(wait)).await;
    }
}

/// What `millis` of running time costs at `quote`, in micros.
pub(crate) fn cost(quote: &crate::store::Quote, millis: i64) -> u64 {
    let micros = u128::from(quote.amount_micros) * millis.max(0) as u128;
    let per = u128::from(quote.per.millis());
    ((micros + per / 2) / per) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// 2026-11-13 12:00 UTC, 07:00 on the 13th five hours west of UTC.
    const NOW: i64 = 1_794_571_200_000;
    const HOUR: i64 = 3_600_000;
    const DAY: i64 = 24 * HOUR;
    /// Midnight starting 2026-11-13 in UTC−5.
    const TODAY: i64 = 1_794_546_000_000;

    /// A store whose rows the test writes as raw SQL, in a process five hours west of UTC.
    fn store_west_of_utc(name: &str) -> Option<(std::path::PathBuf, Store, rusqlite::Connection)> {
        if !crate::config::in_test_child(&["TZ"]) {
            return None;
        }
        std::env::set_var("TZ", "EST5");
        let dir = std::env::temp_dir().join(format!("orx-spend-{name}-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let conn = rusqlite::Connection::open(dir.join("orx.db")).unwrap();
        Some((dir, store, conn))
    }

    /// Micros and currency.
    type Money<'a> = Option<(i64, &'a str)>;

    /// Each `(harness, recorded at, tokens, cost)` as a sample of a turn of its own.
    fn samples(conn: &rusqlite::Connection, samples: &[(&str, i64, u64, Money)]) {
        for (harness, at, tokens, cost) in samples {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO chat_usage_executions (execution_id, turn_id, harness, report_id, suppressed)
                 VALUES (?1, ?1, ?2, 'r', 1)",
                params![id, harness],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_usage_samples (execution_id, sample_id, harness, usage_json, cost_micros, cost_currency, recorded_at)
                 VALUES (?1, 's', ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    harness,
                    serde_json::json!({"inputTokens": tokens, "outputTokens": 0}).to_string(),
                    cost.map(|(micros, _)| micros),
                    cost.map(|(_, currency)| currency),
                    at
                ],
            )
            .unwrap();
        }
    }

    enum Span {
        Queued,
        Running(i64),
        Done(i64, i64),
        /// Finished when orx first saw it, at this time.
        DoneUnseen(i64),
    }
    use Span::*;

    /// Micros per unit of running time.
    type Price = Option<(u64, &'static str)>;
    const HOURLY: Price = Some((1_000_000, "hour"));
    const MINUTELY: Price = Some((1_000, "minute"));

    /// Each `(backend, chat, span, price)` as a run.
    fn runs(conn: &rusqlite::Connection, runs: &[(&str, &str, Span, Price)]) {
        for (backend, chat, span, price) in runs {
            let (status, started, ended) = match *span {
                Queued => ("starting", None, None),
                Running(started) => ("running", Some(started), None),
                Done(started, ended) => ("done", Some(started), Some(ended)),
                DoneUnseen(ended) => ("done", None, Some(ended)),
            };
            let quote = price.map(|(micros, per)| {
                serde_json::json!({"amountMicros": micros, "per": per, "currency": "USD",
                                   "source": "test", "quotedAt": 1})
                .to_string()
            });
            conn.execute(
                "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at,
                                   ended_at, chat_session_id, launch_spec, started_at, price_json)
                 VALUES (?1, 'e', 'p', ?2, '{\"kind\":\"engine\"}', 1, COALESCE(?4, 1), ?4, ?5, ?6, ?3, ?7)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    status,
                    started,
                    ended,
                    chat,
                    serde_json::json!({"kind": backend, "options": {}}).to_string(),
                    quote
                ],
            )
            .unwrap();
        }
    }

    #[test]
    fn seven_days_split_at_the_machines_midnight() {
        let Some((dir, store, conn)) = store_west_of_utc("days") else {
            return;
        };
        let minute_before = TODAY - 60_000;
        let minute_after = TODAY + 60_000;
        samples(
            &conn,
            &[
                ("claude-code", minute_before, 10, Some((1_000, "USD"))),
                ("claude-code", minute_after, 10, Some((2_000, "USD"))),
                ("claude-code", minute_after, 10, Some((7, "EUR"))),
                ("codex", TODAY - 2 * DAY, 10, Some((300, "USD"))),
                ("codex", minute_after, 10, None),
                // Before the week, and a model seen without tokens: neither counts.
                ("claude-code", TODAY - 7 * DAY, 10, Some((9, "USD"))),
                ("cursor", minute_after, 0, None),
            ],
        );
        let (first, old) = (TODAY - 6 * DAY, TODAY - 8 * DAY);
        runs(
            &conn,
            &[
                // $1 an hour from 23:00 yesterday to 01:00 today.
                ("hf_job", "chat", Done(TODAY - HOUR, TODAY + HOUR), HOURLY),
                // A tenth of a cent a minute, running for the last hour.
                ("modal_job", "chat", Running(NOW - HOUR), MINUTELY),
                // Running into the week from before it.
                ("hf_job", "other", Done(first - HOUR, first + HOUR), HOURLY),
                ("openresearch_job", "chat", Running(NOW - HOUR), None),
                ("openresearch_job", "chat", Queued, None),
                ("ssh_job", "chat", Done(NOW - HOUR, NOW), None),
                ("hf_job", "chat", Done(old - DAY, old), HOURLY),
            ],
        );

        let week = seven_days(&store, NOW, Some(&[])).unwrap();
        assert_eq!(
            week.days,
            [
                "2026-11-07",
                "2026-11-08",
                "2026-11-09",
                "2026-11-10",
                "2026-11-11",
                "2026-11-12",
                "2026-11-13"
            ]
        );
        let agent = |harness: &str, currency: &str| {
            let agent = week
                .agents
                .iter()
                .find(|agent| agent.harness == harness && agent.currency == currency)
                .unwrap_or_else(|| panic!("no {harness} {currency}"));
            (agent.day_micros.clone(), agent.unpriced)
        };
        assert_eq!(
            agent("claude-code", "USD"),
            (vec![0, 0, 0, 0, 0, 1_000, 2_000], 0)
        );
        assert_eq!(agent("claude-code", "EUR"), (vec![0, 0, 0, 0, 0, 0, 7], 0));
        // Today's Codex tokens have no price: the day keeps what was priced and is counted.
        assert_eq!(agent("codex", "USD"), (vec![0, 0, 0, 0, 300, 0, 0], 1));
        assert_eq!(week.agents.len(), 3);

        let backend = |name: &str| {
            week.compute
                .iter()
                .find(|row| row.backend == name)
                .unwrap_or_else(|| panic!("no {name}"))
        };
        assert_eq!(
            backend("hf_job").day_micros,
            [1_000_000, 0, 0, 0, 0, 1_000_000, 1_000_000]
        );
        assert_eq!(backend("modal_job").day_micros, [0, 0, 0, 0, 0, 0, 60_000]);
        let boxes = backend("openresearch_job");
        assert_eq!(
            (boxes.day_micros.as_slice(), boxes.unpriced),
            (&[0; 7][..], 1)
        );
        assert_eq!(week.compute.len(), 3);

        drop((store, conn));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_chats_compute_counts_the_runs_it_launched_that_bill() {
        let Some((dir, store, conn)) = store_west_of_utc("chat") else {
            return;
        };
        runs(
            &conn,
            &[
                ("hf_job", "chat", Done(NOW - 2 * HOUR, NOW - HOUR), HOURLY),
                ("modal_job", "chat", Running(NOW - HOUR), MINUTELY),
                ("openresearch_job", "chat", Done(NOW - HOUR, NOW), None),
                // Vercel has no per-run price: never "cost unavailable".
                ("vercel_job", "chat", Done(NOW - HOUR, NOW), None),
                ("ssh_job", "chat", Running(NOW - HOUR), None),
                ("hf_job", "other", Running(NOW - HOUR), HOURLY),
            ],
        );

        let compute = chat_compute(&store, "chat", NOW).unwrap().unwrap();
        assert_eq!(
            compute,
            ChatCompute {
                runs: 3,
                cost_micros: 1_060_000,
                currency: "USD".into(),
                running: true,
                unpriced: 1,
            }
        );
        assert_eq!(chat_compute(&store, "quiet", NOW).unwrap(), None);
        drop((store, conn));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// What the provider billed replaces the estimate, and prices a run orx never saw running.
    #[test]
    fn a_billed_run_counts_what_was_billed() {
        let Some((dir, store, conn)) = store_west_of_utc("billed") else {
            return;
        };
        runs(
            &conn,
            &[
                (
                    "modal_job",
                    "chat",
                    Done(TODAY - HOUR, TODAY + HOUR),
                    HOURLY,
                ),
                ("modal_job", "chat", DoneUnseen(TODAY + 2 * HOUR), None),
                ("modal_job", "chat", DoneUnseen(TODAY + 3 * HOUR), None),
            ],
        );
        for (ended, micros) in [(TODAY + HOUR, 3_000_000), (TODAY + 2 * HOUR, 500_000)] {
            conn.execute(
                "UPDATE runs SET billed_json = ?2 WHERE ended_at = ?1",
                params![
                    ended,
                    serde_json::json!({"micros": micros, "currency": "USD"}).to_string()
                ],
            )
            .unwrap();
        }

        let week = seven_days(&store, NOW, Some(&[])).unwrap();
        assert_eq!(week.compute.len(), 1);
        assert_eq!(
            week.compute[0].day_micros,
            [0, 0, 0, 0, 0, 1_500_000, 2_000_000]
        );
        assert_eq!(week.compute[0].unpriced, 1);
        let compute = chat_compute(&store, "chat", NOW).unwrap().unwrap();
        assert_eq!((compute.cost_micros, compute.unpriced), (3_500_000, 1));
        drop((store, conn));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A run first seen after it finished ran for a time orx never saw: never free, never dropped.
    #[test]
    fn a_billed_run_first_seen_finished_is_unpriced() {
        let Some((dir, store, conn)) = store_west_of_utc("unseen") else {
            return;
        };
        runs(
            &conn,
            &[
                ("hf_job", "chat", Done(NOW - 2 * HOUR, NOW - HOUR), HOURLY),
                ("hf_job", "chat", DoneUnseen(NOW - HOUR), HOURLY),
                ("modal_job", "chat", DoneUnseen(NOW - HOUR), MINUTELY),
                ("modal_job", "chat", DoneUnseen(TODAY - 8 * DAY), MINUTELY),
            ],
        );

        let week = seven_days(&store, NOW, Some(&[])).unwrap();
        let backend = |name: &str| {
            let row = week.compute.iter().find(|row| row.backend == name).unwrap();
            (row.day_micros.iter().sum::<u64>(), row.unpriced)
        };
        assert_eq!(backend("hf_job"), (1_000_000, 1));
        assert_eq!(backend("modal_job"), (0, 1));
        let compute = chat_compute(&store, "chat", NOW).unwrap().unwrap();
        assert_eq!(
            (compute.runs, compute.cost_micros, compute.unpriced),
            (4, 1_000_000, 3)
        );
        drop((store, conn));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn plan(harness: &str, profile: Option<&str>) -> PlanUsage {
        PlanUsage {
            harness: harness.into(),
            profile: profile.map(str::to_string),
            plan: None,
            account: None,
            windows: Vec::new(),
            credits: None,
            extra_usage: None,
            resets: None,
            read_at: None,
            error: None,
        }
    }

    #[test]
    fn agent_spend_leaves_out_accounts_on_a_plan() {
        let Some((dir, store, conn)) = store_west_of_utc("plans") else {
            return;
        };
        // (chat, its agent and profile now, the turn's execution, the agent that spent, micros)
        let spent = [
            ("own", "claude-code", None, "own", "claude-code", 1_000),
            (
                "api",
                "claude-code",
                Some("claude-api"),
                "api",
                "claude-code",
                20,
            ),
            (
                "switched",
                "codex",
                Some("codex-plan"),
                "switched",
                "claude-code",
                300,
            ),
            (
                "late",
                "codex",
                Some("codex-plan"),
                "codex-late:late:child:t",
                "codex",
                4_000,
            ),
        ];
        for (chat, harness, profile, execution, spender, micros) in spent {
            conn.execute(
                "INSERT INTO chat_sessions (id, project_id, harness, profile_id, created_at, updated_at)
                 VALUES (?1, 'p', ?2, ?3, 1, 1)",
                params![chat, harness, profile],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_turns (id, session_id, assistant_message_id, client_turn_id, request_hash,
                                         prepared_input, settings_json, state, delivery_state, created_at, updated_at)
                 VALUES (?1, ?1, 'a', 'c', 'h', '', '{}', 'completed', 'delivered', ?2, 1)",
                params![chat, NOW],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_usage_executions (execution_id, turn_id, harness, report_id, suppressed)
                 VALUES (?1, ?1, ?2, 'r', 1)",
                params![execution, spender],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chat_usage_samples (execution_id, sample_id, harness, usage_json, cost_micros, cost_currency, recorded_at)
                 VALUES (?1, 's', ?2, '{\"inputTokens\":10,\"outputTokens\":0}', ?3, 'USD', ?4)",
                params![execution, spender, micros, NOW],
            )
            .unwrap();
        }
        let total = |plans: Option<&[PlanUsage]>| {
            let week = seven_days(&store, NOW, plans).unwrap();
            week.agents
                .iter()
                .map(|agent| (agent.harness.clone(), agent.day_micros.iter().sum::<u64>()))
                .collect::<Vec<_>>()
        };

        let plans = [plan("claude-code", None), plan("codex", Some("codex-plan"))];
        assert_eq!(
            total(Some(&plans)),
            [("claude-code".to_string(), 20)],
            "only the API key profile's spend; a switched-away agent was the user's own"
        );
        assert_eq!(
            total(Some(&[])),
            [
                ("codex".to_string(), 4_000),
                ("claude-code".to_string(), 1_320)
            ]
        );
        assert!(
            total(None).is_empty(),
            "while the plans are unread, no spend reads as billed"
        );
        drop((store, conn));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// What 0.2.27 recorded reads as spend whose price is unknown, on its turn's day.
    #[test]
    fn a_store_from_0_2_27_reads_as_unpriced_history() {
        if !crate::config::in_test_child(&["TZ"]) {
            return;
        }
        std::env::set_var("TZ", "EST5");
        let dir = std::env::temp_dir().join(format!("orx-spend-0227-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = rusqlite::Connection::open(dir.join("orx.db")).unwrap();
        old.execute_batch(include_str!("../../tests/contracts/store-0.2.27-usage.sql"))
            .unwrap();
        let store = Store::open_at(dir.clone()).unwrap();
        old.execute(
            "INSERT INTO chat_turns (id, session_id, assistant_message_id, client_turn_id, request_hash,
                                     prepared_input, settings_json, state, delivery_state, created_at, updated_at)
             VALUES ('turn_1', 'chat_1', 'a', 'c', 'h', '', '{}', 'completed', 'delivered', 1794312000000, 1)",
            [],
        )
        .unwrap();

        let week = seven_days(&store, NOW, Some(&[])).unwrap();
        assert_eq!(week.agents.len(), 1);
        assert_eq!(week.agents[0].harness, "codex");
        assert_eq!(week.agents[0].day_micros, [0; 7]);
        assert_eq!(week.agents[0].unpriced, 1);
        assert_eq!(week.compute.len(), 1);
        assert_eq!(week.compute[0].backend, "hf_job");
        assert_eq!(week.compute[0].unpriced, 1);
        let compute = chat_compute(&store, "chat_1", NOW).unwrap().unwrap();
        assert_eq!((compute.runs, compute.unpriced), (1, 1));
        drop((store, old));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
