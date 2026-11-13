// Host processes inherit the host environment and the recorded run environment.
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod awake;
pub mod exec;
pub mod legacy;
pub mod log;
pub mod platform;
mod rental;
pub mod server;
pub mod wrapper;

use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

/// fcntl ownership is per process, so run mutations also need thread exclusion.
pub(crate) fn run_mutex(dir: &Path) -> io::Result<Arc<Mutex<()>>> {
    static RUNS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let dir = fs::canonicalize(dir)?;
    let mut runs = RUNS
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| io::Error::other("run lock registry poisoned"))?;
    runs.retain(|_, mutex| mutex.strong_count() > 0);
    let entry = runs.entry(dir).or_default();
    if let Some(mutex) = entry.upgrade() {
        return Ok(mutex);
    }
    let mutex = Arc::new(Mutex::new(()));
    *entry = Arc::downgrade(&mutex);
    Ok(mutex)
}

pub fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn nonce() -> io::Result<String> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(sha(&bytes))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    serde_json::from_slice(&fs::read(path)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Rename publishes only complete, synced receipts, including on shared filesystems.
pub fn publish(path: &Path, value: &impl Serialize) -> io::Result<()> {
    publish_bytes(path, &serde_json::to_vec(value)?)
}

pub fn publish_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", nonce()?));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(
            path.parent()
                .ok_or_else(|| io::Error::other("missing parent"))?,
        )?
        .sync_all()
    })();
    let _ = fs::remove_file(temporary);
    result
}

pub fn lock(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    let mut lock = unsafe { std::mem::zeroed::<libc::flock>() };
    lock.l_type = libc::F_WRLCK as _;
    lock.l_whence = libc::SEEK_SET as _;
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &lock) } == -1 {
        let error = io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EAGAIN)) {
            return Ok(None);
        }
        return Err(error);
    }
    Ok(Some(file))
}

pub fn run_dir(root: &Path, run: &str) -> io::Result<PathBuf> {
    if run.is_empty()
        || run.len() > 200
        || !run
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid run id",
        ));
    }
    Ok(root.join("runs").join(run))
}

pub fn crash_point(name: &str) {
    #[cfg(all(feature = "test-hooks", debug_assertions))]
    if std::env::var("ORX_TEST_CRASH_AT").as_deref() == Ok(name) {
        std::process::abort();
    }
    let _ = name;
}

/// Holds a request back, to show what a slow host does to the requests behind it.
pub fn slow_request() {
    #[cfg(all(feature = "test-hooks", debug_assertions))]
    if let Some(millis) = std::env::var("ORX_TEST_SLOW_RPC")
        .ok()
        .and_then(|millis| millis.parse().ok())
    {
        std::thread::sleep(std::time::Duration::from_millis(millis));
    }
}

/// Whether to pin sources as if they were on another filesystem, one step per request.
pub(crate) fn cross_device() -> bool {
    cfg!(all(feature = "test-hooks", debug_assertions))
        && std::env::var("ORX_TEST_CROSS_DEVICE").as_deref() == Ok("1")
}

pub(crate) const TERM_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) const HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn stop_duration(production: std::time::Duration) -> std::time::Duration {
    #[cfg(all(feature = "test-hooks", debug_assertions))]
    if std::env::var("ORX_TEST_FAST_STOPS").as_deref() == Ok("1") {
        return production.min(std::time::Duration::from_millis(100));
    }
    production
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn production_stop_durations_are_not_shortened_by_default() {
        assert_eq!(stop_duration(TERM_GRACE), Duration::from_secs(10));
        assert_eq!(stop_duration(HOOK_TIMEOUT), Duration::from_secs(30));
    }
}
