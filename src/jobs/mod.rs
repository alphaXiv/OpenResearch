//! Job backends — provider clients the run engine's adapters drive.
//!
//! A run launched by orx 0.2 keeps its handle in `backend_json` as a [`BackendDescriptor`], which
//! the engine reads to watch or adopt it.

pub mod huggingface;
pub mod kubernetes;
pub mod localbox;
pub(crate) mod managed_env;
pub mod modal;
pub mod openresearch;
pub mod ray;
pub mod rental;
pub mod slurm;
pub mod ssh;
pub mod tinker;
pub mod vercel;

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::error::{anyhow, Result};

/// CPython env var that forces stdout/stderr unbuffered. Every backend streams a
/// job's output by tailing a pipe or a redirected `log` file, so a block-buffered
/// job would make its logs appear frozen until the buffer fills. We default it on
/// so prints stream live; it's inert for non-Python jobs and inherited by child
/// processes (unlike the `-u` flag).
pub const PYTHONUNBUFFERED: &str = "PYTHONUNBUFFERED";

/// Redirected CPython output uses Windows' ANSI codepage (often cp1252), so other
/// characters crash a run.
pub const PYTHONIOENCODING: &str = "PYTHONIOENCODING";

/// Default CPython's streaming and encoding unless the caller set them; kubernetes
/// open-codes it because its env is a JSON array.
pub fn default_python_env(env: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env = env.clone();
    env.entry(PYTHONUNBUFFERED.to_string())
        .or_insert_with(|| "1".to_string());
    env.entry(PYTHONIOENCODING.to_string())
        .or_insert_with(|| "utf-8".to_string());
    env
}

/// This process's interpreter overrides, for `orx up` to check against its own.
pub fn caller_python_overrides() -> BTreeMap<String, String> {
    [modal::MODAL_ENV, vercel::VERCEL_ENV]
        .iter()
        .filter_map(|env| env.override_var)
        .filter_map(|var| Some((var.to_string(), std::env::var(var).ok()?)))
        .filter(|(_, python)| !python.trim().is_empty())
        .collect()
}

/// A provider's answer to a launch that nothing started: its HTTP status, which the engine
/// judges, or `None` when nothing is left running for certain.
#[derive(Debug, Deserialize)]
pub struct Refused {
    pub status: Option<u16>,
    pub message: String,
}

/// Launchers exit with this code after printing a [`Refused`] as JSON on stderr.
const LAUNCHER_REFUSED: i32 = 6;

/// A Python launcher's refusal, if its exit says it printed one.
fn launcher_refusal(code: Option<i32>, stderr: &str) -> Option<Refused> {
    if code != Some(LAUNCHER_REFUSED) {
        return None;
    }
    serde_json::from_str(stderr.trim()).ok()
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Refused {}

/// Backend descriptor stored on the local run; `kind` discriminates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDescriptor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_container: Option<ssh::ContainerRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monitoring_error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancellation_accepted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rental_lease: Option<rental::Rental>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flavor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// kubeconfig context (k8s_job only); `None` = kubectl current-context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Repo-relative manifest path the run was launched from (k8s_job only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Everything the manifest created, as `kind/name` handles in creation
    /// order (k8s_job only) — cancel deletes exactly this list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<String>>,
    /// The box's SSH endpoint (openresearch_job only), recorded by the
    /// supervisor once provisioning finishes — `None` while the box boots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_port: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_user: Option<String>,
    /// Requested execution limit for OpenResearch, Hugging Face, and Slurm jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Requested `#SBATCH --cpus-per-task` (slurm_job only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus_per_task: Option<u32>,
    /// Requested `#SBATCH --mem` (slurm_job only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mem: Option<String>,
    /// Immutable local source archive used for this run. These fields make a
    /// delayed or restarted supervisor independent of the working tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_size: Option<u64>,
    /// The host's storage root at submit; Slurm derives its run dir from it, SSH keeps it for source-cache cleanup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_root: Option<String>,
    /// Fields a newer orx wrote, kept so an older supervisor's rewrite doesn't drop them.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
}

