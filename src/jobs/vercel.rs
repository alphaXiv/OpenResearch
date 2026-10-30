//! Vercel Sandbox backend — ephemeral Firecracker microVMs on a Vercel project.
//!
//! Driven through the official `vercel` Python SDK (`vercel.sandbox`), the same
//! shape as the Modal backend: every operation shells out to a bundled launcher
//! running in an orx-managed venv. Sandboxes are addressable by name
//! (`get_sandbox(name=...)`), which lets a detached `orx supervise` reattach from
//! a fresh process.
//!
//! Two Vercel specifics shape the launcher:
//! - A sandbox outlives its command until the session's execution time limit,
//!   billing provisioned memory the whole time, so the supervisor `finish`es
//!   (stops and destroys) it once the run is terminal and its logs are drained.
//! - Process output streams are single-use and forward-only, while supervise
//!   replays logs from the start on every reconnect. The job therefore writes
//!   to a log file inside the sandbox and `logs` tails that file from line 1.
//!
//! Auth is Vercel's own: `VERCEL_OIDC_TOKEN`, or `VERCEL_TOKEN` +
//! `VERCEL_TEAM_ID` + `VERCEL_PROJECT_ID`, from the process env or the synced
//! `~/.openresearch/env`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::process::Command;

use super::managed_env::ManagedEnv;
use crate::error::{anyhow, Result};

/// Bundled Python launcher. Subcommands (argv[1]):
///   submit                spec JSON on stdin -> {"sandboxName": "...", "processId": "..."}
///   status <name> <pid>   -> {"stage": "...", "message": "..."?}
///   logs   <name>         streams the job's merged stdout/stderr from line 1
///   cancel <name>         stop the sandbox (the job dies with it)
///   finish <name>         stop and destroy the sandbox once the run is over
///
/// Exit 3 = the `vercel` package isn't importable (surfaced as a friendly hint).
/// `@@LOG_FOLLOW@@` is replaced with [`LOG_FOLLOW_SH`] before the launcher runs.
const LAUNCHER: &str = r#"
import sys, json, shlex, subprocess

LOG = "/tmp/orx-run.log"
PID = "/tmp/orx-run.pid"
ENV = "/tmp/orx-env.sh"
EXIT_TAG = "orx_exit"

def _sdk():
    try:
        from vercel.sandbox import sync as sandbox
        import vercel.sandbox as types
        return sandbox, types
    except Exception:
        sys.stderr.write("vercel-not-installed")
        sys.exit(3)

def _status(box):
    s = box.status
    return str(getattr(s, "value", s) or "").lower()

def _emit_exit(code):
    if code == 0:
        out = {"stage": "COMPLETED"}
    else:
        out = {"stage": "ERROR", "message": "exited with code %d" % code}
    print(json.dumps(out))

def submit():
    sandbox, types = _sdk()
    spec = json.load(sys.stdin)
    kwargs = {
        "name": spec["name"],
        # Nothing to resume: a finished run's filesystem is discarded with it.
        "persistent": False,
        "execution_time_limit": int(spec["executionTimeLimit"]),
    }
    if spec.get("image"):
        kwargs["image"] = spec["image"]
    if spec.get("vcpus"):
        kwargs["resources"] = types.SandboxResources(vcpus=int(spec["vcpus"]))
    if spec.get("region"):
        kwargs["region"] = spec["region"]
    if spec.get("tags"):
        kwargs["tags"] = spec["tags"]
    box = sandbox.create_sandbox(**kwargs)
    try:
        # Secrets ride a 0600 file the job sources, never the sandbox or process
        # env config, which the Sandbox API and dashboard expose.
        env = spec.get("env") or {}
        body = "".join("export %s=%s\n" % (k, shlex.quote(str(v))) for k, v in sorted(env.items()))
        box.fs.write_text(ENV, body, mode=0o600)
        if spec.get("sourceArchive"):
            with open(spec["sourceArchive"], "rb") as src, \
                    box.fs.open("/tmp/orx-source.tar", "wb", permissions=0o600) as dst:
                while True:
                    chunk = src.read(1 << 20)
                    if not chunk:
                        break
                    dst.write(chunk)
            box.fs.write_text("/tmp/orx-source.tar.ready", "")
        wrapper = (
            "echo $$ > %s; set -a; . %s; set +a; rm -f %s; "
            "( %s ) > %s 2>&1"
        ) % (PID, ENV, ENV, spec["script"], LOG)
        kill_after = spec.get("killAfter")
        proc = box.create_process(
            "bash", ["-c", wrapper],
            kill_after=float(kill_after) if kill_after else None,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    except BaseException:
        try:
            box.stop()
            box.destroy(delete_orphan_snapshots=True)
        except Exception:
            pass
        raise
    print(json.dumps({"sandboxName": box.name, "processId": proc.id}))

def status(name, pid):
    sandbox, types = _sdk()
    try:
        box = sandbox.get_sandbox(name=name)
    except types.SandboxApiError as error:
        # Already destroyed. A terminal stage lets supervise finish the run;
        # raising here would retry forever and leave it "running".
        if getattr(error, "status_code", None) == 404:
            print(json.dumps({
                "stage": "DELETED",
                "message": "sandbox not found",
            }))
            return
        raise
    tags = dict(box.tags or {})
    if EXIT_TAG in tags:
        return _emit_exit(int(tags[EXIT_TAG]))
    state = _status(box)
    if state == "pending":
        print(json.dumps({"stage": "SCHEDULING"}))
        return
    if state != "running":
        # Only a running sandbox is queried for its process: process calls on a
        # stopped sandbox would boot a fresh session.
        print(json.dumps({
            "stage": "ERROR",
            "message": "sandbox %s before the job exited (execution time limit reached or stopped outside orx)" % (state or "stopped"),
        }))
        return
    # The API only records a process's exit once a client waits on it, and a
    # wait blocks while it runs, so check the wrapper's pid inside the sandbox
    # first. No pid file yet means the wrapper hasn't started.
    probe = box.run_process(
        "bash",
        ["-c", "[ ! -s %s ] || kill -0 \"$(cat %s)\" 2>/dev/null && echo running || echo exited" % (PID, PID)],
        capture_output=True,
    )
    if probe.stdout.strip() != "exited":
        print(json.dumps({"stage": "RUNNING"}))
        return
    code = box.get_process(pid, wait=True).returncode
    if code is None:
        print(json.dumps({"stage": "RUNNING"}))
        return
    # Record the exit on the sandbox so a restarted supervisor still knows it
    # after the session stops.
    try:
        tags[EXIT_TAG] = str(code)
        box.update(tags=tags)
    except Exception:
        pass
    _emit_exit(code)

# Replays from line 1 (supervise dedups by line count) until the wrapper pid
# exits. GNU tail --pid when the image has it; otherwise a polling loop. A
# failure prints ORX_LOG_FOLLOW_ERROR: instead of exiting with no output.
FOLLOW = r'''@@LOG_FOLLOW@@'''

def logs(name):
    sandbox, _ = _sdk()
    box = sandbox.get_sandbox(name=name)
    if _status(box) != "running":
        return
    proc = box.create_process(
        "bash", ["-c", FOLLOW, "bash", LOG, PID], stderr=subprocess.DEVNULL,
    )
    for line in proc.stdout:
        if line.startswith("ORX_LOG_FOLLOW_ERROR:"):
            if not line.endswith("\n"):
                line += "\n"
            sys.stderr.write(line)
            sys.exit(1)
        if not line.endswith("\n"):
            line += "\n"
        sys.stdout.write(line)
        sys.stdout.flush()
    code = getattr(proc, "returncode", None)
    if code is None and hasattr(proc, "wait"):
        try:
            code = proc.wait()
        except Exception:
            code = None
    if code not in (0, None):
        sys.stderr.write("ORX_LOG_FOLLOW_ERROR: the log follow command exited %s\n" % code)
        sys.exit(1)

def cancel(name):
    sandbox, _ = _sdk()
    box = sandbox.get_sandbox(name=name)
    if _status(box) in ("pending", "running"):
        box.stop()

def finish(name):
    sandbox, types = _sdk()
    try:
        box = sandbox.get_sandbox(name=name)
    except types.SandboxApiError as error:
        # Already destroyed (an earlier attempt got this far): nothing to do.
        if getattr(error, "status_code", None) == 404:
            return
        raise
    if _status(box) in ("pending", "running"):
        box.stop()
    box.destroy(delete_orphan_snapshots=True)

cmd = sys.argv[1] if len(sys.argv) > 1 else ""
if cmd == "submit":
    submit()
elif cmd == "status":
    status(sys.argv[2], sys.argv[3])
elif cmd == "logs":
    logs(sys.argv[2])
elif cmd == "cancel":
    cancel(sys.argv[2])
elif cmd == "finish":
    finish(sys.argv[2])
else:
    sys.stderr.write("unknown launcher subcommand: %r" % cmd)
    sys.exit(2)
"#;

/// Follow `$1` (log) from the start until the pid in `$2` exits.
///
/// GNU `tail --pid` when the image has it. A custom image (busybox, alpine)
/// often has a `tail` that rejects `--pid` and would otherwise return no logs
/// and no error; this then polls the file until the pid is gone. An unusable
/// `tail` prints `ORX_LOG_FOLLOW_ERROR:` and exits non-zero.
const LOG_FOLLOW_SH: &str = r#"
log=$1
pidfile=$2
touch "$log"
i=0
while [ ! -s "$pidfile" ]; do
  i=$((i + 1))
  if [ "$i" -gt 150 ]; then
    echo "ORX_LOG_FOLLOW_ERROR: the job never wrote its pid"
    exit 1
  fi
  sleep 0.2
done
pid=$(cat "$pidfile")
if command -v tail >/dev/null 2>&1; then
  # A dead pid makes GNU tail exit immediately. A tail that ignores --pid and
  # follows forever is still running after the wait, so it is not GNU.
  tail -n 0 -f --pid=999999999 /dev/null >/dev/null 2>&1 &
  probe=$!
  sleep 0.3
  if kill -0 "$probe" 2>/dev/null; then
    kill "$probe" 2>/dev/null
    wait "$probe" 2>/dev/null
  else
    wait "$probe"
    if [ $? -eq 0 ]; then
      exec tail -n +1 -F --pid="$pid" "$log"
    fi
  fi
fi
offset=0
read_new() {
  size=$(wc -c < "$log" | tr -d '[:space:]')
  case "$size" in
    ''|*[!0-9]*) size=0 ;;
  esac
  if [ "$size" -gt "$offset" ]; then
    if ! tail -c +"$((offset + 1))" "$log"; then
      echo "ORX_LOG_FOLLOW_ERROR: this image cannot follow the job log until the process exits (no GNU tail --pid, and tail -c failed)"
      exit 1
    fi
    offset=$size
  fi
}
while true; do
  read_new
  if ! kill -0 "$pid" 2>/dev/null; then
    sleep 0.2
    read_new
    exit 0
  fi
  sleep 0.2
