//! Backend-agnostic compute lifecycle and immutable source snapshots.
//!
//! Local Git remains the experiment-history database. A launch never asks a
//! remote backend to clone that history: it archives the exact recorded commit
//! once, addresses the archive by SHA-256, and hands that immutable payload to
//! the selected provider adapter.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

use crate::error::{anyhow, Result};
use crate::jobs::BackendDescriptor;
use crate::local::model::{LocalExperiment, LocalProject};
use crate::store::{log_path, Store, StoredRun};

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
    let output = Command::new("git")
        .current_dir(repo)
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
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn prepare_snapshot_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = std::fs::symlink_metadata(path)?;
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

fn digest_file(path: &Path) -> Result<(String, u64)> {
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
    Ok(())
}

pub fn snapshot_script(archive_path: &str, command: &str) -> String {
    format!(
        "set -eo pipefail; mkdir -p repo; tar -xf {} -C repo; cd repo; {}",
        shell_quote(archive_path),
        command
    )
}

pub fn staged_script(command: &str) -> String {
    format!("set -eo pipefail; cd repo; {command}")
}

pub fn gated_script(archive_path: &str, command: &str) -> String {
    format!(
        "set -eo pipefail; mkdir -p \"$(dirname -- {archive})\"; while [ ! -f {ready} ]; do sleep 0.1; done; mkdir -p repo; tar -xf {archive} -C repo; cd repo; {command}",
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

#[derive(Debug, Clone, Copy, Default)]
pub struct LogCursor(pub u64);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogBatch {
    pub data_base64: String,
    pub next_cursor: u64,
    pub eof: bool,
}

#[async_trait]
pub trait ComputeBackend: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn preflight(&self, args: &crate::ExpRunArgs) -> Result<Preflight>;
    async fn stage_source(
        &self,
        project: &LocalProject,
        experiment: &LocalExperiment,
    ) -> Result<StagedSource>;
    async fn submit(
        &self,
        args: &crate::ExpRunArgs,
        source: StagedSource,
        run_id: String,
    ) -> Result<StoredRun>;

    async fn status(&self, handle: &StoredRun) -> Result<StoredRun> {
        Store::open()?
            .get_run(&handle.id)?
            .ok_or_else(|| anyhow!("Run {} not found.", handle.id))
    }

    async fn logs(&self, handle: &StoredRun, cursor: LogCursor) -> Result<LogBatch> {
        use base64::Engine as _;
        use std::io::{Read as _, Seek as _};
        let path = log_path(&handle.id);
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => {
                return Ok(LogBatch {
                    data_base64: String::new(),
                    next_cursor: cursor.0,
                    eof: crate::local::is_terminal(&handle.status),
                })
            }
        };
        let len = file.metadata()?.len();
        let start = cursor.0.min(len);
        file.seek(std::io::SeekFrom::Start(start))?;
        let mut data = Vec::new();
        file.take(256 * 1024).read_to_end(&mut data)?;
        let next = start + data.len() as u64;
        Ok(LogBatch {
            data_base64: base64::engine::general_purpose::STANDARD.encode(data),
            next_cursor: next,
            eof: crate::local::is_terminal(&handle.status) && next >= len,
        })
    }

    async fn cancel(&self, handle: &StoredRun) -> Result<()> {
        // Agent-session callers route through orx up before reaching this trusted path.
        crate::commands::exp::request_local_run_cancel(&Store::open()?, &handle.id)
    }

    async fn cleanup(&self, handle: &StoredRun) -> Result<()> {
        if self.capabilities().id == "hf" && crate::local::is_terminal(&handle.status) {
            let descriptor = BackendDescriptor::parse(&handle.backend_json)?;
            if let (Some(path), Some(digest)) = (descriptor.source_path, descriptor.source_digest) {
                let staging = Path::new(&path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(format!("{digest}.hf"));
                if staging.is_dir() {
                    std::fs::remove_dir_all(staging)?;
                }
            }
        }
        Ok(())
    }
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
        submit |$submit_args:ident, $source:ident, $run_id:ident| $submit:expr
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

            async fn submit(
                &self,
                args: &crate::ExpRunArgs,
                staged: StagedSource,
                id: String,
            ) -> Result<StoredRun> {
                let $submit_args = args;
                let $source = staged.0;
                let $run_id = id;
                $submit
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
    submit | args,
    source,
    run_id | crate::local::localrun::submit_local_run_with_source(args, source, run_id).await
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
    submit | args,
    source,
    run_id | crate::local::localrun::submit_tinker_run_with_source(args, source, run_id).await
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
    submit | args,
    source,
    run_id | crate::local::hf::submit_local_hf_with_source(args, source, run_id).await
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
        crate::jobs::modal::preflight().await?;
        ready()
    },
    submit | args,
    source,
    run_id | crate::local::modal::submit_local_modal_with_source(args, source, run_id).await
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
        ready()
    },
    submit | args,
    source,
    run_id | crate::local::k8s::submit_local_k8s_with_source(args, source, run_id).await
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
    preflight | args | {
        let host = args
            .host
            .as_deref()
            .ok_or_else(|| anyhow!("SSH requires --host <alias>."))?;
        let check = crate::jobs::ssh::preflight(&crate::jobs::ssh::SshTarget::alias(host)).await;
        if !check.reachable || !check.tools_found {
            return Ok(not_ready(
                check
                    .error
                    .as_deref()
                    .unwrap_or("The SSH host needs bash and tar."),
            ));
        }
        ready()
    },
    submit | args,
    source,
    run_id | crate::local::ssh::submit_local_ssh_with_source(args, source, run_id).await
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
    preflight | args | {
        let settings = crate::jobs::slurm::load_settings()?.unwrap_or_default();
        let host = args
            .host
            .as_deref()
            .or(settings.host.as_deref())
            .ok_or_else(|| anyhow!("Slurm requires --host or a configured host."))?;
        let check = crate::jobs::slurm::preflight(host).await;
        if !check.reachable || !check.slurm_found || !check.tools_found {
            return Ok(not_ready(check.error.as_deref().unwrap_or(
                "The Slurm host needs bash, tar, sbatch, squeue, and scancel.",
            )));
        }
        ready()
    },
    submit | args,
    source,
    run_id | crate::local::slurm::submit_local_slurm_with_source(args, source, run_id).await
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
    submit | args,
    source,
    run_id | crate::local::ray::submit_local_ray_with_source(args, source, run_id).await
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
        if args.flavor.is_none() {
            return Ok(not_ready("OpenResearch requires --flavor."));
        }
        if crate::config::load_credentials().await?.is_none() {
            return Ok(not_ready("OpenResearch requires `orx login`."));
        }
        ready()
    },
    submit | args,
    source,
    run_id
        | crate::local::openresearch::submit_local_openresearch_with_source(args, source, run_id)
            .await
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
    if args.manifest.is_some() && args.backend.as_deref() != Some("k8s") {
        return Err(anyhow!("--manifest only applies with --backend k8s."));
    }
    if args.host.is_some() && !matches!(args.backend.as_deref(), Some("ssh") | Some("slurm")) {
        return Err(anyhow!("--host only applies with --backend ssh or slurm."));
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
                 openresearch (an ephemeral OpenResearch box), tinker (local controller with remote model compute), \
                 local (this machine).",
                backend
            ));
        }
    }
    Ok(())
}

