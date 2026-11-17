//! Children held by keepers (`crates/orx-keep`), so they outlive this server and its
//! successor adopts them. A child is spawned once, then attached to by each server
//! generation; its output is offset-addressed, and an owner acknowledges an offset only once
//! everything before it is durable, so a successor that attaches resumes exactly there.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use orx_keep::wire::{self, kind, Control, Endpoint, Hello, Welcome};
pub use orx_keep::wire::{Exit, Retain, Signal};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, watch};

use crate::error::{anyhow, Result};

/// How long a child outlives the last server attached to it: past any restart, short of
/// leaving work running long after orx is gone.
pub const ORPHAN: Duration = Duration::from_secs(5 * 60);

/// Agents' output: nothing is dropped, and past this the child waits to be read.
pub const LOSSLESS: Retain = Retain::Unacked { limit: 64 << 20 };

/// Set for a restart that hands its children to the next server: from then on no stop and no
/// drop ends a kept child.
static HANDING_OFF: AtomicBool = AtomicBool::new(false);

pub fn hand_off() {
    HANDING_OFF.store(true, Ordering::SeqCst);
}

pub fn handing_off() -> bool {
    HANDING_OFF.load(Ordering::SeqCst)
}

/// A frame for the keeper, and who waits to hear it was written.
type Frame = (u8, Vec<u8>, Option<oneshot::Sender<()>>);

pub fn root() -> PathBuf {
    crate::store::data_dir().join("keep")
}

pub enum Io {
    /// Stdin and stdout pipes; stderr appends to the file, if any.
    Pipes {
        stderr: Option<PathBuf>,
    },
    Pty {
        cols: u16,
        rows: u16,
    },
    /// A stdin pipe, and stdout and stderr in one stream, in the order the child wrote them.
    Merged,
}

/// One child to keep: what to run, as built by [`crate::process::command`], and the record a
/// successor reads to know whose it is.
pub struct Keep<'a> {
    pub command: &'a std::process::Command,
    pub io: Io,
    pub retain: Retain,
    pub owner: Value,
}

/// `owner.json`: whose child a keeper holds, for the next server to give it back. Read by
/// every later version, so variants and fields only grow; `state` belongs to the owner.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Owner {
    /// A chat's agent process: a resident child, a server, or one turn's child.
    Agent {
        harness: String,
        session_id: String,
        #[serde(default)]
        state: Value,
    },
    /// A command orx runs on an agent's behalf, such as an ACP terminal.
    AgentCommand {
        harness: String,
        session_id: String,
        command_id: String,
        #[serde(default)]
        state: Value,
    },
    /// A project terminal the page opened.
    Terminal {
        terminal_id: String,
        #[serde(default)]
        state: Value,
    },
    #[serde(other)]
    Unknown,
}

impl Owner {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("owner records serialize")
    }
}

/// A keeper directory, live or left behind. A failure to look is [`Liveness::Unknown`] or
/// [`Record::Unreadable`], which no caller may take for a keeper's absence.
pub struct Entry {
    pub id: String,
    pub owner: Record,
    pub liveness: Liveness,
    /// The child's exit, once the keeper recorded it.
    pub exit: Option<Exit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    /// The lock is missing or released.
    Dead,
    Unknown,
}

#[derive(Debug, PartialEq)]
pub enum Record {
    Known(Owner),
    Missing,
    Unreadable,
}

