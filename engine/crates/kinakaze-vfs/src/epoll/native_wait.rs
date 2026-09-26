//! Per-wait native event fan-in. No persistent worker, descriptor state, or
//! callback pointer crosses fork: callbacks belong only to the parent's active
//! wait and are retired before their handles are released.

use std::ptr;
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, HANDLE, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::System::Threading::{
    CloseThreadpoolWait, CreateEventW, CreateThreadpoolWait, GetCurrentProcess,
    PTP_CALLBACK_INSTANCE, PTP_WAIT, SetEvent, SetThreadpoolWait, WaitForSingleObject,
    WaitForThreadpoolWaitCallbacks,
};

pub(crate) enum Outcome {
    Ready,
    Interrupted,
    Timeout,
}

pub(crate) struct Source(HANDLE);
impl Source {
    /// # Safety
    /// The source owner must prevent close while this duplication runs.
    pub(crate) unsafe fn duplicate(handle: HANDLE) -> Result<Self, i32> {
        let mut pinned = ptr::null_mut();
        let process = unsafe { GetCurrentProcess() };
        if unsafe {
            DuplicateHandle(
                process,
                handle,
                process,
                &mut pinned,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        } == 0
        {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Self(pinned))
    }

    pub(crate) fn for_fd(fd: i32, description: u64) -> Result<Self, i32> {
        let table = crate::table().read().map_err(|_| crate::EIO)?;
        let entry = usize::try_from(fd)
            .ok()
            .and_then(|fd| table.slots.get(fd))
            .and_then(|entry| *entry)
            .ok_or(crate::EBADF)?;
        if entry.description_id != description {
            return Err(crate::EBADF);
        }
        unsafe { Self::duplicate(entry.raw as HANDLE) }
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0
    }
}
impl Drop for Source {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(crate) struct FanIn {
    event: HANDLE,
    waits: Vec<PTP_WAIT>,
}

unsafe extern "system" fn signaled(
    _: PTP_CALLBACK_INSTANCE,
    context: *mut core::ffi::c_void,
    _: PTP_WAIT,
    _: u32,
) {
    // Only a kernel event operation: no allocation, locks, or guest callbacks.
    unsafe { SetEvent(context) };
}

impl FanIn {
    pub(crate) fn new() -> Result<Self, i32> {
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        Ok(Self {
            event,
            waits: Vec::new(),
        })
    }

