//! Self-update on Windows. The PowerShell installer does the download and the
//! unpacking, as the shell installer does elsewhere; this module supplies the
//! two things Windows makes the binary do for itself.
//!
//! Windows refuses to overwrite a mapped executable but allows renaming it. So
//! the installer unpacks into a staging directory, and only then is the
//! installed `orx.exe` renamed aside and the new one renamed into its place —
//! two renames, with no window in which `orx.exe` is half-written. The retired
//! copy is deleted by a later start, once every process that mapped it has
//! exited.
//!
//! There is no `exec`, so a restarting `orx up` spawns the new binary and exits;
//! the child waits on the parent's process handle before binding the port.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{anyhow, Result};

/// Environment a relaunched `orx up` reads to wait for the process it replaces.
const RELAUNCH_WAIT_PID_ENV: &str = "ORX_RELAUNCH_WAIT_PID";

/// Spawn the copy on disk with `args` and `envs`, telling it which process to
/// wait for, then exit. Returns only when the spawn failed.
pub(super) fn relaunch(args: Vec<std::ffi::OsString>, envs: &[(&str, String)]) -> std::io::Error {
    // The load-time path, which the update swapped a new file under.
    let Ok(exe) = std::env::current_exe() else {
        return std::io::Error::other("could not resolve the running executable");
    };
    let spawned = Command::new(exe)
        .args(args)
        .envs(envs.iter().cloned())
        .env(RELAUNCH_WAIT_PID_ENV, std::process::id().to_string())
        .spawn();
    match spawned {
        Ok(child) => {
            // The shell gets its prompt back; say where the server went.
            eprintln!(
                "orx up: continuing as process {} in this console",
                child.id()
            );
            std::process::exit(0)
        }
        Err(err) => err,
    }
}

/// Block until the process named by [`RELAUNCH_WAIT_PID_ENV`] has exited, so its
/// port is free to bind. Bounded: a parent that hangs must not take the new
/// server down with it, and the bind reports the conflict if it is still there.
/// Returns whether there was a parent to wait for.
pub(super) fn await_replaced_parent() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    let Some(pid) = std::env::var(RELAUNCH_WAIT_PID_ENV)
        .ok()
        .and_then(|pid| pid.parse::<u32>().ok())
    else {
        return false;
    };
    // Consumed here so the servers and agents this process spawns don't inherit it.
    std::env::remove_var(RELAUNCH_WAIT_PID_ENV);
    // SAFETY: plain syscalls; the handle is closed on every path out.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return true;
        }
        WaitForSingleObject(process, 15_000);
        CloseHandle(process);
    }
    true
}

const STAGE_PREFIX: &str = ".orx-update-";

/// Remove staging directories an interrupted update left beside the binary.
/// Called under the updater's lock, so none of them is in use.
fn sweep_stale_stages(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGE_PREFIX)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Unpack the release with the PowerShell installer into a staging directory,
/// then swap it in for `installed`. The old binary is restored if the swap fails.
pub(crate) fn install(installed: &Path, script: &[u8], quiet: bool) -> Result<()> {
    // Beside the target, not in %TEMP%: a rename cannot cross volumes, and
    // preflight already proved this directory writable.
    let dir = installed
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", installed.display()))?;
    sweep_stale_stages(dir);
    let stage = dir.join(format!("{STAGE_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&stage)?;
    let outcome = stage_and_swap(installed, script, quiet, &stage);
    let _ = std::fs::remove_dir_all(&stage);
    outcome
}

fn stage_and_swap(installed: &Path, script: &[u8], quiet: bool, stage: &Path) -> Result<()> {
    let script_path = stage.join("installer.ps1");
    std::fs::write(&script_path, script)?;
    let mut cmd = Command::new(powershell());
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(&script_path)
    // A flat unpack into the staging dir, with no receipt naming that dir as
    // the install prefix (UNMANAGED_INSTALL implies it; DISABLE_UPDATE makes sure).
    .env("OPENRESEARCH_CLI_UNMANAGED_INSTALL", stage)
    .env("OPENRESEARCH_CLI_DISABLE_UPDATE", "1")
    // Both outrank UNMANAGED_INSTALL in the installer; a user's shell may export them.
    .env_remove("OPENRESEARCH_CLI_INSTALL_DIR")
    .env_remove("CARGO_DIST_FORCE_INSTALL_DIR");
    if quiet {
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let status = cmd
        .status()
        .map_err(|err| anyhow!("Could not run PowerShell: {}", err))?;
    if !status.success() {
        return Err(anyhow!(
            "The installer exited with {}. The previous orx is untouched.",
            status
        ));
    }
    let staged = stage.join("orx.exe");
    if !staged.exists() {
        return Err(anyhow!(
            "The installer did not produce {}. The previous orx is untouched.",
            staged.display()
        ));
    }

    let retired = super::retired_path(installed);
    let moved_aside = match std::fs::rename(installed, &retired) {
        Ok(()) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => {
            return Err(anyhow!(
                "Could not move {} aside: {}",
                installed.display(),
                err
            ))
        }
    };
    if let Err(err) = std::fs::rename(&staged, installed) {
        if moved_aside {
            let _ = std::fs::rename(&retired, installed);
        }
        return Err(anyhow!(
            "Could not place the new orx at {}: {}",
            installed.display(),
            err
        ));
    }
    Ok(())
}

/// Windows PowerShell by its fixed path, so a PATH without it (or with another
/// `powershell` first) cannot break updates.
fn powershell() -> PathBuf {
    match std::env::var_os("SystemRoot") {
        Some(root) => Path::new(&root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
        None => "powershell.exe".into(),
    }
}
