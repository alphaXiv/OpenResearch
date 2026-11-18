use crate::keeper::{Process, Spawned};
use crate::wire::{Exit, Signal, Spec, Stdio};
use std::fs::OpenOptions;
use std::io;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use windows_sys::Win32::Foundation::{
    CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT,
};
use windows_sys::Win32::System::Console::{
    GenerateConsoleCtrlEvent, GetStdHandle, CTRL_BREAK_EVENT, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    JOBOBJECT_BASIC_PROCESS_ID_LIST, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, TerminateProcess,
    CREATE_NEW_PROCESS_GROUP, PROCESS_TERMINATE,
};

/// The keeper's own Job Object. Its handle closes only when the keeper exits, which ends
/// every process the keeper started, so a child never outlives the keeper holding it.
static JOB: OnceLock<usize> = OnceLock::new();

/// Keeps this process's standard handles out of the processes it starts. `CreateProcess`
/// hands a child every inheritable handle, so a pipe someone reads to its end would stay
/// open for as long as a kept child runs. Children given these handles get duplicates.
pub fn private_stdio() {
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: plain Win32 calls on this process's own standard handles.
        unsafe {
            let handle = GetStdHandle(id);
            if !handle.is_null() {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

pub fn detach() -> io::Result<()> {
    private_stdio();
    // SAFETY: plain Win32 calls; the limit struct is zeroed plain data with its flag set.
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
            || AssignProcessToJobObject(job, GetCurrentProcess()) == 0
        {
            let error = io::Error::last_os_error();
            CloseHandle(job);
            return Err(error);
        }
        let _ = JOB.set(job as usize);
    }
    Ok(())
}

pub fn silence_stdout() {}

pub fn spawn(spec: &Spec) -> io::Result<Spawned> {
    match &spec.stdio {
        Stdio::Pipes { .. } | Stdio::Merged => {
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
            let merged = match &spec.stdio {
                Stdio::Merged => {
                    let (output, writer) = std::io::pipe()?;
                    command.stdout(writer.try_clone()?).stderr(writer);
                    Some(output)
                }
                Stdio::Pipes { stderr: Some(path) } => {
                    let log = OpenOptions::new().create(true).append(true).open(path)?;
                    command.stdout(std::process::Stdio::piped()).stderr(log);
                    None
                }
                _ => {
                    command
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null());
                    None
                }
            };
            // Its own group, so an interrupt reaches the child and not the keeper.
            let mut child = command
                .stdin(std::process::Stdio::piped())
                .creation_flags(CREATE_NEW_PROCESS_GROUP)
                .spawn()?;
            // The pipe's write ends must close here, or reading never ends.
            drop(command);
            let pid = child.id();
            let input = child.stdin.take().map(|stdin| Box::new(stdin) as _);
            let output: Box<dyn std::io::Read + Send> = match merged {
                Some(output) => Box::new(output),
                None => Box::new(
                    child
                        .stdout
                        .take()
                        .ok_or_else(|| io::Error::other("the child has no stdout"))?,
                ),
            };
            Ok(Spawned {
                pid,
                input,
                output,
                process: Box::new(Tree { pid, pty: None }),
                wait: Box::new(move || {
                    let status = child.wait().ok();
                    Exit {
                        code: status.and_then(|status| status.code()),
                        signal: None,
                        finished_at: crate::keeper::now_ms(),
                        reason: None,
                        unfinished_output: false,
                    }
                }),
                answers_cursor_queries: false,
                types_interrupt: false,
            })
        }
        Stdio::Pty { cols, rows } => {
            use portable_pty::{native_pty_system, CommandBuilder, PtySize};
            let pair = native_pty_system()
                .openpty(PtySize {
                    rows: *rows,
                    cols: *cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(io::Error::other)?;
            let mut command = CommandBuilder::new(&spec.program);
            command.args(&spec.args);
            if spec.env_clear {
                command.env_clear();
            }
            for (key, value) in &spec.env {
                match value {
                    Some(value) => command.env(key, value),
                    None => command.env_remove(key),
                }
            }
            if let Some(cwd) = &spec.cwd {
                command.cwd(cwd);
            }
            let mut child = pair
                .slave
                .spawn_command(command)
                .map_err(io::Error::other)?;
            drop(pair.slave);
            let pid = child.process_id().unwrap_or_default();
            let output = pair.master.try_clone_reader().map_err(io::Error::other)?;
            let input = pair.master.take_writer().map_err(io::Error::other)?;
            Ok(Spawned {
                pid,
                input: Some(input),
                output,
                process: Box::new(Tree {
                    pid,
                    pty: Some(Mutex::new(pair.master)),
                }),
                wait: Box::new(move || {
                    let status = child.wait().ok();
                    Exit {
                        code: status.map(|status| status.exit_code() as i32),
                        signal: None,
                        finished_at: crate::keeper::now_ms(),
                        reason: None,
                        unfinished_output: false,
                    }
                }),
                // ConPTY holds the child's output until someone answers where the cursor is.
                answers_cursor_queries: true,
                types_interrupt: true,
            })
        }
    }
}

struct Tree {
    pid: u32,
    pty: Option<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
}

impl Process for Tree {
    fn resize(&self, cols: u16, rows: u16) {
        if let Some(master) = &self.pty {
            if let Ok(master) = master.lock() {
                let _ = master.resize(portable_pty::PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
            }
        }
    }

    fn signal(&self, signal: Signal) {
        match signal {
            // A terminal's interrupt is typed as input instead, so it never reaches here.
            // SAFETY: the child leads its own process group on the keeper's console.
            Signal::Interrupt => unsafe {
                GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, self.pid);
            },
            Signal::Terminate | Signal::Kill => self.kill_tree(),
            Signal::Unknown => {}
        }
    }

    /// Every member of the keeper's job but the keeper: the child and all it started.
    fn kill_tree(&self) {
        let Some(job) = JOB.get().map(|job| *job as HANDLE) else {
            return;
        };
        const CAPACITY: usize = 1024;
        let mut buffer = vec![0usize; CAPACITY + 2];
        // SAFETY: the buffer is large enough for the header and CAPACITY ids, and stays
        // alive for the call; the job handle stays open for the keeper's life.
        unsafe {
            let list = buffer.as_mut_ptr() as *mut JOBOBJECT_BASIC_PROCESS_ID_LIST;
            if QueryInformationJobObject(
                job,
                JobObjectBasicProcessIdList,
                list as *mut _,
                (buffer.len() * std::mem::size_of::<usize>()) as u32,
                std::ptr::null_mut(),
            ) == 0
            {
                return;
            }
            let count = (*list).NumberOfProcessIdsInList as usize;
            let ids = std::slice::from_raw_parts((*list).ProcessIdList.as_ptr(), count);
            let own = GetCurrentProcessId() as usize;
            for &id in ids.iter().filter(|&&id| id != own) {
                let process = OpenProcess(PROCESS_TERMINATE, 0, id as u32);
                if !process.is_null() {
                    TerminateProcess(process, 1);
                    CloseHandle(process);
                }
            }
        }
    }
}
