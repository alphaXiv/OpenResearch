use std::path::{Path, PathBuf};

use crate::error::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct JobState {
    pub stage: String,
    pub message: Option<String>,
    pub exit_code: Option<i32>,
}

#[cfg(windows)]
pub(crate) mod python;
#[cfg(windows)]
pub(crate) mod windows;

/// The run's working directory: `<data dir>/local-runs/<run id>`.
pub fn run_dir(run_id: &str) -> PathBuf {
    // Run ids are locally-minted UUIDs; sanitize anyway (same as log_path).
    let safe: String = run_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    crate::store::data_dir().join("local-runs").join(safe)
}

/// The unique script path distinguishes a run from an unrelated process reusing its PID.
#[cfg(not(windows))]
fn run_alive(dir: &Path, pid: &str) -> bool {
    run_alive_via("ps", dir, pid)
}

#[cfg(not(windows))]
fn run_alive_via(ps: &str, dir: &Path, pid: &str) -> bool {
    let Ok(output) = crate::process::command(ps)
        .args(["-ww", "-o", "stat=,command=", "-p", pid])
        .stderr(std::process::Stdio::null())
        .output()
    else {
        // Agent sandboxes (Codex's seatbelt) refuse to exec setuid `ps`; this probe cannot rule out PID reuse.
        return pid_alive(pid);
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

#[cfg(not(windows))]
fn pid_alive(pid: &str) -> bool {
    let Ok(pid) = pid.parse::<libc::pid_t>() else {
        return false;
    };
    // pid > 0 excludes group and broadcast targets.
    pid > 0 && signal_target_exists(pid)
}

/// EPERM still means the target exists, just not one we may signal.
#[cfg(not(windows))]
fn signal_target_exists(target: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks existence; callers exclude broadcast targets.
    (unsafe { libc::kill(target, 0) }) == 0
        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
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
pub(crate) fn exit_code_state(dir: &Path) -> Option<JobState> {
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
            exit_code: Some(code),
        }
    } else {
        JobState {
            stage: "ERROR".into(),
            message: Some(format!("exited with code {code}")),
            exit_code: Some(code),
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
            exit_code: None,
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
                    exit_code: None,
                };
            }
            JobState {
                stage: "ERROR".into(),
                message: Some("process died without an exit code (killed?)".into()),
                exit_code: None,
            }
        }
        // pid not written yet — just starting.
        Err(_) => JobState {
            stage: "RUNNING".into(),
            message: None,
            exit_code: None,
        },
    }
}

