//! `orx down` — stop a running local or remote `orx up` dashboard server.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{anyhow, Result};
use crate::DownArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetProcess {
    pub pid: u32,
    pub port: Option<u16>,
}

/// Helper for `orx up` to record its running PID and port in the canonical data dir.
/// Removed on drop when the process exits.
pub struct UpPidFile {
    path: PathBuf,
}

impl UpPidFile {
    pub fn record(data_dir: &Path, port: u16) -> Option<Self> {
        let path = data_dir.join("orx-up.json");
        let payload = serde_json::json!({
            "pid": std::process::id(),
            "port": port,
            "startedAt": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        if std::fs::write(&path, serde_json::to_string_pretty(&payload).ok()?).is_ok() {
            Some(Self { path })
        } else {
            None
        }
    }
}

impl Drop for UpPidFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub async fn run(args: DownArgs) -> Result<()> {
    if let Some(host) = &args.remote {
        return run_remote(host, &args).await;
    }
    run_local(&args).await
}

async fn run_remote(host: &str, args: &DownArgs) -> Result<()> {
    let target = crate::commands::up_remote::parse_remote_target(host);
    let mut remote_cmd = "orx down".to_string();
    if let Some(port) = args.port {
        remote_cmd.push_str(&format!(" --port {port}"));
    }
    if args.force {
        remote_cmd.push_str(" --force");
    }
    if args.all {
        remote_cmd.push_str(" --all");
    }
    let output = crate::jobs::ssh::ssh_run(&target, &remote_cmd, None).await?;
    let trimmed = output.trim();
    if !trimmed.is_empty() {
        println!("{trimmed}");
    }
    Ok(())
}

async fn run_local(args: &DownArgs) -> Result<()> {
    let targets = find_orx_up_targets(args.port, args.all).await;

    if targets.is_empty() {
        // If no process was discovered from files/process-table, probe HTTP endpoint directly
        let port = args.port.unwrap_or(4791);
        if dashboard_is_serving(port).await {
            let _ = request_down_endpoint(port).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
            if !dashboard_is_serving(port).await {
                eprintln!("orx down: stopped OpenResearch dashboard on port {port}");
                return Ok(());
            }
        }
        eprintln!("orx down: no running OpenResearch dashboard found.");
        return Ok(());
    }

    let mut stopped_pids = Vec::new();
    for target in &targets {
        match stop_target(target, args.force).await {
            Ok(true) => stopped_pids.push(target.pid),
            Ok(false) => {}
            Err(err) => eprintln!("orx down: warning: {err}"),
        }
    }

    // Clean up pid file in canonical_data_dir if present
    if let Ok(data_dir) = crate::commands::remote_host::canonical_data_dir() {
        let pid_path = data_dir.join("orx-up.json");
        if let Ok(content) = std::fs::read_to_string(&pid_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(p) = val.get("pid").and_then(|v| v.as_u64()) {
                    if stopped_pids.contains(&(p as u32)) || !is_pid_alive(p as u32) {
                        let _ = std::fs::remove_file(&pid_path);
                    }
                }
            }
        }
    }

    if stopped_pids.is_empty() {
        eprintln!("orx down: no running OpenResearch dashboard found.");
    } else if stopped_pids.len() == 1 {
        eprintln!(
            "orx down: stopped OpenResearch dashboard (pid {})",
            stopped_pids[0]
        );
    } else {
        let pid_list = stopped_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!(
            "orx down: stopped {} OpenResearch dashboard instances (pids {})",
            stopped_pids.len(),
            pid_list
        );
    }
    Ok(())
}

async fn find_orx_up_targets(target_port: Option<u16>, all: bool) -> Vec<TargetProcess> {
    let my_pid = std::process::id();
    let mut targets = Vec::new();

    // 1. Read PID file from data_dir
    if let Ok(data_dir) = crate::commands::remote_host::canonical_data_dir() {
        let pid_path = data_dir.join("orx-up.json");
        if let Ok(content) = std::fs::read_to_string(&pid_path) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(p) = val.get("pid").and_then(|v| v.as_u64()) {
                    let pid = p as u32;
                    let port = val.get("port").and_then(|v| v.as_u64()).map(|p| p as u16);
                    if pid != my_pid && is_pid_alive(pid) {
                        let port_matches = match (target_port, port) {
                            (Some(tp), Some(p)) => tp == p,
                            (Some(tp), None) => tp == 4791,
                            (None, _) => true,
                        };
                        if port_matches || all {
                            targets.push(TargetProcess { pid, port });
                        }
                    } else if !is_pid_alive(pid) {
                        let _ = std::fs::remove_file(&pid_path);
                    }
                }
            }
        }
    }

    // 2. Check health endpoint on target_port (or default 4791)
    let port_to_probe = target_port.unwrap_or(4791);
    if let Some(pid) = dashboard_health_pid(port_to_probe).await {
        if pid != my_pid && is_pid_alive(pid) {
            targets.push(TargetProcess {
                pid,
                port: Some(port_to_probe),
            });
        }
    }

    // 3. Scan system processes
    #[cfg(target_os = "linux")]
    {
        scan_proc_targets(target_port, all, &mut targets);
        if targets.is_empty() {
            scan_ps_targets(target_port, all, &mut targets);
        }
    }

    #[cfg(all(unix, not(target_os = "linux")))]
    scan_ps_targets(target_port, all, &mut targets);

    #[cfg(windows)]
    scan_windows_targets(target_port, all, &mut targets);

    targets.retain(|t| t.pid != my_pid && is_pid_alive(t.pid));
    targets.sort_by_key(|t| t.pid);
    targets.dedup_by_key(|t| t.pid);
    targets
}

