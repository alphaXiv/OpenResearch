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
use crate::telemetry::harness::SetupAttempt;

/// URL and interpreter, kept apart from the rendered command so the runner can
/// fetch to a file: piping the script in makes stdin a pipe and codex exits 1.
fn unix_bootstrap(harness: &str) -> Option<(&'static str, &'static str)> {
    match harness {
        "claude-code" => Some(("https://claude.ai/install.sh", "bash")),
        "codex" => Some(("https://chatgpt.com/codex/install.sh", "sh")),
        "opencode" => Some(("https://opencode.ai/install", "bash")),
        "antigravity" => Some(("https://antigravity.google/cli/install.sh", "bash")),
        "cursor" => Some(("https://cursor.com/install", "bash")),
        _ => None,
    }
}

/// The Windows command, run through PowerShell as written.
fn windows_install_command(harness: &str) -> Option<&'static str> {
    match harness {
        "claude-code" => Some("irm https://claude.ai/install.ps1 | iex"),
        "codex" => Some("irm https://chatgpt.com/codex/install.ps1 | iex"),
        "opencode" => Some(include_str!("install_opencode.ps1")),
        "antigravity" => Some("irm https://antigravity.google/cli/install.ps1 | iex"),
        "cursor" => Some("irm 'https://cursor.com/install?win32=true' | iex"),
        _ => None,
    }
}

/// What the approval dialog shows: the same source and interpreter the runner
/// uses, without its timeout and cleanup flags. Temp destination, since a
/// relative path could clobber.
fn install_command(harness: &str, windows: bool) -> Option<String> {
    if windows {
        return windows_install_command(harness).map(str::to_string);
    }
    let (url, interpreter) = unix_bootstrap(harness)?;
    Some(format!(
        "script=$(mktemp) && curl -fsSL {url} -o \"$script\" && {interpreter} \"$script\""
    ))
}

/// Fetch the bootstrap to a temp file, then run it with the PTY as stdin.
/// `|| exit $?` keeps curl's own code, so a 403 and a timeout stay apart.
#[cfg(not(windows))]
fn unix_install_script(url: &str, interpreter: &str) -> String {
    format!(
        "set -e\n\
         script=$(mktemp \"${{TMPDIR:-/tmp}}/orx-install.XXXXXX\")\n\
         trap 'rm -f \"$script\"' EXIT\n\
         curl -fsSL --retry 2 --retry-connrefused --connect-timeout 20 --max-time 300 \
         '{url}' -o \"$script\" || exit $?\n\
         {interpreter} \"$script\"\n"
    )
}

