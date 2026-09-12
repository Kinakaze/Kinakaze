//! Runtime load, symbol lookup and reference release. Naked lookup entries
//! preserve the ELF caller for RTLD_NEXT before creating any Rust frame.
use super::{clear_dl_error, dynamic_loader_lock, loaded_linker, set_dl_error};
use std::ffi::CStr;
use std::path::PathBuf;
use std::ptr;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_dlopen(
    filename: *const core::ffi::c_char,
    flags: i32,
) -> *mut core::ffi::c_void {
    let path = if filename.is_null() {
        None
    } else {
        // SAFETY: dlopen's ABI requires a terminated filename.
        let filename = unsafe { CStr::from_ptr(filename) };
        let Ok(filename) = filename.to_str() else {
            set_dl_error("dlopen filename is not UTF-8");
            return ptr::null_mut();
        };
        if filename.contains('/') {
            match kinakaze_vfs::resolve_linux_path(filename) {
                Ok(path) => Some(path),
                Err(error) => {
                    set_dl_error(error);
                    return ptr::null_mut();
                }
            }
        } else {
            Some(PathBuf::from(filename))
        }
    };

    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if linker.is_null() {
        set_dl_error("no active process dynamic linker");
        return ptr::null_mut();
    }
    // SAFETY: the active pointer names the boxed linker for the lifetime of
    // the guest, and the process loader lock serializes mutable access.
    match unsafe { (&mut *linker).open_runtime(path.as_deref(), flags) } {
        Ok(handle) => {
            if let Err(error) = unsafe { (&mut *linker).refresh_runtime_maps() } {
                let _ = unsafe { (&mut *linker).close_runtime(handle) };
                set_dl_error(error);
                return ptr::null_mut();
            }
            clear_dl_error();
            handle as *mut core::ffi::c_void
        }
        Err(error) => {
            set_dl_error(error);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn kinakaze_process_dlsym(
    _handle: *mut core::ffi::c_void,
    _name: *const core::ffi::c_char,
) -> *mut core::ffi::c_void {
    core::arch::naked_asm!(
        "mov rdx, [rsp]",
        "jmp {implementation}",
        implementation = sym kinakaze_process_dlsym_impl,
    )
}

unsafe extern "sysv64" fn kinakaze_process_dlsym_impl(
    handle: *mut core::ffi::c_void,
    name: *const core::ffi::c_char,
    caller_address: usize,
) -> *mut core::ffi::c_void {
    if name.is_null() {
        set_dl_error("dlsym symbol name is null");
        return ptr::null_mut();
    }
    // SAFETY: dlsym's ABI requires a terminated symbol name.
    let name = unsafe { CStr::from_ptr(name) };
    let Ok(name) = name.to_str() else {
        set_dl_error("dlsym symbol name is not UTF-8");
        return ptr::null_mut();
    };
    let raw_handle = handle as usize;
    let handle = if handle.is_null() || raw_handle == usize::MAX {
        None
    } else {
        Some(raw_handle)
    };
    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if linker.is_null() {
        set_dl_error("no active process dynamic linker");
        return ptr::null_mut();
    }
    // SAFETY: protected by the process loader lock; this is a shared lookup.
    let lookup = if raw_handle == usize::MAX {
        // RTLD_NEXT is relative to the object containing the call site. The
        // naked ABI shim above records the guest return address before Rust
        // can create a stack frame.
        unsafe { (&*linker).lookup_next(caller_address, name) }
    } else {
        unsafe { (&*linker).lookup_runtime(handle, name) }
    };
    match lookup {
        Ok(Some(resolution)) => {
            let mut address = if let Some(address) = resolution.address {
                address
            } else if let (Some(module), Some(offset)) =
                (resolution.tls_module, resolution.tls_offset)
            {
                match kinakaze_tls::elf_tls_get_addr(module, offset) {
                    Ok(address) => address as usize,
                    Err(error) => {
                        set_dl_error(format_args!("TLS symbol {name:?}: {error:?}"));
                        return ptr::null_mut();
                    }
                }
            } else {
                set_dl_error(format_args!(
                    "symbol {name:?} has no address or TLS definition"
                ));
                return ptr::null_mut();
            };
            if resolution.is_ifunc {
                // SAFETY: an STT_GNU_IFUNC definition is a no-argument
                // resolver returning the callable address.
                let resolver: unsafe extern "sysv64" fn() -> usize =
                    unsafe { core::mem::transmute(address) };
                address = unsafe { resolver() };
            }
            clear_dl_error();
            address as *mut core::ffi::c_void
        }
        Ok(None) => {
            set_dl_error(format_args!("symbol {name:?} was not found"));
            ptr::null_mut()
        }
        Err(error) => {
            set_dl_error(error);
            ptr::null_mut()
        }
    }
}

/// Resolve a specific GNU symbol version while preserving RTLD_NEXT's
/// guest call site across the Rust ABI boundary.
#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn kinakaze_process_dlvsym(
    _handle: *mut core::ffi::c_void,
    _name: *const core::ffi::c_char,
    _version: *const core::ffi::c_char,
) -> *mut core::ffi::c_void {
    core::arch::naked_asm!(
        "mov rcx, [rsp]",
        "jmp {implementation}",
        implementation = sym kinakaze_process_dlvsym_impl,
    )
}

unsafe extern "sysv64" fn kinakaze_process_dlvsym_impl(
    handle: *mut core::ffi::c_void,
    name: *const core::ffi::c_char,
    version: *const core::ffi::c_char,
    caller_address: usize,
) -> *mut core::ffi::c_void {
    if name.is_null() || version.is_null() {
        set_dl_error("dlvsym requires symbol and version names");
        return ptr::null_mut();
    }
    // SAFETY: dlvsym's guest ABI requires two terminated strings.
    let (Ok(name), Ok(version)) = (
        unsafe { CStr::from_ptr(name) }.to_str(),
        unsafe { CStr::from_ptr(version) }.to_str(),
    ) else {
        set_dl_error("dlvsym symbol or version name is not UTF-8");
        return ptr::null_mut();
    };
    let raw_handle = handle as usize;
    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if linker.is_null() {
        set_dl_error("no active process dynamic linker");
        return ptr::null_mut();
    }
    // SAFETY: the process loader lock serializes lookup and unloading.
    let linker = unsafe { &*linker };
    let lookup = if raw_handle == usize::MAX {
        linker.lookup_next_version(caller_address, name, version)
    } else {
        linker.lookup_runtime_version((raw_handle != 0).then_some(raw_handle), name, version)
    };
    match lookup {
        Ok(Some(resolution)) => {
            let mut address = if let Some(address) = resolution.address {
                address
            } else if let (Some(module), Some(offset)) =
                (resolution.tls_module, resolution.tls_offset)
            {
                match kinakaze_tls::elf_tls_get_addr(module, offset) {
                    Ok(address) => address as usize,
                    Err(error) => {
                        set_dl_error(format_args!("TLS symbol {name:?}: {error:?}"));
                        return ptr::null_mut();
                    }
                }
            } else {
                set_dl_error(format_args!(
                    "symbol {name:?} has no address or TLS definition"
                ));
                return ptr::null_mut();
            };
            if resolution.is_ifunc {
                // SAFETY: the ELF IFUNC contract defines a no-argument resolver.
                let resolver: unsafe extern "sysv64" fn() -> usize =
                    unsafe { core::mem::transmute(address) };
                address = unsafe { resolver() };
            }
            clear_dl_error();
            address as *mut core::ffi::c_void
        }
        Ok(None) => {
            set_dl_error(format_args!(
                "symbol {name:?} version {version:?} was not found"
            ));
            ptr::null_mut()
        }
        Err(error) => {
            set_dl_error(error);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_dlclose(handle: *mut core::ffi::c_void) -> i32 {
    if handle.is_null() {
        set_dl_error("dlclose handle is null");
        return -1;
    }
    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if linker.is_null() {
        set_dl_error("no active process dynamic linker");
        return -1;
    }
    // SAFETY: protected by the process loader lock.
    match unsafe { (&mut *linker).close_runtime(handle as usize) } {
        Ok(()) => {
            if let Err(error) = unsafe { (&mut *linker).refresh_runtime_maps() } {
                set_dl_error(error);
                return -1;
            }
            clear_dl_error();
            0
        }
        Err(error) => {
            set_dl_error(error);
            -1
        }
    }
}
