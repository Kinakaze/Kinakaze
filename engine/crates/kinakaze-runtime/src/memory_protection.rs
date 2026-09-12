//! Preserve the private-write contract when changing a retained COW view.
use core::ffi::c_void;
use windows_sys::Win32::System::Memory::{
    MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_READWRITE,
    PAGE_WRITECOPY, VirtualProtect, VirtualQuery,
};

/// AllocationProtect survives changes to individual pages (including a private
/// COW fault), unlike Protect. Never grant shared write access to a COW view.
pub fn protection_for_allocation(allocation_protection: u32, requested: u32) -> u32 {
    if matches!(
        allocation_protection & 0xff,
        PAGE_WRITECOPY | PAGE_EXECUTE_WRITECOPY
    ) {
        let base = match requested & 0xff {
            PAGE_READWRITE => PAGE_WRITECOPY,
            PAGE_EXECUTE_READWRITE => PAGE_EXECUTE_WRITECOPY,
            _ => return requested,
        };
        base | (requested & !0xff)
    } else {
        requested
    }
}

/// VirtualProtect with private-write semantics for COW allocations.
///
/// # Safety
/// Same requirements as VirtualProtect: the range must belong to one committed
/// reservation/view, and `previous` must point to writable u32 storage. A range
/// spanning distinct allocations still fails and must be split by the caller.
pub unsafe fn protect_preserving_copy_on_write(
    address: *const c_void,
    length: usize,
    requested: u32,
    previous: *mut u32,
) -> i32 {
    let mut protection = requested;
    if matches!(requested & 0xff, PAGE_READWRITE | PAGE_EXECUTE_READWRITE) {
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        if unsafe { VirtualQuery(address, &mut info, core::mem::size_of_val(&info)) } == 0 {
            return 0;
        }
        protection = protection_for_allocation(info.AllocationProtect, requested);
    }
    unsafe { VirtualProtect(address, length, protection, previous) }
}
