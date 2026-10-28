use super::Status;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) struct Context {
    env: HashMap<String, String>,
    pub home: PathBuf,
}

impl Context {
    pub fn current() -> Self {
        let mut command = tokio::process::Command::new("auth-probe");
        crate::local::chat::prepare_env(&mut command);
        let mut env: HashMap<_, _> = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
            .collect();
        for (key, value) in command.as_std().get_envs() {
            let key = key.to_string_lossy().into_owned();
            if let Some(value) = value {
                env.insert(key, value.to_string_lossy().into_owned());
            } else {
                env.remove(&key);
            }
        }
        let home = env
            .get("HOME")
            .or_else(|| env.get("USERPROFILE"))
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        Self { env, home }
    }
    pub fn env(&self, name: &str) -> Option<&str> {
        self.env
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
    }
    pub fn directory(&self, names: &[&str], fallback: &str) -> PathBuf {
        names
            .iter()
            .find_map(|name| self.env(name))
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(fallback))
    }
}

fn json(path: &Path) -> Result<Value, ()> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|_| ()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(_) => Err(()),
    }
}
fn toml(path: &Path) -> Result<Value, ()> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .parse::<toml::Value>()
            .map_err(|_| ())
            .and_then(|value| serde_json::to_value(value).map_err(|_| ())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(_) => Err(()),
    }
}
fn present(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.trim().is_empty())
}

pub(super) fn kimi(context: &Context) -> Status {
    kimi_checked(context).unwrap_or(Status::Unknown)
}
fn kimi_checked(context: &Context) -> Result<Status, ()> {
    let fallback = if context.home.join(".kimi-code/config.toml").is_file() {
        ".kimi-code"
    } else {
        ".kimi"
    };
    let directory = context.directory(&["KIMI_CODE_HOME", "KIMI_SHARE_DIR"], fallback);
    let config = toml(&directory.join("config.toml"))?;
    let Some(model) = config["default_model"]
        .as_str()
        .filter(|model| !model.is_empty())
    else {
        return Ok(Status::Missing);
    };
    let Some(provider_id) = config["models"][model]["provider"].as_str() else {
        return Err(());
    };
    let provider = &config["providers"][provider_id];
    if provider.is_null() {
        return Err(());
    }
    if let Some(oauth) = provider.get("oauth") {
        if !matches!(
            oauth["storage"].as_str().unwrap_or("file"),
            "file" | "keyring"
        ) {
            return Err(());
        }
        let key = oauth["key"].as_str().ok_or(())?;
        let name = key
            .trim_start_matches("oauth/")
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .ok_or(())?;
        if name == "." || name == ".." || name.contains('\\') {
            return Err(());
        }
        let token = json(&directory.join("credentials").join(format!("{name}.json")))?;
        if token.is_null() && oauth["storage"] == "keyring" {
            return Ok(Status::Keyring);
        }
        return Ok(oauth_status(&token));
    }
    if let Some(key) = provider["api_key_env"].as_str() {
        return Ok(if context.env(key).is_some() {
            Status::Configured
        } else {
            Status::Missing
        });
    }
    if present(&provider["api_key"]) {
        return Ok(Status::Configured);
    }
    let env_key = match provider["type"].as_str() {
        Some("kimi") => "KIMI_API_KEY",
        Some("anthropic") => "ANTHROPIC_API_KEY",
        Some("openai" | "openai_responses" | "openai_legacy") => "OPENAI_API_KEY",
        Some("google-genai") => "GOOGLE_API_KEY",
        Some("vertexai") => return Ok(Status::Unknown),
        _ => return Err(()),
    };
    Ok(if present(&provider["env"][env_key]) {
        Status::Configured
    } else {
        Status::Missing
    })
}

pub(super) fn kimi_keyring_key(context: &Context) -> Option<String> {
    let fallback = if context.home.join(".kimi-code/config.toml").is_file() {
        ".kimi-code"
    } else {
        ".kimi"
    };
    let directory = context.directory(&["KIMI_CODE_HOME", "KIMI_SHARE_DIR"], fallback);
    let config = toml(&directory.join("config.toml")).ok()?;
    let model = config["default_model"].as_str()?;
    let provider = config["models"][model]["provider"].as_str()?;
    config["providers"][provider]["oauth"]["key"]
        .as_str()
        .map(str::to_owned)
}

pub(super) fn oauth_status(token: &Value) -> Status {
    if present(&token["refresh_token"]) {
        return Status::Configured;
    }
    if !present(&token["access_token"]) {
        return Status::Missing;
    }
    match token["expires_at"].as_f64() {
        Some(expiry) if expiry > crate::store::now_ms() as f64 / 1000.0 => Status::Configured,
        Some(_) => Status::Expired,
        None => Status::Unknown,
    }
}

