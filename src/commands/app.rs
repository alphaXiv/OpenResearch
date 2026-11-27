//! Desktop app mode — the GUI entry point for the downloadable OpenResearch app.
//!
//! On macOS the `.app` bundle's executable IS the `orx` binary; launched from
//! Finder with no arguments, `main` routes here instead of parsing CLI args. On
//! Windows the installed `OpenResearch.exe` launcher (windows/launcher) starts
//! the `orx.exe` beside it as `orx app`, in a hidden console its children share;
//! on Linux the AppImage's AppRun (linux/AppRun) does the same. Either way app
//! mode owns the main thread with the window's run loop, and starts the dashboard
//! server as a child, `orx up --app-window`, so an update can replace the server
//! under a window that stays open. [FROZEN.md](app/FROZEN.md) is what the two
//! promise each other across builds.
//!
//! This is distinct from `orx up` launched in a terminal, which stays a plain
//! CLI. The GUI parts build only under `cfg(desktop_app)`: macOS, Windows, and
//! Linux with the `desktop` feature (see build.rs).

#[cfg(all(desktop_app, target_os = "linux"))]
pub mod media;

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
/// with, followed by nothing or the `openresearch:` link or `.orx` file that opened the app.
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

/// `None` unless `args` are `app` or `app <link or .orx file>`; then `Some` of the link, if any.
#[cfg_attr(not(all(desktop_app, not(target_os = "macos"))), allow(dead_code))]
fn app_launch(mut args: impl Iterator<Item = std::ffi::OsString>) -> Option<Option<String>> {
    if args.next()? != APP_ARG {
        return None;
    }
    let link = match args.next() {
        None => None,
        Some(arg) => match arg.to_str().and_then(app_link) {
            Some(link) => Some(link),
            // The launchers pass any `.orx` along; one the app cannot open still starts it.
            None if arg.to_string_lossy().to_ascii_lowercase().ends_with(".orx") => None,
            None => return None,
        },
    };
    args.next().is_none().then_some(link)
}

/// `arg` as a link the app opens: an `openresearch:` link, or a `.orx` file as a `file:` URL.
#[cfg_attr(not(all(desktop_app, not(target_os = "macos"))), allow(dead_code))]
fn app_link(arg: &str) -> Option<String> {
    let starts_with = |prefix: &str| {
        arg.get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    };
    if starts_with(LINK_SCHEME) {
        return Some(arg.to_string());
    }
    let url = if starts_with("file:") {
        url::Url::parse(arg).ok()?
    } else {
        url::Url::from_file_path(arg).ok()?
    };
    link_path(url.as_str()).map(|_| url.into())
}

