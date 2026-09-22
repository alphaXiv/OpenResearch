//! Open a file in the machine's default app for its type.
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

/// Builds the command that reveals `path` in the machine's file manager,
/// selecting it where the platform supports that (Finder on macOS, Explorer on
/// Windows). On Linux there is no portable "select this file" call, so the
/// command opens the containing directory instead.
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
        // `explorer /select,"<path>"` opens the folder with the file selected.
        // explorer tokenizes its own command line on commas, so the path is
        // quoted; an OsString keeps names that aren't valid UTF-16 intact.
        // Still one argv element — no cmd.exe reparse, so metacharacters can't
        // inject.
        let mut c = Command::new("explorer.exe");
        let mut arg = std::ffi::OsString::from("/select,\"");
        arg.push(path);
        arg.push("\"");
        c.arg(arg);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let cmd = {
        // No portable "select the file" opener across Linux file managers, so
        // open the containing directory. `unwrap_or` only covers a filesystem
        // root; callers pass a canonicalized path under a checkout root.
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };
    cmd
}

/// Reveals `path` in the machine's file manager (see [`reveal_command`]).
/// Detached and non-blocking, like [`open_in_default_app`]; the caller has
/// already confirmed the file exists inside the project checkout.
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
