use core::ffi::c_void;
use core::{mem::MaybeUninit, ptr};
use libpthread::{PthreadMutexAttr, Timespec};
use std::alloc::{Layout, alloc, dealloc};

fn status(error: i32) -> i32 {
    match error {
        0 => 0,
        16 => 1,
        12 => 3,
        110 => 4,
        _ => 2,
    }
}

struct Start {
    callback: unsafe extern "sysv64" fn(*mut c_void) -> i32,
    argument: *mut c_void,
}

unsafe extern "sysv64" fn start_thread(packet: *mut c_void) -> *mut c_void {
    let start = unsafe { packet.cast::<Start>().read() };
    unsafe { dealloc(packet.cast(), Layout::new::<Start>()) };
    unsafe { (start.callback)(start.argument) as isize as *mut c_void }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_thrd_create(
    thread: *mut usize,
    callback: Option<unsafe extern "sysv64" fn(*mut c_void) -> i32>,
    argument: *mut c_void,
) -> i32 {
    let Some(callback) = callback.filter(|_| !thread.is_null()) else {
        return 2;
    };
    let packet = unsafe { alloc(Layout::new::<Start>()) }.cast::<Start>();
    if packet.is_null() {
        return 3;
    }
    unsafe { packet.write(Start { callback, argument }) };
    let error = unsafe {
        libpthread::pthread_create(thread, ptr::null(), Some(start_thread), packet.cast())
    };
    if error != 0 {
        unsafe { dealloc(packet.cast(), Layout::new::<Start>()) };
    }
    status(error)
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_thrd_join(thread: usize, result: *mut i32) -> i32 {
    let mut value = ptr::null_mut();
    let error = unsafe { libpthread::pthread_join(thread, &mut value) };
    if error == 0 && !result.is_null() {
        unsafe { result.write(value as isize as i32) };
    }
    status(error)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_thrd_current() -> usize {
    libpthread::pthread_self()
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_thrd_equal(left: usize, right: usize) -> i32 {
    libpthread::pthread_equal(left, right)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_thrd_detach(thread: usize) -> i32 {
    status(libpthread::pthread_detach(thread))
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_call_once(
    control: *mut u32,
    callback: Option<unsafe extern "sysv64" fn()>,
) {
    unsafe { libpthread::pthread_once(control, callback) };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mtx_init(mutex: *mut usize, kind: i32) -> i32 {
    if kind & !3 != 0 {
        return 2;
    }
    let mut attr = MaybeUninit::<PthreadMutexAttr>::uninit();
    let error = unsafe { libpthread::pthread_mutexattr_init(attr.as_mut_ptr()) };
    if error != 0 {
        return status(error);
    }
    let error = unsafe { libpthread::pthread_mutexattr_settype(attr.as_mut_ptr(), kind & 1) };
    let error = if error == 0 {
        unsafe { libpthread::pthread_mutex_init(mutex, attr.as_ptr()) }
    } else {
        error
    };
    libpthread::pthread_mutexattr_destroy(attr.as_mut_ptr());
    status(error)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_mtx_destroy(mutex: *mut usize) {
    libpthread::pthread_mutex_destroy(mutex);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mtx_lock(mutex: *mut usize) -> i32 {
    status(unsafe { libpthread::pthread_mutex_lock(mutex) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mtx_trylock(mutex: *mut usize) -> i32 {
    status(unsafe { libpthread::pthread_mutex_trylock(mutex) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mtx_timedlock(
    mutex: *mut usize,
    deadline: *const Timespec,
) -> i32 {
    status(unsafe { libpthread::pthread_mutex_timedlock(mutex, deadline) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_mtx_unlock(mutex: *mut usize) -> i32 {
    status(unsafe { libpthread::pthread_mutex_unlock(mutex) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cnd_init(condition: *mut usize) -> i32 {
    status(unsafe { libpthread::pthread_cond_init(condition, ptr::null()) })
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_cnd_destroy(condition: *mut usize) {
    libpthread::pthread_cond_destroy(condition);
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cnd_wait(
    condition: *mut usize,
    mutex: *mut usize,
) -> i32 {
    status(unsafe { libpthread::pthread_cond_wait(condition, mutex) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cnd_timedwait(
    condition: *mut usize,
    mutex: *mut usize,
    deadline: *const Timespec,
) -> i32 {
    status(unsafe { libpthread::pthread_cond_timedwait(condition, mutex, deadline) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cnd_signal(condition: *mut usize) -> i32 {
    status(unsafe { libpthread::pthread_cond_signal(condition) })
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cnd_broadcast(condition: *mut usize) -> i32 {
    status(unsafe { libpthread::pthread_cond_broadcast(condition) })
}
