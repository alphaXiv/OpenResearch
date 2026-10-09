//! Google Cloud Compute Engine — a GPU VM created for one run and deleted when
//! it ends.
//!
//! Everything goes through the user's own `gcloud` CLI and its signed-in
//! account, so orx never handles Google credentials. Submit creates the VM; the
//! detached supervisor waits for it, launches the run over plain SSH (the same
//! transport as the ssh and openresearch backends), watches it, and deletes the
//! VM at terminal state. Two nets catch a VM the supervisor could not delete:
//! Compute Engine itself deletes it once `--max-run-duration` passes, and
//! `orx up` deletes VMs whose run has already ended ([`reap_orphans`]).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::process::Command;

use crate::error::{anyhow, Result};

/// Label every orx VM carries; its value is the run id (lowercased, which run
/// ids already are).
pub const RUN_LABEL: &str = "orx-run";
/// The login orx installs its SSH key for on the VM.
pub const SSH_USER: &str = "orx";
/// A fresh VM boots and installs the NVIDIA driver before sshd is useful.
pub const PROVISION_DEADLINE: Duration = Duration::from_secs(15 * 60);
/// How long Compute Engine keeps a VM past the run's own timeout before
/// deleting it regardless of orx.
const MAX_RUN_GRACE_SECS: u64 = 3600;
const DEFAULT_ZONE: &str = "us-central1-a";
const DEFAULT_IMAGE_FAMILY: &str = "common-cu128-ubuntu-2204-nvidia-570";
const DEFAULT_IMAGE_PROJECT: &str = "deeplearning-platform-release";
const DEFAULT_DISK_GB: u64 = 200;
const GCLOUD_TIMEOUT: Duration = Duration::from_secs(180);

pub const INSTALL_HINT: &str =
    "Install the Google Cloud CLI (https://cloud.google.com/sdk/docs/install), then run \
     `gcloud auth login`.";

// --- settings -----------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GcpSettings {
    /// Project that owns and pays for the VMs; gcloud's own default otherwise.
    #[serde(default)]
    pub project: Option<String>,
    /// Zone to create VMs in; gcloud's `compute/zone`, then us-central1-a.
    #[serde(default)]
    pub zone: Option<String>,
    /// Spot VMs cost far less but Google can reclaim them mid-run.
    #[serde(default)]
    pub spot: bool,
    #[serde(default)]
    pub image_family: Option<String>,
    #[serde(default)]
    pub image_project: Option<String>,
    #[serde(default)]
    pub disk_gb: Option<u64>,
    /// Credit the user had left, in USD, copied from Billing → Credits in the
    /// Google Cloud console (gcloud cannot read it).
    #[serde(default)]
    pub credit_usd: Option<f64>,
    /// When `credit_usd` was entered (Unix ms); spend is counted from here.
    #[serde(default)]
    pub credit_as_of: Option<i64>,
    /// BigQuery billing export table (`project.dataset.table`) for exact spend.
    #[serde(default)]
    pub billing_export_table: Option<String>,
}

fn settings_path() -> PathBuf {
    crate::config::config_dir().join("gcp.json")
}

pub fn load_settings() -> Result<Option<GcpSettings>> {
    let raw = match std::fs::read_to_string(settings_path()) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    serde_json::from_str(&raw).map(Some).map_err(|e| {
        anyhow!(
            "Unreadable {} ({}). Fix or delete it and reconfigure.",
            settings_path().display(),
            e
        )
    })
}

pub fn save_settings(settings: &GcpSettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(settings)?),
    )?;
    Ok(())
}

// --- gcloud ---------------------------------------------------------------------

pub fn find_cli() -> Option<PathBuf> {
    crate::local::shell_env::find_on_path("gcloud").or_else(|| {
        dirs::home_dir().and_then(|home| {
            crate::local::shell_env::find_in_dir(&home.join("google-cloud-sdk/bin"), "gcloud")
        })
    })
}

