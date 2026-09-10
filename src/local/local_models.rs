//! Local inference connections owned by OpenResearch, merged into OpenCode at launch.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use crate::error::{anyhow, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ponytail: serialize settings writes across browser tabs; per-file locks if stores become concurrent.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub name: String,
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
    #[serde(flatten)]
    pub probe: Probe,
    pub model: String,
    pub context_window: usize,
}

fn path() -> PathBuf {
    crate::store::data_dir().join("local-models.json")
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
            "models": c.models, "hasApiKey": !c.api_key.is_empty()
        }))
        .collect::<Vec<_>>()))
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

pub async fn discover(req: &Probe) -> Result<Vec<String>> {
    if !is_loopback_url(&req.base_url) {
        return Err(anyhow!(
            "Use an http:// or https:// loopback address, such as http://127.0.0.1:1234/v1."
        ));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5))
        .build()?;
    let mut request = client.get(format!("{}/models", req.base_url.trim_end_matches('/')));
    if !req.api_key.is_empty() {
        request = request.bearer_auth(&req.api_key);
    }
    let response = request.send().await.map_err(|_| anyhow!("Cannot reach the local server. Start it in your model app, then check the address and try again."))?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err(anyhow!(
            "The local server requires an API key. Enter its key and try again."
        ));
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
        return Err(anyhow!("The server is running but has no models. Download and load a model in your model app, then check again."));
    }
    Ok(ids)
}

pub async fn connect(req: Connect) -> Result<String> {
    if req.name.trim().is_empty() || req.name.len() > 100 || req.context_window < 4096 {
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
    let _lock = WRITE_LOCK
        .lock()
        .map_err(|_| anyhow!("Local model settings are unavailable."))?;
    let mut connections = read()?;
    let base = req.probe.base_url.trim_end_matches('/').to_owned();
    let id = connections
        .iter()
        .find(|(_, c)| c.base_url == base)
        .map(|(id, _)| id.clone())
        .unwrap_or_else(|| format!("orx-local-{}", uuid::Uuid::new_v4()));
    let connection = connections.entry(id.clone()).or_insert_with(|| Connection {
        name: req.name.clone(),
        base_url: base,
        api_key: req.probe.api_key.clone(),
        models: BTreeMap::new(),
    });
    connection.name = req.name;
    connection.api_key = req.probe.api_key;
    connection
        .models
        .insert(req.model.clone(), req.context_window);
    save(&connections)?;
    Ok(format!("{id}/{}", req.model))
}

fn save(connections: &BTreeMap<String, Connection>) -> Result<()> {
    std::fs::create_dir_all(crate::store::data_dir())?;
    crate::local::git::atomic_write_with_mode(
        &path(),
        &serde_json::to_vec_pretty(connections)?,
        Some(0o600),
    )
}

pub fn remove(id: &str) -> Result<()> {
    let _lock = WRITE_LOCK
        .lock()
        .map_err(|_| anyhow!("Local model settings are unavailable."))?;
    let mut connections = read()?;
    connections.remove(id);
    save(&connections)
}

pub fn prepare_env(cmd: &mut tokio::process::Command, model: Option<&str>) -> Result<()> {
    crate::local::chat::prepare_env(cmd);
    let connections = read()?;
    if connections.is_empty() {
        return Ok(());
    }
    let inline = cmd
        .as_std()
        .get_envs()
        .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
        .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
    let mut config: Value = match inline {
        Some(value) => serde_json::from_str(&value).map_err(|_| {
            anyhow!("OPENCODE_CONFIG_CONTENT must contain a JSON object to connect local models.")
        })?,
        None => json!({}),
    };
    merge_config(&mut config, &connections, model)?;
    cmd.env("OPENCODE_CONFIG_CONTENT", serde_json::to_string(&config)?);
    Ok(())
}

fn merge_config(
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
                        "name": format!("{} · {} (local)", model, c.name),
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
    if let Some(model) = model.filter(|model| {
        connections
            .keys()
            .any(|id| model.starts_with(&format!("{id}/")))
    }) {
        config.insert("small_model".into(), json!(model));
        config.insert("share".into(), json!("disabled"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(config["small_model"], "orx-local-test/mlx-community/qwen");
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
        assert!(discover(&Probe {
            base_url: "https://example.com/v1".into(),
            api_key: String::new()
        })
        .await
        .is_err());
    }
}
