//! boxd backend: each run gets its own isolated [boxd](https://boxd.sh?ref=OpenResearch) machine,
//! a full Ubuntu VM that is deleted once the run ends and its log is kept. orx calls boxd's gRPC
//! API directly, so nothing has to be installed before a first run.
//!
//! The job runs detached under `setsid` in [`RUN_DIR`] and writes its pid, log and exit code
//! there, so it outlives every call that started or watched it. Status is one `Exec` that reads
//! those files, and logs are read by byte offset, so a restarted `orx up` continues where the last
//! one stopped.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use serde::{Deserialize, Serialize};
use tonic::codegen::http::uri::PathAndQuery;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Code, Status};

use crate::error::{anyhow, Context as _, Result};

pub(crate) mod api;
#[cfg(test)]
pub(crate) mod tests;

pub use super::vercel::credential;

pub const API_KEY: &str = "BOXD_API_KEY";
/// `fra` or `nyc`; unset, runs go to the org's default region.
pub const REGION: &str = "ORX_BOXD_REGION";
pub const NAME_PREFIX: &str = "ORX_BOXD_NAME_PREFIX";
/// Replaces boxd's endpoints, for a self-hosted boxd.
pub const API_URL: &str = "ORX_BOXD_API_URL";

/// Account calls go to the first, the home region; machine calls to the machine's region.
const REGIONS: [(&str, &str); 2] = [
    ("fra", "https://boxd.sh:9443"),
    ("nyc", "https://nyc.boxd.sh:9443"),
];

/// The run's own directory on its machine, owner-only.
pub const RUN_DIR: &str = "/home/boxd/orx";
pub const SOURCE_ARCHIVE: &str = "/home/boxd/orx/source.tar";

pub const DEFAULT_TIMEOUT_SECS: u64 = 4 * 3600;
const VCPUS: [u32; 4] = [1, 2, 4, 8];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Bound on each unary call; `Exec` and uploads take as long as they need.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const READY_TIMEOUT: Duration = Duration::from_secs(180);
const READY_POLL: Duration = Duration::from_millis(500);
/// Staging a launch may take this long plus `STAGE_PER_MIB` per MiB it uploads; past that the
/// launch fails and its machine is deleted.
const STAGE_TIMEOUT: Duration = Duration::from_secs(300);
const STAGE_PER_MIB: Duration = Duration::from_secs(1);
/// Cancel sends SIGTERM, then SIGKILL after 10 s; the whole stop command gets this long.
const CANCEL_TIMEOUT: Duration = Duration::from_secs(60);
/// List prices in dollars (<https://boxd.sh/pricing?ref=OpenResearch>): the most a running machine
/// costs, with every vCPU busy and all of its memory and its 100 GiB disk in use.
const USD_MICROS_PER_VCPU_HOUR: u64 = 47_000;
const USD_MICROS_PER_GIB_HOUR: u64 = 14_000;
const USD_MICROS_DISK_HOUR: u64 = 9_600;

/// boxd rejected the API key. The engine shows it as needing auth, and this process doesn't send
/// the key again.
#[derive(Debug)]
pub struct AuthRejected(pub String);

impl std::fmt::Display for AuthRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthRejected {}

fn setting(key: &str) -> Option<String> {
    credential(key).map(|(value, _)| value.trim().to_string())
}

pub fn not_set_up() -> crate::error::Error {
    anyhow!(
        "boxd isn't set up: BOXD_API_KEY is not set. Create an API key in the boxd console \
         (https://boxd.sh?ref=OpenResearch) or with `boxd auth keys create orx`, then add it in \
         Settings → Compute → boxd or set BOXD_API_KEY in the environment."
    )
}

fn region_url(region: &str) -> Result<&'static str> {
    REGIONS
        .iter()
        .find(|(id, _)| id.eq_ignore_ascii_case(region.trim()))
        .map(|(_, url)| *url)
        .ok_or_else(|| {
            anyhow!("Unknown boxd region '{region}'. Set {REGION} to fra (Frankfurt) or nyc (New York).")
        })
}

/// The region configured for new machines, if any.
pub fn configured_region() -> Result<Option<String>> {
    setting(REGION)
        .map(|region| region_url(&region).map(|_| region.to_ascii_lowercase()))
        .transpose()
}

