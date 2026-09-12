//! XI1 discovery/property handles for the hosted core pair. No XI1 extension
//! event classes are advertised: core events and XI2 own input delivery.
use super::*;
use core::ptr;
use kinakaze_alloc::guest;

#[repr(C)]
struct DeviceInfo {
    id: usize,
    kind: usize,
    name: *mut i8,
    classes: i32,
    use_: i32,
    info: *mut c_void,
}

#[repr(C)]
struct Device {
    id: usize,
    classes: i32,
    info: *mut c_void,
}
unsafe fn id(device: *mut c_void) -> Option<i32> {
    let device = unsafe { device.cast::<Device>().as_ref() }?;
    (2..=5).contains(&device.id).then_some(device.id as i32)
}
pub(crate) unsafe fn open(d: *mut Display, device: usize) -> *mut c_void {
    unsafe {
        kinakaze_libX11::issue_request(d);
    }
    if !(2..=5).contains(&device) {
        unsafe {
            kinakaze_libX11::errors::report(d, 128, 128, 3, device);
        }
        return ptr::null_mut();
    }
    let handle = unsafe { guest::malloc(core::mem::size_of::<Device>()) }.cast::<Device>();
    if !handle.is_null() {
        unsafe {
            handle.write(Device {
                id: device,
                classes: 0,
                info: ptr::null_mut(),
            });
        }
    }
    handle.cast()
}
pub(crate) unsafe fn close(d: *mut Display, device: *mut c_void) -> i32 {
    if unsafe { id(device) }.is_none() {
        return unsafe { bad_device(d, 4) };
    }
    unsafe {
        guest::free(device.cast());
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libXi_XListInputDevices")]
pub unsafe extern "sysv64" fn XListInputDevices(d: *mut Display, count: *mut i32) -> *mut c_void {
    if count.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    let names = [c"Virtual core pointer", c"Virtual core keyboard"];
    let bytes = 2 * core::mem::size_of::<DeviceInfo>()
        + names
            .iter()
            .map(|n| n.to_bytes_with_nul().len())
            .sum::<usize>();
    let result = unsafe { guest::malloc(bytes) }.cast::<DeviceInfo>();
    if result.is_null() {
        return ptr::null_mut();
    }
    let mut text = unsafe { result.add(2).cast::<u8>() };
    for (index, name) in names.into_iter().enumerate() {
        let bytes = name.to_bytes_with_nul();
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), text, bytes.len());
            result.add(index).write(DeviceInfo {
                id: index + 2,
                kind: 0,
                name: text.cast(),
                classes: 0,
                use_: index as i32,
                info: ptr::null_mut(),
            });
            text = text.add(bytes.len());
        }
    }
    unsafe {
        kinakaze_libX11::issue_request(d);
        *count = 2;
    }
    result.cast()
}

#[unsafe(export_name = "kinakaze_engine_libXi_XFreeDeviceList")]
pub unsafe extern "sysv64" fn XFreeDeviceList(list: *mut c_void) {
    unsafe {
        guest::free(list.cast());
    }
}

unsafe fn bad_device(d: *mut Display, minor: u8) -> i32 {
    unsafe { kinakaze_libX11::errors::report(d, 128, 128, minor, 0) }
}

#[unsafe(export_name = "kinakaze_engine_libXi_XGrabDevice")]
pub unsafe extern "sysv64" fn XGrabDevice(
    d: *mut Display,
    device: *mut c_void,
    window: usize,
    owner: i32,
    count: i32,
    _classes: *mut usize,
    mode: i32,
    other_mode: i32,
    time: usize,
) -> i32 {
    let Some(device) = (unsafe { id(device) }) else {
        return unsafe { bad_device(d, 13) };
    };
    if count != 0 {
        return unsafe {
            kinakaze_libX11::errors::report(d, if count < 0 { 2 } else { 132 }, 128, 13, 0)
        };
    }
    let device = if device % 2 == 0 { 2 } else { 3 };
    let mut mask = XIEventMask {
        deviceid: device,
        mask_len: 0,
        mask: ptr::null_mut(),
    };
    unsafe {
        super::grabs::XIGrabDevice(
            d, device, window, time, 0, mode, other_mode, owner, &mut mask,
        )
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XUngrabDevice")]
pub unsafe extern "sysv64" fn XUngrabDevice(
    d: *mut Display,
    device: *mut c_void,
    time: usize,
) -> i32 {
    let Some(device) = (unsafe { id(device) }) else {
        return unsafe { bad_device(d, 14) };
    };
    unsafe { super::grabs::XIUngrabDevice(d, if device % 2 == 0 { 2 } else { 3 }, time) }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XSelectExtensionEvent")]
pub unsafe extern "sysv64" fn XSelectExtensionEvent(
    d: *mut Display,
    window: usize,
    _classes: *mut usize,
    count: i32,
) -> i32 {
    if count != 0 {
        return unsafe {
            kinakaze_libX11::errors::report(d, if count < 0 { 2 } else { 132 }, 128, 6, 0)
        };
    }
    let mut attributes = unsafe { core::mem::zeroed() };
    if unsafe { kinakaze_libX11::XGetWindowAttributes(d, window, &mut attributes) } == 0 {
        return 3;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXi_XGetDeviceMotionEvents")]
pub unsafe extern "sysv64" fn XGetDeviceMotionEvents(
    d: *mut Display,
    device: *mut c_void,
    _start: usize,
    _stop: usize,
    count: *mut i32,
    mode: *mut i32,
    axes: *mut i32,
) -> *mut c_void {
    unsafe {
        if !count.is_null() {
            *count = 0;
        }
        if !mode.is_null() {
            *mode = 0;
        }
        if !axes.is_null() {
            *axes = 0;
        }
        if id(device).is_none() {
            bad_device(d, 10);
        }
    }
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libXi_XFreeDeviceMotionEvents")]
pub unsafe extern "sysv64" fn XFreeDeviceMotionEvents(events: *mut c_void) {
    unsafe {
        guest::free(events.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XQueryDeviceState")]
pub unsafe extern "sysv64" fn XQueryDeviceState(
    d: *mut Display,
    device: *mut c_void,
) -> *mut c_void {
    let Some(device) = (unsafe { id(device) }) else {
        unsafe {
            bad_device(d, 30);
        }
        return ptr::null_mut();
    };
    unsafe { open(d, device as usize) }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XFreeDeviceState")]
pub unsafe extern "sysv64" fn XFreeDeviceState(state: *mut c_void) {
    unsafe {
        guest::free(state.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libXi_XGetDeviceProperty")]
pub unsafe extern "sysv64" fn XGetDeviceProperty(
    d: *mut Display,
    device: *mut c_void,
    property: usize,
    offset: i64,
    length: i64,
    delete: i32,
    requested: usize,
    kind: *mut usize,
    format: *mut i32,
    count: *mut usize,
    after: *mut usize,
    data: *mut *mut u8,
) -> i32 {
    unsafe {
        if !kind.is_null() {
            *kind = 0;
        }
        if !format.is_null() {
            *format = 0;
        }
        if !count.is_null() {
            *count = 0;
        }
        if !after.is_null() {
            *after = 0;
        }
        if !data.is_null() {
            *data = ptr::null_mut();
        }
        let Some(device) = id(device) else {
            return bad_device(d, 39);
        };
        if kind.is_null()
            || format.is_null()
            || count.is_null()
            || after.is_null()
            || data.is_null()
        {
            return 2;
        }
        super::query::XIGetProperty(
            d,
            device,
            property,
            offset,
            length,
            delete,
            requested,
            kind,
            format,
            count.cast(),
            after.cast(),
            data,
        )
    }
}
