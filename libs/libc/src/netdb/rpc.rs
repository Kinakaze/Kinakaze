//! RPC program names and numbers from the guest /etc/rpc database.
use super::records;

#[repr(C)]
pub struct Rpcent {
    r_name: *mut c_char,
    r_aliases: *mut *mut c_char,
    r_number: c_int,
}
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};

super::records::returned::returned_record!(*b"CYRPCR01");
const PATH: &str = "/etc/rpc";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYRPCE01");
}

fn number(text: &[u8]) -> Option<u32> {
    let text = text.strip_prefix(b"+").unwrap_or(text);
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value: u32 = std::str::from_utf8(text).ok()?.parse().ok()?;
    (value <= c_int::MAX as u32).then_some(value)
}

fn lookup(matches: impl Fn(&records::Record<'_>) -> bool) -> *mut Rpcent {
    publish(records::lookup(PATH, number, matches))
}

fn publish(outcome: Result<Option<(u32, records::Names)>, i32>) -> *mut Rpcent {
    let (value, names) = match outcome {
        Ok(Some(entry)) => entry,
        Ok(None) => return ptr::null_mut(),
        Err(error) => {
            crate::set_errno(error);
            return ptr::null_mut();
        }
    };
    if let Err(error) = register_returned() {
        crate::set_errno(error);
        return ptr::null_mut();
    }
    let (allocation, name, aliases) =
        match records::returned::Record::create(&names, core::mem::size_of::<Rpcent>()) {
            Ok(value) => value,
            Err(error) => {
                crate::set_errno(error);
                return ptr::null_mut();
            }
        };
    let result = allocation.0 as *mut Rpcent;
    unsafe {
        result.write(Rpcent {
            r_name: name,
            r_aliases: aliases,
            r_number: value as c_int,
        });
    }
    RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
    result
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setrpcent(_stayopen: c_int) {
    if let Err(error) = cursor::rewind() {
        crate::set_errno(error);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endrpcent() {
    if let Err(error) = cursor::close() {
        crate::set_errno(error);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getrpcent() -> *mut Rpcent {
    publish(cursor::next(|reader| {
        records::find(reader, number, |_| true)
    }))
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getrpcent_r(
    output: *mut Rpcent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Rpcent,
) -> c_int {
    if result.is_null() || output.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    unsafe { *result = ptr::null_mut() };
    cursor::next(|reader| {
        reader.retry_range(|reader| {
            let Some((value, names)) = records::find(reader, number, |_| true)? else {
                return Err(kinakaze_vfs::ENOENT);
            };
            let (name, aliases) = unsafe { records::copy_names(&names, buffer, capacity)? };
            unsafe {
                output.write(Rpcent {
                    r_name: name,
                    r_aliases: aliases,
                    r_number: value as c_int,
                });
                *result = output;
            }
            Ok(())
        })
    })
    .map_or_else(|error| error, |()| 0)
}

/// # Safety
/// name must be null or a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getrpcbyname(name: *const c_char) -> *mut Rpcent {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    lookup(|entry| entry.matches(name, false))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getrpcbynumber(protocol: c_int) -> *mut Rpcent {
    lookup(|entry| protocol >= 0 && entry.value == protocol as u32)
}