/// The API keys boxd rejected, with its reason.
fn rejected() -> MutexGuard<'static, HashMap<String, String>> {
    static REJECTED: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    REJECTED
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// An API key and the endpoints it is used against.
#[derive(Clone)]
pub struct Account {
    api_key: String,
    api_url: Option<String>,
    stage_timeout: Duration,
    cancel_timeout: Duration,
}

impl Account {
    pub fn from_env() -> Result<Self> {
        Ok(Self::new(setting(API_KEY).ok_or_else(not_set_up)?))
    }

    /// `api_key` against the configured endpoints.
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            api_url: setting(API_URL),
            stage_timeout: STAGE_TIMEOUT,
            cancel_timeout: CANCEL_TIMEOUT,
        }
    }

    /// A client for the machines of `region`; empty is the home region.
    pub fn client(&self, region: &str) -> Result<Client> {
        let url = match (&self.api_url, region.trim()) {
            (Some(url), _) => url.as_str(),
            (None, "") => REGIONS[0].1,
            (None, region) => region_url(region)?,
        };
        Ok(Client {
            account: self.clone(),
            channel: channel(url)?,
        })
    }
}

/// One lazily connected channel per endpoint, shared by every run.
fn channel(url: &str) -> Result<Channel> {
    static CHANNELS: OnceLock<Mutex<HashMap<String, Channel>>> = OnceLock::new();
    let mut cache = CHANNELS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(channel) = cache.get(url) {
        return Ok(channel.clone());
    }
    let mut endpoint = Endpoint::from_shared(url.to_string())
        .map_err(|error| anyhow!("Invalid boxd API URL {url}: {error}"))?
        .connect_timeout(CONNECT_TIMEOUT)
        .http2_keep_alive_interval(Duration::from_secs(30))
        .keep_alive_timeout(Duration::from_secs(20))
        .keep_alive_while_idle(true);
    if url.starts_with("https://") {
        let tls = ClientTlsConfig::new()
            .with_webpki_roots()
            .with_native_roots();
        endpoint = endpoint
            .tls_config(tls)
            .map_err(|error| anyhow!("TLS for the boxd API: {error}"))?;
    } else if !url.starts_with("http://") {
        return Err(anyhow!("The boxd API URL must be http(s), got {url}"));
    }
    let channel = endpoint.connect_lazy();
    cache.insert(url.to_string(), channel.clone());
    Ok(channel)
}

fn id(vm_id: &str) -> api::VmId {
    api::VmId {
        vm_id: vm_id.into(),
    }
}

fn path(method: &str) -> PathAndQuery {
    PathAndQuery::try_from(format!("/boxd.api.v1.BoxdApi/{method}")).expect("a valid gRPC path")
}

/// The calls on one region's endpoint.
#[derive(Clone)]
pub struct Client {
    account: Account,
    channel: Channel,
}

impl Client {
    /// A request carrying the API key, and a connection ready to send it.
    async fn grpc<T>(
        &self,
        message: T,
    ) -> Result<(tonic::client::Grpc<Channel>, tonic::Request<T>)> {
        if let Some(reason) = rejected().get(&self.account.api_key) {
            return Err(AuthRejected(reason.clone()).into());
        }
        let mut request = tonic::Request::new(message);
        let mut bearer: tonic::metadata::MetadataValue<_> =
            format!("Bearer {}", self.account.api_key)
                .parse()
                .map_err(|_| anyhow!("BOXD_API_KEY has characters an API key can't have"))?;
        bearer.set_sensitive(true);
        request.metadata_mut().insert("authorization", bearer);
        let mut grpc = tonic::client::Grpc::new(self.channel.clone());
        grpc.ready()
            .await
            .map_err(|error| anyhow!("Couldn't reach boxd: {error}"))?;
        Ok((grpc, request))
    }

    /// A unary call: the outer error is orx's (key, connection, timeout), the inner boxd's.
    async fn call<Req, Resp>(&self, method: &str, message: Req) -> Result<Result<Resp, Status>>
    where
        Req: prost::Message + 'static,
        Resp: prost::Message + Default + 'static,
    {
        let call = async {
            let (mut grpc, request) = self.grpc(message).await?;
            let codec = tonic_prost::ProstCodec::default();
            Ok(grpc
                .unary(request, path(method), codec)
                .await
                .map(tonic::Response::into_inner))
        };
        tokio::time::timeout(CALL_TIMEOUT, call)
            .await
            .unwrap_or_else(|_| Err(anyhow!("boxd did not answer {method} within 60 s")))
    }