/// Every keeper directory under [`root`], for a starting server to adopt or sweep. What can't be
/// read about a keeper is logged and left unknown on its entry; a root that can't be listed is
/// an error, since nothing may be concluded about the keepers in it.
pub fn entries() -> Result<Vec<Entry>> {
    let root = root();
    let unlistable = |error: io::Error| anyhow!("Could not list {}: {error}", root.display());
    let read = match std::fs::read_dir(&root) {
        Ok(read) => read,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(unlistable(error)),
    };
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.map_err(unlistable)?;
        if !entry.file_type().map_err(unlistable)?.is_dir() {
            continue;
        }
        let dir = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let leaving = |error| eprintln!("orx up: leaving keeper {id} alone: {error:#}");
        let owner = match read_json(&dir.join(wire::OWNER_FILE)) {
            Ok(Some(owner)) => Record::Known(owner),
            Ok(None) => Record::Missing,
            Err(error) => {
                leaving(error);
                Record::Unreadable
            }
        };
        let liveness = match lock_held(&dir) {
            Ok(true) => Liveness::Alive,
            Ok(false) => Liveness::Dead,
            Err(error) => {
                leaving(error);
                Liveness::Unknown
            }
        };
        let exit = read_json(&dir.join(wire::EXIT_FILE)).unwrap_or_else(|error| {
            eprintln!("orx up: {error:#}");
            None
        });
        entries.push(Entry {
            id,
            owner,
            liveness,
            exit,
        });
    }
    Ok(entries)
}

/// Removes a keeper directory whose keeper has exited.
pub fn remove(id: &str) -> Result<()> {
    let dir = root().join(id);
    if lock_held(&dir)? {
        return Err(anyhow!("keeper {id} is still running"));
    }
    match std::fs::remove_dir_all(&dir) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(anyhow!(
            "Could not remove keeper directory {}: {error}",
            dir.display()
        )),
        _ => Ok(()),
    }
}

/// `None` when the file does not exist.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(anyhow!("Could not read {}: {error}", path.display())),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| anyhow!("Could not parse {}: {error}", path.display()))
}

/// A keeper holds its lock for its whole life; a missing lock file means no keeper.
fn lock_held(dir: &Path) -> Result<bool> {
    let path = dir.join(wire::LOCK_FILE);
    let lock = match std::fs::File::open(&path) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(anyhow!("Could not open {}: {error}", path.display())),
    };
    match lock.try_lock_shared() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(anyhow!("Could not lock {}: {error}", path.display()))
        }
    }
}

fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension("part");
    std::fs::write(&temporary, bytes)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| anyhow!("Could not write {}: {error}", path.display()))
}

fn utf8(value: &std::ffi::OsStr, what: &str) -> Result<String> {
    value
        .to_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{what} is not valid UTF-8: {}", value.to_string_lossy()))
}

/// Starts `keep.command` under a new keeper and attaches to it.
pub async fn spawn(keep: Keep<'_>) -> Result<Kept> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = root().join(&id);
    create_private_dir(&dir)?;
    let spawned = start_keeper(&dir, &keep).await;
    if let Err(error) = &spawned {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(anyhow!("{error}"));
    }
    attach(&id).await
}

fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|error| anyhow!("Could not create {}: {error}", dir.display()))?;
    #[cfg(unix)]
    for path in [dir, dir.parent().unwrap_or(dir)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| anyhow!("Could not restrict {}: {error}", path.display()))?;
    }
    Ok(())
}

