//! Guest allocation holds the public action array and its copied path strings.
//! Runtime already clones this arena; no native heap repair or registry is needed.
use super::{FileAction, PosixSpawnFileActions};
use core::{
    ffi::{CStr, c_char},
    mem::{align_of, size_of},
    ptr,
};
use kinakaze_alloc::guest;
use kinakaze_vfs::{EINVAL, ENOMEM};

#[repr(C)]
#[derive(Clone, Copy)]
struct Action {
    kind: i32,
    fd: i32,
    second: i32,
    mode: u32,
    path: *mut c_char,
}

unsafe fn copy_string(value: &CStr) -> Result<*mut c_char, i32> {
    let bytes = value.to_bytes_with_nul();
    let buffer = unsafe { guest::malloc(bytes.len()) };
    if buffer.is_null() {
        return Err(ENOMEM);
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len());
    }
    Ok(buffer.cast())
}

impl Action {
    unsafe fn new(action: &FileAction) -> Result<Self, i32> {
        let mut record = Self {
            kind: 0,
            fd: 0,
            second: 0,
            mode: 0,
            path: ptr::null_mut(),
        };
        match action {
            FileAction::Close(fd) => {
                record.kind = 1;
                record.fd = *fd;
            }
            FileAction::Dup2(fd, target) => {
                record.kind = 2;
                record.fd = *fd;
                record.second = *target;
            }
            FileAction::Open {
                fd,
                path,
                oflag,
                mode,
            } => {
                record.kind = 3;
                record.fd = *fd;
                record.second = *oflag;
                record.mode = *mode;
                record.path = unsafe { copy_string(path)? };
            }
            FileAction::Chdir(path) => {
                record.kind = 4;
                record.path = unsafe { copy_string(path)? };
            }
            FileAction::Fchdir(fd) => {
                record.kind = 5;
                record.fd = *fd;
            }
            FileAction::CloseFrom(fd) => {
                record.kind = 6;
                record.fd = *fd;
            }
            FileAction::Tcsetpgrp(fd) => {
                record.kind = 7;
                record.fd = *fd;
            }
        }
        Ok(record)
    }

    unsafe fn decode(&self) -> Result<FileAction, i32> {
        let path = || {
            if self.path.is_null() {
                return Err(EINVAL);
            }
            Ok(unsafe { CStr::from_ptr(self.path) }.to_owned())
        };
        Ok(match self.kind {
            1 => FileAction::Close(self.fd),
            2 => FileAction::Dup2(self.fd, self.second),
            3 => FileAction::Open {
                fd: self.fd,
                path: path()?,
                oflag: self.second,
                mode: self.mode,
            },
            4 => FileAction::Chdir(path()?),
            5 => FileAction::Fchdir(self.fd),
            6 => FileAction::CloseFrom(self.fd),
            7 => FileAction::Tcsetpgrp(self.fd),
            _ => return Err(EINVAL),
        })
    }
}

fn valid(fa: &PosixSpawnFileActions) -> bool {
    fa.used >= 0 && fa.allocated >= fa.used && (fa.allocated == 0 || !fa.actions.is_null())
}

pub(super) unsafe fn append(
    fa: &mut PosixSpawnFileActions,
    action: &FileAction,
) -> Result<(), i32> {
    if !valid(fa) {
        return Err(EINVAL);
    }
    if fa.used == i32::MAX {
        return Err(ENOMEM);
    }
    let record = unsafe { Action::new(action)? };
    if fa.used == fa.allocated {
        let capacity = fa.allocated.saturating_mul(2).max(4);
        let buffer = unsafe {
            guest::reallocate(
                fa.actions.cast(),
                align_of::<Action>(),
                capacity as usize * size_of::<Action>(),
            )
        };
        if buffer.is_null() {
            unsafe {
                guest::free(record.path.cast());
            }
            return Err(ENOMEM);
        }
        fa.actions = buffer.cast();
        fa.allocated = capacity;
    }
    unsafe {
        fa.actions
            .cast::<Action>()
            .add(fa.used as usize)
            .write(record);
    }
    fa.used += 1;
    Ok(())
}

pub(super) unsafe fn read(fa: *const PosixSpawnFileActions) -> Result<Vec<FileAction>, i32> {
    if fa.is_null() {
        return Ok(Vec::new());
    }
    let fa = unsafe { &*fa };
    if !valid(fa) {
        return Err(EINVAL);
    }
    let mut actions = Vec::new();
    actions
        .try_reserve_exact(fa.used as usize)
        .map_err(|_| ENOMEM)?;
    for i in 0..fa.used as usize {
        let record = unsafe { &*fa.actions.cast::<Action>().add(i) };
        actions.push(unsafe { record.decode()? });
    }
    Ok(actions)
}

pub(super) unsafe fn destroy(fa: &mut PosixSpawnFileActions) -> Result<(), i32> {
    if !valid(fa) {
        return Err(EINVAL);
    }
    for i in 0..fa.used as usize {
        let record = unsafe { &*fa.actions.cast::<Action>().add(i) };
        unsafe {
            guest::free(record.path.cast());
        }
    }
    unsafe {
        guest::free(fa.actions.cast());
        ptr::write_bytes(fa, 0, 1);
    }
    Ok(())
}

/// PATH candidates are one compact allocation, with no Rust pointer graph.
pub(super) struct Candidates {
    bytes: *mut u8,
    length: usize,
}
impl Candidates {
    pub fn new(paths: &[std::ffi::CString]) -> Result<Self, i32> {
        let length = paths
            .iter()
            .try_fold(0usize, |n, path| {
                n.checked_add(path.as_bytes_with_nul().len())
            })
            .ok_or(ENOMEM)?;
        let bytes = unsafe { guest::malloc(length) };
        if bytes.is_null() {
            return Err(ENOMEM);
        }
        let mut offset = 0;
        for path in paths {
            let source = path.as_bytes_with_nul();
            unsafe {
                ptr::copy_nonoverlapping(source.as_ptr(), bytes.add(offset), source.len());
            }
            offset += source.len();
        }
        Ok(Self { bytes, length })
    }
    pub fn iter(&self) -> impl Iterator<Item = &CStr> {
        let bytes = unsafe { core::slice::from_raw_parts(self.bytes, self.length) };
        bytes
            .split_inclusive(|byte| *byte == 0)
            .map(|path| CStr::from_bytes_with_nul(path).expect("owned PATH candidate"))
    }
}
impl Drop for Candidates {
    fn drop(&mut self) {
        unsafe {
            guest::free(self.bytes);
        }
    }
}
