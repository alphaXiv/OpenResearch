use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use portable_pty::PtySize;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{bad_request, AppState};
use crate::local::harness::managed;
use crate::telemetry::harness::SetupAttempt;

mod sign_in;
pub(super) use sign_in::SignInPage;

fn login_command(harness: &str) -> Option<(&'static str, Vec<String>)> {
    match harness {
        "claude-code" => Some(("claude auth login", vec!["auth".into(), "login".into()])),
        "codex" => Some(("codex login", vec!["login".into()])),
        "opencode" => Some(("opencode auth login", vec!["auth".into(), "login".into()])),
        "antigravity" => Some(("agy", vec![])),
        "cursor" => Some(("agent login", vec!["login".into()])),
        _ => None,
    }
}

fn update_command(harness: &str) -> Option<(&'static str, Vec<String>)> {
    match harness {
        "claude-code" => Some(("claude update", vec!["update".into()])),
        "codex" => Some(("codex update", vec!["update".into()])),
        "opencode" => Some(("opencode upgrade", vec!["upgrade".into()])),
        "antigravity" => Some(("agy update", vec!["update".into()])),
        "cursor" => Some(("agent update", vec!["update".into()])),
        _ => None,
    }
}

pub(super) async fn commands() -> Json<Value> {
    let mut result = serde_json::Map::new();
    for harness in crate::telemetry::harness::IDS {
        let Some(host) = managed::source(harness).filter(|_| managed::supported(harness)) else {
            continue;
        };
        let login = login_command(harness)
            .map(|(label, _)| label)
            .unwrap_or_default();
        let update = update_command(harness)
            .map(|(label, _)| label)
            .unwrap_or_default();
        result.insert(harness.into(), json!({"install":"", "installUrl":null, "managedFrom":host, "login":login, "update":update, "requiresNpm":false}));
    }
    for preset in crate::local::acp::presets() {
        let id = preset.launch.id;
        if let Some(host) = managed::source(&id).filter(|_| managed::supported(&id)) {
            result.insert(id, json!({"install":"", "installUrl":null, "managedFrom":host, "login":"", "update":"", "requiresNpm":false}));
        }
    }
    Json(Value::Object(result))
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Action {
    Install,
    Login,
    Update,
    /// OpenCode only: from 1.x to 2.x.
    Upgrade,
}

impl Action {
    /// The ingestion schema knows three actions; a major upgrade is an update.
    fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Login => "login",
            Self::Update | Self::Upgrade => "update",
        }
    }
}

#[derive(Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Trigger {
    #[default]
    Manual,
    Automatic,
}

#[derive(Deserialize)]
pub(super) struct SetupRequest {
    harness: String,
    action: Action,
    #[serde(default)]
    trigger: Trigger,
    /// Continue into the user's shell once the command has run (settings play button).
    #[serde(default)]
    shell: bool,
    /// The provider OpenCode's own sign-in starts at, skipping its picker.
    provider: Option<String>,
    /// The Claude Code or Codex profile a sign-in signs in, in place of the user's own CLI.
    profile: Option<String>,
}

/// Only OpenCode's sign-in takes a provider, and only an id that cannot read as a flag.
fn provider_allowed(request: &SetupRequest) -> bool {
    request.provider.as_deref().is_none_or(|provider| {
        request.harness == "opencode"
            && matches!(request.action, Action::Login)
            && crate::local::local_models::is_provider_id(provider)
    })
}

/// A profile only signs in, and only to its own agent.
fn profile_allowed(request: &SetupRequest) -> bool {
    request.profile.as_deref().is_none_or(|profile| {
        matches!(request.action, Action::Login)
            && crate::local::agent_profiles::harness_of(profile) == Some(request.harness.as_str())
    })
}

