use serde::{Deserialize, Serialize};

use crate::error::{bail, Result};

mod delegated;
pub mod host;

struct OwnedProcess {
    child: tokio::process::Child,
    id: Option<u32>,
}

impl Drop for OwnedProcess {
    fn drop(&mut self) {
        crate::local::chat::kill_shell_group(self.id);
    }
}

fn initialization() -> agent_client_protocol::schema::v1::InitializeRequest {
    use agent_client_protocol::schema::{
        v1::{ClientCapabilities, FileSystemCapabilities, InitializeRequest},
        ProtocolVersion,
    };
    InitializeRequest::new(ProtocolVersion::V1).client_capabilities(
        ClientCapabilities::new()
            .fs(FileSystemCapabilities::new()
                .read_text_file(true)
                .write_text_file(true))
            .terminal(true),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    /// Fields from a newer orx, kept across this version's saves.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl Definition {
    pub fn validate(&self) -> Result<()> {
        let Some(id) = self.id.strip_prefix("acp:") else {
            bail!("ACP agent IDs must start with acp:");
        };
        uuid::Uuid::parse_str(id)?;
        self.validate_launch()
    }

    /// The launch alone, which built-in harnesses running over ACP share with
    /// user definitions while keeping their own harness IDs.
    pub fn validate_launch(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.contains('\0') {
            bail!("Enter a name for the ACP agent");
        }
        if self.executable.trim().is_empty() || self.executable.contains('\0') {
            bail!("Enter an executable command or absolute path");
        }
        let path = std::path::Path::new(&self.executable);
        if !path.is_absolute() && path.components().count() != 1 {
            bail!("Use an executable on PATH or an absolute path");
        }
        if self
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
        {
            bail!("Arguments cannot contain null characters");
        }
        Ok(())
    }
}

pub fn definitions() -> Vec<Definition> {
    crate::telemetry::load_settings()
        .map(|settings| settings.acp_harnesses)
        .unwrap_or_default()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    #[serde(flatten)]
    pub launch: Definition,
    pub setup_url: &'static str,
    pub needs_connection_adapter: bool,
}

struct Builtin {
    /// Stored with every session that uses the agent, so it never changes.
    id: &'static str,
    /// The label telemetry sends in place of `id`; analytics rows group by it, so it never changes.
    telemetry: &'static str,
    name: &'static str,
    executable: &'static str,
    arguments: &'static [&'static str],
    setup_url: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "acp:dee4a440-fd64-4217-a4a7-1d4c148c0dd1",
        telemetry: "acp:deepseek-harness",
        name: "DeepSeek Harness",
        executable: "dsh",
        arguments: &["--profile", "acp"],
        setup_url: "https://github.com/deepseek-ai/deepseek-harness",
    },
    Builtin {
        id: "acp:a8b48911-e95c-42cb-bc35-bab3aa83e5e2",
        telemetry: "acp:kimi",
        name: "Kimi Code",
        executable: "kimi",
        arguments: &["acp"],
        setup_url: "https://www.kimi.com/code/docs/en/kimi-code-cli/",
    },
    Builtin {
        id: "acp:1dba9897-707b-4cd8-b595-c8c26115f563",
        telemetry: "acp:minimax",
        name: "MiniMax Code",
        executable: "mcode",
        arguments: &["acp"],
        setup_url: "https://github.com/MiniMax-AI/minimax-code",
    },
    Builtin {
        id: "acp:2f1639e0-3f0f-4594-bb5b-a4078c36d5a4",
        telemetry: "acp:zcode",
        name: "ZCode",
        executable: "zcode-acp-server",
        arguments: &[],
        setup_url: "https://github.com/william0wang/zcode-acp#install",
    },
    Builtin {
        id: "acp:467b1900-c3c3-47c1-8b87-09389e1c44b5",
        telemetry: "acp:pi",
        name: "Pi",
        executable: "pi-acp",
        arguments: &[],
        setup_url: "https://github.com/svkozak/pi-acp#install",
    },
    Builtin {
        id: "acp:3d392186-692e-4b90-93b4-bb398f40007a",
        telemetry: "acp:oh-my-pi",
        name: "oh-my-pi",
        executable: "omp",
        arguments: &["acp"],
        setup_url: "https://github.com/can1357/oh-my-pi#install",
    },
    Builtin {
        id: "acp:565e351e-62c7-488f-a58a-914d30cc5776",
        telemetry: "acp:devin",
        name: "Devin CLI",
        executable: "devin",
        arguments: &["acp"],
        setup_url: "https://docs.devin.ai/cli",
    },
    Builtin {
        id: "acp:ac7f713b-b520-40d2-9b56-75deca1574c7",
        telemetry: "acp:grok",
        name: "Grok Build",
        executable: "grok",
        arguments: &["agent", "stdio"],
        setup_url: "https://github.com/xai-org/grok-build",
    },
    Builtin {
        id: "acp:c33b22bc-6d2e-44c6-a5d6-117dd6722f7f",
        telemetry: "acp:fx",
        name: "fx",
        executable: "fx",
        arguments: &["acp"],
        setup_url: "https://fx.sh/docs/using-fx/acp",
    },
];

pub(crate) fn telemetry_id(id: &str) -> Option<&'static str> {
    BUILTINS
        .iter()
        .find(|builtin| id == builtin.id || id == builtin.telemetry)
        .map(|builtin| builtin.telemetry)
}

pub fn presets() -> Vec<Preset> {
    BUILTINS
        .iter()
        // fx ships for macOS and Linux only, so Windows has no installer to offer.
        .filter(|builtin| !(cfg!(windows) && builtin.executable == "fx"))
        .map(|builtin| Preset {
            needs_connection_adapter: matches!(builtin.executable, "pi-acp" | "zcode-acp-server"),
            launch: Definition {
                id: builtin.id.into(),
                name: builtin.name.into(),
                executable: builtin.executable.into(),
                arguments: builtin
                    .arguments
                    .iter()
                    .map(|argument| (*argument).into())
                    .collect(),
                unknown: Default::default(),
            },
            setup_url: builtin.setup_url,
        })
        .collect()
}

