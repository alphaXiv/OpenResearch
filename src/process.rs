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
/// and the directory overrides app mode imported.
#[allow(clippy::disallowed_methods)]
pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(resolve(program.as_ref()));
    if let Some(path) = shell_env::search_path() {
        command.env("PATH", path);
    }
    shell_env::export_to(|key, value| {
        command.env(key, value);
    });
    command
}

/// [`command`], for tokio.
pub fn tokio_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    command(program).into()
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

    /// The child's PATH, not this lookup, picks what runs, so a caller's own PATH still applies.
    #[cfg(unix)]
    #[test]
    fn a_name_on_path_stays_bare() {
        assert_eq!(command("sh").get_program(), "sh");
    }
}
