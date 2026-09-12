//! Exact x87 long-double to signed-integer conversion at the System V boundary.
//!
//! Rust's Windows f64 cannot represent the input significand. Decode the two
//! payload words directly; valid rounding neither changes the rounding mode nor
//! raises FE_INEXACT. Invalid conversion raises FE_INVALID without changing errno,
//! matching Linux lround(3): https://man7.org/linux/man-pages/man3/lround.3.html

macro_rules! round_entry {
    ($name:ident) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "mov rdi, qword ptr [rsp + 8]",
                "movzx esi, word ptr [rsp + 16]",
                "jmp {convert}",
                convert = sym round_integer,
            );
        }
    };
}

// Both Linux long and long long are 64 bits on this ABI.
round_entry!(lroundl);
round_entry!(llroundl);

// Keep all 64 significand bits and return in x87 ST(0). Only the temporary
// rounding direction changes; the caller's complete control word is restored.
macro_rules! integral_entry {
    ($name:ident, $rounding:literal) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 24",
                "fnstcw word ptr [rsp]",
                "movzx eax, word ptr [rsp]",
                "and eax, 0xf3ff",
                concat!("or eax, ", $rounding),
                "mov word ptr [rsp + 2], ax",
                "fldcw word ptr [rsp + 2]",
                "fld tbyte ptr [rsp + 32]",
                "frndint",
                "fldcw word ptr [rsp]",
                "add rsp, 24",
                "ret",
            );
        }
    };
}
integral_entry!(ceill, "0x800");
integral_entry!(floorl, "0x400");
integral_entry!(truncl, "0xc00");

fn rounded_magnitude(significand: u64, exponent: u16) -> Option<u64> {
    if exponent == 0 {
        return Some(0);
    }
    if exponent == 0x7fff || significand >> 63 == 0 {
        return None;
    }
    let power = exponent as i32 - 16383;
    if power < -1 {
        return Some(0);
    }
    if power == -1 {
        return Some(1);
    }
    if power > 63 {
        return None;
    }
    if power == 63 {
        return Some(significand);
    }
    let shift = 63 - power as u32;
    let integer = significand >> shift;
    let halfway_bit = (significand >> (shift - 1)) & 1;
    Some(integer + halfway_bit)
}

extern "sysv64" fn round_integer(significand: u64, sign_exponent: u16) -> i64 {
    let negative = sign_exponent & 0x8000 != 0;
    if let Some(magnitude) = rounded_magnitude(significand, sign_exponent & 0x7fff) {
        if magnitude <= i64::MAX as u64 {
            return if negative {
                -(magnitude as i64)
            } else {
                magnitude as i64
            };
        }
        if negative && magnitude == 1 << 63 {
            return i64::MIN;
        }
    }
    super::feraiseexcept(1);
    i64::MIN
}

// ---------------------------------------------------------------------------
// Extended-precision x87 long double mathematical functions (SysV AMD64)
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libm_sqrtl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn sqrtl() {
    core::arch::naked_asm!("fld tbyte ptr [rsp + 8]", "fsqrt", "ret",);
}

#[unsafe(export_name = "kinakaze_engine_libm_sinl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn sinl() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fsin",
        "fnstsw ax",
        "test ah, 4",
        "jz 2f",
        "1:",
        "fldpi",
        "fadd st(0), st(0)",
        "fxch st(1)",
        "fprem1",
        "fstp st(1)",
        "fsin",
        "fnstsw ax",
        "test ah, 4",
        "jnz 1b",
        "2:",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_cosl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn cosl() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fcos",
        "fnstsw ax",
        "test ah, 4",
        "jz 2f",
        "1:",
        "fldpi",
        "fadd st(0), st(0)",
        "fxch st(1)",
        "fprem1",
        "fstp st(1)",
        "fcos",
        "fnstsw ax",
        "test ah, 4",
        "jnz 1b",
        "2:",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_tanl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn tanl() {
    core::arch::naked_asm!("fld tbyte ptr [rsp + 8]", "fptan", "fstp st(0)", "ret",);
}