pub(super) async fn connect(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
    Query(request): Query<SetupRequest>,
) -> Response {
    if let Some(rejected) = super::reject_cross_origin(&headers) {
        return rejected;
    }
    if managed::source(&request.harness).is_none() || !managed::supported(&request.harness) {
        return bad_request("Agent installation is unavailable for this platform").into_response();
    }
    if request.harness.starts_with("acp:")
        && !matches!(request.action, Action::Install | Action::Update)
    {
        return bad_request("ACP agents support installation and updates here").into_response();
    }
    if !provider_allowed(&request) {
        return bad_request("Unknown provider").into_response();
    }
    if !profile_allowed(&request) {
        return bad_request("Unknown agent account").into_response();
    }
    if matches!(request.action, Action::Upgrade) && request.harness != "opencode" {
        return bad_request("Only OpenCode upgrades to a new major version").into_response();
    }
    if request.trigger == Trigger::Automatic
        && (request.harness != "opencode" || !matches!(request.action, Action::Install))
    {
        return bad_request("Only OpenCode installation supports automatic setup").into_response();
    }
    ws.on_upgrade(move |mut socket| async move {
        let attempt = SetupAttempt::new(
            &request.harness,
            request.action.label(),
            if request.trigger == Trigger::Automatic {
                "automatic"
            } else {
                "manual"
            },
        );
        let mut size = super::DEFAULT_PTY_SIZE;
        let mut follow_up = None;
        let result = run(
            &state,
            &mut socket,
            &request,
            &attempt,
            &mut size,
            &mut follow_up,
        )
        .await;
        let message = match result {
            Ok(harness) => {
                // Login rejection caches must not mask a successful new login.
                if request.harness == "claude-code" && request.profile.is_none() {
                    state.claude.clear_runtime_rejection();
                }
                super::publish_harness(&state, &harness).await;
                json!({ "type": "complete" })
            }
            Err(error) => {
                *state.harnesses.lock().await = None;
                json!({ "type": "error", "error": error })
            }
        };
        if socket
            .send(Message::Text(message.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        match follow_up {
            Some(env) if request.shell => {
                super::continue_in_shell(&mut socket, &mut size, env).await
            }
            _ => super::close_socket(&mut socket).await,
        }
    })
}

async fn run(
    state: &AppState,
    socket: &mut WebSocket,
    request: &SetupRequest,
    attempt: &SetupAttempt,
    size: &mut PtySize,
    // Set once a command actually ran: the env its follow-up shell must share.
    follow_up: &mut FollowUp,
) -> Result<crate::local::harness::HarnessInfo, String> {
    let mut run_command = true;
    // Held, not terminal: an install whose optional launch failed still counts,
    // but only against evidence this command changed something.
    let mut command_failure: Option<CommandFailure> = None;
    // Only an install can be credited past a nonzero exit, and only if it put a
    // binary where there was none; login/update need no baseline.
    let installed_before = match request.action {
        Action::Install => was_installed(&request.harness).await,
        Action::Login | Action::Update | Action::Upgrade => true,
    };
    if request.trigger == Trigger::Automatic {
        let payload = super::harnesses_payload(
            state,
            &super::HarnessQuery {
                refresh: Some(1),
                retry: None,
                harness: None,
            },
        )
        .await;
        let Some(install) = automatic_install_needed(&payload) else {
            attempt.record("interrupted", "detect", Some("not_eligible"), None, None);
            return Err(
                "Couldn't check which coding agents are installed. Re-check your coding agents."
                    .into(),
            );
        };
        run_command = install;
    }
    if run_command
        && managed::source(&request.harness).is_some()
        && !matches!(request.action, Action::Login)
    {
        run_command = false;
        attempt.record("command_started", "command", None, None, None);
        command_failure = managed_setup(socket, request, attempt, size, follow_up).await?;
    }
    if run_command {
        let (program, mut args) = match request.action {
            Action::Install => return Err("Agent installation requires a managed release".into()),
            Action::Login | Action::Update | Action::Upgrade => {
                let Some(harness) = crate::local::harness::detect_harness(&request.harness).await
                else {
                    attempt.record("failed", "detect", Some("not_installed"), None, None);
                    return Err("Agent not found. Install it, then retry.".into());
                };
                if request.harness == "claude-code"
                    && matches!(request.action, Action::Login)
                    && request.profile.is_none()
                    && (!harness.login_eligible || state.claude.auth_snapshot().runtime_rejected)
                {
                    attempt.record("interrupted", "detect", Some("not_eligible"), None, None);
                    return Err("Claude Code account login is not applicable to the reported provider or auth state. Check its provider configuration and re-check the harness.".into());
                }
                let Some(bin) = harness.bin_path else {
                    attempt.record("failed", "detect", Some("not_installed"), None, None);
                    return Err("Agent not found. Install it, then retry.".into());
                };
                let (_, args) = if matches!(request.action, Action::Login) {
                    login_command(&request.harness)
                } else {
                    update_command(&request.harness)
                }
                .unwrap();
                (bin, args)
            }
        };
        let lease = if request.harness == "opencode" && matches!(request.action, Action::Login) {
            let prepared = async {
                let binary = crate::local::opencode::resolve_binary().await?;
                if binary.protocol != crate::local::opencode::Protocol::V2 {
                    return Ok::<_, crate::error::Error>(None);
                }
                let db = crate::local::native_store::prepare_opencode(
                    crate::local::native_store::NativeStore::Isolated,
                )?;
                crate::local::opencode::prepare_database(&binary, &db)
                    .await
                    .map(Some)
            }
            .await;
            match prepared {
                Ok(lease) => lease,
                Err(error) => {
                    // Preparation runs before any terminal exists, so this is an
                    // executable/database fault, not a failed login attempt.
                    attempt.record(
                        "failed",
                        "detect",
                        Some("spawn_failed"),
                        None,
                        Some(&error.to_string()),
                    );
                    return Err(format!(
                        "{error}\nOpenCode could not be prepared for sign-in. \
                         Re-check the agent to see whether its program file is installed and runnable."
                    ));
                }
            }
        } else {
            None
        };
        if let Some(provider) = &request.provider {
            // OpenCode 2 names the target positionally; 1 takes it as a flag.
            if lease.is_some() {
                args.push(provider.clone());
            } else {
                args.extend(["-p".into(), provider.clone()]);
            }
        }
        // The lease ends with `run`; the follow-up shell only inherits the path.
        let mut env: Vec<_> = lease
            .as_ref()
            .map(|lease| vec![("OPENCODE_DB", lease.path().as_os_str().to_owned())])
            .unwrap_or_default();
        if managed::version_of(std::path::Path::new(&program)).is_some() {
            env.extend(MANAGED_ENV.map(|(key, value)| (key, value.into())));
        }
        if let Some(profile) = &request.profile {
            match crate::local::agent_profiles::login_env(profile) {
                Ok(profile_env) => env.extend(profile_env),
                Err(error) => {
                    attempt.record("failed", "detect", Some("not_installed"), None, None);
                    return Err(error.to_string());
                }
            }
        }
        attempt.record("command_started", "command", None, None, None);
        let login = matches!(request.action, Action::Login);
        let mut screen = TerminalScreen {
            completed: (login && request.harness == "antigravity")
                .then_some(antigravity_prompt_ready as fn(&str) -> bool),
            sign_in: login
                .then(|| SignInPage::for_login(&request.harness))
                .flatten(),
        };
        let (status, output) = run_in_terminal(
            socket,
            attempt,
            size,
            follow_up,
            (program, args),
            env,
            &mut screen,
        )
        .await?;
        command_failure = match screen.sign_in.as_ref().and_then(SignInPage::failure) {
            Some(message) => Some(CommandFailure {
                message: message.to_string(),
                exit_code: status.exit_code(),
                output,
            }),
            None => completed_or_failure(attempt, status, output),
        };
    }
    let mut harness = crate::local::harness::detect_harness(&request.harness).await;
    if request.profile.is_some() {
        if let Some(failure) = command_failure {
            attempt.record(
                "failed",
                "command",
                Some("exit_nonzero"),
                Some(failure.exit_code),
                Some(&failure.output),
            );
            return Err(failure.message);
        }
        attempt.record("succeeded", "verify", None, None, None);
        return harness.ok_or_else(|| "Agent not found. Install it, then retry.".into());
    }
    let verified = setup_verified(request, harness.as_ref());
    if let Some(failure) = command_failure {
        // Only a brand-new working install overrides the failure (codex
        // installs, then its optional launch fails); a binary that was already
        // there proves nothing about a command that exited nonzero.
        if let Some(harness) = harness
            .take_if(|h| verified && fresh_install(request.action, installed_before, Some(h)))
        {
            // One terminal event, with the failed step still visible on it.
            attempt.record(
                "succeeded",
                "verify",
                None,
                Some(failure.exit_code),
                Some(&failure.output),
            );
            return Ok(harness);
        }
        attempt.record(
            "failed",
            "command",
            Some("exit_nonzero"),
            Some(failure.exit_code),
            Some(&failure.output),
        );
        return Err(failure.message);
    }
    if let Some(harness) = harness.take_if(|_| verified) {
        attempt.record("succeeded", "verify", None, None, None);
        return Ok(harness);
    }
    let (reason, detail) = verify_reason(harness.as_ref());
    attempt.record("failed", "verify", Some(reason), None, Some(detail));
    Err(harness
        .and_then(|h| h.agent_note)
        .unwrap_or_else(|| "Could not verify coding agent setup. Re-check and retry.".into()))
}

/// What the setup socket reads in a command's output while it runs.
#[derive(Default)]
pub(super) struct TerminalScreen {
    /// The command's work is done once its output matches, though it keeps running.
    pub(super) completed: Option<fn(&str) -> bool>,
    pub(super) sign_in: Option<SignInPage>,
}

/// A nonzero exit, held until the attempt's one terminal outcome is decided.
struct CommandFailure {
    message: String,
    exit_code: u32,
    output: String,
}

type FollowUp = Option<Vec<(&'static str, std::ffi::OsString)>>;

/// Run `(program, args)` in the setup terminal. A fault in the terminal itself
/// ends the attempt; the exit status and output are the caller's to judge.
async fn run_in_terminal(
    socket: &mut WebSocket,
    attempt: &SetupAttempt,
    size: &mut PtySize,
    follow_up: &mut FollowUp,
    (program, args): (String, Vec<String>),
    env: Vec<(&'static str, std::ffi::OsString)>,
    screen: &mut TerminalScreen,
) -> Result<(portable_pty::ExitStatus, String), String> {
    let pty_size = *size;
    let shell_env = env.clone();
    let session = match tokio::task::spawn_blocking(move || {
        super::start_pty_with_env(&program, args, &env, pty_size, None)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result.map_err(|error| error.to_string()))
    {
        Ok(session) => session,
        Err(error) => {
            // The error embeds the whole script; scanning it would report
            // phrases from branches that never ran.
            attempt.record(
                "failed",
                "command",
                Some("spawn_failed"),
                None,
                Some(&spawn_failure_cause(&error)),
            );
            return Err(error);
        }
    };
    *follow_up = Some(shell_env);
    let mut output = String::new();
    match super::relay_pty(socket, session, Some(&mut output), size, Some(screen)).await {
        Some(Ok(status)) => Ok((status, output)),
        Some(Err(error)) => {
            attempt.record(
                "failed",
                "command",
                Some("terminal_error"),
                None,
                Some(&output),
            );
            Err(error)
        }
        None => {
            attempt.record("interrupted", "command", Some("disconnected"), None, None);
            Err("Terminal disconnected".into())
        }
    }
}

/// A clean exit is recorded now; a nonzero one is held, since one attempt must
/// not emit both `failed` and `succeeded` and what it achieved is only known after.
fn completed_or_failure(
    attempt: &SetupAttempt,
    status: portable_pty::ExitStatus,
    output: String,
) -> Option<CommandFailure> {
    if status.success() {
        attempt.record(
            "command_completed",
            "command",
            None,
            Some(status.exit_code()),
            None,
        );
        return None;
    }
    Some(CommandFailure {
        message: format!("Command exited with code {}", status.exit_code()),
        exit_code: status.exit_code(),
        output,
    })
}

/// Claude Code's and OpenCode's own updaters, which orx's copies never run.
const MANAGED_ENV: [(&str, &str); 2] = [
    ("DISABLE_AUTOUPDATER", "1"),
    ("OPENCODE_DISABLE_AUTOUPDATE", "1"),
];

/// Install and Update for the harnesses orx keeps copies of. An update goes
/// through the user's own updater first; an install, or an update that cannot
/// reach the newest release, becomes orx's copy in the cache.
async fn managed_setup(
    socket: &mut WebSocket,
    request: &SetupRequest,
    attempt: &SetupAttempt,
    size: &mut PtySize,
    follow_up: &mut FollowUp,
) -> Result<Option<CommandFailure>, String> {
    let _setup = managed::lock_installations().map_err(|error| error.to_string())?;
    let harness = request.harness.as_str();
    let user = managed::user_install(harness).await;
    let copy = managed::installed(harness);
    // Installing or upgrading OpenCode means OpenCode 2, whichever major the user runs.
    let major = (harness == "opencode").then(|| match request.action {
        Action::Install | Action::Upgrade => 2,
        Action::Login | Action::Update => following_major(
            user.as_ref().map(|(_, version)| version),
            copy.as_ref().map(|(_, version)| version),
        ),
    });
    if let (Action::Upgrade, Some((bin, version))) = (request.action, &user) {
        if version.major < 2 {
            return upgrade_own_opencode(socket, attempt, size, follow_up, bin).await;
        }
    }
    let release = managed::latest(harness, major).await;
    if let (Some((_, version)), Ok(release)) = (&user, &release) {
        if *version >= release.version {
            attempt.record("command_completed", "command", None, Some(0), None);
            return Ok(None);
        }
    }
    let own_major = |version: &semver::Version| major.is_none_or(|major| version.major == major);
    if let (Action::Update, Some((bin, _))) = (
        request.action,
        user.as_ref()
            .filter(|(_, version)| own_major(version) && runs_own_updater(harness, version)),
    ) {
        let (_, args) = update_command(harness).expect("the selected agent has an updater");
        let program = bin.to_string_lossy().into_owned();
        let (status, output) = run_in_terminal(
            socket,
            attempt,
            size,
            follow_up,
            (program, args),
            Vec::new(),
            &mut TerminalScreen::default(),
        )
        .await?;
        let Ok(release) = &release else {
            // No release list: the user's updater is the only update there is.
            return Ok(completed_or_failure(attempt, status, output));
        };
        let current = managed::user_install(harness)
            .await
            .is_some_and(|(_, version)| version >= release.version);
        if current {
            return Ok(completed_or_failure(attempt, status, output));
        }
    }
    let release = match release {
        Ok(release) => release,
        Err(error) => return Ok(Some(install_failure(error))),
    };
    if let Some((bin, version)) = &copy {
        if current_copy(version, &release) && managed::verify(harness, bin, version).await.is_ok() {
            attempt.record("command_completed", "command", None, Some(0), None);
            return Ok(None);
        }
    }
    match install_with_progress(socket, harness, &release).await {
        Some(Ok(_)) => {
            attempt.record("command_completed", "command", None, Some(0), None);
            Ok(None)
        }
        Some(Err(error)) => Ok(Some(install_failure(error))),
        None => {
            attempt.record("interrupted", "command", Some("disconnected"), None, None);
            Err("Terminal disconnected".into())
        }
    }
}

/// The major OpenCode's copy follows: a 2.x copy stands in for the user's own 1.x
/// once they chose it, and is never traded back for it.
fn following_major(user: Option<&semver::Version>, copy: Option<&semver::Version>) -> u64 {
    match (user, copy) {
        (Some(user), Some(copy)) => user.major.max(copy.major),
        (Some(only), None) | (None, Some(only)) => only.major,
        (None, None) => 2,
    }
}

/// The user's own OpenCode 1 moves itself to OpenCode 2 with its own updater, which
/// answers a method it can't handle (Homebrew, Scoop) with instructions and a clean exit.
async fn upgrade_own_opencode(
    socket: &mut WebSocket,
    attempt: &SetupAttempt,
    size: &mut PtySize,
    follow_up: &mut FollowUp,
    bin: &std::path::Path,
) -> Result<Option<CommandFailure>, String> {
    let args = vec!["upgrade".to_string(), "--major".to_string()];
    let (status, output) = run_in_terminal(
        socket,
        attempt,
        size,
        follow_up,
        (bin.to_string_lossy().into_owned(), args),
        Vec::new(),
        &mut TerminalScreen::default(),
    )
    .await?;
    let moved = managed::user_install("opencode")
        .await
        .is_some_and(|(_, version)| version.major >= 2);
    if moved {
        return Ok(completed_or_failure(attempt, status, output));
    }
    Ok(Some(CommandFailure {
        message: "OpenCode's own updater couldn't move it to OpenCode 2.".into(),
        exit_code: if status.success() {
            1
        } else {
            status.exit_code()
        },
        output,
    }))
}

/// The terminal prints the attempt's error itself, once the attempt ends.
fn install_failure(error: crate::error::Error) -> CommandFailure {
    let message = format!("{error:#}");
    CommandFailure {
        exit_code: 1,
        output: message.clone(),
        message,
    }
}

/// Download and place `release`, drawing its progress in the terminal. `None`
/// when the client went away, which abandons the download.
async fn install_with_progress(
    socket: &mut WebSocket,
    harness: &str,
    release: &managed::Release,
) -> Option<crate::error::Result<std::path::PathBuf>> {
    let name = crate::local::harness::chat_harness(harness)
        .map_or_else(|| harness.to_string(), |h| h.name().to_string());
    let source = managed::source(harness).unwrap_or_default();
    let version = &release.version;
    write_terminal(
        socket,
        &format!("Downloading {name} {version} from {source}…\r\n"),
    )
    .await
    .ok()?;
    write_progress(socket, 0, None).await.ok()?;
    let (sender, mut received) = tokio::sync::watch::channel((0, None));
    let mut progress = move |done: u64, total: Option<u64>| {
        let _ = sender.send((done, total));
    };
    let install = managed::install(harness, release, &mut progress);
    tokio::pin!(install);
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    let megabytes = |bytes: u64| format!("{:.1} MB", bytes as f64 / 1e6);
    loop {
        tokio::select! {
            result = &mut install => {
                let line = match &result {
                    Ok(_) => format!("\r\x1b[2KInstalled {name} {version}.\r\n"),
                    Err(_) => "\r\x1b[2K".to_string(),
                };
                write_terminal(socket, &line).await.ok()?;
                return Some(result);
            }
            _ = tick.tick() => {
                if !received.has_changed().unwrap_or(false) {
                    continue;
                }
                let (done, total) = *received.borrow_and_update();
                let line = match total {
                    Some(total) if done >= total => "Checking and unpacking…".to_string(),
                    Some(total) => format!("{} of {}", megabytes(done), megabytes(total)),
                    None => megabytes(done),
                };
                write_terminal(socket, &format!("\r\x1b[2K{line}")).await.ok()?;
                write_progress(socket, done, total).await.ok()?;
            }
            message = socket.recv() => {
                if matches!(message, None | Some(Err(_)) | Some(Ok(Message::Close(_)))) {
                    return None;
                }
            }
        }
    }
}

/// The same progress as data, for a client that draws a progress bar in place of the terminal.
async fn write_progress(
    socket: &mut WebSocket,
    received: u64,
    total: Option<u64>,
) -> Result<(), axum::Error> {
    let frame = json!({ "type": "progress", "received": received, "total": total });
    socket.send(Message::Text(frame.to_string().into())).await
}

async fn write_terminal(socket: &mut WebSocket, text: &str) -> Result<(), axum::Error> {
    socket
        .send(Message::Binary(text.as_bytes().to_vec().into()))
        .await
}

/// Releases whose update through the user's own updater failed in this process,
/// so a broken updater is not rerun every day for the same release.
static FAILED_UPDATES: std::sync::Mutex<Vec<(&str, semver::Version)>> =
    std::sync::Mutex::new(Vec::new());
const UPDATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The background half of what the Update button does: the user's own install
/// is updated once it falls behind, and orx's copy stands in when that fails or
/// the install doesn't run. An agent the user never installed is left alone.
/// Returns whether the binary in use changed.
pub(super) async fn keep_current(harness: &'static str) -> crate::error::Result<bool> {
    let _setup = managed::lock_installations()?;
    if !matches!(harness, "claude-code" | "codex" | "opencode")
        && managed::installed(harness).is_none()
    {
        return Ok(false);
    }
    let user = managed::user_install(harness).await;
    let copy = managed::installed(harness);
    if user.is_none() && copy.is_none() && !managed::user_install_broken(harness).await {
        return Ok(false);
    }
    let major = (harness == "opencode").then(|| {
        following_major(
            user.as_ref().map(|(_, version)| version),
            copy.as_ref().map(|(_, version)| version),
        )
    });
    let release = managed::latest(harness, major).await?;
    // The user's own install of another major is theirs to keep; it stands
    // neither for nor against orx's copy.
    let user = user.filter(|(_, version)| major.is_none_or(|major| version.major == major));
    if let Some((bin, version)) = &user {
        if *version >= release.version {
            managed::remove_all(harness).await;
            return Ok(copy.is_some());
        }
        let attempted = FAILED_UPDATES
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&(harness, release.version.clone()));
        if !attempted && runs_own_updater(harness, version) {
            let (_, args) = update_command(harness).expect("the selected agent has an updater");
            let mut command = crate::process::tokio_command(bin);
            command
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            crate::local::chat::prepare_env(&mut command);
            // This is the user's own updater, which that variable would switch off.
            command.env_remove("DISABLE_AUTOUPDATER");
            let _ = tokio::time::timeout(UPDATE_TIMEOUT, command.status()).await;
            let updated = managed::user_install(harness)
                .await
                .is_some_and(|(_, version)| version >= release.version);
            if updated {
                managed::remove_all(harness).await;
                return Ok(true);
            }
            FAILED_UPDATES
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push((harness, release.version.clone()));
        }
    }
    if let Some((bin, version)) = &copy {
        if current_copy(version, &release) && managed::verify(harness, bin, version).await.is_ok() {
            return Ok(false);
        }
    }
    managed::install(harness, &release, &mut |_, _| {}).await?;
    Ok(true)
}

/// Whether orx runs the user's own updater for this install. Codex before 0.126
/// has no `update` and would start a chat with the word as its prompt.
fn runs_own_updater(harness: &str, version: &semver::Version) -> bool {
    match harness {
        "claude-code" | "opencode" => true,
        "codex" => *version >= semver::Version::new(0, 126, 0),
        _ => false,
    }
}

/// A copy of another major is no stand-in: OpenCode's majors speak different protocols.
fn current_copy(copy: &semver::Version, release: &managed::Release) -> bool {
    copy.major == release.version.major && *copy >= release.version
}

/// An install that produced an executable where there was none, and that
/// executable runs. Nothing else overrides a nonzero exit: an auth or version
/// reading that merely moved between two live probes is not evidence the
/// command did anything.
fn fresh_install(
    action: Action,
    installed_before: bool,
    harness: Option<&crate::local::harness::HarnessInfo>,
) -> bool {
    matches!(action, Action::Install)
        && !installed_before
        && harness.is_some_and(|h| {
            h.installed && !h.install_broken && h.bin_path.is_some() && h.version.is_some()
        })
}

async fn was_installed(harness: &str) -> bool {
    crate::local::harness::detect_harness(harness)
        .await
        .is_some_and(|h| h.installed && h.bin_path.is_some())
}

/// The ingested reason, plus a fixed phrase naming which predicate failed —
/// `not_ready` alone cannot tell a stale executable from a signed-out CLI.
/// The phrase is one of `safe_error_excerpt`'s allowlisted literals, so it
/// classifies without widening the ingestion schema or leaking local text.
fn verify_reason(
    harness: Option<&crate::local::harness::HarnessInfo>,
) -> (&'static str, &'static str) {
    use crate::local::harness::HarnessAuthState;
    let Some(h) = harness.filter(|h| h.installed) else {
        return ("not_installed", "agent is not installed");
    };
    if h.install_broken {
        return ("not_ready", "agent is installed but failed to run");
    }
    // A local config/database fault reports `Unsupported`, which would otherwise
    // be filed as an out-of-date agent; no command repairs either one.
    if h.needs_config_repair {
        return ("not_ready", "agent configuration needs repair");
    }
    match h.auth_state {
        HarnessAuthState::NeedsLogin => ("not_ready", "agent is not signed in"),
        // A credential the harness will actually send could not be verified.
        HarnessAuthState::Unknown => ("not_ready", "agent sign-in could not be verified"),
        HarnessAuthState::Unsupported => ("not_ready", "agent version is unsupported"),
        HarnessAuthState::Ready if !h.agent_ready => ("not_ready", "agent has no usable model"),
        HarnessAuthState::Ready => ("not_ready", "agent reported no usable state"),
    }
}

/// The OS cause of a spawn failure, with the command line stripped. The error
/// reads `CreateProcessW "powershell.exe …<whole script>…" failed: <cause>`;
/// only the text after the final `failed:` describes what the OS refused.
fn spawn_failure_cause(error: &str) -> String {
    error
        .rsplit_once("failed:")
        .map_or(error, |(_, cause)| cause)
        .trim()
        .to_string()
}

/// Whether the onboarding's unattended setup installs OpenCode, for its free models: while OpenCode is missing or
/// broken, whatever else is installed. A healthy install is only re-checked; `None` when the agents can't be read.
fn automatic_install_needed(payload: &Value) -> Option<bool> {
    let opencode = payload["harnesses"]
        .as_array()?
        .iter()
        .find(|h| h["id"] == "opencode")?;
    Some(opencode["installed"] != true || opencode["installBroken"] == true)
}

fn setup_verified(
    request: &SetupRequest,
    harness: Option<&crate::local::harness::HarnessInfo>,
) -> bool {
    let Some(h) = harness else { return false };
    if !h.installed || h.install_broken || h.adapter_missing {
        return false;
    }
    if request.trigger == Trigger::Automatic {
        return h.agent_ready;
    }
    match request.action {
        Action::Install => true,
        Action::Login => {
            h.authenticated && h.auth_state == crate::local::harness::HarnessAuthState::Ready
        }
        Action::Update => {
            h.version.is_some()
                && h.auth_state != crate::local::harness::HarnessAuthState::Unsupported
        }
        Action::Upgrade => h
            .version
            .as_deref()
            .and_then(managed::parse)
            .is_some_and(|version| version.major >= 2),
    }
}

// The interactive CLI stays open after login; verify auth after its chat prompt appears.
fn antigravity_prompt_ready(output: &str) -> bool {
    output.contains("Antigravity CLI") && output.contains("for shortcuts")
}

pub(super) fn append_output(output: &mut String, bytes: &[u8]) {
    output.push_str(&String::from_utf8_lossy(bytes));
    if output.len() > 65536 {
        let cut = output.ceil_char_boundary(output.len() - 65536);
        output.drain(..cut);
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[tokio::test]
    async fn every_preset_has_a_managed_install_without_a_command() {
        let Json(payload) = commands().await;
        for preset in crate::local::acp::presets() {
            let entry = &payload[&preset.launch.id];
            assert_eq!(entry["install"], "");
            assert_eq!(entry["requiresNpm"], false);
            assert_eq!(
                entry["managedFrom"],
                managed::source(&preset.launch.id).unwrap()
            );
        }
        for id in ["cursor", "antigravity"] {
            assert!(payload[id]["managedFrom"].is_string());
            assert_eq!(payload[id]["install"], "");
        }
    }

    #[test]
    fn antigravity_login_waits_for_chat_prompt() {
        assert!(!antigravity_prompt_ready(
            "Welcome to Antigravity CLI! Choose your color scheme:"
        ));
        assert!(!antigravity_prompt_ready(
            "Antigravity CLI Terms of Service & Data Use [Done]"
        ));
        assert!(antigravity_prompt_ready(
            "Antigravity CLI 1.2.5\naccount@example.com\n? for shortcuts"
        ));
    }

    #[test]
    fn automatic_setup_installs_missing_or_broken_opencode_and_rechecks_healthy_installs() {
        let mut payload = json!({"harnesses": crate::telemetry::harness::IDS.map(|id| json!({"id": id, "installed": false, "installBroken": false}))});
        assert_eq!(automatic_install_needed(&payload), Some(true));
        payload["harnesses"][2]["installed"] = json!(true);
        assert_eq!(automatic_install_needed(&payload), Some(false));
        payload["harnesses"][2]["installBroken"] = json!(true);
        assert_eq!(automatic_install_needed(&payload), Some(true));
        assert_eq!(automatic_install_needed(&json!({})), None);
    }

    #[test]
    fn automatic_setup_installs_opencode_beside_other_agents() {
        let mut entries: Vec<_> = crate::telemetry::harness::IDS
            .iter()
            .map(|id| json!({"id": id, "installed": *id != "opencode", "installBroken": false}))
            .collect();
        entries.push(json!({"id": "acp:565e351e-62c7-488f-a58a-914d30cc5776", "installed": true}));
        assert_eq!(
            automatic_install_needed(&json!({"harnesses": entries})),
            Some(true)
        );
    }

    #[test]
    fn setup_verification_distinguishes_install_login_and_automatic_readiness() {
        let mut h = crate::local::harness::HarnessInfo {
            id: "opencode".into(),
            name: "OpenCode".into(),
            ..blank_harness()
        };
        h.installed = true;
        h.version = Some("1.0.0".into());
        h.auth_state = crate::local::harness::HarnessAuthState::Ready;
        let mut request = SetupRequest {
            harness: "opencode".into(),
            action: Action::Install,
            trigger: Trigger::Manual,
            shell: false,
            provider: None,
            profile: None,
        };
        assert!(setup_verified(&request, Some(&h)));
        request.trigger = Trigger::Automatic;
        assert!(!setup_verified(&request, Some(&h)));
        h.agent_ready = true;
        assert!(setup_verified(&request, Some(&h)));
        request.trigger = Trigger::Manual;
        request.action = Action::Login;
        assert!(!setup_verified(&request, Some(&h)));
        h.authenticated = true;
        assert!(setup_verified(&request, Some(&h)));
        request.action = Action::Update;
        h.auth_state = crate::local::harness::HarnessAuthState::Unsupported;
        assert!(!setup_verified(&request, Some(&h)));
        h.install_broken = true;
        request.action = Action::Install;
        assert!(!setup_verified(&request, Some(&h)));
    }

    /// A blank `HarnessInfo`; `HarnessInfo::new` is private to the harness module.
    fn blank_harness() -> crate::local::harness::HarnessInfo {
        crate::local::harness::HarnessInfo {
            id: "claude-code".into(),
            name: "Claude Code".into(),
            installed: false,
            install_broken: false,
            bin_path: None,
            managed: false,
            version: None,
            authenticated: false,
            auth_state: crate::local::harness::HarnessAuthState::Unknown,
            auth_method: None,
            auth_provider: None,
            login_eligible: false,
            auth_check_failed: false,
            auth_observation: None,
            claude_ultracode: false,
            account: None,
            account_name: None,
            org: None,
            plan: None,
            agent_ready: false,
            agent_note: None,
            setup_url: None,
            auth_env_key: None,
            needs_connection_adapter: false,
            adapter_missing: false,
            logo: None,
            needs_config_repair: false,
            supports_steering: false,
            supports_compaction: false,
            catalog_pending: false,
            default_model: None,
            configured_model: None,
            models: Vec::new(),
            options: crate::local::harness::HarnessOptions::none(),
        }
    }

    #[test]
    fn verify_reason_names_the_failed_predicate() {
        use crate::local::harness::HarnessAuthState as State;
        let mut h = blank_harness();
        assert_eq!(verify_reason(None).0, "not_installed");
        assert_eq!(verify_reason(Some(&h)).0, "not_installed");
        h.installed = true;
        h.install_broken = true;
        assert_eq!(
            verify_reason(Some(&h)),
            ("not_ready", "agent is installed but failed to run")
        );
        h.install_broken = false;
        h.auth_state = State::NeedsLogin;
        assert_eq!(verify_reason(Some(&h)).1, "agent is not signed in");
        // The API/OAuth conflict: distinguishable from a plain signed-out CLI.
        h.auth_state = State::Unknown;
        assert_eq!(
            verify_reason(Some(&h)).1,
            "agent sign-in could not be verified"
        );
        h.auth_state = State::Unsupported;
        assert_eq!(verify_reason(Some(&h)).1, "agent version is unsupported");
        // A refused database also reports Unsupported; it is not a stale install.
        h.needs_config_repair = true;
        assert_eq!(
            verify_reason(Some(&h)).1,
            "agent configuration needs repair"
        );
        h.needs_config_repair = false;
        // Installed, signed in, but OpenCode has no usable model configured.
        h.auth_state = State::Ready;
        assert_eq!(verify_reason(Some(&h)).1, "agent has no usable model");
        h.agent_ready = true;
        assert_eq!(verify_reason(Some(&h)).1, "agent reported no usable state");
    }

    /// `verify_reason` is the one site that computes a reason rather than
    /// writing a literal, so its whole range has to be inside the ingestion
    /// allowlist, and its detail an excerpt the fail-closed filter passes
    /// through. The literals are checked by `record`'s own debug assertions.
    #[test]
    fn computed_verify_reasons_are_accepted_by_the_ingestion_contract() {
        use crate::telemetry::harness::contract;
        let mut h = blank_harness();
        h.installed = true;
        for state in [
            crate::local::harness::HarnessAuthState::Ready,
            crate::local::harness::HarnessAuthState::NeedsLogin,
            crate::local::harness::HarnessAuthState::Unknown,
            crate::local::harness::HarnessAuthState::Unsupported,
        ] {
            for broken in [true, false] {
                for ready in [true, false] {
                    for repair in [true, false] {
                        h.auth_state = state;
                        h.install_broken = broken;
                        h.agent_ready = ready;
                        h.needs_config_repair = repair;
                        let (reason, detail) = verify_reason(Some(&h));
                        assert!(contract::REASONS.contains(&reason), "{reason}");
                        let excerpt = crate::telemetry::harness::excerpt_for_test(detail);
                        assert_eq!(excerpt.as_deref(), Some(detail), "{detail}");
                        assert!(detail.len() <= contract::MAX_ERROR_EXCERPT);
                    }
                }
            }
        }
        assert_eq!(verify_reason(None).0, "not_installed");
    }

    /// Only an install that produced a runnable binary where there was none
    /// may override a nonzero exit; everything else stays a failure.
    #[test]
    fn only_a_brand_new_working_install_overrides_a_failed_command() {
        let mut h = blank_harness();
        h.installed = true;
        h.bin_path = Some("/usr/local/bin/codex".into());
        h.version = Some("1.0.0".into());
        assert!(fresh_install(Action::Install, false, Some(&h)));
        // The same binary was already there: a failed command proves nothing.
        assert!(!fresh_install(Action::Install, true, Some(&h)));
        // A failed update or an auth reading that moved is not an install.
        assert!(!fresh_install(Action::Update, false, Some(&h)));
        assert!(!fresh_install(Action::Login, false, Some(&h)));
        // Installed but not usable: no version answered, or it fails to run.
        h.version = None;
        assert!(!fresh_install(Action::Install, false, Some(&h)));
        h.version = Some("1.0.0".into());
        h.install_broken = true;
        assert!(!fresh_install(Action::Install, false, Some(&h)));
        assert!(!fresh_install(Action::Install, false, None));
    }

    #[test]
    fn spawn_failure_keeps_the_os_cause_and_drops_the_script() {
        // The whole install script is in the error; its unexecuted
        // `Unsupported architecture` branch must not become the excerpt.
        let error = "CreateProcessW \"powershell.exe -NoProfile -Command \
             switch ($arch) { default { throw \\\"Unsupported architecture\\\" } }\" \
             in \"C:\\\\\" failed: The system cannot find the file specified. (os error 2)";
        let cause = spawn_failure_cause(error);
        assert_eq!(
            cause,
            "The system cannot find the file specified. (os error 2)"
        );
        assert!(!cause.contains("Unsupported architecture"));
        assert_eq!(
            crate::telemetry::harness::excerpt_for_test(&cause).as_deref(),
            Some("The system cannot find the file specified")
        );
        // An error with no `failed:` marker is kept whole rather than dropped.
        assert_eq!(
            spawn_failure_cause("permission denied"),
            "permission denied"
        );
    }

    #[test]
    fn output_tail_is_bounded_and_preserves_utf8() {
        let mut output = "é".repeat(40000);
        append_output(&mut output, b"Permission denied");
        assert!(output.len() <= 65536);
        assert!(output.ends_with("Permission denied"));
    }

    #[test]
    fn only_opencode_sign_in_takes_a_provider() {
        let request = |value| serde_json::from_value::<SetupRequest>(value).unwrap();
        assert!(provider_allowed(&request(json!({
            "harness": "opencode", "action": "login", "provider": "xai"
        }))));
        assert!(provider_allowed(&request(json!({
            "harness": "codex", "action": "login"
        }))));
        assert!(!provider_allowed(&request(json!({
            "harness": "codex", "action": "login", "provider": "xai"
        }))));
        assert!(!provider_allowed(&request(json!({
            "harness": "opencode", "action": "install", "provider": "xai"
        }))));
        assert!(!provider_allowed(&request(json!({
            "harness": "opencode", "action": "login", "provider": "--help"
        }))));
    }

    #[test]
    fn only_a_sign_in_to_its_own_agent_takes_a_profile() {
        let request = |value| serde_json::from_value::<SetupRequest>(value).unwrap();
        assert!(profile_allowed(&request(json!({
            "harness": "claude-code", "action": "login", "profile": "claude-ab12cd34ef"
        }))));
        assert!(profile_allowed(&request(json!({
            "harness": "codex", "action": "login", "profile": "codex-ab12cd34ef"
        }))));
        assert!(!profile_allowed(&request(json!({
            "harness": "codex", "action": "login", "profile": "claude-ab12cd34ef"
        }))));
        assert!(!profile_allowed(&request(json!({
            "harness": "claude-code", "action": "install", "profile": "claude-ab12cd34ef"
        }))));
        assert!(!profile_allowed(&request(json!({
            "harness": "claude-code", "action": "login", "profile": "../claude-ab12cd34ef"
        }))));
    }

    #[test]
    fn a_codex_without_update_is_never_handed_the_word_as_a_prompt() {
        let version = |text| semver::Version::parse(text).unwrap();
        assert!(!runs_own_updater("codex", &version("0.125.0")));
        assert!(runs_own_updater("codex", &version("0.126.0")));
        assert!(runs_own_updater("claude-code", &version("2.1.0")));
        assert!(!runs_own_updater("cursor", &version("2026.1.1")));
    }

    #[test]
    fn a_chosen_opencode_2_copy_outranks_the_users_own_1() {
        let version = |text| semver::Version::parse(text).unwrap();
        assert_eq!(following_major(Some(&version("1.19.5")), None), 1);
        assert_eq!(
            following_major(Some(&version("1.19.5")), Some(&version("2.2.5"))),
            2
        );
        assert_eq!(
            following_major(Some(&version("2.2.5")), Some(&version("1.19.5"))),
            2
        );
        assert_eq!(following_major(None, Some(&version("1.19.4"))), 1);
        assert_eq!(following_major(None, None), 2);
    }
}
