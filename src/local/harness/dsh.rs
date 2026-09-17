//! DeepSeek Harness (ACP) adapter.
//!
//! Chat rides `dsh --profile acp`: one long-lived child per orx session (see
//! `local::dsh`), ACP session/new or session/resume, one session/prompt per
//! turn. Updates stream as `session/update` notifications; permission cards
//! are `session/request_permission` reverse requests answered inline.

use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::detect::{api_key, probe_bin, HarnessAuthState, HarnessInfo, ModelInfo};
use super::options::{resolve_reasoning, HarnessOptions};
use super::{Harness, ResumeAction, TurnFailure, TurnOutcome, TurnResult, TURN_WATCHDOG};
use crate::error::{anyhow, Result};
use crate::local::chat::{
    find_part_mut, ContextUsage, DeliveryState, PromptAnswer, ResumeCtx, TurnCtx, WirePart,
    WirePrompt, WireToolState,
};
use crate::local::dsh::{find_dsh, DshClient, TurnEvent};
use crate::local::opencode::ensure_playbook;

const DSH_REASONING: &[&str] = &["off", "low", "high", "max"];
const DSH_REINSTALL: &str = "Install DeepSeek Harness (`dsh`) and add it to PATH";

pub struct Dsh;

#[async_trait]
impl Harness for Dsh {
    fn id(&self) -> &'static str {
        "dsh"
    }

    fn name(&self) -> &'static str {
        "DeepSeek Harness"
    }

    fn supports_chat(&self) -> bool {
        true
    }

    async fn detect(&self) -> Option<HarnessInfo> {
        let mut info = HarnessInfo::new(self.id(), self.name());
        if let Some(bin) = find_dsh() {
            info.record_bin(&bin, probe_bin(&bin).await);
        }

        let creds = dsh_credentials_present();
        if creds {
            info.authenticated = true;
            info.auth_method = Some("apiKey");
        }

        // Key/cred presence is enough to try chat (UI gates on agent_ready).
        // session/new still fills the model cache; first prompt checks the key.
        // Avoid Unknown+!ready (picker blocked) and Unknown+ready (silent promote).
        if info.installed && !info.install_broken {
            info.auth_state = if creds {
                HarnessAuthState::Ready
            } else {
                HarnessAuthState::NeedsLogin
            };
        }
        info.agent_ready = info.installed && !info.install_broken && creds;

        let cache = load_verify_cache();
        if let Some(cache) = cache.filter(|c| c.ok) {
            info = info.with_models(
                cache
                    .models
                    .into_iter()
                    .map(|m| {
                        ModelInfo::new(m.id)
                            .with_label(m.display_name.as_deref(), m.description.as_deref())
                            .with_reasoning(DSH_REASONING)
                    })
                    .collect(),
            );
        }

        if info.install_broken {
            info.agent_note = Some(info.broken_note(DSH_REINSTALL));
        } else if !info.installed {
            info.agent_note = Some(format!("{DSH_REINSTALL}."));
        } else if !creds {
            info.agent_note =
                Some("Set DEEPSEEK_API_KEY or sign in so ~/.dsh/.credentials.yaml exists.".into());
        } else if info.models.is_empty() {
            info.agent_note =
                Some("Start a DeepSeek Harness chat once to load models, then refresh.".into());
        }

        Some(info)
    }

    async fn run_turn(&self, ctx: &mut TurnCtx) -> TurnResult {
        run_turn(ctx)
            .await
            .map(|()| TurnOutcome::Completed)
            .map_err(|error| TurnFailure::adapter(error, ctx.delivery_state()))
    }

    fn options(&self) -> HarnessOptions {
        // ponytail: no permission-mode choices in v1 — DSH sandbox modes are
        // not the same vocabulary; don't fake them.
        HarnessOptions::none().with_reasoning_levels(DSH_REASONING)
    }

    async fn resume_from_prompt(
        &self,
        ctx: &ResumeCtx,
        prompt: &WirePrompt,
        answer: &PromptAnswer,
    ) -> Result<ResumeAction> {
        if prompt.kind != "permission" {
            return Ok(ResumeAction::Nothing);
        }
        if !ctx.is_busy().await {
            ctx.host
                .resolve_zombie_prompt(&ctx.session_id, &answer.prompt_id);
            return Err(anyhow!(
                "this turn is no longer running — its prompt can't be answered"
            ));
        }
        let native = prompt
            .native_id
            .as_deref()
            .ok_or_else(|| anyhow!("dsh prompt has no reply id"))?;
        let rpc_id: Value =
            serde_json::from_str(native).map_err(|_| anyhow!("dsh prompt reply id is invalid"))?;
        let client = ctx
            .host
            .dsh
            .client_for(&ctx.session_id)
            .await
            .ok_or_else(|| anyhow!("dsh acp is not running — cannot deliver the reply"))?;
        client
            .respond(&rpc_id, permission_reply(answer.approve))
            .await?;
        Ok(ResumeAction::Handled { plan_mode: None })
    }

    fn config_home(&self) -> Option<PathBuf> {
        Some(dsh_home())
    }

    fn skill_target(&self) -> Option<PathBuf> {
        Some(
            dirs::home_dir()?
                .join(".agents")
                .join("skills")
                .join("orx")
                .join("SKILL.md"),
        )
    }

    fn skill_shim(&self) -> Option<&'static str> {
        Some(super::CLAUDE_SKILL)
    }

    fn session_skills_dir(&self) -> Option<&'static str> {
        Some(".agents/skills")
    }
}

