use std::io::{BufRead, Write};
use std::process::Command;

#[test]
fn estimate_reads_parent_counts_without_launching_or_changing_the_command() {
    let root = std::env::temp_dir().join(format!("orx-estimate-{}", uuid::Uuid::new_v4()));
    let data = root.join("data");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_orx"));
        crate::home::isolate_home(&mut command, &root)
            .env("ORX_DATA_DIR", &data)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("ORX_NO_UPDATE_CHECK", "1")
            .arg("--no-telemetry");
        command
    };
    assert!(command()
        .args(["projects", "--json"])
        .output()
        .unwrap()
        .status
        .success());
    let db = rusqlite::Connection::open(data.join("orx.db")).unwrap();
    db.execute_batch(
        "INSERT INTO local_projects (id, name, slug, github_owner, github_repo, github_sync_enabled, baseline_branch, repo_path, created_at, updated_at)
         VALUES ('project', 'P', 'p', 'o', 'r', 0, 'main', '', 1, 1);
         INSERT INTO local_experiments (id, project_id, slug, branch_name, title, run_command, agent_status, created_at, updated_at)
         VALUES ('parent', 'project', 'parent', 'main', 'Measured baseline', 'echo parent', 'idle', 1, 1);
         INSERT INTO local_experiments (id, project_id, parent_experiment_id, slug, branch_name, run_command, agent_status, run_monitor_json, created_at, updated_at)
         VALUES ('child', 'project', 'parent', 'child', 'child', 'echo child', 'idle', '{\"plan\":[{\"phase\":\"download\"},{\"phase\":\"train\",\"of\":600,\"unit\":\"steps\"}]}', 1, 1);
         INSERT INTO runs (id, experiment_id, project_id, status, backend_json, launch_spec, command, created_at, updated_at)
         VALUES ('parent-run', 'parent', 'project', 'done', '{\"kind\":\"engine\"}', '{\"kind\":\"modal_job\",\"target\":null,\"options\":{\"gpu\":\"A100\",\"cpu\":null,\"memory\":null}}', 'echo parent', 1, 1);"
    ).unwrap();
    let mut snapshot = orx_signals::Snapshot::new("parent-run", "1-1", "generation");
    snapshot
        .feed(b"::orx:: {\"phase\":\"download\"}\n", false, 0, true)
        .unwrap();
    snapshot
        .feed(
            b"::orx:: {\"phase\":\"train\",\"step\":0,\"of\":200,\"unit\":\"steps\"}\n",
            false,
            60_000,
            true,
        )
        .unwrap();
    snapshot
        .feed(b"::orx:: {\"step\":200}\n", true, 660_000, true)
        .unwrap();
    std::fs::create_dir_all(data.join("run-signals")).unwrap();
    let checkpoint = data.join("run-signals/parent-run.json");
    std::fs::write(&checkpoint, serde_json::to_vec(&snapshot).unwrap()).unwrap();
    let marker = root.join("should-not-exist");
    let saved_command = format!(
        "python -m scripts.base_train --num-iterations=200 && touch '{}'",
        marker.display()
    );
    db.execute(
        "UPDATE local_experiments SET run_command = ? WHERE id = 'child'",
        [&saved_command],
    )
    .unwrap();
    let output = command()
        .args(["exp", "run", "child", "--estimate"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains(&saved_command), "{stdout}");
    assert!(
        stdout.contains("Measured baseline — parent-run, on --backend modal --flavor A100"),
        "{stdout}"
    );
    assert!(
        stdout.contains("600 steps; the parent took 3 s each"),
        "{stdout}"
    );
    assert!(stdout.contains("Total: about 31 min"), "{stdout}");
    assert!(
        stdout.contains("Cost: orx has no price for --backend local"),
        "{stdout}"
    );
    assert!(!marker.exists());
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM runs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT run_command FROM local_experiments WHERE id = 'child'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        saved_command
    );

    let hardware = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", hardware.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in hardware.incoming() {
            let mut stream = stream.unwrap();
            let mut request = std::io::BufReader::new(&stream);
            let mut line = String::new();
            while request.read_line(&mut line).unwrap() > 2 {
                line.clear();
            }
            let body = r#"[{"name":"h200","unitCostMicroUSD":6000000,"unitLabel":"hour"}]"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    let priced = command()
        .args(["exp", "run", "child", "--estimate"])
        .args(["--backend", "hf", "--flavor", "h200"])
        .env("HF_ENDPOINT", &endpoint)
        .env("NO_PROXY", "127.0.0.1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&priced.stdout);
    assert!(priced.status.success(), "{stdout}");
    assert!(
        stdout.contains("Price: $6.00/hr for --backend hf --flavor h200, from 127.0.0.1"),
        "{stdout}"
    );
    assert!(stdout.contains("Cost: about $3.10"), "{stdout}");

    let unlisted = command()
        .args(["exp", "run", "child", "--estimate"])
        .args(["--backend", "hf", "--flavor", "a100-large"])
        .env("HF_ENDPOINT", &endpoint)
        .env("NO_PROXY", "127.0.0.1")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&unlisted.stdout);
    assert!(unlisted.status.success(), "{stdout}");
    assert!(stdout.contains("Total: about 31 min"), "{stdout}");
    assert!(
        stdout.contains("Cost: unknown for --backend hf --flavor a100-large: "),
        "{stdout}"
    );

    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_endpoint = format!("http://{}", silent.local_addr().unwrap());
    std::thread::spawn(move || {
        let mut held = Vec::new();
        for stream in silent.incoming() {
            held.push(stream.unwrap());
        }
    });
    let stalled = command()
        .args(["exp", "run", "child", "--estimate"])
        .args(["--backend", "hf", "--flavor", "h200"])
        .env("HF_ENDPOINT", &silent_endpoint)
        .env("NO_PROXY", "127.0.0.1")
        .env("ORX_TEST_SPEEDUP", "60")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&stalled.stdout);
    assert!(stalled.status.success(), "{stdout}");
    assert!(stdout.contains("Total: about 31 min"), "{stdout}");
    assert!(
        stdout.contains("Cost: unknown for --backend hf --flavor h200: no price within"),
        "{stdout}"
    );

    let refused = command()
        .args(["exp", "run", "child", "--estimate"])
        .args(["--backend", "ssh", "--flavor", "h200"])
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--flavor"));

    let preview = command()
        .args([
            "exp",
            "run",
            "child",
            "--estimate",
            "--command",
            "echo preview",
        ])
        .output()
        .unwrap();
    assert!(!preview.status.success());
    assert!(String::from_utf8_lossy(&preview.stderr).contains("--command"));

    db.execute("UPDATE local_experiments SET run_monitor_json = '{\"plan\":[{\"phase\":\"eval\"}]}' WHERE id = 'child'", []).unwrap();
    let output = command()
        .args(["exp", "run", "child", "--estimate"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success());
    assert!(
        stdout.contains("Total: unknown (eval not timed)"),
        "{stdout}"
    );

    std::fs::write(checkpoint, "broken").unwrap();
    let output = command()
        .args(["exp", "run", "child", "--estimate"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("parent-run"));
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
