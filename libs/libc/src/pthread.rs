//! System V ABI exports for POSIX threads integrated into `libc.dll` (glibc 2.34+).

#![allow(non_camel_case_types, non_snake_case, clippy::missing_safety_doc)]

use core::ffi::c_void;
use libpthread::{PthreadAttr, PthreadCondAttr, PthreadMutexAttr, StartRoutine, Timespec};

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_self() -> usize {
    libpthread::pthread_self()
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_equal(left: usize, right: usize) -> i32 {
    libpthread::pthread_equal(left, right)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_create(
    thread: *mut usize,
    attr: *const PthreadAttr,
    start: Option<StartRoutine>,
    argument: *mut c_void,
) -> i32 {
    unsafe { libpthread::pthread_create(thread, attr, start, argument) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_join(
    thread: usize,
    result: *mut *mut c_void,
) -> i32 {
    unsafe { libpthread::pthread_join(thread, result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_tryjoin_np(
    thread: usize,
    result: *mut *mut c_void,
) -> i32 {
    unsafe { libpthread::pthread_tryjoin_np(thread, result) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_detach(thread: usize) -> i32 {
    libpthread::pthread_detach(thread)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_once(
    control: *mut u32,
    init: Option<unsafe extern "sysv64" fn()>,
) -> i32 {
    unsafe { libpthread::pthread_once(control, init) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutex_init(
    mutex: *mut usize,
    attr: *const PthreadMutexAttr,
) -> i32 {
    unsafe { libpthread::pthread_mutex_init(mutex, attr) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_mutex_destroy(mutex: *mut usize) -> i32 {
    libpthread::pthread_mutex_destroy(mutex)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutex_lock(mutex: *mut usize) -> i32 {
    unsafe { libpthread::pthread_mutex_lock(mutex) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutex_trylock(mutex: *mut usize) -> i32 {
    unsafe { libpthread::pthread_mutex_trylock(mutex) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutex_unlock(mutex: *mut usize) -> i32 {
    unsafe { libpthread::pthread_mutex_unlock(mutex) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutex_timedlock(
    mutex: *mut usize,
    deadline: *const Timespec,
) -> i32 {
    unsafe { libpthread::pthread_mutex_timedlock(mutex, deadline) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutexattr_init(
    attr: *mut PthreadMutexAttr,
) -> i32 {
    unsafe { libpthread::pthread_mutexattr_init(attr) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_mutexattr_destroy(attr: *mut PthreadMutexAttr) -> i32 {
    libpthread::pthread_mutexattr_destroy(attr)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutexattr_settype(
    attr: *mut PthreadMutexAttr,
    kind: i32,
) -> i32 {
    unsafe { libpthread::pthread_mutexattr_settype(attr, kind) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutexattr_gettype(
    attr: *const PthreadMutexAttr,
    kind: *mut i32,
) -> i32 {
    unsafe { libpthread::pthread_mutexattr_gettype(attr, kind) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_cond_init(
    cond: *mut usize,
    attr: *const c_void,
) -> i32 {
    unsafe { libpthread::pthread_cond_init(cond, attr) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_cond_destroy(cond: *mut usize) -> i32 {
    libpthread::pthread_cond_destroy(cond)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_cond_wait(
    cond: *mut usize,
    mutex: *mut usize,
) -> i32 {
    unsafe { libpthread::pthread_cond_wait(cond, mutex) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_cond_timedwait(
    cond: *mut usize,
    mutex: *mut usize,
    deadline: *const Timespec,
) -> i32 {
    unsafe { libpthread::pthread_cond_timedwait(cond, mutex, deadline) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_cond_signal(cond: *mut usize) -> i32 {
    unsafe { libpthread::pthread_cond_signal(cond) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_cond_broadcast(cond: *mut usize) -> i32 {
    unsafe { libpthread::pthread_cond_broadcast(cond) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_init(
    rwlock: *mut usize,
    attr: *const c_void,
) -> i32 {
    unsafe { libpthread::pthread_rwlock_init(rwlock, attr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_destroy(rwlock: *mut usize) -> i32 {
    unsafe { libpthread::pthread_rwlock_destroy(rwlock) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_rdlock(rwlock: *mut usize) -> i32 {
    unsafe { libpthread::pthread_rwlock_rdlock(rwlock) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_timedrdlock(
    rwlock: *mut usize,
    abstime: *const libpthread::Timespec,
) -> i32 {
    unsafe { libpthread::pthread_rwlock_timedrdlock(rwlock, abstime) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_tryrdlock(rwlock: *mut usize) -> i32 {
    unsafe { libpthread::pthread_rwlock_tryrdlock(rwlock) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_wrlock(rwlock: *mut usize) -> i32 {
    unsafe { libpthread::pthread_rwlock_wrlock(rwlock) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_timedwrlock(
    rwlock: *mut usize,
    abstime: *const libpthread::Timespec,
) -> i32 {
    unsafe { libpthread::pthread_rwlock_timedwrlock(rwlock, abstime) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_trywrlock(rwlock: *mut usize) -> i32 {
    unsafe { libpthread::pthread_rwlock_trywrlock(rwlock) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlock_unlock(rwlock: *mut usize) -> i32 {
    unsafe { libpthread::pthread_rwlock_unlock(rwlock) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_key_create(
    key: *mut u32,
    destructor: Option<kinakaze_tls::KeyDestructor>,
) -> i32 {
    unsafe { libpthread::pthread_key_create(key, destructor) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_key_delete(key: u32) -> i32 {
    libpthread::pthread_key_delete(key)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_getspecific(key: u32) -> *mut c_void {
    libpthread::pthread_getspecific(key)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_setspecific(key: u32, value: *mut c_void) -> i32 {
    libpthread::pthread_setspecific(key, value)
}

// C11 threads use the same per-thread key machinery as pthreads on glibc.
// Their status namespace differs, though: zero is `thrd_success` and a generic
// failure is `thrd_error` (2), rather than an errno value.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_tss_create(
    key: *mut u32,
    destructor: Option<kinakaze_tls::KeyDestructor>,
) -> i32 {
    if unsafe { libpthread::pthread_key_create(key, destructor) } == 0 {
        0
    } else {
        2
    }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tss_delete(key: u32) {
    let _ = libpthread::pthread_key_delete(key);
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tss_get(key: u32) -> *mut c_void {
    libpthread::pthread_getspecific(key)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_tss_set(key: u32, value: *mut c_void) -> i32 {
    if libpthread::pthread_setspecific(key, value) == 0 {
        0
    } else {
        2
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_init(attr: *mut PthreadAttr) -> i32 {
    unsafe { libpthread::pthread_attr_init(attr) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_attr_destroy(attr: *mut PthreadAttr) -> i32 {
    libpthread::pthread_attr_destroy(attr)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_setstacksize(
    attr: *mut PthreadAttr,
    stack_size: usize,
) -> i32 {
    unsafe { libpthread::pthread_attr_setstacksize(attr, stack_size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_setdetachstate(
    attr: *mut PthreadAttr,
    state: i32,
) -> i32 {
    unsafe { libpthread::pthread_attr_setdetachstate(attr, state) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_getstacksize(
    attr: *const PthreadAttr,
    stack_size: *mut usize,
) -> i32 {
    unsafe { libpthread::pthread_attr_getstacksize(attr, stack_size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_getdetachstate(
    attr: *const PthreadAttr,
    state: *mut i32,
) -> i32 {
    unsafe { libpthread::pthread_attr_getdetachstate(attr, state) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_setguardsize(
    attr: *mut PthreadAttr,
    guard_size: usize,
) -> i32 {
    unsafe { libpthread::pthread_attr_setguardsize(attr, guard_size) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_getguardsize(
    attr: *const PthreadAttr,
    guard_size: *mut usize,
) -> i32 {
    unsafe { libpthread::pthread_attr_getguardsize(attr, guard_size) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_cancel(thread: usize) -> i32 {
    libpthread::pthread_cancel(thread)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_testcancel() {
    libpthread::pthread_testcancel()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_setcancelstate(
    state: i32,
    previous: *mut i32,
) -> i32 {
    unsafe { libpthread::pthread_setcancelstate(state, previous) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_setcanceltype(
    kind: i32,
    previous: *mut i32,
) -> i32 {
    unsafe { libpthread::pthread_setcanceltype(kind, previous) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_kill(thread: usize, signal: i32) -> i32 {
    libpthread::pthread_kill(thread, signal)
}

#[repr(C)]
pub struct SemT {
    pub value: std::sync::atomic::AtomicI32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn WaitOnAddress(
        Address: *const c_void,
        CompareAddress: *const c_void,
        AddressSize: usize,
        dwMilliseconds: u32,
    ) -> i32;
    fn WakeByAddressSingle(Address: *const c_void);
    fn GetLastError() -> u32;
}

const INFINITE: u32 = 0xFFFFFFFF;

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_init(
    sem: *mut SemT,
    _pshared: i32,
    value: u32,
) -> i32 {
    if sem.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    unsafe {
        (*sem).value = std::sync::atomic::AtomicI32::new(value as i32);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_destroy(_sem: *mut SemT) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_post(sem: *mut SemT) -> i32 {
    if sem.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    unsafe {
        (*sem)
            .value
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        WakeByAddressSingle(sem as *const c_void);
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_wait(sem: *mut SemT) -> i32 {
    if sem.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    loop {
        let val = unsafe { (*sem).value.load(std::sync::atomic::Ordering::SeqCst) };
        if val > 0 {
            if unsafe {
                (*sem)
                    .value
                    .compare_exchange_weak(
                        val,
                        val - 1,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    )
                    .is_ok()
            } {
                return 0;
            }
        } else {
            let compare = 0i32;
            unsafe {
                WaitOnAddress(
                    sem as *const c_void,
                    &compare as *const i32 as *const c_void,
                    4,
                    INFINITE,
                );
            }
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_trywait(sem: *mut SemT) -> i32 {
    if sem.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    loop {
        let val = unsafe { (*sem).value.load(std::sync::atomic::Ordering::SeqCst) };
        if val <= 0 {
            crate::set_errno(kinakaze_vfs::EAGAIN);
            return -1;
        }
        if unsafe {
            (*sem)
                .value
                .compare_exchange_weak(
                    val,
                    val - 1,
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                )
                .is_ok()
        } {
            return 0;
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_timedwait(
    sem: *mut SemT,
    abs_timeout: *const Timespec,
) -> i32 {
    unsafe { sem_wait_until(sem, 0, abs_timeout) }
}

unsafe fn sem_wait_until(sem: *mut SemT, clock: i32, abs_timeout: *const Timespec) -> i32 {
    if sem.is_null() || abs_timeout.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let timeout = unsafe { *abs_timeout };
    if !matches!(clock, 0 | 1) || !(0..1_000_000_000).contains(&timeout.tv_nsec) {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let deadline = i128::from(timeout.tv_sec) * 1_000_000_000 + i128::from(timeout.tv_nsec);
    loop {
        let val = unsafe { (*sem).value.load(std::sync::atomic::Ordering::SeqCst) };
        if val > 0 {
            if unsafe {
                (*sem)
                    .value
                    .compare_exchange_weak(
                        val,
                        val - 1,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    )
                    .is_ok()
            } {
                return 0;
            }
        } else {
            // Use the same guest clock as clock_gettime: a monotonic deadline
            // is unrelated to the Unix epoch, and realtime may be namespaced.
            let mut now = crate::time::TimeSpec::default();
            if unsafe { crate::time::kinakaze_abi_clock_gettime(clock, &mut now) } != 0 {
                return -1;
            }
            let remaining =
                deadline - (i128::from(now.tv_sec) * 1_000_000_000 + i128::from(now.tv_nsec));
            if remaining <= 0 {
                crate::set_errno(kinakaze_vfs::ETIMEDOUT);
                return -1;
            }
            // Subtract before rounding so deadlines crossing a second do not
            // oversleep by almost a second. Never turn a finite wait into INFINITE.
            let timeout_ms =
                ((remaining + 999_999) / 1_000_000).min(i128::from(u32::MAX - 1)) as u32;
            let compare = 0i32;
            let ok = unsafe {
                WaitOnAddress(
                    sem as *const c_void,
                    &compare as *const i32 as *const c_void,
                    4,
                    timeout_ms,
                )
            };
            if ok == 0 && unsafe { GetLastError() } != 1460 {
                crate::set_errno(kinakaze_vfs::EINVAL);
                return -1;
            }
            // Both a wake and a kernel timeout require rechecking the token
            // and the clock; address waits may also wake spuriously.
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_getvalue(sem: *mut SemT, sval: *mut i32) -> i32 {
    if sem.is_null() || sval.is_null() {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    unsafe { *sval = (*sem).value.load(std::sync::atomic::Ordering::SeqCst) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_open(
    _name: *const core::ffi::c_char,
    _oflag: i32,
) -> *mut SemT {
    let sem = Box::into_raw(Box::new(SemT {
        value: std::sync::atomic::AtomicI32::new(1),
    }));
    sem
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_close(_sem: *mut SemT) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_unlink(_name: *const core::ffi::c_char) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_getattr_np(
    thread: usize,
    attr: *mut PthreadAttr,
) -> i32 {
    unsafe { libpthread::pthread_getattr_np(thread, attr) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_getstack(
    attr: *const PthreadAttr,
    stackaddr: *mut *mut c_void,
    stacksize: *mut usize,
) -> i32 {
    unsafe { libpthread::pthread_attr_getstack(attr, stackaddr, stacksize) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_attr_setscope(
    _attr: *mut PthreadAttr,
    _scope: i32,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_getaffinity_np(
    _thread: usize,
    cpusetsize: usize,
    cpuset: *mut u8,
) -> i32 {
    if cpuset.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    unsafe { std::ptr::write_bytes(cpuset, 0, cpusetsize) };
    let nprocs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let mut remaining = nprocs;
    let mut byte_idx = 0;
    while remaining > 0 && byte_idx < cpusetsize {
        let bits = remaining.min(8);
        let val = if bits == 8 { 0xff } else { (1 << bits) - 1 };
        unsafe { *cpuset.add(byte_idx) = val };
        remaining -= bits;
        byte_idx += 1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_setaffinity_np(
    _thread: usize,
    _cpusetsize: usize,
    _cpuset: *const u8,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_condattr_init(attr: *mut c_void) -> i32 {
    unsafe { libpthread::pthread_condattr_init(attr.cast::<PthreadCondAttr>()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_condattr_destroy(attr: *mut c_void) -> i32 {
    libpthread::pthread_condattr_destroy(attr.cast::<PthreadCondAttr>())
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_condattr_setclock(
    attr: *mut c_void,
    clock_id: i32,
) -> i32 {
    unsafe { libpthread::pthread_condattr_setclock(attr.cast::<PthreadCondAttr>(), clock_id) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_condattr_getclock(
    attr: *const c_void,
    clock_id: *mut i32,
) -> i32 {
    unsafe { libpthread::pthread_condattr_getclock(attr.cast::<PthreadCondAttr>(), clock_id) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_getcpuclockid(
    thread: usize,
    clock_id: *mut i32,
) -> i32 {
    if clock_id.is_null() {
        return 22;
    }
    match libpthread::sched::cpu_clock(thread) {
        Ok(clock) => {
            unsafe {
                *clock_id = clock;
            }
            0
        }
        Err(error) => error,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_exit(retval: *mut c_void) -> ! {
    libpthread::pthread_exit(retval);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___register_atfork(
    prepare: Option<unsafe extern "sysv64" fn()>,
    parent: Option<unsafe extern "sysv64" fn()>,
    child: Option<unsafe extern "sysv64" fn()>,
    _dso_handle: *mut c_void,
) -> i32 {
    libpthread::pthread_atfork(prepare, parent, child)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___cxa_thread_atexit_impl(
    func: Option<unsafe extern "sysv64" fn(*mut c_void)>,
    obj: *mut c_void,
    dso_symbol: *mut c_void,
) -> i32 {
    match kinakaze_tls::cxa_thread_atexit(func, obj, dso_symbol) {
        Ok(()) => 0,
        Err(error) => {
            kinakaze_tls::set_errno(error);
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sem_clockwait(
    sem: *mut SemT,
    clock_id: i32,
    abs_timeout: *const Timespec,
) -> i32 {
    unsafe { sem_wait_until(sem, clock_id, abs_timeout) }
}

#[repr(C)]
pub struct PthreadRwlockAttr {
    pub pshared: i32,
    pub kind: i32,
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_condattr_setpshared(
    attr: *mut c_void,
    pshared: i32,
) -> i32 {
    libpthread::pthread_condattr_setpshared(attr.cast::<PthreadCondAttr>(), pshared)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_mutexattr_setpshared(
    attr: *mut c_void,
    pshared: i32,
) -> i32 {
    if attr.is_null() || !matches!(pshared, 0 | 1) {
        kinakaze_vfs::EINVAL
    } else if pshared == 1 {
        // The mutex implementation parks on process-local Windows waits.
        // Like condattr_setpshared, do not promise interprocess wakeups.
        95 // ENOTSUP
    } else {
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlockattr_init(
    attr: *mut PthreadRwlockAttr,
) -> i32 {
    if !attr.is_null() {
        unsafe {
            (*attr).pshared = 0;
            (*attr).kind = 0;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlockattr_destroy(
    _attr: *mut PthreadRwlockAttr,
) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlockattr_setpshared(
    attr: *mut PthreadRwlockAttr,
    value: i32,
) -> i32 {
    if attr.is_null() || !matches!(value, 0 | 1) {
        return kinakaze_vfs::EINVAL;
    }
    unsafe {
        (*attr).pshared = value;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlockattr_getpshared(
    attr: *const PthreadRwlockAttr,
    value: *mut i32,
) -> i32 {
    if attr.is_null() || value.is_null() {
        return kinakaze_vfs::EINVAL;
    }
    unsafe {
        *value = (*attr).pshared;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_rwlockattr_setkind_np(
    attr: *mut PthreadRwlockAttr,
    pref: i32,
) -> i32 {
    if !attr.is_null() {
        unsafe {
            (*attr).kind = pref;
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_getconcurrency() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_getname_np(
    thread: usize,
    name: *mut core::ffi::c_char,
    length: usize,
) -> i32 {
    unsafe { libpthread::pthread_getname_np(thread, name, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_pthread_setname_np(
    thread: usize,
    name: *const core::ffi::c_char,
) -> i32 {
    unsafe { libpthread::pthread_setname_np(thread, name) }
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_pthread_setconcurrency(_new_level: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___pthread_key_create(
    key: *mut u32,
    dtor: Option<unsafe extern "sysv64" fn(*mut c_void)>,
) -> i32 {
    unsafe { kinakaze_abi_pthread_key_create(key, dtor) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_semaphores_lifecycle_and_blocking_sync() {
        let mut sem = SemT {
            value: std::sync::atomic::AtomicI32::new(0),
        };
        assert_eq!(unsafe { kinakaze_abi_sem_init(&mut sem, 0, 0) }, 0);

        let mut val = -1;
        assert_eq!(unsafe { kinakaze_abi_sem_getvalue(&mut sem, &mut val) }, 0);
        assert_eq!(val, 0);

        // Trywait on 0 returns -1 and EAGAIN
        assert_eq!(unsafe { kinakaze_abi_sem_trywait(&mut sem) }, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EAGAIN);

        // Post increases value to 1
        assert_eq!(unsafe { kinakaze_abi_sem_post(&mut sem) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sem_getvalue(&mut sem, &mut val) }, 0);
        assert_eq!(val, 1);

        // Wait consumes value back to 0
        assert_eq!(unsafe { kinakaze_abi_sem_wait(&mut sem) }, 0);
        assert_eq!(unsafe { kinakaze_abi_sem_getvalue(&mut sem, &mut val) }, 0);
        assert_eq!(val, 0);

        // Past timeout returns ETIMEDOUT (110)
        let past = Timespec {
            tv_sec: 1000,
            tv_nsec: 0,
        };
        assert_eq!(unsafe { kinakaze_abi_sem_timedwait(&mut sem, &past) }, -1);
        assert_eq!(kinakaze_tls::errno(), 110);

        // Destroy
        assert_eq!(unsafe { kinakaze_abi_sem_destroy(&mut sem) }, 0);
    }

    #[test]
    fn pthread_getattr_np_reflection() {
        let mut attr: PthreadAttr = unsafe { core::mem::zeroed() };
        assert_eq!(unsafe { kinakaze_abi_pthread_getattr_np(0, &mut attr) }, 0);
    }
}

/// C11 integer exit status uses the common pthread TLS cleanup and join state.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_thrd_exit(result: i32) -> ! {
    libpthread::pthread_exit(result as isize as *mut c_void)
}
