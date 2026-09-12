//! Wide formatting shares the numeric/parser engine but counts Linux wchar_t
//! units for widths, precision, %n and capacity. Common formats use the stack.
use super::*;

struct WideBuffer {
    output: *mut i32,
    capacity: usize,
    count: usize,
}
impl WideBuffer {
    fn push(&mut self, value: i32) {
        if self.count < self.capacity.saturating_sub(1) {
            unsafe {
                *self.output.add(self.count) = value;
            }
        }
        self.count = self.count.saturating_add(1);
    }
    fn spaces(&mut self, count: usize) {
        let fit = count.min(self.capacity.saturating_sub(1).saturating_sub(self.count));
        for index in 0..fit {
            unsafe {
                *self.output.add(self.count + index) = 32;
            }
        }
        self.count = self.count.saturating_add(count);
    }
    fn finish(&self) {
        if self.capacity != 0 {
            unsafe {
                *self.output.add(self.count.min(self.capacity - 1)) = 0;
            }
        }
    }
}
// Built-in numeric conversions emit ASCII. Strings and wchar_t conversions
// take a separate path below; UTF-8 bytes must never be counted as wide units.
impl Sink for WideBuffer {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.push(byte as i32);
        }
    }
    fn written(&self) -> usize {
        self.count
    }
}

unsafe fn multibyte(
    input: *const c_char,
    limit: usize,
    mut emit: impl FnMut(i32),
) -> Result<usize, i32> {
    let mut state = crate::uchar::MbState::default();
    let mut at = input;
    let mut count = 0;
    while count < limit {
        // Read incrementally so precision-limited and page-boundary strings
        // never require touching bytes after their last complete character.
        let mut scalar = 0;
        loop {
            let consumed = unsafe { crate::uchar::mbrtowc(&mut scalar, at, 1, &mut state) };
            if consumed == usize::MAX {
                return Err(84);
            }
            if consumed == 0 {
                return Ok(count);
            }
            at = unsafe { at.add(1) };
            if consumed != usize::MAX - 1 {
                break;
            }
        }
        emit(scalar);
        count += 1;
    }
    Ok(count)
}

unsafe fn string(sink: &mut WideBuffer, spec: &Spec, arguments: &mut VaList) -> Result<(), i32> {
    let input = unsafe { arguments.next_integer::<*const c_void>() };
    let limit = spec.precision.unwrap_or(usize::MAX);
    let wide = spec.length == Length::Long;
    let null = [40, 110, 117, 108, 108, 41, 0];
    let is_wide = wide || input.is_null();
    let text = if input.is_null() {
        null.as_ptr()
    } else {
        input.cast::<i32>()
    };
    let count = if is_wide {
        let mut count = 0;
        while count < limit && unsafe { *text.add(count) } != 0 {
            count += 1;
        }
        count
    } else {
        unsafe { multibyte(input.cast(), limit, |_| {})? }
    };
    let padding = spec.width.unwrap_or(0).saturating_sub(count);
    if !spec.left_align {
        sink.spaces(padding);
    }
    if is_wide {
        for index in 0..count {
            sink.push(unsafe { *text.add(index) });
        }
    } else {
        unsafe { multibyte(input.cast(), count, |value| sink.push(value))? };
    }
    if spec.left_align {
        sink.spaces(padding);
    }
    Ok(())
}

