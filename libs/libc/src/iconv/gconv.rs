//! glibc 2.36 iconv program entry points over our canonical converter.
//! gconv_spec and gconv_module follow iconv/gconv_int.h; handles remain opaque
//! to iconv(1), which passes them back to our iconv/iconv_close entry points.
use super::*;

#[repr(C)]
pub struct Spec {
    from: *mut c_char,
    to: *mut c_char,
    translit: bool,
    ignore: bool,
}

pub(super) unsafe fn split(name: *const c_char) -> Result<(Vec<u8>, bool, bool), i32> {
    if name.is_null() {
        return Err(EINVAL);
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    let parts: Vec<_> = name.split(|&ch| ch == b'/' || ch == b',').collect();
    let code = parts[0].to_ascii_uppercase();
    Ok((
        code,
        parts[1..]
            .iter()
            .any(|part| part.eq_ignore_ascii_case(b"TRANSLIT")),
        parts[1..]
            .iter()
            .any(|part| part.eq_ignore_ascii_case(b"IGNORE")),
    ))
}
unsafe fn allocate(bytes: &[u8]) -> Result<*mut c_char, i32> {
    let result = unsafe { guest::malloc(bytes.len() + 1) };
    if result.is_null() {
        return Err(ENOMEM);
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), result, bytes.len());
        result.add(bytes.len()).write(0);
    }
    Ok(result.cast())
}
/// # Safety
/// Strings are readable and output is a writable gconv_spec.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___gconv_create_spec(
    output: *mut Spec,
    from: *const c_char,
    to: *const c_char,
) -> *mut Spec {
    let result = (|| {
        if output.is_null() {
            return Err(EINVAL);
        }
        let (from, _, _) = unsafe { split(from) }?;
        let (to, translit, ignore) = unsafe { split(to) }?;
        let from = unsafe { allocate(&from) }?;
        let to = match unsafe { allocate(&to) } {
            Ok(to) => to,
            Err(e) => {
                unsafe {
                    guest::free(from.cast());
                }
                return Err(e);
            }
        };
        unsafe {
            output.write(Spec {
                from,
                to,
                translit,
                ignore,
            });
        }
        Ok(output)
    })();
    result.unwrap_or_else(|e| {
        crate::set_errno(e);
        ptr::null_mut()
    })
}
/// # Safety
/// spec was initialized by create_spec and has not yet been destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___gconv_destroy_spec(spec: *mut Spec) {
    if !spec.is_null() {
        unsafe {
            guest::free((*spec).from.cast());
            guest::free((*spec).to.cast());
            (*spec).from = ptr::null_mut();
            (*spec).to = ptr::null_mut();
        }
    }
}
/// # Safety
/// spec is initialized and handle is writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___gconv_open(
    spec: *const Spec,
    handle: *mut *mut c_void,
    flags: c_int,
) -> c_int {
    if spec.is_null() || handle.is_null() {
        error(EINVAL);
        return 8;
    }
    unsafe {
        handle.write(ptr::null_mut());
    }
    let spec = unsafe { &*spec };
    let value = unsafe { kinakaze_abi_iconv_open(spec.to, spec.from) };
    if value as usize == FAILED {
        return if unsafe { *crate::kinakaze_abi___errno_location() } == ENOMEM {
            3
        } else {
            1
        };
    }
    let converter = unsafe { &mut *value.cast::<Converter>() };
    if flags & 1 != 0 && converter.from == converter.to {
        unsafe {
            kinakaze_abi_iconv_close(value);
        }
        return -1;
    }
    converter.ignore = spec.ignore;
    converter.translit = spec.translit;
    unsafe {
        handle.write(value);
    }
    0
}

