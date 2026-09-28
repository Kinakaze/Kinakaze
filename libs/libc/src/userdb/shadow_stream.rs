//! Parse shadow records from a guest FILE into caller-owned storage.
use super::*;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetspent_r(
    file: *mut crate::stdio::File,
    entry: *mut Spwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Spwd,
) -> c_int {
    unsafe {
        super::stream::read(
            file,
            entry,
            buffer,
            length,
            result,
            super::nss_parse::kinakaze_abi__nss_files_parse_spent,
        )
    }
}
