//! Create an ordinary project chat through the resident server's chat API.
//!
//!   orx agent spawn "<message>"   create a chat with the user's New Chat settings
//!   orx agent spawn "<message>" --harness <id> [--model <model>]
//!                                 create it on another agent, for cross-harness review
//!   orx agent harnesses           list the agents a chat can run on, with their vendors
//!   orx agent quota               quota left per harness account and model pool
//!   orx agent route "<task>"      where new work should run, from the decision model

use std::io::Read;

use crate::error::{anyhow, Result};
use crate::local::chat;
use crate::AgentCommand;

pub async fn run(args: crate::AgentArgs) -> Result<()> {
    match args.command {
        AgentCommand::Spawn {
            task,
            stdin,
            title,
            project,
            harness,
            model,
        } => {
            let prompt = task_text(task, stdin)?;
            let port = chat::trusted_up_port()?.ok_or_else(|| anyhow!(
                "Connect to the running OpenResearch server with ORX_LOCAL_SESSION=1 and ORX_UP_PORT (and ORX_UP_AUTH_TOKEN when required)."
            ))?;
            let project = match project {
                Some(project) => project,
                None => {
                    let session_id = chat::launching_chat_session().ok_or_else(|| {
                        anyhow!(
                        "Pass --project <project-id> when there is no current OpenResearch chat."
                    )
                    })?;
                    let session = crate::commands::up::chat_request_via_up(
                        port,
                        reqwest::Method::GET,
                        &format!("/api/chat/sessions/{session_id}"),
                        &serde_json::json!({}),
                    )
                    .await?;
                    session
                        .pointer("/session/projectId")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            anyhow!("The current chat is unavailable. Pass --project <project-id>.")
                        })?
                        .to_string()
                }
            };
            let agent = match harness {
                Some(harness) => Some(agent_choice(port, harness, model).await?),
                None => None,
            };
            spawn(port, &project, &prompt, title.as_deref(), agent).await
        }
        AgentCommand::Harnesses { json } => harnesses(json).await,
        AgentCommand::Quota {
            harness,
            refresh,
            json,
        } => quota(harness.as_deref(), refresh, json).await,
        AgentCommand::Route { task, stdin, json } => {
            let task = task_text(task, stdin).map_err(|_| {
                anyhow!("Describe the work: orx agent route \"<task>\" (or --stdin).")
            })?;
            route(&task, json).await
        }
    }
}

