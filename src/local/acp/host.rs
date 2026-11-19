use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_client_protocol::schema::{v1::RequestPermissionRequest, ProtocolVersion};
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo, Responder, UntypedMessage};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::Definition;
use crate::error::{anyhow, bail, Result};

pub(super) const MAX_OUTPUT_TAIL_BYTES: usize = 8192;

pub enum Event {
    Checkpoint,
    Update(Value),
    TerminalOutput { id: String, output: String },
    Permission { id: String, request: Value },
}

struct Permission {
    responder: Responder,
    options: Vec<String>,
    auto_option: Option<String>,
}

#[derive(Default)]
pub(super) struct SessionIo {
    native_id: Mutex<Option<String>>,
    events: Mutex<Option<mpsc::UnboundedSender<Event>>>,
    replay: Mutex<Option<Vec<Event>>>,
    permissions: Mutex<HashMap<String, Permission>>,
    one_shot: std::sync::atomic::AtomicBool,
}

impl SessionIo {
    fn owns(&self, params: &Value) -> bool {
        self.native_id
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|id| params["sessionId"].as_str() == Some(id))
    }

    pub(super) fn emit(&self, event: Event) -> bool {
        let events = self.events.lock().unwrap();
        if let Some(sender) = events.as_ref() {
            return sender.send(event).is_ok();
        }
        if matches!(event, Event::Update(_) | Event::TerminalOutput { .. }) {
            if let Some(replay) = self.replay.lock().unwrap().as_mut() {
                replay.push(event);
            }
        }
        false
    }

    fn cancel_permissions(&self) {
        for (_, pending) in self.permissions.lock().unwrap().drain() {
            let _ = pending
                .responder
                .respond(json!({"outcome": {"outcome": "cancelled"}}));
        }
    }
}

pub struct Client {
    alphaxiv_signed_in: bool,
    launch: Definition,
    configuration_lock: tokio::sync::Mutex<()>,
    pub connection: ConnectionTo<Agent>,
    pub initialized: Value,
    io: Arc<SessionIo>,
    stop: Mutex<Option<oneshot::Sender<Stop>>>,
    closed: tokio::sync::watch::Receiver<Option<String>>,
    idle_since: Mutex<Option<Instant>>,
    delegated: Arc<super::delegated::Delegated>,
    pub(crate) wire: Arc<super::wire::Wire>,
}

fn exited(method: &str) -> agent_client_protocol::Error {
    let mut error = agent_client_protocol::Error::internal_error();
    error.message = format!("ACP agent exited while handling {method}");
    error.data(json!({"reason": agent_client_protocol::INCOMING_TRANSPORT_CLOSED_REASON, "method": method}))
}

pub(crate) enum Stop {
    /// `true` spares the commands the agent started in sessions of their own.
    Shutdown(bool),
    Failure(String),
}

struct ConfigurationActivity<'a> {
    client: &'a Client,
    was_idle: bool,
}

impl Drop for ConfigurationActivity<'_> {
    fn drop(&mut self) {
        let events = self.client.io.events.lock().unwrap();
        if self.was_idle && events.is_none() {
            *self.client.idle_since.lock().unwrap() = Some(Instant::now());
        }
    }
}

impl Client {
    pub fn approve_pending(&self) -> Result<Vec<(String, String)>> {
        let mut permissions = self.io.permissions.lock().unwrap();
        let approved: Vec<_> = permissions
            .iter()
            .filter_map(|(id, pending)| {
                pending
                    .auto_option
                    .as_ref()
                    .map(|option| (id.clone(), option.clone()))
            })
            .collect();
        for (id, selected) in &approved {
            let pending = permissions
                .remove(id)
                .expect("permission held under the same lock");
            pending
                .responder
                .respond(json!({"outcome": {"outcome": "selected", "optionId": selected}}))?;
        }
        Ok(approved)
    }

    pub async fn configure(&self, session_id: &str, option_id: &str, value: &str) -> Result<()> {
        let _lock = self.configuration_lock.lock().await;
        let _activity = ConfigurationActivity {
            client: self,
            was_idle: self.idle_since.lock().unwrap().take().is_some(),
        };
        let state = crate::store::Store::open()?
            .acp_session_state(session_id)?
            .ok_or_else(|| anyhow!("ACP session no longer exists"))?;
        let native_id = self
            .native_id()
            .ok_or_else(|| anyhow!("Start the conversation before changing agent options"))?;
        if let Some(options) = state.configuration["configOptions"].as_array() {
            let option = options
                .iter()
                .find(|option| {
                    option["id"].as_str() == Some(option_id) && option["type"] == "select"
                })
                .ok_or_else(|| anyhow!("This agent option is no longer available"))?;
            let valid = select_has_value(option, value);
            if !valid {
                bail!("This agent option value is no longer available");
            }
            let response = self
                .request(
                    "session/set_config_option",
                    json!({"sessionId": native_id, "configId": option_id, "value": value}),
                )
                .await?;
            if !response["configOptions"].is_array() {
                bail!("Agent did not return its updated configuration");
            }
            crate::store::Store::open()?.apply_acp_configuration_update(
                session_id,
                &json!({"sessionUpdate": "config_option_update", "configOptions": response["configOptions"]}),
            )?;
        } else {
            let (section, available, current, method, key) = match option_id {
                "mode" => (
                    "modes",
                    "availableModes",
                    "currentModeId",
                    "session/set_mode",
                    "modeId",
                ),
                "model" => (
                    "models",
                    "availableModels",
                    "currentModelId",
                    "session/set_model",
                    "modelId",
                ),
                _ => bail!("Unknown native agent option"),
            };
            if !state.configuration[section][available]
                .as_array()
                .into_iter()
                .flatten()
                .any(|option| {
                    option[if section == "models" { "modelId" } else { "id" }].as_str()
                        == Some(value)
                })
            {
                bail!("This agent option value is no longer available");
            }
            self.request(method, json!({"sessionId": native_id, key: value}))
                .await?;
            crate::store::Store::open()?.set_acp_configuration_value(
                session_id,
                &format!("$.{section}.{current}"),
                &json!(value),
            )?;
        }
        Ok(())
    }

