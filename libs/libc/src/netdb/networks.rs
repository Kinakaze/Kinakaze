//! Network database lookup and the historical inet_network address format.
use super::{AF_INET, AF_UNSPEC, HOST_NOT_FOUND, NO_RECOVERY, records, set_h_errno};
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
const PATH: &str = "/etc/networks";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYNETS01");
}

#[repr(C)]
pub struct Netent {
    pub n_name: *mut c_char,
    pub n_aliases: *mut *mut c_char,
    pub n_addrtype: c_int,
    pub n_net: u32,
}
super::records::returned::returned_record!(*b"CYNETR01");

/// Unlike inet_aton, every component must fit in one octet. Missing trailing
/// octets are added by the file database, not by inet_network itself.
fn address(text: &[u8]) -> Option<(u32, u32)> {
    let text = text.trim_ascii_end();
    let mut value = 0u32;
    let mut count = 0u32;
    for component in text.split(|&b| b == b'.') {
        if count == 4 {
            return None;
        }
        let text = std::str::from_utf8(component).ok()?;
        let byte = if let Some(hex) = text.strip_prefix('x').or_else(|| text.strip_prefix('X')) {
            if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(hex, 16).ok()?
        } else {
            super::parse_component(text)?
        };
        if byte > 255 {
            return None;
        }
        value = (value << 8) | byte;
        count += 1;
    }
    Some((value, count))
}

fn number(text: &[u8]) -> Option<u32> {
    let (value, count) = address(text)?;
    if value == u32::MAX {
        return None;
    }
    Some(value << (8 * (4 - count)))
}

fn lookup(matches: impl Fn(&records::Record<'_>) -> bool) -> *mut Netent {
    publish(records::lookup(PATH, number, matches))
}

fn publish(outcome: Result<Option<(u32, records::Names)>, i32>) -> *mut Netent {
    let (value, names) = match outcome {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            set_h_errno(HOST_NOT_FOUND);
            return ptr::null_mut();
        }
        Err(error) => {
            crate::set_errno(error);
            set_h_errno(NO_RECOVERY);
            return ptr::null_mut();
        }
    };
    if let Err(error) = register_returned() {
        crate::set_errno(error);
        return ptr::null_mut();
    }
    let (allocation, name, aliases) =
        match records::returned::Record::create(&names, core::mem::size_of::<Netent>()) {
            Ok(value) => value,
            Err(error) => {
                crate::set_errno(error);
                return ptr::null_mut();
            }
        };
    let result = allocation.0 as *mut Netent;
    unsafe {
        result.write(Netent {
            n_name: name,
            n_aliases: aliases,
            n_addrtype: AF_INET,
            n_net: value,
        });
    }
    RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
    result
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setnetent(_stayopen: c_int) {
    // Enumeration retains its stream until reset/close, including EOF.
    if let Err(error) = cursor::rewind() {
        crate::set_errno(error);
        set_h_errno(NO_RECOVERY);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getnetent() -> *mut Netent {
    publish(cursor::next(|reader| {
        records::find(reader, number, |_| true)
    }))
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endnetent() {
    if let Err(error) = cursor::close() {
        crate::set_errno(error);
    }
}

/// # Safety
/// name must be null or a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getnetbyname(name: *const c_char) -> *mut Netent {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        set_h_errno(NO_RECOVERY);
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    lookup(|entry| entry.matches(name, true))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getnetbyaddr(network: u32, family: c_int) -> *mut Netent {
    if family != AF_INET && family != AF_UNSPEC {
        set_h_errno(HOST_NOT_FOUND);
        return ptr::null_mut();
    }
    lookup(|entry| entry.value == network)
}

/// # Safety
/// text must be null or a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_inet_network(text: *const c_char) -> u32 {
    if text.is_null() {
        return u32::MAX;
    }
    address(unsafe { CStr::from_ptr(text) }.to_bytes()).map_or(u32::MAX, |(value, _)| value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_octets_and_file_padding_are_distinct() {
        assert_eq!(address(b"192.168"), Some((0xc0a8, 2)));
        assert_eq!(number(b"192.168"), Some(0xc0a80000));
        assert_eq!(number(b"10"), Some(0x0a000000));
        assert_eq!(address(b"0300.0xA8.01.0\t"), Some((0xc0a80100, 4)));
        assert_eq!(address(b"xff.0"), Some((0xff00, 2)));
        for text in [
            b"".as_slice(),
            b"1.256",
            b"1..2",
            b"1.2.3.4.5",
            b"09",
            b"0x",
            b"+1",
            b" 1",
            b"1 x",
        ] {
            assert!(address(text).is_none(), "{text:?}");
        }
        assert_eq!(number(b"255.255.255.255"), None);
        assert_eq!(core::mem::size_of::<Netent>(), 24);
        assert_eq!(core::mem::offset_of!(Netent, n_net), 20);
    }
}
