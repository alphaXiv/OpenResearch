use crate::error::{anyhow, Result};

pub const API_KEY_ENV: &str = "TINKER_API_KEY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyStatus {
    Missing,
    Valid,
    Invalid,
    BillingRequired,
}

fn key_status(status: reqwest::StatusCode) -> Result<KeyStatus> {
    match status.as_u16() {
        200 => Ok(KeyStatus::Valid),
        401 => Ok(KeyStatus::Invalid),
        402 => Ok(KeyStatus::BillingRequired),
        403 => Err(anyhow!(
            "Tinker denied access for this key. Check your account permissions."
        )),
        _ => Err(anyhow!(
            "Could not check the Tinker key (HTTP {}). Try again.",
            status.as_u16()
        )),
    }
}

pub async fn validate_api_key(key: &str) -> Result<KeyStatus> {
    let mut header = reqwest::header::HeaderValue::from_str(key)
        .map_err(|_| anyhow!("Invalid Tinker API key."))?;
    header.set_sensitive(true);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get("https://tinker.thinkingmachines.dev/services/tinker-prod/api/v1/get_server_capabilities")
        .header("X-API-Key", header)
        .send()
        .await
        .map_err(|_| anyhow!("Could not reach Tinker to check the key. Try again."))?;
    let status = key_status(response.status())?;
    if status == KeyStatus::Valid {
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|_| anyhow!("Tinker returned an unexpected response. Try again."))?;
        if !body
            .get("supported_models")
            .is_some_and(serde_json::Value::is_array)
        {
            return Err(anyhow!(
                "Tinker returned an unexpected response. Try again."
            ));
        }
    }
    Ok(status)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySource {
    Env,
    OpenresearchEnv,
}

pub fn resolve_api_key() -> Result<String> {
    resolve_api_key_with_source().map(|(key, _)| key)
}

pub fn resolve_api_key_with_source() -> Result<(String, ApiKeySource)> {
    resolve(
        std::env::var(API_KEY_ENV).ok(),
        crate::config::synced_env_var(API_KEY_ENV),
    )
}

fn resolve(
    process_value: Option<String>,
    synced_value: Option<String>,
) -> Result<(String, ApiKeySource)> {
    if let Some(key) = process_value.filter(|value| !value.trim().is_empty()) {
        return Ok((key.trim().to_string(), ApiKeySource::Env));
    }
    if let Some(key) = synced_value.filter(|value| !value.trim().is_empty()) {
        return Ok((key.trim().to_string(), ApiKeySource::OpenresearchEnv));
    }
    Err(anyhow!(
        "Tinker requires a non-empty TINKER_API_KEY in the process environment or Settings → Environment."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_billing_and_check_failures_are_distinct() {
        use reqwest::StatusCode;
        assert_eq!(key_status(StatusCode::OK).unwrap(), KeyStatus::Valid);
        assert_eq!(
            key_status(StatusCode::UNAUTHORIZED).unwrap(),
            KeyStatus::Invalid
        );
        assert_eq!(
            key_status(StatusCode::PAYMENT_REQUIRED).unwrap(),
            KeyStatus::BillingRequired
        );
        for status in [
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(key_status(status).is_err());
        }
    }

    #[test]
    fn process_environment_wins_and_blank_values_are_ignored() {
        assert_eq!(
            resolve(Some(" process ".into()), Some("synced".into())).unwrap(),
            ("process".into(), ApiKeySource::Env)
        );
        assert_eq!(
            resolve(Some("  ".into()), Some("synced".into())).unwrap(),
            ("synced".into(), ApiKeySource::OpenresearchEnv)
        );
        assert!(resolve(Some(String::new()), Some(" ".into())).is_err());
    }
}
