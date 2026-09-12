//! Filename expansion against the guest VFS; returned strings use guest memory.
use core::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
};
use kinakaze_vfs::fs;
use std::ffi::CString;

type ErrorCallback = Option<unsafe extern "sysv64" fn(*const c_char, c_int) -> c_int>;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_glob_pattern_p(
    pattern: *const c_char,
    quote: c_int,
) -> c_int {
    if pattern.is_null() {
        return 0;
    }
    let bytes = unsafe { CStr::from_ptr(pattern) }.to_bytes();
    let (mut at, mut bracket) = (0, false);
    while at < bytes.len() {
        match bytes[at] {
            b'?' | b'*' => return 1,
            b'\\' if quote != 0 => at += usize::from(at + 1 < bytes.len()),
            b'[' => bracket = true,
            b']' if bracket => return 1,
            _ => (),
        }
        at += 1;
    }
    0
}
#[repr(C)]
pub struct GlobT {
    pub gl_pathc: usize,
    pub gl_pathv: *mut *mut c_char,
    pub gl_offs: usize,
    pub gl_flags: c_int,
    pub gl_closedir: Option<unsafe extern "sysv64" fn(*mut c_void)>,
    pub gl_readdir: Option<unsafe extern "sysv64" fn(*mut c_void) -> *mut c_void>,
    pub gl_opendir: Option<unsafe extern "sysv64" fn(*const c_char) -> *mut c_void>,
    pub gl_lstat: Option<unsafe extern "sysv64" fn(*const c_char, *mut c_void) -> c_int>,
    pub gl_stat: Option<unsafe extern "sysv64" fn(*const c_char, *mut c_void) -> c_int>,
}

fn expand(pattern: &str, flags: c_int, error: ErrorCallback) -> Result<Vec<String>, c_int> {
    let mut paths = vec![if pattern.starts_with('/') {
        "/".to_owned()
    } else {
        String::new()
    }];
    for component in pattern.split('/').filter(|part| !part.is_empty()) {
        let mut next = Vec::new();
        let pattern = CString::new(component).unwrap();
        for base in paths {
            let directory = if base.is_empty() { "." } else { &base };
            let entries = match fs::read_directory(directory) {
                Ok(entries) => entries,
                Err(code) => {
                    if code == kinakaze_vfs::ENOENT || code == kinakaze_vfs::ENOTDIR {
                        continue;
                    }
                    let name = CString::new(directory).unwrap();
                    let abort =
                        error.is_some_and(|callback| unsafe { callback(name.as_ptr(), code) } != 0);
                    if abort || flags & 1 != 0 {
                        crate::set_errno(code);
                        return Err(2);
                    }
                    continue;
                }
            };
            for entry in entries {
                let name = CString::new(entry.name.as_bytes()).unwrap();
                let match_flags =
                    if flags & 64 != 0 { 2 } else { 0 } | if flags & 128 == 0 { 4 } else { 0 };
                if unsafe {
                    crate::fsextra::kinakaze_abi_fnmatch(
                        pattern.as_ptr(),
                        name.as_ptr(),
                        match_flags,
                    )
                } == 0
                {
                    next.push(if base.is_empty() || base.ends_with('/') {
                        format!("{base}{}", entry.name)
                    } else {
                        format!("{base}/{}", entry.name)
                    });
                }
            }
        }
        paths = next;
    }
    let directory_only = flags & 8192 != 0 || pattern.ends_with('/');
    paths.retain_mut(|path| {
        let directory = fs::stat(path).is_ok_and(|stat| stat.st_mode & fs::S_IFMT == fs::S_IFDIR);
        if directory_only && !directory {
            return false;
        }
        if directory && (flags & 2 != 0 || pattern.ends_with('/')) && !path.ends_with('/') {
            path.push('/');
        }
        true
    });
    Ok(paths)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn glob(
    pattern: *const c_char,
    flags: c_int,
    error: ErrorCallback,
    result: *mut GlobT,
) -> c_int {
    if pattern.is_null() || result.is_null() || flags & !(255 | 2048 | 8192) != 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 2;
    }
    let Ok(pattern) = unsafe { CStr::from_ptr(pattern) }.to_str() else {
        crate::set_errno(84 /* EILSEQ */);
        return 2;
    };
    if pattern.is_empty() {
        return 3;
    }
    let magic = pattern.bytes().any(|b| matches!(b, b'*' | b'?' | b'['));
    let mut paths = match expand(pattern, flags, error) {
        Ok(paths) => paths,
        Err(code) => return code,
    };
    if paths.is_empty() && (flags & 16 != 0 || flags & 2048 != 0 && !magic) {
        paths.push(pattern.to_owned());
    }
    if paths.is_empty() {
        return 3;
    }
    if flags & 4 == 0 {
        paths.sort();
    }
    let result = unsafe { &mut *result };
    let offset = if flags & 8 != 0 { result.gl_offs } else { 0 };
    let previous = if flags & 32 != 0 { result.gl_pathc } else { 0 };
    let Some(slots) = offset
        .checked_add(previous)
        .and_then(|n| n.checked_add(paths.len() + 1))
        .and_then(|n| n.checked_mul(size_of::<usize>()))
    else {
        return 1;
    };
    let vector = unsafe { crate::c_malloc(slots) }.cast::<*mut c_char>();
    if vector.is_null() {
        return 1;
    }
    unsafe {
        ptr::write_bytes(vector.cast::<u8>(), 0, slots);
        if previous != 0 {
            ptr::copy_nonoverlapping(result.gl_pathv.add(offset), vector.add(offset), previous);
        }
        for (index, path) in paths.iter().enumerate() {
            let block = crate::c_malloc(path.len() + 1).cast::<c_char>();
            if block.is_null() {
                for i in 0..index {
                    crate::c_free((*vector.add(offset + previous + i)).cast());
                }
                crate::c_free(vector.cast());
                return 1;
            }
            ptr::copy_nonoverlapping(path.as_ptr(), block.cast(), path.len());
            block.add(path.len()).write(0);
            vector.add(offset + previous + index).write(block);
        }
        if flags & 32 != 0 {
            crate::c_free(result.gl_pathv.cast());
        }
    }
    result.gl_pathv = vector;
    result.gl_pathc = previous + paths.len();
    result.gl_offs = offset;
    result.gl_flags = flags | if magic { 256 } else { 0 };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn globfree(result: *mut GlobT) {
    let Some(result) = (unsafe { result.as_mut() }) else {
        return;
    };
    if !result.gl_pathv.is_null() {
        for index in 0..result.gl_pathc {
            unsafe {
                crate::c_free((*result.gl_pathv.add(result.gl_offs + index)).cast());
            }
        }
        unsafe {
            crate::c_free(result.gl_pathv.cast());
        }
        result.gl_pathv = ptr::null_mut();
        result.gl_pathc = 0;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_glob(
    pattern: *const c_char,
    flags: c_int,
    error: ErrorCallback,
    result: *mut GlobT,
) -> c_int {
    unsafe { glob(pattern, flags, error, result) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_glob64(
    pattern: *const c_char,
    flags: c_int,
    error: ErrorCallback,
    result: *mut GlobT,
) -> c_int {
    unsafe { glob(pattern, flags, error, result) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_globfree(result: *mut GlobT) {
    unsafe {
        globfree(result);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_globfree64(result: *mut GlobT) {
    unsafe {
        globfree(result);
    }
}
