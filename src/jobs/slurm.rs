//! Slurm settings, and monitoring of runs that orx 0.2.x submitted. New runs submit through
//! the run engine's `orx-host` (`engine::adapters::slurm`).
//!
//! orx talks to the cluster's **login node** over ssh and drives the Slurm CLI there:
//! `squeue`/`sacct` to poll, `scancel` to cancel. No `slurmrestd`: the REST daemon needs
//! cluster-admin setup that most users don't have, while everyone with a cluster account can
//! ssh to the login node.
//!
//! The 0.2.x layout extends the ssh backend's convention. A run owns
//! `<root>/runs/<run_id>/` (root defaults to `~/.orx`) on the cluster's shared filesystem:
//!   repo/       the experiment branch, cloned on the login node at submit
//!               time (compute nodes often have no internet, so cloning
//!               inside the job — the other backends' pattern — would break)
//!   job.sbatch  the generated batch script (env exports + payload)
//!   log         merged stdout/stderr, captured by Slurm via `--output`
//!   exit_code   written by job.sbatch's closing lines when the payload ends
//!
//! The reattach handle is (host, slurm job id); the run dir derives from the
//! run id and the root recorded at submit. Job state is read exit_code-first (scheduler-independent truth,
//! like the ssh backend), then `squeue` for live jobs, then `sacct` for jobs
//! that left the queue without writing an exit code (scancel/timeout/node
//! failure). `sacct` may be disabled cluster-wide, so it's a best-effort
//! fallback, not a dependency.

use serde::{Deserialize, Serialize};

use super::ssh::{sh_quote, ssh_run, SshTarget};
use crate::error::{anyhow, Result};

// --- settings ---------------------------------------------------------------

/// User-tunable cluster defaults, stored at
/// `$XDG_CONFIG_HOME/openresearch/slurm.json`. No secrets in here — ssh
/// holds all auth. Every field is optional: a bare `sbatch` on a
/// single-partition cluster works with no configuration at all.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlurmSettings {
    /// ssh config host alias of the login node; the `--host` default.
    #[serde(default)]
    pub host: Option<String>,
    /// `#SBATCH --partition=…` default.
    #[serde(default)]
    pub partition: Option<String>,
    /// `#SBATCH --account=…` default (billing/allocation account).
    #[serde(default)]
    pub account: Option<String>,
    /// `#SBATCH --time=…` default, in orx's duration syntax ("4h", "30m").
    #[serde(default)]
    pub time_limit: Option<String>,
    /// `#SBATCH --cpus-per-task=…` default; the `--cpus` default.
    #[serde(default)]
    pub cpus_per_task: Option<u32>,
    /// `#SBATCH --mem=…` default in Slurm's own syntax ("64G"); the `--mem`
    /// default.
    #[serde(default)]
    pub mem: Option<String>,
    /// Absolute shared-filesystem root for runs and the source cache; `None` is `~/.orx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_root: Option<String>,
    /// Keep every finished run's remote dir, not just each project's newest few.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub keep_remote_runs: bool,
}

fn settings_path() -> std::path::PathBuf {
    crate::config::config_dir().join("slurm.json")
}

/// `Ok(None)` when the file is missing — slurm not configured (still usable
/// with explicit `--host`).
pub fn load_settings() -> Result<Option<SlurmSettings>> {
    let raw = match std::fs::read_to_string(settings_path()) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    match serde_json::from_str::<SlurmSettings>(&raw) {
        Ok(s) => Ok(Some(s)),
        Err(e) => Err(anyhow!(
            "Unreadable {} ({}). Fix or delete it and reconfigure.",
            settings_path().display(),
            e
        )),
    }
}

pub fn save_settings(settings: &SlurmSettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(settings)?);
    std::fs::write(&path, body)?;
    Ok(())
}

/// Validate a Slurm memory request: a positive integer with an optional
/// `K`/`M`/`G`/`T` suffix ("64G", "65536", "4000M"). Bare numbers are
/// megabytes, as Slurm itself documents. Strict on purpose — the value is
/// interpolated straight into a `#SBATCH` directive, so anything that is not
/// this shape (a newline, a shell metacharacter, a second directive) is
/// rejected here rather than reaching the cluster.
pub fn parse_mem(mem: &str) -> Result<String> {
    let m = mem.trim();
    let (digits, suffix) = match m.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => m.split_at(i),
        None => (m, ""),
    };
    let ok_suffix = matches!(suffix, "" | "K" | "M" | "G" | "T");
    if digits.is_empty() || digits.trim_start_matches('0').is_empty() || !ok_suffix {
        return Err(anyhow!(
            "Unreadable memory request {:?}. Use a positive size in Slurm's own syntax: \
             64G, 4000M, or a bare number for megabytes.",
            mem
        ));
    }
    Ok(format!("{digits}{suffix}"))
}

