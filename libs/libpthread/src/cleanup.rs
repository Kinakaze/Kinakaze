//! GNU C cancellation cleanup frames. The saved register prefix is the same
//! unmangled SysV jump buffer written by libc::jump, not a Windows CONTEXT.
//! This does not replace DWARF forced unwinding for C++ automatic objects.
use super::*;
use core::arch::naked_asm;

#[repr(C)]
pub struct UnwindBuffer {
    registers: [u64; 8],
    mask_saved: i32,
    padding: i32,
    private: [usize; 4],
}

thread_local! {
    static HEAD: Cell<*mut UnwindBuffer> = const { Cell::new(core::ptr::null_mut()) };
    static EXIT_RESULT: Cell<Option<usize>> = const { Cell::new(None) };
}

pub(super) fn snapshot() -> [u64; 3] {
    let result = EXIT_RESULT.with(Cell::get);
    [
        HEAD.with(Cell::get) as u64,
        u64::from(result.is_some()),
        result.unwrap_or(0) as u64,
    ]
}

pub(super) fn restore_snapshot(state: [u64; 3]) {
    HEAD.with(|slot| slot.set(state[0] as *mut UnwindBuffer));
    EXIT_RESULT.with(|slot| slot.set((state[1] != 0).then_some(state[2] as usize)));
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __pthread_register_cancel(buffer: *mut UnwindBuffer) {
    let previous = HEAD.with(|head| head.replace(buffer));
    unsafe {
        (*buffer).private[0] = previous as usize;
        (*buffer).private[1] = 0;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __pthread_unregister_cancel(buffer: *mut UnwindBuffer) {
    HEAD.with(|head| head.set(unsafe { (*buffer).private[0] } as *mut UnwindBuffer));
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __pthread_register_cancel_defer(buffer: *mut UnwindBuffer) {
    unsafe { __pthread_register_cancel(buffer) };
    // This runtime currently provides deferred cancellation for both types.
    unsafe { (*buffer).private[2] = PTHREAD_CANCEL_DEFERRED as usize };
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __pthread_unregister_cancel_restore(buffer: *mut UnwindBuffer) {
    unsafe { __pthread_unregister_cancel(buffer) };
    // Restoring the deferred type is not itself a cancellation point. Pending
    // cancellation must wait until the caller reaches its next explicit point.
}

pub(super) fn exit(result: usize) -> ! {
    let state = cancel_self();
    state.enabled.store(false, Ordering::Release);
    drop(state);
    EXIT_RESULT.with(|slot| {
        if slot.get().is_none() {
            slot.set(Some(result));
        }
    });
    advance()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn __pthread_unwind_next(buffer: *mut UnwindBuffer) -> ! {
    unsafe { __pthread_unregister_cancel(buffer) };
    advance()
}

fn advance() -> ! {
    // Release the TLS borrow and all native ownership before changing stacks.
    let buffer = HEAD.with(|slot| {
        let current = slot.get();
        if !current.is_null() {
            slot.set(unsafe { (*current).private[0] } as *mut UnwindBuffer);
        }
        current
    });
    if !buffer.is_null() {
        unsafe { resume_cleanup(buffer) }
    }
    let result = EXIT_RESULT.with(|slot| slot.get().unwrap_or(PTHREAD_CANCELED));
    finish_current_thread(result);
    unsafe { ExitThread(0) }
}

pub(super) fn last_thread_exit() -> ! {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows_sys::Win32::System::Threading::{ExitProcess, GetCurrentProcess, TerminateProcess};
    // POSIX last-pthread exit behaves as exit(0), including C exit handlers.
    // Resolve only on final teardown to avoid a libc <-> pthread link cycle.
    let libc = unsafe { GetModuleHandleA(c"libc.so.6".as_ptr().cast()) };
    if !libc.is_null()
        && let Some(symbol) = unsafe { GetProcAddress(libc, c"kinakaze_abi_exit".as_ptr().cast()) }
    {
        let exit: unsafe extern "sysv64" fn(i32) -> ! = unsafe { core::mem::transmute(symbol) };
        unsafe { exit(0) }
    }
    kinakaze_vfs::job::publish_exit(0);
    unsafe {
        TerminateProcess(GetCurrentProcess(), 0);
        ExitProcess(0)
    }
}

#[unsafe(naked)]
unsafe extern "sysv64" fn resume_cleanup(buffer: *const UnwindBuffer) -> ! {
    naked_asm!(
        "mov rbx, [rdi]",
        "mov rbp, [rdi + 8]",
        "mov r12, [rdi + 16]",
        "mov r13, [rdi + 24]",
        "mov r14, [rdi + 32]",
        "mov r15, [rdi + 40]",
        "mov rdx, [rdi + 56]",
        "mov rsp, [rdi + 48]",
        "mov eax, 1",
        "jmp rdx",
    )
}
