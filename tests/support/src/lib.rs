// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod process;
mod proxy;

pub use process::Process;
pub use proxy::{FaultProxy, Faults};

use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::ffi::{CStr, OsString};
use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub struct SshdBuilder {
    pub authentication_methods: String,
    /// Additional authorized keys, named client_key_1, client_key_2, etc.
    pub extra_client_keys: Vec<Option<String>>,
    pub identity_files: Vec<String>,
    pub max_sessions: u32,
    pub max_startups: String,
    pub login_grace_time: Duration,
    pub client_alive_interval: Duration,
    pub client_alive_count_max: u32,
}

impl Default for SshdBuilder {
    fn default() -> Self {
        Self {
            authentication_methods: "publickey".into(),
            extra_client_keys: Vec::new(),
            identity_files: vec!["client_key".into()],
            max_sessions: 10,
            max_startups: "10:30:100".into(),
            login_grace_time: Duration::from_secs(10),
            client_alive_interval: Duration::ZERO,
            client_alive_count_max: 3,
        }
    }
}

/// `orx-host` built for this machine (with test hooks), in the layout the engine reads from
/// `ORX_HOST_BIN_DIR`. Each build gets its own directory, so a rebuild never changes a binary
/// another test is uploading.
/// Stops the `orx up` that CLI calls autostarted for `data_dir`, and waits for it to exit.
/// An autostarted server outlives the test; one with unresolved runs never idles out. Each
/// signal goes only to the process the descriptor recorded, never to one that reused its pid.
pub fn stop_server(data_dir: &Path) {
    let Some(descriptor) = descriptor_path(data_dir).and_then(|path| fs::read_to_string(path).ok())
    else {
        return;
    };
    let number = |field: &str| {
        let rest = descriptor
            .split_once(&format!("\"{field}\":"))?
            .1
            .trim_start();
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse::<u64>().ok()
    };
    let (Some(pid), Some(starttime)) = (number("pid"), number("starttime")) else {
        return;
    };
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };
    let recorded = || pid > 1 && start_stamp(pid) == Some(starttime);
    let signal = |signal| recorded() && unsafe { libc::kill(pid, signal) } == 0;
    let exited = |timeout: Duration| {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !recorded() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    };
    if signal(libc::SIGTERM) && !exited(Duration::from_secs(10)) && signal(libc::SIGKILL) {
        exited(Duration::from_secs(5));
    }
}

/// Where `orx up` publishes its descriptor for `data_dir`.
fn descriptor_path(data_dir: &Path) -> Option<PathBuf> {
    use sha2::{Digest, Sha256};
    let data_dir = fs::canonicalize(data_dir).ok()?;
    let digest = Sha256::digest(data_dir.as_os_str().as_encoded_bytes());
    let name: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let uid = unsafe { libc::geteuid() };
    Some(PathBuf::from(format!("/tmp/orx-{uid}/{name}.server.json")))
}

/// The OS start stamp `orx up` records beside its pid; `None` once the process is gone.
#[cfg(target_os = "linux")]
fn start_stamp(pid: libc::pid_t) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat[stat.rfind(')')? + 1..].split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return None;
    }
    fields.get(19)?.parse().ok()
}

#[cfg(target_os = "macos")]
fn start_stamp(pid: libc::pid_t) -> Option<u64> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: `info` is a writable proc_bsdinfo of exactly `size` bytes.
    let written = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if written != size || info.pbi_status == libc::SZOMB {
        return None;
    }
    Some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

pub fn host_binaries() -> PathBuf {
    use std::hash::{Hash, Hasher};
    static BINARIES: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BINARIES
        .get_or_init(|| {
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let target = std::env::var_os("CARGO_TARGET_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| workspace.join("target"))
                .join("test-hooks");
            let output = Command::new(env!("CARGO"))
                .env("CARGO_PROFILE_DEV_DEBUG", "0")
                .env("CARGO_PROFILE_DEV_STRIP", "symbols")
                .env("CARGO_PROFILE_DEV_OPT_LEVEL", "1")
                .args([
                    "build",
                    "--locked",
                    "--features",
                    "test-hooks",
                    "-p",
                    "orx-host",
                ])
                .arg("--target-dir")
                .arg(&target)
                .current_dir(&workspace)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let binary = target.join("debug/orx-host");
            let mut digest = std::collections::hash_map::DefaultHasher::new();
            fs::read(&binary).unwrap().hash(&mut digest);
            let directory = target
                .join("native")
                .join(format!("{:016x}", digest.finish()));
            fs::create_dir_all(&directory).unwrap();
            let os = if cfg!(target_os = "macos") {
                "darwin"
            } else {
                "linux"
            };
            let destination = directory.join(format!("orx-host-{os}-{}", std::env::consts::ARCH));
            match fs::hard_link(binary, destination) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("Cannot publish test host binary: {error}"),
            }
            directory
        })
        .clone()
}

