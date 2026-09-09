//! Canonicalization that stays usable outside this process.
//!
//! Windows `canonicalize` answers with a verbatim `\\?\C:\…` path, which
//! `CreateProcessW` rejects as a working directory — so a session whose
//! checkout root came straight from it cannot run git at all. Codex receives
//! canonicalized paths as sandbox roots and the dashboard shows them to the
//! user, both of which want the plain spelling too.
//!
//! Every canonicalization in the crate goes through here, because containment
//! checks compare a canonicalized child against a canonicalized root and a mix
//! of the two spellings denies paths that are genuinely inside.

#[cfg(windows)]
use std::path::{Component, Prefix};
use std::path::{Path, PathBuf};

/// `std::fs::canonicalize`, minus the Windows verbatim prefix.
pub fn canonicalize<P: AsRef<Path>>(path: P) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path).map(plain)
}

#[cfg(not(windows))]
fn plain(path: PathBuf) -> PathBuf {
    path
}

/// Matched on the parsed prefix rather than the string: a name that is not
/// valid UTF-8 has no `to_str`, and skipping it there would leave that one path
/// verbatim while the root it is checked against is not.
#[cfg(windows)]
fn plain(path: PathBuf) -> PathBuf {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let head = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => std::ffi::OsString::from(format!("{}:\\", drive as char)),
        // `\\?\UNC\server\share` is an encoding of `\\server\share`.
        Prefix::VerbatimUNC(server, share) => {
            let mut head = std::ffi::OsString::from(r"\\");
            head.push(server);
            head.push(r"\");
            head.push(share);
            head.push(r"\");
            head
        }
        // A device path is itself, not an encoding of anything shorter.
        _ => return path,
    };
    let mut out = PathBuf::from(head);
    out.extend(components.filter(|part| !matches!(part, Component::RootDir)));
    out
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_verbatim_path_keeps_only_the_spelling_a_child_process_accepts() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\C:\Users\me")),
            PathBuf::from(r"C:\Users\me")
        );
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\server\share\dir")),
            PathBuf::from(r"\\server\share\dir")
        );
        // Not an encoding of a shorter path, so it stays as it is.
        let device = PathBuf::from(r"\\.\PIPE\orx");
        assert_eq!(plain(device.clone()), device);
        let plain_already = PathBuf::from(r"C:\Users\me");
        assert_eq!(plain(plain_already.clone()), plain_already);
    }

    #[test]
    fn a_canonicalized_directory_is_accepted_as_a_working_directory() {
        let dir = canonicalize(std::env::temp_dir()).expect("temp dir");
        let out = std::process::Command::new("cmd")
            .args(["/C", "cd"])
            .current_dir(&dir)
            .output()
            .expect("cmd");
        assert!(out.status.success());
        assert!(!String::from_utf8_lossy(&out.stdout).contains(r"\\?\"));
    }
}
