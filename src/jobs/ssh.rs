//! SSH backend — run an experiment as a detached process on your own box.
//!
//! No scheduler: the target is a plain server you can `ssh` into. Everything
//! shells out to the `ssh` binary (like the k8s backend shells out to
//! `kubectl`), so auth is your `~/.ssh/config` + agent/keys — orx never reads a
//! key. On unix, connections are multiplexed (ControlMaster) so the many status/log
//! polls reuse one TCP session instead of a handshake apiece.
//! Win32-OpenSSH cannot, so on Windows background calls need an agent key or no passphrase.
//!
//! The handle is a remote run directory `~/.orx/runs/<run_id>/` holding:
//!   run.sh      the launcher (exported env + snapshot-and-run payload)
//!   log         merged stdout/stderr
//!   pid         the detached process-group leader
//!   exit_code   written when the payload finishes
//! A restarted `orx supervise` reattaches purely from that directory.

use std::collections::HashMap;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::error::{anyhow, Result};

/// Keep sockets out of config paths, which can exceed macOS's 104-byte limit.
#[cfg(unix)]
fn control_dir() -> PathBuf {
    use std::hash::{Hash as _, Hasher as _};

    let uid = unsafe { libc::geteuid() };
    let mut namespace = std::collections::hash_map::DefaultHasher::new();
    crate::config::config_dir().hash(&mut namespace);
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

/// An ssh endpoint. The classic ssh backend connects by `~/.ssh/config` alias
/// (`SshTarget::alias`); backends that learn an endpoint at runtime (an
/// OpenResearch box on a provider-assigned host:port) pass an explicit
/// `user@host` plus the options no config file knows about.
#[derive(Debug, Clone)]
pub struct SshTarget {
    /// What goes after `--`: an alias, or `user@host`.
    pub dest: String,
    /// Extra ssh args before `--` (e.g. `["-p", "2222", "-o", …]`).
    pub extra_opts: Vec<String>,
    /// Whether batch calls to this endpoint can authenticate unaided. Defaults
    /// to [`SecondFactor::None`] from both constructors, so every backend that
    /// predates this field is unchanged.
    pub second_factor: SecondFactor,
}

/// Whether this endpoint's *batch* calls can authenticate on their own.
///
/// `BatchMode=yes` does not merely silence keyboard-interactive — OpenSSH's
/// `authmethod_is_enabled` honours each method's `batch_flag`, so BatchMode
/// **removes** kbdint from the client's candidate list. On a host configured
/// `AuthenticationMethods publickey,keyboard-interactive` the key clears stage
/// one, the server asks for stage two, and the client has nothing left to
/// offer. A multiplexed session skips authentication entirely, which is why a
/// live ControlMaster — not a key — is the real credential on such a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecondFactor {
    /// A key or agent is sufficient; `BatchMode=yes` authenticates cold. Every
    /// backend other than SGE.
    #[default]
    None,
    /// sshd demands `publickey,keyboard-interactive` (Duo/PAM — BU's SCC). A
    /// live ControlMaster is a hard prerequisite for every batch call.
    Required,
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
    /// A bare alias — `~/.ssh/config` alone decides the endpoint.
    pub fn alias(host: &str) -> Self {
        Self {
            dest: host.to_string(),
            extra_opts: Vec::new(),
            second_factor: SecondFactor::None,
        }
    }

    /// Opt this endpoint into the ControlMaster-is-mandatory contract.
    pub fn with_second_factor(mut self, second_factor: SecondFactor) -> Self {
        self.second_factor = second_factor;
        self
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
            second_factor: SecondFactor::None,
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
    // A 16-hex hash leaves room for ssh's temporary bind suffix. It folds in
    // the extra opts so different ports never share a control socket.
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    target.dest.hash(&mut h);
    target.extra_opts.hash(&mut h);
    control_dir().join(format!("{:016x}", h.finish()))
}

/// Shared ssh options: setup may prompt, background work never does; on unix one shared
/// socket lets a single login cover both.
fn ssh_opts(target: &SshTarget, batch: bool) -> Vec<String> {
    let mut opts = vec![
        "-o".into(),
        format!("BatchMode={}", if batch { "yes" } else { "no" }),
        "-o".into(),
        "ConnectTimeout=10".into(),
    ];
    opts.extend(multiplexing_opts(target));
    opts.extend(target.extra_opts.iter().cloned());
    opts
}

#[cfg(unix)]
fn multiplexing_opts(target: &SshTarget) -> Vec<String> {
    vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}", control_path(target).display()),
        "-o".into(),
        "ControlPersist=600".into(),
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
    for option in [
        "ExitOnForwardFailure=yes",
        "ServerAliveInterval=30",
        "ServerAliveCountMax=3",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
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

/// Arguments for the short interactive login opened by Settings. `true` ends
/// the visible session after authentication while ControlPersist keeps its
/// master available to the batch-mode calls below — on Windows there is no
/// master, so this only proves the host reachable and primes nothing.
pub(crate) fn interactive_args(target: &SshTarget) -> Result<Vec<String>> {
    prepare_control_dir()?;
    let mut args = ssh_opts(target, false);
    args.extend(["--".into(), target.dest.clone(), "true".into()]);
    Ok(args)
}

#[cfg(not(unix))]
pub(crate) async fn master_is_running(_target: &SshTarget) -> Result<bool> {
    Ok(false)
}

#[cfg(unix)]
pub(crate) async fn master_is_running(target: &SshTarget) -> Result<bool> {
    prepare_control_dir()?;
    let path = control_path(target);
    if !path.try_exists()? {
        return Ok(false);
    }
    let status = Command::new("ssh")
        .args(["-O", "check", "-S"])
        .arg(path)
        .arg("--")
        .arg(&target.dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|e| anyhow!("Could not check the SSH master: {e}"))?;
    Ok(status.success())
}

/// A batch call refused to run: this endpoint needs a second factor and its
/// multiplexed master is gone. Recoverable — nothing about the remote job
/// changed, only our transport — so callers `downcast_ref` to tell it apart
/// from a real remote failure and never mark a live job failed.
#[derive(Debug, Clone)]
pub struct MasterRequired {
    pub dest: String,
    /// Exactly what a human can run to re-establish it.
    pub login_command: String,
}

impl std::fmt::Display for MasterRequired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{host} requires a second factor (Duo) that orx's background connections cannot \
             answer. They ride one SSH session you open yourself:\n\n    {orx} ssh connect \
             {host}\n\nApprove the prompt; orx keeps that session alive for every submit, \
             status poll and log read. A plain `ssh {host}` will not do — orx uses its own \
             private ControlPath.",
            host = self.dest,
            orx = crate::invocation::orx(),
        )?;
        // The raw form, for anyone debugging outside orx — it carries the
        // private ControlPath that makes a plain `ssh` insufficient.
        if !self.login_command.is_empty() {
            write!(f, "\n\n(equivalently: {})", self.login_command)?;
        }
        Ok(())
    }
}

