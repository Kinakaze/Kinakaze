//! One playback element backed by the selected WinMM device's volume control.
//! Unsupported switch/capture controls are never advertised as capabilities.
use super::*;
use core::ptr;
use windows_sys::Win32::Media::Audio::{
    WAVECAPS_LRVOLUME, WAVECAPS_VOLUME, WAVEOUTCAPSW, waveOutGetDevCapsW, waveOutGetVolume,
    waveOutSetVolume,
};

#[repr(C)]
#[derive(Default)]
pub struct Mixer {
    device: usize,
    attached: bool,
    registered: bool,
    loaded: bool,
    channels: u16,
    support: u32,
    callback: Option<unsafe extern "sysv64" fn(*mut Mixer, u32) -> c_int>,
    callback_private: usize,
    observed_volume: Option<u32>,
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_open")]
pub unsafe extern "sysv64" fn snd_mixer_open(output: *mut *mut Mixer, mode: c_int) -> c_int {
    if output.is_null() || mode != 0 {
        return -22;
    }
    unsafe { params::allocate_params(output, Mixer::default()) }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_close")]
pub unsafe extern "sysv64" fn snd_mixer_close(mixer: *mut Mixer) -> c_int {
    if mixer.is_null() {
        return -22;
    }
    unsafe { kinakaze_alloc::guest::free(mixer.cast()) };
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_attach")]
pub unsafe extern "sysv64" fn snd_mixer_attach(mixer: *mut Mixer, name: *const c_char) -> c_int {
    if mixer.is_null() || name.is_null() {
        return -22;
    }
    if unsafe { (*mixer).attached } {
        return -16;
    }
    let name = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes();
    let device = if name == b"default" {
        WAVE_MAPPER as usize
    } else if let Some(number) = name.strip_prefix(b"hw:") {
        let Some(number) = core::str::from_utf8(number)
            .ok()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            return -22;
        };
        number as usize
    } else {
        return -19;
    };
    let mut caps = WAVEOUTCAPSW::default();
    if unsafe { waveOutGetDevCapsW(device, &raw mut caps, size_of::<WAVEOUTCAPSW>() as u32) } != 0 {
        return -19;
    }
    unsafe {
        (*mixer).device = device;
        (*mixer).attached = true;
        (*mixer).channels = caps.wChannels;
        (*mixer).support = caps.dwSupport;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_register")]
pub unsafe extern "sysv64" fn snd_mixer_selem_register(
    mixer: *mut Mixer,
    options: *const c_void,
    class: *mut *mut c_void,
) -> c_int {
    if mixer.is_null() {
        return -22;
    }
    if !options.is_null() {
        return -95;
    }
    unsafe {
        (*mixer).registered = true;
        if !class.is_null() {
            *class = mixer.cast();
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_load")]
pub unsafe extern "sysv64" fn snd_mixer_load(mixer: *mut Mixer) -> c_int {
    if mixer.is_null() || !unsafe { (*mixer).attached } {
        return -22;
    }
    unsafe {
        (*mixer).loaded = true;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_first_elem")]
pub unsafe extern "sysv64" fn snd_mixer_first_elem(mixer: *mut Mixer) -> *mut Mixer {
    if !mixer.is_null()
        && unsafe {
            (*mixer).loaded && (*mixer).registered && (*mixer).support & WAVECAPS_VOLUME != 0
        }
    {
        mixer
    } else {
        ptr::null_mut()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_elem_next")]
pub unsafe extern "sysv64" fn snd_mixer_elem_next(_element: *mut Mixer) -> *mut Mixer {
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_get_name")]
pub unsafe extern "sysv64" fn snd_mixer_selem_get_name(element: *mut Mixer) -> *const c_char {
    if element.is_null() {
        ptr::null()
    } else {
        c"PCM".as_ptr()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_get_mixername")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_get_mixername(
    info: *const snd_ctl_card_info_t,
) -> *const c_char {
    if info.is_null() {
        ptr::null()
    } else {
        c"WinMM".as_ptr()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_channel_name")]
pub extern "sysv64" fn snd_mixer_selem_channel_name(channel: c_int) -> *const c_char {
    match channel {
        0 => c"Front Left",
        1 => c"Front Right",
        2 => c"Rear Left",
        3 => c"Rear Right",
        4 => c"Front Center",
        5 => c"Woofer",
        6 => c"Side Left",
        7 => c"Side Right",
        8 => c"Rear Center",
        _ => c"Unknown",
    }
    .as_ptr()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_is_active")]
pub unsafe extern "sysv64" fn snd_mixer_selem_is_active(element: *mut Mixer) -> c_int {
    i32::from(!unsafe { snd_mixer_first_elem(element) }.is_null())
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_is_playback_mono")]
pub unsafe extern "sysv64" fn snd_mixer_selem_is_playback_mono(element: *mut Mixer) -> c_int {
    i32::from(!element.is_null() && unsafe { (*element).support & WAVECAPS_LRVOLUME == 0 })
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_has_playback_channel")]
pub unsafe extern "sysv64" fn snd_mixer_selem_has_playback_channel(
    element: *mut Mixer,
    channel: c_int,
) -> c_int {
    i32::from(
        unsafe { snd_mixer_selem_is_active(element) } != 0
            && (channel == 0
                || channel == 1 && unsafe { (*element).support & WAVECAPS_LRVOLUME != 0 }),
    )
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_has_playback_volume")]
pub unsafe extern "sysv64" fn snd_mixer_selem_has_playback_volume(element: *mut Mixer) -> c_int {
    unsafe { snd_mixer_selem_is_active(element) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_get_playback_volume")]
pub unsafe extern "sysv64" fn snd_mixer_selem_get_playback_volume(
    element: *mut Mixer,
    channel: c_int,
    value: *mut c_long,
) -> c_int {
    if value.is_null() || unsafe { snd_mixer_selem_has_playback_channel(element, channel) } == 0 {
        return -22;
    }
    let mut volume = 0;
    if unsafe { waveOutGetVolume((*element).device as HWAVEOUT, &raw mut volume) } != 0 {
        return -5;
    }
    unsafe {
        *value = ((volume >> (channel * 16)) & 0xffff) as c_long;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_set_playback_volume")]
pub unsafe extern "sysv64" fn snd_mixer_selem_set_playback_volume(
    element: *mut Mixer,
    channel: c_int,
    value: c_long,
) -> c_int {
    if !(0..=65535).contains(&value)
        || unsafe { snd_mixer_selem_has_playback_channel(element, channel) } == 0
    {
        return -22;
    }
    let mut volume = 0;
    if unsafe { waveOutGetVolume((*element).device as HWAVEOUT, &raw mut volume) } != 0 {
        return -5;
    }
    volume = (volume & !(0xffff << (channel * 16))) | ((value as u32) << (channel * 16));
    if unsafe { waveOutSetVolume((*element).device as HWAVEOUT, volume) } != 0 {
        -5
    } else {
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_get_playback_volume_range")]
pub unsafe extern "sysv64" fn snd_mixer_selem_get_playback_volume_range(
    element: *mut Mixer,
    min: *mut c_long,
    max: *mut c_long,
) -> c_int {
    if min.is_null() || max.is_null() || unsafe { snd_mixer_selem_is_active(element) } == 0 {
        return -22;
    }
    unsafe {
        *min = 0;
        *max = 65535;
    }
    0
}

macro_rules! absent {
    ($($name:ident),*) => { $(
        #[unsafe(export_name = concat!("kinakaze_engine_libasound_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(_element: *mut Mixer) -> c_int { 0 }
    )* };
}
absent!(
    snd_mixer_selem_has_capture_volume,
    snd_mixer_selem_has_capture_switch,
    snd_mixer_selem_has_playback_switch,
    snd_mixer_selem_is_capture_mono
);
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_has_capture_channel")]
pub unsafe extern "sysv64" fn snd_mixer_selem_has_capture_channel(
    _element: *mut Mixer,
    _channel: c_int,
) -> c_int {
    0
}

macro_rules! unavailable {
    ($($name:ident ($($arg:ident : $ty:ty),*)),*) => { $(
        #[unsafe(export_name = concat!("kinakaze_engine_libasound_", stringify!($name)))]
        pub unsafe extern "sysv64" fn $name(_element: *mut Mixer, $($arg : $ty),*) -> c_int { -6 }
    )* };
}
unavailable!(
    snd_mixer_selem_get_capture_volume(_channel: c_int, _value: *mut c_long),
    snd_mixer_selem_get_capture_volume_range(_min: *mut c_long, _max: *mut c_long),
    snd_mixer_selem_set_capture_volume(_channel: c_int, _value: c_long),
    snd_mixer_selem_get_capture_switch(_channel: c_int, _value: *mut c_int),
    snd_mixer_selem_set_capture_switch_all(_value: c_int),
    snd_mixer_selem_get_playback_switch(_channel: c_int, _value: *mut c_int),
    snd_mixer_selem_set_playback_switch_all(_value: c_int),
    snd_mixer_selem_set_playback_switch(_channel: c_int, _value: c_int),
    snd_mixer_selem_set_capture_volume_all(_value: c_long)
);

#[repr(C)]
pub struct ElementName {
    index: u32,
    name: [u8; 128],
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_id_malloc")]
pub unsafe extern "sysv64" fn snd_mixer_selem_id_malloc(output: *mut *mut ElementName) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe {
        params::allocate_params(
            output,
            ElementName {
                index: 0,
                name: [0; 128],
            },
        )
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_id_free")]
pub unsafe extern "sysv64" fn snd_mixer_selem_id_free(id: *mut ElementName) {
    unsafe { kinakaze_alloc::guest::free(id.cast()) };
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_id_set_index")]
pub unsafe extern "sysv64" fn snd_mixer_selem_id_set_index(id: *mut ElementName, index: u32) {
    if !id.is_null() {
        unsafe {
            (*id).index = index;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_id_set_name")]
pub unsafe extern "sysv64" fn snd_mixer_selem_id_set_name(
    id: *mut ElementName,
    name: *const c_char,
) {
    if id.is_null() {
        return;
    }
    let target = unsafe { &mut (*id).name };
    target.fill(0);
    if !name.is_null() {
        let source = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes();
        let len = source.len().min(target.len() - 1);
        target[..len].copy_from_slice(&source[..len]);
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_find_selem")]
pub unsafe extern "sysv64" fn snd_mixer_find_selem(
    mixer: *mut Mixer,
    id: *const ElementName,
) -> *mut Mixer {
    if id.is_null() || unsafe { (*id).index } != 0 {
        return ptr::null_mut();
    }
    let name = unsafe { core::ffi::CStr::from_ptr((*id).name.as_ptr().cast()) }.to_bytes();
    if name != b"PCM" {
        return ptr::null_mut();
    }
    unsafe { snd_mixer_first_elem(mixer) }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_free")]
pub unsafe extern "sysv64" fn snd_mixer_free(mixer: *mut Mixer) {
    if !mixer.is_null() {
        unsafe {
            (*mixer).loaded = false;
            (*mixer).observed_volume = None;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_detach")]
pub unsafe extern "sysv64" fn snd_mixer_detach(mixer: *mut Mixer, name: *const c_char) -> c_int {
    if mixer.is_null() || name.is_null() {
        return -22;
    }
    if !unsafe { (*mixer).attached } {
        return -2;
    }
    let bytes = unsafe { core::ffi::CStr::from_ptr(name) }.to_bytes();
    let device = if bytes == b"default" {
        Some(WAVE_MAPPER as usize)
    } else {
        bytes
            .strip_prefix(b"hw:")
            .and_then(|v| core::str::from_utf8(v).ok())
            .and_then(|v| v.parse::<usize>().ok())
    };
    if device != Some(unsafe { (*mixer).device }) {
        return -2;
    }
    unsafe {
        snd_mixer_free(mixer);
        (*mixer).attached = false;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_elem_set_callback")]
pub unsafe extern "sysv64" fn snd_mixer_elem_set_callback(
    mixer: *mut Mixer,
    callback: Option<unsafe extern "sysv64" fn(*mut Mixer, u32) -> c_int>,
) {
    if !mixer.is_null() {
        unsafe {
            (*mixer).callback = callback;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_elem_set_callback_private")]
pub unsafe extern "sysv64" fn snd_mixer_elem_set_callback_private(
    mixer: *mut Mixer,
    data: *mut c_void,
) {
    if !mixer.is_null() {
        unsafe {
            (*mixer).callback_private = data as usize;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_elem_get_callback_private")]
pub unsafe extern "sysv64" fn snd_mixer_elem_get_callback_private(
    mixer: *mut Mixer,
) -> *mut c_void {
    if mixer.is_null() {
        ptr::null_mut()
    } else {
        unsafe { (*mixer).callback_private as *mut c_void }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_handle_events")]
pub unsafe extern "sysv64" fn snd_mixer_handle_events(mixer: *mut Mixer) -> c_int {
    if unsafe { snd_mixer_selem_is_active(mixer) } == 0 {
        return -22;
    }
    let mut volume = 0;
    if unsafe { waveOutGetVolume((*mixer).device as HWAVEOUT, &raw mut volume) } != 0 {
        return -5;
    }
    let previous = unsafe { (*mixer).observed_volume.replace(volume) };
    if previous.is_some_and(|old| old != volume) {
        if let Some(callback) = unsafe { (*mixer).callback } {
            return unsafe { callback(mixer, 1) }; // SND_CTL_EVENT_MASK_VALUE
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_poll_descriptors_count")]
pub unsafe extern "sysv64" fn snd_mixer_poll_descriptors_count(mixer: *mut Mixer) -> c_int {
    // WinMM volume discovery has no waitable notification descriptor.
    if mixer.is_null() { -22 } else { 0 }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_poll_descriptors")]
pub unsafe extern "sysv64" fn snd_mixer_poll_descriptors(
    mixer: *mut Mixer,
    _fds: *mut c_void,
    _space: u32,
) -> c_int {
    unsafe { snd_mixer_poll_descriptors_count(mixer) }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_set_playback_volume_all")]
pub unsafe extern "sysv64" fn snd_mixer_selem_set_playback_volume_all(
    mixer: *mut Mixer,
    value: c_long,
) -> c_int {
    let result = unsafe { snd_mixer_selem_set_playback_volume(mixer, 0, value) };
    if result != 0 {
        return result;
    }
    if unsafe { snd_mixer_selem_has_playback_channel(mixer, 1) } != 0 {
        unsafe { snd_mixer_selem_set_playback_volume(mixer, 1, value) }
    } else {
        0
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_ask_playback_vol_dB")]
pub unsafe extern "sysv64" fn snd_mixer_selem_ask_playback_vol_dB(
    mixer: *mut Mixer,
    value: c_long,
    db: *mut c_long,
) -> c_int {
    if db.is_null()
        || !(0..=65535).contains(&value)
        || unsafe { snd_mixer_selem_is_active(mixer) } == 0
    {
        return -22;
    }
    unsafe {
        *db = if value == 0 {
            -9999999
        } else {
            (2000.0 * (value as f64 / 65535.0).log10()).round() as i64
        };
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_mixer_selem_ask_playback_dB_vol")]
pub unsafe extern "sysv64" fn snd_mixer_selem_ask_playback_dB_vol(
    mixer: *mut Mixer,
    db: c_long,
    dir: c_int,
    value: *mut c_long,
) -> c_int {
    if value.is_null() || unsafe { snd_mixer_selem_is_active(mixer) } == 0 {
        return -22;
    }
    let raw = if db <= -9999999 {
        0.0
    } else {
        (65535.0 * 10.0f64.powf(db.min(0) as f64 / 2000.0)).clamp(0.0, 65535.0)
    };
    unsafe {
        *value = (if dir < 0 {
            raw.floor()
        } else if dir > 0 {
            raw.ceil()
        } else {
            raw.round()
        }) as i64;
    }
    0
}
