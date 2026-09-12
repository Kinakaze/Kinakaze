//! ALSA control identifiers and hardware-control discovery for desktop hotkeys.
use super::*;

/// One virtual card whose playback endpoint is Windows' default mapper.
#[repr(C)]
pub struct Control {
    pub(super) card: i32,
    pub(super) mode: i32,
}

pub(super) unsafe fn card_name(card: i32, output: *mut *mut c_char, name: &core::ffi::CStr) -> i32 {
    if output.is_null() {
        return -22;
    }
    unsafe {
        *output = core::ptr::null_mut();
    }
    if card != 0 {
        return -19;
    }
    let bytes = name.to_bytes_with_nul();
    let allocation = unsafe { kinakaze_alloc::c::malloc(bytes.len()) };
    if allocation.is_null() {
        return -12;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), allocation, bytes.len());
        *output = allocation.cast();
    }
    0
}

#[repr(C)]
#[derive(Default)]
pub struct HwdepInfo {
    device: u32,
    card: i32,
    interface: i32,
}

#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_sizeof")]
pub extern "sysv64" fn snd_ctl_card_info_sizeof() -> usize {
    size_of::<snd_ctl_card_info_t>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_get_driver")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_get_driver(
    info: *const snd_ctl_card_info_t,
) -> *const c_char {
    if info.is_null() {
        core::ptr::null()
    } else {
        c"WinMM".as_ptr()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_card_info_get_longname")]
pub unsafe extern "sysv64" fn snd_ctl_card_info_get_longname(
    info: *const snd_ctl_card_info_t,
) -> *const c_char {
    if info.is_null() {
        core::ptr::null()
    } else {
        c"Windows Default Audio via WinMM".as_ptr()
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_hwdep_info_sizeof")]
pub extern "sysv64" fn snd_hwdep_info_sizeof() -> usize {
    size_of::<HwdepInfo>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_hwdep_info_get_iface")]
pub unsafe extern "sysv64" fn snd_hwdep_info_get_iface(info: *const HwdepInfo) -> c_int {
    if info.is_null() {
        -22
    } else {
        unsafe { (*info).interface }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_hwdep_next_device")]
pub unsafe extern "sysv64" fn snd_ctl_hwdep_next_device(
    ctl: *mut c_void,
    device: *mut c_int,
) -> c_int {
    if ctl.is_null() || device.is_null() {
        return -22;
    }
    // WinMM exposes PCM/MIDI endpoints, not Linux hwdep character devices.
    unsafe {
        *device = -1;
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_hwdep_info")]
pub unsafe extern "sysv64" fn snd_ctl_hwdep_info(ctl: *mut c_void, info: *mut HwdepInfo) -> c_int {
    if ctl.is_null() || info.is_null() {
        -22
    } else {
        -2
    }
}
#[repr(C)]
pub struct ElementId {
    numid: u32,
    interface: i32,
    device: u32,
    subdevice: u32,
    name: [u8; 44],
    index: u32,
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_elem_id_sizeof")]
pub unsafe extern "sysv64" fn snd_ctl_elem_id_sizeof() -> usize {
    core::mem::size_of::<ElementId>()
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_elem_id_clear")]
pub unsafe extern "sysv64" fn snd_ctl_elem_id_clear(id: *mut ElementId) {
    if !id.is_null() {
        unsafe {
            core::ptr::write_bytes(id, 0, 1);
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_elem_id_set_interface")]
pub unsafe extern "sysv64" fn snd_ctl_elem_id_set_interface(id: *mut ElementId, interface: i32) {
    if !id.is_null() {
        unsafe {
            (*id).interface = interface;
        }
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_ctl_elem_id_set_name")]
pub unsafe extern "sysv64" fn snd_ctl_elem_id_set_name(id: *mut ElementId, name: *const c_char) {
    if id.is_null() {
        return;
    }
    let output = unsafe { &mut (*id).name };
    output.fill(0);
    if !name.is_null() {
        let bytes = unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes();
        let count = bytes.len().min(output.len() - 1);
        output[..count].copy_from_slice(&bytes[..count]);
    }
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_hctl_open")]
pub unsafe extern "sysv64" fn snd_hctl_open(
    output: *mut *mut c_void,
    name: *const c_char,
    _mode: i32,
) -> i32 {
    if output.is_null() || name.is_null() {
        return -22;
    }
    unsafe {
        *output = core::ptr::null_mut();
    }
    // The hosted PCM endpoint has no Linux kernel HCTL device or GPIO/LED
    // controls. Report that device absence so GNOME skips its hardware probe.
    -19
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_hctl_close")]
pub unsafe extern "sysv64" fn snd_hctl_close(_handle: *mut c_void) -> i32 {
    -77
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_hctl_load")]
pub unsafe extern "sysv64" fn snd_hctl_load(_handle: *mut c_void) -> i32 {
    -77
}
#[unsafe(export_name = "kinakaze_engine_libasound_snd_hctl_find_elem")]
pub unsafe extern "sysv64" fn snd_hctl_find_elem(
    _handle: *mut c_void,
    _id: *const ElementId,
) -> *mut c_void {
    core::ptr::null_mut()
}
