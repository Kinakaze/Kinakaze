//! Xext utility structures are client-owned, with the exact LP64 public layout.
//! Keeping their links and payloads in guest memory also preserves them at fork.
use super::*;
use kinakaze_libX11::xext::XExtensionCodes;
use std::sync::Mutex;
mod lifecycle;
static LISTS: Mutex<usize> = Mutex::new(0);
#[repr(C)]
pub struct XExtensionInfo {
    pub head: *mut XExtDisplayInfo,
    pub cur: *mut XExtDisplayInfo,
    pub ndisplays: c_int,
}
#[repr(C)]
pub struct XExtDisplayInfo {
    pub next: *mut XExtDisplayInfo,
    pub display: *mut Display,
    pub codes: *mut XExtensionCodes,
    pub data: *mut c_char,
}
#[repr(C)]
struct OwnedInfo {
    public: XExtensionInfo,
    next: usize,
}
#[repr(C)]
struct DisplayEntry {
    public: XExtDisplayInfo,
    codes: XExtensionCodes,
    close: usize,
    closing: bool,
}
#[unsafe(export_name = "kinakaze_engine_libXext_XextCreateExtension")]
pub unsafe extern "sysv64" fn XextCreateExtension() -> *mut XExtensionInfo {
    let mut lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    let result = unsafe { guest::malloc(mem::size_of::<OwnedInfo>()) }.cast::<OwnedInfo>();
    if !result.is_null() {
        unsafe {
            result.write(OwnedInfo {
                public: XExtensionInfo {
                    head: ptr::null_mut(),
                    cur: ptr::null_mut(),
                    ndisplays: 0,
                },
                next: *lists,
            });
        }
        *lists = result as usize;
    }
    result.cast()
}
#[unsafe(export_name = "kinakaze_engine_libXext_XextFindDisplay")]
pub unsafe extern "sysv64" fn XextFindDisplay(
    info: *mut XExtensionInfo,
    display: *mut Display,
) -> *mut XExtDisplayInfo {
    let _guard = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { find(info, display) }
}
unsafe fn find(info: *mut XExtensionInfo, display: *mut Display) -> *mut XExtDisplayInfo {
    let Some(info) = (unsafe { info.as_mut() }) else {
        return ptr::null_mut();
    };
    if !info.cur.is_null() && unsafe { (*info.cur).display == display } {
        return info.cur;
    }
    let mut entry = info.head;
    while !entry.is_null() {
        if unsafe { (*entry).display == display } {
            info.cur = entry;
            return entry;
        }
        entry = unsafe { (*entry).next };
    }
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libXext_XextAddDisplay")]
pub unsafe extern "sysv64" fn XextAddDisplay(
    info: *mut XExtensionInfo,
    display: *mut Display,
    name: *const c_char,
    hooks: *mut c_void,
    _events: c_int,
    data: *mut c_char,
) -> *mut XExtDisplayInfo {
    if info.is_null() {
        return ptr::null_mut();
    }
    let _guard = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    let existing = unsafe { find(info, display) };
    if !existing.is_null() {
        return existing;
    }
    let entry = unsafe { guest::malloc(mem::size_of::<DisplayEntry>()) }.cast::<DisplayEntry>();
    if entry.is_null() {
        return ptr::null_mut();
    }
    let mut codes = XExtensionCodes {
        extension: 0,
        major_opcode: 0,
        first_event: 0,
        first_error: 0,
    };
    let available = !name.is_null()
        && unsafe {
            kinakaze_libX11::XQueryExtension(
                display,
                name,
                &raw mut codes.major_opcode,
                &raw mut codes.first_event,
                &raw mut codes.first_error,
            ) != 0
        };
    codes.extension = codes.major_opcode;
    let mut close = 0;
    if !hooks.is_null() {
        let callbacks = unsafe { &*hooks.cast::<[usize; 11]>() };
        // This adapter has no wire transport or font/GC extension callbacks.
        // Refuse hooks we cannot dispatch rather than report registration success.
        if available
            && callbacks
                .iter()
                .enumerate()
                .any(|(i, callback)| i != 6 && *callback != 0)
        {
            unsafe {
                guest::free(entry.cast());
            }
            return ptr::null_mut();
        }
        close = callbacks[6];
    }
    unsafe {
        entry.write(DisplayEntry {
            public: XExtDisplayInfo {
                next: (*info).head,
                display,
                codes: if available {
                    &raw mut (*entry).codes
                } else {
                    ptr::null_mut()
                },
                data,
            },
            codes,
            close,
            closing: false,
        });
        (*info).head = entry.cast();
        (*info).cur = entry.cast();
        (*info).ndisplays += 1;
    }
    entry.cast()
}
#[unsafe(export_name = "kinakaze_engine_libXext_XextRemoveDisplay")]
pub unsafe extern "sysv64" fn XextRemoveDisplay(
    info: *mut XExtensionInfo,
    display: *mut Display,
) -> c_int {
    let _guard = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    unsafe { remove(info, display) }
}
unsafe fn remove(info: *mut XExtensionInfo, display: *mut Display) -> c_int {
    let Some(info) = (unsafe { info.as_mut() }) else {
        return 0;
    };
    let mut link = &raw mut info.head;
    unsafe {
        while !(*link).is_null() {
            let entry = *link;
            if (*entry).display == display {
                *link = (*entry).next;
                if info.cur == entry {
                    info.cur = ptr::null_mut();
                }
                info.ndisplays -= 1;
                guest::free(entry.cast());
                return 1;
            }
            link = &raw mut (*entry).next;
        }
    }
    0
}
#[unsafe(export_name = "kinakaze_engine_libXext_XextDestroyExtension")]
pub unsafe extern "sysv64" fn XextDestroyExtension(info: *mut XExtensionInfo) {
    if info.is_null() {
        return;
    }
    let mut lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut link = &raw mut *lists;
    unsafe {
        while *link != 0 && *link != info as usize {
            link = &raw mut (*(*link as *mut OwnedInfo)).next;
        }
        if *link == info as usize {
            *link = (*info.cast::<OwnedInfo>()).next;
        }
        while !(*info).head.is_null() {
            remove(info, (*(*info).head).display);
        }
        guest::free(info.cast());
    }
}

unsafe fn close_display(display: *mut Display) {
    loop {
        let action = {
            let lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
            let mut current = *lists;
            let mut action = None;
            while current != 0 {
                let info = current as *mut XExtensionInfo;
                let entry = unsafe { find(info, display) }.cast::<DisplayEntry>();
                if !entry.is_null() && !unsafe { (*entry).closing } {
                    unsafe {
                        (*entry).closing = true;
                    }
                    action = Some((info, unsafe { (*entry).close }, unsafe {
                        (*entry).public.codes
                    }));
                    break;
                }
                current = unsafe { (*(current as *mut OwnedInfo)).next };
            }
            action
        };
        let Some((info, callback, codes)) = action else {
            break;
        };
        if callback != 0 {
            let callback: unsafe extern "sysv64" fn(*mut Display, *mut XExtensionCodes) -> c_int =
                unsafe { mem::transmute(callback) };
            unsafe {
                callback(display, codes);
            }
        }
        // A close hook may remove this display or destroy its entire list.
        let lists = LISTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut current = *lists;
        while current != 0 {
            if current == info as usize {
                unsafe {
                    remove(info, display);
                }
                break;
            }
            current = unsafe { (*(current as *mut OwnedInfo)).next };
        }
    }
}
