//! On-demand WinMM output device enumeration.
use super::*;
use windows_sys::Win32::Media::Audio::{MIDIOUTCAPSW, midiOutGetDevCapsW, midiOutGetNumDevs};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Info {
    device: u32,
    subdevice: u32,
    stream: i32,
    name: [u8; 128],
}
impl Default for Info {
    fn default() -> Self {
        Self {
            device: 0,
            subdevice: 0,
            stream: 0,
            name: [0; 128],
        }
    }
}

unsafe fn query(info: *mut Info) -> i32 {
    if info.is_null() {
        return -22;
    }
    let info = unsafe { &mut *info };
    if info.stream != 0 || info.subdevice != 0 {
        return -19;
    }
    let mut caps = MIDIOUTCAPSW::default();
    if unsafe {
        midiOutGetDevCapsW(
            info.device as usize,
            &raw mut caps,
            size_of::<MIDIOUTCAPSW>() as u32,
        )
    } != 0
    {
        return -19;
    }
    let name = caps.szPname;
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    let text = String::from_utf16_lossy(&name[..end]);
    info.name.fill(0);
    let length = text.len().min(info.name.len() - 1);
    info.name[..length].copy_from_slice(&text.as_bytes()[..length]);
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_sizeof")]
pub extern "sysv64" fn snd_rawmidi_info_sizeof() -> usize {
    size_of::<Info>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_malloc")]
pub unsafe extern "sysv64" fn snd_rawmidi_info_malloc(output: *mut *mut Info) -> c_int {
    if output.is_null() {
        -22
    } else {
        unsafe { params::allocate_params(output, Info::default()) }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_free")]
pub unsafe extern "sysv64" fn snd_rawmidi_info_free(info: *mut Info) {
    unsafe {
        kinakaze_alloc::guest::free(info.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info")]
pub unsafe extern "sysv64" fn snd_rawmidi_info(midi: *mut Midi, info: *mut Info) -> c_int {
    if midi.is_null() || info.is_null() {
        return -22;
    }
    unsafe {
        *info = Info {
            device: (*midi).device,
            ..Default::default()
        };
        query(info)
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_rawmidi_info")]
pub unsafe extern "sysv64" fn snd_ctl_rawmidi_info(ctl: *mut c_void, info: *mut Info) -> c_int {
    if ctl.is_null() {
        -22
    } else {
        unsafe { query(info) }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_rawmidi_next_device")]
pub unsafe extern "sysv64" fn snd_ctl_rawmidi_next_device(
    ctl: *mut c_void,
    device: *mut c_int,
) -> c_int {
    if ctl.is_null() || device.is_null() {
        return -22;
    }
    let next = unsafe { *device }.saturating_add(1).max(0);
    unsafe {
        *device = if (next as u32) < midiOutGetNumDevs() {
            next
        } else {
            -1
        };
    }
    0
}

macro_rules! info_setter {
    ($name:ident, $field:ident, $type:ty) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libasound_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(info: *mut Info, value: $type) {
            if !info.is_null() {
                unsafe {
                    (*info).$field = value;
                }
            }
        }
    };
}
info_setter!(snd_rawmidi_info_set_device, device, u32);
info_setter!(snd_rawmidi_info_set_subdevice, subdevice, u32);
info_setter!(snd_rawmidi_info_set_stream, stream, i32);
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_get_card")]
pub unsafe extern "sysv64" fn snd_rawmidi_info_get_card(_info: *const Info) -> c_int {
    -1
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_get_id")]
pub unsafe extern "sysv64" fn snd_rawmidi_info_get_id(info: *const Info) -> *const c_char {
    if info.is_null() {
        ptr::null()
    } else {
        c"winmm".as_ptr()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_get_name")]
pub unsafe extern "sysv64" fn snd_rawmidi_info_get_name(info: *const Info) -> *const c_char {
    if info.is_null() {
        ptr::null()
    } else {
        unsafe { (&raw const (*info).name).cast() }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_rawmidi_info_get_subdevices_count")]
pub unsafe extern "sysv64" fn snd_rawmidi_info_get_subdevices_count(info: *const Info) -> c_uint {
    u32::from(!info.is_null())
}
