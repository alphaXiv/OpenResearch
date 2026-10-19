//! Sleep-mode plans and the GPU ledger behind their hard budget. Like the
//! run metrics, the tables live beside the core schema and are created on
//! first use, so builds without sleep mode ignore them.
use rusqlite::{params, OptionalExtension};

use super::{now_ms, Store};
use crate::error::Result;
use crate::local::sleep::{SleepPlan, SleepState};

#[derive(Debug, Clone, PartialEq)]
pub struct StoredSleep {
    pub session_id: String,
    pub project_id: String,
    pub plan: SleepPlan,
    pub state: SleepState,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What one run is charged: its hourly rate from the moment it was submitted.
#[derive(Debug, Clone, PartialEq)]
pub struct SleepCharge {
    pub run_id: String,
    pub usd_per_hour: f64,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub status: String,
}

impl Store {
    fn ensure_sleep(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS orx_sleep_v1 (
                session_id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL,
                plan TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS orx_sleep_charges_v1 (
                run_id TEXT PRIMARY KEY NOT NULL,
                session_id TEXT NOT NULL,
                usd_per_hour REAL NOT NULL,
                started_at INTEGER NOT NULL
            );
            CREATE TRIGGER IF NOT EXISTS orx_sleep_v1_cleanup
            AFTER DELETE ON chat_sessions BEGIN
                DELETE FROM orx_sleep_v1 WHERE session_id = OLD.id;
                DELETE FROM orx_sleep_charges_v1 WHERE session_id = OLD.id;
            END;",
        )?;
        Ok(())
    }

