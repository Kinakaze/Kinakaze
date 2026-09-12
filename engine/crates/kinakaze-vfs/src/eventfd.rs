//! Linux eventfd counters, shared through native sections on Windows.

#[cfg(windows)]
mod readiness;
#[cfg(windows)]
mod shared;
#[cfg(windows)]
pub(crate) use readiness::ReadyEvents;
#[cfg(windows)]
pub use shared::*;

pub const EFD_SEMAPHORE: i32 = 1;
pub const EFD_CLOEXEC: i32 = 0o2000000;
pub const EFD_NONBLOCK: i32 = 0o0004000;

#[cfg(not(windows))]
pub use local::*;
#[cfg(not(windows))]
mod local {
    use super::{EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE};

    use std::collections::HashMap;
    use std::sync::{Arc, Condvar, Mutex, OnceLock};

    struct EventFdState {
        counter: u64,
        flags: i32,
    }

    struct EventFdItem {
        mutex: Mutex<EventFdState>,
        cvar: Condvar,
    }

    static EVENTFDS: OnceLock<Mutex<HashMap<i32, Arc<EventFdItem>>>> = OnceLock::new();

    fn eventfds() -> &'static Mutex<HashMap<i32, Arc<EventFdItem>>> {
        EVENTFDS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn trace(message: impl core::fmt::Display) {
        if crate::eventfd_trace_enabled() {
            eprintln!("kinakaze eventfd: {message}");
        }
    }

    pub fn create_eventfd(initval: u32, flags: i32) -> Result<i32, i32> {
        let mut fd_flags = crate::FdFlags::NONE;
        if (flags & EFD_CLOEXEC) != 0 {
            fd_flags.0 |= crate::FdFlags::CLOSE_ON_EXEC.0;
        }
        if (flags & EFD_NONBLOCK) != 0 {
            fd_flags.0 |= crate::FdFlags::NONBLOCK.0;
        }
        let fd = crate::install_handleless(crate::FdKind::EventFd, fd_flags)?;
        let item = Arc::new(EventFdItem {
            mutex: Mutex::new(EventFdState {
                counter: initval as u64,
                flags,
            }),
            cvar: Condvar::new(),
        });
        if let Ok(mut map) = eventfds().lock() {
            map.insert(fd, item);
        }
        trace(format_args!(
            "create fd={fd} initial={initval} flags={flags:#x}"
        ));
        Ok(fd)
    }

    pub fn read_eventfd(fd: i32, buffer: &mut [u8], nonblock: bool) -> Result<usize, i32> {
        if buffer.len() < 8 {
            return Err(crate::EINVAL);
        }
        let item = {
            let map = eventfds().lock().map_err(|_| crate::EIO)?;
            map.get(&fd).cloned().ok_or(crate::EBADF)?
        };
        let mut state = item.mutex.lock().map_err(|_| crate::EIO)?;
        loop {
            if state.counter > 0 {
                let val = if (state.flags & EFD_SEMAPHORE) != 0 {
                    state.counter -= 1;
                    1u64
                } else {
                    let v = state.counter;
                    state.counter = 0;
                    v
                };
                buffer[..8].copy_from_slice(&val.to_ne_bytes());
                item.cvar.notify_all();
                trace(format_args!(
                    "read fd={fd} value={val} remaining={}",
                    state.counter
                ));
                return Ok(8);
            }
            if nonblock || (state.flags & EFD_NONBLOCK) != 0 {
                trace(format_args!("read fd={fd} -> EAGAIN"));
                return Err(crate::EAGAIN);
            }
            state = item.cvar.wait(state).map_err(|_| crate::EIO)?;
        }
    }

    pub fn write_eventfd(fd: i32, buffer: &[u8], nonblock: bool) -> Result<usize, i32> {
        if buffer.len() < 8 {
            return Err(crate::EINVAL);
        }
        let val = u64::from_ne_bytes(buffer[..8].try_into().unwrap());
        if val == u64::MAX {
            return Err(crate::EINVAL);
        }
        let item = {
            let map = eventfds().lock().map_err(|_| crate::EIO)?;
            map.get(&fd).cloned().ok_or(crate::EBADF)?
        };
        let mut state = item.mutex.lock().map_err(|_| crate::EIO)?;
        loop {
            if u64::MAX - 1 - state.counter >= val {
                state.counter += val;
                item.cvar.notify_all();
                trace(format_args!(
                    "write fd={fd} value={val} counter={}",
                    state.counter
                ));
                return Ok(8);
            }
            if nonblock || (state.flags & EFD_NONBLOCK) != 0 {
                return Err(crate::EAGAIN);
            }
            state = item.cvar.wait(state).map_err(|_| crate::EIO)?;
        }
    }

    pub fn poll_eventfd(fd: i32) -> Result<(bool, bool), i32> {
        let item = {
            let map = eventfds().lock().map_err(|_| crate::EIO)?;
            map.get(&fd).cloned().ok_or(crate::EBADF)?
        };
        let state = item.mutex.lock().map_err(|_| crate::EIO)?;
        let readable = state.counter > 0;
        let writable = state.counter < u64::MAX - 1;
        Ok((readable, writable))
    }

