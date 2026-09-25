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
    let script = r#"Add-Type -AssemblyName System.Windows.Forms; $dialog = New-Object System.Windows.Forms.FolderBrowserDialog; $dialog.Description = 'Choose a project folder'; if ($dialog.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dialog.SelectedPath }"#;
    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", script])
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
        let mut picker = Command::new(program);
        picker.args(args);
        crate::local::shell_env::restore_host_gui_env(&mut picker);
        // `.` is kdialog's start folder; the app's own is inside the read-only image.
        if let Some(home) = dirs::home_dir() {
            picker.current_dir(home);
        }
        let output = match picker.output() {
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