fn dsh_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dsh")
}

fn dsh_credentials_present() -> bool {
    dsh_home().join(".credentials.yaml").is_file() || api_key("DEEPSEEK_API_KEY").is_some()
}

#[derive(serde::Serialize, serde::Deserialize)]
struct VerifyCache {
    ok: bool,
    models: Vec<CachedModel>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CachedModel {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

fn verify_cache_path() -> PathBuf {
    crate::store::data_dir().join("dsh-verify.json")
}

fn load_verify_cache() -> Option<VerifyCache> {
    let raw = std::fs::read_to_string(verify_cache_path()).ok()?;
    serde_json::from_str(&raw).ok()
}

fn store_verify_cache(models: &[ModelInfo]) {
    let cache = VerifyCache {
        ok: true,
        models: models
            .iter()
            .map(|m| CachedModel {
                id: m.id.clone(),
                display_name: m.display_name.clone(),
                description: m.description.clone(),
            })
            .collect(),
    };
    if let Ok(raw) = serde_json::to_string(&cache) {
        let _ = std::fs::write(verify_cache_path(), raw);
    }
}

/// Flatten a `model` configOption's nested groups into ModelInfo rows.
/// Values are kept as-is (DSH encodes provider+model as a JSON string).
pub(crate) fn flatten_model_options(option: &Value) -> Vec<ModelInfo> {
    let groups = option
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::new();
    for group in &groups {
        let leaves = group.get("options").and_then(Value::as_array);
        if let Some(leaves) = leaves {
            for leaf in leaves {
                push_model(&mut out, leaf);
            }
        } else {
            push_model(&mut out, group);
        }
    }
    out
}

fn push_model(out: &mut Vec<ModelInfo>, leaf: &Value) {
    let id = match leaf.get("value") {
        Some(Value::String(s)) => s.clone(),
        Some(other) if !other.is_null() => other.to_string(),
        _ => return,
    };
    out.push(
        ModelInfo::new(id)
            .with_label(
                leaf.get("name").and_then(Value::as_str),
                leaf.get("description").and_then(Value::as_str),
            )
            .with_reasoning(DSH_REASONING),
    );
}

pub(crate) fn permission_reply(approve: bool) -> Value {
    json!({
        "outcome": {
            "outcome": "selected",
            "optionId": if approve { "allow-once" } else { "reject-once" }
        }
    })
}

fn map_tool_status(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "failed" => "error",
        _ => "running",
    }
}

fn flatten_tool_content(update: &Value) -> Option<String> {
    if let Some(text) = update.get("content").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let items = update.get("content").and_then(Value::as_array)?;
    let mut parts = Vec::new();
    for item in items {
        let text = item
            .pointer("/content/text")
            .and_then(Value::as_str)
            .or_else(|| item.get("text").and_then(Value::as_str));
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            parts.push(text.to_string());
        }
    }
    (!parts.is_empty()).then_some(parts.join("\n"))
}

