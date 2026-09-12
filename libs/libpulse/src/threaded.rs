//! A thread owns dispatch; callers and callbacks share Pulse's recursive lock.
use crate::{lock, loop_lock::LoopLock, mainloop::*, mainloop_apis};
use core::ffi::{c_int, c_void};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

pub struct pa_threaded_mainloop {
    sync: Arc<LoopLock>,
    mainloop: usize,
    stopped: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_new")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_new() -> *mut pa_threaded_mainloop {
    let mainloop = unsafe { pa_mainloop_new() };
    if mainloop.is_null() {
        return core::ptr::null_mut();
    }
    let value = Box::new(pa_threaded_mainloop {
        sync: Arc::new(LoopLock::default()),
        mainloop: mainloop as usize,
        stopped: AtomicBool::new(true),
        worker: Mutex::new(None),
    });
    let api = unsafe { pa_mainloop_get_api(mainloop) };
    lock(mainloop_apis()).insert(api as usize, Arc::clone(&value.sync));
    Box::into_raw(value)
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_free")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_free(pointer: *mut pa_threaded_mainloop) {
    if pointer.is_null() {
        return;
    }
    unsafe {
        pa_threaded_mainloop_stop(pointer);
    }
    let value = unsafe { Box::from_raw(pointer) };
    let mainloop = value.mainloop as *mut pa_mainloop;
    lock(mainloop_apis()).remove(&(unsafe { pa_mainloop_get_api(mainloop) } as usize));
    unsafe {
        pa_mainloop_free(mainloop);
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_get_api")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_get_api(
    pointer: *mut pa_threaded_mainloop,
) -> *mut c_void {
    unsafe { pa_mainloop_get_api((*pointer).mainloop as *mut pa_mainloop).cast() }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_start")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_start(
    pointer: *mut pa_threaded_mainloop,
) -> c_int {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return -1;
    };
    let mut worker = lock(&loop_.worker);
    if worker.is_some() {
        return -1;
    }
    loop_.stopped.store(false, Ordering::Release);
    loop_.sync.set_running(true);
    let address = pointer as usize;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("pulse-mainloop".into())
        .spawn(move || {
            if kinakaze_tls::initialize_thread_tls().is_err() {
                let _ = send.send(false);
                return;
            }
            let _ = send.send(true);
            let loop_ = unsafe { &*(address as *const pa_threaded_mainloop) };
            let mainloop = loop_.mainloop as *mut pa_mainloop;
            loop {
                let Some(guard) = loop_.sync.callback(&loop_.stopped) else {
                    break;
                };
                let prepared = unsafe { pa_mainloop_prepare(mainloop, -1) };
                drop(guard);
                if prepared < 0 {
                    break;
                }
                if unsafe { pa_mainloop_poll(mainloop) } < 0 {
                    break;
                }
                let Some(guard) = loop_.sync.callback(&loop_.stopped) else {
                    break;
                };
                let result = unsafe { pa_mainloop_dispatch(mainloop) };
                drop(guard);
                if result < 0 {
                    break;
                }
            }
            loop_.sync.set_running(false);
        });
    let Ok(thread) = thread else {
        loop_.stopped.store(true, Ordering::Release);
        loop_.sync.set_running(false);
        return -1;
    };
    if receive.recv() != Ok(true) {
        let _ = thread.join();
        loop_.stopped.store(true, Ordering::Release);
        loop_.sync.set_running(false);
        return -1;
    }
    *worker = Some(thread);
    0
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_stop")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_stop(pointer: *mut pa_threaded_mainloop) {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return;
    };
    loop_.stopped.store(true, Ordering::Release);
    loop_.sync.wake();
    unsafe {
        pa_mainloop_wakeup(loop_.mainloop as *mut pa_mainloop);
    }
    loop_.sync.set_running(false);
    let thread = lock(&loop_.worker).take();
    if let Some(thread) = thread {
        let _ = thread.join();
    }
    // Stop can interrupt poll before dispatch. A subsequent start must begin
    // a fresh iteration rather than inheriting the old Polled phase.
    unsafe {
        (&*(loop_.mainloop as *const pa_mainloop)).reset_iteration();
    }
}

#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_in_thread")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_in_thread(
    pointer: *mut pa_threaded_mainloop,
) -> c_int {
    let Some(loop_) = (unsafe { pointer.as_ref() }) else {
        return 0;
    };
    lock(&loop_.worker)
        .as_ref()
        .is_some_and(|thread| thread.thread().id() == std::thread::current().id()) as c_int
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_get_retval")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_get_retval(
    pointer: *mut pa_threaded_mainloop,
) -> c_int {
    unsafe { pa_mainloop_get_retval((*pointer).mainloop as *mut pa_mainloop) }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_lock")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_lock(pointer: *mut pa_threaded_mainloop) {
    unsafe {
        (*pointer).sync.lock();
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_unlock")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_unlock(pointer: *mut pa_threaded_mainloop) {
    unsafe {
        (*pointer).sync.unlock();
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_signal")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_signal(
    pointer: *mut pa_threaded_mainloop,
    accept: c_int,
) {
    unsafe {
        (*pointer).sync.signal(accept != 0);
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_wait")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_wait(pointer: *mut pa_threaded_mainloop) {
    unsafe {
        (*pointer).sync.wait();
    }
}
#[unsafe(export_name = "kinakaze_engine_libpulse_pa_threaded_mainloop_accept")]
pub unsafe extern "sysv64" fn pa_threaded_mainloop_accept(pointer: *mut pa_threaded_mainloop) {
    unsafe {
        (*pointer).sync.accept(false);
    }
}
