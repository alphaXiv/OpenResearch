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
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
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

/// Runs the calling test alone in a child process, against a fake provider API
/// and with fake provider CLIs first on `PATH`. `None` in the parent.
async fn isolated() -> Option<PathBuf> {
    if !crate::config::in_test_child(&[]) {
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
        if uri.path().starts_with("/sandboxes/") {
            if method == axum::http::Method::GET {
                if root.join("box-released").exists() {
                    return (axum::http::StatusCode::NOT_FOUND, Json(json!({}))).into_response();
                }
                if root.join("lookup-fails").exists() {
                    return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
                        .into_response();
                }
                return (
                    headers,
                    Json(json!({"sandbox": {
                        "id":"box", "organizationId":"org", "status":"online",
                        "machineType":"cpu", "updatedAt":"now"
                    }})),
                )
                    .into_response();
            }
            if (method == axum::http::Method::DELETE || uri.path().ends_with("/lease/release"))
                && root.join("delete-fails").exists()
            {
                return (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({})))
                    .into_response();
            }
            return (headers, Json(json!({}))).into_response();
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
        vec![(
            "run".into(),
            Observation::Running {
                started_at: Some(1234)
            }
        )]
    );
    assert!(adapter
        .observe(&runs, Duration::from_secs(2))
        .await
        .is_empty());
    clock.advance(Duration::from_secs(10));
    assert_eq!(
        adapter.observe(&runs, Duration::from_secs(2)).await.len(),
        1
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
    print('{"items":[]}' if a[-1]=='json' else 'pod-1',end='')
elif a[0]=='get' and a[1]=='job' and a[2]!='-n' and (root/'finished').exists():
    print('{"status":{"succeeded":1}}')
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
        target: None,
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
    let adapter = super::api::Adapter::new(
        K8sAccount {
            context: None,
            namespace: "default".into(),
        },
        Arc::new(TestClock::new()),
        root.join("spools"),
    );
    engine.recover(&adapter, "run").await.unwrap();
    assert_eq!(engine.load("run").unwrap().0.status, "starting");
    assert_eq!(std::fs::read(root.join("source.tar")).unwrap(), b"snapshot");
    let state = engine.load("run").unwrap().1;
    let mut intent = RunIntent::new(engine.load("run").unwrap().0, &state).unwrap();
    assert!(matches!(
        adapter.recover(&intent).await,
        Recovery::Found(Handle::Api { .. })
    ));
    assert_eq!(
        std::fs::read_to_string(root.join("copies")).unwrap(),
        "copy\n"
    );
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
                        .await
                        .unwrap();
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
                    .await
                    .unwrap();
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
                .await
                .unwrap();
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
                .await
                .unwrap();
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
                .await
                .unwrap();
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
                .await
                .unwrap();
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
    if watching {
        transport
            .upload("cat > \"$HOME/uploaded\"", &archive)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(server.home().join("uploaded")).unwrap(),
            source
        );
    }
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
        .await
        .unwrap();
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
                .await
                .unwrap();
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
            .await
            .unwrap();
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
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!root.join("leaked-hf").exists());
    assert!(!root.join("leaked-k8s").exists());
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
while :; do printf 'line\n'; sleep 0.01; done
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
  cat history
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
        .await
        .unwrap();
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
        "#!/bin/sh\nprintf 'first\\n'\nsleep 11\nprintf 'last\\n'\n",
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
    let account = K8sAccount {
        context: None,
        namespace: "default".into(),
    };
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
        .await
        .unwrap();
    assert_eq!(engine.load("run").unwrap().0.status, "running");
    assert!(engine.store().run_notice_batch("chat").unwrap().is_empty());

    backend.drain_log(&run).await.unwrap();
    std::fs::write(root.join("gone"), "").unwrap();
    clock.advance(Duration::from_secs(10));
    engine
        .observe_runs(backend.as_ref(), &[run], Duration::from_secs(5))
        .await
        .unwrap();
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
        let account = K8sAccount {
            context: None,
            namespace: "default".into(),
        };
        let mut cursor = crate::jobs::kubernetes::LogResume::default();
        let closed = account
            .logs(&run, &mut cursor, Duration::from_secs(5), &mut |_| {})
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
    let mut hf_cursor = 0;
    assert!(hf
        .logs(&hf_run, &mut hf_cursor, budget, &mut |_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert_eq!(hf_cursor, 0);
    let modal_run = WatchedRun {
        id: "modal".into(),
        handle: crate::engine::adapters::api::test_handle(
            "modal_job",
            None,
            None,
            vec!["sandbox".into()],
        ),
    };
    let mut modal_cursor = 0;
    assert!(ModalAccount
        .logs(&modal_run, &mut modal_cursor, budget, &mut |_| {})
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert_eq!(modal_cursor, 0);
    let k8s = K8sAccount {
        context: None,
        namespace: "default".into(),
    };
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
            &mut |_| {}
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
  cancel) touch '{root}/cancelled' ;;
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
        .await
        .unwrap();
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
    let adopted = engine.scan_legacy(true).unwrap();
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
