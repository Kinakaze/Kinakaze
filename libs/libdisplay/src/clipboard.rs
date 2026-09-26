//! Win32 clipboard interface for Linux guest programs.

use core::ffi::{c_char, c_int};
use std::ffi::CStr;

const CF_UNICODETEXT: u32 = 13;
const GMEM_MOVEABLE: u32 = 0x0002;

#[link(name = "user32")]
unsafe extern "system" {
    fn OpenClipboard(hWndNewOwner: *mut core::ffi::c_void) -> i32;
    fn CloseClipboard() -> i32;
    fn EmptyClipboard() -> i32;
    fn GetClipboardData(uFormat: u32) -> *mut core::ffi::c_void;
    fn SetClipboardData(uFormat: u32, hMem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GlobalAlloc(uFlags: u32, dwBytes: usize) -> *mut core::ffi::c_void;
    fn GlobalLock(hMem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
    fn GlobalUnlock(hMem: *mut core::ffi::c_void) -> i32;
    fn GlobalFree(hMem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}

use std::sync::Mutex;

static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

fn open_clipboard_retry() -> bool {
    for _ in 0..20 {
        if unsafe { OpenClipboard(core::ptr::null_mut()) } != 0 {
            return true;
        }
        std::thread::sleep(core::time::Duration::from_millis(10));
    }
    false
}

/// # Safety
/// When non-null, buffer must be writable for max_len bytes.
pub unsafe fn get_text(buffer: *mut c_char, max_len: usize) -> isize {
    let _guard = CLIPBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !open_clipboard_retry() {
        return -1;
    }
    let handle = unsafe { GetClipboardData(CF_UNICODETEXT) };
    if handle.is_null() {
        unsafe { CloseClipboard() };
        return -1;
    }
    let ptr = unsafe { GlobalLock(handle) } as *const u16;
    if ptr.is_null() {
        unsafe { CloseClipboard() };
        return -1;
    }
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 {
            len += 1;
        }
    }
    let wide_slice = unsafe { core::slice::from_raw_parts(ptr, len) };
    let utf8 = String::from_utf16_lossy(wide_slice);
    unsafe {
        GlobalUnlock(handle);
        CloseClipboard();
    }
    let bytes = utf8.as_bytes();
    if !buffer.is_null() && max_len > 0 {
        let copy_len = bytes.len().min(max_len - 1);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, copy_len);
            *buffer.add(copy_len) = 0;
        }
    }
    bytes.len() as isize
}

/// # Safety
/// When non-null, text must point to a readable NUL-terminated string.
pub unsafe fn set_text(text: *const c_char) -> c_int {
    let _guard = CLIPBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if text.is_null() {
        return -1;
    }
    let cstr = unsafe { CStr::from_ptr(text) };
    let s = match cstr.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let size = wide.len() * 2;
    let hmem = unsafe { GlobalAlloc(GMEM_MOVEABLE, size) };
    if hmem.is_null() {
        return -1;
    }
    let dest = unsafe { GlobalLock(hmem) } as *mut u16;
    if dest.is_null() {
        unsafe { GlobalFree(hmem) };
        return -1;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(wide.as_ptr(), dest, wide.len());
        GlobalUnlock(hmem);
    }
    if !open_clipboard_retry() {
        unsafe { GlobalFree(hmem) };
        return -1;
    }
    unsafe {
        EmptyClipboard();
        if SetClipboardData(CF_UNICODETEXT, hmem).is_null() {
            GlobalFree(hmem);
            CloseClipboard();
            return -1;
        }
        CloseClipboard();
    }
    0
}

pub fn clear() -> c_int {
    let _guard = CLIPBOARD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !open_clipboard_retry() {
        return -1;
    }
    unsafe {
        EmptyClipboard();
        CloseClipboard();
    }
    0
}
