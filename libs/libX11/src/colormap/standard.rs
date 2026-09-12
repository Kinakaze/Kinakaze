//! ICCCM RGB_COLOR_MAP is ten CARD32 fields per map, expanded to LP64 by Xlib.
use crate::{Atom, Colormap, Display, Window};
use core::{ffi::c_int, ptr};
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct XStandardColormap {
    pub colormap: Colormap,
    pub red_max: u64,
    pub red_mult: u64,
    pub green_max: u64,
    pub green_mult: u64,
    pub blue_max: u64,
    pub blue_mult: u64,
    pub base_pixel: u64,
    pub visualid: usize,
    pub killid: usize,
}
#[unsafe(export_name = "kinakaze_engine_libX11_XAllocStandardColormap")]
pub unsafe extern "sysv64" fn XAllocStandardColormap() -> *mut XStandardColormap {
    unsafe { libc::kinakaze_abi_calloc(1, core::mem::size_of::<XStandardColormap>()).cast() }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetRGBColormaps")]
pub unsafe extern "sysv64" fn XSetRGBColormaps(
    d: *mut Display,
    w: Window,
    maps: *const XStandardColormap,
    count: c_int,
    property: Atom,
) {
    let Some(words) = count.checked_mul(10).filter(|n| *n >= 0) else {
        unsafe {
            crate::errors::report(d, 2, 18, 0, property);
        }
        return;
    };
    unsafe {
        crate::XChangeProperty(d, w, property, 24, 32, 0, maps.cast(), words);
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetRGBColormaps")]
pub unsafe extern "sysv64" fn XGetRGBColormaps(
    d: *mut Display,
    w: Window,
    maps: *mut *mut XStandardColormap,
    count: *mut c_int,
    property: Atom,
) -> c_int {
    unsafe {
        *maps = ptr::null_mut();
        *count = 0;
    }
    let (mut kind, mut format, mut n, mut after, mut bytes) = (0, 0, 0, 0, ptr::null_mut());
    if unsafe {
        crate::XGetWindowProperty(
            d,
            w,
            property,
            0,
            i32::MAX as i64,
            0,
            24,
            &mut kind,
            &mut format,
            &mut n,
            &mut after,
            &mut bytes,
        )
    } != 0
    {
        return 0;
    }
    let size = if n == 8 || n == 9 { n as usize } else { 10 };
    if kind != 24
        || format != 32
        || n == 0
        || n % size as u64 != 0
        || n / size as u64 > i32::MAX as u64
    {
        unsafe {
            kinakaze_alloc::guest::free(bytes);
        }
        return 0;
    }
    let entries = n as usize / size;
    let output =
        unsafe { libc::kinakaze_abi_calloc(entries, core::mem::size_of::<XStandardColormap>()) }
            .cast::<XStandardColormap>();
    if !output.is_null() {
        for i in 0..entries {
            // XGetWindowProperty sign-extends format32; all these fields are unsigned.
            for j in 0..size {
                unsafe {
                    output
                        .add(i)
                        .cast::<u64>()
                        .add(j)
                        .write(bytes.cast::<u64>().add(i * size + j).read() as u32 as u64);
                }
            }
            if size == 8 {
                unsafe {
                    (*output.add(i)).visualid = 1;
                }
            }
        }
        unsafe {
            *maps = output;
            *count = entries as c_int;
        }
    }
    unsafe {
        kinakaze_alloc::guest::free(bytes);
    }
    c_int::from(!output.is_null())
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetStandardColormap")]
pub unsafe extern "sysv64" fn XSetStandardColormap(
    d: *mut Display,
    w: Window,
    map: *const XStandardColormap,
    property: Atom,
) {
    unsafe {
        XSetRGBColormaps(d, w, map, 1, property);
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetStandardColormap")]
pub unsafe extern "sysv64" fn XGetStandardColormap(
    d: *mut Display,
    w: Window,
    map: *mut XStandardColormap,
    property: Atom,
) -> c_int {
    let (mut maps, mut count) = (ptr::null_mut(), 0);
    if unsafe { XGetRGBColormaps(d, w, &mut maps, &mut count, property) } == 0 {
        return 0;
    }
    unsafe {
        *map = *maps;
        kinakaze_alloc::guest::free(maps.cast());
    }
    1
}