pub(super) fn minimax_login(context: &Context) -> Status {
    let fallback = if context.home.join(".minimax").exists() {
        ".minimax"
    } else {
        ".mavis"
    };
    let directory = context.directory(&["MINIMAX_DATA_DIR", "MAVIS_DATA_DIR"], fallback);
    let Ok(auth) = json(&directory.join("local-runtime.auth.json")) else {
        return Status::Unknown;
    };
    if !auth.is_null() && auth["version"] != 1 {
        return Status::Unknown;
    }
    if auth["version"] == 1 && present(&auth["auth"]["accessToken"]) {
        let Ok(projection) = json(&directory.join("cli-auth/shared-projection.json")) else {
            return Status::Unknown;
        };
        let Ok(preferences) = json(&directory.join("preferences/mcode-region.json")) else {
            return Status::Unknown;
        };
        if let (Some(build), Some(region)) = (
            projection["buildEnv"].as_str(),
            projection["region"].as_str(),
        ) {
            if !matches!(build, "prod" | "dev" | "test" | "staging")
                || !matches!(region, "en" | "cn")
            {
                return Status::Unknown;
            }
            if preferences["regions"][build]
                .as_str()
                .is_some_and(|preferred| preferred != region)
            {
                return Status::Missing;
            }
            let Ok(identity) = json(
                &directory
                    .join("cli-auth")
                    .join(build)
                    .join(region)
                    .join("account-identity.json"),
            ) else {
                return Status::Unknown;
            };
            use sha2::{Digest, Sha256};
            let token = auth["auth"]["accessToken"].as_str().unwrap_or_default();
            let fingerprint = format!("sha256:{:x}", Sha256::digest(token.trim().as_bytes()));
            if identity["status"] == "rejected" && identity["tokenFingerprint"] == fingerprint {
                return Status::Blocked;
            }
            return jwt_status(token).unwrap_or(Status::Configured);
        }
        return Status::Unknown;
    }
    // Older managed-login layouts are opaque; their presence never proves readiness.
    if ["auth", "cli-auth"]
        .iter()
        .any(|name| has_login_file(&directory.join(name)))
    {
        Status::Unknown
    } else {
        Status::Missing
    }
}

fn has_login_file(directory: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if kind.is_dir() {
            has_login_file(&path)
        } else {
            kind.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension == "json")
        }
    })
}

pub(super) fn zcode(context: &Context) -> Status {
    zcode_checked(context).unwrap_or(Status::Unknown)
}
fn zcode_checked(context: &Context) -> Result<Status, ()> {
    let directory = context.directory(&["ZCODE_HOME"], ".zcode").join("v2");
    let config = json(&directory.join("config.json"))?;
    let personal = json(&directory.join("provider_config.json"))?;
    let cache = json(&directory.join("coding-plan-cache.json"))?;
    let mut available = false;
    let mut unavailable = false;
    if let Some(items) = cache
        .pointer("/entryStatus/items")
        .and_then(Value::as_object)
    {
        for item in items.values() {
            available |= item["status"] == "available";
            unavailable |= item["status"] == "unavailable";
        }
    }
    if let Some(providers) = config["provider"].as_object() {
        for provider in providers.values().filter(|p| p["enabled"] != false) {
            if provider["models"]
                .as_object()
                .is_some_and(|models| !models.is_empty())
                && (present(&provider["options"]["apiKey"])
                    || provider["options"]["apiKeyRequired"] == false)
            {
                available = true;
            }
        }
    }
    if let Some(rules) = personal
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(Value::as_array)
    {
        for rule in rules.iter().filter(|rule| rule["enabled"] != false) {
            if present(&rule["config"]["access"]["apiKey"]) {
                available = true;
            }
        }
    }
    if available {
        return Ok(Status::Configured);
    }
    if unavailable {
        return Ok(Status::Blocked);
    }
    let credentials = json(&directory.join("credentials.json"))?;
    if !credentials.is_null() {
        return Ok(Status::Unknown);
    }
    Ok(Status::Missing)
}