    /// boxd's error in words for the user. `what` names the call, such as "delete machine x".
    fn error(&self, what: &str, status: Status) -> crate::error::Error {
        let message = status
            .message()
            .replace(&self.account.api_key, "[redacted]");
        match status.code() {
            Code::Unauthenticated => {
                let reason = format!(
                    "boxd rejected BOXD_API_KEY ({message}). Check the key in Settings → Compute → \
                     boxd or the environment, or create a new one with `boxd auth keys create orx`."
                );
                rejected().insert(self.account.api_key.clone(), reason.clone());
                AuthRejected(reason).into()
            }
            Code::PermissionDenied => {
                anyhow!("boxd refused to {what}: {message}. An API key acts in its own org only.")
            }
            Code::Unavailable => anyhow!("boxd is unreachable ({what}): {message}"),
            code => anyhow!("boxd could not {what} ({code:?}): {message}"),
        }
    }

    pub async fn org_defaults(&self) -> Result<api::OrgDefaults> {
        self.call("GetOrgMachineDefaults", api::Empty {})
            .await?
            .map_err(|status| self.error("read the org's machine defaults", status))
    }

    /// [`Self::org_defaults`] for a user checking a key: boxd is asked even if it rejected it before.
    pub async fn check(&self) -> Result<api::OrgDefaults> {
        rejected().remove(&self.account.api_key);
        self.org_defaults().await
    }

    /// `None` when boxd has no machine with this id for this key.
    pub async fn get(&self, vm_id: &str) -> Result<Option<api::Vm>> {
        match self.call("GetVm", id(vm_id)).await? {
            Ok(machine) => Ok(Some(machine)),
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(self.error(&format!("read machine {vm_id}"), status)),
        }
    }

    /// Idempotent: a machine that is already gone counts as deleted.
    pub async fn delete(&self, vm_id: &str) -> Result<()> {
        match self.call::<_, api::Empty>("DestroyVm", id(vm_id)).await? {
            Err(status) if status.code() != Code::NotFound => {
                Err(self.error(&format!("delete machine {vm_id}"), status))
            }
            _ => Ok(()),
        }
    }

    /// An isolated machine with auto-suspend off; [`launch`] turns auto-hibernate off.
    async fn create(&self, spec: &BoxdJobSpec) -> Result<api::Vm> {
        let request = api::CreateVmRequest {
            name: spec.name.clone(),
            config: Some(api::VmConfig {
                vcpu: spec.vcpus.unwrap_or(0),
                memory_bytes: spec.vcpus.map_or(0, |vcpu| u64::from(vcpu) * (4 << 30)),
                srf: Some(api::SrfConfig {
                    auto_suspend_timeout_secs: Some(0),
                }),
            }),
            isolated: true,
            labels: spec.labels.clone(),
        };
        let created: api::CreateVmResponse =
            self.call("CreateVm", request).await?.map_err(|status| {
                use Code::*;
                let code = status.code();
                let error = self.error(&format!("create machine {}", spec.name), status);
                // Refused for certain, so nothing was made; any other answer is checked by recovery.
                match code {
                    InvalidArgument | FailedPrecondition | OutOfRange | Unauthenticated
                    | PermissionDenied | NotFound => super::Refused {
                        status: None,
                        message: error.to_string(),
                    }
                    .into(),
                    _ => error,
                }
            })?;
        let vm_id = created.vm_id;
        Ok(created.machine.unwrap_or(api::Vm {
            vm_id,
            ..Default::default()
        }))
    }

