use std::fs::File;

/// A locked file that unlocks when dropped. Closing alone releases the lock only once every
/// child forked meanwhile has exec'd, so until then another holder would find it taken.
pub struct FileLock(File);

impl FileLock {
    /// Takes `file` once it holds its lock.
    pub fn new(file: File) -> Self {
        Self(file)
    }
}

impl std::ops::Deref for FileLock {
    type Target = File;

    fn deref(&self) -> &File {
        &self.0
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Closing still releases it if this fails, only later.
        let _ = self.0.unlock();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_releases_the_lock_a_forked_child_still_shares() {
        let path = crate::config::test_root().join(format!("{}.lock", uuid::Uuid::new_v4()));
        let open = || {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)
                .unwrap()
        };
        let file = open();
        file.try_lock().unwrap();
        // What a fork leaves the child until it execs: another descriptor for the same open file.
        let child = file.try_clone().unwrap();

        drop(FileLock::new(file));

        open().try_lock().unwrap();
        drop(child);
    }
}
