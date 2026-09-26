//! Guest group-shadow database. Returned records live in the forkable guest heap.
use super::records;
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use std::io::BufRead;
const PATH: &str = "/etc/gshadow";
mod cursor {
    super::super::records::cursor::database_cursor!(*b"CYGSHD01");
}
super::records::returned::returned_record!(*b"CYGSHR01");

#[repr(C)]
pub struct Sgrp {
    name: *mut c_char,
    password: *mut c_char,
    administrators: *mut *mut c_char,
    members: *mut *mut c_char,
}
struct Entry {
    name: Vec<u8>,
    password: Vec<u8>,
    administrators: Vec<Vec<u8>>,
    members: Vec<Vec<u8>>,
}
fn next(reader: &mut impl BufRead) -> Result<Option<Entry>, i32> {
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
        let line = line.trim_ascii_end();
        if line.contains(&0) || line.starts_with(b"#") {
            continue;
        }
        let fields: Vec<_> = line.split(|b| *b == b':').collect();
        if fields.len() != 4 || fields[0].is_empty() {
            continue;
        }
        let names = |s: &[u8]| {
            s.split(|b| *b == b',')
                .filter(|s| !s.is_empty())
                .map(Vec::from)
                .collect()
        };
        return Ok(Some(Entry {
            name: fields[0].to_vec(),
            password: fields[1].to_vec(),
            administrators: names(fields[2]),
            members: names(fields[3]),
        }));
    }
}
fn lookup(name: &[u8]) -> Result<Option<Entry>, i32> {
    let mut reader = records::Reader::open(PATH)?;
    while let Some(entry) = next(&mut reader)? {
        if entry.name == name {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}
fn publish(outcome: Result<Option<Entry>, i32>) -> *mut Sgrp {
    let entry = match outcome {
        Ok(Some(entry)) => entry,
        Ok(None) => return ptr::null_mut(),
        Err(e) => {
            crate::set_errno(e);
            return ptr::null_mut();
        }
    };
    if let Err(e) = register_returned() {
        crate::set_errno(e);
        return ptr::null_mut();
    }
    let names: Vec<_> = [&entry.name, &entry.password]
        .into_iter()
        .chain(entry.administrators.iter())
        .chain(entry.members.iter())
        .collect();
    let table_size = (entry.administrators.len() + entry.members.len() + 2)
        * core::mem::size_of::<*mut c_char>();
    let length = core::mem::size_of::<Sgrp>()
        + table_size
        + names.iter().map(|s| s.len() + 1).sum::<usize>();
    let bytes = unsafe { kinakaze_alloc::guest::malloc(length) };
    if bytes.is_null() {
        crate::set_errno(12);
        return ptr::null_mut();
    }
    let allocation = records::returned::Record(bytes as usize);
    unsafe {
        let output = bytes.cast::<Sgrp>();
        let administrators = bytes
            .add(core::mem::size_of::<Sgrp>())
            .cast::<*mut c_char>();
        let members = administrators.add(entry.administrators.len() + 1);
        let mut data = bytes.add(core::mem::size_of::<Sgrp>() + table_size);
        let mut strings = Vec::with_capacity(names.len());
        for value in names {
            strings.push(data.cast::<c_char>());
            ptr::copy_nonoverlapping(value.as_ptr(), data, value.len());
            data.add(value.len()).write(0);
            data = data.add(value.len() + 1);
        }
        for i in 0..entry.administrators.len() {
            administrators.add(i).write(strings[2 + i]);
        }
        administrators
            .add(entry.administrators.len())
            .write(ptr::null_mut());
        for i in 0..entry.members.len() {
            members
                .add(i)
                .write(strings[2 + entry.administrators.len() + i]);
        }
        members.add(entry.members.len()).write(ptr::null_mut());
        output.write(Sgrp {
            name: strings[0],
            password: strings[1],
            administrators,
            members,
        });
        RETURNED.with(|slot| *slot.borrow_mut() = Some(allocation));
        output
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setsgent() {
    if let Err(e) = cursor::rewind() {
        crate::set_errno(e);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endsgent() {
    if let Err(e) = cursor::close() {
        crate::set_errno(e);
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getsgent() -> *mut Sgrp {
    publish(cursor::next(next))
}
/// # Safety
/// name is a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getsgnam(name: *const c_char) -> *mut Sgrp {
    if name.is_null() {
        crate::set_errno(22);
        return ptr::null_mut();
    }
    publish(lookup(unsafe { CStr::from_ptr(name) }.to_bytes()))
}
/// # Safety
/// entry and its strings/tables are readable; file is a valid writable stream.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_putsgent(
    entry: *const Sgrp,
    file: *mut crate::stdio::File,
) -> c_int {
    fn valid(s: &[u8]) -> bool {
        !s.iter().any(|b| matches!(b, b':' | b'\n' | b'\r'))
    }
    unsafe fn append_table(output: &mut Vec<u8>, mut table: *mut *mut c_char) -> bool {
        if table.is_null() {
            return true;
        }
        let mut first = true;
        while !unsafe { *table }.is_null() {
            let name = unsafe { CStr::from_ptr(*table) }.to_bytes();
            if !valid(name) || name.contains(&b',') || name.is_empty() {
                return false;
            }
            if !first {
                output.push(b',');
            }
            first = false;
            output.extend_from_slice(name);
            table = unsafe { table.add(1) };
        }
        true
    }
    if entry.is_null() || file.is_null() {
        crate::set_errno(22);
        return -1;
    }
    let entry = unsafe { &*entry };
    if entry.name.is_null() || entry.password.is_null() {
        crate::set_errno(22);
        return -1;
    }
    let name = unsafe { CStr::from_ptr(entry.name) }.to_bytes();
    let password = unsafe { CStr::from_ptr(entry.password) }.to_bytes();
    if name.is_empty() || !valid(name) || !valid(password) {
        crate::set_errno(22);
        return -1;
    }
    let mut output = name.to_vec();
    output.push(b':');
    output.extend_from_slice(password);
    output.push(b':');
    if !unsafe { append_table(&mut output, entry.administrators) } {
        crate::set_errno(22);
        return -1;
    }
    output.push(b':');
    if !unsafe { append_table(&mut output, entry.members) } {
        crate::set_errno(22);
        return -1;
    }
    output.push(b'\n');
    if unsafe { crate::stdio::fwrite(output.as_ptr().cast(), 1, output.len(), file) }
        == output.len()
    {
        0
    } else {
        -1
    }
}