/// An absolute Windows PowerShell, used when PATH has no `powershell.exe`.
/// A PATH missing `…\\System32\\WindowsPowerShell\\v1.0` otherwise fails every
/// Windows install with `CreateProcessW … (os error 2)` before any installer runs.
#[cfg(windows)]
fn windows_powershell() -> String {
    if crate::local::shell_env::find_on_path("powershell.exe").is_some() {
        return "powershell.exe".into();
    }
    let system_root = crate::local::shell_env::var("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows"));
    let absolute = system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    if absolute.is_file() {
        return absolute.to_string_lossy().into_owned();
    }
    // Nothing better to try; the spawn error names the real missing file.
    "powershell.exe".into()
}

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
        if let (Some(install), Some((login, _)), Some((update, _))) = (
            install_command(harness, cfg!(windows)),
            login_command(harness),
            update_command(harness),
        ) {
            result.insert(
                harness.into(),
                json!({
                    "install": install,
                    // The vendor bootstrap URL, so the UI can recognise an
                    // install note that quotes the one-liner form of it. Unix
                    // only: on Windows `install` is the PowerShell form, so a
                    // note quoting this URL would run a different command.
                    "installUrl": (!cfg!(windows))
                        .then(|| unix_bootstrap(harness).map(|(url, _)| url))
                        .flatten(),
                    "login": login,
                    "update": update,
                    "requiresNpm": cfg!(windows) && install.starts_with("npm "),
                }),
            );
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
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Login => "login",
            Self::Update => "update",
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
    // On Unix this also rejects anything `unix_bootstrap` does not know, since
    // that is the table it renders from.
    if install_command(&request.harness, cfg!(windows)).is_none() {
        return bad_request("Unknown coding agent").into_response();
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
        // Login rejection caches must not mask a successful new login.
        if request.harness == "claude-code" {
            state.claude.clear_runtime_rejection();
        }
        *state.harnesses.lock().await = None;
        let message = match result {
            Ok(()) => json!({ "type": "complete" }),
            Err(error) => json!({ "type": "error", "error": error }),
        };
        if socket
            .send(Message::Text(message.to_string().into()))
            .await
            .is_err()
            || !request.shell
        {
            return;
        }
        if let Some(env) = follow_up {
            super::continue_in_shell(&mut socket, &mut size, env).await;
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
    follow_up: &mut Option<Vec<(&'static str, std::ffi::OsString)>>,
) -> Result<(), String> {
    let mut run_command = true;
    // Held, not terminal: an install whose optional launch failed still counts,
    // but only against evidence this command changed something.
    let mut command_failure: Option<CommandFailure> = None;
    // Only an install can be credited past a nonzero exit, and only if it put a
    // binary where there was none; login/update need no baseline.
    let installed_before = match request.action {
        Action::Install => was_installed(&request.harness).await,
        Action::Login | Action::Update => true,
    };
    if request.trigger == Trigger::Automatic {
        let axum::Json(payload) = super::list_harnesses(
            State(state.clone()),
            Query(super::HarnessQuery {
                refresh: Some(1),
                retry: None,
            }),
        )
        .await;
        let Some(install) = automatic_install_needed(&payload) else {
            // Nothing was attempted and nothing is wrong: another agent is
            // already installed, so the automatic flow stands down.
            attempt.record("interrupted", "detect", Some("not_eligible"), None, None);
            return Err("Another coding agent is installed. Re-check your coding agents.".into());
        };
        run_command = install;
    }
    if run_command {
        let (program, args) = match request.action {
            #[cfg(windows)]
            Action::Install => {
                let install = install_command(&request.harness, true).unwrap();
                (
                    windows_powershell(),
                    vec![
                        "-NoProfile".into(),
                        "-Command".into(),
                        // `$LASTEXITCODE` persists from an earlier native call,
                        // so reset it or a failing cmdlet exits with its code.
                        format!(
                            "$LASTEXITCODE = 0\n{install}\nif (-not $?) {{ if ($LASTEXITCODE) {{ exit $LASTEXITCODE }} else {{ exit 1 }} }}"
                        ),
                    ],
                )
            }
            #[cfg(not(windows))]
            Action::Install => {
                // The PTY is this shell's stdin, so the vendor script — and
                // anything it launches — reads the real terminal.
                let (url, interpreter) = unix_bootstrap(&request.harness).unwrap();
                (
                    "bash".to_string(),
                    vec!["-c".into(), unix_install_script(url, interpreter)],
                )
            }
            Action::Login | Action::Update => {
                let Some(bin) = crate::local::harness::detect_harness(&request.harness)
                    .await
                    .and_then(|h| h.bin_path)
                else {
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
        // The lease ends with `run`; the follow-up shell only inherits the path.
        let env: Vec<_> = lease
            .as_ref()
            .map(|lease| vec![("OPENCODE_DB", lease.path().as_os_str().to_owned())])
            .unwrap_or_default();
        attempt.record("command_started", "command", None, None, None);
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
        let completed =
            if request.harness == "antigravity" && matches!(request.action, Action::Login) {
                Some(antigravity_prompt_ready as fn(&str) -> bool)
            } else {
                None
            };
        match super::relay_pty(socket, session, Some(&mut output), size, completed).await {
            Some(Ok(status)) if status.success() => attempt.record(
                "command_completed",
                "command",
                None,
                Some(status.exit_code()),
                None,
            ),
            // Not recorded here: one attempt must not emit both `failed` and
            // `succeeded`, and what it achieved is only known below.
            Some(Ok(status)) => {
                command_failure = Some(CommandFailure {
                    message: format!("Command exited with code {}", status.exit_code()),
                    exit_code: status.exit_code(),
                    output: std::mem::take(&mut output),
                });
            }
            Some(Err(error)) => {
                attempt.record(
                    "failed",
                    "command",
                    Some("terminal_error"),
                    None,
                    Some(&output),
                );
                return Err(error);
            }
            None => {
                attempt.record("interrupted", "command", Some("disconnected"), None, None);
                return Err("Terminal disconnected".into());
            }
        }
    }
    let harness = crate::local::harness::detect_harness(&request.harness).await;
    let verified = setup_verified(request, harness.as_ref());
    if let Some(failure) = command_failure {
        // Only a brand-new working install overrides the failure (codex
        // installs, then its optional launch fails); a binary that was already
        // there proves nothing about a command that exited nonzero.
        if verified && fresh_install(request.action, installed_before, harness.as_ref()) {
            // One terminal event, with the failed step still visible on it.
            attempt.record(
                "succeeded",
                "verify",
                None,
                Some(failure.exit_code),
                Some(&failure.output),
            );
            return Ok(());
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
    if verified {
        attempt.record("succeeded", "verify", None, None, None);
        return Ok(());
    }
    let (reason, detail) = verify_reason(harness.as_ref());
    attempt.record("failed", "verify", Some(reason), None, Some(detail));
    Err(harness
        .and_then(|h| h.agent_note)
        .unwrap_or_else(|| "Could not verify coding agent setup. Re-check and retry.".into()))
}

/// A nonzero exit, held until the attempt's one terminal outcome is decided.
struct CommandFailure {
    message: String,
    exit_code: u32,
    output: String,
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

fn automatic_install_needed(payload: &Value) -> Option<bool> {
    let entries = payload["harnesses"].as_array()?;
    let opencode = entries.iter().find(|h| h["id"] == "opencode")?;
    if opencode["installed"] == true && opencode["installBroken"] == false {
        return Some(false);
    }
    crate::telemetry::harness::IDS
        .iter()
        .all(|id| {
            entries.iter().any(|h| {
                h["id"] == *id
                    && ((h["installed"] == false && h["installBroken"] == false)
                        || (*id == "opencode" && h["installBroken"] == true))
            })
        })
        .then_some(true)
}

fn setup_verified(
    request: &SetupRequest,
    harness: Option<&crate::local::harness::HarnessInfo>,
) -> bool {
    let Some(h) = harness else { return false };
    if !h.installed || h.install_broken {
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
    fn automatic_setup_reinstalls_broken_opencode_and_rechecks_healthy_installs() {
        let mut payload = json!({"harnesses": crate::telemetry::harness::IDS.map(|id| json!({"id": id, "installed": false, "installBroken": false}))});
        assert_eq!(automatic_install_needed(&payload), Some(true));
        payload["harnesses"][2]["installed"] = json!(true);
        assert_eq!(automatic_install_needed(&payload), Some(false));
        payload["harnesses"][2]["installBroken"] = json!(true);
        assert_eq!(automatic_install_needed(&payload), Some(true));
        payload["harnesses"][0]["installed"] = json!(true);
        assert_eq!(automatic_install_needed(&payload), None);
        assert_eq!(automatic_install_needed(&json!({})), None);
        let payload = json!({"harnesses": crate::telemetry::harness::IDS.map(|id| json!({"id": id, "installed": id == "antigravity", "installBroken": false}))});
        assert_eq!(automatic_install_needed(&payload), None);
    }

    #[test]
    fn setup_verification_distinguishes_install_login_and_automatic_readiness() {
        let mut h = crate::local::harness::HarnessInfo {
            id: "opencode",
            name: "OpenCode",
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
            id: "claude-code",
            name: "Claude Code",
            installed: false,
            install_broken: false,
            bin_path: None,
            version: None,
            authenticated: false,
            auth_state: crate::local::harness::HarnessAuthState::Unknown,
            auth_method: None,
            account: None,
            org: None,
            plan: None,
            agent_ready: false,
            agent_note: None,
            needs_config_repair: false,
            supports_steering: false,
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

    #[cfg(not(windows))]
    #[test]
    fn unix_install_fetches_to_a_file_so_the_installer_owns_the_terminal() {
        let (url, interpreter) = unix_bootstrap("codex").unwrap();
        let script = unix_install_script(url, interpreter);
        // Piping into the interpreter is what gave the launched codex a pipe
        // for stdin ("stdin is not a terminal", exit 1).
        assert!(!script.contains("| sh"));
        assert!(script.contains("-o \"$script\""));
        assert!(script.contains("sh \"$script\""));
        // curl's own exit code must survive the wrapper.
        assert!(script.contains("|| exit $?"));
        // Bounded transient retries, with an explicit ceiling.
        assert!(script.contains("--retry 2") && script.contains("--max-time 300"));
        assert!(script.contains("trap 'rm -f \"$script\"' EXIT"));
        for harness in crate::telemetry::harness::IDS {
            let (url, interpreter) = unix_bootstrap(harness).expect(harness);
            // The approved command must describe the one that runs: same URL,
            // same interpreter, and no pipeline we no longer use.
            let shown = install_command(harness, false).expect(harness);
            assert!(shown.contains(url), "{harness}: {shown}");
            assert!(
                shown.contains(&format!("{interpreter} \"$script\"")),
                "{harness}: {shown}"
            );
            assert!(!shown.contains('|'), "{harness}: {shown}");
            // Pasting the shown command must not overwrite a file in the user's
            // working directory: the only `-o` destination is the temp file.
            // (Asserting on `install.sh` would match the vendor URL itself.)
            assert!(shown.contains("-o \"$script\""), "{harness}: {shown}");
            assert_eq!(shown.matches(" -o ").count(), 1, "{harness}: {shown}");
            assert!(shown.contains("script=$(mktemp)"), "{harness}: {shown}");
            let ran = unix_install_script(url, interpreter);
            assert!(ran.contains(url) && ran.contains(interpreter), "{harness}");
        }
        assert!(unix_bootstrap("sh; touch /tmp/injected").is_none());
    }

    #[test]
    fn output_tail_is_bounded_and_preserves_utf8() {
        let mut output = "é".repeat(40000);
        append_output(&mut output, b"Permission denied");
        assert!(output.len() <= 65536);
        assert!(output.ends_with("Permission denied"));
    }

    #[test]
    fn setup_accepts_only_known_agents_and_actions() {
        for harness in crate::telemetry::harness::IDS {
            assert!(install_command(harness, false).is_some());
            assert!(install_command(harness, true).is_some());
            assert!(login_command(harness).is_some());
            assert!(update_command(harness).is_some());
        }
        assert!(install_command("codex; touch /tmp/injected", false).is_none());
        assert!(!install_command("codex", true).unwrap().starts_with("npm "));
        assert!(login_command("sh").is_none());
        assert!(update_command("sh").is_none());
        assert_eq!(update_command("opencode").unwrap().1, vec!["upgrade"]);
        assert!(matches!(
            serde_json::from_value::<SetupRequest>(json!({
                "harness": "codex", "action": "update"
            }))
            .unwrap()
            .action,
            Action::Update
        ));
        assert!(serde_json::from_value::<SetupRequest>(json!({
            "harness": "codex", "action": "exec"
        }))
        .is_err());
        assert!(
            !serde_json::from_value::<SetupRequest>(json!({
                "harness": "codex", "action": "update"
            }))
            .unwrap()
            .shell
        );
        let uri: axum::http::Uri = "/api/harnesses/setup?harness=codex&action=login&shell=true"
            .parse()
            .unwrap();
        let Query(request) = Query::<SetupRequest>::try_from_uri(&uri).unwrap();
        assert!(request.shell && matches!(request.action, Action::Login));
    }
}
