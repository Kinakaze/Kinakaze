//! Protocol names and numbers from the guest's /etc/protocols, without defaults.
use super::{Protoent, records};
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};

super::records::returned::returned_record!(*b"CYPROT01");

fn number(text: &[u8]) -> Option<u32> {
    let text = text.strip_prefix(b"+").unwrap_or(text);
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value: u32 = std::str::from_utf8(text).ok()?.parse().ok()?;
    (value <= c_int::MAX as u32).then_some(value)
}

fn lookup(matches: impl Fn(&records::Record<'_>) -> bool) -> *mut Protoent {
    let (value, names) = match records::lookup("/etc/protocols", number, matches) {
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
        match records::returned::Record::create(&names, core::mem::size_of::<Protoent>()) {
            Ok(value) => value,
            Err(error) => {
                crate::set_errno(error);
                return ptr::null_mut();
            }
        };
    let result = allocation.0 as *mut Protoent;
    unsafe {
        result.write(Protoent {
            p_name: name,
            p_aliases: aliases,
            p_proto: value as c_int,
        });
    }
    RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
    result
}

/// # Safety
/// name must be null or a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getprotobyname(name: *const c_char) -> *mut Protoent {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    lookup(|entry| entry.matches(name, false))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getprotobynumber(protocol: c_int) -> *mut Protoent {
    lookup(|entry| protocol >= 0 && entry.value == protocol as u32)
}

/// Caller-owned output: all strings and the aligned alias table fit in buffer.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getprotobyname_r(
    name: *const c_char,
    output: *mut Protoent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Protoent,
) -> c_int {
    use kinakaze_vfs::{EINVAL, ERANGE};
    if result.is_null() {
        return EINVAL;
    }
    unsafe { result.write(ptr::null_mut()) };
    if name.is_null() || output.is_null() || (buffer.is_null() && capacity != 0) {
        return EINVAL;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    unsafe {
        lookup_reentrant(
            |record| record.matches(name, false),
            output,
            buffer,
            capacity,
            result,
        )
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getprotobynumber_r(
    protocol: c_int,
    output: *mut Protoent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Protoent,
) -> c_int {
    unsafe {
        lookup_reentrant(
            |record| protocol >= 0 && record.value == protocol as u32,
            output,
            buffer,
            capacity,
            result,
        )
    }
}

unsafe fn lookup_reentrant(
    matches: impl Fn(&records::Record<'_>) -> bool,
    output: *mut Protoent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Protoent,
) -> c_int {
    use kinakaze_vfs::{EINVAL, ERANGE};
    if result.is_null() {
        return EINVAL;
    }
    unsafe { result.write(ptr::null_mut()) };
    if output.is_null() || (buffer.is_null() && capacity != 0) {
        return EINVAL;
    }
    let (value, names) = match records::lookup("/etc/protocols", number, matches) {
        Ok(Some(value)) => value,
        Ok(None) => return 0,
        Err(error) => return error,
    };
    let padding = buffer.addr().wrapping_neg() & (core::mem::align_of::<*mut c_char>() - 1);
    let count = names.aliases().len() + 1;
    let Some(table) = count
        .checked_mul(core::mem::size_of::<*mut c_char>())
        .and_then(|n| n.checked_add(padding))
    else {
        return ERANGE;
    };
    let strings = std::iter::once(&names.name).chain(names.aliases());
    let Some(needed) = strings
        .clone()
        .try_fold(table, |n, s| n.checked_add(s.as_bytes_with_nul().len()))
    else {
        return ERANGE;
    };
    if needed > capacity {
        return ERANGE;
    }
    let aliases = unsafe { buffer.byte_add(padding).cast::<*mut c_char>() };
    let mut offset = table;
    for (index, string) in strings.enumerate() {
        let target = unsafe { buffer.byte_add(offset) };
        unsafe {
            ptr::copy_nonoverlapping(string.as_ptr(), target, string.as_bytes_with_nul().len())
        };
        if index != 0 {
            unsafe { aliases.add(index - 1).write(target) };
        }
        offset += string.as_bytes_with_nul().len();
    }
    unsafe {
        aliases.add(count - 1).write(ptr::null_mut());
        output.write(Protoent {
            p_name: buffer.byte_add(table),
            p_aliases: aliases,
            p_proto: value as c_int,
        });
        result.write(output);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_protocol_numbers_are_not_guessed() {
        assert_eq!(number(b"132"), Some(132));
        assert_eq!(number(b"+6"), Some(6));
        for text in [b"".as_slice(), b"-1", b"0x11", b"2147483648", b"6x"] {
            assert_eq!(number(text), None);
        }
        assert_eq!(core::mem::size_of::<Protoent>(), 24);
        assert_eq!(core::mem::offset_of!(Protoent, p_proto), 16);
    }
}
