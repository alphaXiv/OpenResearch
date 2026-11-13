//! SSH backend — run an experiment as a detached process on your own box.
//!
//! No scheduler: the target is a plain server you can `ssh` into. Everything
//! shells out to the `ssh` binary (like the k8s backend shells out to
//! `kubectl`), so auth is your `~/.ssh/config` + agent/keys — orx never reads a
//! key. A detached local service shares one persistent helper channel per target.
//! Unix attaches only to an authenticated master; Windows keeps the explicitly
//! authenticated ssh process alive. Background user-host work never logs in.
//!
//! A run from orx 0.2 lives in a remote run directory `<root>/runs/<run_id>/` (root defaults to
//! `~/.orx`) holding:
//!   run.sh      the launcher (exported env + snapshot-and-run payload)
//!   log         merged stdout/stderr
//!   pid         the detached process-group leader
//!   exit_code   written when the payload finishes

pub(crate) mod cleanup;
mod container;
#[cfg(unix)]
mod prepared;
pub(crate) mod service;
#[cfg(unix)]
pub(crate) mod sharing;
pub use container::{
    resolve as resolve_container, validate_reference as validate_container_reference, ContainerRun,
};

#[cfg(unix)]
use std::path::PathBuf;
use std::process::Stdio;
#[cfg(unix)]
use std::time::Duration;

use crate::error::{anyhow, Result};

const ROUTE_FIELDS: &str = "hostname|user|port|controlpath|proxyjump|proxycommand|identityfile|certificatefile|identityagent|identitiesonly|pubkeyauthentication|passwordauthentication|kbdinteractiveauthentication|preferredauthentications|gssapiauthentication|hostbasedauthentication|hostkeyalias|stricthostkeychecking|userknownhostsfile|globalknownhostsfile|hostkeyalgorithms|pubkeyacceptedalgorithms|proxyusefdpass|bindaddress|bindinterface";

