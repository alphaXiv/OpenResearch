use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::{extract::State, routing::any, Json, Router};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{hf::HfAccount, k8s::K8sAccount, modal::ModalAccount, ray::RayAccount};
use crate::engine::clock::TestClock;
use crate::engine::*;
use crate::jobs::BackendDescriptor;
use crate::local::model::LocalExperiment;
use crate::store::{LaunchSpec, Store, SubmittingRun};

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    orx_test_support::write_executable(path, body, 0o700).unwrap();
}

pub(super) fn engine(root: &Path) -> Engine {
    let store = Store::open_at(root.join("store")).unwrap();
    store
        .create_local_experiment(&LocalExperiment {
            id: "exp".into(),
            project_id: "proj".into(),
            parent_experiment_id: None,
            slug: "exp".into(),
            branch_name: "orx/exp".into(),
            title: None,
            description: None,
            run_command: "true".into(),
            agent_status: "idle".into(),
            created_at: 1,
            updated_at: 1,
            chat_session_id: None,
            archived: false,
        })
        .unwrap();
    Engine::new(store, Arc::new(TestClock::new()))
}

fn pending<'a>(
    id: &'a str,
    metadata: &'a BackendDescriptor,
    launch: &'a LaunchSpec,
) -> SubmittingRun<'a> {
    SubmittingRun {
        id,
        experiment_id: "exp",
        project_id: "proj",
        metadata,
        command: "true",
        created_at: 1,
        commit_sha: Some("snapshot"),
        chat_session_id: None,
        spec_digest: "digest",
        launch,
    }
}

/// Serves the HF log stream for the next connection from `sse/<n>`, recording each query in
/// `sse.log`; a missing script answers 503. Each script line is sent as is, except `!keepalive`,
/// which sends keep-alive comments and no data from then on.
fn sse(root: &Path, query: &str) -> axum::response::Response {
    use axum::response::IntoResponse;
    use std::io::Write;
    let log = root.join("sse.log");
    let connection = std::fs::read_to_string(&log).map_or(0, |log| log.lines().count());
    let mut requests = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .unwrap();
    writeln!(requests, "{query}").unwrap();
    let Ok(script) = std::fs::read_to_string(root.join("sse").join(connection.to_string())) else {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "unavailable").into_response();
    };
    let lines: std::collections::VecDeque<String> = script.lines().map(str::to_owned).collect();
    let body = futures::stream::unfold(lines, |mut lines| async move {
        let line = match lines.front()?.as_str() {
            "!keepalive" => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                ": keep-alive\n".to_owned()
            }
            _ => format!("{}\n", lines.pop_front()?),
        };
        Some((Ok::<_, std::io::Error>(line), lines))
    });
    axum::body::Body::from_stream(body).into_response()
}

/// An SSE script of HF log events, each `(timestamp, data)`.
fn hf_events(events: &[(&str, &str)]) -> String {
    events
        .iter()
        .map(|(timestamp, data)| {
            format!(
                "data: {}\n\n",
                json!({"data": data, "timestamp": timestamp})
            )
        })
        .collect()
}

