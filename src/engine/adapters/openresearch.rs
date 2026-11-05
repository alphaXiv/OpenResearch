use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orx_host_proto::{LogTarget, Request, Response};
use sha2::{Digest, Sha256};

use super::ssh::Ssh;
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::{ssh, BackendDescriptor};
use crate::store::StoredRun;

pub type TransportFactory =
    Arc<dyn Fn(&ssh::SshTarget) -> Result<Arc<LegacyTransport>> + Send + Sync>;

pub struct LegacyTransport {
    pub ssh: Arc<Ssh>,
    pub log_dir: PathBuf,
}

impl std::fmt::Debug for LegacyTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyTransport")
            .field("host", &self.ssh.host)
            .finish_non_exhaustive()
    }
}

impl LegacyTransport {
    pub fn new(ssh: Arc<Ssh>, log_dir: PathBuf) -> Self {
        Self { ssh, log_dir }
    }

    pub async fn execute(&self, command: &str, stdin: &[u8]) -> Result<String> {
        match self
            .ssh
            .request(Request::Exec {
                command: command.into(),
                stdin: stdin.into(),
                timeout_ms: 120_000,
            })
            .await?
        {
            Response::Exec { result }
                if result.code == Some(0) && !result.timed_out && !result.stdout_truncated =>
            {
                Ok(String::from_utf8(result.stdout)?)
            }
            Response::Exec { result } => Err(anyhow!(
                "Legacy command failed: {}",
                String::from_utf8_lossy(&result.stderr)
            )),
            _ => Err(anyhow!("Unexpected Exec response")),
        }
    }

    pub async fn upload(&self, command: &str, source: &Path) -> Result<String> {
        let mut hash = Sha256::new();
        let mut file = std::fs::File::open(source)?;
        std::io::copy(&mut file, &mut hash)?;
        let sha = format!("{:x}", hash.finalize());
        self.ssh
            .stage_blob(&sha, source, file.metadata()?.len())
            .await
            .map_err(|error| anyhow!("Source upload failed: {error:?}"))?;
        let blob = format!("{}/sources/{sha}", self.ssh.root);
        self.execute(
            &format!(
                "{{ {command}; }} < {}",
                ssh::shell_path(blob.strip_prefix("~/").unwrap_or(&blob))
            ),
            &[],
        )
        .await
    }
}

fn descriptor(handle: &Handle) -> Result<&BackendDescriptor> {
    match handle {
        Handle::OpenResearch { descriptor } | Handle::Legacy { descriptor }
            if descriptor.kind == "openresearch_job" =>
        {
            Ok(descriptor)
        }
        _ => Err(anyhow!("Not an OpenResearch run")),
    }
}

async fn rental_deleted(descriptor: &BackendDescriptor) -> Result<bool> {
    let Some(creds) = crate::config::load_credentials().await? else {
        return Ok(false);
    };
    Ok(crate::jobs::openresearch::box_deleted(&creds, descriptor.openresearch_ref()?.1).await)
}