/// Run gcloud non-interactively and return its stdout.
pub(super) async fn gcloud(args: &[String]) -> Result<String> {
    let cli = find_cli().ok_or_else(|| anyhow!("{INSTALL_HINT}"))?;
    let mut command = Command::new(cli);
    command
        .args(args)
        .arg("--quiet")
        .env("CLOUDSDK_CORE_DISABLE_PROMPTS", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(GCLOUD_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            anyhow!(
                "gcloud did not finish within {}s.",
                GCLOUD_TIMEOUT.as_secs()
            )
        })?
        .map_err(|e| anyhow!("Could not run gcloud: {e}"))?;
    if !output.status.success() {
        return Err(anyhow!(
            "{}",
            gcloud_error(&String::from_utf8_lossy(&output.stderr))
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The useful line of a gcloud failure, reworded where the fix is known.
fn gcloud_error(stderr: &str) -> String {
    let text = stderr.trim();
    if text.contains("ZONE_RESOURCE_POOL_EXHAUSTED")
        || text.contains("does not have enough resources")
    {
        return "This zone has no free capacity for that GPU right now. Try another zone, a \
                different GPU, or another provider."
            .into();
    }
    if text.contains("Quota") && text.contains("exceeded") {
        let quota = text
            .lines()
            .find(|line| line.contains("Quota"))
            .unwrap_or(text)
            .trim();
        return format!(
            "{quota} Request more GPU quota under IAM & Admin → Quotas in the Google Cloud \
             console, or pick another GPU or zone."
        );
    }
    if text.contains("gcloud auth login")
        || text.contains("You do not currently have an active account")
    {
        return "gcloud is not signed in. Run `gcloud auth login`.".into();
    }
    if text.contains("compute.googleapis.com") && text.contains("not been used") {
        return "The Compute Engine API is off for this project. Enable it in the Google Cloud \
                console (APIs & Services) or with `gcloud services enable compute.googleapis.com`."
            .into();
    }
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim_start_matches("ERROR: ").trim())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The project in gcloud's active configuration, read from disk without
/// running gcloud (for the cheap compute summary).
pub fn configured_project_on_disk() -> Option<String> {
    let dir = std::env::var_os("CLOUDSDK_CONFIG")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config/gcloud")))?;
    let active = std::fs::read_to_string(dir.join("active_config"))
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|_| "default".into());
    let raw = std::fs::read_to_string(dir.join("configurations").join(format!("config_{active}")))
        .ok()?;
    let mut in_core = false;
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_core = line == "[core]";
        } else if in_core {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "project" && !value.trim().is_empty() {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

async fn config_value(key: &str) -> Option<String> {
    gcloud(&["config".into(), "get-value".into(), key.into()])
        .await
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && value != "(unset)")
}

/// Signed-in account, from gcloud.
pub async fn account() -> Option<String> {
    config_value("account").await
}

/// The project and zone runs use: saved settings, then gcloud's defaults.
pub async fn resolve_location(settings: &GcpSettings) -> (Option<String>, String) {
    let project = match &settings.project {
        Some(project) => Some(project.clone()),
        None => config_value("project").await,
    };
    let zone = match &settings.zone {
        Some(zone) => zone.clone(),
        None => config_value("compute/zone")
            .await
            .unwrap_or_else(|| DEFAULT_ZONE.to_string()),
    };
    (project, zone)
}

// --- flavors ---------------------------------------------------------------------

/// One GPU Compute Engine offers, and how a VM gets it.
pub struct GpuKind {
    pub id: &'static str,
    pub label: &'static str,
    pub vram_gb: f64,
    /// Accelerator type for N1 VMs; `None` for machine families with the GPU
    /// built in (G2, A2, A3).
    accelerator: Option<&'static str>,
    /// Machine-type prefix for built-in families, completed by the count.
    family: Option<&'static str>,
    counts: &'static [u32],
    /// On-demand USD per hour for one GPU with its smallest machine in
    /// us-central1 (published list prices; other regions differ).
    pub usd_per_hour: f64,
}

pub const GPUS: &[GpuKind] = &[
    GpuKind {
        id: "t4",
        label: "NVIDIA T4",
        vram_gb: 16.0,
        accelerator: Some("nvidia-tesla-t4"),
        family: None,
        counts: &[1, 2, 4],
        usd_per_hour: 0.73,
    },
    GpuKind {
        id: "p4",
        label: "NVIDIA P4",
        vram_gb: 8.0,
        accelerator: Some("nvidia-tesla-p4"),
        family: None,
        counts: &[1, 2, 4],
        usd_per_hour: 0.98,
    },
    GpuKind {
        id: "l4",
        label: "NVIDIA L4",
        vram_gb: 24.0,
        accelerator: None,
        family: Some("g2-standard"),
        counts: &[1, 2, 4, 8],
        usd_per_hour: 0.85,
    },
    GpuKind {
        id: "p100",
        label: "NVIDIA P100",
        vram_gb: 16.0,
        accelerator: Some("nvidia-tesla-p100"),
        family: None,
        counts: &[1, 2, 4],
        usd_per_hour: 1.84,
    },
    GpuKind {
        id: "v100",
        label: "NVIDIA V100",
        vram_gb: 16.0,
        accelerator: Some("nvidia-tesla-v100"),
        family: None,
        counts: &[1, 2, 4, 8],
        usd_per_hour: 2.86,
    },
    GpuKind {
        id: "a100",
        label: "NVIDIA A100 40GB",
        vram_gb: 40.0,
        accelerator: None,
        family: Some("a2-highgpu"),
        counts: &[1, 2, 4, 8],
        usd_per_hour: 3.67,
    },
    GpuKind {
        id: "a100-80gb",
        label: "NVIDIA A100 80GB",
        vram_gb: 80.0,
        accelerator: None,
        family: Some("a2-ultragpu"),
        counts: &[1, 2, 4, 8],
        usd_per_hour: 5.07,
    },
    GpuKind {
        id: "h100",
        label: "NVIDIA H100 80GB",
        vram_gb: 80.0,
        accelerator: None,
        family: Some("a3-highgpu"),
        counts: &[1, 2, 4, 8],
        usd_per_hour: 11.06,
    },
];

/// The machine a `--flavor` asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineShape {
    pub machine_type: String,
    /// `(accelerator type, count)` for N1 VMs.
    pub accelerator: Option<(String, u32)>,
    /// GPU id from [`GPUS`], `None` for CPU-only.
    pub gpu: Option<&'static str>,
    pub gpu_count: u32,
}

/// `t4`, `a100:2`, `h100:8`, `cpu`, or a CPU machine type like `n2-standard-16`.
pub fn parse_flavor(flavor: &str) -> Result<MachineShape> {
    let flavor = flavor.trim().to_ascii_lowercase();
    let (base, count) = match flavor.split_once(':') {
        Some((base, count)) => (
            base.to_string(),
            count
                .parse::<u32>()
                .ok()
                .filter(|count| *count >= 1)
                .ok_or_else(|| {
                    anyhow!("Bad --flavor '{flavor}': the ':N' suffix must be a GPU count.")
                })?,
        ),
        None => (flavor.clone(), 1),
    };
    let base = match base.as_str() {
        "a100-40gb" => "a100",
        "h100-80gb" => "h100",
        other => other,
    };
    if base == "cpu" {
        return Ok(MachineShape {
            machine_type: "e2-standard-8".into(),
            accelerator: None,
            gpu: None,
            gpu_count: 0,
        });
    }
    if let Some(gpu) = GPUS.iter().find(|gpu| gpu.id == base) {
        if !gpu.counts.contains(&count) {
            return Err(anyhow!(
                "Compute Engine offers {} in counts of {}.",
                gpu.label,
                gpu.counts
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        return Ok(match (gpu.family, gpu.accelerator) {
            (Some(family), _) => MachineShape {
                machine_type: built_in_machine_type(family, count),
                accelerator: None,
                gpu: Some(gpu.id),
                gpu_count: count,
            },
            (None, Some(accelerator)) => MachineShape {
                // 8 vCPUs per GPU keeps data loading from starving it.
                machine_type: format!("n1-standard-{}", (8 * count).min(96)),
                accelerator: Some((accelerator.to_string(), count)),
                gpu: Some(gpu.id),
                gpu_count: count,
            },
            (None, None) => unreachable!("every GPU has a family or an accelerator"),
        });
    }
    let looks_like_machine_type = base.split('-').count() >= 2
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && count == 1;
    if looks_like_machine_type {
        return Ok(MachineShape {
            machine_type: base.to_string(),
            accelerator: None,
            gpu: None,
            gpu_count: 0,
        });
    }
    Err(anyhow!(
        "Unknown --flavor '{flavor}' for --backend gcp. Use a GPU ({}), optionally with \
         ':count', `cpu`, or a CPU machine type like n2-standard-16.",
        GPUS.iter().map(|gpu| gpu.id).collect::<Vec<_>>().join(", ")
    ))
}

fn built_in_machine_type(family: &str, count: u32) -> String {
    if family == "g2-standard" {
        let vcpus = match count {
            1 => 8,
            2 => 24,
            4 => 48,
            _ => 96,
        };
        return format!("g2-standard-{vcpus}");
    }
    format!("{family}-{count}g")
}

// --- VMs -------------------------------------------------------------------------

/// Compute Engine names: lowercase letters, digits, and hyphens, ≤ 63 chars.
pub fn instance_name(run_id: &str) -> String {
    let id: String = run_id
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(36)
        .collect();
    format!("orx-{}", id.trim_end_matches('-'))
}

fn label_value(run_id: &str) -> String {
    run_id
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(63)
        .collect()
}

pub struct CreateSpec<'a> {
    pub project: &'a str,
    pub zone: &'a str,
    pub name: &'a str,
    pub run_id: &'a str,
    pub shape: &'a MachineShape,
    pub settings: &'a GcpSettings,
    pub ssh_public_key: &'a str,
    pub timeout_secs: u64,
}

pub fn create_args(spec: &CreateSpec) -> Vec<String> {
    let settings = spec.settings;
    let mut args: Vec<String> = vec![
        "compute".into(),
        "instances".into(),
        "create".into(),
        spec.name.into(),
        format!("--project={}", spec.project),
        format!("--zone={}", spec.zone),
        format!("--machine-type={}", spec.shape.machine_type),
        format!(
            "--image-family={}",
            settings
                .image_family
                .as_deref()
                .unwrap_or(DEFAULT_IMAGE_FAMILY)
        ),
        format!(
            "--image-project={}",
            settings
                .image_project
                .as_deref()
                .unwrap_or(DEFAULT_IMAGE_PROJECT)
        ),
        format!(
            "--boot-disk-size={}GB",
            settings.disk_gb.unwrap_or(DEFAULT_DISK_GB)
        ),
        "--boot-disk-type=pd-balanced".into(),
        format!("--labels={RUN_LABEL}={}", label_value(spec.run_id)),
        format!(
            "--metadata=install-nvidia-driver=True,enable-oslogin=FALSE,ssh-keys={SSH_USER}:{}",
            metadata_key(spec.ssh_public_key)
        ),
        // Compute Engine deletes the VM itself if orx never does.
        format!(
            "--max-run-duration={}s",
            spec.timeout_secs + MAX_RUN_GRACE_SECS
        ),
        "--instance-termination-action=DELETE".into(),
        "--format=json".into(),
    ];
    if let Some((accelerator, count)) = &spec.shape.accelerator {
        args.push(format!("--accelerator=type={accelerator},count={count}"));
    }
    if spec.shape.gpu.is_some() {
        // GPU VMs cannot live-migrate.
        args.push("--maintenance-policy=TERMINATE".into());
    }
    if settings.spot {
        args.push("--provisioning-model=SPOT".into());
    }
    args
}

/// `type base64 orx`: the key's own comment is dropped, since a comma in it
/// would split gcloud's `--metadata` list.
fn metadata_key(public_key: &str) -> String {
    let mut parts = public_key.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some(kind), Some(key)) => format!("{kind} {key} {SSH_USER}"),
        _ => public_key.trim().to_string(),
    }
}

pub async fn create(spec: &CreateSpec<'_>) -> Result<()> {
    gcloud(&create_args(spec)).await.map(|_| ())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub name: String,
    pub zone: String,
    pub status: String,
    pub machine_type: String,
    pub external_ip: Option<String>,
    /// The run it was created for, from its label.
    pub run_id: Option<String>,
    pub gpu: Option<String>,
    pub gpu_count: u32,
}

fn last_segment(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn parse_instance(value: &Value) -> Option<Instance> {
    let machine_type = last_segment(&value["machineType"]);
    let accelerator = value["guestAccelerators"]
        .as_array()
        .and_then(|a| a.first());
    let (gpu, gpu_count) = match accelerator {
        Some(accelerator) => (
            Some(last_segment(&accelerator["acceleratorType"])),
            accelerator["acceleratorCount"].as_u64().unwrap_or(1) as u32,
        ),
        None => GPUS
            .iter()
            .filter_map(|gpu| gpu.family.map(|family| (gpu, family)))
            .find(|(_, family)| machine_type.starts_with(family))
            .map(|(gpu, _)| {
                let count = machine_type
                    .rsplit('-')
                    .next()
                    .and_then(|tail| tail.strip_suffix('g'))
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                (Some(gpu.label.to_string()), count)
            })
            .unwrap_or((None, 0)),
    };
    Some(Instance {
        name: value["name"].as_str()?.to_string(),
        zone: last_segment(&value["zone"]),
        status: value["status"].as_str().unwrap_or("UNKNOWN").to_string(),
        external_ip: value["networkInterfaces"][0]["accessConfigs"][0]["natIP"]
            .as_str()
            .map(str::to_string),
        run_id: value["labels"][RUN_LABEL].as_str().map(str::to_string),
        machine_type,
        gpu,
        gpu_count,
    })
}

/// The VM, or `None` once it no longer exists.
pub async fn describe(project: &str, zone: &str, name: &str) -> Result<Option<Instance>> {
    match gcloud(&[
        "compute".into(),
        "instances".into(),
        "describe".into(),
        name.into(),
        format!("--project={project}"),
        format!("--zone={zone}"),
        "--format=json".into(),
    ])
    .await
    {
        Ok(out) => Ok(serde_json::from_str::<Value>(&out)
            .ok()
            .and_then(|value| parse_instance(&value))),
        Err(error) if is_not_found(&error.to_string()) => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_not_found(message: &str) -> bool {
    message.contains("was not found") || message.contains("notFound")
}

/// Delete the VM; a VM that is already gone counts as deleted.
pub async fn delete(project: &str, zone: &str, name: &str) -> Result<()> {
    match gcloud(&[
        "compute".into(),
        "instances".into(),
        "delete".into(),
        name.into(),
        format!("--project={project}"),
        format!("--zone={zone}"),
        "--delete-disks=all".into(),
    ])
    .await
    {
        Ok(_) => Ok(()),
        Err(error) if is_not_found(&error.to_string()) => Ok(()),
        Err(error) => Err(error),
    }
}

/// Every VM orx created in the project, in any zone.
pub async fn list_orx_instances(project: &str) -> Result<Vec<Instance>> {
    let out = gcloud(&[
        "compute".into(),
        "instances".into(),
        "list".into(),
        format!("--project={project}"),
        format!("--filter=labels.{RUN_LABEL}:*"),
        "--format=json".into(),
    ])
    .await?;
    Ok(serde_json::from_str::<Value>(&out)
        .ok()
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(parse_instance)
        .collect())
}

pub enum WaitOutcome {
    Running(String),
    Cancelled,
    Failed(String),
}

/// Wait until the VM runs with an external IP.
pub async fn wait_running(
    project: &str,
    zone: &str,
    name: &str,
    cancelled: impl Fn() -> bool,
) -> WaitOutcome {
    let deadline = tokio::time::Instant::now() + PROVISION_DEADLINE;
    loop {
        if cancelled() {
            return WaitOutcome::Cancelled;
        }
        match describe(project, zone, name).await {
            Ok(Some(instance)) => match (instance.status.as_str(), instance.external_ip) {
                ("RUNNING", Some(ip)) => return WaitOutcome::Running(ip),
                ("RUNNING", None) => {
                    return WaitOutcome::Failed(
                        "the VM has no external IP, so orx cannot reach it over SSH".into(),
                    )
                }
                ("STOPPING" | "TERMINATED" | "SUSPENDED", _) => {
                    return WaitOutcome::Failed(format!(
                        "the VM stopped while starting ({})",
                        instance.status
                    ))
                }
                _ => {}
            },
            Ok(None) => return WaitOutcome::Failed("the VM no longer exists".into()),
            Err(error) => eprintln!("gcp: describe {name} failed (will retry): {error}"),
        }
        if tokio::time::Instant::now() >= deadline {
            return WaitOutcome::Failed(format!(
                "the VM did not start within {} minutes",
                PROVISION_DEADLINE.as_secs() / 60
            ));
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

// --- SSH --------------------------------------------------------------------------

/// gcloud's own key pair, created the way `gcloud compute ssh` would when this
/// machine has none yet.
pub fn ssh_key_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("No home directory for the SSH key."))?;
    Ok(home.join(".ssh").join("google_compute_engine"))
}

pub async fn ensure_ssh_key() -> Result<String> {
    let key = ssh_key_path()?;
    let public = key.with_extension("pub");
    if !public.is_file() {
        if let Some(parent) = key.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let status = Command::new("ssh-keygen")
            .args([
                "-t", "rsa", "-b", "3072", "-N", "", "-q", "-C", SSH_USER, "-f",
            ])
            .arg(&key)
            .stdin(Stdio::null())
            .status()
            .await
            .map_err(|e| anyhow!("Could not run ssh-keygen: {e}"))?;
        if !status.success() {
            return Err(anyhow!("ssh-keygen could not create {}.", key.display()));
        }
    }
    Ok(std::fs::read_to_string(&public)?.trim().to_string())
}

/// SSH to a VM by IP with orx's key. External IPs are recycled between VMs, so
/// nothing is pinned.
pub fn ssh_target(ip: &str) -> Result<super::ssh::SshTarget> {
    let mut target = super::ssh::SshTarget::host_port(
        format!("{SSH_USER}@{ip}"),
        22,
        super::ssh::HostKeyPolicy::Ephemeral,
    );
    target.extra_opts.extend([
        "-i".to_string(),
        ssh_key_path()?.display().to_string(),
        "-o".to_string(),
        "IdentitiesOnly=yes".to_string(),
    ]);
    Ok(target)
}

/// Before a GPU run, wait for the driver the image installs on first boot.
pub fn wait_for_gpu(script: &str) -> String {
    format!(
        "for i in $(seq 1 90); do nvidia-smi >/dev/null 2>&1 && break; \
         [ \"$i\" = 1 ] && echo 'orx: waiting for the NVIDIA driver to finish installing' >&2; \
         sleep 10; done\n{script}"
    )
}

// --- readiness and cleanup -----------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preflight {
    pub cli_path: Option<String>,
    pub account: Option<String>,
    pub project: Option<String>,
    pub zone: String,
    /// The Compute Engine API answered for this project and zone.
    pub compute_ready: bool,
    pub error: Option<String>,
}

pub async fn preflight(settings: &GcpSettings) -> Preflight {
    let cli_path = find_cli().map(|path| path.display().to_string());
    let mut result = Preflight {
        cli_path: cli_path.clone(),
        account: None,
        project: None,
        zone: settings.zone.clone().unwrap_or_else(|| DEFAULT_ZONE.into()),
        compute_ready: false,
        error: None,
    };
    if cli_path.is_none() {
        result.error = Some(INSTALL_HINT.into());
        return result;
    }
    result.account = account().await;
    let (project, zone) = resolve_location(settings).await;
    result.project = project.clone();
    result.zone = zone.clone();
    if result.account.is_none() {
        result.error = Some("gcloud is not signed in. Run `gcloud auth login`.".into());
        return result;
    }
    let Some(project) = project else {
        result.error = Some(
            "No Google Cloud project. Set one here or run `gcloud config set project <id>`.".into(),
        );
        return result;
    };
    match gcloud(&[
        "compute".into(),
        "zones".into(),
        "describe".into(),
        zone,
        format!("--project={project}"),
        "--format=value(name)".into(),
    ])
    .await
    {
        Ok(_) => result.compute_ready = true,
        Err(error) => result.error = Some(error.to_string()),
    }
    result
}

/// Delete VMs orx created for runs that have already ended. Returns their names.
pub async fn reap_orphans() -> Vec<String> {
    // Only machines that have launched on Google Cloud pay for a gcloud call.
    let Ok(store) = crate::store::Store::open() else {
        return Vec::new();
    };
    let used_gcp = store.list_runs(1000).is_ok_and(|runs| {
        runs.iter().any(|run| {
            serde_json::from_str::<Value>(&run.backend_json)
                .is_ok_and(|backend| backend["kind"] == "gcp_job")
        })
    });
    if !used_gcp || find_cli().is_none() {
        return Vec::new();
    }
    let settings = load_settings().ok().flatten().unwrap_or_default();
    let (Some(project), _) = resolve_location(&settings).await else {
        return Vec::new();
    };
    let Ok(instances) = list_orx_instances(&project).await else {
        return Vec::new();
    };
    let mut deleted = Vec::new();
    for instance in instances {
        let Some(run_id) = &instance.run_id else {
            continue;
        };
        let ended = store
            .get_run(run_id)
            .ok()
            .flatten()
            .is_some_and(|run| crate::store::is_terminal_status(&run.status));
        if !ended {
            continue;
        }
        match delete(&project, &instance.zone, &instance.name).await {
            Ok(()) => {
                eprintln!(
                    "orx: deleted Google Cloud VM {} left behind by run {run_id}",
                    instance.name
                );
                deleted.push(instance.name);
            }
            Err(error) => eprintln!(
                "orx: could not delete Google Cloud VM {}: {error}",
                instance.name
            ),
        }
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flavors_map_to_machine_types() {
        let t4 = parse_flavor("t4").unwrap();
        assert_eq!(t4.machine_type, "n1-standard-8");
        assert_eq!(t4.accelerator, Some(("nvidia-tesla-t4".into(), 1)));
        let v100 = parse_flavor("V100:4").unwrap();
        assert_eq!(v100.machine_type, "n1-standard-32");
        assert_eq!(v100.gpu_count, 4);
        assert_eq!(parse_flavor("l4").unwrap().machine_type, "g2-standard-8");
        assert_eq!(parse_flavor("l4:2").unwrap().machine_type, "g2-standard-24");
        let a100 = parse_flavor("a100:2").unwrap();
        assert_eq!(
            (a100.machine_type.as_str(), a100.accelerator),
            ("a2-highgpu-2g", None)
        );
        assert_eq!(
            parse_flavor("a100-80gb").unwrap().machine_type,
            "a2-ultragpu-1g"
        );
        assert_eq!(
            parse_flavor("h100:8").unwrap().machine_type,
            "a3-highgpu-8g"
        );
        assert_eq!(parse_flavor("cpu").unwrap().machine_type, "e2-standard-8");
        let cpu = parse_flavor("n2-standard-16").unwrap();
        assert_eq!(
            (cpu.machine_type.as_str(), cpu.gpu),
            ("n2-standard-16", None)
        );
        assert!(parse_flavor("t4:3").is_err());
        assert!(parse_flavor("t4:0").is_err());
        assert!(parse_flavor("rtx4090").is_err());
    }

    #[test]
    fn create_args_carry_the_safety_nets() {
        let shape = parse_flavor("t4").unwrap();
        let settings = GcpSettings {
            spot: true,
            ..GcpSettings::default()
        };
        let args = create_args(&CreateSpec {
            project: "my-proj",
            zone: "us-west1-b",
            name: "orx-run1",
            run_id: "Run1",
            shape: &shape,
            settings: &settings,
            ssh_public_key: "ssh-rsa AAAA tuna@laptop,work\n",
            timeout_secs: 3600,
        });
        for expected in [
            "--project=my-proj",
            "--zone=us-west1-b",
            "--machine-type=n1-standard-8",
            "--accelerator=type=nvidia-tesla-t4,count=1",
            "--maintenance-policy=TERMINATE",
            "--max-run-duration=7200s",
            "--instance-termination-action=DELETE",
            "--labels=orx-run=run1",
            "--provisioning-model=SPOT",
            "--metadata=install-nvidia-driver=True,enable-oslogin=FALSE,ssh-keys=orx:ssh-rsa AAAA orx",
        ] {
            assert!(args.iter().any(|arg| arg == expected), "missing {expected}: {args:?}");
        }
        let cpu = parse_flavor("cpu").unwrap();
        let args = create_args(&CreateSpec {
            shape: &cpu,
            ..CreateSpec {
                project: "p",
                zone: "z",
                name: "n",
                run_id: "r",
                shape: &cpu,
                settings: &GcpSettings::default(),
                ssh_public_key: "k",
                timeout_secs: 60,
            }
        });
        assert!(!args.iter().any(|arg| arg.starts_with("--accelerator")
            || arg.starts_with("--maintenance-policy")
            || arg.starts_with("--provisioning-model")));
    }

    #[test]
    fn instances_parse_gpu_and_run_label() {
        let n1 = parse_instance(&json!({
            "name": "orx-abc", "status": "RUNNING",
            "zone": "https://www.googleapis.com/compute/v1/projects/p/zones/us-central1-a",
            "machineType": "https://www.googleapis.com/compute/v1/projects/p/zones/us-central1-a/machineTypes/n1-standard-8",
            "guestAccelerators": [{"acceleratorType": "projects/p/zones/us-central1-a/acceleratorTypes/nvidia-tesla-t4", "acceleratorCount": 1}],
            "networkInterfaces": [{"accessConfigs": [{"natIP": "34.1.2.3"}]}],
            "labels": {"orx-run": "abc"}
        }))
        .unwrap();
        assert_eq!(n1.zone, "us-central1-a");
        assert_eq!(n1.gpu.as_deref(), Some("nvidia-tesla-t4"));
        assert_eq!(n1.external_ip.as_deref(), Some("34.1.2.3"));
        assert_eq!(n1.run_id.as_deref(), Some("abc"));
        let a3 = parse_instance(&json!({
            "name": "orx-x", "status": "PROVISIONING", "zone": "zones/us-east4-a",
            "machineType": "zones/us-east4-a/machineTypes/a3-highgpu-2g"
        }))
        .unwrap();
        assert_eq!(
            (a3.gpu.as_deref(), a3.gpu_count),
            (Some("NVIDIA H100 80GB"), 2)
        );
        assert!(a3.external_ip.is_none());
    }

    #[test]
    fn names_and_errors() {
        assert_eq!(
            instance_name("3F9C2A71-B0DE-4bb0"),
            "orx-3f9c2a71-b0de-4bb0"
        );
        assert!(gcloud_error("ERROR: (gcloud.compute.instances.create) Could not fetch resource:\n - The zone 'projects/p/zones/z' does not have enough resources available to fulfill the request.").contains("no free capacity"));
        assert!(gcloud_error(
            "ERROR: Quota 'NVIDIA_T4_GPUS' exceeded.  Limit: 0.0 in region us-central1."
        )
        .contains("Request more GPU quota"));
        assert!(is_not_found(
            "The resource 'projects/p/zones/z/instances/n' was not found"
        ));
    }
}