/// An isolated real SSH server reached through a live, controllable fault proxy.
/// Clients must use `ssh_args()` or `spawn()`; changing local HOME does not configure OpenSSH.
pub struct Sshd {
    daemon: Process,
    proxy: FaultProxy,
    user: String,
    real_home: PathBuf,
    directory: TempDir,
}

impl SshdBuilder {
    pub fn start(self) -> Result<Sshd> {
        let sshd = find_sshd()?;
        // Short paths also leave room for Unix-domain ControlPath sockets on macOS.
        let directory = tempfile::Builder::new()
            .prefix("orx-ssh-")
            .tempdir_in("/tmp")?;
        let root = directory.path();
        let (user, real_home) = account()?;
        let home = root.join("home");
        fs::create_dir(&home)?;
        let mut keys = vec![
            ("host_key".to_owned(), None),
            ("client_key".to_owned(), None),
        ];
        keys.extend(
            self.extra_client_keys
                .iter()
                .enumerate()
                .map(|(index, passphrase)| {
                    (format!("client_key_{}", index + 1), passphrase.as_deref())
                }),
        );
        let mut authorized_keys = String::new();
        for (name, passphrase) in keys {
            let mut command = Command::new("ssh-keygen");
            command
                .args(["-q", "-t", "ed25519", "-N", passphrase.unwrap_or(""), "-f"])
                .arg(root.join(&name));
            let output = Process::spawn(&mut command)?.wait(Duration::from_secs(5))?;
            if !output.status.success() {
                bail!(
                    "ssh-keygen failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            if name != "host_key" {
                authorized_keys.push_str(&fs::read_to_string(root.join(format!("{name}.pub")))?);
            }
        }
        fs::write(root.join("authorized_keys"), authorized_keys)?;
        let wrapper = root.join("command");
        fs::write(
            &wrapper,
            format!(
                r#"#!/bin/sh
HOME='{}'
export HOME
cd "$HOME" || exit 1
if [ -n "$SSH_ORIGINAL_COMMAND" ]; then
  exec /bin/sh -c "$SSH_ORIGINAL_COMMAND"
else
  exec /bin/sh
fi
"#,
                home.display()
            ),
        )?;
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700))?;
        let config = root.join("sshd_config");
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut attempts = 0;
        // sshd cannot inherit a bound listener. Retry only its bind race, never configuration errors.
        let (daemon, server_port) = 'bind: loop {
            attempts += 1;
            let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let server_port = reservation.local_addr()?.port();
            fs::write(
                &config,
                format!(
                    r#"ListenAddress 127.0.0.1
Port {server_port}
HostKey {root}/host_key
PidFile {root}/sshd.pid
AuthorizedKeysFile {root}/authorized_keys
StrictModes no
UsePAM no
PasswordAuthentication no
KbdInteractiveAuthentication no
AuthenticationMethods {authentication_methods}
PermitRootLogin yes
AllowUsers {user}
PermitUserRC no
AllowAgentForwarding no
AllowTcpForwarding no
X11Forwarding no
PermitTTY no
SetEnv HOME={home}
ForceCommand {root}/command
MaxSessions {sessions}
MaxStartups {startups}
LoginGraceTime {grace}
ClientAliveInterval {alive}
ClientAliveCountMax {count}
LogLevel VERBOSE
"#,
                    root = root.display(),
                    home = home.display(),
                    authentication_methods = self.authentication_methods,
                    sessions = self.max_sessions,
                    startups = self.max_startups,
                    grace = self.login_grace_time.as_secs(),
                    alive = self.client_alive_interval.as_secs(),
                    count = self.client_alive_count_max,
                ),
            )?;
            drop(reservation);
            let mut command = Command::new(&sshd);
            command.args(["-D", "-e", "-f"]).arg(&config);
            let process = Process::spawn(&mut command)?;
            loop {
                let log = process.stderr()?;
                if log.contains("Server listening on") {
                    break 'bind (process, server_port);
                }
                if log.contains("Address already in use") {
                    if attempts == 8 {
                        bail!("sshd could not bind after eight attempts: {log}");
                    }
                    break;
                }
                if !log.trim().is_empty() || Instant::now() >= deadline {
                    bail!("sshd did not become ready: {log}");
                }
                thread::sleep(Duration::from_millis(10));
            }
        };
        let proxy = FaultProxy::start(SocketAddr::from((Ipv4Addr::LOCALHOST, server_port)))?;
        let identities = if self.identity_files.is_empty() {
            "  IdentityFile none\n".to_owned()
        } else {
            self.identity_files
                .iter()
                .map(|name| format!("  IdentityFile {}\n", root.join(name).display()))
                .collect::<String>()
        };
        fs::write(
            root.join("ssh_config"),
            format!(
                r#"Host orx-test
  HostName 127.0.0.1
  Port {}
  User {user}
{identities}  IdentitiesOnly yes
  IdentityAgent none
  BatchMode yes
  StrictHostKeyChecking yes
  UserKnownHostsFile {}/known_hosts
  GlobalKnownHostsFile /dev/null
  ControlMaster no
  ControlPath {}/control-%C
  ControlPersist no
  ConnectTimeout 3
  ConnectionAttempts 1
  LogLevel ERROR
"#,
                proxy.port(),
                root.display(),
                root.display()
            ),
        )?;
        let public_key = fs::read_to_string(root.join("host_key.pub"))?;
        fs::write(
            root.join("known_hosts"),
            format!("[127.0.0.1]:{} {public_key}", proxy.port()),
        )?;
        Ok(Sshd {
            directory,
            daemon,
            proxy,
            user,
            real_home,
        })
    }
}

