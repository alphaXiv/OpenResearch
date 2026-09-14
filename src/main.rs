//! OpenResearch CLI (`orx`) — Rust port entry point.
//!
//! A clap-derive command tree mirroring the USAGE
//! block, dispatched from an async `tokio::main`. Each subcommand routes to one
//! module fn in `commands::<name>`. The six fs verbs (read/write/str-replace/
//! ls/grep/rm) all route into `commands::fs`.
//!
//! Error handling: command fns return `anyhow::Result<()>`. `main` prints the
//! error's `Display` to stderr and exits 1 — matching the TS
//! `main().catch(err => { console.error(err.message); process.exit(1) })`.

mod browser;
mod cli;
mod editors;
// DTOs faithfully mirror every API wire field; not all are read by the CLI yet.
#[allow(dead_code)]
mod client;
mod commands;
mod compute;
mod config;
mod error;
mod folder_picker;
mod invocation;
mod jobs;
// Local mode (`orx up`): builds out across stages; not all of it is wired yet.
#[allow(dead_code)]
mod local;
mod output;
mod paths;
mod plane;
mod remote;
mod store;
mod telemetry;
mod updates;
mod workspace_state;

// The CLI schema (Cli, Command, and every subcommand's Args/Subcommand types)
// lives in `cli`, split by domain; re-export it here so the rest of the crate
// keeps addressing it as `crate::LoginArgs`, `crate::Command`, etc.
pub use cli::*;

use clap::Parser;

// The default multi-thread runtime is load-bearing for macOS app mode: it blocks
// the main thread in the AppKit run loop while the dashboard server runs on
// worker threads. A `current_thread` flavor would deadlock. See commands::app.
#[tokio::main]
async fn main() {
    #[cfg(windows)]
    install_panic_reporter();
    // Double-clicked as the macOS .app? Enter GUI app mode (Dock icon, dashboard
    // server, browser) instead of parsing CLI args. Also require an empty argv so
    // the bundled binary stays usable as a CLI (`…/MacOS/OpenResearch up`), since
    // the bundle itself launches it with no arguments. See commands::app.
    #[cfg(target_os = "macos")]
    if commands::app::launched_as_app_bundle() && std::env::args_os().len() == 1 {
        // Shell hydration may change XDG_CONFIG_HOME; settle it before telemetry or the lifecycle lock.
        commands::app::hydrate_shell_env().await;
        telemetry::set_flag(false);
        let _session = telemetry::TelemetrySession::start_app();
        // AppKit owns process shutdown; the durable outbox covers termination before delivery.
        commands::app::run().await;
        return;
    }

    let mut cli = Cli::parse();
    // Double-clicked from Explorer: start the dashboard, as the macOS .app does.
    if cli.command.is_none() && owns_its_console() {
        cli.command = Cli::parse_from(["orx", "up"]).command;
    }
    let Some(command) = cli.command else {
        // Bare `orx`: print the command overview to stdout and exit 0.
        use clap::CommandFactory;
        Cli::command().print_help().ok();
        return;
    };
    // Outdated-version warning (skipped for the commands that manage updates
    // themselves). `start` prints the cached warning to stderr *now*,
    // before the command runs, so it shows even for commands that
    // `std::process::exit` on their own (e.g. the "not logged in" path) instead
    // of returning here. Never touches stdout or the exit code. Silence it with
    // ORX_NO_UPDATE_CHECK / NO_UPDATE_NOTIFIER.
    // `plan-gate` is a per-tool-call hook body (fires on every Bash call during
    // plan mode): it must stay fast and touch neither stdout nor the network, so
    // skip the update check and telemetry and run it directly.
    if matches!(command, Command::PlanGate) {
        // The hook fires on every Bash call during plan mode; it must NEVER
        // block the turn. Swallow any error to stderr and still exit 0 — a
        // non-zero exit here would fail every Bash tool call. (`run` is
        // infallible today; this keeps the invariant if that ever changes.)
        if let Err(err) = commands::plan_gate::run().await {
            eprintln!("orx plan-gate: {err}");
        }
        return;
    }
    // `mcp-gate` is Claude's stdio MCP child for the turn: stdout is the MCP
    // channel (nothing else may write to it) and startup must be instant or
    // Claude times the server out — skip the update check and telemetry.
    if matches!(command, Command::McpGate) {
        if let Err(err) = commands::mcp_gate::run().await {
            // stderr only; a failed bridge degrades plan mode, never the CLI.
            eprintln!("orx mcp-gate: {err}");
            std::process::exit(1);
        }
        return;
    }
    if let Command::RemoteHost(args) = &command {
        if let Err(err) = commands::remote_host::run(args.clone()).await {
            eprintln!("orx remote-host: {err}");
            std::process::exit(1);
        }
        return;
    }
    if let Command::PublishBranch(args) = &command {
        let publish = || -> error::Result<()> {
            let lock = store::open_lifecycle_lock()?;
            let _guard = lock.read()?;
            local::git::push_branch(&args.repo_path, &args.branch, &args.owner, &args.repo)
        };
        if let Err(err) = publish() {
            eprintln!("orx publish-branch: {err}");
            std::process::exit(1);
        }
        return;
    }

    let warning = (!matches!(
        command,
        Command::Version(_) | Command::Update(_) | Command::Delete(_)
    ))
    .then(updates::UpdateWarning::start);

    // Anonymous usage analytics. Record the flag process-globally so command
    // modules can fire events without threading it through, then fire the
    // per-invocation event *before* dispatch so commands that exit on their own
    // (e.g. the "not logged in" path) are still counted. Opt out with
    // --no-telemetry or `orx telemetry off`.
    telemetry::set_flag(cli.no_telemetry);
    let session = telemetry::TelemetrySession::start(
        should_capture_command(&command).then(|| command_name(&command)),
    );

    let result = dispatch(command).await;
    if let Some(warning) = warning {
        warning.finish().await;
    }
    session.finish(result.is_ok()).await;

    if let Err(err) = result {
        // Match the TS: print only the message, exit 1.
        eprintln!("{}", err);
        show_error_dialog(&err.to_string());
        std::process::exit(1);
    }
}

