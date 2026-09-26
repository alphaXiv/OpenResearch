//! Open a file in the machine's default app for its type, or reveal it in the
//! file manager.
//!
//! Local-only: `orx up` runs on the user's own machine, so the API process can
//! hand a file to the OS opener, which routes it to whatever the user has set as
//! the default for that file type (their editor, for source files). Spawned
//! detached so the API never blocks on the editor.

use std::process::{Command, Stdio};

/// Spawns `cmd` detached with all stdio nulled, so the API never blocks on or
/// inherits handles from the GUI app.
fn spawn_detached(cmd: &mut Command) -> std::io::Result<()> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Opens `path` with the OS default application. Detached and non-blocking; the
/// caller has already confirmed the file exists inside the project checkout.
pub fn open_in_default_app(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `explorer.exe <path>` opens the file with its associated app (like a
        // double-click) and takes the path as one argv element — no cmd.exe
        // reparse, so a filename with shell metacharacters can't inject.
        let mut c = Command::new("explorer.exe");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };

    spawn_detached(&mut cmd)
}

/// Command that reveals `path` in the OS file manager (Linux opens its parent directory).
fn reveal_command(path: &std::path::Path) -> Command {
    #[cfg(target_os = "macos")]
    let cmd = {
        // `open -R <path>` reveals the file in Finder with it selected.
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let cmd = {
        // explorer splits on commas and rejects \" escapes: quote via raw_arg (NTFS bans '"').
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("explorer.exe");
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(path);
        arg.push("\"");
        c.raw_arg(arg);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = {
        // No portable "select the file" opener on Linux, so open the containing directory.
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd
}

/// Reveals `path` in the OS file manager, detached like [`open_in_default_app`].
pub fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    spawn_detached(&mut reveal_command(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn reveal_command_selects_file_in_finder() {
        let cmd = reveal_command(std::path::Path::new("/tmp/orx/data.bin"));
        assert_eq!(cmd.get_program(), "open");
        assert_eq!(args(&cmd), ["-R", "/tmp/orx/data.bin"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reveal_command_selects_file_in_explorer() {
        // Commas are legal in NTFS names but are explorer's token separator;
        // the quoted form keeps the selection target intact.
        let cmd = reveal_command(std::path::Path::new(r"C:\work\a,b.bin"));
        assert_eq!(cmd.get_program(), "explorer.exe");
        assert_eq!(args(&cmd), [r#"/select,"C:\work\a,b.bin""#]);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn reveal_command_opens_containing_dir() {
        let cmd = reveal_command(std::path::Path::new("/tmp/orx/data.bin"));
        assert_eq!(cmd.get_program(), "xdg-open");
        assert_eq!(args(&cmd), ["/tmp/orx"]);
    }
}
