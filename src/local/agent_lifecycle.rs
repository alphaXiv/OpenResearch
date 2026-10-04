//! When `orx up` releases idle resident agent children (Claude, Codex). Each
//! holds a few hundred MB, so low-RAM machines keep fewer of them warm; the
//! next message to a released chat respawns and resumes it.

use std::sync::LazyLock;
use std::time::Duration;

pub(crate) const REAPER_INTERVAL: Duration = Duration::from_secs(30);

const LOW_MEMORY_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Never evict for the idle limit sooner than this, so switching between two
/// chats does not respawn one on every message.
const MIN_IDLE_BEFORE_LIMIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdlePolicy {
    pub idle_timeout: Duration,
    /// Idle children kept warm per host; `None` is unbounded.
    pub max_idle: Option<usize>,
}

impl IdlePolicy {
    pub(crate) const DEFAULT: Self = Self {
        idle_timeout: Duration::from_secs(15 * 60),
        max_idle: None,
    };
    pub(crate) const LOW_MEMORY: Self = Self {
        idle_timeout: Duration::from_secs(2 * 60),
        max_idle: Some(1),
    };

    pub(crate) fn current() -> Self {
        static POLICY: LazyLock<IdlePolicy> =
            LazyLock::new(|| IdlePolicy::for_total_memory(total_memory_bytes()));
        *POLICY
    }

    fn for_total_memory(bytes: Option<u64>) -> Self {
        match bytes {
            Some(bytes) if bytes <= LOW_MEMORY_BYTES => Self::LOW_MEMORY,
            _ => Self::DEFAULT,
        }
    }

    /// Indexes of idle children to release over `max_idle`, least recently used
    /// first. `idle` holds each idle child's time since its last use.
    pub(crate) fn over_limit(&self, idle: &[Duration]) -> Vec<usize> {
        let Some(max_idle) = self.max_idle else {
            return Vec::new();
        };
        let mut order = (0..idle.len()).collect::<Vec<_>>();
        order.sort_by_key(|&index| idle[index]);
        order
            .into_iter()
            .skip(max_idle)
            .filter(|&index| idle[index] >= MIN_IDLE_BEFORE_LIMIT)
            .collect()
    }
}

#[cfg(unix)]
fn total_memory_bytes() -> Option<u64> {
    // SAFETY: sysconf only reads system configuration values.
    let (pages, page_size) = unsafe {
        (
            libc::sysconf(libc::_SC_PHYS_PAGES),
            libc::sysconf(libc::_SC_PAGESIZE),
        )
    };
    let pages = u64::try_from(pages).ok()?;
    let page_size = u64::try_from(page_size).ok()?;
    pages.checked_mul(page_size)
}

#[cfg(windows)]
fn total_memory_bytes() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain data; dwLength is set as the API requires.
    unsafe {
        let mut status: MEMORYSTATUSEX = std::mem::zeroed();
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut status) != 0).then_some(status.ullTotalPhys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn low_memory_machines_get_the_tight_policy() {
        assert_eq!(
            IdlePolicy::for_total_memory(Some(7 * GIB + GIB / 2)),
            IdlePolicy::LOW_MEMORY
        );
        assert_eq!(
            IdlePolicy::for_total_memory(Some(8 * GIB)),
            IdlePolicy::LOW_MEMORY
        );
        assert_eq!(
            IdlePolicy::for_total_memory(Some(16 * GIB)),
            IdlePolicy::DEFAULT
        );
        assert_eq!(IdlePolicy::for_total_memory(None), IdlePolicy::DEFAULT);
    }

    #[test]
    fn over_limit_keeps_the_most_recent_and_spares_just_used_children() {
        let secs = Duration::from_secs;
        let policy = IdlePolicy::LOW_MEMORY;

        assert_eq!(
            policy.over_limit(&[secs(90), secs(40), secs(60)]),
            vec![2, 0]
        );
        assert_eq!(policy.over_limit(&[secs(90), secs(5), secs(10)]), vec![0]);
        assert!(policy.over_limit(&[secs(90)]).is_empty());
        assert!(IdlePolicy::DEFAULT
            .over_limit(&[secs(900), secs(600)])
            .is_empty());
    }

    #[test]
    fn total_memory_is_detected() {
        assert!(total_memory_bytes().is_some_and(|bytes| bytes > 0));
    }
}
