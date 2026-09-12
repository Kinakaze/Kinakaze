//! PCM information and snapshots are flat guest-owned records.
use super::*;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Status {
    state: c_int,
    avail: c_ulong,
    delay: c_long,
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_dump")]
pub unsafe extern "sysv64" fn snd_pcm_status_dump(
    status: *const Status,
    output: *mut output::Output,
) -> c_int {
    let Some(s) = (unsafe { status.as_ref() }) else {
        return -22;
    };
    let text = format!(
        "state: {}\navail: {}\ndelay: {}\n",
        s.state, s.avail, s.delay
    );
    unsafe { output::write(output, text.as_bytes()) }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_get_subdevice_name")]
pub unsafe extern "sysv64" fn snd_pcm_info_get_subdevice_name(
    info: *const snd_pcm_info_t,
) -> *const c_char {
    if info.is_null() {
        core::ptr::null()
    } else {
        c"Windows default playback device".as_ptr()
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_can_pause")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_can_pause(
    params: *const snd_pcm_hw_params_t,
) -> c_int {
    i32::from(!params.is_null())
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_sizeof")]
pub extern "sysv64" fn snd_pcm_status_sizeof() -> usize {
    core::mem::size_of::<Status>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_malloc")]
pub unsafe extern "sysv64" fn snd_pcm_status_malloc(output: *mut *mut Status) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe { params::allocate_params(output, Status::default()) }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_free")]
pub unsafe extern "sysv64" fn snd_pcm_status_free(status: *mut Status) {
    unsafe { kinakaze_alloc::guest::free(status.cast()) };
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status")]
pub unsafe extern "sysv64" fn snd_pcm_status(pcm: *mut SndPcm, status: *mut Status) -> c_int {
    if pcm.is_null() || status.is_null() {
        return -22;
    }
    let pcm = unsafe { &*pcm };
    let mut audio = pcm.audio.lock().unwrap_or_else(|e| e.into_inner());
    let queued = if let Some(output) = audio.as_mut() {
        output.reap();
        output.buffers.iter().map(|b| b.frames).sum::<u64>()
    } else {
        0
    };
    unsafe {
        *status = Status {
            state: pcm.state.load(Ordering::Acquire),
            avail: pcm.hw_params.buffer_size.saturating_sub(queued),
            delay: queued as c_long,
        };
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_get_avail")]
pub unsafe extern "sysv64" fn snd_pcm_status_get_avail(status: *const Status) -> c_ulong {
    if status.is_null() {
        0
    } else {
        unsafe { (*status).avail }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_get_delay")]
pub unsafe extern "sysv64" fn snd_pcm_status_get_delay(status: *const Status) -> c_long {
    if status.is_null() {
        0
    } else {
        unsafe { (*status).delay }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_status_get_state")]
pub unsafe extern "sysv64" fn snd_pcm_status_get_state(status: *const Status) -> c_int {
    if status.is_null() {
        SND_PCM_STATE_DISCONNECTED
    } else {
        unsafe { (*status).state }
    }
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_sizeof")]
pub extern "sysv64" fn snd_pcm_info_sizeof() -> usize {
    core::mem::size_of::<snd_pcm_info_t>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info")]
pub unsafe extern "sysv64" fn snd_pcm_info(pcm: *mut SndPcm, info: *mut snd_pcm_info_t) -> c_int {
    if pcm.is_null() || info.is_null() {
        return -22;
    }
    unsafe {
        *info = snd_pcm_info_t {
            device: 0,
            subdevice: 0,
            stream: (*pcm).stream,
        };
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_get_card")]
pub unsafe extern "sysv64" fn snd_pcm_info_get_card(info: *const snd_pcm_info_t) -> c_int {
    if info.is_null() { -22 } else { -1 } // The Windows PCM is not a Linux hardware card.
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_get_id")]
pub unsafe extern "sysv64" fn snd_pcm_info_get_id(info: *const snd_pcm_info_t) -> *const c_char {
    if info.is_null() {
        core::ptr::null()
    } else {
        c"waveout".as_ptr()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_info_get_subdevices_count")]
pub unsafe extern "sysv64" fn snd_pcm_info_get_subdevices_count(
    info: *const snd_pcm_info_t,
) -> c_uint {
    u32::from(!info.is_null())
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_channels_min")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_channels_min(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_uint,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        *value = (*params).channels_min;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_channels_max")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_channels_max(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_uint,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        *value = (*params).channels_max;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_set_params")]
pub unsafe extern "sysv64" fn snd_pcm_set_params(
    pcm: *mut SndPcm,
    format: c_int,
    access: c_int,
    channels: c_uint,
    rate: c_uint,
    _soft_resample: c_int,
    latency: c_uint,
) -> c_int {
    if pcm.is_null()
        || !formats::supported(format)
        || access != SND_PCM_ACCESS_RW_INTERLEAVED
        || !(1..=8).contains(&channels)
        || rate == 0
        || latency == 0
    {
        return -22;
    }
    let buffer_size = (u64::from(rate) * u64::from(latency))
        .div_ceil(1_000_000)
        .max(4);
    let mut hw = snd_pcm_hw_params_t {
        format,
        access,
        channels,
        rate,
        channels_min: channels,
        channels_max: channels,
        format_mask: 1u64 << format,
        buffer_size,
        period_size: (buffer_size / 4).max(1),
        buffer_time: latency,
        period_time: latency / 4,
        ..Default::default()
    };
    let result = unsafe { snd_pcm_hw_params(pcm, &raw mut hw) };
    if result < 0 {
        return result;
    }
    unsafe {
        (*pcm).sw_params.avail_min = hw.period_size;
        (*pcm).sw_params.start_threshold = hw.buffer_size;
        (*pcm).sw_params.stop_threshold = hw.buffer_size;
        snd_pcm_prepare(pcm)
    }
}
