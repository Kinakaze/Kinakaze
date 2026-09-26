//! Local mail aliases, including continuation lines in the guest database.
use super::records;
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use std::{ffi::CString, io::BufRead};
const PATH: &str = "/etc/aliases";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYALIA01");
}
super::records::returned::returned_record!(*b"CYALIR01");
#[repr(C)]
pub struct Aliasent {
    name: *mut c_char,
    members_len: usize,
    members: *mut *mut c_char,
    local: c_int,
}

fn next(reader: &mut impl BufRead) -> Result<Option<records::Names>, i32> {
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
        loop {
            let rest = reader
                .fill_buf()
                .map_err(|e| e.raw_os_error().unwrap_or(5))?;
            if !rest.first().is_some_and(|b| matches!(b, b' ' | b'\t')) {
                break;
            }
            reader
                .read_until(b'\n', &mut line)
                .map_err(|e| e.raw_os_error().unwrap_or(5))?;
        }
        let clean: Vec<u8> = line
            .split(|b| *b == b'\n')
            .flat_map(|l| {
                l.split(|b| *b == b'#')
                    .next()
                    .unwrap_or_default()
                    .iter()
                    .copied()
                    .chain([b' '])
            })
            .collect();
        if clean.contains(&0) {
            continue;
        }
        let Some(colon) = clean.iter().position(|b| *b == b':') else {
            continue;
        };
        let name = clean[..colon].trim_ascii();
        if name.is_empty() {
            continue;
        }
        let members = clean[colon + 1..]
            .split(|b| *b == b',')
            .map(|s| s.trim_ascii())
            .filter(|s| !s.is_empty())
            .map(|s| CString::new(s).unwrap())
            .collect();
        return Ok(Some(records::Names::new(
            CString::new(name).unwrap(),
            members,
        )));
    }
}
fn publish(outcome: Result<Option<records::Names>, i32>) -> *mut Aliasent {
    let names = match outcome {
        Ok(Some(names)) => names,
        Ok(None) => return ptr::null_mut(),
        Err(error) => {
            crate::set_errno(error);
            return ptr::null_mut();
        }
    };
    let allocated = register_returned()
        .and_then(|()| records::returned::Record::create(&names, core::mem::size_of::<Aliasent>()));
    let (allocation, name, members) = match allocated {
        Ok(value) => value,
        Err(error) => {
            crate::set_errno(error);
            return ptr::null_mut();
        }
    };
    let output = allocation.0 as *mut Aliasent;
    unsafe {
        output.write(Aliasent {
            name,
            members,
            members_len: names.aliases().len(),
            local: 1,
        });
    }
    RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
    output
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setaliasent() {
    if let Err(e) = cursor::rewind() {
        crate::set_errno(e);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endaliasent() {
    if let Err(e) = cursor::close() {
        crate::set_errno(e);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getaliasent() -> *mut Aliasent {
    publish(cursor::next(next))
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getaliasbyname(name: *const c_char) -> *mut Aliasent {
    if name.is_null() {
        crate::set_errno(22);
        return ptr::null_mut();
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    publish((|| {
        let mut reader = records::Reader::open(PATH)?;
        while let Some(entry) = next(&mut reader)? {
            if entry.name.as_bytes().eq_ignore_ascii_case(name) {
                return Ok(Some(entry));
            }
        }
        Ok(None)
    })())
}
