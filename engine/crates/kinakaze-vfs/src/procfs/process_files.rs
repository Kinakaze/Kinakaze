//! Process inspection files. Host counters include the guest's worker runtime.
use super::*;
use crate::{FdFlags, FdKind, fs};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Memory::VirtualQueryEx;
use windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS_EX;
use windows_sys::Win32::System::Threading::{
    GetProcessIoCounters, IO_COUNTERS, OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
};

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
fn process(pid: u32) -> Result<Process, i32> {
    let info = process_info(pid)?;
    // Only registered, namespace-visible guests reach this function.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_INFORMATION | PROCESS_VM_READ,
            0,
            info.entry.pid,
        )
    };
    if handle.is_null() {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    Ok(Process(handle))
}

pub(super) fn statm(pid: u32) -> Result<String, i32> {
    let process = process(pid)?;
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    if unsafe { GetProcessMemoryInfo(process.0, &mut counters, size_of_val(&counters) as u32) } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let mut address = 0usize;
    let mut size = 0u64;
    let mut text = 0u64;
    let mut data = 0u64;
    loop {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe {
            VirtualQueryEx(
                process.0,
                address as *const c_void,
                &mut region,
                size_of_val(&region),
            )
        } == 0
        {
            break;
        }
        if region.State != MEM_FREE {
            size = size.saturating_add(region.RegionSize as u64);
        }
        if region.State == MEM_COMMIT {
            let protection = region.Protect & 0xff;
            if matches!(
                protection,
                PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY
            ) {
                text = text.saturating_add(region.RegionSize as u64);
            } else if matches!(protection, PAGE_READWRITE | PAGE_WRITECOPY) {
                data = data.saturating_add(region.RegionSize as u64);
            }
        }
        let Some(next) = (region.BaseAddress as usize).checked_add(region.RegionSize) else {
            break;
        };
        if next <= address {
            break;
        }
        address = next;
    }
    // Shared RSS is not available from these counters. lib/dt are obsolete
    // Linux fields and zero; text/data approximate committed host protections.
    Ok(format!(
        "{} {} 0 {} 0 {} 0\n",
        size / page_size(),
        counters.WorkingSetSize as u64 / page_size(),
        text / page_size(),
        data / page_size()
    ))
}

