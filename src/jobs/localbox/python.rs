//! Keeps a run's bare `python`/`python3` off the Microsoft Store stub, whose
//! 9009 exit reaches bash as 49 with nothing in the log.

use std::ffi::OsStr;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::jobs::ssh::sh_quote;
use crate::local::bash::bash_path;

const STUB_ERROR: &str =
    "orx: Python on this machine resolves to the Microsoft Store alias, not an installed Python. \
    Install Python from https://www.python.org/downloads/windows/ with \"Add python.exe to PATH\" \
    checked, or turn off the python.exe and python3.exe entries under \
    \"Manage app execution aliases\" in Settings, then rerun.";

/// Resolved per call, so a venv the run command activates (which has no `python3.exe`) wins.
const FORWARD_TO_PYTHON: &str = "#!/bin/sh\nexec python \"$@\"\n";

/// The run.sh line that routes whichever of `python`/`python3` hit the stub to
/// a real interpreter, or to a clear error.
pub(super) fn prelude(dir: &Path) -> std::io::Result<String> {
    let Some(path) = crate::local::shell_env::search_path() else {
        return Ok(String::new());
    };
    let python = first_on_path(&path, "python");
    let python_stub = python.as_deref().is_some_and(is_store_stub);
    let python3_stub = first_on_path(&path, "python3")
        .as_deref()
        .is_some_and(is_store_stub);
    if !python_stub && !python3_stub {
        return Ok(String::new());
    }
    let python_ok = python.is_some() && !python_stub;
    let mut entries = Vec::new();
    let mut shims = Vec::new();
    if !python_ok {
        match real_python(&path).as_deref().and_then(Path::parent) {
            // On PATH rather than shimmed, so native children spawning `python` find it too.
            Some(home) => entries.extend(
                [home.to_path_buf(), home.join("Scripts")]
                    .into_iter()
                    .filter(|dir| dir.is_dir()),
            ),
            None => shims.push(("python", stub_shim())),
        }
    }
    if python3_stub {
        shims.push(("python3", FORWARD_TO_PYTHON.to_string()));
    }
    if !shims.is_empty() {
        let shim_dir = dir.join("python-shims");
        std::fs::create_dir_all(&shim_dir)?;
        for (name, script) in shims {
            std::fs::write(shim_dir.join(name), script)?;
        }
        entries.push(shim_dir);
    }
    let entries: Vec<String> = entries
        .iter()
        .map(|dir| sh_quote(&bash_path(dir)))
        .collect();
    Ok(format!("export PATH={}:\"$PATH\"\n", entries.join(":")))
}

fn stub_shim() -> String {
    format!("#!/bin/sh\necho {} >&2\nexit 127\n", sh_quote(STUB_ERROR))
}

/// What bash tries for `name`, in order: the extensionless file (pyenv-win's shim) before `.exe`.
fn candidates<'a>(path: &'a OsStr, name: &'static str) -> impl Iterator<Item = PathBuf> + 'a {
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .flat_map(move |dir| [dir.join(name), dir.join(format!("{name}.exe"))])
        .filter(|candidate| exists(candidate))
}

fn first_on_path(path: &OsStr, name: &'static str) -> Option<PathBuf> {
    candidates(path, name).next()
}

/// The first `python` past the stub, else the one `py` picks for installs
/// that skipped "Add python.exe to PATH".
fn real_python(path: &OsStr) -> Option<PathBuf> {
    candidates(path, "python")
        .find(|candidate| !in_windows_apps(candidate))
        .or_else(py_launcher_python)
}

fn py_launcher_python() -> Option<PathBuf> {
    let py = crate::local::shell_env::find_on_path("py")?;
    let out = hidden(&py)
        .args(["-3", "-c", "import sys; print(sys.executable)"])
        .env("PYTHONUTF8", "1")
        .output()
        .ok()
        .filter(|out| out.status.success())?;
    let python = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    exists(&python).then_some(python)
}

/// A real Store install also lives in WindowsApps; only the stub exits 9009.
fn is_store_stub(python: &Path) -> bool {
    in_windows_apps(python)
        && hidden(python)
            .arg("--version")
            .stdout(Stdio::null())
            .status()
            .is_ok_and(|status| status.code() == Some(9009))
}

fn in_windows_apps(path: &Path) -> bool {
    path.to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\")
        .contains(r"\microsoft\windowsapps\")
}

/// The Store alias is a reparse point that cannot be followed, so `is_file` misses it.
fn exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.is_dir())
}

fn hidden(program: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_shim_explains_and_exits_127() {
        let script = stub_shim();
        assert!(script.contains("Microsoft Store alias"));
        assert!(script.ends_with(">&2\nexit 127\n"));
    }

    #[test]
    fn git_bash_runs_the_shims_and_python3_reaches_the_error() {
        let dir = std::env::temp_dir().join(format!("orx-shims-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("python"), stub_shim()).unwrap();
        std::fs::write(dir.join("python3"), FORWARD_TO_PYTHON).unwrap();
        let mut path = std::ffi::OsString::from(&dir);
        if let Some(rest) = crate::local::shell_env::search_path() {
            path.push(";");
            path.push(rest);
        }
        let out = Command::new(crate::local::bash::program())
            .args(["-c", "python3 --version"])
            .env("PATH", path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(127));
        assert!(String::from_utf8_lossy(&out.stderr).contains("Microsoft Store alias"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_forward_slashed_path_entry_is_still_windows_apps() {
        assert!(in_windows_apps(Path::new(
            r"C:/Users/me/AppData/Local/Microsoft/WindowsApps\python.exe"
        )));
        assert!(!in_windows_apps(Path::new(r"C:\Python312\python.exe")));
    }

    #[test]
    fn bash_resolution_prefers_an_extensionless_shim() {
        let root = std::env::temp_dir().join(format!("orx-python-{}", uuid::Uuid::new_v4()));
        let (pyenv, python_org) = (root.join("pyenv"), root.join("python-org"));
        std::fs::create_dir_all(&pyenv).unwrap();
        std::fs::create_dir_all(&python_org).unwrap();
        std::fs::write(pyenv.join("python"), "").unwrap();
        std::fs::write(pyenv.join("python.exe"), "").unwrap();
        std::fs::write(python_org.join("python.exe"), "").unwrap();
        let path = std::env::join_paths([&python_org, &pyenv]).unwrap();
        assert_eq!(
            first_on_path(&path, "python"),
            Some(python_org.join("python.exe"))
        );
        let path = std::env::join_paths([&pyenv, &python_org]).unwrap();
        assert_eq!(first_on_path(&path, "python"), Some(pyenv.join("python")));
        assert_eq!(first_on_path(&path, "python3"), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
