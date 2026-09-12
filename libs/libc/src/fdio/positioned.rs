//! Positional vector transfers share one native pin and allocate no data-copy
//! buffer. Native and tmpfs offsets are explicit; no seek/restore emulation.

use super::{IoVec, c_int, c_void, read_at, set_errno};
use kinakaze_vfs::{EBADF, EFAULT, EINVAL, EISDIR, EOPNOTSUPP, ESPIPE, FdFlags, FdKind};

fn result(value: Result<usize, i32>) -> isize {
    match value {
        Ok(count) => count as isize,
        Err(error) => {
            if error == 27 {
                kinakaze_vfs::signal::deliver_pending();
            }
            set_errno(error);
            -1
        }
    }
}

/// Stream fallback for offset -1 and the original readv/writev entry points.
/// Validate the entire length before I/O and preserve bytes moved before a
/// later fault. Large vectors use no aggregate allocation.
pub(super) unsafe fn stream(fd: c_int, iov: *const IoVec, count: c_int, writing: bool) -> isize {
    let operation = || -> Result<usize, i32> {
        let limit = unsafe { validate(iov, count)? }.min(0x7fff_f000);
        let entry = kinakaze_vfs::get(fd)?;
        if entry.flags.contains(FdFlags::PATH_ONLY)
            || (writing
                && entry.flags.contains(FdFlags::READ_ACCESS)
                && !entry.flags.contains(FdFlags::WRITE_ACCESS))
            || (!writing
                && entry.flags.contains(FdFlags::WRITE_ACCESS)
                && !entry.flags.contains(FdFlags::READ_ACCESS))
        {
            return Err(EBADF);
        }
        let mut total = 0usize;
        for index in 0..count as usize {
            if total == limit {
                break;
            }
            let item = unsafe { &*iov.add(index) };
            let wanted = item.iov_len.min(limit - total);
            if wanted == 0 {
                continue;
            }
            if item.iov_base.is_null() {
                return if total == 0 { Err(EFAULT) } else { Ok(total) };
            }
            let amount = if writing {
                unsafe { crate::kinakaze_write(fd, item.iov_base, wanted) }
            } else {
                unsafe { crate::kinakaze_read(fd, item.iov_base, wanted) }
            };
            if amount < 0 {
                return if total == 0 {
                    Err(kinakaze_tls::errno())
                } else {
                    Ok(total)
                };
            }
            total += amount as usize;
            if (amount as usize) < wanted {
                break;
            }
        }
        Ok(total)
    };
    result(operation())
}

/// Validate vector metadata before any I/O, including aggregate ssize_t overflow.
unsafe fn validate(iov: *const IoVec, count: c_int) -> Result<usize, i32> {
    if !(0..=1024).contains(&count) {
        return Err(EINVAL);
    }
    if iov.is_null() && count != 0 {
        return Err(EFAULT);
    }
    let mut length = 0usize;
    for index in 0..count as usize {
        let item = unsafe { &*iov.add(index) };
        length = length
            .checked_add(item.iov_len)
            .filter(|n| *n <= isize::MAX as usize)
            .ok_or(EINVAL)?;
    }
    Ok(length)
}

