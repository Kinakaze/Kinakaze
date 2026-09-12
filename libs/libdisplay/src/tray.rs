//! Win32 System Tray (Shell_NotifyIcon) interface for Linux guest programs.

#![allow(non_snake_case, dead_code)]

use core::ffi::c_void;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const NIM_ADD: u32 = 0x00000000;
const NIM_MODIFY: u32 = 0x00000001;
const NIM_DELETE: u32 = 0x00000002;

const NIF_MESSAGE: u32 = 0x00000001;
const NIF_ICON: u32 = 0x00000002;
const NIF_TIP: u32 = 0x00000004;
const NIF_INFO: u32 = 0x00000010;

const NIIF_INFO: u32 = 0x00000001;

#[repr(C)]
struct NotifyIconDataW {
    cbSize: u32,
    hWnd: *mut c_void,
    uID: u32,
    uFlags: u32,
    uCallbackMessage: u32,
    hIcon: *mut c_void,
    szTip: [u16; 128],
    dwState: u32,
    dwStateMask: u32,
    szInfo: [u16; 256],
    uTimeoutOrVersion: u32,
    szInfoTitle: [u16; 64],
    dwInfoFlags: u32,
    guidItem: [u8; 16],
    hBalloonIcon: *mut c_void,
}

#[link(name = "shell32")]
unsafe extern "system" {
    fn Shell_NotifyIconW(dwMessage: u32, lpData: *const NotifyIconDataW) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn LoadIconW(hInstance: *mut c_void, lpIconName: *const u16) -> *mut c_void;
}

const IDI_APPLICATION: *const u16 = 32512 as *const u16;
const WM_USER: u32 = 0x0400;
const WM_TRAYNOTIFY: u32 = WM_USER + 100;

struct TrayState {
    hwnd: usize,
    uid: u32,
}

static NEXT_TRAY_ID: AtomicU64 = AtomicU64::new(1);

fn trays() -> &'static Mutex<HashMap<u64, TrayState>> {
    static TRAYS: std::sync::OnceLock<Mutex<HashMap<u64, TrayState>>> = std::sync::OnceLock::new();
    TRAYS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn create(window_id: u64, tooltip: &str) -> Option<u64> {
    let hwnd = crate::window::native_handle(window_id)? as *mut c_void;
    let tray_id = NEXT_TRAY_ID.fetch_add(1, Ordering::Relaxed);
    let uid = tray_id as u32;

    let icon = unsafe { LoadIconW(core::ptr::null_mut(), IDI_APPLICATION) };

    let mut data = NotifyIconDataW {
        cbSize: core::mem::size_of::<NotifyIconDataW>() as u32,
        hWnd: hwnd,
        uID: uid,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_TRAYNOTIFY,
        hIcon: icon,
        szTip: [0; 128],
        dwState: 0,
        dwStateMask: 0,
        szInfo: [0; 256],
        uTimeoutOrVersion: 0,
        szInfoTitle: [0; 64],
        dwInfoFlags: 0,
        guidItem: [0; 16],
        hBalloonIcon: core::ptr::null_mut(),
    };

    let tip_wide: Vec<u16> = tooltip.encode_utf16().take(127).collect();
    for (i, &c) in tip_wide.iter().enumerate() {
        data.szTip[i] = c;
    }

    if unsafe { Shell_NotifyIconW(NIM_ADD, &data) } == 0 {
        return None;
    }

    trays().lock().unwrap().insert(
        tray_id,
        TrayState {
            hwnd: hwnd as usize,
            uid,
        },
    );
    Some(tray_id)
}

pub fn set_tooltip(tray_id: u64, tooltip: &str) -> bool {
    let map = trays().lock().unwrap();
    let Some(tray) = map.get(&tray_id) else {
        return false;
    };
    let mut data = NotifyIconDataW {
        cbSize: core::mem::size_of::<NotifyIconDataW>() as u32,
        hWnd: tray.hwnd as *mut c_void,
        uID: tray.uid,
        uFlags: NIF_TIP,
        uCallbackMessage: 0,
        hIcon: core::ptr::null_mut(),
        szTip: [0; 128],
        dwState: 0,
        dwStateMask: 0,
        szInfo: [0; 256],
        uTimeoutOrVersion: 0,
        szInfoTitle: [0; 64],
        dwInfoFlags: 0,
        guidItem: [0; 16],
        hBalloonIcon: core::ptr::null_mut(),
    };
    let tip_wide: Vec<u16> = tooltip.encode_utf16().take(127).collect();
    for (i, &c) in tip_wide.iter().enumerate() {
        data.szTip[i] = c;
    }
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) != 0 }
}

pub fn show_balloon(tray_id: u64, title: &str, msg: &str, timeout_ms: u32) -> bool {
    let map = trays().lock().unwrap();
    let Some(tray) = map.get(&tray_id) else {
        return false;
    };
    let mut data = NotifyIconDataW {
        cbSize: core::mem::size_of::<NotifyIconDataW>() as u32,
        hWnd: tray.hwnd as *mut c_void,
        uID: tray.uid,
        uFlags: NIF_INFO,
        uCallbackMessage: 0,
        hIcon: core::ptr::null_mut(),
        szTip: [0; 128],
        dwState: 0,
        dwStateMask: 0,
        szInfo: [0; 256],
        uTimeoutOrVersion: timeout_ms,
        szInfoTitle: [0; 64],
        dwInfoFlags: NIIF_INFO,
        guidItem: [0; 16],
        hBalloonIcon: core::ptr::null_mut(),
    };
    let title_wide: Vec<u16> = title.encode_utf16().take(63).collect();
    for (i, &c) in title_wide.iter().enumerate() {
        data.szInfoTitle[i] = c;
    }
    let msg_wide: Vec<u16> = msg.encode_utf16().take(255).collect();
    for (i, &c) in msg_wide.iter().enumerate() {
        data.szInfo[i] = c;
    }
    unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) != 0 }
}

pub fn destroy(tray_id: u64) -> bool {
    let mut map = trays().lock().unwrap();
    let Some(tray) = map.remove(&tray_id) else {
        return false;
    };
    let data = NotifyIconDataW {
        cbSize: core::mem::size_of::<NotifyIconDataW>() as u32,
        hWnd: tray.hwnd as *mut c_void,
        uID: tray.uid,
        uFlags: 0,
        uCallbackMessage: 0,
        hIcon: core::ptr::null_mut(),
        szTip: [0; 128],
        dwState: 0,
        dwStateMask: 0,
        szInfo: [0; 256],
        uTimeoutOrVersion: 0,
        szInfoTitle: [0; 64],
        dwInfoFlags: 0,
        guidItem: [0; 16],
        hBalloonIcon: core::ptr::null_mut(),
    };
    unsafe { Shell_NotifyIconW(NIM_DELETE, &data) != 0 }
}
