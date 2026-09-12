//! Historical libc math entry points call the independently owned libm PE.
//! Unit tests link the math rlib directly; production has one native owner.
#[cfg(not(test))]
mod engine_math {
    #[link(name = "kinakaze_math_imports", kind = "dylib")]
    unsafe extern "sysv64" {
        #[link_name = "kinakaze_engine_libm_ldexp"]
        pub safe fn ldexp(value: f64, exponent: i32) -> f64;
        #[link_name = "kinakaze_engine_libm_ldexpf"]
        pub safe fn ldexpf(value: f32, exponent: i32) -> f32;
        #[link_name = "kinakaze_engine_libm_copysign"]
        pub safe fn copysign(value: f64, sign: f64) -> f64;
        #[link_name = "kinakaze_engine_libm_frexp"]
        pub fn frexp(value: f64, exponent: *mut i32) -> f64;
        #[link_name = "kinakaze_engine_libm_frexpf"]
        pub fn frexpf(value: f32, exponent: *mut i32) -> f32;
        #[link_name = "kinakaze_engine_libm_frexpl"]
        pub fn frexpl();
        #[link_name = "kinakaze_engine_libm_ldexpl"]
        pub fn ldexpl();
        #[link_name = "kinakaze_engine_libm_modf"]
        pub fn modf(value: f64, integral: *mut f64) -> f64;
        #[link_name = "kinakaze_engine_libm_modff"]
        pub fn modff(value: f32, integral: *mut f32) -> f32;
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_copysign(value: f64, sign: f64) -> f64 {
    engine_math::copysign(value, sign)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ldexp(value: f64, exponent: i32) -> f64 {
    let result = engine_math::ldexp(value, exponent);
    if value.is_finite() && value != 0.0 && (!result.is_finite() || result == 0.0) {
        crate::set_errno(34); // ERANGE
    }
    result
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_ldexpf(value: f32, exponent: i32) -> f32 {
    let result = engine_math::ldexpf(value, exponent);
    if value.is_finite() && value != 0.0 && (!result.is_finite() || result == 0.0) {
        crate::set_errno(34);
    }
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_frexp(value: f64, exponent: *mut i32) -> f64 {
    unsafe { engine_math::frexp(value, exponent) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_frexpf(value: f32, exponent: *mut i32) -> f32 {
    unsafe { engine_math::frexpf(value, exponent) }
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn kinakaze_abi_frexpl() {
    core::arch::naked_asm!("jmp {entry}", entry = sym engine_math::frexpl);
}

#[unsafe(no_mangle)]
#[unsafe(naked)]
pub unsafe extern "sysv64" fn kinakaze_abi_ldexpl() {
    core::arch::naked_asm!("jmp {entry}", entry = sym engine_math::ldexpl);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_modf(value: f64, integral: *mut f64) -> f64 {
    unsafe { engine_math::modf(value, integral) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_modff(value: f32, integral: *mut f32) -> f32 {
    unsafe { engine_math::modff(value, integral) }
}
