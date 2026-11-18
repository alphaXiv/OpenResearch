//! The only place child processes are built; `clippy.toml` bans building them elsewhere.
//!
//! A Finder-launched app inherits launchd's PATH and none of the user's exports,
//! so a child left to the process environment misses Homebrew tools and agents
//! a terminal finds, and reads a different store than the app.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use crate::local::shell_env;

/// `program` as the user's terminal would start it: a bare name found where
/// detection looks ([`shell_env::find`]), and the child given the shell's PATH
/// and the directory overrides app mode imported. Under test every home the
/// child could resolve is inside the temp root unless the caller sets it.
#[allow(clippy::disallowed_methods)]
pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(resolve(program.as_ref()));
    if let Some(path) = shell_env::search_path() {
        command.env("PATH", path);
    }
    shell_env::export_to(|key, value| {
        command.env(key, value);
    });
    // AppRun's loader path belongs to its webview, not host executables.
    if let Some((var, value)) = shell_env::host_gui_env()
        .into_iter()
        .find(|(var, _)| *var == "LD_LIBRARY_PATH")
    {
        match value {
            Some(value) => command.env(var, value),
            None => command.env_remove(var),
        };
    }
    #[cfg(test)]
    {
        command.env(crate::config::PARENT_TEST_ROOT, crate::config::test_root());
        for (var, path) in crate::config::home_vars(&crate::config::test_home()) {
            if crate::config::env_path(var).is_none() {
                command.env(var, path);
            }
        }
        for var in crate::config::REDIRECT_VARS {
            if crate::config::env_path(var).is_none() {
                command.env_remove(var);
            }
        }
        if std::env::var_os(crate::config::TEST_CHILD).is_none() {
            for var in crate::config::PROVIDER_VARS {
                command.env_remove(var);
            }
        }
    }
    command
}

/// [`command`], for tokio.
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    command(program).into()
}

/// How long an agent CLI gets, at each step of ending it, to exit on its own.
const AGENT_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// Waits for an agent CLI whose input the caller closed to exit on its own, then
/// [`ask_agent_to_stop`]s it. The caller kills what is left.
///
/// Killed outright, an agent can die between rotating its sign-in token and saving the
/// new one, and the token left on disk no longer works: the user is signed out.
pub(crate) async fn let_agent_exit(child: &mut tokio::process::Child, group: Option<u32>) {
    if !exits_within_grace(child).await {
        ask_agent_to_stop(child, group).await;
    }
}

/// Sends SIGTERM to an agent CLI's process group (or the child alone, without one) and
/// waits a moment for it to exit. Windows has no such request, so there it does nothing.
pub(crate) async fn ask_agent_to_stop(child: &mut tokio::process::Child, group: Option<u32>) {
    #[cfg(unix)]
    {
        let target = match group {
            Some(group) => i32::try_from(group).ok().map(|group| -group),
            None => child.id().and_then(|pid| i32::try_from(pid).ok()),
        };
        if let Some(target) = target {
            // SAFETY: a negative target names a process group the child leads; a positive one, the child itself.
            unsafe {
                libc::kill(target, libc::SIGTERM);
            }
        }
        exits_within_grace(child).await;
    }
    #[cfg(not(unix))]
    let _ = (child, group);
}

/// [`let_agent_exit`] for a throwaway agent CLI the caller has its answer from and whose
/// input it closed, reading what it still prints so a closed pipe never cuts its exit
/// short. Dropping `child` kills whatever is left.
pub(crate) async fn let_probe_exit(
    mut child: tokio::process::Child,
    mut output: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) {
    let drain = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut output, &mut tokio::io::sink()).await;
    });
    let_agent_exit(&mut child, None).await;
    drain.abort();
}

/// What an agent CLI printed and how it exited, if it finished by `deadline`; `None` when
/// it did not, after [`ask_agent_to_stop`]ping it, since a CLI this slow may be saving a
/// refreshed sign-in. Its output must be piped. Detached, so a caller that gives up never
/// drops, and so kills, the child.
pub(crate) async fn agent_output_by(
    child: tokio::process::Child,
    deadline: tokio::time::Instant,
) -> Option<std::io::Result<std::process::Output>> {
    tokio::spawn(output_by(child, deadline))
        .await
        .unwrap_or_else(|error| Some(Err(std::io::Error::other(error))))
}

async fn output_by(
    mut child: tokio::process::Child,
    deadline: tokio::time::Instant,
) -> Option<std::io::Result<std::process::Output>> {
    async fn read_all(pipe: Option<impl tokio::io::AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            tokio::io::AsyncReadExt::read_to_end(&mut pipe, &mut bytes).await?;
        }
        Ok(bytes)
    }

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let finished = tokio::time::timeout_at(deadline, async {
        let (status, out, err) = tokio::join!(
            child.wait(),
            read_all(stdout.as_mut()),
            read_all(stderr.as_mut())
        );
        Ok(std::process::Output {
            status: status?,
            stdout: out?,
            stderr: err?,
        })
    })
    .await;
    if finished.is_err() {
        ask_agent_to_stop(&mut child, None).await;
    }
    finished.ok()
}

