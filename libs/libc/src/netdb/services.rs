//! Guest /etc/services parsing and Linux servent ownership. No host defaults.
use super::{Servent, records};
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use std::{ffi::CString, io::BufRead};
mod addresses;
mod cursor;
mod reentrant;
pub(super) use addresses::{AddressHints, resolve_addresses};
pub use reentrant::{kinakaze_abi_getservbyname_r, kinakaze_abi_getservbyport_r};

const PATH: &str = "/etc/services";
struct Record<'a> {
    names: records::Record<'a>,
    protocol: &'a [u8],
}

fn parse(line: &[u8]) -> Option<Record<'_>> {
    let mut rest = line.split(|&byte| byte == b'#').next()?;
    if rest.contains(&0) {
        return None;
    }
    let name = records::field(&mut rest)?;
    let endpoint = records::field(&mut rest)?;
    let separator = endpoint.iter().position(|&byte| byte == b'/')?;
    let (number, protocol) = (&endpoint[..separator], &endpoint[separator + 1..]);
    let number = number.strip_prefix(b"+").unwrap_or(number);
    if number.is_empty() || !number.iter().all(u8::is_ascii_digit) || protocol.is_empty() {
        return None;
    }
    let port: u16 = std::str::from_utf8(number).ok()?.parse().ok()?;
    Some(Record {
        names: records::Record {
            name,
            value: u32::from(port),
            aliases: rest,
        },
        protocol,
    })
}

pub(super) struct Service {
    names: records::Names,
    protocol: CString,
    pub(super) port: u16,
}
impl Service {
    fn from_record(record: Record<'_>) -> Self {
        Self {
            port: record.names.value as u16,
            names: records::Names::from_record(&record.names),
            protocol: CString::new(record.protocol).unwrap(),
        }
    }
    pub fn name(&self) -> &[u8] {
        self.names.name.as_bytes()
    }
}

fn next(
    reader: &mut impl BufRead,
    matches: impl Fn(&Record<'_>) -> bool,
) -> Result<Option<Service>, i32> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.raw_os_error().unwrap_or(kinakaze_vfs::EIO))?
            == 0
        {
            return Ok(None);
        }
        if let Some(record) = parse(&line)
            && matches(&record)
        {
            return Ok(Some(Service::from_record(record)));
        }
    }
}

fn lookup(matches: impl Fn(&Record<'_>) -> bool) -> Result<Option<Service>, i32> {
    // Lookups own a short-lived stream and never touch enumeration position.
    next(&mut records::Reader::open(PATH)?, matches)
}

pub(super) fn find_service_by_name(
    name: &[u8],
    protocol: Option<&[u8]>,
) -> Result<Option<Service>, i32> {
    lookup(|record| {
        protocol.is_none_or(|p| p == record.protocol) && record.names.matches(name, false)
    })
}
pub(super) fn find_service_by_port(
    port: u16,
    protocol: Option<&[u8]>,
) -> Result<Option<Service>, i32> {
    lookup(|record| {
        protocol.is_none_or(|p| p == record.protocol) && record.names.value == u32::from(port)
    })
}

super::records::returned::returned_record!(*b"CYSERR01");
fn publish(outcome: Result<Option<Service>, i32>) -> *mut Servent {
    let service = match outcome {
        Ok(Some(service)) => service,
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
    let (allocation, name, aliases, protocol) = match records::returned::Record::create_extra(
        &service.names,
        core::mem::size_of::<Servent>(),
        Some(&service.protocol),
    ) {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            return ptr::null_mut();
        }
    };
    let result = allocation.0 as *mut Servent;
    unsafe {
        result.write(Servent {
            s_name: name,
            s_aliases: aliases,
            s_port: c_int::from(service.port.to_be()),
            s_proto: protocol,
        });
    }
    RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
    result
}

/// # Safety
/// Non-null name/protocol pointers must refer to NUL-terminated strings.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getservbyname(
    name: *const c_char,
    protocol: *const c_char,
) -> *mut Servent {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    let protocol = (!protocol.is_null()).then(|| unsafe { CStr::from_ptr(protocol) }.to_bytes());
    publish(find_service_by_name(name, protocol))
}

/// # Safety
/// A non-null protocol must refer to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getservbyport(
    port: c_int,
    protocol: *const c_char,
) -> *mut Servent {
    let Ok(port) = u16::try_from(port) else {
        return ptr::null_mut();
    };
    let protocol = (!protocol.is_null()).then(|| unsafe { CStr::from_ptr(protocol) }.to_bytes());
    publish(find_service_by_port(u16::from_be(port), protocol))
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setservent(_stay_open: c_int) {
    // The files backend retains only the enumeration stream, like glibc.
    if let Err(error) = cursor::rewind() {
        crate::set_errno(error);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endservent() {
    if let Err(error) = cursor::close() {
        crate::set_errno(error);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getservent() -> *mut Servent {
    publish(cursor::next(|reader| next(reader, |_| true)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_keeps_case_byte_names_aliases_and_transport() {
        let row = parse(b"named 23456/sctp alpha \xff # comment").unwrap();
        assert_eq!(row.names.value, 23456);
        assert_eq!(row.protocol, b"sctp");
        assert!(row.names.matches(b"\xff", false));
        assert!(!row.names.matches(b"NAMED", false));
        for line in [
            b"".as_slice(),
            b"# comment",
            b"bad 65536/tcp",
            b"bad -1/udp",
            b"bad 42/",
            b"bad\0name 42/tcp",
        ] {
            assert!(parse(line).is_none());
        }
    }
    #[test]
    fn streaming_lookup_allocates_only_the_matching_record() {
        let mut input = std::io::Cursor::new(b"bad x/tcp\nfirst 1/tcp alias\nsecond 2/udp\n");
        let row = next(&mut input, |r| r.names.matches(b"alias", false))
            .unwrap()
            .unwrap();
        assert_eq!(row.name(), b"first");
        assert_eq!(
            next(&mut input, |_| true).unwrap().unwrap().name(),
            b"second"
        );
        assert!(next(&mut input, |_| true).unwrap().is_none());
    }
}
