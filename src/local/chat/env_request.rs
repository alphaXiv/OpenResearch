//! `request_env`: an agent asks the user to set environment variables on a
//! card in its chat, and the values go straight to the saved environment.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{
    default_true, mark_prompt_resolved, message_json, ChatHost, PermissionDecision, PromptAnswer,
    WirePrompt,
};
use crate::error::{anyhow, bail, Context, Result};
use crate::store::{Store, StoredChatSession};

/// Where a variable is saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvScope {
    /// `~/.openresearch/env`, for every project.
    Global,
    /// The chat's project only, over the global value.
    Project,
}

/// A variable an agent asks the user to set, as its card shows it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvRequestVar {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A secret is typed masked and omitted from the tool result.
    #[serde(default = "default_true")]
    pub secret: bool,
    /// A regex the value should match. A mismatch warns; it never blocks saving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// A placeholder showing the value's form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub example: Option<String>,
    /// Answer echo: where the user saved it. Absent when they left it unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_to: Option<EnvScope>,
}

/// One value the user saved from an env card.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvAnswer {
    pub name: String,
    pub value: String,
    pub scope: EnvScope,
    /// The value does not match the requested pattern; the card warned.
    #[serde(default)]
    pub mismatched: bool,
}

impl std::fmt::Debug for EnvAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnvAnswer")
            .field("name", &self.name)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

pub const MAX_VARIABLES: usize = 10;

impl ChatHost {
    /// Shows an env card in `session_id`'s running turn and waits up to
    /// `timeout` for the user. Returns what the agent is told.
    pub async fn request_env(
        self: &Arc<Self>,
        session_id: &str,
        reason: Option<String>,
        variables: Vec<EnvRequestVar>,
        timeout: Duration,
    ) -> Result<String> {
        if !self.is_busy(session_id).await {
            bail!("This chat's turn has ended, so there is no one to show the request to.");
        }
        let prompt_id = format!("env_{}", uuid::Uuid::new_v4());
        let names = names(&variables);
        let prompt = WirePrompt {
            kind: "env".into(),
            question: reason,
            variables,
            native_id: Some(prompt_id.clone()),
            ..Default::default()
        };
        let held = self.post_held_card(session_id, prompt_id, prompt).await?;
        match held.wait(timeout).await {
            Some(PermissionDecision::Allow { updated_input }) => Ok(updated_input
                .and_then(|told| told["text"].as_str().map(str::to_string))
                .unwrap_or_default()),
            Some(PermissionDecision::Deny { message }) => bail!("{message}"),
            None => Ok(format!(
                "The user did not answer within {} minutes, so the request for {names} was withdrawn and nothing was saved. Ask again when you need them, or go on without.",
                timeout.as_secs() / 60
            )),
        }
    }

    /// Saves what the user answered on an env card and tells the waiting agent.
    pub(super) async fn answer_env_request(
        self: &Arc<Self>,
        session: StoredChatSession,
        prompt: WirePrompt,
        mut req: PromptAnswer,
    ) -> Result<()> {
        if req.from.is_some() {
            bail!("Only the user can set variables.");
        }
        if !req.approve {
            req.env.clear();
        }
        req.env.retain(|answer| !answer.value.trim().is_empty());
        if let Some(stray) = req
            .env
            .iter()
            .find(|answer| !prompt.variables.iter().any(|var| var.name == answer.name))
        {
            bail!("{} was not requested.", stray.name);
        }

        let host = self.clone();
        tokio::task::spawn_blocking(move || {
            let tx = host.pending_permissions.lock().unwrap()
                .get_mut(&req.prompt_id)
                .and_then(|pending| pending.tx.take())
                .ok_or_else(|| anyhow!("This request has ended. The agent can ask again."))?;

            let result = save(&session, &prompt, &req);
            let decision = match &result {
                Ok(told) => PermissionDecision::Allow {
                    updated_input: Some(json!({ "text": told })),
                },
                Err(error) => PermissionDecision::deny(format!(
                    "Couldn't finish saving the requested variables. Check Environment settings before asking again. {error:#}"
                )),
            };
            match mark_prompt_resolved(
                &host.msg_write,
                &session.id,
                &req.prompt_id,
                result.is_ok().then_some(&req),
            ) {
                Ok(Some(msg)) => host.emit("chat.message", message_json(&msg, &session.id)),
                Ok(None) => {}
                Err(error) => eprintln!("orx up: env request {} could not be marked resolved: {error:#}", req.prompt_id),
            }
            // The agent can disconnect after the user commits the save.
            let _ = tx.send(decision);
            result.map(|_| ())
        })
        .await
        .context("Couldn't finish the environment request")?
    }
}

