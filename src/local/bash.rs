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

/// `<git>\cmd\git.exe` is what the installer puts on PATH, so the install root
/// is two levels up.
#[cfg(windows)]
fn git_root() -> Option<PathBuf> {
    crate::local::shell_env::find_on_path("git")
        .and_then(|git| Some(git.parent()?.parent()?.to_path_buf()))
}

/// The bash shipped alongside `git`.
#[cfg(windows)]
fn git_bash() -> Option<PathBuf> {
    let root = git_root()?;
    [root.join(r"bin\bash.exe"), root.join(r"usr\bin\bash.exe")]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// `base` with the shell's own toolchain in front, or None where the shell
/// already has one.
///
/// Git for Windows keeps coreutils in `usr\bin` and puts only `cmd` on the
/// Windows PATH, so a bash spawned from a Windows process has no `mkdir` and
/// the generated scripts die on their first command. Scoped to the bash we
/// spawn: fronting these for every child would shadow Windows' own `find` and
/// `sort` with the MSYS ones.
#[cfg(windows)]
pub fn path_with_toolchain(base: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    let root = git_root()?;
    let mut path = std::ffi::OsString::new();
    for dir in [r"usr\bin", r"mingw64\bin", "bin"] {
        let dir = root.join(dir);
        if dir.is_dir() {
            path.push(dir);
            path.push(crate::local::shell_env::PATH_LIST_SEPARATOR);
        }
    }
    if path.is_empty() {
        return None;
    }
    if let Some(base) = base.filter(|base| !base.is_empty()) {
        path.push(base);
    }
    Some(path)
}

#[cfg(not(windows))]
pub fn path_with_toolchain(_base: Option<std::ffi::OsString>) -> Option<std::ffi::OsString> {
    None
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
