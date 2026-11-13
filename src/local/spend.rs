//! What OpenResearch's chats and runs spent: agent cost per day, and compute
//! cost from the price each run was quoted when it launched.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::Result;
use crate::local::usage::{AgentSpend, BackendSpend, ChatCompute, UsageReport};
use crate::store::{BilledRun, Store};

/// Backends that bill for running time and have a price to read. The user's own machines and
/// clusters cost nothing orx can see; Vercel and Tinker have no per-run price.
pub(crate) const BILLED: [&str; 3] = ["openresearch_job", "hf_job", "modal_job"];

/// The currency of a row with nothing priced in it: every price source orx reads is in dollars.
const UNPRICED_CURRENCY: &str = "USD";

/// The seven days up to `now`'s and what was spent in them; the plans and the chat are the
/// caller's to fill.
pub fn seven_days(store: &Store, now: i64) -> Result<UsageReport> {
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
    for sample in store.spent_samples(from, to)? {
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
        let Some((start, end)) = running_time(&run, now) else {
            continue;
        };
        if start >= to || end <= from {
            continue;
        }
        let Some(quote) = &run.quote else {
            *unpriced.entry(run.backend).or_default() += 1;
            continue;
        };
        let row = backends
            .entry((run.backend, quote.currency.clone()))
            .or_default();
        for (day, spent) in row.iter_mut().enumerate() {
            let overlap = end.min(midnights[day + 1]) - start.max(midnights[day]);
            *spent += cost(quote, overlap);
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
        let Some((start, end)) = running_time(run, now) else {
            continue;
        };
        match &run.quote {
            Some(quote) => {
                *spent.entry(quote.currency.clone()).or_default() += cost(quote, end - start)
            }
            None => unpriced += 1,
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

/// From the payload's start to its end, or to now while it runs; None before it starts.
fn running_time(run: &BilledRun, now: i64) -> Option<(i64, i64)> {
    let start = run.started_at?;
    let end = if run.finished { run.ended_at? } else { now };
    Some((start, end.max(start)))
}

/// What `millis` of running time costs at `quote`.
fn cost(quote: &crate::store::Quote, millis: i64) -> u64 {
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

        let week = seven_days(&store, NOW).unwrap();
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

        let week = seven_days(&store, NOW).unwrap();
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
