//! Picking a Colab run up again after this machine shut down, owned by the
//! tuandang304 fork.

use super::*;

/// A Colab run goes on without this machine, so a controller that died with it
/// (a shutdown, a crash) is started again to follow the run instead of failing it.
pub(super) fn resume_colab(dir: &std::path::Path, run_id: &str) -> bool {
    #[cfg(windows)]
    {
        let _ = (dir, run_id);
        false
    }
    #[cfg(not(windows))]
    {
        if !crate::jobs::colab::take_resume(dir) {
            return false;
        }
        let script = format!(
            "bash {}",
            crate::jobs::ssh::sh_quote(crate::jobs::colab::CONTROLLER_FILE)
        );
        match localbox::resume_job(dir, &script) {
            Ok(()) => {
                eprintln!(
                    "supervise {run_id}: the Colab controller stopped; picking the run up again"
                );
                true
            }
            Err(err) => {
                eprintln!("supervise {run_id}: could not pick the Colab run up again: {err}");
                false
            }
        }
    }
}