    /// Brings the machine to running and waits until it runs a command.
    async fn wait_until_ready(&self, vm_id: &str) -> Result<api::Vm> {
        let deadline = Instant::now() + READY_TIMEOUT;
        let mut asked = "";
        let machine = loop {
            let machine = self.get(vm_id).await?.ok_or_else(|| {
                anyhow!("boxd has no machine {vm_id} for this API key: it was deleted outside orx.")
            })?;
            let action = match machine.status.as_str() {
                "running" => break machine,
                "hibernated" => "WakeVm",
                "suspended" | "paused" => "ResumeVm",
                "stopped" => "StartVm",
                status @ ("failed" | "destroyed") => {
                    return Err(anyhow!(
                        "boxd machine {vm_id} is {status} and can't run the job."
                    ))
                }
                _ => "",
            };
            if !action.is_empty() && action != asked {
                // FailedPrecondition: it moved on by itself, and the next read decides.
                match self.call::<_, api::Empty>(action, id(vm_id)).await? {
                    Err(status) if status.code() != Code::FailedPrecondition => {
                        return Err(self.error(&format!("start machine {vm_id}"), status))
                    }
                    _ => asked = action,
                }
            }
            if Instant::now() >= deadline {
                let status = machine.status;
                return Err(anyhow!(
                    "boxd machine {vm_id} was still {status} after {READY_TIMEOUT:?}."
                ));
            }
            tokio::time::sleep(crate::config::paced(READY_POLL)).await;
        };
        // A machine that reads as running may not take commands for a moment.
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let probe =
                tokio::time::timeout(left.min(Duration::from_secs(15)), self.run(vm_id, "true"))
                    .await
                    .unwrap_or_else(|_| Err(anyhow!("the first command timed out")));
            match probe {
                Ok(_) => return Ok(machine),
                Err(error) if error.downcast_ref::<AuthRejected>().is_some() => return Err(error),
                Err(error) if Instant::now() >= deadline => {
                    let failed =
                        format!("boxd machine {vm_id} ran no command in {READY_TIMEOUT:?}");
                    return Err(error.context(failed));
                }
                Err(_) => tokio::time::sleep(crate::config::paced(READY_POLL)).await,
            }
        }
    }

    async fn exec(&self, vm_id: &str, command: &str) -> Result<tonic::Streaming<api::ExecChunk>> {
        let first = api::ExecChunk {
            vm_id: vm_id.into(),
            command: command.into(),
            ..Default::default()
        };
        // Ending the request stream after the first chunk closes the command's stdin.
        let (mut grpc, request) = self.grpc(futures::stream::iter([first])).await?;
        grpc.streaming(request, path("Exec"), tonic_prost::ProstCodec::default())
            .await
            .map(tonic::Response::into_inner)
            .map_err(|status| self.error(&format!("run a command in machine {vm_id}"), status))
    }

    /// Runs `command` with `bash -c` semantics and stdin closed. Its stdout, or an error with its
    /// exit code and stderr.
    pub async fn run(&self, vm_id: &str, command: &str) -> Result<String> {
        let mut stream = self.exec(vm_id, command).await?;
        let (mut stdout, mut stderr, mut code) = (Vec::new(), Vec::new(), None);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|status| {
                self.error(&format!("run a command in machine {vm_id}"), status)
            })?;
            code = chunk.data.is_empty().then_some(chunk.exit_code);
            let out = if chunk.is_stderr {
                &mut stderr
            } else {
                &mut stdout
            };
            out.extend(chunk.data);
        }
        match code {
            Some(0) => Ok(String::from_utf8_lossy(&stdout).into_owned()),
            Some(code) => Err(anyhow!(
                "a command in boxd machine {vm_id} exited {code}: {}",
                String::from_utf8_lossy(&stderr).trim()
            )),
            None => Err(anyhow!(
                "boxd ended a command in machine {vm_id} without its exit code"
            )),
        }
    }

    /// Writes `data` to `path` on the machine, streamed in 1 MiB chunks.
    async fn upload(&self, vm_id: &str, path_on_vm: &str, data: Vec<u8>) -> Result<()> {
        let total = data.len() as u64;
        let mut chunks: Vec<_> = data
            .chunks(1 << 20)
            .map(|bytes| api::UploadFileChunk {
                data: bytes.to_vec(),
                ..Default::default()
            })
            .collect();
        if chunks.is_empty() {
            chunks.push(api::UploadFileChunk::default());
        }
        chunks[0].vm_id = vm_id.into();
        chunks[0].path = path_on_vm.into();
        chunks[0].total_size = total;
        let (mut grpc, request) = self.grpc(futures::stream::iter(chunks)).await?;
        grpc.client_streaming::<_, _, api::Empty, _>(
            request,
            path("UploadFileStream"),
            tonic_prost::ProstCodec::default(),
        )
        .await
        .map_err(|status| self.error(&format!("upload {path_on_vm} to machine {vm_id}"), status))?;
        Ok(())
    }
}

