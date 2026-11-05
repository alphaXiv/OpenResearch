use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::ApiError;
use crate::error::anyhow;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicNetwork {
    ip: IpAddr,
    is_datacenter: Option<bool>,
}

#[derive(Deserialize)]
struct Lookup {
    success: bool,
    ip: IpAddr,
    is_hosting: Option<bool>,
}

pub(super) async fn public_network() -> Result<Json<PublicNetwork>, ApiError> {
    static CACHE: LazyLock<Mutex<Option<(Instant, PublicNetwork)>>> =
        LazyLock::new(|| Mutex::new(None));
    let mut cache = CACHE.lock().await;
    if let Some((at, network)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(300) {
            return Ok(Json(network.clone()));
        }
    }

    let lookup = crate::net::remote_client()
        .timeout(Duration::from_secs(5))
        .user_agent(concat!("OpenResearch/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| ApiError::from(anyhow!(error)))?
        .get("https://ipaddress.to/api/lookup/my")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| ApiError::from(anyhow!(error)))?
        .json::<Lookup>()
        .await
        .map_err(|error| ApiError::from(anyhow!(error)))?;
    if !lookup.success {
        return Err(ApiError::from(anyhow!(
            "Could not check this machine's public IP address"
        )));
    }
    let network = PublicNetwork {
        ip: lookup.ip,
        is_datacenter: lookup.is_hosting,
    };
    *cache = Some((Instant::now(), network.clone()));
    Ok(Json(network))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_classification_stays_unknown_and_ip_must_be_valid() {
        let lookup: Lookup =
            serde_json::from_str(r#"{"success":true,"ip":"2001:db8::1"}"#).unwrap();
        assert!(lookup.is_hosting.is_none());
        assert!(
            serde_json::from_str::<Lookup>(r#"{"success":true,"ip":"not an address"}"#).is_err()
        );
        let lookup: Lookup =
            serde_json::from_str(r#"{"success":true,"ip":"192.0.2.1","is_hosting":true}"#).unwrap();
        assert_eq!(lookup.is_hosting, Some(true));
    }
}
