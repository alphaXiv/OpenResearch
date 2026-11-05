use orx_host_proto::{CancelOutcome, ProcessIdentity, Survivor};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

#[derive(Debug, Clone)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub starttime: u64,
    pub state: String,
}

impl Process {
    pub fn survivor(&self) -> Survivor {
        Survivor {
            pid: self.pid,
            starttime: self.starttime,
            state: self.state.clone(),
        }
    }

    pub fn live(&self) -> bool {
        !matches!(self.state.as_str(), "Z" | "X")
    }
}

#[cfg(not(target_os = "macos"))]
pub fn node() -> io::Result<String> {
    let mut bytes = [0u8; 256];
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

#[cfg(target_os = "macos")]
pub fn node() -> io::Result<String> {
    let mut uuid = [0u8; 16];
    let timeout = libc::timespec {
        tv_sec: 5,
        tv_nsec: 0,
    };
    if unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &timeout) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uuid.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn identity(pid: u32) -> io::Result<ProcessIdentity> {
    Ok(ProcessIdentity {
        node: node()?,
        boot_id: boot_id()?,
        pid,
        starttime: process(pid)?.starttime,
    })
}

pub fn alive(identity: &ProcessIdentity) -> io::Result<bool> {
    if identity.node != node()? {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "wrapper belongs to another node",
        ));
    }
    if identity.boot_id != boot_id()? {
        return Ok(false);
    }
    match process(identity.pid) {
        Ok(process) => Ok(process.starttime == identity.starttime && process.live()),
        Err(error) if vanished(&error) => Ok(false),
        Err(error) => Err(error),
    }
}

fn vanished(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || matches!(error.raw_os_error(), Some(libc::ESRCH))
}

#[derive(Debug)]
pub struct Scan {
    pub processes: Vec<Process>,
    pub unreadable: Vec<u32>,
    pub complete: bool,
}