    /// Starts or replaces the session's plan. A new plan starts a fresh ledger.
    pub fn put_sleep(&self, session_id: &str, project_id: &str, plan: &SleepPlan) -> Result<()> {
        self.ensure_sleep()?;
        let now = now_ms();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM orx_sleep_charges_v1 WHERE session_id = ?1",
            [session_id],
        )?;
        tx.execute(
            "INSERT INTO orx_sleep_v1(session_id, project_id, plan, state, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(session_id) DO UPDATE SET project_id = excluded.project_id,
               plan = excluded.plan, state = excluded.state,
               created_at = excluded.created_at, updated_at = excluded.updated_at",
            params![
                session_id,
                project_id,
                serde_json::to_string(plan)?,
                serde_json::to_string(&SleepState::default())?,
                now
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Changes the plan in place, keeping the ledger and progress.
    pub fn update_sleep_plan(&self, session_id: &str, plan: &SleepPlan) -> Result<bool> {
        self.ensure_sleep()?;
        let changed = self.conn.execute(
            "UPDATE orx_sleep_v1 SET plan = ?2, updated_at = ?3 WHERE session_id = ?1",
            params![session_id, serde_json::to_string(plan)?, now_ms()],
        )?;
        Ok(changed == 1)
    }

    pub fn set_sleep_state(&self, session_id: &str, state: &SleepState) -> Result<bool> {
        self.ensure_sleep()?;
        let changed = self.conn.execute(
            "UPDATE orx_sleep_v1 SET state = ?2, updated_at = ?3 WHERE session_id = ?1",
            params![session_id, serde_json::to_string(state)?, now_ms()],
        )?;
        Ok(changed == 1)
    }

    pub fn get_sleep(&self, session_id: &str) -> Result<Option<StoredSleep>> {
        self.ensure_sleep()?;
        self.conn
            .query_row(
                "SELECT session_id, project_id, plan, state, created_at, updated_at
                 FROM orx_sleep_v1 WHERE session_id = ?1",
                [session_id],
                row_to_sleep,
            )
            .optional()?
            .transpose()
    }

    pub fn list_sleeps(&self) -> Result<Vec<StoredSleep>> {
        self.ensure_sleep()?;
        let mut statement = self.conn.prepare(
            "SELECT session_id, project_id, plan, state, created_at, updated_at
             FROM orx_sleep_v1 ORDER BY created_at",
        )?;
        let rows = statement.query_map([], row_to_sleep)?;
        rows.map(|row| row?).collect()
    }

    pub fn delete_sleep(&self, session_id: &str) -> Result<bool> {
        self.ensure_sleep()?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM orx_sleep_charges_v1 WHERE session_id = ?1",
            [session_id],
        )?;
        let deleted = tx.execute(
            "DELETE FROM orx_sleep_v1 WHERE session_id = ?1",
            [session_id],
        )?;
        tx.commit()?;
        Ok(deleted == 1)
    }

    pub fn record_sleep_charge(
        &self,
        session_id: &str,
        run_id: &str,
        usd_per_hour: f64,
        started_at: i64,
    ) -> Result<()> {
        self.ensure_sleep()?;
        self.conn.execute(
            "INSERT INTO orx_sleep_charges_v1(run_id, session_id, usd_per_hour, started_at)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT(run_id) DO NOTHING",
            params![run_id, session_id, usd_per_hour, started_at],
        )?;
        Ok(())
    }

    /// The session's charges with each run's current end and status. A run
    /// deleted from the store keeps being charged up to now, never less.
    pub fn sleep_charges(&self, session_id: &str) -> Result<Vec<SleepCharge>> {
        self.ensure_sleep()?;
        let mut statement = self.conn.prepare(
            "SELECT c.run_id, c.usd_per_hour, c.started_at, r.ended_at, COALESCE(r.status, 'running')
             FROM orx_sleep_charges_v1 c LEFT JOIN runs r ON r.id = c.run_id
             WHERE c.session_id = ?1 ORDER BY c.started_at",
        )?;
        let rows = statement.query_map([session_id], |r| {
            Ok(SleepCharge {
                run_id: r.get(0)?,
                usd_per_hour: r.get(1)?,
                started_at: r.get(2)?,
                ended_at: r.get(3)?,
                status: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

fn row_to_sleep(r: &rusqlite::Row<'_>) -> rusqlite::Result<Result<StoredSleep>> {
    let plan: String = r.get(2)?;
    let state: String = r.get(3)?;
    let session_id: String = r.get(0)?;
    let project_id: String = r.get(1)?;
    let created_at: i64 = r.get(4)?;
    let updated_at: i64 = r.get(5)?;
    Ok((|| {
        Ok(StoredSleep {
            session_id,
            project_id,
            plan: serde_json::from_str(&plan)?,
            // A state written by a newer build restarts from scratch rather than
            // stopping the watchdog.
            state: serde_json::from_str(&state).unwrap_or_default(),
            created_at,
            updated_at,
        })
    })())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_ledgers_and_cleanup_roundtrip() {
        let root = std::env::temp_dir().join(format!("orx-sleep-{}", uuid::Uuid::new_v4()));
        {
            let store = Store::open_at(root.clone()).unwrap();
            assert!(store.get_sleep("chat_a").unwrap().is_none());
            let plan = SleepPlan {
                gpu_budget_usd: 5.0,
                ..SleepPlan::default()
            };
            store.put_sleep("chat_a", "p1", &plan).unwrap();
            store.record_sleep_charge("chat_a", "r1", 2.0, 0).unwrap();
            store.record_sleep_charge("chat_a", "r1", 9.0, 0).unwrap();
            let charges = store.sleep_charges("chat_a").unwrap();
            assert_eq!(charges.len(), 1);
            assert_eq!(charges[0].usd_per_hour, 2.0);
            assert_eq!(charges[0].status, "running");

            let state = SleepState {
                nudges: 3,
                ..SleepState::default()
            };
            assert!(store.set_sleep_state("chat_a", &state).unwrap());
            let stored = store.get_sleep("chat_a").unwrap().unwrap();
            assert_eq!(stored.plan, plan);
            assert_eq!(stored.state.nudges, 3);
            assert_eq!(store.list_sleeps().unwrap().len(), 1);

            // Restarting sleep mode starts a fresh ledger.
            store.put_sleep("chat_a", "p1", &plan).unwrap();
            assert!(store.sleep_charges("chat_a").unwrap().is_empty());
            assert_eq!(store.get_sleep("chat_a").unwrap().unwrap().state.nudges, 0);

            assert!(store.delete_sleep("chat_a").unwrap());
            assert!(store.get_sleep("chat_a").unwrap().is_none());
            assert!(!store.set_sleep_state("chat_a", &state).unwrap());
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
