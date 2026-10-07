//! Local backend — run an experiment as a detached process on this machine.
//!
//! The no-transport twin of `jobs/ssh.rs`: same run-dir layout
//!   run.sh      the launcher (clone-and-run payload)
//!   log         merged stdout/stderr
//!   pid         the detached process-group leader
//!   exit_code   written when the payload finishes
//! but under the orx data dir (`<data dir>/local-runs/<run_id>/`) instead of a
//! remote `~/.orx/runs/`. A restarted `orx supervise` reattaches purely from
//! that directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};
use crate::jobs::ssh::{sh_quote, JobState};

#[cfg(windows)]
mod python;
#[cfg(windows)]
mod windows;

/// The run's working directory: `<data dir>/local-runs/<run id>`.
pub fn run_dir(run_id: &str) -> PathBuf {
    // Run ids are locally-minted UUIDs; sanitize anyway (same as log_path).
    let safe: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    crate::store::data_dir().join("local-runs").join(safe)
}

pub struct LocalJobSpec {
    /// Names the run dir `<data dir>/local-runs/<run_id>`.
    pub run_id: String,
    /// The shared clone-and-run payload (`bash` script body).
    pub script: String,
    /// Inherited by the launcher without being written into run.sh.
    pub env: HashMap<String, String>,
    /// Inherited by the controller without being written into run.sh.
    pub secret_env: HashMap<String, String>,
}

/// Submit the job: write run.sh, launch it detached in its own process group
/// (pid == pgid, so cancel can TERM the whole tree), record the pid. Returns
/// the run dir — the reattach handle stored on the descriptor.
pub fn run_job(spec: &LocalJobSpec) -> Result<PathBuf> {
    let dir = run_dir(&spec.run_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow!("Could not create {}: {}", dir.display(), e))?;
    let env = super::default_python_env(&spec.env);
    #[cfg(not(windows))]
    // Keep the launcher identifiable until background children exit, including after a payload cd.
    let prelude = format!(
        "trap 'exit 143' TERM\ntrap {} EXIT\n",
        sh_quote(&format!(
            "code=$?; wait; echo \"$code\" > {}; exit \"$code\"",
            sh_quote(&crate::local::bash::bash_path(&dir.join("exit_code")))
        ))
    );
    #[cfg(windows)]
    let prelude = python::prelude(&dir)
        .map_err(|e| anyhow!("Could not set up Python for {}: {}", dir.display(), e))?;
    #[cfg(not(windows))]
    let record_exit = "";
    #[cfg(windows)]
    let record_exit = "echo $? > exit_code\n";
    let run_sh = format!(
        "#!/usr/bin/env bash\ncd {dir} || exit 97\n(\n{prelude}{script}\n) > log 2>&1\n{record_exit}",
        dir = sh_quote(&crate::local::bash::bash_path(&dir)),
        script = spec.script,
    );
    let run_sh_path = dir.join("run.sh");
    std::fs::write(&run_sh_path, run_sh)
        .map_err(|e| anyhow!("Could not write {}: {}", run_sh_path.display(), e))?;
    #[cfg(unix)]
    {
        // The payload may write sensitive output, so keep its artifacts owner-only.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        let _ = std::fs::set_permissions(&run_sh_path, std::fs::Permissions::from_mode(0o600));
    }

    let mut cmd = std::process::Command::new(crate::local::bash::program());
    if let Some(path) =
        crate::local::bash::path_with_toolchain(crate::local::shell_env::search_path())
    {
        cmd.env("PATH", path);
    }
    #[cfg(not(windows))]
    {
        let script_path = crate::paths::canonicalize(&run_sh_path)?;
        std::fs::write(
            dir.join("pid_script"),
            script_path.to_string_lossy().as_bytes(),
        )?;
        cmd.arg(script_path);
    }
    #[cfg(windows)]
    cmd.arg("run.sh");
    cmd.envs(&env)
        .envs(&spec.secret_env)
        .current_dir(&dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    windows::spawn(&mut cmd, &dir).map_err(|e| anyhow!("Could not launch the local run: {}", e))?;
    #[cfg(not(windows))]
    {
        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow!("Could not launch the local run: {}", e))?;
        let pid = child.id();
        let completed_dir = dir.clone();
        std::thread::spawn(move || {
            if let Ok(status) = child.wait() {
                if exit_code_state(&completed_dir).is_none() {
                    let code = status.code().unwrap_or(-1);
                    if let Err(error) =
                        std::fs::write(completed_dir.join("exit_code"), format!("{code}\n"))
                    {
                        eprintln!("Could not record local run exit: {error}");
                    }
                }
            }
        });
        std::fs::write(dir.join("pid"), format!("{pid}\n"))
            .map_err(|e| anyhow!("Could not record the run's pid: {}", e))?;
    }
    Ok(dir)
}

