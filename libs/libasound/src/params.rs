//! Parameter manipulation and checked Linux frame-count ABI.
use super::*;

pub(super) unsafe fn allocate_params<T>(output: *mut *mut T, value: T) -> c_int {
    let data = unsafe { kinakaze_alloc::guest::malloc(core::mem::size_of::<T>()) }.cast::<T>();
    unsafe {
        *output = data;
    }
    if data.is_null() {
        return -12;
    }
    unsafe {
        data.write(value);
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_copy")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_copy(
    to: *mut snd_pcm_hw_params_t,
    from: *const snd_pcm_hw_params_t,
) {
    if !to.is_null() && !from.is_null() {
        unsafe {
            *to = *from;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_sizeof")]
pub extern "sysv64" fn snd_pcm_sw_params_sizeof() -> usize {
    core::mem::size_of::<snd_pcm_sw_params_t>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_stream")]
pub unsafe extern "sysv64" fn snd_pcm_stream(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        -22
    } else {
        unsafe { (*pcm).stream }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_set_start_threshold")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_set_start_threshold(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_sw_params_t,
    value: c_ulong,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    unsafe {
        (*params).start_threshold = value;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_sw_params_set_period_event")]
pub unsafe extern "sysv64" fn snd_pcm_sw_params_set_period_event(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_sw_params_t,
    value: c_int,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    if value != 0 {
        return -38;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_period_size_integer")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_period_size_integer(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
) -> c_int {
    if params.is_null() { -22 } else { 0 } // Only integral frame counts are representable.
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_periods_integer")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_periods_integer(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
) -> c_int {
    if params.is_null() { -22 } else { 0 }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_periods_min")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_periods_min(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    let mut minimum = unsafe { *value };
    if !direction.is_null() && unsafe { *direction } > 0 {
        let Some(next) = minimum.checked_add(1) else {
            return -22;
        };
        minimum = next;
    }
    minimum = minimum.max(unsafe { (*params).periods_min });
    if minimum > unsafe { (*params).periods_max } {
        return -22;
    }
    unsafe {
        (*params).periods = (*params).periods.max(minimum);
        (*params).periods_min = minimum;
        *value = minimum;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_set_periods_first")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_set_periods_first(
    _pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    value: *mut c_uint,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        let first = (*params).periods_min;
        (*params).periods = first;
        (*params).periods_max = first;
        *value = first;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_test_rate")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_test_rate(
    pcm: *mut SndPcm,
    params: *mut snd_pcm_hw_params_t,
    rate: c_uint,
    direction: c_int,
) -> c_int {
    if params.is_null() {
        return -22;
    }
    let mut copy = unsafe { *params };
    unsafe { snd_pcm_hw_params_set_rate(pcm, &raw mut copy, rate, direction) }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_buffer_size_max")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_buffer_size_max(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_ulong,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        *value = 65536;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_period_size_min")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_period_size_min(
    params: *const snd_pcm_hw_params_t,
    value: *mut c_ulong,
    direction: *mut c_int,
) -> c_int {
    if params.is_null() || value.is_null() {
        return -22;
    }
    unsafe {
        *value = 64;
        if !direction.is_null() {
            *direction = 0;
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_free")]
pub unsafe extern "sysv64" fn snd_pcm_hw_free(pcm: *mut SndPcm) -> c_int {
    if pcm.is_null() {
        return -22;
    }
    let mut audio = unsafe { (*pcm).audio.lock() }.unwrap_or_else(|e| e.into_inner());
    *audio = None;
    unsafe {
        (*pcm).state.store(SND_PCM_STATE_OPEN, Ordering::Release);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linux_frame_sizes_are_64_bit_and_preserve_high_words() {
        unsafe {
            assert_eq!(core::mem::size_of::<c_long>(), 8);
            let mut params = snd_pcm_sw_params_t::default();
            assert_eq!(
                snd_pcm_sw_params_set_start_threshold(
                    core::ptr::null_mut(),
                    &raw mut params,
                    1u64 << 40
                ),
                0
            );
            assert_eq!(params.start_threshold, 1u64 << 40);
            let mut frames = u64::MAX;
            let hw = snd_pcm_hw_params_t::default();
            assert_eq!(
                snd_pcm_hw_params_get_buffer_size_max(&raw const hw, &raw mut frames),
                0
            );
            assert_eq!(frames, 65536);
            let mut pcm = core::ptr::null_mut();
            assert_eq!(
                snd_pcm_open(&raw mut pcm, c"default".as_ptr(), SND_PCM_STREAM_CAPTURE, 0),
                -38
            );
            assert!(pcm.is_null());
        }
    }
}
