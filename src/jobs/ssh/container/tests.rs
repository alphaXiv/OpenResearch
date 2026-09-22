use super::*;
use crate::jobs::ssh::{self, HostKeyPolicy, SshJobSpec};
use std::{collections::HashMap, time::Duration};

async fn prepare(
    target: &SshTarget,
    reference: Option<&str>,
    command: &str,
) -> (String, SshJobSpec) {
    let id = uuid::Uuid::new_v4().to_string();
    let container = match reference {
        Some(reference) => Some(resolve(target, reference).await.unwrap().for_run(&id)),
        None => None,
    };
    let temp = crate::local::git::TemporaryDirectory::new("orx-container-test").unwrap();
    std::fs::write(temp.path().join("source.txt"), "snapshot").unwrap();
    std::fs::write(
        temp.path().join("run.sh"),
        format!("set -eo pipefail\n{command}\n"),
    )
    .unwrap();
    let archive = temp.path().join("source.tar");
    assert!(std::process::Command::new("tar")
        .arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(temp.path())
        .arg("source.txt")
        .arg("run.sh")
        .status()
        .unwrap()
        .success());
    let dir = ssh::stage_source(target, &id, &archive, &id, container.as_ref())
        .await
        .unwrap();
    let spec = SshJobSpec {
        target: target.clone(),
        run_id: id,
        script: crate::compute::staged_script("bash run.sh"),
        env: HashMap::from([(
            "ORX_TEST_SECRET".into(),
            "quote' and $literal\nsecond line".into(),
        )]),
        container,
    };
    (dir, spec)
}

async fn launch(
    target: &SshTarget,
    reference: Option<&str>,
    command: &str,
) -> (String, Option<ContainerRun>) {
    let (dir, spec) = prepare(target, reference, command).await;
    ssh::run_job(&spec).await.unwrap();
    (dir, spec.container)
}

async fn terminal(target: &SshTarget, dir: &str, container: Option<&ContainerRun>) -> JobState {
    tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let state = ssh::inspect_job(target, dir, container).await.unwrap();
            if crate::jobs::is_terminal_stage(&state.stage) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("run never became terminal")
}

async fn logs(target: &SshTarget, dir: &str) -> String {
    let mut output = String::new();
    ssh::stream_logs(target, dir, 0, Duration::ZERO, &mut |line| {
        output.push_str(line);
        output.push('\n');
    })
    .await
    .unwrap();
    output
}

