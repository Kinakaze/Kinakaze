//! Message catalogs live entirely in guest memory, including returned strings.
//! Closing or forking needs no registry, retained host heap, or descriptor repair.
mod format;
use core::ffi::{CStr, c_char, c_int, c_void};
use core::{ptr, slice};
use format::Layout;
use kinakaze_alloc::guest;
use kinakaze_vfs::{EBADF, EINTR, EINVAL, EIO, ENOENT, ENOMEM, fs};

const FAILED: *mut c_void = usize::MAX as *mut c_void;
const DEFAULT_PATH: &str = "/usr/share/locale/%L/%N:/usr/share/locale/%L/LC_MESSAGES/%N:/usr/share/locale/%l/%N:/usr/share/locale/%l/LC_MESSAGES/%N:";

struct File(i32);
impl Drop for File {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::close(self.0);
    }
}
struct Allocation(*mut u8);
impl Drop for Allocation {
    fn drop(&mut self) {
        unsafe { guest::free(self.0) };
    }
}
// GNU's private __open_catalog ABI is used by the actual gencat utility.
// Its caller owns this record; file_ptr is separately freeable guest memory.
#[repr(C)]
pub struct Catalog {
    status: c_int,
    width: usize,
    depth: usize,
    names: *mut u32,
    strings: *mut c_char,
    file: *mut u8,
    length: usize,
}
const _: () = assert!(size_of::<Catalog>() == 56);

fn load(file: File) -> Result<Catalog, i32> {
    let stat = fs::fstat(file.0)?;
    if stat.st_mode & fs::S_IFMT != fs::S_IFREG || stat.st_size < 12 {
        return Err(EINVAL);
    }
    let length = usize::try_from(stat.st_size).map_err(|_| EINVAL)?;
    if length > isize::MAX as usize {
        return Err(ENOMEM);
    }
    let allocation = Allocation(unsafe { guest::malloc(length) });
    if allocation.0.is_null() {
        return Err(ENOMEM);
    }
    let bytes = unsafe { slice::from_raw_parts_mut(allocation.0, length) };
    let mut offset = 0;
    while offset < length {
        match kinakaze_vfs::read(file.0, &mut bytes[offset..]) {
            Ok(0) => return Err(EIO),
            Ok(n) => offset += n,
            Err(EINTR) => continue,
            Err(e) => return Err(e),
        }
    }
    let layout = Layout::parse(bytes)?;
    let result = Catalog {
        status: 1, // malloced, as defined by GNU catalog_info
        width: layout.width,
        depth: layout.depth,
        names: unsafe { allocation.0.add(12).cast() },
        strings: unsafe { allocation.0.add(layout.strings).cast() },
        file: allocation.0,
        length,
    };
    core::mem::forget(allocation);
    Ok(result)
}

fn search(name: &str, paths: Option<&str>, locale: &str) -> Result<Catalog, i32> {
    let flags = fs::O_RDONLY | fs::O_CLOEXEC;
    if name.contains('/') || paths.is_none() {
        return load(File(fs::open(name, flags, 0)?));
    }
    let mut error = ENOENT;
    for pattern in paths.unwrap().split(':') {
        let Some(path) = expand(pattern, name, locale) else {
            continue;
        };
        match fs::open(&path, flags, 0) {
            Ok(fd) => return load(File(fd)),
            Err(e) => error = e,
        }
    }
    Err(error)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___open_catalog(
    name: *const c_char,
    paths: *const c_char,
    locale: *const c_char,
    output: *mut Catalog,
) -> c_int {
    let result = (|| {
        if name.is_null() || output.is_null() {
            return Err(EINVAL);
        }
        let text = |p| unsafe { CStr::from_ptr(p) }.to_str().map_err(|_| 84);
        let name = text(name)?;
        let paths = if paths.is_null() {
            None
        } else {
            Some(text(paths)?)
        };
        let locale = if locale.is_null() { "C" } else { text(locale)? };
        let catalog = search(name, paths, locale)?;
        unsafe { output.write(catalog) };
        Ok(0)
    })();
    result.unwrap_or_else(|error| {
        crate::set_errno(error);
        -1
    })
}

unsafe fn environment(name: &CStr) -> String {
    let value = unsafe { crate::process::kinakaze_abi_getenv(name.as_ptr()) };
    if value.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    }
}

