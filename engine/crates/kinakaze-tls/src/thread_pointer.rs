//! The thread pointer, and the static TLS block reached through it.
//!
//! Linux x86_64 keeps the thread pointer in the `fs` base, and compiled code
//! reaches thread-local data as a fixed displacement from it — `%fs:0x28` for the
//! stack-protector canary, `%fs:-N` for an initial-exec variable. None of those
//! accesses carry a relocation: the displacement is baked in, so there is nothing
//! for a loader to rewrite and no way to redirect them.
//!
//! On Windows the `fs` base is zero and the thread environment block lives at `gs`
//! instead. So `%fs:0x28` reads linear address `0x28` and faults. Any binary built
//! with `-fstack-protector` does that in every function holding a buffer, which is
//! most of them — real BusyBox has 1469 such sites — so hosting real programs
//! requires giving `fs` a real base.
//!
//! `wrfsbase` is the only way to do that from user mode, and it needs
//! `CR4.FSGSBASE`. Windows enables it on capable hardware but not universally, so
//! every entry point here checks first and reports rather than executing an
//! instruction that would fault.
//!
//! The layout follows x86_64 variant II: the thread control block sits *at and
//! above* the thread pointer, and module blocks go *below* it at negative offsets.

use std::sync::Mutex;

/// Size of the thread control block.
///
/// The ABI-fixed displacements a compiler emits without relocations all live in
/// this region, and `%fs:0x28` is the highest one that matters, so the block must
/// be at least 0x30 bytes. Undersizing it does not fail at load: it fails as a
/// wild read inside ordinary guest code.
const CONTROL_BLOCK_SIZE: usize = 0x450;

/// Stable size of the host-private state registered by the loader's fork
/// participant. Keeping this fixed makes the payload exhaustive: newly used
/// fields inside the block cannot be silently omitted by child restoration.
pub const HOST_TRANSITION_BLOCK_SIZE: usize = 0x468;
/// Virtual Linux GS base. Windows keeps the hardware GS base pointed at its TEB.
pub const GUEST_GS_BASE_OFFSET: usize = 0x460;

/// Offset of the stack-protector canary within the control block.
///
/// Fixed by the glibc ABI. Every `-fstack-protector` function loads from here on
/// entry and compares on exit.
const CANARY_OFFSET: usize = 0x28;

/// Offset of the self-pointer glibc keeps at the thread pointer.
const SELF_POINTER_OFFSET: usize = 0x0;

/// Private host-return stack used by link-time import trampolines.
///
/// A trampoline tail-jumps to the real SysV provider so stack arguments keep
/// their exact Linux ABI positions. It temporarily replaces the machine return
/// address and stores the original here until the provider returns.
pub const HOST_RETURN_DEPTH_OFFSET: usize = 0x30;
pub const HOST_RETURN_STACK_OFFSET: usize = 0x38;
pub const HOST_RETURN_STACK_CAPACITY: usize = 64;

/// Saved Windows TEB stack limits for nested ELF-to-host calls.
///
/// A language runtime may switch `rsp` to a managed stack without updating the
/// Windows TEB. MSVC's `__chkstk` trusts `TEB.StackLimit`; retaining that stale
/// native-stack address makes a perfectly ordinary large provider frame probe
/// an unrelated allocation. Import trampolines temporarily use zero as the
/// lower limit (the guest mapping supplies the real page protection) and keep
/// the displaced values here so nested callbacks restore them in LIFO order.
pub const HOST_STACK_LIMIT_STACK_OFFSET: usize =
    HOST_RETURN_STACK_OFFSET + HOST_RETURN_STACK_CAPACITY * core::mem::size_of::<usize>();