/// Writes each answered value where the user chose and returns what the
/// agent is told. A secret's value is never part of it.
fn save(session: &StoredChatSession, prompt: &WirePrompt, req: &PromptAnswer) -> Result<String> {
    if !req.approve {
        return Ok(format!(
            "The user declined to set {}.",
            names(&prompt.variables)
        ));
    }
    let store = Store::open()?;
    let project_vars = store.project_env(&session.project_id)?;
    let mut lines = Vec::new();
    let mut exported = Vec::new();
    for var in &prompt.variables {
        let Some(answer) = req.env.iter().find(|answer| answer.name == var.name) else {
            lines.push(format!("The user left `{}` unset.", var.name));
            continue;
        };
        let value = answer.value.trim();
        let project = (answer.scope == EnvScope::Project).then_some(session.project_id.as_str());
        match project {
            Some(project) => store.set_project_env(project, &var.name, value)?,
            None => crate::config::write_synced_env_var(&var.name, value)?,
        }
        if let Some(description) = &var.description {
            store.describe_env(project, &var.name, description, true)?;
        }

        let shown = if var.secret {
            format!("`{}`", var.name)
        } else {
            format!("`{}={value}`", var.name)
        };
        let mut line = match answer.scope {
            EnvScope::Global => format!("Saved {shown} for every project."),
            EnvScope::Project => format!("Saved {shown} for this project."),
        };
        if answer.mismatched {
            line.push_str(" It does not match the pattern you gave; the user saved it anyway.");
        }
        let shadowed_by_project = answer.scope == EnvScope::Global
            && project_vars.iter().any(|saved| saved.key == var.name);
        if crate::local::shell_env::var(&var.name).is_some() {
            line.push_str(&format!(
                " The shell that started OpenResearch also sets `{}`, and agents get that value.",
                var.name
            ));
        } else if shadowed_by_project {
            line.push_str(" This project's own value still applies here.");
        } else {
            exported.push((var.name.as_str(), value));
        }
        lines.push(line);
    }
    if !exported.is_empty() {
        let live = super::export_to_chat_shell(&session.id, &exported).unwrap_or_else(|error| {
            eprintln!(
                "orx up: chat {} keeps its old environment until its agent restarts: {error:#}",
                session.id
            );
            false
        });
        lines.push(if live && !cfg!(windows) {
            "Your next shell command can use the saved values.".into()
        } else {
            "Your shell can use the saved values when this chat's agent next starts.".into()
        });
        lines.push("Most compute backends get the saved values. Hugging Face Jobs receives only its token and Trackio settings from the saved environment.".into());
    }
    Ok(lines.join("\n"))
}

