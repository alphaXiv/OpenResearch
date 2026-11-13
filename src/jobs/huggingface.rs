//! Hugging Face Jobs client — the REST surface behind `hf jobs`.
//!
//! Paths and body shapes mirror huggingface_hub's `hf_api.py`/`_jobs_api.py`:
//!   POST {endpoint}/api/jobs/{namespace}            run a job
//!   GET  {endpoint}/api/jobs/{namespace}/{id}       inspect
//!   GET  {endpoint}/api/jobs/{namespace}/{id}/logs  SSE log stream, `?tail=N` for the last N lines
//!   POST {endpoint}/api/jobs/{namespace}/{id}/cancel
//!   GET  {endpoint}/api/whoami-v2                   token → namespace
//! Wire fields are camelCase; `timeoutSeconds` is integer seconds; auth is a
//! plain `Bearer` header on every call including the log stream.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::managed_env::ManagedEnv;
use crate::error::{anyhow, Result};

/// Prints the job JSON, or exits 1 after printing `{"failed": "staging" | "job", "status", "message"}`.
const SOURCE_LAUNCHER: &str = r#"
import contextlib, json, sys
from huggingface_hub import HfApi

def attempt(step, call):
    try:
        return call()
    except Exception as error:
        status = getattr(getattr(error, "response", None), "status_code", None)
        failure = {"failed": step, "status": status, "message": str(error)}
        sys.__stdout__.write(json.dumps(failure))
        sys.exit(1)

spec = json.load(sys.stdin)
api = HfApi(endpoint=spec["endpoint"], token=spec["token"])
# sync_job_volume prints its sync plan to stdout, which must carry only the job JSON.
with contextlib.redirect_stdout(sys.stderr):
    volume = attempt("staging", lambda: api.sync_job_volume(
        spec["sourceDir"],
        "/orx-source",
        namespace=spec["namespace"],
        remote_name="orx-" + spec["digest"],
        read_only=True,
    ))
    job = attempt("job", lambda: api.run_job(
        image=spec["image"],
        command=spec["command"],
        env=spec["environment"],
        secrets=spec["secrets"],
        flavor=spec["flavor"],
        timeout=spec["timeoutSeconds"],
        labels=spec["labels"],
        volumes=[volume],
        namespace=spec["namespace"],
    ))
print(json.dumps({"id": job.id}))
"#;

/// `huggingface_hub` gained `sync_job_volume` in 1.22, which requires Python 3.10.
const CLIENT_ENV: ManagedEnv = ManagedEnv {
    name: "huggingface",
    label: "Hugging Face Jobs",
    min_python: (3, 10),
    requirement: "huggingface_hub>=1.22.0",
    ready_check: "from huggingface_hub import HfApi; assert hasattr(HfApi, 'sync_job_volume')",
    override_var: None,
};

pub fn endpoint() -> String {
    std::env::var("HF_ENDPOINT").unwrap_or_else(|_| "https://huggingface.co".to_string())
}

/// Which link of the resolution chain produced the token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    OpenresearchEnv,
    HfCache,
}

/// Resolve the HF token: `HF_TOKEN` env first, then the box's synced env file
/// (`~/.openresearch/env` — where the org credential/env-var lands, invisible
/// to non-interactive shells), then the hf CLI's token file.
pub fn resolve_token() -> Result<String> {
    resolve_token_with_source().map(|(tok, _)| tok)
}

/// `resolve_token`, but also reports which source won (settings UI).
pub fn resolve_token_with_source() -> Result<(String, TokenSource)> {
    if let Ok(tok) = std::env::var("HF_TOKEN") {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Ok((tok, TokenSource::Env));
        }
    }
    if let Some(tok) = crate::config::synced_env_var("HF_TOKEN") {
        return Ok((tok, TokenSource::OpenresearchEnv));
    }
    let path = crate::config::home_dir()
        .unwrap_or_default()
        .join(".cache")
        .join("huggingface")
        .join("token");
    if let Ok(tok) = std::fs::read_to_string(&path) {
        let tok = tok.trim().to_string();
        if !tok.is_empty() {
            return Ok((tok, TokenSource::HfCache));
        }
    }
    Err(anyhow!(
        "No Hugging Face token found. Set HF_TOKEN (or run `hf auth login`). \
         Mint one at https://huggingface.co/settings/tokens — or connect it in \
         the org's compute settings so it syncs to agent boxes automatically."
    ))
}

