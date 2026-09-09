//! Locating the `bash` that runs orx's generated scripts.
//!
//! Windows has no usable one on PATH: Git for Windows exposes only its `cmd`
//! directory, and the `bash.exe` in System32 is the WSL launcher, which cannot
//! see the run directory it would be handed. Found through `git` instead.

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// The bash program to spawn.
#[cfg(not(windows))]
pub fn program() -> std::ffi::OsString {
    "bash".into()
}

/// Never the bare name as a last resort: Windows resolves that through its own
/// PATH search, straight back to the WSL launcher rejected below. Naming where
/// Git for Windows would be fails the spawn instead, pointing at the fix.
#[cfg(windows)]
pub fn program() -> std::ffi::OsString {
    git_bash()
        .or_else(|| {
            crate::local::shell_env::find_on_path("bash").filter(|bash| !is_wsl_launcher(bash))
        })
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| r"C:\Program Files\Git\bin\bash.exe".into())
}

/// The bash shipped alongside `git`. `<git>\cmd\git.exe` is what the installer
/// puts on PATH; bash sits in a sibling directory of that `cmd`.
#[cfg(windows)]
fn git_bash() -> Option<PathBuf> {
    let root = crate::local::shell_env::find_on_path("git")
        .and_then(|git| Some(git.parent()?.parent()?.to_path_buf()))?;
    [root.join(r"bin\bash.exe"), root.join(r"usr\bin\bash.exe")]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// `C:\Windows\System32\bash.exe` is WSL's entry point, not a shell for this
/// filesystem.
#[cfg(windows)]
fn is_wsl_launcher(bash: &Path) -> bool {
    bash.parent().is_some_and(|dir| {
        dir.as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(r"\system32")
    })
}