fn expand(pattern: &str, name: &str, locale: &str) -> Option<String> {
    let (language, rest) = locale.split_once('_').unwrap_or((locale, ""));
    let language = language.split('.').next().unwrap_or("");
    let territory = rest.split('.').next().unwrap_or("");
    let codeset = locale.split_once('.').map_or("", |(_, s)| s);
    if pattern.is_empty() {
        return Some(name.to_owned());
    }
    let mut result = String::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            result.push(ch);
            continue;
        }
        result.push_str(match chars.next()? {
            'N' => name,
            'L' => locale,
            'l' => language,
            't' => territory,
            'c' => codeset,
            '%' => "%",
            _ => return None,
        });
    }
    Some(result)
}

unsafe fn open(name: &str, flags: i32) -> Result<Catalog, i32> {
    if name.contains('/') {
        return load(File(fs::open(name, fs::O_RDONLY | fs::O_CLOEXEC, 0)?));
    }
    let mut locale = if flags == 1 {
        let value = unsafe { crate::locale::kinakaze_abi_setlocale(5, ptr::null()) };
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    } else {
        unsafe { environment(c"LANG") }
    };
    if locale.is_empty() {
        locale.push('C');
    }
    let mut paths = unsafe { environment(c"NLSPATH") };
    if !paths.is_empty() {
        paths.push(':');
    }
    paths.push_str(DEFAULT_PATH);
    search(name, Some(&paths), &locale)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_catopen(
    name: *const c_char,
    flags: c_int,
) -> *mut c_void {
    let result = if name.is_null() {
        Err(EINVAL)
    } else {
        match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok("") => Err(ENOENT),
            Ok(name) => unsafe { open(name, flags) },
            Err(_) => Err(84),
        }
    };
    let result = result.and_then(|catalog| {
        let descriptor = unsafe { guest::malloc(size_of::<Catalog>()).cast::<Catalog>() };
        if descriptor.is_null() {
            unsafe { guest::free(catalog.file) };
            return Err(ENOMEM);
        }
        unsafe { descriptor.write(catalog) };
        Ok(descriptor.cast())
    });
    result.unwrap_or_else(|error| {
        crate::set_errno(error);
        FAILED
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_catgets(
    descriptor: *mut c_void,
    set: c_int,
    message: c_int,
    default: *const c_char,
) -> *mut c_char {
    if descriptor == FAILED {
        return default.cast_mut();
    }
    let Some(set) = set.checked_add(1).filter(|&s| s > 0) else {
        return default.cast_mut();
    };
    if message < 0 {
        return default.cast_mut();
    }
    if descriptor.is_null() {
        crate::set_errno(EBADF);
        return default.cast_mut();
    }
    let catalog = unsafe { &*descriptor.cast::<Catalog>() };
    if catalog.status != 1 {
        crate::set_errno(EBADF);
        return default.cast_mut();
    }
    let pointer = catalog.file;
    let bytes = unsafe { slice::from_raw_parts(pointer, catalog.length) };
    let layout = Layout {
        width: catalog.width,
        depth: catalog.depth,
        strings: catalog.strings as usize - pointer as usize,
    };
    match layout.find(bytes, set as u32, message as u32) {
        Some(offset) => unsafe { pointer.add(offset).cast() },
        None => {
            crate::set_errno(42);
            default.cast_mut()
        } // ENOMSG
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_catclose(descriptor: *mut c_void) -> c_int {
    if descriptor.is_null() || descriptor == FAILED {
        crate::set_errno(EBADF);
        return -1;
    }
    let catalog = unsafe { &mut *descriptor.cast::<Catalog>() };
    if catalog.status != 1 {
        crate::set_errno(EBADF);
        return -1;
    }
    catalog.status = -1;
    unsafe { guest::free(catalog.file) };
    unsafe { guest::free(descriptor.cast()) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn locale_search_expands_components_and_rejects_unknown_escapes() {
        assert_eq!(
            expand("/%L/%l/%t/%c/%%/%N", "app", "en_US.UTF-8@custom").as_deref(),
            Some("/en_US.UTF-8@custom/en/US/UTF-8@custom/%/app")
        );
        assert_eq!(expand("", "app", "C").as_deref(), Some("app"));
        assert_eq!(
            expand("%l.%c", "app", "C.UTF-8").as_deref(),
            Some("C.UTF-8")
        );
        assert_eq!(expand("%q", "app", "C"), None);
    }
}
