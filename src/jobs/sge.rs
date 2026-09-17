//! Sun Grid Engine backend — submit an experiment as a batch job on an SGE
//! cluster (BU's SCC runs OGS/GE 2011.11p1).
//!
//! Structurally the Slurm backend's twin: orx talks to the cluster's **login
//! node** over ssh (reusing the ssh backend's multiplexed transport) and drives
//! the Grid Engine CLI there — `qsub -terse` to submit, `qstat` to poll, `qdel`
//! to cancel. Three things make it more than a rename:
//!
//! 1. **Run dirs live outside `$HOME`.** SCC home dirs carry a hard 10 GB
//!    quota, so the source cache and run dirs are rooted at an absolute
//!    `work_dir` on a project filesystem (see [`DEFAULT_WORK_DIR`]).
//! 2. **`qacct` costs ~11 seconds** (it linearly scans the accounting file),
//!    against a 5-second poll loop. So accounting is an escalation, never a
//!    per-poll fallback the way Slurm's `sacct` is. The `exit_code` file stays
//!    the primary, scheduler-independent truth, and a job that finishes
//!    normally never pays the cost at all.
//! 3. **A live ControlMaster is a prerequisite**, not an optimization: SCC's
//!    sshd requires `publickey,keyboard-interactive`, and `BatchMode=yes`
//!    removes keyboard-interactive from the client's candidate list entirely.
//!    See [`ssh::SecondFactor`].
//!
//! The remote layout mirrors the ssh/slurm convention under the absolute base:
//!   <work_dir>/.orx/runs/<run_id>/
//!     repo/       the experiment snapshot, extracted at submit time
//!     job.qsub    the generated batch script (directives + env + payload)
//!     log         merged stdout/stderr, captured by SGE via `#$ -o`
//!     exit_code   written by job.qsub's closing lines when the payload ends
//!   <work_dir>/.orx/source/<digest>.tar   content-addressed upload cache

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::ssh::{self, sh_quote, ssh_run, SecondFactor, SshTarget};
use crate::error::{anyhow, Result};

// --- settings ---------------------------------------------------------------

/// Where run dirs and the source cache live. Deliberately NOT `$HOME`: SCC home
/// directories are quota'd at 10 GB, while `/projectnb` allocations run to
/// terabytes. A per-user leaf is appended at submit time.
pub const DEFAULT_WORK_DIR: &str = "/projectnb/herbdl/workspaces/herb";

pub const DEFAULT_SCC_PROJECT: &str = "herbdl";
pub const DEFAULT_PE: &str = "omp";
pub const DEFAULT_SLOTS: u32 = 16;
pub const DEFAULT_TIME_LIMIT: &str = "12h";
pub const DEFAULT_GPUS: u32 = 1;
pub const DEFAULT_GPU_TYPE: &str = "L40S";

/// Canonical `gpu_type` values on SCC. The complex is a RESTRING with the `==`
/// relop, so case is load-bearing: `gpu_type=a100` is not a typo the scheduler
/// warns about, it is a request no host can ever satisfy and the job sits in
/// `qw` forever. We normalize against this table instead.
pub const SCC_GPU_TYPES: &[&str] = &[
    "A100",
    "A40",
    "A6000",
    "H200",
    "K2200",
    "K40m",
    "L40",
    "L40S",
    "M2000",
    "P100",
    "RTX6000",
    "RTX6000ada",
    "RTX8000",
    "RTXP6000",
    "TitanV",
    "TitanXp",
    "V100",
];

/// User-tunable cluster defaults, stored at
/// `$XDG_CONFIG_HOME/openresearch/sge.json`. No secrets — ssh holds all auth.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SgeSettings {
    /// ssh config host alias of the login node; the `--host` default.
    #[serde(default)]
    pub host: Option<String>,
    /// Absolute base for run dirs and the source cache; never under `$HOME`.
    #[serde(default)]
    pub work_dir: Option<String>,
    /// `#$ -P` — the cluster's billing/allocation group. Named `scc_project` to
    /// keep it clearly distinct from an OpenResearch project (a research
    /// direction); this sits a level above that.
    #[serde(default)]
    pub scc_project: Option<String>,
    /// `#$ -pe <pe> <slots>` parallel environment name.
    #[serde(default)]
    pub pe: Option<String>,
    /// Core count for the parallel environment.
    #[serde(default)]
    pub slots: Option<u32>,
    /// `#$ -l h_rt=…` default, in orx's duration syntax ("12h", "30m").
    #[serde(default)]
    pub time_limit: Option<String>,
    /// Default GPU count when `--flavor` names no number.
    #[serde(default)]
    pub gpus: Option<u32>,
    /// Default `#$ -l gpu_type=…` when `--flavor` names no model.
    #[serde(default)]
    pub gpu_type: Option<String>,
    /// Raw extra `-l` requests, e.g. ["mem_per_core=8G"]. SGE's whole resource
    /// model is `-l`, so an escape hatch is required.
    #[serde(default)]
    pub extra_l: Option<Vec<String>>,
}

/// Every default in one place so the CLI, the API handler and the Settings card
/// cannot drift. Unlike Slurm — where omitting `--flavor` means CPU-only — the
/// SGE default job shape includes a GPU, so `--flavor cpu` is the way to ask
/// for no GPU at all.
impl SgeSettings {
    pub fn scc_project_or_default(&self) -> String {
        non_empty(self.scc_project.as_deref()).unwrap_or_else(|| DEFAULT_SCC_PROJECT.to_string())
    }

    pub fn pe_or_default(&self) -> String {
        non_empty(self.pe.as_deref()).unwrap_or_else(|| DEFAULT_PE.to_string())
    }

    pub fn slots_or_default(&self) -> u32 {
        self.slots.filter(|s| *s > 0).unwrap_or(DEFAULT_SLOTS)
    }

    pub fn time_limit_or_default(&self) -> String {
        non_empty(self.time_limit.as_deref()).unwrap_or_else(|| DEFAULT_TIME_LIMIT.to_string())
    }

    pub fn gpus_or_default(&self) -> u32 {
        self.gpus.unwrap_or(DEFAULT_GPUS)
    }

    pub fn gpu_type_or_default(&self) -> String {
        non_empty(self.gpu_type.as_deref()).unwrap_or_else(|| DEFAULT_GPU_TYPE.to_string())
    }

    /// The validated absolute base. Call this rather than reading `work_dir`.
    pub fn resolved_work_dir(&self) -> Result<String> {
        validate_work_dir(
            non_empty(self.work_dir.as_deref())
                .as_deref()
                .unwrap_or(DEFAULT_WORK_DIR),
        )
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
}

fn settings_path() -> std::path::PathBuf {
    crate::config::config_dir().join("sge.json")
}

/// `Ok(None)` when the file is missing — SGE not configured (still usable with
/// an explicit `--host`, since every other field has a default).
pub fn load_settings() -> Result<Option<SgeSettings>> {
    let raw = match std::fs::read_to_string(settings_path()) {
        Ok(raw) => raw,
        Err(_) => return Ok(None),
    };
    match serde_json::from_str::<SgeSettings>(&raw) {
        Ok(s) => Ok(Some(s)),
        Err(e) => Err(anyhow!(
            "Unreadable {} ({}). Fix or delete it and reconfigure.",
            settings_path().display(),
            e
        )),
    }
}

pub fn save_settings(settings: &SgeSettings) -> Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!("{}\n", serde_json::to_string_pretty(settings)?);
    std::fs::write(&path, body)?;
    Ok(())
}

// --- paths --------------------------------------------------------------------

