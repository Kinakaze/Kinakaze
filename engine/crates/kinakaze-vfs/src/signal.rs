//! Signal dispositions, delivery and `EINTR` generation.
//!
//! Windows has no signals, so this is a userspace implementation: `kill` records
//! a pending signal and wakes the target thread's interrupt event, which is what
//! breaks a blocking overlapped I/O wait. The interrupted call then runs any
//! handler and either restarts (`SA_RESTART`) or reports `EINTR`.
//!
//! Handlers therefore run at well-defined points — when a thread checks for
//! pending signals — rather than truly asynchronously. That is weaker than a
//! real kernel, but it is the part that makes `EINTR` and `SA_RESTART` behave
//! correctly, which is what portable programs actually depend on.

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::{EIO, interrupt};

/// Highest supported signal number.
pub const NSIG: usize = 65;

// Linux x86_64 signal numbers.
pub const SIGHUP: i32 = 1;
pub const SIGBUS: i32 = 7;
pub const SIGINT: i32 = 2;
pub const SIGQUIT: i32 = 3;
pub const SIGILL: i32 = 4;
pub const SIGABRT: i32 = 6;
pub const SIGFPE: i32 = 8;
pub const SIGKILL: i32 = 9;
pub const SIGSEGV: i32 = 11;
pub const SIGPIPE: i32 = 13;
pub const SIGALRM: i32 = 14;
pub const SIGTERM: i32 = 15;
pub const SIGCHLD: i32 = 17;
pub const SIGCONT: i32 = 18;
pub const SIGSTOP: i32 = 19;
pub const SIGTSTP: i32 = 20;
/// A background process read from its controlling terminal.
pub const SIGTTIN: i32 = 21;
/// A background process wrote to its controlling terminal.
pub const SIGTTOU: i32 = 22;
pub const SIGUSR1: i32 = 10;
pub const SIGUSR2: i32 = 12;
pub const SIGWINCH: i32 = 28;
pub const SIGURG: i32 = 23;

/// `sigaction` flags.
pub const SA_NOCLDSTOP: i32 = 1;
pub const SA_ONSTACK: i32 = 0x0800_0000;
pub const SA_RESTART: i32 = 0x1000_0000;
pub const SA_NODEFER: i32 = 0x4000_0000;
/// Linux defines this with the sign bit set, so it is negative as an `i32`.
pub const SA_RESETHAND: i32 = 0x8000_0000_u32 as i32;
pub const SA_SIGINFO: i32 = 4;

/// `sigprocmask` operations.
pub const SIG_BLOCK: i32 = 0;
pub const SIG_UNBLOCK: i32 = 1;
pub const SIG_SETMASK: i32 = 2;

/// `SIG_DFL` and `SIG_IGN` as the guest sees them.
pub const SIG_DFL: usize = 0;
pub const SIG_IGN: usize = 1;
/// `SIG_ERR`, returned by `signal` on failure.
pub const SIG_ERR: usize = usize::MAX;

/// A guest signal handler using the System V ABI.
#[cfg(target_arch = "x86_64")]
pub type Handler = unsafe extern "sysv64" fn(i32);

/// A three-argument handler selected by `SA_SIGINFO`.
///
/// The pointed-to records use the guest Linux ABI.  Keeping them opaque here
/// lets the loader translate a host exception context without making the VFS
/// depend on Win32 types.
#[cfg(target_arch = "x86_64")]
pub type SiginfoHandler = unsafe extern "sysv64" fn(i32, *mut c_void, *mut c_void);

#[cfg(not(target_arch = "x86_64"))]
pub type Handler = unsafe extern "C" fn(i32);

#[cfg(not(target_arch = "x86_64"))]
pub type SiginfoHandler = unsafe extern "C" fn(i32, *mut c_void, *mut c_void);

/// Linux x86_64 `siginfo_t` storage used for a cooperatively delivered signal.
/// The first three words are the stable public header; the remaining 116 bytes
/// are the signal-specific union and padding.
#[repr(C, align(16))]
struct PendingSigInfo {
    signal: i32,
    errno: i32,
    code: i32,
    payload: [u8; 116],
}

/// Capture credentials when tgkill queues the signal, not when its handler runs.
#[derive(Clone, Copy)]
struct SignalSender {
    pid: u32,
    uid: u32,
}

impl SignalSender {
    fn current() -> Self {
        Self {
            pid: crate::job::process_id(),
            uid: crate::credentials::real_uid(),
        }
    }

    fn fill(self, info: &mut PendingSigInfo) {
        // The x86_64 siginfo union starts at byte 16, after four padding bytes
        // following si_code. glibc's SIGSETXID handler checks si_pid and SI_TKILL.
        let pid = kinakaze_runtime::job::namespaces::visible(self.pid).unwrap_or(0);
        let uid = crate::user_namespace::visible(self.uid, false);
        info.payload[4..8].copy_from_slice(&pid.to_le_bytes());
        info.payload[8..12].copy_from_slice(&uid.to_le_bytes());
    }
}

/// The Linux x86_64 `stack_t` used by `sigaltstack` and `ucontext_t`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SignalStack {
    pub ss_sp: *mut c_void,
    pub ss_flags: i32,
    pub ss_size: usize,
}

pub const SS_ONSTACK: i32 = 1;
pub const SS_DISABLE: i32 = 2;
const MINSIGSTKSZ: usize = 2048;

impl Default for SignalStack {
    fn default() -> Self {
        Self {
            ss_sp: core::ptr::null_mut(),
            ss_flags: SS_DISABLE,
            ss_size: 0,
        }
    }
}

thread_local! {
    static ALT_STACK: Cell<SignalStack> = Cell::new(SignalStack {
        ss_sp: core::ptr::null_mut(),
        ss_flags: SS_DISABLE,
        ss_size: 0,
    });
    static ON_ALT_STACK: Cell<bool> = const { Cell::new(false) };
}

/// Installs or queries the calling thread's alternate signal stack.
pub fn sigaltstack(requested: Option<SignalStack>) -> Result<SignalStack, i32> {
    let mut previous = ALT_STACK.get();
    if ON_ALT_STACK.get() {
        previous.ss_flags |= SS_ONSTACK;
    }
    let Some(requested) = requested else {
        return Ok(previous);
    };
    if ON_ALT_STACK.get() {
        return Err(crate::EPERM);
    }
    if requested.ss_flags & SS_DISABLE != 0 {
        if requested.ss_flags & !SS_DISABLE != 0 {
            return Err(crate::EINVAL);
        }
        ALT_STACK.set(SignalStack::default());
        return Ok(previous);
    }
    if requested.ss_flags != 0
        || requested.ss_sp.is_null()
        || requested.ss_size < MINSIGSTKSZ
        || (requested.ss_sp as usize)
            .checked_add(requested.ss_size)
            .is_none()
    {
        return Err(crate::EINVAL);
    }
    ALT_STACK.set(SignalStack {
        ss_flags: 0,
        ..requested
    });
    Ok(previous)
}

/// Linux x86_64 `ucontext_t`, including its inline floating-point save area.
#[repr(C)]
struct PendingUContext {
    uc_flags: u64,
    uc_link: *mut c_void,
    uc_stack: SignalStack,
    gregs: [u64; 23],
    fpregs: *mut c_void,
    reserved: [u64; 8],
    sigmask: [u64; 16],
    fpstate: [u8; 512],
    ssp: [u64; 4],
}

