//! Per-worker POSIX timer registry. Timers are neither fork-inherited nor kept
//! across exec; only this rlib instance in the runtime owns native scheduling.

use core::ffi::{c_int, c_void};
use libc::time::TimeSpec;
use std::collections::HashMap;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThreadId, GetProcessTimes, GetThreadTimes, OpenThread,
    THREAD_QUERY_LIMITED_INFORMATION,
};

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ItimerSpec {
    pub interval: TimeSpec,
    pub value: TimeSpec,
}

/// Linux x86-64 sigevent: sigval + signo + notify + 48-byte variant union.
#[repr(C)]
pub struct SigEvent {
    pub value: usize,
    pub signal: i32,
    pub notify: i32,
    pub variant: [usize; 6],
}

enum Clock {
    Wall(i32),
    Process,
    Thread(OwnedHandle),
}

impl Clock {
    fn new(clock: i32) -> Result<Self, i32> {
        match clock {
            0 | 1 | 7 => Ok(Self::Wall(clock)),
            2 => Ok(Self::Process),
            3 => Self::thread(unsafe { GetCurrentThreadId() }),
            8 | 9 => Err(1), // Wake alarms require a wake-capable host privilege.
            encoded if encoded < 0 && encoded & 7 == 6 => Self::thread(!(encoded >> 3) as u32),
            _ => Err(22),
        }
    }
    fn thread(id: u32) -> Result<Self, i32> {
        // A guest thread clock may only target a thread in this worker.
        if id != unsafe { GetCurrentThreadId() } && !kinakaze_vfs::interrupt::thread_exists(id) {
            return Err(22);
        }
        let handle = unsafe { OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, id) };
        if handle.is_null() {
            return Err(22);
        }
        Ok(Self::Thread(unsafe {
            OwnedHandle::from_raw_handle(handle)
        }))
    }
    fn now(&self) -> Result<i128, i32> {
        if let Self::Wall(clock) = self {
            let mut value = TimeSpec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            if unsafe { libc::time::kinakaze_abi_clock_gettime(*clock, &mut value) } != 0 {
                return Err(unsafe { *libc::kinakaze_abi___errno_location() });
            }
            return Ok(value.tv_sec as i128 * 1_000_000_000 + value.tv_nsec as i128);
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut creation, mut exit, mut kernel, mut user) = (zero, zero, zero, zero);
        let ok = unsafe {
            match self {
                Self::Process => GetProcessTimes(
                    GetCurrentProcess(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                ),
                Self::Thread(handle) => GetThreadTimes(
                    handle.as_raw_handle(),
                    &mut creation,
                    &mut exit,
                    &mut kernel,
                    &mut user,
                ),
                Self::Wall(_) => unreachable!(),
            }
        };
        if ok == 0 {
            return Err(22);
        }
        let ticks =
            |value: FILETIME| ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64;
        Ok((ticks(kernel) as i128 + ticks(user) as i128) * 100)
    }
    fn wait_duration(&self, remaining: i128) -> Duration {
        let ns = remaining.max(1).min(u64::MAX as i128) as u64;
        match self {
            Self::Wall(0) => Duration::from_nanos(ns).min(Duration::from_secs(1)),
            Self::Wall(_) => Duration::from_nanos(ns),
            // Windows has no user CPU-budget wait primitive. Sampling exists
            // only while a CPU timer is armed; disarmed timers never poll.
            _ => Duration::from_millis(1),
        }
    }
}

#[derive(Clone)]
enum Notification {
    None,
    Signal {
        number: i32,
        target: Option<u32>,
        value: usize,
    },
    Thread {
        callback: usize,
        value: usize,
        attributes: Option<libpthread::PthreadAttr>,
    },
}

struct Timer {
    clock: Clock,
    realtime_relative: bool,
    notification: Notification,
    deadline: Option<i128>,
    interval: i128,
    overrun: Arc<AtomicI32>,
}

impl Timer {
    fn now(&self) -> Result<i128, i32> {
        if self.realtime_relative {
            Clock::Wall(1).now()
        } else {
            self.clock.now()
        }
    }
    fn wait_duration(&self, remaining: i128) -> Duration {
        if self.realtime_relative {
            Clock::Wall(1).wait_duration(remaining)
        } else {
            self.clock.wait_duration(remaining)
        }
    }
}