pub fn registered_definitions() -> Vec<Definition> {
    register_definitions(definitions())
}

fn register_definitions(saved: Vec<Definition>) -> Vec<Definition> {
    let presets: Vec<Definition> = presets()
        .into_iter()
        .map(|preset| preset.launch)
        .filter(|launch| !saved.iter().any(|saved| same_launch(saved, launch)))
        .collect();
    saved.into_iter().chain(presets).collect()
}

pub fn same_launch(a: &Definition, b: &Definition) -> bool {
    let executable = |definition: &Definition| {
        executable_path(&definition.executable)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| definition.executable.clone())
    };
    a.arguments == b.arguments && executable(a) == executable(b)
}

pub fn executable_path(executable: &str) -> Option<std::path::PathBuf> {
    let own = user_executable_path(executable);
    if std::path::Path::new(executable).is_absolute() {
        return own;
    }
    crate::local::harness::managed::preferred(executable, own)
}

pub(crate) fn user_executable_path(executable: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(executable);
    if path.is_absolute() {
        #[cfg(test)]
        let path = &fake_agent_or_contained(path.to_path_buf());
        return path.is_file().then(|| path.to_path_buf());
    }
    let own = crate::config::home_dir().into_iter().flat_map(|home| {
        [
            ".grok/bin",
            ".devin/bin",
            ".minimax-code/bin",
            ".zcode/runtime/bin",
        ]
        .map(|directory| home.join(directory))
    });
    crate::local::shell_env::installs(executable, own)
        .into_iter()
        .next()
}

/// The interpreters a test may run a fake agent script with.
#[cfg(test)]
const FAKE_AGENT_INTERPRETERS: [&str; 3] = ["python3", "python", "sh"];

/// An absolute agent under test: one of [`fake_agent_interpreter`]'s, else a fake inside the test root.
#[cfg(test)]
fn fake_agent_or_contained(path: std::path::PathBuf) -> std::path::PathBuf {
    let interpreter = FAKE_AGENT_INTERPRETERS
        .iter()
        .any(|name| crate::local::shell_env::find(name).as_deref() == Some(path.as_path()));
    if interpreter {
        return path;
    }
    crate::config::test_contained(path)
}

/// `program` by absolute path, to run a test's fake agent: a bare executable
/// would be discovered as the user's own agent, which tests never reach.
#[cfg(test)]
pub(crate) fn fake_agent_interpreter(program: &str) -> String {
    let program = if cfg!(windows) && program == "python3" {
        "python"
    } else {
        program
    };
    assert!(FAKE_AGENT_INTERPRETERS.contains(&program), "{program}");
    crate::local::shell_env::find(program)
        .unwrap_or_else(|| panic!("{program} is not on PATH"))
        .to_string_lossy()
        .into_owned()
}

pub fn save(definition: Definition, create: bool) -> Result<()> {
    definition.validate()?;
    if presets()
        .iter()
        .any(|preset| preset.launch.id == definition.id)
    {
        bail!("Built-in coding agents cannot be edited; add a custom configuration instead");
    }
    let mut found = false;
    crate::telemetry::mutate_settings(|settings| {
        if let Some(saved) = settings
            .acp_harnesses
            .iter_mut()
            .find(|saved| saved.id == definition.id)
        {
            found = true;
            if !create {
                let unknown = std::mem::take(&mut saved.unknown);
                *saved = Definition {
                    unknown,
                    ..definition.clone()
                };
            }
        } else if create {
            settings.acp_harnesses.push(definition);
        }
    })?;
    if create && found {
        bail!("An ACP agent with this ID already exists");
    }
    if !create && !found {
        bail!("This ACP agent no longer exists");
    }
    Ok(())
}

pub fn remove(id: &str) -> Result<()> {
    crate::telemetry::mutate_settings(|settings| {
        settings.acp_harnesses.retain(|saved| saved.id != id)
    })?;
    Ok(())
}

pub async fn test_connection(definition: &Definition) -> Result<serde_json::Value> {
    use agent_client_protocol::schema::ProtocolVersion;
    use agent_client_protocol::{Agent, ByteStreams, ConnectionTo};
    use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

    definition.validate()?;
    let executable = executable_path(&definition.executable)
        .unwrap_or_else(|| std::path::PathBuf::from(&definition.executable));
    let mut command = crate::local::harness::managed::command(&executable);
    command
        .args(&definition.arguments)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            crate::error::anyhow!("Executable not found. Install the agent on the machine running OpenResearch, or enter its absolute path.")
        } else {
            crate::error::anyhow!("Could not start the ACP agent: {error}")
        }
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
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        agent_client_protocol::Client.builder().connect_with(
            transport,
            |connection: ConnectionTo<Agent>| async move {
                connection.send_request(initialization()).block_task().await
            },
        ),
    )
    .await;
    crate::local::chat::kill_shell_group(process.id.take());
    let _ = process.child.kill().await;
    let _ = process.child.wait().await;
    let initialized = result.map_err(|_| {
        crate::error::anyhow!("ACP initialization timed out. Check the executable and arguments.")
    })??;
    if initialized.protocol_version != ProtocolVersion::V1 {
        bail!("Incompatible ACP protocol. This backend supports stable ACP v1.");
    }
    Ok(
        serde_json::json!({"status": "connected", "agentInfo": initialized.agent_info, "capabilities": initialized.agent_capabilities}),
    )
}

/// A private temporary working directory, removed on drop.
struct Directory(std::path::PathBuf);