/// Top of the private stack used while dispatching raw Linux syscalls to host code.
///
/// Language runtimes can issue a syscall from a very small managed stack. Rust
/// and Win32 functions have no knowledge of that stack's growth protocol, so
/// executing the dispatcher in place can overwrite the managed stack before the
/// runtime gets a chance to grow it. The syscall trampoline switches to this
/// stack for the host portion and restores the exact guest `rsp` on return.
pub const HOST_CALL_STACK_POINTER_OFFSET: usize = 0x438;
pub const HOST_CALL_STACK_SIZE: usize = 256 * 1024;
/// Bytes reserved for one nested raw-syscall host invocation.
///
/// Signal delivery can enter guest code while an outer dispatcher is still on
/// this stack. Giving every nesting level its own lane prevents that inner
/// syscall from overwriting the outer Rust/Win32 frames.
pub const HOST_CALL_STACK_LANE_SIZE: usize = 64 * 1024;

/// Guest stack and instruction pointers for the raw syscall currently running
/// on this thread. Signal delivery uses these to build the interrupted Linux
/// `ucontext_t`; zero means the thread is not inside a raw syscall boundary.
pub const ACTIVE_GUEST_STACK_POINTER_OFFSET: usize = 0x440;
pub const ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET: usize = 0x448;
/// Bytes of the private syscall stack currently owned by nested dispatchers.
pub const HOST_CALL_STACK_USED_OFFSET: usize = 0x450;
/// Pointer to the fixed register record for the innermost active raw syscall.
///
/// The record is not derived from the lane top: its total size includes the
/// processor's CPUID-sized XSAVE area and therefore varies by machine.  Publishing
/// the exact pointer also makes nested signal delivery and clone independent of
/// that implementation detail.
pub const ACTIVE_RAW_SYSCALL_FRAME_POINTER_OFFSET: usize = 0x458;

/// Offsets within the raw-syscall register frame published above.
///
/// These are an ABI between generated trampolines and the process coordinator;
/// naming them here prevents a second, drifting description in the loader.
pub const RAW_SYSCALL_FRAME_GUEST_RSP_OFFSET: usize = 0x10;
pub const RAW_SYSCALL_FRAME_RDI_OFFSET: usize = 0x18;
pub const RAW_SYSCALL_FRAME_RSI_OFFSET: usize = 0x20;
pub const RAW_SYSCALL_FRAME_RDX_OFFSET: usize = 0x28;
pub const RAW_SYSCALL_FRAME_R8_OFFSET: usize = 0x30;
pub const RAW_SYSCALL_FRAME_R9_OFFSET: usize = 0x38;
pub const RAW_SYSCALL_FRAME_R10_OFFSET: usize = 0x40;
pub const RAW_SYSCALL_FRAME_RBP_OFFSET: usize = 0x48;
pub const RAW_SYSCALL_FRAME_RBX_OFFSET: usize = 0x78;
pub const RAW_SYSCALL_FRAME_R12_OFFSET: usize = 0x80;
pub const RAW_SYSCALL_FRAME_R13_OFFSET: usize = 0x88;
pub const RAW_SYSCALL_FRAME_R14_OFFSET: usize = 0x90;
pub const RAW_SYSCALL_FRAME_R15_OFFSET: usize = 0x98;
pub const RAW_SYSCALL_FRAME_PREVIOUS_OFFSET: usize = 0xa0;
pub const RAW_SYSCALL_FRAME_RAX_OFFSET: usize = 0xa8;
pub const RAW_SYSCALL_FRAME_RESULT_OFFSET: usize = 0xb0;
/// Executable continuation carried with the live frame, including copied trampolines.
pub const RAW_SYSCALL_FRAME_CONTINUATION_OFFSET: usize = 0xb8;
/// Start of the 64-byte-aligned FXSAVE/XSAVE image in a raw-syscall record.
pub const RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET: usize = 0xc0;

/// Processor state format used to preserve the architectural state that a
/// Linux `syscall` instruction leaves untouched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtendedStateFormat {
    /// The architectural x87/MMX/SSE state available on every x86-64 CPU.
    Fxsave,
    /// All user states enabled by the host OS in XCR0.
    Xsave { mask: u64, size: usize },
}

