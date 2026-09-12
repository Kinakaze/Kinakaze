//! Text-list conversion owns its guest buffer; caller strings are never lent
//! out as XFree-able storage. The two passes require no intermediate strings.
use crate::{Display, Status, XTextProperty};
use core::ffi::{c_char, c_int};
use core::ptr;
use std::ffi::CStr;

const XA_STRING: usize = 31;
const NO_MEMORY: c_int = -1;
const CONVERTER_NOT_FOUND: c_int = -3;

#[unsafe(export_name = "kinakaze_engine_libX11_XTextPropertyToStringList")]
pub unsafe extern "sysv64" fn XTextPropertyToStringList(
    property: *const XTextProperty,
    output: *mut *mut *mut c_char,
    count: *mut c_int,
) -> Status {
    unsafe {
        *output = ptr::null_mut();
        *count = 0;
    }
    let property = unsafe { &*property };
    if property.encoding != XA_STRING || property.format != 8 {
        return 0;
    }
    let Ok(length) = usize::try_from(property.nitems) else {
        return 0;
    };
    if length == 0 {
        return 1;
    }
    if length > isize::MAX as usize || property.value.is_null() {
        return 0;
    }
    let bytes = unsafe { core::slice::from_raw_parts(property.value, length) };
    let entries = 1 + bytes.iter().filter(|b| **b == 0).count();
    if entries > c_int::MAX as usize {
        return 0;
    }
    let list = unsafe {
        kinakaze_alloc::guest::malloc((entries + 1) * core::mem::size_of::<*mut c_char>())
    }
    .cast::<*mut c_char>();
    if list.is_null() {
        return 0;
    }
    let strings = unsafe { kinakaze_alloc::guest::malloc(length + 1) };
    if strings.is_null() {
        unsafe {
            kinakaze_alloc::guest::free(list.cast());
        }
        return 0;
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), strings, length);
        *strings.add(length) = 0;
        *list = strings.cast();
    }
    let mut index = 1;
    for (offset, b) in bytes.iter().enumerate() {
        if *b == 0 {
            unsafe {
                *list.add(index) = strings.add(offset + 1).cast();
            }
            index += 1;
        }
    }
    unsafe {
        *list.add(entries) = ptr::null_mut();
        *output = list;
        *count = entries as c_int;
    }
    1
}

#[unsafe(export_name = "kinakaze_engine_libX11_XFreeStringList")]
pub unsafe extern "sysv64" fn XFreeStringList(list: *mut *mut c_char) {
    if !list.is_null() {
        unsafe {
            kinakaze_alloc::guest::free((*list).cast());
            kinakaze_alloc::guest::free(list.cast());
        }
    }
}

#[derive(Clone, Copy)]
enum Encoding {
    Raw,
    Utf8,
    Latin1,
}

unsafe fn convert(
    list: *mut *mut c_char,
    count: c_int,
    output: *mut XTextProperty,
    encoding: Encoding,
    atom: usize,
) -> Result<(), c_int> {
    if output.is_null() || count < 0 || (count > 0 && list.is_null()) {
        return Err(CONVERTER_NOT_FOUND);
    }
    let mut size = 0_usize;
    for index in 0..count as usize {
        let string = unsafe { *list.add(index) };
        let bytes = if string.is_null() {
            &[][..]
        } else {
            unsafe { CStr::from_ptr(string) }.to_bytes()
        };
        let length = match encoding {
            Encoding::Raw => bytes.len(),
            Encoding::Utf8 => std::str::from_utf8(bytes)
                .map_err(|_| CONVERTER_NOT_FOUND)?
                .len(),
            Encoding::Latin1 => {
                let string = std::str::from_utf8(bytes).map_err(|_| CONVERTER_NOT_FOUND)?;
                if string.chars().any(|character| character as u32 > 255) {
                    return Err(CONVERTER_NOT_FOUND);
                }
                string.chars().count()
            }
        };
        size = size
            .checked_add(length)
            .and_then(|v| v.checked_add(1))
            .ok_or(NO_MEMORY)?;
    }
    let value = unsafe { kinakaze_alloc::guest::malloc(size.max(1)) };
    if value.is_null() {
        return Err(NO_MEMORY);
    }
    let mut cursor = value;
    for index in 0..count as usize {
        let string = unsafe { *list.add(index) };
        let bytes = if string.is_null() {
            &[][..]
        } else {
            unsafe { CStr::from_ptr(string) }.to_bytes()
        };
        match encoding {
            Encoding::Raw | Encoding::Utf8 => unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), cursor, bytes.len());
                cursor = cursor.add(bytes.len());
            },
            Encoding::Latin1 => {
                // The first pass validated the same immutable input.
                for character in unsafe { std::str::from_utf8_unchecked(bytes) }.chars() {
                    unsafe {
                        cursor.write(character as u8);
                        cursor = cursor.add(1);
                    }
                }
            }
        }
        unsafe {
            cursor.write(0);
            cursor = cursor.add(1);
        }
    }
    if count == 0 {
        unsafe {
            value.write(0);
        }
    }
    unsafe {
        output.write(XTextProperty {
            value,
            encoding: atom,
            format: 8,
            nitems: size.saturating_sub(1) as u64,
        });
    }
    Ok(())
}

