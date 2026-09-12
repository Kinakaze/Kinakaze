//! Initialize the ELF backing before publishing a private executable view.
use super::{LinkError, ProgramHeader};

pub(crate) struct ImageMapping {
    pub base: *mut u8,
    #[cfg(windows)]
    slot: *mut std::sync::atomic::AtomicUsize,
}

#[cfg(windows)]
impl ImageMapping {
    pub(crate) fn fork_slot(&self) -> usize {
        self.slot as usize
    }

    /// The caller transfers the coordinator-restored view exactly once.
    pub(crate) unsafe fn adopt_fork(
        base: usize,
        len: usize,
        slot: usize,
    ) -> Result<Self, LinkError> {
        let valid = kinakaze_alloc::shared_mapping(base).is_some_and(|mapping| {
            mapping.base == base
                && mapping.len == len
                && mapping.backing_slot == slot
                && mapping.storage
                    == kinakaze_runtime::ForkMappingStorage::CopyOnWriteSection as u32
        });
        if !valid {
            return Err(LinkError::InvalidProvider(
                "ELF fork view is not registered".into(),
            ));
        }
        Ok(Self {
            base: base as _,
            slot: slot as _,
        })
    }

    pub fn new(
        fixed: bool,
        start: u64,
        len: usize,
        display: &str,
        bytes: &crate::ImmutableBytes,
        headers: &[ProgramHeader],
    ) -> Result<Self, LinkError> {
        if let Some(mapping) = Self::from_snapshot(fixed, start, len, display, bytes, headers)? {
            return Ok(mapping);
        }
        use core::alloc::{GlobalAlloc, Layout};
        use std::sync::atomic::AtomicUsize;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Memory::{
                CreateFileMappingW, FILE_MAP_COPY, FILE_MAP_EXECUTE, FILE_MAP_WRITE, MapViewOfFile,
                MapViewOfFileEx, PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, UnmapViewOfFile,
            },
        };
        let failure = || LinkError::MappingFailed {
            object: display.to_owned(),
            len,
        };
        let registration = || LinkError::MappingRegistrationFailed {
            object: display.to_owned(),
            len,
        };
        let creation = crate::linker::profile::begin("section-create", display);
        let _transaction =
            kinakaze_runtime::begin_fork_mapping_transaction().ok_or_else(registration)?;
        // Explicit managed allocation: a provider DLL need not use the loader's
        // global allocator. Fork patches this stable slot before restoring ELF.
        let slot = unsafe { kinakaze_alloc::ManagedAllocator.alloc(Layout::new::<AtomicUsize>()) }
            .cast::<AtomicUsize>();
        if slot.is_null() {
            return Err(failure());
        }
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_EXECUTE_READWRITE,
                ((len as u64) >> 32) as u32,
                len as u32,
                core::ptr::null(),
            )
        };
        if handle.is_null() {
            unsafe {
                kinakaze_alloc::ManagedAllocator.dealloc(slot.cast(), Layout::new::<AtomicUsize>())
            };
            return Err(failure());
        }
        unsafe { slot.write(AtomicUsize::new(handle as usize)) };
        if !unsafe { kinakaze_runtime::register_fork_handle_slot(slot) } {
            unsafe {
                CloseHandle(handle);
                kinakaze_alloc::ManagedAllocator.dealloc(slot.cast(), Layout::new::<AtomicUsize>());
            }
            return Err(registration());
        }
        let mut owned = Self {
            base: core::ptr::null_mut(),
            slot,
        };
        drop(creation);
        let copying = crate::linker::profile::begin("section-copy", display);
        // This writable alias is never published. Its bytes are immutable once
        // the private view can be reached by relocation, patching or guest code.
        let staging = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, len) };
        if staging.Value.is_null() {
            return Err(failure());
        }
        let initialized = super::copy_segments(bytes, headers, staging.Value as usize, start, len);
        unsafe { UnmapViewOfFile(staging) };
        initialized?;
        drop(copying);
        let _publication = crate::linker::profile::begin("section-publish", display);
        let preferred = if fixed {
            start as *const core::ffi::c_void
        } else {
            core::ptr::null()
        };
        let view = unsafe {
            MapViewOfFileEx(
                handle,
                FILE_MAP_COPY | FILE_MAP_EXECUTE,
                0,
                0,
                len,
                preferred,
            )
        };
        if view.Value.is_null() {
            return Err(failure());
        }
        owned.base = view.Value.cast();
        if fixed && owned.base as u64 != start {
            return Err(failure());
        }
        if !kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
            base: owned.base as usize,
            len,
            behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
            storage: kinakaze_runtime::ForkMappingStorage::CopyOnWriteSection,
            backing_slot: slot as usize,
            backing_offset: 0,
            view_protection: PAGE_EXECUTE_WRITECOPY,
            domain: kinakaze_runtime::ForkMappingDomain::GuestMm,
        }) {
            return Err(registration());
        }
        Ok(owned)
    }

    fn from_snapshot(
        fixed: bool,
        start: u64,
        len: usize,
        display: &str,
        bytes: &crate::ImmutableBytes,
        headers: &[ProgramHeader],
    ) -> Result<Option<Self>, LinkError> {
        use core::alloc::{GlobalAlloc, Layout};
        use std::os::windows::io::{AsRawHandle, IntoRawHandle};
        use std::sync::atomic::AtomicUsize;
        use windows_sys::Win32::System::Memory::{
            FILE_MAP_COPY, FILE_MAP_EXECUTE, MapViewOfFileEx, PAGE_EXECUTE_WRITECOPY,
        };
        let Some(layout) = snapshot_layout(bytes.len(), headers, start, len) else {
            return Ok(None);
        };
        let _sharing = crate::linker::profile::begin("snapshot-share", display);
        let registration = || LinkError::MappingRegistrationFailed {
            object: display.to_owned(),
            len,
        };
        let _transaction =
            kinakaze_runtime::begin_fork_mapping_transaction().ok_or_else(registration)?;
        let Ok(section) = bytes.duplicate_section() else {
            return Ok(None);
        };
        let slot = unsafe { kinakaze_alloc::ManagedAllocator.alloc(Layout::new::<AtomicUsize>()) }
            .cast::<AtomicUsize>();
        if slot.is_null() {
            return Err(registration());
        }
        unsafe { slot.write(AtomicUsize::new(section.as_raw_handle() as usize)) };
        if !unsafe { kinakaze_runtime::register_fork_handle_slot(slot) } {
            unsafe {
                kinakaze_alloc::ManagedAllocator.dealloc(slot.cast(), Layout::new::<AtomicUsize>())
            };
            return Err(registration());
        }
        let section = section.into_raw_handle();
        let mut owned = Self {
            base: core::ptr::null_mut(),
            slot,
        };
        let preferred = if fixed {
            start as *const core::ffi::c_void
        } else {
            core::ptr::null()
        };
        let view = unsafe {
            MapViewOfFileEx(
                section,
                FILE_MAP_COPY | FILE_MAP_EXECUTE,
                0,
                0,
                len,
                preferred,
            )
        };
        if view.Value.is_null() {
            // A data-only snapshot or a platform restriction can reject the
            // executable view. The original independent backing remains valid.
            return Ok(None);
        }
        owned.base = view.Value.cast();
        if fixed && owned.base as u64 != start {
            return Ok(None);
        }
        {
            let _normalizing = crate::linker::profile::begin("section-normalize", display);
            let mut end = 0;
            for &(target, source, count) in &layout {
                // Match the original zero-initialized section exactly, including
                // inter-segment padding, BSS and bytes outside PT_LOAD contents.
                unsafe { core::ptr::write_bytes(owned.base.add(end), 0, target - end) };
                if target != source {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            bytes.as_ptr().add(source),
                            owned.base.add(target),
                            count,
                        )
                    };
                }
                end = target + count;
            }
            unsafe { core::ptr::write_bytes(owned.base.add(end), 0, len - end) };
        }
        if !kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
            base: owned.base as usize,
            len,
            behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
            storage: kinakaze_runtime::ForkMappingStorage::CopyOnWriteSection,
            backing_slot: slot as usize,
            backing_offset: 0,
            view_protection: PAGE_EXECUTE_WRITECOPY,
            domain: kinakaze_runtime::ForkMappingDomain::GuestMm,
        }) {
            return Err(registration());
        }
        Ok(Some(owned))
    }
}