#[unsafe(export_name = "kinakaze_engine_libm_asinl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn asinl() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fld st(0)",
        "fld st(0)",
        "fmulp",
        "fld1",
        "fsubp",
        "fsqrt",
        "fpatan",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_acosl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn acosl() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fld st(0)",
        "fld st(0)",
        "fmulp",
        "fld1",
        "fsubp",
        "fsqrt",
        "fxch st(1)",
        "fpatan",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_atanl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn atanl() {
    core::arch::naked_asm!("fld tbyte ptr [rsp + 8]", "fld1", "fpatan", "ret",);
}

#[unsafe(export_name = "kinakaze_engine_libm_atan2l")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn atan2l() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fld tbyte ptr [rsp + 24]",
        "fpatan",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_expl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn expl() {
    core::arch::naked_asm!(
        "fldl2e",
        "fld tbyte ptr [rsp + 8]",
        "fmulp",
        "fld st(0)",
        "frndint",
        "fsub st(1), st(0)",
        "fxch st(1)",
        "f2xm1",
        "fld1",
        "faddp",
        "fscale",
        "fstp st(1)",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_logl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn logl() {
    core::arch::naked_asm!("fldln2", "fld tbyte ptr [rsp + 8]", "fyl2x", "ret",);
}

#[unsafe(export_name = "kinakaze_engine_libm_log10l")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn log10l() {
    core::arch::naked_asm!("fldlg2", "fld tbyte ptr [rsp + 8]", "fyl2x", "ret",);
}

#[unsafe(export_name = "kinakaze_engine_libm_fmodl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn fmodl() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 24]",
        "fld tbyte ptr [rsp + 8]",
        "1:",
        "fprem",
        "fnstsw ax",
        "sahf",
        "jp 1b",
        "fstp st(1)",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_roundl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn roundl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fnstcw word ptr [rsp]",
        "movzx eax, word ptr [rsp]",
        "and eax, 0xf3ff",
        "or eax, 0xc00",
        "mov word ptr [rsp + 2], ax",
        "fldcw word ptr [rsp + 2]",
        "fld tbyte ptr [rsp + 32]",
        "ftst",
        "fnstsw ax",
        "sahf",
        "mov dword ptr [rsp + 4], 0x3f000000",
        "fld dword ptr [rsp + 4]",
        "jb 1f",
        "faddp",
        "frndint",
        "jmp 2f",
        "1:",
        "fsubp",
        "frndint",
        "2:",
        "fldcw word ptr [rsp]",
        "add rsp, 24",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_sinhl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn sinhl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {sinh}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        sinh = sym musl_libm::sinh,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_coshl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn coshl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {cosh}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        cosh = sym musl_libm::cosh,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_tanhl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn tanhl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {tanh}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        tanh = sym musl_libm::tanh,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_asinhl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn asinhl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {asinh}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        asinh = sym musl_libm::asinh,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_acoshl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn acoshl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {acosh}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        acosh = sym musl_libm::acosh,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_atanhl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn atanhl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {atanh}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        atanh = sym musl_libm::atanh,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_fmal")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn fmal() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fld tbyte ptr [rsp + 24]",
        "fmulp",
        "fld tbyte ptr [rsp + 40]",
        "faddp",
        "ret",
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_jnl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn jnl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {jn}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        jn = sym crate::special::jn,
    );
}

#[unsafe(export_name = "kinakaze_engine_libm_ynl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn ynl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "call {yn}",
        "movsd qword ptr [rsp], xmm0",
        "fld qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        yn = sym crate::special::yn,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_fraction_and_integer_boundaries_do_not_round_through_f64() {
        assert_eq!(rounded_magnitude(0xffff_ffff_ffff_ffff, 16381), Some(0));
        assert_eq!(rounded_magnitude(0x8000_0000_0000_0000, 16382), Some(1));
        assert_eq!(rounded_magnitude(0xa000_0000_0000_0000, 16384), Some(3));
        assert_eq!(rounded_magnitude(0x9fff_ffff_ffff_ffff, 16384), Some(2));
        assert_eq!(
            rounded_magnitude(0xffff_ffff_ffff_fffd, 16445),
            Some(i64::MAX as u64)
        );
        assert_eq!(
            rounded_magnitude(0xffff_ffff_ffff_ffff, 16445),
            Some(1 << 63)
        );
        assert_eq!(rounded_magnitude(1, 0), Some(0));
        assert_eq!(rounded_magnitude(0xc000_0000_0000_0000, 0x7fff), None);
        assert_eq!(rounded_magnitude(1, 16383), None);
    }
}
