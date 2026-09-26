//! Exception delivery, syscall traps and per-thread execution state.
use std::cell::Cell;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// Exception callbacks must not unwind when an exec handoff, closed pipe, or
// redirected native stderr makes diagnostic output fail. In particular,
// OpenSSL probes instructions using recoverable synchronous exceptions.
macro_rules! fault_report {
    ($($argument:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stderr().lock(), $($argument)*);
    }};
}

static VEH_CANARY_TARGET: AtomicUsize = AtomicUsize::new(0);
static VEH_THREAD_POINTER_TARGET: AtomicUsize = AtomicUsize::new(0);
static VEH_HOST_TRANSITION_TARGET: AtomicUsize = AtomicUsize::new(0);
static VEH_SCRATCH_TARGET: AtomicUsize = AtomicUsize::new(0);
static VEH_SYSCALL_DISPATCHER: AtomicUsize = AtomicUsize::new(0);
static VEH_SYNC_SIGNAL_DISPATCHER: AtomicUsize = AtomicUsize::new(0);
static VEH_MAPPING_FAULT_DISPATCHER: AtomicUsize = AtomicUsize::new(0);
static VEH_REPORT_HEALS: AtomicBool = AtomicBool::new(false);
thread_local! {
    // Non-null only while the current native thread is dispatching a
    // two-byte syscall trap. Returns-twice syscalls copy this synchronous
    // Windows context before the vectored handler resumes the parent.
    static ACTIVE_TRAP_CONTEXT: Cell<usize> = const { Cell::new(0) };
}

pub fn active_trap_context_pointer() -> usize {
    ACTIVE_TRAP_CONTEXT.with(Cell::get)
}

/// Arguments copied out of a Windows exception context before the raw Linux
/// syscall dispatcher leaves the guest stack. Keeping this as one pointer
/// also avoids depending on either ABI's stack-argument layout during the
/// stack switch itself.
#[repr(C)]
struct TrapSyscallCall {
    dispatcher: usize,
    transition: usize,
    number: i64,
    arguments: [u64; 6],
}

/// Executes a trapped raw syscall on the calling thread's registered host
/// stack lane. Windows exception dispatch begins on the interrupted guest
/// stack; arbitrary Rust/Win32 work must not consume or probe that managed
/// stack because Go and other runtimes own its growth and frame metadata.
#[unsafe(naked)]
unsafe extern "sysv64" fn dispatch_trap_on_host_stack(_call: *const TrapSyscallCall) -> i64 {
    core::arch::naked_asm!(
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov r12, rsp",
        "mov r13, rdi",
        "mov r14, [r13 + {transition}]",
        "test r14, r14",
        "jz 2f",
        "mov r15, [r14 + {used}]",
        "cmp r15, {last_lane}",
        "ja 2f",
        "mov rsp, [r14 + {stack_top}]",
        "sub rsp, r15",
        "mov r10, rsp",
        "add qword ptr [r14 + {used}], {lane}",
        "and rsp, -16",
        "sub rsp, 16",
        "mov rbx, qword ptr gs:[0x08]",
        "mov rbp, qword ptr gs:[0x10]",
        "mov qword ptr gs:[0x08], r10",
        "xor r11d, r11d",
        "mov qword ptr gs:[0x10], r11",
        "mov rdi, [r13 + {number}]",
        "mov rsi, [r13 + {argument0}]",
        "mov rdx, [r13 + {argument1}]",
        "mov rcx, [r13 + {argument2}]",
        "mov r8, [r13 + {argument3}]",
        "mov r9, [r13 + {argument4}]",
        "mov r10, [r13 + {argument5}]",
        "mov [rsp], r10",
        "mov rax, [r13 + {dispatcher}]",
        "call rax",
        "mov qword ptr gs:[0x08], rbx",
        "mov qword ptr gs:[0x10], rbp",
        "mov [r14 + {used}], r15",
        "mov rsp, r12",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "ret",
        // Missing/corrupt transition state is an internal invariant failure,
        // not a guest syscall errno. Trap at the boundary instead of
        // returning a value that would claim the syscall ran.
        "2:",
        "ud2",
        dispatcher = const core::mem::offset_of!(TrapSyscallCall, dispatcher),
        transition = const core::mem::offset_of!(TrapSyscallCall, transition),
        number = const core::mem::offset_of!(TrapSyscallCall, number),
        argument0 = const core::mem::offset_of!(TrapSyscallCall, arguments),
        argument1 = const core::mem::offset_of!(TrapSyscallCall, arguments) + 8,
        argument2 = const core::mem::offset_of!(TrapSyscallCall, arguments) + 16,
        argument3 = const core::mem::offset_of!(TrapSyscallCall, arguments) + 24,
        argument4 = const core::mem::offset_of!(TrapSyscallCall, arguments) + 32,
        argument5 = const core::mem::offset_of!(TrapSyscallCall, arguments) + 40,
        stack_top = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET,
        used = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_USED_OFFSET,
        lane = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE,
        last_lane = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_SIZE
            - kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE,
    )
}

pub fn canary_target() -> usize {
    VEH_CANARY_TARGET.load(Ordering::Acquire)
}

pub fn thread_pointer_target() -> usize {
    VEH_THREAD_POINTER_TARGET.load(Ordering::Acquire)
}

pub fn host_transition_target() -> usize {
    VEH_HOST_TRANSITION_TARGET.load(Ordering::Acquire)
}