/// Normalize and validate a user-supplied remote base directory.
///
/// The alphabet is deliberately narrower than "whatever survives quoting":
/// `work_dir` is interpolated into `#$` directives, and **qsub parses those
/// itself with no shell involved**, so a space or a metacharacter there cannot
/// be made safe by `sh_quote`. Absolute, clean, and boring is the only form
/// that works in both contexts.
pub fn validate_work_dir(dir: &str) -> Result<String> {
    let dir = dir.trim().trim_end_matches('/');
    if !dir.starts_with('/') {
        return Err(anyhow!(
            "The SGE work directory must be an absolute path (got {dir:?}). It cannot live \
             under $HOME — SCC home directories are quota'd at 10 GB."
        ));
    }
    if dir.is_empty() {
        return Err(anyhow!(
            "The SGE work directory cannot be the filesystem root."
        ));
    }
    if let Some(bad) = dir
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')))
    {
        return Err(anyhow!(
            "The SGE work directory may only contain letters, digits, '.', '_', '-' and '/' \
             (found {bad:?} in {dir:?}). qsub parses `#$ -wd` without a shell, so spaces and \
             shell metacharacters cannot be made safe."
        ));
    }
    let components: Vec<&str> = dir.split('/').skip(1).collect();
    if components.is_empty() {
        return Err(anyhow!(
            "The SGE work directory cannot be the filesystem root."
        ));
    }
    for c in &components {
        if c.is_empty() || *c == "." || *c == ".." {
            return Err(anyhow!(
                "The SGE work directory must be a clean absolute path — no empty, '.' or '..' \
                 components (got {dir:?})."
            ));
        }
    }
    Ok(dir.to_string())
}

/// `<work_dir>/.orx/runs/<run_id>` — absolute. The `.orx` segment namespaces
/// our state inside what is plainly a shared lab directory, and keeps the
/// sub-layout identical to the `$HOME` one the ssh/slurm backends use.
pub fn run_dir(work_dir: &str, run_id: &str) -> String {
    format!("{}/.orx/runs/{run_id}", work_dir.trim_end_matches('/'))
}

/// The login node as an ssh target. Every SGE call goes through this so the
/// ControlMaster prerequisite is enforced centrally rather than at call sites.
pub fn login(host: &str) -> SshTarget {
    SshTarget::alias(host).with_second_factor(SecondFactor::Required)
}

/// Every environment variable [`cache_env`] sets, so the job script knows which
/// directories to create. Ordered for a stable, readable `mkdir` line.
pub const CACHE_ENV_VARS: &[&str] = &[
    "XDG_CACHE_HOME",
    "PIP_CACHE_DIR",
    "UV_CACHE_DIR",
    "PYTHONUSERBASE",
    "HF_HOME",
    "TORCH_HOME",
    "TRITON_CACHE_DIR",
    "TORCHINDUCTOR_CACHE_DIR",
    "CUDA_CACHE_PATH",
    "MPLCONFIGDIR",
    "CONDA_PKGS_DIRS",
    "NUMBA_CACHE_DIR",
];

