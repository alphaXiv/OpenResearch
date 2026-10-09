//! The `tensorboard` command — show the event directory and copyable command
//! recorded for one run. ORX does not start or supervise TensorBoard.

use crate::error::{anyhow, Result};
use crate::jobs::{BackendDescriptor, TrackingDescriptor};
use crate::store::Store;

fn shell_path(path: &str) -> String {
    if let Some(relative) = path.strip_prefix("~/") {
        format!("\"$HOME\"/{}", crate::jobs::ssh::sh_quote(relative))
    } else {
        crate::jobs::ssh::sh_quote(path)
    }
}

fn tensorboard_command(root_log_dir: &str, remote: bool) -> String {
    let mut command = format!("tensorboard --logdir {}", shell_path(root_log_dir));
    if remote {
        command.push_str(" --host 127.0.0.1 --port 6006");
    }
    command
}

fn remote_command(host: &str, root_log_dir: &str) -> String {
    format!(
        "ssh -L 6006:localhost:6006 {} {}",
        crate::jobs::ssh::sh_quote(host),
        crate::jobs::ssh::sh_quote(&tensorboard_command(root_log_dir, true))
    )
}

pub async fn run(args: crate::TensorboardArgs) -> Result<()> {
    let store = Store::open()?;
    let run = store
        .get_run(&args.run_id)?
        .ok_or_else(|| anyhow!("Run {} not found in the local store.", args.run_id))?;
    let descriptor = BackendDescriptor::parse(&run.backend_json)?;
    let Some((log_dir, root_log_dir)) =
        descriptor.tracking.iter().find_map(|record| match record {
            TrackingDescriptor::Tensorboard {
                log_dir,
                root_log_dir,
                ..
            } => Some((log_dir.as_str(), root_log_dir.as_str())),
            TrackingDescriptor::Trackio { .. } => None,
        })
    else {
        println!(
            "Run {} was not launched with TensorBoard tracking.",
            args.run_id
        );
        return Ok(());
    };

    println!("Run       {}", args.run_id);
    println!("Log dir   {log_dir}");
    println!("Root      {root_log_dir}");
    println!();
    match descriptor.kind.as_str() {
        "slurm_job" | "ssh_job" => {
            let host = descriptor
                .namespace
                .as_deref()
                .ok_or_else(|| anyhow!("The recorded {} run has no SSH host.", descriptor.kind))?;
            println!("{}", remote_command(host, root_log_dir));
        }
        "local_job" => println!("{}", tensorboard_command(root_log_dir, false)),
        _ => println!(
            "Run TensorBoard where the job filesystem is available:\n\
             {}",
            tensorboard_command(root_log_dir, false)
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trackio_umbrella_tensorboard_remote_command_starts_the_server_over_ssh() {
        assert_eq!(
            remote_command("cluster-login", "~/.orx/tensorboard/demo"),
            "ssh -L 6006:localhost:6006 'cluster-login' 'tensorboard --logdir \"$HOME\"/'\\''.orx/tensorboard/demo'\\'' --host 127.0.0.1 --port 6006'"
        );
    }

    #[test]
    fn trackio_umbrella_tensorboard_quotes_local_paths_and_expands_home_safely() {
        assert_eq!(
            tensorboard_command("/tmp/tensor board;$(false)", false),
            "tensorboard --logdir '/tmp/tensor board;$(false)'"
        );
        assert_eq!(
            shell_path("~/tensor board/a'b"),
            "\"$HOME\"/'tensor board/a'\\''b'"
        );
    }
}
