use super::*;

/// An agent account that refused a turn on its usage limit, until the limit resets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimit {
    pub harness: String,
    /// The agent profile's id; None for the user's own sign-in.
    pub profile: Option<String>,
    /// Epoch ms.
    pub reset_at: i64,
    /// When the account first refused a turn on this limit (epoch ms).
    #[serde(skip)]
    pub since: i64,
}

impl Store {
    /// Records the account's limit, refused at `since`; a later refusal moves its reset and keeps
    /// when it began.
    pub fn record_usage_limit(
        &self,
        harness: &str,
        profile: Option<&str>,
        reset_at: i64,
        since: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO usage_limits (harness, profile, reset_at, since)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (harness, profile) DO UPDATE SET reset_at = excluded.reset_at",
            params![harness, profile.unwrap_or_default(), reset_at, since],
        )?;
        Ok(())
    }

    /// Forgets the account's limit, only the one resetting at `reset_at` when given, as when it
    /// served a turn or spent a reset. The limit it forgot, if any.
    pub fn clear_usage_limit(
        &self,
        harness: &str,
        profile: Option<&str>,
        reset_at: Option<i64>,
    ) -> Result<Option<UsageLimit>> {
        Ok(self
            .conn
            .query_row(
                "DELETE FROM usage_limits
                 WHERE harness = ?1 AND profile = ?2 AND (?3 IS NULL OR reset_at = ?3)
                 RETURNING harness, profile, reset_at, since",
                params![harness, profile.unwrap_or_default(), reset_at],
                usage_limit_from_row,
            )
            .optional()?)
    }

    /// Every recorded limit, including those whose reset has passed but whose held chats have
    /// not been sent yet.
    pub fn usage_limits(&self) -> Result<Vec<UsageLimit>> {
        let mut statement = self.conn.prepare(
            "SELECT harness, profile, reset_at, since FROM usage_limits ORDER BY reset_at",
        )?;
        let rows = statement.query_map([], usage_limit_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn usage_limit(&self, harness: &str, profile: Option<&str>) -> Result<Option<UsageLimit>> {
        Ok(self
            .usage_limits()?
            .into_iter()
            .find(|limit| limit.harness == harness && limit.profile.as_deref() == profile))
    }

    /// Stops or restarts the chat's wait for its account's reset: while stopped, nothing the
    /// chat holds is sent until the user sends it.
    pub fn set_chat_limit_timer_stopped(&self, session_id: &str, stopped: bool) -> Result<()> {
        if stopped {
            self.conn.execute(
                "INSERT OR IGNORE INTO chat_limit_timer_stops (session_id, stopped_at)
                 VALUES (?1, ?2)",
                params![session_id, now_ms()],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM chat_limit_timer_stops WHERE session_id = ?1",
                [session_id],
            )?;
        }
        Ok(())
    }

    pub fn chat_limit_timer_stopped(&self, session_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM chat_limit_timer_stops WHERE session_id = ?1)",
            [session_id],
            |row| row.get(0),
        )?)
    }
}

fn usage_limit_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageLimit> {
    let profile: String = row.get(1)?;
    Ok(UsageLimit {
        harness: row.get(0)?,
        profile: (!profile.is_empty()).then_some(profile),
        reset_at: row.get(2)?,
        since: row.get(3)?,
    })
}
