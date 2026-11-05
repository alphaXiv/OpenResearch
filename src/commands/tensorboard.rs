//! `orx tensorboard <runId>`: the command that opens a run's TensorBoard logs.

use crate::engine::handle::Handle;
use crate::error::{anyhow, Result};
use crate::jobs::ssh::sh_quote;
use crate::jobs::BackendDescriptor;
use crate::store::{LaunchSpec, Store, StoredRun};

const PORT: u16 = 6006;

pub async fn run(args: crate::TensorboardArgs) -> Result<()> {
    let store = Store::open()?;
    let run = crate::local::resolve::resolve_run(&store, &args.run_id)?;
    let launch = store
        .engine_run_state(&run.id)?
        .and_then(|state| state.launch);
    println!("{}", viewer_command(&run, launch.as_ref())?);
    Ok(())
}

/// Where the run's `tensorboard/` directory is: its host (`None` for this machine) and run dir.
fn run_dir(run: &StoredRun, launch: Option<&LaunchSpec>) -> Result<(Option<String>, String)> {
    let containerized = launch.is_some_and(|launch| !launch.options["container"].is_null());
    let engine_dir = |root: &str| {
        let root = root.strip_prefix("~/").unwrap_or(root);
        format!("{root}/runs/{}", run.id)
    };
    Ok(match crate::engine::handle_of(run) {
        Some(Handle::Local { dir } | Handle::Tinker { dir }) => (None, dir),
        Some(Handle::Ssh { host, root }) if !containerized => (Some(host), engine_dir(&root)),
        Some(Handle::Slurm { host, root, .. }) => (Some(host), engine_dir(&root)),
        Some(Handle::Legacy { descriptor }) => legacy_run_dir(&run.id, &descriptor)?,
        None if !crate::engine::is_engine_row(run) => {
            legacy_run_dir(&run.id, &BackendDescriptor::parse(&run.backend_json)?)?
        }
        _ => return Err(refusal()),
    })
}

fn legacy_run_dir(run_id: &str, backend: &BackendDescriptor) -> Result<(Option<String>, String)> {
    Ok(match backend.kind.as_str() {
        "local_job" | "tinker_job" => {
            let dir = crate::jobs::localbox::run_dir(run_id);
            (None, dir.to_string_lossy().into_owned())
        }
        "ssh_job" if backend.ssh_container.is_none() => {
            let (host, dir) = backend.ssh_ref()?;
            (Some(host.into()), dir.into())
        }
        "slurm_job" => (
            Some(backend.slurm_ref()?.0.into()),
            crate::jobs::slurm::run_dir(backend.remote_root.as_deref(), run_id),
        ),
        _ => return Err(refusal()),
    })
}

fn refusal() -> crate::error::Error {
    anyhow!("TensorBoard logs are kept only for local, Slurm, and SSH runs outside a container")
}

fn viewer_command(run: &StoredRun, launch: Option<&LaunchSpec>) -> Result<String> {
    let (host, dir) = run_dir(run, launch)?;
    let Some(host) = host else {
        let logdir = std::path::Path::new(&dir).join("tensorboard");
        return Ok(format!("tensorboard --logdir \"{}\"", logdir.display()));
    };
    let remote = format!(
        "tensorboard --logdir {}/tensorboard --port {PORT}",
        crate::jobs::ssh::shell_path(&dir)
    );
    Ok(format!(
        "ssh -L {PORT}:localhost:{PORT} {} {}\nThen open http://localhost:{PORT}",
        sh_quote(&host),
        sh_quote(&remote)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, backend_json: &str) -> StoredRun {
        StoredRun {
            id: id.into(),
            experiment_id: "exp".into(),
            project_id: "project".into(),
            status: "done".into(),
            backend_json: backend_json.into(),
            command: "train".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }
    }

    fn command(json: &str) -> Result<String> {
        viewer_command(&run("r1", json), None)
    }

    #[test]
    fn local_runs_point_at_the_run_directory() {
        let logdir = crate::jobs::localbox::run_dir("r1").join("tensorboard");
        assert_eq!(
            command(r#"{"kind":"local_job"}"#).unwrap(),
            format!("tensorboard --logdir \"{}\"", logdir.display())
        );
        let engine = r#"{"kind":"engine","jobId":"/data/local-runs/r1","handle":{"type":"Local","dir":"/data/local-runs/r1"}}"#;
        let logdir = std::path::Path::new("/data/local-runs/r1").join("tensorboard");
        assert_eq!(
            command(engine).unwrap(),
            format!("tensorboard --logdir \"{}\"", logdir.display())
        );
    }

    #[test]
    fn ssh_and_slurm_runs_tunnel_to_the_remote_run_directory() {
        for kind in ["ssh_job", "slurm_job"] {
            let json =
                format!(r#"{{"kind":"{kind}","namespace":"gpu-box","jobId":".orx/runs/r1"}}"#);
            assert!(command(&json).unwrap().starts_with(
                "ssh -L 6006:localhost:6006 'gpu-box' 'tensorboard --logdir \"$HOME/.orx/runs/r1\"/tensorboard --port 6006'"
            ));
        }
        let slurm = r#"{"kind":"slurm_job","namespace":"gpu-box","jobId":"7","remoteRoot":"/scratch/me/orx"}"#;
        assert!(command(slurm).unwrap().contains(
            "'tensorboard --logdir '\\''/scratch/me/orx/runs/r1'\\''/tensorboard --port 6006'"
        ));
        let engine =
            r#"{"kind":"engine","handle":{"type":"Ssh","host":"gpu-box","root":"~/.orx/engine"}}"#;
        assert!(command(engine).unwrap().starts_with(
            "ssh -L 6006:localhost:6006 'gpu-box' 'tensorboard --logdir \"$HOME/.orx/engine/runs/r1\"/tensorboard --port 6006'"
        ));
        let engine_slurm = r#"{"kind":"engine","jobId":"7","handle":{"type":"Slurm","host":"login","root":"/scratch/orx/engine","job":"7"}}"#;
        assert!(command(engine_slurm)
            .unwrap()
            .contains("/scratch/orx/engine/runs/r1"));
    }

    #[test]
    fn container_and_ephemeral_runs_are_refused() {
        let container = r#"{"kind":"ssh_job","namespace":"gpu-box","jobId":".orx/runs/r1","sshContainer":{"reference":"img","id":"c1","startedAt":"t","runDir":"/root/.orx/runs/r1"}}"#;
        for json in [container, r#"{"kind":"modal_job"}"#, r#"{"kind":"hf_job"}"#] {
            assert!(command(json).is_err(), "{json}");
        }
        let engine =
            r#"{"kind":"engine","handle":{"type":"Ssh","host":"gpu-box","root":"~/.orx/engine"}}"#;
        let launch = LaunchSpec {
            kind: "ssh_job".into(),
            target: Some("gpu-box".into()),
            options: serde_json::json!({"container": {"reference": "img"}}),
        };
        assert!(viewer_command(&run("r1", engine), Some(&launch)).is_err());
        let submitting = r#"{"kind":"engine"}"#;
        assert!(command(submitting).is_err());
    }
}
