//! Entering a guest program the way the kernel would.
//!
//! A freestanding probe can be called like a function, but a real `crt1.o` cannot:
//! its `_start` takes no arguments and instead reads `argc`, `argv` and `envp`
//! straight off the stack, then calls `__libc_start_main`. So starting a real
//! binary means placing the synthesized stack block at a known address, pointing
//! `rsp` at it, and jumping — which is what this module does.

use crate::LinkError;
use crate::stack::StackImage;

/// How much room the guest's stack gets to grow into.
///
/// This is the whole reason the block is not simply allocated at its own size: the
/// stack grows *downward* from the entry pointer, so an allocation sized to the
/// argv/envp/auxv block alone leaves the guest with nothing beneath `rsp` and the
/// first function call walks straight off the bottom of the reservation. 8 MiB
/// matches the usual Linux `RLIMIT_STACK` default.
const STACK_REGION: usize = 8 * 1024 * 1024;

/// A stack region with the initial argv/envp/auxv block placed at its top.
///
/// The region has to outlive the guest, so it is leaked deliberately: the guest
/// never returns from `_start`, and `argv`/`envp` pointers into a freed buffer
/// would be a use-after-free the moment `main` touched them.
pub struct PlacedStack {
    region: usize,
    base: usize,
    stack_pointer: usize,
}

impl PlacedStack {
    /// Reserves a stack region and places the image at the high end of it.
    ///
    /// The image goes at the top rather than the bottom because everything below
    /// `rsp` must be free for the guest's own frames.
    pub fn place(mut image: StackImage) -> Result<Self, LinkError> {
        let len = image.bytes.len().max(1);
        if len >= STACK_REGION {
            return Err(LinkError::MappingFailed {
                object: "<initial stack>".to_owned(),
                len,
            });
        }
        let region = allocate_aligned(STACK_REGION)?;

        // Seat the block against the top of the region, then align the base down.
        // Aligning down keeps it inside the reservation while giving the stack
        // pointer the 16-byte alignment the ABI requires, since the image's own
        // offsets are relative to this base.
        let base = (region + STACK_REGION - len) & !0xf;
        if base < region {
            return Err(LinkError::AddressOverflow);
        }

        // SAFETY: `relocate_to` is called exactly once, here, before the block is
        // handed to the guest. Calling it twice would double-add the base.
        unsafe { image.relocate_to(base) };

        // SAFETY: `base` is inside the reservation with at least `len` bytes above
        // it, and the source is a distinct heap buffer.
        unsafe {
            std::ptr::copy_nonoverlapping(image.bytes.as_ptr(), base as *mut u8, image.bytes.len())
        };

        let stack_pointer = image.stack_pointer(base);
        if !stack_pointer.is_multiple_of(16) {
            // The ABI violation would otherwise surface much later as a misaligned
            // SSE access deep inside the guest, so it is caught here.
            return Err(LinkError::AddressOverflow);
        }
        Ok(Self {
            region,
            base,
            stack_pointer,
        })
    }

    /// Lowest address the guest's stack may grow to.
    pub fn limit(&self) -> usize {
        self.region
    }

    pub fn stack_pointer(&self) -> usize {
        self.stack_pointer
    }

    pub fn base(&self) -> usize {
        self.base
    }

    /// Points the thread's TEB stack bounds at this region.
    ///
    /// Windows records the current thread's stack in its TEB — `StackBase` at
    /// `gs:0x08` and `StackLimit` at `gs:0x10` — and those fields are not merely
    /// informational. `RtlDispatchException` validates a frame's stack pointer
    /// against them while walking the unwind chain, and rejects one that falls
    /// outside. So does `RtlUnwindEx`, and so does anything else that walks the
    /// stack.
    ///
    /// Guest code does not run on the Windows thread stack. It runs here, on a
    /// `VirtualAlloc` block, which means every address the guest's `rsp` takes is
    /// outside the bounds Windows recorded. Nothing notices until something throws:
    /// a Vulkan driver raising and catching its own C++ exception is dispatched
    /// against those bounds, the frames are judged invalid, and the process dies
    /// with a bare `0xe06d7363` and no message.
    ///
    /// Correcting the bounds is not a workaround. It makes the TEB describe the
    /// stack that is actually in use, which is what the field is for and what every
    /// fibre implementation does for the same reason.
    ///
    /// # Safety
    ///
    /// Must be called on the thread that will enter the guest, and that thread must
    /// not afterwards return into host code that needs the original bounds — the
    /// guest never returns, so that holds here.
    #[cfg(all(windows, target_arch = "x86_64"))]
    pub unsafe fn adopt_as_thread_stack(&self) {
        // The TEB is at `gs:0x30` on x86_64, which is the only way to reach it: the
        // segment is preserved by construction, unlike `fs`.
        let teb: usize;
        // SAFETY: reading the TEB self-pointer, which is always mapped.
        unsafe {
            core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, pure, readonly))
        };

        // `StackBase` is the high end, one past the last usable byte; `StackLimit`
        // is the low end. The region is fully committed, so the limit is its base.
        let stack_base = (teb + 0x08) as *mut usize;
        let stack_limit = (teb + 0x10) as *mut usize;
        // SAFETY: both offsets are within the TEB of the running thread, which is
        // writable — this is how fibre switches update them too.
        unsafe {
            stack_base.write(self.region + STACK_REGION);
            stack_limit.write(self.region);
            let dealloc = (teb + 0x1478) as *mut usize;
            dealloc.write(self.region);
        }
    }
}

