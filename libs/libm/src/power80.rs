//! System V AMD64 passes two 16-byte long doubles on the stack and returns in
//! x87 ST(0). Windows Clang uses a hidden result pointer even with sysv_abi;
//! marshal storage explicitly instead of truncating either input through f64.
unsafe extern "win64" {
    fn kinakaze_powl80(x: *const u8, y: *const u8, output: *mut u8);
}

#[unsafe(export_name = "kinakaze_powl_set_errno")]
pub extern "win64" fn set_errno(error: i32) {
    kinakaze_tls::set_errno(error);
}

#[unsafe(export_name = "kinakaze_engine_libm_powl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn powl() {
    core::arch::naked_asm!(
        "sub rsp, 56",
        "lea rcx, [rsp + 64]",
        "lea rdx, [rsp + 80]",
        "lea r8, [rsp + 32]",
        "call {calculate}",
        "fld tbyte ptr [rsp + 32]",
        "add rsp, 56",
        "ret",
        calculate = sym kinakaze_powl80,
    );
}
