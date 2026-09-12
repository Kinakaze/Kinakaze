//! shadow.h parsing and passwd/group/shadow record output.
//! Returned records and their strings live in the guest arena across fork.
use super::{Group, Passwd, Spwd};
use crate::stdio::File;
use core::{
    ffi::{CStr, c_char, c_int},
    ptr,
};
use kinakaze_alloc::guest;
use kinakaze_vfs::{EINVAL, ENOMEM, EOVERFLOW};
use std::sync::Mutex;

#[derive(Default)]
struct Record {
    entry: usize,
    text: usize,
    capacity: usize,
}

impl Record {
    const fn new() -> Self {
        Self {
            entry: 0,
            text: 0,
            capacity: 0,
        }
    }

    fn reserve(&mut self, size: usize) -> Result<(), i32> {
        if self.entry == 0 {
            self.entry = unsafe { guest::malloc(size_of::<Spwd>()) } as usize;
            if self.entry == 0 {
                return Err(ENOMEM);
            }
        }
        if size > self.capacity {
            let pointer = unsafe { guest::reallocate(self.text as *mut u8, 16, size) };
            if pointer.is_null() {
                return Err(ENOMEM);
            }
            self.text = pointer as usize;
            self.capacity = size;
        }
        Ok(())
    }

    unsafe fn parse(&self) -> *mut Spwd {
        match unsafe { parse_shadow(self.text as *mut c_char) } {
            Some(value) => {
                let entry = self.entry as *mut Spwd;
                unsafe { entry.write(value) };
                entry
            }
            None => ptr::null_mut(),
        }
    }
}

// glibc gives sgetspent and fgetspent separate reusable static records.
static STRING_RECORD: Mutex<Record> = Mutex::new(Record::new());
static FILE_RECORD: Mutex<Record> = Mutex::new(Record::new());

unsafe fn field(cursor: &mut *mut c_char) -> *mut c_char {
    let start = *cursor;
    unsafe {
        while **cursor != 0 && **cursor != b':' as c_char {
            *cursor = (*cursor).add(1);
        }
        if **cursor != 0 {
            **cursor = 0;
            *cursor = (*cursor).add(1);
        }
    }
    start
}

// The Linux shadow parser uses unsigned conversion clamped to 32 bits, then
// casts aging fields through signed int. Empty fields have their own sentinel.
unsafe fn number(cursor: &mut *mut c_char, colon: bool, empty: u64) -> Option<u64> {
    if unsafe { **cursor } == 0 {
        return None;
    }
    let mut end = ptr::null();
    let value = unsafe { crate::process::kinakaze_abi_strtoul(*cursor, &mut end, 10) };
    let value = if end == *cursor {
        empty
    } else {
        value.min(u32::MAX as u64)
    };
    match unsafe { *end } as u8 {
        b':' if colon => *cursor = unsafe { end.add(1).cast_mut() },
        0 => *cursor = end.cast_mut(),
        _ => return None,
    }
    Some(value)
}

unsafe fn parse_shadow(text: *mut c_char) -> Option<Spwd> {
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    if let Some(end) = bytes.iter().position(|&b| b == b'\n') {
        unsafe { text.add(end).write(0) };
    }
    let mut cursor = text;
    let name = unsafe { field(&mut cursor) };
    let mut entry = Spwd {
        sp_namp: name,
        sp_pwdp: ptr::null_mut(),
        sp_lstchg: 0,
        sp_min: 0,
        sp_max: 0,
        sp_warn: -1,
        sp_inact: -1,
        sp_expire: -1,
        sp_flag: u64::MAX,
    };
    if unsafe { *cursor == 0 && matches!(*name as u8, b'+' | b'-') } {
        return Some(entry);
    }
    entry.sp_pwdp = unsafe { field(&mut cursor) };
    entry.sp_lstchg = unsafe { number(&mut cursor, true, u64::MAX)? } as i32 as i64;
    entry.sp_min = unsafe { number(&mut cursor, true, u64::MAX)? } as i32 as i64;
    entry.sp_max = unsafe { number(&mut cursor, true, u64::MAX)? } as i32 as i64;
    while unsafe { (*cursor as u8).is_ascii_whitespace() } {
        cursor = unsafe { cursor.add(1) };
    }
    if unsafe { *cursor } != 0 {
        entry.sp_warn = unsafe { number(&mut cursor, true, u64::MAX)? } as i32 as i64;
        entry.sp_inact = unsafe { number(&mut cursor, true, u64::MAX)? } as i32 as i64;
        entry.sp_expire = unsafe { number(&mut cursor, true, u64::MAX)? } as i32 as i64;
        if unsafe { *cursor } != 0 {
            entry.sp_flag = unsafe { number(&mut cursor, false, u64::MAX)? };
        }
    }
    Some(entry)
}

