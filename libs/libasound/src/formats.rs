//! ALSA format metadata is independent of the host device's supported formats.
use super::*;

#[derive(Clone, Copy)]
struct Format {
    width: i32,
    physical: i32,
    signed: i32,
    endian: i32,
    linear: bool,
}

fn format(value: i32) -> Option<Format> {
    let (width, physical, signed, endian, linear) = match value {
        0..=1 => (8, 8, i32::from(value == 0), -22, true),
        2..=13 => {
            let group = (value - 2) / 4;
            let width = [16, 24, 32][group as usize];
            (
                width,
                if width == 24 { 32 } else { width },
                i32::from((value - 2) % 4 < 2),
                value % 2,
                true,
            )
        }
        14..=17 => (
            if value < 16 { 32 } else { 64 },
            if value < 16 { 32 } else { 64 },
            -22,
            value % 2,
            false,
        ),
        18..=19 => (32, 32, -22, value % 2, false),
        20..=21 => (8, 8, -22, -22, false),
        22 => (4, 4, -22, -22, false),
        25..=28 => (20, 32, i32::from(value < 27), (value - 25) % 2, true),
        32..=43 => (
            24 - ((value - 32) / 4) * 2 - i32::from(value >= 36) * 2,
            24,
            i32::from((value - 32) % 4 < 2),
            value % 2,
            true,
        ),
        48 => (8, 8, -22, -22, false),
        49 | 51 => (16, 16, -22, i32::from(value == 51), false),
        50 | 52 => (32, 32, -22, i32::from(value == 52), false),
        _ => return None,
    };
    Some(Format {
        width,
        physical,
        signed,
        endian,
        linear,
    })
}

macro_rules! property {
    ($name:ident, $field:ident) => {
        #[unsafe(export_name = concat!("kinakaze_engine_libasound_", stringify!($name)))]
        pub extern "sysv64" fn $name(value: c_int) -> c_int {
            format(value).map_or(-22, |f| f.$field)
        }
    };
}
property!(snd_pcm_format_width, width);
property!(snd_pcm_format_physical_width, physical);
property!(snd_pcm_format_signed, signed);
property!(snd_pcm_format_big_endian, endian);

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_size")]
pub extern "sysv64" fn snd_pcm_format_size(value: c_int, samples: usize) -> i64 {
    let Some(format) = format(value) else {
        return -22;
    };
    samples
        .checked_mul(format.physical as usize)
        .map(|bits| bits / 8)
        .and_then(|bytes| i64::try_from(bytes).ok())
        .unwrap_or(-75)
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_linear")]
pub extern "sysv64" fn snd_pcm_format_linear(value: c_int) -> c_int {
    i32::from(format(value).is_some_and(|f| f.linear))
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_build_linear_format")]
pub extern "sysv64" fn snd_pcm_build_linear_format(
    width: c_int,
    physical: c_int,
    unsigned: c_int,
    big_endian: c_int,
) -> c_int {
    (0..=52)
        .find(|&value| {
            format(value).is_some_and(|f| {
                f.linear
                    && f.width == width
                    && f.physical == physical
                    && f.signed == i32::from(unsigned == 0)
                    && (width == 8 || f.endian == i32::from(big_endian != 0))
            })
        })
        .unwrap_or(-1)
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FormatMask {
    bits: [u32; 8],
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_mask_sizeof")]
pub extern "sysv64" fn snd_pcm_format_mask_sizeof() -> usize {
    core::mem::size_of::<FormatMask>()
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_mask_malloc")]
pub unsafe extern "sysv64" fn snd_pcm_format_mask_malloc(output: *mut *mut FormatMask) -> c_int {
    if output.is_null() {
        return -22;
    }
    unsafe { params::allocate_params(output, FormatMask::default()) }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_mask_free")]
pub unsafe extern "sysv64" fn snd_pcm_format_mask_free(mask: *mut FormatMask) {
    unsafe { kinakaze_alloc::guest::free(mask.cast()) };
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_format_mask_test")]
pub unsafe extern "sysv64" fn snd_pcm_format_mask_test(
    mask: *const FormatMask,
    value: c_int,
) -> c_int {
    if mask.is_null() || !(0..256).contains(&value) {
        return 0;
    }
    i32::from(unsafe { (*mask).bits[value as usize / 32] } & (1 << (value % 32)) != 0)
}

pub(super) fn supported(value: i32) -> bool {
    matches!(value, 0 | 1 | 2 | 6 | 10 | 14 | 32)
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_pcm_hw_params_get_format_mask")]
pub unsafe extern "sysv64" fn snd_pcm_hw_params_get_format_mask(
    hw: *const snd_pcm_hw_params_t,
    mask: *mut FormatMask,
) {
    if hw.is_null() || mask.is_null() {
        return;
    }
    let mut result = FormatMask::default();
    result.bits[0] = unsafe { (*hw).format_mask } as u32;
    result.bits[1] = (unsafe { (*hw).format_mask } >> 32) as u32;
    unsafe { *mask = result };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_format_round_trips_and_packed_widths() {
        for value in 0..=52 {
            if let Some(f) = format(value)
                && f.linear
            {
                assert_eq!(
                    snd_pcm_build_linear_format(
                        f.width,
                        f.physical,
                        1 - f.signed,
                        i32::from(f.endian == 1)
                    ),
                    value
                );
            }
        }
        assert_eq!(snd_pcm_format_width(6), 24);
        assert_eq!(snd_pcm_format_physical_width(6), 32);
        assert_eq!(snd_pcm_format_physical_width(32), 24);
        assert_eq!(snd_pcm_format_width(36), 20);
        assert_eq!(snd_pcm_format_width(40), 18);
        assert_eq!(snd_pcm_format_signed(14), -22);
        assert_eq!(snd_pcm_format_linear(14), 0);
        assert_eq!(snd_pcm_format_width(-1), -22);
    }
}
