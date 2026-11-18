use crate::keeper::{Process, Spawned};
use crate::wire::{Exit, Signal, Spec, Stdio};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::Command;
use std::time::Duration;

/// Leaves the server's process tree and session, so neither its exit nor its terminal ends this.
pub fn detach() -> io::Result<()> {
    // SAFETY: runs before any thread exists, so the child is a whole copy of this process.
    match unsafe { libc::fork() } {
        -1 => return Err(io::Error::last_os_error()),
        0 => {}
        // SAFETY: the parent only reports success; the server reaps it at once.
        _ => unsafe { libc::_exit(0) },
    }
    // SAFETY: plain syscalls with no memory arguments.
    unsafe {
        if libc::setsid() == -1 {
            return Err(io::Error::last_os_error());
        }
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
    Ok(())
}

/// Stdout carried only the ready line; a server that stops reading it must not see writes later.
pub fn silence_stdout() {
    if let Ok(null) = File::options().write(true).open("/dev/null") {
        // SAFETY: both descriptors are open; dup2 replaces fd 1 atomically.
        unsafe {
            libc::dup2(null.as_raw_fd(), 1);
        }
    }
}

pub fn spawn(spec: &Spec) -> io::Result<Spawned> {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args);
    if spec.env_clear {
        command.env_clear();
    }
    for (key, value) in &spec.env {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    // SAFETY: getpid has no preconditions.
    let keeper = unsafe { libc::getpid() };
    match &spec.stdio {
        Stdio::Pipes { stderr } => {
            let stderr = match stderr {
                Some(path) => OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?
                    .into(),
                None => std::process::Stdio::null(),
            };
            command
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(stderr)
                .process_group(0);
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                command.pre_exec(move || child_setup(keeper, false));
            }
            let mut child = command.spawn()?;
            let pid = child.id();
            let input = child.stdin.take().map(|stdin| Box::new(stdin) as _);
            let output = child
                .stdout
                .take()
                .ok_or_else(|| io::Error::other("the child has no stdout"))?;
            Ok(Spawned {
                pid,
                input,
                output: Box::new(output),
                process: Box::new(Group {
                    pgid: pid,
                    pty: None,
                }),
                wait: Box::new(move || exit_of(child.wait())),
                answers_cursor_queries: false,
                types_interrupt: false,
            })
        }
        Stdio::Merged => {
            let (output, writer) = std::io::pipe()?;
            command
                .stdin(std::process::Stdio::piped())
                .stdout(writer.try_clone()?)
                .stderr(writer)
                .process_group(0);
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                command.pre_exec(move || child_setup(keeper, false));
            }
            let mut child = command.spawn()?;
            // The pipe's write ends must close here, or reading never ends.
            drop(command);
            let pid = child.id();
            let input = child.stdin.take().map(|stdin| Box::new(stdin) as _);
            Ok(Spawned {
                pid,
                input,
                output: Box::new(output),
                process: Box::new(Group {
                    pgid: pid,
                    pty: None,
                }),
                wait: Box::new(move || exit_of(child.wait())),
                answers_cursor_queries: false,
                types_interrupt: false,
            })
        }
        Stdio::Pty { cols, rows } => {
            let (master, slave) = openpty(*cols, *rows)?;
            command
                .stdin(slave.try_clone()?)
                .stdout(slave.try_clone()?)
                .stderr(slave);
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                command.pre_exec(move || child_setup(keeper, true));
            }
            let mut child = command.spawn()?;
            // The slave copies must close here, or reading the master never ends.
            drop(command);
            let pid = child.id();
            let output = PtyReader(File::from(master.try_clone()?));
            let input = File::from(master.try_clone()?);
            Ok(Spawned {
                pid,
                input: Some(Box::new(input)),
                output: Box::new(output),
                process: Box::new(Group {
                    pgid: pid,
                    pty: Some(master),
                }),
                wait: Box::new(move || exit_of(child.wait())),
                answers_cursor_queries: false,
                types_interrupt: false,
            })
        }
    }
}

