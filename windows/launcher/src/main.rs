//! `OpenResearch.exe`, the Windows app's entry point. A GUI program gets no
//! console, and every console program it started would open a window of its
//! own. So it starts the `orx.exe` beside it as `orx app` in a console no one
//! sees, which `orx` and the git, shell, and agent processes it runs all share.
//! An `openresearch:` link it was opened with (see OpenResearch.iss) goes along.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe.with_file_name("orx.exe"))
            // APP_ARG in src/commands/app.rs.
            .arg("app")
            .args(link_arg(std::env::args_os().skip(1)))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    });
    match started {
        // Explorer lets the program it started take the foreground; pass that on,
        // so orx's window, or the running one it wakes, comes to the front.
        // SAFETY: a plain syscall on the child's process id.
        Ok(child) => unsafe {
            AllowSetForegroundWindow(child.id());
        },
        Err(error) => show_error(&format!(
            "Could not start orx.exe: {error}\n\nReinstall OpenResearch to repair it."
        )),
    }
}

/// The first argument, if it is an `openresearch:` link: the only one orx's app takes.
#[cfg_attr(not(windows), allow(dead_code))]
fn link_arg(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<std::ffi::OsString> {
    // LINK_SCHEME in src/commands/app.rs.
    const SCHEME: &str = "openresearch:";
    args.next().filter(|arg| {
        arg.to_str()
            .and_then(|arg| arg.get(..SCHEME.len()))
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
    })
}

#[cfg(windows)]
fn show_error(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (body, title) = (wide(message), wide("OpenResearch could not start"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}

#[cfg(not(windows))]
fn main() {
    eprintln!("OpenResearch.exe is the Windows app's launcher");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::link_arg;
    use std::ffi::OsString;

    fn link(args: &[&str]) -> Option<OsString> {
        link_arg(args.iter().map(OsString::from))
    }

    #[test]
    fn only_an_openresearch_link_is_passed_on() {
        let url = "openresearch://reproduce/1706.03762";
        assert_eq!(link(&[url]), Some(url.into()));
        assert_eq!(link(&["OPENRESEARCH://x"]), Some("OPENRESEARCH://x".into()));
        assert_eq!(link(&[]), None);
        assert_eq!(link(&["--help"]), None);
        assert_eq!(link(&["https://example.com"]), None);
    }
}
