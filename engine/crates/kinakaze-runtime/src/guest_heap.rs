//! Section-backed C heap. Native handles and their registry stay host-private.
use crate::{
    ForkError, ForkMapping, ForkMappingBehavior, ForkMappingDomain, ForkMappingStorage, ForkStage,
};
use core::{
    alloc::{GlobalAlloc, Layout},
    mem::size_of,
    ptr,
};
use kinakaze_alloc::guest::{ARENA_BASE, MAX_COMMIT_BYTES};
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE},
    System::{Diagnostics::Debug::WriteProcessMemory, Memory::*, Threading::GetCurrentProcess},
};
const MEM_COALESCE_PLACEHOLDERS: u32 = 0x1;

/// Called by the allocator under its topology transaction, before publishing
/// the new committed boundary. Successful chunks, slots and section handles
/// live until process exit, exactly as the allocator's committed high water.
/// No owner pointer or process-local handle is stored in the guest heap.
///
/// # Safety
/// Only the guest allocator may commit its unallocated, reserved suffix.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn kinakaze_runtime_commit_guest_heap(start: usize, len: usize) -> i32 {
    if len == 0
        || len > MAX_COMMIT_BYTES
        || !start.is_multiple_of(0x10000)
        || !len.is_multiple_of(0x10000)
        || start < ARENA_BASE
        || start
            .checked_add(len)
            .is_none_or(|end| end > ARENA_BASE + kinakaze_alloc::ARENA_SIZE)
    {
        return 0;
    }
    let Some(_transaction) = crate::begin_fork_mapping_transaction() else {
        return 0;
    };
    (unsafe { commit(start, len) }) as i32
}

fn placeholder(base: usize, len: usize) -> ForkMapping {
    ForkMapping {
        base,
        len,
        behavior: ForkMappingBehavior::Copy,
        storage: ForkMappingStorage::Placeholder,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: 0,
        domain: ForkMappingDomain::GuestMm,
    }
}

unsafe fn commit(start: usize, len: usize) -> bool {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
    if unsafe { VirtualQuery(start as _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
        return false;
    }
    if info.State == MEM_FREE && start == ARENA_BASE {
        let reserved = unsafe {
            VirtualAlloc2(
                GetCurrentProcess(),
                ARENA_BASE as _,
                kinakaze_alloc::ARENA_SIZE,
                MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                PAGE_NOACCESS,
                ptr::null_mut(),
                0,
            )
        };
        if reserved as usize != ARENA_BASE {
            return false;
        }
        if !crate::register_fork_mapping(placeholder(ARENA_BASE, kinakaze_alloc::ARENA_SIZE)) {
            unsafe {
                VirtualFree(reserved, 0, MEM_RELEASE);
            }
            return false;
        }
        if unsafe { VirtualQuery(start as _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) }
            == 0
        {
            return false;
        }
    }
    if info.State != MEM_RESERVE
        || info.BaseAddress as usize > start
        || (info.BaseAddress as usize)
            .checked_add(info.RegionSize)
            .is_none_or(|end| end < start + len)
    {
        return false;
    }
    let slot = unsafe { kinakaze_alloc::ManagedAllocator.alloc(Layout::new::<AtomicUsize>()) }
        .cast::<AtomicUsize>();
    if slot.is_null() {
        return false;
    }
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_EXECUTE_READWRITE,
            0,
            len as u32,
            ptr::null(),
        )
    };
    if handle.is_null() {
        unsafe {
            kinakaze_alloc::ManagedAllocator.dealloc(slot.cast(), Layout::new::<AtomicUsize>());
        }
        return false;
    }
    unsafe {
        slot.write(AtomicUsize::new(handle as usize));
    }
    let mut pending = PendingChunk {
        start,
        slot,
        registered: false,
        mapped: false,
        placeholder_base: info.BaseAddress as usize,
        placeholder_len: info.RegionSize,
        carved: false,
    };
    if !unsafe { crate::register_fork_handle_slot(slot) } {
        return false;
    }
    pending.registered = true;
    // A failed map restores the original placeholder extent, so a later larger
    // allocation can retry instead of being trapped behind a stale split.
    pending.carved = true;
    if unsafe { crate::windows::carve_remote_placeholder(GetCurrentProcess(), start, len) }.is_err()
    {
        return false;
    }
    let view = unsafe {
        MapViewOfFile3(
            handle,
            GetCurrentProcess(),
            start as _,
            0,
            len,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_EXECUTE_READWRITE,
            ptr::null_mut(),
            0,
        )
    }
    .Value;
    if view as usize != start {
        return false;
    }
    pending.mapped = true;
    let mut old = 0;
    if unsafe { VirtualProtect(view, len, PAGE_READWRITE, &mut old) } == 0 {
        return false;
    }
    if !crate::register_fork_mapping(ForkMapping {
        base: start,
        len,
        behavior: ForkMappingBehavior::Copy,
        storage: ForkMappingStorage::AnonymousSnapshot,
        backing_slot: slot as usize,
        backing_offset: 0,
        view_protection: PAGE_EXECUTE_WRITECOPY,
        domain: ForkMappingDomain::GuestMm,
    }) {
        return false;
    }
    // Intentionally process-lifetime storage; there is no heap trimming yet.
    core::mem::forget(pending);
    true
}

