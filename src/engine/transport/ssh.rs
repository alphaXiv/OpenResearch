use super::{askpass::Askpass, deadline, Config};
use crate::jobs::ssh::sh_quote;
use anyhow::{anyhow, bail, Context, Result};
use orx_host_proto::{Message, Request, Response, MAX_FRAME_LEN, PROTOCOL_VERSION};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;

pub(super) struct Session {
    child: Option<Child>,
    /// The SSH service's key for the login it holds for this session.
    #[cfg(windows)]
    held: Option<String>,
    pub(super) input: Box<dyn AsyncWrite + Send + Unpin>,
    pub(super) output: Option<BufReader<Box<dyn AsyncRead + Send + Unpin>>>,
    stderr: Option<JoinHandle<()>>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    bootstrapped: bool,
    authenticated: Arc<AtomicBool>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
        if let Some(stderr) = &self.stderr {
            stderr.abort();
        }
    }
}

pub(super) fn command(config: &Config) -> Command {
    let mut command = crate::process::tokio_command("ssh");
    if let Some(path) = &config.ssh_config {
        command.arg("-F").arg(path);
    }
    command
        .kill_on_drop(true)
        .env("LC_ALL", "C")
        .env("ORX_SSH_PROBE", "1")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never");
    // A ProxyJump hop ignores our BatchMode; with no terminal it fails instead of prompting.
    #[cfg(unix)]
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::DETACHED_PROCESS);
    command
}

