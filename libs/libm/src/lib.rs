//! Initial libm.dll System V ABI surface.
//!
//! Every export uses the System V AMD64 ABI so ELF guest code can call it
//! directly. Math functions forward explicitly to the musl-derived `libm`
//! crate. Do not implement an exported function with its corresponding
//! `f64`/`f32` method: LLVM can lower that method back to the same C symbol in
//! this DLL, causing unbounded self-recursion.

#[cfg(target_arch = "x86_64")]
use core::ffi::{c_char, c_int};

#[cfg(target_arch = "x86_64")]
mod complex;
#[cfg(target_arch = "x86_64")]
mod extended_integer;
#[cfg(target_arch = "x86_64")]
mod power80;
#[cfg(target_arch = "x86_64")]
mod signaling;
#[cfg(target_arch = "x86_64")]
mod special;

#[cfg(target_arch = "x86_64")]
pub use complex::*;

/// `signgam`: sign of gamma function for lgamma(3).
#[unsafe(no_mangle)]
pub static mut kinakaze_engine_libm_signgam: c_int = 0;

/// `FP_NAN`: the value is not a number.
pub const FP_NAN: i32 = 0;
/// `FP_INFINITE`: the value is positive or negative infinity.
pub const FP_INFINITE: i32 = 1;
/// `FP_ZERO`: the value is positive or negative zero.
pub const FP_ZERO: i32 = 2;
/// `FP_SUBNORMAL`: the value is too small to be represented normally.
pub const FP_SUBNORMAL: i32 = 3;
/// `FP_NORMAL`: the value is an ordinary finite number.
pub const FP_NORMAL: i32 = 4;

// ---------------------------------------------------------------------------
// Trigonometric
// ---------------------------------------------------------------------------

/// Computes the sine of `value` in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sin")]
pub extern "sysv64" fn sin(value: f64) -> f64 {
    musl_libm::sin(value)
}

/// Computes the sine of `value` in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sinf")]
pub extern "sysv64" fn sinf(value: f32) -> f32 {
    musl_libm::sinf(value)
}

/// Computes the cosine of `value` in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cos")]
pub extern "sysv64" fn cos(value: f64) -> f64 {
    musl_libm::cos(value)
}

/// Computes the cosine of `value` in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cosf")]
pub extern "sysv64" fn cosf(value: f32) -> f32 {
    musl_libm::cosf(value)
}

/// Computes the tangent of `value` in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_tan")]
pub extern "sysv64" fn tan(value: f64) -> f64 {
    musl_libm::tan(value)
}

/// Computes the tangent of `value` in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_tanf")]
pub extern "sysv64" fn tanf(value: f32) -> f32 {
    musl_libm::tanf(value)
}

/// Computes the arcsine of `value`, in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_asin")]
pub extern "sysv64" fn asin(value: f64) -> f64 {
    musl_libm::asin(value)
}

/// Computes the arcsine of `value`, in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_asinf")]
pub extern "sysv64" fn asinf(value: f32) -> f32 {
    musl_libm::asinf(value)
}

/// Computes the arccosine of `value`, in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_acos")]
pub extern "sysv64" fn acos(value: f64) -> f64 {
    musl_libm::acos(value)
}

/// Computes the arccosine of `value`, in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_acosf")]
pub extern "sysv64" fn acosf(value: f32) -> f32 {
    musl_libm::acosf(value)
}

/// Computes the arctangent of `value`, in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_atan")]
pub extern "sysv64" fn atan(value: f64) -> f64 {
    musl_libm::atan(value)
}

/// Computes the arctangent of `value`, in radians.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_atanf")]
pub extern "sysv64" fn atanf(value: f32) -> f32 {
    musl_libm::atanf(value)
}

/// Computes the arctangent of `y / x`, using the signs of both arguments to
/// select the correct quadrant.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_atan2")]
pub extern "sysv64" fn atan2(y: f64, x: f64) -> f64 {
    musl_libm::atan2(y, x)
}

/// Computes the arctangent of `y / x`, using the signs of both arguments to
/// select the correct quadrant.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_atan2f")]
pub extern "sysv64" fn atan2f(y: f32, x: f32) -> f32 {
    musl_libm::atan2f(y, x)
}

// ---------------------------------------------------------------------------
// Hyperbolic
// ---------------------------------------------------------------------------

/// Computes the hyperbolic sine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sinh")]
pub extern "sysv64" fn sinh(value: f64) -> f64 {
    musl_libm::sinh(value)
}

/// Computes the hyperbolic sine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sinhf")]
pub extern "sysv64" fn sinhf(value: f32) -> f32 {
    musl_libm::sinhf(value)
}

/// Computes the hyperbolic cosine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cosh")]
pub extern "sysv64" fn cosh(value: f64) -> f64 {
    musl_libm::cosh(value)
}

/// Computes the hyperbolic cosine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_coshf")]
pub extern "sysv64" fn coshf(value: f32) -> f32 {
    musl_libm::coshf(value)
}

/// Computes the hyperbolic tangent of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_tanh")]
pub extern "sysv64" fn tanh(value: f64) -> f64 {
    musl_libm::tanh(value)
}

/// Computes the hyperbolic tangent of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_tanhf")]
pub extern "sysv64" fn tanhf(value: f32) -> f32 {
    musl_libm::tanhf(value)
}

/// Computes the inverse hyperbolic sine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_asinh")]
pub extern "sysv64" fn asinh(value: f64) -> f64 {
    musl_libm::asinh(value)
}

/// Computes the inverse hyperbolic sine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_asinhf")]
pub extern "sysv64" fn asinhf(value: f32) -> f32 {
    musl_libm::asinhf(value)
}

/// Computes the inverse hyperbolic cosine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_acosh")]
pub extern "sysv64" fn acosh(value: f64) -> f64 {
    musl_libm::acosh(value)
}

/// Computes the inverse hyperbolic cosine of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_acoshf")]
pub extern "sysv64" fn acoshf(value: f32) -> f32 {
    musl_libm::acoshf(value)
}

/// Computes the inverse hyperbolic tangent of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_atanh")]
pub extern "sysv64" fn atanh(value: f64) -> f64 {
    musl_libm::atanh(value)
}

/// Computes the inverse hyperbolic tangent of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_atanhf")]
pub extern "sysv64" fn atanhf(value: f32) -> f32 {
    musl_libm::atanhf(value)
}

// ---------------------------------------------------------------------------
// Exponential and logarithmic
// ---------------------------------------------------------------------------

/// Computes `e` raised to the power of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_exp")]
pub extern "sysv64" fn exp(value: f64) -> f64 {
    musl_libm::exp(value)
}

/// Computes `e` raised to the power of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_expf")]
pub extern "sysv64" fn expf(value: f32) -> f32 {
    musl_libm::expf(value)
}

/// Computes `2` raised to the power of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_exp2")]
pub extern "sysv64" fn exp2(value: f64) -> f64 {
    musl_libm::exp2(value)
}

