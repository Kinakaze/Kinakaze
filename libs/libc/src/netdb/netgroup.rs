//! File-backed netgroups, resolved only when requested. Cycles terminate locally.
use core::ffi::{CStr, c_char, c_int};
use std::{collections::HashSet, io::BufRead};

mod enumeration;

fn definition(name: &[u8]) -> Result<Option<Vec<u8>>, i32> {
    let mut file = super::records::Reader::open("/etc/netgroup")?;
    let mut record = Vec::new();
    loop {
        let read = file
            .read_until(b'\n', &mut record)
            .map_err(|_| kinakaze_vfs::EIO)?;
        if read == 0 {
            return Ok(None);
        }
        if record.len() > 65536 {
            return Err(kinakaze_vfs::EOVERFLOW);
        }
        let line = record.trim_ascii_end();
        if line.last() == Some(&b'\\') {
            record.truncate(line.len() - 1);
            continue;
        }
        let mut rest = line.split(|&b| b == b'#').next().unwrap_or_default();
        if super::records::field(&mut rest) == Some(name) {
            return Ok(Some(rest.to_vec()));
        }
        record.clear();
    }
}

fn matches(group: &[u8], wanted: &[Option<&[u8]>; 3], seen: &mut HashSet<Vec<u8>>) -> bool {
    if seen.len() >= 1024 || !seen.insert(group.to_vec()) {
        return false;
    }
    let data = match definition(group) {
        Ok(Some(data)) => data,
        Ok(None) | Err(kinakaze_vfs::ENOENT) => return false,
        Err(error) => {
            crate::set_errno(error);
            return false;
        }
    };
    let mut rest = data.as_slice();
    while !rest.trim_ascii_start().is_empty() {
        rest = rest.trim_ascii_start();
        if rest[0] == b'(' {
            let Some(end) = rest.iter().position(|&b| b == b')') else {
                return false;
            };
            let fields: Vec<_> = rest[1..end]
                .split(|&b| b == b',')
                .map(<[u8]>::trim_ascii)
                .collect();
            if fields.len() == 3
                && fields.iter().zip(wanted).all(|(field, value)| {
                    field.is_empty() || value.is_none_or(|value| *field == value)
                })
            {
                return true;
            }
            rest = &rest[end + 1..];
        } else {
            let Some(nested) = super::records::field(&mut rest) else {
                break;
            };
            if matches(nested, wanted, seen) {
                return true;
            }
        }
    }
    false
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_innetgr(
    group: *const c_char,
    host: *const c_char,
    user: *const c_char,
    domain: *const c_char,
) -> c_int {
    if group.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return 0;
    }
    let wanted = [host, user, domain].map(|value| {
        if value.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(value) }.to_bytes())
        }
    });
    c_int::from(matches(
        unsafe { CStr::from_ptr(group) }.to_bytes(),
        &wanted,
        &mut HashSet::new(),
    ))
}