impl Sshd {
    pub fn start() -> Result<Self> {
        SshdBuilder::default().start()
    }
    /// Restarts the listener with the same keys, port, home and proxy.
    pub fn restart(&mut self) -> Result<()> {
        unsafe {
            libc::kill(self.daemon.pid() as i32, libc::SIGKILL);
        }
        self.daemon.wait(Duration::from_secs(5))?;
        let mut command = Command::new(find_sshd()?);
        command
            .args(["-D", "-e", "-f"])
            .arg(self.directory.path().join("sshd_config"));
        self.daemon = Process::spawn(&mut command)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.daemon.stderr()?.contains("Server listening on") {
            if Instant::now() >= deadline {
                bail!("restarted sshd did not listen: {}", self.daemon.stderr()?);
            }
            thread::sleep(Duration::from_millis(10));
        }
        self.proxy.reset()?;
        Ok(())
    }

    pub fn alias(&self) -> &str {
        "orx-test"
    }
    pub fn config_path(&self) -> PathBuf {
        self.directory.path().join("ssh_config")
    }
    pub fn port(&self) -> u16 {
        self.proxy.port()
    }
    pub fn user(&self) -> &str {
        &self.user
    }
    pub fn home(&self) -> PathBuf {
        self.directory.path().join("home")
    }
    pub fn real_home(&self) -> &Path {
        &self.real_home
    }
    pub fn directory(&self) -> &Path {
        self.directory.path()
    }
    pub fn pid(&self) -> u32 {
        self.daemon.pid()
    }
    pub fn proxy(&self) -> &FaultProxy {
        &self.proxy
    }

    /// Counts sshd auth_log events by (method, outcome); Postponed is an unsigned probe.
    /// Excludes method discovery (none); compare snapshots to assert a quiet window.
    pub fn auth_attempts(&self) -> Result<BTreeMap<(String, String), usize>> {
        Ok(parse_auth_attempts(&self.daemon.stderr()?))
    }

    /// Writes askpass and askpass-prompts in the harness directory; returns SSH_ASKPASS.
    pub fn askpass(&self, passphrase: &str) -> Result<PathBuf> {
        let root = self.directory.path();
        let answer = root.join("askpass-answer");
        fs::write(&answer, passphrase)?;
        fs::set_permissions(&answer, fs::Permissions::from_mode(0o600))?;
        fs::write(root.join("askpass-prompts"), "")?;
        let script = root.join("askpass");
        fs::write(
            &script,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$1" >> '{root}/askpass-prompts'
cat '{root}/askpass-answer'
printf '\n'
"#,
                root = root.display(),
            ),
        )?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))?;
        Ok(script)
    }

    pub fn ssh_args(&self) -> [OsString; 3] {
        [
            "-F".into(),
            self.config_path().into_os_string(),
            self.alias().into(),
        ]
    }

    /// Options precede the host alias. Masters must stay in the foreground (no `-f`).
    pub fn spawn(&self, options: &[&str], command: Option<&str>) -> Result<Process> {
        let mut ssh = Command::new("ssh");
        ssh.arg("-F")
            .arg(self.config_path())
            .args(options)
            .arg(self.alias());
        if let Some(command) = command {
            ssh.arg(command);
        }
        Process::spawn(&mut ssh)
    }

    pub fn run(&self, command: &str) -> Result<Output> {
        self.spawn(&[], Some(command))?
            .wait(Duration::from_secs(10))
    }
}