impl Scan {
    /// A previously seen process remains owned even after it scrubs its run tag.
    pub(crate) fn include(&mut self, known: &[Survivor]) -> io::Result<()> {
        for survivor in known {
            match process(survivor.pid) {
                Ok(process) if process.starttime == survivor.starttime && process.live() => {
                    if !self
                        .processes
                        .iter()
                        .any(|seen| seen.pid == process.pid && seen.starttime == process.starttime)
                    {
                        self.processes.push(process);
                    }
                }
                Ok(_) => {}
                Err(error) if vanished(&error) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn outcome(&self, children_reaped: bool) -> CancelOutcome {
        if !self.processes.is_empty() {
            return CancelOutcome::Survivors;
        }
        if self.complete && children_reaped {
            CancelOutcome::Verified
        } else {
            CancelOutcome::Unverifiable {
                unreadable: self.unreadable.clone(),
            }
        }
    }
}

#[cfg_attr(test, derive(Default))]
pub(crate) struct ProcessTable {
    pub(crate) processes: BTreeMap<u32, Process>,
    pub(crate) tags: BTreeMap<String, BTreeSet<u32>>,
    unreadable: Vec<u32>,
}

impl ProcessTable {
    pub(crate) fn read() -> io::Result<Self> {
        Ok(Self::from_pids(pids()?, process, process_uid))
    }

    fn from_pids(
        pids: Vec<u32>,
        process: impl Fn(u32) -> io::Result<Process>,
        process_uid: impl Fn(u32) -> io::Result<u32>,
    ) -> Self {
        let mut table = Self {
            processes: BTreeMap::new(),
            tags: BTreeMap::new(),
            unreadable: vec![],
        };
        let uid = unsafe { libc::getuid() };
        for pid in pids {
            let process = match process(pid) {
                Ok(process) => process,
                Err(error) if vanished(&error) => continue,
                Err(_) => {
                    match process_uid(pid) {
                        Ok(owner) if owner != uid => {}
                        Err(error) if vanished(&error) => {}
                        _ => table.unreadable.push(pid),
                    }
                    continue;
                }
            };
            if process.uid == uid && process.live() {
                match environment(pid) {
                    Ok(env) => {
                        for entry in env.split(|&byte| byte == 0) {
                            if let Some(tag) = entry
                                .strip_prefix(b"ORX_RUN_TAG=")
                                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                            {
                                table.tags.entry(tag.to_owned()).or_default().insert(pid);
                            }
                        }
                    }
                    Err(error) if vanished(&error) => continue,
                    Err(_) if openssh_session(pid) => {}
                    Err(_) => table.unreadable.push(pid),
                }
            }
            table.processes.insert(pid, process);
        }
        table
    }

    fn select(&self, wrapper: Option<&ProcessIdentity>, tag: &str, subreaper: bool) -> Scan {
        let live_wrapper = wrapper.filter(|identity| alive(identity).unwrap_or(false));
        let mut descendants: BTreeSet<_> = live_wrapper
            .map(|identity| identity.pid)
            .into_iter()
            .collect();
        loop {
            let before = descendants.len();
            for process in self.processes.values() {
                if descendants.contains(&process.ppid) {
                    descendants.insert(process.pid);
                }
            }
            if descendants.len() == before {
                break;
            }
        }
        if let Some(pids) = self.tags.get(tag) {
            descendants.extend(pids);
        }
        let processes = descendants
            .into_iter()
            .filter(|&pid| wrapper.is_none_or(|wrapper| pid != wrapper.pid))
            .filter_map(|pid| self.processes.get(&pid))
            .filter(|process| process.live())
            .cloned()
            .collect();
        // Processes older than the wrapper cannot inherit its newly minted tag.
        let unreadable: Vec<_> = self
            .unreadable
            .iter()
            .copied()
            .filter(|pid| {
                wrapper.is_none_or(|wrapper| {
                    self.processes
                        .get(pid)
                        .is_none_or(|process| process.starttime >= wrapper.starttime)
                })
            })
            .collect();
        Scan {
            processes,
            complete: (subreaper || cfg!(target_os = "macos"))
                && unreadable.is_empty()
                && live_wrapper.is_some()
                && wrapper.is_some_and(|wrapper| alive(wrapper).unwrap_or(false)),
            unreadable,
        }
    }
}

pub fn scan(wrapper: Option<&ProcessIdentity>, tag: &str, subreaper: bool) -> io::Result<Scan> {
    if let Some(wrapper) = wrapper {
        if wrapper.node != node()? || wrapper.boot_id != boot_id()? {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "wrapper belongs to another node or boot",
            ));
        }
    }
    Ok(ProcessTable::read()?.select(wrapper, tag, subreaper))
}

pub fn signal(process: &Process, signal: i32) -> io::Result<()> {
    #[cfg(all(feature = "test-hooks", debug_assertions))]
    if let Some(path) = std::env::var_os("ORX_TEST_DEFER_SIGNALS") {
        let path = std::path::Path::new(&path);
        if path.try_exists()? && matches!(signal, libc::SIGTERM | libc::SIGKILL) {
            if signal == libc::SIGKILL && std::fs::read(path)? == b"once" {
                std::fs::remove_file(path)?;
            }
            return Ok(());
        }
    }
    #[cfg(target_os = "linux")]
    {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, process.pid, 0) } as i32;
        if fd >= 0 {
            use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            if self::process(process.pid)?.starttime != process.starttime {
                return Ok(());
            }
            if unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd.as_raw_fd(),
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                )
            } == 0
            {
                return Ok(());
            }
            return Err(io::Error::last_os_error());
        }
        let error = io::Error::last_os_error();
        // Container seccomp profiles that predate pidfd answer EPERM, which pidfd_open never does.
        if !matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS | libc::EINVAL | libc::EPERM)
        ) {
            return Err(error);
        }
    }
    if self::process(process.pid)?.starttime != process.starttime {
        return Ok(());
    }
    if unsafe { libc::kill(process.pid as i32, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn kernel() -> io::Result<String> {
    let mut info = unsafe { std::mem::zeroed::<libc::utsname>() };
    if unsafe { libc::uname(&mut info) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(info.release.as_ptr()) }
        .to_string_lossy()
        .into_owned())
}

pub fn supported_kernel(release: &str) -> bool {
    let mut parts = release
        .split('.')
        .filter_map(|part| part.parse::<u32>().ok());
    matches!((parts.next(), parts.next()), (Some(major), Some(minor)) if major > 3 || (major == 3 && minor >= 2))
}

#[cfg(target_os = "linux")]
pub fn subreaper() -> io::Result<bool> {
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EINVAL) {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(target_os = "linux")]
pub fn boot_id() -> io::Result<String> {
    Ok(std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned())
}

#[cfg(target_os = "linux")]
pub fn pids() -> io::Result<Vec<u32>> {
    std::fs::read_dir("/proc")?
        .filter_map(|entry| match entry {
            Ok(entry) => entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse().ok())
                .map(Ok),
            Err(error) => Some(Err(error)),
        })
        .collect()
}

