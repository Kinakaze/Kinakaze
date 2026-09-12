//! A `printf` format engine.
//!
//! The parser handles the full C99 conversion grammar:
//! `%[flags][width][.precision][length]conversion`. Output is produced through a
//! [`Sink`] so the same engine serves `printf`, `sprintf`, `snprintf` and the
//! `v*` forms, with `snprintf` truncating while still reporting the length the
//! full output would have needed.

use core::ffi::{c_char, c_double, c_int, c_void};
use core::fmt::Write as _;
mod extension;
mod legacy_float;
mod positional;
mod wide;

/// Receives formatted bytes.
///
/// Implementations count everything written even when they discard it, because
/// `snprintf` must return the untruncated length.
pub trait Sink {
    /// Appends bytes, discarding any that do not fit.
    fn write(&mut self, bytes: &[u8]);
    /// Returns the total number of bytes offered so far.
    fn written(&self) -> usize;
    /// A real FILE is passed through to registered printf converters.
    fn stream(&mut self) -> Option<*mut crate::stdio::File> {
        None
    }
    fn account(&mut self, _count: usize) {}
}

/// Writes into a fixed buffer, truncating and always terminating.
pub struct BufferSink {
    buffer: *mut u8,
    capacity: usize,
    written: usize,
}

impl BufferSink {
    /// # Safety
    ///
    /// `buffer` must be writable for `capacity` bytes, or `capacity` must be 0.
    pub unsafe fn new(buffer: *mut u8, capacity: usize) -> Self {
        Self {
            buffer,
            capacity,
            written: 0,
        }
    }

    /// Writes the terminator and returns the untruncated length.
    pub fn finish(self) -> usize {
        if self.capacity > 0 {
            // The terminator goes at the write position, or at the last slot
            // when the output was truncated.
            let position = self.written.min(self.capacity - 1);
            // SAFETY: `position` is strictly below `capacity`.
            unsafe { *self.buffer.add(position) = 0 };
        }
        self.written
    }
}

impl Sink for BufferSink {
    fn write(&mut self, bytes: &[u8]) {
        // Reserve the final byte for the terminator.
        if self.capacity > 0 && self.written < self.capacity - 1 {
            let room = self.capacity - 1 - self.written;
            let count = room.min(bytes.len());
            // SAFETY: `count` bytes fit before the reserved terminator slot.
            unsafe {
                core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.buffer.add(self.written), count)
            };
        }
        // Count everything, including bytes that were dropped.
        self.written += bytes.len();
    }

    fn written(&self) -> usize {
        self.written
    }
}

/// The System V AMD64 `va_list`, which is a register-save-area cursor rather
/// than a plain pointer.
///
/// Integer arguments occupy `gp_offset` 0..48 in the register save area and
/// spill to the overflow area afterwards; floating-point arguments use
/// `fp_offset` 48..176. Reproducing this layout is what allows the engine to
/// consume real C varargs.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VaList {
    gp_offset: u32,
    fp_offset: u32,
    overflow_arg_area: *mut c_void,
    reg_save_area: *mut c_void,
}

impl VaList {
    /// Reads the next integer-class argument.
    ///
    /// # Safety
    ///
    /// The caller must guarantee another integer-class argument is present and
    /// that `T` matches its size.
    pub(crate) unsafe fn next_integer<T: Copy>(&mut self) -> T {
        // Six integer registers are saved, each 8 bytes.
        if self.gp_offset < 48 {
            // SAFETY: the offset is inside the register save area.
            let value = unsafe { self.reg_save_area.byte_add(self.gp_offset as usize) };
            self.gp_offset += 8;
            // SAFETY: the slot holds the argument.
            unsafe { value.cast::<T>().read_unaligned() }
        } else {
            // SAFETY: the overflow area holds the remaining arguments.
            let value = unsafe { self.overflow_arg_area.cast::<T>().read_unaligned() };
            // Stack arguments are always 8-byte aligned.
            self.overflow_arg_area = unsafe { self.overflow_arg_area.byte_add(8) };
            value
        }
    }