/// A `--flavor` as a vCPU count. `cpu` (or none) leaves the size to the org's default; `cpu-<n>`
/// or `<n>` asks for n vCPUs with 4 GiB of memory each.
pub fn resolve_flavor(name: Option<&str>) -> Result<Option<u32>> {
    let flavor = name.unwrap_or_default().trim().to_ascii_lowercase();
    if flavor.is_empty() || flavor == "cpu" {
        return Ok(None);
    }
    match flavor.strip_prefix("cpu-").unwrap_or(&flavor).parse() {
        Ok(vcpus) if VCPUS.contains(&vcpus) => Ok(Some(vcpus)),
        _ => Err(anyhow!(
            "Unknown boxd flavor '{}'. boxd machines are CPU-only: use cpu (your org's default \
             size), or cpu-<n> or <n> for n vCPUs, where n is 1, 2, 4 or 8 (4 GiB of memory per \
             vCPU).",
            name.unwrap_or_default().trim()
        )),
    }
}

/// Rejects a size above the org's ceiling before anything is created.
pub fn check_size(vcpus: Option<u32>, defaults: &api::OrgDefaults) -> Result<()> {
    let ceiling = defaults.ceiling_vcpu;
    match vcpus {
        Some(vcpus) if ceiling > 0 && vcpus > ceiling => Err(anyhow!(
            "Your boxd org allows machines of at most {ceiling} vCPUs, and --flavor asks for \
             {vcpus}. Pass --flavor {ceiling} or smaller, or ask the org's admin to raise its quota."
        )),
        _ => Ok(()),
    }
}

/// A valid machine name for a run: lowercase alphanumerics and dashes, at most 63 bytes.
pub fn machine_name(run_id: &str) -> String {
    let prefix = setting(NAME_PREFIX).unwrap_or_else(|| "orx".into());
    let name = format!("{prefix}-{run_id}").to_ascii_lowercase();
    let mut name: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    name.truncate(63);
    name.trim_matches('-').to_string()
}

/// The most a machine of `vcpus` costs per hour while it runs, at boxd's list prices.
pub fn hourly_price(vcpus: u32) -> crate::store::Quote {
    let vcpus = u64::from(vcpus);
    crate::store::Quote {
        amount_micros: vcpus * (USD_MICROS_PER_VCPU_HOUR + 4 * USD_MICROS_PER_GIB_HOUR)
            + USD_MICROS_DISK_HOUR,
        per: crate::store::Per::Hour,
        currency: "USD".into(),
        source: "boxd.sh/pricing".into(),
        quoted_at: crate::store::now_ms(),
    }
}

/// A run's launch, as stored on it. Never holds secrets: `env` is filled at launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxdJobSpec {
    pub name: String,
    /// `bash` payload: the shared snapshot-and-run script.
    pub script: String,
    /// `None` takes the org's default size.
    pub vcpus: Option<u32>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub timeout_seconds: u64,
    /// `None` is boxd's home region.
    pub region: Option<String>,
    pub labels: HashMap<String, String>,
    pub source_archive: Option<PathBuf>,
}

/// A run's machine: its id, the region whose endpoint serves it, and its size (0 if unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub vm_id: String,
    pub region: String,
    pub vcpus: u32,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn env_file(env: &HashMap<String, String>) -> Result<String> {
    let mut keys: Vec<_> = env.keys().collect();
    keys.sort();
    let mut body = String::new();
    for key in keys {
        let valid = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err(anyhow!(
                "The run's environment has '{key}', which is not a shell variable name."
            ));
        }
        body.push_str(&format!("export {key}={}\n", shell_quote(&env[key])));
    }
    Ok(body)
}