/// Fast path only for nonoverlapping segments whose source section spans the
/// whole virtual image. Other ELF layouts retain the general copy path.
#[cfg(windows)]
fn snapshot_layout(
    source_len: usize,
    headers: &[ProgramHeader],
    start: u64,
    len: usize,
) -> Option<Vec<(usize, usize, usize)>> {
    if len == 0 || len > (source_len.checked_add(4095)? & !4095) {
        return None;
    }
    let mut spans = Vec::new();
    let mut layout = Vec::new();
    let mut shared = 0usize;
    for header in headers
        .iter()
        .filter(|header| header.kind == kinakaze_elf::PT_LOAD)
    {
        if header.file_size > header.memory_size {
            return None;
        }
        let target = usize::try_from(header.virtual_address.checked_sub(start)?).ok()?;
        let memory_end = target.checked_add(usize::try_from(header.memory_size).ok()?)?;
        if memory_end > len {
            return None;
        }
        spans.push((target, memory_end));
        if header.file_size == 0 {
            continue;
        }
        let source = usize::try_from(header.offset).ok()?;
        let count = usize::try_from(header.file_size).ok()?;
        if source.checked_add(count)? > source_len {
            return None;
        }
        if source == target {
            shared = shared.checked_add(count)?;
        }
        layout.push((target, source, count));
    }
    if shared < 1024 * 1024 {
        return None;
    }
    spans.sort_unstable();
    if spans.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return None;
    }
    // Do not turn a sparse image's large, originally demand-zero BSS into
    // eagerly copied private pages merely to share a small file segment.
    if len.checked_sub(shared)? > shared / 4 {
        return None;
    }
    layout.sort_unstable();
    Some(layout)
}

#[cfg(all(test, windows))]
mod tests;

#[cfg(windows)]
impl Drop for ImageMapping {
    fn drop(&mut self) {
        use core::alloc::{GlobalAlloc, Layout};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Memory::{MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile},
        };
        let _transaction = kinakaze_runtime::begin_fork_mapping_transaction()
            .expect("an ELF view has an initialized mapping transaction");
        if !self.base.is_null() {
            kinakaze_runtime::unregister_fork_mapping(self.base as usize);
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.base.cast(),
                })
            };
        }
        kinakaze_runtime::unregister_fork_handle_slot(self.slot);
        unsafe {
            CloseHandle((*self.slot).swap(0, Ordering::AcqRel) as _);
            kinakaze_alloc::ManagedAllocator
                .dealloc(self.slot.cast(), Layout::new::<AtomicUsize>());
        }
    }
}

#[cfg(not(windows))]
impl ImageMapping {
    pub fn new(
        _: bool,
        _: u64,
        len: usize,
        display: &str,
        _: &crate::ImmutableBytes,
        _: &[ProgramHeader],
    ) -> Result<Self, LinkError> {
        Err(LinkError::MappingFailed {
            object: display.to_owned(),
            len,
        })
    }
}
