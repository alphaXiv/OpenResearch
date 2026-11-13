//! Modal Sandboxes backend — ephemeral serverless containers on Modal.com.
//!
//! Modal is Python-SDK-driven (there is no generic REST job-submit API), so
//! every operation shells out to a bundled Python launcher that drives
//! `modal.Sandbox`. Sandboxes are reconnectable by id (`Sandbox.from_id`),
//! which is exactly what lets the engine reattach from a
//! fresh process — the same reattach model the HF and k8s backends rely on.
//!
//! Transport is `<python> -c <LAUNCHER> <subcommand> [args]`, mirroring how the
//! k8s backend shells out to `kubectl`. The interpreter is an orx-managed venv
//! with the `modal` SDK, auto-provisioned on first launch (`ensure_env`), so the
//! user never has to pip-install `modal` or juggle `$ORX_MODAL_PYTHON`. Auth is
//! Modal's own: MODAL_TOKEN_ID / MODAL_TOKEN_SECRET (process env or the synced
//! `~/.openresearch/env`) or `~/.modal.toml`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::managed_env::ManagedEnv;
use crate::error::{anyhow, Result};

/// Bundled Python launcher. Subcommands (argv[1]):
///   submit          spec JSON on stdin -> prints {"sandboxId": "sb-..."}
///   status <id>     -> prints {"stage": "...", "message": "..."?}, or {"missing": true}
///   logs <id> <from>  streams the sandbox's merged stdout/stderr from a resume point, as
///                     frames: a JSON `[from, length]` line, then that many raw bytes
///   cancel <id>     terminate the sandbox -> prints {"missing": bool}
///   auth            one authenticated call that checks the configured token
///
/// Exit 3 = the `modal` package isn't importable (surfaced as a friendly hint),
/// 4 = Modal rejected the token, 5 = the token check could not reach Modal,
/// 6 = Modal refused the sandbox, or submit terminated it (`jobs::Refused` on stderr).
const LAUNCHER: &str = r#"
import sys, json

def _modal():
    try:
        import modal
        return modal
    except Exception:
        sys.stderr.write("modal-not-installed")
        sys.exit(3)

def _refuse(message):
    sys.stderr.write(json.dumps({"status": None, "message": message}))
    sys.exit(6)

def _sandbox(modal, spec):
    app = modal.App.lookup(spec.get("app", "openresearch"), create_if_missing=True)
    # Registry images rarely ship uv; append it to PATH so an image's own uv still wins.
    # The run command replaces an image ENTRYPOINT (vllm-openai's is `vllm serve`), as on k8s.
    image = modal.Image.from_registry(spec["image"]).dockerfile_commands(
        "COPY --from=ghcr.io/astral-sh/uv:0.12.23 /uv /uvx /opt/orx-uv/",
        "ENV PATH=$PATH:/opt/orx-uv",
        "ENTRYPOINT []",
    )
    kwargs = {
        "app": app,
        "image": image,
        "timeout": int(spec.get("timeoutSeconds", 3600)),
    }
    if spec.get("gpu"):
        kwargs["gpu"] = spec["gpu"]
    if spec.get("cpu"):
        kwargs["cpu"] = float(spec["cpu"])
    if spec.get("memory"):
        kwargs["memory"] = int(spec["memory"])
    env = spec.get("env") or {}
    if env:
        # Tokens ride an ephemeral Secret, never the plain `env` arg (which
        # shows up in the Modal dashboard).
        kwargs["secrets"] = [modal.Secret.from_dict(env)]
    tags = spec.get("tags") or {}
    if tags:
        kwargs["tags"] = tags
    # Only stdout reaches the run log, so even a failure to start bash lands there.
    return modal.Sandbox.create(
        "sh", "-c", 'exec 2>&1; exec bash -c "$1"', "orx", spec["script"], **kwargs
    )

def submit():
    modal = _modal()
    spec = json.load(sys.stdin)
    try:
        sb = _sandbox(modal, spec)
    except Exception as error:
        from modal.exception import AuthError, InvalidError
        if not isinstance(error, (AuthError, InvalidError)):
            raise
        _refuse("Modal refused the sandbox: %s" % (str(error) or type(error).__name__))
    if spec.get("sourceArchive"):
        try:
            sb.filesystem.copy_from_local(spec["sourceArchive"], "/tmp/orx-source.tar")
            sb.exec("touch", "/tmp/orx-source.tar.ready").wait()
        except Exception as error:
            code = sb.poll()
            sb.terminate()
            if code is None:
                message = "Could not copy the run's source into the Modal sandbox: %s" % (
                    str(error) or type(error).__name__)
            else:
                try:
                    output = "".join(sb.stdout.read().splitlines(True)[-20:])
                except Exception:
                    output = ""
                message = "The Modal sandbox exited with code %d before its source arrived.\n%s" % (
                    code, output)
            _refuse(message)
    print(json.dumps({"sandboxId": sb.object_id}))

def _missing(error):
    from modal.exception import NotFoundError
    return isinstance(error, NotFoundError)

def status(sid):
    modal = _modal()
    try:
        code = modal.Sandbox.from_id(sid).poll()  # None while running, exit code once finished
    except Exception as error:
        if not _missing(error):
            raise
        print(json.dumps({"missing": True}))
        return
    if code is None:
        out = {"stage": "RUNNING"}
    elif code == 0:
        out = {"stage": "COMPLETED"}
    else:
        out = {"stage": "ERROR", "message": "exited with code %d" % code}
    out["exitCode"] = code
    print(json.dumps(out))

