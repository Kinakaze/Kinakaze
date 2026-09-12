//! Channel maps and device hints returned to the guest use its allocator.
use super::*;
use core::{mem, ptr};
use kinakaze_alloc::guest;
use std::ffi::CStr;

#[repr(C)]
pub struct ChannelMap {
    channels: u32,
    positions: [u32; 0],
}

/// A null-terminated list of fixed maps supported by the default PCM bridge.
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_query_chmaps")]
pub unsafe extern "sysv64" fn snd_pcm_query_chmaps(pcm: *mut SndPcm) -> *mut *mut u32 {
    if pcm.is_null() {
        return ptr::null_mut();
    }
    let list = unsafe { guest::malloc(3 * mem::size_of::<*mut u32>()) }.cast::<*mut u32>();
    if list.is_null() {
        return list;
    }
    unsafe {
        ptr::write_bytes(list, 0, 3);
    }
    for (index, positions) in [&[2u32][..], &[3u32, 4][..]].into_iter().enumerate() {
        let map = unsafe { guest::malloc((positions.len() + 2) * 4) }.cast::<u32>();
        if map.is_null() {
            unsafe {
                snd_pcm_free_chmaps(list);
            }
            return ptr::null_mut();
        }
        unsafe {
            *map = 1; // SND_CHMAP_TYPE_FIXED
            *map.add(1) = positions.len() as u32;
            ptr::copy_nonoverlapping(positions.as_ptr(), map.add(2), positions.len());
            *list.add(index) = map;
        }
    }
    list
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_free_chmaps")]
pub unsafe extern "sysv64" fn snd_pcm_free_chmaps(list: *mut *mut u32) {
    if list.is_null() {
        return;
    }
    unsafe {
        let mut entry = list;
        while !(*entry).is_null() {
            guest::free((*entry).cast());
            entry = entry.add(1);
        }
        guest::free(list.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_get_chmap")]
pub unsafe extern "sysv64" fn snd_pcm_get_chmap(pcm: *mut SndPcm) -> *mut ChannelMap {
    if pcm.is_null() {
        return ptr::null_mut();
    }
    let positions: &[u32] = match unsafe { (*pcm).hw_params.channels } {
        1 => &[2],
        2 => &[3, 4],
        _ => return ptr::null_mut(),
    };
    let result = unsafe { guest::malloc((positions.len() + 1) * 4) }.cast::<u32>();
    if !result.is_null() {
        unsafe {
            *result = positions.len() as u32;
            ptr::copy_nonoverlapping(positions.as_ptr(), result.add(1), positions.len());
        }
    }
    result.cast()
}
const NAMES: [&str; 37] = [
    "UNKNOWN", "NA", "MONO", "FL", "FR", "RL", "RR", "FC", "LFE", "SL", "SR", "RC", "FLC", "FRC",
    "RLC", "RRC", "FLW", "FRW", "FLH", "FCH", "FRH", "TC", "TFL", "TFR", "TFC", "TRL", "TRR",
    "TRC", "TFLC", "TFRC", "TSL", "TSR", "LLFE", "RLFE", "BC", "BLC", "BRC",
];
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_chmap_print")]
pub unsafe extern "sysv64" fn snd_pcm_chmap_print(
    map: *const ChannelMap,
    capacity: usize,
    buffer: *mut c_char,
) -> c_int {
    if map.is_null() || buffer.is_null() {
        return -22;
    }
    if capacity == 0 {
        return -12;
    }
    let count = unsafe { (*map).channels } as usize;
    let positions = unsafe { core::slice::from_raw_parts(map.cast::<u32>().add(1), count) };
    let mut written = 0;
    for (index, value) in positions.iter().copied().enumerate() {
        let position = (value & 0xffff) as usize;
        let name = if value & 0x20000 != 0 {
            position.to_string()
        } else {
            NAMES
                .get(position)
                .map_or_else(|| format!("Ch{position}"), |name| (*name).into())
        };
        for part in [
            if index == 0 { "" } else { " " },
            name.as_str(),
            if value & 0x10000 != 0 { "[INV]" } else { "" },
        ] {
            for byte in part.bytes() {
                if written == capacity - 1 {
                    unsafe {
                        *buffer.add(written) = 0;
                    }
                    return -12;
                }
                unsafe {
                    *buffer.add(written) = byte as c_char;
                }
                written += 1;
            }
        }
    }
    unsafe {
        *buffer.add(written) = 0;
    }
    written.min(i32::MAX as usize) as i32
}
#[repr(C)]
struct Hints {
    entries: [*mut c_void; 2],
    name: [u8; 8],
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_device_name_hint")]
pub unsafe extern "sysv64" fn snd_device_name_hint(
    card: c_int,
    interface: *const c_char,
    output: *mut *mut *mut c_void,
) -> c_int {
    if output.is_null() || interface.is_null() {
        return -22;
    }
    unsafe {
        *output = ptr::null_mut();
    }
    if card < -1 || card > 0 {
        return -19;
    }
    if unsafe { CStr::from_ptr(interface) }.to_bytes() != b"pcm" {
        return -2;
    }
    let hints = unsafe { guest::malloc(mem::size_of::<Hints>()) }.cast::<Hints>();
    if hints.is_null() {
        return -12;
    }
    unsafe {
        hints.write(Hints {
            entries: [ptr::null_mut(); 2],
            name: *b"default\0",
        });
        if windows_sys::Win32::Media::Audio::waveOutGetNumDevs() > 0 {
            (*hints).entries[0] = (*hints).name.as_mut_ptr().cast();
        }
        *output = (*hints).entries.as_mut_ptr();
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_device_name_free_hint")]
pub unsafe extern "sysv64" fn snd_device_name_free_hint(hints: *mut *mut c_void) -> c_int {
    unsafe {
        guest::free(hints.cast());
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_device_name_get_hint")]
pub unsafe extern "sysv64" fn snd_device_name_get_hint(
    hint: *const c_void,
    id: *const c_char,
) -> *mut c_char {
    if hint.is_null() || id.is_null() {
        return ptr::null_mut();
    }
    let value = match unsafe { CStr::from_ptr(id) }.to_bytes() {
        b"NAME" => unsafe { CStr::from_ptr(hint.cast()) },
        b"DESC" => c"Windows default playback device",
        b"IOID" => c"Output",
        _ => return ptr::null_mut(),
    };
    let bytes = value.to_bytes_with_nul();
    let result = unsafe { guest::malloc(bytes.len()) };
    if !result.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), result, bytes.len());
        }
    }
    result.cast()
}
