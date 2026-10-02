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

pub fn save(definition: Definition, create: bool) -> Result<()> {
    definition.validate()?;
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
    let mut command = tokio::process::Command::new(&definition.executable);
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

#[cfg(test)]
mod tests {
    use super::*;

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
