use super::*;

#[test]
fn references_are_names_not_shell_or_docker_options() {
    for value in ["", " ", "--help", "a/b", "a;id", "a\nb"] {
        assert!(validate_reference(value).is_err());
    }
    for value in ["none", "research-v1.2", "a_b", "012345abcdef"] {
        validate_reference(value).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn engine_run_sh_runs_the_payload_inside_the_container_with_its_source() {
    let temp = crate::local::git::TemporaryDirectory::new("orx-engine-container").unwrap();
    let host = temp.path().join("host-run");
    let staged = temp.path().join("staged");
    let bin = temp.path().join("bin");
    for dir in [&host, &staged, &bin] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(staged.join("source.txt"), "snapshot\n").unwrap();
    assert!(crate::process::command("tar")
        .arg("-cf")
        .arg(host.join("source.tar"))
        .arg("-C")
        .arg(&staged)
        .arg("source.txt")
        .status()
        .unwrap()
        .success());
    // `docker exec` runs its command here; the container is this machine.
    orx_test_support::write_executable(
        bin.join("docker"),
        r#"#!/bin/bash
echo "$*" >> "$CALLS"
case "$1" in
  container) echo '2026-10-30T00:00:00Z running';;
  exec)
    shift
    [ "$1" = -i ] && shift
    shift
    [ "$1" = setsid ] && shift 2
    exec "$@";;
esac
"#,
        0o755,
    )
    .unwrap();
    let container = ContainerRun {
        reference: "research".into(),
        id: "cid".into(),
        started_at: "2026-10-30T00:00:00Z".into(),
        run_dir: temp
            .path()
            .join("container/.orx/runs")
            .to_string_lossy()
            .into_owned(),
        unknown: Default::default(),
    }
    .for_run("run");
    let run_sh = container.engine_run_sh(
        "export ORX_TEST_SECRET='quote'\"'\"' and $literal'",
        &crate::compute::staged_script(
            "cat source.txt > ../out; echo \"$ORX_TEST_SECRET\" >> ../out; exit 3",
        ),
        Some("source.tar"),
    );
    std::fs::write(host.join("run.sh"), run_sh).unwrap();
    let status = crate::process::command("bash")
        .arg("run.sh")
        .current_dir(&host)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("CALLS", temp.path().join("calls"))
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(3));
    assert_eq!(
        std::fs::read_to_string(std::path::Path::new(&container.run_dir).join("out")).unwrap(),
        "snapshot\nquote' and $literal\n"
    );
    let calls = std::fs::read_to_string(temp.path().join("calls")).unwrap();
    assert!(calls.starts_with("container inspect"), "{calls}");
    assert!(calls.contains("exec cid setsid --wait bash"), "{calls}");
    assert!(container
        .engine_cancel_sh()
        .contains("docker exec -i 'cid' bash -c"));
}
