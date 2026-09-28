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
#[derive(Clone, Copy)]
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

struct Access(Option<GlobT>);

impl Access {
    fn stat(&self, path: &str, follow: bool) -> Result<fs::Stat, c_int> {
        let Some(callbacks) = self.0 else {
            return if follow {
                fs::stat(path)
            } else {
                fs::lstat(path)
            };
        };
        let callback = if follow {
            callbacks.gl_stat
        } else {
            callbacks.gl_lstat
        }
        .unwrap();
        let name = CString::new(path).unwrap();
        let mut stat = fs::Stat::default();
        if unsafe { callback(name.as_ptr(), (&mut stat as *mut fs::Stat).cast()) } == 0 {
            Ok(stat)
        } else {
            Err(crate::kinakaze_errno())
        }
    }

    fn entries(&self, path: &str) -> Result<Vec<String>, c_int> {
        let Some(callbacks) = self.0 else {
            return fs::read_directory(path)
                .map(|entries| entries.into_iter().map(|e| e.name).collect());
        };
        let name = CString::new(path).unwrap();
        let stream = unsafe { callbacks.gl_opendir.unwrap()(name.as_ptr()) };
        if stream.is_null() {
            return Err(crate::kinakaze_errno());
        }
        let mut names = Vec::new();
        let result = loop {
            crate::set_errno(0);
            let entry =
                unsafe { callbacks.gl_readdir.unwrap()(stream) }.cast::<crate::dirent::Dirent>();
            if entry.is_null() {
                let code = crate::kinakaze_errno();
                break if code == 0 { Ok(names) } else { Err(code) };
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
            match name.to_str() {
                Ok(name) => names.push(name.to_owned()),
                Err(_) => break Err(84 /* EILSEQ */),
            }
        };
        unsafe { callbacks.gl_closedir.unwrap()(stream) };
        result
    }
}

fn has_magic(pattern: &str, flags: c_int) -> bool {
    let pattern = CString::new(pattern).unwrap();
    unsafe { kinakaze_abi_glob_pattern_p(pattern.as_ptr(), i32::from(flags & 64 == 0)) != 0 }
}

fn literal(pattern: &str, flags: c_int) -> String {
    let mut result = String::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        result.push(if ch == '\\' && flags & 64 == 0 {
            chars.next().unwrap_or(ch)
        } else {
            ch
        });
    }
    result
}

fn joined(base: &str, name: &str) -> String {
    if base.is_empty() || base.ends_with('/') {
        format!("{base}{name}")
    } else {
        format!("{base}/{name}")
    }
}

fn path_error(path: &str, code: c_int, flags: c_int, error: ErrorCallback) -> Result<(), c_int> {
    if code == kinakaze_vfs::ENOENT || code == kinakaze_vfs::ENOTDIR {
        return Ok(());
    }
    let name = CString::new(path).unwrap();
    let abort = error.is_some_and(|callback| unsafe { callback(name.as_ptr(), code) } != 0);
    if abort || flags & 1 != 0 {
        crate::set_errno(code);
        return Err(2);
    }
    Ok(())
}

