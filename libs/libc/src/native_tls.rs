//! The PE symbol only identifies an ELF TLS export. Its query resolves the
//! module/offset; no guest relocation ever points at this process-global tag.
#[unsafe(no_mangle)]
pub static kinakaze_abi_errno: i32 = 0;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_module_tls_v1(
    name: *const u8,
    length: usize,
    module: *mut usize,
    offset: *mut usize,
) -> bool {
    if name.is_null()
        || module.is_null()
        || offset.is_null()
        || length != 5
        || unsafe { core::slice::from_raw_parts(name, length) } != b"errno"
    {
        return false;
    }
    let Ok(id) = kinakaze_tls::errno_module() else {
        return false;
    };
    unsafe {
        module.write(id);
        offset.write(0);
    }
    true
}
