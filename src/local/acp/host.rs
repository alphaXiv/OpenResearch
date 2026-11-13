use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_client_protocol::schema::{v1::RequestPermissionRequest, ProtocolVersion};
use agent_client_protocol::{Agent, ByteStreams, ConnectionTo, Responder, UntypedMessage};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use super::{Definition, OwnedProcess};
use crate::error::{anyhow, bail, Result};

pub enum Event {
    Update(Value),
    TerminalOutput { id: String, output: String },
    Permission { id: String, request: Value },
}

struct Permission {
    responder: Responder,
    options: Vec<String>,
}

#[derive(Default)]
struct SessionIo {
    native_id: Mutex<Option<String>>,
    events: Mutex<Option<mpsc::UnboundedSender<Event>>>,
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

    fn emit(&self, event: Event) -> bool {
        self.events
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|sender| sender.send(event).is_ok())
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
    stop: Mutex<Option<oneshot::Sender<bool>>>,
    closed: tokio::sync::watch::Receiver<bool>,
    idle_since: Mutex<Option<Instant>>,
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
        self.set_native_id(native_id);
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

    pub fn native_id(&self) -> Option<String> {
        self.io.native_id.lock().unwrap().clone()
    }

    pub fn set_native_id(&self, id: &str) {
        *self.io.native_id.lock().unwrap() = Some(id.to_owned());
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
        *self.io.events.lock().unwrap() = Some(sender);
        *self.idle_since.lock().unwrap() = None;
        receiver
    }

    pub fn end_turn(&self) {
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
        let request = self
            .connection
            .send_request(UntypedMessage::new(method, params)?)
            .block_task();
        if method == "session/prompt" {
            return Ok(request.await?);
        }
        match tokio::time::timeout(timeout, request).await {
            Ok(response) => Ok(response?),
            Err(_) => {
                self.shutdown().await;
                bail!(
                    "ACP {method} timed out. Check the agent's connection and send a new message."
                )
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
        self.end(false).await;
    }

    /// Stops the agent; `spare_detached` leaves the commands it started in their own sessions running.
    async fn end(&self, spare_detached: bool) {
        self.cancel();
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(spare_detached);
        }
        let mut closed = self.closed.clone();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            closed.wait_for(|closed| *closed),
        )
        .await;
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.io.cancel_permissions();
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
            !*client.closed.borrow()
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
            .filter(|client| !*client.closed.borrow())
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
                    saved
                        .as_ref()
                        .is_some_and(|client| !*client.closed.borrow())
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
                host.prune_slots();
                let policy = crate::local::agent_lifecycle::IdlePolicy::current();
                let slots: Vec<_> = host.clients.lock().unwrap().values().cloned().collect();
                let mut idle = Vec::new();
                for slot in slots {
                    if let Ok(saved) = slot.try_lock() {
                        if let Some(client) = saved.as_ref() {
                            if let Some(since) = *client.idle_since.lock().unwrap() {
                                idle.push((slot.clone(), since.elapsed()));
                            }
                        }
                    }
                }
                let over_limit =
                    policy.over_limit(&idle.iter().map(|(_, age)| *age).collect::<Vec<_>>());
                for (index, (slot, age)) in idle.into_iter().enumerate() {
                    if age < policy.idle_timeout && !over_limit.contains(&index) {
                        continue;
                    }
                    if let Ok(mut saved) = slot.try_lock() {
                        let idle = saved.as_ref().is_some_and(|client| {
                            client.idle_since.lock().unwrap().is_some()
                                && !client.awaiting_permission()
                        });
                        if idle {
                            if let Some(client) = saved.take() {
                                // A dev server the agent started stays the user's.
                                client.end(true).await;
                            }
                        }
                    }
                }
            }
        });
    }
}

/// How long [`spawn`] waits for an agent to answer `initialize`.
pub(super) const INITIALIZATION_BOUND: std::time::Duration = std::time::Duration::from_secs(15);

