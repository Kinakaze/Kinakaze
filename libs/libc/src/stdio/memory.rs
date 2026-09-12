//! Growable guest memory streams; neither temporary files nor host heap pointers.
use super::*;

#[repr(C)]
struct Memory {
    data: *mut u8,
    capacity: usize,
    length: usize,
    position: usize,
    buffer_out: *mut *mut c_char,
    size_out: *mut usize,
}

impl Memory {
    unsafe fn publish(&self) {
        unsafe {
            *self.buffer_out = self.data.cast();
            *self.size_out = self.position;
        }
    }
}

unsafe extern "sysv64" fn write(context: *mut c_void, data: *const u8, count: usize) -> isize {
    let stream = unsafe { &mut *context.cast::<Memory>() };
    let Some(end) = stream
        .position
        .checked_add(count)
        .filter(|&n| n < isize::MAX as usize)
    else {
        crate::set_errno(12);
        return -1;
    };
    if end >= stream.capacity {
        let capacity = (end + 1).max(stream.capacity.saturating_mul(2));
        let data = unsafe { kinakaze_alloc::c::realloc(stream.data, capacity) };
        if data.is_null() {
            crate::set_errno(12);
            return -1;
        }
        stream.data = data;
        stream.capacity = capacity;
    }
    unsafe {
        if stream.position > stream.length {
            ptr::write_bytes(
                stream.data.add(stream.length),
                0,
                stream.position - stream.length,
            );
        }
        ptr::copy_nonoverlapping(data, stream.data.add(stream.position), count);
        stream.position = end;
        stream.length = stream.length.max(end);
        *stream.data.add(stream.length) = 0;
        stream.publish();
    }
    count as isize
}

unsafe extern "sysv64" fn seek(context: *mut c_void, offset: *mut i64, whence: c_int) -> c_int {
    let stream = unsafe { &mut *context.cast::<Memory>() };
    let base = match whence {
        0 => 0,
        1 => stream.position,
        2 => stream.length,
        _ => {
            crate::set_errno(22);
            return -1;
        }
    };
    let Some(position) = (base as i64)
        .checked_add(unsafe { *offset })
        .filter(|&p| p >= 0 && p < isize::MAX as i64)
    else {
        crate::set_errno(22);
        return -1;
    };
    stream.position = position as usize;
    unsafe {
        *offset = position;
    }
    0
}
unsafe extern "sysv64" fn close(context: *mut c_void) -> c_int {
    let stream = unsafe { &*context.cast::<Memory>() };
    // Seeking without writing may put the cursor beyond the allocated buffer;
    // publish the written extent in that case.
    unsafe {
        *stream.buffer_out = stream.data.cast();
        *stream.size_out = stream.position.min(stream.length);
        kinakaze_alloc::guest::free(context.cast());
    }
    0
}
pub(super) unsafe fn open(buffer: *mut *mut c_char, size: *mut usize) -> *mut File {
    if buffer.is_null() || size.is_null() {
        crate::set_errno(22);
        return ptr::null_mut();
    }
    let data = unsafe { kinakaze_alloc::c::malloc(1) };
    if data.is_null() {
        crate::set_errno(12);
        return ptr::null_mut();
    }
    unsafe {
        *data = 0;
    }
    let state = unsafe { kinakaze_alloc::guest::malloc(size_of::<Memory>()) }.cast::<Memory>();
    if state.is_null() {
        unsafe {
            kinakaze_alloc::c::free(data);
        }
        crate::set_errno(12);
        return ptr::null_mut();
    }
    unsafe {
        state.write(Memory {
            data,
            capacity: 1,
            length: 0,
            position: 0,
            buffer_out: buffer,
            size_out: size,
        });
        (*state).publish();
        let file = cookie::kinakaze_abi_fopencookie(
            state.cast(),
            c"w".as_ptr(),
            cookie::Functions {
                read: None,
                write: Some(write),
                seek: Some(seek),
                close: Some(close),
            },
        );
        if file.is_null() {
            kinakaze_alloc::c::free(data);
            kinakaze_alloc::guest::free(state.cast());
        }
        file
    }
}
