//! Desktop app mode — the GUI entry point for the downloadable OpenResearch app.
//!
//! On macOS the `.app` bundle's executable IS the `orx` binary; launched from
//! Finder with no arguments, `main` routes here instead of parsing CLI args. On
//! Windows the installed `OpenResearch.exe` launcher (windows/launcher) starts
//! the `orx.exe` beside it as `orx app`, in a hidden console its children share;
//! on Linux the AppImage's AppRun (linux/AppRun) does the same. Either way app
//! mode owns the main thread with the window's run loop, while the `orx up`
//! dashboard server runs on background tokio worker threads.
//!
//! This is distinct from `orx up` launched in a terminal, which stays a plain
//! CLI. The GUI parts build only under `cfg(desktop_app)`: macOS, Windows, and
//! Linux with the `desktop` feature (see build.rs).

/// True when `exe` is a `<name>.app/Contents/MacOS` bundle executable that was
/// invoked under its own name — the signal to enter GUI app mode instead of
/// parsing CLI args.
///
/// The name check is what keeps the bundle's `orx` symlink (see
/// `build-macos-app.sh`) a plain CLI: an agent shelling out to a bare `orx`
/// must print help, not open a second dashboard. That relies on `exe` being
/// canonicalized — it is the *symlink* whose name differs, so an uncanonicalized
/// path would compare `orx` against `orx` and match.
// Un-gated so its tests run on CI's Linux runner; only macOS has a caller.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn is_bundle_exe_launch(exe: &std::path::Path, argv0: Option<&std::ffi::OsStr>) -> bool {
    let in_bundle = exe
        .parent()
        .is_some_and(|dir| dir.ends_with("Contents/MacOS"));
    let invoked_as_bundle_exe = argv0
        .map(std::path::Path::new)
        .and_then(std::path::Path::file_name)
        == exe.file_name();
    in_bundle && invoked_as_bundle_exe
}

/// Whether to enter GUI app mode. macOS `current_exe` reports the path the
/// process was *launched as*, symlink and all, so it is canonicalized first.
#[cfg(target_os = "macos")]
pub fn launched_as_app_bundle() -> bool {
    let Ok(exe) = std::env::current_exe().and_then(crate::paths::canonicalize) else {
        return false;
    };
    is_bundle_exe_launch(&exe, std::env::args_os().next().as_deref())
}

/// The argument the Windows launcher and the Linux AppImage's AppRun start `orx`
/// with, followed by nothing or the `openresearch:` link that opened the app.
#[cfg_attr(not(all(desktop_app, not(target_os = "macos"))), allow(dead_code))]
pub const APP_ARG: &str = "app";

// Also in windows/launcher.
const LINK_SCHEME: &str = "openresearch:";

/// Carries `--no-telemetry` into `orx app`, whose argument list must stay [`APP_ARG`] and at most a link.
#[cfg(all(desktop_app, not(target_os = "macos")))]
pub const APP_NO_TELEMETRY_ENV: &str = "ORX_APP_NO_TELEMETRY";

#[cfg(all(desktop_app, not(target_os = "macos")))]
pub fn launched_with_app_arg() -> bool {
    app_launch(std::env::args_os().skip(1)).is_some()
}

/// The `openresearch:` link `orx app` was started with.
#[cfg(all(desktop_app, not(target_os = "macos")))]
fn launch_link() -> Option<String> {
    app_launch(std::env::args_os().skip(1)).flatten()
}

/// `None` unless `args` are `app` or `app <openresearch: link>`; then `Some` of the link, if any.
#[cfg_attr(not(all(desktop_app, not(target_os = "macos"))), allow(dead_code))]
fn app_launch(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<Option<String>> {
    if args.next()? != APP_ARG {
        return None;
    }
    let link = match args.next() {
        None => None,
        Some(arg) => Some(arg.into_string().ok().filter(|arg| is_app_link(arg))?),
    };
    args.next().is_none().then_some(link)
}

/// Whether `arg` is an `openresearch:` link, the only argument the app takes.
#[cfg_attr(not(all(desktop_app, not(target_os = "macos"))), allow(dead_code))]
fn is_app_link(arg: &str) -> bool {
    arg.get(..LINK_SCHEME.len())
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case(LINK_SCHEME))
}

/// The dock and app grid name and icon a window only through a desktop entry.
/// Rewritten whenever it points elsewhere; `TryExec` hides it once the file goes.
#[cfg(all(desktop_app, target_os = "linux"))]
fn install_desktop_entry() {
    let (Some(appimage), Some(data)) = (crate::updates::running_appimage(), dirs::data_dir())
    else {
        return;
    };
    let icon = data.join("icons/hicolor/256x256/apps/openresearch.png");
    let (Some(appimage), Some(icon_path)) = (appimage.to_str(), icon.to_str()) else {
        return;
    };
    let entry = desktop_entry(appimage, icon_path);
    let entry_changed = || -> std::io::Result<bool> {
        write_if_changed(&icon, include_bytes!("../../linux/OpenResearch.png"))?;
        write_if_changed(
            &data.join("applications/openresearch.desktop"),
            entry.as_bytes(),
        )
    };
    match entry_changed() {
        // The entry's MimeType offers it; this makes it the handler for openresearch: links.
        Ok(true) => {
            let mut handler = std::process::Command::new("xdg-mime");
            handler.args([
                "default",
                "openresearch.desktop",
                "x-scheme-handler/openresearch",
            ]);
            crate::local::shell_env::restore_host_gui_env(&mut handler);
            std::thread::spawn(move || match handler.output() {
                Ok(out) if out.status.success() => {}
                Ok(out) => eprintln!(
                    "openresearch app: could not register openresearch: links: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                Err(err) => {
                    eprintln!("openresearch app: could not register openresearch: links: {err}")
                }
            });
        }
        Ok(false) => {}
        Err(err) => {
            eprintln!("openresearch app: could not add OpenResearch to your applications: {err}")
        }
    }
}

/// Whether it wrote the file.
#[cfg(all(desktop_app, target_os = "linux"))]
fn write_if_changed(path: &std::path::Path, contents: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|current| current == contents) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, contents)?;
    Ok(true)
}

/// linux/OpenResearch.desktop, launching the AppImage at `appimage` with the icon at `icon`.
#[cfg_attr(not(all(desktop_app, target_os = "linux")), allow(dead_code))]
fn desktop_entry(appimage: &str, icon: &str) -> String {
    // Every value is a desktop-entry string, which escapes backslashes; Exec's
    // quoted argument escapes these four first, and doubles `%`.
    let string = |value: &str| value.replace('\\', "\\\\");
    let mut arg = String::new();
    for c in appimage.chars() {
        match c {
            '"' | '`' | '$' | '\\' => arg.extend(['\\', c]),
            '%' => arg.push_str("%%"),
            c => arg.push(c),
        }
    }
    include_str!("../../linux/OpenResearch.desktop")
        .lines()
        .map(|line| match line.split_once('=') {
            Some(("Exec", _)) => {
                format!(
                    "Exec=\"{}\" %u\nTryExec={}\n",
                    string(&arg),
                    string(appimage)
                )
            }
            Some(("Icon", _)) => format!("Icon={}\n", string(icon)),
            _ => format!("{line}\n"),
        })
        .collect()
}

#[cfg(desktop_app)]
const APP_PORT: u16 = 4792;

