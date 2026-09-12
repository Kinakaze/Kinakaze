//! GNU packed argument strings use one caller-owned guest allocation.
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use kinakaze_alloc::c as guest;

/// # Safety
/// The argument vector is a guest allocation and `buffer` is readable for `size` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argz_append(
    argz: *mut *mut c_char,
    length: *mut usize,
    buffer: *const c_char,
    size: usize,
) -> c_int {
    if argz.is_null() || length.is_null() || (size != 0 && buffer.is_null()) {
        return kinakaze_vfs::EINVAL;
    }
    if size == 0 {
        return 0;
    }
    let old_length = unsafe { *length };
    let Some(new_length) = old_length.checked_add(size) else {
        return kinakaze_vfs::ENOMEM;
    };
    // The input may be an element of the allocation being resized.
    let input = unsafe { core::slice::from_raw_parts(buffer.cast::<u8>(), size) }.to_vec();
    let resized = unsafe { crate::c_realloc((*argz).cast(), new_length) }.cast::<c_char>();
    if resized.is_null() {
        return kinakaze_vfs::ENOMEM;
    }
    unsafe {
        ptr::copy_nonoverlapping(input.as_ptr(), resized.add(old_length).cast(), size);
        *argz = resized;
        *length = new_length;
    }
    0
}

/// # Safety
/// `before` is null or points inside the argument vector; `entry` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argz_insert(
    argz: *mut *mut c_char,
    length: *mut usize,
    before: *mut c_char,
    entry: *const c_char,
) -> c_int {
    if argz.is_null() || length.is_null() || entry.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    let input = unsafe { CStr::from_ptr(entry) }
        .to_bytes_with_nul()
        .to_vec();
    if before.is_null() {
        return unsafe {
            kinakaze_abi_argz_append(argz, length, input.as_ptr().cast(), input.len())
        };
    }
    let (base, old_length) = unsafe { (*argz, *length) };
    let Some(mut at) = (before as usize)
        .checked_sub(base as usize)
        .filter(|at| *at < old_length)
    else {
        return kinakaze_vfs::EINVAL;
    };
    while at > 0 && unsafe { *base.add(at - 1) } != 0 {
        at -= 1;
    }
    let Some(new_length) = old_length.checked_add(input.len()) else {
        return kinakaze_vfs::ENOMEM;
    };
    let resized = unsafe { crate::c_realloc(base.cast(), new_length) }.cast::<c_char>();
    if resized.is_null() {
        return kinakaze_vfs::ENOMEM;
    }
    unsafe {
        ptr::copy(
            resized.add(at),
            resized.add(at + input.len()),
            old_length - at,
        );
        ptr::copy_nonoverlapping(input.as_ptr(), resized.add(at).cast(), input.len());
        *argz = resized;
        *length = new_length;
    }
    0
}

/// # Safety
/// `argz` is writable for `length` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argz_stringify(
    argz: *mut c_char,
    length: usize,
    separator: c_int,
) {
    if length < 2 || argz.is_null() {
        return;
    }
    for byte in unsafe { core::slice::from_raw_parts_mut(argz, length - 1) } {
        if *byte == 0 {
            *byte = separator as c_char;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_accepts_an_interior_entry_pointer_and_preserves_binary_elements() {
        unsafe {
            let mut value = ptr::null_mut();
            let mut length = 0;
            assert_eq!(
                kinakaze_abi_argz_append(
                    &raw mut value,
                    &raw mut length,
                    b"first\0last\0".as_ptr().cast(),
                    11
                ),
                0
            );
            let before = value.add(8);
            assert_eq!(
                kinakaze_abi_argz_insert(
                    &raw mut value,
                    &raw mut length,
                    before,
                    c"middle".as_ptr()
                ),
                0
            );
            assert_eq!(
                core::slice::from_raw_parts(value.cast::<u8>(), length),
                b"first\0middle\0last\0"
            );
            let end = value.add(length);
            assert_eq!(
                kinakaze_abi_argz_insert(&raw mut value, &raw mut length, end, c"invalid".as_ptr()),
                kinakaze_vfs::EINVAL
            );
            kinakaze_abi_argz_stringify(value, length, b':' as c_int);
            assert_eq!(CStr::from_ptr(value).to_bytes(), b"first:middle:last");
            guest::free(value.cast());
        }
    }
}

/// # Safety
/// `string` is NUL-terminated; `argz` and `length` are writable output pointers.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_argz_create_sep(
    string: *const c_char,
    separator: c_int,
    argz: *mut *mut c_char,
    length: *mut usize,
) -> c_int {
    if string.is_null() || argz.is_null() || length.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    unsafe {
        *argz = ptr::null_mut();
        *length = 0;
    }
    let input = unsafe { CStr::from_ptr(string) }.to_bytes_with_nul();
    if input.len() == 1 {
        return 0;
    }
    let output = unsafe { guest::malloc(input.len()) }.cast::<u8>();
    if output.is_null() {
        return kinakaze_vfs::ENOMEM;
    }
    let mut written = 0;
    for &byte in input {
        if byte as c_char as c_int == separator {
            // Collapse leading and repeated delimiters. The source terminator
            // still preserves GNU's empty final element after a trailing one.
            if written != 0 && unsafe { *output.add(written - 1) } != 0 {
                unsafe {
                    *output.add(written) = 0;
                }
                written += 1;
            }
        } else {
            unsafe {
                *output.add(written) = byte;
            }
            written += 1;
        }
    }
    unsafe {
        *argz = output.cast();
        *length = written;
    }
    0
}
