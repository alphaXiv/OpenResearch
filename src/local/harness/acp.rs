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

mod auth;

static DISCOVERY_SLOTS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

pub struct Acp(pub Definition);

impl Acp {
    fn detection(&self) -> HarnessInfo {
        let mut info = HarnessInfo::new(self.id(), self.name());
        info.bin_path = crate::local::acp::executable_path(&self.0.executable)
            .map(|path| path.to_string_lossy().into_owned());
        info.installed = info.bin_path.is_some();
        if let Some(preset) = crate::local::acp::presets().into_iter().find(|preset| {
            preset.launch.id == self.0.id
                || (std::path::Path::new(&self.0.executable)
                    .file_name()
                    .is_some_and(|name| name == preset.launch.executable.as_str())
                    && self.0.arguments == preset.launch.arguments)
        }) {
            info.setup_url = Some(preset.setup_url);
            info.needs_connection_adapter = preset.needs_connection_adapter;
            info.logo = Some(preset.launch.executable.clone());
            let native_installed = match preset.launch.executable.as_str() {
                "pi-acp" => crate::local::acp::executable_path("pi").is_some(),
                "zcode-acp-server" => {
                    crate::local::acp::executable_path("zcode").is_some()
                        || dirs::home_dir()
                            .is_some_and(|home| home.join("Applications/ZCode.app").is_dir())
                        || std::path::Path::new("/Applications/ZCode.app").is_dir()
                        || dirs::data_local_dir()
                            .is_some_and(|home| home.join("Programs/ZCode/ZCode.exe").is_file())
                }
                _ => false,
            };
            info.adapter_missing = !info.installed && native_installed;
            info.installed |= native_installed;
        }
        info
    }
}

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
        Some(self.detection())
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = self.detection();
        if info.adapter_missing {
            auth::detect(&self.0, &mut info).await;
            return Some(info);
        }
        if !info.installed && info.setup_url.is_some() {
            return Some(info);
        }
        let _discovery = DISCOVERY_SLOTS.acquire().await.ok()?;
        match crate::local::acp::test_connection(&self.0).await {
            Ok(connection) => {
                info.installed = true;
                info.agent_ready = true;
                info.version = connection["agentInfo"]["version"]
                    .as_str()
                    .map(str::to_owned);
                info.models.push(super::ModelInfo::new("default"));
                auth::detect(&self.0, &mut info).await;
                match crate::local::acp::discover_models(&self.0).await {
                    Ok(configuration) => {
                        let models = advertised_models(&configuration);
                        if !models.is_empty() {
                            info.models = models;
                        }
                    }
                    Err(error) => {
                        if info.agent_ready {
                            info.agent_note = Some(format!("Could not load agent models: {error}"));
                        }
                    }
                }
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

fn advertised_models(configuration: &Value) -> Vec<super::ModelInfo> {
    fn choices(options: &Value, models: &mut Vec<super::ModelInfo>) {
        for option in options.as_array().into_iter().flatten() {
            if option["options"].is_array() {
                choices(&option["options"], models);
            } else if let (Some(id), Some(name)) =
                (option["value"].as_str(), option["name"].as_str())
            {
                models.push(
                    super::ModelInfo::new(id)
                        .with_label(Some(name), option["description"].as_str()),
                );
            }
        }
    }
    if let Some(options) = configuration["configOptions"].as_array() {
        let mut models = Vec::new();
        for option in options
            .iter()
            .filter(|option| option["category"] == "model" && option["type"] == "select")
        {
            choices(&option["options"], &mut models);
        }
        return models;
    }
    configuration["models"]["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some(
                super::ModelInfo::new(model["modelId"].as_str()?)
                    .with_label(model["name"].as_str(), model["description"].as_str()),
            )
        })
        .collect()
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
    let first_session = ctx.native_session_id.is_none();
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
    if let Some(model) = ctx
        .model
        .as_deref()
        .filter(|model| first_session && *model != "default")
    {
        let configuration = Store::open()?
            .acp_session_state(&ctx.session_id)?
            .ok_or_else(|| anyhow!("ACP session no longer exists"))?
            .configuration;
        let option = configuration["configOptions"]
            .as_array()
            .and_then(|options| {
                options
                    .iter()
                    .find(|option| option["category"] == "model" && option["type"] == "select")
            });
        let (id, current) = match option {
            Some(option) => (option["id"].as_str(), option["currentValue"].as_str()),
            None if !configuration["configOptions"].is_array() => (
                Some("model"),
                configuration["models"]["currentModelId"].as_str(),
            ),
            None => (None, None),
        };
        let id =
            id.ok_or_else(|| anyhow!("The selected model is no longer advertised by this agent"))?;
        if current != Some(model) {
            client.configure(&ctx.session_id, id, model).await?;
            ctx.host
                .emit_session(Store::open()?.get_chat_session(&ctx.session_id)?)
                .await;
        }
    }
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
    let mut tools = ToolUpdates::default();
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

#[derive(Default)]
struct ToolUpdates {
    calls: HashMap<String, Value>,
    terminals: HashMap<String, String>,
}

fn apply_event(ctx: &mut TurnCtx, event: Event, tools: &mut ToolUpdates) -> Result<()> {
    match event {
        Event::TerminalOutput { id, output } => {
            tools.terminals.insert(id.clone(), output);
            for (tool_id, tool) in &tools.calls {
                if tool["content"].as_array().is_some_and(|content| {
                    content
                        .iter()
                        .any(|item| item["type"] == "terminal" && item["terminalId"] == id)
                }) {
                    render_tool(ctx, tool_id, tool, &tools.terminals);
                }
            }
        }
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
                    tool_input: permission_input(&request["toolCall"]).or_else(|| {
                        request["toolCall"]["toolCallId"]
                            .as_str()
                            .and_then(|id| tools.calls.get(id))
                            .and_then(permission_input)
                    }),
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
                    let tool = tools
                        .calls
                        .entry(id.to_owned())
                        .or_insert_with(|| json!({}));
                    if let (Some(current), Some(next)) = (tool.as_object_mut(), update.as_object())
                    {
                        current.extend(next.clone());
                    }
                    render_tool(ctx, id, tool, &tools.terminals);
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

fn permission_input(tool: &Value) -> Option<Value> {
    if let Some(input) = tool.get("rawInput").filter(|input| input.is_object()) {
        return Some(input.clone());
    }
    tool["content"].as_array()?.iter().find_map(|item| {
        let text = item["content"]["text"].as_str()?;
        serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object)
    })
}

fn render_tool(ctx: &mut TurnCtx, id: &str, tool: &Value, terminals: &HashMap<String, String>) {
    let status = match tool["status"].as_str() {
        Some("completed") => "completed",
        Some("failed") => "error",
        _ => "running",
    };
    let mut part = WirePart::tool(
        format!("acp-tool-{id}"),
        match tool["kind"].as_str() {
            Some("execute") => "bash",
            Some(kind) => kind,
            None => "tool",
        },
        status,
        None,
    );
    if let Some(state) = part.state.as_mut() {
        state.title = tool["title"].as_str().map(str::to_owned);
        state.input = tool.get("rawInput").cloned();
        state.output = tool_output(tool, terminals);
    }
    ctx.upsert_part(part);
}

fn tool_output(tool: &Value, terminals: &HashMap<String, String>) -> Option<String> {
    if let Some(content) = tool["content"].as_array() {
        let rendered = content
            .iter()
            .filter_map(|item| match item["type"].as_str() {
                Some("content") => item["content"]["text"].as_str().map(str::to_owned),
                Some("terminal") => terminals.get(item["terminalId"].as_str()?).cloned(),
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
    fn model_catalog_preserves_native_ids_labels_and_order() {
        let modern = json!({"configOptions": [{"category":"model", "type":"select", "options":[
            {"name":"Provider", "options":[{"value":"opaque/a", "name":"Model A"}]},
            {"value":"opaque/b", "name":"Model B", "description":"Fast"}
        ]}], "models":{"availableModels":[{"modelId":"legacy", "name":"Ignored"}]}});
        let models = advertised_models(&modern);
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["opaque/a", "opaque/b"]
        );
        assert_eq!(models[0].display_name.as_deref(), Some("Model A"));
        assert_eq!(models[1].description.as_deref(), Some("Fast"));
        let legacy = advertised_models(
            &json!({"models":{"availableModels":[{"modelId":"native", "name":"Native Model"}]}}),
        );
        assert_eq!(legacy[0].id, "native");
        assert!(advertised_models(
            &json!({"configOptions":[], "models":{"availableModels":[{"modelId":"legacy"}]}})
        )
        .is_empty());
    }

    #[test]
    fn native_permission_keeps_structured_command_details() {
        assert_eq!(
            permission_input(
                &json!({"content":[{"type":"content","content":{"type":"text","text":"{\"command\":\"printf KIMI_COMMAND_OK\"}"}}]})
            ),
            Some(json!({"command":"printf KIMI_COMMAND_OK"}))
        );
        assert_eq!(
            permission_input(
                &json!({"content":[{"type":"content","content":{"type":"text","text":"not structured input"}}]})
            ),
            None
        );
    }
    #[test]
    fn embedded_terminal_output_uses_only_its_own_id() {
        let outputs = HashMap::from([("own-terminal".into(), "KIMI_COMMAND_OK".into())]);
        assert_eq!(
            tool_output(
                &json!({"content":[{"type":"terminal","terminalId":"own-terminal"}]}),
                &outputs
            )
            .as_deref(),
            Some("KIMI_COMMAND_OK")
        );
        assert_eq!(
            tool_output(
                &json!({"content":[{"type":"terminal","terminalId":"other-terminal"}]}),
                &outputs
            ),
            None
        );
    }
    #[test]
    fn native_tool_content_and_diffs_are_readable() {
        assert_eq!(tool_output(&json!({"content": [{"type":"content", "content":{"type":"text", "text":"result"}}, {"type":"diff", "path":"file", "oldText":"old\n", "newText":"new\n"}]}), &HashMap::new()).unwrap(), "result\n--- file\n+++ file\n-old\n+new\n");
        assert_eq!(
            tool_output(&json!({"rawOutput":"partial"}), &HashMap::new()).unwrap(),
            "partial"
        );
    }
}