/// Starts the job detached in its own session. The wrapper records its pid (a machine without one
/// never started the job), releases the source, sources and deletes the env file, runs the script
/// under `timeout`, then records the exit code. Only a simple command is backgrounded, so no
/// subshell holds the call's output open while the job runs.
fn start_command(timeout_seconds: u64) -> String {
    let wrapper = format!(
        "echo $$ > pid.tmp && mv pid.tmp pid; touch {SOURCE_ARCHIVE}.ready; \
         set -a; . ./env.sh; set +a; rm -f env.sh; \
         timeout -k 30 {timeout_seconds} bash ./run.sh > log 2>&1; \
         code=$?; if [ $code -eq 124 ]; then touch timedout; fi; \
         echo $code > exit.tmp && mv exit.tmp exit"
    );
    format!(
        "cd {RUN_DIR} || exit 1; setsid nohup bash -c {} > /dev/null 2>&1 < /dev/null & \
         for _ in $(seq 300); do [ -s pid ] && exit 0; sleep 0.1; done; \
         echo 'the job never wrote its pid' >&2; exit 1",
        shell_quote(&wrapper)
    )
}

/// Creates the run's machine, stages the job and starts it. A failure after the machine exists
/// deletes it and refuses the launch; if that delete fails too, recovery finds the machine by its
/// `or_run` label.
pub async fn launch(
    account: &Account,
    spec: &BoxdJobSpec,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Placed> {
    let env = env_file(&super::default_python_env(&spec.env))?;
    let archive = match &spec.source_archive {
        Some(path) => Some(
            tokio::fs::read(path)
                .await
                .with_context(|| format!("Couldn't read the run's source {}", path.display()))?,
        ),
        None => None,
    };
    let region = spec.region.clone().unwrap_or_default();
    if cancelled() {
        return Err(cancelled_before_start());
    }
    let machine = account.client(&region)?.create(spec).await?;
    let region = if machine.region.is_empty() {
        region
    } else {
        machine.region
    };
    let (vm_id, vcpus) = (machine.vm_id, machine.vcpu);
    let mut placed = Placed {
        vm_id,
        region,
        vcpus,
    };
    let client = account.client(&placed.region)?;
    let mut uploads = vec![(format!("{RUN_DIR}/env.sh"), env.into_bytes())];
    uploads.push((
        format!("{RUN_DIR}/run.sh"),
        spec.script.clone().into_bytes(),
    ));
    uploads.extend(archive.map(|tar| (SOURCE_ARCHIVE.to_string(), tar)));
    let mib = uploads.iter().map(|(_, data)| data.len()).sum::<usize>() >> 20;
    let deadline = account.stage_timeout + STAGE_PER_MIB * (mib as u32 + 1);
    let vm_id = placed.vm_id.clone();
    let staged = tokio::time::timeout(deadline, async {
        let hibernate_off = api::SetAutoHibernateTimeoutRequest {
            vm_id: vm_id.clone(),
            timeout_secs: 0,
        };
        client
            .call::<_, api::Empty>("SetAutoHibernateTimeout", hibernate_off)
            .await?
            .map_err(|status| {
                client.error(&format!("turn off auto-hibernate on {vm_id}"), status)
            })?;
        placed.vcpus = client.wait_until_ready(&vm_id).await?.vcpu;
        let mkdir = format!("rm -rf {RUN_DIR} && mkdir -m 700 -p {RUN_DIR}");
        client
            .run(&vm_id, &mkdir)
            .await
            .context("Couldn't make the run directory")?;
        for (path, data) in uploads {
            client.upload(&vm_id, &path, data).await?;
        }
        if cancelled() {
            return Err(cancelled_before_start());
        }
        client
            .run(&vm_id, &start_command(spec.timeout_seconds))
            .await
            .context("the job didn't start")?;
        Ok(())
    })
    .await
    .unwrap_or_else(|_| Err(anyhow!("staging the job took longer than {deadline:?}")));
    let Err(error) = staged else {
        return Ok(placed);
    };
    if let Err(cleanup) = client.delete(&vm_id).await {
        return Err(error.context(format!(
            "and deleting machine {vm_id} failed too ({cleanup:#}); orx finds it again by its run \
             label"
        )));
    }
    Err(super::Refused {
        status: None,
        message: format!(
            "Couldn't start the run in boxd machine {vm_id}, so orx deleted it: {error:#}"
        ),
    }
    .into())
}

fn cancelled_before_start() -> crate::error::Error {
    super::Refused {
        status: None,
        message: "The run was cancelled before its job started.".into(),
    }
    .into()
}

/// What the job's machine says, in the engine's stage words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobState {
    pub stage: &'static str,
    pub message: Option<String>,
    pub exit_code: Option<i32>,
}