fn kept_page(
    directory: &Path,
    run: &str,
    range: ByteRange,
    expected: Option<&FileIdent>,
) -> Result<LogPage> {
    let data_dir = directory
        .parent()
        .ok_or_else(|| anyhow!("Missing log parent"))?;
    let mut kept = crate::commands::up::api_v2::kept_ranges(data_dir, run)?;
    let mut partials = Vec::new();
    for suffix in ["log", "tail.log"] {
        let marker = directory.join(format!("{run}.{suffix}.part.json"));
        let bytes = match std::fs::read(&marker) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let (ident, offset, synced): (FileIdent, u64, u64) = serde_json::from_slice(&bytes)?;
        let part = directory.join(format!("{run}.{suffix}.part"));
        let metadata = match std::fs::metadata(&part) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let len = synced.min(metadata.len());
        if len != 0 {
            partials.push((
                std::fs::metadata(marker)?.modified()?,
                ident,
                offset,
                len,
                suffix,
            ));
        }
    }
    if let Some((_, generation, ..)) = partials.iter().max_by_key(|part| part.0) {
        kept.retain(|range| range.ident == *generation);
        for (_, ident, offset, len, suffix) in partials.iter().filter(|part| part.1 == *generation)
        {
            let path = directory.join(format!("{run}.{suffix}"));
            if kept
                .iter()
                .any(|range| range.file == path && range.offset == *offset && range.len >= *len)
            {
                continue;
            }
            let part = directory.join(format!("{run}.{suffix}.part"));
            let file = std::fs::OpenOptions::new().write(true).open(&part)?;
            file.set_len(*len)?;
            file.sync_all()?;
            std::fs::rename(part, &path)?;
            kept.retain(|range| range.file != path);
            kept.push(crate::commands::up::api_v2::KeptRange {
                offset: *offset,
                len: *len,
                file: path,
                ident: ident.clone(),
                eof: false,
            });
            std::fs::remove_file(directory.join(format!("{run}.{suffix}.part.json")))?;
        }
        kept.sort_by_key(|range| range.offset);
    }
    if kept.is_empty() {
        std::fs::create_dir_all(directory)?;
        let path = directory.join(format!("{run}.log"));
        crate::local::git::atomic_write_with_mode(&path, &[], Some(0o600))?;
        kept.push(crate::commands::up::api_v2::KeptRange {
            offset: 0,
            len: 0,
            file: path,
            ident: FileIdent { dev: 0, ino: 0 },
            eof: true,
        });
    }
    let last = kept.last_mut().unwrap();
    last.eof = true;
    let len = last.offset + last.len;
    let ident = last.ident.clone();
    crate::commands::up::api_v2::keep_ranges(data_dir, run, kept)?;
    let kept = crate::commands::up::api_v2::kept_ranges(data_dir, run)?;
    if range.start > len || expected.is_some_and(|expected| *expected != ident) {
        return Ok(LogPage::Reset {
            info: Some(orx_host_proto::LogInfo { ident, len }),
        });
    }
    let mut cursor = range.start;
    let mut bytes = Vec::new();
    for kept in kept {
        if kept.offset <= cursor && cursor <= kept.offset + kept.len {
            let end = range.end.min(kept.offset + kept.len);
            let mut file = std::fs::File::open(kept.file)?;
            file.seek(std::io::SeekFrom::Start(cursor - kept.offset))?;
            file.take(end - cursor).read_to_end(&mut bytes)?;
            cursor = end;
            if cursor == range.end || cursor == len {
                break;
            }
        }
    }
    if bytes.is_empty() && cursor < len && range.start != range.end {
        return Err(anyhow!("Page not on this computer"));
    }
    Ok(LogPage::Data {
        ident,
        len: Some(len),
        start: range.start,
        bytes,
        eof: cursor == len,
    })
}

pub struct OpenResearch {
    engine: Arc<Engine>,
    transport: TransportFactory,
    bootstrap: tokio::sync::Mutex<Option<crate::jobs::rental::Heartbeat>>,
}

const COLLECTION_GRACE: Duration = Duration::from_secs(15 * 60);
const MAX_ACTIVE_SECONDS: u64 = 7 * 24 * 3600;

const REJECTED_KEY_GRACE_MS: i64 = 15 * 60 * 1000;

fn rejected_key_expired(
    created_at: i64,
    contact: &crate::store::Contact,
    host: &crate::engine::transport::HostState,
    now: i64,
) -> bool {
    let crate::store::Contact::NeedsAuth { since, .. } = contact else {
        return false;
    };
    let cause = host
        .cause
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    host.status == crate::engine::transport::Status::NeedsAuth
        && cause.contains("permission denied (publickey)")
        && host
            .last_live_ms
            .is_some_and(|live| live >= created_at && live <= host.since_ms)
        && now.saturating_sub(host.since_ms.max(*since)) >= REJECTED_KEY_GRACE_MS
}

impl OpenResearch {
    pub fn new(engine: Arc<Engine>, transport: TransportFactory) -> Self {
        Self {
            engine,
            transport,
            bootstrap: tokio::sync::Mutex::new(None),
        }
    }

