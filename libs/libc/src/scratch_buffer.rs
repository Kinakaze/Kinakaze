//! glibc's private scratch-buffer ABI, also used by the shipped pldd command.
//! Layout/contract: glibc include/scratch_buffer.h and malloc/scratch_buffer_grow.c.
use core::{ffi::c_void, ptr};
use kinakaze_alloc::guest;

#[repr(C, align(16))]
pub struct ScratchBuffer {
    data: *mut c_void,
    length: usize,
    space: [u8; 1024],
}

/// Discard the old allocation and double the capacity. On failure the object
/// returns to its embedded buffer so the caller may still safely free it.
/// # Safety
/// `buffer` is an initialized scratch_buffer; its heap storage uses guest malloc.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___libc_scratch_buffer_grow(
    buffer: *mut ScratchBuffer,
) -> bool {
    if buffer.is_null() {
        crate::set_errno(22);
        return false;
    }
    let buffer = unsafe { &mut *buffer };
    let embedded = buffer.space.as_mut_ptr().cast();
    if buffer.data != embedded {
        unsafe {
            guest::free(buffer.data.cast());
        }
    }
    let next = buffer.length.checked_mul(2);
    buffer.data = embedded;
    buffer.length = buffer.space.len();
    let allocation = next.map_or(ptr::null_mut(), |size| unsafe { guest::malloc(size) });
    if allocation.is_null() {
        crate::set_errno(12);
        return false;
    }
    buffer.data = allocation.cast();
    buffer.length = next.unwrap();
    true
}