async def _positioned(reader, start):
    # Each chunk comes with the point a reconnect passes to replay from that chunk's first byte.
    from modal import io_streams
    impl = getattr(reader, "_impl", None)
    server = getattr(io_streams, "_StreamReaderThroughServer", ())
    router = getattr(io_streams, "_stdio_stream_from_command_router", None)
    if isinstance(impl, server) and hasattr(impl, "_last_entry_id"):
        # An entry id names a batch; reading from it resumes after the whole batch.
        impl._last_entry_id = group = point = start
        async for data in impl._get_logs():
            if impl._last_entry_id != group:
                point, group = group, impl._last_entry_id
            yield point, data
    elif router is not None and hasattr(impl, "_params"):
        async for data, end in router(impl._params, int(start or 0)):
            yield str(end - len(data)), data
    else:
        async for text in reader:
            yield "", text.encode()

def logs(sid, start):
    modal = _modal()
    from modal._utils.async_utils import synchronizer
    reader = modal.Sandbox.from_id(sid).stdout
    out = sys.stdout.buffer
    for point, data in synchronizer.create_blocking(_positioned)(reader, start):
        out.write(json.dumps([point, len(data)]).encode() + b"\n" + data)
        out.flush()

def cancel(sid):
    modal = _modal()
    try:
        modal.Sandbox.from_id(sid).terminate()
    except Exception as error:
        if not _missing(error):
            raise
        print(json.dumps({"missing": True}))
        return
    print(json.dumps({"missing": False}))

def auth():
    modal = _modal()
    from modal.exception import AuthError
    try:
        modal.Client.from_env().hello()
    except AuthError as error:
        sys.stderr.write(str(error))
        sys.exit(4)
    except Exception as error:
        sys.stderr.write(str(error) or type(error).__name__)
        sys.exit(5)

cmd = sys.argv[1] if len(sys.argv) > 1 else ""
if cmd == "submit":
    submit()
elif cmd == "status":
    status(sys.argv[2])
elif cmd == "logs":
    logs(sys.argv[2], sys.argv[3])
elif cmd == "cancel":
    cancel(sys.argv[2])
elif cmd == "auth":
    auth()
else:
    sys.stderr.write("unknown launcher subcommand: %r" % cmd)
    sys.exit(2)
"#;

/// `Sandbox.create(tags=...)` arrived in modal 1.4.3, which requires Python 3.10.
pub(crate) const MODAL_ENV: ManagedEnv = ManagedEnv {
    name: "modal",
    label: "Modal",
    min_python: (3, 10),
    requirement: "modal>=1.4.3",
    ready_check:
        "import inspect, modal; assert 'tags' in inspect.signature(modal.Sandbox.create).parameters",
    override_var: Some("ORX_MODAL_PYTHON"),
};

/// Ensure a usable Modal interpreter exists, provisioning the orx-managed venv
/// on first use so `--backend modal` "just works" without the user pip-installing
/// `modal` or setting `ORX_MODAL_PYTHON`. Idempotent and fast once built; a
/// no-op when `ORX_MODAL_PYTHON` is set (the user owns that interpreter).
/// Prints one-time progress to stderr. Only the launch path calls this — the
/// supervisor relies on the env already being there.
pub async fn ensure_env() -> Result<()> {
    if MODAL_ENV.override_python().is_some() {
        return Ok(());
    }
    MODAL_ENV.ensure().await.map_err(|e| {
        anyhow!(
            "{}. Or set ORX_MODAL_PYTHON to an interpreter with {}.",
            e.to_string().trim_end_matches('.'),
            MODAL_ENV.requirement
        )
    })?;
    Ok(())
}

/// Modal auth to inject into the launcher subprocess: the inherited process env
/// wins, otherwise the box's synced env file (`~/.openresearch/env`) — so creds
/// set in `orx up` Settings → Environment reach Modal even when orx's own
/// process env doesn't carry them. (`~/.modal.toml` is picked up by the SDK
/// directly and needs nothing here.)
fn modal_auth_env() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for key in ["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"] {
        if std::env::var_os(key).is_some() {
            continue; // already inherited
        }
        if let Some(v) = crate::config::synced_env_var(key) {
            out.push((key.to_string(), v));
        }
    }
    out
}

fn missing_python_hint(e: &std::io::Error) -> crate::error::Error {
    if e.kind() == std::io::ErrorKind::NotFound {
        anyhow!(
            "{} not found on PATH — Modal is driven through its Python SDK. Install Python 3 \
             and `pip install modal`, then `modal token new` (or set MODAL_TOKEN_ID / \
             MODAL_TOKEN_SECRET). Point orx at a specific interpreter with ORX_MODAL_PYTHON.",
            MODAL_ENV.interpreter()
        )
    } else {
        anyhow!("Could not run {}: {}", MODAL_ENV.interpreter(), e)
    }
}

