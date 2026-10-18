//! The `bash` that runs orx's generated scripts. On Windows PATH's is usually the WSL
//! launcher, which cannot see the run dir, so this one is found via `git`.

/// Preserve direct CMD invocations without changing other native commands.
pub fn local_run_command(command: &str) -> std::borrow::Cow<'_, str> {
    #[cfg(windows)]
    {
        let head = command.trim_start();
        let executable = match head.chars().next() {
            Some(quote @ ('\'' | '"')) => match head[1..].split_once(quote) {
                Some((executable, rest))
                    if rest.is_empty()
                        || rest.starts_with(char::is_whitespace)
                        || rest.starts_with([';', '&', '|', '<', '>']) =>
                {
                    executable
                }
                _ => "",
            },
            _ => head.split_whitespace().next().unwrap_or_default(),
        };
        if !head.starts_with(['\'', '"'])
            && executable.contains(['=', ';', '&', '|', '<', '>', '(', ')', '`'])
        {
            return command.into();
        }
        let executable = executable.rsplit(['/', '\\']).next().unwrap_or_default();
        if executable.eq_ignore_ascii_case("cmd")
            || executable.eq_ignore_ascii_case("cmd.exe")
            || matches!(executable, "$COMSPEC" | "${COMSPEC}")
        {
            // CMD switches and command text are not POSIX paths for MSYS to convert.
            let leading = &command[..command.len() - head.len()];
            return format!("{leading}MSYS2_ARG_CONV_EXCL='*' {head}").into();
        }
    }
    command.into()
}

#[cfg(windows)]
use std::path::{Path, PathBuf};

/// The bash program to spawn.
#[cfg(not(windows))]
pub fn program() -> std::ffi::OsString {
    "bash".into()
}

/// Falls back to Git's default path, not bare `bash`, which Windows resolves to the WSL launcher.
#[cfg(windows)]
pub fn program() -> std::ffi::OsString {
    usable_bash()
        .map(std::ffi::OsString::from)
        .unwrap_or_else(|| r"C:\Program Files\Git\bin\bash.exe".into())
}

/// Git for Windows' bash, else any on PATH that is not the WSL launcher.
#[cfg(windows)]
fn usable_bash() -> Option<PathBuf> {
    git_bash().or_else(|| {
        crate::local::shell_env::find_on_path("bash").filter(|bash| !is_wsl_launcher(bash))
    })
}

/// Warn at startup rather than fail at the first run's first command.
#[cfg(windows)]
pub fn missing_toolchain() -> Option<&'static str> {
    usable_bash().is_none().then_some(
        "Git for Windows not found — orx needs the bash it ships to run experiments. \
         Install it from the dashboard or from https://git-scm.com/download/win.",
    )
}

#[cfg(not(windows))]
pub fn missing_toolchain() -> Option<&'static str> {
    None
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

/// `base` behind Git for Windows' coreutils, which its installer leaves off PATH.
/// Only for the bash we spawn, so MSYS `find`/`sort` never shadow Windows' own.
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

/// `/c/…`, not `C:/…`: GNU tar reads a drive colon as `host:path`.
#[cfg(windows)]
pub fn bash_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut head = text.chars();
    match (head.next(), head.next(), head.next()) {
        (Some(drive), Some(':'), Some('/')) if drive.is_ascii_alphabetic() => {
            format!("/{}/{}", drive.to_ascii_lowercase(), &text[3..])
        }
        _ => text,
    }
}

#[cfg(not(windows))]
pub fn bash_path(path: &std::path::Path) -> String {
    path.to_string_lossy().into_owned()
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn only_direct_cmd_commands_skip_msys_argument_conversion() {
        for command in [
            "cmd /c run_validation.cmd",
            "CMD.EXE /c run_validation.cmd",
            "cMd.ExE /c run_validation.cmd",
            r#""C:\Windows\System32\cmd.exe" /c run_validation.cmd"#,
            "'/c/Windows/System32/CMD.EXE' /c run_validation.cmd",
            r#""C:\Program Files\cmd.exe" /c run_validation.cmd"#,
            r#""$COMSPEC" /c run_validation.cmd"#,
            r#""${COMSPEC}" /c run_validation.cmd"#,
        ] {
            assert_eq!(
                local_run_command(command),
                format!("MSYS2_ARG_CONV_EXCL='*' {command}")
            );
        }
        for command in [
            "",
            "python script.py /c/data",
            "bash -c 'cmd /c run_validation.cmd'",
            "'cmd'helper /c/data",
            "\"cmd.exe\"helper /c/data",
            "OUT_DIR=./cmd python script.py /c/data",
            "echo;/tmp/cmd /c/data",
        ] {
            assert_eq!(local_run_command(command), command);
        }
        assert_eq!(
            local_run_command("\n\tcmd /c run_validation.cmd"),
            "\n\tMSYS2_ARG_CONV_EXCL='*' cmd /c run_validation.cmd"
        );
    }

    #[test]
    fn a_windows_path_reaches_the_shell_without_a_drive_colon() {
        // A colon anywhere before the first slash makes GNU tar treat the
        // argument as `host:path`.
        assert_eq!(
            bash_path(Path::new(r"C:\Users\me\source.tar")),
            "/c/Users/me/source.tar"
        );
        assert_eq!(
            bash_path(Path::new(r"\\server\share\x")),
            "//server/share/x"
        );
    }
}
