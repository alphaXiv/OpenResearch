//! Model providers OpenResearch configures for OpenCode: OpenAI-compatible
//! endpoints, local or remote, and the hosted providers it keeps a key for.
//! Both are merged into OpenCode's inline config at launch.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use crate::error::{anyhow, Result};
use crate::local::agent_profiles::{validate_identity, Edit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Value>,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    pub models: BTreeMap<String, usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connect {
    pub name: String,
    pub icon: Option<Value>,
    #[serde(flatten)]
    pub probe: Probe,
    pub model: String,
    pub context_window: usize,
}

fn path() -> PathBuf {
    crate::store::data_dir().join("local-models.json")
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Provider {
    #[serde(flatten)]
    pub credential: ProviderCredential,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<Value>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ProviderCredential {
    Key {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        canonical: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        models: Option<Vec<String>>,
    },
    Oauth,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddProvider {
    pub provider: String,
    pub api_key: String,
    pub name: String,
    pub icon: Option<Value>,
}

pub(crate) fn providers_path() -> PathBuf {
    crate::config::config_dir().join("opencode-providers.json")
}

pub fn providers() -> Result<BTreeMap<String, Provider>> {
    read_providers(&providers_path())
}

fn read_providers(path: &Path) -> Result<BTreeMap<String, Provider>> {
    let records: BTreeMap<String, Provider> = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| anyhow!("{} is not valid provider settings ({e}). Fix or remove this file, then re-check OpenCode.", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(e) => return Err(anyhow!("Cannot read {}: {e}", path.display())),
    };
    for (id, record) in &records {
        validate_identity(&record.name, record.icon.as_ref())?;
        let canonical = match &record.credential {
            ProviderCredential::Key { key, canonical, .. } => {
                validate_key(key)?;
                canonical.as_deref()
            }
            ProviderCredential::Oauth => None,
        };
        let valid_id = match canonical {
            Some(base) => {
                is_provider_id(base)
                    && id
                        .strip_prefix(base)
                        .and_then(|suffix| suffix.strip_prefix('-'))
                        .is_some_and(|suffix| {
                            suffix.len() == 6
                                && suffix
                                    .bytes()
                                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                        })
            }
            None => is_provider_id(id),
        };
        if !valid_id {
            return Err(anyhow!("Provider settings contain an incorrect id. Remove {id} and add the provider again."));
        }
    }
    Ok(records)
}

pub fn provider_list() -> Result<Value> {
    Ok(Value::Array(
        providers()?
            .into_iter()
            .map(|(id, provider)| {
                let mut item = json!({"id": id, "name": provider.name});
                if let Some(icon) = provider.icon {
                    item["icon"] = icon;
                }
                match provider.credential {
                    ProviderCredential::Key { key, canonical, .. } => {
                        item["kind"] = json!("key");
                        let ending: String = key
                            .chars()
                            .skip(key.chars().count().saturating_sub(4))
                            .collect();
                        item["keyEnding"] = json!(ending);
                        if let Some(canonical) = canonical {
                            item["canonical"] = json!(canonical);
                        }
                    }
                    ProviderCredential::Oauth => item["kind"] = json!("oauth"),
                }
                item
            })
            .collect(),
    ))
}

pub fn configures_remote() -> bool {
    !providers().unwrap_or_default().is_empty()
        || read()
            .unwrap_or_default()
            .values()
            .any(|connection| !is_loopback_url(&connection.base_url))
}

pub fn is_provider_id(id: &str) -> bool {
    id.len() <= 64
        && id.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
}

pub fn validate_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_whitespace) {
        return Err(anyhow!("Paste the API key on its own, without spaces."));
    }
    Ok(())
}

fn alias_id(base: &str, records: &BTreeMap<String, Provider>) -> String {
    loop {
        let suffix: String = uuid::Uuid::new_v4()
            .simple()
            .to_string()
            .chars()
            .take(6)
            .collect();
        let id = format!("{base}-{suffix}");
        if !records.contains_key(&id) {
            return id;
        }
    }
}

/// A second key for a provider reads its models from OpenCode, so it needs OpenCode installed.
pub(crate) async fn add_provider(
    binary: Option<&crate::local::opencode::ResolvedBinary>,
    req: AddProvider,
) -> Result<String> {
    if !is_provider_id(&req.provider) {
        return Err(anyhow!("{} is not an OpenCode provider id.", req.provider));
    }
    validate_key(&req.api_key)?;
    validate_identity(&req.name, req.icon.as_ref())?;
    let mut models = None;
    loop {
        {
            let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
            let mut records = providers()?;
            let duplicate = records.contains_key(&req.provider);
            if !duplicate || models.is_some() {
                let id = if duplicate {
                    alias_id(&req.provider, &records)
                } else {
                    req.provider.clone()
                };
                records.insert(
                    id.clone(),
                    Provider {
                        credential: ProviderCredential::Key {
                            key: req.api_key,
                            canonical: duplicate.then_some(req.provider),
                            models: if duplicate { models } else { None },
                        },
                        name: req.name.trim().to_owned(),
                        icon: req.icon,
                    },
                );
                save_providers(&records)?;
                return Ok(id);
            }
        }
        let binary = binary
            .ok_or_else(|| anyhow!("Install OpenCode to add another key for this provider."))?;
        models = Some(
            crate::local::opencode_signin::provider_models(binary, &req.provider, &req.api_key)
                .await?,
        );
    }
}

pub fn replace_key(id: &str, key: &str) -> Result<()> {
    validate_key(key)?;
    let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut records = providers()?;
    let Some(Provider {
        credential: ProviderCredential::Key { key: current, .. },
        ..
    }) = records.get_mut(id)
    else {
        return Err(anyhow!(
            "API key provider not found. Add the provider again."
        ));
    };
    *current = key.to_owned();
    save_providers(&records)
}

/// Renames or re-icons a provider or an endpoint; true when OpenCode's model names carry the name.
pub fn edit(id: &str, edit: &Edit) -> Result<bool> {
    let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut records = providers()?;
    if let Some(record) = records.get_mut(id) {
        let renamed = edit.apply(&mut record.name, &mut record.icon)?;
        // Only a second key's models are labelled with its name.
        let alias = matches!(
            record.credential,
            ProviderCredential::Key {
                canonical: Some(_),
                ..
            }
        );
        save_providers(&records)?;
        return Ok(renamed && alias);
    }
    let mut connections = read()?;
    let connection = connections
        .get_mut(id)
        .ok_or_else(|| anyhow!("This agent was removed. Add it again."))?;
    let renamed = edit.apply(&mut connection.name, &mut connection.icon)?;
    save(&connections)?;
    Ok(renamed)
}

pub fn record_oauth(path: &Path, id: &str, name: String, icon: Option<Value>) -> Result<()> {
    validate_identity(&name, icon.as_ref())?;
    let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut records = read_providers(path)?;
    if records
        .get(id)
        .is_some_and(|record| matches!(record.credential, ProviderCredential::Key { .. }))
    {
        return Err(anyhow!(
            "This provider has an API key. Remove it before signing in."
        ));
    }
    records.insert(
        id.to_owned(),
        Provider {
            credential: ProviderCredential::Oauth,
            name,
            icon,
        },
    );
    write_providers(path, &records)
}

pub fn remove_provider(id: &str) -> Result<()> {
    let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut records = providers()?;
    records.remove(id);
    save_providers(&records)
}

fn save_providers(records: &BTreeMap<String, Provider>) -> Result<()> {
    write_providers(&providers_path(), records)
}

fn write_providers(path: &Path, records: &BTreeMap<String, Provider>) -> Result<()> {
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| anyhow!("Provider settings have no directory"))?,
    )?;
    crate::local::git::atomic_write_with_mode(
        path,
        &serde_json::to_vec_pretty(records)?,
        Some(0o600),
    )
}