async fn stop_target(target: &TargetProcess, force: bool) -> Result<bool> {
    let pid = target.pid;
    if !is_pid_alive(pid) {
        return Ok(false);
    }

    if !force {
        // 1. Try graceful shutdown via HTTP endpoint if port is known
        if let Some(port) = target.port {
            if request_down_endpoint(port).await {
                for _ in 0..10 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    if !is_pid_alive(pid) {
                        return Ok(true);
                    }
                }
            }
        }

        // 2. Try termination signal (SIGTERM on Unix)
        if is_pid_alive(pid) {
            send_signal(pid, false);
            for _ in 0..15 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if !is_pid_alive(pid) {
                    return Ok(true);
                }
            }
        }
    }

    // 3. Force termination (SIGKILL on Unix / taskkill /F on Windows)
    if is_pid_alive(pid) {
        send_signal(pid, true);
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if !is_pid_alive(pid) {
                return Ok(true);
            }
        }
    }

    if is_pid_alive(pid) {
        return Err(anyhow!("Could not stop process {pid}"));
    }

    Ok(true)
}

fn send_signal(pid: u32, force: bool) {
    #[cfg(unix)]
    {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
    #[cfg(windows)]
    {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T"]);
        if force {
            cmd.arg("/F");
        }
        let _ = cmd.output();
    }
}

pub(crate) fn is_orx_up_cmdline(args: &[String]) -> Option<Option<u16>> {
    if args.is_empty() {
        return None;
    }
    let exe = &args[0];
    let exe_path = Path::new(exe);
    let exe_name = exe_path.file_name()?.to_string_lossy().to_lowercase();
    let is_orx_bin = exe_name == "orx"
        || exe_name == "orx.exe"
        || exe_name == "openresearch"
        || exe_name == "openresearch-cli"
        || exe_name.starts_with("orx-");
    if !is_orx_bin {
        return None;
    }

    let subargs = &args[1..];
    let has_up = subargs.iter().any(|arg| arg == "up");
    let has_down = subargs.iter().any(|arg| arg == "down");
    if !has_up || has_down {
        return None;
    }

    let mut port = None;
    let mut iter = subargs.iter().peekable();
    while let Some(arg) = iter.next() {
        if arg == "--port" || arg == "-p" {
            if let Some(next) = iter.next() {
                port = next.parse::<u16>().ok();
            }
        } else if let Some(p) = arg.strip_prefix("--port=") {
            port = p.parse::<u16>().ok();
        }
    }
    Some(port)
}

#[cfg(target_os = "linux")]
fn scan_proc_targets(target_port: Option<u16>, all: bool, targets: &mut Vec<TargetProcess>) {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return;
    };
    let my_pid = std::process::id();
    let my_uid = unsafe { libc::geteuid() };
    for entry in entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<u32>() else {
            continue;
        };
        if pid == my_pid {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            use std::os::unix::fs::MetadataExt as _;
            if meta.uid() != my_uid {
                continue;
            }
        }
        let cmdline_path = entry.path().join("cmdline");
        let Ok(bytes) = std::fs::read(cmdline_path) else {
            continue;
        };
        let args: Vec<String> = bytes
            .split(|&b| b == 0)
            .filter(|slice| !slice.is_empty())
            .map(|slice| String::from_utf8_lossy(slice).to_string())
            .collect();
        if let Some(detected_port) = is_orx_up_cmdline(&args) {
            let port = detected_port.or(Some(4791));
            let matches_port = match (target_port, port) {
                (Some(tp), Some(p)) => tp == p,
                (Some(tp), None) => tp == 4791,
                (None, _) => true,
            };
            if matches_port || all {
                targets.push(TargetProcess { pid, port });
            }
        }
    }
}

