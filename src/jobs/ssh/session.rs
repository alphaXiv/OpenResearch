//! A batch shell keeps Win32-OpenSSH operations on one authenticated transport.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use super::{sh_quote, ssh_opts, SshTarget};
use crate::error::{anyhow, Result};

// Length framing preserves binary archives, partial log lines, and arbitrary command output.
const WORKER: &str = r#"set -eo pipefail
umask 077
work=$(mktemp -d) || exit 1
trap 'rm -rf -- "$work"' EXIT
printf 'ORX_SSH_READY\n'
while read -r command_size input_size; do
    head -c "$command_size" > "$work/command"
    test "$(wc -c < "$work/command")" -eq "$command_size" || exit 1
    if head -c "$input_size" | {
        code=0
        bash -c "$(cat "$work/command")" > "$work/out" 2> "$work/err" || code=$?
        cat > /dev/null
        exit "$code"
    }; then
        code=0
    else
        code=$?
    fi
    printf '%s %s %s\n' "$code" "$(wc -c < "$work/out")" "$(wc -c < "$work/err")"
    cat "$work/out" "$work/err"
done
"#;

type Key = (String, Vec<String>);
type Slot = Arc<Mutex<Option<Session>>>;
static SESSIONS: OnceLock<Mutex<HashMap<Key, Slot>>> = OnceLock::new();

pub(super) async fn run(
    target: &SshTarget,
    command: &str,
    input: &mut (dyn AsyncRead + Unpin + Send),
    size: u64,
) -> Result<String> {
    let slot = SESSIONS
        .get_or_init(Mutex::default)
        .lock()
        .await
        .entry((target.dest.clone(), target.extra_opts.clone()))
        .or_default()
        .clone();
    let mut slot = slot.lock().await;
    // -G resolves Includes and alias changes locally; it opens no SSH connection.
    let config = connection_config(target).await?;
    // A cancelled request must drop its transport, not leave a half-read reply in the pool.
    let mut cached = slot.take();
    if let Some(session) = &mut cached {
        if session.config != config || session.child.try_wait()?.is_some() {
            cached = None;
        }
    }
    let mut session = match cached {
        Some(session) => session,
        None => connect(target, config).await?,
    };
    match session.request(command, input, size).await {
        Ok(reply) => {
            *slot = Some(session);
            if reply.code != 0 {
                return Err(anyhow!(
                    "ssh {} failed (exit {}): {}",
                    target.dest,
                    reply.code,
                    String::from_utf8_lossy(&reply.stderr).trim()
                ));
            }
            Ok(String::from_utf8_lossy(&reply.stdout).into_owned())
        }
        // Never replay a submitted command: it could already have launched a job.
        Err(error) => Err(session.failure(&target.dest, error).await),
    }
}

