// Generated object layout query; PE export tables do not carry data sizes.
// Buffers are borrowed only for this call. No memory ownership crosses the ABI.
#[rustfmt::skip]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kinakaze_module_object_v1(name: *const u8, length: usize) -> u64 {
    if name.is_null() || length > 128 { return 0; }
    match unsafe { core::slice::from_raw_parts(name, length) } {
        b"__signgam" => (4u64 << 32) | 4,
        b"signgam" => (4u64 << 32) | 4,
        _ => 0,
    }
}
