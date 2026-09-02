//! Open a file in the machine's default app for its type.
//!
//! Local-only: `orx up` runs on the user's own machine, so the API process can
//! hand a file to the OS opener, which routes it to whatever the user has set as
//! the default for that file type (their editor, for source files). Spawned
//! detached so the API never blocks on the editor.

use std::process::{Command, Stdio};

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

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}
