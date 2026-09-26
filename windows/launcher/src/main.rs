//! `OpenResearch.exe`, the Windows app's entry point. A GUI program gets no
//! console, and every console program it started would open a window of its
//! own. So it starts the `orx.exe` beside it as `orx app` in a console no one
//! sees, which `orx` and the git, shell, and agent processes it runs all share.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

    let started = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe.with_file_name("orx.exe"))
            // APP_ARG in src/commands/app.rs.
            .arg("app")
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
