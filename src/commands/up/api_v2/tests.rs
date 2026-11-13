use super::*;
use crate::commands::remote_host::ServerDescriptor;
use crate::engine::handle::Handle;
use crate::engine::{
    CancelOutcome, Fence, FileIdent, LaunchError, Observation, Recovery, RunIntent,
};
use crate::plane::local_plane::ApiClient;
use crate::store::{LaunchSpec, SubmittingRun};
use futures::StreamExt;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
struct Fake {
    offline: AtomicBool,
    outage: Mutex<Option<crate::engine::transport::HostState>>,
    invalid_log: AtomicBool,
    scripted_pages: Mutex<VecDeque<LogPage>>,
    identities: Mutex<Vec<Option<FileIdent>>>,
    launch_gate: Mutex<Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>>,
    /// A stop the backend never answers.
    cancel_hangs: AtomicBool,
    /// A stop the backend refuses each time.
    cancel_refused: AtomicBool,
    cancel_entered: tokio::sync::Notify,
    cancels: AtomicUsize,
    reads: AtomicUsize,
    follows: Arc<AtomicUsize>,
    /// Launch admissions currently held.
    admitted: Arc<AtomicUsize>,
}
#[async_trait]
impl Backend for Fake {
    async fn launch(
        &self,
        _: &RunIntent,
        _: &Fence<'_>,
    ) -> std::result::Result<Handle, LaunchError> {
        let gate = self.launch_gate.lock().unwrap().clone();
        if let Some((entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
        Ok(Handle::Local {
            dir: "/fake".into(),
        })
    }
    async fn recover(&self, _: &RunIntent) -> Recovery {
        Recovery::Absent
    }
    async fn observe(&self, _: &[WatchedRun], _: Duration) -> Vec<(String, Observation)> {
        vec![]
    }
    async fn cancel(&self, _: &WatchedRun) -> CancelOutcome {
        self.cancel_entered.notify_one();
        self.cancels.fetch_add(1, Ordering::SeqCst);
        if self.cancel_hangs.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        if self.cancel_refused.load(Ordering::SeqCst) {
            return CancelOutcome::Undelivered {
                detail: "refused by the provider".into(),
            };
        }
        CancelOutcome::Pending
    }
    async fn read_log(
        &self,
        _: &WatchedRun,
        _: LogFile,
        range: ByteRange,
        expected: Option<&FileIdent>,
    ) -> Result<LogPage> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.identities.lock().unwrap().push(expected.cloned());
        if let Some(page) = self.scripted_pages.lock().unwrap().pop_front() {
            return Ok(page);
        }
        if self.offline.load(Ordering::SeqCst) {
            if let Some(state) = self.outage.lock().unwrap().clone() {
                return Err(anyhow!(crate::engine::transport::Unavailable {
                    state,
                    sent: false
                }));
            }
            return Err(std::io::Error::from(std::io::ErrorKind::NotConnected).into());
        }
        if self.invalid_log.load(Ordering::SeqCst) {
            return Err(anyhow!("malformed host page"));
        }
        if expected.is_some_and(|expected| *expected != ident()) {
            return Ok(LogPage::Reset {
                info: Some(orx_host_proto::LogInfo {
                    ident: ident(),
                    len: 10,
                }),
            });
        }
        let bytes = b"0123456789";
        let end = range.end.min(bytes.len() as u64);
        Ok(LogPage::Data {
            len: Some(10),
            ident: ident(),
            start: range.start,
            bytes: bytes[range.start as usize..end as usize].to_vec(),
            eof: end == bytes.len() as u64,
        })
    }
}
struct Services(Arc<Fake>, Arc<Engine>, PathBuf);
#[async_trait]
impl Commands for Services {
    async fn run(&self, args: crate::ExpRunArgs, admission: Box<dyn Send>) -> Result<String> {
        assert_eq!(self.0.admitted.load(Ordering::SeqCst), 1);
        self.1.insert(SubmittingRun {
            id: "run",
            experiment_id: &args.exp_id,
            project_id: "project",
            metadata: &crate::jobs::BackendDescriptor::parse(r#"{"kind":"local"}"#)?,
            command: "train",
            created_at: 1,
            commit_sha: None,
            chat_session_id: None,
            spec_digest: "digest",
            launch: &LaunchSpec {
                kind: "local_job".into(),
                target: None,
                options: json!({}),
            },
        })?;
        drop(admission);
        self.1.launch_inserted(self.0.as_ref(), "run").await?;
        Ok("run".into())
    }
    fn backend(&self, _: &StoredRun) -> Result<Arc<dyn Backend>> {
        Ok(self.0.clone())
    }
    async fn cancel(&self, run: &StoredRun, reason: &str) -> Result<()> {
        self.1.cancel(self.0.as_ref(), &run.id, reason).await?;
        Ok(())
    }
    fn host_target(&self, _: &str, _: bool) -> Result<crate::jobs::ssh::SshTarget> {
        Err(anyhow!("No hosts in this fixture"))
    }
    async fn cleanup(&self, _: &WatchedRun) -> Result<Value> {
        Ok(json!({"accepted":true}))
    }
    async fn compute(
        &self,
        action: &str,
        _: Value,
        prompt: Option<crate::engine::transport::Prompt>,
    ) -> Result<Value> {
        if let Some(prompt) = prompt {
            assert_eq!(prompt("MFA code:".into()).await?, Some("123456".into()));
        }
        Ok(json!({"action":action, "ready":true}))
    }
    fn host_states(&self, alias: &str) -> Vec<(String, crate::engine::transport::HostState)> {
        Store::open_at(self.2.clone())
            .unwrap()
            .engine_hosts()
            .unwrap()
            .into_iter()
            .filter(|(host, _, _)| host == alias)
            .map(|(_, root, state)| (root, state))
            .collect()
    }
    async fn retry_host(&self, alias: &str) {
        assert_eq!(alias, "lab");
    }
    fn follow_log(&self, _: &str) -> Result<Box<dyn Send>> {
        struct Guard(Arc<AtomicUsize>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.0.follows.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(Guard(self.0.follows.clone())))
    }
}
fn ident() -> FileIdent {
    FileIdent { dev: 1, ino: 2 }
}

/// Keeps `bytes` at `start` of the run's log, beside what is kept already.
fn keep(api: &Api, run: &str, start: u64, bytes: &[u8], eof: bool) {
    let file = api.data_dir.join(format!("run-logs/{run}.{start}.page"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, bytes).unwrap();
    let mut ranges = kept_ranges(&api.data_dir, run).unwrap();
    ranges.push(KeptRange {
        offset: start,
        len: bytes.len() as u64,
        file,
        ident: ident(),
        eof,
    });
    keep_ranges(&api.data_dir, run, ranges).unwrap();
}

async fn completion(events: &mut (impl Stream<Item = Result<Value>> + Unpin)) -> Value {
    while let Some(value) = events.next().await {
        let value = value.unwrap();
        if value["drained"] == true
            || Phase::parse(value["status"].as_str().unwrap_or_default())
                .is_some_and(Phase::is_terminal)
        {
            return value;
        }
    }
    panic!("event stream ended before completion")
}

struct Fixture {
    api: Arc<Api>,
    backend: Arc<Fake>,
    refuse_admission: Arc<AtomicBool>,
    client: ApiClient,
    descriptor: ServerDescriptor,
    requests: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let data_dir = std::env::temp_dir().join(format!("orx-api-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(data_dir.clone()).unwrap();
        store
            .create_local_experiment(&crate::local::model::LocalExperiment {
                id: "exp".into(),
                project_id: "project".into(),
                parent_experiment_id: None,
                slug: "exp".into(),
                branch_name: "orx/exp".into(),
                title: None,
                description: None,
                run_command: "train".into(),
                agent_status: "idle".into(),
                created_at: 1,
                updated_at: 1,
                chat_session_id: None,
                archived: false,
            })
            .unwrap();
        let backend = Arc::new(Fake::default());
        let admitted = backend.admitted.clone();
        let refuse_admission = Arc::new(AtomicBool::new(false));
        let admit: Admit = {
            let (admitted, refuse) = (admitted.clone(), refuse_admission.clone());
            Box::new(move |project| {
                assert_eq!(project, "project");
                if refuse.load(Ordering::SeqCst) {
                    return Err(anyhow!("A data-directory move is in progress."));
                }
                struct Held(Arc<AtomicUsize>);
                impl Drop for Held {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                admitted.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Held(admitted.clone())))
            })
        };
        let engine = Arc::new(Engine::new(
            store,
            Arc::new(crate::engine::clock::SystemClock::new()),
        ));
        let api = Arc::new(
            Api::new(
                "instance".into(),
                "secret".into(),
                engine.clone(),
                Arc::new(Services(backend.clone(), engine, data_dir.clone())),
                admit,
                data_dir,
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let descriptor = ServerDescriptor {
            instance: api.instance.clone(),
            token: api.token.clone(),
            pid: std::process::id(),
            starttime: crate::engine::process::identity(std::process::id())
                .unwrap()
                .starttime,
            port: listener.local_addr().unwrap().port(),
            version: env!("CARGO_PKG_VERSION").into(),
            proto: 2,
        };
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let app = routes(api.clone())
            .merge(dashboard_routes(api.clone()))
            .layer(middleware::from_fn(move |request: Request, next: Next| {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    next.run(request).await
                }
            }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = ApiClient::new(descriptor.clone()).unwrap();
        Self {
            api,
            backend,
            refuse_admission,
            client,
            descriptor,
            requests,
            task,
        }
    }
    fn insert(&self, id: &str, experiment: &str, project: &str, status: &str) {
        self.api
            .store()
            .upsert_run(&StoredRun {
                id: id.into(),
                experiment_id: experiment.into(),
                project_id: project.into(),
                status: status.into(),
                backend_json:
                    r#"{"kind":"engine","jobId":"/fake","handle":{"type":"Local","dir":"/fake"}}"#
                        .into(),
                command: "train".into(),
                created_at: 1,
                updated_at: 1,
                ended_at: None,
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: None,
            })
            .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.api.data_dir);
    }
}

#[tokio::test]
async fn authenticated_routes_expose_contact_and_commands() {
    let f = Fixture::new().await;
    let response = crate::net::loopback_client()
        .build()
        .unwrap()
        .get(format!(
            "http://127.0.0.1:{}/api/v2/runs",
            f.descriptor.port
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        f.client
            .json(reqwest::Method::GET, "hello?nonce=test", None)
            .await
            .unwrap()["instance"],
        "instance"
    );
    f.insert("run", "exp", "project", "starting");
    f.api
        .engine
        .observe(
            "run",
            Observation::Running {
                started_at: Some(1234),
            },
        )
        .unwrap();
    let list = f
        .client
        .json(reqwest::Method::GET, "runs?project=project", None)
        .await
        .unwrap();
    let _: crate::plane::RunListing = serde_json::from_value(list.clone()).unwrap();
    let run_view: crate::plane::Run = serde_json::from_value(list["runs"][0].clone()).unwrap();
    assert_eq!(run_view.id, "run");
    assert_eq!(run_view.status, "running");
    assert_eq!(list["runs"][0]["contact"]["type"], "live");
    assert_eq!(list["runs"][0]["startedAt"], 1234);
    let cancel = f
        .client
        .json(reqwest::Method::POST, "runs/run/cancel", None)
        .await
        .unwrap();
    assert_eq!(cancel["cancelling"], true);
    assert!(f
        .client
        .json(reqwest::Method::POST, "runs/run/cleanup", None)
        .await
        .is_err());
    f.api
        .store()
        .set_run_contact(
            "run",
            &Contact::Unreachable {
                since: 3,
                cause: "offline".into(),
            },
        )
        .unwrap();
    let abandoned = f
        .client
        .json(reqwest::Method::POST, "runs/run/abandon", None)
        .await
        .unwrap();
    assert_eq!(abandoned["status"], "lost");
    assert_eq!(abandoned["contact"]["type"], "unreconciled");
    assert_eq!(
        f.client
            .json(reqwest::Method::POST, "runs/run/cleanup", None)
            .await
            .unwrap()["accepted"],
        true
    );
    for action in ["test", "connect"] {
        assert_eq!(
            f.client
                .json(
                    reqwest::Method::POST,
                    &format!("compute/{action}"),
                    Some(&json!({}))
                )
                .await
                .unwrap()["ready"],
            true
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let uninitialized = Router::new()
        .route("/api/runs/{id}", get(|| async { "legacy" }))
        .merge(router());
    let server = tokio::spawn(async move {
        axum::serve(listener, uninitialized).await.unwrap();
    });
    let response = crate::net::loopback_client()
        .build()
        .unwrap()
        .get(format!("http://{address}/api/v2/runs"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    install(f.api.clone()).unwrap();
    let response = crate::net::loopback_client()
        .build()
        .unwrap()
        .get(format!("http://{address}/api/v2/runs/run"))
        .bearer_auth("secret")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], "run");
    let response = crate::net::loopback_client()
        .build()
        .unwrap()
        .get(format!("http://{address}/api/runs/run/log?start=0&end=2"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["data"], "MDE=");
    server.abort();
    assert!(install(f.api.clone()).is_err());
}

#[tokio::test]
async fn wait_is_one_sse_request_completed_by_first_matching_bus_event() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    f.insert("other", "other", "other", "running");
    let filter = Filter {
        experiment: Some("exp".into()),
        wait: true,
        ..Default::default()
    };
    let mut events = f.client.events(&filter).await.unwrap();
    assert_eq!(f.api.sse_clients(), 1);
    f.api
        .engine
        .observe(
            "other",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    f.api
        .engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    f.api
        .engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(1), completion(&mut events))
        .await
        .unwrap();
    assert_eq!(event["id"], "run");
    assert_eq!(event["status"], "done");
    assert!(events.next().await.is_none());
    assert_eq!(f.requests.load(Ordering::SeqCst), 1);
    f.client
        .wait(Filter {
            run: Some("run".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 0);
    assert_eq!(f.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn project_wait_returns_drained_and_filters_other_projects() {
    let f = Fixture::new().await;
    f.insert("old", "exp", "project", "done");
    let mut events = f
        .client
        .events(&Filter {
            project: Some("project".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(completion(&mut events).await["drained"], true);
    f.insert("new", "exp", "project", "running");
    let mut events = f
        .client
        .events(&Filter {
            project: Some("project".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    f.api
        .engine
        .observe(
            "new",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    assert_eq!(completion(&mut events).await["id"], "new");
}

#[tokio::test]
async fn log_pages_use_kept_offsets_remote_ranges_and_503_on_unreachable() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    keep(&f.api, "run", 6, b"6789", true);
    let local = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=7&end=10", None)
        .await
        .unwrap();
    assert_eq!(local["data"], json!("Nzg5"));
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 0);
    let remote = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=2&end=5", None)
        .await
        .unwrap();
    assert_eq!(remote["data"], json!("MjM0"));
    f.api
        .store()
        .set_engine_phase("run", "running", "done", Some(1), Some(0), None)
        .unwrap();
    f.backend.offline.store(true, Ordering::SeqCst);
    *f.backend.outage.lock().unwrap() = Some(crate::engine::transport::HostState {
        status: crate::engine::transport::Status::Backoff,
        since_ms: 123,
        ..Default::default()
    });
    f.api
        .store()
        .set_run_contact(
            "run",
            &Contact::Unreachable {
                since: 123,
                cause: "offline".into(),
            },
        )
        .unwrap();
    let response = f
        .client
        .request(reqwest::Method::GET, "runs/run/log?start=0&end=2")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"error":"Page not on this computer", "host":null, "since":123, "contact":"unreachable"})
    );
    assert!(f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=5&end=2", None)
        .await
        .is_err());
}

/// Pending final collection does not turn an earlier kept prefix into the final log.
#[tokio::test]
async fn finished_runs_read_from_their_host_until_the_log_is_kept() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    keep(&f.api, "run", 0, b"0123", true);
    {
        let store = f.api.store();
        store
            .set_engine_phase("run", "running", "done", Some(1), Some(0), None)
            .unwrap();
        store.queue_log_collection("run").unwrap();
        assert!(!store
            .defer_log_collection("run", "host in backoff")
            .unwrap());
    }
    let run = f
        .client
        .json(reqwest::Method::GET, "runs/run", None)
        .await
        .unwrap();
    assert_eq!(run["logCollection"], "pending");
    assert_eq!(run["logCollectionError"], "host in backoff");
    let page = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=0&end=100", None)
        .await
        .unwrap();
    assert_eq!(page["data"], json!("MDEyMw=="));
    assert_eq!(page["eof"], false);
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 0);
    let rest = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=4&end=100", None)
        .await
        .unwrap();
    assert_eq!(rest["data"], json!("NDU2Nzg5"));
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 1);
    f.backend.offline.store(true, Ordering::SeqCst);
    assert!(f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=4&end=100", None)
        .await
        .is_err());

    f.api.store().finish_log_collection("run").unwrap();
    let run = f
        .client
        .json(reqwest::Method::GET, "runs/run", None)
        .await
        .unwrap();
    assert!(run.get("logCollection").is_none());
    let kept = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=0&end=100", None)
        .await
        .unwrap();
    assert_eq!(kept["data"], json!("MDEyMw=="));
    assert_eq!(kept["eof"], true);
}

#[tokio::test]
async fn idle_requires_ten_uninterrupted_minutes_and_all_blockers_clear() {
    let _serial = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    let mut idle = Idle::default();
    assert!(!idle.check(&f.api, Duration::ZERO).unwrap());
    assert!(idle.check(&f.api, Duration::from_secs(600)).unwrap());
    f.insert("run", "exp", "project", "running");
    assert_eq!(f.api.store().pending_work().unwrap().runs.len(), 1);
    assert!(!idle.check(&f.api, Duration::from_secs(601)).unwrap());
    f.api
        .store()
        .set_engine_phase("run", "running", "lost", Some(1), None, None)
        .unwrap();
    f.api
        .store()
        .set_run_contact(
            "run",
            &Contact::Unreconciled {
                since: 1,
                detail: "kill pending".into(),
            },
        )
        .unwrap();
    assert_eq!(f.api.store().pending_work().unwrap().runs.len(), 0);
    assert!(!idle.check(&f.api, Duration::from_secs(1201)).unwrap());
    f.api.store().queue_log_collection("run").unwrap();
    assert!(idle.check(&f.api, Duration::from_secs(1801)).unwrap());
    f.api
        .store()
        .set_run_contact("run", &Contact::Live)
        .unwrap();
    let connection = rusqlite::Connection::open(f.api.data_dir.join("orx.db")).unwrap();
    connection.execute("INSERT INTO run_notices(run_id, chat_session_id, kind, dedupe, body, created_at) VALUES ('run','chat','attention','one','body',1)", []).unwrap();
    assert_eq!(f.api.store().pending_work().unwrap().runs.len(), 1);
    assert!(!idle.check(&f.api, Duration::from_secs(1801)).unwrap());
    connection
        .execute("UPDATE run_notices SET delivered_at=1", [])
        .unwrap();
    connection.execute("INSERT INTO chat_sessions(id,project_id,harness,created_at,updated_at) VALUES ('chat','project','codex',1,1)", []).unwrap();
    connection.execute("INSERT INTO chat_turns(id,session_id,assistant_message_id,client_turn_id,request_hash,prepared_input,settings_json,state,delivery_state,created_at,updated_at) VALUES ('turn','chat','assistant','client','hash','input','{}','running','pending',1,1)", []).unwrap();
    let pending = f.api.store().pending_work().unwrap();
    assert_eq!((pending.agents.len(), pending.runs.len()), (1, 0));
    assert!(!idle.check(&f.api, Duration::from_secs(2401)).unwrap());
    connection.execute("DELETE FROM chat_turns", []).unwrap();
    let events = f
        .client
        .events(&Filter {
            run: Some("run".into()),
            follow: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(f.backend.follows.load(Ordering::SeqCst), 1);
    assert!(f.api.store().pending_work().unwrap().is_empty());
    assert!(!idle.check(&f.api, Duration::from_secs(3001)).unwrap());
    drop(events);
    f.task.abort();
    tokio::task::yield_now().await;
    assert!(!idle.should_exit(Duration::from_secs(3002), false));
    assert!(idle.should_exit(Duration::from_secs(3602), false));
}

#[tokio::test]
async fn a_force_quit_stops_runs_that_need_the_app_even_those_launched_while_a_backend_stays_silent(
) {
    let _serial = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    let needs_app = |run: &str| {
        let connection = rusqlite::Connection::open(f.api.data_dir.join("orx.db")).unwrap();
        connection
            .execute(
                r#"UPDATE runs SET backend_json='{"kind":"engine","handle":{"type":"Legacy","descriptor":{"kind":"local_job"}}}' WHERE id=?1"#,
                [run],
            )
            .unwrap();
    };
    f.insert("local", "exp", "project", "running");
    needs_app("local");
    f.insert("remote", "exp", "project", "running");
    let cancelling = |run: &str| {
        f.api
            .store()
            .engine_run_state(run)
            .unwrap()
            .is_some_and(|state| state.cancel_accepted_at.is_some())
    };

    f.backend.cancel_hangs.store(true, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let launch_late = async {
        f.backend.cancel_entered.notified().await;
        f.insert("late", "exp", "project", "running");
        needs_app("late");
    };
    tokio::join!(f.api.force_stop_runs(Duration::from_secs(2)), launch_late);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(cancelling("local"));
    assert!(cancelling("late"));
    assert!(!cancelling("remote"));
}

#[tokio::test]
async fn a_force_quit_waits_for_a_launching_run_to_answer_its_stop() {
    use clap::Parser;
    let _serial = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *f.backend.launch_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let cli =
        crate::Cli::try_parse_from(["orx", "exp", "run", "exp", "--backend", "local"]).unwrap();
    let Some(crate::Command::Exp(crate::ExpArgs {
        command: crate::ExpCommand::Run(args),
    })) = cli.command
    else {
        panic!("run command")
    };
    let api = f.api.clone();
    let launch = tokio::spawn(async move { api.launch(*args).await });
    entered.notified().await;

    let (api, backend) = (f.api.clone(), f.backend.clone());
    let wait = Duration::from_secs(1);
    let stopping = tokio::spawn(async move {
        let started = std::time::Instant::now();
        api.force_stop_runs(wait).await;
        (started.elapsed(), backend.cancels.load(Ordering::SeqCst))
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f
            .api
            .store()
            .engine_run_state("run")
            .unwrap()
            .is_none_or(|state| state.cancel_accepted_at.is_none())
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    f.backend.cancel_hangs.store(true, Ordering::SeqCst);
    release.notify_one();
    let (elapsed, cancels) = stopping.await.unwrap();
    assert!(cancels > 0);
    assert!(
        elapsed >= wait,
        "returned after {elapsed:?} with the stop unanswered"
    );
    launch.abort();
}

#[tokio::test]
async fn cli_launch_and_cancel_use_the_server_and_kept_pages_span_file_boundaries() {
    use clap::Parser;
    let f = Fixture::new().await;
    let cli =
        crate::Cli::try_parse_from(["orx", "exp", "run", "exp", "--backend", "local"]).unwrap();
    let Some(crate::Command::Exp(crate::ExpArgs {
        command: crate::ExpCommand::Run(args),
    })) = cli.command
    else {
        panic!("run command")
    };
    f.client.launch(*args).await.unwrap();
    assert_eq!(f.api.run("run").unwrap().status, "starting");
    f.client.cancel(Some("exp"), None).await.unwrap();
    assert!(f
        .api
        .store()
        .engine_run_state("run")
        .unwrap()
        .unwrap()
        .cancel_accepted_at
        .is_some());
    keep(&f.api, "run", 0, b"01234", false);
    keep(&f.api, "run", 5, b"56789", true);
    let value = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=3&end=8", None)
        .await
        .unwrap();
    assert_eq!(value["data"], json!("MzQ1Njc="));
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 0);
    f.client.logs("run", false).await.unwrap();
    f.api
        .engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    // What the runtime records once the finished run's log is kept.
    f.api.store().finish_log_collection("run").unwrap();
    f.backend.offline.store(true, Ordering::SeqCst);
    f.client.logs("run", true).await.unwrap();
}

#[tokio::test]
async fn project_wait_resync_excludes_already_completed_runs() {
    let f = Fixture::new().await;
    f.insert("old", "exp", "project", "done");
    f.insert("run", "exp", "project", "running");
    let mut events = f
        .client
        .events(&Filter {
            project: Some("project".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    for _ in 0..2048 {
        f.api
            .engine
            .bus
            .publish(RunEvent::Notices { chats: vec![] });
    }
    f.api
        .engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), completion(&mut events))
            .await
            .unwrap()["id"],
        "run"
    );
}

#[tokio::test]
async fn log_feed_is_refcounted_and_pushes_without_a_cli_poll() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    let mut events = f
        .client
        .events(&Filter {
            run: Some("run".into()),
            follow: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(events.next().await.unwrap().unwrap()["id"], "run");
    f.api.engine.bus.publish(RunEvent::Grew {
        file: LogFile::Log,
        run_id: "run".into(),
        len: 10,
    });
    let event = tokio::time::timeout(Duration::from_secs(1), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(event.get("event").is_none());
    assert_eq!(event["file"], "log");
    assert_eq!(event["len"], 10);
    assert_eq!(f.backend.follows.load(Ordering::SeqCst), 1);
    drop(events);
    tokio::time::timeout(Duration::from_secs(1), async {
        while f.api.sse_clients() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.backend.follows.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn v2_launches_are_host_checked_admitted_and_keep_a_run_submission_active() {
    use clap::Parser;
    let _guard = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *f.backend.launch_gate.lock().unwrap() = Some((entered.clone(), release.clone()));
    let app = crate::commands::up::server_router(
        routes(f.api.clone()),
        Some(crate::commands::remote_host::RemoteAuth::new(
            "legacy-token",
        )),
        crate::commands::up_remote::Reach::Loopback,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = crate::net::loopback_client().build().unwrap();
    let response = client
        .get(format!("http://{address}/api/v2/hello?nonce=test"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let cli =
        crate::Cli::try_parse_from(["orx", "exp", "run", "exp", "--backend", "local"]).unwrap();
    let Some(crate::Command::Exp(crate::ExpArgs {
        command: crate::ExpCommand::Run(args),
    })) = cli.command
    else {
        panic!("run command")
    };
    let body = serde_json::to_value(&args).unwrap();
    let post = move || {
        client
            .post(format!("http://{address}/api/v2/runs"))
            .bearer_auth("secret")
            .json(&body)
            .send()
    };
    f.refuse_admission.store(true, Ordering::SeqCst);
    let refused = post().await.unwrap();
    assert!(!refused.status().is_success());
    assert!(refused
        .text()
        .await
        .unwrap()
        .contains("move is in progress"));
    assert!(f.api.store().get_run("run").unwrap().is_none());
    f.refuse_admission.store(false, Ordering::SeqCst);
    let request = tokio::spawn(async move { post().await.unwrap().text().await.unwrap() });
    entered.notified().await;
    assert_eq!(crate::commands::up::ACTIVE.load(Ordering::SeqCst), 1);
    // Admitted through preparation (checked at insert), released once the row is durable.
    assert_eq!(f.backend.admitted.load(Ordering::SeqCst), 0);
    assert!(f.api.store().get_run("run").unwrap().is_some());
    release.notify_one();
    request.await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while crate::commands::up::ACTIVE.load(Ordering::SeqCst) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    server.abort();
}

#[tokio::test]
async fn terminal_nonlive_contacts_do_not_block_idle_exit() {
    let _serial = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "lost");
    for contact in [
        Contact::Unreachable {
            since: 1,
            cause: "offline".into(),
        },
        Contact::NeedsAuth {
            since: 1,
            cause: "login".into(),
        },
        Contact::Unreconciled {
            since: 1,
            detail: "unknown".into(),
        },
    ] {
        let mut idle = Idle::default();
        f.api.store().set_run_contact("run", &contact).unwrap();
        assert!(!idle.check(&f.api, Duration::ZERO).unwrap());
        assert!(idle.check(&f.api, Duration::from_secs(601)).unwrap());
    }
    f.api
        .store()
        .set_run_contact("run", &Contact::Live)
        .unwrap();
    let mut idle = Idle::default();
    assert!(!idle.check(&f.api, Duration::ZERO).unwrap());
    assert!(idle.check(&f.api, Duration::from_secs(600)).unwrap());
}

#[tokio::test]
async fn experiment_wait_excludes_older_completed_runs_after_subscribing() {
    let f = Fixture::new().await;
    f.insert("old", "exp", "project", "done");
    f.insert("run", "exp", "project", "running");
    let connection = rusqlite::Connection::open(f.api.data_dir.join("orx.db")).unwrap();
    connection
        .execute("UPDATE runs SET created_at=2 WHERE id='run'", [])
        .unwrap();
    let mut events = f
        .client
        .events(&Filter {
            experiment: Some("exp".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    f.api.engine.bus.publish(RunEvent::Changed {
        run_id: "old".into(),
        phase: Phase::Done,
        contact: Contact::Live,
        cancelling: false,
    });
    f.api
        .engine
        .observe(
            "run",
            Observation::Exited {
                finished_at: None,
                code: Some(0),
                signal: None,
            },
        )
        .unwrap();
    assert_eq!(completion(&mut events).await["id"], "run");
}

#[tokio::test]
async fn experiment_wait_follows_new_launches_and_excludes_older_active_runs() {
    let f = Fixture::new().await;
    f.insert("old", "exp", "project", "running");
    f.insert("run", "exp", "project", "running");
    let connection = rusqlite::Connection::open(f.api.data_dir.join("orx.db")).unwrap();
    connection
        .execute("UPDATE runs SET created_at=2 WHERE id='run'", [])
        .unwrap();
    let mut events = f
        .client
        .events(&Filter {
            experiment: Some("exp".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    for id in ["old", "run", "new"] {
        if id == "new" {
            f.insert("new", "exp", "project", "running");
            connection
                .execute("UPDATE runs SET created_at=3 WHERE id='new'", [])
                .unwrap();
        }
        f.api
            .engine
            .observe(
                id,
                Observation::Exited {
                    finished_at: None,
                    code: Some(0),
                    signal: None,
                },
            )
            .unwrap();
    }
    assert_eq!(completion(&mut events).await["id"], "new");
}

#[tokio::test]
async fn log_replacement_is_marked_and_identity_reaches_the_backend() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    let mut next = ident();
    next.ino += 1;
    f.backend.scripted_pages.lock().unwrap().extend([
        LogPage::Data {
            len: None,
            ident: ident(),
            start: 0,
            bytes: b"old".to_vec(),
            eof: false,
        },
        LogPage::Reset {
            info: Some(orx_host_proto::LogInfo {
                ident: next.clone(),
                len: 3,
            }),
        },
        LogPage::Data {
            len: None,
            ident: next.clone(),
            start: 0,
            bytes: b"new".to_vec(),
            eof: true,
        },
    ]);
    let mut output = Vec::new();
    f.client.logs_to("run", false, &mut output).await.unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        "old\n[log replaced; starting new generation]\nnew"
    );
    assert_eq!(
        *f.backend.identities.lock().unwrap(),
        vec![None, Some(ident()), Some(next)]
    );
}

#[tokio::test]
async fn malformed_log_pages_are_not_reported_as_transport_outages() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    f.backend.invalid_log.store(true, Ordering::SeqCst);
    let response = f
        .client
        .request(reqwest::Method::GET, "runs/run/log?start=0&end=5")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response.text().await.unwrap(), "malformed host page");
}

#[tokio::test]
async fn compute_connect_relays_interactive_prompts_to_the_client() {
    let f = Fixture::new().await;
    let prompted = Arc::new(AtomicUsize::new(0));
    let count = prompted.clone();
    let prompt: crate::engine::transport::Prompt = Arc::new(move |question| {
        assert_eq!(question, "MFA code:");
        count.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(Some("123456".into())) })
    });
    assert_eq!(
        f.client
            .connect(json!({"backend":"ssh","host":"test"}), prompt)
            .await
            .unwrap()["ready"],
        true
    );
    assert_eq!(prompted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn reconnect_child() {
    let Ok(mode) = std::env::var("ORX_TEST_STREAM_RECONNECT") else {
        return;
    };
    let client = ApiClient::discover().await.unwrap();
    if mode == "post" {
        let port = std::env::var("ORX_TEST_STALE_PORT").unwrap_or_else(|_| "0".into());
        let stale = ApiClient::new(
            ServerDescriptor::agent_session(Some(port), Some("old-token".into()))
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let result = stale
            .json(
                reqwest::Method::POST,
                "echo",
                Some(&json!({"command":"one launch"})),
            )
            .await
            .unwrap();
        assert_eq!(result, json!({"command":"one launch"}));
        if std::env::var_os("ORX_TEST_STALE_PORT").is_some() {
            let events = stale.events(&Filter::default()).await.unwrap();
            futures::pin_mut!(events);
            assert!(events.next().await.is_none());
        }
        return;
    }
    if mode == "wait" || mode == "projectwait" {
        client
            .wait(Filter {
                run: (mode == "wait").then(|| "run".into()),
                project: (mode == "projectwait").then(|| "project".into()),
                wait: true,
                ..Default::default()
            })
            .await
            .unwrap();
    } else {
        let mut output = Vec::new();
        client.logs_to("run", true, &mut output).await.unwrap();
        assert_eq!(output, b"0123456789");
    }
}

#[tokio::test]
async fn wait_and_follow_rediscover_and_rebuild_snapshots_when_the_sse_connection_closes() {
    for mode in ["wait", "projectwait", "logs"] {
        let f = Fixture::new().await;
        f.insert("run", "exp", "project", "running");
        if mode == "projectwait" {
            f.insert("other", "other", "project", "running");
        }
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let api = f.api.clone();
        let app = Router::new()
            .route(
                "/api/v2/events",
                get(move |Query(filter): Query<Filter>| {
                    let count = count.clone();
                    let api = api.clone();
                    async move {
                        let number = count.fetch_add(1, Ordering::SeqCst);
                        if number != 0 {
                            api.engine
                                .observe(
                                    "run",
                                    Observation::Exited {
                                        finished_at: None,
                                        code: Some(0),
                                        signal: None,
                                    },
                                )
                                .unwrap();
                        }
                        let bytes = if filter.project.is_some() {
                            let rows = api.store().list_runs_by_project("project").unwrap();
                            let values: Vec<_> =
                                rows.iter().map(|run| api.view(run).unwrap()).collect();
                            format!("event: snapshot\ndata: {}\n\n", json!({"snapshot":values}))
                        } else if number == 0 && filter.wait {
                            let run = api.view(&api.run("run").unwrap()).unwrap();
                            format!("event: snapshot\ndata: {}\n\n", json!({"snapshot":[run]}))
                        } else {
                            format!(
                                "event: run\ndata: {}\n\n",
                                api.view(&api.run("run").unwrap()).unwrap()
                            )
                        };
                        ([("content-type", "text/event-stream")], bytes)
                    }
                }),
            )
            .route("/api/v2/runs/{id}/log", get(log))
            .with_state(f.api.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let output = tokio::time::timeout(
            Duration::from_secs(5),
            crate::process::tokio_command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "commands::up::api_v2::tests::reconnect_child",
                    "--nocapture",
                ])
                .env("ORX_TEST_STREAM_RECONNECT", mode)
                .env("ORX_UP_PORT", port.to_string())
                .env("ORX_UP_AUTH_TOKEN", "secret")
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if mode == "wait" || mode == "projectwait" {
            assert!(String::from_utf8_lossy(&output.stdout).contains("run done"));
        }
        if mode == "wait" {
            let progress = String::from_utf8_lossy(&output.stderr);
            assert!(progress.contains("run  running"), "{progress}");
            assert!(!progress.contains("run  done"), "{progress}");
        }
        if mode == "projectwait" {
            assert!(String::from_utf8_lossy(&output.stderr)
                .contains("Watching 2 run(s) in project (2 in flight)"));
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.abort();
    }
}

#[tokio::test]
async fn stale_client_reset_preserves_kept_ranges_from_the_current_generation() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    keep(&f.api, "run", 6, b"6789", true);
    let mut stale = ident();
    stale.ino += 1;
    let response = f
        .client
        .request(reqwest::Method::GET, "runs/run/log")
        .query(&[
            ("start", "0"),
            ("end", "5"),
            ("ident", &String::from(stale)),
        ])
        .send()
        .await
        .unwrap();
    assert!(
        matches!(response.json::<LogPage>().await.unwrap(), LogPage::Reset { info: Some(info) } if info.ident == ident())
    );
    f.backend.offline.store(true, Ordering::SeqCst);
    let page = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=6&end=10", None)
        .await
        .unwrap();
    assert_eq!(page["data"], json!("Njc4OQ=="));
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 1);
}

/// A finished run over 1 GiB whose host directory was cleaned: a sequential read past the
/// kept head meets a reset without identity, and the kept head and tail still answer.
#[tokio::test]
async fn a_reset_past_the_kept_head_leaves_kept_ranges_alone() {
    const MIB: u64 = 1 << 20;
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "done");
    let len = 1100 * MIB;
    let logs = f.api.data_dir.join("run-logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mut ranges = Vec::new();
    for (offset, size, name, eof) in [
        (0, 64 * MIB, "log", false),
        (len - 256 * MIB, 256 * MIB, "tail.log", true),
    ] {
        let file = logs.join(format!("run.{name}"));
        std::fs::File::create(&file).unwrap().set_len(size).unwrap();
        ranges.push(KeptRange {
            offset,
            len: size,
            file,
            ident: ident(),
            eof,
        });
    }
    keep_ranges(&f.api.data_dir, "run", ranges).unwrap();
    let manifest = std::fs::read(logs.join("run.ranges.json")).unwrap();
    let page = |start: u64| {
        let end = start + MIB;
        format!("runs/run/log?start={start}&end={end}&ident=1-2")
    };

    let last_head = f
        .client
        .json(reqwest::Method::GET, &page(63 * MIB), None)
        .await
        .unwrap();
    assert_eq!(last_head["kind"], "data");
    f.backend
        .scripted_pages
        .lock()
        .unwrap()
        .push_back(LogPage::Reset { info: None });
    let past_head = f
        .client
        .json(reqwest::Method::GET, &page(64 * MIB), None)
        .await
        .unwrap();
    assert_eq!(past_head, json!({"kind":"reset","info":null}));
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 1);

    assert_eq!(
        std::fs::read(logs.join("run.ranges.json")).unwrap(),
        manifest
    );
    f.backend.offline.store(true, Ordering::SeqCst);
    for start in [0, len - MIB] {
        let kept = f
            .client
            .json(reqwest::Method::GET, &page(start), None)
            .await
            .unwrap();
        assert_eq!(kept["kind"], "data", "{start}");
    }
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 1);
}

/// After a reconnect the host's log has another identity: the reset neither drops nor joins
/// what is kept.
#[tokio::test]
async fn a_new_identity_after_reconnect_leaves_kept_ranges_alone() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "done");
    keep(&f.api, "run", 0, b"0123", false);
    let manifest = std::fs::read(f.api.data_dir.join("run-logs/run.ranges.json")).unwrap();
    let replaced = FileIdent { dev: 1, ino: 3 };
    f.backend
        .scripted_pages
        .lock()
        .unwrap()
        .push_back(LogPage::Reset {
            info: Some(orx_host_proto::LogInfo {
                ident: replaced.clone(),
                len: 10,
            }),
        });
    let reset = f
        .client
        .json(
            reqwest::Method::GET,
            "runs/run/log?start=4&end=10&ident=1-2",
            None,
        )
        .await
        .unwrap();
    assert_eq!(reset["info"]["ident"], String::from(replaced));
    assert_eq!(
        std::fs::read(f.api.data_dir.join("run-logs/run.ranges.json")).unwrap(),
        manifest
    );

    f.backend.offline.store(true, Ordering::SeqCst);
    let kept = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=0&end=10", None)
        .await;
    let kept = kept.unwrap();
    assert_eq!(kept["data"], json!("MDEyMw=="));
    assert_eq!(kept["eof"], false);
    let kept = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=0&end=4", None)
        .await
        .unwrap();
    assert_eq!(kept["data"], json!("MDEyMw=="));
}

#[tokio::test]
async fn following_logs_retries_transport_503_with_backoff_and_preserves_the_offset() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "done");
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let seen = attempts.clone();
    let app = Router::new()
        .route(
            "/api/v2/events",
            get(|| async {
                (
                    [("content-type", "text/event-stream")],
                    "event: run\ndata: {\"status\":\"done\"}\n\n",
                )
            }),
        )
        .route(
            "/api/v2/runs/{id}/log",
            get(move |Query(query): Query<LogQuery>| {
                let seen = seen.clone();
                async move {
                    let count = {
                        let mut seen = seen.lock().unwrap();
                        seen.push((std::time::Instant::now(), query.start));
                        seen.len()
                    };
                    if count == 1 || count == 3 {
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "page not on this computer; host unreachable since 1",
                        )
                            .into_response();
                    }
                    let bytes = if count == 2 {
                        b"first".to_vec()
                    } else if count == 4 {
                        b"last".to_vec()
                    } else {
                        vec![]
                    };
                    Json(LogPage::Data {
                        len: None,
                        ident: ident(),
                        start: query.start,
                        bytes,
                        eof: count != 2,
                    })
                    .into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut descriptor = f.descriptor.clone();
    descriptor.port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut output = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        ApiClient::new(descriptor)
            .unwrap()
            .logs_to("run", true, &mut output),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(output, b"firstlast");
    let attempts = attempts.lock().unwrap();
    assert_eq!(
        attempts
            .iter()
            .map(|(_, offset)| *offset)
            .collect::<Vec<_>>(),
        [0, 0, 5, 5, 9]
    );
    assert!(attempts[1].0.duration_since(attempts[0].0) >= Duration::from_millis(200));
    assert!(attempts[3].0.duration_since(attempts[2].0) >= Duration::from_millis(200));
    server.abort();
}

#[tokio::test]
async fn dashboard_log_pages_preserve_kept_ranges_gaps_resets_and_contact_errors() {
    use tower::ServiceExt;
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    let mut run = f.api.run("run").unwrap();
    run.backend_json = r#"{"kind":"engine","jobId":"/root/runs/run","handle":{"type":"Ssh","host":"lab","root":"/root"}}"#.into();
    f.api.store().upsert_run(&run).unwrap();
    keep(&f.api, "run", 6, b"6789", true);
    let app = dashboard_routes(f.api.clone());
    for (query, expected, reads) in [
        (
            "start=7&end=10",
            json!({"kind":"data","ident":String::from(ident()),"start":7,"data":"Nzg5","eof":true,"len":10,"kept":[[6,10]]}),
            0,
        ),
        (
            "start=2&end=5",
            json!({"kind":"data","ident":String::from(ident()),"start":2,"data":"MjM0","eof":false,"len":10,"kept":[[6,10]]}),
            1,
        ),
        (
            "start=5&end=8",
            json!({"kind":"data","ident":String::from(ident()),"start":5,"data":"NTY3","eof":false,"len":10,"kept":[[6,10]]}),
            2,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/runs/run/log?{query}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
        assert_eq!(f.backend.reads.load(Ordering::SeqCst), reads);
    }
    let stale = FileIdent { dev: 0, ino: 0 };
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/runs/run/log?start=0&end=1&ident={}",
                    String::from(stale)
                ))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!({"kind":"reset","info":{"ident":String::from(ident()),"len":10}})
    );
    f.backend.offline.store(true, Ordering::SeqCst);
    for (contact, kind) in [
        (
            Contact::Unreachable {
                since: 123,
                cause: "offline".into(),
            },
            "unreachable",
        ),
        (
            Contact::NeedsAuth {
                since: 123,
                cause: "key rejected".into(),
            },
            "needs_auth",
        ),
    ] {
        f.api
            .store()
            .set_run_contact("run", &Contact::Live)
            .unwrap();
        *f.backend.outage.lock().unwrap() = Some(crate::engine::transport::HostState {
            status: match contact {
                Contact::NeedsAuth { .. } => crate::engine::transport::Status::NeedsAuth,
                _ => crate::engine::transport::Status::Backoff,
            },
            since_ms: 123,
            ..Default::default()
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runs/run/log?start=0&end=1")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({"error":"Page not on this computer","host":"lab","since":123,"contact":kind})
        );
    }
    // Kept bytes remain available during an authentication outage.
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/runs/run/log?start=6&end=10")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn dashboard_abandon_rejects_live_and_terminal_runs_and_keeps_the_kill() {
    use tower::ServiceExt;
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    let app = dashboard_routes(f.api.clone());
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/api/runs/run/abandon")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    f.api
        .store()
        .set_run_contact(
            "run",
            &Contact::Unreachable {
                since: 123,
                cause: "offline".into(),
            },
        )
        .unwrap();
    let response = app.clone().oneshot(request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["status"], "lost");
    assert_eq!(value["lostReason"], "abandoned");
    assert_eq!(value["cancelling"], false);
    assert_eq!(value["contact"]["type"], "unreconciled");
    assert!(f
        .api
        .store()
        .engine_run_state("run")
        .unwrap()
        .unwrap()
        .cancel_accepted_at
        .is_some());
    assert!(f.api.engine.holds("run"));
    assert_eq!(
        app.oneshot(request()).await.unwrap().status(),
        StatusCode::CONFLICT
    );
}

#[tokio::test]
async fn dashboard_host_routes_report_states_and_retry_only_key_hosts() {
    use crate::engine::transport::{HostClass, HostState, Status};
    use tower::ServiceExt;
    let f = Fixture::new().await;
    f.api
        .store()
        .insert_submitting_run(SubmittingRun {
            id: "pending",
            experiment_id: "exp",
            project_id: "project",
            metadata: &crate::jobs::BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap(),
            command: "train",
            created_at: 1,
            commit_sha: None,
            chat_session_id: None,
            spec_digest: "digest",
            launch: &LaunchSpec {
                kind: "ssh_job".into(),
                target: Some("lab".into()),
                options: json!({}),
            },
        })
        .unwrap();
    let app = dashboard_routes(f.api.clone());
    let state = HostState {
        status: Status::Backoff,
        since_ms: 100,
        next_retry_ms: Some(200),
        cause: Some("offline".into()),
        ..HostState::default()
    };
    f.api
        .store()
        .seed_engine_host("lab", "/engine", Some(HostClass::Key), &state)
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/compute/hosts")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap(),
        json!([{"alias":"lab","status":"backoff","class":"key","connectRequired":false,"sinceMs":100,"nextRetryMs":200,"cause":"offline","root":"/engine"}])
    );
    let request = || {
        Request::builder()
            .method("POST")
            .uri("/api/compute/hosts/lab/retry")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    assert_eq!(
        app.clone().oneshot(request()).await.unwrap().status(),
        StatusCode::OK
    );
    for (class, state) in [
        (
            HostClass::Key,
            HostState {
                status: Status::NeedsAuth,
                ..state.clone()
            },
        ),
        (HostClass::Interactive, state),
    ] {
        f.api
            .store()
            .seed_engine_host("lab", "/engine", Some(class), &state)
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request()).await.unwrap().status(),
            StatusCode::CONFLICT
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/compute/hosts/missing/retry")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dashboard_retry_returns_the_worst_root_state() {
    use crate::engine::transport::{HostClass, HostState, Status};
    use tower::ServiceExt;
    let f = Fixture::new().await;
    let app = dashboard_routes(f.api.clone());
    for (lower, worse, expected) in [
        (Status::Idle, Status::Live, "live"),
        (Status::Live, Status::Connecting, "connecting"),
        (Status::Connecting, Status::Backoff, "backoff"),
    ] {
        for (root, status) in [("/a-first", lower), ("/z-worst", worse)] {
            f.api
                .store()
                .seed_engine_host(
                    "lab",
                    root,
                    Some(HostClass::Key),
                    &HostState {
                        status,
                        ..HostState::default()
                    },
                )
                .unwrap();
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/compute/hosts/lab/retry")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let host: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(host["alias"], "lab");
        assert_eq!(host["root"], "/z-worst");
        assert_eq!(host["status"], expected);
    }
}

#[cfg(unix)]
#[test]
fn dashboard_connect_clears_the_engine_breaker_and_resumes_pending_runs() {
    let directory =
        crate::config::test_home().join(format!("orx-dashboard-host-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let mut command = crate::process::command(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "commands::up::api_v2::tests::dashboard_host_lifecycle_child",
            "--ignored",
            "--nocapture",
        ])
        .env("ORX_HOST_BIN_DIR", orx_test_support::host_binaries())
        .env("HOME", &directory)
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .env("XDG_DATA_HOME", directory.join("data"))
        .env("ORX_DATA_DIR", directory.join("data/orx"));
    let output = command.output().unwrap();
    std::fs::remove_dir_all(directory).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "isolated dashboard SSH lifecycle subprocess"]
async fn dashboard_host_lifecycle_child() {
    use crate::engine::transport::Status;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let server = orx_test_support::Sshd::start().unwrap();
    let ssh_dir = crate::config::home_dir().unwrap().join(".ssh");
    std::fs::create_dir_all(&ssh_dir).unwrap();
    std::fs::copy(server.config_path(), ssh_dir.join("config")).unwrap();
    let f = Fixture::new().await;
    f.insert("seed", "exp", "project", "done");
    let mut seed = f.api.run("seed").unwrap();
    seed.backend_json = r#"{"kind":"engine","jobId":"~/.orx/engine/runs/seed","handle":{"type":"Ssh","host":"orx-test","root":"~/.orx/engine"}}"#.into();
    let total = 2 * (1u64 << 30);
    let middle = 128 * (1u64 << 20);
    let large_dir = server.home().join("large-root/runs/large");
    std::fs::create_dir_all(&large_dir).unwrap();
    let mut file = std::fs::File::create(large_dir.join("log")).unwrap();
    file.set_len(total).unwrap();
    use std::io::{Seek, SeekFrom, Write};
    file.seek(SeekFrom::Start(middle)).unwrap();
    file.write_all(b"middle page from host").unwrap();
    f.insert("large", "exp", "project", "done");
    let mut large = f.api.run("large").unwrap();
    large.backend_json = r#"{"kind":"engine","jobId":"~/large-root/runs/large","handle":{"type":"Ssh","host":"orx-test","root":"~/large-root"}}"#.into();
    f.api.store().upsert_run(&large).unwrap();
    let clock = Arc::new(crate::engine::clock::SystemClock::new());
    let before_restart = crate::engine::runtime::Runtime::new(
        f.api.engine.clone(),
        clock,
        f.api.data_dir.clone(),
        Some(server.config_path()),
    );
    let old_backend = before_restart.backend(&large).unwrap();
    let watched = WatchedRun {
        id: "large".into(),
        handle: handle_of(&large).unwrap(),
    };
    let info = match old_backend
        .read_log(
            &watched,
            LogFile::Log,
            ByteRange {
                start: u64::MAX - 1,
                end: u64::MAX,
            },
            None,
        )
        .await
        .unwrap()
    {
        LogPage::Reset { info: Some(info) } => info,
        page => panic!("{page:?}"),
    };
    let directory = f.api.data_dir.join("run-logs");
    std::fs::create_dir_all(&directory).unwrap();
    let mut kept = Vec::new();
    for (offset, len, eof, name) in [
        (0, 64 * (1u64 << 20), false, "large.head"),
        (
            total - 256 * (1u64 << 20),
            256 * (1u64 << 20),
            true,
            "large.tail",
        ),
    ] {
        let file = directory.join(name);
        std::fs::File::create(&file).unwrap().set_len(len).unwrap();
        kept.push(KeptRange {
            offset,
            len,
            eof,
            ident: info.ident.clone(),
            file,
        });
    }
    keep_ranges(&f.api.data_dir, "large", kept).unwrap();
    drop(old_backend);
    drop(before_restart);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let clock = Arc::new(crate::engine::clock::SystemClock::new());
    let runtime = crate::engine::runtime::Runtime::new(
        f.api.engine.clone(),
        clock,
        f.api.data_dir.clone(),
        Some(server.config_path()),
    );
    let api = Arc::new(
        Api::new(
            "dashboard".into(),
            "token".into(),
            f.api.engine.clone(),
            runtime.clone(),
            Box::new(|_| Ok(Box::new(()))),
            f.api.data_dir.clone(),
        )
        .unwrap(),
    );
    install(api.clone()).unwrap();
    let mut receiver = api.engine.bus.subscribe();
    async fn host_event(receiver: &mut broadcast::Receiver<RunEvent>, wanted: Status) {
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                if let RunEvent::Host { alias, root, state } = receiver.recv().await.unwrap() {
                    assert_eq!(alias, "orx-test");
                    if root == "~/.orx/engine" && state.status == wanted {
                        return;
                    }
                }
            }
        })
        .await
        .unwrap();
    }
    let backend = runtime.backend(&seed).unwrap();
    host_event(&mut receiver, Status::Live).await;
    let app = dashboard_routes(api.clone())
        .route("/api/settings/ssh/connect", get(super::super::ssh_connect))
        .route(
            "/api/settings/ssh/preflight",
            post(super::super::compute_settings::ssh_preflight),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = crate::net::loopback_client().build().unwrap();
    let page = client
        .get(format!(
            "http://{address}/api/runs/large/log?start={middle}&end={}",
            middle + 6
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page: Value = page.json().await.unwrap();
    assert_eq!(page["data"], "bWlkZGxl");
    assert_eq!(page["len"], total);
    assert_eq!(
        page["kept"],
        json!([[0, 64 * (1u64 << 20)], [total - 256 * (1u64 << 20), total]])
    );
    assert_eq!(
        api.store()
            .engine_run_state("large")
            .unwrap()
            .unwrap()
            .contact,
        Contact::Live
    );
    let live_dir = server.home().join("large-root/runs/live");
    std::fs::create_dir_all(&live_dir).unwrap();
    let mut live_file = std::fs::File::create(live_dir.join("log")).unwrap();
    live_file.set_len(total).unwrap();
    live_file.write_all(b"live!!").unwrap();
    f.insert("live", "exp", "project", "running");
    let mut live = f.api.run("live").unwrap();
    live.backend_json = r#"{"kind":"engine","jobId":"~/large-root/runs/live","handle":{"type":"Ssh","host":"orx-test","root":"~/large-root"}}"#.into();
    f.api.store().upsert_run(&live).unwrap();
    let page = client
        .get(format!("http://{address}/api/runs/live/log?start=0&end=6"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page: Value = page.json().await.unwrap();
    assert_eq!(page["len"], total);
    assert_eq!(page["kept"], json!([]));
    assert_eq!(page["eof"], false);
    api.store()
        .set_engine_phase("live", "running", "done", Some(1), Some(0), None)
        .unwrap();
    let attempts = server.auth_attempts().unwrap();
    let tested = client
        .post(format!("http://{address}/api/settings/ssh/preflight"))
        .json(&json!({"host":"orx-test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(tested.status(), StatusCode::OK);
    let tested: Value = tested.json().await.unwrap();
    assert_eq!(tested["reachable"], true, "{tested}");
    assert_eq!(server.auth_attempts().unwrap(), attempts);
    let processes = crate::process::command("ps")
        .args(["-axo", "ppid=,comm=,args="])
        .output()
        .unwrap();
    let processes = String::from_utf8(processes.stdout).unwrap();
    let sessions: Vec<_> = processes
        .lines()
        .filter(|line| {
            line.trim_start()
                .starts_with(&format!("{} ", std::process::id()))
                && line.contains(server.config_path().to_str().unwrap())
                && line.contains("BatchMode=yes")
        })
        .collect();
    assert_eq!(sessions.len(), 2, "{sessions:?}");
    for session in sessions {
        assert!(
            session.contains("ControlMaster=no") && session.contains("ControlPath=none"),
            "{session}"
        );
        assert!(!session.contains(" -M "), "{session}");
    }
    let hosts: Value = client
        .get(format!("http://{address}/api/compute/hosts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(hosts.as_array().unwrap().len(), 2);
    assert_eq!(runtime.host_states("orx-test").len(), 2);
    api.store()
        .seed_engine_host(
            "unused",
            "/old-root",
            Some(crate::engine::transport::HostClass::Key),
            &crate::engine::transport::HostState::default(),
        )
        .unwrap();
    assert_eq!(
        client
            .post(format!("http://{address}/api/compute/hosts/unused/retry"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(runtime.host_states("unused").is_empty());
    let retry = format!("http://{address}/api/compute/hosts/orx-test/retry");
    server.proxy().reset().unwrap();
    host_event(&mut receiver, Status::Backoff).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime
            .host_states("orx-test")
            .iter()
            .any(|(_, state)| state.status != Status::Backoff)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let outage = client
        .get(format!(
            "http://{address}/api/runs/large/log?start={}&end={}",
            middle + 6,
            middle + 7
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(outage.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        outage.json::<Value>().await.unwrap()["contact"],
        "unreachable"
    );
    let response = client.post(&retry).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    host_event(&mut receiver, Status::Live).await;
    assert_eq!(runtime.host_states("orx-test").len(), 2);
    assert!(runtime
        .host_states("orx-test")
        .iter()
        .all(|(_, state)| state.status == Status::Live));
    let attempts = server.auth_attempts().unwrap();
    assert_eq!(
        client.post(&retry).send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(server.auth_attempts().unwrap(), attempts);
    let keys = std::fs::read(server.directory().join("authorized_keys")).unwrap();
    std::fs::write(server.directory().join("authorized_keys"), "").unwrap();
    server.proxy().reset().unwrap();
    host_event(&mut receiver, Status::Backoff).await;
    let response = client.post(&retry).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["status"],
        "needs_auth"
    );
    host_event(&mut receiver, Status::NeedsAuth).await;
    let outage = client
        .get(format!(
            "http://{address}/api/runs/large/log?start={}&end={}",
            middle + 8,
            middle + 9
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(outage.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        outage.json::<Value>().await.unwrap()["contact"],
        "needs_auth"
    );
    let attempts = server.auth_attempts().unwrap();
    assert_eq!(
        client.post(&retry).send().await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(server.auth_attempts().unwrap(), attempts);
    api.engine
        .submit(
            backend.as_ref(),
            SubmittingRun {
                id: "pending",
                experiment_id: "exp",
                project_id: "project",
                metadata: &crate::jobs::BackendDescriptor::parse(r#"{"kind":"ssh_job"}"#).unwrap(),
                command: "printf resumed",
                created_at: 1,
                commit_sha: None,
                chat_session_id: None,
                spec_digest: &orx_host::sha(b"printf resumed"),
                launch: &LaunchSpec {
                    kind: "ssh_job".into(),
                    target: Some("orx-test".into()),
                    options: json!({"root":"~/.orx/engine"}),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(api.run("pending").unwrap().status, "submitting");
    runtime.start();
    std::fs::write(server.directory().join("authorized_keys"), keys).unwrap();
    let mut request = format!("ws://{address}/api/settings/ssh/connect?host=orx-test&backend=ssh")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    tokio::time::timeout(Duration::from_secs(45), async {
        while let Some(message) = socket.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message.unwrap() {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(value["type"], "error", "{value}");
                if value["type"] == "complete" {
                    return;
                }
            }
        }
        panic!("Connect ended without completion");
    })
    .await
    .unwrap();
    let _ = socket.close(None).await;
    host_event(&mut receiver, Status::Live).await;
    let attempts = server.auth_attempts().unwrap();
    let mut request = format!("ws://{address}/api/settings/ssh/connect?host=orx-test&backend=ssh")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(message) = socket.next().await {
            if let tokio_tungstenite::tungstenite::Message::Text(text) = message.unwrap() {
                let value: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(value["type"], "prompt");
                assert_ne!(value["type"], "error", "{value}");
                if value["type"] == "complete" {
                    return;
                }
            }
        }
        panic!("Live Connect ended without completion");
    })
    .await
    .unwrap();
    let _ = socket.close(None).await;
    assert_eq!(server.auth_attempts().unwrap(), attempts);
    let host = api
        .store()
        .engine_host("orx-test", "~/.orx/engine")
        .unwrap()
        .unwrap();
    assert_eq!(host.status, Status::Live);
    assert!(runtime
        .host_states("orx-test")
        .iter()
        .all(|(_, state)| state.status == Status::Live));
    assert!(host.cause.is_none());
    assert!(host.next_retry_ms.is_none());
    tokio::time::timeout(Duration::from_secs(45), async {
        while api.run("pending").unwrap().status != "done" {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("pending run: {:?}", api.run("pending").unwrap()));
    task.abort();
}

#[tokio::test]
async fn runner_one_local_logs_use_pages_for_live_and_finished_runs() {
    use tower::ServiceExt;
    let f = Fixture::new().await;
    f.insert("legacy", "exp", "project", "running");
    let mut run = f.api.run("legacy").unwrap();
    run.backend_json = r#"{"kind":"local","jobId":"/legacy"}"#.into();
    f.api.store().upsert_run(&run).unwrap();
    std::fs::create_dir_all(f.api.data_dir.join("run-logs")).unwrap();
    std::fs::write(f.api.data_dir.join("run-logs/legacy.log"), b"legacy output").unwrap();
    let app = dashboard_routes(f.api.clone());
    for (status, eof) in [("running", true), ("done", true)] {
        if status == "done" {
            f.api
                .store()
                .set_engine_phase("legacy", "running", "done", Some(10), Some(0), None)
                .unwrap();
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/runs/legacy/log?start=7&end=20")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["kind"], "data");
        assert_eq!(value["data"], "b3V0cHV0");
        assert_eq!(value["start"], 7);
        assert_eq!(value["len"], 13);
        assert_eq!(value["eof"], eof);
    }
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dashboard_log_identity_is_opaque_and_preserves_large_device_and_inode_numbers() {
    use tower::ServiceExt;
    let f = Fixture::new().await;
    f.insert("wide", "exp", "project", "running");
    let wide = FileIdent {
        dev: u64::MAX - 2,
        ino: (1u64 << 60) + 7,
    };
    let page = LogPage::Data {
        len: Some(4),
        ident: wide.clone(),
        start: 0,
        bytes: b"wide".to_vec(),
        eof: true,
    };
    f.backend.scripted_pages.lock().unwrap().extend([
        page.clone(),
        page.clone(),
        LogPage::Reset {
            info: Some(orx_host_proto::LogInfo {
                ident: wide.clone(),
                len: 4,
            }),
        },
        page,
    ]);
    let app = dashboard_routes(f.api.clone());
    let mut token = None;
    for reset in [false, false, true] {
        let query = token
            .as_ref()
            .map_or(String::new(), |token| format!("&ident={token}"));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/runs/wide/log?start=0&end=4{query}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let actual = if reset {
            assert_eq!(value["kind"], "reset");
            &value["info"]["ident"]
        } else {
            assert_eq!(value["kind"], "data");
            &value["ident"]
        };
        let actual = actual.as_str().expect("dashboard identity must be opaque");
        if let Some(token) = &token {
            assert_eq!(actual, token);
        }
        token = Some(actual.to_owned());
    }
    assert_eq!(
        *f.backend.identities.lock().unwrap(),
        [None, Some(wide.clone()), Some(wide.clone())]
    );
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/runs/wide/log?start=0&end=4&ident=incorrect")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let cli = f
        .client
        .json(reqwest::Method::GET, "runs/wide/log?start=0&end=4", None)
        .await
        .unwrap();
    assert_eq!(cli["ident"], String::from(wide));
}

#[tokio::test]
async fn idle_closes_admission_then_rechecks_in_flight_requests_and_sse() {
    let _serial = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    let mut idle = Idle::default();
    let active = crate::commands::up::Active::new();
    assert!(!idle.check(&f.api, Duration::ZERO).unwrap());
    assert!(!idle.check(&f.api, Duration::from_secs(600)).unwrap());
    assert!(!crate::commands::up::stop_admitting_if_idle(&f.api).unwrap());
    assert!(!crate::commands::up::IDLE_EXIT.load(Ordering::SeqCst));
    drop(active);
    assert!(!idle.check(&f.api, Duration::from_secs(601)).unwrap());
    assert!(idle.check(&f.api, Duration::from_secs(1201)).unwrap());
    f.api.clients.fetch_add(1, Ordering::SeqCst);
    assert!(!crate::commands::up::stop_admitting_if_idle(&f.api).unwrap());
    f.api.clients.fetch_sub(1, Ordering::SeqCst);
    assert!(crate::commands::up::stop_admitting_if_idle(&f.api).unwrap());
    crate::commands::up::IDLE_EXIT.store(false, Ordering::SeqCst);
}

#[tokio::test]
async fn kept_prefix_is_a_short_page_even_when_the_host_is_unavailable() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    keep(&f.api, "run", 0, b"0123", false);
    f.backend.offline.store(true, Ordering::SeqCst);
    let page = f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=2&end=10", None)
        .await
        .unwrap();
    assert_eq!(page["kind"], "data");
    assert_eq!(page["start"], 2);
    assert_eq!(page["data"], "MjM=");
    assert_eq!(page["eof"], false);
    assert_eq!(f.backend.reads.load(Ordering::SeqCst), 0);
    assert!(f
        .client
        .json(reqwest::Method::GET, "runs/run/log?start=4&end=10", None)
        .await
        .is_err());
}

#[tokio::test]
async fn cli_wait_skips_unknown_sse_event_names_before_decoding_their_data() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "done");
    let terminal = f.api.view(&f.api.run("run").unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut descriptor = f.descriptor.clone();
    descriptor.port = listener.local_addr().unwrap().port();
    let app = axum::Router::new().route("/api/v2/events", axum::routing::get(move || {
        let body = format!("event: future-progress\ndata: this is not JSON\n\nevent: run\ndata: {terminal}\n\n");
        async move { ([("content-type", "text/event-stream")], body) }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    ApiClient::new(descriptor)
        .unwrap()
        .wait(Filter {
            run: Some("run".into()),
            wait: true,
            ..Default::default()
        })
        .await
        .unwrap();
    server.abort();
}

#[tokio::test]
async fn initial_connection_failure_and_idle_refusal_rediscover_without_replaying_accepted_posts() {
    for idle in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let app = Router::new()
            .route(
                "/api/v2/echo",
                post(
                    move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                        let count = count.clone();
                        async move {
                            assert_eq!(headers["authorization"], "Bearer secret");
                            count.fetch_add(1, Ordering::SeqCst);
                            Json(body)
                        }
                    },
                ),
            )
            .route(
                "/api/v2/events",
                get(|| async { ([("content-type", "text/event-stream")], "") }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let refusals = Arc::new(AtomicUsize::new(0));
        let count = refusals.clone();
        let app = Router::new().fallback(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { StatusCode::SERVICE_UNAVAILABLE }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let stale_port = listener.local_addr().unwrap().port();
        let exiting = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut command = crate::process::tokio_command(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "commands::up::api_v2::tests::reconnect_child",
                "--nocapture",
            ])
            .env("ORX_TEST_STREAM_RECONNECT", "post")
            .env("ORX_UP_PORT", port.to_string())
            .env("ORX_UP_AUTH_TOKEN", "secret")
            .env_remove("ORX_TEST_STALE_PORT");
        if idle {
            command.env("ORX_TEST_STALE_PORT", stale_port.to_string());
        }
        let output = command.output().await.unwrap();
        server.abort();
        exiting.abort();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(refusals.load(Ordering::SeqCst), if idle { 2 } else { 0 });
    }
}

#[tokio::test]
async fn cli_abandon_uses_the_existing_route_and_preserves_its_guards() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "running");
    assert!(f.client.abandon("run").await.is_err());
    f.api
        .store()
        .set_run_contact(
            "run",
            &Contact::Unreachable {
                since: 3,
                cause: "offline".into(),
            },
        )
        .unwrap();
    f.client.abandon("run").await.unwrap();
    let run = f.api.run("run").unwrap();
    assert_eq!(run.status, "lost");
    assert!(f
        .api
        .store()
        .engine_run_state("run")
        .unwrap()
        .unwrap()
        .cancel_accepted_at
        .is_some());
}

#[tokio::test]
async fn cli_cancel_selects_one_run_and_checks_its_experiment() {
    use clap::Parser;
    let cli = crate::Cli::try_parse_from(["orx", "exp", "cancel", "--run", "first"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(crate::Command::Exp(crate::ExpArgs {
            command: crate::ExpCommand::Cancel {
                exp_id: None,
                run: Some(_)
            }
        }))
    ));
    let f = Fixture::new().await;
    f.insert("first", "exp", "project", "running");
    f.insert("second", "exp", "project", "running");
    f.insert("done", "exp", "project", "done");
    assert!(f.client.cancel(Some("other"), Some("first")).await.is_err());
    assert!(f.client.cancel(None, Some("done")).await.is_err());
    f.client.cancel(None, Some("first")).await.unwrap();
    let accepted = |id| {
        f.api
            .store()
            .engine_run_state(id)
            .unwrap()
            .unwrap()
            .cancel_accepted_at
            .is_some()
    };
    assert!(accepted("first"));
    assert!(!accepted("second"));
    assert!(!accepted("done"));
    f.client.cancel(Some("exp"), None).await.unwrap();
    assert!(accepted("second"));
}

#[tokio::test]
async fn dashboard_cleanup_uses_the_existing_handler_and_abandon_security_boundary() {
    use tower::ServiceExt;
    let _guard = crate::commands::up::ACTIVE_TEST_LOCK.lock().await;
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "cancelled");
    let auth = crate::commands::remote_host::RemoteAuth::new("callback");
    auth.register("session");
    let app = crate::commands::up::server_router(
        dashboard_routes(f.api.clone()).merge(routes(f.api.clone())),
        Some(auth),
        crate::commands::up_remote::Reach::Loopback,
    );
    let request = |action: &str, token: Option<&str>, origin: &str| {
        let mut builder = Request::builder()
            .method("POST")
            .uri(format!("/api/runs/run/{action}"))
            .header("host", "127.0.0.1:1234")
            .header("origin", origin);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        builder.body(axum::body::Body::empty()).unwrap()
    };
    for action in ["cleanup", "abandon"] {
        for token in [None, Some("callback")] {
            assert_eq!(
                app.clone()
                    .oneshot(request(action, token, "http://127.0.0.1:1234"))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            app.clone()
                .oneshot(request(action, Some("session"), "https://evil.example"))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    let response = app
        .clone()
        .oneshot(request("cleanup", Some("session"), "http://127.0.0.1:1234"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"accepted":true})
    );
    f.insert("active", "exp", "project", "running");
    let response = dashboard_routes(f.api.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/runs/active/cleanup")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn cancellation_proof_reaches_both_api_projections_without_contact_prose() {
    let f = Fixture::new().await;
    for (id, remaining, note) in [
        ("survivors", 2, None),
        ("unconfirmed", 0, Some(vec![])),
        ("unreadable", 0, Some(vec![789, 790])),
        ("verified", 0, None),
    ] {
        f.insert(id, "exp", "project", "running");
        f.api.store().set_backend_json(id, &json!({
            "kind": "engine", "handle": {"type": "Ssh", "host": "gpu-box", "root": "/engine"}
        }).to_string()).unwrap();
        f.api
            .engine
            .observe(
                id,
                Observation::CancelReceipt {
                    finished_at: Some(10),
                    code: None,
                    remaining,
                    note: note.clone(),
                },
            )
            .unwrap();
        let store = f.api.store();
        let run = store.get_run(id).unwrap().unwrap();
        let dashboard =
            serde_json::to_value(crate::commands::up::ApiRun::new(&store, &run).unwrap()).unwrap();
        drop(store);
        let view = f.api.view(&run).unwrap();
        for projection in [&view, &dashboard] {
            assert_eq!(projection["host"], "gpu-box");
            assert_eq!(projection["cancelUnconfirmed"], json!(note));
            if remaining > 0 {
                assert_eq!(projection["contact"]["type"], "stopping");
                assert_eq!(projection["contact"]["count"], remaining);
                assert!(projection["contact"].get("detail").is_none());
            } else {
                assert!(projection["contact"].is_null() || projection["contact"]["type"] == "live");
            }
            assert_eq!(
                projection["resultMarkdown"].as_str(),
                note.as_ref().map(|pids| if pids.is_empty() {
                    "In-container processes or processes outside the host scan couldn't be checked."
                } else {
                    "The host couldn't inspect processes 789, 790 to confirm the stop."
                })
            );
        }
        f.api
            .store()
            .set_result_markdown(id, "Edited result")
            .unwrap();
        let edited = f.api.run(id).unwrap();
        assert_eq!(
            f.api.view(&edited).unwrap()["cancelUnconfirmed"],
            json!(note)
        );
    }
}

#[tokio::test]
async fn a_waiting_reason_reaches_both_views_and_the_event_stream() {
    let f = Fixture::new().await;
    f.insert("run", "exp", "project", "starting");
    let mut events = f
        .client
        .events(&Filter {
            run: Some("run".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    async fn next(events: &mut (impl Stream<Item = Result<Value>> + Unpin)) -> Value {
        tokio::time::timeout(Duration::from_secs(1), events.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
    }
    assert!(next(&mut events).await["waitingReason"].is_null());

    f.api
        .engine
        .observe(
            "run",
            Observation::Queued {
                detail: Some("Provisioning box".into()),
            },
        )
        .unwrap();
    let event = next(&mut events).await;
    assert_eq!(event["event"], "changed");
    assert_eq!(event["waitingReason"], "Provisioning box");
    let cli: crate::plane::Run = serde_json::from_value(event).unwrap();
    assert_eq!(
        cli.waiting_detail().as_deref(),
        Some("waiting: Provisioning box")
    );
    let dashboard = {
        let store = f.api.store();
        let run = store.get_run("run").unwrap().unwrap();
        serde_json::to_value(crate::commands::up::ApiRun::new(&store, &run).unwrap()).unwrap()
    };
    assert_eq!(dashboard["waitingReason"], "Provisioning box");

    f.api
        .engine
        .observe("run", Observation::Running { started_at: None })
        .unwrap();
    let event = next(&mut events).await;
    assert_eq!(event["status"], "running");
    assert!(event["waitingReason"].is_null());
}

#[tokio::test]
async fn abandon_refusals_name_their_cause_and_the_way_forward() {
    use tower::ServiceExt;
    let f = Fixture::new().await;
    let app = dashboard_routes(f.api.clone());
    let refusal = |id: &'static str| {
        let app = app.clone();
        async move {
            let request = Request::builder()
                .method("POST")
                .uri(format!("/api/runs/{id}/abandon"))
                .body(axum::body::Body::empty())
                .unwrap();
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            (status, String::from_utf8(bytes.to_vec()).unwrap())
        }
    };

    f.insert("done", "exp", "project", "done");
    let (status, message) = refusal("done").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(message.contains("already ended (done)"), "{message}");

    f.insert("live", "exp", "project", "running");
    let (status, message) = refusal("live").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(message.contains("`orx exp cancel --run live`"), "{message}");

    f.insert("legacy", "exp", "project", "running");
    let mut legacy = f.api.run("legacy").unwrap();
    legacy.backend_json = r#"{"kind":"ssh_job","host":"gpu-box"}"#.into();
    f.api.store().upsert_run(&legacy).unwrap();
    f.api
        .store()
        .set_run_contact(
            "legacy",
            &Contact::Unreachable {
                since: 1,
                cause: "offline".into(),
            },
        )
        .unwrap();
    let (status, message) = refusal("legacy").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(message.contains("orx 0.2"), "{message}");
    assert!(
        message.contains("`orx exp cancel --run legacy`"),
        "{message}"
    );
    assert_eq!(f.api.run("legacy").unwrap().status, "running");

    // A stop its backend keeps refusing leaves a reachable run nothing but Abandon.
    f.backend.cancel_refused.store(true, Ordering::SeqCst);
    let cancel = f
        .client
        .json(reqwest::Method::POST, "runs/live/cancel", None)
        .await
        .unwrap();
    assert_eq!(cancel["cancelError"], "refused by the provider");
    let (status, message) = refusal("live").await;
    assert_eq!(status, StatusCode::OK, "{message}");
    assert_eq!(f.api.run("live").unwrap().status, "lost");
}
