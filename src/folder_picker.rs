//! Native folder selection for the loopback dashboard.

use std::path::PathBuf;
#[cfg(not(target_os = "windows"))]
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
    use raw_window_handle::{
        DisplayHandle, HasDisplayHandle, HasWindowHandle, RawWindowHandle, Win32WindowHandle,
        WindowHandle,
    };
    use std::num::NonZeroIsize;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SetForegroundWindow, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP, WS_VISIBLE,
    };

    struct Owner(NonZeroIsize);
    impl HasWindowHandle for Owner {
        fn window_handle(
            &self,
        ) -> std::result::Result<WindowHandle<'_>, raw_window_handle::HandleError> {
            // SAFETY: the owner remains alive for the duration of the borrowed handle.
            Ok(unsafe {
                WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(self.0)))
            })
        }
    }
    impl HasDisplayHandle for Owner {
        fn display_handle(
            &self,
        ) -> std::result::Result<DisplayHandle<'_>, raw_window_handle::HandleError> {
            Ok(DisplayHandle::windows())
        }
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: this window was created on this thread and is no longer borrowed.
            unsafe {
                DestroyWindow(self.0.get() as _);
            }
        }
    }

    // A visible topmost owner keeps the modal dialog in front of the dashboard.
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            windows_sys::core::w!("STATIC"),
            windows_sys::core::w!(""),
            WS_POPUP | WS_VISIBLE,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    let owner = Owner(NonZeroIsize::new(hwnd as isize).ok_or_else(|| {
        anyhow!(
            "Could not open the folder picker: {}",
            std::io::Error::last_os_error()
        )
    })?);
    unsafe {
        SetForegroundWindow(hwnd);
    }
    Ok(rfd::FileDialog::new()
        .set_title("Choose a project folder")
        .set_parent(&owner)
        .pick_folder())
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
        if let Ok(base) = crate::local::projects::relative_base() {
            picker.current_dir(base);
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