/// Parse a shadow record without modifying the caller's string.
/// # Safety
/// `text` must be a readable NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sgetspent(text: *const c_char) -> *mut Spwd {
    if text.is_null() {
        crate::set_errno(EINVAL);
        return ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes_with_nul();
    let mut record = STRING_RECORD.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(errno) = record.reserve(bytes.len()) {
        crate::set_errno(errno);
        return ptr::null_mut();
    }
    unsafe {
        ptr::copy(bytes.as_ptr(), record.text as *mut u8, bytes.len());
        record.parse()
    }
}

/// Read the next valid shadow record, skipping comments and malformed lines.
/// # Safety
/// `file` must be a live readable FILE.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetspent(file: *mut File) -> *mut Spwd {
    let mut record = FILE_RECORD.lock().unwrap_or_else(|e| e.into_inner());
    if let Err(errno) = record.reserve(256) {
        crate::set_errno(errno);
        return ptr::null_mut();
    }
    loop {
        let mut text = record.text as *mut c_char;
        let mut capacity = record.capacity;
        let read = unsafe { crate::stdio::line::account_line(file, &mut text, &mut capacity) };
        record.text = text as usize;
        record.capacity = capacity;
        if let Err(errno) = read {
            crate::set_errno(errno);
            return ptr::null_mut();
        }
        while unsafe { (*text as u8).is_ascii_whitespace() } {
            text = unsafe { text.add(1) };
        }
        if unsafe { matches!(*text as u8, 0 | b'#') } {
            continue;
        }
        if let Some(entry) = unsafe { parse_shadow(text) } {
            let pointer = record.entry as *mut Spwd;
            unsafe { pointer.write(entry) };
            return pointer;
        }
    }
}

unsafe fn bytes<'a>(value: *const c_char) -> &'a [u8] {
    if value.is_null() {
        &[]
    } else {
        unsafe { CStr::from_ptr(value) }.to_bytes()
    }
}
fn valid(value: &[u8]) -> bool {
    !value.iter().any(|b| matches!(*b, b':' | b'\n'))
}

// Assemble one record before taking the FILE lock. A single fwrite keeps
// concurrent writers from interleaving fields. No host Vec crosses callbacks.
struct Line {
    pointer: *mut u8,
    length: usize,
    capacity: usize,
}
impl Line {
    fn new() -> Self {
        Self {
            pointer: ptr::null_mut(),
            length: 0,
            capacity: 0,
        }
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), i32> {
        let end = self.length.checked_add(bytes.len()).ok_or(EOVERFLOW)?;
        if end > self.capacity {
            let size = end.max(self.capacity.saturating_mul(2)).max(256);
            let grown = unsafe { guest::reallocate(self.pointer, 16, size) };
            if grown.is_null() {
                return Err(ENOMEM);
            }
            self.pointer = grown;
            self.capacity = size;
        }
        if !bytes.is_empty() {
            unsafe {
                ptr::copy_nonoverlapping(bytes.as_ptr(), self.pointer.add(self.length), bytes.len())
            };
        }
        self.length = end;
        Ok(())
    }
    fn decimal(&mut self, value: i64) -> Result<(), i32> {
        let mut digits = [0u8; 21];
        let mut cursor = digits.len();
        let mut magnitude = value.unsigned_abs();
        loop {
            cursor -= 1;
            digits[cursor] = b'0' + (magnitude % 10) as u8;
            magnitude /= 10;
            if magnitude == 0 {
                break;
            }
        }
        if value < 0 {
            cursor -= 1;
            digits[cursor] = b'-';
        }
        self.append(&digits[cursor..])
    }
    fn write(self, file: *mut File) -> Result<(), i32> {
        let count = unsafe { crate::stdio::fwrite(self.pointer.cast(), 1, self.length, file) };
        if count == self.length {
            Ok(())
        } else {
            Err(crate::kinakaze_errno())
        }
    }
}
impl Drop for Line {
    fn drop(&mut self) {
        unsafe { guest::free(self.pointer) };
    }
}

