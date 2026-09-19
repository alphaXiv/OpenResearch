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

// Fail closed: only fixed diagnostic phrases from output can leave the machine, never arbitrary tokens.
fn safe_error_excerpt(output: &str) -> Option<String> {
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
    ];
    let output = output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let phrases: Vec<_> = PHRASES
        .iter()
        .filter(|phrase| output.contains(&phrase.to_ascii_lowercase()))
        .copied()
        .collect();
    (!phrases.is_empty()).then(|| phrases.join("; "))
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
