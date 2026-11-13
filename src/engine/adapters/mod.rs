pub(crate) mod api;
pub mod local;
pub mod openresearch;
pub mod slurm;
pub mod ssh;

macro_rules! providers {
    ($($module:ident => ($kind:literal, |$account:ident| $build:expr)),* $(,)?) => {
        $(pub mod $module;)*
        pub(crate) const PROVIDERS: &[api::Provider] = &[
            $(api::Provider {
                kind: $kind,
                build: |$account, clock, spools| {
                    Ok(std::sync::Arc::new(api::Adapter::new($build, clock, spools)))
                },
            }),*
        ];
    };
}

providers! {
    hf => ("hf_job", |account| hf::HfAccount { namespace: account.target()?.into() }),
    modal => ("modal_job", |_account| modal::ModalAccount),
    k8s => ("k8s_job", |account| k8s::K8sAccount::new(
        account.context.clone(), account.target()?.into()
    )),
    ray => ("ray_job", |account| ray::RayAccount { address: account.target()?.into() }),
    vercel => ("vercel_job", |_account| vercel::VercelAccount),
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, windows))]
mod windows_tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{local::Local, slurm::Slurm, ssh::Ssh};
    use crate::engine::clock::TestClock;
    use crate::engine::transport::{Config, HostConn, HostState, Status};
    use crate::engine::*;
    use crate::store::{LaunchSpec, Store};

    #[tokio::test]
    async fn adapters_respect_the_auth_breaker_without_starting_an_ssh_process() {
        let dir = std::env::temp_dir().join(format!("orx-adapters-{}", uuid::Uuid::new_v4()));
        let store = Store::open_at(dir.clone()).unwrap();
        store
            .seed_engine_host(
                "host",
                "/root",
                None,
                &HostState {
                    status: Status::NeedsAuth,
                    ..HostState::default()
                },
            )
            .unwrap();
        let clock = Arc::new(TestClock::new());
        let conn = {
            let config = Config {
                alias: "host".into(),
                root: "/root".into(),
                ssh_config: None,
                binary_dir: None,
                askpass_executable: std::env::current_exe().unwrap(),
            };
            let auth = crate::engine::transport::Auth::new(&store, &config.alias).unwrap();
            HostConn::start_shared(config, store, clock.clone(), auth)
        }
        .unwrap();
        let ssh = Arc::new(Ssh::new("host".into(), "/root".into(), conn));
        let slurm = Slurm::new(ssh.clone(), clock);
        let store = std::sync::Mutex::new(Store::open_at(dir.clone()).unwrap());
        let fence = Fence {
            store: &store,
            run_id: "run",
            attempt: 1,
            taken_at: 1,
        };
        let run = crate::store::StoredRun {
            id: "run".into(),
            experiment_id: "exp".into(),
            project_id: "proj".into(),
            status: "submitting".into(),
            backend_json: "{\"kind\":\"engine\"}".into(),
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
        let intent = RunIntent {
            run,
            launch: LaunchSpec {
                kind: "ssh_job".into(),
                target: Some("host".into()),
                options: serde_json::json!({}),
            },
            spec_digest: "0".repeat(64),
            attempt: 1,
        };
        let watched = WatchedRun {
            id: "run".into(),
            handle: ssh.handle(),
        };
        let subscription = ssh.follow_logs("run");
        assert!(ssh.summary("run").await.is_none());
        assert!(ssh
            .stage_blob(&"0".repeat(64), &dir.join("absent"), 1)
            .await
            .is_err());
        assert!(ssh.cleanup(&watched).await.is_err());
        for backend in [ssh.as_ref() as &dyn Backend, &slurm] {
            assert!(matches!(
                backend.launch(&intent, &fence).await,
                Err(LaunchError::Blocked { .. })
            ));
            assert!(matches!(
                backend.recover(&intent).await,
                Recovery::Unknown { .. }
            ));
            assert!(matches!(
                backend
                    .observe(std::slice::from_ref(&watched), Duration::ZERO)
                    .await[0]
                    .1,
                Observation::NeedsAuth { .. }
            ));
            assert!(matches!(
                backend.cancel(&watched).await,
                CancelOutcome::Undelivered { .. }
            ));
            assert!(backend
                .read_log(
                    &watched,
                    LogFile::Notify,
                    ByteRange { start: 0, end: 1 },
                    None
                )
                .await
                .is_err());
        }
        let clock = Arc::new(crate::engine::clock::SystemClock::new());
        let engine = Arc::new(Engine::new(
            Store::open_at(dir.clone()).unwrap(),
            clock.clone(),
        ));
        let transport = Arc::new(super::openresearch::LegacyTransport::new(
            ssh.clone(),
            dir.join("logs"),
        ));
        let adapters: Vec<Box<dyn Backend>> = vec![
            Box::new(super::api::Adapter::new(
                super::hf::HfAccount {
                    namespace: "org".into(),
                },
                clock.clone(),
                dir.join("spools"),
            )),
            Box::new(super::api::Adapter::new(
                super::modal::ModalAccount,
                clock.clone(),
                dir.join("spools"),
            )),
            Box::new(super::api::Adapter::new(
                super::k8s::K8sAccount::new(None, "default".into()),
                clock.clone(),
                dir.join("spools"),
            )),
            Box::new(super::api::Adapter::new(
                super::ray::RayAccount {
                    address: "unused".into(),
                },
                clock,
                dir.join("spools"),
            )),
            Box::new(super::openresearch::OpenResearch::new(
                engine,
                Arc::new(move |_| Ok(transport.clone())),
            )),
        ];
        for adapter in adapters {
            assert!(matches!(
                adapter.launch(&intent, &fence).await,
                Err(LaunchError::Rejected { .. })
            ));
            assert!(adapter
                .read_log(
                    &watched,
                    LogFile::Notify,
                    ByteRange { start: 0, end: 1 },
                    None
                )
                .await
                .is_err());
        }
        drop(subscription);
        drop(slurm);
        drop(ssh);
        drop(store);
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn local_recovery_preserves_the_tinker_handle() {
        let dir = std::env::temp_dir().join(format!("orx-local-{}", uuid::Uuid::new_v4()));
        let store = std::sync::Mutex::new(Store::open_at(dir.clone()).unwrap());
        let local = Local::new(dir.join("runs"));
        let run = crate::store::StoredRun {
            id: "tinker".into(),
            experiment_id: "exp".into(),
            project_id: "proj".into(),
            status: "submitting".into(),
            backend_json: "{\"kind\":\"engine\"}".into(),
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
        let mut intent = RunIntent {
            run,
            launch: LaunchSpec {
                kind: "tinker_job".into(),
                target: None,
                options: serde_json::json!({}),
            },
            spec_digest: "digest".into(),
            attempt: 1,
        };
        let run_dir = dir.join("runs/tinker");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("launch.json"), "digest").unwrap();
        std::fs::write(run_dir.join("started"), "").unwrap();
        assert!(matches!(
            local.recover(&intent).await,
            Recovery::Found(Handle::Tinker { .. })
        ));
        let watched = WatchedRun {
            id: "tinker".into(),
            handle: Handle::Tinker {
                dir: run_dir.to_string_lossy().into_owned(),
            },
        };
        assert!(matches!(
            local
                .read_log(&watched, LogFile::Log, ByteRange { start: 0, end: 1 }, None)
                .await
                .unwrap(),
            LogPage::Reset { .. }
        ));
        assert!(matches!(
            local.cancel(&watched).await,
            CancelOutcome::Undelivered { .. }
        ));
        assert!(!local.observe(&[watched], Duration::ZERO).await.is_empty());
        intent.run.id = "invalid/../id".into();
        let fence = Fence {
            store: &store,
            run_id: "tinker",
            attempt: 1,
            taken_at: 1,
        };
        assert!(matches!(
            local.launch(&intent, &fence).await,
            Err(LaunchError::Rejected { .. })
        ));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(all(test, unix))]
mod api_tests;
