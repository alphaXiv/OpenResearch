//! Per-account harness usage: what orx saw each account spend, the limits
//! providers reported, and which account each chat sticks to. Tables are
//! created on first use so builds without accounts never touch them.

use rusqlite::{params, OptionalExtension};

use super::Store;
use crate::error::Result;

/// One usage window a provider reported for an account's quota pool.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaSnapshot {
    pub harness: String,
    pub account: String,
    pub pool: String,
    /// `5h`, `weekly`, … as the provider names the window.
    pub window: String,
    pub used_percent: f64,
    pub resets_at_ms: Option<i64>,
    pub read_at_ms: i64,
}

/// Usage orx recorded for one account and pool since a point in time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrackedUsage {
    pub turns: u64,
    pub tokens: u64,
}

impl Store {
    fn ensure_harness_accounts(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS harness_account_usage (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness TEXT NOT NULL,
                account TEXT NOT NULL,
                pool TEXT NOT NULL,
                model TEXT,
                at_ms INTEGER NOT NULL,
                tokens INTEGER NOT NULL DEFAULT 0,
                outcome TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS harness_account_usage_by_pool
                ON harness_account_usage (harness, account, pool, at_ms);
            CREATE TABLE IF NOT EXISTS harness_account_limits (
                harness TEXT NOT NULL,
                account TEXT NOT NULL,
                pool TEXT NOT NULL,
                until_ms INTEGER NOT NULL,
                reason TEXT,
                PRIMARY KEY (harness, account, pool)
            );
            CREATE TABLE IF NOT EXISTS harness_account_quota (
                harness TEXT NOT NULL,
                account TEXT NOT NULL,
                pool TEXT NOT NULL,
                window TEXT NOT NULL,
                used_percent REAL NOT NULL,
                resets_at_ms INTEGER,
                read_at_ms INTEGER NOT NULL,
                PRIMARY KEY (harness, account, pool, window)
            );
            CREATE TABLE IF NOT EXISTS chat_session_accounts (
                session_id TEXT PRIMARY KEY,
                harness TEXT NOT NULL,
                account TEXT NOT NULL
            );
            CREATE TRIGGER IF NOT EXISTS chat_session_accounts_cleanup
            AFTER DELETE ON chat_sessions BEGIN
                DELETE FROM chat_session_accounts WHERE session_id = OLD.id;
            END;",
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_account_usage(
        &self,
        harness: &str,
        account: &str,
        pool: &str,
        model: Option<&str>,
        at_ms: i64,
        tokens: u64,
        outcome: &str,
    ) -> Result<()> {
        self.ensure_harness_accounts()?;
        self.conn.execute(
            "INSERT INTO harness_account_usage (harness, account, pool, model, at_ms, tokens, outcome)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![harness, account, pool, model, at_ms, tokens as i64, outcome],
        )?;
        // Nothing reads usage older than the longest window anyone sets.
        self.conn.execute(
            "DELETE FROM harness_account_usage WHERE at_ms < ?1",
            [at_ms - 45 * 24 * 3600 * 1000],
        )?;
        Ok(())
    }

    pub(crate) fn account_usage_since(
        &self,
        harness: &str,
        account: &str,
        pool: &str,
        since_ms: i64,
    ) -> Result<TrackedUsage> {
        self.ensure_harness_accounts()?;
        let (turns, tokens): (i64, i64) = self.conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(tokens), 0) FROM harness_account_usage
             WHERE harness = ?1 AND account = ?2 AND pool = ?3 AND at_ms >= ?4",
            params![harness, account, pool, since_ms],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(TrackedUsage {
            turns: turns.max(0) as u64,
            tokens: tokens.max(0) as u64,
        })
    }

    /// Input and output tokens the harness reported for one turn attempt.
    pub(crate) fn usage_execution_tokens(&self, execution_id: &str) -> Result<u64> {
        let mut stmt = self
            .conn
            .prepare("SELECT usage_json FROM chat_usage_samples WHERE execution_id = ?1")?;
        let rows = stmt.query_map([execution_id], |row| row.get::<_, String>(0))?;
        let mut total = 0u64;
        for row in rows {
            if let Ok(usage) = serde_json::from_str::<super::TokenUsage>(&row?) {
                total = total
                    .saturating_add(usage.input_tokens.unwrap_or(0))
                    .saturating_add(usage.output_tokens.unwrap_or(0));
            }
        }
        Ok(total)
    }

    pub(crate) fn set_account_limit(
        &self,
        harness: &str,
        account: &str,
        pool: &str,
        until_ms: i64,
        reason: Option<&str>,
    ) -> Result<()> {
        self.ensure_harness_accounts()?;
        self.conn.execute(
            "INSERT INTO harness_account_limits (harness, account, pool, until_ms, reason)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(harness, account, pool) DO UPDATE
             SET until_ms = excluded.until_ms, reason = excluded.reason",
            params![harness, account, pool, until_ms, reason],
        )?;
        Ok(())
    }

    pub(crate) fn clear_account_limits(&self, harness: &str, account: &str) -> Result<()> {
        self.ensure_harness_accounts()?;
        self.conn.execute(
            "DELETE FROM harness_account_limits WHERE harness = ?1 AND account = ?2",
            params![harness, account],
        )?;
        Ok(())
    }

    /// When `account`'s `pool` may be used again, if a limit stopped it.
    pub(crate) fn account_limit(
        &self,
        harness: &str,
        account: &str,
        pool: &str,
    ) -> Result<Option<(i64, Option<String>)>> {
        self.ensure_harness_accounts()?;
        Ok(self
            .conn
            .query_row(
                "SELECT until_ms, reason FROM harness_account_limits
                 WHERE harness = ?1 AND account = ?2 AND pool = ?3",
                params![harness, account, pool],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// Replace what the provider last reported for one account.
    pub(crate) fn replace_quota_snapshots(
        &self,
        harness: &str,
        account: &str,
        snapshots: &[QuotaSnapshot],
    ) -> Result<()> {
        self.ensure_harness_accounts()?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM harness_account_quota WHERE harness = ?1 AND account = ?2",
            params![harness, account],
        )?;
        for snapshot in snapshots {
            tx.execute(
                "INSERT OR REPLACE INTO harness_account_quota
                 (harness, account, pool, window, used_percent, resets_at_ms, read_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    harness,
                    account,
                    snapshot.pool,
                    snapshot.window,
                    snapshot.used_percent,
                    snapshot.resets_at_ms,
                    snapshot.read_at_ms
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn quota_snapshots(
        &self,
        harness: &str,
        account: &str,
    ) -> Result<Vec<QuotaSnapshot>> {
        self.ensure_harness_accounts()?;
        let mut stmt = self.conn.prepare(
            "SELECT pool, window, used_percent, resets_at_ms, read_at_ms FROM harness_account_quota
             WHERE harness = ?1 AND account = ?2 ORDER BY pool, window",
        )?;
        let rows = stmt.query_map(params![harness, account], |row| {
            Ok(QuotaSnapshot {
                harness: harness.to_string(),
                account: account.to_string(),
                pool: row.get(0)?,
                window: row.get(1)?,
                used_percent: row.get(2)?,
                resets_at_ms: row.get(3)?,
                read_at_ms: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Forget everything recorded for an account that was removed.
    pub(crate) fn forget_account(&self, harness: &str, account: &str) -> Result<()> {
        self.ensure_harness_accounts()?;
        for table in [
            "harness_account_usage",
            "harness_account_limits",
            "harness_account_quota",
            "chat_session_accounts",
        ] {
            self.conn.execute(
                &format!("DELETE FROM {table} WHERE harness = ?1 AND account = ?2"),
                params![harness, account],
            )?;
        }
        Ok(())
    }

    /// The account a chat last ran `harness` on.
    pub(crate) fn chat_session_account(
        &self,
        session_id: &str,
        harness: &str,
    ) -> Result<Option<String>> {
        self.ensure_harness_accounts()?;
        Ok(self
            .conn
            .query_row(
                "SELECT account FROM chat_session_accounts WHERE session_id = ?1 AND harness = ?2",
                params![session_id, harness],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn set_chat_session_account(
        &self,
        session_id: &str,
        harness: &str,
        account: &str,
    ) -> Result<()> {
        self.ensure_harness_accounts()?;
        self.conn.execute(
            "INSERT INTO chat_session_accounts (session_id, harness, account) VALUES (?1, ?2, ?3)
             ON CONFLICT(session_id) DO UPDATE SET harness = excluded.harness, account = excluded.account",
            params![session_id, harness, account],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_limits_and_snapshots_round_trip() {
        let dir = std::env::temp_dir().join(format!("orx-accounts-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .record_account_usage("codex", "work", "openai", Some("gpt"), 1_000, 50, "done")
            .unwrap();
        store
            .record_account_usage("codex", "work", "openai", None, 2_000, 25, "failed")
            .unwrap();
        store
            .record_account_usage("codex", "home", "openai", None, 2_000, 99, "done")
            .unwrap();
        assert_eq!(
            store
                .account_usage_since("codex", "work", "openai", 1_500)
                .unwrap(),
            TrackedUsage {
                turns: 1,
                tokens: 25
            }
        );
        store
            .set_account_limit("codex", "work", "openai", 9_000, Some("limit"))
            .unwrap();
        assert_eq!(
            store.account_limit("codex", "work", "openai").unwrap(),
            Some((9_000, Some("limit".into())))
        );
        let snapshot = QuotaSnapshot {
            harness: "codex".into(),
            account: "work".into(),
            pool: "openai".into(),
            window: "5h".into(),
            used_percent: 40.0,
            resets_at_ms: Some(10),
            read_at_ms: 5,
        };
        store
            .replace_quota_snapshots("codex", "work", std::slice::from_ref(&snapshot))
            .unwrap();
        assert_eq!(
            store.quota_snapshots("codex", "work").unwrap(),
            vec![snapshot]
        );
        store
            .set_chat_session_account("s1", "codex", "work")
            .unwrap();
        assert_eq!(
            store
                .chat_session_account("s1", "codex")
                .unwrap()
                .as_deref(),
            Some("work")
        );
        assert_eq!(
            store.chat_session_account("s1", "claude-code").unwrap(),
            None
        );
        store.forget_account("codex", "work").unwrap();
        assert_eq!(
            store.account_limit("codex", "work", "openai").unwrap(),
            None
        );
        assert!(store.quota_snapshots("codex", "work").unwrap().is_empty());
        assert_eq!(store.chat_session_account("s1", "codex").unwrap(), None);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