/// Returns the exact extended-state format enabled for this process.
///
/// XSAVE is usable only when both the CPU and the OS advertise it.  CPUID leaf
/// 0xD then supplies the byte count for the current XCR0 mask, so new vector
/// extensions do not require a guessed fixed buffer size.  FXSAVE is the native
/// x86-64 architectural baseline when XSAVE is unavailable.
pub fn extended_state_format() -> ExtendedStateFormat {
    #[cfg(target_arch = "x86_64")]
    {
        let features = core::arch::x86_64::__cpuid(1);
        const XSAVE: u32 = 1 << 26;
        const OSXSAVE: u32 = 1 << 27;
        if features.ecx & (XSAVE | OSXSAVE) == (XSAVE | OSXSAVE) {
            let low: u32;
            let high: u32;
            // SAFETY: OSXSAVE means XGETBV(0) is enabled by the operating system.
            unsafe {
                core::arch::asm!(
                    "xgetbv",
                    in("ecx") 0u32,
                    out("eax") low,
                    out("edx") high,
                    options(nostack, preserves_flags),
                )
            };
            let mask = u64::from(low) | (u64::from(high) << 32);
            // Leaf 0xD is defined when XSAVE is present. EBX is the
            // required size for every state component currently enabled in XCR0.
            let layout = core::arch::x86_64::__cpuid_count(0x0d, 0);
            let size = layout.ebx as usize;
            if mask != 0 && size >= 512 {
                return ExtendedStateFormat::Xsave { mask, size };
            }
        }
    }
    ExtendedStateFormat::Fxsave
}

/// Returns the guest RSP/RIP published by the active raw syscall trampoline.
///
/// This state is host-private and deliberately does not live at the Linux FS
/// base. `ARCH_SET_FS` is allowed to replace that base with a language runtime's
/// own TCB; the independently published transition block remains stable.
pub fn active_guest_signal_context() -> Option<(usize, usize)> {
    let base = super::host_transition_pointer();
    if base == 0 {
        return None;
    }
    // SAFETY: both slots are within every installed transition block.
    let stack =
        unsafe { ((base + ACTIVE_GUEST_STACK_POINTER_OFFSET) as *const usize).read_unaligned() };
    let instruction = unsafe {
        ((base + ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET) as *const usize).read_unaligned()
    };
    (stack != 0 && instruction != 0).then_some((stack, instruction))
}

/// Returns the register frame for the raw syscall currently dispatched on this
/// thread. The pointer is valid only until that dispatcher returns; consumers
/// must copy every needed field synchronously.
pub fn active_raw_syscall_frame_pointer() -> Option<usize> {
    let base = super::host_transition_pointer();
    if base == 0 {
        return None;
    }
    // SAFETY: the generated trampoline publishes this process-private field
    // before entering the dispatcher and restores the outer value afterwards.
    let frame = unsafe {
        ((base + ACTIVE_RAW_SYSCALL_FRAME_POINTER_OFFSET) as *const usize).read_unaligned()
    };
    (frame != 0).then_some(frame)
}

/// SysV entry point used by a signal bridge immediately before it restores the
/// interrupted guest's callee-saved registers.
///
/// Returning zero is the exact representation of no active syscall frame; the
/// signal dispatcher only enters the register-restoring bridge after proving a
/// frame is active.
pub unsafe extern "sysv64" fn active_raw_syscall_frame() -> usize {
    active_raw_syscall_frame_pointer().unwrap_or(0)
}