fn http() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::net::remote_client()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .expect("reqwest client")
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub stage: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: String,
    pub status: JobStatus,
}

async fn check(res: reqwest::Response, what: &str) -> Result<reqwest::Response> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    if matches!(status.as_u16(), 401 | 403) {
        res.error_for_status_ref().map_err(|error| {
            anyhow!(error).context(format!(
                "Hugging Face rejected the token (HTTP {status}) during {what}. Check HF_TOKEN."
            ))
        })?;
    }
    let body = res.text().await.unwrap_or_default();
    Err(anyhow!(
        "Hugging Face {} failed ({}): {}",
        what,
        status.as_u16(),
        body
    ))
}

/// The token's account name — the default jobs namespace. whoami-v2 is heavily
/// rate-limited upstream, so call once per command, not per poll.
pub async fn whoami(token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct WhoAmI {
        name: String,
    }
    let res = http()
        .get(format!("{}/api/whoami-v2", endpoint()))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Hugging Face: {}", e))?;
    let who: WhoAmI = check(res, "whoami").await?.json().await?;
    Ok(who.name)
}

/// whoami-v2 details for the settings UI.
#[derive(Debug, Clone)]
pub struct WhoamiDetails {
    pub name: String,
    /// Can this token submit jobs? `None` = shape didn't say.
    pub jobs_write: Option<bool>,
}

pub async fn whoami_details(token: &str) -> Result<WhoamiDetails> {
    let res = http()
        .get(format!("{}/api/whoami-v2", endpoint()))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Hugging Face: {}", e))?;
    let body: serde_json::Value = check(res, "whoami").await?.json().await?;
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("whoami response had no name"))?
        .to_string();
    let jobs_write = match body["auth"]["accessToken"]["role"].as_str() {
        Some("write") => Some(true),
        Some("read") => Some(false),
        // fineGrained: job.write must appear in the global or a scoped grant.
        Some("fineGrained") => {
            let fg = &body["auth"]["accessToken"]["fineGrained"];
            let global = fg.get("global").and_then(|v| v.as_array());
            let scoped = fg.get("scoped").and_then(|v| v.as_array());
            if global.is_none() && scoped.is_none() {
                None
            } else {
                let has = |perms: &[serde_json::Value]| {
                    perms.iter().any(|p| p.as_str() == Some("job.write"))
                };
                let hit = global.map(|g| has(g)).unwrap_or(false)
                    || scoped.into_iter().flatten().any(|s| {
                        s.get("permissions")
                            .and_then(|p| p.as_array())
                            .map(|p| has(p))
                            .unwrap_or(false)
                    });
                Some(hit)
            }
        }
        _ => None,
    };
    Ok(WhoamiDetails { name, jobs_write })
}

/// The private bucket `sync_job_volume` stages each run's source into.
const STAGING_BUCKET: &str = "jobs-artifacts";

pub fn staging_forbidden(namespace: &str) -> String {
    format!(
        "Your Hugging Face token can't create or write the private bucket \
         {namespace}/{STAGING_BUCKET}, where orx stages each run's source. Give the token write \
         access to repositories and buckets in {namespace} at \
         https://huggingface.co/settings/tokens, or use a token with the write role, then run \
         `orx compute test hf`."
    )
}

