//! On-demand XI queries over the same native seat used by core X11.
use super::*;
use core::ptr;
#[repr(C)]
pub struct ButtonState {
    length: c_int,
    mask: *mut u8,
}
#[repr(C)]
pub struct ModifierState {
    base: c_int,
    latched: c_int,
    locked: c_int,
    effective: c_int,
}
fn pointer(device: c_int) -> bool {
    device == 2 || device == 4
}

#[unsafe(export_name = "kinakaze_engine_libXi_XIChangeProperty")]
pub unsafe extern "sysv64" fn XIChangeProperty(
    d: *mut Display,
    device: c_int,
    property: usize,
    _type: usize,
    _format: c_int,
    _mode: c_int,
    _data: *const u8,
    _count: c_int,
) {
    // Device Enabled is read-only for the hosted seat; changing arbitrary XI
    // properties must not claim to reconfigure the Windows input device.
    unsafe {
        kinakaze_libX11::errors::report(
            d,
            if (2..=5).contains(&device) { 10 } else { 128 },
            128,
            57,
            property,
        );
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIDefineCursor")]
pub unsafe extern "sysv64" fn XIDefineCursor(
    d: *mut Display,
    device: c_int,
    w: Window,
    cursor: usize,
) -> Status {
    if !pointer(device) {
        return 128;
    }
    if unsafe { kinakaze_libX11::XDefineCursor(d, w, cursor) } != 0 {
        0
    } else {
        3
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIUndefineCursor")]
pub unsafe extern "sysv64" fn XIUndefineCursor(
    d: *mut Display,
    device: c_int,
    w: Window,
) -> Status {
    if !pointer(device) {
        return 128;
    }
    if unsafe { kinakaze_libX11::XUndefineCursor(d, w) } != 0 {
        0
    } else {
        3
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIQueryPointer")]
pub unsafe extern "sysv64" fn XIQueryPointer(
    d: *mut Display,
    device: c_int,
    w: Window,
    root: *mut Window,
    child: *mut Window,
    rx: *mut f64,
    ry: *mut f64,
    wx: *mut f64,
    wy: *mut f64,
    buttons: *mut ButtonState,
    mods: *mut ModifierState,
    group: *mut ModifierState,
) -> Bool {
    if !pointer(device) {
        unsafe {
            kinakaze_libX11::errors::report(d, 128, 128, 40, device as usize);
        }
        return 0;
    }
    let (mut x, mut y, mut cx, mut cy, mut mask) = (0, 0, 0, 0, 0);
    let ok = unsafe {
        kinakaze_libX11::XQueryPointer(
            d, w, root, child, &mut x, &mut y, &mut cx, &mut cy, &mut mask,
        )
    };
    if ok == 0 {
        return 0;
    }
    let data = unsafe { kinakaze_alloc::guest::malloc(4) };
    if data.is_null() {
        return 0;
    }
    unsafe {
        *rx = x as f64;
        *ry = y as f64;
        *wx = cx as f64;
        *wy = cy as f64;
        ptr::write_bytes(data, 0, 4);
        *data = ((mask >> 8) as u8 & 31) << 1;
        buttons.write(ButtonState {
            length: 4,
            mask: data,
        });
        mods.write(ModifierState {
            base: (mask & 0xcd) as i32,
            latched: 0,
            locked: (mask & 0x32) as i32,
            effective: (mask & 255) as i32,
        });
        group.write(ModifierState {
            base: 0,
            latched: 0,
            locked: 0,
            effective: 0,
        });
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIGetProperty")]
pub unsafe extern "sysv64" fn XIGetProperty(
    d: *mut Display,
    device: c_int,
    property: usize,
    offset: i64,
    length: i64,
    _delete: Bool,
    requested: usize,
    kind: *mut usize,
    format: *mut c_int,
    count: *mut u64,
    after: *mut u64,
    output: *mut *mut u8,
) -> Status {
    unsafe {
        *kind = 0;
        *format = 0;
        *count = 0;
        *after = 0;
        *output = ptr::null_mut();
    }
    if !(2..=5).contains(&device) {
        return 128;
    }
    if offset < 0 || length < 0 {
        return 2;
    }
    let enabled = unsafe { kinakaze_libX11::XInternAtom(d, c"Device Enabled".as_ptr(), 0) };
    if property != enabled {
        return 0;
    }
    unsafe {
        *kind = 19;
        *format = 8;
    }
    if requested != 0 && requested != 19 {
        unsafe {
            *after = 1;
        }
        return 0;
    }
    if offset != 0 {
        return 2;
    }
    let bytes = unsafe { kinakaze_alloc::guest::malloc(2) };
    if bytes.is_null() {
        return 11;
    }
    unsafe {
        *bytes = if length == 0 { 0 } else { 1 };
        *bytes.add(1) = 0;
        *output = bytes;
        *count = u64::from(length != 0);
        *after = u64::from(length == 0);
    }
    0
}
