//! Linux CUDA driver ABI to the installed Windows NVIDIA driver.
//!
//! No driver is loaded until a CUDA entry point is used. Handles and device
//! memory remain driver-owned; the bridge holds only immutable dispatch caches.
//! Function-pointer queries return our SysV entry points, never Windows code.
//! Signatures and ABI-version anchors follow NVIDIA CUDA 12.8 cudaTypedefs.h.
//! See README.md for the verified scope and native-resource fork behavior.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use core::ffi::{c_char, c_void};

mod api;
mod dispatch;
mod host;
mod lifecycle;
mod module;

pub use api::*;
pub use dispatch::{cuGetProcAddress, cuGetProcAddress_v2};
pub use module::cuModuleLoad;

pub type CuResult = i32;
pub type CuDevice = i32;
pub type CuDevicePtr = u64;
pub type CuContext = *mut c_void;
pub type CuModule = *mut c_void;
pub type CuFunction = *mut c_void;
pub type CuStream = *mut c_void;
pub type CuEvent = *mut c_void;

#[repr(C)]
pub struct CuUuid {
    pub bytes: [u8; 16],
}

#[repr(C)]
pub struct CuExecAffinityParam {
    pub kind: i32,
    pub sm_count: u32,
}

const _: () = {
    assert!(size_of::<usize>() == 8);
    assert!(size_of::<CuResult>() == 4);
    assert!(size_of::<CuDevice>() == 4);
    assert!(size_of::<CuDevicePtr>() == 8);
    assert!(size_of::<CuUuid>() == 16 && align_of::<CuUuid>() == 1);
    assert!(size_of::<CuExecAffinityParam>() == 8 && align_of::<CuExecAffinityParam>() == 4);
};

pub const SUCCESS: CuResult = 0;
pub const INVALID_VALUE: CuResult = 1;
pub const OUT_OF_MEMORY: CuResult = 2;
pub const NO_DEVICE: CuResult = 100;
pub const INVALID_IMAGE: CuResult = 200;
pub const FILE_NOT_FOUND: CuResult = 301;
pub const NOT_FOUND: CuResult = 500;
pub const NOT_SUPPORTED: CuResult = 801;
pub const UNKNOWN: CuResult = 999;

/// One signature supplies the guest thunk, host call and proc-address record.
macro_rules! cuda_api {
    ($( [$base:ident, $version:literal, $flags:literal] fn $name:ident(
        $($arg:ident: $ty:ty),* $(,)?
    ); )*) => {
        $(
            mod $name {
                pub static HOST: crate::host::Symbol = crate::host::Symbol::new(
                    concat!(stringify!($name), "\0").as_bytes(),
                    concat!(stringify!($base), "\0").as_bytes(), $version, $flags,
                );
                pub fn thunk() -> usize { super::$name as *const () as usize }
            }
            #[unsafe(export_name = concat!("kinakaze_engine_libcuda_", stringify!($name)))]
            pub unsafe extern "sysv64" fn $name($($arg: $ty),*) -> crate::CuResult {
                let _tls = crate::host::RestoreTls;
                if crate::lifecycle::unusable()
                    && !matches!(stringify!($name), "cuGetErrorName" | "cuGetErrorString" | "cuDriverGetVersion")
                { return crate::NOT_SUPPORTED; }
                let Some(address) = $name::HOST.address() else {
                    return if stringify!($name) == "cuInit" { crate::NO_DEVICE } else { crate::NOT_SUPPORTED };
                };
                // The public signatures differ only in x86-64 calling convention.
                let call: unsafe extern "system" fn($($ty),*) -> crate::CuResult =
                    unsafe { core::mem::transmute(address) };
                let result = unsafe { call($($arg),*) };
                if stringify!($name) == "cuInit" && result == crate::SUCCESS {
                    crate::lifecycle::initialized();
                }
                result
            }
        )*
        pub(crate) static ENTRIES: &[crate::dispatch::Entry] = &[
            $(crate::dispatch::Entry { host: &$name::HOST, thunk: $name::thunk },)*
        ];
    }
}
pub(crate) use cuda_api;

#[cfg(test)]
mod tests {
    #[test]
    fn loading_the_provider_does_not_load_the_cuda_driver() {
        #[cfg(windows)]
        {
            let name: Vec<u16> = "nvcuda.dll\0".encode_utf16().collect();
            let module = unsafe {
                windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(name.as_ptr())
            };
            assert!(
                module.is_null(),
                "CUDA must remain unloaded until a CUDA API is used"
            );
        }
    }
}

mod object_layout;