/// `orx agent route`: the harness, model, account, and compute new work
/// should use. The decision model answers when it is on; otherwise, or when
/// it fails, the route comes from quota and the user's settings alone.
async fn route(task: &str, json: bool) -> Result<()> {
    use crate::local::decision;
    use crate::local::harness::accounts;
    let now = crate::store::now_ms();
    let mut request = decision::RouteRequest::new(decision::Purpose::Plan, task);
    request.harnesses = ready_harnesses().await;
    request.accounts = accounts::statuses(None)?
        .iter()
        .filter(|account| account.signed_in && !account.paused)
        .filter(|account| {
            request
                .harnesses
                .iter()
                .any(|option| option.id == account.harness)
        })
        .flat_map(|account| {
            account.pools.iter().map(|pool| {
                decision::AccountOption::from_status(&account.harness, &account.id, pool, now)
            })
        })
        .collect();
    let compute = super::up::compute_settings::routing_targets().await;
    let default_compute = compute["defaultBackend"].as_str().unwrap_or("local");
    request.compute = compute["targets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|target| target["configured"] == true)
        .filter_map(|target| {
            let id = target["id"].as_str()?;
            Some(decision::ComputeOption {
                id: id.to_string(),
                summary: target["summary"].as_str().unwrap_or_default().to_string(),
                is_default: id == default_compute,
            })
        })
        .collect();
    request.default_gpu = compute["defaultFlavor"].as_str().map(str::to_string);

    let fallback = quota_route(&request);
    let settings = decision::settings();
    let (route, source, note) = match decision::decide(&request).await {
        Ok(Some(decided)) => (complete_route(decided, &fallback), "decision", None),
        Ok(None) if !settings.enabled => (fallback, "quota", Some("The decision model is off (Settings → Harnesses).".to_string())),
        Ok(None) if settings.key_source().is_none() => (fallback, "quota", Some("The decision model has no OpenAI API key (Settings → Harnesses).".to_string())),
        Ok(None) => (fallback, "quota", Some("The decision model is paused after OpenAI refused it; it retries in a few minutes.".to_string())),
        Err(error) => (fallback, "quota", Some(format!("The decision model is unavailable ({error}), so this route comes from quota alone."))),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "route": route,
                "source": source,
                "decisionModel": (source == "decision").then_some(settings.model.as_str()),
                "note": note,
            }))?
        );
        return Ok(());
    }
    let mut parts = Vec::new();
    if let Some(harness) = &route.harness {
        parts.push(format!("harness {harness}"));
    }
    if let Some(model) = &route.model {
        parts.push(format!("model {model}"));
    }
    if let Some(account) = &route.account {
        parts.push(format!("account {account}"));
    }
    match (&route.compute, &route.gpu) {
        (Some(compute), Some(gpu)) => parts.push(format!("compute {compute} ({gpu})")),
        (Some(compute), None) => parts.push(format!("compute {compute}")),
        _ => {}
    }
    let by = match source {
        "decision" => format!("decision model {}", settings.model),
        _ => "quota".to_string(),
    };
    if parts.is_empty() {
        println!("No route ({by}): no ready harness has room.");
    } else {
        println!("Route ({by}): {}", parts.join(" · "));
    }
    if !route.reason.is_empty() {
        println!("Why: {}", route.reason);
    }
    if let Some(note) = note {
        println!("{note}");
    }
    if let Some(harness) = &route.harness {
        let model = route
            .model
            .as_ref()
            .map(|model| format!(" --model {model}"))
            .unwrap_or_default();
        println!("Start it with: orx agent spawn --harness {harness}{model} \"<message>\"");
    }
    Ok(())
}

/// The harnesses a chat can run on right now, with their models: the
/// running server's catalog when there is one (it is warm), else a full
/// detection pass. A snapshot pass is no use here: it never reports a
/// harness ready, since readiness is only confirmed by the full pass.
async fn ready_harnesses() -> Vec<crate::local::decision::HarnessOption> {
    if let Some(port) = chat::trusted_up_port().ok().flatten() {
        if let Some(catalog) = harness_catalog(port).await {
            return harness_options_from_catalog(&catalog);
        }
    }
    crate::local::harness::detect_harnesses()
        .await
        .into_iter()
        .filter(|info| info.agent_ready)
        .map(|info| crate::local::decision::HarnessOption {
            id: info.id.to_string(),
            vendor: harness_vendor(info.id).to_string(),
            models: crate::local::decision::model_ids(&info.models),
            recommended_model: info.recommended_model.clone(),
        })
        .collect()
}

