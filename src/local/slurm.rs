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
    // HF and Modal CPU shapes would become the nonsense GRES `gpu:cpu-basic` and die
    // with an opaque sbatch error.
    if let Some(flavor) = &args.flavor {
        let flavor = flavor.trim();
        if flavor.to_ascii_lowercase().starts_with("cpu") && !flavor.eq_ignore_ascii_case("cpu") {
            return Err(anyhow!(
                "--flavor {flavor} isn't a Slurm GPU request. For a CPU-only run pass \
                 --flavor cpu and ask for cores with --cpus (e.g. --cpus 8); for GPUs name \
                 them, e.g. --flavor h100:2."
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
    let (directives, sbatch_args) = directives(args, settings)?;
    options["directives"] = serde_json::to_value(directives)?;
    options["sbatch_args"] = serde_json::to_value(sbatch_args)?;
    Ok(LaunchSpec {
        kind: "slurm_job".into(),
        target: Some(host),
        options,
    })
}

/// `#SBATCH` lines: the ones orx writes, where flags beat the saved defaults and neither
/// leaves the cluster's own, then the user's own arguments, saved before per-run.
fn directives(
    args: &crate::ExpRunArgs,
    settings: slurm::SlurmSettings,
) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    let time_limit_secs =
        slurm::resolve_time_limit(args.timeout.as_deref(), settings.time_limit.as_deref())?;
    // Worth asking for explicitly on a GPU run: a partition that hands out one core and
    // `DefMemPerCPU` with it will starve a dataloader feeding an H100.
    let cpus_per_task = args.cpus.or(settings.cpus_per_task).filter(|c| *c > 0);
    let mem = match args.mem.as_deref().or(settings.mem.as_deref()) {
        Some(m) => Some(slurm::parse_mem(m)?),
        None => None,
    };
    // Some clusters' job-submit plugins refuse a job that names no task count; others refuse
    // `--ntasks` itself and turn this off.
    let mut directives = BTreeMap::new();
    if settings.request_one_task != Some(false) {
        directives.insert("ntasks".to_string(), "1".to_string());
    }
    let optional = [
        ("time", time_limit_secs.map(slurm::slurm_time)),
        ("partition", args.partition.clone().or(settings.partition)),
        ("account", settings.account),
        ("qos", args.qos.clone().or(settings.qos)),
        ("gres", args.flavor.as_deref().and_then(slurm::resolve_gres)),
        ("cpus-per-task", cpus_per_task.map(|c| c.to_string())),
        ("mem", mem),
        ("nodelist", args.nodelist.clone()),
        ("constraint", args.constraint.clone()),
        ("exclude", args.exclude.clone()),
    ];
    for (name, value) in optional {
        if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
            directives.insert(name.to_string(), value);
        }
    }

    let sbatch_args = settings
        .sbatch_args
        .iter()
        .chain(&args.sbatch_args)
        .map(|arg| slurm::parse_sbatch_arg(arg))
        .collect::<Result<Vec<_>>>()?;
    for (index, arg) in sbatch_args.iter().enumerate() {
        let Some(written) = slurm::repeated_line(arg, directives.keys().map(String::as_str)) else {
            continue;
        };
        if index < settings.sbatch_args.len() {
            return Err(anyhow!(
                "The saved sbatch option '{arg}' sets #SBATCH --{written}, which this run also \
                 sets. Leave that out of the run, or change the saved options with orx compute \
                 configure slurm --sbatch-arg."
            ));
        }
        let instead = match written {
            "ntasks" => {
                "To set your own, first: orx compute configure slurm --request-one-task false"
            }
            "time" => "Use --timeout instead",
            "partition" => "Use --partition instead",
            "account" => "Use orx compute configure slurm --account instead",
            "qos" => "Use --qos instead",
            "gres" => "Use --flavor instead",
            "cpus-per-task" => "Use --cpus instead",
            "mem" => "Use --mem instead",
            "nodelist" => "Use --nodelist instead",
            "constraint" => "Use --constraint instead",
            _ => "Use --exclude instead",
        };
        return Err(anyhow!(
            "--sbatch-arg '{arg}' sets #SBATCH --{written}, which orx already writes for this \
             run. {instead}."
        ));
    }
    Ok((directives, sbatch_args))
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
        let (saved, _) = directives(&args, settings()).unwrap();
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
        let (flags, _) = directives(&args, settings()).unwrap();
        assert_eq!(flags["time"], "02:00:00");
        assert_eq!(flags["cpus-per-task"], "8");
        assert_eq!(flags["mem"], "4000");
        assert_eq!(flags["gres"], "gpu:h100:2");
        args.mem = Some("lots".into());
        assert!(directives(&args, settings()).is_err());
        let (bare, _) = directives(
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
        directives(&args, settings).unwrap().0["partition"],
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

#[cfg(test)]
#[test]
fn cpu_flavor_requests_no_gpu_over_a_gpu_default_flavor() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    crate::config::set_compute_default(Some("slurm".into()), Some("gpu".into())).unwrap();
    let launch = |flavor: Option<&str>| {
        let mut args = crate::compute::tests::tinker_args();
        args.backend = None;
        args.host = Some("login".into());
        args.flavor = flavor.map(Into::into);
        crate::local::apply_compute_default(&mut args.backend, &mut args.flavor);
        crate::compute::validate_run_args(&args)?;
        launch_spec(&args, "true")
    };
    let gres = |flavor| launch(flavor).unwrap().options["directives"]["gres"].clone();
    assert_eq!(gres(None), "gpu:1");
    assert_eq!(gres(Some("cpu")), serde_json::Value::Null);
    assert_eq!(gres(Some("CPU")), serde_json::Value::Null);
    assert_eq!(gres(Some("h100:2")), "gpu:h100:2");
    assert!(launch(Some("cpu-basic")).is_err());
}

#[cfg(test)]
#[test]
fn qos_and_placement_reach_the_script_and_flags_beat_saved_qos() {
    let mut args = crate::compute::tests::tinker_args();
    let settings = || slurm::SlurmSettings {
        qos: Some("acc_ehpc".into()),
        ..Default::default()
    };
    assert_eq!(directives(&args, settings()).unwrap().0["qos"], "acc_ehpc");
    args.qos = Some("acc_debug".into());
    args.nodelist = Some("node[01-04]".into());
    args.constraint = Some("a100&ib".into());
    args.exclude = Some("node07".into());
    let (written, _) = directives(&args, settings()).unwrap();
    assert_eq!(written["qos"], "acc_debug");
    assert_eq!(written["nodelist"], "node[01-04]");
    assert_eq!(written["constraint"], "a100&ib");
    assert_eq!(written["exclude"], "node07");

    args.backend = Some("slurm".into());
    crate::compute::validate_run_args(&args).unwrap();
    args.constraint = Some("a100 ib".into());
    assert!(crate::compute::validate_run_args(&args).is_err());
    args.constraint = None;
    args.backend = Some("ssh".into());
    assert!(crate::compute::validate_run_args(&args).is_err());
}

#[cfg(test)]
#[test]
fn sbatch_args_follow_saved_then_per_run_and_never_fight_orx() {
    let mut args = crate::compute::tests::tinker_args();
    let settings = |partition: Option<&str>, one_task: Option<bool>| slurm::SlurmSettings {
        partition: partition.map(Into::into),
        sbatch_args: vec!["--rsc p=1:g=1".into()],
        request_one_task: one_task,
        ..Default::default()
    };
    args.sbatch_args = vec![
        "--mail-type=END".into(),
        "#SBATCH -N 1".into(),
        "--comment=\"two words\"".into(),
        "-v".into(),
    ];
    assert_eq!(
        directives(&args, settings(None, None)).unwrap().1,
        [
            "--rsc p=1:g=1",
            "--mail-type=END",
            "-N 1",
            "--comment=\"two words\"",
            "-v"
        ]
    );

    for owned in [
        "--output=elsewhere",
        "-o x",
        "-J name",
        "--job=x",
        "--requeue",
        "--array=1-4",
    ] {
        args.sbatch_args = vec![owned.into()];
        assert!(directives(&args, settings(None, None)).is_err(), "{owned}");
        assert!(slurm::parse_sbatch_arg(owned).is_err(), "{owned}");
    }
    for malformed in [
        "rsc=g=1",
        "--",
        "#SBATCH",
        "--rsc g=1\n#SBATCH --output=x",
        "--exclusive --array=1-4",
        "--exclusive --ntasks=4",
        "-vn4",
        "-pdebug -n4",
        "--rsc p=1 g=1",
        "--exclusive \\--array=1-4",
        "--\"output\"=elsewhere",
        "-'o' x",
        "--requeue#comment",
        "--exclusive #--array=1-4",
    ] {
        assert!(slurm::parse_sbatch_arg(malformed).is_err(), "{malformed}");
    }

    args.sbatch_args = vec!["-p debug".into()];
    let error = directives(&args, settings(Some("gpu"), None))
        .unwrap_err()
        .to_string();
    assert!(error.contains("--partition"), "{error}");
    assert!(directives(&args, settings(None, None)).is_ok());
    args.sbatch_args.clear();
    args.partition = Some("debug".into());
    let saved_partition = slurm::SlurmSettings {
        sbatch_args: vec!["-p gpu".into()],
        ..Default::default()
    };
    let error = directives(&args, saved_partition).unwrap_err().to_string();
    assert!(
        error.starts_with("The saved sbatch option '-p gpu'"),
        "{error}"
    );
    args.partition = None;

    args.sbatch_args = vec!["--ntasks=4".into()];
    let error = directives(&args, settings(None, None))
        .unwrap_err()
        .to_string();
    assert!(error.contains("--request-one-task false"), "{error}");
    let (written, extra) = directives(&args, settings(None, Some(false))).unwrap();
    assert!(!written.contains_key("ntasks"));
    assert_eq!(extra.last().unwrap(), "--ntasks=4");
}
