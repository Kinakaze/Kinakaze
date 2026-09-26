//! Enumeration reads guest /etc/hosts; DNS lookups keep their own resolver path.
use super::*;
use std::io::BufRead;

const PATH: &str = "/etc/hosts";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYHOST01");
}

struct Entry {
    name: CString,
    aliases: Vec<CString>,
    family: c_int,
    address: Vec<u8>,
}

fn next(reader: &mut impl BufRead) -> Result<Option<Entry>, i32> {
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
        let content = line.split(|b| *b == b'#').next().unwrap_or_default();
        if content.contains(&0) {
            continue;
        }
        let mut fields = content
            .split(u8::is_ascii_whitespace)
            .filter(|s| !s.is_empty());
        let (Some(address), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        let Some(address) = std::str::from_utf8(address)
            .ok()
            .and_then(|s| s.parse::<std::net::IpAddr>().ok())
        else {
            continue;
        };
        let (family, address) = match address {
            std::net::IpAddr::V4(ip) => (AF_INET, ip.octets().to_vec()),
            std::net::IpAddr::V6(ip) => (AF_INET6, ip.octets().to_vec()),
        };
        return Ok(Some(Entry {
            name: CString::new(name).unwrap(),
            aliases: fields.map(|s| CString::new(s).unwrap()).collect(),
            family,
            address,
        }));
    }
}

fn publish(entry: Entry) -> *mut Hostent {
    set_h_errno(0);
    publish_hostent_names(entry.name, entry.aliases, entry.family, vec![entry.address])
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sethostent(_stayopen: c_int) {
    if let Err(error) = cursor::rewind() {
        set_errno(error);
        set_h_errno(NO_RECOVERY);
    }
}

pub(super) fn close() {
    if let Err(error) = cursor::close() {
        set_errno(error);
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_gethostent() -> *mut Hostent {
    if !super::nss::host_sources().0 {
        set_h_errno(HOST_NOT_FOUND);
        return ptr::null_mut();
    }
    match cursor::next(next) {
        Ok(Some(entry)) => publish(entry),
        outcome => {
            if let Err(error) = outcome {
                set_errno(error);
                set_h_errno(NO_RECOVERY);
            } else {
                set_h_errno(HOST_NOT_FOUND);
            }
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostent_r(
    output: *mut Hostent,
    buffer: *mut c_char,
    capacity: usize,
    result: *mut *mut Hostent,
    h_error: *mut c_int,
) -> c_int {
    if result.is_null() {
        return EINVAL;
    }
    unsafe {
        *result = ptr::null_mut();
    }
    if output.is_null() || buffer.is_null() {
        return EINVAL;
    }
    if !super::nss::host_sources().0 {
        if !h_error.is_null() {
            unsafe {
                *h_error = HOST_NOT_FOUND;
            }
        }
        return kinakaze_vfs::ENOENT;
    }
    let outcome = cursor::next(|reader| {
        reader.retry_range(|reader| {
            let Some(entry) = next(reader)? else {
                if !h_error.is_null() {
                    unsafe {
                        *h_error = HOST_NOT_FOUND;
                    }
                }
                return Err(kinakaze_vfs::ENOENT);
            };
            let code = unsafe {
                reentrant::copy_host(publish(entry), output, buffer, capacity, result, h_error)
            };
            if code == 0 { Ok(()) } else { Err(code) }
        })
    });
    outcome.map_or_else(
        |error| {
            if !h_error.is_null() && error != kinakaze_vfs::ENOENT {
                unsafe {
                    *h_error = if error == kinakaze_vfs::ERANGE {
                        -1
                    } else {
                        NO_RECOVERY
                    };
                }
            }
            error
        },
        |()| 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hosts_parser_keeps_aliases_and_both_address_families() {
        let mut input = std::io::Cursor::new(
            b"# comment\nbad host\n127.0.0.1 local alias # comment\n::1 ipv6\n",
        );
        let entry = next(&mut input).unwrap().unwrap();
        assert_eq!(entry.name.as_bytes(), b"local");
        assert_eq!(entry.aliases[0].as_bytes(), b"alias");
        assert_eq!(entry.address, [127, 0, 0, 1]);
        assert_eq!(next(&mut input).unwrap().unwrap().family, AF_INET6);
        assert!(next(&mut input).unwrap().is_none());
    }
}