pub(crate) async fn refresh_alias_models(
    binary: &crate::local::opencode::ResolvedBinary,
) -> Result<()> {
    for (id, record) in providers()? {
        let ProviderCredential::Key {
            ref key,
            canonical: Some(ref base),
            ..
        } = record.credential
        else {
            continue;
        };
        let refreshed = crate::local::opencode_signin::provider_models(binary, base, key).await?;
        let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut records = providers()?;
        if let Some(Provider {
            credential:
                ProviderCredential::Key {
                    key: current_key,
                    canonical,
                    models,
                },
            ..
        }) = records.get_mut(&id)
        {
            if current_key == key && canonical.as_deref() == Some(base.as_str()) {
                *models = Some(refreshed);
                save_providers(&records)?;
            }
        }
    }
    Ok(())
}

pub fn read() -> Result<BTreeMap<String, Connection>> {
    match std::fs::read(path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| anyhow!("{} is not valid local model settings ({e}). Fix or remove this file, then re-check OpenCode.", path().display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(anyhow!("Cannot read {}: {e}", path().display())),
    }
}

pub fn list() -> Result<Value> {
    Ok(json!(read()?
        .into_iter()
        .map(|(id, c)| json!({
            "id": id, "name": c.name, "baseUrl": c.base_url,
            "models": c.models, "hasApiKey": !c.api_key.is_empty(), "icon": c.icon
        }))
        .collect::<Vec<_>>()))
}

/// An https:// address with no credentials in it; plain http would send the key in the clear.
fn is_remote_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.host_str().is_some()
    })
}

