use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, watch};

use crate::error::{anyhow, Result};
use crate::local::local_models;
use crate::local::native_store::{self, opencode_database::DatabaseLease, NativeStore};
use crate::local::opencode::{self, AgentEndpoint, ResolvedBinary};

const ATTEMPT_LIMIT: usize = 8;
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(600);

struct Server {
    child: tokio::process::Child,
    endpoint: AgentEndpoint,
    database: DatabaseLease,
}

impl Server {
    async fn start(binary: &ResolvedBinary, provider: Option<(&str, &str)>) -> Result<Self> {
        let db = native_store::prepare_opencode(NativeStore::Isolated)?;
        let lease = opencode::prepare_database(binary, &db).await?;
        let directory = db
            .parent()
            .ok_or_else(|| anyhow!("OpenCode database has no directory"))?
            .to_owned();
        let mut cmd = crate::process::tokio_command(&binary.path);
        if let Some((base, key)) = provider {
            local_models::prepare_provider_probe(&mut cmd, base, key)?;
        } else {
            local_models::prepare_env(&mut cmd, None, crate::local::opencode::Protocol::V2)?;
        }
        cmd.env("OPENCODE_DB", lease.path())
            .env("OPENCODE_CONFIG_PROJECT_DISABLE", "1")
            .current_dir(&directory);
        let (child, endpoint) = opencode::start_server(binary, cmd).await?;
        Ok(Self {
            child,
            endpoint,
            database: lease,
        })
    }

    fn request(&self, method: Method, segments: &[&str]) -> reqwest::RequestBuilder {
        let mut url = reqwest::Url::parse(&self.endpoint.base_url).expect("OpenCode server URL");
        url.path_segments_mut()
            .expect("OpenCode HTTP URL")
            .extend(segments);
        self.endpoint
            .client
            .request(method, url)
            .query(&[(
                "location[directory]",
                self.database
                    .path()
                    .parent()
                    .expect("OpenCode database directory")
                    .to_string_lossy(),
            )])
            .timeout(Duration::from_secs(10))
    }

    async fn get(&self, segments: &[&str]) -> Result<Value> {
        Ok(self
            .request(Method::GET, segments)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn stop(&mut self) {
        crate::local::chat::kill_shell_group(self.child.id());
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        crate::local::chat::kill_shell_group(self.child.id());
    }
}

async fn catalog(server: &Server, resource: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let response = server
            .request(Method::GET, &["api", resource])
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        let value: Value = response.error_for_status()?.json().await?;
        if value["data"]
            .as_array()
            .is_some_and(|items| items.is_empty())
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }
        if !value["data"].is_array() {
            return Err(anyhow!(
                "OpenCode returned an incomplete {resource} list. Re-check OpenCode."
            ));
        }
        return Ok(value);
    }
}