/// Map a launcher non-zero exit into a useful error (exit codes at `LAUNCHER`).
fn launcher_error(what: &str, code: Option<i32>, stderr: &str) -> crate::error::Error {
    let stderr = stderr.trim();
    if code == Some(3) || stderr == "modal-not-installed" {
        return anyhow!(
            "The `modal` Python package isn't importable by {}. Run `pip install modal` \
             (and `modal token new`), or set ORX_MODAL_PYTHON to an interpreter that has it.",
            MODAL_ENV.interpreter()
        );
    }
    if code == Some(4) {
        let source = match token_source() {
            Some("env") => "orx up's environment",
            Some("syncedEnv") => "Settings → Environment",
            _ => "~/.modal.toml",
        };
        return anyhow!(
            "Modal didn't accept the token in {source} ({stderr}). Run `modal token new`, or \
             replace MODAL_TOKEN_ID and MODAL_TOKEN_SECRET in {source}, then run \
             `orx compute test modal`."
        );
    }
    if let Some(refused) = super::launcher_refusal(code, stderr) {
        return refused.into();
    }
    if code == Some(5) {
        return anyhow!(
            "Could not reach Modal to check the token ({stderr}). Check your connection and run \
             `orx compute test modal` again."
        );
    }
    anyhow!(
        "modal {} failed{}: {}",
        what,
        code.map(|c| format!(" (exit {c})")).unwrap_or_default(),
        if stderr.is_empty() {
            "no stderr"
        } else {
            stderr
        }
    )
}

