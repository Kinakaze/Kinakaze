//! Supported NSS source overrides and the dynarray ABI used by Debian getent.
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
};
use std::sync::atomic::{AtomicU64, Ordering};

// Only hosts has two implemented providers. Other databases are file backed.
static HOST_SOURCES: AtomicU64 = AtomicU64::new(3);
pub(super) fn host_sources() -> (bool, bool) {
    let flags = HOST_SOURCES.load(Ordering::Acquire);
    (flags & 1 != 0, flags & 2 != 0)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___nss_configure_lookup(
    database: *const c_char,
    services: *const c_char,
) -> c_int {
    if database.is_null() || services.is_null() {
        crate::set_errno(22);
        return -1;
    }
    let database = unsafe { CStr::from_ptr(database) }.to_bytes();
    let services = unsafe { CStr::from_ptr(services) }.to_bytes();
    let tokens: Vec<_> = services
        .split(u8::is_ascii_whitespace)
        .filter(|s| !s.is_empty())
        .collect();
    let supported = matches!(
        database,
        b"hosts"
            | b"passwd"
            | b"group"
            | b"shadow"
            | b"gshadow"
            | b"aliases"
            | b"ethers"
            | b"networks"
            | b"protocols"
            | b"rpc"
            | b"services"
            | b"netgroup"
    );
    let implemented = tokens == [b"files".as_slice()]
        || database == b"hosts"
            && (tokens == [b"dns".as_slice()]
                || tokens == [b"files".as_slice(), b"dns".as_slice()]);
    if !supported || !implemented {
        crate::set_errno(kinakaze_vfs::EOPNOTSUPP);
        return -1;
    }
    if database == b"hosts" {
        let bits = u64::from(tokens.contains(&b"files".as_slice()))
            | (u64::from(tokens.contains(&b"dns".as_slice())) << 1);
        HOST_SOURCES.store(bits, Ordering::Release);
    }
    0
}

#[repr(C)]
pub struct DynArray {
    used: usize,
    allocated: usize,
    array: *mut u8,
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___libc_dynarray_resize(
    list: *mut DynArray,
    size: usize,
    scratch: *mut c_void,
    element_size: usize,
) -> bool {
    if list.is_null() {
        crate::set_errno(22);
        return false;
    }
    let list = unsafe { &mut *list };
    if list.allocated == usize::MAX {
        crate::set_errno(12);
        return false;
    }
    if size <= list.allocated {
        list.used = size;
        return true;
    }
    let Some(bytes) = size
        .checked_mul(element_size)
        .filter(|n| *n <= isize::MAX as usize)
    else {
        crate::set_errno(12);
        return false;
    };
    let allocation = if list.array.cast::<c_void>() == scratch {
        let target = unsafe { kinakaze_alloc::guest::malloc(bytes) };
        if !target.is_null() && list.used != 0 {
            unsafe {
                ptr::copy_nonoverlapping(list.array, target, list.used * element_size);
            }
        }
        target
    } else {
        unsafe { kinakaze_alloc::guest::reallocate(list.array, 16, bytes) }
    };
    if allocation.is_null() {
        crate::set_errno(12);
        return false;
    }
    list.array = allocation;
    list.allocated = size;
    list.used = size;
    true
}

unsafe extern "system" fn snapshot(output: *mut u8, capacity: usize) -> isize {
    if output.is_null() {
        return 8;
    }
    if capacity < 8 {
        return -22;
    }
    unsafe {
        output
            .cast::<u64>()
            .write_unaligned(HOST_SOURCES.load(Ordering::Acquire));
    }
    8
}
unsafe extern "system" fn restore(input: *const u8, length: usize) -> i32 {
    if input.is_null() || length != 8 {
        return 22;
    }
    let value = unsafe { input.cast::<u64>().read_unaligned() };
    if !(1..=3).contains(&value) {
        return 22;
    }
    HOST_SOURCES.store(value, Ordering::Release);
    0
}
extern "C" fn register() {
    let _ = unsafe { kinakaze_runtime::register_fork_participant_without_inherited_handles(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 500,
        key: u64::from_le_bytes(*b"CYNSS001"),
        prepare: None,
        snapshot: Some(snapshot),
        parent: None,
        child: Some(restore),
    }) };
}
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static INITIALIZER: extern "C" fn() = register;
