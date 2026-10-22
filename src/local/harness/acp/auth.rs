use super::super::detect::{detect_spawn_output_timed, HarnessAuthState};
use super::super::HarnessInfo;
use crate::local::acp::{executable_path, Definition};
use serde_json::Value;
use std::time::Duration;

mod config_auth;
use config_auth::Context;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Status {
    Configured,
    Missing,
    Expired,
    Blocked,
    Unknown,
    Keyring,
}

pub(super) async fn detect(definition: &Definition, info: &mut HarnessInfo) {
    let logo = info.logo.clone().unwrap_or_default();
    let context = Context::current();
    let custom_arguments = logo != "dsh"
        && crate::local::acp::presets()
            .iter()
            .find(|preset| preset.launch.executable == logo)
            .is_some_and(|preset| preset.launch.arguments != definition.arguments);
    let status = if custom_arguments {
        Status::Unknown
    } else {
        match logo.as_str() {
            "dsh" => deepseek(definition).await,
            "devin" => native_output(&definition.executable, &["auth", "status"])
                .await
                .map(|output| devin_status(&output.stdout))
                .unwrap_or(Status::Unknown),
            "kimi" => {
                let status = config_auth::kimi(&context);
                if status == Status::Keyring {
                    kimi_keyring(definition, &context).await
                } else {
                    status
                }
            }
            "mcode" => {
                match native_output(&definition.executable, &["provider", "list", "--json"])
                    .await
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes.stdout).ok())
                {
                    Some(snapshot) => minimax_status(&snapshot, &context),
                    None => Status::Unknown,
                }
            }
            "pi-acp" => native_output("pi", &["--list-models"])
                .await
                .map(|output| {
                    pi_status(if output.stdout.is_empty() {
                        &output.stderr
                    } else {
                        &output.stdout
                    })
                })
                .unwrap_or(Status::Unknown),
            "zcode-acp-server" => {
                if executable_path(&definition.executable).is_some() {
                    zcode(definition).await
                } else {
                    config_auth::zcode(&context)
                }
            }
            "grok" => config_auth::grok(&context),
            _ => Status::Unknown,
        }
    };
    if logo == "dsh" && matches!(status, Status::Configured | Status::Missing) {
        info.auth_env_key = Some("DEEPSEEK_API_KEY");
    }
    let setup = match logo.as_str() {
        "dsh" => "Configure the selected provider in DeepSeek Harness's Models settings. The default ACP provider needs DEEPSEEK_API_KEY.",
        "devin" => "Run `devin auth login`, then re-check.",
        "kimi" => "Run `kimi login`, or configure the selected provider's API key, then re-check.",
        "mcode" => "Run `mcode login --region global`, or configure a provider with `mcode provider`, then re-check.",
        "pi-acp" => "Run `pi` and use `/login`, or configure a provider API key or local model, then re-check.",
        "zcode-acp-server" => "Connect an account or model provider in ZCode's model settings, then re-check.",
        "grok" => "Run `grok login`, or configure XAI_API_KEY if your provider allows it, then re-check.",
        _ => "Use the agent's own setup instructions, then re-check.",
    };
    match status {
        Status::Configured => {
            info.auth_state = HarnessAuthState::Ready;
            info.agent_note = Some("Provider credentials or model configuration found. Provider acceptance and available credits have not been checked.".into());
        }
        Status::Missing | Status::Expired | Status::Blocked => {
            info.agent_ready = false;
            info.auth_state = HarnessAuthState::NeedsLogin;
            let reason = match status {
                Status::Expired => "Saved login has expired.",
                Status::Blocked => "The configured provider reports unavailable model access.",
                _ => "Authentication or model setup needed.",
            };
            info.agent_note = Some(format!("{reason} {setup}"));
        }
        Status::Unknown | Status::Keyring => {
            info.auth_check_failed = true;
            info.auth_state = HarnessAuthState::Unknown;
            info.agent_note = Some(format!("Authentication could not be verified. {setup}"));
        }
    }
}

