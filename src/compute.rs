//! Backend-agnostic compute lifecycle and immutable source snapshots.
//!
//! Local Git remains the experiment-history database. A launch never asks a
//! remote backend to clone that history: it archives the exact recorded commit
//! once, addresses the archive by SHA-256, and hands that immutable payload to
//! the selected provider adapter.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::error::{anyhow, Result};
use crate::jobs::BackendDescriptor;
use crate::local::model::{LocalExperiment, LocalProject};
use crate::store::{LaunchSpec, Store, StoredRun};

#[derive(Debug, Clone)]
pub struct SourceSnapshot {
    pub revision: String,
    pub digest: String,
    pub size: u64,
    pub path: PathBuf,
    pub ray_package: Option<(String, PathBuf)>,
}

impl SourceSnapshot {
    pub fn create(
        project: &LocalProject,
        experiment: &LocalExperiment,
        include_ray_package: bool,
    ) -> Result<Self> {
        let repo = Path::new(&project.repo_path);
        let revision = crate::local::git::local_head_sha(repo, &experiment.branch_name)?;
        let dir = crate::store::data_dir().join("source-snapshots");
        prepare_snapshot_dir(&dir)?;

        let nonce = uuid::Uuid::new_v4();
        let tar_tmp = dir.join(format!(".{nonce}.tar"));
        archive(repo, &revision, "tar", &tar_tmp)?;
        let (digest, size) = digest_file(&tar_tmp)?;
        let path = dir.join(format!("{digest}.tar"));
        install_content_addressed(&tar_tmp, &path, &digest, size)?;

        let ray_package = if include_ray_package {
            let zip_tmp = dir.join(format!(".{nonce}.zip"));
            archive(repo, &revision, "zip", &zip_tmp)?;
            let (zip_digest, zip_size) = digest_file(&zip_tmp)?;
            let zip_path = dir.join(format!("{zip_digest}.zip"));
            install_content_addressed(&zip_tmp, &zip_path, &zip_digest, zip_size)?;
            Some((zip_digest, zip_path))
        } else {
            None
        };

        Ok(Self {
            revision,
            digest,
            size,
            path,
            ray_package,
        })
    }

    pub fn apply_to_descriptor(&self, descriptor: &mut BackendDescriptor) {
        descriptor.source_digest = Some(self.digest.clone());
        descriptor.source_path = Some(self.path.to_string_lossy().into_owned());
        descriptor.source_size = Some(self.size);
    }

    pub fn from_run(run: &StoredRun, descriptor: &BackendDescriptor) -> Result<Self> {
        let revision = run
            .commit_sha
            .clone()
            .ok_or_else(|| anyhow!("Run {} has no recorded source revision.", run.id))?;
        let digest = descriptor
            .source_digest
            .clone()
            .ok_or_else(|| anyhow!("Run {} has no recorded source digest.", run.id))?;
        let recorded_path = descriptor
            .source_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| anyhow!("Run {} has no recorded source archive.", run.id))?;
        let path = if recorded_path.is_file() {
            recorded_path
        } else {
            crate::store::data_dir()
                .join("source-snapshots")
                .join(format!("{digest}.tar"))
        };
        if !path.is_file() {
            return Err(anyhow!(
                "Run {} source archive is missing at {}.",
                run.id,
                path.display()
            ));
        }
        let size = descriptor
            .source_size
            .or_else(|| std::fs::metadata(&path).ok().map(|m| m.len()))
            .unwrap_or(0);
        let (actual_digest, actual_size) = digest_file(&path)?;
        if actual_digest != digest || (size != 0 && actual_size != size) {
            return Err(anyhow!(
                "Run {} source archive failed its digest check.",
                run.id
            ));
        }
        Ok(Self {
            revision,
            digest,
            size,
            path,
            ray_package: None,
        })
    }
}

fn archive(repo: &Path, revision: &str, format: &str, destination: &Path) -> Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(destination)?;
    let output = crate::local::git::git_command()
        .current_dir(repo)
        // Snapshots carry committed blob bytes; Git for Windows' system autocrlf would add CRs.
        .args(["-c", "core.autocrlf=false", "-c", "core.eol=lf"])
        .args(["archive", &format!("--format={format}"), revision])
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| anyhow!("Could not run git archive: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let _ = std::fs::remove_file(destination);
    Err(anyhow!(
        "git archive failed for {}: {}",
        revision,
        crate::local::git::failure_detail(&output)
    ))
}

fn prepare_snapshot_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        // Follow symlinks: a user-owned link to a user-owned directory is fine.
        let metadata = std::fs::metadata(path)?;
        if !metadata.file_type().is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(anyhow!(
                "Source snapshot directory {} is not owned by the current user.",
                path.display()
            ));
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(crate) fn digest_file(path: &Path) -> Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buf = [0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
        size += read as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), size))
}