/// Applies handler edits from Linux `ucontext_t` to the active syscall frame.
/// The generated trampoline consumes these fields after the dispatcher returns,
/// matching `rt_sigreturn` for every general-purpose register that `syscall`
/// itself preserves.
pub fn update_active_guest_general_registers(gregs: &[u64; 23]) -> bool {
    let Some(frame) = active_raw_syscall_frame_pointer() else {
        return false;
    };
    let fields = [
        (0, RAW_SYSCALL_FRAME_R8_OFFSET),
        (1, RAW_SYSCALL_FRAME_R9_OFFSET),
        (2, RAW_SYSCALL_FRAME_R10_OFFSET),
        (4, RAW_SYSCALL_FRAME_R12_OFFSET),
        (5, RAW_SYSCALL_FRAME_R13_OFFSET),
        (6, RAW_SYSCALL_FRAME_R14_OFFSET),
        (7, RAW_SYSCALL_FRAME_R15_OFFSET),
        (8, RAW_SYSCALL_FRAME_RDI_OFFSET),
        (9, RAW_SYSCALL_FRAME_RSI_OFFSET),
        (10, RAW_SYSCALL_FRAME_RBP_OFFSET),
        (11, RAW_SYSCALL_FRAME_RBX_OFFSET),
        (12, RAW_SYSCALL_FRAME_RDX_OFFSET),
        // The post-syscall RAX is the dispatcher result rather than the entry
        // syscall number saved in RAW_SYSCALL_FRAME_RAX_OFFSET.
        (13, RAW_SYSCALL_FRAME_RESULT_OFFSET),
    ];
    for (register, offset) in fields {
        unsafe { ((frame + offset) as *mut u64).write_unaligned(gregs[register]) };
    }
    true
}

/// Restores the extended register image saved by the active raw syscall.
///
/// Cooperative Linux signal delivery runs a guest handler while the host
/// dispatcher is still active.  The handler must observe the interrupted
/// guest's vector/FPU state rather than Rust or Win32 temporaries.  The outer
/// syscall trampoline restores the same image again on final `sigreturn`.
///
/// # Safety
///
/// This changes every enabled user extended-state component of the calling
/// thread. Callers must invoke it at an ABI boundary immediately before entering
/// guest code, where all vector registers are caller-clobbered.
pub unsafe extern "sysv64" fn restore_active_guest_extended_state() -> bool {
    let Some(frame) = active_raw_syscall_frame_pointer() else {
        return false;
    };
    let state = frame + RAW_SYSCALL_FRAME_EXTENDED_STATE_OFFSET;
    match extended_state_format() {
        ExtendedStateFormat::Fxsave => unsafe {
            core::arch::asm!(
                "fxrstor64 [{}]",
                in(reg) state,
                clobber_abi("sysv64"),
                options(nostack),
            )
        },
        ExtendedStateFormat::Xsave { mask, .. } => unsafe {
            core::arch::asm!(
                "xrstor64 [{}]",
                in(reg) state,
                in("eax") mask as u32,
                in("edx") (mask >> 32) as u32,
                clobber_abi("sysv64"),
                options(nostack),
            )
        },
    }
    true
}

/// Applies the stack and instruction pointers selected by a signal handler.
///
/// Linux restores these fields from `ucontext_t` when the handler returns.  The
/// raw-syscall trampoline owns the actual register restoration, so the shared
/// TCB slots are the ABI-neutral handoff between signal delivery and that stub.
pub fn update_active_guest_signal_context(stack: usize, instruction: usize) -> bool {
    if stack == 0 || instruction == 0 {
        return false;
    }
    // Provider DLLs resolve this process-owned pointer through the executable,
    // so their private Rust TLS/statics never become an accidental authority.
    let base = super::host_transition_pointer();
    if base == 0 {
        return false;
    }
    let active = active_guest_signal_context().is_some();
    if !active {
        return false;
    }
    unsafe {
        ((base + ACTIVE_GUEST_STACK_POINTER_OFFSET) as *mut usize).write_unaligned(stack);
        ((base + ACTIVE_GUEST_INSTRUCTION_POINTER_OFFSET) as *mut usize)
            .write_unaligned(instruction);
    }
    true
}

/// One module's reserved slot in the static block.
#[derive(Clone, Copy, Debug)]
pub(super) struct StaticModule {
    pub module_id: usize,
    /// Distance *below* the thread pointer where this module's block begins.
    pub offset: usize,
    pub size: usize,
}

#[derive(Clone, Default)]
pub(super) struct Layout {
    pub modules: Vec<StaticModule>,
    /// Total bytes reserved below the thread pointer.
    pub total: usize,
    /// Set once a thread pointer has been installed, after which the layout can no
    /// longer change: existing threads already have their blocks sized.
    pub frozen: bool,
}

