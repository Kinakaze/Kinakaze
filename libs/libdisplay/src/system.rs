//! System-level integration: dark mode, DPI scaling, power/sleep control, shell execution.

#![allow(non_snake_case, dead_code)]

use core::ffi::{c_float, c_void};

#[link(name = "shell32")]
unsafe extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        lpOperation: *const u16,
        lpFile: *const u16,
        lpParameters: *const u16,
        lpDirectory: *const u16,
        nShowCmd: i32,
    ) -> isize;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetThreadExecutionState(esFlags: u32) -> u32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDpiForWindow(hwnd: *mut c_void) -> u32;
    fn GetDpiForSystem() -> u32;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        hKey: isize,
        lpSubKey: *const u16,
        ulOptions: u32,
        samDesired: u32,
        phkResult: *mut isize,
    ) -> i32;
    fn RegQueryValueExW(
        hKey: isize,
        lpValueName: *const u16,
        lpReserved: *const u32,
        lpType: *mut u32,
        lpData: *mut u8,
        lpcbData: *mut u32,
    ) -> i32;
    fn RegCloseKey(hKey: isize) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x80000001u32 as i32 as isize;
const KEY_READ: u32 = 0x20019;
const ES_CONTINUOUS: u32 = 0x80000000;
const ES_SYSTEM_REQUIRED: u32 = 0x00000001;
const ES_DISPLAY_REQUIRED: u32 = 0x00000002;
const SW_SHOWNORMAL: i32 = 1;

pub fn is_dark_mode() -> bool {
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let valname: Vec<u16> = "AppsUseLightTheme"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey: isize = 0;
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
        return false;
    }
    let mut data: u32 = 1;
    let mut size: u32 = 4;
    let mut type_: u32 = 0;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            valname.as_ptr(),
            core::ptr::null(),
            &mut type_,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        )
    };
    unsafe { RegCloseKey(hkey) };
    if status == 0 { data == 0 } else { false }
}

pub fn get_dpi(window_id: u64) -> u32 {
    if window_id != 0 {
        if let Some(hwnd) = crate::window::native_handle(window_id) {
            let dpi = unsafe { GetDpiForWindow(hwnd as *mut c_void) };
            if dpi > 0 {
                return dpi;
            }
        }
    }
    let sys_dpi = unsafe { GetDpiForSystem() };
    if sys_dpi > 0 { sys_dpi } else { 96 }
}

pub fn get_scale_factor(window_id: u64) -> c_float {
    get_dpi(window_id) as f32 / 96.0
}

pub fn get_accent_color() -> u32 {
    let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\DWM"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let valname: Vec<u16> = "AccentColor"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut hkey: isize = 0;
    if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey) } != 0 {
        return 0x0078D7;
    }
    let mut data: u32 = 0;
    let mut size: u32 = 4;
    let mut type_: u32 = 0;
    let status = unsafe {
        RegQueryValueExW(
            hkey,
            valname.as_ptr(),
            core::ptr::null(),
            &mut type_,
            &mut data as *mut u32 as *mut u8,
            &mut size,
        )
    };
    unsafe { RegCloseKey(hkey) };
    if status == 0 { data } else { 0x0078D7 }
}

pub fn prevent_sleep(enable: bool) {
    let flags = if enable {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    unsafe { SetThreadExecutionState(flags) };
}

pub fn shell_open(target: &str) -> bool {
    let op: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let ret = unsafe {
        ShellExecuteW(
            core::ptr::null_mut(),
            op.as_ptr(),
            file.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    ret > 32
}
