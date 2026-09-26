//! Shadow lookups preserve the on-disk values and VFS access checks.
use super::records;
use crate::userdb::{Spwd, account_files::parse_shadow};
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use std::io::BufRead;
const PATH: &str = "/etc/shadow";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYSPWC01");
}
super::records::returned::returned_record!(*b"CYSPWR01");

fn next(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, i32> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.raw_os_error().unwrap_or(5))?
            == 0
        {
            return Ok(None);
        }
        if line.contains(&0) || line.starts_with(b"#") || line.trim_ascii().is_empty() {
            continue;
        }
        line.push(0);
        let mut check = line.clone();
        if unsafe { parse_shadow(check.as_mut_ptr().cast()) }.is_some() {
            return Ok(Some(line));
        }
    }
}
fn lookup(name: &[u8]) -> Result<Option<Vec<u8>>, i32> {
    let mut reader = records::Reader::open(PATH)?;
    while let Some(line) = next(&mut reader)? {
        if line.split(|&c| c == b':' || c == 0).next() == Some(name) {
            return Ok(Some(line));
        }
    }
    Ok(None)
}
unsafe fn pack(
    line: &[u8],
    entry: *mut Spwd,
    buffer: *mut c_char,
    length: usize,
) -> Result<(), i32> {
    if entry.is_null() || buffer.is_null() {
        return Err(22);
    }
    if line.len() > length {
        return Err(34);
    }
    unsafe {
        ptr::copy_nonoverlapping(line.as_ptr().cast(), buffer, line.len());
        entry.write(parse_shadow(buffer).ok_or(22)?);
    }
    Ok(())
}
fn publish(result: Result<Option<Vec<u8>>, i32>) -> *mut Spwd {
    let outcome = (|| {
        let Some(line) = result? else {
            return Ok(ptr::null_mut());
        };
        register_returned()?;
        let allocation = unsafe { kinakaze_alloc::guest::malloc(size_of::<Spwd>() + line.len()) };
        if allocation.is_null() {
            return Err(12);
        }
        let record = records::returned::Record(allocation as usize);
        unsafe {
            pack(
                &line,
                allocation.cast(),
                allocation.add(size_of::<Spwd>()).cast(),
                line.len(),
            )?;
        }
        RETURNED.with(|slot| *slot.borrow_mut() = Some(record));
        Ok(allocation.cast())
    })();
    outcome.unwrap_or_else(|e| {
        crate::set_errno(e);
        ptr::null_mut()
    })
}
pub(crate) unsafe fn getspnam(name: *const c_char) -> *mut Spwd {
    if name.is_null() {
        crate::set_errno(22);
        return ptr::null_mut();
    }
    publish(lookup(unsafe { CStr::from_ptr(name) }.to_bytes()))
}
pub(crate) fn getspent() -> *mut Spwd {
    publish(cursor::next(next))
}
pub(crate) fn setspent() {
    if let Err(e) = cursor::rewind() {
        crate::set_errno(e);
    }
}
pub(crate) fn endspent() {
    if let Err(e) = cursor::close() {
        crate::set_errno(e);
    }
}
pub(crate) unsafe fn getspnam_r(
    name: *const c_char,
    entry: *mut Spwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Spwd,
) -> c_int {
    if result.is_null() {
        return 22;
    }
    unsafe {
        result.write(ptr::null_mut());
    }
    if name.is_null() {
        return 22;
    }
    let outcome = (|| {
        if let Some(line) = lookup(unsafe { CStr::from_ptr(name) }.to_bytes())? {
            unsafe {
                pack(&line, entry, buffer, length)?;
                result.write(entry);
            }
        }
        Ok(())
    })();
    outcome.map_or_else(
        |e| {
            crate::set_errno(e);
            e
        },
        |()| 0,
    )
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getspent_r(
    entry: *mut Spwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Spwd,
) -> c_int {
    if result.is_null() {
        return 22;
    }
    unsafe {
        result.write(ptr::null_mut());
    }
    let outcome = cursor::next(|reader| {
        reader.retry_range(|reader| {
            let Some(line) = next(reader)? else {
                return Err(2);
            };
            unsafe {
                pack(&line, entry, buffer, length)?;
                result.write(entry);
            }
            Ok(())
        })
    });
    outcome.map_or_else(
        |e| {
            crate::set_errno(e);
            e
        },
        |()| 0,
    )
}
