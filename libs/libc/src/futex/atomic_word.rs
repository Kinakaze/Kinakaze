//! One faultable instruction per user-word operation. The exception handler
//! recognizes only these two instruction addresses and resumes at their error
//! return; guest exceptions elsewhere continue through the normal handlers.

use super::*;
use kinakaze_vfs::EFAULT;
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};

core::arch::global_asm!(
    ".text",
    ".p2align 4",
    ".globl kinakaze_futex_cmpxchg",
    "kinakaze_futex_cmpxchg:",
    "mov eax, edx",
    ".globl kinakaze_futex_cmpxchg_instruction",
    "kinakaze_futex_cmpxchg_instruction:",
    "lock cmpxchg dword ptr [rcx], r8d",
    "ret",
    ".globl kinakaze_futex_atomic_fault",
    "kinakaze_futex_atomic_fault:",
    "mov rax, -14",
    "ret",
);

unsafe extern "C" {
    fn kinakaze_futex_cmpxchg(address: *mut u32, old: u32, new: u32) -> i64;
    fn kinakaze_futex_cmpxchg_instruction();
    fn kinakaze_futex_atomic_fault();
}

unsafe extern "system" fn fault(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return 0;
    }
    let info = unsafe { &*info };
    let (exception, context) = unsafe { (&*info.ExceptionRecord, &mut *info.ContextRecord) };
    if matches!(exception.ExceptionCode as u32, 0xc000_0005 | 0x8000_0001)
        && context.Rip == kinakaze_futex_cmpxchg_instruction as *const () as u64
    {
        context.Rip = kinakaze_futex_atomic_fault as *const () as u64;
        return -1; // EXCEPTION_CONTINUE_EXECUTION
    }
    0 // EXCEPTION_CONTINUE_SEARCH
}

// The process incarnation is part of the state: a fork-restored initializer
// must install a handler in the new host process, without an inherited mutex.
static INSTALLED: AtomicU64 = AtomicU64::new(0);
fn install() -> Result<(), i32> {
    let owner = u64::from(std::process::id()) << 32;
    loop {
        let state = INSTALLED.load(Ordering::Acquire);
        if state == owner | 2 {
            return Ok(());
        }
        if state == owner | 1 {
            std::thread::yield_now();
            continue;
        }
        if INSTALLED
            .compare_exchange(state, owner | 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            continue;
        }
        if unsafe { AddVectoredExceptionHandler(1, Some(fault)) }.is_null() {
            INSTALLED.store(0, Ordering::Release);
            return Err(EIO);
        }
        INSTALLED.store(owner | 2, Ordering::Release);
        return Ok(());
    }
}

pub(super) fn read(address: usize) -> Result<u32, i32> {
    crate::ptrace::read_value(address)
}

pub(super) fn compare_exchange(address: usize, old: u32, new: u32) -> Result<u32, i32> {
    install()?;
    // Validate writable protection before issuing an instruction (in particular
    // avoid consuming PAGE_GUARD). A subsequent unmap/protection race is caught
    // at the exact instruction, without exposing an unchecked Rust reference.
    crate::sysadmin::futex_access(address, 4, true).map_err(|error| -error as i32)?;
    let value = unsafe { kinakaze_futex_cmpxchg(address as _, old, new) };
    if value < 0 {
        Err(EFAULT)
    } else {
        Ok(value as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_word_atomic_fault_is_contained() {
        install().unwrap();
        assert_eq!(unsafe { kinakaze_futex_cmpxchg(1 as _, 0, 1) }, -14);
        let word = AtomicU32::new(7);
        assert_eq!(compare_exchange(word.as_ptr() as usize, 6, 9), Ok(7));
        assert_eq!(word.load(Ordering::Acquire), 7);
        assert_eq!(compare_exchange(word.as_ptr() as usize, 7, 9), Ok(7));
        assert_eq!(word.load(Ordering::Acquire), 9);
    }
}