pub(super) async fn route(config: &Config) -> Result<String> {
    let output = tokio::time::timeout(
        deadline(),
        command(config).args(["-G", "--", &config.alias]).output(),
    )
    .await??;
    if !output.status.success() {
        bail!("ssh -G failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let output = String::from_utf8(output.stdout)?;
    // Legacy route fingerprints are frozen for live 0.2.x supervisors.
    let fields: Vec<_> = output
        .lines()
        .filter(|line| {
            let key = line.split_whitespace().next().unwrap_or_default();
            matches!(
                key,
                "hostname"
                    | "user"
                    | "port"
                    | "proxyjump"
                    | "proxycommand"
                    | "identityfile"
                    | "certificatefile"
                    | "identityagent"
                    | "identitiesonly"
                    | "pubkeyauthentication"
                    | "passwordauthentication"
                    | "kbdinteractiveauthentication"
                    | "preferredauthentications"
                    | "gssapiauthentication"
                    | "hostbasedauthentication"
                    | "hostkeyalias"
                    | "stricthostkeychecking"
                    | "userknownhostsfile"
                    | "globalknownhostsfile"
                    | "hostkeyalgorithms"
                    | "pubkeyacceptedalgorithms"
                    | "proxyusefdpass"
                    | "bindaddress"
                    | "bindinterface"
                    | "canonicalizehostname"
            )
        })
        .collect();
    Ok(fields.join("\n"))
}

#[cfg(unix)]
pub(super) fn socket(config: &Config, route: &str) -> Result<PathBuf> {
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        std::fs::create_dir_all(&config.control_dir)?;
        let metadata = std::fs::symlink_metadata(&config.control_dir)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            bail!("SSH control directory must be owned by the current user");
        }
        std::fs::set_permissions(&config.control_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let key = format!("{:x}", Sha256::digest(format!("{}\0{route}", config.alias)));
    let path = config.control_dir.join(&key[..16]);
    if path.as_os_str().len() > 100 {
        bail!("SSH control path exceeds 100 bytes: {}", path.display());
    }
    Ok(path)
}

fn isolation(command: &mut Command) {
    command.args([
        "-T",
        "-o",
        "ClearAllForwardings=yes",
        "-o",
        "RemoteCommand=none",
        "-o",
        "ForwardAgent=no",
        "-o",
        "ForwardX11=no",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "ServerAliveInterval=15",
        "-o",
        "ServerAliveCountMax=3",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "NumberOfPasswordPrompts=1",
        "-o",
        "LogLevel=VERBOSE",
    ]);
}

#[cfg(unix)]
pub(super) async fn master_alive(config: &Config, path: &Path) -> Result<bool> {
    let check = tokio::time::timeout(
        Duration::from_secs(5),
        command(config)
            .arg("-S")
            .arg(path)
            .args([
                "-O",
                "check",
                "-o",
                "BatchMode=yes",
                "-o",
                "ProxyCommand=false",
                "--",
                &config.alias,
            ])
            .output(),
    )
    .await??;
    Ok(check.status.success())
}

#[cfg(unix)]
pub(super) async fn master(config: &Config, path: &Path, askpass: &Askpass) -> Result<bool> {
    if master_alive(config, path).await? {
        return Ok(false);
    }
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    let mut command = command(config);
    isolation(&mut command);
    command
        .args([
            "-M",
            "-N",
            "-f",
            "-o",
            "ControlPersist=yes",
            "-o",
            "BatchMode=no",
        ])
        .arg("-S")
        .arg(path)
        .args(["--", &config.alias]);
    askpass.configure(&mut command, &config.askpass_executable);
    let output =
        tokio::time::timeout(std::time::Duration::from_secs(300), command.output()).await??;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(true)
}

#[cfg(unix)]
pub(super) async fn withdraw(config: &Config, route: &str) {
    let Some(home) = &config.sharing_home else {
        return;
    };
    let result =
        async { crate::jobs::ssh::sharing::withdraw_engine(home, &socket(config, route)?).await }
            .await;
    if let Err(error) = result {
        eprintln!(
            "Host {}: could not withdraw SSH sharing: {error:#}",
            config.alias
        );
    }
}

#[cfg(unix)]
pub(super) async fn stop_master(config: &Config, route: &str) -> Result<()> {
    let path = socket(config, route)?;
    withdraw(config, route).await;
    if path.exists() {
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            command(config)
                .arg("-S")
                .arg(&path)
                .args(["-O", "exit", "--", &config.alias])
                .output(),
        )
        .await??;
        if !output.status.success() && path.exists() {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn prefix(route: &str) -> Result<String> {
    let hostname = route
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .ok_or_else(|| anyhow!("ssh -G did not report a hostname"))?;
    Ok(format!("Authenticated to {hostname} ("))
}

/// Keeps the last 16 KiB of `ssh -v` output, and latches once a line starts with `prefix`,
/// which may since have left that tail.
pub(crate) fn read_stderr(
    mut stderr: impl AsyncRead + Send + Unpin + 'static,
    prefix: String,
) -> (JoinHandle<()>, Arc<AtomicBool>, Arc<Mutex<Vec<u8>>>) {
    let authenticated = Arc::new(AtomicBool::new(false));
    let latched = authenticated.clone();
    let tail = Arc::new(Mutex::new(Vec::new()));
    let writer = tail.clone();
    let reader = tokio::spawn(async move {
        let prefix = prefix.into_bytes();
        let mut bytes = [0; 4096];
        let mut line = Vec::new();
        while let Ok(len @ 1..) = stderr.read(&mut bytes).await {
            for &byte in &bytes[..len] {
                if byte == b'\n' {
                    line.clear();
                } else if line.len() < prefix.len() {
                    line.push(byte);
                    if line == prefix {
                        latched.store(true, Ordering::Relaxed);
                    }
                }
            }
            let mut tail = writer.lock().unwrap();
            tail.extend_from_slice(&bytes[..len]);
            if tail.len() > 16 * 1024 {
                let excess = tail.len() - 16 * 1024;
                tail.drain(..excess);
            }
        }
    });
    (reader, authenticated, tail)
}

fn session_command(
    config: &Config,
    attach: bool,
    #[cfg_attr(windows, allow(unused_variables))] route: &str,
    askpass: Option<&Askpass>,
) -> Result<Command> {
    let mut command = command(config);
    isolation(&mut command);
    command.args(["-o", "ControlMaster=no"]);
    #[cfg(unix)]
    {
        command.args(["-o", "BatchMode=yes"]);
        if attach {
            command
                .arg("-S")
                .arg(socket(config, route)?)
                .args(["-o", "ProxyCommand=false"]);
        } else {
            command.args(["-o", "ControlPath=none"]);
        }
    }
    #[cfg(windows)]
    {
        let _ = attach;
        command.args(["-o", "ControlPath=none"]);
        if let Some(askpass) = askpass {
            command.args(["-o", "BatchMode=no"]);
            askpass.configure(&mut command, &config.askpass_executable);
        } else {
            command.args(["-o", "BatchMode=yes"]);
        }
    }
    #[cfg(unix)]
    let _ = askpass;
    command
        .args(["--", &config.alias])
        .arg(stub(&config.root))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

pub(super) fn spawn(
    config: &Config,
    attach: bool,
    route: &str,
    askpass: Option<&Askpass>,
) -> Result<Session> {
    let mut command = session_command(config, attach, route, askpass)?;
    let prefix = prefix(route)?;
    let mut child = command.spawn().context("start OpenSSH")?;
    let input = Box::new(child.stdin.take().unwrap());
    let output: Box<dyn AsyncRead + Send + Unpin> = Box::new(child.stdout.take().unwrap());
    let (stderr, authenticated, stderr_tail) = read_stderr(child.stderr.take().unwrap(), prefix);
    Ok(Session {
        child: Some(child),
        #[cfg(windows)]
        held: None,
        input,
        output: Some(BufReader::new(output)),
        stderr: Some(stderr),
        stderr_tail,
        bootstrapped: false,
        authenticated,
    })
}

#[cfg(windows)]
fn held_key(config: &Config, route: &str) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{}\0{}\0{route}", config.alias, config.root))
    )
}

#[cfg(windows)]
async fn held(operation: serde_json::Value, key: String) -> Result<Session> {
    let (stream, _) = crate::jobs::ssh::service::hold(operation).await?;
    let mut session = Session {
        child: None,
        held: Some(key),
        input: Box::new(tokio::io::empty()),
        output: None,
        stderr: None,
        stderr_tail: Arc::default(),
        bootstrapped: false,
        authenticated: Arc::default(),
    };
    session.connect(stream);
    Ok(session)
}

/// Logs in inside the SSH service, which keeps the login across `orx up` restarts.
#[cfg(windows)]
pub(super) async fn hold(config: &Config, route: &str, askpass: &Askpass) -> Result<Session> {
    let command = session_command(config, false, route, Some(askpass))?;
    let command = command.as_std();
    let text = |value: &std::ffi::OsStr| {
        value
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("ssh arguments must be Unicode"))
    };
    let args = command.get_args().map(text).collect::<Result<Vec<_>>>()?;
    let env = command
        .get_envs()
        .map(|(name, value)| {
            Ok(serde_json::json!([
                text(name)?,
                value.map(text).transpose()?
            ]))
        })
        .collect::<Result<Vec<_>>>()?;
    let key = held_key(config, route);
    let operation = serde_json::json!({
        "op": "hold_start",
        "key": key,
        "args": args,
        "env": env,
        "prefix": prefix(route)?,
    });
    held(operation, key).await
}

/// Attaches to the login the SSH service holds for this root, if it is still alive.
#[cfg(windows)]
pub(super) async fn attach(config: &Config, route: &str) -> Result<Session> {
    let key = held_key(config, route);
    let operation = serde_json::json!({"op": "hold_attach", "key": key});
    let mut session = held(operation, key).await?;
    session.bootstrapped = true;
    Ok(session)
}

#[cfg(windows)]
pub(super) async fn release(config: &Config, route: &str) {
    let operation = serde_json::json!({"op": "hold_stop", "key": held_key(config, route)});
    if let Err(error) = crate::jobs::ssh::service::hold(operation).await {
        eprintln!(
            "Host {}: could not end the held SSH login: {error:#}",
            config.alias
        );
    }
}

const EMBEDDED_HOSTS: &[(&str, &str, &str, &[u8])] =
    include!(concat!(env!("OUT_DIR"), "/host_binaries.rs"));

pub(super) struct Binary {
    sha: String,
    bytes: Vec<u8>,
}

#[cfg(unix)]
pub(crate) fn local_binary(root: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let os = if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    #[cfg(test)]
    let directory = Some(orx_test_support::host_binaries());
    #[cfg(not(test))]
    let directory: Option<PathBuf> = None;
    let mut native = None;
    if cfg!(debug_assertions)
        && directory.is_none()
        && std::env::var_os("ORX_HOST_BIN_DIR").is_none()
    {
        let executable = std::env::current_exe()?;
        if let Some(parent) = executable.parent() {
            let candidate = parent.join("orx-host");
            if candidate.is_file() {
                let bytes = std::fs::read(candidate)?;
                native = Some(Binary {
                    sha: format!("{:x}", Sha256::digest(&bytes)),
                    bytes,
                });
            }
        }
    }
    let binary = match native {
        Some(binary) => binary,
        None => binary(directory.as_deref(), os, std::env::consts::ARCH)?,
    };
    let cache = root
        .parent()
        .ok_or_else(|| anyhow!("Local run root has no parent"))?
        .join("host-bin");
    std::fs::create_dir_all(&cache)?;
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700))?;
    let path = cache.join(format!("orx-host-{}", binary.sha));
    if !path.try_exists()? {
        orx_host::publish_bytes(&path, &binary.bytes)?;
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

pub(super) fn binary(directory: Option<&Path>, os: &str, arch: &str) -> Result<Binary> {
    let os = match os {
        "Linux" => "linux",
        "Darwin" => "darwin",
        _ => bail!("Unsupported SSH host OS: {os}"),
    };
    let arch = match arch {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        _ => bail!("Unsupported SSH host architecture: {arch}"),
    };
    let Some(directory) = directory
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("ORX_HOST_BIN_DIR").map(PathBuf::from))
    else {
        let (_, _, sha, bytes) = EMBEDDED_HOSTS
            .iter()
            .find(|(host_os, host_arch, _, _)| *host_os == os && *host_arch == arch)
            .ok_or_else(|| anyhow!("No orx-host for {os}-{arch}. Release builds embed hosts; for development, run node scripts/build-host.mjs and set ORX_HOST_BIN_DIR to its output directory"))?;
        return Ok(Binary {
            sha: (*sha).into(),
            bytes: bytes.to_vec(),
        });
    };
    let universal = directory.join("orx-host-darwin-universal");
    let path = if os == "darwin" && universal.exists() {
        universal
    } else {
        directory.join(format!("orx-host-{os}-{arch}"))
    };
    let metadata = std::fs::metadata(&path).with_context(|| {
        format!(
            "Missing {}; run node scripts/build-host.mjs --out-dir {}",
            path.display(),
            directory.display()
        )
    })?;
    if metadata.len() == 0 || metadata.len() > 4 * 1024 * 1024 {
        bail!(
            "orx-host binary must be between 1 byte and 4 MiB: {}",
            path.display()
        );
    }
    let bytes = std::fs::read(&path)?;
    if bytes.len() as u64 != metadata.len() {
        bail!("orx-host binary changed while reading {}", path.display());
    }
    Ok(Binary {
        sha: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
    })
}

impl Session {
    pub async fn close(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }

    #[cfg(windows)]
    fn connect(&mut self, stream: tokio::net::TcpStream) {
        let (output, input) = stream.into_split();
        self.input = Box::new(input);
        self.output = Some(BufReader::new(Box::new(output)));
    }

    pub async fn handshake(&mut self) -> Result<(String, String)> {
        use tokio::io::AsyncBufReadExt;
        let mut output = self.output.as_mut().unwrap().take(16 * 1024);
        let mut line = Vec::new();
        for _ in 0..64 {
            line.clear();
            if output.read_until(b'\n', &mut line).await? == 0 {
                bail!("SSH connection closed");
            }
            if !line.ends_with(b"\n") {
                bail!("SSH bootstrap exceeded 16 KiB");
            }
            if !line.starts_with(b"ORX1 ") {
                continue;
            }
            let line = std::str::from_utf8(&line)?;
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 3 {
                bail!("Invalid orx-host bootstrap handshake: {line:?}");
            }
            self.bootstrapped = true;
            return Ok((fields[1].into(), fields[2].into()));
        }
        bail!("SSH bootstrap exceeded 64 lines")
    }

    pub async fn bootstrap(&mut self, binary: &Binary) -> Result<()> {
        use tokio::io::AsyncBufReadExt;
        self.input
            .write_all(format!("RUN {}\n", binary.sha).as_bytes())
            .await?;
        self.input.flush().await?;
        let mut ready = String::new();
        self.output
            .as_mut()
            .unwrap()
            .take(256)
            .read_line(&mut ready)
            .await?;
        if ready == "ORX1 missing\n" {
            self.input
                .write_all(format!("PUT {} {}\n", binary.sha, binary.bytes.len()).as_bytes())
                .await?;
            for bytes in binary.bytes.chunks(256 * 1024) {
                self.input.write_all(bytes).await?;
            }
            self.input.flush().await?;
            // BSD head can consume trailing pipe bytes; wait before sending Hello.
            ready.clear();
            self.output
                .as_mut()
                .unwrap()
                .take(256)
                .read_line(&mut ready)
                .await?;
        }
        if ready != "ORX1 ready\n" {
            bail!("Invalid bootstrap response: {ready:?}");
        }
        // The service relays a held login in whole frames once its unframed bootstrap is over.
        #[cfg(windows)]
        if let Some(key) = self.held.clone() {
            let operation = serde_json::json!({"op": "hold_attach", "key": key});
            self.connect(crate::jobs::ssh::service::hold(operation).await?.0);
        }
        let (_, _, sha) = self.hello().await?;
        if sha != binary.sha {
            bail!(
                "orx-host Hello reported binary {sha}, expected {}",
                binary.sha
            );
        }
        Ok(())
    }

    /// Checks that a held login's orx-host speaks this build's control protocol. An older
    /// binary that does keeps serving, and wrapping new runs, until the login ends.
    #[cfg(any(windows, test))]
    pub async fn resume(&mut self) -> Result<()> {
        self.hello()
            .await
            .context("The SSH login's orx-host is gone or incompatible; press Connect")?;
        Ok(())
    }

    /// Returns orx-host's OS, architecture and binary hash.
    async fn hello(&mut self) -> Result<(String, String, String)> {
        let hello = Request::Hello {
            proto: PROTOCOL_VERSION,
        };
        write(&mut self.input, &Message { id: 0, body: hello }).await?;
        loop {
            // A resumed login may first deliver replies meant for a server that went away.
            match read(self.output.as_mut().unwrap()).await? {
                Message {
                    id: 0,
                    body:
                        Response::Hello {
                            proto: PROTOCOL_VERSION,
                            binary_sha,
                            os,
                            arch,
                            ..
                        },
                } => return Ok((os, arch, binary_sha)),
                Message { id: 0, body } => bail!("orx-host Hello mismatch: {body:?}"),
                _ => {}
            }
        }
    }

    pub async fn failure(&mut self, error: anyhow::Error, retry_closure: bool) -> Failure {
        let mut code = None;
        if let Some(child) = &mut self.child {
            let status = match tokio::time::timeout(Duration::from_millis(200), child.wait()).await
            {
                Ok(status) => status.ok(),
                Err(_) => {
                    let _ = child.start_kill();
                    child.wait().await.ok()
                }
            };
            code = status.and_then(|status| status.code());
        }
        if let Some(stderr) = &mut self.stderr {
            let _ = tokio::time::timeout(Duration::from_secs(1), stderr).await;
        }
        #[cfg(windows)]
        if let Some(key) = &self.held {
            let operation = serde_json::json!({"op": "hold_stop", "key": key});
            if let Ok((_, ended)) = crate::jobs::ssh::service::hold(operation).await {
                code = ended["code"].as_i64().map(|code| code as i32);
                self.authenticated
                    .store(ended["authenticated"] == true, Ordering::Relaxed);
                let stderr = ended["stderr"].as_str().unwrap_or_default();
                *self.stderr_tail.lock().unwrap() = stderr.as_bytes().to_vec();
            }
        }
        let tail = self.stderr_tail.lock().unwrap();
        let stderr = String::from_utf8_lossy(&tail);
        let retry_closure = retry_closure || self.authenticated.load(Ordering::Relaxed);
        let mut failure = failure(&error.to_string(), &stderr, code, retry_closure);
        if self.bootstrapped
            || self.authenticated.load(Ordering::Relaxed) && matches!(code, Some(95..=98))
        {
            failure.breaker = false;
        }
        failure
    }
}

pub(super) async fn write(
    writer: &mut (impl AsyncWrite + Unpin),
    message: &Message<Request>,
) -> Result<()> {
    let mut bytes = Vec::new();
    orx_host_proto::write_frame(&mut bytes, message)?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}

pub(super) async fn read(reader: &mut (impl AsyncRead + Unpin)) -> Result<Message<Response>> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix).await?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        bail!("Invalid orx-host frame length: {len}");
    }
    let mut bytes = vec![0; len + 4];
    bytes[..4].copy_from_slice(&prefix);
    reader.read_exact(&mut bytes[4..]).await?;
    orx_host_proto::read_frame(&mut bytes.as_slice())?.ok_or_else(|| anyhow!("orx-host EOF"))
}

