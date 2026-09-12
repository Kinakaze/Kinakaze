//! Shared listener events and per-wait non-consuming NPFS stream notifications.
//! The kernel wakes on queued connections, data or last-writer close, including an
//! independently hosted peer crashing. Message-mode data reads are excluded:
//! a zero-byte read can consume an empty record.
use super::*;
use crate::fs::object::Object;
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::Threading::SetEvent;

pub(crate) struct ReadWait {
    operation: Box<OVERLAPPED>,
    event: Object,
    pipe: Object,
    pending: bool,
    _listener: Option<Arc<listener::Lease>>,
}
impl ReadWait {
    pub(crate) fn raw(&self) -> HANDLE {
        self.event.raw()
    }

    /// Wait without delivering guest handlers while the caller owns inode pins.
    /// Drop retires a timed-out/interrupted request before outer signal dispatch.
    pub(super) fn wait_interruptible(&mut self, timeout: u32) -> Result<(), i32> {
        let interrupt = interrupt::current();
        if interrupt.is_null() {
            return Err(EIO);
        }
        signal::register_waiter();
        let handles = [self.raw(), interrupt];
        let ready = if signal::pending() & !signal::blocked_mask() != 0 {
            WAIT_OBJECT_0 + 1
        } else {
            unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout) }
        };
        signal::unregister_waiter();
        match ready {
            WAIT_OBJECT_0 => {
                self.pending = false;
                Ok(())
            }
            windows_sys::Win32::Foundation::WAIT_TIMEOUT => Ok(()),
            value if value == WAIT_OBJECT_0 + 1 => {
                if signal::pending() & !signal::blocked_mask() != 0 {
                    Err(EINTR)
                } else {
                    Ok(()) // a stale/blocked interrupt; the caller rechecks
                }
            }
            _ => Err(EIO),
        }
    }
}
impl Drop for ReadWait {
    fn drop(&mut self) {
        if self.pending {
            // Retire exactly our request before freeing its stable OVERLAPPED.
            // Other readers, including inherited descriptors, remain untouched.
            unsafe {
                CancelIoEx(self.pipe.raw(), &raw const *self.operation);
                let mut transferred = 0;
                GetOverlappedResult(
                    self.pipe.raw(),
                    &raw const *self.operation,
                    &mut transferred,
                    1,
                );
            }
        }
    }
}

pub(crate) fn prepare(fd: i32) -> Result<Option<ReadWait>, i32> {
    let (entry, socket, pin) = ancillary::pin_socket(fd)?;
    if !(socket.state == State::Listening
        || (socket.state == State::Connected && !socket.message_mode()))
        || !entry.flags.contains(FdFlags::OVERLAPPED)
    {
        return Ok(None);
    }
    let pipe = pin.ok_or(EIO)?;
    if socket.state == State::Listening {
        let listener = socket.listener.ok_or(EIO)?;
        return Ok(Some(ReadWait {
            operation: Box::new(unsafe { std::mem::zeroed() }),
            event: listener.pool.notification()?,
            pipe,
            pending: false,
            _listener: Some(listener),
        }));
    }
    prepare_stream(pipe).map(Some)
}

/// The owned pipe is a byte stream; message-mode zero-byte reads consume records.
pub(super) fn prepare_stream(pipe: Object) -> Result<ReadWait, i32> {
    let event = Object::owned(unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) })?;
    let mut operation: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
    operation.hEvent = event.raw();
    let mut result = ReadWait {
        operation,
        event,
        pipe,
        pending: false,
        _listener: None,
    };
    let mut transferred = 0;
    let ok = unsafe {
        ReadFile(
            result.pipe.raw(),
            std::ptr::null_mut(),
            0,
            &mut transferred,
            &raw mut *result.operation,
        )
    };
    if ok == 0 {
        match unsafe { GetLastError() } {
            ERROR_IO_PENDING => result.pending = true,
            ERROR_PIPE_CONNECTED | ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA => unsafe {
                SetEvent(result.raw());
            },
            error => return Err(crate::errno_from_win32(error)),
        }
    } else {
        unsafe {
            SetEvent(result.raw());
        }
    }
    Ok(result)
}