fn expand(
    pattern: &str,
    flags: c_int,
    error: ErrorCallback,
    access: &Access,
) -> Result<Vec<String>, c_int> {
    let mut paths = vec![if pattern.starts_with('/') {
        "/".to_owned()
    } else {
        String::new()
    }];
    let mut components = pattern
        .split('/')
        .filter(|part| !part.is_empty())
        .peekable();
    while let Some(component) = components.next() {
        let mut next = Vec::new();
        // Literal prefixes need lookup/search permission, not a directory scan.
        if !has_magic(component, flags) {
            let name = literal(component, flags);
            for base in paths {
                let path = joined(&base, &name);
                if components.peek().is_some() {
                    next.push(path);
                } else {
                    match access.stat(&path, false) {
                        Ok(_) => next.push(path),
                        Err(code) => path_error(&path, code, flags, error)?,
                    }
                }
            }
            paths = next;
            continue;
        }
        let pattern = CString::new(component).unwrap();
        for base in paths {
            let directory = if base.is_empty() { "." } else { &base };
            let entries = match access.entries(directory) {
                Ok(entries) => entries,
                Err(code) => {
                    path_error(directory, code, flags, error)?;
                    continue;
                }
            };
            for entry in entries {
                let name = CString::new(entry.as_bytes()).unwrap();
                let match_flags = if flags & 64 != 0 {
                    crate::fsextra::FNM_NOESCAPE
                } else {
                    0
                } | if flags & 128 == 0 {
                    crate::fsextra::FNM_PERIOD
                } else {
                    0
                };
                if unsafe {
                    crate::fsextra::kinakaze_abi_fnmatch(
                        pattern.as_ptr(),
                        name.as_ptr(),
                        match_flags,
                    )
                } == 0
                {
                    next.push(joined(&base, &entry));
                }
            }
        }
        paths = next;
    }
    let directory_only = flags & 8192 != 0 || pattern.ends_with('/');
    paths.retain_mut(|path| {
        if !directory_only && flags & 2 == 0 {
            return true;
        }
        let directory = access
            .stat(path, true)
            .is_ok_and(|stat| stat.st_mode & fs::S_IFMT == fs::S_IFDIR);
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

// Return the first balanced brace group containing alternatives. Keeping a
// worklist avoids recursion for nested GNU brace patterns.
fn alternatives(pattern: &str, flags: c_int) -> Option<Vec<String>> {
    let bytes = pattern.as_bytes();
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut first = None;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' if flags & 64 == 0 => at += usize::from(at + 1 < bytes.len()),
            b'{' => groups.push((at, Vec::new())),
            b',' => {
                if let Some((_, commas)) = groups.last_mut() {
                    commas.push(at);
                }
            }
            b'}' => {
                if let Some((start, commas)) = groups.pop() {
                    if !commas.is_empty()
                        && first.as_ref().is_none_or(|(prior, _, _)| start < *prior)
                    {
                        first = Some((start, at, commas));
                    }
                }
            }
            _ => (),
        }
        at += 1;
    }
    first.map(|(start, close, commas)| {
        let mut from = start + 1;
        commas
            .into_iter()
            .chain([close])
            .map(|end| {
                let value = format!(
                    "{}{}{}",
                    &pattern[..start],
                    &pattern[from..end],
                    &pattern[close + 1..]
                );
                from = end + 1;
                value
            })
            .collect()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn glob(
    pattern: *const c_char,
    flags: c_int,
    error: ErrorCallback,
    result: *mut GlobT,
) -> c_int {
    if pattern.is_null() || result.is_null() || flags & !(255 | 512 | 1024 | 2048 | 8192) != 0 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 2;
    }
    let Ok(pattern) = unsafe { CStr::from_ptr(pattern) }.to_str() else {
        crate::set_errno(84 /* EILSEQ */);
        return 2;
    };
    let result = unsafe { &mut *result };
    if flags & 32 == 0 {
        result.gl_pathc = 0;
        result.gl_pathv = ptr::null_mut();
    }
    let access = Access(if flags & 512 != 0 {
        if result.gl_opendir.is_none()
            || result.gl_readdir.is_none()
            || result.gl_closedir.is_none()
            || result.gl_stat.is_none()
            || result.gl_lstat.is_none()
        {
            crate::set_errno(kinakaze_vfs::EINVAL);
            return 2;
        }
        Some(*result)
    } else {
        None
    });
    if pattern.is_empty() {
        return 3;
    }
    let magic = has_magic(pattern, flags);
    let mut pending = vec![pattern.to_owned()];
    let mut paths = Vec::new();
    while let Some(pattern) = pending.pop() {
        if flags & 1024 != 0 {
            if let Some(alternatives) = alternatives(&pattern, flags) {
                pending.extend(alternatives.into_iter().rev());
                continue;
            }
        }
        match expand(&pattern, flags, error, &access) {
            Ok(found) => paths.extend(found),
            Err(code) => return code,
        }
    }
    if paths.is_empty() && (flags & 16 != 0 || flags & 2048 != 0 && !magic) {
        paths.push(pattern.to_owned());
    }
    if paths.is_empty() {
        return 3;
    }
    if flags & 4 == 0 {
        paths.sort();
    }
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