async fn start_keeper(dir: &Path, keep: &Keep<'_>) -> Result<()> {
    publish(
        &dir.join(wire::OWNER_FILE),
        &serde_json::to_vec(&keep.owner)?,
    )?;
    let command = keep.command;
    let spec = wire::Spec {
        protocol: wire::PROTOCOL,
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        program: utf8(command.get_program(), "The program path")?,
        args: command
            .get_args()
            .map(|arg| utf8(arg, "An argument"))
            .collect::<Result<_>>()?,
        env: command
            .get_envs()
            .map(|(key, value)| {
                Ok((
                    utf8(key, "An environment variable name")?,
                    value
                        .map(|value| utf8(value, "An environment variable"))
                        .transpose()?,
                ))
            })
            .collect::<Result<_>>()?,
        env_clear: false,
        cwd: command
            .get_current_dir()
            .map(|dir| utf8(dir.as_os_str(), "The working directory"))
            .transpose()?,
        stdio: match &keep.io {
            Io::Pipes { stderr } => wire::Stdio::Pipes {
                stderr: stderr
                    .as_deref()
                    .map(|path| utf8(path.as_os_str(), "The log path"))
                    .transpose()?,
            },
            Io::Pty { cols, rows } => wire::Stdio::Pty {
                cols: *cols,
                rows: *rows,
            },
            Io::Merged => wire::Stdio::Merged,
        },
        retain: keep.retain,
        orphan_seconds: ORPHAN.as_secs(),
    };

    let program = keeper_program().await?;
    let log = std::fs::File::create(dir.join(wire::LOG_FILE))
        .map_err(|error| anyhow!("Could not create the keeper log: {error}"))?;
    let mut keeper = crate::process::tokio_command(&program);
    keeper
        .arg("keep")
        .arg(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(log);
    let mut keeper = spawn_detached(keeper)
        .map_err(|error| anyhow!("Could not start a keeper ({}): {error}", program.display()))?;
    let mut stdin = keeper
        .stdin
        .take()
        .ok_or_else(|| anyhow!("keeper has no stdin"))?;
    stdin.write_all(&wire::json(&spec)).await?;
    drop(stdin);
    let mut stdout = keeper
        .stdout
        .take()
        .ok_or_else(|| anyhow!("keeper has no stdout"))?;
    let mut answer = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(15), async {
        let mut byte = [0; 1];
        while stdout.read(&mut byte).await? == 1 && byte[0] != b'\n' {
            answer.push(byte[0]);
        }
        io::Result::Ok(())
    })
    .await;
    // On Unix the process started here forks the keeper and exits; reap it.
    #[cfg(unix)]
    let _ = keeper.wait().await;
    let answer = String::from_utf8_lossy(&answer).trim().to_string();
    match read {
        Err(_) => Err(anyhow!(
            "The keeper did not start within 15 seconds; see {}",
            dir.join(wire::LOG_FILE).display()
        )),
        Ok(Err(error)) => Err(anyhow!("Could not read the keeper's answer: {error}")),
        Ok(Ok(())) if answer == "ready" => Ok(()),
        Ok(Ok(())) => match answer.strip_prefix("error: ") {
            Some(error) => Err(anyhow!("{error}")),
            None => Err(anyhow!(
                "The keeper exited before starting; see {}",
                dir.join(wire::LOG_FILE).display()
            )),
        },
    }
}

#[cfg(unix)]
fn spawn_detached(mut command: tokio::process::Command) -> io::Result<tokio::process::Child> {
    command.kill_on_drop(false).spawn()
}

/// No window, its own console group, and out of any job this server runs in, which would
/// end the keeper with the server.
#[cfg(windows)]
fn spawn_detached(mut command: tokio::process::Command) -> io::Result<tokio::process::Child> {
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    };
    let flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    orx_keep::private_stdio();
    command.kill_on_drop(false);
    match command
        .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
    {
        // A job that forbids breaking away keeps the keeper; it then ends with this server.
        Err(error) if error.raw_os_error() == Some(5) => command.creation_flags(flags).spawn(),
        result => result,
    }
}