impl std::error::Error for MasterRequired {}

/// Does this ssh failure mean the second factor could never be attempted?
/// Backstop for the race where the master dies between check and exec.
pub fn is_second_factor_failure(message: &str) -> bool {
    message.contains("Permission denied") && message.contains("keyboard-interactive")
}

fn master_required(target: &SshTarget) -> MasterRequired {
    MasterRequired {
        dest: target.dest.clone(),
        login_command: master_login_command(target).unwrap_or_default(),
    }
}

/// `Ok(())` unless this endpoint needs a second factor and has no live master.
/// Free for every existing backend — the flag short-circuits before any syscall.
async fn require_master(target: &SshTarget) -> Result<()> {
    if target.second_factor == SecondFactor::None {
        return Ok(());
    }
    if master_is_running(target).await.unwrap_or(false) {
        return Ok(());
    }
    Err(master_required(target).into())
}

/// The exact shell command that re-establishes the multiplexed master these
/// batch calls ride on.
///
/// This is NOT `ssh <alias>`: orx keeps its control socket in a private
/// `/tmp/orx-ssh-<uid>-<hash>/` directory, so a plain login authenticates a
/// session orx cannot see. The rendered command carries orx's ControlPath.
pub fn master_login_command(target: &SshTarget) -> Result<String> {
    let args = interactive_args(target)?;
    let mut parts = vec!["ssh".to_string()];
    parts.extend(
        args.into_iter()
            .map(|a| if a.contains(' ') { sh_quote(&a) } else { a }),
    );
    Ok(parts.join(" "))
}