/// Merge a `tool_call_update` into an existing tool part, keeping prior fields.
pub(crate) fn merge_tool_update(part: &mut WirePart, update: &Value) {
    let state = part.state.get_or_insert_with(|| WireToolState {
        status: "running".into(),
        input: None,
        output: None,
        error: None,
        title: None,
    });
    if let Some(status) = update.get("status").and_then(Value::as_str) {
        state.status = map_tool_status(status).into();
    }
    if let Some(title) = update
        .get("title")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
    {
        state.title = Some(title.to_string());
    }
    if let Some(raw) = update.get("rawInput") {
        state.input = Some(raw.clone());
    }
    if let Some(output) = flatten_tool_content(update) {
        if state.status == "error" {
            state.error = Some(output);
        } else {
            state.output = Some(output);
        }
    }
}

fn chunk_text(update: &Value) -> Option<&str> {
    update
        .pointer("/content/text")
        .and_then(Value::as_str)
        .or_else(|| update.get("content").and_then(Value::as_str))
}

fn models_from_config(config_options: &Value) -> Vec<ModelInfo> {
    config_options
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o.get("id").and_then(Value::as_str) == Some("model"))
        .map(flatten_model_options)
        .unwrap_or_default()
}

fn apply_session_update(ctx: &mut TurnCtx, tools: &mut HashMap<String, Value>, params: &Value) {
    let Some(update) = params.get("update") else {
        return;
    };
    let kind = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or("");
    match kind {
        "agent_message_chunk" => {
            let Some(text) = chunk_text(update) else {
                return;
            };
            let id = update
                .get("messageId")
                .and_then(Value::as_str)
                .map(|m| format!("dsh-msg-{m}"))
                .unwrap_or_else(|| format!("dsh-msg-{}", ctx.assistant.parts.len()));
            if ctx.assistant.parts.iter().all(|p| p.id != id) {
                ctx.upsert_part(WirePart::text(&id, ""));
            }
            ctx.append_part_text(&id, text);
        }
        "agent_thought_chunk" => {
            let Some(text) = chunk_text(update) else {
                return;
            };
            let id = update
                .get("messageId")
                .and_then(Value::as_str)
                .map(|m| format!("dsh-thought-{m}"))
                .unwrap_or_else(|| format!("dsh-thought-{}", ctx.assistant.parts.len()));
            if ctx.assistant.parts.iter().all(|p| p.id != id) {
                ctx.upsert_part(WirePart::reasoning(&id, ""));
            }
            ctx.append_part_text(&id, text);
        }
        "tool_call" => {
            let Some(call_id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            let title = update
                .get("title")
                .and_then(Value::as_str)
                .or_else(|| update.get("kind").and_then(Value::as_str))
                .unwrap_or("tool");
            let raw = update.get("rawInput").cloned();
            if let Some(raw) = raw.clone() {
                tools.insert(call_id.to_string(), raw);
            }
            let status = update
                .get("status")
                .and_then(Value::as_str)
                .map(map_tool_status)
                .unwrap_or("running");
            ctx.upsert_part(WirePart {
                id: call_id.to_string(),
                kind: "tool".into(),
                text: None,
                tool: Some(title.to_string()),
                state: Some(WireToolState {
                    status: status.into(),
                    input: raw,
                    output: None,
                    error: None,
                    title: Some(title.to_string()),
                }),
                prompt: None,
                phase: None,
                children: Vec::new(),
            });
        }
        "tool_call_update" => {
            let Some(call_id) = update.get("toolCallId").and_then(Value::as_str) else {
                return;
            };
            if let Some(raw) = update.get("rawInput") {
                tools.insert(call_id.to_string(), raw.clone());
            }
            if let Some(part) = find_part_mut(&mut ctx.assistant.parts, call_id) {
                merge_tool_update(part, update);
            } else {
                let mut part = WirePart::tool(call_id, "tool", "running", None);
                merge_tool_update(&mut part, update);
                ctx.upsert_part(part);
            }
        }
        "usage_update" => {
            let used = update.get("used").and_then(Value::as_u64).unwrap_or(0);
            let size = update.get("size").and_then(Value::as_u64);
            if used > 0 || size.is_some() {
                ctx.report_usage(ContextUsage {
                    used_tokens: used,
                    context_window: size,
                });
            }
        }
        // ponytail: config_option_update ignored in stage 2; refresh picker when verify UI exists.
        _ => {}
    }
}

fn permission_card(id: &Value, params: &Value, tools: &HashMap<String, Value>) -> WirePart {
    let tool_call_id = params
        .pointer("/toolCall/toolCallId")
        .and_then(Value::as_str);
    let mut input = serde_json::Map::new();
    if let Some(call_id) = tool_call_id {
        input.insert("toolCallId".into(), json!(call_id));
        if let Some(raw) = tools.get(call_id) {
            if let Some(obj) = raw.as_object() {
                for (k, v) in obj {
                    input.insert(k.clone(), v.clone());
                }
            } else {
                input.insert("rawInput".into(), raw.clone());
            }
        }
    }
    let options: Vec<String> = params
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|o| {
                    o.get("optionId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_else(|| vec!["allow-once".into(), "reject-once".into()]);
    input.insert("options".into(), json!(options));
    let title = tool_call_id.unwrap_or("tool").to_string();
    let part_id = format!("dsh-perm-{id}");
    let prompt = WirePrompt {
        kind: "permission".into(),
        tool: Some(title),
        tool_input: Some(Value::Object(input)),
        native_id: Some(id.to_string()),
        ..Default::default()
    };
    WirePart::prompt(part_id, prompt)
}

fn prompt_blocks(playbook: Option<&str>, user: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    if let Some(text) = playbook.filter(|s| !s.trim().is_empty()) {
        blocks.push(json!({ "type": "text", "text": text }));
    }
    blocks.push(json!({ "type": "text", "text": user }));
    blocks
}

async fn apply_config(client: &DshClient, acp_id: &str, ctx: &TurnCtx) -> Result<()> {
    if let Some(model) = ctx.model.as_deref().filter(|m| !m.is_empty()) {
        client
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": acp_id,
                    "configId": "model",
                    "value": model,
                }),
            )
            .await?;
    }
    if let Some(effort) = resolve_reasoning(ctx.reasoning_level.as_deref(), DSH_REASONING) {
        client
            .request(
                "session/set_config_option",
                json!({
                    "sessionId": acp_id,
                    "configId": "reasoning_effort",
                    "value": effort,
                }),
            )
            .await?;
    }
    Ok(())
}

