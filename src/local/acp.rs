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
}

impl Definition {
    pub fn validate(&self) -> Result<()> {
        let Some(id) = self.id.strip_prefix("acp:") else {
            bail!("ACP harness IDs must start with acp:");
        };
        uuid::Uuid::parse_str(id)?;
        if self.name.trim().is_empty() || self.name.contains('\0') {
            bail!("Enter a name for the ACP harness");
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

pub fn presets() -> Vec<Preset> {
    [
        (
            "dee4a440-fd64-4217-a4a7-1d4c148c0dd1",
            "DeepSeek Harness",
            "dsh",
            &["--profile", "acp"][..],
            "https://github.com/deepseek-ai/deepseek-harness",
        ),
        (
            "a8b48911-e95c-42cb-bc35-bab3aa83e5e2",
            "Kimi Code",
            "kimi",
            &["acp"][..],
            "https://www.kimi.com/code/docs/en/kimi-code-cli/",
        ),
        (
            "1dba9897-707b-4cd8-b595-c8c26115f563",
            "MiniMax Code",
            "mcode",
            &["acp"][..],
            "https://github.com/MiniMax-AI/minimax-code",
        ),
        (
            "2f1639e0-3f0f-4594-bb5b-a4078c36d5a4",
            "ZCode",
            "zcode-acp-server",
            &[][..],
            "https://github.com/william0wang/zcode-acp#install",
        ),
        (
            "467b1900-c3c3-47c1-8b87-09389e1c44b5",
            "Pi",
            "pi-acp",
            &[][..],
            "https://github.com/svkozak/pi-acp#install",
        ),
        (
            "565e351e-62c7-488f-a58a-914d30cc5776",
            "Devin CLI",
            "devin",
            &["acp"][..],
            "https://docs.devin.ai/cli",
        ),
        (
            "ac7f713b-b520-40d2-9b56-75deca1574c7",
            "Grok Build",
            "grok",
            &["agent", "stdio"][..],
            "https://github.com/xai-org/grok-build",
        ),
    ]
    .into_iter()
    .map(|(id, name, executable, arguments, setup_url)| Preset {
        needs_connection_adapter: matches!(executable, "pi-acp" | "zcode-acp-server"),
        launch: Definition {
            id: format!("acp:{id}"),
            name: name.into(),
            executable: executable.into(),
            arguments: arguments
                .iter()
                .map(|argument| (*argument).into())
                .collect(),
        },
        setup_url,
    })
    .collect()
}

pub fn registered_definitions() -> Vec<Definition> {
    let mut definitions = definitions();
    for preset in presets() {
        let mut launch = preset.launch;
        if let Some(path) = executable_path(&launch.executable) {
            launch.executable = path.to_string_lossy().into_owned();
        }
        if !definitions.iter().any(|saved| {
            executable_path(&saved.executable)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|| saved.executable.clone())
                == launch.executable
                && saved.arguments == launch.arguments
        }) {
            definitions.push(launch);
        }
    }
    definitions
}

pub fn executable_path(executable: &str) -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(executable);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    crate::local::shell_env::find_on_path(executable).or_else(|| {
        let home = dirs::home_dir()?;
        [
            ".local/bin",
            ".grok/bin",
            ".devin/bin",
            ".zcode/runtime/bin",
        ]
        .iter()
        .find_map(|directory| {
            crate::local::shell_env::find_in_dir(&home.join(directory), executable)
        })
    })
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
                *saved = definition.clone();
            }
        } else if create {
            settings.acp_harnesses.push(definition);
        }
    })?;
    if create && found {
        bail!("An ACP harness with this ID already exists");
    }
    if !create && !found {
        bail!("This ACP harness no longer exists");
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
    let mut command = tokio::process::Command::new(executable);
    command
        .args(&definition.arguments)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    crate::local::chat::prepare_env(&mut command);
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            crate::error::anyhow!("Executable not found. Install the harness on the backend machine or provide its absolute path.")
        } else {
            crate::error::anyhow!("Could not start ACP harness: {error}")
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

pub async fn discover_models(definition: &Definition) -> Result<serde_json::Value> {
    let id = format!("acp-catalog-{}", uuid::Uuid::new_v4());
    let cwd = std::env::temp_dir().join(&id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&cwd)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&cwd)?;
    struct Directory(std::path::PathBuf);
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _directory = Directory(cwd.clone());
    let client = host::spawn(definition, &cwd, &id, None, None).await?;
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        client.request(
            "session/new",
            agent_client_protocol::schema::v1::NewSessionRequest::new(&cwd),
        ),
    )
    .await;
    // Discovery never sets session ownership, so delegated tools and approvals are rejected.
    client.shutdown().await;
    response.map_err(|_| crate::error::anyhow!("Loading the agent's models timed out"))?
}

#[cfg(test)]
mod tests {
    use super::*;

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
            executable: if cfg!(windows) { "python" } else { "python3" }.into(),
            arguments: vec!["-u".into(), "-c".into(), peer.into()],
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
        assert_eq!(
            commands,
            [
                ("DeepSeek Harness", "dsh", vec!["--profile", "acp"]),
                ("Kimi Code", "kimi", vec!["acp"]),
                ("MiniMax Code", "mcode", vec!["acp"]),
                ("ZCode", "zcode-acp-server", vec![]),
                ("Pi", "pi-acp", vec![]),
                ("Devin CLI", "devin", vec!["acp"]),
                ("Grok Build", "grok", vec!["agent", "stdio"]),
            ]
        );
        let file = std::env::current_exe().unwrap();
        assert_eq!(executable_path(file.to_str().unwrap()), Some(file));
        assert!(executable_path("missing-popular-agent-28e8c11").is_none());
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
            executable: if cfg!(windows) { "python" } else { "python3" }.into(),
            arguments: vec![
                "-u".into(),
                "-c".into(),
                peer.into(),
                "two words".into(),
                "\"quoted\"".into(),
                "$(echo untouched)".into(),
            ],
        };
        let result = test_connection(&definition).await.unwrap();
        assert_eq!(result["status"], "connected");
        assert_eq!(result["agentInfo"]["version"], "1.0");
        use crate::local::harness::Harness;
        let mut detected = definition.clone();
        detected.id = presets()[0].launch.id.clone();
        let adapter = crate::local::harness::acp::Acp(detected);
        let snapshot = adapter.detect_snapshot().await.unwrap();
        assert!(snapshot.installed);
        assert!(!snapshot.agent_ready);
        let connected = adapter.detect().await.unwrap();
        assert!(connected.agent_ready);
        assert_eq!(connected.version.as_deref(), Some("1.0"));
        assert_eq!(connected.setup_url, Some(presets()[0].setup_url));
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
