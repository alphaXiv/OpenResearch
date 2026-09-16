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

fn install_command(harness: &str, windows: bool) -> Option<&'static str> {
    match (harness, windows) {
        ("claude-code", false) => Some("curl -fsSL https://claude.ai/install.sh | bash"),
        ("claude-code", true) => Some("irm https://claude.ai/install.ps1 | iex"),
        ("codex", false) => Some("curl -fsSL https://chatgpt.com/codex/install.sh | sh"),
        ("codex", true) => Some("npm install -g @openai/codex"),
        ("opencode", false) => Some("curl -fsSL https://opencode.ai/install | bash"),
        ("opencode", true) => Some("npm install -g opencode-ai"),
        ("cursor", false) => Some("curl https://cursor.com/install -fsS | bash"),
        ("cursor", true) => Some("irm 'https://cursor.com/install?win32=true' | iex"),
        _ => None,
    }
}

fn login_command(harness: &str) -> Option<(&'static str, Vec<String>)> {
    match harness {
        "claude-code" => Some(("claude auth login", vec!["auth".into(), "login".into()])),
        "codex" => Some(("codex login", vec!["login".into()])),
        "opencode" => Some(("opencode auth login", vec!["auth".into(), "login".into()])),
        "cursor" => Some(("agent login", vec!["login".into()])),
        _ => None,
    }
}

fn update_command(harness: &str) -> Option<(&'static str, Vec<String>)> {
    match harness {
        "claude-code" => Some(("claude update", vec!["update".into()])),
        "codex" => Some(("codex update", vec!["update".into()])),
        "opencode" => Some(("opencode upgrade", vec!["upgrade".into()])),
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
                    "login": login,
                    "update": update,
                    "requiresNpm": cfg!(windows) && matches!(harness, "codex" | "opencode"),
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
    if !super::same_origin(&headers) {
        return super::ApiError(
            axum::http::StatusCode::FORBIDDEN,
            "Setup terminal origin rejected".into(),
        )
        .into_response();
    }
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
        let mut ran = false;
        let result = run(&state, &mut socket, &request, &attempt, &mut size, &mut ran).await;
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
            || !ran
        {
            return;
        }
        let (shell, args) = super::interactive_shell();
        match super::spawn_pty(shell, args, size).await {
            Ok(session) => {
                super::relay_pty(&mut socket, session, None, &mut size).await;
            }
            Err(error) => super::send_terminal_error(&mut socket, error).await,
        }
    })
}

async fn run(
    state: &AppState,
    socket: &mut WebSocket,
    request: &SetupRequest,
    attempt: &SetupAttempt,
    size: &mut PtySize,
    ran: &mut bool,
) -> Result<(), String> {
    let mut run_command = true;
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
            attempt.record("failed", "detect", Some("not_eligible"), None, None);
            return Err("Another coding agent is installed. Re-check your coding agents.".into());
        };
        run_command = install;
    }
    if run_command {
        let install = install_command(&request.harness, cfg!(windows)).unwrap();
        let (program, args) = match request.action {
            Action::Install if cfg!(windows) => (
                "powershell.exe".to_string(),
                vec![
                    "-NoProfile".into(),
                    "-Command".into(),
                    format!("{install}; if (-not $?) {{ exit 1 }}"),
                ],
            ),
            Action::Install => (
                "bash".to_string(),
                vec!["-o".into(), "pipefail".into(), "-c".into(), install.into()],
            ),
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
                    attempt.record("failed", "detect", Some("not_ready"), None, None);
                    return Err(error.to_string());
                }
            }
        } else {
            None
        };
        let env: Vec<_> = lease
            .as_ref()
            .map(|lease| vec![("OPENCODE_DB", lease.path().as_os_str().to_owned())])
            .unwrap_or_default();
        attempt.record("command_started", "command", None, None, None);
        let pty_size = *size;
        let session = match tokio::task::spawn_blocking(move || {
            super::start_pty_with_env(&program, args, &env, pty_size)
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result.map_err(|error| error.to_string()))
        {
            Ok(session) => session,
            Err(error) => {
                attempt.record(
                    "failed",
                    "command",
                    Some("spawn_failed"),
                    None,
                    Some(&error),
                );
                return Err(error);
            }
        };
        *ran = true;
        let mut output = String::new();
        match super::relay_pty(socket, session, Some(&mut output), size).await {
            Some(Ok(status)) if status.success() => attempt.record(
                "command_completed",
                "command",
                None,
                Some(status.exit_code()),
                None,
            ),
            Some(Ok(status)) => {
                attempt.record(
                    "failed",
                    "command",
                    Some("exit_nonzero"),
                    Some(status.exit_code()),
                    Some(&output),
                );
                return Err(format!("Command exited with code {}", status.exit_code()));
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
    if setup_verified(request, harness.as_ref()) {
        attempt.record("succeeded", "verify", None, None, None);
        Ok(())
    } else {
        attempt.record("failed", "verify", Some("not_ready"), None, None);
        Err(harness
            .and_then(|h| h.agent_note)
            .unwrap_or_else(|| "Could not verify coding agent setup. Re-check and retry.".into()))
    }
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
    }

    #[test]
    fn setup_verification_distinguishes_install_login_and_automatic_readiness() {
        let mut h = crate::local::harness::HarnessInfo {
            id: "opencode",
            name: "OpenCode",
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
            supports_steering: false,
            models: Vec::new(),
            options: crate::local::harness::HarnessOptions::none(),
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
            serde_json::from_value::<SetupRequest>(json!({
                "harness": "codex", "action": "login", "shell": true
            }))
            .unwrap()
            .shell
        );
    }
}
