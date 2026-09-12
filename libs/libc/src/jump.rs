//! Non-local jumps: `setjmp`, `longjmp` and the sigjmp variants.
//!
//! These are the only functions in this libc that cannot be written in Rust. A
//! `setjmp` has to record the stack pointer its own caller will return to and
//! then return twice, and a `longjmp` has to reseat `rsp` and continue at a
//! saved address. Both are stated directly in assembly, and the register set
//! they save is the System V callee-saved set — `rbx`, `rbp`, `r12`-`r15`, plus
//! the stack pointer and the return address.
//!
//! Three decisions here are worth stating up front, because each is a place
//! where following glibc literally would have been wrong.
//!
//! **No pointer mangling.** Debian's glibc XORs the saved `rsp` and `rip`
//! against a per-process guard held at `%fs:0x30` before storing them, so a
//! buffer overflow that rewrites a `jmp_buf` cannot aim the jump. That guard
//! lives at a fixed `%fs` displacement, and on Windows `%fs` has no usable base
//! at all: this project rewrites the guest's `%fs:0x28` canary reads to a TEB
//! slot precisely because the segment is not available (see
//! `kinakaze-link/src/segment_patch.rs`). Mangling against a location that does
//! not exist would store a value we could not unmangle. It is safe to omit only
//! because both halves come from this file — no `jmp_buf` is ever filled by one
//! implementation and consumed by another. If that ever stops being true, this
//! is the comment that stops being true with it.
//!
//! **`__mask_was_saved` is honoured, but the mask it stores is ours.** The
//! `jmp_buf` reserves 128 bytes for a `sigset_t` because that is Linux's size.
//! This layer tracks the blocked set as one `u64` (64 signals, which is all
//! Linux defines), so the saved mask occupies the first eight bytes and the rest
//! stays zero. The field is still 128 bytes wide because guest code compiled
//! against real headers computes `sizeof(jmp_buf)` and allocates from it.
//!
//! **`__longjmp_chk` cannot check what glibc checks.** glibc rejects a jump
//! whose target stack pointer lies outside the current thread's stack, which it
//! learns from the thread descriptor. Guest code here does not run on the
//! Windows thread stack: the loader hands it a separate 8 MiB block and seats
//! `rsp` inside that (see `kinakaze-link/src/launch.rs`), so
//! `GetCurrentThreadStackLimits` describes a region the guest's `rsp` is not in
//! and would reject every legitimate jump. What is checked instead is stated at
//! [`kinakaze_abi___longjmp_chk`], and it catches the same bug class.

use core::arch::naked_asm;
use core::ffi::{c_int, c_void};

use kinakaze_vfs::signal::{self, SIG_SETMASK};

/// Byte offsets within `__jmpbuf`, matching glibc's `jmpbuf-offsets.h` for
/// x86_64. These are ABI: guest code allocates a `jmp_buf` sized by the real
/// header and this layer writes into it.
const JB_RBX: usize = 0;
const JB_RBP: usize = 8;
const JB_R12: usize = 16;
const JB_R13: usize = 24;
const JB_R14: usize = 32;
const JB_R15: usize = 40;
const JB_RSP: usize = 48;
const JB_PC: usize = 56;

/// The Linux x86_64 `jmp_buf`, 200 bytes.
///
/// Field order and size are ABI, not a choice. `registers` is glibc's
/// `__jmpbuf`; `mask_was_saved` is the flag `setjmp` sets and `_setjmp` clears;
/// `saved_mask` is 128 bytes because Linux's `sigset_t` is, even though only the
/// first eight carry information here.
#[repr(C)]
pub struct JmpBuf {
    /// Callee-saved registers, then `rsp`, then the resume address.
    pub registers: [u64; 8],
    /// Nonzero when `saved_mask` holds a mask to restore.
    pub mask_was_saved: c_int,
    /// Explicit, because C's alignment rules would insert it anyway and a
    /// reader should not have to derive the offset of `saved_mask`.
    pad: c_int,
    /// The blocked set at the time of the `setjmp`.
    pub saved_mask: [u64; 16],
}