fn route_configuration(output: &str) -> String {
    // Match final can repeat unrelated list settings such as SendEnv.
    output
        .lines()
        .filter(|line| {
            line.split_once(' ')
                .is_some_and(|(key, _)| ROUTE_FIELDS.split('|').any(|field| field == key))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keep sockets out of config paths, which can exceed macOS's 104-byte limit.
#[cfg(unix)]
fn control_dir() -> PathBuf {
    control_dir_for(&crate::config::config_dir())
}

#[cfg(unix)]
fn control_dir_for(config_dir: &std::path::Path) -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};

    let uid = unsafe { libc::geteuid() };
    let mut namespace = std::collections::hash_map::DefaultHasher::new();
    config_dir.hash(&mut namespace);
    PathBuf::from("/tmp").join(format!("orx-ssh-{uid}-{:08x}", namespace.finish() as u32))
}

#[cfg(unix)]
fn prepare_control_dir() -> Result<()> {
    let dir = control_dir();
    std::fs::create_dir_all(&dir).map_err(|e| {
        anyhow!(
            "Could not create SSH control directory {}: {e}",
            dir.display()
        )
    })?;
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let metadata = std::fs::symlink_metadata(&dir)?;
    let uid = unsafe { libc::geteuid() };
    if !metadata.file_type().is_dir() || metadata.uid() != uid {
        return Err(anyhow!(
            "SSH control path {} is not an owner-controlled directory.",
            dir.display()
        ));
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&dir, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn prepare_control_dir() -> Result<()> {
    Ok(())
}

pub fn validate_host_options(options: &crate::config::SshHostSettings) -> Result<()> {
    if let Some(reference) = &options.container {
        validate_container_reference(reference)?;
    }
    if let Some(root) = &options.remote_root {
        validate_remote_root(root)?;
    }
    Ok(())
}

/// A configured remote storage root: absolute and free of shell metacharacters,
/// so it stays one path however a remote shell or batch script reads it.
pub fn validate_remote_root(root: &str) -> Result<()> {
    let components = root.strip_prefix('/').map(|rest| rest.split('/'));
    let valid = components.is_some_and(|mut components| {
        components.all(|part| {
            !matches!(part, "" | "." | "..")
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-+@".contains(c))
        })
    });
    if !valid {
        return Err(anyhow!(
            "Remote storage root must be an absolute path like /scratch/<user>/orx (letters, digits, . _ - + @; no trailing slash)."
        ));
    }
    Ok(())
}

/// A run's remote dir: under the configured root, or `$HOME`-relative by default.
pub fn remote_run_dir(root: Option<&str>, run_id: &str) -> String {
    match root {
        Some(root) => format!("{root}/runs/{run_id}"),
        None => format!(".orx/runs/{run_id}"),
    }
}

fn remote_source_dir(root: Option<&str>) -> String {
    match root {
        Some(root) => format!("{root}/source"),
        None => ".orx/source".into(),
    }
}

/// One shell word for a remote path; relative paths resolve against `$HOME`.
pub(crate) fn shell_path(path: &str) -> String {
    if path.starts_with('/') {
        sh_quote(path)
    } else {
        format!("\"$HOME/{path}\"")
    }
}

pub fn resolve_options(
    args: &crate::ExpRunArgs,
    settings: &crate::config::SshSettings,
) -> Result<(String, crate::config::SshHostSettings)> {
    let host = args
        .host
        .as_ref()
        .or(settings.default_host.as_ref())
        .filter(|host| !host.trim().is_empty())
        .ok_or_else(|| {
            anyhow!("SSH requires --host <alias> or a default host saved in SSH compute settings.")
        })?
        .clone();
    let saved = settings.hosts.get(&host).cloned().unwrap_or_default();
    let container = if args.no_container {
        None
    } else {
        args.container.clone().or_else(|| saved.container.clone())
    };
    let options = crate::config::SshHostSettings { container, ..saved };
    validate_host_options(&options)?;
    Ok((host, options))
}

/// An ssh endpoint. The classic ssh backend connects by `~/.ssh/config` alias
/// (`SshTarget::alias`); backends that learn an endpoint at runtime (an
/// OpenResearch box on a provider-assigned host:port) pass an explicit
/// `user@host` plus the options no config file knows about.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SshTarget {
    #[serde(skip)]
    pub(crate) transport:
        Option<std::sync::Arc<crate::engine::adapters::openresearch::LegacyTransport>>,
    /// What goes after `--`: an alias, or `user@host`.
    pub dest: String,
    #[serde(default)]
    pub managed_generation: Option<String>,
    #[serde(default)]
    pub auth_agent: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    /// Extra ssh args before `--` (e.g. `["-p", "2222", "-o", …]`).
    pub extra_opts: Vec<String>,
}

/// How to treat the remote's SSH host key for a `host_port` target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// `~/.ssh/config` + the user's own `known_hosts` decide everything — impose
    /// nothing. For a user-typed hostname they may already have pinned.
    UserConfig,
    /// `StrictHostKeyChecking=accept-new` against the user's real `known_hosts`:
    /// genuine trust-on-first-use — the first connection is accepted and
    /// recorded, and a later key change is caught. For a freshly-seen box
    /// identified by a raw IP (nothing to have pinned yet).
    AcceptNew,
    /// `StrictHostKeyChecking=no` + `UserKnownHostsFile=/dev/null`: accept any
    /// key every time, persist nothing. ONLY for machine-provisioned boxes whose
    /// proxy `host:port` pairs are recycled by the provider, where a real pin
    /// would just produce spurious mismatches (see `openresearch_ssh_target`).
    Ephemeral,
}

impl SshTarget {
    pub fn for_run(mut self, run_id: &str) -> Self {
        self.run_id = Some(run_id.to_owned());
        self
    }

    /// A bare alias — `~/.ssh/config` alone decides the endpoint.
    pub fn alias(host: &str) -> Self {
        Self {
            dest: host.to_string(),
            managed_generation: None,
            auth_agent: crate::local::shell_env::var("SSH_AUTH_SOCK")
                .map(|value| value.to_string_lossy().into_owned()),
            run_id: None,
            transport: None,
            extra_opts: Vec::new(),
        }
    }

    /// `dest` (an alias or `user@host`) on an explicit `port`, with an explicit
    /// host-key `policy`. Centralizes the `-p`/`-o` opt vector that both the
    /// `--remote` CLI and the openresearch backend need, so the host-key
    /// rationale lives in one place ([`HostKeyPolicy`]) instead of drifting
    /// across call sites.
    pub fn host_port(dest: String, port: u16, policy: HostKeyPolicy) -> Self {
        let mut extra_opts = vec!["-p".into(), port.to_string()];
        match policy {
            HostKeyPolicy::UserConfig => {}
            HostKeyPolicy::AcceptNew => {
                extra_opts.extend(["-o".into(), "StrictHostKeyChecking=accept-new".into()]);
            }
            HostKeyPolicy::Ephemeral => {
                extra_opts.extend([
                    "-o".into(),
                    "StrictHostKeyChecking=no".into(),
                    "-o".into(),
                    format!("UserKnownHostsFile={}", discarded_known_hosts().display()),
                    "-o".into(),
                    "LogLevel=ERROR".into(),
                ]);
            }
        }
        Self {
            dest,
            extra_opts,
            managed_generation: None,
            auth_agent: crate::local::shell_env::var("SSH_AUTH_SOCK")
                .map(|value| value.to_string_lossy().into_owned()),
            run_id: None,
            transport: None,
        }
    }
}

#[cfg(unix)]
fn discarded_known_hosts() -> PathBuf {
    PathBuf::from("/dev/null")
}

/// Windows' OpenSSH has no `/dev/null`, and would create a `\dev\null` on the current drive.
#[cfg(not(unix))]
fn discarded_known_hosts() -> std::path::PathBuf {
    crate::config::config_dir().join("ephemeral-known-hosts")
}

#[cfg(unix)]
fn control_path(target: &SshTarget) -> PathBuf {
    if target.extra_opts.is_empty() {
        return prepared::cached(target)
            .map(|connection| control_dir().join(connection.publication.socket_name))
            .unwrap_or_else(|| control_dir().join("%C"));
    }
    // A 16-hex hash leaves room for ssh's temporary bind suffix. It folds in
    // the extra opts so different ports never share a control socket.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    target.dest.hash(&mut h);
    target.extra_opts.hash(&mut h);
    target.auth_agent.hash(&mut h);
    control_dir().join(format!("{:016x}", h.finish()))
}

/// Shared ssh options: setup may prompt, background work never does; on unix one shared
/// socket lets a single login cover both.
fn ssh_opts(target: &SshTarget, batch: bool) -> Vec<String> {
    #[cfg(unix)]
    if target.extra_opts.is_empty() {
        if let Some(connection) = prepared::cached(target) {
            return ssh_opts_prepared(target, batch, &connection);
        }
    }
    ssh_options(target, batch, multiplexing_opts(target), None)
}

#[cfg(unix)]
fn ssh_opts_prepared(
    target: &SshTarget,
    batch: bool,
    connection: &prepared::Prepared,
) -> Vec<String> {
    ssh_options(
        target,
        batch,
        multiplexing_opts_for(
            &connection
                .path
                .with_file_name(&connection.publication.socket_name),
        ),
        Some(&connection.snapshot),
    )
}

fn ssh_options(
    target: &SshTarget,
    batch: bool,
    multiplexing: Vec<String>,
    config: Option<&std::path::Path>,
) -> Vec<String> {
    let mut opts = vec![
        "-o".into(),
        format!("BatchMode={}", if batch { "yes" } else { "no" }),
        "-o".into(),
        "ConnectTimeout=10".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "TCPKeepAlive=yes".into(),
    ];
    opts.extend(multiplexing);
    opts.extend(target.extra_opts.iter().cloned());
    if let Some(config) = config {
        opts.extend(["-F".into(), config.to_string_lossy().into_owned()]);
    }
    opts
}

#[cfg(unix)]
fn multiplexing_opts(target: &SshTarget) -> Vec<String> {
    multiplexing_opts_for(&control_path(target))
}

#[cfg(unix)]
fn multiplexing_opts_for(path: &std::path::Path) -> Vec<String> {
    vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}", path.display()),
        "-o".into(),
        "ControlPersist=86400".into(),
    ]
}

