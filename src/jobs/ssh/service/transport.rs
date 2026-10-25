use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch, Mutex, Semaphore};

use super::{state, wire, Endpoint};
use crate::error::{anyhow, Result};

#[cfg(windows)]
struct AskpassBridge(std::path::PathBuf);
#[cfg(windows)]
impl Drop for AskpassBridge {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

pub(super) struct Session {
    writer: mpsc::Sender<Value>,
    pending: Pending,
    sequence: AtomicU64,
    alive: watch::Sender<bool>,
    since: Instant,
    finished: watch::Receiver<bool>,
    lanes: [Semaphore; 5],
}

impl Session {
    pub fn alive(&self) -> bool {
        *self.alive.borrow()
    }
    pub fn stop(&self) {
        self.alive.send_replace(false);
    }
    pub async fn stop_and_wait(&self) -> Result<()> {
        self.stop();
        let mut finished = self.finished.clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            while !*finished.borrow_and_update() {
                finished.changed().await?;
            }
            Ok::<_, crate::error::Error>(())
        })
        .await??;
        Ok(())
    }

    pub fn stable(&self) -> bool {
        self.alive() && self.since.elapsed() >= Duration::from_secs(60)
    }

    pub async fn request(&self, mut request: Value) -> Result<Value> {
        if !self.alive() {
            return Err(anyhow!(
                "SSH connection is disconnected; use Connect to authenticate again."
            ));
        }
        let lane = match request["op"].as_str() {
            Some("ping") => 4,
            Some("log") => 1,
            Some("slurm") => 2,
            _ => match request["lane"].as_str() {
                Some("control") => 0,
                Some("status") => 2,
                _ => 3,
            },
        };
        let _permit = self.lanes[lane].acquire().await?;
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        request["id"] = json!(id);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let long_running =
            lane == 3 && matches!(request["op"].as_str(), Some("exec" | "upload_commit"));
        let response = async {
            self.writer
                .send(request)
                .await
                .map_err(|_| anyhow!("SSH helper writer closed"))?;
            rx.await.map_err(|_| {
                anyhow!("SSH helper response lost; command outcome may be uncertain")
            })?
        };
        let result = if long_running {
            Ok(response.await)
        } else {
            tokio::time::timeout(Duration::from_secs(130), response).await
        };
        self.pending.lock().await.remove(&id);
        match result {
            Ok(result) => result,
            Err(_) => {
                self.stop();
                Err(anyhow!(
                    "SSH helper timed out; command outcome may be uncertain"
                ))
            }
        }
    }
}

