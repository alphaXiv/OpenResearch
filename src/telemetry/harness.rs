use super::*;
use serde_json::Value;

pub(crate) const IDS: [&str; 5] = ["claude-code", "codex", "opencode", "cursor", "antigravity"];

pub(crate) fn wire_id(id: &str) -> Option<&'static str> {
    if id.starts_with("acp:") {
        return Some(crate::local::acp::telemetry_id(id).unwrap_or("acp:custom"));
    }
    IDS.into_iter().find(|known| *known == id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct InitialSnapshot {
    event_id: String,
    payload: Value,
    queued: bool,
    /// Fields from a newer orx, kept across this version's saves.
    #[serde(flatten)]
    unknown: serde_json::Map<String, Value>,
}

fn state(id: &str, h: &Value) -> Value {
    let installed = h["installed"].as_bool();
    let broken = h["installBroken"] == true;
    let checked = installed == Some(true) && !broken;
    let auth = h["authState"].as_str();
    // ACP detection never sets `authenticated`; a ready agent's failed check still reads `ready`.
    let acp = id.starts_with("acp:");
    let signed_in = h["authenticated"] == true
        || (id == "claude-code" && auth == Some("unsupported"))
        || (acp && auth == Some("ready"));
    let auth_state = if !checked {
        "not_checked"
    } else if auth == Some("unknown") || (acp && h["authCheckFailed"] == true) {
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
    if load_settings().is_some_and(|settings| {
        [settings.harness_snapshot, settings.acp_harness_snapshot]
            .into_iter()
            .all(|snapshot| snapshot.is_some_and(|snapshot| snapshot.queued))
    }) {
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
    let mut queued = Vec::new();
    let saved = mutate_settings(|settings| {
        for snapshot in [
            &mut settings.harness_snapshot,
            &mut settings.acp_harness_snapshot,
        ]
        .into_iter()
        .filter_map(Option::as_mut)
        .filter(|snapshot| !snapshot.queued)
        {
            if let Some(path) = uuid::Uuid::parse_str(&snapshot.event_id)
                .ok()
                .and_then(|id| persist_payload(id, &snapshot.payload))
            {
                snapshot.queued = true;
                queued.push((path, snapshot.payload.clone()));
            }
        }
    });
    if saved.is_ok() {
        for (path, payload) in queued {
            register_pending(tokio::spawn(deliver_payload(Some(path), payload)));
        }
    }
}

fn ensure_initial(settings: &mut Settings, install: &str, harnesses: &Value) {
    let items = harnesses["harnesses"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if settings.harness_snapshot.is_none() {
        settings.harness_snapshot = Some(initial_snapshot(
            install,
            IDS.iter().map(|id| {
                let info = items
                    .iter()
                    .find(|h| h["id"] == *id)
                    .unwrap_or(&Value::Null);
                state(id, info)
            }),
        ));
    }
    if settings.acp_harness_snapshot.is_none() {
        let mut acp = std::collections::BTreeMap::new();
        let readiness = |info: &Value| (info["agentReady"] == true, info["installed"] == true);
        for info in items {
            let Some(id) = info["id"]
                .as_str()
                .filter(|id| id.starts_with("acp:"))
                .and_then(wire_id)
            else {
                continue;
            };
            let previous = acp.entry(id).or_insert(info);
            if readiness(info) > readiness(previous) {
                *previous = info;
            }
        }
        if !acp.is_empty() {
            settings.acp_harness_snapshot = Some(initial_snapshot(
                install,
                acp.into_iter().map(|(id, info)| state(id, info)),
            ));
        }
    }
}

fn initial_snapshot(install: &str, states: impl Iterator<Item = Value>) -> InitialSnapshot {
    let event_id = uuid::Uuid::new_v4();
    let mut payload = build_payload_with_id("harness_initial_state", install, event_id, json!({}));
    let template = payload["events"][0].clone();
    payload["events"] = Value::Array(
        states
            .map(|properties| {
                let mut event = template.clone();
                event["eventId"] = json!(uuid::Uuid::new_v4().to_string());
                event["properties"] = properties;
                event
            })
            .collect(),
    );
    InitialSnapshot {
        event_id: event_id.to_string(),
        payload,
        queued: false,
        unknown: Default::default(),
    }
}

/// One detection pass — the boot snapshot or a catalog fill — sent as a
/// single payload: a `harness_detect` row for the pass's own wall clock, then
/// each probe the pass recorded as a `harness_detect_probe` event on the same
/// `fillId`. `first_ready_ms` marks when the first `agentReady` entry
/// committed — the span the onboarding gate actually waits on.
pub(crate) fn capture_detect(
    fill_id: uuid::Uuid,
    pass: &'static str,
    duration_ms: u64,
    first_ready_ms: Option<u64>,
    installed: usize,
    ready: usize,
    probes: Vec<crate::local::harness::ProbeTiming>,
) {
    if !is_enabled(flag()) {
        return;
    }
    debug_assert!(contract::DETECT_PASSES.contains(&pass), "{pass}");
    let Some(install) = install_id() else { return };
    let event_id = uuid::Uuid::new_v4();
    let payload = detect_payload(
        &install,
        event_id,
        fill_id,
        pass,
        duration_ms,
        first_ready_ms,
        installed,
        ready,
        probes,
    );
    let path = persist_payload(event_id, &payload);
    register_pending(tokio::spawn(deliver_payload(path, payload)));
}

#[expect(clippy::too_many_arguments)]
fn detect_payload(
    install: &str,
    event_id: uuid::Uuid,
    fill_id: uuid::Uuid,
    pass: &str,
    duration_ms: u64,
    first_ready_ms: Option<u64>,
    installed: usize,
    ready: usize,
    mut probes: Vec<crate::local::harness::ProbeTiming>,
) -> Value {
    let mut payload = build_payload_with_id(
        "harness_detect",
        install,
        event_id,
        json!({
            "fillId": fill_id.to_string(),
            "pass": pass,
            "durationMs": duration_ms.min(9_007_199_254_740_991),
            "firstReadyMs": first_ready_ms.map(|ms| ms.min(9_007_199_254_740_991)),
            "installed": installed.min(64),
            "ready": ready.min(64),
        }),
    );
    let template = payload["events"][0].clone();
    if let Some(events) = payload["events"].as_array_mut() {
        // Keep per-agent totals ahead of detailed timings under the 25-event cap.
        if probes.len() > 24 {
            probes.sort_by_key(|probe| probe.probe != "total");
        }
        for probe in probes.into_iter().take(24) {
            debug_assert_eq!(wire_id(probe.harness), Some(probe.harness));
            debug_assert!(probe.probe.len() <= 64, "{}", probe.probe);
            let mut event = template.clone();
            event["eventId"] = json!(uuid::Uuid::new_v4().to_string());
            event["name"] = json!("cli_harness_detect_probe");
            event["properties"] = json!({
                "fillId": fill_id.to_string(),
                "harness": probe.harness,
                "probe": probe.probe,
                "durationMs": probe.ms.min(9_007_199_254_740_991),
            });
            events.push(event);
        }
    }
    payload
}

#[derive(Clone, Copy)]
pub(crate) enum InstallStep {
    ReleaseLookup,
    NativeUpdater,
    ManagedCopy,
}

impl InstallStep {
    fn label(self) -> &'static str {
        match self {
            Self::ReleaseLookup => "managed release lookup",
            Self::NativeUpdater => "native agent updater",
            Self::ManagedCopy => "managed agent copy",
        }
    }
}

pub(crate) struct SetupAttempt {
    id: uuid::Uuid,
    install_step: std::sync::Mutex<Option<InstallStep>>,
    harness: String,
    action: &'static str,
    trigger: &'static str,
    started: std::time::Instant,
}

impl SetupAttempt {
    pub(crate) fn new(harness: &str, action: &'static str, trigger: &'static str) -> Self {
        let attempt = Self {
            id: uuid::Uuid::new_v4(),
            install_step: Default::default(),
            harness: harness.into(),
            action,
            trigger,
            started: std::time::Instant::now(),
        };
        attempt.record("started", "detect", None, None, None);
        attempt
    }

    pub(crate) async fn command<T>(&self, work: impl std::future::Future<Output = T>) -> T {
        observe_command(work, |outcome, detail| {
            self.record(outcome, "command", None, None, detail);
        })
        .await
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
                "errorExcerpt": self.error_excerpt(outcome, stage, exit_code, output),
            }),
        );
    }

    pub(crate) fn set_install_step(&self, step: InstallStep) {
        if let Ok(mut current) = self.install_step.lock() {
            *current = Some(step);
        }
    }

    fn error_excerpt(
        &self,
        outcome: &str,
        stage: &str,
        exit_code: Option<u32>,
        output: Option<&str>,
    ) -> Option<String> {
        // A failed command may be credited after verification if it installed a runnable copy.
        let failed_command = (outcome == "failed" && stage == "command")
            || (outcome == "succeeded" && exit_code.is_some_and(|code| code != 0));
        let step = if failed_command {
            self.install_step.lock().ok().and_then(|step| *step)
        } else {
            None
        };
        safe_error_excerpt_with_step(output.unwrap_or_default(), step)
    }
}

