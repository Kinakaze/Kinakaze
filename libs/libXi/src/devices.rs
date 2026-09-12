//! The adapter has one virtual master pair and one raw pointer/keyboard pair.
//! No touch or barrier capability is advertised by this XI 2.0 seat.
use super::*;
use core::ffi::c_char;
use core::{mem, ptr};
use kinakaze_alloc::guest;

// XI1 discovery clients (including CEF) still open the core devices to query
// properties. Their handles contain guest memory only; XI2 owns event delivery.
#[unsafe(export_name = "kinakaze_engine_libXi_XOpenDevice")]
pub unsafe extern "sysv64" fn XOpenDevice(d: *mut Display, device: usize) -> *mut c_void {
    unsafe { super::legacy::open(d, device) }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XCloseDevice")]
pub unsafe extern "sysv64" fn XCloseDevice(d: *mut Display, device: *mut c_void) -> c_int {
    unsafe { super::legacy::close(d, device) }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XSetDeviceMode")]
pub unsafe extern "sysv64" fn XSetDeviceMode(
    d: *mut Display,
    _device: *mut c_void,
    _mode: c_int,
) -> c_int {
    unsafe { kinakaze_libX11::errors::report(d, 128, 128, 5, 0) }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XSetDeviceButtonMapping")]
pub unsafe extern "sysv64" fn XSetDeviceButtonMapping(
    d: *mut Display,
    _device: *mut c_void,
    _map: *mut u8,
    _count: c_int,
) -> c_int {
    unsafe { kinakaze_libX11::errors::report(d, 128, 128, 29, 0) }
}

#[repr(C)]
pub struct DeviceInfo {
    device: c_int,
    name: *mut c_char,
    use_: c_int,
    attachment: c_int,
    enabled: c_int,
    classes_count: c_int,
    classes: *mut *mut c_void,
}
#[repr(C)]
struct KeyClass {
    kind: c_int,
    source: c_int,
    count: c_int,
    keys: *mut c_int,
}
#[repr(C)]
struct Valuator {
    kind: c_int,
    source: c_int,
    number: c_int,
    label: usize,
    min: f64,
    max: f64,
    value: f64,
    resolution: c_int,
    mode: c_int,
}
#[repr(C)]
struct ButtonClass {
    kind: c_int,
    source: c_int,
    count: c_int,
    labels: *mut usize,
    mask_len: c_int,
    mask: *mut u8,
}
#[repr(C)]
struct Classes {
    pointers: [*mut c_void; 3],
    axes: [Valuator; 2],
    buttons: ButtonClass,
    labels: [usize; 9],
    mask: [u8; 2],
    key: KeyClass,
    keys: [c_int; 248],
    name: [u8; 32],
}

#[unsafe(export_name = "kinakaze_engine_libXi_XIQueryDevice")]
pub unsafe extern "sysv64" fn XIQueryDevice(
    display: *mut Display,
    device: c_int,
    count: *mut c_int,
) -> *mut DeviceInfo {
    if count.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    if !(0..=5).contains(&device) {
        unsafe { kinakaze_libX11::errors::report(display, 128, 128, 48, device as usize) };
        return ptr::null_mut();
    }
    let ids: [c_int; 4] = [2, 3, 4, 5];
    let chosen: Vec<_> = ids
        .into_iter()
        .filter(|id| device == 0 || device == 1 && *id < 4 || device == *id)
        .collect();
    let n = chosen.len();
    let bytes = n * (mem::size_of::<DeviceInfo>() + mem::size_of::<Classes>());
    let result = unsafe { guest::malloc(bytes) }.cast::<DeviceInfo>();
    if result.is_null() {
        return result;
    }
    unsafe {
        ptr::write_bytes(result.cast::<u8>(), 0, bytes);
    }
    let storage = unsafe { result.add(n).cast::<Classes>() };
    for (index, id) in chosen.into_iter().enumerate() {
        unsafe {
            let block = &mut *storage.add(index);
            let pointer = id % 2 == 0;
            let source = if pointer { 4 } else { 5 };
            let name: &[u8] = match id {
                2 => b"Virtual core pointer",
                3 => b"Virtual core keyboard",
                4 => b"Windows raw pointer",
                _ => b"Windows raw keyboard",
            };
            block.name[..name.len()].copy_from_slice(name);
            let classes = if pointer {
                for axis in 0..2 {
                    block.axes[axis] = Valuator {
                        kind: 2,
                        source,
                        number: axis as i32,
                        label: 0,
                        min: 0.0,
                        max: 0.0,
                        value: 0.0,
                        resolution: 0,
                        mode: 0,
                    };
                    block.pointers[axis] = (&raw mut block.axes[axis]).cast();
                }
                block.buttons = ButtonClass {
                    kind: 1,
                    source,
                    count: 9,
                    labels: block.labels.as_mut_ptr(),
                    mask_len: 2,
                    mask: block.mask.as_mut_ptr(),
                };
                block.pointers[2] = (&raw mut block.buttons).cast();
                3
            } else {
                for (offset, key) in block.keys.iter_mut().enumerate() {
                    *key = offset as i32 + 8;
                }
                block.key = KeyClass {
                    kind: 0,
                    source,
                    count: 248,
                    keys: block.keys.as_mut_ptr(),
                };
                block.pointers[0] = (&raw mut block.key).cast();
                1
            };
            result.add(index).write(DeviceInfo {
                device: id,
                name: block.name.as_mut_ptr().cast(),
                use_: id - 1,
                attachment: match id {
                    2 => 3,
                    3 => 2,
                    4 => 2,
                    _ => 3,
                },
                enabled: 1,
                classes_count: classes,
                classes: block.pointers.as_mut_ptr(),
            });
        }
    }
    unsafe {
        *count = n as i32;
    }
    result
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIFreeDeviceInfo")]
pub unsafe extern "sysv64" fn XIFreeDeviceInfo(info: *mut DeviceInfo) {
    unsafe { guest::free(info.cast()) };
}

#[unsafe(export_name = "kinakaze_engine_libXi_XIGetClientPointer")]
pub unsafe extern "sysv64" fn XIGetClientPointer(
    _display: *mut Display,
    _window: Window,
    device: *mut c_int,
) -> Bool {
    if device.is_null() {
        return 0;
    }
    unsafe {
        *device = 2;
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libXi_XISetClientPointer")]
pub unsafe extern "sysv64" fn XISetClientPointer(
    display: *mut Display,
    _window: Window,
    device: c_int,
) -> Status {
    if device == 2 {
        0
    } else {
        unsafe { kinakaze_libX11::errors::report(display, 128, 128, 44, device as usize) }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIWarpPointer")]
pub unsafe extern "sysv64" fn XIWarpPointer(
    display: *mut Display,
    device: c_int,
    source: Window,
    destination: Window,
    x: f64,
    y: f64,
    width: c_uint,
    height: c_uint,
    dx: f64,
    dy: f64,
) -> Bool {
    if device != 2 && device != 4 {
        unsafe { kinakaze_libX11::errors::report(display, 128, 128, 41, device as usize) };
        return 0;
    }
    if [x, y, dx, dy]
        .iter()
        .any(|v| !v.is_finite() || *v < i32::MIN as f64 || *v > i32::MAX as f64)
    {
        unsafe { kinakaze_libX11::errors::report(display, 2, 128, 41, 0) };
        return 0;
    }
    unsafe {
        kinakaze_libX11::wm::XWarpPointer(
            display,
            source,
            destination,
            x as i32,
            y as i32,
            width,
            height,
            dx as i32,
            dy as i32,
        )
    }
}
#[repr(C)]
pub struct GrabModifiers {
    modifiers: c_int,
    status: c_int,
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIGrabTouchBegin")]
pub unsafe extern "sysv64" fn XIGrabTouchBegin(
    display: *mut Display,
    device: c_int,
    _window: Window,
    _owner: c_int,
    _mask: *mut XIEventMask,
    _count: c_int,
    _modifiers: *mut GrabModifiers,
) -> c_int {
    unsafe {
        super::grabs::touch_select(
            display,
            device,
            _window,
            _owner,
            _mask,
            _count,
            _modifiers.cast(),
            true,
        )
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIUngrabTouchBegin")]
pub unsafe extern "sysv64" fn XIUngrabTouchBegin(
    display: *mut Display,
    device: c_int,
    _window: Window,
    _count: c_int,
    _modifiers: *mut GrabModifiers,
) -> c_int {
    unsafe {
        super::grabs::touch_select(
            display,
            device,
            _window,
            0,
            core::ptr::null(),
            _count,
            _modifiers.cast(),
            false,
        )
    }
}
#[repr(C)]
pub struct BarrierRelease {
    device: c_int,
    barrier: usize,
    event: c_uint,
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIBarrierReleasePointers")]
pub unsafe extern "sysv64" fn XIBarrierReleasePointers(
    display: *mut Display,
    barriers: *mut BarrierRelease,
    count: c_int,
) {
    if count < 0 || count > 0 && barriers.is_null() {
        unsafe { kinakaze_libX11::errors::report(display, 2, 128, 61, 0) };
        return;
    }
    for i in 0..count as usize {
        let item = unsafe { &*barriers.add(i) };
        if item.device != 2 {
            unsafe { kinakaze_libX11::errors::report(display, 128, 128, 61, item.device as usize) };
        } else if kinakaze_libdisplay::ui::barriers::release(item.barrier as u32, item.event)
            .is_err()
        {
            unsafe { kinakaze_libX11::errors::report(display, 157, 128, 61, item.barrier) };
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XIBarrierReleasePointer")]
pub unsafe extern "sysv64" fn XIBarrierReleasePointer(
    display: *mut Display,
    device: c_int,
    barrier: usize,
    event: c_uint,
) {
    let mut release = BarrierRelease {
        device,
        barrier,
        event,
    };
    unsafe { XIBarrierReleasePointers(display, &raw mut release, 1) };
}