fn install_content_addressed(
    source: &Path,
    destination: &Path,
    expected_digest: &str,
    expected_size: u64,
) -> Result<()> {
    if destination.exists() {
        let (digest, size) = digest_file(destination)?;
        if digest == expected_digest && size == expected_size {
            std::fs::remove_file(source)?;
            restrict_snapshot_file(destination)?;
            return Ok(());
        }
        std::fs::remove_file(destination)?;
    }
    match std::fs::rename(source, destination) {
        Ok(()) => restrict_snapshot_file(destination),
        Err(_err) if destination.exists() => {
            let (digest, size) = digest_file(destination)?;
            std::fs::remove_file(source)?;
            if digest == expected_digest && size == expected_size {
                restrict_snapshot_file(destination)
            } else {
                Err(anyhow!(
                    "Cached source snapshot {} failed its digest check.",
                    destination.display()
                ))
            }
        }
        Err(err) => Err(err.into()),
    }
}

fn restrict_snapshot_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

pub fn snapshot_script(archive_path: &str, command: &str) -> String {
    format!(
        "set -eo pipefail; mkdir -p repo; tar -xf {} -C repo; cd repo; {}",
        shell_quote(archive_path),
        command
    )
}

pub const TENSORBOARD_LOGDIR: &str = "TENSORBOARD_LOGDIR";

/// Run before `cd repo`, so `$PWD` is the run's own directory (inside the container for container runs).
pub const TENSORBOARD_LOGDIR_EXPORT: &str = "export TENSORBOARD_LOGDIR=\"$PWD/tensorboard\";";

pub fn staged_script(command: &str) -> String {
    format!("set -eo pipefail; cd repo; {command}")
}

/// Bash runs each line once it has read it, so a command that does not parse fails only after
/// the source arrived, and the provider's copy still finds the container running.
pub fn gated_script(archive_path: &str, command: &str) -> String {
    format!(
        "set -eo pipefail; mkdir -p \"$(dirname -- {archive})\"; while [ ! -f {ready} ]; do sleep 0.1; done; mkdir -p repo; tar -xf {archive} -C repo; cd repo\n{command}",
        archive = shell_quote(archive_path),
        ready = shell_quote(&format!("{archive_path}.ready")),
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub id: &'static str,
    pub label: &'static str,
    pub remote: bool,
    pub flavors: bool,
    pub requires_flavor: bool,
    pub source_transport: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub ready: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StagedSource(pub SourceSnapshot);

#[async_trait]
// async-trait marks its boxed futures `#[must_use]`; clippy 1.99 flags that generated code.
#[allow(clippy::double_must_use)]
pub trait ComputeBackend: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn preflight(&self, args: &crate::ExpRunArgs) -> Result<Preflight>;
    async fn stage_source(
        &self,
        project: &LocalProject,
        experiment: &LocalExperiment,
    ) -> Result<StagedSource>;
    /// The launch intent the run engine stores before any side effect. Secrets are added by
    /// the engine at launch.
    async fn prepare(
        &self,
        args: &crate::ExpRunArgs,
        source: &SourceSnapshot,
        run_id: &str,
        command: &str,
    ) -> Result<LaunchSpec>;
}

/// A finished Hugging Face run no longer needs the archive staged for its job volume.
pub fn remove_hf_staging(run: &StoredRun) -> Result<()> {
    let descriptor = BackendDescriptor::parse(&run.backend_json)?;
    if !matches!(descriptor.kind.as_str(), "hf_job" | "engine") {
        return Ok(());
    }
    if let (Some(path), Some(digest)) = (descriptor.source_path, descriptor.source_digest) {
        let staging = Path::new(&path)
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!("{digest}.hf"));
        if staging.is_dir() {
            std::fs::remove_dir_all(staging)?;
        }
    }
    Ok(())
}

async fn stage_snapshot(
    project: &LocalProject,
    experiment: &LocalExperiment,
    include_ray_package: bool,
) -> Result<StagedSource> {
    let project = project.clone();
    let experiment = experiment.clone();
    tokio::task::spawn_blocking(move || {
        SourceSnapshot::create(&project, &experiment, include_ray_package)
    })
    .await
    .map_err(|error| anyhow!("source snapshot task failed: {error}"))?
    .map(StagedSource)
}

fn ready() -> Result<Preflight> {
    Ok(Preflight {
        ready: true,
        detail: None,
    })
}

macro_rules! backend_adapter {
    (
        $name:ident, $id:literal, $label:literal, $remote:literal, $flavors:literal,
        $requires_flavor:literal, $transport:literal, $ray_package:literal,
        preflight |$preflight_args:ident| $preflight:expr,
        prepare |$prepare_args:ident, $source:ident, $run_id:ident, $command:ident| $prepare:expr
    ) => {
        pub struct $name;

        #[async_trait]
        impl ComputeBackend for $name {
            fn capabilities(&self) -> Capabilities {
                Capabilities {
                    id: $id,
                    label: $label,
                    remote: $remote,
                    flavors: $flavors,
                    requires_flavor: $requires_flavor,
                    source_transport: $transport,
                }
            }

            async fn preflight(&self, args: &crate::ExpRunArgs) -> Result<Preflight> {
                let $preflight_args = args;
                $preflight
            }

            async fn stage_source(
                &self,
                project: &LocalProject,
                experiment: &LocalExperiment,
            ) -> Result<StagedSource> {
                stage_snapshot(project, experiment, $ray_package).await
            }

            async fn prepare(
                &self,
                args: &crate::ExpRunArgs,
                source: &SourceSnapshot,
                run_id: &str,
                command: &str,
            ) -> Result<LaunchSpec> {
                let $prepare_args = args;
                let $source = source;
                let $run_id = run_id;
                let $command = command;
                $prepare
            }
        }
    };
}

