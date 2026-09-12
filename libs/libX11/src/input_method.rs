//! Local direct-key input contexts. Only styles without application preedit or
//! status UI are advertised. No remote IM or composition service is claimed.
//! Handles and their linked ownership live entirely in the guest arena, so a
//! fork restores plain data instead of native Rust allocation internals.
use crate::{Display, KeySym, Status, Window, XKeyEvent};
use core::ffi::{c_char, c_int, c_void};
use core::ptr;
use std::ffi::CStr;

mod arguments;
mod lookup;
pub use arguments::*;
pub use lookup::*;

const STYLES: [u64; 2] = [0x408, 0x810];
#[repr(C)]
pub struct InputMethod {
    display: usize,
    contexts: *mut InputContext,
}
#[repr(C)]
pub struct InputContext {
    method: *mut InputMethod,
    next: *mut InputContext,
    style: u64,
    client: Window,
    focus: Window,
    focused: bool,
}
#[repr(C)]
struct Styles {
    count: u16,
    styles: *mut u64,
}

unsafe fn allocate<T>(value: T) -> *mut T {
    let pointer = unsafe { kinakaze_alloc::guest::malloc(size_of::<T>()).cast::<T>() };
    if !pointer.is_null() {
        unsafe {
            pointer.write(value);
        }
    }
    pointer
}