fn layout() -> &'static Mutex<Layout> {
    static LAYOUT: std::sync::OnceLock<Mutex<Layout>> = std::sync::OnceLock::new();
    LAYOUT.get_or_init(|| Mutex::new(Layout::default()))
}

#[cfg(windows)]
pub(super) fn fork_layout() -> Option<Layout> {
    Some(layout().lock().ok()?.clone())
}

#[cfg(windows)]
pub(super) fn restore_fork_layout(restored: Layout) -> Result<(), ()> {
    let mut current = layout().lock().map_err(|_| ())?;
    // Guest code contains fixed TPOFF relocations. The child must keep exactly
    // the parent's reservations when it initializes subsequently born threads.
    // The bootstrap owner may refer to the overwritten fork arena.
    let stale = std::mem::replace(&mut *current, restored);
    std::mem::forget(stale);
    Ok(())
}

/// True when this machine allows `wrfsbase` from user mode.
pub fn supported() -> bool {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        // PF_RDWRFSGSBASE_AVAILABLE. Asking is mandatory: executing `wrfsbase`
        // without CR4.FSGSBASE raises an invalid-opcode fault.
        const PF_RDWRFSGSBASE_AVAILABLE: u32 = 22;
        unsafe extern "system" {
            fn IsProcessorFeaturePresent(feature: u32) -> i32;
        }
        // SAFETY: the call takes a feature number and has no other preconditions.
        unsafe { IsProcessorFeaturePresent(PF_RDWRFSGSBASE_AVAILABLE) != 0 }
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        false
    }
}

/// Restores an arbitrary Linux FS base selected by the guest.
///
/// The base is a guest ABI value, not necessarily the bootstrap [`ThreadBlock`]
/// pointer (language runtimes commonly install their own TCB).
pub fn restore_thread_pointer(base: usize) -> Result<bool, super::TlsError> {
    if !supported() {
        return Err(super::TlsError::ThreadPointerUnsupported);
    }
    // SAFETY: support for both FSGSBASE instructions was checked above.
    let current = unsafe { read_fs_base() } as usize;
    if current == base {
        return Ok(false);
    }
    unsafe { write_fs_base(base as u64) };
    Ok(true)
}

/// Reserves space for a module in the static block, returning its offset.
///
/// The offset is the distance *below* the thread pointer, so a variable at segment
/// offset `v` in this module ends up at `tp - offset + v`, and the value an
/// `R_X86_64_TPOFF64` relocation needs is `v - offset`.
pub fn reserve(module_id: usize, size: usize, align: usize) -> Result<usize, super::TlsError> {
    if !supported() {
        return Err(super::TlsError::ThreadPointerUnsupported);
    }
    if align == 0 || !align.is_power_of_two() {
        return Err(super::TlsError::InvalidAlignment);
    }
    let mut layout = layout().lock().map_err(|_| super::TlsError::Poisoned)?;
    if layout.frozen {
        // A thread already has a block sized to the current layout, so growing it
        // now would put this module beyond that thread's allocation.
        return Err(super::TlsError::LayoutFrozen);
    }
    if let Some(existing) = layout
        .modules
        .iter()
        .find(|module| module.module_id == module_id)
    {
        return Ok(existing.offset);
    }

    // Grow downward: the new module's block ends where the previous one began, and
    // its start is rounded up to its own alignment.
    let raw = layout
        .total
        .checked_add(size)
        .ok_or(super::TlsError::OffsetOutOfRange)?;
    let offset = round_up(raw, align).ok_or(super::TlsError::OffsetOutOfRange)?;
    layout.total = offset;
    layout.modules.push(StaticModule {
        module_id,
        offset,
        size,
    });
    Ok(offset)
}

/// The offset a module was reserved at, if it has one.
pub fn offset_of(module_id: usize) -> Option<usize> {
    layout()
        .lock()
        .ok()?
        .modules
        .iter()
        .find(|module| module.module_id == module_id)
        .map(|module| module.offset)
}

