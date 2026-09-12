//! search.h linear search in caller-owned arrays; no allocation or hidden state.
use super::Compare;
use core::{ffi::c_void, ptr};

/// Returns the first matching element without modifying the array or its count.
///
/// # Safety
/// `count` and the array it describes must be readable. `compare` must accept
/// `key` and every array element. The caller synchronizes access to the array.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lfind(
    key: *const c_void,
    base: *const c_void,
    count: *const usize,
    width: usize,
    compare: Compare,
) -> *mut c_void {
    let mut current = base.cast::<u8>();
    let mut index = 0;
    // Keep pointers, not Rust references, across a guest comparator callback.
    while index < unsafe { count.read() } {
        if unsafe { compare(key, current.cast()) } == 0 {
            return current.cast_mut().cast();
        }
        current = unsafe { current.add(width) };
        index += 1;
    }
    ptr::null_mut()
}

/// Finds an element or copies the key into the next free array slot.
///
/// # Safety
/// The lfind requirements apply. On a miss, `count` and one additional array
/// slot must be writable, and that slot must not overlap the key's storage.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_lsearch(
    key: *const c_void,
    base: *mut c_void,
    count: *mut usize,
    width: usize,
    compare: Compare,
) -> *mut c_void {
    let found = unsafe { kinakaze_abi_lfind(key, base, count, width, compare) };
    if !found.is_null() {
        return found;
    }
    unsafe {
        let length = count.read();
        let slot = base.cast::<u8>().add(length * width);
        ptr::copy_nonoverlapping(key.cast::<u8>(), slot, width);
        count.write(length + 1);
        slot.cast()
    }
}
