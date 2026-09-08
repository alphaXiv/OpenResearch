//! Canonicalization that stays usable outside this process.
//!
//! Windows `std::fs::canonicalize` answers with a verbatim `\\?\C:\…` path,
//! and this codebase hands canonicalized paths to things that do not accept
//! one: git and the agent CLIs receive them as a working directory, Codex
//! receives them as sandbox writable roots, and the dashboard shows them to the
//! user. `CreateProcessW` rejects a verbatim `lpCurrentDirectory`, so a session
//! whose checkout root came straight from `canonicalize` cannot run git at all.
//!
//! Consistency matters as much as the form itself: containment checks compare a
//! canonicalized child against a canonicalized root, so a codebase that mixes
//! the two spellings denies access to paths that are genuinely inside. Every
//! canonicalization goes through here for that reason.

use std::path::{Path, PathBuf};

/// `std::fs::canonicalize`, minus the Windows verbatim prefix.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(plain)
}

#[cfg(not(windows))]
fn plain(path: PathBuf) -> PathBuf {
    path
}

/// Only a drive path has a plain spelling; UNC and device paths keep theirs,
/// where the prefix is the path rather than an encoding of it.
#[cfg(windows)]
fn plain(path: PathBuf) -> PathBuf {
    let Some(rest) = path.to_str().and_then(|path| path.strip_prefix(r"\\?\")) else {
        return path;
    };
    let mut chars = rest.chars();
    let drive = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.next() == Some(':')
        && chars.next() == Some('\\');
    if drive {
        PathBuf::from(rest)
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalizing_a_real_directory_keeps_it_usable_as_a_working_directory() {
        let dir = canonicalize(std::env::temp_dir()).expect("temp dir");
        assert!(
            std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
                .args(if cfg!(windows) {
                    vec!["/C", "cd"]
                } else {
                    vec![]
                })
                .current_dir(&dir)
                .output()
                .is_ok()
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_verbatim_drive_path_loses_its_prefix_but_a_unc_one_does_not() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        let unc = PathBuf::from(r"\\?\UNC\server\share");
        assert_eq!(plain(unc.clone()), unc);
    }
}
