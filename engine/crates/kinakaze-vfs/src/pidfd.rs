//! Process descriptors: `pidfd_open`, `CLONE_PIDFD`, `pidfd_send_signal`,
//! `pidfd_getfd` and `waitid(P_PIDFD)`.
//!
//! A pidfd names one Linux process incarnation, not a number: the registry pid
//! plus its birth tick, which survives exec but never pid reuse. That identity
//! is immutable, so it lives in an anonymous section whose handle is the fd's
//! host object; dup, fork and SCM_RIGHTS share it without extra state.
use crate::job::ESRCH;
use crate::{EBADF, EINVAL, EIO, EPERM, FdFlags, FdKind, errno_from_win32};
use kinakaze_runtime::job as table;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, FILE_MAP_READ, MapViewOfFile, PAGE_READWRITE,
    UnmapViewOfFile,
};

const MAGIC: u64 = u64::from_le_bytes(*b"CYPIDFD1");
/// `PIDFD_NONBLOCK`, which is `O_NONBLOCK`.
pub const PIDFD_NONBLOCK: i32 = 0o4000;
/// `PIDFD_THREAD`, which is `O_EXCL`. Hosted processes are single thread
/// groups, so a thread-group leader pidfd is the only kind that can exist.
pub const PIDFD_THREAD: i32 = 0o200;
const PIDFD_SIGNAL_THREAD: u32 = 1;
const PIDFD_SIGNAL_THREAD_GROUP: u32 = 2;
const PIDFD_SIGNAL_PROCESS_GROUP: u32 = 4;
/// Never matches a registry row: the child was already reaped when the
/// descriptor was requested.
const REAPED: u64 = u64::MAX;

#[repr(C)]
#[derive(Clone, Copy)]
struct State {
    magic: u64,
    pid: u32,
    reserved: u32,
    start_ticks: u64,
}

/// The process one pidfd refers to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    /// Registry (root namespace) pid.
    pid: u32,
    start_ticks: u64,
}

impl Target {
    fn entry(self) -> Option<table::Entry> {
        table::lookup(self.pid).filter(|entry| entry.start_ticks == self.start_ticks)
    }

    /// Linux makes a pidfd readable once the process has exited, while it is
    /// still a zombie; a reaped or recycled pid is exited as well.
    pub fn exited(self) -> bool {
        self.entry()
            .is_none_or(|entry| entry.flags & table::FLAG_ZOMBIE != 0)
    }

    /// The pid as the caller's PID namespace sees it, while the process (or
    /// its zombie) still exists.
    pub fn visible_pid(self) -> Option<u32> {
        self.entry()?;
        table::namespaces::visible(self.pid)
    }

    pub fn is_current(self) -> bool {
        self.entry()
            .is_some_and(|entry| entry.namespace_pid == crate::job::process_id())
    }
}

fn create(target: Target, nonblock: bool) -> Result<i32, i32> {
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            4096,
            ptr::null(),
        )
    };
    if handle.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
    if view.Value.is_null() {
        unsafe { CloseHandle(handle) };
        return Err(EIO);
    }
    unsafe {
        ptr::write(
            view.Value.cast::<State>(),
            State {
                magic: MAGIC,
                pid: target.pid,
                reserved: 0,
                start_ticks: target.start_ticks,
            },
        );
        UnmapViewOfFile(view);
    }
    // Linux always creates pidfds close-on-exec.
    let mut flags = FdFlags::CLOSE_ON_EXEC;
    if nonblock {
        flags = flags.union(FdFlags::NONBLOCK);
    }
    crate::install(handle as usize, FdKind::PidFd, flags).inspect_err(|_| unsafe {
        CloseHandle(handle);
    })
}

fn read_state(raw: HANDLE) -> Result<Target, i32> {
    let view = unsafe { MapViewOfFile(raw, FILE_MAP_READ, 0, 0, size_of::<State>()) };
    if view.Value.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let state = unsafe { ptr::read(view.Value.cast::<State>()) };
    unsafe { UnmapViewOfFile(view) };
    if state.magic != MAGIC {
        return Err(EINVAL);
    }
    Ok(Target {
        pid: state.pid,
        start_ticks: state.start_ticks,
    })
}

