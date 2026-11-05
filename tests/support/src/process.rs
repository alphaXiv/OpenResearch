use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// A foreground child in its own process group, killed and reaped on unwind.
pub struct Process {
    child: Child,
    stdout: File,
    stderr: File,
    reaped: bool,
}

impl Process {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        let stdout = tempfile::tempfile()?;
        let stderr = tempfile::tempfile()?;
        let child = command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?)
            .spawn()
            .with_context(|| format!("spawn {command:?}"))?;

        Ok(Self {
            child,
            stdout,
            stderr,
            reaped: false,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn stderr(&self) -> Result<String> {
        let mut bytes = vec![0; self.stderr.metadata()?.len() as usize];
        self.stderr.read_exact_at(&mut bytes, 0)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn wait(&mut self, timeout: Duration) -> Result<Output> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid() as _,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if unsafe { info.si_pid() } != 0 {
                self.kill_group();
                let status = self.child.wait()?;
                self.reaped = true;
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                self.stdout.seek(SeekFrom::Start(0))?;
                self.stderr.seek(SeekFrom::Start(0))?;
                self.stdout.read_to_end(&mut stdout)?;
                self.stderr.read_to_end(&mut stderr)?;
                return Ok(Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            if Instant::now() >= deadline {
                bail!(
                    "process {} timed out after {timeout:?}: {}",
                    self.pid(),
                    self.stderr()?
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill_group(&self) {
        // The unreaped child reserves this process-group ID; it cannot name an unrelated group.
        unsafe {
            libc::kill(-(self.pid() as i32), libc::SIGKILL);
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if !self.reaped {
            self.kill_group();
            let _ = self.child.wait();
        }
    }
}