/// Computes `2` raised to the power of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_exp2f")]
pub extern "sysv64" fn exp2f(value: f32) -> f32 {
    musl_libm::exp2f(value)
}

/// Computes `e**value - 1`, staying accurate for `value` near zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_expm1")]
pub extern "sysv64" fn expm1(value: f64) -> f64 {
    musl_libm::expm1(value)
}

/// Computes `e**value - 1`, staying accurate for `value` near zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_expm1f")]
pub extern "sysv64" fn expm1f(value: f32) -> f32 {
    musl_libm::expm1f(value)
}

/// Computes the natural logarithm of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log")]
pub extern "sysv64" fn log(value: f64) -> f64 {
    musl_libm::log(value)
}

/// Computes the natural logarithm of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_logf")]
pub extern "sysv64" fn logf(value: f32) -> f32 {
    musl_libm::logf(value)
}

/// Computes the base-2 logarithm of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log2")]
pub extern "sysv64" fn log2(value: f64) -> f64 {
    musl_libm::log2(value)
}

/// Computes the base-2 logarithm of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log2f")]
pub extern "sysv64" fn log2f(value: f32) -> f32 {
    musl_libm::log2f(value)
}

/// Computes the base-10 logarithm of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log10")]
pub extern "sysv64" fn log10(value: f64) -> f64 {
    musl_libm::log10(value)
}

/// Computes the base-10 logarithm of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log10f")]
pub extern "sysv64" fn log10f(value: f32) -> f32 {
    musl_libm::log10f(value)
}

/// Computes `ln(1 + value)`, staying accurate for `value` near zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log1p")]
pub extern "sysv64" fn log1p(value: f64) -> f64 {
    musl_libm::log1p(value)
}

/// Computes `ln(1 + value)`, staying accurate for `value` near zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_log1pf")]
pub extern "sysv64" fn log1pf(value: f32) -> f32 {
    musl_libm::log1pf(value)
}

// ---------------------------------------------------------------------------
// Power and roots
// ---------------------------------------------------------------------------

/// Raises `base` to the power `exponent`.
///
/// Forwarded to musl libm, which implements the C99 special cases such as
/// `pow(1.0, NAN) == 1.0` and `pow(NAN, 0.0) == 1.0`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_pow")]
pub extern "sysv64" fn pow(base: f64, exponent: f64) -> f64 {
    musl_libm::pow(base, exponent)
}

/// Raises `base` to the power `exponent`.
///
/// Forwarded to musl libm, which implements the C99 special cases such as
/// `powf(1.0, NAN) == 1.0` and `powf(NAN, 0.0) == 1.0`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_powf")]
pub extern "sysv64" fn powf(base: f32, exponent: f32) -> f32 {
    musl_libm::powf(base, exponent)
}

/// Computes the square root of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sqrt")]
pub extern "sysv64" fn sqrt(value: f64) -> f64 {
    musl_libm::sqrt(value)
}

/// Computes the square root of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sqrtf")]
pub extern "sysv64" fn sqrtf(value: f32) -> f32 {
    musl_libm::sqrtf(value)
}

/// Computes the cube root of `value`, preserving the sign of the argument.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cbrt")]
pub extern "sysv64" fn cbrt(value: f64) -> f64 {
    musl_libm::cbrt(value)
}

/// Computes the cube root of `value`, preserving the sign of the argument.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cbrtf")]
pub extern "sysv64" fn cbrtf(value: f32) -> f32 {
    musl_libm::cbrtf(value)
}

/// Computes `sqrt(x*x + y*y)` without undue overflow or underflow.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_hypot")]
pub extern "sysv64" fn hypot(x: f64, y: f64) -> f64 {
    musl_libm::hypot(x, y)
}

/// Computes `sqrt(x*x + y*y)` without undue overflow or underflow.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_hypotf")]
pub extern "sysv64" fn hypotf(x: f32, y: f32) -> f32 {
    musl_libm::hypotf(x, y)
}

// ---------------------------------------------------------------------------
// Rounding
// ---------------------------------------------------------------------------

/// Rounds `value` up to the nearest integer.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_ceil")]
pub extern "sysv64" fn ceil(value: f64) -> f64 {
    musl_libm::ceil(value)
}

/// Rounds `value` up to the nearest integer.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_ceilf")]
pub extern "sysv64" fn ceilf(value: f32) -> f32 {
    musl_libm::ceilf(value)
}

/// Rounds `value` down to the nearest integer.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_floor")]
pub extern "sysv64" fn floor(value: f64) -> f64 {
    musl_libm::floor(value)
}

/// Rounds `value` down to the nearest integer.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_floorf")]
pub extern "sysv64" fn floorf(value: f32) -> f32 {
    musl_libm::floorf(value)
}

/// Rounds `value` to the nearest integer, with halfway cases rounded away from
/// zero as C99 requires.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_round")]
pub extern "sysv64" fn round(value: f64) -> f64 {
    musl_libm::round(value)
}

/// Rounds `value` to the nearest integer, with halfway cases rounded away from
/// zero as C99 requires.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_roundf")]
pub extern "sysv64" fn roundf(value: f32) -> f32 {
    musl_libm::roundf(value)
}

/// Truncates `value` toward zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_trunc")]
pub extern "sysv64" fn trunc(value: f64) -> f64 {
    musl_libm::trunc(value)
}

/// Truncates `value` toward zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_truncf")]
pub extern "sysv64" fn truncf(value: f32) -> f32 {
    musl_libm::truncf(value)
}

/// Rounds with the current floating-point rounding mode, raising inexact.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_rint")]
pub extern "sysv64" fn rint(value: f64) -> f64 {
    let mut result = value;
    unsafe {
        core::arch::asm!("fld qword ptr [{p}]", "frndint", "fstp qword ptr [{p}]",
            p = in(reg) &raw mut result, options(nostack, preserves_flags));
    }
    result
}

/// Rounds with the current floating-point rounding mode, raising inexact.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_rintf")]
pub extern "sysv64" fn rintf(value: f32) -> f32 {
    rint(value as f64) as f32
}

/// Rounds with the current rounding mode while preserving the inexact flag.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nearbyint")]
pub extern "sysv64" fn nearbyint(value: f64) -> f64 {
    unsafe {
        let mut before = core::mem::MaybeUninit::<fenv_t>::uninit();
        fegetenv(before.as_mut_ptr());
        let before = before.assume_init();
        let control = before.control_word | 0x20;
        core::arch::asm!("fldcw [{}]", in(reg) &control, options(nostack, preserves_flags));
        let result = rint(value);
        let mut after = core::mem::MaybeUninit::<fenv_t>::uninit();
        fegetenv(after.as_mut_ptr());
        let mut after = after.assume_init();
        after.control_word = before.control_word;
        after.status_word = (after.status_word & !0x20) | (before.status_word & 0x20);
        core::arch::asm!("fldenv [{}]", in(reg) &after, options(nostack, preserves_flags));
        result
    }
}

/// The float form of nearbyint, preserving the inexact flag.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nearbyintf")]
pub extern "sysv64" fn nearbyintf(value: f32) -> f32 {
    nearbyint(value as f64) as f32
}

