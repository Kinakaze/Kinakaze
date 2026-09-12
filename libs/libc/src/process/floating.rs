//! C-locale conversion through UCRT's correctly rounded decimal/hex parser.
//! Native locale pointers remain process-local and are created only on use.
use core::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

#[link(name = "ucrt")]
unsafe extern "C" {
    fn _create_locale(category: c_int, name: *const c_char) -> *mut c_void;
    fn _strtod_l(text: *const c_char, end: *mut *mut c_char, locale: *mut c_void) -> f64;
    fn _strtof_l(text: *const c_char, end: *mut *mut c_char, locale: *mut c_void) -> f32;
    fn _errno() -> *mut c_int;
}

fn locale() -> *mut c_void {
    static LOCALE: OnceLock<usize> = OnceLock::new();
    *LOCALE.get_or_init(|| unsafe { _create_locale(0, c"C".as_ptr()) as usize }) as *mut c_void
}

unsafe fn convert<T: Default>(
    text: *const c_char,
    end: *mut *const c_char,
    parser: unsafe extern "C" fn(*const c_char, *mut *mut c_char, *mut c_void) -> T,
) -> T {
    if !end.is_null() {
        unsafe { *end = text };
    }
    if text.is_null() {
        crate::set_errno(22);
        return T::default();
    }
    unsafe {
        let host_errno = _errno();
        let saved = *host_errno;
        let native_locale = locale();
        if native_locale.is_null() {
            *host_errno = saved;
            crate::set_errno(12);
            return T::default();
        }
        *host_errno = 0;
        let result = parser(text, end.cast(), native_locale);
        let error = *host_errno;
        *host_errno = saved;
        if error != 0 {
            // UCRT EINVAL/ERANGE have the same values as Linux.
            crate::set_errno(error);
        }
        result
    }
}

pub(super) unsafe fn double(text: *const c_char, end: *mut *const c_char) -> f64 {
    unsafe { convert(text, end, _strtod_l) }
}

pub(super) unsafe fn float(text: *const c_char, end: *mut *const c_char) -> f32 {
    unsafe { convert(text, end, _strtof_l) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_grammar_end_pointers_and_range_errors() {
        unsafe {
            let mut end = core::ptr::null();
            assert_eq!(double(c"  -0x1.8p+2!".as_ptr(), &raw mut end), -6.0);
            assert_eq!(*end, b'!' as c_char);
            assert_eq!(double(c"12.5  ".as_ptr(), &raw mut end), 12.5);
            assert_eq!(*end, b' ' as c_char);
            assert_eq!(double(c"1e+oops".as_ptr(), &raw mut end), 1.0);
            assert_eq!(*end, b'e' as c_char);
            let invalid = c"  nope";
            assert_eq!(double(invalid.as_ptr(), &raw mut end), 0.0);
            assert_eq!(end, invalid.as_ptr());
            crate::set_errno(7);
            assert_eq!(
                float(c"-0".as_ptr(), &raw mut end).to_bits(),
                (-0.0f32).to_bits()
            );
            assert_eq!(kinakaze_tls::errno(), 7);
            assert!(double(c"1e9999".as_ptr(), &raw mut end).is_infinite());
            assert_eq!(kinakaze_tls::errno(), 34);
            assert_eq!(float(c"1e-9999".as_ptr(), &raw mut end), 0.0);
            assert_eq!(kinakaze_tls::errno(), 34);
            // A direct f32 parse avoids double rounding at a halfway value.
            assert_eq!(
                float(c"1.000000059604644775390626".as_ptr(), &raw mut end).to_bits(),
                0x3f800001
            );
        }
    }
}