/// Run the launcher and capture its output. A launcher past `limit` is killed and fails with
/// exit 5's error.
async fn launcher_capture(
    args: &[&str],
    stdin: Option<&str>,
    limit: Option<Duration>,
    fence: Option<&crate::engine::Fence<'_>>,
) -> Result<Vec<u8>> {
    let mut cmd = crate::process::tokio_command(MODAL_ENV.interpreter());
    cmd.arg("-c")
        .arg(LAUNCHER)
        .args(args)
        // Unbuffer the launcher's own stdout: we read it line-by-line, so block
        // buffering (stdout is a pipe here, not a TTY) would stall log streaming.
        .env(super::PYTHONUNBUFFERED, "1")
        .envs(modal_auth_env())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(limit.is_some());
    let mut child = cmd.spawn().map_err(|e| missing_python_hint(&e))?;
    if let Some(fence) = fence {
        fence.record_child(&mut child).await?;
    }
    if let Some(input) = stdin {
        use tokio::io::AsyncWriteExt as _;
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(input.as_bytes()).await;
            drop(pipe); // EOF
        }
    }
    let output = child.wait_with_output();
    let out = match limit {
        Some(limit) => tokio::time::timeout(limit, output).await.map_err(|_| {
            launcher_error(
                args.first().copied().unwrap_or("?"),
                Some(5),
                &format!("no answer within {} seconds", limit.as_secs()),
            )
        })?,
        None => output.await,
    }
    .map_err(|e| anyhow!("Could not run {}: {}", MODAL_ENV.interpreter(), e))?;
    if !out.status.success() {
        return Err(launcher_error(
            args.first().copied().unwrap_or("?"),
            out.status.code(),
            &String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(out.stdout)
}

/// Resources resolved from a flavor name.
pub struct ModalResources {
    /// Modal GPU spec (e.g. "A10G", "A100-80GB", "H100:2"), or `None` for CPU.
    pub gpu: Option<String>,
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
}

/// Map a `--flavor` name to Modal resources. CPU flavors (name starts with
/// "cpu") get no GPU; anything else is treated as a Modal GPU spec and passed
/// through uppercased, so `--flavor a100-80gb` and `--flavor h100:2` both work.
pub fn resolve_flavor(name: &str) -> ModalResources {
    let n = name.trim();
    if n.is_empty() || n.eq_ignore_ascii_case("cpu") {
        return ModalResources {
            gpu: None,
            cpu: None,
            memory: None,
        };
    }
    if let Some(rest) = n.strip_prefix("cpu-").or_else(|| n.strip_prefix("cpu_")) {
        // cpu-large / cpu-xlarge -> a bit more CPU/RAM; otherwise defaults.
        let (cpu, memory) = match rest.to_ascii_lowercase().as_str() {
            "large" => (Some(8.0), Some(32768)),
            "xlarge" => (Some(16.0), Some(65536)),
            _ => (None, None),
        };
        return ModalResources {
            gpu: None,
            cpu,
            memory,
        };
    }
    ModalResources {
        gpu: Some(n.to_ascii_uppercase()),
        cpu: None,
        memory: None,
    }
}

/// Default docker image: plain python for CPU, a CUDA-ready pytorch image for
/// GPU — same families as the HF/k8s defaults.
pub fn default_image(gpu: bool) -> String {
    if gpu {
        "pytorch/pytorch:2.6.0-cuda12.4-cudnn9-runtime".to_string()
    } else {
        "python:3.12".to_string()
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ModalJobSpec {
    /// `bash -c` payload (the shared snapshot-and-run script).
    pub script: String,
    pub image: String,
    pub gpu: Option<String>,
    pub cpu: Option<f64>,
    pub memory: Option<u64>,
    /// Passed to the sandbox as an ephemeral Secret (tokens, synced env).
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub timeout_seconds: u64,
    /// Modal app name to group sandboxes under.
    pub app: String,
    /// Sandbox tags (or_run / or_experiment / or_project) for observability.
    pub tags: HashMap<String, String>,
    pub source_archive: Option<PathBuf>,
}

/// Submit the sandbox; returns its object id (the reattach handle). The launcher waits for its
/// spec on stdin, so it creates nothing before it is recorded on `fence`.
pub async fn run_job(
    spec: &ModalJobSpec,
    fence: Option<&crate::engine::Fence<'_>>,
) -> Result<String> {
    // Default the sandbox's Python to unbuffered so the job's own prints stream
    // live instead of block-buffering behind a pipe (see jobs::default_python_env).
    let env = super::default_python_env(&spec.env);
    let body = json!({
        "app": spec.app,
        "image": spec.image,
        "script": spec.script,
        "gpu": spec.gpu,
        "cpu": spec.cpu,
        "memory": spec.memory,
        "env": env,
        "timeoutSeconds": spec.timeout_seconds,
        "tags": spec.tags,
        "sourceArchive": spec.source_archive,
    });
    let out = launcher_capture(&["submit"], Some(&body.to_string()), None, fence).await?;
    let v: Value = serde_json::from_slice(&out).map_err(|e| {
        anyhow!(
            "Unreadable modal submit output: {} ({:?})",
            e,
            String::from_utf8_lossy(&out)
        )
    })?;
    v.get("sandboxId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("modal submit returned no sandboxId"))
}

/// Sandbox state in the shared stage vocabulary (see `jobs::stage_to_run_status`).
#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
    pub exit_code: Option<i32>,
}

/// `None` when Modal raises `NotFoundError`, which it does both for a deleted sandbox and for one
/// launched under another workspace or environment.
pub async fn inspect_job(sandbox_id: &str) -> Result<Option<JobState>> {
    let out = launcher_capture(&["status", sandbox_id], None, None, None).await?;
    let v: Value = serde_json::from_slice(&out)
        .map_err(|e| anyhow!("Unreadable modal status output: {}", e))?;
    if v["missing"] == true {
        return Ok(None);
    }
    Ok(Some(JobState {
        stage: v
            .get("stage")
            .and_then(Value::as_str)
            .unwrap_or("RUNNING")
            .to_string(),
        message: v.get("message").and_then(Value::as_str).map(str::to_string),
        exit_code: v
            .get("exitCode")
            .and_then(Value::as_i64)
            .and_then(|code| code.try_into().ok()),
    }))
}

/// False when Modal raises `NotFoundError`, as for [`inspect_job`].
pub async fn cancel_job(sandbox_id: &str) -> Result<bool> {
    let out = launcher_capture(&["cancel", sandbox_id], None, None, None).await?;
    let v: Value = serde_json::from_slice(&out)
        .map_err(|e| anyhow!("Unreadable modal cancel output: {}", e))?;
    Ok(v["missing"] != true)
}

/// Where the sandbox log left off: the provider position a reconnect replays from, and how
/// many bytes past it were already written. Modal names positions by log batch on V1 sandboxes
/// and by byte offset on V2; an SDK without either replays from the start.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogResume {
    from: String,
    kept: u64,
}

impl LogResume {
    /// Past the first `kept` bytes of the whole log.
    pub fn from_start(kept: u64) -> Self {
        Self {
            from: String::new(),
            kept,
        }
    }
}

/// Follows the sandbox's log stream from `resume`, handing `sink` its bytes as Modal sent
/// them. True when the stream reached the sandbox's end; false after `idle` without output.
pub async fn stream_logs(
    sandbox_id: &str,
    resume: &mut LogResume,
    idle: Duration,
    sink: &mut (dyn FnMut(&[u8]) + Send),
) -> Result<bool> {
    use tokio::io::{AsyncBufReadExt as _, AsyncReadExt as _, BufReader};

    let mut child = crate::process::tokio_command(MODAL_ENV.interpreter())
        .arg("-c")
        .arg(LAUNCHER)
        .args(["logs", sandbox_id, &resume.from])
        .envs(modal_auth_env())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| missing_python_hint(&e))?;
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr = tokio::spawn(async move {
        let mut text = Vec::new();
        let _ = stderr.read_to_end(&mut text).await;
        text
    });

    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    // Bytes the replay resends before any new ones, which may span several positions.
    let mut replayed = resume.kept;
    let (mut point, mut past) = (resume.from.clone(), 0u64);
    loop {
        let mut header = Vec::new();
        let frame = async {
            if stdout.read_until(b'\n', &mut header).await? == 0 {
                return Ok(None);
            }
            let (from, len): (String, usize) = serde_json::from_slice(&header)?;
            let mut data = vec![0; len];
            stdout.read_exact(&mut data).await?;
            Ok::<_, crate::error::Error>(Some((from, data)))
        };
        let (from, data) = match tokio::time::timeout(idle, frame).await {
            Err(_) => return Ok(false),
            Ok(Err(e)) => {
                return Err(anyhow!(
                    "Could not read the Modal log launcher output: {}",
                    e
                ))
            }
            Ok(Ok(Some(frame))) => frame,
            Ok(Ok(None)) => {
                let status = child.wait().await?;
                if !status.success() {
                    let stderr = stderr.await.unwrap_or_default();
                    return Err(launcher_error(
                        "logs",
                        status.code(),
                        &String::from_utf8_lossy(&stderr),
                    ));
                }
                return Ok(true);
            }
        };
        if from != point {
            point = from;
            past = 0;
        }
        past += data.len() as u64;
        let seen = replayed.min(data.len() as u64);
        replayed -= seen;
        let fresh = &data[seen as usize..];
        if !fresh.is_empty() {
            sink(fresh);
            resume.from.clone_from(&point);
            resume.kept = past;
        }
    }
}

/// Modal's client keeps retrying an unreachable server for about a minute.
const TOKEN_CHECK_LIMIT: Duration = Duration::from_secs(10);

/// Detected Modal readiness for preflight.
pub struct ModalStatus {
    pub modal_importable: bool,
    pub token_configured: bool,
    pub error: Option<String>,
}

/// Fail fast before doing any source staging or provider submission if
/// Modal isn't usable — provisioning the orx-managed env on first use. Returns
/// a friendly, actionable error otherwise.
/// `caller_overrides` come from the calling shell.
pub async fn preflight(
    caller_overrides: &std::collections::BTreeMap<String, String>,
) -> Result<()> {
    MODAL_ENV.require_caller_override(caller_overrides)?;
    // Build the managed venv on first launch (no-op once it exists).
    ensure_env().await?;
    let s = detect().await;
    if !s.modal_importable {
        return Err(anyhow!(
            "The `modal` SDK isn't importable by {} ({}). Remove {} to force a rebuild, or set \
             ORX_MODAL_PYTHON to an interpreter that has it.",
            MODAL_ENV.interpreter(),
            s.error.as_deref().unwrap_or("import modal failed"),
            MODAL_ENV.dir().display()
        ));
    }
    if !s.token_configured {
        return Err(anyhow!(
            "No Modal token configured. Run `modal token new`, or set MODAL_TOKEN_ID and \
             MODAL_TOKEN_SECRET in the process environment or Settings → Environment."
        ));
    }
    launcher_capture(
        &["auth"],
        None,
        Some(crate::config::paced(TOKEN_CHECK_LIMIT)),
        None,
    )
    .await?;
    Ok(())
}

/// Where a Modal token would come from, if anywhere: process env, the synced
/// env file, or `~/.modal.toml`. Sync and cheap (env + fs only) — safe for the
/// compute-summary endpoint, which must not pay `detect()`'s python probe.
pub fn token_source() -> Option<&'static str> {
    resolve_token().ok().flatten().map(|token| token.source)
}

pub struct ModalToken {
    pub id: String,
    pub secret: String,
    pub source: &'static str,
}

fn token_profile(config: &toml::Table, requested: Option<&str>) -> Result<toml::Table> {
    let active: Vec<_> = config
        .iter()
        .filter(|(_, value)| value.get("active").and_then(toml::Value::as_bool) == Some(true))
        .map(|(name, _)| name.as_str())
        .collect();
    let profile = requested
        .or_else(|| active.first().copied())
        .unwrap_or("default");
    if active.len() > 1 || (config.len() > 1 && active.is_empty() && profile == "default") {
        return Err(anyhow!(
            "Select an active Modal profile with `modal profile activate`."
        ));
    }
    if config.values().any(|value| !value.is_table()) {
        return Err(anyhow!(
            "Modal configuration must contain a table for each profile."
        ));
    }
    Ok(config
        .get(profile)
        .and_then(toml::Value::as_table)
        .cloned()
        .unwrap_or_default())
}

pub fn resolve_token() -> Result<Option<ModalToken>> {
    let path = std::env::var("MODAL_CONFIG_PATH")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::config::home_dir().map(|home| home.join(".modal.toml")))
        .ok_or_else(|| anyhow!("Could not locate the Modal configuration."))?;
    let config = match std::fs::read_to_string(path) {
        Ok(body) => body.parse::<toml::Table>().map_err(|_| {
            anyhow!("Could not read Modal configuration. Check it with `modal profile list`.")
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => toml::Table::new(),
        Err(_) => {
            return Err(anyhow!(
                "Could not read Modal configuration. Check file permissions."
            ))
        }
    };
    let requested = std::env::var("MODAL_PROFILE")
        .ok()
        .filter(|p| !p.is_empty());
    let profile = token_profile(&config, requested.as_deref())?;
    let mut source = "modalToml";
    let mut resolve = |key: &str, field: &str| {
        if let Ok(value) = std::env::var(key) {
            source = "env";
            Some(value)
        } else if let Some(value) = crate::config::synced_env_var(key) {
            if source != "env" {
                source = "syncedEnv";
            }
            Some(value)
        } else {
            profile
                .get(field)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        }
    };
    let id = resolve("MODAL_TOKEN_ID", "token_id");
    let secret = resolve("MODAL_TOKEN_SECRET", "token_secret");
    match (id, secret) {
        (Some(id), Some(secret)) if !id.trim().is_empty() && !secret.trim().is_empty() => {
            Ok(Some(ModalToken { id, secret, source }))
        }
        _ => Ok(None),
    }
}

/// Best-effort probe: is a Modal interpreter present, is `modal` importable, and
/// is a token configured (process env, the synced env file, or `~/.modal.toml`)?
pub async fn detect() -> ModalStatus {
    let token_source = token_source();
    let token_configured = token_source.is_some();
    let mk = |modal_importable, error| ModalStatus {
        modal_importable,
        token_configured,
        error,
    };
    let probe = crate::process::tokio_command(MODAL_ENV.interpreter())
        .arg("-c")
        .arg("import modal")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .await;
    match probe {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => mk(
            false,
            Some(format!("{} not found on PATH", MODAL_ENV.interpreter())),
        ),
        Err(e) => mk(false, Some(e.to_string())),
        Ok(out) if out.status.success() => mk(true, None),
        Ok(out) => mk(
            false,
            Some(
                String::from_utf8_lossy(&out.stderr)
                    .lines()
                    .last()
                    .unwrap_or("`import modal` failed")
                    .to_string(),
            ),
        ),
    }
}

#[cfg(test)]
mod credential_tests {
    use super::*;
    use std::io::Write as _;

    #[tokio::test]
    async fn status_keeps_the_sandbox_exit_code() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-modal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (code, stage) in [("None", "RUNNING"), ("0", "COMPLETED"), ("7", "ERROR")] {
            std::fs::write(dir.join("modal.py"), format!(
                "class Sandbox:\n    @staticmethod\n    def from_id(sid): return Sandbox()\n    def poll(self): return {code}\n"
            )).unwrap();
            let output = crate::process::command(python)
                .args(["-B", "-c", LAUNCHER, "status", "sandbox"])
                .env("PYTHONPATH", &dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let state: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(state["stage"], stage);
            assert_eq!(state["exitCode"].as_i64(), code.parse::<i64>().ok());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A sandbox Modal can't find is reported missing, which proves nothing about deletion;
    /// any other failure stays one.
    #[tokio::test]
    async fn a_missing_sandbox_is_reported_missing() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            crate::config::skip_without_tool("Python 3.10+");
            return;
        };
        for (error, missing) in [("NotFoundError", true), ("ConnectionError", false)] {
            let dir = std::env::temp_dir().join(format!("orx-modal-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("modal")).unwrap();
            std::fs::write(
                dir.join("modal").join("__init__.py"),
                format!(
                    "from .exception import NotFoundError\nclass Sandbox:\n    @staticmethod\n    def from_id(sid): raise {error}(sid)\n"
                ),
            )
            .unwrap();
            std::fs::write(
                dir.join("modal").join("exception.py"),
                "class NotFoundError(Exception): pass\n",
            )
            .unwrap();
            let launch = |command: &str| {
                crate::process::command(python)
                    .args(["-B", "-c", LAUNCHER, command, "sandbox"])
                    .env("PYTHONPATH", &dir)
                    .output()
                    .unwrap()
            };
            let (status, cancel) = (launch("status"), launch("cancel"));
            std::fs::remove_dir_all(&dir).unwrap();
            assert_eq!(status.status.success(), missing, "{error}: {status:?}");
            assert_eq!(cancel.status.success(), missing, "{error}: {cancel:?}");
            if missing {
                for output in [status, cancel] {
                    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
                    assert_eq!(reply["missing"], true, "{output:?}");
                }
            }
        }
    }

    /// A fake of the SDK pieces the launcher reads positions from: entry-id batches on V1
    /// sandboxes, byte offsets on V2.
    const FAKE_SDK: &[(&str, &str)] = &[
        (
            "modal/__init__.py",
            r#"from . import io_streams
class Sandbox:
    @staticmethod
    def from_id(sid):
        sandbox = Sandbox()
        sandbox.stdout = type("Reader", (), {})()
        sandbox.stdout._impl = io_streams._StreamReaderThroughServer() if sid == "v1" else io_streams.Router()
        return sandbox
"#,
        ),
        (
            "modal/io_streams.py",
            r#"BATCHES = [("1", [b"A\n"]), ("2", [b"BIG1", b"BIG2\n"]), ("3", [b"B"])]
OUTPUT = b"hello \xe2\x9c\x93\n"
class _StreamReaderThroughServer:
    _last_entry_id = ""
    async def _get_logs(self):
        ids = [batch for batch, _ in BATCHES]
        start = ids.index(self._last_entry_id) + 1 if self._last_entry_id else 0
        for batch, items in BATCHES[start:]:
            for item in items:
                self._last_entry_id = batch
                yield item
class Router:
    _params = None
async def _stdio_stream_from_command_router(params, offset):
    while offset < len(OUTPUT):
        data = OUTPUT[offset:offset + 4]
        offset += len(data)
        yield data, offset
"#,
        ),
        ("modal/_utils/__init__.py", ""),
        (
            "modal/_utils/async_utils.py",
            r#"import asyncio
class Synchronizer:
    def create_blocking(self, function):
        def blocking(*args):
            loop, items = asyncio.new_event_loop(), function(*args)
            while True:
                try:
                    yield loop.run_until_complete(items.__anext__())
                except StopAsyncIteration:
                    return
        return blocking
synchronizer = Synchronizer()
"#,
        ),
    ];

    #[tokio::test]
    async fn logs_frames_carry_the_point_each_chunk_replays_from() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            crate::config::skip_without_tool("Python 3.10+");
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-modal-logs-{}", uuid::Uuid::new_v4()));
        for (path, body) in FAKE_SDK {
            std::fs::create_dir_all(dir.join(path).parent().unwrap()).unwrap();
            std::fs::write(dir.join(path), body).unwrap();
        }
        let frames = |sandbox: &str, from: &str| {
            let output = crate::process::command(python)
                .args(["-B", "-c", LAUNCHER, "logs", sandbox, from])
                .env("PYTHONPATH", &dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let mut stdout = output.stdout.as_slice();
            let mut frames = Vec::new();
            while let Some(end) = stdout.iter().position(|b| *b == b'\n') {
                let (from, len): (String, usize) = serde_json::from_slice(&stdout[..end]).unwrap();
                frames.push((from, stdout[end + 1..end + 1 + len].to_vec()));
                stdout = &stdout[end + 1 + len..];
            }
            frames
        };
        let owned = |frames: &[(&str, &[u8])]| -> Vec<(String, Vec<u8>)> {
            frames
                .iter()
                .map(|(from, data)| (from.to_string(), data.to_vec()))
                .collect()
        };
        assert_eq!(
            frames("v1", ""),
            owned(&[("", b"A\n"), ("1", b"BIG1"), ("1", b"BIG2\n"), ("2", b"B")])
        );
        assert_eq!(
            frames("v1", "1"),
            owned(&[("1", b"BIG1"), ("1", b"BIG2\n"), ("2", b"B")])
        );
        assert_eq!(
            frames("v2", "4"),
            owned(&[("4", b"o \xe2\x9c"), ("8", b"\x93\n")])
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selects_active_or_explicit_profile_and_rejects_ambiguity() {
        let config: toml::Table =
            "[default]\ntoken_id = 'old'\n[work]\nactive = true\ntoken_id = 'current'"
                .parse()
                .unwrap();
        assert_eq!(
            token_profile(&config, None).unwrap()["token_id"].as_str(),
            Some("current")
        );
        assert_eq!(
            token_profile(&config, Some("default")).unwrap()["token_id"].as_str(),
            Some("old")
        );
        let ambiguous: toml::Table = "[one]\n[other]".parse().unwrap();
        assert!(token_profile(&ambiguous, None).is_err());
        assert!(token_profile(&toml::Table::new(), None).unwrap().is_empty());
    }

    #[tokio::test]
    async fn ready_check_requires_sandbox_tags() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            eprintln!(
                "skipped: no Python {}.{}+ on PATH",
                MODAL_ENV.min_python.0, MODAL_ENV.min_python.1
            );
            return;
        };
        for (params, ready) in [
            ("*args, app=None", false),
            ("*args, app=None, tags=None", true),
        ] {
            let dir = std::env::temp_dir().join(format!("orx-modal-env-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join("modal")).unwrap();
            std::fs::write(
                dir.join("modal").join("__init__.py"),
                format!("class Sandbox:\n    @classmethod\n    def create(cls, {params}):\n        pass\n"),
            )
            .unwrap();
            let status = crate::process::command(python)
                .args(["-c", MODAL_ENV.ready_check])
                .env("PYTHONPATH", &dir)
                .stderr(Stdio::null())
                .status()
                .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            assert_eq!(status.success(), ready, "create({params})");
        }
    }

    #[tokio::test]
    async fn submit_prepares_the_registry_image_and_merges_stderr() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            eprintln!(
                "skipped: no Python {}.{}+ on PATH",
                MODAL_ENV.min_python.0, MODAL_ENV.min_python.1
            );
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-modal-uv-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("modal")).unwrap();
        std::fs::write(
            dir.join("modal").join("__init__.py"),
            r#"import sys
class App:
    @staticmethod
    def lookup(name, create_if_missing): return name
class Image:
    @staticmethod
    def from_registry(tag): return Image()
    def dockerfile_commands(self, *cmds):
        sys.stderr.write("\n".join(cmds)); Image.with_uv = Image(); return Image.with_uv
class Sandbox:
    object_id = "sb-1"
    @classmethod
    def create(cls, *args, **kwargs):
        import subprocess
        assert kwargs["image"] is Image.with_uv, "sandbox did not get the uv image"
        # Modal's run log is the sandbox's stdout alone.
        ran = subprocess.run(list(args), capture_output=True, text=True)
        sys.stderr.write("\nstdout=%r stderr=%r" % (ran.stdout, ran.stderr))
        return cls()
"#,
        )
        .unwrap();
        let mut child = crate::process::command(python)
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
            .write_all(br#"{"image": "python:3.12", "script": "echo out; echo err >&2"}"#)
            .unwrap();
        let out = child.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{stderr}");
        assert!(
            stderr
                .lines()
                .any(|line| line.starts_with("COPY --from=ghcr.io/astral-sh/uv:")
                    && line.ends_with(" /uv /uvx /opt/orx-uv/")),
            "{stderr}"
        );
        assert!(
            stderr
                .lines()
                .any(|line| line == "ENV PATH=$PATH:/opt/orx-uv"),
            "{stderr}"
        );
        assert!(
            stderr.lines().any(|line| line == "ENTRYPOINT []"),
            "{stderr}"
        );
        assert!(
            stderr.contains(r"stdout='out\nerr\n' stderr=''"),
            "{stderr}"
        );
    }

    /// Points `ORX_MODAL_PYTHON` at a real interpreter that imports `module` as `modal`.
    #[cfg(not(windows))]
    fn fake_modal(python: &str, module: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orx-modal-sdk-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("modal")).unwrap();
        std::fs::write(dir.join("modal").join("__init__.py"), module).unwrap();
        std::fs::write(
            dir.join("modal").join("exception.py"),
            "class AuthError(Exception): pass\nclass InvalidError(Exception): pass\n",
        )
        .unwrap();
        let wrapper = dir.join("python");
        orx_test_support::write_executable(
            &wrapper,
            format!(
                "#!/bin/sh\nPYTHONPATH={} exec {python} \"$@\"\n",
                dir.display()
            ),
            0o755,
        )
        .unwrap();
        std::env::set_var("ORX_MODAL_PYTHON", &wrapper);
        dir
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_sandbox_that_exits_before_its_source_fails_the_launch_with_its_output() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            crate::config::skip_without_tool("no Python 3.10+ on PATH");
            return;
        };
        let dir = fake_modal(
            python,
            r#"import io, subprocess
class App:
    @staticmethod
    def lookup(name, create_if_missing): return name
class Image:
    @staticmethod
    def from_registry(tag): return Image()
    def dockerfile_commands(self, *cmds): return self
class Secret:
    @staticmethod
    def from_dict(env): return env
class Filesystem:
    def __init__(self, sandbox): self.sandbox = sandbox
    def copy_from_local(self, local, remote):
        raise RuntimeError("Sandbox is not running")
class Sandbox:
    object_id = "sb-1"
    @classmethod
    def create(cls, *args, **kwargs):
        sb = cls()
        ran = subprocess.run(list(args), capture_output=True, text=True)
        sb.code, sb.stdout = ran.returncode, io.StringIO(ran.stdout)
        sb.filesystem = Filesystem(sb)
        return sb
    def poll(self): return self.code
    def terminate(self): pass
"#,
        );
        let spec = ModalJobSpec {
            script: "echo 'bash: not found' >&2; exit 127".into(),
            image: "alpine".into(),
            gpu: None,
            cpu: None,
            memory: None,
            env: HashMap::new(),
            timeout_seconds: 60,
            app: "openresearch".into(),
            tags: HashMap::new(),
            source_archive: Some(dir.join("source.tar")),
        };
        let error = run_job(&spec, None).await.unwrap_err();
        std::fs::remove_dir_all(dir).unwrap();
        let refused = error
            .downcast_ref::<super::super::Refused>()
            .unwrap_or_else(|| panic!("not a definite failure: {error}"));
        assert_eq!(refused.status, None);
        assert!(refused.message.contains("code 127"), "{refused}");
        assert!(refused.message.contains("bash: not found"), "{refused}");
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_sandbox_modal_refuses_fails_the_launch() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            crate::config::skip_without_tool("no Python 3.10+ on PATH");
            return;
        };
        let dir = fake_modal(
            python,
            r#"import os
from modal.exception import AuthError, InvalidError
class App:
    @staticmethod
    def lookup(name, create_if_missing):
        if os.environ.get("ORX_TEST_MODAL") == "auth": raise AuthError("Token invalid")
        return name
class Image:
    @staticmethod
    def from_registry(tag): return Image()
    def dockerfile_commands(self, *cmds): return self
class Secret:
    @staticmethod
    def from_dict(env): return env
class Sandbox:
    @classmethod
    def create(cls, *args, gpu=None, **kwargs):
        raise InvalidError("Invalid GPU type: %s" % gpu)
"#,
        );
        let spec = ModalJobSpec {
            script: "true".into(),
            image: "alpine".into(),
            gpu: Some("H1000".into()),
            cpu: None,
            memory: None,
            env: HashMap::new(),
            timeout_seconds: 60,
            app: "openresearch".into(),
            tags: HashMap::new(),
            source_archive: None,
        };
        for (mode, cause) in [("gpu", "H1000"), ("auth", "Token invalid")] {
            std::env::set_var("ORX_TEST_MODAL", mode);
            let error = run_job(&spec, None).await.unwrap_err();
            let refused = error
                .downcast_ref::<super::super::Refused>()
                .unwrap_or_else(|| panic!("{mode} is not a definite failure: {error}"));
            assert_eq!(refused.status, None);
            assert!(refused.message.contains(cause), "{refused}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn preflight_checks_the_token_with_modal() {
        if !crate::config::in_test_child(&["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"]) {
            return;
        }
        let Ok(python) =
            crate::jobs::managed_env::base_python(MODAL_ENV.min_python, MODAL_ENV.label).await
        else {
            crate::config::skip_without_tool("no Python 3.10+ on PATH");
            return;
        };
        let dir = fake_modal(
            python,
            r#"import os
from . import exception
class Client:
    @staticmethod
    def from_env(): return Client()
    def hello(self):
        mode = os.environ["ORX_TEST_MODAL_AUTH"]
        if mode == "rejected": raise exception.AuthError("Token validation failed")
        if mode == "offline": raise ConnectionError("Could not connect to api.modal.com")
        if mode == "hang": import time; time.sleep(600)
"#,
        );
        std::env::set_var("MODAL_TOKEN_ID", "ak-1");
        std::env::set_var("MODAL_TOKEN_SECRET", "as-1");
        std::env::set_var("ORX_TEST_SPEEDUP", "4");
        let none = std::collections::BTreeMap::new();

        std::env::set_var("ORX_TEST_MODAL_AUTH", "ok");
        preflight(&none).await.unwrap();

        std::env::set_var("ORX_TEST_MODAL_AUTH", "rejected");
        let error = preflight(&none).await.unwrap_err().to_string();
        assert!(error.contains("didn't accept the token"), "{error}");
        assert!(error.contains("orx up's environment"), "{error}");
        assert!(error.contains("Token validation failed"), "{error}");

        std::env::set_var("ORX_TEST_MODAL_AUTH", "offline");
        let error = preflight(&none).await.unwrap_err().to_string();
        assert!(error.contains("Could not reach Modal"), "{error}");

        std::env::set_var("ORX_TEST_MODAL_AUTH", "hang");
        let started = std::time::Instant::now();
        let error = preflight(&none).await.unwrap_err().to_string();
        assert!(error.contains("Could not reach Modal"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(30), "{error}");

        std::env::set_var("ORX_TEST_MODAL_AUTH", "ok");
        let caller = std::collections::BTreeMap::from([(
            "ORX_MODAL_PYTHON".to_string(),
            "/opt/py312/bin/python".to_string(),
        )]);
        let error = preflight(&caller).await.unwrap_err().to_string();
        assert!(
            error.contains("This shell sets ORX_MODAL_PYTHON"),
            "{error}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