unsafe fn render(
    sink: &mut WideBuffer,
    wide: *const i32,
    arguments: &mut VaList,
) -> Result<(), i32> {
    // One ASCII grammar byte per input wchar_t preserves cursor offsets. Wide
    // literals are emitted from the original input, without a UTF-8 round trip.
    let mut small = [0u8; 256];
    let mut large = Vec::new();
    let mut length = 0;
    loop {
        let value = unsafe { *wide.add(length) };
        let byte = if (0..128).contains(&value) {
            value as u8
        } else {
            b'?'
        };
        if length < small.len() {
            small[length] = byte;
        } else {
            if large.is_empty() {
                large.extend_from_slice(&small);
            }
            large.push(byte);
        }
        if value == 0 {
            break;
        }
        length += 1;
    }
    let ascii = if large.is_empty() {
        small.as_ptr()
    } else {
        large.as_ptr()
    }
    .cast();
    let mut locations = Vec::new();
    if let Some(types) = unsafe { positional::types(ascii)? } {
        let mut scan = *arguments;
        for kind in types {
            locations.push(scan);
            if kind & 0x800 != 0 || matches!(kind & 0xff, 0..=5) {
                unsafe {
                    scan.next_integer::<u64>();
                }
            } else if matches!(kind & 0xff, 6 | 7) && kind & 0x100 == 0 {
                unsafe {
                    scan.next_double();
                }
            } else {
                return Err(22);
            }
        }
        *arguments = scan;
    }
    let mut cursor = 0;
    while cursor < length {
        let value = unsafe { *wide.add(cursor) };
        cursor += 1;
        if value != 37 {
            sink.push(value);
            continue;
        }
        if unsafe { *wide.add(cursor) } == 37 {
            sink.push(37);
            cursor += 1;
            continue;
        }
        let spec = unsafe {
            parse_spec(ascii, &mut cursor, |index| {
                if let Some(index) = index {
                    let mut copy = locations[index - 1];
                    copy.next_integer::<c_int>()
                } else {
                    arguments.next_integer::<c_int>()
                }
            })
        }
        .ok_or(22)?;
        let mut copy;
        let value = if let Some(index) = spec.argument {
            copy = *index
                .checked_sub(1)
                .and_then(|i| locations.get(i))
                .ok_or(22)?;
            &mut copy
        } else {
            &mut *arguments
        };
        match spec.conversion {
            b's' => unsafe { string(sink, &spec, value)? },
            b'c' => {
                let scalar = unsafe { value.next_integer::<i32>() };
                let scalar = if spec.length == Length::Long {
                    scalar
                } else {
                    let byte = scalar as u8;
                    if byte >= 128 {
                        return Err(84);
                    }
                    byte as i32
                };
                let pad = spec.width.unwrap_or(0).saturating_sub(1);
                if !spec.left_align {
                    sink.spaces(pad);
                }
                sink.push(scalar);
                if spec.left_align {
                    sink.spaces(pad);
                }
            }
            b'n' => {
                let output = unsafe { value.next_integer::<*mut c_void>() };
                unsafe {
                    match spec.length {
                        Length::Char => output.cast::<i8>().write(sink.count as i8),
                        Length::Short => output.cast::<i16>().write(sink.count as i16),
                        Length::Int => output.cast::<i32>().write(sink.count as i32),
                        _ => output.cast::<i64>().write(sink.count as i64),
                    }
                }
            }
            // Do not consume an SSE double for the different long-double ABI.
            _ if spec.length == Length::LongDouble || spec.user != 0 => return Err(22),
            _ => unsafe {
                emit(sink, &spec, value);
            },
        }
        if sink.count >= sink.capacity {
            return Err(75);
        }
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___vswprintf_chk(
    output: *mut i32,
    capacity: usize,
    _flag: c_int,
    object_size: usize,
    format: *const i32,
    arguments: *mut VaList,
) -> c_int {
    if capacity > object_size {
        crate::startup::kinakaze_abi___chk_fail();
    }
    unsafe { kinakaze_abi_vswprintf(output, capacity, format, arguments) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_vswprintf(
    output: *mut i32,
    capacity: usize,
    format: *const i32,
    arguments: *mut VaList,
) -> c_int {
    if format.is_null() || arguments.is_null() || (capacity != 0 && output.is_null()) {
        crate::set_errno(22);
        return -1;
    }
    if capacity == 0 {
        return -1;
    }
    let mut sink = WideBuffer {
        output,
        capacity,
        count: 0,
    };
    let result = unsafe { render(&mut sink, format, &mut *arguments) };
    sink.finish();
    if let Err(error) = result {
        crate::set_errno(error);
        return -1;
    }
    if sink.count >= capacity || sink.count > i32::MAX as usize {
        crate::set_errno(75);
        return -1;
    }
    sink.count as i32
}