struct Registry {
    next: i32,
    timers: HashMap<i32, Timer>,
}
struct Service {
    registry: Mutex<Registry>,
    changed: Condvar,
}
static SERVICE: OnceLock<Result<Arc<Service>, i32>> = OnceLock::new();

fn service() -> Result<&'static Arc<Service>, i32> {
    SERVICE
        .get_or_init(|| {
            let service = Arc::new(Service {
                registry: Mutex::new(Registry {
                    next: 1,
                    timers: HashMap::new(),
                }),
                changed: Condvar::new(),
            });
            let worker = Arc::clone(&service);
            std::thread::Builder::new()
                .name("kinakaze-posix-timers".into())
                .spawn(move || run(worker))
                .map_err(|_| 11)?;
            Ok(service)
        })
        .as_ref()
        .map_err(|error| *error)
}

fn fail(error: i32) -> i32 {
    unsafe {
        *libc::kinakaze_abi___errno_location() = error;
    }
    -1
}
fn nanoseconds(time: TimeSpec) -> Result<i128, i32> {
    if time.tv_sec < 0 || !(0..1_000_000_000).contains(&time.tv_nsec) {
        return Err(22);
    }
    Ok(time.tv_sec as i128 * 1_000_000_000 + time.tv_nsec as i128)
}
fn timespec(ns: i128) -> TimeSpec {
    let ns = ns.max(0);
    TimeSpec {
        tv_sec: (ns / 1_000_000_000).min(i64::MAX as i128) as i64,
        tv_nsec: (ns % 1_000_000_000) as i64,
    }
}
fn status(timer: &Timer, now: i128) -> ItimerSpec {
    ItimerSpec {
        interval: timespec(timer.interval),
        value: timespec(timer.deadline.map_or(0, |deadline| (deadline - now).max(1))),
    }
}

fn run(service: Arc<Service>) {
    loop {
        let mut registry = service
            .registry
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut wait = None::<Duration>;
        let mut due = Vec::new();
        for (&id, timer) in registry.timers.iter_mut() {
            let Some(deadline) = timer.deadline else {
                continue;
            };
            let Ok(now) = timer.now() else {
                timer.deadline = None;
                continue;
            };
            if now < deadline {
                let duration = timer.wait_duration(deadline - now);
                wait = Some(wait.map_or(duration, |previous| previous.min(duration)));
                continue;
            }
            let elapsed = if timer.interval > 0 {
                1 + (now - deadline) / timer.interval
            } else {
                1
            };
            timer.deadline = (timer.interval > 0).then(|| deadline + elapsed * timer.interval);
            let overrun = (elapsed - 1).min(i32::MAX as i128) as i32;
            due.push((
                id,
                timer.notification.clone(),
                Arc::clone(&timer.overrun),
                overrun,
            ));
        }
        if due.is_empty() {
            if let Some(wait) = wait {
                drop(
                    service
                        .changed
                        .wait_timeout(registry, wait)
                        .unwrap_or_else(|error| error.into_inner()),
                );
            } else {
                drop(
                    service
                        .changed
                        .wait(registry)
                        .unwrap_or_else(|error| error.into_inner()),
                );
            }
            continue;
        }
        drop(registry);
        for (id, notification, last_overrun, overrun) in due {
            match notification {
                Notification::None => {
                    last_overrun.store(overrun, Ordering::Release);
                }
                Notification::Signal {
                    number,
                    target,
                    value,
                } => {
                    let _ = kinakaze_vfs::signal::queue_timer_signal(
                        target,
                        number,
                        id,
                        value,
                        overrun,
                        last_overrun,
                    );
                }
                Notification::Thread {
                    callback,
                    value,
                    attributes,
                } => {
                    last_overrun.store(overrun, Ordering::Release);
                    let packet = Box::into_raw(Box::new((callback, value)));
                    let mut thread = 0;
                    let result = unsafe {
                        libpthread::pthread_create(
                            &mut thread,
                            attributes
                                .as_ref()
                                .map_or(core::ptr::null(), core::ptr::from_ref),
                            Some(invoke),
                            packet.cast(),
                        )
                    };
                    if result == 0 {
                        libpthread::pthread_detach(thread);
                    } else {
                        drop(unsafe { Box::from_raw(packet) });
                    }
                }
            }
        }
    }
}