/// Win32-OpenSSH cannot multiplex, and a ControlPath inherited from ssh_config fails the
/// connection ("getsockname failed: Not a socket"); only explicit opts override it.
#[cfg(not(unix))]
fn multiplexing_opts(_target: &SshTarget) -> Vec<String> {
    vec![
        "-o".into(),
        "ControlMaster=no".into(),
        "-o".into(),
        "ControlPath=none".into(),
    ]
}

/// Arguments for a long-lived local forward, riding the same authenticated
/// ControlMaster as settings and background jobs where the platform has one.
pub(crate) fn forward_args(
    target: &SshTarget,
    forward: &str,
    remote_cmd: &str,
) -> Result<Vec<String>> {
    prepare_control_dir()?;
    let mut args = ssh_opts(target, true);
    args.extend(["-o".into(), "ExitOnForwardFailure=yes".into()]);
    args.extend([
        // No PTY: the remote session bearer is delivered over stdin and must
        // never be echoed by terminal line discipline.
        "-T".into(),
        "-L".into(),
        forward.into(),
        "--".into(),
        target.dest.clone(),
        remote_cmd.into(),
    ]);
    Ok(args)
}

pub(crate) struct InteractiveConnection {
    pub args: Vec<String>,
    #[cfg(unix)]
    _lock: crate::file_lock::FileLock,
}

