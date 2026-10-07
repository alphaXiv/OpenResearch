//! orx-managed Python venvs for backends driven through a Python SDK (Hugging Face, Modal).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use crate::error::{anyhow, Result};

pub struct ManagedEnv {
    /// Directory name under `<config>/envs`.
    pub name: &'static str,
    /// Backend name used in progress and error messages.
    pub label: &'static str,
    pub min_python: (u32, u32),
    pub requirement: &'static str,
    /// Python snippet that exits 0 only when the installed SDK has what orx calls.
    pub ready_check: &'static str,
}

impl ManagedEnv {
    pub fn dir(&self) -> PathBuf {
        crate::config::config_dir().join("envs").join(self.name)
    }

    pub fn python(&self) -> PathBuf {
        if cfg!(windows) {
            self.dir().join("Scripts").join("python.exe")
        } else {
            self.dir().join("bin").join("python")
        }
    }

    async fn ready(&self, python: &Path) -> bool {
        tokio::process::Command::new(python)
            .args(["-c", self.ready_check])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map(|status| status.success())
            .unwrap_or(false)
    }

    /// Returns the env's interpreter, (re)building the env when it is missing or stale.
    pub async fn ensure(&self) -> Result<PathBuf> {
        // One lock for every env: installs are rare, so serializing them is cheaper than keying.
        static INSTALL_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        let _install = INSTALL_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let python = self.python();
        let env_dir = self.dir();
        let _env_lock = lock_env(&env_dir).await;
        if python.exists() && self.ready(&python).await {
            return Ok(python);
        }
        let base = base_python(self.min_python, self.label).await?;
        // An unusable env may be pinned to an interpreter too old to upgrade, so rebuild it.
        if env_dir.exists() {
            std::fs::remove_dir_all(&env_dir)?;
        }
        if let Some(parent) = env_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let status = tokio::process::Command::new(base)
            .args(["-m", "venv"])
            .arg(&env_dir)
            .status()
            .await?;
        if !status.success() {
            return Err(anyhow!(
                "Could not create the {} environment at {}.",
                self.label,
                env_dir.display()
            ));
        }
        eprintln!(
            "orx: installing {} for {} (one time)…",
            self.requirement, self.label
        );
        let status = tokio::process::Command::new(&python)
            .args([
                "-m",
                "pip",
                "install",
                "--quiet",
                "--disable-pip-version-check",
                self.requirement,
            ])
            .status()
            .await?;
        if !status.success() || !self.ready(&python).await {
            return Err(anyhow!(
                "Could not install {} into the {} environment at {}.",
                self.requirement,
                self.label,
                env_dir.display()
            ));
        }
        Ok(python)
    }
}

fn parse_python_version(text: &str) -> Option<(u32, u32)> {
    let (major, minor) = text.trim().split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Versioned names catch a newer Python hidden behind an old `python3` (Xcode CLT ships 3.9).
pub async fn base_python(min_python: (u32, u32), label: &str) -> Result<&'static str> {
    let mut too_old = None;
    for candidate in [
        "python3",
        "python",
        "python3.14",
        "python3.13",
        "python3.12",
        "python3.11",
        "python3.10",
    ] {
        let Some(version) = tokio::process::Command::new(candidate)
            .args([
                "-c",
                "import sys, venv; print('%d.%d' % sys.version_info[:2])",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .await
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| parse_python_version(&String::from_utf8_lossy(&output.stdout)))
        else {
            continue;
        };
        if version >= min_python {
            return Ok(candidate);
        }
        too_old.get_or_insert((candidate, version));
    }
    let (min_major, min_minor) = min_python;
    Err(match too_old {
        Some((candidate, (major, minor))) => anyhow!(
            "{label} needs Python {min_major}.{min_minor} or newer, but `{candidate}` is Python \
             {major}.{minor}. Install a newer Python and retry."
        ),
        None => anyhow!("{label} needs Python {min_major}.{min_minor} or newer."),
    })
}

/// Serializes installs across orx processes so one can't delete another's in-progress env.
/// Best-effort, like the settings lock: a filesystem without locks shouldn't block launches.
async fn lock_env(env_dir: &Path) -> Option<std::fs::File> {
    let lock_path = env_dir.with_extension("lock");
    std::fs::create_dir_all(lock_path.parent()?).ok()?;
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok()?;
    tokio::task::spawn_blocking(move || lock_file.lock().map(|()| lock_file).ok())
        .await
        .ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_python_versions() {
        assert_eq!(parse_python_version("3.9\n"), Some((3, 9)));
        assert_eq!(parse_python_version("3.14"), Some((3, 14)));
        assert_eq!(parse_python_version("garbage"), None);
    }
}
