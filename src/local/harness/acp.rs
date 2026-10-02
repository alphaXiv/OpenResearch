use std::collections::HashMap;
use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    ContentBlock, NewSessionRequest, PromptRequest, TextContent,
};
use async_trait::async_trait;
use serde_json::{json, Value};

use super::{Harness, HarnessInfo, ResumeAction, TurnFailure, TurnOutcome, TurnResult};
use crate::error::{anyhow, Result};
use crate::local::acp::{
    host::{Client, Event},
    Definition,
};
use crate::local::chat::{
    DeliveryState, NativePermissionChoice, PromptAnswer, ResumeCtx, TurnCtx, WirePart, WirePrompt,
};
use crate::store::Store;

pub struct Acp(pub Definition);

#[async_trait]
impl Harness for Acp {
    fn id(&self) -> &str {
        &self.0.id
    }
    fn name(&self) -> &str {
        &self.0.name
    }
    fn supports_chat(&self) -> bool {
        true
    }

    async fn detect_snapshot(&self) -> Option<HarnessInfo> {
        Some(HarnessInfo::new(self.id(), self.name()))
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        match crate::local::acp::test_connection(&self.0).await {
            Ok(connection) => {
                info.installed = true;
                info.agent_ready = true;
                info.version = connection["agentInfo"]["version"]
                    .as_str()
                    .map(str::to_owned);
                info.models.push(super::ModelInfo::new("default"));
            }
            Err(error) => info.agent_note = Some(error.to_string()),
        }
        Some(info)
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        match run_turn(ctx, &self.0).await {
            Ok(()) => Ok(TurnOutcome::Completed),
            Err(error) => {
                ctx.host.acp.stop(&ctx.session_id).await;
                Err(TurnFailure::adapter(error, ctx.delivery_state()))
            }
        }
    }

    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        let id = prompt
            .native_id
            .as_deref()
            .ok_or_else(|| anyhow!("This approval is no longer pending"))?;
        let [selected] = answer.answers.as_slice() else {
            return Err(anyhow!("Select an agent permission option"));
        };
        ctx.host.acp.answer(&ctx.session_id, id, selected).await?;
        Ok(ResumeAction::Handled { plan_mode: None })
    }
}

struct ActiveTurn {
    client: Arc<Client>,
    host: Arc<crate::local::chat::ChatHost>,
    session_id: String,
}
impl Drop for ActiveTurn {
    fn drop(&mut self) {
        self.client.end_turn();
        let _ = self.host.resolve_stale_prompts(&self.session_id, true);
    }
}

