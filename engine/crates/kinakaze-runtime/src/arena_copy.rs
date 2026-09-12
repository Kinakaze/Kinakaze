//! A fresh child-owned arena with an operation-local writable staging view.
//! This is a bulk copy transport, not shared parent/child allocator state or
//! copy-on-write. The parent's original arena is never remapped or modified.

use crate::{ForkError, ForkStage};
use core::{cell::Cell, mem::size_of, ptr};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEM_COMMIT, MEM_FREE, MEM_PRIVATE, MEM_RELEASE,
    MEMORY_BASIC_INFORMATION, MapViewOfFile, MapViewOfFile3, PAGE_EXECUTE, PAGE_EXECUTE_READWRITE,
    PAGE_GUARD, PAGE_NOACCESS, PAGE_READONLY, PAGE_READWRITE, SEC_RESERVE, UnmapViewOfFile,
    VirtualAlloc, VirtualFreeEx, VirtualProtect, VirtualProtectEx, VirtualQuery, VirtualQueryEx,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

pub(crate) struct ArenaCopy {
    local: *mut u8,
    len: usize,
    process: HANDLE,
    address: usize,
    copied_len: Cell<usize>,
}

fn error(stage: ForkStage) -> ForkError {
    ForkError {
        stage,
        os_code: unsafe { GetLastError() },
    }
}

fn invalid() -> ForkError {
    ForkError {
        stage: ForkStage::ArenaMapping,
        os_code: 0,
    }
}

impl Drop for ArenaCopy {
    fn drop(&mut self) {
        if self.local.is_null() {
            return;
        }
        unsafe {
            UnmapViewOfFile(
                windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.local.cast(),
                },
            )
        };
    }
}

