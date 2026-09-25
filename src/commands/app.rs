//! Desktop app mode — the GUI entry point for the downloadable OpenResearch app.
//!
//! On macOS the `.app` bundle's executable IS the `orx` binary; launched from
//! Finder with no arguments, `main` routes here instead of parsing CLI args. On
//! Windows the installed `OpenResearch.exe` launcher (windows/launcher) starts
//! the `orx.exe` beside it as `orx app`, in a hidden console its children share.
//! Either way app mode owns the main thread with the window's run loop, while the
//! `orx up` dashboard server runs on background tokio worker threads.
//!
//! This is distinct from `orx up` launched in a terminal, which stays a plain
//! CLI. The GUI parts are macOS- and Windows-only; other targets compile them away.

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

/// The whole argument list the Windows launcher starts `orx.exe` with; `orx app`
/// with anything after it is not the app.
#[cfg(windows)]
pub const WINDOWS_APP_ARG: &str = "app";

#[cfg(windows)]
pub fn launched_as_windows_app() -> bool {
    let mut args = std::env::args_os().skip(1);
    args.next().is_some_and(|arg| arg == WINDOWS_APP_ARG) && args.next().is_none()
}

#[cfg(any(target_os = "macos", windows))]
const APP_PORT: u16 = 4792;

/// Enter GUI app mode: pick a port, start the dashboard server on background
/// threads, and hand the main thread to the window's run loop. Returns only if
/// setup fails; quitting exits the process.
#[cfg(any(target_os = "macos", windows))]
pub async fn run() {
    #[cfg(windows)]
    let focus_requests = {
        // A relaunched app first waits out its predecessor, which holds the claim.
        crate::updates::await_replaced_parent();
        match instance::claim() {
            Some(requests) => requests,
            None => return,
        }
    };
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
    imp::run_event_loop(port);
    #[cfg(windows)]
    imp::run_event_loop(port, focus_requests);
}

