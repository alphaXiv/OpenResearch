//! OpenResearch backend — an ephemeral platform box per run.
//!
//! Unlike the other backends, the compute itself comes from the OpenResearch
//! API: submit provisions an org-billed GPU/CPU sandbox (`POST /sandboxes`),
//! the supervisor polls `GET /sandboxes/{id}` until the box is online, runs
//! the clone-and-run payload on it over ssh (the ssh backend's transport and
//! run-dir layout, via `BackendDescriptor::openresearch_ssh_target`), and
//! deletes the box (`DELETE /sandboxes/{id}`) once the run is terminal.
//! Auth is the `orx login` credentials, not a backend-specific token.

use std::time::Duration;

use crate::client::{current_user, delete_sandbox, list_orgs, SandboxTarget};
use crate::config::Credentials;
use crate::error::{anyhow, Result};

/// The remote run dir for a run, relative to `$HOME` (shared convention with
/// the ssh backend; derived, not stored in the descriptor).
pub fn run_dir(run_id: &str) -> String {
    format!(".orx/runs/{run_id}")
}

/// Parse `--flavor` into a `POST /sandboxes` target: `<gpu_id>[:count]`
/// (e.g. `h100_sxm:2`) or a CPU flavor `cpu…[:vcpus]` (e.g. `cpu5c:8`).
/// Case-insensitive: the API's GPU ids are uppercase and its CPU flavors lowercase.
/// See `orx compute` for the provider catalog.
pub fn parse_flavor(flavor: &str, disk_gb: i64, provider: Option<String>) -> Result<SandboxTarget> {
    let flavor = flavor.trim();
    let (base, count) = match flavor.split_once(':') {
        Some((base, count)) => {
            let count: i64 = count.parse().ok().filter(|c| *c >= 1).ok_or_else(|| {
                anyhow!(
                    "Bad --flavor '{flavor}': the ':{count}' suffix must be a positive \
                     count (GPUs) or vCPU tier. See `orx compute` for available shapes."
                )
            })?;
            (base, Some(count))
        }
        None => (flavor, None),
    };
    if base.is_empty() {
        return Err(anyhow!(
            "--flavor is empty. Pass a GPU id like h100_sxm[:count] or a CPU flavor \
             like cpu5c[:vcpus] — see `orx compute`."
        ));
    }
    if base.to_ascii_lowercase().starts_with("cpu") {
        Ok(SandboxTarget::NewCpu {
            cpu_flavor: base.to_ascii_lowercase(),
            vcpu_count: count.unwrap_or(8),
        })
    } else {
        Ok(SandboxTarget::New {
            gpu: base.to_ascii_uppercase(),
            gpu_count: count.unwrap_or(1),
            disk_gb,
            provider,
        })
    }
}

/// Wrap the clone-and-run payload in a wall-clock guard so a hung run can't
/// bill the box forever. TERM first (checkpoint-friendly), KILL 30s later.
/// GNU coreutils `timeout` is on the box image.
pub fn wrap_with_timeout(script: &str, timeout_secs: u64) -> String {
    format!(
        "timeout --signal=TERM --kill-after=30s {timeout_secs} bash -c {q}\n\
         rc=$?\n\
         if [ \"$rc\" = 124 ]; then echo \"orx: run timed out after {timeout_secs}s\" >&2; fi\n\
         exit $rc",
        q = super::ssh::sh_quote(script),
    )
}

pub(crate) fn is_not_found_error(message: &str) -> bool {
    message.contains("(404 ")
}

/// OpenResearch hides a box from an account outside its organization, so a 404 for it says
/// nothing about whether it still bills.
#[derive(Debug)]
pub struct OtherAccount(pub String);

impl std::fmt::Display for OtherAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for OtherAccount {}

/// After a 404 for box `sandbox_id` of organization `org`: Ok when the signed-in account
/// belongs to `org`, so the box is gone; `OtherAccount` when it doesn't.
pub(crate) async fn confirm_gone(creds: &Credentials, org: &str, sandbox_id: &str) -> Result<()> {
    if list_orgs(creds)
        .await?
        .orgs
        .iter()
        .any(|member| member.id == org)
    {
        return Ok(());
    }
    let account = current_user(creds)
        .await
        .map_or_else(|_| "signed in now".into(), |me| me.user.email);
    Err(OtherAccount(format!(
        "OpenResearch can't show box {sandbox_id} to the account {account}, which isn't in organization {org} that launched it, so orx can't tell whether it still runs. Sign in with an account in that organization (`orx login`)."
    ))
    .into())
}

