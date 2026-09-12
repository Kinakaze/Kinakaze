//! Fixed TrueColor ramps; unsupported names fail without color substitution.
use crate::graphics::XColor;
use crate::{Bool, Colormap, Display, Status, c_ulong};
use core::ffi::{c_char, c_int, c_uint};
use std::ffi::CStr;

fn parse(s: &str) -> Option<[u16; 3]> {
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.is_ascii() || !matches!(hex.len(), 3 | 6 | 9 | 12) {
            return None;
        }
        let n = hex.len() / 3;
        let mut values = [0; 3];
        for (i, v) in values.iter_mut().enumerate() {
            *v = u16::from_str_radix(&hex[i * n..(i + 1) * n], 16).ok()? << (16 - n * 4);
        }
        return Some(values);
    }
    if let Some(rgb) = s.strip_prefix("rgb:") {
        let mut parts = rgb.split('/');
        let mut values = [0; 3];
        for v in &mut values {
            let part = parts.next()?;
            if !part.is_ascii() || !(1..=4).contains(&part.len()) {
                return None;
            }
            let n = u32::from(u16::from_str_radix(part, 16).ok()?);
            *v = (n * 65535 / ((1u32 << (part.len() * 4)) - 1)) as u16;
        }
        return parts.next().is_none().then_some(values);
    }
    // The built-in primary color names require no file lookup or allocation.
    for (name, value) in [
        ("black", [0, 0, 0]),
        ("white", [65535; 3]),
        ("red", [65535, 0, 0]),
        ("green", [0, 65535, 0]),
        ("blue", [0, 0, 65535]),
        ("yellow", [65535, 65535, 0]),
        ("cyan", [0, 65535, 65535]),
        ("magenta", [65535, 0, 65535]),
    ] {
        if s.eq_ignore_ascii_case(name) {
            return Some(value);
        }
    }
    None
}

fn check(display: *mut Display, map: Colormap, request: u8) -> bool {
    if super::valid(map) {
        return true;
    }
    unsafe {
        crate::errors::report(display, 12, request, 0, map);
    }
    false
}
fn quantize(color: &mut XColor) {
    let (r, g, b) = (color.red >> 8, color.green >> 8, color.blue >> 8);
    color.pixel = (u64::from(r) << 16) | (u64::from(g) << 8) | u64::from(b);
    color.red = r * 257;
    color.green = g * 257;
    color.blue = b * 257;
    color.flags = 7;
}

