//! Local-run memory guard, owned by the tuandang304 fork.

use super::*;

/// Stops a local run before it exhausts this machine's memory: two
/// consecutive critical readings (one poll apart, so a brief spike passes)
/// terminate the run and record why.
#[derive(Default)]
pub(super) struct MemoryGuard {
    critical_polls: u8,
    pub(super) reason: Option<String>,
}

impl MemoryGuard {
    /// Take a reading; true when this reading tripped the guard.
    pub(super) async fn check(&mut self) -> bool {
        let snapshot = tokio::task::spawn_blocking(localbox::memory_snapshot)
            .await
            .ok()
            .flatten();
        let Some(snapshot) = snapshot.filter(|memory| memory.is_critical()) else {
            self.critical_polls = 0;
            return false;
        };
        self.critical_polls += 1;
        if self.critical_polls < 2 {
            return false;
        }
        self.reason = Some(format!(
            "Stopped by orx to keep this machine responsive: memory ran out during the run \
             ({}). Lower the batch size, sequence length, or model size, stream the data \
             instead of loading it all at once, or launch on remote compute such as \
             `--backend colab`.",
            snapshot.describe()
        ));
        true
    }
}
