// Generated object layout query; PE export tables do not carry data sizes.
// Buffers are borrowed only for this call. No memory ownership crosses the ABI.
#[rustfmt::skip]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_module_object_v1(name: *const u8, length: usize) -> u64 {
    if name.is_null() || length > 128 { return 0; }
    match unsafe { core::slice::from_raw_parts(name, length) } {
        b"_XLockMutex_fn" => (8u64 << 32) | 8,
        b"_XUnlockMutex_fn" => (8u64 << 32) | 8,
        b"_Xglobal_lock" => (8u64 << 32) | 8,
        _ => 0,
    }
}
