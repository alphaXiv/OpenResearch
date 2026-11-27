//! The boxd backend against a fake boxd gRPC API. Each machine's commands run in the local `bash`,
//! with its [`RUN_DIR`] mapped into a temp dir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tonic::{Request, Response, Streaming};

use super::*;

pub(crate) struct State {
    root: PathBuf,
    /// The one API key the fake accepts.
    key: String,
    machines: BTreeMap<String, api::Vm>,
    pub creates: Vec<api::CreateVmRequest>,
    pub destroyed: Vec<String>,
    pub hibernate_off: Vec<String>,
    pub uploads: Vec<(String, Vec<u8>)>,
    /// An exec whose command contains this never answers.
    hang_exec: Option<String>,
}

type Answer<T> = Result<T, Status>;

impl State {
    fn add(&mut self, labels: HashMap<String, String>, vcpu: u32, status: &str) -> String {
        let vm_id = format!("vm-{}", self.machines.len() + self.destroyed.len() + 1);
        std::fs::create_dir_all(self.root.join(&vm_id)).unwrap();
        let (status, region) = (status.into(), "nyc".into());
        let vm = api::Vm {
            vm_id: vm_id.clone(),
            status,
            vcpu,
            labels,
            region,
        };
        self.machines.insert(vm_id.clone(), vm);
        vm_id
    }

    /// Machines are addressed by id only. A new machine runs by its second read.
    fn get(&mut self, request: api::VmId) -> Answer<api::Vm> {
        let missing = || Status::not_found(format!("vm not found: {}", request.vm_id));
        let vm = self.machines.get_mut(&request.vm_id).ok_or_else(missing)?;
        let read = vm.clone();
        if vm.status == "starting" {
            vm.status = "running".into();
        }
        Ok(read)
    }

    fn wake(&mut self, request: api::VmId) -> Answer<api::Empty> {
        self.get(request.clone())?;
        self.machines.get_mut(&request.vm_id).unwrap().status = "running".into();
        Ok(api::Empty {})
    }

    fn destroy(&mut self, request: api::VmId) -> Answer<api::Empty> {
        self.get(request.clone())?;
        self.machines.remove(&request.vm_id);
        self.destroyed.push(request.vm_id);
        Ok(api::Empty {})
    }

    fn create(&mut self, request: api::CreateVmRequest) -> Answer<api::CreateVmResponse> {
        self.creates.push(request.clone());
        if request.name.contains("refuse") {
            return Err(Status::invalid_argument("name is not allowed"));
        }
        let vcpu = request.config.map_or(0, |config| config.vcpu);
        let vm_id = self.add(request.labels, if vcpu == 0 { 2 } else { vcpu }, "starting");
        let machine = Some(self.machines[&vm_id].clone());
        Ok(api::CreateVmResponse { vm_id, machine })
    }

    fn list(&mut self, request: api::ListVmsRequest) -> Answer<api::ListVmsResponse> {
        let labelled = |vm: &&api::Vm| {
            request.label_selector.iter().all(|selector| {
                let (key, value) = selector.split_once('=').unwrap();
                vm.labels.get(key).map(String::as_str) == Some(value)
            })
        };
        let vms = self.machines.values().filter(labelled).cloned().collect();
        Ok(api::ListVmsResponse { vms })
    }

    fn hibernate_off(
        &mut self,
        request: api::SetAutoHibernateTimeoutRequest,
    ) -> Answer<api::Empty> {
        assert_eq!(request.timeout_secs, 0);
        self.hibernate_off.push(request.vm_id);
        Ok(api::Empty {})
    }

    fn defaults(&mut self, _: api::Empty) -> Answer<api::OrgDefaults> {
        let region = "nyc".into();
        Ok(api::OrgDefaults {
            vcpu: 1,
            ceiling_vcpu: 2,
            region,
        })
    }

