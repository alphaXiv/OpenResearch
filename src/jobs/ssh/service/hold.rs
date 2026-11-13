//! Windows OpenSSH has no ControlMaster, so the engine's authenticated `ssh` to an orx-host
//! root runs here instead of in `orx up`. A restarted server attaches to it again; it ends when
//! the user signs out, Connect replaces it, or no server has attached for `IDLE`.
//!
//! `hold_start` spawns it and splices the unframed bootstrap until the engine hangs up after
//! `ORX1 ready`; each `hold_attach` then carries whole orx-host frames, so a server that dies
//! mid-frame never leaves a partial frame on the session.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{mpsc, watch, Mutex};
use tokio::task::JoinHandle;

use super::{wire, Response};
use crate::error::{anyhow, Result};

const IDLE: Duration = Duration::from_secs(24 * 60 * 60);

pub(super) type Registry = Mutex<HashMap<String, Arc<Held>>>;

pub(super) struct Held {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    /// Read raw by the bootstrap, then by the frame pump the first attach starts once the
    /// bootstrap lets go of it.
    stdout: Mutex<Option<ChildStdout>>,
    sink: std::sync::Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    attachment: watch::Sender<u64>,
    detached: std::sync::Mutex<Option<Instant>>,
    stderr: Arc<std::sync::Mutex<Vec<u8>>>,
    /// Whether the target, not a jump host, accepted the login.
    authenticated: Arc<AtomicBool>,
    stderr_reader: std::sync::Mutex<Option<JoinHandle<()>>>,
}

pub(super) async fn serve(registry: &Registry, operation: &Value, mut stream: TcpStream) {
    let key = operation["key"].as_str().unwrap_or_default();
    let result = match operation["op"].as_str() {
        Some("hold_start") => start(registry, key, operation)
            .await
            .map(|held| (json!({}), Some((held, true)))),
        Some("hold_attach") => attach(registry, key)
            .await
            .map(|held| (json!({}), Some((held, false)))),
        _ => stop(registry, key).await.map(|value| (value, None)),
    };
    let (reply, held) = match result {
        Ok((value, held)) => (
            Response {
                value: Some(value),
                error: None,
            },
            held,
        ),
        Err(error) => (
            Response {
                value: None,
                error: Some(error.to_string()),
            },
            None,
        ),
    };
    if wire::write(&mut stream, &reply).await.is_err() {
        return;
    }
    match held {
        Some((held, true)) => bootstrap(&held, stream).await,
        Some((held, false)) => relay(&held, stream).await,
        None => {}
    }
}

/// Ends logins whose `ssh` exited, or that no server has attached to for `IDLE`.
pub(super) async fn sweep(registry: &Registry) {
    loop {
        tokio::time::sleep(crate::config::paced(Duration::from_secs(60))).await;
        let mut held = registry.lock().await;
        let mut expired = Vec::new();
        for (key, session) in held.iter() {
            let idle = session
                .detached
                .lock()
                .unwrap()
                .is_some_and(|since| since.elapsed() >= crate::config::paced(IDLE));
            if idle
                || session
                    .child
                    .lock()
                    .await
                    .try_wait()
                    .map_or(true, |exit| exit.is_some())
            {
                expired.push(key.clone());
            }
        }
        for key in expired {
            if let Some(session) = held.remove(&key) {
                let _ = session.child.lock().await.start_kill();
            }
        }
    }
}

