#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Sandbox(PathBuf);
impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn wait_for(timeout: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// OR-334/OR-335: an idle `orx up` must not keep serving old code after the
/// auto-updater swaps a newer orx in underneath it.
#[test]
fn idle_orx_up_relaunches_into_an_installed_update() {
    let root = std::env::temp_dir().join(format!("orx-up-restart-{}", uuid::Uuid::new_v4()));
    let _sandbox = Sandbox(root.clone());
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let orx = bin.join("orx");
    std::fs::copy(env!("CARGO_BIN_EXE_orx"), &orx).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();

    let mut up = Command::new(&orx)
        .args(["--no-telemetry", "up", "--no-browser", "--port"])
        .arg(port.to_string())
        .env("HOME", &root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("share"))
        .env("ORX_DATA_DIR", root.join("data"))
        .env("PATH", "/usr/bin:/bin")
        .env("GITHUB_TOKEN", "")
        // Keep the startup release check off the network.
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env_remove("ORX_NO_UPDATE_CHECK")
        .env_remove("NO_UPDATE_NOTIFIER")
        .env_remove("OPENRESEARCH_CLI_DISABLE_UPDATE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let serving = wait_for(Duration::from_secs(60), || {
        std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });

    // What the installer does: rename a newer orx over the running one, then record it.
    let marker = root.join("relaunched");
    let next = bin.join("orx.next");
    std::fs::write(
        &next,
        format!(
            "#!/bin/sh\n[ \"$1\" = --version ] && echo 'orx 99.0.0' && exit 0\necho \"$@\" > '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&next, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(&next, &orx).unwrap();
    let config = root.join("config/openresearch");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("update-check-unknown.json"),
        r#"{"checked_at":4102444800,"latest":"99.0.0","installed_version":"99.0.0"}"#,
    )
    .unwrap();

    let relaunched = wait_for(Duration::from_secs(45), || marker.exists());
    let _ = up.kill();
    let _ = up.wait();
    assert!(serving, "orx up never started serving");
    assert!(relaunched, "idle orx up kept serving its old code");
    let args = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(
        args.trim(),
        format!("--no-telemetry up --no-browser --port {port}")
    );
}