pub fn parse_stat(pid: u32, uid: u32, stat: &str) -> io::Result<Process> {
    let (_, fields) = stat
        .rsplit_once(") ")
        .ok_or_else(|| io::Error::other("invalid proc stat"))?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let number = |index: usize| -> io::Result<u64> {
        fields
            .get(index)
            .ok_or_else(|| io::Error::other("short proc stat"))?
            .parse()
            .map_err(io::Error::other)
    };
    Ok(Process {
        pid,
        uid,
        ppid: number(1)?.try_into().map_err(io::Error::other)?,
        state: fields
            .first()
            .ok_or_else(|| io::Error::other("missing state"))?
            .to_string(),
        starttime: number(19)?,
    })
}

#[cfg(target_os = "linux")]
pub fn process(pid: u32) -> io::Result<Process> {
    let path = format!("/proc/{pid}");
    parse_stat(
        pid,
        process_uid(pid)?,
        &std::fs::read_to_string(format!("{path}/stat"))?,
    )
}

#[cfg(any(target_os = "linux", test))]
fn parse_uid(status: &str) -> io::Result<u32> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|line| line.split_whitespace().next())
        .ok_or_else(|| io::Error::other("missing process Uid"))?
        .parse()
        .map_err(io::Error::other)
}

#[cfg(target_os = "linux")]
fn process_uid(pid: u32) -> io::Result<u32> {
    parse_uid(&std::fs::read_to_string(format!("/proc/{pid}/status"))?)
}