/// `__sigsetjmp`: the entry point every `setjmp` spelling funnels into.
///
/// Saves the callee-saved registers and returns 0. A later `longjmp` on the same
/// buffer returns here a second time with that call's value.
///
/// The stack pointer stored is the value `rsp` will have *after* this function's
/// return address is popped, which is what the caller's frame will see when the
/// jump lands. The return address itself becomes the resume point.
///
/// `savemask` nonzero also records the blocked signal set, which is what
/// separates `setjmp` from `_setjmp`.
///
/// # Safety
///
/// `env` must point to a writable `jmp_buf`. The frame that calls this must
/// still be live when a `longjmp` targets the buffer — jumping into a function
/// that has already returned is undefined, and no implementation can detect it
/// in general.
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___sigsetjmp(env: *mut JmpBuf, savemask: c_int) -> c_int {
    naked_asm!(
        // Callee-saved registers, stored unmangled. See the module docs.
        "mov [rdi + {rbx}], rbx",
        "mov [rdi + {rbp}], rbp",
        "mov [rdi + {r12}], r12",
        "mov [rdi + {r13}], r13",
        "mov [rdi + {r14}], r14",
        "mov [rdi + {r15}], r15",
        // `rsp` as the caller will see it: this frame's return address popped.
        "lea rax, [rsp + 8]",
        "mov [rdi + {rsp}], rax",
        // The return address is both where a `longjmp` resumes and where this
        // call returns normally.
        "mov rax, [rsp]",
        "mov [rdi + {pc}], rax",
        // Tail-call the mask half. It returns 0 into `eax`, and because this is
        // a jump rather than a call, its `ret` returns to *our* caller — which
        // is the normal, first-time return from `setjmp`.
        "jmp {finish}",
        rbx = const JB_RBX,
        rbp = const JB_RBP,
        r12 = const JB_R12,
        r13 = const JB_R13,
        r14 = const JB_R14,
        r15 = const JB_R15,
        rsp = const JB_RSP,
        pc = const JB_PC,
        finish = sym setjmp_finish,
    )
}

/// Records the signal mask and produces `setjmp`'s first-time return value.
///
/// Split out of the assembly because querying the blocked set is ordinary Rust,
/// and reached by a tail jump so that its `ret` lands in the original caller.
/// `env` and `savemask` are still in their argument registers at the jump, so
/// the signature repeats them.
extern "sysv64" fn setjmp_finish(env: *mut JmpBuf, savemask: c_int) -> c_int {
    if env.is_null() {
        return 0;
    }
    // SAFETY: `env` is the caller's `jmp_buf`, non-null and writable, and the
    // assembly above has already written the register half of it.
    if savemask != 0 {
        let buffer = unsafe { &mut *env };
        buffer.mask_was_saved = 1;
        buffer.saved_mask = [0; 16];
        buffer.saved_mask[0] = signal::blocked_mask();
    } else {
        // GNU pthread cleanup uses a shorter buffer with only this prefix.
        // Do not form a reference covering the absent 128-byte signal mask.
        unsafe { core::ptr::addr_of_mut!((*env).mask_was_saved).write(0) };
    }
    0
}

/// `setjmp`: saves the register state and the signal mask.
///
/// glibc's `setjmp` saves the mask; only `_setjmp` skips it. That is a BSD
/// inheritance rather than what C89 requires, and guest code compiled against
/// glibc depends on it.
///
/// # Safety
///
/// As [`kinakaze_abi___sigsetjmp`].
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setjmp(env: *mut JmpBuf) -> c_int {
    naked_asm!("mov esi, 1", "jmp {inner}", inner = sym kinakaze_abi___sigsetjmp)
}

/// `_setjmp`: saves the register state but not the signal mask.
///
/// # Safety
///
/// As [`kinakaze_abi___sigsetjmp`].
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__setjmp(env: *mut JmpBuf) -> c_int {
    naked_asm!("xor esi, esi", "jmp {inner}", inner = sym kinakaze_abi___sigsetjmp)
}

/// Reseats the stack and resumes at the saved address. Never returns.
///
/// `value` arrives in `esi` and leaves in `eax`, so it becomes the second return
/// value of the `setjmp` that filled this buffer. A `value` of 0 is rewritten to
/// 1, because 0 is how `setjmp` reports its *first* return and a caller must be
/// able to tell the two apart.
///
/// # Safety
///
/// `env` must hold a buffer written by this file's `setjmp`, and the frame that
/// wrote it must still be live.
#[unsafe(naked)]
unsafe extern "sysv64" fn restore(env: *const JmpBuf, value: c_int) -> ! {
    naked_asm!(
        "mov eax, esi",
        "test eax, eax",
        "jnz 2f",
        "mov eax, 1",
        "2:",
        // Registers first, while `rdi` still addresses the buffer and the old
        // stack is still current.
        "mov rbx, [rdi + {rbx}]",
        "mov rbp, [rdi + {rbp}]",
        "mov r12, [rdi + {r12}]",
        "mov r13, [rdi + {r13}]",
        "mov r14, [rdi + {r14}]",
        "mov r15, [rdi + {r15}]",
        // The resume address goes to a scratch register before `rsp` moves, so
        // the jump target does not depend on the new stack being readable.
        "mov rdx, [rdi + {pc}]",
        "mov rsp, [rdi + {rsp}]",
        "jmp rdx",
        rbx = const JB_RBX,
        rbp = const JB_RBP,
        r12 = const JB_R12,
        r13 = const JB_R13,
        r14 = const JB_R14,
        r15 = const JB_R15,
        rsp = const JB_RSP,
        pc = const JB_PC,
    )
}