/// Runs the calling test alone in a child process, against a fake provider API
/// and with fake provider CLIs first on `PATH`. `None` in the parent.
async fn isolated() -> Option<PathBuf> {
    if !crate::config::in_test_child(&["SSH_AUTH_SOCK"]) {
        return None;
    }
    let root = std::env::temp_dir().join("orx-api");
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    async fn lookup(
        State(root): State<PathBuf>,
        method: axum::http::Method,
        uri: axum::http::Uri,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        use std::io::Write;
        let mut requests = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(root.join("http.log"))
            .unwrap();
        writeln!(requests, "{method} {}", uri.path()).unwrap();
        let headers = axum::http::HeaderMap::new();
        if uri.path().ends_with("/logs") && root.join("sse").is_dir() {
            return sse(&root, uri.query().unwrap_or_default());
        }
        if uri.path().ends_with("/logs") && root.join("slow-logs").exists() {
            tokio::time::sleep(Duration::from_secs(5)).await;
            return (headers, Json(json!({}))).into_response();
        }
        if method == axum::http::Method::POST && uri.path() == "/sandboxes" {
            std::fs::write(root.join("sandbox-request.json"), body).unwrap();
            let body = std::fs::read(root.join("sandbox-response.json")).unwrap();
            return (
                headers,
                Json(serde_json::from_slice::<Value>(&body).unwrap()),
            )
                .into_response();
        }
        if method == axum::http::Method::GET && uri.path() == "/orgs" {
            let orgs = std::fs::read(root.join("orgs.json")).unwrap_or_else(|_| {
                br#"{"orgs":[{"id":"org","name":"Org","createdBy":"u"}]}"#.into()
            });
            return (
                headers,
                Json(serde_json::from_slice::<Value>(&orgs).unwrap()),
            )
                .into_response();
        }
        if method == axum::http::Method::GET && uri.path() == "/users/me" {
            return (
                headers,
                Json(json!({"user": {"id": "u", "email": "other@example.com"}})),
            )
                .into_response();
        }
        if method == axum::http::Method::GET && uri.path() == "/api/whoami-v2" {
            let whoami = std::fs::read(root.join("whoami.json"))
                .unwrap_or_else(|_| br#"{"name":"me","orgs":[{"name":"org"}]}"#.into());
            return (
                headers,
                Json(serde_json::from_slice::<Value>(&whoami).unwrap()),
            )
                .into_response();
        }
        if method == axum::http::Method::GET && uri.path() == "/ssh-keys" {
            if let Ok(keys) = std::fs::read(root.join("ssh-keys.json")) {
                return (
                    headers,
                    Json(serde_json::from_slice::<Value>(&keys).unwrap()),
                )
                    .into_response();
            }
        }
        if uri.path().starts_with("/sandboxes/") {
            if method == axum::http::Method::GET {
                if root.join("box-released").exists() {
                    return (axum::http::StatusCode::NOT_FOUND, Json(json!({}))).into_response();
                }
                if root.join("lookup-fails").exists() {
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
                        .into_response();
                }
                let mut sandbox = json!({
                    "id":"box", "organizationId":"org", "status":"online",
                    "machineType":"cpu", "updatedAt":"now"
                });
                if let Ok(fields) = std::fs::read(root.join("sandbox.json")) {
                    let fields: serde_json::Map<String, Value> =
                        serde_json::from_slice(&fields).unwrap();
                    sandbox.as_object_mut().unwrap().extend(fields);
                }
                return (headers, Json(json!({ "sandbox": sandbox }))).into_response();
            }
            if (method == axum::http::Method::DELETE || uri.path().ends_with("/lease/release"))
                && root.join("delete-fails").exists()
            {
                return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
                    .into_response();
            }
            return (headers, Json(json!({}))).into_response();
        }
        if method == axum::http::Method::POST
            && (uri.path().ends_with("/cancel") || uri.path().ends_with("/stop"))
        {
            let status = std::fs::read_to_string(root.join("cancel-status"))
                .map_or(200, |status| status.parse().unwrap());
            return (
                axum::http::StatusCode::from_u16(status).unwrap(),
                Json(json!({})),
            )
                .into_response();
        }
        assert_eq!(
            method,
            axum::http::Method::GET,
            "recovery must never create a job"
        );
        if let Ok(body) = std::fs::read(root.join("inspect-response.json")) {
            let status = std::fs::read_to_string(root.join("inspect-status"))
                .unwrap_or_else(|_| "200".into())
                .parse::<u16>()
                .unwrap();
            return (
                axum::http::StatusCode::from_u16(status).unwrap(),
                Json(serde_json::from_slice::<Value>(&body).unwrap()),
            )
                .into_response();
        }
        let jobs = if root.join("created").exists() {
            json!([{"id":"hf-1", "status":{"stage":"RUNNING"}, "labels":{"or_run":"run"}}])
        } else {
            json!([])
        };
        let mut headers = axum::http::HeaderMap::new();
        if uri.path().starts_with("/api/jobs/")
            && !uri.query().unwrap_or_default().contains("cursor=next")
        {
            headers.insert(
                "link",
                "</api/jobs/org?cursor=next>; rel=\"next\"".parse().unwrap(),
            );
            return (headers, Json(json!([]))).into_response();
        }
        (headers, Json(jobs)).into_response()
    }
    let app = Router::new().fallback(any(lookup)).with_state(root.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let path = format!(
        "{}:{}",
        root.join("bin").display(),
        std::env::var("PATH").unwrap()
    );
    std::env::set_var("PATH", path);
    std::env::set_var("ORX_API_ROOT", &root);
    std::env::set_var("ORX_DATA_DIR", root.join("store"));
    std::env::set_var("HF_ENDPOINT", endpoint);
    std::env::set_var("HF_TOKEN", "fake");
    std::env::set_var("ORX_MODAL_PYTHON", root.join("bin/modal-python"));
    std::env::set_var("ORX_VERCEL_PYTHON", root.join("bin/vercel-python"));
    Some(root)
}

#[tokio::test]
async fn modal_interrupted_before_id_is_unreconciled() {
    let root = std::env::temp_dir().join(format!("orx-modal-{}", uuid::Uuid::new_v4()));
    let engine = engine(&root);
    let metadata = BackendDescriptor::parse(r#"{"kind":"modal_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "modal_job".into(),
        target: None,
        options: json!({}),
    };
    engine
        .store()
        .insert_submitting_run(pending("run", &metadata, &launch))
        .unwrap();
    engine
        .store()
        .set_submission_fence(
            "run",
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let adapter = super::api::Adapter::new(
        ModalAccount,
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    engine.recover(&adapter, "run").await.unwrap();
    assert_eq!(engine.load("run").unwrap().0.status, "submitting");
    assert!(matches!(
        engine.load("run").unwrap().1.contact,
        crate::store::Contact::Unreconciled { .. }
    ));
    drop(engine);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn hf_live_launcher_is_fenced_before_recovery_and_never_created_twice() {
    let Some(root) = isolated().await else {
        return;
    };
    let python = crate::config::config_dir().join("envs/huggingface/bin/python");
    script(
        &python,
        &format!(
            r#"#!/bin/sh
case "$2" in *'assert hasattr'*) exit 0;; esac
cat > '{root}/submission.json'
printf 'created\n' >> '{root}/created'
exec sleep 120
"#,
            root = root.display()
        ),
    );
    let archive = root.join("source.tar");
    std::fs::write(&archive, "snapshot").unwrap();
    let digest = format!("{:x}", Sha256::digest(b"snapshot"));
    let metadata = BackendDescriptor::parse(
        &json!({ "kind":"hf_job", "sourcePath":archive, "sourceDigest":digest, "sourceSize":8 })
            .to_string(),
    )
    .unwrap();
    let launch = LaunchSpec {
        kind: "hf_job".into(),
        target: Some("org".into()),
        options: json!({
            "command":["true"], "docker_image":"python", "flavor":"cpu", "environment":{}, "secrets":{}, "timeout_seconds":3600, "labels":{}
        }),
    };
    let engine = engine(&root);
    let adapter = super::api::Adapter::new(
        HfAccount {
            namespace: "org".into(),
        },
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    let identity = {
        let submit = engine.submit(&adapter, pending("run", &metadata, &launch));
        tokio::pin!(submit);
        tokio::select! {
            result = &mut submit => panic!("launcher returned: {result:?}"),
            identity = async {
                loop {
                    if root.join("created").exists() {
                        break engine.load("run").unwrap().1.submission_fence.unwrap().submitter.unwrap();
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } => identity,
        }
    };
    assert_eq!(
        crate::engine::process::identity(identity.pid),
        Some(identity.clone())
    );
    drop(engine);
    let engine = Engine::new(
        Store::open_at(root.join("store")).unwrap(),
        Arc::new(TestClock::new()),
    );
    engine.recover(&adapter, "run").await.unwrap();
    assert_eq!(engine.load("run").unwrap().0.status, "starting");
    assert_eq!(engine.load("run").unwrap().1.submission_attempt, 1);
    assert!(crate::engine::process::identity(identity.pid).is_none());
    let submitted: Value =
        serde_json::from_slice(&std::fs::read(root.join("submission.json")).unwrap()).unwrap();
    assert_eq!(submitted["labels"]["or_run"], "run");
    assert_eq!(
        std::fs::read_to_string(root.join("created"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn ray_submission_id_survives_lost_response_and_observations_are_batched() {
    use std::sync::Mutex;
    type Context = (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Value>>);
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let response = Arc::new(Mutex::new(json!({"status":"RUNNING", "start_time":1234})));
    async fn submit(
        State((requests, _)): State<Context>,
        Json(body): Json<Value>,
    ) -> axum::http::StatusCode {
        requests.lock().unwrap().push(body);
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    }
    async fn inspect(State((_, response)): State<Context>) -> Json<Value> {
        Json(response.lock().unwrap().clone())
    }
    let app = Router::new()
        .route("/api/jobs/", axum::routing::post(submit))
        .route("/api/jobs/run", axum::routing::get(inspect))
        .with_state((requests.clone(), response.clone()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = std::env::temp_dir().join(format!("orx-ray-{}", uuid::Uuid::new_v4()));
    let engine = engine(&root);
    let metadata = BackendDescriptor::parse(r#"{"kind":"ray_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "ray_job".into(),
        target: Some(address.clone()),
        options: json!({
            "entrypoint":"true", "submission_id":"wrong", "resources":{"cpus":1.0,"gpus":0.0,"memory_bytes":null}, "env":{}, "metadata":{}, "working_dir":null
        }),
    };
    let clock = Arc::new(TestClock::new());
    let adapter =
        super::api::Adapter::new(RayAccount { address }, clock.clone(), root.join("spools"));
    engine
        .submit(&adapter, pending("run", &metadata, &launch))
        .await
        .unwrap();
    assert_eq!(engine.load("run").unwrap().0.status, "starting");
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(requests.lock().unwrap()[0]["submission_id"], "run");
    let runs = [WatchedRun {
        id: "run".into(),
        handle: handle_of(&engine.load("run").unwrap().0).unwrap(),
    }];
    assert_eq!(
        adapter.observe(&runs, Duration::from_secs(2)).await,
        vec![
            (
                "run".into(),
                Observation::Running {
                    started_at: Some(1234)
                }
            ),
            ("run".into(), Observation::Log { len: 0 })
        ]
    );
    assert!(adapter
        .observe(&runs, Duration::from_secs(2))
        .await
        .is_empty());
    clock.advance(Duration::from_secs(10));
    assert_eq!(
        adapter.observe(&runs, Duration::from_secs(2)).await.len(),
        2
    );
    *response.lock().unwrap() = json!({"status":"SUCCEEDED", "start_time":1234, "end_time":4567});
    clock.advance(Duration::from_secs(10));
    let observations = adapter.observe(&runs, Duration::from_secs(2)).await;
    assert!(observations.iter().any(|(_, observation)| matches!(
        observation,
        Observation::Exited {
            finished_at: Some(4567),
            ..
        }
    )));
    for (id, observation) in observations {
        engine.observe(&id, observation).unwrap();
    }
    assert_eq!(engine.load("run").unwrap().0.ended_at, Some(4567));
    server.abort();
    drop(engine);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn provider_terminal_observations_keep_source_finish_times_and_exit_codes() {
    for (stage, code, expected) in [
        (
            "FAILED",
            Some(17),
            Observation::Exited {
                finished_at: Some(123),
                code: Some(17),
                signal: None,
            },
        ),
        (
            "CANCELLED",
            Some(137),
            Observation::Cancelled {
                finished_at: Some(123),
                code: Some(137),
                signal: None,
            },
        ),
        (
            "FAILED",
            None,
            Observation::Failed {
                finished_at: Some(123),
                reason: "FAILED".into(),
            },
        ),
    ] {
        assert_eq!(
            super::api::stage(stage, None, code, Some(100), Some(123)),
            expected
        );
    }
}

#[tokio::test]
async fn k8s_recovery_resumes_staging_and_ready_is_immutable() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/kubectl"),
        r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root=Path(os.environ['ORX_API_ROOT'])
a=sys.argv[1:]
with (root/'kubectl.log').open('a') as f: f.write(' '.join(a)+'\n')
if '--dry-run=client' in a:
    print(sys.stdin.read())
elif a[0]=='get' and a[1]=='pods':
    print(json.dumps({'items':[{'metadata':{'name':'pod-1','annotations':{'kubectl.kubernetes.io/default-container':'run'}},'status':{'phase':'Running','containerStatuses':[{'name':'run','state':{'running':{'startedAt':'2026-01-01T00:00:00Z'}}}]}}]}))
elif a[0]=='get' and a[1]=='job' and a[2]!='-n' and (root/'finished').exists():
    print('{"status":{"succeeded":1}}')
elif a[0]=='get' and a[1]=='job' and a[2]!='-n':
    print('{"spec":{"activeDeadlineSeconds":3600}}')
elif a[0]=='get':
    print((root/'resources.json').read_text())
elif a[0]=='exec' and 'test' in a:
    sys.exit(0 if (root/'ready').exists() else 1)
elif a[0]=='cp':
    (root/'part').write_bytes(Path(a[1]).read_bytes())
    with (root/'copies').open('a') as f: f.write('copy\n')
elif a[0]=='exec' and 'sh' in a:
    if not (root/'ready').exists():
        assert (root/'part').stat().st_size==8
        (root/'part').rename(root/'source.tar')
        (root/'ready').touch()
elif a[0]=='delete':
    (root/'resources.json').write_text('{"items":[]}')
else:
    raise Exception(a)
"#,
    );
    let manifest = json!({"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"run"},"spec":{"template":{"spec":{"restartPolicy":"Never","containers":[{"name":"run","image":"python","command":["bash","-c","eval \"$ORX_SCRIPT\""]}]}}}});
    std::fs::write(
        root.join("resources.json"),
        json!({"items":[manifest.clone()]}).to_string(),
    )
    .unwrap();
    let archive = root.join("archive.tar");
    std::fs::write(&archive, "snapshot").unwrap();
    // Simulate a crash after create, with an incomplete staging copy.
    std::fs::write(root.join("part"), "snap").unwrap();
    let engine = engine(&root);
    let metadata = BackendDescriptor::parse(r#"{"kind":"k8s_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "k8s_job".into(),
        target: Some("default".into()),
        options: json!({
            "manifest":manifest.to_string(), "script":"true", "run_token":"run", "env":{}, "timeout_seconds":3600, "labels":{}, "pod_env":[], "source_archive":archive
        }),
    };
    engine
        .store()
        .insert_submitting_run(pending("run", &metadata, &launch))
        .unwrap();
    engine
        .store()
        .set_submission_fence(
            "run",
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let adapter = |clock: &Arc<TestClock>| {
        super::api::Adapter::new(
            K8sAccount::new(None, "default".into()),
            clock.clone(),
            root.join("spools"),
        )
    };
    let clock = Arc::new(TestClock::new());
    let first = adapter(&clock);
    engine.recover(&first, "run").await.unwrap();
    assert_eq!(engine.load("run").unwrap().0.status, "starting");
    let run = WatchedRun {
        id: "run".into(),
        handle: handle_of(&engine.load("run").unwrap().0).unwrap(),
    };
    let budget = Duration::from_secs(10);
    engine
        .observe_runs(&first, std::slice::from_ref(&run), budget)
        .await;
    assert_eq!(std::fs::read(root.join("source.tar")).unwrap(), b"snapshot");
    engine
        .observe_runs(&adapter(&clock), std::slice::from_ref(&run), budget)
        .await;
    assert_eq!(
        std::fs::read_to_string(root.join("copies")).unwrap(),
        "copy\n"
    );
    let adapter = first;
    let state = engine.load("run").unwrap().1;
    let mut intent = RunIntent::new(engine.load("run").unwrap().0, &state).unwrap();
    assert!(matches!(
        adapter.recover(&intent).await,
        Recovery::Found(Handle::Api { .. })
    ));
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(
        !calls
            .lines()
            .any(|call| call.starts_with("create -f") || call.starts_with("delete")),
        "{calls}"
    );
    let mut primary = manifest.clone();
    primary["metadata"]["name"] = json!("primary");
    primary["metadata"]["labels"] = json!({"orx-primary":"true"});
    let items = json!([manifest, primary, {"apiVersion":"v1","kind":"Service","metadata":{"name":"rendezvous"},"spec":{"clusterIP":"None"}}]);
    intent.launch.options["manifest"] = json!({"kind":"List","items":items}).to_string().into();
    std::fs::write(
        root.join("resources.json"),
        json!({"items":items}).to_string(),
    )
    .unwrap();
    let Recovery::Found(Handle::Api { ids: resources, .. }) = adapter.recover(&intent).await else {
        panic!("primary Job was not recovered");
    };
    assert_eq!(resources[0], "job/primary");
    std::fs::write(root.join("finished"), "").unwrap();
    let before = std::fs::read_to_string(root.join("kubectl.log"))
        .unwrap()
        .lines()
        .count();
    let recovered = adapter.recover(&intent).await;
    assert!(matches!(recovered, Recovery::Found(_)), "{recovered:?}");
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(!calls
        .lines()
        .skip(before)
        .any(|call| call.starts_with("exec") || call.starts_with("cp")));
    let calls_before = std::fs::read_to_string(root.join("kubectl.log"))
        .unwrap()
        .lines()
        .count();
    std::fs::write(
        root.join("resources.json"),
        json!({"items":[{"kind":"Service","metadata":{"name":"rendezvous"}}]}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        adapter.recover(&intent).await,
        Recovery::Unknown { .. }
    ));
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(!calls
        .lines()
        .skip(calls_before)
        .any(|call| call.starts_with("delete")));
    let partial = json!({"items":[{"kind":"Job","metadata":{"name":"primary"}}]});
    std::fs::write(root.join("resources.json"), partial.to_string()).unwrap();
    assert!(matches!(adapter.recover(&intent).await, Recovery::Absent));
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(calls.contains("delete job/primary -n default --cascade=foreground --wait=false"));
}

/// A cluster whose one Job's pod is whatever `pod.json` says; `cp` fails as a real API server
/// does until the pod is on a node. The Job started at 2026-01-01T00:00:00Z and fails once `now`
/// (seconds since then) reaches its deadline.
const WAITING_KUBECTL: &str = r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root = Path(os.environ['ORX_API_ROOT'])
a = sys.argv[1:]
with (root/'kubectl.log').open('a') as f: f.write(' '.join(a)+'\n')
pod = json.loads((root/'pod.json').read_text())
pod['metadata'] = {'name': 'run-pod', 'annotations': {'kubectl.kubernetes.io/default-container': 'run'}}
deleted = (root/'deleted').exists()
job_path = root/'job.json'
job = json.loads(job_path.read_text()) if job_path.exists() else {'spec': {}}
if '--dry-run=client' in a:
    print(sys.stdin.read())
elif a[0] == 'apply':
    sys.stdin.read()
elif a[0] == 'create':
    doc = json.loads(sys.stdin.read())
    if doc['kind'] == 'Job':
        job_path.write_text(json.dumps(doc))
elif a[0] == 'patch':
    if (root/'patch-forbidden').exists():
        print('Error from server (Forbidden): jobs.batch "run" is forbidden: User "lab" cannot patch resource "jobs" in API group "batch" in the namespace "default"', file=sys.stderr)
        sys.exit(1)
    job['spec'].update(json.loads(a[a.index('-p') + 1])['spec'])
    job_path.write_text(json.dumps(job))
elif a[0] == 'delete':
    (root/'deleted').touch()
elif a[:2] == ['get', 'job']:
    if deleted:
        print('Error from server (NotFound): jobs.batch "run" not found', file=sys.stderr)
        sys.exit(1)
    now = int((root/'now').read_text()) if (root/'now').exists() else 0
    deadline = job['spec'].get('activeDeadlineSeconds')
    status = {'startTime': '2026-01-01T00:00:00Z'}
    if deadline is not None and now >= deadline:
        status['conditions'] = [{'type': 'Failed', 'status': 'True', 'reason': 'DeadlineExceeded',
                                 'message': 'Job was active longer than specified deadline'}]
    else:
        status['active'] = 1
    print(json.dumps({'spec': job['spec'], 'status': status}))
elif a[:2] == ['get', 'pods'] and a[-1] == 'json':
    print(json.dumps({'items': [] if deleted else [pod]}))
elif a[:2] == ['get', 'pods']:
    print('' if deleted else 'run-pod', end='')
elif a[:2] == ['get', 'pod']:
    print(json.dumps(pod))
elif a[:2] == ['get', 'events']:
    path = root/'events.json'
    print(path.read_text() if path.exists() else '{"items":[]}')
elif a[0] == 'exec' and 'test' in a:
    sys.exit(0 if (root/'ready').exists() else 1)
elif a[0] == 'cp':
    if not pod['spec'].get('nodeName'):
        print('Error from server (BadRequest): pod run-pod does not have a host assigned', file=sys.stderr)
        sys.exit(1)
    (root/'source.tar').write_bytes(Path(a[1]).read_bytes())
elif a[0] == 'exec' and 'sh' in a:
    (root/'ready').touch()
else:
    raise Exception(a)
"#;

fn waiting_pod(root: &Path, node: Option<&str>, status: Value) {
    let pod = json!({"spec": {"nodeName": node}, "status": status});
    std::fs::write(root.join("pod.json"), pod.to_string()).unwrap();
}

fn script_waiting(reason: &str, message: &str) -> Value {
    json!({"phase": "Pending", "containerStatuses": [
        {"name": "run", "image": "ghcr.io/lab/train:v1",
         "state": {"waiting": {"reason": reason, "message": message}}}
    ]})
}

/// `Failed` events kubelet recorded for the pod, as (container field path, time, message).
fn pull_events(root: &Path, events: &[(&str, &str, &str)]) {
    let items: Vec<Value> = events
        .iter()
        .map(|(field, at, message)| {
            json!({"reason": "Failed", "lastTimestamp": at, "message": message,
                "involvedObject": {"kind": "Pod", "name": "run-pod", "fieldPath": field}})
        })
        .collect();
    std::fs::write(
        root.join("events.json"),
        json!({"items": items}).to_string(),
    )
    .unwrap();
}

fn script_running(started_at: &str) -> Value {
    json!({"phase": "Running", "containerStatuses": [
        {"name": "run", "state": {"running": {"startedAt": started_at}}}
    ]})
}

/// Submits a k8s run against [`WAITING_KUBECTL`] and returns its engine, adapter and handle.
async fn waiting_run(
    root: &Path,
) -> (
    Engine,
    super::api::Adapter<K8sAccount>,
    Arc<TestClock>,
    WatchedRun,
) {
    script(&root.join("bin/kubectl"), WAITING_KUBECTL);
    let archive = root.join("archive.tar");
    std::fs::write(&archive, "snapshot").unwrap();
    let manifest = json!({"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"run"},"spec":{"template":{"spec":{"restartPolicy":"Never","containers":[{"name":"run","image":"ghcr.io/lab/train:v1","command":["bash","-c","eval \"$ORX_SCRIPT\""]}]}}}});
    let metadata = BackendDescriptor::parse(r#"{"kind":"k8s_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "k8s_job".into(),
        target: Some("default".into()),
        options: json!({
            "manifest":manifest.to_string(), "script":"true", "run_token":"run", "env":{}, "timeout_seconds":3600, "labels":{}, "pod_env":[], "source_archive":archive
        }),
    };
    let engine = engine(root);
    let clock = Arc::new(TestClock::new());
    let adapter = super::api::Adapter::new(
        K8sAccount::new(None, "default".into()),
        clock.clone(),
        root.join("spools"),
    );
    engine
        .submit(&adapter, pending("run", &metadata, &launch))
        .await
        .unwrap();
    let run = WatchedRun {
        id: "run".into(),
        handle: handle_of(&engine.load("run").unwrap().0).expect("the launch recorded no handle"),
    };
    (engine, adapter, clock, run)
}

#[tokio::test]
async fn k8s_waiting_pod_stays_queued_and_receives_the_source_once_it_runs() {
    let Some(root) = isolated().await else {
        return;
    };
    waiting_pod(
        &root,
        None,
        json!({"phase": "Pending", "conditions": [{"type": "PodScheduled", "status": "False",
            "reason": "Unschedulable", "message": "0/2 nodes are available: 2 Insufficient nvidia.com/gpu."}]}),
    );
    let (engine, adapter, clock, run) = waiting_run(&root).await;
    let starting = |engine: &Engine| {
        let (row, state) = engine.load("run").unwrap();
        assert_eq!(row.status, "starting");
        assert_eq!(state.contact, crate::store::Contact::Live);
    };
    starting(&engine);

    let budget = Duration::from_secs(10);
    engine
        .observe_runs(&adapter, std::slice::from_ref(&run), budget)
        .await;
    starting(&engine);

    waiting_pod(
        &root,
        Some("gpu-1"),
        script_waiting("ContainerCreating", ""),
    );
    clock.advance(budget);
    engine
        .observe_runs(&adapter, std::slice::from_ref(&run), budget)
        .await;
    starting(&engine);
    assert!(!root.join("source.tar").exists());

    // orx up restarts before the pod runs.
    let adapter = super::api::Adapter::new(
        K8sAccount::new(None, "default".into()),
        clock.clone(),
        root.join("spools"),
    );
    waiting_pod(&root, Some("gpu-1"), script_running("2026-01-01T00:01:00Z"));
    engine
        .observe_runs(&adapter, std::slice::from_ref(&run), budget)
        .await;
    assert_eq!(std::fs::read(root.join("source.tar")).unwrap(), b"snapshot");
    assert_eq!(engine.load("run").unwrap().0.status, "running");
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(
        !calls.lines().any(|call| call.starts_with("delete")),
        "{calls}"
    );
    assert_eq!(
        calls
            .lines()
            .filter(|call| call.starts_with("create -f"))
            .count(),
        1,
        "{calls}"
    );
}

#[tokio::test]
async fn k8s_timeout_starts_when_the_script_container_does() {
    let Some(root) = isolated().await else {
        return;
    };
    waiting_pod(
        &root,
        None,
        json!({"phase": "Pending", "conditions": [{"type": "PodScheduled", "status": "False",
            "reason": "Unschedulable", "message": "0/2 nodes are available: 2 Insufficient nvidia.com/gpu."}]}),
    );
    let (engine, adapter, clock, run) = waiting_run(&root).await;
    let budget = Duration::from_secs(10);
    let observe_at = |now: u64| {
        std::fs::write(root.join("now"), now.to_string()).unwrap();
        clock.advance(budget);
        async {
            engine
                .observe_runs(&adapter, std::slice::from_ref(&run), budget)
                .await;
            engine.load("run").unwrap().0
        }
    };

    // Waits two hours for a GPU, twice its one-hour timeout.
    assert_eq!(observe_at(7200).await.status, "starting");

    waiting_pod(&root, Some("gpu-1"), script_running("2026-01-01T02:00:00Z"));
    assert_eq!(observe_at(7200).await.status, "running");
    assert_eq!(observe_at(7200 + 3599).await.status, "running");

    let row = observe_at(7200 + 3600).await;
    assert_eq!(row.status, "failed");
    let reason = row.result_markdown.unwrap();
    assert!(reason.contains("DeadlineExceeded"), "{reason}");
}

#[tokio::test]
async fn k8s_pull_errors_that_may_have_cleared_keep_the_run_queued() {
    let Some(root) = isolated().await else {
        return;
    };
    waiting_pod(
        &root,
        Some("gpu-1"),
        script_waiting(
            "ImagePullBackOff",
            "Back-off pulling image \"ghcr.io/lab/train:v1\"",
        ),
    );
    pull_events(
        &root,
        &[
            (
                "spec.initContainers{setup}",
                "2026-01-01T00:01:00Z",
                "Failed to pull image \"ghcr.io/lab/setup:v1\": unauthorized: authentication required",
            ),
            (
                "spec.containers{run}",
                "2026-01-01T00:02:00Z",
                "Failed to pull image \"ghcr.io/lab/train:v1\": unauthorized: authentication required",
            ),
            (
                "spec.containers{run}",
                "2026-01-01T00:03:00Z",
                "Failed to pull image \"ghcr.io/lab/train:v1\": toomanyrequests: rate limit exceeded",
            ),
        ],
    );
    let (engine, adapter, _clock, run) = waiting_run(&root).await;
    engine
        .observe_runs(
            &adapter,
            std::slice::from_ref(&run),
            Duration::from_secs(10),
        )
        .await;
    assert_eq!(engine.load("run").unwrap().0.status, "starting");
    assert!(!root.join("deleted").exists());
}

#[tokio::test]
async fn k8s_a_forbidden_deadline_patch_fails_the_run_and_frees_the_job() {
    let Some(root) = isolated().await else {
        return;
    };
    std::fs::write(root.join("patch-forbidden"), "").unwrap();
    waiting_pod(&root, Some("gpu-1"), script_running("2026-01-01T00:01:00Z"));
    let (engine, adapter, _clock, run) = waiting_run(&root).await;
    engine
        .observe_runs(
            &adapter,
            std::slice::from_ref(&run),
            Duration::from_secs(10),
        )
        .await;
    let row = engine.load("run").unwrap().0;
    assert_eq!(row.status, "failed");
    let reason = row.result_markdown.unwrap();
    assert!(reason.contains("cannot patch"), "{reason}");
    assert!(root.join("deleted").exists());
    assert!(!root.join("source.tar").exists());
}

#[tokio::test]
async fn k8s_preflight_needs_permission_to_patch_jobs() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/kubectl"),
        r#"#!/usr/bin/env python3
import sys
a = sys.argv[1:]
if a[:2] == ['auth', 'can-i']:
    print('yes' if a[2] == 'create' else 'no')
    sys.exit(0 if a[2] == 'create' else 1)
"#,
    );
    let backend = crate::compute::backend("k8s").unwrap();
    let preflight = backend
        .preflight(&crate::compute::tests::tinker_args())
        .await
        .unwrap();
    assert!(!preflight.ready, "{:?}", preflight.detail);
}

#[tokio::test]
async fn k8s_unpullable_image_fails_with_the_pull_error_and_frees_the_job() {
    let Some(root) = isolated().await else {
        return;
    };
    let backoff = || {
        script_waiting(
            "ImagePullBackOff",
            "Back-off pulling image \"ghcr.io/lab/train:v1\"",
        )
    };
    waiting_pod(&root, Some("gpu-1"), backoff());
    let rate_limited = (
        "spec.containers{run}",
        "2026-01-01T00:01:00Z",
        "Failed to pull image \"ghcr.io/lab/train:v1\": toomanyrequests: rate limit exceeded",
    );
    pull_events(&root, &[rate_limited]);
    let (engine, adapter, clock, run) = waiting_run(&root).await;
    let budget = Duration::from_secs(10);
    engine
        .observe_runs(&adapter, std::slice::from_ref(&run), budget)
        .await;
    assert_eq!(engine.load("run").unwrap().0.status, "starting");
    assert!(!root.join("deleted").exists());

    let missing = (
        "spec.containers{run}",
        "2026-01-01T00:02:00Z",
        "Failed to pull image \"ghcr.io/lab/train:v1\": rpc error: code = NotFound desc = failed to resolve reference \"ghcr.io/lab/train:v1\": not found",
    );
    pull_events(&root, &[rate_limited, missing]);
    clock.advance(budget);
    engine
        .observe_runs(&adapter, std::slice::from_ref(&run), budget)
        .await;
    let row = engine.load("run").unwrap().0;
    assert_eq!(row.status, "failed");
    let reason = row.result_markdown.unwrap();
    assert!(
        reason.contains("ghcr.io/lab/train:v1") && reason.contains("not found"),
        "{reason}"
    );
    assert!(root.join("deleted").exists());
}