async fn exits_within_grace(child: &mut tokio::process::Child) -> bool {
    let grace = crate::config::paced(AGENT_EXIT_GRACE);
    tokio::time::timeout(grace, child.wait())
        .await
        .is_ok_and(|status| status.is_ok())
}

/// Waits for a child, killing and reaping it when the deadline expires.
#[cfg(any(not(unix), test))]
pub(crate) fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let started = std::time::Instant::now();
    let error = loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Err(error) => break error,
            Ok(None) => {}
        }

        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("Process {} did not exit within {timeout:?}", child.id()),
            );
        }
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(10)));
    };

    if let Err(kill_error) = child.kill() {
        if child.try_wait()?.is_none() {
            return Err(kill_error);
        }
    }
    child.wait()?;
    Err(error)
}

/// A bare name found on the search PATH stays bare on Unix, so a caller that
/// gives the child its own PATH still decides what runs. Windows spawning
/// tries only `.exe`, so there every hit is spelled out to reach `.cmd` shims.
fn resolve(program: &OsStr) -> OsString {
    let bare = Path::new(program)
        .parent()
        .is_some_and(|dir| dir.as_os_str().is_empty());
    let Some(name) = program.to_str().filter(|_| bare) else {
        return program.to_owned();
    };
    if cfg!(unix) && shell_env::find_on_path(name).is_some() {
        return program.to_owned();
    }
    shell_env::find(name).map_or_else(|| program.to_owned(), PathBuf::into_os_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_wait_keeps_exit_codes_and_reaps_a_stalled_child() {
        use std::io::{BufRead, Write};
        use std::process::Stdio;
        use std::time::Duration;

        for exit_code in [Some(7), None] {
            let python = crate::local::acp::fake_agent_interpreter("python3");
            let mut child = command(python)
                .args([
                    "-u",
                    "-c",
                    "import sys; print('ready'); raise SystemExit(int(sys.stdin.readline()))",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut input = child.stdin.take().unwrap();
            let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
            let mut ready = String::new();
            output.read_line(&mut ready).unwrap();
            assert_eq!(ready.trim(), "ready");

            if let Some(code) = exit_code {
                writeln!(input, "{code}").unwrap();
            }
            let timeout = if exit_code.is_some() {
                Duration::from_secs(5)
            } else {
                Duration::from_millis(50)
            };
            let result = wait_with_timeout(&mut child, timeout);
            match exit_code {
                Some(code) => assert_eq!(result.unwrap().code(), Some(code)),
                None => assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::TimedOut),
            }
            assert!(child.try_wait().unwrap().is_some());
            drop(input);
        }
    }

    #[test]
    fn a_child_gets_the_search_path_and_keeps_explicit_programs() {
        let child = command("orx-surely-not-installed");
        let path = child
            .get_envs()
            .find(|(key, _)| *key == "PATH")
            .and_then(|(_, value)| value);
        assert_eq!(path, shell_env::search_path().as_deref());
        assert_eq!(child.get_program(), "orx-surely-not-installed");

        let exe = std::env::current_exe().unwrap();
        assert_eq!(command(&exe).get_program(), exe.as_os_str());
    }

    #[test]
    fn a_child_of_the_test_process_never_sees_the_developers_keys() {
        let child = command("orx-surely-not-installed");
        let removed: Vec<_> = child
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_owned())
            .collect();
        for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"] {
            assert!(
                removed.iter().any(|key| key == var),
                "{var} reaches the child"
            );
        }
    }

    #[test]
    fn a_child_never_inherits_the_developers_homes() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        for var in ["HOME", "CODEX_HOME"] {
            std::env::set_var(var, "/orx-not-a-home");
        }
        let mut child = if cfg!(windows) {
            let mut child = command("cmd");
            child.args(["/C", "set"]);
            child
        } else {
            command("env")
        };
        let output = child
            .env("CLAUDE_CONFIG_DIR", "/orx-chosen")
            .output()
            .unwrap();
        assert!(output.status.success());

        let stdout = String::from_utf8(output.stdout).unwrap();
        let seen = |var: &str| {
            stdout
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{var}=")))
                .map(PathBuf::from)
        };
        let home = crate::config::test_home();
        assert_eq!(seen("HOME"), Some(home.clone()));
        assert_eq!(seen("CODEX_HOME"), Some(home.join(".codex")));
        assert_eq!(
            seen("CLAUDE_CONFIG_DIR"),
            Some(PathBuf::from("/orx-chosen"))
        );
    }

    /// The child's PATH, not this lookup, picks what runs, so a caller's own PATH still applies.
    #[cfg(unix)]
    #[test]
    fn a_name_on_path_stays_bare() {
        assert_eq!(command("sh").get_program(), "sh");
    }
}
