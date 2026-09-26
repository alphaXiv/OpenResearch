//! Cross-platform "open URL in browser".

use std::process::{Command, Stdio};

/// Opens `url` in the user's default browser. Best-effort and non-fatal: errors
/// (e.g. no browser, headless) are swallowed, since the caller is expected to
/// have already printed the URL for manual opening. The child is detached so the
/// CLI does not block on it.
pub fn open_browser(url: &str) {
    for mut cmd in openers(url) {
        if cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .is_ok()
        {
            return;
        }
    }
}

/// The open commands to try, in order. On WSL the URL goes to the Windows
/// host's browser through interop; `xdg-open` stays last for distros with a
/// Linux browser or wslu wired into it.
fn openers(url: &str) -> Vec<Command> {
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("open");
        c.arg(url);
        vec![c]
    }
    #[cfg(target_os = "windows")]
    {
        vec![rundll("rundll32.exe", url)]
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut xdg = Command::new("xdg-open");
        xdg.arg(url);
        crate::local::shell_env::restore_host_gui_env(&mut xdg);
        if is_wsl() {
            // rundll32 first: it takes the URL as argv, while some wslview
            // builds re-parse it through `cmd /c start` and truncate at '&'
            // (login URLs carry one). The absolute path covers WSL configs
            // with appendWindowsPath=false — interop still executes it.
            let mut wslview = Command::new("wslview");
            wslview.arg(url);
            return vec![
                rundll("rundll32.exe", url),
                rundll("/mnt/c/Windows/System32/rundll32.exe", url),
                wslview,
                xdg,
            ];
        }
        vec![xdg]
    }
}

/// `url.dll,FileProtocolHandler` opens the default browser with the URL as a
/// plain argv element — no shell re-parses it, so '&' in query strings
/// survives. Works on Windows and on WSL through interop.
#[cfg(not(target_os = "macos"))]
fn rundll(program: &str, url: &str) -> Command {
    let mut c = Command::new(program);
    c.args(["url.dll,FileProtocolHandler", url]);
    c
}

/// WSL sets `WSL_INTEROP`/`WSL_DISTRO_NAME` and stamps "microsoft" into the
/// kernel release.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn is_wsl() -> bool {
    std::env::var_os("WSL_INTEROP").is_some()
        || std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .is_ok_and(|release| release.to_ascii_lowercase().contains("microsoft"))
}