// ---------------------------------------------------------------------------
// Remainder
// ---------------------------------------------------------------------------

/// Computes the floating-point remainder of `x / y`, with the sign of `x`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fmod")]
pub extern "sysv64" fn fmod(x: f64, y: f64) -> f64 {
    musl_libm::fmod(x, y)
}

/// Computes the floating-point remainder of `x / y`, with the sign of `x`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fmodf")]
pub extern "sysv64" fn fmodf(x: f32, y: f32) -> f32 {
    musl_libm::fmodf(x, y)
}

/// Computes the IEEE 754 remainder of `x / y`, where the quotient is rounded to
/// the nearest integer with halfway cases rounded to even.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_remainder")]
pub extern "sysv64" fn remainder(x: f64, y: f64) -> f64 {
    musl_libm::remainder(x, y)
}

/// Computes the IEEE 754 remainder of `x / y`, where the quotient is rounded to
/// the nearest integer with halfway cases rounded to even.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_remainderf")]
pub extern "sysv64" fn remainderf(x: f32, y: f32) -> f32 {
    musl_libm::remainderf(x, y)
}

// ---------------------------------------------------------------------------
// Classification
//
// C exposes these as type-generic macros, but the guest links against real
// symbols, so each is exported as a function returning a C `int`.
// ---------------------------------------------------------------------------

/// Returns a non-zero value when `value` is NaN.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_isnan")]
pub extern "sysv64" fn isnan(value: f64) -> i32 {
    i32::from(value.is_nan())
}

/// Returns a non-zero value when `value` is NaN.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_isnanf")]
pub extern "sysv64" fn isnanf(value: f32) -> i32 {
    i32::from(value.is_nan())
}

/// Returns a non-zero value when `value` is positive or negative infinity.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_isinf")]
pub extern "sysv64" fn isinf(value: f64) -> i32 {
    if value.is_infinite() {
        if value.is_sign_negative() { -1 } else { 1 }
    } else {
        0
    }
}

/// Returns a non-zero value when `value` is positive or negative infinity.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_isinff")]
pub extern "sysv64" fn isinff(value: f32) -> i32 {
    if value.is_infinite() {
        if value.is_sign_negative() { -1 } else { 1 }
    } else {
        0
    }
}

/// Returns a non-zero value when `value` is neither infinite nor NaN.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_isfinite")]
pub extern "sysv64" fn isfinite(value: f64) -> i32 {
    i32::from(value.is_finite())
}

/// Returns a non-zero value when `value` is neither infinite nor NaN.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_isfinitef")]
pub extern "sysv64" fn isfinitef(value: f32) -> i32 {
    i32::from(value.is_finite())
}

/// Returns a non-zero value when the sign bit of `value` is set.
///
/// Reads the sign bit directly so that `-0.0` and negative NaN report as
/// negative, which a `value < 0.0` comparison would miss.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_signbit")]
pub extern "sysv64" fn signbit(value: f64) -> i32 {
    i32::from(value.to_bits() >> 63 != 0)
}

/// Returns a non-zero value when the sign bit of `value` is set.
///
/// Reads the sign bit directly so that `-0.0` and negative NaN report as
/// negative, which a `value < 0.0` comparison would miss.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_signbitf")]
pub extern "sysv64" fn signbitf(value: f32) -> i32 {
    i32::from(value.to_bits() >> 31 != 0)
}

/// Classifies `value` as one of `FP_NAN`, `FP_INFINITE`, `FP_ZERO`,
/// `FP_SUBNORMAL`, or `FP_NORMAL`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fpclassify")]
pub extern "sysv64" fn fpclassify(value: f64) -> i32 {
    match value.classify() {
        core::num::FpCategory::Nan => FP_NAN,
        core::num::FpCategory::Infinite => FP_INFINITE,
        core::num::FpCategory::Zero => FP_ZERO,
        core::num::FpCategory::Subnormal => FP_SUBNORMAL,
        core::num::FpCategory::Normal => FP_NORMAL,
    }
}

/// Classifies `value` as one of `FP_NAN`, `FP_INFINITE`, `FP_ZERO`,
/// `FP_SUBNORMAL`, or `FP_NORMAL`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fpclassifyf")]
pub extern "sysv64" fn fpclassifyf(value: f32) -> i32 {
    match value.classify() {
        core::num::FpCategory::Nan => FP_NAN,
        core::num::FpCategory::Infinite => FP_INFINITE,
        core::num::FpCategory::Zero => FP_ZERO,
        core::num::FpCategory::Subnormal => FP_SUBNORMAL,
        core::num::FpCategory::Normal => FP_NORMAL,
    }
}

// ---------------------------------------------------------------------------
// Miscellaneous
// ---------------------------------------------------------------------------

/// Returns the positive difference `x - y`, or `+0.0` when `x <= y`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fdim")]
pub extern "sysv64" fn fdim(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        f64::NAN
    } else if x > y {
        x - y
    } else {
        0.0
    }
}

/// Returns the positive difference `x - y`, or `+0.0` when `x <= y`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fdimf")]
pub extern "sysv64" fn fdimf(x: f32, y: f32) -> f32 {
    if x.is_nan() || y.is_nan() {
        f32::NAN
    } else if x > y {
        x - y
    } else {
        0.0
    }
}

/// Computes `x * y + z` as a single operation, rounding only once.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fma")]
pub extern "sysv64" fn fma(x: f64, y: f64, z: f64) -> f64 {
    musl_libm::fma(x, y, z)
}

/// Computes `x * y + z` as a single operation, rounding only once.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fmaf")]
pub extern "sysv64" fn fmaf(x: f32, y: f32, z: f32) -> f32 {
    musl_libm::fmaf(x, y, z)
}

/// Computes `value * 2**exponent`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_ldexp")]
pub extern "sysv64" fn ldexp(value: f64, exponent: c_int) -> f64 {
    musl_libm::ldexp(value, exponent)
}

/// Computes `value * 2**exponent`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_ldexpf")]
pub extern "sysv64" fn ldexpf(value: f32, exponent: c_int) -> f32 {
    musl_libm::ldexpf(value, exponent)
}

/// Returns a quiet NaN.
///
/// The `tag` payload is implementation-defined per C99 7.12.11.2; this
/// implementation ignores it and never dereferences the pointer, so the
/// canonical quiet NaN is always returned.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nan")]
pub extern "sysv64" fn nan(_tag: *const c_char) -> f64 {
    f64::NAN
}

/// Returns a quiet NaN.
///
/// The `tag` payload is implementation-defined per C99 7.12.11.2; this
/// implementation ignores it and never dereferences the pointer, so the
/// canonical quiet NaN is always returned.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nanf")]
pub extern "sysv64" fn nanf(_tag: *const c_char) -> f32 {
    f32::NAN
}

/// Computes the error function of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_erf")]
pub extern "sysv64" fn erf(value: f64) -> f64 {
    musl_libm::erf(value)
}

