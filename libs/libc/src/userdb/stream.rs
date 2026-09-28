//! FILE account parsers share the NSS grammar and return forkable guest storage.
use super::*;
use crate::netdb::records::returned::{Record, returned_record};
use crate::stdio::File;

type Parser<T> =
    unsafe extern "sysv64" fn(*mut c_char, *mut T, *mut c_char, usize, *mut c_int) -> c_int;
type Reader<T> =
    unsafe extern "sysv64" fn(*mut File, *mut T, *mut c_char, usize, *mut *mut T) -> c_int;

pub(super) unsafe fn read<T>(
    file: *mut File,
    entry: *mut T,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut T,
    parser: Parser<T>,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    unsafe { *result = ptr::null_mut() };
    if file.is_null() || entry.is_null() || buffer.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    let code = unsafe {
        crate::stdio::line::account_entry(file, buffer, length, |line| {
            let mut error = 0;
            parser(line, entry, buffer, length, &mut error)
        })
    };
    if code == 0 {
        unsafe { *result = entry };
    }
    code
}

#[repr(C)]
struct Storage<T> {
    capacity: usize,
    entry: T,
}

unsafe fn owned<T>(file: *mut File, record: &mut Record, read: Reader<T>) -> Result<*mut T, c_int> {
    let mut length = if record.0 == 0 {
        0
    } else {
        unsafe { (*(record.0 as *mut Storage<T>)).capacity }
    };
    let mut needed = length.max(1024);
    loop {
        if length < needed {
            let size = size_of::<Storage<T>>()
                .checked_add(needed)
                .ok_or(kinakaze_vfs::ENOMEM)?;
            let base = unsafe { kinakaze_alloc::guest::reallocate(record.0 as *mut u8, 16, size) };
            if base.is_null() {
                return Err(kinakaze_vfs::ENOMEM);
            }
            record.0 = base as usize;
            length = needed;
            unsafe { (*(base.cast::<Storage<T>>())).capacity = length };
        }
        let base = record.0 as *mut Storage<T>;
        let entry = unsafe { ptr::addr_of_mut!((*base).entry) };
        let mut result = ptr::null_mut();
        let code = unsafe { read(file, entry, base.add(1).cast(), length, &mut result) };
        match code {
            0 => return Ok(entry),
            ERANGE => needed = length.checked_mul(2).ok_or(kinakaze_vfs::ENOMEM)?,
            error => return Err(error),
        }
    }
}

macro_rules! file_record {
    ($module:ident, $magic:expr, $record:ty, $reader:ident) => {
        pub(super) mod $module {
            use super::*;
            returned_record!($magic);
            pub(in super::super) unsafe fn read(file: *mut File) -> *mut $record {
                let outcome = register_returned().and_then(|()| {
                    // Keep capacity in the forked guest allocation, not a host
                    // TLS field. Release the slot borrow before calling FILE.
                    let mut record = RETURNED
                        .with(|slot| slot.borrow_mut().take())
                        .unwrap_or(Record(0));
                    let outcome = unsafe { owned(file, &mut record, $reader) };
                    RETURNED.with(|slot| *slot.borrow_mut() = (record.0 != 0).then_some(record));
                    outcome
                });
                outcome.unwrap_or_else(|error| {
                    crate::set_errno(error);
                    ptr::null_mut()
                })
            }
        }
    };
}
file_record!(passwd, *b"CYFPWR01", Passwd, kinakaze_abi_fgetpwent_r);
file_record!(group, *b"CYFGRR01", Group, kinakaze_abi_fgetgrent_r);
