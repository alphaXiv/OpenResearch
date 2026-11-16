//! The `instance` command group: standalone compute in an organization,
//! independent of any experiment.
//!
//!   orx instance create <orgId> --gpu <id> [--count N] [--disk GB] [--provider P]
//!   orx instance create <orgId> --cpu <cpu5c|cpu5g|cpu5m> [--vcpus 2|8|32]
//!   orx instance list <orgId> [--json]
//!   orx instance delete <sandboxId>
//!
//! Create is the CLI equivalent of the dashboard's org "Spin up" panel — it
//! hits `POST /sandboxes`, which provisions an org-level (not project-scoped)
//! box. List/delete double as the cleanup path for a `--backend openresearch`
//! box whose automatic teardown failed.

use crate::client::{
    create_sandbox, current_user, delete_sandbox, list_sandboxes, CreateSandboxBody, Sandbox,
    SandboxTarget,
};
use crate::error::{anyhow, require_credentials, Result};
use crate::{InstanceCommand, InstanceCreateArgs, InstanceDeleteArgs, InstanceListArgs};

pub async fn run(args: crate::InstanceArgs) -> Result<()> {
    let creds = require_credentials().await;
    match args.command {
        InstanceCommand::Create(create_args) => create(&creds, create_args).await,
        InstanceCommand::List(list_args) => list(&creds, list_args).await,
        InstanceCommand::Delete(delete_args) => delete(&creds, delete_args).await,
    }
}

/// A listed box plus what `instance list` derives for agents and scripts.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ListedInstance<'a> {
    #[serde(flatten)]
    sandbox: &'a Sandbox,
    /// `None` when the creator or the signed-in account is unknown.
    created_by_you: Option<bool>,
    failure: Option<String>,
    ssh_command: Option<String>,
}

fn created_by_you(s: &Sandbox, me: Option<&str>) -> Option<bool> {
    Some(s.created_by.as_deref()? == me?)
}

/// `ssh` into an online box, pinning its host key under the box's own id: providers
/// recycle proxy `host:port` pairs across boxes, so pinning the endpoint would
/// mismatch, while the alias still catches a key change on this box.
fn ssh_command(s: &Sandbox) -> Option<String> {
    if s.status != "online" {
        return None;
    }
    let (Some(user), Some(host), Some(port)) = (&s.ssh_username, &s.ssh_hostname, s.ssh_port)
    else {
        return None;
    };
    Some(format!(
        "ssh -p {port} -o HostKeyAlias=orx-instance-{} -o StrictHostKeyChecking=accept-new {user}@{host}",
        s.id
    ))
}

/// `orx instance list <orgId>` — every box in the org, one line each.
async fn list(creds: &crate::config::Credentials, args: InstanceListArgs) -> Result<()> {
    let (sandboxes, me) = tokio::join!(list_sandboxes(creds, &args.org_id), current_user(creds));
    let sandboxes = sandboxes?.sandboxes;
    // Ownership is a label; a slow or failing account call must not hide the boxes.
    let me = me.map(|me| me.user.id).map_err(|error| error.to_string());
    let listed: Vec<_> = sandboxes
        .iter()
        .map(|sandbox| ListedInstance {
            sandbox,
            created_by_you: created_by_you(sandbox, me.as_deref().ok()),
            failure: sandbox.failure(),
            ssh_command: ssh_command(sandbox),
        })
        .collect();
    if args.json {
        println!("{}", serde_json::to_string_pretty(&listed)?);
        return Ok(());
    }
    if listed.is_empty() {
        println!("No instances in org {}.", args.org_id);
        return Ok(());
    }
    if let Err(error) = &me {
        eprintln!("Could not read your account, so owners are not shown: {error}");
    }
    for item in &listed {
        let s = item.sandbox;
        let shape = match (&s.gpu, s.vcpu_count) {
            (Some(gpu), _) => format!("{} x{}", gpu, s.gpu_count.unwrap_or(1)),
            (None, Some(vcpus)) => format!("{vcpus} vcpus"),
            _ => "byom".to_string(),
        };
        let price = s
            .price_per_hour
            .map(|p| format!("  ${p:.2}/hr"))
            .unwrap_or_default();
        let endpoint = match (&s.ssh_username, &s.ssh_hostname, s.ssh_port) {
            (Some(user), Some(host), Some(port)) => format!("  {user}@{host}:{port}"),
            _ => String::new(),
        };
        let owner = match item.created_by_you {
            Some(true) => "  [yours]",
            Some(false) => "  [another org member's]",
            None => "",
        };
        println!(
            "{}  {:<12}  {shape}{price}{endpoint}{owner}",
            s.id, s.status
        );
        if let Some(failure) = &item.failure {
            println!("    {failure}");
        }
        if !s.connections.is_empty() {
            println!("    {} live SSH session(s) on it now.", s.connections.len());
        }
        if s.status == "offline" && s.provider_name.is_some() {
            println!(
                "    It still bills while offline. Delete it with `orx instance delete {}` if you no longer need it.",
                s.id
            );
        }
        if let Some(command) = &item.ssh_command {
            println!("    {command}");
        }
    }
    Ok(())
}

/// `orx instance delete <sandboxId>` — terminate the box and its provider machine.
async fn delete(creds: &crate::config::Credentials, args: InstanceDeleteArgs) -> Result<()> {
    delete_sandbox(creds, &args.sandbox_id).await?;
    println!(
        "\u{2713} Termination requested for instance {}.",
        args.sandbox_id
    );
    Ok(())
}

