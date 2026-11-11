//! Keeps this machine from sleeping while a local run's payload runs. The operating system
//! drops the prevention when the wrapper exits, however it exits.

/// Held until dropped. Failing to take it is logged and never fails the run.
pub(crate) fn hold(run: &str) -> Option<os::Held> {
    match os::hold(run) {
        Ok(held) => Some(held),
        Err(error) => {
            eprintln!("orx-host: could not keep this machine awake for run {run}: {error}");
            None
        }
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::ffi::{c_char, c_void, CString};
    use std::io;

    type CFStringRef = *const c_void;
    const UTF8: u32 = 0x0800_0100;
    const ASSERTION_ON: u32 = 255;

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithCString(
            allocator: *const c_void,
            string: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFRelease(object: *const c_void);
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPMAssertionCreateWithName(
            kind: CFStringRef,
            level: u32,
            name: CFStringRef,
            id: *mut u32,
        ) -> i32;
        fn IOPMAssertionRelease(id: u32) -> i32;
    }

    pub(crate) struct Held(u32);

    impl Drop for Held {
        fn drop(&mut self) {
            // SAFETY: the id came from a successful IOPMAssertionCreateWithName.
            unsafe { IOPMAssertionRelease(self.0) };
        }
    }

    struct CfString(CFStringRef);

    impl CfString {
        fn new(text: &str) -> io::Result<Self> {
            let text = CString::new(text).map_err(io::Error::other)?;
            // SAFETY: a NUL-terminated UTF-8 string that outlives the call.
            let string =
                unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) };
            if string.is_null() {
                return Err(io::Error::other("CFStringCreateWithCString failed"));
            }
            Ok(Self(string))
        }
    }

    impl Drop for CfString {
        fn drop(&mut self) {
            // SAFETY: created above and released once.
            unsafe { CFRelease(self.0) };
        }
    }

    /// Idle sleep only, as Chromium's app-suspension blocker takes: closing the lid still sleeps.
    pub(super) fn hold(run: &str) -> io::Result<Held> {
        let name = CfString::new(&format!("OpenResearch experiment run {run}"))?;
        let kind = CfString::new("PreventUserIdleSystemSleep")?;
        let mut id = 0;
        // SAFETY: both strings are live CFStrings and `id` is writable.
        let status = unsafe { IOPMAssertionCreateWithName(kind.0, ASSERTION_ON, name.0, &mut id) };
        if status != 0 {
            return Err(io::Error::other(format!(
                "IOPMAssertionCreateWithName returned {status:#x}"
            )));
        }
        Ok(Held(id))
    }
}

#[cfg(not(target_os = "macos"))]
mod os {
    use std::io;
    use std::process::{ChildStdin, Command, Stdio};

    /// logind holds the inhibitor while `systemd-inhibit` runs, and `cat` runs until this end
    /// of its stdin closes.
    pub(crate) type Held = ChildStdin;

    /// Sleep only, never `handle-lid-switch`: logind's default `LidSwitchIgnoreInhibited=yes`
    /// still suspends on a closed lid. The inhibitor is orphaned before the wrapper becomes a
    /// subreaper, so it never counts among the run's children.
    const INHIBIT: &str = r#"exec 3<&0
{ systemd-inhibit --what=sleep --who=OpenResearch --why="$1" cat <&3 >/dev/null 3<&- ||
  echo "orx-host: could not keep this machine awake for run $2: systemd-inhibit exited with $?" >&2; } &"#;

    pub(super) fn hold(run: &str) -> io::Result<Held> {
        let mut shell = Command::new("/bin/sh")
            .args(["-c", INHIBIT, "orx-host"])
            .arg(format!("Experiment run {run}"))
            .arg(run)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?;
        let stdin = shell.stdin.take();
        let status = shell.wait()?;
        match stdin {
            Some(stdin) if status.success() => Ok(stdin),
            _ => Err(io::Error::other(format!(
                "could not start systemd-inhibit: {status}"
            ))),
        }
    }
}