impl JobState {
    fn failed(message: String) -> Self {
        Self {
            stage: "FAILED",
            message: Some(message),
            exit_code: None,
        }
    }
}

const STATUS_PROBE: &str = r#"cd /home/boxd/orx 2>/dev/null || { echo nostart; exit 0; }
code=; [ -f exit ] && code=$(cat exit)
if [ -f cancelled ]; then echo "cancelled $code"; exit 0; fi
if [ -n "$code" ]; then if [ -f timedout ]; then echo timedout; else echo "exit $code"; fi; exit 0; fi
if [ ! -s pid ]; then echo nostart; exit 0; fi
if kill -0 "$(cat pid)" 2>/dev/null; then echo running; exit 0; fi
if [ -f exit ]; then echo "exit $(cat exit)"; else echo lost; fi"#;

fn parse_probe(line: &str, vm_id: &str) -> Result<JobState> {
    let (word, code) = line.split_once(' ').unwrap_or((line, ""));
    let exit_code = code.trim().parse().ok();
    let stage = |stage| {
        Ok(JobState {
            stage,
            message: None,
            exit_code,
        })
    };
    match word {
        "running" => stage("RUNNING"),
        "exit" if exit_code.is_some() => stage("COMPLETED"),
        "cancelled" => stage("CANCELED"),
        "timedout" => Ok(JobState::failed(
            "timed out: the job ran past its --timeout and orx stopped it".into(),
        )),
        "nostart" => Ok(JobState::failed(format!(
            "the job never started in boxd machine {vm_id}: orx stopped while launching it. Run the \
             experiment again."
        ))),
        "lost" => Ok(JobState::failed(format!(
            "the job's process in boxd machine {vm_id} ended without an exit code, so the machine \
             restarted or something outside orx killed it"
        ))),
        _ => Err(anyhow!("Unreadable job status from boxd machine {vm_id}: {line:?}")),
    }
}

/// The run's machine, woken if it sleeps (its disk still holds the job's files); `None` when boxd
/// has no such machine.
async fn reach(client: &Client, vm_id: &str) -> Result<Option<api::Vm>> {
    match client.get(vm_id).await? {
        Some(machine) if !matches!(machine.status.as_str(), "running" | "failed" | "destroyed") => {
            client.wait_until_ready(vm_id).await.map(Some)
        }
        machine => Ok(machine),
    }
}

/// `None` when boxd has no such machine for this key.
pub async fn inspect(account: &Account, placed: &Placed) -> Result<Option<JobState>> {
    let client = account.client(&placed.region)?;
    let Some(machine) = reach(&client, &placed.vm_id).await? else {
        return Ok(None);
    };
    if machine.status != "running" {
        return Ok(Some(JobState::failed(format!(
            "boxd machine {} is {}, and the job went with it",
            placed.vm_id, machine.status
        ))));
    }
    let line = client
        .run(&placed.vm_id, STATUS_PROBE)
        .await
        .context("Couldn't read the job's status")?;
    parse_probe(line.trim(), &placed.vm_id).map(Some)
}

/// Marks the run cancelled unless it already exited, then stops every process of the job's
/// session: SIGTERM, and SIGKILL after 10 s. False when boxd has no such machine.
pub async fn cancel(account: &Account, placed: &Placed) -> Result<bool> {
    let client = account.client(&placed.region)?;
    let Some(machine) = reach(&client, &placed.vm_id).await? else {
        return Ok(false);
    };
    if machine.status != "running" {
        return Ok(true);
    }
    let stop = format!(
        "cd {RUN_DIR} 2>/dev/null || exit 0; [ -f exit ] || touch cancelled; [ -s pid ] || exit 0; \
         sid=$(cat pid); pkill -TERM -s \"$sid\"; \
         for _ in $(seq 20); do pgrep -s \"$sid\" > /dev/null || exit 0; sleep 0.5; done; \
         pkill -KILL -s \"$sid\"; exit 0"
    );
    tokio::time::timeout(account.cancel_timeout, client.run(&placed.vm_id, &stop))
        .await
        .unwrap_or_else(|_| {
            Err(anyhow!(
                "it took longer than {} s",
                account.cancel_timeout.as_secs()
            ))
        })
        .with_context(|| format!("Couldn't stop the job in boxd machine {}", placed.vm_id))?;
    Ok(true)
}

