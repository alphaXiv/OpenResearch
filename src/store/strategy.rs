//! Strategy versions for self-improvement: the strategy an agent follows in
//! sleep mode, the runs each version launched, and the log of every change.
//! Like the sleep tables, they live beside the core schema and are created on
//! first use.
use rusqlite::{params, OptionalExtension};

use super::{now_ms, Store};
use crate::error::Result;
use crate::local::strategy::{Strategy, VersionStatus};

#[derive(Debug, Clone, PartialEq)]
pub struct StoredStrategy {
    pub project_id: String,
    pub version: u32,
    pub parent: Option<u32>,
    pub status: VersionStatus,
    pub strategy: Strategy,
    pub rationale: String,
    /// `default`, `agent`, or `user`.
    pub author: String,
    pub created_at: i64,
    pub decided_at: Option<i64>,
    pub decision: Option<String>,
}

/// Which version a run was launched under, and on which arm of a trial.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyRun {
    pub run_id: String,
    pub version: u32,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StrategyEvent {
    pub at: i64,
    pub kind: String,
    pub version: u32,
    pub detail: String,
}

/// The version a sleep chat's current turn runs under.
#[derive(Debug, Clone, PartialEq)]
pub struct StrategyArm {
    pub version: u32,
    pub turns: u32,
}

