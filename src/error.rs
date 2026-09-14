//! Single crate error type.
//!
//! Commands propagate failures with the `?` operator. We use `anyhow::Error`
//! as the crate-wide error so any error (HTTP, IO, serde, custom messages)
//! flows through one channel. `main` prints the error's `Display` to stderr
//! and exits 1, matching the TS entry point's
//! `console.error(err.message); process.exit(1)`.

// Convenience re-exports forming the crate's error vocabulary. Not all are used
// today, but command modules build against this surface.
#[allow(unused_imports)]
pub use anyhow::{anyhow, bail, Context, Error, Result};

use crate::config::{load_credentials, Credentials};

/// Stable, machine-readable failure category (Priority 9: "Error
/// Classification and Observability"). Distinct from the crate-wide
/// `anyhow::Error` above (used for ordinary `?`-propagated command
/// failures): this is a small, closed vocabulary a consumer — the
/// dashboard, an agent parsing `orx exp status --json` — can switch on
/// reliably, printed alongside (never instead of) the free-text explanation
/// a failure already carries (`StoredRun::result_markdown`/`recovery_reason`).
///
/// Wired in so far at the two places `orx` itself force-fails a run rather
/// than the backend reporting its own outcome:
/// [`crate::store::Store::mark_run_unrecoverable`], called from
/// `commands::exp::reconcile_active_runs` (TASK 2: [`Self::Reconciliation`])
/// and `commands::supervise::give_up_on_unreachable_backend` (TASK 4:
/// [`Self::BackendUnavailable`]). Classifying every other failure site
/// across 8 backends and the rest of the CLI is future work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Bad or missing configuration, caught before anything ran.
    Configuration,
    /// Missing, expired, or rejected credentials.
    Authentication,
    /// Local Git state didn't allow the operation (dirty tree, bad branch, …).
    GitState,
    /// The compute backend could not be reached at all.
    BackendUnavailable,
    /// The job could not be launched.
    Launch,
    /// The job launched but failed during execution.
    Runtime,
    /// Cancellation itself could not be completed.
    Cancellation,
    /// Crash-recovery/reconciliation gave up on the run.
    Reconciliation,
    /// The local SQLite store itself failed.
    Storage,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration_error",
            Self::Authentication => "authentication_error",
            Self::GitState => "git_state_error",
            Self::BackendUnavailable => "backend_unavailable",
            Self::Launch => "launch_failure",
            Self::Runtime => "runtime_failure",
            Self::Cancellation => "cancellation_failure",
            Self::Reconciliation => "reconciliation_failure",
            Self::Storage => "storage_failure",
        }
    }

    pub fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "configuration_error" => Self::Configuration,
            "authentication_error" => Self::Authentication,
            "git_state_error" => Self::GitState,
            "backend_unavailable" => Self::BackendUnavailable,
            "launch_failure" => Self::Launch,
            "runtime_failure" => Self::Runtime,
            "cancellation_failure" => Self::Cancellation,
            "reconciliation_failure" => Self::Reconciliation,
            "storage_failure" => Self::Storage,
            _ => return None,
        })
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Loads stored credentials or exits the process with code 1 and the message
/// `Not logged in. Run `orx login` first.` (stderr), exactly like the TS
/// `requireCredentials`.
///
/// This intentionally returns `Credentials` (not `Result`) and `exit`s on the
/// missing-credentials path so command authors can write:
///
/// ```ignore
/// let creds = require_credentials().await;
/// ```
///
/// IO errors while *reading* an existing file are treated as "not logged in",
/// matching the TS behavior where `loadCredentials` swallows errors and returns
/// `null`.
pub async fn require_credentials() -> Credentials {
    match load_credentials().await {
        Ok(Some(creds)) => creds,
        _ => {
            eprintln!(
                "Not logged in. Run `{} login` first.",
                crate::invocation::orx()
            );
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [ErrorKind; 9] = [
        ErrorKind::Configuration,
        ErrorKind::Authentication,
        ErrorKind::GitState,
        ErrorKind::BackendUnavailable,
        ErrorKind::Launch,
        ErrorKind::Runtime,
        ErrorKind::Cancellation,
        ErrorKind::Reconciliation,
        ErrorKind::Storage,
    ];

    #[test]
    fn error_kind_as_str_round_trips_through_parse() {
        for kind in ALL {
            assert_eq!(ErrorKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ErrorKind::parse("not-a-real-code"), None);
    }

    #[test]
    fn error_kind_codes_are_unique_and_snake_case() {
        let codes: Vec<&str> = ALL.iter().map(|k| k.as_str()).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            codes.len(),
            "duplicate error codes: {codes:?}"
        );
        for code in codes {
            assert!(
                code.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "not snake_case: {code}"
            );
        }
    }
}