    async fn release(&self, descriptor: &BackendDescriptor) -> Result<()> {
        if let Some(rental) = &descriptor.rental_lease {
            match rental.operation("release").await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    if rental_deleted(descriptor).await? {
                        return Ok(());
                    }
                    return Err(error);
                }
            }
        }
        let creds = crate::config::load_credentials()
            .await?
            .ok_or_else(|| anyhow!("Missing OpenResearch credentials"))?;
        crate::jobs::openresearch::teardown(&creds, descriptor.openresearch_ref()?.1).await
    }

    async fn inspect(
        &self,
        watched: &WatchedRun,
        budget: Duration,
    ) -> Result<Vec<(String, Observation)>> {
        let (mut stored, state) = self.engine.load(&watched.id)?;
        if state.cancel_accepted_at.is_some() || phase_of(&stored)?.is_terminal() {
            return Ok(Vec::new());
        }
        let mut descriptor =
            descriptor(&handle_of(&stored).ok_or_else(|| anyhow!("Missing box handle"))?)?.clone();
        if self
            .cancel_rejected_key(&stored, &state.contact, &descriptor)
            .await?
        {
            return Ok(Vec::new());
        }
        let Some(_gate) = self.engine.try_gate(&watched.id) else {
            return Ok(Vec::new());
        };
        {
            let age = Duration::from_millis(
                self.engine
                    .clock
                    .wall_ms()
                    .saturating_sub(stored.created_at)
                    .max(0)
                    .unsigned_abs(),
            );
            let remaining = crate::engine::runtime::PREPARATION_BUDGET.saturating_sub(age);
            let mut bootstrap = self.bootstrap.lock().await;
            if matches!(watched.handle, Handle::OpenResearch { .. })
                && state.started_at.is_none()
                && state.contact == crate::store::Contact::Live
                && !remaining.is_zero()
            {
                if bootstrap.is_none() {
                    *bootstrap = descriptor
                        .rental_lease
                        .as_ref()
                        .map(|rental| rental.heartbeat().bounded(remaining));
                }
            } else {
                bootstrap.take();
            }
        }
        if matches!(watched.handle, Handle::OpenResearch { .. })
            && descriptor.rental_lease.is_none()
        {
            self.release(&descriptor).await?;
            return Ok(vec![(
                watched.id.clone(),
                Observation::Failed {
                    finished_at: None,
                    reason: "OpenResearch returned no supported rental lease; the box was deleted"
                        .into(),
                },
            )]);
        }
        if descriptor.openresearch_ssh_target().is_none() {
            let creds = crate::config::load_credentials()
                .await?
                .ok_or_else(|| anyhow!("Missing OpenResearch credentials"))?;
            let sandbox = match crate::client::get_sandbox(&creds, descriptor.openresearch_ref()?.1)
                .await
            {
                Ok(envelope) => envelope.sandbox,
                Err(error) if crate::jobs::openresearch::is_not_found_error(&error.to_string()) => {
                    return Ok(vec![(
                        watched.id.clone(),
                        Observation::Gone {
                            reason: crate::plane::LABELS["run_notice_rental_outcome_unavailable"]
                                .clone(),
                        },
                    )]);
                }
                Err(error) => return Err(error),
            };
            if sandbox.status == "failed" {
                return Ok(vec![(
                    watched.id.clone(),
                    Observation::Failed {
                        finished_at: None,
                        reason: sandbox
                            .failure_message
                            .unwrap_or_else(|| "Box provisioning failed".into()),
                    },
                )]);
            }
            if sandbox.status != "online"
                || sandbox.ssh_hostname.is_none()
                || sandbox.ssh_port.is_none()
                || sandbox.ssh_username.is_none()
            {
                return Ok(vec![(
                    watched.id.clone(),
                    Observation::Queued {
                        detail: "Provisioning box".into(),
                    },
                )]);
            }
            descriptor.ssh_host = sandbox.ssh_hostname;
            descriptor.ssh_port = sandbox.ssh_port;
            descriptor.ssh_user = sandbox.ssh_username;
            let handle = if matches!(watched.handle, Handle::OpenResearch { .. }) {
                Handle::OpenResearch {
                    descriptor: Box::new(descriptor.clone()),
                }
            } else {
                Handle::Legacy {
                    descriptor: Box::new(descriptor.clone()),
                }
            };
            let metadata = BackendDescriptor::parse(&stored.backend_json)?;
            stored.backend_json = handle::backend_json(&stored.id, Some(&handle), &metadata)?;
            self.engine
                .store()
                .set_backend_json(&stored.id, &stored.backend_json)?;
        }
        let target = descriptor.openresearch_ssh_target().unwrap();
        let transport = (self.transport)(&target)?;
        if matches!(watched.handle, Handle::Legacy { .. }) {
            let directory = format!("~/{}", crate::jobs::openresearch::run_dir(&watched.id));
            return match transport
                .ssh
                .request(Request::InspectLegacy { dir: directory })
                .await?
            {
                Response::InspectLegacy { inspection } => {
                    let observation = if let Some(code) = inspection.exit_code {
                        Observation::Exited {
                            finished_at: None,
                            code: Some(code),
                            signal: None,
                        }
                    } else if inspection.alive {
                        Observation::Running { started_at: None }
                    } else {
                        Observation::Queued {
                            detail: "Waiting for the rental supervisor".into(),
                        }
                    };
                    Ok(vec![(watched.id.clone(), observation)])
                }
                _ => Err(anyhow!("Unexpected legacy inspection")),
            };
        }
        let _follow = transport.ssh.follow_logs(&watched.id);
        let mut summary = transport
            .ssh
            .changes(
                std::slice::from_ref(&watched.id),
                budget.min(Duration::from_secs(1)),
            )
            .await?
            .pop()
            .ok_or_else(|| anyhow!("Missing box summary"))?
            .1;
        if matches!(
            summary.state,
            orx_host_proto::RunState::Missing | orx_host_proto::RunState::Staging
        ) {
            let (current, current_state) = self.engine.load(&watched.id)?;
            if current_state.cancel_accepted_at.is_some() || phase_of(&current)?.is_terminal() {
                return Ok(Vec::new());
            }
            let mut intent = RunIntent::new(stored, &state)?;
            intent.launch.target = Some(transport.ssh.host.clone());
            let command = crate::compute::staged_script(&intent.run.command);
            let script = format!("export PATH=\"$HOME/.local/bin:$HOME/.cargo/bin:$PATH\"; if ! command -v uv >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -LsSf https://astral.sh/uv/0.8.22/install.sh | env UV_NO_MODIFY_PATH=1 sh || exit; fi; {command}");
            let script = crate::jobs::openresearch::wrap_with_timeout(
                &script,
                descriptor.timeout_secs.unwrap_or(4 * 3600),
            );
            intent.launch.options = serde_json::json!({ "run_sh": script });
            let payload = transport
                .ssh
                .payload(&intent, &|| {
                    self.engine
                        .load(&watched.id)
                        .map_or(true, |(_, state)| state.cancel_accepted_at.is_some())
                })
                .await
                .map_err(|error| anyhow!("Box staging: {error:?}"))?;
            let rental = descriptor
                .rental_lease
                .as_ref()
                .ok_or_else(|| anyhow!("Missing rental capability"))?;
            let directory = format!("{}/runs/{}", transport.ssh.root, watched.id);
            let directory = if let Some(relative) = directory.strip_prefix("~/") {
                let home = transport.execute("printf '%s' \"$HOME\"", &[]).await?;
                format!("{}/{relative}", home.trim_end_matches('/'))
            } else {
                directory
            };
            let path = ssh::sh_quote(&directory);
            let setup = format!("umask 077; mkdir -p {path}; cat > {path}/rental.json.tmp && mv {path}/rental.json.tmp {path}/rental.json");
            let config = crate::jobs::rental::RemoteConfig {
                rental: rental.clone(),
                directory,
            };
            transport
                .execute(&setup, &serde_json::to_vec(&config)?)
                .await?;
            if self
                .engine
                .load(&watched.id)?
                .1
                .cancel_accepted_at
                .is_some()
            {
                return Ok(Vec::new());
            }
            summary = match transport.ssh.request(Request::Launch(payload)).await? {
                Response::Launch { summary } => summary,
                _ => return Err(anyhow!("Unexpected box launch reply")),
            };
        }
        if !matches!(
            summary.state,
            orx_host_proto::RunState::Missing
                | orx_host_proto::RunState::Staging
                | orx_host_proto::RunState::Starting
        ) {
            self.bootstrap.lock().await.take();
        }
        Ok(super::ssh::observations(watched.id.clone(), &summary))
    }

    async fn cancel_rejected_key(
        &self,
        run: &StoredRun,
        contact: &crate::store::Contact,
        descriptor: &BackendDescriptor,
    ) -> Result<bool> {
        let Some(target) = descriptor.openresearch_ssh_target() else {
            return Ok(false);
        };

        let Ok(transport) = (self.transport)(&target) else {
            return Ok(false);
        };
        let host = transport.ssh.conn.state().borrow().clone();
        if !rejected_key_expired(run.created_at, contact, &host, self.engine.clock.wall_ms()) {
            return Ok(false);
        }

        let Some(_gate) = self.engine.try_gate(&run.id) else {
            return Ok(true);
        };
        let (_, sandbox) = descriptor.openresearch_ref()?;
        let creds = crate::config::load_credentials()
            .await?
            .ok_or_else(|| anyhow!("Missing OpenResearch credentials"))?;
        let released = match crate::client::get_sandbox(&creds, sandbox).await {
            Ok(_) => false,
            Err(error) if crate::jobs::openresearch::is_not_found_error(&error.to_string()) => true,
            Err(error) => return Err(error),
        };

        let (current, state) = self.engine.load(&run.id)?;
        let host = transport.ssh.conn.state().borrow().clone();
        if current.cancel_requested
            || state.cancel_accepted_at.is_some()
            || phase_of(&current)?.is_terminal()
            || !rejected_key_expired(
                current.created_at,
                &state.contact,
                &host,
                self.engine.clock.wall_ms(),
            )
        {
            return Ok(true);
        }

        if released {
            self.engine.observe(
                &run.id,
                Observation::Gone {
                    reason: crate::plane::LABELS["run_notice_rental_outcome_unavailable"].clone(),
                },
            )?;
        } else {
            self.engine
                .cancel(
                    self,
                    &run.id,
                    &crate::plane::LABELS["run_notice_rental_key_cancelled"],
                )
                .await?;
        }

        Ok(true)
    }
}