pub fn is_loopback_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.host_str().is_some_and(|host| {
            host == "localhost"
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        })
}

/// Why a model server listed no models, where the caller acts on which: start the server, ask for its
/// key, or load a model in it.
#[derive(Debug)]
pub enum Unlisted {
    /// Nothing answered at the address.
    Unreachable { local: bool },
    /// The server takes an API key, and none was sent.
    NeedsKey,
    /// The server answered with no models.
    NoModels,
}

impl Unlisted {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unreachable { .. } => "unreachable",
            Self::NeedsKey => "needsKey",
            Self::NoModels => "noModels",
        }
    }
}

impl std::fmt::Display for Unlisted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unreachable { local: true } => "Cannot reach the local server. Start it in your model app, then check the address and try again.",
            Self::Unreachable { local: false } => "Cannot reach the server. Check the address and try again.",
            Self::NeedsKey => "The server requires an API key. Enter its key and try again.",
            Self::NoModels => "The server is running but has no models. Download and load a model in your model app, then check again.",
        })
    }
}

impl std::error::Error for Unlisted {}

pub async fn discover(req: &Probe) -> Result<Vec<String>> {
    let local = is_loopback_url(&req.base_url);
    if !local && !is_remote_url(&req.base_url) {
        return Err(anyhow!(
            "Use an https:// address, or an http:// one on this computer such as http://127.0.0.1:1234/v1."
        ));
    }
    let client = if local {
        crate::net::loopback_client()
    } else {
        crate::net::remote_client()
    }
    .redirect(reqwest::redirect::Policy::none())
    .timeout(Duration::from_secs(if local { 5 } else { 15 }))
    .build()?;
    let mut request = client.get(format!("{}/models", req.base_url.trim_end_matches('/')));
    if !req.api_key.is_empty() {
        request = request.bearer_auth(&req.api_key);
    }
    let response = request
        .send()
        .await
        .map_err(|_| Unlisted::Unreachable { local })?;
    if matches!(
        response.status(),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
    ) {
        return Err(if req.api_key.is_empty() {
            Unlisted::NeedsKey.into()
        } else {
            anyhow!("The server didn't accept this key. Check the key and try again.")
        });
    }
    let response = response.error_for_status().map_err(|e| {
        anyhow!(
            "The local server returned {}. Check that the address ends with /v1.",
            e.status().map(|s| s.as_u16()).unwrap_or(0)
        )
    })?;
    let data: Value = response
        .json()
        .await
        .map_err(|_| anyhow!("The server did not return an OpenAI-compatible model list."))?;
    let models = data
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("The server did not return an OpenAI-compatible model list."))?;
    let ids: Vec<_> = models
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    if ids.is_empty() {
        return Err(Unlisted::NoModels.into());
    }
    Ok(ids)
}

/// Adds a model to the connection at its address, made under the request's name unless one is
/// saved there already; true when the model is new to it.
pub async fn connect(req: Connect) -> Result<(String, bool)> {
    validate_identity(&req.name, req.icon.as_ref())?;
    if req.context_window < 4096 {
        return Err(anyhow!(
            "Enter a connection name and a context window of at least 4096 tokens."
        ));
    }
    let models = discover(&req.probe).await?;
    if !models.contains(&req.model) {
        return Err(anyhow!(
            "That model is no longer available. Check the connection again."
        ));
    }
    let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut connections = read()?;
    let base = req.probe.base_url.trim_end_matches('/').to_owned();
    let id = connections
        .iter()
        .find(|(_, c)| c.base_url == base)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| format!("orx-local-{}", uuid::Uuid::new_v4()));
    let connection = connections.entry(id.clone()).or_insert_with(|| Connection {
        name: req.name,
        icon: req.icon,
        base_url: base,
        api_key: req.probe.api_key.clone(),
        models: BTreeMap::new(),
    });
    connection.api_key = req.probe.api_key;
    let created = connection
        .models
        .insert(req.model.clone(), req.context_window)
        .is_none();
    save(&connections)?;
    Ok((format!("{id}/{}", req.model), created))
}

