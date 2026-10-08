//! This machine's memory headroom, checked before and during a local run so a
//! run cannot starve the desktop, the dashboard, or the agent beside it.

/// A point-in-time view of RAM and swap, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemorySnapshot {
    pub total_bytes: u64,
    /// Memory a new workload can use without swapping (Linux `MemAvailable`,
    /// macOS free + inactive + speculative pages, Windows `ullAvailPhys`).
    pub available_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_free_bytes: u64,
}

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

impl MemorySnapshot {
    /// The least free RAM a new local run may start with: 1 GiB, or 5% of a
    /// large machine.
    pub fn launch_floor_bytes(&self) -> u64 {
        GIB.max(self.total_bytes / 20)
    }

    /// Below this much RAM plus swap, the machine is about to stall or start
    /// killing processes: 512 MiB, or 3% of a large machine.
    pub fn critical_floor_bytes(&self) -> u64 {
        (512 * MIB).max(self.total_bytes * 3 / 100)
    }

    /// Why a local run should not start now, if it shouldn't.
    pub fn launch_refusal(&self) -> Option<String> {
        if self.total_bytes == 0 || self.available_bytes >= self.launch_floor_bytes() {
            return None;
        }
        Some(format!(
            "Only {} of {} RAM is free on this machine, below the {} a local run needs to start \
             safely. Close other programs, wait for running jobs to finish, or launch on remote \
             compute (for example `--backend colab`).",
            fmt_bytes(self.available_bytes),
            fmt_bytes(self.total_bytes),
            fmt_bytes(self.launch_floor_bytes()),
        ))
    }

    /// RAM and swap are nearly exhausted.
    pub fn is_critical(&self) -> bool {
        self.total_bytes > 0
            && self.available_bytes.saturating_add(self.swap_free_bytes)
                < self.critical_floor_bytes()
    }

    pub fn describe(&self) -> String {
        let mut text = format!(
            "{} of {} RAM free",
            fmt_bytes(self.available_bytes),
            fmt_bytes(self.total_bytes)
        );
        if self.swap_total_bytes > 0 {
            text.push_str(&format!(
                ", {} of {} swap free",
                fmt_bytes(self.swap_free_bytes),
                fmt_bytes(self.swap_total_bytes)
            ));
        } else {
            text.push_str(", no swap");
        }
        text
    }
}

pub fn fmt_bytes(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else {
        format!("{} MiB", bytes / MIB)
    }
}

/// Best-effort memory probe; `None` when the platform reports nothing usable.
/// Blocking on macOS (subprocesses); call via `spawn_blocking` from async code.
pub fn memory_snapshot() -> Option<MemorySnapshot> {
    platform_snapshot().filter(|snapshot| snapshot.total_bytes > 0)
}

#[cfg(target_os = "linux")]
fn platform_snapshot() -> Option<MemorySnapshot> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)
}

/// Parse `/proc/meminfo` ("MemAvailable:   1234 kB").
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(raw: &str) -> Option<MemorySnapshot> {
    let field = |name: &str| -> Option<u64> {
        raw.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()
            .map(|kib| kib * 1024)
    };
    let total = field("MemTotal")?;
    // Kernels before 3.14 lack MemAvailable; free + page cache is the old estimate.
    let available = field("MemAvailable").or_else(|| {
        Some(field("MemFree")? + field("Cached").unwrap_or(0) + field("Buffers").unwrap_or(0))
    })?;
    Some(MemorySnapshot {
        total_bytes: total,
        available_bytes: available.min(total),
        swap_total_bytes: field("SwapTotal").unwrap_or(0),
        swap_free_bytes: field("SwapFree").unwrap_or(0),
    })
}

#[cfg(target_os = "macos")]
fn platform_snapshot() -> Option<MemorySnapshot> {
    let run = |program: &str, args: &[&str]| -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    };
    let total = run("sysctl", &["-n", "hw.memsize"])?.trim().parse().ok()?;
    let available = parse_vm_stat(&run("vm_stat", &[])?)?;
    let (swap_total, swap_free) = run("sysctl", &["-n", "vm.swapusage"])
        .and_then(|raw| parse_swapusage(&raw))
        .unwrap_or((0, 0));
    Some(MemorySnapshot {
        total_bytes: total,
        available_bytes: available.min(total),
        swap_total_bytes: swap_total,
        swap_free_bytes: swap_free,
    })
}