pub async fn submit(args: &crate::ExpRunArgs) -> Result<StoredRun> {
    let backend_id = args.backend.as_deref().unwrap_or("local");
    let backend = backend(backend_id)?;
    let store = Store::open()?;
    let experiment = store
        .get_local_experiment(&args.exp_id)?
        .ok_or_else(|| anyhow!("Local experiment {} not found.", args.exp_id))?;
    let project = store
        .get_local_project(&experiment.project_id)?
        .ok_or_else(|| anyhow!("Local project {} not found.", experiment.project_id))?;
    let preflight = backend.preflight(args).await?;
    if !preflight.ready {
        return Err(anyhow!(
            "{}",
            preflight
                .detail
                .unwrap_or_else(|| "Compute backend is not ready.".to_string())
        ));
    }
    let source = backend.stage_source(&project, &experiment).await?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let command = Some(experiment.run_command.clone())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            project
                .run_command
                .clone()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_default();
    let mut descriptor = BackendDescriptor {
        kind: format!("{}_job", backend_id),
        namespace: None,
        job_id: None,
        flavor: args.flavor.clone(),
        image: args.image.clone(),
        url: None,
        context: None,
        manifest: args.manifest.clone(),
        resources: None,
        ssh_host: None,
        ssh_port: None,
        ssh_user: None,
        timeout_secs: None,
        source_digest: None,
        source_path: None,
        source_size: None,
    };
    source.0.apply_to_descriptor(&mut descriptor);
    let now = crate::store::now_ms();
    let pending = StoredRun {
        id: run_id.clone(),
        experiment_id: experiment.id.clone(),
        project_id: project.id.clone(),
        status: "starting".to_string(),
        backend_json: descriptor.to_json(),
        command,
        created_at: now,
        updated_at: now,
        ended_at: None,
        exit_code: None,
        commit_sha: Some(source.0.revision.clone()),
        result_markdown: None,
        cancel_requested: false,
        chat_session_id: args.launching_chat_session(),
    };
    reserve_run(&store, &pending, args.force)?;
    let pending_backend_json = descriptor.to_json();
    match backend.submit(args, source, run_id.clone()).await {
        Ok(run) => {
            if project.github_enabled() {
                if let Err(error) = crate::local::git::spawn_branch_publication(
                    Path::new(&project.repo_path),
                    &experiment.branch_name,
                    &project.github_owner,
                    &project.github_repo,
                ) {
                    eprintln!(
                        "GitHub sync could not start; compute is already running from the source snapshot: {error}"
                    );
                }
            }
            Ok(run)
        }
        Err(error) => {
            let current = store.get_run(&run_id)?;
            let handle_was_persisted = current
                .as_ref()
                .is_some_and(|run| run.backend_json != pending_backend_json);
            if !handle_was_persisted {
                store.update_status(&run_id, "failed", Some(crate::store::now_ms()), None)?;
                store
                    .set_result_markdown(&run_id, &format!("Compute submission failed: {error}"))?;
            }
            Err(error)
        }
    }
}

