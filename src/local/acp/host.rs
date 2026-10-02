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
    configuration_lock: tokio::sync::Mutex<()>,
    pub connection: ConnectionTo<Agent>,
    pub initialized: Value,
    io: Arc<SessionIo>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
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

    pub async fn restore(&self, native_id: &str, cwd: &Path) -> Result<Value> {
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
            serde_json::to_value(ResumeSessionRequest::new(native_id.to_owned(), cwd))?
        } else {
            serde_json::to_value(LoadSessionRequest::new(native_id.to_owned(), cwd))?
        };
        self.request(method, params).await
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
        Ok(self
            .connection
            .send_request(UntypedMessage::new(method, params)?)
            .block_task()
            .await?)
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

    pub async fn shutdown(&self) {
        self.io.cancel_permissions();
        if let Some(native_id) = self.native_id() {
            if let Ok(notification) =
                UntypedMessage::new("session/cancel", json!({"sessionId": native_id}))
            {
                let _ = self.connection.send_notification(notification);
            }
        }
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
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

fn select_has_value(option: &Value, value: &str) -> bool {
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
}

impl Host {
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
        if let Some(client) = saved.as_ref().filter(|client| !*client.closed.borrow()) {
            *client.idle_since.lock().unwrap() = None;
            return Ok(client.clone());
        }
        let client = spawn(definition, cwd, session_id, self.events.clone()).await?;
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
                                client.shutdown().await;
                            }
                        }
                    }
                }
            }
        });
    }
}

async fn spawn(
    definition: &Definition,
    cwd: &Path,
    session_id: &str,
    events: Option<tokio::sync::broadcast::Sender<(&'static str, Value)>>,
) -> Result<Arc<Client>> {
    definition.validate()?;
    let mut command = tokio::process::Command::new(&definition.executable);
    command
        .args(&definition.arguments)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::local::chat::prepare_env(&mut command);
    command.env(crate::local::chat::CHAT_SESSION_ENV, session_id);
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
    let delegated = Arc::new(super::delegated::Delegated::new(cwd));
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
    tokio::spawn(async move {
        let run = agent_client_protocol::Client
            .builder()
            .on_receive_notification(
                async move |notification: UntypedMessage, _connection| {
                    if notification.method == "session/update"
                        && notifications.owns(&notification.params)
                    {
                        let update = &notification.params["update"];
                        if matches!(update["sessionUpdate"].as_str(), Some("config_option_update" | "current_mode_update")) {
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
                        let delegated = request_tools.clone();
                        connection.spawn(async move {
                            match delegated.request(&request.method, request.params).await {
                                Ok(response) => {
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
        tokio::select! {
            _ = stop_receiver => {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(500), &mut run).await;
            }
            result = &mut run => {
                if let Some(sender) = ready_sender.lock().unwrap().take() {
                    let message = result.err().map(|error| error.to_string()).unwrap_or_else(|| "Agent exited before initialization".into());
                    let _ = sender.send(Err(message));
                }
            }
        }
        task_io.cancel_permissions();
        task_io.events.lock().unwrap().take();
        delegated.shutdown().await;
        crate::local::chat::kill_shell_group(process.id.take());
        let _ = process.child.kill().await;
        let _ = process.child.wait().await;
        let _ = closed_sender.send(true);
    });
    let initialized =
        tokio::time::timeout(std::time::Duration::from_secs(15), ready_receiver).await;
    match initialized {
        Ok(Ok(Ok((connection, initialized)))) => Ok(Arc::new(Client {
            configuration_lock: tokio::sync::Mutex::new(()),
            connection,
            initialized,
            io,
            stop: Mutex::new(Some(stop_sender)),
            closed: closed_receiver,
            idle_since: Mutex::new(Some(Instant::now())),
        })),
        Ok(Ok(Err(message))) => {
            drop(stop_sender);
            bail!("ACP initialization failed: {message}")
        }
        _ => {
            drop(stop_sender);
            bail!("ACP initialization failed or timed out. Check the executable, arguments, and supported protocol version.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(mode: &str) -> Definition {
        Definition {
            id: format!("acp:{}", uuid::Uuid::new_v4()),
            name: "Peer".into(),
            executable: if cfg!(windows) { "python" } else { "python3" }.into(),
            arguments: vec![
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/local/harness/fixtures/acp-peer.py"
                )
                .into(),
                mode.into(),
            ],
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
        assert!(first_events.try_recv().is_ok());
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