/// Restores the saved signal mask, if `setjmp` recorded one.
///
/// Runs before the stack is reseated: once `rsp` moves there is no frame to make
/// an ordinary call from.
///
/// # Safety
///
/// `env` must point to a readable `jmp_buf`.
unsafe fn restore_mask(env: *const JmpBuf) {
    if env.is_null() {
        return;
    }
    // SAFETY: the caller guarantees a readable buffer.
    let buffer = unsafe { &*env };
    if buffer.mask_was_saved != 0 {
        let _ = signal::sigprocmask(SIG_SETMASK, buffer.saved_mask[0]);
    }
}

/// `longjmp`: resumes at the matching `setjmp` with `value` as its result.
///
/// # Safety
///
/// `env` must hold a buffer written by this file's `setjmp`, and the frame that
/// wrote it must still be live.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_longjmp(env: *const JmpBuf, value: c_int) -> ! {
    // SAFETY: the caller guarantees a readable buffer.
    unsafe { restore_mask(env) };
    // SAFETY: the contract is the caller's; nothing after this point runs.
    unsafe { restore(env, value) }
}

/// `siglongjmp`: identical to `longjmp`.
///
/// The two are separate symbols in glibc but the same code: whether the mask is
/// restored is decided by the flag the *`setjmp`* side stored, not by which
/// spelling of the jump is used.
///
/// # Safety
///
/// As [`kinakaze_abi_longjmp`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_siglongjmp(env: *const JmpBuf, value: c_int) -> ! {
    // SAFETY: the caller's contract, forwarded unchanged.
    unsafe { kinakaze_abi_longjmp(env, value) }
}

/// `_longjmp`: `longjmp` without restoring the signal mask.
///
/// # Safety
///
/// As [`kinakaze_abi_longjmp`].
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi__longjmp(env: *const JmpBuf, value: c_int) -> ! {
    // SAFETY: the contract is the caller's; the mask is deliberately untouched.
    unsafe { restore(env, value) }
}

/// `__longjmp_chk`: `longjmp` with the checks this platform can actually make.
///
/// glibc's version rejects a jump whose target stack pointer is outside the
/// current thread's stack, which it reads from the thread descriptor. That test
/// is unavailable here and would be actively wrong: the loader runs guest code
/// on a private 8 MiB block rather than the Windows thread stack, so the thread
/// bounds exclude every legitimate guest `rsp`.
///
/// Three checks remain:
///
/// 1. Neither the resume address nor the stack pointer may be zero. A zeroed or
///    never-initialised buffer fails here.
/// 2. The target stack pointer must be *above* the current one. Stacks grow
///    down, so the frame that called `setjmp` is always at a higher address than
///    the frame jumping back to it.
/// 3. The target must be committed, readable memory, which `VirtualQuery`
///    answers directly. This catches a wild pointer that happens to satisfy (2).
///
/// What this does **not** catch is worth being precise about, because it would
/// be easy to read (2) as more than it is: a jump into a *sibling* frame that
/// has already returned can still pass, since a dead frame's recorded `rsp` is
/// above the current one whenever the jump is made from deeper in the stack.
/// Detecting that would need liveness information no runtime has. glibc's
/// version has the same blind spot for the same reason — its check is also a
/// bounds test, not a liveness test. What (2) reliably rejects is a buffer whose
/// stack pointer was overwritten with a smaller value, and (3) a wild one.
///
/// A failure goes to `__chk_fail`, the same abort path the fortified string
/// functions use, so the diagnostic is consistent with the rest of the layer.
///
/// # Safety
///
/// `env` must point to a readable `jmp_buf`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi___longjmp_chk(env: *const JmpBuf, value: c_int) -> ! {
    if env.is_null() {
        fail();
    }
    // SAFETY: checked non-null; the caller guarantees it is readable.
    let buffer = unsafe { &*env };
    let target = buffer.registers[JB_RSP / 8] as usize;
    let resume = buffer.registers[JB_PC / 8] as usize;
    if target == 0 || resume == 0 || !target_is_live(target) {
        fail();
    }
    // SAFETY: the buffer passed the checks above.
    unsafe { kinakaze_abi_longjmp(env, value) }
}