fn reserve_run(store: &Store, pending: &StoredRun, force: bool) -> Result<()> {
    let dir = crate::store::data_dir().join("submission-locks");
    std::fs::create_dir_all(&dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir.join(&pending.experiment_id))?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = lock.write()?;
    if !force {
        if let Some(run) = store
            .list_runs_by_experiment(&pending.experiment_id)?
            .into_iter()
            .find(|run| !crate::local::is_terminal(&run.status))
        {
            return Err(anyhow!(
                "Run {} is already in flight for this experiment ({}). Cancel it with \
                 `orx exp cancel {}` or pass --force to launch anyway.",
                run.id,
                run.status,
                pending.experiment_id
            ));
        }
    }
    store.upsert_run(pending)
}

pub fn record_submission_handle(run_id: &str, descriptor: &BackendDescriptor) -> Result<()> {
    let database_error = Store::open()
        .and_then(|store| store.set_backend_json(run_id, &descriptor.to_json()))
        .err();
    let dir = crate::store::data_dir().join("submission-handles");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        return match database_error {
            None => {
                eprintln!("warning: could not create submission recovery directory: {error}");
                Ok(())
            }
            Some(database_error) => Err(anyhow!(
                "Could not persist provider handle in SQLite ({database_error}) or the recovery directory ({error})."
            )),
        };
    }
    let destination = dir.join(format!("{run_id}.json"));
    let temporary = dir.join(format!(".{run_id}.{}.tmp", uuid::Uuid::new_v4()));
    let file_error = std::fs::write(&temporary, descriptor.to_json())
        .and_then(|()| std::fs::rename(&temporary, destination))
        .err();
    if let Some(error) = file_error {
        let _ = std::fs::remove_file(temporary);
        if let Some(database_error) = database_error {
            return Err(anyhow!(
                "Could not persist provider handle in SQLite ({database_error}) or its recovery file ({error})."
            ));
        }
        eprintln!("warning: could not write redundant submission recovery record: {error}");
    }
    Ok(())
}

pub fn recover_submission_handle(run_id: &str) -> Result<Option<BackendDescriptor>> {
    let path = crate::store::data_dir()
        .join("submission-handles")
        .join(format!("{run_id}.json"));
    match std::fs::read_to_string(path) {
        Ok(json) => BackendDescriptor::parse(&json).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn not_ready(detail: impl Into<String>) -> Preflight {
    Preflight {
        ready: false,
        detail: Some(detail.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tinker_args() -> crate::ExpRunArgs {
        crate::ExpRunArgs {
            exp_id: "exp".into(),
            disk: None,
            provider: None,
            backend: Some("tinker".into()),
            flavor: None,
            org: None,
            host: None,
            manifest: None,
            image: None,
            timeout: None,
            force: false,
            chat_session_id: None,
        }
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
}