impl BackendDescriptor {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| anyhow!("Unreadable backend descriptor: {}", e))
    }

    /// Writes a 0.2.x row for tests; orx no longer writes descriptors.
    #[cfg(test)]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap()
    }

    /// The k8s (namespace, job name) handle; context rides on `self.context`.
    pub fn k8s_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "k8s_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(ns), Some(id)) => Ok((ns, id)),
            _ => Err(anyhow!(
                "Backend descriptor is missing namespace/jobName — was the job submitted?"
            )),
        }
    }

    /// The Vercel (sandbox name, process id) handle; the name rides on
    /// `namespace`.
    pub fn vercel_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "vercel_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(name), Some(pid)) => Ok((name, pid)),
            _ => Err(anyhow!(
                "Backend descriptor is missing the Vercel sandbox name/process id — was the job submitted?"
            )),
        }
    }

    /// The Slurm (host, job id) handle; the login-node host rides on
    /// `namespace`, and the run dir derives from the run id and `remote_root`.
    pub fn slurm_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "slurm_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(host), Some(id)) => Ok((host, id)),
            _ => Err(anyhow!(
                "Backend descriptor is missing the slurm host/job id — was the job submitted?"
            )),
        }
    }

    /// The SSH (host, remote run dir) handle; host rides on `namespace`.
    pub fn ssh_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "ssh_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(host), Some(dir)) => Ok((host, dir)),
            _ => Err(anyhow!(
                "Backend descriptor is missing the ssh host/dir — was the job submitted?"
            )),
        }
    }

    /// The OpenResearch (org id, sandbox id) handle; the org rides on
    /// `namespace`, and the run dir derives from the run id.
    pub fn openresearch_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "openresearch_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(org), Some(id)) => Ok((org, id)),
            _ => Err(anyhow!(
                "Backend descriptor is missing the org/sandbox id — was the box provisioned?"
            )),
        }
    }

    /// The box's SSH endpoint as a ready-to-use target, once the supervisor
    /// has recorded it (openresearch_job only). Uses
    /// [`ssh::HostKeyPolicy::Ephemeral`] — see its docs for why these
    /// machine-provisioned, host:port-recycled boxes accept any key.
    pub fn openresearch_ssh_target(&self) -> Option<ssh::SshTarget> {
        if self.kind != "openresearch_job" {
            return None;
        }
        let host = self.ssh_host.as_deref()?;
        let user = self.ssh_user.as_deref()?;
        // Stored as i64 (SQLite); a real SSH port fits u16. Bail rather than
        // panic if the row somehow holds an out-of-range value.
        let port = u16::try_from(self.ssh_port?).ok()?;
        let mut target = ssh::SshTarget::host_port(
            format!("{user}@{host}"),
            port,
            ssh::HostKeyPolicy::Ephemeral,
        );
        target.managed_generation = Some(self.job_id.clone()?);
        Some(target)
    }
}