#[unsafe(export_name = "kinakaze_engine_libX11_XStringListToTextProperty")]
pub unsafe extern "sysv64" fn XStringListToTextProperty(
    list: *mut *mut c_char,
    count: c_int,
    output: *mut XTextProperty,
) -> Status {
    unsafe { convert(list, count, output, Encoding::Raw, XA_STRING).is_ok() as Status }
}

#[unsafe(export_name = "kinakaze_engine_libX11_Xutf8TextListToTextProperty")]
pub unsafe extern "sysv64" fn Xutf8TextListToTextProperty(
    display: *mut Display,
    list: *mut *mut c_char,
    count: c_int,
    style: c_int,
    output: *mut XTextProperty,
) -> c_int {
    let (encoding, atom) = match style {
        0 | 3 => (Encoding::Latin1, XA_STRING),
        2 | 4 => (Encoding::Utf8, unsafe {
            crate::XInternAtom(display, c"UTF8_STRING".as_ptr(), 0)
        }),
        // Compound-text needs its own charset encoder; do not label UTF-8
        // bytes as COMPOUND_TEXT or borrow a caller's buffer on this path.
        _ => return CONVERTER_NOT_FOUND,
    };
    unsafe { convert(list, count, output, encoding, atom) }.map_or_else(|error| error, |()| 0)
}

