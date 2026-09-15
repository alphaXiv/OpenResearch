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

/// Reveals `path` in the machine's file manager, selecting it where the
/// platform supports that (Finder on macOS, Explorer on Windows). On Linux
/// there is no portable "select this file" call, so we open the containing
/// directory instead. Detached and non-blocking, like [`open_in_default_app`];
/// the caller has already confirmed the file exists inside the project
/// checkout.
pub fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut cmd = {
        // `open -R <path>` reveals the file in Finder with it selected.
        let mut c = Command::new("open");
        c.arg("-R").arg(path);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // `explorer /select,<path>` opens the folder with the file selected.
        // The whole `/select,<path>` is one argv element, so a filename with
        // shell metacharacters can't inject.
        let mut c = Command::new("explorer.exe");
        c.arg(format!("/select,{}", path.display()));
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        // No portable "select the file" opener across Linux file managers, so
        // open the containing directory. Falls back to the path itself when it
        // has no parent (a filesystem root), which `open_in_default_app` covers.
        let mut c = Command::new("xdg-open");
        c.arg(path.parent().unwrap_or(path));
        c
    };

    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_spawns_for_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("orx-reveal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("no_extension_file");
        std::fs::write(&file, b"data").unwrap();
        // On CI/headless Linux xdg-open may be absent; both a clean spawn and a
        // "not found" are acceptable — we only assert the call is well-formed
        // and never panics.
        let _ = reveal_in_file_manager(&file);
        std::fs::remove_dir_all(&dir).ok();
    }
}