backend_adapter!(
    LocalCompute,
    "local",
    "This machine",
    false,
    false,
    false,
    "local archive",
    false,
    preflight | _args | ready(),
    prepare | args,
    _source,
    _run_id,
    _command | crate::local::localrun::launch_spec(args, "local_job")
);

backend_adapter!(
    TinkerCompute,
    "tinker",
    "Tinker",
    false,
    false,
    false,
    "local controller, remote model compute",
    false,
    preflight | _args | {
        crate::jobs::tinker::resolve_api_key()?;
        ready()
    },
    prepare | args,
    _source,
    _run_id,
    _command | crate::local::localrun::launch_spec(args, "tinker_job")
);

backend_adapter!(
    HuggingFaceCompute,
    "hf",
    "Hugging Face Jobs",
    true,
    true,
    true,
    "private job volume",
    false,
    preflight | args | {
        if args.flavor.is_none() {
            return Ok(not_ready("Hugging Face Jobs requires --flavor."));
        }
        let token = crate::jobs::huggingface::resolve_token()?;
        crate::jobs::huggingface::whoami(&token).await?;
        ready()
    },
    prepare | args,
    source,
    run_id,
    command | crate::local::hf::launch_spec(args, source, run_id, command).await
);

backend_adapter!(
    ModalCompute,
    "modal",
    "Modal",
    true,
    true,
    true,
    "sandbox filesystem",
    false,
    preflight | args | {
        if args.flavor.is_none() {
            return Ok(not_ready("Modal requires --flavor."));
        }
        crate::jobs::modal::preflight(&args.caller_python_overrides).await?;
        ready()
    },
    prepare | args,
    source,
    run_id,
    command | crate::local::modal::launch_spec(args, source, run_id, command)
);

backend_adapter!(
    VercelCompute,
    "vercel",
    "Vercel Sandbox",
    true,
    true,
    false,
    "sandbox filesystem",
    false,
    preflight | args | {
        if let Err(error) = crate::jobs::vercel::resolve_flavor(args.flavor.as_deref()) {
            return Ok(not_ready(error.to_string()));
        }
        crate::jobs::vercel::preflight(&args.caller_python_overrides).await?;
        ready()
    },
    prepare | args,
    source,
    run_id,
    command | crate::local::vercel::launch_spec(args, source, run_id, command).await
);

backend_adapter!(
    KubernetesCompute,
    "k8s",
    "Kubernetes",
    true,
    false,
    false,
    "kubectl cp",
    false,
    preflight | _args | {
        let settings = crate::jobs::kubernetes::load_settings()?.unwrap_or_default();
        let check =
            crate::jobs::kubernetes::preflight(settings.context.as_deref(), &settings.namespace)
                .await;
        if !check.kubectl_found || !check.reachable || !check.can_create_jobs {
            return Ok(not_ready(check.error.as_deref().unwrap_or(
                "kubectl cannot reach the cluster or create Jobs in the namespace.",
            )));
        }
        if !check.can_patch_jobs {
            return Ok(not_ready(format!(
                "kubectl can create Jobs in namespace {} but not patch them, and orx patches a \
                 run's Job to start its timeout. Allow patching Jobs there, then run again.",
                settings.namespace
            )));
        }
        ready()
    },
    prepare | args,
    source,
    run_id,
    command | crate::local::k8s::launch_spec(args, source, run_id, command).await
);

backend_adapter!(
    SshCompute,
    "ssh",
    "SSH",
    true,
    false,
    false,
    "SSH tar stream",
    false,
    preflight | _args | ready(),
    prepare | args,
    _source,
    _run_id,
    command | crate::local::ssh::launch_spec(args, command)
);

backend_adapter!(
    SlurmCompute,
    "slurm",
    "Slurm",
    true,
    false,
    false,
    "SSH tar stream",
    false,
    preflight | _args | ready(),
    prepare | args,
    _source,
    _run_id,
    command | crate::local::slurm::launch_spec(args, command)
);

backend_adapter!(
    RayCompute,
    "ray",
    "Ray Jobs",
    true,
    false,
    false,
    "working_dir package",
    true,
    preflight | _args | {
        let address = crate::jobs::ray::resolve_address(None);
        crate::jobs::ray::preflight(&address).await?;
        ready()
    },
    prepare | args,
    source,
    run_id,
    command | crate::local::ray::launch_spec(args, source, run_id, command).await
);

