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
