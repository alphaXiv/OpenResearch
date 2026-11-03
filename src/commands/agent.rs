//! Create an ordinary project chat through the resident server's chat API.

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
        } => {
            let prompt = prompt_text(
                task,
                stdin,
                "Provide an opening message: orx agent spawn \"<message>\".",
            )?;
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
            spawn(port, &project, &prompt, title.as_deref()).await
        }
        AgentCommand::Message {
            message,
            stdin,
            session,
        } => {
            let prompt = prompt_text(
                message,
                stdin,
                "Provide a message: orx agent message \"<message>\".",
            )?;
            let port = chat::trusted_up_port()?.ok_or_else(|| anyhow!(
                "Connect to the running OpenResearch server with ORX_LOCAL_SESSION=1 and ORX_UP_PORT (and ORX_UP_AUTH_TOKEN when required)."
            ))?;
            let target_session_id = match session {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => resolve_target_session(port).await?,
            };
            send(port, &target_session_id, &prompt).await
        }
    }
}

async fn resolve_target_session(port: u16) -> Result<String> {
    resolve_target_session_with(port, std::env::var(chat::PARENT_CHAT_SESSION_ENV).ok()).await
}

async fn resolve_target_session_with(port: u16, parent_env: Option<String>) -> Result<String> {
    if let Some(parent_id) = parent_env {
        let parent_id = parent_id.trim();
        if !parent_id.is_empty() {
            return Ok(parent_id.to_string());
        }
    }
    let session_id = chat::launching_chat_session().ok_or_else(|| {
        anyhow!("Pass --session <session-id> when there is no current OpenResearch chat.")
    })?;
    let session = crate::commands::up::chat_request_via_up(
        port,
        reqwest::Method::GET,
        &format!("/api/chat/sessions/{session_id}"),
        &serde_json::json!({}),
    )
    .await?;
    if let Some(parent) = session
        .pointer("/session/sideParentSessionId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(parent.to_string());
    }
    Err(anyhow!(
        "Chat {session_id} is not a side chat with a parent chat. Pass --session <session-id> to specify the target chat."
    ))
}

fn prompt_text(text: Option<String>, stdin: bool, empty_hint: &str) -> Result<String> {
    let raw = if stdin {
        if text.is_some() {
            return Err(anyhow!(
                "Pass the message as an argument or --stdin, not both."
            ));
        }
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        text.unwrap_or_default()
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("{empty_hint}"));
    }
    Ok(trimmed.to_string())
}

async fn spawn(port: u16, project: &str, prompt: &str, title: Option<&str>) -> Result<()> {
    use crate::commands::up::chat_request_via_up;
    use reqwest::Method;
    use serde_json::json;

    let created = chat_request_via_up(
        port,
        Method::POST,
        "/api/chat/sessions",
        &json!({"projectId": project}),
    )
    .await?;
    let id = created
        .pointer("/session/id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("OpenResearch returned no created chat ID."))?;
    println!("Created chat {id}.");
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

async fn send(port: u16, target_id: &str, prompt: &str) -> Result<()> {
    use crate::commands::up::chat_request_via_up;
    use reqwest::Method;
    use serde_json::json;

    let client_turn_id = format!("ct_{}", uuid::Uuid::new_v4());
    let result = chat_request_via_up(
        port,
        Method::POST,
        &format!("/api/chat/sessions/{target_id}/message"),
        &json!({"text": prompt, "clientTurnId": client_turn_id}),
    )
    .await
    .map_err(|error| anyhow!("Could not send message to chat {target_id}: {error}"))?;

    if result
        .pointer("/turn/queued")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        println!("Message sent to chat {target_id} and queued.");
    } else {
        println!("Message sent to chat {target_id}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{prompt_text, resolve_target_session_with};

    #[test]
    fn prompt_requires_one_nonempty_source() {
        assert_eq!(
            prompt_text(Some("  Inspect the code  ".into()), false, "empty").unwrap(),
            "Inspect the code"
        );
        assert!(prompt_text(None, false, "empty").is_err());
        assert!(prompt_text(Some("   ".into()), false, "empty").is_err());
        assert!(prompt_text(Some("argument".into()), true, "empty").is_err());
    }

    #[tokio::test]
    async fn resolve_target_session_prefers_parent_env() {
        let target = resolve_target_session_with(1234, Some("chat_parent_123".into()))
            .await
            .unwrap();
        assert_eq!(target, "chat_parent_123");
    }
}