/// Reclaimable memory from `vm_stat`: free, inactive, speculative, and
/// purgeable pages times the page size in its header.
#[cfg(any(target_os = "macos", test))]
fn parse_vm_stat(raw: &str) -> Option<u64> {
    let page_size: u64 = raw
        .lines()
        .next()?
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |name: &str| -> u64 {
        raw.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().trim_end_matches('.').parse::<u64>().ok())
            .unwrap_or(0)
    };
    let free = pages("Pages free:")
        + pages("Pages inactive:")
        + pages("Pages speculative:")
        + pages("Pages purgeable:");
    Some(free * page_size)
}

/// `vm.swapusage`: "total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)".
#[cfg(any(target_os = "macos", test))]
fn parse_swapusage(raw: &str) -> Option<(u64, u64)> {
    let value = |name: &str| -> Option<u64> {
        let rest = raw.split(&format!("{name} = ")).nth(1)?;
        let token = rest.split_whitespace().next()?;
        let (number, unit) = token.split_at(token.len().checked_sub(1)?);
        let factor = match unit {
            "K" => 1024.0,
            "M" => MIB as f64,
            "G" => GIB as f64,
            _ => return None,
        };
        Some((number.parse::<f64>().ok()? * factor) as u64)
    };
    Some((value("total")?, value("free")?))
}

#[cfg(windows)]
fn platform_snapshot() -> Option<MemorySnapshot> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed MEMORYSTATUSEX with dwLength set is the documented input.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return None;
    }
    // The page file total includes physical memory.
    let swap_total = status.ullTotalPageFile.saturating_sub(status.ullTotalPhys);
    let swap_free = status
        .ullAvailPageFile
        .saturating_sub(status.ullAvailPhys)
        .min(swap_total);
    Some(MemorySnapshot {
        total_bytes: status.ullTotalPhys,
        available_bytes: status.ullAvailPhys,
        swap_total_bytes: swap_total,
        swap_free_bytes: swap_free,
    })
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_snapshot() -> Option<MemorySnapshot> {
    None
}

/// Free space on the filesystem holding `path`, in bytes.
#[cfg(unix)]
pub fn disk_free_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs writes into the zeroed struct; the path is NUL-terminated.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(windows)]
pub fn disk_free_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: a NUL-terminated wide path and a valid out pointer; the others may be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(total_gib: u64, available_mib: u64, swap_free_mib: u64) -> MemorySnapshot {
        MemorySnapshot {
            total_bytes: total_gib * GIB,
            available_bytes: available_mib * MIB,
            swap_total_bytes: swap_free_mib * MIB,
            swap_free_bytes: swap_free_mib * MIB,
        }
    }

    #[test]
    fn launch_needs_a_gib_or_five_percent_free() {
        assert!(snapshot(8, 3000, 0).launch_refusal().is_none());
        let refusal = snapshot(8, 700, 0).launch_refusal().unwrap();
        assert!(refusal.contains("700 MiB of 8.0 GiB"), "{refusal}");
        // 5% of 64 GiB is 3.2 GiB.
        assert!(snapshot(64, 3000, 0).launch_refusal().is_some());
        assert!(snapshot(64, 4000, 0).launch_refusal().is_none());
    }

    #[test]
    fn critical_counts_swap_as_headroom() {
        assert!(snapshot(8, 300, 0).is_critical());
        assert!(!snapshot(8, 300, 2048).is_critical());
        assert!(!snapshot(8, 2048, 0).is_critical());
    }

    #[test]
    fn meminfo_parses_available_and_swap() {
        let raw = "MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:    4000000 kB\nSwapTotal:       2000000 kB\nSwapFree:        1500000 kB\n";
        let parsed = parse_meminfo(raw).unwrap();
        assert_eq!(parsed.total_bytes, 16_000_000 * 1024);
        assert_eq!(parsed.available_bytes, 4_000_000 * 1024);
        assert_eq!(parsed.swap_free_bytes, 1_500_000 * 1024);
        let old = parse_meminfo("MemTotal: 1000 kB\nMemFree: 100 kB\nCached: 200 kB\n").unwrap();
        assert_eq!(old.available_bytes, 300 * 1024);
    }

    #[test]
    fn vm_stat_and_swapusage_parse() {
        let raw = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               10.\nPages active:                             99.\nPages inactive:                           20.\nPages speculative:                         2.\nPages purgeable:                           3.\n";
        assert_eq!(parse_vm_stat(raw), Some(35 * 16384));
        assert_eq!(
            parse_swapusage("total = 2048.00M  used = 1024.50M  free = 1023.50M  (encrypted)"),
            Some((2048 * MIB, (1023.5 * MIB as f64) as u64))
        );
    }
}