async fn run_turn(ctx: &mut TurnCtx, definition: &Definition) -> Result<()> {
    ctx.host.resolve_stale_prompts(&ctx.session_id, true)?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let (cwd, _) = tokio::task::spawn_blocking(move || {
        crate::local::opencode::ensure_playbook(&project, &session_id, None)
    })
    .await??;
    let host = ctx.host.acp.clone();
    let mut client = host.get(&ctx.session_id, definition, &cwd).await?;
    if client.native_id().is_some() && client.native_id() != ctx.native_session_id {
        host.stop(&ctx.session_id).await;
        client = host.get(&ctx.session_id, definition, &cwd).await?;
    }
    let mut prompt = ctx.text.clone();
    if client.native_id().is_none() {
        let saved_configuration = Store::open()?
            .acp_session_state(&ctx.session_id)?
            .map(|state| state.configuration);
        let mut created = false;
        let mut configuration = None;
        if let Some(native_id) = ctx.native_session_id.clone() {
            let capabilities = &client.initialized["agentCapabilities"];
            let method = if capabilities["sessionCapabilities"]["resume"].is_object() {
                Some("session/resume")
            } else if capabilities["loadSession"].as_bool() == Some(true) {
                Some("session/load")
            } else {
                None
            };
            client.set_native_id(&native_id);
            if method.is_some() {
                match client.restore(&native_id, &cwd).await {
                    Ok(response) => configuration = Some(response),
                    Err(error)
                        if error
                            .downcast_ref::<agent_client_protocol::Error>()
                            .is_some_and(|error| {
                                error.code == agent_client_protocol::ErrorCode::ResourceNotFound
                            }) => {}
                    Err(error) => return Err(error),
                }
            }
            if configuration.is_none() {
                if let Some(recovery) =
                    super::native_recovery_context(ctx, definition.name.as_str())
                {
                    prompt = format!("{recovery}\n\n{prompt}");
                }
                ctx.upsert_part(WirePart::annotation("acp-recovery", "The agent could not restore its native session. Continuing with the saved conversation transcript."));
                ctx.flush()?;
            }
        }
        if configuration.is_none() {
            created = true;
            let response = client
                .request("session/new", NewSessionRequest::new(&cwd))
                .await?;
            let native_id = response["sessionId"]
                .as_str()
                .ok_or_else(|| anyhow!("ACP agent did not return a session ID"))?;
            client.set_native_id(native_id);
            ctx.persist_native_session_id(native_id)?;
            configuration = Some(response);
            prompt = format!(
                "Read and follow {}.\n\n{prompt}",
                crate::local::opencode::PLAYBOOK_REL
            );
        }
        if let Some(configuration) = configuration {
            Store::open()?.set_acp_configuration(&ctx.session_id, &configuration)?;
            if created {
                if let Some(saved) = saved_configuration {
                    client
                        .apply_saved_configuration(&ctx.session_id, &saved)
                        .await?;
                }
            }
            ctx.host
                .emit_session(Store::open()?.get_chat_session(&ctx.session_id)?)
                .await;
        }
    }
    let native_id = client
        .native_id()
        .ok_or_else(|| anyhow!("ACP session was not initialized"))?;
    ctx.persist_native_session_id(&native_id)?;
    let mut events = client.begin_turn();
    let _active = ActiveTurn {
        client: client.clone(),
        host: ctx.host.clone(),
        session_id: ctx.session_id.clone(),
    };
    ctx.persist_delivery(DeliveryState::Unknown)?;
    let response = client.request(
        "session/prompt",
        PromptRequest::new(
            native_id,
            vec![ContentBlock::Text(TextContent::new(prompt))],
        ),
    );
    tokio::pin!(response);
    let mut tools = HashMap::new();
    let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut deadline = tokio::time::Instant::now() + super::TURN_WATCHDOG;
    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                if client.awaiting_permission() {
                    deadline = tokio::time::Instant::now() + super::TURN_WATCHDOG;
                } else if tokio::time::Instant::now() >= deadline {
                    return Err(anyhow!("ACP turn exceeded the response timeout"));
                }
            }
            result = &mut response => {
                let result = result?;
                while let Ok(event) = events.try_recv() { apply_event(ctx, event, &mut tools)?; }
                ctx.persist_delivery(DeliveryState::Accepted)?;
                if result["stopReason"] == "cancelled" { return Err(anyhow!("Agent turn was cancelled")); }
                break;
            }
            event = events.recv() => {
                let event = event.ok_or_else(|| anyhow!("ACP process exited during the turn; the prompt was not replayed"))?;
                deadline = tokio::time::Instant::now() + super::TURN_WATCHDOG;
                ctx.mark_delivery(DeliveryState::Accepted);
                apply_event(ctx, event, &mut tools)?;
            }
        }
    }
    ctx.flush()?;
    Ok(())
}