/// Retires unusable Unix masters and holds their lock through the interactive login.
pub(crate) async fn prepare_interactive_connection(
    target: &SshTarget,
    persist: Option<u64>,
    remote_command: Option<String>,
) -> Result<InteractiveConnection> {
    prepare_control_dir()?;
    #[cfg(unix)]
    let connection = if target.extra_opts.is_empty() {
        Some(prepared::prepare(target, true).await?)
    } else {
        None
    };
    #[cfg(unix)]
    let path = connection
        .as_ref()
        .map(|connection| connection.path.clone())
        .unwrap_or_else(|| control_path(target));
    #[cfg(unix)]
    let options = connection
        .as_ref()
        .map(|connection| ssh_opts_prepared(target, false, connection))
        .unwrap_or_else(|| ssh_opts(target, false));
    #[cfg(not(unix))]
    let options = ssh_opts(target, false);
    #[cfg(unix)]
    let lock = lock_master(&path).await?;
    #[cfg(unix)]
    if control_master(target, &path, &options, "check").await? {
        if !master_accepts_sessions(target, &path, &options).await?
            && !control_master(target, &path, &options, "stop").await?
        {
            return Err(anyhow!("Could not retire the existing SSH connection; retry Connect once the gateway responds."));
        }
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut args = Vec::new();
    if let Some(seconds) = persist {
        args.extend(["-o".into(), format!("ControlPersist={seconds}")]);
    }
    args.extend(options);
    args.extend([
        "--".into(),
        target.dest.clone(),
        remote_command.unwrap_or_else(|| "true".into()),
    ]);
    Ok(InteractiveConnection {
        args,
        #[cfg(unix)]
        _lock: lock,
    })
}

#[cfg(unix)]
async fn lock_master(path: &std::path::Path) -> Result<crate::file_lock::FileLock> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    loop {
        match file.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    Ok(crate::file_lock::FileLock::new(file))
}

#[cfg(unix)]
async fn control_master(
    target: &SshTarget,
    path: &std::path::Path,
    options: &[String],
    operation: &str,
) -> Result<bool> {
    let mut command = crate::process::tokio_command("ssh");
    command
        .env("ORX_SSH_PROBE", "1")
        .args(["-O", operation, "-S"])
        .arg(path)
        .args(options)
        .arg("--")
        .arg(&target.dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(Duration::from_secs(10), command.status())
        .await
        .map_err(|_| anyhow!("Timed out trying to {operation} the SSH master; retry Connect once the gateway responds."))?
        .map_err(|e| anyhow!("Could not {operation} the SSH master: {e}"))?;
    Ok(status.success())
}

#[cfg(unix)]
async fn master_accepts_sessions(
    target: &SshTarget,
    path: &std::path::Path,
    options: &[String],
) -> Result<bool> {
    let mut command = crate::process::tokio_command("ssh");
    command
        .env("ORX_SSH_PROBE", "1")
        // Refused mux sessions can fall back to a new transport; this probe must not.
        .args([
            "-oBatchMode=yes",
            "-oControlMaster=no",
            "-oControlPersist=no",
            "-oProxyCommand=false",
            "-oClearAllForwardings=yes",
            "-oRemoteCommand=none",
            "-T",
            "-S",
        ])
        .arg(path)
        .args(options)
        .args(["--", &target.dest, "echo __ORX_SESSION_OK__"])
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| anyhow!("Timed out probing the existing SSH connection; retry Connect once the gateway responds."))??;
    Ok(output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == "__ORX_SESSION_OK__"))
}

