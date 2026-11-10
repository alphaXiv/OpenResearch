use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};

pub const HEARTBEAT: Duration = Duration::from_secs(30);

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rental {
    pub api_url: String,
    pub sandbox_id: String,
    pub token: String,
    /// Fields a newer orx wrote, kept so an older supervisor's rewrite doesn't drop them.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl std::fmt::Debug for Rental {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rental")
            .field("sandbox_id", &self.sandbox_id)
            .finish_non_exhaustive()
    }
}

impl Rental {
    pub async fn operation(&self, operation: &str) -> Result<()> {
        let response = crate::net::remote_client()
            .timeout(Duration::from_secs(10))
            .build()?
            .post(format!(
                "{}/sandboxes/{}/lease/{operation}",
                self.api_url.trim_end_matches('/'),
                self.sandbox_id
            ))
            .bearer_auth(&self.token)
            .json(&serde_json::json!({}))
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(anyhow!(
                "rental {operation} rejected ({})",
                response.status()
            ));
        }
        Ok(())
    }

    pub fn heartbeat(&self) -> Heartbeat {
        let rental = self.clone();
        Heartbeat(tokio::spawn(async move {
            loop {
                if let Err(error) = rental.operation("heartbeat").await {
                    eprintln!("rental heartbeat unavailable: {error}");
                }
                tokio::time::sleep(crate::config::paced(HEARTBEAT)).await;
            }
        }))
    }
}

pub struct Heartbeat(tokio::task::JoinHandle<()>);
impl Heartbeat {
    pub(crate) fn bounded(self, duration: Duration) -> Self {
        Self(tokio::spawn(async move {
            let _heartbeat = self;
            tokio::time::sleep(duration).await;
        }))
    }
}
impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfig {
    pub rental: Rental,
    pub directory: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rental_debug_does_not_expose_capability() {
        let rental = Rental {
            api_url: "https://example.invalid".into(),
            sandbox_id: "rental".into(),
            token: "secret".into(),
            unknown: Default::default(),
        };
        assert!(!format!("{rental:?}").contains("secret"));
        assert_eq!(HEARTBEAT.as_secs(), 30);
    }
}