/// The dock and app grid name and icon a window only through a desktop entry.
/// Rewritten whenever it points elsewhere; `TryExec` hides it once the file goes.
#[cfg(all(desktop_app, target_os = "linux"))]
fn install_desktop_entry() {
    let (Some(appimage), Some(data)) = (
        crate::updates::running_appimage(),
        crate::config::data_local_dir(),
    ) else {
        return;
    };
    let icon = data.join("icons/hicolor/256x256/apps/openresearch.png");
    let (Some(appimage), Some(icon_path)) = (appimage.to_str(), icon.to_str()) else {
        return;
    };
    let entry = desktop_entry(appimage, icon_path);
    let mime = data.join("mime");
    let registration_changed = || -> std::io::Result<bool> {
        write_if_changed(&icon, include_bytes!("../../linux/OpenResearch.png"))?;
        let file_type = write_if_changed(
            &mime.join("packages/openresearch.xml"),
            include_bytes!("../../linux/openresearch-mime.xml"),
        )?;
        let listed = write_if_changed(
            &data.join("applications/openresearch.desktop"),
            entry.as_bytes(),
        )?;
        Ok(file_type || listed)
    };
    match registration_changed() {
        // The entry's MimeType offers it; this makes it the handler for openresearch: links and .orx files.
        Ok(true) => {
            let mut update_mime = crate::process::command("update-mime-database");
            update_mime.arg(&mime);
            let mut handler = crate::process::command("xdg-mime");
            handler.args([
                "default",
                "openresearch.desktop",
                "x-scheme-handler/openresearch",
                "application/x-openresearch-project",
            ]);
            for command in [&mut update_mime, &mut handler] {
                crate::local::shell_env::restore_host_gui_env(command);
            }
            std::thread::spawn(move || {
                for mut command in [update_mime, handler] {
                    match command.output() {
                        Ok(out) if out.status.success() => {}
                        Ok(out) => eprintln!(
                            "openresearch app: could not register OpenResearch links and files: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        ),
                        Err(err) => eprintln!(
                            "openresearch app: could not register OpenResearch links and files: {err}"
                        ),
                    }
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

/// The user's choice when a close or quit finds work pending.
#[cfg_attr(not(desktop_app), allow(dead_code))]
#[derive(Clone, Copy)]
enum CloseAnswer {
    /// Keep working without the window: hidden on macOS, where the Dock still shows the app, minimized elsewhere.
    Background,
    Quit,
    Cancel,
}

/// The native alert's text, for when the page cannot ask.
#[cfg_attr(not(desktop_app), allow(dead_code))]
fn close_alert_description(work: &crate::store::PendingWork) -> String {
    use crate::store::AfterQuit;

    let mut lines = Vec::new();
    if work.blocks_quit() {
        lines.push("Force Quit stops the experiments that need OpenResearch open.".to_string());
    }
    let mut section = |heading: &str, items: Vec<String>| {
        if !items.is_empty() {
            lines.push(format!("\n{heading}"));
            lines.extend(items.into_iter().map(|item| format!("• {item}")));
        }
    };

    let agents = work.agents.iter().map(|agent| {
        let title = agent.title.as_deref().unwrap_or("Untitled");
        match &agent.project {
            Some(project) => format!("{title} ({project})"),
            None => title.to_string(),
        }
    });
    section("Agents stop when you quit:", agents.collect());
    let runs = |fate| work.runs.iter().filter(move |run| run.after_quit == fate);
    let needs_app = runs(AfterQuit::NeedsApp).map(|run| {
        let reason = match run.backend.as_deref() {
            Some("local_job") => "runs on this computer",
            Some("tinker_job") => "runs on this computer and trains on Tinker",
            Some("vercel_job") => {
                "without OpenResearch, the sandbox bills until its time limit and the log is lost"
            }
            Some("openresearch_job") => {
                "without OpenResearch, the result is lost when the box shuts down"
            }
            Some("boxd_job") => {
                "without OpenResearch, the machine keeps billing after the job ends, until OpenResearch deletes it"
            }
            _ => return run.name.clone(),
        };
        format!("{}: {reason}", run.name)
    });
    section(
        "Experiments that need OpenResearch open until they finish:",
        needs_app.collect(),
    );
    section(
        "Experiments that keep running and report when OpenResearch next opens:",
        runs(AfterQuit::Continues)
            .map(|run| run.name.clone())
            .collect(),
    );
    section(
        "Finished experiments whose results haven't been sent to their chat yet:",
        runs(AfterQuit::Reports)
            .map(|run| run.name.clone())
            .collect(),
    );
    lines.join("\n").trim_start().to_string()
}

#[cfg(desktop_app)]
static WINDOWLESS: std::sync::LazyLock<tokio::sync::watch::Sender<bool>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(false).0);

#[cfg(desktop_app)]
fn windowless() -> bool {
    *WINDOWLESS.borrow()
}

/// Whether a native quit goes through the window, which asks about pending work and has the
/// server stop its kept children. The system's own quit (logout, restart) cannot wait for it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn native_quit_goes_through_window(system_reason: Option<u32>) -> bool {
    system_reason.is_none()
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
    #[cfg(target_os = "linux")]
    let media_available = media::available().await;
    let discovered = match crate::commands::remote_host::canonical_data_dir() {
        Ok(data_dir) => crate::commands::remote_host::ServerDescriptor::live(&data_dir).await,
        Err(error) => Err(error),
    };
    match discovered {
        Ok(Some(server)) => {
            #[cfg(target_os = "macos")]
            imp::run_event_loop(server.port, None);
            #[cfg(not(target_os = "macos"))]
            imp::run_event_loop(
                server.port,
                None,
                #[cfg(target_os = "linux")]
                media_available,
                focus_requests,
                link.as_deref().and_then(link_path),
            );
            return;
        }
        Ok(None) => {}
        Err(error) => {
            crate::show_error_dialog(&error.to_string());
            return;
        }
    }
    // After the relaunch of an app from before the window outlived its server, keep its port,
    // as after a crash the port the agents it left running call. Otherwise a fixed port, since
    // the window's localStorage is keyed by origin, falling back to an ephemeral one when a
    // terminal `orx up` or another app holds it.
    let port_is_free = |port: &u16| std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok();
    let port = std::env::var(crate::updates::APP_RELAUNCH_PORT_ENV)
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
        .filter(port_is_free)
        .or_else(|| crate::commands::up::kept_agents_port().filter(port_is_free))
        .or_else(|| Some(APP_PORT).filter(port_is_free))
        .unwrap_or_else(|| {
            std::net::TcpListener::bind(("127.0.0.1", 0))
                .and_then(|l| l.local_addr())
                .map(|a| a.port())
                .unwrap_or(4791)
        });
    let server = start_server(port);
    #[cfg(target_os = "macos")]
    imp::run_event_loop(port, Some(server));
    #[cfg(not(target_os = "macos"))]
    imp::run_event_loop(
        port,
        Some(server),
        #[cfg(target_os = "linux")]
        media_available,
        focus_requests,
        link.as_deref().and_then(link_path),
    );
}

#[cfg(all(target_os = "macos", debug_assertions))]
pub fn run_dev(port: u16) {
    imp::run_event_loop(port, None);
}

/// Starts the server a window controls: `orx up --app-window`, with its stdin as the
/// window's lease (FROZEN.md). From the AppImage it starts through the image's `server`
/// entry, so it runs from a mount of its own that outlives this window's.
#[cfg(desktop_app)]
fn start_server(port: u16) -> std::io::Result<std::process::Child> {
    #[cfg(target_os = "linux")]
    let mut server = match crate::updates::running_appimage() {
        Some(appimage) => {
            let mut server = crate::process::command(appimage);
            // Or the image's AppRun would save this one's GTK settings as the session's.
            crate::local::shell_env::restore_host_gui_env(&mut server);
            server.arg("server");
            server
        }
        None => crate::process::command(crate::paths::spawnable_exe()?),
    };
    #[cfg(not(target_os = "linux"))]
    let mut server = crate::process::command(crate::paths::spawnable_exe()?);
    #[cfg(not(target_os = "macos"))]
    if std::env::var_os(APP_NO_TELEMETRY_ENV).is_some() {
        server.arg("--no-telemetry");
    }
    server
        .args(["up", "--app-window", "--port", &port.to_string()])
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
}

/// A request to the window's own server, through its control API (FROZEN.md).
#[cfg(desktop_app)]
async fn ask_server(
    port: u16,
    method: reqwest::Method,
    path: &str,
) -> anyhow::Result<reqwest::Response> {
    let response = crate::net::loopback_client()
        .build()?
        .request(method, format!("http://127.0.0.1:{port}{path}"))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await?;
    Ok(response.error_for_status()?)
}

/// Whether the server would interrupt something by quitting; true when it cannot say.
#[cfg(desktop_app)]
async fn server_busy(port: u16) -> bool {
    let busy = async {
        let state: serde_json::Value = ask_server(port, reqwest::Method::GET, "/api/app-window")
            .await?
            .json()
            .await?;
        anyhow::Ok(state["busy"].as_bool())
    };
    busy.await.ok().flatten().unwrap_or(true)
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
    let mut probe = crate::process::tokio_command(&shell);
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

/// Schemes a pop-up may hand to the system opener. A browser asks before
/// launching the app behind any other scheme (`ssh:`, `vscode:`); `open` would not.
/// Of Linear's links, only the team's draft issue, for a bug report.
#[cfg_attr(not(desktop_app), allow(dead_code))]
fn opens_in_browser(url: &str) -> bool {
    [
        "http:",
        "https:",
        "mailto:",
        "linear://alphaxiv/team/ORX/new?",
    ]
    .iter()
    .any(|scheme| url.starts_with(scheme))
}

/// The dashboard path an `openresearch://reproduce/<paper id>` link opens; the page validates the id.
/// A `.orx` file opened with the app, or an `openresearch://p/<slug>` link from a
/// published project's page, goes to the page that imports it.
#[cfg_attr(not(desktop_app), allow(dead_code))]
fn deep_link_path(url: &url::Url) -> Option<String> {
    if url.scheme() == "file" {
        let file = url.to_file_path().ok()?;
        let shared = file.extension().is_some_and(|extension| {
            extension.eq_ignore_ascii_case(crate::local::share::EXTENSION)
        });
        return shared.then(|| {
            format!(
                "/open?file={}",
                urlencoding::encode(&file.to_string_lossy())
            )
        });
    }
    let path = url.path();
    if url.scheme() == "openresearch" && url.host_str() == Some("p") {
        // Only the slug crosses over; the app fetches the file from its own API.
        let slug = crate::client::published_project_slug(path.trim_start_matches('/'))?;
        return Some(format!("/open?shared={slug}"));
    }
    if url.scheme() != "openresearch" || url.host_str() != Some("reproduce") || path.len() < 2 {
        return None;
    }
    Some(format!("/reproduce{path}"))
}

/// [`deep_link_path`] for a link that arrived as text, on the command line or from a second launch.
#[cfg_attr(not(desktop_app), allow(dead_code))]
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
        link.and_then(|link| super::app_link(&link))
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
        let link_dir = crate::config::data_local_dir().map(|dir| dir.join("OpenResearch"));
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
        let dir = crate::config::cache_dir()?.join("openresearch");
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
    ) -> std::io::Result<Option<crate::file_lock::FileLock>> {
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(crate::file_lock::FileLock::new(file))),
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
        super::app_link(&link)
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
    #[cfg(not(target_os = "linux"))]
    use tao::window::Theme;
    use tao::window::{Window, WindowBuilder};
    use wry::{NewWindowResponse, PageLoadEvent, WebView, WebViewBuilder};

    #[cfg(target_os = "macos")]
    use muda::{
        accelerator::Accelerator, AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem,
        MenuItemKind, PredefinedMenuItem, Submenu,
    };
    #[cfg(target_os = "macos")]
    use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol, Sel};
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

    enum UserEvent {
        ServerReady,
        EngineBusy(bool),
        LoadTimedOut,
        SyncZoom,
        Zoom(ZoomAction),
        #[cfg(target_os = "macos")]
        Menu(MenuId),
        #[cfg(target_os = "macos")]
        MenuSpec(Vec<PageMenu>),
        #[cfg(target_os = "macos")]
        MenuState(HashMap<String, PageMenuState>),
        /// Bring the window forward, opening the `openresearch:` link a later launch carried.
        Focus(Option<String>),
        /// The dashboard page finished loading and can take `orx:open-path`.
        PageReady,
        /// A close or quit that did not come through the window's own close event.
        Close,
        /// The page is showing the dialog a close asked it for.
        CloseAsked,
        CloseAnswer(super::CloseAnswer),
        /// The window's server exited, and no successor took over from it.
        ServerExited(ServerStop),
        /// No page answered for the stopped server, so a native alert asks.
        AskAfterStop(ServerStop),
        /// The alert's answer: true to start the server again, false to quit.
        StopAnswer(bool),
        /// The page asks to start the server again after it stopped.
        StartServer,
        /// The page is showing the notice that the server stopped.
        StoppedAsked,
        /// Tell a newly loaded page whether the window is maximized (Windows) or fullscreen (macOS).
        #[cfg(any(windows, target_os = "macos"))]
        SyncWindowState,
        /// The page asks for a screenshot of itself on the clipboard, for a bug report.
        #[cfg(target_os = "macos")]
        CopyScreenshot,
        /// The screenshot is on the clipboard, or why it isn't.
        #[cfg(target_os = "macos")]
        ScreenshotCopied(Result<(), String>),
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
        /// The page unloads, which sends its last workspace state to the server.
        Flushing,
        /// The server stops its runs and agents and exits; the window waits for it, for a while.
        ShuttingDown,
    }

    /// How long a quitting window waits for its server: the runs it stops get ten seconds, the agents five.
    const SERVER_QUIT_WAIT: Duration = Duration::from_secs(16);

    /// The port of the server this window started, for AppKit's quit callback, which has no context.
    static SERVER_PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();

    /// A close with work pending, waiting on the user's choice.
    enum Ask {
        No,
        /// Until the page says it is asking; after the deadline a native alert asks instead.
        Sent(Instant),
        Page,
        /// A native alert asks.
        Alert,
        /// A windowless close brought the window back, so the page can list the work once it loads;
        /// past the deadline a native alert asks instead.
        #[cfg(target_os = "macos")]
        OnLoad(Instant),
    }

    pub(super) fn run_event_loop(
        port: u16,
        server: Option<std::io::Result<std::process::Child>>,
        #[cfg(target_os = "linux")] media_available: bool,
        #[cfg(not(target_os = "macos"))] focus_requests: super::instance::FocusRequests,
        #[cfg(not(target_os = "macos"))] launch_path: Option<String>,
    ) {
        let external_server = server.is_none();
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
        EVENT_PROXY.with(|slot| *slot.borrow_mut() = Some(event_loop.create_proxy()));
        #[cfg(target_os = "macos")]
        if !external_server {
            intercept_native_quit();
        }

        if let Some(server) = server {
            let _ = SERVER_PORT.set(port);
            match server {
                Ok(server) => watch_server(server, event_loop.create_proxy()),
                Err(error) => {
                    let _ = event_loop
                        .create_proxy()
                        .send_event(UserEvent::ServerExited(ServerStop::not_started(&error)));
                }
            }
        }

        if !external_server {
            let lifecycle = event_loop.create_proxy();
            let mut visibility = super::WINDOWLESS.subscribe();
            tokio::spawn(async move {
                loop {
                    while !*visibility.borrow_and_update() {
                        if visibility.changed().await.is_err() {
                            return;
                        }
                        let _ = lifecycle.send_event(UserEvent::EngineBusy(true));
                    }
                    let busy = super::server_busy(port).await;
                    if lifecycle.send_event(UserEvent::EngineBusy(busy)).is_err() {
                        return;
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(10)) => {}
                        result = visibility.changed() => {
                            if result.is_err() { return; }
                            let _ = lifecycle.send_event(UserEvent::EngineBusy(true));
                        }
                    }
                }
            });
        }

        let ready = event_loop.create_proxy();
        tokio::spawn(async move {
            serving(external_server, port).await;
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
                    _ if event.id.as_ref() == "app.quit" => UserEvent::Close,
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
                // `align_traffic_lights` measures and corrects this once the window exists.
                .with_traffic_light_inset(tao::dpi::LogicalPosition::new(
                    TRAFFIC_LIGHTS_X,
                    2.0 + TRAFFIC_LIGHTS_CENTER_Y,
                ))
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

        // The page leaves the sidebar transparent over the window's material where there is one.
        #[cfg(target_os = "macos")]
        let material = true;
        #[cfg(windows)]
        let material = add_window_material(&window);
        #[cfg(target_os = "linux")]
        let material = false;

        let shown = Rc::new(Cell::new(false));
        let page_ready = Rc::new(Cell::new(false));
        let replaced = Rc::new(RefCell::new(HashMap::new()));
        let builder = WebViewBuilder::new()
            .with_accept_first_mouse(true)
            .with_transparent(material)
            // Lets the dashboard tell it is in the app, where pop-ups open in the browser.
            .with_initialization_script("window.__ORX_DESKTOP__ = true;")
            .with_initialization_script_for_main_only(
                include_str!("desktop_zoom.js")
                    .replace("__ORX_ZOOM_PLATFORM__", std::env::consts::OS),
                false,
            )
            // Pop-ups go to the browser, dashboard pages included: a window wry
            // opens itself has none of this window's handlers.
            .with_new_window_req_handler({
                let window = window.clone();
                move |url, _features| {
                    open_outside(&window, &url);
                    NewWindowResponse::Deny
                }
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
                    #[cfg(any(windows, target_os = "macos"))]
                    let _ = proxy.send_event(UserEvent::SyncWindowState);
                    if super::is_dashboard_url(&url, &origin)
                        && !super::windowless()
                        && !shown.replace(true)
                    {
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
            #[cfg(target_os = "macos")]
            let origin = origin.clone();
            let proxy = event_loop.create_proxy();
            move |request| {
                let message = request.body().as_str();
                // Frames of other origins, such as an artifact's preview, reach the handler too.
                #[cfg(target_os = "macos")]
                let from_dashboard = super::is_dashboard_url(&request.uri().to_string(), &origin);
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
                    // The page finished a browser sign-in and asks to come back.
                    "window:focus" => {
                        let _ = proxy.send_event(UserEvent::Focus(None));
                    }
                    "close:asking" => {
                        let _ = proxy.send_event(UserEvent::CloseAsked);
                    }
                    "server:start" => {
                        let _ = proxy.send_event(UserEvent::StartServer);
                    }
                    "server:asking" => {
                        let _ = proxy.send_event(UserEvent::StoppedAsked);
                    }
                    "app:quit" => {
                        let _ = proxy.send_event(UserEvent::Close);
                    }
                    #[cfg(target_os = "macos")]
                    "report:screenshot" if from_dashboard => {
                        let _ = proxy.send_event(UserEvent::CopyScreenshot);
                    }
                    // A pop-up the page opens after an await, when WebKit no longer counts the click.
                    #[cfg(target_os = "macos")]
                    message if from_dashboard && message.starts_with("open:") => {
                        open_outside(&window, &message["open:".len()..]);
                    }
                    "close:background" => {
                        let _ = proxy
                            .send_event(UserEvent::CloseAnswer(super::CloseAnswer::Background));
                    }
                    "close:quit" => {
                        let _ = proxy.send_event(UserEvent::CloseAnswer(super::CloseAnswer::Quit));
                    }
                    "close:cancel" => {
                        let _ =
                            proxy.send_event(UserEvent::CloseAnswer(super::CloseAnswer::Cancel));
                    }
                    #[cfg(not(target_os = "linux"))]
                    "titlebar:drag" => {
                        let _ = window.drag_window();
                    }
                    #[cfg(not(target_os = "linux"))]
                    "titlebar:zoom" => window.set_maximized(!window.is_maximized()),
                    // The window material and macOS traffic lights follow the page's theme preference.
                    #[cfg(not(target_os = "linux"))]
                    "appearance:system" => window.set_theme(None),
                    #[cfg(not(target_os = "linux"))]
                    "appearance:light" => window.set_theme(Some(Theme::Light)),
                    #[cfg(not(target_os = "linux"))]
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
            .with_initialization_script_for_main_only(include_str!("macos_escape.js"), false);
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
        let builder = if material {
            builder.with_initialization_script("window.__ORX_WINDOW_MATERIAL__ = true;")
        } else {
            builder
        };
        #[cfg(target_os = "linux")]
        let builder = if media_available {
            builder
        } else {
            builder.with_initialization_script("window.__ORX_MEDIA_DISABLED__ = true;")
        };
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
        #[cfg(target_os = "linux")]
        {
            use webkit2gtk::{SettingsExt, WebViewExt};
            use wry::WebViewExtUnix;

            if let Some(settings) = webview.webview().settings() {
                if !media_available {
                    settings.set_enable_media(false);
                    settings.set_enable_webaudio(false);
                    settings.set_enable_mediasource(false);
                }
                // Remote workspace pop-ups follow an async request, after the click's user gesture expires.
                settings.set_javascript_can_open_windows_automatically(true);
            }
        }
        #[cfg(target_os = "macos")]
        add_window_material(&webview);

        let mut zoom_percent = crate::config::desktop_zoom_percent();
        if let Err(error) = sync_page_zoom(&webview, zoom_percent, false) {
            eprintln!("openresearch app: could not restore page zoom: {error}");
        }
        #[cfg(target_os = "macos")]
        align_traffic_lights(&window, zoom_percent);
        let mut quit = Quit::No;
        let mut ask = Ask::No;
        let mut idle = crate::commands::up::api_v2::Idle::default();
        let started = Instant::now();
        let mut server_running = !external_server;
        // Until when the page has to say it shows the stopped server's notice.
        let mut stopped_asked: Option<(Instant, ServerStop)> = None;
        let proxy = event_loop.create_proxy();
        let mut links = DeepLinks {
            server_ready: false,
            page_ready,
            #[cfg(target_os = "macos")]
            pending: None,
            #[cfg(not(target_os = "macos"))]
            pending: launch_path,
        };
        // What the loaded page was last told; a new page starts unmaximized and windowed.
        #[cfg(windows)]
        let mut page_maximized = false;
        #[cfg(target_os = "macos")]
        let mut page_fullscreen = false;
        event_loop.run(move |event, _, control_flow| match event {
            Event::NewEvents(StartCause::Init) => {
                *control_flow = ControlFlow::Wait;
                #[cfg(target_os = "macos")]
                menu.init_for_nsapp();
            }
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) => match quit {
                Quit::No => {
                    let deadline = match ask {
                        Ask::Sent(deadline) => Some(deadline),
                        #[cfg(target_os = "macos")]
                        Ask::OnLoad(deadline) => Some(deadline),
                        _ => None,
                    };
                    if deadline.is_some_and(|deadline| deadline <= Instant::now()) {
                        ask = Ask::No;
                        *control_flow = ControlFlow::Wait;
                        close_window(external_server, false, &mut quit, &mut ask, &window, &webview, control_flow);
                    }
                    if stopped_asked.as_ref().is_some_and(|(deadline, _)| *deadline <= Instant::now()) {
                        if let Some((_, stop)) = stopped_asked.take() {
                            let _ = proxy.send_event(UserEvent::AskAfterStop(stop));
                        }
                    }
                }
                Quit::Flushing => {
                    if external_server || !server_running {
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                    // Past the wait, leaving drops the lease, and the server hands its work off instead.
                    tokio::spawn(async move {
                        if let Err(error) = super::ask_server(port, reqwest::Method::POST, "/api/app-window/quit").await {
                            eprintln!("openresearch app: could not ask the server to quit: {error:#}");
                        }
                    });
                    quit = Quit::ShuttingDown;
                    *control_flow = ControlFlow::WaitUntil(Instant::now() + SERVER_QUIT_WAIT);
                }
                Quit::ShuttingDown => *control_flow = ControlFlow::Exit,
            },
            Event::UserEvent(UserEvent::EngineBusy(value)) if matches!(quit, Quit::No) => {
                if idle.should_exit(started.elapsed(), !super::windowless() || value) {
                    begin_quit(&mut quit, &window, &webview, control_flow);
                }
            }
            // A page that started the server again reloads itself once it answers.
            Event::UserEvent(UserEvent::ServerReady) if matches!(quit, Quit::No) && !links.server_ready => {
                if super::windowless() {
                    links.server_ready = true;
                    return;
                }
                let _ = webview.load_url(&format!("{origin}{}", links.on_server_ready()));
            }
            // A window that never appears is worse than a blank one.
            Event::UserEvent(UserEvent::LoadTimedOut)
                if matches!(quit, Quit::No) && !super::windowless() && !shown.replace(true) =>
            {
                eprintln!("openresearch app: the dashboard has not finished loading; showing the window anyway");
                window.set_visible(true);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::Close) => {
                let page_ready = links.page_ready.get();
                close_window(external_server, page_ready, &mut quit, &mut ask, &window, &webview, control_flow);
            }
            Event::UserEvent(UserEvent::CloseAsked)
                if matches!(quit, Quit::No) && matches!(ask, Ask::Sent(_)) =>
            {
                ask = Ask::Page;
                *control_flow = ControlFlow::Wait;
            }
            Event::UserEvent(UserEvent::CloseAnswer(answer))
                if matches!(quit, Quit::No) && !matches!(ask, Ask::No) =>
            {
                ask = Ask::No;
                *control_flow = ControlFlow::Wait;
                answer_close(answer, &mut quit, &window, &webview, control_flow);
            }
            Event::UserEvent(UserEvent::ServerExited(stop)) => {
                server_running = false;
                if !matches!(quit, Quit::No) {
                    *control_flow = ControlFlow::Exit;
                } else if links.page_ready.get() && !super::windowless() {
                    let detail = serde_json::to_string(&stop).unwrap_or_else(|_| "null".into());
                    let _ = webview.evaluate_script(&format!(
                        "window.dispatchEvent(new CustomEvent('orx:server-stopped', {{ detail: {detail} }}));"
                    ));
                    let deadline = Instant::now() + Duration::from_secs(1);
                    stopped_asked = Some((deadline, stop));
                    *control_flow = ControlFlow::WaitUntil(deadline);
                } else {
                    let _ = proxy.send_event(UserEvent::AskAfterStop(stop));
                }
            }
            Event::UserEvent(UserEvent::StoppedAsked) => stopped_asked = None,
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::CopyScreenshot) => copy_screenshot(&webview, proxy.clone()),
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::ScreenshotCopied(copied)) => {
                if let Err(error) = &copied {
                    eprintln!("openresearch app: could not copy a screenshot: {error}");
                }
                let detail = serde_json::json!({ "error": copied.err() });
                let _ = webview.evaluate_script(&format!(
                    "window.dispatchEvent(new CustomEvent('orx:screenshot', {{ detail: {detail} }}));"
                ));
            }
            Event::UserEvent(UserEvent::AskAfterStop(stop)) if !server_running && matches!(quit, Quit::No) => {
                // The alert is a sheet on the window, which comes back for it.
                #[cfg(target_os = "macos")]
                {
                    super::WINDOWLESS.send_replace(false);
                    shown.set(true);
                    window.set_visible(true);
                }
                ask_after_stop(&window, &stop);
            }
            Event::UserEvent(UserEvent::StopAnswer(restart)) if !server_running && matches!(quit, Quit::No) => {
                if !restart {
                    *control_flow = ControlFlow::Exit;
                    return;
                }
                // The window loads the page it showed once the server answers, and until then
                // holds the links and reopens that would load one.
                let current = links.page_ready.get().then(|| webview.url().ok()).flatten();
                links.pending = current
                    .as_deref()
                    .and_then(|url| url.strip_prefix(origin.as_str()))
                    .filter(|path| path.starts_with('/'))
                    .map(str::to_owned)
                    .or(links.pending.take());
                links.server_ready = false;
                let _ = proxy.send_event(UserEvent::StartServer);
            }
            Event::UserEvent(UserEvent::StartServer) if !server_running && matches!(quit, Quit::No) => {
                match super::start_server(port) {
                    Ok(server) => {
                        server_running = true;
                        watch_server(server, proxy.clone());
                        let ready = proxy.clone();
                        tokio::spawn(async move {
                            serving(false, port).await;
                            let _ = ready.send_event(UserEvent::ServerReady);
                        });
                    }
                    Err(error) => {
                        let _ = proxy.send_event(UserEvent::ServerExited(ServerStop::not_started(&error)));
                    }
                }
            }
            #[cfg(windows)]
            Event::UserEvent(UserEvent::SyncWindowState) => {
                page_maximized = window.is_maximized();
                sync_root_flag(&webview, "data-maximized", page_maximized);
            }
            #[cfg(windows)]
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } if window.is_maximized() != page_maximized => {
                page_maximized = !page_maximized;
                sync_root_flag(&webview, "data-maximized", page_maximized);
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::SyncWindowState) => {
                page_fullscreen = window.fullscreen().is_some();
                sync_root_flag(&webview, "data-fullscreen", page_fullscreen);
            }
            #[cfg(target_os = "macos")]
            Event::WindowEvent {
                event: WindowEvent::Resized(_),
                ..
            } => {
                if super::windowless() && !in_native_fullscreen(&window) {
                    window.set_visible(false);
                }
                align_traffic_lights(&window, zoom_percent);
                if window.fullscreen().is_some() != page_fullscreen {
                    page_fullscreen = !page_fullscreen;
                    sync_root_flag(&webview, "data-fullscreen", page_fullscreen);
                }
            }
            #[cfg(target_os = "macos")]
            Event::UserEvent(UserEvent::Menu(id)) if id.as_ref() == "window.close" => {
                let page_ready = links.page_ready.get();
                close_window(external_server, page_ready, &mut quit, &mut ask, &window, &webview, control_flow);
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
            Event::UserEvent(UserEvent::Focus(link)) if matches!(quit, Quit::No) => {
                if super::WINDOWLESS.send_replace(false) {
                    shown.set(false);
                    links.reopen(&webview, &origin, link.as_deref().and_then(super::link_path));
                    return;
                }
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
                let page_ready = links.page_ready.get();
                close_window(external_server, page_ready, &mut quit, &mut ask, &window, &webview, control_flow);
            }
            #[cfg(target_os = "macos")]
            Event::Reopen { .. } if matches!(quit, Quit::No) => {
                if super::WINDOWLESS.send_replace(false) {
                    shown.set(false);
                    links.reopen(&webview, &origin, None);
                    return;
                }
                shown.set(true);
                window.set_visible(true);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::PageReady) if matches!(quit, Quit::No) && !super::windowless() => {
                links.on_page_ready(&webview);
                #[cfg(target_os = "macos")]
                if matches!(ask, Ask::OnLoad(_)) {
                    ask = Ask::No;
                    close_window(external_server, true, &mut quit, &mut ask, &window, &webview, control_flow);
                }
            }
            #[cfg(target_os = "macos")]
            Event::Opened { urls } if matches!(quit, Quit::No) => {
                // Before the first page shows, the page-load handler reveals the window.
                if shown.get() {
                    window.set_minimized(false);
                    window.set_visible(true);
                    window.set_focus();
                }
                let Some(path) = urls.iter().find_map(super::deep_link_path) else {
                    return;
                };
                if super::WINDOWLESS.send_replace(false) {
                    shown.set(false);
                    links.reopen(&webview, &origin, Some(path));
                } else {
                    links.open(path, &webview, &origin);
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

        fn reopen(&mut self, webview: &WebView, origin: &str, path: Option<String>) {
            self.page_ready.set(false);
            if let Some(path) = path {
                self.pending = Some(path);
            }
            if self.server_ready {
                let path = self.pending.take().unwrap_or_else(|| "/".to_string());
                let _ = webview.load_url(&format!("{origin}{path}"));
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

    thread_local! {
        static EVENT_PROXY: RefCell<Option<tao::event_loop::EventLoopProxy<UserEvent>>> = const { RefCell::new(None) };
    }

    /// For code on the main thread with no proxy of its own: AppKit's callbacks, alerts and a windowless close.
    fn send_later(event: UserEvent) -> bool {
        EVENT_PROXY
            .with(|slot| slot.borrow().clone())
            .is_some_and(|proxy| proxy.send_event(event).is_ok())
    }

    /// Why the window's server stopped, as its stderr and exit tell (FROZEN.md).
    #[derive(Debug, serde::Serialize)]
    struct ServerStop {
        /// The server's own message for the user, from an exit it reported as an error.
        reason: Option<String>,
        /// The end of its stderr, then how it exited.
        log: String,
    }

    impl ServerStop {
        /// From the end of the server's stderr and its exit code and description, when known.
        fn read(mut lines: Vec<String>, exit: Option<(Option<i32>, String)>) -> Self {
            // Exit status 1 is orx's for an error it reported, as its last line; the dialog
            // shows that line above the log, so the log leaves it out.
            let mut reason = None;
            if exit.as_ref().is_some_and(|(code, _)| *code == Some(1)) {
                if let Some(last) = lines.iter().rposition(|line| !line.trim().is_empty()) {
                    reason = Some(lines.remove(last));
                }
            }
            lines.extend(exit.map(|(_, exit)| exit));
            Self {
                reason,
                log: lines.join("\n"),
            }
        }

        fn not_started(error: &std::io::Error) -> Self {
            let reason = format!("Could not start OpenResearch's server: {error}");
            eprintln!("openresearch app: {reason}");
            Self {
                reason: Some(reason),
                log: String::new(),
            }
        }
    }

    /// Until the server at `port` takes connections.
    async fn serving(external_server: bool, port: u16) {
        let host = if external_server {
            "localhost"
        } else {
            "127.0.0.1"
        };
        while tokio::net::TcpStream::connect((host, port)).await.is_err() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The lines of the server's stderr kept for [`ServerStop::log`].
    const STOP_LOG_LINES: usize = 40;

    #[derive(Default)]
    struct StderrTail {
        lines: std::collections::VecDeque<String>,
        closed: bool,
    }

    /// Passes the server's stderr on to the window's own, keeping its last lines.
    fn read_stderr(
        stderr: std::process::ChildStderr,
    ) -> std::sync::Arc<(std::sync::Mutex<StderrTail>, std::sync::Condvar)> {
        use std::io::{BufRead as _, Write as _};

        let tail = std::sync::Arc::new((
            std::sync::Mutex::new(StderrTail::default()),
            std::sync::Condvar::new(),
        ));
        let shared = tail.clone();
        std::thread::spawn(move || {
            let (kept, eof) = &*shared;
            let mut reader = std::io::BufReader::new(stderr);
            let mut line = Vec::new();
            while reader
                .read_until(b'\n', &mut line)
                .is_ok_and(|read| read > 0)
            {
                let _ = std::io::stderr().write_all(&line);
                let mut tail = kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if tail.lines.len() == STOP_LOG_LINES {
                    tail.lines.pop_front();
                }
                tail.lines
                    .push_back(String::from_utf8_lossy(&line).trim_end().to_owned());
                line.clear();
            }
            kept.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .closed = true;
            eof.notify_all();
        });
        tail
    }

    /// Waits on the window's server and, on Windows, on each successor it restarts into,
    /// which inherits the lease and stderr, until the last one exits.
    fn watch_server(
        mut server: std::process::Child,
        proxy: tao::event_loop::EventLoopProxy<UserEvent>,
    ) {
        // Held while any server it was handed to runs: `wait` would close it first.
        let lease = server.stdin.take();
        let stderr = server.stderr.take().map(read_stderr);
        #[cfg(windows)]
        let runtime = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let _lease = lease;
            #[cfg(windows)]
            let status = follow_server(&server, &runtime);
            #[cfg(not(windows))]
            let status = match server.wait() {
                Ok(status) => {
                    eprintln!("openresearch app: the server exited: {status}");
                    Some((status.code(), status.to_string()))
                }
                Err(error) => {
                    eprintln!("openresearch app: could not wait for the server: {error}");
                    None
                }
            };

            let mut lines = Vec::new();
            if let Some(shared) = stderr {
                let (kept, eof) = &*shared;
                let tail = kept.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                // What the server wrote before it exited is in the pipe already; a process it
                // started may hold the pipe open past that.
                let (tail, _) = eof
                    .wait_timeout_while(tail, Duration::from_secs(1), |tail| !tail.closed)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                lines.extend(tail.lines.iter().cloned());
            }
            let _ = proxy.send_event(UserEvent::ServerExited(ServerStop::read(lines, status)));
        });
    }

    /// Windows has no exec, so a server restarts by starting its successor, naming it in
    /// `relaunch.json` and exiting 0; the successor inherits the lease. Waits out the server and
    /// each successor in turn, returning the last one's exit. A successor is opened as soon as
    /// it is named, while the server that started it still holds it, so one that fails at once
    /// still reports how.
    #[cfg(windows)]
    fn follow_server(
        server: &std::process::Child,
        runtime: &tokio::runtime::Handle,
    ) -> Option<(Option<i32>, String)> {
        use crate::commands::remote_host::{canonical_data_dir, runtime_path, ServerDescriptor};
        use std::os::windows::{ffi::OsStrExt as _, io::AsRawHandle as _};
        use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
        use windows_sys::Win32::Storage::FileSystem::{
            FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
            FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
        };
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, WaitForMultipleObjects, INFINITE,
        };

        let data_dir = canonical_data_dir()
            .map_err(|error| {
                eprintln!("openresearch app: could not look for a restarted server: {error:#}")
            })
            .ok();
        let relaunch = data_dir
            .as_deref()
            .and_then(|data_dir| runtime_path(data_dir, "relaunch.json").ok());
        let runtime_dir = relaunch.as_deref().and_then(|path| {
            Some(
                path.parent()?
                    .as_os_str()
                    .encode_wide()
                    .chain(Some(0))
                    .collect::<Vec<_>>(),
            )
        });
        // SAFETY: a NUL-terminated path; the handle is closed below.
        let changes = runtime_dir
            .map(|dir| unsafe {
                FindFirstChangeNotificationW(
                    dir.as_ptr(),
                    0,
                    FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE,
                )
            })
            .filter(|changes| *changes != INVALID_HANDLE_VALUE);
        // An announcement outlives a successor that failed before taking over, so one naming a
        // server already followed, or one gone, is stale.
        let announced = |followed: &[u32]| {
            ServerDescriptor::announced_successor(data_dir.as_deref()?)
                .filter(|successor| !followed.contains(successor))
        };

        let mut pid = server.id();
        let mut process = match open_process(pid) {
            Ok(process) => process,
            Err(error) => {
                eprintln!("openresearch app: could not wait for the server: {error}");
                return None;
            }
        };
        let mut followed = vec![pid];
        let mut named = None;
        let status = loop {
            // An announcement first: the wait reports the lowest signaled handle.
            let mut handles: Vec<_> = changes.into_iter().collect();
            handles.push(process.as_raw_handle());
            // SAFETY: every handle stays open across the wait.
            let woke = unsafe {
                WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, INFINITE)
            };
            if let Some(changes) = changes.filter(|_| woke == WAIT_OBJECT_0) {
                // SAFETY: the notification handle opened above.
                unsafe { FindNextChangeNotification(changes) };
                if named.is_none() {
                    named = announced(&followed).and_then(|successor| {
                        open_successor(successor)?
                            .map_err(|error| {
                                eprintln!("openresearch app: could not follow the restarted server {successor}: {error}")
                            })
                            .ok()
                    });
                }
                continue;
            }
            let mut code = 0;
            // SAFETY: the process handle is open and, after the wait, signaled.
            if woke != WAIT_OBJECT_0 + handles.len() as u32 - 1
                || unsafe { GetExitCodeProcess(process.as_raw_handle(), &mut code) } == 0
            {
                eprintln!(
                    "openresearch app: could not wait for the server {pid}: {}",
                    std::io::Error::last_os_error()
                );
                break None;
            }
            let status = {
                use std::os::windows::process::ExitStatusExt as _;
                std::process::ExitStatus::from_raw(code)
            };
            eprintln!("openresearch app: the server {pid} exited: {status}");
            let exit = Some((Some(code as i32), status.to_string()));
            if code != 0 {
                break exit;
            }
            let successor = match named.take() {
                Some(successor) => Ok(successor),
                // A successor quick enough to have started already replaced the announcement with its descriptor.
                None => match announced(&followed).and_then(open_successor).or_else(|| {
                    open_successor(live_successor(runtime, data_dir.as_deref()?, &followed)?)
                }) {
                    Some(successor) => successor,
                    None => break exit,
                },
            };
            match successor {
                Ok(successor) => {
                    (pid, process) = successor;
                    followed.push(pid);
                }
                Err(error) => {
                    eprintln!("openresearch app: could not follow the restarted server: {error}");
                    break None;
                }
            }
        };
        if let Some(changes) = changes {
            // SAFETY: the notification handle opened above, closed once.
            unsafe { FindCloseChangeNotification(changes) };
        }
        status
    }

    /// The server that has taken over from the one that exited, if one has.
    #[cfg(windows)]
    fn live_successor(
        runtime: &tokio::runtime::Handle,
        data_dir: &std::path::Path,
        followed: &[u32],
    ) -> Option<u32> {
        match runtime.block_on(crate::commands::remote_host::ServerDescriptor::live(
            data_dir,
        )) {
            Ok(server) => server
                .map(|server| server.pid)
                .filter(|pid| !followed.contains(pid)),
            Err(error) => {
                eprintln!("openresearch app: could not look for a restarted server: {error:#}");
                None
            }
        }
    }

    /// The successor `pid` and a handle to it, or `None` once no process has that pid.
    #[cfg(windows)]
    fn open_successor(
        pid: u32,
    ) -> Option<std::io::Result<(u32, std::os::windows::io::OwnedHandle)>> {
        match open_process(pid) {
            Ok(process) => Some(Ok((pid, process))),
            // ERROR_INVALID_PARAMETER: no process has this pid.
            Err(error) if error.raw_os_error() == Some(87) => None,
            Err(error) => Some(Err(error)),
        }
    }

    /// A handle to wait on the process `pid` and read its exit code.
    #[cfg(windows)]
    fn open_process(pid: u32) -> std::io::Result<std::os::windows::io::OwnedHandle> {
        use std::os::windows::io::FromRawHandle as _;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };

        // SAFETY: a plain syscall; the handle it returns is owned from here on.
        let process = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if process.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a valid handle no one else owns.
        Ok(unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(process) })
    }

    #[cfg(target_os = "macos")]
    fn intercept_native_quit() {
        let app = NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
        unsafe {
            let delegate: *mut AnyObject = objc2::msg_send![&*app, delegate];
            let Some(delegate) = delegate.as_ref() else {
                return;
            };
            let class: *const AnyClass = delegate.class();
            type ShouldTerminate =
                unsafe extern "C-unwind" fn(
                    &AnyObject,
                    Sel,
                    &AnyObject,
                )
                    -> objc2_app_kit::NSApplicationTerminateReply;
            let handler: ShouldTerminate = native_quit_requested;
            objc2::ffi::class_addMethod(
                class.cast_mut(),
                objc2::sel!(applicationShouldTerminate:),
                std::mem::transmute::<ShouldTerminate, objc2::runtime::Imp>(handler),
                c"Q@:@".as_ptr(),
            );
            // AppKit caches optional methods when the delegate is assigned.
            let _: () = objc2::msg_send![&*app, setDelegate: delegate];
        }
    }

    #[cfg(target_os = "macos")]
    unsafe extern "C-unwind" fn native_quit_requested(
        _delegate: &AnyObject,
        _selector: Sel,
        _app: &AnyObject,
    ) -> objc2_app_kit::NSApplicationTerminateReply {
        use objc2_app_kit::NSApplicationTerminateReply;
        use objc2_foundation::NSAppleEventManager;

        let reason = NSAppleEventManager::sharedAppleEventManager()
            .currentAppleEvent()
            .and_then(|event| {
                // AEKeyword is u32; the generated binding otherwise requires CoreServices.
                let reason: Option<objc2::rc::Retained<objc2_foundation::NSAppleEventDescriptor>> = unsafe {
                    objc2::msg_send![&*event, paramDescriptorForKeyword: u32::from_be_bytes(*b"why?")]
                };
                reason
            })
            .map(|reason| reason.enumCodeValue());
        if !super::native_quit_goes_through_window(reason) {
            return NSApplicationTerminateReply::TerminateNow;
        }
        // The answer may come from the page, which cannot run while AppKit waits on a deferred reply.
        if send_later(UserEvent::Close) {
            NSApplicationTerminateReply::TerminateCancel
        } else {
            NSApplicationTerminateReply::TerminateNow
        }
    }

    /// The work a quit would interrupt, as the window's server lists it; none when it can't
    /// say, which a developer has to fix. Kept whole for the page, which the server serves.
    fn pending_work() -> Option<serde_json::Value> {
        let port = *SERVER_PORT.get()?;
        let read = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let work: serde_json::Value =
                    super::ask_server(port, reqwest::Method::GET, "/api/pending-work")
                        .await?
                        .json()
                        .await?;
                anyhow::Ok(work)
            })
        });
        match read {
            Ok(work) => {
                let empty = |list: &str| work[list].as_array().is_some_and(Vec::is_empty);
                (!(empty("agents") && empty("runs"))).then_some(work)
            }
            Err(error) => {
                eprintln!("openresearch app: could not check for pending work: {error:#}");
                None
            }
        }
    }

    /// Quits when nothing is pending; otherwise the user chooses, in the page when `page_ready` says it can ask.
    fn close_window(
        external_server: bool,
        page_ready: bool,
        quit: &mut Quit,
        ask: &mut Ask,
        window: &Window,
        webview: &WebView,
        control_flow: &mut ControlFlow,
    ) {
        // An alert is up already; a second can't show.
        if !matches!(quit, Quit::No) || matches!(ask, Ask::Alert) {
            return;
        }
        if external_server {
            begin_quit(quit, window, webview, control_flow);
            return;
        }

        let Some(pending) = pending_work() else {
            begin_quit(quit, window, webview, control_flow);
            return;
        };

        // Only the page can list the work, so a hidden window comes back to ask once it loads.
        #[cfg(target_os = "macos")]
        if super::windowless() && send_later(UserEvent::Focus(None)) {
            let deadline = Instant::now() + Duration::from_secs(10);
            *ask = Ask::OnLoad(deadline);
            *control_flow = ControlFlow::WaitUntil(deadline);
            return;
        }
        if !page_ready {
            *ask = Ask::Alert;
            ask_natively(window, &pending);
            return;
        }
        let detail =
            serde_json::json!({ "work": pending, "minimizes": !cfg!(target_os = "macos") });
        // A quit from the Dock or the menu can find the window minimized or behind, where the page draws nothing.
        window.set_minimized(false);
        window.set_focus();
        let _ = webview.evaluate_script(&format!(
            "window.dispatchEvent(new CustomEvent('orx:close-request', {{ detail: {detail} }}));"
        ));
        let deadline = Instant::now() + Duration::from_secs(1);
        *ask = Ask::Sent(deadline);
        *control_flow = ControlFlow::WaitUntil(deadline);
    }

    fn answer_close(
        answer: super::CloseAnswer,
        quit: &mut Quit,
        window: &Window,
        webview: &WebView,
        control_flow: &mut ControlFlow,
    ) {
        match answer {
            super::CloseAnswer::Cancel => {}
            super::CloseAnswer::Quit => begin_quit(quit, window, webview, control_flow),
            #[cfg(not(target_os = "macos"))]
            super::CloseAnswer::Background => {
                let _ = webview;
                window.set_minimized(true);
            }
            #[cfg(target_os = "macos")]
            super::CloseAnswer::Background => {
                super::WINDOWLESS.send_replace(true);
                let _ = webview.load_url("about:blank");
                // Ordering out a fullscreen window strands its Space; the resize that ends the exit hides it.
                if in_native_fullscreen(window) {
                    window.set_fullscreen(None);
                    return;
                }
                window.set_visible(false);
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn in_native_fullscreen(window: &Window) -> bool {
        use objc2_app_kit::{NSWindow, NSWindowStyleMask};
        use tao::platform::macos::WindowExtMacOS;

        // Tao owns this window for the duration of the main-thread event.
        let native_window = unsafe { &*window.ns_window().cast::<NSWindow>() };
        native_window
            .styleMask()
            .contains(NSWindowStyleMask::FullScreen)
    }

    /// Asks in an alert, answering with [`UserEvent::CloseAnswer`].
    fn ask_natively(window: &Window, pending: &serde_json::Value) {
        let title = "Work is still running";
        // A newer server may list work this build cannot read; the page it serves still can.
        let pending = serde_json::from_value::<crate::store::PendingWork>(pending.clone());
        let description = pending.as_ref().map_or_else(
            |_| "Agents or experiments are still running.".to_string(),
            super::close_alert_description,
        );
        let pending = pending.unwrap_or_default();
        let background = if cfg!(target_os = "macos") {
            "Run in background"
        } else {
            "Minimize instead"
        };
        // A hidden window is already running in the background.
        let background = (!super::windowless()).then_some(background);
        let quit = if pending.blocks_quit() {
            "Force Quit"
        } else {
            "Quit"
        };
        let answer = move |label: &str| match label {
            label if label == quit => super::CloseAnswer::Quit,
            label if Some(label) == background => super::CloseAnswer::Background,
            _ => super::CloseAnswer::Cancel,
        };

        #[cfg(target_os = "macos")]
        {
            let buttons: Vec<&str> = background.into_iter().chain([quit, "Cancel"]).collect();
            ask_in_sheet(
                window,
                title,
                &description,
                &buttons.clone(),
                false,
                move |chosen| {
                    let label = buttons.get(chosen).copied().unwrap_or("Cancel");
                    send_later(UserEvent::CloseAnswer(answer(label)));
                },
            );
        }
        #[cfg(windows)]
        let chosen = {
            let cancel = "Cancel".to_string();
            let buttons = match background {
                Some(background) => {
                    rfd::MessageButtons::YesNoCancelCustom(background.into(), quit.into(), cancel)
                }
                None => rfd::MessageButtons::OkCancelCustom(quit.into(), cancel),
            };
            let dialog = rfd::MessageDialog::new()
                .set_title(title)
                .set_description(description)
                .set_buttons(buttons)
                .set_parent(window);
            match dialog.show() {
                rfd::MessageDialogResult::Custom(label) => label,
                _ => "Cancel".into(),
            }
        };
        #[cfg(target_os = "linux")]
        let chosen = {
            use gtk::prelude::*;
            use tao::platform::unix::WindowExtUnix;

            let dialog = gtk::MessageDialog::builder()
                .transient_for(window.gtk_window())
                .modal(true)
                .text(title)
                .secondary_text(description)
                .build();
            dialog.add_button("Cancel", gtk::ResponseType::Cancel);
            dialog.add_button(quit, gtk::ResponseType::Accept);
            if let Some(background) = background {
                dialog.add_button(background, gtk::ResponseType::Other(0));
            }
            dialog.set_default_response(gtk::ResponseType::Cancel);
            let response = dialog.run();
            dialog.close();
            match response {
                gtk::ResponseType::Accept => quit,
                gtk::ResponseType::Other(0) => background.unwrap_or("Cancel"),
                _ => "Cancel",
            }
            .to_string()
        };

        #[cfg(not(target_os = "macos"))]
        send_later(UserEvent::CloseAnswer(answer(&chosen)));
    }

    /// Asks, when no page can, whether to start the stopped server again or quit, answering
    /// with [`UserEvent::StopAnswer`].
    fn ask_after_stop(window: &Window, stop: &ServerStop) {
        /// The end of the log the alert has room for; the server stopped where it ends.
        const LINES: usize = 12;
        const LINE_CHARS: usize = 300;

        let title = "OpenResearch stopped";
        let reason = stop
            .reason
            .as_deref()
            .unwrap_or("Restart it to keep working.");
        let lines: Vec<&str> = stop.log.lines().collect();
        let shown = &lines[lines.len().saturating_sub(LINES)..];
        let mut description = reason.to_string();
        if !shown.is_empty() {
            description.push_str("\n\n");
            if shown.len() < lines.len() {
                description.push_str("…\n");
            }
            for line in shown {
                match line.char_indices().nth(LINE_CHARS) {
                    Some((end, _)) => description.push_str(&format!("{}…\n", &line[..end])),
                    None => description.push_str(&format!("{line}\n")),
                }
            }
        }
        let description = description.trim_end();

        #[cfg(target_os = "macos")]
        ask_in_sheet(
            window,
            title,
            description,
            &["Restart", "Quit"],
            true,
            |chosen| {
                send_later(UserEvent::StopAnswer(chosen == 0));
            },
        );
        #[cfg(windows)]
        let restart = {
            let dialog = rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(title)
                .set_description(description)
                .set_buttons(rfd::MessageButtons::OkCancelCustom(
                    "Restart".into(),
                    "Quit".into(),
                ));
            let dialog = if window.is_visible() {
                dialog.set_parent(window)
            } else {
                dialog
            };
            matches!(dialog.show(), rfd::MessageDialogResult::Custom(label) if label == "Restart")
        };
        #[cfg(target_os = "linux")]
        let restart = {
            use gtk::prelude::*;
            use tao::platform::unix::WindowExtUnix;

            let dialog = gtk::MessageDialog::builder()
                .transient_for(window.gtk_window())
                .modal(true)
                .message_type(gtk::MessageType::Error)
                .text(title)
                .secondary_text(description)
                .build();
            dialog.add_button("Quit", gtk::ResponseType::Reject);
            dialog.add_button("Restart", gtk::ResponseType::Accept);
            dialog.set_default_response(gtk::ResponseType::Accept);
            let response = dialog.run();
            dialog.close();
            response == gtk::ResponseType::Accept
        };
        #[cfg(not(target_os = "macos"))]
        send_later(UserEvent::StopAnswer(restart));
    }

    /// An alert drawn in this process as a sheet on the window, calling `answered` with the index
    /// of the button chosen; the first is the default, and `critical` marks it as a failure. It opens once tao's event handler returns:
    /// the sheet's animation redraws the window, which needs that handler, and would deadlock.
    #[cfg(target_os = "macos")]
    fn ask_in_sheet(
        window: &Window,
        title: &str,
        description: &str,
        buttons: &[&str],
        critical: bool,
        answered: impl Fn(usize) + 'static,
    ) {
        use objc2::rc::Retained;
        use objc2_app_kit::{NSAlertStyle, NSModalResponse, NSWindow};
        use objc2_foundation::NSRunLoop;
        use tao::platform::macos::WindowExtMacOS;

        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let alert = NSAlert::new(mtm);
        if critical {
            alert.setAlertStyle(NSAlertStyle::Critical);
        }
        alert.setMessageText(&NSString::from_str(title));
        alert.setInformativeText(&NSString::from_str(description));
        for button in buttons {
            alert.addButtonWithTitle(&NSString::from_str(button));
        }
        window.set_visible(true);
        // SAFETY: tao's window is an NSWindow, retained here for as long as the sheet needs it.
        let Some(parent) = (unsafe { Retained::retain(window.ns_window().cast::<NSWindow>()) })
        else {
            return;
        };
        // Holds the alert until the sheet ends.
        let kept = alert.clone();
        let handler = block2::RcBlock::new(move |response: NSModalResponse| {
            let _ = &kept;
            answered(usize::try_from(response - NSAlertFirstButtonReturn).unwrap_or(usize::MAX));
        });
        let open = block2::RcBlock::new(move || {
            alert.beginSheetModalForWindow_completionHandler(&parent, Some(&handler));
        });
        // SAFETY: the block runs once, on the main thread, and owns everything it uses.
        unsafe { NSRunLoop::mainRunLoop().performBlock(&open) };
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

    /// Windows swaps its maximize and restore buttons on `data-maximized`; macOS drops the
    /// traffic lights' inset on `data-fullscreen`, where the lights hide.
    #[cfg(any(windows, target_os = "macos"))]
    fn sync_root_flag(webview: &WebView, attribute: &str, on: bool) {
        let script = format!("document.documentElement.toggleAttribute('{attribute}', {on});");
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

    /// The lights' left edge sits on the sidebar's icon column.
    #[cfg(target_os = "macos")]
    const TRAFFIC_LIGHTS_X: f64 = 18.0;
    /// The middle of the page's 44px titlebar strip, where its toggle, logo and title are centered.
    #[cfg(target_os = "macos")]
    const TRAFFIC_LIGHTS_CENTER_Y: f64 = 22.0;

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
                    container.frame().size.height - button.frame().size.height
                        + TRAFFIC_LIGHTS_CENTER_Y * zoom
                        - current,
                )
            })
            .unwrap_or(2.0 + TRAFFIC_LIGHTS_CENTER_Y * zoom);
        window.set_traffic_light_inset(LogicalPosition::new(TRAFFIC_LIGHTS_X, inset));
        // Tao reapplies the inset on a title change, not on request_redraw.
        window.set_title(&window.title());
    }

    #[cfg(target_os = "macos")]
    #[derive(serde::Deserialize)]
    struct PageMenu {
        id: String,
        label: String,
        /// Read one by one, so an item a newer page adds costs only that item.
        items: Vec<serde_json::Value>,
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

    /// The entries of a menu section this window can show; a newer page's others are left out.
    #[cfg(target_os = "macos")]
    fn menu_entries(items: &[serde_json::Value]) -> Vec<PageMenuEntry> {
        items
            .iter()
            .filter_map(|item| match serde_json::from_value(item.clone()) {
                Ok(entry) => Some(entry),
                Err(error) => {
                    eprintln!("openresearch app: leaving out menu item {item}: {error}");
                    None
                }
            })
            .collect()
    }

    /// A shortcut this window cannot parse leaves its item without one.
    #[cfg(target_os = "macos")]
    fn shortcut(id: &str, hotkey: &str) -> Option<Accelerator> {
        match hotkey.replace("Mod", "CmdOrCtrl").parse() {
            Ok(accelerator) => Some(accelerator),
            Err(error) => {
                eprintln!("openresearch app: showing {id} without its shortcut {hotkey}: {error}");
                None
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn build_menu(spec: &[PageMenu]) -> anyhow::Result<Menu> {
        let menu = Menu::new();
        for section in spec {
            let submenu = Submenu::with_id(&section.id, &section.label, true);
            for entry in &menu_entries(&section.items) {
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
                            .and_then(|hotkey| shortcut(id, hotkey));
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
                            _ => {
                                eprintln!("openresearch app: leaving out native menu item {name}");
                                continue;
                            }
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

    /// Mica behind the whole window, so it shows wherever the page is transparent.
    /// False before Windows 11 22H2, which has no system backdrop.
    #[cfg(windows)]
    fn add_window_material(window: &Window) -> bool {
        use tao::platform::windows::WindowExtWindows;
        use windows_sys::Win32::Graphics::Dwm::{
            DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW,
            DWMWA_SYSTEMBACKDROP_TYPE,
        };
        use windows_sys::Win32::UI::Controls::MARGINS;

        let hwnd = window.hwnd() as _;
        let backdrop = DWMSBT_MAINWINDOW;
        // SAFETY: the window is alive and the attribute points at a value of its size.
        let set = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE as u32,
                (&raw const backdrop).cast(),
                size_of_val(&backdrop) as u32,
            )
        };
        if set < 0 {
            return false;
        }

        // DWM draws the backdrop only under the frame, so the frame takes the whole client area.
        let whole = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        // SAFETY: as above.
        if unsafe { DwmExtendFrameIntoClientArea(hwnd, &whole) } < 0 {
            return false;
        }
        // Over an extended frame DWM shows the backdrop through black, not the default white.
        window.set_background_color(Some((0, 0, 0, 0)));
        true
    }

    /// Hands a pop-up's URL to the system opener, when its scheme is one that may leave the app.
    fn open_outside(
        #[cfg_attr(not(target_os = "linux"), allow(unused))] window: &Window,
        url: &str,
    ) {
        if !super::opens_in_browser(url) {
            return;
        }

        #[cfg(target_os = "linux")]
        let activation_token = {
            use gtk::{gio, prelude::*};
            use tao::platform::unix::WindowExtUnix;

            let context = window.gtk_window().display().app_launch_context();
            context.and_then(|context| {
                // Wayland needs the click's activation token to focus an existing browser.
                context.set_timestamp(gtk::current_event_time());
                gio::AppInfo::create_from_commandline(
                    "xdg-open",
                    None,
                    gio::AppInfoCreateFlags::NONE,
                )
                .ok()
                .and_then(|app| context.startup_notify_id(&app, &[]))
            })
        };
        #[cfg(not(target_os = "linux"))]
        let activation_token: Option<String> = None;

        crate::browser::open_browser(url, activation_token.as_deref());
    }

    /// Puts a PNG of the page on the clipboard, then sends [`UserEvent::ScreenshotCopied`].
    #[cfg(target_os = "macos")]
    fn copy_screenshot(webview: &WebView, proxy: tao::event_loop::EventLoopProxy<UserEvent>) {
        use objc2_app_kit::{
            NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard, NSPasteboardTypePNG,
        };
        use objc2_foundation::{NSDictionary, NSError};

        let done = block2::RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
            let copied = match unsafe { image.as_ref() } {
                None => Err(unsafe { error.as_ref() }.map_or_else(
                    || "WebKit returned no image".into(),
                    |e| e.localizedDescription().to_string(),
                )),
                Some(image) => image
                    .TIFFRepresentation()
                    .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                    .and_then(|bitmap| unsafe {
                        bitmap.representationUsingType_properties(
                            NSBitmapImageFileType::PNG,
                            &NSDictionary::new(),
                        )
                    })
                    .ok_or_else(|| "the snapshot could not be encoded as PNG".to_string())
                    .and_then(|png| {
                        let pasteboard = NSPasteboard::generalPasteboard();
                        pasteboard.clearContents();
                        pasteboard
                            .setData_forType(Some(&png), unsafe { NSPasteboardTypePNG })
                            .then_some(())
                            .ok_or_else(|| "the clipboard refused the image".to_string())
                    }),
            };
            let _ = proxy.send_event(UserEvent::ScreenshotCopied(copied));
        });
        let wk = webview.webview();
        unsafe {
            let _: () = objc2::msg_send![
                &*wk,
                takeSnapshotWithConfiguration: std::ptr::null::<AnyObject>(),
                completionHandler: &*done
            ];
        }
    }

    #[cfg(test)]
    mod tests {
        use super::ZoomAction;

        #[cfg(target_os = "macos")]
        #[test]
        fn a_newer_pages_menu_items_cost_only_themselves() {
            let items = serde_json::json!([
                { "kind": "command", "id": "chat.new", "label": "New Chat", "hotkey": "Mod+N" },
                { "kind": "submenu", "id": "recent", "label": "Open Recent", "items": [] },
                { "kind": "separator" },
                { "kind": "native", "name": "copy", "role": "edit" },
            ]);

            let entries = super::menu_entries(items.as_array().unwrap());

            assert!(matches!(
                entries.as_slice(),
                [
                    super::PageMenuEntry::Command { id, .. },
                    super::PageMenuEntry::Separator,
                    super::PageMenuEntry::Native { name },
                ] if id == "chat.new" && name == "copy"
            ));
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn a_shortcut_the_window_cannot_parse_is_left_off() {
            assert!(super::shortcut("chat.new", "Mod+N").is_some());
            assert!(super::shortcut("chat.find", "Mod+Nonsense").is_none());
        }

        #[test]
        fn a_reported_error_is_the_reason_and_leaves_the_log() {
            let lines = |text: &str| text.lines().map(String::from).collect::<Vec<_>>();
            let reported = super::ServerStop::read(
                lines("orx up: starting\nAnother program is using port 4792. Quit it and try again.\n"),
                Some((Some(1), "exit status: 1".into())),
            );
            assert_eq!(
                reported.reason.as_deref(),
                Some("Another program is using port 4792. Quit it and try again.")
            );
            assert_eq!(reported.log, "orx up: starting\nexit status: 1");

            let killed = super::ServerStop::read(
                lines("orx up: dashboard on http://127.0.0.1:4792"),
                Some((None, "signal: 9 (SIGKILL)".into())),
            );
            assert_eq!(killed.reason, None);
            assert_eq!(
                killed.log,
                "orx up: dashboard on http://127.0.0.1:4792\nsignal: 9 (SIGKILL)"
            );

            let unknown = super::ServerStop::read(lines("could not restart"), None);
            assert_eq!(
                (unknown.reason, unknown.log.as_str()),
                (None, "could not restart")
            );
        }

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
    fn close_alert_names_each_pending_item_under_what_quitting_does_to_it() {
        use crate::store::{AfterQuit, PendingAgent, PendingRun, PendingWork};
        let run = |name: &str, backend: &str, after_quit| PendingRun {
            project_id: "project".into(),
            project: None,
            experiment_id: "experiment".into(),
            run_id: name.into(),
            session_id: None,
            name: name.into(),
            backend: Some(backend.into()),
            after_quit,
        };
        let agent = PendingAgent {
            project_id: "project".into(),
            project: Some("Scaling laws".into()),
            session_id: "chat".into(),
            side_parent_id: None,
            harness: "codex".into(),
            title: Some("Tune the schedule".into()),
            resumes_after_restart: false,
        };

        let remote = super::close_alert_description(&PendingWork {
            agents: vec![agent],
            runs: vec![run("lr-sweep", "modal_job", AfterQuit::Continues)],
        });
        assert!(remote.contains("Tune the schedule (Scaling laws)") && remote.contains("lr-sweep"));
        assert!(!remote.contains("Force Quit") && !remote.contains("Finished"));

        let blocked = super::close_alert_description(&PendingWork {
            agents: vec![],
            runs: vec![
                run("eval", "tinker_job", AfterQuit::NeedsApp),
                run("probe", "vercel_job", AfterQuit::NeedsApp),
            ],
        });
        assert!(blocked.starts_with("Force Quit stops"));
        assert!(
            blocked.contains("eval: runs on this computer") && blocked.contains("probe: without")
        );
        assert!(!blocked.contains("Agents"));
    }

    #[test]
    fn a_users_native_quit_goes_through_the_window_and_the_systems_does_not() {
        assert!(super::native_quit_goes_through_window(None));
        for reason in [*b"quia", *b"shut", *b"rest", *b"rlgo"] {
            assert!(!super::native_quit_goes_through_window(Some(
                u32::from_be_bytes(reason)
            )));
        }
    }

    #[test]
    fn new_work_resets_windowless_idle_exit() {
        let mut idle = crate::commands::up::api_v2::Idle::default();
        let secs = std::time::Duration::from_secs;

        assert!(!idle.should_exit(secs(0), false));
        assert!(!idle.should_exit(secs(599), true));
        assert!(!idle.should_exit(secs(600), false));
        assert!(!idle.should_exit(secs(1199), false));
        assert!(idle.should_exit(secs(1200), false));
    }

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
        assert!(entry.contains(
            "\nMimeType=x-scheme-handler/openresearch;application/x-openresearch-project;\n"
        ));
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
        let shared = std::env::temp_dir().join("tiny lm.orx");
        assert_eq!(
            link_path(url::Url::from_file_path(&shared).unwrap().as_str()),
            Some(format!(
                "/open?file={}",
                urlencoding::encode(&shared.to_string_lossy())
            ))
        );
        let notes = url::Url::from_file_path(std::env::temp_dir().join("notes.txt")).unwrap();
        assert_eq!(link_path(notes.as_str()), None);
        assert_eq!(
            link_path("openresearch://p/a1B2c3D4e5F6").as_deref(),
            Some("/open?shared=a1B2c3D4e5F6")
        );
        assert_eq!(
            link_path("openresearch://p/a1B2c3D4e5F6/").as_deref(),
            Some("/open?shared=a1B2c3D4e5F6")
        );
        assert_eq!(link_path("openresearch://p/../../etc"), None);
        assert_eq!(link_path("openresearch://p/short"), None);
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
    fn the_app_takes_a_shared_project_file_as_a_file_url() {
        let shared = std::env::temp_dir().join("tiny lm.orx");
        let url = url::Url::from_file_path(&shared).unwrap().to_string();
        let path = shared.to_str().unwrap();
        assert_eq!(app_args(&["app", path]), Some(Some(url.clone())));
        assert_eq!(app_args(&["app", &url]), Some(Some(url)));
        let notes = std::env::temp_dir().join("notes.txt");
        assert_eq!(app_args(&["app", notes.to_str().unwrap()]), None);
        assert_eq!(app_args(&["app", "tiny.orx"]), Some(None));
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
        assert!(opens_in_browser("linear://alphaxiv/team/ORX/new?title=Bug"));
        assert!(!opens_in_browser("linear://alphaxiv/settings/api"));
        assert!(!opens_in_browser("ssh://attacker.example"));
        assert!(!opens_in_browser("vscode://file/etc/passwd"));
        assert!(!opens_in_browser(
            "x-apple.systempreferences:com.apple.preference"
        ));
    }
}