pub(crate) fn prepare_write(fd: i32) -> Result<Option<Object>, i32> {
    ancillary::prepare_write_wait(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    #[test]
    fn empty_ancillary_peek_retires_a_previous_read_edge() {
        use crate::epoll::{self, EPOLL_CTL_ADD, EPOLLET, EPOLLIN, EpollEvent};
        let (reader, writer) = socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        let poller = epoll::epoll_create1(0).unwrap();
        epoll::epoll_ctl(
            poller,
            EPOLL_CTL_ADD,
            reader,
            Some(EpollEvent {
                events: EPOLLIN | EPOLLET,
                data: 92,
            }),
        )
        .unwrap();
        crate::write(writer, b"a").unwrap();
        let mut events = [EpollEvent::default(); 1];
        assert_eq!(epoll::epoll_wait(poller, &mut events, 0).unwrap(), 1);
        // Model an independently hosted reader draining the shared pipe; its
        // local epoll table cannot update this process's readiness history.
        let mut byte = [0];
        let entry = crate::get(reader).unwrap();
        assert_eq!(
            unsafe { crate::platform_read(entry, byte.as_mut_ptr(), 1) }.unwrap(),
            1
        );
        assert_eq!(crate::read(reader, &mut byte), Err(EAGAIN));
        crate::write(writer, b"b").unwrap();
        assert_eq!(epoll::epoll_wait(poller, &mut events, 0).unwrap(), 1);
        assert_eq!(crate::read(reader, &mut byte).unwrap(), 1);
        assert_eq!(byte, [b'b']);
        for fd in [poller, reader, writer] {
            crate::close(fd).unwrap();
        }
    }

    #[test]
    fn read_wait_wakes_without_consuming_and_observes_peer_close() {
        let (reader, writer) = socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        let wait = prepare(reader).unwrap().unwrap();
        assert_eq!(unsafe { WaitForSingleObject(wait.raw(), 0) }, WAIT_TIMEOUT);
        crate::write(writer, b"payload").unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(wait.raw(), 1000) },
            WAIT_OBJECT_0
        );
        drop(wait);
        let mut bytes = [0; 7];
        assert_eq!(crate::read(reader, &mut bytes).unwrap(), 7);
        assert_eq!(&bytes, b"payload");
        let wait = prepare(reader).unwrap().unwrap();
        crate::close(writer).unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(wait.raw(), 1000) },
            WAIT_OBJECT_0
        );
        drop(wait);
        assert_eq!(crate::read(reader, &mut bytes).unwrap(), 0);
        crate::close(reader).unwrap();
    }

    #[test]
    fn read_progress_wakes_all_blocked_writers_without_a_timer() {
        let (writer, reader) = socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        let bytes = [1; 8192];
        loop {
            match crate::write(writer, &bytes) {
                Ok(n) => assert!(n > 0),
                Err(EAGAIN) => break,
                Err(e) => panic!("fill: {e}"),
            }
        }
        let first = prepare_write(writer).unwrap().unwrap();
        let second = prepare_write(writer).unwrap().unwrap();
        assert_eq!(unsafe { WaitForSingleObject(first.raw(), 0) }, WAIT_TIMEOUT);
        assert_eq!(
            unsafe { WaitForSingleObject(second.raw(), 0) },
            WAIT_TIMEOUT
        );
        let mut buffer = [0; 8192];
        assert_eq!(crate::read(reader, &mut buffer).unwrap(), 8192);
        // A nonblocking native wait proves publication, rather than a timer
        // eventually discovering the new quota. Both subscribers are woken.
        assert_eq!(
            unsafe { WaitForSingleObject(first.raw(), 0) },
            WAIT_OBJECT_0
        );
        assert_eq!(
            unsafe { WaitForSingleObject(second.raw(), 0) },
            WAIT_OBJECT_0
        );
        drop((first, second));
        // Space released before enrollment must also be observed immediately.
        let late = prepare_write(writer).unwrap().unwrap();
        assert_eq!(unsafe { WaitForSingleObject(late.raw(), 0) }, WAIT_OBJECT_0);
        assert_eq!(crate::write(writer, b"x").unwrap(), 1);
        drop(late);
        crate::close(writer).unwrap();
        crate::close(reader).unwrap();
    }

    #[test]
    fn write_wait_enrolls_without_a_previous_eagain_and_can_be_reenrolled() {
        let (writer, reader) = socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        let raw = crate::get(writer).unwrap().raw as HANDLE;
        let bytes = [0x57; 8192];
        let mut buffer = [0; 8192];
        for _ in 0..16 {
            // Stop at exactly full; do not let send(EAGAIN) register the wait
            // for us. A poller may be the first observer of backpressure.
            loop {
                let quota = pipe_information(raw).unwrap().write_quota_available as usize;
                if quota == 0 {
                    break;
                }
                let amount = quota.min(bytes.len());
                assert_eq!(crate::write(writer, &bytes[..amount]).unwrap(), amount);
            }
            let first = prepare_write(writer).unwrap().unwrap();
            let cancelled = prepare_write(writer).unwrap().unwrap();
            let second = prepare_write(writer).unwrap().unwrap();
            drop(cancelled);
            assert_eq!(unsafe { WaitForSingleObject(first.raw(), 0) }, WAIT_TIMEOUT);
            assert_eq!(crate::read(reader, &mut buffer).unwrap(), buffer.len());
            assert!(buffer.iter().all(|byte| *byte == 0x57));
            assert_eq!(
                unsafe { WaitForSingleObject(first.raw(), 0) },
                WAIT_OBJECT_0
            );
            assert_eq!(
                unsafe { WaitForSingleObject(second.raw(), 0) },
                WAIT_OBJECT_0
            );
        }
        crate::close(writer).unwrap();
        crate::close(reader).unwrap();
    }

    #[test]
    fn cancellation_preserves_other_waiters_and_fd_lifetime() {
        let (reader, writer) = socketpair(SOCK_STREAM | SOCK_NONBLOCK).unwrap();
        let first = prepare(reader).unwrap().unwrap();
        let second = prepare(reader).unwrap().unwrap();
        drop(first);
        assert_eq!(
            unsafe { WaitForSingleObject(second.raw(), 0) },
            WAIT_TIMEOUT
        );
        // The native request retains the description even if the numeric fd
        // closes. Cleanup must never cancel another operation on a reused fd.
        crate::close(reader).unwrap();
        crate::write(writer, b"x").unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(second.raw(), 1000) },
            WAIT_OBJECT_0
        );
        drop(second);
        crate::close(writer).unwrap();
    }

    #[test]
    fn timed_read_wait_retires_only_its_request_and_keeps_the_payload() {
        let (reader, writer) = socketpair(SOCK_STREAM).unwrap();
        let other = prepare(reader).unwrap().unwrap();
        let mut timed = prepare(reader).unwrap().unwrap();
        timed.wait_interruptible(0).unwrap();
        drop(timed);
        assert_eq!(unsafe { WaitForSingleObject(other.raw(), 0) }, WAIT_TIMEOUT);
        crate::write(writer, b"retained").unwrap();
        let mut ready = prepare(reader).unwrap().unwrap();
        ready.wait_interruptible(1000).unwrap();
        drop((ready, other));
        let mut bytes = [0; 8];
        assert_eq!(crate::read(reader, &mut bytes), Ok(8));
        assert_eq!(&bytes, b"retained");
        crate::close(reader).unwrap();
        crate::close(writer).unwrap();
    }

    #[test]
    fn message_sockets_keep_empty_records() {
        let (reader, writer) = socketpair(SOCK_SEQPACKET | SOCK_NONBLOCK).unwrap();
        assert!(prepare(reader).unwrap().is_none());
        unsafe {
            send(writer, b"".as_ptr(), 0, 0).unwrap();
        }
        assert!(prepare(reader).unwrap().is_none());
        let mut bytes = [0; 1];
        assert_eq!(
            unsafe { recv(reader, bytes.as_mut_ptr(), 1, 0) }.unwrap(),
            0
        );
        assert_eq!(
            unsafe { recv(reader, bytes.as_mut_ptr(), 1, 0) },
            Err(EAGAIN)
        );
        crate::close(reader).unwrap();
        crate::close(writer).unwrap();
    }
}
