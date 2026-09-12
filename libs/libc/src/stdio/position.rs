//! Linux AMD64 fpos_t: file offset followed by the eight-byte conversion state.
use super::{EOF, File, SEEK_SET, fseek, ftell};
use core::ffi::c_int;

#[repr(C)]
pub struct Position {
    offset: i64,
    state: [u32; 2],
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetpos(
    file: *mut File,
    position: *mut Position,
) -> c_int {
    if position.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return EOF;
    }
    let offset = ftell(file);
    if offset == -1 {
        return EOF;
    }
    // C and UTF-8 have no shift state between complete stream characters.
    unsafe {
        position.write(Position {
            offset,
            state: [0; 2],
        });
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fsetpos(
    file: *mut File,
    position: *const Position,
) -> c_int {
    if position.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return EOF;
    }
    fseek(file, unsafe { (*position).offset }, SEEK_SET)
}