/// Reports whether `target` is plausibly a live outer frame.
///
/// Split from the caller so the address arithmetic is unit-testable: the failure
/// path aborts the process and cannot be reached from a test.
fn target_is_live(target: usize) -> bool {
    let current = current_stack_pointer();
    if target <= current {
        return false;
    }
    let mut info = MemoryBasicInformation::default();
    let size = core::mem::size_of::<MemoryBasicInformation>();
    // SAFETY: `info` is a live, correctly sized local.
    let written = unsafe { VirtualQuery(target as *const c_void, &raw mut info, size) };
    if written == 0 {
        return false;
    }
    // Committed and readable. `PAGE_NOACCESS` and `PAGE_GUARD` both mean a read
    // would fault, and the loader uses `PAGE_NOACCESS` deliberately for the
    // guard page that missing data symbols point at.
    const MEM_COMMIT: u32 = 0x1000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_GUARD: u32 = 0x100;
    info.state == MEM_COMMIT && info.protect & PAGE_NOACCESS == 0 && info.protect & PAGE_GUARD == 0
}

/// The caller's stack pointer, near enough for the ordering test.
///
/// The address of a local is a few bytes above `rsp` in this frame, which is
/// below any frame that could legitimately be jumped to, so the comparison in
/// [`target_is_live`] is unaffected by the difference.
#[inline(never)]
fn current_stack_pointer() -> usize {
    let anchor = 0_u64;
    &raw const anchor as usize
}

unsafe extern "sysv64" {
    /// `__chk_fail`: the shared abort path, defined in [`crate::startup`].
    ///
    /// Reached by linkage rather than by path because that module is private, the
    /// same way [`crate::fortify`] reaches it. One definition, one diagnostic.
    safe fn kinakaze_abi___chk_fail() -> !;
}

/// Reports a rejected jump and exits, reusing the fortify abort path.
fn fail() -> ! {
    kinakaze_abi___chk_fail()
}

