//! Linux x86-64 message-queue ABI with fault-contained guest memory access.
use super::mount_api::{read_user, text, write_user};
use kinakaze_vfs::{EINVAL, fs, mqueue};
fn attr(address: usize) -> Result<mqueue::Attr, i32> {
    let mut b = [0; 64];
    read_user(address, &mut b)?;
    let word = |i| i64::from_le_bytes(b[i..i + 8].try_into().unwrap());
    Ok(mqueue::Attr {
        flags: word(0),
        maxmsg: word(8),
        msgsize: word(16),
        curmsgs: word(24),
    })
}
fn output(address: usize, a: mqueue::Attr) -> Result<(), i32> {
    let mut b = [0; 64];
    for (i, v) in [a.flags, a.maxmsg, a.msgsize, a.curmsgs]
        .into_iter()
        .enumerate()
    {
        b[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    write_user(address, &b)
}
fn deadline(address: usize) -> Result<Option<[i64; 2]>, i32> {
    if address == 0 {
        return Ok(None);
    }
    let mut b = [0; 16];
    read_user(address, &mut b)?;
    Ok(Some([
        i64::from_le_bytes(b[..8].try_into().unwrap()),
        i64::from_le_bytes(b[8..].try_into().unwrap()),
    ]))
}
pub(super) fn syscall(number: u64, a: u64, b: u64, c: u64, d: u64, e: u64) -> Result<i64, i32> {
    match number {
        240 => {
            let name = text(a as usize, 257)?;
            let attributes = if b as i32 & fs::O_CREAT != 0 && d != 0 {
                Some(attr(d as usize)?)
            } else {
                None
            };
            mqueue::open(&name, b as i32, c as u32, attributes).map(i64::from)
        }
        241 => {
            mqueue::unlink(&text(a as usize, 257)?)?;
            Ok(0)
        }
        242 => {
            let timeout = deadline(e as usize)?;
            let attributes = mqueue::getattr(a as i32, None)?;
            if c > attributes.msgsize as u64 {
                return Err(kinakaze_vfs::EMSGSIZE);
            }
            let mut data = vec![0; c as usize];
            read_user(b as usize, &mut data)?;
            mqueue::send(a as i32, &data, d as u32, timeout)?;
            Ok(0)
        }
        243 => {
            let timeout = deadline(e as usize)?;
            let (data, priority) = mqueue::receive(a as i32, c as usize, timeout)?;
            write_user(b as usize, &data)?;
            if d != 0 {
                write_user(d as usize, &priority.to_le_bytes())?;
            }
            Ok(data.len() as i64)
        }
        244 => {
            let notification = if b == 0 {
                None
            } else {
                let mut data = [0; 64];
                read_user(b as usize, &mut data)?;
                Some(mqueue::Notification {
                    pid: kinakaze_vfs::job::process_id(),
                    host: std::process::id(),
                    value: u64::from_le_bytes(data[..8].try_into().unwrap()),
                    signal: u32::from_le_bytes(data[8..12].try_into().unwrap()),
                    method: u32::from_le_bytes(data[12..16].try_into().unwrap()),
                    sender: 0,
                    uid: 0,
                })
            };
            mqueue::notify(a as i32, notification)?;
            Ok(0)
        }
        245 => {
            let flags = if b == 0 {
                None
            } else {
                Some(attr(b as usize)?.flags)
            };
            let old = mqueue::getattr(a as i32, flags)?;
            if c != 0 {
                output(c as usize, old)?;
            }
            Ok(0)
        }
        _ => Err(EINVAL),
    }
}
fn posix(result: Result<i64, i32>) -> i64 {
    match result {
        Ok(n) => n,
        Err(e) => {
            crate::set_errno(e);
            -1
        }
    }
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_open(name: usize, flags: i32, mode: u32, attributes: usize) -> i32 {
    posix((|| {
        let name = text(name, 258)?;
        let name = name.strip_prefix('/').ok_or(EINVAL)?;
        let a = if flags & fs::O_CREAT != 0 && attributes != 0 {
            Some(attr(attributes)?)
        } else {
            None
        };
        mqueue::open(name, flags, mode, a).map(i64::from)
    })()) as i32
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_unlink(name: usize) -> i32 {
    posix((|| {
        let name = text(name, 258)?;
        mqueue::unlink(name.strip_prefix('/').ok_or(EINVAL)?)?;
        Ok(0)
    })()) as i32
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_close(fd: i32) -> i32 {
    posix((|| {
        if kinakaze_vfs::get(fd)?.kind != kinakaze_vfs::FdKind::MessageQueue {
            return Err(kinakaze_vfs::EBADF);
        }
        kinakaze_vfs::close(fd)?;
        Ok(0)
    })()) as i32
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_timedsend(
    fd: i32,
    data: usize,
    length: usize,
    priority: u32,
    time: usize,
) -> i32 {
    posix(syscall(
        242,
        fd as u64,
        data as u64,
        length as u64,
        priority as u64,
        time as u64,
    )) as i32
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_timedreceive(
    fd: i32,
    data: usize,
    length: usize,
    priority: usize,
    time: usize,
) -> isize {
    posix(syscall(
        243,
        fd as u64,
        data as u64,
        length as u64,
        priority as u64,
        time as u64,
    )) as isize
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_send(fd: i32, data: usize, length: usize, priority: u32) -> i32 {
    mq_timedsend(fd, data, length, priority, 0)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_receive(fd: i32, data: usize, length: usize, priority: usize) -> isize {
    mq_timedreceive(fd, data, length, priority, 0)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_notify(fd: i32, notification: usize) -> i32 {
    posix(syscall(244, fd as u64, notification as u64, 0, 0, 0)) as i32
}
pub(crate) extern "sysv64" fn mq_getattr(fd: i32, attributes: usize) -> i32 {
    if attributes == 0 {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    mq_setattr(fd, 0, attributes)
}
#[unsafe(no_mangle)]
pub extern "sysv64" fn mq_setattr(fd: i32, attributes: usize, old: usize) -> i32 {
    posix(syscall(245, fd as u64, attributes as u64, old as u64, 0, 0)) as i32
}