async fn native_output(executable: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut command = tokio::process::Command::new(executable_path(executable)?);
    command.args(args).stdin(std::process::Stdio::null());
    crate::local::chat::prepare_env(&mut command);
    command.env("NO_COLOR", "1");
    let output = detect_spawn_output_timed(command, Duration::from_secs(20))
        .await?
        .ok()?;
    output.status.success().then_some(output)
}

async fn deepseek(definition: &Definition) -> Status {
    let Some(node) = executable_path("node") else {
        return Status::Unknown;
    };
    let Some(executable) = executable_path(&definition.executable) else {
        return Status::Unknown;
    };
    let mut command = tokio::process::Command::new(node);
    command.args(["-e", include_str!("deepseek-auth.cjs")]);
    command
        .arg(executable)
        .arg(serde_json::to_string(&definition.arguments).unwrap_or_default());
    command.stdin(std::process::Stdio::null());
    crate::local::chat::prepare_env(&mut command);
    match detect_spawn_output_timed(command, Duration::from_secs(15)).await {
        Some(Ok(output)) if output.status.success() => match output.stdout.as_slice() {
            b"configured" => Status::Configured,
            b"missing" => Status::Missing,
            _ => Status::Unknown,
        },
        _ => Status::Unknown,
    }
}

async fn zcode(definition: &Definition) -> Status {
    let Some(node) = executable_path("node") else {
        return Status::Unknown;
    };
    let Some(executable) = executable_path(&definition.executable) else {
        return Status::Unknown;
    };
    let mut command = tokio::process::Command::new(node);
    let script = format!(
        "{{ {} }}\n{}",
        include_str!("../../acp/zcode-runtime.cjs"),
        include_str!("zcode-auth.cjs")
    );
    command.args(["-e", &script]).arg(executable);
    command.stdin(std::process::Stdio::null());
    crate::local::chat::prepare_env(&mut command);
    match detect_spawn_output_timed(command, Duration::from_secs(10)).await {
        Some(Ok(output)) if output.status.success() => match output.stdout.as_slice() {
            b"configured" => Status::Configured,
            b"missing" => Status::Missing,
            b"blocked" => Status::Blocked,
            b"expired" => Status::Expired,
            _ => Status::Unknown,
        },
        _ => Status::Unknown,
    }
}

async fn kimi_keyring(definition: &Definition, context: &Context) -> Status {
    let Some(key) = config_auth::kimi_keyring_key(context) else {
        return Status::Unknown;
    };
    let interpreter = executable_path(&definition.executable)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|script| {
            script
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("#!"))
                .map(str::to_owned)
        })
        .filter(|path| !path.contains(' ') && path.contains("python"))
        .map(std::path::PathBuf::from)
        .or_else(|| executable_path("python3"));
    let Some(interpreter) = interpreter else {
        return Status::Unknown;
    };
    let mut command = tokio::process::Command::new(interpreter);
    command.args(["-c", "import json,keyring,sys; from kimi_cli.auth.oauth import KEYRING_SERVICE; raw=keyring.get_password(KEYRING_SERVICE,sys.argv[1]); print(json.dumps(json.loads(raw) if raw else {}))"]);
    command.arg(key).stdin(std::process::Stdio::null());
    crate::local::chat::prepare_env(&mut command);
    // Credentials stay in memory; only the resulting readiness status reaches the API.
    match detect_spawn_output_timed(command, Duration::from_secs(10)).await {
        Some(Ok(output)) if output.status.success() => serde_json::from_slice(&output.stdout)
            .map(|token| config_auth::oauth_status(&token))
            .unwrap_or(Status::Unknown),
        _ => Status::Unknown,
    }
}

fn devin_status(output: &[u8]) -> Status {
    let output = String::from_utf8_lossy(output);
    if output.lines().any(|line| line.trim() == "Not logged in.") {
        Status::Missing
    } else if output.lines().any(|line| {
        line.trim() == "Logged in."
            || line.trim() == "Logged in (via Devin)."
            || line.trim().starts_with("Logged in as ")
    }) {
        Status::Configured
    } else {
        Status::Unknown
    }
}

