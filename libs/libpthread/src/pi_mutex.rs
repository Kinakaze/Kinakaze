//! libc owns the futex PI engine; this callback avoids a reverse dependency.
use super::*;
pub(super) const ATTR_MASK: i32 = 3 << 28;
pub(super) const ATTR_INHERIT: i32 = 1 << 28;
pub(super) const OBJECT_INHERIT: i32 = 32;
pub type Backend = unsafe extern "sysv64" fn(u32, *mut usize, i32, *const Timespec, i32) -> i32;
static BACKEND: AtomicUsize = AtomicUsize::new(0);
pub(super) fn install(backend: Backend) {
    BACKEND.store(backend as usize, Ordering::Release);
}
pub(super) unsafe fn is_mutex(mutex: *mut usize) -> bool {
    (unsafe { mutex.cast::<u8>().add(16).cast::<i32>().read() }) & OBJECT_INHERIT != 0
}
pub(super) unsafe fn call(
    op: u32,
    mutex: *mut usize,
    clock: i32,
    deadline: *const Timespec,
    flags: i32,
) -> i32 {
    let value = BACKEND.load(Ordering::Acquire);
    if value == 0 {
        return 95;
    }
    let backend: Backend = unsafe { core::mem::transmute(value) };
    unsafe { backend(op, mutex, clock, deadline, flags) }
}