fn output(file: *mut File, build: impl FnOnce(&mut Line) -> Result<(), i32>) -> c_int {
    let result = (|| {
        if file.is_null() {
            return Err(EINVAL);
        }
        let mut line = Line::new();
        build(&mut line)?;
        line.append(b"\n")?;
        line.write(file)
    })();
    match result {
        Ok(()) => 0,
        Err(errno) => {
            crate::set_errno(errno);
            -1
        }
    }
}

/// # Safety
/// `entry` and its fields must be readable; `file` must be a writable FILE.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_putpwent(
    entry: *const Passwd,
    file: *mut File,
) -> c_int {
    output(file, |line| unsafe {
        let entry = entry.as_ref().ok_or(EINVAL)?;
        if entry.pw_name.is_null() {
            return Err(EINVAL);
        }
        let fields =
            [entry.pw_name, entry.pw_passwd, entry.pw_dir, entry.pw_shell].map(|p| bytes(p));
        if !fields.iter().all(|f| valid(f)) {
            return Err(EINVAL);
        }
        line.append(fields[0])?;
        line.append(b":")?;
        line.append(fields[1])?;
        line.append(b":")?;
        let compat = matches!(fields[0].first(), Some(b'+' | b'-'));
        if !compat {
            line.decimal(entry.pw_uid as i64)?;
        }
        line.append(b":")?;
        if !compat {
            line.decimal(entry.pw_gid as i64)?;
        }
        line.append(b":")?;
        for &byte in bytes(entry.pw_gecos) {
            line.append(&[if matches!(byte, b':' | b'\n') {
                b' '
            } else {
                byte
            }])?;
        }
        line.append(b":")?;
        line.append(fields[2])?;
        line.append(b":")?;
        line.append(fields[3])
    })
}

/// # Safety
/// `entry` including its NUL-terminated member vector must be readable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_putgrent(entry: *const Group, file: *mut File) -> c_int {
    output(file, |line| unsafe {
        let entry = entry.as_ref().ok_or(EINVAL)?;
        let name = bytes(entry.gr_name);
        let password = bytes(entry.gr_passwd);
        if entry.gr_name.is_null() || !valid(name) || !valid(password) {
            return Err(EINVAL);
        }
        line.append(name)?;
        line.append(b":")?;
        line.append(password)?;
        line.append(b":")?;
        if !matches!(name.first(), Some(b'+' | b'-')) {
            line.decimal(entry.gr_gid as i64)?;
        }
        line.append(b":")?;
        let mut member = entry.gr_mem;
        if !member.is_null() {
            let mut first = true;
            while !(*member).is_null() {
                let text = bytes(*member);
                if !valid(text) || text.contains(&b',') {
                    return Err(EINVAL);
                }
                if !first {
                    line.append(b",")?;
                }
                line.append(text)?;
                first = false;
                member = member.add(1);
            }
        }
        Ok(())
    })
}

/// # Safety
/// `entry` and its strings must be readable; `file` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_putspent(entry: *const Spwd, file: *mut File) -> c_int {
    output(file, |line| unsafe {
        let entry = entry.as_ref().ok_or(EINVAL)?;
        let name = bytes(entry.sp_namp);
        let password = bytes(entry.sp_pwdp);
        if entry.sp_namp.is_null() || !valid(name) || !valid(password) {
            return Err(EINVAL);
        }
        line.append(name)?;
        line.append(b":")?;
        line.append(password)?;
        for value in [
            entry.sp_lstchg,
            entry.sp_min,
            entry.sp_max,
            entry.sp_warn,
            entry.sp_inact,
            entry.sp_expire,
        ] {
            line.append(b":")?;
            if value != -1 {
                line.decimal(value)?;
            }
        }
        line.append(b":")?;
        if entry.sp_flag != u64::MAX {
            line.decimal(entry.sp_flag as i64)?;
        }
        Ok(())
    })
}