pub(super) struct Failure {
    pub cause: String,
    pub breaker: bool,
}

pub(super) fn failure(
    error: &str,
    stderr: &str,
    code: Option<i32>,
    retry_closure: bool,
) -> Failure {
    let lines: Vec<_> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with("Authenticated to ")
                && !line.starts_with("Transferred: ")
                && !line.starts_with("Bytes per second: ")
                && !line.starts_with("Killed by signal ")
        })
        .collect();
    let clean = |mut line: &str| {
        while let Some((head, rest)) = line.split_once(": ") {
            if head.chars().any(char::is_whitespace)
                || head.contains(['/', '\\'])
                || (head.len() == 1 && head.as_bytes()[0].is_ascii_alphabetic())
            {
                break;
            }
            line = rest;
        }
        line.to_owned()
    };
    let detail = lines.first().map(|first| {
        let first = clean(first);
        let last = clean(lines.last().unwrap());
        if first == last {
            if first.chars().count() > 200 {
                format!("{}…", first.chars().take(199).collect::<String>())
            } else {
                first
            }
        } else if first.chars().count() + last.chars().count() + 2 <= 200 {
            format!("{first}; {last}")
        } else {
            format!(
                "{}…; {}…",
                first.chars().take(98).collect::<String>(),
                last.chars().take(98).collect::<String>()
            )
        }
    });
    let remote = match code {
        Some(98) => Some("Could not prepare the remote run directory".into()),
        Some(97) => Some("The remote host rejected the bootstrap request".into()),
        Some(96) => Some("Could not install or verify the remote host binary".into()),
        Some(95) => Some("The remote host binary is missing".into()),
        Some(code) if code != 255 => Some(format!("Remote command exited with status {code}")),
        _ => None,
    };
    let lower = stderr.to_ascii_lowercase();
    for (needle, message) in [
        (
            "remote host identification has changed",
            "Host key verification failed",
        ),
        (
            "host key verification failed",
            "Host key verification failed",
        ),
        (
            "too many authentication failures",
            "Too many authentication failures",
        ),
        ("authentication failed", "Authentication failed"),
    ] {
        if lower.contains(needle) {
            return Failure {
                cause: message.into(),
                breaker: true,
            };
        }
    }
    if let Some(cause) = lines.iter().find_map(|line| {
        let start = line.to_ascii_lowercase().find("permission denied")?;
        Some(line[start..].trim_end_matches('.').to_owned())
    }) {
        return Failure {
            cause,
            breaker: true,
        };
    }
    let first = lines.first().copied().unwrap_or(error);
    let lower = first.to_ascii_lowercase();
    for (needle, message) in [
        ("connection refused", Some("Connection refused")),
        ("connection timed out", Some("Connection timed out")),
        ("operation timed out", Some("Connection timed out")),
        ("could not resolve hostname", None),
        ("no route to host", Some("No route to host")),
        ("network is unreachable", Some("Network is unreachable")),
        ("host is down", Some("Host is down")),
        ("network is down", Some("Network is down")),
        ("connection reset by peer", Some("Connection reset by peer")),
        ("connection closed by", None),
        ("kex_exchange_identification: connection closed", None),
        ("ssh_exchange_identification: connection closed", None),
    ] {
        if let Some(start) = lower.find(needle) {
            return Failure {
                cause: message
                    .map(str::to_owned)
                    .unwrap_or_else(|| first[start..].trim_end_matches('.').into()),
                breaker: !retry_closure && needle.contains("connection closed"),
            };
        }
    }
    let primary = remote.as_deref().unwrap_or(error);
    Failure {
        cause: detail.map_or_else(|| primary.into(), |detail| format!("{primary}: {detail}")),
        breaker: true,
    }
}

