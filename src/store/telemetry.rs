use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationIdentity {
    pub harness: String,
    pub model: String,
    pub provider: Option<String>,
}

fn valid_model_label(label: &str) -> bool {
    let trimmed = label.trim();
    let lower = trimmed.to_ascii_lowercase();
    !trimmed.is_empty()
        && trimmed.encode_utf16().count() <= 256
        && !label.chars().any(|c| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}'))
        && !trimmed.contains('\\')
        && !lower.starts_with("arn:")
        && !lower.contains("//")
        && !lower.starts_with("file:")
        && ![":file:", "=file:", "@file:"]
            .iter()
            .any(|prefix| lower.contains(prefix))
        && !trimmed.split(|c: char| c.is_whitespace() || [':', '=', '@', '('].contains(&c)).any(|part| {
            part.starts_with(['/', '\\', '~'])
                || part.starts_with("./")
                || part.starts_with("../")
                || part.starts_with(".\\")
                || part.starts_with("..\\")
        })
}

impl InvocationIdentity {
    pub fn validate(&self) -> Result<()> {
        if !["claude-code", "codex", "opencode", "cursor", "antigravity"]
            .contains(&self.harness.as_str())
        {
            return Err(anyhow!("Invalid invoking harness"));
        }
        for label in std::iter::once(self.model.as_str()).chain(self.provider.as_deref()) {
            if !valid_model_label(label) {
                return Err(anyhow!("Invalid invoking model/provider"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_write_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct CumulativeBaseline {
    usage: TokenUsage,
    generation: u64,
}

impl TokenUsage {
    pub fn validate(&self) -> Result<()> {
        const MAX: u64 = 9_007_199_254_740_991;
        let counters = [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
            self.reasoning_tokens,
        ];
        if counters.into_iter().flatten().any(|value| value > MAX)
            || self.total().is_some_and(|total| total > MAX)
            || self.input_tokens.is_some_and(|input| {
                self.cache_read_tokens.unwrap_or(0) + self.cache_write_tokens.unwrap_or(0) > input
            })
            || self
                .output_tokens
                .is_some_and(|output| self.reasoning_tokens.unwrap_or(0) > output)
        {
            return Err(anyhow!("Invalid native token counters"));
        }
        Ok(())
    }

    pub fn total(&self) -> Option<u64> {
        self.input_tokens?.checked_add(self.output_tokens?)
    }
}

/// A chat report's explicit attribution from the native model label alone: `(attribution, reason)`.
/// The model is reported only when exact; configured or selected models never reach here.
fn chat_attribution(
    harness: &str,
    model: Option<&str>,
    sampled: bool,
    delivery: Option<&str>,
) -> (&'static str, Option<&'static str>) {
    match model {
        Some("<synthetic>") if harness == "claude-code" => ("unresolved", Some("synthetic_model")),
        Some(model) if harness == "cursor" && model.eq_ignore_ascii_case("auto") => {
            ("auto_routing", None)
        }
        Some(_) => ("exact", None),
        None if sampled => ("unresolved", Some("identity_not_reported")),
        None if matches!(delivery, Some("not_sent" | "rejected")) => ("not_executed", None),
        None => ("unresolved", Some("no_usage_reported")),
    }
}

impl Store {
    pub(crate) fn native_invocation_identity(
        &self,
        harness: &str,
        call_id: &str,
    ) -> Result<Option<InvocationIdentity>> {
        let json: Option<String> = self.conn.query_row("SELECT identity_json FROM native_invocation_identities WHERE harness = ?1 AND call_id = ?2", params![harness, call_id], |row| row.get(0)).optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn record_native_invocation(
        &self,
        call_id: &str,
        identity: &InvocationIdentity,
        session_id: Option<&str>,
    ) -> Result<()> {
        identity.validate()?;
        let tx = self.begin_immediate()?;
        let owner: Option<String> = self.conn.query_row("SELECT id FROM chat_sessions WHERE harness = ?1 AND (id = ?2 OR native_session_id = ?2)", params![identity.harness, session_id], |row| row.get(0)).optional()?;
        self.conn.execute("INSERT INTO native_invocation_identities (harness, call_id, identity_json, session_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(harness, call_id) DO NOTHING", params![identity.harness, call_id, serde_json::to_string(identity)?, owner, now_ms()])?;
        if self
            .native_invocation_identity(&identity.harness, call_id)?
            .as_ref()
            != Some(identity)
        {
            return Err(anyhow!("Native tool identity changed after capture"));
        }
        tx.commit()?;
        Ok(())
    }

    /// A native turn's current model after a reroute; unlike tool invocations, a later reroute
    /// replaces it.
    pub(crate) fn record_native_reroute(
        &self,
        call_id: &str,
        identity: &InvocationIdentity,
    ) -> Result<()> {
        identity.validate()?;
        self.conn.execute("INSERT INTO native_invocation_identities (harness, call_id, identity_json, session_id, created_at) VALUES (?1, ?2, ?3, NULL, ?4) ON CONFLICT(harness, call_id) DO UPDATE SET identity_json = excluded.identity_json", params![identity.harness, call_id, serde_json::to_string(identity)?, now_ms()])?;
        Ok(())
    }

    pub(crate) fn reserve_run_telemetry(
        &self,
        run_id: &str,
        identity: Option<&InvocationIdentity>,
        report: Option<&(String, serde_json::Value)>,
    ) -> Result<()> {
        if let Some(identity) = identity {
            identity.validate()?;
        }
        self.conn.execute("INSERT INTO run_telemetry (run_id, identity_json, report_json) VALUES (?1, ?2, ?3) ON CONFLICT(run_id) DO NOTHING", params![run_id, identity.map(serde_json::to_string).transpose()?, report.map(|(_, payload)| serde_json::to_string(payload)).transpose()?])?;
        Ok(())
    }

    pub(super) fn stage_run_terminal(&self, run_id: &str, status: RunStatus) -> Result<()> {
        let report: Option<String> = self
            .conn
            .query_row(
                "SELECT report_json FROM run_telemetry WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        if let Some(report) = report {
            let mut payload: serde_json::Value = serde_json::from_str(&report)?;
            payload["events"][0]["properties"]["status"] = serde_json::json!(status.as_str());
            payload["events"][0]["occurredAt"] =
                serde_json::json!(crate::telemetry::iso8601_utc(now_ms()));
            let id = payload["events"][0]["eventId"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing run telemetry event ID"))?
                .to_string();
            self.stage_telemetry(&id, &payload)?;
        }
        Ok(())
    }

    /// A native turn's last counted cumulative total, shared by every execution that sees the turn
    /// (a sub-agent outliving its parent's turn, a resume replaying it). Stores from before the
    /// shared row fall back to the turn's recorded sample ids (`{scope}:{turn}:{generation}:{total}`).
    fn cumulative_baseline(
        &self,
        harness: &str,
        native_scope: &str,
        native_turn: &str,
    ) -> Result<Option<CumulativeBaseline>> {
        let shared: Option<String> = self.conn.query_row("SELECT totals_json FROM native_usage_baselines WHERE execution_id = ?1 AND prefix = ?2", params![harness, format!("cumulative:{native_scope}:{native_turn}")], |row| row.get(0)).optional()?.flatten();
        if let Some(json) = shared {
            return Ok(Some(serde_json::from_str(&json)?));
        }
        let ids = format!("{native_scope}:{native_turn}:");
        let mut stmt = self.conn.prepare("SELECT substr(sample_id, length(?2) + 1) FROM chat_usage_samples WHERE harness = ?1 AND substr(sample_id, 1, length(?2)) = ?2")?;
        let mut latest: Option<CumulativeBaseline> = None;
        for rest in stmt.query_map(params![harness, ids], |row| row.get::<_, String>(0))? {
            let rest = rest?;
            let Some((generation, total)) = rest.split_once(':') else {
                continue;
            };
            let recorded = CumulativeBaseline {
                generation: generation.parse()?,
                usage: serde_json::from_str(total)?,
            };
            if latest.as_ref().is_none_or(|latest| {
                (recorded.generation, recorded.usage.total())
                    > (latest.generation, latest.usage.total())
            }) {
                latest = Some(recorded);
            }
        }
        Ok(latest)
    }

    /// Whether `total` is already this native turn's counted total.
    pub(crate) fn cumulative_counted(
        &self,
        harness: &str,
        native_scope: &str,
        native_turn: &str,
        total: &TokenUsage,
    ) -> Result<bool> {
        Ok(self
            .cumulative_baseline(harness, native_scope, native_turn)?
            .is_some_and(|baseline| &baseline.usage == total))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_cumulative_usage(
        &self,
        execution_id: &str,
        harness: &str,
        native_scope: &str,
        native_turn: &str,
        model: Option<&str>,
        total: &TokenUsage,
        last: &TokenUsage,
    ) -> Result<()> {
        total.validate()?;
        last.validate()?;
        let prefix = format!("cumulative:{native_scope}:{native_turn}");
        let tx = self.begin()?;
        let previous = self.cumulative_baseline(harness, native_scope, native_turn)?;
        if previous
            .as_ref()
            .is_some_and(|previous| &previous.usage == total)
        {
            return Ok(());
        }
        let delta = if let Some(previous) = &previous {
            TokenUsage {
                input_tokens: total
                    .input_tokens
                    .and_then(|n| n.checked_sub(previous.usage.input_tokens?)),
                output_tokens: total
                    .output_tokens
                    .and_then(|n| n.checked_sub(previous.usage.output_tokens?)),
                cache_read_tokens: total
                    .cache_read_tokens
                    .and_then(|n| n.checked_sub(previous.usage.cache_read_tokens?)),
                cache_write_tokens: total
                    .cache_write_tokens
                    .and_then(|n| n.checked_sub(previous.usage.cache_write_tokens?)),
                reasoning_tokens: total
                    .reasoning_tokens
                    .and_then(|n| n.checked_sub(previous.usage.reasoning_tokens?)),
            }
        } else {
            last.clone()
        };
        let reset =
            previous.is_some() && (delta.input_tokens.is_none() || delta.output_tokens.is_none());
        let generation =
            previous.as_ref().map_or(0, |previous| previous.generation) + u64::from(reset);
        if !reset {
            let sample_id = format!(
                "{native_scope}:{native_turn}:{generation}:{}",
                serde_json::to_string(total)?
            );
            self.record_usage_sample(execution_id, &sample_id, harness, model, None, &delta)?;
            // A delta of two native totals covers its requests whole when both counters are known.
            tx.execute("UPDATE chat_usage_samples SET complete = ?3 WHERE execution_id = ?1 AND sample_id = ?2", params![execution_id, sample_id, delta.input_tokens.is_some() && delta.output_tokens.is_some()])?;
        }
        let baseline = CumulativeBaseline {
            usage: total.clone(),
            generation,
        };
        // Keyed by harness, not execution: the baseline belongs to the native turn.
        tx.execute("INSERT INTO native_usage_baselines (execution_id, prefix, totals_json) VALUES (?1, ?2, ?3) ON CONFLICT(execution_id, prefix) DO UPDATE SET totals_json = excluded.totals_json", params![harness, prefix, serde_json::to_string(&baseline)?])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn begin_native_usage_attempt(
        &self,
        execution_id: &str,
        prefix: &str,
        harness: &str,
        native_scope: Option<&str>,
    ) -> Result<()> {
        let baseline: Option<String> = if let Some(scope) = native_scope {
            self.conn.query_row("SELECT totals_json FROM native_usage_totals WHERE harness = ?1 AND native_scope = ?2", params![harness, scope], |row| row.get(0)).optional()?
        } else {
            Some("{}".into())
        };
        self.conn.execute("INSERT INTO native_usage_baselines (execution_id, prefix, totals_json) VALUES (?1, ?2, ?3) ON CONFLICT(execution_id, prefix) DO NOTHING", params![execution_id, prefix, baseline])?;
        Ok(())
    }

    pub(crate) fn replace_native_usage_aggregate(
        &self,
        execution_id: &str,
        prefix: &str,
        harness: &str,
        native_scope: &str,
        samples: &[(String, Option<String>, TokenUsage)],
    ) -> Result<()> {
        for (_, _, usage) in samples {
            usage.validate()?;
        }
        let tx = self.begin()?;
        let active: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM chat_usage_executions WHERE execution_id = ?1 AND outcome IS NULL)", [execution_id], |row| row.get(0))?;
        let baseline: Option<String> = tx.query_row("SELECT totals_json FROM native_usage_baselines WHERE execution_id = ?1 AND prefix = ?2", params![execution_id, prefix], |row| row.get(0)).optional()?.flatten();
        let totals: std::collections::BTreeMap<_, _> = samples
            .iter()
            .map(|(model, _, usage)| (model, usage))
            .collect();
        if active && !samples.is_empty() {
            if let Some(baseline) = baseline {
                let baseline: std::collections::BTreeMap<String, TokenUsage> =
                    serde_json::from_str(&baseline)?;
                let reset = samples.iter().any(|(model, _, usage)| {
                    baseline.get(model).is_some_and(|previous| {
                        matches!((usage.input_tokens, previous.input_tokens), (Some(now), Some(before)) if now < before)
                            || matches!((usage.output_tokens, previous.output_tokens), (Some(now), Some(before)) if now < before)
                    })
                });
                if reset {
                    tx.execute("UPDATE native_usage_baselines SET totals_json = NULL WHERE execution_id = ?1 AND prefix = ?2", params![execution_id, prefix])?;
                    tx.execute("UPDATE chat_usage_samples SET complete = 0 WHERE execution_id = ?1 AND substr(sample_id, 1, length(?2)) = ?2", params![execution_id, prefix])?;
                } else {
                    // The aggregate counts every request: its models replace their messages, and any
                    // other model keeps only its identity.
                    let models: Vec<_> = samples.iter().map(|(model, _, _)| model).collect();
                    tx.execute("DELETE FROM chat_usage_samples WHERE execution_id = ?1 AND substr(sample_id, 1, length(?2)) = ?2 AND (model IS NULL OR model IN (SELECT value FROM json_each(?3)))", params![execution_id, prefix, serde_json::to_string(&models)?])?;
                    tx.execute("UPDATE chat_usage_samples SET usage_json = ?3, complete = 0 WHERE execution_id = ?1 AND substr(sample_id, 1, length(?2)) = ?2", params![execution_id, prefix, serde_json::to_string(&TokenUsage::default())?])?;
                    for (model, provider, usage) in samples {
                        let zero = TokenUsage {
                            input_tokens: Some(0),
                            output_tokens: Some(0),
                            cache_read_tokens: Some(0),
                            cache_write_tokens: Some(0),
                            reasoning_tokens: Some(0),
                        };
                        let previous = baseline.get(model).unwrap_or(&zero);
                        if usage == previous {
                            continue;
                        }
                        let delta = TokenUsage {
                            input_tokens: usage
                                .input_tokens
                                .and_then(|n| n.checked_sub(previous.input_tokens?)),
                            output_tokens: usage
                                .output_tokens
                                .and_then(|n| n.checked_sub(previous.output_tokens?)),
                            cache_read_tokens: usage
                                .cache_read_tokens
                                .and_then(|n| n.checked_sub(previous.cache_read_tokens?)),
                            cache_write_tokens: usage
                                .cache_write_tokens
                                .and_then(|n| n.checked_sub(previous.cache_write_tokens?)),
                            reasoning_tokens: usage
                                .reasoning_tokens
                                .and_then(|n| n.checked_sub(previous.reasoning_tokens?)),
                        };
                        let sample_id = format!("{prefix}aggregate:{model}");
                        self.record_usage_sample(
                            execution_id,
                            &sample_id,
                            harness,
                            Some(model),
                            provider.as_deref(),
                            &delta,
                        )?;
                        tx.execute("UPDATE chat_usage_samples SET complete = ?3 WHERE execution_id = ?1 AND sample_id = ?2", params![execution_id, sample_id, delta.input_tokens.is_some() && delta.output_tokens.is_some()])?;
                    }
                }
            }
            tx.execute("INSERT INTO native_usage_totals (harness, native_scope, totals_json) VALUES (?1, ?2, ?3) ON CONFLICT(harness, native_scope) DO UPDATE SET totals_json = excluded.totals_json", params![harness, native_scope, serde_json::to_string(&totals)?])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn begin_usage_execution(
        &self,
        execution_id: &str,
        turn_id: &str,
        harness: &str,
    ) -> Result<()> {
        self.conn.execute("INSERT INTO chat_usage_executions (execution_id, turn_id, harness, report_id, suppressed) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(execution_id) DO NOTHING", params![execution_id, turn_id, harness, uuid::Uuid::new_v4().to_string(), !crate::telemetry::accounting_reports_enabled()])?;
        Ok(())
    }

    pub(crate) fn recover_terminal_usage(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT t.id, t.state FROM chat_usage_executions u JOIN chat_turns t ON t.id = u.turn_id WHERE u.outcome IS NULL AND t.state IN ('completed', 'failed', 'interrupted') AND NOT EXISTS (SELECT 1 FROM chat_turn_leases l WHERE l.chat_session_id = t.session_id)")?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (turn_id, state) in rows {
            self.finalize_turn_usage(
                &turn_id,
                match state.as_str() {
                    "completed" => "done",
                    "interrupted" => "cancelled",
                    _ => "failed",
                },
            )?;
        }
        Ok(())
    }

    pub(crate) fn finalize_usage_turns(&self, turn_prefix: &str, outcome: &str) -> Result<()> {
        let turns: Vec<String> = self
            .conn
            .prepare("SELECT DISTINCT turn_id FROM chat_usage_executions WHERE outcome IS NULL AND substr(turn_id, 1, length(?1)) = ?1")?
            .query_map([turn_prefix], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for turn in turns {
            self.finalize_turn_usage(&turn, outcome)?;
        }
        Ok(())
    }

    pub(crate) fn finalize_turn_usage(&self, turn_id: &str, outcome: &str) -> Result<()> {
        let mut stmt = self.conn.prepare("SELECT execution_id, harness, report_id, suppressed FROM chat_usage_executions WHERE turn_id = ?1 AND outcome IS NULL")?;
        let rows = stmt
            .query_map([turn_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for (execution_id, harness, report_id, suppressed) in rows {
            // One write lock from reading samples to closing, so a hook's concurrent sample is
            // either in this report or rejected as late.
            let tx = self.begin_immediate()?;
            let mut samples = self.conn.prepare("SELECT s.model, s.provider, s.usage_json, s.complete, n.identity_json FROM chat_usage_samples s LEFT JOIN native_invocation_identities n ON n.harness = s.harness AND n.call_id = s.sample_id WHERE s.execution_id = ?1 ORDER BY s.sample_id")?;
            let mut grouped = std::collections::BTreeMap::<
                (Option<String>, Option<String>, [bool; 5]),
                Vec<(TokenUsage, bool)>,
            >::new();
            for row in samples.query_map([&execution_id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })? {
                let (model, provider, json, complete, identity) = row?;
                let identity: Option<InvocationIdentity> = identity
                    .map(|json| serde_json::from_str(&json))
                    .transpose()?;
                let model =
                    model.or_else(|| identity.as_ref().map(|identity| identity.model.clone()));
                let provider = provider.or_else(|| identity.and_then(|identity| identity.provider));
                let usage: TokenUsage = serde_json::from_str(&json)?;
                let measured = [
                    usage.input_tokens,
                    usage.output_tokens,
                    usage.cache_read_tokens,
                    usage.cache_write_tokens,
                    usage.reasoning_tokens,
                ]
                .map(|counter| counter.is_some());
                grouped
                    .entry((model, provider, measured))
                    .or_default()
                    .push((usage, complete));
            }
            // A model seen without tokens joins that model and provider's measured requests, if any.
            let measured: Vec<(Option<String>, Option<String>)> = grouped
                .keys()
                .filter(|(_, _, mask)| mask.contains(&true))
                .map(|(model, provider, _)| (model.clone(), provider.clone()))
                .collect();
            grouped.retain(|(model, provider, mask), _| {
                mask.contains(&true)
                    || model.is_none()
                    || !measured.contains(&(model.clone(), provider.clone()))
            });
            if grouped.is_empty() {
                grouped.insert((None, None, [false; 5]), Vec::new());
            }
            let delivery: Option<String> = self
                .conn
                .query_row(
                    "SELECT delivery_state FROM chat_turns WHERE id = ?1",
                    [turn_id],
                    |row| row.get(0),
                )
                .optional()?;
            let mut reports = Vec::new();
            if !suppressed {
                for ((model, provider, _), samples) in grouped {
                    let sum = |field: fn(&TokenUsage) -> Option<u64>| -> Option<u64> {
                        if samples.is_empty() {
                            return None;
                        }
                        let measured: Vec<_> = samples
                            .iter()
                            .filter_map(|(usage, _)| field(usage))
                            .collect();
                        if measured.is_empty() {
                            return None;
                        }
                        measured
                            .into_iter()
                            .try_fold(0u64, |total, value| total.checked_add(value))
                    };
                    let usage = TokenUsage {
                        input_tokens: sum(|u| u.input_tokens),
                        output_tokens: sum(|u| u.output_tokens),
                        cache_read_tokens: sum(|u| u.cache_read_tokens),
                        cache_write_tokens: sum(|u| u.cache_write_tokens),
                        reasoning_tokens: sum(|u| u.reasoning_tokens),
                    };
                    usage.validate()?;
                    let mut properties = serde_json::to_value(&usage)?;
                    properties["reportId"] = serde_json::json!(report_id);
                    properties["harness"] = serde_json::json!(harness);
                    let (attribution, reason) = chat_attribution(
                        &harness,
                        model.as_deref(),
                        !samples.is_empty(),
                        delivery.as_deref(),
                    );
                    let exact = attribution == "exact";
                    properties["model"] = serde_json::json!(model.filter(|_| exact));
                    properties["provider"] = serde_json::json!(provider.filter(|_| exact));
                    properties["attribution"] = serde_json::json!(attribution);
                    properties["attributionReason"] = serde_json::json!(reason);
                    properties["totalTokens"] = serde_json::json!(usage.total());
                    properties["outcome"] = serde_json::json!(outcome);
                    properties["coverage"] = serde_json::json!(if [
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens,
                        usage.cache_write_tokens,
                        usage.reasoning_tokens
                    ]
                    .iter()
                    .all(Option::is_none)
                    {
                        "missing"
                    } else if samples.iter().all(|(usage, complete)| *complete
                        && usage.input_tokens.is_some()
                        && usage.output_tokens.is_some())
                    {
                        "complete"
                    } else {
                        "partial"
                    });
                    if let Some(report) =
                        crate::telemetry::pending_event_payload("chat_model_usage", properties)
                    {
                        reports.push(report);
                    }
                }
            }
            drop(samples);
            self.finalize_usage_execution(&execution_id, outcome, reports)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Callers hold a write transaction.
    fn finalize_usage_execution(
        &self,
        execution_id: &str,
        outcome: &str,
        reports: Vec<(String, serde_json::Value)>,
    ) -> Result<()> {
        let suppressed: Option<bool> = self.conn.query_row("UPDATE chat_usage_executions SET outcome = ?2 WHERE execution_id = ?1 AND outcome IS NULL RETURNING suppressed", params![execution_id, outcome], |row| row.get(0)).optional()?;
        if suppressed == Some(false) {
            for (id, payload) in reports {
                self.stage_telemetry(&id, &payload)?;
            }
        }
        Ok(())
    }

    pub(crate) fn stage_telemetry(
        &self,
        event_id: &str,
        payload: &serde_json::Value,
    ) -> Result<()> {
        self.conn.execute("INSERT INTO telemetry_pending_events (event_id, payload_json) VALUES (?1, ?2) ON CONFLICT(event_id) DO NOTHING", params![event_id, serde_json::to_string(payload)?])?;
        Ok(())
    }

    pub(crate) fn pending_telemetry(&self) -> Result<Vec<(String, serde_json::Value)>> {
        let mut stmt = self.conn.prepare(
            "SELECT event_id, payload_json FROM telemetry_pending_events ORDER BY rowid LIMIT 100",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (id, json) = row?;
            Ok((id, serde_json::from_str(&json)?))
        })
        .collect()
    }

    pub(crate) fn acknowledge_pending_telemetry(&self, event_id: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM telemetry_pending_events WHERE event_id = ?1",
            [event_id],
        )?;
        Ok(())
    }

    pub(crate) fn purge_pending_telemetry(&self) -> Result<()> {
        let tx = self.begin()?;
        tx.execute("DELETE FROM telemetry_pending_events", [])?;
        tx.execute("UPDATE run_telemetry SET report_json = NULL", [])?;
        tx.execute(
            "UPDATE chat_usage_executions SET suppressed = 1 WHERE outcome IS NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn record_usage_sample(
        &self,
        execution_id: &str,
        sample_id: &str,
        harness: &str,
        model: Option<&str>,
        provider: Option<&str>,
        usage: &TokenUsage,
    ) -> Result<()> {
        usage.validate()?;
        let model = model.filter(|label| valid_model_label(label));
        let provider = provider.filter(|label| valid_model_label(label));
        self.conn.execute("INSERT INTO chat_usage_samples (execution_id, sample_id, harness, model, provider, usage_json) SELECT ?1, ?2, ?3, ?4, ?5, ?6 WHERE EXISTS (SELECT 1 FROM chat_usage_executions WHERE execution_id = ?1 AND outcome IS NULL) ON CONFLICT(execution_id, sample_id) DO UPDATE SET model = COALESCE(excluded.model, model), provider = COALESCE(excluded.provider, provider), usage_json = CASE WHEN ?7 THEN excluded.usage_json ELSE usage_json END", params![execution_id, sample_id, harness, model, provider, serde_json::to_string(usage)?, usage != &TokenUsage::default()])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_chat_report_cannot_escape_opt_out_or_duplicate_finalization() {
        let dir = std::env::temp_dir().join(format!("orx-opt-out-race-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .begin_usage_execution("execution", "turn", "claude-code")
            .unwrap();
        store
            .conn
            .execute("UPDATE chat_usage_executions SET suppressed = 0", [])
            .unwrap();
        let prepared = vec![("prepared".into(), serde_json::json!({"usage": 12}))];
        store.purge_pending_telemetry().unwrap();
        store
            .finalize_usage_execution("execution", "done", prepared.clone())
            .unwrap();
        assert!(store.pending_telemetry().unwrap().is_empty());
        store
            .conn
            .execute("UPDATE chat_usage_executions SET suppressed = 0", [])
            .unwrap();
        store
            .finalize_usage_execution("execution", "done", prepared)
            .unwrap();
        assert!(store.pending_telemetry().unwrap().is_empty());
        let outcome: String = store
            .conn
            .query_row("SELECT outcome FROM chat_usage_executions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(outcome, "done");
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A sub-agent outliving its parent's turn continues the native turn's shared baseline: the
    /// late execution counts the increase since the parent's last total (requests between them
    /// included), a replay adds nothing, a new native turn starts its own baseline, and a legacy
    /// store's replayed total (no shared row) opens no execution.
    #[test]
    fn late_codex_child_usage_continues_from_its_parent_execution() {
        let dir = std::env::temp_dir().join(format!("orx-late-child-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let models = std::sync::Mutex::new(std::collections::HashMap::new());
        let record = |method: &str, params: serde_json::Value| {
            crate::local::harness::codex::record_unowned_in(&store, "s", &models, method, &params)
                .unwrap()
        };
        let counters = |n: u64| TokenUsage {
            input_tokens: Some(n),
            output_tokens: Some(n),
            ..Default::default()
        };
        let usage = |turn: &str, total: u64, last: u64| {
            serde_json::json!({"threadId": "child", "turnId": turn, "tokenUsage": {
                "total": {"inputTokens": total, "outputTokens": total},
                "last": {"inputTokens": last, "outputTokens": last}}})
        };
        let samples = |execution: &str| -> Vec<(Option<String>, u64)> {
            store
                .conn
                .prepare("SELECT model, usage_json FROM chat_usage_samples WHERE execution_id = ?1")
                .unwrap()
                .query_map([execution], |row| {
                    Ok((row.get(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .map(|row| {
                    let (model, json) = row.unwrap();
                    (
                        model,
                        serde_json::from_str::<TokenUsage>(&json)
                            .unwrap()
                            .input_tokens
                            .unwrap(),
                    )
                })
                .collect()
        };
        let executions = || -> i64 {
            store
                .conn
                .query_row("SELECT COUNT(*) FROM chat_usage_executions WHERE execution_id LIKE 'codex-late:%'", [], |row| row.get(0))
                .unwrap()
        };
        let safe = || Some("gpt-6-safe".to_string());
        store
            .begin_usage_execution("parent", "parent-turn", "codex")
            .unwrap();
        store
            .record_cumulative_usage(
                "parent",
                "codex",
                "child",
                "ct",
                safe().as_deref(),
                &counters(10),
                &counters(4),
            )
            .unwrap();
        store.finalize_turn_usage("parent-turn", "done").unwrap();
        crate::local::harness::codex::capture_reroute(
            Some(&store),
            &models,
            &serde_json::json!({"threadId": "child", "turnId": "ct", "toModel": "gpt-6-safe"}),
        );
        models.lock().unwrap().clear();
        record("thread/tokenUsage/updated", usage("ct", 10, 4));
        assert_eq!(executions(), 0);
        record("thread/tokenUsage/updated", usage("ct", 30, 5));
        record("thread/tokenUsage/updated", usage("ct", 30, 5));
        record(
            "turn/completed",
            serde_json::json!({"threadId": "child", "turn": {"id": "ct", "status": "completed"}}),
        );
        record("thread/tokenUsage/updated", usage("ct", 40, 5));
        assert_eq!(samples("parent"), [(safe(), 4)]);
        assert_eq!(samples("codex-late:s:child:ct"), [(safe(), 20)]);
        let outcome: String = store
            .conn
            .query_row("SELECT outcome FROM chat_usage_executions WHERE execution_id = 'codex-late:s:child:ct'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(outcome, "done");
        models
            .lock()
            .unwrap()
            .insert(("child".into(), "ct2".into()), "gpt-6-sol".into());
        record("thread/tokenUsage/updated", usage("ct2", 47, 7));
        assert_eq!(
            samples("codex-late:s:child:ct2"),
            [(Some("gpt-6-sol".into()), 7)]
        );
        // A store from before the shared baseline knows a finished turn only by its sample ids.
        store
            .begin_usage_execution("old", "old-turn", "codex")
            .unwrap();
        store
            .record_cumulative_usage(
                "old",
                "codex",
                "child",
                "ct3",
                None,
                &counters(60),
                &counters(3),
            )
            .unwrap();
        store.finalize_turn_usage("old-turn", "done").unwrap();
        store
            .conn
            .execute(
                "DELETE FROM native_usage_baselines WHERE prefix = 'cumulative:child:ct3'",
                [],
            )
            .unwrap();
        record("thread/tokenUsage/updated", usage("ct3", 60, 3));
        assert_eq!(executions(), 2);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A hook without a planner row may be a call that failed before running: its model is kept
    /// as invocation evidence but never becomes an executed sample.
    #[test]
    fn antigravity_hooks_record_executed_models_only_with_a_planner() {
        let dir = std::env::temp_dir().join(format!("orx-agy-planner-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let transcript = dir.join("transcript.jsonl");
        std::fs::write(
            &transcript,
            serde_json::json!({"step_index": 4, "type": "PLANNER_RESPONSE"}).to_string(),
        )
        .unwrap();
        store
            .begin_usage_execution("e", "t", "antigravity")
            .unwrap();
        for step in [3, 5] {
            let payload = serde_json::json!({"conversationId": "c", "initialNumSteps": step,
                "modelName": "gemini-3.8-flash-high", "transcriptPath": transcript});
            crate::commands::mcp_gate::record_post_invocation(&store, &payload, Some("e")).unwrap();
        }
        let samples: Vec<(String, Option<String>)> = store
            .conn
            .prepare("SELECT sample_id, model FROM chat_usage_samples")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(
            samples,
            [(
                "antigravity:c:step:4".into(),
                Some("gemini-3.8-flash-high".into())
            )]
        );
        assert!(store
            .native_invocation_identity("antigravity", "antigravity:c:invocation:5")
            .unwrap()
            .is_some());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn complementary_partial_samples_finalize_without_losing_measured_counters() {
        let dir = std::env::temp_dir().join(format!("orx-partial-usage-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .begin_usage_execution("execution", "turn", "claude-code")
            .unwrap();
        store
            .conn
            .execute("UPDATE chat_usage_executions SET suppressed = 0", [])
            .unwrap();
        for (id, usage) in [
            (
                "input",
                TokenUsage {
                    input_tokens: Some(10),
                    cache_read_tokens: Some(8),
                    ..Default::default()
                },
            ),
            (
                "cache",
                TokenUsage {
                    cache_read_tokens: Some(9),
                    ..Default::default()
                },
            ),
            (
                "output",
                TokenUsage {
                    output_tokens: Some(3),
                    reasoning_tokens: Some(2),
                    ..Default::default()
                },
            ),
            (
                "reasoning",
                TokenUsage {
                    reasoning_tokens: Some(4),
                    ..Default::default()
                },
            ),
        ] {
            store
                .record_usage_sample(
                    "execution",
                    id,
                    "claude-code",
                    Some("claude-opus-5-5"),
                    None,
                    &usage,
                )
                .unwrap();
        }
        store.finalize_turn_usage("turn", "failed").unwrap();
        store.finalize_turn_usage("turn", "failed").unwrap();
        let outcome: String = store
            .conn
            .query_row("SELECT outcome FROM chat_usage_executions", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(outcome, "failed");
        let counters: Vec<String> = store
            .conn
            .prepare("SELECT usage_json FROM chat_usage_samples")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        let cache: u64 = counters
            .iter()
            .map(|json| {
                serde_json::from_str::<TokenUsage>(json)
                    .unwrap()
                    .cache_read_tokens
                    .unwrap_or(0)
            })
            .sum();
        assert_eq!(cache, 17);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invocation_identities_follow_session_deletion_and_unowned_retention() {
        let dir =
            std::env::temp_dir().join(format!("orx-identity-retention-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        for id in ["session", "project-session"] {
            store.conn.execute("INSERT INTO chat_sessions (id, project_id, harness, created_at, updated_at) VALUES (?1, 'project', 'claude-code', 1, 1)", [id]).unwrap();
        }
        let identity = InvocationIdentity {
            harness: "claude-code".into(),
            model: "claude-opus-5-5".into(),
            provider: None,
        };
        store
            .record_native_invocation("session-call", &identity, Some("session"))
            .unwrap();
        store
            .record_native_invocation("project-call", &identity, Some("project-session"))
            .unwrap();
        store
            .record_native_invocation("legacy-call", &identity, None)
            .unwrap();
        store
            .record_native_invocation("recent-unowned", &identity, None)
            .unwrap();
        store.conn.execute("UPDATE native_invocation_identities SET created_at = 0 WHERE call_id = 'legacy-call'", []).unwrap();
        store.delete_chat_session("session").unwrap();
        assert_eq!(
            store
                .native_invocation_identity("claude-code", "session-call")
                .unwrap(),
            None
        );
        store.delete_local_project("project").unwrap();
        assert_eq!(
            store
                .native_invocation_identity("claude-code", "project-call")
                .unwrap(),
            None
        );
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        assert_eq!(
            store
                .native_invocation_identity("claude-code", "legacy-call")
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .native_invocation_identity("claude-code", "recent-unowned")
                .unwrap(),
            Some(identity)
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pending_reports_commit_and_survive_restart() {
        let dir = std::env::temp_dir().join(format!("orx-telemetry-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let payload = serde_json::json!({"measured": 0});
        {
            let tx = store.begin().unwrap();
            store.stage_telemetry("rolled-back", &payload).unwrap();
            drop(tx);
        }
        assert!(store.pending_telemetry().unwrap().is_empty());
        let tx = store.begin().unwrap();
        store.stage_telemetry("committed", &payload).unwrap();
        tx.commit().unwrap();
        drop(store);
        let store = Store::open_at(dir.clone()).unwrap();
        store.stage_telemetry("committed", &payload).unwrap();
        assert_eq!(
            store.pending_telemetry().unwrap(),
            vec![("committed".into(), payload)]
        );
        store.acknowledge_pending_telemetry("committed").unwrap();
        assert!(store.pending_telemetry().unwrap().is_empty());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn terminal_report_is_guarded_transactional_and_independent_of_run_deletion() {
        let dir = std::env::temp_dir().join(format!("orx-terminal-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let run = StoredRun {
            id: "run".into(),
            experiment_id: "exp".into(),
            project_id: "project".into(),
            status: "starting".into(),
            backend_json: "{}".into(),
            command: "".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            chat_session_id: None,
        };
        let identity = InvocationIdentity {
            harness: "codex".into(),
            model: "gpt-6-sol".into(),
            provider: Some("openai".into()),
        };
        let event_id = uuid::Uuid::new_v4().to_string();
        let report = (
            event_id.clone(),
            serde_json::json!({"events":[{"eventId":event_id,"properties":{"harness":"codex","model":"gpt-6-sol","provider":"openai","status":"failed"}}]}),
        );
        let tx = store.begin().unwrap();
        store
            .reserve_run_telemetry(&run.id, Some(&identity), Some(&report))
            .unwrap();
        store.upsert_run(&run).unwrap();
        tx.commit().unwrap();
        {
            let tx = store.begin().unwrap();
            assert!(store
                .update_status("run", RunStatus::Failed, Some(2), Some(1))
                .unwrap());
            assert_eq!(store.pending_telemetry().unwrap().len(), 1);
            drop(tx);
        }
        assert!(store.pending_telemetry().unwrap().is_empty());
        assert!(store
            .update_status("run", RunStatus::Cancelled, Some(3), None)
            .unwrap());
        assert!(!store
            .update_status("run", RunStatus::Done, Some(4), Some(0))
            .unwrap());
        store.conn.execute("DELETE FROM runs", []).unwrap();
        let events = store.pending_telemetry().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].1["events"][0]["properties"]["status"],
            "cancelled"
        );
        assert_eq!(events[0].1["events"][0]["properties"]["model"], "gpt-6-sol");
        store.purge_pending_telemetry().unwrap();
        assert!(store.pending_telemetry().unwrap().is_empty());
        let report: Option<String> = store
            .conn
            .query_row("SELECT report_json FROM run_telemetry", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(report.is_none());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cumulative_baselines_exclude_resume_history_and_replace_repeated_snapshots() {
        let dir = std::env::temp_dir().join(format!("orx-baselines-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let usage = |input, output| TokenUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
            reasoning_tokens: Some(0),
        };
        let first = vec![
            ("parent".into(), None, usage(100, 20)),
            ("child".into(), None, usage(30, 4)),
        ];
        store
            .begin_usage_execution("first", "turn1", "claude-code")
            .unwrap();
        store
            .begin_native_usage_attempt("first", "attempt:", "claude-code", None)
            .unwrap();
        store
            .replace_native_usage_aggregate("first", "attempt:", "claude-code", "scope", &first)
            .unwrap();
        store.finalize_turn_usage("turn1", "done").unwrap();
        store
            .begin_usage_execution("second", "turn2", "claude-code")
            .unwrap();
        store
            .begin_native_usage_attempt("second", "attempt:", "claude-code", Some("scope"))
            .unwrap();
        let second = vec![("parent".into(), None, usage(140, 29)), first[1].clone()];
        for _ in 0..2 {
            store
                .replace_native_usage_aggregate(
                    "second",
                    "attempt:",
                    "claude-code",
                    "scope",
                    &second,
                )
                .unwrap();
        }
        let samples: Vec<String> = store
            .conn
            .prepare("SELECT usage_json FROM chat_usage_samples WHERE execution_id = 'second'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(
            serde_json::from_str::<TokenUsage>(&samples[0]).unwrap(),
            usage(40, 9)
        );
        store
            .begin_usage_execution("imported", "turn3", "claude-code")
            .unwrap();
        store
            .begin_native_usage_attempt(
                "imported",
                "attempt:",
                "claude-code",
                Some("unknown-history"),
            )
            .unwrap();
        store
            .replace_native_usage_aggregate(
                "imported",
                "attempt:",
                "claude-code",
                "unknown-history",
                &second,
            )
            .unwrap();
        let count: i64 = store
            .conn
            .query_row(
                "SELECT count(*) FROM chat_usage_samples WHERE execution_id = 'imported'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        store
            .begin_usage_execution("reset", "turn4", "claude-code")
            .unwrap();
        store
            .begin_native_usage_attempt("reset", "attempt:", "claude-code", Some("scope"))
            .unwrap();
        store
            .record_usage_sample(
                "reset",
                "attempt:request",
                "claude-code",
                Some("parent"),
                None,
                &usage(12, 3),
            )
            .unwrap();
        store
            .replace_native_usage_aggregate(
                "reset",
                "attempt:",
                "claude-code",
                "scope",
                &[("parent".into(), None, usage(20, 4))],
            )
            .unwrap();
        let (json, complete): (String, bool) = store
            .conn
            .query_row(
                "SELECT usage_json, complete FROM chat_usage_samples WHERE execution_id = 'reset'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<TokenUsage>(&json).unwrap(),
            usage(12, 3)
        );
        assert!(!complete);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cumulative_notifications_exclude_history_deduplicate_and_handle_resets() {
        let dir = std::env::temp_dir().join(format!("orx-cumulative-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let usage = |input, output| TokenUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            cache_read_tokens: Some(0),
            cache_write_tokens: Some(0),
            reasoning_tokens: Some(0),
        };
        store
            .begin_usage_execution("execution", "turn", "codex")
            .unwrap();
        for _ in 0..2 {
            store
                .record_cumulative_usage(
                    "execution",
                    "codex",
                    "thread",
                    "native-turn",
                    None,
                    &usage(1100, 110),
                    &usage(100, 10),
                )
                .unwrap();
        }
        store
            .record_cumulative_usage(
                "execution",
                "codex",
                "thread",
                "native-turn",
                None,
                &usage(1200, 120),
                &usage(100, 10),
            )
            .unwrap();
        store
            .record_cumulative_usage(
                "execution",
                "codex",
                "thread",
                "native-turn",
                None,
                &usage(1000, 100),
                &usage(0, 0),
            )
            .unwrap();
        store
            .record_cumulative_usage(
                "execution",
                "codex",
                "thread",
                "native-turn",
                None,
                &usage(1100, 110),
                &usage(100, 10),
            )
            .unwrap();
        let samples: Vec<String> = store
            .conn
            .prepare("SELECT usage_json FROM chat_usage_samples WHERE execution_id = 'execution'")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(samples.len(), 3);
        assert_eq!(
            samples
                .iter()
                .map(|json| serde_json::from_str::<TokenUsage>(json)
                    .unwrap()
                    .total()
                    .unwrap())
                .sum::<u64>(),
            330
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn repeated_native_samples_replace_and_finalized_samples_do_not_change() {
        let dir = std::env::temp_dir().join(format!("orx-samples-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .begin_usage_execution("execution", "turn", "claude-code")
            .unwrap();
        for output in [2, 4, 4] {
            store
                .record_usage_sample(
                    "execution",
                    "request",
                    "claude-code",
                    Some("claude-opus-5-5"),
                    Some("anthropic"),
                    &TokenUsage {
                        input_tokens: Some(10),
                        output_tokens: Some(output),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        let count: i64 = store
            .conn
            .query_row("SELECT count(*) FROM chat_usage_samples", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
        store.finalize_turn_usage("turn", "cancelled").unwrap();
        store
            .record_usage_sample(
                "execution",
                "request",
                "claude-code",
                Some("wrong"),
                None,
                &TokenUsage::default(),
            )
            .unwrap();
        let model: String = store
            .conn
            .query_row("SELECT model FROM chat_usage_samples", [], |row| row.get(0))
            .unwrap();
        assert_eq!(model, "claude-opus-5-5");
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn native_invocations_are_immutable_and_distinct_by_call_id() {
        let dir =
            std::env::temp_dir().join(format!("orx-native-identities-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        let parent = InvocationIdentity {
            harness: "claude-code".into(),
            model: "claude-opus-5-5".into(),
            provider: None,
        };
        let child = InvocationIdentity {
            model: "claude-haiku-4-5-20251001".into(),
            ..parent.clone()
        };
        store
            .record_native_invocation("parent-call", &parent, None)
            .unwrap();
        store
            .record_native_invocation("child-call", &child, None)
            .unwrap();
        assert!(store
            .record_native_invocation("parent-call", &child, None)
            .is_err());
        assert_eq!(
            store
                .native_invocation_identity("claude-code", "parent-call")
                .unwrap(),
            Some(parent)
        );
        assert_eq!(
            store
                .native_invocation_identity("claude-code", "child-call")
                .unwrap(),
            Some(child)
        );
        assert!(!valid_model_label("/Users/person/models/weights.gguf"));
        assert!(!valid_model_label("https://user:secret@example.com/model"));
        assert!(!valid_model_label("C:\\models\\weights.gguf"));
        assert!(!valid_model_label("models\\weights.gguf"));
        assert!(!valid_model_label(
            "arn:aws:bedrock:us-east-1:123456789012:inference-profile/private"
        ));
        assert!(valid_model_label("org/model-v1"));
        store
            .begin_usage_execution("execution", "turn", "opencode")
            .unwrap();
        let usage = TokenUsage {
            input_tokens: Some(2),
            output_tokens: Some(3),
            ..Default::default()
        };
        store
            .record_usage_sample(
                "execution",
                "sample",
                "opencode",
                Some("/private/model.gguf"),
                None,
                &usage,
            )
            .unwrap();
        let (model, counters): (Option<String>, String) = store
            .conn
            .query_row(
                "SELECT model, usage_json FROM chat_usage_samples WHERE execution_id = 'execution'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(model, None);
        assert_eq!(
            serde_json::from_str::<TokenUsage>(&counters).unwrap(),
            usage
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn token_breakdowns_are_subsets_and_unknown_is_not_zero() {
        let usage = TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_read_tokens: Some(80),
            reasoning_tokens: Some(15),
            ..Default::default()
        };
        usage.validate().unwrap();
        assert_eq!(usage.total(), Some(120));
        assert_eq!(TokenUsage::default().total(), None);
        assert!(TokenUsage {
            cache_write_tokens: Some(30),
            ..usage
        }
        .validate()
        .is_err());
    }

    /// Only a native label is exact; reserved labels and missing evidence stay explicit, and a
    /// model seen without tokens never erases measured usage (or the reverse).
    #[test]
    fn native_labels_attribute_and_identity_writes_keep_usage() {
        for (harness, model, sampled, delivery, expected) in [
            ("codex", Some("gpt-6-sol"), true, None, ("exact", None)),
            ("cursor", Some("Auto"), true, None, ("auto_routing", None)),
            (
                "claude-code",
                Some("<synthetic>"),
                true,
                None,
                ("unresolved", Some("synthetic_model")),
            ),
            (
                "codex",
                None,
                true,
                None,
                ("unresolved", Some("identity_not_reported")),
            ),
            (
                "codex",
                None,
                false,
                Some("rejected"),
                ("not_executed", None),
            ),
            (
                "codex",
                None,
                false,
                Some("accepted"),
                ("unresolved", Some("no_usage_reported")),
            ),
        ] {
            assert_eq!(
                chat_attribution(harness, model, sampled, delivery),
                expected
            );
        }
        let dir = std::env::temp_dir().join(format!("orx-identity-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store.begin_usage_execution("e", "t", "codex").unwrap();
        let measured = TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            ..Default::default()
        };
        store
            .record_usage_sample("e", "a", "codex", None, None, &measured)
            .unwrap();
        store
            .record_usage_sample(
                "e",
                "a",
                "codex",
                Some("gpt-6-sol"),
                None,
                &TokenUsage::default(),
            )
            .unwrap();
        store
            .record_usage_sample("e", "a", "codex", None, None, &measured)
            .unwrap();
        let (model, usage): (Option<String>, String) = store
            .conn
            .query_row(
                "SELECT model, usage_json FROM chat_usage_samples",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(model.as_deref(), Some("gpt-6-sol"));
        assert_eq!(
            serde_json::from_str::<TokenUsage>(&usage).unwrap(),
            measured
        );
        // A native cumulative delta with both counters covers its requests whole.
        store
            .record_cumulative_usage(
                "e",
                "codex",
                "th",
                "tu",
                Some("gpt-6-sol"),
                &measured,
                &measured,
            )
            .unwrap();
        let complete: bool = store
            .conn
            .query_row(
                "SELECT complete FROM chat_usage_samples WHERE sample_id LIKE 'th:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(complete);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
