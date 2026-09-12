//! Incremental MIDI bytes to Linux sequencer events, including bounded SysEx.
use super::*;

#[repr(C)]
pub struct Encoder {
    buffer: *mut u8,
    capacity: usize,
    length: usize,
    bytes: [u8; 3],
    used: u8,
    sysex: bool,
    last_decoded: u8,
    no_status: bool,
}

/// A short-message decoder has no payload storage or running status to retain.
/// Keep it on the stack for native output instead of allocating per MIDI event.
pub(super) unsafe fn decode_short(event: *const u8, output: &mut [u8; 12]) -> i64 {
    let mut encoder = Encoder {
        buffer: core::ptr::null_mut(),
        capacity: 0,
        length: 0,
        bytes: [0; 3],
        used: 0,
        sysex: false,
        last_decoded: 0xff,
        no_status: true,
    };
    unsafe { snd_midi_event_decode(&raw mut encoder, output.as_mut_ptr(), 12, event) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_new")]
pub unsafe extern "sysv64" fn snd_midi_event_new(size: usize, output: *mut *mut Encoder) -> c_int {
    if output.is_null() || size > u32::MAX as usize {
        return -22;
    }
    unsafe {
        *output = core::ptr::null_mut();
    }
    let buffer = if size == 0 {
        core::ptr::null_mut()
    } else {
        unsafe { kinakaze_alloc::guest::malloc(size) }
    };
    if size != 0 && buffer.is_null() {
        return -12;
    }
    let result = unsafe {
        params::allocate_params(
            output,
            Encoder {
                buffer,
                capacity: size,
                length: 0,
                bytes: [0; 3],
                used: 0,
                sysex: false,
                last_decoded: 0xff,
                no_status: false,
            },
        )
    };
    if result < 0 {
        unsafe {
            kinakaze_alloc::guest::free(buffer);
        }
    }
    result
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_free")]
pub unsafe extern "sysv64" fn snd_midi_event_free(encoder: *mut Encoder) {
    if encoder.is_null() {
        return;
    }
    unsafe {
        kinakaze_alloc::guest::free((*encoder).buffer);
        kinakaze_alloc::guest::free(encoder.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_reset_encode")]
pub unsafe extern "sysv64" fn snd_midi_event_reset_encode(encoder: *mut Encoder) {
    if !encoder.is_null() {
        unsafe {
            (*encoder).used = 0;
            (*encoder).length = 0;
            (*encoder).sysex = false;
        }
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_encode_byte")]
pub unsafe extern "sysv64" fn snd_midi_event_encode_byte(
    encoder: *mut Encoder,
    byte: c_int,
    event: *mut u8,
) -> c_int {
    if encoder.is_null() || event.is_null() || !(0..=255).contains(&byte) {
        return -22;
    }
    let encoder = unsafe { &mut *encoder };
    let byte = byte as u8;
    unsafe {
        *event = 255;
    }
    if byte == 0xf0 {
        encoder.sysex = true;
        encoder.length = 0;
        encoder.used = 0;
    }
    if encoder.sysex && byte < 0xf8 {
        if byte & 0x80 != 0 && byte != 0xf0 && byte != 0xf7 {
            encoder.sysex = false;
            encoder.length = 0;
        } else {
            if encoder.capacity == 0 {
                return -12;
            }
            unsafe {
                *encoder.buffer.add(encoder.length) = byte;
            }
            encoder.length += 1;
            encoder.sysex = byte != 0xf7;
            if !encoder.sysex || encoder.length == encoder.capacity {
                unsafe {
                    *event = 130;
                    *event.add(1) = (*event.add(1) & !12) | 4;
                    event
                        .add(16)
                        .cast::<u32>()
                        .write_unaligned(encoder.length as u32);
                    event
                        .add(20)
                        .cast::<*mut u8>()
                        .write_unaligned(encoder.buffer);
                }
                encoder.length = 0;
                return 1;
            }
            return 0;
        }
    }
    let message = match rawmidi::encode(&mut encoder.bytes, &mut encoder.used, byte) {
        Ok(Some(m)) => m.to_le_bytes(),
        Ok(None) => return 0,
        Err(_) => {
            // ALSA consumes stray data and undefined status bytes silently.
            if byte & 0x80 != 0 {
                encoder.used = 0;
            }
            return 0;
        }
    };
    let status = message[0];
    let (kind, value) = match status {
        0x80..=0x8f => (7, 0),
        0x90..=0x9f => (6, 0),
        0xa0..=0xaf => (8, 0),
        0xb0..=0xbf => (10, message[2] as i32),
        0xc0..=0xcf => (11, message[1] as i32),
        0xd0..=0xdf => (12, message[1] as i32),
        0xe0..=0xef => (13, (message[1] as i32 | (message[2] as i32) << 7) - 8192),
        0xf1 => (22, message[1] as i32),
        0xf2 => (20, message[1] as i32 | (message[2] as i32) << 7),
        0xf3 => (21, message[1] as i32),
        0xf6 => (40, 0),
        0xf8 => (36, 0),
        0xfa => (30, 0),
        0xfb => (31, 0),
        0xfc => (32, 0),
        0xfe => (42, 0),
        0xff => (41, 0),
        _ => return 0,
    };
    unsafe {
        *event = kind;
        *event.add(1) &= !12;
        let data = event.add(16);
        core::ptr::write_bytes(data, 0, 12);
        if status < 0xf0 {
            *data = status & 15;
        }
        if (6..=8).contains(&kind) {
            *data.add(1) = message[1];
            *data.add(2) = message[2];
        } else {
            if kind == 10 {
                data.add(4).cast::<u32>().write_unaligned(message[1] as u32);
            }
            data.add(8).cast::<i32>().write_unaligned(value);
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_no_status")]
pub unsafe extern "sysv64" fn snd_midi_event_no_status(encoder: *mut Encoder, on: c_int) {
    if !encoder.is_null() {
        unsafe {
            (*encoder).no_status = on != 0;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_reset_decode")]
pub unsafe extern "sysv64" fn snd_midi_event_reset_decode(encoder: *mut Encoder) {
    if !encoder.is_null() {
        unsafe {
            (*encoder).last_decoded = 0xff;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_init")]
pub unsafe extern "sysv64" fn snd_midi_event_init(encoder: *mut Encoder) {
    unsafe {
        snd_midi_event_reset_encode(encoder);
        snd_midi_event_reset_decode(encoder);
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_resize_buffer")]
pub unsafe extern "sysv64" fn snd_midi_event_resize_buffer(
    encoder: *mut Encoder,
    size: usize,
) -> c_int {
    if encoder.is_null() || size > u32::MAX as usize {
        return -22;
    }
    if unsafe { (*encoder).capacity } == size {
        return 0;
    }
    let buffer = if size == 0 {
        core::ptr::null_mut()
    } else {
        unsafe { kinakaze_alloc::guest::malloc(size) }
    };
    if size != 0 && buffer.is_null() {
        return -12;
    }
    unsafe {
        kinakaze_alloc::guest::free((*encoder).buffer);
        (*encoder).buffer = buffer;
        (*encoder).capacity = size;
        snd_midi_event_reset_encode(encoder);
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_encode")]
pub unsafe extern "sysv64" fn snd_midi_event_encode(
    encoder: *mut Encoder,
    input: *const u8,
    count: i64,
    event: *mut u8,
) -> i64 {
    if encoder.is_null() || event.is_null() || count < 0 || (count > 0 && input.is_null()) {
        return -22;
    }
    unsafe {
        *event = 255;
    }
    for index in 0..count as usize {
        let result =
            unsafe { snd_midi_event_encode_byte(encoder, *input.add(index) as i32, event) };
        if result < 0 {
            return result as i64;
        }
        if result > 0 {
            return (index + 1) as i64;
        }
    }
    count
}

/// Emit complete MIDI messages atomically with respect to the output capacity.
/// No heap allocation is needed, including 14-bit controller/RPN expansion.
#[unsafe(export_name = "kinakaze_engine_libasound_snd_midi_event_decode")]
pub unsafe extern "sysv64" fn snd_midi_event_decode(
    encoder: *mut Encoder,
    output: *mut u8,
    count: i64,
    event: *const u8,
) -> i64 {
    if encoder.is_null() || event.is_null() || count < 0 || (count > 0 && output.is_null()) {
        return -22;
    }
    let encoder = unsafe { &mut *encoder };
    let kind = unsafe { *event };
    if kind == 130 {
        if unsafe { *event.add(1) } & 12 != 4 {
            return -22;
        }
        let length = unsafe { event.add(16).cast::<u32>().read_unaligned() } as usize;
        let input = unsafe { event.add(20).cast::<*const u8>().read_unaligned() };
        if length as i64 > count {
            return -12;
        }
        if length != 0 {
            if input.is_null() {
                return -22;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(input, output, length);
            }
        }
        encoder.last_decoded = 0xff;
        return length as i64;
    }
    let data = unsafe { event.add(16) };
    let channel = unsafe { *data } & 15;
    let param = unsafe { data.add(4).cast::<u32>().read_unaligned() };
    let value = unsafe { data.add(8).cast::<i32>().read_unaligned() };
    let mut bytes = [0u8; 12];
    let mut used = 0;
    let mut last = encoder.last_decoded;
    let mut message = |status: u8, values: &[u8]| {
        if status >= 0xf0 || encoder.no_status || last != status {
            bytes[used] = status;
            used += 1;
        }
        bytes[used..used + values.len()].copy_from_slice(values);
        used += values.len();
        if status < 0xf0 {
            last = status;
        } else if status < 0xf8 {
            last = 0xff;
        }
    };
    match kind {
        6..=8 => {
            let status = match kind {
                6 => 0x90,
                7 => 0x80,
                _ => 0xa0,
            } | channel;
            message(
                status,
                &[unsafe { *data.add(1) } & 127, unsafe { *data.add(2) } & 127],
            );
        }
        10 => message(0xb0 | channel, &[param as u8 & 127, value as u8 & 127]),
        11 | 12 => message(
            (if kind == 11 { 0xc0 } else { 0xd0 }) | channel,
            &[value as u8 & 127],
        ),
        13 => {
            let value = value.wrapping_add(8192);
            message(
                0xe0 | channel,
                &[value as u8 & 127, (value >> 7) as u8 & 127],
            );
        }
        14..=16 => {
            if kind == 14 {
                if param > 31 {
                    return -22;
                }
                message(0xb0 | channel, &[param as u8, (value >> 7) as u8 & 127]);
                message(0xb0 | channel, &[param as u8 + 32, value as u8 & 127]);
            } else {
                let selector = if kind == 15 { 99 } else { 101 };
                message(0xb0 | channel, &[selector, (param >> 7) as u8 & 127]);
                message(0xb0 | channel, &[selector - 1, param as u8 & 127]);
                message(0xb0 | channel, &[6, (value >> 7) as u8 & 127]);
                message(0xb0 | channel, &[38, value as u8 & 127]);
            }
        }
        20 => message(0xf2, &[value as u8 & 127, (value >> 7) as u8 & 127]),
        21 => message(0xf3, &[value as u8 & 127]),
        22 => message(0xf1, &[value as u8 & 127]),
        30 => message(0xfa, &[]),
        31 => message(0xfb, &[]),
        32 => message(0xfc, &[]),
        36 => message(0xf8, &[]),
        40 => message(0xf6, &[]),
        41 => message(0xff, &[]),
        42 => message(0xfe, &[]),
        _ => return -2,
    }
    if used as i64 > count {
        return -12;
    }
    if used != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), output, used);
        }
    }
    encoder.last_decoded = last;
    used as i64
}
