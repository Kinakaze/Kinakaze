//! On-demand native monitor snapshots, allocated as one guest-owned block.
use super::*;
use kinakaze_libX11::Atom;
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;
#[repr(C)]
pub struct XRRMonitorInfo {
    pub name: Atom,
    pub primary: Bool,
    pub automatic: Bool,
    pub noutput: c_int,
    pub x: c_int,
    pub y: c_int,
    pub width: c_int,
    pub height: c_int,
    pub mwidth: c_int,
    pub mheight: c_int,
    pub outputs: *mut RROutput,
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRAllocateMonitor")]
pub unsafe extern "sysv64" fn XRRAllocateMonitor(
    _display: *mut Display,
    outputs: c_int,
) -> *mut XRRMonitorInfo {
    if outputs < 0 {
        return ptr::null_mut();
    }
    let Some(bytes) = (outputs as usize)
        .checked_mul(size_of::<RROutput>())
        .and_then(|n| n.checked_add(size_of::<XRRMonitorInfo>()))
    else {
        return ptr::null_mut();
    };
    let result = unsafe { zeroed(bytes) }.cast::<XRRMonitorInfo>();
    if !result.is_null() {
        unsafe {
            (*result).noutput = outputs;
            (*result).outputs = result.add(1).cast();
        }
    }
    result
}

// Monitor mutation needs a RandR server. The hosted display only exposes
// snapshots; match Xlib's unadvertised-extension failure path.
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRSetMonitor")]
pub unsafe extern "sysv64" fn XRRSetMonitor(
    d: *mut Display,
    window: Window,
    _monitor: *mut XRRMonitorInfo,
) {
    unsafe { error(d, 43, window) };
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRDeleteMonitor")]
pub unsafe extern "sysv64" fn XRRDeleteMonitor(d: *mut Display, window: Window, _name: Atom) {
    unsafe { error(d, 44, window) };
}
unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
    let values = unsafe { &mut *(data as *mut Vec<MONITORINFOEXW>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(m, (&mut info as *mut MONITORINFOEXW).cast()) } == 0 {
        return 0;
    }
    values.push(info);
    1
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRGetMonitors")]
pub unsafe extern "sysv64" fn XRRGetMonitors(
    dpy: *mut Display,
    _: Window,
    _: Bool,
    count: *mut c_int,
) -> *mut XRRMonitorInfo {
    if count.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        *count = 0;
    }
    let mut monitors = Vec::<MONITORINFOEXW>::new();
    if unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(collect),
            (&mut monitors as *mut Vec<MONITORINFOEXW>) as isize,
        )
    } == 0
        || monitors.is_empty()
    {
        return ptr::null_mut();
    }
    let output =
        unsafe { zeroed(monitors.len() * size_of::<XRRMonitorInfo>()).cast::<XRRMonitorInfo>() };
    if output.is_null() {
        return output;
    }
    let origin_x = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows_sys::Win32::UI::WindowsAndMessaging::SM_XVIRTUALSCREEN,
        )
    };
    let origin_y = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetSystemMetrics(
            windows_sys::Win32::UI::WindowsAndMessaging::SM_YVIRTUALSCREEN,
        )
    };
    for (i, m) in monitors.iter().enumerate() {
        let end = m
            .szDevice
            .iter()
            .position(|v| *v == 0)
            .unwrap_or(m.szDevice.len());
        let name = std::ffi::CString::new(String::from_utf16_lossy(&m.szDevice[..end])).unwrap();
        let dc = unsafe { CreateDCW(ptr::null(), m.szDevice.as_ptr(), ptr::null(), ptr::null()) };
        let (mw, mh) = if dc.is_null() {
            (0, 0)
        } else {
            let size = unsafe {
                (
                    GetDeviceCaps(dc, HORZSIZE as i32),
                    GetDeviceCaps(dc, VERTSIZE as i32),
                )
            };
            unsafe {
                DeleteDC(dc);
            }
            size
        };
        let rect = m.monitorInfo.rcMonitor;
        unsafe {
            output.add(i).write(XRRMonitorInfo {
                name: kinakaze_libX11::XInternAtom(dpy, name.as_ptr(), 0),
                primary: (m.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0) as i32,
                automatic: 1,
                noutput: 0,
                x: rect.left - origin_x,
                y: rect.top - origin_y,
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
                mwidth: mw,
                mheight: mh,
                outputs: ptr::null_mut(),
            });
        }
    }
    unsafe {
        *count = monitors.len() as i32;
    }
    output
}
#[unsafe(export_name = "kinakaze_engine_libXrandr_XRRFreeMonitors")]
pub unsafe extern "sysv64" fn XRRFreeMonitors(monitors: *mut XRRMonitorInfo) {
    unsafe {
        guest::free(monitors.cast());
    }
}
