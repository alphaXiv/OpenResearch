//! Identity of a local submitter subprocess (§2 fence): pid plus the OS start stamp, so a
//! reused pid never passes for the process that was recorded.

use std::io;
use std::time::Duration;

use crate::store::SubmitterIdentity;

pub fn identity(pid: u32) -> Option<SubmitterIdentity> {
    start_stamp(pid)
        .ok()
        .flatten()
        .map(|starttime| SubmitterIdentity { pid, starttime })
}

/// Kills the submitter if it is still the recorded process; true once it is provably gone.
pub async fn ensure_gone(submitter: &SubmitterIdentity) -> bool {
    for _ in 0..50 {
        match start_stamp(submitter.pid) {
            Ok(Some(starttime)) if starttime == submitter.starttime => {}
            Ok(_) => return true,
            Err(_) => return false,
        }
        kill(submitter);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(target_os = "linux")]
fn start_stamp(pid: u32) -> io::Result<Option<u64>> {
    if pid == 0 || libc::pid_t::try_from(pid).is_err() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let (_, fields) = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    let fields: Vec<&str> = fields.split_whitespace().collect();
    // Field 3 is the state and field 22 the start time; a zombie has already exited.
    if fields.first() == Some(&"Z") {
        return Ok(None);
    }
    let stamp = fields
        .get(19)
        .and_then(|stamp| stamp.parse().ok())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    Ok(Some(stamp))
}

#[cfg(target_os = "macos")]
fn start_stamp(pid: u32) -> io::Result<Option<u64>> {
    let pid = libc::c_int::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
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
    if written != size {
        let error = io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(error)
        };
    }
    if info.pbi_status == libc::SZOMB {
        return Ok(None);
    }
    Ok(Some(
        info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
    ))
}

#[cfg(unix)]
fn kill(submitter: &SubmitterIdentity) {
    let Ok(pid) = libc::pid_t::try_from(submitter.pid) else {
        return;
    };
    if identity(submitter.pid).as_ref() == Some(submitter) {
        // The stat→kill race is accepted on Unix, where no portable handle pins the PID.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

#[cfg(windows)]
fn with_process<T>(
    pid: u32,
    access: u32,
    f: impl FnOnce(*mut std::ffi::c_void) -> T,
) -> io::Result<Option<T>> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::OpenProcess;

    // SAFETY: plain syscalls; the handle is closed before returning.
    unsafe {
        let process = OpenProcess(access, 0, pid);
        if process.is_null() {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(87) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let result = f(process);
        CloseHandle(process);
        Ok(Some(result))
    }
}

#[cfg(windows)]
fn start_stamp(pid: u32) -> io::Result<Option<u64>> {
    use windows_sys::Win32::Foundation::{FILETIME, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    with_process(pid, PROCESS_QUERY_LIMITED_INFORMATION, |process| {
        // SAFETY: `process` is a live handle and every out-pointer is a local.
        unsafe {
            let mut code = 0u32;
            if GetExitCodeProcess(process, &mut code) == 0 {
                return Err(io::Error::last_os_error());
            }
            if code != STILL_ACTIVE as u32 {
                return Ok(None);
            }
            let mut times: [FILETIME; 4] = std::mem::zeroed();
            let [created, exited, kernel, user] = &mut times;
            if GetProcessTimes(process, created, exited, kernel, user) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Some(
                u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime),
            ))
        }
    })?
    .unwrap_or(Ok(None))
}

#[cfg(windows)]
fn kill(submitter: &SubmitterIdentity) {
    use windows_sys::Win32::System::Threading::{
        TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    // The handle pins the process, so the identity check and the kill name the same one.
    let _ = with_process(
        submitter.pid,
        PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
        |process| {
            if identity(submitter.pid).as_ref() == Some(submitter) {
                // SAFETY: `process` is a live handle opened with PROCESS_TERMINATE.
                unsafe { TerminateProcess(process, 1) };
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn an_unreadable_identity_does_not_release_the_fence() {
        let unreadable = SubmitterIdentity {
            pid: u32::MAX,
            starttime: 1,
        };
        assert!(start_stamp(unreadable.pid).is_err());
        assert!(!ensure_gone(&unreadable).await);
    }

    #[tokio::test]
    async fn a_live_submitter_is_killed_and_a_reused_pid_is_not_mistaken_for_it() {
        let mut child = if cfg!(windows) {
            crate::process::command("ping")
                .args(["-n", "60", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        } else {
            crate::process::command("sleep").arg("60").spawn().unwrap()
        };
        let recorded = identity(child.id()).unwrap();
        let impostor = SubmitterIdentity {
            starttime: recorded.starttime + 1,
            ..recorded.clone()
        };
        assert!(ensure_gone(&impostor).await);
        assert_eq!(identity(child.id()), Some(recorded.clone()));
        assert!(ensure_gone(&recorded).await);
        assert!(child.wait().is_ok());
        assert!(identity(child.id()).is_none());
    }
}