pub(super) async fn spawn(
    definition: &Definition,
    cwd: &Path,
    session_id: &str,
    events: Option<tokio::sync::broadcast::Sender<(&'static str, Value)>>,
    up_port: Option<u16>,
) -> Result<Arc<Client>> {
    definition.validate_launch()?;
    let executable = super::executable_path(&definition.executable).ok_or_else(|| {
        anyhow!("Executable not found. Install the agent or check its launch command.")
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
    command
        .args(&definition.arguments)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if up_port.is_some() {
        crate::local::chat::set_chat_session_env(
            &mut command,
            session_id,
            &definition.id,
            up_port,
        )?;
    }
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().map_err(|error| {
        anyhow!("Could not start ACP harness. Check its installation and executable path: {error}")
    })?;
    let mut process = OwnedProcess {
        id: child.id(),
        child,
    };
    let transport = ByteStreams::new(
        process
            .child
            .stdin
            .take()
            .expect("piped stdin")
            .compat_write(),
        process.child.stdout.take().expect("piped stdout").compat(),
    );
    let io = Arc::new(SessionIo::default());
    let mut delegated = super::delegated::Delegated::new(cwd);
    if let Some(port) = up_port {
        delegated.set_session(session_id, &definition.id, port);
    }
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
    let (stop_sender, stop_receiver) = oneshot::channel();
    let (closed_sender, closed_receiver) = tokio::sync::watch::channel(false);
    let task_io = io.clone();
    let initialized_tools = delegated.clone();
    tokio::spawn(async move {
        let run = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: UntypedMessage, _connection| {
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
                },
                agent_client_protocol::on_receive_notification!(),
            )
            .on_receive_request(
                async move |request: UntypedMessage, responder, connection| {
                    if !requests.owns(&request.params) {
                        return responder.respond_with_error(
                            agent_client_protocol::Error::invalid_params()
                                .data("Unknown ACP session"),
                        );
                    }
                    if request.method != "session/request_permission" {
                        if requests.one_shot.load(std::sync::atomic::Ordering::Relaxed) {
                            return responder.respond_with_error(
                                agent_client_protocol::Error::invalid_params()
                                    .data("Tools are disabled for one-shot requests"),
                            );
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
                        _ => return responder.respond_with_error(agent_client_protocol::Error::invalid_params().data("Invalid native permission options")),
                    };
                    let options = permission.options.iter().map(|option| option.option_id.to_string()).collect();
                    let id = uuid::Uuid::new_v4().to_string();
                    requests
                        .permissions
                        .lock()
                        .unwrap()
                        .insert(id.clone(), Permission { responder, options });
                    if !requests.emit(Event::Permission {
                        id: id.clone(),
                        request: request.params,
                    }) {
                        if let Some(pending) = requests.permissions.lock().unwrap().remove(&id) {
                            let _ = pending
                                .responder
                                .respond(json!({"outcome": {"outcome": "cancelled"}}));
                        }
                    }
                    Ok(())
                },
                agent_client_protocol::on_receive_request!(),
            )
            .connect_with(transport, |connection: ConnectionTo<Agent>| async move {
                let initialized = connection
                    .send_request(super::initialization())
                    .block_task()
                    .await?;
                if initialized.protocol_version != ProtocolVersion::V1 {
                    return Err(agent_client_protocol::Error::invalid_params()
                        .data("Incompatible ACP protocol: stable v1 is required"));
                }
                if let Some(agent) = &initialized.agent_info {
                    initialized_tools.set_agent(&agent.name);
                }
                if let Some(sender) = initialized_sender.lock().unwrap().take() {
                    let _ = sender.send(Ok((
                        connection.clone(),
                        serde_json::to_value(initialized).expect("ACP response is serializable"),
                    )));
                }
                connection.incoming_closed().await;
                Ok(())
            });
        tokio::pin!(run);
        let spare_detached = tokio::select! {
            stopped = stop_receiver => {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut run).await;
                stopped.unwrap_or(false)
            }
            result = &mut run => {
                if let Some(sender) = ready_sender.lock().unwrap().take() {
                    let message = result.err().map(|error| error.to_string()).unwrap_or_else(|| "Agent exited before initialization".into());
                    let _ = sender.send(Err(message));
                }
                false
            }
        };
        task_io.cancel_permissions();
        task_io.events.lock().unwrap().take();
        delegated.shutdown().await;
        // Windows has no sessions to tell detached commands apart.
        if spare_detached && cfg!(unix) {
            crate::local::chat::kill_shell_group(process.id.take());
        } else {
            crate::local::chat::kill_agent_tree(process.id.take()).await;
        }
        let _ = process.child.kill().await;
        let _ = process.child.wait().await;
        let _ = closed_sender.send(true);
    });
    let initialized = tokio::time::timeout(INITIALIZATION_BOUND, ready_receiver).await;
    let message = match initialized {
        Ok(Ok(Ok((connection, initialized)))) => {
            return Ok(Arc::new(Client {
                alphaxiv_signed_in: crate::alphaxiv::signed_in(),
                launch: definition.clone(),
                configuration_lock: tokio::sync::Mutex::new(()),
                connection,
                initialized,
                io,
                stop: Mutex::new(Some(stop_sender)),
                closed: closed_receiver,
                idle_since: Mutex::new(Some(Instant::now())),
            }))
        }
        Ok(Ok(Err(message))) => format!("ACP initialization failed: {message}"),
        _ => "ACP initialization failed or timed out. Check the executable, arguments, and supported protocol version.".to_owned(),
    };
    drop(stop_sender);
    // Windows keeps `cwd` in use until the child exits, and callers remove it next.
    let mut closed = closed_receiver;
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        closed.wait_for(|closed| *closed),
    )
    .await;
    bail!("{message}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        client.set_native_id(response["sessionId"].as_str().unwrap());
        client
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
            client.end(idle).await;
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
        assert!(*first.closed.borrow());
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
        assert!(*client.closed.borrow());
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
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(client.answer(&id, "opaque-once").is_err());
        host.shutdown().await;
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
        for mode in ["incompatible", "auth-init", "crash-init"] {
            assert!(host
                .get(mode, &definition(mode), &std::env::temp_dir())
                .await
                .is_err());
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