fn harness_options_from_catalog(
    catalog: &[serde_json::Value],
) -> Vec<crate::local::decision::HarnessOption> {
    catalog
        .iter()
        .filter(|entry| entry["agentReady"] == true)
        .filter_map(|entry| {
            let id = entry["id"].as_str()?;
            let models: Vec<String> = entry["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|model| model["id"].as_str().map(str::to_string))
                .take(16)
                .collect();
            Some(crate::local::decision::HarnessOption {
                id: id.to_string(),
                vendor: harness_vendor(id).to_string(),
                models,
                recommended_model: entry["recommendedModel"].as_str().map(str::to_string),
            })
        })
        .collect()
}

/// The route quota alone gives: the first ready harness in the user's
/// fallback order with room on its default model's pool, the account a turn
/// there would use, and the default compute target.
fn quota_route(request: &crate::local::decision::RouteRequest) -> crate::local::decision::Route {
    use crate::local::harness::accounts;
    let order = crate::store::Store::open()
        .ok()
        .and_then(|store| store.ui_state().ok())
        .map(|state| state.harness_failover.order)
        .unwrap_or_default();
    let mut harnesses: Vec<&crate::local::decision::HarnessOption> = order
        .iter()
        .filter_map(|id| request.harnesses.iter().find(|option| &option.id == id))
        .collect();
    for option in &request.harnesses {
        if !harnesses.iter().any(|picked| picked.id == option.id) {
            harnesses.push(option);
        }
    }
    let has_room = |harness: &str, model: Option<&str>| {
        !accounts::supports_accounts(harness)
            || request.accounts.iter().any(|account| {
                account.harness == harness
                    && account.pool == accounts::pool_for(harness, model)
                    && account.state != "blocked"
            })
    };
    let pick = harnesses
        .iter()
        .find(|option| has_room(&option.id, option.recommended_model.as_deref()));
    let compute = request
        .compute
        .iter()
        .find(|option| option.is_default)
        .map(|option| option.id.clone());
    crate::local::decision::Route {
        harness: pick.map(|option| option.id.clone()),
        model: pick.and_then(|option| option.recommended_model.clone()),
        account: pick.and_then(|option| {
            accounts::choose(&option.id, option.recommended_model.as_deref(), None)
        }),
        gpu: compute
            .as_deref()
            .filter(|id| *id != "local")
            .and(request.default_gpu.clone()),
        compute,
        reason: String::new(),
    }
}

/// Fill what the decision model left open (the harness, and the account a
/// turn there would use) from the quota route; compute stays as it said.
fn complete_route(
    mut decided: crate::local::decision::Route,
    fallback: &crate::local::decision::Route,
) -> crate::local::decision::Route {
    if decided.harness.is_none() {
        decided.harness.clone_from(&fallback.harness);
        decided.model.clone_from(&fallback.model);
        if decided.account.is_none() {
            decided.account.clone_from(&fallback.account);
        }
    }
    if decided.account.is_none() {
        if let Some(harness) = &decided.harness {
            decided.account =
                crate::local::harness::accounts::choose(harness, decided.model.as_deref(), None);
        }
    }
    decided
}

/// `orx agent quota`: what each account has left per quota pool, and the
/// account a new turn on that pool would use.
async fn quota(harness: Option<&str>, refresh: bool, json: bool) -> Result<()> {
    use crate::local::harness::accounts;
    if let Some(harness) = harness {
        if !accounts::supports_accounts(harness) {
            return Err(anyhow!(
                "`{harness}` has no accounts to report. Use one of {}.",
                accounts::ACCOUNT_HARNESSES.join(", ")
            ));
        }
    }
    let mut read_errors = Vec::new();
    if refresh {
        for account in accounts::statuses(harness)? {
            if account.signed_in && !account.paused {
                if let Err(error) = accounts::refresh(&account.harness, &account.id).await {
                    read_errors.push(format!("{}/{}: {error}", account.harness, account.id));
                }
            }
        }
    }
    let statuses = accounts::statuses(harness)?;
    let mut rows = Vec::new();
    for account in &statuses {
        for pool in &account.pools {
            rows.push(serde_json::json!({
                "harness": account.harness,
                "account": account.id,
                "email": account.email,
                "signedIn": account.signed_in,
                "paused": account.paused,
                "pool": pool.pool,
                "poolLabel": pool.label,
                "state": pool.state,
                "remaining": pool.remaining,
                "blockedUntil": pool.blocked_until,
                "windows": pool.windows,
                "trackedTurns24h": pool.tracked_turns,
                "trackedTokens24h": pool.tracked_tokens,
            }));
        }
    }
    let mut next = Vec::new();
    for harness in accounts::ACCOUNT_HARNESSES
        .iter()
        .filter(|id| harness.is_none_or(|only| only == **id))
    {
        for pool in accounts::pools(harness) {
            let model = match pool.id {
                "partner" => Some("claude"),
                _ => None,
            };
            let pick = accounts::choose(harness, model, None);
            let usable = statuses.iter().any(|account| {
                account.harness == *harness
                    && !account.paused
                    && account.signed_in
                    && account.pools.iter().any(|status| {
                        status.pool == pool.id && status.state != accounts::QuotaState::Blocked
                    })
            });
            next.push(serde_json::json!({
                "harness": harness,
                "pool": pool.id,
                "poolLabel": pool.label,
                "account": pick.clone().or_else(|| usable.then(|| accounts::DEFAULT.to_string())),
                "available": usable,
            }));
        }
    }
    if json {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "accounts": rows,
                "next": next,
                "readErrors": read_errors,
            }))?
        );
        return Ok(());
    }
    let now = crate::store::now_ms();
    let when = |ms: i64| {
        let minutes = ((ms - now).max(0) + 59_999) / 60_000;
        if minutes >= 120 {
            format!("in {}h", minutes / 60)
        } else {
            format!("in {minutes}m")
        }
    };
    let table: Vec<Vec<String>> = statuses
        .iter()
        .flat_map(|account| {
            account.pools.iter().map(move |pool| {
                let left = pool
                    .remaining
                    .map(|left| format!("{:.0}%", left * 100.0))
                    .unwrap_or_else(|| "unknown".into());
                let mut note = Vec::new();
                if let Some(until) = pool.blocked_until {
                    note.push(format!("limited, back {}", when(until)));
                }
                for window in &pool.windows {
                    let reset = window
                        .resets_at
                        .map(|at| format!(", resets {}", when(at)))
                        .unwrap_or_default();
                    note.push(format!(
                        "{} {:.0}% used{reset}",
                        window.window, window.used_percent
                    ));
                }
                if let (Some(limit), Some(used)) = (&pool.limit, pool.used) {
                    note.push(format!(
                        "{used:.0}/{:.0} {} per {}h",
                        limit.limit,
                        match limit.unit {
                            accounts::LimitUnit::Turns => "turns",
                            accounts::LimitUnit::Tokens => "tokens",
                        },
                        limit.window_hours
                    ));
                }
                if !account.signed_in {
                    note.push("not signed in".into());
                } else if account.paused {
                    note.push("paused".into());
                }
                vec![
                    account.harness.clone(),
                    account.id.clone(),
                    account.email.clone().unwrap_or_default(),
                    pool.label.to_string(),
                    left,
                    format!("{:?}", pool.state).to_lowercase(),
                    note.join("; "),
                ]
            })
        })
        .collect();
    crate::output::print_table(
        &[
            "HARNESS",
            "ACCOUNT",
            "SIGNED IN AS",
            "POOL",
            "LEFT",
            "STATE",
            "DETAIL",
        ],
        &table,
    );
    println!();
    for entry in &next {
        let harness = entry["harness"].as_str().unwrap_or_default();
        let pool = entry["poolLabel"].as_str().unwrap_or_default();
        match entry["account"].as_str() {
            Some(account) => println!("{harness} · {pool}: next turn uses `{account}`"),
            None => println!("{harness} · {pool}: every account is at its limit"),
        }
    }
    for error in &read_errors {
        println!("Could not read quota for {error}");
    }
    Ok(())
}