impl ArenaCopy {
    /// The target must be an unpublished, suspended bootstrap child. On any
    /// error its caller terminates it; an old bootstrap reservation may already
    /// have been retired. No allocation or guest callback runs here.
    pub(crate) unsafe fn prepare(
        process: HANDLE,
        address: usize,
        len: usize,
    ) -> Result<Self, ForkError> {
        if address == 0 || len == 0 || address.checked_add(len).is_none() {
            return Err(invalid());
        }
        let mut first = MEMORY_BASIC_INFORMATION::default();
        if unsafe {
            VirtualQueryEx(
                process,
                address as _,
                &mut first,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(error(ForkStage::ArenaMapping));
        }
        if first.State == MEM_FREE {
            if first.BaseAddress as usize > address
                || (first.BaseAddress as usize)
                    .checked_add(first.RegionSize)
                    .is_none_or(|end| end < address + len)
            {
                return Err(invalid());
            }
        } else {
            // Only the bootstrap's exact private arena may be replaced. Do not
            // retire an unrelated allocation merely because the range overlaps.
            let mut cursor = address;
            while cursor < address + len {
                let mut region = MEMORY_BASIC_INFORMATION::default();
                if unsafe {
                    VirtualQueryEx(
                        process,
                        cursor as _,
                        &mut region,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                } == 0
                {
                    return Err(error(ForkStage::ArenaMapping));
                }
                let end = (region.BaseAddress as usize)
                    .checked_add(region.RegionSize)
                    .ok_or_else(invalid)?;
                if region.AllocationBase as usize != address
                    || region.Type != MEM_PRIVATE
                    || end <= cursor
                    || end > address + len
                {
                    return Err(invalid());
                }
                cursor = end;
            }
            let mut after = MEMORY_BASIC_INFORMATION::default();
            if unsafe {
                VirtualQueryEx(
                    process,
                    (address + len) as _,
                    &mut after,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            } == 0
            {
                return Err(error(ForkStage::ArenaMapping));
            }
            if after.AllocationBase as usize == address {
                return Err(invalid());
            }
        }
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_EXECUTE_READWRITE | SEC_RESERVE,
                (len as u64 >> 32) as u32,
                len as u32,
                ptr::null(),
            )
        };
        if section.is_null() {
            return Err(error(ForkStage::ArenaMapping));
        }
        let local = unsafe { MapViewOfFile(section, FILE_MAP_WRITE, 0, 0, len) };
        if local.Value.is_null() {
            let failure = error(ForkStage::ArenaMapping);
            unsafe { CloseHandle(section) };
            return Err(failure);
        }
        let staging = Self {
            local: local.Value.cast(),
            len,
            process,
            address,
            copied_len: Cell::new(0),
        };
        if first.State != MEM_FREE
            && unsafe { VirtualFreeEx(process, address as _, 0, MEM_RELEASE) } == 0
        {
            let failure = error(ForkStage::ArenaMapping);
            unsafe { CloseHandle(section) };
            return Err(failure);
        }
        let remote = unsafe {
            MapViewOfFile3(
                section,
                process,
                address as _,
                0,
                len,
                0,
                PAGE_EXECUTE_READWRITE,
                ptr::null_mut(),
                0,
            )
        };
        let failure = error(ForkStage::ArenaMapping);
        // Both views retain the kernel object; no inherited native handle or
        // registration slot is needed for allocator growth or a nested fork.
        unsafe { CloseHandle(section) };
        if remote.Value as usize != address {
            return Err(failure);
        }
        Ok(staging)
    }

    /// Copy only while the source arena and sibling threads are frozen. The
    /// staging view is disjoint from the source and aliases only the new child.
    pub(crate) unsafe fn copy(
        &self,
        snapshot: &kinakaze_alloc::ArenaSnapshot,
    ) -> Result<(), ForkError> {
        if snapshot.mapped_len != self.len
            || snapshot.used_len > snapshot.committed_len
            || snapshot.committed_len > self.len
            || snapshot.used_len < kinakaze_alloc::HEADER_PAGE_SIZE
        {
            return Err(invalid());
        }
        if unsafe {
            VirtualAlloc(
                self.local.cast(),
                snapshot.committed_len,
                MEM_COMMIT,
                PAGE_READWRITE,
            )
        } != self.local.cast()
        {
            return Err(error(ForkStage::ArenaMapping));
        }
        unsafe {
            // Guest malloc shares the managed arena and may be mprotected.
            // Preserve every native protection run; only the stopped parent
            // briefly permits reads of an inaccessible run. The kernel copy
            // contains in-page faults rather than faulting through Rust memcpy.
            let mut offset = 0usize;
            while offset < snapshot.committed_len {
                let source = snapshot.base.add(offset);
                let mut region = MEMORY_BASIC_INFORMATION::default();
                if VirtualQuery(
                    source.cast(),
                    &mut region,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                ) == 0
                {
                    return Err(error(ForkStage::ArenaCopy));
                }
                let end = (region.BaseAddress as usize)
                    .checked_add(region.RegionSize)
                    .ok_or_else(invalid)?;
                if region.State != MEM_COMMIT || end <= source as usize {
                    return Err(invalid());
                }
                let length = (end - source as usize).min(snapshot.committed_len - offset);
                let bytes = length.min(snapshot.used_len.saturating_sub(offset));
                let changed = region.Protect & PAGE_GUARD != 0
                    || matches!(region.Protect & 0xff, PAGE_NOACCESS | PAGE_EXECUTE);
                let mut original = 0;
                if bytes != 0
                    && changed
                    && VirtualProtect(source.cast(), length, PAGE_READONLY, &mut original) == 0
                {
                    return Err(error(ForkStage::ArenaCopy));
                }
                let mut copied = 0;
                let ok = bytes == 0
                    || ReadProcessMemory(
                        GetCurrentProcess(),
                        source.cast(),
                        self.local.add(offset).cast(),
                        bytes,
                        &mut copied,
                    ) != 0;
                let copy_error = error(ForkStage::ArenaCopy);
                if bytes != 0 && changed {
                    let mut previous = 0;
                    if VirtualProtect(source.cast(), length, original, &mut previous) == 0 {
                        // Resuming with a changed parent protection is not an
                        // admissible failed-fork result. Do not thaw into it.
                        windows_sys::Win32::System::Threading::TerminateProcess(self.process, 127);
                        std::process::abort();
                    }
                }
                if !ok || copied != bytes {
                    return Err(copy_error);
                }
                let mut previous = 0;
                if VirtualProtectEx(
                    self.process,
                    (self.address + offset) as _,
                    length,
                    region.Protect,
                    &mut previous,
                ) == 0
                {
                    return Err(error(ForkStage::ArenaCopy));
                }
                offset += length;
            }
            self.local
                .add(kinakaze_alloc::fork_lock_offset())
                .cast::<u32>()
                .write(0);
            self.local
                .add(kinakaze_alloc::fork_mappings_lock_offset())
                .cast::<u32>()
                .write(0);
            ptr::write_bytes(
                self.local.add(kinakaze_alloc::fork_bin_locks_offset()),
                0,
                kinakaze_alloc::fork_bin_locks_len(),
            );
            for index in 0..kinakaze_alloc::fork_cache_count() {
                self.local
                    .add(kinakaze_alloc::fork_cache_lock_offset(index))
                    .cast::<u32>()
                    .write(0);
            }
        }
        self.copied_len.set(snapshot.used_len);
        Ok(())
    }

    pub(crate) unsafe fn patch_handle(
        &self,
        arena_base: usize,
        address: usize,
        value: usize,
    ) -> Result<(), ForkError> {
        let offset = address.checked_sub(arena_base).ok_or_else(invalid)?;
        if !offset.is_multiple_of(size_of::<usize>())
            || offset
                .checked_add(size_of::<usize>())
                .is_none_or(|end| end > self.copied_len.get())
        {
            return Err(invalid());
        }
        unsafe { self.local.add(offset).cast::<usize>().write(value) };
        Ok(())
    }

    /// Checked publication boundary: release the sender alias, then make any
    /// copied executable bytes visible before the child can execute them.
    pub(crate) fn finish(mut self) -> Result<(), ForkError> {
        let view = windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
            Value: self.local.cast(),
        };
        if unsafe { UnmapViewOfFile(view) } == 0 {
            return Err(error(ForkStage::ArenaCopy));
        }
        self.local = ptr::null_mut();
        if unsafe { FlushInstructionCache(self.process, self.address as _, self.copied_len.get()) }
            == 0
        {
            return Err(error(ForkStage::ArenaCopy));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        MEM_RESERVE, MEMORY_MAPPED_VIEW_ADDRESS, VirtualFree,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    #[test]
    fn private_snapshot_keeps_data_and_can_grow_after_staging_is_gone() {
        let len = 8 * 1024 * 1024;
        let used = kinakaze_alloc::HEADER_PAGE_SIZE + 65536;
        let source = vec![0xa5u8; used];
        let address = unsafe { VirtualAlloc(ptr::null(), len, MEM_RESERVE, PAGE_READWRITE) };
        assert!(!address.is_null());
        let target =
            unsafe { ArenaCopy::prepare(GetCurrentProcess(), address as usize, len) }.unwrap();
        let snapshot = kinakaze_alloc::ArenaSnapshot {
            base: source.as_ptr(),
            mapped_len: len,
            used_len: used,
            committed_len: used,
        };
        unsafe { target.copy(&snapshot) }.unwrap();
        let slot = kinakaze_alloc::HEADER_PAGE_SIZE;
        unsafe { target.patch_handle(address as usize, address as usize + slot, 0x1234) }.unwrap();
        target.finish().unwrap();
        assert_eq!(
            unsafe { address.byte_add(slot).cast::<usize>().read() },
            0x1234
        );
        assert_eq!(
            unsafe { address.byte_add(used - 1).cast::<u8>().read() },
            0xa5
        );
        assert_eq!(
            unsafe {
                address
                    .byte_add(kinakaze_alloc::fork_lock_offset())
                    .cast::<u32>()
                    .read()
            },
            0
        );
        assert!(source.iter().all(|&byte| byte == 0xa5));
        // Fresh commitment remains valid even after every section HANDLE and
        // the sender's only view have closed.
        assert_eq!(
            unsafe { VirtualAlloc(address.byte_add(used), 65536, MEM_COMMIT, PAGE_READWRITE) },
            unsafe { address.byte_add(used) }
        );
        assert_eq!(unsafe { address.byte_add(used).cast::<u8>().read() }, 0);
        unsafe { address.byte_add(used).cast::<u8>().write(0x6b) };
        assert_ne!(
            unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: address }) },
            0
        );
    }

    #[test]
    fn inaccessible_and_executable_malloc_pages_keep_data_and_protection() {
        let len = 8 * 1024 * 1024;
        let used = kinakaze_alloc::HEADER_PAGE_SIZE + 65536;
        let source =
            unsafe { VirtualAlloc(ptr::null(), len, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        let address = unsafe { VirtualAlloc(ptr::null(), len, MEM_RESERVE, PAGE_READWRITE) };
        assert!(!source.is_null() && !address.is_null());
        unsafe { ptr::write_bytes(source.cast::<u8>(), 0xa5, used) };
        let inaccessible = kinakaze_alloc::HEADER_PAGE_SIZE + 4096;
        let executable = inaccessible + 4096;
        let mut previous = 0;
        assert_ne!(
            unsafe {
                VirtualProtect(
                    source.byte_add(inaccessible),
                    4096,
                    PAGE_NOACCESS,
                    &mut previous,
                )
            },
            0
        );
        assert_ne!(
            unsafe {
                VirtualProtect(
                    source.byte_add(executable),
                    4096,
                    windows_sys::Win32::System::Memory::PAGE_EXECUTE_READ,
                    &mut previous,
                )
            },
            0
        );
        let target =
            unsafe { ArenaCopy::prepare(GetCurrentProcess(), address as usize, len) }.unwrap();
        let snapshot = kinakaze_alloc::ArenaSnapshot {
            base: source.cast(),
            mapped_len: len,
            used_len: used,
            committed_len: used,
        };
        unsafe { target.copy(&snapshot) }.unwrap();
        assert!(
            unsafe { target.patch_handle(address as usize, address as usize + used, 1) }.is_err()
        );
        target.finish().unwrap();
        for (offset, protection) in [
            (inaccessible, PAGE_NOACCESS),
            (
                executable,
                windows_sys::Win32::System::Memory::PAGE_EXECUTE_READ,
            ),
        ] {
            for base in [source, address] {
                let mut info = MEMORY_BASIC_INFORMATION::default();
                assert_ne!(
                    unsafe {
                        VirtualQuery(
                            base.byte_add(offset),
                            &mut info,
                            size_of::<MEMORY_BASIC_INFORMATION>(),
                        )
                    },
                    0
                );
                assert_eq!(info.Protect, protection);
            }
        }
        assert_ne!(
            unsafe {
                VirtualProtect(
                    address.byte_add(inaccessible),
                    4096,
                    PAGE_READWRITE,
                    &mut previous,
                )
            },
            0
        );
        assert_eq!(
            unsafe { address.byte_add(inaccessible).cast::<u8>().read() },
            0xa5
        );
        // A fork must not permanently lower malloc's future execution ceiling.
        assert_ne!(
            unsafe {
                VirtualProtect(
                    address.byte_add(executable),
                    4096,
                    PAGE_EXECUTE_READWRITE,
                    &mut previous,
                )
            },
            0
        );
        let code = [0xb8u8, 0x78, 0x56, 0x34, 0x12, 0xc3];
        unsafe {
            ptr::copy_nonoverlapping(
                code.as_ptr(),
                address.byte_add(executable).cast(),
                code.len(),
            )
        };
        assert_ne!(
            unsafe {
                FlushInstructionCache(
                    GetCurrentProcess(),
                    address.byte_add(executable),
                    code.len(),
                )
            },
            0
        );
        let function: unsafe extern "system" fn() -> u32 =
            unsafe { core::mem::transmute(address.byte_add(executable)) };
        assert_eq!(unsafe { function() }, 0x12345678);
        assert_eq!(
            unsafe { source.byte_add(executable).cast::<u8>().read() },
            0xa5
        );
        assert_ne!(unsafe { VirtualFree(source, 0, MEM_RELEASE) }, 0);
        assert_ne!(
            unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: address }) },
            0
        );
    }

    #[test]
    fn refuses_a_larger_reservation_even_when_regions_end_at_the_requested_boundary() {
        let address = unsafe { VirtualAlloc(ptr::null(), 131072, MEM_RESERVE, PAGE_READWRITE) };
        assert!(!address.is_null());
        assert_eq!(
            unsafe { VirtualAlloc(address, 65536, MEM_COMMIT, PAGE_READWRITE) },
            address
        );
        assert!(
            unsafe { ArenaCopy::prepare(GetCurrentProcess(), address as usize, 65536) }.is_err()
        );
        unsafe { address.cast::<u8>().write(0x5a) };
        assert_ne!(unsafe { VirtualFree(address, 0, MEM_RELEASE) }, 0);
    }

    #[test]
    fn refuses_to_retire_a_larger_unrelated_reservation() {
        let address = unsafe { VirtualAlloc(ptr::null(), 131072, MEM_RESERVE, PAGE_READWRITE) };
        assert!(!address.is_null());
        assert!(
            unsafe { ArenaCopy::prepare(GetCurrentProcess(), address as usize, 65536) }.is_err()
        );
        assert_ne!(unsafe { VirtualFree(address, 0, MEM_RELEASE) }, 0);
    }
}