struct PendingChunk {
    start: usize,
    slot: *mut AtomicUsize,
    registered: bool,
    mapped: bool,
    placeholder_base: usize,
    placeholder_len: usize,
    carved: bool,
}
impl Drop for PendingChunk {
    fn drop(&mut self) {
        unsafe {
            if self.mapped {
                // Failure here would leave an unregistered writable view. Do
                // not continue with topology that fork cannot reproduce.
                if UnmapViewOfFile2(
                    GetCurrentProcess(),
                    MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.start as _,
                    },
                    MEM_PRESERVE_PLACEHOLDER,
                ) == 0
                {
                    std::process::abort();
                }
            }
            if self.carved {
                VirtualFree(
                    self.placeholder_base as _,
                    self.placeholder_len,
                    MEM_RELEASE | MEM_COALESCE_PLACEHOLDERS,
                );
            }
            if self.registered {
                crate::unregister_fork_handle_slot(self.slot);
            }
            CloseHandle((*self.slot).load(Ordering::Relaxed) as _);
            kinakaze_alloc::ManagedAllocator
                .dealloc(self.slot.cast(), Layout::new::<AtomicUsize>());
        }
    }
}

/// The parent holds the guest allocator locks while taking the snapshot. Patch
/// only the child's metadata before participants or guest code can allocate.
/// Native scratch avoids acquiring any frozen host allocator lock.
pub(super) unsafe fn repair_child_locks(process: HANDLE) -> Result<usize, ForkError> {
    let bytes = kinakaze_alloc::guest::HEADER_BYTES;
    let scratch =
        unsafe { VirtualAlloc(ptr::null(), bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
    if scratch.is_null() {
        return Err(error());
    }
    unsafe {
        ptr::copy_nonoverlapping(ARENA_BASE as *const u8, scratch.cast::<u8>(), bytes);
        kinakaze_alloc::guest::clear_copied_locks(scratch.cast());
    }
    let mut written = 0;
    let success =
        unsafe { WriteProcessMemory(process, ARENA_BASE as _, scratch, bytes, &mut written) };
    let failure = error();
    unsafe {
        VirtualFree(scratch, 0, MEM_RELEASE);
    }
    if success == 0 || written != bytes {
        Err(failure)
    } else {
        Ok(bytes)
    }
}
fn error() -> ForkError {
    ForkError {
        stage: ForkStage::GuestMapping,
        os_code: unsafe { GetLastError() },
    }
}

extern "C" fn install_allocator_backend() {
    kinakaze_alloc::guest::install_backend(kinakaze_runtime_commit_guest_heap);
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static ALLOCATOR_BACKEND: extern "C" fn() = install_allocator_backend;