    /// Reads the next floating-point argument.
    ///
    /// # Safety
    ///
    /// The caller must guarantee another floating-point argument is present.
    unsafe fn next_double(&mut self) -> c_double {
        // Eight SSE registers follow the integer ones, each occupying 16 bytes.
        if self.fp_offset < 176 {
            // SAFETY: the offset is inside the register save area.
            let value = unsafe { self.reg_save_area.byte_add(self.fp_offset as usize) };
            self.fp_offset += 16;
            // SAFETY: the slot holds the argument.
            unsafe { value.cast::<c_double>().read_unaligned() }
        } else {
            // SAFETY: the overflow area holds the remaining arguments.
            let value = unsafe { self.overflow_arg_area.cast::<c_double>().read_unaligned() };
            self.overflow_arg_area = unsafe { self.overflow_arg_area.byte_add(8) };
            value
        }
    }
}

/// Argument width selected by a length modifier.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Length {
    Char,
    Short,
    Int,
    Long,
    LongLong,
    Size,
    IntMax,
    PtrDiff,
    LongDouble,
}

/// One parsed conversion specification.
#[derive(Clone, Copy)]
struct Spec {
    argument: Option<usize>,
    left_align: bool,
    plus: bool,
    space: bool,
    alternate: bool,
    zero_pad: bool,
    width: Option<usize>,
    precision: Option<usize>,
    length: Length,
    conversion: u8,
    group: bool,
    i18n: bool,
    user: u16,
    width_dynamic: bool,
    precision_dynamic: bool,
}

impl Spec {
    /// Emits `body` with sign, padding and prefix applied.
    fn pad<S: Sink>(&self, sink: &mut S, sign: Option<u8>, prefix: &[u8], body: &[u8]) {
        let content = sign.map_or(0, |_| 1) + prefix.len() + body.len();
        let padding = self.width.unwrap_or(0).saturating_sub(content);

        // Right-aligned space padding precedes the sign; zero padding follows it
        // so `%+05d` produces `+0042` rather than `000+42`.
        if !self.left_align && !self.zero_pad {
            write_repeated(sink, b' ', padding);
        }
        if let Some(sign) = sign {
            sink.write(&[sign]);
        }
        sink.write(prefix);
        if !self.left_align && self.zero_pad {
            write_repeated(sink, b'0', padding);
        }
        sink.write(body);
        if self.left_align {
            write_repeated(sink, b' ', padding);
        }
    }
}

fn write_repeated<S: Sink>(sink: &mut S, byte: u8, count: usize) {
    // A small stack chunk avoids one sink call per byte for wide fields.
    const CHUNK: [u8; 32] = [0; 32];
    let mut chunk = CHUNK;
    chunk.fill(byte);
    let mut remaining = count;
    while remaining > 0 {
        let step = remaining.min(chunk.len());
        sink.write(&chunk[..step]);
        remaining -= step;
    }
}

/// Formats `format` against `arguments`, writing into `sink`.
///
/// # Safety
///
/// `format` must be a null-terminated string whose conversions match the types
/// actually present in `arguments`.
pub unsafe fn format<S: Sink>(
    sink: &mut S,
    format: *const c_char,
    arguments: &mut VaList,
) -> c_int {
    // The common sequential path allocates nothing. Translated strings opt in
    // to the two-pass argument index only when they contain a dollar sign.
    if unsafe { core::ffi::CStr::from_ptr(format) }
        .to_bytes()
        .contains(&b'$')
    {
        match unsafe { positional::format(sink, format, arguments) } {
            Ok(Some(count)) => return count,
            Ok(None) => {}
            Err(error) => {
                crate::set_errno(error);
                return -1;
            }
        }
    }
    let mut cursor = 0usize;
    loop {
        // SAFETY: the caller guarantees a null-terminated format string.
        let byte = unsafe { *format.add(cursor) } as u8;
        if byte == 0 {
            break;
        }
        cursor += 1;
        if byte != b'%' {
            let start = cursor - 1;
            while unsafe { *format.add(cursor) } != 0
                && unsafe { *format.add(cursor) } as u8 != b'%'
            {
                cursor += 1;
            }
            sink.write(unsafe {
                core::slice::from_raw_parts(format.add(start).cast(), cursor - start)
            });
            continue;
        }

        // SAFETY: the string is null-terminated, so this read is in bounds.
        if unsafe { *format.add(cursor) } as u8 == b'%' {
            cursor += 1;
            sink.write(b"%");
            continue;
        }

        // SAFETY: parsing stops at the terminator.
        let Some(spec) =
            (unsafe { parse_spec(format, &mut cursor, |_| arguments.next_integer::<c_int>()) })
        else {
            // An unterminated specification is emitted literally, which is
            // friendlier than dropping it and matches common libc behaviour.
            sink.write(b"%");
            continue;
        };
        // SAFETY: the caller guarantees the arguments match the conversions.
        match unsafe { extension::emit(sink, &spec, arguments) } {
            Ok(true) => {}
            Ok(false) => unsafe { emit(sink, &spec, arguments) },
            Err(error) => {
                crate::set_errno(error);
                return -1;
            }
        }
    }
    sink.written() as c_int
}