unsafe fn attempt(
    fd: c_int,
    iov: *const IoVec,
    count: c_int,
    offset: i64,
    writing: bool,
) -> Result<usize, i32> {
    if offset < 0 {
        return Err(EINVAL);
    }
    let length = unsafe { validate(iov, count)? };
    let entry = kinakaze_vfs::get(fd)?;
    if entry.flags.contains(FdFlags::PATH_ONLY)
        || (writing
            && entry.flags.contains(FdFlags::READ_ACCESS)
            && !entry.flags.contains(FdFlags::WRITE_ACCESS))
        || (!writing
            && entry.flags.contains(FdFlags::WRITE_ACCESS)
            && !entry.flags.contains(FdFlags::READ_ACCESS))
    {
        return Err(EBADF);
    }
    if matches!(
        entry.kind,
        FdKind::Directory | FdKind::SyntheticDirectory | FdKind::TmpfsDirectory
    ) {
        return Err(EISDIR);
    }
    // Pread/pwrite on pipes must not consume or produce stream data.
    let tmpfs = matches!(
        entry.kind,
        FdKind::TmpfsFile | FdKind::MessageQueue | FdKind::SysfsFile
    );
    let device = matches!(
        entry.kind,
        FdKind::Null | FdKind::Zero | FdKind::Full | FdKind::Random
    );
    if !tmpfs && !device && !entry.flags.contains(FdFlags::SEEKABLE) {
        return Err(ESPIPE);
    }
    let native = if entry.kind == FdKind::File {
        Some(kinakaze_vfs::positional::File::open(fd, writing)?)
    } else {
        None
    };
    if writing && !tmpfs && !device && native.is_none() {
        return Err(EOPNOTSUPP);
    }
    if length == 0 {
        return Ok(0);
    }
    // Match Linux's transfer bound without allocating an aggregate buffer.
    let limit = length.min(0x7fff_f000);
    let mut total = 0usize;
    for index in 0..count as usize {
        let item = unsafe { &*iov.add(index) };
        let wanted = item.iov_len.min(limit - total);
        if wanted == 0 {
            continue;
        }
        let outcome = if item.iov_base.is_null() {
            Err(EFAULT)
        } else if let Some(at) = (offset as u64)
            .checked_add(total as u64)
            .filter(|at| *at <= i64::MAX as u64)
        {
            if device {
                if writing {
                    if entry.kind == FdKind::Full {
                        Err(kinakaze_vfs::ENOSPC)
                    } else {
                        Ok(wanted)
                    }
                } else if entry.kind == FdKind::Null {
                    Ok(0)
                } else if entry.kind == FdKind::Random {
                    let count = unsafe {
                        crate::sysadmin::kinakaze_abi_getrandom(item.iov_base, wanted, 0)
                    };
                    if count < 0 {
                        Err(kinakaze_tls::errno())
                    } else {
                        Ok(count as usize)
                    }
                } else {
                    unsafe { item.iov_base.cast::<u8>().write_bytes(0, wanted) };
                    Ok(wanted)
                }
            } else if let Some(native) = &native {
                if writing {
                    unsafe { native.write_once(at, item.iov_base.cast(), wanted) }
                } else {
                    unsafe { native.read_once(at, item.iov_base.cast(), wanted) }
                }
            } else if writing {
                kinakaze_vfs::tmpfs::write(
                    fd,
                    unsafe { core::slice::from_raw_parts(item.iov_base.cast(), wanted) },
                    Some(at),
                )
            } else {
                unsafe { read_at(fd, item.iov_base.cast(), wanted, at) }
            }
        } else {
            Err(EINVAL)
        };
        match outcome {
            Ok(bytes) => {
                total += bytes;
                if bytes < wanted || total == limit {
                    break;
                }
            }
            Err(_) if total != 0 => return Ok(total),
            Err(error) => return Err(error),
        }
    }
    Ok(total)
}

unsafe fn transfer(
    fd: c_int,
    iov: *const IoVec,
    count: c_int,
    offset: i64,
    writing: bool,
) -> isize {
    loop {
        let outcome = unsafe { attempt(fd, iov, count, offset, writing) };
        // attempt's native pins/reopened handles have been dropped before a
        // signal handler is allowed to fork, exec, close or reuse descriptors.
        if outcome == Err(kinakaze_vfs::EINTR)
            && kinakaze_vfs::signal::deliver_pending() == kinakaze_vfs::signal::Delivery::Restart
        {
            continue;
        }
        return result(outcome);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pwrite64(
    fd: c_int,
    buffer: *const c_void,
    length: usize,
    offset: i64,
) -> isize {
    let iov = IoVec {
        iov_base: buffer.cast_mut(),
        iov_len: length,
    };
    unsafe { transfer(fd, &iov, 1, offset, true) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_preadv64(
    fd: c_int,
    iov: *const IoVec,
    count: c_int,
    offset: i64,
) -> isize {
    unsafe { transfer(fd, iov, count, offset, false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pwritev64(
    fd: c_int,
    iov: *const IoVec,
    count: c_int,
    offset: i64,
) -> isize {
    unsafe { transfer(fd, iov, count, offset, true) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_preadv64v2(
    fd: c_int,
    iov: *const IoVec,
    count: c_int,
    offset: i64,
    flags: c_int,
) -> isize {
    if flags != 0 {
        return result(Err(EOPNOTSUPP));
    }
    if offset == -1 {
        return unsafe { stream(fd, iov, count, false) };
    }
    unsafe { transfer(fd, iov, count, offset, false) }
}
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pwritev64v2(
    fd: c_int,
    iov: *const IoVec,
    count: c_int,
    offset: i64,
    flags: c_int,
) -> isize {
    if flags != 0 {
        return result(Err(EOPNOTSUPP));
    }
    if offset == -1 {
        return unsafe { stream(fd, iov, count, true) };
    }
    unsafe { transfer(fd, iov, count, offset, true) }
}