impl Store {
    fn ensure_strategies(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS orx_strategies_v1 (
                project_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                parent INTEGER,
                status TEXT NOT NULL,
                strategy TEXT NOT NULL,
                rationale TEXT NOT NULL,
                author TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                decided_at INTEGER,
                decision TEXT,
                PRIMARY KEY (project_id, version)
            );
            CREATE TABLE IF NOT EXISTS orx_strategy_runs_v1 (
                run_id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS orx_strategy_arms_v1 (
                session_id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                turns INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS orx_strategy_events_v1 (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                project_id TEXT NOT NULL,
                at INTEGER NOT NULL,
                kind TEXT NOT NULL,
                version INTEGER NOT NULL,
                detail TEXT NOT NULL
            );
            CREATE TRIGGER IF NOT EXISTS orx_strategies_v1_cleanup
            AFTER DELETE ON local_projects BEGIN
                DELETE FROM orx_strategies_v1 WHERE project_id = OLD.id;
                DELETE FROM orx_strategy_runs_v1 WHERE project_id = OLD.id;
                DELETE FROM orx_strategy_arms_v1 WHERE project_id = OLD.id;
                DELETE FROM orx_strategy_events_v1 WHERE project_id = OLD.id;
            END;
            CREATE TRIGGER IF NOT EXISTS orx_strategy_arms_v1_cleanup
            AFTER DELETE ON chat_sessions BEGIN
                DELETE FROM orx_strategy_arms_v1 WHERE session_id = OLD.id;
            END;",
        )?;
        Ok(())
    }

    /// Every version of the project's strategy, oldest first.
    pub fn strategy_versions(&self, project_id: &str) -> Result<Vec<StoredStrategy>> {
        self.ensure_strategies()?;
        let mut statement = self.conn.prepare(
            "SELECT project_id, version, parent, status, strategy, rationale, author,
                    created_at, decided_at, decision
             FROM orx_strategies_v1 WHERE project_id = ?1 ORDER BY version",
        )?;
        let rows = statement.query_map([project_id], row_to_strategy)?;
        rows.map(|row| row?).collect()
    }

    pub fn get_strategy_version(
        &self,
        project_id: &str,
        version: u32,
    ) -> Result<Option<StoredStrategy>> {
        self.ensure_strategies()?;
        self.conn
            .query_row(
                "SELECT project_id, version, parent, status, strategy, rationale, author,
                        created_at, decided_at, decision
                 FROM orx_strategies_v1 WHERE project_id = ?1 AND version = ?2",
                params![project_id, version],
                row_to_strategy,
            )
            .optional()?
            .transpose()
    }

    /// Adds the next version and returns its number.
    pub fn insert_strategy_version(
        &self,
        project_id: &str,
        parent: Option<u32>,
        status: VersionStatus,
        strategy: &Strategy,
        rationale: &str,
        author: &str,
    ) -> Result<u32> {
        self.ensure_strategies()?;
        let tx = self.conn.unchecked_transaction()?;
        let next: u32 = tx.query_row(
            "SELECT COALESCE(MAX(version) + 1, 0) FROM orx_strategies_v1 WHERE project_id = ?1",
            [project_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO orx_strategies_v1(project_id, version, parent, status, strategy,
                rationale, author, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project_id,
                next,
                parent,
                status.id(),
                serde_json::to_string(strategy)?,
                rationale,
                author,
                now_ms()
            ],
        )?;
        tx.commit()?;
        Ok(next)
    }

    /// Moves a version to `status`, recording why when it is a decision.
    pub fn set_strategy_status(
        &self,
        project_id: &str,
        version: u32,
        status: VersionStatus,
        decision: Option<&str>,
    ) -> Result<bool> {
        self.ensure_strategies()?;
        let changed = self.conn.execute(
            "UPDATE orx_strategies_v1 SET status = ?3,
                decided_at = CASE WHEN ?4 IS NULL THEN decided_at ELSE ?5 END,
                decision = COALESCE(?4, decision)
             WHERE project_id = ?1 AND version = ?2",
            params![project_id, version, status.id(), decision, now_ms()],
        )?;
        Ok(changed == 1)
    }

    pub fn record_strategy_run(
        &self,
        project_id: &str,
        run_id: &str,
        version: u32,
        created_at: i64,
    ) -> Result<()> {
        self.ensure_strategies()?;
        self.conn.execute(
            "INSERT INTO orx_strategy_runs_v1(run_id, project_id, version, created_at)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT(run_id) DO NOTHING",
            params![run_id, project_id, version, created_at],
        )?;
        Ok(())
    }

    pub fn strategy_runs(&self, project_id: &str) -> Result<Vec<StrategyRun>> {
        self.ensure_strategies()?;
        let mut statement = self.conn.prepare(
            "SELECT run_id, version, created_at FROM orx_strategy_runs_v1
             WHERE project_id = ?1 ORDER BY created_at",
        )?;
        let rows = statement.query_map([project_id], |r| {
            Ok(StrategyRun {
                run_id: r.get(0)?,
                version: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn get_strategy_arm(&self, session_id: &str) -> Result<Option<StrategyArm>> {
        self.ensure_strategies()?;
        Ok(self
            .conn
            .query_row(
                "SELECT version, turns FROM orx_strategy_arms_v1 WHERE session_id = ?1",
                [session_id],
                |r| {
                    Ok(StrategyArm {
                        version: r.get(0)?,
                        turns: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn set_strategy_arm(
        &self,
        session_id: &str,
        project_id: &str,
        arm: &StrategyArm,
    ) -> Result<()> {
        self.ensure_strategies()?;
        self.conn.execute(
            "INSERT INTO orx_strategy_arms_v1(session_id, project_id, version, turns, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id) DO UPDATE SET project_id = excluded.project_id,
               version = excluded.version, turns = excluded.turns,
               updated_at = excluded.updated_at",
            params![session_id, project_id, arm.version, arm.turns, now_ms()],
        )?;
        Ok(())
    }

    pub fn log_strategy_event(
        &self,
        project_id: &str,
        kind: &str,
        version: u32,
        detail: &str,
    ) -> Result<()> {
        self.ensure_strategies()?;
        self.conn.execute(
            "INSERT INTO orx_strategy_events_v1(project_id, at, kind, version, detail)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, now_ms(), kind, version, detail],
        )?;
        Ok(())
    }

    /// The project's change log, oldest first.
    pub fn strategy_events(&self, project_id: &str) -> Result<Vec<StrategyEvent>> {
        self.ensure_strategies()?;
        let mut statement = self.conn.prepare(
            "SELECT at, kind, version, detail FROM orx_strategy_events_v1
             WHERE project_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([project_id], |r| {
            Ok(StrategyEvent {
                at: r.get(0)?,
                kind: r.get(1)?,
                version: r.get(2)?,
                detail: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

fn row_to_strategy(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<StoredStrategy>> {
    let project_id: String = r.get(0)?;
    let version: u32 = r.get(1)?;
    let parent: Option<u32> = r.get(2)?;
    let status: String = r.get(3)?;
    let strategy: String = r.get(4)?;
    let rationale: String = r.get(5)?;
    let author: String = r.get(6)?;
    let created_at: i64 = r.get(7)?;
    let decided_at: Option<i64> = r.get(8)?;
    let decision: Option<String> = r.get(9)?;
    Ok((|| {
        Ok(StoredStrategy {
            project_id,
            version,
            parent,
            // An unknown status from a newer build reads as retired, so it never
            // runs by accident.
            status: VersionStatus::from_id(&status).unwrap_or(VersionStatus::Retired),
            strategy: serde_json::from_str(&strategy)?,
            rationale,
            author,
            created_at,
            decided_at,
            decision,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_runs_arms_and_events_roundtrip() {
        let root = std::env::temp_dir().join(format!("orx-strategy-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(root.clone()).unwrap();
            assert!(store.strategy_versions("p1").unwrap().is_empty());
            let base = Strategy::default();
            let v0 = store
                .insert_strategy_version("p1", None, VersionStatus::Incumbent, &base, "", "default")
                .unwrap();
            let changed = Strategy {
                approach: "Try optimizers first.".into(),
                ..Strategy::default()
            };
            let v1 = store
                .insert_strategy_version(
                    "p1",
                    Some(v0),
                    VersionStatus::Candidate,
                    &changed,
                    "optimizer runs won twice",
                    "agent",
                )
                .unwrap();
            assert_eq!((v0, v1), (0, 1));
            assert!(store
                .set_strategy_status("p1", v1, VersionStatus::Rejected, Some("lost 0/4"))
                .unwrap());
            let stored = store.get_strategy_version("p1", v1).unwrap().unwrap();
            assert_eq!(stored.status, VersionStatus::Rejected);
            assert_eq!(stored.decision.as_deref(), Some("lost 0/4"));
            assert_eq!(stored.strategy, changed);
            assert_eq!(stored.parent, Some(0));

            store.record_strategy_run("p1", "r1", v1, 5).unwrap();
            store.record_strategy_run("p1", "r1", v0, 5).unwrap();
            assert_eq!(store.strategy_runs("p1").unwrap()[0].version, v1);

            let arm = StrategyArm {
                version: 1,
                turns: 3,
            };
            store.set_strategy_arm("chat", "p1", &arm).unwrap();
            assert_eq!(store.get_strategy_arm("chat").unwrap(), Some(arm));

            store.log_strategy_event("p1", "proposed", 1, "x").unwrap();
            store.log_strategy_event("p1", "rejected", 1, "y").unwrap();
            let events = store.strategy_events("p1").unwrap();
            assert_eq!(
                events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
                ["proposed", "rejected"]
            );
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