/// Whose models a harness runs by default. Cross-harness review is about
/// vendor diversity, and OpenCode and Cursor front many vendors' models.
pub(crate) fn harness_vendor(harness: &str) -> &'static str {
    match harness {
        "claude-code" => "Anthropic",
        "codex" => "OpenAI",
        "antigravity" => "Google",
        _ => "depends on the model",
    }
}

/// The resident server's harness catalog, or `None` when it cannot be asked.
async fn harness_catalog(port: u16) -> Option<Vec<serde_json::Value>> {
    crate::commands::up::chat_request_via_up(
        port,
        reqwest::Method::GET,
        "/api/harnesses",
        &serde_json::json!({}),
    )
    .await
    .ok()?
    .get("harnesses")?
    .as_array()
    .cloned()
}

/// The harness and model a `--harness` chat runs on. The harness must be
/// ready; the model defaults to that harness's strongest one.
async fn agent_choice(
    port: u16,
    harness: String,
    model: Option<String>,
) -> Result<(String, Option<String>)> {
    if !crate::local::harness::is_chat_harness(&harness) {
        return Err(anyhow!(
            "Unknown harness `{harness}`. `orx agent harnesses` lists them."
        ));
    }
    let entry = harness_catalog(port).await.and_then(|all| {
        all.into_iter()
            .find(|entry| entry["id"] == harness.as_str())
    });
    if let Some(refusal) = entry.as_ref().and_then(|entry| not_ready(&harness, entry)) {
        return Err(anyhow!(refusal));
    }
    let model = model.or_else(|| {
        entry
            .as_ref()
            .and_then(|entry| entry["recommendedModel"].as_str())
            .map(str::to_string)
    });
    Ok((harness, model))
}

