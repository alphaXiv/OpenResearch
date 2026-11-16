//! `orx agents`: the rows of Settings → Agents, read and labelled through the dashboard that
//! started this chat, which owns them.

use clap::Subcommand;
use serde_json::{json, Value};

use crate::error::{anyhow, bail, Result};
use crate::output::print_table;

#[derive(Subcommand, Debug)]
pub enum AgentsCommand {
    /// Name a row or pin a logo on it. KEY is a row's key from `orx agents`.
    Label {
        key: String,
        /// The name Settings and the model picker show.
        #[arg(long, conflicts_with = "clear_name")]
        name: Option<String>,
        /// A logo slug from the dashboard's set, such as `openai` or `thinkingmachines`.
        #[arg(long, conflicts_with = "clear_logo")]
        logo: Option<String>,
        /// An emoji for the row's icon.
        #[arg(long)]
        emoji: Option<String>,
        /// The icon's color: gray, red, orange, amber, green, teal, blue, indigo, purple or pink.
        #[arg(long, requires = "emoji")]
        color: Option<String>,
        /// Go back to the row's own name; only agents and providers found on this computer have one.
        #[arg(long)]
        clear_name: bool,
        /// Remove the pinned logo.
        #[arg(long)]
        clear_logo: bool,
    },
    /// Add an OpenAI-compatible endpoint, on this computer or over HTTPS, for OpenCode to run.
    AddEndpoint {
        #[arg(long)]
        name: String,
        /// The address ending in /v1, such as https://example.com/v1 or http://127.0.0.1:1234/v1.
        #[arg(long)]
        url: String,
        /// A model the endpoint lists at /models.
        #[arg(long)]
        model: String,
        /// The model's context window in tokens.
        #[arg(long, default_value_t = 32768)]
        context: usize,
        /// The endpoint's API key.
        #[arg(long, conflicts_with = "key_env")]
        key: Option<String>,
        /// The environment variable holding the endpoint's API key.
        #[arg(long)]
        key_env: Option<String>,
        /// A logo slug to pin on the new row.
        #[arg(long)]
        logo: Option<String>,
    },
}

pub async fn run(args: crate::AgentsArgs) -> Result<()> {
    let dashboard = Dashboard::from_env()?;
    match args.command {
        None => {
            let rows = rows(&dashboard).await?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print_rows(&rows);
            }
        }
        Some(AgentsCommand::Label {
            key,
            name,
            logo,
            emoji,
            color,
            clear_name,
            clear_logo,
        }) => {
            let mut edit = json!({});
            if let Some(name) = name {
                edit["name"] = json!(name);
            } else if clear_name {
                edit["name"] = Value::Null;
            }
            if let Some(logo) = logo {
                edit["logo"] = json!(logo);
            } else if clear_logo {
                edit["logo"] = Value::Null;
            }
            if let Some(emoji) = emoji {
                edit["icon"] = json!({"emoji": emoji});
                if let Some(color) = color {
                    edit["icon"]["color"] = json!(color);
                }
            }
            let row = label(&dashboard, &key, edit).await?;
            show(&row, args.json)?;
        }
        Some(AgentsCommand::AddEndpoint {
            name,
            url,
            model,
            context,
            key,
            key_env,
            logo,
        }) => {
            let key = match key_env {
                Some(var) => std::env::var(&var).map_err(|_| {
                    anyhow!("`{var}` is not set in this environment. Set it, or pass the key with --key.")
                })?,
                None => key.unwrap_or_default(),
            };
            let request = json!({
                "name": name, "baseUrl": url, "apiKey": key, "model": model, "contextWindow": context,
            });
            let row = add_endpoint(&dashboard, request, logo).await?;
            show(&row, args.json)?;
        }
    }
    Ok(())
}

pub(crate) struct Dashboard {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Dashboard {
    fn from_env() -> Result<Self> {
        let port = crate::local::chat::trusted_up_port()?.ok_or_else(|| {
            anyhow!("`orx agents` talks to the OpenResearch dashboard that started this chat; run it from an OpenResearch chat.")
        })?;
        let token = std::env::var(crate::local::chat::UP_AUTH_TOKEN_ENV)
            .ok()
            .filter(|token| !token.is_empty());
        Self::new(port, token)
    }

    pub(crate) fn new(port: u16, token: Option<String>) -> Result<Self> {
        Ok(Self {
            base: format!("http://127.0.0.1:{port}"),
            token,
            http: crate::net::loopback_client().build()?,
        })
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|error| {
            anyhow!("Could not reach the OpenResearch dashboard at {}: {error}. Check that OpenResearch is still running.", self.base)
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            bail!("Chats on a remote OpenResearch host can't use `orx agents`. Ask the user to make this change in OpenResearch.");
        }
        let text = response.text().await?;
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|body| body["error"].as_str().map(str::to_owned))
                .unwrap_or_else(|| format!("The dashboard answered {status}: {}", text.trim()));
            bail!("{message}");
        }
        Ok(serde_json::from_str(&text)?)
    }
}