pub fn resolve_time_limit(explicit: Option<&str>, saved: Option<&str>) -> Result<Option<u64>> {
    explicit
        .or(saved)
        .map(super::huggingface::parse_timeout)
        .transpose()
}

/// Map a `--flavor` string onto a `--gres` request. A flavor names GPUs —
/// CPU-only runs just omit it (unlike HF/Modal there is no machine shape to
/// pick). Cores and memory are asked for separately, with `--cpus`/`--mem`;
/// omitting those leaves the partition defaults.
///   "gpu"      -> gpu:1
///   "gpu:…"    -> passed through verbatim (already a GRES spec)
///   "h100:2"   -> gpu:h100:2
///   "h100"     -> gpu:h100
pub fn resolve_gres(flavor: &str) -> Option<String> {
    let f = flavor.trim();
    if f.is_empty() {
        return None;
    }
    if f == "gpu" {
        return Some("gpu:1".to_string());
    }
    if f.starts_with("gpu:") {
        return Some(f.to_string());
    }
    Some(format!("gpu:{f}"))
}

/// Seconds → Slurm's `--time` syntax (`D-HH:MM:SS` / `HH:MM:SS`).
pub(crate) fn slurm_time(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    if days > 0 {
        format!("{days}-{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}:{s:02}")
    }
}

/// The remote run dir for a run (shared convention with the ssh backend),
/// derived from the root recorded in the descriptor at submit.
pub fn run_dir(root: Option<&str>, run_id: &str) -> String {
    super::ssh::remote_run_dir(root, run_id)
}

/// Job state in the shared stage vocabulary (see `jobs::stage_to_run_status`).
#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
}

// A job-specific query can fail after Slurm forgets a completed job.
fn queue_probe(job_id: &str) -> String {
    format!(
        "if st=$(squeue --all --states=all -h --jobs={job_id} -o %T 2>/dev/null); then :; \
         elif all=$(squeue --all --states=all --user=\"$(id -un)\" -h -o '%i %T' 2>/dev/null); then \
         st=$(printf '%s\\n' \"$all\" | while read -r id state; do \
         if [ \"$id\" = {job_id} ]; then printf '%s\\n' \"$state\"; fi; done); \
         else exit 1; fi; \
         if [ -n \"$st\" ]; then echo \"SQ $st\"; else echo GONE; fi",
        job_id = sh_quote(job_id),
    )
}

/// One combined remote probe emitting a single token — exit_code first
/// (ground truth), then live queue state, then accounting for jobs that left
/// the queue without one. POSIX-sh only: the command runs under the remote
/// user's login shell. `sacct -P` (parsable) because the default fixed-width
/// State column truncates ("CANCELLED by 1234" prints as "CANCELLED+").
///
/// `GONE` means monitoring is unavailable, not that the job terminated.
/// Accounting can be disabled or unavailable while a job still runs.
pub async fn inspect_job(host: &str, run_id: &str, dir: &str, job_id: &str) -> Result<JobState> {
    let value = super::ssh::service::request(
        &SshTarget::alias(host).for_run(run_id),
        serde_json::json!({"op":"slurm", "run":run_id, "job":job_id, "dir":dir}),
    )
    .await?;
    let token = value["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing Slurm status response"))?;
    Ok(map_inspect_token(token.trim()))
}