const MASTER_PROBE_TIMEOUT: Duration = Duration::from_secs(45);

/// Try to (re)establish the master WITHOUT a terminal.
///
/// Runs the interactive method — the only one that can reach
/// keyboard-interactive at all — but shaped so it is *structurally* incapable
/// of prompting, so it can never wedge a headless supervisor:
///   * `setsid()` before exec: no controlling terminal, so OpenSSH's
///     `open("/dev/tty")` fails and `readpassphrase(RPP_REQUIRE_TTY)` returns
///     ENOTTY instead of blocking. (`/dev/null` on stdin is NOT enough —
///     `read_passphrase` is called without `RP_ALLOW_STDIN` and reads the tty.)
///   * `SSH_ASKPASS_REQUIRE=force` with `SSH_ASKPASS` pointed at a path that
///     does not exist, and `DISPLAY` cleared.
///   * `NumberOfPasswordPrompts=1`: one empty answer, not three.
///   * `PasswordAuthentication=no`: BatchMode=no would otherwise re-enable
///     password auth and send that empty string as a unix password.
///   * an outer timeout, independent of the local OpenSSH version's behaviour.
///
/// Inside the cluster's second-factor grace window (SCC: 30 days per source
/// IP plus login node) PAM answers with zero prompts and this succeeds
/// silently, so
/// a master lost to laptop sleep or a network change self-heals. Outside it,
/// ssh exits in seconds. Returns `Ok(false)` — not an error — for the expired
/// grace: that is a normal, user-actionable state.
#[cfg(unix)]
pub(crate) async fn ensure_master_headless(target: &SshTarget) -> Result<bool> {
    if master_is_running(target).await.unwrap_or(false) {
        return Ok(true);
    }
    prepare_control_dir()?;
    let mut args = ssh_opts(target, false);
    for option in [
        "NumberOfPasswordPrompts=1",
        "PasswordAuthentication=no",
        "KbdInteractiveAuthentication=yes",
    ] {
        args.extend(["-o".into(), option.into()]);
    }
    args.extend(["-T".into(), "--".into(), target.dest.clone(), "true".into()]);

    let mut cmd = Command::new("ssh");
    cmd.args(args)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("SSH_ASKPASS", "/nonexistent/orx-never-prompts")
        .env_remove("DISPLAY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // SAFETY: setsid() is async-signal-safe and the child is about to exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match tokio::time::timeout(MASTER_PROBE_TIMEOUT, cmd.status()).await {
        Ok(Ok(_)) => master_is_running(target).await,
        Ok(Err(e)) => Err(anyhow!("Could not run ssh: {e}")),
        Err(_) => Ok(false), // kill_on_drop reaps it
    }
}

/// Windows cannot multiplex, so there is no master to establish.
#[cfg(not(unix))]
pub(crate) async fn ensure_master_headless(_target: &SshTarget) -> Result<bool> {
    Ok(false)
}

/// A remote run dir rendered as a single shell word.
///
/// A relative dir keeps the historical `$HOME`-anchored meaning (the ssh and
/// slurm backends' `.orx/runs/<id>`); an ABSOLUTE dir is used verbatim and
/// single-quoted — the SGE backend roots its runs on a project filesystem
/// because SCC home dirs carry a hard 10 GB quota. The relative branch emits
/// the exact literal the callers used before, so those backends' remote
/// commands are unchanged.
pub(crate) fn remote_path(dir: &str) -> String {
    // Never `Path::is_absolute()`: this is a POSIX remote path being judged on
    // a possibly-Windows client.
    if dir.starts_with('/') {
        sh_quote(dir)
    } else {
        format!("\"$HOME/{dir}\"")
    }
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
    ssh_run_bytes(target, remote_cmd, stdin.map(str::as_bytes)).await
}

