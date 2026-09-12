//! Host memory accounting shared by procfs and the Linux sysinfo ABI.
//!
//! Commit charge is not swap usage. Query the installed pagefiles themselves,
//! and keep wholly free pages separate from reclaimable standby pages. All
//! query state is call-local: no handles, sampling threads or cached counters
//! need to be transferred/reset by the fork coordinator.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{GetLastError, RtlNtStatusToDosError};
use windows_sys::Win32::System::ProcessStatus::{
    ENUM_PAGE_FILE_INFORMATION, K32EnumPageFilesW, K32GetPerformanceInfo, PERFORMANCE_INFORMATION,
};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(
        information_class: u32,
        information: *mut c_void,
        length: u32,
        returned: *mut u32,
    ) -> i32;
}

const SYSTEM_MEMORY_LIST_INFORMATION: u32 = 80;

// Native SIZE_T fields, in the order defined by phnt/ntexapi.h. In particular,
// the second array counts repurposing *events*, not currently available pages.
// https://github.com/winsiderss/phnt/blob/master/ntexapi.h
#[repr(C)]
#[derive(Default)]
struct MemoryLists {
    zero: usize,
    free: usize,
    modified: usize,
    modified_no_write: usize,
    bad: usize,
    standby_by_priority: [usize; 8],
    repurposed_by_priority: [usize; 8],
    modified_pagefile: usize,
}

fn memory_lists() -> Result<MemoryLists, i32> {
    let mut lists = MemoryLists::default();
    let mut returned = 0;
    // SAFETY: the buffer is correctly aligned and writable for the supplied
    // size; this information class performs a read-only query.
    let status = unsafe {
        NtQuerySystemInformation(
            SYSTEM_MEMORY_LIST_INFORMATION,
            (&raw mut lists).cast(),
            size_of::<MemoryLists>() as u32,
            &raw mut returned,
        )
    };
    if status < 0 {
        // SAFETY: the status conversion has no pointer arguments.
        return Err(crate::errno_from_win32(unsafe {
            RtlNtStatusToDosError(status)
        }));
    }
    if returned as usize != size_of::<MemoryLists>() {
        // An incompatible layout must not turn the zero-initialized tail into
        // apparently valid statistics. There is no alternate/fabricated source.
        return Err(crate::EIO);
    }
    Ok(lists)
}

impl MemoryLists {
    fn free_pages(&self) -> Result<u64, i32> {
        (self.zero as u64)
            .checked_add(self.free as u64)
            .ok_or(crate::EOVERFLOW)
    }

    fn available_pages(&self) -> Result<u64, i32> {
        self.standby_by_priority
            .iter()
            .try_fold(self.free_pages()?, |sum, &pages| {
                sum.checked_add(pages as u64).ok_or(crate::EOVERFLOW)
            })
    }
}

#[derive(Debug, Default, PartialEq)]
struct Pagefiles {
    total: u64,
    used: u64,
}

impl Pagefiles {
    fn include(&mut self, info: &ENUM_PAGE_FILE_INFORMATION) -> Result<(), i32> {
        // Microsoft documents both quantities in pages, not bytes. Do not use
        // PeakUsage or a commit-limit calculation for current swap usage.
        // https://learn.microsoft.com/windows/win32/api/psapi/ns-psapi-enum_page_file_information
        if info.cb as usize != size_of::<ENUM_PAGE_FILE_INFORMATION>()
            || info.TotalInUse > info.TotalSize
        {
            return Err(crate::EIO);
        }
        let total = self
            .total
            .checked_add(info.TotalSize as u64)
            .ok_or(crate::EOVERFLOW)?;
        let used = self
            .used
            .checked_add(info.TotalInUse as u64)
            .ok_or(crate::EOVERFLOW)?;
        *self = Self { total, used };
        Ok(())
    }
}

#[derive(Default)]
struct PagefileQuery {
    files: Pagefiles,
    error: Option<i32>,
}