#[async_trait]
impl Backend for OpenResearch {
    async fn launch(&self, intent: &RunIntent, _fence: &Fence<'_>) -> Result<Handle, LaunchError> {
        let mut descriptor =
            BackendDescriptor::parse(&intent.run.backend_json).map_err(super::api::rejected)?;
        descriptor.kind = "openresearch_job".into();
        let org = intent
            .launch
            .target
            .clone()
            .ok_or_else(|| super::api::rejected("Missing organization"))?;
        let flavor = intent.launch.options["flavor"]
            .as_str()
            .ok_or_else(|| super::api::rejected("Missing flavor"))?;
        let target = crate::jobs::openresearch::parse_flavor(
            flavor,
            intent.launch.options["disk_gb"].as_i64().unwrap_or(100),
            intent.launch.options["provider"]
                .as_str()
                .map(str::to_owned),
        )
        .map_err(super::api::rejected)?;
        let timeout = intent.launch.options["timeout_seconds"]
            .as_u64()
            .unwrap_or(4 * 3600);
        let collection_grace = COLLECTION_GRACE
            .as_secs()
            .min(MAX_ACTIVE_SECONDS.saturating_sub(timeout));
        let creds = crate::config::load_credentials()
            .await
            .map_err(super::api::rejected)?
            .ok_or_else(|| super::api::rejected("Missing OpenResearch credentials"))?;
        let created = crate::client::create_sandbox(
            &creds,
            &crate::client::CreateSandboxBody {
                organization_id: org.clone(),
                target,
                rental_lease: Some(crate::client::CreateRentalLease {
                    version: 1,
                    max_active_seconds: timeout + collection_grace,
                }),
            },
        )
        .await
        .map_err(|error| {
            // A 4xx answer to the create request proves no box exists.
            if error.to_string().contains(" failed (4") {
                super::api::rejected(crate::local::openresearch::billing_friendly(error))
            } else {
                super::api::interrupted(error)
            }
        })?;
        descriptor.namespace = Some(org);
        descriptor.job_id = Some(created.sandbox.id.clone());
        descriptor.timeout_secs = Some(timeout);
        let lease = match created.rental_lease.filter(|lease| lease.version == 1) {
            Some(lease) => lease,
            None => match crate::jobs::openresearch::teardown(&creds, &created.sandbox.id).await {
                Ok(()) => {
                    return Err(super::api::rejected(
                        "OpenResearch returned no supported rental lease; the box was deleted",
                    ))
                }
                Err(error) => {
                    eprintln!(
                        "Could not delete box {} after an unsupported rental lease: {error}",
                        created.sandbox.id
                    );
                    return Ok(Handle::OpenResearch {
                        descriptor: Box::new(descriptor),
                    });
                }
            },
        };
        descriptor.rental_lease = Some(crate::jobs::rental::Rental {
            api_url: creds.api_url,
            sandbox_id: created.sandbox.id,
            token: lease.token,
            unknown: Default::default(),
        });
        Ok(Handle::OpenResearch {
            descriptor: Box::new(descriptor),
        })
    }
    async fn recover(&self, _intent: &RunIntent) -> Recovery {
        super::api::unknown("OpenResearch provisioning outcome has no recorded sandbox id")
    }
    async fn observe(&self, runs: &[WatchedRun], budget: Duration) -> Vec<(String, Observation)> {
        let mut observations = Vec::new();
        for watched in runs {
            if !matches!(self.engine.own(&watched.id), Ok(true)) {
                continue;
            }
            let result = self.inspect(watched, budget).await;
            match result {
                Ok(mut changes) => observations.append(&mut changes),
                Err(error) => {
                    self.bootstrap.lock().await.take();
                    if let Ok(descriptor) = descriptor(&watched.handle) {
                        if rental_deleted(descriptor).await.unwrap_or(false) {
                            observations.push((
                                watched.id.clone(),
                                Observation::Gone {
                                    reason: crate::plane::LABELS
                                        ["run_notice_rental_outcome_unavailable"]
                                        .clone(),
                                },
                            ));
                            continue;
                        }
                    }
                    let observation = descriptor(&watched.handle)
                        .ok()
                        .and_then(BackendDescriptor::openresearch_ssh_target)
                        .and_then(|target| (self.transport)(&target).ok())
                        .map_or_else(
                            || Observation::Unreachable {
                                cause: error.to_string(),
                            },
                            |transport| transport.ssh.disconnected(error.to_string()),
                        );
                    observations.push((watched.id.clone(), observation));
                }
            }
        }
        observations
    }
    async fn cancel(&self, run: &WatchedRun) -> CancelOutcome {
        self.bootstrap.lock().await.take();
        let Ok(descriptor) = descriptor(&run.handle) else {
            return CancelOutcome::Pending;
        };
        if let Some(target) = descriptor.openresearch_ssh_target() {
            if let Ok(transport) = (self.transport)(&target) {
                if matches!(run.handle, Handle::OpenResearch { .. }) {
                    let _ = tokio::time::timeout(
                        Duration::from_secs(3),
                        transport.ssh.request(Request::Cancel {
                            run: run.id.clone(),
                        }),
                    )
                    .await;
                }
                let mut progressed = false;
                let collected = crate::engine::runtime::Runtime::collect_log(
                    transport.log_dir.parent().unwrap(),
                    self,
                    run,
                    &mut progressed,
                );
                let _ = tokio::time::timeout(Duration::from_secs(3), collected).await;
            }
        }
        let released = self.release(descriptor).await;
        match released {
            Ok(()) => CancelOutcome::Verified,
            Err(error) => CancelOutcome::Undelivered {
                detail: error.to_string(),
            },
        }
    }