fn save(connections: &BTreeMap<String, Connection>) -> Result<()> {
    std::fs::create_dir_all(crate::store::data_dir())?;
    crate::local::git::atomic_write_with_mode(
        &path(),
        &serde_json::to_vec_pretty(connections)?,
        Some(0o600),
    )
}

/// Removes one of a connection's models, or with none named the whole connection;
/// a connection left without models goes too.
pub fn remove(id: &str, model: Option<&str>) -> Result<()> {
    let _lock = WRITE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut connections = read()?;
    if let (Some(model), Some(connection)) = (model, connections.get_mut(id)) {
        connection.models.remove(model);
        if !connection.models.is_empty() {
            return save(&connections);
        }
    }
    connections.remove(id);
    save(&connections)
}

pub fn prepare_env(
    cmd: &mut tokio::process::Command,
    model: Option<&str>,
    protocol: crate::local::opencode::Protocol,
) -> Result<()> {
    crate::local::chat::prepare_env(cmd);
    let connections = read()?;
    let records = providers()?;
    if connections.is_empty() && records.is_empty() {
        return Ok(());
    }
    let mut config = inline_config(cmd)?;
    merge_config(&mut config, &connections, model)?;
    merge_providers(&mut config, &records, protocol)?;
    cmd.env("OPENCODE_CONFIG_CONTENT", serde_json::to_string(&config)?);
    Ok(())
}

fn merge_providers(
    config: &mut Value,
    records: &BTreeMap<String, Provider>,
    protocol: crate::local::opencode::Protocol,
) -> Result<()> {
    let config = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode inline config must be a JSON object."))?;
    for (id, record) in records {
        let ProviderCredential::Key {
            key,
            canonical,
            models,
        } = &record.credential
        else {
            continue;
        };
        if let Some(canonical) = canonical {
            if protocol == crate::local::opencode::Protocol::V1 {
                continue;
            }
            let providers = config
                .entry("providers")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .ok_or_else(|| anyhow!("OpenCode providers config must be a JSON object."))?;
            let models: BTreeMap<_, _> = models
                .iter()
                .flatten()
                .map(|id| (id.clone(), json!({})))
                .collect();
            providers.insert(id.clone(), json!({"canonical": canonical, "name": record.name, "settings": {"apiKey": key}, "models": models}));
        } else {
            merge_provider_key(config, "provider", id, "options", key)?;
        }
    }
    Ok(())
}

fn merge_provider_key(
    config: &mut serde_json::Map<String, Value>,
    group: &str,
    id: &str,
    settings: &str,
    key: &str,
) -> Result<()> {
    let providers = config
        .entry(group)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode {group} config must be a JSON object."))?;
    let provider = providers
        .entry(id)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode {id} config must be a JSON object."))?;
    let settings = provider
        .entry(settings)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode {id} settings must be a JSON object."))?;
    settings.insert("apiKey".into(), json!(key));
    Ok(())
}

pub(crate) fn prepare_provider_probe(
    cmd: &mut tokio::process::Command,
    base: &str,
    key: &str,
) -> Result<()> {
    prepare_env(cmd, None, crate::local::opencode::Protocol::V2)?;
    let mut config = inline_config(cmd)?;
    let root = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode inline config must be a JSON object."))?;
    merge_provider_key(root, "provider", base, "options", key)?;
    merge_provider_key(root, "providers", base, "settings", key)?;
    cmd.env("OPENCODE_CONFIG_CONTENT", serde_json::to_string(&config)?);
    Ok(())
}

pub fn model_label(provider: &str, name: &str, records: &BTreeMap<String, Provider>) -> String {
    match records.get(provider) {
        Some(Provider {
            credential:
                ProviderCredential::Key {
                    canonical: Some(_), ..
                },
            name: record_name,
            ..
        }) => format!("{name} · {record_name}"),
        _ => name.to_owned(),
    }
}

