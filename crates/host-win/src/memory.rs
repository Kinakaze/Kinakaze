use std::io;
use std::mem::zeroed;
use std::ptr::{NonNull, null};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_NOACCESS, PAGE_READONLY,
    PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryProtection {
    NoAccess,
    ReadOnly,
    ReadWrite,
    ReadExecute,
}

/// Native virtual-memory page size (not the larger allocation granularity).
pub fn page_size() -> usize {
    // SAFETY: GetSystemInfo initializes the complete valid output structure.
    unsafe {
        let mut info: SYSTEM_INFO = zeroed();
        GetSystemInfo(&mut info);
        info.dwPageSize as usize
    }
}

/// Owned, page-rounded Windows allocation for generated ELF facade images.
/// Allocation begins zeroed/RW; no API exposes a writable-and-executable mode.
pub struct ExecutableMemory {
    base: NonNull<u8>,
    length: usize,
}

impl ExecutableMemory {
    pub fn allocate(size: usize) -> io::Result<Self> {
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty memory allocation",
            ));
        }
        let page = page_size();
        let length = size
            .checked_add(page - 1)
            .map(|n| n / page * page)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "allocation size overflow")
            })?;
        // SAFETY: Null requests a new region; VirtualAlloc commits zero-filled pages.
        let allocation =
            unsafe { VirtualAlloc(null(), length, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) };
        let base = NonNull::new(allocation.cast()).ok_or_else(io::Error::last_os_error)?;
        Ok(Self { base, length })
    }

    pub fn as_ptr(&self) -> *mut u8 {
        self.base.as_ptr()
    }
    pub fn len(&self) -> usize {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Change whole pages within this allocation. RX transitions also flush the
    /// process instruction cache before generated code may be called.
    pub fn protect(
        &self,
        offset: usize,
        len: usize,
        protection: MemoryProtection,
    ) -> io::Result<()> {
        let page = page_size();
        if len == 0
            || !offset.is_multiple_of(page)
            || !len.is_multiple_of(page)
            || offset.checked_add(len).is_none_or(|end| end > self.length)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "protection range must contain whole allocated pages",
            ));
        }
        let flags = match protection {
            MemoryProtection::NoAccess => PAGE_NOACCESS,
            MemoryProtection::ReadOnly => PAGE_READONLY,
            MemoryProtection::ReadWrite => PAGE_READWRITE,
            MemoryProtection::ReadExecute => PAGE_EXECUTE_READ,
        };
        // SAFETY: Checked bounds keep this pointer within the live allocation.
        let address = unsafe { self.base.as_ptr().add(offset) };
        let mut previous = 0;
        // SAFETY: The range lies in this committed allocation; flags are valid.
        if unsafe { VirtualProtect(address.cast(), len, flags, &mut previous) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if protection == MemoryProtection::ReadExecute {
            // SAFETY: Address and length denote the newly executable region.
            if unsafe { FlushInstructionCache(GetCurrentProcess(), address.cast(), len) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        // SAFETY: MEM_RELEASE with size zero releases exactly this owned allocation.
        unsafe {
            VirtualFree(self.base.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}
