//! Public C buffers follow the executable's allocator interposition.
//! Raw libc malloc/free/realloc entry points deliberately keep using `guest`:
//! an interposer may call them through RTLD_NEXT without recursing here.
use core::sync::atomic::{AtomicUsize, Ordering};

static MALLOC: AtomicUsize = AtomicUsize::new(0);
static REALLOC: AtomicUsize = AtomicUsize::new(0);
static FREE: AtomicUsize = AtomicUsize::new(0);

/// Publish once after relocation, before constructors; restore before fork
/// child handlers. Entries remain mapped until guest teardown completes.
/// Zero selects the underlying allocator. No lookup or lock on the hot path.
///
/// # Safety
/// Nonzero entries must have the corresponding Linux C allocator ABI. There
/// must be no concurrent guest execution while replacing this allocator set.
pub unsafe fn install(malloc: usize, realloc: usize, free: usize) {
    MALLOC.store(malloc, Ordering::Release);
    REALLOC.store(realloc, Ordering::Release);
    FREE.store(free, Ordering::Release);
}

/// # Safety
/// The caller must eventually release the result using the same C allocator.
pub unsafe fn malloc(size: usize) -> *mut u8 {
    let entry = MALLOC.load(Ordering::Acquire);
    if entry == 0 {
        return unsafe { super::guest::malloc(size) };
    }
    let call: unsafe extern "sysv64" fn(usize) -> *mut u8 = unsafe { core::mem::transmute(entry) };
    unsafe { call(size) }
}

/// # Safety
/// `pointer` must be null or a live allocation from the active C allocator.
pub unsafe fn realloc(pointer: *mut u8, size: usize) -> *mut u8 {
    let entry = REALLOC.load(Ordering::Acquire);
    if entry == 0 {
        return unsafe { super::guest::reallocate(pointer, 16, size) };
    }
    let call: unsafe extern "sysv64" fn(*mut u8, usize) -> *mut u8 =
        unsafe { core::mem::transmute(entry) };
    unsafe { call(pointer, size) }
}

/// # Safety
/// `pointer` must be null or a live allocation from the active C allocator.
pub unsafe fn free(pointer: *mut u8) {
    if pointer.is_null() {
        return;
    }
    let entry = FREE.load(Ordering::Acquire);
    if entry == 0 {
        unsafe { super::guest::free(pointer) };
        return;
    }
    let call: unsafe extern "sysv64" fn(*mut u8) = unsafe { core::mem::transmute(entry) };
    unsafe { call(pointer) };
}
