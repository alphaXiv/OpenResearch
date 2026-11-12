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
    /// Names a user-owned interpreter that replaces the managed env.
    pub override_var: Option<&'static str>,
}

impl ManagedEnv {
    pub fn dir(&self) -> PathBuf {
        crate::config::config_dir().join("envs").join(self.name)
    }

    pub fn python(&self) -> PathBuf {
        python_in(&self.dir())
    }

    /// The user's interpreter: `orx up`'s own environment, then Settings → Environment.
    pub fn override_python(&self) -> Option<String> {
        let var = self.override_var?;
        std::env::var(var)
            .ok()
            .filter(|python| !python.trim().is_empty())
            .or_else(|| crate::config::synced_env_var(var))
    }

    /// The interpreter a launcher runs with, resolved without side effects.
    pub fn interpreter(&self) -> String {
        if let Some(python) = self.override_python() {
            return python;
        }
        let managed = self.python();
        if managed.exists() {
            return managed.to_string_lossy().into_owned();
        }
        "python3".to_string()
    }

    /// A caller whose shell sets the override expects it used, but `orx up` resolves its own.
    pub fn require_caller_override(
        &self,
        caller_overrides: &std::collections::BTreeMap<String, String>,
    ) -> Result<()> {
        let Some(var) = self.override_var else {
            return Ok(());
        };
        let Some(caller) = caller_overrides.get(var) else {
            return Ok(());
        };
        let caller = caller.trim();
        let own = self.override_python();
        if caller.is_empty() || own.as_deref().map(str::trim) == Some(caller) {
            return Ok(());
        }
        let own = match own {
            Some(python) => format!("{var} set to {python}"),
            None => format!("its managed environment at {}", self.dir().display()),
        };
        Err(anyhow!(
            "This shell sets {var} to {caller}, but orx up runs {} with {own}. Add {var} in \
             Settings → Environment so every {} call uses it, or restart orx up from this shell.",
            self.label,
            self.label
        ))
    }

    async fn ready(&self, python: &Path) -> bool {
        crate::process::tokio_command(python)
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
        self.ensure_in(self.dir()).await
    }

