//! Xlibint exposes function-pointer DATA, not functions with those names.
//! Native critical sections are initialized afresh when the module loads in a
//! fork child; neither a host thread owner nor a wait queue is copied.
use core::{cell::UnsafeCell, ffi::c_void};
use std::sync::Once;
use windows_sys::Win32::System::Threading::{
    CRITICAL_SECTION, EnterCriticalSection, InitializeCriticalSection, LeaveCriticalSection,
};
struct NativeLock(UnsafeCell<CRITICAL_SECTION>);
unsafe impl Sync for NativeLock {}
static GLOBAL: NativeLock = NativeLock(UnsafeCell::new(unsafe { core::mem::zeroed() }));
static INITIALIZED: Once = Once::new();
fn global() -> *mut CRITICAL_SECTION {
    INITIALIZED.call_once(|| unsafe {
        InitializeCriticalSection(GLOBAL.0.get());
    });
    GLOBAL.0.get()
}
#[unsafe(export_name = "kinakaze_engine_libX11__Xglobal_lock")]
pub static mut _Xglobal_lock: *mut c_void = GLOBAL.0.get().cast();
#[unsafe(export_name = "kinakaze_engine_libX11__XLockMutex_fn")]
pub static mut _XLockMutex_fn: Option<unsafe extern "sysv64" fn(*mut c_void)> = Some(lock);
#[unsafe(export_name = "kinakaze_engine_libX11__XUnlockMutex_fn")]
pub static mut _XUnlockMutex_fn: Option<unsafe extern "sysv64" fn(*mut c_void)> = Some(unlock);
unsafe extern "sysv64" fn lock(mutex: *mut c_void) {
    let global = global();
    unsafe {
        EnterCriticalSection(if mutex.is_null() {
            global
        } else {
            mutex.cast()
        });
    }
}
unsafe extern "sysv64" fn unlock(mutex: *mut c_void) {
    let global = global();
    unsafe {
        LeaveCriticalSection(if mutex.is_null() {
            global
        } else {
            mutex.cast()
        });
    }
}
