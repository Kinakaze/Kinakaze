//! Locale ownership and selection. C is built in; C.UTF-8 uses installed,
//! validated GNU locale data. Unsupported names fail without substitution.
mod data;
mod lifecycle;
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
};
use kinakaze_alloc::guest;
use kinakaze_vfs::{EINVAL, ENOENT, ENOMEM};
use std::sync::{
    Mutex,
    atomic::{AtomicU32, AtomicUsize, Ordering},
};

const LC_ALL: usize = 6;
const CATEGORIES: usize = 13;
const MASK: u32 = ((1 << CATEGORIES) - 1) & !(1 << LC_ALL);
const GLOBAL: usize = usize::MAX;
const MAGIC: u64 = u64::from_le_bytes(*b"CYLOC001");
const CATEGORY_NAMES: [&CStr; CATEGORIES] = [
    c"LC_CTYPE",
    c"LC_NUMERIC",
    c"LC_TIME",
    c"LC_COLLATE",
    c"LC_MONETARY",
    c"LC_MESSAGES",
    c"LC_ALL",
    c"LC_PAPER",
    c"LC_NAME",
    c"LC_ADDRESS",
    c"LC_TELEPHONE",
    c"LC_MEASUREMENT",
    c"LC_IDENTIFICATION",
];
static GLOBAL_FLAGS: AtomicU32 = AtomicU32::new(0);
static UTF8_DATA: AtomicUsize = AtomicUsize::new(0);
static MUTATION: Mutex<()> = Mutex::new(());

/// glibc exposes this prefix through locale_t and its ctype macros.
#[repr(C)]
struct Locale {
    categories: [usize; CATEGORIES],
    ctype_b: *const u16,
    ctype_tolower: *const c_int,
    ctype_toupper: *const c_int,
    names: [*const c_char; CATEGORIES],
    magic: u64,
    flags: u32,
}
impl Locale {
    fn new(flags: u32) -> Self {
        Self {
            categories: [0; CATEGORIES],
            // C and UTF-8 classify individual bytes using the same ASCII tables.
            ctype_b: unsafe { *crate::startup::kinakaze_abi___ctype_b_loc() },
            ctype_tolower: unsafe { *crate::startup::kinakaze_abi___ctype_tolower_loc() },
            ctype_toupper: unsafe { *crate::startup::kinakaze_abi___ctype_toupper_loc() },
            names: core::array::from_fn(|index| name(flags & (1 << index) != 0).as_ptr()),
            magic: MAGIC,
            flags,
        }
    }
}
fn name(utf8: bool) -> &'static CStr {
    if utf8 { c"C.UTF-8" } else { c"C" }
}

pub(crate) fn flags(handle: usize) -> u32 {
    if handle == GLOBAL {
        GLOBAL_FLAGS.load(Ordering::Acquire)
    } else {
        unsafe { (*(handle as *const Locale)).flags }
    }
}
pub(crate) fn utf8() -> bool {
    flags(kinakaze_tls::locale()) & 1 != 0
}
fn valid(handle: usize) -> bool {
    handle == GLOBAL
        || (handle % core::mem::align_of::<Locale>() == 0
            && guest::contains(handle)
            && handle
                .checked_add(core::mem::size_of::<Locale>() - 1)
                .is_some_and(guest::contains)
            && unsafe { (*(handle as *const Locale)).magic == MAGIC })
}
fn error(code: i32) -> *mut c_void {
    crate::set_errno(code);
    ptr::null_mut()
}
fn parse_name(value: &[u8]) -> Result<bool, i32> {
    match value {
        b"C" | b"POSIX" => Ok(false),
        b"C.UTF-8" | b"C.utf8" => Ok(true),
        _ => Err(ENOENT),
    }
}
fn environment_name(category: usize) -> Result<bool, i32> {
    for key in [c"LC_ALL", CATEGORY_NAMES[category], c"LANG"] {
        let value = unsafe { crate::process::kinakaze_abi_getenv(key.as_ptr()) };
        if !value.is_null() {
            let bytes = unsafe { CStr::from_ptr(value) }.to_bytes();
            if !bytes.is_empty() {
                return parse_name(bytes);
            }
        }
    }
    Ok(false)
}
fn selected(mask: u32, value: &[u8], mut flags: u32) -> Result<u32, i32> {
    for category in 0..CATEGORIES {
        let bit = 1 << category;
        if mask & bit == 0 {
            continue;
        }
        let utf8 = if value.is_empty() {
            environment_name(category)?
        } else if value.contains(&b'=') {
            let mut found = None;
            for pair in value.split(|c| *c == b';') {
                let Some(separator) = pair.iter().position(|c| *c == b'=') else {
                    return Err(EINVAL);
                };
                let (key, rest) = pair.split_at(separator);
                let val = &rest[1..];
                if key == CATEGORY_NAMES[category].to_bytes() {
                    if found.is_some() {
                        return Err(EINVAL);
                    }
                    found = Some(parse_name(val)?);
                }
            }
            found.ok_or(EINVAL)?
        } else {
            parse_name(value)?
        };
        if utf8 {
            flags |= bit;
        } else {
            flags &= !bit;
        }
    }
    Ok(flags)
}