impl PendingUContext {
    fn new(blocked: u64, stack: SignalStack, guest_stack: usize, guest_instruction: usize) -> Self {
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
        let mut context = Self {
            uc_flags: 0,
            uc_link: core::ptr::null_mut(),
            uc_stack: stack,
            gregs: [0; 23],
            fpregs: core::ptr::null_mut(),
            reserved: [0; 8],
            sigmask: [0; 16],
            fpstate: [0; 512],
            ssp: [0; 4],
        };
        context.gregs[REG_RSP] = guest_stack as u64;
        context.gregs[REG_RIP] = guest_instruction as u64;
        context.gregs[REG_EFL] = 0x202;
        context.sigmask[0] = blocked;
        if let Some(frame) = kinakaze_tls::thread_pointer::active_raw_syscall_frame_pointer() {
            let read = |offset| unsafe { ((frame + offset) as *const u64).read_unaligned() };
            use kinakaze_tls::thread_pointer::{
                RAW_SYSCALL_FRAME_R8_OFFSET, RAW_SYSCALL_FRAME_R9_OFFSET,
                RAW_SYSCALL_FRAME_R10_OFFSET, RAW_SYSCALL_FRAME_R12_OFFSET,
                RAW_SYSCALL_FRAME_R13_OFFSET, RAW_SYSCALL_FRAME_R14_OFFSET,
                RAW_SYSCALL_FRAME_R15_OFFSET, RAW_SYSCALL_FRAME_RAX_OFFSET,
                RAW_SYSCALL_FRAME_RBP_OFFSET, RAW_SYSCALL_FRAME_RBX_OFFSET,
                RAW_SYSCALL_FRAME_RDI_OFFSET, RAW_SYSCALL_FRAME_RDX_OFFSET,
                RAW_SYSCALL_FRAME_RSI_OFFSET,
            };
            context.gregs[REG_R8] = read(RAW_SYSCALL_FRAME_R8_OFFSET);
            context.gregs[REG_R9] = read(RAW_SYSCALL_FRAME_R9_OFFSET);
            context.gregs[REG_R10] = read(RAW_SYSCALL_FRAME_R10_OFFSET);
            // SYSCALL defines R11 as the saved user RFLAGS. The bridge does not
            // yet expose an architectural flags slot, so use the same valid
            // user-mode baseline published in REG_EFL rather than host-call
            // temporaries.
            context.gregs[REG_R11] = context.gregs[REG_EFL];
            context.gregs[REG_R12] = read(RAW_SYSCALL_FRAME_R12_OFFSET);
            context.gregs[REG_R13] = read(RAW_SYSCALL_FRAME_R13_OFFSET);
            context.gregs[REG_R14] = read(RAW_SYSCALL_FRAME_R14_OFFSET);
            context.gregs[REG_R15] = read(RAW_SYSCALL_FRAME_R15_OFFSET);
            context.gregs[REG_RDI] = read(RAW_SYSCALL_FRAME_RDI_OFFSET);
            context.gregs[REG_RSI] = read(RAW_SYSCALL_FRAME_RSI_OFFSET);
            context.gregs[REG_RBP] = read(RAW_SYSCALL_FRAME_RBP_OFFSET);
            context.gregs[REG_RBX] = read(RAW_SYSCALL_FRAME_RBX_OFFSET);
            context.gregs[REG_RDX] = read(RAW_SYSCALL_FRAME_RDX_OFFSET);
            context.gregs[REG_RAX] = read(RAW_SYSCALL_FRAME_RAX_OFFSET);
            context.gregs[REG_RCX] = guest_instruction as u64;
        }
        context
    }

    /// Repairs the ABI pointer after the context has reached its final address.
    ///
    /// `ucontext_t` embeds the floating-point image but `mcontext_t` points to
    /// it.  Setting that pointer in `new` would create a self-reference before
    /// Rust moves the returned value into the caller's stack frame.
    fn bind_internal_pointers(&mut self) {
        self.fpregs = self.fpstate.as_mut_ptr().cast();
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
unsafe extern "sysv64" fn call_siginfo_on_stack(
    _handler: SiginfoHandler,
    _signal: i32,
    _siginfo: *mut c_void,
    _context: *mut c_void,
    _stack_top: usize,
) {
    core::arch::naked_asm!(
        // Preserve bridge state on the host stack. Before calling guest code we
        // replace these temporaries with the interrupted callee-saved registers;
        // Go, JVMs and other runtimes keep thread state in those registers.
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov r12, rdi",
        "mov r13, rsi",
        "mov r14, rdx",
        "mov r15, rcx",
        "mov rbx, r8",
        "sub rsp, 8",
        "call {active_frame}",
        "mov rbp, rax",
        "call {restore_xstate}",
        "add rsp, 8",
        "mov r11, rsp",
        "mov rdi, r13",
        "mov rsi, r14",
        "mov rdx, r15",
        "mov rsp, rbx",
        "and rsp, -16",
        "sub rsp, 32",
        "mov [rsp], r11",
        "mov [rsp + 8], r12",
        "mov [rsp + 16], rbp",
        "test rbp, rbp",
        "jz 2f",
        "mov rax, [rsp + 16]",
        "mov rbx, [rax + {rbx_offset}]",
        "mov rbp, [rax + {rbp_offset}]",
        "mov r12, [rax + {r12_offset}]",
        "mov r13, [rax + {r13_offset}]",
        "mov r14, [rax + {r14_offset}]",
        "mov r15, [rax + {r15_offset}]",
        "2:",
        "call qword ptr [rsp + 8]",
        "mov rsp, [rsp]",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "ret",
        active_frame = sym kinakaze_tls::thread_pointer::active_raw_syscall_frame,
        restore_xstate = sym kinakaze_tls::thread_pointer::restore_active_guest_extended_state,
        rbx_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RBX_OFFSET,
        rbp_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RBP_OFFSET,
        r12_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R12_OFFSET,
        r13_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R13_OFFSET,
        r14_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R14_OFFSET,
        r15_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R15_OFFSET,
    )
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(naked)]
unsafe extern "sysv64" fn call_handler_on_stack(
    _handler: Handler,
    _signal: i32,
    _stack_top: usize,
) {
    core::arch::naked_asm!(
        "push rbx",
        "push rbp",
        "push r12",
        "push r13",
        "push r14",
        "push r15",
        "mov r12, rdi",
        "mov r13, rsi",
        "mov rbx, rdx",
        "sub rsp, 8",
        "call {active_frame}",
        "mov rbp, rax",
        "call {restore_xstate}",
        "add rsp, 8",
        "mov r11, rsp",
        "mov rdi, r13",
        "mov rsp, rbx",
        "and rsp, -16",
        "sub rsp, 32",
        "mov [rsp], r11",
        "mov [rsp + 8], r12",
        "mov [rsp + 16], rbp",
        "test rbp, rbp",
        "jz 2f",
        "mov rax, [rsp + 16]",
        "mov rbx, [rax + {rbx_offset}]",
        "mov rbp, [rax + {rbp_offset}]",
        "mov r12, [rax + {r12_offset}]",
        "mov r13, [rax + {r13_offset}]",
        "mov r14, [rax + {r14_offset}]",
        "mov r15, [rax + {r15_offset}]",
        "2:",
        "call qword ptr [rsp + 8]",
        "mov rsp, [rsp]",
        "pop r15",
        "pop r14",
        "pop r13",
        "pop r12",
        "pop rbp",
        "pop rbx",
        "ret",
        active_frame = sym kinakaze_tls::thread_pointer::active_raw_syscall_frame,
        restore_xstate = sym kinakaze_tls::thread_pointer::restore_active_guest_extended_state,
        rbx_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RBX_OFFSET,
        rbp_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_RBP_OFFSET,
        r12_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R12_OFFSET,
        r13_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R13_OFFSET,
        r14_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R14_OFFSET,
        r15_offset = const kinakaze_tls::thread_pointer::RAW_SYSCALL_FRAME_R15_OFFSET,
    )
}

unsafe fn invoke_handler(
    action: Action,
    handler: Handler,
    signal: i32,
    siginfo: *mut c_void,
    context: *mut c_void,
) {
    let alternate = ALT_STACK.get();
    let use_alternate = action.flags & SA_ONSTACK != 0
        && alternate.ss_flags & SS_DISABLE == 0
        && !ON_ALT_STACK.get();
    if use_alternate {
        let stack_top = alternate.ss_sp as usize + alternate.ss_size;
        ON_ALT_STACK.set(true);
        if action.flags & SA_SIGINFO != 0 {
            let handler = unsafe { core::mem::transmute::<Handler, SiginfoHandler>(handler) };
            unsafe { call_siginfo_on_stack(handler, signal, siginfo, context, stack_top) };
        } else {
            unsafe { call_handler_on_stack(handler, signal, stack_top) };
        }
        ON_ALT_STACK.set(false);
    } else if action.flags & SA_SIGINFO != 0 {
        let handler = unsafe { core::mem::transmute::<Handler, SiginfoHandler>(handler) };
        unsafe { handler(signal, siginfo, context) };
    } else {
        unsafe { handler(signal) };
    }
}

/// What to do when a signal arrives.
#[derive(Clone, Copy)]
pub enum Disposition {
    /// Terminate, or ignore for the signals whose default is to be ignored.
    Default,
    /// Explicitly ignored.
    Ignore,
    /// Run a guest handler.
    Handle(Handler, i32),
}

/// One signal's configuration.
#[derive(Clone, Copy)]
pub struct Action {
    pub disposition: Disposition,
    pub flags: i32,
    /// Signals blocked for the duration of the handler.
    pub mask: u64,
    /// Guest restorer address supplied with `SA_RESTORER`.
    ///
    /// Cooperative delivery does not call it, but Linux returns it from a
    /// later `sigaction` query and fork must preserve it.
    pub restorer: usize,
}

impl Default for Action {
    fn default() -> Self {
        Self {
            disposition: Disposition::Default,
            flags: 0,
            mask: 0,
            restorer: 0,
        }
    }
}

struct SignalState {
    actions: Vec<Action>,
    /// Threads that have blocked in an interruptible wait, by host thread ID.
    waiting: HashMap<u32, ()>,
    /// Signals directed by `tgkill` to one host thread rather than the process.
    thread_pending: HashMap<u32, u64>,
    thread_senders: HashMap<(u32, i32), SignalSender>,
    timer_pending: VecDeque<TimerSignal>,
}

#[derive(Clone)]
struct TimerSignal {
    target: Option<u32>,
    signal: i32,
    id: i32,
    value: usize,
    overrun: i32,
    delivered_overrun: Arc<AtomicI32>,
}

impl TimerSignal {
    fn fill(&self, info: &mut PendingSigInfo) {
        info.code = -2; // SI_TIMER
        info.payload[4..8].copy_from_slice(&self.id.to_le_bytes());
        info.payload[8..12].copy_from_slice(&self.overrun.to_le_bytes());
        info.payload[12..20].copy_from_slice(&self.value.to_le_bytes());
        self.delivered_overrun
            .store(self.overrun, Ordering::Release);
    }
}

/// Queues at most one outstanding signal per POSIX timer. Repeated expirations
/// accumulate the Linux overrun count until this signal is actually delivered.
pub fn queue_timer_signal(
    target: Option<u32>,
    signal: i32,
    timer_id: i32,
    value: usize,
    overrun: i32,
    delivered_overrun: Arc<AtomicI32>,
) -> Result<(), i32> {
    let signal_bit = bit(signal).ok_or(crate::EINVAL)?;
    if let Some(thread) = target {
        if !interrupt::thread_exists(thread) {
            return Err(crate::job::ESRCH);
        }
    }
    crate::job::ensure_registered();
    let mut state = state().lock().map_err(|_| crate::EIO)?;
    let action = state.actions[signal as usize];
    if matches!(action.disposition, Disposition::Ignore)
        || (matches!(action.disposition, Disposition::Default) && default_is_ignore(signal))
    {
        return Ok(());
    }
    if let Some(pending) = state
        .timer_pending
        .iter_mut()
        .find(|pending| pending.id == timer_id)
    {
        pending.overrun = pending.overrun.saturating_add(overrun.saturating_add(1));
    } else {
        state.timer_pending.push_back(TimerSignal {
            target,
            signal,
            id: timer_id,
            value,
            overrun,
            delivered_overrun,
        });
    }
    let waiters = if let Some(thread) = target {
        *state.thread_pending.entry(thread).or_default() |= signal_bit;
        vec![thread]
    } else {
        PENDING.fetch_or(signal_bit, Ordering::AcqRel);
        state.waiting.keys().copied().collect()
    };
    drop(state);
    for thread in waiters {
        interrupt::interrupt_thread(thread);
    }
    Ok(())
}

static STATE: OnceLock<Mutex<SignalState>> = OnceLock::new();

/// Signals raised but not yet handled, as a bitmask indexed by signal number.
static PENDING: AtomicU64 = AtomicU64::new(0);

#[inline]
fn signal_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_SIGNAL_TRACE").is_some())
}

fn state() -> &'static Mutex<SignalState> {
    STATE.get_or_init(|| {
        Mutex::new(SignalState {
            actions: vec![Action::default(); NSIG],
            waiting: HashMap::new(),
            thread_pending: HashMap::new(),
            thread_senders: HashMap::new(),
            timer_pending: VecDeque::new(),
        })
    })
}

