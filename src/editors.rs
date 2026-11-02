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
        crate::local::shell_env::restore_host_gui_env(&mut c);
        c
    };

    spawn_detached(&mut cmd)
}

/// Command that reveals `path` in the OS file manager (Linux opens directories or the file's parent).
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
        c.arg(if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        });
        crate::local::shell_env::restore_host_gui_env(&mut c);
        c
    };
    cmd
}

/// Reveals `path` in the OS file manager, detached like [`open_in_default_app`].
pub fn reveal_in_file_manager(path: &std::path::Path) -> std::io::Result<()> {
    spawn_detached(&mut reveal_command(path))
}

/// The directory handler named by the platform, if it can be identified.
pub fn file_manager_name(_locale: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        None
    }
    #[cfg(target_os = "windows")]
    {
        None
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut command = Command::new("xdg-mime");
        command.args(["query", "default", "inode/directory"]);
        crate::local::shell_env::restore_host_gui_env(&mut command);
        let output = command.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let id = String::from_utf8(output.stdout).ok()?;
        let id = id.trim();
        if id.is_empty() || id.contains(['/', '\\']) {
            return None;
        }
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| home.map(|path| path.join(".local/share")));
        let dirs = crate::local::shell_env::host_gui_env()
            .into_iter()
            .find(|(key, _)| *key == "XDG_DATA_DIRS")
            .map(|(_, value)| value)
            .unwrap_or_else(|| std::env::var_os("XDG_DATA_DIRS"))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        data_home
            .into_iter()
            .chain(std::env::split_paths(&dirs))
            .find_map(|path| {
                std::fs::read_to_string(path.join("applications").join(id))
                    .ok()
                    .and_then(|entry| desktop_entry_name(&entry, _locale))
            })
    }
}

#[cfg(any(test, not(any(target_os = "macos", target_os = "windows"))))]
fn desktop_entry_name(entry: &str, locale: &str) -> Option<String> {
    let locale = locale.replace('-', "_");
    let language = locale.split('_').next().unwrap_or(&locale);
    let mut in_entry = false;
    let mut default = None;
    let mut localized = None;
    for line in entry.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            let Some((key, name)) = line.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            if key == "Name" {
                default = Some(name);
            } else if key == format!("Name[{locale}]") {
                return Some(name.to_owned());
            } else if key == format!("Name[{language}]") {
                localized = Some(name);
            }
        }
    }
    localized.or(default).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_name_uses_the_application_section() {
        assert_eq!(
            desktop_entry_name(
                "[Other]\nName=Wrong\n[Desktop Entry]\nName=Files\n[Desktop Action New]\nName=New",
                "en"
            ),
            Some("Files".into())
        );
        assert_eq!(desktop_entry_name("[Desktop Entry]\nName=\n", "en"), None);
        let entry =
            "[Desktop Entry]\nName=Files\nName[zh]=文件\nName[zh_CN]=文件管理器\nName[es]=Archivos";
        assert_eq!(
            desktop_entry_name(entry, "zh-CN"),
            Some("文件管理器".into())
        );
        assert_eq!(desktop_entry_name(entry, "zh-TW"), Some("文件".into()));
        assert_eq!(desktop_entry_name(entry, "es"), Some("Archivos".into()));
        assert_eq!(desktop_entry_name(entry, "ja"), Some("Files".into()));
    }

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
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn reveal_command_opens_directory_itself() {
        let dir = std::env::temp_dir();
        let cmd = reveal_command(&dir);
        assert_eq!(args(&cmd), [dir.to_string_lossy().into_owned()]);
    }
}
