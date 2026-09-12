//! GNU custom streams. Callbacks use the Linux System V ABI, including off64_t.
//! Reference: https://codebrowser.dev/glibc/glibc/libio/iofopncook.c.html
use super::{Buffering, File, Stream, parse_mode, publish};
use core::ffi::{c_char, c_int, c_void};
use kinakaze_vfs::{EBADF, EINVAL, EIO, ESPIPE};

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Functions {
    pub read: Option<unsafe extern "sysv64" fn(*mut c_void, *mut u8, usize) -> isize>,
    pub write: Option<unsafe extern "sysv64" fn(*mut c_void, *const u8, usize) -> isize>,
    pub seek: Option<unsafe extern "sysv64" fn(*mut c_void, *mut i64, c_int) -> c_int>,
    pub close: Option<unsafe extern "sysv64" fn(*mut c_void) -> c_int>,
}

#[derive(Clone, Copy)]
pub(super) struct Cookie {
    pub context: usize,
    pub functions: Functions,
    pub readable: bool,
    pub writable: bool,
    pub append: bool,
}

impl Cookie {
    pub fn read(&self, bytes: &mut [u8]) -> Result<usize, i32> {
        if !self.readable {
            return Err(EBADF);
        }
        let read = self.functions.read.ok_or(EBADF)?;
        let count = unsafe { read(self.context as _, bytes.as_mut_ptr(), bytes.len()) };
        checked_count(count, bytes.len())
    }

    pub fn write(&self, bytes: &[u8]) -> Result<usize, i32> {
        if !self.writable {
            return Err(EBADF);
        }
        let write = self.functions.write.ok_or(EBADF)?;
        if self.append {
            self.seek(0, 2)?;
        }
        let count = unsafe { write(self.context as _, bytes.as_ptr(), bytes.len()) };
        checked_count(count, bytes.len())
    }

    pub fn seek(&self, mut offset: i64, whence: i32) -> Result<u64, i32> {
        if !(0..=2).contains(&whence) {
            return Err(EINVAL);
        }
        let seek = self.functions.seek.ok_or(ESPIPE)?;
        if unsafe { seek(self.context as _, &mut offset, whence) } != 0 {
            return Err(kinakaze_tls::errno());
        }
        u64::try_from(offset).map_err(|_| EINVAL)
    }

    pub fn close(&self) -> Result<(), i32> {
        match self.functions.close {
            Some(close) if unsafe { close(self.context as _) } != 0 => Err(kinakaze_tls::errno()),
            _ => Ok(()),
        }
    }
}

fn checked_count(count: isize, capacity: usize) -> Result<usize, i32> {
    if count < 0 {
        return Err(kinakaze_tls::errno());
    }
    let count = count as usize;
    if count > capacity {
        return Err(EIO);
    }
    Ok(count)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fopencookie(
    context: *mut c_void,
    mode: *const c_char,
    functions: Functions,
) -> *mut File {
    if mode.is_null() {
        crate::set_errno(EINVAL);
        return core::ptr::null_mut();
    }
    let mode = unsafe { core::ffi::CStr::from_ptr(mode) }.to_bytes();
    let Some((flags, _)) = parse_mode(mode) else {
        crate::set_errno(EINVAL);
        return core::ptr::null_mut();
    };
    let access = flags & 3;
    let mut stream = Stream::new(-1, Buffering::Full).with_access(flags);
    stream.cookie = Some(Box::new(Cookie {
        context: context as usize,
        functions,
        readable: access != kinakaze_vfs::fs::O_WRONLY,
        writable: access != kinakaze_vfs::fs::O_RDONLY,
        append: flags & kinakaze_vfs::fs::O_APPEND != 0,
    }));
    publish(stream)
}
