//! POSIX numbered conversions retain the real AMD64 integer/SSE/stack classes.
use super::*;
const EINVAL: i32 = 22;

pub(super) unsafe fn index(format: *const c_char, cursor: &mut usize) -> Option<usize> {
    let mut end = *cursor;
    let mut number = 0usize;
    while (unsafe { *format.add(end) } as u8).is_ascii_digit() {
        number = number
            .saturating_mul(10)
            .saturating_add((unsafe { *format.add(end) } as u8 - b'0') as usize);
        end += 1;
    }
    if end == *cursor || unsafe { *format.add(end) } as u8 != b'$' {
        return None;
    }
    *cursor = end + 1;
    Some(number)
}

pub(super) unsafe fn types(format: *const c_char) -> Result<Option<Vec<i32>>, i32> {
    let mut types = Vec::new();
    let mut numbered = false;
    let mut sequential = false;
    let mut error = false;
    let mut record = |index: Option<usize>, kind: i32| {
        let Some(index) = index else {
            sequential = true;
            return;
        };
        numbered = true;
        // Bound adversarial formats before allocating; POSIX guarantees at least 9.
        if index == 0 || index > 4096 {
            error = true;
            return;
        }
        if types.len() < index {
            types.resize(index, -1);
        }
        let previous = types[index - 1];
        if previous != -1 && previous != kind {
            error = true;
        }
        types[index - 1] = kind;
    };
    let mut cursor = 0;
    while unsafe { *format.add(cursor) } != 0 {
        let byte = unsafe { *format.add(cursor) } as u8;
        cursor += 1;
        if byte != b'%' {
            continue;
        }
        if unsafe { *format.add(cursor) } as u8 == b'%' {
            cursor += 1;
            continue;
        }
        let Some(spec) = (unsafe {
            parse_spec(format, &mut cursor, |index| {
                record(index, 0);
                0
            })
        }) else {
            break;
        };
        if let Some(custom) = unsafe { extension::argument_types(&spec)? } {
            for n in 0..custom.count {
                record(spec.argument.and_then(|i| i.checked_add(n)), custom.kind(n));
            }
        } else if let Some(kind) = builtin_kind(&spec) {
            record(spec.argument, kind);
        }
    }
    if !numbered {
        return Ok(None);
    }
    if error || sequential || types.contains(&-1) {
        return Err(EINVAL);
    }
    Ok(Some(types))
}

pub(super) unsafe fn format<S: Sink>(
    sink: &mut S,
    format: *const c_char,
    arguments: &mut VaList,
) -> Result<Option<c_int>, i32> {
    let Some(types) = (unsafe { types(format)? }) else {
        return Ok(None);
    };
    let mut locations = Vec::with_capacity(types.len());
    let mut scan = *arguments;
    for kind in types {
        locations.push(scan);
        if kind & 0x800 != 0 || matches!(kind & 0xff, 0..=5) {
            let _ = unsafe { scan.next_integer::<u64>() };
        } else if matches!(kind & 0xff, 6 | 7) && kind & 0x100 == 0 {
            let _ = unsafe { scan.next_double() };
        } else {
            // A registered aggregate reader needs its own ABI description.
            return Err(EINVAL);
        }
    }
    let mut cursor = 0;
    loop {
        let start = cursor;
        while unsafe { *format.add(cursor) } != 0 && unsafe { *format.add(cursor) } as u8 != b'%' {
            cursor += 1;
        }
        sink.write(unsafe {
            core::slice::from_raw_parts(format.add(start).cast(), cursor - start)
        });
        if unsafe { *format.add(cursor) } == 0 {
            break;
        }
        cursor += 1;
        if unsafe { *format.add(cursor) } as u8 == b'%' {
            sink.write(b"%");
            cursor += 1;
            continue;
        }
        let Some(spec) = (unsafe {
            parse_spec(format, &mut cursor, |index| {
                let mut value = locations[index.expect("validated positional width") - 1];
                value.next_integer::<c_int>()
            })
        }) else {
            sink.write(b"%");
            break;
        };
        let mut value = spec.argument.map_or(*arguments, |i| locations[i - 1]);
        if !unsafe { extension::emit(sink, &spec, &mut value)? } {
            unsafe {
                emit(sink, &spec, &mut value);
            }
        }
    }
    *arguments = scan;
    Ok(Some(sink.written() as c_int))
}