done
"#;

fn launcher() -> &'static str {
    static SRC: OnceLock<String> = OnceLock::new();
    SRC.get_or_init(|| LAUNCHER.replace("@@LOG_FOLLOW@@", LOG_FOLLOW_SH))
}

/// The sync API with `SandboxResources` and process handles is in vercel 0.12.
const VERCEL_ENV: ManagedEnv = ManagedEnv {
    name: "vercel",
    label: "Vercel Sandbox",
    min_python: (3, 10),
    requirement: "vercel>=0.12",
    ready_check: "from vercel.sandbox import sync, SandboxResources; \
                  assert hasattr(sync, 'create_sandbox') and hasattr(sync, 'get_sandbox')",
};

/// Credentials the Sandbox SDK reads from its environment.
const OIDC_TOKEN: &str = "VERCEL_OIDC_TOKEN";
const ACCESS_TOKEN_KEYS: [&str; 3] = ["VERCEL_TOKEN", "VERCEL_TEAM_ID", "VERCEL_PROJECT_ID"];

/// Default image: Vercel's managed Python image, which ships pip, venv, and uv.
pub const DEFAULT_IMAGE: &str = "vercel/sandbox/python:3.14";

/// Vercel caps one session at 24 hours (45 minutes on Hobby).
pub const MAX_TIMEOUT_SECS: u64 = 24 * 3600;

/// Job timeout when the plan is not Hobby (and when the plan cannot be read).
pub const DEFAULT_TIMEOUT_SECS: u64 = 4 * 3600;

/// Hobby's session cap. The job timeout is this minus [`SESSION_GRACE_SECS`].
pub const HOBBY_SESSION_SECS: u64 = 45 * 60;

/// Headroom past the job's own kill deadline, so the final log lines and the
/// exit status can still be read before Vercel stops the session.
const SESSION_GRACE_SECS: u64 = 5 * 60;

/// Vercel team billing plans that change sandbox limits.
/// <https://vercel.com/docs/sandbox/pricing> and the Sandbox FAQ: Hobby 45
/// minutes and 4 vCPUs, Pro 24 hours and 8 vCPUs, Enterprise 24 hours and 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamPlan {
    Hobby,
    Pro,
    Enterprise,
}

impl TeamPlan {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "hobby" => Some(Self::Hobby),
            "pro" => Some(Self::Pro),
            "enterprise" => Some(Self::Enterprise),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hobby => "Hobby",
            Self::Pro => "Pro",
            Self::Enterprise => "Enterprise",
        }
    }

    fn max_vcpus(self) -> u32 {
        match self {
            Self::Hobby => 4,
            Self::Pro => 8,
            Self::Enterprise => 32,
        }
    }

    fn max_session_secs(self) -> u64 {
        match self {
            Self::Hobby => HOBBY_SESSION_SECS,
            Self::Pro | Self::Enterprise => MAX_TIMEOUT_SECS,
        }
    }
}

