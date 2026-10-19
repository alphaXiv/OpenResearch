//! Optional, versioned research measurements. Keep the core run schema untouched.
//! The table and cleanup trigger travel with normal SQLite backups and are
//! ignored by upstream builds that do not implement this feature.
use std::collections::BTreeMap;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::Store;
use crate::error::{anyhow, Result};

pub const MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunMetrics {
    pub version: u32,
    /// Include the dataset revision here when different revisions exist.
    pub dataset: String,
    pub split: String,
    pub metrics: BTreeMap<String, f64>,
    #[serde(default)]
    pub parameters: BTreeMap<String, Value>,
    pub seed: Option<i64>,
}

impl RunMetrics {
    pub fn validate(&self) -> Result<()> {
        let valid_name =
            |s: &str| !s.trim().is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
        if self.version != 1 {
            return Err(anyhow!("Unsupported metrics version; expected version 1."));
        }
        if self
            .seed
            .is_some_and(|seed| !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&seed))
        {
            return Err(anyhow!(
                "Seed must be an integer within the JSON safe integer range."
            ));
        }
        if !valid_name(&self.dataset) || !valid_name(&self.split) {
            return Err(anyhow!(
                "Provide a dataset (including revision) and evaluation split."
            ));
        }
        if self.metrics.is_empty()
            || self.metrics.len() > 128
            || self
                .metrics
                .iter()
                .any(|(k, v)| !valid_name(k) || !v.is_finite())
        {
            return Err(anyhow!("Provide 1–128 named, finite numeric metrics."));
        }
        if self.parameters.len() > 128
            || self.parameters.iter().any(|(k, v)| {
                !valid_name(k)
                    || k.eq_ignore_ascii_case("seed")
                    || !matches!(
                        v,
                        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
                    )
            })
        {
            return Err(anyhow!(
                "Parameters must be named scalar values; put seed in the top-level seed field."
            ));
        }
        if serde_json::to_vec(self)?.len() > MAX_BYTES {
            return Err(anyhow!("Metrics exceed the 64 KiB limit."));
        }
        Ok(())
    }
}

impl Store {
    fn ensure_run_metrics(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS orx_run_metrics_v1 (
                run_id TEXT PRIMARY KEY NOT NULL,
                payload TEXT NOT NULL
            );
            CREATE TRIGGER IF NOT EXISTS orx_run_metrics_v1_cleanup
            AFTER DELETE ON runs BEGIN
                DELETE FROM orx_run_metrics_v1 WHERE run_id = OLD.id;
            END;",
        )?;
        Ok(())
    }

    pub fn put_run_metrics(&self, run_id: &str, metrics: &RunMetrics, replace: bool) -> Result<()> {
        metrics.validate()?;
        self.ensure_run_metrics()?;
        // One transaction prevents a concurrent run deletion or import from
        // leaving orphaned measurements or silently overwriting another import.
        let tx = self.conn.unchecked_transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1)",
            [run_id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(anyhow!("Run not found."));
        }
        let payload = serde_json::to_string(metrics)?;
        let sql = if replace {
            "INSERT INTO orx_run_metrics_v1(run_id, payload) VALUES (?1, ?2)
             ON CONFLICT(run_id) DO UPDATE SET payload = excluded.payload"
        } else {
            "INSERT INTO orx_run_metrics_v1(run_id, payload) VALUES (?1, ?2) ON CONFLICT(run_id) DO NOTHING"
        };
        if tx.execute(sql, params![run_id, payload])? == 0 {
            return Err(anyhow!(
                "Metrics already exist for this run. Use replace explicitly to overwrite."
            ));
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get_run_metrics(&self, run_id: &str) -> Result<Option<RunMetrics>> {
        self.ensure_run_metrics()?;
        let payload: Option<String> = self
            .conn
            .query_row(
                "SELECT payload FROM orx_run_metrics_v1 WHERE run_id = ?1",
                [run_id],
                |r| r.get(0),
            )
            .optional()?;
        payload
            .map(|v| serde_json::from_str(&v).map_err(Into::into))
            .transpose()
    }

    pub fn project_run_metrics(&self, project_id: &str) -> Result<BTreeMap<String, RunMetrics>> {
        self.ensure_run_metrics()?;
        let mut statement = self.conn.prepare(
            "SELECT m.run_id, m.payload FROM orx_run_metrics_v1 m
             JOIN runs r ON r.id = m.run_id WHERE r.project_id = ?1",
        )?;
        let rows = statement.query_map([project_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (id, payload) = row?;
            Ok((id, serde_json::from_str(&payload)?))
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> RunMetrics {
        serde_json::from_value(json!({"version":1,"dataset":"sample-v1","split":"test","metrics":{"f1":0.8},"parameters":{"lr":0.001},"seed":42})).unwrap()
    }

    #[test]
    fn rejects_invalid_or_ambiguous_measurements() {
        let mut data = fixture();
        assert!(data.validate().is_ok());
        data.version = 2;
        assert!(data.validate().is_err());
        data.version = 1;
        data.metrics.insert("loss".into(), f64::NAN);
        assert!(data.validate().is_err());
        data = fixture();
        data.parameters.insert("seed".into(), json!(12));
        assert!(data.validate().is_err());
        data = fixture();
        data.parameters.insert("nested".into(), json!({"a":1}));
        assert!(data.validate().is_err());
        data = fixture();
        data.seed = Some(9_007_199_254_740_992);
        assert!(data.validate().is_err());
        data.seed = Some(-9_007_199_254_740_991);
        assert!(data.validate().is_ok());
        data = fixture();
        data.dataset.clear();
        assert!(data.validate().is_err());
    }

    #[test]
    fn metrics_are_optional_scoped_atomic_and_deleted_with_runs() {
        let root = std::env::temp_dir().join(format!("orx-metrics-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(root.clone()).unwrap();
            assert!(store.project_run_metrics("p1").unwrap().is_empty());
            assert!(store.put_run_metrics("missing", &fixture(), false).is_err());
            store.conn.execute("INSERT INTO runs (id, experiment_id, project_id, status, backend_json, created_at, updated_at) VALUES ('r1', 'e1', 'p1', 'done', '{}', 0, 0)", []).unwrap();
            store.put_run_metrics("r1", &fixture(), false).unwrap();
            assert!(store.put_run_metrics("r1", &fixture(), false).is_err());
            assert!(store.project_run_metrics("p2").unwrap().is_empty());
            assert_eq!(store.get_run_metrics("r1").unwrap(), Some(fixture()));
            let mut changed = fixture();
            changed.metrics.insert("f1".into(), 0.9);
            store.put_run_metrics("r1", &changed, true).unwrap();
            assert_eq!(store.project_run_metrics("p1").unwrap()["r1"], changed);
            changed.metrics.clear();
            assert!(store.put_run_metrics("r1", &changed, true).is_err());
            assert_eq!(
                store.get_run_metrics("r1").unwrap().unwrap().metrics["f1"],
                0.9
            );
            store
                .conn
                .execute("DELETE FROM runs WHERE id = 'r1'", [])
                .unwrap();
            assert!(store.get_run_metrics("r1").unwrap().is_none());
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
