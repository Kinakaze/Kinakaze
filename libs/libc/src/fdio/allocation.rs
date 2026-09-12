//! C return and errno conventions over the VFS allocation operation.
use super::*;

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fallocate(
    fd: c_int,
    mode: c_int,
    offset: i64,
    length: i64,
) -> c_int {
    match fs::fallocate(fd, mode, offset, length) {
        Ok(()) => 0,
        Err(error) => {
            set_errno(error);
            -1
        }
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_fallocate64(
    fd: c_int,
    mode: c_int,
    offset: i64,
    length: i64,
) -> c_int {
    kinakaze_abi_fallocate(fd, mode, offset, length)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_posix_fallocate(fd: c_int, offset: i64, length: i64) -> c_int {
    // POSIX returns the error itself and does not write errno.
    fs::fallocate(fd, 0, offset, length).err().unwrap_or(0)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_posix_fallocate64(
    fd: c_int,
    offset: i64,
    length: i64,
) -> c_int {
    kinakaze_abi_posix_fallocate(fd, offset, length)
}