pub async fn finish(account: &Account, placed: &Placed) -> Result<()> {
    account.client(&placed.region)?.delete(&placed.vm_id).await
}

/// Finds the machine of a launch whose answer was lost, by its `or_run` label.
pub async fn find_run(account: &Account, spec: &BoxdJobSpec, run_id: &str) -> Result<Vec<Placed>> {
    let region = spec.region.clone().unwrap_or_default();
    let client = account.client(&region)?;
    let request = api::ListVmsRequest {
        label_selector: vec![format!("or_run={run_id}")],
    };
    let listed: api::ListVmsResponse = client
        .call("ListVms", request)
        .await?
        .map_err(|status| client.error("list the org's machines", status))?;
    Ok(listed
        .vms
        .into_iter()
        .filter(|machine| machine.labels.get("or_run").map(String::as_str) == Some(run_id))
        .map(|machine| Placed {
            vm_id: machine.vm_id,
            region: Some(machine.region)
                .filter(|r| !r.is_empty())
                .unwrap_or(region.clone()),
            vcpus: machine.vcpu,
        })
        .collect())
}

/// Follows the job's log from byte `offset`, handing `sink` its bytes with the offset after them.
/// True once every byte of it was read, false after `idle` without output. A machine that failed
/// has no more log to give, so its log ends with a note saying so.
pub async fn stream_logs(
    account: &Account,
    placed: &Placed,
    offset: &mut u64,
    idle: Duration,
    sink: &mut (dyn FnMut(&[u8], u64) + Send),
) -> Result<bool> {
    let client = account.client(&placed.region)?;
    let error = match follow_log(&client, &placed.vm_id, offset, idle, sink).await {
        Err(error) => error,
        followed => return followed,
    };
    // Not found may only mean this key can't see the machine, so only a failed one ends here.
    let status = match client.get(&placed.vm_id).await {
        Ok(Some(machine)) if matches!(machine.status.as_str(), "failed" | "destroyed") => {
            machine.status
        }
        _ => return Err(error),
    };
    let note = format!(
        "\n[orx] boxd machine {} is {status}, so the rest of its log is unavailable.\n",
        placed.vm_id
    );
    sink(note.as_bytes(), *offset);
    Ok(true)
}

/// `tail --pid` returns once the job's process is gone, so the log is then whole. A job that never
/// started has an empty log, which is whole at once.
async fn follow_log(
    client: &Client,
    vm_id: &str,
    offset: &mut u64,
    idle: Duration,
    sink: &mut (dyn FnMut(&[u8], u64) + Send),
) -> Result<bool> {
    let command = format!(
        "cd {RUN_DIR} 2>/dev/null && [ -s pid ] || {{ echo 'ORX_LOG_END 0' >&2; exit 0; }}; \
         touch log; tail -c +{} --pid=\"$(cat pid)\" -f log; echo \"ORX_LOG_END $(stat -c %s log)\" >&2",
        *offset + 1
    );
    let Ok(stream) = tokio::time::timeout(idle, client.exec(vm_id, &command)).await else {
        return Ok(false);
    };
    let mut stream = stream?;
    let (mut stderr, mut code) = (Vec::new(), None);
    loop {
        let chunk = match tokio::time::timeout(idle, stream.next()).await {
            Err(_) => return Ok(false),
            Ok(None) => break,
            Ok(Some(chunk)) => chunk.map_err(|status| {
                client.error(&format!("read the log in machine {vm_id}"), status)
            })?,
        };
        code = chunk.data.is_empty().then_some(chunk.exit_code);
        if chunk.is_stderr {
            stderr.extend(chunk.data);
        } else if !chunk.data.is_empty() {
            *offset += chunk.data.len() as u64;
            sink(&chunk.data, *offset);
        }
    }
    let stderr = String::from_utf8_lossy(&stderr);
    match code {
        Some(0) => {}
        Some(code) => {
            return Err(anyhow!(
                "Following the log in boxd machine {vm_id} exited {code}: {}",
                stderr.trim()
            ))
        }
        None => return Ok(false),
    }
    let end = stderr
        .lines()
        .find_map(|line| line.strip_prefix("ORX_LOG_END "))
        .and_then(|size| size.trim().parse::<u64>().ok());
    Ok(end.is_some_and(|size| *offset >= size))
}