/// Parses one specification, advancing `cursor` past it.
///
/// `*` width and precision consume an argument here, in the order C requires.
///
/// # Safety
///
/// `format` must be null-terminated.
unsafe fn parse_spec(
    format: *const c_char,
    cursor: &mut usize,
    mut read_int: impl FnMut(Option<usize>) -> c_int,
) -> Option<Spec> {
    let mut spec = Spec {
        argument: unsafe { positional::index(format, cursor) },
        left_align: false,
        plus: false,
        space: false,
        alternate: false,
        zero_pad: false,
        width: None,
        precision: None,
        length: Length::Int,
        conversion: 0,
        group: false,
        i18n: false,
        user: 0,
        width_dynamic: false,
        precision_dynamic: false,
    };

    // SAFETY: reads stay within the null-terminated string.
    let peek = |offset: usize| unsafe { *format.add(offset) } as u8;

    loop {
        match peek(*cursor) {
            b'-' => spec.left_align = true,
            b'+' => spec.plus = true,
            b' ' => spec.space = true,
            b'#' => spec.alternate = true,
            b'0' => spec.zero_pad = true,
            b'\'' => spec.group = true,
            b'I' => spec.i18n = true,
            _ => break,
        }
        *cursor += 1;
    }

    if peek(*cursor) == b'*' {
        *cursor += 1;
        // SAFETY: the caller guarantees an int argument for `*`.
        spec.width_dynamic = true;
        let width = read_int(unsafe { positional::index(format, cursor) });
        // A negative `*` width means left alignment with the absolute value.
        if width < 0 {
            spec.left_align = true;
            spec.width = Some(width.unsigned_abs() as usize);
        } else {
            spec.width = Some(width as usize);
        }
    } else {
        let mut width = 0usize;
        let mut saw_digit = false;
        while peek(*cursor).is_ascii_digit() {
            width = width * 10 + (peek(*cursor) - b'0') as usize;
            saw_digit = true;
            *cursor += 1;
        }
        if saw_digit {
            spec.width = Some(width);
        }
    }

    if peek(*cursor) == b'.' {
        *cursor += 1;
        if peek(*cursor) == b'*' {
            *cursor += 1;
            // SAFETY: the caller guarantees an int argument for `.*`.
            spec.precision_dynamic = true;
            let precision = read_int(unsafe { positional::index(format, cursor) });
            // A negative precision is treated as if omitted.
            spec.precision = (precision >= 0).then_some(precision as usize);
        } else {
            let mut precision = 0usize;
            while peek(*cursor).is_ascii_digit() {
                precision = precision * 10 + (peek(*cursor) - b'0') as usize;
                *cursor += 1;
            }
            // `%.d` means precision zero.
            spec.precision = Some(precision);
        }
    }

    let (modifier_bytes, user) = unsafe { extension::modifier(format.add(*cursor)) };
    *cursor += modifier_bytes;
    spec.user = user;
    spec.length = if modifier_bytes != 0 {
        Length::Int
    } else {
        match peek(*cursor) {
            b'h' => {
                *cursor += 1;
                if peek(*cursor) == b'h' {
                    *cursor += 1;
                    Length::Char
                } else {
                    Length::Short
                }
            }
            b'l' => {
                *cursor += 1;
                if peek(*cursor) == b'l' {
                    *cursor += 1;
                    Length::LongLong
                } else {
                    Length::Long
                }
            }
            b'z' => {
                *cursor += 1;
                Length::Size
            }
            b'j' => {
                *cursor += 1;
                Length::IntMax
            }
            b't' => {
                *cursor += 1;
                Length::PtrDiff
            }
            b'L' => {
                *cursor += 1;
                Length::LongDouble
            }
            _ => Length::Int,
        }
    };

    let conversion = peek(*cursor);
    if conversion == 0 {
        return None;
    }
    *cursor += 1;
    spec.conversion = conversion;
    Some(spec)
}

