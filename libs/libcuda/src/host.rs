//! Process-lifetime native bindings. The fast path reads one cached pointer.

use core::ffi::{c_char, c_void};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::{CuResult, NOT_FOUND, NOT_SUPPORTED, SUCCESS};

fn module() -> Option<usize> {
    static MODULE: OnceLock<usize> = OnceLock::new();
    let module = *MODULE.get_or_init(|| {
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::System::LibraryLoader::{
                LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
            };
            // Use the system driver, independent of the guest's cwd and PATH.
            let name: Vec<u16> = "nvcuda.dll\0".encode_utf16().collect();
            LoadLibraryExW(
                name.as_ptr(),
                core::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            ) as usize
        }
        #[cfg(not(windows))]
        {
            0
        }
    });
    (module != 0).then_some(module)
}

pub(crate) fn symbol(name: &'static [u8]) -> Option<usize> {
    debug_assert_eq!(name.last(), Some(&0));
    let module = module()?;
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            module as *mut c_void,
            name.as_ptr(),
        )
        .map(|entry| entry as usize)
    }
    #[cfg(not(windows))]
    {
        let _ = (module, name);
        None
    }
}

/// Reinstall ELF FS even for a thunk called directly via cuGetProcAddress.
pub(crate) struct RestoreTls;
impl Drop for RestoreTls {
    fn drop(&mut self) {
        let _ = kinakaze_tls::restore_current_thread_static_tls();
    }
}

pub(crate) struct Symbol {
    export: &'static [u8],
    pub base: &'static [u8],
    version: i32,
    flags: u64,
    address: AtomicUsize,
    canonical: AtomicUsize,
}

impl Symbol {
    pub const fn new(export: &'static [u8], base: &'static [u8], version: i32, flags: u64) -> Self {
        Self {
            export,
            base,
            version,
            flags,
            address: AtomicUsize::new(0),
            canonical: AtomicUsize::new(0),
        }
    }

    pub fn address(&self) -> Option<usize> {
        cached(&self.address, || symbol(self.export))
    }

    /// CUDA's query returns an internal implementation address, which differs
    /// from GetProcAddress's export thunk. Compare against the documented ABI
    /// version for this signature, not against the public Windows thunk.
    pub fn canonical(&self) -> Option<usize> {
        cached(&self.canonical, || {
            self.address()?;
            let query = unsafe { query(self.base.as_ptr().cast(), self.version, self.flags) };
            (query.result == SUCCESS && query.status == 0 && !query.address.is_null())
                .then_some(query.address as usize)
        })
    }
}

fn cached(cell: &AtomicUsize, resolve: impl FnOnce() -> Option<usize>) -> Option<usize> {
    let address = match cell.load(Ordering::Acquire) {
        0 => {
            let found = resolve().unwrap_or(1);
            cell.store(found, Ordering::Release);
            found
        }
        address => address,
    };
    (address != 1).then_some(address)
}

pub(crate) struct Query {
    pub result: CuResult,
    pub address: *mut c_void,
    pub status: i32,
}

pub(crate) unsafe fn query(name: *const c_char, version: i32, flags: u64) -> Query {
    static V2: OnceLock<Option<usize>> = OnceLock::new();
    static V1: OnceLock<Option<usize>> = OnceLock::new();
    let mut answer = Query {
        result: NOT_SUPPORTED,
        address: core::ptr::null_mut(),
        status: 1,
    };
    if let Some(address) = *V2.get_or_init(|| symbol(b"cuGetProcAddress_v2\0")) {
        let call: unsafe extern "system" fn(
            *const c_char,
            *mut *mut c_void,
            i32,
            u64,
            *mut i32,
        ) -> CuResult = unsafe { core::mem::transmute(address) };
        answer.result = unsafe {
            call(
                name,
                &mut answer.address,
                version,
                flags,
                &mut answer.status,
            )
        };
    } else if let Some(address) = *V1.get_or_init(|| symbol(b"cuGetProcAddress\0")) {
        let call: unsafe extern "system" fn(*const c_char, *mut *mut c_void, i32, u64) -> CuResult =
            unsafe { core::mem::transmute(address) };
        answer.result = unsafe { call(name, &mut answer.address, version, flags) };
        if answer.result == SUCCESS && !answer.address.is_null() {
            answer.status = 0;
        }
        if answer.result == NOT_FOUND {
            answer.result = SUCCESS;
        }
    }
    answer
}