#[tokio::test]
async fn openresearch_cancel_and_abandon_release_without_ssh_and_retry_api_failures() {
    use super::openresearch::OpenResearch;
    use crate::store::{Contact, StoredRun};

    let Some(root) = isolated().await else {
        return;
    };
    // Paces the box deletion retries this test makes fail.
    std::env::set_var("ORX_TEST_SPEEDUP", "100");
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    for leased in [false, true] {
        for abandon in [false, true] {
            for needs_auth in [false, true] {
                let id = format!("box-{leased}-{abandon}-{needs_auth}");
                let mut descriptor = BackendDescriptor::parse(
                    &json!({"kind":"openresearch_job","namespace":"org","jobId":id}).to_string(),
                )
                .unwrap();
                if leased {
                    descriptor.rental_lease = Some(crate::jobs::rental::Rental {
                        api_url: std::env::var("HF_ENDPOINT").unwrap(),
                        sandbox_id: id.clone(),
                        token: "fake".into(),
                        unknown: Default::default(),
                    });
                }
                let handle = Handle::Legacy {
                    descriptor: Box::new(descriptor.clone()),
                };
                let backend_json = if leased {
                    handle::backend_json(&id, Some(&handle), &descriptor).unwrap()
                } else {
                    descriptor.to_json()
                };
                let mut engine = engine(&root.join(&id));
                let clock = Arc::new(TestClock::new());
                engine.clock = clock.clone();
                let engine = Arc::new(engine);
                engine
                    .store()
                    .upsert_run(&StoredRun {
                        id: id.clone(),
                        experiment_id: "exp".into(),
                        project_id: "proj".into(),
                        status: "running".into(),
                        backend_json,
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
                    })
                    .unwrap();
                let observation = if needs_auth {
                    Observation::NeedsAuth {
                        cause: "Permission denied (publickey)".into(),
                    }
                } else {
                    Observation::Unreachable {
                        cause: "Connection timed out".into(),
                    }
                };
                engine.observe(&id, observation).unwrap();
                clock.advance(Duration::from_secs(600));
                engine.check_outages().unwrap();
                assert_eq!(engine.load(&id).unwrap().0.status, "running");
                assert!(engine
                    .store()
                    .run_notice_batch("chat")
                    .unwrap()
                    .iter()
                    .any(|notice| {
                        notice.kind == crate::store::ATTENTION
                            && notice.body.contains("still billing")
                    }));
                let adapter =
                    OpenResearch::new(engine.clone(), Arc::new(|_| panic!("teardown used SSH")));
                let watched = [WatchedRun {
                    id: id.clone(),
                    handle,
                }];
                std::fs::write(root.join("delete-fails"), "").unwrap();
                if abandon {
                    assert!(engine.abandon(&id).unwrap());
                    engine
                        .observe_runs(&adapter, &watched, Duration::ZERO)
                        .await;
                } else {
                    assert!(engine.cancel(&adapter, &id, "user").await.unwrap());
                }
                let (run, state) = engine.load(&id).unwrap();
                assert_eq!(run.status, if abandon { "lost" } else { "running" });
                assert_ne!(state.contact, Contact::Live);
                assert!(!engine.memory().cancels_sent.contains(&id));
                std::fs::remove_file(root.join("delete-fails")).unwrap();
                engine
                    .observe_runs(&adapter, &watched, Duration::ZERO)
                    .await;
                let (run, state) = engine.load(&id).unwrap();
                assert_eq!(run.status, if abandon { "lost" } else { "cancelled" });
                assert_eq!(state.contact, Contact::Live);
                let released = if leased {
                    format!("POST /sandboxes/{id}/lease/release")
                } else {
                    format!("DELETE /sandboxes/{id}")
                };
                assert!(std::fs::read_to_string(root.join("http.log"))
                    .unwrap()
                    .lines()
                    .any(|line| line == released));
            }
        }
    }
}