// ---------------------------------------------------------------------------
// Colormap & Color queries
// ---------------------------------------------------------------------------

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocColor")]
pub unsafe extern "sysv64" fn XAllocColor(
    _dpy: *mut Display,
    _colormap: Colormap,
    screen_in_out: *mut XColor,
) -> Status {
    if check(_dpy, _colormap, 84) && !screen_in_out.is_null() {
        quantize(unsafe { &mut *screen_in_out });
        1
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocNamedColor")]
pub unsafe extern "sysv64" fn XAllocNamedColor(
    _dpy: *mut Display,
    _colormap: Colormap,
    color_name: *const c_char,
    screen_def_return: *mut XColor,
    exact_def_return: *mut XColor,
) -> Status {
    if !check(_dpy, _colormap, 85) {
        return 0;
    }
    let mut c = XColor::default();
    if unsafe { XParseColor(_dpy, _colormap, color_name, &raw mut c) } == 0 {
        return 0;
    }
    let mut screen = c;
    quantize(&mut screen);
    c.pixel = screen.pixel;
    unsafe {
        *screen_def_return = screen;
        *exact_def_return = c;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocColorCells")]
pub unsafe extern "sysv64" fn XAllocColorCells(
    _dpy: *mut Display,
    _colormap: Colormap,
    _contig: Bool,
    _plane_masks_return: *mut c_ulong,
    _nplanes: c_uint,
    pixels_return: *mut c_ulong,
    npixels: c_uint,
) -> Status {
    let _ = (pixels_return, npixels);
    if check(_dpy, _colormap, 86) {
        unsafe {
            crate::errors::report(_dpy, 11, 86, 0, _colormap);
        }
    }
    0 // TrueColor cells are read-only.
}

#[unsafe(export_name = "kinakaze_engine_libX11_XAllocColorPlanes")]
pub unsafe extern "sysv64" fn XAllocColorPlanes(
    _dpy: *mut Display,
    _colormap: Colormap,
    _contig: Bool,
    pixels_return: *mut c_ulong,
    ncolors: c_int,
    _nreds: c_int,
    _ngreens: c_int,
    _nblues: c_int,
    _rmask: *mut c_ulong,
    _gmask: *mut c_ulong,
    _bmask: *mut c_ulong,
) -> Status {
    let _ = (pixels_return, ncolors);
    if check(_dpy, _colormap, 87) {
        unsafe {
            crate::errors::report(_dpy, 11, 87, 0, _colormap);
        }
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeColors")]
pub unsafe extern "sysv64" fn XFreeColors(
    _dpy: *mut Display,
    _colormap: Colormap,
    _pixels: *mut c_ulong,
    _npixels: c_int,
    _planes: c_ulong,
) -> c_int {
    if !check(_dpy, _colormap, 88) {
        return 0;
    }
    for i in 0.._npixels.max(0) as usize {
        let pixel = unsafe { *_pixels.add(i) } | _planes;
        if pixel > 0xffffff {
            unsafe {
                crate::errors::report(_dpy, 2, 88, 0, pixel as usize);
            }
            return 0;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryColor")]
pub unsafe extern "sysv64" fn XQueryColor(
    _dpy: *mut Display,
    _colormap: Colormap,
    color_in_out: *mut XColor,
) -> c_int {
    if !check(_dpy, _colormap, 91) {
        return 0;
    }
    if !color_in_out.is_null() {
        let p = unsafe { (*color_in_out).pixel };
        if p > 0xffffff {
            unsafe {
                crate::errors::report(_dpy, 2, 91, 0, p as usize);
            }
            return 0;
        }
        let r = (((p >> 16) & 0xff) as u16) * 257;
        let g = (((p >> 8) & 0xff) as u16) * 257;
        let b = ((p & 0xff) as u16) * 257;
        unsafe {
            (*color_in_out).red = r;
            (*color_in_out).green = g;
            (*color_in_out).blue = b;
            (*color_in_out).flags = 7;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XQueryColors")]
pub unsafe extern "sysv64" fn XQueryColors(
    _dpy: *mut Display,
    _colormap: Colormap,
    defs: *mut XColor,
    ncolors: c_int,
) -> c_int {
    if !check(_dpy, _colormap, 91) {
        return 0;
    }
    if defs.is_null() || ncolors <= 0 {
        return 1;
    }
    for i in 0..ncolors as usize {
        if unsafe { XQueryColor(_dpy, _colormap, defs.add(i)) } == 0 {
            return 0;
        }
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XLookupColor")]
pub unsafe extern "sysv64" fn XLookupColor(
    _dpy: *mut Display,
    colormap: Colormap,
    color_name: *const c_char,
    exact_def_return: *mut XColor,
    screen_def_return: *mut XColor,
) -> Status {
    unsafe {
        XAllocNamedColor(
            _dpy,
            colormap,
            color_name,
            screen_def_return,
            exact_def_return,
        )
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XParseColor")]
pub unsafe extern "sysv64" fn XParseColor(
    _dpy: *mut Display,
    _colormap: Colormap,
    spec: *const c_char,
    exact_def_return: *mut XColor,
) -> Status {
    if spec.is_null() || exact_def_return.is_null() {
        return 0;
    }
    let Ok(s) = unsafe { CStr::from_ptr(spec) }.to_str() else {
        return 0;
    };
    let Some([r, g, b]) = parse(s) else {
        return 0;
    };
    unsafe {
        (*exact_def_return).red = r;
        (*exact_def_return).green = g;
        (*exact_def_return).blue = b;
        (*exact_def_return).flags = 7;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XStoreColor")]
pub unsafe extern "sysv64" fn XStoreColor(
    _dpy: *mut Display,
    _colormap: Colormap,
    _color: *mut XColor,
) -> c_int {
    unsafe { XStoreColors(_dpy, _colormap, _color, 1) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XStoreColors")]
pub unsafe extern "sysv64" fn XStoreColors(
    _dpy: *mut Display,
    _colormap: Colormap,
    _color: *mut XColor,
    _ncolors: c_int,
) -> c_int {
    if !check(_dpy, _colormap, 89) {
        return 0;
    }
    if _ncolors == 0 {
        return 1;
    }
    unsafe {
        crate::errors::report(_dpy, 10, 89, 0, _colormap);
    }
    0 // No writable cells in the advertised TrueColor visual.
}