fn pi_status(output: &[u8]) -> Status {
    let output = String::from_utf8_lossy(output);
    if output
        .lines()
        .any(|line| line.starts_with("No models available."))
    {
        return Status::Missing;
    }
    let mut lines = output.lines();
    if lines.any(|line| {
        line.split_whitespace().collect::<Vec<_>>()
            == [
                "provider", "model", "context", "max-out", "thinking", "images",
            ]
    }) && lines.any(|line| line.split_whitespace().count() == 6)
    {
        Status::Configured
    } else {
        Status::Unknown
    }
}

fn minimax_status(snapshot: &Value, context: &Context) -> Status {
    let Some(providers) = snapshot["providers"].as_array() else {
        return Status::Unknown;
    };
    let Some(provider) = providers
        .iter()
        .find(|p| {
            p["active"] == true && matches!(p["kind"].as_str(), Some("custom" | "codex-oauth"))
        })
        .or_else(|| providers.iter().find(|p| p["active"] == true))
    else {
        return Status::Missing;
    };
    if provider["enabled"] == false {
        return Status::Blocked;
    }
    if matches!(
        provider["status"]["lastErrorCode"].as_str(),
        Some("unauthorized" | "invalid_api_key" | "insufficient_quota")
    ) {
        return Status::Blocked;
    }
    match provider["kind"].as_str() {
        Some("minimax-oauth") => config_auth::minimax_login(context),
        Some("codex-oauth") => match provider["status"]["state"].as_str() {
            Some("connected") => Status::Configured,
            Some("disconnected" | "failed") => Status::Missing,
            _ => Status::Unknown,
        },
        Some("custom" | "minimax-api-key") => match provider["hasApiKey"].as_bool() {
            Some(true) => Status::Configured,
            Some(false) => Status::Missing,
            None => Status::Unknown,
        },
        _ => Status::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn minimax_requires_the_active_providers_credentials() {
        let context = Context::current();
        let snapshot = serde_json::json!({"providers": [
            {"active": false, "enabled": true, "kind": "custom", "hasApiKey": true},
            {"active": true, "enabled": true, "kind": "custom", "hasApiKey": false}
        ]});
        assert_eq!(minimax_status(&snapshot, &context), Status::Missing);
        assert_eq!(
            minimax_status(
                &serde_json::json!({"providers": [
                    {"active":true,"enabled":true,"kind":"minimax-oauth","hasApiKey":false},
                    {"active":true,"enabled":true,"kind":"custom","hasApiKey":true}
                ]}),
                &context
            ),
            Status::Configured
        );
        assert_eq!(
            minimax_status(
                &serde_json::json!({"providers": [{"active":true,"enabled":false,"kind":"custom","hasApiKey":true}]}),
                &context
            ),
            Status::Blocked
        );
        assert_eq!(
            minimax_status(
                &serde_json::json!({"providers": [{"active":true,"enabled":true,"kind":"custom","hasApiKey":true}]}),
                &context
            ),
            Status::Configured
        );
        assert_eq!(
            minimax_status(&serde_json::json!({"error":"transport"}), &context),
            Status::Unknown
        );
    }
    #[test]
    fn native_status_requires_explicit_results() {
        assert_eq!(
            devin_status(b"Not logged in.\nRun `devin auth login` to authenticate.\n"),
            Status::Missing
        );
        assert_eq!(
            devin_status(b"Logged in as test@example.com\n"),
            Status::Configured
        );
        assert_eq!(
            devin_status(b"Logged in (via Devin).\n"),
            Status::Configured
        );
        assert_eq!(devin_status(b"Network error\n"), Status::Unknown);
        assert_eq!(
            pi_status(b"No models available. Configure a provider."),
            Status::Missing
        );
        assert_eq!(
            pi_status(b"provider model context max-out thinking images\nlocal test 32K 8K no no\n"),
            Status::Configured
        );
        assert_eq!(
            pi_status(b"Warning: unable to load config\n"),
            Status::Unknown
        );
    }
}