/// Whether the token can do what source staging does first: create the staging bucket, or
/// read it when it already exists. Creating it early is harmless; staging creates the same one.
pub async fn can_stage_source(token: &str, namespace: &str) -> Result<bool> {
    let url = format!("{}/api/buckets/{namespace}/{STAGING_BUCKET}", endpoint());
    let unreachable = |e: reqwest::Error| anyhow!("Could not reach Hugging Face: {}", e);
    let create = http()
        .post(&url)
        .bearer_auth(token)
        .json(&json!({"visibility": "private"}))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(unreachable)?;
    let status = create.status();
    if status.is_success() || status == reqwest::StatusCode::CONFLICT {
        return Ok(true);
    }
    if !matches!(status.as_u16(), 401 | 403) {
        return check(create, "the source bucket check").await.map(|_| true);
    }
    let info = http()
        .get(&url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(unreachable)?;
    match info.status().as_u16() {
        200..=299 => Ok(true),
        401 | 403 | 404 => Ok(false),
        _ => check(info, "the source bucket check").await.map(|_| true),
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct JobSubmission {
    pub command: Vec<String>,
    pub docker_image: String,
    pub flavor: String,
    #[serde(default)]
    pub environment: HashMap<String, String>,
    #[serde(default)]
    pub secrets: HashMap<String, String>,
    pub timeout_seconds: u64,
    pub labels: HashMap<String, String>,
}

/// Sync the immutable archive into the private `jobs-artifacts` bucket and
/// mount it into the Job. The digest-derived remote name makes retries upload
/// nothing when the exact source was already staged.
pub async fn run_job_with_source(
    token: &str,
    namespace: &str,
    spec: &JobSubmission,
    archive: &Path,
    digest: &str,
    fence: Option<&crate::engine::Fence<'_>>,
) -> Result<JobInfo> {
    let python = CLIENT_ENV.ensure().await?;
    let source_dir = archive
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{digest}.hf"));
    std::fs::create_dir_all(&source_dir)?;
    let staged = source_dir.join("source.tar");
    if !staged.exists() && std::fs::hard_link(archive, &staged).is_err() {
        std::fs::copy(archive, &staged)?;
    }
    let body = json!({
        "endpoint": endpoint(),
        "token": token,
        "namespace": namespace,
        "sourceDir": source_dir,
        "digest": digest,
        "image": spec.docker_image,
        "command": spec.command,
        "environment": super::default_python_env(&spec.environment),
        "secrets": spec.secrets,
        "flavor": spec.flavor,
        "timeoutSeconds": spec.timeout_seconds,
        "labels": spec.labels,
    });
    let mut child = crate::process::tokio_command(python)
        .args(["-c", SOURCE_LAUNCHER])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(fence) = fence {
        let recorded = fence.record_submitter(
            child
                .id()
                .ok_or_else(|| anyhow!("HF launcher exited before fencing"))?,
        );
        if let Err(error) = recorded {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(error);
        }
    }
    use tokio::io::AsyncWriteExt as _;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body.to_string().as_bytes())
        .await?;
    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(match serde_json::from_slice(&output.stdout) {
            Ok(failure) => launch_failure(namespace, failure),
            Err(_) => anyhow!(
                "The Hugging Face launcher failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let id = value["id"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| anyhow!("Hugging Face source launch returned no job id."))?;
    Ok(JobInfo {
        id: id.to_string(),
        status: JobStatus {
            stage: "SCHEDULING".to_string(),
            message: None,
        },
    })
}

/// A step of `SOURCE_LAUNCHER` that failed.
#[derive(Deserialize)]
struct LaunchFailure {
    failed: String,
    status: Option<u16>,
    message: String,
}

fn launch_failure(namespace: &str, failure: LaunchFailure) -> crate::error::Error {
    let LaunchFailure {
        failed,
        status,
        message,
    } = failure;
    let text = match (failed.as_str(), status) {
        ("staging", Some(401 | 403)) => format!("{} ({message})", staging_forbidden(namespace)),
        ("staging", _) => format!(
            "Hugging Face could not stage the run's source in {namespace}/{STAGING_BUCKET}: {message}"
        ),
        ("job", Some(402)) => format!(
            "Hugging Face won't run jobs in {namespace} without billing ({message}). Add a \
             payment method or prepaid credit to {namespace} at \
             https://huggingface.co/settings/billing, or in the organization's billing settings, \
             then run the experiment again."
        ),
        ("job", Some(401 | 403)) => format!(
            "Your Hugging Face token can't run jobs in {namespace} ({message}). Give it the \
             job.write permission for {namespace} at https://huggingface.co/settings/tokens, then \
             run `orx compute test hf`."
        ),
        _ => format!("Hugging Face did not create the job in {namespace}: {message}"),
    };
    match status {
        Some(status) => super::Refused {
            status: Some(status),
            message: text,
        }
        .into(),
        None => anyhow!(text),
    }
}

pub async fn inspect_job(token: &str, namespace: &str, job_id: &str) -> Result<JobInfo> {
    let res = http()
        .get(format!("{}/api/jobs/{}/{}", endpoint(), namespace, job_id))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Hugging Face: {}", e))?;
    let job: JobInfo = check(res, "job inspect").await?.json().await?;
    Ok(job)
}

pub async fn cancel_job(token: &str, namespace: &str, job_id: &str) -> Result<()> {
    let res = http()
        .post(format!(
            "{}/api/jobs/{}/{}/cancel",
            endpoint(),
            namespace,
            job_id
        ))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Hugging Face: {}", e))?;
    check(res, "job cancel").await?;
    Ok(())
}

/// HF serves a line longer than this as several events of exactly this many bytes, with no
/// marker on the pieces; a replay may split a line that the live stream sent whole.
const PARTIAL_EVENT: usize = 16 * 1024;

/// Lines a reconnect asks for first. A tail that does not reach back past what was kept may
/// have skipped output, so the reconnect falls back to the whole replay.
const RESUME_TAIL: usize = 100;

/// Where the run log left off: the timestamp of the newest line kept, and how many bytes of
/// lines carrying it were written, so a replay chunked differently resumes at the same byte.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogResume {
    key: String,
    tied: u64,
}

/// One connection's progress through the events HF replays.
struct Pass {
    /// Bytes of lines at the resume timestamp this replay has yet to repeat.
    tied: u64,
    caught_up: bool,
    /// The previous event was a piece of a longer line, so this one continues it.
    continues: bool,
    line: Line,
}

/// What a replayed line is to the resume point; a continuation inherits its line's.
#[derive(Clone, Copy, PartialEq)]
enum Line {
    Kept,
    Tied,
    Skipped,
    /// Kept without moving the resume point: unstamped, or stamped earlier after catching up.
    Untracked,
}

impl LogResume {
    /// The part of `event` not yet written, judged by the timestamp of the line it belongs to.
    /// Only the replay is filtered, as for [`crate::jobs::kubernetes::LogResume`].
    fn admit<'a>(&mut self, pass: &mut Pass, key: Option<String>, event: &'a [u8]) -> &'a [u8] {
        if !pass.continues {
            pass.line = match key {
                None => Line::Untracked,
                Some(key) => match key.cmp(&self.key) {
                    Ordering::Less if pass.caught_up => Line::Untracked,
                    Ordering::Less => Line::Skipped,
                    Ordering::Equal if !pass.caught_up => Line::Tied,
                    Ordering::Equal => Line::Kept,
                    Ordering::Greater => {
                        self.key = key;
                        self.tied = 0;
                        Line::Kept
                    }
                },
            };
        }
        let fresh = match pass.line {
            Line::Skipped => &event[..0],
            Line::Tied => {
                let seen = pass.tied.min(event.len() as u64);
                pass.tied -= seen;
                &event[seen as usize..]
            }
            Line::Kept | Line::Untracked => event,
        };
        if !fresh.is_empty() && pass.line != Line::Untracked {
            pass.caught_up = true;
            self.tied += fresh.len() as u64;
        }
        fresh
    }
}

/// How one connection to the log stream ended.
enum Ended {
    Closed,
    Idle,
    /// The tail began after what was kept.
    Gap,
}

/// Follows the job's SSE log stream from `resume`, handing `sink` new lines' bytes with the
/// resume point past them. A line gets back the newline the stream strips, unless it continues
/// in the next event.
///
/// The server replays its retained history on every connection, so a reconnect asks for a
/// short tail and drops what `resume` says was written. True when the server closed the stream;
/// false after `idle` without log data, which keep-alive comments do not count as.
pub async fn stream_logs(
    token: &str,
    namespace: &str,
    job_id: &str,
    resume: &mut LogResume,
    idle: Duration,
    sink: &mut (dyn FnMut(&[u8], &LogResume) + Send),
) -> Result<bool> {
    let mut tail = (!resume.key.is_empty()).then_some(RESUME_TAIL);
    loop {
        match stream_once(token, namespace, job_id, resume, tail, idle, sink).await? {
            Ended::Closed => return Ok(true),
            Ended::Idle => return Ok(false),
            Ended::Gap => tail = None,
        }
    }
}

async fn stream_once(
    token: &str,
    namespace: &str,
    job_id: &str,
    resume: &mut LogResume,
    tail: Option<usize>,
    idle: Duration,
    sink: &mut (dyn FnMut(&[u8], &LogResume) + Send),
) -> Result<Ended> {
    #[derive(Deserialize)]
    struct LogEvent {
        data: String,
        timestamp: Option<String>,
    }
    let mut request = http()
        .get(format!(
            "{}/api/jobs/{}/{}/logs",
            endpoint(),
            namespace,
            job_id
        ))
        .bearer_auth(token);
    if let Some(tail) = tail {
        request = request.query(&[("tail", tail)]);
    }
    let res = request
        .send()
        .await
        .map_err(|e| anyhow!("Could not reach Hugging Face: {}", e))?;
    let mut res = check(res, "log stream").await?;

    let mut pass = Pass {
        tied: resume.tied,
        caught_up: false,
        continues: false,
        line: Line::Kept,
    };
    let mut first = true;
    let mut deadline = tokio::time::Instant::now() + idle;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        let chunk = match tokio::time::timeout_at(deadline, res.chunk()).await {
            Err(_) => return Ok(Ended::Idle),
            Ok(Err(e)) => return Err(anyhow!("Hugging Face log stream failed: {}", e)),
            Ok(Ok(None)) => return Ok(Ended::Closed),
            Ok(Ok(Some(c))) => c,
        };
        buf.extend_from_slice(&chunk);
        // One hand-off per chunk, since each moves the resume point the caller saves.
        let mut batch = Vec::new();
        // SSE frames are newline-delimited; process complete lines only.
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(json) = line.trim_end().strip_prefix("data: ") else {
                continue; // keep-alive comments, event: lines, blanks
            };
            let Ok(event) = serde_json::from_str::<LogEvent>(json) else {
                continue;
            };
            deadline = tokio::time::Instant::now() + idle;
            if event.data.starts_with("===== Job started") {
                continue;
            }
            let key = event
                .timestamp
                .as_deref()
                .and_then(super::kubernetes::sort_key);
            if std::mem::take(&mut first) && !resume.key.is_empty() {
                let reaches_back = key.as_ref().is_some_and(|key| *key < resume.key);
                if tail.is_some() && !reaches_back {
                    return Ok(Ended::Gap);
                }
                if key.as_ref().is_some_and(|key| *key > resume.key) {
                    eprintln!(
                        "orx up: Hugging Face job {job_id}: the log replay starts after the \
                         output already kept, so the lines between are missing"
                    );
                }
            }
            let mut bytes = event.data.into_bytes();
            let continued = bytes.len() == PARTIAL_EVENT;
            if !continued {
                bytes.push(b'\n');
            }
            batch.extend_from_slice(resume.admit(&mut pass, key, &bytes));
            pass.continues = continued;
        }
        if !batch.is_empty() {
            sink(&batch, resume);
        }
    }
}