/// Limits applied at launch. `plan` is absent when the team could not be read
/// (OIDC only, or the lookup failed); those launches keep the wide caps and
/// surface a plan-limit error from sandbox create instead of blocking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchLimits {
    pub plan: Option<TeamPlan>,
    max_vcpus: u32,
    max_session_secs: u64,
}

impl LaunchLimits {
    fn for_plan(plan: TeamPlan) -> Self {
        Self {
            plan: Some(plan),
            max_vcpus: plan.max_vcpus(),
            max_session_secs: plan.max_session_secs(),
        }
    }

    fn unknown() -> Self {
        Self {
            plan: None,
            max_vcpus: 32,
            max_session_secs: MAX_TIMEOUT_SECS,
        }
    }

    pub fn session_cap(self) -> u64 {
        self.max_session_secs
    }

    /// Longest `--timeout` that still leaves [`SESSION_GRACE_SECS`] inside the session cap.
    pub fn max_job_timeout(self) -> u64 {
        self.max_session_secs.saturating_sub(SESSION_GRACE_SECS)
    }

    /// Hobby defaults to the longest job that fits a 45-minute session plus grace.
    /// Every other case keeps the 4-hour default.
    pub fn default_timeout(self) -> u64 {
        if self.plan == Some(TeamPlan::Hobby) {
            self.max_job_timeout()
        } else {
            DEFAULT_TIMEOUT_SECS
        }
    }
}

pub fn plan_from_team_body(body: &Value) -> Option<TeamPlan> {
    body.get("billing")
        .and_then(|billing| billing.get("plan"))
        .and_then(Value::as_str)
        .and_then(TeamPlan::parse)
}

fn team_id_is_safe(team_id: &str) -> bool {
    !team_id.is_empty()
        && team_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn redact_token(text: &str, token: &str) -> String {
    if token.is_empty() {
        text.to_string()
    } else {
        text.replace(token, "[redacted]")
    }
}

async fn request_team_plan_with(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    team_id: &str,
) -> Result<Option<TeamPlan>> {
    let url = format!("{}/v2/teams/{team_id}", base.trim_end_matches('/'));
    let response = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|error| anyhow!("{}", redact_token(&error.to_string(), token)))?;
    let status = response.status();
    if !status.is_success() {
        return Err(anyhow!("HTTP {status}"));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| anyhow!("{}", redact_token(&error.to_string(), token)))?;
    Ok(plan_from_team_body(&body))
}

async fn request_team_plan(token: &str, team_id: &str) -> Result<Option<TeamPlan>> {
    let client = crate::net::remote_client()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| anyhow!("HTTP client: {error}"))?;
    request_team_plan_with(&client, "https://api.vercel.com", token, team_id).await
}

/// One `GET /v2/teams/{teamId}` when a token and team id are both available.
/// Missing credentials, an unreadable plan, or a failed request yield `None`
/// and do not block the launch.
pub async fn lookup_team_plan() -> Option<TeamPlan> {
    let (token, _) = credential("VERCEL_TOKEN")?;
    let (team_id, _) = credential("VERCEL_TEAM_ID")?;
    if !team_id_is_safe(&team_id) {
        eprintln!(
            "orx: VERCEL_TEAM_ID cannot be used to look up the team plan; submitting without \
             plan-specific limits."
        );
        return None;
    }
    match request_team_plan(&token, &team_id).await {
        Ok(Some(plan)) => Some(plan),
        Ok(None) => {
            eprintln!(
                "orx: the Vercel team response had no hobby, pro, or enterprise plan; submitting \
                 without plan-specific limits."
            );
            None
        }
        Err(error) => {
            eprintln!(
                "orx: could not read the Vercel team plan ({error}); submitting without \
                 plan-specific limits."
            );
            None
        }
    }
}

pub async fn launch_limits() -> LaunchLimits {
    match lookup_team_plan().await {
        Some(plan) => LaunchLimits::for_plan(plan),
        None => LaunchLimits::unknown(),
    }
}

fn format_duration(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    match (hours, minutes, seconds) {
        (0, 0, seconds) => format!("{seconds}s"),
        (0, minutes, 0) => format!("{minutes}m"),
        (0, minutes, seconds) => format!("{minutes}m{seconds}s"),
        (hours, 0, 0) => format!("{hours}h"),
        (hours, minutes, 0) => format!("{hours}h{minutes}m"),
        (hours, minutes, seconds) => format!("{hours}h{minutes}m{seconds}s"),
    }
}

pub fn check_timeout(timeout_seconds: u64, limits: &LaunchLimits) -> Result<()> {
    let max = limits.max_job_timeout();
    if timeout_seconds <= max {
        return Ok(());
    }
    let grace = format_duration(SESSION_GRACE_SECS);
    let session = format_duration(limits.max_session_secs);
    let max_timeout = format_duration(max);
    let got = format_duration(timeout_seconds);
    Err(match limits.plan {
        Some(plan) => anyhow!(
            "This Vercel team is on the {} plan, which caps a sandbox session at {session}. \
             orx keeps {grace} after the job to read the exit status and the last log lines, \
             so --timeout can be at most {max_timeout} (got {got}).",
            plan.label()
        ),
        None => anyhow!(
            "Vercel Sandbox sessions run for at most {session}. orx keeps {grace} after the job \
             to read the exit status and the last log lines, so --timeout can be at most \
             {max_timeout} (got {got})."
        ),
    })
}

pub fn check_vcpus(vcpus: Option<u32>, limits: &LaunchLimits) -> Result<()> {
    let Some(vcpus) = vcpus else {
        return Ok(());
    };
    if vcpus <= limits.max_vcpus {
        return Ok(());
    }
    Err(match limits.plan {
        Some(plan) => anyhow!(
            "This Vercel team is on the {} plan, which allows at most {} vCPUs (got {vcpus}).",
            plan.label(),
            limits.max_vcpus
        ),
        None => anyhow!(
            "Vercel Sandbox takes at most {} vCPUs (got {vcpus}).",
            limits.max_vcpus
        ),
    })
}

fn python_bin() -> String {
    if let Ok(p) = std::env::var("ORX_VERCEL_PYTHON") {
        if !p.trim().is_empty() {
            return p;
        }
    }
    let managed = VERCEL_ENV.python();
    if managed.exists() {
        return managed.to_string_lossy().into_owned();
    }
    "python3".to_string()
}

/// Ensure a usable interpreter exists, provisioning the orx-managed venv on
/// first use. A no-op when `$ORX_VERCEL_PYTHON` is set. Only the launch path
/// calls this — the supervisor relies on the env already being there.
pub async fn ensure_env() -> Result<()> {
    if std::env::var("ORX_VERCEL_PYTHON").is_ok_and(|p| !p.trim().is_empty()) {
        return Ok(());
    }
    VERCEL_ENV.ensure().await.map_err(|e| {
        anyhow!(
            "{}. Or set ORX_VERCEL_PYTHON to an interpreter with {}.",
            e.to_string().trim_end_matches('.'),
            VERCEL_ENV.requirement
        )
    })?;
    Ok(())
}