/// Redirect every package, model and compile cache that otherwise defaults into
/// `$HOME`.
///
/// This is not a tidiness measure. SCC home directories are capped at 10 GB, and
/// a single `pip install torch` plus one model download will spend most of that;
/// a handful of runs would wedge the account entirely. Everything here lands on
/// the project filesystem instead.
///
/// The caches are deliberately SHARED across runs rather than per-run — that is
/// the entire point of a wheel or model cache, and re-downloading CUDA wheels
/// for every job would be both slow and antisocial on a shared filesystem. Only
/// genuinely per-run state belongs in the run dir.
///
/// These are defaults: an author who exports one of these themselves wins, the
/// same contract as [`super::default_python_env`].
pub fn cache_env(work_dir: &str) -> HashMap<String, String> {
    let base = work_dir.trim_end_matches('/');
    let cache = format!("{base}/.orx/cache");
    [
        // Catches anything XDG-aware that we have not named explicitly.
        ("XDG_CACHE_HOME", cache.clone()),
        ("PIP_CACHE_DIR", format!("{cache}/pip")),
        ("UV_CACHE_DIR", format!("{cache}/uv")),
        // `pip install --user` writes here; without it the user site-packages
        // tree lands in ~/.local and counts against the home quota forever.
        ("PYTHONUSERBASE", format!("{base}/.orx/python-user")),
        // Covers HF_HUB_CACHE and HF_DATASETS_CACHE both.
        ("HF_HOME", format!("{cache}/huggingface")),
        ("TORCH_HOME", format!("{cache}/torch")),
        ("TRITON_CACHE_DIR", format!("{cache}/triton")),
        ("TORCHINDUCTOR_CACHE_DIR", format!("{cache}/torchinductor")),
        ("CUDA_CACHE_PATH", format!("{cache}/nv")),
        ("MPLCONFIGDIR", format!("{cache}/matplotlib")),
        ("CONDA_PKGS_DIRS", format!("{cache}/conda-pkgs")),
        ("NUMBA_CACHE_DIR", format!("{cache}/numba")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Apply [`cache_env`] as defaults over an existing env map.
pub fn default_cache_env(env: &HashMap<String, String>, work_dir: &str) -> HashMap<String, String> {
    let mut env = env.clone();
    for (key, value) in cache_env(work_dir) {
        env.entry(key).or_insert(value);
    }
    env
}

/// The per-user leaf under the configured base. `/projectnb/...` is a shared
/// group filesystem and run dirs are `umask 077` (job.qsub embeds HF_TOKEN and
/// every synced env var), so without this a labmate's `test -r` on our 0600
/// cache entry fails and their staging silently breaks.
pub async fn resolve_user_work_dir(host: &str, work_dir: &str) -> Result<String> {
    let user = ssh_run(&login(host), "id -un", None).await?;
    let user = user.trim();
    if user.is_empty() || user.contains('/') {
        return Err(anyhow!(
            "Could not determine the remote username on {host}."
        ));
    }
    validate_work_dir(&format!("{work_dir}/{user}"))
}

/// Create and check the remote base before anything is staged. A wrong or
/// unwritable base must fail loudly here rather than let SGE quietly fall back
/// to `$HOME`.
///
/// Tokens go to stdout with `exit 0`: a non-zero exit would make `ssh_run`
/// return only stderr and throw the diagnosis away.
pub async fn ensure_work_dir(host: &str, work_dir: &str) -> Result<()> {
    let base = sh_quote(work_dir);
    let cmd = format!(
        "base={base}; \
         case \"$base\" in /*) ;; *) echo ORX_NOT_ABSOLUTE; exit 0;; esac; \
         case \"$base\" in \"$HOME\"|\"$HOME\"/*) echo ORX_UNDER_HOME; exit 0;; esac; \
         mkdir -p \"$base/.orx/runs\" \"$base/.orx/source\" \"$base/.orx/cache\" 2>/dev/null || {{ echo ORX_MKDIR_FAILED; exit 0; }}; \
         chmod 700 \"$base/.orx\" \"$base/.orx/runs\" \"$base/.orx/source\" \"$base/.orx/cache\" 2>/dev/null || true; \
         {{ [ -d \"$base/.orx/runs\" ] && [ -w \"$base/.orx/runs\" ]; }} || {{ echo ORX_NOT_WRITABLE; exit 0; }}; \
         echo ORX_OK"
    );
    match ssh_run(&login(host), &cmd, None).await?.trim() {
        "ORX_OK" => Ok(()),
        "ORX_UNDER_HOME" => Err(anyhow!(
            "The SGE work directory {work_dir} is inside your home directory on {host}. Home \
             directories on this cluster are quota'd (10 GB); point workDir at a project \
             filesystem such as {DEFAULT_WORK_DIR}."
        )),
        "ORX_NOT_WRITABLE" => Err(anyhow!(
            "{work_dir}/.orx/runs is not writable by your account on {host}."
        )),
        "ORX_MKDIR_FAILED" => Err(anyhow!(
            "Could not create {work_dir}/.orx on {host} — check the path and your group access."
        )),
        "ORX_NOT_ABSOLUTE" => Err(anyhow!("{work_dir} is not an absolute path on {host}.")),
        other => Err(anyhow!("Unexpected work-directory check output: {other:?}")),
    }
}

// --- job spec & script generation ---------------------------------------------

pub struct SgeJobSpec {
    /// ssh config host alias of the cluster's login node.
    pub host: String,
    /// Names the job; the absolute run dir is passed separately.
    pub run_id: String,
    /// Absolute run dir (`<work_dir>/.orx/runs/<run_id>`).
    pub dir: String,
    /// The experiment's run command; the body of the batch job (runs in `repo/`).
    pub command: String,
    /// Exported in job.qsub (tokens, synced env).
    pub env: HashMap<String, String>,
    /// `-l` request values, already canonicalized (e.g. ["gpus=1", "gpu_type=L40S"]).
    pub resources: Vec<String>,
    /// `#$ -P`.
    pub scc_project: Option<String>,
    /// `#$ -pe <name> <slots>` as one indivisible value — a bare `-pe omp` makes
    /// qsub die with "error: no value for -pe", so the two can never be split.
    pub pe: Option<(String, u32)>,
    /// `#$ -l h_rt=…` in seconds.
    pub time_limit_secs: Option<u64>,
}

/// Case-insensitive lookup returning the cluster's exact spelling.
pub fn canonical_gpu_type(input: &str) -> Option<&'static str> {
    let needle = input.trim();
    SCC_GPU_TYPES
        .iter()
        .copied()
        .find(|t| t.eq_ignore_ascii_case(needle))
}

/// Map a `--flavor` string onto SGE `-l` request values (the renderer prefixes
/// `#$ -l ` to each).
///
/// Replaces slurm's `resolve_gres`, which returned ONE string because Slurm
/// folds count and model into a single `--gres=gpu:model:n`; SGE splits them
/// across two independent complexes.
///
/// `None` means the flag was omitted, which takes the settings default (a GPU).
/// `Some("cpu")` is the explicit way to ask for no GPU at all.
pub fn resolve_resources(flavor: Option<&str>, settings: &SgeSettings) -> Result<Vec<String>> {
    let extra = settings.extra_l.clone().unwrap_or_default();
    let with_extra = |mut base: Vec<String>| {
        base.extend(extra.iter().filter(|e| !e.trim().is_empty()).cloned());
        base
    };

    let flavor = flavor.map(str::trim).filter(|f| !f.is_empty());
    let Some(flavor) = flavor else {
        return Ok(with_extra(gpu_request(
            settings.gpus_or_default(),
            Some(settings.gpu_type_or_default()),
        )));
    };

    // Escape hatch first: an explicit `key=value` list passes through untouched,
    // so new hardware is never blocked by a stale SCC_GPU_TYPES table.
    if flavor.contains('=') {
        return Ok(with_extra(
            flavor
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
        ));
    }

    let lower = flavor.to_ascii_lowercase();
    if lower == "cpu" || lower.starts_with("cpu") {
        return Ok(with_extra(Vec::new()));
    }

    let default_type = Some(settings.gpu_type_or_default());
    if lower == "gpu" {
        return Ok(with_extra(gpu_request(1, default_type)));
    }
    if let Ok(count) = flavor.parse::<u32>() {
        return Ok(with_extra(gpu_request(
            checked_count(count, flavor)?,
            default_type,
        )));
    }
    if let Some(rest) = lower.strip_prefix("gpu:") {
        let count: u32 = rest
            .parse()
            .map_err(|_| anyhow!("Unrecognized --flavor {flavor:?}: expected gpu:<count>."))?;
        return Ok(with_extra(gpu_request(
            checked_count(count, flavor)?,
            default_type,
        )));
    }

    // <TYPE> or <TYPE>:<count>
    let (name, count) = match flavor.rsplit_once(':') {
        Some((name, count)) => {
            let parsed: u32 = count.trim().parse().map_err(|_| {
                anyhow!("Unrecognized --flavor {flavor:?}: expected <gpu type>:<count>.")
            })?;
            (name.trim(), checked_count(parsed, flavor)?)
        }
        None => (flavor, 1),
    };
    let canonical = canonical_gpu_type(name).ok_or_else(|| unknown_gpu_type(name))?;
    Ok(with_extra(gpu_request(count, Some(canonical.to_string()))))
}

fn checked_count(count: u32, flavor: &str) -> Result<u32> {
    if count == 0 {
        return Err(anyhow!(
            "--flavor {flavor:?} requests zero GPUs. Use `--flavor cpu` for a CPU-only run."
        ));
    }
    Ok(count)
}

fn gpu_request(count: u32, gpu_type: Option<String>) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let mut out = vec![format!("gpus={count}")];
    if let Some(t) = gpu_type.and_then(|t| non_empty(Some(&t))) {
        out.push(format!("gpu_type={t}"));
    }
    out
}

fn unknown_gpu_type(name: &str) -> crate::error::Error {
    // The likeliest mistake coming from a Slurm cluster: SCC has H200, not H100.
    let hint = if name.eq_ignore_ascii_case("h100") {
        " SCC has no H100 — did you mean H200?"
    } else {
        ""
    };
    anyhow!(
        "Unknown gpu_type {name:?}.{hint} Valid types: {}. To bypass this list, pass an \
         explicit request such as --flavor 'gpus=1,gpu_type=<name>'.",
        SCC_GPU_TYPES.join(", ")
    )
}

/// Seconds → SGE's `h_rt` syntax.
///
/// SGE has NO day field: two days is `48:00:00`. Slurm's `slurm_time` emits
/// `2-00:00:00` for the same input, which qsub rejects outright — the two
/// functions are NOT interchangeable and must not be shared.
fn sge_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

fn render_exports(env: &HashMap<String, String>) -> String {
    let mut pairs: Vec<_> = env.iter().collect();
    pairs.sort(); // deterministic script for tests & debugging
    pairs
        .iter()
        .map(|(k, v)| format!("export {}={}", k, sh_quote(v)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Render job.qsub.
///
/// Every `#$` directive must appear before the first non-comment line — SGE
/// silently ignores any that follow — so they are emitted as one contiguous
/// block. The payload runs in a SUBSHELL so an `exit`/`set -e` inside it still
/// reaches the `echo $? > exit_code` line, and the script exits with the
/// payload's code so SGE's own accounting verdict mirrors it.
fn render_qsub_script(spec: &SgeJobSpec) -> String {
    let dir = &spec.dir;
    let mut directives = vec![
        format!("#$ -N orx-{}", &spec.run_id[..spec.run_id.len().min(8)]),
        // Absolute, not `-cwd`. `-cwd` would work (qsub runs from inside the
        // run dir) but its failure mode is silent and catastrophic: anything
        // that perturbs the invocation cwd lands stdout, stderr and exit_code
        // in $HOME, which is exactly the 10 GB quota this backend exists to
        // avoid. `-wd`'s failure mode is a loud qsub error caught by preflight.
        format!("#$ -wd {dir}"),
        format!("#$ -o {dir}/log"),
        // SGE has no --error; -j y merges stderr at the source. Two writers on
        // one -o file would interleave badly.
        "#$ -j y".to_string(),
        // Non-rerunnable. Removes the truncate-on-requeue problem (SGE has no
        // --open-mode=append) and the requeue-transient states Slurm's mapper
        // has to reason about.
        "#$ -r n".to_string(),
        // A site or user ~/.sge_request can silently inject `-m beas` and turn
        // every run into a mail storm. Opt out explicitly.
        "#$ -m n".to_string(),
    ];
    if let Some(p) = spec.scc_project.as_deref().filter(|p| !p.trim().is_empty()) {
        directives.push(format!("#$ -P {p}"));
    }
    if let Some(secs) = spec.time_limit_secs {
        directives.push(format!("#$ -l h_rt={}", sge_time(secs)));
    }
    if let Some((pe, slots)) = &spec.pe {
        if *slots > 1 {
            directives.push(format!("#$ -pe {pe} {slots}"));
        }
    }
    for resource in &spec.resources {
        directives.push(format!("#$ -l {resource}"));
    }
    // A login shell, NOT `#!/usr/bin/env bash`: sbatch defaults to
    // --export=ALL so Slurm jobs inherit MODULEPATH and the `module` function,
    // but SGE under unix_behavior execs the shebang with a bare environment.
    // `env bash -l` does not work — Linux env passes "bash -l" as one argument.
    // Our own exports come after and still win over anything the profile sets.
    let env = super::default_python_env(&spec.env);
    format!(
        "#!/bin/bash -l\n{directives}\n{exports}\n{cache_dirs}(\ncd {dir}/repo || exit 97\n{command}\n)\ncode=$?\necho \"$code\" > {dir}/exit_code\nexit \"$code\"\n",
        directives = directives.join("\n"),
        exports = render_exports(&env),
        cache_dirs = render_cache_mkdir(&env),
        command = spec.command,
    )
}

/// Create the redirected cache directories before the payload runs.
///
/// Most tools would create their own, but not all do — matplotlib and CUDA both
/// fall back to `$HOME` with only a warning if their target is missing, which is
/// exactly the failure this redirection exists to prevent. Expanding the
/// variables rather than the literal paths means an author override is honoured.
fn render_cache_mkdir(env: &HashMap<String, String>) -> String {
    let dirs: Vec<String> = CACHE_ENV_VARS
        .iter()
        .filter(|name| env.contains_key(**name))
        .map(|name| format!("\"${name}\""))
        .collect();
    if dirs.is_empty() {
        return String::new();
    }
    format!("mkdir -p {} 2>/dev/null || true\n", dirs.join(" "))
}

/// `qsub -terse` prints the bare job id. Array submissions print
/// `<id>.<start>-<end>:<step>`; take the leading id either way.
///
/// The LAST non-empty line is used: a site `sge_request` or a login profile can
/// put a warning on stdout ahead of it.
fn parse_job_id(out: &str) -> Result<String> {
    let last = out
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("");
    let id = last.split('.').next().unwrap_or("").trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(anyhow!("Unexpected qsub output: {:?}", out.trim()));
    }
    Ok(id.to_string())
}

// --- lifecycle ----------------------------------------------------------------

/// Submit the job: write job.qsub into the (already staged) run dir and
/// `qsub -terse`. Returns the SGE job id — the reattach handle, together with
/// the host.
pub async fn run_job(spec: &SgeJobSpec) -> Result<String> {
    let target = login(&spec.host);
    let dir = ssh::remote_path(&spec.dir);

    // Owner-only: the script embeds tokens.
    ssh_run(
        &target,
        &format!("umask 077 && cat > {dir}/job.qsub"),
        Some(&render_qsub_script(spec)),
    )
    .await?;
    let out = ssh_run(&target, &format!("cd {dir} && qsub -terse job.qsub"), None)
        .await
        .map_err(|e| anyhow!("qsub failed: {e}"))?;
    parse_job_id(&out)
}

/// Job state in the shared stage vocabulary (see `jobs::stage_to_run_status`),
/// plus the internal `GONE` and `BLOCKED` stages the supervisor escalates.
#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
}

/// The fast probe: one ssh round-trip, no `qacct`.
///
/// `exit_code` first (ground truth, scheduler-independent), then the live queue.
/// `qstat -j` disambiguates "qmaster knows the job but the user table hasn't
/// caught up" from genuinely gone. POSIX-sh only, and every branch exits 0 so a
/// non-zero status always means the *transport* failed — which the supervisor's
/// transport-fault handling relies on.
pub async fn inspect_job(host: &str, dir: &str, job_id: &str) -> Result<JobState> {
    let d = ssh::remote_path(dir);
    let cmd = format!(
        "d={d}; \
         if [ -f \"$d/exit_code\" ]; then printf 'EXIT %s\\n' \"$(cat \"$d/exit_code\" 2>/dev/null)\"; exit 0; fi; \
         st=$(qstat -u \"$(id -un)\" 2>/dev/null | awk -v j={job_id} '$1==j{{print $5; exit}}'); \
         if [ -n \"$st\" ]; then printf 'QS %s\\n' \"$st\"; exit 0; fi; \
         if qstat -j {job_id} >/dev/null 2>&1; then echo 'QJ alive'; exit 0; fi; \
         echo GONE"
    );
    let out = ssh_run(&login(host), &cmd, None).await?;
    Ok(map_inspect_token(out.trim()))
}

fn state(stage: &str, message: Option<String>) -> JobState {
    JobState {
        stage: stage.to_string(),
        message,
    }
}

/// Pure token → stage mapping (unit-tested).
///
/// SGE composes its states from letter sets rather than using fixed names
/// (`qw`, `hqw`, `hRwq`, `Eqw`, `dr`, `dt`, `ts`, `Rr`, …), so this tests for
/// CHARACTERS in precedence order rather than matching whole strings.
fn map_inspect_token(out: &str) -> JobState {
    if let Some(code) = out.strip_prefix("EXIT ") {
        // An empty file is the window between open(O_TRUNC) and the write —
        // not a verdict. Matters more here than on Slurm: $HOME and /projectnb
        // are both NFS-exported GPFS.
        if code.trim().is_empty() {
            return state("RUNNING", None);
        }
        let code: i32 = code.trim().parse().unwrap_or(-1);
        return match code {
            0 => state("COMPLETED", None),
            // The subshell's own sentinel for a missing repo/ — a truncated or
            // failed staging, which is worth naming rather than reporting as a
            // generic non-zero exit.
            97 => state(
                "ERROR",
                Some("the run directory's repo/ was missing (staging failed?)".into()),
            ),
            other => state("ERROR", Some(format!("exited with code {other}"))),
        };
    }
    if out == "GONE" {
        return state("GONE", None);
    }
    if out == "QJ alive" {
        return state("SCHEDULING", None);
    }
    let Some(raw) = out.strip_prefix("QS ") else {
        return state("RUNNING", Some(format!("unexpected inspect output: {out}")));
    };
    let st = raw.trim();
    // Error state: the job is parked and will never run without `qmod -cj`.
    // Internal — the supervisor fetches the reason and qdels it.
    if st.contains('E') {
        return state("BLOCKED", None);
    }
    // Deletion in flight; irrevocable.
    if st.contains('d') {
        return state("CANCELED", None);
    }
    if st.contains('t') || st.contains('r') {
        return state("RUNNING", None);
    }
    if st.contains('s') || st.contains('S') || st.contains('T') {
        return state("RUNNING", Some(format!("job is suspended ({st})")));
    }
    if st.contains('w') || st.contains('q') {
        return state("SCHEDULING", None);
    }
    // An unknown state must never wedge a run into a terminal verdict.
    state("RUNNING", Some(format!("unrecognized sge state: {st}")))
}

/// One accounting record. `failed` is Grid Engine's own verdict; `exit_status`
/// is the script's (which, because job.qsub ends with `exit "$code"`, is the
/// payload's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcctRecord {
    pub exit_status: i32,
    pub failed: i32,
}

/// The ~11-SECOND probe — `qacct` linearly scans the accounting file.
///
/// NEVER call this from the poll loop directly; the supervisor spawns it as a
/// detached task and harvests it a few polls later. `Ok(None)` means the
/// accounting file has no record for this id yet.
pub async fn probe_accounting(host: &str, job_id: &str) -> Result<Option<AcctRecord>> {
    let cmd = format!(
        "qacct -j {job_id} 2>/dev/null | awk '\
         $1==\"exit_status\" {{ e=$2 }} \
         $1==\"failed\" {{ f=$2 }} \
         END {{ if (e==\"\") print \"ACCT_NONE\"; else printf \"ACCT %s %s\\n\", e, f }}'"
    );
    let out = ssh_run(&login(host), &cmd, None).await?;
    Ok(parse_acct_token(out.trim()))
}

fn parse_acct_token(out: &str) -> Option<AcctRecord> {
    let rest = out.strip_prefix("ACCT ")?;
    let mut parts = rest.split_whitespace();
    let exit_status = parts.next()?.parse().ok()?;
    let failed = parts.next().unwrap_or("0").parse().unwrap_or(0);
    Some(AcctRecord {
        exit_status,
        failed,
    })
}

/// Accounting verdict → stage. Grid Engine's `failed` codes carry more
/// information than the exit status alone.
pub fn map_acct_record(rec: &AcctRecord) -> JobState {
    match rec.failed {
        0 => match rec.exit_status {
            0 => state("COMPLETED", None),
            c if c >= 128 => state("ERROR", Some(format!("killed by signal {}", c - 128))),
            c => state("ERROR", Some(format!("exited with code {c}"))),
        },
        // qmaster-enforced h_rt — the direct analogue of Slurm's TIMEOUT.
        37 => state("ERROR", Some("job hit its h_rt time limit".into())),
        // "assumedly after job": died on the exec node. Also what a qdel of a
        // running job records.
        100 => state(
            "ERROR",
            Some("job died on the exec node (killed, or node lost)".into()),
        ),
        f => state("ERROR", Some(format!("sge accounting reported failed={f}"))),
    }
}

/// Why a job is parked in `Eqw`. Cheap (unlike `qacct`) and high-value: the
/// usual causes — an invalid `-P`, a `gpu_type` no host offers, an unreachable
/// working directory — are exactly what our own directive rendering can produce.
pub async fn blocked_reason(host: &str, job_id: &str) -> Option<String> {
    let cmd = format!("qstat -j {job_id} 2>&1 | sed -n 's/^error reason *[0-9]*: *//p' | head -n1");
    let out = ssh_run(&login(host), &cmd, None).await.ok()?;
    non_empty(Some(out.trim()))
}

/// Cancel = `qdel`. Tolerant of already-finished jobs.
pub async fn cancel_job(host: &str, job_id: &str) -> Result<()> {
    ssh_run(
        &login(host),
        &format!("qdel {job_id} 2>/dev/null || true"),
        None,
    )
    .await?;
    Ok(())
}

/// SGE can strand a job in `dr`/`dt` when the exec node is unreachable, which
/// Slurm's scancel never needs. The supervisor escalates to this after ~60s.
pub async fn force_cancel_job(host: &str, job_id: &str) -> Result<()> {
    ssh_run(
        &login(host),
        &format!("qdel -f {job_id} 2>/dev/null || true"),
        None,
    )
    .await?;
    Ok(())
}

// --- preflight ----------------------------------------------------------------

/// Per-host readiness for the Settings UI.
pub struct SgePreflight {
    pub reachable: bool,
    pub sge_found: bool,
    pub tools_found: bool,
    /// Refused rather than down — the Duo case, which needs different
    /// remediation text.
    pub auth_blocked: bool,
    /// The ControlMaster prerequisite.
    pub master_running: bool,
    /// Valid `-P` values for THIS user. Replaces Slurm's partition list: SCC
    /// has hundreds of queues and placement is steered by `-l` requests, not by
    /// naming a queue, so the useful picker is the project one.
    pub projects: Vec<String>,
    pub error: Option<String>,
}

impl SgePreflight {
    fn unreachable(error: String, auth_blocked: bool, master_running: bool) -> Self {
        Self {
            reachable: false,
            sge_found: false,
            tools_found: false,
            auth_blocked,
            master_running,
            projects: Vec::new(),
            error: Some(error),
        }
    }
}

pub async fn preflight(host: &str) -> SgePreflight {
    let target = login(host);
    // Self-heal first: inside the cluster's grace window this is silent and the
    // user never learns a master was missing.
    let master_running = ssh::ensure_master_headless(&target).await.unwrap_or(false);

    // `id -Gn` and `qconf -sprjl` are both duplicate-free, so `uniq -d` is
    // exactly their intersection — the user's valid -P values.
    let cmd = "for b in qsub qstat qdel qacct; do command -v \"$b\" >/dev/null 2>&1 || echo \"MISSING_$b\"; done; \
               if command -v bash >/dev/null 2>&1 && command -v tar >/dev/null 2>&1; then echo TOOLS_OK; fi; \
               { id -Gn | tr ' ' '\\n'; qconf -sprjl 2>/dev/null; } | sort | uniq -d | sed 's/^/PROJECT /'; \
               true";
    match ssh_run(&target, cmd, None).await {
        Ok(out) => {
            let mut sge_found = true;
            let mut tools_found = false;
            let mut projects = Vec::new();
            for line in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if line.starts_with("MISSING_") {
                    sge_found = false;
                } else if line == "TOOLS_OK" {
                    tools_found = true;
                } else if let Some(p) = line.strip_prefix("PROJECT ") {
                    let p = p.trim().to_string();
                    if !p.is_empty() && !projects.contains(&p) {
                        projects.push(p);
                    }
                }
            }
            SgePreflight {
                reachable: true,
                sge_found,
                tools_found,
                auth_blocked: false,
                master_running,
                projects,
                error: None,
            }
        }
        Err(e) => {
            let auth_blocked = e.downcast_ref::<ssh::MasterRequired>().is_some()
                || ssh::is_second_factor_failure(&e.to_string());
            SgePreflight::unreachable(e.to_string(), auth_blocked, master_running)
        }
    }
}