/// True when anything needs the static block.
///
/// The block is installed regardless, because the canary lives in it even when no
/// module does.
pub fn has_modules() -> bool {
    layout()
        .lock()
        .map(|layout| !layout.modules.is_empty())
        .unwrap_or(false)
}

/// A thread's static TLS block, owned for the thread's lifetime.
pub struct ThreadBlock {
    /// Keeps the storage alive. Never read: the thread pointer below points into
    /// it, and compiled guest code reaches the block only through `%fs:`, so this
    /// field exists purely so the allocation is not freed while `fs` refers to it.
    _allocation: super::TlsBlock,
    /// Host-only ABI transition state. It has its own published address because
    /// Linux guests are free to replace FS with an unrelated TCB.
    _transition_allocation: super::TlsBlock,
    /// Fully committed private host-call stack. Keeping this separate from the
    /// static TLS allocation prevents guest positive FS offsets from reaching it.
    _host_call_stack: super::TlsBlock,
    thread_pointer: usize,
    transition_pointer: usize,
}

impl ThreadBlock {
    /// The installed thread pointer.
    pub fn thread_pointer(&self) -> usize {
        self.thread_pointer
    }

    /// Stable base of the host-only transition state for this thread.
    pub fn transition_pointer(&self) -> usize {
        self.transition_pointer
    }

    /// Reads the canary, for tests and diagnostics.
    pub fn canary(&self) -> u64 {
        // SAFETY: the pointer is inside this block's own allocation and the canary
        // was written at construction.
        unsafe { ((self.thread_pointer + CANARY_OFFSET) as *const u64).read_unaligned() }
    }

    /// Restores this block as the calling thread's Linux thread pointer.
    ///
    /// Windows can discard a user-written FS base while returning from a kernel
    /// transition.  The block itself remains valid, so the VEH fallback uses this
    /// method to put the base back and retry the original guest instruction.
    /// Returns `true` only when the base actually had to be changed; this lets the
    /// exception handler reject a genuine bad access instead of retrying forever.
    pub fn restore(&self) -> Result<bool, super::TlsError> {
        restore_thread_pointer(self.thread_pointer)
    }
}

impl Drop for ThreadBlock {
    fn drop(&mut self) {
        // Clear the base before the storage goes away. Leaving it pointing into a
        // freed allocation turns every later `%fs:` access on this thread into a
        // use-after-free rather than a clean fault.
        // SAFETY: writing zero is always valid when the feature is present.
        if supported() {
            unsafe { write_fs_base(0) };
        }
    }
}