/// Explorer gives a double-clicked exe a console of its own, which closes when it exits.
#[cfg(windows)]
fn owns_its_console() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;

    let mut attached = [0u32; 2];
    // SAFETY: writes at most `attached.len()` process ids into `attached`.
    let count = unsafe { GetConsoleProcessList(attached.as_mut_ptr(), attached.len() as u32) };
    count == 1
}

#[cfg(not(windows))]
fn owns_its_console() -> bool {
    false
}

/// A double-clicked exe's console closes with it, so repeat the error in a dialog.
#[cfg(windows)]
fn show_error_dialog(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    if !owns_its_console() {
        return;
    }
    let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (body, title) = (wide(message), wide("OpenResearch stopped"));
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

#[cfg(target_os = "macos")]
fn show_error_dialog(message: &str) {
    if !commands::app::launched_as_app_bundle() || std::env::args_os().len() != 1 {
        return;
    }
    let _ = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv\ndisplay alert \"OpenResearch could not start\" message (item 1 of argv) as critical\nend run",
            "--",
            message,
        ])
        .output();
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_error_dialog(_message: &str) {}

/// Main thread only: a worker's panic has a running dashboard to report through.
#[cfg(windows)]
fn install_panic_reporter() {
    let inner = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        inner(info);
        if std::thread::current().name() == Some("main") {
            show_error_dialog(&format!("{info}"));
        }
    }));
}

fn should_capture_command(command: &Command) -> bool {
    !matches!(
        command,
        Command::Supervise(_)
            | Command::Update(UpdateArgs {
                background: true,
                ..
            })
    )
}

