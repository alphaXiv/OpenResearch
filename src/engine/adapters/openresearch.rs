use std::collections::HashMap;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orx_host_proto::{LogTarget, Request, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::ssh::Ssh;
use crate::commands::supervise::RunSink;
use crate::engine::*;
use crate::error::{anyhow, Result};
use crate::jobs::{ssh, BackendDescriptor};
use crate::store::{RunStatus, StoredRun};

pub type TransportFactory =
    Arc<dyn Fn(&ssh::SshTarget) -> Result<Arc<LegacyTransport>> + Send + Sync>;

pub struct LegacyTransport {
    pub ssh: Arc<Ssh>,
    pub log_dir: PathBuf,
    collect: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for LegacyTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LegacyTransport")
            .field("host", &self.ssh.host)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    ident: Option<FileIdent>,
    offset: u64,
}

impl LegacyTransport {
    pub fn new(ssh: Arc<Ssh>, log_dir: PathBuf) -> Self {
        Self {
            ssh,
            log_dir,
            collect: tokio::sync::Mutex::new(()),
        }
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

    pub async fn page(
        &self,
        dir: &str,
        file: LogFile,
        range: ByteRange,
        ident: Option<FileIdent>,
        anchor: Option<[u8; 64]>,
    ) -> Result<LogPage> {
        let dir = super::ssh::legacy_dir(dir);
        match self
            .ssh
            .request(Request::ReadLog {
                target: LogTarget::Dir(dir),
                file,
                ident,
                range,
                anchor,
            })
            .await?
        {
            Response::ReadLog { page } => Ok(page),
            _ => Err(anyhow!("Unexpected legacy log response")),
        }
    }

    pub async fn collect(&self, run: &str, dir: &str) -> Result<serde_json::Value> {
        let _guard = self.collect.lock().await;
        uuid::Uuid::parse_str(run)?;
        std::fs::create_dir_all(&self.log_dir)?;
        let path = self.log_dir.join(format!("{run}.log"));
        let cursor_path = path.with_extension("engine-cursor.json");
        let mut cursor: Cursor = match std::fs::read(&cursor_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Cursor {
                ident: None,
                offset: 0,
            },
            Err(error) => return Err(error.into()),
        };
        let mut anchor = None;
        if cursor.offset != 0 {
            match std::fs::File::open(&path) {
                Ok(mut file) if file.metadata()?.len() >= cursor.offset => {
                    if cursor.offset >= 64 {
                        let mut bytes = [0; 64];
                        file.seek(std::io::SeekFrom::Start(cursor.offset - 64))?;
                        file.read_exact(&mut bytes)?;
                        anchor = Some(bytes);
                    }
                }
                Ok(_) => {
                    cursor = Cursor {
                        ident: None,
                        offset: 0,
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    cursor = Cursor {
                        ident: None,
                        offset: 0,
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        let page = self
            .page(
                dir,
                LogFile::Log,
                ByteRange {
                    start: cursor.offset,
                    end: cursor.offset + 256 * 1024,
                },
                cursor.ident.clone(),
                anchor,
            )
            .await?;
        let eof = match page {
            LogPage::Reset { info: Some(_) } => {
                cursor = Cursor {
                    ident: None,
                    offset: 0,
                };
                false
            }
            LogPage::Reset { info: None } => return Err(anyhow!("Legacy log is not available")),
            LogPage::Data {
                len: _,
                ident,
                start,
                bytes,
                eof,
            } => {
                if start != cursor.offset {
                    return Err(anyhow!("Legacy log offset changed"));
                }
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .write(true)
                    .open(&path)?;
                file.set_len(cursor.offset)?;
                file.seek(std::io::SeekFrom::Start(cursor.offset))?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                cursor.ident = Some(ident);
                cursor.offset += bytes.len() as u64;
                eof
            }
        };
        crate::local::git::atomic_write_with_mode(
            &cursor_path,
            &serde_json::to_vec(&cursor)?,
            Some(0o600),
        )?;
        #[cfg(unix)]
        std::fs::File::open(&self.log_dir)?.sync_all()?;
        Ok(serde_json::json!({ "eof": eof }))
    }
}

struct EngineSink {
    engine: Arc<Engine>,
}

impl RunSink for EngineSink {
    fn finish(
        &self,
        run: &str,
        status: RunStatus,
        exit_code: Option<i64>,
        reason: Option<&str>,
    ) -> Result<bool> {
        let phase =
            Phase::parse(status.as_str()).ok_or_else(|| anyhow!("Unknown legacy status"))?;
        self.engine.commit(run, |_, _| reconcile::Change {
            exit_code,
            ..reconcile::Change::terminal(phase, reason.map(str::to_owned))
        })?;
        Ok(true)
    }
    fn get_run(&self, run: &str) -> Result<Option<StoredRun>> {
        let store = self.engine.store();
        let mut stored = store.get_run(run)?;
        if let Some(stored) = &mut stored {
            let state = store.engine_run_state(run)?;
            stored.cancel_requested = state.is_some_and(|state| state.cancel_accepted_at.is_some());
        }
        Ok(stored)
    }
    fn update_status(
        &self,
        run: &str,
        status: RunStatus,
        _ended_at: Option<i64>,
        exit_code: Option<i64>,
    ) -> Result<bool> {
        let observation = match status {
            RunStatus::Starting => Observation::Queued {
                detail: "Legacy loop starting".into(),
            },
            RunStatus::Running => Observation::Running,
            RunStatus::Done => Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
            RunStatus::Cancelled => Observation::Cancelled {
                finished_at: None,
                code: None,
                signal: None,
            },
            RunStatus::Lost => Observation::Lost {
                reason: "Legacy outcome unavailable".into(),
            },
            RunStatus::Failed => Observation::Exited {
                finished_at: None,
                code: Some(
                    exit_code
                        .and_then(|code| i32::try_from(code).ok())
                        .unwrap_or(1),
                ),
                signal: None,
            },
        };
        self.engine.observe(run, observation)?;
        Ok(true)
    }
    fn set_result_markdown(&self, run: &str, result: &str) -> Result<()> {
        self.engine.store().set_result_markdown(run, result)
    }
    fn set_backend_json(&self, run: &str, descriptor: &str) -> Result<()> {
        let descriptor = BackendDescriptor::parse(descriptor)?;
        let handle = Handle::Legacy {
            descriptor: Box::new(descriptor.clone()),
        };
        let json = handle::backend_json(run, Some(&handle), &descriptor)?;
        self.engine.store().set_backend_json(run, &json)
    }
    fn queue_finished_remote_cleanup(&self, run: &str) -> Result<()> {
        self.engine.store().queue_finished_remote_cleanup(run)
    }
    fn transport_error(&self, run: &str, target: &ssh::SshTarget, cause: &str) -> Result<()> {
        let transport = target
            .transport
            .as_ref()
            .ok_or_else(|| anyhow!("Missing engine transport"))?;
        self.engine
            .observe(run, transport.ssh.disconnected(cause.into()))
    }
}

pub struct OpenResearch {
    engine: Arc<Engine>,
    transport: TransportFactory,
    loops: tokio::sync::Mutex<HashMap<String, tokio::task::JoinHandle<()>>>,
}

impl OpenResearch {
    pub fn new(engine: Arc<Engine>, transport: TransportFactory) -> Self {
        Self {
            engine,
            transport,
            loops: tokio::sync::Mutex::default(),
        }
    }
}

impl Drop for OpenResearch {
    fn drop(&mut self) {
        for task in self.loops.get_mut().values() {
            task.abort();
        }
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
                    max_active_seconds: timeout,
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
                    return Ok(Handle::Legacy {
                        descriptor: Box::new(descriptor),
                    });
                }
            },
        };
        descriptor.rental_lease = Some(crate::jobs::rental::Rental {
            api_url: creds.api_url,
            sandbox_id: created.sandbox.id,
            token: lease.token,
        });
        Ok(Handle::Legacy {
            descriptor: Box::new(descriptor),
        })
    }
    async fn recover(&self, _intent: &RunIntent) -> Recovery {
        super::api::unknown("OpenResearch provisioning outcome has no recorded sandbox id")
    }
    async fn observe(&self, runs: &[WatchedRun], _budget: Duration) -> Vec<(String, Observation)> {
        let mut tasks = self.loops.lock().await;
        for run in runs {
            if tasks.get(&run.id).is_some_and(|task| !task.is_finished()) {
                continue;
            }
            if !matches!(self.engine.own(&run.id), Ok(true)) {
                continue;
            }
            let Handle::Legacy { .. } = &run.handle else {
                continue;
            };
            let Ok((stored, _)) = self.engine.load(&run.id) else {
                continue;
            };
            let Some(Handle::Legacy { descriptor }) = handle_of(&stored) else {
                continue;
            };
            let descriptor = *descriptor;
            let sink = EngineSink {
                engine: self.engine.clone(),
            };
            let factory = self.transport.clone();
            let id = run.id.clone();
            tasks.insert(
                run.id.clone(),
                tokio::spawn(async move {
                    if let Err(error) = crate::commands::supervise::run_openresearch_loop(
                        &sink,
                        stored,
                        descriptor,
                        id.clone(),
                        Some(factory),
                    )
                    .await
                    {
                        let current = sink.engine.load(&id).ok().map(|(_, state)| state.contact);
                        if !matches!(current, Some(crate::store::Contact::NeedsAuth { .. })) {
                            let _ = sink.engine.observe(
                                &id,
                                Observation::Unreachable {
                                    cause: error.to_string(),
                                },
                            );
                        }
                    }
                }),
            );
        }
        vec![]
    }
    async fn cancel(&self, _run: &WatchedRun) -> CancelOutcome {
        CancelOutcome::Pending
    }
    async fn unwatch(&self, run: &WatchedRun) {
        let task = self.loops.lock().await.remove(&run.id);
        if let Some(task) = task {
            let _ = task.await;
        }
    }
    async fn read_log(
        &self,
        run: &WatchedRun,
        file: LogFile,
        range: ByteRange,
        ident: Option<&FileIdent>,
    ) -> Result<LogPage> {
        let Handle::Legacy { descriptor } = &run.handle else {
            return Err(anyhow!("Not a legacy run"));
        };
        let target = descriptor
            .openresearch_ssh_target()
            .ok_or_else(|| anyhow!("Sandbox has no SSH endpoint"))?;
        (self.transport)(&target)?
            .page(
                &crate::jobs::openresearch::run_dir(&run.id),
                file,
                range,
                ident.cloned(),
                None,
            )
            .await
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn engine_sink_keeps_legacy_lease_and_terminal_reason_in_the_notice() {
        let root = std::env::temp_dir().join(format!("orx-legacy-sink-{}", uuid::Uuid::new_v4()));
        let engine = Arc::new(super::super::api_tests::engine(&root));
        let descriptor = BackendDescriptor::parse(r#"{"kind":"openresearch_job","namespace":"org","jobId":"box","rentalLease":{"apiUrl":"https://example.invalid","sandboxId":"box","token":"private"}}"#).unwrap();
        let run = StoredRun {
            id: "run".into(),
            experiment_id: "exp".into(),
            project_id: "proj".into(),
            status: "running".into(),
            backend_json: descriptor.to_json(),
            command: "true".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: Some("chat".into()),
        };
        engine.store().upsert_run(&run).unwrap();
        let sink = EngineSink {
            engine: engine.clone(),
        };
        sink.set_backend_json("run", &descriptor.to_json()).unwrap();
        let stored = sink.get_run("run").unwrap().unwrap();
        let Some(Handle::Legacy { descriptor: saved }) = handle_of(&stored) else {
            panic!("lost legacy envelope");
        };
        assert_eq!(saved.rental_lease.unwrap().token, "private");
        sink.finish(
            "run",
            RunStatus::Failed,
            Some(7),
            Some("Job failed: payload exited 7"),
        )
        .unwrap();
        let notices = engine.store().run_notice_batch("chat").unwrap();
        assert_eq!(notices.len(), 1);
        assert!(notices[0].body.contains("payload exited 7"));
        // The shipped Store sink keeps the old descriptor and writes no engine notices.
        let store = std::sync::Mutex::new(Store::open_at(root.join("legacy")).unwrap());
        store.lock().unwrap().upsert_run(&run).unwrap();
        store
            .set_backend_json("run", &descriptor.to_json())
            .unwrap();
        store
            .finish(
                "run",
                RunStatus::Failed,
                Some(7),
                Some("Job failed: payload exited 7"),
            )
            .unwrap();
        let legacy = store.get_run("run").unwrap().unwrap();
        assert_eq!(legacy.backend_json, descriptor.to_json());
        assert_eq!(legacy.status, "failed");
        assert_eq!(legacy.exit_code, Some(7));
        assert!(store
            .lock()
            .unwrap()
            .run_notice_batch("chat")
            .unwrap()
            .is_empty());
        drop(store);
        drop(sink);
        drop(engine);
        std::fs::remove_dir_all(root).unwrap();
    }
}