/// Pure token → stage mapping (unit-tested; Slurm state names are stable).
/// Emits the internal `GONE` stage for a job the scheduler no longer knows —
/// missing scheduler evidence never proves the job has terminated.
fn map_inspect_token(out: &str) -> JobState {
    let state = |stage: &str, message: Option<String>| JobState {
        stage: stage.to_string(),
        message,
    };
    if let Some(code) = out.strip_prefix("EXIT ") {
        // An empty file is the window between open(O_TRUNC) and the write —
        // not a verdict; poll again.
        if code.trim().is_empty() {
            return state("RUNNING", None);
        }
        let code: i32 = code.trim().parse().unwrap_or(-1);
        return if code == 0 {
            state("COMPLETED", None)
        } else {
            state("ERROR", Some(format!("exited with code {code}")))
        };
    }
    // `squeue %T` / `sacct -P State` values. sacct suffixes cancellations
    // ("CANCELLED by 1234"), so match on the first word; the trailing-`+`
    // trim is belt-and-braces against fixed-width truncation.
    let (source, raw) = match (out.strip_prefix("SQ "), out.strip_prefix("SA ")) {
        (Some(s), _) => ("squeue", s),
        (_, Some(s)) => ("sacct", s),
        _ if matches!(out, "GONE" | "UNAVAILABLE") => return state(out, None),
        _ => return state("RUNNING", Some(format!("unexpected inspect output: {out}"))),
    };
    let word = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches('+');
    match word {
        "PENDING" | "CONFIGURING" | "REQUEUED" | "RESV_DEL_HOLD" => state("SCHEDULING", None),
        "RUNNING" | "COMPLETING" | "SUSPENDED" | "STAGE_OUT" | "SIGNALING" => {
            state("RUNNING", None)
        }
        // The batch script exits with the payload code, so either scheduler verdict is authoritative.
        "COMPLETED" => state("COMPLETED", None),
        // Transient from squeue: default JobRequeue=1 (and
        // PreemptMode=REQUEUE) re-queues these under the same job id — a poll
        // landing in the window must not finalize a run that's about to
        // re-run. If the job is NOT requeued it leaves the queue and the
        // sacct path delivers the terminal verdict.
        "NODE_FAIL" | "PREEMPTED" if source == "squeue" => state("RUNNING", None),
        "CANCELLED" | "REVOKED" => state("CANCELED", None),
        "TIMEOUT" | "DEADLINE" => state("ERROR", Some("job hit its time limit".into())),
        "FAILED" | "NODE_FAIL" | "BOOT_FAIL" | "OUT_OF_MEMORY" | "PREEMPTED" => state(
            "ERROR",
            Some(format!("slurm reported {}", word.to_ascii_lowercase())),
        ),
        other => state(
            "RUNNING",
            Some(format!("unrecognized slurm state: {other}")),
        ),
    }
}

/// Cancellation succeeds when accepted or the controller confirms the job is absent.
pub async fn cancel_job(host: &str, run_id: &str, job_id: &str) -> Result<()> {
    let queue = queue_probe(job_id);
    super::ssh::ssh_control(
        &SshTarget::alias(host).for_run(run_id),
        &format!(
            "if error=$(scancel {} 2>&1); then exit 0; \
             elif [ \"$({queue})\" = GONE ]; then exit 0; \
             else printf '%s\\n' \"$error\" >&2; exit 1; fi",
            sh_quote(job_id)
        ),
        None,
    )
    .await?;
    Ok(())
}

// Log streaming reuses `ssh::stream_logs` directly — the run-dir/`log` layout
// is identical, and Slurm appends to the same file via `--output`.

// --- preflight ----------------------------------------------------------------

/// Per-host readiness for the Settings UI: reachable, Slurm CLI + snapshot tools
/// present, and which partitions exist.
pub struct SlurmPreflight {
    pub reachable: bool,
    pub slurm_found: bool,
    pub tools_found: bool,
    /// From `sinfo` (default partition's trailing `*` stripped).
    pub partitions: Vec<String>,
    pub error: Option<String>,
}

pub async fn preflight(target: &SshTarget) -> SlurmPreflight {
    let cmd = "if command -v sbatch >/dev/null 2>&1 && command -v squeue >/dev/null 2>&1 \
               && command -v scancel >/dev/null 2>&1; then echo SLURM_OK; fi; \
               if command -v bash >/dev/null 2>&1 && command -v tar >/dev/null 2>&1; \
               then echo TOOLS_OK; fi; \
               sinfo -h -o %P 2>/dev/null || true";
    match ssh_run(target, cmd, None).await {
        Ok(out) => {
            let mut slurm_found = false;
            let mut tools_found = false;
            let mut partitions = Vec::new();
            for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
                match line {
                    "SLURM_OK" => slurm_found = true,
                    "TOOLS_OK" => tools_found = true,
                    p => {
                        let p = p.trim_end_matches('*').to_string();
                        if !p.is_empty() && !partitions.contains(&p) {
                            partitions.push(p);
                        }
                    }
                }
            }
            SlurmPreflight {
                reachable: true,
                slurm_found,
                tools_found,
                partitions,
                error: None,
            }
        }
        Err(e) => SlurmPreflight {
            reachable: false,
            slurm_found: false,
            tools_found: false,
            partitions: Vec::new(),
            error: Some(e.to_string()),
        },
    }
}

