//! Native folder selection for the loopback dashboard.

use std::path::PathBuf;
use std::process::Command;

use crate::error::{anyhow, Result};

#[cfg(target_os = "macos")]
pub fn pick_folder() -> Result<Option<PathBuf>> {
    let output = Command::new("osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Choose a project folder\")",
        ])
        .output()
        .map_err(|error| anyhow!("Could not open the folder picker: {error}"))?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("-128") || stderr.to_ascii_lowercase().contains("user canceled") {
        return Ok(None);
    }
    Err(anyhow!("The folder picker failed: {}", stderr.trim()))
}

#[cfg(target_os = "windows")]
pub fn pick_folder() -> Result<Option<PathBuf>> {
    let output = windows_picker_command(include_str!("folder_picker.ps1"))
        .output()
        .map_err(|error| anyhow!("Could not open the folder picker: {error}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "The folder picker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!path.is_empty()).then(|| PathBuf::from(path)))
}

#[cfg(target_os = "windows")]
fn windows_picker_command(script: &str) -> Command {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-STA", "-NonInteractive", "-Command", script])
        // The picker has its own owner window; it does not need a console.
        .creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn pick_folder() -> Result<Option<PathBuf>> {
    for (program, args) in [
        (
            "zenity",
            &[
                "--file-selection",
                "--directory",
                "--title=Choose a project folder",
            ][..],
        ),
        (
            "kdialog",
            &[
                "--getexistingdirectory",
                ".",
                "--title",
                "Choose a project folder",
            ][..],
        ),
    ] {
        let output = match Command::new(program).args(args).output() {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(anyhow!("Could not open the folder picker: {error}")),
        };
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Ok((!path.is_empty()).then(|| PathBuf::from(path)));
        }
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        return Err(anyhow!(
            "The folder picker failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Err(anyhow!(
        "No native folder picker is available. Install zenity or kdialog, then try again."
    ))
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use std::process::Output;
    use std::time::{Duration, Instant};

    fn run_picker(result: &str, path: &str) -> Output {
        let script = format!(
            "{}\ntry {{ & {{ {} }} }} finally {{ Assert-PickerDisposed }}",
            include_str!("folder_picker_test.ps1"),
            include_str!("folder_picker.ps1")
        );
        let mut child = windows_picker_command(&script)
            .env("ORX_PICKER_TEST_RESULT", result)
            .env("ORX_PICKER_TEST_PATH", path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("start the Windows folder picker test");
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().expect("poll folder picker").is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("folder picker test did not finish within 15 seconds");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        child.wait_with_output().expect("read folder picker output")
    }

    #[test]
    fn windows_picker_shows_an_owned_dialog_and_returns_the_selection() {
        let output = run_picker("ok", r"C:\Research project");
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stderr.is_empty(), "{:?}", output);
        assert_eq!(
            String::from_utf8(output.stdout).unwrap().trim(),
            r"C:\Research project"
        );
    }

    #[test]
    fn windows_picker_preserves_unicode_paths() {
        let path = "C:\\Badania żółć\\研究";
        let output = run_picker("ok", path);
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stderr.is_empty(), "{:?}", output);
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), path);
    }

    #[test]
    fn windows_picker_cancellation_returns_no_path() {
        let output = run_picker("cancel", r"C:\Unused");
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stdout.is_empty(), "{:?}", output);
        assert!(output.stderr.is_empty(), "{:?}", output);
    }

    #[test]
    fn windows_picker_errors_fail_the_process_and_release_the_owner() {
        let output = run_picker("error", r"C:\Unused");
        assert!(!output.status.success(), "{:?}", output);
        assert!(output.stdout.is_empty(), "{:?}", output);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("Simulated folder picker failure"),
            "{stderr}"
        );
        assert!(
            !stderr.contains("Picker resources were not disposed"),
            "{stderr}"
        );
    }
}
