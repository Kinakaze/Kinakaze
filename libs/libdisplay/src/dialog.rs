//! Native file and message dialogs for Linux guest applications.

#![allow(non_snake_case, dead_code)]

use core::ffi::{c_char, c_int, c_void};

#[repr(C)]
struct OpenFileNameW {
    lStructSize: u32,
    hwndOwner: *mut c_void,
    hInstance: *mut c_void,
    lpstrFilter: *const u16,
    lpstrCustomFilter: *mut u16,
    nMaxCustFilter: u32,
    nFilterIndex: u32,
    lpstrFile: *mut u16,
    nMaxFile: u32,
    lpstrFileTitle: *mut u16,
    nMaxFileTitle: u32,
    lpstrInitialDir: *const u16,
    lpstrTitle: *const u16,
    Flags: u32,
    nFileOffset: u16,
    nFileExtension: u16,
    lpstrDefExt: *const u16,
    lCustData: usize,
    lpfnHook: *mut c_void,
    lpTemplateName: *const u16,
    pvReserved: *mut c_void,
    dwReserved: u32,
    FlagsEx: u32,
}

#[link(name = "comdlg32")]
unsafe extern "system" {
    fn GetOpenFileNameW(lpofn: *mut OpenFileNameW) -> i32;
    fn GetSaveFileNameW(lpofn: *mut OpenFileNameW) -> i32;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn MessageBoxW(hWnd: *mut c_void, lpText: *const u16, lpCaption: *const u16, uType: u32)
    -> i32;
}

const OFN_FILEMUSTEXIST: u32 = 0x00001000;
const OFN_PATHMUSTEXIST: u32 = 0x00000800;
const OFN_OVERWRITEPROMPT: u32 = 0x00000002;
const OFN_NOCHANGEDIR: u32 = 0x00000008;

/// # Safety
/// When non-null, buffer must be writable for max_len bytes.
pub unsafe fn open_file(
    title: Option<&str>,
    filter: Option<&str>,
    buffer: *mut c_char,
    max_len: usize,
) -> isize {
    let mut file_buf = [0u16; 1024];
    let title_wide = title.map(|t| {
        t.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    });
    let filter_wide = filter.map(|f| {
        let mut v: Vec<u16> = f.encode_utf16().collect();
        for b in &mut v {
            if *b == b'|' as u16 {
                *b = 0;
            }
        }
        v.push(0);
        v.push(0);
        v
    });

    let mut ofn = OpenFileNameW {
        lStructSize: core::mem::size_of::<OpenFileNameW>() as u32,
        hwndOwner: core::ptr::null_mut(),
        hInstance: core::ptr::null_mut(),
        lpstrFilter: filter_wide
            .as_ref()
            .map(|v| v.as_ptr())
            .unwrap_or(core::ptr::null()),
        lpstrCustomFilter: core::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrFileTitle: core::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: core::ptr::null(),
        lpstrTitle: title_wide
            .as_ref()
            .map(|v| v.as_ptr())
            .unwrap_or(core::ptr::null()),
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: core::ptr::null(),
        lCustData: 0,
        lpfnHook: core::ptr::null_mut(),
        lpTemplateName: core::ptr::null(),
        pvReserved: core::ptr::null_mut(),
        dwReserved: 0,
        FlagsEx: 0,
    };

    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return -1;
    }

    let mut len = 0;
    while len < file_buf.len() && file_buf[len] != 0 {
        len += 1;
    }
    let path = String::from_utf16_lossy(&file_buf[..len]);
    let bytes = path.as_bytes();
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
/// When non-null, buffer must be writable for max_len bytes.
pub unsafe fn save_file(
    title: Option<&str>,
    filter: Option<&str>,
    default_name: Option<&str>,
    buffer: *mut c_char,
    max_len: usize,
) -> isize {
    let mut file_buf = [0u16; 1024];
    if let Some(name) = default_name {
        let name_wide: Vec<u16> = name.encode_utf16().take(file_buf.len() - 1).collect();
        file_buf[..name_wide.len()].copy_from_slice(&name_wide);
    }
    let title_wide = title.map(|t| {
        t.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    });
    let filter_wide = filter.map(|f| {
        let mut v: Vec<u16> = f.encode_utf16().collect();
        for b in &mut v {
            if *b == b'|' as u16 {
                *b = 0;
            }
        }
        v.push(0);
        v.push(0);
        v
    });

    let mut ofn = OpenFileNameW {
        lStructSize: core::mem::size_of::<OpenFileNameW>() as u32,
        hwndOwner: core::ptr::null_mut(),
        hInstance: core::ptr::null_mut(),
        lpstrFilter: filter_wide
            .as_ref()
            .map(|v| v.as_ptr())
            .unwrap_or(core::ptr::null()),
        lpstrCustomFilter: core::ptr::null_mut(),
        nMaxCustFilter: 0,
        nFilterIndex: 1,
        lpstrFile: file_buf.as_mut_ptr(),
        nMaxFile: file_buf.len() as u32,
        lpstrFileTitle: core::ptr::null_mut(),
        nMaxFileTitle: 0,
        lpstrInitialDir: core::ptr::null(),
        lpstrTitle: title_wide
            .as_ref()
            .map(|v| v.as_ptr())
            .unwrap_or(core::ptr::null()),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        nFileOffset: 0,
        nFileExtension: 0,
        lpstrDefExt: core::ptr::null(),
        lCustData: 0,
        lpfnHook: core::ptr::null_mut(),
        lpTemplateName: core::ptr::null(),
        pvReserved: core::ptr::null_mut(),
        dwReserved: 0,
        FlagsEx: 0,
    };

    if unsafe { GetSaveFileNameW(&mut ofn) } == 0 {
        return -1;
    }

    let mut len = 0;
    while len < file_buf.len() && file_buf[len] != 0 {
        len += 1;
    }
    let path = String::from_utf16_lossy(&file_buf[..len]);
    let bytes = path.as_bytes();
    if !buffer.is_null() && max_len > 0 {
        let copy_len = bytes.len().min(max_len - 1);
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer as *mut u8, copy_len);
            *buffer.add(copy_len) = 0;
        }
    }
    bytes.len() as isize
}

pub fn message_box(title: &str, message: &str, flags: u32) -> c_int {
    let t: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let m: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { MessageBoxW(core::ptr::null_mut(), m.as_ptr(), t.as_ptr(), flags) }
}