impl Directory {
    fn create(id: &str) -> Result<Self> {
        let cwd = std::env::temp_dir().join(id);
        #[cfg(unix)]
        let created = {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&cwd)
        };
        #[cfg(not(unix))]
        let created = std::fs::create_dir(&cwd);
        created.map_err(|error| {
            crate::error::anyhow!(
                "Could not create a working directory at {}: {error}",
                cwd.display()
            )
        })?;
        Ok(Self(cwd))
    }

    /// Removes the directory once its child has exited. Windows can hold the
    /// child's working directory for a moment after the exit, so this retries for
    /// about three seconds.
    async fn remove(self) {
        let mut delay = std::time::Duration::from_millis(100);
        for _ in 0..5 {
            if std::fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            tokio::time::sleep(delay).await;
            delay *= 2;
        }
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            if self.0.exists() {
                eprintln!(
                    "orx up: could not remove the agent's directory {}: {error}",
                    self.0.display()
                );
            }
        }
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub async fn discover_models(definition: &Definition) -> Result<serde_json::Value> {
    let id = format!("acp-catalog-{}", uuid::Uuid::new_v4());
    let directory = Directory::create(&id)?;
    let models = async {
        let cwd = &directory.0;
        let client = host::spawn(definition, cwd, &id, None, None).await?;
        let response = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            client.request(
                "session/new",
                agent_client_protocol::schema::v1::NewSessionRequest::new(cwd),
            ),
        )
        .await;
        // Discovery never sets session ownership, so delegated tools and approvals are rejected.
        client.shutdown().await;
        response.map_err(|_| crate::error::anyhow!("Loading the agent's models timed out"))?
    }
    .await;
    directory.remove().await;
    models
}

/// A one-shot's client, and a receiver that closes when its task has cleaned up.
type RunningOneShot = (
    std::sync::Weak<host::Client>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(not(test))]
static RUNNING_ONE_SHOTS: std::sync::Mutex<Vec<RunningOneShot>> = std::sync::Mutex::new(Vec::new());

