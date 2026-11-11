//! Slurm launch intents: a batch job on a Slurm cluster reached via its login node.
//! `--host` names an `~/.ssh/config` alias (defaultable in the slurm settings); `--flavor`
//! asks for GPUs as a GRES spec, `--cpus`/`--mem` for cores and host memory.

use crate::error::{anyhow, Result};
use crate::jobs::{slurm, BackendDescriptor};
use crate::store::LaunchSpec;
use std::collections::BTreeMap;

/// The launch-summary line for a Slurm job that asked for cores or memory.
pub fn requested_resources(backend: &BackendDescriptor) -> Option<String> {
    if backend.cpus_per_task.is_none() && backend.mem.is_none() {
        return None;
    }
    Some(format!(
        "cpus {}  mem {}",
        backend
            .cpus_per_task
            .map_or_else(|| "partition default".into(), |c| c.to_string()),
        backend.mem.as_deref().unwrap_or("partition default")
    ))
}

/// The `exp status` line. Runs launched before cpus/mem were recorded may have
/// requested them, so an empty descriptor is unknown rather than the default.
pub fn status_resources(backend: &BackendDescriptor) -> String {
    match requested_resources(backend) {
        Some(resources) => format!("requested {resources}"),
        None => "requested cpus/mem: unknown (older run or partition default)".into(),
    }
}

pub fn launch_spec(args: &crate::ExpRunArgs, command: &str) -> Result<LaunchSpec> {
    if args.image.is_some() {
        return Err(anyhow!(
            "--image doesn't apply to --backend slurm — the job runs in your cluster \
             environment (modules/conda), not a container."
        ));
    }
    if args.manifest.is_some() {
        return Err(anyhow!("--manifest only applies with --backend k8s."));
    }
    // A muscle-memory `--flavor cpu` (HF/Modal habit) would become the
    // nonsense GRES `gpu:cpu` and die with an opaque sbatch error.
    if let Some(f) = &args.flavor {
        if f.trim().to_ascii_lowercase().starts_with("cpu") {
            return Err(anyhow!(
                "--flavor names GPUs on --backend slurm (e.g. h100:2). For a CPU-only \
                 run omit --flavor and ask for cores with --cpus (e.g. --cpus 8)."
            ));
        }
    }

    let settings = slurm::load_settings()?.unwrap_or_default();
    if let Some(root) = &settings.remote_root {
        crate::jobs::ssh::validate_remote_root(root)?;
    }
    let host = args
        .host
        .clone()
        .or_else(|| settings.host.clone())
        .ok_or_else(|| {
            anyhow!(
                "--backend slurm needs a login node: pass --host <alias> (an ~/.ssh/config \
                 alias) or use a configured default Slurm host."
            )
        })?;

    let launch = crate::engine::adapters::ssh::LaunchOptions {
        run_sh: Some(format!(
            "{} {}",
            crate::compute::TENSORBOARD_LOGDIR_EXPORT,
            crate::compute::staged_script(command)
        )),
        root: Some(crate::engine::runtime::engine_root(
            settings.remote_root.as_deref(),
        )),
        ..Default::default()
    };
    let mut options = serde_json::to_value(launch)?;
    options["timeout_seconds"] = serde_json::to_value(slurm::resolve_time_limit(
        args.timeout.as_deref(),
        settings.time_limit.as_deref(),
    )?)?;
    options["directives"] = serde_json::to_value(directives(args, settings)?)?;
    Ok(LaunchSpec {
        kind: "slurm_job".into(),
        target: Some(host),
        options,
    })
}

/// `#SBATCH` options: flags beat the saved defaults, and neither leaves the cluster's own.
fn directives(
    args: &crate::ExpRunArgs,
    settings: slurm::SlurmSettings,
) -> Result<BTreeMap<String, String>> {
    let time_limit_secs =
        slurm::resolve_time_limit(args.timeout.as_deref(), settings.time_limit.as_deref())?;
    // Worth asking for explicitly on a GPU run: a partition that hands out one core and
    // `DefMemPerCPU` with it will starve a dataloader feeding an H100.
    let cpus_per_task = args.cpus.or(settings.cpus_per_task).filter(|c| *c > 0);
    let mem = match args.mem.as_deref().or(settings.mem.as_deref()) {
        Some(m) => Some(slurm::parse_mem(m)?),
        None => None,
    };
    // Some clusters' job-submit plugins refuse a job that names no task count.
    let mut directives = BTreeMap::from([("ntasks".to_string(), "1".to_string())]);
    let optional = [
        ("time", time_limit_secs.map(slurm::slurm_time)),
        ("partition", args.partition.clone().or(settings.partition)),
        ("account", settings.account),
        ("gres", args.flavor.as_deref().and_then(slurm::resolve_gres)),
        ("cpus-per-task", cpus_per_task.map(|c| c.to_string())),
        ("mem", mem),
    ];
    for (name, value) in optional {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            directives.insert(name.to_string(), value);
        }
    }
    Ok(directives)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_beat_saved_defaults_and_blank_values_are_omitted() {
        let settings = || slurm::SlurmSettings {
            partition: Some("gpu".into()),
            account: Some(" ".into()),
            time_limit: Some("24h".into()),
            cpus_per_task: Some(4),
            mem: Some("32G".into()),
            ..Default::default()
        };
        let mut args = crate::compute::tests::tinker_args();
        let saved = directives(&args, settings()).unwrap();
        assert_eq!(
            saved,
            BTreeMap::from(
                [
                    ("ntasks", "1"),
                    ("time", "1-00:00:00"),
                    ("partition", "gpu"),
                    ("cpus-per-task", "4"),
                    ("mem", "32G"),
                ]
                .map(|(name, value)| (name.to_string(), value.to_string()))
            )
        );
        args.timeout = Some("2h".into());
        args.cpus = Some(8);
        args.mem = Some("4000".into());
        args.flavor = Some("h100:2".into());
        let flags = directives(&args, settings()).unwrap();
        assert_eq!(flags["time"], "02:00:00");
        assert_eq!(flags["cpus-per-task"], "8");
        assert_eq!(flags["mem"], "4000");
        assert_eq!(flags["gres"], "gpu:h100:2");
        args.mem = Some("lots".into());
        assert!(directives(&args, settings()).is_err());
        let bare = directives(
            &crate::compute::tests::tinker_args(),
            slurm::SlurmSettings::default(),
        )
        .unwrap();
        assert_eq!(bare.keys().collect::<Vec<_>>(), ["ntasks"]);
    }
}

#[cfg(test)]
#[test]
fn per_run_partition_overrides_default_and_preserves_lists() {
    let mut args = crate::compute::tests::tinker_args();
    args.partition = Some("gpu,batch".into());
    let settings = slurm::SlurmSettings {
        partition: Some("cpu".into()),
        ..Default::default()
    };
    assert_eq!(
        directives(&args, settings).unwrap()["partition"],
        "gpu,batch"
    );
    args.backend = Some("slurm".into());
    crate::compute::validate_run_args(&args).unwrap();
    for partition in ["", "gpu,,cpu", "gpu\n--account=other", "gpu cpu"] {
        args.partition = Some(partition.into());
        assert!(crate::compute::validate_run_args(&args).is_err());
    }
    args.partition = Some("gpu".into());
    args.backend = Some("ssh".into());
    assert!(crate::compute::validate_run_args(&args).is_err());
}
