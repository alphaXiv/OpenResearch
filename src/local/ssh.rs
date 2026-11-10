//! SSH launch intents: the run executes as a detached process on one of your own boxes.
//! `--host` names an `~/.ssh/config` host alias (there's no hardware scheduler on a plain
//! server).

use crate::error::Result;
use crate::store::LaunchSpec;

/// The saved container is resolved by the engine over its own connection to the host.
pub fn launch_spec(args: &crate::ExpRunArgs, command: &str) -> Result<LaunchSpec> {
    let settings = crate::config::ssh_settings()?;
    let (host, options) = crate::jobs::ssh::resolve_options(args, &settings)?;
    let launch = crate::engine::adapters::ssh::LaunchOptions {
        run_sh: Some(format!(
            "{} {}",
            crate::compute::TENSORBOARD_LOGDIR_EXPORT,
            crate::compute::staged_script(command)
        )),
        container_reference: options.container,
        root: Some(crate::engine::runtime::engine_root(
            options.remote_root.as_deref(),
        )),
        ..Default::default()
    };
    Ok(LaunchSpec {
        kind: "ssh_job".into(),
        target: Some(host),
        options: serde_json::to_value(launch)?,
    })
}