/// Computes the error function of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_erff")]
pub extern "sysv64" fn erff(value: f32) -> f32 {
    musl_libm::erff(value)
}

/// Computes the complementary error function `1 - erf(value)`, staying accurate
/// for large `value` where the subtraction would lose all precision.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_erfc")]
pub extern "sysv64" fn erfc(value: f64) -> f64 {
    musl_libm::erfc(value)
}

/// Computes the complementary error function `1 - erff(value)`, staying accurate
/// for large `value` where the subtraction would lose all precision.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_erfcf")]
pub extern "sysv64" fn erfcf(value: f32) -> f32 {
    musl_libm::erfcf(value)
}

/// Computes the natural logarithm of the absolute value of the gamma function.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_lgamma")]
pub extern "sysv64" fn lgamma(value: f64) -> f64 {
    let (res, sign) = musl_libm::lgamma_r(value);
    unsafe { kinakaze_engine_libm_signgam = sign };
    res
}

/// Computes the natural logarithm of the absolute value of the gamma function.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_lgammaf")]
pub extern "sysv64" fn lgammaf(value: f32) -> f32 {
    let (res, sign) = musl_libm::lgammaf_r(value);
    unsafe { kinakaze_engine_libm_signgam = sign };
    res
}

/// Computes the gamma function of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_tgamma")]
pub extern "sysv64" fn tgamma(value: f64) -> f64 {
    musl_libm::tgamma(value)
}

/// Computes the gamma function of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_tgammaf")]
pub extern "sysv64" fn tgammaf(value: f32) -> f32 {
    musl_libm::tgammaf(value)
}

/// `2**64`, used to lift a subnormal `f64` into the normal range.
#[cfg(target_arch = "x86_64")]
const F64_SUBNORMAL_SCALE: f64 = 18_446_744_073_709_551_616.0;

/// `2**32`, used to lift a subnormal `f32` into the normal range.
#[cfg(target_arch = "x86_64")]
const F32_SUBNORMAL_SCALE: f32 = 4_294_967_296.0;

/// Splits `value` into a normalized fraction in `[0.5, 1.0)` and a power of two,
/// storing the exponent through `exponent` and returning the fraction.
///
/// Rust std has no `frexp`, so this reads the IEEE 754 fields directly: the
/// biased exponent is replaced with `1022` to force the result into
/// `[0.5, 1.0)`, which keeps the mantissa bit-exact. Subnormals are first scaled
/// up by `2**64` and the exponent compensated, because their biased exponent
/// field is zero and carries no usable value. For zero the fraction is the input
/// (sign preserved) and the exponent is `0`; for infinities and NaN the input is
/// returned and the exponent is set to `0`.
///
/// # Safety
///
/// `exponent` must be non-null, aligned, and valid for a write of one `c_int`.
/// It is always written before this function returns.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_frexp")]
pub unsafe extern "sysv64" fn frexp(value: f64, exponent: *mut c_int) -> f64 {
    let mut bits = value.to_bits();
    let mut biased = ((bits >> 52) & 0x7ff) as c_int;
    let mut adjust: c_int = 0;

    if biased == 0x7ff {
        unsafe { *exponent = 0 };
        return value;
    }

    if biased == 0 {
        if value == 0.0 {
            unsafe { *exponent = 0 };
            return value;
        }
        bits = (value * F64_SUBNORMAL_SCALE).to_bits();
        biased = ((bits >> 52) & 0x7ff) as c_int;
        adjust = -64;
    }

    unsafe { *exponent = biased - 1022 + adjust };
    f64::from_bits((bits & !(0x7ffu64 << 52)) | (1022u64 << 52))
}

/// Splits `value` into a normalized fraction in `[0.5, 1.0)` and a power of two,
/// storing the exponent through `exponent` and returning the fraction.
///
/// See [`frexp`] for why this is done with bit manipulation rather than
/// arithmetic; the only differences here are the field widths and the `2**32`
/// subnormal scale.
///
/// # Safety
///
/// `exponent` must be non-null, aligned, and valid for a write of one `c_int`.
/// It is always written before this function returns.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_frexpf")]
pub unsafe extern "sysv64" fn frexpf(value: f32, exponent: *mut c_int) -> f32 {
    let mut bits = value.to_bits();
    let mut biased = ((bits >> 23) & 0xff) as c_int;
    let mut adjust: c_int = 0;

    if biased == 0xff {
        unsafe { *exponent = 0 };
        return value;
    }

    if biased == 0 {
        if value == 0.0 {
            unsafe { *exponent = 0 };
            return value;
        }
        bits = (value * F32_SUBNORMAL_SCALE).to_bits();
        biased = ((bits >> 23) & 0xff) as c_int;
        adjust = -32;
    }

    unsafe { *exponent = biased - 126 + adjust };
    f32::from_bits((bits & !(0xffu32 << 23)) | (126u32 << 23))
}

/// Splits `value` into integral and fractional parts, storing the integral part
/// through `integral` and returning the fractional part.
///
/// Both parts carry the sign of `value`, so the fractional part is passed
/// through `copysign` to produce `-0.0` rather than `+0.0` for negative
/// integral input. Infinities yield a correctly signed zero fraction, since
/// subtracting the integral part would produce NaN.
///
/// # Safety
///
/// `integral` must be non-null, aligned, and valid for a write of one `f64`. It
/// is always written before this function returns.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_modf")]
pub unsafe extern "sysv64" fn modf(value: f64, integral: *mut f64) -> f64 {
    if value.is_nan() || value.is_infinite() {
        unsafe { *integral = value };
        return if value.is_nan() {
            value
        } else {
            copysign(0.0, value)
        };
    }

    let integral_part = musl_libm::trunc(value);
    unsafe { *integral = integral_part };
    copysign(value - integral_part, value)
}

/// Splits `value` into integral and fractional parts, storing the integral part
/// through `integral` and returning the fractional part.
///
/// See [`modf`] for the sign handling.
///
/// # Safety
///
/// `integral` must be non-null, aligned, and valid for a write of one `f32`. It
/// is always written before this function returns.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_modff")]
pub unsafe extern "sysv64" fn modff(value: f32, integral: *mut f32) -> f32 {
    if value.is_nan() || value.is_infinite() {
        unsafe { *integral = value };
        return if value.is_nan() {
            value
        } else {
            copysignf(0.0, value)
        };
    }

    let integral_part = musl_libm::truncf(value);
    unsafe { *integral = integral_part };
    copysignf(value - integral_part, value)
}

// ---------------------------------------------------------------------------
// Sign and comparison
// ---------------------------------------------------------------------------

/// Returns the absolute value of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fabs")]
pub extern "sysv64" fn fabs(value: f64) -> f64 {
    f64::from_bits(value.to_bits() & !(1u64 << 63))
}

/// Returns the absolute value of `value`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fabsf")]
pub extern "sysv64" fn fabsf(value: f32) -> f32 {
    f32::from_bits(value.to_bits() & !(1u32 << 31))
}