async fn run_turn(ctx: &mut TurnCtx) -> Result<()> {
    ctx.host
        .resolve_stale_prompts(&ctx.session_id, true)
        .await?;
    let project = ctx.project.clone();
    let session_id = ctx.session_id.clone();
    let skills_dir = Dsh.session_skills_dir();
    let (repo, playbook) =
        tokio::task::spawn_blocking(move || ensure_playbook(&project, &session_id, skills_dir))
            .await
            .map_err(|e| anyhow!("playbook task failed: {e}"))??;
    let playbook_md = std::fs::read_to_string(&playbook).unwrap_or_default();

    let client = ctx.host.dsh.ensure(&ctx.session_id).await?;
    let cwd = repo.to_string_lossy().into_owned();

    let mut first_turn = ctx.native_session_id.is_none();
    let acp_id = if let Some(existing) = client.acp_session_id() {
        existing
    } else if let Some(native) = ctx.native_session_id.clone() {
        match client
            .request(
                "session/resume",
                json!({
                    "sessionId": native,
                    "cwd": cwd,
                    "mcpServers": [],
                }),
            )
            .await
        {
            Ok(resumed) => {
                client.set_acp_session_id(&native);
                if let Some(opts) = resumed.get("configOptions") {
                    let models = models_from_config(opts);
                    if !models.is_empty() {
                        store_verify_cache(&models);
                    }
                }
                native
            }
            Err(_) => {
                first_turn = true;
                new_session(&client, &cwd).await?
            }
        }
    } else {
        new_session(&client, &cwd).await?
    };
    ctx.persist_native_session_id(&acp_id)?;
    apply_config(&client, &acp_id, ctx).await?;

    let mut user = ctx.text.clone();
    if first_turn {
        if let Some(recovery) = super::native_recovery_context(ctx, "DeepSeek Harness") {
            user = format!("{recovery}\n\n{user}");
        }
    }
    let playbook = first_turn
        .then_some(playbook_md.as_str())
        .filter(|s| !s.trim().is_empty());

    let (tx, mut rx) = mpsc::unbounded_channel();
    let _route = client.register_turn(tx);

    let params = json!({
        "sessionId": acp_id,
        "prompt": prompt_blocks(playbook, &user),
    });
    let prompt_client = client.clone();
    let mut prompt =
        tokio::spawn(async move { prompt_client.try_request("session/prompt", params).await });
    ctx.mark_delivery(DeliveryState::Unknown);

    let mut tools: HashMap<String, Value> = HashMap::new();
    let mut deadline = tokio::time::Instant::now() + TURN_WATCHDOG;
    let mut saw_update = false;

    let finish = |ctx: &mut TurnCtx,
                  tools: &mut HashMap<String, Value>,
                  rx: &mut mpsc::UnboundedReceiver<TurnEvent>| {
        while let Ok(event) = rx.try_recv() {
            match event {
                TurnEvent::Notification { method, params } if method == "session/update" => {
                    apply_session_update(ctx, tools, &params);
                }
                _ => {}
            }
        }
        let _ = ctx.flush();
    };

    loop {
        let wait_event = async {
            if client.has_unanswered() {
                Ok(rx.recv().await)
            } else {
                tokio::time::timeout_at(deadline, rx.recv()).await
            }
        };
        tokio::select! {
            result = &mut prompt => {
                ctx.mark_delivery(DeliveryState::Accepted);
                match result {
                    Ok(Ok(Ok(value))) => {
                        finish(ctx, &mut tools, &mut rx);
                        let stop = value
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .unwrap_or("end_turn");
                        if matches!(stop, "cancelled" | "cancel") {
                            client.settle_pending_permissions().await;
                            ctx.push_error("Turn interrupted.".into());
                            let _ = ctx.flush();
                            return Ok(());
                        }
                        if stop != "end_turn" && !saw_update {
                            client.settle_pending_permissions().await;
                            return Err(anyhow!("dsh session/prompt stopped: {stop}"));
                        }
                        return Ok(());
                    }
                    Ok(Ok(Err(err))) => {
                        client.settle_pending_permissions().await;
                        return Err(anyhow!("dsh session/prompt failed: {err}"))
                    }
                    Ok(Err(err)) => {
                        client.settle_pending_permissions().await;
                        return Err(err);
                    }
                    Err(e) => {
                        client.settle_pending_permissions().await;
                        return Err(anyhow!("dsh session/prompt task failed: {e}"));
                    }
                }
            }
            event = wait_event => {
                match event {
                    Ok(Some(TurnEvent::Notification { method, params })) => {
                        if method == "session/update" {
                            saw_update = true;
                            ctx.mark_delivery(DeliveryState::Accepted);
                            apply_session_update(ctx, &mut tools, &params);
                            ctx.maybe_flush();
                        }
                        deadline = tokio::time::Instant::now() + TURN_WATCHDOG;
                    }
                    Ok(Some(TurnEvent::Request { id, method, params })) => {
                        if method == "session/request_permission" {
                            ctx.mark_delivery(DeliveryState::Accepted);
                            ctx.upsert_part(permission_card(&id, &params, &tools));
                            let _ = ctx.flush();
                        } else {
                            let _ = client.respond_method_unsupported(&id).await;
                        }
                    }
                    Ok(Some(TurnEvent::Closed)) => {
                        client.settle_pending_permissions().await;
                        return Err(anyhow!("dsh acp event stream ended mid-turn"));
                    }
                    Ok(None) => {
                        client.settle_pending_permissions().await;
                        return Err(anyhow!("dsh acp event stream ended mid-turn"));
                    }
                    Err(_) => {
                        ctx.host.dsh.interrupt_session(&ctx.session_id).await;
                        return Err(anyhow!(
                            "DeepSeek Harness produced no output for {} minutes — turn interrupted",
                            TURN_WATCHDOG.as_secs() / 60
                        ));
                    }
                }
            }
        }
    }
}

