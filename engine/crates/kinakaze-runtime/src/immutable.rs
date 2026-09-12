//! Immutable runtime bytes retained across fork without copying their payload.
use std::{io, ops::Deref};

/// A stable, read-only snapshot. Each process owns its view and handle; fork
/// maps the same immutable backing rather than copying it into the child arena.
pub struct ImmutableBytes {
    #[cfg(windows)]
    base: *mut u8,
    #[cfg(windows)]
    slot: *mut std::sync::atomic::AtomicUsize,
    #[cfg(windows)]
    len: usize,
    #[cfg(not(windows))]
    bytes: Box<[u8]>,
}

// Initialization finishes before publication. No mutable reference or writable
// view is exposed afterwards; ownership can move without moving the bytes.
unsafe impl Send for ImmutableBytes {}
unsafe impl Sync for ImmutableBytes {}

impl ImmutableBytes {
    #[cfg(windows)]
    pub fn fork_parts(&self) -> [usize; 3] {
        [self.base as usize, self.len, self.slot as usize]
    }

    /// Adopt one immutable view already restored by the fork coordinator.
    ///
    /// # Safety
    /// The handoff must transfer ownership exactly once; no other child owner
    /// may release this view or its retained handle slot.
    #[cfg(windows)]
    pub unsafe fn adopt_fork(parts: [usize; 3]) -> io::Result<Self> {
        let [base, len, slot] = parts;
        if len == 0 && slot == 0 {
            return Ok(Self {
                base: core::ptr::NonNull::dangling().as_ptr(),
                slot: core::ptr::null_mut(),
                len,
            });
        }
        let valid = kinakaze_alloc::shared_mapping(base).is_some_and(|mapping| {
            mapping.base == base
                && mapping.len >= len
                && mapping.backing_slot == slot
                && mapping.storage == crate::ForkMappingStorage::RetainedSection as u32
                && mapping.view_protection == windows_sys::Win32::System::Memory::PAGE_READONLY
        });
        if !valid {
            return Err(io::Error::other("immutable fork view is not registered"));
        }
        Ok(Self {
            base: base as _,
            slot: slot as _,
            len,
        })
    }

    pub fn from_slice(bytes: &[u8]) -> io::Result<Self> {
        Self::initialize(bytes.len(), |target| {
            target.copy_from_slice(bytes);
            Ok(())
        })
    }