#[cfg(unix)]
async fn resolved_control_path(target: &SshTarget) -> Result<PathBuf> {
    if !target.extra_opts.is_empty() {
        return Ok(control_path(target));
    }
    Ok(prepared::prepare(target, false).await?.path)
}

pub(crate) fn prepare_sharing_env(cmd: &mut tokio::process::Command) {
    cmd.env_remove("ORX_SSH_PROBE");
    #[cfg(unix)]
    cmd.env("ORX_SSH_CONTROL_DIR", control_dir());
    #[cfg(not(unix))]
    cmd.env_remove("ORX_SSH_CONTROL_DIR");
}

pub(crate) async fn setup_connection_sharing(target: &SshTarget) -> Option<String> {
    #[cfg(unix)]
    if target.extra_opts.is_empty() && target.transport.is_none() {
        return sharing::setup(target).await.err().map(|error| {
            format!("SSH login succeeded, but connection sharing is unavailable: {error}")
        });
    }
    #[cfg(not(unix))]
    let _ = target;
    None
}

/// Run a command on `target` over ssh, feeding `stdin` if given, returning stdout.
/// A non-zero exit is an error carrying stderr (the ssh/remote failure reason).
/// Shared with the slurm backend, which drives a cluster's login node the same
/// way, and the openresearch backend, which drives a provisioned box.
pub(crate) async fn ssh_run(
    target: &SshTarget,
    remote_cmd: &str,
    stdin: Option<&str>,
) -> Result<String> {
    if let Some(transport) = &target.transport {
        return transport
            .execute(remote_cmd, stdin.map(str::as_bytes).unwrap_or_default())
            .await;
    }
    service::execute(target, remote_cmd, stdin.map(str::as_bytes)).await
}

pub(crate) async fn direct_run(
    target: &SshTarget,
    command: &str,
    input: Option<&str>,
) -> Result<String> {
    direct_run_bytes(target, command, input.map(str::as_bytes)).await
}