/// Unix keepers run the extracted static `orx-host`, so an update or an unmounted AppImage
/// never takes their code away; Windows renames a running `orx.exe` aside instead.
async fn keeper_program() -> Result<PathBuf> {
    #[cfg(unix)]
    {
        // Resolving digests the embedded binary; once per data directory is enough.
        static RESOLVED: std::sync::Mutex<Option<(PathBuf, PathBuf)>> = std::sync::Mutex::new(None);
        let root = root();
        let cached = RESOLVED.lock().unwrap().clone();
        if let Some((resolved_root, host)) = cached {
            if resolved_root == root && host.is_file() {
                return Ok(host);
            }
        }
        let resolving = root.clone();
        match tokio::task::spawn_blocking(move || {
            crate::engine::transport::local_binary(&resolving)
        })
        .await
        {
            Ok(Ok(host)) => {
                *RESOLVED.lock().unwrap() = Some((root, host.clone()));
                return Ok(host);
            }
            // A test binary given `keep DIR` runs tests instead of keeping anything.
            Ok(Err(error)) if cfg!(test) => return Err(error),
            Ok(Err(error)) => {
                eprintln!("orx up: keeping children with this binary; no orx-host: {error:#}")
            }
            Err(error) => eprintln!("orx up: could not resolve orx-host: {error}"),
        }
    }
    #[cfg(all(test, windows))]
    {
        // Cargo nests unit-test binaries at a varying depth below the profile directory holding `orx`.
        let test = std::env::current_exe()?;
        return test
            .ancestors()
            .map(|dir| dir.join("orx.exe"))
            .find(|path| path.is_file())
            .ok_or_else(|| anyhow!("no orx.exe in any directory above {}", test.display()));
    }
    #[allow(unreachable_code)]
    crate::paths::spawnable_exe().map_err(|error| anyhow!("Could not find orx: {error}"))
}

/// Attaches to a live keeper, receiving everything it retains.
pub async fn attach(id: &str) -> Result<Kept> {
    let dir = root().join(id);
    let endpoint: Endpoint = read_json(&dir.join(wire::ENDPOINT_FILE))?
        .ok_or_else(|| anyhow!("Keeper {id} has no endpoint"))?;
    if !lock_held(&dir)? {
        return Err(anyhow!("Keeper {id} is not running"));
    }
    let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port))
        .await
        .map_err(|error| anyhow!("Could not reach keeper {id}: {error}"))?;
    stream.set_nodelay(true)?;
    let hello = Hello {
        protocol: wire::PROTOCOL,
        token: endpoint.token.clone(),
        from: 0,
    };
    write_frame(&mut stream, kind::HELLO, &wire::json(&hello)).await?;
    let (frame, body) = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut stream))
        .await
        .map_err(|_| anyhow!("Keeper {id} did not answer"))??;
    let welcome: Welcome = match frame {
        kind::WELCOME => wire::parse(&body)?,
        kind::REFUSED => {
            let refused: wire::Refused = wire::parse(&body)?;
            return Err(anyhow!("Keeper {id} refused: {}", refused.error));
        }
        other => return Err(anyhow!("Keeper {id} answered frame {other:#x}")),
    };

    let (frames, outgoing) = mpsc::unbounded_channel::<Frame>();
    let (chunks_tx, chunks) = mpsc::channel(64);
    let (exit_tx, exit) = watch::channel(None);
    let stopping = Arc::new(AtomicBool::new(false));
    tokio::spawn(link(
        stream,
        outgoing,
        chunks_tx,
        exit_tx,
        stopping.clone(),
        dir.clone(),
    ));
    Ok(assemble(
        Inner {
            id: id.to_string(),
            dir: Some(dir),
            child_pid: welcome.child_pid,
            start: welcome.start,
            next: welcome.next,
            frames,
            exit,
            detached: AtomicBool::new(false),
            acked: AtomicU64::new(welcome.start),
            stopping,
        },
        chunks,
    ))
}

/// Starts `command` as this server's own child, behind a kept child's interface: for short
/// work a restart waits for instead of handing off. Its stdin and stdout are pipes, stderr is
/// dropped, and it ends with its last handle or with this server.
pub fn spawn_here(command: &mut tokio::process::Command) -> Result<Kept> {
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let child = command.spawn()?;
    let (frames, outgoing) = mpsc::unbounded_channel::<Frame>();
    let (chunks_tx, chunks) = mpsc::channel(64);
    let (exit_tx, exit) = watch::channel(None);
    let child_pid = child.id().unwrap_or_default();
    tokio::spawn(hold(child, outgoing, chunks_tx, exit_tx));
    Ok(assemble(
        Inner {
            id: format!("here-{}", uuid::Uuid::new_v4().simple()),
            dir: None,
            child_pid,
            start: 0,
            next: 0,
            frames,
            exit,
            detached: AtomicBool::new(false),
            acked: AtomicU64::new(0),
            stopping: Arc::new(AtomicBool::new(false)),
        },
        chunks,
    ))
}