    async fn ensure_in(&self, env_dir: PathBuf) -> Result<PathBuf> {
        // One lock for every env: installs are rare, so serializing them is cheaper than keying.
        static INSTALL_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        let _install = INSTALL_LOCK
            .get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await;
        let python = python_in(&env_dir);
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
        let via_uv = !self.create_venv(base, &env_dir).await?;
        eprintln!(
            "orx: installing {} for {} (one time)…",
            self.requirement, self.label
        );
        let status = if via_uv {
            crate::process::tokio_command("uv")
                .args(["pip", "install", "--quiet", "--python"])
                .arg(&python)
                .arg(self.requirement)
                .status()
                .await?
        } else {
            crate::process::tokio_command(&python)
                .args([
                    "-m",
                    "pip",
                    "install",
                    "--quiet",
                    "--disable-pip-version-check",
                    self.requirement,
                ])
                .status()
                .await?
        };
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

    /// Builds the venv with `base`, falling back to uv when `base` lacks ensurepip (Debian
    /// without `python3-venv`). Returns whether the venv has pip.
    async fn create_venv(&self, base: &str, env_dir: &Path) -> Result<bool> {
        let venv = crate::process::tokio_command(base)
            .args(["-m", "venv"])
            .arg(env_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;
        if venv.status.success() {
            return Ok(true);
        }
        let venv_error = first_line(&venv.stderr);
        if env_dir.exists() {
            std::fs::remove_dir_all(env_dir)?;
        }
        let uv = crate::process::tokio_command("uv")
            .args(["venv", "--quiet", "--python", base])
            .arg(env_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await;
        match uv {
            Ok(uv) if uv.status.success() => Ok(false),
            Ok(uv) => Err(anyhow!(
                "Could not create the {} environment at {} with `{base} -m venv` ({venv_error}) \
                 or with uv ({}).",
                self.label,
                env_dir.display(),
                first_line(&uv.stderr)
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(anyhow!(
                "Could not create the {} environment at {}: `{base} -m venv` failed \
                 ({venv_error}). Install uv (https://docs.astral.sh/uv/) or your Python's venv \
                 package, such as `sudo apt install python3-venv`, and retry.",
                self.label,
                env_dir.display()
            )),
            Err(error) => Err(anyhow!("Could not run uv: {error}")),
        }
    }
}

fn first_line(output: &[u8]) -> String {
    let output = String::from_utf8_lossy(output);
    output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("no output")
        .to_string()
}

fn python_in(env_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        env_dir.join("Scripts").join("python.exe")
    } else {
        env_dir.join("bin").join("python")
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
        let Some(version) = crate::process::tokio_command(candidate)
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
async fn lock_env(env_dir: &Path) -> Option<crate::file_lock::FileLock> {
    let lock_path = env_dir.with_extension("lock");
    std::fs::create_dir_all(lock_path.parent()?).ok()?;
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .ok()?;
    tokio::task::spawn_blocking(move || {
        lock_file
            .lock()
            .map(|()| crate::file_lock::FileLock::new(lock_file))
            .ok()
    })
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

    fn test_env(ready_check: &'static str, min_python: (u32, u32)) -> ManagedEnv {
        ManagedEnv {
            name: "test",
            label: "Test",
            min_python,
            requirement: "pip",
            ready_check,
            override_var: None,
        }
    }

    /// A throwaway env whose interpreter is the host's Python, plus a marker that a rebuild would delete.
    #[cfg(not(windows))]
    async fn fake_env() -> Option<PathBuf> {
        let host = base_python((3, 0), "Test").await.ok()?;
        let host = crate::process::command(host)
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .ok()?;
        let host = String::from_utf8(host.stdout).ok()?.trim().to_owned();
        let dir = std::env::temp_dir().join(format!("orx-managed-env-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::os::unix::fs::symlink(host, python_in(&dir)).unwrap();
        std::fs::write(dir.join("marker"), "").unwrap();
        Some(dir)
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn ensure_reuses_a_ready_env() {
        let Some(dir) = fake_env().await else {
            crate::config::skip_without_tool("no Python on PATH");
            return;
        };
        let python = test_env("pass", (3, 0))
            .ensure_in(dir.clone())
            .await
            .unwrap();
        assert_eq!(python, python_in(&dir));
        assert!(dir.join("marker").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn ensure_keeps_a_stale_env_when_no_python_is_new_enough() {
        let Some(dir) = fake_env().await else {
            crate::config::skip_without_tool("no Python on PATH");
            return;
        };
        let error = test_env("raise SystemExit(1)", (99, 0))
            .ensure_in(dir.clone())
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Test needs Python 99.0 or newer"),
            "{error}"
        );
        assert!(dir.join("marker").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A `python3` whose venv module lacks ensurepip, a `uv` that logs its calls, and
    /// nothing else on PATH. Returns the bin dir.
    #[cfg(not(windows))]
    async fn without_ensurepip(with_uv: bool) -> Option<PathBuf> {
        let host = base_python((3, 0), "Test").await.ok()?;
        let host = crate::process::command(host)
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .ok()?;
        let host = String::from_utf8(host.stdout).ok()?.trim().to_owned();
        let bin = std::env::temp_dir().join(format!("orx-no-ensurepip-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&bin).unwrap();
        let log = bin.join("uv.log");
        let mut scripts = vec![(
            "python3",
            "#!/bin/sh\nif [ \"$1\" = -m ]; then /bin/mkdir -p \"$3/bin\"; \
             echo 'Error: ensurepip is not available' >&2; exit 1; fi\necho 3.12\n"
                .to_string(),
        )];
        if with_uv {
            scripts.push((
                "uv",
                format!(
                    "#!/bin/sh\necho \"$@\" >> {log}\nif [ \"$1\" = venv ]; then \
                     /bin/mkdir -p \"$5/bin\" && /bin/ln -s {host} \"$5/bin/python\"; fi\n",
                    log = log.display()
                ),
            ));
        }
        for (name, body) in scripts {
            orx_test_support::write_executable(bin.join(name), body, 0o755).unwrap();
        }
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
        Some(bin)
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn ensure_falls_back_to_uv_without_ensurepip() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let Some(bin) = without_ensurepip(true).await else {
            crate::config::skip_without_tool("no Python on PATH");
            return;
        };
        let env_dir = bin.join("env");
        let ready = "import os; assert 'pip install' in open(os.environ['ORX_TEST_UV_LOG']).read()";
        std::env::set_var("ORX_TEST_UV_LOG", bin.join("uv.log"));
        let python = test_env(ready, (3, 10))
            .ensure_in(env_dir.clone())
            .await
            .unwrap();
        assert_eq!(python, python_in(&env_dir));
        let log = std::fs::read_to_string(bin.join("uv.log")).unwrap();
        assert!(log.contains("venv --quiet --python python3"), "{log}");
        assert!(
            log.contains(&format!(
                "pip install --quiet --python {} pip",
                python.display()
            )),
            "{log}"
        );
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn ensure_without_ensurepip_or_uv_names_both_fixes() {
        if !crate::config::in_test_child(&[]) {
            return;
        }
        let Some(bin) = without_ensurepip(false).await else {
            crate::config::skip_without_tool("no Python on PATH");
            return;
        };
        let error = test_env("raise SystemExit(1)", (3, 10))
            .ensure_in(bin.join("env"))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("ensurepip is not available"), "{error}");
        assert!(error.contains("Install uv"), "{error}");
        assert!(error.contains("python3-venv"), "{error}");
    }

    #[test]
    fn a_caller_override_must_be_the_servers() {
        if !crate::config::in_test_child(&["ORX_TEST_PYTHON"]) {
            return;
        }
        let env = ManagedEnv {
            override_var: Some("ORX_TEST_PYTHON"),
            ..test_env("pass", (3, 10))
        };
        let caller = |python: &str| {
            std::collections::BTreeMap::from([("ORX_TEST_PYTHON".to_string(), python.to_string())])
        };
        assert!(env.require_caller_override(&Default::default()).is_ok());

        let error = env
            .require_caller_override(&caller("/opt/py312/bin/python"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("/opt/py312/bin/python"), "{error}");
        assert!(error.contains(&env.dir().display().to_string()), "{error}");

        let synced = crate::config::test_home().join(".openresearch");
        std::fs::create_dir_all(&synced).unwrap();
        std::fs::write(
            synced.join("env"),
            "export ORX_TEST_PYTHON='/opt/py312/bin/python'\n",
        )
        .unwrap();
        assert_eq!(env.interpreter(), "/opt/py312/bin/python");
        assert!(env
            .require_caller_override(&caller("/opt/py312/bin/python"))
            .is_ok());

        std::env::set_var("ORX_TEST_PYTHON", "/usr/bin/python3.11");
        assert_eq!(env.interpreter(), "/usr/bin/python3.11");
        let error = env
            .require_caller_override(&caller("/opt/py312/bin/python"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("/usr/bin/python3.11"), "{error}");
    }
}
