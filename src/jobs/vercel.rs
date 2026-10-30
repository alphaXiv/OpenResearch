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
    sandbox, _ = _sdk()
    box = sandbox.get_sandbox(name=name)
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

def logs(name):
    sandbox, _ = _sdk()
    box = sandbox.get_sandbox(name=name)
    if _status(box) != "running":
        return
    # Replays from line 1 on every connect (supervise dedups by line count) and
    # ends once the job's wrapper process exits.
    follow = (
        "touch %s; while [ ! -s %s ]; do sleep 0.2; done; "
        "exec tail -n +1 -F --pid=\"$(cat %s)\" %s 2>/dev/null"
    ) % (LOG, PID, PID, LOG)
    proc = box.create_process("bash", ["-c", follow], stderr=subprocess.DEVNULL)
    for line in proc.stdout:
        if not line.endswith("\n"):
            line += "\n"
        sys.stdout.write(line)
        sys.stdout.flush()

def cancel(name):
    sandbox, _ = _sdk()
    box = sandbox.get_sandbox(name=name)
    if _status(box) in ("pending", "running"):
        box.stop()

def finish(name):
    sandbox, _ = _sdk()
    box = sandbox.get_sandbox(name=name)
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

/// Headroom past the job's own kill deadline, so the final log lines and the
/// exit status can still be read before Vercel stops the session.
const SESSION_GRACE_SECS: u64 = 5 * 60;

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

/// A credential's value and where it came from: the process env wins, then the
/// synced env file (`orx up` Settings → Environment).
fn credential(key: &str) -> Option<(String, &'static str)> {
    if let Ok(v) = std::env::var(key) {
        if !v.trim().is_empty() {
            return Some((v, "env"));
        }
    }
    crate::config::synced_env_var(key)
        .filter(|v| !v.trim().is_empty())
        .map(|v| (v, "syncedEnv"))
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

/// Credentials to inject into the launcher subprocess: synced values the
/// inherited process env doesn't already carry.
fn auth_env() -> Vec<(String, String)> {
    std::iter::once(OIDC_TOKEN)
        .chain(ACCESS_TOKEN_KEYS)
        .filter(|key| std::env::var_os(key).is_none())
        .filter_map(|key| crate::config::synced_env_var(key).map(|v| (key.to_string(), v)))
        .collect()
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
    anyhow!(
        "vercel sandbox {} failed{}: {}",
        what,
        code.map(|c| format!(" (exit {c})")).unwrap_or_default(),
        detail
    )
}

async fn launcher_capture(args: &[&str], stdin: Option<&str>) -> Result<Vec<u8>> {
    let mut cmd = Command::new(python_bin());
    cmd.arg("-c")
        .arg(LAUNCHER)
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
        "executionTimeLimit": session_limit(spec.timeout_seconds),
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

/// The session limit for a job deadline: the deadline plus grace, capped.
pub fn session_limit(timeout_seconds: u64) -> u64 {
    timeout_seconds
        .saturating_add(SESSION_GRACE_SECS)
        .min(MAX_TIMEOUT_SECS)
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
        .arg(LAUNCHER)
        .args(["logs", sandbox_name])
        .env(super::PYTHONUNBUFFERED, "1")
        .envs(auth_env())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| missing_python_hint(&e))?;

    let stdout = child.stdout.take().expect("piped stdout");
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
                    return Err(anyhow!(
                        "the Vercel Sandbox log launcher exited with {}",
                        status
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
    fn session_limit_adds_grace_and_caps() {
        assert_eq!(session_limit(3600), 3600 + 300);
        assert_eq!(session_limit(MAX_TIMEOUT_SECS), MAX_TIMEOUT_SECS);
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
                .args(["-c", LAUNCHER, "status", "orx-run-1", "cmd_1"])
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
            .args(["-c", LAUNCHER, "submit"])
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
}