backend_adapter!(
    OpenResearchCompute,
    "openresearch",
    "OpenResearch",
    true,
    true,
    true,
    "SSH tar stream",
    false,
    preflight | args | {
        let Some(flavor) = args.flavor.as_deref() else {
            return Ok(not_ready("OpenResearch requires --flavor."));
        };
        let Some(creds) = crate::config::load_credentials().await? else {
            return Ok(not_ready("OpenResearch requires `orx login`."));
        };
        // An unknown GPU id would only 400 after the run is recorded as failed.
        if let crate::client::SandboxTarget::New { gpu, .. } =
            crate::jobs::openresearch::parse_flavor(flavor, 0, None)?
        {
            if let Ok(Ok(catalog)) = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                crate::client::list_catalog(&creds),
            )
            .await
            {
                let mut ids: Vec<_> = catalog.offers.into_iter().map(|o| o.gpu).collect();
                ids.sort();
                ids.dedup();
                if !ids.is_empty() && !ids.contains(&gpu) {
                    return Ok(not_ready(format!(
                        "No OpenResearch offers for GPU '{gpu}'. Available now: {}. \
                         See `orx compute`.",
                        ids.join(", ")
                    )));
                }
            }
        }
        ready()
    },
    prepare | args,
    _source,
    _run_id,
    _command | crate::local::openresearch::launch_spec(args).await
);

pub fn backend(id: &str) -> Result<Box<dyn ComputeBackend>> {
    match id {
        "local" => Ok(Box::new(LocalCompute)),
        "tinker" => Ok(Box::new(TinkerCompute)),
        "hf" => Ok(Box::new(HuggingFaceCompute)),
        "modal" => Ok(Box::new(ModalCompute)),
        "k8s" => Ok(Box::new(KubernetesCompute)),
        "ssh" => Ok(Box::new(SshCompute)),
        "slurm" => Ok(Box::new(SlurmCompute)),
        "ray" => Ok(Box::new(RayCompute)),
        "vercel" => Ok(Box::new(VercelCompute)),
        "openresearch" => Ok(Box::new(OpenResearchCompute)),
        _ => Err(anyhow!("Unknown compute backend '{id}'.")),
    }
}

pub fn capabilities() -> Vec<Capabilities> {
    crate::local::BACKENDS
        .iter()
        .filter_map(|id| backend(id).ok())
        .map(|backend| backend.capabilities())
        .collect()
}

pub fn validate_run_args(args: &crate::ExpRunArgs) -> Result<()> {
    if let Some(timeout) = &args.timeout {
        match args.backend.as_deref().unwrap_or("local") {
            "local" | "ssh" => {
                return Err(anyhow!("--timeout does not apply to this backend."));
            }
            "ray" => {
                return Err(anyhow!("--timeout isn't supported on --backend ray."));
            }
            "tinker" => {}
            _ => {
                crate::jobs::huggingface::parse_timeout(timeout)?;
            }
        }
    }
    if (args.container.is_some() || args.no_container) && args.backend.as_deref() != Some("ssh") {
        return Err(anyhow!(
            "--container and --no-container only apply with --backend ssh."
        ));
    }
    if args.container.is_some() && args.no_container {
        return Err(anyhow!("--container conflicts with --no-container."));
    }
    if let Some(reference) = &args.container {
        crate::jobs::ssh::validate_container_reference(reference)?;
    }
    if args.backend.as_deref() == Some("ssh") && (args.image.is_some() || args.flavor.is_some()) {
        return Err(anyhow!("SSH does not support --flavor or --image."));
    }
    if args.manifest.is_some() && args.backend.as_deref() != Some("k8s") {
        return Err(anyhow!("--manifest only applies with --backend k8s."));
    }
    if args.host.is_some() && !matches!(args.backend.as_deref(), Some("ssh") | Some("slurm")) {
        return Err(anyhow!("--host only applies with --backend ssh or slurm."));
    }
    if (args.cpus.is_some() || args.mem.is_some()) && args.backend.as_deref() != Some("slurm") {
        return Err(anyhow!(
            "--cpus/--mem only apply with --backend slurm. Size other backends with --flavor."
        ));
    }
    if let Some(partition) = &args.partition {
        if args.backend.as_deref() != Some("slurm") {
            return Err(anyhow!("--partition only applies with --backend slurm."));
        }
        if !partition.split(',').all(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
        }) {
            return Err(anyhow!(
                "--partition expects names separated by commas (letters, digits, _, - or .)."
            ));
        }
    }
    let slurm_values = [
        ("--qos", &args.qos),
        ("--nodelist", &args.nodelist),
        ("--constraint", &args.constraint),
        ("--exclude", &args.exclude),
    ];
    for (flag, value) in slurm_values {
        let Some(value) = value else { continue };
        if args.backend.as_deref() != Some("slurm") {
            return Err(anyhow!("{flag} only applies with --backend slurm."));
        }
        crate::jobs::slurm::parse_value(flag, value)?;
    }
    if !args.sbatch_args.is_empty() && args.backend.as_deref() != Some("slurm") {
        return Err(anyhow!("--sbatch-arg only applies with --backend slurm."));
    }
    for arg in &args.sbatch_args {
        crate::jobs::slurm::parse_sbatch_arg(arg)?;
    }
    if args.cpus == Some(0) {
        return Err(anyhow!("--cpus must be at least 1."));
    }
    if let Some(mem) = &args.mem {
        crate::jobs::slurm::parse_mem(mem)?;
    }
    if args.org.is_some() && args.backend.as_deref() != Some("openresearch") {
        return Err(anyhow!("--org only applies with --backend openresearch."));
    }
    if args.disk.is_some() && args.backend.as_deref() != Some("openresearch") {
        return Err(anyhow!("--disk only applies with --backend openresearch."));
    }
    if args.provider.is_some() && args.backend.as_deref() != Some("openresearch") {
        return Err(anyhow!(
            "--provider only applies with --backend openresearch."
        ));
    }
    if args.backend.as_deref() == Some("tinker") {
        if args.flavor.is_some() {
            return Err(anyhow!("--backend tinker does not take --flavor."));
        }
        if args.image.is_some() {
            return Err(anyhow!("--image does not apply to --backend tinker."));
        }
        if args.timeout.is_some() {
            return Err(anyhow!("--timeout does not apply to --backend tinker."));
        }
    }
    if let Some(backend) = args.backend.as_deref() {
        if !crate::local::BACKENDS.contains(&backend) {
            return Err(anyhow!(
                "Unknown --backend '{}'. Local experiments support: hf (Hugging Face Jobs), \
                 modal (Modal serverless GPUs), k8s (your Kubernetes cluster), ssh (your own box), \
                 slurm (your Slurm cluster), ray (a Ray Jobs cluster), \
                 vercel (Vercel Sandbox microVMs), \
                 openresearch (an ephemeral OpenResearch box), tinker (local controller with remote model compute), \
                 local (this machine).",
                backend
            ));
        }
    }
    Ok(())
}