pub(crate) async fn provider_models(
    binary: &ResolvedBinary,
    base: &str,
    key: &str,
) -> Result<Vec<String>> {
    let mut server = Server::start(binary, Some((base, key))).await?;
    let result = catalog(&server, "model").await;
    server.stop().await;
    let models: Vec<String> = result?["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|model| model["providerID"] == base)
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .collect();
    if models.is_empty() {
        return Err(anyhow!(
            "OpenCode listed no models for this provider. Check the key and re-check OpenCode."
        ));
    }
    Ok(models)
}

pub(crate) async fn integrations(binary: &ResolvedBinary) -> Result<Value> {
    let mut server = Server::start(binary, None).await?;
    let result = catalog(&server, "integration").await;
    server.stop().await;
    Ok(Value::Array(result?["data"].as_array().into_iter().flatten().filter_map(|integration| {
        let methods: Vec<Value> = integration["methods"].as_array().into_iter().flatten()
            .filter(|method| method["type"] == "oauth" || method["type"] == "key")
            .map(|method| {
                let mut result = json!({"type":method["type"]});
                if method["type"] == "oauth" { result["id"] = method["id"].clone(); }
                if let Some(label) = method.get("label") { result["label"] = label.clone(); }
                // What the sign-in asks before it starts, such as GitHub's deployment type.
                if method["type"] == "oauth" && method["form"].is_array() { result["form"] = method["form"].clone(); }
                result
            }).collect();
        if methods.is_empty() { return None; }
        Some(json!({"id":integration["id"], "name":integration["name"], "methods":methods,
            "connected":integration["connections"].as_array().is_some_and(|items| !items.is_empty())}))
    }).collect()))
}

pub(crate) async fn sign_out(binary: &ResolvedBinary, integration: &str) -> Result<()> {
    let mut server = Server::start(binary, None).await?;
    let result = disconnect(&server, integration).await;
    server.stop().await;
    result
}

async fn disconnect(server: &Server, integration: &str) -> Result<()> {
    let catalog = catalog(server, "integration").await?;
    let info = catalog["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["id"] == integration)
        .ok_or_else(|| anyhow!("OpenCode could not find this provider. Re-check OpenCode."))?;
    for connection in info["connections"].as_array().into_iter().flatten() {
        if connection["type"] != "credential" {
            continue;
        }
        let id = connection["id"]
            .as_str()
            .ok_or_else(|| anyhow!("OpenCode returned a connection without an id"))?;
        server
            .request(Method::DELETE, &["api", "credential", id])
            .send()
            .await?
            .error_for_status()?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct SignIn {
    pub integration: String,
    pub method: String,
    pub name: String,
    pub icon: Option<Value>,
    /// Answers to the method's form, passed to OpenCode as they are.
    pub answer: Option<serde_json::Map<String, Value>>,
}

enum Command {
    Code(String, oneshot::Sender<Result<()>>),
    Cancel(oneshot::Sender<()>),
}

struct Attempt {
    integration: String,
    status: watch::Receiver<Value>,
    commands: mpsc::Sender<Command>,
}

pub struct SignIns {
    attempts: Mutex<Option<BTreeMap<String, Attempt>>>,
    records: PathBuf,
}

impl Default for SignIns {
    fn default() -> Self {
        Self {
            attempts: Mutex::new(Some(BTreeMap::new())),
            records: local_models::providers_path(),
        }
    }
}

impl SignIns {
    pub(crate) async fn start(&self, binary: &ResolvedBinary, req: SignIn) -> Result<Value> {
        if !local_models::is_provider_id(&req.integration)
            || req.method.is_empty()
            || req.method.len() > 128
        {
            return Err(anyhow!("Choose an OpenCode sign-in method."));
        }
        crate::local::agent_profiles::validate_identity(&req.name, req.icon.as_ref())?;
        if req.answer.as_ref().is_some_and(|answer| {
            answer.len() > 32
                || serde_json::to_string(answer).map_or(true, |text| text.len() > 8192)
        }) {
            return Err(anyhow!("Answer the sign-in's questions again."));
        }
        if local_models::providers()?
            .get(&req.integration)
            .is_some_and(|record| {
                matches!(
                    record.credential,
                    local_models::ProviderCredential::Key { .. }
                )
            })
        {
            return Err(anyhow!(
                "This provider has an API key. Remove it before signing in."
            ));
        }
        let server = Server::start(binary, None).await?;
        self.start_with_server(server, req).await
    }

    async fn start_with_server(&self, server: Server, req: SignIn) -> Result<Value> {
        let integrations = catalog(&server, "integration").await?;
        let available = integrations["data"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|integration| {
                integration["id"] == req.integration
                    && integration["methods"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|method| method["type"] == "oauth" && method["id"] == req.method)
            });
        if !available {
            return Err(anyhow!(
                "This sign-in method is unavailable. Re-check OpenCode."
            ));
        }
        let started: Value = server
            .request(
                Method::POST,
                &["api", "integration", &req.integration, "connect", "oauth"],
            )
            .json(&match &req.answer {
                Some(answer) => json!({"methodID":req.method,"label":req.name,"answer":answer}),
                None => json!({"methodID":req.method,"label":req.name}),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.attach(server, req, &started["data"])
    }

    fn attach(&self, server: Server, req: SignIn, started: &Value) -> Result<Value> {
        let remote_id = started["attemptID"]
            .as_str()
            .ok_or_else(|| anyhow!("OpenCode returned no sign-in attempt"))?
            .to_owned();
        let mode = started["mode"]
            .as_str()
            .filter(|mode| ["auto", "code"].contains(mode))
            .ok_or_else(|| anyhow!("OpenCode returned no sign-in mode"))?;
        let url = started["url"]
            .as_str()
            .ok_or_else(|| anyhow!("OpenCode returned no sign-in URL"))?;
        let parsed = reqwest::Url::parse(url)?;
        if !matches!(parsed.scheme(), "https" | "http") {
            return Err(anyhow!("OpenCode returned an unsupported sign-in URL"));
        }
        let expires = started["time"]["expires"]
            .as_i64()
            .ok_or_else(|| anyhow!("OpenCode returned no sign-in expiry"))?;
        let remaining = expires.saturating_sub(crate::store::now_ms()).max(0) as u64;
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(remaining).min(ATTEMPT_TIMEOUT);
        let (status, receiver) = watch::channel(json!({"status":"pending"}));
        let (commands, incoming) = mpsc::channel(4);
        let id = uuid::Uuid::new_v4().to_string();
        let mut guard = self
            .attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let attempts = guard
            .as_mut()
            .ok_or_else(|| anyhow!("OpenResearch is stopping. Restart it to sign in."))?;
        let active = attempts
            .values()
            .filter(|attempt| !attempt.commands.is_closed())
            .count();
        if active >= ATTEMPT_LIMIT {
            return Err(anyhow!(
                "Finish or cancel an OpenCode sign-in before starting another."
            ));
        }
        if attempts
            .values()
            .any(|attempt| attempt.integration == req.integration && !attempt.commands.is_closed())
        {
            return Err(anyhow!(
                "A sign-in for this provider is already open. Finish or cancel it first."
            ));
        }
        if attempts.len() >= 32 {
            attempts.retain(|_, attempt| !attempt.commands.is_closed());
        }
        attempts.insert(
            id.clone(),
            Attempt {
                integration: req.integration.clone(),
                status: receiver,
                commands,
            },
        );
        tokio::spawn(run_attempt(
            server,
            req,
            remote_id,
            deadline,
            self.records.clone(),
            status,
            incoming,
        ));
        Ok(
            json!({"attempt":id,"url":url,"mode":mode,"instructions":started["instructions"].as_str().unwrap_or_default()}),
        )
    }

    pub fn status(&self, id: &str) -> Option<Value> {
        self.attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()?
            .get(id)
            .map(|attempt| attempt.status.borrow().clone())
    }

    pub async fn code(&self, id: &str, code: String) -> Result<()> {
        if code.trim().is_empty() || code.len() > 4096 {
            return Err(anyhow!(
                "Paste the authorization code from the sign-in page."
            ));
        }
        let sender = self
            .attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .and_then(|attempts| attempts.get(id))
            .ok_or_else(|| anyhow!("Sign-in not found. Start sign-in again."))?
            .commands
            .clone();
        let (reply, result) = oneshot::channel();
        sender
            .send(Command::Code(code.trim().to_owned(), reply))
            .await
            .map_err(|_| anyhow!("This sign-in has ended. Start sign-in again."))?;
        result
            .await
            .map_err(|_| anyhow!("This sign-in has ended. Start sign-in again."))?
    }

    pub async fn cancel(&self, id: &str) {
        let attempt = self
            .attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_mut()
            .and_then(|attempts| attempts.remove(id));
        if let Some(attempt) = attempt {
            cancel(attempt).await;
        }
    }

    pub async fn cancel_integration(&self, integration: &str) {
        let ids: Vec<String> = self
            .attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .flat_map(|attempts| attempts.iter())
            .filter(|(_, attempt)| attempt.integration == integration)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.cancel(&id).await;
        }
    }

    pub async fn shutdown(&self) {
        let attempts = self
            .attempts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .unwrap_or_default();
        futures::future::join_all(attempts.into_values().map(cancel)).await;
    }
}

async fn cancel(attempt: Attempt) {
    let (reply, result) = oneshot::channel();
    if attempt.commands.send(Command::Cancel(reply)).await.is_ok() {
        let _ = result.await;
    }
}

async fn run_attempt(
    mut server: Server,
    req: SignIn,
    remote_id: String,
    deadline: tokio::time::Instant,
    records: PathBuf,
    status: watch::Sender<Value>,
    mut commands: mpsc::Receiver<Command>,
) {
    let path = [
        "api",
        "integration",
        &req.integration,
        "connect",
        "oauth",
        &remote_id,
    ];
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    let mut cancelled = None;
    let outcome: Result<Value> = tokio::time::timeout_at(deadline, async {
        loop {
            tokio::select! {
                command = commands.recv() => {
                    match command {
                        Some(Command::Code(code, reply)) => {
                            let mut complete = path.to_vec(); complete.push("complete");
                            let result = server.request(Method::POST, &complete).json(&json!({"code":code})).send().await.and_then(reqwest::Response::error_for_status);
                            match result {
                                Ok(_) => { let _ = reply.send(Ok(())); }
                                Err(_) => {
                                    let message = "OpenCode could not accept the code. Start sign-in again.";
                                    let _ = reply.send(Err(anyhow!(message)));
                                    return Ok(json!({"status":"failed","message":message}));
                                }
                            }
                        }
                        Some(Command::Cancel(reply)) => { cancelled = Some(reply); return Ok(json!({"status":"failed","message":"Sign-in cancelled."})); }
                        None => return Ok(json!({"status":"failed","message":"Sign-in cancelled."})),
                    }
                }
                _ = interval.tick() => {
                    if server.child.try_wait()?.is_some() { return Err(anyhow!("OpenCode stopped. Start sign-in again.")); }
                    let response = server.get(&path).await?;
                    match response["data"]["status"].as_str() {
                        Some("pending") => {}
                        Some("complete") => {
                            local_models::record_oauth(&records, &req.integration, req.name.clone(), req.icon.clone())?;
                            return Ok(json!({"status":"complete"}));
                        }
                        Some("failed") => return Ok(json!({"status":"failed","message":response["data"]["message"].as_str().unwrap_or("OpenCode could not sign in. Start sign-in again.")})),
                        Some("expired") => return Ok(json!({"status":"expired"})),
                        _ => return Err(anyhow!("OpenCode returned an unknown sign-in status. Re-check OpenCode.")),
                    }
                }
            }
        }
    }).await.unwrap_or_else(|_| Ok(json!({"status":"expired"})));
    if !outcome
        .as_ref()
        .is_ok_and(|value| value["status"] == "complete")
    {
        let _ = server
            .request(Method::DELETE, &path)
            .timeout(Duration::from_secs(2))
            .send()
            .await;
    }
    server.stop().await;
    drop(server);
    status.send_replace(
        outcome.unwrap_or_else(|error| json!({"status":"failed","message":error.to_string()})),
    );
    if let Some(reply) = cancelled {
        let _ = reply.send(());
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use axum::extract::{Query, State};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[derive(Default)]
    struct Mock {
        complete: AtomicBool,
        cancelled: AtomicBool,
        requests: Mutex<Vec<Value>>,
    }

    async fn fixture(root: &std::path::Path) -> (Server, Arc<Mock>, tokio::task::JoinHandle<()>) {
        let mock = Arc::new(Mock::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new()
            .route("/api/integration", get(|| async { Json(json!({"data":[{"id":"xai","methods":[{"type":"oauth","id":"device","label":"SuperGrok"}],"connections":[{"type":"credential","id":"cred_oauth","method":"oauth"},{"type":"credential","id":"cred_key","method":"key"},{"type":"env","name":"XAI_API_KEY"}]}]})) }))
            .route("/api/credential/{id}", axum::routing::delete(|State(mock): State<Arc<Mock>>, axum::extract::Path(id): axum::extract::Path<String>| async move {
                mock.requests.lock().unwrap().push(json!({"removed":id}));
                axum::http::StatusCode::NO_CONTENT
            }))
            .route("/api/integration/xai/connect/oauth", post(|State(mock): State<Arc<Mock>>, Query(query): Query<BTreeMap<String, String>>, Json(body): Json<Value>| async move {
                assert!(query.contains_key("location[directory]"));
                mock.requests.lock().unwrap().push(body);
                Json(json!({"data":{"attemptID":"con_test","url":"https://example.test/sign-in","mode":"code","instructions":"Paste the code","time":{"expires":crate::store::now_ms()+60000}}}))
            }))
            .route("/api/integration/xai/connect/oauth/con_test", get(|State(mock): State<Arc<Mock>>| async move {
                let status = if mock.complete.load(Ordering::SeqCst) { "complete" } else { "pending" };
                Json(json!({"data":{"status":status}}))
            }).delete(|State(mock): State<Arc<Mock>>| async move {
                mock.cancelled.store(true, Ordering::SeqCst);
                axum::http::StatusCode::NO_CONTENT
            }))
            .route("/api/integration/xai/connect/oauth/con_test/complete", post(|State(mock): State<Arc<Mock>>, Json(body): Json<Value>| async move {
                assert_eq!(body, json!({"code":"test-code"}));
                mock.complete.store(true, Ordering::SeqCst);
                axum::http::StatusCode::NO_CONTENT
            }))
            .with_state(mock.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut cmd = crate::process::tokio_command("/bin/sleep");
        cmd.arg("60")
            .env_clear()
            .current_dir(root)
            .process_group(0)
            .kill_on_drop(true);
        let child = cmd.spawn().unwrap();
        let lease = DatabaseLease::acquire(&root.join("opencode.db"), 2).unwrap();
        let server = Server {
            child,
            endpoint: AgentEndpoint {
                base_url: format!("http://{address}"),
                client: crate::net::loopback_client().build().unwrap(),
                protocol: opencode::Protocol::V2,
                legacy_v2_api: false,
            },
            database: lease,
        };
        (server, mock, task)
    }

    fn request() -> SignIn {
        SignIn {
            integration: "xai".into(),
            method: "device".into(),
            name: "Research".into(),
            icon: Some(json!({"brand":true,"color":"gray"})),
            answer: None,
        }
    }

    fn manager(root: &std::path::Path) -> SignIns {
        SignIns {
            attempts: Mutex::new(Some(BTreeMap::new())),
            records: root.join("opencode-providers.json"),
        }
    }

    async fn terminal(manager: &SignIns, id: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let status = manager.status(id).unwrap();
                if status["status"] != "pending" {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }

    fn assert_stopped(pid: u32) {
        // SAFETY: signal zero only checks existence of the child spawned by the fixture.
        assert_eq!(unsafe { libc::kill(pid.try_into().unwrap(), 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[tokio::test]
    async fn code_signin_writes_record_and_releases_child_and_lease() {
        // Another test's forked child can hold an inherited lease descriptor, so this runs alone.
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let root = crate::local::git::TemporaryDirectory::new("orx-signin-test").unwrap();
        let manager = manager(root.path());
        let (server, mock, task) = fixture(root.path()).await;
        let pid = server.child.id().unwrap();
        let start = manager.start_with_server(server, request()).await.unwrap();
        let id = start["attempt"].as_str().unwrap();
        assert_eq!(start["mode"], "code");
        assert_eq!(manager.status(id).unwrap()["status"], "pending");
        assert_eq!(
            mock.requests.lock().unwrap()[0],
            json!({"methodID":"device","label":"Research"})
        );
        manager.code(id, "test-code".into()).await.unwrap();
        assert_eq!(terminal(&manager, id).await, json!({"status":"complete"}));
        let record: Value =
            serde_json::from_slice(&std::fs::read(&manager.records).unwrap()).unwrap();
        assert_eq!(
            record,
            json!({"xai":{"kind":"oauth","name":"Research","icon":{"brand":true,"color":"gray"}}})
        );
        assert_stopped(pid);
        assert!(DatabaseLease::acquire(&root.path().join("opencode.db"), 2).is_ok());
        assert!(!mock.cancelled.load(Ordering::SeqCst));
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn cancel_and_shutdown_stop_children_without_writing_records() {
        // Another test's forked child can hold an inherited lease descriptor, so this runs alone.
        if !crate::config::in_test_child(&[]) {
            return;
        }
        for shutdown in [false, true] {
            let root = crate::local::git::TemporaryDirectory::new("orx-signin-test").unwrap();
            let manager = manager(root.path());
            let (server, mock, task) = fixture(root.path()).await;
            let pid = server.child.id().unwrap();
            let start = manager.start_with_server(server, request()).await.unwrap();
            let id = start["attempt"].as_str().unwrap();
            if shutdown {
                manager.shutdown().await;
            } else {
                manager.cancel(id).await;
            }
            assert!(manager.status(id).is_none());
            assert!(!manager.records.exists());
            assert_stopped(pid);
            assert!(mock.cancelled.load(Ordering::SeqCst));
            assert!(DatabaseLease::acquire(&root.path().join("opencode.db"), 2).is_ok());
            task.abort();
            let _ = task.await;
        }
    }

    #[tokio::test]
    async fn sign_out_removes_the_integrations_stored_credentials_through_the_api() {
        let root = crate::local::git::TemporaryDirectory::new("orx-signin-test").unwrap();
        let (mut server, mock, task) = fixture(root.path()).await;
        disconnect(&server, "xai").await.unwrap();
        assert_eq!(
            *mock.requests.lock().unwrap(),
            vec![
                json!({"removed":"cred_oauth"}),
                json!({"removed":"cred_key"})
            ]
        );
        server.stop().await;
        task.abort();
        let _ = task.await;
    }

    #[tokio::test]
    async fn expiry_and_child_failure_release_resources() {
        for expired in [true, false] {
            let root = crate::local::git::TemporaryDirectory::new("orx-signin-test").unwrap();
            let manager = manager(root.path());
            let (mut server, mock, task) = fixture(root.path()).await;
            let pid = server.child.id().unwrap();
            if !expired {
                server.child.kill().await.unwrap();
            }
            let expires = if expired {
                crate::store::now_ms() - 1
            } else {
                crate::store::now_ms() + 60000
            };
            let start = manager.attach(server, request(), &json!({"attemptID":"con_test","url":"https://example.test/sign-in","mode":"auto","time":{"expires":expires}})).unwrap();
            let id = start["attempt"].as_str().unwrap();
            assert_eq!(
                terminal(&manager, id).await["status"],
                if expired { "expired" } else { "failed" }
            );
            assert!(!manager.records.exists());
            assert_stopped(pid);
            assert!(mock.cancelled.load(Ordering::SeqCst));
            task.abort();
            let _ = task.await;
        }
    }
}