/// `MEMORY_BASIC_INFORMATION`. Declared here rather than pulled from
/// `windows-sys` because this crate's enabled features do not cover
/// `Win32_System_Memory`.
#[repr(C)]
#[derive(Default)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    reserved: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    region_type: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    /// `VirtualQuery`: the committed state and protection of one address.
    fn VirtualQuery(
        address: *const c_void,
        buffer: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};
    use std::hint::black_box;
    use std::sync::atomic::{AtomicI32, Ordering};

    /// The layout is ABI: guest code sizes its own `jmp_buf` from the real
    /// header, so a mismatch here is silent stack corruption in the guest.
    #[test]
    fn the_buffer_matches_the_linux_layout() {
        assert_eq!(size_of::<JmpBuf>(), 200);
        assert_eq!(offset_of!(JmpBuf, registers), 0);
        assert_eq!(offset_of!(JmpBuf, mask_was_saved), 64);
        assert_eq!(offset_of!(JmpBuf, saved_mask), 72);
    }

    fn empty() -> JmpBuf {
        JmpBuf {
            registers: [0; 8],
            mask_was_saved: 0,
            pad: 0,
            saved_mask: [0; 16],
        }
    }

    /// The defining property: `setjmp` returns 0, and the `longjmp` makes it
    /// return again with the jump's value.
    ///
    /// The counter is atomic and the buffer goes through `black_box` because a
    /// value the compiler believes is loop-invariant may live in a register
    /// across the `setjmp`, and the second return would then see a stale copy.
    #[test]
    fn a_jump_returns_through_setjmp_a_second_time() {
        static VISITS: AtomicI32 = AtomicI32::new(0);
        VISITS.store(0, Ordering::SeqCst);
        let mut buffer = empty();

        // SAFETY: `buffer` outlives the jump and this frame is live throughout.
        let first = unsafe { kinakaze_abi___sigsetjmp(black_box(&raw mut buffer), 0) };
        VISITS.fetch_add(1, Ordering::SeqCst);

        if first == 0 {
            // SAFETY: the buffer was filled by the call above, in this frame.
            unsafe { kinakaze_abi_longjmp(&raw const buffer, 42) };
        }

        assert_eq!(first_and_second(first), 42);
        assert_eq!(VISITS.load(Ordering::SeqCst), 2);
    }

    /// Reads the second-return value, kept out of the test body so the compiler
    /// cannot fold the comparison into the first pass.
    #[inline(never)]
    fn first_and_second(value: c_int) -> c_int {
        black_box(value)
    }

    /// `longjmp(env, 0)` must not be indistinguishable from the first return.
    #[test]
    fn zero_is_promoted_to_one() {
        let mut buffer = empty();
        // SAFETY: as above.
        let result = unsafe { kinakaze_abi___sigsetjmp(black_box(&raw mut buffer), 0) };
        if result == 0 {
            // SAFETY: filled in this frame, which is still live.
            unsafe { kinakaze_abi_longjmp(&raw const buffer, 0) };
        }
        assert_eq!(first_and_second(result), 1);
    }

    /// Callee-saved registers must survive the round trip. Six live values
    /// across the jump is enough to force the compiler to use them.
    #[test]
    fn callee_saved_registers_come_back() {
        let mut buffer = empty();
        let (a, b, c, d, e, f) = (
            black_box(1_u64),
            black_box(2_u64),
            black_box(3_u64),
            black_box(4_u64),
            black_box(5_u64),
            black_box(6_u64),
        );
        // SAFETY: as above.
        let pass = unsafe { kinakaze_abi___sigsetjmp(black_box(&raw mut buffer), 0) };
        let sum = a + b + c + d + e + f;
        if pass == 0 {
            // SAFETY: filled in this frame.
            unsafe { kinakaze_abi_longjmp(&raw const buffer, 7) };
        }
        assert_eq!(black_box(sum), 21);
    }

    /// `setjmp` records the mask; `_setjmp` does not. The flag is what
    /// `longjmp` consults, so it has to be set from the right side.
    #[test]
    fn only_the_masking_form_records_the_mask() {
        let mut with = empty();
        // SAFETY: writable buffer, live frame; the jump is never taken.
        if unsafe { kinakaze_abi_setjmp(black_box(&raw mut with)) } == 0 {
            assert_eq!(with.mask_was_saved, 1);
        }

        let mut without = empty();
        // SAFETY: as above.
        if unsafe { kinakaze_abi__setjmp(black_box(&raw mut without)) } == 0 {
            assert_eq!(without.mask_was_saved, 0);
        }
    }

    /// The recorded stack pointer must be this frame's, and above any deeper
    /// frame's — the second half is what makes [`target_is_live`] work.
    #[test]
    fn the_saved_stack_pointer_is_the_callers() {
        let mut buffer = empty();
        let anchor = 0_u64;
        let local = &raw const anchor as u64;
        // SAFETY: as above.
        if unsafe { kinakaze_abi___sigsetjmp(black_box(&raw mut buffer), 0) } == 0 {
            let saved = buffer.registers[JB_RSP / 8];
            // The value recorded is `rsp` at the call site, which is the bottom
            // of this frame: below this frame's locals, not above them.
            assert!(
                saved < local,
                "saved rsp {saved:#x} should be below local {local:#x}"
            );
            assert!(
                local - saved < 4096,
                "saved rsp {saved:#x} should be in this frame"
            );
            // The property `__longjmp_chk` relies on: a jump arriving from a
            // deeper frame sees its target above the current stack pointer.
            let deeper = current_stack_pointer() as u64;
            assert!(
                saved > deeper,
                "saved rsp {saved:#x} should be above deeper {deeper:#x}"
            );
            assert_ne!(buffer.registers[JB_PC / 8], 0);
        }
    }

    /// A target below the current stack pointer names a frame that has already
    /// returned, which is exactly what `__longjmp_chk` must reject.
    #[test]
    fn a_target_below_the_stack_pointer_is_rejected() {
        assert!(!target_is_live(current_stack_pointer() - 4096));
        assert!(!target_is_live(0));
    }

    /// A real address further up this thread's stack passes all three checks.
    #[test]
    fn a_live_outer_frame_is_accepted() {
        let anchor = 0_u64;
        let here = &raw const anchor as usize;
        assert!(target_is_live(here + 256));
    }

    /// An unmapped address is rejected even though it is above `rsp`.
    #[test]
    fn an_unmapped_target_is_rejected() {
        assert!(!target_is_live(0x7fff_ffff_0000));
    }
}
