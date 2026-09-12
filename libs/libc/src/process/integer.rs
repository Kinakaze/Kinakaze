//! Shared LP64 integer conversion with separate signed/unsigned limits.
use core::ffi::{c_char, c_int};
pub(super) unsafe fn parse(
    text: *const c_char,
    end: *mut *const c_char,
    base: c_int,
    signed: bool,
) -> u64 {
    if !end.is_null() {
        unsafe {
            *end = text;
        }
    }
    if text.is_null() || base != 0 && !(2..=36).contains(&base) {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    }
    let peek = |offset| unsafe { *text.add(offset) as u8 };
    let mut at = 0;
    while peek(at).is_ascii_whitespace() {
        at += 1;
    }
    let negative = peek(at) == b'-';
    if negative || peek(at) == b'+' {
        at += 1;
    }
    let mut radix = base as u64;
    if (radix == 0 || radix == 16)
        && peek(at) == b'0'
        && peek(at + 1).eq_ignore_ascii_case(&b'x')
        && peek(at + 2).is_ascii_hexdigit()
    {
        radix = 16;
        at += 2;
    } else if radix == 0 {
        radix = if peek(at) == b'0' { 8 } else { 10 };
    }
    let limit = if signed {
        i64::MAX as u64 + u64::from(negative)
    } else {
        u64::MAX
    };
    let (cutoff, last) = (limit / radix, limit % radix);
    let (start, mut value, mut overflow) = (at, 0u64, false);
    loop {
        let byte = peek(at);
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'z' => byte - b'a' + 10,
            b'A'..=b'Z' => byte - b'A' + 10,
            _ => break,
        } as u64;
        if digit >= radix {
            break;
        }
        at += 1;
        if value > cutoff || value == cutoff && digit > last {
            overflow = true;
        } else if !overflow {
            value = value * radix + digit;
        }
    }
    if !end.is_null() && at != start {
        unsafe {
            *end = text.add(at);
        }
    }
    if overflow {
        crate::set_errno(kinakaze_vfs::ERANGE);
        return if signed && negative {
            i64::MIN as u64
        } else {
            limit
        };
    }
    if negative {
        value.wrapping_neg()
    } else {
        value
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_unsigned_hashes_and_signed_minimum_are_not_overflow() {
        for (text, radix, signed, expected) in [
            (c"d168fd5efc2d2b72!", 16, false, 0xd168fd5efc2d2b72),
            (c"18446744073709551615!", 10, false, u64::MAX),
            (c"-9223372036854775808!", 10, true, i64::MIN as u64),
            (c"-1!", 10, false, u64::MAX),
            (c"  +0x2a!", 0, true, 42),
        ] {
            let mut end = core::ptr::null();
            crate::set_errno(7);
            assert_eq!(
                unsafe { parse(text.as_ptr(), &raw mut end, radix, signed) },
                expected
            );
            assert_eq!(unsafe { *end }, b'!' as c_char);
            assert_eq!(kinakaze_tls::errno(), 7);
        }
    }
    #[test]
    fn overflow_consumes_digits_and_no_conversion_retains_start() {
        for (text, signed, expected) in [
            (c"18446744073709551616000!", false, u64::MAX),
            (c"-18446744073709551616!", false, u64::MAX),
            (c"9223372036854775808!", true, i64::MAX as u64),
        ] {
            let mut end = core::ptr::null();
            crate::set_errno(0);
            assert_eq!(
                unsafe { parse(text.as_ptr(), &raw mut end, 10, signed) },
                expected
            );
            assert_eq!(unsafe { *end }, b'!' as c_char);
            assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ERANGE);
        }
        let mut end = core::ptr::null();
        let text = c" -xyz";
        assert_eq!(unsafe { parse(text.as_ptr(), &raw mut end, 10, false) }, 0);
        assert_eq!(end, text.as_ptr());
        let text = c"0x!";
        assert_eq!(unsafe { parse(text.as_ptr(), &raw mut end, 16, false) }, 0);
        assert_eq!(unsafe { *end }, b'x' as c_char);
    }
}
