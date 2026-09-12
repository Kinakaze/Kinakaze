//! Exact stateless Unicode/ASCII/Latin-1 conversion. Unsupported codecs fail at
//! open; malformed input never becomes a successful byte-for-byte copy.
use core::ffi::{CStr, c_char, c_int, c_void};
use core::{ptr, slice};
use kinakaze_alloc::guest;
use kinakaze_vfs::{EBADF, EFAULT, EINVAL, ENOMEM};

const E2BIG: i32 = 7;
const EILSEQ: i32 = 84;
const FAILED: usize = usize::MAX;
const MAGIC: u32 = 0x4943_5631;

#[derive(Clone, Copy)]
enum Encoding {
    Ascii,
    Latin1,
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
}
struct Converter {
    magic: u32,
    from: Encoding,
    to: Encoding,
}

fn error(value: i32) -> usize {
    crate::set_errno(value);
    FAILED
}
unsafe fn encoding(name: *const c_char) -> Result<Encoding, i32> {
    if name.is_null() {
        return Err(EINVAL);
    }
    let name = unsafe { CStr::from_ptr(name) }
        .to_str()
        .map_err(|_| EINVAL)?;
    if name.is_empty() {
        return Ok(if crate::locale::utf8() {
            Encoding::Utf8
        } else {
            Encoding::Ascii
        });
    }
    let normalized: String = name
        .chars()
        .filter(|&c| c != '-' && c != '_' && c != ' ')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match normalized.as_str() {
        "UTF8" => Ok(Encoding::Utf8),
        "ASCII" | "USASCII" | "ANSIX3.41968" | "ANSIX3.41986" | "ISO646US" => Ok(Encoding::Ascii),
        "ISO88591" | "LATIN1" => Ok(Encoding::Latin1),
        "UTF16LE" => Ok(Encoding::Utf16Le),
        "UTF16BE" => Ok(Encoding::Utf16Be),
        "UTF32LE" | "UCS4LE" | "WCHART" => Ok(Encoding::Utf32Le),
        "UTF32BE" | "UCS4BE" => Ok(Encoding::Utf32Be),
        _ => Err(EINVAL),
    }
}

fn decode(encoding: Encoding, input: &[u8]) -> Result<(char, usize), i32> {
    let scalar = match encoding {
        Encoding::Ascii if input[0] >= 128 => return Err(EILSEQ),
        Encoding::Ascii | Encoding::Latin1 => return Ok((input[0] as char, 1)),
        Encoding::Utf8 => {
            let width = match input[0] {
                0..=0x7f => 1,
                0xc2..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf4 => 4,
                _ => return Err(EILSEQ),
            };
            let text = core::str::from_utf8(&input[..width.min(input.len())]).map_err(|e| {
                if e.error_len().is_none() {
                    EINVAL
                } else {
                    EILSEQ
                }
            })?;
            return Ok((text.chars().next().unwrap(), width));
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            if input.len() < 2 {
                return Err(EINVAL);
            }
            let unit = |i| {
                let bytes = [input[i], input[i + 1]];
                if matches!(encoding, Encoding::Utf16Le) {
                    u16::from_le_bytes(bytes)
                } else {
                    u16::from_be_bytes(bytes)
                }
            };
            let first = unit(0);
            if (0xdc00..=0xdfff).contains(&first) {
                return Err(EILSEQ);
            }
            if (0xd800..=0xdbff).contains(&first) {
                if input.len() < 4 {
                    return Err(EINVAL);
                }
                let second = unit(2);
                if !(0xdc00..=0xdfff).contains(&second) {
                    return Err(EILSEQ);
                }
                (
                    0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00,
                    4,
                )
            } else {
                (u32::from(first), 2)
            }
        }
        Encoding::Utf32Le | Encoding::Utf32Be => {
            if input.len() < 4 {
                return Err(EINVAL);
            }
            let bytes = input[..4].try_into().unwrap();
            (
                if matches!(encoding, Encoding::Utf32Le) {
                    u32::from_le_bytes(bytes)
                } else {
                    u32::from_be_bytes(bytes)
                },
                4,
            )
        }
    };
    char::from_u32(scalar.0)
        .map(|c| (c, scalar.1))
        .ok_or(EILSEQ)
}

