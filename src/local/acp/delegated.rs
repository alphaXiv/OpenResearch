use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::{
    CreateTerminalRequest, ReadTextFileRequest, TerminalExitStatus, TerminalOutputResponse,
    WriteTextFileRequest,
};
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;
use tokio::sync::watch;

use super::OwnedProcess;
use crate::error::{anyhow, bail, Result};

const MAX_OUTPUT: usize = 1024 * 1024;

struct Output {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl Output {
    fn append(&mut self, bytes: &[u8]) {
        let excess = (self.bytes.len() + bytes.len()).saturating_sub(self.limit);
        if excess > 0 {
            self.truncated = true;
            let discard = excess.min(self.bytes.len());
            self.bytes.drain(..discard);
            self.bytes.extend_from_slice(&bytes[excess - discard..]);
        } else {
            self.bytes.extend_from_slice(bytes);
        }
    }

    fn text(&self) -> String {
        let start = self
            .bytes
            .iter()
            .position(|byte| byte & 0xc0 != 0x80)
            .unwrap_or(self.bytes.len());
        let mut text = String::from_utf8_lossy(&self.bytes[start..]).into_owned();
        let mut start = text.len().saturating_sub(self.limit);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        text.drain(..start);
        text
    }
}

struct Terminal {
    output: Arc<Mutex<Output>>,
    kill: watch::Sender<bool>,
    exit: watch::Receiver<Option<TerminalExitStatus>>,
}

impl Terminal {
    async fn wait(&self) -> Result<Value> {
        let mut exit = self.exit.clone();
        exit.wait_for(Option::is_some)
            .await
            .map_err(|_| anyhow!("Terminal ended without an exit status"))?;
        let status = exit
            .borrow()
            .clone()
            .ok_or_else(|| anyhow!("Terminal has no exit status"))?;
        Ok(serde_json::to_value(status)?)
    }

    async fn kill(&self) -> Result<()> {
        let _ = self.kill.send(true);
        self.wait().await?;
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.kill.send(true);
    }
}

pub struct Delegated {
    cwd: PathBuf,
    terminals: Mutex<HashMap<String, Arc<Terminal>>>,
}

impl Delegated {
    pub fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_owned(),
            terminals: Mutex::new(HashMap::new()),
        }
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        match method {
            "fs/read_text_file" => {
                let request: ReadTextFileRequest = serde_json::from_value(params)?;
                absolute(&request.path)?;
                if request.line == Some(0) {
                    bail!("File line numbers start at 1");
                }
                let content = tokio::fs::read_to_string(&request.path).await?;
                let content = if request.line.is_none() && request.limit.is_none() {
                    content
                } else {
                    content
                        .split_inclusive('\n')
                        .skip(request.line.unwrap_or(1).saturating_sub(1) as usize)
                        .take(
                            request
                                .limit
                                .map(|limit| limit as usize)
                                .unwrap_or(usize::MAX),
                        )
                        .collect()
                };
                Ok(json!({"content": content}))
            }
            "fs/write_text_file" => {
                let request: WriteTextFileRequest = serde_json::from_value(params)?;
                absolute(&request.path)?;
                tokio::fs::write(&request.path, request.content).await?;
                Ok(json!({}))
            }
            "terminal/create" => {
                let request: CreateTerminalRequest = serde_json::from_value(params)?;
                self.create(request).await
            }
            "terminal/output" | "terminal/wait_for_exit" | "terminal/kill" | "terminal/release" => {
                let id = params["terminalId"]
                    .as_str()
                    .ok_or_else(|| anyhow!("Missing terminal ID"))?;
                let terminal = self
                    .terminals
                    .lock()
                    .unwrap()
                    .get(id)
                    .cloned()
                    .ok_or_else(|| anyhow!("Unknown terminal in this ACP session"))?;
                match method {
                    "terminal/output" => {
                        let output = terminal.output.lock().unwrap();
                        Ok(serde_json::to_value(
                            TerminalOutputResponse::new(output.text(), output.truncated)
                                .exit_status(terminal.exit.borrow().clone()),
                        )?)
                    }
                    "terminal/wait_for_exit" => terminal.wait().await,
                    "terminal/kill" => {
                        terminal.kill().await?;
                        Ok(json!({}))
                    }
                    _ => {
                        terminal.kill().await?;
                        self.terminals.lock().unwrap().remove(id);
                        Ok(json!({}))
                    }
                }
            }
            _ => bail!("Unsupported ACP client method"),
        }
    }

    async fn create(&self, request: CreateTerminalRequest) -> Result<Value> {
        let cwd = request.cwd.as_deref().unwrap_or(&self.cwd);
        absolute(cwd)?;
        if request.command.is_empty() {
            bail!("Terminal command must not be empty");
        }
        let mut command = tokio::process::Command::new(request.command);
        command
            .args(request.args)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        crate::local::chat::prepare_env(&mut command);
        for variable in request.env {
            command.env(variable.name, variable.value);
        }
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn()?;
        let mut process = OwnedProcess {
            id: child.id(),
            child,
        };
        let output = Arc::new(Mutex::new(Output {
            bytes: Vec::new(),
            limit: request
                .output_byte_limit
                .unwrap_or(MAX_OUTPUT as u64)
                .min(MAX_OUTPUT as u64) as usize,
            truncated: false,
        }));
        let mut stdout = tokio::spawn(drain(
            process.child.stdout.take().expect("piped stdout"),
            output.clone(),
        ));
        let mut stderr = tokio::spawn(drain(
            process.child.stderr.take().expect("piped stderr"),
            output.clone(),
        ));
        let (kill, mut kill_receiver) = watch::channel(false);
        let (exit_sender, exit) = watch::channel(None);
        let id = uuid::Uuid::new_v4().to_string();
        self.terminals.lock().unwrap().insert(
            id.clone(),
            Arc::new(Terminal {
                output: output.clone(),
                kill,
                exit,
            }),
        );
        tokio::spawn(async move {
            let status = tokio::select! {
                status = process.child.wait() => status,
                _ = async { let _ = kill_receiver.wait_for(|kill| *kill).await; } => {
                    crate::local::chat::kill_shell_group(process.id.take());
                    let _ = process.child.kill().await;
                    process.child.wait().await
                }
            };
            crate::local::chat::kill_shell_group(process.id.take());
            // Detached descendants can retain the pipes after the owned process group exits.
            if tokio::time::timeout(std::time::Duration::from_millis(500), async {
                let _ = tokio::join!(&mut stdout, &mut stderr);
            })
            .await
            .is_err()
            {
                stdout.abort();
                stderr.abort();
                output.lock().unwrap().truncated = true;
            }
            let status = match status {
                Ok(status) => {
                    #[cfg(unix)]
                    let signal = {
                        use std::os::unix::process::ExitStatusExt;
                        status.signal().map(|signal| signal.to_string())
                    };
                    #[cfg(not(unix))]
                    let signal: Option<String> = None;
                    TerminalExitStatus::new()
                        .exit_code(status.code().map(|code| code as u32))
                        .signal(signal)
                }
                Err(error) => TerminalExitStatus::new().signal(error.to_string()),
            };
            let _ = exit_sender.send(Some(status));
        });
        Ok(json!({"terminalId": id}))
    }

    pub async fn shutdown(&self) {
        let terminals = std::mem::take(&mut *self.terminals.lock().unwrap());
        for terminal in terminals.into_values() {
            let _ = terminal.kill().await;
        }
    }
}

