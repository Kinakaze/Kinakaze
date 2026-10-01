//! One faultable instruction per user-word operation. The exception handler
//! recognizes only these instruction addresses and resumes at their error
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
    ".p2align 4",
    ".globl kinakaze_futex_exchange",
    "kinakaze_futex_exchange:",
    ".globl kinakaze_futex_exchange_instruction",
    "kinakaze_futex_exchange_instruction:",
    // A memory XCHG is implicitly locked. No old-value read or retry is needed.
    "xchg dword ptr [rcx], edx",
    "xor eax, eax",
    "ret",
    ".globl kinakaze_futex_atomic_fault",
    "kinakaze_futex_atomic_fault:",
    "mov rax, -14",
    "ret",
);

unsafe extern "C" {
    fn kinakaze_futex_cmpxchg(address: *mut u32, old: u32, new: u32) -> i64;
    fn kinakaze_futex_cmpxchg_instruction();
    fn kinakaze_futex_exchange(address: *mut u32, value: u32) -> i64;
    fn kinakaze_futex_exchange_instruction();
    fn kinakaze_futex_atomic_fault();
}

unsafe extern "system" fn fault(info: *mut EXCEPTION_POINTERS) -> i32 {
    if info.is_null() {
        return 0;
    }
    let info = unsafe { &*info };
    let (exception, context) = unsafe { (&*info.ExceptionRecord, &mut *info.ContextRecord) };
    if matches!(exception.ExceptionCode as u32, 0xc000_0005 | 0x8000_0001)
        && (context.Rip == kinakaze_futex_cmpxchg_instruction as *const () as u64
            || context.Rip == kinakaze_futex_exchange_instruction as *const () as u64)
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

fn prepare_write(address: usize) -> Result<(), i32> {
    install()?;
    // Validate writable protection before issuing an instruction (in particular
    // avoid consuming PAGE_GUARD). A subsequent unmap/protection race is caught
    // at the exact instruction, without exposing an unchecked Rust reference.
    crate::sysadmin::futex_access(address, 4, true).map_err(|error| -error as i32)?;
    Ok(())
}

pub(super) fn write(address: usize, value: u32) -> Result<(), i32> {
    prepare_write(address)?;
    if unsafe { kinakaze_futex_exchange(address as _, value) } < 0 {
        Err(EFAULT)
    } else {
        Ok(())
    }
}

pub(super) fn compare_exchange(address: usize, old: u32, new: u32) -> Result<u32, i32> {
    prepare_write(address)?;
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

    #[test]
    fn atomic_write_contains_faults_and_preserves_guard_and_readonly_pages() {
        use windows_sys::Win32::System::Memory::*;
        install().unwrap();
        assert_eq!(unsafe { kinakaze_futex_exchange(1 as _, 7) }, -14);
        struct Page(*mut core::ffi::c_void);
        impl Drop for Page {
            fn drop(&mut self) {
                unsafe { VirtualFree(self.0, 0, MEM_RELEASE) };
            }
        }
        let page = Page(unsafe {
            VirtualAlloc(ptr::null(), 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE)
        });
        assert!(!page.0.is_null());
        let address = page.0 as usize;
        for value in [0, u32::MAX, 7] {
            assert_eq!(write(address, value), Ok(()));
            assert_eq!(read(address), Ok(value));
            assert_eq!(read(address + 4), Ok(0));
        }
        for protection in [PAGE_READONLY, PAGE_NOACCESS, PAGE_READWRITE | PAGE_GUARD] {
            let mut old = 0;
            assert_ne!(
                unsafe { VirtualProtect(page.0, 4096, protection, &mut old) },
                0
            );
            assert_eq!(write(address, 9), Err(EFAULT));
            let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
            assert_ne!(
                unsafe { VirtualQuery(page.0, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) },
                0
            );
            assert_eq!(info.Protect, protection);
            if protection & PAGE_GUARD == 0 {
                // Simulate protection changing after validation: only the
                // assembly instruction is faulted, with no Rust dereference.
                assert_eq!(unsafe { kinakaze_futex_exchange(page.0.cast(), 9) }, -14);
            }
            assert_ne!(
                unsafe { VirtualProtect(page.0, 4096, PAGE_READWRITE, &mut old) },
                0
            );
            assert_eq!(read(address), Ok(7));
        }
    }
}
