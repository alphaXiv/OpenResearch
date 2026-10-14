//! On-demand diagnostics reuse backend probes and never allocate compute.
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    backend: String,
    pub ready: bool,
    checked_at: i64,
    version: &'static str,
    executable: String,
    build_channel: &'static str,
    checks: Vec<Check>,
}

#[derive(Debug, Serialize)]
pub struct Check {
    id: &'static str,
    status: &'static str,
    message: String,
    action: Option<String>,
}

impl Check {
    fn new(
        id: &'static str,
        ok: bool,
        message: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            id,
            status: if ok { "pass" } else { "fail" },
            message: message.into(),
            action: (!ok).then(|| action.into()),
        }
    }
}

pub async fn diagnose(
    backend: &str,
    host: Option<String>,
    container: Option<String>,
    no_container: bool,
    address: Option<String>,
) -> Report {
    let mut report = Report {
        backend: backend.into(),
        ready: false,
        checked_at: crate::store::now_ms(),
        version: env!("CARGO_PKG_VERSION"),
        executable: std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
        build_channel: crate::telemetry::build_channel(),
        checks: Vec::new(),
    };
    report.checks.push(Check::new("orx", true,
        "This is the running orx executable. After pulling source changes, rebuild and restart the dashboard.", ""));
    let result = tokio::time::timeout(Duration::from_secs(45), async {
        if backend == "colab" {
            colab_checks(&mut report.checks).await;
        } else {
            let result = super::up::compute_settings::check(backend, host, container, no_container, address).await;
            let (ok, message) = match result {
                Ok(value) => (value["ready"] == true, probe_message(&value)),
                Err(error) => (false, error.to_string()),
            };
            report.checks.push(Check::new("connection", ok, message,
                format!("Inspect `orx compute show {backend} --json` and correct its configuration, then run `orx compute doctor {backend}` again.")));
        }
    }).await;
    if result.is_err() {
        report.checks.push(Check::new(
            "connection",
            false,
            "The compute check exceeded 45 seconds.",
            "Check network access, proxy settings and backend availability, then retry.",
        ));
    }
    report.ready = report.checks.iter().all(|c| c.status == "pass");
    report
}

fn probe_message(value: &Value) -> String {
    if value["ready"] == true {
        return "The backend readiness check passed.".into();
    }
    for key in ["error", "validationError", "monitoringError"] {
        if let Some(error) = value[key].as_str().filter(|s| !s.is_empty()) {
            return error.into();
        }
    }
    "The backend is not ready. Inspect its saved configuration and credentials.".into()
}

async fn colab_checks(checks: &mut Vec<Check>) {
    use crate::jobs::colab;
    let status = colab::status().await;
    checks.push(Check::new(
        "platform",
        status.supported_os,
        if status.supported_os {
            "Colab is supported on this operating system."
        } else {
            "Colab CLI requires Linux or macOS."
        },
        "Use a Linux or macOS compute host.",
    ));
    if !status.supported_os {
        return;
    }
    let cli_ok = status.cli_path.is_some() && status.cli_version.is_some();
    checks.push(Check::new(
        "cli",
        cli_ok,
        match (&status.cli_path, &status.cli_version) {
            (Some(path), Some(version)) => format!("{version} — {path}"),
            (Some(path), None) => format!("Found {path}, but `colab version` failed or timed out."),
            _ => "Colab CLI was not found.".into(),
        },
        colab::INSTALL_HINT,
    ));
    checks.push(Check::new(
        "credentials",
        status.signed_in,
        if status.signed_in {
            "A Colab credential file exists; live authentication is checked next."
        } else {
            "No Colab credential file was found."
        },
        colab::SIGN_IN_HINT,
    ));
    if !status.signed_in {
        return;
    }
    // A doctor request must not reuse a token from before the user signed in again.
    match crate::jobs::colab_account::probe_connection().await {
        Ok(runtime_count) => checks.push(Check::new(
            "connection",
            true,
            format!(
                "Google accepted the sign-in; {} assigned runtime(s). No runtime was started.",
                runtime_count
            ),
            "",
        )),
        Err(error) => {
            let action = colab_recovery(&error);
            checks.push(Check::new("connection", false, error, action));
        }
    }
}

fn colab_recovery(error: &str) -> &'static str {
    if error.contains("401")
        || error.contains("refused the Colab sign-in")
        || error.contains("sign-in has no token")
        || error.contains("sign-in could not be read")
    {
        "Run `orx compute connect colab` in a terminal, finish Google sign-in, then check again."
    } else if error.contains("403") {
        "Google denied access. Check the selected Google account and its Colab permissions; reconnect with `orx compute connect colab`."
    } else {
        "Check access to oauth2.googleapis.com and colab.research.google.com, DNS and proxy settings, then retry."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_authentication_permissions_and_transport_failures() {
        assert!(colab_recovery("HTTP 401 Unauthorized").contains("connect colab"));
        assert!(colab_recovery("HTTP 403 Forbidden").contains("permissions"));
        assert!(colab_recovery("connection timed out").contains("DNS"));
        assert!(
            probe_message(&serde_json::json!({"ready":false,"error":"missing CLI"}))
                .contains("missing CLI")
        );
    }
    #[tokio::test]
    async fn local_diagnostics_include_binary_and_never_need_a_job() {
        let report = diagnose("local", None, None, false, None).await;
        assert!(report.ready);
        assert!(!report.executable.is_empty());
        assert_eq!(report.checks.len(), 2);
    }
}