fn assemble(inner: Inner, chunks: mpsc::Receiver<(u64, Vec<u8>)>) -> Kept {
    let stdin = KeptStdin {
        frames: inner.frames.clone(),
        written: None,
    };
    let stdout = KeptStdout {
        chunks,
        current: Vec::new(),
        read: 0,
        position: inner.start,
    };
    Kept {
        handle: KeptHandle(Arc::new(inner)),
        stdin: Some(stdin),
        stdout: Some(stdout),
    }
}

/// Carries a child [`spawn_here`] started as [`link`] carries a keeper: input and controls in,
/// output and the exit out. Once every handle is gone the child goes too.
async fn hold(
    mut child: tokio::process::Child,
    mut outgoing: mpsc::UnboundedReceiver<Frame>,
    chunks: mpsc::Sender<(u64, Vec<u8>)>,
    exit: watch::Sender<Option<Exit>>,
) {
    let pid = child.id();
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let reading = tokio::spawn(async move {
        let mut chunks = Some(chunks);
        let mut offset = 0;
        let mut buffer = vec![0; 64 << 10];
        while let Ok(read @ 1..) = stdout.read(&mut buffer).await {
            // Without a reader the output is drained, so the child never blocks on it.
            if let Some(sender) = &chunks {
                if sender
                    .send((offset, buffer[..read].to_vec()))
                    .await
                    .is_err()
                {
                    chunks = None;
                }
            }
            offset += read as u64;
        }
    });
    let mut stopped = false;
    let mut handles = true;
    let status = loop {
        tokio::select! {
            frame = outgoing.recv(), if handles => match frame {
                Some((kind::INPUT, body, written)) => {
                    if let Some(input) = &mut stdin {
                        if input.write_all(&body).await.is_err() {
                            stdin = None;
                        }
                    }
                    if let Some(written) = written {
                        let _ = written.send(());
                    }
                }
                Some((kind::CONTROL, body, _)) => match wire::parse::<Control>(&body) {
                    Ok(Control::CloseStdin) => stdin = None,
                    Ok(Control::Stop | Control::Signal { signal: Signal::Kill }) => {
                        stopped = true;
                        crate::local::chat::kill_agent_tree(pid).await;
                        let _ = child.start_kill();
                    }
                    _ => {}
                },
                Some(_) => {}
                None => {
                    handles = false;
                    crate::local::chat::kill_agent_tree(pid).await;
                    let _ = child.start_kill();
                }
            },
            status = child.wait() => break status,
        }
    };
    // The exit follows the last output, unless something the child started still holds it.
    let unfinished_output = tokio::time::timeout(Duration::from_secs(1), reading)
        .await
        .is_err();
    #[cfg(unix)]
    let signal = status
        .as_ref()
        .ok()
        .and_then(std::os::unix::process::ExitStatusExt::signal);
    #[cfg(not(unix))]
    let signal = None;
    exit.send_replace(Some(Exit {
        code: status
            .as_ref()
            .ok()
            .and_then(std::process::ExitStatus::code),
        signal,
        finished_at: crate::store::now_ms() as u64,
        reason: stopped.then(|| "stopped".into()),
        unfinished_output,
    }));
}

async fn write_frame(stream: &mut (impl AsyncWrite + Unpin), kind: u8, body: &[u8]) -> Result<()> {
    let mut frame = Vec::with_capacity(body.len() + 5);
    frame.extend_from_slice(&(body.len() as u32 + 1).to_le_bytes());
    frame.push(kind);
    frame.extend_from_slice(body);
    stream.write_all(&frame).await?;
    Ok(())
}

