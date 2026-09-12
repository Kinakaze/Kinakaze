//! PulseAudio volume arithmetic, independent of stream/device ownership.
use super::*;
const NORM: u32 = 0x10000;
const MAX: u32 = 0x7fffffff;
const INVALID: u32 = u32::MAX;
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_sw_volume_multiply")]
pub extern "sysv64" fn pa_sw_volume_multiply(a: u32, b: u32) -> u32 {
    if a > MAX || b > MAX {
        return INVALID;
    }
    ((a as u64 * b as u64 + NORM as u64 / 2) / NORM as u64).min(MAX as u64) as u32
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_sw_volume_divide")]
pub extern "sysv64" fn pa_sw_volume_divide(a: u32, b: u32) -> u32 {
    if a > MAX || b > MAX {
        return INVALID;
    }
    if b == 0 {
        return 0;
    }
    ((a as u64 * NORM as u64 + b as u64 / 2) / b as u64).min(MAX as u64) as u32
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_valid")]
pub unsafe extern "sysv64" fn pa_cvolume_valid(volume: *const pa_cvolume) -> c_int {
    if volume.is_null() {
        return 0;
    }
    let v = unsafe { &*volume };
    i32::from(
        (1..=32).contains(&v.channels) && v.values[..v.channels as usize].iter().all(|v| *v <= MAX),
    )
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_set")]
pub unsafe extern "sysv64" fn pa_cvolume_set(
    volume: *mut pa_cvolume,
    channels: c_uint,
    value: u32,
) -> *mut pa_cvolume {
    if volume.is_null() || !(1..=32).contains(&channels) || value > MAX {
        return ptr::null_mut();
    }
    unsafe {
        (*volume).channels = channels as u8;
        (&mut (*volume).values)[..channels as usize].fill(value);
    }
    volume
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_avg")]
pub unsafe extern "sysv64" fn pa_cvolume_avg(volume: *const pa_cvolume) -> u32 {
    if unsafe { pa_cvolume_valid(volume) } == 0 {
        return 0;
    }
    let v = unsafe { &*volume };
    (v.values[..v.channels as usize]
        .iter()
        .map(|x| *x as u64)
        .sum::<u64>()
        / v.channels as u64) as u32
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_sw_cvolume_multiply_scalar")]
pub unsafe extern "sysv64" fn pa_sw_cvolume_multiply_scalar(
    output: *mut pa_cvolume,
    input: *const pa_cvolume,
    scale: u32,
) -> *mut pa_cvolume {
    if output.is_null() || unsafe { pa_cvolume_valid(input) } == 0 || scale > MAX {
        return ptr::null_mut();
    }
    let value = unsafe { *input };
    unsafe {
        (*output).channels = value.channels;
    }
    for index in 0..value.channels as usize {
        unsafe {
            (*output).values[index] = pa_sw_volume_multiply(value.values[index], scale);
        }
    }
    output
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_get_library_version")]
pub extern "sysv64" fn pa_get_library_version() -> *const c_char {
    SERVER_VERSION.as_ptr().cast()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn volume_rounding_overflow_and_aliasing() {
        unsafe {
            assert_eq!(pa_sw_volume_multiply(NORM, NORM / 2), NORM / 2);
            assert_eq!(pa_sw_volume_multiply(MAX, MAX), MAX);
            assert_eq!(pa_sw_volume_divide(NORM, 0), 0);
            assert_eq!(pa_sw_volume_multiply(INVALID, 0), INVALID);
            let mut volume = pa_cvolume {
                channels: 0,
                values: [0; 32],
            };
            assert!(!pa_cvolume_set(&raw mut volume, 2, NORM).is_null());
            assert!(
                !pa_sw_cvolume_multiply_scalar(&raw mut volume, &raw const volume, NORM / 2)
                    .is_null()
            );
            assert_eq!(pa_cvolume_avg(&raw const volume), NORM / 2);
        }
    }
}