/// Returns `magnitude` with the sign of `sign`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_copysign")]
pub extern "sysv64" fn copysign(magnitude: f64, sign: f64) -> f64 {
    f64::from_bits((magnitude.to_bits() & !(1u64 << 63)) | (sign.to_bits() & (1u64 << 63)))
}

/// Returns `magnitude` with the sign of `sign`.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_copysignf")]
pub extern "sysv64" fn copysignf(magnitude: f32, sign: f32) -> f32 {
    f32::from_bits((magnitude.to_bits() & !(1u32 << 31)) | (sign.to_bits() & (1u32 << 31)))
}

/// Returns the lesser of the two arguments, ignoring a single NaN operand.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fmin")]
pub extern "sysv64" fn fmin(left: f64, right: f64) -> f64 {
    if left.is_nan() {
        right
    } else if right.is_nan() || left < right {
        left
    } else {
        right
    }
}

/// Returns the lesser of the two arguments, ignoring a single NaN operand.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fminf")]
pub extern "sysv64" fn fminf(left: f32, right: f32) -> f32 {
    if left.is_nan() {
        right
    } else if right.is_nan() || left < right {
        left
    } else {
        right
    }
}

/// Returns the greater of the two arguments, ignoring a single NaN operand.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fmax")]
pub extern "sysv64" fn fmax(left: f64, right: f64) -> f64 {
    if left.is_nan() {
        right
    } else if right.is_nan() || left > right {
        left
    } else {
        right
    }
}

/// Returns the greater of the two arguments, ignoring a single NaN operand.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fmaxf")]
pub extern "sysv64" fn fmaxf(left: f32, right: f32) -> f32 {
    if left.is_nan() {
        right
    } else if right.is_nan() || left > right {
        left
    } else {
        right
    }
}

// ---------------------------------------------------------------------------
// Rounding/conversion functions needed by Node.js and Python3
// ---------------------------------------------------------------------------

/// `lrint` — round to nearest integer using current rounding mode.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_lrint")]
pub extern "sysv64" fn lrint(x: f64) -> i64 {
    let result: i64;
    unsafe {
        core::arch::asm!("cvtsd2si {result}, {x}", result = out(reg) result,
        x = in(xmm_reg) x, options(nostack, preserves_flags));
    }
    result
}

/// `lrintf` — float version.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_lrintf")]
pub extern "sysv64" fn lrintf(x: f32) -> i64 {
    lrint(x as f64)
}

/// `llround` — round to nearest, ties away from zero.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_llround")]
pub extern "sysv64" fn llround(x: f64) -> i64 {
    let result: i64;
    let rounded = musl_libm::round(x);
    unsafe {
        core::arch::asm!("cvttsd2si {result}, {x}", result = out(reg) result,
        x = in(xmm_reg) rounded, options(nostack, preserves_flags));
    }
    result
}

/// `llroundf` — float version.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_llroundf")]
pub extern "sysv64" fn llroundf(x: f32) -> i64 {
    llround(x as f64)
}

/// `lround` — round to nearest long.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_lround")]
pub extern "sysv64" fn lround(x: f64) -> i64 {
    llround(x)
}

/// `lroundf` — float version.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_lroundf")]
pub extern "sysv64" fn lroundf(x: f32) -> i64 {
    llround(x as f64)
}

/// `scalbn` — multiply by 2^n.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_scalbn")]
pub extern "sysv64" fn scalbn(x: f64, n: c_int) -> f64 {
    musl_libm::scalbn(x, n)
}

/// `scalbnf` — float version.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_scalbnf")]
pub extern "sysv64" fn scalbnf(x: f32, n: c_int) -> f32 {
    musl_libm::scalbnf(x, n)
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_scalbln")]
pub extern "sysv64" fn scalbln(x: f64, n: i64) -> f64 {
    let exponent = n.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    musl_libm::scalbn(x, exponent)
}

/// `sincos` — compute sin and cos simultaneously.
///
/// # Safety
/// `sin_out` and `cos_out` must be writable.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sincos")]
pub unsafe extern "sysv64" fn sincos(x: f64, sin_out: *mut f64, cos_out: *mut f64) {
    let (sin_value, cos_value) = musl_libm::sincos(x);
    if !sin_out.is_null() {
        unsafe {
            *sin_out = sin_value;
        }
    }
    if !cos_out.is_null() {
        unsafe {
            *cos_out = cos_value;
        }
    }
}

/// `sincosf` — float version.
///
/// # Safety
/// `sin_out` and `cos_out` must be writable.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_sincosf")]
pub unsafe extern "sysv64" fn sincosf(x: f32, sin_out: *mut f32, cos_out: *mut f32) {
    let (sin_value, cos_value) = musl_libm::sincosf(x);
    if !sin_out.is_null() {
        unsafe {
            *sin_out = sin_value;
        }
    }
    if !cos_out.is_null() {
        unsafe {
            *cos_out = cos_value;
        }
    }
}

/// `nextafter` — next representable value after x towards y.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nextafter")]
pub extern "sysv64" fn nextafter(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        return f64::NAN;
    }
    if x == y {
        return y;
    }
    if x == 0.0 {
        return if y > 0.0 {
            f64::from_bits(1)
        } else {
            f64::from_bits(0x8000_0000_0000_0001)
        };
    }
    let bits = x.to_bits();
    let next = if (x < y) == (x > 0.0) {
        bits + 1
    } else {
        bits - 1
    };
    f64::from_bits(next)
}

/// `nextafterf` — float version.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nextafterf")]
pub extern "sysv64" fn nextafterf(x: f32, y: f32) -> f32 {
    if x.is_nan() || y.is_nan() {
        return f32::NAN;
    }
    if x == y {
        return y;
    }
    if x == 0.0 {
        return if y > 0.0 {
            f32::from_bits(1)
        } else {
            f32::from_bits(0x8000_0001)
        };
    }
    let bits = x.to_bits();
    let next = if (x < y) == (x > 0.0) {
        bits + 1
    } else {
        bits - 1
    };
    f32::from_bits(next)
}

/// System V AMD64 passes the second `nexttoward` argument as an 80-bit
/// long double on the stack. Its precision matters even when it rounds to `x`.
/// This naked boundary compares in x87 before advancing the f64 bit pattern.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_nexttoward")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn nexttoward() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "movsd qword ptr [rsp], xmm0",
        "fld tbyte ptr [rsp + 32]",
        "fld qword ptr [rsp]",
        "fucomip st(0), st(1)",
        "fstp st(0)",
        "jp 3f",
        "je 2f",
        "mov edi, 1",
        "jb 4f",
        "mov edi, -1",
        "4:",
        "add rsp, 24",
        "jmp {step}",
        "2:",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "movsd xmm0, qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        "3:",
        "fld tbyte ptr [rsp + 32]",
        "fstp qword ptr [rsp]",
        "addsd xmm0, qword ptr [rsp]",
        "add rsp, 24",
        "ret",
        step = sym nexttoward_step,
    );
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn nexttoward_step(x: f64, direction: c_int) -> f64 {
    nextafter(
        x,
        if direction > 0 {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        },
    )
}

