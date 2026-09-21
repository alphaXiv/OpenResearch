use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
    OpenJobObjectW, QueryInformationJobObject, TerminateJobObject,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
};
use windows_sys::Win32::System::SystemServices::{JOB_OBJECT_QUERY, JOB_OBJECT_TERMINATE};
use windows_sys::Win32::System::Threading::{
    OpenThread, ResumeThread, CREATE_SUSPENDED, THREAD_SUSPEND_RESUME,
};

use crate::error::{anyhow, Result};

fn own(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: callers pass newly opened handles, transferred exactly once.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

pub(super) fn spawn(command: &mut Command, dir: &Path) -> Result<Child> {
    let name = format!("Local\\orx-job-{}", uuid::Uuid::new_v4());
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        bInheritHandle: 1,
        ..Default::default()
    };
    // SAFETY: both inputs remain live; the launcher inherits the handle to retain the name.
    let job = own(unsafe { CreateJobObjectW(&attributes, wide.as_ptr()) })?;
    let mut child = command.creation_flags(CREATE_SUSPENDED).spawn()?;
    let setup = (|| -> Result<()> {
        // SAFETY: both handles are live; the suspended child cannot spawn outside the job.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        std::fs::write(dir.join("windows_job"), &name)?;
        std::fs::write(dir.join("pid"), format!("{}\n", child.id()))?;
        resume(child.id())
    })();
    if let Err(error) = setup {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    // Bash keeps the inherited handle open so a separate supervisor can reopen the job.
    Ok(child)
}

fn resume(pid: u32) -> Result<()> {
    // SAFETY: the snapshot owns no process resources and is closed by OwnedHandle.
    let snapshot = own(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) })?;
    let mut entry = THREADENTRY32 {
        dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: entry has the required size and remains writable throughout enumeration.
    let mut found = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    let mut resumed = false;
    while found != 0 {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: this is the primary thread of our still-suspended child.
            let thread = own(unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) })?;
            // SAFETY: the thread handle has suspend/resume access.
            let previous = unsafe { ResumeThread(thread.as_raw_handle()) };
            if previous == u32::MAX {
                return Err(io::Error::last_os_error().into());
            }
            resumed |= previous == 1;
        }
        // SAFETY: same initialized entry and live snapshot as above.
        found = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    if resumed {
        Ok(())
    } else {
        Err(anyhow!("Could not find the local run's suspended thread"))
    }
}

pub(super) fn open_job(dir: &Path) -> io::Result<OwnedHandle> {
    let name = std::fs::read_to_string(dir.join("windows_job"))?;
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the stored job name is terminated; the handle is not inherited.
    own(unsafe { OpenJobObjectW(JOB_OBJECT_TERMINATE | JOB_OBJECT_QUERY, 0, wide.as_ptr()) })
}

pub(super) fn cancel(dir: &Path, pid: &str) -> Result<()> {
    if super::exit_code_state(dir).is_some() {
        return Ok(());
    }
    let job = match open_job(dir) {
        Ok(job) => job,
        Err(error) if error.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => {
            return super::terminate_tree(pid);
        }
        Err(error) => return Err(error.into()),
    };
    // SAFETY: the handle belongs to this run and has terminate access.
    if unsafe { TerminateJobObject(job.as_raw_handle(), 1) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    for _ in 0..50 {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: info is writable and its size matches the requested information class.
        if unsafe {
            QueryInformationJobObject(
                job.as_raw_handle(),
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut info).cast(),
                std::mem::size_of_val(&info) as u32,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        if info.ActiveProcesses == 0 {
            if super::exit_code_state(dir).is_none() {
                std::fs::write(dir.join("exit_code"), "1\n")?;
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(anyhow!("Could not terminate all local run processes"))
}