    async fn finish(&self, run: &WatchedRun) -> Result<()> {
        self.bootstrap.lock().await.take();
        let descriptor = descriptor(&run.handle)?;
        let (_, state) = self.engine.load(&run.id)?;
        if state.cancel_accepted_at.is_some() && state.contact == crate::store::Contact::Live {
            return Ok(());
        }
        let transport = descriptor
            .openresearch_ssh_target()
            .and_then(|target| (self.transport)(&target).ok());
        if let Some(transport) = &transport {
            let directory = match run.handle {
                Handle::OpenResearch { .. } => format!("{}/runs/{}", transport.ssh.root, run.id),
                _ => format!("~/{}", crate::jobs::openresearch::run_dir(&run.id)),
            };
            let command = format!(
                "touch {}/final_collected",
                ssh::shell_path(directory.strip_prefix("~/").unwrap_or(&directory))
            );
            let _ = tokio::time::timeout(Duration::from_secs(3), transport.execute(&command, &[]))
                .await;
        }
        self.release(descriptor).await?;
        if let Some(transport) = transport {
            let data_dir = transport
                .log_dir
                .parent()
                .ok_or_else(|| anyhow!("Missing log parent"))?;
            let mut ranges = crate::commands::up::api_v2::kept_ranges(data_dir, &run.id)?;
            if let Some(last) = ranges.last_mut() {
                last.eof = true;
                crate::commands::up::api_v2::keep_ranges(data_dir, &run.id, ranges)?;
            }
        }
        Ok(())
    }

    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage> {
        let descriptor = descriptor(&run.handle)?;
        let Some(target) = descriptor.openresearch_ssh_target() else {
            return Ok(LogPage::Reset { info: None });
        };
        let transport = (self.transport)(&target)?;
        let (stored, state) = self.engine.load(&run.id)?;
        let released = phase_of(&stored)?.is_terminal()
            && state.cancel_accepted_at.is_some()
            && state.contact == crate::store::Contact::Live;
        let page = if released {
            Err(anyhow!("Rental released"))
        } else if matches!(run.handle, Handle::OpenResearch { .. }) {
            let native = WatchedRun {
                id: run.id.clone(),
                handle: transport.ssh.handle(),
            };
            transport
                .ssh
                .read_log(&native, file, range.clone(), ident)
                .await
        } else {
            match transport
                .ssh
                .request(Request::ReadLog {
                    target: LogTarget::Dir(super::ssh::legacy_dir(
                        &crate::jobs::openresearch::run_dir(&run.id),
                    )),
                    file,
                    range: range.clone(),
                    ident: ident.cloned(),
                    anchor: None,
                })
                .await
            {
                Ok(Response::ReadLog { page }) => Ok(page),
                Ok(_) => Err(anyhow!("Unexpected box log reply")),
                Err(error) => Err(error),
            }
        };
        match page {
            Ok(page) => Ok(page),
            Err(error) => {
                let gone = released || rental_deleted(descriptor).await?;
                if !gone {
                    return Err(error);
                }
                if file != LogFile::Log {
                    return Ok(LogPage::Reset { info: None });
                }
                kept_page(&transport.log_dir, &run.id, range, ident)
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn released_rental_preserves_completed_head_after_tail_failure() {
        const HEAD: u64 = 64 << 20;
        const LEN: u64 = (1 << 30) + 100;
        const TAIL: u64 = LEN - (256 << 20);
        struct Log(u64);

        #[async_trait]
        impl Backend for Log {
            async fn launch(
                &self,
                _: &RunIntent,
                _: &Fence<'_>,
            ) -> std::result::Result<Handle, LaunchError> {
                unreachable!()
            }
            async fn recover(&self, _: &RunIntent) -> Recovery {
                unreachable!()
            }
            async fn observe(&self, _: &[WatchedRun], _: Duration) -> Vec<(String, Observation)> {
                unreachable!()
            }
            async fn cancel(&self, _: &WatchedRun) -> CancelOutcome {
                unreachable!()
            }
            async fn read_log(
                &self,
                _: &WatchedRun,
                _: LogFile,
                range: ByteRange,
                _: Option<&FileIdent>,
            ) -> Result<LogPage> {
                let ident = FileIdent { dev: 1, ino: 2 };
                if range.start == u64::MAX - 1 {
                    return Ok(LogPage::Reset {
                        info: Some(orx_host_proto::LogInfo { ident, len: LEN }),
                    });
                }
                let bytes = match range.start {
                    HEAD => Vec::new(),
                    TAIL if self.0 != 0 => vec![b't'; self.0 as usize],
                    _ => return Err(anyhow!("SSH proxy disconnected")),
                };
                Ok(LogPage::Data {
                    ident,
                    len: Some(LEN),
                    start: range.start,
                    bytes,
                    eof: false,
                })
            }
        }

        let root =
            std::env::temp_dir().join(format!("orx-completed-head-{}", uuid::Uuid::new_v4()));
        let directory = root.join("run-logs");
        std::fs::create_dir_all(&directory).unwrap();
        let ident = FileIdent { dev: 1, ino: 2 };
        for (synced_tail, saved_tail) in [(0, false), (17, false), (0, true)] {
            let run = WatchedRun {
                id: format!("tail-{synced_tail}-{saved_tail}"),
                handle: Handle::Local {
                    dir: "/unused".into(),
                },
            };
            let part = directory.join(format!("{}.log.part", run.id));
            std::fs::write(&part, b"head\n").unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&part)
                .unwrap()
                .set_len(HEAD)
                .unwrap();
            std::fs::write(
                directory.join(format!("{}.log.part.json", run.id)),
                serde_json::to_vec(&(&ident, 0u64, HEAD)).unwrap(),
            )
            .unwrap();
            let tail = directory.join(format!("{}.tail.log", run.id));
            if saved_tail {
                std::fs::write(&tail, b"previous tail\n").unwrap();
                crate::commands::up::api_v2::keep_ranges(
                    &root,
                    &run.id,
                    vec![crate::commands::up::api_v2::KeptRange {
                        offset: TAIL - 100,
                        len: 14,
                        file: tail.clone(),
                        ident: ident.clone(),
                        eof: true,
                    }],
                )
                .unwrap();
            }
            let error = crate::engine::runtime::Runtime::collect_log(
                &root,
                &Log(synced_tail),
                &run,
                &mut false,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("SSH proxy disconnected"));
            assert!(!directory.join(format!("{}.log.part.json", run.id)).exists());
            let kept = crate::commands::up::api_v2::kept_ranges(&root, &run.id).unwrap();
            assert!(kept.iter().all(|range| !range.eof));
            assert!(matches!(
                kept_page(&directory, &run.id, ByteRange { start: 0, end: 5 }, Some(&ident)).unwrap(),
                LogPage::Data { bytes, .. } if bytes == b"head\n"
            ));
            if saved_tail {
                assert_eq!(std::fs::read(&tail).unwrap(), b"previous tail\n");
            }
            if synced_tail != 0 {
                assert!(matches!(
                    kept_page(&directory, &run.id, ByteRange { start: TAIL, end: TAIL + synced_tail }, Some(&ident)).unwrap(),
                    LogPage::Data { bytes, .. } if bytes == vec![b't'; synced_tail as usize]
                ));
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn round_two_rotated_log_discards_obsolete_tail_partial() {
        let root = std::env::temp_dir().join(format!("orx-rotated-part-{}", uuid::Uuid::new_v4()));
        let directory = root.join("run-logs");
        std::fs::create_dir_all(&directory).unwrap();
        let previous = FileIdent { dev: 2, ino: 1 };
        let current = FileIdent { dev: 2, ino: 2 };
        let payload = b"generation B\n";
        let backend = crate::engine::fake::FakeBackend::default();
        *backend.log.lock().unwrap() = crate::engine::fake::HostFile {
            bytes: payload.to_vec(),
            ino: current.ino,
        };
        for cached in [false, true] {
            let run = WatchedRun {
                id: format!("cached-{cached}"),
                handle: Handle::Local {
                    dir: "/unused".into(),
                },
            };
            if cached {
                crate::engine::runtime::Runtime::collect_log(&root, &backend, &run, &mut false)
                    .await
                    .unwrap();
            }
            let part = directory.join(format!("{}.tail.log.part", run.id));
            let marker = directory.join(format!("{}.tail.log.part.json", run.id));
            std::fs::write(&part, b"generation A tail").unwrap();
            std::fs::write(
                &marker,
                serde_json::to_vec(&(&previous, 1u64 << 30, 17u64)).unwrap(),
            )
            .unwrap();
            crate::engine::runtime::Runtime::collect_log(&root, &backend, &run, &mut false)
                .await
                .unwrap();
            assert!(
                matches!(kept_page(&directory, &run.id, ByteRange { start: 0, end: payload.len() as u64 }, Some(&current)).unwrap(),
                LogPage::Data { bytes, ident, eof: true, .. } if bytes == payload && ident == current)
            );
            assert_eq!(
                std::fs::read(directory.join(format!("{}.log", run.id))).unwrap(),
                payload
            );
            assert!(!part.exists());
            assert!(!marker.exists());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn round_one_released_rental_preserves_synced_partial_ranges() {
        let root =
            std::env::temp_dir().join(format!("orx-released-parts-{}", uuid::Uuid::new_v4()));
        let directory = root.join("run-logs");
        std::fs::create_dir_all(&directory).unwrap();
        let ident = FileIdent { dev: 1, ino: 2 };
        for (run, tail) in [("short", None), ("sparse", Some((1u64 << 30) + 100))] {
            for (suffix, offset, synced) in
                std::iter::once(("log", 0, 49)).chain(tail.map(|offset| ("tail.log", offset, 17)))
            {
                std::fs::write(
                    directory.join(format!("{run}.{suffix}.part")),
                    vec![b'x'; synced + 31],
                )
                .unwrap();
                std::fs::write(
                    directory.join(format!("{run}.{suffix}.part.json")),
                    serde_json::to_vec(&(&ident, offset, synced as u64)).unwrap(),
                )
                .unwrap();
            }
            let expected_len = tail.map_or(49, |offset| offset + 17);
            assert!(
                matches!(kept_page(&directory, run, ByteRange { start: u64::MAX - 1, end: u64::MAX }, None).unwrap(),
                LogPage::Reset { info: Some(info) } if info.ident == ident && info.len == expected_len)
            );
            for (offset, len) in std::iter::once((0, 49)).chain(tail.map(|offset| (offset, 17))) {
                assert!(
                    matches!(kept_page(&directory, run, ByteRange { start: offset, end: offset + len }, Some(&ident)).unwrap(),
                    LogPage::Data { bytes, .. } if bytes == vec![b'x'; len as usize])
                );
            }
            let kept = crate::commands::up::api_v2::kept_ranges(&root, run).unwrap();
            assert!(kept.last().unwrap().eof);
            assert_eq!(
                kept.iter().map(|range| range.len).sum::<u64>(),
                if tail.is_some() { 66 } else { 49 }
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejected_key_grace_requires_continuous_auth_failure_and_live_contact_in_this_run() {
        use crate::engine::transport::{HostState, Status};
        use crate::store::Contact;

        let created_at = 100;
        let contact = Contact::NeedsAuth {
            since: 200,
            cause: "Permission denied (publickey)".into(),
        };
        let host = HostState {
            status: Status::NeedsAuth,
            since_ms: 200,
            last_live_ms: Some(100),
            cause: Some("Permission denied (publickey)".into()),
            ..HostState::default()
        };
        assert!(!rejected_key_expired(
            created_at,
            &contact,
            &host,
            200 + 14 * 60_000
        ));
        assert!(rejected_key_expired(
            created_at,
            &contact,
            &host,
            200 + 15 * 60_000
        ));
        for (contact, host) in [
            (
                Contact::Live,
                HostState {
                    status: Status::Live,
                    ..host.clone()
                },
            ),
            (
                Contact::Unreachable {
                    since: 200,
                    cause: "Connection timed out".into(),
                },
                HostState {
                    status: Status::Backoff,
                    ..host.clone()
                },
            ),
            (
                contact.clone(),
                HostState {
                    last_live_ms: None,
                    ..host.clone()
                },
            ),
            (
                contact.clone(),
                HostState {
                    last_live_ms: Some(99),
                    ..host.clone()
                },
            ),
            (
                contact.clone(),
                HostState {
                    last_live_ms: Some(201),
                    ..host.clone()
                },
            ),
            (
                contact.clone(),
                HostState {
                    cause: Some("Host key verification failed".into()),
                    ..host.clone()
                },
            ),
            (
                contact.clone(),
                HostState {
                    cause: Some("SSH master ended; press Connect".into()),
                    ..host.clone()
                },
            ),
            (
                contact.clone(),
                HostState {
                    cause: Some("Permission denied (password)".into()),
                    ..host.clone()
                },
            ),
        ] {
            assert!(!rejected_key_expired(
                created_at,
                &contact,
                &host,
                200 + 60 * 60_000
            ));
        }
        let after_connect = HostState {
            since_ms: 200 + 14 * 60_000,
            ..host.clone()
        };
        assert!(!rejected_key_expired(
            created_at,
            &contact,
            &after_connect,
            200 + 15 * 60_000
        ));
        assert!(rejected_key_expired(
            created_at,
            &contact,
            &after_connect,
            after_connect.since_ms + 15 * 60_000
        ));
        let after_recovery = HostState {
            last_live_ms: Some(200 + 13 * 60_000),
            ..after_connect.clone()
        };
        assert!(!rejected_key_expired(
            created_at,
            &contact,
            &after_recovery,
            200 + 15 * 60_000
        ));
        assert!(rejected_key_expired(
            created_at,
            &contact,
            &after_recovery,
            after_recovery.since_ms + 15 * 60_000
        ));
        let recovered_at_fourteen = Contact::NeedsAuth {
            since: 200 + 14 * 60_000,
            cause: host.cause.clone().unwrap(),
        };
        assert!(!rejected_key_expired(
            created_at,
            &recovered_at_fourteen,
            &host,
            200 + 15 * 60_000
        ));
    }
}
