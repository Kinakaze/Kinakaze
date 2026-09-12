//! The `scanf` family: formatted input scanning.
#![allow(unsafe_op_in_unsafe_fn, unused_assignments)]

use crate::format::VaList;
use core::ffi::{c_char, c_double, c_float, c_int, c_void};
use core::ptr;

const EOF: c_int = -1;

trait ScanSource {
    fn next_char(&mut self) -> Option<u8>;
    fn unget_char(&mut self, ch: u8);
}

struct StrSource<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> ScanSource for StrSource<'a> {
    fn next_char(&mut self) -> Option<u8> {
        if self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c == 0 {
                return None;
            }
            self.pos += 1;
            Some(c)
        } else {
            None
        }
    }

    fn unget_char(&mut self, _ch: u8) {
        if self.pos > 0 {
            self.pos -= 1;
        }
    }
}

struct FileSource {
    file: *mut crate::stdio::File,
}

impl ScanSource for FileSource {
    fn next_char(&mut self) -> Option<u8> {
        let ch = crate::stdio::fgetc(self.file);
        if ch == EOF { None } else { Some(ch as u8) }
    }

    fn unget_char(&mut self, ch: u8) {
        crate::stdio::ungetc(ch as i32, self.file);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LengthModifier {
    None,
    Hh,   // char
    H,    // short
    L,    // long / double
    Ll,   // long long
    J,    // intmax_t
    Z,    // size_t
    T,    // ptrdiff_t
    BigL, // long double
}

unsafe fn scan_format<S: ScanSource>(
    source: &mut S,
    format: *const c_char,
    va: &mut VaList,
) -> c_int {
    if format.is_null() {
        return EOF;
    }
    let mut fmt_ptr = format as *const u8;
    let mut matched_items = 0i32;
    let mut chars_read = 0usize;
    let mut had_input = false;

    while *fmt_ptr != 0 {
        let fc = *fmt_ptr;
        fmt_ptr = fmt_ptr.add(1);

        if fc.is_ascii_whitespace() {
            // Match zero or more whitespace in source
            while let Some(sc) = source.next_char() {
                chars_read += 1;
                had_input = true;
                if !sc.is_ascii_whitespace() {
                    source.unget_char(sc);
                    chars_read -= 1;
                    break;
                }
            }
            continue;
        }

        if fc != b'%' {
            match source.next_char() {
                Some(sc) => {
                    chars_read += 1;
                    had_input = true;
                    if sc != fc {
                        source.unget_char(sc);
                        chars_read -= 1;
                        break;
                    }
                }
                None => break,
            }
            continue;
        }

        // Parse '%'
        if *fmt_ptr == b'%' {
            fmt_ptr = fmt_ptr.add(1);
            match source.next_char() {
                Some(sc) => {
                    chars_read += 1;
                    had_input = true;
                    if sc != b'%' {
                        source.unget_char(sc);
                        chars_read -= 1;
                        break;
                    }
                }
                None => break,
            }
            continue;
        }

        // Optional assignment suppression '*'
        let suppress = if *fmt_ptr == b'*' {
            fmt_ptr = fmt_ptr.add(1);
            true
        } else {
            false
        };

        // Optional max field width
        let mut width: Option<usize> = None;
        if (*fmt_ptr).is_ascii_digit() {
            let mut w = 0usize;
            while (*fmt_ptr).is_ascii_digit() {
                w = w
                    .saturating_mul(10)
                    .saturating_add((*fmt_ptr - b'0') as usize);
                fmt_ptr = fmt_ptr.add(1);
            }
            if w > 0 {
                width = Some(w);
            }
        }

        // Optional length modifier
        let mut len_mod = LengthModifier::None;
        match *fmt_ptr {
            b'h' => {
                fmt_ptr = fmt_ptr.add(1);
                if *fmt_ptr == b'h' {
                    fmt_ptr = fmt_ptr.add(1);
                    len_mod = LengthModifier::Hh;
                } else {
                    len_mod = LengthModifier::H;
                }
            }
            b'l' => {
                fmt_ptr = fmt_ptr.add(1);
                if *fmt_ptr == b'l' {
                    fmt_ptr = fmt_ptr.add(1);
                    len_mod = LengthModifier::Ll;
                } else {
                    len_mod = LengthModifier::L;
                }
            }
            b'j' => {
                fmt_ptr = fmt_ptr.add(1);
                len_mod = LengthModifier::J;
            }
            b'z' => {
                fmt_ptr = fmt_ptr.add(1);
                len_mod = LengthModifier::Z;
            }
            b't' => {
                fmt_ptr = fmt_ptr.add(1);
                len_mod = LengthModifier::T;
            }
            b'L' => {
                fmt_ptr = fmt_ptr.add(1);
                len_mod = LengthModifier::BigL;
            }
            _ => {}
        }

        let conv = *fmt_ptr;
        if conv == 0 {
            break;
        }
        fmt_ptr = fmt_ptr.add(1);

        if conv == b'n' {
            if !suppress {
                let dest: *mut c_int = va.next_integer();
                if !dest.is_null() {
                    match len_mod {
                        LengthModifier::Hh => *(dest as *mut i8) = chars_read as i8,
                        LengthModifier::H => *(dest as *mut i16) = chars_read as i16,
                        LengthModifier::L
                        | LengthModifier::Ll
                        | LengthModifier::J
                        | LengthModifier::Z
                        | LengthModifier::T => {
                            *(dest as *mut i64) = chars_read as i64;
                        }
                        _ => *dest = chars_read as c_int,
                    }
                }
            }
            continue;
        }

        if conv == b'c' {
            let count = width.unwrap_or(1);
            let mut buf = Vec::new();
            for _ in 0..count {
                match source.next_char() {
                    Some(sc) => {
                        chars_read += 1;
                        had_input = true;
                        buf.push(sc);
                    }
                    None => break,
                }
            }
            if buf.is_empty() {
                break;
            }
            if !suppress {
                let dest: *mut u8 = va.next_integer();
                if !dest.is_null() {
                    ptr::copy_nonoverlapping(buf.as_ptr(), dest, buf.len());
                }
                matched_items += 1;
            }
            continue;
        }

        if conv == b'[' {
            // Scanset
            let mut invert = false;
            if *fmt_ptr == b'^' {
                invert = true;
                fmt_ptr = fmt_ptr.add(1);
            }
            let mut set = [false; 256];
            let mut first = true;
            while *fmt_ptr != 0 && (first || *fmt_ptr != b']') {
                first = false;
                let ch = *fmt_ptr;
                fmt_ptr = fmt_ptr.add(1);
                if *fmt_ptr == b'-' && *fmt_ptr.add(1) != 0 && *fmt_ptr.add(1) != b']' {
                    fmt_ptr = fmt_ptr.add(1);
                    let end_ch = *fmt_ptr;
                    fmt_ptr = fmt_ptr.add(1);
                    for b in ch..=end_ch {
                        set[b as usize] = true;
                    }
                } else {
                    set[ch as usize] = true;
                }
            }
            if *fmt_ptr == b']' {
                fmt_ptr = fmt_ptr.add(1);
            }

            let max_w = width.unwrap_or(usize::MAX);
            let mut buf = Vec::new();
            while buf.len() < max_w {
                match source.next_char() {
                    Some(sc) => {
                        let matches = set[sc as usize] != invert;
                        if matches {
                            chars_read += 1;
                            had_input = true;
                            buf.push(sc);
                        } else {
                            source.unget_char(sc);
                            break;
                        }
                    }
                    None => break,
                }
            }
            if buf.is_empty() {
                break;
            }
            if !suppress {
                let dest: *mut u8 = va.next_integer();
                if !dest.is_null() {
                    ptr::copy_nonoverlapping(buf.as_ptr(), dest, buf.len());
                    *dest.add(buf.len()) = 0;
                }
                matched_items += 1;
            }
            continue;
        }

        // Other conversions skip leading whitespace
        while let Some(sc) = source.next_char() {
            chars_read += 1;
            had_input = true;
            if !sc.is_ascii_whitespace() {
                source.unget_char(sc);
                chars_read -= 1;
                break;
            }
        }

        if conv == b's' {
            let max_w = width.unwrap_or(usize::MAX);
            let mut buf = Vec::new();
            while buf.len() < max_w {
                match source.next_char() {
                    Some(sc) => {
                        if sc.is_ascii_whitespace() {
                            source.unget_char(sc);
                            break;
                        }
                        chars_read += 1;
                        had_input = true;
                        buf.push(sc);
                    }
                    None => break,
                }
            }
            if buf.is_empty() {
                break;
            }
            if !suppress {
                let dest: *mut u8 = va.next_integer();
                if !dest.is_null() {
                    ptr::copy_nonoverlapping(buf.as_ptr(), dest, buf.len());
                    *dest.add(buf.len()) = 0;
                }
                matched_items += 1;
            }
            continue;
        }

        if conv == b'd'
            || conv == b'i'
            || conv == b'u'
            || conv == b'x'
            || conv == b'X'
            || conv == b'o'
            || conv == b'p'
            || conv == b'b'
        {
            let max_w = width.unwrap_or(usize::MAX);
            let mut token = Vec::new();
            let mut base: u32 = match conv {
                b'd' | b'u' => 10,
                b'o' => 8,
                b'x' | b'X' | b'p' => 16,
                b'b' => 2,
                _ => 0, // 'i' auto-detects
            };

            // Read sign
            let mut is_negative = false;
            if let Some(sc) = source.next_char() {
                chars_read += 1;
                had_input = true;
                if sc == b'-' {
                    is_negative = true;
                    token.push(sc);
                } else if sc == b'+' {
                    token.push(sc);
                } else {
                    source.unget_char(sc);
                    chars_read -= 1;
                }
            }

            // Read prefix 0x or 0b
            if base == 0 || base == 16 || base == 2 {
                if let Some(c0) = source.next_char() {
                    chars_read += 1;
                    if c0 == b'0' {
                        if let Some(c1) = source.next_char() {
                            chars_read += 1;
                            if (c1 == b'x' || c1 == b'X') && (base == 0 || base == 16) {
                                base = 16;
                            } else if (c1 == b'b' || c1 == b'B') && (base == 0 || base == 2) {
                                base = 2;
                            } else {
                                source.unget_char(c1);
                                chars_read -= 1;
                                token.push(c0);
                                if base == 0 {
                                    base = 8;
                                }
                            }
                        } else {
                            token.push(c0);
                            if base == 0 {
                                base = 8;
                            }
                        }
                    } else {
                        source.unget_char(c0);
                        chars_read -= 1;
                        if base == 0 {
                            base = 10;
                        }
                    }
                }
            }

            while token.len() < max_w {
                match source.next_char() {
                    Some(sc) => {
                        let valid = match base {
                            2 => sc == b'0' || sc == b'1',
                            8 => (b'0'..=b'7').contains(&sc),
                            10 => sc.is_ascii_digit(),
                            16 => sc.is_ascii_hexdigit(),
                            _ => sc.is_ascii_digit(),
                        };
                        if valid {
                            chars_read += 1;
                            had_input = true;
                            token.push(sc);
                        } else {
                            source.unget_char(sc);
                            break;
                        }
                    }
                    None => break,
                }
            }

            if token.is_empty() {
                break;
            }

            let num_str = String::from_utf8_lossy(&token);
            let parsed_u64 = u64::from_str_radix(
                num_str.trim_start_matches('+').trim_start_matches('-'),
                base,
            );
            let Ok(val) = parsed_u64 else { break };
            let final_val = if is_negative {
                (val as i64).wrapping_neg() as u64
            } else {
                val
            };

            if !suppress {
                let dest: *mut c_void = va.next_integer();
                if !dest.is_null() {
                    match len_mod {
                        LengthModifier::Hh => *(dest as *mut u8) = final_val as u8,
                        LengthModifier::H => *(dest as *mut u16) = final_val as u16,
                        LengthModifier::L
                        | LengthModifier::Ll
                        | LengthModifier::J
                        | LengthModifier::Z
                        | LengthModifier::T => {
                            *(dest as *mut u64) = final_val;
                        }
                        _ => *(dest as *mut u32) = final_val as u32,
                    }
                }
                matched_items += 1;
            }
            continue;
        }

        if conv == b'f' || conv == b'e' || conv == b'E' || conv == b'g' || conv == b'G' {
            let max_w = width.unwrap_or(usize::MAX);
            let mut token = Vec::new();
            while token.len() < max_w {
                match source.next_char() {
                    Some(sc) => {
                        if sc.is_ascii_digit()
                            || sc == b'+'
                            || sc == b'-'
                            || sc == b'.'
                            || sc == b'e'
                            || sc == b'E'
                        {
                            chars_read += 1;
                            had_input = true;
                            token.push(sc);
                        } else {
                            source.unget_char(sc);
                            break;
                        }
                    }
                    None => break,
                }
            }
            if token.is_empty() {
                break;
            }
            let s = String::from_utf8_lossy(&token);
            let Ok(val) = s.parse::<f64>() else { break };
            if !suppress {
                let dest: *mut c_void = va.next_integer();
                if !dest.is_null() {
                    if len_mod == LengthModifier::L || len_mod == LengthModifier::BigL {
                        *(dest as *mut c_double) = val;
                    } else {
                        *(dest as *mut c_float) = val as f32;
                    }
                }
                matched_items += 1;
            }
            continue;
        }
    }

    if matched_items == 0 && !had_input {
        EOF
    } else {
        matched_items
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_isoc23_fscanf_impl(
    file: *mut crate::stdio::File,
    format: *const c_char,
    va: *mut VaList,
) -> c_int {
    if file.is_null() || format.is_null() || va.is_null() {
        return EOF;
    }
    let mut source = FileSource { file };
    unsafe { scan_format(&mut source, format, &mut *va) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_isoc23_sscanf_impl(
    string: *const c_char,
    format: *const c_char,
    va: *mut VaList,
) -> c_int {
    if string.is_null() || format.is_null() || va.is_null() {
        return EOF;
    }
    let len = unsafe { crate::string::strlen(string) };
    let bytes = unsafe { core::slice::from_raw_parts(string as *const u8, len) };
    let mut source = StrSource { bytes, pos: 0 };
    unsafe { scan_format(&mut source, format, &mut *va) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vscanf(format: *const c_char, va: *mut VaList) -> c_int {
    unsafe { kinakaze_isoc23_fscanf_impl(crate::stdio::exports::kinakaze_stdin(), format, va) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vfscanf(
    file: *mut crate::stdio::File,
    format: *const c_char,
    va: *mut VaList,
) -> c_int {
    unsafe { kinakaze_isoc23_fscanf_impl(file, format, va) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vsscanf(
    string: *const c_char,
    format: *const c_char,
    va: *mut VaList,
) -> c_int {
    unsafe { kinakaze_isoc23_sscanf_impl(string, format, va) }
}
