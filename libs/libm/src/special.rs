//! Special functions share the existing musl math backend and Linux ABI.
use core::ffi::c_int;
macro_rules! unary {
    ($($name:ident),* $(,)?) => {$ (
        #[unsafe(export_name = concat!("kinakaze_engine_libm_", stringify!($name)))]
        pub extern "sysv64" fn $name(x: f64) -> f64 { musl_libm::$name(x) }
    )*};
}
unary!(j0, j1, y0, y1, exp10);

#[unsafe(export_name = "kinakaze_engine_libm_ilogb")]
pub extern "sysv64" fn ilogb(value: f64) -> c_int {
    if value == 0.0 || !value.is_finite() {
        kinakaze_tls::set_errno(33);
        super::feraiseexcept(1);
        return if value.is_infinite() {
            c_int::MAX
        } else {
            c_int::MIN
        };
    }
    musl_libm::ilogb(value)
}

#[unsafe(export_name = "kinakaze_engine_libm_ilogbf")]
pub extern "sysv64" fn ilogbf(value: f32) -> c_int {
    // Conversion is exact, including subnormals, and does not change the exponent.
    ilogb(value as f64)
}
#[unsafe(export_name = "kinakaze_engine_libm_exp10f")]
pub extern "sysv64" fn exp10f(x: f32) -> f32 {
    musl_libm::exp10f(x)
}
#[unsafe(export_name = "kinakaze_engine_libm_jnf")]
pub extern "sysv64" fn jnf(n: c_int, x: f32) -> f32 {
    musl_libm::jnf(n, x)
}
#[unsafe(export_name = "kinakaze_engine_libm_ynf")]
pub extern "sysv64" fn ynf(n: c_int, x: f32) -> f32 {
    musl_libm::ynf(n, x)
}
#[unsafe(export_name = "kinakaze_engine_libm_jn")]
pub extern "sysv64" fn jn(n: c_int, x: f64) -> f64 {
    musl_libm::jn(n, x)
}
#[unsafe(export_name = "kinakaze_engine_libm_yn")]
pub extern "sysv64" fn yn(n: c_int, x: f64) -> f64 {
    musl_libm::yn(n, x)
}
#[unsafe(export_name = "kinakaze_engine_libm_lgamma_r")]
pub unsafe extern "sysv64" fn lgamma_r(x: f64, sign: *mut c_int) -> f64 {
    let (value, direction) = musl_libm::lgamma_r(x);
    if !sign.is_null() {
        unsafe {
            *sign = direction;
        }
    }
    value
}
#[unsafe(export_name = "kinakaze_engine_libm_significand")]
pub extern "sysv64" fn significand(value: f64) -> f64 {
    if value == 0.0 || !value.is_finite() {
        return value + value;
    }
    musl_libm::frexp(value).0 * 2.0
}
#[unsafe(export_name = "kinakaze_engine_libm_significandf")]
pub extern "sysv64" fn significandf(value: f32) -> f32 {
    if value == 0.0 || !value.is_finite() {
        return value + value;
    }
    musl_libm::frexpf(value).0 * 2.0
}
#[unsafe(export_name = "kinakaze_engine_libm_logb")]
pub extern "sysv64" fn logb(value: f64) -> f64 {
    if value.is_nan() {
        return value + value;
    }
    if value.is_infinite() {
        return f64::INFINITY;
    }
    if value == 0.0 {
        kinakaze_tls::set_errno(34);
        super::feraiseexcept(4);
        return f64::NEG_INFINITY;
    }
    musl_libm::ilogb(value) as f64
}
#[unsafe(export_name = "kinakaze_engine_libm_scalb")]
pub extern "sysv64" fn scalb(value: f64, exponent: f64) -> f64 {
    if value.is_nan() || exponent.is_nan() {
        return value * exponent;
    }
    if exponent.is_infinite() {
        return if exponent > 0.0 {
            value * exponent
        } else {
            value / -exponent
        };
    }
    if musl_libm::trunc(exponent) != exponent {
        kinakaze_tls::set_errno(33);
        super::feraiseexcept(1);
        return f64::NAN;
    }
    let result = musl_libm::scalbn(value, exponent as i32);
    if value.is_finite() && value != 0.0 && (result.is_infinite() || result == 0.0) {
        kinakaze_tls::set_errno(34);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn subnormals_special_values_and_orders() {
        assert_eq!(ilogb(f64::from_bits(1)), -1074);
        assert_eq!(ilogbf(f32::from_bits(1)), -149);
        assert_eq!(ilogbf(-8.0), 3);
        super::super::feclearexcept(0x3f);
        assert_eq!(ilogbf(0.0), c_int::MIN);
        assert_eq!(ilogb(f64::NAN), c_int::MIN);
        assert_eq!(ilogb(f64::INFINITY), c_int::MAX);
        assert_eq!(kinakaze_tls::errno(), 33);
        assert_ne!(super::super::fetestexcept(1), 0);
        super::super::feclearexcept(0x3f);
        assert_eq!(significand(f64::from_bits(1)), 1.0);
        assert_eq!(significand(-6.0), -1.5);
        assert_eq!(significand(-0.0).to_bits(), (-0.0f64).to_bits());
        assert_eq!(significandf(f32::from_bits(1)), 1.0);
        assert_eq!(logb(f64::from_bits(1)), -1074.0);
        assert_eq!(logb(-f64::INFINITY), f64::INFINITY);
        assert_eq!(scalb(1.5, 3.0), 12.0);
        assert_eq!(j0(0.0), 1.0);
        assert_eq!(j1(0.0), 0.0);
        assert_eq!(jn(2, 0.0), 0.0);
        assert_eq!(exp10(3.0), 1000.0);
        let mut sign = 0;
        let gamma = unsafe { lgamma_r(-0.5, &raw mut sign) };
        assert_eq!(sign, -1);
        assert!((gamma - 1.2655121234846454).abs() < 1e-14);
        super::super::feclearexcept(0x3f);
        assert_eq!(logb(0.0), f64::NEG_INFINITY);
        assert_ne!(super::super::fetestexcept(4), 0);
        assert!(scalb(1.0, 0.5).is_nan());
        assert_ne!(super::super::fetestexcept(1), 0);
        super::super::feclearexcept(0x3f);
    }
}