/// System V long-double `frexpl`: stack argument, x87 ST(0) result, exponent
/// pointer in RDI. Normalize the explicit 64-bit significand without reducing
/// precision through f64, including subnormal 80-bit inputs.
#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_frexpl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn frexpl() {
    core::arch::naked_asm!(
        "sub rsp, 24",
        "mov rax, qword ptr [rsp + 32]",
        "movzx edx, word ptr [rsp + 40]",
        "mov ecx, edx",
        "and ecx, 0x7fff",
        "cmp ecx, 0x7fff",
        "je 4f",
        "test ecx, ecx",
        "jnz 2f",
        "test rax, rax",
        "jz 4f",
        "bsr rcx, rax",
        "neg ecx",
        "add ecx, 63",
        "shl rax, cl",
        "neg ecx",
        "sub ecx, 16381",
        "jmp 3f",
        "2:",
        "sub ecx, 16382",
        "3:",
        "and edx, 0x8000",
        "or edx, 0x3ffe",
        "jmp 5f",
        "4:",
        "xor ecx, ecx",
        "5:",
        "mov dword ptr [rdi], ecx",
        "mov qword ptr [rsp], rax",
        "mov word ptr [rsp + 8], dx",
        "fld tbyte ptr [rsp]",
        "add rsp, 24",
        "ret",
    );
}

// The 80-bit argument is passed in 16 bytes on the guest stack; its integer
// exponent is in RDI, and the result must remain in x87 ST(0). No f64 temporary
// participates in this path. Host Rust/CRT math names remain untouched.
macro_rules! extended_scale {
    ($name:ident, $exponent:literal) => {
        #[cfg(target_arch = "x86_64")]
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        #[unsafe(naked)]
        pub unsafe extern "sysv64" fn $name() {
            core::arch::naked_asm!(
                "sub rsp, 40",
                "movzx eax, word ptr [rsp + 56]",
                "and eax, 0x7fff",
                "cmp eax, 0x7fff",
                "je 5f",
                "test eax, eax",
                "jnz 2f",
                "cmp qword ptr [rsp + 48], 0",
                "je 5f",
                "2:",
                "mov qword ptr [rsp + 24], rdi",
                concat!("fild ", $exponent, " ptr [rsp + 24]"),
                "fld tbyte ptr [rsp + 48]",
                "fscale",
                "fstp st(1)",
                "fstp tbyte ptr [rsp]",
                "movzx eax, word ptr [rsp + 8]",
                "and eax, 0x7fff",
                "cmp eax, 0x7fff",
                "je 3f",
                "test eax, eax",
                "jnz 4f",
                "cmp qword ptr [rsp], 0",
                "jne 4f",
                "3:",
                "call {range_error}",
                "4:",
                "fld tbyte ptr [rsp]",
                "add rsp, 40",
                "ret",
                "5:",
                "fld tbyte ptr [rsp + 48]",
                "fadd st(0), st(0)",
                "add rsp, 40",
                "ret",
                range_error = sym extended_range_error,
            );
        }
    };
}

extended_scale!(scalbnl, "dword");
extended_scale!(scalblnl, "qword");

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_ldexpl")]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn ldexpl() {
    core::arch::naked_asm!("jmp {scale}", scale = sym scalbnl);
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn extended_range_error() {
    // A single native C ABI dependency shares libc's actual thread-local errno.
    // The standalone PE imports it; static libc links its own definition.
    unsafe { errno_location().write(34) }; // ERANGE
}

fn errno_location() -> *mut i32 {
    kinakaze_tls::errno_location()
}