/// Writes one conversion's output.
///
/// # Safety
///
/// The next argument must match `spec.conversion` and `spec.length`.
unsafe fn emit<S: Sink>(sink: &mut S, spec: &Spec, arguments: &mut VaList) {
    match spec.conversion {
        b'd' | b'i' => {
            // Narrow types are promoted to int by the caller, so read an int and
            // truncate to the declared width.
            // SAFETY: the caller guarantees an integer argument.
            let value = unsafe {
                match spec.length {
                    Length::Char => arguments.next_integer::<c_int>() as i8 as i64,
                    Length::Short => arguments.next_integer::<c_int>() as i16 as i64,
                    Length::Int => arguments.next_integer::<c_int>() as i64,
                    _ => arguments.next_integer::<i64>(),
                }
            };
            emit_signed(sink, spec, value);
        }
        b'u' | b'o' | b'x' | b'X' => {
            // SAFETY: the caller guarantees an integer argument.
            let value = unsafe {
                match spec.length {
                    Length::Char => arguments.next_integer::<c_int>() as u8 as u64,
                    Length::Short => arguments.next_integer::<c_int>() as u16 as u64,
                    Length::Int => arguments.next_integer::<c_int>() as u32 as u64,
                    _ => arguments.next_integer::<u64>(),
                }
            };
            emit_unsigned(sink, spec, value);
        }
        b'c' => {
            // SAFETY: the caller guarantees an int argument.
            let value = unsafe { arguments.next_integer::<c_int>() };
            if spec.length == Length::Long {
                let (bytes, length) = encode_wchar(value);
                spec.pad(sink, None, b"", &bytes[..length]);
            } else {
                spec.pad(sink, None, b"", &[value as u8]);
            }
        }
        b's' => {
            if spec.length == Length::Long {
                // `%ls` receives a Linux wchar_t string (UTF-32), not a byte
                // string. Treating it as `char *` prints only its first ASCII
                // character because each code point contains three zero bytes.
                let text = unsafe { arguments.next_integer::<*const i32>() };
                emit_wide_string(sink, spec, text);
            } else {
                // SAFETY: the caller guarantees a string pointer.
                let text = unsafe { arguments.next_integer::<*const c_char>() };
                emit_string(sink, spec, text);
            }
        }
        b'p' => {
            // SAFETY: the caller guarantees a pointer argument.
            let value = unsafe { arguments.next_integer::<usize>() };
            if value == 0 {
                // glibc prints "(nil)" for a null pointer.
                spec.pad(sink, None, b"", b"(nil)");
            } else {
                let mut digits = [0u8; 16];
                let length = write_radix(&mut digits, value as u64, 16, false);
                spec.pad(sink, None, b"0x", &digits[digits.len() - length..]);
            }
        }
        b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
            // SAFETY: the caller guarantees a double argument.
            let value = unsafe { arguments.next_double() };
            emit_float(sink, spec, value);
        }
        b'n' => {
            // SAFETY: the caller guarantees a writable int pointer.
            let out = unsafe { arguments.next_integer::<*mut c_int>() };
            if !out.is_null() {
                // SAFETY: the caller guarantees the pointer is writable.
                unsafe { *out = sink.written() as c_int };
            }
        }
        other => {
            // An unknown conversion is echoed so the output shows the mistake
            // rather than silently losing it.
            sink.write(b"%");
            sink.write(&[other]);
        }
    }
}

/// Renders `value` into `digits` right-aligned, returning the digit count.
fn write_radix(digits: &mut [u8], mut value: u64, radix: u64, upper: bool) -> usize {
    let alphabet: &[u8] = if upper {
        b"0123456789ABCDEF"
    } else {
        b"0123456789abcdef"
    };
    if value == 0 {
        let last = digits.len() - 1;
        digits[last] = b'0';
        return 1;
    }
    let mut position = digits.len();
    while value > 0 {
        position -= 1;
        digits[position] = alphabet[(value % radix) as usize];
        value /= radix;
    }
    digits.len() - position
}