pub(super) fn io(pid: u32) -> Result<String, i32> {
    let process = process(pid)?;
    let mut counters = IO_COUNTERS::default();
    if unsafe { GetProcessIoCounters(process.0, &mut counters) } == 0 {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    // Windows exposes transferred bytes/operations, not Linux storage-layer
    // accounting or cancelled writes. Those last three fields are synthetic.
    Ok(format!(
        "rchar: {}\nwchar: {}\nsyscr: {}\nsyscw: {}\nread_bytes: 0\nwrite_bytes: 0\ncancelled_write_bytes: 0\n",
        counters.ReadTransferCount,
        counters.WriteTransferCount,
        counters.ReadOperationCount,
        counters.WriteOperationCount
    ))
}

pub(super) fn smaps_rollup(pid: u32) -> Result<String, i32> {
    let process = process(pid)?;
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    if unsafe {
        GetProcessMemoryInfo(
            process.0,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            size_of_val(&counters) as u32,
        )
    } == 0
    {
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    let rss_kb = counters.WorkingSetSize as u64 / 1024;
    let pss_kb = rss_kb;
    let anon_kb = counters.PagefileUsage as u64 / 1024;
    let pdirty_kb = anon_kb;
    let pclean_kb = rss_kb.saturating_sub(pdirty_kb);
    let file_kb = pclean_kb;

    Ok(format!(
        "00400000-7ffffffff000 ---p 00000000 00:00 0                              [rollup]\n\
Rss:            {:8} kB\n\
Pss:            {:8} kB\n\
Pss_Dirty:             0 kB\n\
Pss_Anon:       {:8} kB\n\
Pss_File:       {:8} kB\n\
Pss_Shmem:             0 kB\n\
Shared_Clean:          0 kB\n\
Shared_Dirty:          0 kB\n\
Private_Clean:  {:8} kB\n\
Private_Dirty:  {:8} kB\n\
Referenced:     {:8} kB\n\
Anonymous:      {:8} kB\n\
KSM:                    0 kB\n\
LazyFree:               0 kB\n\
AnonHugePages:          0 kB\n\
ShmemPmdMapped:         0 kB\n\
FilePmdMapped:          0 kB\n\
Shared_Hugetlb:         0 kB\n\
Private_Hugetlb:        0 kB\n\
Swap:                  0 kB\n\
SwapPss:               0 kB\n\
Locked:                0 kB\n",
        rss_kb, pss_kb, anon_kb, file_kb, pclean_kb, pdirty_kb, rss_kb, anon_kb
    ))
}

pub(super) fn auxv(pid: u32) -> Result<Vec<u8>, i32> {
    let _process = process(pid)?;
    if pid == crate::job::process_id() {
        if let Some(published) = super::published_auxv() {
            return Ok(published);
        }
    }
    let ids = crate::credentials::process_ids(pid)?;
    let uid = ids[0] as u64;
    let euid = ids[1] as u64;
    let gid = ids[4] as u64;
    let egid = ids[5] as u64;

    let pairs: &[(u64, u64)] = &[
        (6, 4096),  // AT_PAGESZ
        (17, 100),  // AT_CLKTCK
        (4, 56),    // AT_PHENT
        (8, 0),     // AT_FLAGS
        (23, 0),    // AT_SECURE
        (11, uid),  // AT_UID
        (12, euid), // AT_EUID
        (13, gid),  // AT_GID
        (14, egid), // AT_EGID
        (16, 0),    // AT_HWCAP
        (0, 0),     // AT_NULL
    ];
    let mut bytes = Vec::with_capacity(pairs.len() * 16);
    for &(tag, val) in pairs {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    Ok(bytes)
}

pub(super) fn fdinfo(pid: u32, fd: i32) -> Result<String, i32> {
    fd_link_target(pid, fd).map_err(|e| if e == crate::EBADF { crate::ENOENT } else { e })?;
    // Cross-worker publication currently carries link names only, not OFD
    // positions/flags. Do not report the caller's descriptor with the same fd.
    if pid != crate::job::process_id() {
        return Err(crate::EACCES);
    }
    crate::ofd::with(fd, || {
        let entry = crate::get(fd)?;
        let stat = fs::fstat(fd)?;
        let f = entry.flags;
        let mut flags = if f.contains(FdFlags::PATH_ONLY) {
            fs::O_PATH
                | if f.contains(FdFlags::PROC_SYMLINK) {
                    fs::O_NOFOLLOW
                } else {
                    0
                }
        } else {
            match (
                f.contains(FdFlags::READ_ACCESS) || f.contains(FdFlags::PIPE_READ_END),
                f.contains(FdFlags::WRITE_ACCESS) || f.contains(FdFlags::PIPE_WRITE_END),
            ) {
                (true, true) => fs::O_RDWR,
                (false, true) => fs::O_WRONLY,
                (true, false) => fs::O_RDONLY,
                (false, false) => fs::O_RDWR,
            }
        };
        for (flag, linux) in [
            (FdFlags::CLOSE_ON_EXEC, fs::O_CLOEXEC),
            (FdFlags::NONBLOCK, fs::O_NONBLOCK),
            (FdFlags::APPEND, fs::O_APPEND),
            (FdFlags::NOATIME, fs::O_NOATIME),
        ] {
            if f.contains(flag) {
                flags |= linux;
            }
        }
        if entry.kind == FdKind::Fifo {
            flags = crate::fifo::status_flags(fd, entry)? | (flags & fs::O_CLOEXEC);
        }
        let target = local_fd_link_target(fd)?;
        // Anonymous inodes have no pathname mount; use zero for that synthetic
        // mount identity. Named files use the same IDs as mountinfo.
        let mount = mounts::records(pid)?
            .into_iter()
            .filter(|r| {
                target == r.target
                    || target
                        .strip_prefix(&r.target)
                        .is_some_and(|s| r.target == "/" || s.starts_with('/'))
            })
            .max_by_key(|r| r.target.len())
            .map_or(0, |r| r.id);
        let result = format!(
            "pos:\t{}\nflags:\t0{:o}\nmnt_id:\t{}\nino:\t{}\n",
            if matches!(
                entry.kind,
                FdKind::TmpfsFile
                    | FdKind::TmpfsDirectory
                    | FdKind::SysfsFile
                    | FdKind::MessageQueue
            ) {
                crate::tmpfs::position(fd)?
            } else {
                entry.offset
            },
            flags,
            mount,
            stat.st_ino
        );
        Ok(result)
    })
    .map_err(|e| if e == crate::EBADF { crate::ENOENT } else { e })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_and_io_have_linux_field_shapes() {
        let pid = crate::job::process_id();
        let values: Vec<u64> = statm(pid)
            .unwrap()
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(values.len(), 7);
        assert!(values[0] >= values[1] && values[1] > 0);
        let contents = io(pid).unwrap();
        assert_eq!(contents.lines().count(), 7);
        for line in contents.lines() {
            line.split_once(':')
                .unwrap()
                .1
                .trim()
                .parse::<u64>()
                .unwrap();
        }
    }
    #[test]
    fn fdinfo_tracks_position_flags_and_closed_descriptors() {
        let fd = fs::open("/proc/self/status", fs::O_RDONLY | fs::O_CLOEXEC, 0).unwrap();
        let pid = crate::job::process_id();
        fs::lseek(fd, 7, fs::SEEK_SET).unwrap();
        let contents = fdinfo(pid, fd).unwrap();
        assert!(contents.contains("pos:\t7\n"));
        let flags = contents
            .lines()
            .find_map(|s| s.strip_prefix("flags:\t"))
            .unwrap();
        assert_ne!(i32::from_str_radix(flags, 8).unwrap() & fs::O_CLOEXEC, 0);
        assert_eq!(fs::lseek(fd, 0, fs::SEEK_CUR), Ok(7));
        assert_eq!(
            metadata(&format!("/proc/self/fdinfo/{fd}")).unwrap().kind,
            ProcKind::File
        );
        crate::close(fd).unwrap();
        assert_eq!(
            metadata(&format!("/proc/self/fdinfo/{fd}")),
            Err(crate::ENOENT)
        );
    }
    #[test]
    fn smaps_rollup_and_auxv_have_valid_shapes() {
        let pid = crate::job::process_id();
        let rollup = smaps_rollup(pid).unwrap();
        assert!(rollup.contains("[rollup]"));
        assert!(rollup.contains("Rss:"));
        assert!(rollup.contains("Pss:"));
        let aux = auxv(pid).unwrap();
        assert!(aux.len() >= 16);
        assert_eq!(aux.len() % 16, 0);
        assert_eq!(&aux[aux.len() - 16..], &[0u8; 16]);
    }
}
