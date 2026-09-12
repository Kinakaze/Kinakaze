//! Shared timerfd open descriptions. Deadlines use host clocks so another time
//! namespace can read or rearm an inherited timer without changing its epoch.
use crate::{EAGAIN, EBADF, EFAULT, EINTR, EINVAL, EIO, FdFlags, FdKind, errno_from_win32};
use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
};
const MAGIC: u64 = u64::from_le_bytes(*b"CYTMR001");
#[repr(C)]
struct State {
    magic: u64,
    id: u64,
    clock: i32,
    _pad: u32,
    deadline: i64,
    interval: i64,
    ticks: u64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Timespec {
    pub sec: i64,
    pub nsec: i64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Itimerspec {
    pub interval: Timespec,
    pub value: Timespec,
}
impl Timespec {
    fn nanos(self) -> Result<i64, i32> {
        if self.sec < 0 || !(0..1_000_000_000).contains(&self.nsec) {
            return Err(EINVAL);
        }
        self.sec
            .checked_mul(1_000_000_000)
            .and_then(|s| s.checked_add(self.nsec))
            .ok_or(EINVAL)
    }
    fn from_nanos(n: i64) -> Self {
        Self {
            sec: n / 1_000_000_000,
            nsec: n % 1_000_000_000,
        }
    }
}
struct Timer {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    mutex: HANDLE,
}
unsafe impl Send for Timer {}
impl Drop for Timer {
    fn drop(&mut self) {
        unsafe {
            UnmapViewOfFile(self.view);
            CloseHandle(self.mutex);
        }
    }
}
struct Locked<'a>(&'a Timer);
impl Drop for Locked<'_> {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0.mutex);
        }
    }
}
impl Timer {
    fn map(raw: HANDLE) -> Result<Self, i32> {
        let view = unsafe { MapViewOfFile(raw, FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
        if view.Value.is_null() {
            return Err(errno_from_win32(unsafe { GetLastError() }));
        }
        let state = unsafe { &*view.Value.cast::<State>() };
        if state.magic != MAGIC {
            unsafe {
                UnmapViewOfFile(view);
            }
            return Err(EINVAL);
        }
        let name = format!(
            r"Local\kinakaze.timer.v1.{}.{}",
            kinakaze_runtime::authority::domain_id(),
            state.id
        )
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
        let mutex = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if mutex.is_null() {
            unsafe {
                UnmapViewOfFile(view);
            }
            return Err(EIO);
        }
        Ok(Self { view, mutex })
    }
    fn from_fd(fd: i32) -> Result<(Self, FdFlags), i32> {
        let table = crate::table().read().map_err(|_| EIO)?;
        let entry = table.slots.get(fd as usize).and_then(|e| *e).ok_or(EBADF)?;
        if entry.kind != FdKind::TimerFd {
            return Err(EINVAL);
        }
        Ok((Self::map(entry.raw as HANDLE)?, entry.flags))
    }
    fn lock(&self) -> Result<Locked<'_>, i32> {
        match unsafe { WaitForSingleObject(self.mutex, INFINITE) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Locked(self)),
            _ => Err(EIO),
        }
    }
}
impl Locked<'_> {
    fn state(&mut self) -> &mut State {
        unsafe { &mut *self.0.view.Value.cast::<State>() }
    }
}
fn now(clock: i32) -> Result<i64, i32> {
    if clock == 0 {
        let mut ft = windows_sys::Win32::Foundation::FILETIME::default();
        unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime(&mut ft);
        }
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        Ok(((ticks as i128 - 116444736000000000) * 100) as i64)
    } else {
        crate::time_namespace::host_nanoseconds(clock)
    }
}
fn refresh(state: &mut State, now: i64) {
    if state.deadline != 0 && now >= state.deadline {
        let count = if state.interval == 0 {
            1
        } else {
            ((now - state.deadline) / state.interval) as u64 + 1
        };
        state.ticks = state.ticks.saturating_add(count);
        state.deadline = if state.interval == 0 {
            0
        } else {
            (state.deadline as i128 + count as i128 * state.interval as i128).min(i64::MAX as i128)
                as i64
        };
    }
}
fn remaining(state: &State, now: i64) -> Itimerspec {
    Itimerspec {
        interval: Timespec::from_nanos(state.interval),
        value: Timespec::from_nanos((state.deadline - now).max(0)),
    }
}
pub fn create(clock: i32, flags: i32) -> Result<i32, i32> {
    if matches!(clock, 8 | 9) {
        return Err(crate::EPERM);
    }
    if !matches!(clock, 0 | 1 | 7) || flags & !(0o2000000 | 0o4000) != 0 {
        return Err(EINVAL);
    }
    let mut fd_flags = FdFlags::NONE;
    if flags & 0o2000000 != 0 {
        fd_flags = fd_flags.union(FdFlags::CLOSE_ON_EXEC);
    }
    if flags & 0o4000 != 0 {
        fd_flags = fd_flags.union(FdFlags::NONBLOCK);
    }
    let id = crate::time_namespace::allocate_object_id()?;
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            ptr::null(),
            PAGE_READWRITE,
            0,
            4096,
            ptr::null(),
        )
    };
    if handle.is_null() {
        return Err(errno_from_win32(unsafe { GetLastError() }));
    }
    let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 4096) };
    if view.Value.is_null() {
        unsafe {
            CloseHandle(handle);
        }
        return Err(EIO);
    }
    unsafe {
        ptr::write(
            view.Value.cast::<State>(),
            State {
                magic: MAGIC,
                id,
                clock,
                _pad: 0,
                deadline: 0,
                interval: 0,
                ticks: 0,
            },
        );
        UnmapViewOfFile(view);
    }
    match crate::install(handle as usize, FdKind::TimerFd, fd_flags) {
        Ok(fd) => Ok(fd),
        Err(e) => {
            unsafe {
                CloseHandle(handle);
            }
            Err(e)
        }
    }
}
pub fn settime(fd: i32, flags: i32, value: Itimerspec) -> Result<Itimerspec, i32> {
    if flags & !3 != 0 {
        return Err(EINVAL);
    }
    // Host-wide wall-clock cancellation needs a clock-change subscription.
    if flags & 2 != 0 {
        return Err(crate::EOPNOTSUPP);
    }
    let interval = value.interval.nanos()?;
    let value = value.value.nanos()?;
    let (timer, _) = Timer::from_fd(fd)?;
    let mut guard = timer.lock()?;
    let state = guard.state();
    let now = now(state.clock)?;
    let deadline = if value == 0 {
        0
    } else if flags & 1 != 0 {
        value
            .checked_sub(crate::time_namespace::offset(state.clock)?)
            .ok_or(EINVAL)?
            .max(1)
    } else {
        now.checked_add(value).ok_or(EINVAL)?
    };
    refresh(state, now);
    let old = remaining(state, now);
    state.interval = interval;
    state.deadline = deadline;
    state.ticks = 0;
    Ok(old)
}
pub fn gettime(fd: i32) -> Result<Itimerspec, i32> {
    let (timer, _) = Timer::from_fd(fd)?;
    let mut guard = timer.lock()?;
    let state = guard.state();
    let now = now(state.clock)?;
    refresh(state, now);
    Ok(remaining(state, now))
}
pub fn poll(fd: i32) -> Result<(bool, bool), i32> {
    let (timer, _) = Timer::from_fd(fd)?;
    let mut guard = timer.lock()?;
    let state = guard.state();
    refresh(state, now(state.clock)?);
    Ok((state.ticks != 0, false))
}
pub fn read(fd: i32, buffer: &mut [u8]) -> Result<usize, i32> {
    if buffer.len() < 8 {
        return Err(EINVAL);
    }
    let (timer, flags) = Timer::from_fd(fd)?;
    let interrupt = crate::interrupt::current();
    if interrupt.is_null() {
        return Err(EIO);
    }
    struct Waiter;
    impl Drop for Waiter {
        fn drop(&mut self) {
            crate::signal::unregister_waiter();
        }
    }
    crate::signal::register_waiter();
    let _waiter = Waiter;
    loop {
        let timeout = {
            let mut guard = timer.lock()?;
            let state = guard.state();
            let now = now(state.clock)?;
            refresh(state, now);
            if state.ticks != 0 {
                buffer[..8].copy_from_slice(&state.ticks.to_ne_bytes());
                state.ticks = 0;
                return Ok(8);
            }
            if flags.contains(FdFlags::NONBLOCK) {
                return Err(EAGAIN);
            }
            if state.deadline == 0 {
                10
            } else {
                ((state.deadline - now).max(0) as u64)
                    .div_ceil(1_000_000)
                    .clamp(1, 10) as u32
            }
        };
        match crate::signal::deliver_pending() {
            crate::signal::Delivery::Interrupted => return Err(EINTR),
            _ => (),
        }
        unsafe {
            WaitForSingleObject(interrupt, timeout);
        }
    }
}
/// Copy guest structures only after validating null pointers at the ABI boundary.
pub unsafe fn settime_raw(
    fd: i32,
    flags: i32,
    new: *const Itimerspec,
    old: *mut Itimerspec,
) -> Result<(), i32> {
    if new.is_null() {
        return Err(EFAULT);
    }
    let previous = settime(fd, flags, unsafe { ptr::read_unaligned(new) })?;
    if !old.is_null() {
        unsafe {
            ptr::write_unaligned(old, previous);
        }
    }
    Ok(())
}
