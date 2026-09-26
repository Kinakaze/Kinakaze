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

unsafe fn pack(
    entry: &Entry,
    output: *mut Sgrp,
    buffer: *mut c_char,
    size: usize,
) -> Result<(), i32> {
    let alignment = core::mem::align_of::<*mut c_char>();
    let padding = (buffer as usize).wrapping_neg() & (alignment - 1);
    let table_size = (entry.administrators.len() + entry.members.len() + 2) * alignment;
    let strings: Vec<_> = [&entry.name, &entry.password]
        .into_iter()
        .chain(entry.administrators.iter())
        .chain(entry.members.iter())
        .collect();
    let needed = padding + table_size + strings.iter().map(|s| s.len() + 1).sum::<usize>();
    if size < needed {
        return Err(34);
    }
    unsafe {
        let administrators = buffer.byte_add(padding).cast::<*mut c_char>();
        let members = administrators.add(entry.administrators.len() + 1);
        let mut data = buffer.byte_add(padding + table_size);
        let mut pointers = Vec::with_capacity(strings.len());
        for value in strings {
            pointers.push(data);
            ptr::copy_nonoverlapping(value.as_ptr(), data.cast(), value.len());
            data.add(value.len()).write(0);
            data = data.add(value.len() + 1);
        }
        for (n, &value) in pointers[2..2 + entry.administrators.len()]
            .iter()
            .enumerate()
        {
            administrators.add(n).write(value);
        }
        administrators
            .add(entry.administrators.len())
            .write(ptr::null_mut());
        for (n, &value) in pointers[2 + entry.administrators.len()..]
            .iter()
            .enumerate()
        {
            members.add(n).write(value);
        }
        members.add(entry.members.len()).write(ptr::null_mut());
        output.write(Sgrp {
            name: pointers[0],
            password: pointers[1],
            administrators,
            members,
        });
    }
    Ok(())
}

/// # Safety
/// The caller supplies a name, writable record, byte buffer and result pointer.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getsgnam_r(
    name: *const c_char,
    output: *mut Sgrp,
    buffer: *mut c_char,
    size: usize,
    result: *mut *mut Sgrp,
) -> c_int {
    if result.is_null() {
        return 22;
    }
    unsafe {
        result.write(ptr::null_mut());
    }
    if name.is_null() || output.is_null() || buffer.is_null() {
        return 22;
    }
    match lookup(unsafe { CStr::from_ptr(name) }.to_bytes()) {
        Ok(Some(entry)) => match unsafe { pack(&entry, output, buffer, size) } {
            Ok(()) => {
                unsafe {
                    result.write(output);
                }
                0
            }
            Err(error) => error,
        },
        Ok(None) => 0,
        Err(error) => error,
    }
}

/// # Safety
/// file must identify an open readable guest FILE, not a host CRT stream.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetsgent(file: *mut crate::stdio::File) -> *mut Sgrp {
    if file.is_null() {
        crate::set_errno(22);
        return ptr::null_mut();
    }
    loop {
        let mut line = Vec::new();
        loop {
            let byte = crate::stdio::fgetc(file);
            if byte < 0 || byte == 10 {
                break;
            }
            line.push(byte as u8);
        }
        if crate::stdio::ferror(file) != 0 {
            crate::set_errno(5);
            return ptr::null_mut();
        }
        if line.is_empty() && crate::stdio::feof(file) != 0 {
            return ptr::null_mut();
        }
        match next(&mut line.as_slice()) {
            Ok(None) => continue,
            outcome => return publish(outcome),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reentrant_record_aligns_tables_and_preserves_every_name() {
        let entry = next(&mut b"staff:!:root,admin:alice,bob\n".as_slice())
            .unwrap()
            .unwrap();
        let mut bytes = [0u8; 256];
        let buffer = unsafe { bytes.as_mut_ptr().add(1).cast() };
        let mut output = core::mem::MaybeUninit::<Sgrp>::uninit();
        assert_eq!(
            unsafe { pack(&entry, output.as_mut_ptr(), buffer, 1) },
            Err(34)
        );
        unsafe {
            pack(&entry, output.as_mut_ptr(), buffer, 255).unwrap();
            let output = output.assume_init();
            assert_eq!(output.administrators as usize % 8, 0);
            assert_eq!(CStr::from_ptr(output.name).to_bytes(), b"staff");
            assert_eq!(
                CStr::from_ptr(*output.administrators.add(1)).to_bytes(),
                b"admin"
            );
            assert_eq!(CStr::from_ptr(*output.members.add(1)).to_bytes(), b"bob");
            assert!((*output.administrators.add(2)).is_null());
            assert!((*output.members.add(2)).is_null());
        }
    }
}
