//! Freeze a private anonymous section at its first fork, without copying bytes.
//! The caller pins topology and parks sibling writers. Active stacks use the
//! ordinary copied-section path; they must never be temporarily unmapped here.
use super::{ForkError, ForkMapping, ForkStage};
use core::{
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr,
};
use windows_sys::Win32::{
    Foundation::GetLastError,
    System::{
        Diagnostics::Debug::FlushInstructionCache,
        Memory::{
            MEM_COMMIT, MEM_MAPPED, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER,
            MEM_RESERVE, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3,
            PAGE_READWRITE, UnmapViewOfFile2, VirtualAlloc, VirtualFree, VirtualProtect,
            VirtualQuery,
        },
        Threading::GetCurrentProcess,
    },
};

#[derive(Clone, Copy)]
struct Run {
    base: usize,
    len: usize,
    protection: u32,
}
struct Scratch(*mut Run);
impl Drop for Scratch {
    fn drop(&mut self) {
        unsafe {
            VirtualFree(self.0.cast(), 0, MEM_RELEASE);
        }
    }
}
fn error(code: u32) -> ForkError {
    ForkError {
        stage: ForkStage::GuestMapping,
        os_code: code,
    }
}
unsafe fn query(address: usize) -> Result<MEMORY_BASIC_INFORMATION, ForkError> {
    let mut info = unsafe { zeroed() };
    if unsafe {
        VirtualQuery(
            address as _,
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } == 0
    {
        Err(error(unsafe { GetLastError() }))
    } else {
        Ok(info)
    }
}
unsafe fn protect(runs: &[Run], allocation: u32) -> bool {
    for run in runs {
        let mut old = 0;
        let protection =
            super::memory_protection::protection_for_allocation(allocation, run.protection);
        if unsafe { VirtualProtect(run.base as _, run.len, protection, &mut old) } == 0 {
            return false;
        }
    }
    true
}

/// Returns true only when this call converted a previously writable section.
/// A fresh conversion has no private pages; later forks must inspect COW state.
/// Uses native scratch memory only, even with the managed allocator frozen.
pub(super) unsafe fn snapshot(
    mapping: &ForkMapping,
    section: *mut c_void,
) -> Result<bool, ForkError> {
    let end = mapping.base.checked_add(mapping.len).ok_or(error(87))?;
    let stack_marker = 0u8;
    let stack = ptr::addr_of!(stack_marker) as usize;
    if (mapping.base..end).contains(&stack) {
        return Err(error(87));
    }
    let first = unsafe { query(mapping.base)? };
    if matches!(first.AllocationProtect & 0xff, 0x08 | 0x80) {
        return Ok(false);
    }
    if !matches!(first.AllocationProtect & 0xff, 0x02 | 0x04 | 0x20 | 0x40) {
        return Err(error(87));
    }
    // A partial view cannot be unmapped independently. Provider splits first
    // materialize it into a conventional copied backing, outside this path.
    let mut cursor = mapping.base;
    let mut count = 0usize;
    while cursor < end {
        let info = unsafe { query(cursor)? };
        if info.AllocationBase as usize != mapping.base
            || info.State != MEM_COMMIT
            || info.Type != MEM_MAPPED
        {
            return Err(error(87));
        }
        let next = (info.BaseAddress as usize)
            .checked_add(info.RegionSize)
            .ok_or(error(87))?;
        if next <= cursor || next > end {
            return Err(error(87));
        }
        count += 1;
        cursor = next;
    }
    if unsafe { query(end)? }.AllocationBase as usize == mapping.base {
        return Err(error(87));
    }
    let bytes = count.checked_mul(size_of::<Run>()).ok_or(error(8))?;
    let scratch = Scratch(
        unsafe { VirtualAlloc(ptr::null(), bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) }
            .cast(),
    );
    if scratch.0.is_null() {
        return Err(error(unsafe { GetLastError() }));
    }
    cursor = mapping.base;
    for index in 0..count {
        let info = unsafe { query(cursor)? };
        let len = info.RegionSize;
        unsafe {
            scratch.0.add(index).write(Run {
                base: cursor,
                len,
                protection: info.Protect,
            });
        }
        cursor += len;
    }
    let runs = unsafe { core::slice::from_raw_parts(scratch.0, count) };
    let process = unsafe { GetCurrentProcess() };
    let old = MEMORY_MAPPED_VIEW_ADDRESS {
        Value: mapping.base as _,
    };
    if unsafe { UnmapViewOfFile2(process, old, MEM_PRESERVE_PLACEHOLDER) } == 0 {
        return Err(error(unsafe { GetLastError() }));
    }
    let view = unsafe {
        MapViewOfFile3(
            section,
            process,
            mapping.base as _,
            mapping.backing_offset,
            mapping.len,
            MEM_REPLACE_PLACEHOLDER,
            mapping.view_protection,
            ptr::null_mut(),
            0,
        )
    };
    let installed = !view.Value.is_null();
    if installed
        && unsafe { protect(runs, mapping.view_protection) }
        && unsafe { FlushInstructionCache(process, mapping.base as _, mapping.len) } != 0
    {
        return Ok(true);
    }
    let failure = error(unsafe { GetLastError() });
    // No guest writer has run since unmap, so the original section still owns
    // every byte. Restore it and its permissions before any failure can return.
    if installed && unsafe { UnmapViewOfFile2(process, view, MEM_PRESERVE_PLACEHOLDER) } == 0 {
        std::process::abort();
    }
    let restored = unsafe {
        MapViewOfFile3(
            section,
            process,
            mapping.base as _,
            mapping.backing_offset,
            mapping.len,
            MEM_REPLACE_PLACEHOLDER,
            first.AllocationProtect,
            ptr::null_mut(),
            0,
        )
    };
    if restored.Value as usize != mapping.base
        || !unsafe { protect(runs, first.AllocationProtect) }
        || unsafe { FlushInstructionCache(process, mapping.base as _, mapping.len) } == 0
    {
        std::process::abort();
    }
    Err(failure)
}

/// Readability is temporary, process-local, and restored on every return.
/// The topology transaction and parked siblings cover the whole operation.
pub(super) unsafe fn copy_private_pages(
    source: usize,
    length: usize,
    protection: u32,
    copy: impl FnMut(usize, usize) -> Result<(), ForkError>,
) -> Result<super::cow::CopyStats, ForkError> {
    let changed = protection & 0x100 != 0 || matches!(protection & 0xff, 0x01 | 0x10);
    let mut previous = 0;
    if changed && unsafe { VirtualProtect(source as _, length, 0x02, &mut previous) } == 0 {
        return Err(error(unsafe { GetLastError() }));
    }
    let result = unsafe { super::cow::copy_readable_private_pages(source, length, copy) };
    if changed {
        let mut ignored = 0;
        if unsafe {
            VirtualProtect(
                source as _,
                length,
                super::memory_protection::protection_for_allocation(0x80, previous),
                &mut ignored,
            )
        } == 0
        {
            std::process::abort();
        }
    }
    result
}

#[cfg(test)]
mod tests;
