use super::{FIOASYNC, FIONBIO, kinakaze_abi_ioctl};
use crate::fdio::{kinakaze_abi_dup, kinakaze_abi_fcntl64};
use kinakaze_vfs::{self as vfs, FdFlags, fs, socket};

struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        let _ = vfs::close(self.0);
    }
}

fn flags(fd: i32) -> i32 {
    unsafe { kinakaze_abi_fcntl64(fd, 3, 0) }
}

fn nonblocking(fd: i32, mut value: i32) {
    assert_eq!(
        unsafe { kinakaze_abi_ioctl(fd, FIONBIO, (&raw mut value).cast()) },
        0
    );
}

#[test]
fn unix_nonblocking_drains_to_eagain_and_preserves_async_and_append() {
    let (first, second) = vfs::unix::socketpair(socket::SOCK_STREAM).unwrap();
    let first = Fd(first);
    let second = Fd(second);
    let alias = Fd(kinakaze_abi_dup(first.0));
    assert!(alias.0 >= 0);
    vfs::set_status_flags(first.0, true, false).unwrap();
    let mut on = 1i32;
    assert_eq!(
        unsafe { kinakaze_abi_ioctl(first.0, FIOASYNC, (&raw mut on).cast()) },
        0
    );
    nonblocking(alias.0, -1); // Any nonzero int enables O_NONBLOCK.
    let wanted = fs::O_NONBLOCK | fs::O_APPEND | 0o20000;
    assert_eq!(flags(first.0) & wanted, wanted);
    assert_eq!(flags(alias.0) & wanted, wanted);
    let mut byte = [0u8];
    assert_eq!(vfs::read(first.0, &mut byte), Err(vfs::EAGAIN));
    assert_eq!(vfs::write(second.0, b"x"), Ok(1));
    assert_eq!(vfs::read(alias.0, &mut byte), Ok(1));
    assert_eq!(byte, [b'x']);
    assert_eq!(vfs::read(alias.0, &mut byte), Err(vfs::EAGAIN));
    nonblocking(first.0, 0);
    assert_eq!(flags(alias.0) & wanted, fs::O_APPEND | 0o20000);
}

#[test]
fn empty_pipe_nonblocking_is_shared_by_dup_and_can_be_disabled() {
    let (reader, writer) = vfs::create_pipe(FdFlags::NONE, 4096).unwrap();
    let reader = Fd(reader);
    let _writer = Fd(writer);
    let alias = Fd(kinakaze_abi_dup(reader.0));
    assert!(alias.0 >= 0);
    nonblocking(alias.0, 1);
    assert_ne!(flags(reader.0) & fs::O_NONBLOCK, 0);
    assert_eq!(vfs::read(reader.0, &mut [0u8]), Err(vfs::EAGAIN));
    nonblocking(reader.0, 0);
    assert_eq!(flags(alias.0) & fs::O_NONBLOCK, 0);
}

#[test]
fn native_socket_uses_the_same_nonblocking_status_as_fcntl() {
    let fd = Fd(socket::socket(socket::AF_INET, socket::SOCK_STREAM, 0).unwrap());
    nonblocking(fd.0, 1);
    assert_ne!(flags(fd.0) & fs::O_NONBLOCK, 0);
    nonblocking(fd.0, 0);
    assert_eq!(flags(fd.0) & fs::O_NONBLOCK, 0);
}

#[test]
fn invalid_fd_and_null_argument_report_errors() {
    assert_eq!(
        unsafe { kinakaze_abi_ioctl(-1, FIONBIO, core::ptr::null_mut()) },
        -1
    );
    assert_eq!(kinakaze_tls::errno(), vfs::EBADF);
    let (reader, writer) = vfs::create_pipe(FdFlags::NONE, 4096).unwrap();
    let reader = Fd(reader);
    let _writer = Fd(writer);
    assert_eq!(
        unsafe { kinakaze_abi_ioctl(reader.0, FIONBIO, core::ptr::null_mut()) },
        -1
    );
    assert_eq!(kinakaze_tls::errno(), vfs::EFAULT);
    assert_eq!(flags(reader.0) & fs::O_NONBLOCK, 0);
}