/// `orx instance create <orgId> …` — provision a standalone GPU or CPU instance.
async fn create(creds: &crate::config::Credentials, args: InstanceCreateArgs) -> Result<()> {
    // Resolve the target: exactly one of --gpu or --cpu.
    if args.gpu.is_some() && args.cpu.is_some() {
        return Err(anyhow!("Pass exactly one of --gpu or --cpu."));
    }
    if args.provider.is_some() && args.gpu.is_none() {
        return Err(anyhow!(
            "--provider only applies with --gpu (it selects among new GPU offers)."
        ));
    }
    let target = if let Some(gpu) = &args.gpu {
        SandboxTarget::New {
            gpu: gpu.to_ascii_uppercase(),
            gpu_count: args.count.unwrap_or(1),
            disk_gb: args.disk.unwrap_or(100),
            // Omitted = the server picks the cheapest matching offer across all
            // providers (matching the dashboard's spin-up). The server validates
            // the name and 400s on an unknown provider, so no client-side check.
            provider: args.provider.clone(),
        }
    } else if let Some(cpu_flavor) = &args.cpu {
        SandboxTarget::NewCpu {
            cpu_flavor: cpu_flavor.clone(),
            vcpu_count: args.vcpus.unwrap_or(8),
        }
    } else {
        return Err(anyhow!(
            "Choose compute: --gpu <id> [--count N] [--disk GB] [--provider P], \
             or --cpu <cpu5c|cpu5g|cpu5m> [--vcpus 2|8|32]. \
             See `orx compute` for available GPUs."
        ));
    };

    let sandbox = create_sandbox(
        creds,
        &CreateSandboxBody {
            organization_id: args.org_id.clone(),
            rental_lease: None,
            target,
        },
    )
    .await?
    .sandbox;

    println!("\u{2713} Instance requested");
    println!("  id:       {}", sandbox.id);
    println!("  status:   {}", sandbox.status);
    println!("  type:     {}", sandbox.machine_type);
    if let Some(provider) = &sandbox.provider_name {
        println!("  provider: {}", provider);
    }
    match (&sandbox.gpu, sandbox.vcpu_count) {
        (Some(gpu), _) => {
            println!("  gpu:      {} x{}", gpu, sandbox.gpu_count.unwrap_or(1));
        }
        (None, Some(vcpus)) => println!("  vcpus:    {}", vcpus),
        _ => {}
    }
    if let Some(price) = sandbox.price_per_hour {
        println!("  price:    ${:.2}/hr", price);
    }
    println!(
        "\n  The box is provisioning. `orx instance list {}` shows its SSH command once it's online, or why it failed.",
        args.org_id
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(status: &str) -> Sandbox {
        serde_json::from_value(serde_json::json!({
            "id": "sb_1", "organizationId": "org_1",
            "sshHostname": "ssh7.vast.ai", "sshPort": 13440, "sshUsername": "root",
            "status": status, "machineType": "persistent", "createdBy": "user_1",
            "updatedAt": "2026-11-16T00:00:00Z", "provisionWarnings": null,
            "failureCode": null, "failureMessage": null, "failedAt": null,
            "providerName": "vast", "providerInstanceId": "1", "pricePerHour": 0.33,
            "gpu": "RTX_4090", "gpuCount": 1, "vcpuCount": null
        }))
        .unwrap()
    }

    #[test]
    fn json_lists_box_with_owner_and_pinned_ssh_command() {
        let online = sandbox("online");
        let json = serde_json::to_value(ListedInstance {
            sandbox: &online,
            created_by_you: created_by_you(&online, Some("user_1")),
            failure: online.failure(),
            ssh_command: ssh_command(&online),
        })
        .unwrap();
        assert_eq!(json["id"], "sb_1");
        assert_eq!(json["status"], "online");
        assert_eq!(json["sshPort"], 13440);
        assert_eq!(json["createdByYou"], true);
        assert!(json["failure"].is_null());
        let command = json["sshCommand"].as_str().unwrap();
        assert!(
            command.contains("HostKeyAlias=orx-instance-sb_1"),
            "{command}"
        );
        assert!(
            command.contains("StrictHostKeyChecking=accept-new"),
            "{command}"
        );
        assert!(!command.contains("StrictHostKeyChecking=no"), "{command}");
        assert_eq!(ssh_command(&sandbox("offline")), None);
    }

    #[test]
    fn owner_is_unknown_rather_than_guessed() {
        let mut box_ = sandbox("failed");
        assert_eq!(created_by_you(&box_, Some("user_1")), Some(true));
        assert_eq!(created_by_you(&box_, Some("user_2")), Some(false));
        assert_eq!(created_by_you(&box_, None), None);
        box_.created_by = None;
        assert_eq!(created_by_you(&box_, Some("user_1")), None);
    }

    #[test]
    fn failed_box_carries_the_service_reason() {
        let mut box_ = sandbox("failed");
        box_.failure_code = Some("readiness_timeout".into());
        box_.failure_message = Some("Not reachable within five minutes.".into());
        assert_eq!(
            box_.failure().as_deref(),
            Some("readiness_timeout: Not reachable within five minutes.")
        );
    }
}