fn emit_signed<S: Sink>(sink: &mut S, spec: &Spec, value: i64) {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let mut digits = [0u8; 20];
    let length = write_radix(&mut digits, magnitude, 10, false);
    let body = &digits[digits.len() - length..];

    let sign = if negative {
        Some(b'-')
    } else if spec.plus {
        Some(b'+')
    } else if spec.space {
        Some(b' ')
    } else {
        None
    };

    // An explicit precision is a minimum digit count and overrides zero padding.
    if let Some(precision) = spec.precision {
        let zeros = precision.saturating_sub(body.len());
        let mut padded = [0u8; 40];
        let total = zeros + body.len();
        if total <= padded.len() {
            padded[..zeros].fill(b'0');
            padded[zeros..total].copy_from_slice(body);
            let spec = Spec {
                zero_pad: false,
                ..copy_spec(spec)
            };
            spec.pad(sink, sign, b"", &padded[..total]);
            return;
        }
    }
    spec.pad(sink, sign, b"", body);
}

fn emit_unsigned<S: Sink>(sink: &mut S, spec: &Spec, value: u64) {
    let (radix, upper, prefix): (u64, bool, &[u8]) = match spec.conversion {
        b'o' => (8, false, if spec.alternate { b"0" } else { b"" }),
        b'x' => (
            16,
            false,
            if spec.alternate && value != 0 {
                b"0x"
            } else {
                b""
            },
        ),
        b'X' => (
            16,
            true,
            if spec.alternate && value != 0 {
                b"0X"
            } else {
                b""
            },
        ),
        _ => (10, false, b""),
    };
    let mut digits = [0u8; 22];
    let length = write_radix(&mut digits, value, radix, upper);
    let body = &digits[digits.len() - length..];

    if let Some(precision) = spec.precision {
        let zeros = precision.saturating_sub(body.len());
        let mut padded = [0u8; 44];
        let total = zeros + body.len();
        if total <= padded.len() {
            padded[..zeros].fill(b'0');
            padded[zeros..total].copy_from_slice(body);
            let spec = Spec {
                zero_pad: false,
                ..copy_spec(spec)
            };
            spec.pad(sink, None, prefix, &padded[..total]);
            return;
        }
    }
    spec.pad(sink, None, prefix, body);
}

fn copy_spec(spec: &Spec) -> Spec {
    *spec
}