async fn read_frame(stream: &mut (impl AsyncRead + Unpin)) -> Result<(u8, Vec<u8>)> {
    let mut len = [0; 4];
    stream.read_exact(&mut len).await?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > wire::MAX_FRAME {
        return Err(anyhow!("keeper frame length {len} out of range"));
    }
    let mut frame = vec![0; len];
    stream.read_exact(&mut frame).await?;
    let kind = frame.remove(0);
    Ok((kind, frame))
}

/// Carries one attachment: frames out, output and the exit in. A keeper that goes away
/// without an exit took its child with it.
async fn link(
    stream: TcpStream,
    mut outgoing: mpsc::UnboundedReceiver<Frame>,
    chunks: mpsc::Sender<(u64, Vec<u8>)>,
    exit: watch::Sender<Option<Exit>>,
    stopping: Arc<AtomicBool>,
    dir: PathBuf,
) {
    let (mut reader, mut writer) = stream.into_split();
    let writing = tokio::spawn(async move {
        while let Some((kind, body, written)) = outgoing.recv().await {
            if write_frame(&mut writer, kind, &body).await.is_err() {
                break;
            }
            if let Some(written) = written {
                let _ = written.send(());
            }
        }
    });
    let mut chunks = Some(chunks);
    while let Ok((frame, body)) = read_frame(&mut reader).await {
        match frame {
            kind::OUTPUT => {
                let Ok((offset, bytes)) = wire::split_offset(&body) else {
                    break;
                };
                if let Some(sender) = &chunks {
                    // A reader that went away still lets the link watch for the exit.
                    let _ = sender.send((offset, bytes.to_vec())).await;
                }
            }
            kind::EXIT => {
                // The exit follows the last output, so stdout ends here.
                chunks = None;
                if let Ok(recorded) = wire::parse::<Exit>(&body) {
                    exit.send_replace(Some(recorded));
                }
            }
            _ => {}
        }
    }
    drop(chunks);
    writing.abort();
    if exit.borrow().is_none() {
        exit.send_replace(Some(lost()));
    }
    // A stopped keeper exits after delivering the exit; its directory has served its purpose.
    if stopping.load(Ordering::SeqCst) {
        for _ in 0..50 {
            if matches!(lock_held(&dir), Ok(false)) {
                let _ = std::fs::remove_dir_all(&dir);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

struct Inner {
    id: String,
    /// The keeper's directory; `None` for a child held here.
    dir: Option<PathBuf>,
    child_pid: u32,
    start: u64,
    next: u64,
    frames: mpsc::UnboundedSender<Frame>,
    exit: watch::Receiver<Option<Exit>>,
    detached: AtomicBool,
    acked: AtomicU64,
    stopping: Arc<AtomicBool>,
}

impl Inner {
    fn control(&self, control: &Control) {
        let _ = self.frames.send((kind::CONTROL, wire::json(control), None));
    }

    fn request_stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.control(&Control::Stop);
    }
}

impl Drop for Inner {
    /// Like `kill_on_drop`: an owner that lets go of its child without handing it off ends it.
    fn drop(&mut self) {
        if !self.detached.load(Ordering::SeqCst) && !handing_off() {
            self.request_stop();
        }
    }
}

/// A kept child, split like a `tokio::process::Child`.
pub struct Kept {
    handle: KeptHandle,
    stdin: Option<KeptStdin>,
    stdout: Option<KeptStdout>,
}

impl Kept {
    pub fn handle(&self) -> KeptHandle {
        self.handle.clone()
    }

    pub fn take_stdin(&mut self) -> Option<KeptStdin> {
        self.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<KeptStdout> {
        self.stdout.take()
    }
}

/// Controls one kept child. The child ends when the last handle drops, unless detached.
#[derive(Clone)]
pub struct KeptHandle(Arc<Inner>);

impl KeptHandle {
    pub fn id(&self) -> &str {
        &self.0.id
    }

    pub fn pid(&self) -> u32 {
        self.0.child_pid
    }

    /// The offset this attachment's output starts at: everything before it was acknowledged.
    pub fn start(&self) -> u64 {
        self.0.start
    }

    /// The offset the child had written up to when this attachment began: output before it
    /// is replay of what an earlier server already saw.
    pub fn next(&self) -> u64 {
        self.0.next
    }

    /// Lets the keeper forget output before `offset`; call only once it is durable.
    pub fn ack(&self, offset: u64) {
        if self.0.acked.fetch_max(offset, Ordering::SeqCst) < offset {
            let _ = self
                .0
                .frames
                .send((kind::ACK, offset.to_le_bytes().to_vec(), None));
        }
    }

    pub fn signal(&self, signal: Signal) {
        self.0.control(&Control::Signal { signal });
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.0.control(&Control::Resize { cols, rows });
    }

    pub fn close_stdin(&self) {
        self.0.control(&Control::CloseStdin);
    }

    /// Replaces the record a successor reads to know whose this child is.
    pub fn set_owner(&self, owner: &Value) -> Result<()> {
        // A child held here, or a keeper that delivered its exit and was cleaned up, leaves no
        // successor to tell.
        let Some(dir) = self
            .0
            .dir
            .as_ref()
            .filter(|dir| self.try_wait().is_none() || dir.exists())
        else {
            return Ok(());
        };
        publish(&dir.join(wire::OWNER_FILE), &serde_json::to_vec(owner)?)
    }

    pub fn try_wait(&self) -> Option<Exit> {
        self.0.exit.borrow().clone()
    }

    pub async fn wait(&self) -> Exit {
        let mut exit = self.0.exit.clone();
        let recorded = match exit.wait_for(Option::is_some).await {
            Ok(exit) => exit.clone(),
            Err(_) => None,
        };
        recorded.unwrap_or_else(lost)
    }

    /// Ends the child's process tree and waits for its exit, unless this server is handing
    /// its children to the next.
    pub async fn stop(&self, timeout: Duration) -> Option<Exit> {
        if handing_off() {
            return None;
        }
        self.0.request_stop();
        tokio::time::timeout(timeout, self.wait()).await.ok()
    }

    /// Lets go without ending the child, for a successor to attach to.
    pub fn detach(&self) {
        self.0.detached.store(true, Ordering::SeqCst);
    }
}

fn lost() -> Exit {
    Exit {
        code: None,
        signal: None,
        finished_at: crate::store::now_ms() as u64,
        reason: Some("lost".into()),
        unfinished_output: false,
    }
}

/// The child's stdin; every write reaches it whole, as one frame. `flush` returns once the
/// keeper has been sent everything written, so an owner that records having answered the
/// child flushes first.
pub struct KeptStdin {
    frames: mpsc::UnboundedSender<Frame>,
    written: Option<oneshot::Receiver<()>>,
}

fn keeper_gone() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "the keeper is gone")
}

impl AsyncWrite for KeptStdin {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        // Frames go out in order, so hearing about the last one covers every earlier write.
        let (written, receiver) = oneshot::channel();
        if self
            .frames
            .send((kind::INPUT, buf.to_vec(), Some(written)))
            .is_err()
        {
            return Poll::Ready(Err(keeper_gone()));
        }
        self.written = Some(receiver);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let Some(written) = self.written.as_mut() else {
            return Poll::Ready(Ok(()));
        };
        let result = match Pin::new(written).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(Ok(())) => Ok(()),
            Poll::Ready(Err(_)) => Err(keeper_gone()),
        };
        self.written = None;
        Poll::Ready(result)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        let _ = self
            .frames
            .send((kind::CONTROL, wire::json(&Control::CloseStdin), None));
        Poll::Ready(Ok(()))
    }
}