/// C-locale transliteration uses the default missing-character replacement.
/// The caller's conversion step encodes it, including stateful codecs and
/// output exhaustion; UTF-32 input advances only after successful conversion.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___gconv_transliterate(
    step: *mut c_void,
    data: *mut c_void,
    _input_start: *const u8,
    input: *mut *const u8,
    end: *const u8,
    output: *mut *mut u8,
    irreversible: *mut usize,
) -> c_int {
    if step.is_null()
        || data.is_null()
        || input.is_null()
        || output.is_null()
        || irreversible.is_null()
    {
        return 8;
    }
    let current = unsafe { *input };
    if current == end {
        return 4;
    }
    if (end as usize).saturating_sub(current as usize) < 4 {
        return 7;
    }
    let mut address = unsafe { step.cast::<u8>().add(40).cast::<usize>().read() };
    if unsafe { step.cast::<usize>().read() } != 0 {
        let tp = kinakaze_tls::get_current_thread_fs_base();
        if tp == 0 {
            return 8;
        }
        address = address.rotate_right(17) ^ unsafe { ((tp + 0x30) as *const usize).read() };
    }
    if address == 0 {
        return 8;
    }
    type Convert = unsafe extern "sysv64" fn(
        *mut c_void,
        *mut c_void,
        *mut *const u8,
        *const u8,
        *mut *mut u8,
        *mut usize,
        c_int,
        c_int,
    ) -> c_int;
    let convert: Convert = unsafe { core::mem::transmute(address) };
    let replacement = '?' as u32;
    let mut source = (&raw const replacement).cast::<u8>();
    let mut destination = unsafe { *output };
    let status = unsafe {
        convert(
            step,
            data,
            &mut source,
            source.add(4),
            &mut destination,
            ptr::null_mut(),
            0,
            0,
        )
    };
    if status != 5 {
        unsafe {
            output.write(destination);
        }
    }
    if status == 4 {
        unsafe {
            input.write(current.add(4));
            *irreversible += 1;
        }
        0
    } else {
        status
    }
}

// The built-in conversion graph has no loadable-module cache or separate alias
// tree. Enumerate every supported spelling as a graph node so iconv -l reports
// the same codecs that iconv_open accepts, rather than Debian's unimplemented
// gconv module inventory.
const NAMES: &[&CStr] = &[
    c"ASCII//",
    c"US-ASCII//",
    c"ANSI_X3.4-1968//",
    c"ANSI_X3.4-1986//",
    c"ISO646-US//",
    c"UTF-8//",
    c"ISO-8859-1//",
    c"LATIN1//",
    c"L1//",
    c"UTF-16LE//",
    c"UTF-16BE//",
    c"UTF-32LE//",
    c"UTF-32BE//",
    c"UCS-4LE//",
    c"UCS-4BE//",
    c"WCHAR_T//",
];
#[derive(Clone, Copy)]
#[repr(C)]
pub struct Module {
    from: *const c_char,
    to: *const c_char,
    cost_hi: i32,
    cost_lo: i32,
    name: *const c_char,
    left: *const Module,
    same: *const Module,
    right: *const Module,
}
struct Modules([Module; NAMES.len()]);
// Immutable records and strings in this native module's image.
unsafe impl Sync for Modules {}
const fn modules() -> [Module; NAMES.len()] {
    let mut list = [Module {
        from: ptr::null(),
        to: c"UTF-8//".as_ptr(),
        cost_hi: 1,
        cost_lo: 1,
        name: c"builtin".as_ptr(),
        left: ptr::null(),
        same: ptr::null(),
        right: ptr::null(),
    }; NAMES.len()];
    let mut i = 0;
    while i < NAMES.len() {
        list[i].from = NAMES[i].as_ptr();
        if i + 1 < NAMES.len() {
            list[i].same = &MODULES.0[i + 1];
        }
        i += 1;
    }
    list
}
static MODULES: Modules = Modules(modules());
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___gconv_get_cache() -> *const c_void {
    ptr::null()
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___gconv_get_alias_db() -> *const c_void {
    ptr::null()
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi___gconv_get_modules_db() -> *const Module {
    MODULES.0.as_ptr()
}