/// Enter GUI app mode: pick a port, start the dashboard server on background
/// threads, and hand the main thread to the window's run loop. Returns only if
/// setup fails; quitting exits the process.
#[cfg(desktop_app)]
pub async fn run() {
    #[cfg(not(target_os = "macos"))]
    let (focus_requests, link) = {
        // A relaunched Windows app first waits out its predecessor, which holds
        // the claim (Linux relaunches by exec, which releases it).
        crate::updates::await_replaced_parent();
        let link = launch_link();
        match instance::claim(link.as_deref()) {
            Some(requests) => (requests, link),
            None => return,
        }
    };
    // A launcher's environment lacks .bashrc's PATH. Before storage and telemetry,
    // which read the directories it may change.
    #[cfg(target_os = "linux")]
    hydrate_shell_env().await;
    #[cfg(target_os = "linux")]
    install_desktop_entry();
    // After the claim, so a launch that only brings the window forward isn't a
    // start. The durable outbox covers a quit before delivery.
    let _telemetry = crate::telemetry::TelemetrySession::start_app();
    if let Err(error) = crate::local::storage::prepare().await {
        eprintln!("OpenResearch storage: {error}");
        crate::show_error_dialog(&error.to_string());
        return;
    }
    // App mode returns before `dispatch`, which is where `orx up` takes this
    // same read lock. Without it `orx delete` from a CLI install sees no reader
    // and wipes the store out from under a running app.
    let lifecycle = match crate::store::open_lifecycle_lock() {
        Ok(lock) => lock,
        Err(error) => {
            crate::show_error_dialog(&format!("Could not open the storage lock: {error}"));
            return;
        }
    };
    let _lifecycle_guard = match lifecycle.read() {
        Ok(guard) => guard,
        Err(error) => {
            crate::show_error_dialog(&format!("Could not hold the storage lock: {error}"));
            return;
        }
    };
    // After an update relaunch, keep the previous port. Otherwise a fixed port,
    // since the window's localStorage is keyed by origin, falling back to an
    // ephemeral one when a terminal `orx up` or another app holds it.
    let port_is_free = |port: &u16| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok();
    let port = std::env::var(crate::updates::APP_RELAUNCH_PORT_ENV)
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(port_is_free)
        .or_else(|| Some(APP_PORT).filter(port_is_free))
        .unwrap_or_else(|| {
            std::net::TcpListener::bind(("127.0.0.1", 0))
                .and_then(|l| l.local_addr())
                .map(|a| a.port())
                .unwrap_or(4791)
        });
    #[cfg(target_os = "macos")]
    imp::run_event_loop(port, false);
    #[cfg(not(target_os = "macos"))]
    imp::run_event_loop(
        port,
        false,
        focus_requests,
        link.as_deref().and_then(link_path),
    );
}

#[cfg(all(target_os = "macos", debug_assertions))]
pub fn run_dev(port: u16) {
    imp::run_event_loop(port, true);
}