    // Caller retains each source through destruction of this fan-in.
    pub(crate) unsafe fn add(&mut self, source: HANDLE) -> Result<(), i32> {
        self.waits.try_reserve(1).map_err(|_| crate::ENOMEM)?;
        let wait = unsafe { CreateThreadpoolWait(Some(signaled), self.event, ptr::null()) };
        if wait == 0 {
            return Err(crate::errno_from_win32(unsafe { GetLastError() }));
        }
        self.waits.push(wait);
        unsafe { SetThreadpoolWait(wait, source, ptr::null()) };
        Ok(())
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.event
    }
}

impl Drop for FanIn {
    fn drop(&mut self) {
        // Stop queueing first, then cancel queued callbacks and retire running
        // ones. Neither this event nor any source can be closed before that.
        for &wait in &self.waits {
            unsafe { SetThreadpoolWait(wait, ptr::null_mut(), ptr::null()) };
        }
        for &wait in &self.waits {
            unsafe {
                WaitForThreadpoolWaitCallbacks(wait, 1);
                CloseThreadpoolWait(wait);
            }
        }
        unsafe { CloseHandle(self.event) };
    }
}

/// Wait on any number of non-consuming readiness events, plus an interrupt.
/// # Safety
/// Every source and interrupt HANDLE remains live until this function returns.
/// Sources must be events or other non-consuming waitable notification objects,
/// not mutexes/semaphores whose wait would mutate ownership or consume a count.
pub(crate) unsafe fn wait(
    sources: &[HANDLE],
    interrupt: HANDLE,
    timeout: u32,
) -> Result<Outcome, i32> {
    const MAXIMUM_WAIT_OBJECTS: usize = 64;
    let interrupt_count = usize::from(!interrupt.is_null());
    if timeout == 0 && sources.len() + interrupt_count > MAXIMUM_WAIT_OBJECTS {
        // A nonblocking probe must observe already-set events immediately,
        // without waiting for a threadpool callback to be scheduled first.
        for handle in (!interrupt.is_null())
            .then_some(interrupt)
            .into_iter()
            .chain(sources.iter().copied())
        {
            match unsafe { WaitForSingleObject(handle, 0) } {
                WAIT_OBJECT_0 => {
                    return Ok(if handle == interrupt {
                        Outcome::Interrupted
                    } else {
                        Outcome::Ready
                    });
                }
                WAIT_TIMEOUT => {}
                _ => return Err(crate::errno_from_win32(unsafe { GetLastError() })),
            }
        }
        return Ok(Outcome::Timeout);
    }
    let timer_count = usize::from(timeout != 0 && timeout != u32::MAX);
    let fan_in = if sources.len() + interrupt_count + timer_count > MAXIMUM_WAIT_OBJECTS {
        let mut group = FanIn::new()?;
        for &source in sources {
            unsafe { group.add(source)? };
        }
        Some(group)
    } else {
        None
    };
    let mut handles = Vec::new();
    // Deliver a pending signal even when another readiness source stays set.
    if !interrupt.is_null() {
        handles.push(interrupt);
    }
    if let Some(group) = &fan_in {
        handles.push(group.event);
    } else {
        handles.extend_from_slice(sources);
    }
    if handles.is_empty() {
        return Err(crate::EINVAL);
    }
    let result = unsafe { crate::deadline_wait::any(&handles, timeout) };
    if result == WAIT_TIMEOUT {
        return Ok(Outcome::Timeout);
    }
    if result == WAIT_OBJECT_0 && !interrupt.is_null() {
        return Ok(Outcome::Interrupted);
    }
    if result < WAIT_OBJECT_0 + handles.len() as u32 {
        return Ok(Outcome::Ready);
    }
    Err(crate::errno_from_win32(unsafe { GetLastError() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Events(Vec<HANDLE>);
    impl Events {
        fn new(count: usize) -> Self {
            Self(
                (0..count)
                    .map(|_| {
                        let h = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
                        assert!(!h.is_null());
                        h
                    })
                    .collect(),
            )
        }
    }
    impl Drop for Events {
        fn drop(&mut self) {
            for &h in &self.0 {
                unsafe { CloseHandle(h) };
            }
        }
    }

    #[test]
    fn source_above_the_native_wait_limit_wakes_without_polling() {
        let events = Events::new(130);
        unsafe { SetEvent(events.0[129]) };
        assert!(matches!(
            unsafe { wait(&events.0, ptr::null_mut(), 1000) },
            Ok(Outcome::Ready)
        ));
        assert!(matches!(
            unsafe { wait(&events.0, ptr::null_mut(), 0) },
            Ok(Outcome::Ready)
        ));
    }

    #[test]
    fn interrupt_is_not_truncated_from_a_large_wait_set() {
        let events = Events::new(130);
        let interrupt = Events::new(1);
        unsafe { SetEvent(interrupt.0[0]) };
        assert!(matches!(
            unsafe { wait(&events.0, interrupt.0[0], 1000) },
            Ok(Outcome::Interrupted)
        ));
    }

    #[test]
    fn finite_deadline_reserves_a_handle_at_the_native_wait_limit() {
        let events = Events::new(63);
        let interrupt = Events::new(1);
        assert!(matches!(
            unsafe { wait(&events.0, interrupt.0[0], 1) },
            Ok(Outcome::Timeout)
        ));
        unsafe { SetEvent(events.0[62]) };
        assert!(matches!(
            unsafe { wait(&events.0, interrupt.0[0], 1000) },
            Ok(Outcome::Ready)
        ));
        unsafe { SetEvent(interrupt.0[0]) };
        assert!(matches!(
            unsafe { wait(&events.0, interrupt.0[0], 1000) },
            Ok(Outcome::Interrupted)
        ));
    }

    #[test]
    fn timeout_retires_callbacks_before_sources_can_close() {
        for _ in 0..16 {
            let events = Events::new(65);
            assert!(matches!(
                unsafe { wait(&events.0, ptr::null_mut(), 1) },
                Ok(Outcome::Timeout)
            ));
            // Closing all the sources immediately is valid only after disarm
            // and callback retirement, including callbacks queued at timeout.
        }
    }

    #[test]
    fn descriptor_close_does_not_release_an_active_wait_source() {
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        assert!(!event.is_null());
        let fd =
            crate::install(event as usize, crate::FdKind::Event, crate::FdFlags::NONE).unwrap();
        let entry = crate::get(fd).unwrap();
        let source = Source::for_fd(fd, entry.description_id).unwrap();
        crate::close(fd).unwrap();
        assert_ne!(unsafe { SetEvent(source.raw()) }, 0);
        assert!(matches!(
            unsafe { wait(&[source.raw()], ptr::null_mut(), 0) },
            Ok(Outcome::Ready)
        ));
    }
}