unsafe extern "sysv64" fn invoke(packet: *mut c_void) -> *mut c_void {
    let (callback, value) = *unsafe { Box::from_raw(packet.cast::<(usize, usize)>()) };
    let callback: unsafe extern "sysv64" fn(usize) = unsafe { core::mem::transmute(callback) };
    unsafe {
        callback(value);
    }
    core::ptr::null_mut()
}

#[unsafe(export_name = "kinakaze_engine_librt_timer_create")]
pub unsafe extern "sysv64" fn timer_create(
    clock: c_int,
    event: *const SigEvent,
    output: *mut *mut c_void,
) -> c_int {
    if output.is_null() {
        return fail(14);
    }
    let clock = match Clock::new(clock) {
        Ok(clock) => clock,
        Err(error) => return fail(error),
    };
    let service = match service() {
        Ok(service) => service,
        Err(error) => return fail(error),
    };
    let mut registry = service
        .registry
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if registry.timers.len() >= 4096 || registry.next == i32::MAX {
        return fail(11);
    }
    let id = registry.next;
    let notification = if event.is_null() {
        Notification::Signal {
            number: 14,
            target: None,
            value: id as usize,
        }
    } else {
        let event = unsafe { &*event };
        match event.notify {
            1 => Notification::None,
            0 | 4 => {
                if !(1..=64).contains(&event.signal) {
                    return fail(22);
                }
                let target = if event.notify == 4 {
                    Some(event.variant[0] as u32)
                } else {
                    None
                };
                if target.is_some_and(|tid| !kinakaze_vfs::interrupt::thread_exists(tid)) {
                    return fail(22);
                }
                Notification::Signal {
                    number: event.signal,
                    target,
                    value: event.value,
                }
            }
            2 => {
                if event.variant[0] == 0 {
                    return fail(22);
                }
                let attributes = if event.variant[1] == 0 {
                    None
                } else {
                    Some(unsafe { *(event.variant[1] as *const libpthread::PthreadAttr) })
                };
                Notification::Thread {
                    callback: event.variant[0],
                    value: event.value,
                    attributes,
                }
            }
            _ => return fail(22),
        }
    };
    registry.next += 1;
    registry.timers.insert(
        id,
        Timer {
            clock,
            realtime_relative: false,
            notification,
            deadline: None,
            interval: 0,
            overrun: Arc::new(AtomicI32::new(0)),
        },
    );
    unsafe {
        output.write(id as usize as *mut c_void);
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_librt_timer_delete")]
pub extern "sysv64" fn timer_delete(id: *mut c_void) -> c_int {
    let service = match service() {
        Ok(service) => service,
        Err(error) => return fail(error),
    };
    if id as usize > i32::MAX as usize {
        return fail(22);
    }
    if service
        .registry
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .timers
        .remove(&(id as i32))
        .is_none()
    {
        return fail(22);
    }
    service.changed.notify_one();
    0
}

#[unsafe(export_name = "kinakaze_engine_librt_timer_settime")]
pub unsafe extern "sysv64" fn timer_settime(
    id: *mut c_void,
    flags: c_int,
    value: *const ItimerSpec,
    old: *mut ItimerSpec,
) -> c_int {
    if value.is_null() {
        return fail(14);
    }
    if flags & !1 != 0 || id as usize > i32::MAX as usize {
        return fail(22);
    }
    let value = unsafe { value.read() };
    let interval = match nanoseconds(value.interval) {
        Ok(ns) => ns,
        Err(error) => return fail(error),
    };
    let initial = match nanoseconds(value.value) {
        Ok(ns) => ns,
        Err(error) => return fail(error),
    };
    let service = match service() {
        Ok(service) => service,
        Err(error) => return fail(error),
    };
    let mut registry = service
        .registry
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(timer) = registry.timers.get_mut(&(id as i32)) else {
        return fail(22);
    };
    let old_now = match timer.now() {
        Ok(now) => now,
        Err(error) => return fail(error),
    };
    if !old.is_null() {
        unsafe {
            old.write(status(timer, old_now));
        }
    }
    // Relative CLOCK_REALTIME timers must ignore later wall-clock changes.
    let realtime_relative = matches!(timer.clock, Clock::Wall(0)) && flags & 1 == 0;
    let now = match if realtime_relative {
        Clock::Wall(1).now()
    } else {
        timer.clock.now()
    } {
        Ok(now) => now,
        Err(error) => return fail(error),
    };
    timer.realtime_relative = realtime_relative;
    timer.interval = interval;
    timer.deadline = if initial == 0 {
        None
    } else {
        Some(if flags & 1 == 0 {
            now + initial
        } else {
            initial
        })
    };
    timer.overrun.store(0, Ordering::Release);
    drop(registry);
    service.changed.notify_one();
    0
}

#[unsafe(export_name = "kinakaze_engine_librt_timer_gettime")]
pub unsafe extern "sysv64" fn timer_gettime(id: *mut c_void, value: *mut ItimerSpec) -> c_int {
    if value.is_null() {
        return fail(14);
    }
    if id as usize > i32::MAX as usize {
        return fail(22);
    }
    let service = match service() {
        Ok(service) => service,
        Err(error) => return fail(error),
    };
    let registry = service
        .registry
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(timer) = registry.timers.get(&(id as i32)) else {
        return fail(22);
    };
    let now = match timer.now() {
        Ok(now) => now,
        Err(error) => return fail(error),
    };
    unsafe {
        value.write(status(timer, now));
    }
    0
}

#[unsafe(export_name = "kinakaze_engine_librt_timer_getoverrun")]
pub extern "sysv64" fn timer_getoverrun(id: *mut c_void) -> c_int {
    if id as usize > i32::MAX as usize {
        return fail(22);
    }
    let service = match service() {
        Ok(service) => service,
        Err(error) => return fail(error),
    };
    let registry = service
        .registry
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(timer) = registry.timers.get(&(id as i32)) else {
        return fail(22);
    };
    timer.overrun.load(Ordering::Acquire)
}

/// Linux x86-64 raw timer syscalls, shared with the libc facade registry.
/// Unlike the libc functions this returns negative errno and preserves errno.
/// Raw timer_create writes a 32-bit kernel timer ID and does not support the
/// libc-only SIGEV_THREAD callback construction.
pub unsafe fn raw_syscall(number: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let errno = libc::kinakaze_abi___errno_location();
    let previous = unsafe { *errno };
    let result = match number {
        222 => {
            if a3 == 0 {
                return -14;
            }
            if a2 != 0 && unsafe { (*(a2 as *const SigEvent)).notify } == 2 {
                return -22;
            }
            let mut timer = core::ptr::null_mut();
            let result = unsafe { timer_create(a1 as i32, a2 as *const SigEvent, &mut timer) };
            if result == 0 {
                unsafe {
                    (a3 as *mut i32).write(timer as i32);
                }
            }
            result
        }
        223 => unsafe {
            timer_settime(
                a1 as *mut c_void,
                a2 as i32,
                a3 as *const ItimerSpec,
                a4 as *mut ItimerSpec,
            )
        },
        224 => unsafe { timer_gettime(a1 as *mut c_void, a2 as *mut ItimerSpec) },
        225 => timer_getoverrun(a1 as *mut c_void),
        226 => timer_delete(a1 as *mut c_void),
        _ => return -22,
    };
    let result = if result == -1 {
        -i64::from(unsafe { *errno })
    } else {
        i64::from(result)
    };
    unsafe {
        *errno = previous;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none_event() -> SigEvent {
        SigEvent {
            value: 0,
            signal: 0,
            notify: 1,
            variant: [0; 6],
        }
    }
    struct TimerGuard(*mut c_void);
    impl Drop for TimerGuard {
        fn drop(&mut self) {
            timer_delete(self.0);
        }
    }

    #[test]
    fn timer_ids_arm_disarm_delete_and_errors_have_real_state() {
        assert_eq!(core::mem::size_of::<SigEvent>(), 64);
        assert_eq!(core::mem::size_of::<ItimerSpec>(), 32);
        let event = none_event();
        let mut first = core::ptr::null_mut();
        let mut second = core::ptr::null_mut();
        assert_eq!(unsafe { timer_create(1, &event, &mut first) }, 0);
        let _first = TimerGuard(first);
        assert_eq!(unsafe { timer_create(1, &event, &mut second) }, 0);
        let second_guard = TimerGuard(second);
        assert_ne!(first, second);
        let setting = ItimerSpec {
            interval: timespec(250_000_000),
            value: timespec(2_000_000_000),
        };
        assert_eq!(
            unsafe { timer_settime(first, 0, &setting, core::ptr::null_mut()) },
            0
        );
        let mut observed = ItimerSpec {
            interval: timespec(0),
            value: timespec(0),
        };
        assert_eq!(unsafe { timer_gettime(first, &mut observed) }, 0);
        assert_eq!(nanoseconds(observed.interval), Ok(250_000_000));
        assert!((1_000_000_000..=2_000_000_000).contains(&nanoseconds(observed.value).unwrap()));
        assert_eq!(
            unsafe { timer_settime(first, 2, &setting, core::ptr::null_mut()) },
            -1
        );
        let disarm = ItimerSpec {
            interval: timespec(0),
            value: timespec(0),
        };
        assert_eq!(
            unsafe { timer_settime(first, 0, &disarm, &mut observed) },
            0
        );
        assert!(nanoseconds(observed.value).unwrap() > 0);
        assert_eq!(unsafe { timer_gettime(first, &mut observed) }, 0);
        assert_eq!(nanoseconds(observed.value), Ok(0));
        drop(second_guard);
        assert_eq!(timer_getoverrun(second), -1);
        assert_eq!(unsafe { *libc::kinakaze_abi___errno_location() }, 22);
    }

    #[test]
    fn raw_timer_syscalls_share_registry_use_kernel_id_width_and_preserve_errno() {
        let event = none_event();
        let mut storage = 0x1234_5678_0000_0000u64;
        unsafe {
            *libc::kinakaze_abi___errno_location() = 77;
        }
        assert_eq!(
            unsafe {
                raw_syscall(
                    222,
                    1,
                    core::ptr::from_ref(&event) as u64,
                    (&raw mut storage) as u64,
                    0,
                )
            },
            0
        );
        assert_eq!(storage >> 32, 0x1234_5678);
        assert_eq!(unsafe { *libc::kinakaze_abi___errno_location() }, 77);
        let id = storage as u32 as usize as *mut c_void;
        let _timer = TimerGuard(id);
        assert_eq!(timer_getoverrun(id), 0);
        assert_eq!(unsafe { raw_syscall(224, id as u64, 0, 0, 0) }, -14);
        assert_eq!(unsafe { *libc::kinakaze_abi___errno_location() }, 77);
    }

    unsafe extern "sysv64" fn notify(value: usize) {
        let sender = unsafe { Box::from_raw(value as *mut std::sync::mpsc::Sender<usize>) };
        let _ = sender.send(libpthread::pthread_self());
    }

    #[test]
    fn armed_timer_invokes_a_real_pthread_callback() {
        let (sender, receiver) = std::sync::mpsc::channel::<usize>();
        let sender = Box::into_raw(Box::new(sender));
        let event = SigEvent {
            value: sender as usize,
            signal: 0,
            notify: 2,
            variant: [notify as *const () as usize, 0, 0, 0, 0, 0],
        };
        let mut id = core::ptr::null_mut();
        assert_eq!(unsafe { timer_create(1, &event, &mut id) }, 0);
        let _timer = TimerGuard(id);
        let setting = ItimerSpec {
            interval: timespec(0),
            value: timespec(10_000_000),
        };
        assert_eq!(
            unsafe { timer_settime(id, 0, &setting, core::ptr::null_mut()) },
            0
        );
        assert_ne!(receiver.recv_timeout(Duration::from_secs(3)).unwrap(), 0);
        let mut observed = ItimerSpec {
            interval: timespec(0),
            value: timespec(0),
        };
        assert_eq!(unsafe { timer_gettime(id, &mut observed) }, 0);
        assert_eq!(nanoseconds(observed.value), Ok(0));
    }
}