/// A stable, PII-free event label for each command, decoupled from the enum
/// variant name so renames don't silently break analytics continuity.
fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Login(_) => "login",
        Command::Logout => "logout",
        Command::Projects(_) => "projects",
        Command::Orgs(_) => "orgs",
        Command::Project(_) => "project",
        Command::Agent(_) => "agent",
        Command::Runs(_) => "runs",
        Command::Logs(_) => "logs",
        Command::CreateExperiment(_) => "create-experiment",
        Command::Compute(_) => "compute",
        Command::Instance(_) => "instance",
        Command::SshKey(_) => "ssh-key",
        Command::Exp(_) => "exp",
        Command::Skill(_) => "skill",
        Command::Skills(_) => "skills",
        Command::Templates(_) => "templates",
        Command::InstallSkills(_) => "install-skills",
        Command::Discover(_) => "discover",
        Command::Paper(_) => "paper",
        Command::Version(_) => "version",
        Command::Update(_) => "update",
        Command::InstallCli(_) => "install-cli",
        Command::Delete(_) => "delete",
        Command::Serve(_) => "serve",
        Command::Supervise(_) => "supervise",
        Command::Up(_) => "up",
        Command::Telemetry(_) => "telemetry",
        Command::PlanGate => "plan-gate",
        Command::McpGate => "mcp-gate",
        Command::PublishBranch(_) => "publish-branch",
        Command::RemoteHost(_) => "remote-host",
    }
}

async fn dispatch(command: Command) -> error::Result<()> {
    let uses_lock = command_uses_lifecycle_lock(&command);
    if uses_lock {
        local::storage::prepare().await?;
    }
    let lifecycle_lock = uses_lock.then(store::open_lifecycle_lock).transpose()?;
    let _lifecycle_guard = lifecycle_lock
        .as_ref()
        .map(|lock| lock.read())
        .transpose()?;

    match command {
        Command::Login(args) => commands::login::run(args).await,
        Command::Logout => commands::logout::run().await,
        Command::Projects(args) => commands::projects::run(args).await,
        Command::Orgs(args) => commands::orgs::run(args).await,
        Command::Project(args) => commands::project::run(args).await,
        Command::Agent(args) => commands::agent::run(args).await,
        Command::Runs(args) => commands::runs::run(args).await,
        Command::Logs(args) => commands::logs::run(args).await,
        Command::CreateExperiment(args) => commands::create_experiment::run(args).await,
        Command::Compute(args) => commands::compute::run(args).await,
        Command::Instance(args) => commands::instance::run(args).await,
        Command::SshKey(args) => match args.command {
            SshKeyCommand::Add(a) => commands::ssh_key::add(a.path).await,
            SshKeyCommand::List => commands::ssh_key::list().await,
        },
        Command::Exp(args) => commands::exp::run(args).await,
        Command::Skill(args) => commands::skill::run(args).await,
        Command::Skills(args) => commands::library::skills(args),
        Command::Templates(args) => commands::library::templates(args),
        Command::InstallSkills(args) => commands::install_skills::run(args).await,
        Command::Discover(args) => commands::discover::run(args).await,
        Command::Paper(args) => commands::paper::run(args).await,
        Command::Version(args) => commands::version::run(args).await,
        Command::Update(args) => commands::update::run(args).await,
        Command::InstallCli(args) => commands::install_cli::run(args).await,
        Command::Delete(args) => commands::delete::run(args).await,
        Command::Serve(args) => commands::serve::run(args).await,
        Command::Supervise(args) => commands::supervise::run(args).await,
        Command::Up(args) => match args.remote.clone() {
            Some(host) => commands::up_remote::run(&host, args).await,
            None => commands::up::run(args).await,
        },
        Command::Telemetry(args) => commands::telemetry::run(args).await,
        // Handled before dispatch (fast path, no telemetry/update check).
        Command::PlanGate => commands::plan_gate::run().await,
        Command::McpGate => commands::mcp_gate::run().await,
        Command::PublishBranch(_) => unreachable!("handled before dispatch"),
        Command::RemoteHost(_) => unreachable!("handled before dispatch"),
    }
}

fn command_uses_lifecycle_lock(command: &Command) -> bool {
    !matches!(
        command,
        Command::Login(_)
            | Command::Logout
            | Command::InstallSkills(_)
            | Command::Discover(_)
            | Command::Paper(_)
            | Command::Version(_)
            | Command::Delete(_)
            | Command::Telemetry(_)
            | Command::PlanGate
            | Command::McpGate
            | Command::PublishBranch(_)
            | Command::RemoteHost(_)
    )
}