/// Delete the box, retrying transient failures. A 404 is success — the box is
/// already gone (dashboard delete, billing sweeper) — which makes teardown
/// idempotent across supervisor restarts, once the signed-in account can see
/// `org`'s boxes.
pub async fn teardown(creds: &Credentials, org: &str, sandbox_id: &str) -> Result<()> {
    let mut last = None;
    for backoff_secs in [0u64, 2, 5, 10] {
        if backoff_secs > 0 {
            tokio::time::sleep(crate::config::paced(Duration::from_secs(backoff_secs))).await;
        }
        match delete_sandbox(creds, sandbox_id).await {
            Ok(()) => return Ok(()),
            Err(err) if is_not_found_error(&err.to_string()) => {
                return confirm_gone(creds, org, sandbox_id).await
            }
            Err(err) => last = Some(err),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("teardown failed")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flavor_gpu_defaults_to_one() {
        match parse_flavor("h100_sxm", 100, None).unwrap() {
            SandboxTarget::New {
                gpu,
                gpu_count,
                disk_gb,
                provider,
            } => {
                assert_eq!(gpu, "H100_SXM");
                assert_eq!(gpu_count, 1);
                assert_eq!(disk_gb, 100);
                assert!(provider.is_none());
            }
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn parse_flavor_sends_uppercase_gpu_id() {
        let body = crate::client::CreateSandboxBody {
            organization_id: "org".into(),
            rental_lease: None,
            target: parse_flavor("rtx_3090", 100, None).unwrap(),
        };
        let body = serde_json::to_value(&body).unwrap();
        assert_eq!(body["target"]["gpu"], "RTX_3090");
    }

    #[test]
    fn parse_flavor_gpu_with_count_and_provider() {
        match parse_flavor("h100_sxm:2", 250, Some("runpod".into())).unwrap() {
            SandboxTarget::New {
                gpu,
                gpu_count,
                disk_gb,
                provider,
            } => {
                assert_eq!(gpu, "H100_SXM");
                assert_eq!(gpu_count, 2);
                assert_eq!(disk_gb, 250);
                assert_eq!(provider.as_deref(), Some("runpod"));
            }
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn parse_flavor_cpu_defaults_to_eight_vcpus() {
        match parse_flavor("cpu5c", 100, None).unwrap() {
            SandboxTarget::NewCpu {
                cpu_flavor,
                vcpu_count,
            } => {
                assert_eq!(cpu_flavor, "cpu5c");
                assert_eq!(vcpu_count, 8);
            }
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn parse_flavor_cpu_with_vcpus() {
        match parse_flavor("cpu5m:32", 100, None).unwrap() {
            SandboxTarget::NewCpu {
                cpu_flavor,
                vcpu_count,
            } => {
                assert_eq!(cpu_flavor, "cpu5m");
                assert_eq!(vcpu_count, 32);
            }
            other => panic!("wrong target: {other:?}"),
        }
    }

    #[test]
    fn parse_flavor_rejects_bad_suffix_and_empty() {
        assert!(parse_flavor("h100_sxm:x", 100, None).is_err());
        assert!(parse_flavor("h100_sxm:0", 100, None).is_err());
        assert!(parse_flavor("", 100, None).is_err());
        assert!(parse_flavor(":2", 100, None).is_err());
    }

    #[test]
    fn timeout_wrapper_quotes_and_guards() {
        let wrapped = wrap_with_timeout("echo 'hi there'", 14400);
        assert!(wrapped.starts_with("timeout --signal=TERM --kill-after=30s 14400 bash -c "));
        // The payload survives quoting (embedded single quotes escaped).
        assert!(wrapped.contains("'echo '\\''hi there'\\'''"));
        assert!(wrapped.contains("rc=$?"));
        assert!(wrapped.trim_end().ends_with("exit $rc"));
    }

    #[test]
    fn run_dir_matches_ssh_convention() {
        assert_eq!(run_dir("abc"), ".orx/runs/abc");
    }
}