unsafe extern "system" fn count_pagefile(
    context: *mut c_void,
    info: *mut ENUM_PAGE_FILE_INFORMATION,
    _filename: *const u16,
) -> i32 {
    // SAFETY: EnumPageFiles invokes this callback synchronously with our live
    // query context and one valid, callback-scoped information structure. No
    // allocations, locks or panicking arithmetic cross this foreign frame.
    let query = unsafe { &mut *context.cast::<PagefileQuery>() };
    if query.error.is_some() {
        return 0;
    }
    match query.files.include(unsafe { &*info }) {
        Ok(()) => 1,
        Err(error) => {
            query.error = Some(error);
            0
        }
    }
}

fn pagefiles() -> Result<Pagefiles, i32> {
    let mut query = PagefileQuery::default();
    // SAFETY: context remains live and exclusively borrowed through every
    // callback; the API retains no pointers after it returns.
    let success = unsafe { K32EnumPageFilesW(Some(count_pagefile), (&raw mut query).cast()) };
    if let Some(error) = query.error {
        return Err(error);
    }
    if success == 0 {
        // SAFETY: retrieve the failure from the immediately preceding API call.
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    // A successful enumeration with no callbacks is genuinely swap-disabled.
    Ok(query.files)
}

/// Current host-wide quantities in bytes, except `page_size` (bytes per page).
/// Free/available come from one page-list query; physical capacity and pagefile
/// inventory are independent observations, as with Linux's live procfs data.
#[derive(Debug, PartialEq)]
pub struct MemorySnapshot {
    pub page_size: u64,
    pub total: u64,
    pub free: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

fn assemble(
    performance: &PERFORMANCE_INFORMATION,
    lists: &MemoryLists,
    swap: &Pagefiles,
) -> Result<MemorySnapshot, i32> {
    let page_size = performance.PageSize as u64;
    if !page_size.is_power_of_two() || page_size < 1024 {
        return Err(crate::EIO);
    }
    let bytes = |pages: u64| pages.checked_mul(page_size).ok_or(crate::EOVERFLOW);
    let result = MemorySnapshot {
        page_size,
        total: bytes(performance.PhysicalTotal as u64)?,
        free: bytes(lists.free_pages()?)?,
        available: bytes(lists.available_pages()?)?,
        swap_total: bytes(swap.total)?,
        swap_free: bytes(swap.total.checked_sub(swap.used).ok_or(crate::EIO)?)?,
    };
    if result.total == 0 || result.available > result.total {
        return Err(crate::EIO);
    }
    Ok(result)
}

/// Query real physical/pagefile memory without estimating swap from commit.
pub fn snapshot() -> Result<MemorySnapshot, i32> {
    let mut performance = PERFORMANCE_INFORMATION {
        cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
        ..Default::default()
    };
    // SAFETY: a writable, correctly sized PSAPI structure.
    if unsafe { K32GetPerformanceInfo(&raw mut performance, performance.cb) } == 0 {
        // SAFETY: read the immediately preceding failure.
        return Err(crate::errno_from_win32(unsafe { GetLastError() }));
    }
    assemble(&performance, &memory_lists()?, &pagefiles()?)
}

/// Unallocated physical pages, already in the unit required by /proc/vmstat.
pub fn free_pages() -> Result<u64, i32> {
    memory_lists()?.free_pages()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(total: usize, used: usize) -> ENUM_PAGE_FILE_INFORMATION {
        ENUM_PAGE_FILE_INFORMATION {
            cb: size_of::<ENUM_PAGE_FILE_INFORMATION>() as u32,
            TotalSize: total,
            TotalInUse: used,
            PeakUsage: total,
            ..Default::default()
        }
    }

    #[test]
    fn sums_pagefiles_in_pages_and_ignores_peak_usage() {
        let mut files = Pagefiles::default();
        files.include(&file(100, 20)).unwrap();
        files.include(&file(300, 70)).unwrap();
        assert_eq!(
            files,
            Pagefiles {
                total: 400,
                used: 90
            }
        );
    }

    #[test]
    fn invalid_or_overflowing_pagefile_does_not_change_totals() {
        let mut files = Pagefiles {
            total: u64::MAX,
            used: 0,
        };
        assert_eq!(files.include(&file(1, 0)), Err(crate::EOVERFLOW));
        assert_eq!(files.include(&file(0, 1)), Err(crate::EIO));
        assert_eq!(
            files.include(&ENUM_PAGE_FILE_INFORMATION::default()),
            Err(crate::EIO)
        );
        assert_eq!(
            files,
            Pagefiles {
                total: u64::MAX,
                used: 0
            }
        );
    }

    #[test]
    fn callback_failure_is_retained_without_unwinding() {
        let mut query = PagefileQuery::default();
        let mut invalid = file(10, 11);
        // SAFETY: both callback arguments are valid, uniquely borrowed locals.
        assert_eq!(
            unsafe { count_pagefile((&raw mut query).cast(), &raw mut invalid, std::ptr::null()) },
            0
        );
        assert_eq!(query.error, Some(crate::EIO));
        assert_eq!(query.files, Pagefiles::default());
    }

    #[test]
    fn physical_free_excludes_standby_and_swap_is_not_commit() {
        let performance = PERFORMANCE_INFORMATION {
            PageSize: 4096,
            PhysicalTotal: 1000,
            // Deliberately unrelated to actual pagefile size and free lists.
            CommitLimit: 9000,
            CommitTotal: 8000,
            PhysicalAvailable: 999,
            ..Default::default()
        };
        let lists = MemoryLists {
            zero: 2,
            free: 3,
            standby_by_priority: [10; 8],
            repurposed_by_priority: [usize::MAX; 8],
            modified: 90,
            modified_pagefile: 80,
            ..Default::default()
        };
        let result = assemble(
            &performance,
            &lists,
            &Pagefiles {
                total: 200,
                used: 20,
            },
        )
        .unwrap();
        assert_eq!(
            result,
            MemorySnapshot {
                page_size: 4096,
                total: 1000 * 4096,
                free: 5 * 4096,
                available: 85 * 4096,
                swap_total: 200 * 4096,
                swap_free: 180 * 4096,
            }
        );
        let no_swap = assemble(&performance, &lists, &Pagefiles::default()).unwrap();
        assert_eq!((no_swap.swap_total, no_swap.swap_free), (0, 0));
    }

    #[test]
    fn rejects_invalid_sizes_and_page_to_byte_overflow() {
        let mut performance = PERFORMANCE_INFORMATION {
            PageSize: 4096,
            PhysicalTotal: usize::MAX,
            ..Default::default()
        };
        let lists = MemoryLists::default();
        let files = Pagefiles::default();
        assert_eq!(
            assemble(&performance, &lists, &files),
            Err(crate::EOVERFLOW)
        );
        for page_size in [0, 1, 3000] {
            performance.PageSize = page_size;
            assert_eq!(assemble(&performance, &lists, &files), Err(crate::EIO));
        }
    }

    #[test]
    fn live_snapshot_has_consistent_units_and_capacity() {
        let result = snapshot().expect("native memory queries failed");
        assert!(result.free <= result.available);
        assert!(result.available <= result.total);
        assert!(result.swap_free <= result.swap_total);
        for bytes in [
            result.total,
            result.free,
            result.available,
            result.swap_total,
            result.swap_free,
        ] {
            assert_eq!(bytes % result.page_size, 0);
        }
        eprintln!("native memory snapshot: {result:?}");
    }

    #[test]
    #[ignore = "manual timing of live host queries; not a CI latency threshold"]
    fn benchmark_live_snapshots() {
        let iterations = 1000;
        std::hint::black_box(snapshot().unwrap());
        let started = std::time::Instant::now();
        for _ in 0..iterations {
            std::hint::black_box(snapshot().unwrap());
        }
        let seconds = started.elapsed().as_secs_f64();
        eprintln!(
            "memory snapshot: {iterations} queries, {:.3} ms total, {:.3} us/query",
            seconds * 1000.0,
            seconds * 1_000_000.0 / f64::from(iterations),
        );
    }
}
