//! Linux signalfd records consume the reading process/thread's pending signals.
//! Only the selection mask belongs to the shared open file description.
use crate::{
    mount::shared::{self, Store},
    *,
};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

const MAGIC: &[u8; 8] = b"KSIGFD01";
fn store(fd: i32) -> Result<Store, i32> {
    if get(fd)?.kind != FdKind::SignalFd {
        return Err(EINVAL);
    }
    shared::object_fd(fd)
}
fn mask(fd: i32) -> Result<u64, i32> {
    let data = store(fd)?.read()?.1;
    if data.len() != 16 || &data[..8] != MAGIC {
        return Err(EIO);
    }
    Ok(u64::from_le_bytes(data[8..].try_into().unwrap()))
}
pub fn create(fd: i32, signals: u64, flags: i32) -> Result<i32, i32> {
    if flags & !(0o4000 | 0o2000000) != 0 {
        return Err(EINVAL);
    }
    let signals = signals & !(1 << (signal::SIGKILL - 1) | 1 << (signal::SIGSTOP - 1));
    let object = if fd == -1 {
        shared::new_object()?
    } else {
        store(fd)?
    };
    object.update(|_| {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&signals.to_le_bytes());
        Ok((bytes, ()))
    })?;
    if fd != -1 {
        return Ok(fd);
    }
    object.descriptor_kind(FdKind::SignalFd, fs::special_fd_flags(flags))
}
pub fn poll(fd: i32) -> Result<bool, i32> {
    crate::job::drain_external();
    Ok(signal::pending() & mask(fd)? != 0)
}
fn record(signal: signal::PendingSignal, buffer: &mut [u8]) {
    buffer.fill(0);
    buffer[0..4].copy_from_slice(&signal.signal.to_le_bytes());
    buffer[8..12].copy_from_slice(&signal.code.to_le_bytes());
    let payload = signal.payload;
    if signal.code == -2 {
        // SI_TIMER
        buffer[24..28].copy_from_slice(&payload[4..8]);
        buffer[32..36].copy_from_slice(&payload[8..12]);
    } else {
        buffer[12..16].copy_from_slice(&payload[4..8]);
        buffer[16..20].copy_from_slice(&payload[8..12]);
    }
    if matches!(signal.code, -1 | -2 | -3) {
        buffer[44..48].copy_from_slice(&payload[12..16]);
        buffer[48..56].copy_from_slice(&payload[12..20]);
    } else if signal.signal == signal::SIGCHLD {
        buffer[40..44].copy_from_slice(&payload[12..16]);
        buffer[56..64].copy_from_slice(&payload[20..28]);
        buffer[64..72].copy_from_slice(&payload[28..36]);
    }
}
pub fn read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    if buffer.len() < 128 {
        return Err(EINVAL);
    }
    let entry = get(fd)?;
    let interrupt = interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    signal::register_waiter();
    struct Waiter;
    impl Drop for Waiter {
        fn drop(&mut self) {
            signal::unregister_waiter();
        }
    }
    let _waiter = Waiter;
    loop {
        let selected = mask(fd)?;
        let mut count = 0;
        for slot in buffer.chunks_exact_mut(128) {
            let Some(pending) = signal::take_pending(selected) else {
                break;
            };
            record(pending, slot);
            count += 128;
        }
        if count != 0 {
            crate::epoll::readiness_consumed(
                entry.description_id,
                crate::epoll::EPOLLIN | crate::epoll::EPOLLRDNORM,
            );
            return Ok(count);
        }
        if get(fd)?.flags.contains(FdFlags::NONBLOCK) {
            return Err(EAGAIN);
        }
        if matches!(signal::deliver_pending(), signal::Delivery::Interrupted) {
            return Err(EINTR);
        }
        unsafe {
            WaitForSingleObject(interrupt, 10);
        }
    }
}