/// Best-effort accounting details; absent records are explicitly unknown.
pub async fn diagnostics(host: &str, job_id: &str) -> Result<serde_json::Value> {
    let cmd = format!(
        "sacct -nPX -j {} -o State,Timelimit,Elapsed,Reason,ExitCode",
        sh_quote(job_id)
    );
    let output = ssh_run(&SshTarget::alias(host), &cmd, None).await?;
    let Some(line) = output.lines().find(|line| !line.trim().is_empty()) else {
        return Ok(serde_json::json!({"available": false}));
    };
    let fields: Vec<_> = line.split('|').map(str::trim).collect();
    if fields.len() != 5 {
        return Err(anyhow!("Unrecognized Slurm accounting response."));
    }
    Ok(
        serde_json::json!({"available": true, "state": fields[0], "timeLimit": fields[1], "elapsed": fields[2], "schedulingReason": fields[3], "exitCode": fields[4]}),
    )
}

// --- tests ----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_parses_slurm_sizes_and_rejects_the_rest() {
        assert_eq!(parse_mem("64G").unwrap(), "64G");
        assert_eq!(parse_mem(" 4000M ").unwrap(), "4000M");
        assert_eq!(parse_mem("65536").unwrap(), "65536");
        for bad in [
            "",
            "0",
            "G",
            "64g",
            "64GB",
            "64 G",
            "-1",
            "64G\n#SBATCH --account=someone-else",
        ] {
            assert!(parse_mem(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn slurm_time_formats() {
        assert_eq!(slurm_time(90), "00:01:30");
        assert_eq!(slurm_time(4 * 3600), "04:00:00");
        assert_eq!(slurm_time(86_400 + 3661), "1-01:01:01");
    }

    #[test]
    fn gres_from_flavor() {
        assert_eq!(resolve_gres(""), None);
        assert_eq!(resolve_gres("gpu").as_deref(), Some("gpu:1"));
        assert_eq!(resolve_gres("gpu:a100:4").as_deref(), Some("gpu:a100:4"));
        assert_eq!(resolve_gres("h100:2").as_deref(), Some("gpu:h100:2"));
        assert_eq!(resolve_gres("h100").as_deref(), Some("gpu:h100"));
    }

    #[test]
    fn inspect_token_mapping() {
        assert_eq!(map_inspect_token("EXIT 0").stage, "COMPLETED");
        let failed = map_inspect_token("EXIT 137");
        assert_eq!(failed.stage, "ERROR");
        assert!(failed.message.unwrap().contains("137"));
        // Empty exit_code = caught mid-write; not a verdict.
        assert_eq!(map_inspect_token("EXIT ").stage, "RUNNING");
        assert_eq!(map_inspect_token("SQ PENDING").stage, "SCHEDULING");
        assert_eq!(map_inspect_token("SQ RUNNING").stage, "RUNNING");
        assert_eq!(map_inspect_token("SQ COMPLETING").stage, "RUNNING");
        // Both scheduler sources confirm the script (= payload) exited successfully.
        assert_eq!(map_inspect_token("SQ COMPLETED").stage, "COMPLETED");
        assert_eq!(map_inspect_token("SA COMPLETED").stage, "COMPLETED");
        assert_eq!(map_inspect_token("SA CANCELLED by 1234").stage, "CANCELED");
        // Fixed-width sacct truncates with a trailing '+'; -P avoids it, but
        // the parser must survive it anyway.
        assert_eq!(map_inspect_token("SA CANCELLED+").stage, "CANCELED");
        assert_eq!(map_inspect_token("SA TIMEOUT").stage, "ERROR");
        assert_eq!(map_inspect_token("SA NODE_FAIL").stage, "ERROR");
        // From squeue these are requeue-transient (JobRequeue=1); terminal
        // only once sacct confirms the job really left.
        assert_eq!(map_inspect_token("SQ NODE_FAIL").stage, "RUNNING");
        assert_eq!(map_inspect_token("SQ PREEMPTED").stage, "RUNNING");
        // GONE is not terminal: absent accounting is not evidence of termination.
        assert_eq!(map_inspect_token("GONE").stage, "GONE");
        // Unknown states never wedge the supervisor into a terminal state.
        assert_eq!(map_inspect_token("SQ SOMETHING_NEW").stage, "RUNNING");
    }
}