/// Builds this thread's static block and points `fs` at it.
///
/// `images` supplies each module's initialization bytes, keyed by module id;
/// anything a module does not provide stays zero, which is what `.tbss` requires.
///
/// The returned block must be kept alive for as long as the thread runs.
pub fn install(images: &[(usize, &[u8])]) -> Result<ThreadBlock, super::TlsError> {
    let mut layout = layout().lock().map_err(|_| super::TlsError::Poisoned)?;
    layout.frozen = true;
    let below = layout.total.max(4096);
    let modules = layout.modules.clone();
    drop(layout);

    // One allocation holds the module area, the control block, and enough slack to
    // align the thread pointer without a second allocation.
    let total = below
        .checked_add(CONTROL_BLOCK_SIZE)
        .and_then(|size| size.checked_add(64))
        .ok_or(super::TlsError::OffsetOutOfRange)?;
    // Guest FS, generated host-call trampolines and suspended fork frames keep
    // these addresses. Allocate only these escaping buffers through the runtime
    // arena; module-private Vec/Box storage stays on the ordinary host heap.
    let allocation = super::TlsBlock::zeroed(total, 64).ok_or(super::TlsError::OutOfMemory)?;
    let transition_allocation = super::TlsBlock::zeroed(HOST_TRANSITION_BLOCK_SIZE + 64, 64)
        .ok_or(super::TlsError::OutOfMemory)?;
    let transition_pointer = round_up(transition_allocation.as_ptr() as usize, 64)
        .ok_or(super::TlsError::OffsetOutOfRange)?;
    if transition_pointer + HOST_TRANSITION_BLOCK_SIZE
        > transition_allocation.as_ptr() as usize + transition_allocation.len()
    {
        return Err(super::TlsError::OffsetOutOfRange);
    }
    let host_call_stack = super::TlsBlock::zeroed(HOST_CALL_STACK_SIZE + 16, 16)
        .ok_or(super::TlsError::OutOfMemory)?;

    // The thread pointer sits just above the module area, aligned to 64 bytes so
    // every module's own alignment below it is also satisfied.
    let base = allocation.as_ptr() as usize;
    let unaligned = base
        .checked_add(below)
        .ok_or(super::TlsError::OffsetOutOfRange)?;
    let thread_pointer = round_up(unaligned, 64).ok_or(super::TlsError::OffsetOutOfRange)?;
    if thread_pointer + CONTROL_BLOCK_SIZE > base + allocation.len() {
        return Err(super::TlsError::OffsetOutOfRange);
    }

    // Initialize Linux x86_64 tcbhead_t self-pointer at %fs:0
    unsafe {
        (thread_pointer as *mut usize).write(thread_pointer);
    }

    // Copy each module's template into its slot.
    for module in &modules {
        let Some((_, image)) = images.iter().find(|(id, _)| *id == module.module_id) else {
            continue;
        };
        let length = image.len().min(module.size);
        let destination = thread_pointer
            .checked_sub(module.offset)
            .ok_or(super::TlsError::OffsetOutOfRange)?;
        if destination < base {
            return Err(super::TlsError::OffsetOutOfRange);
        }
        // SAFETY: `destination` lies inside the allocation, bounded above by the
        // thread pointer and below by the base, with at least `length` bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(image.as_ptr(), destination as *mut u8, length);
        }
    }

    let configured = super::configured_canary();
    let canary = if configured != 0 {
        configured as u64
    } else {
        canary_value()
    };
    // SAFETY: both offsets are inside the control block, which was bounds-checked
    // above.
    unsafe {
        // glibc keeps a pointer to the TCB at its own start, and some code reads it
        // rather than issuing `rdfsbase`.
        ((thread_pointer + SELF_POINTER_OFFSET) as *mut usize).write_unaligned(thread_pointer);
        ((thread_pointer + CANARY_OFFSET) as *mut u64).write_unaligned(canary);
        let host_stack_top = (host_call_stack.as_ptr() as usize + host_call_stack.len()) & !15usize;
        ((transition_pointer + HOST_CALL_STACK_POINTER_OFFSET) as *mut usize)
            .write_unaligned(host_stack_top);
    }

    if supported() {
        // SAFETY: the feature was confirmed present, and the block outlives this call
        // because the caller keeps the returned value.
        unsafe { write_fs_base(thread_pointer as u64) };
    }

    Ok(ThreadBlock {
        _allocation: allocation,
        _transition_allocation: transition_allocation,
        _host_call_stack: host_call_stack,
        thread_pointer,
        transition_pointer,
    })
}

/// A value for the stack-protector canary.
///
/// Deliberately not cryptographic. The canary defends against a buffer overflow
/// overwriting a return address, and any value the attacker cannot predict from the
/// binary serves; a hosted process here is not a hardening boundary. Real entropy
/// would mean a CSPRNG dependency for a guard that the host's own protections
/// already sit behind.
fn canary_value() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos() as u64)
        .unwrap_or(0);
    let pid = u64::from(std::process::id());
    let local = 0u64;
    let address = std::ptr::from_ref(&local) as u64;
    let mut value = nanos.rotate_left(17) ^ pid.rotate_left(31) ^ address;
    // glibc keeps the low byte zero so a string overflow that stops at a NUL cannot
    // learn the rest of the canary by overwriting it one byte at a time.
    value &= !0xff;
    if value == 0 {
        // A zero canary would compare equal to a zeroed frame, defeating the check.
        value = 0x0123_4567_89ab_cd00;
    }
    value
}

fn round_up(value: usize, align: usize) -> Option<usize> {
    let mask = align - 1;
    value.checked_add(mask).map(|sum| sum & !mask)
}