/// Adopt the user's shell environment in place of the one launchd handed us
/// (see [`crate::local::shell_env`]).
///
/// `-ilc`, not `-lc`: zsh reads `.zshrc` only for *interactive* shells, and
/// that is where these exports overwhelmingly live. The inner `sh -c` keeps the
/// answer portable — the outer shell execs `/bin/sh`, which prints the values it
/// inherited, where fish would have printed its own list-valued `$PATH`
/// space-separated. NUL separates them because a PATH or a directory may
/// contain spaces, colons, and newlines, but never NUL.
#[cfg(target_os = "macos")]
pub(crate) async fn hydrate_shell_env() {
    // Nonce, so rc-file chatter can't forge the fence around the values. The
    // leading `_` is load-bearing: `printf` reads `\0` plus up to three octal
    // digits, so a marker starting with a digit would be eaten by the escape.
    let marker = format!("__ORX_ENV_{}__", uuid::Uuid::new_v4().simple());
    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    let reads = crate::local::shell_env::IMPORTED
        .map(|key| format!(r#""${key}""#))
        .join(" ");
    let template = "%s\\0".repeat(crate::local::shell_env::IMPORTED.len());
    let script = format!(r#"/bin/sh -c 'printf "{marker}{template}{marker}" {reads}'"#);
    let fut = tokio::process::Command::new(&shell)
        .args(["-ilc".to_string(), script])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    // A slow rc file (nvm, conda) delays the dashboard, so cap the wait; the
    // inherited environment stays in force when the probe doesn't answer.
    let out = match tokio::time::timeout(std::time::Duration::from_secs(5), fut).await {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => {
            eprintln!(
                "openresearch app: could not run {shell:?}: {err}; using the inherited environment"
            );
            return;
        }
        Err(_) => {
            eprintln!("openresearch app: {shell:?} did not answer within 5s; using the inherited environment");
            return;
        }
    };
    // The markers are the success signal, not the exit status — an interactive
    // rc file routinely ends on a failing command.
    match crate::local::shell_env::parse_probe(&String::from_utf8_lossy(&out.stdout), &marker) {
        Some(vars) => {
            let adopted: Vec<String> = crate::local::shell_env::IMPORTED
                .iter()
                .filter_map(|key| Some(format!("{key}={:?}", vars.get(key)?)))
                .collect();
            eprintln!(
                "openresearch app: adopted the shell environment: {}",
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
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn is_dashboard_url(url: &str, origin: &str) -> bool {
    url.strip_prefix(origin)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Schemes a pop-up may hand to the system browser. A browser asks before
/// launching the app behind any other scheme (`ssh:`, `vscode:`); `open` would not.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
fn opens_in_browser(url: &str) -> bool {
    ["http:", "https:", "mailto:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

/// One app per user session on Windows, where nothing else enforces it: a
/// second launch brings the running window forward and exits.
#[cfg(windows)]
mod instance {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, SetEvent, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY};

    const MUTEX: &str = r"Local\OpenResearchApp";
    const FOCUS_EVENT: &str = r"Local\OpenResearchAppFocus";

    pub(super) fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// Signalled by each later launch.
    pub(super) struct FocusRequests(HANDLE);

    // SAFETY: an event handle may be waited on from any thread.
    unsafe impl Send for FocusRequests {}

    /// Claims the app for this process. `None` means another instance has it
    /// and was asked to come forward.
    pub(super) fn claim() -> Option<FocusRequests> {
        // SAFETY: plain syscalls on NUL-terminated names. The handles stay open
        // on purpose: the claim lasts until the process exits.
        unsafe {
            // The event first, so it exists by the time anyone sees the claim.
            // For a second launch this opens the running app's event.
            let event = CreateEventW(std::ptr::null(), 0, 0, wide(FOCUS_EVENT).as_ptr());
            CreateMutexW(std::ptr::null(), 0, wide(MUTEX).as_ptr());
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // Windows lets the running app take the foreground only with our leave.
                AllowSetForegroundWindow(ASFW_ANY);
                SetEvent(event);
                CloseHandle(event);
                return None;
            }
            Some(FocusRequests(event))
        }
    }

    impl FocusRequests {
        /// Calls `on_request` for every later launch, from a thread of its own.
        pub(super) fn listen(self, on_request: impl Fn() + Send + 'static) {
            std::thread::spawn(move || {
                while self.wait() {
                    on_request();
                }
            });
        }

        // A method, so the thread above captures the `Send` wrapper and not
        // its bare handle.
        fn wait(&self) -> bool {
            // SAFETY: waits on the event this process created and never closes.
            unsafe { WaitForSingleObject(self.0, INFINITE) == WAIT_OBJECT_0 }
        }
    }
}

#[cfg(any(target_os = "macos", windows))]
mod imp {
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use tao::dpi::LogicalSize;
    use tao::event::{Event, StartCause, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::{Window, WindowBuilder};
    use wry::{NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder};

    #[cfg(target_os = "macos")]
    use muda::{
        accelerator::{Accelerator, Code, Modifiers},
        AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu,
    };
    #[cfg(target_os = "macos")]
    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    #[cfg(target_os = "macos")]
    use objc2::MainThreadMarker;
    #[cfg(target_os = "macos")]
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn};
    #[cfg(target_os = "macos")]
    use objc2_foundation::NSString;
    #[cfg(target_os = "macos")]
    use wry::WebViewExtMacOS;

    enum UserEvent {
        ServerReady,
        #[cfg(target_os = "macos")]
        Menu(MenuId),
        #[cfg(windows)]
        Focus,
    }

    enum Quit {
        No,
        Flushing,
        ShuttingDown,
    }

    pub(super) fn run_event_loop(
        port: u16,
        #[cfg(windows)] focus_requests: super::instance::FocusRequests,
    ) {
        let origin = format!("http://127.0.0.1:{port}");
        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();

        // Dashboard server on background workers (we're inside main's runtime).
        tokio::spawn(async move {
            let args = crate::UpArgs {
                port,
                remote: None,
                no_browser: true,
                no_agent: false,
                model: None,
                remote_host: false,
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

        let ready = event_loop.create_proxy();
        tokio::spawn(async move {
            while tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let _ = ready.send_event(UserEvent::ServerReady);
        });

        #[cfg(windows)]
        {
            let focus = event_loop.create_proxy();
            focus_requests.listen(move || {
                let _ = focus.send_event(UserEvent::Focus);
            });
            set_taskbar_identity();
        }

        #[cfg(target_os = "macos")]
        let (menu, quit_item, reload_item) = {
            let menu_proxy = event_loop.create_proxy();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let _ = menu_proxy.send_event(UserEvent::Menu(event.id));
            }));
            let quit = MenuItem::new(
                "Quit OpenResearch",
                true,
                Some(Accelerator::new(Modifiers::META, Code::KeyQ)),
            );
            let reload = MenuItem::new(
                "Reload",
                true,
                Some(Accelerator::new(Modifiers::META, Code::KeyR)),
            );
            match build_menu(&quit, &reload) {
                Ok(menu) => (menu, quit, reload),
                Err(err) => {
                    crate::show_error_dialog(&format!("Could not build the menu bar: {err}"));
                    return;
                }
            }
        };

        let window = WindowBuilder::new()
            .with_title("OpenResearch")
            .with_inner_size(LogicalSize::new(1280.0, 820.0))
            .with_min_inner_size(LogicalSize::new(720.0, 480.0))
            // Shown once the dashboard has loaded, so it never flashes blank.
            .with_visible(false);
        #[cfg(windows)]
        let window = {
            use tao::platform::windows::IconExtWindows;
            // Resource 1 is the icon build.rs embeds in orx.exe.
            window.with_window_icon(tao::window::Icon::from_resource(1, None).ok())
        };
        let window = match window.build(&event_loop) {
            Ok(window) => Rc::new(window),
            Err(err) => {
                crate::show_error_dialog(&format!("Could not open the window: {err}"));
                return;
            }
        };

        let shown = Rc::new(Cell::new(false));
        let webview = WebViewBuilder::new()
            .with_accept_first_mouse(true)
            // Lets the dashboard tell it is in the app, where pop-ups open in the browser.
            .with_initialization_script("window.__ORX_DESKTOP__ = true;")
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
                move |_url, path| choose_download_path(&window, path)
            })
            .with_document_title_changed_handler({
                let window = window.clone();
                move |title| window.set_title(&title)
            })
            .with_on_page_load_handler({
                let window = window.clone();
                let origin = origin.clone();
                let shown = shown.clone();
                move |event, url| {
                    if matches!(event, PageLoadEvent::Finished)
                        && super::is_dashboard_url(&url, &origin)
                        && !shown.replace(true)
                    {
                        window.set_visible(true);
                        window.set_focus();
                    }
                }
            })
            .build(&*window);
        let webview = match webview {
            Ok(webview) => webview,
            Err(err) => {
                crate::show_error_dialog(&format!("Could not open the dashboard view: {err}"));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        add_confirm_panel(&webview);

        let mut quit = Quit::No;
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
                    crate::commands::up::request_shutdown();
                    quit = Quit::ShuttingDown;
                    *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_secs(5));
                }
                Quit::ShuttingDown => *control_flow = ControlFlow::Exit,
            },
            Event::UserEvent(UserEvent::ServerReady) if matches!(quit, Quit::No) => {
                let _ = webview.load_url(&format!("{origin}/"));
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(id)) if id == quit_item.id() => {
                begin_quit(&mut quit, &window, &webview, control_flow);
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(id)) if id == reload_item.id() => {
                let _ = webview.reload();
            }
            // Before the first load the window shows itself; while quitting it
            // must stay hidden.
            #[cfg(windows)]
            Event::UserEvent(UserEvent::Focus) if shown.get() && matches!(quit, Quit::No) => {
                window.set_minimized(false);
                window.set_visible(true);
                window.set_focus();
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                // Closing hides, like other Mac apps; the Dock icon brings it back.
                #[cfg(target_os = "macos")]
                window.set_visible(false);
                #[cfg(windows)]
                begin_quit(&mut quit, &window, &webview, control_flow);
            }
            #[cfg(target_os = "macos")]
            Event::Reopen { .. } => {
                window.set_visible(true);
                window.set_focus();
            }
            _ => {}
        })
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

    /// Matches the Start menu shortcut's AppUserModelID, so the taskbar groups
    /// the window with the shortcut and pins the launcher rather than orx.exe.
    #[cfg(windows)]
    fn set_taskbar_identity() {
        use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

        let id = super::instance::wide("alphaXiv.OpenResearch");
        // SAFETY: the string is NUL-terminated and outlives the call.
        unsafe { SetCurrentProcessExplicitAppUserModelID(id.as_ptr()) };
    }

    #[cfg(target_os = "macos")]
    fn build_menu(quit: &MenuItem, reload: &MenuItem) -> muda::Result<Menu> {
        let about = AboutMetadata {
            name: Some("OpenResearch".into()),
            version: Some(env!("CARGO_PKG_VERSION").into()),
            ..Default::default()
        };
        Menu::with_items(&[
            &Submenu::with_items(
                "OpenResearch",
                true,
                &[
                    &PredefinedMenuItem::about(None, Some(about)),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::services(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::hide(None),
                    &PredefinedMenuItem::hide_others(None),
                    &PredefinedMenuItem::show_all(None),
                    &PredefinedMenuItem::separator(),
                    quit,
                ],
            )?,
            &Submenu::with_items(
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(None),
                    &PredefinedMenuItem::redo(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::cut(None),
                    &PredefinedMenuItem::copy(None),
                    &PredefinedMenuItem::paste(None),
                    &PredefinedMenuItem::select_all(None),
                ],
            )?,
            &Submenu::with_items(
                "View",
                true,
                &[
                    reload,
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::fullscreen(None),
                ],
            )?,
            &Submenu::with_items(
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(None),
                    &PredefinedMenuItem::maximize(None),
                    &PredefinedMenuItem::separator(),
                    &PredefinedMenuItem::close_window(None),
                ],
            )?,
        ])
    }

    /// Without a handler WKWebView drops downloads and WebView2 saves them
    /// silently into Downloads; ask where, as a browser would.
    fn choose_download_path(window: &Window, path: &mut PathBuf) -> bool {
        // Owned by the window, which Windows disables while the dialog is up.
        let mut dialog = rfd::FileDialog::new().set_parent(window);
        if let Some(dir) = path.parent() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = path.file_name() {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        match dialog.save_file() {
            Some(chosen) => {
                // Neither webview writes over an existing file, and the panel
                // has already confirmed replacing it.
                if chosen.exists() && std::fs::remove_file(&chosen).is_err() {
                    return false;
                }
                *path = chosen;
                true
            }
            None => false,
        }
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
}

#[cfg(test)]
mod tests {
    use super::{is_bundle_exe_launch, is_dashboard_url, opens_in_browser};
    use std::ffi::OsStr;
    use std::path::Path;

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
