use orx_host_proto::ExecResult;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub fn run(
    command: &mut Command,
    input: &[u8],
    timeout: Duration,
    stop: Option<&AtomicBool>,
) -> io::Result<ExecResult> {
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let result = (|| {
        let mut stdin = child.stdin.take();
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("missing stdout"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("missing stderr"))?;
        for fd in [
            stdin.as_ref().map(AsRawFd::as_raw_fd),
            Some(stdout.as_raw_fd()),
            Some(stderr.as_raw_fd()),
        ]
        .into_iter()
        .flatten()
        {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags == -1
                || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
            {
                return Err(io::Error::last_os_error());
            }
        }
        let mut result = ExecResult {
            code: None,
            signal: None,
            timed_out: false,
            stdout: vec![],
            stderr: vec![],
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let mut sent = 0;
        let mut out_closed = false;
        let mut err_closed = false;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pipe) = &mut stdin {
                if sent == input.len() {
                    stdin = None;
                } else {
                    match pipe.write(&input[sent..]) {
                        Ok(len) => sent += len,
                        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                            stdin = None;
                        }
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            out_closed |= drain(
                &mut stdout,
                &mut result.stdout,
                256 * 1024,
                &mut result.stdout_truncated,
            )?;
            err_closed |= drain(
                &mut stderr,
                &mut result.stderr,
                16 * 1024,
                &mut result.stderr_truncated,
            )?;
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            if unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            if unsafe { info.si_pid() } != 0 && out_closed && err_closed {
                break;
            }
            if stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) || Instant::now() >= deadline {
                result.timed_out = true;
                break;
            }
            let mut fds = Vec::new();
            if let Some(pipe) = &stdin {
                fds.push(libc::pollfd {
                    fd: pipe.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                });
            }
            if !out_closed {
                fds.push(libc::pollfd {
                    fd: stdout.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            if !err_closed {
                fds.push(libc::pollfd {
                    fd: stderr.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            let wait_ms = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(100) as i32;
            if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, wait_ms) } == -1 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
        Ok(result)
    })();
    // WNOWAIT keeps the session leader reserved until the final group signal.
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    let reap_deadline = Instant::now() + Duration::from_millis(100);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= reap_deadline {
            // A driver can leave SIGKILL pending in uninterruptible sleep.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return result;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    result.map(|mut result| {
        result.code = status.code();
        result.signal = status.signal();
        result
    })
}

fn drain(
    reader: &mut impl Read,
    output: &mut Vec<u8>,
    limit: usize,
    truncated: &mut bool,
) -> io::Result<bool> {
    let mut bytes = [0; 8192];
    for _ in 0..32 {
        match reader.read(&mut bytes) {
            Ok(0) => return Ok(true),
            Ok(len) => {
                let kept = len.min(limit - output.len());
                output.extend_from_slice(&bytes[..kept]);
                *truncated |= kept < len;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_and_input_are_bounded_without_pipe_deadlock() {
        let result = run(
            Command::new("/bin/sh")
                .arg("-c")
                .arg("cat; head -c 400000 /dev/zero; head -c 40000 /dev/zero >&2"),
            b"hello\n",
            Duration::from_secs(5),
            None,
        )
        .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stdout.len(), 256 * 1024);
        assert_eq!(result.stderr.len(), 16 * 1024);
        assert!(result.stdout.starts_with(b"hello\n"));
        assert!(result.stdout_truncated && result.stderr_truncated);
    }

    #[test]
    fn timeout_kills_session_and_handles_inherited_pipes() {
        let start = Instant::now();
        let result = run(
            Command::new("/bin/sh").arg("-c").arg("sleep 60 & wait"),
            &[],
            Duration::from_millis(50),
            None,
        )
        .unwrap();
        assert!(result.timed_out);
        assert_eq!(result.signal, Some(libc::SIGKILL));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn detached_child_survives_command_group_cleanup() {
        let dir = std::env::temp_dir().join(format!("orx-exec-detach-{}", crate::nonce().unwrap()));
        std::fs::create_dir_all(&dir).unwrap();
        let launch = "python3 -c 'import os,subprocess; os.setsid(); subprocess.Popen([\"/bin/sh\", \"-c\", \"sleep 0.2; echo survived > marker\"], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)'; exit $?";
        let result = run(
            Command::new("/bin/sh")
                .args(["-c", launch])
                .current_dir(&dir),
            &[],
            Duration::from_secs(2),
            None,
        )
        .unwrap();
        assert_eq!(result.code, Some(0), "{result:?}");
        assert!(!result.timed_out);

        let deadline = Instant::now() + Duration::from_secs(2);
        while std::fs::read_to_string(dir.join("marker")).ok().as_deref() != Some("survived\n") {
            assert!(Instant::now() < deadline, "detached child was killed");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