// One immutable guest allocation, loaded only on first C.UTF-8 request. Both
// the explicit locale handles and this buffer retain their addresses on fork.
fn require_data(flags: u32) -> Result<(), i32> {
    if flags & 1 == 0 || UTF8_DATA.load(Ordering::Acquire) != 0 {
        return Ok(());
    }
    let bytes = data::load()?;
    data::Data::parse(&bytes)?;
    let length = bytes.len();
    let allocation = unsafe { guest::malloc(core::mem::size_of::<usize>() + length) };
    if allocation.is_null() {
        return Err(ENOMEM);
    }
    unsafe {
        allocation.cast::<usize>().write(length);
        ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            allocation.add(core::mem::size_of::<usize>()),
            length,
        );
    }
    UTF8_DATA.store(allocation as usize, Ordering::Release);
    Ok(())
}
fn unicode_data() -> data::Data<'static> {
    let pointer = UTF8_DATA.load(Ordering::Acquire) as *const usize;
    // A UTF-8 locale can be published only after the immutable input validates.
    assert!(!pointer.is_null());
    unsafe { data::Data::validated(core::slice::from_raw_parts(pointer.add(1).cast(), *pointer)) }
}
fn allocate(flags: u32) -> *mut c_void {
    let allocation = unsafe { guest::malloc(core::mem::size_of::<Locale>()) }.cast::<Locale>();
    if allocation.is_null() {
        return error(ENOMEM);
    }
    unsafe {
        allocation.write(Locale::new(flags));
    }
    allocation.cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_newlocale(
    mask: c_int,
    value: *const c_char,
    base: *mut c_void,
) -> *mut c_void {
    // GNU libstdc++ uses the legacy single LC_ALL bit.
    let mask = if mask == 1 << LC_ALL {
        MASK as c_int
    } else {
        mask
    };
    if mask as u32 & !MASK != 0
        || value.is_null()
        || base as usize == GLOBAL
        || (!base.is_null() && !valid(base as usize))
    {
        return error(EINVAL);
    }
    let initial = if base.is_null() {
        0
    } else {
        flags(base as usize)
    };
    let value = unsafe { CStr::from_ptr(value) }.to_bytes();
    let _guard = MUTATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let updated = match selected(mask as u32, value, initial).and_then(|f| {
        require_data(f)?;
        Ok(f)
    }) {
        Ok(flags) => flags,
        Err(e) => return error(e),
    };
    if base.is_null() {
        allocate(updated)
    } else {
        unsafe {
            base.cast::<Locale>().write(Locale::new(updated));
        }
        base
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_freelocale(locale: *mut c_void) {
    if locale.is_null() || locale as usize == GLOBAL {
        return;
    }
    unsafe {
        guest::free(locale.cast());
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_duplocale(locale: *mut c_void) -> *mut c_void {
    if !valid(locale as usize) {
        return error(EINVAL);
    }
    allocate(flags(locale as usize))
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_uselocale(locale: *mut c_void) -> *mut c_void {
    let previous = kinakaze_tls::locale();
    if !locale.is_null() {
        if !valid(locale as usize) {
            return error(EINVAL);
        }
        kinakaze_tls::set_locale(locale as usize);
    }
    previous as *mut c_void
}

static mut QUERY: [u8; 512] = [0; 512];
fn query(flags: u32, category: usize) -> *mut c_char {
    if category != LC_ALL {
        return name(flags & (1 << category) != 0).as_ptr().cast_mut();
    }
    if flags == 0 || flags == MASK {
        return name(flags != 0).as_ptr().cast_mut();
    }
    let mut output = [0u8; 512];
    let mut offset = 0;
    for (i, key) in CATEGORY_NAMES.iter().enumerate() {
        if i == LC_ALL {
            continue;
        }
        if offset != 0 {
            output[offset] = b';';
            offset += 1;
        }
        for bytes in [key.to_bytes(), b"=", name(flags & (1 << i) != 0).to_bytes()] {
            output[offset..offset + bytes.len()].copy_from_slice(bytes);
            offset += bytes.len();
        }
    }
    unsafe {
        ptr::addr_of_mut!(QUERY).write(output);
        ptr::addr_of_mut!(QUERY).cast()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setlocale(
    category: c_int,
    value: *const c_char,
) -> *mut c_char {
    if !(0..CATEGORIES as c_int).contains(&category) {
        return error(EINVAL).cast();
    }
    let _guard = MUTATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut flags = GLOBAL_FLAGS.load(Ordering::Acquire);
    if !value.is_null() {
        let mask = if category as usize == LC_ALL {
            MASK
        } else {
            1 << category
        };
        let value = unsafe { CStr::from_ptr(value) }.to_bytes();
        flags = match selected(mask, value, flags).and_then(|f| {
            require_data(f)?;
            Ok(f)
        }) {
            Ok(flags) => flags,
            Err(e) => return error(e).cast(),
        };
        GLOBAL_FLAGS.store(flags, Ordering::Release);
    }
    query(flags, category as usize)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___ctype_get_mb_cur_max() -> usize {
    if utf8() { 6 } else { 1 }
}

macro_rules! alias {
    (safe $name:ident,$target:ident,($($arg:ident:$ty:ty),*)->$ret:ty) => {
        #[unsafe(no_mangle)]
        pub extern "sysv64" fn $name($($arg:$ty),*)->$ret { $target($($arg),*) }
    };
    ($name:ident,$target:ident,($($arg:ident:$ty:ty),*)->$ret:ty) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "sysv64" fn $name($($arg:$ty),*)->$ret { unsafe { $target($($arg),*) } }
    }
}
alias!(kinakaze_abi___newlocale,kinakaze_abi_newlocale,(mask:c_int,value:*const c_char,base:*mut c_void)->*mut c_void);
alias!(kinakaze_abi___duplocale,kinakaze_abi_duplocale,(locale:*mut c_void)->*mut c_void);
alias!(kinakaze_abi___freelocale,kinakaze_abi_freelocale,(locale:*mut c_void)->());
alias!(kinakaze_abi___uselocale,kinakaze_abi_uselocale,(locale:*mut c_void)->*mut c_void);

pub mod wide;

pub mod info;

// Unit tests for the encoding algorithms do not need filesystem locale data.
// This guard owns its stable handle and restores the test thread's selection.
#[cfg(test)]
pub(crate) struct TestLocale {
    previous: usize,
    _handle: Box<Locale>,
}
#[cfg(test)]
impl TestLocale {
    pub(crate) fn new(utf8: bool) -> Self {
        let handle = Box::new(Locale::new(if utf8 { MASK } else { 0 }));
        let previous = kinakaze_tls::locale();
        kinakaze_tls::set_locale((&*handle as *const Locale) as usize);
        Self {
            previous,
            _handle: handle,
        }
    }
}
#[cfg(test)]
impl Drop for TestLocale {
    fn drop(&mut self) {
        kinakaze_tls::set_locale(self.previous);
    }
}