fn not_ready(harness: &str, entry: &serde_json::Value) -> Option<String> {
    (entry["agentReady"].as_bool() != Some(true)).then(|| {
        let name = entry["name"].as_str().unwrap_or(harness);
        format!(
            "{name} is not installed or signed in for OpenResearch, so no chat was created. \
             Omit `--harness {harness}` to use your New Chat settings, or ask the user to set \
             {name} up in OpenResearch."
        )
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessRow {
    id: &'static str,
    name: &'static str,
    vendor: &'static str,
    /// `None` when the OpenResearch server could not be asked.
    installed: Option<bool>,
    current: bool,
    /// The user allows this harness for cross-harness review.
    review: bool,
}

async fn harnesses(json: bool) -> Result<()> {
    let current = chat::launching_chat_session()
        .and_then(|id| {
            crate::store::Store::open()
                .ok()?
                .get_chat_session(&id)
                .ok()
                .flatten()
        })
        .map(|session| session.harness);
    let catalog = match chat::trusted_up_port().ok().flatten() {
        Some(port) => harness_catalog(port).await,
        None => None,
    };
    let rows: Vec<HarnessRow> = crate::local::harness::registry()
        .into_iter()
        .filter(|harness| harness.supports_chat())
        .map(|harness| HarnessRow {
            id: harness.id(),
            name: harness.name(),
            vendor: harness_vendor(harness.id()),
            installed: catalog.as_ref().map(|all| {
                all.iter()
                    .any(|entry| entry["id"] == harness.id() && entry["agentReady"] == true)
            }),
            current: current.as_deref() == Some(harness.id()),
            review: crate::config::cross_harness_allows(harness.id()),
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string(&rows)?);
        return Ok(());
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.id.to_string(),
                row.name.to_string(),
                row.vendor.to_string(),
                match row.installed {
                    Some(true) => "yes".to_string(),
                    Some(false) => "no".to_string(),
                    None => "unknown".to_string(),
                },
                if row.review { "on" } else { "off" }.to_string(),
                if row.current { "this session" } else { "" }.to_string(),
            ]
        })
        .collect();
    crate::output::print_table(
        &["HARNESS", "NAME", "VENDOR", "INSTALLED", "REVIEW", ""],
        &table,
    );
    if catalog.is_none() {
        println!(
            "Installation is unknown outside a running OpenResearch session; \
             `orx agent spawn --harness <id>` refuses a harness that is not ready."
        );
    }
    Ok(())
}

fn task_text(task: Option<String>, stdin: bool) -> Result<String> {
    let text = if stdin {
        if task.is_some() {
            return Err(anyhow!(
                "Pass the message as an argument or --stdin, not both."
            ));
        }
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        task.unwrap_or_default()
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(anyhow!(
            "Provide an opening message: orx agent spawn \"<message>\"."
        ));
    }
    Ok(text.to_string())
}

async fn spawn(
    port: u16,
    project: &str,
    prompt: &str,
    title: Option<&str>,
    agent: Option<(String, Option<String>)>,
) -> Result<()> {
    use crate::commands::up::chat_request_via_up;
    use reqwest::Method;
    use serde_json::json;

    let mut body = json!({"projectId": project});
    if let Some((harness, model)) = &agent {
        // The rest of the chat's settings are that harness's defaults; only
        // the autonomy carries over from New Chat.
        let autonomy = crate::store::Store::open()
            .ok()
            .and_then(|store| store.ui_state().ok())
            .and_then(|state| state.preferred_autonomy)
            .unwrap_or_default();
        body["harness"] = json!(harness);
        body["model"] = json!(model);
        body["autonomy"] = json!(autonomy.id());
    }
    let created = chat_request_via_up(port, Method::POST, "/api/chat/sessions", &body).await?;
    let id = created
        .pointer("/session/id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("OpenResearch returned no created chat ID."))?;
    match &agent {
        Some((harness, Some(model))) => println!("Created chat {id} on {harness} ({model})."),
        Some((harness, None)) => println!("Created chat {id} on {harness}."),
        None => println!("Created chat {id}."),
    }
    println!("Open it in OpenResearch: http://127.0.0.1:{port}/projects/{project}/tasks/{id}");
    if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
        chat_request_via_up(
            port,
            Method::PATCH,
            &format!("/api/chat/sessions/{id}"),
            &json!({"title": title}),
        )
        .await
        .map_err(|error| {
            anyhow!(
                "Chat {id} was created, but naming it failed: {error}. Open that chat to continue."
            )
        })?;
    }
    let client_turn_id = format!("ct_{}", uuid::Uuid::new_v4());
    let result = chat_request_via_up(port, Method::POST, &format!("/api/chat/sessions/{id}/message"),
        &json!({"text": prompt, "clientTurnId": client_turn_id})).await
        .map_err(|error| anyhow!("Chat {id} was created, but opening-message acceptance could not be confirmed: {error}. Check that chat before retrying."))?;
    if result
        .pointer("/turn/queued")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        println!("Opening message accepted and queued.");
    } else {
        println!("Opening message accepted.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{harness_options_from_catalog, not_ready, task_text};

    #[test]
    fn route_offers_only_ready_harnesses_from_the_catalog() {
        let catalog = vec![
            serde_json::json!({"id": "codex", "agentReady": true, "recommendedModel": "gpt-6.1-sol",
                "models": [{"id": "gpt-6.1-sol"}, {"id": "gpt-6-luna"}]}),
            serde_json::json!({"id": "cursor", "agentReady": false, "models": []}),
        ];
        let options = harness_options_from_catalog(&catalog);
        assert_eq!(options.len(), 1);
        assert_eq!(options[0].id, "codex");
        assert_eq!(options[0].vendor, "OpenAI");
        assert_eq!(options[0].models, ["gpt-6.1-sol", "gpt-6-luna"]);
        assert_eq!(options[0].recommended_model.as_deref(), Some("gpt-6.1-sol"));
    }

    #[test]
    fn a_harness_chat_is_refused_only_when_the_harness_is_not_ready() {
        let entry =
            |ready| serde_json::json!({"id": "codex", "name": "Codex", "agentReady": ready});
        assert_eq!(not_ready("codex", &entry(true)), None);
        let refusal = not_ready("codex", &entry(false)).unwrap();
        assert!(refusal.contains("Codex is not installed"), "{refusal}");
        assert!(refusal.contains("`--harness codex`"), "{refusal}");
    }

    #[test]
    fn opening_message_requires_one_nonempty_source() {
        assert_eq!(
            task_text(Some("  Inspect the code  ".into()), false).unwrap(),
            "Inspect the code"
        );
        assert!(task_text(None, false).is_err());
        assert!(task_text(Some("   ".into()), false).is_err());
        assert!(task_text(Some("argument".into()), true).is_err());
    }
}