async fn start(registry: &Registry, key: &str, operation: &Value) -> Result<Arc<Held>> {
    let held = {
        let mut command = crate::process::tokio_command("ssh");
        for arg in operation["args"].as_array().into_iter().flatten() {
            command.arg(
                arg.as_str()
                    .ok_or_else(|| anyhow!("Invalid ssh argument"))?,
            );
        }
        for pair in operation["env"].as_array().into_iter().flatten() {
            let name = pair[0]
                .as_str()
                .ok_or_else(|| anyhow!("Invalid ssh environment"))?;
            match pair[1].as_str() {
                Some(value) => command.env(name, value),
                None => command.env_remove(name),
            };
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Missing ssh stdin"))?;
        let stdout = child.stdout.take();
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("Missing ssh stderr"))?;
        let prefix = operation["prefix"].as_str().unwrap_or_default().to_owned();
        let (stderr_reader, authenticated, stderr) =
            crate::engine::transport::read_stderr(stderr, prefix);
        Arc::new(Held {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(stdout),
            sink: std::sync::Mutex::new(None),
            attachment: watch::channel(0).0,
            detached: std::sync::Mutex::new(Some(Instant::now())),
            stderr,
            authenticated,
            stderr_reader: std::sync::Mutex::new(Some(stderr_reader)),
        })
    };
    if let Some(previous) = registry.lock().await.insert(key.to_owned(), held.clone()) {
        let _ = previous.child.lock().await.start_kill();
    }
    Ok(held)
}

async fn attach(registry: &Registry, key: &str) -> Result<Arc<Held>> {
    let held = registry
        .lock()
        .await
        .get(key)
        .cloned()
        .ok_or_else(|| anyhow!("No SSH login is held for this host"))?;
    if !matches!(held.child.lock().await.try_wait(), Ok(None)) {
        registry.lock().await.remove(key);
        return Err(anyhow!("The held SSH login ended"));
    }
    Ok(held)
}

async fn stop(registry: &Registry, key: &str) -> Result<Value> {
    let Some(held) = registry.lock().await.remove(key) else {
        return Ok(json!({"code": null, "stderr": ""}));
    };
    let mut child = held.child.lock().await;
    let _ = child.start_kill();
    let code = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .ok()
        .and_then(|status| status.ok()?.code());
    let reader = held.stderr_reader.lock().unwrap().take();
    if let Some(reader) = reader {
        let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    }
    let stderr = String::from_utf8_lossy(&held.stderr.lock().unwrap()).into_owned();
    let authenticated = held.authenticated.load(Ordering::Relaxed);
    Ok(json!({"code": code, "stderr": stderr, "authenticated": authenticated}))
}

async fn bootstrap(held: &Held, mut stream: TcpStream) {
    let mut stdout = held.stdout.lock().await;
    let Some(stdout) = stdout.as_mut() else {
        return;
    };
    let mut stdin = held.stdin.lock().await;
    let (mut from_server, mut to_server) = stream.split();
    // The engine hangs up once `ORX1 ready` arrives, before orx-host has more to say.
    tokio::select! {
        _ = tokio::io::copy(&mut from_server, &mut *stdin) => {}
        _ = tokio::io::copy(stdout, &mut to_server) => {}
    }
    *held.detached.lock().unwrap() = Some(Instant::now());
}

async fn relay(held: &Arc<Held>, stream: TcpStream) {
    if let Some(stdout) = held.stdout.lock().await.take() {
        tokio::spawn(pump(held.clone(), stdout));
    }
    let (sender, mut receiver) = mpsc::channel(16);
    *held.sink.lock().unwrap() = Some(sender);
    held.attachment.send_modify(|generation| *generation += 1);
    let generation = *held.attachment.borrow();
    let mut superseded = held.attachment.subscribe();
    *held.detached.lock().unwrap() = None;

    let (mut from_server, mut to_server) = stream.into_split();
    let writer = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            if to_server.write_all(&frame).await.is_err() {
                break;
            }
        }
    });
    loop {
        let frame = tokio::select! {
            frame = read_frame(&mut from_server) => frame,
            _ = superseded.changed() => break,
        };
        let Ok(frame) = frame else { break };
        let mut stdin = held.stdin.lock().await;
        if stdin.write_all(&frame).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
    writer.abort();
    if *held.attachment.borrow() == generation {
        *held.sink.lock().unwrap() = None;
        *held.detached.lock().unwrap() = Some(Instant::now());
    }
}

/// Forwards whole frames to the attached server; while none is attached, replies to a server
/// that went away are dropped.
async fn pump(held: Arc<Held>, mut stdout: ChildStdout) {
    while let Ok(frame) = read_frame(&mut stdout).await {
        let sink = held.sink.lock().unwrap().clone();
        if let Some(sink) = sink {
            let _ = sink.send(frame).await;
        }
    }
    // Ends the attached server's stream, so it sees the login end at once.
    held.sink.lock().unwrap().take();
}

