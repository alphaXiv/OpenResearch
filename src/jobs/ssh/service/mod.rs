pub(crate) mod logs;
mod state;
mod transport;
mod wire;

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{oneshot, Mutex, Semaphore};

use super::SshTarget;
use crate::error::{anyhow, Result};

const VERSION: u32 = 1;
const UPLOAD_CHUNK: usize = 256 * 1024;

#[derive(Clone, Serialize, Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
    version: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    token: String,
    version: u32,
    target: Option<SshTarget>,
    operation: Value,
}

#[derive(Serialize, Deserialize)]
struct Response {
    value: Option<Value>,
    error: Option<String>,
}

struct Host {
    resolved: state::Resolved,
    session: Option<Arc<transport::Session>>,
}
impl Host {
    async fn reusable_session(&self) -> Result<Option<Arc<transport::Session>>> {
        let Some(session) = &self.session else {
            return Ok(None);
        };
        if session.alive() {
            return Ok(Some(session.clone()));
        }
        // Teardown persists backoff before it signals completion.
        session.stop_and_wait().await?;
        Ok(None)
    }
}

#[derive(Default)]
struct SlurmCache {
    sampled: Option<std::time::Instant>,
    values: Value,
}
struct Attempt {
    created: std::time::Instant,
    prompt: Option<String>,
    answer: Option<oneshot::Sender<String>>,
    result: Option<std::result::Result<(), String>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
struct Broker {
    hosts: Mutex<HashMap<String, Arc<Mutex<Host>>>>,
    attempts: Mutex<HashMap<String, Attempt>>,
    endpoint: Endpoint,
    collectors: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    slurm: Mutex<HashMap<String, Arc<Mutex<SlurmCache>>>>,
    shutdown: tokio::sync::Notify,
    stopping: std::sync::atomic::AtomicBool,
}

fn private_directory(path: &Path) -> Result<()> {
    static VERIFIED: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let mut verified = VERIFIED
        .lock()
        .map_err(|_| anyhow!("SSH directory validation lock failed"))?;
    if verified.contains(path) {
        return Ok(());
    }
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(anyhow!("SSH service directory is not a private directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(anyhow!("SSH service directory has a different owner"));
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        let script = r#"$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$acl = Get-Acl -LiteralPath $env:ORX_PRIVATE_DIRECTORY
$owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
if ($owner -notin @($identity.User.Value, 'S-1-5-32-544', 'S-1-5-18')) { throw 'SSH service directory has a different owner' }
$acl.SetAccessRuleProtection($true, $false)
foreach ($rule in @($acl.Access)) { [void]$acl.RemoveAccessRuleAll($rule) }
$rule = [Security.AccessControl.FileSystemAccessRule]::new($identity.User, 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
$acl.SetAccessRule($rule)
Set-Acl -LiteralPath $env:ORX_PRIVATE_DIRECTORY -AclObject $acl
"#;
        let status = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            // Windows PowerShell must not load PowerShell 7's incompatible modules.
            .env_remove("PSModulePath")
            .env("ORX_PRIVATE_DIRECTORY", path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(anyhow!(
                "Could not verify and protect the private SSH service directory."
            ));
        }
    }
    verified.insert(path.to_owned());
    Ok(())
}

fn endpoint() -> Result<Endpoint> {
    let endpoint: Endpoint =
        serde_json::from_slice(&std::fs::read(state::root().join("endpoint.json"))?)?;
    if endpoint.version != VERSION {
        return Err(anyhow!("SSH service version changed. Restart OpenResearch and its monitors before reconnecting."));
    }
    Ok(endpoint)
}

async fn exchange(
    endpoint: &Endpoint,
    target: Option<SshTarget>,
    operation: Value,
) -> Result<Value> {
    let mut stream = tokio::time::timeout(
        Duration::from_secs(2),
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port)),
    )
    .await??;
    wire::write(
        &mut stream,
        &Request {
            token: endpoint.token.clone(),
            version: VERSION,
            target,
            operation,
        },
    )
    .await?;
    let response: Response = wire::read(&mut stream).await?;
    if let Some(error) = response.error {
        return Err(anyhow!(error));
    }
    response
        .value
        .ok_or_else(|| anyhow!("Missing SSH service response"))
}

async fn ensure() -> Result<Endpoint> {
    let _storage_guard = crate::store::acquire_data_dir_read_lock()?;
    private_directory(&state::root())?;
    if let Ok(endpoint) = endpoint() {
        if tokio::time::timeout(
            Duration::from_secs(2),
            exchange(&endpoint, None, json!({"op":"hello"})),
        )
        .await
        .is_ok_and(|result| result.is_ok())
        {
            return Ok(endpoint);
        }
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state::root().join("start.lock"))?;
    let mut locked = false;
    for _ in 0..100 {
        match lock.try_lock() {
            Ok(()) => {
                locked = true;
                break;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    if !locked {
        return Err(anyhow!("SSH service startup is busy; retry shortly."));
    }
    if let Ok(endpoint) = endpoint() {
        if exchange(&endpoint, None, json!({"op":"hello"}))
            .await
            .is_ok()
        {
            return Ok(endpoint);
        }
    }
    let executable = crate::paths::spawnable_exe()?;
    #[cfg(test)]
    let executable = executable
        .parent()
        .and_then(|path| path.parent())
        .ok_or_else(|| anyhow!("Missing test binary directory"))?
        .join(format!("orx{}", std::env::consts::EXE_SUFFIX));
    let mut command = std::process::Command::new(executable);
    command
        .arg("ssh-service")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::local::shell_env::export_to(|key, value| {
        command.env(key, value);
    });
    command.env("ORX_DATA_DIR", crate::store::data_dir());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = command.spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(endpoint) = endpoint() {
            if exchange(&endpoint, None, json!({"op":"hello"}))
                .await
                .is_ok()
            {
                return Ok(endpoint);
            }
        }
    }
    Err(anyhow!(
        "Could not start the SSH monitoring service. Restart OpenResearch and retry."
    ))
}

pub(crate) async fn request(target: &SshTarget, operation: Value) -> Result<Value> {
    let endpoint = ensure().await?;
    // A lost response may follow a successful mutation: never replay it here.
    exchange(&endpoint, Some(target.clone()), operation).await
}

pub(crate) async fn healthy(target: &SshTarget) -> bool {
    let Ok(endpoint) = endpoint() else {
        return false;
    };
    exchange(&endpoint, Some(target.clone()), json!({"op":"status"}))
        .await
        .is_ok_and(|value| value["connected"] == true)
}

pub(crate) async fn attach(target: &SshTarget) -> Result<()> {
    request(target, json!({"op":"attach"})).await?;
    Ok(())
}

pub(crate) async fn execute(
    target: &SshTarget,
    command: &str,
    input: Option<&[u8]>,
) -> Result<String> {
    execute_lane(target, command, input, "commands").await
}

pub(crate) async fn execute_lane(
    target: &SshTarget,
    command: &str,
    input: Option<&[u8]>,
    lane: &str,
) -> Result<String> {
    let endpoint = ensure().await?;
    execute_at(&endpoint, target, command, input, lane).await
}

async fn execute_at(
    endpoint: &Endpoint,
    target: &SshTarget,
    command: &str,
    input: Option<&[u8]>,
    lane: &str,
) -> Result<String> {
    if let Some(input) = input.filter(|input| input.len() > UPLOAD_CHUNK) {
        return upload_reader(endpoint, target, command, input, lane).await;
    }
    let value = exchange(
        endpoint,
        Some(target.clone()),
        json!({"op":"exec", "command":command, "lane":lane,
        "input":base64::engine::general_purpose::STANDARD.encode(input.unwrap_or_default())}),
    )
    .await?;
    output(value)
}

fn output(value: Value) -> Result<String> {
    let decode = |field: &str| -> Result<Vec<u8>> {
        Ok(base64::engine::general_purpose::STANDARD.decode(
            value[field]
                .as_str()
                .ok_or_else(|| anyhow!("Invalid SSH helper output"))?,
        )?)
    };
    let stdout = decode("stdout")?;
    let stderr = decode("stderr")?;
    let code = value["code"]
        .as_i64()
        .ok_or_else(|| anyhow!("Invalid remote exit status"))?;
    if code != 0 {
        return Err(anyhow!(
            "Remote command failed (exit {code}): {}",
            String::from_utf8_lossy(&stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&stdout).into_owned())
}

pub(crate) async fn upload(target: &SshTarget, command: &str, path: &Path) -> Result<String> {
    let file = tokio::fs::File::open(path).await?;
    let endpoint = ensure().await?;
    upload_reader(&endpoint, target, command, file, "commands").await
}

async fn upload_reader(
    endpoint: &Endpoint,
    target: &SshTarget,
    command: &str,
    mut input: impl tokio::io::AsyncRead + Unpin,
    lane: &str,
) -> Result<String> {
    use tokio::io::AsyncReadExt;
    let start = exchange(
        endpoint,
        Some(target.clone()),
        json!({"op":"upload_start", "lane":lane}),
    )
    .await?;
    let token = start["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing remote upload token"))?;
    let result = async {
        let mut chunk = vec![0; UPLOAD_CHUNK];
        loop {
            let count = input.read(&mut chunk).await?;
            if count == 0 {break;}
            exchange(endpoint, Some(target.clone()), json!({"op":"upload_chunk", "lane":lane, "token":token, "data":base64::engine::general_purpose::STANDARD.encode(&chunk[..count])})).await?;
        }
        output(exchange(endpoint, Some(target.clone()), json!({"op":"upload_commit", "lane":lane, "token":token, "command":command})).await?)
    }.await;
    if result.is_err() {
        let _ = exchange(
            endpoint,
            Some(target.clone()),
            json!({"op":"upload_abort", "lane":lane, "token":token}),
        )
        .await;
    }
    result
}

impl Broker {
    async fn host(&self, target: &SshTarget) -> Result<Arc<Mutex<Host>>> {
        let resolved = state::resolve(target).await?;
        let mut hosts = self.hosts.lock().await;
        Ok(hosts
            .entry(resolved.key.clone())
            .or_insert_with(|| {
                Arc::new(Mutex::new(Host {
                    resolved,
                    session: None,
                }))
            })
            .clone())
    }

    async fn session(
        &self,
        target: &SshTarget,
        interactive: Option<&str>,
        explicit: bool,
    ) -> Result<Arc<transport::Session>> {
        let host = self.host(target).await?;
        let mut host = host.lock().await;
        if let Some(session) = host.reusable_session().await? {
            return Ok(session);
        }
        let next = state::retry_at(&host.resolved.key)?;
        if !explicit && next > crate::store::now_ms() {
            return Err(anyhow!("SSH connection unavailable; automatic retries are backing off. Use Connect if authentication is required."));
        }
        for run in crate::store::Store::open()?.list_active_runs()? {
            let descriptor = crate::jobs::BackendDescriptor::parse(&run.backend_json)?;
            if matches!(
                descriptor.kind.as_str(),
                "ssh_job" | "slurm_job" | "openresearch_job"
            ) && crate::commands::supervise::supervisor_running(&run.id)
                && !crate::commands::supervise::uses_shared_ssh(&run.id)
            {
                let legacy_target = match descriptor.kind.as_str() {
                    "ssh_job" | "slurm_job" => {
                        descriptor.namespace.as_deref().map(SshTarget::alias)
                    }
                    "openresearch_job" => descriptor.openresearch_ssh_target(),
                    _ => None,
                };
                if let Some(legacy_target) = legacy_target {
                    // An obsolete alias must not prevent other hosts from connecting.
                    let Ok(legacy) = state::resolve(&legacy_target.for_run(&run.id)).await else {
                        continue;
                    };
                    if legacy.key == host.resolved.key {
                        return Err(anyhow!("Run {} is monitored by an older OpenResearch process. Restart that monitor with the updated version before using shared SSH; its lock has not been taken over.", run.id));
                    }
                }
            }
        }
        state::reserve_attempt(&host.resolved.key)?;
        let session = transport::connect(
            &host.resolved,
            interactive.map(|attempt| (&self.endpoint, attempt)),
        )
        .await;
        match session {
            Ok(session) => {
                host.session = Some(session.clone());
                Ok(session)
            }
            Err(error) => {
                state::failure(&host.resolved.key, &error.to_string())?;
                Err(error)
            }
        }
    }

    async fn dispatch(
        self: &Arc<Self>,
        target: Option<SshTarget>,
        operation: Value,
    ) -> Result<Value> {
        if self.stopping.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(anyhow!("SSH monitoring is stopping for a storage move."));
        }
        let op = operation["op"]
            .as_str()
            .ok_or_else(|| anyhow!("Missing SSH operation"))?;
        if op == "hello" {
            return Ok(json!({"version":VERSION}));
        }
        if op == "askpass" {
            let id = operation["attempt"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing authentication attempt"))?;
            let (tx, rx) = oneshot::channel();
            {
                let mut attempts = self.attempts.lock().await;
                let attempt = attempts
                    .get_mut(id)
                    .ok_or_else(|| anyhow!("Authentication attempt expired"))?;
                if attempt.result.is_some() || attempt.answer.is_some() {
                    return Err(anyhow!("Authentication prompt is no longer valid"));
                }
                attempt.prompt = Some(
                    operation["prompt"]
                        .as_str()
                        .unwrap_or("SSH authentication")
                        .chars()
                        .take(4096)
                        .collect(),
                );
                attempt.answer = Some(tx);
            }
            let answer = tokio::time::timeout(Duration::from_secs(180), rx).await??;
            return Ok(json!({"answer":answer}));
        }
        if op.starts_with("connect_") && op != "connect_start" {
            let id = operation["attempt"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing authentication attempt"))?;
            let mut attempts = self.attempts.lock().await;
            let attempt = attempts
                .get_mut(id)
                .ok_or_else(|| anyhow!("Authentication attempt expired"))?;
            if op == "connect_cancel" {
                if attempt.result.is_none() {
                    if let Some(task) = attempt.task.take() {
                        task.abort();
                    }
                }
                attempts.remove(id);
                return Ok(json!({}));
            }
            if op == "connect_answer" {
                let answer = operation["answer"]
                    .as_str()
                    .filter(|answer| answer.len() <= 16384)
                    .ok_or_else(|| anyhow!("Invalid SSH answer"))?;
                let sender = attempt
                    .answer
                    .take()
                    .ok_or_else(|| anyhow!("SSH prompt already answered"))?;
                attempt.prompt = None;
                sender
                    .send(answer.to_owned())
                    .map_err(|_| anyhow!("SSH prompt expired"))?;
            }
            return Ok(
                json!({"prompt":attempt.prompt, "done":attempt.result.is_some(), "error":attempt.result.as_ref().and_then(|result| result.as_ref().err())}),
            );
        }
        let target = target.ok_or_else(|| anyhow!("Missing SSH target"))?;
        if op == "connect_start" {
            let id = uuid::Uuid::new_v4().to_string();
            let mut attempts = self.attempts.lock().await;
            attempts.retain(|_, attempt| {
                let keep = attempt.created.elapsed() < Duration::from_secs(600);
                if !keep {
                    if let Some(task) = attempt.task.take() {
                        task.abort();
                    }
                }
                keep
            });
            if attempts.len() >= 8 {
                return Err(anyhow!(
                    "Close existing SSH Connect dialogs before opening another."
                ));
            }
            attempts.insert(
                id.clone(),
                Attempt {
                    created: std::time::Instant::now(),
                    prompt: None,
                    answer: None,
                    result: None,
                    task: None,
                },
            );
            let broker = self.clone();
            let task_id = id.clone();
            let task = tokio::spawn(async move {
                let result = broker
                    .session(&target, Some(&task_id), true)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string());
                if let Some(attempt) = broker.attempts.lock().await.get_mut(&task_id) {
                    attempt.result = Some(result);
                    attempt.answer = None;
                    attempt.prompt = None;
                }
            });
            attempts.get_mut(&id).unwrap().task = Some(task);
            return Ok(json!({"attempt":id}));
        }
        if op == "status" {
            let host = self.host(&target).await?;
            let host = host.lock().await;
            return Ok(
                json!({"connected":host.session.as_ref().is_some_and(|session| session.alive()), "retryAt":state::retry_at(&host.resolved.key)?}),
            );
        }
        #[cfg(unix)]
        if op == "attach" && target.extra_opts.is_empty() {
            super::prepared::prepare(&target, true).await?;
        }
        let session = self.session(&target, None, op == "attach").await?;
        if op == "attach" {
            return Ok(json!({}));
        }
        if op == "slurm" {
            let run = operation["run"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing Slurm run"))?;
            let job = operation["job"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing Slurm job"))?;
            let resolved = state::resolve(&target).await?;
            let cache = self
                .slurm
                .lock()
                .await
                .entry(resolved.key.clone())
                .or_default()
                .clone();
            let mut cache = cache.lock().await;
            if cache
                .sampled
                .is_none_or(|sampled| sampled.elapsed() >= Duration::from_secs(5))
                || cache.values.get(run).is_none()
            {
                let mut jobs = vec![json!({"run":run, "job":job, "dir":operation["dir"]})];
                let active = crate::store::Store::open()?.list_active_runs()?;
                let db = state::database()?;
                for stored in active {
                    if stored.id == run {
                        continue;
                    }
                    let descriptor = crate::jobs::BackendDescriptor::parse(&stored.backend_json)?;
                    if descriptor.kind != "slurm_job" {
                        continue;
                    }
                    let matches: bool = db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM bindings WHERE run = ?1 AND connection = ?2)",
                        rusqlite::params![stored.id, resolved.key],
                        |row| row.get(0),
                    )?;
                    if matches {
                        if let Some(job) = descriptor.job_id {
                            let dir = crate::jobs::slurm::run_dir(
                                descriptor.remote_root.as_deref(),
                                &stored.id,
                            );
                            jobs.push(json!({"run":stored.id, "job":job, "dir":dir}));
                        }
                    }
                }
                drop(db);
                cache.values = session.request(json!({"op":"slurm", "jobs":jobs})).await?;
                cache.sampled = Some(std::time::Instant::now());
            }
            return Ok(json!({"token":cache.values[run]}));
        }
        if op == "collect" {
            let run = operation["run"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing run"))?;
            let dir = operation["dir"]
                .as_str()
                .ok_or_else(|| anyhow!("Missing run directory"))?;
            if target.run_id.as_deref() != Some(run) {
                return Err(anyhow!("Log collector target is not bound to this run"));
            }
            let lock = self
                .collectors
                .lock()
                .await
                .entry(run.to_owned())
                .or_default()
                .clone();
            let _guard = lock.lock().await;
            return logs::collect(&session, run, dir).await;
        }
        session.request(operation).await
    }
}

pub(crate) async fn run() -> Result<()> {
    let _storage_guard = crate::store::acquire_data_dir_read_lock()?;
    private_directory(&state::root())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state::root().join("service.lock"))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    state::database()?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let endpoint = Endpoint {
        port: listener.local_addr()?.port(),
        token: format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
        version: VERSION,
    };
    crate::local::git::atomic_write_with_mode(
        &state::root().join("endpoint.json"),
        &serde_json::to_vec(&endpoint)?,
        Some(0o600),
    )?;
    let broker = Arc::new(Broker {
        hosts: Mutex::new(HashMap::new()),
        attempts: Mutex::new(HashMap::new()),
        collectors: Mutex::new(HashMap::new()),
        slurm: Mutex::new(HashMap::new()),
        shutdown: tokio::sync::Notify::new(),
        stopping: std::sync::atomic::AtomicBool::new(false),
        endpoint,
    });
    let limit = Arc::new(Semaphore::new(128));
    loop {
        let (mut stream, _) = tokio::select! {
            _ = broker.shutdown.notified() => break,
            accepted = listener.accept() => accepted?,
        };
        let Ok(permit) = limit.clone().try_acquire_owned() else {
            continue;
        };
        let broker = broker.clone();
        let limits = limit.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let request: Result<Request> =
                match tokio::time::timeout(Duration::from_secs(5), wire::read(&mut stream)).await {
                    Ok(result) => result,
                    Err(_) => return,
                };
            let Ok(request) = request else { return };
            if request.token != broker.endpoint.token || request.version != VERSION {
                return;
            }
            let idle_only = request.operation["op"] == "shutdown_if_idle";
            let shutting_down = idle_only || request.operation["op"] == "shutdown";
            let result = if shutting_down {
                if limits.available_permits() == 127
                    && broker
                        .attempts
                        .lock()
                        .await
                        .values()
                        .all(|attempt| attempt.result.is_some())
                    && crate::store::Store::open()
                        .and_then(|store| store.count_active_runs())
                        .is_ok_and(|count| count == 0)
                {
                    broker
                        .stopping
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    let mut error = None;
                    for host in broker.hosts.lock().await.values() {
                        if let Some(session) = &host.lock().await.session {
                            if let Err(failure) = session.stop_and_wait().await {
                                error = Some(failure);
                            }
                        }
                    }
                    match error {
                        Some(error) => {
                            broker
                                .stopping
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            Err(error)
                        }
                        None => Ok(json!({})),
                    }
                } else if idle_only {
                    Ok(json!({"busy":true}))
                } else {
                    Err(anyhow!(
                        "SSH monitoring is busy; finish active work before moving storage."
                    ))
                }
            } else {
                broker.dispatch(request.target, request.operation).await
            };
            let stop = shutting_down && result.as_ref().is_ok_and(|value| value["busy"] != true);
            let response = match result {
                Ok(value) => Response {
                    value: Some(value),
                    error: None,
                },
                Err(error) => Response {
                    value: None,
                    error: Some(error.to_string()),
                },
            };
            let _ = wire::write(&mut stream, &response).await;
            if stop {
                broker.shutdown.notify_one();
            }
        });
    }
    Ok(())
}

pub(crate) async fn shutdown_idle() -> Result<()> {
    if let Ok(endpoint) = endpoint() {
        if let Err(error) = exchange(&endpoint, None, json!({"op":"shutdown"})).await {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
            {
                return Ok(());
            }
            return Err(error);
        }
        for _ in 0..100 {
            if TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, endpoint.port))
                .await
                .is_err()
            {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        return Err(anyhow!(
            "SSH monitoring service has not stopped yet; retry the storage move."
        ));
    }
    Ok(())
}

pub(crate) async fn askpass() -> Result<()> {
    let path = std::path::PathBuf::from(
        std::env::var_os("ORX_SSH_ASKPASS_ENDPOINT")
            .ok_or_else(|| anyhow!("Missing SSH prompt bridge"))?,
    );
    let bridge: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let endpoint: Endpoint = serde_json::from_value(bridge["endpoint"].clone())?;
    let attempt = bridge["attempt"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing SSH authentication attempt"))?;
    let prompt = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "SSH authentication".into());
    let value = exchange(
        &endpoint,
        None,
        json!({"op":"askpass", "attempt":attempt, "prompt":prompt}),
    )
    .await?;
    println!(
        "{}",
        value["answer"]
            .as_str()
            .ok_or_else(|| anyhow!("SSH prompt cancelled"))?
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn large_command_input_crosses_bounded_frames_without_losing_bytes() {
        let input = (0..2 * 1024 * 1024 + 17)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        let expected = input.clone();
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let endpoint = Endpoint {
            port: listener.local_addr().unwrap().port(),
            token: "test".into(),
            version: VERSION,
        };
        let server = tokio::spawn(async move {
            let mut received = Vec::new();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request: Request = wire::read(&mut stream).await.unwrap();
                assert_eq!(request.token, "test");
                let operation = request.operation;
                assert_eq!(operation["lane"], "control");
                let result = match operation["op"].as_str().unwrap() {
                    "upload_start" => json!({"token":"upload"}),
                    "upload_chunk" => {
                        assert_eq!(operation["token"], "upload");
                        received.extend(
                            base64::engine::general_purpose::STANDARD
                                .decode(operation["data"].as_str().unwrap())
                                .unwrap(),
                        );
                        json!({})
                    }
                    "upload_commit" => {
                        assert_eq!(operation["token"], "upload");
                        assert_eq!(operation["command"], "consume");
                        assert_eq!(received, expected);
                        json!({"code":0,"stdout":"YWNjZXB0ZWQ=","stderr":""})
                    }
                    other => panic!("Unexpected input operation: {other}"),
                };
                wire::write(
                    &mut stream,
                    &Response {
                        value: Some(result),
                        error: None,
                    },
                )
                .await
                .unwrap();
                if operation["op"] == "upload_commit" {
                    break;
                }
            }
        });
        let result = execute_at(
            &endpoint,
            &SshTarget::alias("lab"),
            "consume",
            Some(&input),
            "control",
        )
        .await;
        assert_eq!(result.unwrap(), "accepted");
        server.await.unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn private_directory_ignores_inherited_powershell_module_path() {
        const CHILD_DIR: &str = "ORX_TEST_PRIVATE_DIRECTORY_MODULE_PATH";
        if let Some(path) = std::env::var_os(CHILD_DIR) {
            private_directory(Path::new(&path)).unwrap();
            return;
        }

        let dir = crate::local::git::TemporaryDirectory::new("orx-private-module-path").unwrap();
        let modules = dir.path().join("modules with spaces");
        let security = modules.join("Microsoft.PowerShell.Security");
        std::fs::create_dir_all(&security).unwrap();
        // Model an incompatible module earlier on PowerShell 7's inherited search path.
        std::fs::write(
            security.join("Microsoft.PowerShell.Security.psd1"),
            r#"@{
RootModule = 'Microsoft.PowerShell.Security.psm1'
ModuleVersion = '99.0'
FunctionsToExport = @('Get-Acl', 'Set-Acl')
}"#,
        )
        .unwrap();
        std::fs::write(
            security.join("Microsoft.PowerShell.Security.psm1"),
            "throw 'incompatible module'\nfunction Get-Acl {}\nfunction Set-Acl {}\n",
        )
        .unwrap();
        // Including the built-in path last preserves the inherited ordering on startup.
        let system_modules = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join(r"System32\WindowsPowerShell\v1.0\Modules");
        let module_path = std::env::join_paths([modules, system_modules]).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "jobs::ssh::service::tests::private_directory_ignores_inherited_powershell_module_path",
                "--nocapture",
            ])
            .env(CHILD_DIR, dir.path().join("ssh service"))
            .env("PSModulePath", module_path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}