/// The unique script path distinguishes a run from an unrelated process reusing its PID.
#[cfg(not(windows))]
fn run_alive(dir: &Path, pid: &str) -> bool {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-ww", "-o", "stat=,command=", "-p", pid])
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return false;
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let Some((stat, command)) = text.trim().split_once(char::is_whitespace) else {
        return false;
    };
    if stat.starts_with('Z') {
        return false;
    }
    match std::fs::read_to_string(dir.join("pid_script")) {
        Ok(script) => command
            .strip_suffix(&script)
            .is_some_and(|prefix| prefix.ends_with(' ')),
        // Runs launched before pid_script existed still use their original PID contract.
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(windows)]
fn run_alive(_dir: &Path, pid: &str) -> bool {
    pid_alive(pid)
}

/// Windows has no `ps`. A zero-timeout wait, not the exit code, where a real 259 reads as live.
#[cfg(windows)]
fn pid_alive(pid: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    let Ok(pid) = pid.trim().parse::<u32>() else {
        return false;
    };
    // SAFETY: plain syscalls; the handle is closed on every path out.
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

/// The terminal state recorded in exit_code, if any. An empty file is run.sh
/// mid-write (`>` truncates before the code lands) — not terminal yet.
fn exit_code_state(dir: &Path) -> Option<JobState> {
    let raw = std::fs::read_to_string(dir.join("exit_code")).ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let code: i32 = raw.parse().unwrap_or(-1);
    Some(if code == 0 {
        JobState {
            stage: "COMPLETED".into(),
            message: None,
        }
    } else {
        JobState {
            stage: "ERROR".into(),
            message: Some(format!("exited with code {code}")),
        }
    })
}

/// Job state in the shared stage vocabulary (see `jobs::stage_to_run_status`).
/// exit_code present -> finished; pid alive -> running; pid dead & no
/// exit_code -> killed/crashed.
pub fn inspect_job(dir: &Path) -> JobState {
    if let Some(state) = exit_code_state(dir) {
        return state;
    }
    match std::fs::read_to_string(dir.join("pid")) {
        Ok(pid) if run_alive(dir, pid.trim()) => JobState {
            stage: "RUNNING".into(),
            message: None,
        },
        // Dead pid: run.sh may have written exit_code and exited between the
        // check above and the ps probe — re-read before calling it killed.
        Ok(_) => {
            for _ in 0..3 {
                if let Some(state) = exit_code_state(dir) {
                    return state;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if std::fs::metadata(dir.join("pid"))
                .and_then(|metadata| metadata.modified())
                .and_then(|modified| modified.elapsed().map_err(std::io::Error::other))
                .is_ok_and(|age| age < std::time::Duration::from_secs(1))
            {
                return JobState {
                    stage: "RUNNING".into(),
                    message: None,
                };
            }
            JobState {
                stage: "ERROR".into(),
                message: Some("process died without an exit code (killed?)".into()),
            }
        }
        // pid not written yet — just starting.
        Err(_) => JobState {
            stage: "RUNNING".into(),
            message: None,
        },
    }
}

/// One poll of the log past `skip` lines (the supervisor loops every ~2s).
/// A missing log file just means the payload hasn't printed yet.
pub fn stream_logs(
    dir: &Path,
    skip: u64,
    drain: bool,
    sink: &mut (dyn FnMut(&str) + Send),
) -> Result<u64> {
    let content = match std::fs::read_to_string(dir.join("log")) {
        Ok(c) => c,
        Err(_) => return Ok(skip),
    };
    let mut seen = skip;
    for line in super::log_lines(&content, drain).skip(skip as usize) {
        seen += 1;
        sink(line);
    }
    Ok(seen)
}

/// TERM the process group (pid == pgid), else the pid alone; on Windows, the process tree.
pub fn cancel_job(dir: &Path) -> Result<()> {
    let pid = std::fs::read_to_string(dir.join("pid"))
        .map_err(|e| anyhow!("Could not read the run's pid: {}", e))?;
    let pid = pid.trim().to_string();
    #[cfg(windows)]
    {
        windows::cancel(dir, &pid)
    }
    #[cfg(not(windows))]
    {
        if !run_alive(dir, &pid) {
            return Ok(());
        }
        terminate_group(&pid)
    }
}

/// `/T` also kills the python the launcher started, as TERMing the group does on unix.
#[cfg(windows)]
fn terminate_tree(pid: &str) -> Result<()> {
    // `/T` fails if any descendant already exited, so success is read from the leader's liveness.
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", pid, "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    for _ in 0..50 {
        if !pid_alive(pid) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(anyhow!("Could not terminate local process tree {pid}"))
}

#[cfg(not(windows))]
fn terminate_group(pid: &str) -> Result<()> {
    let group = std::process::Command::new("kill")
        .args(["-TERM", "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !group {
        let process = std::process::Command::new("kill")
            .args(["-TERM", pid])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !process {
            return Err(anyhow!("Could not terminate local process group {pid}"));
        }
    }
    Ok(())
}

/// What "this machine" is, for the Compute settings card: the hardware a
/// `--backend local` run gets. Matters most when the dashboard is reached over
/// port forwarding from a GPU box — the card is how the user sees they're
/// sitting on real GPUs.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalHardware {
    pub hostname: String,
    pub os: &'static str,
    pub arch: &'static str,
    /// CPU brand string on macOS (e.g. "Apple M2 Pro"); NVIDIA-less Linux
    /// boxes just show cores/RAM.
    pub chip: Option<String>,
    pub cpu_count: usize,
    pub mem_bytes: Option<u64>,
    pub gpus: Vec<Gpu>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub mem_mib: Option<u64>,
}

/// Best-effort hardware probe. Every field degrades independently — a missing
/// `nvidia-smi` (or any probe failure) is an empty GPU list, never an error.
/// Blocking (subprocesses); call via `spawn_blocking` from async handlers.
pub fn hardware_info() -> LocalHardware {
    let cmd = |name: &str, args: &[&str]| -> Option<String> {
        let out = std::process::Command::new(name).args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let mem_bytes = if cfg!(target_os = "macos") {
        cmd("sysctl", &["-n", "hw.memsize"]).and_then(|s| s.parse().ok())
    } else {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|raw| {
                // "MemTotal:       32763528 kB"
                raw.lines()
                    .find(|l| l.starts_with("MemTotal:"))?
                    .split_whitespace()
                    .nth(1)?
                    .parse::<u64>()
                    .ok()
                    .map(|kb| kb * 1024)
            })
    };
    LocalHardware {
        hostname: cmd("hostname", &[]).unwrap_or_else(|| "unknown".to_string()),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        chip: if cfg!(target_os = "macos") {
            cmd("sysctl", &["-n", "machdep.cpu.brand_string"])
        } else {
            None
        },
        cpu_count: std::thread::available_parallelism().map_or(0, |n| n.get()),
        mem_bytes,
        gpus: cmd(
            "nvidia-smi",
            &[
                "--query-gpu=name,memory.total",
                "--format=csv,noheader,nounits",
            ],
        )
        .map(|out| parse_nvidia_smi_csv(&out))
        .unwrap_or_default(),
    }
}

/// Parse `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`:
/// one `name, mem_mib` line per GPU. Names may contain no commas in this
/// format (nvidia-smi separates fields with ", "), but tolerate odd lines by
/// keeping the name and dropping the memory rather than dropping the GPU.
fn parse_nvidia_smi_csv(out: &str) -> Vec<Gpu> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| match line.rsplit_once(',') {
            Some((name, mem)) => Gpu {
                name: name.trim().to_string(),
                mem_mib: mem.trim().parse().ok(),
            },
            None => Gpu {
                name: line.trim().to_string(),
                mem_mib: None,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_csv_two_gpus() {
        let parsed =
            parse_nvidia_smi_csv("NVIDIA A100-SXM4-80GB, 81920\nNVIDIA A100-SXM4-80GB, 81920\n");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].name, "NVIDIA A100-SXM4-80GB");
        assert_eq!(parsed[0].mem_mib, Some(81920));
    }

    #[test]
    fn nvidia_smi_csv_empty_and_malformed() {
        assert!(parse_nvidia_smi_csv("").is_empty());
        assert!(parse_nvidia_smi_csv("\n  \n").is_empty());
        let odd = parse_nvidia_smi_csv("Tesla T4");
        assert_eq!(odd.len(), 1, "GPU kept even without a memory field");
        assert_eq!(odd[0].name, "Tesla T4");
        assert_eq!(odd[0].mem_mib, None);
    }

    fn wait_terminal(dir: &Path) -> JobState {
        let mut state = inspect_job(dir);
        for _ in 0..100 {
            if state.stage != "RUNNING" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            state = inspect_job(dir);
        }
        state
    }

    #[cfg(unix)]
    fn assert_reaped(dir: &Path) {
        let pid = std::fs::read_to_string(dir.join("pid")).unwrap();
        for _ in 0..100 {
            let output = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            if output.stdout.is_empty() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("local run {} was not reaped", pid.trim());
    }

    #[test]
    fn local_job_lifecycle() {
        // The only test that touches ORX_DATA_DIR, so the global env is safe.
        let base = std::env::temp_dir().join(format!("orx-localbox-test-{}", uuid::Uuid::new_v4()));
        std::env::set_var("ORX_DATA_DIR", &base);

        let dir = run_job(&LocalJobSpec {
            run_id: "lifecycle".into(),
            script: "[ -n \"$TINKER_API_KEY\" ] && echo hello-$ORX_TEST_VAR-$HF_TOKEN".into(),
            env: HashMap::from([
                ("ORX_TEST_VAR".to_string(), "42".to_string()),
                ("HF_TOKEN".to_string(), "fake-token".to_string()),
            ]),
            secret_env: HashMap::from([("TINKER_API_KEY".to_string(), "s3cr3t-value".to_string())]),
        })
        .unwrap();
        let state = wait_terminal(&dir);
        assert_eq!(state.stage, "COMPLETED", "message: {:?}", state.message);
        #[cfg(unix)]
        assert_reaped(&dir);

        let run_sh = std::fs::read_to_string(dir.join("run.sh")).unwrap();
        assert!(!run_sh.contains("fake-token"));
        assert!(!run_sh.contains("s3cr3t-value"));

        let mut lines = Vec::new();
        let seen = stream_logs(&dir, 0, false, &mut |l| lines.push(l.to_string())).unwrap();
        assert_eq!(seen, 1);
        assert_eq!(lines, ["hello-42-fake-token"]);
        // Re-poll past the consumed lines: nothing new.
        assert_eq!(stream_logs(&dir, seen, false, &mut |_| ()).unwrap(), seen);
        #[cfg(windows)]
        {
            cancel_job(&dir).unwrap();
            assert_eq!(inspect_job(&dir).stage, "COMPLETED");
        }

        let failed = run_job(&LocalJobSpec {
            run_id: "failing".into(),
            script: "exit 3".into(),
            env: HashMap::new(),
            secret_env: HashMap::new(),
        })
        .unwrap();
        let state = wait_terminal(&failed);
        assert_eq!(state.stage, "ERROR");
        assert_eq!(state.message.as_deref(), Some("exited with code 3"));

        #[cfg(unix)]
        assert_reaped(&failed);

        let cancelled = run_job(&LocalJobSpec {
            run_id: "cancelled".into(),
            script: "sleep 60".into(),
            env: HashMap::new(),
            secret_env: HashMap::new(),
        })
        .unwrap();
        assert_eq!(inspect_job(&cancelled).stage, "RUNNING");
        #[cfg(windows)]
        drop(windows::open_job(&cancelled).expect("the launcher must retain the job name"));
        cancel_job(&cancelled).unwrap();
        let state = wait_terminal(&cancelled);
        // TERM leaves either a dead pid with no exit_code, or a non-zero
        // exit_code if run.sh got to write one — ERROR either way.
        assert_eq!(state.stage, "ERROR");

        #[cfg(unix)]
        assert_reaped(&cancelled);

        let descendants = run_job(&LocalJobSpec {
            run_id: "descendants".into(),
            script: "(for i in {1..100}; do echo tick >> heartbeat; sleep 0.1; done) & wait".into(),
            env: HashMap::new(),
            secret_env: HashMap::new(),
        })
        .unwrap();
        for _ in 0..100 {
            if descendants.join("heartbeat").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let started = descendants.join("heartbeat").exists();
        cancel_job(&descendants).unwrap();
        assert!(started, "the descendant did not start");
        assert_eq!(wait_terminal(&descendants).stage, "ERROR");
        #[cfg(unix)]
        assert_reaped(&descendants);

        let heartbeat = std::fs::read(descendants.join("heartbeat")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(
            std::fs::read(descendants.join("heartbeat")).unwrap(),
            heartbeat
        );

        #[cfg(unix)]
        {
            let background = run_job(&LocalJobSpec {
                run_id: "background".into(),
                script: "mkdir repo; cd repo; (for i in {1..100}; do echo tick >> ../heartbeat; sleep 0.1; done) &".into(),
                env: HashMap::new(),
                secret_env: HashMap::new(),
            })
            .unwrap();
            let heartbeat_path = background.join("heartbeat");
            for _ in 0..100 {
                if heartbeat_path.exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            assert!(
                heartbeat_path.exists(),
                "the background child did not start"
            );
            assert_eq!(inspect_job(&background).stage, "RUNNING");
            assert!(!background.join("exit_code").exists());
            cancel_job(&background).unwrap();
            assert_reaped(&background);
            let heartbeat = std::fs::read(background.join("heartbeat")).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            assert_eq!(
                std::fs::read(background.join("heartbeat")).unwrap(),
                heartbeat,
                "rollback must terminate descendants after the payload finishes"
            );
        }

        #[cfg(unix)]
        {
            let stale = base.join("stale-pid");
            std::fs::create_dir_all(&stale).unwrap();
            let mut unrelated = std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .unwrap();
            std::fs::write(stale.join("pid"), unrelated.id().to_string()).unwrap();
            std::fs::write(
                stale.join("pid_script"),
                stale.join("run.sh").to_string_lossy().as_bytes(),
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_secs(1));
            let state = inspect_job(&stale);
            let cancelled = cancel_job(&stale);
            let alive = unrelated.try_wait().unwrap().is_none();
            let _ = unrelated.kill();
            unrelated.wait().unwrap();
            assert_eq!(state.stage, "ERROR");
            cancelled.unwrap();
            assert!(alive, "cancellation must not signal a reused PID");
        }

        std::env::remove_var("ORX_DATA_DIR");
        let _ = std::fs::remove_dir_all(&base);
    }
}