/// Reserves a 16-byte aligned, writable block.
#[cfg(windows)]
fn allocate_aligned(len: usize) -> Result<usize, LinkError> {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc,
    };
    let _transaction = kinakaze_runtime::begin_fork_mapping_transaction().ok_or(
        LinkError::MappingRegistrationFailed {
            object: "<initial stack>".to_owned(),
            len,
        },
    )?;
    // VirtualAlloc is page-aligned, which is far stronger than the 16 bytes the
    // ABI needs.
    // SAFETY: requests a fresh private commit; a null base lets the OS choose.
    let block = unsafe {
        VirtualAlloc(
            std::ptr::null(),
            len,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        )
    };
    if block.is_null() {
        return Err(LinkError::MappingFailed {
            object: "<initial stack>".to_owned(),
            len,
        });
    }
    // A raw syscall runs on a separate host transition stack. Consequently the
    // fork coordinator's active-stack snapshot cannot discover this guest
    // stack; it is a guest mapping just like a PT_LOAD segment or mmap region.
    if !kinakaze_runtime::register_fork_mapping(kinakaze_runtime::ForkMapping {
        base: block as usize,
        len,
        behavior: kinakaze_runtime::ForkMappingBehavior::Copy,
        storage: kinakaze_runtime::ForkMappingStorage::Ordinary,
        backing_slot: 0,
        backing_offset: 0,
        view_protection: 0,
        domain: kinakaze_runtime::ForkMappingDomain::GuestMm,
    }) {
        unsafe {
            windows_sys::Win32::System::Memory::VirtualFree(
                block,
                0,
                windows_sys::Win32::System::Memory::MEM_RELEASE,
            )
        };
        return Err(LinkError::MappingRegistrationFailed {
            object: "<initial stack>".to_owned(),
            len,
        });
    }
    Ok(block as usize)
}

#[cfg(not(windows))]
fn allocate_aligned(len: usize) -> Result<usize, LinkError> {
    Err(LinkError::MappingFailed {
        object: "<initial stack>".to_owned(),
        len,
    })
}

/// Jumps to a guest entry point with the kernel's register contract.
///
/// At process entry Linux guarantees:
/// - `rsp` points at `argc`, with `argv`, `envp` and the auxiliary vector above it
/// - `rdx` holds the dynamic linker's finalizer, or zero when there is none
/// - `rbp` is zero, which is what terminates a frame-pointer walk
///
/// glibc's `_start` reads all three, so leaving any of them holding whatever the
/// caller happened to have would send it into garbage.
///
/// # Safety
///
/// `entry` must be the mapped entry point of a fully linked image, and
/// `stack_pointer` must be a 16-byte aligned pointer into a live stack block that
/// outlives the guest. This function never returns.
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn enter(entry: usize, stack_pointer: usize) -> ! {
    // The hosted loader supplies the same rtld_fini contract as an ELF
    // interpreter. Keep the stack and entry operands separate from RDX.
    // SAFETY: the caller guarantees a valid entry and stack; `noreturn` is correct
    // because the guest leaves through `exit`, never by returning here.
    unsafe {
        core::arch::asm!(
            "mov rsp, rcx",
            // A zero frame pointer is what terminates a backtrace walk.
            "xor rbp, rbp",
            "jmp rax",
            in("rcx") stack_pointer,
            in("rax") entry,
            in("rdx") crate::process::rtld_fini as usize,
            options(noreturn)
        )
    }
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
pub unsafe fn enter(_entry: usize, _stack_pointer: usize) -> ! {
    panic!("entering a guest requires x86_64 Windows");
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::stack::{AuxiliaryValues, build};

    fn placed(argument_count: usize) -> PlacedStack {
        let arguments: Vec<String> = (0..argument_count).map(|i| format!("arg{i}")).collect();
        let environment = vec!["PATH=/usr/bin".to_owned()];
        let auxiliary = AuxiliaryValues {
            page_size: 4096,
            ..AuxiliaryValues::default()
        };
        PlacedStack::place(build(&arguments, &environment, &auxiliary)).unwrap()
    }

    #[test]
    fn the_guest_gets_room_to_grow_downward() {
        // The bug this guards: sizing the reservation to the argv/envp block alone
        // leaves nothing beneath the entry pointer, so the guest's first call walks
        // off the bottom of the allocation. The stack grows down, so the usable
        // room is the distance from the stack pointer to the region's base.
        for count in [0, 1, 2, 5] {
            let stack = placed(count);
            let headroom = stack.stack_pointer() - stack.limit();
            assert!(
                headroom >= 4 * 1024 * 1024,
                "only {headroom} bytes below rsp for {count} arguments"
            );
        }
    }

    #[test]
    fn the_entry_stack_pointer_is_abi_aligned() {
        // A misaligned rsp does not fault at entry; it fails much later inside the
        // guest on the first aligned SSE spill, so it has to be checked here.
        for count in 0..8 {
            let stack = placed(count);
            assert!(
                stack.stack_pointer().is_multiple_of(16),
                "rsp {:#x} is misaligned for {count} arguments",
                stack.stack_pointer()
            );
        }
    }

    #[test]
    fn the_block_sits_inside_its_reservation() {
        let stack = placed(3);
        assert!(stack.base() >= stack.limit());
        // The block is seated against the top of the region, so the stack pointer
        // must not be below the block's own base.
        assert!(stack.stack_pointer() >= stack.base());
    }

    #[test]
    fn a_block_larger_than_the_region_is_refused() {
        // A pathological environment must fail cleanly rather than produce a base
        // address below the reservation.
        // Each entry carries a 64 KiB value, so 200 of them clear the 8 MiB region
        // on string data alone.
        let padding = "x".repeat(64 * 1024);
        let huge: Vec<String> = (0..200).map(|i| format!("BIG_{i}={padding}")).collect();
        let image = build(&["prog".to_owned()], &huge, &AuxiliaryValues::default());
        assert!(image.bytes.len() > STACK_REGION);
        assert!(PlacedStack::place(image).is_err());
    }
}