/// Serializes dispositions plus the calling thread's signal state.
///
/// The process's group and session identity is appended after the disposition
/// records rather than being given a handoff tag of its own. The tag list lives
/// in this crate's `lib.rs`, and the group identity has no meaning apart from
/// the signal state it travels with: a child that adopted its parent's handlers
/// but not its process group would answer a `^C` aimed at a group it is not in.
pub(crate) fn serialize_fork_state() -> Result<Vec<u8>, i32> {
    serialize_process_state(false, None)
}

/// Serializes signal and job identity for an `execve` replacement. Unlike a
/// fork, exec retains the caller's parent, but resets caught dispositions and
/// the active-handler guard: their code and execution frames do not survive.
/// Ignored dispositions, the blocked mask and pending signals are retained.
pub(crate) fn serialize_exec_state() -> Result<Vec<u8>, i32> {
    serialize_process_state(true, None)
}

/// A fresh spawn has exec dispositions but fork parentage and no pending signals.
/// Transform the snapshot only; the spawning thread keeps its mask and handlers.
pub(crate) fn serialize_spawn_state(mask: Option<u64>, defaults: u64) -> Result<Vec<u8>, i32> {
    serialize_process_state(true, Some((mask, defaults)))
}

fn serialize_process_state(
    for_exec: bool,
    spawn: Option<(Option<u64>, u64)>,
) -> Result<Vec<u8>, i32> {
    let state = state().lock().map_err(|_| EIO)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&(state.actions.len() as u32).to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    let pending = if for_exec && spawn.is_none() {
        PENDING.load(Ordering::Acquire)
    } else {
        0
    };
    payload.extend_from_slice(&pending.to_le_bytes());
    let blocked = spawn
        .and_then(|(mask, _)| mask)
        .unwrap_or_else(|| BLOCKED.get())
        & !(bit(SIGKILL).unwrap_or(0) | bit(SIGSTOP).unwrap_or(0));
    payload.extend_from_slice(&blocked.to_le_bytes());
    payload.extend_from_slice(&if for_exec { 0u64 } else { IN_HANDLER.get() }.to_le_bytes());
    for (number, action) in state.actions.iter().enumerate() {
        // Transform only the handoff. Failed exec must leave the caller's
        // dispositions intact, and no old handler/restorer address may enter
        // the new image where it can refer to unmapped code.
        let reset =
            spawn.is_some_and(|(_, defaults)| number > 0 && defaults & (1u64 << (number - 1)) != 0);
        let action = if reset || for_exec && matches!(action.disposition, Disposition::Handle(..)) {
            Action::default()
        } else {
            *action
        };
        let (kind, address) = match action.disposition {
            Disposition::Default => (0u32, 0usize),
            Disposition::Ignore => (1u32, 0usize),
            Disposition::Handle(handler, argument) => {
                (2u32 | ((argument as u32) << 8), handler as usize)
            }
        };
        payload.extend_from_slice(&kind.to_le_bytes());
        payload.extend_from_slice(&action.flags.to_le_bytes());
        payload.extend_from_slice(&action.mask.to_le_bytes());
        payload.extend_from_slice(&(address as u64).to_le_bytes());
        payload.extend_from_slice(&(action.restorer as u64).to_le_bytes());
    }
    // Released before the job registry is touched: registering this process
    // reads its own dispositions, and re-entering this lock would deadlock.
    drop(state);
    let job = if for_exec && spawn.is_none() {
        crate::job::serialize_exec_state()?
    } else {
        crate::job::serialize_fork_state()?
    };
    payload.extend_from_slice(&job);
    Ok(payload)
}

