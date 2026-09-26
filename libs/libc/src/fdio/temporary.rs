//! Legacy name-only temporary paths; callers own the returned guest allocation.
use super::*;

/// # Safety
/// Non-null arguments are readable NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tempnam(
    directory: *const c_char,
    prefix: *const c_char,
) -> *mut c_char {
    let environment = unsafe { crate::process::kinakaze_abi_secure_getenv(c"TMPDIR".as_ptr()) };
    let mut candidates = Vec::new();
    for value in [environment.cast_const(), directory, c"/tmp".as_ptr()] {
        if !value.is_null() {
            if let Ok(path) = unsafe { CStr::from_ptr(value) }.to_str() {
                if !path.is_empty() {
                    candidates.push(path.to_owned());
                }
            }
        }
    }
    let Some(directory) = candidates
        .into_iter()
        .find(|path| fs::stat(path).is_ok_and(|stat| stat.st_mode & 0o170000 == 0o040000))
    else {
        set_errno(2);
        return ptr::null_mut();
    };
    let prefix = if prefix.is_null() {
        b"".as_slice()
    } else {
        unsafe { CStr::from_ptr(prefix) }.to_bytes()
    };
    let mut template = directory.trim_end_matches('/').as_bytes().to_vec();
    template.push(b'/');
    template.extend_from_slice(&prefix[..prefix.len().min(5)]);
    let marks = template.len();
    template.extend_from_slice(b"XXXXXX\0");
    for _ in 0..TEMPLATE_ATTEMPTS {
        if let Err(error) = unsafe { write_suffix(template.as_mut_ptr().cast(), marks) } {
            set_errno(error);
            return ptr::null_mut();
        }
        let path = match core::str::from_utf8(&template[..template.len() - 1]) {
            Ok(path) => path,
            Err(_) => {
                set_errno(84);
                return ptr::null_mut();
            }
        };
        match fs::lstat(path) {
            Ok(_) => continue,
            Err(2) => {
                let result = unsafe { kinakaze_alloc::guest::malloc(template.len()) };
                if result.is_null() {
                    set_errno(ENOMEM);
                    return ptr::null_mut();
                }
                unsafe {
                    ptr::copy_nonoverlapping(template.as_ptr(), result, template.len());
                }
                return result.cast();
            }
            Err(error) => {
                set_errno(error);
                return ptr::null_mut();
            }
        }
    }
    set_errno(EEXIST);
    ptr::null_mut()
}
