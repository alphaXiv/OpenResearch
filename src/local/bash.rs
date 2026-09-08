//! Locating the `bash` that runs orx's generated scripts.
//!
//! Unix has one on PATH and that is the end of it. Windows usually does not:
//! Git for Windows puts only its `cmd` directory on PATH, and the `bash.exe`
//! that *is* commonly there is the WSL launcher in System32 — a different
//! machine with a different filesystem, where the Windows run directory we hand
//! it does not exist. So bash is found through `git` instead, which locates the
//! msys build that shares our view of the disk.

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// The bash program to spawn. Falls back to the bare name so a machine without
/// one fails at the spawn, where callers already report it.
#[cfg(not(windows))]
pub fn program() -> std::ffi::OsString {
    "bash".into()
}

#[cfg(windows)]
pub fn program() -> std::ffi::OsString {
    git_bash()
        .or_else(|| {
            // A PATH hit is still better than nothing, as long as it is not the
            // WSL launcher.
            crate::local::shell_env::find_on_path("bash").filter(|bash| !is_wsl_launcher(bash))
        })
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| "bash".into())
}

/// The bash shipped alongside `git`. `<git>\cmd\git.exe` is what the installer
/// puts on PATH; bash sits in a sibling directory of that `cmd`.
#[cfg(windows)]
fn git_bash() -> Option<PathBuf> {
    let roots = crate::local::shell_env::find_on_path("git")
        .and_then(|git| Some(git.parent()?.parent()?.to_path_buf()))
        .into_iter()
        .chain(install_roots());
    roots
        .flat_map(|root| [root.join(r"bin\bash.exe"), root.join(r"usr\bin\bash.exe")])
        .find(|candidate| candidate.is_file())
}

/// Where the Git for Windows installer puts things when it is not on PATH at
/// all — per-machine for the system installer, per-user for the portable one.
#[cfg(windows)]
fn install_roots() -> impl Iterator<Item = PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .map(|dir| PathBuf::from(dir).join("Git"))
        .chain(std::env::var_os("LOCALAPPDATA").map(|dir| PathBuf::from(dir).join(r"Programs\Git")))
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