    /// The local directory standing in for a running machine.
    fn dir(&mut self, vm_id: &str) -> Answer<PathBuf> {
        let status = self
            .get(api::VmId {
                vm_id: vm_id.into(),
            })?
            .status;
        if status != "running" {
            return Err(Status::failed_precondition(format!("vm is {status}")));
        }
        Ok(self.root.join(vm_id))
    }
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn authorize<T>(state: &Shared, request: &Request<T>) -> Answer<()> {
    let state = lock(state);
    let bearer = request
        .metadata()
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    let known = bearer == Some(format!("Bearer {}", state.key).as_str());
    // Echoes what it got, so a test sees whether orx keeps the key out of its errors.
    let unknown = || Status::unauthenticated(format!("API key not recognized: {bearer:?}"));
    known.then_some(()).ok_or_else(unknown)
}

fn unary<Req, Resp>(
    state: &Shared,
    handle: fn(&mut State, Req) -> Answer<Resp>,
) -> axum::routing::MethodRouter
where
    Req: prost::Message + Default + Send + 'static,
    Resp: prost::Message + Send + 'static,
{
    let state = state.clone();
    axum::routing::post(move |request: axum::extract::Request| async move {
        let service = tower::service_fn(move |request: Request<Req>| {
            let answer = authorize(&state, &request)
                .and_then(|()| handle(&mut lock(&state), request.into_inner()));
            std::future::ready(answer.map(Response::new))
        });
        tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
            .unary(service, request)
            .await
    })
}

fn local(dir: &Path, text: &str) -> String {
    text.replace(RUN_DIR, &dir.join("orx").display().to_string())
}

type Chunks = futures::stream::Iter<std::vec::IntoIter<Answer<api::ExecChunk>>>;

/// Runs the command to its end, then answers with its stdout, its stderr and its exit code.
async fn exec(
    state: Shared,
    request: Request<Streaming<api::ExecChunk>>,
) -> Answer<Response<Chunks>> {
    authorize(&state, &request)?;
    let first = request
        .into_inner()
        .next()
        .await
        .ok_or_else(|| Status::invalid_argument("no command"))??;
    let (dir, hang) = {
        let mut state = lock(&state);
        let marker = state.hang_exec.clone();
        (
            state.dir(&first.vm_id)?,
            marker.is_some_and(|marker| first.command.contains(&marker)),
        )
    };
    if hang {
        std::future::pending::<()>().await;
    }
    let output = crate::process::tokio_command("bash")
        .args(["-c", &local(&dir, &first.command)])
        .current_dir(&dir)
        // A machine has none of this process's environment.
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| Status::internal(error.to_string()))?;
    let chunk = |data, is_stderr, exit_code| {
        Ok(api::ExecChunk {
            data,
            is_stderr,
            exit_code,
            ..Default::default()
        })
    };
    let code = output.status.code().unwrap_or(-1);
    let chunks = vec![
        chunk(output.stdout, false, 0),
        chunk(output.stderr, true, 0),
        chunk(Vec::new(), false, code),
    ];
    Ok(Response::new(futures::stream::iter(chunks)))
}

async fn upload(
    state: Shared,
    request: Request<Streaming<api::UploadFileChunk>>,
) -> Answer<Response<api::Empty>> {
    authorize(&state, &request)?;
    let mut stream = request.into_inner();
    let first = stream
        .next()
        .await
        .ok_or_else(|| Status::invalid_argument("no chunk"))??;
    let dir = lock(&state).dir(&first.vm_id)?;
    let mut data = first.data;
    while let Some(chunk) = stream.next().await {
        data.extend(chunk?.data);
    }
    assert_eq!(data.len() as u64, first.total_size);
    // Scripts name the run directory too.
    if let Ok(text) = std::str::from_utf8(&data) {
        data = local(&dir, text).into_bytes();
    }
    std::fs::write(local(&dir, &first.path), &data)
        .map_err(|error| Status::internal(error.to_string()))?;
    lock(&state).uploads.push((first.path, data));
    Ok(Response::new(api::Empty {}))
}

pub(crate) struct Fake {
    pub root: PathBuf,
    state: Shared,
    pub account: Account,
}