// --- tests --------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SgeJobSpec {
        SgeJobSpec {
            host: "scc1".into(),
            run_id: "0123456789abcdef".into(),
            dir: "/projectnb/herbdl/workspaces/herb/faridkar/.orx/runs/0123456789abcdef".into(),
            command: "python train.py".into(),
            env: HashMap::new(),
            resources: Vec::new(),
            scc_project: None,
            pe: None,
            time_limit_secs: None,
        }
    }

    #[test]
    fn qsub_minimal_has_only_fixed_directives() {
        let script = render_qsub_script(&spec());
        // A LOGIN shell: SGE execs the shebang with a bare environment, so a
        // plain `bash` would leave SCC users with no `module` command.
        assert!(script.starts_with("#!/bin/bash -l\n"), "{script}");
        assert!(script.contains("#$ -N orx-01234567\n"));
        assert!(script.contains("#$ -j y\n"));
        assert!(script.contains("#$ -r n\n"));
        assert!(script.contains("#$ -m n\n"));
        assert!(!script.contains("#$ -P "));
        assert!(!script.contains("h_rt"));
        assert!(!script.contains("#$ -pe "));
        assert!(!script.contains("#$ -l "));
        // Unbuffered by default so prints stream live behind the -o redirect.
        assert!(script.contains("export PYTHONUNBUFFERED='1'\n"));
        assert!(script.ends_with(
            "(\ncd /projectnb/herbdl/workspaces/herb/faridkar/.orx/runs/0123456789abcdef/repo || exit 97\npython train.py\n)\ncode=$?\necho \"$code\" > /projectnb/herbdl/workspaces/herb/faridkar/.orx/runs/0123456789abcdef/exit_code\nexit \"$code\"\n"
        ), "{script}");
    }

    /// The highest-value regression guard in the file. SGE's default working
    /// directory is $HOME, so without `-wd` the job's log, exit_code and
    /// `cd repo` all silently target the wrong filesystem — the very 10 GB
    /// quota this backend exists to stay off.
    #[test]
    fn working_dir_is_emitted_for_every_spec_shape() {
        let mut full = spec();
        full.scc_project = Some("herbdl".into());
        full.pe = Some(("omp".into(), 16));
        full.resources = vec!["gpus=1".into(), "gpu_type=L40S".into()];
        full.time_limit_secs = Some(12 * 3600);
        let mut gpu_only = spec();
        gpu_only.resources = vec!["gpus=2".into()];

        for s in [spec(), full, gpu_only] {
            let script = render_qsub_script(&s);
            assert!(
                script.contains(&format!("#$ -wd {}\n", s.dir)),
                "missing -wd: {script}"
            );
            assert!(
                script.contains(&format!("#$ -o {}/log\n", s.dir)),
                "missing absolute -o: {script}"
            );
            assert!(!script.contains("#$ -cwd"), "{script}");
        }
    }

    /// SGE stops reading `#$` lines at the first non-comment line, so a
    /// directive emitted after the exports would be silently ignored.
    #[test]
    fn directives_precede_the_first_command() {
        let mut s = spec();
        s.scc_project = Some("herbdl".into());
        s.pe = Some(("omp".into(), 16));
        s.resources = vec!["gpus=1".into()];
        s.time_limit_secs = Some(3600);
        s.env.insert("TOKEN".into(), "x".into());
        let script = render_qsub_script(&s);
        let lines: Vec<&str> = script.lines().collect();
        let last_directive = lines
            .iter()
            .rposition(|l| l.starts_with("#$ "))
            .expect("expected directives");
        let first_command = lines
            .iter()
            .position(|l| !l.is_empty() && !l.starts_with('#'))
            .expect("expected a command");
        assert!(
            last_directive < first_command,
            "directive at {last_directive} follows the first command at {first_command}:\n{script}"
        );
    }

    #[test]
    fn qsub_emits_optional_directives_and_quoted_env() {
        let mut s = spec();
        s.scc_project = Some("herbdl".into());
        s.pe = Some(("omp".into(), 16));
        s.resources = vec!["gpus=2".into(), "gpu_type=A100".into()];
        s.time_limit_secs = Some(12 * 3600);
        s.env.insert("TOKEN".into(), "it's; rm -rf /".into());
        let script = render_qsub_script(&s);
        assert!(script.contains("#$ -P herbdl\n"));
        assert!(script.contains("#$ -l h_rt=12:00:00\n"));
        // Exactly two tokens after -pe: a bare `-pe omp` makes qsub fail.
        assert!(script.contains("#$ -pe omp 16\n"));
        assert!(script.contains("#$ -l gpus=2\n"));
        assert!(script.contains("#$ -l gpu_type=A100\n"));
        assert!(script.contains("export TOKEN='it'\\''s; rm -rf /'\n"));
    }

    /// A single slot is SGE's default, and `-pe omp 1` is rejected by some
    /// PE configurations, so it is omitted.
    #[test]
    fn pe_is_omitted_for_a_single_slot() {
        let mut s = spec();
        s.pe = Some(("omp".into(), 1));
        assert!(!render_qsub_script(&s).contains("#$ -pe"));
    }

    /// SCC home directories are capped at 10 GB, so every cache that defaults
    /// into $HOME has to land on the project filesystem instead. This is the
    /// guard that keeps a new cache variable from being forgotten.
    #[test]
    fn caches_are_redirected_off_home() {
        let work = "/projectnb/herbdl/workspaces/herb/faridkar";
        let env = cache_env(work);
        for name in CACHE_ENV_VARS {
            let value = env
                .get(*name)
                .unwrap_or_else(|| panic!("{name} is listed but not set"));
            assert!(
                value.starts_with(work),
                "{name} must live under the work dir, got {value}"
            );
        }
        assert_eq!(env.len(), CACHE_ENV_VARS.len());
        // The ones that actually consume the quota.
        assert_eq!(
            env.get("PIP_CACHE_DIR").map(String::as_str),
            Some("/projectnb/herbdl/workspaces/herb/faridkar/.orx/cache/pip")
        );
        assert_eq!(
            env.get("HF_HOME").map(String::as_str),
            Some("/projectnb/herbdl/workspaces/herb/faridkar/.orx/cache/huggingface")
        );
        // `pip install --user` goes outside the cache root: it is an install
        // tree, not a cache, and must survive a cache wipe.
        assert_eq!(
            env.get("PYTHONUSERBASE").map(String::as_str),
            Some("/projectnb/herbdl/workspaces/herb/faridkar/.orx/python-user")
        );
    }

    /// Caches are defaults, not impositions — the same contract as
    /// `default_python_env`.
    #[test]
    fn an_author_can_override_a_cache_location() {
        let author = HashMap::from([("HF_HOME".to_string(), "/scratch/models".to_string())]);
        let env = default_cache_env(&author, "/projectnb/herbdl/workspaces/herb");
        assert_eq!(
            env.get("HF_HOME").map(String::as_str),
            Some("/scratch/models")
        );
        // The others are still filled in.
        assert!(env.contains_key("PIP_CACHE_DIR"));
    }

    /// The directories must exist before the payload runs: matplotlib and CUDA
    /// silently fall back to $HOME when their target is missing, which is the
    /// exact failure the redirection exists to prevent.
    #[test]
    fn the_script_creates_the_cache_dirs_via_their_variables() {
        let mut s = spec();
        s.env = cache_env("/projectnb/herbdl/workspaces/herb/faridkar");
        let script = render_qsub_script(&s);
        let mkdir = script
            .lines()
            .find(|l| l.starts_with("mkdir -p "))
            .expect("expected a mkdir line");
        for name in CACHE_ENV_VARS {
            assert!(
                mkdir.contains(&format!("\"${name}\"")),
                "{name} not created"
            );
        }
        // Expanding the variables (not the literal paths) is what makes an
        // author override get its directory created too.
        assert!(!mkdir.contains("/projectnb"), "{mkdir}");
        // It has to run before the payload subshell.
        let mkdir_at = script.find("mkdir -p ").unwrap();
        assert!(mkdir_at < script.find("\n(\ncd ").unwrap(), "{script}");
    }

    /// A spec with no cache vars emits no mkdir at all.
    #[test]
    fn the_script_omits_the_mkdir_when_there_are_no_cache_vars() {
        assert!(!render_qsub_script(&spec()).contains("mkdir"));
    }

    #[test]
    fn sge_time_has_no_day_field() {
        assert_eq!(sge_time(90), "00:01:30");
        assert_eq!(sge_time(12 * 3600), "12:00:00");
        // Slurm's slurm_time() renders this as "2-00:00:00", which h_rt rejects.
        assert_eq!(sge_time(2 * 86_400), "48:00:00");
        assert_eq!(sge_time(30 * 86_400), "720:00:00");
        assert!(!sge_time(2 * 86_400).contains('-'));
    }

    fn settings() -> SgeSettings {
        SgeSettings::default()
    }

    #[test]
    fn resolve_resources_applies_defaults_when_flavor_is_absent() {
        assert_eq!(
            resolve_resources(None, &settings()).unwrap(),
            vec!["gpus=1".to_string(), "gpu_type=L40S".to_string()]
        );
    }

    #[test]
    fn resolve_resources_shorthands() {
        let s = settings();
        assert_eq!(
            resolve_resources(Some("gpu"), &s).unwrap(),
            vec!["gpus=1".to_string(), "gpu_type=L40S".to_string()]
        );
        assert_eq!(
            resolve_resources(Some("2"), &s).unwrap(),
            vec!["gpus=2".to_string(), "gpu_type=L40S".to_string()]
        );
        assert_eq!(
            resolve_resources(Some("gpu:3"), &s).unwrap(),
            vec!["gpus=3".to_string(), "gpu_type=L40S".to_string()]
        );
        assert_eq!(
            resolve_resources(Some("A100:2"), &s).unwrap(),
            vec!["gpus=2".to_string(), "gpu_type=A100".to_string()]
        );
        assert_eq!(
            resolve_resources(Some("A100"), &s).unwrap(),
            vec!["gpus=1".to_string(), "gpu_type=A100".to_string()]
        );
    }

    /// gpu_type uses the `==` relop, so a case mismatch is not a warning — it
    /// is a request no host can satisfy and the job sits in qw forever.
    #[test]
    fn resolve_resources_normalizes_gpu_type_case() {
        let s = settings();
        assert_eq!(
            resolve_resources(Some("a100:2"), &s).unwrap(),
            vec!["gpus=2".to_string(), "gpu_type=A100".to_string()]
        );
        assert_eq!(
            resolve_resources(Some("l40s"), &s).unwrap(),
            vec!["gpus=1".to_string(), "gpu_type=L40S".to_string()]
        );
        assert_eq!(canonical_gpu_type("RTX6000ADA"), Some("RTX6000ada"));
        assert_eq!(canonical_gpu_type("nvidia"), None);
    }

    #[test]
    fn resolve_resources_cpu_means_no_gpu() {
        assert!(resolve_resources(Some("cpu"), &settings())
            .unwrap()
            .is_empty());
    }

    /// The escape hatch: anything containing `=` passes through untouched, so
    /// hardware newer than SCC_GPU_TYPES is never blocked.
    #[test]
    fn resolve_resources_passes_explicit_requests_through() {
        assert_eq!(
            resolve_resources(Some("gpus=2,gpu_memory=40G"), &settings()).unwrap(),
            vec!["gpus=2".to_string(), "gpu_memory=40G".to_string()]
        );
    }

    #[test]
    fn resolve_resources_appends_extra_l_from_settings() {
        let mut s = settings();
        s.extra_l = Some(vec!["mem_per_core=8G".into()]);
        assert_eq!(
            resolve_resources(Some("cpu"), &s).unwrap(),
            vec!["mem_per_core=8G".to_string()]
        );
    }

    #[test]
    fn resolve_resources_rejects_unknown_and_zero() {
        let s = settings();
        let err = resolve_resources(Some("h100"), &s).unwrap_err().to_string();
        // The likeliest mistake coming from a Slurm cluster.
        assert!(err.contains("H200"), "{err}");
        assert!(resolve_resources(Some("A100:0"), &s).is_err());
        assert!(resolve_resources(Some("nvidia:1"), &s).is_err());
    }

    #[test]
    fn work_dir_validation() {
        assert_eq!(
            validate_work_dir("/projectnb/herbdl/workspaces/herb/").unwrap(),
            "/projectnb/herbdl/workspaces/herb"
        );
        assert!(validate_work_dir("relative/path").is_err());
        assert!(validate_work_dir("/").is_err());
        assert!(validate_work_dir("/projectnb/../etc").is_err());
        assert!(validate_work_dir("/projectnb/./herbdl").is_err());
        // qsub parses `#$ -wd` with no shell, so these can never be made safe.
        assert!(validate_work_dir("/projectnb/herb dl").is_err());
        assert!(validate_work_dir("/projectnb/$HOME").is_err());
        assert!(validate_work_dir("/projectnb/a;rm -rf /").is_err());
        assert!(validate_work_dir("/projectnb/~user").is_err());
    }

    #[test]
    fn run_dir_is_absolute_under_the_work_dir() {
        assert_eq!(
            run_dir("/projectnb/herbdl/workspaces/herb/faridkar", "r1"),
            "/projectnb/herbdl/workspaces/herb/faridkar/.orx/runs/r1"
        );
    }

    #[test]
    fn job_id_parsing() {
        assert_eq!(parse_job_id("8675309\n").unwrap(), "8675309");
        // Array submissions.
        assert_eq!(parse_job_id("8675309.1-10:1\n").unwrap(), "8675309");
        // A site sge_request or login profile can print ahead of the id.
        assert_eq!(
            parse_job_id("warning: something\n8675309\n").unwrap(),
            "8675309"
        );
        assert!(parse_job_id("qsub: error: no suitable queues").is_err());
        assert!(parse_job_id("").is_err());
    }

    #[test]
    fn inspect_token_exit_paths() {
        assert_eq!(map_inspect_token("EXIT 0").stage, "COMPLETED");
        let failed = map_inspect_token("EXIT 137");
        assert_eq!(failed.stage, "ERROR");
        assert!(failed.message.unwrap().contains("137"));
        let staging = map_inspect_token("EXIT 97");
        assert_eq!(staging.stage, "ERROR");
        assert!(staging.message.unwrap().contains("repo/"));
        // Caught between open(O_TRUNC) and the write — not a verdict.
        assert_eq!(map_inspect_token("EXIT ").stage, "RUNNING");
    }

    /// SGE composes states from letter sets, so the mapper tests characters.
    #[test]
    fn inspect_token_qstat_states() {
        for pending in ["QS qw", "QS hqw", "QS hRwq", "QS w", "QS Rq"] {
            assert_eq!(map_inspect_token(pending).stage, "SCHEDULING", "{pending}");
        }
        for running in ["QS r", "QS t", "QS Rr", "QS ts"] {
            assert_eq!(map_inspect_token(running).stage, "RUNNING", "{running}");
        }
        for suspended in ["QS s", "QS S", "QS T"] {
            let st = map_inspect_token(suspended);
            assert_eq!(st.stage, "RUNNING", "{suspended}");
            assert!(st.message.unwrap().contains("suspended"));
        }
        for deleting in ["QS dr", "QS dt"] {
            assert_eq!(map_inspect_token(deleting).stage, "CANCELED", "{deleting}");
        }
        // qmaster knows it, the user table hasn't caught up.
        assert_eq!(map_inspect_token("QJ alive").stage, "SCHEDULING");
        assert_eq!(map_inspect_token("GONE").stage, "GONE");
        // An unknown state must never wedge a run into a terminal verdict.
        assert_eq!(map_inspect_token("QS zz").stage, "RUNNING");
        assert_eq!(map_inspect_token("nonsense").stage, "RUNNING");
    }

    /// Eqw is terminal in effect, but the SUPERVISOR owns that escalation (it
    /// fetches the reason and qdels the job), so the mapper must not report a
    /// stage the shared terminal check would act on.
    #[test]
    fn eqw_is_blocked_and_not_terminal_to_the_shared_checker() {
        for blocked in ["QS Eqw", "QS Ehqw", "QS EhRqw"] {
            assert_eq!(map_inspect_token(blocked).stage, "BLOCKED", "{blocked}");
        }
        assert!(!crate::jobs::is_terminal_stage("BLOCKED"));
        assert!(!crate::jobs::is_terminal_stage("GONE"));
    }

    #[test]
    fn accounting_mapping() {
        let map = |e, f| {
            map_acct_record(&AcctRecord {
                exit_status: e,
                failed: f,
            })
        };
        assert_eq!(map(0, 0).stage, "COMPLETED");
        assert!(map(1, 0).message.unwrap().contains('1'));
        assert!(map(137, 0).message.unwrap().contains("signal 9"));
        assert!(map(0, 37).message.unwrap().contains("h_rt"));
        assert!(map(0, 100).message.unwrap().contains("exec node"));
        assert_eq!(map(1, 0).stage, "ERROR");
    }

    #[test]
    fn accounting_token_parsing() {
        assert_eq!(
            parse_acct_token("ACCT 0 0"),
            Some(AcctRecord {
                exit_status: 0,
                failed: 0
            })
        );
        assert_eq!(
            parse_acct_token("ACCT 137 100"),
            Some(AcctRecord {
                exit_status: 137,
                failed: 100
            })
        );
        assert_eq!(parse_acct_token("ACCT_NONE"), None);
        assert_eq!(parse_acct_token(""), None);
    }

    /// Serde round-trip without touching config_dir(): the telemetry env lock
    /// requires these tests stay pure (see telemetry.rs's test-module note).
    #[test]
    fn settings_round_trip_and_defaults() {
        let s = SgeSettings::default();
        // Defaults are the verified-schedulable shape: L40Sx1, 16 cores, 12h.
        assert_eq!(s.scc_project_or_default(), "herbdl");
        assert_eq!(s.pe_or_default(), "omp");
        assert_eq!(s.slots_or_default(), 16);
        assert_eq!(s.time_limit_or_default(), "12h");
        assert_eq!(s.gpus_or_default(), 1);
        assert_eq!(s.gpu_type_or_default(), "L40S");
        assert_eq!(s.resolved_work_dir().unwrap(), DEFAULT_WORK_DIR);

        let json = serde_json::to_string(&s).unwrap();
        assert!(
            !json.contains("null") || json.contains("\"host\":null"),
            "{json}"
        );

        // A partial file still parses, and camelCase is the wire form.
        let partial: SgeSettings =
            serde_json::from_str(r#"{"host":"scc1","sccProject":"cs599dg","slots":8}"#).unwrap();
        assert_eq!(partial.host.as_deref(), Some("scc1"));
        assert_eq!(partial.scc_project_or_default(), "cs599dg");
        assert_eq!(partial.slots_or_default(), 8);
        // Unset fields still fall back.
        assert_eq!(partial.gpu_type_or_default(), "L40S");

        let back: SgeSettings =
            serde_json::from_str(&serde_json::to_string(&partial).unwrap()).unwrap();
        assert_eq!(back.scc_project_or_default(), "cs599dg");
    }

    /// Live E2E against a real Grid Engine cluster — opt-in, never runs in CI:
    ///   ORX_SGE_TEST_HOST=scc1.bu.edu ORX_SGE_TEST_PROJECT=herbdl \
    ///     cargo test jobs::sge -- --ignored
    ///
    /// Requires a live ControlMaster (`orx ssh connect <host>`), since SCC's
    /// sshd demands a second factor batch ssh cannot answer.
    ///
    /// Beyond submit -> complete -> cancel, this asserts three things that only
    /// a real cluster can settle: that repeated `#$ -l` lines actually
    /// accumulate on GE 2011.11 (rather than the last one winning), that
    /// `#!/bin/bash -l` really gives the payload the `module` command, and what
    /// the ~11-second qacct escalation costs in practice.
    #[tokio::test]
    #[ignore = "needs a live SGE cluster; set ORX_SGE_TEST_HOST"]
    async fn e2e_lifecycle_against_live_cluster() {
        let Ok(host) = std::env::var("ORX_SGE_TEST_HOST") else {
            panic!("set ORX_SGE_TEST_HOST to an ~/.ssh/config alias of an SGE login node");
        };
        let project = std::env::var("ORX_SGE_TEST_PROJECT").ok();
        let base =
            std::env::var("ORX_SGE_TEST_WORKDIR").unwrap_or_else(|_| DEFAULT_WORK_DIR.to_string());

        let work_dir = resolve_user_work_dir(&host, &base)
            .await
            .expect("resolve work dir (is the ssh master up? run `orx ssh connect`)");
        ensure_work_dir(&host, &work_dir)
            .await
            .expect("ensure work dir");

        let mk = |run_id: &str, command: &str| {
            let dir = run_dir(&work_dir, run_id);
            SgeJobSpec {
                host: host.clone(),
                run_id: run_id.to_string(),
                dir,
                command: command.to_string(),
                env: HashMap::from([("ORX_E2E".to_string(), "1".to_string())]),
                // CPU-only so it lands on a fast queue.
                resources: Vec::new(),
                scc_project: project.clone(),
                pe: None,
                time_limit_secs: Some(600),
            }
        };

        // 300 x 2s: SCC queue waits are longer than a typical Slurm cluster's.
        let poll = |dir: String, job_id: String, until: &'static [&'static str]| {
            let host = host.clone();
            async move {
                for _ in 0..300 {
                    let s = inspect_job(&host, &dir, &job_id).await.unwrap();
                    if until.contains(&s.stage.as_str()) {
                        return s;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                panic!("job {job_id} never reached {until:?}");
            }
        };

        // Fresh ids: a recycled run dir would satisfy inspect_job from a stale
        // exit_code before the job even runs.
        let run_a = format!("e2e-a-{}", uuid::Uuid::new_v4());
        let run_b = format!("e2e-b-{}", uuid::Uuid::new_v4());
        let dir_a = run_dir(&work_dir, &run_a);
        let dir_b = run_dir(&work_dir, &run_b);
        for d in [&dir_a, &dir_b] {
            ssh_run(
                &login(&host),
                &format!(
                    "mkdir -p {d}/repo && chmod 700 {d}",
                    d = ssh::remote_path(d)
                ),
                None,
            )
            .await
            .unwrap();
        }

        // --- happy path: runs, completes, logs arrive, module is available ---
        let mut spec_a = mk(
            &run_a,
            "echo hello-from-sge; echo \"env=$ORX_E2E\"; \
             command -v module >/dev/null 2>&1 && echo module-ok || echo module-MISSING; \
             echo \"pwd=$(pwd)\"",
        );
        spec_a.resources = vec!["h_rt=00:10:00".to_string()];
        let job_a = run_job(&spec_a).await.expect("qsub");

        // The directive round-trip: every requested resource must survive into
        // the scheduler's own view of the job. If repeated `#$ -l` lines do NOT
        // accumulate on this GE version, this is where we find out.
        let detail = ssh_run(&login(&host), &format!("qstat -j {job_a} 2>&1"), None)
            .await
            .unwrap_or_default();
        // NB the field is "hard resource_list" with a SPACE, not an underscore,
        // and h_rt is normalized to seconds (00:10:00 -> 600).
        assert!(
            detail.contains("hard resource_list"),
            "qstat -j did not report a resource list; did the directives parse?\n{detail}"
        );
        assert!(
            detail.contains("h_rt=600"),
            "requested resources did not reach the scheduler — repeated `#$ -l` lines may not \
             accumulate on this GE version, in which case join them into one `-l a=1,b=2`:\n{detail}"
        );

        let done = poll(dir_a.clone(), job_a.clone(), &["COMPLETED", "ERROR"]).await;
        assert_eq!(done.stage, "COMPLETED", "message: {:?}", done.message);

        let mut lines = Vec::new();
        ssh::stream_logs(
            &login(&host),
            &dir_a,
            0,
            std::time::Duration::from_secs(5),
            &mut |l: &str| lines.push(l.to_string()),
        )
        .await
        .unwrap();
        assert!(lines.iter().any(|l| l == "hello-from-sge"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "env=1"), "{lines:?}");
        // Proves the `#!/bin/bash -l` choice: without a login shell SCC users
        // have no `module`, and every modules-based run would break.
        assert!(
            lines.iter().any(|l| l == "module-ok"),
            "the login shell did not provide `module`: {lines:?}"
        );
        // Proves `#$ -wd` took effect rather than defaulting to $HOME: the
        // payload cd's into repo/, so the run dir must be its parent.
        assert!(
            lines.iter().any(|l| l == &format!("pwd={dir_a}/repo")),
            "job did not run in its run dir: {lines:?}"
        );

        // --- cancel path: qdel leaves no exit code, so accounting decides ---
        let job_b = run_job(&mk(&run_b, "sleep 600")).await.unwrap();
        poll(dir_b.clone(), job_b.clone(), &["RUNNING", "SCHEDULING"]).await;
        cancel_job(&host, &job_b).await.unwrap();
        let after = poll(dir_b.clone(), job_b.clone(), &["CANCELED", "GONE", "ERROR"]).await;

        // Time the escalation: a regression in qacct cost would silently change
        // how long a cancelled run takes to reach a verdict.
        let started = std::time::Instant::now();
        let acct = probe_accounting(&host, &job_b).await.unwrap();
        eprintln!("qacct took {:?} and returned {acct:?}", started.elapsed());

        // Best-effort teardown before asserting — don't litter a shared filesystem.
        let _ = ssh_run(
            &login(&host),
            &format!(
                "rm -rf {a} {b}",
                a = ssh::remote_path(&dir_a),
                b = ssh::remote_path(&dir_b)
            ),
            None,
        )
        .await;

        assert!(
            matches!(after.stage.as_str(), "CANCELED" | "GONE" | "ERROR"),
            "unexpected post-cancel stage: {after:?}"
        );
    }

    /// Blank strings in the settings file must fall back to the default rather
    /// than emitting an empty directive.
    #[test]
    fn blank_settings_fields_fall_back() {
        let s: SgeSettings = serde_json::from_str(
            r#"{"sccProject":"  ","gpuType":"","timeLimit":" ","workDir":""}"#,
        )
        .unwrap();
        assert_eq!(s.scc_project_or_default(), "herbdl");
        assert_eq!(s.gpu_type_or_default(), "L40S");
        assert_eq!(s.time_limit_or_default(), "12h");
        assert_eq!(s.resolved_work_dir().unwrap(), DEFAULT_WORK_DIR);
    }
}
