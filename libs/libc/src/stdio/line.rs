//! Bounded and dynamically sized line reads under one FILE lock.
use super::{File, with_oriented};
use core::{
    ffi::{c_char, c_int},
    ptr,
};
use kinakaze_vfs::{EAGAIN, ENOENT, ENOMEM, EOVERFLOW};

unsafe fn reserve(buffer: &mut *mut c_char, capacity: &mut usize, need: usize) -> Result<(), i32> {
    if need > isize::MAX as usize {
        return Err(EOVERFLOW);
    }
    if !buffer.is_null() && need <= *capacity {
        return Ok(());
    }
    let previous = if buffer.is_null() { 0 } else { *capacity };
    let size = need
        .max(previous.saturating_mul(2).min(isize::MAX as usize))
        .max(120);
    let grown = unsafe { kinakaze_alloc::c::realloc((*buffer).cast(), size) };
    if grown.is_null() {
        return Err(ENOMEM);
    }
    *buffer = grown.cast();
    *capacity = size;
    Ok(())
}

/// One FILE lock per line, scanning and copying the existing input buffer in
/// blocks. memchr uses the bounded SSE2/AVX2 path; unread bytes stay in FILE.
pub(crate) unsafe fn getdelim(
    file: *mut File,
    buffer: *mut *mut c_char,
    capacity: *mut usize,
    delimiter: u8,
) -> isize {
    with_oriented(file, -1, -1, |stream| {
        let (buffer, capacity) = unsafe { (&mut *buffer, &mut *capacity) };
        let mut length = 0usize;
        loop {
            if let Err(error) = unsafe { reserve(buffer, capacity, length + 2) } {
                stream.error = true;
                crate::set_errno(error);
                return -1;
            }
            if stream.pushback.is_empty() && stream.input_pos < stream.input_end {
                let available = stream.input_end - stream.input_pos;
                let source = unsafe { stream.input.as_ptr().add(stream.input_pos) };
                let end =
                    unsafe { crate::string::memchr(source.cast(), delimiter as c_int, available) };
                let count = if end.is_null() {
                    available
                } else {
                    end as usize - source as usize + 1
                };
                let need = length.saturating_add(count).saturating_add(1);
                if let Err(error) = unsafe { reserve(buffer, capacity, need) } {
                    stream.error = true;
                    crate::set_errno(error);
                    return -1;
                }
                unsafe { ptr::copy_nonoverlapping(source, (*buffer).add(length).cast(), count) };
                length += count;
                stream.input_pos += count;
                if !end.is_null() {
                    break;
                }
            } else {
                // Refill, pushback and unbuffered/cookie streams retain the
                // same I/O/error behavior as fgetc, without reacquiring FILE.
                let mut byte = [0];
                match stream.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) => {
                        unsafe { (*buffer).add(length).write(byte[0] as c_char) };
                        length += 1;
                        if byte[0] == delimiter {
                            break;
                        }
                    }
                    Err(error) => {
                        crate::set_errno(error);
                        break;
                    }
                }
            }
        }
        if length == 0 {
            return -1;
        }
        unsafe { (*buffer).add(length).write(0) };
        length as isize
    })
}

unsafe extern "sysv64" {
    safe fn kinakaze_abi___chk_fail() -> !;
}

pub(crate) unsafe fn checked_fgets(
    buffer: *mut c_char,
    capacity: usize,
    count: c_int,
    file: *mut File,
) -> *mut c_char {
    if count <= 0 {
        return ptr::null_mut();
    }
    with_oriented(file, -1, ptr::null_mut(), |stream| {
        let old_error = stream.error;
        stream.error = false;
        let mut length = 0;
        let limit = capacity.min(count as usize - 1);
        let mut failed = false;
        while length < limit {
            let mut byte = [0];
            match stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    unsafe { buffer.add(length).write(byte[0] as c_char) };
                    length += 1;
                    if byte[0] == b'\n' {
                        break;
                    }
                }
                Err(errno) => {
                    crate::set_errno(errno);
                    failed = errno != EAGAIN;
                    break;
                }
            }
        }
        stream.error |= old_error;
        if length == 0 || failed {
            return ptr::null_mut();
        }
        if length >= capacity {
            kinakaze_abi___chk_fail();
        }
        unsafe { buffer.add(length).write(0) };
        buffer
    })
}

/// Grow guest-owned storage while reading a complete account database line.
/// No fixed line limit, and no unlocked gap between characters.
pub(crate) unsafe fn account_line(
    file: *mut File,
    buffer: &mut *mut c_char,
    capacity: &mut usize,
) -> Result<usize, i32> {
    with_oriented(file, -1, Err(kinakaze_vfs::EBADF), |stream| {
        let mut length = 0usize;
        loop {
            if length.checked_add(2).ok_or(EOVERFLOW)? > *capacity {
                let size = capacity.checked_mul(2).ok_or(EOVERFLOW)?.max(256);
                let grown =
                    unsafe { kinakaze_alloc::guest::reallocate((*buffer).cast(), 16, size) };
                if grown.is_null() {
                    return Err(ENOMEM);
                }
                *buffer = grown.cast();
                *capacity = size;
            }
            let mut byte = [0];
            match stream.read(&mut byte)? {
                0 if length == 0 => return Err(ENOENT),
                0 => break,
                _ => {
                    unsafe { (*buffer).add(length).write(byte[0] as c_char) };
                    length += 1;
                    if byte[0] == b'\n' {
                        break;
                    }
                }
            }
        }
        unsafe { (*buffer).add(length).write(0) };
        Ok(length)
    })
}
