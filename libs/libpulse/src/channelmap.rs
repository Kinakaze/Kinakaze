//! Pulse channel order definitions and bounded, allocation-free formatting.
use super::*;
const NAMES: [&[u8]; 51] = [
    b"mono\0",
    b"front-left\0",
    b"front-right\0",
    b"front-center\0",
    b"rear-center\0",
    b"rear-left\0",
    b"rear-right\0",
    b"lfe\0",
    b"front-left-of-center\0",
    b"front-right-of-center\0",
    b"side-left\0",
    b"side-right\0",
    b"aux0\0",
    b"aux1\0",
    b"aux2\0",
    b"aux3\0",
    b"aux4\0",
    b"aux5\0",
    b"aux6\0",
    b"aux7\0",
    b"aux8\0",
    b"aux9\0",
    b"aux10\0",
    b"aux11\0",
    b"aux12\0",
    b"aux13\0",
    b"aux14\0",
    b"aux15\0",
    b"aux16\0",
    b"aux17\0",
    b"aux18\0",
    b"aux19\0",
    b"aux20\0",
    b"aux21\0",
    b"aux22\0",
    b"aux23\0",
    b"aux24\0",
    b"aux25\0",
    b"aux26\0",
    b"aux27\0",
    b"aux28\0",
    b"aux29\0",
    b"aux30\0",
    b"aux31\0",
    b"top-center\0",
    b"top-front-center\0",
    b"top-front-left\0",
    b"top-front-right\0",
    b"top-rear-center\0",
    b"top-rear-left\0",
    b"top-rear-right\0",
];
fn valid(map: &pa_channel_map) -> bool {
    (1..=32).contains(&map.channels)
        && map.map[..map.channels as usize]
            .iter()
            .all(|p| (0..51).contains(p))
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_valid")]
pub unsafe extern "sysv64" fn pa_channel_map_valid(map: *const pa_channel_map) -> c_int {
    i32::from(!map.is_null() && valid(unsafe { &*map }))
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_init")]
pub unsafe extern "sysv64" fn pa_channel_map_init(map: *mut pa_channel_map) -> *mut pa_channel_map {
    if !map.is_null() {
        unsafe {
            *map = pa_channel_map {
                channels: 0,
                map: [-1; 32],
            };
        }
    }
    map
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_init_auto")]
pub unsafe extern "sysv64" fn pa_channel_map_init_auto(
    map: *mut pa_channel_map,
    channels: c_uint,
    definition: c_int,
) -> *mut pa_channel_map {
    if map.is_null() || !(1..=32).contains(&channels) || !(0..5).contains(&definition) {
        return ptr::null_mut();
    }
    unsafe {
        pa_channel_map_init(map);
    }
    let n = channels as usize;
    let order: &[i32] = match (definition, channels) {
        (2, _) => {
            unsafe {
                for i in 0..n {
                    (*map).map[i] = 12 + i as i32;
                }
                (*map).channels = channels as u8;
            }
            return map;
        }
        (_, 1) => &[0],
        (_, 2) => &[1, 2],
        (0, 3) => &[1, 2, 3],
        (0, 4) => &[1, 3, 2, 4],
        (0, 5) => &[1, 2, 3, 5, 6],
        (0, 6) => &[1, 8, 3, 2, 9, 4],
        (1, 4) => &[1, 2, 5, 6],
        (1, 5) => &[1, 2, 5, 6, 3],
        (1, 6) => &[1, 2, 5, 6, 3, 7],
        (1, 8) => &[1, 2, 5, 6, 3, 7, 10, 11],
        (3 | 4, 3) => &[1, 2, 3],
        (3 | 4, 4) => &[1, 2, 3, 7],
        (3, 6) => &[1, 2, 3, 7, 5, 6],
        (3, 8) => &[1, 2, 3, 7, 5, 6, 10, 11],
        (3, 9) => &[1, 2, 3, 7, 5, 6, 8, 9, 4],
        (3, 11) => &[1, 2, 3, 7, 5, 6, 8, 9, 4, 10, 11],
        (3, 12) => &[1, 2, 3, 7, 5, 6, 8, 9, 4, 10, 11, 44],
        (3, 15) => &[1, 2, 3, 7, 5, 6, 8, 9, 4, 10, 11, 44, 46, 45, 47],
        (3, 18) => &[
            1, 2, 3, 7, 5, 6, 8, 9, 4, 10, 11, 44, 46, 45, 47, 49, 48, 50,
        ],
        (4, 6) => &[1, 2, 3, 7, 10, 11],
        (4, 8) => &[1, 2, 3, 7, 10, 11, 5, 6],
        _ => return ptr::null_mut(),
    };
    unsafe {
        (*map).channels = channels as u8;
        (&mut (*map).map)[..n].copy_from_slice(order);
    }
    map
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_init_mono")]
pub unsafe extern "sysv64" fn pa_channel_map_init_mono(
    map: *mut pa_channel_map,
) -> *mut pa_channel_map {
    unsafe { pa_channel_map_init_auto(map, 1, 0) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_init_stereo")]
pub unsafe extern "sysv64" fn pa_channel_map_init_stereo(
    map: *mut pa_channel_map,
) -> *mut pa_channel_map {
    unsafe { pa_channel_map_init_auto(map, 2, 0) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_init_extend")]
pub unsafe extern "sysv64" fn pa_channel_map_init_extend(
    map: *mut pa_channel_map,
    channels: c_uint,
    definition: c_int,
) -> *mut pa_channel_map {
    if map.is_null() || !(1..=32).contains(&channels) || !(0..5).contains(&definition) {
        return ptr::null_mut();
    }
    for n in (1..=channels).rev() {
        if unsafe { pa_channel_map_init_auto(map, n, definition) }.is_null() {
            continue;
        }
        unsafe {
            for i in n..channels {
                (*map).map[i as usize] = 12 + (i - n) as i32;
            }
            (*map).channels = channels as u8;
        }
        return map;
    }
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_equal")]
pub unsafe extern "sysv64" fn pa_channel_map_equal(
    a: *const pa_channel_map,
    b: *const pa_channel_map,
) -> c_int {
    if unsafe { pa_channel_map_valid(a) == 0 || pa_channel_map_valid(b) == 0 } {
        return 0;
    }
    let (a, b) = unsafe { (&*a, &*b) };
    i32::from(
        a.channels == b.channels && a.map[..a.channels as usize] == b.map[..b.channels as usize],
    )
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_superset")]
pub unsafe extern "sysv64" fn pa_channel_map_superset(
    a: *const pa_channel_map,
    b: *const pa_channel_map,
) -> c_int {
    if unsafe { pa_channel_map_valid(a) == 0 || pa_channel_map_valid(b) == 0 } {
        return 0;
    }
    let (a, b) = unsafe { (&*a, &*b) };
    i32::from(
        b.map[..b.channels as usize]
            .iter()
            .all(|p| a.map[..a.channels as usize].contains(p)),
    )
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_position_to_string")]
pub unsafe extern "sysv64" fn pa_channel_position_to_string(position: c_int) -> *const c_char {
    NAMES
        .get(position as usize)
        .map_or(ptr::null(), |s| s.as_ptr().cast())
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_position_from_string")]
pub unsafe extern "sysv64" fn pa_channel_position_from_string(text: *const c_char) -> c_int {
    if text.is_null() {
        return -1;
    }
    position(unsafe { CStr::from_ptr(text) }.to_bytes())
}
fn position(bytes: &[u8]) -> i32 {
    match bytes {
        b"left" => 1,
        b"right" => 2,
        b"center" => 3,
        b"subwoofer" => 7,
        _ => NAMES
            .iter()
            .position(|s| &s[..s.len() - 1] == bytes)
            .map_or(-1, |i| i as i32),
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_snprint")]
pub unsafe extern "sysv64" fn pa_channel_map_snprint(
    output: *mut c_char,
    length: usize,
    map: *const pa_channel_map,
) -> *mut c_char {
    if output.is_null() || length == 0 {
        return output;
    }
    let mut written = 0;
    let mut append = |s: &[u8]| {
        let count = s.len().min(length - 1 - written);
        unsafe {
            ptr::copy_nonoverlapping(s.as_ptr(), output.add(written).cast(), count);
        }
        written += count;
    };
    if unsafe { pa_channel_map_valid(map) } == 0 {
        append(b"(invalid)");
    } else {
        let map = unsafe { &*map };
        for (i, p) in map.map[..map.channels as usize].iter().enumerate() {
            if i != 0 {
                append(b",");
            }
            let name = NAMES[*p as usize];
            append(&name[..name.len() - 1]);
        }
    }
    unsafe {
        *output.add(written) = 0;
    }
    output
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_channel_map_parse")]
pub unsafe extern "sysv64" fn pa_channel_map_parse(
    map: *mut pa_channel_map,
    text: *const c_char,
) -> *mut pa_channel_map {
    if map.is_null() || text.is_null() {
        return ptr::null_mut();
    }
    let text = unsafe { CStr::from_ptr(text) }.to_bytes();
    let standard = match text {
        b"stereo" => Some(2),
        b"surround-40" => Some(4),
        b"surround-50" => Some(5),
        b"surround-51" => Some(6),
        b"surround-71" => Some(8),
        _ => None,
    };
    if let Some(n) = standard {
        unsafe {
            *map = default_map(n);
        }
        return map;
    }
    let mut value = pa_channel_map {
        channels: 0,
        map: [-1; 32],
    };
    for (i, name) in text.split(|b| *b == b',').enumerate() {
        if i >= 32 {
            return ptr::null_mut();
        }
        let p = position(name);
        if p < 0 {
            return ptr::null_mut();
        }
        value.map[i] = p;
        value.channels += 1;
    }
    unsafe {
        *map = value;
    }
    map
}