/// # Safety
///
/// `CR4.FSGSBASE` must be enabled, which [`supported`] confirms.
#[cfg(all(windows, target_arch = "x86_64"))]
unsafe fn write_fs_base(base: u64) {
    // SAFETY: the caller guarantees the instruction is permitted.
    unsafe { core::arch::asm!("wrfsbase {base}", base = in(reg) base) };
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
unsafe fn write_fs_base(_base: u64) {}

/// Reads the current `fs` base.
///
/// # Safety
///
/// As [`write_fs_base`].
#[cfg(all(windows, target_arch = "x86_64"))]
pub unsafe fn read_fs_base() -> u64 {
    let base: u64;
    // SAFETY: the caller guarantees the instruction is permitted.
    unsafe { core::arch::asm!("rdfsbase {base}", base = out(reg) base) };
    base
}

#[cfg(not(all(windows, target_arch = "x86_64")))]
pub unsafe fn read_fs_base() -> u64 {
    0
}

#[cfg(all(test, windows, target_arch = "x86_64"))]
mod tests {
    use super::*;

    #[test]
    fn the_canary_is_readable_at_the_abi_fixed_offset() {
        if !supported() {
            eprintln!("skipped: this machine does not permit wrfsbase");
            return;
        }
        // This is the exact access every -fstack-protector function makes on entry,
        // and the one real BusyBox faulted on before the thread pointer existed.
        let block = install(&[]).unwrap();
        let canary: u64;
        // SAFETY: the block installed a thread pointer with a canary at 0x28.
        unsafe { core::arch::asm!("mov {out}, fs:[0x28]", out = out(reg) canary) };
        assert_ne!(canary, 0, "a zero canary would match a zeroed frame");
        assert_eq!(canary, block.canary());
        // glibc zeroes the low byte so a NUL-terminated overflow cannot leak it.
        assert_eq!(canary & 0xff, 0);
    }

    #[test]
    fn the_self_pointer_matches_the_thread_pointer() {
        if !supported() {
            return;
        }
        let block = install(&[]).unwrap();
        let self_pointer: usize;
        // SAFETY: the self-pointer is written at the thread pointer itself.
        unsafe { core::arch::asm!("mov {out}, fs:[0x0]", out = out(reg) self_pointer) };
        assert_eq!(self_pointer, block.thread_pointer());
    }

    #[test]
    fn dropping_the_block_clears_the_base() {
        if !supported() {
            return;
        }
        {
            let block = install(&[]).unwrap();
            // SAFETY: the feature is present.
            assert_eq!(unsafe { read_fs_base() }, block.thread_pointer() as u64);
        }
        // Leaving a base pointing into freed storage would turn later %fs: accesses
        // into a use-after-free instead of a clean fault.
        // SAFETY: the feature is present.
        assert_eq!(unsafe { read_fs_base() }, 0);
    }

    #[test]
    fn reserved_modules_land_below_the_thread_pointer() {
        if !supported() {
            return;
        }
        // A module's data must be reachable at a negative displacement, which is
        // what the initial-exec model compiles to.
        let marker: [u8; 8] = 0xa5a5_a5a5_dead_beefu64.to_le_bytes();
        let module = 4242;
        let offset = match reserve(module, 8, 8) {
            Ok(offset) => offset,
            // A previous test may have frozen the layout; that is not this test's
            // subject, so it is skipped rather than forced.
            Err(super::super::TlsError::LayoutFrozen) => return,
            Err(error) => panic!("unexpected reservation failure: {error:?}"),
        };
        let block = install(&[(module, &marker)]).unwrap();
        // Read at `tp - offset`, which is where an initial-exec access lands. The
        // displacement is a runtime value here, so this is a plain load rather than
        // the `%fs:-N` form a compiler would emit; the address computed is the same.
        // SAFETY: the module's block was placed at `tp - offset` and holds 8 bytes.
        let read = unsafe { ((block.thread_pointer() - offset) as *const u64).read_unaligned() };
        assert_eq!(read, 0xa5a5_a5a5_dead_beef);
    }
}
