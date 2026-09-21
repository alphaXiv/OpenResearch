use super::*;
use serde_json::Value;

pub(crate) const IDS: [&str; 5] = ["claude-code", "codex", "opencode", "cursor", "antigravity"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct InitialSnapshot {
    event_id: String,
    payload: Value,
    queued: bool,
}

fn state(id: &str, h: &Value) -> Value {
    let installed = h["installed"].as_bool();
    let broken = h["installBroken"] == true;
    let checked = installed == Some(true) && !broken;
    let auth = h["authState"].as_str();
    let signed_in =
        h["authenticated"] == true || (id == "claude-code" && auth == Some("unsupported"));
    let auth_state = if !checked {
        "not_checked"
    } else if auth == Some("unknown") {
        "unverifiable"
    } else if auth == Some("needsLogin") {
        "signed_out"
    } else if signed_in {
        "signed_in"
    } else {
        "signed_out"
    };
    json!({
        "harness": id,
        "installation": if broken { "broken" } else { match installed { Some(true) => "installed", Some(false) => "not_installed", None => "unknown" } },
        "auth": auth_state,
        "authEvidence": if !checked || auth_state == "unverifiable" { "none" }
            else if matches!(id, "claude-code" | "cursor" | "antigravity") && h["authMethod"] != "apiKey" { "cli_status" } else { "configuration" },
        "compatibility": if auth == Some("unsupported") { "update_required" } else if checked && h["version"].is_string() { "no_known_requirement" } else { "unknown" },
        "usability": if h["agentReady"] == true { "usable" } else if auth_state == "unverifiable" || installed.is_none() { "unknown" } else { "unavailable" },
        "localConfigured": h["authMethod"] == "local",
    })
}

// Keep the original payload and IDs so a crash between enqueue and claim retries the same snapshot.
pub(crate) fn capture_initial(harnesses: &Value) {
    if !is_enabled(flag()) {
        return;
    }
    if load_settings()
        .and_then(|s| s.harness_snapshot)
        .is_some_and(|s| s.queued)
    {
        return;
    }
    if !crate::store::Store::open()
        .and_then(|store| store.ui_state())
        .is_ok_and(|ui| !ui.onboarding_completed)
    {
        return;
    }
    let Some(install) = install_id() else { return };
    if mutate_settings(|settings| ensure_initial(settings, &install, harnesses)).is_err() {
        return;
    }
    let mut queued = None;
    let saved = mutate_settings(|settings| {
        let Some(snapshot) = settings.harness_snapshot.as_mut().filter(|s| !s.queued) else {
            return;
        };
        if let Some(path) = uuid::Uuid::parse_str(&snapshot.event_id)
            .ok()
            .and_then(|id| persist_payload(id, &snapshot.payload))
        {
            snapshot.queued = true;
            queued = Some((path, snapshot.payload.clone()));
        }
    });
    if saved.is_ok() {
        if let Some((path, payload)) = queued {
            register_pending(tokio::spawn(deliver_payload(Some(path), payload)));
        }
    }
}

fn ensure_initial(settings: &mut Settings, install: &str, harnesses: &Value) {
    if settings.harness_snapshot.is_some() {
        return;
    }
    let event_id = uuid::Uuid::new_v4();
    let mut payload = build_payload_with_id("harness_initial_state", install, event_id, json!({}));
    let template = payload["events"][0].clone();
    payload["events"] = Value::Array(
        IDS.iter()
            .map(|id| {
                let h = harnesses["harnesses"]
                    .as_array()
                    .and_then(|items| items.iter().find(|h| h["id"] == *id))
                    .unwrap_or(&Value::Null);
                let mut event = template.clone();
                event["eventId"] = json!(uuid::Uuid::new_v4().to_string());
                event["properties"] = state(id, h);
                event
            })
            .collect(),
    );
    settings.harness_snapshot = Some(InitialSnapshot {
        event_id: event_id.to_string(),
        payload,
        queued: false,
    });
}

pub(crate) struct SetupAttempt {
    id: uuid::Uuid,
    harness: String,
    action: &'static str,
    trigger: &'static str,
    started: std::time::Instant,
}

impl SetupAttempt {
    pub(crate) fn new(harness: &str, action: &'static str, trigger: &'static str) -> Self {
        let attempt = Self {
            id: uuid::Uuid::new_v4(),
            harness: harness.into(),
            action,
            trigger,
            started: std::time::Instant::now(),
        };
        attempt.record("started", "detect", None, None, None);
        attempt
    }

    pub(crate) fn record(
        &self,
        outcome: &str,
        stage: &str,
        reason: Option<&str>,
        exit_code: Option<u32>,
        output: Option<&str>,
    ) {
        // Ingestion rejects the whole event on a value outside the companion
        // schema, so a new call site fails here in any debug run that hits it.
        debug_assert!(contract::OUTCOMES.contains(&outcome), "{outcome}");
        debug_assert!(contract::STAGES.contains(&stage), "{stage}");
        debug_assert!(
            reason.is_none_or(|reason| contract::REASONS.contains(&reason)),
            "{reason:?}"
        );
        capture(
            "harness_setup",
            json!({
                "attemptId": self.id.to_string(), "harness": self.harness, "action": self.action, "trigger": self.trigger,
                "outcome": outcome, "stage": stage, "reason": reason,
                "exitCode": exit_code, "durationMs": self.started.elapsed().as_millis().min(9_007_199_254_740_991) as u64,
                "errorExcerpt": output.and_then(safe_error_excerpt),
            }),
        );
    }
}

#[cfg(test)]
pub(super) async fn assert_production_contract() {
    let mut settings = Settings::default();
    ensure_initial(
        &mut settings,
        "cli-release-contract-test",
        &json!({"harnesses": []}),
    );
    let snapshot = settings.harness_snapshot.unwrap();
    assert_eq!(
        post_payload(&snapshot.payload).await,
        DeliveryOutcome::Acknowledged
    );
    for harness in IDS {
        let payload = build_payload(
            "harness_setup",
            "cli-release-contract-test",
            json!({"attemptId":uuid::Uuid::new_v4().to_string(),"harness":harness,"action":"login","trigger":"manual","outcome":"failed","stage":"verify","reason":"not_ready","exitCode":null,"durationMs":100,"errorExcerpt":null}),
        );
        assert_eq!(
            post_payload(&payload).await,
            DeliveryOutcome::Acknowledged,
            "{harness}"
        );
    }
}

#[cfg(test)]
pub(crate) fn excerpt_for_test(output: &str) -> Option<String> {
    safe_error_excerpt(output)
}

/// `zHarnessSetup` in openresearch.sh's `cli-analytics-contract.ts`. Ingestion
/// rejects the whole event on anything outside these, so every `record` call
/// has to stay inside them until the companion schema widens.
pub(crate) mod contract {
    pub(crate) const OUTCOMES: &[&str] = &[
        "started",
        "command_started",
        "command_completed",
        "succeeded",
        "failed",
        "interrupted",
    ];
    pub(crate) const STAGES: &[&str] = &["detect", "command", "verify"];
    pub(crate) const REASONS: &[&str] = &[
        "not_eligible",
        "not_installed",
        "spawn_failed",
        "exit_nonzero",
        "terminal_error",
        "disconnected",
        "not_ready",
    ];
    pub(crate) const MAX_ERROR_EXCERPT: usize = 1024;
}

// Fail closed: only fixed diagnostic phrases from output can leave the machine, never arbitrary tokens.
const PHRASES: &[&str] = &[
    "Could not resolve host",
    "Could not resolve proxy",
    "Failed to connect",
    "Connection refused",
    "Connection timed out",
    "Operation timed out",
    "SSL certificate problem",
    "certificate verify failed",
    "Permission denied",
    "Access is denied",
    "No space left on device",
    "Read-only file system",
    "No such file or directory",
    "command not found",
    "is not recognized as the name of a cmdlet",
    "running scripts is disabled on this system",
    "Unsupported platform",
    "Unsupported architecture",
    // Real failures the previous list discarded, leaving a NULL excerpt on
    // an exit that had a perfectly specific cause.
    "stdin is not a terminal",
    "Recv failure",
    "Send failure",
    "Connection reset by peer",
    "Connection was aborted",
    "SSL connection timeout",
    "OpenSSL SSL_connect",
    "Empty reply from server",
    "Unable to connect to the remote server",
    "The system cannot find the file specified",
    "The system cannot find the path specified",
    "Cannot create a file when that file already exists",
    "The process cannot access the file",
    "being used by another process",
    "Failed to fetch version information",
    "Failed to install native update",
    "file is not a database",
    "database disk image is malformed",
    "Login failed or timed out",
    "Request failed with status code 400",
    "ECONNREFUSED",
    "ETIMEDOUT",
    "ENOENT",
    "The requested URL returned error: 403",
    "The requested URL returned error: 404",
    "The requested URL returned error: 429",
    "The requested URL returned error: 500",
    "The requested URL returned error: 502",
    "The requested URL returned error: 503",
    "Authentication failed",
    "Invalid API key",
    "Unauthorized",
    "EACCES",
    "ENOSPC",
    "ECONNRESET",
    // Fixed classifications `verify_reason` passes in; the ingested
    // `reason` allowlist has no vocabulary for which predicate failed.
    "agent is not installed",
    "agent is installed but failed to run",
    "agent is not signed in",
    "agent sign-in could not be verified",
    "agent version is unsupported",
    "agent configuration needs repair",
    "agent has no usable model",
    "agent reported no usable state",
];

fn safe_error_excerpt(output: &str) -> Option<String> {
    let output = output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    // Ingestion caps `errorExcerpt` and rejects the whole event over it, so
    // stop joining before the limit rather than lose the attempt.
    const MAX: usize = contract::MAX_ERROR_EXCERPT;
    let mut excerpt = String::new();
    for phrase in PHRASES
        .iter()
        .filter(|phrase| output.contains(&phrase.to_ascii_lowercase()))
    {
        let separator = if excerpt.is_empty() { "" } else { "; " };
        if excerpt.len() + separator.len() + phrase.len() > MAX {
            break;
        }
        excerpt.push_str(separator);
        excerpt.push_str(phrase);
    }
    (!excerpt.is_empty()).then_some(excerpt)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_survives_restart_and_does_not_change() {
        let mut settings = Settings::default();
        ensure_initial(&mut settings, "installation", &json!({"harnesses":[]}));
        let first = settings.harness_snapshot.as_ref().unwrap().payload.clone();
        let serialized = serde_json::to_string(&settings).unwrap();
        let mut restarted: Settings = serde_json::from_str(&serialized).unwrap();
        ensure_initial(
            &mut restarted,
            "installation",
            &json!({"harnesses":[{"id":"opencode","installed":true}]}),
        );
        assert_eq!(first, restarted.harness_snapshot.unwrap().payload);
        assert_eq!(first["events"].as_array().unwrap().len(), IDS.len());
    }
    #[test]
    fn snapshot_includes_antigravity_and_preserves_unknown_states() {
        let mut settings = Settings::default();
        ensure_initial(
            &mut settings,
            "installation",
            &json!({"harnesses": [{
                "id": "antigravity", "installed": true, "authenticated": true,
                "authState": "ready", "authMethod": "oauth", "agentReady": true, "version": "1"
            }]}),
        );
        let snapshot = settings.harness_snapshot.unwrap();
        let events = snapshot.payload["events"].as_array().unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(
            events
                .iter()
                .map(|e| e["properties"]["harness"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["claude-code", "codex", "opencode", "cursor", "antigravity"]
        );
        assert_eq!(
            events[4]["properties"],
            json!({
                "harness": "antigravity", "installation": "installed", "auth": "signed_in",
                "authEvidence": "cli_status", "compatibility": "no_known_requirement",
                "usability": "usable", "localConfigured": false
            })
        );
        for event in &events[..4] {
            assert_eq!(event["properties"]["installation"], "unknown");
            assert_eq!(event["properties"]["usability"], "unknown");
        }
    }

    #[test]
    fn distinguishes_free_auth_unknown_and_unsupported() {
        let base = json!({"installed":true,"authenticated":false,"agentReady":true,"authState":"ready","version":"1"});
        let free = state("opencode", &base);
        assert_eq!(free["auth"], "signed_out");
        assert_eq!(free["usability"], "usable");
        let mut h = base;
        h["agentReady"] = json!(false);
        h["authState"] = json!("unknown");
        assert_eq!(state("cursor", &h)["auth"], "unverifiable");
        h["authState"] = json!("unsupported");
        assert_eq!(state("claude-code", &h)["auth"], "signed_in");
        assert_eq!(state("claude-code", &h)["compatibility"], "update_required");
        h["installed"] = json!(false);
        h["authenticated"] = json!(true);
        assert_eq!(state("codex", &h)["auth"], "not_checked");
    }
    #[test]
    fn excerpts_never_include_dynamic_output() {
        assert_eq!(safe_error_excerpt("\x1b[31mPermission denied: /Users/person/secret Bearer sk-abc a@b.com https://auth?token=secret"), Some("Permission denied".into()));
        assert_eq!(safe_error_excerpt("my-secret-api-key"), None);
        assert_eq!(
            safe_error_excerpt("curl: (22) The requested URL returned error: 403 secret"),
            Some("The requested URL returned error: 403".into())
        );
    }

    #[test]
    fn real_setup_failures_are_no_longer_dropped_to_null() {
        // Each of these reached the setup UI as a useful message and was
        // uploaded as NULL, leaving the failure unattributable.
        for (output, expected) in [
            ("Error: stdin is not a terminal", "stdin is not a terminal"),
            (
                "curl: (35) Recv failure: Connection reset by peer",
                "Recv failure; Connection reset by peer",
            ),
            (
                "CreateProcessW ... failed: The system cannot find the file specified. (os error 2)",
                "The system cannot find the file specified",
            ),
            (
                "Move-Item : Cannot create a file when that file already exists",
                "Cannot create a file when that file already exists",
            ),
            (
                "Invoke-RestMethod : Unable to connect to the remote server",
                "Unable to connect to the remote server",
            ),
            (
                "Failed to fetch version information",
                "Failed to fetch version information",
            ),
            ("Error: file is not a database", "file is not a database"),
            (
                "Failed to fetch version: connect ECONNREFUSED 127.0.0.1:9",
                "ECONNREFUSED",
            ),
        ] {
            assert_eq!(safe_error_excerpt(output).as_deref(), Some(expected), "{output}");
        }
        // Still fail-closed: paths, tokens and addresses never leave.
        assert_eq!(
            safe_error_excerpt(
                "stdin is not a terminal /Users/person/.codex/auth.json sk-ant-secret"
            )
            .as_deref(),
            Some("stdin is not a terminal")
        );
    }

    #[test]
    fn excerpt_stays_within_the_ingested_maximum() {
        // The worst case: output containing every allowlisted phrase at once.
        // Over 1024 the ingestion rejects the event and the attempt is lost.
        let everything = PHRASES.join(" ");
        assert!(
            everything.len() > contract::MAX_ERROR_EXCERPT,
            "test no longer exercises the cap"
        );
        let excerpt = safe_error_excerpt(&everything).unwrap();
        assert!(
            excerpt.len() <= contract::MAX_ERROR_EXCERPT,
            "{}",
            excerpt.len()
        );
        // Truncation is by whole phrase, so nothing partial ever leaves.
        for phrase in excerpt.split("; ") {
            assert!(PHRASES.contains(&phrase), "{phrase}");
        }
        // A single phrase is unaffected by the cap.
        assert_eq!(
            safe_error_excerpt("Connection refused").as_deref(),
            Some("Connection refused")
        );
    }

    #[test]
    fn wrapped_and_mixed_case_errors_keep_only_fixed_diagnostics() {
        assert_eq!(
            safe_error_excerpt("npm : The term 'npm' is not recognized as the name of a cmdlet, function, script file, or operable program. C:\\Users\\private"),
            Some("is not recognized as the name of a cmdlet".into())
        );
        assert_eq!(
            safe_error_excerpt("C:\\Users\\private\\npm.ps1 cannot be loaded because running scripts is\r\n disabled on this system."),
            Some("running scripts is disabled on this system".into())
        );
        assert_eq!(
            safe_error_excerpt("ERROR: permission denied for secret-token"),
            Some("Permission denied".into())
        );
    }
}