impl Fake {
    pub(crate) async fn start() -> Self {
        let root = std::env::temp_dir().join(format!("orx-boxd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let account = Account {
            api_url: Some(format!("http://{}", listener.local_addr().unwrap())),
            ..Account::new(format!("test-key-{}", uuid::Uuid::new_v4().simple()))
        };
        let state: Shared = Arc::new(Mutex::new(State {
            root: root.clone(),
            key: account.api_key.clone(),
            machines: BTreeMap::new(),
            creates: Vec::new(),
            destroyed: Vec::new(),
            hibernate_off: Vec::new(),
            uploads: Vec::new(),
            hang_exec: None,
        }));
        let route = |method: &str| format!("/boxd.api.v1.BoxdApi/{method}");
        let mut grpc = axum::Router::new()
            .route(&route("CreateVm"), unary(&state, State::create))
            .route(&route("GetVm"), unary(&state, State::get))
            .route(&route("DestroyVm"), unary(&state, State::destroy))
            .route(&route("ListVms"), unary(&state, State::list))
            .route(
                &route("SetAutoHibernateTimeout"),
                unary(&state, State::hibernate_off),
            )
            .route(
                &route("GetOrgMachineDefaults"),
                unary(&state, State::defaults),
            );
        for method in ["StartVm", "WakeVm", "ResumeVm"] {
            grpc = grpc.route(&route(method), unary(&state, State::wake));
        }
        let (exec_state, upload_state) = (state.clone(), state.clone());
        let exec = move |request| async move {
            let service = tower::service_fn(move |request| exec(exec_state.clone(), request));
            tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
                .streaming(service, request)
                .await
        };
        let upload = move |request| async move {
            let service = tower::service_fn(move |request| upload(upload_state.clone(), request));
            tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
                .client_streaming(service, request)
                .await
        };
        let grpc = grpc
            .route(
                &route("Exec"),
                axum::routing::post(move |request: axum::extract::Request| exec(request)),
            )
            .route(
                &route("UploadFileStream"),
                axum::routing::post(move |request: axum::extract::Request| upload(request)),
            );
        let incoming = futures::stream::unfold(listener, |listener| async move {
            Some((listener.accept().await.map(|(stream, _)| stream), listener))
        });
        let server =
            tonic::transport::Server::builder().add_routes(tonic::service::Routes::from(grpc));
        tokio::spawn(server.serve_with_incoming(incoming));
        Self {
            root,
            state,
            account,
        }
    }

    /// Points the process env at this fake. Only in a test child process.
    pub(crate) fn configure_env(&self) {
        std::env::set_var(API_KEY, &self.account.api_key);
        std::env::set_var(API_URL, self.account.api_url.as_deref().unwrap());
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    fn run_dir(&self, vm_id: &str) -> PathBuf {
        self.root.join(vm_id).join("orx")
    }

    /// A machine made outside the code under test.
    fn add_machine(&self, run: &str) -> Placed {
        let labels = HashMap::from([("or_run".to_string(), run.to_string())]);
        let vm_id = self.state().add(labels, 2, "running");
        Placed {
            vm_id,
            region: "nyc".into(),
            vcpus: 2,
        }
    }

    fn set_status(&self, vm_id: &str, status: &str) {
        self.state().machines.get_mut(vm_id).unwrap().status = status.into();
    }

    async fn launch(&self, name: &str, script: &str) -> Placed {
        launch(&self.account, &spec(name, script), &|| false)
            .await
            .unwrap()
    }
}

async fn status(account: &Account, placed: &Placed) -> JobState {
    inspect(account, placed).await.unwrap().unwrap()
}

async fn wait(account: &Account, placed: &Placed) -> JobState {
    for _ in 0..200 {
        let state = status(account, placed).await;
        if state.stage != "RUNNING" {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the job did not end");
}

/// The log from `from` on: its bytes, the offset after them, and whether it is whole.
async fn read_log(account: &Account, placed: &Placed, from: u64) -> (Vec<u8>, u64, bool) {
    let (mut offset, mut bytes, mut last) = (from, Vec::new(), from);
    let mut sink = |chunk: &[u8], at| {
        bytes.extend_from_slice(chunk);
        last = at;
    };
    let idle = Duration::from_secs(10);
    let whole = stream_logs(account, placed, &mut offset, idle, &mut sink)
        .await
        .unwrap();
    assert_eq!(last, offset, "each chunk carries the offset after it");
    (bytes, offset, whole)
}

impl Drop for Fake {
    fn drop(&mut self) {
        // Ends any job a test left behind, by its session, then its files.
        let machines = lock(&self.state)
            .machines
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for pid in machines
            .iter()
            .filter_map(|id| std::fs::read_to_string(self.run_dir(id).join("pid")).ok())
        {
            let _ = crate::process::command("pkill")
                .args(["-KILL", "-s", pid.trim()])
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn spec(name: &str, script: &str) -> BoxdJobSpec {
    BoxdJobSpec {
        name: name.into(),
        script: script.into(),
        vcpus: Some(1),
        env: HashMap::new(),
        timeout_seconds: 60,
        region: None,
        labels: HashMap::from([("or_run".to_string(), "run-1".to_string())]),
        source_archive: None,
    }
}

fn state(stage: &'static str, exit_code: Option<i32>) -> JobState {
    JobState {
        stage,
        message: None,
        exit_code,
    }
}

#[test]
fn flavors_sizes_and_prices() {
    let sizes = [None, Some(" CPU "), Some("cpu-1"), Some("4")].map(|f| resolve_flavor(f).unwrap());
    assert_eq!(sizes, [None, None, Some(1), Some(4)]);
    for bad in ["3", "16", "h100", "gpu"] {
        let error = resolve_flavor(Some(bad)).unwrap_err().to_string();
        assert!(error.contains("1, 2, 4 or 8"), "{error}");
    }
    let defaults = api::OrgDefaults {
        ceiling_vcpu: 2,
        ..Default::default()
    };
    assert!(check_size(Some(2), &defaults).is_ok());
    let error = check_size(Some(8), &defaults).unwrap_err().to_string();
    assert!(error.contains("--flavor 2"), "{error}");
    assert_eq!(machine_name("Run_01ABC"), "orx-run-01abc");
    // boxd.sh/pricing: 2 vCPU · 8 GiB at most $0.22/h.
    assert_eq!(hourly_price(2).amount_micros, 215_600);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_launch_makes_one_isolated_machine_and_runs_the_job_from_its_source() {
    let fake = Fake::start().await;
    let source = fake.root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("train.py"), "print('hi')\n").unwrap();
    let archive = fake.root.join("source.tar");
    let mut tar = crate::process::command("tar");
    tar.current_dir(&fake.root)
        .args(["-cf", "source.tar", "-C", "source", "train.py"]);
    assert!(tar.status().unwrap().success());
    let script = "cat train.py; echo err >&2; echo \"$KEEP\"; exit 3";
    let script = crate::compute::gated_script(SOURCE_ARCHIVE, script);
    let mut spec = spec("orx-run-1", &script);
    spec.source_archive = Some(archive);
    spec.env.insert("KEEP".into(), "it's kept".into());
    let placed = launch(&fake.account, &spec, &|| false).await.unwrap();
    assert_eq!((placed.region.as_str(), placed.vcpus), ("nyc", 1));
    {
        let state = fake.state();
        let [create] = state.creates.as_slice() else {
            panic!("{} creates", state.creates.len())
        };
        assert!(create.isolated);
        assert_eq!(create.labels["or_run"], "run-1");
        let config = create.config.unwrap();
        assert_eq!((config.vcpu, config.memory_bytes), (1, 4 << 30));
        assert_eq!(config.srf.unwrap().auto_suspend_timeout_secs, Some(0));
        assert_eq!(state.hibernate_off, std::slice::from_ref(&placed.vm_id));
    }
    let ended = wait(&fake.account, &placed).await;
    assert_eq!(ended, state("COMPLETED", Some(3)));
    let (log, _, whole) = read_log(&fake.account, &placed, 0).await;
    assert!(whole && log == b"print('hi')\nerr\nit's kept\n", "{log:?}");
    // The env file is read once and deleted, and the run directory is the owner's alone.
    let dir = fake.run_dir(&placed.vm_id);
    assert!(!dir.join("env.sh").exists());
    use std::os::unix::fs::PermissionsExt as _;
    let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn logs_follow_the_job_and_resume_at_the_exact_byte() {
    let fake = Fake::start().await;
    let script =
        "echo first; sleep 1; for i in $(seq 200); do printf 'line %03d é\\n' $i; done; printf end";
    let placed = fake.launch("orx-run-2", script).await;
    let (whole, end, closed) = read_log(&fake.account, &placed, 0).await;
    assert!(closed && whole.starts_with(b"first\nline 001") && whole.ends_with(b"end"));
    assert_eq!(end, whole.len() as u64);
    // A restarted orx continues inside a line and inside a multibyte character.
    for cut in [1, 7, 12, 1000, end - 1, end] {
        let (rest, after, closed) = read_log(&fake.account, &placed, cut).await;
        assert!(closed && after == end);
        assert_eq!(rest, whole[cut as usize..], "from {cut}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn cancel_stops_every_process_of_the_job_and_never_hangs() {
    let mut fake = Fake::start().await;
    fake.account.cancel_timeout = Duration::from_secs(2);
    let placed = fake.launch("orx-run-3", "sleep 300 & sleep 300").await;
    let session = std::fs::read_to_string(fake.run_dir(&placed.vm_id).join("pid")).unwrap();
    let pgrep = || {
        crate::process::command("pgrep")
            .args(["-s", session.trim()])
            .output()
    };
    let alive = || pgrep().unwrap().status.success();
    assert!(alive());
    assert_eq!(status(&fake.account, &placed).await, state("RUNNING", None));

    fake.state().hang_exec = Some("pkill -TERM".into());
    let hung = tokio::time::timeout(Duration::from_secs(30), cancel(&fake.account, &placed)).await;
    let error = format!("{:#}", hung.expect("cancel waited forever").unwrap_err());
    assert!(error.contains("took longer than"), "{error}");
    fake.state().hang_exec = None;

    assert!(cancel(&fake.account, &placed).await.unwrap());
    assert_eq!(wait(&fake.account, &placed).await, state("CANCELED", None));
    assert!(!alive(), "a process of the job survived");
    assert!(read_log(&fake.account, &placed, 0).await.2);

    let done = fake.launch("orx-run-4", "exit 0").await;
    wait(&fake.account, &done).await;
    assert!(cancel(&fake.account, &done).await.unwrap());
    let ended = status(&fake.account, &done).await;
    assert_eq!(ended, state("COMPLETED", Some(0)));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_timeout_stops_the_job_and_says_so() {
    let fake = Fake::start().await;
    let mut spec = spec("orx-run-5", "sleep 30");
    spec.timeout_seconds = 1;
    let placed = launch(&fake.account, &spec, &|| false).await.unwrap();
    let ended = wait(&fake.account, &placed).await;
    assert!(ended.stage == "FAILED" && ended.message.unwrap().contains("--timeout"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_failed_launch_leaves_no_machine() {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    // (case, machine name, valid env, cancelled from this check on, an exec that hangs, creates)
    let cases = [
        ("env that can't be sourced", "orx-run", false, 9, None, 0),
        ("cancelled before create", "orx-run", true, 0, None, 0),
        ("create refused", "orx-refuse", true, 9, None, 1),
        ("cancelled after create", "orx-run", true, 1, None, 1),
        ("staging hangs", "orx-run", true, 9, Some("mkdir -m 700"), 1),
    ];
    for (case, name, valid_env, cancel_from, hang, creates) in cases {
        let mut fake = Fake::start().await;
        fake.account.stage_timeout = Duration::from_secs(2);
        fake.state().hang_exec = hang.map(Into::into);
        let mut spec = spec(name, "true");
        if !valid_env {
            spec.env.insert("NOT A NAME".into(), "x".into());
        }
        let checks = AtomicUsize::new(0);
        let cancelled = || checks.fetch_add(1, SeqCst) >= cancel_from;
        let launched = tokio::time::timeout(
            Duration::from_secs(30),
            launch(&fake.account, &spec, &cancelled),
        );
        let error = launched.await.expect(case).unwrap_err();
        let state = fake.state();
        assert_eq!(state.creates.len(), creates, "{case}");
        assert!(state.machines.is_empty(), "{case}: a machine was left");
        let refused = error.downcast_ref::<crate::jobs::Refused>().is_some();
        assert!(creates == 0 || refused, "{case}: {error:#}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn finish_and_recovery_find_machines_by_id_and_run_label() {
    let fake = Fake::start().await;
    let placed = fake.launch("orx-run-6", "true").await;
    let other = fake.add_machine("run-b");
    let spec = spec("orx-run-6", "true");
    let found = find_run(&fake.account, &spec, "run-1").await.unwrap();
    assert_eq!(found, std::slice::from_ref(&placed));
    assert_eq!(find_run(&fake.account, &spec, "run-c").await.unwrap(), []);
    finish(&fake.account, &placed).await.unwrap();
    finish(&fake.account, &placed).await.unwrap();
    assert_eq!(fake.state().destroyed, std::slice::from_ref(&placed.vm_id));
    assert!(fake.state().machines.contains_key(&other.vm_id));
    assert_eq!(inspect(&fake.account, &placed).await.unwrap(), None);
    assert!(!cancel(&fake.account, &placed).await.unwrap());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn sleeping_failed_and_never_started_machines_end_their_runs() {
    let fake = Fake::start().await;
    let message = |state: JobState| state.message.unwrap_or_default();
    let placed = fake.launch("orx-run-11", "echo done; exit 5").await;
    wait(&fake.account, &placed).await;
    fake.set_status(&placed.vm_id, "hibernated");
    let ended = status(&fake.account, &placed).await;
    assert_eq!(ended, state("COMPLETED", Some(5)));

    // A failed machine runs no command: its log ends with a note, so the run can be cleaned up.
    fake.set_status(&placed.vm_id, "failed");
    assert!(message(status(&fake.account, &placed).await).contains("is failed"));
    let (log, end, whole) = read_log(&fake.account, &placed, 5).await;
    assert!(whole && end == 5 && String::from_utf8_lossy(&log).contains("log is unavailable"));
    finish(&fake.account, &placed).await.unwrap();
    let (mut offset, idle) = (5, Duration::from_secs(10));
    let mut sink = |_: &[u8], _| {};
    let unseen = stream_logs(&fake.account, &placed, &mut offset, idle, &mut sink).await;
    assert!(unseen.is_err(), "this key may not see a live machine");

    // Found by its label after a launch stopped before the job started.
    let bare = fake.add_machine("run-12");
    let staged = fake.add_machine("run-13");
    std::fs::create_dir_all(fake.run_dir(&staged.vm_id)).unwrap();
    for placed in [bare, staged] {
        assert!(message(status(&fake.account, &placed).await).contains("never started"));
        let log = read_log(&fake.account, &placed, 0).await;
        assert_eq!(log, (Vec::new(), 0, true));
    }
}

#[tokio::test]
async fn a_rejected_key_is_never_sent_again() {
    let fake = Fake::start().await;
    let client = fake.account.client("").unwrap();
    client.org_defaults().await.unwrap();
    fake.state().key = "revoked-key".into();
    let error = client.org_defaults().await.unwrap_err();
    assert!(error.downcast_ref::<AuthRejected>().is_some(), "{error}");
    assert!(!error.to_string().contains("test-key-"), "{error}");
    // boxd would take the key again now, but nothing asks it until a user checks the key.
    fake.state().key = fake.account.api_key.clone();
    let error = client.org_defaults().await.unwrap_err();
    assert!(error.downcast_ref::<AuthRejected>().is_some(), "{error}");
    client.check().await.unwrap();
    client.org_defaults().await.unwrap();

    let unreachable = Account {
        api_url: Some("http://127.0.0.1:9".into()),
        ..Account::new("unreachable-key".into())
    };
    let client = unreachable.client("").unwrap();
    let error = client.org_defaults().await.unwrap_err();
    assert!(error.downcast_ref::<AuthRejected>().is_none(), "{error}");
}

#[tokio::test]
async fn settings_save_only_a_key_boxd_accepts() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    let fake = Fake::start().await;
    std::env::set_var(API_URL, fake.account.api_url.as_deref().unwrap());
    use crate::commands::up::compute_settings::configure;
    let error = configure("boxd", serde_json::json!({"key": "wrong-key"}))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Nothing was saved"), "{error}");
    assert_eq!(crate::config::synced_env_var(API_KEY), None);
    let key = fake.account.api_key.clone();
    let saved = configure("boxd", serde_json::json!({ "key": key }))
        .await
        .unwrap();
    assert_eq!(saved["validationStatus"], "valid");
    assert_eq!(crate::config::synced_env_var(API_KEY), Some(key.clone()));
    assert!(!saved.to_string().contains(&key), "the key is masked");
}

/// One real run on boxd, start to finish: `ORX_BOXD_E2E=1 BOXD_API_KEY=… cargo test --bin orx
/// live_boxd_run -- --ignored`. `ORX_BOXD_REGION` and `ORX_BOXD_NAME_PREFIX` apply as they do for
/// `orx exp run`. It makes one machine and deletes it.
#[tokio::test]
#[ignore = "makes a real boxd machine; run with ORX_BOXD_E2E=1 and BOXD_API_KEY"]
async fn live_boxd_run() {
    assert_eq!(std::env::var("ORX_BOXD_E2E").as_deref(), Ok("1"));
    let account = Account::from_env().unwrap();
    let run_id = format!("e2e-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let script = "set -e; head -c 5242880 /dev/urandom > blob; sha256sum blob; nproc; echo done";
    let mut spec = spec(&machine_name(&run_id), script);
    spec.region = configured_region().unwrap().or(Some("nyc".into()));
    spec.labels.insert("or_run".into(), run_id);
    let placed = launch(&account, &spec, &|| false).await.unwrap();
    eprintln!("launched machine {}", placed.vm_id);
    let ended = wait(&account, &placed).await;
    let (log, _, whole) = read_log(&account, &placed, 0).await;
    let log = String::from_utf8_lossy(&log).into_owned();
    finish(&account, &placed).await.unwrap();
    assert_eq!(ended, state("COMPLETED", Some(0)), "{log}");
    assert!(
        whole && log.contains("blob") && log.ends_with("1\ndone\n"),
        "{log}"
    );
    assert_eq!(inspect(&account, &placed).await.unwrap(), None);
}
