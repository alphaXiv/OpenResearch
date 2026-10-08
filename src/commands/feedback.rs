//! `orx feedback`: agent-filed product feedback for the OpenResearch team.

use serde::Serialize;

use crate::error::Result;
use crate::FeedbackKind;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Feedback {
    kind: FeedbackKind,
    summary: String,
    details: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    quote: Option<String>,
    context: Context,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Context {
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_session_id: Option<String>,
    cli_version: &'static str,
    /// The `orx up` that spawned this agent, which may differ from `cli_version`.
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_version: Option<String>,
    os: &'static str,
    arch: &'static str,
    build_channel: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    install_id: Option<String>,
}

pub async fn run(args: crate::FeedbackArgs) -> Result<()> {
    // Same gate as analytics: nothing from development builds or opted-out users.
    if crate::telemetry::effective_disabled_reason().is_some() {
        return Ok(());
    }
    let feedback = Feedback {
        kind: args.kind,
        summary: args.summary,
        details: args.details,
        quote: args.quote,
        context: Context {
            harness: crate::local::chat::launching_chat_harness(),
            chat_session_id: crate::local::chat::launching_chat_session(),
            cli_version: env!("CARGO_PKG_VERSION"),
            backend_version: std::env::var(crate::local::chat::UP_VERSION_ENV)
                .ok()
                .filter(|version| !version.is_empty()),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            build_channel: crate::telemetry::build_channel(),
            install_id: crate::telemetry::install_id(),
        },
    };
    let creds = crate::config::load_credentials().await.ok().flatten();
    crate::client::submit_feedback(creds.as_ref(), &feedback).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_matches_the_api_contract() {
        let feedback = Feedback {
            kind: FeedbackKind::FeatureRequest,
            summary: "s".into(),
            details: "d".into(),
            quote: None,
            context: Context {
                harness: Some("codex".into()),
                chat_session_id: None,
                cli_version: "0.2.8",
                backend_version: Some("0.2.6".into()),
                os: "macos",
                arch: "aarch64",
                build_channel: "source",
                install_id: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&feedback).unwrap(),
            serde_json::json!({
                "kind": "feature_request",
                "summary": "s",
                "details": "d",
                "context": {
                    "harness": "codex",
                    "cliVersion": "0.2.8",
                    "backendVersion": "0.2.6",
                    "os": "macos",
                    "arch": "aarch64",
                    "buildChannel": "source",
                },
            })
        );
    }
}
