use std::ffi::{CStr, c_void};
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleHandleExW, GetProcAddress,
    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

/// One explicit LoadLibrary reference. Symbol users must retain this owner.
pub struct Library {
    module: LoadedModule,
    path: PathBuf,
}

impl Library {
    /// Require an absolute path. Resolve dependent libraries only beside this
    /// DLL or in System32, never via the caller's current directory or PATH.
    pub fn open(path: &Path) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "DLL path must be absolute",
            ));
        }
        let filename = crate::wide(path.as_os_str())?;
        let path = path.canonicalize()?;
        // SAFETY: Filename is NUL terminated; no file handle is supplied.
        let module = unsafe {
            LoadLibraryExW(
                filename.as_ptr(),
                null_mut(),
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        };
        if module.is_null() {
            let error = io::Error::last_os_error();
            Err(io::Error::new(
                error.kind(),
                format!("load {}: {error}", path.display()),
            ))
        } else {
            Ok(Self {
                module: LoadedModule { handle: module },
                path,
            })
        }
    }

    /// Canonical absolute path used for this library load.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn base_address(&self) -> usize {
        self.module.base_address()
    }
    pub fn mapped_len(&self) -> usize {
        self.module.mapped_len()
    }

    /// # Safety
    /// Keep the library alive and call with the symbol's actual ABI.
    pub unsafe fn symbol(&self, name: &CStr) -> io::Result<*mut c_void> {
        unsafe { self.module.symbol(name) }
    }
}

/// An owning reference to an already loaded native module. Pinning does not
/// search for a filename, reload an image or allocate a path buffer.
pub struct LoadedModule {
    handle: HMODULE,
}

impl LoadedModule {
    pub fn pin(base: usize) -> io::Result<Self> {
        if base == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "null module base",
            ));
        }
        let mut handle = null_mut();
        // SAFETY: FROM_ADDRESS interprets the second argument as an address,
        // without dereferencing it. Success acquires a reference for this owner.
        if unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                base as *const u16,
                &mut handle,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let owned = Self { handle };
        if owned.base_address() != base {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "address is not a module base",
            ));
        }
        Ok(owned)
    }

    /// Process-local module base, for the runtime's native fork registry.
    pub fn base_address(&self) -> usize {
        self.handle as usize
    }

    /// Extent of the PE mapping validated and retained by the Windows loader.
    pub fn mapped_len(&self) -> usize {
        // SAFETY: LoadLibraryExW returned a live executable PE mapping (never
        // LOAD_LIBRARY_AS_DATAFILE). Its DOS/NT headers remain mapped until Drop.
        unsafe {
            let base = self.handle.cast::<u8>();
            let nt = base.add(0x3c).cast::<u32>().read_unaligned() as usize;
            // Signature + IMAGE_FILE_HEADER + offsetof(OptionalHeader.SizeOfImage).
            base.add(nt + 4 + 20 + 56).cast::<u32>().read_unaligned() as usize
        }
    }

    /// Resolve an exported address.
    ///
    /// # Safety
    /// The caller must use the correct signature and calling convention and
    /// must keep this library alive for every use of the returned address.
    pub unsafe fn symbol(&self, name: &CStr) -> io::Result<*mut c_void> {
        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty DLL symbol",
            ));
        }
        // SAFETY: This instance owns the loaded module and name is NUL terminated.
        match unsafe { GetProcAddress(self.handle, name.as_ptr().cast()) } {
            Some(address) => Ok(address as *const () as *mut c_void),
            None => Err(io::Error::last_os_error()),
        }
    }
}

impl Drop for LoadedModule {
    fn drop(&mut self) {
        // SAFETY: Exactly one LoadLibrary reference belongs to this owner.
        unsafe {
            FreeLibrary(self.handle);
        }
    }
}

// SAFETY: module references are process-wide and pin the image until Drop.
unsafe impl Send for LoadedModule {}
unsafe impl Sync for LoadedModule {}

// SAFETY: Windows module references are process-wide and resolution is thread safe.
unsafe impl Send for Library {}
// SAFETY: The last owner cannot unload a library while borrowed by symbol().
unsafe impl Sync for Library {}
