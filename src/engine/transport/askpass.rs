use anyhow::{anyhow, bail, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::task::JoinHandle;

pub type Prompt = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Option<String>>> + Send>> + Send + Sync,
>;

pub(super) struct Askpass {
    endpoint: String,
    token: String,
    task: JoinHandle<()>,
    auth_prompted: Arc<AtomicBool>,
}

impl Askpass {
    pub async fn start(prompt: Prompt) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let endpoint = listener.local_addr()?.to_string();
        let token = uuid::Uuid::new_v4().to_string();
        let expected = token.clone();
        let auth_prompted = Arc::new(AtomicBool::new(false));
        let invoked = auth_prompted.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let mut stream = BufReader::new(stream);
                let exchange = async {
                    let mut line = String::new();
                    (&mut stream).take(16384).read_line(&mut line).await?;
                    let (token, question): (String, String) = serde_json::from_str(&line)?;
                    if token != expected {
                        bail!("Invalid askpass token");
                    }
                    let host_key = question.contains("continue connecting (yes/no")
                        || question.starts_with("Please type 'yes' or 'no':")
                        || question.starts_with("Please type 'yes', 'no' or the fingerprint:")
                        || question.starts_with("Accept updated hostkeys? (yes/no):");
                    if !host_key {
                        invoked.store(true, Ordering::SeqCst);
                    }
                    let answer = prompt(question).await?;
                    let bytes = serde_json::to_vec(&answer)?;
                    stream.get_mut().write_all(&bytes).await?;
                    stream.get_mut().write_all(b"\n").await?;
                    anyhow::Ok(())
                };
                let _ = tokio::time::timeout(std::time::Duration::from_secs(300), exchange).await;
            }
        });
        Ok(Self {
            endpoint,
            token,
            task,
            auth_prompted,
        })
    }

    pub fn auth_prompted(&self) -> bool {
        self.auth_prompted.load(Ordering::SeqCst)
    }

    pub fn configure(&self, command: &mut Command, executable: &std::path::Path) {
        command
            .env("SSH_ASKPASS", executable)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "orx-engine")
            .env("ORX_ENGINE_ASKPASS", &self.endpoint)
            .env("ORX_ENGINE_ASKPASS_TOKEN", &self.token)
            .stdin(std::process::Stdio::null());
        // Askpass is a console program: a detached ssh would open a window for it. Batch
        // logins stay detached, so a ProxyJump hop fails instead of waiting on a hidden console.
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
}

impl Drop for Askpass {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn relay() -> Result<()> {
    let stream = TcpStream::connect(std::env::var("ORX_ENGINE_ASKPASS")?).await?;
    let mut stream = BufReader::new(stream);
    let token = std::env::var("ORX_ENGINE_ASKPASS_TOKEN")?;
    let question = std::env::args().nth(1).unwrap_or_default();
    let bytes = serde_json::to_vec(&(token, question))?;
    stream.get_mut().write_all(&bytes).await?;
    stream.get_mut().write_all(b"\n").await?;
    let mut line = String::new();
    (&mut stream).take(16384).read_line(&mut line).await?;
    let answer: Option<String> = serde_json::from_str(&line)?;
    let answer = answer.ok_or_else(|| anyhow!("SSH authentication cancelled"))?;
    if answer.contains(['\n', '\r']) {
        bail!("Invalid multiline SSH answer");
    }
    println!("{answer}");
    Ok(())
}
