//! Create an ordinary project chat through the resident server's chat API.
//!
//!   orx agent spawn "<message>"   create a chat with the user's New Chat settings
//!   orx agent spawn "<message>" --harness <id> [--model <model>]
//!                                 create it on another agent, for cross-harness review
//!   orx agent harnesses           list the agents a chat can run on, with their vendors

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
    }
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
    use super::{not_ready, task_text};

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
