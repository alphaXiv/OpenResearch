//! Persistent SSH remote host and its private same-user control channel.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};
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

pub(crate) enum DashboardLockMode {
    Shared,
    Exclusive,
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
    pub(crate) fn acquire(data_dir: &Path, mode: DashboardLockMode) -> Result<Self> {
        let path = shared_path(data_dir, "lock")?;
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let result = (|| -> Result<()> {
                let mut lock = open_lock(&path)?;
                match mode {
                    DashboardLockMode::Shared => {
                        let _guard = lock.try_read().map_err(|_| {
                            anyhow!(
                                "A persistent OpenResearch host is already using this database."
                            )
                        })?;
                        ready_tx.send(Ok(())).ok();
                        let _ = release_rx.recv();
                    }
                    DashboardLockMode::Exclusive => {
                        let _guard = lock.try_write().map_err(|_| {
                            anyhow!(
                                "Another OpenResearch dashboard is already using this database."
                            )
                        })?;
                        ready_tx.send(Ok(())).ok();
                        let _ = release_rx.recv();
                    }
                }
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

pub(crate) async fn start_control_server(
    descriptor: HostDescriptor,
    auth: RemoteAuth,
    chat: Arc<ChatHost>,
    stopping: Arc<AtomicBool>,
    stop: watch::Sender<bool>,
) -> Result<ControlServer> {
    let data_dir = canonical_data_dir()?;
    let socket_path = control_socket_path(&data_dir)?;
    if socket_path.exists() {
        let metadata = std::fs::symlink_metadata(&socket_path)?;
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
            tokio::spawn(async move {
                let _ = handle_control(stream, descriptor, auth, chat, stopping, stop).await;
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

async fn handle_control(
    stream: UnixStream,
    descriptor: HostDescriptor,
    auth: RemoteAuth,
    chat: Arc<ChatHost>,
    stopping: Arc<AtomicBool>,
    stop: watch::Sender<bool>,
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
            write_response(reader.get_mut(), &ControlResponse::Attached { descriptor }).await?;
            auth.register(&token);
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
            let preview = stop_preview(&chat, &auth).await;
            if expected_instance_id != descriptor.instance_id || expected_preview != preview {
                stopping.store(false, Ordering::SeqCst);
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
            let response = write_response(reader.get_mut(), &ControlResponse::Accepted).await;
            let _ = stop.send(true);
            response?;
        }
    }
    Ok(())
}

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

pub(crate) async fn run(args: RemoteHostArgs) -> Result<()> {
    match args.command {
        RemoteHostCommand::Ensure { expected_instance } => ensure(expected_instance).await,
        RemoteHostCommand::Status => print_status().await,
        RemoteHostCommand::Attach { expected_instance } => attach(&expected_instance).await,
        RemoteHostCommand::Stop => stop().await,
    }
}

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

    let mut start_lock = open_lock(&shared_path(&data_dir, "start.lock")?)?;
    let _start_guard = start_lock.write()?;
    match live_descriptor(&data_dir).await {
        LiveHost::Running(descriptor) => return print_descriptor(&descriptor),
        LiveHost::Stopping(_) => wait_for_server_lock_free(&data_dir).await?,
        LiveHost::Missing => ensure_server_lock_free(&data_dir)?,
    }
    spawn_detached_host()?;
    let started = tokio::time::timeout(START_TIMEOUT, async {
        loop {
            if let LiveHost::Running(descriptor) = live_descriptor(&data_dir).await {
                return descriptor;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| anyhow!("Timed out starting the persistent OpenResearch host."))?;
    print_descriptor(&started)
}

async fn print_status() -> Result<()> {
    let data_dir = canonical_data_dir()?;
    let response = control_exchange(&data_dir, &ControlRequest::Status).await?;
    if !server_lock_is_held(&data_dir)? {
        return Err(anyhow!("The remote-host descriptor is stale."));
    }
    println!("{HOST_MARKER}{}", serde_json::to_string(&response)?);
    Ok(())
}

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

enum LiveHost {
    Running(HostDescriptor),
    Stopping(HostDescriptor),
    Missing,
}

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

fn server_lock_is_held(data_dir: &Path) -> Result<bool> {
    let mut lock = open_lock(&shared_path(data_dir, "lock")?)?;
    let held = lock.try_write().is_err();
    Ok(held)
}

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

async fn write_request(stream: &mut UnixStream, request: &ControlRequest) -> Result<()> {
    stream
        .write_all(serde_json::to_string(request)?.as_bytes())
        .await?;
    stream.write_all(b"\n").await?;
    stream.flush().await?;
    Ok(())
}

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
    let read = reader.read_line(&mut line).await?;
    if read == 0 || read > MAX_CONTROL_LINE {
        return Err(anyhow!("Invalid remote-host control message."));
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

fn spawn_detached_host() -> Result<()> {
    let executable = std::env::current_exe()?;
    let args = ["up", "--no-browser", "--remote-host", "--port", "0"];
    let mut command = std::process::Command::new(&executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt as _;
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    if command.spawn().is_ok() {
        return Ok(());
    }
    std::process::Command::new("nohup")
        .arg(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| anyhow!("Could not start the persistent OpenResearch host: {error}"))
}

fn ensure_server_lock_free(data_dir: &Path) -> Result<()> {
    let mut lock = open_lock(&shared_path(data_dir, "lock")?)?;
    lock.try_write().map(|_| ()).map_err(|_| {
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
    if !metadata.is_file() || metadata_uid(&metadata) != effective_uid() {
        return Err(anyhow!("Unsafe OpenResearch dashboard lock."));
    }
    set_mode(path, 0o600)?;
    Ok(fd_lock::RwLock::new(file))
}

pub(crate) fn canonical_data_dir() -> Result<PathBuf> {
    let data_dir = crate::store::data_dir();
    std::fs::create_dir_all(&data_dir)?;
    data_dir
        .canonicalize()
        .map_err(|error| anyhow!("Could not resolve {}: {error}", data_dir.display()))
}

pub(crate) fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|hostname| hostname.trim().to_string())
        .filter(|hostname| !hostname.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "remote host".into())
}

fn descriptor_path(data_dir: &Path) -> Result<PathBuf> {
    shared_path(data_dir, "json")
}

fn control_socket_path(data_dir: &Path) -> Result<PathBuf> {
    runtime_path(data_dir, "sock")
}

fn runtime_path(data_dir: &Path, extension: &str) -> Result<PathBuf> {
    let root = PathBuf::from(format!("/tmp/orx-{}", effective_uid()));
    ensure_private_dir(&root)?;
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
    Ok(parent.join(format!(".orx-remote-{name}.{extension}")))
}

fn normalize_lock_key(path: &Path) -> Result<PathBuf> {
    if let Ok(path) = path.canonicalize() {
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
    let mut normalized = current.canonicalize()?;
    for component in missing.into_iter().rev() {
        normalized.push(component);
    }
    Ok(normalized)
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)?;
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
        return Ok(());
    }
    std::fs::create_dir(path)?;
    set_mode(path, 0o700)
}

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

    #[test]
    fn server_lock_is_shared_but_control_socket_is_node_local() {
        let data_dir = std::env::temp_dir().join(format!("orx-lock-test-{}", uuid::Uuid::new_v4()));
        let parent = data_dir.parent().unwrap().canonicalize().unwrap();
        assert_eq!(
            shared_path(&data_dir, "lock").unwrap().parent(),
            Some(parent.as_path())
        );
        assert!(control_socket_path(&data_dir)
            .unwrap()
            .starts_with(format!("/tmp/orx-{}", effective_uid())));
    }
}