/// A credential's value and where it came from: a non-empty process env value
/// wins, then the synced env file (`orx up` Settings → Environment). The one
/// resolver behind both readiness and the launcher's env, so they agree.
fn resolve_credential(
    key: &str,
    process: impl Fn(&str) -> Option<String>,
    synced: impl Fn(&str) -> Option<String>,
) -> Option<(String, &'static str)> {
    let present = |v: &String| !v.trim().is_empty();
    process(key)
        .filter(present)
        .map(|v| (v, "env"))
        .or_else(|| synced(key).filter(present).map(|v| (v, "syncedEnv")))
}

fn credential(key: &str) -> Option<(String, &'static str)> {
    resolve_credential(
        key,
        |k| std::env::var(k).ok(),
        crate::config::synced_env_var,
    )
}

/// The synced credentials the launcher must set explicitly — those the
/// inherited process env lacks or holds empty.
fn auth_overrides(
    process: impl Fn(&str) -> Option<String>,
    synced: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    std::iter::once(OIDC_TOKEN)
        .chain(ACCESS_TOKEN_KEYS)
        .filter_map(|key| match resolve_credential(key, &process, &synced) {
            Some((value, "syncedEnv")) => Some((key.to_string(), value)),
            _ => None,
        })
        .collect()
}

/// Where usable Vercel credentials come from, if anywhere: `"env"` when the
/// process env supplies them, `"syncedEnv"` when any part comes from the
/// Environment tab. Sync and cheap (env + fs only).
pub fn token_source() -> Option<&'static str> {
    if let Some((_, source)) = credential(OIDC_TOKEN) {
        return Some(source);
    }
    let parts: Vec<_> = ACCESS_TOKEN_KEYS.iter().map(|k| credential(k)).collect();
    if parts.iter().any(Option::is_none) {
        return None;
    }
    if parts.iter().flatten().any(|(_, s)| *s == "syncedEnv") {
        Some("syncedEnv")
    } else {
        Some("env")
    }
}

/// Per-variable credential source: `"env"`, `"syncedEnv"`, or absent.
pub fn credential_sources() -> Vec<(&'static str, Option<&'static str>)> {
    std::iter::once(OIDC_TOKEN)
        .chain(ACCESS_TOKEN_KEYS)
        .map(|key| (key, credential(key).map(|(_, source)| source)))
        .collect()
}

/// Whether the managed SDK venv is installed, or `ORX_VERCEL_PYTHON` overrides it.
/// `"override"` means the user pointed at an interpreter; `"managed"` is the
/// orx venv, ready only once its interpreter exists.
pub fn sdk_env_status() -> (&'static str, bool) {
    if std::env::var("ORX_VERCEL_PYTHON")
        .ok()
        .is_some_and(|path| !path.trim().is_empty())
    {
        return ("override", true);
    }
    ("managed", VERCEL_ENV.python().exists())
}

/// Credentials to inject into the launcher subprocess.
fn auth_env() -> Vec<(String, String)> {
    auth_overrides(|k| std::env::var(k).ok(), crate::config::synced_env_var)
}

/// Drop the Vercel credentials from a job's env: the job has no business
/// creating more sandboxes on the user's project.
pub fn strip_credentials(env: &mut HashMap<String, String>) {
    env.remove(OIDC_TOKEN);
    for key in ACCESS_TOKEN_KEYS {
        env.remove(key);
    }
}

fn missing_python_hint(e: &std::io::Error) -> crate::error::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow!(
            "{} not found on PATH — Vercel Sandbox is driven through its Python SDK. Install \
             Python 3.10+, or point orx at an interpreter with `vercel` via ORX_VERCEL_PYTHON.",
            python_bin()
        )
    } else {
        anyhow!("Could not run {}: {}", python_bin(), e)
    }
}

fn launcher_error(what: &str, code: Option<i32>, stderr: &str) -> crate::error::Error {
    let stderr = stderr.trim();
    if code == Some(3) || stderr == "vercel-not-installed" {
        return anyhow!(
            "The `vercel` Python package isn't importable by {}. Remove {} to force a rebuild, \
             or set ORX_VERCEL_PYTHON to an interpreter that has it.",
            python_bin(),
            VERCEL_ENV.dir().display()
        );
    }
    // The SDK's traceback ends with the useful line.
    let detail = stderr.lines().last().unwrap_or("no stderr");
    let hint = plan_limit_hint(stderr);
    anyhow!(
        "vercel sandbox {} failed{}: {}{}",
        what,
        code.map(|c| format!(" (exit {c})")).unwrap_or_default(),
        detail,
        hint.unwrap_or("")
    )
}

/// Extra context when Vercel rejects a create for a plan limit and orx could
/// not (or did not) reject it first. The sandbox error itself stays in `detail`.
fn plan_limit_hint(stderr: &str) -> Option<&'static str> {
    let lower = stderr.to_ascii_lowercase();
    let timeout = lower.contains("execution_time")
        || lower.contains("execution time")
        || lower.contains("timeout")
        || (lower.contains("duration") && lower.contains("limit"));
    let vcpus = lower.contains("vcpu")
        || (lower.contains("cpu")
            && (lower.contains("plan") || lower.contains("limit") || lower.contains("maximum")));
    if timeout {
        Some(
            ". Vercel caps a sandbox session at 45 minutes on Hobby and 24 hours on Pro and \
             Enterprise. orx keeps 5 minutes of that to read the exit status and the last log \
             lines, so pass a shorter --timeout.",
        )
    } else if vcpus {
        Some(". Vercel allows at most 4 vCPUs on Hobby, 8 on Pro, and 32 on Enterprise.")
    } else {
        None
    }
}

async fn launcher_capture(args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>> {
    let mut cmd = Command::new(python_bin());
    cmd.arg("-c")
        .arg(launcher())
        .args(args)
        .env(super::PYTHONUNBUFFERED, "1")
        .envs(auth_env())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| missing_python_hint(&e))?;
    if let Some(input) = stdin {
        use tokio::io::AsyncWriteExt as _;
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(input.as_bytes()).await;
            drop(pipe);
        }
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| anyhow!("Could not run {}: {}", python_bin(), e))?;
    if !out.status.success() {
        return Err(launcher_error(
            args.first().copied().unwrap_or("?"),
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out.stdout)
}

/// Parse a `--flavor` into a vCPU count. Vercel Sandbox has no GPUs; memory is
/// fixed at 2 GB per vCPU. Accepts `cpu` (the default 2 vCPUs), `cpu-<n>`, or
/// a bare `<n>`, where n is 1 or an even number up to 32 (plan caps apply).
pub fn resolve_flavor(name: Option<&str>) -> Result<Option<u32>> {
    let Some(raw) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Ok(None);
    };
    let n = raw.to_ascii_lowercase();
    if n == "cpu" {
        return Ok(None);
    }
    let digits = n
        .strip_prefix("cpu-")
        .or_else(|| n.strip_prefix("cpu_"))
        .unwrap_or(&n);
    let vcpus: u32 = digits.parse().map_err(|_| {
        anyhow!(
            "Unknown Vercel Sandbox flavor '{raw}'. Vercel Sandbox is CPU-only: use cpu, \
             cpu-<n> or <n> vCPUs (1, or an even number up to 32; 2 GB memory per vCPU)."
        )
    })?;
    if vcpus == 1 || ((2..=32).contains(&vcpus) && vcpus.is_multiple_of(2)) {
        Ok(Some(vcpus))
    } else {
        Err(anyhow!(
            "Vercel Sandbox takes 1 or an even number of vCPUs up to 32 (got {vcpus})."
        ))
    }
}