/// A run ready for the engine: its source snapshot and the launch intent stored with it.
pub struct Submission {
    pub run_id: String,
    pub experiment: LocalExperiment,
    pub source: SourceSnapshot,
    pub command: String,
    pub launch: LaunchSpec,
}

/// Snapshots the source and prepares the launch intent, then hands it to `insert` while the
/// snapshot is protected from cleanup and no other run of the experiment can start.
pub async fn submit<F, Fut>(args: &crate::ExpRunArgs, insert: F) -> Result<String>
where
    F: FnOnce(Submission) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    validate_run_args(args)?;
    let backend_id = args.backend.as_deref().unwrap_or("local");
    let backend = backend(backend_id)?;
    let store = Store::open()?;
    let preflight = backend.preflight(args).await?;
    if !preflight.ready {
        return Err(anyhow!(
            "{}",
            preflight
                .detail
                .filter(|detail| !detail.trim().is_empty())
                .unwrap_or_else(|| "Compute backend is not ready.".to_string())
        ));
    }
    let artifacts_lock = store.run_artifacts_lock()?;
    let deadline = std::time::Instant::now() + crate::store::RUN_ARTIFACTS_WAIT;
    let _artifacts_guard = loop {
        match artifacts_lock.try_read() {
            Ok(guard) => break guard,
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(crate::store::RUN_ARTIFACTS_POLL).await;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(anyhow!("Run artifacts are busy; retry the run submission."))
            }
            Err(error) => return Err(error.into()),
        }
    };
    let experiment = store
        .get_local_experiment(&args.exp_id)?
        .ok_or_else(|| anyhow!("Local experiment {} not found.", args.exp_id))?;
    let project = store
        .get_local_project(&experiment.project_id)?
        .ok_or_else(|| anyhow!("Local project {} not found.", experiment.project_id))?;
    let command = crate::local::experiments::effective_run_command(
        &experiment,
        project.run_command.as_deref(),
    )
    .ok_or_else(|| anyhow!("{}", crate::invocation::no_run_command(&project.id)))?
    .to_string();
    let source = backend.stage_source(&project, &experiment).await?.0;
    let run_id = uuid::Uuid::new_v4().to_string();
    let launch = backend.prepare(args, &source, &run_id, &command).await?;

    let chat_session = args.launching_chat_session();
    let chat_harness = match &chat_session {
        Some(session) => store.get_chat_session(session)?.map(|chat| chat.harness),
        None => None,
    };
    let identity = args.invocation_identity()?;
    let report = (!args.telemetry_suppressed && backend_id != "openresearch")
        .then(|| {
            crate::telemetry::pending_event_payload(
                "experiment_finished",
                run_attribution(
                    identity.as_ref(),
                    chat_harness,
                    args.agent_origin.as_deref(),
                ),
            )
        })
        .flatten();

    let dir = crate::store::data_dir().join("submission-locks");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir.join(&experiment.id))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write()?;
    if !args.force {
        if let Some(run) = store
            .list_runs_by_experiment(&experiment.id)?
            .into_iter()
            .find(|run| !crate::local::is_terminal(&run.status))
        {
            return Err(anyhow!(
                "Run {} is already in flight for this experiment ({}). Wait for it with \
                 `orx exp wait {exp}`; cancel it with `orx exp cancel {exp}` only if it \
                 should stop, or pass --force to launch anyway.",
                run.id,
                run.status,
                exp = experiment.id
            ));
        }
    }
    store.reserve_run_telemetry(&run_id, identity.as_ref(), report.as_ref())?;
    let branch = experiment.branch_name.clone();
    insert(Submission {
        run_id: run_id.clone(),
        experiment,
        source,
        command,
        launch,
    })
    .await?;
    if project.github_enabled() {
        if let Err(error) = crate::local::git::spawn_branch_publication(
            Path::new(&project.repo_path),
            &branch,
            &project.github_owner,
            &project.github_repo,
        ) {
            eprintln!(
                "GitHub sync could not start; compute is already running from the source snapshot: {error}"
            );
        }
    }
    Ok(run_id)
}

