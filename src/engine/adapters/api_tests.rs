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

async fn isolated(test: &str) -> Option<PathBuf> {
    if std::env::var("ORX_API_TEST").as_deref() == Ok(test) {
        return Some(PathBuf::from(std::env::var_os("ORX_API_ROOT").unwrap()));
    }
    let root = std::env::temp_dir().join(format!("orx-api-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    async fn lookup(
        State(root): State<PathBuf>,
        method: axum::http::Method,
        uri: axum::http::Uri,
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let output = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("engine::adapters::api_tests::{test}"),
            "--nocapture",
        ])
        .env("ORX_API_TEST", test)
        .env("ORX_API_ROOT", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("ORX_DATA_DIR", root.join("store"))
        .env("HF_ENDPOINT", endpoint)
        .env("HF_TOKEN", "fake")
        .env("ORX_MODAL_PYTHON", root.join("bin/modal-python"))
        .env("ORX_VERCEL_PYTHON", root.join("bin/vercel-python"))
        .env(
            "PATH",
            format!(
                "{}:{}",
                root.join("bin").display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .await
        .unwrap();
    server.abort();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::remove_dir_all(root).unwrap();
    None
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
    let Some(root) =
        isolated("hf_live_launcher_is_fenced_before_recovery_and_never_created_twice").await
    else {
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

#[tokio::test]
async fn k8s_recovery_resumes_staging_and_ready_is_immutable() {
    let Some(root) = isolated("k8s_recovery_resumes_staging_and_ready_is_immutable").await else {
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
    print('pod-1',end='')
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
    assert!(calls.contains("delete job/primary -n default --wait=false"));
}

#[tokio::test]
async fn openresearch_cancel_and_abandon_release_without_ssh_and_retry_api_failures() {
    use super::openresearch::OpenResearch;
    use crate::store::{Contact, StoredRun};

    let Some(root) =
        isolated("openresearch_cancel_and_abandon_release_without_ssh_and_retry_api_failures")
            .await
    else {
        return;
    };
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

    let Some(root) =
        isolated("openresearch_rejected_key_auto_cancel_checks_rental_and_survives_restart").await
    else {
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

async fn legacy_rejected_key(root: PathBuf, watching: bool) {
    use super::openresearch::{LegacyTransport, OpenResearch};
    use super::ssh::Ssh;
    use crate::engine::transport::{Config, HostConn, Status};
    use crate::store::StoredRun;
    use orx_test_support::host_binaries as binaries;
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
    adapter.observe(&watched, Duration::from_secs(1)).await;
    if watching {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if engine.load(&id).unwrap().0.status == "running"
                    && root.join(format!("store/run-logs/{id}.log")).exists()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let dir = format!(".orx/runs/{id}");
        assert_eq!(transport.collect(&id, &dir).await.unwrap()["eof"], true);
        let remote = server.home().join(&dir).join("log");
        let mut changed = vec![b'x'; 5000];
        changed[4999] = b'y';
        std::fs::write(&remote, &changed).unwrap();
        assert_eq!(transport.collect(&id, &dir).await.unwrap()["eof"], false);
        let local = root.join(format!("store/run-logs/{id}.log"));
        assert_eq!(std::fs::read(&local).unwrap(), vec![b'x'; 5000]);
        let resumed = LegacyTransport::new(transport.ssh.clone(), transport.log_dir.clone());
        assert_eq!(resumed.collect(&id, &dir).await.unwrap()["eof"], true);
        assert_eq!(std::fs::read(local).unwrap(), changed);
    } else {
        let partial = server
            .home()
            .join(format!("engine-root/sources/{digest}.part"));
        tokio::time::timeout(Duration::from_secs(20), async {
            while !partial.exists() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
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
    let checks = if watching { 6 } else { 32 };
    for _ in 0..checks {
        adapter.observe(&watched, Duration::from_secs(1)).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
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
    let processes = std::process::Command::new("ps")
        .args(["-axo", "command"])
        .output()
        .unwrap();
    let needle = format!("{} ssh-service", std::env::current_exe().unwrap().display());
    assert!(!String::from_utf8_lossy(&processes.stdout).contains(&needle));
    if watching {
        assert!(engine.cancel(&adapter, &id, "user").await.unwrap());
        assert_eq!(engine.load(&id).unwrap().0.status, "cancelled");
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
    let Some(root) =
        isolated("openresearch_staging_rejected_key_needs_auth_after_one_attempt").await
    else {
        return;
    };
    legacy_rejected_key(root, false).await;
}

#[tokio::test]
async fn openresearch_watching_rejected_key_needs_auth_after_one_attempt() {
    let Some(root) =
        isolated("openresearch_watching_rejected_key_needs_auth_after_one_attempt").await
    else {
        return;
    };
    legacy_rejected_key(root, true).await;
}

#[tokio::test]
async fn unsupported_leases_are_deleted_or_keep_the_known_box_handle() {
    let Some(root) = isolated("unsupported_leases_are_deleted_or_keep_the_known_box_handle").await
    else {
        return;
    };
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
    let launch = LaunchSpec {
        kind: "openresearch_job".into(),
        target: Some("org".into()),
        options: json!({"flavor":"cpu5c"}),
    };
    for (id, lease, deletion_fails) in [
        ("missing", Value::Null, false),
        ("unsupported", json!({"version":2,"token":"fake"}), false),
        ("retained", Value::Null, true),
    ] {
        let response = json!({"sandbox":{"id":id,"organizationId":"org","status":"online","machineType":"cpu","updatedAt":"now"},"rentalLease":lease});
        std::fs::write(root.join("sandbox-response.json"), response.to_string()).unwrap();
        if deletion_fails {
            std::fs::write(root.join("delete-fails"), "").unwrap();
        }
        engine
            .submit(&backend, pending(id, &metadata, &launch))
            .await
            .unwrap();
        let (run, _) = engine.load(id).unwrap();
        if deletion_fails {
            assert_eq!(run.status, "starting");
            let Some(Handle::Legacy { descriptor }) = handle_of(&run) else {
                panic!("lost paid box id");
            };
            assert_eq!(descriptor.job_id.as_deref(), Some(id));
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
    let Some(root) =
        isolated("failed_submission_fences_kill_launchers_before_stdin_is_released").await
    else {
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
    let Some(root) = isolated("chatty_modal_logs_publish_a_partial_page_and_reuse_it").await else {
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

    let Some(root) =
        isolated("live_modal_eof_reconnects_and_final_collection_keeps_later_output").await
    else {
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
    let Some(root) =
        isolated("finished_modal_logs_drain_through_pauses_before_they_are_kept").await
    else {
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

/// A pod lookup that fails, or finds no pod yet, is never a closed stream: draining errs, and
/// the log stays owed.
#[tokio::test]
async fn kubernetes_lookup_failures_never_complete_a_drain() {
    use super::api::Api;
    let Some(root) = isolated("kubernetes_lookup_failures_never_complete_a_drain").await else {
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
    for get in [
        "echo 'Unable to connect to the server' >&2; exit 1",
        "printf '{\"items\":[]}'",
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
        assert!(!matches!(closed, Ok(true)), "{get}: {closed:?}");
        let backend =
            super::api::Adapter::new(account, Arc::new(TestClock::new()), root.join("spools"));
        assert!(backend.drain_log(&run).await.is_err(), "{get}");
    }
}

#[tokio::test]
async fn empty_stream_timeouts_surface_for_hf_modal_and_kubernetes() {
    use super::api::Api;
    let Some(root) = isolated("empty_stream_timeouts_surface_for_hf_modal_and_kubernetes").await
    else {
        return;
    };
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
        .logs(&hf_run, &mut hf_cursor, Duration::from_secs(1), &mut |_| {})
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
        .logs(
            &modal_run,
            &mut modal_cursor,
            Duration::from_secs(1),
            &mut |_| {}
        )
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
            Duration::from_secs(1),
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
    let Some(root) = isolated("vercel_uses_api_handles_and_adopts_legacy_sandboxes").await else {
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
