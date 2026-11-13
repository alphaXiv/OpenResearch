//! Persistent SSH remote host and its private same-user control channel.

// Windows compiles out the Unix-socket half; what only it reaches stays ungated, since the
// half returns intact once the channel has a Windows transport.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::AtomicBool;
#[cfg(unix)]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
#[cfg(unix)]
use tokio::io::AsyncWriteExt as _;
use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::watch;

use crate::error::{anyhow, Result};
use crate::local::chat::ChatHost;
use crate::store::Store;
use crate::{RemoteHostArgs, RemoteHostCommand};

pub(crate) const CONTROL_PROTOCOL: u32 = 1;
pub(crate) const HOST_MARKER: &str = "ORX_REMOTE_HOST=";
pub(crate) const ATTACHED_MARKER: &str = "ORX_REMOTE_ATTACHED=1";

const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const ATTACHMENT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CONTROL_LINE: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HostDescriptor {
    pub instance_id: String,
    pub hostname: String,
    pub port: u16,
    pub version: String,
    /// Absent from hosts that predate it.
    #[serde(default)]
    pub build_id: Option<String>,
    pub dashboard_protocol: u32,
    pub control_protocol: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopPreview {
    pub active_turn_count: usize,
    pub queued_message_count: usize,
    pub pending_permission_count: usize,
    pub active_run_count: usize,
    pub attachment_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopRequest {
    pub expected_instance_id: String,
    pub expected_preview: StopPreview,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum ControlRequest {
    Status,
    Attach {
        expected_instance_id: String,
        token: String,
    },
    Stop {
        expected_instance_id: String,
        expected_preview: StopPreview,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub(crate) enum ControlResponse {
    Status {
        descriptor: HostDescriptor,
        preview: StopPreview,
    },
    Stopping {
        descriptor: HostDescriptor,
    },
    Attached {
        descriptor: HostDescriptor,
    },
    Conflict {
        error: String,
        descriptor: HostDescriptor,
        preview: StopPreview,
    },
    Accepted,
    Error {
        error: String,
    },
}

#[derive(Clone)]
pub(crate) struct RemoteAuth {
    attachments: Arc<RwLock<HashSet<[u8; 32]>>>,
    callback: [u8; 32],
}

impl RemoteAuth {
    pub(crate) fn new(callback: &str) -> Self {
        Self {
            attachments: Arc::new(RwLock::new(HashSet::new())),
            callback: digest(callback),
        }
    }

    pub(crate) fn register(&self, token: &str) {
        self.attachments.write().unwrap().insert(digest(token));
    }

    pub(crate) fn unregister(&self, token: &str) {
        self.attachments.write().unwrap().remove(&digest(token));
    }

    pub(crate) fn matches_attachment(&self, provided: &[u8]) -> bool {
        self.attachments.read().unwrap().contains(provided)
    }

    pub(crate) fn matches_callback(&self, provided: &[u8]) -> bool {
        constant_time_eq(provided, &self.callback)
    }

    pub(crate) fn attachment_count(&self) -> usize {
        self.attachments.read().unwrap().len()
    }
}

fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

pub(crate) struct DashboardLock {
    release: Option<std::sync::mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for DashboardLock {
    fn drop(&mut self) {
        self.release.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl DashboardLock {
    /// The engine lock: exclusive, so one server owns the data directory's runs.
    pub(crate) fn acquire(data_dir: &Path) -> Result<Self> {
        let path = shared_path(data_dir, "lock")?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut lock = open_lock(&path).map_err(|error| {
                    anyhow!("Could not open dashboard lock {}: {error}", path.display())
                })?;
                let _guard = lock.try_write().map_err(|error| {
                    if is_lock_conflict(&error) {
                        anyhow!("Another OpenResearch dashboard is already using this database.")
                    } else {
                        lock_error(&path, error)
                    }
                })?;
                ready_tx.send(Ok(())).ok();
                let _ = release_rx.recv();
                Ok(())
            })();
            if let Err(error) = result {
                let _ = ready_tx.send(Err(error.to_string()));
            }
        });
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self {
                release: Some(release_tx),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(anyhow!(error))
            }
            Err(_) => {
                let _ = thread.join();
                Err(anyhow!("dashboard lock thread exited"))
            }
        }
    }
}

pub(crate) struct ControlServer {
    task: tokio::task::JoinHandle<()>,
    socket_path: PathBuf,
    descriptor_path: PathBuf,
    instance_id: String,
}

impl ControlServer {
    pub(crate) async fn shutdown(self) {
        let Self {
            task,
            socket_path,
            descriptor_path,
            instance_id,
        } = self;
        task.abort();
        let _ = task.await;
        let _ = std::fs::remove_file(socket_path);
        if read_descriptor(&descriptor_path)
            .is_some_and(|descriptor| descriptor.instance_id == instance_id)
        {
            let _ = std::fs::remove_file(descriptor_path);
        }
    }
}

/// No Unix sockets on Windows: refuse `--remote-host` rather than start a host nothing can reach.
#[cfg(not(unix))]
pub(crate) async fn start_control_server(
    _descriptor: HostDescriptor,
    _auth: RemoteAuth,
    _chat: Arc<ChatHost>,
    _stopping: Arc<AtomicBool>,
    _stop: watch::Sender<bool>,
) -> Result<ControlServer> {
    Err(anyhow!(
        "Running as a persistent remote host needs a Unix domain socket for its control \
         channel, which Windows does not provide."
    ))
}

#[cfg(unix)]
pub(crate) async fn start_control_server(
    descriptor: HostDescriptor,
    auth: RemoteAuth,
    chat: Arc<ChatHost>,
    stopping: Arc<AtomicBool>,
    stop: watch::Sender<bool>,
) -> Result<ControlServer> {
    let data_dir = canonical_data_dir()?;
    let socket_path = control_socket_path(&data_dir)?;
    if let Ok(metadata) = std::fs::symlink_metadata(&socket_path) {
        if metadata.file_type().is_symlink() || metadata_uid(&metadata) != effective_uid() {
            return Err(anyhow!(
                "Refusing to replace an unsafe remote-host control socket."
            ));
        }
        std::fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    set_mode(&socket_path, 0o600)?;
    let descriptor_path = descriptor_path(&data_dir)?;
    write_descriptor(&descriptor_path, &descriptor)?;
    let instance_id = descriptor.instance_id.clone();
    let task_descriptor = descriptor.clone();
    let control_gate = Arc::new(tokio::sync::Mutex::new(()));
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(connection) => connection,
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            if stream
                .peer_cred()
                .map(|credentials| credentials.uid() != effective_uid())
                .unwrap_or(true)
            {
                continue;
            }
            let descriptor = task_descriptor.clone();
            let auth = auth.clone();
            let chat = chat.clone();
            let stopping = stopping.clone();
            let stop = stop.clone();
            let control_gate = control_gate.clone();
            tokio::spawn(async move {
                let _ =
                    handle_control(stream, descriptor, auth, chat, stopping, stop, control_gate)
                        .await;
            });
        }
    });
    Ok(ControlServer {
        task,
        socket_path,
        descriptor_path,
        instance_id,
    })
}

#[cfg(unix)]
async fn handle_control(
    stream: UnixStream,
    descriptor: HostDescriptor,
    auth: RemoteAuth,
    chat: Arc<ChatHost>,
    stopping: Arc<AtomicBool>,
    stop: watch::Sender<bool>,
    control_gate: Arc<tokio::sync::Mutex<()>>,
) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let line = tokio::time::timeout(CONTROL_TIMEOUT, read_bounded_line(&mut reader))
        .await
        .map_err(|_| anyhow!("Timed out reading a remote-host control message."))??;
    let request: ControlRequest = serde_json::from_str(&line)?;
    match request {
        ControlRequest::Status => {
            if stopping.load(Ordering::SeqCst) {
                write_response(reader.get_mut(), &ControlResponse::Stopping { descriptor }).await?;
                return Ok(());
            }
            let preview = stop_preview(&chat, &auth).await;
            write_response(
                reader.get_mut(),
                &ControlResponse::Status {
                    descriptor,
                    preview,
                },
            )
            .await?;
        }
        ControlRequest::Attach {
            expected_instance_id,
            token,
        } => {
            let _control = control_gate.lock().await;
            if stopping.load(Ordering::SeqCst) {
                write_response(
                    reader.get_mut(),
                    &ControlResponse::Error {
                        error: "The remote OpenResearch host is stopping.".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            if expected_instance_id != descriptor.instance_id {
                write_response(
                    reader.get_mut(),
                    &ControlResponse::Error {
                        error: "The remote OpenResearch host changed. Retry the connection.".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            validate_token(&token)?;
            auth.register(&token);
            if let Err(error) =
                write_response(reader.get_mut(), &ControlResponse::Attached { descriptor }).await
            {
                auth.unregister(&token);
                return Err(error);
            }
            drop(_control);
            let result = async {
                loop {
                    let line =
                        tokio::time::timeout(ATTACHMENT_TIMEOUT, read_bounded_line(&mut reader))
                            .await
                            .map_err(|_| anyhow!("Remote attachment heartbeat expired."))??;
                    if line != "ping" {
                        return Err(anyhow!("Invalid remote attachment heartbeat."));
                    }
                }
            }
            .await;
            auth.unregister(&token);
            result?;
        }
        ControlRequest::Stop {
            expected_instance_id,
            expected_preview,
        } => {
            let _control = control_gate.lock().await;
            if stopping.load(Ordering::SeqCst) {
                write_response(
                    reader.get_mut(),
                    &ControlResponse::Error {
                        error: "The remote OpenResearch host is already stopping.".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            let preview = stop_preview(&chat, &auth).await;
            if expected_instance_id != descriptor.instance_id || expected_preview != preview {
                write_response(
                    reader.get_mut(),
                    &ControlResponse::Conflict {
                        error: "Remote activity changed. Review the current work before stopping."
                            .into(),
                        descriptor,
                        preview,
                    },
                )
                .await?;
                return Ok(());
            }
            if stopping
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                write_response(
                    reader.get_mut(),
                    &ControlResponse::Error {
                        error: "The remote OpenResearch host is already stopping.".into(),
                    },
                )
                .await?;
                return Ok(());
            }
            let response = write_response(reader.get_mut(), &ControlResponse::Accepted).await;
            let _ = stop.send(true);
            response?;
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn stop_preview(chat: &ChatHost, auth: &RemoteAuth) -> StopPreview {
    StopPreview {
        active_turn_count: chat.busy_sessions().await.len(),
        queued_message_count: chat.queued_count(),
        pending_permission_count: chat.pending_permission_count(),
        active_run_count: Store::open()
            .and_then(|store| store.count_active_runs())
            .unwrap_or(0),
        attachment_count: auth.attachment_count(),
    }
}

/// The whole `orx remote-host` surface talks over the control socket.
#[cfg(not(unix))]
pub(crate) async fn run(_args: RemoteHostArgs) -> Result<()> {
    Err(anyhow!(
        "`orx remote-host` needs a Unix domain socket for its control channel, which Windows \
         does not provide."
    ))
}

#[cfg(unix)]
pub(crate) async fn run(args: RemoteHostArgs) -> Result<()> {
    match args.command {
        RemoteHostCommand::Ensure { expected_instance } => ensure(expected_instance).await,
        RemoteHostCommand::Status => print_status().await,
        RemoteHostCommand::Attach { expected_instance } => attach(&expected_instance).await,
        RemoteHostCommand::Stop => stop().await,
    }
}

#[cfg(unix)]
async fn ensure(expected_instance: Option<String>) -> Result<()> {
    let data_dir = canonical_data_dir()?;
    match live_descriptor(&data_dir).await {
        LiveHost::Running(descriptor) => {
            verify_expected(&descriptor, expected_instance.as_deref())?;
            return print_descriptor(&descriptor);
        }
        LiveHost::Stopping(descriptor) if expected_instance.is_some() => {
            verify_expected(&descriptor, expected_instance.as_deref())?;
            return Err(anyhow!(
                "The expected OpenResearch host is stopping. Retry after it exits."
            ));
        }
        LiveHost::Stopping(_) | LiveHost::Missing => {}
    }
    if expected_instance.is_some() {
        ensure_server_lock_free(&data_dir)?;
        return Err(anyhow!(
            "The expected OpenResearch host is no longer running. Start a new host explicitly."
        ));
    }

    let started = ensure_started(
        data_dir.clone(),
        START_TIMEOUT,
        ServerMode::Persistent,
        vec![
            "up".into(),
            "--no-browser".into(),
            "--remote-host".into(),
            "--port".into(),
            "0".into(),
        ],
        move || {
            let data_dir = data_dir.clone();
            async move {
                match live_descriptor(&data_dir).await {
                    LiveHost::Running(descriptor) => Ok(Some(descriptor)),
                    LiveHost::Stopping(_) => {
                        wait_for_server_lock_free(&data_dir).await?;
                        Ok(None)
                    }
                    LiveHost::Missing => Ok(None),
                }
            }
        },
    )
    .await?;
    print_descriptor(&started)
}

#[cfg(unix)]
async fn print_status() -> Result<()> {
    let data_dir = canonical_data_dir()?;
    let response = control_exchange(&data_dir, &ControlRequest::Status).await?;
    if !server_lock_is_held(&data_dir)? {
        return Err(anyhow!("The remote-host descriptor is stale."));
    }
    println!("{HOST_MARKER}{}", serde_json::to_string(&response)?);
    Ok(())
}

#[cfg(unix)]
async fn attach(expected_instance: &str) -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin());
    let token = tokio::time::timeout(Duration::from_secs(10), read_bounded_line(&mut input))
        .await
        .map_err(|_| anyhow!("Timed out waiting for the remote attachment credential."))??;
    validate_token(&token)?;
    let data_dir = canonical_data_dir()?;
    let mut stream = UnixStream::connect(control_socket_path(&data_dir)?).await?;
    write_request(
        &mut stream,
        &ControlRequest::Attach {
            expected_instance_id: expected_instance.to_string(),
            token,
        },
    )
    .await?;
    let mut reader = BufReader::new(stream);
    let response: ControlResponse = serde_json::from_str(&read_bounded_line(&mut reader).await?)?;
    match response {
        ControlResponse::Attached { .. } => {
            let mut stdout = tokio::io::stdout();
            stdout
                .write_all(format!("{ATTACHED_MARKER}\n").as_bytes())
                .await?;
            stdout.flush().await?;
        }
        ControlResponse::Error { error } => return Err(anyhow!(error)),
        _ => return Err(anyhow!("Unexpected remote-host attachment response.")),
    }

    loop {
        let mut line = String::new();
        match tokio::time::timeout(ATTACHMENT_TIMEOUT, input.read_line(&mut line)).await {
            Ok(Ok(read)) if read > 0 && line.trim() == "ping" => {
                reader.get_mut().write_all(b"ping\n").await?;
                reader.get_mut().flush().await?;
            }
            _ => return Ok(()),
        }
    }
}

#[cfg(unix)]
async fn stop() -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin());
    let request: StopRequest = serde_json::from_str(&read_bounded_line(&mut input).await?)?;
    let data_dir = canonical_data_dir()?;
    let response = control_exchange(
        &data_dir,
        &ControlRequest::Stop {
            expected_instance_id: request.expected_instance_id,
            expected_preview: request.expected_preview,
        },
    )
    .await?;
    println!("{HOST_MARKER}{}", serde_json::to_string(&response)?);
    Ok(())
}

#[cfg(unix)]
enum LiveHost {
    Running(HostDescriptor),
    Stopping(HostDescriptor),
    Missing,
}

#[cfg(unix)]
async fn live_descriptor(data_dir: &Path) -> LiveHost {
    let Ok(response) = control_exchange(data_dir, &ControlRequest::Status).await else {
        return LiveHost::Missing;
    };
    if !server_lock_is_held(data_dir).unwrap_or(false) {
        return LiveHost::Missing;
    }
    match response {
        ControlResponse::Status { descriptor, .. } => LiveHost::Running(descriptor),
        ControlResponse::Stopping { descriptor } => LiveHost::Stopping(descriptor),
        _ => LiveHost::Missing,
    }
}

#[cfg(unix)]
fn server_lock_is_held(data_dir: &Path) -> Result<bool> {
    let path = shared_path(data_dir, "lock")?;
    let lock = open_lock(&path)?;
    let result = match lock.try_read() {
        Ok(_) => Ok(false),
        Err(error) if is_lock_conflict(&error) => Ok(true),
        Err(error) => Err(lock_error(&path, error)),
    };
    result
}

#[cfg(unix)]
async fn wait_for_server_lock_free(data_dir: &Path) -> Result<()> {
    tokio::time::timeout(START_TIMEOUT, async {
        loop {
            if !server_lock_is_held(data_dir)? {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("Timed out waiting for the previous OpenResearch host to stop."))?
}

#[cfg(unix)]
async fn control_exchange(data_dir: &Path, request: &ControlRequest) -> Result<ControlResponse> {
    tokio::time::timeout(CONTROL_TIMEOUT, async {
        let mut stream = UnixStream::connect(control_socket_path(data_dir)?).await?;
        write_request(&mut stream, request).await?;
        let mut reader = BufReader::new(stream);
        let line = read_bounded_line(&mut reader).await?;
        Ok::<_, crate::error::Error>(serde_json::from_str(&line)?)
    })
    .await
    .map_err(|_| anyhow!("Timed out contacting the persistent OpenResearch host."))?
}

#[cfg(unix)]
async fn write_request(stream: &mut UnixStream, request: &ControlRequest) -> Result<()> {
    stream
        .write_all(serde_json::to_string(request)?.as_bytes())
        .await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(unix)]
async fn write_response(stream: &mut UnixStream, response: &ControlResponse) -> Result<()> {
    stream
        .write_all(serde_json::to_string(response)?.as_bytes())
        .await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

async fn read_bounded_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<String> {
    let mut line = String::new();
    let read = (&mut *reader)
        .take((MAX_CONTROL_LINE + 1) as u64)
        .read_line(&mut line)
        .await?;
    if read == 0 {
        return Err(anyhow!("Remote-host control message was empty."));
    }
    if read > MAX_CONTROL_LINE {
        return Err(anyhow!("Remote-host control message was too large."));
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn validate_token(token: &str) -> Result<()> {
    if token.len() < 32 || token.len() > 256 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(anyhow!("Invalid remote attachment credential."));
    }
    Ok(())
}

fn verify_expected(descriptor: &HostDescriptor, expected: Option<&str>) -> Result<()> {
    if expected.is_some_and(|expected| expected != descriptor.instance_id) {
        return Err(anyhow!(
            "A different OpenResearch host is running on {}. Start a new connection explicitly.",
            descriptor.hostname
        ));
    }
    Ok(())
}

fn print_descriptor(descriptor: &HostDescriptor) -> Result<()> {
    println!("{HOST_MARKER}{}", serde_json::to_string(descriptor)?);
    Ok(())
}

// Startup children must inherit neither the readiness pipe nor its environment capability.
static READY_PIPE: Mutex<Option<File>> = Mutex::new(None);

pub(crate) fn prepare_readiness_pipe() -> Result<()> {
    let Some(pipe) = std::env::var_os("ORX_SERVER_READY_PIPE") else {
        return Ok(());
    };
    std::env::remove_var("ORX_SERVER_READY_PIPE");
    #[cfg(unix)]
    let file = {
        let pipe: usize = pipe.to_string_lossy().parse()?;
        use std::os::fd::FromRawFd as _;
        let pipe = i32::try_from(pipe)?;
        let file = unsafe { File::from_raw_fd(pipe) };
        if unsafe { libc::fcntl(pipe, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(std::io::Error::last_os_error().into());
        }
        file
    };
    #[cfg(windows)]
    let file = OpenOptions::new().write(true).open(pipe)?;
    *READY_PIPE.lock().unwrap() = Some(file);
    Ok(())
}

#[derive(Clone, Copy)]
enum ServerMode {
    Persistent,
    Headless,
}

enum Readiness {
    #[cfg(unix)]
    Pipe(std::io::PipeReader),
    #[cfg(windows)]
    Named(tokio::net::windows::named_pipe::NamedPipeServer),
}

impl Readiness {
    async fn wait(self) -> std::io::Result<()> {
        let byte = match self {
            #[cfg(unix)]
            Self::Pipe(mut reader) => tokio::task::spawn_blocking(move || {
                let mut byte = [0];
                std::io::Read::read_exact(&mut reader, &mut byte)?;
                Ok::<_, std::io::Error>(byte)
            })
            .await
            .map_err(std::io::Error::other)??,
            #[cfg(windows)]
            Self::Named(mut server) => {
                server.connect().await?;
                let mut byte = [0];
                server.read_exact(&mut byte).await?;
                byte
            }
        };
        if byte != [1] {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Incorrect readiness signal",
            ));
        }
        Ok(())
    }
}

fn spawn_detached_host(
    data_dir: &Path,
    args: &[String],
    mode: ServerMode,
) -> Result<(std::process::Child, PathBuf, Option<Readiness>)> {
    #[cfg(unix)]
    if args.iter().any(|arg| arg == "--remote-host") {
        ensure_server_lock_free(data_dir)?;
    }
    #[cfg(unix)]
    let readiness = if matches!(mode, ServerMode::Headless) {
        Some(std::io::pipe()?)
    } else {
        None
    };
    #[cfg(windows)]
    let readiness = if matches!(mode, ServerMode::Headless) {
        let path = format!(r"\\.\pipe\orx-ready-{}", uuid::Uuid::new_v4());
        let server = tokio::net::windows::named_pipe::ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(&path)?;
        Some((server, path))
    } else {
        None
    };
    let executable = std::env::current_exe()?;
    let log_path = runtime_path(data_dir, "log")?;
    let log = open_runtime_log(&log_path)?;
    let mut command = crate::process::command(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    if matches!(mode, ServerMode::Headless) {
        command.env("ORX_DATA_DIR", data_dir);
    }
    #[cfg(unix)]
    if let Some((_, writer)) = &readiness {
        use std::os::fd::AsRawFd as _;
        command.env("ORX_SERVER_READY_PIPE", writer.as_raw_fd().to_string());
    }
    #[cfg(windows)]
    if let Some((_, path)) = &readiness {
        command.env("ORX_SERVER_READY_PIPE", path);
    }
    #[cfg(unix)]
    unsafe {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::process::CommandExt as _;
        let ready_fd = readiness.as_ref().map(|(_, writer)| writer.as_raw_fd());
        command.pre_exec(move || {
            if let Some(fd) = ready_fd {
                if libc::fcntl(fd, libc::F_SETFD, 0) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }

    #[cfg(not(windows))]
    let spawned = command.spawn();
    #[cfg(windows)]
    let spawned = {
        use windows_sys::Win32::Foundation::{
            GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };
        // A server inheriting this CLI's std pipes keeps whoever reads its output waiting until the server exits.
        let inheritable: Vec<_> = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
            .into_iter()
            .map(|id| unsafe { GetStdHandle(id) })
            .filter(|&handle| unsafe {
                let mut flags = 0;
                GetHandleInformation(handle, &mut flags) != 0
                    && flags & HANDLE_FLAG_INHERIT != 0
                    && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0
            })
            .collect();
        let spawned = command.spawn();
        for handle in inheritable {
            unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
        }
        spawned
    };

    match spawned {
        Ok(child) => {
            #[cfg(unix)]
            let readiness = readiness.map(|(reader, _)| Readiness::Pipe(reader));
            #[cfg(windows)]
            let readiness = readiness.map(|(server, _)| Readiness::Named(server));
            return Ok((child, log_path, readiness));
        }
        Err(error) if matches!(mode, ServerMode::Headless) => return Err(error.into()),
        Err(_) => {}
    }
    let log = open_runtime_log(&log_path)?;
    let child = crate::process::command("nohup")
        .arg(&executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|error| anyhow!("Could not start the persistent OpenResearch host: {error}"))?;
    Ok((child, log_path, None))
}

fn open_runtime_log(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    set_mode(path, 0o600)?;
    Ok(file)
}

#[cfg(unix)]
fn ensure_server_lock_free(data_dir: &Path) -> Result<()> {
    let path = shared_path(data_dir, "lock")?;
    let mut lock = open_lock(&path)?;
    lock.try_write().map(|_| ()).map_err(|error| {
        if !is_lock_conflict(&error) {
            return lock_error(&path, error);
        }
        let host = descriptor_path(data_dir)
            .ok()
            .and_then(|path| read_descriptor(&path))
            .map(|descriptor| descriptor.hostname)
            .unwrap_or_else(|| "the remote machine".into());
        anyhow!(
            "Another OpenResearch dashboard is using this database on {host}. Stop it before starting a persistent host."
        )
    })
}

fn is_lock_conflict(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
}

fn lock_error(path: &Path, error: std::io::Error) -> anyhow::Error {
    anyhow!("Could not lock dashboard lock {}: {error}", path.display())
}

fn open_lock(path: &Path) -> Result<fd_lock::RwLock<File>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(anyhow!("Unsafe OpenResearch dashboard lock."));
    }
    // Windows has no uid or mode to check, so a lock under an ORX_DATA_DIR the
    // user shares with someone else is guarded by its ACL alone.
    #[cfg(unix)]
    {
        if metadata_uid(&metadata) != effective_uid() {
            return Err(anyhow!("Unsafe OpenResearch dashboard lock."));
        }
        set_mode(path, 0o600)?;
    }
    Ok(fd_lock::RwLock::new(file))
}

pub(crate) fn canonical_data_dir() -> Result<PathBuf> {
    let data_dir = crate::store::data_dir();
    crate::paths::create_dir_all(&data_dir)?;
    crate::paths::canonicalize(&data_dir)
        .map_err(|error| anyhow!("Could not resolve {}: {error}", data_dir.display()))
}

pub(crate) fn hostname() -> String {
    crate::process::command("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|hostname| hostname.trim().to_string())
        .filter(|hostname| !hostname.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "remote host".into())
}

#[cfg(unix)]
fn descriptor_path(data_dir: &Path) -> Result<PathBuf> {
    shared_path(data_dir, "json")
}

#[cfg(unix)]
fn control_socket_path(data_dir: &Path) -> Result<PathBuf> {
    runtime_path(data_dir, "sock")
}

fn runtime_path(data_dir: &Path, extension: &str) -> Result<PathBuf> {
    #[cfg(unix)]
    let root = PathBuf::from(format!("/tmp/orx-{}", effective_uid()));
    #[cfg(unix)]
    ensure_private_dir(&root)?;
    #[cfg(not(unix))]
    let root = {
        let root = data_dir.join("runtime");
        std::fs::create_dir_all(&root)?;
        root
    };
    let lock_key = normalize_lock_key(data_dir)?;
    let hash = Sha256::digest(lock_key.as_os_str().as_encoded_bytes());
    let name = hash
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(root.join(format!("{name}.{extension}")))
}

fn shared_path(data_dir: &Path, extension: &str) -> Result<PathBuf> {
    let normalized = normalize_lock_key(data_dir)?;
    let parent = normalized
        .parent()
        .ok_or_else(|| anyhow!("OpenResearch data directory must have a parent."))?;
    let hash = Sha256::digest(normalized.as_os_str().as_encoded_bytes());
    let name = hash
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let prefix = format!(".orx-remote-{name}");
    let inside = normalized.join(format!("{prefix}.{extension}"));
    let sibling_lock = parent.join(format!("{prefix}.lock"));
    let use_inside = !sibling_lock.exists()
        && (normalized.join(format!("{prefix}.lock")).exists()
            || normalized.join(format!("{prefix}.start.lock")).exists()
            || !directory_writable(parent));
    Ok(if use_inside {
        inside
    } else {
        parent.join(format!("{prefix}.{extension}"))
    })
}

#[cfg(unix)]
fn directory_writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;

    std::ffi::CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK | libc::X_OK) == 0 })
}

#[cfg(not(unix))]
fn directory_writable(_path: &Path) -> bool {
    true
}

fn normalize_lock_key(path: &Path) -> Result<PathBuf> {
    if let Ok(path) = crate::paths::canonicalize(path) {
        return Ok(path);
    }
    let mut current = path;
    let mut missing = Vec::new();
    while !current.exists() {
        missing.push(
            current
                .file_name()
                .ok_or_else(|| anyhow!("OpenResearch data directory must have a name."))?,
        );
        current = current
            .parent()
            .ok_or_else(|| anyhow!("OpenResearch data directory must have a parent."))?;
    }
    let mut normalized = crate::paths::canonicalize(current)?;
    for component in missing.into_iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

#[cfg(unix)]
fn ensure_private_dir(path: &Path) -> Result<()> {
    use anyhow::Context as _;
    use std::os::unix::fs::DirBuilderExt as _;
    // Another orx may win the race to create it, so verify an existing one instead of failing.
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| format!("Cannot create {}", path.display()))
        }
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("Cannot inspect {}", path.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata_uid(&metadata) != effective_uid()
        || metadata_mode(&metadata) & 0o077 != 0
    {
        return Err(anyhow!(
            "Unsafe remote-host runtime directory {}.",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn write_descriptor(path: &Path, descriptor: &HostDescriptor) -> Result<()> {
    crate::local::git::atomic_write_with_mode(
        path,
        serde_json::to_string(descriptor)?.as_bytes(),
        Some(0o600),
    )
}

fn read_descriptor(path: &Path) -> Option<HostDescriptor> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn metadata_uid(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.uid()
}

#[cfg(unix)]
fn metadata_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode()
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_tokens_are_independent() {
        let auth = RemoteAuth::new("callback");
        auth.register("abcdefghijklmnopqrstuvwxyz-attachment-one");
        auth.register("abcdefghijklmnopqrstuvwxyz-attachment-two");
        assert!(auth.matches_attachment(&digest("abcdefghijklmnopqrstuvwxyz-attachment-one")));
        auth.unregister("abcdefghijklmnopqrstuvwxyz-attachment-one");
        assert!(!auth.matches_attachment(&digest("abcdefghijklmnopqrstuvwxyz-attachment-one")));
        assert!(auth.matches_attachment(&digest("abcdefghijklmnopqrstuvwxyz-attachment-two")));
        assert!(auth.matches_callback(&digest("callback")));
    }

    // Asserts the /tmp/orx-<uid> runtime layout, which is unix's alone.
    #[cfg(unix)]
    #[test]
    fn server_lock_is_shared_but_control_socket_is_node_local() {
        let data_dir = std::env::temp_dir().join(format!("orx-lock-test-{}", uuid::Uuid::new_v4()));
        let parent = crate::paths::canonicalize(data_dir.parent().unwrap()).unwrap();
        assert_eq!(
            shared_path(&data_dir, "lock").unwrap().parent(),
            Some(parent.as_path())
        );
        assert!(control_socket_path(&data_dir)
            .unwrap()
            .starts_with(format!("/tmp/orx-{}", effective_uid())));
    }

    #[cfg(unix)]
    fn descriptor() -> HostDescriptor {
        HostDescriptor {
            instance_id: "instance".into(),
            hostname: "box".into(),
            port: 1,
            version: "0".into(),
            build_id: None,
            dashboard_protocol: 0,
            control_protocol: CONTROL_PROTOCOL,
        }
    }

    #[test]
    fn a_descriptor_from_a_host_without_build_ids_still_reads() {
        let old = r#"{"instanceId":"i","hostname":"box","port":1,"version":"0.2.26","dashboardProtocol":5,"controlProtocol":1}"#;
        let descriptor: HostDescriptor = serde_json::from_str(old).unwrap();
        assert_eq!(descriptor.build_id, None);
        assert_eq!(descriptor.dashboard_protocol, 5);

        let current = HostDescriptor {
            build_id: Some("canary-20261112.3".into()),
            ..descriptor
        };
        let written = serde_json::to_value(&current).unwrap();
        assert_eq!(written["buildId"], "canary-20261112.3");
    }

    #[cfg(unix)]
    fn chat() -> Arc<ChatHost> {
        Arc::new(ChatHost::new(
            Arc::new(crate::local::opencode::AgentHost::new(None)),
            Arc::new(crate::local::codex::CodexHost::new()),
            Arc::new(crate::local::claude::ClaudeHost::new()),
        ))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn control_server_never_replaces_a_symlinked_socket() {
        let socket = control_socket_path(&canonical_data_dir().unwrap()).unwrap();
        let target =
            std::env::temp_dir().join(format!("orx-socket-target-{}", uuid::Uuid::new_v4()));
        for target_exists in [true, false] {
            if target_exists {
                std::fs::write(&target, "").unwrap();
            }
            std::os::unix::fs::symlink(&target, &socket).unwrap();
            let (stop, _) = watch::channel(false);
            let started = start_control_server(
                descriptor(),
                RemoteAuth::new("callback"),
                chat(),
                Default::default(),
                stop,
            )
            .await;
            let link_kept =
                std::fs::symlink_metadata(&socket).is_ok_and(|m| m.file_type().is_symlink());
            std::fs::remove_file(&socket).unwrap();
            let _ = std::fs::remove_file(&target);
            assert!(started.is_err(), "target exists: {target_exists}");
            assert!(link_kept, "target exists: {target_exists}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_callers_all_get_one_private_runtime_dir() {
        let root = crate::local::git::TemporaryDirectory::new("orx-runtime-race").unwrap();
        let racing = root.path().join("racing");
        let barrier = std::sync::Barrier::new(16);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    barrier.wait();
                    ensure_private_dir(&racing).unwrap();
                });
            }
        });
        assert_eq!(
            metadata_mode(&std::fs::metadata(&racing).unwrap()) & 0o777,
            0o700
        );
    }

    #[cfg(unix)]
    #[test]
    fn runtime_dir_must_be_a_private_directory_of_ours() {
        let root = crate::local::git::TemporaryDirectory::new("orx-runtime-dir").unwrap();
        let private = root.path().join("private");
        ensure_private_dir(&private).unwrap();
        ensure_private_dir(&private).unwrap();
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&private, &link).unwrap();
        let shared = root.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        set_mode(&shared, 0o755).unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "").unwrap();
        set_mode(&file, 0o600).unwrap();
        for unsafe_dir in [&link, &shared, &file] {
            assert!(
                ensure_private_dir(unsafe_dir).is_err(),
                "{}",
                unsafe_dir.display()
            );
        }

        let foreign = Path::new("/root");
        if std::fs::symlink_metadata(foreign).is_ok_and(|metadata| {
            metadata.is_dir()
                && metadata_mode(&metadata) & 0o077 == 0
                && metadata_uid(&metadata) != effective_uid()
        }) {
            assert!(ensure_private_dir(foreign).is_err());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_attachment_token_dies_with_its_heartbeat() {
        const TOKEN: &str = "abcdefghijklmnopqrstuvwxyz-attachment";
        for last_line in [Some("pong"), None] {
            let (client, server) = UnixStream::pair().unwrap();
            let auth = RemoteAuth::new("callback");
            let (stop, _) = watch::channel(false);
            let host = tokio::spawn(handle_control(
                server,
                descriptor(),
                auth.clone(),
                chat(),
                Default::default(),
                stop,
                Default::default(),
            ));
            let mut client = BufReader::new(client);
            write_request(
                client.get_mut(),
                &ControlRequest::Attach {
                    expected_instance_id: descriptor().instance_id,
                    token: TOKEN.into(),
                },
            )
            .await
            .unwrap();
            let attached = read_bounded_line(&mut client).await.unwrap();
            assert!(matches!(
                serde_json::from_str(&attached).unwrap(),
                ControlResponse::Attached { .. }
            ));
            assert!(auth.matches_attachment(&digest(TOKEN)));

            match last_line {
                Some(line) => client
                    .get_mut()
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .unwrap(),
                // Nothing is left to arrive, so the paused clock jumps straight to the expiry.
                None => tokio::time::pause(),
            }
            assert!(host.await.unwrap().is_err());
            assert!(!auth.matches_attachment(&digest(TOKEN)), "{last_line:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attaching_to_another_instance_registers_nothing() {
        let (client, server) = UnixStream::pair().unwrap();
        let auth = RemoteAuth::new("callback");
        let (stop, _) = watch::channel(false);
        let host = tokio::spawn(handle_control(
            server,
            descriptor(),
            auth.clone(),
            chat(),
            Default::default(),
            stop,
            Default::default(),
        ));
        let mut client = BufReader::new(client);
        write_request(
            client.get_mut(),
            &ControlRequest::Attach {
                expected_instance_id: "a previous instance".into(),
                token: "abcdefghijklmnopqrstuvwxyz-attachment".into(),
            },
        )
        .await
        .unwrap();
        let response = read_bounded_line(&mut client).await.unwrap();
        assert!(matches!(
            serde_json::from_str(&response).unwrap(),
            ControlResponse::Error { .. }
        ));
        host.await.unwrap().unwrap();
        assert_eq!(auth.attachment_count(), 0);
    }

    #[test]
    fn only_would_block_is_lock_contention() {
        assert!(is_lock_conflict(&std::io::ErrorKind::WouldBlock.into()));
        assert!(!is_lock_conflict(&std::io::ErrorKind::Unsupported.into()));
    }

    #[tokio::test]
    async fn empty_control_message_has_a_specific_error() {
        let mut reader = BufReader::new(tokio::io::empty());
        let error = read_bounded_line(&mut reader).await.unwrap_err();
        assert_eq!(error.to_string(), "Remote-host control message was empty.");
    }

    #[tokio::test]
    async fn oversized_control_message_is_rejected() {
        let input = vec![b'x'; MAX_CONTROL_LINE + 1];
        let mut reader = BufReader::new(input.as_slice());
        let error = read_bounded_line(&mut reader).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Remote-host control message was too large."
        );
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct ServerDescriptor {
    pub instance: String,
    pub pid: u32,
    pub starttime: u64,
    pub port: u16,
    pub version: String,
    pub proto: u32,
    pub token: String,
}

impl ServerDescriptor {
    pub fn agent_session(port: Option<String>, token: Option<String>) -> Result<Option<Self>> {
        match (port, token) {
            (None, None) => Ok(None),
            (Some(port), Some(token)) if !token.is_empty() => Ok(Some(Self {
                instance: String::new(),
                pid: 0,
                starttime: 0,
                port: port.parse()?,
                version: String::new(),
                proto: 2,
                token,
            })),
            _ => Err(anyhow!(
                "Agent server credentials are incomplete. Restart this agent session."
            )),
        }
    }

    pub fn publish(&self, data_dir: &Path) -> Result<()> {
        crate::local::git::atomic_write_with_mode(
            &runtime_path(data_dir, "server.json")?,
            &serde_json::to_vec(self)?,
            Some(0o600),
        )?;
        if let Some(mut file) = READY_PIPE.lock().unwrap().take() {
            std::io::Write::write_all(&mut file, &[1])?;
        }
        Ok(())
    }

    /// Withdraws this server's descriptor before an exec relaunch, which keeps the pid and start time that vouch for it.
    pub(crate) fn withdraw(data_dir: &Path) -> Result<()> {
        std::fs::remove_file(runtime_path(data_dir, "server.json")?)?;
        Ok(())
    }

    pub(crate) async fn live(data_dir: &Path) -> Result<Option<Self>> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let path = runtime_path(data_dir, "server.json")?;
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(anyhow!(
                    "Unsafe server descriptor. Remove it and restart OpenResearch."
                ));
            }
            #[cfg(unix)]
            if metadata_uid(&metadata) != effective_uid() || metadata_mode(&metadata) & 0o077 != 0 {
                return Err(anyhow!(
                    "Unsafe server descriptor. Remove it and restart OpenResearch."
                ));
            }
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options.open(&path)?;
            #[cfg(unix)]
            {
                let metadata = file.metadata()?;
                if metadata_uid(&metadata) != effective_uid()
                    || metadata_mode(&metadata) & 0o077 != 0
                {
                    return Err(anyhow!(
                        "Unsafe server descriptor. Remove it and restart OpenResearch."
                    ));
                }
            }
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(&mut file, 65537), &mut bytes)?;
            let descriptor: Self = match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                Err(_) => return Ok(None),
            };
            if crate::engine::process::identity(descriptor.pid)
                .is_none_or(|identity| identity.starttime != descriptor.starttime)
            {
                return Ok(None);
            }
            if descriptor.proto != 2 || descriptor.token.is_empty() {
                return Err(anyhow!("The running OpenResearch server uses an unsupported discovery protocol. Restart it with this version of orx."));
            }
            let nonce = uuid::Uuid::new_v4().to_string();
            let response = crate::net::loopback_client()
                .build()?
                .get(format!("http://127.0.0.1:{}/api/v2/hello", descriptor.port))
                .query(&[("nonce", &nonce)])
                .timeout(Duration::from_millis(500))
                .send()
                .await;
            let hello = match response {
                Ok(response) if response.status().is_success() => {
                    response.json::<serde_json::Value>().await.ok()
                }
                _ => None,
            };
            let Some(hello) = hello else {
                if tokio::time::Instant::now() >= deadline {
                    return Err(anyhow!(
                        "The running OpenResearch server is not ready. Try again shortly."
                    ));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            };
            let verified = hello["instance"].as_str() == Some(&descriptor.instance)
                && hello["proto"] == 2
                && hello["proof"].as_str().is_some_and(|proof| {
                    use base64::Engine as _;
                    use hmac::Mac;
                    let Ok(proof) = base64::engine::general_purpose::STANDARD.decode(proof) else {
                        return false;
                    };
                    hello_mac(&descriptor.token, &descriptor.instance, &nonce)
                        .verify_slice(&proof)
                        .is_ok()
                });
            if !verified {
                return Err(anyhow!("The running OpenResearch server could not be verified. Restart it before trying again."));
            }
            return Ok(Some(descriptor));
        }
    }
}

async fn ensure_started<T, F, Fut>(
    data_dir: PathBuf,
    timeout: Duration,
    mode: ServerMode,
    args: Vec<String>,
    mut inspect: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Option<T>>> + Send,
{
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let mut lock = open_lock(&shared_path(&data_dir, "start.lock")?)?;
        let _guard = lock.write()?;
        runtime.block_on(async {
            if let Some(descriptor) = inspect().await? {
                return Ok(descriptor);
            }
            let (mut child, log, readiness) = spawn_detached_host(&data_dir, &args, mode)?;
            if let Some(readiness) = readiness {
                match tokio::time::timeout(timeout, readiness.wait()).await {
                    Ok(Ok(())) => {
                        return inspect()
                            .await?
                            .ok_or_else(|| anyhow!("Server published an invalid descriptor"));
                    }
                    result => {
                        let eof = matches!(
                            &result,
                            Ok(Err(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof
                        );
                        let status = if eof {
                            Some(child.wait()?)
                        } else {
                            child.try_wait()?
                        };
                        if let Some(status) = status {
                            return Err(anyhow!(
                                "OpenResearch server exited ({status}). See {}.",
                                log.display()
                            ));
                        }

                        let _ = child.kill();
                        let _ = child.wait();
                        return match result {
                            Ok(Err(error)) => Err(anyhow!(
                                "OpenResearch server readiness failed: {error}. See {}.",
                                log.display()
                            )),
                            _ => Err(anyhow!(
                                "Timed out starting OpenResearch server. See {}.",
                                log.display()
                            )),
                        };
                    }
                }
            }
            tokio::time::timeout(timeout, async {
                loop {
                    if let Some(descriptor) = inspect().await? {
                        return Ok(descriptor);
                    }
                    if let Some(status) = child.try_wait()? {
                        return Err(anyhow!(
                            "The persistent OpenResearch host exited ({status}). See {}.",
                            log.display()
                        ));
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .map_err(|_| {
                anyhow!(
                    "Timed out starting the persistent OpenResearch host. See {}.",
                    log.display()
                )
            })?
        })
    })
    .await?
}

pub(crate) async fn discover() -> Result<ServerDescriptor> {
    if let Some(descriptor) = ServerDescriptor::agent_session(
        std::env::var("ORX_UP_PORT").ok(),
        std::env::var("ORX_UP_AUTH_TOKEN").ok(),
    )? {
        return Ok(descriptor);
    }
    discover_at(canonical_data_dir()?).await
}

async fn discover_at(data_dir: PathBuf) -> Result<ServerDescriptor> {
    if let Some(descriptor) = ServerDescriptor::live(&data_dir).await? {
        return Ok(descriptor);
    }
    ensure_started(
        data_dir.clone(),
        Duration::from_secs(10),
        ServerMode::Headless,
        vec![
            "up".into(),
            "--headless".into(),
            "--port".into(),
            "0".into(),
        ],
        move || {
            let data_dir = data_dir.clone();
            async move { ServerDescriptor::live(&data_dir).await }
        },
    )
    .await
}

pub(crate) fn hello_proof(token: &str, instance: &str, nonce: &str) -> String {
    use base64::Engine as _;
    use hmac::Mac;
    base64::engine::general_purpose::STANDARD
        .encode(hello_mac(token, instance, nonce).finalize().into_bytes())
}

fn hello_mac(token: &str, instance: &str, nonce: &str) -> hmac::Hmac<Sha256> {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<Sha256>::new_from_slice(token.as_bytes())
        .expect("HMAC accepts any key length");
    mac.update(instance.as_bytes());
    mac.update(b"\0");
    mac.update(nonce.as_bytes());
    mac
}

#[cfg(all(test, unix))]
mod discovery_tests {
    use super::*;

    #[test]
    fn exits_before_readiness_child() {
        if std::env::var_os("ORX_SERVER_READY_PIPE").is_some() {
            std::process::exit(7);
        }
    }

    #[tokio::test]
    async fn early_headless_exit_reports_the_child_status() {
        let directory =
            std::env::temp_dir().join(format!("orx-early-exit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let result: Result<ServerDescriptor> = ensure_started(
            directory.clone(),
            Duration::from_secs(2),
            ServerMode::Headless,
            vec![
                "--exact".into(),
                "commands::remote_host::discovery_tests::exits_before_readiness_child".into(),
            ],
            || async { Ok(None) },
        )
        .await;
        let error = result.err().unwrap().to_string();
        assert!(error.contains("exited") && error.contains('7'), "{error}");
        assert!(!error.contains("persistent"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_spawn_child() {
        let Some(output) = std::env::var_os("ORX_TEST_LEGACY_SPAWN") else {
            return;
        };
        let source = if matches!(
            crate::store::data_dir_source(),
            crate::store::DataDirSource::Env
        ) {
            "env"
        } else {
            "not env"
        };
        std::fs::write(output, source).unwrap();
    }

    #[test]
    fn legacy_spawn_parent() {
        let Some(output) = std::env::var_os("ORX_TEST_LEGACY_SPAWN") else {
            return;
        };
        let output = PathBuf::from(output);
        let (mut child, log, readiness) = spawn_detached_host(
            output.parent().unwrap(),
            &[
                "--exact".into(),
                "commands::remote_host::discovery_tests::legacy_spawn_child".into(),
            ],
            ServerMode::Persistent,
        )
        .unwrap();
        assert!(readiness.is_none());
        assert!(child.wait().unwrap().success());
        std::fs::remove_file(log).unwrap();
    }

    #[test]
    fn legacy_spawn_does_not_change_data_directory_provenance() {
        let directory =
            std::env::temp_dir().join(format!("orx-legacy-spawn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let output = directory.join("source");
        let result = crate::process::command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::remote_host::discovery_tests::legacy_spawn_parent",
                "--nocapture",
            ])
            .env_remove("ORX_DATA_DIR")
            .env("ORX_TEST_LEGACY_SPAWN", &output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read_to_string(output).unwrap(), "not env");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn discovery_waits_for_a_live_owner_during_restart_without_spawning() {
        let directory =
            std::env::temp_dir().join(format!("orx-restart-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let descriptor = ServerDescriptor {
            instance: "draining".into(),
            pid: std::process::id(),
            starttime: crate::engine::process::identity(std::process::id())
                .unwrap()
                .starttime,
            port: listener.local_addr().unwrap().port(),
            version: "test".into(),
            proto: 2,
            token: "secret".into(),
        };
        descriptor.publish(&directory).unwrap();
        let app = axum::Router::new().route(
            "/api/v2/hello",
            axum::routing::get(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let error = discover_at(directory.clone()).await.err().unwrap();
        assert!(error.to_string().contains("not ready"));
        assert!(!shared_path(&directory, "start.lock").unwrap().exists());
        server.abort();
        let _ = server.await;
        let error = discover_at(directory.clone()).await.err().unwrap();
        assert!(error.to_string().contains("not ready"));
        assert!(!shared_path(&directory, "start.lock").unwrap().exists());
        std::fs::remove_file(runtime_path(&directory, "server.json").unwrap()).unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn discovery_succeeds_when_the_live_owner_becomes_ready_within_the_bound() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let directory =
            std::env::temp_dir().join(format!("orx-ready-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let descriptor = ServerDescriptor {
            instance: "restarting".into(),
            pid: std::process::id(),
            starttime: crate::engine::process::identity(std::process::id())
                .unwrap()
                .starttime,
            port: listener.local_addr().unwrap().port(),
            version: "test".into(),
            proto: 2,
            token: "secret".into(),
        };
        descriptor.publish(&directory).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = axum::Router::new().route("/api/v2/hello", axum::routing::get(move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>| {
            let count = count.clone();
            async move {
                if count.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
                }
                Ok(axum::Json(serde_json::json!({"instance":"restarting", "proto":2, "proof":hello_proof("secret", "restarting", &query["nonce"])})))
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let started = tokio::time::Instant::now();
        let found = discover_at(directory.clone()).await.unwrap();
        assert_eq!(found.instance, descriptor.instance);
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(calls.load(Ordering::SeqCst) >= 2);
        assert!(!shared_path(&directory, "start.lock").unwrap().exists());
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn discovery_checks_process_identity_and_a_nonce_proof_without_sending_the_token() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let directory =
            std::env::temp_dir().join(format!("orx-discovery-proof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut descriptor = ServerDescriptor {
            instance: "instance".into(),
            pid: std::process::id(),
            starttime: crate::engine::process::identity(std::process::id())
                .unwrap()
                .starttime
                + 1,
            port: listener.local_addr().unwrap().port(),
            version: "test".into(),
            proto: 2,
            token: "private-token".into(),
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = axum::Router::new().route("/api/v2/hello", axum::routing::get(move |headers: axum::http::HeaderMap, axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String,String>>| {
            let count = count.clone();
            async move {
                assert!(!headers.contains_key("authorization"));
                count.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({"instance":"instance", "proto":2, "proof":hello_proof("private-token", "instance", &query["nonce"])}))
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        descriptor.publish(&directory).unwrap();
        assert!(ServerDescriptor::live(&directory).await.unwrap().is_none());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        descriptor.starttime -= 1;
        descriptor.publish(&directory).unwrap();
        assert!(ServerDescriptor::live(&directory).await.unwrap().is_some());
        descriptor.token = "incorrect-token".into();
        descriptor.publish(&directory).unwrap();
        assert!(ServerDescriptor::live(&directory).await.is_err());
        let path = runtime_path(&directory, "server.json").unwrap();
        set_mode(&path, 0o666).unwrap();
        assert!(ServerDescriptor::live(&directory).await.is_err());
        set_mode(&path, 0o600).unwrap();
        let target = directory.join("descriptor");
        std::fs::rename(&path, &target).unwrap();
        std::os::unix::fs::symlink(target, &path).unwrap();
        assert!(ServerDescriptor::live(&directory).await.is_err());
        std::fs::remove_file(path).unwrap();
        server.abort();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn agent_session_credentials_take_precedence_and_must_be_complete() {
        let descriptor =
            ServerDescriptor::agent_session(Some("1234".into()), Some("agent-token".into()))
                .unwrap()
                .unwrap();
        assert_eq!(descriptor.port, 1234);
        assert_eq!(descriptor.token, "agent-token");
        assert!(ServerDescriptor::agent_session(Some("1234".into()), None).is_err());
        assert!(ServerDescriptor::agent_session(None, None)
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn agent_session_child() {
        if std::env::var_os("ORX_TEST_AGENT_DISCOVERY").is_none() {
            return;
        }
        let descriptor = discover().await.unwrap();
        assert_eq!(descriptor.port, 1234);
        assert_eq!(descriptor.token, "agent-token");
        assert!(!PathBuf::from(std::env::var("ORX_DATA_DIR").unwrap()).exists());
    }

    #[test]
    fn discovery_uses_agent_environment_without_touching_a_descriptor_or_starting_a_server() {
        let path =
            std::env::temp_dir().join(format!("orx-agent-discovery-{}", uuid::Uuid::new_v4()));
        let output = crate::process::command(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "commands::remote_host::discovery_tests::agent_session_child",
                "--nocapture",
            ])
            .env("ORX_TEST_AGENT_DISCOVERY", "1")
            .env("ORX_UP_PORT", "1234")
            .env("ORX_UP_AUTH_TOKEN", "agent-token")
            .env("ORX_DATA_DIR", path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn autostart_child() {
        let Ok(data_dir) = std::env::var("ORX_DATA_DIR").map(PathBuf::from) else {
            return;
        };
        if !data_dir.join("autostart-fixture").exists() {
            return;
        }
        prepare_readiness_pipe().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let descriptor = ServerDescriptor {
            instance: uuid::Uuid::new_v4().to_string(),
            pid: std::process::id(),
            starttime: crate::engine::process::identity(std::process::id())
                .unwrap()
                .starttime,
            port: listener.local_addr().unwrap().port(),
            version: "test".into(),
            proto: 2,
            token: "private-test-token".into(),
        };
        use std::io::Write;
        let mut count = OpenOptions::new()
            .create(true)
            .append(true)
            .open(data_dir.join("spawn-count"))
            .unwrap();
        writeln!(count, "started").unwrap();
        let instance = descriptor.instance.clone();
        let token = descriptor.token.clone();
        let app = axum::Router::new().route(
            "/api/v2/hello",
            axum::routing::get(move |axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>| {
                let instance = instance.clone(); let token = token.clone();
                async move { axum::Json(serde_json::json!({"instance":instance,"proto":2,"proof":hello_proof(&token, &instance, &query["nonce"])})) }
            }),
        );
        descriptor.publish(&data_dir).unwrap();
        let _ = tokio::time::timeout(
            Duration::from_secs(20),
            std::future::IntoFuture::into_future(axum::serve(listener, app)),
        )
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_autostarts_recheck_the_descriptor_under_the_shared_start_lock() {
        let data_dir = std::env::temp_dir().join(format!("orx-discover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("autostart-fixture"), []).unwrap();
        let stale = ServerDescriptor {
            instance: "dead".into(),
            pid: i32::MAX as u32,
            starttime: 0,
            port: 1,
            version: "test".into(),
            proto: 2,
            token: "old".into(),
        };
        stale.publish(&data_dir).unwrap();
        assert!(ServerDescriptor::live(&data_dir).await.unwrap().is_none());
        let start = |data_dir: PathBuf| {
            ensure_started(
                data_dir.clone(),
                Duration::from_secs(10),
                ServerMode::Headless,
                vec![
                    "--exact".into(),
                    "commands::remote_host::discovery_tests::autostart_child".into(),
                    "--nocapture".into(),
                ],
                move || {
                    let data_dir = data_dir.clone();
                    async move { ServerDescriptor::live(&data_dir).await }
                },
            )
        };
        let (left, right) = tokio::join!(start(data_dir.clone()), start(data_dir.clone()));
        let left = left.unwrap();
        let right = right.unwrap();
        assert_eq!(left.instance, right.instance);
        assert_eq!(
            std::fs::read_to_string(data_dir.join("spawn-count")).unwrap(),
            "started\n"
        );
        unsafe {
            libc::kill(left.pid as i32, libc::SIGTERM);
        }
        let _ = std::fs::remove_file(runtime_path(&data_dir, "server.json").unwrap());
        let _ = std::fs::remove_dir_all(data_dir);
    }

    #[tokio::test]
    async fn wrong_instance_echo_and_reused_port_refuse_discovery() {
        let data_dir =
            std::env::temp_dir().join(format!("orx-wrong-server-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&data_dir).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let descriptor = ServerDescriptor {
            instance: "expected".into(),
            pid: std::process::id(),
            starttime: crate::engine::process::identity(std::process::id())
                .unwrap()
                .starttime,
            port: listener.local_addr().unwrap().port(),
            version: "test".into(),
            proto: 2,
            token: "secret".into(),
        };
        descriptor.publish(&data_dir).unwrap();
        let app = axum::Router::new().route(
            "/api/v2/hello",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({"instance":"different", "proto":2}))
            }),
        );
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        assert!(ServerDescriptor::live(&data_dir).await.is_err());
        task.abort();
        task.await.unwrap_err();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", descriptor.port))
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new()).await.unwrap();
        });
        assert!(ServerDescriptor::live(&data_dir).await.is_err());
        task.abort();
        let _ = std::fs::remove_file(runtime_path(&data_dir, "server.json").unwrap());
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