pub(crate) async fn direct_run_bytes(
    target: &SshTarget,
    remote_cmd: &str,
    stdin: Option<&[u8]>,
) -> Result<String> {
    prepare_control_dir()?;
    #[cfg(unix)]
    if target.extra_opts.is_empty() {
        prepared::prepare(target, false).await?;
    }
    let mut cmd = crate::process::tokio_command("ssh");
    cmd.env("ORX_SSH_PROBE", "1");
    cmd.args(ssh_opts(target, true))
        .arg("--")
        .arg(&target.dest)
        .arg(remote_cmd)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow!("`ssh` not found on PATH — the SSH backend needs the OpenSSH client.")
        } else {
            anyhow!("Could not run ssh: {e}")
        }
    })?;
    if let Some(input) = stdin {
        use tokio::io::AsyncWriteExt as _;
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(input).await;
            drop(pipe); // EOF
        }
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| anyhow!("ssh wait failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(anyhow!(
            "ssh {} failed{}: {}",
            target.dest,
            out.status
                .code()
                .map(|c| format!(" (exit {c})"))
                .unwrap_or_default(),
            if err.is_empty() { "no stderr" } else { err }
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Single-quote a value for safe embedding in the remote bash script.
pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

const HOST_PROCESS_HELPERS: &str = r#"
host_process_alive() {
    local p="$1" stat fields pgid state file
    if [ -r "/proc/$p/stat" ]; then
        IFS= read -r stat < "/proc/$p/stat" || return 1
        stat=${stat##*) }
        read -ra fields <<< "$stat"
        if [ "${fields[0]}" != Z ] && [ "${fields[0]}" != X ]; then return 0; fi
        if [ "${fields[2]}" = "$p" ]; then
            for file in /proc/[0-9]*/stat; do
                IFS= read -r stat < "$file" 2>/dev/null || continue
                stat=${stat##*) }
                read -ra fields <<< "$stat"
                if [ "${fields[2]}" = "$p" ] && [ "${fields[0]}" != Z ] && [ "${fields[0]}" != X ]; then return 0; fi
            done
            return 1
        fi
        return 1
    else
        stat=$(ps -o pgid= -o stat= -p "$p" 2>/dev/null) || return 1
        read -r pgid state <<< "$stat"
        if [[ $state != Z* && $state != X* ]]; then return 0; fi
        if [ "$pgid" = "$p" ]; then
            ps -e -o pgid= -o stat= | awk -v p="$p" '$1 == p && $2 !~ /^[ZX]/ { alive=1 } END { exit !alive }'
        else
            return 1
        fi
    fi
}

host_group_alive() {
    local p="$1" file stat fields
    if [ -d /proc ]; then
        for file in /proc/[0-9]*/stat; do
            IFS= read -r stat < "$file" 2>/dev/null || continue
            stat=${stat##*) }
            read -ra fields <<< "$stat"
            if [ "${fields[2]}" = "$p" ] && [ "${fields[0]}" != Z ] && [ "${fields[0]}" != X ]; then return 0; fi
        done
        return 1
    fi
    ps -e -o pgid= -o stat= 2>/dev/null | awk -v p="$p" '$1 == p && $2 !~ /^[ZX]/ { alive=1 } END { exit !alive }'
}
"#;

/// Per-host readiness for the Settings UI: can we reach it and execute snapshots?
pub struct SshPreflight {
    pub reachable: bool,
    pub tools_found: bool,
    pub missing_tools: Vec<String>,
    pub error: Option<String>,
}

pub async fn preflight(target: &SshTarget) -> SshPreflight {
    match ssh_run(
        target,
        "command -v bash >/dev/null 2>&1 || echo MISSING_BASH; \
         command -v tar >/dev/null 2>&1 || echo MISSING_TAR",
        None,
    )
    .await
    {
        Ok(out) => {
            if let Some(warning) = setup_connection_sharing(target).await {
                eprintln!("orx: warning: {warning}");
            }
            let missing_tools = [("MISSING_BASH", "bash"), ("MISSING_TAR", "tar")]
                .into_iter()
                .filter(|(marker, _)| out.contains(marker))
                .map(|(_, tool)| tool.to_string())
                .collect::<Vec<_>>();
            let error = (!missing_tools.is_empty()).then(|| {
                format!(
                    "This host needs {} installed before orx can copy and run experiments. Install the missing tools, then retest.",
                    missing_tools.join(" and ")
                )
            });
            SshPreflight {
                reachable: true,
                tools_found: missing_tools.is_empty(),
                missing_tools,
                error,
            }
        }
        Err(e) => SshPreflight {
            reachable: false,
            tools_found: false,
            missing_tools: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_roots_are_absolute_plain_paths() {
        for root in ["/scratch/me/orx", "/data/user.name/orx-1", "/gpfs/a+b@c"] {
            validate_remote_root(root).unwrap();
        }
        for root in [
            "",
            "/",
            "scratch/orx",
            "~/orx",
            "/scratch/orx/",
            "/scratch//orx",
            "/scratch/../etc",
            "/scratch/$USER",
            "/scratch/a b",
            "/scratch/`id`",
            "/scratch/x;rm",
            "/scratch/'q'",
        ] {
            assert!(validate_remote_root(root).is_err(), "{root}");
        }
        assert_eq!(remote_run_dir(None, "r1"), ".orx/runs/r1");
        assert_eq!(
            remote_run_dir(Some("/scratch/orx"), "r1"),
            "/scratch/orx/runs/r1"
        );
        assert_eq!(shell_path(".orx/runs/r1"), "\"$HOME/.orx/runs/r1\"");
        assert_eq!(shell_path("/scratch/orx/runs/r1"), "'/scratch/orx/runs/r1'");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn interactive_login_removes_orphaned_sockets_and_broken_symlinks() {
        use std::os::unix::{fs::symlink, net::UnixListener};
        let target = SshTarget::alias(&format!("orx-test-{}", uuid::Uuid::new_v4()));
        prepare_control_dir().unwrap();
        let path = resolved_control_path(&target).await.unwrap();
        drop(UnixListener::bind(&path).unwrap());
        prepare_interactive_connection(&target, None, None)
            .await
            .unwrap();
        assert!(std::fs::symlink_metadata(&path).is_err());
        symlink(path.with_extension("missing"), &path).unwrap();
        let args = prepare_interactive_connection(&target, Some(120), None)
            .await
            .unwrap();
        assert!(std::fs::symlink_metadata(&path).is_err());
        assert_eq!(args.args[..2], ["-o", "ControlPersist=120"]);
        let socket = UnixListener::bind(&path).unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(100),
            prepare_interactive_connection(&target, None, None)
        )
        .await
        .is_err());
        assert!(path.exists());
        drop(socket);
        drop(args);
        assert!(prepare_interactive_connection(&target, None, None)
            .await
            .is_ok());
        let opts = ssh_opts(&target, true);
        for option in [
            "ServerAliveInterval=30",
            "ServerAliveCountMax=3",
            "TCPKeepAlive=yes",
        ] {
            assert!(opts.iter().any(|value| value == option));
        }
    }

    /// Replaces a forked child with `sleep`. Sleeping in place would hold every descriptor the
    /// test process had open, so other tests' pipes, sockets and locks would not close.
    #[cfg(unix)]
    fn exec_sleep() -> ! {
        let argv = [c"sleep".as_ptr(), c"60".as_ptr(), std::ptr::null()];
        unsafe {
            libc::execv(c"/bin/sleep".as_ptr(), argv.as_ptr());
            libc::_exit(127)
        }
    }

    #[cfg(unix)]
    #[test]
    fn host_probe_waits_for_live_group_members() {
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe { libc::setsid() };
            let member = unsafe { libc::fork() };
            if member == 0 {
                exec_sleep();
            }
            unsafe { libc::_exit(0) };
        }

        let mut zombie = false;
        for _ in 0..100 {
            let output = crate::process::command("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            zombie = String::from_utf8_lossy(&output.stdout)
                .trim()
                .starts_with('Z');
            if zombie {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let alive = || {
            crate::process::command("bash")
                .arg("-c")
                .arg(format!("{HOST_PROCESS_HELPERS}\nhost_process_alive {pid}"))
                .status()
                .unwrap()
                .success()
        };
        let with_member = alive();
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let mut without_member = false;
        for _ in 0..100 {
            if !alive() {
                without_member = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
        assert!(zombie);
        assert!(with_member);
        assert!(without_member);
    }

    #[test]
    fn alias_target_adds_no_extra_opts() {
        let target = SshTarget::alias("mybox");
        assert_eq!(target.dest, "mybox");
        assert!(target.extra_opts.is_empty());
        // No `-p`/`-o Strict…` beyond the shared multiplexing opts.
        let shared = 10 + multiplexing_opts(&target).len();
        assert_eq!(ssh_opts(&target, true).len(), shared);
    }

    #[test]
    fn host_port_policy_shapes_the_opt_vector() {
        // UserConfig: -p only, user's own config/known_hosts untouched.
        let t = SshTarget::host_port("root@h".into(), 2222, HostKeyPolicy::UserConfig);
        assert_eq!(t.extra_opts, vec!["-p".to_string(), "2222".to_string()]);

        // AcceptNew: real TOFU — accept-new, but NOT /dev/null.
        let t = SshTarget::host_port("root@h".into(), 2222, HostKeyPolicy::AcceptNew);
        let joined = t.extra_opts.join(" ");
        assert!(joined.contains("-p 2222"));
        assert!(joined.contains("StrictHostKeyChecking=accept-new"));
        assert!(!joined.contains("/dev/null"));

        // Ephemeral: provider box — accept-anything, persist nothing. Assert the
        // EXACT vector so the openresearch backend (which relies on this shape,
        // incl. LogLevel=ERROR and ordering) can't silently drift.
        let t = SshTarget::host_port("root@h".into(), 2222, HostKeyPolicy::Ephemeral);
        let (head, known_hosts, tail) = (&t.extra_opts[..5], &t.extra_opts[5], &t.extra_opts[6..]);
        assert_eq!(head, ["-p", "2222", "-o", "StrictHostKeyChecking=no", "-o"]);
        assert_eq!(tail, ["-o", "LogLevel=ERROR"]);
        #[cfg(unix)]
        assert_eq!(known_hosts, "UserKnownHostsFile=/dev/null");
        // By shape: on Windows it follows XDG_CONFIG_HOME, which telemetry tests mutate.
        #[cfg(not(unix))]
        assert!(
            known_hosts.starts_with("UserKnownHostsFile=")
                && known_hosts.ends_with("ephemeral-known-hosts")
        );
    }

    #[cfg(unix)]
    #[test]
    fn multiplexing_is_on_and_persistent() {
        let opts = multiplexing_opts(&SshTarget::alias("cluster"));
        assert_eq!(opts[0..2], ["-o", "ControlMaster=auto"]);
        assert!(opts[3].starts_with("ControlPath="));
        assert_eq!(opts[4..6], ["-o", "ControlPersist=86400"]);
    }

    /// Present and off, not absent: a user's own ssh_config would otherwise
    /// re-enable a ControlPath that fails the connection.
    #[cfg(not(unix))]
    #[test]
    fn multiplexing_is_disabled_not_omitted() {
        assert_eq!(
            multiplexing_opts(&SshTarget::alias("cluster")),
            vec!["-o", "ControlMaster=no", "-o", "ControlPath=none"],
        );
    }

    /// Explicit targets on the same host but different ports must not share a
    /// ControlMaster socket — the opts are part of the ControlPath hash.
    #[cfg(unix)]
    #[test]
    fn control_path_differs_per_port() {
        let control_path = |t: &SshTarget| {
            ssh_opts(t, true)
                .into_iter()
                .find(|o| o.starts_with("ControlPath="))
                .unwrap()
        };
        let mk = |port: &str| SshTarget {
            dest: "root@h".to_string(),
            managed_generation: None,
            auth_agent: crate::local::shell_env::var("SSH_AUTH_SOCK")
                .map(|value| value.to_string_lossy().into_owned()),
            run_id: None,
            transport: None,
            extra_opts: vec!["-p".into(), port.into()],
        };
        assert_ne!(control_path(&mk("22022")), control_path(&mk("22023")));
        assert_eq!(control_path(&mk("22022")), control_path(&mk("22022")));
    }

    #[cfg(unix)]
    #[test]
    fn control_path_fits_macos_unix_socket_limit() {
        let option = ssh_opts(
            &SshTarget::host_port("root@ssh3.vast.ai".into(), 22, HostKeyPolicy::Ephemeral),
            true,
        )
        .into_iter()
        .find(|o| o.starts_with("ControlPath="))
        .unwrap();
        let path = option.strip_prefix("ControlPath=").unwrap();

        assert!(path.starts_with("/tmp/orx-ssh-"));
        assert!(path.len() + 17 < 104, "{path}");
    }

    #[cfg(unix)]
    #[test]
    fn interactive_and_batch_modes_share_the_control_path() {
        let target = SshTarget::alias("cluster");
        let option = |batch| {
            ssh_opts(&target, batch)
                .into_iter()
                .find(|arg| arg.starts_with("ControlPath="))
                .unwrap()
        };

        assert_eq!(option(true), option(false));
    }

    #[test]
    fn batch_mode_follows_the_mode_flag() {
        let target = SshTarget::alias("cluster");
        assert!(ssh_opts(&target, true).contains(&"BatchMode=yes".to_string()));
        assert!(ssh_opts(&target, false).contains(&"BatchMode=no".to_string()));
    }
}
