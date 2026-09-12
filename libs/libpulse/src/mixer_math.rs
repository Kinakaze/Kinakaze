//! Volume and channel-map helpers used by desktop mixers.
use super::*;
const NORM: u32 = 0x1_0000;
const MAX: u32 = 0x7fff_ffff;
pub(crate) fn left(p: i32) -> bool {
    matches!(p, 1 | 5 | 8 | 10 | 46 | 49)
}
pub(crate) fn right(p: i32) -> bool {
    matches!(p, 2 | 6 | 9 | 11 | 47 | 50)
}
fn front(p: i32) -> bool {
    matches!(p, 1 | 2 | 3 | 8 | 9 | 45 | 46 | 47)
}
fn rear(p: i32) -> bool {
    matches!(p, 4 | 5 | 6 | 48 | 49 | 50)
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_position_to_pretty_string")]
pub extern "sysv64" fn pa_channel_position_to_pretty_string(position: i32) -> *const c_char {
    // Names are stable, human-readable untranslated labels, as with an English
    // PulseAudio locale. Unknown positions return NULL.
    static LABELS: [&[u8]; 12] = [
        b"Mono\0",
        b"Front Left\0",
        b"Front Right\0",
        b"Front Center\0",
        b"Rear Center\0",
        b"Rear Left\0",
        b"Rear Right\0",
        b"Subwoofer\0",
        b"Front Left-of-center\0",
        b"Front Right-of-center\0",
        b"Side Left\0",
        b"Side Right\0",
    ];
    if let Some(name) = LABELS.get(position as usize) {
        name.as_ptr().cast()
    } else {
        unsafe { pa_channel_position_to_string(position) }
    }
}

unsafe fn set_axis(
    v: *mut pa_cvolume,
    m: *const pa_channel_map,
    value: f32,
    negative: fn(i32) -> bool,
    positive: fn(i32) -> bool,
) -> *mut pa_cvolume {
    if !(-1.0..=1.0).contains(&value)
        || unsafe { pa_cvolume_compatible_with_channel_map(v, m) } == 0
    {
        return ptr::null_mut();
    }
    if unsafe { can_axis(m, negative, positive) } == 0 {
        return v;
    }
    let (vol, map) = unsafe { (&mut *v, &*m) };
    let average = |which: fn(i32) -> bool| {
        let mut sum = 0u64;
        let mut count = 0u64;
        for n in 0..vol.channels as usize {
            if which(map.map[n]) {
                sum += vol.values[n] as u64;
                count += 1;
            }
        }
        (sum / count.max(1)) as u32
    };
    let a = average(negative);
    let b = average(positive);
    let loud = a.max(b);
    let wanted_a = if value > 0. {
        (loud as f32 * (1. - value)) as u32
    } else {
        loud
    };
    let wanted_b = if value < 0. {
        (loud as f32 * (1. + value)) as u32
    } else {
        loud
    };
    for n in 0..vol.channels as usize {
        let pair = if negative(map.map[n]) {
            Some((a, wanted_a))
        } else if positive(map.map[n]) {
            Some((b, wanted_b))
        } else {
            None
        };
        if let Some((old, new)) = pair {
            vol.values[n] = if old == 0 {
                new
            } else {
                (vol.values[n] as u64 * new as u64 / old as u64).min(MAX as u64) as u32
            };
        }
    }
    v
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_set_balance")]
pub unsafe extern "sysv64" fn pa_cvolume_set_balance(
    v: *mut pa_cvolume,
    m: *const pa_channel_map,
    value: f32,
) -> *mut pa_cvolume {
    unsafe { set_axis(v, m, value, left, right) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_set_fade")]
pub unsafe extern "sysv64" fn pa_cvolume_set_fade(
    v: *mut pa_cvolume,
    m: *const pa_channel_map,
    value: f32,
) -> *mut pa_cvolume {
    unsafe { set_axis(v, m, value, rear, front) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_set_position")]
pub unsafe extern "sysv64" fn pa_cvolume_set_position(
    v: *mut pa_cvolume,
    m: *const pa_channel_map,
    position: i32,
    value: u32,
) -> *mut pa_cvolume {
    if value > MAX
        || !(0..51).contains(&position)
        || unsafe { pa_cvolume_compatible_with_channel_map(v, m) } == 0
    {
        return ptr::null_mut();
    }
    let (vol, map) = unsafe { (&mut *v, &*m) };
    for n in 0..vol.channels as usize {
        if map.map[n] == position {
            vol.values[n] = value;
        }
    }
    v
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_has_position")]
pub unsafe extern "sysv64" fn pa_channel_map_has_position(
    map: *const pa_channel_map,
    position: c_int,
) -> c_int {
    if unsafe { pa_channel_map_valid(map) } == 0 {
        return 0;
    }
    i32::from(unsafe { &(&(*map).map)[..(*map).channels as usize] }.contains(&position))
}
unsafe fn can_axis(map: *const pa_channel_map, a: fn(i32) -> bool, b: fn(i32) -> bool) -> c_int {
    if unsafe { pa_channel_map_valid(map) } == 0 {
        return 0;
    }
    let positions = unsafe { &(&(*map).map)[..(*map).channels as usize] };
    i32::from(positions.iter().any(|p| a(*p)) && positions.iter().any(|p| b(*p)))
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_can_balance")]
pub unsafe extern "sysv64" fn pa_channel_map_can_balance(map: *const pa_channel_map) -> c_int {
    unsafe { can_axis(map, left, right) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_can_fade")]
pub unsafe extern "sysv64" fn pa_channel_map_can_fade(map: *const pa_channel_map) -> c_int {
    unsafe { can_axis(map, rear, front) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_to_pretty_name")]
pub unsafe extern "sysv64" fn pa_channel_map_to_pretty_name(
    map: *const pa_channel_map,
) -> *const c_char {
    if unsafe { pa_channel_map_valid(map) } == 0 {
        return ptr::null();
    }
    let map = unsafe { &*map };
    match &map.map[..map.channels as usize] {
        [0] => c"Mono".as_ptr(),
        [1, 2] => c"Stereo".as_ptr(),
        [1, 2, 5, 6] => c"Surround 4.0".as_ptr(),
        [1, 2, 5, 6, 3] => c"Surround 5.0".as_ptr(),
        [1, 2, 5, 6, 3, 7] => c"Surround 5.1".as_ptr(),
        [1, 2, 5, 6, 3, 7, 10, 11] => c"Surround 7.1".as_ptr(),
        _ => ptr::null(),
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_compatible_with_channel_map")]
pub unsafe extern "sysv64" fn pa_cvolume_compatible_with_channel_map(
    v: *const pa_cvolume,
    m: *const pa_channel_map,
) -> c_int {
    i32::from(unsafe {
        pa_cvolume_valid(v) != 0 && pa_channel_map_valid(m) != 0 && (*v).channels == (*m).channels
    })
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_equal")]
pub unsafe extern "sysv64" fn pa_cvolume_equal(
    a: *const pa_cvolume,
    b: *const pa_cvolume,
) -> c_int {
    if unsafe { pa_cvolume_valid(a) == 0 || pa_cvolume_valid(b) == 0 } {
        return 0;
    }
    let (a, b) = unsafe { (&*a, &*b) };
    i32::from(
        a.channels == b.channels
            && a.values[..a.channels as usize] == b.values[..b.channels as usize],
    )
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_max")]
pub unsafe extern "sysv64" fn pa_cvolume_max(v: *const pa_cvolume) -> u32 {
    if unsafe { pa_cvolume_valid(v) } == 0 {
        return 0;
    }
    unsafe { &(&(*v).values)[..(*v).channels as usize] }
        .iter()
        .copied()
        .max()
        .unwrap_or(0)
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_scale")]
pub unsafe extern "sysv64" fn pa_cvolume_scale(
    v: *mut pa_cvolume,
    maximum: u32,
) -> *mut pa_cvolume {
    if unsafe { pa_cvolume_valid(v) } == 0 || maximum > MAX {
        return ptr::null_mut();
    }
    let old = unsafe { pa_cvolume_max(v) };
    for value in unsafe { &mut (&mut (*v).values)[..(*v).channels as usize] } {
        *value = if old == 0 {
            maximum
        } else {
            ((*value as u64 * maximum as u64) / old as u64).min(MAX as u64) as u32
        };
    }
    v
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_get_position")]
pub unsafe extern "sysv64" fn pa_cvolume_get_position(
    v: *const pa_cvolume,
    m: *const pa_channel_map,
    position: c_int,
) -> u32 {
    if unsafe { pa_cvolume_compatible_with_channel_map(v, m) } == 0 {
        return 0;
    }
    let (v, m) = unsafe { (&*v, &*m) };
    (0..v.channels as usize)
        .filter(|i| m.map[*i] == position)
        .map(|i| v.values[i])
        .max()
        .unwrap_or(0)
}
unsafe fn balance(
    v: *const pa_cvolume,
    m: *const pa_channel_map,
    negative: fn(i32) -> bool,
    positive: fn(i32) -> bool,
) -> f32 {
    if unsafe {
        pa_cvolume_compatible_with_channel_map(v, m) == 0 || can_axis(m, negative, positive) == 0
    } {
        return 0.;
    }
    let (v, m) = unsafe { (&*v, &*m) };
    let average = |test: fn(i32) -> bool| {
        let mut sum = 0u64;
        let mut count = 0u64;
        for i in 0..v.channels as usize {
            if test(m.map[i]) {
                sum += v.values[i] as u64;
                count += 1;
            }
        }
        (sum / count.max(1)) as f32
    };
    let (a, b) = (average(negative), average(positive));
    if a == b {
        0.
    } else if a > b {
        b / a - 1.
    } else {
        1. - a / b
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_get_balance")]
pub unsafe extern "sysv64" fn pa_cvolume_get_balance(
    v: *const pa_cvolume,
    m: *const pa_channel_map,
) -> f32 {
    unsafe { balance(v, m, left, right) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_cvolume_get_fade")]
pub unsafe extern "sysv64" fn pa_cvolume_get_fade(
    v: *const pa_cvolume,
    m: *const pa_channel_map,
) -> f32 {
    unsafe { balance(v, m, rear, front) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_sw_volume_to_dB")]
pub extern "sysv64" fn pa_sw_volume_to_dB(v: u32) -> f64 {
    if v == 0 {
        f64::NEG_INFINITY
    } else {
        60. * (v.min(MAX) as f64 / NORM as f64).log10()
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_sw_volume_from_dB")]
pub extern "sysv64" fn pa_sw_volume_from_dB(db: f64) -> u32 {
    (10f64.powf(db / 60.) * NORM as f64)
        .round()
        .clamp(0., MAX as f64) as u32
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_timeval_store")]
pub unsafe extern "sysv64" fn pa_timeval_store(
    tv: *mut mainloop::Timeval,
    usec: u64,
) -> *mut mainloop::Timeval {
    if !tv.is_null() {
        unsafe {
            *tv = mainloop::Timeval {
                tv_sec: (usec / 1_000_000) as i64,
                tv_usec: (usec % 1_000_000) as i64,
            };
        }
    }
    tv
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_timeval_cmp")]
pub unsafe extern "sysv64" fn pa_timeval_cmp(
    a: *const mainloop::Timeval,
    b: *const mainloop::Timeval,
) -> c_int {
    let (a, b) = unsafe { (&*a, &*b) };
    match (a.tv_sec, a.tv_usec).cmp(&(b.tv_sec, b.tv_usec)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_timeval_diff")]
pub unsafe extern "sysv64" fn pa_timeval_diff(
    a: *const mainloop::Timeval,
    b: *const mainloop::Timeval,
) -> u64 {
    let (a, b) = unsafe { (&*a, &*b) };
    ((a.tv_sec as i128 - b.tv_sec as i128) * 1_000_000 + a.tv_usec as i128 - b.tv_usec as i128)
        .unsigned_abs()
        .min(u64::MAX as u128) as u64
}