#[unsafe(export_name = "kinakaze_engine_libX11_XOpenIM")]
pub unsafe extern "sysv64" fn XOpenIM(
    display: *mut Display,
    _db: *mut c_void,
    _name: *mut c_char,
    _class: *mut c_char,
) -> *mut InputMethod {
    if display.is_null() {
        return ptr::null_mut();
    }
    // The process's native singleton is rebound through XDisplayOfIM in a fork
    // child. Guest-allocated displays, when present, retain their arena address.
    let display = if display == &raw mut crate::GLOBAL_DISPLAY {
        0
    } else {
        display as usize
    };
    unsafe {
        allocate(InputMethod {
            display,
            contexts: ptr::null_mut(),
        })
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XCloseIM")]
pub unsafe extern "sysv64" fn XCloseIM(method: *mut InputMethod) -> Status {
    if method.is_null() {
        return 0;
    }
    while !unsafe { (*method).contexts }.is_null() {
        unsafe {
            XDestroyIC((*method).contexts);
        }
    }
    unsafe {
        kinakaze_alloc::guest::free(method.cast());
    }
    1
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDisplayOfIM")]
pub unsafe extern "sysv64" fn XDisplayOfIM(method: *mut InputMethod) -> *mut Display {
    if method.is_null() {
        return ptr::null_mut();
    }
    if unsafe { (*method).display } == 0 {
        &raw mut crate::GLOBAL_DISPLAY
    } else {
        unsafe { (*method).display as *mut Display }
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XIMOfIC")]
pub unsafe extern "sysv64" fn XIMOfIC(context: *mut InputContext) -> *mut InputMethod {
    unsafe { context.as_ref() }.map_or(ptr::null_mut(), |context| context.method)
}
#[unsafe(export_name = "kinakaze_engine_libX11_XDestroyIC")]
pub unsafe extern "sysv64" fn XDestroyIC(context: *mut InputContext) {
    if context.is_null() {
        return;
    }
    unsafe {
        let mut link = &raw mut (*(*context).method).contexts;
        while !(*link).is_null() {
            if *link == context {
                *link = (*context).next;
                break;
            }
            link = &raw mut (**link).next;
        }
        kinakaze_alloc::guest::free(context.cast());
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XSetICFocus")]
pub unsafe extern "sysv64" fn XSetICFocus(context: *mut InputContext) {
    if let Some(context) = unsafe { context.as_mut() } {
        context.focused = true;
    }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XUnsetICFocus")]
pub unsafe extern "sysv64" fn XUnsetICFocus(context: *mut InputContext) {
    if let Some(context) = unsafe { context.as_mut() } {
        context.focused = false;
    }
}

// Direct-key contexts commit each key immediately. They have no uncommitted
// preedit text; all three reset encodings therefore return an empty result.
#[unsafe(export_name = "kinakaze_engine_libX11_XmbResetIC")]
pub unsafe extern "sysv64" fn XmbResetIC(_context: *mut InputContext) -> *mut c_char {
    ptr::null_mut()
}
#[unsafe(export_name = "kinakaze_engine_libX11_Xutf8ResetIC")]
pub unsafe extern "sysv64" fn Xutf8ResetIC(context: *mut InputContext) -> *mut c_char {
    unsafe { XmbResetIC(context) }
}
#[unsafe(export_name = "kinakaze_engine_libX11_XwcResetIC")]
pub unsafe extern "sysv64" fn XwcResetIC(context: *mut InputContext) -> *mut i32 {
    unsafe { XmbResetIC(context).cast() }
}

unsafe fn context_values(
    context: *mut InputContext,
    args: &mut arguments::Cursor,
    get: bool,
    creating: bool,
) -> *mut c_char {
    loop {
        let name = unsafe { args.next() } as *mut c_char;
        if name.is_null() {
            return ptr::null_mut();
        }
        let value = unsafe { args.next() };
        if context.is_null() || (get && value == 0) {
            return name;
        }
        let context = unsafe { &mut *context };
        let bytes = unsafe { CStr::from_ptr(name) }.to_bytes();
        if get {
            let result = match bytes {
                b"inputStyle" => context.style,
                b"clientWindow" => context.client as u64,
                b"focusWindow" => context.focus as u64,
                b"filterEvents" => 0, // no asynchronous XFilterEvent consumption
                _ => return name,
            };
            unsafe {
                (value as *mut u64).write(result);
            }
        } else {
            match bytes {
                b"inputStyle" if creating && STYLES.contains(&(value as u64)) => {
                    context.style = value as u64
                }
                b"clientWindow" if creating || context.client == 0 => {
                    context.client = value;
                    if context.focus == 0 {
                        context.focus = value;
                    }
                }
                b"focusWindow" => context.focus = value,
                _ => return name,
            }
        }
    }
}
unsafe extern "sysv64" fn create(
    method: *mut InputMethod,
    args: *const arguments::Saved,
) -> *mut InputContext {
    if method.is_null() {
        return ptr::null_mut();
    }
    let context = unsafe {
        allocate(InputContext {
            method,
            next: ptr::null_mut(),
            style: 0,
            client: 0,
            focus: 0,
            focused: false,
        })
    };
    if context.is_null() {
        return context;
    }
    let error = unsafe { context_values(context, &mut arguments::Cursor::new(args), false, true) };
    if !error.is_null() || unsafe { (*context).style } == 0 {
        unsafe {
            kinakaze_alloc::guest::free(context.cast());
        }
        return ptr::null_mut();
    }
    unsafe {
        (*context).next = (*method).contexts;
        (*method).contexts = context;
    }
    context
}
unsafe extern "sysv64" fn get_ic(
    context: *mut InputContext,
    args: *const arguments::Saved,
) -> *mut c_char {
    unsafe { context_values(context, &mut arguments::Cursor::new(args), true, false) }
}
unsafe extern "sysv64" fn set_ic(
    context: *mut InputContext,
    args: *const arguments::Saved,
) -> *mut c_char {
    unsafe { context_values(context, &mut arguments::Cursor::new(args), false, false) }
}
unsafe extern "sysv64" fn get_im(
    method: *mut InputMethod,
    args: *const arguments::Saved,
) -> *mut c_char {
    let mut args = unsafe { arguments::Cursor::new(args) };
    loop {
        let name = unsafe { args.next() } as *mut c_char;
        if name.is_null() {
            return ptr::null_mut();
        }
        let output = unsafe { args.next() } as *mut *mut Styles;
        if method.is_null()
            || output.is_null()
            || unsafe { CStr::from_ptr(name) }.to_bytes() != b"queryInputStyle"
        {
            return name;
        }
        let styles = unsafe {
            kinakaze_alloc::guest::malloc(size_of::<Styles>() + size_of_val(&STYLES))
                .cast::<Styles>()
        };
        if styles.is_null() {
            return name;
        }
        unsafe {
            let values = styles.add(1).cast::<u64>();
            ptr::copy_nonoverlapping(STYLES.as_ptr(), values, STYLES.len());
            styles.write(Styles {
                count: STYLES.len() as u16,
                styles: values,
            });
            output.write(styles);
        }
    }
}
unsafe extern "sysv64" fn set_im(
    _method: *mut InputMethod,
    args: *const arguments::Saved,
) -> *mut c_char {
    // This local method has no writable IM-wide resources. Report the first
    // unsupported name, as required by XSetIMValues, instead of claiming success.
    unsafe { arguments::Cursor::new(args).next() as *mut c_char }
}