#[unsafe(export_name = "kinakaze_engine_libX11_XmbTextListToTextProperty")]
pub unsafe extern "sysv64" fn XmbTextListToTextProperty(
    display: *mut Display,
    list: *mut *mut c_char,
    count: c_int,
    style: c_int,
    output: *mut XTextProperty,
) -> c_int {
    // The hosted locale's multibyte representation is UTF-8.
    unsafe { Xutf8TextListToTextProperty(display, list, count, style, output) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XmbTextPropertyToTextList")]
pub unsafe extern "sysv64" fn XmbTextPropertyToTextList(
    display: *mut Display,
    property: *const XTextProperty,
    output: *mut *mut *mut c_char,
    count: *mut c_int,
) -> c_int {
    let utf8 = libc::locale::kinakaze_abi___ctype_get_mb_cur_max() > 1;
    unsafe { text_property_to_list(display, property, output, count, utf8) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_Xutf8TextPropertyToTextList")]
pub unsafe extern "sysv64" fn Xutf8TextPropertyToTextList(
    display: *mut Display,
    property: *const XTextProperty,
    output: *mut *mut *mut c_char,
    count: *mut c_int,
) -> c_int {
    // This entry point always returns UTF-8, even in the C locale.
    unsafe { text_property_to_list(display, property, output, count, true) }
}

unsafe fn text_property_to_list(
    display: *mut Display,
    property: *const XTextProperty,
    output: *mut *mut *mut c_char,
    count: *mut c_int,
    utf8: bool,
) -> c_int {
    unsafe {
        *output = ptr::null_mut();
        *count = 0;
    }
    let p = unsafe { &*property };
    if p.format != 8 || (p.nitems != 0 && p.value.is_null()) {
        return CONVERTER_NOT_FOUND;
    }
    if p.nitems == 0 {
        return 0;
    }
    if p.nitems > isize::MAX as u64 {
        return NO_MEMORY;
    }
    let input = unsafe { core::slice::from_raw_parts(p.value, p.nitems as usize) };
    let bytes;
    let input = if p.encoding == XA_STRING {
        if !utf8 && input.iter().any(|b| *b >= 128) {
            return CONVERTER_NOT_FOUND;
        }
        bytes = input
            .iter()
            .map(|&b| char::from(b))
            .collect::<String>()
            .into_bytes();
        &bytes[..]
    } else if p.encoding == unsafe { crate::XInternAtom(display, c"UTF8_STRING".as_ptr(), 0) } {
        if std::str::from_utf8(input).is_err() || !utf8 && input.iter().any(|b| *b >= 128) {
            return CONVERTER_NOT_FOUND;
        }
        input
    } else {
        return CONVERTER_NOT_FOUND;
    };
    let raw = XTextProperty {
        value: input.as_ptr().cast_mut(),
        encoding: XA_STRING,
        format: 8,
        nitems: input.len() as u64,
    };
    if unsafe { XTextPropertyToStringList(&raw, output, count) } == 0 {
        NO_MEMORY
    } else {
        0
    }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XwcTextListToTextProperty")]
pub unsafe extern "sysv64" fn XwcTextListToTextProperty(
    d: *mut Display,
    list: *mut *mut i32,
    count: c_int,
    style: c_int,
    output: *mut XTextProperty,
) -> c_int {
    if count < 0 || count > 0 && list.is_null() {
        return CONVERTER_NOT_FOUND;
    }
    let mut strings = Vec::new();
    for index in 0..count as usize {
        let mut p = unsafe { *list.add(index) };
        let mut string = String::new();
        if !p.is_null() {
            while unsafe { *p } != 0 {
                let Some(ch) = char::from_u32(unsafe { *p } as u32) else {
                    return CONVERTER_NOT_FOUND;
                };
                string.push(ch);
                p = unsafe { p.add(1) };
            }
        }
        strings.push(std::ffi::CString::new(string).unwrap());
    }
    let mut pointers: Vec<_> = strings.iter().map(|s| s.as_ptr().cast_mut()).collect();
    unsafe { Xutf8TextListToTextProperty(d, pointers.as_mut_ptr(), count, style, output) }
}

#[unsafe(export_name = "kinakaze_engine_libX11_XwcTextPropertyToTextList")]
pub unsafe extern "sysv64" fn XwcTextPropertyToTextList(
    d: *mut Display,
    property: *const XTextProperty,
    output: *mut *mut *mut i32,
    count: *mut c_int,
) -> c_int {
    if output.is_null() || count.is_null() || property.is_null() {
        return CONVERTER_NOT_FOUND;
    }
    unsafe {
        *output = ptr::null_mut();
        *count = 0;
    }
    let (mut list, mut n) = (ptr::null_mut(), 0);
    let status = unsafe { Xutf8TextPropertyToTextList(d, property, &mut list, &mut n) };
    if status != 0 || n == 0 {
        return status;
    }
    let strings: Vec<Vec<i32>> = (0..n as usize)
        .map(|i| {
            unsafe { CStr::from_ptr(*list.add(i)) }
                .to_str()
                .unwrap()
                .chars()
                .map(|c| c as i32)
                .chain([0])
                .collect()
        })
        .collect();
    unsafe {
        XFreeStringList(list);
    }
    let pointers = unsafe {
        kinakaze_alloc::guest::malloc((n as usize + 1) * core::mem::size_of::<*mut i32>())
    }
    .cast::<*mut i32>();
    let units: usize = strings.iter().map(Vec::len).sum();
    let storage = unsafe { kinakaze_alloc::guest::malloc(units * 4) }.cast::<i32>();
    if pointers.is_null() || storage.is_null() {
        unsafe {
            kinakaze_alloc::guest::free(pointers.cast());
            kinakaze_alloc::guest::free(storage.cast());
        }
        return NO_MEMORY;
    }
    let mut cursor = storage;
    for (i, string) in strings.iter().enumerate() {
        unsafe {
            *pointers.add(i) = cursor;
            ptr::copy_nonoverlapping(string.as_ptr(), cursor, string.len());
            cursor = cursor.add(string.len());
        }
    }
    unsafe {
        *pointers.add(n as usize) = ptr::null_mut();
        *output = pointers;
        *count = n;
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_libX11_XwcFreeStringList")]
pub unsafe extern "sysv64" fn XwcFreeStringList(list: *mut *mut i32) {
    unsafe {
        XFreeStringList(list.cast());
    }
}
