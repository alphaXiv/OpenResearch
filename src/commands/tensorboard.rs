//! `orx tensorboard <runId>`: the command that opens a run's TensorBoard logs.

use crate::error::{anyhow, Result};
use crate::jobs::ssh::sh_quote;
use crate::jobs::BackendDescriptor;
use crate::store::Store;

const PORT: u16 = 6006;

pub async fn run(args: crate::TensorboardArgs) -> Result<()> {
    let store = Store::open()?;
    let run = crate::local::resolve::resolve_run(&store, &args.run_id)?;
    let backend = BackendDescriptor::parse(&run.backend_json)?;
    println!("{}", viewer_command(&run.id, &backend)?);
    Ok(())
}

fn viewer_command(run_id: &str, backend: &BackendDescriptor) -> Result<String> {
    let (host, dir) = match backend.kind.as_str() {
        "local_job" | "tinker_job" => {
            let logdir = crate::jobs::localbox::run_dir(run_id).join("tensorboard");
            return Ok(format!("tensorboard --logdir \"{}\"", logdir.display()));
        }
        "ssh_job" if backend.ssh_container.is_none() => {
            let (host, dir) = backend.ssh_ref()?;
            (host, dir.to_string())
        }
        "slurm_job" => (backend.slurm_ref()?.0, crate::jobs::slurm::run_dir(run_id)),
        _ => {
            return Err(anyhow!(
                "TensorBoard logs are kept only for local, Slurm, and SSH runs outside a container"
            ))
        }
    };
    let remote = format!("tensorboard --logdir \"$HOME\"/{dir}/tensorboard --port {PORT}");
    Ok(format!(
        "ssh -L {PORT}:localhost:{PORT} {} {}\nThen open http://localhost:{PORT}",
        sh_quote(host),
        sh_quote(&remote)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(json: &str) -> BackendDescriptor {
        BackendDescriptor::parse(json).unwrap()
    }

    #[test]
    fn local_runs_point_at_the_run_directory() {
        let command = viewer_command("r1", &backend(r#"{"kind":"local_job"}"#)).unwrap();
        let logdir = crate::jobs::localbox::run_dir("r1").join("tensorboard");
        assert_eq!(
            command,
            format!("tensorboard --logdir \"{}\"", logdir.display())
        );
    }

    #[test]
    fn ssh_and_slurm_runs_tunnel_to_the_remote_run_directory() {
        for kind in ["ssh_job", "slurm_job"] {
            let json =
                format!(r#"{{"kind":"{kind}","namespace":"gpu-box","jobId":".orx/runs/r1"}}"#);
            let command = viewer_command("r1", &backend(&json)).unwrap();
            assert!(command.starts_with(
                "ssh -L 6006:localhost:6006 'gpu-box' 'tensorboard --logdir \"$HOME\"/.orx/runs/r1/tensorboard --port 6006'"
            ));
        }
    }

    #[test]
    fn container_and_ephemeral_runs_are_refused() {
        let container = r#"{"kind":"ssh_job","namespace":"gpu-box","jobId":".orx/runs/r1","sshContainer":{"reference":"img","id":"c1","startedAt":"t","runDir":"/root/.orx/runs/r1"}}"#;
        for json in [container, r#"{"kind":"modal_job"}"#, r#"{"kind":"hf_job"}"#] {
            assert!(viewer_command("r1", &backend(json)).is_err(), "{json}");
        }
    }
}