    /// Initialize directly into the backing, avoiding a temporary large Vec in
    /// the managed allocator whose high-water mark would still be fork-copied.
    #[cfg(windows)]
    pub fn initialize(
        len: usize,
        initialize: impl FnOnce(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::initialize_backing(len, false, initialize)
    }

    /// Read-only source bytes whose section also permits a separate executable
    /// copy-on-write view. This view itself never becomes writable/executable.
    pub fn initialize_executable(
        len: usize,
        initialize: impl FnOnce(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        #[cfg(windows)]
        {
            Self::initialize_backing(len, true, initialize)
        }
        #[cfg(not(windows))]
        {
            Self::initialize(len, initialize)
        }
    }

    /// Retain independent ownership for another private view of this snapshot.
    /// The caller must not create writable shared aliases of immutable bytes.
    #[cfg(windows)]
    pub fn duplicate_section(&self) -> io::Result<std::os::windows::io::OwnedHandle> {
        use std::os::windows::io::{FromRawHandle, OwnedHandle};
        use std::sync::atomic::Ordering;
        use windows_sys::Win32::{
            Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
            System::Threading::GetCurrentProcess,
        };
        if self.slot.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty snapshot",
            ));
        }
        let mut handle = core::ptr::null_mut();
        if unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                (*self.slot).load(Ordering::Acquire) as _,
                GetCurrentProcess(),
                &mut handle,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }

    #[cfg(windows)]
    fn initialize_backing(
        len: usize,
        executable: bool,
        initialize: impl FnOnce(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        use core::alloc::{GlobalAlloc, Layout};
        use std::sync::atomic::AtomicUsize;
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Memory::*,
        };
        if len == 0 {
            initialize(&mut [])?;
            return Ok(Self {
                base: core::ptr::NonNull::dangling().as_ptr(),
                slot: core::ptr::null_mut(),
                len,
            });
        }
        let mapped_len = len
            .checked_add(4095)
            .map(|size| size & !4095)
            .filter(|&size| size <= isize::MAX as usize)
            .ok_or_else(|| io::Error::new(io::ErrorKind::OutOfMemory, "snapshot too large"))?;
        let registration = || io::Error::other("immutable snapshot fork registration failed");
        let _transaction = crate::begin_fork_mapping_transaction().ok_or_else(registration)?;
        let slot = unsafe { kinakaze_alloc::ManagedAllocator.alloc(Layout::new::<AtomicUsize>()) }
            .cast::<AtomicUsize>();
        if slot.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "snapshot handle slot",
            ));
        }
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                if executable {
                    PAGE_EXECUTE_READWRITE
                } else {
                    PAGE_READWRITE
                },
                ((mapped_len as u64) >> 32) as u32,
                mapped_len as u32,
                core::ptr::null(),
            )
        };
        if handle.is_null() {
            let error = io::Error::last_os_error();
            unsafe {
                kinakaze_alloc::ManagedAllocator.dealloc(slot.cast(), Layout::new::<AtomicUsize>())
            };
            return Err(error);
        }
        unsafe { slot.write(AtomicUsize::new(handle as usize)) };
        if !unsafe { crate::register_fork_handle_slot(slot) } {
            unsafe {
                CloseHandle(handle);
                kinakaze_alloc::ManagedAllocator.dealloc(slot.cast(), Layout::new::<AtomicUsize>());
            }
            return Err(registration());
        }
        let mut owned = Self {
            base: core::ptr::null_mut(),
            slot,
            len,
        };
        let staging = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, mapped_len) };
        if staging.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        owned.base = staging.Value.cast();
        initialize(unsafe { core::slice::from_raw_parts_mut(owned.base, len) })?;
        // Publish this same populated view read-only. Unmapping and mapping a
        // second view discards its resident page-table entries, forcing the
        // loader's parse/hash/copy passes to fault every image page in again.
        // There is no writable alias after initialization; fork still maps the
        // retained section PAGE_READONLY using the registered protection below.
        let mut previous = 0;
        if unsafe { VirtualProtect(owned.base.cast(), mapped_len, PAGE_READONLY, &mut previous) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        if !crate::register_fork_mapping(crate::ForkMapping {
            base: owned.base as usize,
            len: mapped_len,
            behavior: crate::ForkMappingBehavior::Copy,
            storage: crate::ForkMappingStorage::RetainedSection,
            backing_slot: slot as usize,
            backing_offset: 0,
            view_protection: PAGE_READONLY,
            // These bytes belong to the loader, not to the guest's mutable mm.
            domain: crate::ForkMappingDomain::HostPrivate,
        }) {
            return Err(registration());
        }
        Ok(owned)
    }

    #[cfg(not(windows))]
    pub fn initialize(
        len: usize,
        initialize: impl FnOnce(&mut [u8]) -> io::Result<()>,
    ) -> io::Result<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(io::Error::other)?;
        bytes.resize(len, 0);
        initialize(&mut bytes)?;
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        #[cfg(windows)]
        {
            unsafe { core::slice::from_raw_parts(self.base, self.len) }
        }
        #[cfg(not(windows))]
        {
            &self.bytes
        }
    }
}

impl Deref for ImmutableBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl AsRef<[u8]> for ImmutableBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl std::fmt::Debug for ImmutableBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImmutableBytes")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(windows)]
impl Drop for ImmutableBytes {
    fn drop(&mut self) {
        use core::alloc::{GlobalAlloc, Layout};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use windows_sys::Win32::{Foundation::CloseHandle, System::Memory::*};
        if self.slot.is_null() {
            return;
        }
        let _transaction =
            crate::begin_fork_mapping_transaction().expect("snapshot mapping transaction");
        if !self.base.is_null() {
            crate::unregister_fork_mapping(self.base as usize);
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.base.cast(),
                })
            };
        }
        crate::unregister_fork_handle_slot(self.slot);
        unsafe {
            CloseHandle((*self.slot).swap(0, Ordering::AcqRel) as _);
            kinakaze_alloc::ManagedAllocator
                .dealloc(self.slot.cast(), Layout::new::<AtomicUsize>());
        }
    }
}

#[cfg(all(test, windows))]
mod tests;
