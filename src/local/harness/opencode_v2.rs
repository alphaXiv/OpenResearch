//! V2 admits prompts before execution finishes; reconcile its durable projection.
use super::*;
use crate::local::opencode::AgentEndpoint;

async fn get(endpoint: &AgentEndpoint, path: &str) -> Result<Value> {
    Ok(endpoint
        .client
        .get(format!("{}{path}", endpoint.base_url))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn post(endpoint: &AgentEndpoint, path: &str, body: &Value) -> Result<()> {
    endpoint
        .client
        .post(format!("{}{path}", endpoint.base_url))
        .json(body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub(super) async fn run_turn(
    ctx: &mut TurnCtx,
    store: NativeStore,
    binary: crate::local::opencode::ResolvedBinary,
    database: PathBuf,
) -> Result<()> {
    ensure_runtime(ctx, store, binary, database).await?;
    let endpoint = ctx
        .host
        .opencode
        .endpoint_for(&ctx.session_id)
        .await
        .ok_or_else(|| anyhow!("OpenCode stopped during setup"))?;
    let mut model = ctx
        .model
        .as_deref()
        .and_then(|id| id.split_once('/'))
        .map(|(provider, id)| json!({"providerID":provider,"id":id}));
    if let (Some(model), Some(variant)) = (
        model.as_mut(),
        opencode_variant(ctx.reasoning_level.as_deref()),
    ) {
        model["variant"] = json!(variant);
    }
    let native_id = if let Some(id) = &ctx.native_session_id {
        get(&endpoint, &format!("/api/session/{id}")).await?;
        id.clone()
    } else {
        let directory =
            crate::local::git::existing_session_worktree_path(&ctx.project, &ctx.session_id);
        let mut body =
            json!({"agent":opencode_agent(ctx.plan_mode),"location":{"directory":directory}});
        if let Some(model) = &model {
            body["model"] = model.clone();
        }
        let session: Value = endpoint
            .client
            .post(format!("{}/api/session", endpoint.base_url))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let id = session
            .pointer("/data/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("OpenCode V2 session response has no id"))?
            .to_owned();
        ctx.persist_native_session_id(&id)?;
        id
    };
    let path = format!("/api/session/{native_id}");
    post(
        &endpoint,
        &format!("{path}/agent"),
        &json!({"agent":opencode_agent(ctx.plan_mode)}),
    )
    .await?;
    if let Some(model) = model {
        post(&endpoint, &format!("{path}/model"), &json!({"model":model})).await?;
    }
    let before = get(&endpoint, &format!("{path}/export")).await?;
    let previous: HashSet<String> = messages(&before)?
        .iter()
        .filter_map(|m| m["id"].as_str().map(str::to_owned))
        .collect();
    let prompt_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    ctx.persist_delivery(DeliveryState::Unknown)?;
    // A transport failure can follow durable admission. Never replay this POST.
    let admission = post(
        &endpoint,
        &format!("{path}/prompt"),
        &json!({"id":prompt_id,"text":ctx.text}),
    )
    .await;
    if admission.is_ok() {
        ctx.persist_delivery(DeliveryState::Accepted)?;
    }
    let mut surfaced = HashSet::new();
    let mut was_idle = false;
    loop {
        // ponytail: full projected history polling; switch to paged durable log if long chats make this costly.
        let projection = get(&endpoint, &format!("{path}/export")).await?;
        let current = messages(&projection)?;
        let delivered = current.iter().any(|m| m["id"].as_str() == Some(&prompt_id));
        merge_projection(ctx, &endpoint, &native_id, current, &previous).await?;
        if delivered && ctx.delivery_state() != DeliveryState::Accepted {
            ctx.persist_delivery(DeliveryState::Accepted)?;
        }
        prompts(ctx, &endpoint, &path, &mut surfaced).await?;
        let inbox = get(&endpoint, &format!("{path}/inbox")).await?;
        let queued = inbox["data"]
            .as_array()
            .ok_or_else(|| anyhow!("OpenCode V2 inbox response is invalid"))?
            .iter()
            .any(|item| item["id"].as_str() == Some(&prompt_id));
        let active = get(&endpoint, "/api/session/active").await?;
        let running = active["data"]
            .as_object()
            .ok_or_else(|| anyhow!("OpenCode V2 active response is invalid"))?
            .contains_key(&native_id);
        if observed_idle(&mut was_idle, queued, running) {
            let final_projection = get(&endpoint, &format!("{path}/export")).await?;
            let final_messages = messages(&final_projection)?;
            let delivered = delivered
                || final_messages
                    .iter()
                    .any(|m| m["id"].as_str() == Some(&prompt_id));
            let answered =
                merge_projection(ctx, &endpoint, &native_id, final_messages, &previous).await?;
            if delivered
                && answered
                && final_projection
                    .pointer("/data/info/outcome")
                    .and_then(Value::as_str)
                    == Some("succeeded")
            {
                ctx.persist_delivery(DeliveryState::Accepted)?;
                ctx.mark_final_text_tail();
                return Ok(());
            }
            if let Err(error) = admission {
                return Err(error);
            }
            return Err(anyhow!(
                "OpenCode V2 became idle without completing this prompt"
            ));
        }
        ctx.flush()?;
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

fn observed_idle(was_idle: &mut bool, queued: bool, running: bool) -> bool {
    let idle = !queued && !running;
    let settled = *was_idle && idle;
    *was_idle = idle;
    settled
}

async fn merge_projection(
    ctx: &mut TurnCtx,
    endpoint: &AgentEndpoint,
    native_id: &str,
    messages: &[Value],
    previous: &HashSet<String>,
) -> Result<bool> {
    let mut answered = false;
    for message in messages.iter().filter(|m| {
        m["id"].as_str().is_some_and(|id| !previous.contains(id)) && m["type"] == "assistant"
    }) {
        if message.get("retry").is_none_or(Value::is_null)
            && message
                .pointer("/time/completed")
                .is_some_and(|value| !value.is_null())
        {
            if let Some(error) = message.get("error").filter(|error| !error.is_null()) {
                let message = error_text(error);
                ctx.mark_native_retry_exhausted();
                ctx.mark_terminal_failure("opencode_terminal", &message);
                return Err(anyhow!("OpenCode V2: {message}"));
            }
        }
        answered |= message
            .pointer("/time/completed")
            .is_some_and(|v| !v.is_null());
        if let Some(used) = opencode_used_tokens(message.get("tokens")) {
            ctx.report_usage(ContextUsage {
                used_tokens: used,
                context_window: None,
            });
        }
        if let Some(retry) = message.get("retry").filter(|retry| !retry.is_null()) {
            ctx.show_retry_status(
                "native",
                &error_text(&retry["error"]),
                retry["attempt"].as_i64().unwrap_or(1),
                None,
                retry["at"].as_i64(),
            );
        } else {
            ctx.clear_retry_status();
        }
        let mut parts = projected_parts(message);
        for (part, content) in &mut parts {
            if content["type"] != "tool" || content["name"] != "subagent" {
                continue;
            }
            let Some(child_id) = content
                .pointer("/state/metadata/sessionID")
                .and_then(Value::as_str)
            else {
                continue;
            };
            let child = get(endpoint, &format!("/api/session/{child_id}/export")).await?;
            if child.pointer("/data/info/parentID").and_then(Value::as_str) != Some(native_id) {
                return Err(anyhow!("OpenCode returned an unrelated subagent session"));
            }
            part.children = self::messages(&child)?
                .iter()
                .filter(|m| m["type"] == "assistant")
                .flat_map(wire_parts)
                .collect();
        }
        for (part, _) in parts {
            ctx.upsert_part_preserving_children(part);
        }
    }
    Ok(answered)
}

fn messages(projection: &Value) -> Result<&Vec<Value>> {
    projection
        .pointer("/data/messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("OpenCode V2 history response is invalid"))
}

fn error_text(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| error.to_string())
}

fn wire_parts(message: &Value) -> Vec<WirePart> {
    projected_parts(message)
        .into_iter()
        .map(|(part, _)| part)
        .collect()
}

fn projected_parts(message: &Value) -> Vec<(WirePart, &Value)> {
    let Some(id) = message["id"].as_str() else {
        return Vec::new();
    };
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, content)| {
            let mut part = content.clone();
            part["id"] = json!(format!("{id}:{index}"));
            if content["type"] == "tool" {
                part["tool"] = content["name"].clone();
                let state = &content["state"];
                if state["status"] == "streaming" {
                    part["state"]["status"] = json!("running");
                }
                if let Some(error) = state.get("error") {
                    part["state"]["error"] = json!(error_text(error));
                }
                if let Some(output) = state["content"].as_array() {
                    part["state"]["output"] = json!(output
                        .iter()
                        .filter_map(|item| item["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n"));
                }
            }
            to_wire_part(&part).map(|part| (part, content))
        })
        .collect()
}

async fn prompts(
    ctx: &mut TurnCtx,
    endpoint: &AgentEndpoint,
    path: &str,
    surfaced: &mut HashSet<String>,
) -> Result<()> {
    let permissions = get(endpoint, &format!("{path}/permission")).await?;
    for request in permissions["data"].as_array().into_iter().flatten() {
        let Some(id) = request["id"].as_str() else {
            continue;
        };
        if surfaced.contains(id) {
            continue;
        }
        if opencode_auto_approve(ctx.permission_mode)
            && post(
                endpoint,
                &format!("{path}/permission/{id}/reply"),
                &json!({"reply":"always"}),
            )
            .await
            .is_ok()
        {
            surfaced.insert(id.to_owned());
            continue;
        }
        surface_card(
            ctx,
            WirePrompt {
                kind: "permission".into(),
                native_id: Some(id.into()),
                tool: request["action"].as_str().map(str::to_owned),
                tool_input: Some(request["resources"].clone()),
                ..Default::default()
            },
        );
        surfaced.insert(id.to_owned());
    }
    let forms = get(endpoint, &format!("{path}/form")).await?;
    for form in forms["data"].as_array().into_iter().flatten() {
        let Some(id) = form["id"].as_str() else {
            continue;
        };
        let fields = form["fields"]
            .as_array()
            .ok_or_else(|| anyhow!("OpenCode V2 form has no fields"))?;
        let mut answers = serde_json::Map::new();
        saved_form_answers(id, fields, &ctx.assistant.parts, &mut answers)?;
        for field in fields.iter().filter(|field| field_active(field, &answers)) {
            let key = field["key"]
                .as_str()
                .ok_or_else(|| anyhow!("OpenCode V2 form field has no key"))?;
            let native_id = serde_json::to_string(&(id, key))?;
            if surfaced.contains(&native_id) {
                continue;
            }
            surface_card(ctx, form_card(form, field, native_id.clone())?);
            surfaced.insert(native_id);
        }
    }
    let pending: HashSet<_> = forms["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|form| form["id"].as_str())
        .collect();
    for part in &mut ctx.assistant.parts {
        if let Some(prompt) = &mut part.prompt {
            if let Some(id) = &prompt.native_id {
                if let Ok((form, _)) = serde_json::from_str::<(String, String)>(id) {
                    if !pending.contains(form.as_str()) {
                        prompt.resolved = true;
                    }
                }
            }
        }
    }
    Ok(())
}

fn field_active(field: &Value, answers: &serde_json::Map<String, Value>) -> bool {
    field["when"]
        .as_array()
        .into_iter()
        .flatten()
        .all(|condition| {
            let Some(answer) = condition["key"].as_str().and_then(|key| answers.get(key)) else {
                return false;
            };
            let equal = answer
                .as_array()
                .map(|items| items.contains(&condition["value"]))
                .unwrap_or_else(|| answer == &condition["value"]);
            match condition["op"].as_str() {
                Some("eq") => equal,
                Some("neq") => !equal,
                _ => false,
            }
        })
}

fn form_card(form: &Value, field: &Value, native_id: String) -> Result<WirePrompt> {
    let mut options: Vec<_> = field["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|option| {
            Some(WireQuestionOption {
                label: option["label"].as_str()?.into(),
                description: option["description"].as_str().map(str::to_owned),
            })
        })
        .collect();
    if field["type"] == "boolean" {
        options = ["Yes", "No"]
            .into_iter()
            .map(|label| WireQuestionOption {
                label: label.into(),
                description: None,
            })
            .collect();
    }
    let question = field["title"]
        .as_str()
        .or_else(|| form["title"].as_str())
        .unwrap_or("Answer");
    let question = if field["type"] == "external" {
        format!(
            "{question}\n\n{}",
            field["url"].as_str().unwrap_or_default()
        )
    } else {
        question.to_owned()
    };
    if field["type"] == "external" {
        options = vec![WireQuestionOption {
            label: "Done".into(),
            description: None,
        }];
    }
    Ok(WirePrompt {
        kind: "question".into(),
        native_id: Some(native_id),
        header: form["title"].as_str().map(str::to_owned),
        question: Some(match field["description"].as_str() {
            Some(description) => format!("{question}\n\n{description}"),
            None => question,
        }),
        options,
        multi_select: field["type"] == "multiselect",
        ..Default::default()
    })
}

pub(super) async fn reply(
    ctx: &ResumeCtx,
    endpoint: &AgentEndpoint,
    prompt: &WirePrompt,
    answer: &PromptAnswer,
) -> Result<()> {
    let session = ctx
        .native_session_id
        .as_deref()
        .ok_or_else(|| anyhow!("OpenCode session has no native id"))?;
    let id = prompt
        .native_id
        .as_deref()
        .ok_or_else(|| anyhow!("OpenCode prompt has no id"))?;
    let path = format!("/api/session/{session}");
    if prompt.kind == "permission" {
        return post(
            endpoint,
            &format!("{path}/permission/{id}/reply"),
            &json!({"reply":if answer.approve {"always"} else {"reject"}}),
        )
        .await;
    }
    let (form_id, field_key): (String, String) = serde_json::from_str(id)?;
    let submitted = submitted_answers(&answer.answers, answer.note.as_ref());
    if submitted.is_empty() {
        return post(
            endpoint,
            &format!("{path}/form/{form_id}/cancel"),
            &json!({}),
        )
        .await;
    }
    let form = get(endpoint, &format!("{path}/form/{form_id}")).await?;
    let fields = form["data"]["fields"]
        .as_array()
        .ok_or_else(|| anyhow!("OpenCode V2 form has no fields"))?;
    let mut values = serde_json::Map::new();
    let messages = crate::store::Store::open()?.list_chat_messages(&ctx.session_id)?;
    for message in messages {
        let parts: Vec<WirePart> = serde_json::from_str(&message.parts_json)?;
        saved_form_answers(&form_id, fields, &parts, &mut values)?;
    }
    let field = fields
        .iter()
        .find(|field| field["key"].as_str() == Some(&field_key))
        .ok_or_else(|| anyhow!("OpenCode form field no longer exists"))?;
    values.insert(field_key, field_answer(field, submitted)?);
    let active: Vec<_> = fields
        .iter()
        .filter(|field| field_active(field, &values))
        .collect();
    if active.iter().all(|field| {
        field["key"]
            .as_str()
            .is_some_and(|key| values.contains_key(key))
    }) {
        values.retain(|key, _| {
            active
                .iter()
                .any(|field| field["key"].as_str() == Some(key) && field["type"] != "external")
        });
        post(
            endpoint,
            &format!("{path}/form/{form_id}/reply"),
            &json!({"answer":values}),
        )
        .await?;
    }
    Ok(())
}

fn saved_form_answers(
    form_id: &str,
    fields: &[Value],
    parts: &[WirePart],
    answers: &mut serde_json::Map<String, Value>,
) -> Result<()> {
    for part in parts {
        let Some(prompt) = part.prompt.as_ref().filter(|prompt| prompt.resolved) else {
            continue;
        };
        let Some(native_id) = &prompt.native_id else {
            continue;
        };
        let Ok((saved_form, key)) = serde_json::from_str::<(String, String)>(native_id) else {
            continue;
        };
        if saved_form != form_id {
            continue;
        }
        let submitted = submitted_answers(&prompt.answers, prompt.note.as_ref());
        if submitted.is_empty() {
            continue;
        }
        if let Some(field) = fields
            .iter()
            .find(|field| field["key"].as_str() == Some(&key))
        {
            answers.insert(key, field_answer(field, submitted)?);
        }
    }
    Ok(())
}

fn field_answer(field: &Value, answers: &[String]) -> Result<Value> {
    let values: Vec<_> = answers
        .iter()
        .map(|answer| {
            field["options"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|option| option["label"].as_str() == Some(answer))
                .and_then(|option| option["value"].as_str())
                .unwrap_or(answer)
                .to_owned()
        })
        .collect();
    let first = values
        .first()
        .ok_or_else(|| anyhow!("Missing form answer"))?;
    match field["type"].as_str() {
        Some("multiselect") => Ok(json!(values)),
        Some("boolean") => match first.to_ascii_lowercase().as_str() {
            "true" | "yes" => Ok(json!(true)),
            "false" | "no" => Ok(json!(false)),
            _ => Err(anyhow!("Choose Yes or No")),
        },
        Some("integer") => Ok(json!(first
            .parse::<i64>()
            .map_err(|_| anyhow!("Enter a whole number"))?)),
        Some("number") => {
            let number = first
                .parse::<f64>()
                .map_err(|_| anyhow!("Enter a number"))?;
            if !number.is_finite() {
                return Err(anyhow!("Enter a finite number"));
            }
            Ok(json!(number))
        }
        _ => Ok(json!(first)),
    }
}

fn reconnect_command(path: &Path) -> String {
    let path = path.to_string_lossy();
    #[cfg(windows)]
    {
        format!(
            "$env:OPENCODE_DB='{}'; opencode auth login",
            path.replace('\'', "''")
        )
    }
    #[cfg(not(windows))]
    {
        format!(
            "OPENCODE_DB='{}' opencode auth login",
            path.replace('\'', "'\"'\"'")
        )
    }
}

pub(super) async fn detect(
    binary: crate::local::opencode::ResolvedBinary,
    mut info: HarnessInfo,
) -> HarnessInfo {
    let catalog = tokio::spawn(async move {
        let db = native_store::prepare_opencode(NativeStore::Isolated)?;
        let _lease = crate::local::opencode::prepare_database(&binary, &db).await?;
        let mut cmd = tokio::process::Command::new(&binary.path);
        crate::local::local_models::prepare_env(&mut cmd, None)?;
        cmd.env("OPENCODE_DB", _lease.path())
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .current_dir(
                db.parent()
                    .ok_or_else(|| anyhow!("OpenCode database has no parent"))?,
            );
        let (mut child, endpoint) = crate::local::opencode::start_server(&binary, cmd).await?;
        let result = tokio::time::timeout(Duration::from_secs(30), async {
            post(&endpoint, "/api/plugin/await-activation", &json!({})).await?;
            let models = get(&endpoint, "/api/model").await?;
            let integrations = get(&endpoint, "/api/integration").await?;
            Ok::<_, anyhow::Error>((models, integrations))
        })
        .await
        .map_err(|_| anyhow!("OpenCode model discovery timed out"))
        .and_then(|result| result);
        let _ = child.kill().await;
        let _ = child.wait().await;
        result
    })
    .await;
    match catalog {
        Ok(Ok((models, integrations))) => {
            let connected: HashSet<&str> = integrations["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|integration| {
                    integration["connections"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty())
                })
                .filter_map(|integration| integration["id"].as_str())
                .collect();
            info.models = models["data"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|model| model["enabled"] == true)
                .filter_map(|model| {
                    let provider = model["providerID"].as_str()?;
                    let id = model["id"].as_str()?;
                    let variants: Vec<_> = model["variants"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v["id"].as_str())
                        .collect();
                    Some(
                        ModelInfo::new(format!("{provider}/{id}"))
                            .with_label(model["name"].as_str(), None)
                            .with_reasoning(&variants),
                    )
                })
                .collect();
            info.authenticated = !connected.is_empty();
            info.agent_ready = !info.models.is_empty();
            if !info.agent_ready {
                if info.authenticated {
                    info.agent_note = Some("OpenCode V2 listed no enabled models. Check its model configuration and re-check OpenCode.".into());
                } else {
                    info.auth_state = HarnessAuthState::NeedsLogin;
                    info.agent_note = Some(format!(
                        "Connect a provider for OpenResearch's OpenCode V2 database: `{}`",
                        reconnect_command(&native_store::opencode_db(NativeStore::Isolated))
                    ));
                }
            }
        }
        Ok(Err(error)) => info.agent_note = Some(error.to_string()),
        Err(error) => info.agent_note = Some(format!("OpenCode discovery failed: {error}")),
    }
    info
}

pub(super) async fn generate(
    binary: crate::local::opencode::ResolvedBinary,
    model: Option<String>,
    prompt: String,
    timeout: Duration,
) -> Result<String> {
    tokio::spawn(async move {
        let db = native_store::prepare_opencode(NativeStore::Isolated)?;
        let lease = crate::local::opencode::prepare_database(&binary, &db).await?;
        let mut cmd = tokio::process::Command::new(&binary.path);
        crate::local::local_models::prepare_env(&mut cmd, model.as_deref())?;
        cmd.env("OPENCODE_DB", lease.path())
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .current_dir(std::env::temp_dir());
        let (mut child, endpoint) = crate::local::opencode::start_server(&binary, cmd).await?;
        let mut body = json!({"prompt":prompt});
        if let Some((provider, model)) = model.as_deref().and_then(|model| model.split_once('/')) {
            body["model"] = json!({"providerID":provider,"id":model});
        }
        let result = tokio::time::timeout(timeout, async {
            let response: Value = endpoint
                .client
                .post(format!("{}/api/generate", endpoint.base_url))
                .json(&body)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            response
                .pointer("/data/text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("OpenCode generation returned no text"))
        })
        .await
        .map_err(|_| anyhow!("OpenCode generation timed out"))
        .and_then(|result| result);
        let _ = child.kill().await;
        let _ = child.wait().await;
        result
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn free_text_and_numeric_notes_survive_form_replies_and_reload() {
        let note = "a custom answer".to_string();
        let submitted = submitted_answers(&[], Some(&note));
        assert_eq!(
            field_answer(&json!({"type":"string"}), submitted).unwrap(),
            json!(note)
        );
        assert!(submitted_answers(&[], Some(&"  ".to_string())).is_empty());
        let chosen = vec!["Selected option".to_string()];
        assert_eq!(submitted_answers(&chosen, Some(&note)), chosen);

        let fields = vec![
            json!({"key":"count","type":"integer"}),
            json!({"key":"text","type":"string"}),
        ];
        let make_part = |key: &str, resolved, note: Option<&str>| {
            WirePart::prompt(
                key,
                WirePrompt {
                    kind: "question".into(),
                    native_id: Some(serde_json::to_string(&("frm_test", key)).unwrap()),
                    resolved,
                    note: note.map(str::to_owned),
                    ..Default::default()
                },
            )
        };
        let parts = vec![
            make_part("count", true, Some("42")),
            make_part("text", false, Some("not submitted")),
            make_part("text", true, None),
        ];
        let mut saved = serde_json::Map::new();
        saved_form_answers("frm_test", &fields, &parts, &mut saved).unwrap();
        assert_eq!(saved.get("count"), Some(&json!(42)));
        assert!(!saved.contains_key("text"));
    }

    #[test]
    fn filtered_content_keeps_its_subagent_metadata() {
        let message = json!({"id":"msg_test","content":[{"type":"future-part"},{"type":"text","text":"working"},{"type":"tool","name":"subagent","state":{"status":"running","metadata":{"sessionID":"ses_child"}}}]});
        let parts = projected_parts(&message);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[1].0.id, "msg_test:2");
        assert_eq!(
            parts[1]
                .1
                .pointer("/state/metadata/sessionID")
                .and_then(Value::as_str),
            Some("ses_child")
        );
    }

    #[test]
    fn one_idle_snapshot_cannot_finish_a_just_admitted_prompt() {
        let mut idle = false;
        assert!(!observed_idle(&mut idle, false, false));
        assert!(!observed_idle(&mut idle, false, true));
        assert!(!observed_idle(&mut idle, true, false));
        assert!(!observed_idle(&mut idle, false, false));
        assert!(observed_idle(&mut idle, false, false));
    }

    #[test]
    fn conditional_forms_wait_for_dependencies_and_preserve_types() {
        let field = json!({"key":"count","type":"integer","when":[{"key":"continue","op":"eq","value":true}]});
        let mut answers = serde_json::Map::new();
        assert!(!field_active(&field, &answers));
        answers.insert("continue".into(), json!(true));
        assert!(field_active(&field, &answers));
        assert_eq!(field_answer(&field, &["3".into()]).unwrap(), json!(3));
        assert!(field_answer(&field, &["3.5".into()]).is_err());
        assert_eq!(
            field_answer(&json!({"type":"boolean"}), &["Yes".into()]).unwrap(),
            json!(true)
        );
        assert!(field_answer(&json!({"type":"number"}), &["NaN".into()]).is_err());
    }
    #[test]
    fn projected_parts_and_form_values_preserve_native_shapes() {
        let parts = wire_parts(
            &json!({"id":"msg_one","content":[{"type":"text","text":"hello"},{"type":"tool","id":"call","name":"read","state":{"status":"completed","input":{},"content":[{"type":"text","text":"file"}]}}]}),
        );
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].text.as_deref(), Some("hello"));
        assert_eq!(
            parts[1].state.as_ref().unwrap().output.as_deref(),
            Some("file")
        );
        let form = json!({"fields":[{"key":"answer","type":"string","options":[{"label":"Continue","value":"yes"}]}]});
        assert_eq!(
            field_answer(&form["fields"][0], &["Continue".into()]).unwrap(),
            json!("yes")
        );
    }
}
