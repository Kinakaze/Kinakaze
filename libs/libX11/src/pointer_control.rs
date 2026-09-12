//! Core pointer control uses the native desktop's real acceleration settings.
//! X11 permits rounding the requested fraction; Windows exposes factors 1/2/4
//! and may normalize legacy level 2 to 1. Queries always read the native result.
//! XI2 raw motion continues to report unaccelerated hardware counts.
mod lifecycle;
use crate::{Bool, Display};
use core::ffi::c_int;
use std::sync::Mutex;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SPI_GETMOUSE, SPI_SETMOUSE, SystemParametersInfoW,
};

// Captured only on the first control request, not at module/process startup.
// Keep the server defaults distinct from later settings, including after fork.
static DEFAULTS: Mutex<Option<[i32; 3]>> = Mutex::new(None);

#[unsafe(export_name = "kinakaze_engine_libX11_XGetPointerMapping")]
pub unsafe extern "sysv64" fn XGetPointerMapping(
    _display: *mut Display,
    map: *mut u8,
    size: c_int,
) -> c_int {
    // The virtual master exposes the same nine logical buttons as XIQueryDevice:
    // left/middle/right, vertical/horizontal wheel, and two side buttons.
    for i in 0..size.clamp(0, 9) as usize {
        unsafe {
            *map.add(i) = (i + 1) as u8;
        }
    }
    9
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryBestCursor")]
pub unsafe extern "sysv64" fn XQueryBestCursor(
    display: *mut Display,
    drawable: usize,
    width: u32,
    height: u32,
    best_width: *mut u32,
    best_height: *mut u32,
) -> c_int {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, IsWindow, SM_CXCURSOR, SM_CYCURSOR,
    };
    if drawable != 1
        && unsafe { IsWindow(drawable as _) } == 0
        && crate::graphics::drawable_dimensions(drawable).is_none()
    {
        unsafe {
            crate::errors::report(display, 9, 97, 0, drawable);
        }
        return 0;
    }
    let (x, y) = unsafe { (GetSystemMetrics(SM_CXCURSOR), GetSystemMetrics(SM_CYCURSOR)) };
    if x <= 0 || y <= 0 {
        return 0;
    }
    unsafe {
        *best_width = width.min(x as u32);
        *best_height = height.min(y as u32);
    }
    1
}
fn read_native() -> Result<[i32; 3], u8> {
    let mut values = [0; 3];
    if unsafe { SystemParametersInfoW(SPI_GETMOUSE, 0, values.as_mut_ptr().cast(), 0) } == 0
        || values[0] < 0
        || values[1] < 0
        || !(0..=2).contains(&values[2])
    {
        return Err(17);
    }
    Ok(values)
}
fn configured(
    mut current: [i32; 3],
    defaults: [i32; 3],
    accel: bool,
    threshold: bool,
    numerator: i32,
    denominator: i32,
    distance: i32,
) -> Result<[i32; 3], u8> {
    if (accel && (numerator < -1 || denominator < -1 || denominator == 0))
        || (threshold && distance < -1)
    {
        return Err(2);
    }
    if accel {
        let n = i64::from(if numerator == -1 {
            1 << defaults[2]
        } else {
            numerator
        });
        let d = i64::from(if denominator == -1 { 1 } else { denominator });
        current[2] = (0..=2)
            .min_by_key(|level| (n - ((1_i64 << level) * d)).abs())
            .unwrap();
    }
    if threshold {
        if distance == -1 {
            current[0] = defaults[0];
            current[1] = defaults[1];
        } else {
            current[0] = distance;
            current[1] = distance;
        }
    }
    Ok(current)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XChangePointerControl")]
pub unsafe extern "sysv64" fn XChangePointerControl(
    display: *mut Display,
    do_accel: Bool,
    do_threshold: Bool,
    numerator: c_int,
    denominator: c_int,
    threshold: c_int,
) -> c_int {
    let result = (|| -> Result<(), u8> {
        // Invalid requests must not query or mutate host state.
        if (do_accel != 0 && (numerator < -1 || denominator < -1 || denominator == 0))
            || (do_threshold != 0 && threshold < -1)
        {
            return Err(2);
        }
        if do_accel == 0 && do_threshold == 0 {
            return Ok(());
        }
        let mut defaults = DEFAULTS.lock().unwrap();
        let current = read_native()?;
        let saved = *defaults.get_or_insert(current);
        let mut desired = configured(
            current,
            saved,
            do_accel != 0,
            do_threshold != 0,
            numerator,
            denominator,
            threshold,
        )?;
        if desired != current
            && unsafe { SystemParametersInfoW(SPI_SETMOUSE, 0, desired.as_mut_ptr().cast(), 0) }
                == 0
        {
            return Err(17);
        }
        Ok(())
    })();
    match result {
        Ok(()) => 1,
        Err(code) => {
            unsafe {
                crate::errors::report(display, code, 105, 0, 0);
            }
            0
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetPointerControl")]
pub unsafe extern "sysv64" fn XGetPointerControl(
    display: *mut Display,
    numerator: *mut c_int,
    denominator: *mut c_int,
    threshold: *mut c_int,
) -> c_int {
    let result = (|| {
        let mut defaults = DEFAULTS.lock().unwrap();
        let current = read_native()?;
        defaults.get_or_insert(current);
        Ok::<_, u8>(current)
    })();
    match result {
        Ok(values) => {
            unsafe {
                *numerator = 1 << values[2];
                *denominator = 1;
                *threshold = values[0];
            }
            1
        }
        Err(code) => {
            unsafe {
                crate::errors::report(display, code, 106, 0, 0);
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn control_preserves_masked_fields_rounds_and_restores_defaults() {
        let base = [6, 10, 1];
        assert_eq!(configured(base, base, true, true, 1, 1, 0), Ok([0, 0, 0]));
        assert_eq!(configured(base, base, false, false, -99, 0, -99), Ok(base));
        assert_eq!(
            configured(base, base, true, false, 7, 2, 99),
            Ok([6, 10, 2])
        );
        assert_eq!(
            configured([1, 1, 2], base, true, true, -1, -1, -1),
            Ok(base)
        );
        assert_eq!(configured(base, base, true, false, 2, 0, 0), Err(2));
        assert_eq!(configured(base, base, false, true, 0, 0, -2), Err(2));
    }
}