/// Resolves a pidfd. Any other descriptor is `EBADF`, as Linux reports.
pub fn target(fd: i32) -> Result<(Target, FdFlags), i32> {
    let entry = crate::get(fd)?;
    if entry.kind != FdKind::PidFd {
        return Err(EBADF);
    }
    Ok((read_state(entry.raw as HANDLE)?, entry.flags))
}

/// `pidfd_open(2)`.
pub fn open(pid: i32, flags: i32) -> Result<i32, i32> {
    if pid <= 0 || flags & !(PIDFD_NONBLOCK | PIDFD_THREAD) != 0 {
        return Err(EINVAL);
    }
    crate::job::ensure_registered();
    let pid = table::namespaces::resolve(pid as u32).ok_or(ESRCH)?;
    let entry = table::lookup(pid).ok_or(ESRCH)?;
    create(
        Target {
            pid,
            start_ticks: entry.start_ticks,
        },
        flags & PIDFD_NONBLOCK != 0,
    )
}

/// The `CLONE_PIDFD` result for a child the caller just created. Unlike
/// `pidfd_open` this cannot fail with `ESRCH`: a child that exited and was
/// auto-reaped in between still gets a descriptor, which is simply readable.
pub fn for_child(pid: i32) -> Result<i32, i32> {
    let resolved = table::namespaces::resolve(pid as u32).unwrap_or(0);
    let target = match table::lookup(resolved) {
        Some(entry) if resolved != 0 => Target {
            pid: resolved,
            start_ticks: entry.start_ticks,
        },
        _ => Target {
            pid: resolved,
            start_ticks: REAPED,
        },
    };
    create(target, false)
}

/// Readiness for poll, select and epoll: readable once the process exited.
pub fn poll(fd: i32) -> Result<bool, i32> {
    Ok(target(fd)?.0.exited())
}

/// `pidfd_send_signal(2)`. `info` carries the caller's `si_code` and
/// `si_value` when a siginfo was supplied.
pub fn send_signal(
    fd: i32,
    signal: i32,
    info: Option<(i32, i32, usize)>,
    flags: u32,
) -> Result<(), i32> {
    let scope = match flags {
        0 | PIDFD_SIGNAL_THREAD_GROUP => PIDFD_SIGNAL_THREAD_GROUP,
        PIDFD_SIGNAL_THREAD | PIDFD_SIGNAL_PROCESS_GROUP => flags,
        _ => return Err(EINVAL),
    };
    if signal < 0 || signal as usize >= crate::signal::NSIG {
        return Err(EINVAL);
    }
    let (target, _) = target(fd)?;
    crate::job::ensure_registered();
    let entry = target.entry().ok_or(ESRCH)?;
    let own = entry.namespace_pid == crate::job::process_id();
    if let Some((signo, code, _)) = info {
        if signo != signal {
            return Err(EINVAL);
        }
        // Only the kernel may forge a positive si_code for another process.
        if (code >= 0 || code == -6) && !own {
            return Err(EPERM);
        }
    }
    if scope == PIDFD_SIGNAL_PROCESS_GROUP {
        return crate::job::signal_process_group(entry.pgid as i32, signal);
    }
    if entry.flags & table::FLAG_ZOMBIE != 0 || signal == 0 {
        return Ok(());
    }
    if let Some((_, _, value)) = info {
        let visible = table::namespaces::visible(target.pid).ok_or(ESRCH)?;
        return crate::job::sigqueue(visible as i32, signal, value);
    }
    if crate::job::deliver_to(entry.namespace_pid, signal) {
        Ok(())
    } else {
        Err(ESRCH)
    }
}

