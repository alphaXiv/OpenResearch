// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

#[path = "common/home.rs"]
mod home;

use orx_test_support::{Process, Sshd};
use sha2::{Digest, Sha256};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires an orx built with embedded hosts; release CI supplies ORX_PACKAGED_BINARY"]
fn packaged_orx_bootstraps_host_without_binary_directory() {
    let binary = std::env::var_os("ORX_PACKAGED_BINARY")
        .expect("set ORX_PACKAGED_BINARY to the orx built with embedded hosts");
    let server = Sshd::start().unwrap();
    let local = server.directory().join("laptop");
    fs::create_dir_all(&local).unwrap();
    let data = local.join("data");
    let command = || {
        let mut command = Command::new(&binary);
        home::isolate_home(&mut command, &local)
            .env("XDG_CONFIG_HOME", local.join("config"))
            .env("ORX_DATA_DIR", &data)
            .env("ORX_SSH_CONFIG", server.config_path())
            .env("ORX_NO_UPDATE_CHECK", "1")
            .env_remove("ORX_HOST_BIN_DIR")
            .env_remove("ORX_LOCAL_SESSION")
            .env_remove("ORX_UP_PORT")
            .env_remove("ORX_UP_AUTH_TOKEN")
            .arg("--no-telemetry");
        command
    };
    let initialized = command().args(["projects", "--json"]).output().unwrap();
    assert!(initialized.status.success(), "{initialized:?}");
    let db = rusqlite::Connection::open(data.join("orx.db")).unwrap();
    db.execute(
        "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, github_sync_enabled,
                                     baseline_branch, repo_path, created_at, updated_at)
         VALUES ('project', 'P', 'p', 'o', 'r', 0, 'main', ?1, 1, 1)",
        [local.to_str().unwrap()],
    )
    .unwrap();
    db.execute_batch(
        "INSERT INTO local_experiments (id, project_id, slug, branch_name, run_command, agent_status,
                                        created_at, updated_at)
         VALUES ('exp', 'project', 'exp', 'main', 'true', 'idle', 1, 1);",
    )
    .unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    let remote = server.home().join(".orx/runs").join(&run);
    fs::create_dir_all(&remote).unwrap();
    fs::write(remote.join("log"), "packaged host smoke\n").unwrap();
    fs::write(remote.join("exit_code"), "0\n").unwrap();
    let descriptor = serde_json::json!({
        "kind": "ssh_job", "namespace": server.alias(), "jobId": format!(".orx/runs/{run}")
    });
    db.execute(
        "INSERT INTO runs (id, experiment_id, project_id, status, backend_json, command, created_at, updated_at)
         VALUES (?1, 'exp', 'project', 'running', ?2, 'true', 1, 1)",
        rusqlite::params![run, descriptor.to_string()],
    )
    .unwrap();
    let bin = server.home().join(".orx/engine/bin");
    assert!(!bin.exists());
    let _app = Process::spawn(command().args(["up", "--no-browser", "--port", "0"])).unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let status: String = db
            .query_row("SELECT status FROM runs WHERE id = ?1", [&run], |row| {
                row.get(0)
            })
            .unwrap();
        if status == "done" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "packaged host did not observe the remote receipt: {status}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let uploaded: Vec<_> = fs::read_dir(bin)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(uploaded.len(), 1, "{uploaded:?}");
    let bytes = fs::read(&uploaded[0]).unwrap();
    let sha = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(
        uploaded[0].file_name().unwrap().to_str().unwrap(),
        format!("orx-host-{sha}")
    );
    println!("packaged orx bootstrapped orx-host-{sha} over SSH with ORX_HOST_BIN_DIR unset");
}