/// Adopt the user's shell environment in place of the one launchd or the desktop
/// session handed us (see [`crate::local::shell_env`]).
///
/// `-ilc`, not `-lc`: zsh reads `.zshrc` only for *interactive* shells, and
/// that is where these exports overwhelmingly live. The inner `sh -c` keeps the
/// answer portable — the outer shell execs `/bin/sh`, which prints the values it
/// inherited, where fish would have printed its own list-valued `$PATH`
/// space-separated. NUL separates them because a PATH or a directory may
/// contain spaces, colons, and newlines, but never NUL.
#[cfg(all(desktop_app, unix))]
pub(crate) async fn hydrate_shell_env() {
    // Nonce, so rc-file chatter can't forge the fence around the values. The
    // leading `_` is load-bearing: `printf` reads `\0` plus up to three octal
    // digits, so a marker starting with a digit would be eaten by the escape.
    let marker = format!("__ORX_ENV_{}__", uuid::Uuid::new_v4().simple());
    let fallback = if cfg!(target_os = "macos") {
        "/bin/zsh"
    } else {
        "/bin/sh"
    };
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| fallback.into());
    let reads = crate::local::shell_env::IMPORTED
        .map(|key| format!(r#""${key}""#))
        .join(" ");
    let template = "%s\\0".repeat(crate::local::shell_env::IMPORTED.len());
    let script = format!(r#"/bin/sh -c 'printf "{marker}{template}{marker}" {reads}'"#);
    let mut probe = tokio::process::Command::new(&shell);
    probe
        .args(["-ilc".to_string(), script])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // Anything the rc files start is a host program.
    crate::local::shell_env::restore_host_gui_env(probe.as_std_mut());
    // Past the startup wait below, a late answer still supplies PATH; this bounds a hung rc file.
    let mut answer = tokio::spawn(tokio::time::timeout(
        std::time::Duration::from_secs(60),
        probe.output(),
    ));
    // A slow rc file (nvm, conda) delays the dashboard, so cap the startup wait.
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut answer).await {
        Ok(out) => adopt_probe(out, &shell, &marker, false),
        Err(_) => {
            eprintln!(
                "openresearch app: {shell:?} did not answer within 5s; using the inherited \
                 environment (a later answer supplies PATH)"
            );
            tokio::spawn(async move { adopt_probe(answer.await, &shell, &marker, true) });
        }
    }
}

#[cfg(all(desktop_app, unix))]
fn adopt_probe(
    out: Result<
        Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed>,
        tokio::task::JoinError,
    >,
    shell: &std::ffi::OsStr,
    marker: &str,
    late: bool,
) {
    let out = match out {
        Ok(Ok(Ok(out))) => out,
        Ok(Ok(Err(err))) => {
            eprintln!(
                "openresearch app: could not run {shell:?}: {err}; using the inherited environment"
            );
            return;
        }
        Ok(Err(_)) => {
            eprintln!(
                "openresearch app: {shell:?} did not answer within 60s; using the inherited \
                 environment"
            );
            return;
        }
        Err(err) => {
            eprintln!(
                "openresearch app: probing {shell:?} failed: {err}; using the inherited \
                 environment"
            );
            return;
        }
    };
    // The markers are the success signal, not the exit status — an interactive
    // rc file routinely ends on a failing command.
    match crate::local::shell_env::parse_probe(&String::from_utf8_lossy(&out.stdout), marker) {
        Some(mut vars) => {
            // The other imports pick directories already in use; switching them mid-session
            // would split state between the old and new directories.
            if late {
                vars.retain(|key, _| *key == "PATH");
            }
            let adopted: Vec<String> = crate::local::shell_env::IMPORTED
                .iter()
                .filter_map(|key| Some(format!("{key}={:?}", vars.get(key)?)))
                .collect();
            eprintln!(
                "openresearch app: adopted the shell environment{}: {}",
                if late { " late (PATH only)" } else { "" },
                adopted.join(" ")
            );
            crate::local::shell_env::set(vars);
        }
        None => eprintln!(
            "openresearch app: the environment probe returned nothing usable; using the inherited \
             environment. shell stderr: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    }
}

/// True when `url` is a page of the dashboard at `origin`, as opposed to the
/// `about:blank` the window holds before the server is up and while quitting.
// Un-gated so its tests run on CI's Linux runner; only the desktop targets call it.
#[cfg_attr(not(desktop_app), allow(dead_code))]
fn is_dashboard_url(url: &str, origin: &str) -> bool {
    url.strip_prefix(origin)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Schemes a pop-up may hand to the system browser. A browser asks before
/// launching the app behind any other scheme (`ssh:`, `vscode:`); `open` would not.
#[cfg_attr(not(desktop_app), allow(dead_code))]
fn opens_in_browser(url: &str) -> bool {
    ["http:", "https:", "mailto:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

/// The dashboard path an `openresearch://reproduce/<paper id>` link opens; the page validates the id.
#[cfg_attr(not(desktop_app), allow(dead_code))]
fn deep_link_path(url: &url::Url) -> Option<String> {
    let path = url.path();
    if url.scheme() != "openresearch" || url.host_str() != Some("reproduce") || path.len() < 2 {
        return None;
    }
    Some(format!("/reproduce{path}"))
}

/// [`deep_link_path`] for a link that arrived as text, on the command line or from a second launch.
#[cfg_attr(not(all(desktop_app, not(target_os = "macos"))), allow(dead_code))]
fn link_path(link: &str) -> Option<String> {
    deep_link_path(&url::Url::parse(link).ok()?)
}

/// The link a second Windows launch leaves for the running app, whose focus
/// event has no room for one. Written before the event is set.
#[cfg(any(windows, test))]
mod link_file {
    use std::path::Path;

    const NAME: &str = "app-link";

    pub(super) fn leave(dir: &Path, link: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        // Renamed into place, so the app never reads half a link.
        let staged = dir.join(format!("{NAME}.{}", std::process::id()));
        std::fs::write(&staged, link)?;
        std::fs::rename(staged, dir.join(NAME))
    }

    pub(super) fn take(dir: &Path) -> Option<String> {
        // Renamed away first, so a link left meanwhile waits for its own event.
        let taken = dir.join(format!("{NAME}.taken"));
        std::fs::rename(dir.join(NAME), &taken).ok()?;
        let link = std::fs::read_to_string(&taken).ok();
        let _ = std::fs::remove_file(&taken);
        link.filter(|link| super::is_app_link(link))
    }

    #[cfg(test)]
    mod tests {
        use super::{leave, take};

        #[test]
        fn the_running_app_takes_the_link_a_second_launch_left_once() {
            let dir = std::env::temp_dir().join(format!("orx-link-{}", uuid::Uuid::new_v4()));
            assert_eq!(take(&dir), None);
            leave(&dir, "openresearch://reproduce/1").unwrap();
            leave(&dir, "openresearch://reproduce/2").unwrap();
            assert_eq!(take(&dir).as_deref(), Some("openresearch://reproduce/2"));
            assert_eq!(take(&dir), None);
            leave(&dir, "https://example.com").unwrap();
            assert_eq!(take(&dir), None);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// One app per user session on Windows, where nothing else enforces it: a
/// second launch brings the running window forward and exits.
#[cfg(windows)]
mod instance {
    use std::path::PathBuf;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    // Also the installer's AppMutex (windows/OpenResearch.iss).
    const MUTEX: &str = r"Local\OpenResearchApp";
    const FOCUS_EVENT: &str = r"Local\OpenResearchAppFocus";

    /// Signalled by each later launch, which may first leave a link in the directory.
    pub(super) struct FocusRequests {
        event: HANDLE,
        link_dir: Option<PathBuf>,
    }

    // SAFETY: an event handle may be waited on from any thread.
    unsafe impl Send for FocusRequests {}

    /// Claims the app for this process. `None` means another instance has it
    /// and was asked to come forward, opening `link`.
    pub(super) fn claim(link: Option<&str>) -> Option<FocusRequests> {
        let link_dir = dirs::data_local_dir().map(|dir| dir.join("OpenResearch"));
        // SAFETY: plain syscalls on NUL-terminated names. The handles stay open
        // on purpose: the claim lasts until the process exits.
        // Bound before the calls, so no drop runs between CreateMutexW and GetLastError.
        let (event_name, mutex_name) = (super::wide(FOCUS_EVENT), super::wide(MUTEX));
        let (event, running) = unsafe {
            // The event first, so it exists by the time anyone sees the claim.
            // For a second launch this opens the running app's event.
            let event = CreateEventW(std::ptr::null(), 0, 0, event_name.as_ptr());
            CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr());
            (event, GetLastError() == ERROR_ALREADY_EXISTS)
        };
        if running {
            if let (Some(link), Some(dir)) = (link, &link_dir) {
                if let Err(error) = super::link_file::leave(dir, link) {
                    eprintln!("openresearch app: could not pass the link on: {error}");
                }
            }
            // SAFETY: plain syscalls on the event opened above.
            unsafe {
                // Windows lets the running app take the foreground only with our leave.
                AllowSetForegroundWindow(ASFW_ANY);
                SetEvent(event);
                CloseHandle(event);
            }
            return None;
        }
        // Left for an app that quit before it read it.
        if let Some(dir) = &link_dir {
            let _ = super::link_file::take(dir);
        }
        Some(FocusRequests { event, link_dir })
    }

    impl FocusRequests {
        /// Calls `on_request` with its link for every later launch, from a thread of its own.
        pub(super) fn listen(self, on_request: impl Fn(Option<String>) + Send + 'static) {
            std::thread::spawn(move || {
                while self.wait() {
                    on_request(self.link_dir.as_deref().and_then(super::link_file::take));
                }
            });
        }

        // A method, so the thread above captures the `Send` wrapper and not
        // its bare handle.
        fn wait(&self) -> bool {
            // SAFETY: waits on the event this process created and never closes.
            unsafe { WaitForSingleObject(self.event, INFINITE) == WAIT_OBJECT_0 }
        }
    }
}

/// One app per user session on Linux, where nothing else enforces it either: a
/// second launch connects to the running app's socket, which brings its window
/// forward, and exits.
#[cfg(all(desktop_app, target_os = "linux"))]
mod instance {
    use std::fs::{File, OpenOptions, TryLockError};
    use std::io::{Read, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    /// `None` when the socket couldn't be bound: the app still runs, just
    /// without the one-instance guard.
    pub(super) struct FocusRequests(Option<UnixListener>);

    /// Where the lock and socket live, and their name. Private to the user: in a
    /// shared directory another user could hold the lock or create the socket first.
    fn claim_paths() -> Option<(PathBuf, String)> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute() && is_private(dir));
        if let Some(dir) = runtime {
            return Some((dir, "openresearch-app".to_string()));
        }
        // A home on NFS is shared across machines, and so would be a lock in it.
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").ok()?;
        let dir = dirs::cache_dir()?.join("openresearch");
        Some((dir, format!("openresearch-app-{}", host.trim())))
    }

    /// Owned by this user and writable by no one else, as the XDG spec requires
    /// of `XDG_RUNTIME_DIR` and some containers ignore.
    fn is_private(dir: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: getuid has no failure mode.
        let uid = unsafe { libc::getuid() };
        std::fs::metadata(dir).is_ok_and(|meta| meta.uid() == uid && meta.mode() & 0o022 == 0)
    }

    /// Claims the app for this process. `None` means another instance has it
    /// and was asked to come forward, opening `link`.
    pub(super) fn claim(link: Option<&str>) -> Option<FocusRequests> {
        let Some((dir, name)) = claim_paths() else {
            eprintln!(
                "openresearch app: found no private directory for the app lock; a second \
                 launch will open another window"
            );
            return Some(FocusRequests(None));
        };
        claim_at(&dir, &name, Duration::from_secs(5), link)
    }

    fn claim_at(
        dir: &Path,
        name: &str,
        wait: Duration,
        link: Option<&str>,
    ) -> Option<FocusRequests> {
        let socket = dir.join(format!("{name}.sock"));
        let lock_path = dir.join(format!("{name}.lock"));
        let lock = std::fs::create_dir_all(dir)
            .and_then(|()| {
                OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(&lock_path)
            })
            .and_then(|file| take_lock(file, &socket, wait, link));
        let lock = match lock {
            Ok(Some(file)) => Some(file),
            Ok(None) => return None,
            Err(error) => {
                eprintln!(
                    "openresearch app: could not lock {}: {error}; two launches at once may \
                     both open a window",
                    lock_path.display()
                );
                // Unguarded, so only the socket can say whether an app is running.
                if hand_over(&socket, link) {
                    return None;
                }
                None
            }
        };
        // The lock (or, unguarded, the failed connect) says no app is listening, so a
        // file there is left from one that didn't exit cleanly or exec'd into an update.
        let _ = std::fs::remove_file(&socket);
        match UnixListener::bind(&socket) {
            Ok(listener) => {
                // Held until exit, or the exec into an update: files are close-on-exec.
                std::mem::forget(lock);
                Some(FocusRequests(Some(listener)))
            }
            // The lock drops here, so a later launch opens a window rather than
            // waiting on a socket that will never answer.
            Err(error) => {
                eprintln!(
                    "openresearch app: could not bind {}: {error}; a second launch will open \
                     another window",
                    socket.display()
                );
                Some(FocusRequests(None))
            }
        }
    }

    /// The lock, or `None` once the app holding it was asked to come forward (or
    /// never answered). Retries both: when two launch together, the winner may not
    /// have bound its socket yet, or may give the lock up because it can't.
    fn take_lock(
        file: File,
        socket: &Path,
        wait: Duration,
        link: Option<&str>,
    ) -> std::io::Result<Option<File>> {
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(file)),
                Err(TryLockError::Error(error)) => return Err(error),
                Err(TryLockError::WouldBlock) => {}
            }
            if hand_over(socket, link) {
                return Ok(None);
            }
            if Instant::now() >= deadline {
                eprintln!(
                    "openresearch app: OpenResearch is already running but did not answer at {}",
                    socket.display()
                );
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// The running app takes the connection itself as the request, and what is
    /// written on it as the link to open.
    fn hand_over(socket: &Path, link: Option<&str>) -> bool {
        let Ok(mut stream) = UnixStream::connect(socket) else {
            return false;
        };
        if let Some(link) = link {
            // An app from before links closes without reading; it still comes forward.
            let _ = stream.write_all(link.as_bytes());
        }
        true
    }

    fn read_link(stream: UnixStream) -> Option<String> {
        // A launch that never closes its end must not hold up the next one.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut link = String::new();
        stream.take(8192).read_to_string(&mut link).ok()?;
        Some(link).filter(|link| super::is_app_link(link))
    }

    impl FocusRequests {
        /// Calls `on_request` with its link for every later launch, from a thread of its own.
        pub(super) fn listen(self, on_request: impl Fn(Option<String>) + Send + 'static) {
            let Some(listener) = self.0 else {
                return;
            };
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    on_request(read_link(stream));
                }
            });
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{claim_at, is_private};
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};
        use std::sync::mpsc;
        use std::time::Duration;

        fn private_dir() -> PathBuf {
            let dir = std::env::temp_dir().join(format!("orx-instance-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&dir).unwrap();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
            dir
        }

        #[test]
        fn a_second_launch_defers_to_the_running_app() {
            let dir = private_dir();
            let first =
                claim_at(&dir, "app", Duration::ZERO, None).expect("the first launch is the app");
            assert!(first.0.is_some());
            assert!(claim_at(&dir, "app", Duration::from_secs(1), None).is_none());
        }

        #[test]
        fn a_second_launch_hands_its_link_to_the_running_app() {
            let dir = private_dir();
            let (sent, requests) = mpsc::channel();
            claim_at(&dir, "app", Duration::ZERO, None)
                .expect("the first launch is the app")
                .listen(move |link| sent.send(link).unwrap());
            let link = "openresearch://reproduce/1706.03762";
            assert!(claim_at(&dir, "app", Duration::from_secs(1), Some(link)).is_none());
            assert!(claim_at(&dir, "app", Duration::from_secs(1), None).is_none());
            assert!(claim_at(&dir, "app", Duration::from_secs(1), Some("https://x.y")).is_none());
            let wait = Duration::from_secs(5);
            assert_eq!(requests.recv_timeout(wait).unwrap().as_deref(), Some(link));
            assert_eq!(requests.recv_timeout(wait).unwrap(), None);
            assert_eq!(requests.recv_timeout(wait).unwrap(), None);
        }

        #[test]
        fn an_app_that_cannot_bind_leaves_the_next_launch_free() {
            let dir = private_dir();
            // Past sun_path's 108 bytes, so the bind fails after the lock is taken.
            let name = "x".repeat(120);
            let first = claim_at(&dir, &name, Duration::ZERO, None).expect("still runs");
            assert!(first.0.is_none());
            assert!(claim_at(&dir, &name, Duration::ZERO, None).is_some());
        }

        #[test]
        fn only_a_directory_no_one_else_can_write_is_private() {
            assert!(is_private(&private_dir()));
            assert!(!is_private(Path::new("/tmp")));
        }
    }
}

#[cfg(desktop_app)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use tao::dpi::LogicalSize;
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    #[cfg(target_os = "macos")]
    use tao::window::Theme;
    use tao::window::{Window, WindowBuilder};
    use wry::{NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder};

    #[cfg(target_os = "macos")]
    use muda::{
        accelerator::Accelerator, AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem,
        MenuItemKind, PredefinedMenuItem, Submenu,
    };
    #[cfg(target_os = "macos")]
    use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol, Sel};
    #[cfg(target_os = "macos")]
    use objc2::MainThreadMarker;
    #[cfg(target_os = "macos")]
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn, NSApplication, NSWritingToolsBehavior};
    #[cfg(target_os = "macos")]
    use objc2_foundation::NSString;
    #[cfg(target_os = "macos")]
    use objc2_web_kit::WKWebViewConfiguration;
    #[cfg(target_os = "macos")]
    use wry::{WebViewBuilderExtMacos, WebViewExtMacOS};

    #[cfg(target_os = "macos")]
    const MACOS_ESCAPE_SCRIPT: &str = r#"
        window.addEventListener("keydown", (event) => {
            if (event.key !== "Escape" || event.defaultPrevented) return;
            if (event.target?.matches?.("input[type=search]:enabled:not([readonly])")) return;
            // WebKit resends Escape to AppKit after its own dialog/popover dismissal, so dismiss here instead.
            event.preventDefault();
            // shortcut: last open popover stands in for the topmost; revisit if nested popovers render out of DOM order.
            const popover = CSS.supports("selector(:popover-open)") && [...document.querySelectorAll("[popover]:popover-open:not([popover=manual])")].pop();
            if (popover) return popover.hidePopover();
            // shortcut: old WebKit treats open dialogs as modal; revisit if non-modal dialogs are added.
            const dialog = [...document.querySelectorAll(CSS.supports("selector(:modal)") ? "dialog:modal" : "dialog[open]")].pop();
            if (dialog?.dispatchEvent(new Event("cancel", { cancelable: true }))) dialog.close();
        });
    "#;

    enum UserEvent {
        ServerReady,
        LoadTimedOut,
        SyncZoom,
        Zoom(ZoomAction),
        #[cfg(target_os = "macos")]
        Menu(MenuId),
        #[cfg(target_os = "macos")]
        MenuSpec(Vec<PageMenu>),
        #[cfg(target_os = "macos")]
        MenuState(HashMap<String, PageMenuState>),
        /// A later launch, with the `openresearch:` link it was opened with.
        #[cfg(not(target_os = "macos"))]
        Focus(Option<String>),
        /// The dashboard page finished loading and can take `orx:open-path`.
        PageReady,
        #[cfg(windows)]
        Close,
        #[cfg(windows)]
        SyncMaximized,
    }

    #[derive(Clone, Copy)]
    enum ZoomAction {
        In,
        Out,
        Reset,
    }

    impl ZoomAction {
        fn next(self, percent: i32) -> i32 {
            match self {
                Self::In => (percent + 10).min(200),
                Self::Out => (percent - 10).max(50),
                Self::Reset => 100,
            }
        }
    }

    enum Quit {
        No,
        Flushing,
        ShuttingDown,
    }

    pub(super) fn run_event_loop(
        port: u16,
        external_server: bool,
        #[cfg(not(target_os = "macos"))] focus_requests: super::instance::FocusRequests,
        #[cfg(not(target_os = "macos"))] launch_path: Option<String>,
    ) {
        let origin = format!(
            "http://{}:{port}",
            if external_server {
                "localhost"
            } else {
                "127.0.0.1"
            }
        );
        // Before GTK starts: the window class the dock matches to the desktop
        // entry's StartupWMClass.
        #[cfg(target_os = "linux")]
        gtk::glib::set_prgname(Some("OpenResearch"));
        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

        // Dashboard server on background workers (we're inside main's runtime).
        if !external_server {
            tokio::spawn(async move {
                let args = crate::UpArgs {
                    port,
                    remote: None,
                    no_browser: true,
                    no_agent: false,
                    model: None,
                    remote_host: false,
                    desktop_app: true,
                };
                // The window is useless without its server, so the app goes with it.
                match crate::commands::up::run(args).await {
                    Ok(()) => std::process::exit(0),
                    Err(err) => {
                        eprintln!("openresearch app: dashboard server exited: {err}");
                        crate::show_error_dialog(&format!("The dashboard server stopped: {err}"));
                        std::process::exit(1);
                    }
                }
            });
        }

        let ready = event_loop.create_proxy();
        tokio::spawn(async move {
            while tokio::net::TcpStream::connect((
                if external_server {
                    "localhost"
                } else {
                    "127.0.0.1"
                },
                port,
            ))
            .await
            .is_err()
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let _ = ready.send_event(UserEvent::ServerReady);
            tokio::time::sleep(Duration::from_secs(15)).await;
            let _ = ready.send_event(UserEvent::LoadTimedOut);
        });

        #[cfg(not(target_os = "macos"))]
        {
            let focus = event_loop.create_proxy();
            focus_requests.listen(move |link| {
                let _ = focus.send_event(UserEvent::Focus(link));
            });
        }
        #[cfg(windows)]
        set_taskbar_identity();

        #[cfg(target_os = "macos")]
        let mut menu = Menu::new();
        #[cfg(target_os = "macos")]
        {
            let proxy = event_loop.create_proxy();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let action = match event.id.as_ref() {
                    "zoom-in" => Some(ZoomAction::In),
                    "zoom-out" => Some(ZoomAction::Out),
                    "zoom-reset" => Some(ZoomAction::Reset),
                    _ => None,
                };
                let event = match action {
                    Some(action) => UserEvent::Zoom(action),
                    None => UserEvent::Menu(event.id),
                };
                let _ = proxy.send_event(event);
            }));
        }

        let window = WindowBuilder::new()
            .with_title("OpenResearch")
            .with_inner_size(LogicalSize::new(1280.0, 820.0))
            .with_min_inner_size(LogicalSize::new(720.0, 480.0))
            // Shown once the dashboard has loaded, so it rarely flashes blank.
            .with_visible(false);
        // The dashboard runs under the titlebar, leaving only the traffic lights.
        #[cfg(target_os = "macos")]
        let window = {
            use tao::platform::macos::WindowBuilderExtMacOS;
            window
                .with_titlebar_transparent(true)
                .with_title_hidden(true)
                .with_fullsize_content_view(true)
                // Centers the lights in the dashboard's 44px top bar (`mac-titlebar:h-11`).
                .with_traffic_light_inset(tao::dpi::LogicalPosition::new(16.0, 22.0))
        };
        #[cfg(windows)]
        let window = {
            use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows};
            // The dashboard draws the titlebar and its caption buttons itself.
            window
                .with_decorations(false)
                .with_undecorated_shadow(true)
                // Resource 1 is the icon build.rs embeds in orx.exe.
                .with_window_icon(tao::window::Icon::from_resource(1, None).ok())
        };
        #[cfg(target_os = "linux")]
        let window = window.with_window_icon(linux_window_icon());
        let window = match window.build(&event_loop) {
            Ok(window) => Rc::new(window),
            Err(err) => {
                crate::show_error_dialog(&format!("Could not open the window: {err}"));
                return;
            }
        };

        let shown = Rc::new(Cell::new(false));
        let page_ready = Rc::new(Cell::new(false));
        let replaced = Rc::new(RefCell::new(HashMap::new()));
        let builder = WebViewBuilder::new()
            .with_accept_first_mouse(true)
            // Shows the window material (`add_window_material`) where the page is transparent.
            .with_transparent(cfg!(target_os = "macos"))
            // Lets the dashboard tell it is in the app, where pop-ups open in the browser.
            .with_initialization_script("window.__ORX_DESKTOP__ = true;")
            .with_initialization_script_for_main_only(
                include_str!("desktop_zoom.js")
                    .replace("__ORX_ZOOM_PLATFORM__", std::env::consts::OS),
                false,
            )
            // Pop-ups go to the browser, dashboard pages included: a window wry
            // opens itself has none of this window's handlers.
            .with_new_window_req_handler(|url, _features| {
                if super::opens_in_browser(&url) {
                    crate::browser::open_browser(&url);
                }
                NewWindowResponse::Deny
            })
            .with_download_started_handler({
                let window = window.clone();
                let replaced = replaced.clone();
                move |url, path| choose_download_path(&window, url, path, &replaced)
            })
            .with_download_completed_handler(move |url, _path, success| {
                finish_replacing(&url, success, &replaced)
            })
            .with_document_title_changed_handler({
                let window = window.clone();
                move |title| window.set_title(&title)
            })
            .with_on_page_load_handler({
                let window = window.clone();
                let origin = origin.clone();
                let shown = shown.clone();
                let page_ready = page_ready.clone();
                let proxy = event_loop.create_proxy();
                move |event, url| {
                    if matches!(event, PageLoadEvent::Started) {
                        page_ready.set(false);
                    }
                    if !matches!(event, PageLoadEvent::Finished) {
                        return;
                    }
                    if super::is_dashboard_url(&url, &origin) {
                        let _ = proxy.send_event(UserEvent::SyncZoom);
                        page_ready.set(true);
                        let _ = proxy.send_event(UserEvent::PageReady);
                    }
                    #[cfg(windows)]
                    let _ = proxy.send_event(UserEvent::SyncMaximized);
                    if super::is_dashboard_url(&url, &origin) && !shown.replace(true) {
                        window.set_visible(true);
                        window.set_focus();
                    }
                }
            });
        #[cfg(windows)]
        let builder = {
            use wry::WebViewBuilderExtWindows;
            builder.with_browser_accelerator_keys(false)
        };
        let builder = builder.with_ipc_handler({
            #[cfg(not(target_os = "linux"))]
            let window = window.clone();
            let proxy = event_loop.create_proxy();
            move |request| {
                let message = request.body().as_str();
                match message {
                    "page-zoom:sync" => {
                        let _ = proxy.send_event(UserEvent::SyncZoom);
                    }
                    "page-zoom:in" => {
                        let _ = proxy.send_event(UserEvent::Zoom(ZoomAction::In));
                    }
                    "page-zoom:out" => {
                        let _ = proxy.send_event(UserEvent::Zoom(ZoomAction::Out));
                    }
                    "page-zoom:reset" => {
                        let _ = proxy.send_event(UserEvent::Zoom(ZoomAction::Reset));
                    }
                    #[cfg(not(target_os = "linux"))]
                    "titlebar:drag" => {
                        let _ = window.drag_window();
                    }
                    #[cfg(not(target_os = "linux"))]
                    "titlebar:zoom" => window.set_maximized(!window.is_maximized()),
                    // The window material and traffic lights follow the page's theme preference.
                    #[cfg(target_os = "macos")]
                    "appearance:system" => window.set_theme(None),
                    #[cfg(target_os = "macos")]
                    "appearance:light" => window.set_theme(Some(Theme::Light)),
                    #[cfg(target_os = "macos")]
                    "appearance:dark" => window.set_theme(Some(Theme::Dark)),
                    #[cfg(windows)]
                    "titlebar:minimize" => window.set_minimized(true),
                    #[cfg(windows)]
                    "titlebar:close" => {
                        let _ = proxy.send_event(UserEvent::Close);
                    }
                    #[cfg(windows)]
                    _ => {
                        if let Some(direction) = message
                            .strip_prefix("titlebar:resize:")
                            .and_then(top_resize_direction)
                        {
                            let _ = window.drag_resize_window(direction);
                        }
                    }
                    #[cfg(target_os = "linux")]
                    _ => {}
                    #[cfg(target_os = "macos")]
                    _ => {
                        if let Some(spec) = message.strip_prefix("menu:spec:") {
                            match serde_json::from_str(spec) {
                                Ok(spec) => {
                                    let _ = proxy.send_event(UserEvent::MenuSpec(spec));
                                }
                                Err(error) => {
                                    eprintln!("openresearch app: invalid menu spec: {error}")
                                }
                            }
                        } else if let Some(state) = message.strip_prefix("menu:state:") {
                            match serde_json::from_str(state) {
                                Ok(state) => {
                                    let _ = proxy.send_event(UserEvent::MenuState(state));
                                }
                                Err(error) => {
                                    eprintln!("openresearch app: invalid menu state: {error}")
                                }
                            }
                        }
                    }
                }
            }
        });
        #[cfg(target_os = "macos")]
        // Run after dashboard handlers; keep overlay dismissal, not WebKit's native fullscreen exit.
        let builder = builder
            .with_initialization_script("window.__ORX_MAC_TITLEBAR__ = true;")
            .with_initialization_script_for_main_only(MACOS_ESCAPE_SCRIPT, false);
        #[cfg(target_os = "macos")]
        let builder = {
            let mtm = MainThreadMarker::new().expect("desktop app runs on the main thread");
            let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
            if configuration.respondsToSelector(objc2::sel!(setWritingToolsBehavior:)) {
                unsafe {
                    let _: () = objc2::msg_send![&*configuration, setWritingToolsBehavior: NSWritingToolsBehavior::None];
                }
            }
            builder.with_webview_configuration(configuration)
        };
        #[cfg(windows)]
        let builder = builder.with_initialization_script("window.__ORX_WIN_TITLEBAR__ = true;");
        #[cfg(not(target_os = "linux"))]
        let webview = builder.build(&*window);
        // `build` supports only X11 on Linux.
        #[cfg(target_os = "linux")]
        let webview = {
            use tao::platform::unix::WindowExtUnix;
            use wry::WebViewBuilderExtUnix;
            let vbox = window
                .default_vbox()
                .expect("tao gives every window a default vbox");
            builder.build_gtk(vbox)
        };
        let webview = match webview {
            Ok(webview) => webview,
            Err(err) => {
                crate::show_error_dialog(&format!("Could not open the dashboard view: {err}"));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        add_confirm_panel(&webview);
        #[cfg(target_os = "macos")]
        add_window_material(&webview);

        let mut zoom_percent = crate::config::desktop_zoom_percent();
        if let Err(error) = sync_page_zoom(&webview, zoom_percent, false) {
            eprintln!("openresearch app: could not restore page zoom: {error}");
        }
        #[cfg(target_os = "macos")]
        align_traffic_lights(&window, zoom_percent);
        let mut quit = Quit::No;
        let mut links = DeepLinks {
            server_ready: false,
            page_ready,
            #[cfg(target_os = "macos")]
            pending: None,
            #[cfg(not(target_os = "macos"))]
            pending: launch_path,
        };
        // What the loaded page was last told; a new page starts unmaximized.
        #[cfg(windows)]
        let mut page_maximized = false;
        event_loop.run(move |event, _, control_flow| match event {
            Event::NewEvents(StartCause::Init) => {
                *control_flow = ControlFlow::Wait;
                #[cfg(target_os = "macos")]
                menu.init_for_nsapp();
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => match quit {
                Quit::No => {}
                // The server's own shutdown path stops the agents and then exits
                // the process.
                Quit::Flushing => {
                    if external_server {
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                    crate::commands::up::request_shutdown();
                    quit = Quit::ShuttingDown;
                    *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(5));
                }
                Quit::ShuttingDown => *control_flow = ControlFlow::Exit,
            },
            Event::UserEvent(UserEvent::ServerReady) if matches!(quit, Quit::No) => {
                let _ = webview.load_url(&format!("{origin}{}", links.on_server_ready()));
            }
            // A window that never appears is worse than a blank one.
            Event::UserEvent(UserEvent::LoadTimedOut)
                if matches!(quit, Quit::No) && !shown.replace(true) =>
            {
                eprintln!("openresearch app: the dashboard has not finished loading; showing the window anyway");
                window.set_visible(true);
                window.set_focus();
            }
            #[cfg(windows)]
            Event::UserEvent(UserEvent::Close) => {
                begin_quit(&mut quit, &window, &webview, control_flow);
            }
            #[cfg(windows)]
            Event::UserEvent(UserEvent::SyncMaximized) => {
                page_maximized = window.is_maximized();
                sync_maximized(&webview, page_maximized);
            }
            #[cfg(windows)]
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } if window.is_maximized() != page_maximized => {
                page_maximized = !page_maximized;
                sync_maximized(&webview, page_maximized);
            }
            #[cfg(target_os = "macos")]
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } => align_traffic_lights(&window, zoom_percent),
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(id)) if id.as_ref() == "app.quit" => {
                begin_quit(&mut quit, &window, &webview, control_flow);
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(id)) if id.as_ref() == "window.close" => {
                NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).hide(None);
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::MenuSpec(spec)) => match build_menu(&spec) {
                Ok(next) => {
                    next.init_for_nsapp();
                    menu = next;
                }
                Err(error) => eprintln!("openresearch app: could not build menu: {error}"),
            },
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::MenuState(states)) => {
                for submenu in menu.items().iter().filter_map(MenuItemKind::as_submenu) {
                    for item in submenu.items() {
                        let Some(state) = states.get(item.id().as_ref()) else { continue; };
                        match item {
                            MenuItemKind::MenuItem(item) => {
                                if let Some(enabled) = state.enabled { item.set_enabled(enabled); }
                            }
                            MenuItemKind::Check(item) => {
                                if let Some(enabled) = state.enabled { item.set_enabled(enabled); }
                                if let Some(checked) = state.checked { item.set_checked(checked); }
                            }
                            _ => {}
                        }
                    }
                }
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(id)) => {
                let _ = webview.evaluate_script(&format!(
                    "window.dispatchEvent(new CustomEvent('orx:command', {{ detail: {} }}));",
                    serde_json::Value::from(id.as_ref())
                ));
            }
            Event::UserEvent(UserEvent::Zoom(action)) if matches!(quit, Quit::No) => {
                let next = action.next(zoom_percent);
                match sync_page_zoom(&webview, next, true) {
                    Ok(()) => {
                        if next != zoom_percent {
                            zoom_percent = next;
                            #[cfg(target_os = "macos")]
                            align_traffic_lights(&window, zoom_percent);
                            if let Err(error) = crate::config::set_desktop_zoom_percent(next) {
                                eprintln!("openresearch app: could not save page zoom: {error}");
                            }
                        }
                    }
                    Err(error) => eprintln!("openresearch app: could not apply page zoom: {error}"),
                }
            }
            Event::UserEvent(UserEvent::SyncZoom) if matches!(quit, Quit::No) => {
                let _ = sync_page_zoom(&webview, zoom_percent, false);
                // The startup alignment measures a window that is still hidden and unlaid-out.
                #[cfg(target_os = "macos")]
                align_traffic_lights(&window, zoom_percent);
            }
            // Even before the page loads: a launch that brings nothing up looks
            // like a broken app. While quitting it must stay hidden.
            #[cfg(not(target_os = "macos"))]
            Event::UserEvent(UserEvent::Focus(link)) if matches!(quit, Quit::No) => {
                if let Some(path) = link.as_deref().and_then(super::link_path) {
                    links.open(path, &webview, &origin);
                }
                shown.set(true);
                window.set_minimized(false);
                window.set_visible(true);
                window.set_focus();
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                #[cfg(target_os = "macos")]
                if external_server {
                    begin_quit(&mut quit, &window, &webview, control_flow);
                } else {
                    // AppKit switches Spaces when hiding the app, unlike ordering out its window.
                    NSApplication::sharedApplication(MainThreadMarker::new().unwrap()).hide(None);
                }
                #[cfg(not(target_os = "macos"))]
                begin_quit(&mut quit, &window, &webview, control_flow);
            }
            #[cfg(target_os = "macos")]
            Event::Reopen { .. } => {
                shown.set(true);
                window.set_visible(true);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::PageReady) if matches!(quit, Quit::No) => {
                links.on_page_ready(&webview);
            }
            #[cfg(target_os = "macos")]
            Event::Opened { urls } if matches!(quit, Quit::No) => {
                let Some(path) = urls.iter().find_map(super::deep_link_path) else {
                    return;
                };
                links.open(path, &webview, &origin);
                // Before the first page shows, the page-load handler reveals the window.
                if shown.get() {
                    window.set_minimized(false);
                    window.set_visible(true);
                    window.set_focus();
                }
            }
            _ => {}
        })
    }

    /// Where `openresearch:` links go. One that opens the app arrives before its server answers.
    struct DeepLinks {
        server_ready: bool,
        page_ready: Rc<Cell<bool>>,
        pending: Option<String>,
    }

    impl DeepLinks {
        /// The path the first page load opens.
        fn on_server_ready(&mut self) -> String {
            self.server_ready = true;
            self.pending.take().unwrap_or_else(|| "/".to_string())
        }

        fn on_page_ready(&mut self, webview: &WebView) {
            if let Some(path) = self.pending.take() {
                open_in_page(webview, &path);
            }
        }

        fn open(&mut self, path: String, webview: &WebView, origin: &str) {
            if self.page_ready.get() {
                open_in_page(webview, &path);
            } else if self.server_ready
                && !webview
                    .url()
                    .is_ok_and(|url| super::is_dashboard_url(&url, origin))
            {
                // A timed-out or failed first load has nothing coming to deliver it.
                let _ = webview.load_url(&format!("{origin}{path}"));
            } else {
                // ServerReady loads it, or PageReady hands it to the loading page.
                self.pending = Some(path);
            }
        }
    }

    /// In-page navigation keeps the page's unsaved-work guards; a reload would skip them.
    fn open_in_page(webview: &WebView, path: &str) {
        let _ = webview.evaluate_script(&format!(
            "window.dispatchEvent(new CustomEvent('orx:open-path', {{ detail: {} }}));",
            serde_json::Value::from(path)
        ));
    }

    fn begin_quit(
        quit: &mut Quit,
        window: &Window,
        webview: &WebView,
        control_flow: &mut ControlFlow,
    ) {
        // A repeat request would restart the sequence and push back the fallback exit.
        if !matches!(quit, Quit::No) {
            return;
        }
        window.set_visible(false);
        // Unloading fires the page's `pagehide` flush of workspace state,
        // which has to reach the in-process server before it exits.
        let _ = webview.load_url("about:blank");
        *quit = Quit::Flushing;
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(300));
    }

    /// The AppImage's icon, for the title bar and a taskbar that doesn't match
    /// the window to an installed desktop entry.
    #[cfg(target_os = "linux")]
    fn linux_window_icon() -> Option<tao::window::Icon> {
        let png = include_bytes!("../../linux/OpenResearch.png");
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .ok()?;
        let mut rgba = vec![0; reader.output_buffer_size()?];
        let frame = reader.next_frame(&mut rgba).ok()?;
        rgba.truncate(frame.buffer_size());
        tao::window::Icon::from_rgba(rgba, frame.width, frame.height).ok()
    }

    /// Matches the Start menu shortcut's AppUserModelID, so the taskbar groups
    /// the window with the shortcut and pins the launcher rather than orx.exe.
    #[cfg(windows)]
    fn set_taskbar_identity() {
        use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

        let id = super::wide("alphaXiv.OpenResearch");
        // SAFETY: the string is NUL-terminated and outlives the call.
        unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    }

    /// The top edge only: the shadow frame tao keeps around an undecorated window
    /// resizes the others, but the webview covers the top.
    #[cfg(windows)]
    fn top_resize_direction(edge: &str) -> Option<tao::window::ResizeDirection> {
        use tao::window::ResizeDirection;
        match edge {
            "n" => Some(ResizeDirection::North),
            "ne" => Some(ResizeDirection::NorthEast),
            "nw" => Some(ResizeDirection::NorthWest),
            _ => None,
        }
    }

    /// The page swaps its maximize and restore buttons on this.
    #[cfg(windows)]
    fn sync_maximized(webview: &WebView, maximized: bool) {
        let script =
            format!("document.documentElement.toggleAttribute('data-maximized', {maximized});");
        let _ = webview.evaluate_script(&script);
    }

    fn sync_page_zoom(webview: &WebView, percent: i32, show_controls: bool) -> wry::Result<()> {
        let factor = f64::from(percent) / 100.0;
        webview.zoom(factor)?;
        // The native traffic lights and drag strip keep their unscaled size.
        webview.evaluate_script(&format!(
            "document.documentElement.style.setProperty('--desktop-page-zoom', '{factor}');
             if ({show_controls}) window.dispatchEvent(new CustomEvent('orx:page-zoom', {{ detail: {percent} }}));"
        ))
    }

    #[cfg(target_os = "macos")]
    fn align_traffic_lights(window: &Window, percent: i32) {
        use objc2_app_kit::{NSWindow, NSWindowButton};
        use objc2_foundation::NSPoint;
        use tao::dpi::LogicalPosition;
        use tao::platform::macos::WindowExtMacOS;

        let zoom = (f64::from(percent) / 100.0).max(1.0);
        // Tao owns this window for the duration of the main-thread event.
        let native_window = unsafe { &*window.ns_window().cast::<NSWindow>() };
        let inset = native_window
            .standardWindowButton(NSWindowButton::CloseButton)
            .and_then(|button| {
                let container = unsafe { button.superview()?.superview()? };
                let bounds = button.bounds();
                let center = button.convertPoint_toView(
                    NSPoint::new(
                        bounds.origin.x + bounds.size.width / 2.0,
                        bounds.origin.y + bounds.size.height / 2.0,
                    ),
                    None,
                );
                let current = native_window.frame().size.height - center.y;
                Some(
                    container.frame().size.height - button.frame().size.height + 20.0 * zoom
                        - current,
                )
            })
            .unwrap_or(22.0 + 20.0 * (zoom - 1.0));
        window.set_traffic_light_inset(LogicalPosition::new(16.0, inset));
        // Tao reapplies the inset on a title change, not on request_redraw.
        window.set_title(&window.title());
    }

    #[cfg(target_os = "macos")]
    #[derive(serde::Deserialize)]
    struct PageMenu {
        id: String,
        label: String,
        items: Vec<PageMenuEntry>,
    }

    #[cfg(target_os = "macos")]
    #[derive(serde::Deserialize)]
    #[serde(tag = "kind", rename_all = "lowercase")]
    enum PageMenuEntry {
        Command {
            id: String,
            label: String,
            hotkey: Option<serde_json::Value>,
            checked: Option<bool>,
        },
        Native {
            name: String,
        },
        Separator,
    }

    #[cfg(target_os = "macos")]
    #[derive(serde::Deserialize)]
    struct PageMenuState {
        enabled: Option<bool>,
        checked: Option<bool>,
    }

    #[cfg(target_os = "macos")]
    fn build_menu(spec: &[PageMenu]) -> anyhow::Result<Menu> {
        let menu = Menu::new();
        for section in spec {
            let submenu = Submenu::with_id(&section.id, &section.label, true);
            for entry in &section.items {
                match entry {
                    PageMenuEntry::Command {
                        id,
                        label,
                        hotkey,
                        checked,
                    } => {
                        let accelerator = hotkey
                            .as_ref()
                            .and_then(serde_json::Value::as_str)
                            .map(|hotkey| hotkey.replace("Mod", "CmdOrCtrl").parse::<Accelerator>())
                            .transpose()?;
                        if let Some(checked) = checked {
                            submenu.append(&CheckMenuItem::with_id(
                                id,
                                label,
                                true,
                                *checked,
                                accelerator,
                            ))?;
                        } else {
                            submenu.append(&MenuItem::with_id(id, label, true, accelerator))?;
                        }
                    }
                    PageMenuEntry::Separator => submenu.append(&PredefinedMenuItem::separator())?,
                    PageMenuEntry::Native { name } => {
                        let item = match name.as_str() {
                            "about" => PredefinedMenuItem::about(
                                None,
                                Some(AboutMetadata {
                                    name: Some("OpenResearch".into()),
                                    version: Some(env!("CARGO_PKG_VERSION").into()),
                                    ..Default::default()
                                }),
                            ),
                            "services" => PredefinedMenuItem::services(None),
                            "hide" => PredefinedMenuItem::hide(None),
                            "hide_others" => PredefinedMenuItem::hide_others(None),
                            "show_all" => PredefinedMenuItem::show_all(None),
                            "undo" => PredefinedMenuItem::undo(None),
                            "redo" => PredefinedMenuItem::redo(None),
                            "cut" => PredefinedMenuItem::cut(None),
                            "copy" => PredefinedMenuItem::copy(None),
                            "paste" => PredefinedMenuItem::paste(None),
                            "select_all" => PredefinedMenuItem::select_all(None),
                            "fullscreen" => PredefinedMenuItem::fullscreen(None),
                            "minimize" => PredefinedMenuItem::minimize(None),
                            "maximize" => PredefinedMenuItem::maximize(None),
                            _ => anyhow::bail!("unknown native menu item: {name}"),
                        };
                        submenu.append(&item)?;
                    }
                }
            }
            if section.id == "window" {
                submenu.set_as_windows_menu_for_nsapp();
            }
            menu.append(&submenu)?;
        }
        Ok(menu)
    }

    /// Without a handler WKWebView drops downloads and WebView2 saves them
    /// silently into Downloads; ask where, as a browser would.
    fn choose_download_path(
        window: &Window,
        url: String,
        path: &mut PathBuf,
        replaced: &RefCell<HashMap<String, PathBuf>>,
    ) -> bool {
        let Some(chosen) = ask_save_path(window, path) else {
            return false;
        };
        // No webview writes over an existing file, and the dialog has already
        // confirmed replacing it; the original waits aside until the download
        // completes, or comes back if it doesn't.
        if chosen.exists() {
            if std::fs::rename(&chosen, set_aside_path(&chosen)).is_err() {
                return false;
            }
            replaced.borrow_mut().insert(url, chosen.clone());
        }
        *path = chosen;
        true
    }

    #[cfg(not(target_os = "linux"))]
    fn ask_save_path(window: &Window, suggested: &std::path::Path) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new();
        // Owned by the window, which Windows disables while the dialog is up. On
        // macOS a parent turns the panel into a sheet, which a hidden window can't show.
        #[cfg(windows)]
        {
            dialog = dialog.set_parent(window);
        }
        #[cfg(not(windows))]
        let _ = window;
        if let Some(dir) = suggested.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = suggested.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        dialog.save_file()
    }

    /// On the main thread, which owns GTK here: WebKit asks from a signal
    /// handler there, and the modal loop keeps the window painting.
    #[cfg(target_os = "linux")]
    fn ask_save_path(window: &Window, suggested: &std::path::Path) -> Option<PathBuf> {
        use gtk::prelude::*;
        use tao::platform::unix::WindowExtUnix;

        let dialog = gtk::FileChooserDialog::with_buttons(
            None,
            Some(window.gtk_window()),
            gtk::FileChooserAction::Save,
            &[
                ("_Cancel", gtk::ResponseType::Cancel),
                ("_Save", gtk::ResponseType::Accept),
            ],
        );
        dialog.set_do_overwrite_confirmation(true);
        if let Some(dir) = suggested.parent() {
            dialog.set_current_folder(dir);
        }
        if let Some(name) = suggested.file_name() {
            dialog.set_current_name(&name.to_string_lossy());
        }
        let chosen = (dialog.run() == gtk::ResponseType::Accept)
            .then(|| dialog.filename())
            .flatten();
        dialog.close();
        chosen
    }

    /// Keyed by URL: wry reports no path on a finished macOS download, nor on
    /// a failed Linux one.
    fn finish_replacing(url: &str, success: bool, replaced: &RefCell<HashMap<String, PathBuf>>) {
        let Some(path) = replaced.borrow_mut().remove(url) else {
            return;
        };
        let original = set_aside_path(&path);
        let _ = if success {
            std::fs::remove_file(original)
        } else {
            std::fs::rename(original, path)
        };
    }

    fn set_aside_path(path: &Path) -> PathBuf {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".orx-replaced");
        path.with_file_name(name)
    }

    /// The sidebar's blur: the page leaves the sidebar transparent over this view.
    #[cfg(target_os = "macos")]
    fn add_window_material(webview: &WebView) {
        use objc2_app_kit::{
            NSAutoresizingMaskOptions, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
            NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
        };

        let mtm = MainThreadMarker::new().expect("desktop app runs on the main thread");
        let wk = webview.webview();
        let Some(parent) = (unsafe { wk.superview() }) else {
            return;
        };
        let material = NSVisualEffectView::new(mtm);
        material.setFrame(parent.bounds());
        material.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        material.setMaterial(NSVisualEffectMaterial::Sidebar);
        material.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        material.setState(NSVisualEffectState::FollowsWindowActiveState);
        parent.addSubview_positioned_relativeTo(&material, NSWindowOrderingMode::Below, Some(&wk));
    }

    /// wry's WKUIDelegate has no confirm panel, and without one WebKit answers
    /// every `window.confirm` with false, so add it to the delegate's class.
    #[cfg(target_os = "macos")]
    fn add_confirm_panel(webview: &WebView) {
        type ConfirmPanel = unsafe extern "C-unwind" fn(
            &AnyObject,
            Sel,
            *mut AnyObject,
            &NSString,
            *mut AnyObject,
            &block2::Block<dyn Fn(Bool)>,
        );
        let wk = webview.webview();
        unsafe {
            let delegate: *mut AnyObject = objc2::msg_send![&*wk, UIDelegate];
            let Some(delegate) = delegate.as_ref() else {
                return;
            };
            let class: *const AnyClass = delegate.class();
            let imp: ConfirmPanel = run_confirm_panel;
            objc2::ffi::class_addMethod(
                class.cast_mut(),
                objc2::sel!(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:),
                std::mem::transmute::<ConfirmPanel, objc2::runtime::Imp>(imp),
                c"v@:@@@@?".as_ptr(),
            );
            // WebKit reads which methods a delegate has only when it is set.
            let _: () = objc2::msg_send![&*wk, setUIDelegate: delegate];
        }
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C-unwind" fn run_confirm_panel(
        _this: &AnyObject,
        _cmd: Sel,
        _webview: *mut AnyObject,
        message: &NSString,
        _frame: *mut AnyObject,
        handler: &block2::Block<dyn Fn(Bool)>,
    ) {
        // SAFETY: WebKit calls its UI delegate on the main thread.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        // An NSAlert modal keeps the run loop turning, so the window still paints.
        let alert = NSAlert::new(mtm);
        alert.setMessageText(message);
        alert.addButtonWithTitle(&NSString::from_str("OK"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        let confirmed = alert.runModal() == NSAlertFirstButtonReturn;
        handler.call((Bool::new(confirmed),));
    }

    #[cfg(test)]
    mod tests {
        use super::ZoomAction;

        #[test]
        fn zoom_steps_clamp_and_reset() {
            assert_eq!(ZoomAction::In.next(100), 110);
            assert_eq!(ZoomAction::In.next(200), 200);
            assert_eq!(ZoomAction::Out.next(100), 90);
            assert_eq!(ZoomAction::Out.next(50), 50);
            assert_eq!(ZoomAction::Reset.next(50), 100);
            assert_eq!(ZoomAction::Reset.next(200), 100);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        app_launch, desktop_entry, is_bundle_exe_launch, is_dashboard_url, link_path,
        opens_in_browser,
    };
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    #[test]
    fn the_desktop_entry_launches_the_appimage_wherever_it_is() {
        let entry = desktop_entry(
            r"/home/me/My Apps/Open$Re%1\x.AppImage",
            "/home/me/.local/share/icons/hicolor/256x256/apps/openresearch.png",
        );
        assert!(entry.contains("\nExec=\"/home/me/My Apps/Open\\\\$Re%%1\\\\\\\\x.AppImage\" %u\n"));
        assert!(entry.contains("\nTryExec=/home/me/My Apps/Open$Re%1\\\\x.AppImage\n"));
        assert!(entry.contains(
            "\nIcon=/home/me/.local/share/icons/hicolor/256x256/apps/openresearch.png\n"
        ));
        assert!(entry.contains("\nStartupWMClass=OpenResearch\n"));
        assert!(entry.contains("\nMimeType=x-scheme-handler/openresearch;\n"));
        assert!(!entry.contains("orx app"));
    }

    #[test]
    fn reproduce_links_open_the_reproduce_page_for_their_paper() {
        assert_eq!(
            link_path("openresearch://reproduce/1706.03762").as_deref(),
            Some("/reproduce/1706.03762")
        );
        assert_eq!(
            link_path("openresearch://reproduce/hep-th/9901001").as_deref(),
            Some("/reproduce/hep-th/9901001")
        );
        assert_eq!(link_path("openresearch://reproduce/"), None);
        assert_eq!(link_path("openresearch://reproduce"), None);
        assert_eq!(link_path("openresearch://settings/x"), None);
        assert_eq!(link_path("https://reproduce/x"), None);
        assert_eq!(link_path("not a link"), None);
        assert_eq!(
            link_path("OPENRESEARCH://reproduce/1").as_deref(),
            Some("/reproduce/1")
        );
    }

    fn app_args(args: &[&str]) -> Option<Option<String>> {
        app_launch(args.iter().map(OsString::from))
    }

    #[test]
    fn the_app_takes_only_an_openresearch_link_after_app() {
        let link = "openresearch://reproduce/1706.03762";
        assert_eq!(app_args(&["app"]), Some(None));
        assert_eq!(app_args(&["app", link]), Some(Some(link.to_string())));
        assert_eq!(
            app_args(&["app", "OpenResearch://reproduce/x"]),
            Some(Some("OpenResearch://reproduce/x".to_string()))
        );
        assert_eq!(app_args(&[]), None);
        assert_eq!(app_args(&["up"]), None);
        assert_eq!(app_args(&["app", "--help"]), None);
        assert_eq!(app_args(&["app", "https://example.com"]), None);
        assert_eq!(app_args(&["app", link, link]), None);
        assert_eq!(app_args(&["app", "openresearch"]), None);
        assert_eq!(app_args(&["app", "openresearché"]), None);
    }

    const EXE: &str = "/Applications/OpenResearch.app/Contents/MacOS/OpenResearch";

    #[test]
    fn finder_and_direct_runs_of_the_bundle_exe_are_app_launches() {
        assert!(is_bundle_exe_launch(Path::new(EXE), Some(OsStr::new(EXE))));
        assert!(is_bundle_exe_launch(
            Path::new(EXE),
            Some(OsStr::new("./OpenResearch"))
        ));
    }

    #[test]
    fn the_bundles_orx_symlink_stays_a_cli() {
        // `exe` is canonicalized, so the symlink shows up only in argv.
        assert!(!is_bundle_exe_launch(
            Path::new(EXE),
            Some(OsStr::new("orx"))
        ));
        assert!(!is_bundle_exe_launch(
            Path::new(EXE),
            Some(OsStr::new(
                "/Applications/OpenResearch.app/Contents/MacOS/orx"
            ))
        ));
    }

    #[test]
    fn installs_outside_a_bundle_are_never_app_launches() {
        assert!(!is_bundle_exe_launch(
            Path::new("/usr/local/bin/orx"),
            Some(OsStr::new("orx"))
        ));
        assert!(!is_bundle_exe_launch(Path::new(EXE), None));
    }

    #[test]
    fn only_dashboard_pages_count_as_the_dashboard() {
        let origin = "http://127.0.0.1:4792";
        assert!(is_dashboard_url("http://127.0.0.1:4792", origin));
        assert!(is_dashboard_url(
            "http://127.0.0.1:4792/remote-launch",
            origin
        ));
        assert!(!is_dashboard_url("about:blank", origin));
        assert!(!is_dashboard_url("http://127.0.0.1:47920/", origin));
        assert!(!is_dashboard_url("https://github.com/alphaXiv", origin));
        assert!(!is_dashboard_url(
            "https://www.overleaf.com/project",
            origin
        ));
    }

    #[test]
    fn only_web_and_mail_links_reach_the_system_browser() {
        assert!(opens_in_browser("https://github.com/alphaXiv"));
        assert!(opens_in_browser("http://127.0.0.1:4792/api/artifacts/raw"));
        assert!(opens_in_browser("mailto:team@example.com"));
        assert!(!opens_in_browser("ssh://attacker.example"));
        assert!(!opens_in_browser("vscode://file/etc/passwd"));
        assert!(!opens_in_browser(
            "x-apple.systempreferences:com.apple.preference"
        ));
    }
}
