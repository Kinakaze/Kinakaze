//! Retained implementation handles are copied explicitly at fork, never made
//! inheritable just to carry a pointer-shaped integer into another process.

use super::{ForkError, ForkStage, begin_fork_mapping_transaction, mappings};
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(windows)]
pub(super) type RegistrationExport = unsafe extern "system" fn(*const AtomicUsize) -> i32;
// Unregistration compares the address as an opaque key; it never reads the slot.
#[cfg(windows)]
pub(super) type UnregistrationExport = extern "system" fn(*const AtomicUsize) -> i32;

pub(super) fn valid_slot_address(slot: usize) -> bool {
    slot.is_multiple_of(core::mem::align_of::<AtomicUsize>())
        && slot >= kinakaze_alloc::ARENA_BASE + kinakaze_alloc::HEADER_PAGE_SIZE
        && slot
            .checked_add(core::mem::size_of::<AtomicUsize>())
            .is_some_and(|end| end <= kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE)
}

/// Registers an owned native handle stored in a stable managed-arena slot.
///
/// The child gets a non-inheritable duplicate of the same kernel object. Its
/// copied slot is patched before any child participant or guest code resumes.
/// This is not a socket/IoRing/IOCP cloning API, nor does it copy process-owned
/// locks or a Rust owner outside the managed arena.
///
/// # Safety
/// The slot must be a live, aligned AtomicUsize in the managed arena until it is
/// unregistered. It owns a real non-inheritable handle supported by cross-process
/// DuplicateHandle. All slot mutation, final unregister/close and publication of
/// the associated mapping/owner must hold begin_fork_mapping_transaction(). The
/// child participant must adopt the copied owner, including its reference count.
pub unsafe fn register_fork_handle_slot(slot: *const AtomicUsize) -> bool {
    #[cfg(windows)]
    if let Some(register) = unsafe { (*super::PROCESS_THREAD_EXPORTS.0.get()).register_handle_slot }
    {
        return unsafe { register(slot) != 0 };
    }
    unsafe { kinakaze_runtime_register_fork_handle_slot(slot) != 0 }
}

/// Retires a registration, without closing its handle or dereferencing its slot.
/// The caller keeps the mapping transaction held until the owner is removed and
/// its handle closed, so a new fork cannot snapshot a half-retired object.
pub fn unregister_fork_handle_slot(slot: *const AtomicUsize) -> bool {
    #[cfg(windows)]
    if let Some(unregister) =
        unsafe { (*super::PROCESS_THREAD_EXPORTS.0.get()).unregister_handle_slot }
    {
        return unregister(slot) != 0;
    }
    kinakaze_runtime_unregister_fork_handle_slot(slot) != 0
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn kinakaze_runtime_register_fork_handle_slot(
    slot: *const AtomicUsize,
) -> i32 {
    if !valid_slot_address(slot as usize) {
        return 0;
    }
    let Some(_transaction) = begin_fork_mapping_transaction() else {
        return 0;
    };
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{GetHandleInformation, HANDLE_FLAG_INHERIT};
        // SAFETY: caller owns this stable slot for the registration lifetime.
        let value = unsafe { (*slot).load(Ordering::Acquire) };
        let mut flags = 0;
        if value == 0
            || value >= usize::MAX - 16
            || unsafe { GetHandleInformation(value as _, &mut flags) } == 0
            || flags & HANDLE_FLAG_INHERIT != 0
        {
            return 0;
        }
    }
    #[cfg(not(windows))]
    return 0;
    #[cfg(windows)]
    {
        let Ok(mut registry) = mappings().lock() else {
            return 0;
        };
        let address = slot as usize;
        if !registry.handle_slots.contains(&address) {
            if registry.handle_slots.try_reserve(1).is_err() {
                return 0;
            }
            registry.handle_slots.push(address);
        }
        1
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_unregister_fork_handle_slot(
    slot: *const AtomicUsize,
) -> i32 {
    if !valid_slot_address(slot as usize) {
        return 0;
    }
    let Some(_transaction) = begin_fork_mapping_transaction() else {
        return 0;
    };
    let Ok(mut registry) = mappings().lock() else {
        return 0;
    };
    let Some(index) = registry
        .handle_slots
        .iter()
        .position(|&address| address == slot as usize)
    else {
        return 0;
    };
    registry.handle_slots.swap_remove(index);
    1
}

#[cfg(all(windows, target_arch = "x86_64"))]
pub(super) struct ChildSlot {
    pub address: usize,
    pub value: usize,
}

/// The caller owns a suspended, not-yet-published child. On any failure it must
/// terminate that child; its handle table owns all duplicates already produced.
/// Keep the mapping transaction held from handoff serialization through this
/// function and the final arena copy/slot patch.
#[cfg(all(test, windows, target_arch = "x86_64"))]
pub(super) unsafe fn duplicate_into(
    process: windows_sys::Win32::Foundation::HANDLE,
) -> Result<Vec<ChildSlot>, ForkError> {
    unsafe { duplicate_into_excluding(process, &[]) }
}

/// Excludes owners that receive a fresh copied section (e.g. active stacks).
#[cfg(all(windows, target_arch = "x86_64"))]
pub(super) unsafe fn duplicate_into_excluding(
    process: windows_sys::Win32::Foundation::HANDLE,
    excluded: &[usize],
) -> Result<Vec<ChildSlot>, ForkError> {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let registry = mappings().lock().map_err(|_| ForkError {
        stage: ForkStage::GuestMapping,
        os_code: 0,
    })?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(registry.handle_slots.len())
        .map_err(|_| ForkError {
            stage: ForkStage::GuestMapping,
            os_code: 8,
        })?;
    for &address in &registry.handle_slots {
        if excluded.contains(&address) {
            continue;
        }
        // SAFETY: registration and the caller's topology transaction retain it.
        let source = unsafe { (*(address as *const AtomicUsize)).load(Ordering::Acquire) };
        let mut target = core::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                source as _,
                process,
                &mut target,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            super::LAST_NATIVE_FAILURE_LINE.store(100_000 + line!(), Ordering::Release);
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: unsafe { GetLastError() },
            });
        }
        result.push(ChildSlot {
            address,
            value: target as usize,
        });
    }
    Ok(result)
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests;