#[cfg(target_os = "macos")]
fn process_uid(pid: u32) -> io::Result<u32> {
    unsafe extern "C" {
        fn orx_process_uid(pid: libc::pid_t, uid: *mut libc::uid_t) -> libc::c_int;
    }

    let mut uid = 0;
    if unsafe { orx_process_uid(pid as libc::pid_t, &mut uid) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

#[cfg(target_os = "linux")]
fn environment(pid: u32) -> io::Result<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/environ"))
}

/// OpenSSH makes its processes non-dumpable, so their environment is unreadable even to the
/// owner; they never descend from a run.
#[cfg(target_os = "linux")]
fn openssh_session(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .is_ok_and(|comm| comm.trim_end() == "sshd" || comm.starts_with("sshd-"))
}

#[cfg(target_os = "macos")]
fn openssh_session(_: u32) -> bool {
    false
}

#[cfg(target_os = "macos")]
pub fn subreaper() -> io::Result<bool> {
    Ok(false)
}

#[cfg(target_os = "macos")]
pub fn boot_id() -> io::Result<String> {
    let mut bytes = [0u8; 128];
    let mut size = bytes.len();
    if unsafe {
        libc::sysctlbyname(
            c"kern.bootsessionuuid".as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let uuid = std::ffi::CStr::from_bytes_until_nul(&bytes[..size]).map_err(io::Error::other)?;
    Ok(uuid.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
pub fn pids() -> io::Result<Vec<u32>> {
    let count = unsafe { libc::proc_listpids(1, 0, std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pids = vec![0u32; count as usize / 4 + 1024];
    let bytes =
        unsafe { libc::proc_listpids(1, 0, pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
    if bytes <= 0 || bytes as usize >= pids.len() * 4 {
        return Err(io::Error::other("incomplete process list"));
    }
    pids.truncate(bytes as usize / 4);
    pids.retain(|&pid| pid > 0);
    Ok(pids)
}

#[cfg(target_os = "macos")]
pub fn process(pid: u32) -> io::Result<Process> {
    let mut info = unsafe { std::mem::zeroed::<libc::proc_bsdinfo>() };
    let size = std::mem::size_of_val(&info) as i32;
    let result = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    if result != size {
        return Err(io::Error::last_os_error());
    }
    Ok(Process {
        pid,
        ppid: info.pbi_ppid,
        uid: info.pbi_uid,
        starttime: info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec,
        state: if info.pbi_status == 5 { "Z" } else { "R" }.into(),
    })
}

#[cfg(target_os = "macos")]
fn environment(pid: u32) -> io::Result<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut bytes = vec![0; 1024 * 1024];
    let mut size = bytes.len();
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    bytes.truncate(size);
    // KERN_PROCARGS2 starts with argc, then the executable path, argv and env strings.
    let argc = i32::from_ne_bytes(
        bytes
            .get(..4)
            .ok_or_else(|| io::Error::other("short procargs"))?
            .try_into()
            .map_err(io::Error::other)?,
    );
    if argc < 0 {
        return Err(io::Error::other("invalid argc"));
    }
    let mut rest = &bytes[4..];
    let end = rest
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| io::Error::other("missing executable"))?;
    rest = &rest[end..];
    while rest.first() == Some(&0) {
        rest = &rest[1..];
    }
    for _ in 0..argc {
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or_else(|| io::Error::other("missing argument"))?;
        rest = &rest[end + 1..];
    }
    Ok(rest.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unreadable_processes_proven_to_predate_the_wrapper_can_be_excluded() {
        let wrapper = identity(std::process::id()).unwrap();
        let mut table = ProcessTable::default();
        let pid = u32::MAX;
        table.processes.insert(
            pid,
            Process {
                pid,
                ppid: 1,
                uid: unsafe { libc::getuid() },
                starttime: wrapper.starttime - 1,
                state: "S".into(),
            },
        );
        table.unreadable.push(pid);
        let scan = table.select(Some(&wrapper), "not-minted-yet", true);
        assert_eq!(scan.outcome(true), CancelOutcome::Verified);
        assert!(scan.unreadable.is_empty());
        for starttime in [wrapper.starttime, wrapper.starttime + 1] {
            table.processes.get_mut(&pid).unwrap().starttime = starttime;
            assert_eq!(
                table.select(Some(&wrapper), "tag", true).outcome(true),
                CancelOutcome::Unverifiable {
                    unreadable: vec![pid]
                }
            );
        }
        table.processes.clear();
        assert_eq!(
            table.select(Some(&wrapper), "tag", true).outcome(true),
            CancelOutcome::Unverifiable {
                unreadable: vec![pid]
            }
        );
    }

    #[test]
    fn unknown_ownership_cannot_make_a_failed_inspection_verified() {
        let wrapper = identity(std::process::id()).unwrap();
        let pid = u32::MAX;
        let uid = unsafe { libc::getuid() };
        for owner in [None, Some(uid), Some(uid + 1)] {
            let table = ProcessTable::from_pids(
                vec![pid],
                |_| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                |_| owner.ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied)),
            );
            let outcome = table.select(Some(&wrapper), "tag", true).outcome(true);
            if owner == Some(uid + 1) {
                assert_eq!(outcome, CancelOutcome::Verified);
            } else {
                assert_eq!(
                    outcome,
                    CancelOutcome::Unverifiable {
                        unreadable: vec![pid]
                    }
                );
            }
        }
        for code in [libc::ENOENT, libc::ESRCH] {
            let table = ProcessTable::from_pids(
                vec![pid],
                |_| Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                |_| Err(io::Error::from_raw_os_error(code)),
            );
            assert_eq!(
                table.select(Some(&wrapper), "tag", true).outcome(true),
                CancelOutcome::Verified
            );
        }
    }

    #[test]
    fn identity_rejects_reused_pid_and_reboot() {
        let mut own = identity(std::process::id()).unwrap();
        assert!(alive(&own).unwrap());
        own.starttime += 1;
        assert!(!alive(&own).unwrap());
        own.starttime -= 1;
        own.boot_id.push_str("-reboot");
        assert!(!alive(&own).unwrap());
        own = identity(std::process::id()).unwrap();
        own.node.push_str("-other-node");
        assert_eq!(alive(&own).unwrap_err().kind(), io::ErrorKind::NotConnected);
        assert_eq!(
            scan(Some(&own), "tag", false).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        own = identity(std::process::id()).unwrap();
        own.boot_id.push_str("-other-boot");
        assert_eq!(
            scan(Some(&own), "tag", false).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_zombies_are_gone_not_unreadable() {
        let child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!ProcessTable::read().unwrap().unreadable.contains(&pid));
        drop(child);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_other_uid_processes_have_readable_ownership() {
        let uid = unsafe { libc::getuid() };
        let mut other_uid = Vec::new();
        for pid in pids().unwrap() {
            match process_uid(pid) {
                Ok(owner) if owner != uid => other_uid.push(pid),
                Ok(owner) => {
                    if let Ok(process) = process(pid) {
                        assert_eq!(owner, process.uid);
                    }
                }
                Err(error) if matches!(error.raw_os_error(), Some(libc::ESRCH)) => {}
                Err(error) => panic!("owner of {pid}: {error}"),
            }
        }
        assert!(!other_uid.is_empty(), "test requires other-uid processes");
        let table = ProcessTable::read().unwrap();
        assert!(other_uid.iter().all(|pid| !table.unreadable.contains(pid)));
        assert!(table.unreadable.is_empty(), "{:?}", table.unreadable);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_node_is_the_hardware_uuid() {
        let mut uuid = [0u8; 16];
        let timeout = libc::timespec {
            tv_sec: 5,
            tv_nsec: 0,
        };
        assert_eq!(unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &timeout) }, 0);
        let expected: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(node().unwrap(), expected);
        assert_eq!(identity(std::process::id()).unwrap().node, expected);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_complete_tag_scan_and_reaped_children_verify_the_stop() {
        let wrapper = identity(std::process::id()).unwrap();
        let mut table = ProcessTable::default();
        assert_eq!(
            table.select(Some(&wrapper), "tag", false).outcome(true),
            CancelOutcome::Verified
        );
        assert!(matches!(
            table.select(Some(&wrapper), "tag", false).outcome(false),
            CancelOutcome::Unverifiable { .. }
        ));
        table.unreadable.push(u32::MAX);
        assert!(matches!(
            table.select(Some(&wrapper), "tag", false).outcome(true),
            CancelOutcome::Unverifiable { .. }
        ));
    }

    #[test]
    fn cancellation_requires_complete_scan_and_reaped_children() {
        let mut scan = Scan {
            processes: vec![],
            unreadable: vec![],
            complete: true,
        };
        assert_eq!(scan.outcome(true), CancelOutcome::Verified);
        assert!(matches!(
            scan.outcome(false),
            CancelOutcome::Unverifiable { .. }
        ));
        scan.complete = false;
        scan.unreadable.push(123);
        assert_eq!(
            scan.outcome(true),
            CancelOutcome::Unverifiable {
                unreadable: vec![123]
            }
        );
        scan.processes.push(Process {
            pid: 42,
            ppid: 1,
            uid: 0,
            starttime: 3,
            state: "D".into(),
        });
        assert!(matches!(scan.outcome(true), CancelOutcome::Survivors));
    }

    #[test]
    fn process_uid_uses_status_real_uid_instead_of_proc_directory_owner() {
        assert_eq!(
            parse_uid("Name:\tworker\nUid:\t501\t0\t0\t0\n").unwrap(),
            501
        );
        for status in ["", "Uid:\n", "Uid:\tnone\t0\t0\t0\n"] {
            assert!(parse_uid(status).is_err());
        }
    }

    #[test]
    fn linux_stat_parser_handles_parentheses_and_field_positions() {
        let mut fields = vec!["0"; 20];
        fields[0] = "S";
        fields[1] = "5";
        fields[19] = "123456";
        let stat = format!("42 (name ) with spaces) {}", fields.join(" "));
        let process = parse_stat(42, 501, &stat).unwrap();
        assert_eq!(
            (process.pid, process.ppid, process.uid, process.starttime),
            (42, 5, 501, 123456)
        );
        assert!(parse_stat(42, 501, "42 (truncated)").is_err());
    }

    #[test]
    fn linux_kernel_floor() {
        for release in ["3.2.0", "3.4.1", "6.12.0-custom"] {
            assert!(supported_kernel(release));
        }
        for release in ["2.6.32", "3.1.9", "unknown"] {
            assert!(!supported_kernel(release));
        }
    }

    #[test]
    fn wrong_starttime_never_signals_a_live_process() {
        let mut own = process(std::process::id()).unwrap();
        own.starttime += 1;
        signal(&own, libc::SIGKILL).unwrap();
        assert!(process(std::process::id()).is_ok());
    }
}