pub(super) fn stub(root: &str) -> String {
    let home_relative = root
        .strip_prefix("~/")
        .or_else(|| root.strip_prefix("$HOME/"));
    let quoted = sh_quote(home_relative.unwrap_or(root));
    let root = if home_relative.is_some() {
        format!("\"$HOME\"/{quoted}")
    } else {
        quoted
    };
    // csh expands history inside single quotes; avoid '!' in the bootstrap syntax.
    let script = format!(
        r#"umask 077; R={root};
mkdir -p "$R/bin" "$R/runs" "$R/sources" && chmod 700 "$R" "$R/bin" || exit 98;
printf 'ORX1 %s %s\n' "$(uname -s)" "$(uname -m)";
read_request() {{ IFS=' ' read -r verb sha len || return 1; [ "$sha" = "$(printf '%s' "$sha" | tr -cd '0-9a-f')" ] && [ "${{#sha}}" -eq 64 ]; }};
read_request || exit 97; B="$R/bin/orx-host-$sha";
if [ "$verb" = RUN ]; then
  if [ -x "$B" ]; then :; else printf 'ORX1 missing\n'; read_request || exit 97; [ "$verb" = PUT ] || exit 97; B="$R/bin/orx-host-$sha"; fi;
fi;
if [ "$verb" = PUT ]; then
  [ "$len" = "$(printf '%s' "$len" | tr -cd '0-9')" ] || exit 97;
  [ "$len" -gt 0 ] && [ "$len" -le 4194304 ] || exit 97;
  T="$B.$$"; trap 'rm -f "$T"' 0 HUP INT TERM;
  head -c "$len" > "$T" && [ "$(wc -c < "$T")" -eq "$len" ] && chmod 700 "$T" && "$T" verify-self "$sha" && mv -f "$T" "$B" || exit 96;
  trap - 0 HUP INT TERM;
elif [ "$verb" = RUN ]; then :; else exit 97; fi;
[ -x "$B" ] || exit 95; printf 'ORX1 ready\n'; exec "$B" serve --root "$R""#
    );
    format!("exec /bin/sh -c {}", sh_quote(&script.replace('\n', " ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_hosts_keep_their_digest_and_allow_directory_overrides() {
        for (_, _, sha, bytes) in EMBEDDED_HOSTS {
            assert_eq!(*sha, format!("{:x}", Sha256::digest(bytes)));
        }
        let directory =
            std::env::temp_dir().join(format!("orx-host-override-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        assert!(binary(Some(&directory), "Linux", "x86_64").is_err());
        std::fs::write(directory.join("orx-host-linux-x86_64"), b"explicit host").unwrap();
        let binary = binary(Some(&directory), "Linux", "x86_64").unwrap();
        assert_eq!(binary.bytes, b"explicit host");
        assert_eq!(
            binary.sha,
            format!("{:x}", Sha256::digest(b"explicit host"))
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(all(test, unix))]
mod properties;