fn apply_event(ctx: &mut TurnCtx, event: Event, tools: &mut HashMap<String, Value>) -> Result<()> {
    match event {
        Event::Permission { id, request } => {
            let choices = request["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    Some(NativePermissionChoice {
                        id: option["optionId"].as_str()?.into(),
                        label: option["name"].as_str()?.into(),
                        kind: option["kind"].as_str()?.into(),
                    })
                })
                .collect();
            ctx.upsert_part(WirePart::prompt(
                format!("acp-permission-{id}"),
                WirePrompt {
                    kind: "permission".into(),
                    native_id: Some(id),
                    native_choices: choices,
                    header: request["toolCall"]["title"].as_str().map(str::to_owned),
                    tool: request["toolCall"]["title"].as_str().map(str::to_owned),
                    tool_input: request["toolCall"].get("rawInput").cloned(),
                    ..Default::default()
                },
            ));
            ctx.flush()?;
        }
        Event::Update(update) => match update["sessionUpdate"].as_str() {
            Some(kind @ ("agent_message_chunk" | "agent_thought_chunk")) => {
                if let Some(text) = update["content"]["text"].as_str() {
                    let part_kind = if kind == "agent_thought_chunk" {
                        "reasoning"
                    } else {
                        "text"
                    };
                    let id = ctx
                        .assistant
                        .parts
                        .last()
                        .filter(|part| part.kind == part_kind && part.id.starts_with("acp-chunk-"))
                        .map(|part| part.id.clone());
                    let id = id.unwrap_or_else(|| format!("acp-chunk-{}", uuid::Uuid::new_v4()));
                    if !ctx.assistant.parts.iter().any(|part| part.id == id) {
                        ctx.upsert_part(if kind == "agent_thought_chunk" {
                            WirePart::reasoning(&id, "")
                        } else {
                            WirePart::text(&id, "")
                        });
                    }
                    ctx.append_part_text(&id, text);
                }
            }
            Some("tool_call" | "tool_call_update") => {
                if let Some(id) = update["toolCallId"].as_str() {
                    let tool = tools.entry(id.to_owned()).or_insert_with(|| json!({}));
                    if let (Some(current), Some(next)) = (tool.as_object_mut(), update.as_object())
                    {
                        current.extend(next.clone());
                    }
                    let status = match tool["status"].as_str() {
                        Some("completed") => "completed",
                        Some("failed") => "error",
                        _ => "running",
                    };
                    let mut part = WirePart::tool(
                        format!("acp-tool-{id}"),
                        tool["kind"].as_str().unwrap_or("tool"),
                        status,
                        None,
                    );
                    if let Some(state) = part.state.as_mut() {
                        state.title = tool["title"].as_str().map(str::to_owned);
                        state.input = tool.get("rawInput").cloned();
                        state.output = tool_output(tool);
                    }
                    ctx.upsert_part(part);
                }
            }
            Some("plan") => {
                let lines = update["entries"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|entry| entry["content"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n- ");
                ctx.upsert_part(WirePart::text("acp-plan", format!("- {lines}")));
            }
            Some("session_info_update") => {
                if let Some(title) = update["title"].as_str() {
                    ctx.set_title(title);
                }
            }
            Some("usage_update") => {
                if let Some(used_tokens) = update["used"].as_u64() {
                    ctx.report_usage(crate::local::chat::ContextUsage {
                        used_tokens,
                        context_window: update["size"].as_u64(),
                    });
                }
            }
            _ => {}
        },
    }
    ctx.maybe_flush();
    Ok(())
}

fn tool_output(tool: &Value) -> Option<String> {
    if let Some(content) = tool["content"].as_array() {
        let rendered = content
            .iter()
            .filter_map(|item| match item["type"].as_str() {
                Some("content") => item["content"]["text"].as_str().map(str::to_owned),
                Some("diff") => {
                    let path = item["path"].as_str()?;
                    let old = item["oldText"].as_str().unwrap_or("");
                    let new = item["newText"].as_str()?;
                    Some(format!(
                        "--- {path}\n+++ {path}\n{}{}",
                        old.lines()
                            .map(|line| format!("-{line}\n"))
                            .collect::<String>(),
                        new.lines()
                            .map(|line| format!("+{line}\n"))
                            .collect::<String>()
                    ))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !rendered.is_empty() {
            return Some(rendered);
        }
    }
    tool.get("rawOutput").map(|value| {
        value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_tool_content_and_diffs_are_readable() {
        assert_eq!(tool_output(&json!({"content": [{"type":"content", "content":{"type":"text", "text":"result"}}, {"type":"diff", "path":"file", "oldText":"old\n", "newText":"new\n"}]})).unwrap(), "result\n--- file\n+++ file\n-old\n+new\n");
        assert_eq!(
            tool_output(&json!({"rawOutput":"partial"})).unwrap(),
            "partial"
        );
    }
}