/// System V classifies both f32 components together in one SSE register.
#[repr(C)]
pub struct Complex32 {
    pub real: f32,
    pub imaginary: f32,
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cabsf")]
pub extern "sysv64" fn cabsf(value: Complex32) -> f32 {
    musl_libm::hypotf(value.real, value.imaginary)
}

/// Two f64 components occupy the first two System V SSE argument registers.
#[repr(C)]
pub struct Complex64 {
    pub real: f64,
    pub imaginary: f64,
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_cabs")]
pub extern "sysv64" fn cabs(value: Complex64) -> f64 {
    musl_libm::hypot(value.real, value.imaginary)
}

// ---------------------------------------------------------------------------
// Floating-point environment (<fenv.h>)
// ---------------------------------------------------------------------------

#[inline(always)]
unsafe fn get_mxcsr() -> u32 {
    let mut val: u32 = 0;
    unsafe {
        core::arch::asm!("stmxcsr [{}]", in(reg) &raw mut val, options(nostack, preserves_flags));
    }
    val
}

#[inline(always)]
unsafe fn set_mxcsr(val: u32) {
    unsafe {
        core::arch::asm!("ldmxcsr [{}]", in(reg) &val, options(nostack, preserves_flags));
    }
}

#[repr(C)]
pub struct fenv_t {
    pub control_word: u16,
    pub unused1: u16,
    pub status_word: u16,
    pub unused2: u16,
    pub tags: u16,
    pub unused3: u16,
    pub eip: u32,
    pub cs_selector: u16,
    pub opcode: u16,
    pub data_offset: u32,
    pub data_selector: u16,
    pub unused5: u16,
    pub mxcsr: u32,
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fegetenv")]
pub unsafe extern "sysv64" fn fegetenv(envp: *mut fenv_t) -> c_int {
    if envp.is_null() {
        return -1;
    }
    unsafe {
        // FNSTENV stores the 28-byte x87 environment and masks exceptions.
        // Reload it immediately so reading the environment has no side effects.
        core::arch::asm!("fnstenv [{env}]", "fldenv [{env}]", env = in(reg) envp,
            options(nostack, preserves_flags));
        (*envp).mxcsr = get_mxcsr();
    }
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fesetenv")]
pub unsafe extern "sysv64" fn fesetenv(envp: *const fenv_t) -> c_int {
    if envp.is_null() {
        return -1;
    }
    if (envp as usize) == usize::MAX {
        // FE_DFL_ENV
        unsafe {
            core::arch::asm!("fninit", options(nostack, preserves_flags));
            set_mxcsr(0x1f80);
        }
        return 0;
    }
    if (envp as usize) == usize::MAX - 1 {
        // GNU FE_NOMASK_ENV enables the five public IEEE exceptions.
        let control = 0x0342u16;
        unsafe {
            core::arch::asm!("fninit", "fldcw [{}]", in(reg) &control,
                options(nostack, preserves_flags));
            set_mxcsr(0x0100);
        }
        return 0;
    }
    unsafe {
        core::arch::asm!("fldenv [{}]", in(reg) envp, options(nostack, preserves_flags));
        set_mxcsr((*envp).mxcsr);
    }
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_feholdexcept")]
pub unsafe extern "sysv64" fn feholdexcept(envp: *mut fenv_t) -> c_int {
    if envp.is_null() {
        return -1;
    }
    unsafe {
        let _ = fegetenv(envp);
        let control = (*envp).control_word | 0x3f;
        core::arch::asm!("fnclex", "fldcw [{}]", in(reg) &control,
            options(nostack, preserves_flags));
        set_mxcsr((get_mxcsr() | 0x1f80) & !0x3f);
    }
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_feupdateenv")]
pub unsafe extern "sysv64" fn feupdateenv(envp: *const fenv_t) -> c_int {
    let current_excepts = fetestexcept(0x3f);
    unsafe {
        if fesetenv(envp) != 0 {
            return -1;
        }
    }
    let _ = feraiseexcept(current_excepts);
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fegetround")]
pub extern "sysv64" fn fegetround() -> c_int {
    let mut control = 0u16;
    unsafe {
        core::arch::asm!("fnstcw [{}]", in(reg) &raw mut control,
        options(nostack, preserves_flags));
    }
    (control & 0x0c00) as c_int
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fesetround")]
pub extern "sysv64" fn fesetround(round: c_int) -> c_int {
    // glibc's FE_DOWNWARD/UPWARD/TOWARDZERO are 0x400/0x800/0xc00.
    if round & !0x0c00 != 0 {
        return -1;
    }
    unsafe {
        let mxcsr = get_mxcsr();
        let mut control = 0u16;
        core::arch::asm!("fnstcw [{}]", in(reg) &raw mut control,
            options(nostack, preserves_flags));
        control = (control & !0x0c00) | round as u16;
        core::arch::asm!("fldcw [{}]", in(reg) &control,
            options(nostack, preserves_flags));
        let new_mxcsr = (mxcsr & !0x6000) | ((round as u32) << 3);
        set_mxcsr(new_mxcsr);
    }
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_feclearexcept")]
pub extern "sysv64" fn feclearexcept(excepts: c_int) -> c_int {
    unsafe {
        let mut env = core::mem::MaybeUninit::<fenv_t>::uninit();
        fegetenv(env.as_mut_ptr());
        let mut env = env.assume_init();
        env.status_word &= !(excepts as u16 & 0x3d);
        core::arch::asm!("fldenv [{}]", in(reg) &env, options(nostack, preserves_flags));
        let mxcsr = get_mxcsr();
        let new_mxcsr = mxcsr & !(excepts as u32 & 0x3d);
        set_mxcsr(new_mxcsr);
    }
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_feraiseexcept")]
pub extern "sysv64" fn feraiseexcept(excepts: c_int) -> c_int {
    unsafe {
        let mut env = core::mem::MaybeUninit::<fenv_t>::uninit();
        fegetenv(env.as_mut_ptr());
        let mut env = env.assume_init();
        env.status_word |= excepts as u16 & 0x3d;
        core::arch::asm!("fldenv [{}]", in(reg) &env, options(nostack, preserves_flags));
        let mxcsr = get_mxcsr();
        let new_mxcsr = mxcsr | (excepts as u32 & 0x3d);
        set_mxcsr(new_mxcsr);
        core::arch::asm!("fwait", options(nostack, preserves_flags));
    }
    0
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm_fetestexcept")]
pub extern "sysv64" fn fetestexcept(excepts: c_int) -> c_int {
    let mxcsr = unsafe { get_mxcsr() };
    let status: u16;
    unsafe {
        core::arch::asm!("fnstsw ax", out("ax") status,
        options(nostack, preserves_flags));
    }
    ((mxcsr | status as u32) & (excepts as u32 & 0x3d)) as c_int
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm___isnan")]
pub extern "sysv64" fn __isnan(x: f64) -> i32 {
    i32::from(x.is_nan())
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm___isnanf")]
pub extern "sysv64" fn __isnanf(x: f32) -> i32 {
    i32::from(x.is_nan())
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm___isinf")]
pub extern "sysv64" fn __isinf(x: f64) -> i32 {
    isinf(x)
}

#[cfg(target_arch = "x86_64")]
#[unsafe(export_name = "kinakaze_engine_libm___isinff")]
pub extern "sysv64" fn __isinff(x: f32) -> i32 {
    isinff(x)
}

#[cfg(all(test, target_arch = "x86_64"))]
mod engine_tests {
    use super::*;

    fn extended(mantissa: u64, exponent: u16) -> [u8; 16] {
        let mut value = [0; 16];
        value[..8].copy_from_slice(&mantissa.to_le_bytes());
        value[8..10].copy_from_slice(&exponent.to_le_bytes());
        value
    }

    fn call_frexpl(input: [u8; 16]) -> ([u8; 16], i32) {
        let mut output = [0; 16];
        let mut exponent = 0;
        unsafe {
            core::arch::asm!(
                "sub rsp, 32",
                "mov rax, qword ptr [rsi]",
                "mov qword ptr [rsp], rax",
                "mov rax, qword ptr [rsi + 8]",
                "mov qword ptr [rsp + 8], rax",
                "call {entry}",
                "fstp tbyte ptr [r12]",
                "add rsp, 32",
                entry = sym frexpl,
                in("rdi") &raw mut exponent, in("rsi") input.as_ptr(),
                in("r12") output.as_mut_ptr(), clobber_abi("sysv64"),
            );
        }
        (output, exponent)
    }

    fn call_nexttoward(x: f64, y: [u8; 16]) -> f64 {
        let output: f64;
        unsafe {
            core::arch::asm!(
                "sub rsp, 32",
                "mov rax, qword ptr [rdi]",
                "mov qword ptr [rsp], rax",
                "mov rax, qword ptr [rdi + 8]",
                "mov qword ptr [rsp + 8], rax",
                "call {entry}",
                "add rsp, 32",
                entry = sym nexttoward,
                in("rdi") y.as_ptr(), inlateout("xmm0") x => output,
                clobber_abi("sysv64"),
            );
        }
        output
    }

    fn call_extended_scale(input: [u8; 16], exponent: i64, wide: bool) -> [u8; 16] {
        let mut output = [0; 16];
        let entry = if wide { scalblnl } else { ldexpl };
        unsafe {
            core::arch::asm!(
                "sub rsp, 32",
                "mov rax, qword ptr [rsi]",
                "mov qword ptr [rsp], rax",
                "mov rax, qword ptr [rsi + 8]",
                "mov qword ptr [rsp + 8], rax",
                "call r13",
                "fstp tbyte ptr [r12]",
                "add rsp, 32",
                in("rdi") exponent, in("rsi") input.as_ptr(),
                in("r12") output.as_mut_ptr(), in("r13") entry,
                clobber_abi("sysv64"),
            );
        }
        output
    }

    #[test]
    fn extended_scaling_preserves_80_bits_subnormals_and_range_errors() {
        let saved_round = fegetround();
        assert_eq!(fesetround(0), 0);
        struct Restore(i32);
        impl Drop for Restore {
            fn drop(&mut self) {
                fesetround(self.0);
            }
        }
        let _restore = Restore(saved_round);
        kinakaze_tls::set_errno(7);
        assert_eq!(
            call_extended_scale(extended(0xc000_0000_0000_0001, 0x3fff), 3, false),
            extended(0xc000_0000_0000_0001, 0x4002)
        );
        assert_eq!(
            call_extended_scale(extended(1, 0), 63, false),
            extended(0x8000_0000_0000_0000, 1)
        );
        assert_eq!(
            call_extended_scale(extended(0x8000_0000_0000_0000, 0x3fff), -16445, false),
            extended(1, 0)
        );
        assert_eq!(kinakaze_tls::errno(), 7);
        assert_eq!(
            call_extended_scale(extended(0, 0x8000), i64::MAX, true),
            extended(0, 0x8000)
        );
        assert_eq!(kinakaze_tls::errno(), 7);
        assert_eq!(
            call_extended_scale(extended(u64::MAX, 0x7ffe), 1, false),
            extended(0x8000_0000_0000_0000, 0x7fff)
        );
        assert_eq!(kinakaze_tls::errno(), 34);
        kinakaze_tls::set_errno(7);
        assert_eq!(
            call_extended_scale(extended(0x8000_0000_0000_0000, 0x3fff), -16446, false),
            extended(0, 0)
        );
        assert_eq!(kinakaze_tls::errno(), 34);
        assert_eq!(
            call_extended_scale(extended(0x8000_0000_0000_0000, 0x3fff), i64::MAX, true),
            extended(0x8000_0000_0000_0000, 0x7fff)
        );
        assert_eq!(
            call_extended_scale(extended(0x8000_0000_0000_0000, 0xbfff), i64::MIN, true),
            extended(0, 0x8000)
        );
    }

    #[test]
    fn complex_float_abi_packs_both_components_in_xmm0() {
        let packed =
            f64::from_bits(u64::from(3.0f32.to_bits()) | (u64::from(4.0f32.to_bits()) << 32));
        let result: f64;
        unsafe {
            core::arch::asm!(
                "sub rsp, 32", "call {entry}", "add rsp, 32",
                entry = sym cabsf, inlateout("xmm0") packed => result, clobber_abi("sysv64"),
            );
        }
        assert_eq!(f32::from_bits(result.to_bits() as u32), 5.0);
        assert!(
            cabsf(Complex32 {
                real: 1e20,
                imaginary: 1e20
            })
            .is_finite()
        );
        assert_eq!(
            cabsf(Complex32 {
                real: f32::INFINITY,
                imaginary: f32::NAN
            }),
            f32::INFINITY
        );
        assert_eq!(
            cabs(Complex64 {
                real: 3.0,
                imaginary: 4.0
            }),
            5.0
        );
    }

    #[test]
    fn long_double_boundary_preserves_extended_precision_and_x87_return() {
        assert_eq!(
            call_frexpl(extended(0xc000_0000_0000_0000, 0x4000)),
            (extended(0xc000_0000_0000_0000, 0x3ffe), 2)
        );
        assert_eq!(
            call_frexpl(extended(1, 0)),
            (extended(0x8000_0000_0000_0000, 0x3ffe), -16444)
        );
        assert_eq!(call_frexpl(extended(0, 0x8000)), (extended(0, 0x8000), 0));
        // 1 + 2^-63 rounds back to 1 as f64; the long-double comparison must
        // still advance by one f64 ulp instead of treating it as equal.
        assert_eq!(
            call_nexttoward(1.0, extended(0x8000_0000_0000_0001, 0x3fff)).to_bits(),
            1.0f64.to_bits() + 1
        );
        assert_eq!(
            call_nexttoward(0.0, extended(0, 0x8000)).to_bits(),
            (-0.0f64).to_bits()
        );
    }

    #[test]
    fn guest_math_symbols_do_not_recurse_into_host_crt() {
        assert!((sin(0.5) - 0.479425538604203).abs() < 1e-15);
        assert_eq!(sqrt(81.0), 9.0);
        assert_eq!(pow(2.0, 10.0), 1024.0);
        assert_eq!(fabs(-0.0).to_bits(), 0);
        assert_eq!(copysign(0.0, -1.0).to_bits(), (-0.0f64).to_bits());
    }

    #[test]
    fn floating_environment_uses_linux_rounding_constants_and_restores_state() {
        assert_eq!(core::mem::size_of::<fenv_t>(), 32);
        let mut original = core::mem::MaybeUninit::<fenv_t>::uninit();
        unsafe {
            assert_eq!(fegetenv(original.as_mut_ptr()), 0);
        }
        let original = unsafe { original.assume_init() };
        struct Restore(fenv_t);
        impl Drop for Restore {
            fn drop(&mut self) {
                unsafe {
                    fesetenv(&self.0);
                }
            }
        }
        let _restore = Restore(original);
        for round in [0, 0x400, 0x800, 0xc00] {
            assert_eq!(fesetround(round), 0);
            assert_eq!(fegetround(), round);
            assert_eq!((unsafe { get_mxcsr() } >> 3) & 0xc00, round as u32);
        }
        assert_eq!(fesetround(1), -1);
        assert_eq!(fesetround(0x400), 0);
        assert_eq!(rint(1.75), 1.0);
        assert_eq!(lrint(-1.25), -2);
        assert_eq!(fesetround(0x800), 0);
        assert_eq!(rint(1.25), 2.0);
        assert_eq!(lrint(-1.75), -1);
        let mut held = core::mem::MaybeUninit::<fenv_t>::uninit();
        unsafe {
            assert_eq!(feholdexcept(held.as_mut_ptr()), 0);
        }
        assert_eq!(fetestexcept(0x3d), 0);
        assert_eq!(nearbyint(1.5), 2.0);
        assert_eq!(fetestexcept(0x20), 0);
        assert_eq!(rint(1.5), 2.0);
        assert_eq!(fetestexcept(0x20), 0x20);
        feclearexcept(0x3d);
        assert_eq!(feraiseexcept(0x05), 0);
        assert_eq!(fetestexcept(0x3d), 0x05);
        assert_eq!(feclearexcept(0x01), 0);
        assert_eq!(fetestexcept(0x3d), 0x04);
    }

    #[test]
    fn lgamma_updates_signgam() {
        unsafe { kinakaze_engine_libm_signgam = 0 };
        let res = lgamma(-0.5);
        assert!(res.is_finite());
        assert_eq!(unsafe { kinakaze_engine_libm_signgam }, -1);

        let res2 = lgamma(2.5);
        assert!(res2.is_finite());
        assert_eq!(unsafe { kinakaze_engine_libm_signgam }, 1);
    }

    #[test]
    fn complex_math_basic() {
        let mut out = [0.0f64; 2];
        compute_cexp(0.0, 0.0, &raw mut out);
        assert!((out[0] - 1.0).abs() < 1e-7);
        assert!(out[1].abs() < 1e-7);

        compute_csin(0.0, 0.0, &raw mut out);
        assert!(out[0].abs() < 1e-7);
        assert!(out[1].abs() < 1e-7);

        compute_ccos(0.0, 0.0, &raw mut out);
        assert!((out[0] - 1.0).abs() < 1e-7);
        assert!(out[1].abs() < 1e-7);
    }
}

mod object_layout;
