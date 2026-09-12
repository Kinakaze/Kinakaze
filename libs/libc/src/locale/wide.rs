//! Wide classification and simple case mappings selected by the caller's locale.
use super::*;
fn ascii(scalar: u32, class: usize) -> bool {
    if scalar >= 128 {
        return false;
    }
    let c = scalar as u8;
    match class {
        0 => c.is_ascii_uppercase(),
        1 => c.is_ascii_lowercase(),
        2 => c.is_ascii_alphabetic(),
        3 => c.is_ascii_digit(),
        4 => c.is_ascii_hexdigit(),
        5 => matches!(c, b' ' | b'\t'..=b'\r'),
        6 => (0x20..=0x7e).contains(&c),
        7 => c.is_ascii_graphic(),
        8 => matches!(c, b' ' | b'\t'),
        9 => c.is_ascii_control(),
        10 => c.is_ascii_punctuation(),
        11 => c.is_ascii_alphanumeric(),
        _ => false,
    }
}
fn classify(scalar: u32, class: usize, locale: usize) -> c_int {
    if class >= data::CLASSES.len() {
        return 0;
    }
    if flags(locale) & 1 == 0 {
        return ascii(scalar, class) as c_int;
    }
    unicode_data().classify(scalar, class) as c_int
}
fn map(scalar: u32, upper: bool, locale: usize) -> u32 {
    if flags(locale) & 1 != 0 {
        return unicode_data().map(scalar, upper);
    }
    if scalar >= 128 {
        return scalar;
    }
    if upper {
        (scalar as u8).to_ascii_uppercase() as u32
    } else {
        (scalar as u8).to_ascii_lowercase() as u32
    }
}
macro_rules! classification {
    ($plain:ident,$localized:ident,$class:expr) => {
        #[unsafe(no_mangle)]
        pub extern "sysv64" fn $plain(scalar: u32) -> c_int {
            classify(scalar, $class, kinakaze_tls::locale())
        }
        #[unsafe(no_mangle)]
        pub extern "sysv64" fn $localized(scalar: u32, locale: *mut c_void) -> c_int {
            classify(scalar, $class, locale as usize)
        }
    };
}
classification!(kinakaze_abi_iswupper, kinakaze_abi_iswupper_l, 0);
classification!(kinakaze_abi_iswlower, kinakaze_abi_iswlower_l, 1);
classification!(kinakaze_abi_iswalpha, kinakaze_abi_iswalpha_l, 2);
classification!(kinakaze_abi_iswdigit, kinakaze_abi_iswdigit_l, 3);
classification!(kinakaze_abi_iswxdigit, kinakaze_abi_iswxdigit_l, 4);
classification!(kinakaze_abi_iswspace, kinakaze_abi_iswspace_l, 5);
classification!(kinakaze_abi_iswprint, kinakaze_abi_iswprint_l, 6);
classification!(kinakaze_abi_iswgraph, kinakaze_abi_iswgraph_l, 7);
classification!(kinakaze_abi_iswblank, kinakaze_abi_iswblank_l, 8);
classification!(kinakaze_abi_iswcntrl, kinakaze_abi_iswcntrl_l, 9);
classification!(kinakaze_abi_iswpunct, kinakaze_abi_iswpunct_l, 10);
classification!(kinakaze_abi_iswalnum, kinakaze_abi_iswalnum_l, 11);

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_towupper(scalar: u32) -> u32 {
    map(scalar, true, kinakaze_tls::locale())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_towlower(scalar: u32) -> u32 {
    map(scalar, false, kinakaze_tls::locale())
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_towupper_l(scalar: u32, locale: *mut c_void) -> u32 {
    map(scalar, true, locale as usize)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_towlower_l(scalar: u32, locale: *mut c_void) -> u32 {
    map(scalar, false, locale as usize)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wctype_l(
    name: *const c_char,
    locale: *mut c_void,
) -> usize {
    if name.is_null() {
        return 0;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    data::CLASSES
        .iter()
        .position(|value| *value == name)
        .map_or(0, |class| {
            class + 1 | (((flags(locale as usize) & 1) as usize) << 8)
        })
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wctype(name: *const c_char) -> usize {
    unsafe { kinakaze_abi_wctype_l(name, kinakaze_tls::locale() as _) }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_iswctype(scalar: u32, descriptor: usize) -> c_int {
    let Some(class) = (descriptor & 255).checked_sub(1) else {
        return 0;
    };
    if class >= data::CLASSES.len() || descriptor & !0x1ff != 0 {
        return 0;
    }
    if descriptor & 256 != 0 {
        unicode_data().classify(scalar, class) as c_int
    } else {
        ascii(scalar, class) as c_int
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_iswctype_l(
    scalar: u32,
    descriptor: usize,
    _locale: *mut c_void,
) -> c_int {
    kinakaze_abi_iswctype(scalar, descriptor)
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wctrans_l(
    name: *const c_char,
    locale: *mut c_void,
) -> usize {
    if name.is_null() {
        return 0;
    }
    let kind = match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"toupper" => 1,
        b"tolower" => 2,
        _ => return 0,
    };
    kind | (((flags(locale as usize) & 1) as usize) << 8)
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wctrans(name: *const c_char) -> usize {
    unsafe { kinakaze_abi_wctrans_l(name, kinakaze_tls::locale() as _) }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_towctrans(scalar: u32, descriptor: usize) -> u32 {
    if !matches!(descriptor & 255, 1 | 2) || descriptor & !0x1ff != 0 {
        return scalar;
    }
    let upper = descriptor & 255 == 1;
    if descriptor & 256 != 0 {
        unicode_data().map(scalar, upper)
    } else if scalar < 128 {
        if upper {
            (scalar as u8).to_ascii_uppercase() as u32
        } else {
            (scalar as u8).to_ascii_lowercase() as u32
        }
    } else {
        scalar
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_towctrans_l(
    scalar: u32,
    descriptor: usize,
    _locale: *mut c_void,
) -> u32 {
    kinakaze_abi_towctrans(scalar, descriptor)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_wcwidth(scalar: u32) -> c_int {
    if scalar == 0 {
        return 0;
    }
    if utf8() {
        unicode_data().width(scalar)
    } else if ascii(scalar, 6) {
        1
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strcoll_l(
    a: *const c_char,
    b: *const c_char,
    _locale: *mut c_void,
) -> c_int {
    // Both supported collation profiles order bytes directly.
    unsafe { crate::string::strcmp(a, b) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_strxfrm_l(
    dst: *mut c_char,
    src: *const c_char,
    capacity: usize,
    _locale: *mut c_void,
) -> usize {
    let text = unsafe { CStr::from_ptr(src) }.to_bytes_with_nul();
    if capacity != 0 {
        unsafe {
            ptr::copy_nonoverlapping(src, dst, text.len().min(capacity));
        }
    }
    text.len() - 1
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wcscoll_l(
    a: *const i32,
    b: *const i32,
    _locale: *mut c_void,
) -> c_int {
    unsafe { crate::strextra::windows::kinakaze_abi_wcscmp(a, b) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_wcsxfrm_l(
    dst: *mut i32,
    src: *const i32,
    capacity: usize,
    _locale: *mut c_void,
) -> usize {
    let length = unsafe { crate::strextra::windows::kinakaze_abi_wcslen(src) };
    if capacity != 0 {
        unsafe {
            ptr::copy_nonoverlapping(src, dst, (length + 1).min(capacity));
        }
    }
    length
}

alias!(kinakaze_abi___wctype_l,kinakaze_abi_wctype_l,(name:*const c_char,locale:*mut c_void)->usize);
alias!(kinakaze_abi___iswctype_l,kinakaze_abi_iswctype_l,(scalar:u32,descriptor:usize,locale:*mut c_void)->c_int);
alias!(kinakaze_abi___towupper_l,kinakaze_abi_towupper_l,(scalar:u32,locale:*mut c_void)->u32);
alias!(kinakaze_abi___towlower_l,kinakaze_abi_towlower_l,(scalar:u32,locale:*mut c_void)->u32);
alias!(kinakaze_abi___strcoll_l,kinakaze_abi_strcoll_l,(a:*const c_char,b:*const c_char,locale:*mut c_void)->c_int);
alias!(kinakaze_abi___strxfrm_l,kinakaze_abi_strxfrm_l,(dst:*mut c_char,src:*const c_char,n:usize,locale:*mut c_void)->usize);
alias!(kinakaze_abi___wcscoll_l,kinakaze_abi_wcscoll_l,(a:*const i32,b:*const i32,locale:*mut c_void)->c_int);
alias!(kinakaze_abi___wcsxfrm_l,kinakaze_abi_wcsxfrm_l,(dst:*mut i32,src:*const i32,n:usize,locale:*mut c_void)->usize);
