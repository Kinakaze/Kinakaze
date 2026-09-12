//! Resident-page locks backed by Windows, with no shadow lock registry.
//!
//! Windows owns lock retirement on unmap and does not inherit these locks when
//! fork reconstructs a child address space. Queries are only made by a locking
//! operation; no sampling thread or retained page list is needed.
use core::ffi::c_void;
use kinakaze_vfs::{EAGAIN, EINVAL, EIO, ENOMEM, EOPNOTSUPP};
use windows_sys::Win32::{
    Foundation::{ERROR_NOT_LOCKED, ERROR_WORKING_SET_QUOTA, GetLastError},
    System::{
        Memory::{
            MEM_COMMIT, MEM_FREE, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualLock,
            VirtualQuery, VirtualUnlock,
        },
        ProcessStatus::K32QueryWorkingSetEx,
        SystemInformation::{GetSystemInfo, SYSTEM_INFO},
        Threading::GetCurrentProcess,
    },
};

const MCL_CURRENT: i32 = 1;
const MCL_FUTURE: i32 = 2;
const MCL_ONFAULT: i32 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Span {
    start: usize,
    end: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WorkingSetEntry {
    address: usize,
    attributes: usize,
}

fn geometry() -> (usize, usize) {
    static GEOMETRY: std::sync::OnceLock<(usize, usize)> = std::sync::OnceLock::new();
    *GEOMETRY.get_or_init(|| {
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        (
            info.dwPageSize as usize,
            info.lpMaximumApplicationAddress as usize,
        )
    })
}

fn normalize(address: usize, length: usize) -> Result<Span, i32> {
    let page = geometry().0;
    let end = address.checked_add(length).ok_or(EINVAL)?;
    let end = end.checked_add(page - 1).ok_or(EINVAL)? & !(page - 1);
    Ok(Span {
        start: address & !(page - 1),
        end,
    })
}

fn query(address: usize) -> Result<MEMORY_BASIC_INFORMATION, i32> {
    let mut region = MEMORY_BASIC_INFORMATION::default();
    if unsafe { VirtualQuery(address as _, &mut region, core::mem::size_of_val(&region)) } == 0 {
        return Err(ENOMEM);
    }
    Ok(region)
}

fn accessible(region: &MEMORY_BASIC_INFORMATION) -> bool {
    region.State == MEM_COMMIT && region.Protect & (PAGE_GUARD | PAGE_NOACCESS) == 0
}

fn spans(request: Span, require_access: bool) -> Result<Vec<Span>, i32> {
    let mut result = Vec::new();
    let mut cursor = request.start;
    while cursor < request.end {
        let region = query(cursor)?;
        if region.State == MEM_FREE || (require_access && !accessible(&region)) {
            return Err(ENOMEM);
        }
        let end = (region.BaseAddress as usize)
            .checked_add(region.RegionSize)
            .ok_or(ENOMEM)?
            .min(request.end);
        if end <= cursor {
            return Err(ENOMEM);
        }
        // Reserved pages cannot be locked; munlock of a PROT_NONE reservation
        // has nothing to release and must not populate it.
        if region.State == MEM_COMMIT {
            result.push(Span { start: cursor, end });
        }
        cursor = end;
    }
    Ok(result)
}

fn current_spans(require_access: bool) -> Result<Vec<Span>, i32> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while cursor <= geometry().1 {
        let region = query(cursor)?;
        let end = (region.BaseAddress as usize)
            .checked_add(region.RegionSize)
            .ok_or(ENOMEM)?;
        if end <= cursor {
            return Err(EIO);
        }
        // A page can retain its lock after mprotect(PROT_NONE). Unlocking
        // must inspect every committed region, without touching its contents.
        if region.State == MEM_COMMIT && (!require_access || accessible(&region)) {
            result.push(Span { start: cursor, end });
        }
        cursor = end;
    }
    Ok(result)
}

