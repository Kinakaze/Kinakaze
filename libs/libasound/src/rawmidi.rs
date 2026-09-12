//! WinMM short-message output. Input and SysEx are explicitly unavailable.
//! Handles are reopened on first use after fork; no host allocation is saved.
use super::*;
use core::ptr;
mod info;
use windows_sys::Win32::Media::Audio::{HMIDIOUT, midiOutClose, midiOutOpen, midiOutShortMsg};

#[repr(C)]
pub struct Midi {
    handle: usize,
    owner: u32,
    device: u32,
    fd: i32,
    mode: i32,
    bytes: [u8; 3],
    used: u8,
}

unsafe fn native(midi: *mut Midi) -> Result<HMIDIOUT, i32> {
    if midi.is_null() {
        return Err(-22);
    }
    let midi = unsafe { &mut *midi };
    if midi.owner != std::process::id() {
        let mut handle = ptr::null_mut();
        if unsafe { midiOutOpen(&raw mut handle, midi.device, 0, 0, CALLBACK_NULL) } != 0 {
            return Err(-19);
        }
        midi.handle = handle as usize;
        midi.owner = std::process::id();
    }
    Ok(midi.handle as HMIDIOUT)
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_open")]
pub unsafe extern "sysv64" fn snd_rawmidi_open(
    input: *mut *mut Midi,
    output: *mut *mut Midi,
    name: *const c_char,
    mode: c_int,
) -> c_int {
    unsafe {
        if !input.is_null() {
            *input = ptr::null_mut();
        }
        if !output.is_null() {
            *output = ptr::null_mut();
        }
    }
    if name.is_null() || (input.is_null() && output.is_null()) || mode & !3 != 0 {
        return -22;
    }
    if !input.is_null() {
        return -38;
    }
    let name = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes();
    let device = if name == b"default" {
        0
    } else {
        let Some(tail) = name.strip_prefix(b"hw:0,") else {
            return -19;
        };
        let tail = tail.strip_suffix(b",0").unwrap_or(tail);
        let Some(value) = core::str::from_utf8(tail)
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            return -22;
        };
        value
    };
    let mut handle = ptr::null_mut();
    if unsafe { midiOutOpen(&raw mut handle, device, 0, 0, CALLBACK_NULL) } != 0 {
        return -19;
    }
    let fd = match kinakaze_vfs::eventfd::create_eventfd(1, 0x80800) {
        Ok(fd) => fd,
        Err(error) => {
            unsafe {
                midiOutClose(handle);
            }
            return -error;
        }
    };
    let result = unsafe {
        params::allocate_params(
            output,
            Midi {
                handle: handle as usize,
                owner: std::process::id(),
                device,
                fd,
                mode,
                bytes: [0; 3],
                used: 0,
            },
        )
    };
    if result < 0 {
        let _ = kinakaze_vfs::close(fd);
        unsafe {
            midiOutClose(handle);
        }
    }
    result
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_close")]
pub unsafe extern "sysv64" fn snd_rawmidi_close(midi: *mut Midi) -> c_int {
    if midi.is_null() {
        return -22;
    }
    unsafe {
        if (*midi).owner == std::process::id() && midiOutClose((*midi).handle as HMIDIOUT) != 0 {
            return -5;
        }
        let _ = kinakaze_vfs::close((*midi).fd);
        kinakaze_alloc::guest::free(midi.cast());
    }
    0
}

fn length(status: u8) -> u8 {
    match status {
        0x80..=0xbf | 0xe0..=0xef | 0xf2 => 3,
        0xc0..=0xdf | 0xf1 | 0xf3 => 2,
        0xf6 | 0xf8..=0xff => 1,
        _ => 0,
    }
}

/// Maintains running status without allowing realtime bytes to interrupt it.
pub(super) fn encode(bytes: &mut [u8; 3], used: &mut u8, byte: u8) -> Result<Option<u32>, i32> {
    if byte >= 0xf8 {
        return Ok(Some(byte as u32));
    }
    if byte >= 0x80 {
        if length(byte) == 0 {
            return Err(-95);
        }
        bytes[0] = byte;
        *used = 1;
    } else {
        if *used == 0 || length(bytes[0]) == 0 {
            return Err(-22);
        }
        bytes[*used as usize] = byte;
        *used += 1;
    }
    if *used < length(bytes[0]) {
        return Ok(None);
    }
    let mut word = [0; 4];
    word[..*used as usize].copy_from_slice(&bytes[..*used as usize]);
    *used = u8::from(bytes[0] < 0xf0);
    Ok(Some(u32::from_le_bytes(word)))
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_write")]
pub unsafe extern "sysv64" fn snd_rawmidi_write(
    midi: *mut Midi,
    data: *const c_void,
    size: usize,
) -> i64 {
    if data.is_null() && size != 0 || size > isize::MAX as usize {
        return -22;
    }
    let handle = match unsafe { native(midi) } {
        Ok(handle) => handle,
        Err(e) => return e as i64,
    };
    let midi = unsafe { &mut *midi };
    for at in 0..size {
        let mut bytes = midi.bytes;
        let mut used = midi.used;
        let message = match encode(&mut bytes, &mut used, unsafe { *data.cast::<u8>().add(at) }) {
            Ok(m) => m,
            Err(e) => return if at > 0 { at as i64 } else { e as i64 },
        };
        if let Some(message) = message
            && unsafe { midiOutShortMsg(handle, message) } != 0
        {
            return if at > 0 { at as i64 } else { -5 };
        }
        midi.bytes = bytes;
        midi.used = used;
    }
    size as i64
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_read")]
pub unsafe extern "sysv64" fn snd_rawmidi_read(
    midi: *mut Midi,
    _data: *mut c_void,
    _size: usize,
) -> i64 {
    if midi.is_null() { -22 } else { -9 } // Output-only handles cannot be read.
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_drain")]
pub unsafe extern "sysv64" fn snd_rawmidi_drain(midi: *mut Midi) -> c_int {
    if midi.is_null() {
        return -22;
    }
    // Completed short messages have already been sent. Draining must preserve
    // running status (and partial messages) for the next byte-oriented write.
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_nonblock")]
pub unsafe extern "sysv64" fn snd_rawmidi_nonblock(midi: *mut Midi, nonblock: c_int) -> c_int {
    if midi.is_null() {
        return -22;
    }
    unsafe {
        (*midi).mode = ((*midi).mode & !2) | if nonblock != 0 { 2 } else { 0 };
    }
    0
}

#[repr(C)]
pub struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_poll_descriptors_count")]
pub unsafe extern "sysv64" fn snd_rawmidi_poll_descriptors_count(midi: *mut Midi) -> c_int {
    if midi.is_null() { -22 } else { 1 }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_poll_descriptors")]
pub unsafe extern "sysv64" fn snd_rawmidi_poll_descriptors(
    midi: *mut Midi,
    fds: *mut PollFd,
    space: c_uint,
) -> c_int {
    if midi.is_null() || fds.is_null() || space < 1 {
        return -22;
    }
    unsafe {
        *fds = PollFd {
            fd: (*midi).fd,
            events: 1,
            revents: 0,
        };
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_poll_descriptors_revents")]
pub unsafe extern "sysv64" fn snd_rawmidi_poll_descriptors_revents(
    midi: *mut Midi,
    fds: *mut PollFd,
    count: c_uint,
    revents: *mut u16,
) -> c_int {
    if midi.is_null()
        || fds.is_null()
        || count != 1
        || revents.is_null()
        || unsafe { (*fds).fd != (*midi).fd }
    {
        return -22;
    }
    unsafe {
        *revents =
            ((*fds).revents as u16 & (8 | 16 | 32)) | if (*fds).revents & 1 != 0 { 4 } else { 0 };
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_messages_running_status_and_realtime_interleave() {
        let mut data = [0; 3];
        let mut used = 0;
        assert_eq!(encode(&mut data, &mut used, 0x90), Ok(None));
        assert_eq!(encode(&mut data, &mut used, 60), Ok(None));
        assert_eq!(encode(&mut data, &mut used, 0xf8), Ok(Some(0xf8)));
        assert_eq!(encode(&mut data, &mut used, 64), Ok(Some(0x403c90)));
        assert_eq!(encode(&mut data, &mut used, 61), Ok(None));
        assert_eq!(encode(&mut data, &mut used, 0), Ok(Some(0x3d90)));
        assert_eq!(encode(&mut data, &mut used, 0xf0), Err(-95));
    }

    #[test]
    fn drain_does_not_erase_running_status() {
        let mut midi = Midi {
            handle: 0,
            owner: 0,
            device: 0,
            fd: -1,
            mode: 0,
            bytes: [0x90, 60, 64],
            used: 1,
        };
        assert_eq!(unsafe { snd_rawmidi_drain(&raw mut midi) }, 0);
        assert_eq!(encode(&mut midi.bytes, &mut midi.used, 61), Ok(None));
        assert_eq!(
            encode(&mut midi.bytes, &mut midi.used, 64),
            Ok(Some(0x403d90))
        );
    }
}