#[test]
fn references_are_names_not_shell_or_docker_options() {
    for value in ["", " ", "--help", "a/b", "a;id", "a\nb"] {
        assert!(validate_reference(value).is_err());
    }
    for value in ["none", "research-v1.2", "a_b", "012345abcdef"] {
        validate_reference(value).unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable fixtures; run scripts/test-ssh-container.sh"]
async fn ssh_container_lifecycle() {
    let port = std::env::var("ORX_SSH_TEST_PORT")
        .expect("fixture port")
        .parse()
        .unwrap();
    let reference = std::env::var("ORX_SSH_TEST_CONTAINER").expect("fixture container");
    let mut target = SshTarget::host_port("root@127.0.0.1".into(), port, HostKeyPolicy::Ephemeral);
    target.extra_opts.extend([
        "-i".into(),
        std::env::var("ORX_SSH_TEST_KEY").expect("fixture key"),
    ]);
    let base = resolve(&target, &reference).await.unwrap();
    ssh_run(
        &target,
        &base.exec("/opt/conda/bin/conda create -y -n research --offline"),
        None,
    )
    .await
    .unwrap();
    let setup = "source /opt/conda/etc/profile.d/conda.sh\nconda activate research\nprintf 'setup:%s\\n' \"$CONDA_DEFAULT_ENV\"";
    let (dir, container) = launch(&target, Some(&reference), &format!("{setup}\ncat source.txt; printf '%s\\n' \"$ORX_TEST_SECRET\" \"$PYTHONUNBUFFERED\" \"$CONDA_DEFAULT_ENV\"; echo early; sleep 2; echo late")).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let early = logs(&target, &dir).await;
    assert!(
        early.contains("early") && !early.contains("late"),
        "{early}"
    );
    assert_eq!(
        terminal(&target, &dir, container.as_ref()).await.stage,
        "COMPLETED"
    );
    let output = logs(&target, &dir).await;
    assert!(
        output.contains("snapshot")
            && output.contains("quote' and $literal\nsecond line")
            && output.contains("\n1\n/tmp/conda-envs/research\n"),
        "{output}"
    );
    assert_eq!(output.matches("setup:/tmp/conda-envs/research").count(), 1);
    ssh_run(&target, &format!("test ! -d \"$HOME/{dir}/repo\""), None)
        .await
        .unwrap();
    for code in [7, 125, 126, 127] {
        let (dir, container) = launch(&target, Some(&reference), &format!("exit {code}")).await;
        let state = terminal(&target, &dir, container.as_ref()).await;
        assert_eq!(
            state.message.as_deref(),
            Some(format!("exited with code {code}").as_str())
        );
    }
    let (dir, container) = launch(
        &target,
        Some(&reference),
        "echo SETUP_FAILED; false; echo SHOULD_NOT_RUN",
    )
    .await;
    assert_eq!(
        terminal(&target, &dir, container.as_ref()).await.stage,
        "ERROR"
    );
    assert_eq!(logs(&target, &dir).await, "SETUP_FAILED\n");
    let (dir, _) = launch(
        &target,
        None,
        "cat source.txt; export HOME=/tmp; cd /tmp; echo DIRECT",
    )
    .await;
    assert_eq!(terminal(&target, &dir, None).await.stage, "COMPLETED");
    assert!(logs(&target, &dir).await.contains("DIRECT"));

    let (dir, container) = launch(
        &target,
        Some(&reference),
        "trap '' TERM; (trap '' TERM; sleep 300) & wait",
    )
    .await;
    let container = container.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let restored: ContainerRun =
        serde_json::from_str(&serde_json::to_string(&container).unwrap()).unwrap();
    assert_eq!(
        ssh::inspect_job(&target, &dir, Some(&restored))
            .await
            .unwrap()
            .stage,
        "RUNNING"
    );
    let processes = ssh_run(&target, &container.exec("ps auxww"), None)
        .await
        .unwrap();
    assert!(
        !processes.contains("quote' and $literal"),
        "secret leaked to argv"
    );
    ssh_run(
        &target,
        &format!("docker exec -d {} sleep 301", sh_quote(&base.id)),
        None,
    )
    .await
    .unwrap();
    ssh_run(
        &target,
        &format!("docker pause {}", sh_quote(&base.id)),
        None,
    )
    .await
    .unwrap();
    let paused = ssh::inspect_job(&target, &dir, Some(&container))
        .await
        .unwrap();
    assert_eq!(paused.stage, "RUNNING");
    assert!(paused.message.unwrap().contains("paused"));
    assert!(ssh::cancel_job(&target, &dir, Some(&container))
        .await
        .unwrap_err()
        .to_string()
        .contains("Unpause"));
    ssh_run(
        &target,
        &format!("docker unpause {}", sh_quote(&base.id)),
        None,
    )
    .await
    .unwrap();
    ssh::cancel_job(&target, &dir, Some(&container))
        .await
        .unwrap();
    assert_eq!(
        terminal(&target, &dir, Some(&container)).await.stage,
        "ERROR"
    );
    let processes = ssh_run(&target, &base.exec("ps auxww"), None)
        .await
        .unwrap();
    assert!(
        !processes.contains("sleep 300"),
        "child escaped cancellation: {processes}"
    );
    assert!(
        processes.contains("sleep 301"),
        "unrelated process was killed"
    );
    base.require_running(&target).await.unwrap();

    let (dir, spec) = prepare(&target, Some(&reference), "echo ONCE; sleep 1").await;
    ssh_run(&target, "touch /tmp/drop-launch-ack", None)
        .await
        .unwrap();
    assert!(
        ssh::run_job(&spec)
            .await
            .unwrap_err()
            .is::<ssh::LaunchUncertain>(),
        "fixture must drop the launch acknowledgement"
    );
    let container = spec.container;
    let saved: ContainerRun =
        serde_json::from_str(&serde_json::to_string(&container.unwrap()).unwrap()).unwrap();
    assert_eq!(
        terminal(&target, &dir, Some(&saved)).await.stage,
        "COMPLETED"
    );
    assert_eq!(logs(&target, &dir).await.matches("ONCE").count(), 1);
    let (dir, spec) = prepare(&target, Some(&reference), "echo NEVER_LAUNCHED").await;
    ssh_run(
        &target,
        &spec.container.as_ref().unwrap().exec(&format!(
            "mkdir {}",
            sh_quote(&format!(
                "{}/run.sh",
                spec.container.as_ref().unwrap().run_dir
            ))
        )),
        None,
    )
    .await
    .unwrap();
    let error = ssh::run_job(&spec).await.unwrap_err();
    assert!(
        !error.is::<ssh::LaunchUncertain>(),
        "script upload failure is not a lost acknowledgement"
    );
    assert!(!logs(&target, &dir).await.contains("NEVER_LAUNCHED"));

    let (dir, container) = launch(&target, Some(&reference), "true").await;
    let container = container.unwrap();
    assert_eq!(
        terminal(&target, &dir, Some(&container)).await.stage,
        "COMPLETED"
    );
    ssh_run(&target, &format!("cd \"$HOME/{dir}\"; rm exit_code; setsid sleep 300 </dev/null >/dev/null 2>&1 & echo $! > pid"), None).await.unwrap();
    assert_eq!(
        inspect(&target, &dir, &container).await.unwrap().stage,
        "RUNNING"
    );
    ssh_run(
        &target,
        &format!("echo 0 > \"$HOME/{dir}/completion_wait\""),
        None,
    )
    .await
    .unwrap();
    let state = inspect(&target, &dir, &container).await.unwrap();
    assert_eq!(state.stage, "ERROR");
    assert!(state
        .message
        .unwrap()
        .contains("did not record its exit status"));

    let (dir, container) = launch(&target, Some(&reference), "sleep 300").await;
    let container = container.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    ssh_run(
        &target,
        &format!("kill -KILL $(cat \"$HOME/{dir}/pid\")"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        terminal(&target, &dir, Some(&container)).await.stage,
        "ERROR"
    );
    assert!(!ssh_run(&target, &base.exec("ps auxww"), None)
        .await
        .unwrap()
        .contains("sleep 300"));

    let id = uuid::Uuid::new_v4().to_string();
    let pending = base.for_run(&id);
    let pending_dir = format!(".orx/runs/{id}");
    ssh_run(
        &target,
        &pending.exec(&format!("mkdir -p {}", sh_quote(&pending.run_dir))),
        None,
    )
    .await
    .unwrap();
    ssh_run(
        &target,
        &format!(
            "mkdir -p \"$HOME/{pending_dir}\"; date +%s > \"$HOME/{pending_dir}/launch_time\""
        ),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        inspect(&target, &pending_dir, &pending)
            .await
            .unwrap()
            .stage,
        "RUNNING"
    );
    ssh_run(
        &target,
        &format!("echo 0 > \"$HOME/{pending_dir}/launch_time\""),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        inspect(&target, &pending_dir, &pending)
            .await
            .unwrap()
            .stage,
        "ERROR"
    );

    let (dir, container) = launch(&target, Some(&reference), "sleep 300").await;
    let container = container.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let identity = sh_quote(&format!("{}/identity", container.run_dir));
    ssh_run(&target, &container.exec(&format!("cp {identity} {identity}.saved; read -r p started < {identity}; echo \"$p 0\" > {identity}")), None).await.unwrap();
    assert!(cancel(&target, &container)
        .await
        .unwrap_err()
        .to_string()
        .contains("identity changed"));
    ssh_run(
        &target,
        &container.exec(&format!("mv {identity}.saved {identity}")),
        None,
    )
    .await
    .unwrap();
    cancel(&target, &container).await.unwrap();
    terminal(&target, &dir, Some(&container)).await;

    ssh_run(&target, "mv /usr/local/bin/docker /usr/local/bin/docker-real; printf '#!/bin/sh\\nDOCKER_HOST=unix:///missing-docker.sock exec /usr/local/bin/docker-real \"$@\"\\n' > /usr/local/bin/docker; chmod +x /usr/local/bin/docker", None).await.unwrap();
    assert!(inspection(&target, &base.id).await.is_err());
    ssh_run(
        &target,
        "mv /usr/local/bin/docker-real /usr/local/bin/docker",
        None,
    )
    .await
    .unwrap();
    supervisor_restart(&target, &reference, port).await;
    for action in ["stop", "restart", "rm -f"] {
        let (dir, container) = launch(&target, Some(&reference), "echo SURVIVES; sleep 300").await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        ssh_run(
            &target,
            &format!("docker {action} {}", sh_quote(&reference)),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            terminal(&target, &dir, container.as_ref()).await.stage,
            "ERROR"
        );
        assert!(logs(&target, &dir).await.contains("SURVIVES"));
        if action == "stop" {
            ssh_run(
                &target,
                &format!("docker start {}", sh_quote(&reference)),
                None,
            )
            .await
            .unwrap();
        }
        if action == "rm -f" {
            ssh_run(&target, &format!("docker run -d --name {} --entrypoint bash condaforge/miniforge3 -c 'exec sleep infinity'", sh_quote(&reference)), None).await.unwrap();
            assert!(unavailable(&target, container.as_ref().unwrap())
                .await
                .unwrap()
                .unwrap()
                .contains("removed"));
            cancel(&target, container.as_ref().unwrap()).await.unwrap();
            resolve(&target, &reference).await.unwrap();
        }
    }
    eprintln!("SSH container lifecycle: passed");
}

async fn supervisor_restart(target: &SshTarget, reference: &str, port: u16) {
    use crate::local::model::{LocalExperiment, LocalProject};
    use crate::store::{Store, StoredRun};
    let temp = crate::local::git::TemporaryDirectory::new("orx-supervisor-restart").unwrap();
    let root = temp.path();
    let ssh_dir = root.join(".ssh");
    std::fs::create_dir(&ssh_dir).unwrap();
    std::fs::write(ssh_dir.join("config"), format!("Host fixture\n HostName 127.0.0.1\n User root\n Port {port}\n IdentityFile {}\n StrictHostKeyChecking no\n UserKnownHostsFile /dev/null\n", std::env::var("ORX_SSH_TEST_KEY").unwrap())).unwrap();
    let store = Store::open_at(root.join("data")).unwrap();
    store
        .create_local_project(&LocalProject {
            id: "project".into(),
            name: "test".into(),
            slug: "test".into(),
            github_owner: String::new(),
            github_repo: String::new(),
            github_sync_enabled: false,
            baseline_branch: "main".into(),
            repo_path: root.to_string_lossy().into_owned(),
            run_command: None,
            paper_id: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    store
        .create_local_experiment(&LocalExperiment {
            id: "experiment".into(),
            project_id: "project".into(),
            parent_experiment_id: None,
            slug: "test".into(),
            branch_name: "main".into(),
            title: None,
            description: None,
            run_command: "echo ONCE; sleep 8; echo AFTER".into(),
            agent_status: "idle".into(),
            created_at: 1,
            updated_at: 1,
            chat_session_id: None,
        })
        .unwrap();
    let (dir, container) = launch(target, Some(reference), "echo ONCE; sleep 8; echo AFTER").await;
    let id = dir.rsplit('/').next().unwrap();
    let descriptor = serde_json::json!({"kind":"ssh_job","namespace":"fixture","jobId":dir,"sshContainer":container});
    store
        .upsert_run(&StoredRun {
            id: id.into(),
            experiment_id: "experiment".into(),
            project_id: "project".into(),
            status: "starting".into(),
            backend_json: descriptor.to_string(),
            command: "test".into(),
            created_at: 1,
            updated_at: 1,
            ended_at: None,
            exit_code: None,
            commit_sha: None,
            result_markdown: None,
            cancel_requested: false,
            chat_session_id: None,
        })
        .unwrap();
    // OpenSSH ignores HOME for its config lookup, so confine this override to the supervisor child.
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let ssh = bin.join("ssh");
    std::fs::write(
        &ssh,
        format!(
            "#!/bin/sh\nexec /usr/bin/ssh -F {} \"$@\"\n",
            sh_quote(&ssh_dir.join("config").to_string_lossy())
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let executable = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("orx{}", std::env::consts::EXE_SUFFIX));
    let spawn = || {
        let mut command = tokio::process::Command::new(&executable);
        command
            .args(["--no-telemetry", "supervise", id])
            .env("HOME", root)
            .env("ORX_DATA_DIR", root.join("data"))
            .env("ORX_CACHE_DIR", root.join("cache"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("ORX_NO_UPDATE_CHECK", "1")
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        command.spawn().unwrap()
    };
    let mut first = spawn();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        first.try_wait().unwrap().is_none(),
        "supervisor exited before restart"
    );
    first.kill().await.unwrap();
    first.wait().await.unwrap();
    let config = root.join("config/openresearch");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), r#"{"ssh":{"defaultHost":"wrong-host","hosts":{"fixture":{"container":"wrong-container"}}}}"#).unwrap();
    let second = spawn();
    let result = tokio::time::timeout(Duration::from_secs(25), second.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(store.get_run(id).unwrap().unwrap().status, "done");
    let output = logs(target, &dir).await;
    assert_eq!(output.matches("ONCE").count(), 1);
    assert!(output.contains("AFTER"));
}