/// A dropped future is an observed interruption, not evidence the agent command failed.
async fn observe_command<T>(
    work: impl std::future::Future<Output = T>,
    report: impl Fn(&'static str, Option<&'static str>),
) -> T {
    struct Pending<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Pending<F> {
        fn drop(&mut self) {
            if let Some(report) = self.0.take() {
                report();
            }
        }
    }

    report("command_started", None);
    let mut pending = Pending(Some(|| {
        report(
            "interrupted",
            Some("setup command task ended before a result"),
        );
    }));
    let result = work.await;
    pending.0.take();
    result
}

#[cfg(test)]
pub(super) async fn assert_production_contract() {
    let mut settings = Settings::default();
    let preset = &crate::local::acp::presets()[0].launch;
    ensure_initial(
        &mut settings,
        "cli-release-contract-test",
        &json!({"harnesses": [{"id": preset.id}]}),
    );
    for snapshot in [
        settings.harness_snapshot.unwrap(),
        settings.acp_harness_snapshot.unwrap(),
    ] {
        assert_eq!(
            post_payload(&snapshot.payload).await,
            DeliveryOutcome::Acknowledged
        );
    }
    for harness in IDS.into_iter().chain(["acp:custom"]) {
        let payload = build_payload(
            "harness_setup",
            "cli-release-contract-test",
            json!({"attemptId":uuid::Uuid::new_v4().to_string(),"harness":harness,"action":"install","trigger":"manual","outcome":"failed","stage":"verify","reason":"not_ready","exitCode":null,"durationMs":100,"errorExcerpt":null}),
        );
        assert_eq!(
            post_payload(&payload).await,
            DeliveryOutcome::Acknowledged,
            "{harness}"
        );
    }
    let fill_id = uuid::Uuid::new_v4().to_string();
    for (name, properties) in [
        (
            "harness_detect",
            json!({"fillId":fill_id,"pass":"full","durationMs":8200,"firstReadyMs":3200,"installed":4,"ready":1}),
        ),
        (
            "harness_detect_probe",
            json!({"fillId":fill_id,"harness":"opencode","probe":"models","durationMs":6900}),
        ),
    ] {
        let payload = build_payload(name, "cli-release-contract-test", properties);
        assert_eq!(
            post_payload(&payload).await,
            DeliveryOutcome::Acknowledged,
            "{name}"
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
    /// `z.enum` in `zHarnessDetect` — the pass a `harness_detect` row covers.
    pub(crate) const DETECT_PASSES: &[&str] = &["snapshot", "full"];
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

pub(crate) const MANAGED_LOOKUP: &str = "Managed release lookup failed";
pub(crate) const MANAGED_INSTALL: &str = "Managed release installation failed";

// Fail closed: only fixed diagnostic phrases from output can leave the machine, never arbitrary tokens.
const PHRASES: &[&str] = &[
    MANAGED_LOOKUP,
    MANAGED_INSTALL,
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
    // orx's own installs of the agents it manages.
    "Could not download",
    "Could not reach",
    "does not match its published SHA-256",
    "Could not unpack the download",
    "The download is not zstd",
    "Could not run the downloaded",
    "did not answer `--version`",
    "failed `--version`",
    "Could not look up the latest Claude Code release",
    "Could not read the latest Claude Code release",
    "returned something other than a version",
    "The download was interrupted",
    "Could not move the release into",
    "OpenCode is in use",
    "Couldn't create",
    "Couldn't open",
    "Couldn't lock",
    // What reqwest, hyper and rustls say under those, which is the actual cause.
    "dns error",
    "failed to lookup address information",
    "No such host is known",
    "tcp connect error",
    "actively refused it",
    "did not properly respond after a period of time",
    "forcibly closed by the remote host",
    "connection closed before message completed",
    "invalid peer certificate",
    "UnknownIssuer",
    "certificate not valid for name",
    "certificate expired",
    "error decoding response body",
    "HTTP status client error (403",
    "HTTP status client error (404",
    "HTTP status client error (407",
    "HTTP status client error (429",
    "HTTP status server error",
    // .NET's network faults, which PowerShell installers print.
    "The remote name could not be resolved",
    "The operation has timed out",
    "The underlying connection was closed",
    "Could not create SSL/TLS secure channel",
    "Could not establish trust relationship",
    "Proxy Authentication Required",
    "The revocation function was unable to check revocation",
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
    "agent detection returned no result",
    "agent account could not be prepared",
    "agent executable path is unavailable",
    "agent is not installed",
    "agent is installed but failed to run",
    "setup command task ended before a result",
    "agent is not signed in",
    "agent sign-in could not be verified",
    "agent version is unsupported",
    "agent configuration needs repair",
    "agent has no usable model",
    "agent reported no usable state",
];

#[cfg(test)]
fn safe_error_excerpt(output: &str) -> Option<String> {
    safe_error_excerpt_with_step(output, None)
}

fn safe_error_excerpt_with_step(output: &str, step: Option<InstallStep>) -> Option<String> {
    let output = output
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    // Ingestion caps `errorExcerpt` and rejects the whole event over it, so
    // stop joining before the limit rather than lose the attempt.
    const MAX: usize = contract::MAX_ERROR_EXCERPT;
    let mut excerpt = step.map_or_else(String::new, |step| step.label().to_owned());
    for phrase in PHRASES
        .iter()
        .copied()
        .chain(crate::local::harness::cursor::CursorStatusError::ALL.map(|e| e.excerpt()))
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
    #[tokio::test]
    async fn only_an_observed_command_drop_adds_an_interruption() {
        let events = std::cell::RefCell::new(Vec::new());
        let observe = |outcome, detail: Option<&'static str>| {
            events
                .borrow_mut()
                .push((outcome, detail.and_then(safe_error_excerpt)));
        };

        // Never polled: setup did not reach its command.
        drop(observe_command(std::future::pending::<()>(), observe));
        assert!(events.borrow().is_empty());

        let mut command = Box::pin(observe_command(std::future::pending::<()>(), observe));
        assert!(futures::poll!(&mut command).is_pending());
        drop(command);
        assert_eq!(
            *events.borrow(),
            vec![
                ("command_started", None),
                (
                    "interrupted",
                    Some("setup command task ended before a result".to_owned())
                ),
            ]
        );

        for result in [Ok(()), Err("disconnected")] {
            events.borrow_mut().clear();
            assert_eq!(
                observe_command(std::future::ready(result), observe).await,
                result
            );
            assert_eq!(*events.borrow(), vec![("command_started", None)]);
        }
    }

    #[test]
    fn install_failures_name_the_step_even_without_an_allowed_error() {
        let attempt = SetupAttempt {
            id: uuid::Uuid::new_v4(),
            harness: "opencode".into(),
            action: "update",
            trigger: "manual",
            started: std::time::Instant::now(),
            install_step: Default::default(),
        };
        let noise = Some("native agent updater /home/private https://private.test/?token=abc");
        assert!(attempt
            .error_excerpt("failed", "command", Some(1), noise)
            .is_none());
        for step in [
            InstallStep::ReleaseLookup,
            InstallStep::NativeUpdater,
            InstallStep::ManagedCopy,
        ] {
            attempt.set_install_step(step);
            assert_eq!(
                attempt
                    .error_excerpt("failed", "command", Some(1), noise)
                    .as_deref(),
                Some(step.label())
            );
            assert_eq!(
                attempt
                    .error_excerpt("succeeded", "verify", Some(1), None)
                    .as_deref(),
                Some(step.label())
            );
            assert!(attempt
                .error_excerpt("succeeded", "verify", Some(0), None)
                .is_none());
            assert_eq!(
                attempt
                    .error_excerpt("failed", "verify", None, Some("agent is not signed in"))
                    .as_deref(),
                Some("agent is not signed in")
            );
            let diagnostic = attempt
                .error_excerpt(
                    "failed",
                    "command",
                    None,
                    Some("Permission denied: /secret"),
                )
                .unwrap();
            assert_eq!(diagnostic, format!("{}; Permission denied", step.label()));
            let longest = attempt
                .error_excerpt("failed", "command", Some(1), Some(&PHRASES.join(" ")))
                .unwrap();
            assert!(longest.len() <= contract::MAX_ERROR_EXCERPT);
        }
    }

    #[test]
    fn detect_payload_batches_pass_and_probe_events_on_one_fill_id() {
        let fill_id = uuid::Uuid::new_v4();
        let payload = detect_payload(
            "installation",
            uuid::Uuid::new_v4(),
            fill_id,
            "full",
            8_200,
            Some(3_200),
            4,
            1,
            vec![
                crate::local::harness::ProbeTiming {
                    harness: "opencode",
                    probe: "models",
                    ms: 6_900,
                },
                crate::local::harness::ProbeTiming {
                    harness: "opencode",
                    probe: "total",
                    ms: 7_895,
                },
            ],
        );
        let events = payload["events"].as_array().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["name"], "cli_harness_detect");
        assert_eq!(
            events[0]["properties"],
            json!({
                "fillId": fill_id.to_string(), "pass": "full", "durationMs": 8_200,
                "firstReadyMs": 3_200, "installed": 4, "ready": 1
            })
        );
        for event in &events[1..] {
            assert_eq!(event["name"], "cli_harness_detect_probe");
            assert_eq!(event["properties"]["fillId"], fill_id.to_string());
        }
        assert_eq!(events[1]["properties"]["harness"], "opencode");
        assert_eq!(events[2]["properties"]["probe"], "total");
        // The batch must stay inside the ingestion cap even for a stray-heavy fill.
        let payload = detect_payload(
            "installation",
            uuid::Uuid::new_v4(),
            fill_id,
            "snapshot",
            3,
            None,
            0,
            0,
            (0..64)
                .map(|_| crate::local::harness::ProbeTiming {
                    harness: "codex",
                    probe: "spec",
                    ms: 1,
                })
                .chain([crate::local::harness::ProbeTiming {
                    harness: "acp:pi",
                    probe: "total",
                    ms: 2,
                }])
                .collect(),
        );
        assert_eq!(payload["events"].as_array().unwrap().len(), 25);
        assert_eq!(payload["events"][1]["properties"]["harness"], "acp:pi");
    }
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
            &json!({"harnesses":[
                {"id":"opencode","installed":true},
                {"id":"acp:467b1900-c3c3-47c1-8b87-09389e1c44b5","installed":true},
                {"id":"acp:97e2c73d-18f0-4933-90b4-57da9d880a62","installed":false},
                {"id":"acp:5df69eb9-06a5-4741-8fe6-2cc47b0f2176","installed":true,"agentReady":true}
            ]}),
        );
        assert_eq!(first, restarted.harness_snapshot.unwrap().payload);
        let acp = restarted.acp_harness_snapshot.unwrap().payload;
        let events = acp["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["properties"]["harness"], "acp:custom");
        assert_eq!(events[0]["properties"]["usability"], "usable");
        assert_eq!(events[1]["properties"]["harness"], "acp:pi");
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
    fn acp_auth_follows_the_credential_check() {
        let mut h = json!({"installed":true,"authenticated":false,"agentReady":true,"authState":"ready","version":"1"});
        assert_eq!(state("acp:pi", &h)["auth"], "signed_in");
        assert_eq!(state("acp:pi", &h)["authEvidence"], "configuration");
        h["authCheckFailed"] = json!(true);
        let unverified = state("acp:custom", &h);
        assert_eq!(unverified["auth"], "unverifiable");
        assert_eq!(unverified["authEvidence"], "none");
        assert_eq!(unverified["usability"], "usable");
        assert_eq!(state("claude-code", &h)["auth"], "signed_out");
    }
    #[test]
    fn harness_ids_reach_the_wire_canonical_and_non_acp_ids_unchanged() {
        let mut seen = std::collections::BTreeSet::new();
        for preset in crate::local::acp::presets() {
            let id = wire_id(&preset.launch.id).unwrap();
            assert_ne!(id, "acp:custom", "{}", preset.launch.name);
            let slug = id.strip_prefix("acp:").unwrap();
            assert!(slug.len() <= 31 && slug.starts_with(|c: char| c.is_ascii_alphanumeric()));
            assert!(slug
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-'));
            assert_eq!(wire_id(id), Some(id));
            assert!(seen.insert(id), "{id}");
        }
        let custom = format!("acp:{}", uuid::Uuid::new_v4());
        for (sent, wire) in [
            (custom.as_str(), "acp:custom"),
            ("acp:custom", "acp:custom"),
            ("gemini", "gemini"),
        ] {
            let payload = build_payload("chat_message_sent", "did", json!({ "harness": sent }));
            assert_eq!(payload["events"][0]["properties"]["harness"], wire);
        }
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
    fn a_managed_download_keeps_the_network_cause_without_the_url() {
        let url = "https://registry.npmjs.org/@opencode%2fcli-windows-x64/latest?token=secret";
        for (cause, expected) in [
            (
                "client error (Connect): dns error: No such host is known. (os error 11001)",
                "Could not reach; dns error; No such host is known",
            ),
            (
                "client error (Connect): tcp connect error: No connection could be made because the target machine actively refused it. (os error 10061)",
                "Could not reach; tcp connect error; actively refused it",
            ),
            (
                "client error (Connect): invalid peer certificate: UnknownIssuer",
                "Could not reach; invalid peer certificate; UnknownIssuer",
            ),
            (
                "client error (SendRequest): connection closed before message completed",
                "Could not reach; connection closed before message completed",
            ),
        ] {
            let output = format!("Could not reach {url}: error sending request for url ({url}): {cause}");
            assert_eq!(safe_error_excerpt(&output).as_deref(), Some(expected), "{output}");
        }
        assert_eq!(
            safe_error_excerpt(&format!(
                "Could not reach {url}: HTTP status client error (403 rate limit exceeded) for url ({url})"
            ))
            .as_deref(),
            Some("Could not reach; HTTP status client error (403")
        );
        assert_eq!(
            safe_error_excerpt(
                "Could not move the release into C:\\Users\\person\\.cache\\openresearch\\managed-agents\\opencode\\2.3.0: Access is denied. (os error 5)"
            )
            .as_deref(),
            Some("Access is denied; Could not move the release into")
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
