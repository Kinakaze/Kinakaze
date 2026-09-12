//! GNU long double uses an 80-bit value in x87 ST(0), including on Windows.
//! Lexing handles C prefixes; APFloat rounds directly to 64 significand bits.
use core::ffi::{c_char, c_void};
use rustc_apfloat::{Float, Round, Status, ieee::X87DoubleExtended as Extended};

fn round() -> Round {
    let mut control = 0u16;
    unsafe {
        core::arch::asm!("fnstcw [{}]", in(reg) &mut control, options(nostack, preserves_flags));
    }
    match (control >> 10) & 3 {
        1 => Round::TowardNegative,
        2 => Round::TowardPositive,
        3 => Round::TowardZero,
        _ => Round::NearestTiesToEven,
    }
}

fn parse(bytes: &[u8]) -> (u128, usize, bool) {
    let mut at = 0;
    while bytes
        .get(at)
        .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c'))
    {
        at += 1;
    }
    let start = at;
    let negative = bytes.get(at) == Some(&b'-');
    if matches!(bytes.get(at), Some(b'+' | b'-')) {
        at += 1;
    }
    let sign = |value: Extended| if negative { -value } else { value };
    let word = |at: usize, expected: &[u8]| {
        bytes
            .get(at..at + expected.len())
            .is_some_and(|v| v.eq_ignore_ascii_case(expected))
    };
    if word(at, b"inf") {
        at += if word(at, b"infinity") { 8 } else { 3 };
        return (sign(Extended::INFINITY).to_bits(), at, false);
    }
    if word(at, b"nan") {
        at += 3;
        let mut payload = None;
        if bytes.get(at) == Some(&b'(') {
            let begin = at + 1;
            let mut end = begin;
            while bytes
                .get(end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
            {
                end += 1;
            }
            if bytes.get(end) == Some(&b')') {
                let token = &bytes[begin..end];
                let (digits, radix) = if token.starts_with(b"0x") || token.starts_with(b"0X") {
                    (&token[2..], 16)
                } else if token.starts_with(b"0") {
                    (token, 8)
                } else {
                    (token, 10)
                };
                payload = core::str::from_utf8(digits)
                    .ok()
                    .and_then(|s| u128::from_str_radix(s, radix).ok());
                at = end + 1;
            }
        }
        return (sign(Extended::qnan(payload)).to_bits(), at, false);
    }
    let mut hex = word(at, b"0x");
    if hex {
        let digit = at + 2;
        hex = bytes.get(digit).is_some_and(u8::is_ascii_hexdigit)
            || bytes.get(digit) == Some(&b'.')
                && bytes.get(digit + 1).is_some_and(u8::is_ascii_hexdigit);
        if hex {
            at += 2;
        }
    }
    let digit = |b: &u8| {
        if hex {
            b.is_ascii_hexdigit()
        } else {
            b.is_ascii_digit()
        }
    };
    let mut digits = 0;
    while bytes.get(at).is_some_and(digit) {
        at += 1;
        digits += 1;
    }
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while bytes.get(at).is_some_and(digit) {
            at += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return (0, 0, false);
    }
    let exponent_marker = at;
    let exponent = if hex { b'p' } else { b'e' };
    if bytes
        .get(at)
        .is_some_and(|b| b.to_ascii_lowercase() == exponent)
    {
        let mut cursor = at + 1;
        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        let first = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor > first {
            at = cursor;
        }
    }
    // APFloat requires a binary exponent on a hexadecimal literal; C does not.
    let token = core::str::from_utf8(&bytes[start..at]).unwrap();
    let token = token.strip_prefix('+').unwrap_or(token);
    let extended;
    let normalized = if hex && at == exponent_marker {
        extended = format!("{token}p0");
        extended.as_str()
    } else {
        token
    };
    match Extended::from_str_r(normalized, round()) {
        Ok(result) => (
            result.value.to_bits(),
            at,
            result
                .status
                .intersects(Status::OVERFLOW | Status::UNDERFLOW),
        ),
        Err(_) => (0, 0, false),
    }
}

unsafe extern "sysv64" fn convert(text: *const c_char, end: *mut *const c_char, output: *mut u128) {
    if !end.is_null() {
        unsafe {
            *end = text;
        }
    }
    if text.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        unsafe {
            output.write(0);
        }
        return;
    }
    let bytes = unsafe { core::ffi::CStr::from_ptr(text) }.to_bytes();
    let (bits, consumed, range) = parse(bytes);
    unsafe {
        output.write(bits);
        if !end.is_null() {
            *end = text.add(consumed);
        }
    }
    if range {
        crate::set_errno(kinakaze_vfs::ERANGE);
    }
}

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtold(_text: *const c_char, _end: *mut *const c_char) {
    core::arch::naked_asm!(
        "sub rsp, 24", "mov rdx, rsp", "call {convert}",
        "fld tbyte ptr [rsp]", "add rsp, 24", "ret", convert = sym convert,
    );
}
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strtold_l(
    _text: *const c_char,
    _end: *mut *const c_char,
    _locale: *mut c_void,
) {
    // Both supported numeric locales, C and C.UTF-8, use the same grammar.
    core::arch::naked_asm!("jmp {entry}", entry = sym kinakaze_abi_strtold);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extended_precision_range_and_c_prefixes() {
        assert_eq!(
            parse(b"18446744073709551615!").0,
            (0x403eu128 << 64) | u64::MAX as u128
        );
        assert_eq!(
            parse(b"0x1.0000000000000002p0").0,
            (0x3fffu128 << 64) | 0x8000000000000001
        );
        assert_eq!(parse(b"0x1.8!").0, (0x3fffu128 << 64) | 0xc000000000000000);
        assert_eq!(parse(b"  -2e+oops").1, 4);
        assert_eq!(parse(b" nope").1, 0);
        assert_eq!(parse(b"-0").0, 1u128 << 79);
        assert!(!parse(b"1e4000").2);
        assert!(parse(b"1e5000").2);
        assert!(parse(b"1e-5000").2);
        assert_eq!(parse(b"NaN(0x123)!").1, 10);
    }
}
