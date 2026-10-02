use super::*;
use crate::local::acp::Definition;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpSessionState {
    pub launch: Definition,
    pub configuration: serde_json::Value,
}

impl Store {
    pub fn acp_session_state(&self, session_id: &str) -> Result<Option<AcpSessionState>> {
        let saved: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT launch_json, configuration_json FROM acp_sessions WHERE session_id = ?1",
                [session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        saved
            .map(|(launch, configuration)| {
                Ok(AcpSessionState {
                    launch: serde_json::from_str(&launch)?,
                    configuration: serde_json::from_str(&configuration)?,
                })
            })
            .transpose()
    }

    pub fn apply_acp_configuration_update(
        &self,
        session_id: &str,
        update: &serde_json::Value,
    ) -> Result<()> {
        let (path, value) = match update["sessionUpdate"].as_str() {
            Some("config_option_update") if update["configOptions"].is_array() => {
                ("$.configOptions", &update["configOptions"])
            }
            Some("current_mode_update") if update["currentModeId"].is_string() => {
                ("$.modes.currentModeId", &update["currentModeId"])
            }
            _ => return Ok(()),
        };
        self.set_acp_configuration_value(session_id, path, value)
    }

    pub fn set_acp_configuration_value(
        &self,
        session_id: &str,
        path: &str,
        value: &serde_json::Value,
    ) -> Result<()> {
        let changed = self.conn.execute("UPDATE acp_sessions SET configuration_json = json_set(configuration_json, ?2, json(?3)) WHERE session_id = ?1", params![session_id, path, value.to_string()])?;
        if changed != 1 {
            return Err(anyhow!("ACP session no longer exists"));
        }
        Ok(())
    }

    pub fn set_acp_configuration(
        &self,
        session_id: &str,
        configuration: &serde_json::Value,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE acp_sessions SET configuration_json = ?2 WHERE session_id = ?1",
            params![session_id, serde_json::to_string(configuration)?],
        )?;
        if changed != 1 {
            return Err(anyhow!("ACP session no longer exists"));
        }
        Ok(())
    }

    pub(super) fn acp_launch_for_session(
        &self,
        session: &StoredChatSession,
    ) -> Result<Option<AcpSessionState>> {
        if !session.harness.starts_with("acp:") {
            return Ok(None);
        }
        if let Some(parent) = session
            .side_parent_session_id
            .as_deref()
            .or(session.parent_session_id.as_deref())
        {
            if let Some(state) = self.acp_session_state(parent)? {
                if state.launch.id == session.harness {
                    return Ok(Some(state));
                }
            }
        }
        let launch = crate::local::acp::definitions()
            .into_iter()
            .find(|definition| definition.id == session.harness)
            .ok_or_else(|| anyhow!("ACP harness is no longer configured"))?;
        launch.validate()?;
        Ok(Some(AcpSessionState {
            launch,
            configuration: serde_json::json!([]),
        }))
    }
}