pub fn scratch_target() -> usize {
    VEH_SCRATCH_TARGET.load(Ordering::Acquire)
}

pub fn syscall_dispatcher() -> usize {
    VEH_SYSCALL_DISPATCHER.load(Ordering::Acquire)
}

pub fn set_syscall_dispatcher(addr: usize) {
    VEH_SYSCALL_DISPATCHER.store(addr, Ordering::Release);
}

pub fn synchronous_signal_dispatcher() -> usize {
    VEH_SYNC_SIGNAL_DISPATCHER.load(Ordering::Acquire)
}

pub fn set_synchronous_signal_dispatcher(addr: usize) {
    VEH_SYNC_SIGNAL_DISPATCHER.store(addr, Ordering::Release);
}

pub fn mapping_fault_dispatcher() -> usize {
    VEH_MAPPING_FAULT_DISPATCHER.load(Ordering::Acquire)
}

pub fn set_mapping_fault_dispatcher(addr: usize) {
    VEH_MAPPING_FAULT_DISPATCHER.store(addr, Ordering::Release);
}

/// Installs a handler that names the faulting address before exiting.
///
/// A guest fault is otherwise indistinguishable from a loader bug: the process
/// dies with `0xC0000005` and nothing else. Knowing the address, and whether it
/// falls inside a mapped object, decides in one step whether the guest
/// dereferenced something the linker got wrong or something it computed itself.
pub(crate) fn install_fault_reporter(
    canary_target: usize,
    thread_pointer_target: usize,
    host_transition_target: usize,
    scratch_target: usize,
) {
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, SetUnhandledExceptionFilter,
    };
    const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;

    // Linux x86_64 `gregset_t` indices from <sys/ucontext.h>.  HotSpot reads
    // RIP/RSP/RBP at their ABI offsets and edits RIP to recover from guarded
    // memory probes, so this must be the real Linux layout rather than a
    // loader-private approximation.
    const REG_R8: usize = 0;
    const REG_R9: usize = 1;
    const REG_R10: usize = 2;
    const REG_R11: usize = 3;
    const REG_R12: usize = 4;
    const REG_R13: usize = 5;
    const REG_R14: usize = 6;
    const REG_R15: usize = 7;
    const REG_RDI: usize = 8;
    const REG_RSI: usize = 9;
    const REG_RBP: usize = 10;
    const REG_RBX: usize = 11;
    const REG_RDX: usize = 12;
    const REG_RAX: usize = 13;
    const REG_RCX: usize = 14;
    const REG_RSP: usize = 15;
    const REG_RIP: usize = 16;
    const REG_EFL: usize = 17;
    const REG_CSGSFS: usize = 18;
    const REG_ERR: usize = 19;
    const REG_TRAPNO: usize = 20;
    const REG_OLDMASK: usize = 21;
    const REG_CR2: usize = 22;

    #[repr(C)]
    struct LinuxStack {
        sp: *mut core::ffi::c_void,
        flags: i32,
        _padding: i32,
        size: usize,
    }

    #[repr(C)]
    struct LinuxMContext {
        gregs: [u64; 23],
        fpregs: *mut core::ffi::c_void,
        reserved: [u64; 8],
    }

    /// glibc's Linux x86_64 `ucontext_t` layout.
    ///
    /// In particular, `gregs[REG_RBP]`, `gregs[REG_RSP]` and
    /// `gregs[REG_RIP]` land at 0x78, 0xa0 and 0xa8.  Those offsets are part
    /// of the guest ABI and are used directly by native runtimes.
    #[repr(C)]
    struct LinuxUContext {
        flags: u64,
        link: *mut LinuxUContext,
        stack: LinuxStack,
        mcontext: LinuxMContext,
        sigmask: [u64; 16],
        fpregs_memory: [u8; 512],
        shadow_stack: [u64; 4],
    }

    impl LinuxUContext {
        fn from_windows(ctx: &CONTEXT, fault_address: u64) -> Self {
            let mut result = Self {
                flags: 0,
                link: ptr::null_mut(),
                stack: LinuxStack {
                    sp: ptr::null_mut(),
                    flags: 2, // SS_DISABLE; sigaltstack state is provider-owned.
                    _padding: 0,
                    size: 0,
                },
                mcontext: LinuxMContext {
                    gregs: [0; 23],
                    fpregs: ptr::null_mut(),
                    reserved: [0; 8],
                },
                sigmask: [0; 16],
                fpregs_memory: [0; 512],
                shadow_stack: [0; 4],
            };
            let g = &mut result.mcontext.gregs;
            g[REG_R8] = ctx.R8;
            g[REG_R9] = ctx.R9;
            g[REG_R10] = ctx.R10;
            g[REG_R11] = ctx.R11;
            g[REG_R12] = ctx.R12;
            g[REG_R13] = ctx.R13;
            g[REG_R14] = ctx.R14;
            g[REG_R15] = ctx.R15;
            g[REG_RDI] = ctx.Rdi;
            g[REG_RSI] = ctx.Rsi;
            g[REG_RBP] = ctx.Rbp;
            g[REG_RBX] = ctx.Rbx;
            g[REG_RDX] = ctx.Rdx;
            g[REG_RAX] = ctx.Rax;
            g[REG_RCX] = ctx.Rcx;
            g[REG_RSP] = ctx.Rsp;
            g[REG_RIP] = ctx.Rip;
            g[REG_EFL] = u64::from(ctx.EFlags);
            g[REG_CSGSFS] =
                u64::from(ctx.SegCs) | (u64::from(ctx.SegGs) << 16) | (u64::from(ctx.SegFs) << 32);
            g[REG_ERR] = 0;
            g[REG_TRAPNO] = 0;
            g[REG_OLDMASK] = kinakaze_vfs::signal::blocked_mask();
            g[REG_CR2] = fault_address;
            result.sigmask[0] = kinakaze_vfs::signal::blocked_mask();
            unsafe {
                ptr::copy_nonoverlapping(
                    (&raw const ctx.Anonymous.FltSave).cast::<u8>(),
                    result.fpregs_memory.as_mut_ptr(),
                    512,
                );
            }
            result.mcontext.fpregs = result.fpregs_memory.as_mut_ptr().cast();
            result
        }

        fn copy_to_windows(&self, ctx: &mut CONTEXT) {
            let g = &self.mcontext.gregs;
            ctx.R8 = g[REG_R8];
            ctx.R9 = g[REG_R9];
            ctx.R10 = g[REG_R10];
            ctx.R11 = g[REG_R11];
            ctx.R12 = g[REG_R12];
            ctx.R13 = g[REG_R13];
            ctx.R14 = g[REG_R14];
            ctx.R15 = g[REG_R15];
            ctx.Rdi = g[REG_RDI];
            ctx.Rsi = g[REG_RSI];
            ctx.Rbp = g[REG_RBP];
            ctx.Rbx = g[REG_RBX];
            ctx.Rdx = g[REG_RDX];
            ctx.Rax = g[REG_RAX];
            ctx.Rcx = g[REG_RCX];
            ctx.Rsp = g[REG_RSP];
            ctx.Rip = g[REG_RIP];
            ctx.EFlags = g[REG_EFL] as u32;
            unsafe {
                ptr::copy_nonoverlapping(
                    self.fpregs_memory.as_ptr(),
                    (&raw mut ctx.Anonymous.FltSave).cast::<u8>(),
                    512,
                );
            }
        }
    }

    /// The common 16-byte header plus the fault member of Linux siginfo's
    /// 112-byte union.  `address` is consequently at the ABI offset 0x10.
    #[repr(C, align(8))]
    struct LinuxSigInfo {
        signal: i32,
        errno: i32,
        code: i32,
        _padding: i32,
        payload: [u8; 112],
    }

    impl LinuxSigInfo {
        fn fault(signal: i32, code: i32, address: u64) -> Self {
            let mut result = Self {
                signal,
                errno: 0,
                code,
                _padding: 0,
                payload: [0; 112],
            };
            result.payload[..8].copy_from_slice(&address.to_ne_bytes());
            result
        }
    }

    fn synchronous_signal(code: u32, target: u64, access: usize) -> crate::fault_action::Action {
        let mut mapping_result = 0;
        if code == 0xc000_0005 {
            let callback = VEH_MAPPING_FAULT_DISPATCHER.load(Ordering::Acquire);
            if callback != 0 {
                // The retained canonical provider owns mapping geometry.
                // The classifier cannot allocate or take a registry lock.
                // A conversion wait must release its index reader first,
                // hold no mapping/fork lock, and resolve before retrying.
                let classify: unsafe extern "sysv64" fn(usize, usize) -> i32 =
                    unsafe { std::mem::transmute(callback) };
                mapping_result = unsafe { classify(target as usize, access) };
            }
        }
        crate::fault_action::classify(code, target, mapping_result)
    }

    fn address_is_in_host_module(address: usize) -> bool {
        use windows_sys::Win32::Foundation::HMODULE;
        use windows_sys::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleHandleExW,
        };

        let mut module: HMODULE = std::ptr::null_mut();
        unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address as *const u16,
                &raw mut module,
            ) != 0
                && !module.is_null()
        }
    }

    /// Enters the real vectored-exception implementation on a private host
    /// stack lane.
    ///
    /// Windows starts a VEH callback on the interrupted stack.  That stack
    /// can be a two-kilobyte Go goroutine stack (or another managed runtime's
    /// equivalent), while decoding and translating one previously unseen
    /// instruction can consume much more native stack.  Merely moving that
    /// work into a non-inlined Rust function does not change RSP: it still
    /// overwrites the guest stack and leaves native return addresses in a
    /// runtime's free-stack links.
    ///
    /// The direct TEB slot published by the loader names the same per-thread
    /// transition block used by syscall trampolines.  A separate lane makes
    /// nested exceptions and exceptions raised from a provider deterministic.
    /// Missing or exhausted transition state cannot safely run arbitrary host
    /// code, so the handler declines the exception and lets Windows' normal
    /// unhandled path terminate it instead of falling back to the guest stack.
    #[unsafe(naked)]
    unsafe extern "system" fn handler(_info: *mut EXCEPTION_POINTERS) -> i32 {
        core::arch::naked_asm!(
            // VEH_HOST_TRANSITION_TARGET is a direct gs byte offset, not a
            // Windows TLS index.  Reading it this way requires no Rust frame
            // or API call on the interrupted guest stack.
            "mov rdx, qword ptr [rip + {transition_target}]",
            "test rdx, rdx",
            "jz 2f",
            "mov rdx, qword ptr gs:[rdx]",
            "test rdx, rdx",
            "jz 2f",
            "mov r8, qword ptr [rdx + {used}]",
            "cmp r8, {last_lane}",
            "ja 2f",
            "mov r9, qword ptr [rdx + {stack_top}]",
            "sub r9, r8",
            "add qword ptr [rdx + {used}], {lane}",
            "and r9, -16",

            // Preserve all state needed after handler_body in the home area
            // and private words of the selected host lane.  Only volatile
            // registers have been touched, so the callback still obeys the
            // Windows x64 ABI without pushing onto the guest stack.
            "mov r10, rsp",
            "mov rsp, r9",
            "sub rsp, 96",
            "mov qword ptr [rsp + 32], r10",
            "mov qword ptr [rsp + 40], rdx",
            "mov qword ptr [rsp + 48], r8",
            "mov r10, qword ptr gs:[0x08]",
            "mov qword ptr [rsp + 56], r10",
            "mov r10, qword ptr gs:[0x10]",
            "mov qword ptr [rsp + 64], r10",
            "mov qword ptr gs:[0x08], r9",
            "xor r10d, r10d",
            "mov qword ptr gs:[0x10], r10",

            // RCX still holds the EXCEPTION_POINTERS argument.  RSP is
            // 16-byte aligned before CALL and the first 32 bytes are the
            // mandatory Windows home area.
            "call {body}",

            // EAX is the handler result and none of the following moves
            // clobber it.  Restore the interrupted TEB description, release
            // exactly this nested lane, and return on the original stack.
            "mov r10, qword ptr [rsp + 56]",
            "mov qword ptr gs:[0x08], r10",
            "mov r10, qword ptr [rsp + 64]",
            "mov qword ptr gs:[0x10], r10",
            "mov rdx, qword ptr [rsp + 40]",
            "mov r8, qword ptr [rsp + 48]",
            "mov qword ptr [rdx + {used}], r8",
            "mov r10, qword ptr [rsp + 32]",
            "mov rsp, r10",
            "ret",

            // EXCEPTION_CONTINUE_SEARCH.  The transition block is mandatory
            // for guest execution; running the translator in-place would be
            // memory corruption disguised as a compatibility fallback.
            "2:",
            "xor eax, eax",
            "ret",
            transition_target = sym VEH_HOST_TRANSITION_TARGET,
            body = sym handler_body,
            stack_top = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET,
            used = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_USED_OFFSET,
            lane = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE,
            last_lane = const kinakaze_tls::thread_pointer::HOST_CALL_STACK_SIZE
                - kinakaze_tls::thread_pointer::HOST_CALL_STACK_LANE_SIZE,
        )
    }

    unsafe extern "system" fn handler_body(info: *mut EXCEPTION_POINTERS) -> i32 {
        if VEH_REPORT_HEALS.load(Ordering::Relaxed) {
            // SAFETY: the OS passes live exception records to VEH callbacks.
            let record = unsafe { (*info).ExceptionRecord };
            let context = unsafe { (*info).ContextRecord };
            let (code, address, target) = unsafe {
                (
                    (*record).ExceptionCode,
                    (*record).ExceptionAddress as usize,
                    (*record).ExceptionInformation[1],
                )
            };
            fault_report!(
                "kinakaze: [VEH handler entry] code={code:#x} at {address:#x} target={target:#x}"
            );
            if code as u32 == EXCEPTION_ACCESS_VIOLATION as u32 && target != 0 {
                use core::mem::{size_of, zeroed};
                use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
                let mut page: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
                let found = unsafe {
                    VirtualQuery(
                        target as *const core::ffi::c_void,
                        &mut page,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                if found != 0 {
                    fault_report!(
                        "kinakaze: [VEH target page] allocation={:?} base={:?} size={:#x} state={:#x} protect={:#x} type={:#x}",
                        page.AllocationBase,
                        page.BaseAddress,
                        page.RegionSize,
                        page.State,
                        page.Protect,
                        page.Type,
                    );
                }
            }
            use windows_sys::Win32::Foundation::HMODULE;
            use windows_sys::Win32::System::LibraryLoader::{
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleExW,
            };
            let mut module: HMODULE = ptr::null_mut();
            let _ = unsafe {
                GetModuleHandleExW(
                    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                        | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                    address as *const u16,
                    &raw mut module,
                )
            };
            if !context.is_null() {
                let context = unsafe { &*context };
                fault_report!(
                    "kinakaze: [VEH context] module={:#x} rva={:#x} rip={:#x} rsp={:#x} rbp={:#x} rax={:#x} rcx={:#x} rdx={:#x} r8={:#x} r9={:#x}",
                    module as usize,
                    address.wrapping_sub(module as usize),
                    context.Rip,
                    context.Rsp,
                    context.Rbp,
                    context.Rax,
                    context.Rcx,
                    context.Rdx,
                    context.R8,
                    context.R9,
                );
                if context.Rsp != 0 {
                    // An unhandled overflow can leave RSP in reserved memory.
                    // Diagnostics must not recursively fault while reading it.
                    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
                    use windows_sys::Win32::System::Threading::GetCurrentProcess;
                    let mut words = [0usize; 4];
                    if unsafe {
                        ReadProcessMemory(
                            GetCurrentProcess(),
                            context.Rsp as *const core::ffi::c_void,
                            words.as_mut_ptr().cast(),
                            core::mem::size_of_val(&words),
                            core::ptr::null_mut(),
                        )
                    } != 0
                    {
                        fault_report!("kinakaze: [VEH stack] {words:#x?}");
                    }
                }
            }
        }
        unsafe { handler_impl(info) }
    }

    #[inline(never)]
    unsafe extern "system" fn handler_impl(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the OS passes a valid pointer for the duration of the call.
        let record = unsafe { (*info).ExceptionRecord };
        // SAFETY: as above.
        let (code, address, parameters) = unsafe {
            (
                (*record).ExceptionCode,
                (*record).ExceptionAddress as usize,
                (*record).ExceptionInformation,
            )
        };
        // Vectored handlers also see faults raised by ntdll, the Rust CRT
        // and PE provider code.  Those are not guest instructions and must
        // continue through normal Windows exception handling.  Retrying a
        // native AV after touching guest TLS only repeats the same fault and
        // turns one abort into an infinite exception loop.
        if code as u32 == EXCEPTION_ACCESS_VIOLATION as u32 && address_is_in_host_module(address) {
            return 0;
        }

        // AOT handles instructions present while the ELF image is linked.
        // This VEH path is the dynamic fallback for generated code, late
        // mappings and any instruction the AOT walk could not safely prove.
        if code as u32 == EXCEPTION_ACCESS_VIOLATION as u32 && parameters[0] != 8 {
            let ctx = unsafe { (*info).ContextRecord };
            if !ctx.is_null()
                && unsafe {
                    crate::execution::instruction_trampoline::first_fs_instruction_len(
                        (*ctx).Rip as usize,
                    )
                }
                .is_some()
                && let Some(thread_pointer) = kinakaze_tls::restore_current_thread_static_tls()
            {
                if VEH_REPORT_HEALS.load(Ordering::Relaxed) {
                    fault_report!(
                        "kinakaze: VEH restored guest FS={thread_pointer:#x} before retrying {:#x}",
                        unsafe { (*ctx).Rip }
                    );
                }
                return EXCEPTION_CONTINUE_EXECUTION;
            }
        }

        if code as u32 == 0xc000_001d {
            let ctx = unsafe { (*info).ContextRecord };
            if !ctx.is_null() {
                let rip = unsafe { (*ctx).Rip as usize };
                if let Some(target) = crate::execution::guest_gs::redirect(rip) {
                    unsafe { (*ctx).Rip = target as u64 };
                    return EXCEPTION_CONTINUE_EXECUTION;
                }
                if rip != 0 {
                    let buf = unsafe { *(rip as *const [u8; 2]) };
                    if buf == [0x0f, 0x05] || buf == [0x0f, 0x0b] {
                        let dispatcher = VEH_SYSCALL_DISPATCHER.load(Ordering::Acquire);
                        if dispatcher != 0 {
                            let nr = unsafe { (*ctx).Rax as i64 };
                            let a1 = unsafe { (*ctx).Rdi };
                            let a2 = unsafe { (*ctx).Rsi };
                            let a3 = unsafe { (*ctx).Rdx };
                            let a4 = unsafe { (*ctx).R10 };
                            let a5 = unsafe { (*ctx).R8 };
                            let a6 = unsafe { (*ctx).R9 };
                            let previous_context =
                                ACTIVE_TRAP_CONTEXT.with(|slot| slot.replace(ctx as usize));
                            let call = TrapSyscallCall {
                                dispatcher,
                                transition: kinakaze_tls::host_transition_pointer(),
                                number: nr,
                                arguments: [a1, a2, a3, a4, a5, a6],
                            };
                            // SAFETY: the transition block and dispatcher are
                            // process-lifetime registrations. The helper copies
                            // every argument before leaving this handler stack.
                            let res = unsafe { dispatch_trap_on_host_stack(&raw const call) };
                            ACTIVE_TRAP_CONTEXT.with(|slot| slot.set(previous_context));
                            unsafe {
                                (*ctx).Rax = res as u64;
                                (*ctx).Rip += 2;
                            }
                            // A Windows transition made by the provider may
                            // have cleared FS. The process TEB slot remains
                            // authoritative, so restore it before exception
                            // continuation re-enters guest code. Do not grow a
                            // two-byte trap into a five-byte detour here: the
                            // AOT pass selected the trap precisely because the
                            // following bytes contain an independent entry.
                            let _ = kinakaze_tls::restore_current_thread_static_tls();
                            if VEH_REPORT_HEALS.load(Ordering::Relaxed) {
                                fault_report!(
                                    "kinakaze: VEH dynamically executed syscall {nr} at {rip:#x} -> {res}"
                                );
                            }
                            return EXCEPTION_CONTINUE_EXECUTION;
                        }
                    }
                }
            }
        }

        // Hardware faults are synchronous Linux signals, not pending
        // process-directed signals.  Run the installed guest sigaction now
        // and translate any ucontext edits (most importantly a recovered
        // RIP) back into the Windows exception context before resuming.
        let fault_target = if code as u32 == EXCEPTION_ACCESS_VIOLATION as u32 {
            parameters[1] as u64
        } else {
            address as u64
        };
        let fault = synchronous_signal(code as u32, fault_target, parameters[0]);
        if fault == crate::fault_action::Action::Retry {
            return EXCEPTION_CONTINUE_EXECUTION;
        }
        if let crate::fault_action::Action::Signal {
            number: signal,
            code: signal_code,
        } = fault
        {
            let ctx = unsafe { (*info).ContextRecord };
            if !ctx.is_null() && kinakaze_tls::install_current_thread_static_tls().is_ok() {
                let windows_context = unsafe { &mut *ctx };
                let original_rip = windows_context.Rip;
                let mut linux_context = LinuxUContext::from_windows(windows_context, fault_target);
                // `from_windows` returns by value, so establish this internal
                // pointer only after the record is in its final stack slot.
                linux_context.mcontext.fpregs = linux_context.fpregs_memory.as_mut_ptr().cast();
                let mut siginfo = LinuxSigInfo::fault(signal, signal_code, fault_target);
                let dispatcher = VEH_SYNC_SIGNAL_DISPATCHER.load(Ordering::Acquire);
                let handled = if dispatcher == 0 {
                    false
                } else {
                    let deliver: unsafe extern "sysv64" fn(
                        i32,
                        *mut core::ffi::c_void,
                        *mut core::ffi::c_void,
                    ) -> i32 = unsafe { core::mem::transmute(dispatcher) };
                    unsafe {
                        deliver(
                            signal,
                            (&raw mut siginfo).cast(),
                            (&raw mut linux_context).cast(),
                        ) != 0
                    }
                };
                if handled {
                    linux_context.copy_to_windows(windows_context);
                    if VEH_REPORT_HEALS.load(Ordering::Relaxed) {
                        fault_report!(
                            "kinakaze: delivered synchronous signal {signal} at {original_rip:#x}; resume={:#x}",
                            windows_context.Rip
                        );
                    }
                    return EXCEPTION_CONTINUE_EXECUTION;
                }
            }
        }

        // A vectored handler sees *every* exception, including ones that are
        // about to be caught and are part of normal operation. Reporting those
        // is worse than saying nothing: a run that succeeded printed six copies
        // of `0x40010006`, which is `DBG_PRINTEXCEPTION_C` — a graphics driver
        // calling `OutputDebugString`. It read as six crashes.
        //
        // Bits 30-31 of an NTSTATUS are its severity, and only `0b11` is an
        // error. Everything else is informational or a warning and is not this
        // reporter's business.
        const SEVERITY_ERROR: u32 = 0b11;
        if (code as u32) >> 30 != SEVERITY_ERROR {
            return 0;
        }

        if VEH_REPORT_HEALS.load(Ordering::Relaxed) {
            fault_report!("kinakaze: [VEH triggered] code={code:#x} at address={address:#x}");
        }

        if (code as u32) == (EXCEPTION_ACCESS_VIOLATION as u32) {
            // The first parameter says read (0) or write (1); the second is the
            // address that was inaccessible, which is the useful one.
            let operation = match parameters[0] {
                0 => "read",
                1 => "write",
                8 => "execute",
                other => {
                    fault_report!("kinakaze: fault with unknown operation {other}");
                    "unknown"
                }
            };
            fault_report!(
                "kinakaze: guest fault: {operation} of {:#x} at instruction {address:#x}",
                parameters[1]
            );
            let ctx = unsafe { (*info).ContextRecord };
            if !ctx.is_null() {
                unsafe {
                    fault_report!(
                        "kinakaze:   RAX={:#x} RBX={:#x} RCX={:#x} RDX={:#x}\n            RSI={:#x} RDI={:#x} RBP={:#x} RSP={:#x}\n            R8={:#x} R9={:#x} R10={:#x} R11={:#x}\n            R12={:#x} R13={:#x} R14={:#x} R15={:#x}\n            RIP={:#x}",
                        (*ctx).Rax,
                        (*ctx).Rbx,
                        (*ctx).Rcx,
                        (*ctx).Rdx,
                        (*ctx).Rsi,
                        (*ctx).Rdi,
                        (*ctx).Rbp,
                        (*ctx).Rsp,
                        (*ctx).R8,
                        (*ctx).R9,
                        (*ctx).R10,
                        (*ctx).R11,
                        (*ctx).R12,
                        (*ctx).R13,
                        (*ctx).R14,
                        (*ctx).R15,
                        (*ctx).Rip,
                    );
                    let guest_tp =
                        crate::teb_slot_value(VEH_THREAD_POINTER_TARGET.load(Ordering::Acquire))
                            .unwrap_or(0);
                    let transition =
                        crate::teb_slot_value(VEH_HOST_TRANSITION_TARGET.load(Ordering::Acquire))
                            .unwrap_or(0);
                    let actual_fs = if kinakaze_tls::thread_pointer::supported() {
                        kinakaze_tls::thread_pointer::read_fs_base() as usize
                    } else {
                        0
                    };
                    let go_g = if guest_tp >= core::mem::size_of::<usize>() {
                        ((guest_tp - core::mem::size_of::<usize>()) as *const usize)
                            .read_unaligned()
                    } else {
                        0
                    };
                    let host_stack_top = if transition != 0 {
                        ((transition + kinakaze_tls::thread_pointer::HOST_CALL_STACK_POINTER_OFFSET)
                            as *const usize)
                            .read_unaligned()
                    } else {
                        0
                    };
                    fault_report!(
                        "kinakaze:   guest_tp={guest_tp:#x} actual_fs={actual_fs:#x} fs[-8]={go_g:#x} transition={transition:#x} host_stack_top={host_stack_top:#x}"
                    );
                    if std::env::var_os("KINAKAZE_FORK_TRACE").is_some() && host_stack_top >= 192 {
                        // The raw-syscall bridge leaves its most recent frame
                        // intact at the top of the private transition stack.
                        // Comparing the saved Linux register with the fault
                        // context distinguishes an entry-state problem from a
                        // restore/clobber problem without changing guest state.
                        let frame = host_stack_top - 192;
                        let saved_r14 = ((frame + 0x90) as *const usize).read_unaligned();
                        let saved_r12 = ((frame + 0x80) as *const usize).read_unaligned();
                        let saved_r13 = ((frame + 0x88) as *const usize).read_unaligned();
                        let saved_r15 = ((frame + 0x98) as *const usize).read_unaligned();
                        fault_report!(
                            "kinakaze:   last syscall frame={frame:#x} R12={saved_r12:#x} R13={saved_r13:#x} R14={saved_r14:#x} R15={saved_r15:#x}"
                        );
                    }
                    if let Some((stack, count)) = read_words((*ctx).Rsp as usize) {
                        fault_report!("kinakaze:   stack[0..{count}]={:#x?}", &stack[..count]);
                        for (i, val) in stack[..count].iter().enumerate() {
                            let mod_name = containing_module(*val);
                            if !mod_name.contains("no loaded module") {
                                fault_report!("kinakaze:     stack[{i}] = {val:#x} -> {mod_name}");
                            }
                        }
                    } else {
                        fault_report!("kinakaze:   stack unreadable at RSP={:#x}", (*ctx).Rsp);
                    }
                    // A register holding a bad pointer is the usual reason for a
                    // fault, but the value alone never says whether it was ever
                    // mapped: 0x40000fc97900 could be a live heap object or pure
                    // garbage. Naming the region it lands in separates them, and
                    // also reveals structures the guest never told us about - a
                    // 4 GiB committed block is V8's pointer-compression cage, not
                    // something any mmap trace mentions.
                    if std::env::var_os("KINAKAZE_FAULT_ADDRS").is_some() {
                        for (name, value) in [
                            ("RAX", (*ctx).Rax),
                            ("RBX", (*ctx).Rbx),
                            ("RCX", (*ctx).Rcx),
                            ("RDX", (*ctx).Rdx),
                            ("RSI", (*ctx).Rsi),
                            ("RDI", (*ctx).Rdi),
                            ("RBP", (*ctx).Rbp),
                            ("RSP", (*ctx).Rsp),
                            ("R8", (*ctx).R8),
                            ("R9", (*ctx).R9),
                            ("R12", (*ctx).R12),
                            ("R13", (*ctx).R13),
                            ("R14", (*ctx).R14),
                            ("R15", (*ctx).R15),
                        ] {
                            match describe_region(value as usize) {
                                Some(text) => {
                                    fault_report!("kinakaze:   reg {name}={value:#x} -> {text}");
                                    if let Some((words, count)) = read_words(value as usize) {
                                        fault_report!(
                                            "kinakaze:     first {count} words = {:#x?}",
                                            &words[..count]
                                        );
                                    }
                                }
                                None => {
                                    fault_report!("kinakaze:   reg {name}={value:#x} -> unmapped")
                                }
                            }
                        }
                    }
                }
            }
            report_region("fault target", parameters[1]);
            report_region("instruction", address);
            if address >= 16
                && let Some((inst_bytes, count)) = read_bytes::<48>(address - 16)
            {
                fault_report!(
                    "kinakaze:   raw bytes at fault RIP-16..+32: {:02x?}",
                    &inst_bytes[..count]
                );
            }
        } else {
            if code as u32 == 0xe06d_7363 || code as u32 == 0x406d_1388 {
                // C++/MSVC exceptions and debugger thread naming exceptions
                // are handled by SEH/C++ catch blocks; ignore in VEH.
                return 0;
            }
            let description = match code as u32 {
                0x8000_0003 => " (breakpoint)",
                0xc000_001d => " (illegal instruction)",
                0xc000_008e => " (float divide by zero)",
                0xc000_0094 => " (integer divide by zero)",
                0xc000_00fd => " (stack overflow)",
                _ => "",
            };
            fault_report!(
                "kinakaze: guest exception {:#x}{description} at {address:#x}",
                code as u32
            );
            let ctx = unsafe { (*info).ContextRecord };
            if !ctx.is_null() {
                unsafe {
                    fault_report!(
                        "kinakaze:   RAX={:#x} RBX={:#x} RCX={:#x} RDX={:#x}\n            RSI={:#x} RDI={:#x} RBP={:#x} RSP={:#x}\n            R8={:#x} R9={:#x} R10={:#x} R11={:#x}\n            R12={:#x} R13={:#x} R14={:#x} R15={:#x}\n            RIP={:#x}",
                        (*ctx).Rax,
                        (*ctx).Rbx,
                        (*ctx).Rcx,
                        (*ctx).Rdx,
                        (*ctx).Rsi,
                        (*ctx).Rdi,
                        (*ctx).Rbp,
                        (*ctx).Rsp,
                        (*ctx).R8,
                        (*ctx).R9,
                        (*ctx).R10,
                        (*ctx).R11,
                        (*ctx).R12,
                        (*ctx).R13,
                        (*ctx).R14,
                        (*ctx).R15,
                        (*ctx).Rip,
                    );
                }
            }
            if let Some((bytes, count)) = read_bytes::<16>(address) {
                fault_report!("kinakaze:   bytes at RIP: {:02x?}", &bytes[..count]);
            }
        }
        fault_report!("kinakaze:   in {}", containing_module(address));
        // EXCEPTION_CONTINUE_SEARCH: let the default handler terminate as usual,
        // now that the diagnosis has been printed.
        0
    }

    fn describe_region(address: usize) -> Option<String> {
        use core::mem::{size_of, zeroed};
        use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        let found = unsafe {
            VirtualQuery(
                address as *const core::ffi::c_void,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if found == 0 {
            None
        } else {
            Some(format!(
                "allocation={:?} base={:?} size={:#x} state={:#x} protect={:#x}",
                info.AllocationBase, info.BaseAddress, info.RegionSize, info.State, info.Protect
            ))
        }
    }

    /// Reads diagnostic memory without dereferencing a guest pointer in the
    /// exception handler. `ReadProcessMemory` reports a short/invalid range
    /// as failure instead of raising a second access violation and destroying
    /// the original fault context.
    fn read_words(address: usize) -> Option<([usize; 8], usize)> {
        let (bytes, bytes_read) = read_bytes::<{ 8 * core::mem::size_of::<usize>() }>(address)?;
        let mut words = [0usize; 8];
        for (index, chunk) in bytes[..bytes_read]
            .chunks_exact(core::mem::size_of::<usize>())
            .enumerate()
        {
            words[index] = usize::from_ne_bytes(chunk.try_into().ok()?);
        }
        Some((words, bytes_read / core::mem::size_of::<usize>()))
    }

    fn read_bytes<const N: usize>(address: usize) -> Option<([u8; N], usize)> {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let mut bytes = [0u8; N];
        let mut bytes_read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                address as *const core::ffi::c_void,
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                &raw mut bytes_read,
            )
        };
        (ok != 0 || bytes_read != 0).then_some((bytes, bytes_read.min(N)))
    }

    fn report_region(label: &str, address: usize) {
        match describe_region(address) {
            Some(text) => fault_report!("kinakaze:   {label}: {text}"),
            None => fault_report!("kinakaze:   {label}: VirtualQuery failed"),
        }
    }

    /// Names the loaded module containing `address`.
    ///
    /// This is what turns an address into a diagnosis. A fault inside
    /// `libvulkan.dll` and one inside the guest image mean entirely different
    /// things, and the bare address does not say which — but the module does, in
    /// one line, without a debugger.
    fn containing_module(address: usize) -> String {
        use windows_sys::Win32::Foundation::HMODULE;
        use windows_sys::Win32::System::LibraryLoader::{
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            GetModuleFileNameW, GetModuleHandleExW,
        };

        let mut module: HMODULE = std::ptr::null_mut();
        // The refcount flag matters: this runs on a dying process and must not
        // leave a reference behind that changes teardown behaviour.
        // SAFETY: the address is passed as a lookup key, never dereferenced.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address as *const u16,
                &raw mut module,
            )
        };
        if found == 0 || module.is_null() {
            // No module means the address is in the guest's own mapping, which the
            // loader placed with `VirtualAlloc` and Windows therefore does not
            // know as a module. That is itself the answer.
            return format!("no loaded module — likely the guest image itself ({address:#x})");
        }

        let mut buffer = [0u16; 512];
        // SAFETY: `module` is a live handle and the buffer bound is its true length.
        let length =
            unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
        if length == 0 {
            return format!("module at {:?}", module);
        }
        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        let name = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
        let offset = address - module as usize;
        format!("{name}+{offset:#x}")
    }

    /// Reports an exception that no handler claimed.
    ///
    /// This is the complement to the vectored handler: it runs only when nothing
    /// caught the exception, so anything reaching it really is fatal. That makes
    /// it the right place for C++ and Rust exceptions, which the vectored handler
    /// deliberately ignores because it cannot know whether a handler exists.
    unsafe extern "system" fn unhandled(info: *const EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the OS passes a valid pointer for the duration of the call.
        let record = unsafe { (*info).ExceptionRecord };
        // SAFETY: as above.
        let (code, address) = unsafe {
            (
                (*record).ExceptionCode as u32,
                (*record).ExceptionAddress as usize,
            )
        };
        let description = match code {
            0xe06d_7363 => " (an unhandled C++/Rust exception)",
            0xc000_0005 => " (access violation)",
            0xc000_001d => " (illegal instruction)",
            0xc000_0094 => " (integer divide by zero)",
            0xc000_00fd => " (stack overflow)",
            _ => "",
        };
        fault_report!("kinakaze: unhandled exception {code:#x}{description} at {address:#x}");
        fault_report!("kinakaze:   in {}", containing_module(address));
        // EXCEPTION_EXECUTE_HANDLER: terminate, now that it has been described.
        1
    }

    VEH_CANARY_TARGET.store(canary_target, Ordering::Release);
    VEH_THREAD_POINTER_TARGET.store(thread_pointer_target, Ordering::Release);
    VEH_HOST_TRANSITION_TARGET.store(host_transition_target, Ordering::Release);
    VEH_SCRATCH_TARGET.store(scratch_target, Ordering::Release);
    VEH_REPORT_HEALS.store(crate::report_traps_enabled(), Ordering::Relaxed);

    // SAFETY: the handler is a valid function with the documented signature; a
    // first-handler value of 1 runs it before anything else.
    let vectored = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    if crate::report_traps_enabled() {
        fault_report!("kinakaze: vectored exception reporter={vectored:?}");
    }
    // SAFETY: as above.
    unsafe { SetUnhandledExceptionFilter(Some(unhandled)) };
}

/// Enables verbose VEH diagnostics after the handler is installed.
///
/// Fork restoration uses an already-snapshotted flag so the first child
/// participant does not query environment state or allocate.
pub(crate) fn set_trap_reporting(enabled: bool) {
    VEH_REPORT_HEALS.store(enabled, Ordering::Relaxed);
}