fn emit_string<S: Sink>(sink: &mut S, spec: &Spec, text: *const c_char) {
    if text.is_null() {
        // Printing "(null)" matches glibc and avoids dereferencing null.
        spec.pad(sink, None, b"", b"(null)");
        return;
    }
    // A precision caps the number of bytes taken from the string.
    let mut length = 0;
    let limit = spec.precision.unwrap_or(usize::MAX);
    // SAFETY: the caller guarantees a null-terminated string.
    while length < limit && unsafe { *text.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` bytes were just verified to be present.
    let bytes = unsafe { core::slice::from_raw_parts(text.cast::<u8>(), length) };
    // Zero padding is not defined for strings.
    let spec = Spec {
        zero_pad: false,
        ..copy_spec(spec)
    };
    spec.pad(sink, None, b"", bytes);
}

/// Encodes one Linux `wchar_t` as UTF-8.
///
/// Invalid scalar values are replaced rather than read past the input. Locale
/// error reporting can be added when the formatter itself has a fallible return
/// path; valid UTF-32, which is what CPython passes, is preserved exactly.
fn encode_wchar(value: i32) -> ([u8; 4], usize) {
    let character = char::from_u32(value as u32).unwrap_or(char::REPLACEMENT_CHARACTER);
    let mut bytes = [0u8; 4];
    let length = character.encode_utf8(&mut bytes).len();
    (bytes, length)
}

fn emit_wide_string<S: Sink>(sink: &mut S, spec: &Spec, text: *const i32) {
    if text.is_null() {
        spec.pad(sink, None, b"", b"(null)");
        return;
    }

    // For `%ls`, C defines precision as a maximum number of output bytes and
    // forbids emitting a partial multibyte character.
    let limit = spec.precision.unwrap_or(usize::MAX);
    let mut characters = 0usize;
    let mut byte_length = 0usize;
    loop {
        let value = unsafe { *text.add(characters) };
        if value == 0 {
            break;
        }
        let (_, length) = encode_wchar(value);
        if byte_length.saturating_add(length) > limit {
            break;
        }
        byte_length += length;
        characters += 1;
    }

    let padding = spec.width.unwrap_or(0).saturating_sub(byte_length);
    if !spec.left_align {
        write_repeated(sink, b' ', padding);
    }
    for index in 0..characters {
        let (bytes, length) = encode_wchar(unsafe { *text.add(index) });
        sink.write(&bytes[..length]);
    }
    if spec.left_align {
        write_repeated(sink, b' ', padding);
    }
}

fn emit_float<S: Sink>(sink: &mut S, spec: &Spec, value: c_double) {
    // Non-finite values ignore precision and zero padding entirely.
    if value.is_nan() || value.is_infinite() {
        let upper = spec.conversion.is_ascii_uppercase();
        let body: &[u8] = match (value.is_nan(), upper) {
            (true, false) => b"nan",
            (true, true) => b"NAN",
            (false, false) => b"inf",
            (false, true) => b"INF",
        };
        let sign = if value.is_sign_negative() {
            Some(b'-')
        } else if spec.plus {
            Some(b'+')
        } else if spec.space {
            Some(b' ')
        } else {
            None
        };
        let spec = Spec {
            zero_pad: false,
            ..copy_spec(spec)
        };
        spec.pad(sink, sign, b"", body);
        return;
    }

    let precision = spec.precision.unwrap_or(6);
    let magnitude = value.abs();
    let sign = if value.is_sign_negative() {
        Some(b'-')
    } else if spec.plus {
        Some(b'+')
    } else if spec.space {
        Some(b' ')
    } else {
        None
    };

    // Rust's float formatting is correctly rounded, so the digits themselves are
    // delegated and only C's field mechanics are applied here.
    let mut rendered = FloatBuffer::new();
    match spec.conversion {
        b'e' | b'E' => write_exponential(&mut rendered, magnitude, precision, spec.conversion),
        b'g' | b'G' => write_general(&mut rendered, magnitude, precision, spec.conversion),
        // %a is rare; hexadecimal float output falls back to %e rather than
        // silently producing a wrong hexadecimal form.
        b'a' | b'A' => write_exponential(&mut rendered, magnitude, precision, b'e'),
        _ => {
            let _ = write!(rendered, "{magnitude:.precision$}");
        }
    }

    let mut spec_copy = copy_spec(spec);
    // Zero padding for floats pads between the sign and the digits, which
    // `Spec::pad` already does, so only the sign handling changes.
    spec_copy.precision = None;
    spec_copy.pad(sink, sign, b"", rendered.as_bytes());
}

/// A small stack buffer that implements [`core::fmt::Write`].
///
/// 512 bytes covers every finite `f64` in `%f` with the default precision, and
/// longer output is truncated rather than heap-allocated inside a formatter.
struct FloatBuffer {
    bytes: [u8; 512],
    length: usize,
}

impl FloatBuffer {
    fn new() -> Self {
        Self {
            bytes: [0; 512],
            length: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

impl core::fmt::Write for FloatBuffer {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.bytes.len() - self.length;
        let count = room.min(text.len());
        self.bytes[self.length..self.length + count].copy_from_slice(&text.as_bytes()[..count]);
        self.length += count;
        Ok(())
    }
}

/// Writes `%e` form: one digit, the precision, then a signed two-digit exponent.
fn write_exponential(out: &mut FloatBuffer, value: f64, precision: usize, conversion: u8) {
    let mut exponent = 0i32;
    let mut mantissa = value;
    if mantissa != 0.0 {
        // Normalize into [1, 10) by decimal exponent.
        while mantissa >= 10.0 {
            mantissa /= 10.0;
            exponent += 1;
        }
        while mantissa < 1.0 {
            mantissa *= 10.0;
            exponent -= 1;
        }
        // Rounding the mantissa can carry it back to 10.0.
        let rounded = format_args!("{mantissa:.precision$}").to_string();
        if rounded.starts_with("10") {
            mantissa /= 10.0;
            exponent += 1;
        }
    }
    let marker = if conversion.is_ascii_uppercase() {
        'E'
    } else {
        'e'
    };
    let sign = if exponent < 0 { '-' } else { '+' };
    // C requires at least two exponent digits.
    let _ = write!(
        out,
        "{mantissa:.precision$}{marker}{sign}{:02}",
        exponent.abs()
    );
}

/// Writes `%g` form: `%e` or `%f` by exponent, with trailing zeros removed.
fn write_general(out: &mut FloatBuffer, value: f64, precision: usize, conversion: u8) {
    // A precision of zero is treated as one significant digit.
    let significant = precision.max(1);
    let exponent = if value == 0.0 {
        0
    } else {
        value.abs().log10().floor() as i32
    };

    // C selects %e when the exponent is below -4 or at least the precision.
    let mut scratch = FloatBuffer::new();
    if exponent < -4 || exponent >= significant as i32 {
        write_exponential(&mut scratch, value, significant - 1, conversion);
    } else {
        let decimals = (significant as i32 - 1 - exponent).max(0) as usize;
        let _ = write!(scratch, "{value:.decimals$}");
    }

    // %g strips trailing zeros in the fraction, and a bare trailing point.
    let text = core::str::from_utf8(scratch.as_bytes()).unwrap_or("");
    let trimmed = if text.contains('.') {
        let (mantissa, exponent_part) = match text.find(['e', 'E']) {
            Some(index) => (&text[..index], &text[index..]),
            None => (text, ""),
        };
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        let _ = write!(out, "{mantissa}{exponent_part}");
        return;
    } else {
        text
    };
    let _ = write!(out, "{trimmed}");
}

/// GNU argument introspection shares the conversion parser and hook arginfo.
pub(crate) unsafe fn parse_types(
    format: *const c_char,
    capacity: usize,
    output: *mut i32,
) -> usize {
    if format.is_null() || (output.is_null() && capacity != 0) {
        crate::set_errno(22);
        return 0;
    }
    if unsafe { core::ffi::CStr::from_ptr(format) }
        .to_bytes()
        .contains(&b'$')
    {
        match unsafe { positional::types(format) } {
            Ok(Some(types)) => {
                for (i, &kind) in types.iter().take(capacity).enumerate() {
                    unsafe {
                        output.add(i).write(kind);
                    }
                }
                return types.len();
            }
            Ok(None) => {}
            Err(error) => {
                crate::set_errno(error);
                return 0;
            }
        }
    }
    let mut count = 0usize;
    let mut cursor = 0usize;
    let mut record = |kind: i32| {
        if count < capacity {
            unsafe { output.add(count).write(kind) };
        }
        count += 1;
    };
    loop {
        let c = unsafe { *format.add(cursor) } as u8;
        if c == 0 {
            break;
        }
        cursor += 1;
        if c != b'%' {
            continue;
        }
        if unsafe { *format.add(cursor) } as u8 == b'%' {
            cursor += 1;
            continue;
        }
        let Some(spec) = (unsafe {
            parse_spec(format, &mut cursor, |_| {
                record(0);
                0
            })
        }) else {
            break;
        };
        match unsafe { extension::argument_types(&spec) } {
            Err(error) => {
                crate::set_errno(error);
                return 0;
            }
            Ok(Some(types)) => {
                for i in 0..types.count {
                    record(types.kind(i));
                }
                continue;
            }
            Ok(None) => {}
        }
        if let Some(kind) = builtin_kind(&spec) {
            record(kind);
        }
    }
    count
}

fn builtin_kind(spec: &Spec) -> Option<i32> {
    let integer = match spec.length {
        Length::Char => 1,
        Length::Short => 0x400,
        Length::Int => 0,
        Length::LongLong | Length::IntMax => 0x100,
        _ => 0x200,
    };
    let kind = match spec.conversion {
        b'd' | b'i' | b'u' | b'o' | b'x' | b'X' => integer,
        b'c' => {
            if spec.length == Length::Long {
                2
            } else {
                1
            }
        }
        b's' => {
            if spec.length == Length::Long {
                4
            } else {
                3
            }
        }
        b'p' => 5,
        b'n' => 0x800,
        b'e' | b'E' | b'f' | b'F' | b'g' | b'G' | b'a' | b'A' => {
            if spec.length == Length::LongDouble {
                7 | 0x100
            } else {
                7
            }
        }
        _ => return None,
    };
    Some(kind)
}