/// The keeper ignores hangups and interrupts; its child must not inherit that.
fn child_setup(keeper: libc::pid_t, pty: bool) -> io::Result<()> {
    // SAFETY: async-signal-safe syscalls on this process only.
    unsafe {
        for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGPIPE, libc::SIGQUIT] {
            libc::signal(signal, libc::SIG_DFL);
        }
        if pty {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        #[cfg(target_os = "linux")]
        {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != keeper {
                libc::_exit(1);
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = keeper;
    }
    Ok(())
}

fn openpty(cols: u16, rows: u16) -> io::Result<(OwnedFd, OwnedFd)> {
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: the out-pointers are valid; a null name and termios are allowed.
    let opened = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    if opened == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned two fresh descriptors this function now owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [&master, &slave] {
        // SAFETY: fd is open; FD_CLOEXEC keeps it out of every other child.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((master, slave))
}

/// Linux answers a read of a master whose terminal closed with EIO, which is the end of output.
struct PtyReader(File);

impl io::Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(error) if error.raw_os_error() == Some(libc::EIO) => Ok(0),
            result => result,
        }
    }
}

struct Group {
    pgid: u32,
    pty: Option<OwnedFd>,
}

impl Group {
    fn send(&self, signal: libc::c_int) {
        if let Ok(pgid) = libc::pid_t::try_from(self.pgid) {
            // SAFETY: the group is the child's own; a negative pid names it.
            unsafe {
                libc::kill(-pgid, signal);
            }
        }
    }
}

impl Process for Group {
    fn resize(&self, cols: u16, rows: u16) {
        let Some(master) = &self.pty else {
            return;
        };
        let size = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: master is open and size outlives the call.
        unsafe {
            libc::ioctl(master.as_raw_fd(), libc::TIOCSWINSZ as _, &size);
        }
    }

    fn signal(&self, signal: Signal) {
        match signal {
            Signal::Interrupt => self.send(libc::SIGINT),
            Signal::Terminate => self.send(libc::SIGTERM),
            Signal::Kill => self.send(libc::SIGKILL),
            Signal::Unknown => {}
        }
    }

    fn kill_tree(&self) {
        if self.pty.is_some() {
            // A terminal's programs get the hangup a closed window would send them first.
            self.send(libc::SIGHUP);
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Ok(root) = libc::pid_t::try_from(self.pgid) {
            kill_descendants(root);
        }
        self.send(libc::SIGKILL);
    }
}

/// Kills `root` and every process descending from it, including ones that moved to a session
/// of their own. Each is stopped as it is found, so none forks away while the tree is read.
fn kill_descendants(root: libc::pid_t) {
    // SAFETY: the keeper's own child; stopping it holds its tree still.
    if unsafe { libc::kill(root, libc::SIGSTOP) } != 0 {
        return;
    }
    let mut tree = vec![root];
    for _ in 0..16 {
        let Ok(listing) = Command::new("ps").args(["-axo", "pid=,ppid="]).output() else {
            eprintln!("orx keep: could not list processes to stop {root}'s descendants");
            break;
        };
        let found: Vec<libc::pid_t> = String::from_utf8_lossy(&listing.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse().ok()?;
                let parent = fields.next()?.parse().ok()?;
                (tree.contains(&parent) && !tree.contains(&pid)).then_some(pid)
            })
            .collect();
        if found.is_empty() {
            break;
        }
        for pid in found {
            // SAFETY: a descendant observed while its parent was stopped.
            unsafe {
                libc::kill(pid, libc::SIGSTOP);
            }
            tree.push(pid);
        }
    }
    for pid in tree {
        // SAFETY: as above.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
}

fn exit_of(status: io::Result<std::process::ExitStatus>) -> Exit {
    let status = status.ok();
    Exit {
        code: status.and_then(|status| status.code()),
        signal: status.and_then(|status| status.signal()),
        finished_at: crate::keeper::now_ms(),
        reason: None,
        unfinished_output: false,
    }
}