/// Restores process dispositions and the one thread that survives fork.
pub(crate) fn restore_fork_state(payload: &[u8]) -> bool {
    const HEADER: usize = 32;
    const RECORD: usize = 32;
    if payload.len() < HEADER {
        return false;
    }
    let count = u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) as usize;
    let Some(records_end) = HEADER.checked_add(count * RECORD) else {
        return false;
    };
    let Some(expected_len) = records_end.checked_add(16) else {
        return false;
    };
    if count != NSIG || payload.len() != expected_len {
        return false;
    }
    let pending = u64::from_le_bytes(payload[8..16].try_into().unwrap_or_default());
    let blocked = u64::from_le_bytes(payload[16..24].try_into().unwrap_or_default());
    let in_handler = u64::from_le_bytes(payload[24..32].try_into().unwrap_or_default());
    let mut actions = Vec::with_capacity(count);
    for index in 0..count {
        let start = HEADER + index * RECORD;
        let kind = u32::from_le_bytes(payload[start..start + 4].try_into().unwrap_or_default());
        let flags =
            i32::from_le_bytes(payload[start + 4..start + 8].try_into().unwrap_or_default());
        let mask = u64::from_le_bytes(
            payload[start + 8..start + 16]
                .try_into()
                .unwrap_or_default(),
        );
        let address = u64::from_le_bytes(
            payload[start + 16..start + 24]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let restorer = u64::from_le_bytes(
            payload[start + 24..start + 32]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        let disposition = match kind & 0xff {
            0 if address == 0 => Disposition::Default,
            1 if address == 0 => Disposition::Ignore,
            2 if address != 0 => {
                // SAFETY: guest and provider mappings retain their verified base.
                let handler = unsafe { core::mem::transmute::<usize, Handler>(address) };
                Disposition::Handle(handler, (kind >> 8) as i32)
            }
            _ => return false,
        };
        actions.push(Action {
            disposition,
            flags,
            mask,
            restorer,
        });
    }
    let Ok(mut state) = state().lock() else {
        return false;
    };
    state.actions = actions;
    state.waiting.clear();
    state.thread_pending.clear();
    state.thread_senders.clear();
    // POSIX timers are not inherited across fork and are deleted by exec.
    state.timer_pending.clear();
    drop(state);
    PENDING.store(pending, Ordering::Release);
    BLOCKED.set(blocked);
    IN_HANDLER.set(in_handler);
    // The child adopts its parent's group and session last, once the signal
    // state that identity applies to is already in place.
    crate::job::restore_fork_state(&payload[records_end..])
}

thread_local! {
    /// This thread's blocked-signal mask.
    static BLOCKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Guards against a handler for signal N re-entering while N is running.
    static IN_HANDLER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static NONRESTART_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Returns the bit for `signal`, or `None` if it is out of range.
fn bit(signal: i32) -> Option<u64> {
    if signal <= 0 || signal as usize >= NSIG {
        return None;
    }
    Some(1u64 << (signal as u64 - 1))
}

/// Signals whose default action is to be ignored rather than to terminate.
fn default_is_ignore(signal: i32) -> bool {
    matches!(signal, SIGCHLD | SIGCONT | SIGWINCH | SIGURG)
}

/// Signals that cannot be caught, blocked or ignored.
fn uncatchable(signal: i32) -> bool {
    matches!(signal, SIGKILL | SIGSTOP)
}

/// Signals whose default action is to stop the process rather than kill it.
///
/// Before job control existed here these fell through to [`default_terminate`],
/// so a `^Z` killed the pipeline with status 148 instead of suspending it. The
/// two terminal signals are in the set as well as `SIGSTOP` and `SIGTSTP`
/// because a background process touching the terminal must suspend rather than
/// die — suspending is the entire mechanism by which a shell learns it has to
/// bring that process to the foreground.
pub fn default_is_stop(signal: i32) -> bool {
    matches!(signal, SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU)
}

/// Installs `mask` as this thread's blocked set, returning the previous one.
///
/// `pselect` and `ppoll` promise that replacing the mask and starting the wait
/// are one indivisible step. A signal arriving between the two would otherwise
/// be handled before the wait began and never wake it — which is precisely the
/// race those calls exist to close. A kernel gets the atomicity by doing both
/// inside one syscall; here the wait is assembled in the caller, so the
/// arrangement that actually works is: swap the mask, register as a waiter, and
/// check the pending set *before* sleeping. A signal that lands in the window is
/// already pending when the caller looks, so it is handled rather than lost.
///
/// Restoring the previous mask is the caller's job: only the caller knows when
/// its wait is over. `SIGKILL` and `SIGSTOP` are dropped from the request, as
/// [`sigprocmask`] also drops them, because they can never be blocked.
pub fn swap_blocked_mask(mask: u64) -> u64 {
    let previous = BLOCKED.get();
    BLOCKED.set(mask & !(bit(SIGKILL).unwrap_or(0) | bit(SIGSTOP).unwrap_or(0)));
    previous
}

/// Installs an action, returning the previous one.
pub fn sigaction(signal: i32, action: Option<Action>) -> Result<Action, i32> {
    if bit(signal).is_none() {
        return Err(crate::EINVAL);
    }
    if uncatchable(signal) && action.is_some() {
        return Err(crate::EINVAL);
    }
    let mut state = state().lock().map_err(|_| crate::EIO)?;
    let previous = state.actions[signal as usize];
    if let Some(action) = action {
        state.actions[signal as usize] = action;
    }
    Ok(previous)
}

/// Reads the current action without changing it.
pub fn current_action(signal: i32) -> Result<Action, i32> {
    if bit(signal).is_none() {
        return Err(crate::EINVAL);
    }
    let state = state().lock().map_err(|_| crate::EIO)?;
    Ok(state.actions[signal as usize])
}

/// Returns the calling thread's blocked mask.
pub fn blocked_mask() -> u64 {
    BLOCKED.get()
}

/// Applies a `sigprocmask` operation, returning the previous mask.
pub fn sigprocmask(how: i32, mask: u64) -> Result<u64, i32> {
    let previous = BLOCKED.get();
    // SIGKILL and SIGSTOP can never be blocked.
    let blockable = mask & !(bit(SIGKILL).unwrap_or(0) | bit(SIGSTOP).unwrap_or(0));
    let next = match how {
        SIG_BLOCK => previous | blockable,
        SIG_UNBLOCK => previous & !blockable,
        SIG_SETMASK => blockable,
        _ => return Err(crate::EINVAL),
    };
    BLOCKED.set(next);
    Ok(previous)
}

/// Returns the set of pending signals.
pub fn pending() -> u64 {
    let directed = state()
        .lock()
        .ok()
        .and_then(|state| {
            state
                .thread_pending
                .get(&interrupt::current_thread_id())
                .copied()
        })
        .unwrap_or(0);
    PENDING.load(Ordering::Acquire) | directed
}

/// A queued signal accepted by sigwaitinfo/sigtimedwait without changing any
/// disposition or executing a guest handler. Payload bytes have siginfo_t's
/// original offset 12, including the alignment padding before its union.
pub struct PendingSignal {
    pub signal: i32,
    pub code: i32,
    pub payload: [u8; 116],
}

pub fn take_pending(mask: u64) -> Option<PendingSignal> {
    crate::job::drain_external();
    let thread = interrupt::current_thread_id();
    let mut state = state().lock().ok()?;
    loop {
        let directed = state.thread_pending.get(&thread).copied().unwrap_or(0);
        let ready = (directed | PENDING.load(Ordering::Acquire)) & mask;
        if ready == 0 {
            return None;
        }
        let index = ready.trailing_zeros();
        let signal = index as i32 + 1;
        let claim = 1u64 << index;
        let target = (directed & claim != 0).then_some(thread);
        let sender = if target.is_some() {
            let bits = state.thread_pending.get_mut(&thread).unwrap();
            *bits &= !claim;
            if *bits == 0 {
                state.thread_pending.remove(&thread);
            }
            state.thread_senders.remove(&(thread, signal))
        } else {
            if PENDING.fetch_and(!claim, Ordering::AcqRel) & claim == 0 {
                continue;
            }
            None
        };
        let timer = state
            .timer_pending
            .iter()
            .position(|timer| timer.signal == signal && timer.target == target)
            .and_then(|index| state.timer_pending.remove(index));
        if state
            .timer_pending
            .iter()
            .any(|timer| timer.signal == signal && timer.target == target)
        {
            if target.is_some() {
                *state.thread_pending.entry(thread).or_default() |= claim;
            } else {
                PENDING.fetch_or(claim, Ordering::AcqRel);
            }
        }
        drop(state);
        let mut info = PendingSigInfo {
            signal,
            errno: 0,
            code: if target.is_some() { -6 } else { 0 },
            payload: [0; 116],
        };
        if let Some(sender) = sender {
            sender.fill(&mut info);
        }
        if let Some(timer) = timer {
            timer.fill(&mut info);
        } else if target.is_none() && crate::mqueue::signal_info(signal, &mut info.payload) {
            info.code = -3;
        }
        return Some(PendingSignal {
            signal,
            code: info.code,
            payload: info.payload,
        });
    }
}

/// Lets an outer syscall boundary recognize an EINTR whose non-restarting
/// handler was already dispatched by an inner FIFO/pipe rendezvous.
pub fn nonrestart_epoch() -> u64 {
    NONRESTART_EPOCH.get()
}

fn record_nonrestart() {
    NONRESTART_EPOCH.set(NONRESTART_EPOCH.get().wrapping_add(1));
}

/// A wake or a pending default-ignored signal cannot interrupt native I/O.
/// Inspect dispositions without dispatching guest handlers under VFS locks.
pub(crate) fn interrupt_pending() -> bool {
    let Ok(state) = state().lock() else {
        return false;
    };
    let directed = state
        .thread_pending
        .get(&interrupt::current_thread_id())
        .copied()
        .unwrap_or(0);
    let mut ready =
        (PENDING.load(Ordering::Acquire) | directed) & !BLOCKED.get() & !IN_HANDLER.get();
    while ready != 0 {
        let number = ready.trailing_zeros() as i32 + 1;
        ready &= ready - 1;
        if state
            .actions
            .get(number as usize)
            .is_some_and(|a| match a.disposition {
                Disposition::Ignore => false,
                Disposition::Default => !default_is_ignore(number),
                Disposition::Handle(..) => true,
            })
        {
            return true;
        }
    }
    false
}

/// Whether the calling thread has a deliverable terminating signal. Long
/// filesystem transactions use this only to unwind their guards; they must
/// deliver the signal after rollback, never run guest handlers while locked.
pub fn fatal_pending() -> Result<bool, i32> {
    let state = state().lock().map_err(|_| crate::EIO)?;
    let directed = state
        .thread_pending
        .get(&interrupt::current_thread_id())
        .copied()
        .unwrap_or(0);
    let ready = (PENDING.load(Ordering::Acquire) | directed) & !BLOCKED.get() & !IN_HANDLER.get();
    Ok(has_fatal_action(&state.actions, ready))
}

fn has_fatal_action(actions: &[Action], mut ready: u64) -> bool {
    while ready != 0 {
        let index = ready.trailing_zeros();
        ready &= ready - 1;
        let signal = index as i32 + 1;
        if actions.get(signal as usize).is_some_and(|action| {
            matches!(action.disposition, Disposition::Default)
                && !default_is_ignore(signal)
                && !default_is_stop(signal)
        }) {
            return true;
        }
    }
    false
}

/// Queues a signal for exactly one thread, as Linux `tgkill(2)` does.
///
/// The target ID is the Windows thread ID exposed by the guest `gettid` ABI.
/// Delivery remains cooperative, but only that thread may claim the signal and
/// its interrupt event is the only one woken.
pub fn raise_thread_signal(thread_id: u32, signal: i32) -> Result<(), i32> {
    if signal == 0 {
        return interrupt::thread_exists(thread_id)
            .then_some(())
            .ok_or(crate::job::ESRCH);
    }
    let Some(signal_bit) = bit(signal) else {
        return Err(crate::EINVAL);
    };
    if signal_trace_enabled() {
        eprintln!(
            "kinakaze signal: queue sig={signal} sender_tid={} target_tid={thread_id}",
            interrupt::current_thread_id()
        );
    }
    if !interrupt::thread_exists(thread_id) {
        return Err(crate::job::ESRCH);
    }
    crate::job::ensure_registered();

    let sender = SignalSender::current();
    let mut state = state().lock().map_err(|_| crate::EIO)?;
    let action = state.actions[signal as usize];
    if matches!(action.disposition, Disposition::Ignore)
        || (matches!(action.disposition, Disposition::Default) && default_is_ignore(signal))
    {
        return Ok(());
    }
    *state.thread_pending.entry(thread_id).or_insert(0) |= signal_bit;
    state
        .thread_senders
        .entry((thread_id, signal))
        .or_insert(sender);
    drop(state);
    interrupt::interrupt_thread(thread_id);
    Ok(())
}

/// Raises `signal` in this process and wakes any interruptible waiter.
///
/// The wake is what turns a signal into an `EINTR`: a thread parked in
/// `WaitForMultipleObjects` on its interrupt event returns, cancels its I/O and
/// then runs [`deliver_pending`].
///
/// This is process-local by construction. Reaching another process goes through
/// [`crate::job`], which posts into that process's registry slot and wakes it;
/// the far side then arrives back here.
pub fn raise_signal(signal: i32) -> Result<(), i32> {
    let Some(bit) = bit(signal) else {
        return Err(crate::EINVAL);
    };
    // Registration is what makes this process reachable *from* another one. It
    // happens here rather than in a process initializer because a forked child
    // runs its initializers before the parent replaces its arena, so anything an
    // initializer set up would describe the parent instead of the child.
    crate::job::ensure_registered();

    // An explicitly ignored signal is discarded rather than left pending, which
    // is what keeps `SIG_IGN` from making every later wait return early.
    if let Ok(state) = state().lock()
        && matches!(
            state.actions[signal as usize].disposition,
            Disposition::Ignore
        )
    {
        return Ok(());
    }
    // A default-ignored signal with no handler is likewise dropped.
    if let Ok(state) = state().lock()
        && matches!(
            state.actions[signal as usize].disposition,
            Disposition::Default
        )
        && default_is_ignore(signal)
    {
        return Ok(());
    }

    PENDING.fetch_or(bit, Ordering::AcqRel);

    // Wake every registered waiter. Only threads that do not block the signal
    // will actually act on it, but the wake itself is cheap and a sleeping
    // thread cannot be inspected for its mask without waking it.
    let waiters: Vec<u32> = match state().lock() {
        Ok(state) => state.waiting.keys().copied().collect(),
        Err(_) => Vec::new(),
    };
    for thread in waiters {
        interrupt::interrupt_thread(thread);
    }
    Ok(())
}

/// Registers the calling thread as interruptible for the duration of a wait.
pub fn register_waiter() {
    // A thread about to block is exactly the thread another process needs to be
    // able to wake, so this is the second place registration must have happened
    // by: a program that only ever reads still has to answer a `^C`.
    crate::job::ensure_registered();
    let thread = interrupt::current_thread_id();
    if let Ok(mut state) = state().lock() {
        state.waiting.insert(thread, ());
    }
}

/// Removes the calling thread from the interruptible set.
pub fn unregister_waiter() {
    let thread = interrupt::current_thread_id();
    if let Ok(mut state) = state().lock() {
        state.waiting.remove(&thread);
    }
}

/// The outcome of running pending handlers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// Nothing was pending for this thread.
    None,
    /// A handler ran and requested that the call be restarted.
    Restart,
    /// A handler ran without `SA_RESTART`, so the call reports `EINTR`.
    Interrupted,
}

thread_local! { static DEFERRED_DELIVERY: Cell<u32> = const { Cell::new(0) }; }
pub(crate) struct DeferredDelivery;
impl Drop for DeferredDelivery {
    fn drop(&mut self) {
        DEFERRED_DELIVERY.set(DEFERRED_DELIVERY.get() - 1);
    }
}
pub(crate) fn defer_delivery() -> DeferredDelivery {
    DEFERRED_DELIVERY.set(DEFERRED_DELIVERY.get() + 1);
    DeferredDelivery
}
pub(crate) fn delivery_deferred() -> bool {
    DEFERRED_DELIVERY.get() != 0
}

/// Runs handlers for signals pending on the calling thread.
///
/// Returns whether the interrupted operation should restart. When several
/// signals are pending, any one lacking `SA_RESTART` makes the call interrupted,
/// because the guest must observe `EINTR` for that handler.
///
/// Signals posted by another process are collected here as well as by the pump
/// thread [`crate::job`] runs. The duplication is deliberate: a process whose
/// pump could not start still sees every cross-process signal at exactly the
/// points it would have seen a local one, which makes the pump a latency
/// improvement rather than a correctness requirement.
pub fn deliver_pending() -> Delivery {
    crate::job::drain_external();
    let blocked = BLOCKED.get();
    let in_handler = IN_HANDLER.get();
    let mut outcome = Delivery::None;
    let thread_id = interrupt::current_thread_id();

    loop {
        // Claim one deliverable signal at a time so a handler that raises
        // another signal is still serviced.
        let directed = state()
            .lock()
            .ok()
            .and_then(|state| state.thread_pending.get(&thread_id).copied())
            .unwrap_or(0);
        let ready = (PENDING.load(Ordering::Acquire) | directed) & !blocked & !in_handler;
        if ready == 0 {
            return outcome;
        }
        #[cfg(windows)]
        if delivery_deferred() || crate::ofd::signal_delivery_deferred() {
            // Do not claim a signal while positioned I/O owns locks needed by
            // its guest handler or a child forked from that handler.
            return Delivery::Interrupted;
        }
        let index = ready.trailing_zeros();
        let signal = index as i32 + 1;
        let claim = 1u64 << index;
        // A directed signal belongs to this thread and is claimed before a
        // process-wide occurrence of the same number. Process signals retain
        // their atomic any-unblocked-thread arbitration.
        let (claimed_directed, sender, timer) = {
            let Ok(mut state) = state().lock() else {
                return outcome;
            };
            let claimed_directed = state
                .thread_pending
                .get(&thread_id)
                .is_some_and(|bits| bits & claim != 0);
            let sender = if claimed_directed {
                let bits = state.thread_pending.get_mut(&thread_id).unwrap();
                *bits &= !claim;
                if *bits == 0 {
                    state.thread_pending.remove(&thread_id);
                }
                state.thread_senders.remove(&(thread_id, signal))
            } else {
                if PENDING.fetch_and(!claim, Ordering::AcqRel) & claim == 0 {
                    continue;
                }
                None
            };
            let target = claimed_directed.then_some(thread_id);
            let timer = state
                .timer_pending
                .iter()
                .position(|timer| timer.signal == signal && timer.target == target)
                .and_then(|index| state.timer_pending.remove(index));
            // Several timers may share a signal. Preserve readiness for each
            // independently queued timer after claiming this occurrence.
            if state
                .timer_pending
                .iter()
                .any(|timer| timer.signal == signal && timer.target == target)
            {
                if claimed_directed {
                    *state.thread_pending.entry(thread_id).or_default() |= claim;
                } else {
                    PENDING.fetch_or(claim, Ordering::AcqRel);
                }
            }
            (claimed_directed, sender, timer)
        };

        let Ok(action) = current_action(signal) else {
            continue;
        };
        let active_guest = kinakaze_tls::thread_pointer::active_guest_signal_context();
        if signal_trace_enabled() {
            eprintln!(
                "kinakaze signal: deliver sig={signal} target_tid={thread_id} directed={claimed_directed} active={active_guest:?} flags={:#x}",
                action.flags
            );
        }
        if matches!(action.disposition, Disposition::Handle(_, _))
            && action.flags & (SA_SIGINFO | SA_ONSTACK) == (SA_SIGINFO | SA_ONSTACK)
            && active_guest.is_none()
        {
            // A three-argument alternate-stack handler (Go and JVM both use
            // this form) must see the interrupted guest registers, not the
            // host provider stack. Keep it pending until the target reaches a
            // raw syscall boundary that has published those registers.
            if let Ok(mut state) = state().lock() {
                if let Some(timer) = timer {
                    if let Some(queued) = state
                        .timer_pending
                        .iter_mut()
                        .find(|queued| queued.id == timer.id)
                    {
                        queued.overrun = queued
                            .overrun
                            .saturating_add(timer.overrun.saturating_add(1));
                    } else {
                        state.timer_pending.push_front(timer);
                    }
                }
                if claimed_directed {
                    *state.thread_pending.entry(thread_id).or_default() |= claim;
                    if let Some(sender) = sender {
                        state.thread_senders.insert((thread_id, signal), sender);
                    }
                } else {
                    PENDING.fetch_or(claim, Ordering::AcqRel);
                }
            }
            return outcome;
        }
        match action.disposition {
            Disposition::Ignore => continue,
            Disposition::Default => {
                if default_is_ignore(signal) {
                    continue;
                }
                if default_is_stop(signal) {
                    // The process really stops here: every other thread is
                    // suspended and this one parks until a SIGCONT arrives. The
                    // interrupted call reports EINTR afterwards, which is what
                    // Linux does for a wait broken by a stop.
                    outcome = Delivery::Interrupted;
                    record_nonrestart();
                    crate::job::stop_self(signal);
                    continue;
                }
                // The default action for the rest is to terminate. Reporting it
                // rather than killing the process keeps this crate free of
                // process-lifetime policy, which belongs in libc.
                outcome = Delivery::Interrupted;
                default_terminate(signal);
            }
            Disposition::Handle(handler, _) => {
                // Block this signal and the action's mask while the handler runs
                // unless SA_NODEFER says otherwise.
                let previous_blocked = BLOCKED.get();
                if action.flags & SA_NODEFER == 0 {
                    BLOCKED.set(previous_blocked | action.mask | claim);
                }
                IN_HANDLER.set(in_handler | claim);

                // SA_RESETHAND restores the default before the handler runs.
                if action.flags & SA_RESETHAND != 0 {
                    let _ = sigaction(signal, Some(Action::default()));
                }

                let mut siginfo = PendingSigInfo {
                    signal,
                    errno: 0,
                    // SI_TKILL for a thread-directed signal, SI_USER for a
                    // process-directed one.
                    code: if claimed_directed { -6 } else { 0 },
                    payload: [0; 116],
                };
                if let Some(sender) = sender {
                    sender.fill(&mut siginfo);
                }
                if let Some(timer) = timer {
                    timer.fill(&mut siginfo);
                } else if !claimed_directed
                    && crate::mqueue::signal_info(signal, &mut siginfo.payload)
                {
                    siginfo.code = -3;
                }
                let (guest_stack, guest_instruction) = active_guest.unwrap_or_else(|| {
                    let stack_marker = 0usize;
                    (
                        core::ptr::from_ref(&stack_marker) as usize,
                        handler as *const () as usize,
                    )
                });
                let mut context = PendingUContext::new(
                    previous_blocked,
                    ALT_STACK.get(),
                    guest_stack,
                    guest_instruction,
                );
                context.bind_internal_pointers();
                // SAFETY: both Linux ABI records remain live throughout the
                // callback; `invoke_handler` also honors SA_ONSTACK.
                unsafe {
                    invoke_handler(
                        action,
                        handler,
                        signal,
                        (&raw mut siginfo).cast(),
                        (&raw mut context).cast(),
                    )
                };
                if active_guest.is_some() {
                    // Linux's rt_sigreturn restores the handler-edited machine
                    // context.  The raw-syscall trampoline consumes these two
                    // fields after the dispatcher returns and either resumes the
                    // interrupted stream or branches to the handler's new PC.
                    let _ = kinakaze_tls::thread_pointer::update_active_guest_signal_context(
                        context.gregs[15] as usize,
                        context.gregs[16] as usize,
                    );
                    let _ = kinakaze_tls::thread_pointer::update_active_guest_general_registers(
                        &context.gregs,
                    );
                    if signal_trace_enabled() {
                        eprintln!(
                            "kinakaze signal: sigreturn sig={signal} rsp={:#x} rip={:#x}",
                            context.gregs[15], context.gregs[16]
                        );
                    }
                }

                IN_HANDLER.set(in_handler);
                BLOCKED.set(previous_blocked);

                // Any handler without SA_RESTART forces EINTR.
                if action.flags & SA_RESTART != 0 {
                    if outcome == Delivery::None {
                        outcome = Delivery::Restart;
                    }
                } else {
                    outcome = Delivery::Interrupted;
                    record_nonrestart();
                }
            }
        }
    }
}

/// Delivers a hardware-generated signal at the faulting instruction.
///
/// Unlike process-directed signals, a synchronous fault cannot be put on the
/// cooperative pending queue: the faulting instruction has not completed and
/// must not be retried until its handler has had a chance to edit the supplied
/// `ucontext_t`.  The loader calls this from its exception boundary with Linux
/// ABI `siginfo_t` and `ucontext_t` records translated from the host context.
///
/// Returns `true` when a guest handler ran.  `false` means the disposition was
/// default/ignored, or that the signal was blocked or recursively faulted; the
/// caller must then continue normal fatal-exception processing.
///
/// # Safety
///
/// `siginfo` and `context` must either be null or point to live guest-ABI records
/// for the duration of the handler.  A handler installed with `SA_SIGINFO` is
/// trusted to follow that ABI, exactly as it is on Linux.
pub unsafe fn deliver_synchronous(signal: i32, siginfo: *mut c_void, context: *mut c_void) -> bool {
    let Some(claim) = bit(signal) else {
        return false;
    };
    let blocked = BLOCKED.get();
    let in_handler = IN_HANDLER.get();
    if blocked & claim != 0 || in_handler & claim != 0 {
        return false;
    }
    let Ok(action) = current_action(signal) else {
        return false;
    };
    let Disposition::Handle(handler, _) = action.disposition else {
        // Ignoring a hardware fault would immediately execute the same faulting
        // instruction again.  Linux treats that as fatal rather than spinning.
        return false;
    };

    let previous_blocked = blocked;
    if action.flags & SA_NODEFER == 0 {
        BLOCKED.set(previous_blocked | action.mask | claim);
    } else {
        BLOCKED.set(previous_blocked | action.mask);
    }
    IN_HANDLER.set(in_handler | claim);
    if action.flags & SA_RESETHAND != 0 {
        let _ = sigaction(signal, Some(Action::default()));
    }

    // SAFETY: the loader supplied live Linux ABI records and the installed
    // action determines the handler signature and alternate stack.
    unsafe { invoke_handler(action, handler, signal, siginfo, context) };

    IN_HANDLER.set(in_handler);
    BLOCKED.set(previous_blocked);
    true
}

/// Handles a signal whose disposition is the default terminating action.
fn default_terminate(signal: i32) {
    // The hook lets libc install process termination without this crate
    // depending on it.
    if let Some(hook) = TERMINATE_HOOK.get() {
        hook(signal);
    }
}

/// Terminates this process for `signal` immediately, consulting no mask.
///
/// Only [`crate::job`] calls this, and only for `SIGKILL`. Every other signal
/// must go through the pending set, because whether it is deliverable at all
/// depends on per-thread blocked masks that the thread noticing the signal does
/// not own. `SIGKILL` cannot be blocked, caught or ignored, so there is nothing
/// to consult and nothing to wait for.
pub fn terminate_now(signal: i32) {
    default_terminate(signal);
}

/// Called when a signal's default action would terminate the process.
pub type TerminateHook = fn(i32);
static TERMINATE_HOOK: OnceLock<TerminateHook> = OnceLock::new();

/// Installs the process-termination hook. The first call wins.
pub fn set_terminate_hook(hook: TerminateHook) {
    let _ = TERMINATE_HOOK.set(hook);
}

/// Serializes tests that raise signals.
///
/// Dispositions and the pending set are process-global, and a process-directed
/// signal is delivered on whichever thread happens to be waiting. Concurrent
/// tests would therefore steal each other's signals; this lock is the honest
/// alternative to pretending they are independent.
#[cfg(test)]
pub(crate) fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering as AtomicOrdering};

    static OBSERVED: AtomicI32 = AtomicI32::new(0);
    static HANDLER_STACK_ADDRESS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "sysv64" fn record(signal: i32) {
        OBSERVED.store(signal, AtomicOrdering::SeqCst);
    }

    unsafe extern "sysv64" fn record_siginfo(signal: i32, info: *mut c_void, context: *mut c_void) {
        if info.is_null() || context.is_null() {
            return;
        }
        let info = unsafe { &*info.cast::<PendingSigInfo>() };
        if info.signal == signal && info.errno == 0 && info.code == 0 {
            OBSERVED.store(signal, AtomicOrdering::SeqCst);
        }
    }

    #[inline(never)]
    unsafe extern "sysv64" fn record_handler_stack(_signal: i32) {
        let marker = 0usize;
        HANDLER_STACK_ADDRESS.store(
            core::ptr::from_ref(&marker) as usize,
            AtomicOrdering::SeqCst,
        );
    }

    #[test]
    fn linux_x86_64_signal_records_have_the_kernel_abi_layout() {
        assert_eq!(core::mem::size_of::<SignalStack>(), 24);
        assert_eq!(core::mem::size_of::<PendingSigInfo>(), 128);
        assert_eq!(core::mem::size_of::<PendingUContext>(), 968);

        let mut context = PendingUContext::new(0, SignalStack::default(), 0, 0);
        context.bind_internal_pointers();
        assert_eq!(context.fpregs, context.fpstate.as_mut_ptr().cast());
    }

    #[test]
    fn transaction_abort_checks_only_terminating_dispositions() {
        let mut actions = vec![Action::default(); NSIG];
        assert!(!has_fatal_action(&actions, 0));
        for signal in [
            SIGCHLD, SIGCONT, SIGWINCH, SIGURG, SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU,
        ] {
            assert!(!has_fatal_action(&actions, bit(signal).unwrap()));
        }
        for signal in [SIGKILL, SIGTERM, SIGINT, SIGBUS, SIGSEGV] {
            assert!(has_fatal_action(&actions, bit(signal).unwrap()));
        }
        actions[SIGTERM as usize].disposition = Disposition::Ignore;
        actions[SIGINT as usize].disposition = Disposition::Handle(record, 0);
        assert!(!has_fatal_action(
            &actions,
            bit(SIGTERM).unwrap() | bit(SIGINT).unwrap()
        ));
    }

    #[test]
    fn native_io_ignores_stale_default_ignored_and_active_handler_signals() {
        let _serialized = test_lock();
        let saved_action = sigaction(SIGCHLD, Some(Action::default())).unwrap();
        let saved_mask = swap_blocked_mask(0);
        let saved_pending = PENDING.swap(bit(SIGCHLD).unwrap(), Ordering::AcqRel);
        let saved_handler = IN_HANDLER.replace(0);
        assert!(!interrupt_pending());
        sigaction(
            SIGCHLD,
            Some(Action {
                disposition: Disposition::Handle(record, 0),
                ..Action::default()
            }),
        )
        .unwrap();
        assert!(interrupt_pending());
        IN_HANDLER.set(bit(SIGCHLD).unwrap());
        assert!(!interrupt_pending());
        IN_HANDLER.set(0);
        swap_blocked_mask(bit(SIGCHLD).unwrap());
        assert!(!interrupt_pending());
        PENDING.store(saved_pending, Ordering::Release);
        IN_HANDLER.set(saved_handler);
        swap_blocked_mask(saved_mask);
        sigaction(SIGCHLD, Some(saved_action)).unwrap();
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    #[test]
    fn sa_onstack_switches_to_the_calling_threads_alternate_stack() {
        let _serialized = test_lock();
        let mut memory = vec![0u8; 64 * 1024];
        let bottom = memory.as_mut_ptr() as usize;
        let top = bottom + memory.len();
        let previous = sigaltstack(Some(SignalStack {
            ss_sp: memory.as_mut_ptr().cast(),
            ss_flags: 0,
            ss_size: memory.len(),
        }))
        .unwrap();
        HANDLER_STACK_ADDRESS.store(0, AtomicOrdering::SeqCst);

        unsafe {
            invoke_handler(
                Action {
                    disposition: Disposition::Handle(record_handler_stack, 0),
                    flags: SA_ONSTACK,
                    mask: 0,
                    restorer: 0,
                },
                record_handler_stack,
                SIGUSR1,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
            )
        };

        let observed = HANDLER_STACK_ADDRESS.load(AtomicOrdering::SeqCst);
        assert!(
            (bottom..top).contains(&observed),
            "handler stack address {observed:#x} is outside {bottom:#x}..{top:#x}"
        );
        sigaltstack(Some(previous)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn positioned_io_releases_locks_before_signal_handlers() {
        let _serialized = test_lock();
        static FD: AtomicI32 = AtomicI32::new(-1);
        unsafe extern "sysv64" fn handler(signal: i32) {
            if crate::ofd::signal_delivery_deferred() {
                return;
            }
            // Promotion takes the same local lock as positioned I/O. This
            // would deadlock if the handler ran inside the interrupted call.
            if crate::ofd::promote(FD.load(AtomicOrdering::SeqCst)).is_ok() {
                OBSERVED.store(signal, AtomicOrdering::SeqCst);
            }
        }
        let fd = crate::eventfd::create_eventfd(0, 0).unwrap();
        FD.store(fd, AtomicOrdering::SeqCst);
        let old_mask = sigprocmask(SIG_SETMASK, 0).unwrap();
        for flags in [SA_RESTART, 0] {
            OBSERVED.store(0, AtomicOrdering::SeqCst);
            sigaction(
                SIGUSR1,
                Some(Action {
                    disposition: Disposition::Handle(handler, 0),
                    flags,
                    mask: 0,
                    restorer: 0,
                }),
            )
            .unwrap();
            let mut attempts = 0;
            let result = crate::ofd::with(fd, || {
                attempts += 1;
                if attempts == 1 {
                    raise_signal(SIGUSR1).unwrap();
                    assert_eq!(deliver_pending(), Delivery::Interrupted);
                    assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), 0);
                    return Err(crate::EINTR);
                }
                Ok(7)
            });
            assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), SIGUSR1);
            assert_eq!(
                result,
                if flags == SA_RESTART {
                    Ok(7)
                } else {
                    Err(crate::EINTR)
                }
            );
            assert_eq!(attempts, if flags == SA_RESTART { 2 } else { 1 });
        }
        sigaction(SIGUSR1, Some(Action::default())).unwrap();
        sigprocmask(SIG_SETMASK, old_mask).unwrap();
        crate::close(fd).unwrap();
    }

    #[test]
    fn a_handler_runs_and_reports_restart_semantics() {
        let _serialized = test_lock();
        OBSERVED.store(0, AtomicOrdering::SeqCst);
        // SA_RESTART means the interrupted call should resume.
        sigaction(
            SIGUSR1,
            Some(Action {
                disposition: Disposition::Handle(record, 0),
                flags: SA_RESTART,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        raise_signal(SIGUSR1).unwrap();
        assert_eq!(deliver_pending(), Delivery::Restart);
        assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), SIGUSR1);

        // Without SA_RESTART the same handler produces EINTR instead.
        sigaction(
            SIGUSR1,
            Some(Action {
                disposition: Disposition::Handle(record, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        raise_signal(SIGUSR1).unwrap();
        assert_eq!(deliver_pending(), Delivery::Interrupted);

        // Nothing is left pending.
        assert_eq!(deliver_pending(), Delivery::None);
        sigaction(SIGUSR1, Some(Action::default())).unwrap();
    }

    #[test]
    fn sigwait_accepts_timer_payload_without_changing_disposition() {
        let _serialized = test_lock();
        let old = sigaction(SIGUSR2, Some(Action::default())).unwrap();
        let mask = bit(SIGUSR2).unwrap();
        let previous_mask = sigprocmask(SIG_BLOCK, mask).unwrap();
        let overruns = Arc::new(AtomicI32::new(0));
        queue_timer_signal(None, SIGUSR2, 773, 0x123456, 4, Arc::clone(&overruns)).unwrap();
        let info = take_pending(mask).unwrap();
        assert_eq!((info.signal, info.code), (SIGUSR2, -2));
        assert_eq!(
            i32::from_le_bytes(info.payload[4..8].try_into().unwrap()),
            773
        );
        assert_eq!(
            usize::from_le_bytes(info.payload[12..20].try_into().unwrap()),
            0x123456
        );
        assert_eq!(overruns.load(Ordering::Acquire), 4);
        assert!(take_pending(mask).is_none());
        assert!(matches!(
            current_action(SIGUSR2).unwrap().disposition,
            Disposition::Default
        ));
        sigprocmask(SIG_SETMASK, previous_mask).unwrap();
        sigaction(SIGUSR2, Some(old)).unwrap();
    }

    #[test]
    fn posix_timers_preserve_siginfo_payload_and_coalesce_overruns() {
        let _serialized = test_lock();
        static RECORDS: Mutex<Vec<(i32, i32, usize)>> = Mutex::new(Vec::new());
        unsafe extern "sysv64" fn record_timer(
            _signal: i32,
            info: *mut c_void,
            _context: *mut c_void,
        ) {
            let info = unsafe { &*info.cast::<PendingSigInfo>() };
            assert_eq!(info.code, -2);
            let id = i32::from_le_bytes(info.payload[4..8].try_into().unwrap());
            let overrun = i32::from_le_bytes(info.payload[8..12].try_into().unwrap());
            let value = usize::from_le_bytes(info.payload[12..20].try_into().unwrap());
            RECORDS.lock().unwrap().push((id, overrun, value));
        }
        let handler = unsafe { core::mem::transmute::<SiginfoHandler, Handler>(record_timer) };
        let old = sigaction(
            SIGUSR2,
            Some(Action {
                disposition: Disposition::Handle(handler, SA_SIGINFO),
                flags: SA_SIGINFO,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        let first = Arc::new(AtomicI32::new(0));
        let second = Arc::new(AtomicI32::new(0));
        queue_timer_signal(
            None,
            SIGUSR2,
            771,
            0x1234_5678_abcdef,
            2,
            Arc::clone(&first),
        )
        .unwrap();
        queue_timer_signal(
            None,
            SIGUSR2,
            771,
            0x1234_5678_abcdef,
            3,
            Arc::clone(&first),
        )
        .unwrap();
        queue_timer_signal(None, SIGUSR2, 772, 123, 0, Arc::clone(&second)).unwrap();
        assert_eq!(first.load(Ordering::Acquire), 0);
        assert_eq!(deliver_pending(), Delivery::Interrupted);
        assert_eq!(
            *RECORDS.lock().unwrap(),
            [(771, 6, 0x1234_5678_abcdef), (772, 0, 123)]
        );
        assert_eq!(first.load(Ordering::Acquire), 6);
        assert_eq!(second.load(Ordering::Acquire), 0);
        assert_eq!(deliver_pending(), Delivery::None);
        sigaction(SIGUSR2, Some(old)).unwrap();
    }

    #[test]
    fn a_pending_siginfo_handler_receives_three_linux_arguments() {
        let _serialized = test_lock();
        OBSERVED.store(0, AtomicOrdering::SeqCst);
        let handler = unsafe { core::mem::transmute::<SiginfoHandler, Handler>(record_siginfo) };
        sigaction(
            SIGUSR2,
            Some(Action {
                disposition: Disposition::Handle(handler, SA_SIGINFO),
                flags: SA_SIGINFO,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        raise_signal(SIGUSR2).unwrap();
        assert_eq!(deliver_pending(), Delivery::Interrupted);
        assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), SIGUSR2);
        sigaction(SIGUSR2, Some(Action::default())).unwrap();
    }

    #[test]
    fn ignored_signals_are_discarded_rather_than_left_pending() {
        let _serialized = test_lock();
        sigaction(
            SIGUSR2,
            Some(Action {
                disposition: Disposition::Ignore,
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        raise_signal(SIGUSR2).unwrap();
        // An ignored signal must not make the next wait return early.
        assert_eq!(pending() & bit(SIGUSR2).unwrap(), 0);
        sigaction(SIGUSR2, Some(Action::default())).unwrap();
    }

    #[test]
    fn a_blocked_signal_stays_pending_until_unblocked() {
        let _serialized = test_lock();
        OBSERVED.store(0, AtomicOrdering::SeqCst);
        sigaction(
            SIGHUP,
            Some(Action {
                disposition: Disposition::Handle(record, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        sigprocmask(SIG_BLOCK, bit(SIGHUP).unwrap()).unwrap();
        raise_signal(SIGHUP).unwrap();

        // Blocked: the handler must not run yet.
        assert_eq!(deliver_pending(), Delivery::None);
        assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), 0);
        assert_ne!(pending() & bit(SIGHUP).unwrap(), 0);

        sigprocmask(SIG_UNBLOCK, bit(SIGHUP).unwrap()).unwrap();
        assert_eq!(deliver_pending(), Delivery::Interrupted);
        assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), SIGHUP);
        sigaction(SIGHUP, Some(Action::default())).unwrap();
    }

    #[test]
    fn uncatchable_signals_are_refused() {
        let _serialized = test_lock();
        assert!(matches!(
            sigaction(
                SIGKILL,
                Some(Action {
                    disposition: Disposition::Ignore,
                    flags: 0,
                    mask: 0,
                    restorer: 0,
                }),
            ),
            Err(crate::EINVAL)
        ));
        // Blocking them is silently dropped rather than an error, as in Linux.
        sigprocmask(SIG_BLOCK, bit(SIGKILL).unwrap()).unwrap();
        assert_eq!(blocked_mask() & bit(SIGKILL).unwrap(), 0);
    }

    #[test]
    fn the_job_control_signals_stop_rather_than_terminate() {
        // These four reached the terminating default before job control existed,
        // which turned every `^Z` into an exit with status 148.
        for signal in [SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU] {
            assert!(default_is_stop(signal), "signal {signal} must stop");
            assert!(!default_is_ignore(signal));
        }
        // And nothing else is a stop: SIGTERM and SIGINT still terminate.
        for signal in [SIGTERM, SIGINT, SIGHUP, SIGKILL] {
            assert!(!default_is_stop(signal), "signal {signal} must not stop");
        }
        assert!(default_is_ignore(SIGCHLD));
        assert!(default_is_ignore(SIGCONT));
    }

    #[test]
    fn a_handler_for_a_stop_signal_runs_instead_of_stopping() {
        let _serialized = test_lock();
        OBSERVED.store(0, AtomicOrdering::SeqCst);
        // A caught SIGTSTP is an ordinary delivery. Only the default action
        // suspends, which is why a shell can install a handler and keep running.
        sigaction(
            SIGTSTP,
            Some(Action {
                disposition: Disposition::Handle(record, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        raise_signal(SIGTSTP).unwrap();
        assert_eq!(deliver_pending(), Delivery::Interrupted);
        assert_eq!(OBSERVED.load(AtomicOrdering::SeqCst), SIGTSTP);
        sigaction(SIGTSTP, Some(Action::default())).unwrap();

        // Explicitly ignoring one drops it before it is ever pending, so a
        // process that ignores SIGTTOU is not suspended by terminal output.
        sigaction(
            SIGTTOU,
            Some(Action {
                disposition: Disposition::Ignore,
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .unwrap();
        raise_signal(SIGTTOU).unwrap();
        assert_eq!(pending() & bit(SIGTTOU).unwrap(), 0);
        sigaction(SIGTTOU, Some(Action::default())).unwrap();
    }

    #[test]
    fn swapping_the_blocked_mask_returns_the_previous_one() {
        let _serialized = test_lock();
        sigprocmask(SIG_SETMASK, 0).unwrap();
        let wanted = bit(SIGUSR1).unwrap() | bit(SIGUSR2).unwrap();

        // The swap installs the requested set and hands back what it replaced,
        // which is what a pselect caller restores afterwards.
        assert_eq!(swap_blocked_mask(wanted), 0);
        assert_eq!(blocked_mask(), wanted);

        // The two unblockable signals are dropped from the request rather than
        // making it fail, exactly as sigprocmask drops them.
        assert_eq!(swap_blocked_mask(u64::MAX), wanted);
        assert_eq!(blocked_mask() & bit(SIGKILL).unwrap(), 0);
        assert_eq!(blocked_mask() & bit(SIGSTOP).unwrap(), 0);
        assert_ne!(blocked_mask() & bit(SIGUSR1).unwrap(), 0);

        // And restoring an empty set really empties it.
        assert_ne!(swap_blocked_mask(0), 0);
        assert_eq!(blocked_mask(), 0);
    }
}
