//! Slurm settings and supervised launches. Engine launches use `engine::adapters::slurm`.
//!
//! orx talks to the cluster's **login node** over ssh and drives the Slurm CLI there:
//! `squeue`/`sacct` to poll, `scancel` to cancel. No `slurmrestd`: the REST daemon needs
//! cluster-admin setup that most users don't have, while everyone with a cluster account can
//! ssh to the login node.
//!
//! The 0.2.x layout extends the ssh backend's convention. A run owns
//! `<root>/runs/<run_id>/` (root defaults to `~/.orx`) on the cluster's shared filesystem:
//!   repo/       the experiment's committed source snapshot
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
    /// `#SBATCH --qos=…` default; the `--qos` default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qos: Option<String>,
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
    /// `#SBATCH` arguments written after orx's own on every run, before each run's `--sbatch-arg`s.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sbatch_args: Vec<String>,
    /// `Some(false)` leaves out `#SBATCH --ntasks=1`, which some clusters' submit filters refuse;
    /// absent writes it, as some other clusters refuse a job without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_one_task: Option<bool>,
    /// Fields from a newer orx, kept across this version's saves.
    #[serde(flatten)]
    pub unknown: serde_json::Map<String, serde_json::Value>,
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

/// A single-token directive value, named by its flag or its field (`--qos`, `QoS`, `--nodelist`):
/// whitespace or a control character would split or end its `#SBATCH` line.
pub fn parse_value(name: &str, value: &str) -> Result<String> {
    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(anyhow!(
            "{name} takes one Slurm value without spaces, such as {}.",
            match name {
                "--qos" | "QoS" => "acc_ehpc",
                "--nodelist" | "--exclude" => "node[01-04],gpu7",
                _ => "a100&ib",
            }
        ));
    }
    Ok(value.to_string())
}

/// The long name an `#SBATCH` argument sets: `-p gpu` is `partition`, `--qos=x` is `qos`;
/// empty for a short option orx never writes. Long names may be abbreviated, as sbatch's
/// option parser accepts any unambiguous prefix.
pub fn sbatch_option(arg: &str) -> &str {
    if let Some(long) = arg.strip_prefix("--") {
        return long.split(['=', ' ', '\t']).next().unwrap_or_default();
    }
    match arg.strip_prefix('-').and_then(|short| short.chars().next()) {
        Some('o') => "output",
        Some('e') => "error",
        Some('J') => "job-name",
        Some('a') => "array",
        Some('W') => "wait",
        Some('p') => "partition",
        Some('A') => "account",
        Some('q') => "qos",
        Some('t') => "time",
        Some('c') => "cpus-per-task",
        Some('n') => "ntasks",
        Some('w') => "nodelist",
        Some('C') => "constraint",
        Some('x') => "exclude",
        _ => "",
    }
}

/// Options orx-host passes to `sbatch` itself, or that would stop it tracking the job.
const HOST_OWNED: [&str; 8] = [
    "output",
    "error",
    "job-name",
    "requeue",
    "no-requeue",
    "array",
    "wait",
    "wrap",
];

/// Validate one extra `#SBATCH` option (`--rsc g=1`), as written after `#SBATCH` or with it.
/// Whether it repeats a line orx writes depends on the settings and the run; callers check that.
pub fn parse_sbatch_arg(arg: &str) -> Result<String> {
    let arg = arg.trim();
    let arg = arg
        .strip_prefix("#SBATCH")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map_or(arg, str::trim_start);
    if arg.chars().any(char::is_control) {
        return Err(anyhow!(
            "{} spans more than one line. Give each sbatch option separately.",
            arg.lines().next().unwrap_or_default()
        ));
    }
    // sbatch unquotes and unescapes a line and ends it at `#` before parsing it, so an
    // escaped, quoted or cut-off name could hide an option from the checks below.
    let option = arg.split(['=', ' ', '\t']).next().unwrap_or_default();
    if arg.contains(['\\', '#']) || option.contains(['"', '\'']) {
        return Err(anyhow!(
            "{arg} has a backslash, a # or a quoted option name. Write the option plainly, \
             such as --rsc p=1:g=1."
        ));
    }
    let name = sbatch_option(arg);
    if !arg.starts_with('-') || (name.is_empty() && (arg == "-" || arg.starts_with("--"))) {
        return Err(anyhow!(
            "{arg} isn't an sbatch option. Write it as you would after #SBATCH, such as \
             --rsc p=1:g=1."
        ));
    }
    if !single_option(arg) {
        return Err(anyhow!(
            "{arg} holds more than one sbatch option. Put each on its own line."
        ));
    }
    let owned = HOST_OWNED
        .iter()
        .find(|owned| !name.is_empty() && owned.starts_with(name));
    if let Some(owned) = owned {
        return Err(anyhow!(
            "{arg} sets --{owned}, which OpenResearch sets itself to log and track the job. \
             Remove that line."
        ));
    }
    Ok(arg.to_string())
}

