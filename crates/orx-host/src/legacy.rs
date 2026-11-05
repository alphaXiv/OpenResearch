use crate::{exec, platform};
use orx_host_proto::{ContainerRun, LegacyCancelOutcome, LegacyInspection};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub fn directory(value: &str) -> io::Result<PathBuf> {
    let relative = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("$HOME/"));
    let components = relative.or_else(|| value.strip_prefix('/'));
    let valid = components.is_some_and(|value| {
        !value.is_empty()
            && value.split('/').all(|part| {
                !matches!(part, "" | "." | "..")
                    && part
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-+@".contains(c))
            })
    });
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory must be absolute or HOME-relative without shell metacharacters",
        ));
    }
    if let Some(relative) = relative {
        let home = std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is unset"))?;
        if !Path::new(&home).is_absolute() {
            return Err(io::Error::other("HOME must be absolute"));
        }
        return Ok(PathBuf::from(home).join(relative));
    }
    Ok(PathBuf::from(value))
}

pub fn inspect(dir: &Path) -> io::Result<LegacyInspection> {
    let pid = match fs::read_to_string(dir.join("pid")) {
        Ok(text) => text
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|&pid| (2..=i32::MAX as u32).contains(&pid)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let (exit_code, receipt_unreadable) = match fs::read_to_string(dir.join("exit_code")) {
        Ok(text) => match text.trim().parse() {
            Ok(code) => (Some(code), false),
            Err(_) => (None, true),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => (None, false),
        Err(_) => (None, true),
    };
    let alive = if let Some(pid) = pid {
        platform::process(pid).is_ok_and(|process| process.live()) || !group(pid)?.is_empty()
    } else {
        false
    };
    Ok(LegacyInspection {
        pid,
        alive,
        exit_code,
        receipt_unreadable,
    })
}

pub fn cancel(
    dir: &Path,
    container: Option<&ContainerRun>,
    stop: Option<&AtomicBool>,
) -> io::Result<LegacyCancelOutcome> {
    if let Some(container) = container {
        cancel_container(container, stop)?;
    }
    let Some(pid) = inspect(dir)?.pid else {
        return Ok(LegacyCancelOutcome::GroupGone);
    };
    let original = platform::process(pid).ok();
    let send = |signal| -> io::Result<()> {
        if let Some(original) = &original {
            if platform::process(pid).is_ok_and(|process| process.starttime != original.starttime) {
                return Err(io::Error::other("legacy process identity changed"));
            }
        }
        if unsafe { libc::kill(-(pid as i32), signal) } != 0 && signal == libc::SIGTERM {
            if let Some(original) = &original {
                let _ = platform::signal(original, signal);
            }
        }
        Ok(())
    };
    cancel_group(
        pid,
        send,
        stop,
        crate::stop_duration(Duration::from_secs(5)),
        crate::stop_duration(Duration::from_secs(1)),
    )
}

fn cancel_group(
    pid: u32,
    mut send: impl FnMut(i32) -> io::Result<()>,
    stop: Option<&AtomicBool>,
    grace: Duration,
    recheck: Duration,
) -> io::Result<LegacyCancelOutcome> {
    send(libc::SIGTERM)?;
    let deadline = Instant::now() + grace;
    loop {
        let pids = group(pid)?;
        if pids.is_empty() {
            return Ok(LegacyCancelOutcome::GroupGone);
        }
        if stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    send(libc::SIGKILL)?;
    let deadline = Instant::now() + recheck;
    loop {
        let pids = group(pid)?;
        if pids.is_empty() {
            return Ok(LegacyCancelOutcome::GroupGone);
        }
        if stop.is_some_and(|stop| stop.load(Ordering::Relaxed)) || Instant::now() >= deadline {
            return Ok(LegacyCancelOutcome::Survivors { pids });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn group(pgid: u32) -> io::Result<Vec<u32>> {
    let mut result = Vec::new();
    for pid in platform::pids()? {
        if let Ok(process) = platform::process(pid) {
            if process.live() && unsafe { libc::getpgid(pid as i32) } == pgid as i32 {
                result.push(pid);
            }
        }
    }
    Ok(result)
}

fn cancel_container(container: &ContainerRun, stop: Option<&AtomicBool>) -> io::Result<()> {
    if container.id.is_empty()
        || !container
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid container id",
        ));
    }
    if !container.run_dir.starts_with('/') || container.run_dir.contains(['\n', '\r', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "container run directory must be absolute",
        ));
    }
    let result = exec::run(
        Command::new("docker").args([
            "container",
            "inspect",
            "--format",
            "{{.Id}}|{{.State.StartedAt}}|{{.State.Status}}",
            "--",
            &container.id,
        ]),
        &[],
        Duration::from_secs(5),
        stop,
    )?;
    if result.timed_out || result.stdout_truncated {
        return Err(io::Error::other(
            "container inspection timed out or exceeded its output bound",
        ));
    }
    if result.code != Some(0) {
        let list = exec::run(
            Command::new("docker").args([
                "container",
                "ls",
                "--all",
                "--no-trunc",
                "--format",
                "{{.ID}} {{.Names}}",
            ]),
            &[],
            Duration::from_secs(5),
            stop,
        )?;
        if list.code != Some(0) || list.timed_out || list.stdout_truncated {
            return Err(io::Error::other("cannot inspect Docker daemon"));
        }
        if String::from_utf8_lossy(&list.stdout)
            .split_whitespace()
            .any(|value| value == container.id || value.starts_with(&container.id))
        {
            return Err(io::Error::other("cannot inspect container"));
        }
        return Ok(());
    }
    let output = String::from_utf8_lossy(&result.stdout);
    let fields: Vec<_> = output.trim().split('|').collect();
    if fields.len() != 3 {
        return Err(io::Error::other("invalid Docker inspection"));
    }
    if fields[0] != container.id || fields[1] != container.started_at {
        return Ok(());
    }
    if fields[2] == "paused" {
        return Err(io::Error::other(format!(
            "Container '{}' is paused. Unpause it to complete cancellation.",
            container.reference
        )));
    }
    if fields[2] != "running" {
        return Ok(());
    }
    let directory = format!("'{}'", container.run_dir.replace('\'', "'\\''"));
    let script = format!(
        r#"
start_time() {{ local stat; IFS= read -r stat < "/proc/$1/stat" 2>/dev/null || return 1; stat=${{stat##*) }}; local fields; read -ra fields <<< "$stat"; printf '%s' "${{fields[19]}}"; }}
group_alive() {{ local file stat fields; for file in /proc/[0-9]*/stat; do IFS= read -r stat < "$file" 2>/dev/null || continue; stat=${{stat##*) }}; read -ra fields <<< "$stat"; if [ "${{fields[2]}}" = "$p" ] && [ "${{fields[0]}}" != Z ] && [ "${{fields[0]}}" != X ]; then return 0; fi; done; return 1; }}
cd {directory} || exit 1
umask 077
: > cancel
[ -f identity ] || exit 0
read -r p started < identity
case "$p" in ''|*[!0-9]*) exit 1;; esac
[ "$p" -gt 1 ] || exit 1
current=$(start_time "$p")
if [ -n "$current" ] && [ "$current" != "$started" ]; then echo 'Process identity changed; refusing to signal it' >&2; exit 1; fi
if ! group_alive; then exit 0; fi
kill -TERM -- -"$p" 2>/dev/null || true
for ((i=0; i<50; i++)); do group_alive || exit 0; sleep 0.1; done
kill -KILL -- -"$p" 2>/dev/null || true
for ((i=0; i<10; i++)); do group_alive || exit 0; sleep 0.1; done
echo 'Experiment process group is still alive' >&2
exit 1
"#
    );
    let result = exec::run(
        Command::new("docker").args(["exec", "-i", &container.id, "bash", "-c", &script]),
        &[],
        Duration::from_secs(8),
        stop,
    )?;
    if result.code == Some(0) && !result.timed_out {
        Ok(())
    } else {
        let reason = if result.timed_out {
            "Container cancellation timed out".to_owned()
        } else {
            String::from_utf8_lossy(&result.stderr).trim().to_owned()
        };
        Err(io::Error::other(reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    #[test]
    fn surviving_fixture_group_is_reported_when_signals_cannot_remove_it() {
        let mut child = Command::new("sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let result = cancel_group(
            pid,
            |_| Ok(()),
            None,
            Duration::from_millis(20),
            Duration::from_millis(20),
        );
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
        child.wait().unwrap();
        assert!(
            matches!(result.unwrap(), LegacyCancelOutcome::Survivors { pids } if pids.contains(&pid))
        );
    }
}
