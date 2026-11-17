//! Self-update on Windows. Where the shell installer updates other platforms,
//! orx unpacks the release archive itself, then does the two things Windows
//! makes the binary do for itself.
//!
//! Windows refuses to overwrite a mapped executable but allows renaming it. So
//! the release is unpacked into a staging directory, and only then is the
//! installed `orx.exe` renamed aside and the new one renamed into its place —
//! two renames, with no window in which `orx.exe` is half-written. The retired
//! copy is deleted by a later start, once every process that mapped it has
//! exited.
//!
//! While the desktop app's backend runs, only the unpacking and a first run of the
//! result happen; the restart then does just the two renames, from the staged
//! directory. A CLI install is swapped in one go.
//!
//! There is no `exec`, so a restarting `orx up` or app spawns the new binary and exits;
//! the child waits on the parent's process handle before binding the port.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::LatestRelease;
use crate::error::{anyhow, Result};

pub(crate) async fn fetch_manifest(
    timeout: std::time::Duration,
) -> Result<Option<super::SingleAssetManifest>> {
    Ok(super::fetch_canary_manifest(timeout)
        .await?
        .and_then(|mut manifest| {
            manifest
                .assets
                .remove("windows")
                .map(|asset| super::SingleAssetManifest {
                    version: manifest.version,
                    tag: manifest.tag,
                    asset: asset.asset,
                    sha256: asset.sha256,
                    ancestors: manifest.ancestors,
                })
        }))
}

/// Environment a relaunched `orx up` or app reads to wait for the process it replaces.
const RELAUNCH_WAIT_PID_ENV: &str = "ORX_RELAUNCH_WAIT_PID";