#[cfg(unix)]
fn scan_ps_targets(target_port: Option<u16>, all: bool, targets: &mut Vec<TargetProcess>) {
    let my_pid = std::process::id();
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,args="])
        .output();
    let Ok(output) = output else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let mut parts = line.split_whitespace();
        let Some(pid_str) = parts.next() else {
            continue;
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        if pid == my_pid {
            continue;
        }
        let args: Vec<String> = parts.map(|s| s.to_string()).collect();
        if let Some(detected_port) = is_orx_up_cmdline(&args) {
            let port = detected_port.or(Some(4791));
            let matches_port = match (target_port, port) {
                (Some(tp), Some(p)) => tp == p,
                (Some(tp), None) => tp == 4791,
                (None, _) => true,
            };
            if matches_port || all {
                targets.push(TargetProcess { pid, port });
            }
        }
    }
}

#[cfg(windows)]
fn scan_windows_targets(target_port: Option<u16>, all: bool, targets: &mut Vec<TargetProcess>) {
    let my_pid = std::process::id();
    let output = std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output();
    let Ok(output) = output else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split(',').map(|s| s.trim_matches('"')).collect();
        if fields.len() >= 2 {
            let image_name = fields[0].to_lowercase();
            if image_name == "orx.exe" || image_name == "openresearch.exe" {
                if let Ok(pid) = fields[1].parse::<u32>() {
                    if pid != my_pid {
                        targets.push(TargetProcess {
                            pid,
                            port: target_port.or(Some(4791)),
                        });
                    }
                }
            }
        }
    }
}

async fn dashboard_is_serving(port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(800))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://127.0.0.1:{port}/api/health"))
        .send()
        .await
    else {
        return false;
    };
    response
        .json::<serde_json::Value>()
        .await
        .is_ok_and(|body| {
            body.get("dashboardProtocol")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(crate::commands::up_remote::DASHBOARD_PROTOCOL))
        })
}

async fn dashboard_health_pid(port: u16) -> Option<u32> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(800))
        .build()
        .ok()?;
    let response = client
        .get(format!("http://127.0.0.1:{port}/api/health"))
        .send()
        .await
        .ok()?;
    let body = response.json::<serde_json::Value>().await.ok()?;
    if body
        .get("dashboardProtocol")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(crate::commands::up_remote::DASHBOARD_PROTOCOL))
    {
        return None;
    }
    body.get("pid")
        .and_then(serde_json::Value::as_u64)
        .map(|p| p as u32)
}

async fn request_down_endpoint(port: u16) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(800))
        .build()
    else {
        return false;
    };
    client
        .post(format!("http://127.0.0.1:{port}/api/down"))
        .send()
        .await
        .is_ok_and(|res| res.status().is_success())
}

#[cfg(unix)]
pub fn is_pid_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    if unsafe { libc::kill(pid, 0) } == 0 {
        #[cfg(target_os = "linux")]
        {
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                if let Some(state) = stat.split_whitespace().nth(2) {
                    return state != "Z";
                }
            }
        }
        true
    } else {
        false
    }
}

#[cfg(not(unix))]
pub fn is_pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        unsafe {
            let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
            if process.is_null() {
                return false;
            }
            let waited = WaitForSingleObject(process, 0);
            CloseHandle(process);
            waited == WAIT_TIMEOUT
        }
    }
    #[cfg(not(windows))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_orx_up_command_lines() {
        let cmd = vec!["orx".into(), "up".into()];
        assert_eq!(is_orx_up_cmdline(&cmd), Some(None));

        let cmd = vec![
            "/usr/local/bin/orx".into(),
            "up".into(),
            "--port".into(),
            "5000".into(),
        ];
        assert_eq!(is_orx_up_cmdline(&cmd), Some(Some(5000)));

        let cmd = vec![
            "target/debug/orx".into(),
            "up".into(),
            "--port=8080".into(),
            "--no-browser".into(),
        ];
        assert_eq!(is_orx_up_cmdline(&cmd), Some(Some(8080)));

        let cmd = vec!["orx".into(), "down".into()];
        assert_eq!(is_orx_up_cmdline(&cmd), None);

        let cmd = vec!["orx".into(), "exp".into(), "run".into(), "exp-1".into()];
        assert_eq!(is_orx_up_cmdline(&cmd), None);

        let cmd = vec!["python".into(), "orx".into(), "up".into()];
        assert_eq!(is_orx_up_cmdline(&cmd), None);
    }

    #[test]
    fn current_process_is_alive() {
        assert!(is_pid_alive(std::process::id()));
        assert!(!is_pid_alive(99999999));
    }
}