pub fn is_terminal_stage(stage: &str) -> bool {
    matches!(stage, "COMPLETED" | "CANCELED" | "ERROR" | "DELETED")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openresearch_descriptor() -> BackendDescriptor {
        BackendDescriptor {
            ssh_container: None,
            monitoring_error: None,
            cancellation_accepted: false,
            rental_lease: None,
            kind: "openresearch_job".to_string(),
            namespace: Some("org_1".to_string()),
            job_id: Some("sb_1".to_string()),
            flavor: Some("h100_sxm:2".to_string()),
            image: None,
            url: None,
            context: None,
            manifest: None,
            resources: None,
            ssh_host: None,
            ssh_port: None,
            ssh_user: None,
            timeout_secs: Some(14_400),
            cpus_per_task: None,
            mem: None,
            source_digest: None,
            source_path: None,
            source_size: None,
            remote_root: None,
            unknown: Default::default(),
        }
    }

    /// The ssh endpoint fields are camelCase on the wire, absent while `None`,
    /// and survive a parse → to_json round-trip once set.
    #[test]
    fn openresearch_descriptor_round_trips() {
        let mut d = openresearch_descriptor();
        let json = d.to_json();
        assert!(
            !json.contains("sshHost"),
            "None fields must not serialize: {json}"
        );
        assert!(json.contains("\"timeoutSecs\":14400"), "{json}");

        d.ssh_host = Some("203.0.113.7".to_string());
        d.ssh_port = Some(22022);
        d.ssh_user = Some("root".to_string());
        d.cpus_per_task = Some(8);
        d.mem = Some("64G".to_string());
        let json = d.to_json();
        assert!(json.contains("\"sshHost\":\"203.0.113.7\""), "{json}");
        assert!(json.contains("\"cpusPerTask\":8"), "{json}");

        let back = BackendDescriptor::parse(&json).unwrap();
        assert_eq!(back.ssh_port, Some(22022));
        assert_eq!(back.mem.as_deref(), Some("64G"));
        assert_eq!(
            crate::local::slurm::requested_resources(&back).as_deref(),
            Some("cpus 8  mem 64G")
        );
        assert_eq!(
            crate::local::slurm::status_resources(&back),
            "requested cpus 8  mem 64G"
        );
        d.cpus_per_task = None;
        assert_eq!(
            crate::local::slurm::requested_resources(&d).as_deref(),
            Some("cpus partition default  mem 64G")
        );
        d.mem = None;
        assert_eq!(crate::local::slurm::requested_resources(&d), None);
        assert_eq!(back.openresearch_ref().unwrap(), ("org_1", "sb_1"));
    }

    /// Descriptors written before the ssh/timeout fields existed still parse.
    #[test]
    fn older_descriptors_without_new_fields_still_parse() {
        let d = BackendDescriptor::parse(
            r#"{"kind":"ssh_job","namespace":"mybox","jobId":".orx/runs/r1"}"#,
        )
        .unwrap();
        assert_eq!(d.ssh_ref().unwrap(), ("mybox", ".orx/runs/r1"));
        assert_eq!(d.ssh_host, None);
        assert_eq!(d.timeout_secs, None);
        assert_eq!(d.cpus_per_task, None);
        // A Slurm run from before cpus/mem were recorded may still have requested them.
        let slurm =
            BackendDescriptor::parse(r#"{"kind":"slurm_job","namespace":"login","jobId":"123"}"#)
                .unwrap();
        assert_eq!(
            crate::local::slurm::status_resources(&slurm),
            "requested cpus/mem: unknown (older run or partition default)"
        );
        assert_eq!(d.ssh_container, None);
        let mut d = d;
        d.ssh_container = Some(ssh::ContainerRun {
            reference: "research".into(),
            id: "immutable-id".into(),
            started_at: "2026-09-21T00:00:00Z".into(),
            run_dir: "/home/user/.orx/runs/r1".into(),
            unknown: Default::default(),
        });
        let json = d.to_json();
        assert!(json.contains("sshContainer"));
        assert_eq!(
            BackendDescriptor::parse(&json).unwrap().ssh_container,
            d.ssh_container
        );
    }

    /// One descriptor per backend as shipped versions wrote it. An older supervisor
    /// reattaching to a newer run rewrites the descriptor, so a field it does not
    /// know, at any level, must survive.
    #[test]
    fn shipped_descriptors_keep_fields_from_newer_versions() {
        let shipped: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/contracts/run-descriptors.json"))
                .unwrap();
        let newer = serde_json::json!({"nested": [1, 2]});
        for mut descriptor in shipped {
            for nested in descriptor.as_object_mut().unwrap().values_mut() {
                if let Some(nested) = nested.as_object_mut() {
                    nested.insert("fieldFromNewerOrx".into(), newer.clone());
                }
            }
            descriptor["fieldFromNewerOrx"] = newer.clone();
            let parsed = BackendDescriptor::parse(&descriptor.to_string()).unwrap();
            let rewritten: serde_json::Value = serde_json::from_str(&parsed.to_json()).unwrap();
            assert_eq!(rewritten, descriptor);
        }
    }

    #[test]
    fn tinker_descriptor_round_trips_as_a_local_handle() {
        let json = r#"{"kind":"tinker_job","jobId":"/tmp/run"}"#;
        let descriptor = BackendDescriptor::parse(json).unwrap();
        assert_eq!(
            BackendDescriptor::parse(&descriptor.to_json())
                .unwrap()
                .kind,
            "tinker_job"
        );
    }

    #[test]
    fn openresearch_ref_rejects_other_kinds() {
        let mut d = openresearch_descriptor();
        d.kind = "ssh_job".to_string();
        assert!(d.openresearch_ref().is_err());
        assert!(d.openresearch_ssh_target().is_none());
    }

    /// The target is only available once the supervisor recorded the endpoint,
    /// and carries the port + first-use host-key options.
    #[test]
    fn openresearch_ssh_target_builds_dest_and_opts() {
        let mut d = openresearch_descriptor();
        assert!(d.openresearch_ssh_target().is_none());
        d.ssh_host = Some("203.0.113.7".to_string());
        d.ssh_port = Some(22022);
        d.ssh_user = Some("root".to_string());
        let target = d.openresearch_ssh_target().unwrap();
        assert_eq!(target.dest, "root@203.0.113.7");
        let opts = target.extra_opts.join(" ");
        assert!(opts.contains("-p 22022"), "{opts}");
        assert!(opts.contains("StrictHostKeyChecking=no"), "{opts}");
    }

    #[test]
    fn default_python_env_injects_when_absent_and_lets_author_win() {
        // Injected when the caller didn't set them.
        let got = default_python_env(&HashMap::new());
        assert_eq!(got.get(PYTHONUNBUFFERED).map(String::as_str), Some("1"));
        assert_eq!(got.get(PYTHONIOENCODING).map(String::as_str), Some("utf-8"));

        // Explicit values are preserved — even falsy ones — never overwritten.
        let author = HashMap::from([
            (PYTHONUNBUFFERED.to_string(), "0".to_string()),
            (PYTHONIOENCODING.to_string(), "cp1252".to_string()),
        ]);
        let got = default_python_env(&author);
        assert_eq!(got.get(PYTHONUNBUFFERED).map(String::as_str), Some("0"));
        assert_eq!(
            got.get(PYTHONIOENCODING).map(String::as_str),
            Some("cp1252")
        );
    }
}
