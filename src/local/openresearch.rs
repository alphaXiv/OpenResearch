//! OpenResearch launch intents: an ephemeral org-billed box for one run. The engine
//! provisions it at launch, waits for it, runs the payload over SSH, collects the log, and
//! releases the box.

use crate::client::list_orgs;
use crate::error::{anyhow, Result};
use crate::jobs::openresearch;
use crate::local::ssh_identity;
use crate::store::LaunchSpec;

/// Requires `--flavor <shape>` (`h100_sxm[:count]` or `cpu5c|cpu5g|cpu5m[:vcpus]`) and
/// `orx login`. The box is provisioned by the engine at launch.
pub async fn launch_spec(args: &crate::ExpRunArgs) -> Result<LaunchSpec> {
    if args.host.is_some() {
        return Err(anyhow!(
            "--host doesn't apply to --backend openresearch — the box is provisioned \
             for you (use --backend ssh to run on your own machine)."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest is k8s-only."));
    }
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend openresearch — provider base images are fixed \
             by OpenResearch; install project dependencies from the project's lockfile in the \
             run command."
        ));
    }
    let flavor = args.flavor.clone().ok_or_else(|| {
        anyhow!(
            "--backend openresearch requires --flavor: a GPU id like h100_sxm[:count] \
             or a CPU flavor like cpu5c[:vcpus]. See `orx compute` for the catalog."
        )
    })?;
    // Parsed now so a typo fails before any billing starts.
    openresearch::parse_flavor(&flavor, args.disk.unwrap_or(100), args.provider.clone())?;
    let timeout_seconds = match &args.timeout {
        Some(t) => crate::jobs::huggingface::parse_timeout(t)?,
        None => 4 * 3600,
    };

    let creds = crate::config::load_credentials()
        .await?
        .ok_or_else(|| anyhow!("OpenResearch requires `orx login`."))?;

    // Which org pays for the box.
    let org_id = match &args.org {
        Some(org) => org.clone(),
        None => {
            let orgs = list_orgs(&creds).await?.orgs;
            match orgs.len() {
                0 => {
                    return Err(anyhow!(
                        "You belong to no organization — create an OpenResearch organization first."
                    ))
                }
                1 => orgs[0].id.clone(),
                _ => {
                    let rows = orgs
                        .iter()
                        .map(|o| format!("  {}  {}", o.id, o.name))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Err(anyhow!(
                        "You belong to several orgs — pass --org <id>:\n{rows}"
                    ));
                }
            }
        }
    };

    // Boxes authorize org members' *registered* SSH keys, and orx authenticates
    // with whatever this machine's ssh offers — so a key registered from another
    // computer leaves the box online but unreachable.
    match ssh_identity::check(&creds).await {
        ssh_identity::KeyStatus::Matched { matched } => {
            if let Some(private) = ssh_identity::locked(&matched).await {
                let problem = format!(
                    "{} needs its passphrase, and no SSH agent holds it, so orx can't reach a box with it. {}, then launch again.",
                    ssh_identity::tilde(&private),
                    ssh_identity::unlock_hint(&private)
                );
                // macOS ssh may read the passphrase from the keychain without a prompt, and a
                // config `ssh -G` can't resolve may offer a key orx can't see.
                if cfg!(target_os = "macos") || !ssh_identity::offered_identities().await.1 {
                    eprintln!("warning: {problem}");
                } else {
                    return Err(anyhow!("{problem}"));
                }
            }
        }
        ssh_identity::KeyStatus::Unknown { reason } => {
            eprintln!("warning: could not check your registered SSH keys: {reason}");
        }
        // No keys at all is the api's own answer, so blocking is safe.
        ssh_identity::KeyStatus::NoneRegistered { local } => {
            return Err(anyhow!(
                "{}\n\n{}",
                ssh_identity::diagnosis(&[]),
                ssh_identity::remediation(&[], &local)
            ));
        }
        // ssh can still succeed with a key orx can't see: another agent, a token, or a
        // passphrase-protected identity file with no .pub beside it. Warn loudly
        // rather than block a launch that would have worked.
        ssh_identity::KeyStatus::NoLocalMatch { registered, local } => {
            eprintln!(
                "warning: {}\n\n{}",
                ssh_identity::diagnosis(&registered),
                ssh_identity::remediation(&registered, &local)
            );
        }
    }

    Ok(LaunchSpec {
        kind: "openresearch_job".into(),
        target: Some(org_id),
        options: serde_json::json!({
            "flavor": flavor,
            "disk_gb": args.disk.unwrap_or(100),
            "provider": args.provider,
            "timeout_seconds": timeout_seconds,
        }),
    })
}

/// Reword a 402 from `POST /sandboxes` (org has no available compute balance)
/// into the top-up action, surfacing the checkout URL the API attaches.
pub(crate) fn billing_friendly(err: crate::error::Error) -> crate::error::Error {
    let text = err.to_string();
    if !text.contains("(402 ") {
        return err;
    }
    let url = text
        .split("checkoutUrl\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|u| format!(" Top up at {u}"))
        .unwrap_or_default();
    anyhow!("Your org has no available compute balance, so no box was provisioned.{url}")
}
