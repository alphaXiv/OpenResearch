use super::*;

/// What a run's backend charges per unit of running time, read when the run launched. A later
/// price change never rewrites it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub amount_micros: u64,
    pub per: Per,
    pub currency: String,
    /// Where the price came from, such as `huggingface.co`.
    pub source: String,
    /// Unix millis.
    pub quoted_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Per {
    Minute,
    Hour,
}

impl Per {
    pub fn millis(self) -> u64 {
        match self {
            Self::Minute => 60_000,
            Self::Hour => 3_600_000,
        }
    }
}

/// A run on a backend that bills for running time, as spend reads it.
pub(crate) struct BilledRun {
    pub backend: String,
    pub finished: bool,
    /// Unix millis; None until the payload runs.
    pub started_at: Option<i64>,
    /// Unix millis; for a finished run, its end or its last update.
    pub ended_at: Option<i64>,
    /// None when the price couldn't be read, or was stored in a form this version can't read.
    pub quote: Option<Quote>,
}

/// One usage sample as spend reads it.
pub(crate) struct SpentSample {
    pub harness: String,
    /// `YYYY-MM-DD` in local time.
    pub day: String,
    pub usage: TokenUsage,
    pub cost: Option<Cost>,
}

impl Store {
    /// Records the run's quote unless it already has one.
    pub(crate) fn record_run_price(&self, run_id: &str, quote: &Quote) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE runs SET price_json = ?2 WHERE id = ?1 AND price_json IS NULL",
            params![run_id, serde_json::to_string(quote)?],
        )? > 0)
    }

    /// The launch intent of a run that has no price yet.
    pub(crate) fn unpriced_launch(&self, run_id: &str) -> Result<Option<LaunchSpec>> {
        let launch: Option<String> = self
            .conn
            .query_row(
                "SELECT launch_spec FROM runs WHERE id = ?1 AND price_json IS NULL",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        Ok(launch.map(|json| serde_json::from_str(&json)).transpose()?)
    }

    /// Runs on `backends` still running at or after `since`, or only the ones `chat` launched.
    pub(crate) fn billed_runs(
        &self,
        backends: &[&str],
        since: i64,
        chat: Option<&str>,
    ) -> Result<Vec<BilledRun>> {
        let mut statement = self.conn.prepare(
            "SELECT kind, status, started_at, COALESCE(ended_at, updated_at), price_json FROM (
                 SELECT COALESCE(json_extract(launch_spec, '$.kind'),
                                 json_extract(backend_json, '$.handle.descriptor.kind'),
                                 json_extract(backend_json, '$.kind')) AS kind,
                        status, started_at, ended_at, updated_at, price_json, chat_session_id
                 FROM runs
                 WHERE (?3 IS NULL OR chat_session_id = ?3)
                   AND (ended_at IS NULL OR ended_at >= ?2)
             ) WHERE kind IN (SELECT value FROM json_each(?1))",
        )?;
        let rows = statement.query_map(
            params![serde_json::to_string(backends)?, since, chat],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )?;
        let mut runs = Vec::new();
        for row in rows {
            let (backend, status, started_at, ended_at, price) = row?;
            let finished = crate::local::is_terminal(&status);
            runs.push(BilledRun {
                backend,
                finished,
                started_at,
                ended_at: ended_at.filter(|_| finished),
                quote: price.and_then(|json| serde_json::from_str(&json).ok()),
            });
        }
        Ok(runs)
    }

    /// Usage samples recorded in `[from, to)`. A sample from before samples had a time takes its
    /// turn's.
    pub(crate) fn spent_samples(&self, from: i64, to: i64) -> Result<Vec<SpentSample>> {
        let mut statement = self.conn.prepare(
            "SELECT harness, date(recorded_at / 1000, 'unixepoch', 'localtime'),
                    usage_json, cost_micros, cost_currency
             FROM chat_usage_samples WHERE recorded_at >= ?1 AND recorded_at < ?2
             UNION ALL
             SELECT s.harness, date(t.created_at / 1000, 'unixepoch', 'localtime'),
                    s.usage_json, s.cost_micros, s.cost_currency
             FROM chat_usage_samples s
             JOIN chat_usage_executions u ON u.execution_id = s.execution_id
             JOIN chat_turns t ON t.id = u.turn_id
             WHERE s.recorded_at IS NULL AND t.created_at >= ?1 AND t.created_at < ?2",
        )?;
        let rows = statement.query_map(params![from, to], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut samples = Vec::new();
        for row in rows {
            let (harness, day, usage, micros, currency) = row?;
            samples.push(SpentSample {
                harness,
                day,
                usage: serde_json::from_str(&usage)?,
                cost: micros.zip(currency).map(|(micros, currency)| Cost {
                    micros: micros as u64,
                    currency,
                }),
            });
        }
        Ok(samples)
    }

    /// The machine's local date `offset` days from `now`, and the Unix millis of its midnight.
    pub(crate) fn local_day(&self, now: i64, offset: i64) -> Result<(String, i64)> {
        let shift = format!("{offset} days");
        Ok(self.conn.query_row(
            "SELECT date(?1 / 1000, 'unixepoch', 'localtime', ?2),
                    CAST(strftime('%s', date(?1 / 1000, 'unixepoch', 'localtime', ?2), 'utc') AS INTEGER) * 1000",
            params![now, shift],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0.2.27's sample write, which knows no cost or time.
    const OLD_SAMPLE_UPSERT: &str = "INSERT INTO chat_usage_samples (execution_id, sample_id, harness, model, provider, usage_json) SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM chat_usage_executions WHERE execution_id = ?1 AND outcome IS NULL) ON CONFLICT(execution_id, sample_id) DO UPDATE SET model = COALESCE(excluded.model, model), provider = COALESCE(excluded.provider, provider), usage_json = CASE WHEN ?7 THEN excluded.usage_json ELSE usage_json END";

    #[test]
    fn rows_an_older_orx_rewrites_keep_their_cost_and_price() {
        let dir = std::env::temp_dir().join(format!("orx-old-writer-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let usage = TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            ..Default::default()
        };
        let cost = Cost {
            micros: 42,
            currency: "USD".into(),
        };
        store.begin_usage_execution("e", "t", "opencode").unwrap();
        store
            .record_usage_sample(
                "e",
                "s",
                "opencode",
                Some("m"),
                None,
                &usage,
                SampleCost::Reported(&cost),
            )
            .unwrap();
        let recorded = |store: &Store| -> (Option<i64>, Option<String>, Option<i64>) {
            store
                .conn
                .query_row(
                    "SELECT cost_micros, cost_currency, recorded_at FROM chat_usage_samples",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap()
        };
        let before = recorded(&store);
        assert_eq!((before.0, before.1.as_deref()), (Some(42), Some("USD")));
        let grown = TokenUsage {
            output_tokens: Some(3),
            ..usage
        };
        store
            .conn
            .execute(
                OLD_SAMPLE_UPSERT,
                params![
                    "e",
                    "s",
                    "opencode",
                    "m",
                    None::<String>,
                    serde_json::to_string(&grown).unwrap(),
                    true
                ],
            )
            .unwrap();
        assert_eq!(recorded(&store), before);

        store
            .conn
            .execute(
                "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at)
                 VALUES ('run', 'e', 'p', 'running', '{\"kind\":\"hf_job\"}', 1, 1)",
                [],
            )
            .unwrap();
        let quote = Quote {
            amount_micros: 167,
            per: Per::Minute,
            currency: "USD".into(),
            source: "huggingface.co".into(),
            quoted_at: 5,
        };
        assert!(store.record_run_price("run", &quote).unwrap());
        // A later price never replaces the quote.
        let later = Quote {
            amount_micros: 999,
            ..quote.clone()
        };
        assert!(!store.record_run_price("run", &later).unwrap());
        // Run writes unchanged since 0.2.27 leave the price alone.
        store
            .update_status("run", RunStatus::Done, Some(10), Some(0))
            .unwrap();
        let stored: String = store
            .conn
            .query_row("SELECT price_json FROM runs WHERE id = 'run'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(serde_json::from_str::<Quote>(&stored).unwrap(), quote);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
            serde_json::json!({"amountMicros": 167, "per": "minute", "currency": "USD",
                               "source": "huggingface.co", "quotedAt": 5})
        );
        assert!(store.unpriced_launch("run").unwrap().is_none());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
