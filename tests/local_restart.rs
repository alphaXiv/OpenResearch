// Separate test crate; commands inherit the terminal environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

use orx_host::{platform, read_json, wrapper};
use orx_host_proto::ProcessIdentity;
use orx_test_support::Process;
use serde_json::{json, Value};
use std::fs;
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

#[tokio::test]
async fn local_cancel_and_observe_survive_an_orx_up_exec_restart() {
    let root = std::env::temp_dir().join(format!("orx-local-restart-{}", uuid::Uuid::new_v4()));
    let data = root.join("data");
    let config = root.join("config/openresearch");
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(root.join(".ssh")).unwrap();
    fs::write(root.join(".ssh/config"), "").unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_orx"));
        command
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME")
            .env_remove("ORX_LOCAL_SESSION")
            .env_remove("ORX_UP_PORT")
            .env_remove("ORX_UP_AUTH_TOKEN")
            .env_remove("SSH_AUTH_SOCK")
            .env("HOME", &root)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("XDG_DATA_HOME", root.join("xdg-data"))
            .env("ORX_DATA_DIR", &data)
            .env("ORX_CACHE_DIR", root.join("cache"))
            .env("ORX_SSH_CONFIG", root.join(".ssh/config"))
            .env("ORX_HOST_BIN_DIR", orx_test_support::host_binaries())
            .env("ORX_NO_UPDATE_CHECK", "1")
            .arg("--no-telemetry");
        command
    };
    let initialized = command().args(["projects", "--json"]).output().unwrap();
    assert!(initialized.status.success(), "{initialized:?}");
    let db = rusqlite::Connection::open(data.join("orx.db")).unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.execute("INSERT INTO local_projects (id,name,slug,github_owner,github_repo,repo_path,created_at,updated_at) VALUES ('project','P','p','','',?1,1,1)", [root.to_str().unwrap()]).unwrap();
    db.execute_batch("INSERT INTO local_experiments (id,project_id,slug,branch_name,run_command,agent_status,created_at,updated_at) VALUES ('exp','project','exp','main','sleep 60','idle',1,1)").unwrap();
    let run = uuid::Uuid::new_v4().to_string();
    let script = "echo $$ > payload-pid; touch ready; exec sleep 120";
    let launch =
        json!({"kind":"local_job","target":null,"options":{"env":{"ORX_TEST_FAST_STOPS":"1"}}});
    db.execute("INSERT INTO runs (id,experiment_id,project_id,status,backend_json,command,created_at,updated_at,spec_digest,submission_attempt,launch_spec) VALUES (?1,'exp','project','submitting','{\"kind\":\"engine\"}',?2,1,1,?3,1,?4)", rusqlite::params![run, script, orx_host::sha(script.as_bytes()), launch.to_string()]).unwrap();
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut up = command();
    up.args([
        "up",
        "--no-browser",
        "--no-agent",
        "--headless",
        "--port",
        &port.to_string(),
    ]);
    let app = Process::spawn(&mut up).unwrap();
    #[allow(clippy::disallowed_methods)]
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let url = format!("http://127.0.0.1:{port}");
    let dir = data.join("local-runs").join(&run);
    let deadline = Instant::now() + Duration::from_secs(45);
    while !dir.join("ready").exists() {
        assert!(Instant::now() < deadline, "{}", app.stderr().unwrap());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let identity: ProcessIdentity = read_json(&dir.join("wrapper.json")).unwrap();
    assert_ne!(platform::process(identity.pid).unwrap().ppid, app.pid());
    let payload: u32 = fs::read_to_string(dir.join("payload-pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let payload = platform::identity(payload).unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while db
        .query_row("SELECT status FROM runs WHERE id=?1", [&run], |row| {
            row.get::<_, String>(0)
        })
        .unwrap()
        != "running"
    {
        assert!(Instant::now() < deadline, "{}", app.stderr().unwrap());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let app_identity = platform::identity(app.pid()).unwrap();
    let before: Value = client
        .get(format!("{url}/api/update"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    fs::write(config.join("update-check-unknown.json"), json!({"checked_at":0,"latest":"0.2.99","installed_version":"0.2.99","installed_tag":"v0.2.99"}).to_string()).unwrap();
    let response = client
        .post(format!("{url}/api/update/restart"))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Ok(response) = client.get(format!("{url}/api/update")).send().await {
            if let Ok(after) = response.json::<Value>().await {
                if after["instance"] != before["instance"] {
                    break;
                }
            }
        }
        assert!(Instant::now() < deadline, "{}", app.stderr().unwrap());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(platform::alive(&identity).unwrap());
    assert!(platform::alive(&app_identity).unwrap());
    assert!(app.stderr().unwrap().contains("orx up: restarting"));
    let response = client
        .post(format!("{url}/api/runs/{run}/cancel"))
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    while !dir.join("exit.json").exists() {
        assert!(Instant::now() < deadline, "{}", app.stderr().unwrap());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let receipt = wrapper::receipt(&dir).unwrap().unwrap();
    assert_eq!(receipt.reason, orx_host_proto::ExitReason::Cancelled);
    assert!(receipt.processes.is_empty());
    assert_eq!(receipt.outcome, orx_host_proto::CancelOutcome::Verified);
    loop {
        let (status, contact): (String, Option<String>) = db
            .query_row(
                "SELECT status,contact FROM runs WHERE id=?1",
                [&run],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        if status == "cancelled" && contact.is_none() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{status} {contact:?}: {}",
            app.stderr().unwrap()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!platform::alive(&payload).unwrap());
    drop(app);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