// Parallel tests each run their own runtime on their own thread and call `Host::shutdown`;
// a registry per thread keeps one test from stopping another test's one-shots.
#[cfg(test)]
thread_local! {
    static RUNNING_ONE_SHOTS: std::cell::RefCell<Vec<RunningOneShot>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn running_one_shots<R>(update: impl FnOnce(&mut Vec<RunningOneShot>) -> R) -> R {
    #[cfg(not(test))]
    {
        update(&mut RUNNING_ONE_SHOTS.lock().unwrap())
    }
    #[cfg(test)]
    {
        RUNNING_ONE_SHOTS.with_borrow_mut(update)
    }
}

/// Stops the one-shots still answering or cleaning up, and waits for their cleanup, so
/// that their children and directories do not outlive `orx up`.
async fn stop_one_shots() {
    for (client, finished) in running_one_shots(std::mem::take) {
        if let Some(client) = client.upgrade() {
            client.shutdown().await;
        }
        // Once the child is gone the cleanup only removes the directory; `Directory`'s
        // drop makes the last attempt if this bound passes.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), finished).await;
    }
}

/// [`crate::local::harness::Harness::one_shot`] on a throwaway child in a private
/// directory, with no MCP servers, delegated tools or approvals.
pub async fn one_shot(
    definition: &Definition,
    request: crate::local::harness::OneShot<'_>,
) -> Result<String> {
    let (answer, _cleanup) = spawn_one_shot(definition, request);
    answer.await.unwrap_or_else(|_| {
        Err(crate::error::anyhow!(
            "{} stopped before answering",
            definition.name
        ))
    })
}

/// Runs [`one_shot`] in a task that answers as soon as it can, then cleans up: deletes
/// the session, stops the child and removes its directory. The task ends after cleanup.
fn spawn_one_shot(
    definition: &Definition,
    request: crate::local::harness::OneShot<'_>,
) -> (
    tokio::sync::oneshot::Receiver<Result<String>>,
    tokio::task::JoinHandle<()>,
) {
    use crate::error::anyhow;
    let definition = definition.clone();
    // ACP has no system prompt field, so the system text leads the message.
    let text = format!("{}\n\n{}", request.system, request.prompt);
    let model = request.model.map(str::to_owned);
    let timeout = request.timeout;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let cleanup = tokio::spawn(async move {
        // Dropped when the task ends, which tells `stop_one_shots` the cleanup is over.
        let (_cleaning, finished) = tokio::sync::oneshot::channel::<()>();
        let name = &definition.name;
        let timed_out = || anyhow!("{name} did not answer within {}s", timeout.as_secs());
        let deadline = tokio::time::Instant::now() + timeout;
        let id = format!("acp-oneshot-{}", uuid::Uuid::new_v4());
        let directory = match Directory::create(&id) {
            Ok(directory) => directory,
            Err(error) => {
                let _ = sender.send(Err(error));
                return;
            }
        };
        let mut reply = Some(sender);
        let spawned = {
            let spawn = host::spawn(&definition, &directory.0, &id, None, None);
            tokio::pin!(spawn);
            tokio::select! {
                spawned = &mut spawn => spawned,
                _ = tokio::time::sleep_until(deadline) => {
                    if let Some(sender) = reply.take() {
                        let _ = sender.send(Err(timed_out()));
                    }
                    // A failed spawn returns after its child has exited.
                    spawn.await
                }
            }
        };
        match spawned {
            Err(error) => {
                if let Some(sender) = reply.take() {
                    let _ = sender.send(Err(anyhow!("{name} could not start: {error}")));
                }
            }
            Ok(client) => {
                running_one_shots(|running| {
                    running.retain(|(client, _)| client.strong_count() > 0);
                    running.push((std::sync::Arc::downgrade(&client), finished));
                });
                if let Some(mut sender) = reply.take() {
                    client.mark_one_shot();
                    let answer = tokio::select! {
                        answer = tokio::time::timeout_at(
                            deadline,
                            prompt_once(&client, &directory.0, name, text, model.as_deref()),
                        ) => answer.ok(),
                        // The caller gave up, so nobody reads the answer.
                        _ = sender.closed() => None,
                    };
                    if answer.is_none() {
                        // The agent may still be working on the abandoned prompt.
                        client.cancel();
                    }
                    let _ = sender.send(answer.unwrap_or_else(|| Err(timed_out())));
                }
                if let Some(session_id) = client.native_id().filter(|_| {
                    client.initialized["agentCapabilities"]["sessionCapabilities"]["delete"]
                        .is_object()
                }) {
                    // Best effort, bounded by `Client::request`: a failure only leaves the
                    // session in the agent's own history.
                    if let Err(error) = client
                        .request(
                            "session/delete",
                            serde_json::json!({"sessionId": session_id}),
                        )
                        .await
                    {
                        eprintln!("orx up: {name} kept one-shot session {session_id}; session/delete failed: {error}");
                    }
                }
                client.shutdown().await;
            }
        }
        directory.remove().await;
    });
    (receiver, cleanup)
}

async fn prompt_once(
    client: &host::Client,
    cwd: &std::path::Path,
    name: &str,
    text: String,
    model: Option<&str>,
) -> Result<String> {
    use crate::error::anyhow;
    use agent_client_protocol::schema::v1::{
        ContentBlock, NewSessionRequest, PromptRequest, TextContent,
    };
    let session = client
        .request("session/new", NewSessionRequest::new(cwd))
        .await
        .map_err(|error| anyhow!("{name} could not start a session: {error}"))?;
    let native_id = session["sessionId"]
        .as_str()
        .ok_or_else(|| anyhow!("{name} did not return a session ID"))?;
    client.set_native_id(native_id);
    let has_model_selection = session["models"].is_object()
        || session["configOptions"]
            .as_array()
            .is_some_and(|options| options.iter().any(|option| option["category"] == "model"));
    let model = model
        .filter(|model| crate::local::harness::concrete_model(model))
        .map(str::to_owned)
        .or_else(|| {
            crate::local::harness::resolve_models(
                "",
                &mut crate::local::harness::acp::advertised_models(&session),
                crate::local::harness::acp::current_model(&session),
            )
        });
    if has_model_selection {
        let model = model
            .ok_or_else(|| anyhow!("{name} advertised model selection without a usable model"))?;
        select_model(client, &session, native_id, name, &model).await?;
    }
    // pi-acp announces its startup banner in this `_meta` and also sends it as a message
    // chunk right after `session/new`, which can arrive once the turn has begun.
    let mut startup = session["_meta"]["piAcp"]["startupInfo"].as_str();
    let mut events = client.begin_turn();
    let response = client.request(
        "session/prompt",
        PromptRequest::new(
            native_id.to_owned(),
            vec![ContentBlock::Text(TextContent::new(text))],
        ),
    );
    tokio::pin!(response);
    let mut answer = String::new();
    let result = loop {
        tokio::select! {
            result = &mut response => break result.map_err(|error| {
                let exited = error
                    .downcast_ref::<agent_client_protocol::Error>()
                    .is_some_and(agent_client_protocol::is_incoming_transport_closed);
                if exited {
                    anyhow!("{name} exited before answering")
                } else {
                    anyhow!("{name} could not answer: {error}")
                }
            })?,
            event = events.recv() => {
                let event = event.ok_or_else(|| anyhow!("{name} exited before answering"))?;
                one_shot_event(client, event, &mut startup, &mut answer);
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        one_shot_event(client, event, &mut startup, &mut answer);
    }
    match result["stopReason"].as_str() {
        Some("end_turn") => {}
        reason => bail!(
            "{name} stopped without answering ({})",
            reason.unwrap_or("no stop reason")
        ),
    }
    if answer.trim().is_empty() {
        bail!("{name} returned no reply");
    }
    Ok(answer)
}

async fn select_model(
    client: &host::Client,
    session: &serde_json::Value,
    native_id: &str,
    name: &str,
    model: &str,
) -> Result<()> {
    use serde_json::json;
    let (method, params) = if let Some(options) = session["configOptions"].as_array() {
        let Some(option) = options
            .iter()
            .find(|option| option["category"] == "model" && host::select_has_value(option, model))
        else {
            bail!("{name} does not offer model {model}. Re-check it in Settings → Agents.");
        };
        if option["currentValue"].as_str() == Some(model) {
            return Ok(());
        }
        (
            "session/set_config_option",
            json!({"sessionId": native_id, "configId": option["id"], "value": model}),
        )
    } else {
        let models = &session["models"];
        let advertised = models["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|option| option["modelId"].as_str() == Some(model));
        if models["currentModelId"].as_str() == Some(model) {
            return Ok(());
        }
        if !advertised {
            bail!("{name} does not offer model {model}. Re-check it in Settings → Agents.");
        }
        (
            "session/set_model",
            json!({"sessionId": native_id, "modelId": model}),
        )
    };
    client
        .request(method, params)
        .await
        .map_err(|error| crate::error::anyhow!("{name} could not select model {model}: {error}"))?;
    Ok(())
}

/// Adds a message chunk to `answer`, except a first chunk that equals the announced `startup` text.
fn one_shot_event(
    client: &host::Client,
    event: host::Event,
    startup: &mut Option<&str>,
    answer: &mut String,
) {
    match event {
        host::Event::Update(update) => {
            if update["sessionUpdate"] == "agent_message_chunk"
                && update["content"]["type"] == "text"
            {
                if let Some(text) = update["content"]["text"].as_str() {
                    if startup.take() != Some(text) {
                        answer.push_str(text);
                    }
                }
            }
        }
        host::Event::Permission { id, request } => {
            let reject = request["options"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|option| {
                    option["kind"]
                        .as_str()
                        .is_some_and(|kind| kind.starts_with("reject"))
                })
                .and_then(|option| option["optionId"].as_str());
            if reject.is_none_or(|option| client.answer(&id, option).is_err()) {
                client.cancel_permissions();
            }
        }
        host::Event::TerminalOutput { .. } => {}
    }
}

pub(crate) fn alphaxiv_mcp_servers(
) -> crate::error::Result<Vec<agent_client_protocol::schema::v1::McpServer>> {
    use agent_client_protocol::schema::v1::{McpServer, McpServerStdio};
    Ok(vec![McpServer::Stdio(
        McpServerStdio::new("alphaxiv", crate::paths::spawnable_exe()?)
            .args(vec!["alphaxiv-mcp".to_owned()]),
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::harness::{OneShot, OneShotQuality};

    #[test]
    fn an_absolute_agent_outside_the_test_root_is_refused() {
        let outside = std::path::absolute("/orx-not-an-agent").unwrap();
        let found = std::panic::catch_unwind(|| user_executable_path(&outside.to_string_lossy()));
        assert!(found.is_err(), "resolved {:?}", found.unwrap());
        let interpreter = fake_agent_interpreter("python3");
        assert_eq!(user_executable_path(&interpreter), Some(interpreter.into()));
    }

    fn peer(mode: &str, record: &std::path::Path) -> Definition {
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
                record.to_string_lossy().into_owned(),
            ],
            unknown: Default::default(),
        }
    }

    fn request<'a>(prompt: &'a str, model: Option<&'a str>, timeout: u64) -> OneShot<'a> {
        OneShot {
            profile: None,
            system: "SYSTEM TEXT",
            prompt,
            quality: OneShotQuality::Cheap,
            model,
            timeout: std::time::Duration::from_secs(timeout),
        }
    }

    /// Runs one request against the fixture peer and waits for its cleanup; returns the
    /// answer and the methods the peer received.
    async fn run(
        mode: &str,
        request: OneShot<'_>,
    ) -> (Result<String>, Vec<(String, serde_json::Value)>) {
        let stalled =
            matches!(mode, "stalled-init" | "stalled-setup") || request.prompt == "ACP_TEST:hang";
        let listener = if stalled {
            Some(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap())
        } else {
            None
        };
        let signaled_mode = listener
            .as_ref()
            .map(|listener| format!("{mode}:{}", listener.local_addr().unwrap().port()));
        // Only to just past the stall's deadline: cleanup bounds its own waits with Tokio timers.
        let stall = if mode == "stalled-init" {
            super::host::INITIALIZATION_BOUND
        } else {
            request.timeout
        };
        let advance = stall + std::time::Duration::from_secs(1);
        let record =
            std::env::temp_dir().join(format!("acp-oneshot-record-{}", uuid::Uuid::new_v4()));
        let (mut answer, cleanup) = spawn_one_shot(
            &peer(signaled_mode.as_deref().unwrap_or(mode), &record),
            request,
        );

        if let Some(listener) = listener {
            let reached_stall =
                tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept());
            tokio::select! {
                result = &mut answer => panic!("the peer never stalled: {result:?}"),
                connected = reached_stall => {
                    connected.expect("the peer never reached its stall").unwrap();
                }
            }
            tokio::time::pause();
            tokio::time::advance(advance).await;
            tokio::time::resume();
        }

        let answer = answer.await.unwrap();
        finish(cleanup).await;
        (answer, received(record))
    }

    async fn finish(cleanup: tokio::task::JoinHandle<()>) {
        tokio::time::timeout(std::time::Duration::from_secs(60), cleanup)
            .await
            .unwrap()
            .unwrap();
    }

    /// Reads and removes the peer's record, asserting that its working directory is gone.
    fn received(record: std::path::PathBuf) -> Vec<(String, serde_json::Value)> {
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&record)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let _ = std::fs::remove_file(record);
        if let Some(line) = lines.first() {
            let cwd = std::path::PathBuf::from(line["cwd"].as_str().unwrap());
            assert!(cwd
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("acp-oneshot-"));
            assert!(!cwd.exists(), "{} was left behind", cwd.display());
        }
        lines
            .into_iter()
            .map(|line| {
                (
                    line["method"].as_str().unwrap().to_owned(),
                    line["params"].clone(),
                )
            })
            .collect()
    }

    fn position(received: &[(String, serde_json::Value)], method: &str) -> Option<usize> {
        received.iter().position(|(name, _)| name == method)
    }

    /// The `slow-delete` peer mode: the peer holds `session/delete` on a connection to
    /// the returned listener and answers it once that connection closes.
    async fn slow_delete() -> (String, tokio::net::TcpListener) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mode = format!("slow-delete:{}", listener.local_addr().unwrap().port());
        (mode, listener)
    }

    async fn held_delete(listener: &tokio::net::TcpListener) -> tokio::net::TcpStream {
        let (hold, _) = tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept())
            .await
            .expect("the peer never held session/delete")
            .unwrap();
        hold
    }

    #[tokio::test]
    async fn one_shot_answers_with_the_message_text_after_the_system_text() {
        let (answer, received) = run("normal", request("ACP_TEST:echo", None, 20)).await;
        let text = "SYSTEM TEXT\n\nACP_TEST:echo";
        assert_eq!(answer.unwrap(), format!("turn 1{text}"));
        let (_, session) = &received[position(&received, "session/new").unwrap()];
        assert_eq!(session["mcpServers"], serde_json::json!([]));
        let cwd = std::path::Path::new(session["cwd"].as_str().unwrap());
        assert!(cwd
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("acp-oneshot-"));
        let (_, prompt) = &received[position(&received, "session/prompt").unwrap()];
        assert_eq!(
            prompt["prompt"],
            serde_json::json!([{"type": "text", "text": text}])
        );
        assert!(position(&received, "session/delete").is_none());
    }

    #[tokio::test]
    async fn one_shot_leaves_out_the_startup_text_the_agent_announced() {
        for model in [None, Some("second")] {
            let (answer, received) = run("startup", request("ACP_TEST:hello", model, 20)).await;
            assert_eq!(answer.unwrap(), "turn 1", "{model:?}");
            let switched = position(&received, "session/set_config_option").is_some();
            assert_eq!(switched, model.is_some());
        }
        // Text other than the announced startup text stays in the answer.
        let (answer, _) = run("startup-other", request("ACP_TEST:hello", None, 20)).await;
        assert_eq!(answer.unwrap(), "Update available\nturn 1");
    }

    #[tokio::test]
    async fn one_shot_selects_a_concrete_model_before_prompting() {
        for (mode, model, method) in [
            ("normal", Some("second"), Some("session/set_config_option")),
            ("legacy-models", Some("second"), Some("session/set_model")),
            ("normal", Some("first"), None),
            ("normal", Some("default"), None),
            ("normal", None, None),
        ] {
            let (answer, received) = run(mode, request("ACP_TEST:hello", model, 20)).await;
            assert_eq!(answer.unwrap(), "turn 1", "{mode} {model:?}");
            let prompt = position(&received, "session/prompt").unwrap();
            let selected = received
                .iter()
                .position(|(name, _)| name.starts_with("session/set_"));
            assert_eq!(selected.map(|index| received[index].0.as_str()), method);
            if let Some(index) = selected {
                assert!(index < prompt);
                assert_eq!(received[index].1["sessionId"], "peer-session");
                let value = &received[index].1[if mode == "normal" { "value" } else { "modelId" }];
                assert_eq!(value.as_str(), model);
            }
        }
    }

    #[tokio::test]
    async fn one_shot_refuses_unavailable_or_rejected_models_before_prompting() {
        for (mode, model) in [
            ("normal", "reject"),
            ("normal", "unknown"),
            ("legacy-models", "unknown"),
        ] {
            let (answer, received) = run(mode, request("ACP_TEST:hello", Some(model), 20)).await;
            assert!(answer.unwrap_err().to_string().contains(model));
            assert!(position(&received, "session/prompt").is_none());
        }
    }

    #[tokio::test]
    async fn one_shot_inherits_the_agents_selection_without_a_model_api() {
        for mode in ["no-models", "other-options"] {
            for model in [None, Some("second")] {
                let (answer, received) = run(mode, request("ACP_TEST:hello", model, 20)).await;
                assert_eq!(answer.unwrap(), "turn 1");
                assert!(position(&received, "session/prompt").is_some());
                assert!(!received
                    .iter()
                    .any(|(method, _)| method.starts_with("session/set_")));
            }
        }
        let (answer, received) = run("empty-models", request("ACP_TEST:hello", None, 20)).await;
        assert!(answer
            .unwrap_err()
            .to_string()
            .contains("without a usable model"));
        assert!(position(&received, "session/prompt").is_none());
    }

    #[tokio::test]
    async fn one_shot_rejects_permissions_and_delegated_tools() {
        let (answer, _) = run("normal", request("ACP_TEST:permission", None, 20)).await;
        let answer = answer.unwrap();
        assert!(answer.contains("opaque-deny"), "{answer}");
        assert!(!answer.contains("opaque-once"), "{answer}");
        let path = std::env::temp_dir().join(format!("acp-oneshot-write-{}", uuid::Uuid::new_v4()));
        let prompt = format!("ACP_TEST:denied-write:{}", path.display());
        let (answer, _) = run("normal", request(&prompt, None, 20)).await;
        assert!(answer.unwrap().contains("Tools are disabled"));
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn one_shot_deletes_its_session_when_the_agent_can() {
        let (answer, received) = run("delete", request("ACP_TEST:hello", None, 20)).await;
        assert_eq!(answer.unwrap(), "turn 1");
        let delete = position(&received, "session/delete").unwrap();
        assert!(delete > position(&received, "session/prompt").unwrap());
        assert_eq!(received[delete].1["sessionId"], "peer-session");
        // An answered prompt needs no cancel before the delete.
        assert!(position(&received, "session/cancel").unwrap() > delete);
    }

    #[tokio::test]
    async fn one_shot_cancels_an_abandoned_prompt_before_deleting_its_session() {
        let (answer, received) = run("delete", request("ACP_TEST:hang", None, 5)).await;
        let error = answer.unwrap_err().to_string();
        assert!(error.contains("Peer") && error.contains("5s"), "{error}");
        let cancel = position(&received, "session/cancel").unwrap();
        assert!(cancel > position(&received, "session/prompt").unwrap());
        assert!(cancel < position(&received, "session/delete").unwrap());
    }

    #[tokio::test]
    async fn one_shot_cancels_the_prompt_of_a_caller_that_gave_up() {
        let record =
            std::env::temp_dir().join(format!("acp-oneshot-record-{}", uuid::Uuid::new_v4()));
        // Far beyond `finish`'s bound, so only the caller leaving can end the prompt.
        let (answer, cleanup) = spawn_one_shot(
            &peer("delete", &record),
            request("ACP_TEST:hang", None, 600),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !std::fs::read_to_string(&record)
            .unwrap_or_default()
            .contains("session/prompt")
        {
            assert!(std::time::Instant::now() < deadline, "no session/prompt");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        drop(answer);
        finish(cleanup).await;
        let received = received(record);
        let cancel = position(&received, "session/cancel").unwrap();
        assert!(cancel > position(&received, "session/prompt").unwrap());
        assert!(cancel < position(&received, "session/delete").unwrap());
    }

    #[tokio::test]
    async fn host_shutdown_stops_a_one_shot_that_is_still_cleaning_up() {
        let record =
            std::env::temp_dir().join(format!("acp-oneshot-record-{}", uuid::Uuid::new_v4()));
        let (mode, listener) = slow_delete().await;
        let (answer, cleanup) =
            spawn_one_shot(&peer(&mode, &record), request("ACP_TEST:hello", None, 20));
        assert_eq!(answer.await.unwrap().unwrap(), "turn 1");
        let mut hold = held_delete(&listener).await;
        host::Host::default().shutdown().await;
        // The peer's exit closes the connection that holds the delete.
        let closed = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::io::AsyncReadExt::read(&mut hold, &mut [0]),
        )
        .await;
        assert!(closed.is_ok(), "the peer still holds session/delete");
        // `received` asserts that the directory is already gone.
        let received = received(record);
        assert!(position(&received, "session/delete").is_some());
        assert!(position(&received, "answered session/delete").is_none());
        finish(cleanup).await;
    }

    #[tokio::test]
    async fn one_shot_answers_before_a_slow_session_delete_and_stops_the_agent_after_it() {
        let record =
            std::env::temp_dir().join(format!("acp-oneshot-record-{}", uuid::Uuid::new_v4()));
        let (mode, listener) = slow_delete().await;
        let (answer, cleanup) =
            spawn_one_shot(&peer(&mode, &record), request("ACP_TEST:hello", None, 20));
        let answer = tokio::time::timeout(std::time::Duration::from_secs(25), answer)
            .await
            .expect("no answer while the peer held session/delete");
        assert_eq!(answer.unwrap().unwrap(), "turn 1");
        let hold = held_delete(&listener).await;
        assert!(!cleanup.is_finished());
        drop(hold);
        finish(cleanup).await;
        let received = received(record);
        let delete = position(&received, "session/delete").unwrap();
        assert!(delete > position(&received, "session/prompt").unwrap());
        let answered = position(&received, "answered session/delete").unwrap();
        // The peer lived to answer the delete, then exited on shutdown's session/cancel.
        assert!(position(&received, "session/cancel").unwrap() > answered);
    }

    #[tokio::test]
    async fn one_shot_removes_its_directory_after_failed_starts_and_timeouts() {
        for (mode, prompt, timeout) in [
            ("crash-init", "ACP_TEST:hello", 20),
            ("stalled-init", "ACP_TEST:hello", 1),
            ("delete", "ACP_TEST:hang", 2),
        ] {
            let (answer, received) = run(mode, request(prompt, None, timeout)).await;
            assert!(answer.is_err(), "{mode}");
            // `received` asserts that the peer's working directory is gone.
            assert!(position(&received, "initialize").is_some(), "{mode}");
        }
    }

    #[tokio::test]
    async fn one_shot_names_the_agent_and_cause_of_each_failure() {
        let mut causes = std::collections::HashSet::new();
        let started = std::time::Instant::now();
        let (answer, received) = run("delete", request("ACP_TEST:hang", None, 5)).await;
        let error = answer.unwrap_err().to_string();
        assert!(error.contains("Peer") && error.contains("5s"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(15));
        assert!(position(&received, "session/delete").is_some());
        causes.insert(error.replace("5s", ""));
        let (answer, _) = run("stalled-setup", request("ACP_TEST:hello", None, 1)).await;
        let error = answer.unwrap_err().to_string();
        assert!(error.contains("Peer") && error.contains("1s"), "{error}");
        // Both are timeouts, so they share a cause.
        assert!(!causes.insert(error.replace("1s", "")), "{error}");
        for (mode, prompt, part) in [
            ("normal", "ACP_TEST:refuse", "refusal"),
            ("normal", "ACP_TEST:empty", "Peer"),
            // The agent's own error on the prompt, unlike an exit.
            ("normal", "ACP_TEST:error", "Model overloaded"),
            ("auth-init", "ACP_TEST:hello", "Authentication required"),
        ] {
            let (answer, _) = run(mode, request(prompt, None, 20)).await;
            let error = answer.unwrap_err().to_string();
            assert!(error.contains("Peer") && error.contains(part), "{error}");
            assert!(causes.insert(error.clone()), "{error}");
        }
        let mut crashes = std::collections::HashSet::new();
        for _ in 0..3 {
            let (answer, _) = run("normal", request("ACP_TEST:crash", None, 20)).await;
            crashes.insert(answer.unwrap_err().to_string());
        }
        assert_eq!(crashes.len(), 1, "{crashes:?}");
        let crash = crashes.into_iter().next().unwrap();
        assert!(
            crash.contains("Peer") && crash.contains("exited"),
            "{crash}"
        );
        assert!(!crash.contains("transport"), "{crash}");
        assert!(causes.insert(crash.clone()), "{crash}");
        let mut missing = peer("normal", &std::env::temp_dir());
        missing.executable = format!("missing-acp-{}", uuid::Uuid::new_v4());
        let error = one_shot(&missing, request("ACP_TEST:hello", None, 20))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Executable not found"), "{error}");
    }

    #[tokio::test]
    async fn model_discovery_creates_session_without_prompting_or_delegated_tools() {
        let peer = r#"
import json, os, sys
for line in sys.stdin:
    request = json.loads(line)
    method = request['method']
    if method == 'initialize':
        result = {'protocolVersion':1, 'agentCapabilities':{}}
    elif method == 'session/new':
        assert os.path.isabs(request['params']['cwd']), request
        print(json.dumps({'jsonrpc':'2.0','id':'read','method':'fs/read_text_file','params':{'sessionId':'catalog','path':'/etc/hosts'}}), flush=True)
        denied = json.loads(sys.stdin.readline())
        assert 'error' in denied, denied
        result = {'sessionId':'catalog','models':{'currentModelId':'one','availableModels':[{'modelId':'one','name':'One'},{'modelId':'two','name':'Two'}]}}
    else:
        raise AssertionError(method)
    print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result}), flush=True)
"#;
        let definition = Definition {
            id: format!("acp:{}", uuid::Uuid::new_v4()),
            name: "Catalog peer".into(),
            executable: crate::local::acp::fake_agent_interpreter("python3"),
            arguments: vec!["-u".into(), "-c".into(), peer.into()],
            unknown: Default::default(),
        };
        let configuration = discover_models(&definition).await.unwrap();
        assert_eq!(
            configuration["models"]["availableModels"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn popular_agents_have_valid_stable_launchers() {
        let presets = presets();
        let mut ids = std::collections::HashSet::new();
        for preset in &presets {
            preset.launch.validate().unwrap();
            assert!(ids.insert(preset.launch.id.clone()));
            assert!(preset.setup_url.starts_with("https://"));
        }
        let commands: Vec<_> = presets
            .iter()
            .map(|preset| {
                (
                    preset.launch.name.as_str(),
                    preset.launch.executable.as_str(),
                    preset
                        .launch
                        .arguments
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        let mut expected = vec![
            ("DeepSeek Harness", "dsh", vec!["--profile", "acp"]),
            ("Kimi Code", "kimi", vec!["acp"]),
            ("MiniMax Code", "mcode", vec!["acp"]),
            ("ZCode", "zcode-acp-server", vec![]),
            ("Pi", "pi-acp", vec![]),
            ("oh-my-pi", "omp", vec!["acp"]),
            ("Devin CLI", "devin", vec!["acp"]),
            ("Grok Build", "grok", vec!["agent", "stdio"]),
        ];
        if !cfg!(windows) {
            expected.push(("fx", "fx", vec!["acp"]));
        }
        assert_eq!(commands, expected);
        let file = crate::config::test_home().join("popular-agent");
        std::fs::write(&file, "").unwrap();
        assert_eq!(executable_path(file.to_str().unwrap()), Some(file));
        assert!(executable_path("missing-popular-agent-28e8c11").is_none());
    }

    #[test]
    fn saved_launch_suppresses_preset_without_losing_customization() {
        let preset = presets().remove(0).launch;
        let mut saved = preset.clone();
        saved.id = format!("acp:{}", uuid::Uuid::new_v4());
        saved.name = "My DeepSeek".into();
        if let Some(path) = executable_path(&saved.executable) {
            saved.executable = path.to_string_lossy().into_owned();
        }

        let mut second = saved.clone();
        second.id = format!("acp:{}", uuid::Uuid::new_v4());
        second.name = "Another DeepSeek".into();
        let registered = register_definitions(vec![saved.clone(), second.clone()]);
        assert_eq!(&registered[..2], &[saved.clone(), second]);
        let matched = registered
            .iter()
            .find(|item| same_launch(item, &preset))
            .unwrap();
        assert_eq!(matched, &saved);
        assert!(!registered.iter().any(|item| item.id == preset.id));

        saved.arguments.push("--other".into());
        let refreshed = register_definitions(vec![saved.clone()]);
        assert!(refreshed.contains(&saved));
        assert_eq!(
            refreshed
                .iter()
                .find(|item| same_launch(item, &preset))
                .unwrap()
                .id,
            preset.id
        );
        assert_eq!(
            register_definitions(vec![])
                .iter()
                .find(|item| same_launch(item, &preset))
                .unwrap()
                .id,
            preset.id
        );
    }

    #[test]
    fn preset_registration_refreshes_executable_paths() {
        if !crate::config::in_test_child(&[]) {
            return;
        }

        let root = crate::local::git::TemporaryDirectory::new("orx-preset-path").unwrap();
        let preset = presets().remove(0).launch;
        let binary = root
            .path()
            .join(if cfg!(windows) { "dsh.cmd" } else { "dsh" });
        let mut saved = preset.clone();
        saved.id = format!("acp:{}", uuid::Uuid::new_v4());
        saved.executable = binary.to_string_lossy().into_owned();
        std::env::set_var("PATH", root.path());

        assert!(register_definitions(vec![saved.clone()])
            .iter()
            .any(|definition| definition.id == preset.id));

        std::fs::write(&binary, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let registered = register_definitions(vec![saved.clone()]);
        assert_eq!(registered.first(), Some(&saved));
        assert!(!registered
            .iter()
            .any(|definition| definition.id == preset.id));

        std::fs::remove_file(binary).unwrap();
        assert!(register_definitions(vec![saved])
            .iter()
            .any(|definition| definition.id == preset.id));
    }

    #[tokio::test]
    async fn initialization_uses_stdio_and_never_creates_a_session() {
        let peer = r#"
import json, sys
assert sys.argv[1:] == ['two words', '\"quoted\"', '$(echo untouched)']
request = json.loads(sys.stdin.readline())
assert request['method'] == 'initialize'
assert request['params']['protocolVersion'] == 1
assert request['params']['clientCapabilities']['fs'] == {'readTextFile': True, 'writeTextFile': True}
assert request['params']['clientCapabilities']['terminal'] is True
print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': {
    'protocolVersion': 1, 'agentCapabilities': {},
    'agentInfo': {'name': 'deterministic-peer', 'version': '1.0'}
}}), flush=True)
for line in sys.stdin:
    request = json.loads(line)
    assert request['method'] not in ['session/new', 'session/prompt']
"#;
        let definition = Definition {
            id: format!("acp:{}", uuid::Uuid::new_v4()),
            name: "Test".into(),
            executable: crate::local::acp::fake_agent_interpreter("python3"),
            arguments: vec![
                "-u".into(),
                "-c".into(),
                peer.into(),
                "two words".into(),
                "\"quoted\"".into(),
                "$(echo untouched)".into(),
            ],
            unknown: Default::default(),
        };
        let result = test_connection(&definition).await.unwrap();
        assert_eq!(result["status"], "connected");
        assert_eq!(result["agentInfo"]["version"], "1.0");
        use crate::local::harness::Harness;
        let preset = presets()
            .into_iter()
            .find(|preset| preset.launch.executable == "grok")
            .unwrap();
        let mut detected = definition.clone();
        detected.id = preset.launch.id.clone();
        let adapter = crate::local::harness::acp::Acp(detected);
        let snapshot = adapter.detect_snapshot().await.unwrap();
        assert!(snapshot.installed);
        assert!(!snapshot.agent_ready);
        let connected = adapter.detect().await.unwrap();
        assert!(connected.agent_ready);
        assert_eq!(connected.version.as_deref(), Some("1.0"));
        assert_eq!(connected.setup_url, Some(preset.setup_url));
        let mut missing = definition;
        missing.executable = format!("missing-acp-{}", uuid::Uuid::new_v4());
        assert!(test_connection(&missing)
            .await
            .unwrap_err()
            .to_string()
            .contains("Executable not found"));
    }

    #[test]
    fn launch_configuration_preserves_argument_boundaries() {
        let definition = Definition {
            id: format!("acp:{}", uuid::Uuid::new_v4()),
            name: "Custom agent".into(),
            executable: std::env::temp_dir()
                .join("agent with spaces")
                .to_string_lossy()
                .into_owned(),
            arguments: vec![
                "two words".into(),
                "\"quoted\"".into(),
                "$(echo untouched)".into(),
            ],
            unknown: Default::default(),
        };
        definition.validate().unwrap();
        let saved = serde_json::to_string(&definition).unwrap();
        assert_eq!(
            serde_json::from_str::<Definition>(&saved).unwrap(),
            definition
        );
        let mut invalid = definition;
        invalid.executable = "./agent".into();
        assert!(invalid.validate().is_err());
        invalid.executable = "agent".into();
        invalid.arguments.push("invalid\0argument".into());
        assert!(invalid.validate().is_err());
    }
}