fn inline_config(cmd: &tokio::process::Command) -> Result<Value> {
    let inline = cmd
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
        .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
    match inline {
        Some(value) => serde_json::from_str(&value)
            .map_err(|_| anyhow!("OPENCODE_CONFIG_CONTENT must contain a JSON object.")),
        None => Ok(json!({})),
    }
}

pub(crate) fn prepare_generation_env(
    cmd: &mut tokio::process::Command,
    model: Option<&str>,
    protocol: crate::local::opencode::Protocol,
) -> Result<()> {
    prepare_env(cmd, model, protocol)?;
    let Some(model) = model.filter(|model| !model.trim().is_empty()) else {
        return Ok(());
    };
    let mut config = inline_config(cmd)?;
    merge_generation_models(&mut config, model, protocol)?;
    cmd.env("OPENCODE_CONFIG_CONTENT", serde_json::to_string(&config)?);
    Ok(())
}

fn merge_generation_models(
    config: &mut Value,
    model: &str,
    protocol: crate::local::opencode::Protocol,
) -> Result<()> {
    let config = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode inline config must be a JSON object."))?;
    let agents_key = match protocol {
        crate::local::opencode::Protocol::V1 => {
            config.insert("small_model".into(), json!(model));
            "agent"
        }
        crate::local::opencode::Protocol::V2 => "agents",
    };
    let agents = config
        .entry(agents_key)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode {agents_key} config must be a JSON object."))?;
    for name in ["title", "compaction"] {
        agents
            .entry(name)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| anyhow!("OpenCode {name} agent config must be a JSON object."))?
            .insert("model".into(), json!(model));
    }
    Ok(())
}