const LINGERING_GROUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Whether the run may still have live processes, including a group lingering after exit_code.
/// Called only while submission is excluded and a supervisor lock is checked separately.
pub fn orphaned_job_is_running(dir: &Path) -> Result<bool> {
    #[cfg(unix)]
    if dir.join("launch.json").try_exists()? || dir.join("wrapper.json").try_exists()? {
        return Ok(orx_host::wrapper::has_live_processes(dir)?);
    }
    #[cfg(windows)]
    if dir.join("windows_job").try_exists()? {
        return Ok(windows::job_is_running(dir)?);
    }
    if exit_code_state(dir).is_some() {
        // Children may outlive exit_code briefly; after the window a live PGID is likely reused.
        let age = std::fs::metadata(dir.join("exit_code"))?
            .modified()?
            .elapsed()
            .unwrap_or_default();
        return Ok(age < LINGERING_GROUP_WINDOW && process_group_alive(dir));
    }
    match std::fs::read_to_string(dir.join("pid")) {
        Ok(_) => return Ok(inspect_job(dir).stage == "RUNNING"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    #[cfg(not(windows))]
    {
        let script = match dir.join("run.sh").canonicalize() {
            Ok(script) => script,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let output = crate::process::command("ps")
            .args(["-A", "-ww", "-o", "stat=,command="])
            .output()?;
        if !output.status.success() {
            return Err(anyhow!("Could not inspect orphaned local processes"));
        }
        Ok(String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            line.trim()
                .split_once(char::is_whitespace)
                .is_some_and(|(stat, command)| {
                    !stat.starts_with('Z')
                        && command
                            .strip_suffix(script.to_string_lossy().as_ref())
                            .is_some_and(|prefix| prefix.ends_with(' '))
                })
        }))
    }
    #[cfg(windows)]
    // The native launcher records its PID before resuming the suspended process.
    Ok(false)
}

/// Whether any process remains in the run's group (pgid == recorded pid).
#[cfg(not(windows))]
pub(crate) fn process_group_alive(dir: &Path) -> bool {
    let Ok(pid) = std::fs::read_to_string(dir.join("pid")) else {
        return false;
    };
    let Ok(group) = pid.trim().parse::<libc::pid_t>() else {
        return false;
    };
    // -1 would broadcast to every process.
    group > 1 && signal_target_exists(-group)
}

/// Windows cancel waits for an empty job object; deleting files a survivor holds open fails and is retried.
#[cfg(windows)]
pub(crate) fn process_group_alive(dir: &Path) -> bool {
    if dir.join("windows_job").exists() {
        windows::job_is_running(dir).unwrap_or(true)
    } else {
        false
    }
}

/// TERM the process group (pid == pgid), else the pid alone; on Windows, the process tree.
pub fn cancel_job(dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        windows::cancel(dir)
    }
    #[cfg(not(windows))]
    {
        let pid = std::fs::read_to_string(dir.join("pid"))
            .map_err(|e| anyhow!("Could not read the run's pid: {}", e))?;
        let pid = pid.trim().to_string();
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
    use std::os::windows::process::CommandExt;
    let _ = crate::process::command("taskkill")
        .args(["/PID", pid, "/T", "/F"])
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
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
    let group = crate::process::command("kill")
        .args(["-TERM", "--", &format!("-{pid}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !group {
        let process = crate::process::command("kill")
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
        let out = crate::process::command(name).args(args).output().ok()?;
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

/// Writes `dir/pid` for a group whose leader has exited; the caller owns and must reap the survivor.
#[cfg(all(test, unix))]
pub(crate) fn spawn_lingering_group(dir: &Path) -> std::process::Child {
    use std::os::unix::process::CommandExt;
    let mut leader = crate::process::command("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let survivor = crate::process::command("sleep")
        .arg("30")
        .process_group(i32::try_from(leader.id()).unwrap())
        .spawn()
        .unwrap();
    leader.kill().unwrap();
    leader.wait().unwrap();
    std::fs::write(dir.join("pid"), leader.id().to_string()).unwrap();
    survivor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn process_group_stays_alive_while_a_survivor_outlives_the_leader() {
        let dir = std::env::temp_dir().join(format!("orx-group-probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!process_group_alive(&dir));
        let mut survivor = spawn_lingering_group(&dir);
        assert!(process_group_alive(&dir));
        std::fs::write(dir.join("exit_code"), "143").unwrap();
        assert!(orphaned_job_is_running(&dir).unwrap());
        std::fs::File::options()
            .write(true)
            .open(dir.join("exit_code"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - 2 * LINGERING_GROUP_WINDOW)
            .unwrap();
        assert!(!orphaned_job_is_running(&dir).unwrap());
        survivor.kill().unwrap();
        survivor.wait().unwrap();
        assert!(!process_group_alive(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn orphaned_job_probe_distinguishes_an_abandoned_launch_from_a_missing_pid() {
        let dir = std::env::temp_dir().join(format!("orx orphan probe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("run.sh");
        std::fs::write(&script, "read -r value\n").unwrap();
        assert!(!orphaned_job_is_running(&dir).unwrap());
        let mut child = crate::process::command(crate::local::bash::program())
            .arg(script.canonicalize().unwrap())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let running = orphaned_job_is_running(&dir).unwrap();
        drop(child.stdin.take());
        child.wait().unwrap();
        assert!(running);
        assert!(!orphaned_job_is_running(&dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

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

    #[cfg(not(windows))]
    #[test]
    fn run_alive_without_ps_probes_the_pid() {
        let dir = std::env::temp_dir();
        let unrunnable_ps = "/nonexistent/orx-test-ps";
        let mut child = crate::process::command("sleep").arg("60").spawn().unwrap();
        let pid = child.id().to_string();
        let alive = run_alive_via(unrunnable_ps, &dir, &pid);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(alive, "a live run must not read as dead when ps cannot run");
        assert!(!run_alive_via(unrunnable_ps, &dir, &pid));
        assert!(!run_alive_via(unrunnable_ps, &dir, ""));
        assert!(!run_alive_via(unrunnable_ps, &dir, "0"));
        // pid 1 always exists: 0 as root, EPERM otherwise.
        assert!(run_alive_via(unrunnable_ps, &dir, "1"));
    }
}