/// Whether `arg` is one option and at most one value, as sbatch splits an `#SBATCH` line:
/// a second option, or a bundle of short flags such as `-vn4`, would escape the checks above.
fn single_option(arg: &str) -> bool {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    for c in arg.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            (_, c) => word.push(c),
        }
    }
    words.push(word);

    // sbatch's short options that take no value; any other takes the rest of its word.
    const FLAGS: &str = "hHIOQsuvVW";
    let option = &words[0];
    let attached_value = match option
        .strip_prefix('-')
        .filter(|_| !option.starts_with("--"))
    {
        Some(short) if short.starts_with(|c| FLAGS.contains(c)) => {
            return short.len() == 1 && words.len() == 1
        }
        Some(short) => short.len() > 1,
        None => option.contains('='),
    };
    match &words[1..] {
        [] => true,
        [value] => !attached_value && !value.starts_with('-'),
        _ => false,
    }
}

/// The line among `written` that `arg` would set a second time, if any.
pub fn repeated_line<'a>(arg: &str, written: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let name = sbatch_option(arg);
    written
        .into_iter()
        .find(|line| !name.is_empty() && line.starts_with(name))
}

pub fn resolve_time_limit(explicit: Option<&str>, saved: Option<&str>) -> Result<Option<u64>> {
    explicit
        .or(saved)
        .map(super::huggingface::parse_timeout)
        .transpose()
}

/// Map a `--flavor` string onto a `--gres` request. A flavor names GPUs; `cpu`
/// asks for none, as on Modal, so it can override a GPU default flavor. Cores
/// and memory are asked for separately, with `--cpus`/`--mem`.
///   "cpu"      -> no GRES
///   "gpu"      -> gpu:1
///   "gpu:…"    -> passed through verbatim (already a GRES spec)
///   "h100:2"   -> gpu:h100:2
///   "h100"     -> gpu:h100
pub fn resolve_gres(flavor: &str) -> Option<String> {
    let f = flavor.trim();
    if f.is_empty() || f.eq_ignore_ascii_case("cpu") {
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

    /// Settings files as shipped versions wrote them read back unchanged, with none of the
    /// launch options added since.
    #[test]
    fn shipped_settings_read_and_rewrite_unchanged() {
        let shipped: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../tests/contracts/slurm-settings.json"))
                .unwrap();
        for file in shipped {
            let settings: SlurmSettings = serde_json::from_value(file.clone()).unwrap();
            assert_eq!(settings.qos, None);
            assert!(settings.sbatch_args.is_empty() && settings.request_one_task.is_none());
            assert_eq!(serde_json::to_value(&settings).unwrap(), file);
        }
        let added = SlurmSettings {
            qos: Some("acc_ehpc".into()),
            sbatch_args: vec!["--rsc g=1".into()],
            request_one_task: Some(false),
            ..Default::default()
        };
        let read: SlurmSettings =
            serde_json::from_value(serde_json::to_value(&added).unwrap()).unwrap();
        assert_eq!(read.qos, added.qos);
        assert_eq!(read.sbatch_args, added.sbatch_args);
        assert_eq!(read.request_one_task, Some(false));
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
        assert_eq!(resolve_gres("cpu"), None);
        assert_eq!(resolve_gres(" CPU "), None);
        assert_eq!(resolve_gres("gpu").as_deref(), Some("gpu:1"));
        assert_eq!(resolve_gres("gpu:a100:4").as_deref(), Some("gpu:a100:4"));
        assert_eq!(resolve_gres("h100:2").as_deref(), Some("gpu:h100:2"));
        assert_eq!(resolve_gres("h100").as_deref(), Some("gpu:h100"));
    }
}
