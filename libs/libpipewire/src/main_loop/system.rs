//! SPA system callbacks keep Linux fd ownership and negative errno results.
use super::*;
type Data = *mut c_void;
type Time = timerfd::Timespec;
type Timer = timerfd::Itimerspec;
#[repr(C)]
pub(super) struct PollEvent {
    events: u32,
    data: *mut c_void,
}
#[repr(C)]
pub(super) struct Methods {
    version: u32,
    read: unsafe extern "sysv64" fn(Data, i32, Data, usize) -> isize,
    write: unsafe extern "sysv64" fn(Data, i32, *const c_void, usize) -> isize,
    ioctl: unsafe extern "sysv64" fn(Data, i32, u64, Data) -> i32,
    close: unsafe extern "sysv64" fn(Data, i32) -> i32,
    clock_gettime: unsafe extern "sysv64" fn(Data, i32, *mut Time) -> i32,
    clock_getres: unsafe extern "sysv64" fn(Data, i32, *mut Time) -> i32,
    poll_create: unsafe extern "sysv64" fn(Data, i32) -> i32,
    poll_add: unsafe extern "sysv64" fn(Data, i32, i32, u32, Data) -> i32,
    poll_mod: unsafe extern "sysv64" fn(Data, i32, i32, u32, Data) -> i32,
    poll_del: unsafe extern "sysv64" fn(Data, i32, i32) -> i32,
    poll_wait: unsafe extern "sysv64" fn(Data, i32, *mut PollEvent, i32, i32) -> i32,
    timer_create: unsafe extern "sysv64" fn(Data, i32, i32) -> i32,
    timer_set: unsafe extern "sysv64" fn(Data, i32, i32, *const Timer, *mut Timer) -> i32,
    timer_get: unsafe extern "sysv64" fn(Data, i32, *mut Timer) -> i32,
    timer_read: unsafe extern "sysv64" fn(Data, i32, *mut u64) -> i32,
    event_create: unsafe extern "sysv64" fn(Data, i32) -> i32,
    event_write: unsafe extern "sysv64" fn(Data, i32, u64) -> i32,
    event_read: unsafe extern "sysv64" fn(Data, i32, *mut u64) -> i32,
    signal_create: unsafe extern "sysv64" fn(Data, i32, i32) -> i32,
    signal_read: unsafe extern "sysv64" fn(Data, i32, *mut i32) -> i32,
}
pub(super) static SYSTEM_METHODS: Methods = Methods {
    version: 0,
    read,
    write,
    ioctl,
    close,
    clock_gettime,
    clock_getres,
    poll_create,
    poll_add,
    poll_mod,
    poll_del,
    poll_wait,
    timer_create,
    timer_set,
    timer_get,
    timer_read,
    event_create,
    event_write,
    event_read,
    signal_create,
    signal_read,
};
fn fd_flags(flags: i32) -> i32 {
    (if flags & 1 != 0 { 0x80000 } else { 0 }) | (if flags & 2 != 0 { 0x800 } else { 0 })
}
fn errno(result: i32) -> i32 {
    if result < 0 {
        -kinakaze_tls::errno()
    } else {
        result
    }
}
unsafe extern "sysv64" fn read(_: Data, fd: i32, buf: Data, size: usize) -> isize {
    let result = unsafe { libc::kinakaze_read(fd, buf, size) };
    if result < 0 {
        -kinakaze_tls::errno() as isize
    } else {
        result
    }
}
unsafe extern "sysv64" fn write(_: Data, fd: i32, buf: *const c_void, size: usize) -> isize {
    let result = unsafe { libc::kinakaze_write(fd, buf, size) };
    if result < 0 {
        -kinakaze_tls::errno() as isize
    } else {
        result
    }
}
unsafe extern "sysv64" fn ioctl(_: Data, fd: i32, request: u64, arg: Data) -> i32 {
    errno(unsafe { libc::term::kinakaze_abi_ioctl(fd, request, arg) })
}
unsafe extern "sysv64" fn close(_: Data, fd: i32) -> i32 {
    kinakaze_vfs::close(fd).map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn clock_gettime(_: Data, clock: i32, value: *mut Time) -> i32 {
    errno(unsafe { libc::time::kinakaze_abi_clock_gettime(clock, value.cast()) })
}
unsafe extern "sysv64" fn clock_getres(_: Data, clock: i32, value: *mut Time) -> i32 {
    errno(unsafe { libc::time::kinakaze_abi_clock_getres(clock, value.cast()) })
}
unsafe extern "sysv64" fn poll_create(_: Data, flags: i32) -> i32 {
    if flags & !1 != 0 {
        return -22;
    }
    poll::epoll_create1(fd_flags(flags)).unwrap_or_else(|e| -e)
}
unsafe extern "sysv64" fn poll_add(_: Data, pfd: i32, fd: i32, events: u32, user: Data) -> i32 {
    poll::epoll_ctl(
        pfd,
        1,
        fd,
        Some(poll::EpollEvent {
            events,
            data: user as u64,
        }),
    )
    .map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn poll_mod(_: Data, pfd: i32, fd: i32, events: u32, user: Data) -> i32 {
    poll::epoll_ctl(
        pfd,
        3,
        fd,
        Some(poll::EpollEvent {
            events,
            data: user as u64,
        }),
    )
    .map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn poll_del(_: Data, pfd: i32, fd: i32) -> i32 {
    poll::epoll_ctl(pfd, 2, fd, None).map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn poll_wait(
    _: Data,
    pfd: i32,
    output: *mut PollEvent,
    count: i32,
    timeout: i32,
) -> i32 {
    if output.is_null() || count <= 0 {
        return -22;
    }
    let mut events = [poll::EpollEvent::default(); 64];
    let cap = (count as usize).min(64);
    match poll::epoll_wait(pfd, &mut events[..cap], timeout) {
        Err(e) => -e,
        Ok(n) => {
            for (i, event) in events[..n].iter().enumerate() {
                unsafe {
                    output.add(i).write(PollEvent {
                        events: event.events,
                        data: event.data as usize as _,
                    })
                };
            }
            n as i32
        }
    }
}
unsafe extern "sysv64" fn timer_create(_: Data, clock: i32, flags: i32) -> i32 {
    if flags & !3 != 0 {
        return -22;
    }
    timerfd::create(clock, fd_flags(flags)).unwrap_or_else(|e| -e)
}
unsafe extern "sysv64" fn timer_set(
    _: Data,
    fd: i32,
    flags: i32,
    value: *const Timer,
    old: *mut Timer,
) -> i32 {
    if value.is_null() || flags & !24 != 0 {
        return -22;
    }
    let native = i32::from(flags & 8 != 0) | (i32::from(flags & 16 != 0) << 1);
    match timerfd::settime(fd, native, unsafe { *value }) {
        Err(e) => -e,
        Ok(value) => {
            if !old.is_null() {
                unsafe {
                    *old = value;
                }
            }
            0
        }
    }
}
unsafe extern "sysv64" fn timer_get(_: Data, fd: i32, value: *mut Timer) -> i32 {
    if value.is_null() {
        return -22;
    }
    match timerfd::gettime(fd) {
        Err(e) => -e,
        Ok(timer) => {
            unsafe {
                *value = timer;
            }
            0
        }
    }
}
unsafe extern "sysv64" fn timer_read(_: Data, fd: i32, value: *mut u64) -> i32 {
    if value.is_null() {
        return -22;
    }
    let mut bytes = [0; 8];
    match timerfd::read(fd, &mut bytes) {
        Err(e) => -e,
        Ok(_) => {
            unsafe {
                *value = u64::from_ne_bytes(bytes);
            }
            0
        }
    }
}
unsafe extern "sysv64" fn event_create(_: Data, flags: i32) -> i32 {
    if flags & !7 != 0 {
        return -22;
    }
    eventfd::create_eventfd(0, fd_flags(flags) | i32::from(flags & 4 != 0)).unwrap_or_else(|e| -e)
}
unsafe extern "sysv64" fn event_write(_: Data, fd: i32, count: u64) -> i32 {
    eventfd::write_eventfd(fd, &count.to_ne_bytes(), false).map_or_else(|e| -e, |_| 0)
}
unsafe extern "sysv64" fn event_read(_: Data, fd: i32, value: *mut u64) -> i32 {
    if value.is_null() {
        return -22;
    }
    let mut bytes = [0; 8];
    match eventfd::read_eventfd(fd, &mut bytes, false) {
        Err(e) => -e,
        Ok(_) => {
            unsafe {
                *value = u64::from_ne_bytes(bytes);
            }
            0
        }
    }
}
// The current libc signalfd returns an eventfd rather than signal records.
// Do not expose that as a signal source or leave it continuously readable.
pub(super) unsafe extern "sysv64" fn signal_create(_: Data, _signal: i32, _flags: i32) -> i32 {
    -95
}
unsafe extern "sysv64" fn signal_read(_: Data, _fd: i32, _signal: *mut i32) -> i32 {
    -95
}
