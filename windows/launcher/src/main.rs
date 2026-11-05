//! `OpenResearch.exe`, the Windows app's entry point. A GUI program gets no
//! console, and every console program it started would open a window of its
//! own. So it starts the `orx.exe` beside it as `orx app` in a console no one
//! sees, which `orx` and the git, shell, and agent processes it runs all share.
//! An `openresearch:` link or `.orx` file it was opened with (see OpenResearch.iss) goes along.

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]
// Its own crate, so `crate::process` is unreachable; it starts `orx.exe` by absolute path.
#![allow(clippy::disallowed_methods)]

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

/// The first argument, if it is an `openresearch:` link or a `.orx` file: the only ones orx's app takes.
#[cfg_attr(not(windows), allow(dead_code))]
fn link_arg(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<std::ffi::OsString> {
    // LINK_SCHEME in src/commands/app.rs and EXTENSION in src/local/share.rs.
    const SCHEME: &str = "openresearch:";
    const SHARED: &str = ".orx";
    args.next().filter(|arg| {
        arg.to_str().is_some_and(|arg| {
            let scheme = arg.get(..SCHEME.len());
            let extension = arg
                .len()
                .checked_sub(SHARED.len())
                .and_then(|at| arg.get(at..));
            scheme.is_some_and(|scheme| scheme.eq_ignore_ascii_case(SCHEME))
                || extension.is_some_and(|extension| extension.eq_ignore_ascii_case(SHARED))
        })
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
        let shared = r"C:\Users\me\Downloads\tiny lm.ORX";
        assert_eq!(link(&[shared]), Some(shared.into()));
        assert_eq!(link(&[r"C:\notes.txt"]), None);
    }
}