fn absolute(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        bail!("ACP file and working-directory paths must be absolute");
    }
    Ok(())
}

async fn drain(mut pipe: impl tokio::io::AsyncRead + Unpin, output: Arc<Mutex<Output>>) {
    let mut buffer = [0; 8192];
    while let Ok(count) = pipe.read(&mut buffer).await {
        if count == 0 {
            break;
        }
        output.lock().unwrap().append(&buffer[..count]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn terminal_arguments_environment_and_cancellation_are_preserved() {
        let delegated = Delegated::new(&std::env::temp_dir());
        let python = if cfg!(windows) { "python" } else { "python3" };
        let created = delegated.request("terminal/create", json!({"sessionId":"s", "command":python, "args":["-c", "import os,sys;print(sys.argv[1]);print(os.environ['ACP_TEST_VALUE']);sys.stdout.flush();import time;time.sleep(60)", "two words \"quoted\" $(untouched)"], "env":[{"name":"ACP_TEST_VALUE", "value":"native value"}]})).await.unwrap();
        let params = json!({"terminalId":created["terminalId"]});
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let output = delegated
                    .request("terminal/output", params.clone())
                    .await
                    .unwrap();
                if output["output"].as_str().unwrap().contains("native value") {
                    assert!(output.get("exitStatus").is_none());
                    assert!(output["output"]
                        .as_str()
                        .unwrap()
                        .contains("two words \"quoted\" $(untouched)"));
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let wait = delegated.request("terminal/wait_for_exit", params.clone());
        let kill = delegated.request("terminal/kill", params.clone());
        let (exit, killed) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(wait, kill)
        })
        .await
        .unwrap();
        assert_ne!(exit.unwrap()["exitCode"], 0);
        killed.unwrap();
        delegated
            .request("terminal/release", params.clone())
            .await
            .unwrap();
        assert!(delegated.request("terminal/output", params).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_release_reaps_descendants() {
        let delegated = Delegated::new(&std::env::temp_dir());
        let created = delegated.request("terminal/create", json!({"sessionId":"s", "command":"python3", "args":["-c", "import subprocess,sys,time; p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)']);print(p.pid,flush=True);time.sleep(60)"]})).await.unwrap();
        let params = json!({"terminalId":created["terminalId"]});
        let pid: i32 = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let output = delegated
                    .request("terminal/output", params.clone())
                    .await
                    .unwrap();
                if let Ok(pid) = output["output"].as_str().unwrap().trim().parse() {
                    break pid;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        delegated.request("terminal/release", params).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn detached_pipe_does_not_block_exit_or_release() {
        let delegated = Delegated::new(&std::env::temp_dir());
        let created = delegated.request("terminal/create", json!({"sessionId":"s", "command":"python3", "args":["-c", "import subprocess,sys; p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(5)'], start_new_session=True);print(p.pid,flush=True)"]})).await.unwrap();
        let params = json!({"terminalId": created["terminalId"]});
        let exit = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            delegated.request("terminal/wait_for_exit", params.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(exit["exitCode"], 0);
        let output = delegated
            .request("terminal/output", params.clone())
            .await
            .unwrap();
        assert_eq!(output["truncated"], true);
        let pid: u32 = output["output"].as_str().unwrap().trim().parse().unwrap();
        crate::local::chat::kill_shell_group(Some(pid));
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            delegated.request("terminal/release", params),
        )
        .await
        .unwrap()
        .unwrap();
        delegated.shutdown().await;
    }

    #[test]
    fn output_keeps_a_bounded_utf8_tail() {
        let mut output = Output {
            bytes: Vec::new(),
            limit: 5,
            truncated: false,
        };
        output.append("hello café".as_bytes());
        assert!(output.truncated);
        assert_eq!(output.text(), "café");
        output.append("世界".as_bytes());
        assert_eq!(output.text(), "界");
        output.limit = 0;
        output.append(b"anything");
        assert_eq!(output.text(), "");
    }
}