/// Every row of Settings → Agents, keyed as `hiddenAgents`, labels and `PATCH /api/agents/:key` are.
pub(crate) async fn rows(dashboard: &Dashboard) -> Result<Vec<Value>> {
    let get = reqwest::Method::GET;
    let (harnesses, providers, endpoints, ui) = tokio::try_join!(
        dashboard.call(get.clone(), "/api/harnesses", None),
        dashboard.call(get.clone(), "/api/opencode/providers", None),
        dashboard.call(get.clone(), "/api/local-models", None),
        dashboard.call(get, "/api/settings/ui-state", None),
    )?;
    let harnesses = harnesses["harnesses"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let opencode = harnesses
        .iter()
        .find(|entry| entry["id"] == "opencode" && entry["profile"].is_null());
    let off_providers: Vec<String> = opencode
        .and_then(|entry| serde_json::from_value(entry["offProviders"].clone()).ok())
        .unwrap_or_default();
    let provider_on = |id: &Value| !off_providers.iter().any(|off| id == off.as_str());

    let mut rows = Vec::new();
    for entry in &harnesses {
        let id = entry["id"].as_str().unwrap_or_default();
        let on = entry["off"] != true;
        if let Some(profile) = entry.get("profile") {
            rows.push(row(
                &profile["id"],
                "account",
                &profile["name"],
                on,
                true,
                &profile["logo"],
            ));
        } else if id == "opencode" {
            let name = entry["label"]["name"]
                .as_str()
                .unwrap_or("OpenCode free models");
            let free = ui["opencodeFreeModels"] != false;
            rows.push(row(
                &json!(id),
                "free-models",
                &json!(name),
                free,
                false,
                &entry["label"]["logo"],
            ));
            for found in entry["foundProviders"].as_array().into_iter().flatten() {
                let name = found["label"]["name"]
                    .as_str()
                    .or(found["name"].as_str())
                    .or(found["id"].as_str());
                let mut found_row = row(
                    &json!(format!(
                        "opencode:{}",
                        found["id"].as_str().unwrap_or_default()
                    )),
                    "found-provider",
                    &json!(name),
                    provider_on(&found["id"]),
                    found["removable"] == true,
                    &found["label"]["logo"],
                );
                for field in ["baseUrl", "npm", "sources"] {
                    if !found[field].is_null() {
                        found_row[field] = found[field].clone();
                    }
                }
                rows.push(found_row);
            }
        } else if entry["installed"] == true {
            let name = entry["label"]["name"].as_str().or(entry["name"].as_str());
            let removable =
                entry["managed"] == true && !matches!(id, "claude-code" | "codex" | "opencode");
            rows.push(row(
                &json!(id),
                "agent",
                &json!(name),
                on,
                removable,
                &entry["label"]["logo"],
            ));
        }
    }
    for provider in providers.as_array().into_iter().flatten() {
        let key = json!(format!(
            "opencode:{}",
            provider["id"].as_str().unwrap_or_default()
        ));
        rows.push(row(
            &key,
            "provider",
            &provider["name"],
            provider_on(&provider["id"]),
            true,
            &provider["logo"],
        ));
    }
    for endpoint in endpoints.as_array().into_iter().flatten() {
        let key = json!(format!(
            "opencode:{}",
            endpoint["id"].as_str().unwrap_or_default()
        ));
        let mut endpoint_row = row(
            &key,
            "endpoint",
            &endpoint["name"],
            provider_on(&endpoint["id"]),
            true,
            &endpoint["logo"],
        );
        endpoint_row["baseUrl"] = endpoint["baseUrl"].clone();
        rows.push(endpoint_row);
    }
    Ok(rows)
}

fn row(key: &Value, kind: &str, name: &Value, on: bool, removable: bool, logo: &Value) -> Value {
    let mut row = json!({"key": key, "name": name, "kind": kind, "on": on, "removable": removable});
    if !logo.is_null() {
        row["logo"] = logo.clone();
    }
    row
}

async fn find(dashboard: &Dashboard, key: &str) -> Result<Value> {
    rows(dashboard)
        .await?
        .into_iter()
        .find(|row| row["key"] == key)
        .ok_or_else(|| anyhow!("No row `{key}`. Run `orx agents` for the keys."))
}

pub(crate) async fn label(dashboard: &Dashboard, key: &str, edit: Value) -> Result<Value> {
    if edit.as_object().is_some_and(|edit| edit.is_empty()) {
        bail!("Pass --name, --logo, --emoji, --clear-name or --clear-logo.");
    }
    let path = format!("/api/agents/{}", urlencoding::encode(key));
    dashboard
        .call(reqwest::Method::PATCH, &path, Some(edit))
        .await?;
    find(dashboard, key).await
}

pub(crate) async fn add_endpoint(
    dashboard: &Dashboard,
    request: Value,
    logo: Option<String>,
) -> Result<Value> {
    let added = dashboard
        .call(reqwest::Method::POST, "/api/local-models", Some(request))
        .await?;
    let id = added["model"]
        .as_str()
        .and_then(|model| model.split_once('/'))
        .map(|(id, _)| id)
        .ok_or_else(|| {
            anyhow!(
                "The dashboard did not say which endpoint it added. Run `orx agents` to find it."
            )
        })?;
    let key = format!("opencode:{id}");
    match logo {
        Some(logo) => label(dashboard, &key, json!({"logo": logo})).await,
        None => find(dashboard, &key).await,
    }
}

fn show(row: &Value, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(row)?);
    } else {
        print_rows(std::slice::from_ref(row));
    }
    Ok(())
}

fn print_rows(rows: &[Value]) {
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let detail = match row["baseUrl"].as_str() {
                Some(url) => url.to_string(),
                None => row["sources"]
                    .as_array()
                    .map(|sources| sources.iter().map(text).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default(),
            };
            vec![
                text(&row["key"]),
                text(&row["kind"]),
                if row["on"] == true { "on" } else { "off" }.to_string(),
                text(&row["name"]),
                text(&row["logo"]),
                detail,
            ]
        })
        .collect();
    print_table(&["KEY", "KIND", "ON", "NAME", "LOGO", "DETAIL"], &table);
}
