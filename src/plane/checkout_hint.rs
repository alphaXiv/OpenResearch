enum ShellKind {
    Posix,
    PowerShell,
    Cmd,
}

impl ShellKind {
    fn from_name(name: &str) -> Option<Self> {
        let name = name.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
        match name.trim_start_matches('-').trim_end_matches(".exe") {
            "sh" | "bash" | "dash" | "zsh" | "ksh" | "fish" => Some(Self::Posix),
            "powershell" | "pwsh" => Some(Self::PowerShell),
            "cmd" => Some(Self::Cmd),
            _ => None,
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Posix => "Bash / POSIX shell",
            Self::PowerShell => "PowerShell",
            Self::Cmd => "Command Prompt",
        }
    }

    fn checkout(&self, path: &str, branch: &str) -> String {
        match self {
            Self::Posix => format!(
                "cd {} && git checkout {}",
                crate::jobs::ssh::sh_quote(path),
                crate::jobs::ssh::sh_quote(branch)
            ),
            Self::PowerShell => format!(
                "Set-Location -LiteralPath '{}' -ErrorAction Stop; if ($?) {{ git checkout '{}' }}",
                path.replace('\'', "''"),
                branch.replace('\'', "''")
            ),
            Self::Cmd => format!("cd /d \"{path}\" && git checkout \"{branch}\""),
        }
    }
}

pub(super) fn print(path: &str, branch: &str) {
    let shell = parent_shell().unwrap_or_else(|| {
        if cfg!(windows) {
            ShellKind::Cmd
        } else {
            std::env::var("SHELL")
                .ok()
                .and_then(|name| ShellKind::from_name(&name))
                .unwrap_or(ShellKind::Posix)
        }
    });
    println!(
        "To edit it, check out the branch in the project's local clone ({}):",
        shell.name()
    );
    println!("  {}", shell.checkout(path, branch));
    println!("After editing, commit your changes:");
    println!("  git commit -am \"<msg>\"");
}

#[cfg(unix)]
fn parent_shell() -> Option<ShellKind> {
    let output = std::process::Command::new("ps")
        // SAFETY: getppid has no preconditions.
        .args(["-p", &unsafe { libc::getppid() }.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    ShellKind::from_name(String::from_utf8_lossy(&output.stdout).trim())
}

#[cfg(windows)]
fn parent_shell() -> Option<ShellKind> {
    use std::collections::HashMap;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot is read-only and its new handle is owned below.
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return None;
    }
    // SAFETY: the valid snapshot handle is transferred exactly once.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = HashMap::new();
    // SAFETY: entry has the required size and remains writable during enumeration.
    let mut found = unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) };
    while found != 0 {
        let end = entry.szExeFile.iter().position(|&c| c == 0)?;
        processes.insert(
            entry.th32ProcessID,
            (
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..end]),
            ),
        );
        // SAFETY: the same initialized entry and live snapshot are reused.
        found = unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) };
    }
    let mut pid = processes.get(&std::process::id())?.0;
    while let Some((parent, name)) = processes.remove(&pid) {
        if let Some(shell) = ShellKind::from_name(&name) {
            return Some(shell);
        }
        pid = parent;
    }
    None
}