/// The run report's invoker: the forwarded native identity, else an explicit exception. A context
/// inherited from another agent around the app never stands in for the launching chat's own.
fn run_attribution(
    identity: Option<&crate::store::InvocationIdentity>,
    chat_harness: Option<String>,
    agent_origin: Option<&str>,
) -> serde_json::Value {
    let identity = identity.filter(|identity| {
        chat_harness
            .as_ref()
            .is_none_or(|harness| *harness == identity.harness)
    });
    let (harness, model, provider, attribution, reason) = match (identity, chat_harness) {
        (Some(identity), _) => (
            Some(identity.harness.clone()),
            Some(identity.model.clone()),
            identity.provider.clone(),
            "exact",
            None,
        ),
        (None, Some(harness)) => (
            Some(harness),
            None,
            None,
            "unresolved",
            Some("invoker_not_linked"),
        ),
        // The origin can arrive over HTTP: only the known marker labels reach telemetry.
        (None, None) => match agent_origin.filter(|origin| {
            ["claude-code", "codex", "opencode", "cursor", "unknown"].contains(origin)
        }) {
            Some(origin) => (
                Some(origin.to_string()).filter(|origin| origin != "unknown"),
                None,
                None,
                "unresolved",
                Some("external_agent"),
            ),
            None => (None, None, None, "manual", None),
        },
    };
    serde_json::json!({
        "harness": harness,
        "model": model,
        "provider": provider,
        "attribution": attribution,
        "attributionReason": reason,
        "status": "failed",
    })
}

/// Connect also logs in 0.2.x supervisors' SSH for the host's supervised runs the engine has
/// not adopted (`held`).
pub(crate) fn supervised_ssh_needed(
    runs: &[StoredRun],
    host: &str,
    held: impl Fn(&str) -> bool,
) -> bool {
    runs.iter().any(|run| {
        !crate::local::is_terminal(&run.status)
            && !held(&run.id)
            && BackendDescriptor::parse(&run.backend_json).is_ok_and(|descriptor| {
                matches!(descriptor.kind.as_str(), "ssh_job" | "slurm_job")
                    && descriptor.namespace.as_deref() == Some(host)
            })
    })
}