impl Drop for Sshd {
    fn drop(&mut self) {
        // Ask masters to exit before destroying the transport; socket paths are private to this test.
        let mut directories = vec![self.directory.path().to_path_buf()];
        while let Some(directory) = directories.pop() {
            if let Ok(entries) = fs::read_dir(directory) {
                for entry in entries.flatten() {
                    if let Ok(kind) = entry.file_type() {
                        if kind.is_dir() {
                            directories.push(entry.path());
                        }
                        if kind.is_socket() {
                            let mut ssh = Command::new("ssh");
                            ssh.arg("-F")
                                .arg(self.config_path())
                                .arg("-S")
                                .arg(entry.path())
                                .args(["-O", "exit", self.alias()]);
                            if let Ok(mut process) = Process::spawn(&mut ssh) {
                                let _ = process.wait(Duration::from_millis(500));
                            }
                        }
                    }
                }
            }
        }
    }
}

fn parse_auth_attempts(log: &str) -> BTreeMap<(String, String), usize> {
    let mut counts = BTreeMap::new();
    for line in log.lines() {
        let mut words = line.split_whitespace();
        // Syslog-style prefixes vary across OpenSSH versions; auth_log's body does not.
        while let Some(outcome) = words.next() {
            if !matches!(outcome, "Accepted" | "Failed" | "Partial" | "Postponed") {
                continue;
            }
            let Some(method) = words.next() else { break };
            if words.next() == Some("for") && method != "none" {
                *counts
                    .entry((method.to_owned(), outcome.to_owned()))
                    .or_default() += 1;
            }
            break;
        }
    }
    counts
}

fn find_sshd() -> Result<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/usr/sbin/sshd"),
        PathBuf::from("/usr/local/sbin/sshd"),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&path).map(|directory| directory.join("sshd")));
    }
    for path in paths {
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("real-sshd tests require OpenSSH sshd. On Debian/Ubuntu install it with `sudo apt-get update && sudo apt-get install -y openssh-server`; on macOS use /usr/sbin/sshd (Remote Login need not be enabled).")
}

fn account() -> Result<(String, PathBuf)> {
    let mut record = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut buffer = vec![0; 16384];
    let mut result = std::ptr::null_mut();
    let error = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut record,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if error != 0 || result.is_null() {
        bail!("getpwuid_r failed for current user: {error}");
    }
    let name = unsafe { CStr::from_ptr(record.pw_name) }
        .to_str()?
        .to_owned();
    let home = unsafe { CStr::from_ptr(record.pw_dir) }.to_str()?.into();
    Ok((name, home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_server_signals_only_the_process_its_descriptor_recorded() {
        let data = tempfile::tempdir().unwrap();
        let path = descriptor_path(data.path()).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id() as libc::pid_t;
        let stamp = start_stamp(pid).unwrap();
        // A stale descriptor whose pid now names another process.
        fs::write(
            &path,
            format!(r#"{{"pid":{pid},"starttime":{}}}"#, stamp + 1),
        )
        .unwrap();
        stop_server(data.path());
        assert!(child.try_wait().unwrap().is_none());
        fs::write(&path, format!(r#"{{"pid":{pid},"starttime":{stamp}}}"#)).unwrap();
        stop_server(data.path());
        fs::remove_file(&path).unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert!(child.wait().unwrap().signal().is_some());
    }

    #[test]
    fn auth_log_outcomes_ignore_prefixes_and_key_lookup_messages() {
        let log = "sshd[10]: Failed none for user from 127.0.0.1 port 42 ssh2\n\
                   Accepted key ED25519 SHA256:example found at /tmp/authorized_keys:1\n\
                   Postponed publickey for user from 127.0.0.1 port 42 ssh2 [preauth]\n\
                   Partial publickey for user from 127.0.0.1 port 42 ssh2: ED25519 SHA256:example\n\
                   sshd-auth[11]: Accepted publickey for user from 127.0.0.1 port 42 ssh2: ED25519 SHA256:example\n\
                   Oct 29 00:00:00 host sshd[12]: Failed password for invalid user nobody from ::1 port 43 ssh2\n\
                   Failed keyboard-interactive/pam for user from ::1 port 44 ssh2\n";
        assert_eq!(
            parse_auth_attempts(log),
            [
                (("publickey".into(), "Postponed".into()), 1),
                (("publickey".into(), "Partial".into()), 1),
                (("publickey".into(), "Accepted".into()), 1),
                (("password".into(), "Failed".into()), 1),
                (("keyboard-interactive/pam".into(), "Failed".into()), 1),
            ]
            .into()
        );
    }

    #[test]
    fn builder_knobs_work_in_unit_tests() {
        let server = SshdBuilder {
            extra_client_keys: vec![None],
            identity_files: vec!["client_key_1".into()],
            max_sessions: 2,
            max_startups: "2:50:4".into(),
            login_grace_time: Duration::from_secs(4),
            client_alive_interval: Duration::from_secs(1),
            client_alive_count_max: 2,
            ..SshdBuilder::default()
        }
        .start()
        .unwrap();
        assert!(server.run("true").unwrap().status.success());
    }
}
