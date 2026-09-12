//! Native stack preparation for unprobed Linux ELF frames.
//!
//! If RSP skips Windows' guard into MEM_RESERVE, Windows can fail to construct
//! the exception frame before VEH is entered. Commit the usable reservation
//! before guest execution, without touching its demand-zero physical pages.
use core::ffi::c_void;
use std::cell::Cell;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_DECOMMIT, MEM_PRIVATE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_GUARD,
    PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect, VirtualQuery,
};

// Native Windows x64 stack pages are 4 KiB. The allocation's bottom page stays
// inaccessible; the complete native guard span is retained immediately above.
const PAGE: usize = 4096;

thread_local! {
    static REGISTERED: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
}

fn query(address: usize) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut region = MEMORY_BASIC_INFORMATION::default();
    (unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut region,
            core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } != 0)
        .then_some(region)
}

fn commit(base: usize, limit: usize) -> Option<usize> {
    if limit == 0 || limit >= base {
        return None;
    }
    let top = query(base - 1)?;
    if top.State != MEM_COMMIT || top.Type != MEM_PRIVATE {
        return None;
    }
    let allocation = top.AllocationBase as usize;
    // Query from the allocation's bottom, not limit-1: VirtualQuery only
    // describes the region at and above its argument and would truncate a
    // multi-page guard when queried from that guard's last page.
    let reserved = query(allocation)?;
    if reserved.State != MEM_RESERVE {
        return None;
    }
    let old_guard = allocation.checked_add(reserved.RegionSize)?;
    let guard = query(old_guard)?;
    if guard.AllocationBase != top.AllocationBase
        || guard.State != MEM_COMMIT
        || guard.Protect != PAGE_READWRITE | PAGE_GUARD
        || old_guard.checked_add(guard.RegionSize) != Some(limit)
    {
        return None;
    }
    let new_guard = allocation.checked_add(PAGE)?;
    if new_guard > old_guard {
        return None;
    }
    if new_guard == old_guard {
        return Some(limit);
    }
    let new_limit = new_guard.checked_add(guard.RegionSize)?;
    if new_limit > old_guard {
        return None;
    }
    let address = new_guard as *const c_void;
    let length = old_guard - new_guard;
    if unsafe { VirtualAlloc(address, length, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
        return None;
    }
    let mut protection = 0;
    let ready = unsafe {
        VirtualProtect(
            address,
            guard.RegionSize,
            PAGE_READWRITE | PAGE_GUARD,
            &mut protection,
        ) != 0
            && VirtualProtect(
                old_guard as *const c_void,
                guard.RegionSize,
                PAGE_READWRITE,
                &mut protection,
            ) != 0
    };
    if !ready {
        unsafe { VirtualFree(address as *mut c_void, length, MEM_DECOMMIT) };
        return None;
    }
    Some(new_limit)
}

/// Called once by a newly created native thread, before any guest code or
/// private-stack transition. Windows continues to own and free the reservation.
pub(super) fn prepare_current() -> bool {
    let limit: usize;
    let base: usize;
    // GetCurrentThreadStackLimits returns the reservation's low bound on
    // Windows, not the current committed StackLimit needed to locate its guard.
    unsafe {
        core::arch::asm!(
            "mov {}, gs:[0x08]",
            "mov {}, gs:[0x10]",
            out(reg) base,
            out(reg) limit,
            options(nostack, preserves_flags, readonly),
        );
    }
    let Some(limit) = commit(base, limit) else {
        return false;
    };
    // Match the committed bounds used by Windows unwinding and by fork's
    // native-stack snapshot. Later ABI transitions save and restore this value.
    unsafe {
        core::arch::asm!("mov gs:[0x10], {}", in(reg) limit, options(nostack, preserves_flags));
    }
    let Some(region) = query(base - 1) else {
        return false;
    };
    let allocation = region.AllocationBase as usize;
    // Guest routines execute on this Windows-owned stack. Linux fork preserves
    // every thread's stack memory even though only the calling thread survives.
    // Locks and other objects on sibling stacks must remain addressable while
    // child handlers repair their state. Windows still owns the parent's stack;
    // use an ordinary private copy, never replace its backing with a section.
    let registered = kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
        base: allocation,
        len: base - allocation,
        behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
        storage: kinakaze_runtime::ForkMappingStorage::Ordinary,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: 0,
        domain: kinakaze_runtime::ForkMappingDomain::HostPrivate,
    });
    if registered {
        REGISTERED.with(|slot| slot.set((allocation, base)));
    }
    registered
}

/// Remove the registration before Windows releases an exiting thread's stack.
/// The returned range also retires internal metadata for objects on that stack.
pub(super) fn retire_current() -> Option<(usize, usize)> {
    let (base, end) = REGISTERED.try_with(|slot| slot.replace((0, 0))).ok()?;
    if base == 0 {
        return None;
    }
    kinakaze_runtime::unregister_fork_mapping(base);
    Some((base, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{MEM_RELEASE, PAGE_NOACCESS};

    #[test]
    fn commits_usable_stack_and_preserves_full_guard_and_bottom_page() {
        for guard_size in [PAGE, 3 * PAGE] {
            let allocation =
                unsafe { VirtualAlloc(core::ptr::null(), 1024 * 1024, MEM_RESERVE, PAGE_NOACCESS) };
            assert!(!allocation.is_null());
            let base = allocation as usize + 1024 * 1024;
            let old_limit = base - 2 * PAGE;
            let old_guard = old_limit - guard_size;
            assert!(
                !unsafe {
                    VirtualAlloc(
                        old_guard as _,
                        guard_size + 2 * PAGE,
                        MEM_COMMIT,
                        PAGE_READWRITE,
                    )
                }
                .is_null()
            );
            let mut old = 0;
            assert_ne!(
                unsafe {
                    VirtualProtect(
                        old_guard as _,
                        guard_size,
                        PAGE_READWRITE | PAGE_GUARD,
                        &mut old,
                    )
                },
                0
            );
            let limit = commit(base, old_limit).unwrap();
            assert_eq!(limit, allocation as usize + PAGE + guard_size);
            let bottom = query(allocation as usize).unwrap();
            assert_eq!(bottom.State, MEM_RESERVE);
            assert_eq!(bottom.RegionSize, PAGE);
            let guard = query(allocation as usize + PAGE).unwrap();
            assert_eq!(guard.Protect, PAGE_READWRITE | PAGE_GUARD);
            assert_eq!(guard.RegionSize, guard_size);
            let usable = query(limit).unwrap();
            assert_eq!(usable.State, MEM_COMMIT);
            assert_eq!(usable.Protect, PAGE_READWRITE);
            assert_eq!(usable.RegionSize, base - limit);
            assert_eq!(commit(base, limit), Some(limit));
            assert_ne!(unsafe { VirtualFree(allocation, 0, MEM_RELEASE) }, 0);
        }
    }

    #[test]
    fn rejects_unrelated_mappings_without_changing_protection() {
        let allocation = unsafe {
            VirtualAlloc(
                core::ptr::null(),
                4 * PAGE,
                MEM_RESERVE | MEM_COMMIT,
                PAGE_NOACCESS,
            )
        };
        assert!(!allocation.is_null());
        let base = allocation as usize;
        assert_eq!(commit(base + 4 * PAGE, base + 2 * PAGE), None);
        assert_eq!(query(base).unwrap().Protect, PAGE_NOACCESS);
        assert_eq!(commit(0, 0), None);
        assert_ne!(unsafe { VirtualFree(allocation, 0, MEM_RELEASE) }, 0);
    }
}
