//! Job backends — external compute that orx launches and supervises itself.
//!
//! Orx submits natively (HF Jobs, Modal, Kubernetes, SSH, Slurm, an OpenResearch
//! box, Tinker through a local controller, or this machine) and a detached
//! `orx supervise` watches the job beside it. The run's `backend_json` descriptor
//! is the serialized handle a later supervisor uses to reattach.

pub mod huggingface;
pub mod kubernetes;
pub mod localbox;
pub mod modal;
pub mod openresearch;
pub mod ray;
pub mod slurm;
pub mod ssh;
pub mod tinker;

use std::collections::HashMap;

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

/// Backend descriptor stored on the local run.
/// `kind` discriminates. This is a fixed field list — a key absent here does
/// NOT survive a parse → to_json round-trip, so anything a backend must keep
/// needs its own (optional) field.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDescriptor {
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
    /// Wall-clock bound the supervisor wraps around the payload
    /// (openresearch_job only) — persisted here because the launch happens in
    /// the supervisor, long after the `--timeout` flag is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Immutable local source archive used for this run. These fields make a
    /// delayed or restarted supervisor independent of the working tree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_size: Option<u64>,
}

impl BackendDescriptor {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| anyhow!("Unreadable backend descriptor: {}", e))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    /// The HF (namespace, jobId) handle, or an error naming what's missing.
    pub fn hf_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "hf_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(ns), Some(id)) => Ok((ns, id)),
            _ => Err(anyhow!(
                "Backend descriptor is missing namespace/jobId — was the job submitted?"
            )),
        }
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

    /// The Modal sandbox id (the reattach handle); `namespace` holds the app.
    pub fn modal_ref(&self) -> Result<&str> {
        if self.kind != "modal_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        self.job_id.as_deref().ok_or_else(|| {
            anyhow!("Backend descriptor is missing the Modal sandbox id — was the job submitted?")
        })
    }

    /// The Slurm (host, job id) handle; the login-node host rides on
    /// `namespace`, and the run dir derives from the run id.
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

    /// The local run dir (the reattach handle) for a local controller.
    pub fn local_ref(&self) -> Result<&str> {
        if !matches!(self.kind.as_str(), "local_job" | "tinker_job") {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        self.job_id.as_deref().ok_or_else(|| {
            anyhow!("Backend descriptor is missing the run dir — was the job submitted?")
        })
    }

    /// The Ray Jobs (address, submission id) handle; address rides on
    /// `namespace`, dashboard link on `url`.
    pub fn ray_ref(&self) -> Result<(&str, &str)> {
        if self.kind != "ray_job" {
            return Err(anyhow!("Unsupported backend kind: {}", self.kind));
        }
        match (self.namespace.as_deref(), self.job_id.as_deref()) {
            (Some(addr), Some(id)) => Ok((addr, id)),
            _ => Err(anyhow!(
                "Backend descriptor is missing the Ray address/submission id — was the job submitted?"
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
        Some(ssh::SshTarget::host_port(
            format!("{user}@{host}"),
            port,
            ssh::HostKeyPolicy::Ephemeral,
        ))
    }
}

/// Map an HF job stage onto the local run-status vocabulary. `UPDATING` appears
/// in the wild as a live state (see huggingface_hub).
pub fn stage_to_run_status(stage: &str) -> crate::store::RunStatus {
    use crate::store::RunStatus;
    match stage {
        "SCHEDULING" => RunStatus::Starting,
        "RUNNING" | "UPDATING" => RunStatus::Running,
        "COMPLETED" => RunStatus::Done,
        "ERROR" => RunStatus::Failed,
        "CANCELED" | "DELETED" => RunStatus::Cancelled,
        _ => RunStatus::Running,
    }
}

pub fn is_terminal_stage(stage: &str) -> bool {
    matches!(stage, "COMPLETED" | "CANCELED" | "ERROR" | "DELETED")
}

/// Canonical shape for a backend's poll result, in the shared stage
/// vocabulary above. `ssh`, `kubernetes`, `modal`, and `slurm` re-export this
/// type as their own `JobState` (`ray` as `JobInfo`) rather than each
/// defining an identical struct — one shape, so `supervise.rs`'s per-backend
/// poll loops never need backend-specific field access. Huggingface's native
/// API response nests this one level deeper (`JobInfo { status: JobStatus {
/// .. } }`); see `huggingface::JobInfo::state`.
#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
}

/// Debounces a scheduler-reported "GONE" stage — the job vanished from the
/// scheduler's own bookkeeping (Slurm's `squeue`/`sacct`, Ray's job list),
/// as opposed to a definite terminal report — before believing it. This also
/// fires during a scheduler restart or while a result is still propagating,
/// so a momentary "GONE" must persist for a full window before being treated
/// as a real, unrecoverable disappearance; any other observation resets it.
/// Shared by every backend whose scheduler can report this, so the window
/// and the reset rule can't drift between them.
pub struct GoneDebounce {
    threshold: u32,
    consecutive: u32,
}

/// What [`GoneDebounce::observe`] learned from one poll's raw stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoneOutcome {
    /// Not "GONE" — proceed normally with the raw stage (the counter reset).
    NotGone,
    /// "GONE", but hasn't persisted through the full debounce window yet —
    /// keep polling as usual without treating this as terminal.
    StillWaiting,
    /// "GONE" has now persisted for the full window — believe it: synthesize
    /// a terminal `ERROR` state rather than waiting any longer.
    ConfirmedGone,
}

impl GoneDebounce {
    /// `window` / `poll_interval` (rounded down, minimum 1) consecutive
    /// "GONE" polls are required before [`GoneOutcome::ConfirmedGone`].
    pub fn new(window: std::time::Duration, poll_interval: std::time::Duration) -> Self {
        let threshold = (window.as_secs() / poll_interval.as_secs().max(1)).max(1) as u32;
        Self {
            threshold,
            consecutive: 0,
        }
    }

    pub fn observe(&mut self, stage: &str) -> GoneOutcome {
        if stage != "GONE" {
            self.consecutive = 0;
            return GoneOutcome::NotGone;
        }
        self.consecutive += 1;
        if self.consecutive >= self.threshold {
            GoneOutcome::ConfirmedGone
        } else {
            GoneOutcome::StillWaiting
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openresearch_descriptor() -> BackendDescriptor {
        BackendDescriptor {
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
            source_digest: None,
            source_path: None,
            source_size: None,
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
        let json = d.to_json();
        assert!(json.contains("\"sshHost\":\"203.0.113.7\""), "{json}");

        let back = BackendDescriptor::parse(&json).unwrap();
        assert_eq!(back.ssh_port, Some(22022));
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
    }

    #[test]
    fn tinker_descriptor_round_trips_as_a_local_handle() {
        let json = r#"{"kind":"tinker_job","jobId":"/tmp/run"}"#;
        let descriptor = BackendDescriptor::parse(json).unwrap();
        assert_eq!(descriptor.local_ref().unwrap(), "/tmp/run");
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

    /// TASK 4 (compute backend contracts): a lone "GONE" poll — the shape a
    /// scheduler restart or an NFS-lagged status write produces — must not be
    /// believed; the debounce window must fully elapse first.
    #[test]
    fn gone_debounce_requires_the_full_window_of_consecutive_gone_polls() {
        let mut debounce = GoneDebounce::new(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(5),
        );
        assert_eq!(debounce.observe("GONE"), GoneOutcome::StillWaiting);
        assert_eq!(debounce.observe("GONE"), GoneOutcome::StillWaiting);
        assert_eq!(debounce.observe("GONE"), GoneOutcome::ConfirmedGone);
    }

    /// Any real observation in between resets the count — a job that flaps
    /// between "GONE" and a real stage never accumulates toward the window.
    #[test]
    fn gone_debounce_resets_on_any_non_gone_observation() {
        let mut debounce = GoneDebounce::new(
            std::time::Duration::from_secs(15),
            std::time::Duration::from_secs(5),
        );
        assert_eq!(debounce.observe("GONE"), GoneOutcome::StillWaiting);
        assert_eq!(debounce.observe("GONE"), GoneOutcome::StillWaiting);
        assert_eq!(debounce.observe("RUNNING"), GoneOutcome::NotGone);
        assert_eq!(debounce.observe("GONE"), GoneOutcome::StillWaiting);
        assert_eq!(debounce.observe("GONE"), GoneOutcome::StillWaiting);
        assert_eq!(debounce.observe("GONE"), GoneOutcome::ConfirmedGone);
    }

    /// A window shorter than one poll interval still requires at least one
    /// "GONE" poll — never confirms on the first sight of anything.
    #[test]
    fn gone_debounce_threshold_is_never_zero() {
        let mut debounce = GoneDebounce::new(
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(5),
        );
        assert_eq!(debounce.observe("GONE"), GoneOutcome::ConfirmedGone);
    }
}