pub(crate) fn merge_config(
    config: &mut Value,
    connections: &BTreeMap<String, Connection>,
    model: Option<&str>,
) -> Result<()> {
    let config = config
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode inline config must be a JSON object."))?;
    let providers = config
        .entry("provider")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| anyhow!("OpenCode provider config must be a JSON object."))?;
    for (id, c) in connections {
        let models: BTreeMap<_, _> = c
            .models
            .iter()
            .map(|(model, context)| {
                (
                    model.clone(),
                    json!({
                        "name": if is_loopback_url(&c.base_url) {
                            format!("{} · {} (local)", model, c.name)
                        } else {
                            format!("{} · {}", model, c.name)
                        },
                        "limit": {"context": context, "output": 4096}
                    }),
                )
            })
            .collect();
        providers.insert(
            id.clone(),
            json!({"npm":"@ai-sdk/openai-compatible", "name":c.name,
            "options":{"baseURL":c.base_url,"apiKey":c.api_key}, "models":models}),
        );
    }
    if model.is_some_and(|model| {
        connections
            .keys()
            .any(|id| model.starts_with(&format!("{id}/")))
    }) {
        config.insert("share".into(), json!("disabled"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_pins_auxiliary_models_and_preserves_unrelated_config() {
        use crate::local::opencode::Protocol;
        for (protocol, key) in [(Protocol::V1, "agent"), (Protocol::V2, "agents")] {
            for model in [
                "orx-local-test/qwen",
                "openrouter/anthropic/claude",
                "custom/model",
            ] {
                let mut config = json!({"model":"global/default","permission":{"bash":"ask"},
                    "small_model":"global/small",key:{"title":{"model":"global/title","hidden":true},
                    "compaction":{"model":"global/compact","description":"Keep this"},
                    "build":{"model":"global/build"}}});
                merge_generation_models(&mut config, model, protocol).unwrap();
                assert_eq!(config[key]["title"]["model"], model);
                assert_eq!(config[key]["compaction"]["model"], model);
                assert_eq!(config[key]["title"]["hidden"], true);
                assert_eq!(config[key]["compaction"]["description"], "Keep this");
                assert_eq!(config[key]["build"]["model"], "global/build");
                assert_eq!(config["model"], "global/default");
                assert_eq!(config["permission"]["bash"], "ask");
                assert_eq!(
                    config["small_model"],
                    if protocol == Protocol::V1 {
                        model
                    } else {
                        "global/small"
                    }
                );
            }
        }
    }

    #[test]
    fn malformed_agent_overrides_fail_without_replacing_them() {
        let mut config = json!({"agents":{"title":"invalid"}});
        assert!(merge_generation_models(
            &mut config,
            "custom/model",
            crate::local::opencode::Protocol::V2
        )
        .is_err());
        assert_eq!(config["agents"]["title"], "invalid");
    }

    #[tokio::test]
    async fn discovery_tells_a_stopped_server_from_an_empty_one() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/v1/models",
                    axum::routing::get(|| async { axum::Json(json!({"data":[]})) }),
                ),
            )
            .await
            .unwrap();
        });
        let unlisted = |base: String| async move {
            let error = discover(&Probe {
                base_url: base,
                api_key: String::new(),
            })
            .await
            .unwrap_err();
            error.downcast_ref::<Unlisted>().map(Unlisted::code)
        };
        assert_eq!(unlisted(base).await, Some("noModels"));
        server.abort();

        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        assert_eq!(
            unlisted(format!("http://{address}/v1")).await,
            Some("unreachable")
        );
    }

    #[tokio::test]
    async fn a_server_that_takes_a_key_asks_for_one_then_judges_it() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/v1/models",
                    axum::routing::get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
                ),
            )
            .await
            .unwrap();
        });
        let probe = |api_key: &str| Probe {
            base_url: base.clone(),
            api_key: api_key.into(),
        };
        let error = discover(&probe("")).await.unwrap_err();
        assert!(
            matches!(error.downcast_ref::<Unlisted>(), Some(Unlisted::NeedsKey)),
            "{error}"
        );
        let error = discover(&probe("wrong")).await.unwrap_err();
        assert!(error.downcast_ref::<Unlisted>().is_none());
        assert!(
            error.to_string().contains("didn't accept this key"),
            "{error}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn discovers_models_and_merges_without_replacing_cloud_settings() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                axum::Router::new().route(
                    "/v1/models",
                    axum::routing::get(|| async {
                        axum::Json(json!({"data":[{"id":"mlx-community/qwen"}]}))
                    }),
                ),
            )
            .await
            .unwrap();
        });
        assert_eq!(
            discover(&Probe {
                base_url: base.clone(),
                api_key: String::new()
            })
            .await
            .unwrap(),
            vec!["mlx-community/qwen"]
        );
        let request: Connect = serde_json::from_value(json!({"name":"LM Studio","baseUrl":base,"model":"mlx-community/qwen","contextWindow":32768})).unwrap();
        assert_eq!(request.probe.base_url, base);
        assert!(request.probe.api_key.is_empty());
        let connections = BTreeMap::from([(
            "orx-local-test".into(),
            Connection {
                name: "LM Studio".into(),
                icon: None,
                base_url: base.clone(),
                api_key: String::new(),
                models: BTreeMap::from([("mlx-community/qwen".into(), 32768)]),
            },
        )]);
        let mut config = json!({"provider":{"anthropic":{"options":{"apiKey":"unchanged"}}},"model":"anthropic/claude","small_model":"anthropic/haiku","permission":{"bash":"ask"}});
        merge_config(
            &mut config,
            &connections,
            Some("orx-local-test/mlx-community/qwen"),
        )
        .unwrap();
        assert_eq!(
            config["provider"]["anthropic"]["options"]["apiKey"],
            "unchanged"
        );
        assert_eq!(config["model"], "anthropic/claude");
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(config["small_model"], "anthropic/haiku");
        assert_eq!(
            config["provider"]["orx-local-test"]["models"]["mlx-community/qwen"]["limit"]
                ["context"],
            32768
        );
        server.abort();
        let _ = server.await;
        assert!(discover(&Probe {
            base_url: base,
            api_key: String::new()
        })
        .await
        .is_err());
        // Plain http to another host is refused before any request goes out.
        assert!(discover(&Probe {
            base_url: "http://example.com/v1".into(),
            api_key: String::new()
        })
        .await
        .is_err());
        assert!(is_remote_url("https://api.example.com/v1"));
        assert!(!is_remote_url("https://user:key@api.example.com/v1"));
    }

    #[test]
    fn validates_keys_and_provider_ids() {
        for id in ["", "../keys", "Open Router", "a/b"] {
            assert!(!is_provider_id(id));
        }
        assert!(is_provider_id("moonshotai"));
        for key in ["", " key", "key\n", "two keys"] {
            assert!(validate_key(key).is_err());
        }
        assert!(validate_key(&"a".repeat(513)).is_err());
        assert!(validate_key("sk-test").is_ok());
    }

    #[test]
    fn provider_records_and_alias_config() {
        let records: BTreeMap<String, Provider> = serde_json::from_value(json!({
            "openai": {"kind":"key","key":"sk-base","name":"Personal"},
            "openai-abc123": {"kind":"key","key":"sk-work","canonical":"openai","models":["gpt-4.1"],"name":"Work","icon":{"brand":true,"color":"green"}},
            "xai": {"kind":"oauth","name":"SuperGrok"}
        })).unwrap();
        assert_eq!(
            serde_json::to_value(&records).unwrap()["xai"],
            json!({"kind":"oauth","name":"SuperGrok"})
        );
        let mut config =
            json!({"provider":{"openai":{"options":{"baseURL":"https://proxy.example/v1"}}}});
        merge_providers(&mut config, &records, crate::local::opencode::Protocol::V2).unwrap();
        assert_eq!(config["provider"]["openai"]["options"]["apiKey"], "sk-base");
        assert_eq!(
            config["provider"]["openai"]["options"]["baseURL"],
            "https://proxy.example/v1"
        );
        assert_eq!(
            config["providers"]["openai-abc123"],
            json!({"canonical":"openai","name":"Work","settings":{"apiKey":"sk-work"},"models":{"gpt-4.1":{}}})
        );
        assert!(config["provider"].get("xai").is_none());
        assert_eq!(
            model_label("openai-abc123", "GPT 4.1", &records),
            "GPT 4.1 · Work"
        );
        assert_eq!(model_label("openai", "GPT 4.1", &records), "GPT 4.1");
        for _ in 0..30 {
            let id = alias_id("openai", &records);
            let suffix = id.strip_prefix("openai-").unwrap();
            assert_eq!(suffix.len(), 6);
            assert!(suffix
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
            assert!(!records.contains_key(&id));
        }
    }

    #[test]
    fn provider_file_is_private_and_rejects_bad_records() {
        let root = crate::local::git::TemporaryDirectory::new("orx-provider-records").unwrap();
        let path = root.path().join("config/opencode-providers.json");
        assert!(read_providers(&path).unwrap().is_empty());
        let records: BTreeMap<String, Provider> =
            serde_json::from_value(json!({"openai":{"kind":"key","key":"sk-test","name":"Work"}}))
                .unwrap();
        write_providers(&path, &records).unwrap();
        assert_eq!(
            serde_json::to_value(read_providers(&path).unwrap()).unwrap(),
            serde_json::to_value(&records).unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for invalid in [
            json!({"../key":{"kind":"key","key":"sk-test","name":"Work"}}),
            json!({"openai":{"kind":"key","key":"has spaces","name":"Work"}}),
            json!({"openai":{"kind":"key","key":"sk-test","name":""}}),
            json!({"xai":{"kind":"oauth","name":"Work","icon":{"brand":true,"color":"wrong"}}}),
        ] {
            std::fs::write(&path, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(read_providers(&path).is_err());
        }
    }

    #[test]
    fn provider_key_overrides_preserve_settings_and_reject_malformed_config() {
        let mut config = json!({"providers":{"openai":{"settings":{"baseURL":"https://example.test/v1","apiKey":"old"}}}});
        merge_provider_key(
            config.as_object_mut().unwrap(),
            "providers",
            "openai",
            "settings",
            "new",
        )
        .unwrap();
        assert_eq!(
            config["providers"]["openai"]["settings"],
            json!({"baseURL":"https://example.test/v1","apiKey":"new"})
        );
        for mut config in [
            json!({"providers":[]}),
            json!({"providers":{"openai":[]}}),
            json!({"providers":{"openai":{"settings":[]}}}),
        ] {
            let before = config.clone();
            assert!(merge_provider_key(
                config.as_object_mut().unwrap(),
                "providers",
                "openai",
                "settings",
                "new"
            )
            .is_err());
            assert_eq!(config, before);
        }
    }

    #[test]
    fn v1_config_omits_aliases() {
        let records = serde_json::from_value(json!({"openai":{"kind":"key","key":"sk-base","name":"Personal"},"openai-abc123":{"kind":"key","key":"sk-work","canonical":"openai","models":["gpt-4.1"],"name":"Work"}})).unwrap();
        let mut config = json!({});
        merge_providers(&mut config, &records, crate::local::opencode::Protocol::V1).unwrap();
        assert!(config.get("providers").is_none());
        assert_eq!(config["provider"]["openai"]["options"]["apiKey"], "sk-base");
    }
}