/// One length-prefixed orx-host frame, prefix included.
async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>> {
    let mut frame = vec![0; 4];
    reader.read_exact(&mut frame).await?;
    let len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    if len == 0 || len > orx_host_proto::MAX_FRAME_LEN {
        return Err(anyhow!("Invalid orx-host frame length"));
    }
    frame.resize(4 + len, 0);
    reader.read_exact(&mut frame[4..]).await?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn a_server_resumes_the_login_after_the_last_one_died_mid_frame() {
        let mut child = crate::process::tokio_command("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let held = Arc::new(Held {
            stdin: Mutex::new(child.stdin.take().unwrap()),
            stdout: Mutex::new(child.stdout.take()),
            child: Mutex::new(child),
            sink: std::sync::Mutex::new(None),
            attachment: watch::channel(0).0,
            detached: std::sync::Mutex::new(None),
            stderr: Arc::default(),
            authenticated: Arc::default(),
            stderr_reader: std::sync::Mutex::default(),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attach = async || {
            let client = TcpStream::connect(address).await.unwrap();
            let (server, _) = listener.accept().await.unwrap();
            let held = held.clone();
            (
                client,
                tokio::spawn(async move { relay(&held, server).await }),
            )
        };
        let mut echo = [0; 5];

        let (mut first, relayed) = attach().await;
        first.write_all(b"\0\0\0\x01a").await.unwrap();
        first.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"\0\0\0\x01a");
        first.write_all(b"\0\0\0\x04x").await.unwrap();
        drop(first);
        relayed.await.unwrap();
        assert!(held.detached.lock().unwrap().is_some());

        let (mut second, _relayed) = attach().await;
        second.write_all(b"\0\0\0\x01b").await.unwrap();
        second.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"\0\0\0\x01b");
    }

    /// Starts a login whose `ssh` is `script`, kept in the returned directory.
    #[cfg(unix)]
    async fn start_fake(registry: &Registry, script: &str) -> (Arc<Held>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("orx-hold-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        orx_test_support::write_executable(
            dir.join("ssh"),
            format!("#!/bin/sh\n{script}\n"),
            0o755,
        )
        .unwrap();
        let path = format!("{}:{}", dir.display(), std::env::var("PATH").unwrap());
        let operation = json!({
            "env": [["PATH", path]],
            "prefix": "Authenticated to lab.example (",
        });
        (start(registry, "key", &operation).await.unwrap(), dir)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_login_no_server_has_attached_to_can_expire() {
        let registry = Registry::default();
        let (held, dir) = start_fake(&registry, "exec sleep 30").await;
        assert!(held.detached.lock().unwrap().is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_target_login_is_reported_after_its_line_leaves_the_stderr_tail() {
        let registry = Registry::default();
        let (held, dir) = start_fake(
            &registry,
            "echo 'Authenticated to lab.example ([10.0.0.1]:22) using \"password\".' >&2\n\
             head -c 20000 /dev/zero | tr '\\0' x >&2\n\
             echo end >&2\n\
             exit 97",
        )
        .await;
        held.child.lock().await.wait().await.unwrap();

        let ended = stop(&registry, "key").await.unwrap();
        let stderr = ended["stderr"].as_str().unwrap();
        assert!(stderr.ends_with("end\n") && !stderr.contains("Authenticated"));
        assert_eq!(ended["authenticated"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn frames_split_across_reads_arrive_whole_and_a_bad_length_stops() {
        let (mut writer, mut reader) = tokio::io::duplex(64);
        let task = tokio::spawn(async move {
            writer.write_all(&[0, 0, 0, 3, b'a']).await.unwrap();
            tokio::task::yield_now().await;
            writer.write_all(b"bc").await.unwrap();
            writer.write_all(&[0, 0, 0, 0]).await.unwrap();
        });
        assert_eq!(read_frame(&mut reader).await.unwrap(), b"\0\0\0\x03abc");
        assert!(read_frame(&mut reader).await.is_err());
        task.await.unwrap();
    }
}