fn encode(encoding: Encoding, scalar: char, output: &mut [u8; 4]) -> Result<usize, i32> {
    match encoding {
        Encoding::Ascii | Encoding::Latin1 => {
            let limit = if matches!(encoding, Encoding::Ascii) {
                127
            } else {
                255
            };
            if scalar as u32 > limit {
                return Err(EILSEQ);
            }
            output[0] = scalar as u8;
            Ok(1)
        }
        Encoding::Utf8 => Ok(scalar.encode_utf8(output).len()),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let mut units = [0; 2];
            let text = scalar.encode_utf16(&mut units);
            for (i, &unit) in text.iter().enumerate() {
                let bytes = if matches!(encoding, Encoding::Utf16Le) {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                };
                output[i * 2..i * 2 + 2].copy_from_slice(&bytes);
            }
            Ok(text.len() * 2)
        }
        Encoding::Utf32Le | Encoding::Utf32Be => {
            *output = if matches!(encoding, Encoding::Utf32Le) {
                (scalar as u32).to_le_bytes()
            } else {
                (scalar as u32).to_be_bytes()
            };
            Ok(4)
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_iconv_open(
    to: *const c_char,
    from: *const c_char,
) -> *mut c_void {
    let result = (|| {
        let from = unsafe { encoding(from) }?;
        let to = unsafe { encoding(to) }?;
        let descriptor = unsafe { guest::malloc(size_of::<Converter>()).cast::<Converter>() };
        if descriptor.is_null() {
            return Err(ENOMEM);
        }
        unsafe {
            descriptor.write(Converter {
                magic: MAGIC,
                from,
                to,
            })
        };
        Ok(descriptor.cast())
    })();
    result.unwrap_or_else(|e| error(e) as *mut c_void)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_iconv(
    descriptor: *mut c_void,
    input: *mut *mut c_char,
    input_left: *mut usize,
    output: *mut *mut c_char,
    output_left: *mut usize,
) -> usize {
    if descriptor.is_null() || descriptor as usize == FAILED {
        return error(EBADF);
    }
    let converter = unsafe { &*descriptor.cast::<Converter>() };
    if converter.magic != MAGIC {
        return error(EBADF);
    }
    if input.is_null() || unsafe { (*input).is_null() } {
        return 0;
    }
    if input_left.is_null() || output_left.is_null() || output.is_null() {
        return error(EFAULT);
    }
    while unsafe { *input_left } != 0 {
        let bytes = unsafe { slice::from_raw_parts((*input).cast(), *input_left) };
        let (scalar, consumed) = match decode(converter.from, bytes) {
            Ok(v) => v,
            Err(e) => return error(e),
        };
        let mut encoded = [0; 4];
        let produced = match encode(converter.to, scalar, &mut encoded) {
            Ok(n) => n,
            Err(e) => return error(e),
        };
        if unsafe { *output_left } < produced {
            return error(E2BIG);
        }
        if unsafe { (*output).is_null() } {
            return error(EFAULT);
        }
        unsafe {
            ptr::copy_nonoverlapping(encoded.as_ptr(), (*output).cast(), produced);
            *input = (*input).add(consumed);
            *input_left -= consumed;
            *output = (*output).add(produced);
            *output_left -= produced;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_iconv_close(descriptor: *mut c_void) -> c_int {
    if descriptor.is_null() || descriptor as usize == FAILED {
        error(EBADF);
        return -1;
    }
    let converter = unsafe { &mut *descriptor.cast::<Converter>() };
    if converter.magic != MAGIC {
        error(EBADF);
        return -1;
    }
    converter.magic = 0;
    unsafe { guest::free(descriptor.cast()) };
    0
}
