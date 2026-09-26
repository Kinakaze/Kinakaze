//! X SECURITY client ABI. The native display does not implement untrusted
//! authorization, so extension queries and authorization requests report its
//! absence. Local Xauthority file operations can still use the allocation API.
//! Contract: x.org/releases/X11R7.6/doc/xextproto/security.pdf, C binding.
use super::*;

#[repr(C)]
pub struct Xauth {
    family: u16,
    address_length: u16,
    address: *mut c_char,
    number_length: u16,
    number: *mut c_char,
    name_length: u16,
    name: *mut c_char,
    data_length: u16,
    data: *mut c_char,
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSecurityAllocXauth")]
pub extern "sysv64" fn XSecurityAllocXauth() -> *mut Xauth {
    let value = unsafe { guest::malloc(size_of::<Xauth>()).cast::<Xauth>() };
    if !value.is_null() {
        unsafe {
            ptr::write_bytes(value, 0, 1);
        }
    }
    value
}
/// # Safety
/// auth was returned by XSecurityAllocXauth. Its name/data remain caller owned.
#[unsafe(export_name = "kinakaze_engine_libXext_XSecurityFreeXauth")]
pub unsafe extern "sysv64" fn XSecurityFreeXauth(auth: *mut Xauth) {
    unsafe {
        guest::free(auth.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSecurityQueryExtension")]
pub extern "sysv64" fn XSecurityQueryExtension(
    _display: *mut Display,
    _major: *mut c_int,
    _minor: *mut c_int,
) -> Status {
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSecurityGenerateAuthorization")]
pub extern "sysv64" fn XSecurityGenerateAuthorization(
    _display: *mut Display,
    _auth: *mut Xauth,
    _mask: u64,
    _attributes: *const c_void,
    _id: *mut u64,
) -> *mut Xauth {
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libXext_XSecurityRevokeAuthorization")]
pub extern "sysv64" fn XSecurityRevokeAuthorization(_display: *mut Display, _id: u64) -> Bool {
    0
}