/// Spawn the copy on disk with `args` and `envs`, telling it which process to
/// wait for, then exit. Returns only when the spawn failed.
pub(super) fn relaunch(args: Vec<std::ffi::OsString>, envs: &[(&str, String)]) -> std::io::Error {
    // The load-time path, which the update swapped a new file under.
    let Ok(exe) = std::env::current_exe() else {
        return std::io::Error::other("could not resolve the running executable");
    };
    let spawned = crate::process::command(exe)
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

/// Where a launch-checked release waits beside the installed `orx.exe` for the
/// restart that swaps it in. Deliberately not [`STAGE_PREFIX`]: the sweep would
/// delete it, and it outlives the run that made it.
const STAGED_DIR: &str = ".orx-staged";

/// Defender and SmartScreen scan an executable on its first run, which can take a while.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(120);

/// Install the `orx.exe` in a verified release `archive` at `installed`.
pub(crate) fn install(installed: &Path, archive: &[u8]) -> Result<()> {
    with_scratch(installed, |scratch| {
        unpack(scratch, archive)?;
        swap_staged(installed, &scratch.join("orx.exe"))
    })
}

/// The stage directory for the install at `installed`, whether or not one is there.
pub(crate) fn staged_dir(installed: &Path) -> PathBuf {
    installed.with_file_name(STAGED_DIR)
}

/// Unpack `release` from its verified `archive` and stage it for `installed`.
pub(crate) fn stage_archive(
    installed: &Path,
    release: &LatestRelease,
    archive: &[u8],
) -> Result<()> {
    stage(installed, release, |scratch| unpack(scratch, archive))
}

/// Stage a desktop `orx.exe` for `release` after checking it is signed by the
/// expected publisher.
pub(crate) fn stage_binary(installed: &Path, release: &LatestRelease, bytes: &[u8]) -> Result<()> {
    if crate::telemetry::build_channel() != "production" {
        return Err(anyhow!("This OpenResearch app is not an official build."));
    }
    stage(installed, release, |scratch| {
        let staged = scratch.join("orx.exe");
        std::fs::write(&staged, bytes)?;
        if signer(&staged).as_deref() != Some("alphaXiv Inc.") {
            return Err(anyhow!(
                "The downloaded app is not signed by the expected publisher."
            ));
        }
        Ok(())
    })
}

/// Build the release in a scratch directory with `produce`, run it once, and only
/// then publish it as the staged directory, so that directory only ever holds an
/// `orx.exe` that started and a failed attempt leaves the previous one.
fn stage(
    installed: &Path,
    release: &LatestRelease,
    produce: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    with_scratch(installed, |scratch| {
        produce(scratch)?;
        launch_check(&scratch.join("orx.exe"))?;
        super::write_staged_release(scratch, &release.version, &release.tag)?;

        let staged = staged_dir(installed);
        let _ = std::fs::remove_dir_all(&staged);
        std::fs::rename(scratch, &staged)
            .map_err(|e| anyhow!("Could not stage the new orx at {}: {}", staged.display(), e))
    })
}

fn with_scratch(installed: &Path, run: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    // Beside the target, not in %TEMP%: a rename cannot cross volumes, and
    // preflight already proved this directory writable.
    let dir = installed
        .parent()
        .ok_or_else(|| anyhow!("{} has no parent directory", installed.display()))?;
    sweep_stale_stages(dir);
    let scratch = dir.join(format!("{STAGE_PREFIX}{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&scratch)?;

    let result = run(&scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Run the staged binary's CLI once. A first run is the slow one, so doing it here,
/// where a failure just rejects the update, leaves the restart a fast second run of
/// a binary known to start.
fn launch_check(exe: &Path) -> Result<()> {
    let mut child = crate::process::command(exe)
        .args(["--no-telemetry", "version", "--build-id"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("The downloaded app could not be started: {e}"))?;
    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    loop {
        match child.try_wait()? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => {
                return Err(anyhow!(
                    "The downloaded app failed to start ({status}). Nothing was installed."
                ))
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(anyhow!(
                    "The downloaded app did not start within {} seconds. Nothing was installed.",
                    LAUNCH_TIMEOUT.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Swap the staged `orx.exe` in for `installed`. The old binary is restored if the
/// swap fails.
pub(crate) fn install_staged(installed: &Path) -> Result<()> {
    let staged = staged_dir(installed);
    swap_staged(installed, &staged.join("orx.exe"))?;
    let _ = std::fs::remove_dir_all(&staged);
    Ok(())
}

fn unpack(scratch: &Path, archive: &[u8]) -> Result<()> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|e| anyhow!("The downloaded orx archive could not be read: {e}"))?;
    let mut exe = archive
        .by_name("orx.exe")
        .map_err(|e| anyhow!("The downloaded orx archive has no orx.exe: {e}"))?;
    let staged = scratch.join("orx.exe");
    std::fs::File::create(&staged)
        .and_then(|mut file| std::io::copy(&mut exe, &mut file))
        .map_err(|e| anyhow!("Could not unpack the new orx to {}: {e}", staged.display()))?;
    Ok(())
}

fn swap_staged(installed: &Path, staged: &Path) -> Result<()> {
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
    if let Err(err) = std::fs::rename(staged, installed) {
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

/// The subject of `file`'s Authenticode signature when Windows trusts it and it
/// is timestamped, so it stays valid after the certificate expires.
fn signer(file: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO,
        WTD_CHOICE_FILE, WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY,
        WTD_UI_NONE,
    };

    let path: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: path.as_ptr(),
        ..Default::default()
    };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let mut verify = |data: &mut WINTRUST_DATA| {
        // SAFETY: `data` and the file info it points to outlive the call.
        unsafe {
            WinVerifyTrust(
                INVALID_HANDLE_VALUE.cast(),
                &mut action,
                (data as *mut WINTRUST_DATA).cast(),
            )
        }
    };

    let trusted = verify(&mut data) == 0;
    // SAFETY: the verify left its state open until the close below, and a
    // signer with a chain points at that many certificates.
    let name = trusted.then(|| unsafe {
        let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
        let signer = WTHelperGetProvSignerFromChain(provider, 0, 0, 0);
        let timestamper = WTHelperGetProvSignerFromChain(provider, 0, 1, 0);
        if signer.is_null() || (*signer).csCertChain == 0 {
            return None;
        }
        if timestamper.is_null() || (*timestamper).csCertChain == 0 {
            return None;
        }
        let mut name = [0u16; 256];
        let length = CertGetNameStringW(
            (*(*signer).pasCertChain).pCert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            name.as_mut_ptr(),
            name.len() as u32,
        );
        // The length counts the terminator.
        Some(String::from_utf16_lossy(
            &name[..length.saturating_sub(1) as usize],
        ))
    });
    data.dwStateAction = WTD_STATEACTION_CLOSE;
    verify(&mut data);
    name.flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_archive_with_orx_exe_replaces_the_installed_binary() {
        fn archive(name: &str) -> Vec<u8> {
            use std::io::Write as _;
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip.start_file::<_, ()>(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"new").unwrap();
            zip.finish().unwrap().into_inner()
        }
        let dir = std::env::temp_dir().join(format!("orx-archive-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let installed = dir.join("orx.exe");
        std::fs::write(&installed, "old").unwrap();

        assert!(install(&installed, b"not a zip").is_err());
        assert!(install(&installed, &archive("README.md")).is_err());
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "old");

        install(&installed, &archive("orx.exe")).unwrap();
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "new");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_unsigned_binary_has_no_trusted_signer() {
        assert_eq!(signer(&std::env::current_exe().unwrap()), None);
    }

    #[test]
    fn a_restart_swaps_the_staged_exe_in_and_clears_the_stage() {
        let dir = std::env::temp_dir().join(format!("orx-windows-{}", uuid::Uuid::new_v4()));
        let installed = dir.join("orx.exe");
        let staged = staged_dir(&installed);
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(&installed, "old").unwrap();
        std::fs::write(staged.join("orx.exe"), "new").unwrap();
        super::super::write_staged_release(
            &staged,
            &semver::Version::parse("0.2.18").unwrap(),
            "v0.2.18",
        )
        .unwrap();

        install_staged(&installed).unwrap();

        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "new");
        assert!(!staged.exists());
        let retired: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| *path != installed)
            .collect();
        assert_eq!(retired.len(), 1);
        assert_eq!(std::fs::read_to_string(&retired[0]).unwrap(), "old");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