async fn ssh_run_bytes(
    target: &SshTarget,
    remote_cmd: &str,
    stdin: Option<&[u8]>,
) -> Result<String> {
    prepare_control_dir()?;
    require_master(target).await?;
    let mut cmd = Command::new("ssh");
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
        // Race: the master died between require_master and the exec.
        if target.second_factor == SecondFactor::Required && is_second_factor_failure(err) {
            return Err(master_required(target).into());
        }
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

async fn ssh_run_file(
    target: &SshTarget,
    remote_cmd: &str,
    source: &std::path::Path,
) -> Result<String> {
    prepare_control_dir()?;
    require_master(target).await?;
    let mut child = Command::new("ssh")
        .args(ssh_opts(target, true))
        .arg("--")
        .arg(&target.dest)
        .arg(remote_cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("Could not run ssh: {e}"))?;
    let mut file = tokio::fs::File::open(source).await?;
    if let Some(mut pipe) = child.stdin.take() {
        tokio::io::copy(&mut file, &mut pipe).await?;
        drop(pipe);
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| anyhow!("ssh wait failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        if target.second_factor == SecondFactor::Required && is_second_factor_failure(err) {
            return Err(master_required(target).into());
        }
        return Err(anyhow!("ssh {} failed: {}", target.dest, err));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Upload a content-addressed tar once, then materialize it into this run's
/// private `repo/` directory. Both the cache write and extraction are safe to
/// repeat after a client or supervisor restart.
pub async fn stage_source(
    target: &SshTarget,
    run_id: &str,
    archive: &std::path::Path,
    digest: &str,
) -> Result<String> {
    stage_source_under(target, None, run_id, archive, digest).await
}

/// As [`stage_source`], but rooted at `base` — an ABSOLUTE remote directory —
/// when given. `None` means `$HOME`. Returns the run dir: relative to `$HOME`
/// for `None`, absolute otherwise.
///
/// The presence probe is `test -r`, not `test -f`: under an absolute base the
/// tree may be a shared group filesystem, where another user's 0600 cache entry
/// exists but cannot be read — `-f` would report it present and the following
/// `tar -xf` would fail with EACCES.
pub async fn stage_source_under(
    target: &SshTarget,
    base: Option<&str>,
    run_id: &str,
    archive: &std::path::Path,
    digest: &str,
) -> Result<String> {
    let root = match base {
        Some(base) => format!("{}/.orx", base.trim_end_matches('/')),
        None => ".orx".to_string(),
    };
    let dir = format!("{root}/runs/{run_id}");
    let cache = format!("{root}/source/{digest}.tar");
    let (d, c) = (remote_path(&dir), remote_path(&cache));
    let runs = remote_path(&format!("{root}/runs"));
    let source = remote_path(&format!("{root}/source"));

    let present = ssh_run(
        target,
        &format!("test -r {c} && echo present || true"),
        None,
    )
    .await?;
    if present.trim() != "present" {
        let upload = format!(
            "umask 077; mkdir -p {source}; \
             tmp={c}.tmp.$$; cat > \"$tmp\" && mv \"$tmp\" {c}"
        );
        ssh_run_file(target, &upload, archive).await?;
    }
    ssh_run(
        target,
        &format!(
            "umask 077; mkdir -p {runs} {d}/repo; \
             chmod 700 {runs} {d} {d}/repo; \
             tar -xf {c} -C {d}/repo"
        ),
        None,
    )
    .await?;
    Ok(dir)
}

/// Single-quote a value for safe embedding in the remote bash script.
pub(crate) fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub struct SshJobSpec {
    /// Where to run: a config alias (the ssh backend) or an explicit endpoint.
    pub target: SshTarget,
    /// Names the remote run dir `~/.orx/runs/<run_id>`.
    pub run_id: String,
    /// The shared snapshot-and-run payload (`bash` script body).
    pub script: String,
    /// Exported inside run.sh on the remote (tokens, synced env).
    pub env: HashMap<String, String>,
}

/// Submit the job: write run.sh, launch it detached, record its pid. Returns
/// the remote run dir (relative to `$HOME`) — the reattach handle.
pub async fn run_job(spec: &SshJobSpec) -> Result<String> {
    let dir = format!(".orx/runs/{}", spec.run_id);
    let env = super::default_python_env(&spec.env);
    let exports: String = env
        .iter()
        .map(|(k, v)| format!("export {}={}", k, sh_quote(v)))
        .collect::<Vec<_>>()
        .join("\n");
    // run.sh: set up env, run the payload capturing all output to `log`, then
    // record the exit status. The payload runs in a SUBSHELL `( … )` — not a
    // `{ … }` group — so an `exit`/`set -e` failure inside it ends the subshell,
    // not run.sh, and we still reach `echo $? > exit_code`.
    let run_sh = format!(
        "#!/usr/bin/env bash\n{exports}\ncd \"$HOME/{dir}\" || exit 97\n(\n{script}\n) > log 2>&1\necho $? > exit_code\n",
        script = spec.script,
    );

    // Create the dir (owner-only) and write run.sh from stdin.
    let setup = format!(
        "mkdir -p \"$HOME/{dir}\" && chmod 700 \"$HOME/{dir}\" && cat > \"$HOME/{dir}/run.sh\"",
    );
    ssh_run(&spec.target, &setup, Some(&run_sh)).await?;

    // Launch detached so it survives the ssh channel closing. Prefer `setsid`
    // (new session → pid == pgid, so cancel can TERM the whole group); fall back
    // to `nohup` where setsid is absent (e.g. a macOS host). Record the pid.
    let launch = format!(
        "cd \"$HOME/{dir}\" && \
         if command -v setsid >/dev/null 2>&1; then setsid bash run.sh </dev/null >/dev/null 2>&1 & \
         else nohup bash run.sh </dev/null >/dev/null 2>&1 & fi; \
         echo $! > pid",
    );
    ssh_run(&spec.target, &launch, None).await?;
    Ok(dir)
}

/// Job state in the shared stage vocabulary (see `jobs::stage_to_run_status`).
#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
}

pub async fn inspect_job(target: &SshTarget, dir: &str) -> Result<JobState> {
    // exit_code present -> finished; pid alive -> running; pid dead & no
    // exit_code -> killed/crashed; no pid yet -> just starting.
    let cmd = format!(
        "d={d}; \
         if [ -f \"$d/exit_code\" ]; then echo \"EXIT $(cat \"$d/exit_code\")\"; \
         elif [ -f \"$d/pid\" ] && kill -0 \"$(cat \"$d/pid\")\" 2>/dev/null; then echo RUNNING; \
         elif [ -f \"$d/pid\" ]; then echo DEAD; else echo PENDING; fi",
        d = remote_path(dir),
    );
    let out = ssh_run(target, &cmd, None).await?;
    let out = out.trim();
    if let Some(code) = out.strip_prefix("EXIT ") {
        let code: i32 = code.trim().parse().unwrap_or(-1);
        return Ok(if code == 0 {
            JobState {
                stage: "COMPLETED".into(),
                message: None,
            }
        } else {
            JobState {
                stage: "ERROR".into(),
                message: Some(format!("exited with code {code}")),
            }
        });
    }
    Ok(match out {
        "RUNNING" | "PENDING" => JobState {
            stage: "RUNNING".into(),
            message: None,
        },
        "DEAD" => JobState {
            stage: "ERROR".into(),
            message: Some("process died without an exit code (killed?)".into()),
        },
        other => JobState {
            stage: "RUNNING".into(),
            message: Some(format!("unexpected inspect output: {other}")),
        },
    })
}

/// One poll of the remote log past `skip` lines. Unlike the streaming backends
/// this returns promptly (the supervisor loops every ~2s); `idle` is unused.
pub async fn stream_logs(
    target: &SshTarget,
    dir: &str,
    skip: u64,
    _idle: Duration,
    sink: &mut (dyn FnMut(&str) + Send),
) -> Result<u64> {
    let cmd = format!(
        "tail -n +{} {}/log 2>/dev/null || true",
        skip + 1,
        remote_path(dir)
    );
    let out = ssh_run(target, &cmd, None).await?;
    let mut seen = skip;
    // A trailing newline yields a final empty element under split('\n'); use
    // lines() which ignores it, matching the "one line = one log line" contract.
    for line in out.lines() {
        seen += 1;
        sink(line);
    }
    Ok(seen)
}

/// Cancel = TERM the process group if we have one (setsid case), else the pid
/// (nohup fallback). The negative-pid form targets the whole group.
pub async fn cancel_job(target: &SshTarget, dir: &str) -> Result<()> {
    let cmd = format!(
        "p=$(cat {d}/pid 2>/dev/null); \
         [ -n \"$p\" ] && {{ kill -TERM -\"$p\" 2>/dev/null || kill -TERM \"$p\" 2>/dev/null; }}; true",
        d = remote_path(dir),
    );
    ssh_run(target, &cmd, None).await?;
    Ok(())
}

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
    fn alias_target_adds_no_extra_opts() {
        let target = SshTarget::alias("mybox");
        assert_eq!(target.dest, "mybox");
        assert!(target.extra_opts.is_empty());
        // No `-p`/`-o Strict…` beyond the shared multiplexing opts.
        let shared = 4 + multiplexing_opts(&target).len(); // BatchMode, ConnectTimeout
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
        assert_eq!(opts[4..6], ["-o", "ControlPersist=600"]);
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
            extra_opts: vec!["-p".into(), port.into()],
            second_factor: SecondFactor::None,
        };
        assert_ne!(control_path(&mk("22022")), control_path(&mk("22023")));
        assert_eq!(control_path(&mk("22022")), control_path(&mk("22022")));
    }

    /// THE load-bearing invariant of the second-factor design. The master is
    /// primed by the Settings "Connect" button, which builds a plain
    /// `alias()` (i.e. `SecondFactor::None`). If the flag entered the control
    /// path hash, SGE's batch calls would look for a socket at a different
    /// path and Connect would silently stop helping.
    #[cfg(unix)]
    #[test]
    fn second_factor_does_not_change_the_control_path() {
        let plain = SshTarget::alias("scc1");
        let guarded = SshTarget::alias("scc1").with_second_factor(SecondFactor::Required);
        assert_eq!(control_path(&plain), control_path(&guarded));
    }

    /// The flag must not leak into the argv either — it only gates whether we
    /// spawn at all.
    #[test]
    fn batch_opts_are_identical_for_both_second_factor_values() {
        let plain = SshTarget::alias("scc1");
        let guarded = SshTarget::alias("scc1").with_second_factor(SecondFactor::Required);
        assert_eq!(ssh_opts(&plain, true), ssh_opts(&guarded, true));
    }

    /// Relative dirs keep the historical `$HOME` anchor byte-for-byte; an
    /// absolute dir is single-quoted and used verbatim.
    #[test]
    fn remote_path_anchors_relative_dirs_and_passes_absolute_through() {
        assert_eq!(remote_path(".orx/runs/r1"), "\"$HOME/.orx/runs/r1\"");
        assert_eq!(
            remote_path("/projectnb/herbdl/workspaces/herb/faridkar/.orx/runs/r1"),
            "'/projectnb/herbdl/workspaces/herb/faridkar/.orx/runs/r1'"
        );
    }

    /// The Duo-lapse signature, and the false positive it must not catch: a
    /// remote `Permission denied` from the command itself is exit 1, and never
    /// mentions keyboard-interactive.
    #[test]
    fn second_factor_failure_is_distinguished_from_a_remote_permission_error() {
        assert!(is_second_factor_failure(
            "ssh scc1 failed (exit 255): faridkar@scc1.bu.edu: Permission denied \
             (publickey,keyboard-interactive)."
        ));
        assert!(!is_second_factor_failure(
            "ssh scc1 failed (exit 1): mkdir: cannot create directory: Permission denied"
        ));
        assert!(!is_second_factor_failure(
            "ssh scc1 failed (exit 255): Connection timed out"
        ));
    }

    /// A target that needs no second factor must never touch the filesystem or
    /// fork `ssh -O check` — the gate is free for every existing backend.
    #[tokio::test]
    async fn require_master_is_free_for_targets_without_a_second_factor() {
        let plain = SshTarget::alias("definitely-not-a-real-host-orx-test");
        assert!(require_master(&plain).await.is_ok());
    }

    /// A guarded target with no live master fails fast with the typed,
    /// downcastable error rather than a raw ssh string.
    #[cfg(unix)]
    #[tokio::test]
    async fn require_master_rejects_a_guarded_target_without_a_master() {
        let guarded = SshTarget::alias("definitely-not-a-real-host-orx-test")
            .with_second_factor(SecondFactor::Required);
        let err = require_master(&guarded).await.unwrap_err();
        assert!(
            err.downcast_ref::<MasterRequired>().is_some(),
            "expected MasterRequired, got: {err}"
        );
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
