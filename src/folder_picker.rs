//! Native folder selection for the loopback dashboard.

use std::path::PathBuf;
use std::process::Command;

use crate::error::{anyhow, Result};

/// Whether a native GUI folder picker can be opened in the current environment.
pub fn can_pick_folder() -> bool {
    if crate::remote::detect_ssh_session().is_some() {
        return false;
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var("SSH_CONNECTION").is_err() && std::env::var("SSH_CLIENT").is_err()
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("SSH_CONNECTION").is_err() && std::env::var("SSH_CLIENT").is_err()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let has_display = std::env::var("DISPLAY")
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
            || std::env::var("WAYLAND_DISPLAY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
        has_display && which_dialog().is_some()
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn which_dialog() -> Option<&'static str> {
    ["zenity", "kdialog"].into_iter().find(|&program| {
        Command::new("which")
            .arg(program)
            .output()
            .is_ok_and(|out| out.status.success())
    })
}

#[cfg(target_os = "macos")]
pub fn pick_folder() -> Result<Option<PathBuf>> {
    if !can_pick_folder() {
        return Err(anyhow!(
            "No native folder picker is available. Running in a remote or headless environment."
        ));
    }
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
    if !can_pick_folder() {
        return Err(anyhow!(
            "No native folder picker is available. Running in a remote or headless environment."
        ));
    }
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
    if !can_pick_folder() {
        return Err(anyhow!(
            "No native folder picker is available. A graphical desktop session with zenity or kdialog is required."
        ));
    }
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Failed to open display")
            || stderr.contains("cannot open display")
            || stderr.contains("No protocol specified")
        {
            return Err(anyhow!("The folder picker failed: {}", stderr.trim()));
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