/// `pidfd_getfd(2)`: a close-on-exec duplicate of `target_fd` in the process
/// behind `fd`.
pub fn getfd(fd: i32, target_fd: i32, flags: u32) -> Result<i32, i32> {
    if flags != 0 {
        return Err(EINVAL);
    }
    let (target, _) = target(fd)?;
    crate::job::ensure_registered();
    if target.exited() {
        return Err(ESRCH);
    }
    if target.is_current() {
        return crate::duplicate_descriptor(target_fd, crate::DuplicateTarget::Lowest, true);
    }
    crate::job::remote_fd::fetch(target.pid, target_fd)
}

/// The pid `waitid(P_PIDFD, fd)` waits for, or `ECHILD` once it was reaped.
/// A nonblocking pidfd whose process is still running is `EAGAIN` unless
/// the caller asked for `WNOHANG` itself.
pub fn wait_target(fd: i32, nohang: bool) -> Result<i32, i32> {
    const ECHILD: i32 = 10;
    const EAGAIN: i32 = 11;
    let (target, flags) = target(fd)?;
    let pid = target.visible_pid().ok_or(ECHILD)?;
    if flags.contains(FdFlags::NONBLOCK) && !nohang && !target.exited() {
        return Err(EAGAIN);
    }
    Ok(pid as i32)
}

/// `/proc/<pid>/fdinfo` style `Pid:` value; `-1` once the process is gone.
pub fn reported_pid(fd: i32) -> Result<i32, i32> {
    let (target, _) = target(fd)?;
    Ok(target.visible_pid().map_or(-1, |pid| pid as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pidfd_is_running_cloexec_and_resolves_to_self() {
        let pid = crate::job::process_id() as i32;
        let fd = open(pid, PIDFD_NONBLOCK).unwrap();
        let entry = crate::get(fd).unwrap();
        assert_eq!(entry.kind, FdKind::PidFd);
        assert!(entry.flags.contains(FdFlags::CLOSE_ON_EXEC));
        assert!(entry.flags.contains(FdFlags::NONBLOCK));
        assert!(!poll(fd).unwrap());
        assert_eq!(reported_pid(fd).unwrap(), pid);
        assert!(target(fd).unwrap().0.is_current());
        send_signal(fd, 0, None, 0).unwrap();
        assert_eq!(send_signal(fd, 0, None, 8), Err(EINVAL));
        assert_eq!(wait_target(fd, false), Err(11));
        let copy = getfd(fd, fd, 0).unwrap();
        assert_eq!(crate::get(copy).unwrap().kind, FdKind::PidFd);
        assert!(
            crate::get(copy)
                .unwrap()
                .flags
                .contains(FdFlags::CLOSE_ON_EXEC)
        );
        crate::close(copy).unwrap();
        crate::close(fd).unwrap();
    }

    #[test]
    fn invalid_requests_follow_linux_errors() {
        assert_eq!(open(0, 0), Err(EINVAL));
        assert_eq!(open(-1, 0), Err(EINVAL));
        assert_eq!(open(1, 1), Err(EINVAL));
        assert_eq!(open(0x3fff_fff0, 0), Err(ESRCH));
        let null = crate::fs::open("/dev/null", crate::fs::O_RDONLY, 0).unwrap();
        assert_eq!(poll(null), Err(EBADF));
        assert_eq!(send_signal(null, 0, None, 0), Err(EBADF));
        crate::close(null).unwrap();
    }

    #[test]
    fn reaped_child_descriptor_is_readable_and_unwaitable() {
        let fd = create(
            Target {
                pid: 0,
                start_ticks: REAPED,
            },
            false,
        )
        .unwrap();
        assert!(poll(fd).unwrap());
        assert_eq!(reported_pid(fd).unwrap(), -1);
        assert_eq!(wait_target(fd, false), Err(10));
        assert_eq!(send_signal(fd, 15, None, 0), Err(ESRCH));
        assert_eq!(getfd(fd, 0, 0), Err(ESRCH));
        crate::close(fd).unwrap();
    }
}