/// PSAPI_WORKING_SET_EX_BLOCK: Valid is bit 0 and Locked is bit 22.
/// Consult Locked only in the valid layout; a locked page is resident.
fn selected_runs(span: Span, locked: bool) -> Result<Vec<Span>, i32> {
    let page = geometry().0;
    let mut entries = [WorkingSetEntry::default(); 256];
    let mut cursor = span.start;
    let mut runs: Vec<Span> = Vec::new();
    while cursor < span.end {
        let count = ((span.end - cursor) / page).min(entries.len());
        for (index, entry) in entries[..count].iter_mut().enumerate() {
            *entry = WorkingSetEntry {
                address: cursor + index * page,
                attributes: 0,
            };
        }
        if unsafe {
            K32QueryWorkingSetEx(
                GetCurrentProcess(),
                entries.as_mut_ptr().cast(),
                (count * core::mem::size_of::<WorkingSetEntry>()) as u32,
            )
        } == 0
        {
            return Err(EIO);
        }
        for entry in &entries[..count] {
            let is_locked = entry.attributes & 1 != 0 && entry.attributes & (1 << 22) != 0;
            if is_locked == locked {
                if let Some(last) = runs.last_mut()
                    && last.end == entry.address
                {
                    last.end += page;
                } else {
                    runs.push(Span {
                        start: entry.address,
                        end: entry.address + page,
                    });
                }
            }
        }
        cursor += count * page;
    }
    Ok(runs)
}

fn unlock_spans(spans: &[Span]) -> Result<(), i32> {
    for &span in spans {
        for run in selected_runs(span, true)? {
            if unsafe { VirtualUnlock(run.start as *const c_void, run.end - run.start) } == 0 {
                let error = unsafe { GetLastError() };
                if error != ERROR_NOT_LOCKED {
                    return Err(kinakaze_vfs::errno_from_win32(error));
                }
            }
        }
    }
    Ok(())
}

fn lock_spans(spans: &[Span]) -> Result<(), i32> {
    let mut added = Vec::new();
    let result = (|| {
        for &span in spans {
            // Keep pre-existing locks out of rollback. Native locks, like
            // Linux mlock, are not a reference count: one munlock releases one
            // page even if that page was locked repeatedly.
            added.extend(selected_runs(span, false)?);
            if unsafe { VirtualLock(span.start as *const c_void, span.end - span.start) } == 0 {
                let error = unsafe { GetLastError() };
                return Err(if error == ERROR_WORKING_SET_QUOTA {
                    ENOMEM
                } else {
                    kinakaze_vfs::errno_from_win32(error)
                });
            }
        }
        Ok(())
    })();
    if result.is_err() {
        // A failed native request can cover partially locked pages. Query the
        // added spans again so rollback never calls VirtualUnlock on an
        // unlocked page (which would evict it from the working set).
        unlock_spans(&added)?;
    }
    result
}

pub fn lock(address: usize, length: usize) -> Result<(), i32> {
    let range = normalize(address, length)?;
    if range.start == range.end {
        return Ok(());
    }
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(EAGAIN)?;
    lock_spans(&spans(range, true)?)
}

pub fn unlock(address: usize, length: usize) -> Result<(), i32> {
    let range = normalize(address, length)?;
    if range.start == range.end {
        return Ok(());
    }
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(EAGAIN)?;
    unlock_spans(&spans(range, false)?)
}

pub fn lock_all(flags: i32) -> Result<(), i32> {
    if flags & !(MCL_CURRENT | MCL_FUTURE | MCL_ONFAULT) != 0
        || flags & (MCL_CURRENT | MCL_FUTURE) == 0
    {
        return Err(EINVAL);
    }
    // Windows has no process-wide future/on-fault lock policy. These flags
    // need hooks in every guest allocation/commit and fault path before they
    // can truthfully succeed. Never downgrade such a request to CURRENT.
    if flags & (MCL_FUTURE | MCL_ONFAULT) != 0 {
        return Err(EOPNOTSUPP);
    }
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(EAGAIN)?;
    lock_spans(&current_spans(true)?)
}

pub fn unlock_all() -> Result<(), i32> {
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(EAGAIN)?;
    unlock_spans(&current_spans(false)?)
}

fn posix(result: Result<(), i32>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mlock(address: *const c_void, length: usize) -> i32 {
    posix(lock(address as usize, length))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_munlock(address: *const c_void, length: usize) -> i32 {
    posix(unlock(address as usize, length))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mlockall(flags: i32) -> i32 {
    posix(lock_all(flags))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_munlockall() -> i32 {
    posix(unlock_all())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mlock2(
    address: *const c_void,
    length: usize,
    flags: u32,
) -> i32 {
    posix(match flags {
        0 => lock(address as usize, length),
        1 => Err(EOPNOTSUPP),
        _ => Err(EINVAL),
    })
}

#[cfg(test)]
mod tests;
