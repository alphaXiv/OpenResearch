use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::Path;

use super::{phase_of, Engine};
use crate::error::Result;
use crate::jobs::BackendDescriptor;

impl Engine {
    /// Refresh sharing only after this store's legacy supervisors release ownership.
    pub fn retire_ssh_config(&self, home: &Path) -> Result<bool> {
        let store = self.store();
        let transaction = store.begin_immediate()?;
        // LIMIT -1 also includes terminal rows whose legacy supervisor may still be exiting.
        for run in store.list_runs(usize::MAX)? {
            if self.memory().locks.holds(&run.id) {
                continue;
            }
            let descriptor = BackendDescriptor::parse(&run.backend_json);
            if descriptor.as_ref().is_ok_and(|descriptor| {
                !matches!(
                    descriptor.kind.as_str(),
                    "ssh_job" | "slurm_job" | "openresearch_job"
                )
            }) {
                continue;
            }
            if descriptor.is_ok() && phase_of(&run).is_ok_and(|phase| !phase.is_terminal()) {
                return Ok(false);
            }
            for path in store.supervisor_lock_paths(&run.id)? {
                let file = match OpenOptions::new().write(true).open(path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => return Err(error.into()),
                };
                match file.try_lock() {
                    Ok(()) => drop(crate::file_lock::FileLock::new(file)),
                    Err(std::fs::TryLockError::WouldBlock) => return Ok(false),
                    Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
                }
            }
        }

        #[cfg(unix)]
        crate::jobs::ssh::sharing::retire_legacy(home)?;
        #[cfg(not(unix))]
        let _ = home;
        transaction.commit()?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::clock::TestClock;
    use crate::store::{Store, StoredRun};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    const MANAGED_FILES: [&str; 3] = [
        "openresearch_config",
        "openresearch_match",
        "openresearch_connections.json",
    ];

    const FOOTER: &[u8] = b"\nHost *\nInclude openresearch_config\n";

    struct Fixture {
        home: PathBuf,
        engine: Engine,
    }

    impl Fixture {
        fn new(config: &[u8]) -> Self {
            let home =
                std::env::temp_dir().join(format!("orx-ssh-retirement-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(home.join(".ssh")).unwrap();
            fs::write(home.join(".ssh/config"), config).unwrap();
            for name in MANAGED_FILES {
                fs::write(home.join(".ssh").join(name), b"legacy").unwrap();
            }
            fs::write(home.join(".ssh/openresearch_connections.json"), b"{}").unwrap();
            let store = Store::open_at(home.join("data")).unwrap();
            Self {
                home,
                engine: Engine::new(store, Arc::new(TestClock::new())),
            }
        }

        fn retire(&self) -> Result<bool> {
            for _ in 0..20 {
                if self.engine.retire_ssh_config(&self.home)? {
                    return Ok(true);
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(false)
        }

        fn config(&self) -> Vec<u8> {
            fs::read(self.home.join(".ssh/config")).unwrap()
        }

        fn assert_managed(&self) {
            for name in MANAGED_FILES {
                assert!(self.home.join(".ssh").join(name).try_exists().unwrap());
            }
        }

        fn legacy(&self, status: &str) {
            self.engine
                .store()
                .upsert_run(&StoredRun {
                    id: "legacy".into(),
                    experiment_id: "exp".into(),
                    project_id: "proj".into(),
                    status: status.into(),
                    backend_json: r#"{"kind":"ssh_job","jobId":"/old/run"}"#.into(),
                    command: "train".into(),
                    created_at: 1,
                    updated_at: 1,
                    ended_at: None,
                    exit_code: None,
                    commit_sha: None,
                    result_markdown: None,
                    cancel_requested: false,
                    cancel_reason: None,
                    chat_session_id: None,
                })
                .unwrap();
        }

        fn cleanup(self) {
            drop(self.engine);
            fs::remove_dir_all(self.home).unwrap();
        }
    }

    #[test]
    fn config_and_its_metadata_are_untouched() {
        let config = [b"Host example\n  HostName example.org\n".as_slice(), FOOTER].concat();
        let f = Fixture::new(&config);
        let path = f.home.join(".ssh/config");
        let before = fs::metadata(&path).unwrap();
        assert!(f.retire().unwrap());
        assert!(f.retire().unwrap());
        assert_eq!(f.config(), config);
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            before.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let after = fs::metadata(&path).unwrap();
            assert_eq!(
                (after.ino(), after.mode(), after.uid()),
                (before.ino(), before.mode(), before.uid())
            );
        }
        f.assert_managed();
        f.cleanup();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_and_target_are_untouched() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new(FOOTER);
        let target = f.home.join("user-config");
        fs::rename(f.home.join(".ssh/config"), &target).unwrap();
        symlink(&target, f.home.join(".ssh/config")).unwrap();
        assert!(f.retire().unwrap());
        assert!(fs::symlink_metadata(f.home.join(".ssh/config"))
            .unwrap()
            .is_symlink());
        assert_eq!(fs::read(target).unwrap(), FOOTER);
        f.assert_managed();
        f.cleanup();
    }

    #[test]
    fn missing_files_and_ssh_directory_are_noops() {
        let f = Fixture::new(FOOTER);
        for name in MANAGED_FILES {
            fs::remove_file(f.home.join(".ssh").join(name)).unwrap();
        }
        assert!(f.retire().unwrap());
        assert!(!f.home.join(".ssh/.orx-config.lock").exists());
        assert_eq!(f.config(), FOOTER);
        fs::remove_dir_all(f.home.join(".ssh")).unwrap();
        assert!(f.retire().unwrap());
        assert!(!f.home.join(".ssh").exists());
        f.cleanup();
    }

    #[test]
    fn malformed_rows_only_block_while_either_supervisor_lock_is_held() {
        for malformed_backend in [false, true] {
            for lock_index in 0..2 {
                let f = Fixture::new(FOOTER);
                f.legacy("running");
                let db = rusqlite::Connection::open(f.home.join("data/orx.db")).unwrap();
                if malformed_backend {
                    db.execute(
                        "UPDATE runs SET backend_json = 'invalid' WHERE id = 'legacy'",
                        [],
                    )
                    .unwrap();
                } else {
                    db.execute("UPDATE runs SET status = 'unknown' WHERE id = 'legacy'", [])
                        .unwrap();
                }
                let paths = f.engine.store().supervisor_lock_paths("legacy").unwrap();
                fs::create_dir_all(paths[lock_index].parent().unwrap()).unwrap();
                let file = OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(&paths[lock_index])
                    .unwrap();
                let mut lock = fd_lock::RwLock::new(file);
                let guard = lock.try_write().unwrap();
                assert!(!f.retire().unwrap());
                assert_eq!(f.config(), FOOTER);
                drop(guard);
                assert!(f.retire().unwrap());
                f.assert_managed();
                drop(lock);
                drop(db);
                f.cleanup();
            }
        }
    }

    #[test]
    fn legacy_runs_block_until_adopted_or_finished_and_locks_released() {
        for lock_index in 0..2 {
            let f = Fixture::new(FOOTER);
            f.legacy("running");
            assert!(!f.retire().unwrap());
            let paths = f.engine.store().supervisor_lock_paths("legacy").unwrap();
            fs::create_dir_all(paths[lock_index].parent().unwrap()).unwrap();
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&paths[lock_index])
                .unwrap();
            let mut lock = fd_lock::RwLock::new(file);
            let guard = lock.try_write().unwrap();
            assert!(!f.retire().unwrap());
            f.legacy("done");
            assert!(!f.retire().unwrap());
            assert_eq!(f.config(), FOOTER);
            assert_eq!(
                fs::read(f.home.join(".ssh/openresearch_config")).unwrap(),
                b"legacy"
            );
            drop(guard);
            assert!(f.retire().unwrap());
            drop(lock);
            f.cleanup();
        }
        let f = Fixture::new(FOOTER);
        f.legacy("running");
        assert!(f.engine.own("legacy").unwrap());
        assert!(f.retire().unwrap());
        f.cleanup();
    }
}