#[tokio::test]
async fn openresearch_rejected_key_auto_cancel_checks_rental_and_survives_restart() {
    use super::openresearch::{LegacyTransport, OpenResearch};
    use super::ssh::Ssh;
    use crate::engine::transport::{Auth, Config, HostClass, HostConn, HostState, Status};
    use crate::store::StoredRun;

    let Some(root) = isolated().await else {
        return;
    };
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    for case in [
        "leased", "sandbox", "released", "restart", "retry", "lookup", "ssh",
    ] {
        let dir = root.join(case);
        let mut initial = engine(&dir);
        let clock = Arc::new(TestClock::new());
        initial.clock = clock.clone();
        let mut engine = Arc::new(initial);
        let mut descriptor = BackendDescriptor::parse(
            &json!({
                "kind": if case == "ssh" { "ssh_job" } else { "openresearch_job" },
                "namespace":"org", "jobId":"box", "sshHost":"unused", "sshPort":22, "sshUser":"root"
            })
            .to_string(),
        )
        .unwrap();
        if case != "sandbox" && case != "ssh" {
            descriptor.rental_lease = Some(crate::jobs::rental::Rental {
                api_url: std::env::var("HF_ENDPOINT").unwrap(),
                sandbox_id: "box".into(),
                token: "fake".into(),
                unknown: Default::default(),
            });
        }
        let watched = [WatchedRun {
            id: case.into(),
            handle: Handle::Legacy {
                descriptor: Box::new(descriptor.clone()),
            },
        }];
        engine
            .store()
            .upsert_run(&StoredRun {
                id: case.into(),
                experiment_id: "exp".into(),
                project_id: "proj".into(),
                status: "running".into(),
                backend_json: descriptor.to_json(),
                command: "true".into(),
                created_at: clock.wall_ms(),
                updated_at: clock.wall_ms(),
                ended_at: None,
                exit_code: None,
                commit_sha: None,
                result_markdown: None,
                cancel_requested: false,
                cancel_reason: None,
                chat_session_id: Some("chat".into()),
            })
            .unwrap();
        let live = clock.wall_ms();
        clock.advance(Duration::from_secs(1));
        engine
            .store()
            .seed_engine_host(
                "box",
                "/root",
                Some(HostClass::Key),
                &HostState {
                    status: Status::NeedsAuth,
                    since_ms: clock.wall_ms(),
                    last_live_ms: Some(live),
                    cause: Some("Permission denied (publickey)".into()),
                    ..HostState::default()
                },
            )
            .unwrap();
        engine
            .observe(
                case,
                Observation::NeedsAuth {
                    cause: "Permission denied (publickey)".into(),
                },
            )
            .unwrap();
        let make_adapter = |engine: Arc<Engine>| {
            let store = Store::open_at(dir.join("store")).unwrap();
            let auth = Auth::new(&store, "box").unwrap();
            let conn = HostConn::start_shared(
                Config {
                    alias: "box".into(),
                    root: "/root".into(),
                    ssh_config: Some(dir.join("never-connect")),
                    control_dir: dir.join("cm"),
                    sharing_home: None,
                    binary_dir: None,
                    askpass_executable: std::env::current_exe().unwrap(),
                },
                store,
                clock.clone(),
                auth,
            )
            .unwrap();
            let transport = Arc::new(LegacyTransport::new(
                Arc::new(Ssh::new("box".into(), "/root".into(), conn)),
                dir.join("logs"),
            ));
            OpenResearch::new(engine, Arc::new(move |_| Ok(transport.clone())))
        };
        let mut adapter = make_adapter(engine.clone());
        clock.advance(Duration::from_secs(7 * 60));
        adapter.observe(&watched, Duration::ZERO).await;
        assert!(engine.load(case).unwrap().1.cancel_accepted_at.is_none());
        if case == "restart" {
            drop(adapter);
            tokio::time::timeout(Duration::from_secs(5), async {
                while Arc::strong_count(&engine) != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            drop(engine);
            engine = Arc::new(Engine::new(
                Store::open_at(dir.join("store")).unwrap(),
                clock.clone(),
            ));
            adapter = make_adapter(engine.clone());
        }
        clock.advance(Duration::from_secs(8 * 60 - 1));
        adapter.observe(&watched, Duration::ZERO).await;
        assert!(engine.load(case).unwrap().1.cancel_accepted_at.is_none());
        if case == "released" {
            std::fs::write(root.join("box-released"), "").unwrap();
        }
        if case == "retry" {
            std::fs::write(root.join("delete-fails"), "").unwrap();
        }
        if case == "lookup" {
            std::fs::write(root.join("lookup-fails"), "").unwrap();
        }
        std::fs::write(root.join("http.log"), "").unwrap();
        clock.advance(Duration::from_secs(1));
        adapter.observe(&watched, Duration::ZERO).await;
        let (run, state) = engine.load(case).unwrap();
        if case == "retry" || case == "lookup" {
            assert_eq!(run.status, "running");
            assert_eq!(state.cancel_accepted_at.is_some(), case == "retry");
            let flag = if case == "retry" {
                "delete-fails"
            } else {
                "lookup-fails"
            };
            std::fs::remove_file(root.join(flag)).unwrap();
            engine
                .observe_runs(&adapter, &watched, Duration::ZERO)
                .await;
        }
        let (run, state) = engine.load(case).unwrap();
        assert_eq!(
            run.status,
            match case {
                "released" => "lost",
                "ssh" => "running",
                _ => "cancelled",
            },
            "{case}"
        );
        let accepted = state.cancel_accepted_at;
        if case != "ssh" && case != "released" {
            assert!(accepted.is_some());
            assert_eq!(
                run.cancel_reason.as_deref(),
                Some(crate::plane::LABELS["run_notice_rental_key_cancelled"].as_str())
            );
        } else {
            assert!(accepted.is_none());
        }
        drop(adapter);
        drop(engine);
        engine = Arc::new(Engine::new(
            Store::open_at(dir.join("store")).unwrap(),
            clock.clone(),
        ));
        adapter = make_adapter(engine.clone());
        for _ in 0..2 {
            engine
                .observe_runs(&adapter, &watched, Duration::ZERO)
                .await;
        }
        assert_eq!(engine.load(case).unwrap().1.cancel_accepted_at, accepted);
        let requests = std::fs::read_to_string(root.join("http.log")).unwrap();
        let releases = requests
            .lines()
            .filter(|line| line.ends_with("/lease/release") || line.starts_with("DELETE "))
            .count();
        assert_eq!(
            releases,
            match case {
                "ssh" | "released" => 0,
                "retry" => 2,
                _ => 1,
            },
            "{case}: {requests}"
        );
        if case != "ssh" {
            assert!(requests
                .lines()
                .next()
                .unwrap()
                .starts_with("GET /sandboxes/"));
            let notices = engine.store().run_notice_batch("chat").unwrap();
            let terminal: Vec<_> = notices
                .iter()
                .filter(|notice| notice.kind == crate::store::TERMINAL)
                .collect();
            assert_eq!(terminal.len(), 1, "{case}");
            assert!(terminal[0].body.contains(if case == "released" {
                &crate::plane::LABELS["run_notice_rental_outcome_unavailable"]
            } else {
                &crate::plane::LABELS["run_notice_rental_key_cancelled"]
            }));
        } else {
            assert!(requests.is_empty());
        }
        if case == "released" {
            std::fs::remove_file(root.join("box-released")).unwrap();
        }
    }
}

#[tokio::test]
async fn round_one_bootstrap_heartbeat_precedes_wrapper_activation() {
    use super::openresearch::{LegacyTransport, OpenResearch};
    use super::ssh::Ssh;
    use crate::engine::transport::{Config, HostConn};
    let Some(root) = isolated().await else {
        return;
    };
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    let server = orx_test_support::Sshd::start().unwrap();
    let shell = server.directory().join("command");
    let command = std::fs::read_to_string(&shell).unwrap();
    std::fs::write(
        shell,
        command.replacen(
            "export HOME",
            "export HOME\nexport PATH=\"$HOME/.local/bin:$PATH\"",
            1,
        ),
    )
    .unwrap();
    let clock = Arc::new(TestClock::new());
    let config = Config {
        alias: server.alias().into(),
        root: "~/engine-root".into(),
        ssh_config: Some(server.config_path()),
        control_dir: server.directory().join("cm"),
        binary_dir: Some(orx_test_support::host_binaries()),
        sharing_home: None,
        askpass_executable: std::env::current_exe().unwrap(),
    };
    let store = Store::open_at(root.join("transport")).unwrap();
    let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
    let conn = HostConn::start_shared(config, store, clock.clone(), auth).unwrap();
    let transport = Arc::new(LegacyTransport::new(
        Arc::new(Ssh::new(
            server.alias().into(),
            "~/engine-root".into(),
            conn.clone(),
        )),
        root.join("store/run-logs"),
    ));
    let engine = Arc::new(engine(&root));
    let id = uuid::Uuid::new_v4().to_string();
    let fixture = root.join("snapshot");
    std::fs::create_dir_all(&fixture).unwrap();
    std::fs::write(fixture.join("README"), "snapshot").unwrap();
    let archive = root.join("source.tar");
    assert!(crate::process::command("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&fixture)
        .arg(".")
        .status()
        .unwrap()
        .success());
    let bytes = std::fs::read(&archive).unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    script(&server.home().join(".local/bin/uv"), "#!/bin/sh\nexit 0\n");
    script(
        &server.home().join(".local/bin/timeout"),
        "#!/bin/sh\nshift 3\nexec \"$@\"\n",
    );
    let descriptor = BackendDescriptor::parse(&json!({
        "kind":"openresearch_job", "namespace":"org", "jobId":"box",
            "sourceDigest":digest,"sourcePath":archive,"sourceSize":bytes.len(),
        "sshHost":"127.0.0.1", "sshPort":server.port(), "sshUser":server.user(),
        "rentalLease":{"apiUrl":std::env::var("HF_ENDPOINT").unwrap(),"sandboxId":"box","token":"fake"}
    }).to_string()).unwrap();
    let launch = LaunchSpec {
        kind: "openresearch_job".into(),
        target: Some("org".into()),
        options: json!({}),
    };
    let mut submission = pending(&id, &descriptor, &launch);
    submission.spec_digest = &digest;
    submission.command = "printf 'before disconnect\\n'; sleep 2; printf 'final result\\n'; exit 7";
    submission.created_at = engine.clock.wall_ms();
    engine.insert(submission).unwrap();
    let watched = WatchedRun {
        id: id.clone(),
        handle: Handle::OpenResearch {
            descriptor: Box::new(descriptor),
        },
    };
    engine
        .store()
        .set_submission_fence(
            &id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    assert!(engine
        .store()
        .record_run_handle(&id, 1, &watched.handle)
        .unwrap());
    let factory: super::openresearch::TransportFactory = {
        let transport = transport.clone();
        Arc::new(move |_| Ok(transport.clone()))
    };
    let adapter = OpenResearch::new(engine.clone(), factory.clone());
    let directory = server.home().join(format!("engine-root/runs/{id}"));
    tokio::time::timeout(Duration::from_secs(25), async {
        while !directory.join("started").exists() {
            engine
                .observe_runs(&adapter, std::slice::from_ref(&watched), Duration::ZERO)
                .await;
            clock.advance(Duration::from_secs(10));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let requests = std::fs::read_to_string(root.join("http.log")).unwrap();
    let heartbeat = requests
        .find("POST /sandboxes/box/lease/heartbeat")
        .expect("bootstrap heartbeat");
    let activated = requests
        .find("POST /sandboxes/box/lease/activate")
        .expect("wrapper activation");
    assert!(heartbeat < activated, "{requests}");
    server.proxy().reset().unwrap();
    drop(adapter);
    let adapter = OpenResearch::new(engine.clone(), factory);
    tokio::time::timeout(Duration::from_secs(25), async {
        while !phase_of(&engine.load(&id).unwrap().0)
            .unwrap()
            .is_terminal()
        {
            clock.advance(Duration::from_secs(60));
            engine
                .observe_runs(&adapter, std::slice::from_ref(&watched), Duration::ZERO)
                .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    let (run, state) = engine.load(&id).unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(
        run.exit_code,
        Some(7),
        "{}",
        std::fs::read_to_string(directory.join("log")).unwrap_or_default()
    );
    assert_eq!(state.started_at, orx_host::wrapper::started_at(&directory));
    crate::engine::runtime::Runtime::collect_log(
        &root.join("store"),
        &adapter,
        &watched,
        &mut false,
    )
    .await
    .unwrap();
    adapter.finish(&watched).await.unwrap();
    assert!(directory.join("final_collected").exists());
    assert_eq!(
        std::fs::read(root.join(format!("store/run-logs/{id}.log"))).unwrap(),
        b"before disconnect\nfinal result\n"
    );
    let receipt = orx_host::wrapper::receipt(&directory).unwrap().unwrap();
    assert_eq!(receipt.code, Some(7));
}

async fn legacy_box(root: PathBuf, initial_stage: &str) {
    use super::openresearch::{LegacyTransport, OpenResearch};
    use super::ssh::Ssh;
    use crate::engine::transport::{Config, HostConn, Status};
    use crate::store::StoredRun;
    use orx_test_support::host_binaries as binaries;
    // Paces the timers the checks below outlast.
    std::env::set_var("ORX_TEST_SPEEDUP", "10");
    let watching = initial_stage != "starting";
    let server = orx_test_support::Sshd::start().unwrap();
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    let transport_clock = Arc::new(TestClock::new());
    let conn = {
        let config = Config {
            alias: server.alias().into(),
            root: "~/engine-root".into(),
            ssh_config: Some(server.config_path()),
            control_dir: server.directory().join("cm"),
            binary_dir: Some(binaries()),
            #[cfg(unix)]
            sharing_home: None,
            askpass_executable: std::env::current_exe().unwrap(),
        };
        let store = Store::open_at(root.join("transport")).unwrap();
        let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
        HostConn::start_shared(config, store, transport_clock.clone(), auth)
    }
    .unwrap();
    let mut state = conn.state();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let status = state.borrow_and_update().status.clone();
            if status == Status::Live {
                break;
            }
            state.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let ssh = Arc::new(Ssh::new(
        server.alias().into(),
        "~/engine-root".into(),
        conn.clone(),
    ));
    let transport = Arc::new(LegacyTransport::new(ssh, root.join("store/run-logs")));
    let id = uuid::Uuid::new_v4().to_string();
    let archive = root.join("source.tar");
    let source = if watching {
        b"snapshot".to_vec()
    } else {
        vec![42; 16 * 1024 * 1024]
    };
    std::fs::write(&archive, &source).unwrap();
    let digest = format!("{:x}", Sha256::digest(&source));
    let descriptor = BackendDescriptor::parse(
        &json!({
            "kind":"openresearch_job", "namespace":"org", "jobId":"box",
            "sshHost":"127.0.0.1", "sshPort":server.port(), "sshUser":server.user(),
            "sourceDigest":digest, "sourcePath":archive, "sourceSize":source.len(),
            "rentalLease":{"apiUrl":std::env::var("HF_ENDPOINT").unwrap(),"sandboxId":"box","token":"fake"}
        })
        .to_string(),
    )
    .unwrap();
    if watching {
        let dir = server.home().join(format!(".orx/runs/{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("pid"), server.pid().to_string()).unwrap();
        std::fs::write(dir.join("log"), vec![b'x'; 5000]).unwrap();
        if initial_stage == "done" {
            std::fs::write(dir.join("exit_code"), "0\n").unwrap();
        }
    }
    let engine = Arc::new(engine(&root));
    let stored = StoredRun {
        id: id.clone(),
        experiment_id: "exp".into(),
        project_id: "proj".into(),
        status: "starting".into(),
        backend_json: descriptor.to_json(),
        command: "true".into(),
        created_at: 1,
        updated_at: 1,
        ended_at: None,
        exit_code: None,
        commit_sha: Some("snapshot".into()),
        result_markdown: None,
        cancel_requested: false,
        cancel_reason: None,
        chat_session_id: None,
    };
    engine.store().upsert_run(&stored).unwrap();
    let factory_transport = transport.clone();
    let adapter = OpenResearch::new(
        engine.clone(),
        Arc::new(move |_| Ok(factory_transport.clone())),
    );
    let watched = [WatchedRun {
        id: id.clone(),
        handle: Handle::Legacy {
            descriptor: Box::new(descriptor),
        },
    }];
    engine
        .observe_runs(&adapter, &watched, Duration::from_secs(1))
        .await;
    if !watching {
        assert!(matches!(
            engine.load(&id).unwrap().1.contact,
            crate::store::Contact::Unreconciled { .. }
        ));
    }
    if initial_stage == "done" {
        crate::engine::runtime::Runtime::collect_log(
            &root.join("store"),
            &adapter,
            &watched[0],
            &mut false,
        )
        .await
        .unwrap();
        let mut ranges =
            crate::commands::up::api_v2::kept_ranges(&root.join("store"), &id).unwrap();
        ranges.last_mut().unwrap().eof = false;
        crate::commands::up::api_v2::keep_ranges(&root.join("store"), &id, ranges).unwrap();
        adapter.finish(&watched[0]).await.unwrap();
        engine.store().finish_log_collection(&id).unwrap();
        let dir = server.home().join(format!(".orx/runs/{id}"));
        tokio::time::timeout(Duration::from_secs(20), async {
            while !dir.join("final_collected").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(engine.load(&id).unwrap().0.status, "done");
        assert!(!engine.store().log_collection_pending(&id).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
        let runtime = crate::engine::runtime::Runtime::new(
            engine.clone(),
            Arc::new(TestClock::new()),
            root.join("store"),
            None,
        );
        let api = crate::commands::up::api_v2::Api::new(
            "test".into(),
            "token".into(),
            engine,
            runtime,
            Box::new(|_| Ok(Box::new(()))),
            root.join("store"),
        )
        .unwrap();
        crate::commands::up::api_v2::install(Arc::new(api)).unwrap();
        use tower::ServiceExt;
        let response = crate::commands::up::api_v2::router()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/runs/{id}/log?start=0&end=1048576"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let page: LogPage = serde_json::from_slice(&bytes).unwrap();
        assert!(
            matches!(page, LogPage::Data { bytes, eof: true, .. } if bytes == vec![b'x'; 5000])
        );
        return;
    }
    if watching {
        assert_eq!(engine.load(&id).unwrap().0.status, "running");
        crate::engine::runtime::Runtime::collect_log(
            &root.join("store"),
            &adapter,
            &watched[0],
            &mut false,
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(root.join(format!("store/run-logs/{id}.log"))).unwrap(),
            vec![b'x'; 5000]
        );
    }
    std::fs::write(server.directory().join("authorized_keys"), "").unwrap();
    server.proxy().reset().unwrap();
    transport_clock.advance(Duration::from_secs(60));
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if state.borrow_and_update().status == Status::NeedsAuth {
                break;
            }
            transport_clock.advance(Duration::from_secs(5));
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            engine
                .observe_runs(&adapter, &watched, Duration::ZERO)
                .await;
            if matches!(
                engine.load(&id).unwrap().1.contact,
                crate::store::Contact::NeedsAuth { .. }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let heartbeats = || {
        std::fs::read_to_string(root.join("http.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.ends_with("/lease/heartbeat"))
            .count()
    };
    let stopped = heartbeats();
    // Outlasts a poll interval while watching and a lease heartbeat while staging.
    let checks = if watching { 6 } else { 32 };
    for _ in 0..checks {
        adapter.observe(&watched, Duration::from_secs(1)).await;
        tokio::time::sleep(crate::config::paced(Duration::from_secs(1))).await;
        assert_eq!(
            heartbeats(),
            stopped,
            "an unreachable box renewed its lease"
        );
    }
    let attempts = server.auth_attempts().unwrap();
    assert_eq!(
        attempts
            .get(&("publickey".into(), "Failed".into()))
            .copied()
            .unwrap_or(0),
        1,
        "{attempts:?}"
    );
    transport_clock.advance(Duration::from_secs(1800));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(server.auth_attempts().unwrap(), attempts);
    assert!(matches!(
        engine.load(&id).unwrap().0.status.as_str(),
        "starting" | "running"
    ));
    assert!(!std::fs::read_dir(root.join("store"))
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("ssh-service-")));
    let processes = crate::process::command("ps")
        .args(["-axo", "command"])
        .output()
        .unwrap();
    let needle = format!("{} ssh-service", std::env::current_exe().unwrap().display());
    assert!(!String::from_utf8_lossy(&processes.stdout).contains(&needle));
    if watching {
        assert!(engine.cancel(&adapter, &id, "user").await.unwrap());
        assert_eq!(engine.load(&id).unwrap().0.status, "cancelled");
        crate::engine::runtime::Runtime::collect_log(
            &root.join("store"),
            &adapter,
            &watched[0],
            &mut false,
        )
        .await
        .unwrap();
        engine.store().finish_log_collection(&id).unwrap();
        assert!(!engine.store().log_collection_pending(&id).unwrap());
    } else {
        assert!(engine.abandon(&id).unwrap());
        engine
            .observe_runs(&adapter, &watched, Duration::ZERO)
            .await;
        assert_eq!(engine.load(&id).unwrap().0.status, "lost");
    }
    assert_eq!(
        engine.load(&id).unwrap().1.contact,
        crate::store::Contact::Live
    );
    assert!(std::fs::read_to_string(root.join("http.log"))
        .unwrap()
        .lines()
        .any(|line| line == "POST /sandboxes/box/lease/release"));
    assert_eq!(server.auth_attempts().unwrap(), attempts);
    drop(adapter);
    drop(conn);
}

#[tokio::test]
async fn openresearch_staging_rejected_key_needs_auth_after_one_attempt() {
    let Some(root) = isolated().await else {
        return;
    };
    legacy_box(root, "starting").await;
}

#[tokio::test]
async fn openresearch_watching_rejected_key_needs_auth_after_one_attempt() {
    let Some(root) = isolated().await else {
        return;
    };
    legacy_box(root, "running").await;
}

#[tokio::test]
async fn openresearch_completed_log_is_readable_after_release() {
    let Some(root) = isolated().await else {
        return;
    };
    legacy_box(root, "done").await;
}

/// An engine-launched run on box `box`, its descriptor extended with `fields`.
fn box_run(engine: &Engine, fields: Value) -> WatchedRun {
    let mut descriptor = json!({
        "kind":"openresearch_job", "namespace":"org", "jobId":"box",
        "rentalLease":{"apiUrl":std::env::var("HF_ENDPOINT").unwrap(),"sandboxId":"box","token":"fake"}
    });
    descriptor
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    let descriptor = BackendDescriptor::parse(&descriptor.to_string()).unwrap();
    let launch = LaunchSpec {
        kind: "openresearch_job".into(),
        target: Some("org".into()),
        options: json!({}),
    };
    let id = uuid::Uuid::new_v4().to_string();
    let mut submission = pending(&id, &descriptor, &launch);
    submission.created_at = engine.clock.wall_ms();
    engine.insert(submission).unwrap();
    engine
        .store()
        .set_submission_fence(
            &id,
            1,
            &crate::store::SubmissionFence {
                taken_at: 1,
                submitter: None,
            },
        )
        .unwrap();
    let handle = Handle::OpenResearch {
        descriptor: Box::new(descriptor),
    };
    assert!(engine.store().record_run_handle(&id, 1, &handle).unwrap());
    WatchedRun { id, handle }
}

/// Makes `~/.ssh/id_ed25519` with `passphrase` and registers its public half on the fake account.
fn local_key(root: &Path, passphrase: &str) {
    let ssh = crate::config::home_dir().unwrap().join(".ssh");
    std::fs::create_dir_all(&ssh).unwrap();
    let key = ssh.join("id_ed25519");
    for path in [key.clone(), key.with_extension("pub")] {
        let _ = std::fs::remove_file(path);
    }
    assert!(crate::process::command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", passphrase, "-f"])
        .arg(&key)
        .status()
        .unwrap()
        .success());
    let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
    std::fs::write(
        root.join("ssh-keys.json"),
        json!({"sshKeys":[{"id":"key","name":"laptop","publicKey":public.trim()}]}).to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn openresearch_box_refusing_the_key_fails_the_run_with_the_cause() {
    use super::openresearch::{LegacyTransport, OpenResearch};
    use super::ssh::Ssh;
    use crate::engine::transport::{Auth, Config, HostConn};

    let Some(root) = isolated().await else {
        return;
    };
    std::env::set_var("HOME", root.join("home"));
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    for (case, passphrase) in [
        ("absent", ""),
        ("locked", "secret"),
        ("released", ""),
        ("dead", ""),
    ] {
        local_key(&root, passphrase);
        let server = orx_test_support::Sshd::start().unwrap();
        std::fs::write(server.directory().join("authorized_keys"), "").unwrap();
        let alias = "orx-box-refusal";
        let config = std::fs::read_to_string(server.config_path())
            .unwrap()
            .replace(
                &format!("Host {}", server.alias()),
                &format!("Host {alias}"),
            );
        std::fs::write(server.config_path(), config).unwrap();

        let dir = root.join(case);
        let clock = Arc::new(TestClock::new());
        let mut engine = engine(&dir);
        engine.clock = clock.clone();
        let engine = Arc::new(engine);
        let store = Store::open_at(dir.join("transport")).unwrap();
        let auth = Auth::new(&store, alias).unwrap();
        let conn = HostConn::start_shared(
            Config {
                alias: alias.into(),
                root: "~/engine-root".into(),
                ssh_config: Some(server.config_path()),
                control_dir: server.directory().join("cm"),
                binary_dir: Some(orx_test_support::host_binaries()),
                sharing_home: None,
                askpass_executable: std::env::current_exe().unwrap(),
            },
            store,
            clock.clone(),
            auth,
        )
        .unwrap();
        let transport = Arc::new(LegacyTransport::new(
            Arc::new(Ssh::new(alias.into(), "~/engine-root".into(), conn)),
            dir.join("store/run-logs"),
        ));
        let watched = box_run(
            &engine,
            json!({"sshHost":"127.0.0.1", "sshPort":server.port(), "sshUser":server.user()}),
        );
        let adapter = OpenResearch::new(engine.clone(), Arc::new(move |_| Ok(transport.clone())));
        std::fs::write(root.join("http.log"), "").unwrap();

        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                engine
                    .observe_runs(&adapter, std::slice::from_ref(&watched), Duration::ZERO)
                    .await;
                let (run, state) = engine.load(&watched.id).unwrap();
                if phase_of(&run).unwrap().is_terminal() {
                    break;
                }
                if case == "released"
                    && matches!(state.contact, crate::store::Contact::Unreachable { .. })
                {
                    std::fs::write(root.join("box-released"), "").unwrap();
                }
                if case == "dead"
                    && matches!(state.contact, crate::store::Contact::NeedsAuth { .. })
                {
                    let sandbox =
                        json!({"status":"dead", "provisionWarnings":"the host was reclaimed"});
                    std::fs::write(root.join("sandbox.json"), sandbox.to_string()).unwrap();
                }
                clock.advance(Duration::from_secs(10));
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();

        let (run, _) = engine.load(&watched.id).unwrap();
        assert_eq!(run.status, "failed", "{case}");
        let reason = run.result_markdown.unwrap();
        // A locked key names its private file and how to load it; a refused one names the `.pub`.
        assert!(reason.contains("~/.ssh/id_ed25519"), "{reason}");
        assert_eq!(reason.contains("ssh-add"), case == "locked", "{reason}");
        assert_eq!(
            reason.contains("~/.ssh/id_ed25519.pub"),
            case != "locked",
            "{reason}"
        );
        assert_eq!(
            reason.contains("the host was reclaimed"),
            case == "dead",
            "{reason}"
        );
        let releases = std::fs::read_to_string(root.join("http.log"))
            .unwrap()
            .lines()
            .filter(|line| *line == "POST /sandboxes/box/lease/release")
            .count();
        assert_eq!(releases, usize::from(case != "released"), "{case}");
        let _ = std::fs::remove_file(root.join("box-released"));
        let _ = std::fs::remove_file(root.join("sandbox.json"));
    }
}

#[tokio::test]
async fn openresearch_box_that_never_comes_online_fails_with_the_service_reason() {
    use super::openresearch::OpenResearch;

    let Some(root) = isolated().await else {
        return;
    };
    // Paces the box deletion retries this test makes fail.
    std::env::set_var("ORX_TEST_SPEEDUP", "100");
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    // `details` are what tells the causes apart: the service's code, status and words.
    for (case, sandbox, details) in [
        (
            "failed",
            json!({"status":"failed", "failureCode":"readiness_timeout", "failureMessage":"The box was not ready within 5 minutes."}),
            &[
                "readiness_timeout",
                "The box was not ready within 5 minutes.",
            ][..],
        ),
        (
            "offline",
            json!({"status":"offline", "provisionWarnings":"host unreachable"}),
            &["offline", "host unreachable"],
        ),
        ("deleted", json!({}), &[]),
        (
            "deadline",
            json!({"status":"provisioning"}),
            &["provisioning"],
        ),
    ] {
        std::fs::write(root.join("sandbox.json"), sandbox.to_string()).unwrap();
        if case == "deleted" {
            std::fs::write(root.join("box-released"), "").unwrap();
        }
        std::fs::write(root.join("http.log"), "").unwrap();
        let clock = Arc::new(TestClock::new());
        let mut engine = engine(&root.join(case));
        engine.clock = clock.clone();
        let engine = Arc::new(engine);
        let watched = [box_run(&engine, json!({}))];
        let adapter = OpenResearch::new(
            engine.clone(),
            Arc::new(|_| panic!("no SSH before the box is online")),
        );
        let observe = || async {
            engine
                .observe_runs(&adapter, &watched, Duration::ZERO)
                .await;
            engine.load(&watched[0].id).unwrap().0
        };
        if case == "deadline" {
            assert_eq!(observe().await.status, "starting");
            clock.advance(Duration::from_secs(10 * 60));
            std::fs::write(root.join("delete-fails"), "").unwrap();
            assert_eq!(observe().await.status, "starting");
            std::fs::remove_file(root.join("delete-fails")).unwrap();
        }
        let run = observe().await;
        assert_eq!(run.status, "failed", "{case}");
        let reason = run.result_markdown.unwrap();
        for detail in details {
            assert!(reason.contains(detail), "{case}: {reason}");
        }
        let released = std::fs::read_to_string(root.join("http.log"))
            .unwrap()
            .lines()
            .any(|line| line == "POST /sandboxes/box/lease/release");
        assert_eq!(released, case == "deadline", "{case}");
        let _ = std::fs::remove_file(root.join("box-released"));
    }
}

#[tokio::test]
async fn openresearch_box_lost_after_contact_keeps_the_service_reason() {
    use super::openresearch::{LegacyTransport, OpenResearch};
    use super::ssh::Ssh;
    use crate::engine::transport::{Auth, Config, HostClass, HostConn, HostState, Status};

    let Some(root) = isolated().await else {
        return;
    };
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    // `details` are the ssh cause and the service's code and words the reason must keep.
    for (case, sandbox, reached, status, details) in [
        (
            "dead",
            Some(json!({"status":"dead", "provisionWarnings":"the provider reclaimed the host"})),
            true,
            "lost",
            &["dead", "the provider reclaimed the host"][..],
        ),
        (
            "failed",
            Some(
                json!({"status":"failed", "failureCode":"host_lost", "failureMessage":"The provider lost the host."}),
            ),
            true,
            "lost",
            &["host_lost", "The provider lost the host."],
        ),
        ("unreached", None, false, "failed", &["Connection refused"]),
        (
            "unreached dead",
            Some(json!({"status":"dead", "provisionWarnings":"the provider reclaimed the host"})),
            false,
            "failed",
            &[
                "Connection refused",
                "dead",
                "the provider reclaimed the host",
            ],
        ),
        (
            "unreached offline",
            Some(json!({"status":"offline", "provisionWarnings":"host unreachable"})),
            false,
            "failed",
            &["Connection refused", "offline", "host unreachable"],
        ),
    ] {
        let dir = root.join(case);
        let clock = Arc::new(TestClock::new());
        let mut engine = engine(&dir);
        engine.clock = clock.clone();
        let engine = Arc::new(engine);
        let watched = [box_run(
            &engine,
            json!({"sshHost":"unused", "sshPort":22, "sshUser":"root"}),
        )];
        clock.advance(Duration::from_secs(60));
        let store = Store::open_at(dir.join("store")).unwrap();
        store
            .seed_engine_host(
                "box",
                "/root",
                Some(HostClass::Key),
                &HostState {
                    status: Status::NeedsAuth,
                    since_ms: clock.wall_ms(),
                    last_live_ms: reached.then(|| clock.wall_ms() - 1),
                    cause: Some(
                        if reached {
                            "Host key verification failed"
                        } else {
                            "Connection refused"
                        }
                        .into(),
                    ),
                    ..HostState::default()
                },
            )
            .unwrap();
        let auth = Auth::new(&store, "box").unwrap();
        let conn = HostConn::start_shared(
            Config {
                alias: "box".into(),
                root: "/root".into(),
                ssh_config: Some(dir.join("never-connect")),
                control_dir: dir.join("cm"),
                sharing_home: None,
                binary_dir: None,
                askpass_executable: std::env::current_exe().unwrap(),
            },
            store,
            clock.clone(),
            auth,
        )
        .unwrap();
        let transport = Arc::new(LegacyTransport::new(
            Arc::new(Ssh::new("box".into(), "/root".into(), conn)),
            dir.join("logs"),
        ));
        let adapter = OpenResearch::new(engine.clone(), Arc::new(move |_| Ok(transport.clone())));
        match &sandbox {
            Some(sandbox) => std::fs::write(root.join("sandbox.json"), sandbox.to_string()),
            None => std::fs::write(root.join("box-released"), ""),
        }
        .unwrap();
        std::fs::write(root.join("http.log"), "").unwrap();
        engine
            .observe_runs(&adapter, &watched, Duration::ZERO)
            .await;
        let (run, _) = engine.load(&watched[0].id).unwrap();
        assert_eq!(run.status, status, "{case}");
        let reason = run.result_markdown.unwrap();
        for detail in details {
            assert!(reason.contains(detail), "{case}: {reason}");
        }
        let released = std::fs::read_to_string(root.join("http.log"))
            .unwrap()
            .lines()
            .any(|line| line == "POST /sandboxes/box/lease/release");
        assert_eq!(released, sandbox.is_some(), "{case}");
        let _ = std::fs::remove_file(root.join("sandbox.json"));
        let _ = std::fs::remove_file(root.join("box-released"));
    }
}

#[tokio::test]
async fn openresearch_launch_checks_the_key_is_usable_without_a_prompt() {
    let Some(root) = isolated().await else {
        return;
    };
    std::env::set_var("HOME", root.join("home"));
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    let mut args = crate::compute::tests::tinker_args();
    args.backend = Some("openresearch".into());
    args.flavor = Some("cpu5c".into());
    args.org = Some("org".into());

    local_key(&root, "secret");
    let launched = crate::local::openresearch::launch_spec(&args).await;
    assert_eq!(
        launched.is_ok(),
        cfg!(target_os = "macos"),
        "{:?}",
        launched.as_ref().err()
    );
    if let Err(error) = launched {
        let error = error.to_string();
        assert!(error.contains("~/.ssh/id_ed25519 "), "{error}");
        assert!(error.contains("ssh-add"), "{error}");
    }

    // ssh offers the box a registered key from outside ~/.ssh, so the locked one doesn't matter.
    let home = crate::config::home_dir().unwrap();
    let work = home.join("keys").join("work");
    std::fs::create_dir_all(work.parent().unwrap()).unwrap();
    assert!(crate::process::command("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&work)
        .status()
        .unwrap()
        .success());
    let registered = std::fs::read_to_string(home.join(".ssh").join("id_ed25519.pub")).unwrap();
    let working = std::fs::read_to_string(work.with_extension("pub")).unwrap();
    std::fs::write(
        root.join("ssh-keys.json"),
        json!({"sshKeys":[
            {"id":"old","name":"old","publicKey":registered.trim()},
            {"id":"work","name":"work","publicKey":working.trim()},
        ]})
        .to_string(),
    )
    .unwrap();
    let config = home.join(".ssh").join("config");
    std::fs::write(&config, "Host *\n  IdentityFile ~/keys/work\n").unwrap();
    crate::local::openresearch::launch_spec(&args)
        .await
        .unwrap();

    // Which key ssh offers depends on the box, so the locked key only warns.
    local_key(&root, "secret");
    std::fs::write(&config, "Host *\n  IdentityFile ~/keys/%r\n").unwrap();
    crate::local::openresearch::launch_spec(&args)
        .await
        .unwrap();
    std::fs::write(
        &config,
        "Match host 203.0.113.7\n  IdentityFile ~/keys/work\n",
    )
    .unwrap();
    crate::local::openresearch::launch_spec(&args)
        .await
        .unwrap();
    std::fs::remove_file(&config).unwrap();

    // An agent orx can't reach may hold the key.
    std::env::set_var("SSH_AUTH_SOCK", root.join("no-agent.sock"));
    crate::local::openresearch::launch_spec(&args)
        .await
        .unwrap();
    std::env::remove_var("SSH_AUTH_SOCK");
    local_key(&root, "");
    crate::local::openresearch::launch_spec(&args)
        .await
        .unwrap();
}

#[tokio::test]
async fn unsupported_leases_are_deleted_or_keep_the_known_box_handle() {
    let Some(root) = isolated().await else {
        return;
    };
    // Paces the box deletion retries this test makes fail.
    std::env::set_var("ORX_TEST_SPEEDUP", "100");
    let creds = crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    };
    crate::config::save_credentials(&creds).await.unwrap();
    let engine = Arc::new(engine(&root));
    let backend = super::openresearch::OpenResearch::new(
        engine.clone(),
        Arc::new(|_| panic!("no SSH before a valid handle")),
    );
    let metadata = BackendDescriptor::parse(r#"{"kind":"openresearch_job"}"#).unwrap();
    for (id, lease, deletion_fails, timeout, max_active_seconds) in [
        ("missing", Value::Null, false, None, 15_300),
        (
            "unsupported",
            json!({"version":2,"token":"fake"}),
            false,
            Some(60),
            960,
        ),
        ("retained", Value::Null, true, Some(604_800), 604_800),
    ] {
        let launch = LaunchSpec {
            kind: "openresearch_job".into(),
            target: Some("org".into()),
            options: json!({"flavor":"cpu5c", "timeout_seconds":timeout}),
        };
        let response = json!({"sandbox":{"id":id,"organizationId":"org","status":"online","machineType":"cpu","updatedAt":"now"},"rentalLease":lease});
        std::fs::write(root.join("sandbox-response.json"), response.to_string()).unwrap();
        if deletion_fails {
            std::fs::write(root.join("delete-fails"), "").unwrap();
        }
        engine
            .submit(&backend, pending(id, &metadata, &launch))
            .await
            .unwrap();
        let request: Value =
            serde_json::from_slice(&std::fs::read(root.join("sandbox-request.json")).unwrap())
                .unwrap();
        assert_eq!(
            request["rentalLease"]["maxActiveSeconds"],
            max_active_seconds
        );
        let (run, _) = engine.load(id).unwrap();
        if deletion_fails {
            assert_eq!(run.status, "starting");
            let Some(Handle::OpenResearch { descriptor }) = handle_of(&run) else {
                panic!("lost paid box id");
            };
            assert_eq!(descriptor.job_id.as_deref(), Some(id));
            assert_eq!(descriptor.timeout_secs, timeout);
            assert!(descriptor.rental_lease.is_none());
        } else {
            assert_eq!(run.status, "failed");
            assert!(run.result_markdown.unwrap().contains("box was deleted"));
        }
        let requests = std::fs::read_to_string(root.join("http.log")).unwrap();
        assert!(requests
            .lines()
            .any(|line| line == format!("DELETE /sandboxes/{id}")));
    }
}

#[tokio::test]
async fn failed_submission_fences_kill_launchers_before_stdin_is_released() {
    let Some(root) = isolated().await else {
        return;
    };
    let python = crate::config::config_dir().join("envs/huggingface/bin/python");
    script(
        &python,
        &format!(
            r#"#!/bin/sh
case "$2" in *'assert hasattr'*) exit 0;; esac
cat >/dev/null
sleep 0.2
touch '{}/leaked-hf'
"#,
            root.display()
        ),
    );
    script(
        &root.join("bin/kubectl"),
        &format!(
            r#"#!/bin/sh
case "$*" in *--dry-run=client*) cat; exit 0;; esac
cat >/dev/null
sleep 0.2
touch '{}/leaked-k8s'
"#,
            root.display()
        ),
    );
    for launcher in ["modal", "vercel"] {
        script(
            &root.join(format!("bin/{launcher}-python")),
            &format!(
                "#!/bin/sh\ncat >/dev/null\nsleep 0.2\ntouch '{}/leaked-{launcher}'\n",
                root.display()
            ),
        );
    }
    let engine = engine(&root);
    let fence = Fence {
        store: &engine.store,
        run_id: "not-submitting",
        attempt: 1,
        taken_at: 1,
    };
    let archive = root.join("source.tar");
    std::fs::write(&archive, "snapshot").unwrap();
    let hf = serde_json::from_value(json!({"command":["true"],"docker_image":"python","flavor":"cpu","environment":{},"secrets":{},"timeout_seconds":60,"labels":{}})).unwrap();
    let error = crate::jobs::huggingface::run_job_with_source(
        "fake",
        "org",
        &hf,
        &archive,
        "digest",
        Some(&fence),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("no longer awaits"), "{error}");
    let manifest = json!({"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"run"},"spec":{"template":{"spec":{"containers":[{"name":"run","command":["bash","-c","eval \"$ORX_SCRIPT\""]}]}}}});
    let spec = serde_json::from_value(json!({"manifest":manifest.to_string(),"script":"true","run_token":"run","env":{},"timeout_seconds":60,"labels":{},"pod_env":[],"source_archive":archive})).unwrap();
    let error = crate::jobs::kubernetes::run_manifest(None, "default", &spec, Some(&fence))
        .await
        .err()
        .expect("unfenced creation succeeded");
    assert!(error.to_string().contains("no longer awaits"), "{error}");
    let modal = serde_json::from_value(json!({"script":"true","image":"python","gpu":null,"cpu":null,"memory":null,"env":{},"timeout_seconds":60,"app":"openresearch","tags":{},"source_archive":null})).unwrap();
    let error = crate::jobs::modal::run_job(&modal, Some(&fence))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no longer awaits"), "{error}");
    let vercel = serde_json::from_value(json!({"name":"orx-run","script":"true","image":"python","vcpus":null,"env":{},"timeout_seconds":60,"session_cap":2700,"region":null,"tags":{},"source_archive":null})).unwrap();
    let error = crate::jobs::vercel::run_job(&vercel, Some(&fence))
        .await
        .err()
        .expect("unfenced creation succeeded");
    assert!(error.to_string().contains("no longer awaits"), "{error}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    for leak in ["leaked-hf", "leaked-k8s", "leaked-modal", "leaked-vercel"] {
        assert!(!root.join(leak).exists(), "{leak}");
    }
}

#[tokio::test]
async fn chatty_modal_logs_publish_a_partial_page_and_reuse_it() {
    let Some(root) = isolated().await else {
        return;
    };
    let python = root.join("bin/modal-python");
    script(
        &python,
        &format!(
            r#"#!/bin/sh
printf 'download\n' >> '{}/downloads'
while :; do printf '["", 5]\nline\n'; sleep 0.01; done
"#,
            root.display()
        ),
    );
    let backend = super::api::Adapter::new(
        ModalAccount,
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    let run = WatchedRun {
        id: "chatty".into(),
        handle: crate::engine::adapters::api::test_handle(
            "modal_job",
            None,
            None,
            vec!["sandbox".into()],
        ),
    };
    let page = backend
        .read_log(&run, LogFile::Log, ByteRange { start: 0, end: 100 }, None)
        .await
        .unwrap();
    let LogPage::Data { bytes, ident, .. } = page else {
        panic!("no live log page")
    };
    assert!(!bytes.is_empty());
    for _ in 0..5 {
        backend
            .read_log(
                &run,
                LogFile::Log,
                ByteRange { start: 0, end: 100 },
                Some(&ident),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(root.join("downloads"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[tokio::test]
async fn live_modal_eof_reconnects_and_final_collection_keeps_later_output() {
    use crate::commands::up::api_v2::Commands;

    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/modal-python"),
        &format!(
            r#"#!/bin/sh
cd '{}'
case "$3" in
status) cat state ;;
logs)
  if ! mkdir active-stream; then touch overlapping-streams; exit 1; fi
  trap 'rmdir active-stream' EXIT
  from=${{5:-0}}
  cp history snapshot
  size=$(wc -c < snapshot | tr -d ' ')
  if [ "$size" -lt "$from" ]; then size=$from; fi
  printf '["%s", %d]\n' "$from" $((size - from))
  tail -c +$((from + 1)) snapshot
  ;;
esac
"#,
            root.display()
        ),
    );
    std::fs::write(root.join("state"), r#"{"stage":"RUNNING"}"#).unwrap();
    std::fs::write(root.join("history"), "first\n").unwrap();
    let engine = Arc::new(engine(&root));
    let clock = Arc::new(TestClock::new());
    let runtime = crate::engine::runtime::Runtime::new(
        engine.clone(),
        clock.clone(),
        root.join("store"),
        None,
    );
    let handle = super::api::test_handle("modal_job", None, None, vec!["sandbox".into()]);
    let row = crate::store::StoredRun {
        id: "run".into(),
        experiment_id: "exp".into(),
        project_id: "proj".into(),
        status: "running".into(),
        backend_json: crate::engine::handle::backend_json(
            "run",
            Some(&handle),
            &BackendDescriptor::parse(r#"{"kind":"modal_job"}"#).unwrap(),
        )
        .unwrap(),
        command: "true".into(),
        created_at: 1,
        updated_at: 1,
        ended_at: None,
        exit_code: None,
        commit_sha: None,
        result_markdown: None,
        cancel_requested: false,
        cancel_reason: None,
        chat_session_id: None,
    };
    engine.store().upsert_run(&row).unwrap();
    let backend = runtime.backend(&row).unwrap();
    let run = WatchedRun {
        id: "run".into(),
        handle,
    };
    let whole = ByteRange {
        start: 0,
        end: orx_host_proto::MAX_PAGE_LEN,
    };
    backend.drain_log(&run).await.unwrap();
    let observations = backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;
    assert!(observations.contains(&("run".into(), Observation::Running { started_at: None })));
    std::fs::write(root.join("history"), "first\nsecond\n").unwrap();
    clock.advance(Duration::from_secs(10));
    backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;
    backend.drain_log(&run).await.unwrap();
    let page = backend
        .read_log(&run, LogFile::Log, whole, None)
        .await
        .unwrap();
    assert!(matches!(page, LogPage::Data { bytes, .. } if bytes == b"first\nsecond\n"));
    std::fs::write(root.join("history"), "first\nsecond\nlast\n").unwrap();
    std::fs::write(root.join("state"), r#"{"stage":"COMPLETED"}"#).unwrap();
    clock.advance(Duration::from_secs(10));
    let observations = backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;
    assert!(observations
        .iter()
        .any(|(_, observation)| observation.ends_run()));
    engine
        .apply_observations(backend.as_ref(), &[run], observations)
        .await;
    runtime.start();
    tokio::time::timeout(Duration::from_secs(30), async {
        while engine.store().log_collection_pending("run").unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("final log was not kept");
    assert_eq!(
        std::fs::read(root.join("store/run-logs/run.log")).unwrap(),
        b"first\nsecond\nlast\n"
    );
    assert!(!root.join("overlapping-streams").exists());
    assert!(!root.join("active-stream").exists());
}

#[tokio::test]
async fn finished_modal_logs_drain_through_pauses_before_they_are_kept() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/modal-python"),
        "#!/bin/sh\nprintf '[\"\", 6]\\nfirst\\n'\nsleep 11\nprintf '[\"\", 5]\\nlast\\n'\n",
    );
    let backend = super::api::Adapter::new(
        ModalAccount,
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    let run = WatchedRun {
        id: "drained".into(),
        handle: crate::engine::adapters::api::test_handle(
            "modal_job",
            None,
            None,
            vec!["sandbox".into()],
        ),
    };
    let whole = ByteRange {
        start: 0,
        end: orx_host_proto::MAX_PAGE_LEN,
    };
    let LogPage::Data { bytes, .. } = backend
        .read_log(&run, LogFile::Log, whole.clone(), None)
        .await
        .unwrap()
    else {
        panic!("no live log page")
    };
    assert_eq!(bytes, b"first\n");
    backend.drain_log(&run).await.unwrap();
    let LogPage::Data { bytes, eof, .. } = backend
        .read_log(&run, LogFile::Log, whole, None)
        .await
        .unwrap()
    else {
        panic!("no drained log page")
    };
    assert!(eof);
    assert_eq!(bytes, b"first\nlast\n");
}

/// Modal launcher frames: each chunk with the point a reconnect replays it from.
fn modal_frames(chunks: &[(&str, &[u8])]) -> Vec<u8> {
    let mut frames = Vec::new();
    for (from, data) in chunks {
        frames.extend_from_slice(json!([from, data.len()]).to_string().as_bytes());
        frames.push(b'\n');
        frames.extend_from_slice(data);
    }
    frames
}

#[tokio::test]
async fn modal_logs_reopen_a_stalled_stream_mid_batch_without_losing_bytes() {
    use super::api::Api;
    let Some(root) = isolated().await else {
        return;
    };
    std::env::set_var("ORX_TEST_SPEEDUP", "120");
    script(
        &root.join("bin/modal-python"),
        &format!(
            r#"#!/bin/sh
cd '{}'
n=$(cat from.log 2>/dev/null | wc -l | tr -d ' ')
printf '%s\n' "$5" >> from.log
cat "frames/$n"
if [ -e "frames/$n.stall" ]; then exec sleep 600; fi
"#,
            root.display()
        ),
    );
    let record = format!("{{\"summary\":\"{}\"}}\n", "x".repeat(59_000)).into_bytes();
    std::fs::create_dir_all(root.join("frames")).unwrap();
    std::fs::write(
        root.join("frames/0"),
        modal_frames(&[("", b"A\n"), ("1", &record[..16 * 1024])]),
    )
    .unwrap();
    std::fs::write(root.join("frames/0.stall"), "").unwrap();
    // The reconnect regroups the batch it resumes inside.
    std::fs::write(
        root.join("frames/1"),
        modal_frames(&[
            ("1", &record[..20_000]),
            ("1", &record[20_000..]),
            ("2", b"B"),
        ]),
    )
    .unwrap();
    let run = WatchedRun {
        id: "run".into(),
        handle: super::api::test_handle("modal_job", None, None, vec!["sandbox".into()]),
    };
    let mut cursor = crate::jobs::modal::LogResume::default();
    let mut log = Vec::new();
    let closed = ModalAccount
        .logs(
            &run,
            &mut cursor,
            Duration::from_secs(30),
            &mut |bytes, _| log.extend_from_slice(bytes),
        )
        .await
        .unwrap();
    assert!(closed);
    assert_eq!(log, [b"A\n".as_slice(), &record, b"B"].concat());
    assert_eq!(
        std::fs::read_to_string(root.join("from.log")).unwrap(),
        "\n1\n"
    );
}

#[tokio::test]
async fn kubernetes_cancel_keeps_the_spool_without_an_attention_notice() {
    use super::api::Api;
    use crate::commands::up::api_v2::Commands;

    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/kubectl"),
        r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
root = Path(os.environ['ORX_API_ROOT'])
a = sys.argv[1:]
if a[0] == 'delete':
    assert '--cascade=foreground' in a and '--wait=false' in a
    (root/'deleting').touch()
elif a[:2] == ['get', 'job']:
    if (root/'gone').exists() or (root/'unexpected-deletion').exists():
        print('Error from server (NotFound): jobs.batch "job" not found', file=sys.stderr)
        sys.exit(1)
    metadata = {'deletionTimestamp': '2026-11-05T01:43:00Z'} if (root/'deleting').exists() else {}
    print(json.dumps({'metadata': metadata, 'status': {'active': 1}}))
elif a[:2] == ['get', 'pods']:
    metadata = {'name': 'pod'}
    if (root/'deleting').exists():
        metadata['deletionTimestamp'] = '2026-11-05T01:43:00Z'
    pods = [] if (root/'gone').exists() else [{'metadata': metadata, 'status': {'phase': 'Running'}}]
    print(json.dumps({'items': pods}))
elif a[0] == 'logs':
    assert not (root/'gone').exists()
    print('2026-11-05T01:42:00Z SMOKE_CANCEL_READY')
elif a[:3] == ['exec', '-n', 'default']:
    assert 'test' in a, 'the source was already staged'
else:
    raise Exception(a)
"#,
    );
    let engine = Arc::new(engine(&root));
    let clock = Arc::new(TestClock::new());
    let runtime = crate::engine::runtime::Runtime::new(
        engine.clone(),
        clock.clone(),
        root.join("store"),
        None,
    );
    let handle = super::api::test_handle("k8s_job", Some("default"), None, vec!["job/job".into()]);
    let row = crate::store::StoredRun {
        id: "run".into(),
        experiment_id: "exp".into(),
        project_id: "proj".into(),
        status: "running".into(),
        backend_json: crate::engine::handle::backend_json(
            "run",
            Some(&handle),
            &BackendDescriptor::parse(r#"{"kind":"k8s_job","namespace":"default"}"#).unwrap(),
        )
        .unwrap(),
        command: "sleep 180".into(),
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
    engine.store().upsert_run(&row).unwrap();
    engine.store().subscribe_run("run", "chat", 1).unwrap();
    let backend = runtime.backend(&row).unwrap();
    let run = WatchedRun {
        id: "run".into(),
        handle,
    };
    let account = K8sAccount::new(None, "default".into());
    for terminating in [false, true] {
        std::fs::write(root.join("unexpected-deletion"), "").unwrap();
        if terminating {
            std::fs::write(root.join("deleting"), "").unwrap();
        }
        assert!(matches!(
            account.inspect(&run).await.unwrap(),
            Observation::Unreconciled { .. }
        ));
    }
    std::fs::remove_file(root.join("unexpected-deletion")).unwrap();
    std::fs::remove_file(root.join("deleting")).unwrap();

    backend.drain_log(&run).await.unwrap();
    assert!(engine
        .cancel(backend.as_ref(), "run", "Dashboard stop")
        .await
        .unwrap());
    engine
        .observe_runs(
            backend.as_ref(),
            std::slice::from_ref(&run),
            Duration::from_secs(5),
        )
        .await;
    assert_eq!(engine.load("run").unwrap().0.status, "running");
    assert!(engine.store().run_notice_batch("chat").unwrap().is_empty());

    backend.drain_log(&run).await.unwrap();
    std::fs::write(root.join("gone"), "").unwrap();
    clock.advance(Duration::from_secs(10));
    engine
        .observe_runs(backend.as_ref(), &[run], Duration::from_secs(5))
        .await;
    assert_eq!(engine.load("run").unwrap().0.status, "cancelled");
    let notices = engine.store().run_notice_batch("chat").unwrap();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].kind, crate::store::TERMINAL);

    runtime.start();
    tokio::time::timeout(Duration::from_secs(30), async {
        while engine.store().log_collection_pending("run").unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancelled log was not kept");
    assert_eq!(
        std::fs::read(root.join("store/run-logs/run.log")).unwrap(),
        b"SMOKE_CANCEL_READY\n"
    );
    let ranges = crate::commands::up::api_v2::kept_ranges(&root.join("store"), "run").unwrap();
    assert!(ranges.iter().any(|range| range.eof));
}

#[tokio::test]
async fn kubernetes_missing_pods_close_logs_only_after_job_deletion() {
    use super::api::Api;
    let Some(root) = isolated().await else {
        return;
    };
    let run = WatchedRun {
        id: "k8s".into(),
        handle: crate::engine::adapters::api::test_handle(
            "k8s_job",
            Some("default"),
            None,
            vec!["job/job".into()],
        ),
    };
    for (get, expected_closed) in [
        ("echo 'Unable to connect to the server' >&2; exit 1", false),
        ("printf '{\"items\":[]}'", false),
        (
            r#"if test "$2" = job; then echo 'Error from server (Unauthorized)' >&2; exit 1; fi; printf '{"items":[]}'"#,
            false,
        ),
        (
            r#"if test "$2" = job; then echo 'Error from server (NotFound)' >&2; exit 1; fi; printf '{"items":[]}'"#,
            true,
        ),
    ] {
        script(
            &root.join("bin/kubectl"),
            &format!("#!/bin/sh\ncase \"$1\" in\nget) {get} ;;\nlogs) exit 0 ;;\nesac\n"),
        );
        let account = K8sAccount::new(None, "default".into());
        let mut cursor = crate::jobs::kubernetes::LogResume::default();
        let closed = account
            .logs(&run, &mut cursor, Duration::from_secs(5), &mut |_, _| {})
            .await;
        assert_eq!(
            matches!(closed, Ok(true)),
            expected_closed,
            "{get}: {closed:?}"
        );
        let backend =
            super::api::Adapter::new(account, Arc::new(TestClock::new()), root.join("spools"));
        assert_eq!(
            backend.drain_log(&run).await.is_ok(),
            expected_closed,
            "{get}"
        );
    }
}

#[tokio::test]
async fn empty_stream_timeouts_surface_for_hf_modal_and_kubernetes() {
    use super::api::Api;
    let Some(root) = isolated().await else {
        return;
    };
    // Paces the page wait the reads below give up on.
    std::env::set_var("ORX_TEST_SPEEDUP", "2");
    let budget = crate::config::paced(Duration::from_secs(1));
    std::fs::write(root.join("slow-logs"), "").unwrap();
    script(
        &root.join("bin/modal-python"),
        "#!/bin/sh\nprintf 'download\n' >> \"$ORX_API_ROOT/downloads\"\nexec sleep 5\n",
    );
    script(
        &root.join("bin/kubectl"),
        r#"#!/bin/sh
case "$1" in
get) printf '{"items":[{"metadata":{"name":"pod-1"}}]}' ;;
logs) exec sleep 5 ;;
esac
"#,
    );
    let hf = HfAccount {
        namespace: "org".into(),
    };
    let hf_run = WatchedRun {
        id: "hf".into(),
        handle: crate::engine::adapters::api::test_handle(
            "hf_job",
            Some("org"),
            None,
            vec!["job".into()],
        ),
    };
    let mut hf_cursor = crate::jobs::huggingface::LogResume::default();
    assert!(hf
        .logs(&hf_run, &mut hf_cursor, budget, &mut |_, _| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert_eq!(hf_cursor, Default::default());
    let modal_run = WatchedRun {
        id: "modal".into(),
        handle: crate::engine::adapters::api::test_handle(
            "modal_job",
            None,
            None,
            vec!["sandbox".into()],
        ),
    };
    let mut modal_cursor = crate::jobs::modal::LogResume::default();
    assert!(ModalAccount
        .logs(&modal_run, &mut modal_cursor, budget, &mut |_, _| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert_eq!(modal_cursor, Default::default());
    let k8s = K8sAccount::new(None, "default".into());
    let k8s_run = WatchedRun {
        id: "k8s".into(),
        handle: crate::engine::adapters::api::test_handle(
            "k8s_job",
            Some("default"),
            None,
            vec!["job/job".into()],
        ),
    };
    assert!(k8s
        .logs(
            &k8s_run,
            &mut crate::jobs::kubernetes::LogResume::default(),
            budget,
            &mut |_, _| {}
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    let backend = super::api::Adapter::new(
        ModalAccount,
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    for _ in 0..2 {
        assert!(backend
            .read_log(
                &modal_run,
                LogFile::Log,
                ByteRange { start: 0, end: 1 },
                None
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("timed out"));
    }
    assert_eq!(
        std::fs::read_to_string(root.join("downloads"))
            .unwrap()
            .lines()
            .count(),
        2
    );
}

#[tokio::test]
async fn vercel_uses_api_handles_and_adopts_legacy_sandboxes() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/vercel-python"),
        &format!(
            r#"#!/bin/sh
case "$3" in
  submit) cat > '{root}/submitted.json'; printf '{{"sandboxName":"orx-run","processId":"proc-1"}}' ;;
  status)
    [ "$4" = orx-run ] && [ "$5" = proc-1 ] || exit 1
    if [ -f '{root}/cancelled' ]; then
      printf '{{"stage":"CANCELED"}}'
    else
      printf '{{"stage":"RUNNING"}}'
    fi ;;
  logs) printf 'first\nlast\n' ;;
  cancel) touch '{root}/cancelled'; printf '{{"missing":false}}' ;;
  finish) touch '{root}/destroyed' ;;
  *) exit 1 ;;
esac
"#,
            root = root.display()
        ),
    );
    let engine = engine(&root);
    let metadata = BackendDescriptor::parse(r#"{"kind":"vercel_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "vercel_job".into(),
        target: None,
        options: json!({
            "name":"ignored", "script":"true", "image":"python", "vcpus":2,
            "env":{"VERCEL_TOKEN":"secret", "VERCEL_OIDC_TOKEN":"secret", "KEEP":"yes"},
            "timeout_seconds":60, "session_cap":2700, "region":null, "tags":{},
            "source_archive":null
        }),
    };
    let adapter = (super::api::provider("vercel_job").unwrap().build)(
        &crate::engine::handle::ApiAccount {
            target: None,
            context: None,
        },
        Arc::new(TestClock::new()),
        root.join("spools"),
    )
    .unwrap();
    engine
        .submit(adapter.as_ref(), pending("run", &metadata, &launch))
        .await
        .unwrap();
    let run = engine.load("run").unwrap().0;
    let watched = WatchedRun {
        id: run.id.clone(),
        handle: handle_of(&run).unwrap(),
    };
    assert!(matches!(&watched.handle, Handle::Api { provider, ids, .. }
        if provider == "vercel_job" && ids == &["orx-run", "proc-1"]));
    let submitted: Value =
        serde_json::from_slice(&std::fs::read(root.join("submitted.json")).unwrap()).unwrap();
    assert_eq!(submitted["name"], "orx-run");
    assert_eq!(submitted["tags"]["or_run"], "run");
    assert_eq!(submitted["env"]["KEEP"], "yes");
    assert!(!submitted["env"]
        .as_object()
        .unwrap()
        .keys()
        .any(|key| key.starts_with("VERCEL_")));
    adapter.drain_log(&watched).await.unwrap();
    let page = adapter
        .read_log(
            &watched,
            LogFile::Log,
            ByteRange { start: 0, end: 11 },
            None,
        )
        .await
        .unwrap();
    assert!(matches!(page, LogPage::Data { bytes, .. } if bytes == b"first\nlast\n"));
    engine
        .cancel(adapter.as_ref(), "run", "stop")
        .await
        .unwrap();
    engine
        .observe_runs(
            adapter.as_ref(),
            std::slice::from_ref(&watched),
            Duration::from_secs(1),
        )
        .await;
    assert_eq!(engine.load("run").unwrap().0.status, "cancelled");
    adapter.finish(&watched).await.unwrap();
    assert!(root.join("destroyed").exists());
    let legacy = WatchedRun {
        id: "legacy".into(),
        handle: Handle::Legacy {
            descriptor: Box::new(
                BackendDescriptor::parse(
                    r#"{"kind":"vercel_job","namespace":"orx-run","jobId":"proc-1"}"#,
                )
                .unwrap(),
            ),
        },
    };
    assert!(matches!(
        adapter.cancel(&legacy).await,
        CancelOutcome::Pending
    ));
    adapter.finish(&legacy).await.unwrap();
    let mut legacy_row = run.clone();
    legacy_row.id = legacy.id.clone();
    legacy_row.updated_at = 1;
    legacy_row.backend_json = serde_json::json!({
        "kind":"vercel_job", "namespace":"orx-run", "jobId":"proc-1"
    })
    .to_string();
    engine.store().upsert_run(&legacy_row).unwrap();
    let adopted = engine.scan_legacy().unwrap();
    assert!(adopted.iter().any(|(id, adoption)| {
        id == "legacy"
            && matches!(adoption,
            crate::engine::legacy::Adoption::Native { kind } if kind == "vercel_job")
    }));
    let intent = RunIntent {
        run,
        launch,
        spec_digest: "digest".into(),
        attempt: 1,
    };
    assert!(matches!(
        adapter.recover(&intent).await,
        Recovery::Unknown { .. }
    ));
}

#[tokio::test]
async fn providers_reject_credentials_and_preserve_numeric_exits() {
    let Some(root) = isolated().await else {
        return;
    };
    for launcher in ["modal-python", "vercel-python", "kubectl"] {
        script(
            &root.join("bin").join(launcher),
            &format!(
                r#"#!/bin/sh
cd '{}'
if test -f inspect-error; then cat inspect-error >&2; exit 1; fi
case "$*" in
  *"get pods"*) cat pods.json ;;
  *) cat inspect-response.json ;;
esac
"#,
                root.display()
            ),
        );
    }
    let engine = engine(&root);
    let endpoint = std::env::var("HF_ENDPOINT").unwrap();
    for (kind, response, rejection) in [
        ("ray_job", json!({"status":"FAILED", "driver_exit_code":7}), ""),
        ("hf_job", json!({"id":"job", "status":{"stage":"ERROR", "message":"Job failed with exit code: 7. Reason: Error"}}), ""),
        ("vercel_job", json!({"stage":"ERROR", "exitCode":7}), "vercel.sandbox._internal.errors.SandboxApiError: HTTP 403: Not authorized (code=forbidden)"),
        ("modal_job", json!({"stage":"ERROR", "exitCode":7}), "modal.exception.AuthError: Token is not authorized"),
        ("k8s_job", json!({"status":{"conditions":[{"type":"Failed","status":"True","reason":"BackoffLimitExceeded"}]}}), "Error from server (Forbidden): jobs.batch is forbidden"),
    ] {
        let target = if kind == "ray_job" {
            endpoint.as_str()
        } else {
            "org"
        };
        let ids = match kind {
            "vercel_job" => vec!["box".into(), "process".into()],
            "k8s_job" => vec!["job/job".into()],
            _ => vec!["job".into()],
        };
        let handle = super::api::test_handle(kind, Some(target), None, ids);
        let metadata = BackendDescriptor::parse(&json!({"kind":kind}).to_string()).unwrap();
        engine.store().upsert_run(&crate::store::StoredRun {
            id: kind.into(),
            experiment_id: "exp".into(),
            project_id: "proj".into(),
            status: "running".into(),
            backend_json: crate::engine::handle::backend_json(kind, Some(&handle), &metadata).unwrap(),
            command: "exit 7".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            cancel_reason: None,
            chat_session_id: None,
        }).unwrap();

        let clock = Arc::new(TestClock::new());
        let provider = super::api::provider(kind).unwrap();
        let adapter = (provider.build)(
            &crate::engine::handle::ApiAccount {
                target: Some(target.into()),
                context: None,
            },
            clock.clone(),
            root.join("spools"),
        ).unwrap();
        let runs = [WatchedRun { id: kind.into(), handle }];
        std::fs::write(root.join("inspect-response.json"), response.to_string()).unwrap();
        std::fs::write(root.join("pods.json"), json!({"items":[{
            "metadata":{"annotations":{"kubectl.kubernetes.io/default-container":"main"}},
            "status":{"containerStatuses":[
                {"name":"sidecar", "state":{"terminated":{"exitCode":99}}},
                {"name":"main", "state":{"terminated":{"exitCode":7}}}
            ]}
        }]}).to_string()).unwrap();

        let rejections = match kind {
            "ray_job" | "hf_job" => vec![("401", true), ("403", true), ("503", false)],
            "vercel_job" => vec![
                ("SandboxApiError: HTTP 401: Unauthorized", true),
                (rejection, true),
                ("connection timed out", false),
            ],
            "modal_job" => vec![
                ("GRPCError: (<Status.UNAUTHENTICATED: 16>, 'Token rejected')", true),
                ("GRPCError: (<Status.PERMISSION_DENIED: 7>, 'Access denied')", true),
                (rejection, true),
                ("connection timed out", false),
            ],
            _ => vec![
                ("Error from server (Unauthorized): You must be logged in to the server", true),
                (rejection, true),
                ("connection timed out", false),
            ],
        };
        for (rejection, needs_auth) in rejections {
            let file = if kind == "ray_job" || kind == "hf_job" {
                "inspect-status"
            } else {
                "inspect-error"
            };
            std::fs::write(root.join(file), rejection).unwrap();
            let observations = adapter.observe(&runs, Duration::from_secs(5)).await;
            assert_eq!(observations.len(), 1, "{kind}: {observations:?}");
            let (_, observation) = observations.into_iter().next().unwrap();
            assert!(
                matches!(
                    (needs_auth, &observation),
                    (true, Observation::NeedsAuth { .. }) | (false, Observation::Unreachable { .. })
                ),
                "{kind}: {observation:?}"
            );
            engine.observe(kind, observation).unwrap();
            assert_eq!(engine.load(kind).unwrap().0.status, "running");
            assert_eq!(
                matches!(engine.load(kind).unwrap().1.contact, crate::store::Contact::NeedsAuth { .. }),
                needs_auth
            );
            std::fs::remove_file(root.join(file)).unwrap();
            clock.advance(Duration::from_secs(10));
        }
        for (_, observation) in adapter.observe(&runs, Duration::from_secs(5)).await {
            engine.observe(kind, observation).unwrap();
        }

        let row = engine.load(kind).unwrap().0;
        assert_eq!(row.status, "failed", "{kind}");
        assert_eq!(row.exit_code, Some(7), "{kind}");

        let cancel_id = format!("{kind}-cancel");
        let cancel_runs = [WatchedRun {
            id: cancel_id.clone(),
            handle: runs[0].handle.clone(),
        }];
        let pending_cancel = crate::store::StoredRun {
            id: cancel_id.clone(),
            status: "running".into(),
            exit_code: None,
            ended_at: None,
            ..row
        };
        engine.store().upsert_run(&pending_cancel).unwrap();
        assert!(engine.store().accept_run_cancel(&cancel_id, Some("Dashboard stop")).unwrap().is_some());
        let response = match kind {
            "ray_job" => json!({"status":"FAILED", "driver_exit_code":137}),
            "hf_job" => json!({"id":"job", "status":{"stage":"ERROR", "message":"Job failed with exit code: 137. Reason: Error"}}),
            "k8s_job" => response,
            _ => json!({"stage":"ERROR", "exitCode":137}),
        };
        std::fs::write(root.join("inspect-response.json"), response.to_string()).unwrap();
        clock.advance(Duration::from_secs(10));
        for (id, observation) in adapter.observe(&cancel_runs, Duration::from_secs(5)).await {
            engine.observe(&id, observation).unwrap();
        }
        let row = engine.load(&cancel_id).unwrap().0;
        assert_eq!(row.status, "cancelled", "{kind}");
        assert_eq!(row.exit_code, Some(if kind == "k8s_job" { 7 } else { 137 }), "{kind}");
    }
}

/// A provider's "no such job" ends the run only when it proves deletion: cancelled after an
/// accepted cancel, lost otherwise, and a cancel it answers is delivered. Otherwise the run stays
/// open and says why, and the cancel stays undelivered.
#[tokio::test]
async fn provider_404s_end_runs_only_when_they_prove_deletion() {
    let Some(root) = isolated().await else {
        return;
    };
    for launcher in ["modal-python", "vercel-python"] {
        script(
            &root.join("bin").join(launcher),
            "#!/bin/sh\ncase \"$3\" in status|cancel) printf '{\"missing\":true}' ;; *) exit 1 ;; esac\n",
        );
    }
    std::fs::write(root.join("inspect-response.json"), "{}").unwrap();
    std::fs::write(root.join("inspect-status"), "404").unwrap();
    std::fs::write(root.join("cancel-status"), "404").unwrap();
    let engine = engine(&root);
    let endpoint = std::env::var("HF_ENDPOINT").unwrap();
    let member = r#"{"name":"ada-lovelace","orgs":[{"name":"org"}]}"#;
    let outsider = r#"{"name":"ada-lovelace","orgs":[{"name":"elsewhere"}]}"#;
    let explains = |kind: &str, why: &str| match kind {
        "hf_job" => why.contains("ada-lovelace") && why.contains(" org "),
        _ => why.contains("doesn't prove"),
    };
    for (case, kind, whoami, proven) in [
        ("hf-member", "hf_job", member, true),
        ("hf-outsider", "hf_job", outsider, false),
        ("ray", "ray_job", member, true),
        ("modal", "modal_job", member, false),
        ("vercel", "vercel_job", member, false),
    ] {
        std::fs::write(root.join("whoami.json"), whoami).unwrap();
        let target = match kind {
            "ray_job" => Some(endpoint.as_str()),
            "hf_job" => Some("org"),
            _ => None,
        };
        let ids = match kind {
            "vercel_job" => vec!["box".into(), "process".into()],
            _ => vec!["job".into()],
        };
        let handle = super::api::test_handle(kind, target, None, ids);
        let metadata = BackendDescriptor::parse(&json!({"kind":kind}).to_string()).unwrap();
        let cancelled = format!("{case}-cancel");
        let runs = [case.to_string(), cancelled.clone()].map(|id| WatchedRun {
            id,
            handle: handle.clone(),
        });
        for run in &runs {
            engine
                .store()
                .upsert_run(&crate::store::StoredRun {
                    id: run.id.clone(),
                    experiment_id: "exp".into(),
                    project_id: "proj".into(),
                    status: "running".into(),
                    backend_json: crate::engine::handle::backend_json(
                        kind,
                        Some(&handle),
                        &metadata,
                    )
                    .unwrap(),
                    command: "true".into(),
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
        let adapter = (super::api::provider(kind).unwrap().build)(
            &crate::engine::handle::ApiAccount {
                target: target.map(str::to_owned),
                context: None,
            },
            Arc::new(TestClock::new()),
            root.join("spools"),
        )
        .unwrap();
        let cancel = adapter.cancel(&runs[1]).await;
        match &cancel {
            CancelOutcome::Pending => assert!(proven, "{case}"),
            CancelOutcome::Undelivered { detail } => {
                assert!(!proven && explains(kind, detail), "{case}: {detail}");
            }
            other => panic!("{case}: {other:?}"),
        }
        assert!(engine
            .store()
            .accept_run_cancel(&cancelled, Some("Dashboard stop"))
            .unwrap()
            .is_some());
        for (id, observation) in adapter.observe(&runs, Duration::from_secs(5)).await {
            engine.observe(&id, observation).unwrap();
        }
        if proven {
            assert_eq!(engine.load(case).unwrap().0.status, "lost", "{case}");
            assert_eq!(
                engine.load(&cancelled).unwrap().0.status,
                "cancelled",
                "{case}"
            );
            continue;
        }
        for id in [case, cancelled.as_str()] {
            let (run, state) = engine.load(id).unwrap();
            assert_eq!(run.status, "running", "{id}");
            let why = match (kind, state.contact) {
                ("hf_job", crate::store::Contact::NeedsAuth { cause, .. }) => cause,
                (
                    "modal_job" | "vercel_job",
                    crate::store::Contact::Unreconciled { detail, .. },
                ) => detail,
                (_, contact) => panic!("{id}: {contact:?}"),
            };
            assert!(explains(kind, &why), "{id}: {why}");
        }
    }
}

#[tokio::test]
async fn hf_exit_evidence_requires_the_provider_failure_format() {
    let Some(root) = isolated().await else {
        return;
    };
    let account = HfAccount {
        namespace: "org".into(),
    };
    let run = WatchedRun {
        id: "run".into(),
        handle: super::api::test_handle("hf_job", Some("org"), None, vec!["job".into()]),
    };
    for (stage, message, expected) in [
        (
            "ERROR",
            "Job failed with exit code: 7. Reason: Error",
            Some(7),
        ),
        (
            "ERROR",
            "Job failed with exit code: -7. Reason: Error",
            Some(-7),
        ),
        (
            "ERROR",
            "Job failed with exit code: seven. Reason: Error",
            None,
        ),
        ("ERROR", "Could not start: image unavailable", None),
    ] {
        std::fs::write(
            root.join("inspect-response.json"),
            json!({"id":"job", "status":{"stage":stage, "message":message}}).to_string(),
        )
        .unwrap();
        let observation = super::api::Api::inspect(&account, &run).await.unwrap();
        match expected {
            Some(code) => assert_eq!(
                observation,
                Observation::Exited {
                    finished_at: None,
                    code: Some(code),
                    signal: None
                }
            ),
            None => assert_eq!(
                observation,
                Observation::Failed {
                    finished_at: None,
                    reason: message.into()
                }
            ),
        }
    }
}

#[tokio::test]
async fn hf_refusals_fail_the_run_with_the_step_that_refused() {
    let Some(root) = isolated().await else {
        return;
    };
    let Ok(real) = crate::jobs::managed_env::base_python((3, 10), "Hugging Face Jobs").await else {
        crate::config::skip_without_tool("no Python 3.10+ on PATH");
        return;
    };
    let library = root.join("hf-lib/huggingface_hub");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(
        library.join("__init__.py"),
        r#"import os
class HTTPError(Exception):
    def __init__(self, status, text):
        super().__init__("%d %s" % (status, text))
        self.response = type("Response", (), {"status_code": status})()
def fail_at(step):
    failing, status = os.environ["ORX_TEST_HF"].split(":")
    with open(os.environ["ORX_API_ROOT"] + "/launches", "a") as log:
        log.write(step + "\n")
    if failing == step:
        raise HTTPError(int(status), "Payment Required" if status == "402" else "refused")
class HfApi:
    def __init__(self, **kwargs): pass
    def sync_job_volume(self, *args, **kwargs):
        fail_at("staging")
        return "volume"
    def run_job(self, **kwargs):
        fail_at("job")
"#,
    )
    .unwrap();
    script(
        &crate::config::config_dir().join("envs/huggingface/bin/python"),
        &format!(
            "#!/bin/sh\nPYTHONPATH='{}' exec '{real}' \"$@\"\n",
            library.parent().unwrap().display()
        ),
    );
    let archive = root.join("source.tar");
    std::fs::write(&archive, "snapshot").unwrap();
    let digest = format!("{:x}", Sha256::digest(b"snapshot"));
    let metadata = BackendDescriptor::parse(
        &json!({ "kind":"hf_job", "sourcePath":archive, "sourceDigest":digest, "sourceSize":8 })
            .to_string(),
    )
    .unwrap();
    let launch = LaunchSpec {
        kind: "hf_job".into(),
        target: Some("org".into()),
        options: json!({
            "command":["true"], "docker_image":"python", "flavor":"cpu", "environment":{}, "secrets":{}, "timeout_seconds":3600, "labels":{}
        }),
    };
    let adapter = super::api::Adapter::new(
        HfAccount {
            namespace: "org".into(),
        },
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    let engine = engine(&root);
    for (run, failure, status, message) in [
        ("billing", "job:402", "failed", "without billing"),
        ("bucket", "staging:403", "failed", "org/jobs-artifacts"),
        (
            "outage",
            "job:503",
            "submitting",
            "did not create the job in org",
        ),
    ] {
        std::env::set_var("ORX_TEST_HF", failure);
        engine
            .submit(&adapter, pending(run, &metadata, &launch))
            .await
            .unwrap();
        let (got, reason) = launch_outcome(&engine, run);
        assert_eq!(got, status, "{failure}");
        assert!(reason.contains(message), "{failure}: {reason}");
        assert!(!reason.contains("staging failed"), "{failure}: {reason}");
    }
    let launches = std::fs::read_to_string(root.join("launches")).unwrap();
    assert_eq!(
        launches.lines().collect::<Vec<_>>(),
        ["staging", "job", "staging", "staging", "job", "staging", "job"],
        "a refusal is never retried; an outage is"
    );
}

/// The run's reason: the failure, or what keeps it unreconciled.
fn launch_outcome(engine: &Engine, run: &str) -> (String, String) {
    let (row, state) = engine.load(run).unwrap();
    let reason = match state.contact {
        crate::store::Contact::Unreconciled { detail, .. } => detail,
        _ => row.result_markdown.unwrap_or_default(),
    };
    (row.status, reason)
}

#[tokio::test]
async fn vercel_refusals_fail_the_run_and_conflicts_stay_unknown() {
    let Some(root) = isolated().await else {
        return;
    };
    let Ok(real) = crate::jobs::managed_env::base_python((3, 10), "Vercel Sandbox").await else {
        crate::config::skip_without_tool("no Python 3.10+ on PATH");
        return;
    };
    let package = root.join("vercel-lib/vercel/sandbox");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(root.join("vercel-lib/vercel/__init__.py"), "").unwrap();
    std::fs::write(
        package.join("__init__.py"),
        r#"class SandboxResources:
    def __init__(self, vcpus=None): self.vcpus = vcpus
class SandboxApiError(Exception):
    def __init__(self, status_code):
        super().__init__("%d from Vercel" % status_code)
        self.status_code = status_code
"#,
    )
    .unwrap();
    std::fs::write(
        package.join("sync.py"),
        r#"import os
from vercel.sandbox import SandboxApiError
class FS:
    def write_text(self, *args, **kwargs): raise OSError("disk full")
class Box:
    name = "orx-run"
    fs = FS()
    def stop(self): pass
    def destroy(self, **kwargs):
        open(os.environ["ORX_API_ROOT"] + "/destroyed", "a").write("x")
def create_sandbox(**kwargs):
    mode = os.environ["ORX_TEST_VERCEL"]
    if mode == "setup": return Box()
    if mode == "created-then-403":
        open(os.environ["ORX_API_ROOT"] + "/created", "w").close()
        raise SandboxApiError(403)
    raise SandboxApiError(int(mode))
def get_sandbox(**kwargs):
    if os.path.exists(os.environ["ORX_API_ROOT"] + "/created"): return Box()
    raise SandboxApiError(404)
"#,
    )
    .unwrap();
    script(
        &root.join("bin/vercel-python"),
        &format!(
            "#!/bin/sh\nPYTHONPATH='{}' exec '{real}' \"$@\"\n",
            root.join("vercel-lib").display()
        ),
    );
    let engine = engine(&root);
    let metadata = BackendDescriptor::parse(r#"{"kind":"vercel_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "vercel_job".into(),
        target: None,
        options: json!({
            "name":"ignored", "script":"true", "image":"python", "vcpus":2, "env":{},
            "timeout_seconds":60, "session_cap":2700, "region":null, "tags":{},
            "source_archive":null
        }),
    };
    let adapter = (super::api::provider("vercel_job").unwrap().build)(
        &crate::engine::handle::ApiAccount {
            target: None,
            context: None,
        },
        Arc::new(TestClock::new()),
        root.join("spools"),
    )
    .unwrap();
    for (run, mode, status, reason) in [
        ("quota", "402", "failed", "HTTP 402"),
        ("setup", "setup", "failed", "orx deleted it"),
        ("taken", "409", "submitting", "409 from Vercel"),
        (
            "polled",
            "created-then-403",
            "submitting",
            "403 from Vercel",
        ),
    ] {
        std::env::set_var("ORX_TEST_VERCEL", mode);
        engine
            .submit(adapter.as_ref(), pending(run, &metadata, &launch))
            .await
            .unwrap();
        let (got, why) = launch_outcome(&engine, run);
        assert_eq!(got, status, "{mode}: {why}");
        assert!(why.contains(reason), "{mode}: {why}");
    }
    assert_eq!(
        std::fs::read_to_string(root.join("destroyed")).unwrap(),
        "x"
    );
}

#[tokio::test]
async fn ray_refusals_fail_the_run_unless_the_job_exists() {
    use std::sync::Mutex;
    type Answers = Arc<Mutex<(u16, u16)>>;
    async fn api(
        State(answers): State<Answers>,
        method: axum::http::Method,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let (status, inspected) = *answers.lock().unwrap();
        if method == axum::http::Method::POST {
            let status = axum::http::StatusCode::from_u16(status).unwrap();
            return (status, "Job with submission_id already exists").into_response();
        }
        match inspected {
            200 => Json(json!({"status":"RUNNING"})).into_response(),
            other => axum::http::StatusCode::from_u16(other)
                .unwrap()
                .into_response(),
        }
    }
    let answers: Answers = Arc::new(Mutex::new((400, 404)));
    let app = Router::new().fallback(any(api)).with_state(answers.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let root = std::env::temp_dir().join(format!("orx-ray-refused-{}", uuid::Uuid::new_v4()));
    let engine = engine(&root);
    let metadata = BackendDescriptor::parse(r#"{"kind":"ray_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "ray_job".into(),
        target: Some(address.clone()),
        options: json!({
            "entrypoint":"true", "submission_id":"ignored", "resources":{"cpus":1.0,"gpus":0.0,"memory_bytes":null}, "env":{}, "metadata":{}, "working_dir":null
        }),
    };
    let adapter = super::api::Adapter::new(
        RayAccount { address },
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    for (run, status, inspected, expected, reason) in [
        ("refused", 400, 404, "failed", "HTTP 400"),
        ("duplicate", 400, 200, "starting", ""),
        ("uninspectable", 400, 500, "submitting", "inspect"),
        ("limited", 429, 404, "submitting", "HTTP 429"),
    ] {
        *answers.lock().unwrap() = (status, inspected);
        engine
            .submit(&adapter, pending(run, &metadata, &launch))
            .await
            .unwrap();
        let (got, why) = launch_outcome(&engine, run);
        assert_eq!(got, expected, "{run}: {why}");
        assert!(why.contains(reason), "{run}: {why}");
    }
    server.abort();
    drop(engine);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn k8s_refusals_fail_the_run_unless_something_may_remain() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/kubectl"),
        r#"#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
a = sys.argv[1:]
mode = os.environ['ORX_TEST_K8S']
with (Path(os.environ['ORX_API_ROOT'])/'kubectl.log').open('a') as f: f.write(mode+' '+' '.join(a)+'\n')
def fail(message):
    print(message, file=sys.stderr)
    sys.exit(1)
if '--dry-run=client' in a:
    if mode == 'invalid':
        fail('error: error validating "STDIN": error validating data: ValidationError(Job.spec): unknown field "templat" in io.k8s.api.batch.v1.JobSpec')
    if mode == 'unparsable':
        fail('error: error parsing STDIN: error converting YAML to JSON: yaml: line 3: mapping values are not allowed in this context')
    if mode == 'unrecognized':
        fail('error: Get "https://k8s:6443/api?timeout=32s": net/http: TLS handshake timeout')
    if mode == 'offline':
        fail('E1111 memcache.go:265] couldn\'t get current server API group list: Get "https://k8s:6443/api": dial tcp: i/o timeout\nUnable to connect to the server: dial tcp: i/o timeout')
    print(sys.stdin.read())
elif a[0] == 'apply':
    sys.stdin.read()
elif a[0] == 'create':
    kind = json.loads(sys.stdin.read())['kind']
    if kind == 'ConfigMap' or (mode == 'remnant' and kind == 'Job'):
        pass
    elif mode == 'limited':
        fail('Error from server (TooManyRequests): error when creating "STDIN": the server has received too many requests and has asked us to try again later')
    else:
        fail('Error from server (Forbidden): error when creating "STDIN": jobs.batch is forbidden: User "lab" cannot create resource "jobs" in API group "batch" in the namespace "default"')
elif a[0] == 'delete':
    if mode == 'stuck' or (mode == 'remnant' and a[1].startswith('configmap/')):
        fail('Unable to connect to the server: dial tcp: i/o timeout')
elif a[0] == 'get':
    print('{"items":[]}')
else:
    raise Exception(a)
"#,
    );
    let config =
        json!({"apiVersion":"v1","kind":"ConfigMap","metadata":{"name":"{{ORX_RUN}}-config"}});
    let job = json!({"apiVersion":"batch/v1","kind":"Job","metadata":{"name":"{{ORX_RUN}}"},"spec":{"template":{"spec":{"restartPolicy":"Never","containers":[{"name":"run","image":"python","command":["bash","-c","eval \"$ORX_SCRIPT\""]}]}}}});
    let metadata = BackendDescriptor::parse(r#"{"kind":"k8s_job"}"#).unwrap();
    let launch = LaunchSpec {
        kind: "k8s_job".into(),
        target: Some("default".into()),
        options: json!({
            "manifest":format!("{config}{job}"), "script":"true", "run_token":"run", "env":{}, "timeout_seconds":3600, "labels":{}, "pod_env":[], "source_archive":root.join("archive.tar")
        }),
    };
    let engine = engine(&root);
    let adapter = super::api::Adapter::new(
        K8sAccount::new(None, "default".into()),
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    for (run, status, reason) in [
        ("forbidden", "failed", "jobs.batch is forbidden"),
        ("invalid", "failed", "unknown field"),
        ("unparsable", "failed", "error parsing STDIN"),
        ("unrecognized", "submitting", "TLS handshake timeout"),
        ("limited", "submitting", "TooManyRequests"),
        ("stuck", "submitting", "rolling back"),
        ("offline", "submitting", "Unable to connect"),
    ] {
        std::env::set_var("ORX_TEST_K8S", run);
        engine
            .submit(&adapter, pending(run, &metadata, &launch))
            .await
            .unwrap();
        let (got, why) = launch_outcome(&engine, run);
        assert_eq!(got, status, "{run}: {why}");
        assert!(why.contains(reason), "{run}: {why}");
    }
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(
        calls
            .lines()
            .any(|call| call.starts_with("forbidden delete configmap/run-config")),
        "{calls}"
    );

    // A rollback that stops part way leaves the Job, which recovery finds by its label.
    std::env::set_var("ORX_TEST_K8S", "remnant");
    let service = json!({"apiVersion":"v1","kind":"Service","metadata":{"name":"{{ORX_RUN}}-svc"}});
    let mut launch = launch;
    launch.options["manifest"] = json!(format!("{config}{job}{service}"));
    engine
        .submit(&adapter, pending("remnant", &metadata, &launch))
        .await
        .unwrap();
    let (got, why) = launch_outcome(&engine, "remnant");
    assert_eq!(got, "submitting", "{why}");
    assert!(why.contains("rolling back"), "{why}");
    let calls = std::fs::read_to_string(root.join("kubectl.log")).unwrap();
    assert!(
        calls
            .lines()
            .any(|call| call.starts_with("remnant delete configmap/")),
        "{calls}"
    );
    assert!(
        !calls
            .lines()
            .any(|call| call.starts_with("remnant delete job/")),
        "{calls}"
    );
}

fn hf_backend(root: &Path, clock: Arc<TestClock>) -> super::api::Adapter<HfAccount> {
    std::fs::create_dir_all(root.join("sse")).unwrap();
    std::fs::write(
        root.join("inspect-response.json"),
        json!({"id":"job", "status":{"stage":"RUNNING"}}).to_string(),
    )
    .unwrap();
    super::api::Adapter::new(
        HfAccount {
            namespace: "org".into(),
        },
        clock,
        root.join("spools"),
    )
}

fn hf_run() -> WatchedRun {
    WatchedRun {
        id: "run".into(),
        handle: super::api::test_handle("hf_job", Some("org"), None, vec!["job".into()]),
    }
}

async fn spooled(backend: &dyn Backend, run: &WatchedRun) -> String {
    let whole = ByteRange {
        start: 0,
        end: orx_host_proto::MAX_PAGE_LEN,
    };
    let LogPage::Data { bytes, .. } = backend
        .read_log(run, LogFile::Log, whole, None)
        .await
        .unwrap()
    else {
        panic!("no log page")
    };
    String::from_utf8(bytes).unwrap()
}

fn sse_queries(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join("sse.log"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn hf_logs_keep_long_lines_whole_however_a_replay_splits_them() {
    let Some(root) = isolated().await else {
        return;
    };
    let record = format!("{{\"metadata\":\"{}\"}}", "x".repeat(59_000));
    let stamp = |ms: u32| format!("2026-11-10T12:00:00.{ms:03}Z");
    let mut replay = vec![(stamp(100), "A".to_owned())];
    for (i, piece) in record.as_bytes().chunks(16 * 1024).enumerate() {
        replay.push((
            stamp(200 + i as u32),
            String::from_utf8(piece.to_vec()).unwrap(),
        ));
    }
    replay.extend([(stamp(300), "B".to_owned()), (stamp(400), "C".to_owned())]);
    let replay: Vec<_> = replay
        .iter()
        .map(|(t, d)| (t.as_str(), d.as_str()))
        .collect();
    std::fs::create_dir_all(root.join("sse")).unwrap();
    std::fs::write(
        root.join("sse/0"),
        hf_events(&[
            (&stamp(0), "===== Job started at 2026-11-10 12:00:00 ====="),
            (&stamp(100), "A"),
            (&stamp(200), &record),
            (&stamp(300), "B"),
        ]),
    )
    .unwrap();
    std::fs::write(root.join("sse/1"), hf_events(&replay)).unwrap();
    std::fs::write(root.join("sse/2"), hf_events(&replay)).unwrap();
    let expected = format!("A\n{record}\nB\nC\n");

    let backend = hf_backend(&root, Arc::new(TestClock::new()));
    let run = hf_run();
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, format!("A\n{record}\nB\n"));
    backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, expected);

    // A restarted engine continues from where the last one left off.
    drop(backend);
    let backend = hf_backend(&root, Arc::new(TestClock::new()));
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, expected);
    assert_eq!(sse_queries(&root), ["", "tail=100", "tail=100"]);
}

#[tokio::test]
async fn hf_logs_resume_past_a_capped_replay_and_a_tail_that_skips_output() {
    let Some(root) = isolated().await else {
        return;
    };
    let lines = |range: std::ops::RangeInclusive<u32>| {
        let events: Vec<_> = range
            .map(|n| (format!("2026-11-10T12:00:{n:02}.000Z"), format!("line {n}")))
            .collect();
        let events: Vec<_> = events
            .iter()
            .map(|(t, d)| (t.as_str(), d.as_str()))
            .collect();
        hf_events(&events)
    };
    let written = |range: std::ops::RangeInclusive<u32>| {
        range.map(|n| format!("line {n}\n")).collect::<String>()
    };
    std::fs::create_dir_all(root.join("sse")).unwrap();
    std::fs::write(root.join("sse/0"), lines(1..=10)).unwrap();
    // The provider keeps only the newest part of a long history.
    std::fs::write(root.join("sse/1"), lines(6..=12)).unwrap();
    std::fs::write(root.join("sse/2"), lines(14..=14)).unwrap();
    std::fs::write(root.join("sse/3"), lines(5..=14)).unwrap();
    let clock = Arc::new(TestClock::new());
    let backend = hf_backend(&root, clock.clone());
    let run = hf_run();
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, written(1..=10));
    for expected in [written(1..=12), written(1..=14)] {
        backend
            .observe(std::slice::from_ref(&run), Duration::from_secs(5))
            .await;
        clock.advance(Duration::from_secs(10));
        backend.drain_log(&run).await.unwrap();
        assert_eq!(spooled(&backend, &run).await, expected);
    }
    assert_eq!(sse_queries(&root), ["", "tail=100", "tail=100", ""]);
}

#[tokio::test]
async fn hf_logs_reopen_a_stream_sending_only_keep_alives_and_survive_a_failed_reopen() {
    let Some(root) = isolated().await else {
        return;
    };
    std::env::set_var("ORX_TEST_SPEEDUP", "120");
    let stamp = |s: u32| format!("2026-11-10T12:00:{s:02}.000Z");
    std::fs::create_dir_all(root.join("sse")).unwrap();
    std::fs::write(
        root.join("sse/0"),
        hf_events(&[(&stamp(0), "zero"), (&stamp(1), "one")]) + "!keepalive\n",
    )
    .unwrap();
    // sse/1 is missing, so the first reopen answers 503.
    std::fs::write(
        root.join("sse/2"),
        hf_events(&[(&stamp(0), "zero"), (&stamp(1), "one"), (&stamp(2), "two")]),
    )
    .unwrap();
    let backend = hf_backend(&root, Arc::new(TestClock::new()));
    let run = hf_run();
    assert!(backend.drain_log(&run).await.is_err());
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, "zero\none\ntwo\n");
    assert_eq!(sse_queries(&root), ["", "tail=100", "tail=100"]);
}

#[tokio::test]
async fn hf_logs_stream_while_running_unread_so_a_capped_final_replay_loses_nothing() {
    let Some(root) = isolated().await else {
        return;
    };
    let lines = |range: std::ops::RangeInclusive<u32>| {
        let events: Vec<_> = range
            .map(|n| (format!("2026-11-10T12:00:{n:02}.000Z"), format!("line {n}")))
            .collect();
        let events: Vec<_> = events
            .iter()
            .map(|(t, d)| (t.as_str(), d.as_str()))
            .collect();
        hf_events(&events)
    };
    let written = |range: std::ops::RangeInclusive<u32>| {
        range.map(|n| format!("line {n}\n")).collect::<String>()
    };
    std::fs::create_dir_all(root.join("sse")).unwrap();
    std::fs::write(root.join("sse/0"), lines(1..=10)).unwrap();
    let clock = Arc::new(TestClock::new());
    let backend = hf_backend(&root, clock.clone());
    let run = hf_run();
    backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;

    let spool = super::api::spool_path(&root.join("spools"), &run.id).unwrap();
    let mut kept = String::new();
    for _ in 0..100 {
        kept = std::fs::read_to_string(&spool).unwrap_or_default();
        if kept == written(1..=10) {
            break;
        }
        tokio::time::sleep(crate::config::paced(Duration::from_millis(50))).await;
    }
    assert_eq!(kept, written(1..=10), "nothing streamed the unread log");

    // Once the job ends, the provider replays only the newest part of its history.
    std::fs::write(
        root.join("inspect-response.json"),
        json!({"id":"job", "status":{"stage":"COMPLETED"}}).to_string(),
    )
    .unwrap();
    std::fs::write(root.join("sse/1"), lines(6..=12)).unwrap();
    clock.advance(Duration::from_secs(10));
    backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, written(1..=12));
}

#[tokio::test]
async fn hf_logs_continue_across_an_engine_restart_past_a_capped_replay() {
    use std::io::Write;
    let Some(root) = isolated().await else {
        return;
    };
    let lines = |range: std::ops::RangeInclusive<u32>| {
        let events: Vec<_> = range
            .map(|n| (format!("2026-11-10T12:00:{n:02}.000Z"), format!("line {n}")))
            .collect();
        let events: Vec<_> = events
            .iter()
            .map(|(t, d)| (t.as_str(), d.as_str()))
            .collect();
        hf_events(&events)
    };
    let written = |range: std::ops::RangeInclusive<u32>| {
        range.map(|n| format!("line {n}\n")).collect::<String>()
    };
    std::fs::create_dir_all(root.join("sse")).unwrap();
    std::fs::write(root.join("sse/0"), lines(1..=10) + "!keepalive\n").unwrap();
    let backend = hf_backend(&root, Arc::new(TestClock::new()));
    let run = hf_run();
    let mut kept = String::new();
    for _ in 0..100 {
        kept = spooled(&backend, &run).await;
        if kept == written(1..=10) {
            break;
        }
        tokio::time::sleep(crate::config::paced(Duration::from_millis(50))).await;
    }
    assert_eq!(kept, written(1..=10));

    // orx up stops mid-run, cut off partway through an append, while the provider goes on to
    // keep only the newest part of the history.
    drop(backend);
    let spool = super::api::spool_path(&root.join("spools"), &run.id).unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&spool)
        .unwrap()
        .write_all(b"line 1")
        .unwrap();
    std::fs::write(root.join("sse/1"), lines(6..=12)).unwrap();
    let backend = hf_backend(&root, Arc::new(TestClock::new()));
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, written(1..=12));
    assert_eq!(sse_queries(&root), ["", "tail=100"]);
}

#[tokio::test]
async fn modal_logs_continue_across_a_regrouped_reconnect_and_an_engine_restart() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/modal-python"),
        &format!(
            r#"#!/bin/sh
cd '{}'
if [ "$3" = status ]; then echo '{{"stage":"RUNNING"}}'; exit; fi
n=$(cat from.log 2>/dev/null | wc -l | tr -d ' ')
printf '%s\n' "$5" >> from.log
cat "frames/$n"
"#,
            root.display()
        ),
    );
    let replay: [(&str, &[u8]); 4] = [
        ("0", b"first\n"),
        ("6", b"second\n"),
        ("13", b"last\n"),
        ("18", b"end\n"),
    ];
    std::fs::create_dir_all(root.join("frames")).unwrap();
    std::fs::write(
        root.join("frames/0"),
        modal_frames(&[("0", b"first\nsecond\n")]),
    )
    .unwrap();
    // The reconnect sends the chunk it resumes after in smaller pieces.
    std::fs::write(root.join("frames/1"), modal_frames(&replay[..3])).unwrap();
    std::fs::write(root.join("frames/2"), modal_frames(&replay)).unwrap();
    let clock = Arc::new(TestClock::new());
    let backend = super::api::Adapter::new(ModalAccount, clock.clone(), root.join("spools"));
    let run = WatchedRun {
        id: "run".into(),
        handle: super::api::test_handle("modal_job", None, None, vec!["sandbox".into()]),
    };
    backend.drain_log(&run).await.unwrap();
    backend
        .observe(std::slice::from_ref(&run), Duration::from_secs(5))
        .await;
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, "first\nsecond\nlast\n");

    drop(backend);
    let backend = super::api::Adapter::new(ModalAccount, clock, root.join("spools"));
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, "first\nsecond\nlast\nend\n");
    assert_eq!(
        std::fs::read_to_string(root.join("from.log")).unwrap(),
        "\n0\n\n"
    );
}

#[tokio::test]
async fn modal_logs_an_older_orx_spooled_in_lines_are_rebuilt_from_the_start() {
    let Some(root) = isolated().await else {
        return;
    };
    script(
        &root.join("bin/modal-python"),
        &format!(
            r#"#!/bin/sh
cd '{}'
if [ "$3" = status ]; then echo '{{"stage":"RUNNING"}}'; exit; fi
printf '%s\n' "$5" >> from.log
cat frames
"#,
            root.display()
        ),
    );
    std::fs::write(
        root.join("frames"),
        modal_frames(&[("0", b"a\r\nb\r\nc"), ("8", b"d\n")]),
    )
    .unwrap();
    // How an older orx spooled the first chunk: one line at a time, each ending in `\n`.
    std::fs::create_dir_all(root.join("spools")).unwrap();
    std::fs::write(root.join("spools/run.spool.log"), "a\nb\nc\n").unwrap();
    let backend = super::api::Adapter::new(
        ModalAccount,
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    let run = WatchedRun {
        id: "run".into(),
        handle: super::api::test_handle("modal_job", None, None, vec!["sandbox".into()]),
    };
    backend.drain_log(&run).await.unwrap();
    assert_eq!(spooled(&backend, &run).await, "a\r\nb\r\ncd\n");
    assert_eq!(
        std::fs::read_to_string(root.join("from.log")).unwrap(),
        "\n"
    );
}

/// OpenResearch answers 404 for a box outside the signed-in account's organizations: no proof
/// it was released, so a cancel stays undelivered and the run names the account to change.
#[tokio::test]
async fn openresearch_404_under_another_account_is_not_a_release() {
    use super::openresearch::OpenResearch;
    use crate::store::StoredRun;

    let Some(root) = isolated().await else {
        return;
    };
    crate::config::save_credentials(&crate::config::Credentials {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        token: "fake".into(),
    })
    .await
    .unwrap();
    let id = "hidden".to_string();
    let mut descriptor = BackendDescriptor::parse(
        &json!({"kind":"openresearch_job","namespace":"org","jobId":id}).to_string(),
    )
    .unwrap();
    descriptor.rental_lease = Some(crate::jobs::rental::Rental {
        api_url: std::env::var("HF_ENDPOINT").unwrap(),
        sandbox_id: id.clone(),
        token: "fake".into(),
        unknown: Default::default(),
    });
    let handle = Handle::Legacy {
        descriptor: Box::new(descriptor.clone()),
    };
    let engine = Arc::new(engine(&root.join(&id)));
    engine
        .store()
        .upsert_run(&StoredRun {
            id: id.clone(),
            experiment_id: "exp".into(),
            project_id: "proj".into(),
            status: "running".into(),
            backend_json: handle::backend_json(&id, Some(&handle), &descriptor).unwrap(),
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
        })
        .unwrap();
    let adapter = OpenResearch::new(engine.clone(), Arc::new(|_| panic!("used SSH")));
    let watched = WatchedRun { id, handle };
    for flag in ["box-released", "delete-fails"] {
        std::fs::write(root.join(flag), "").unwrap();
    }
    std::fs::write(root.join("orgs.json"), r#"{"orgs":[]}"#).unwrap();
    assert!(matches!(
        adapter.cancel(&watched).await,
        CancelOutcome::Undelivered { .. }
    ));
    let observed = adapter
        .observe(std::slice::from_ref(&watched), Duration::ZERO)
        .await;
    assert!(
        observed.iter().any(|(_, observation)| matches!(
            observation,
            Observation::NeedsAuth { cause }
                if cause.contains("other@example.com") && cause.contains("org")
        )),
        "{observed:?}"
    );
    std::fs::remove_file(root.join("orgs.json")).unwrap();
    assert!(matches!(
        adapter.cancel(&watched).await,
        CancelOutcome::Verified
    ));
    for flag in ["box-released", "delete-fails"] {
        std::fs::remove_file(root.join(flag)).unwrap();
    }
}