async fn connection_config(target: &SshTarget) -> Result<Vec<u8>> {
    let mut command = Command::new("ssh");
    command
        .env("ORX_SSH_PROBE", "1")
        .args(ssh_opts(target, true))
        .args(["-G", "--", &target.dest])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(crate::local::codex::windows_job::SPAWN_FLAGS);
    let child = command.spawn()?;
    // Match exec inherits Windows pipes; closing the job must also clean up on cancellation.
    #[cfg(windows)]
    let _job = {
        use crate::local::codex::windows_job;
        let job = windows_job::assign(&child)?;
        windows_job::resume(child.id().unwrap())?;
        job
    };
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .map_err(|_| {
            anyhow!("Timed out resolving SSH configuration; check your Match exec commands")
        })??;
    if !output.status.success() {
        return Err(anyhow!(
            "Could not resolve SSH configuration: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

async fn connect(target: &SshTarget, config: Vec<u8>) -> Result<Session> {
    for attempt in 0..4 {
        let result = Session::start(target, config.clone()).await;
        match result {
            Ok(session) => return Ok(session),
            Err(error) if attempt < 3 && connect_timeout(&error.to_string()) => {
                let delay = Duration::from_secs(30 << attempt);
                eprintln!(
                    "SSH connection to {} timed out; retrying in {}s before sending any command.",
                    target.dest,
                    delay.as_secs()
                );
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

fn connect_timeout(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("timed out") || error.contains("connection timeout")
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    config: Vec<u8>,
    diagnostics: Diagnostics,
}

struct Diagnostics {
    done: tokio::sync::oneshot::Receiver<()>,
    tail: Arc<StdMutex<Vec<u8>>>,
}

impl Diagnostics {
    fn from_child(stderr: ChildStderr) -> Result<Self> {
        // Cached Windows pipes must not keep Tokio's blocking workers alive at CLI exit.
        #[cfg(windows)]
        let stderr = std::fs::File::from(stderr.into_owned_handle()?);
        #[cfg(unix)]
        let stderr = std::fs::File::from(stderr.into_owned_fd()?);
        Ok(Self::new(stderr))
    }

    fn new(mut stderr: impl std::io::Read + Send + 'static) -> Self {
        let tail = Arc::new(StdMutex::new(Vec::new()));
        let output = tail.clone();
        let (finished, done) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(count) = stderr.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let mut output = output.lock().unwrap();
                output.extend_from_slice(&buffer[..count]);
                let excess = output.len().saturating_sub(8192);
                output.drain(..excess);
            }
            let _ = finished.send(());
        });
        Self { done, tail }
    }
}

struct Reply {
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl Session {
    async fn start(target: &SshTarget, config: Vec<u8>) -> Result<Self> {
        let mut child = Command::new("ssh")
            .env("ORX_SSH_PROBE", "1")
            .args(ssh_opts(target, true))
            .args(["-T", "--", &target.dest])
            .arg(format!("bash -c {}", sh_quote(WORKER)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| anyhow!("Could not run ssh: {error}"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let diagnostics = Diagnostics::from_child(child.stderr.take().unwrap())?;
        let mut session = Self {
            child,
            stdin,
            stdout,
            config,
            diagnostics,
        };
        let ready = tokio::time::timeout(Duration::from_secs(45), session.ready()).await;
        match ready {
            Ok(Ok(())) => Ok(session),
            other => {
                let error = match other {
                    Ok(Err(error)) => error,
                    Err(_) => anyhow!("SSH connection timed out waiting for the batch shell"),
                    Ok(Ok(())) => unreachable!(),
                };
                Err(session.failure(&target.dest, error).await)
            }
        }
    }

    async fn ready(&mut self) -> Result<()> {
        let mut line = String::new();
        self.stdout.read_line(&mut line).await?;
        if line != "ORX_SSH_READY\n" {
            return Err(anyhow!("SSH batch shell did not start"));
        }
        Ok(())
    }

    async fn request(
        &mut self,
        command: &str,
        input: &mut (dyn AsyncRead + Unpin + Send),
        size: u64,
    ) -> Result<Reply> {
        self.stdin
            .write_all(format!("{} {size}\n", command.len()).as_bytes())
            .await?;
        self.stdin.write_all(command.as_bytes()).await?;
        if tokio::io::copy(&mut input.take(size), &mut self.stdin).await? != size {
            return Err(anyhow!("SSH input ended before its declared length"));
        }
        self.stdin.flush().await?;
        let mut header = String::new();
        self.stdout.read_line(&mut header).await?;
        let fields: Vec<_> = header.split_whitespace().collect();
        if fields.len() != 3 {
            return Err(anyhow!("SSH batch shell returned an invalid response"));
        }
        let code = fields[0].parse()?;
        let stdout = self.read_body(fields[1].parse()?).await?;
        let stderr = self.read_body(fields[2].parse()?).await?;
        Ok(Reply {
            code,
            stdout,
            stderr,
        })
    }

    async fn read_body(&mut self, size: u64) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let read = (&mut self.stdout)
            .take(size)
            .read_to_end(&mut bytes)
            .await?;
        if read as u64 != size {
            return Err(anyhow!("SSH batch shell response was interrupted"));
        }
        Ok(bytes)
    }

    async fn failure(mut self, dest: &str, error: anyhow::Error) -> anyhow::Error {
        let _ = self.child.kill().await;
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut self.diagnostics.done).await;
        let stderr = String::from_utf8_lossy(&self.diagnostics.tail.lock().unwrap())
            .trim()
            .to_string();
        anyhow!(
            "ssh {dest}: {error}{separator}{stderr}",
            separator = if stderr.is_empty() { "" } else { ": " }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_connection_timeouts_are_retryable_before_submission() {
        assert!(connect_timeout(
            "Connection timed out during banner exchange"
        ));
        assert!(connect_timeout(
            "ssh: connect to host box port 22: Connection timed out"
        ));
        for message in [
            "Permission denied (publickey)",
            "Host key verification failed",
            "Connection refused",
            "SSH batch shell did not start",
        ] {
            assert!(!connect_timeout(message));
        }
    }

    #[tokio::test]
    #[ignore = "requires a real SSH host; set ORX_SSH_TEST_HOST"]
    async fn real_ssh_reuses_one_shell_across_commands_and_remote_failures() {
        let target = SshTarget::alias(&std::env::var("ORX_SSH_TEST_HOST").unwrap());
        let call = |command: &'static str| {
            let target = target.clone();
            async move { run(&target, command, &mut &b""[..], 0).await }
        };
        let first = call("echo $SSH_CONNECTION").await.unwrap();
        assert!(call("echo expected_error >&2; exit 42")
            .await
            .unwrap_err()
            .to_string()
            .contains("expected_error"));
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..12 {
            requests.spawn(call("echo $SSH_CONNECTION"));
        }
        while let Some(result) = requests.join_next().await {
            assert_eq!(result.unwrap().unwrap(), first);
        }
        let data = b"\0\xff\nORX_SSH_READY\n0 0 0\n";
        assert_eq!(
            run(&target, "wc -c", &mut &data[..], data.len() as u64)
                .await
                .unwrap()
                .trim(),
            data.len().to_string()
        );
        let key = (target.dest.clone(), target.extra_opts.clone());
        let pool = SESSIONS.get().unwrap().lock().await;
        let mut cached = pool.get(&key).unwrap().lock().await;
        cached.as_mut().unwrap().child.kill().await.unwrap();
        drop(cached);
        drop(pool);
        let reconnected = call("echo $SSH_CONNECTION").await.unwrap();
        if cfg!(windows) {
            assert_ne!(reconnected, first);
        }
    }

    #[tokio::test]
    async fn batch_shell_preserves_binary_input_failures_and_response_boundaries() {
        let mut child = Command::new(crate::local::bash::program())
            .args(["-c", WORKER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut session = Session {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            diagnostics: Diagnostics::from_child(child.stderr.take().unwrap()).unwrap(),
            config: Vec::new(),
            child,
        };
        session.ready().await.unwrap();
        let data = b"\0\xff\nORX_SSH_READY\n0 0 0\n";
        let reply = session
            .request(
                "cat; printf 'failure without newline' >&2; exit 42",
                &mut &data[..],
                data.len() as u64,
            )
            .await
            .unwrap();
        assert_eq!(reply.code, 42);
        assert_eq!(reply.stdout, data);
        assert_eq!(reply.stderr, b"failure without newline");
        let reply = session
            .request("printf 'second\n\u{2713}'", &mut &b""[..], 0)
            .await
            .unwrap();
        assert_eq!(reply.code, 0);
        assert_eq!(reply.stdout, "second\n\u{2713}".as_bytes());
        let data = vec![b'x'; 512 * 1024];
        let reply = session
            .request("head -c 1; exit 42", &mut &data[..], data.len() as u64)
            .await
            .unwrap();
        assert_eq!(reply.stdout, b"x");
        assert_eq!(reply.code, 42);
        let reply = session
            .request("printf aligned", &mut &b""[..], 0)
            .await
            .unwrap();
        assert_eq!(reply.stdout, b"aligned");
        session.child.kill().await.unwrap();
    }

    #[tokio::test]
    async fn transport_stderr_is_drained_with_bounded_diagnostics() {
        let mut child = std::process::Command::new(crate::local::bash::program())
            .args([
                "-c",
                "head -c 262144 /dev/zero >&2; printf 'last diagnostic' >&2",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut diagnostics = Diagnostics::new(child.stderr.take().unwrap());
        tokio::time::timeout(Duration::from_secs(5), async {
            (&mut diagnostics.done).await.unwrap();
        })
        .await
        .unwrap();
        assert!(child.wait().unwrap().success());
        let tail = diagnostics.tail.lock().unwrap();
        assert_eq!(tail.len(), 8192);
        assert!(tail.ends_with(b"last diagnostic"));
    }

    #[tokio::test]
    async fn effective_configuration_detects_an_alias_retarget_without_connecting() {
        let dir = std::env::temp_dir().join(format!("orx-ssh-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let mut target = SshTarget::alias("test-alias");
        target.extra_opts = vec!["-F".into(), config.to_str().unwrap().into()];
        std::fs::write(&config, "Host test-alias\n Hostname 127.0.0.1\n").unwrap();
        let first = connection_config(&target).await.unwrap();
        std::fs::write(&config, "Host test-alias\n Hostname 127.0.0.2\n").unwrap();
        assert_ne!(connection_config(&target).await.unwrap(), first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn configuration_match_exec_is_bounded() {
        let dir = std::env::temp_dir().join(format!("orx-ssh-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let delay = if cfg!(windows) {
            "powershell.exe -NoProfile -Command Start-Sleep -Seconds 12"
        } else {
            "sleep 12"
        };
        std::fs::write(&config, format!("Match exec \"{delay}\"\n User ignored\n")).unwrap();
        let mut target = SshTarget::alias("test-alias");
        target.extra_opts = vec!["-F".into(), config.to_str().unwrap().into()];
        let error = tokio::time::timeout(Duration::from_secs(15), connection_config(&target))
            .await
            .unwrap()
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("Timed out resolving SSH configuration"));
        remove_config_dir(&dir).await;
    }

    async fn remove_config_dir(dir: &std::path::Path) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while std::fs::remove_dir_all(dir).is_err() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn cancelling_configuration_kills_match_exec_descendants() {
        let dir = std::env::temp_dir().join(format!("orx-ssh-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        let marker = dir.join("started");
        let path = marker
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "''");
        std::fs::write(
            &config,
            format!("Match exec \"powershell.exe -NoProfile -Command [IO.File]::WriteAllText('{path}', 'started'); Start-Sleep -Seconds 12\"\n User ignored\n"),
        )
        .unwrap();
        let mut target = SshTarget::alias("test-alias");
        target.extra_opts = vec!["-F".into(), config.to_str().unwrap().into()];
        let probe = tokio::spawn(async move { connection_config(&target).await });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        probe.abort();
        assert!(probe.await.unwrap_err().is_cancelled());
        remove_config_dir(&dir).await;
    }
}
