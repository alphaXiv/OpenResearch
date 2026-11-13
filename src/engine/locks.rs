use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;

use crate::error::Result;
use crate::file_lock::FileLock;

/// The per-run `.supervisor.lock` + `.supervisor.shared-ssh.lock` pairs the engine owns.
/// Holding both is what makes 0.2.x startup, `orx supervise` and `ssh-service` leave a run alone;
/// dropping the files releases them.
#[derive(Default)]
pub struct RunLocks(HashMap<String, [FileLock; 2]>);

impl RunLocks {
    pub fn holds(&self, run_id: &str) -> bool {
        self.0.contains_key(run_id)
    }

    /// False when another process (an old supervisor, or a momentary probe) holds either lock.
    pub fn acquire(&mut self, run_id: &str, paths: [PathBuf; 2]) -> Result<bool> {
        if self.holds(run_id) {
            return Ok(true);
        }
        let [supervisor, shared] = paths.map(|path| -> Result<File> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            Ok(OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(path)?)
        });
        let supervisor = supervisor?;
        let shared = shared?;
        let lock = |file: File| match file.try_lock() {
            Ok(()) => Ok(Some(FileLock::new(file))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(error),
        };
        // Supervisor first, as `orx supervise` takes them: whoever holds it owns the run.
        let Some(supervisor) = lock(supervisor)? else {
            return Ok(false);
        };
        let Some(shared) = lock(shared)? else {
            return Ok(false);
        };
        self.0.insert(run_id.to_string(), [supervisor, shared]);
        Ok(true)
    }

    pub fn release(&mut self, run_id: &str) {
        self.0.remove(run_id);
    }
}
