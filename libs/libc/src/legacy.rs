//! Older glibc entry points over the canonical native implementations.
//! Private ABI contracts follow the pinned Debian glibc 2.36 headers.
use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use std::sync::atomic::{AtomicI32, Ordering};

// These entry points deliberately bypass pthread cancellation. They retain the
// ordinary errno and partial-I/O contracts, including EINTR from guest signals.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___close_nocancel(fd: c_int) -> c_int {
    kinakaze_vfs::close(fd).map_or_else(
        |e| {
            crate::set_errno(e);
            -1
        },
        |()| 0,
    )
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___read_nocancel(
    fd: c_int,
    data: *mut c_void,
    size: usize,
) -> isize {
    if size == 0 {
        return 0;
    }
    if data.is_null() {
        crate::set_errno(14);
        return -1;
    }
    kinakaze_vfs::read(fd, unsafe {
        core::slice::from_raw_parts_mut(data.cast(), size)
    })
    .map_or_else(
        |e| {
            crate::set_errno(e);
            -1
        },
        |n| n as isize,
    )
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___open64_nocancel(
    path: *const c_char,
    flags: c_int,
    mode: u32,
) -> c_int {
    unsafe { crate::fsextra::kinakaze_abi_open64(path, flags, mode) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___pread64_nocancel(
    fd: c_int,
    data: *mut c_void,
    size: usize,
    offset: i64,
) -> isize {
    unsafe { crate::fdio::kinakaze_abi_pread64(fd, data, size, offset) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___mmap(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> *mut c_void {
    unsafe { crate::fdio::kinakaze_abi_mmap(address, length, protection, flags, fd, offset) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___munmap(address: *mut c_void, length: usize) -> c_int {
    unsafe { crate::fdio::kinakaze_abi_munmap(address, length) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___madvise(
    address: *mut c_void,
    length: usize,
    advice: c_int,
) -> c_int {
    unsafe { crate::fdio::kinakaze_abi_madvise(address, length, advice) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___libc_secure_getenv(
    name: *const c_char,
) -> *mut c_char {
    unsafe { crate::process::kinakaze_abi_secure_getenv(name) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___strtoull_internal(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
    _group: c_int,
) -> u64 {
    // The native locale's numeric grouping is empty (C/C.UTF-8).
    unsafe { crate::process::kinakaze_abi_strtoull(text, end, base) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___libc_fatal(message: *const c_char) -> ! {
    if !message.is_null() {
        let mut bytes = unsafe { CStr::from_ptr(message) }.to_bytes();
        while !bytes.is_empty() {
            match kinakaze_vfs::write(2, bytes) {
                Ok(0) => break,
                Ok(n) => bytes = &bytes[n..],
                Err(kinakaze_vfs::EINTR) => continue,
                Err(_) => break,
            }
        }
    }
    crate::process::kinakaze_abi_abort()
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___nss_files_fopen(
    path: *const c_char,
) -> *mut crate::stdio::File {
    let file = unsafe { crate::stdio::fopen(path, c"re".as_ptr()) };
    if file.is_null() {
        return file;
    }
    unsafe {
        crate::misc::kinakaze_abi___fsetlocking(file.cast(), 2);
    }
    if crate::stdio::fseek(file, 0, 0) != 0 {
        unsafe {
            crate::stdio::fclose(file);
        }
        crate::set_errno(29);
        return ptr::null_mut();
    }
    file
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___nss_hash(key: *const u8, len: usize) -> u32 {
    if len == 0 {
        return 0;
    }
    unsafe { core::slice::from_raw_parts(key, len) }
        .iter()
        .fold(0u32, |hash, &byte| {
            hash.wrapping_mul(65599).wrapping_add(byte.into())
        })
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn WaitOnAddress(
        address: *const c_void,
        compare: *const c_void,
        size: usize,
        milliseconds: u32,
    ) -> i32;
    fn WakeByAddressSingle(address: *const c_void);
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___lll_lock_wait_private(lock: *mut AtomicI32) {
    let lock = unsafe { &*lock };
    // glibc uses 0 = free, 1 = owned, 2 = owned with possible waiters.
    while lock.swap(2, Ordering::Acquire) != 0 {
        let contended = 2i32;
        unsafe {
            WaitOnAddress(
                (lock as *const AtomicI32).cast(),
                (&contended as *const i32).cast(),
                4,
                u32::MAX,
            );
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___lll_lock_wake_private(lock: *mut AtomicI32) {
    // The caller has already published the release store to zero.
    unsafe {
        WakeByAddressSingle(lock.cast());
    }
}
