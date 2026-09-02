use crate::error::{anyhow, Result};

pub const API_KEY_ENV: &str = "TINKER_API_KEY";

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
        "Tinker requires a non-empty TINKER_API_KEY in the process environment or ~/.openresearch/env."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
