//! `orx ssh connect <host>` — establish the shared, multiplexed SSH session
//! that every background orx call rides on.
//!
//! Why this exists rather than "just run `ssh <host>`": orx keeps its control
//! socket in a private `/tmp/orx-ssh-<uid>-<hash>/` directory, so a plain login
//! authenticates a session orx cannot see. On a cluster whose sshd requires
//! `publickey,keyboard-interactive` — BU's SCC, where the second factor is Duo
//! — `BatchMode=yes` removes keyboard-interactive from the client's candidate
//! list entirely, so background calls can NEVER authenticate on their own. The
//! master this command leaves behind is the credential.

use crate::error::{anyhow, Result};
use crate::jobs::ssh::{self, SshTarget};
use crate::{SshArgs, SshCommand, SshConnectArgs};

pub async fn run(args: SshArgs) -> Result<()> {
    match args.command {
        SshCommand::Connect(a) => connect(a).await,
        SshCommand::Status(a) => status(a).await,
    }
}

async fn connect(args: SshConnectArgs) -> Result<()> {
    let host = args.host.trim();
    if host.is_empty() {
        return Err(anyhow!("A host is required: `orx ssh connect <alias>`."));
    }
    let target = SshTarget::alias(host);
    if ssh::master_is_running(&target).await.unwrap_or(false) {
        println!("\u{2713} Already connected to {host}.");
        return Ok(());
    }

    // Inherit stdio so the user can actually answer a password or Duo prompt —
    // this is the one orx path that is allowed to be interactive.
    let cmd_args = ssh::interactive_args(&target)?;
    println!("Connecting to {host} (approve any prompt)\u{2026}");
    let status = tokio::process::Command::new("ssh")
        .args(cmd_args)
        .status()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow!("`ssh` not found on PATH — orx needs the OpenSSH client.")
            } else {
                anyhow!("Could not run ssh: {e}")
            }
        })?;

    if ssh::master_is_running(&target).await.unwrap_or(false) {
        println!("\u{2713} Connected to {host}. orx will reuse this session for submits, status polls and logs.");
        return Ok(());
    }
    if !status.success() {
        return Err(anyhow!(
            "Could not authenticate to {host}. If this cluster uses Duo, approve the prompt \
             and try again."
        ));
    }
    // Windows' OpenSSH cannot multiplex, so there is nothing to keep alive.
    #[cfg(unix)]
    {
        Err(anyhow!(
            "Authenticated to {host}, but the shared session did not stay open. Check that \
             your ssh supports ControlMaster (OpenSSH on unix does)."
        ))
    }
    #[cfg(not(unix))]
    {
        println!("\u{2713} Authenticated to {host}. (Windows OpenSSH cannot multiplex, so each background call authenticates on its own.)");
        Ok(())
    }
}

async fn status(args: SshConnectArgs) -> Result<()> {
    let host = args.host.trim();
    let target = SshTarget::alias(host);
    if ssh::master_is_running(&target).await.unwrap_or(false) {
        println!("\u{2713} {host}: shared session is live.");
        if let Some(control_path) = ssh::control_path_for_riding(&target) {
            println!(
                "  It's a real socket on disk, not something private to orx — anything running \
                 as you can ride it directly:\n\n    ssh -S {} -- {host} '<command>'",
                control_path.display(),
            );
        }
    } else {
        println!(
            "\u{2717} {host}: no shared session. Run `{} ssh connect {host}`.",
            crate::invocation::orx()
        );
    }
    Ok(())
}