pub(super) fn grok(context: &Context) -> Status {
    grok_checked(context).unwrap_or(Status::Unknown)
}
fn grok_checked(context: &Context) -> Result<Status, ()> {
    let directory = context.directory(&["GROK_HOME"], ".grok");
    let config = toml(&directory.join("config.toml"))?;
    let auth_path = context
        .env("GROK_AUTH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| directory.join("auth.json"));
    let auth = json(&auth_path)?;
    if !auth.is_null() && !auth.is_object() {
        return Err(());
    }
    let preferred = config
        .pointer("/grok_com_config/preferred_method")
        .and_then(Value::as_str);
    let disabled = context
        .env("GROK_DISABLE_API_KEY_AUTH")
        .is_some_and(|value| matches!(value, "1" | "true"))
        || config.pointer("/grok_com_config/disable_api_key_auth") == Some(&Value::Bool(true));
    if !disabled && preferred != Some("oidc") {
        if let Some(model) = config.pointer("/models/default").and_then(Value::as_str) {
            let model = &config["model"][model];
            if present(&model["api_key"]) {
                return Ok(Status::Configured);
            }
            if let Some(key) = model["env_key"].as_str() {
                return Ok(if context.env(key).is_some() {
                    Status::Configured
                } else {
                    Status::Missing
                });
            }
            if model.get("base_url").is_some() {
                return Ok(Status::Unknown);
            }
        }
        if context.env("XAI_API_KEY").is_some()
            || (auth["xai::api_key"]["auth_mode"] == "api_key"
                && present(&auth["xai::api_key"]["key"]))
        {
            return Ok(Status::Configured);
        }
    }
    if preferred == Some("api_key") {
        return Ok(Status::Missing);
    }
    let oidc = &config["grok_com_config"]["oidc"];
    let oauth2 = &config["grok_com_config"]["oauth2"];
    let issuer = oidc["issuer"]
        .as_str()
        .or_else(|| context.env("GROK_OIDC_ISSUER"))
        .or_else(|| oauth2["issuer"].as_str())
        .or_else(|| context.env("GROK_OAUTH2_ISSUER"))
        .unwrap_or("https://auth.x.ai");
    let client = oidc["client_id"]
        .as_str()
        .or_else(|| context.env("GROK_OIDC_CLIENT_ID"))
        .or_else(|| oauth2["client_id"].as_str())
        .or_else(|| context.env("GROK_OAUTH2_CLIENT_ID"))
        .unwrap_or("b1a00492-073a-47ea-816f-4c329264a828");
    let expected_scope = format!("{}::{client}", issuer.trim_end_matches('/'));
    if let Some(tokens) = auth.as_object() {
        for (scope, token) in tokens {
            if scope != &expected_scope
                || !present(&token["key"])
                || !matches!(token["auth_mode"].as_str(), Some("oidc" | "external"))
            {
                continue;
            }
            if present(&token["refresh_token"]) {
                return Ok(Status::Configured);
            }
            return Ok(
                jwt_status(token["key"].as_str().unwrap_or_default()).unwrap_or(Status::Unknown)
            );
        }
    }
    if config
        .pointer("/grok_com_config/auth_provider_command")
        .is_some()
        || context.env("GROK_AUTH_PROVIDER_COMMAND").is_some()
    {
        return Ok(Status::Unknown);
    }
    Ok(Status::Missing)
}

fn jwt_status(token: &str) -> Option<Status> {
    use base64::Engine;
    let payload = token
        .split('.')
        .nth(1)
        .and_then(|part| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(part)
                .ok()
        })
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())?;
    let expiry = payload["exp"].as_f64()?;
    Some(if expiry > crate::store::now_ms() as f64 / 1000.0 {
        Status::Configured
    } else {
        Status::Expired
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(Context);
    impl Fixture {
        fn new() -> Self {
            let home = std::env::temp_dir().join(format!("or-378-auth-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&home).unwrap();
            Self(Context {
                env: HashMap::new(),
                home,
            })
        }
        fn write(&self, path: &str, content: &str) {
            let path = self.0.home.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.home);
        }
    }

    #[test]
    fn kimi_checks_selected_provider_and_explicit_env_reference() {
        let mut fixture = Fixture::new();
        fixture.write(".kimi/config.toml", "default_model = 'chosen'\n[models.chosen]\nprovider = 'selected'\n[providers.selected]\ntype = 'kimi'\napi_key_env = 'SELECTED_KEY'\n[providers.other]\napi_key = 'unrelated-key'\n");
        fixture
            .0
            .env
            .insert("KIMI_API_KEY".into(), "unrelated-key".into());
        assert_eq!(kimi(&fixture.0), Status::Missing);
        fixture
            .0
            .env
            .insert("SELECTED_KEY".into(), "test-key".into());
        assert_eq!(kimi(&fixture.0), Status::Configured);
        fixture.write(".kimi/config.toml", "malformed = [");
        assert_eq!(kimi(&fixture.0), Status::Unknown);
    }
    #[test]
    fn kimi_refreshable_and_expired_logins_and_migrated_keyring() {
        let fixture = Fixture::new();
        fixture.write(".kimi/config.toml", "default_model = 'chosen'\n[models.chosen]\nprovider = 'selected'\n[providers.selected.oauth]\nstorage = 'keyring'\nkey = 'oauth/selected'\n");
        assert_eq!(kimi(&fixture.0), Status::Keyring);
        fixture.write(
            ".kimi/credentials/selected.json",
            r#"{"access_token":"test","expires_at":1}"#,
        );
        assert_eq!(kimi(&fixture.0), Status::Expired);
        fixture.write(
            ".kimi/credentials/selected.json",
            r#"{"access_token":"test","expires_at":1,"refresh_token":"test"}"#,
        );
        assert_eq!(kimi(&fixture.0), Status::Configured);
    }
    #[test]
    fn zcode_does_not_treat_encrypted_store_or_disabled_provider_as_ready() {
        let fixture = Fixture::new();
        assert_eq!(zcode(&fixture.0), Status::Missing);
        fixture.write(".zcode/v2/credentials.json", r#"{"unrelated":"encrypted"}"#);
        fixture.write(".zcode/v2/config.json", r#"{"provider":{"test":{"enabled":false,"models":{"test":{}},"options":{"apiKey":"test"}}}}"#);
        assert_eq!(zcode(&fixture.0), Status::Unknown);
        fixture.write(
            ".zcode/v2/coding-plan-cache.json",
            r#"{"entryStatus":{"items":{"builtin:test":{"status":"unavailable"}}}}"#,
        );
        assert_eq!(zcode(&fixture.0), Status::Blocked);
    }
    #[test]
    fn grok_honors_policy_and_ignores_unrelated_login_scopes() {
        let mut fixture = Fixture::new();
        fixture
            .0
            .env
            .insert("XAI_API_KEY".into(), "test-key".into());
        assert_eq!(grok(&fixture.0), Status::Configured);
        fixture.write(
            ".grok/config.toml",
            "[models]\ndefault = 'custom'\n[model.custom]\nenv_key = 'SELECTED_KEY'\n",
        );
        assert_eq!(grok(&fixture.0), Status::Missing);
        fixture
            .0
            .env
            .insert("SELECTED_KEY".into(), "test-key".into());
        assert_eq!(grok(&fixture.0), Status::Configured);
        fixture.write(
            ".grok/config.toml",
            "[grok_com_config]\npreferred_method = 'oidc'\n",
        );
        fixture.write(
            ".grok/auth.json",
            r#"{"other-issuer":{"auth_mode":"oidc","key":"test","refresh_token":"test"}}"#,
        );
        assert_eq!(grok(&fixture.0), Status::Missing);
        fixture.write(".grok/auth.json", r#"{"https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828":{"auth_mode":"oidc","key":"test","refresh_token":"test"}}"#);
        assert_eq!(grok(&fixture.0), Status::Configured);
    }
    #[test]
    fn jwt_expiry_is_checked_without_a_provider_request() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"exp":1}"#);
        assert_eq!(
            jwt_status(&format!("header.{payload}.signature")),
            Some(Status::Expired)
        );
        assert_eq!(jwt_status("opaque-token"), None);
    }
    #[test]
    fn minimax_lock_files_do_not_prove_a_saved_login() {
        let fixture = Fixture::new();
        fixture.write(".minimax/auth/prod/cn/mcode-public/auth.lock", "");
        assert_eq!(minimax_login(&fixture.0), Status::Missing);
        fixture.write(".minimax/auth/prod/cn/mcode-public/auth.json", "{}");
        assert_eq!(minimax_login(&fixture.0), Status::Unknown);
    }
    #[test]
    fn minimax_scopes_managed_login_to_the_selected_region() {
        let mut fixture = Fixture::new();
        fixture.write(
            ".minimax/local-runtime.auth.json",
            r#"{"version":1,"auth":{"accessToken":"test"}}"#,
        );
        fixture.write(
            ".minimax/cli-auth/shared-projection.json",
            r#"{"version":1,"buildEnv":"prod","region":"cn"}"#,
        );
        fixture.write(
            ".minimax/preferences/mcode-region.json",
            r#"{"version":1,"regions":{"prod":"en"}}"#,
        );
        assert_eq!(minimax_login(&fixture.0), Status::Missing);
        fixture.write(
            ".minimax/preferences/mcode-region.json",
            r#"{"version":1,"regions":{"prod":"cn"}}"#,
        );
        assert_eq!(minimax_login(&fixture.0), Status::Configured);
        let isolated = fixture
            .0
            .home
            .join("isolated-minimax")
            .to_string_lossy()
            .into_owned();
        fixture.0.env.insert("MINIMAX_DATA_DIR".into(), isolated);
        assert_eq!(minimax_login(&fixture.0), Status::Missing);
    }
}