    pub fn forget_eventfd(fd: i32) {
        if let Ok(mut map) = eventfds().lock() {
            map.remove(&fd);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::epoll::{
        EPOLL_CTL_ADD, EPOLLET, EPOLLIN, EpollEvent, epoll_create1, epoll_ctl, epoll_wait,
    };
    use std::sync::Arc;

    #[test]
    fn edge_triggered_epoll_tracks_eventfd_counter_transitions() {
        let fd = create_eventfd(0, EFD_NONBLOCK).expect("eventfd");
        let epoll_fd = epoll_create1(0).expect("epoll_create1");
        epoll_ctl(
            epoll_fd,
            EPOLL_CTL_ADD,
            fd,
            Some(EpollEvent {
                events: EPOLLIN | EPOLLET,
                data: 0xfeed,
            }),
        )
        .expect("epoll_ctl add");

        let mut events = [EpollEvent::default(); 2];
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        write_eventfd(fd, &1u64.to_ne_bytes(), true).expect("first write");
        assert_eq!(epoll_wait(epoll_fd, &mut events, 100).unwrap(), 1);
        let cookie = events[0].data;
        assert_eq!(cookie, 0xfeed);
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        let mut value = [0u8; 8];
        read_eventfd(fd, &mut value, true).expect("drain");
        assert_eq!(epoll_wait(epoll_fd, &mut events, 0).unwrap(), 0);

        write_eventfd(fd, &1u64.to_ne_bytes(), true).expect("second write");
        assert_eq!(epoll_wait(epoll_fd, &mut events, 100).unwrap(), 1);

        crate::close(epoll_fd).expect("close epoll");
        crate::close(fd).expect("close eventfd");
    }

    #[test]
    fn eventfd_semaphore_mode_increments_and_decrements() {
        let fd = create_eventfd(0, EFD_SEMAPHORE | EFD_NONBLOCK).expect("create_eventfd");

        // Write 3 to counter
        assert_eq!(write_eventfd(fd, &3u64.to_ne_bytes(), true).unwrap(), 8);

        // Consecutive reads in semaphore mode each return 1
        let mut val = [0u8; 8];
        assert_eq!(read_eventfd(fd, &mut val, true).unwrap(), 8);
        assert_eq!(u64::from_ne_bytes(val), 1);

        assert_eq!(read_eventfd(fd, &mut val, true).unwrap(), 8);
        assert_eq!(u64::from_ne_bytes(val), 1);

        assert_eq!(read_eventfd(fd, &mut val, true).unwrap(), 8);
        assert_eq!(u64::from_ne_bytes(val), 1);

        // Now empty -> returns EAGAIN in nonblocking mode
        assert_eq!(read_eventfd(fd, &mut val, true), Err(crate::EAGAIN));

        crate::close(fd).expect("close");
    }

    #[test]
    fn eventfd_normal_mode_drains_whole_counter() {
        let fd = create_eventfd(0, EFD_NONBLOCK).expect("create_eventfd");

        assert_eq!(write_eventfd(fd, &5u64.to_ne_bytes(), true).unwrap(), 8);
        assert_eq!(write_eventfd(fd, &7u64.to_ne_bytes(), true).unwrap(), 8);

        let mut val = [0u8; 8];
        assert_eq!(read_eventfd(fd, &mut val, true).unwrap(), 8);
        assert_eq!(u64::from_ne_bytes(val), 12);

        // Reset to 0 -> subsequent read reports EAGAIN
        assert_eq!(read_eventfd(fd, &mut val, true), Err(crate::EAGAIN));

        crate::close(fd).expect("close");
    }

    #[test]
    fn eventfd_write_u64_max_returns_einval() {
        let fd = create_eventfd(0, EFD_NONBLOCK).expect("create_eventfd");

        assert_eq!(
            write_eventfd(fd, &u64::MAX.to_ne_bytes(), true),
            Err(crate::EINVAL)
        );

        crate::close(fd).expect("close");
    }

    #[test]
    fn eventfd_short_buffer_length_contract() {
        let fd = create_eventfd(0, EFD_NONBLOCK).expect("create_eventfd");

        let mut short_buf = [0u8; 7];
        assert_eq!(read_eventfd(fd, &mut short_buf, true), Err(crate::EINVAL));
        assert_eq!(write_eventfd(fd, &short_buf, true), Err(crate::EINVAL));

        crate::close(fd).expect("close");
    }

    #[test]
    fn concurrent_slot_reuse_does_not_remove_a_new_counter() {
        let ready = Arc::new(std::sync::Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|worker| {
                let ready = ready.clone();
                std::thread::spawn(move || {
                    ready.wait();
                    for iteration in 0..300 {
                        let value = 1 + worker * 300 + iteration;
                        let fd = create_eventfd(value, EFD_NONBLOCK).unwrap();
                        std::thread::yield_now();
                        let mut buffer = [0u8; 8];
                        let result = read_eventfd(fd, &mut buffer, true);
                        let closed = crate::close(fd);
                        assert_eq!(result, Ok(8));
                        assert_eq!(u64::from_ne_bytes(buffer), value as u64);
                        assert_eq!(closed, Ok(()));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
    }
}