fn names(variables: &[EnvRequestVar]) -> String {
    variables
        .iter()
        .map(|var| format!("`{}`", var.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reads `request_env`'s `variables`, rejecting what could never be saved.
pub fn parse_variables(args: &Value) -> Result<Vec<EnvRequestVar>> {
    let variables: Vec<EnvRequestVar> = serde_json::from_value(
        args.get("variables").cloned().unwrap_or(Value::Null),
    )
    .map_err(|error| anyhow!("`variables` is a list of {{name, description, …}}: {error}"))?;
    if variables.is_empty() {
        bail!("`variables` needs at least one variable.");
    }
    if variables.len() > MAX_VARIABLES {
        bail!("Ask for at most {MAX_VARIABLES} variables at a time.");
    }
    for (index, var) in variables.iter().enumerate() {
        if variables[..index].iter().any(|seen| seen.name == var.name) {
            bail!("`{}` is listed twice.", var.name);
        }
        if var.saved_to.is_some() {
            bail!("`savedTo` is the user's answer; leave it out.");
        }
    }
    Ok(variables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::chat::{TurnState, WirePart};

    fn host() -> Arc<ChatHost> {
        Arc::new(ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            Arc::new(crate::local::claude::ClaudeHost::new()),
        ))
    }

    /// A chat in a project of its own whose agent is mid-turn.
    async fn running_chat(host: &ChatHost) -> StoredChatSession {
        let store = Store::open().unwrap();
        let project = format!("proj_{}", uuid::Uuid::new_v4().simple());
        store
            .create_local_project(&crate::store::tests::local_project_fixture(&project))
            .unwrap();
        let session = StoredChatSession {
            project_id: project,
            ..crate::store::tests::chat_session_fixture(&format!(
                "chat_{}",
                uuid::Uuid::new_v4().simple()
            ))
        };
        store.create_chat_session(&session).unwrap();
        host.turns
            .lock()
            .await
            .insert(session.id.clone(), TurnState::Reserved { turn_id: None });
        session
    }

    fn var(name: &str, secret: bool, description: Option<&str>) -> EnvRequestVar {
        EnvRequestVar {
            name: name.into(),
            description: description.map(str::to_string),
            secret,
            ..Default::default()
        }
    }

    /// Starts `variables`' request in `session` and returns the card's prompt id with the call.
    async fn ask(
        host: &Arc<ChatHost>,
        session: &StoredChatSession,
        variables: Vec<EnvRequestVar>,
    ) -> (String, tokio::task::JoinHandle<Result<String>>) {
        let call = tokio::spawn({
            let host = host.clone();
            let session = session.id.clone();
            async move {
                host.request_env(
                    &session,
                    Some("for the sweep".into()),
                    variables,
                    Duration::from_secs(600),
                )
                .await
            }
        });
        let prompt_id = loop {
            let held = host
                .pending_permissions
                .lock()
                .unwrap()
                .iter()
                .find(|(_, pending)| pending.session_id == session.id)
                .map(|(id, _)| id.clone());
            if let Some(id) = held {
                break id;
            }
            tokio::task::yield_now().await;
        };
        (prompt_id, call)
    }

    fn answer(
        session: &StoredChatSession,
        prompt_id: &str,
        approve: bool,
        env: Vec<EnvAnswer>,
    ) -> PromptAnswer {
        PromptAnswer {
            session_id: session.id.clone(),
            prompt_id: prompt_id.into(),
            approve,
            resume_mode: None,
            answers: Vec::new(),
            note: None,
            annotations: Vec::new(),
            from: None,
            env,
        }
    }

    fn card(session: &StoredChatSession, prompt_id: &str) -> WirePrompt {
        let message = Store::open()
            .unwrap()
            .get_chat_message(&format!("msg_{prompt_id}"))
            .unwrap()
            .unwrap();
        let parts: Vec<WirePart> = serde_json::from_str(&message.parts_json).unwrap();
        assert_eq!(message.session_id, session.id);
        parts[0].prompt.clone().unwrap()
    }

    #[tokio::test]
    async fn saved_values_land_where_the_user_chose_and_the_agent_never_reads_a_secret() {
        let host = host();
        let session = running_chat(&host).await;
        let secret = format!("ORX_TEST_KEY_{}", uuid::Uuid::new_v4().simple());
        let setting = format!("ORX_TEST_SETTING_{}", uuid::Uuid::new_v4().simple());
        let skipped = format!("ORX_TEST_SKIPPED_{}", uuid::Uuid::new_v4().simple());
        let (prompt_id, call) = ask(
            &host,
            &session,
            vec![
                var(&secret, true, Some("From the dashboard")),
                var(&setting, false, None),
                var(&skipped, true, None),
            ],
        )
        .await;
        assert_eq!(card(&session, &prompt_id).kind, "env");

        host.respond(answer(
            &session,
            &prompt_id,
            true,
            vec![
                EnvAnswer {
                    name: secret.clone(),
                    value: " sk-it's-secret ".into(),
                    scope: EnvScope::Global,
                    mismatched: true,
                },
                EnvAnswer {
                    name: setting.clone(),
                    value: "sweeps".into(),
                    scope: EnvScope::Project,
                    mismatched: false,
                },
            ],
        ))
        .await
        .unwrap();

        let told = call.await.unwrap().unwrap();
        assert!(!told.contains("sk-it"), "{told}");
        assert!(
            told.contains(&format!(
                "Saved `{secret}` for every project. It does not match"
            )),
            "{told}"
        );
        assert!(
            told.contains(&format!("Saved `{setting}=sweeps` for this project.")),
            "{told}"
        );
        assert!(
            told.contains(&format!("The user left `{skipped}` unset.")),
            "{told}"
        );

        assert_eq!(
            crate::config::synced_env_var(&secret).as_deref(),
            Some("sk-it's-secret")
        );
        assert_eq!(crate::config::synced_env_var(&setting), None);
        let store = Store::open().unwrap();
        assert_eq!(
            store.env_descriptions().unwrap()[&secret],
            "From the dashboard"
        );
        let project = store.project_env(&session.project_id).unwrap();
        assert_eq!(project.len(), 1);
        assert_eq!(
            (project[0].key.as_str(), project[0].value.as_str()),
            (setting.as_str(), "sweeps")
        );

        let resolved = card(&session, &prompt_id);
        assert!(resolved.resolved);
        let saved: Vec<_> = resolved.variables.iter().map(|var| var.saved_to).collect();
        assert_eq!(
            saved,
            [Some(EnvScope::Global), Some(EnvScope::Project), None]
        );
        assert!(!serde_json::to_string(&resolved).unwrap().contains("sk-it"));
    }

    #[tokio::test]
    async fn a_declined_card_saves_nothing_and_an_ended_turn_withdraws_it() {
        let host = host();
        let session = running_chat(&host).await;
        let name = format!("ORX_TEST_DECLINED_{}", uuid::Uuid::new_v4().simple());
        let (prompt_id, call) = ask(&host, &session, vec![var(&name, true, None)]).await;
        let mut declined = answer(&session, &prompt_id, false, Vec::new());
        declined.env = vec![EnvAnswer {
            name: name.clone(),
            value: "x".into(),
            scope: EnvScope::Global,
            mismatched: false,
        }];
        host.respond(declined).await.unwrap();
        assert_eq!(
            call.await.unwrap().unwrap(),
            format!("The user declined to set `{name}`.")
        );
        assert_eq!(crate::config::synced_env_var(&name), None);
        assert_eq!(card(&session, &prompt_id).approved, Some(false));

        let (prompt_id, call) = ask(&host, &session, vec![var(&name, true, None)]).await;
        host.cancel_pending_permissions(&session.id);
        assert!(call.await.unwrap().is_err());
        assert!(card(&session, &prompt_id).resolved);
        let late = host
            .respond(answer(&session, &prompt_id, true, Vec::new()))
            .await;
        assert!(
            late.is_ok(),
            "a withdrawn card is a no-op, like any resolved card"
        );
    }

    #[test]
    fn requests_reject_what_could_never_be_saved() {
        let parse = |variables: Value| parse_variables(&json!({ "variables": variables }));
        assert!(parse(json!([])).is_err());
        assert!(parse(json!([{ "name": "A" }, { "name": "A" }])).is_err());
        assert!(parse(json!([{ "name": "A", "savedTo": "global" }])).is_err());
        assert!(parse(json!((0..=MAX_VARIABLES)
            .map(|i| json!({ "name": format!("V{i}") }))
            .collect::<Vec<_>>()))
        .is_err());
        let parsed = parse(json!([{ "name": "A" }])).unwrap();
        assert!(
            parsed[0].secret,
            "a variable is secret unless the agent says otherwise"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_claimed_env_save_keeps_its_reply_after_timeout_and_interrupt() {
        let host = host();
        let session = running_chat(&host).await;
        let (prompt_id, call) =
            ask(&host, &session, vec![var("ORX_TEST_PENDING", true, None)]).await;
        let tx = host
            .pending_permissions
            .lock()
            .unwrap()
            .get_mut(&prompt_id)
            .unwrap()
            .tx
            .take()
            .unwrap();

        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(601)).await;
        host.cancel_pending_permissions(&session.id);
        tokio::task::yield_now().await;
        assert!(!call.is_finished());

        tx.send(PermissionDecision::Allow {
            updated_input: Some(json!({"text": "Saved ORX_TEST_PENDING."})),
        })
        .unwrap();
        assert_eq!(call.await.unwrap().unwrap(), "Saved ORX_TEST_PENDING.");
    }
}