    pub async fn apply_saved_configuration(&self, session_id: &str, saved: &Value) -> Result<()> {
        if let Some(options) = saved["configOptions"].as_array() {
            for option in options {
                let (Some(id), Some(value)) =
                    (option["id"].as_str(), option["currentValue"].as_str())
                else {
                    continue;
                };
                let current = crate::store::Store::open()?
                    .acp_session_state(session_id)?
                    .map(|state| state.configuration)
                    .unwrap_or_default();
                let available = current["configOptions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|option| option["id"].as_str() == Some(id));
                if let Some(option) = available {
                    if option["currentValue"].as_str() != Some(value)
                        && select_has_value(option, value)
                    {
                        self.configure(session_id, id, value).await?;
                    }
                }
            }
        }
        if !saved["configOptions"].is_array() {
            for (id, section, available, current, key) in [
                (
                    "model",
                    "models",
                    "availableModels",
                    "currentModelId",
                    "modelId",
                ),
                ("mode", "modes", "availableModes", "currentModeId", "id"),
            ] {
                let Some(value) = saved[section][current].as_str() else {
                    continue;
                };
                let configuration = crate::store::Store::open()?
                    .acp_session_state(session_id)?
                    .map(|state| state.configuration)
                    .unwrap_or_default();
                if !configuration["configOptions"].is_array()
                    && configuration[section][current].as_str() != Some(value)
                    && configuration[section][available]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|option| option[key].as_str() == Some(value))
                {
                    self.configure(session_id, id, value).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn restore(
        &self,
        native_id: &str,
        cwd: &Path,
        mcp_servers: Vec<agent_client_protocol::schema::v1::McpServer>,
    ) -> Result<Value> {
        use agent_client_protocol::schema::v1::{LoadSessionRequest, ResumeSessionRequest};
        let capabilities = &self.initialized["agentCapabilities"];
        let method = if capabilities["sessionCapabilities"]["resume"].is_object() {
            "session/resume"
        } else if capabilities["loadSession"].as_bool() == Some(true) {
            "session/load"
        } else {
            bail!("The agent cannot restore this session. Send a message to continue with the saved transcript");
        };
        self.set_native_id(native_id)?;
        let params = if method == "session/resume" {
            serde_json::to_value(
                ResumeSessionRequest::new(native_id.to_owned(), cwd).mcp_servers(mcp_servers),
            )?
        } else {
            serde_json::to_value(
                LoadSessionRequest::new(native_id.to_owned(), cwd).mcp_servers(mcp_servers),
            )?
        };
        let error = match self.request(method, params).await {
            Ok(response) => return Ok(response),
            Err(error) => error,
        };
        let not_found = error
            .downcast_ref::<agent_client_protocol::Error>()
            .is_some_and(|error| error.code == agent_client_protocol::ErrorCode::ResourceNotFound);
        if not_found || !self.lists_without(native_id).await {
            return Err(error);
        }
        let mut missing = agent_client_protocol::Error::resource_not_found(None);
        missing.message = format!("The agent no longer has session {native_id}: {error}");
        Err(missing.into())
    }

    /// Whether the agent's own session list leaves out `native_id`. Some agents answer restoring a
    /// session they no longer have with a generic error instead of `ResourceNotFound`.
    async fn lists_without(&self, native_id: &str) -> bool {
        if !self.initialized["agentCapabilities"]["sessionCapabilities"]["list"].is_object() {
            return false;
        }
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let params = match &cursor {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let Ok(page) = self.request("session/list", params).await else {
                return false;
            };
            let Some(sessions) = page["sessions"].as_array() else {
                return false;
            };
            if sessions
                .iter()
                .any(|session| session["sessionId"] == native_id)
            {
                return false;
            }
            match page["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_owned()),
                None => return true,
            }
        }
        false
    }

    pub fn awaiting_permission(&self) -> bool {
        !self.io.permissions.lock().unwrap().is_empty()
    }

    /// A tool call the agent reports running proves nothing once its turn ends
    /// (agy never closes an async one), so this asks the processes instead.
    pub(crate) fn running_a_command(&self) -> bool {
        self.delegated.running_a_terminal()
            || crate::local::chat::runs_a_detached_command(self.wire.keeper.pid())
    }

    pub fn native_id(&self) -> Option<String> {
        self.io.native_id.lock().unwrap().clone()
    }

    pub fn set_native_id(&self, id: &str) -> Result<()> {
        self.wire.owner.lock().unwrap()["state"]["native_id"] = json!(id);
        self.wire.persist()?;
        *self.io.native_id.lock().unwrap() = Some(id.to_owned());
        Ok(())
    }

    /// A throwaway one-shot session: no delegated tools, and no stored chat session to update.
    pub fn mark_one_shot(&self) {
        self.io
            .one_shot
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn cancel_permissions(&self) {
        self.io.cancel_permissions();
    }

    pub fn begin_turn(&self) -> mpsc::UnboundedReceiver<Event> {
        let (sender, receiver) = mpsc::unbounded_channel();
        let mut events = self.io.events.lock().unwrap();
        if let Some(replay) = self.io.replay.lock().unwrap().take() {
            for event in replay {
                if sender.send(event).is_err() {
                    break;
                }
            }
        }
        *events = Some(sender);
        *self.idle_since.lock().unwrap() = None;
        receiver
    }

    pub fn end_turn(&self) {
        if crate::keep::handing_off() {
            return;
        }
        self.wire
            .durable
            .store(false, std::sync::atomic::Ordering::SeqCst);
        self.wire.advance();
        self.io.cancel_permissions();
        self.io.events.lock().unwrap().take();
        *self.idle_since.lock().unwrap() = Some(Instant::now());
    }

    pub async fn request(
        &self,
        method: &str,
        params: impl serde::Serialize + Send,
    ) -> Result<Value> {
        self.request_with_timeout(method, params, std::time::Duration::from_secs(30))
            .await
    }

    async fn request_with_timeout(
        &self,
        method: &str,
        params: impl serde::Serialize + Send,
        timeout: std::time::Duration,
    ) -> Result<Value> {
        if self.closed.borrow().is_some() {
            let message = self.connection_error().await.to_string();
            return Err(anyhow!(exited(method)).context(message));
        }
        if matches!(method, "session/new" | "session/load" | "session/resume") {
            self.io.replay.lock().unwrap().take();
        }
        if method == "session/prompt" {
            self.wire.state.lock().unwrap().prompt_result = None;
        }
        let request = self
            .connection
            .send_request(UntypedMessage::new(method, params)?)
            .block_task();
        let response = async {
            let mut closed = self.closed.clone();
            tokio::select! {
                biased;
                result = request => {
                    if method == "session/prompt" && result.is_err() && self.wire.state.lock().unwrap().prompt_result.is_none() {
                        return Err(exited(method).into());
                    }
                    Ok(result?)
                }
                _ = async { closed.wait_for(Option::is_some).await.map(|_| ()) } => {
                    let completed = method == "session/prompt" && self.wire.state.lock().unwrap().prompt_result.is_some();
                    if completed { return self.wire.resumed_response().await; }
                    Err(exited(method).into())
                }
            }
        };
        let result = if method == "session/prompt" {
            response.await
        } else {
            match tokio::time::timeout(timeout, response).await {
                Ok(response) => response,
                Err(_) => {
                    let reason = self
                        .end(Stop::Failure(format!(
                            "{method} timed out after {} seconds",
                            timeout.as_secs()
                        )))
                        .await;
                    return Err(anyhow!(reason));
                }
            }
        };
        match result {
            Err(error) => {
                let stopping =
                    self.closed.borrow().is_some() || self.stop.lock().unwrap().is_none();
                let transport_closed = error
                    .downcast_ref::<agent_client_protocol::Error>()
                    .is_some_and(agent_client_protocol::is_incoming_transport_closed);
                if stopping || transport_closed {
                    let message = self.connection_error().await.to_string();
                    Err(error.context(message))
                } else {
                    Err(error)
                }
            }
            Ok(response) => Ok(response),
        }
    }

    pub(crate) async fn connection_error(&self) -> crate::error::Error {
        match closed_reason(self.closed.clone()).await {
            Ok(message) => anyhow!(message),
            Err(error) => {
                eprintln!("orx up: could not stop {}: {error:#}", self.launch.name);
                error.context(format!(
                    "{} closed its connection before diagnostics were available",
                    self.launch.name
                ))
            }
        }
    }

    pub fn answer(&self, id: &str, selected: &str) -> Result<()> {
        let mut permissions = self.io.permissions.lock().unwrap();
        let pending = permissions
            .get(id)
            .ok_or_else(|| anyhow!("This permission request is no longer pending"))?;
        if !pending.options.iter().any(|option| option == selected) {
            bail!("Select one of the agent's permission options");
        }
        let pending = permissions
            .remove(id)
            .expect("permission checked under the same lock");
        pending
            .responder
            .respond(json!({"outcome": {"outcome": "selected", "optionId": selected}}))?;
        Ok(())
    }

    /// Asks the agent to stop the session's running prompt.
    pub fn cancel(&self) {
        self.io.cancel_permissions();
        if let Some(native_id) = self.native_id() {
            if let Ok(notification) =
                UntypedMessage::new("session/cancel", json!({"sessionId": native_id}))
            {
                let _ = self.connection.send_notification(notification);
            }
        }
    }

    pub async fn shutdown(&self) {
        self.end(Stop::Shutdown(false)).await;
    }

    /// Ends the agent and says why it ended; a restart handing it off leaves it running.
    pub(crate) async fn end(&self, stop: Stop) -> String {
        if crate::keep::handing_off() {
            self.wire.keeper.detach();
            return format!("orx restarted while {} was working.", self.launch.name);
        }
        self.cancel();
        if let Some(sender) = self.stop.lock().unwrap().take() {
            let _ = sender.send(stop);
        }
        self.connection_error().await.to_string()
    }
}

async fn closed_reason(mut closed: tokio::sync::watch::Receiver<Option<String>>) -> Result<String> {
    let completed = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while closed.changed().await.is_ok() {}
    })
    .await
    .is_ok();
    let reason = closed
        .borrow()
        .clone()
        .ok_or_else(|| anyhow!("The ACP process ended without reporting why"))?;
    if completed {
        Ok(reason)
    } else {
        let message = format!(
            "{reason} Cleanup did not finish within 15 seconds; the agent may still be stopping."
        );
        eprintln!("orx up: {message}");
        Ok(message)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if !crate::keep::handing_off() {
            self.io.cancel_permissions();
        }
        self.stop.lock().unwrap().take();
    }
}

pub(super) fn select_has_value(option: &Value, value: &str) -> bool {
    option["type"] == "select"
        && option["options"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|choice| {
                choice["value"].as_str() == Some(value)
                    || choice["options"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|choice| choice["value"].as_str() == Some(value))
            })
}

type ClientSlot = Arc<tokio::sync::Mutex<Option<Arc<Client>>>>;

#[derive(Default)]
pub struct Host {
    clients: Mutex<HashMap<String, ClientSlot>>,
    reaper_started: std::sync::atomic::AtomicBool,
    events: Option<tokio::sync::broadcast::Sender<(&'static str, Value)>>,
    up_port: std::sync::OnceLock<u16>,
}

impl Host {
    pub fn set_up_port(&self, port: u16) {
        let _ = self.up_port.set(port);
    }

    pub fn new(events: tokio::sync::broadcast::Sender<(&'static str, Value)>) -> Self {
        Self {
            events: Some(events),
            ..Self::default()
        }
    }

    pub async fn get(
        self: &Arc<Self>,
        session_id: &str,
        definition: &Definition,
        cwd: &Path,
    ) -> Result<Arc<Client>> {
        self.start_reaper();
        let slot = self
            .clients
            .lock()
            .unwrap()
            .entry(session_id.to_owned())
            .or_default()
            .clone();
        let mut saved = slot.lock().await;
        if let Some(client) = saved.as_ref().filter(|client| {
            client.closed.borrow().is_none()
                && client.alphaxiv_signed_in == crate::alphaxiv::signed_in()
                && client.launch == *definition
        }) {
            *client.idle_since.lock().unwrap() = None;
            return Ok(client.clone());
        }
        if let Some(client) = saved.take() {
            client.shutdown().await;
        }
        let client = spawn(
            definition,
            cwd,
            session_id,
            self.events.clone(),
            self.up_port.get().copied(),
        )
        .await?;
        *client.idle_since.lock().unwrap() = None;
        *saved = Some(client.clone());
        Ok(client)
    }

    pub async fn adopt(self: &Arc<Self>, adoption: crate::local::harness::Adoption) -> Result<()> {
        if !adoption.state["initialized"].is_object() {
            bail!(
                "Kept ACP agent {} has no saved initialization result",
                adoption.keep_id
            );
        }
        let definition: Definition = serde_json::from_value(adoption.state["definition"].clone())?;
        let cwd: std::path::PathBuf = serde_json::from_value(adoption.state["cwd"].clone())?;
        let owner = crate::keep::Owner::Agent {
            harness: definition.id.clone(),
            session_id: adoption.session_id.clone(),
            state: adoption.state,
        }
        .to_value();
        let kept = crate::keep::attach(&adoption.keep_id).await?;
        let offset = adoption.resume_at.unwrap_or_else(|| kept.handle().start());
        let client = connect(
            &definition,
            &cwd,
            &adoption.session_id,
            self.events.clone(),
            self.up_port.get().copied(),
            kept,
            owner,
            Some(offset),
            StderrLog::at(stderr_path(&definition, &adoption.session_id), false),
        )
        .await?;
        for (keep_id, command_id, state) in adoption.commands {
            client.delegated.adopt(&keep_id, command_id, state).await?;
        }
        self.clients.lock().unwrap().insert(
            adoption.session_id,
            Arc::new(tokio::sync::Mutex::new(Some(client.clone()))),
        );
        if adoption.resume_at.is_none() {
            client.wire.advance();
        }
        self.start_reaper();
        Ok(())
    }

    pub async fn connected(&self, session_id: &str) -> Result<Arc<Client>> {
        let slot = self
            .clients
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or_else(|| anyhow!("The ACP connection has ended"))?;
        let client = slot
            .lock()
            .await
            .as_ref()
            .cloned()
            .filter(|client| client.closed.borrow().is_none())
            .ok_or_else(|| anyhow!("The ACP connection has ended"))?;
        Ok(client)
    }

    pub async fn answer(&self, session_id: &str, id: &str, selected: &str) -> Result<()> {
        self.connected(session_id).await?.answer(id, selected)
    }

    pub async fn stop(&self, session_id: &str) {
        let slot = self.clients.lock().unwrap().get(session_id).cloned();
        if let Some(slot) = slot {
            let mut saved = slot.lock().await;
            if let Some(client) = saved.take() {
                client.shutdown().await;
            }
        }
        self.prune_slots();
    }

    fn prune_slots(&self) {
        self.clients.lock().unwrap().retain(|_, slot| {
            Arc::strong_count(slot) > 1
                || slot.try_lock().map_or(true, |saved| {
                    // Kept until cleanup ends, so the chat cannot relaunch beside a stopping agent.
                    saved
                        .as_ref()
                        .is_some_and(|client| client.closed.has_changed().is_ok())
                })
        });
    }

    pub async fn shutdown(&self) {
        let slots = std::mem::take(&mut *self.clients.lock().unwrap());
        for slot in slots.into_values() {
            if let Some(client) = slot.lock().await.take() {
                client.shutdown().await;
            }
        }
        super::stop_one_shots().await;
    }

    fn start_reaper(self: &Arc<Self>) {
        if self
            .reaper_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(crate::local::agent_lifecycle::REAPER_INTERVAL).await;
                let Some(host) = weak.upgrade() else {
                    break;
                };
                host.reap_idle_at(
                    Instant::now(),
                    crate::local::agent_lifecycle::IdlePolicy::current(),
                )
                .await;
            }
        });
    }

    async fn reap_idle_at(&self, now: Instant, policy: crate::local::agent_lifecycle::IdlePolicy) {
        self.prune_slots();
        let slots: Vec<_> = self.clients.lock().unwrap().values().cloned().collect();
        let mut idle = Vec::new();
        for slot in slots {
            if let Ok(saved) = slot.try_lock() {
                if let Some(client) = saved.as_ref() {
                    let since = *client.idle_since.lock().unwrap();
                    if let Some(since) = since.filter(|_| !client.running_a_command()) {
                        idle.push((slot.clone(), now.saturating_duration_since(since)));
                    }
                }
            }
        }
        let over_limit = policy.over_limit(&idle.iter().map(|(_, age)| *age).collect::<Vec<_>>());
        for (index, (slot, age)) in idle.into_iter().enumerate() {
            if age < policy.idle_timeout && !over_limit.contains(&index) {
                continue;
            }
            if let Ok(mut saved) = slot.try_lock() {
                let idle = saved.as_ref().is_some_and(|client| {
                    client.idle_since.lock().unwrap().is_some()
                        && !client.awaiting_permission()
                        && !client.running_a_command()
                });
                if idle {
                    if let Some(client) = saved.take() {
                        // A dev server the agent started stays the user's.
                        client.end(Stop::Shutdown(true)).await;
                    }
                }
            }
        }
    }
}

/// How long [`spawn`] waits for an agent to answer `initialize`.
pub(super) const INITIALIZATION_BOUND: std::time::Duration = std::time::Duration::from_secs(60);

pub(super) async fn spawn(
    definition: &Definition,
    cwd: &Path,
    session_id: &str,
    events: Option<tokio::sync::broadcast::Sender<(&'static str, Value)>>,
    up_port: Option<u16>,
) -> Result<Arc<Client>> {
    let (mut command, executable) = command(definition, cwd)?;
    if up_port.is_some() {
        crate::local::chat::set_chat_session_env(
            &mut command,
            session_id,
            &definition.id,
            up_port,
        )?;
    }
    let owner = owner(definition, cwd, session_id);
    let stderr = StderrLog::at(stderr_path(definition, session_id), false);
    let kept = crate::keep::spawn(crate::keep::Keep {
        command: command.as_std(),
        io: crate::keep::Io::Pipes {
            stderr: Some(stderr.path.clone()),
        },
        retain: crate::keep::LOSSLESS,
        owner: owner.clone(),
    })
    .await
    .map_err(|error| start_error(definition, &executable, error))?;
    connect(
        definition, cwd, session_id, events, up_port, kept, owner, None, stderr,
    )
    .await
}

/// [`spawn`] for a one-shot, a catalog read or a connection test: short work no later server
/// takes over, so the agent is this server's own child, with no chat to call back into.
pub(super) async fn spawn_here(
    definition: &Definition,
    cwd: &Path,
    session_id: &str,
) -> Result<Arc<Client>> {
    let (mut command, executable) = command(definition, cwd)?;
    let directory = crate::store::data_dir();
    std::fs::create_dir_all(&directory)
        .map_err(|error| anyhow!("Could not create {}: {error}", directory.display()))?;
    let path = directory.join(format!(
        "agent-acp-here-{}.log",
        uuid::Uuid::new_v4().simple()
    ));
    let stderr = StderrLog::at(path, true);
    let kept = crate::keep::spawn_here(&mut command, Some(&stderr.path))
        .map_err(|error| start_error(definition, &executable, error))?;
    let owner = owner(definition, cwd, session_id);
    connect(
        definition, cwd, session_id, None, None, kept, owner, None, stderr,
    )
    .await
}

fn start_error(
    definition: &Definition,
    executable: &Path,
    error: crate::error::Error,
) -> crate::error::Error {
    anyhow!(
        "Could not start {} ({}): {error}",
        definition.name,
        executable.display()
    )
}

/// A kept agent's stderr log, which outlives the server that started it.
fn stderr_path(definition: &Definition, session_id: &str) -> std::path::PathBuf {
    if definition.id == "antigravity" {
        crate::local::harness::antigravity::log_path(session_id)
    } else {
        crate::store::data_dir().join(format!("agent-acp-{session_id}.log"))
    }
}

/// Where an agent's stderr goes; a failure reads back what it wrote since this connection began.
struct StderrLog {
    path: std::path::PathBuf,
    start: u64,
    /// Removed with this connection, for an agent no later server takes over.
    temporary: bool,
}

impl StderrLog {
    fn at(path: std::path::PathBuf, temporary: bool) -> Self {
        let start = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
        Self {
            path,
            start,
            temporary,
        }
    }

    /// The newest [`MAX_OUTPUT_TAIL_BYTES`] written since this connection began.
    fn tail(&self) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek};
        let mut file = std::fs::File::open(&self.path)?;
        let length = file.metadata()?.len();
        let from = self
            .start
            .max(length.saturating_sub(MAX_OUTPUT_TAIL_BYTES as u64))
            .min(length);
        file.seek(std::io::SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }
}

impl Drop for StderrLog {
    fn drop(&mut self) {
        if self.temporary {
            if let Err(error) = std::fs::remove_file(&self.path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!("orx up: could not remove {}: {error}", self.path.display());
                }
            }
        }
    }
}

fn owner(definition: &Definition, cwd: &Path, session_id: &str) -> Value {
    crate::keep::Owner::Agent {
        harness: definition.id.clone(),
        session_id: session_id.to_owned(),
        state: json!({"definition": definition, "cwd": cwd, "alphaxiv_signed_in": crate::alphaxiv::signed_in(), "id_base": 0}),
    }
    .to_value()
}

/// The agent's launch command, and the executable it runs.
fn command(
    definition: &Definition,
    cwd: &Path,
) -> Result<(tokio::process::Command, std::path::PathBuf)> {
    definition.validate_launch()?;
    let executable = super::executable_path(&definition.executable).ok_or_else(|| {
        anyhow!(
            "Executable not found: {}. Install {} or check its launch command.",
            definition.executable,
            definition.name
        )
    })?;
    let zcode = Path::new(&definition.executable)
        .file_stem()
        .is_some_and(|name| name == "zcode-acp-server")
        && definition.arguments.is_empty();
    let mut command = if zcode {
        let node = crate::local::harness::managed::node()
            .or_else(|| super::executable_path("node"))
            .ok_or_else(|| anyhow!("ZCode ACP requires Node.js on PATH"))?;
        let mut command = crate::process::tokio_command(node);
        command
            .args(["-e", include_str!("zcode-runtime.cjs")])
            .arg(&executable)
            .arg("--orx-launch");
        crate::local::chat::prepare_env(&mut command);
        crate::local::harness::managed::environment(&mut command, &executable);
        command
    } else {
        crate::local::harness::managed::command(&executable)
    };
    command.args(&definition.arguments).current_dir(cwd);
    Ok((command, executable))
}

#[allow(clippy::too_many_arguments)]
async fn connect(
    definition: &Definition,
    cwd: &Path,
    session_id: &str,
    events: Option<tokio::sync::broadcast::Sender<(&'static str, Value)>>,
    up_port: Option<u16>,
    kept: crate::keep::Kept,
    owner: Value,
    resume_at: Option<u64>,
    stderr: StderrLog,
) -> Result<Arc<Client>> {
    let alphaxiv_signed_in: bool =
        serde_json::from_value(owner["state"]["alphaxiv_signed_in"].clone())?;
    let saved_initialized = owner["state"].get("initialized").cloned();
    let saved_native_id = owner["state"]["native_id"].as_str().map(str::to_owned);
    let io = Arc::new(SessionIo::default());
    if resume_at.is_some() {
        *io.replay.lock().unwrap() = Some(Vec::new());
    }
    *io.native_id.lock().unwrap() = saved_native_id;
    let (wire, transport) = super::wire::connect(kept, owner, io.clone(), resume_at)?;
    let (reader, writer) = tokio::io::split(transport);
    let transport = ByteStreams::new(writer.compat_write(), reader.compat());
    let mut delegated = super::delegated::Delegated::new(cwd);
    delegated.set_session(session_id, &definition.id, up_port);
    let delegated = Arc::new(delegated);
    if Path::new(&definition.executable)
        .file_stem()
        .is_some_and(|name| name == "grok")
        && definition
            .arguments
            .iter()
            .map(String::as_str)
            .eq(["agent", "stdio"])
    {
        delegated.set_agent("grok");
    }
    let request_tools = delegated.clone();
    let notifications = io.clone();
    let chat_id = session_id.to_owned();
    let requests = io.clone();
    let (ready_sender, ready_receiver) = oneshot::channel();
    let ready_sender = Arc::new(Mutex::new(Some(ready_sender)));
    let initialized_sender = ready_sender.clone();
    let (stop_sender, mut stop_receiver) = oneshot::channel();
    let (closed_sender, closed_receiver) = tokio::sync::watch::channel(None);
    let task_io = io.clone();
    let initialized_tools = delegated.clone();
    let client_tools = delegated.clone();
    let task_wire = wire.clone();
    let notification_wire = wire.clone();
    let request_wire = wire.clone();
    let launch = definition.clone();
    tokio::spawn(async move {
        let run = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: UntypedMessage, _connection| {
                    let handled = async {
                        if notification.method == "session/update"
                            && notifications.owns(&notification.params)
                        {
                            let update = &notification.params["update"];
                            // A one-shot session has no stored chat session to update.
                            let one_shot = notifications.one_shot.load(std::sync::atomic::Ordering::Relaxed);
                            if !one_shot && matches!(update["sessionUpdate"].as_str(), Some("config_option_update" | "current_mode_update")) {
                                let store = crate::store::Store::open().map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                store.apply_acp_configuration_update(&chat_id, update).map_err(|error| agent_client_protocol::Error::internal_error().data(error.to_string()))?;
                                if let (Some(events), Ok(Some(session))) = (&events, store.get_chat_session(&chat_id)) {
                                    let busy = notifications.events.lock().unwrap().is_some();
                                    let _ = events.send(("chat.session", json!({"session": crate::local::chat::session_json(&session, busy)})));
                                }
                            }
                            let delivered = notifications.emit(Event::Update(update.clone()));
                            if !delivered && update["sessionUpdate"] == "session_info_update" {
                                if let Some(title) = update["title"].as_str() {
                                    if let Ok(store) = crate::store::Store::open() {
                                        if store.set_chat_session_title_if_placeholder(&chat_id, title).unwrap_or(false) {
                                            if let (Some(events), Ok(Some(session))) = (&events, store.get_chat_session(&chat_id)) {
                                                let _ = events.send(("chat.session", json!({"session": crate::local::chat::session_json(&session, false)})));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Ok(())
                    }
                    .await;
                    // Even one that failed, or the agent's next message never arrives.
                    notification_wire.processed();
                    handled
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: UntypedMessage, responder, connection| {
                    if !requests.owns(&request.params) {
                        if matches!(request.method.as_str(), "session/request_permission" | "terminal/wait_for_exit") {
                            request_wire.processed();
                        }
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("Unknown ACP session"),
                        );
                    }
                    if request.method != "session/request_permission" {
                        if requests.one_shot.load(std::sync::atomic::Ordering::Relaxed) {
                            if request.method == "terminal/wait_for_exit" { request_wire.processed(); }
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data("Tools are disabled for one-shot requests"),
                            );
                        }
                        if request.method == "terminal/wait_for_exit" {
                            request_wire.processed();
                        }
                        let delegated = request_tools.clone();
                        let terminal_io = requests.clone();
                        connection.spawn(async move {
                            let terminal_id = request.params["terminalId"].as_str().map(str::to_owned);
                            let final_output = if request.method == "terminal/release" {
                                delegated.request("terminal/output", request.params.clone()).await.ok()
                            } else { None };
                            match delegated.request(&request.method, request.params).await {
                                Ok(response) => {
                                    let output = final_output.as_ref().or_else(|| (request.method == "terminal/output").then_some(&response));
                                    if let (Some(id), Some(output)) = (terminal_id, output.and_then(|value| value["output"].as_str())) {
                                        terminal_io.emit(Event::TerminalOutput { id, output: output.to_owned() });
                                    }
                                    let _ = responder.respond(response);
                                }
                                Err(error) => {
                                    let _ = responder.respond_with_error(
                                        agent_client_protocol::Error::invalid_params()
                                            .data(error.to_string()),
                                    );
                                }
                            }
                            Ok(())
                        })?;
                        return Ok(());
                    }
                    let permission: RequestPermissionRequest = match serde_json::from_value::<RequestPermissionRequest>(request.params.clone()) {
                        Ok(permission) if !permission.options.is_empty() => permission,
                        _ => {
                            request_wire.processed();
                            return responder.respond_with_error(agent_client_protocol::Error::invalid_params().data("Invalid native permission options"));
                        }
                    };
                    // Keep lasting grants and agent mode switches behind a manual decision.
                    let auto_option = (request.params["toolCall"]["kind"] != "switch_mode")
                        .then(|| request.params["options"].as_array()?.iter()
                            .find(|option| option["kind"] == "allow_once")?["optionId"].as_str().map(str::to_owned))
                        .flatten();
                    let options = permission.options.iter().map(|option| option.option_id.to_string()).collect();
                    let id = request_wire.state.lock().unwrap().inbound.iter()
                        .find(|(_, value)| value["method"] == request.method && value["params"] == request.params)
                        .map(|(_, request)| format!("{}:{}", request_wire.keeper.id(), request["_orxOffset"])).expect("keeper recorded the permission request");
                    requests
                        .permissions
                        .lock()
                        .unwrap()
                        .insert(id.clone(), Permission { responder, options, auto_option });
                    // A hand-off leaves it pending: the successor replays it from the keeper.
                    if !requests.emit(Event::Permission {
                        id: id.clone(),
                        request: request.params,
                    }) && !crate::keep::handing_off() {
                        if let Some(pending) = requests.permissions.lock().unwrap().remove(&id) {
                            let _ = pending
                                .responder
                                .respond(json!({"outcome": {"outcome": "cancelled"}}));
                        }
                    }
                    request_wire.processed();
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, |connection: ConnectionTo<Agent>| async move {
                let initialized = if let Some(initialized) = saved_initialized {
                    initialized
                } else {
                    let initialized = connection.send_request(super::initialization()).block_task().await?;
                    if initialized.protocol_version != ProtocolVersion::V1 {
                        return Err(agent_client_protocol::Error::invalid_params().data("Incompatible ACP protocol: stable v1 is required"));
                    }
                    serde_json::to_value(initialized).expect("ACP response is serializable")
                };
                if let Some(agent) = initialized["agentInfo"]["name"].as_str() {
                    initialized_tools.set_agent(agent);
                }
                if let Some(sender) = initialized_sender.lock().unwrap().take() {
                    let _ = sender.send(Ok((connection.clone(), initialized)));
                }
                connection.incoming_closed().await;
                Ok(())
            });
        tokio::pin!(run);
        let run = std::future::poll_fn(|cx| {
            let mut result = std::task::Poll::Pending;
            // Publish closure under the same lock readers use before a failed request can escape.
            closed_sender.send_if_modified(|reason| {
                result = std::future::Future::poll(run.as_mut(), cx);
                if result.is_ready() {
                    *reason = Some(format!("{} closed its connection.", launch.name));
                    true
                } else {
                    false
                }
            });
            result
        });
        tokio::pin!(run);
        let (spare_detached, failure) = tokio::select! {
            stopped = &mut stop_receiver => {
                match stopped.unwrap_or(Stop::Shutdown(false)) {
                    Stop::Shutdown(spare) => {
                        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut run).await;
                        (spare, None)
                    }
                    Stop::Failure(message) => (false, Some(anyhow!(message))),
                }
            }
            result = &mut run => {
                let error = match result {
                    Ok(()) => anyhow!("closed its connection"),
                    Err(error) if agent_client_protocol::is_incoming_transport_closed(&error) => {
                        anyhow!(error).context("closed its connection")
                    }
                    Err(error) => {
                        // The SDK nests a cause under `data.data` (a failed task) or `data.error` (a bad reply).
                        let mut data = error.data.as_ref();
                        while let Some(Value::Object(fields)) = data {
                            data = fields.get("data").or_else(|| fields.get("error"));
                        }
                        let cause = data.and_then(Value::as_str).unwrap_or(&error.message);
                        let message = format!("connection failed: {cause}");
                        anyhow!(error).context(message)
                    }
                };
                (false, Some(error))
            }
        };
        drop(stop_receiver);
        closed_sender.send_replace(Some(match &failure {
            Some(error) => format!("{} {error}.", launch.name),
            None => format!("{} was stopped.", launch.name),
        }));
        if crate::keep::handing_off() {
            task_wire.keeper.detach();
            task_wire.gate.close();
            task_io.events.lock().unwrap().take();
            return;
        }
        task_io.cancel_permissions();
        task_io.events.lock().unwrap().take();
        // Only its own process group: Windows has no sessions to tell detached commands apart,
        // so there the whole tree goes.
        if spare_detached && cfg!(unix) {
            task_wire.keeper.signal(crate::keep::Signal::Kill);
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(5), task_wire.keeper.wait())
                    .await;
        }
        let exit = match task_wire
            .keeper
            .stop(std::time::Duration::from_secs(5))
            .await
        {
            Some(exit) => crate::keep::describe_exit(&exit),
            None => "still running 5 seconds after it was stopped".to_owned(),
        };
        task_wire.gate.close();
        delegated.shutdown().await;
        let message = if let Some(error) = failure {
            let (mut stderr, stderr_read) = match stderr.tail() {
                Ok(bytes) => (String::from_utf8_lossy(&bytes).trim().to_owned(), None),
                Err(error) => (
                    String::new(),
                    Some(format!("could not read {}: {error}", stderr.path.display())),
                ),
            };
            let mut start = stderr.len().saturating_sub(MAX_OUTPUT_TAIL_BYTES);
            while !stderr.is_char_boundary(start) {
                start += 1;
            }
            stderr.drain(..start);
            let excerpt = stderr
                .lines()
                .rev()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            let mut start = excerpt.len().saturating_sub(320);
            while !excerpt.is_char_boundary(start) {
                start += 1;
            }
            let excerpt = &excerpt[start..];
            let detail = if excerpt.is_empty() {
                String::new()
            } else {
                format!(" Agent output: {excerpt}.")
            };
            let record = json!({
                "agent": launch.name,
                "executable": super::executable_path(&launch.executable)
                    .unwrap_or_else(|| launch.executable.clone().into())
                    .to_string_lossy(),
                "arguments": launch.arguments,
                "pid": task_wire.keeper.pid(),
                "error": format!("{error:#}"),
                "exit": exit,
                "stderr": stderr,
                "stderrReadError": stderr_read,
                "discardedStdout": task_wire.discarded.lock().unwrap().trim_end(),
            });
            match crate::local::chat::harness_log("acp").and_then(|mut file| {
                file.write_all(format!("{record}\n").as_bytes())
                    .map_err(Into::into)
            }) {
                Ok(()) => format!(
                    "{} {error}.{detail} See {} for details, then try again.",
                    launch.name,
                    crate::local::chat::harness_log_path("acp").display()
                ),
                Err(log_error) => {
                    eprintln!("orx up: ACP failure diagnostics: {record}");
                    format!(
                        "{} {error}.{detail} Could not save the diagnostics: {log_error}.",
                        launch.name
                    )
                }
            }
        } else {
            format!("{} was stopped.", launch.name)
        };
        drop(stderr);
        let _ = closed_sender.send(Some(message.clone()));
        if let Some(sender) = ready_sender.lock().unwrap().take() {
            let _ = sender.send(Err(message));
        }
    });
    let initialized =
        tokio::time::timeout(crate::config::paced(INITIALIZATION_BOUND), ready_receiver).await;
    match initialized {
        Ok(Ok(Ok((connection, initialized)))) => {
            wire.owner.lock().unwrap()["state"]["initialized"] = initialized.clone();
            wire.persist()?;
            return Ok(Arc::new(Client {
                alphaxiv_signed_in,
                launch: definition.clone(),
                configuration_lock: tokio::sync::Mutex::new(()),
                connection,
                initialized,
                io,
                stop: Mutex::new(Some(stop_sender)),
                closed: closed_receiver,
                idle_since: Mutex::new(Some(Instant::now())),
                delegated: client_tools,
                wire,
            }));
        }
        Ok(Ok(Err(message))) => bail!("{message}"),
        Err(_) => {
            let _ = stop_sender.send(Stop::Failure(format!(
                "did not initialize within {} seconds",
                INITIALIZATION_BOUND.as_secs()
            )));
        }
        Ok(Err(_)) => drop(stop_sender),
    }
    bail!("{}", closed_reason(closed_receiver).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn definition(mode: &str) -> Definition {
        Definition {
            id: format!("acp:{}", uuid::Uuid::new_v4()),
            name: "Peer".into(),
            executable: crate::local::acp::fake_agent_interpreter("python3"),
            arguments: vec![
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/local/harness/fixtures/acp-peer.py"
                )
                .into(),
                mode.into(),
            ],
            unknown: Default::default(),
        }
    }

    async fn new_client(host: &Arc<Host>, id: &str) -> Arc<Client> {
        let client = host
            .get(id, &definition("normal"), &std::env::temp_dir())
            .await
            .unwrap();
        let response = client
            .request(
                "session/new",
                json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
            )
            .await
            .unwrap();
        client
            .set_native_id(response["sessionId"].as_str().unwrap())
            .unwrap();
        client
    }

    async fn turn(client: &Client, text: &str) {
        let session = client.native_id().unwrap();
        let prompt = json!([{"type": "text", "text": format!("ACP_TEST:{text}")}]);
        client
            .request(
                "session/prompt",
                json!({"sessionId": session, "prompt": prompt}),
            )
            .await
            .unwrap();
        client.end_turn();
    }

    fn diagnostics(name: &str) -> Value {
        let log = std::fs::read_to_string(crate::local::chat::harness_log_path("acp")).unwrap();
        log.lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|entry| entry["agent"] == name)
            .unwrap()
    }

    async fn assert_startup_socket_closed(mut gate: tokio::net::TcpStream) {
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), gate.read(&mut [0]))
            .await
            .unwrap();
        match result {
            Ok(0) => {}
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            result => panic!("The stopped agent kept its startup socket open: {result:?}"),
        }
    }

    #[tokio::test]
    async fn setup_and_chats_accept_windows_framing_fragmented_unicode_and_stdout_noise() {
        let host = Arc::new(Host::default());
        for mode in [
            "transport-crlf",
            "transport-fragments",
            "transport-blank",
            "transport-noise",
            "transport-bom",
        ] {
            let launch = definition(mode);
            let connected = super::super::test_connection(&launch).await.unwrap();
            assert_eq!(connected["status"], "connected");
            if mode == "transport-fragments" {
                assert_eq!(connected["agentInfo"]["name"], "代理🌸");
            }

            let client = host
                .get(mode, &launch, &std::env::temp_dir())
                .await
                .unwrap();
            let session = client
                .request(
                    "session/new",
                    json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
                )
                .await
                .unwrap();
            client
                .set_native_id(session["sessionId"].as_str().unwrap())
                .unwrap();
            let mut events = client.begin_turn();
            let response = prompt(&client, "hello").await.unwrap();
            assert_eq!(response["stopReason"], "end_turn");
            assert!(matches!(events.try_recv(), Ok(Event::Update(_))));
            host.stop(mode).await;
        }
    }

    #[tokio::test]
    async fn a_failure_keeps_a_bounded_tail_of_stdout_that_was_not_json() {
        let host = Arc::new(Host::default());
        let mut launch = definition("transport-noise");
        launch.name = format!("Noisy peer {}", uuid::Uuid::new_v4());
        let client = host
            .get("noisy", &launch, &std::env::temp_dir())
            .await
            .unwrap();
        let session = client
            .request(
                "session/new",
                json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
            )
            .await
            .unwrap();
        client
            .set_native_id(session["sessionId"].as_str().unwrap())
            .unwrap();
        client.begin_turn();
        prompt(&client, "hello").await.unwrap();
        prompt(&client, "crash").await.unwrap_err();

        let log = diagnostics(&launch.name);
        assert_eq!(log["stderr"], "");
        let discarded = log["discardedStdout"].as_str().unwrap();
        assert!(discarded.len() <= 8192);
        assert!(!discarded.contains("discarded-prefix"));
        assert!(
            discarded.ends_with(
                "x\n[dotenv@17.2.0] injecting env (0)\nnpm notice banner\n代理 final-stdout-banner"
            ),
            "{discarded}"
        );
        host.shutdown().await;
    }

    #[tokio::test]
    async fn slow_initialization_can_finish_after_the_previous_deadline() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mode = format!("gated-init:{}", listener.local_addr().unwrap().port());
        let launch = definition(&mode);
        let connecting = tokio::spawn(async move { super::super::test_connection(&launch).await });
        let (mut gate, _) =
            tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept())
                .await
                .unwrap()
                .unwrap();

        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(16)).await;
        assert!(!connecting.is_finished());
        tokio::time::resume();
        gate.write_all(b"\n").await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), connecting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result["status"], "connected");
    }

    #[tokio::test]
    async fn initialization_timeout_stops_the_agent_and_reports_its_diagnostics() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mode = format!("gated-init:{}", listener.local_addr().unwrap().port());
        let mut launch = definition(&mode);
        launch.name = format!("Timeout peer {}", uuid::Uuid::new_v4());
        let name = launch.name.clone();
        let connecting = tokio::spawn(async move { super::super::test_connection(&launch).await });
        let (gate, _) = tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept())
            .await
            .unwrap()
            .unwrap();

        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(61)).await;
        tokio::time::resume();
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), connecting)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(error.contains(&name), "{error}");
        assert!(error.contains("60"), "{error}");
        assert!(error.contains(
            &crate::local::chat::harness_log_path("acp")
                .display()
                .to_string()
        ));
        assert_startup_socket_closed(gate).await;
        assert!(diagnostics(&name)["error"].as_str().is_some());
    }

    #[tokio::test]
    async fn cancelling_initialization_stops_its_process() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mode = format!("gated-init:{}", listener.local_addr().unwrap().port());
        let launch = definition(&mode);
        let connecting = tokio::spawn(async move { super::super::test_connection(&launch).await });
        let (gate, _) = tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept())
            .await
            .unwrap()
            .unwrap();

        connecting.abort();
        assert!(connecting.await.unwrap_err().is_cancelled());
        assert_startup_socket_closed(gate).await;
    }

    #[tokio::test(start_paused = true)]
    async fn slow_cleanup_keeps_the_cause_and_waits_past_five_seconds() {
        let (sender, receiver) = tokio::sync::watch::channel(Some(
            "Peer did not initialize within 60 seconds.".to_owned(),
        ));
        let waiting = tokio::spawn(closed_reason(receiver));
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(6)).await;
        assert!(!waiting.is_finished());
        drop(sender);
        let message = waiting.await.unwrap().unwrap();
        assert!(message.contains("Peer"));
        assert!(message.contains("60 seconds"));
        assert!(!message.contains("may still be stopping"));
    }

    #[tokio::test(start_paused = true)]
    async fn cleanup_timeout_keeps_the_cause_and_reports_that_stopping_is_incomplete() {
        let (_sender, receiver) = tokio::sync::watch::channel(Some(
            "Peer did not initialize within 60 seconds.".to_owned(),
        ));
        let message = closed_reason(receiver).await.unwrap();
        assert!(message.contains("Peer"));
        assert!(message.contains("60 seconds"));
        assert!(message.contains("15 seconds"));
        assert!(message.contains("may still be stopping"));
    }

    #[tokio::test]
    async fn failures_keep_a_bounded_stderr_tail_and_observed_exit_status() {
        let mut launch = definition("stderr-init");
        launch.name = format!("Startup peer {}", uuid::Uuid::new_v4());
        let error = super::super::test_connection(&launch)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&launch.name), "{error}");
        assert!(error.contains(
            &crate::local::chat::harness_log_path("acp")
                .display()
                .to_string()
        ));
        let log = diagnostics(&launch.name);
        let stderr = log["stderr"].as_str().unwrap();
        assert!(stderr.len() <= 8192);
        assert!(!stderr.contains("discarded-prefix"));
        assert!(stderr.ends_with("代理 final-startup-cause"));
        assert!(error.contains("代理 final-startup-cause"), "{error}");
        assert!(!error.contains("xxxx"), "{error}");
        assert!(log["exit"].is_string());
        assert!(log["stderrReadError"].is_null());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::metadata(crate::local::chat::harness_log_path("acp"))
                .unwrap()
                .permissions();
            assert_eq!(permissions.mode() & 0o077, 0);
        }
    }

    #[tokio::test]
    async fn a_long_stderr_excerpt_is_bounded_without_splitting_unicode() {
        let error = super::super::test_connection(&definition("stderr-init:long"))
            .await
            .unwrap_err()
            .to_string();
        let excerpt = error
            .split_once("Agent output: ")
            .unwrap()
            .1
            .split_once(". See ")
            .unwrap()
            .0;
        assert!(excerpt.len() <= 320, "{error}");
        assert!(excerpt.ends_with("final-startup-cause"), "{error}");
        assert!(!excerpt.contains('�'), "{error}");
        assert!(excerpt.contains("代理"), "{error}");
    }

    #[tokio::test]
    async fn an_unwritable_log_reports_the_failure_without_claiming_it_was_saved() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let path = crate::local::chat::harness_log_path("acp");
        std::fs::create_dir_all(&path).unwrap();

        let launch = definition("stderr-init");
        let error = super::super::test_connection(&launch)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(&launch.name), "{error}");
        assert!(error.contains("Could not save"), "{error}");
        assert!(error.contains(&path.display().to_string()), "{error}");
        assert!(!error.contains("See "), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_spared_detached_command_can_keep_writing_to_stderr() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        for (mode, spare) in [("detached-stderr", true), ("orphan-stderr", false)] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mode = format!("{mode}:{}", listener.local_addr().unwrap().port());
            let host = Arc::new(Host::default());
            let client = host
                .get("detached-stderr", &definition(&mode), &std::env::temp_dir())
                .await
                .unwrap();
            let session = client
                .request(
                    "session/new",
                    json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
                )
                .await
                .unwrap();
            client
                .set_native_id(session["sessionId"].as_str().unwrap())
                .unwrap();
            client.begin_turn();
            prompt(&client, "detached-stderr").await.unwrap();
            let (mut gate, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap()
                    .unwrap();
            let mut ready = [0; 5];
            gate.read_exact(&mut ready).await.unwrap();
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                client.end(Stop::Shutdown(spare)),
            )
            .await
            .expect("shutdown waited for an orphan's stderr pipe");
            gate.write_all(b"\n").await.unwrap();
            let mut output = Vec::new();
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                gate.read_to_end(&mut output),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(output, b"writable");
            host.shutdown().await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_turn_that_loses_its_agent_keeps_the_failure_details() {
        let host = Arc::new(Host::default());
        for mode in ["stderr-turn", "invalid-utf8-turn"]
            .into_iter()
            .cycle()
            .take(8)
        {
            let mut launch = definition(mode);
            launch.name = format!("Turn peer {}", uuid::Uuid::new_v4());
            let client = host
                .get(mode, &launch, &std::env::temp_dir())
                .await
                .unwrap();
            let session = client
                .request(
                    "session/new",
                    json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
                )
                .await
                .unwrap();
            client
                .set_native_id(session["sessionId"].as_str().unwrap())
                .unwrap();
            client.begin_turn();
            let rejected = prompt(&client, "error").await.unwrap_err();
            assert_eq!(
                rejected
                    .downcast_ref::<agent_client_protocol::Error>()
                    .unwrap()
                    .code,
                agent_client_protocol::ErrorCode::InternalError
            );
            assert_eq!(
                prompt(&client, "hello").await.unwrap()["stopReason"],
                "end_turn"
            );
            let error = prompt(&client, "crash").await.unwrap_err().to_string();
            assert!(error.contains(&launch.name), "{error}");
            assert!(error.contains(
                &crate::local::chat::harness_log_path("acp")
                    .display()
                    .to_string()
            ));
            let log = diagnostics(&launch.name);
            assert_eq!(log["stderr"], "final-turn-cause");
            if mode == "invalid-utf8-turn" {
                assert_eq!(log["discardedStdout"], "\u{fffd}");
            }
            assert!(error.contains("final-turn-cause"), "{error}");
        }
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_permission_for_an_unknown_session_does_not_stall_the_connection() {
        let host = Arc::new(Host::default());
        let client = new_client(&host, "wrong-permission-session").await;
        client.set_native_id("another-session").unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(5), client.request(
            "session/prompt", json!({"sessionId":"another-session", "prompt":[{"type":"text","text":"ACP_TEST:permission"}]}),
        )).await.unwrap();
        assert!(response.is_err());
        host.shutdown().await;
    }

    #[tokio::test]
    async fn loading_history_does_not_replay_it_as_a_new_reply() {
        for adopt in [false, true] {
            let mut host = Arc::new(Host::default());
            let mut client = new_client(&host, "load-history").await;
            if adopt {
                client.wire.persist().unwrap();
                client.wire.keeper.detach();
                let adoption = crate::local::harness::Adoption {
                    keep_id: client.wire.keeper.id().to_owned(),
                    session_id: "load-history".into(),
                    state: client.wire.owner.lock().unwrap()["state"].clone(),
                    resume_at: None,
                    commands: Vec::new(),
                };
                host = Arc::new(Host::default());
                host.adopt(adoption).await.unwrap();
                client = host.connected("load-history").await.unwrap();
            }
            client
                .restore(
                    &client.native_id().unwrap(),
                    &std::env::temp_dir(),
                    Vec::new(),
                )
                .await
                .unwrap();
            let mut events = client.begin_turn();
            assert!(events.try_recv().is_err());
            tokio::time::timeout(std::time::Duration::from_secs(5), prompt(&client, "hello"))
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(events.try_recv(), Ok(Event::Update(_))));
            host.shutdown().await;
        }
    }

    #[tokio::test]
    async fn checkpoints_a_permission_answer_without_more_agent_output() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

        let mut command = crate::process::command(super::super::fake_agent_interpreter("python3"));
        command.args(["-u", "-c", "import sys;sys.stdin.read()"]);
        let pending = json!({"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{},"_orxOffset":10});
        let owner = crate::keep::Owner::Agent {
            harness: "acp:checkpoint-test".into(),
            session_id: "checkpoint-test".into(),
            state: json!({"id_base":0,"rpc":super::super::wire::State {
                inbound: HashMap::from([("7".into(), pending)]),
                ..Default::default()
            }}),
        }
        .to_value();
        let kept = crate::keep::spawn(crate::keep::Keep {
            command: &command,
            io: crate::keep::Io::Pipes { stderr: None },
            retain: crate::keep::LOSSLESS,
            owner: owner.clone(),
        })
        .await
        .unwrap();
        let io = Arc::new(SessionIo::default());
        let (sender, mut events) = mpsc::unbounded_channel();
        *io.events.lock().unwrap() = Some(sender);
        let (wire, transport) = super::super::wire::connect(kept, owner, io, None).unwrap();
        wire.durable
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (reader, mut writer) = tokio::io::split(transport);
        let request = tokio::io::BufReader::new(reader)
            .lines()
            .next_line()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&request).unwrap()["id"], 7);
        wire.processed();
        assert!(matches!(events.recv().await, Some(Event::Checkpoint)));
        wire.advance();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{}}\n")
            .await
            .unwrap();
        let checkpoint = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .unwrap();
        assert!(matches!(checkpoint, Some(Event::Checkpoint)));
        assert!(wire.state.lock().unwrap().inbound.is_empty());
        wire.keeper.stop(std::time::Duration::from_secs(2)).await;
    }

    #[tokio::test]
    async fn adopts_an_idle_agent_without_initializing_it_again() {
        let host = Arc::new(Host::default());
        let original = new_client(&host, "adopt-idle").await;
        let keeper = original.wire.keeper.clone();
        keeper.detach();
        original.wire.persist().unwrap();
        let state = original.wire.owner.lock().unwrap()["state"].clone();
        let successor = Arc::new(Host::default());
        let mut incomplete = state.clone();
        incomplete.as_object_mut().unwrap().remove("initialized");
        assert!(successor
            .adopt(crate::local::harness::Adoption {
                keep_id: keeper.id().to_owned(),
                session_id: "adopt-idle".into(),
                state: incomplete,
                resume_at: None,
                commands: Vec::new(),
            })
            .await
            .is_err());
        successor
            .adopt(crate::local::harness::Adoption {
                keep_id: keeper.id().to_owned(),
                session_id: "adopt-idle".into(),
                state,
                resume_at: None,
                commands: Vec::new(),
            })
            .await
            .unwrap();
        let client = successor.connected("adopt-idle").await.unwrap();
        assert_eq!(client.wire.keeper.pid(), keeper.pid());
        assert_eq!(client.native_id(), original.native_id());
        turn(&client, "hello").await;
        successor.shutdown().await;
    }

    #[tokio::test]
    async fn adopts_a_permission_and_finishes_the_original_prompt() {
        let host = Arc::new(Host::default());
        let original = new_client(&host, "adopt-permission").await;
        let mut events = original.begin_turn();
        let running = original.clone();
        let prompt = tokio::spawn(async move {
            running.request("session/prompt",
            json!({"sessionId": running.native_id().unwrap(), "prompt":[{"type":"text","text":"ACP_TEST:permission"}]})).await
        });
        let old_id = loop {
            if let Event::Permission { id, .. } =
                tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                    .await
                    .expect("permission delivery timed out")
                    .expect("permission connection closed")
            {
                break id;
            }
        };
        let offset = original
            .wire
            .offset
            .load(std::sync::atomic::Ordering::SeqCst);
        original.wire.persist().unwrap();
        original.wire.keeper.detach();
        let state = original.wire.owner.lock().unwrap()["state"].clone();
        let successor = Arc::new(Host::default());
        successor
            .adopt(crate::local::harness::Adoption {
                keep_id: original.wire.keeper.id().to_owned(),
                session_id: "adopt-permission".into(),
                state,
                resume_at: Some(offset),
                commands: Vec::new(),
            })
            .await
            .unwrap();
        let client = successor.connected("adopt-permission").await.unwrap();
        let mut events = client.begin_turn();
        client.wire.advance();
        let id = loop {
            if let Event::Permission { id, .. } =
                tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                    .await
                    .expect("permission delivery timed out")
                    .expect("permission connection closed")
            {
                break id;
            }
        };
        assert_eq!(id, old_id);
        client.answer(&id, "opaque-once").unwrap();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.wire.resumed_response(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result["stopReason"], "end_turn");
        prompt.abort();
        successor.shutdown().await;
    }

    #[tokio::test]
    async fn reaper_keeps_an_agent_only_while_its_terminal_runs() {
        use crate::local::agent_lifecycle::IdlePolicy;
        let host = Arc::new(Host::default());
        let kept = |id: &str| {
            host.clients
                .lock()
                .unwrap()
                .get(id)
                .is_some_and(|slot| slot.try_lock().is_ok_and(|saved| saved.is_some()))
        };
        let later = Instant::now() + std::time::Duration::from_secs(60 * 60);

        let client = new_client(&host, "terminal").await;
        turn(&client, "live-terminal").await;
        for policy in [IdlePolicy::DEFAULT, IdlePolicy::LOW_MEMORY] {
            host.reap_idle_at(later, policy).await;
            assert!(kept("terminal"), "reaped while its terminal ran");
        }
        turn(&client, "kill-live-terminal").await;
        host.reap_idle_at(later, IdlePolicy::DEFAULT).await;
        assert!(!kept("terminal"), "kept after its terminal exited");
        assert!(client.closed.borrow().is_some());

        let client = new_client(&host, "reported").await;
        turn(&client, "background").await;
        let cancel = async {
            tokio::task::yield_now().await;
            client.cancel();
        };
        tokio::join!(turn(&client, "hang"), cancel);
        assert!(
            client.closed.borrow().is_none(),
            "the cancel ended the agent"
        );
        host.reap_idle_at(later, IdlePolicy::DEFAULT).await;
        assert!(
            !kept("reported"),
            "a call left running after a cancel kept the agent"
        );
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaper_keeps_an_agent_while_a_command_it_detached_runs() {
        use crate::local::agent_lifecycle::IdlePolicy;
        let host = Arc::new(Host::default());
        let kept = || {
            host.clients
                .lock()
                .unwrap()
                .get("detached")
                .is_some_and(|slot| slot.try_lock().is_ok_and(|saved| saved.is_some()))
        };
        let later = Instant::now() + std::time::Duration::from_secs(60 * 60);
        let client = new_client(&host, "detached").await;
        // A helper in the agent's own session, like agy's MCP servers, keeps nothing.
        turn(&client, "attached").await;
        turn(&client, "detached").await;
        for policy in [IdlePolicy::DEFAULT, IdlePolicy::LOW_MEMORY] {
            host.reap_idle_at(later, policy).await;
            assert!(kept(), "reaped while its detached command ran");
        }

        turn(&client, "end-detached").await;
        host.reap_idle_at(later, IdlePolicy::DEFAULT).await;
        assert!(!kept(), "kept after its detached command exited");
        assert!(client.closed.borrow().is_some());
        host.shutdown().await;
    }

    #[tokio::test]
    async fn alphaxiv_server_reaches_new_loaded_and_resumed_sessions() {
        use agent_client_protocol::schema::v1::NewSessionRequest;
        for mode in ["alphaxiv-load", "alphaxiv-resume"] {
            let host = Arc::new(Host::default());
            let cwd = std::env::temp_dir();
            let client = host.get(mode, &definition(mode), &cwd).await.unwrap();
            let response = client
                .request(
                    "session/new",
                    NewSessionRequest::new(&cwd)
                        .mcp_servers(super::super::alphaxiv_mcp_servers().unwrap()),
                )
                .await
                .unwrap();
            let native_id = response["sessionId"].as_str().unwrap();
            client
                .restore(
                    native_id,
                    &cwd,
                    super::super::alphaxiv_mcp_servers().unwrap(),
                )
                .await
                .unwrap();
            host.shutdown().await;
        }
    }

    #[tokio::test]
    async fn restoring_a_session_the_agent_no_longer_lists_is_not_found() {
        let host = Arc::new(Host::default());
        let cwd = std::env::temp_dir();
        let client = host
            .get("unlisted", &definition("unlisted"), &cwd)
            .await
            .unwrap();
        let code = |error: crate::error::Error| {
            error
                .downcast_ref::<agent_client_protocol::Error>()
                .map(|error| error.code)
        };
        let gone = client.restore("gone", &cwd, vec![]).await.unwrap_err();
        assert_eq!(
            code(gone),
            Some(agent_client_protocol::ErrorCode::ResourceNotFound)
        );
        // Listed on the second page: the agent still has it, so its own error stands.
        let kept = client.restore("kept", &cwd, vec![]).await.unwrap_err();
        assert_eq!(
            code(kept),
            Some(agent_client_protocol::ErrorCode::InternalError)
        );
        host.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_idle_stop_spares_only_commands_in_their_own_sessions() {
        // An orphan stays a zombie under an init that never reaps it, as in some containers.
        fn running(pid: libc::pid_t) -> bool {
            let state = crate::process::command("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&state.stdout)
                .trim()
                .chars()
                .next()
                .is_some_and(|state| state != 'Z')
        }
        async fn exits(pid: libc::pid_t) -> bool {
            for _ in 0..200 {
                if !running(pid) {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            false
        }
        for idle in [true, false] {
            let pids = std::env::temp_dir().join(format!("orx-acp-pids-{}", uuid::Uuid::new_v4()));
            let peer = definition("normal");
            let mut launch = peer.clone();
            launch.executable = crate::local::acp::fake_agent_interpreter("sh");
            launch.arguments = [
                "-c".into(),
                r#"python3 -c 'import os, time; os.setsid(); time.sleep(60)' & echo $! > "$0"
sleep 60 & echo $! >> "$0"
exec "$@""#
                    .into(),
                pids.to_string_lossy().into_owned(),
                peer.executable,
            ]
            .into_iter()
            .chain(peer.arguments)
            .collect();
            let host = Arc::new(Host::default());
            let client = host
                .get("detached", &launch, &std::env::temp_dir())
                .await
                .unwrap();
            let started = std::fs::read_to_string(&pids).unwrap();
            std::fs::remove_file(&pids).unwrap();
            let started: Vec<libc::pid_t> =
                started.lines().map(|pid| pid.parse().unwrap()).collect();
            let [detached, attached] = started[..] else {
                panic!("expected two commands: {started:?}");
            };
            client.end(Stop::Shutdown(idle)).await;
            assert!(
                exits(attached).await,
                "a command in the agent's group outlived it"
            );
            if idle {
                assert!(running(detached), "an idle stop killed a detached command");
                unsafe { libc::kill(detached, libc::SIGKILL) };
            } else {
                assert!(exits(detached).await, "a detached command outlived a stop");
            }
        }
    }

    #[tokio::test]
    async fn a_changed_launch_restarts_the_resident_process() {
        let host = Arc::new(Host::default());
        let cwd = std::env::temp_dir();
        let launch = definition("normal");
        let first = host.get("relaunch", &launch, &cwd).await.unwrap();
        let again = host.get("relaunch", &launch, &cwd).await.unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        let mut changed = launch.clone();
        changed.arguments.push("--changed".into());
        let restarted = host.get("relaunch", &changed, &cwd).await.unwrap();
        assert!(!Arc::ptr_eq(&first, &restarted));
        assert!(first.closed.borrow().is_some());
        host.shutdown().await;
    }

    #[tokio::test]
    async fn resident_process_receives_owning_chat_connection() {
        let host = Arc::new(Host::default());
        host.set_up_port(4901);
        host.get(
            "environment-chat",
            &definition("environment"),
            &std::env::temp_dir(),
        )
        .await
        .unwrap();
        host.shutdown().await;
    }

    #[tokio::test]
    async fn stalled_session_setup_times_out_and_stops_the_process() {
        let host = Arc::new(Host::default());
        let client = host
            .get(
                "stalled",
                &definition("stalled-setup"),
                &std::env::temp_dir(),
            )
            .await
            .unwrap();
        let error = client
            .request_with_timeout(
                "session/new",
                json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
                std::time::Duration::from_secs(1),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("session/new timed out"));
        assert!(client.closed.borrow().is_some());
        host.stop("stalled").await;
    }

    async fn prompt(client: &Client, text: &str) -> Result<Value> {
        client
            .request(
                "session/prompt",
                json!({"sessionId": client.native_id(), "prompt": [{"type":"text", "text": text}]}),
            )
            .await
    }

    #[tokio::test]
    async fn peer_interleaves_permissions_without_blocking_notifications() {
        let host = Arc::new(Host::default());
        let client = new_client(&host, "chat").await;
        for selected in ["opaque-once", "opaque-deny"] {
            let mut events = client.begin_turn();
            let task_client = client.clone();
            let task = tokio::spawn(async move { prompt(&task_client, "permission").await });
            let id = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if let Some(Event::Permission { id, .. }) = events.recv().await {
                        break id;
                    }
                }
            })
            .await
            .unwrap();
            let waiting = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(waiting, Event::Update(value) if value["content"]["text"] == "while waiting")
            );
            assert!(host.answer("other-chat", &id, selected).await.is_err());
            assert!(host.answer("chat", &id, "not-offered").await.is_err());
            host.answer("chat", &id, selected).await.unwrap();
            assert!(host.answer("chat", &id, selected).await.is_err());
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            client.end_turn();
        }
        host.shutdown().await;
    }

    #[tokio::test]
    async fn automatic_approval_uses_only_offered_single_action_grants() {
        let host = Arc::new(Host::default());
        let client = new_client(&host, "auto-approval").await;
        for (text, automatic) in [
            ("permission", true),
            ("permission-always", false),
            ("permission-deny", false),
            ("permission-switch", false),
        ] {
            let mut events = client.begin_turn();
            let task_client = client.clone();
            let task = tokio::spawn(async move { prompt(&task_client, text).await });
            let id = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if let Some(Event::Permission { id, .. }) = events.recv().await {
                        break id;
                    }
                }
            })
            .await
            .unwrap();
            let approved = client.approve_pending().unwrap();
            if automatic {
                assert_eq!(approved, vec![(id, "opaque-once".into())]);
            } else {
                assert!(approved.is_empty());
                assert!(client.awaiting_permission());
                client.answer(&id, "opaque-deny").unwrap();
            }
            assert!(client.approve_pending().unwrap().is_empty());
            tokio::time::timeout(std::time::Duration::from_secs(5), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            client.end_turn();
        }
        host.shutdown().await;
    }

    #[tokio::test]
    async fn peer_files_terminals_and_conversation_are_isolated() {
        let host = Arc::new(Host::default());
        let first = new_client(&host, "first").await;
        let second = new_client(&host, "second").await;
        let mut first_events = first.begin_turn();
        let mut second_events = second.begin_turn();
        let path = std::env::temp_dir().join(format!("acp-peer-file-{}", uuid::Uuid::new_v4()));
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt(&first, &format!("files:{}", path.display())),
        )
        .await
        .unwrap();
        result.unwrap();
        assert_eq!(
            tokio::fs::read_to_string(&path).await.unwrap(),
            "one\ntwo\nthree\n"
        );
        assert!(second_events.try_recv().is_err());
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            prompt(&first, "terminal"),
        )
        .await
        .unwrap()
        .unwrap();
        let mut terminal_output = None;
        while let Ok(event) = first_events.try_recv() {
            if let Event::TerminalOutput { output, .. } = event {
                terminal_output = Some(output);
            }
        }
        let output =
            terminal_output.expect("delegated terminal output reaches the transcript stream");
        assert!(!output.is_empty() && output.len() <= 100);
        assert!(second_events.try_recv().is_err());
        tokio::time::timeout(std::time::Duration::from_secs(5), prompt(&second, "hello"))
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(second_events.recv().await, Some(Event::Update(value)) if value["content"]["text"] == "turn 1")
        );
        first.end_turn();
        second.end_turn();
        host.stop("first").await;
        assert!(host.connected("first").await.is_err());
        assert!(!host.clients.lock().unwrap().contains_key("first"));
        assert!(host.connected("second").await.is_ok());
        host.shutdown().await;
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_resolves_native_permission_as_cancelled() {
        let host = Arc::new(Host::default());
        let client = new_client(&host, "chat").await;
        let mut events = client.begin_turn();
        let task_client = client.clone();
        let task = tokio::spawn(async move { prompt(&task_client, "permission").await });
        let id = loop {
            if let Some(Event::Permission { id, .. }) = events.recv().await {
                break id;
            }
        };
        client.end_turn();
        assert!(client.approve_pending().unwrap().is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(client.answer(&id, "opaque-once").is_err());
        host.shutdown().await;
    }

    /// What the agent hears about a prompt whose request the server dropped mid-turn.
    async fn cancels_of_a_dropped_prompt(hand_off: bool) -> (Vec<Value>, Value) {
        let record = crate::config::test_root().join(format!("acp-{}.jsonl", uuid::Uuid::new_v4()));
        let mut definition = definition("normal");
        definition
            .arguments
            .push(record.to_string_lossy().into_owned());
        let host = Arc::new(Host::default());
        let client = host
            .get("chat", &definition, &std::env::temp_dir())
            .await
            .unwrap();
        let session = client
            .request(
                "session/new",
                json!({"cwd": std::env::temp_dir(), "mcpServers": []}),
            )
            .await
            .unwrap();
        client
            .set_native_id(session["sessionId"].as_str().unwrap())
            .unwrap();
        drop(client.begin_turn());
        if hand_off {
            crate::keep::hand_off();
        }
        tokio::select! {
            biased;
            _ = prompt(&client, "hang") => panic!("the hanging prompt was answered"),
            listed = client.request("session/list", json!({})) => { listed.unwrap(); }
        }
        // Messages reach the agent in order, so any cancel precedes this request.
        client.request("session/list", json!({})).await.unwrap();

        let recorded: Vec<Value> = std::fs::read_to_string(&record)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let prompt_id = recorded
            .iter()
            .find(|line| line["method"] == "session/prompt")
            .unwrap()["id"]
            .clone();
        let cancels = recorded
            .into_iter()
            .filter(|line| line["method"] == "$/cancel_request")
            .collect();
        client.wire.keeper.signal(crate::keep::Signal::Kill);
        (cancels, prompt_id)
    }

    #[tokio::test]
    async fn a_dropped_request_is_cancelled_under_the_id_the_agent_saw() {
        let (cancels, prompt_id) = cancels_of_a_dropped_prompt(false).await;
        assert_eq!(cancels.len(), 1, "{cancels:?}");
        assert_eq!(cancels[0]["params"]["requestId"], prompt_id);
    }

    #[tokio::test]
    async fn a_hand_off_leaves_the_prompt_running_for_the_successor() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let (cancels, _) = cancels_of_a_dropped_prompt(true).await;
        assert!(cancels.is_empty(), "{cancels:?}");
    }

    #[tokio::test]
    async fn a_permission_during_a_hand_off_is_not_cancelled_for_the_user() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let host = Arc::new(Host::default());
        let client = new_client(&host, "chat").await;
        drop(client.begin_turn());
        crate::keep::hand_off();
        let task_client = client.clone();
        let task = tokio::spawn(async move { prompt(&task_client, "permission").await });

        let answered = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;

        assert!(
            answered.is_err(),
            "the agent's permission request was answered"
        );
        client.wire.keeper.signal(crate::keep::Signal::Kill);
    }

    #[tokio::test]
    async fn one_shot_configuration_updates_reach_the_turn_without_a_stored_chat() {
        let host = Arc::new(Host::default());
        let client = new_client(&host, "acp-oneshot-configuration").await;
        client.mark_one_shot();
        let mut events = client.begin_turn();
        client
            .request(
                "session/set_config_option",
                json!({"sessionId": client.native_id(), "configId": "opaque-model", "value": "second"}),
            )
            .await
            .unwrap();
        // No chat session has this ID, so a store write would fail and drop the update.
        assert!(
            matches!(events.try_recv(), Ok(Event::Update(update)) if update["sessionUpdate"] == "current_mode_update")
        );
        client.end_turn();
        host.shutdown().await;
    }

    #[tokio::test]
    async fn a_configuration_update_the_store_refuses_holds_up_no_later_message() {
        let host = Arc::new(Host::default());
        // Not one-shot and never stored, like a built-in agent's chat.
        let client = new_client(&host, "acp-unstored-configuration").await;
        drop(client.begin_turn());
        for value in ["second", "third"] {
            let answered = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client.request(
                    "session/set_config_option",
                    json!({"sessionId": client.native_id(), "configId": "opaque-model", "value": value}),
                ),
            )
            .await;
            assert!(matches!(answered, Ok(Ok(_))), "{answered:?}");
        }
        client.end_turn();
        host.shutdown().await;
    }

    #[tokio::test]
    async fn chat_configuration_updates_from_the_agent_are_stored() {
        let store = crate::store::Store::open().unwrap();
        let id = format!("chat_{}", uuid::Uuid::new_v4());
        store
            .create_chat_session(&crate::store::StoredChatSession {
                profile_id: None,
                id: id.clone(),
                project_id: "proj_1".into(),
                harness: super::super::presets().remove(0).launch.id,
                native_session_id: None,
                native_fork_at: None,
                title: None,
                title_source: None,
                model: None,
                service_tier: None,
                permission_mode: None,
                plan_mode: false,
                plan_reset_pending: false,
                reasoning_level: None,
                archived: false,
                context_usage_json: None,
                bootstrap_context: None,
                goal: None,
                autonomy: None,
                active_leaf_id: None,
                side_parent_session_id: None,
                side_parent_leaf_id: None,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        store
            .set_acp_configuration(&id, &json!({"modes": {"currentModeId": "initial"}}))
            .unwrap();
        let host = Arc::new(Host::default());
        let client = new_client(&host, &id).await;
        let mut events = client.begin_turn();
        client
            .request(
                "session/set_config_option",
                json!({"sessionId": client.native_id(), "configId": "opaque-model", "value": "second"}),
            )
            .await
            .unwrap();
        // The update reaches the turn only after the store write.
        assert!(
            matches!(events.try_recv(), Ok(Event::Update(update)) if update["sessionUpdate"] == "current_mode_update")
        );
        let configuration = store.acp_session_state(&id).unwrap().unwrap().configuration;
        assert_eq!(configuration["modes"]["currentModeId"], "agent-changed");
        client.end_turn();
        host.shutdown().await;
    }

    #[tokio::test]
    async fn failed_initialization_and_process_death_are_explicit() {
        let host = Arc::new(Host::default());
        for mode in ["incompatible", "auth-init", "crash-init", "shapeless-init"] {
            let error = host
                .get(mode, &definition(mode), &std::env::temp_dir())
                .await
                .err()
                .expect("initialization must fail")
                .to_string();
            if mode == "incompatible" {
                assert!(error.contains("stable v1"), "{error}");
                assert!(error.contains("Incompatible ACP protocol"), "{error}");
            }
            if mode == "shapeless-init" {
                assert!(error.contains("protocolVersion"), "{error}");
            }
        }
        let client = new_client(&host, "crash").await;
        client.begin_turn();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), prompt(&client, "crash"))
                .await
                .unwrap()
                .is_err()
        );
        host.shutdown().await;
    }
}
