//! Inspect NaN encodings without performing floating-point operations.
#[unsafe(export_name = "kinakaze_engine_libm___issignaling")]
pub extern "sysv64" fn double(value: f64) -> i32 {
    let bits = value.to_bits() & 0x7fff_ffff_ffff_ffff;
    i32::from(bits > 0x7ff0_0000_0000_0000 && bits & 0x0008_0000_0000_0000 == 0)
}
#[unsafe(export_name = "kinakaze_engine_libm___issignalingf")]
pub extern "sysv64" fn float(value: f32) -> i32 {
    let bits = value.to_bits() & 0x7fff_ffff;
    i32::from(bits > 0x7f80_0000 && bits & 0x0040_0000 == 0)
}

#[unsafe(export_name = "kinakaze_engine_libm___issignalingl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn extended() {
    core::arch::naked_asm!(
        "mov rdi, qword ptr [rsp + 8]",
        "movzx esi, word ptr [rsp + 16]",
        "jmp {classify}", classify = sym extended_bits,
    );
}
extern "sysv64" fn extended_bits(significand: u64, exponent: u16) -> i32 {
    i32::from(
        exponent & 0x7fff == 0x7fff
            && significand & 0x7fff_ffff_ffff_ffff != 0
            && significand & 0x4000_0000_0000_0000 == 0,
    )
}

#[unsafe(export_name = "kinakaze_engine_libm___issignalingf128")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn quad() {
    core::arch::naked_asm!(
        "movq rdi, xmm0", "psrldq xmm0, 8", "movq rsi, xmm0",
        "jmp {classify}", classify = sym quad_bits,
    );
}
extern "sysv64" fn quad_bits(low: u64, high: u64) -> i32 {
    i32::from(
        high & 0x7fff_0000_0000_0000 == 0x7fff_0000_0000_0000
            && high & 0x0000_8000_0000_0000 == 0
            && (low != 0 || high & 0x0000_7fff_ffff_ffff != 0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quiet_nan_infinity_and_signaling_payloads() {
        assert_eq!(double(f64::from_bits(0x7ff0_0000_0000_0001)), 1);
        assert_eq!(double(f64::NAN), 0);
        assert_eq!(double(f64::INFINITY), 0);
        assert_eq!(float(f32::from_bits(0xff80_0001)), 1);
        assert_eq!(float(f32::NAN), 0);
        assert_eq!(extended_bits(0x8000_0000_0000_0001, 0x7fff), 1);
        assert_eq!(extended_bits(0xc000_0000_0000_0001, 0x7fff), 0);
        assert_eq!(quad_bits(1, 0xffff_0000_0000_0000), 1);
        assert_eq!(quad_bits(0, 0x7fff_0000_0000_0000), 0);
    }
}