async fn new_session(client: &DshClient, cwd: &str) -> Result<String> {
    let created = client
        .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
        .await?;
    let id = created
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("dsh session/new returned no sessionId"))?
        .to_string();
    client.set_acp_session_id(&id);
    if let Some(opts) = created.get("configOptions") {
        let models = models_from_config(opts);
        if !models.is_empty() {
            store_verify_cache(&models);
        }
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::super::options::REASONING_DEFAULT_ID;
    use super::*;
    use std::sync::Arc;

    #[test]
    fn flatten_keeps_model_value_as_string() {
        let option = json!({
            "id": "model",
            "options": [{
                "group": "deepseek-official",
                "name": "DeepSeek",
                "options": [{
                    "value": "[\"deepseek-official\",\"deepseek-v4-flash\"]",
                    "name": "DeepSeek-V4-Flash",
                    "description": "fast"
                }]
            }]
        });
        let models = flatten_model_options(&option);
        assert_eq!(models.len(), 1);
        assert_eq!(
            models[0].id,
            "[\"deepseek-official\",\"deepseek-v4-flash\"]"
        );
        assert_eq!(models[0].display_name.as_deref(), Some("DeepSeek-V4-Flash"));
    }

    #[test]
    fn permission_reply_matches_probe() {
        assert_eq!(
            permission_reply(true),
            json!({"outcome":{"outcome":"selected","optionId":"allow-once"}})
        );
        assert_eq!(
            permission_reply(false),
            json!({"outcome":{"outcome":"selected","optionId":"reject-once"}})
        );
    }

    #[test]
    fn merge_tool_update_keeps_input_and_maps_status() {
        let mut part = WirePart {
            id: "call_1".into(),
            kind: "tool".into(),
            text: None,
            tool: Some("bash".into()),
            state: Some(WireToolState {
                status: "running".into(),
                input: Some(json!({"command": "ls"})),
                output: None,
                error: None,
                title: Some("bash".into()),
            }),
            prompt: None,
            phase: None,
            children: Vec::new(),
        };
        merge_tool_update(
            &mut part,
            &json!({
                "status": "failed",
                "content": [{
                    "type": "content",
                    "content": { "type": "text", "text": "denied" }
                }]
            }),
        );
        let state = part.state.as_ref().unwrap();
        assert_eq!(state.status, "error");
        assert_eq!(state.error.as_deref(), Some("denied"));
        assert_eq!(state.input, Some(json!({"command": "ls"})));
        assert_eq!(state.title.as_deref(), Some("bash"));
    }

    #[test]
    fn apply_update_maps_chunks_tools_and_usage() {
        let mut ctx = TurnCtx::test_stub();
        let mut tools = HashMap::new();
        apply_session_update(
            &mut ctx,
            &mut tools,
            &json!({
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "messageId": "m1",
                    "content": { "type": "text", "text": "hmm" }
                }
            }),
        );
        apply_session_update(
            &mut ctx,
            &mut tools,
            &json!({
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": { "type": "text", "text": "hi" }
                }
            }),
        );
        apply_session_update(
            &mut ctx,
            &mut tools,
            &json!({
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": "call_1",
                    "title": "write",
                    "kind": "other",
                    "status": "in_progress",
                    "rawInput": { "file_path": "/tmp/x" }
                }
            }),
        );
        apply_session_update(
            &mut ctx,
            &mut tools,
            &json!({
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "call_1",
                    "status": "completed",
                    "content": [{
                        "type": "content",
                        "content": { "type": "text", "text": "ok" }
                    }]
                }
            }),
        );
        apply_session_update(
            &mut ctx,
            &mut tools,
            &json!({
                "update": {
                    "sessionUpdate": "usage_update",
                    "used": 12,
                    "size": 100
                }
            }),
        );
        assert!(ctx
            .assistant
            .parts
            .iter()
            .any(|p| p.id == "dsh-thought-m1" && p.text.as_deref() == Some("hmm")));
        assert!(ctx
            .assistant
            .parts
            .iter()
            .any(|p| p.id == "dsh-msg-m1" && p.text.as_deref() == Some("hi")));
        let tool = ctx
            .assistant
            .parts
            .iter()
            .find(|p| p.id == "call_1")
            .unwrap();
        let state = tool.state.as_ref().unwrap();
        assert_eq!(state.status, "completed");
        assert_eq!(state.output.as_deref(), Some("ok"));
        assert_eq!(state.input, Some(json!({"file_path": "/tmp/x"})));
        assert_eq!(ctx.context_usage.as_ref().map(|u| u.used_tokens), Some(12));
        assert_eq!(tools.get("call_1"), Some(&json!({"file_path": "/tmp/x"})));
    }

    #[test]
    fn advertised_options_are_reasoning_only() {
        let o = Dsh.options();
        assert!(o.permission_modes.is_empty());
        let ids: Vec<_> = o.reasoning_levels.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["default", "off", "low", "high", "max"]);
        assert_eq!(
            o.default_reasoning_level.as_deref(),
            Some(REASONING_DEFAULT_ID)
        );
    }

    #[cfg(unix)]
    fn write_fake_dsh(dir: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("dsh");
        // Stateful fake: two prompts, a tool turn, cancel, and resume.
        std::fs::write(
            &bin,
            r#"#!/usr/bin/env python3
import json, sys, os
STATE = os.environ.get("ORX_DSH_FAKE_STATE", "")
if "--version" in sys.argv:
    print("dsh 0.0.1"); raise SystemExit(0)

def read():
    line = sys.stdin.readline()
    return json.loads(line) if line else None

def write(obj):
    sys.stdout.write(json.dumps(obj, separators=(",", ":")) + "\n")
    sys.stdout.flush()

prompt_n = 0
while True:
    msg = read()
    if msg is None: break
    method, mid, params = msg.get("method"), msg.get("id"), msg.get("params") or {}
    if method == "initialize":
        write({"id": mid, "result": {"protocolVersion": 1, "authMethods": []}})
    elif method == "authenticate":
        write({"id": mid, "result": {}})
    elif method == "session/new":
        write({"id": mid, "result": {
            "sessionId": "fake-session",
            "configOptions": [{"id": "model", "options": [
                {"value": "[\"p\",\"m\"]", "name": "M"}
            ]}]
        }})
    elif method == "session/resume":
        write({"id": mid, "result": {
            "sessionId": params.get("sessionId", "fake-session"),
            "configOptions": [{"id": "model", "options": [
                {"value": "[\"p\",\"m\"]", "name": "M"}
            ]}]
        }})
    elif method == "session/set_config_option":
        write({"id": mid, "result": {"configOptions": []}})
    elif method == "session/prompt":
        prompt_n += 1
        sid = params.get("sessionId")
        text = (params.get("prompt") or [{}])[0].get("text", "")
        if "CANCEL" in text:
            write({"id": mid, "result": {"stopReason": "cancelled"}})
            continue
        if "TOOL" in text:
            write({"method": "session/update", "params": {"sessionId": sid, "update": {
                "sessionUpdate": "tool_call", "toolCallId": "call_1",
                "title": "write", "status": "in_progress",
                "rawInput": {"file_path": "t.txt", "content": "x"}
            }}})
            write({"method": "session/update", "params": {"sessionId": sid, "update": {
                "sessionUpdate": "tool_call_update", "toolCallId": "call_1",
                "status": "completed",
                "content": [{"type": "content", "content": {"type": "text", "text": "wrote"}}]
            }}})
        label = "TURN2" if prompt_n > 1 or "RESUME" in text else "PROBE-OK"
        write({"method": "session/update", "params": {"sessionId": sid, "update": {
            "sessionUpdate": "agent_message_chunk", "messageId": f"m{prompt_n}",
            "content": {"type": "text", "text": label}
        }}})
        write({"id": mid, "result": {"stopReason": "end_turn"}})
    elif method == "session/close":
        write({"id": mid, "result": {}})
    elif method == "session/cancel":
        pass
"#,
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    async fn with_fake_dsh<F, Fut>(f: F)
    where
        F: FnOnce(Arc<crate::local::dsh::DshHost>, std::path::PathBuf) -> Fut,
        Fut: std::future::Future<Output = crate::error::Result<()>>,
    {
        if crate::local::shell_env::find_on_path("python3").is_none() {
            return;
        }
        static BIN_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let _guard = BIN_LOCK.lock().await;
        let dir = std::env::temp_dir().join(format!("orx-dsh-fake-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        write_fake_dsh(&dir);
        let bin = dir.join("dsh");
        // Avoid mutating PATH (races other detect tests). Prefer an override env.
        let prev = std::env::var_os("ORX_DSH_BIN");
        std::env::set_var("ORX_DSH_BIN", &bin);
        let host = Arc::new(crate::local::dsh::DshHost::new());
        let result = f(host.clone(), dir.clone()).await;
        host.shutdown().await;
        match prev {
            Some(v) => std::env::set_var("ORX_DSH_BIN", v),
            None => std::env::remove_var("ORX_DSH_BIN"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result.unwrap();
    }

    #[cfg(unix)]
    async fn drain_prompt(
        client: &Arc<DshClient>,
        sid: &str,
        text: &str,
    ) -> crate::error::Result<(String, Vec<Value>)> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let _route = client.register_turn(tx);
        let prompt = client.request(
            "session/prompt",
            json!({
                "sessionId": sid,
                "prompt": [{ "type": "text", "text": text }]
            }),
        );
        let mut updates = Vec::new();
        let stop;
        tokio::pin!(prompt);
        loop {
            tokio::select! {
                biased;
                event = rx.recv() => {
                    if let Some(TurnEvent::Notification { method, params }) = event {
                        assert_eq!(method, "session/update");
                        updates.push(params);
                    }
                }
                result = &mut prompt => {
                    let value = result?;
                    stop = value
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    while let Ok(event) = rx.try_recv() {
                        if let TurnEvent::Notification { method, params } = event {
                            assert_eq!(method, "session/update");
                            updates.push(params);
                        }
                    }
                    break;
                }
            }
        }
        Ok((stop, updates))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_dsh_two_turns_tool_cancel_and_resume() {
        with_fake_dsh(|host, dir| async move {
            let client = host.ensure("fake-orx-session").await?;
            let created = client
                .request(
                    "session/new",
                    json!({ "cwd": dir.to_string_lossy(), "mcpServers": [] }),
                )
                .await?;
            let sid = created
                .get("sessionId")
                .and_then(Value::as_str)
                .unwrap()
                .to_string();
            client.set_acp_session_id(&sid);

            let (stop1, u1) = drain_prompt(&client, &sid, "hi").await?;
            assert_eq!(stop1, "end_turn");
            assert!(u1.iter().any(|p| {
                p.pointer("/update/content/text").and_then(Value::as_str) == Some("PROBE-OK")
            }));

            let (stop2, u2) = drain_prompt(&client, &sid, "TOOL please").await?;
            assert_eq!(stop2, "end_turn");
            assert!(u2.iter().any(|p| {
                p.pointer("/update/sessionUpdate").and_then(Value::as_str) == Some("tool_call")
            }));
            assert!(u2.iter().any(|p| {
                p.pointer("/update/status").and_then(Value::as_str) == Some("completed")
            }));
            assert!(u2.iter().any(|p| {
                p.pointer("/update/content/text").and_then(Value::as_str) == Some("TURN2")
            }));

            let (stop3, _) = drain_prompt(&client, &sid, "CANCEL now").await?;
            assert_eq!(stop3, "cancelled");

            host.kill_session("fake-orx-session").await;
            let client2 = host.ensure("fake-orx-session").await?;
            let resumed = client2
                .request(
                    "session/resume",
                    json!({
                        "sessionId": sid,
                        "cwd": dir.to_string_lossy(),
                        "mcpServers": []
                    }),
                )
                .await?;
            assert_eq!(
                resumed.get("sessionId").and_then(Value::as_str),
                Some(sid.as_str())
            );
            client2.set_acp_session_id(&sid);
            let (stop4, u4) = drain_prompt(&client2, &sid, "RESUME round").await?;
            assert_eq!(stop4, "end_turn");
            assert!(u4.iter().any(|p| {
                p.pointer("/update/content/text").and_then(Value::as_str) == Some("TURN2")
            }));
            Ok(())
        })
        .await;
    }
}