/// The child's output, from [`KeptHandle::start`]; [`KeptStdout::position`] is the offset of
/// the next byte it returns.
pub struct KeptStdout {
    chunks: mpsc::Receiver<(u64, Vec<u8>)>,
    current: Vec<u8>,
    read: usize,
    position: u64,
}

impl KeptStdout {
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn lines(self) -> KeptLines {
        KeptLines {
            pending_start: self.position,
            skip: self.position,
            stdout: self,
            pending: Vec::new(),
        }
    }

    /// The next chunk of output and the offset it starts at; `None` at the end.
    pub async fn chunk(&mut self) -> Option<(u64, Vec<u8>)> {
        if self.read < self.current.len() {
            let offset = self.position;
            let rest = self.current.split_off(self.read);
            self.current.clear();
            self.read = 0;
            self.position += rest.len() as u64;
            return Some((offset, rest));
        }
        let (offset, bytes) = self.chunks.recv().await?;
        // Tail retention may have dropped output this reader never saw.
        self.position = offset + bytes.len() as u64;
        Some((offset, bytes))
    }
}

impl AsyncRead for KeptStdout {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = &mut *self;
        if this.read == this.current.len() {
            match this.chunks.poll_recv(cx) {
                Poll::Ready(Some((offset, bytes))) => {
                    this.position = offset;
                    this.current = bytes;
                    this.read = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
        let available = &this.current[this.read..];
        let taken = available.len().min(buf.remaining());
        buf.put_slice(&available[..taken]);
        this.read += taken;
        this.position += taken as u64;
        Poll::Ready(Ok(()))
    }
}

/// Lines of output, each with the offset just past its newline, so an owner can record
/// exactly how far it has applied.
pub struct KeptLines {
    stdout: KeptStdout,
    pending: Vec<u8>,
    /// The offset of `pending`'s first byte.
    pending_start: u64,
    skip: u64,
}

impl KeptLines {
    /// Drops output before `offset`, which an owner applied before this attachment. It must
    /// be a line boundary at or after where this attachment's output starts.
    pub fn skip_to(&mut self, offset: u64) -> io::Result<()> {
        if offset < self.pending_start {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "output before {} is gone; resuming at {offset} is impossible",
                    self.pending_start
                ),
            ));
        }
        self.skip = self.skip.max(offset);
        Ok(())
    }

    /// The next line without its line ending, and the offset after it. A final line without
    /// a newline is returned at the end of output.
    pub async fn next_line(&mut self) -> io::Result<Option<(String, u64)>> {
        loop {
            if self.pending_start < self.skip && !self.pending.is_empty() {
                let dropped = ((self.skip - self.pending_start) as usize).min(self.pending.len());
                if self.pending_start + dropped as u64 == self.skip
                    && self.pending[dropped - 1] != b'\n'
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("output offset {} is not at a line boundary", self.skip),
                    ));
                }
                self.pending.drain(..dropped);
                self.pending_start += dropped as u64;
            }
            if self.pending_start >= self.skip {
                if let Some(newline) = self.pending.iter().position(|byte| *byte == b'\n') {
                    let mut line: Vec<u8> = self.pending.drain(..=newline).collect();
                    self.pending_start += line.len() as u64;
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    return Ok(Some((
                        String::from_utf8_lossy(&line).into_owned(),
                        self.pending_start,
                    )));
                }
            }
            match self.stdout.chunk().await {
                Some((offset, bytes)) => {
                    if self.pending.is_empty() {
                        self.pending_start = offset;
                    }
                    self.pending.extend_from_slice(&bytes);
                }
                None if self.pending.is_empty() || self.pending_start < self.skip => {
                    return Ok(None)
                }
                None => {
                    let line = std::mem::take(&mut self.pending);
                    self.pending_start += line.len() as u64;
                    return Ok(Some((
                        String::from_utf8_lossy(&line).into_owned(),
                        self.pending_start,
                    )));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