pub(super) async fn connect(
    resolved: &state::Resolved,
    interactive: Option<(&Endpoint, &str)>,
) -> Result<Arc<Session>> {
    state::validate_configuration(resolved).await?;
    #[cfg(windows)]
    let mut _bridge = None;
    let mut command = tokio::process::Command::new("ssh");
    command.env("ORX_SSH_PROBE", "1");
    if let Some(agent) = &resolved.target.auth_agent {
        command.env("SSH_AUTH_SOCK", agent);
    } else {
        command.env_remove("SSH_AUTH_SOCK");
    }
    command.args([
        "-T",
        "-oClearAllForwardings=yes",
        "-oRemoteCommand=none",
        "-oControlMaster=no",
        "-oControlPersist=no",
    ]);
    let managed = resolved.target.managed_generation.is_some();
    if managed {
        command.args(["-oBatchMode=yes", "-oControlPath=none"]);
    } else {
        #[cfg(unix)]
        {
            command
                .args(["-oBatchMode=yes", "-oProxyCommand=false", "-S"])
                .arg(
                    resolved
                        .socket
                        .as_ref()
                        .ok_or_else(|| anyhow!("Missing pinned SSH socket"))?,
                );
        }
        #[cfg(windows)]
        {
            let (endpoint, attempt) = interactive.ok_or_else(|| {
                anyhow!(
                    "SSH disconnected. Use Connect to authenticate; background login is disabled."
                )
            })?;
            let path = state::root().join(format!("auth-{attempt}.json"));
            crate::local::git::atomic_write_with_mode(
                &path,
                &serde_json::to_vec(&json!({"endpoint":endpoint, "attempt":attempt}))?,
                Some(0o600),
            )?;
            _bridge = Some(AskpassBridge(path.clone()));
            command
                .args(["-oBatchMode=no", "-oControlPath=none"])
                .env("SSH_ASKPASS", crate::paths::spawnable_exe()?)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", "orx-ssh-connect")
                .env("ORX_SSH_ASKPASS_ENDPOINT", path);
        }
    }
    #[cfg(not(windows))]
    let _ = interactive;
    let source = include_str!("../helper.py");
    let bootstrap = format!(
        "import sys; exec(compile(sys.stdin.buffer.read({}), '<orx-ssh-helper>', 'exec'))",
        source.len()
    );
    command
        .args(&resolved.options)
        .args(["--", &resolved.target.dest])
        .arg(format!(
            "python3 -u -c {}",
            super::super::sh_quote(&bootstrap)
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    let mut child = command.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Missing SSH stdin"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Missing SSH stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("Missing SSH stderr"))?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let diagnostic_writer = diagnostics.clone();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut chunk = [0; 4096];
        while let Ok(count) = stderr.read(&mut chunk).await {
            if count == 0 {
                break;
            }
            let mut tail = diagnostic_writer.lock().await;
            tail.extend_from_slice(&chunk[..count]);
            if tail.len() > 16384 {
                let end = tail.len() - 16384;
                tail.drain(..end);
            }
        }
    });
    let handshake = tokio::time::timeout(
        Duration::from_secs(if interactive.is_some() { 300 } else { 20 }),
        async {
            stdin.write_all(source.as_bytes()).await?;
            stdin.flush().await?;
            let mut banner = Vec::new();
            loop {
                banner.push(stdout.read_u8().await?);
                if banner.ends_with(b"__ORX_SSH_HELPER_V1__\n") {
                    break;
                }
                if banner.len() > 16384 {
                    return Err(anyhow!("SSH startup output exceeds 16 KiB"));
                }
            }
            let hello: Value = wire::read(&mut stdout).await?;
            if hello["version"] != 1 {
                return Err(anyhow!("Unsupported SSH helper protocol"));
            }
            Ok::<_, crate::error::Error>(())
        },
    )
    .await;
    if !matches!(handshake, Ok(Ok(()))) {
        let _ = child.kill().await;
        let detail = String::from_utf8_lossy(&diagnostics.lock().await)
            .trim()
            .to_owned();
        return Err(anyhow!("SSH helper did not start. Python 3.6+ is required. Use Connect if authentication expired. {detail}"));
    }
    let (writer, mut queue) = mpsc::channel(32);
    let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
    let (alive, mut death) = watch::channel(true);
    let (finished_tx, finished) = watch::channel(false);
    let session = Arc::new(Session {
        writer,
        pending: pending.clone(),
        sequence: AtomicU64::new(1),
        alive: alive.clone(),
        since: Instant::now(),
        finished,
        lanes: [
            Semaphore::new(2),
            Semaphore::new(32),
            Semaphore::new(2),
            Semaphore::new(4),
            Semaphore::new(1),
        ],
    });
    let writer_alive = alive.clone();
    let mut writer_death = alive.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_death.changed() => break,
                request = queue.recv() => {
                    let Some(request) = request else {break};
                    if wire::write(&mut stdin, &request).await.is_err() { break; }
                }
            }
        }
        writer_alive.send_replace(false);
    });
    let reader_alive = alive.clone();
    let mut reader_death = alive.subscribe();
    tokio::spawn(async move {
        loop {
            let response: Result<Value> = tokio::select! {
                _ = reader_death.changed() => break,
                response = wire::read(&mut stdout) => response,
            };
            let Ok(response) = response else { break };
            let Some(id) = response["id"].as_u64() else {
                break;
            };
            if let Some(reply) = pending.lock().await.remove(&id) {
                let result = if let Some(error) = response["error"].as_str() {
                    Err(anyhow!("Remote helper operation failed: {error}"))
                } else {
                    Ok(response["result"].clone())
                };
                let _ = reply.send(result);
            }
        }
        reader_alive.send_replace(false);
        pending.lock().await.clear();
    });
    let key = resolved.key.clone();
    let heartbeat_session = Arc::downgrade(&session);
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = death.changed() => break,
                _ = child.wait() => break,
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    let Some(session) = heartbeat_session.upgrade() else {break};
                    if !tokio::time::timeout(Duration::from_secs(10), session.request(json!({"op":"ping"}))).await.is_ok_and(|result| result.is_ok_and(|value| value["version"] == 1)) {break;}
                    if session.stable() { let _ = state::healthy(&key); }
                }
            }
        }
        alive.send_replace(false);
        let _ = child.kill().await;
        let detail = String::from_utf8_lossy(&diagnostics.lock().await)
            .trim()
            .to_owned();
        let _ = state::failure(&key, &format!("SSH transport closed. {detail}"));
        finished_tx.send_replace(true);
    });
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dead_session_cannot_be_replaced_before_teardown_finishes() {
        let (alive, _) = watch::channel(true);
        let (finished_tx, finished) = watch::channel(false);
        let (writer, _queue) = mpsc::channel(1);
        let session = Arc::new(Session {
            writer,
            pending: Arc::new(Mutex::new(HashMap::new())),
            sequence: AtomicU64::new(1),
            alive: alive.clone(),
            since: Instant::now(),
            finished,
            lanes: std::array::from_fn(|_| Semaphore::new(1)),
        });
        let host = super::super::Host {
            resolved: serde_json::from_value(json!({
                "key":"test", "key_version":1,
                "target":crate::jobs::ssh::SshTarget::alias("test"),
                "options":[], "socket":null,
            }))
            .unwrap(),
            session: Some(session.clone()),
        };
        assert!(Arc::ptr_eq(
            &host.reusable_session().await.unwrap().unwrap(),
            &session
        ));
        alive.send_replace(false);
        let mut replacement = Box::pin(host.reusable_session());
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut replacement)
                .await
                .is_err()
        );
        finished_tx.send_replace(true);
        assert!(replacement.await.unwrap().is_none());
    }
}