/// A stable, valid sandbox name for a run: lowercase alphanumerics and dashes.
pub fn sandbox_name(run_id: &str) -> String {
    let mut name = String::from("orx-");
    for c in run_id.chars() {
        let c = c.to_ascii_lowercase();
        name.push(if c.is_ascii_alphanumeric() { c } else { '-' });
    }
    name.truncate(63);
    name.trim_end_matches('-').to_string()
}

pub struct VercelJobSpec {
    pub name: String,
    /// `bash -c` payload (the shared snapshot-and-run script).
    pub script: String,
    pub image: String,
    pub vcpus: Option<u32>,
    /// Sourced by the job from a 0600 file (tokens, synced env).
    pub env: HashMap<String, String>,
    pub timeout_seconds: u64,
    /// Plan session cap. The execution time limit is the job timeout plus grace,
    /// and never more than this.
    pub session_cap: u64,
    pub region: Option<String>,
    pub tags: HashMap<String, String>,
    pub source_archive: Option<PathBuf>,
}

/// The reattach handle: sandbox name and the job's process id.
pub struct Submitted {
    pub sandbox_name: String,
    pub process_id: String,
}

/// Create the sandbox, upload the source, and start the job.
pub async fn run_job(spec: &VercelJobSpec) -> Result<Submitted> {
    let env = super::default_python_env(&spec.env);
    let body = json!({
        "name": spec.name,
        "image": spec.image,
        "script": spec.script,
        "vcpus": spec.vcpus,
        "env": env,
        "killAfter": spec.timeout_seconds,
        "executionTimeLimit": session_limit(spec.timeout_seconds, spec.session_cap),
        "region": spec.region,
        "tags": spec.tags,
        "sourceArchive": spec.source_archive,
    });
    let out = launcher_capture(&["submit"], Some(&body.to_string())).await?;
    let v: Value = serde_json::from_slice(&out).map_err(|e| {
        anyhow!(
            "Unreadable vercel sandbox submit output: {} ({:?})",
            e,
            String::from_utf8_lossy(&out)
        )
    })?;
    let field = |key: &str| {
        v.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("vercel sandbox submit returned no {key}"))
    };
    Ok(Submitted {
        sandbox_name: field("sandboxName")?,
        process_id: field("processId")?,
    })
}

/// The session limit for a job deadline: the deadline plus grace, never past
/// the plan's session cap. Callers reject a timeout that would eat the grace
/// (`check_timeout`) so this cap is a backstop, not the usual path.
pub fn session_limit(timeout_seconds: u64, session_cap: u64) -> u64 {
    timeout_seconds
        .saturating_add(SESSION_GRACE_SECS)
        .min(session_cap)
}

/// Sandbox state in the shared stage vocabulary (see `jobs::stage_to_run_status`).
#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
}

pub async fn inspect_job(sandbox_name: &str, process_id: &str) -> Result<JobState> {
    let out = launcher_capture(&["status", sandbox_name, process_id], None).await?;
    let v: Value = serde_json::from_slice(&out)
        .map_err(|e| anyhow!("Unreadable vercel sandbox status output: {}", e))?;
    Ok(JobState {
        stage: v
            .get("stage")
            .and_then(Value::as_str)
            .unwrap_or("RUNNING")
            .to_string(),
        message: v.get("message").and_then(Value::as_str).map(str::to_string),
    })
}

pub async fn cancel_job(sandbox_name: &str) -> Result<()> {
    launcher_capture(&["cancel", sandbox_name], None).await?;
    Ok(())
}

/// Stop and destroy the sandbox; called once the run is terminal.
pub async fn finish_job(sandbox_name: &str) -> Result<()> {
    launcher_capture(&["finish", sandbox_name], None).await?;
    Ok(())
}

/// One pass over the job's log, invoking `sink` per line past `skip`.
///
/// Same replay/dedup contract as `modal::stream_logs`: each connect replays the
/// log from line 1, so the caller passes how many lines it has consumed and
/// gets the new total back. Ends when the job exits or after `idle` silence.
pub async fn stream_logs(
    sandbox_name: &str,
    skip: u64,
    idle: Duration,
    sink: &mut (dyn FnMut(&str) + Send),
) -> Result<u64> {
    use tokio::io::{AsyncBufReadExt as _, BufReader};

    let mut child = Command::new(python_bin())
        .arg("-c")
        .arg(launcher())
        .args(["logs", sandbox_name])
        .env(super::PYTHONUNBUFFERED, "1")
        .envs(auth_env())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| missing_python_hint(&e))?;

    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take();
    let mut lines = BufReader::new(stdout).lines();
    let mut seen = 0u64;
    loop {
        match tokio::time::timeout(idle, lines.next_line()).await {
            Err(_) => break,
            Ok(Err(e)) => {
                return Err(anyhow!(
                    "Could not read the Vercel Sandbox log launcher output: {}",
                    e
                ))
            }
            Ok(Ok(None)) => {
                let status = child.wait().await?;
                if !status.success() {
                    let mut detail = String::new();
                    if let Some(mut stderr) = stderr.take() {
                        use tokio::io::AsyncReadExt as _;
                        let _ = stderr.read_to_string(&mut detail).await;
                    }
                    let detail = detail.trim();
                    return Err(anyhow!(
                        "the Vercel Sandbox log launcher exited with {}{}",
                        status,
                        if detail.is_empty() {
                            String::new()
                        } else {
                            format!(": {detail}")
                        }
                    ));
                }
                break;
            }
            Ok(Ok(Some(line))) => {
                seen += 1;
                if seen > skip {
                    sink(&line);
                }
            }
        }
    }
    let _ = child.kill().await;
    Ok(seen.max(skip))
}

