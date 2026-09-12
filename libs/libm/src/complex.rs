//! System V AMD64 Complex Math Implementation.
//!
//! Handles single-precision (Complex32 in xmm0), double-precision (Complex64 in
//! xmm0, xmm1), and extended-precision (long double complex in ST(0), ST(1)).

#[cfg(target_arch = "x86_64")]
use core::ffi::c_void;

/// SysV passes double _Complex as its real and imaginary SSE arguments.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_carg")]
pub extern "sysv64" fn carg(real: f64, imaginary: f64) -> f64 {
    musl_libm::atan2(imaginary, real)
}

// ---------------------------------------------------------------------------
// Single-precision float _Complex (Complex32)
// Returned packed in xmm0 (low 32-bit: real, high 32-bit: imag)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_cexpf(r: f32, i: f32, out: *mut [f32; 2]) {
    let exp_r = musl_libm::expf(r);
    let s = musl_libm::sinf(i);
    let c = musl_libm::cosf(i);
    unsafe {
        *out = [exp_r * c, exp_r * s];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_csinf(r: f32, i: f32, out: *mut [f32; 2]) {
    let s = musl_libm::sinf(r);
    let c = musl_libm::cosf(r);
    let sh = musl_libm::sinhf(i);
    let ch = musl_libm::coshf(i);
    unsafe {
        *out = [s * ch, c * sh];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_ccosf(r: f32, i: f32, out: *mut [f32; 2]) {
    let s = musl_libm::sinf(r);
    let c = musl_libm::cosf(r);
    let sh = musl_libm::sinhf(i);
    let ch = musl_libm::coshf(i);
    unsafe {
        *out = [c * ch, -s * sh];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_csqrtf(r: f32, i: f32, out: *mut [f32; 2]) {
    let mag = musl_libm::hypotf(r, i);
    let real = musl_libm::sqrtf((mag + r) * 0.5);
    let imag = musl_libm::copysignf(musl_libm::sqrtf((mag - r) * 0.5), i);
    unsafe {
        *out = [real, imag];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_clogf(r: f32, i: f32, out: *mut [f32; 2]) {
    let mag = musl_libm::hypotf(r, i);
    let angle = musl_libm::atan2f(i, r);
    unsafe {
        *out = [musl_libm::logf(mag), angle];
    }
}

#[cfg(target_arch = "x86_64")]
macro_rules! complex32_stub {
    ($name:ident, $algo:ident) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 24",
                "movq qword ptr [rsp], xmm0",
                "mov eax, dword ptr [rsp]",
                "movd xmm0, eax",
                "mov eax, dword ptr [rsp + 4]",
                "movd xmm1, eax",
                "lea rdi, [rsp + 8]",
                "call {compute}",
                "movq xmm0, qword ptr [rsp + 8]",
                "add rsp, 24",
                "ret",
                compute = sym $algo,
            );
        }
    };
}

#[cfg(target_arch = "x86_64")]
complex32_stub!(cexpf, compute_cexpf);
#[cfg(target_arch = "x86_64")]
complex32_stub!(csinf, compute_csinf);
#[cfg(target_arch = "x86_64")]
complex32_stub!(ccosf, compute_ccosf);
#[cfg(target_arch = "x86_64")]
complex32_stub!(csqrtf, compute_csqrtf);
#[cfg(target_arch = "x86_64")]
complex32_stub!(clogf, compute_clogf);

// ---------------------------------------------------------------------------
// Double-precision double _Complex (Complex64)
// Returned with real in xmm0, imag in xmm1
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
pub(crate) extern "sysv64" fn compute_cexp(r: f64, i: f64, out: *mut [f64; 2]) {
    let exp_r = musl_libm::exp(r);
    let s = musl_libm::sin(i);
    let c = musl_libm::cos(i);
    unsafe {
        *out = [exp_r * c, exp_r * s];
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) extern "sysv64" fn compute_csin(r: f64, i: f64, out: *mut [f64; 2]) {
    let s = musl_libm::sin(r);
    let c = musl_libm::cos(r);
    let sh = musl_libm::sinh(i);
    let ch = musl_libm::cosh(i);
    unsafe {
        *out = [s * ch, c * sh];
    }
}

#[cfg(target_arch = "x86_64")]
pub(crate) extern "sysv64" fn compute_ccos(r: f64, i: f64, out: *mut [f64; 2]) {
    let s = musl_libm::sin(r);
    let c = musl_libm::cos(r);
    let sh = musl_libm::sinh(i);
    let ch = musl_libm::cosh(i);
    unsafe {
        *out = [c * ch, -s * sh];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_csqrt(r: f64, i: f64, out: *mut [f64; 2]) {
    let mag = musl_libm::hypot(r, i);
    let real = musl_libm::sqrt((mag + r) * 0.5);
    let imag = musl_libm::copysign(musl_libm::sqrt((mag - r) * 0.5), i);
    unsafe {
        *out = [real, imag];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_clog(r: f64, i: f64, out: *mut [f64; 2]) {
    let mag = musl_libm::hypot(r, i);
    let angle = musl_libm::atan2(i, r);
    unsafe {
        *out = [musl_libm::log(mag), angle];
    }
}

#[cfg(target_arch = "x86_64")]
macro_rules! complex64_stub {
    ($name:ident, $algo:ident) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 24",
                "lea rdi, [rsp]",
                "call {compute}",
                "movsd xmm0, qword ptr [rsp]",
                "movsd xmm1, qword ptr [rsp + 8]",
                "add rsp, 24",
                "ret",
                compute = sym $algo,
            );
        }
    };
}

#[cfg(target_arch = "x86_64")]
complex64_stub!(cexp, compute_cexp);
#[cfg(target_arch = "x86_64")]
complex64_stub!(csin, compute_csin);
#[cfg(target_arch = "x86_64")]
complex64_stub!(ccos, compute_ccos);
#[cfg(target_arch = "x86_64")]
complex64_stub!(csqrt, compute_csqrt);
#[cfg(target_arch = "x86_64")]
complex64_stub!(clog, compute_clog);

// ---------------------------------------------------------------------------
// Extended-precision long double _Complex
// Input on stack at [rsp + 8] (real) and [rsp + 24] (imag)
// Returned in ST(0) (real) and ST(1) (imag)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_cexpl(input_stack: *const c_void, out: *mut [f64; 2]) {
    let mut real_f64 = 0.0f64;
    let mut imag_f64 = 0.0f64;
    unsafe {
        core::arch::asm!(
            "fld tbyte ptr [{in_ptr}]",
            "fstp qword ptr [{r_ptr}]",
            "fld tbyte ptr [{in_ptr} + 16]",
            "fstp qword ptr [{i_ptr}]",
            in_ptr = in(reg) input_stack,
            r_ptr = in(reg) &raw mut real_f64,
            i_ptr = in(reg) &raw mut imag_f64,
        );
    }
    let exp_r = musl_libm::exp(real_f64);
    let s = musl_libm::sin(imag_f64);
    let c = musl_libm::cos(imag_f64);
    unsafe {
        *out = [exp_r * c, exp_r * s];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_csinl(input_stack: *const c_void, out: *mut [f64; 2]) {
    let mut real_f64 = 0.0f64;
    let mut imag_f64 = 0.0f64;
    unsafe {
        core::arch::asm!(
            "fld tbyte ptr [{in_ptr}]",
            "fstp qword ptr [{r_ptr}]",
            "fld tbyte ptr [{in_ptr} + 16]",
            "fstp qword ptr [{i_ptr}]",
            in_ptr = in(reg) input_stack,
            r_ptr = in(reg) &raw mut real_f64,
            i_ptr = in(reg) &raw mut imag_f64,
        );
    }
    let s = musl_libm::sin(real_f64);
    let c = musl_libm::cos(real_f64);
    let sh = musl_libm::sinh(imag_f64);
    let ch = musl_libm::cosh(imag_f64);
    unsafe {
        *out = [s * ch, c * sh];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_ccosl(input_stack: *const c_void, out: *mut [f64; 2]) {
    let mut real_f64 = 0.0f64;
    let mut imag_f64 = 0.0f64;
    unsafe {
        core::arch::asm!(
            "fld tbyte ptr [{in_ptr}]",
            "fstp qword ptr [{r_ptr}]",
            "fld tbyte ptr [{in_ptr} + 16]",
            "fstp qword ptr [{i_ptr}]",
            in_ptr = in(reg) input_stack,
            r_ptr = in(reg) &raw mut real_f64,
            i_ptr = in(reg) &raw mut imag_f64,
        );
    }
    let s = musl_libm::sin(real_f64);
    let c = musl_libm::cos(real_f64);
    let sh = musl_libm::sinh(imag_f64);
    let ch = musl_libm::cosh(imag_f64);
    unsafe {
        *out = [c * ch, -s * sh];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_csqrtl(input_stack: *const c_void, out: *mut [f64; 2]) {
    let mut real_f64 = 0.0f64;
    let mut imag_f64 = 0.0f64;
    unsafe {
        core::arch::asm!(
            "fld tbyte ptr [{in_ptr}]",
            "fstp qword ptr [{r_ptr}]",
            "fld tbyte ptr [{in_ptr} + 16]",
            "fstp qword ptr [{i_ptr}]",
            in_ptr = in(reg) input_stack,
            r_ptr = in(reg) &raw mut real_f64,
            i_ptr = in(reg) &raw mut imag_f64,
        );
    }
    let mag = musl_libm::hypot(real_f64, imag_f64);
    let real = musl_libm::sqrt((mag + real_f64) * 0.5);
    let imag = musl_libm::copysign(musl_libm::sqrt((mag - real_f64) * 0.5), imag_f64);
    unsafe {
        *out = [real, imag];
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn compute_clogl(input_stack: *const c_void, out: *mut [f64; 2]) {
    let mut real_f64 = 0.0f64;
    let mut imag_f64 = 0.0f64;
    unsafe {
        core::arch::asm!(
            "fld tbyte ptr [{in_ptr}]",
            "fstp qword ptr [{r_ptr}]",
            "fld tbyte ptr [{in_ptr} + 16]",
            "fstp qword ptr [{i_ptr}]",
            in_ptr = in(reg) input_stack,
            r_ptr = in(reg) &raw mut real_f64,
            i_ptr = in(reg) &raw mut imag_f64,
        );
    }
    let mag = musl_libm::hypot(real_f64, imag_f64);
    let angle = musl_libm::atan2(imag_f64, real_f64);
    unsafe {
        *out = [musl_libm::log(mag), angle];
    }
}

#[cfg(target_arch = "x86_64")]
macro_rules! complexl_stub {
    ($name:ident, $algo:ident) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 40",
                "lea rdi, [rsp + 48]",
                "lea rsi, [rsp]",
                "call {compute}",
                "fld qword ptr [rsp + 8]",
                "fld qword ptr [rsp]",
                "add rsp, 40",
                "ret",
                compute = sym $algo,
            );
        }
    };
}

#[cfg(target_arch = "x86_64")]
complexl_stub!(cexpl, compute_cexpl);
#[cfg(target_arch = "x86_64")]
complexl_stub!(csinl, compute_csinl);
#[cfg(target_arch = "x86_64")]
complexl_stub!(ccosl, compute_ccosl);
#[cfg(target_arch = "x86_64")]
complexl_stub!(csqrtl, compute_csqrtl);
#[cfg(target_arch = "x86_64")]
complexl_stub!(clogl, compute_clogl);

/// `cabsl`: complex absolute value for long double (returns in ST(0)).
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cabsl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn cabsl() {
    core::arch::naked_asm!(
        "fld tbyte ptr [rsp + 8]",
        "fmul st(0), st(0)",
        "fld tbyte ptr [rsp + 24]",
        "fmul st(0), st(0)",
        "faddp",
        "fsqrt",
        "ret",
    );
}