fn not_ready(detail: impl Into<String>) -> Preflight {
    Preflight {
        ready: false,
        detail: Some(detail.into()),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Providers copy the source into a container that is already running the script.
    #[cfg(unix)]
    #[test]
    fn a_command_that_does_not_parse_still_waits_for_its_source() {
        let root = std::env::temp_dir().join(format!("orx-gate-{}", uuid::Uuid::new_v4()));
        let archive = root.join("incoming/source.tar");
        std::fs::create_dir_all(root.join("work")).unwrap();
        let mut bash = crate::process::command("bash")
            .args(["-c", &gated_script(archive.to_str().unwrap(), "echo (")])
            .current_dir(root.join("work"))
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !archive.parent().unwrap().exists() {
            assert!(
                bash.try_wait().unwrap().is_none(),
                "bash exited before the source arrived"
            );
            assert!(std::time::Instant::now() < deadline, "the gate never ran");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(bash.try_wait().unwrap().is_none());

        let source = root.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let tar = crate::process::command("tar")
            .args([
                "-cf",
                archive.to_str().unwrap(),
                "-C",
                source.to_str().unwrap(),
                ".",
            ])
            .status()
            .unwrap();
        assert!(tar.success());
        std::fs::write(archive.with_extension("tar.ready"), "").unwrap();
        let output = bash.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(output.status.code(), Some(2));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("syntax error"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_dir_accepts_a_user_owned_symlinked_directory() {
        let root = std::env::temp_dir().join(format!("orx-snapshot-link-{}", uuid::Uuid::new_v4()));
        let target = root.join("target");
        let link = root.join("source-snapshots");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        prepare_snapshot_dir(&link).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn source_archives_keep_committed_lf_bytes_under_autocrlf() {
        let repo = std::env::temp_dir().join(format!("orx-archive-eol-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = crate::local::git::git_command()
                .current_dir(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        git(&["config", "core.autocrlf", "true"]);
        let script = "#!/usr/bin/env bash
set -euo pipefail
echo ok
";
        std::fs::write(repo.join("run.sh"), script).unwrap();
        git(&["add", "run.sh"]);
        git(&[
            "-c",
            "user.name=orx-test",
            "-c",
            "user.email=orx-test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "script",
        ]);
        let tar = repo.join("snapshot.tar");
        archive(&repo, "HEAD", "tar", &tar).unwrap();
        let bytes = std::fs::read(&tar).unwrap();
        assert!(bytes.windows(script.len()).any(|w| w == script.as_bytes()));
        let zip = repo.join("snapshot.zip");
        archive(&repo, "HEAD", "zip", &zip).unwrap();
        let mut entry = String::new();
        zip::ZipArchive::new(std::fs::File::open(&zip).unwrap())
            .unwrap()
            .by_name("run.sh")
            .unwrap()
            .read_to_string(&mut entry)
            .unwrap();
        assert_eq!(entry, script);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn supervisor_transport_is_needed_only_for_opt_out_or_unadopted_runs_on_the_alias() {
        let mut run = StoredRun {
            id: "run".into(),
            experiment_id: "exp".into(),
            project_id: "project".into(),
            status: "running".into(),
            backend_json: String::new(),
            command: String::new(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        };
        let free = |_: &str| false;
        assert!(!supervised_ssh_needed(&[], "lab", free));
        for kind in ["ssh_job", "slurm_job", "engine", "hf_job"] {
            run.backend_json =
                serde_json::json!({"kind": kind, "namespace": "lab", "jobId": "job"}).to_string();
            let runs = std::slice::from_ref(&run);
            let expected = matches!(kind, "ssh_job" | "slurm_job");
            assert_eq!(supervised_ssh_needed(runs, "lab", free), expected);
            assert!(!supervised_ssh_needed(runs, "other", free));
            assert!(!supervised_ssh_needed(runs, "lab", |id| id == "run"));
            run.status = "done".into();
            assert!(!supervised_ssh_needed(
                std::slice::from_ref(&run),
                "lab",
                free
            ));
            run.status = "running".into();
        }
    }

    #[test]
    fn run_reports_name_the_native_invoker_or_an_explicit_exception() {
        let identity = |harness: &str| crate::store::InvocationIdentity {
            harness: harness.into(),
            model: "m".into(),
            provider: None,
        };
        let row = |value: serde_json::Value| {
            (
                value["harness"].clone(),
                value["model"].clone(),
                value["attribution"].clone(),
                value["attributionReason"].clone(),
            )
        };
        let cases = [
            (
                Some(identity("claude-code")),
                Some("claude-code"),
                None,
                ("claude-code", "m", "exact", None),
            ),
            // A host Codex's context inside an OpenCode chat is not this chat's invoker.
            (
                Some(identity("codex")),
                Some("opencode"),
                Some("codex"),
                ("opencode", "", "unresolved", Some("invoker_not_linked")),
            ),
            (
                None,
                None,
                Some("cursor"),
                ("cursor", "", "unresolved", Some("external_agent")),
            ),
            (
                None,
                None,
                Some("unknown"),
                ("", "", "unresolved", Some("external_agent")),
            ),
            (None, None, None, ("", "", "manual", None)),
            (None, None, Some("<script>"), ("", "", "manual", None)),
        ];
        for (identity, chat, origin, (harness, model, attribution, reason)) in cases {
            let opt = |text: &str| {
                if text.is_empty() {
                    serde_json::json!(null)
                } else {
                    serde_json::json!(text)
                }
            };
            assert_eq!(
                row(run_attribution(
                    identity.as_ref(),
                    chat.map(str::to_string),
                    origin
                )),
                (
                    opt(harness),
                    opt(model),
                    serde_json::json!(attribution),
                    serde_json::json!(reason)
                )
            );
        }
    }

    pub(crate) fn tinker_args() -> crate::ExpRunArgs {
        crate::ExpRunArgs {
            exp_id: "exp".into(),
            disk: None,
            provider: None,
            backend: Some("tinker".into()),
            flavor: None,
            org: None,
            host: None,
            container: None,
            no_container: false,
            manifest: None,
            image: None,
            timeout: None,
            cpus: None,
            mem: None,
            partition: None,
            qos: None,
            nodelist: None,
            constraint: None,
            exclude: None,
            sbatch_args: Vec::new(),
            force: false,
            chat_session_id: None,
            invocation_context: None,
            agent_origin: None,
            forwarded: false,
            telemetry_suppressed: false,
            caller_config_dir: None,
            caller_python_overrides: Default::default(),
            caller_compute_settings: None,
        }
    }

    #[test]
    fn ssh_selectors_and_saved_target_precedence() {
        use crate::config::{SshHostSettings, SshSettings};
        let settings = SshSettings {
            default_host: Some("lab".into()),
            hosts: std::collections::BTreeMap::from([
                (
                    "lab".into(),
                    SshHostSettings {
                        container: Some("research".into()),
                        remote_root: Some("/scratch/lab/orx".into()),
                        keep_remote_runs: false,
                        ..Default::default()
                    },
                ),
                ("direct".into(), SshHostSettings::default()),
            ]),
            ..Default::default()
        };
        let mut args = tinker_args();
        args.backend = Some("ssh".into());
        for (host, container, no_container, expected_host, expected_container) in [
            (None, None, false, "lab", Some("research")),
            (
                Some("lab"),
                Some("research"),
                false,
                "lab",
                Some("research"),
            ),
            (None, Some("other"), false, "lab", Some("other")),
            (None, Some("none"), false, "lab", Some("none")),
            (None, None, true, "lab", None),
            (Some("direct"), None, true, "direct", None),
            (Some("unknown"), None, false, "unknown", None),
        ] {
            args.host = host.map(str::to_string);
            args.container = container.map(str::to_string);
            args.no_container = no_container;
            validate_run_args(&args).unwrap();
            let (host, options) = crate::jobs::ssh::resolve_options(&args, &settings).unwrap();
            assert_eq!(host, expected_host);
            assert_eq!(options.container.as_deref(), expected_container);
            assert_eq!(
                options.remote_root.is_some(),
                host == "lab",
                "the saved root follows the host, not the container"
            );
        }
        args.host = None;
        assert!(crate::jobs::ssh::resolve_options(&args, &SshSettings::default()).is_err());
        args.container = Some(String::new());
        assert!(validate_run_args(&args).is_err());
        args.container = Some("research".into());
        args.no_container = true;
        assert!(validate_run_args(&args).is_err());
        args.no_container = false;
        for backend in ["local", "slurm", "openresearch"] {
            args.backend = Some(backend.into());
            assert!(validate_run_args(&args).is_err());
        }
        use clap::Parser;
        assert!(crate::Cli::try_parse_from([
            "orx",
            "exp",
            "run",
            "exp",
            "--container",
            "research",
            "--no-container"
        ])
        .is_err());
    }

    #[test]
    fn tinker_is_registered_as_a_local_controller_without_flavors() {
        let capabilities = backend("tinker").unwrap().capabilities();
        assert_eq!(capabilities.id, "tinker");
        assert!(!capabilities.remote);
        assert!(!capabilities.flavors);
        assert_eq!(
            capabilities.source_transport,
            "local controller, remote model compute"
        );
    }

    #[test]
    fn tinker_rejects_provider_launch_options() {
        let mut args = tinker_args();
        assert!(validate_run_args(&args).is_ok());
        args.flavor = Some("gpu".into());
        assert!(validate_run_args(&args).is_err());
        args.flavor = None;
        args.image = Some("python:3.11".into());
        assert!(validate_run_args(&args).is_err());
        args.image = None;
        args.timeout = Some("1h".into());
        assert!(validate_run_args(&args).is_err());
    }

    #[test]
    fn slurm_resources_are_rejected_on_every_other_backend() {
        let mut args = tinker_args();
        for backend in crate::local::BACKENDS
            .iter()
            .filter(|backend| **backend != "slurm")
        {
            args.backend = Some(backend.to_string());
            for (cpus, mem) in [(Some(8), None), (None, Some("64G"))] {
                args.cpus = cpus;
                args.mem = mem.map(str::to_string);
                assert_eq!(
                    validate_run_args(&args).unwrap_err().to_string(),
                    "--cpus/--mem only apply with --backend slurm. Size other backends with --flavor.",
                    "{backend}"
                );
            }
        }
        args.backend = Some("slurm".into());
        args.cpus = Some(8);
        args.mem = Some("64G".into());
        assert!(validate_run_args(&args).is_ok());
        args.mem = Some("64GB".into());
        assert!(validate_run_args(&args).is_err());
        args.mem = None;
        args.cpus = Some(0);
        assert!(validate_run_args(&args).is_err());
    }

    #[test]
    fn invalid_timeout_is_rejected_before_submission() {
        let mut args = tinker_args();
        for backend in [
            None,
            Some("local"),
            Some("ssh"),
            Some("ray"),
            Some("tinker"),
        ] {
            args.backend = backend.map(str::to_string);
            args.timeout = Some("1h".into());
            assert!(validate_run_args(&args).is_err(), "{backend:?}");
        }
        args.backend = Some("hf".into());
        for timeout in ["0s", "18446744073709551615d", "bad"] {
            args.timeout = Some(timeout.into());
            assert!(validate_run_args(&args).is_err(), "{timeout}");
        }
        args.timeout = Some("1h".into());
        assert!(validate_run_args(&args).is_ok());
    }
}
