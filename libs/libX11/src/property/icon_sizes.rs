//! WM_ICON_SIZE entries are six signed CARD32 fields in each LP64 property row.
use crate::{Display, Window};
use core::{ffi::c_int, ptr};
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XIconSize {
    min_width: c_int,
    min_height: c_int,
    max_width: c_int,
    max_height: c_int,
    width_inc: c_int,
    height_inc: c_int,
}
#[unsafe(export_name = "kinakaze_engine_libX11_XAllocIconSize")]
pub unsafe extern "sysv64" fn XAllocIconSize() -> *mut XIconSize {
    unsafe { libc::kinakaze_abi_calloc(1, core::mem::size_of::<XIconSize>()).cast() }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetIconSizes")]
pub unsafe extern "sysv64" fn XSetIconSizes(
    d: *mut Display,
    w: Window,
    list: *const XIconSize,
    count: c_int,
) -> c_int {
    let Some(words) = count.checked_mul(6).filter(|n| *n >= 0) else {
        unsafe {
            crate::errors::report(d, 2, 18, 0, 38);
        }
        return 0;
    };
    let mut data = Vec::<u64>::new();
    if data.try_reserve_exact(words as usize).is_err() {
        return 0;
    }
    for i in 0..words as usize {
        data.push(unsafe { *list.cast::<i32>().add(i) } as u64);
    }
    unsafe { crate::XChangeProperty(d, w, 38, 38, 32, 0, data.as_ptr().cast(), words) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XGetIconSizes")]
pub unsafe extern "sysv64" fn XGetIconSizes(
    d: *mut Display,
    w: Window,
    list: *mut *mut XIconSize,
    count: *mut c_int,
) -> c_int {
    unsafe {
        *list = ptr::null_mut();
        *count = 0;
    }
    let (mut kind, mut format, mut n, mut after, mut bytes) = (0, 0, 0, 0, ptr::null_mut());
    if unsafe {
        crate::XGetWindowProperty(
            d,
            w,
            38,
            0,
            i32::MAX as i64,
            0,
            38,
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
    if kind != 38 || format != 32 || n == 0 || n % 6 != 0 || n / 6 > i32::MAX as u64 {
        unsafe {
            kinakaze_alloc::guest::free(bytes);
        }
        return 0;
    }
    let output = unsafe { kinakaze_alloc::guest::malloc(n as usize * 4) }.cast::<XIconSize>();
    if !output.is_null() {
        for i in 0..n as usize {
            unsafe {
                *output.cast::<i32>().add(i) = *bytes.cast::<u64>().add(i) as i32;
            }
        }
        unsafe {
            *list = output;
            *count = (n / 6) as c_int;
        }
    }
    unsafe {
        kinakaze_alloc::guest::free(bytes);
    }
    c_int::from(!output.is_null())
}