/// Fail fast before source staging or submission if Vercel Sandbox isn't
/// usable, provisioning the orx-managed env on first use.
pub async fn preflight() -> Result<()> {
    ensure_env().await?;
    if token_source().is_none() {
        return Err(anyhow!(
            "No Vercel Sandbox credentials. Set VERCEL_TOKEN, VERCEL_TEAM_ID and \
             VERCEL_PROJECT_ID (or a VERCEL_OIDC_TOKEN from `vercel env pull`) in the process \
             environment or Settings → Environment."
        ));
    }
    let probe = Command::new(python_bin())
        .args(["-c", VERCEL_ENV.ready_check])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| missing_python_hint(&e))?;
    if !probe.status.success() {
        return Err(launcher_error("preflight", Some(3), ""));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn flavors_map_to_vcpus() {
        assert_eq!(resolve_flavor(None).unwrap(), None);
        assert_eq!(resolve_flavor(Some("cpu")).unwrap(), None);
        assert_eq!(resolve_flavor(Some("cpu-8")).unwrap(), Some(8));
        assert_eq!(resolve_flavor(Some("CPU_4")).unwrap(), Some(4));
        assert_eq!(resolve_flavor(Some("1")).unwrap(), Some(1));
        assert_eq!(resolve_flavor(Some("32")).unwrap(), Some(32));
        for bad in ["3", "34", "0", "h100", "cpu-large"] {
            assert!(resolve_flavor(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn sandbox_names_are_stable_and_valid() {
        assert_eq!(sandbox_name("Run_01ABC"), "orx-run-01abc");
        let long = sandbox_name(&"x".repeat(100));
        assert_eq!(long.len(), 63);
    }

    #[test]
    fn session_limit_keeps_the_grace_inside_the_cap() {
        assert_eq!(session_limit(3600, MAX_TIMEOUT_SECS), 3600 + 300);
        assert_eq!(
            session_limit(MAX_TIMEOUT_SECS - SESSION_GRACE_SECS, MAX_TIMEOUT_SECS),
            MAX_TIMEOUT_SECS
        );
        // A timeout that fills the session would drop the grace; check_timeout
        // rejects it, and the cap here is only a backstop.
        assert_eq!(
            session_limit(MAX_TIMEOUT_SECS, MAX_TIMEOUT_SECS),
            MAX_TIMEOUT_SECS
        );
        assert_eq!(
            session_limit(HOBBY_SESSION_SECS - SESSION_GRACE_SECS, HOBBY_SESSION_SECS),
            HOBBY_SESSION_SECS
        );
    }

    #[test]
    fn hobby_plan_defaults_and_caps_leave_room_for_grace() {
        let hobby = LaunchLimits::for_plan(TeamPlan::Hobby);
        assert_eq!(
            hobby.default_timeout(),
            HOBBY_SESSION_SECS - SESSION_GRACE_SECS
        );
        assert!(check_timeout(hobby.default_timeout(), &hobby).is_ok());
        assert!(check_timeout(HOBBY_SESSION_SECS, &hobby).is_err());
        let timeout_err = check_timeout(4 * 3600, &hobby).unwrap_err().to_string();
        assert!(timeout_err.contains("Hobby"), "{timeout_err}");
        assert!(timeout_err.contains("45m"), "{timeout_err}");
        assert!(timeout_err.contains("40m"), "{timeout_err}");
        assert!(check_vcpus(Some(4), &hobby).is_ok());
        assert!(check_vcpus(None, &hobby).is_ok());
        let cpu_err = check_vcpus(Some(8), &hobby).unwrap_err().to_string();
        assert!(cpu_err.contains("4 vCPUs"), "{cpu_err}");

        let pro = LaunchLimits::for_plan(TeamPlan::Pro);
        assert_eq!(pro.default_timeout(), DEFAULT_TIMEOUT_SECS);
        assert!(check_vcpus(Some(8), &pro).is_ok());
        assert!(check_vcpus(Some(16), &pro).is_err());
        assert!(check_timeout(MAX_TIMEOUT_SECS - SESSION_GRACE_SECS, &pro).is_ok());
        assert!(check_timeout(MAX_TIMEOUT_SECS, &pro).is_err());

        let enterprise = LaunchLimits::for_plan(TeamPlan::Enterprise);
        assert!(check_vcpus(Some(32), &enterprise).is_ok());
        assert!(check_vcpus(Some(34), &enterprise).is_err());
        assert_eq!(
            enterprise.max_job_timeout(),
            MAX_TIMEOUT_SECS - SESSION_GRACE_SECS
        );

        let unknown = LaunchLimits::unknown();
        assert_eq!(unknown.default_timeout(), DEFAULT_TIMEOUT_SECS);
        assert!(check_vcpus(Some(32), &unknown).is_ok());
        assert!(check_timeout(MAX_TIMEOUT_SECS - SESSION_GRACE_SECS, &unknown).is_ok());
        let unknown_timeout = check_timeout(MAX_TIMEOUT_SECS, &unknown)
            .unwrap_err()
            .to_string();
        assert!(unknown_timeout.contains("23h55m"), "{unknown_timeout}");
        assert!(!unknown_timeout.contains("Hobby"), "{unknown_timeout}");
    }

    #[test]
    fn team_payload_plan_is_billing_plan() {
        assert_eq!(
            plan_from_team_body(&json!({"billing": {"plan": "hobby"}})),
            Some(TeamPlan::Hobby)
        );
        assert_eq!(
            plan_from_team_body(&json!({"billing": {"plan": "Pro"}})),
            Some(TeamPlan::Pro)
        );
        assert_eq!(
            plan_from_team_body(&json!({"billing": {"plan": "enterprise"}})),
            Some(TeamPlan::Enterprise)
        );
        assert_eq!(plan_from_team_body(&json!({"billing": {}})), None);
        assert_eq!(
            plan_from_team_body(&json!({"billing": {"plan": "trial"}})),
            None
        );
    }

    #[test]
    fn plan_limit_failures_name_the_cap() {
        let timeout = launcher_error(
            "submit",
            Some(1),
            "Traceback\nError: execution timeout exceeds the plan limit",
        )
        .to_string();
        assert!(timeout.contains("45 minutes"), "{timeout}");
        let cpu =
            launcher_error("submit", Some(1), "Error: vcpus exceeds plan maximum").to_string();
        assert!(cpu.contains("4 vCPUs"), "{cpu}");
        let other = launcher_error("submit", Some(1), "Error: network unreachable").to_string();
        assert!(!other.contains("45 minutes"), "{other}");
    }

    #[tokio::test]
    async fn team_lookup_reads_billing_plan_once() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = sock.read(&mut tmp).await.unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let req = String::from_utf8_lossy(&buf);
            assert!(req.starts_with("GET /v2/teams/team_abc "), "{req}");
            assert!(
                req.to_ascii_lowercase()
                    .contains("authorization: bearer test-token"),
                "{req}"
            );
            let body = r#"{"id":"team_abc","billing":{"plan":"hobby"}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        let client = crate::net::loopback_client()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let plan =
            request_team_plan_with(&client, &format!("http://{addr}"), "test-token", "team_abc")
                .await
                .unwrap();
        assert_eq!(plan, Some(TeamPlan::Hobby));
    }

    #[tokio::test]
    async fn team_lookup_does_not_treat_an_error_as_a_plan() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut tmp = [0u8; 1024];
            let _ = sock.read(&mut tmp).await;
            let body = "nope";
            let resp = format!(
                "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
        });
        let client = crate::net::loopback_client()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let error = request_team_plan_with(
            &client,
            &format!("http://{addr}"),
            "secret-token",
            "team_abc",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(error.contains("403"), "{error}");
        assert!(!error.contains("secret-token"), "{error}");
    }

    #[test]
    fn launcher_env_falls_back_to_saved_credentials_for_empty_process_values() {
        let process = |k: &str| match k {
            "VERCEL_TOKEN" => Some(String::new()),
            "VERCEL_TEAM_ID" => Some("team_env".to_string()),
            _ => None,
        };
        let synced = |k: &str| match k {
            "VERCEL_TOKEN" => Some("saved-token".to_string()),
            "VERCEL_TEAM_ID" => Some("team_saved".to_string()),
            "VERCEL_PROJECT_ID" => Some("prj_saved".to_string()),
            _ => None,
        };
        assert_eq!(
            auth_overrides(process, synced),
            [
                ("VERCEL_TOKEN".to_string(), "saved-token".to_string()),
                ("VERCEL_PROJECT_ID".to_string(), "prj_saved".to_string()),
            ]
        );
        assert_eq!(
            resolve_credential("VERCEL_TOKEN", process, synced),
            Some(("saved-token".to_string(), "syncedEnv"))
        );
        assert_eq!(
            resolve_credential("VERCEL_OIDC_TOKEN", process, synced),
            None
        );
    }

    #[test]
    fn job_env_drops_vercel_credentials() {
        let mut env: HashMap<String, String> = [
            "VERCEL_TOKEN",
            "VERCEL_TEAM_ID",
            "VERCEL_PROJECT_ID",
            "VERCEL_OIDC_TOKEN",
            "HF_TOKEN",
        ]
        .into_iter()
        .map(|k| (k.to_string(), "v".to_string()))
        .collect();
        strip_credentials(&mut env);
        assert_eq!(env.keys().collect::<Vec<_>>(), ["HF_TOKEN"]);
    }

    /// The Sandbox API only reports a process's exit once a client waits on it,
    /// so `status` must probe the wrapper's pid and then wait, never just read
    /// `returncode` (which stays `None` forever).
    #[tokio::test]
    async fn status_waits_for_the_exit_only_once_the_wrapper_is_gone() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(VERCEL_ENV.min_python, VERCEL_ENV.label).await
        else {
            eprintln!("skipped: no Python 3.10+ on PATH");
            return;
        };
        for (probe, expected) in [
            ("running", json!({"stage": "RUNNING"})),
            (
                "exited",
                json!({"stage": "ERROR", "message": "exited with code 3"}),
            ),
        ] {
            let dir = std::env::temp_dir().join(format!("orx-vercel-{}", uuid::Uuid::new_v4()));
            let pkg = dir.join("vercel").join("sandbox");
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(dir.join("vercel").join("__init__.py"), "").unwrap();
            std::fs::write(pkg.join("__init__.py"), "").unwrap();
            std::fs::write(
                pkg.join("sync.py"),
                format!(
                    r#"import sys
class Done:
    stdout = "{probe}\n"
class Proc:
    def __init__(self, rc): self.returncode = rc
class Box:
    status = "running"
    tags = {{}}
    def run_process(self, *a, **kw): return Done()
    def get_process(self, pid, wait=False):
        if not wait: return Proc(None)
        assert "{probe}" == "exited", "waited on a running job"
        return Proc(3)
    def update(self, tags): sys.stderr.write("tagged " + tags["orx_exit"])
def get_sandbox(**kw): return Box()
"#
                ),
            )
            .unwrap();
            let out = std::process::Command::new(python)
                .args(["-c", launcher(), "status", "orx-run-1", "cmd_1"])
                .env("PYTHONPATH", &dir)
                .output()
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert!(out.status.success(), "{probe}: {stderr}");
            assert_eq!(
                serde_json::from_slice::<Value>(&out.stdout).unwrap(),
                expected,
                "{probe}"
            );
            assert_eq!(stderr.contains("tagged 3"), probe == "exited", "{stderr}");
        }
    }

    /// Drive the launcher against a fake `vercel` package and check what it
    /// sends: no secrets in sandbox config, env file 0600, job logs to a file.
    #[tokio::test]
    async fn submit_keeps_secrets_out_of_sandbox_config() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(VERCEL_ENV.min_python, VERCEL_ENV.label).await
        else {
            eprintln!("skipped: no Python 3.10+ on PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-vercel-{}", uuid::Uuid::new_v4()));
        let pkg = dir.join("vercel").join("sandbox");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(dir.join("vercel").join("__init__.py"), "").unwrap();
        std::fs::write(
            pkg.join("__init__.py"),
            "class SandboxResources:\n    def __init__(self, vcpus=None, memory=None): self.vcpus = vcpus\n",
        )
        .unwrap();
        std::fs::write(
            pkg.join("sync.py"),
            r#"import sys, json
class FS:
    def write_text(self, path, text, mode=None):
        sys.stderr.write(json.dumps({"write": path, "mode": mode, "text": text}) + "\n")
class Proc:
    id = "cmd_1"
class Box:
    name = "orx-run-1"
    fs = FS()
    def create_process(self, cmd, args, **kw):
        sys.stderr.write(json.dumps({"process": args[1]}) + "\n")
        return Proc()
def create_sandbox(**kw):
    sys.stderr.write(json.dumps({"create": {k: v for k, v in kw.items() if k != "resources"},
                                 "vcpus": kw["resources"].vcpus}) + "\n")
    return Box()
def get_sandbox(**kw): return Box()
"#,
        )
        .unwrap();
        let mut child = std::process::Command::new(python)
            .args(["-c", launcher(), "submit"])
            .env("PYTHONPATH", &dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(
                br#"{"name": "orx-run-1", "script": "python train.py", "vcpus": 4,
                     "executionTimeLimit": 3900, "killAfter": 3600,
                     "env": {"HF_TOKEN": "s3cr'et"}}"#,
            )
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stderr}");
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            json!({"sandboxName": "orx-run-1", "processId": "cmd_1"})
        );
        let events: Vec<Value> = stderr
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let create = &events[0];
        assert_eq!(create["vcpus"], 4);
        assert_eq!(create["create"]["persistent"], false);
        assert!(create["create"].get("env").is_none(), "{create}");
        assert!(!create.to_string().contains("s3cr"), "{create}");
        let env_file = &events[1];
        assert_eq!(env_file["mode"], 0o600);
        assert_eq!(env_file["text"], "export HF_TOKEN='s3cr'\"'\"'et'\n");
        let process = events[2]["process"].as_str().unwrap();
        assert!(process.contains("( python train.py ) > /tmp/orx-run.log 2>&1"));
    }

    /// A missing sandbox is terminal. Raising would make supervise retry forever
    /// and leave the run `running`.
    #[tokio::test]
    async fn status_reports_deleted_when_the_sandbox_is_gone() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(VERCEL_ENV.min_python, VERCEL_ENV.label).await
        else {
            eprintln!("skipped: no Python 3.10+ on PATH");
            return;
        };
        for (status_code, deleted) in [(404, true), (500, false)] {
            let dir = std::env::temp_dir().join(format!("orx-vercel-{}", uuid::Uuid::new_v4()));
            let pkg = dir.join("vercel").join("sandbox");
            std::fs::create_dir_all(&pkg).unwrap();
            std::fs::write(dir.join("vercel").join("__init__.py"), "").unwrap();
            std::fs::write(
                pkg.join("__init__.py"),
                "class SandboxApiError(Exception):\n    def __init__(self, status_code):\n        super().__init__('http %s' % status_code)\n        self.status_code = status_code\n",
            )
            .unwrap();
            std::fs::write(
                pkg.join("sync.py"),
                format!(
                    "from vercel.sandbox import SandboxApiError\ndef get_sandbox(**kw):\n    raise SandboxApiError({status_code})\n"
                ),
            )
            .unwrap();
            let out = std::process::Command::new(python)
                .args(["-c", launcher(), "status", "orx-run-1", "cmd_1"])
                .env("PYTHONPATH", &dir)
                .output()
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if deleted {
                assert!(out.status.success(), "{status_code}: {stderr}");
                assert_eq!(
                    serde_json::from_slice::<Value>(stdout.as_bytes()).unwrap()["stage"],
                    "DELETED",
                    "{stdout}"
                );
            } else {
                assert!(!out.status.success(), "{status_code} should fail: {stdout}");
                assert!(!stdout.contains("DELETED"), "{stdout}");
            }
        }
    }

    #[tokio::test]
    async fn logs_command_uses_the_portable_follow_script() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(VERCEL_ENV.min_python, VERCEL_ENV.label).await
        else {
            eprintln!("skipped: no Python 3.10+ on PATH");
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-vercel-{}", uuid::Uuid::new_v4()));
        let pkg = dir.join("vercel").join("sandbox");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(dir.join("vercel").join("__init__.py"), "").unwrap();
        std::fs::write(pkg.join("__init__.py"), "").unwrap();
        std::fs::write(
            pkg.join("sync.py"),
            r#"import sys, json
class Proc:
    stdout = iter(["hello\n"])
    returncode = 0
class Box:
    status = "running"
    def create_process(self, cmd, args, **kw):
        sys.stderr.write(args[1])
        return Proc()
def get_sandbox(**kw):
    return Box()
"#,
        )
        .unwrap();
        let out = std::process::Command::new(python)
            .args(["-c", launcher(), "logs", "orx-run-1"])
            .env("PYTHONPATH", &dir)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stderr}");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\n");
        assert!(stderr.contains("--pid"), "{stderr}");
        assert!(stderr.contains("kill -0"), "{stderr}");
        assert!(stderr.contains("ORX_LOG_FOLLOW_ERROR:"), "{stderr}");
        assert!(
            stderr.contains("exec tail -n +1 -F --pid=\"$pid\" \"$log\"\n"),
            "{stderr}"
        );
    }

    #[test]
    fn log_follow_reads_until_the_pid_exits_with_gnu_tail() {
        let dir = std::env::temp_dir().join(format!("orx-vercel-follow-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let output = follow_log(None, &dir);
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "gnu follow failed: {stdout} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("first"), "{stdout}");
        assert!(stdout.contains("second"), "{stdout}");
    }

    #[test]
    fn log_follow_reads_until_the_pid_exits_without_gnu_tail() {
        let real_tail = std::process::Command::new("sh")
            .arg("-c")
            .arg("command -v tail")
            .output()
            .unwrap();
        let real_tail = String::from_utf8_lossy(&real_tail.stdout)
            .trim()
            .to_string();
        assert!(
            !real_tail.is_empty(),
            "tail is required for the follow test"
        );
        let dir = std::env::temp_dir().join(format!("orx-vercel-follow-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let stub = bin.join("tail");
        std::fs::write(
            &stub,
            format!(
                "#!/bin/sh\nfor arg in \"$@\"; do\n  case \"$arg\" in\n    --pid|--pid=*) echo unrecognized >&2; exit 1 ;;\n  esac\ndone\nexec {real_tail} \"$@\"\n"
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(&stub).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&stub, perms).unwrap();
        }
        let output = follow_log(Some(&bin), &dir);
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "fallback failed: {stdout} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("first"), "{stdout}");
        assert!(stdout.contains("second"), "{stdout}");
        assert!(!stdout.contains("ORX_LOG_FOLLOW_ERROR"), "{stdout}");
    }

    #[test]
    fn log_follow_errors_when_tail_cannot_read() {
        let dir = std::env::temp_dir().join(format!("orx-vercel-follow-{}", uuid::Uuid::new_v4()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let stub = bin.join("tail");
        std::fs::write(&stub, "#!/bin/sh\necho 'tail: not available' >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mut perms = std::fs::metadata(&stub).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&stub, perms).unwrap();
        }
        let output = follow_log(Some(&bin), &dir);
        let _ = std::fs::remove_dir_all(&dir);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!output.status.success(), "{stdout}");
        assert!(stdout.contains("ORX_LOG_FOLLOW_ERROR:"), "{stdout}");
    }

    fn follow_log(
        tail_dir: Option<&std::path::Path>,
        dir: &std::path::Path,
    ) -> std::process::Output {
        let log = dir.join("run.log");
        let pidfile = dir.join("run.pid");
        std::fs::write(&log, "first\n").unwrap();
        // Detach sleep so killing it is reaped by init. A direct child stays a
        // zombie, and `kill -0` on a zombie still succeeds, so the follow never ends.
        let launched = std::process::Command::new("bash")
            .args(["-c", "sleep 30 >/dev/null 2>&1 & echo $! > \"$1\""])
            .arg("bash")
            .arg(&pidfile)
            .status()
            .unwrap();
        assert!(launched.success());
        let pid = std::fs::read_to_string(&pidfile).unwrap();
        let pid = pid.trim().to_string();
        let log_for_append = log.clone();
        let pid_for_kill = pid.clone();
        let killer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(900));
            let _ = std::fs::write(&log_for_append, "first\nsecond\n");
            let _ = std::process::Command::new("kill")
                .arg(&pid_for_kill)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        });
        let mut cmd = std::process::Command::new("timeout");
        cmd.args(["15", "bash", "-c", LOG_FOLLOW_SH, "bash"])
            .arg(&log)
            .arg(&pidfile);
        if let Some(tail_dir) = tail_dir {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let mut next = std::ffi::OsString::from(tail_dir);
            next.push(":");
            next.push(path);
            cmd.env("PATH", next);
        }
        let output = cmd.output().unwrap();
        let _ = killer.join();
        let _ = std::process::Command::new("kill")
            .arg(&pid)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        output
    }
}