/// Parse a human duration ("90s", "30m", "4h", "1d", or bare seconds).
pub fn parse_timeout(value: &str) -> Result<u64> {
    let v = value.trim();
    let (num, factor) = match v.chars().last() {
        Some('s') => (&v[..v.len() - 1], 1u64),
        Some('m') => (&v[..v.len() - 1], 60),
        Some('h') => (&v[..v.len() - 1], 3600),
        Some('d') => (&v[..v.len() - 1], 86_400),
        _ => (v, 1),
    };
    let n: u64 = num
        .parse()
        .map_err(|_| anyhow!("Bad --timeout '{}': use e.g. 30m, 4h, 1d.", value))?;
    n.checked_mul(factor)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            anyhow!(
                "Bad --timeout '{}': use a positive duration that fits in seconds.",
                value
            )
        })
}

pub async fn find_run(token: &str, namespace: &str, run: &str) -> Result<Vec<JobInfo>> {
    let base = reqwest::Url::parse(&endpoint())?;
    let mut url = base.join(&format!("/api/jobs/{namespace}"))?;
    url.query_pairs_mut()
        .append_pair("labels", &format!("or_run={run}"));
    let mut visited = std::collections::HashSet::new();
    let mut found = Vec::new();
    loop {
        if url.origin() != base.origin() || !visited.insert(url.clone()) {
            return Err(anyhow!("Invalid HF pagination link"));
        }
        let response = check(
            http().get(url.clone()).bearer_auth(token).send().await?,
            "job lookup",
        )
        .await?;
        let next = response
            .headers()
            .get("link")
            .and_then(|header| header.to_str().ok())
            .and_then(|links| links.split(',').find(|link| link.contains("rel=\"next\"")))
            .map(|link| -> Result<reqwest::Url> {
                let target = link
                    .trim()
                    .strip_prefix('<')
                    .and_then(|link| link.split_once('>').map(|(target, _)| target))
                    .ok_or_else(|| anyhow!("Invalid HF pagination link"))?;
                Ok(url.join(target)?)
            })
            .transpose()?;
        let jobs: Vec<serde_json::Value> = response.json().await?;
        for job in jobs {
            if job["labels"]["or_run"].as_str() == Some(run) {
                found.push(serde_json::from_value(job)?);
            }
        }
        match next {
            Some(next) => url = next,
            None => return Ok(found),
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[tokio::test]
    async fn invalid_token_retains_http_status() {
        for status in [401, 403, 503] {
            let response = axum::http::Response::builder()
                .status(status)
                .body("")
                .unwrap();
            let error = check(response.into(), "whoami").await.unwrap_err();
            assert_eq!(
                error
                    .downcast_ref::<reqwest::Error>()
                    .and_then(|e| e.status()),
                matches!(status, 401 | 403).then(|| reqwest::StatusCode::from_u16(status).unwrap())
            );
        }
    }

    #[tokio::test]
    async fn source_launcher_keeps_library_output_off_stdout() {
        let Ok(python) =
            crate::jobs::managed_env::base_python(CLIENT_ENV.min_python, CLIENT_ENV.label).await
        else {
            eprintln!(
                "skipped: no Python {}.{}+ on PATH",
                CLIENT_ENV.min_python.0, CLIENT_ENV.min_python.1
            );
            return;
        };
        let dir = std::env::temp_dir().join(format!("orx-hf-launcher-{}", uuid::Uuid::new_v4()));
        let package = dir.join("huggingface_hub");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("__init__.py"),
            r#"
class HfApi:
    def __init__(self, **kwargs):
        pass
    def sync_job_volume(self, *args, **kwargs):
        print("Sync plan: local -> hf://buckets/x")
        return "volume"
    def run_job(self, **kwargs):
        print("Job started")
        return type("Job", (), {"id": "job-123"})()
"#,
        )
        .unwrap();
        let spec = json!({
            "endpoint": "", "token": "", "namespace": "", "sourceDir": "", "digest": "",
            "image": "", "command": [], "environment": {}, "secrets": {}, "flavor": "",
            "timeoutSeconds": 0, "labels": {},
        });
        let mut child = crate::process::command(python)
            .args(["-c", SOURCE_LAUNCHER])
            .env("PYTHONPATH", &dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write as _;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(spec.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["id"], "job-123");
        assert!(String::from_utf8_lossy(&output.stderr).contains("Sync plan"));
    }
}
