//! Process groups, sessions, cross-process signals and stop/continue.
//!
//! [`crate::signal`] delivers a signal to the process that raised it. That is
//! enough for `raise`, `alarm` and an `EINTR` out of a blocking read, and it is
//! not enough for anything a shell does: `^C` has to reach the pipeline the
//! shell started, `^Z` has to stop it, and `fg` has to start it again. On this
//! host every one of those processes is a separate Windows process — `fork`
//! starts a new image and `exec` starts another one on top of that — so the
//! delivery path has to leave the address space.
//!
//! # How a signal crosses a process boundary
//!
//! Each hosted process owns a slot in the shared registry
//! ([`kinakaze_runtime::job`]) and a named manual-reset event. Under the shared
//! table mutex, a sender sets the event and ORs the signal's bit into the slot.
//! The target resets and drains under that same mutex. The target runs a
//! pump thread which wakes, swaps the pending mask out of its slot, and feeds
//! the signals into exactly the machinery [`crate::signal`] already has: the
//! process-local pending word plus a wake of every interruptible waiter. From
//! the guest's point of view nothing new happens — a blocking call returns
//! `EINTR` and the handler runs at the same point it always did.
//!
//! The event is a wake-up, not the message. The message is the bit in shared
//! memory, so a `SetEvent` that arrives before the target has created its event
//! object loses nothing: the target drains its slot the moment it registers.
//! Process handles and explicit topology events wake the pump without polling.
//! That is what makes the window between
//! `CreateProcess` and the child's first line of code harmless.
//!
//! # Stopping
//!
//! A stopped process really stops. The thread that notices the stop signal
//! suspends every other thread in the process with `SuspendThread` and then
//! parks itself until the slot's state goes back to running. `SIGCONT` from any
//! process writes that state and sets a second named event, which is what wakes
//! the parked thread to resume the others.
//!
//! # Divergences that cannot be fixed here
//!
//! * **Delivery is not asynchronous.** A signal whose default action is to
//!   terminate takes effect when the target next enters libc, not immediately,
//!   because the pump thread cannot know the per-thread blocked masks that
//!   decide whether the signal is deliverable at all. `SIGKILL` is the one
//!   exception — it cannot be blocked, so the pump acts on it directly. A guest
//!   spinning in a compute loop with no libc calls therefore survives a
//!   `SIGTERM` that Linux would have delivered at once.
//! * **Threads are suspended wherever they happen to be.** A kernel stops a
//!   process at a point of its choosing; `SuspendThread` stops a thread mid
//!   instruction. Nothing inside the stop path allocates or takes a lock a
//!   suspended thread could hold, so the stop itself is safe, but a thread
//!   suspended while holding, say, the CRT heap lock blocks anything else that
//!   wants it until the process continues.
//! * **Process creation requires a host-side handoff.** `exec` and
//!   `posix_spawn` create the replacement loader suspended, publish its shared
//!   namespace row and process identity, and only then resume its primary
//!   thread. This preserves Linux-visible PID/group/session ordering even
//!   though Windows still owns the underlying process objects.
//! * **Signals do not queue.** The pending mask is a bitmask, so two `SIGINT`s
//!   delivered before either is handled are one `SIGINT`. Standard signals
//!   behave that way on Linux too; real-time signals do not, and they do not
//!   here either.
//! * **`kill(-1, sig)` reaches hosted processes only.** A native Windows
//!   process has no Linux disposition to run, so including it would mean either
//!   lying or terminating it by a rule the guest did not ask for.

use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, GetCurrentThreadId, GetThreadId, ResetEvent, ResumeThread,
    SetEvent, SuspendThread, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME,
    WaitForSingleObject,
};

use crate::signal::{
    self, SIGCHLD, SIGCONT, SIGHUP, SIGINT, SIGKILL, SIGQUIT, SIGSTOP, SIGTSTP, SIGTTIN, SIGTTOU,
};
use crate::{EACCES, EINVAL, EIO, EPERM};

/// The shared registry itself. Aliased so that every use below reads as a table
/// lookup rather than as a call into another crate's namespace.
use kinakaze_runtime::job as table;
pub use kinakaze_runtime::job::{Entry, ProcessInfo, STATE_RUNNING, STATE_STOPPED, StateChange};
use kinakaze_runtime::job::{FLAG_EXECED, current_host_pid, current_pid as uncached_current_pid};

/// `ESRCH`, which the crate's errno list does not carry because nothing below
/// this module can produce it.
pub const ESRCH: i32 = 3;

/// How long a stopped thread waits before re-reading its state.
const STOP_POLL_MS: u32 = 100;

/// A hosted JVM normally has tens of threads. Keeping this storage inline lets
/// the stop path stay independent of the process heap after the first thread is
/// suspended while still leaving ample room for unusually large applications.
const MAX_STOP_THREADS: usize = 4096;
const CREATE_FAILED: u32 = u32::MAX;
const STATUS_NO_MORE_ENTRIES: i32 = 0x8000_001a_u32 as i32;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        process_handle: HANDLE,
        process_information_class: u32,
        process_information: *mut core::ffi::c_void,
        process_information_length: u32,
        return_length: *mut u32,
    ) -> i32;

    fn NtGetNextThread(
        process_handle: HANDLE,
        thread_handle: HANDLE,
        desired_access: u32,
        handle_attributes: u32,
        flags: u32,
        new_thread_handle: *mut HANDLE,
    ) -> i32;
}

/// Set once this process owns a registry slot, its events and a pump.
static REGISTERED: AtomicBool = AtomicBool::new(false);
/// Distinguishes a live exec restore from statics copied out of a fork parent.
static REGISTERED_HOST: AtomicU32 = AtomicU32::new(0);
/// Native service helpers are implementation details rather than Linux
/// processes.  They may call VFS code while pinning an fd, but must never
/// appear as children in the guest PID table or become waitable zombies.
static INTERNAL_HELPER: AtomicBool = AtomicBool::new(false);
/// A successful exec leaves this host alive only to relay native exit status.
/// It must never allocate another Linux identity when a sibling wakes up.
static EXEC_RETIRED: AtomicBool = AtomicBool::new(false);
/// Native candidate reservation owned only by the image executing execve.
static MANAGED_EXEC_TRANSACTION: AtomicU64 = AtomicU64::new(0);
struct ExecBackup {
    info: ProcessInfo,
    links: Vec<(i32, String)>,
}
static EXEC_BACKUP: Mutex<Option<ExecBackup>> = Mutex::new(None);
/// Linux-visible pid published with the registry slot. Unlike process group
/// and session ids it cannot change during this image's lifetime.
static PROCESS_ID: AtomicU32 = AtomicU32::new(0);
/// Serializes registration against itself.
static REGISTRATION: Mutex<()> = Mutex::new(());
/// This process's wake event, or zero.
static WAKE_EVENT: AtomicUsize = AtomicUsize::new(0);
/// This process's continue event, or zero.
static CONTINUE_EVENT: AtomicUsize = AtomicUsize::new(0);
/// The pump thread's host thread id, which a stop must not suspend.
static PUMP_THREAD: AtomicU32 = AtomicU32::new(0);
static PUMP_STARTED: AtomicBool = AtomicBool::new(false);
/// Private retirement event; unlike signal notifications no other pump drains it.
static PUMP_RETIRE: AtomicUsize = AtomicUsize::new(0);
/// Prevents a wait diagnostic from logging the same stable handles per loop.
static PUMP_WAIT_TRACED: AtomicBool = AtomicBool::new(false);
/// Group, session and parent to adopt when this process registers, published by
/// the fork handoff before the child registers itself.
static INHERITED: Mutex<Option<(u32, u32, u32)>> = Mutex::new(None);
/// Serializes stopping, so two threads noticing the same stop signal do not each
/// try to suspend the other.
static STOPPING: Mutex<()> = Mutex::new(());

/// Returns this image's stable Linux pid without re-entering the shared table.
/// Before registration the runtime lookup preserves the bootstrap behaviour.
fn current_pid() -> u32 {
    if kinakaze_runtime::authority::get().is_some() {
        return kinakaze_runtime::authority::process_id();
    }
    let cached = PROCESS_ID.load(Ordering::Acquire);
    if cached != 0 {
        cached
    } else {
        uncached_current_pid()
    }
}

/// Opens, or creates, the named event `suffix` for `pid`.
///
/// `CreateEventW` on an existing name opens it, which is what lets a sender
/// reach a target's event without knowing whether the target got there first.
/// The returned handle belongs to the caller.
fn named_event(suffix: &str, pid: u32) -> HANDLE {
    // Both event names are short ASCII strings. Build their UTF-16 form on the
    // stack so registration and every cross-process signal avoid a `format!`
    // allocation followed by a second allocation for UTF-16 conversion.
    let mut name = [0u16; 64];
    let mut length = 0usize;
    for unit in "Local\\kinakaze.job."
        .encode_utf16()
        .chain(suffix.encode_utf16())
        .chain(core::iter::once('.' as u16))
    {
        name[length] = unit;
        length += 1;
    }
    let mut digits = [0u16; 10];
    let mut digit_count = 0usize;
    let mut remaining = pid;
    loop {
        digits[digit_count] = u16::from(b'0') + (remaining % 10) as u16;
        digit_count += 1;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    for digit in digits[..digit_count].iter().rev() {
        name[length] = *digit;
        length += 1;
    }
    name[length] = 0;
    // Signal notification is level-triggered. During provider ownership
    // transfer an old pump may still be retiring from its native wait; it must
    // not consume the new owner's wake. Only the owner resets before draining.
    // The continue rendezvous retains its single-consumer auto-reset behavior.
    // SAFETY: a null attribute pointer requests the default descriptor, and the
    // name buffer is live for the duration of the call.
    unsafe {
        CreateEventW(
            core::ptr::null(),
            i32::from(suffix == "wake"),
            0,
            name.as_ptr(),
        )
    }
}

/// Wakes `pid` on one of its named events.
fn poke(suffix: &str, pid: u32) {
    let event = named_event(suffix, pid);
    if event.is_null() {
        return;
    }
    // SAFETY: the handle was just created or opened by this call, and is not
    // used again after being closed.
    unsafe {
        SetEvent(event);
        CloseHandle(event);
    }
}

/// This process's parent, or zero when it cannot be determined.
///
/// Windows records the creating process id in the process entry, which is the
/// closest thing to a POSIX parent it has. It differs in one way that matters:
/// Windows never reparents, so when the creator exits the id stays behind and
/// can be recycled. Every use of the result is therefore guarded by a liveness
/// check on the registry rather than trusted on its own.
fn parent_host_pid() -> u32 {
    #[repr(C)]
    struct ProcessBasicInformation {
        exit_status: i32,
        peb_base_address: *mut core::ffi::c_void,
        affinity_mask: usize,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    // `ntdll` is a required part of the Windows ABI. Calling it directly avoids
    // resolving the same export at registration time and, more importantly,
    // removes the former all-process ToolHelp snapshot fallback.
    unsafe {
        let mut info: ProcessBasicInformation = core::mem::zeroed();
        let mut return_len = 0u32;
        let status = NtQueryInformationProcess(
            GetCurrentProcess(),
            0, // ProcessBasicInformation
            &raw mut info as *mut _,
            core::mem::size_of::<ProcessBasicInformation>() as u32,
            &raw mut return_len,
        );
        if status >= 0 {
            info.inherited_from_unique_process_id as u32
        } else {
            0
        }
    }
}

/// Ensures this process has a registry slot, its events and a pump thread.
///
/// Every public entry point calls this. It is deliberately not a process
/// initializer: a forked child runs its C runtime initializers *before* the
/// parent overwrites its arena and stack, so anything a `.CRT$XCU` hook set up
/// would be replaced by the parent's copy of the same statics — including a
/// handle for a thread that does not exist in the child.
pub fn ensure_registered() {
    let _ = ensure_registered_with_identity(None);
}

/// Excludes this freshly started native helper from Linux process accounting.
/// Call this before the helper enters any VFS operation that can register the
/// process (BPF descriptor activation is one such operation).
pub fn enter_internal_helper() {
    INTERNAL_HELPER.store(true, Ordering::Release);
}

/// Registers the process, optionally publishing its final guest identity in
/// the same critical section. The return value says whether this call claimed
/// the slot and installed that identity.
fn ensure_registered_with_identity(identity: Option<(&str, &str, &[String])>) -> bool {
    if INTERNAL_HELPER.load(Ordering::Acquire)
        || EXEC_RETIRED.load(Ordering::Acquire)
        || REGISTERED.load(Ordering::Relaxed)
    {
        return false;
    }
    // The registration lock is released before anything is delivered. A signal
    // that was already waiting in this process's slot may be a stop, and
    // stopping while holding this lock would leave every other thread's
    // `ensure_registered` blocked until the process is continued.
    {
        let Ok(_serialized) = REGISTRATION.lock() else {
            return false;
        };
        if INTERNAL_HELPER.load(Ordering::Acquire)
            || EXEC_RETIRED.load(Ordering::Acquire)
            || REGISTERED.load(Ordering::Acquire)
        {
            return false;
        }
        if !claim_slot(identity) {
            return false;
        }
    }
    // Anything posted before the event object existed is still in the slot.
    drain_external();
    true
}

/// Installs this process's registry slot, its events, its pump and the console
/// bridge. Called once, under [`REGISTRATION`].
fn claim_slot(identity: Option<(&str, &str, &[String])>) -> bool {
    let host_pid = current_host_pid();
    // These values only seed a fresh row. claim() retains an existing row under
    // the PID-table lock, including adoption and process-group changes made
    // after the fork/exec startup identity was captured.
    let inherited = INHERITED.lock().ok().and_then(|slot| *slot);
    let (pgid, sid, mut ppid) = inherited.unwrap_or_else(|| {
        let ppid = table::lookup_host(parent_host_pid()).map_or(0, |entry| entry.namespace_pid);
        (0, 0, ppid)
    });
    if kinakaze_runtime::authority::get().is_some() {
        ppid = kinakaze_runtime::authority::required_identity().parent_pid;
    }
    let Some(entry) = table::claim(host_pid, pgid, sid, ppid) else {
        return false;
    };
    PROCESS_ID.store(entry.namespace_pid, Ordering::Release);
    if let Some((comm, executable, arguments)) = identity {
        let cmdline = encode_cmdline(arguments);
        table::set_process_identity(entry.namespace_pid, comm, executable, &cmdline);
    } else if table::process_info(entry.namespace_pid).is_none_or(|info| info.comm.is_empty()) {
        let executable = std::env::current_exe()
            .map(|path| crate::to_guest_path(&path))
            .unwrap_or_default();
        let arguments: Vec<String> = std::env::args().collect();
        let cmdline = encode_cmdline(&arguments);
        table::set_process_identity(entry.namespace_pid, "kinakaze", &executable, &cmdline);
    }

    WAKE_EVENT.store(named_event("wake", entry.pid) as usize, Ordering::Release);
    CONTINUE_EVENT.store(named_event("cont", entry.pid) as usize, Ordering::Release);
    if WAKE_EVENT.load(Ordering::Acquire) == 0 || CONTINUE_EVENT.load(Ordering::Acquire) == 0 {
        std::process::abort();
    }
    // A sender may have posted before we retained the named event. Publish one
    // bootstrap wake after acquiring the handle, before any indefinite wait.
    if unsafe { SetEvent(WAKE_EVENT.load(Ordering::Acquire) as HANDLE) } == 0 {
        std::process::abort();
    }

    // Published before the pump starts, because the pump immediately drains and
    // draining is a no-op for an unregistered process.
    REGISTERED_HOST.store(host_pid, Ordering::Release);
    REGISTERED.store(true, Ordering::Release);
    let _ = crate::credentials::publish();
    start_pump();
    install_console_handler();
    true
}

fn pump_failed(stage: &str) -> ! {
    let message = format!(
        "kinakaze: signal pump failed at {stage}, host_pid={}, guest_pid={}, win32={}",
        current_host_pid(),
        current_pid(),
        unsafe { GetLastError() }
    );
    eprintln!("{message}");
    if let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR") {
        let _ = std::fs::write(
            std::path::PathBuf::from(directory)
                .join(format!("pump-failure-{}.log", current_host_pid())),
            message,
        );
    }
    std::process::abort()
}

/// Starts the thread that turns a slot's pending bits into local deliveries.
fn start_pump() {
    use crate::epoll::native_wait;
    use std::os::windows::io::{AsRawHandle, IntoRawHandle};
    if !crate::fork_handoff::is_owner() {
        return;
    }
    if PUMP_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let retirement = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) };
    if retirement.is_null() || !crate::fork_handoff::watch_owner(retirement as usize) {
        pump_failed("watch owner");
    }
    PUMP_RETIRE.store(retirement as usize, Ordering::Release);
    let changed = table::notifications::event("changes", current_host_pid())
        .unwrap_or_else(|| pump_failed("create changes event"))
        .into_raw_handle() as usize;
    let started = std::thread::Builder::new()
        .name("kinakaze-signal-pump".into())
        .spawn(move || {
            // SAFETY: GetCurrentThreadId has no preconditions.
            PUMP_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Release);
            loop {
                // Loading a higher-priority provider can transfer ownership.
                // A private event wakes the old owner even when the new owner's
                // pump has already consumed every shared notification.
                if EXEC_RETIRED.load(Ordering::Acquire) || !crate::fork_handoff::is_owner() {
                    PUMP_THREAD.store(0, Ordering::Release);
                    return;
                }
                let wake = WAKE_EVENT.load(Ordering::Acquire) as HANDLE;
                let child_activity = kinakaze_runtime::child_activity_event() as HANDLE;
                let trace_target = std::env::var_os("KINAKAZE_WAIT_TRACE")
                    .map(|value| value.to_string_lossy().into_owned());
                if trace_target.is_some_and(|target| {
                    target == "1"
                        || std::env::args_os()
                            .any(|argument| argument.to_string_lossy().contains(&target))
                }) && !PUMP_WAIT_TRACED.swap(true, Ordering::AcqRel)
                {
                    eprintln!(
                        "kinakaze signal pump: host_pid={} wake={:#x} child_activity={:#x}",
                        std::process::id(),
                        wake as usize,
                        child_activity as usize
                    );
                }
                if wake.is_null() || child_activity.is_null() {
                    pump_failed("missing wake or child activity event");
                }
                // Reset before scanning. Exact handles cover late teardown and
                // inherited/adopted children absent from the local fork registry.
                if unsafe { ResetEvent(child_activity) } == 0 {
                    pump_failed("reset child activity");
                }
                notice_exited_children();
                let sources = table::notifications::sources(current_pid(), changed)
                    .unwrap_or_else(|_| pump_failed("child sources"));
                drain_external_notified(true);
                if sources.rescan {
                    continue;
                }
                let mut events = vec![
                    PUMP_RETIRE.load(Ordering::Acquire) as HANDLE,
                    wake,
                    changed as HANDLE,
                    child_activity,
                ];
                events.extend(sources.handles.iter().map(AsRawHandle::as_raw_handle));
                // Fan-in retains all sources beyond Windows' 64-handle limit.
                // It retires callbacks before `sources` releases the handles.
                match unsafe { native_wait::wait(&events, core::ptr::null_mut(), u32::MAX) } {
                    Ok(native_wait::Outcome::Ready) => {}
                    _ => pump_failed(&format!("native wait {events:x?}")),
                }
            }
        });
    if started.is_err() {
        // Event-driven delivery must not silently degrade to no observer.
        pump_failed("create thread");
    }
}

/// Moves anything posted to this process's slot into local delivery.
///
/// Returns whether any signal was taken. [`crate::signal::deliver_pending`]
/// calls this so that a process without a working pump thread still observes
/// cross-process signals at every point it would have observed a local one.
pub fn drain_external() -> bool {
    drain_external_notified(false)
}

fn drain_external_notified(from_pump: bool) -> bool {
    if EXEC_RETIRED.load(Ordering::Acquire)
        || !REGISTERED.load(Ordering::Relaxed)
        || !crate::fork_handoff::is_owner()
    {
        return false;
    }
    let wake = WAKE_EVENT.load(Ordering::Acquire) as HANDLE;
    // Guest libc hot paths retain a pure userspace empty-mask check. The pump
    // must also drain a pre-commit wake with no visible bit yet: it takes the
    // publisher's mutex before resetting that event, rather than losing it.
    if wake.is_null() || (!from_pump && !table::notifications::has_pending(current_pid())) {
        return false;
    }
    let mut pending = table::notifications::take_pending(current_pid(), wake as usize)
        .unwrap_or_else(|_| pump_failed("drain pending"));
    if pending == 0 {
        return false;
    }
    // SIGCONT is taken first regardless of its bit position: a process that is
    // stopped has to resume before it can act on anything else.
    let cont_bit = 1u64 << (SIGCONT as u64 - 1);
    if pending & cont_bit != 0 {
        pending &= !cont_bit;
        arrive(SIGCONT);
    }
    while pending != 0 {
        let index = pending.trailing_zeros();
        pending &= !(1u64 << index);
        arrive(index as i32 + 1);
    }
    true
}

/// Raises `SIGCHLD` locally when one of this process's children has exited.
///
/// Final exit reports precede native descriptor teardown. The pump observes
/// kernel completion before signalling, so a SIGCHLD-driven WNOHANG reaper can
/// collect the child immediately. Stop/continue notifications remain immediate.
fn notice_exited_children() {
    if !REGISTERED.load(Ordering::Relaxed) || !crate::fork_handoff::is_owner() {
        return;
    }
    match table::reap_dead_children(current_pid()) {
        Ok(true) => {
            let _ = signal::raise_signal(SIGCHLD);
        }
        Ok(false) => {}
        Err(()) => {
            // The shared process table is the only authoritative PID/wait
            // state. Losing it cannot be treated as "no child exited".
            eprintln!("kinakaze: shared process registry unavailable while reaping children");
        }
    }
    match table::take_parent_death_signal(current_pid()) {
        Ok(Some(number)) => arrive(number as i32),
        Ok(None) => {}
        Err(()) => {
            eprintln!("kinakaze: shared process registry unavailable while checking parent death");
        }
    }
}

/// Implements the process-scoped `PR_SET_PDEATHSIG` state.
pub fn set_parent_death_signal(signal: i32) -> Result<(), i32> {
    if signal < 0 || signal as usize >= signal::NSIG {
        return Err(EINVAL);
    }
    ensure_registered();
    table::set_parent_death_signal(current_pid(), signal as u32).map_err(|()| EIO)
}

/// Returns the process-scoped `PR_SET_PDEATHSIG` state.
pub fn parent_death_signal() -> Result<i32, i32> {
    ensure_registered();
    table::parent_death_signal(current_pid())
        .map(|signal| signal as i32)
        .map_err(|()| EIO)
}

/// Reads one process's enforced `RLIMIT_NOFILE` values.
pub fn nofile_limits(pid: u32) -> Result<(u64, u64), i32> {
    ensure_registered();
    let target = if pid == 0 { current_pid() } else { pid };
    if table::process_info(target).is_none() {
        return Err(ESRCH);
    }
    table::nofile_limits(target).map_err(|()| EIO)
}

/// Updates one process's enforced `RLIMIT_NOFILE` values.
pub fn set_nofile_limits(pid: u32, soft: u64, hard: u64) -> Result<(), i32> {
    if soft > hard {
        return Err(EINVAL);
    }
    if hard > crate::MAX_FDS as u64 {
        return Err(EPERM);
    }
    ensure_registered();
    let target = if pid == 0 { current_pid() } else { pid };
    if table::process_info(target).is_none() {
        return Err(ESRCH);
    }
    // Raising the hard ceiling requires CAP_SYS_RESOURCE in the initial user
    // namespace, not merely capabilities inside an unprivileged container.
    table::set_nofile_limits(target, soft, hard, crate::user_namespace::capable(1, 24))
}

/// Soft descriptor ceiling enforced by every allocator in the VFS.
pub fn current_nofile_limit() -> Result<usize, i32> {
    // Escrow/broker processes have a private VFS table but intentionally have
    // no Linux PID-table row. They still need to install the small set of
    // handles they pin on a guest's behalf.
    if INTERNAL_HELPER.load(Ordering::Acquire) {
        return Ok(crate::MAX_FDS);
    }
    let (soft, _) = nofile_limits(0)?;
    usize::try_from(soft.min(crate::MAX_FDS as u64)).map_err(|_| EIO)
}

/// Acts on one signal that arrived from another process.
fn arrive(number: i32) {
    if namespace_init(current_pid())
        && !matches!(number, SIGKILL | SIGSTOP)
        && signal::current_action(number).is_ok_and(|a| {
            matches!(
                a.disposition,
                signal::Disposition::Default | signal::Disposition::Ignore
            )
        })
    {
        return;
    }
    match number {
        // SIGCONT resumes first and is then delivered, because a handler for it
        // has to run in a process that is already running again.
        SIGCONT => {
            resume_self();
            let _ = signal::raise_signal(SIGCONT);
        }
        // SIGKILL cannot be caught, blocked or ignored, so there is no mask this
        // thread would have to consult before acting on it.
        SIGKILL => signal::terminate_now(SIGKILL),
        SIGSTOP => stop_self(SIGSTOP),
        SIGTSTP | SIGTTIN | SIGTTOU => {
            // A job-control stop with a handler installed is an ordinary
            // delivery; only the default action suspends the process.
            match signal::current_action(number).map(|action| action.disposition) {
                Ok(signal::Disposition::Default) => stop_self(number),
                Ok(signal::Disposition::Ignore) => {}
                _ => {
                    let _ = signal::raise_signal(number);
                }
            }
        }
        // Everything else takes the local path, which already knows how to drop
        // an ignored signal and how to wake an interruptible waiter.
        other => {
            let _ = signal::raise_signal(other);
        }
    }
}

// ---------------------------------------------------------------------------
// The seam the terminal line discipline uses.
// ---------------------------------------------------------------------------

/// Delivers `signal` to every process in group `pgid`.
///
/// Used by the tty line discipline for `^C`, `^\` and `^Z`, and by
/// `kill(-pgid)`. A `signal` of zero performs the existence check without
/// delivering anything, which is what a caller probing for a live group wants.
///
/// Reports `ESRCH` when the group has no live members. That is the honest answer
/// and the one a shell reads to decide a job has finished.
pub fn signal_process_group(pgid: i32, signal: i32) -> Result<(), i32> {
    if pgid <= 0 {
        return Err(EINVAL);
    }
    if signal < 0 || (signal != 0 && (signal as usize) >= signal::NSIG) {
        return Err(EINVAL);
    }
    ensure_registered();
    let members = table::members(pgid as u32);
    if members.is_empty() {
        return Err(ESRCH);
    }
    if signal == 0 {
        return Ok(());
    }
    let mut delivered = false;
    for member in members {
        if deliver_to(member.namespace_pid, signal) {
            delivered = true;
        }
    }
    if delivered { Ok(()) } else { Err(ESRCH) }
}

/// The calling process's group id.
pub fn current_pgid() -> i32 {
    ensure_registered();
    let pid = current_pid();
    table::lookup(pid).map_or(pid as i32, |entry| entry.pgid as i32)
}

/// The calling process's session id.
pub fn current_sid() -> i32 {
    ensure_registered();
    let pid = current_pid();
    table::lookup(pid).map_or(pid as i32, |entry| entry.sid as i32)
}

/// The calling process id as seen by Linux code.
pub fn process_id() -> u32 {
    ensure_registered();
    current_pid()
}

/// The calling process's parent in the shared PID namespace.
pub fn parent_process_id() -> u32 {
    ensure_registered();
    // The shared registry owns Linux reparenting, including PID-namespace init
    // and subreapers. The manager's startup parent is not this live relation.
    table::lookup(current_pid()).map_or(0, |entry| entry.ppid)
}

/// Snapshot of the live processes visible in this PID namespace.
pub fn processes() -> Vec<Entry> {
    ensure_registered();
    table::everyone()
}

/// Converts a Linux namespace pid to the real Windows pid.
pub fn host_pid(pid: u32) -> Option<u32> {
    ensure_registered();
    table::host_pid(pid)
}

/// Converts a real Windows pid to the Linux namespace pid.
pub fn namespace_pid(host_pid: u32) -> Option<u32> {
    ensure_registered();
    table::namespace_pid(host_pid)
}

/// Publishes the identity shown by `/proc` and process-listing tools.
pub fn set_process_identity(comm: &str, executable: &str, arguments: &[String]) -> bool {
    if ensure_registered_with_identity(Some((comm, executable, arguments))) {
        return true;
    }
    let cmdline = encode_cmdline(arguments);
    table::set_process_identity(current_pid(), comm, executable, &cmdline)
}

fn encode_cmdline(arguments: &[String]) -> Vec<u8> {
    let mut cmdline = Vec::new();
    for argument in arguments {
        cmdline.extend_from_slice(argument.as_bytes());
        cmdline.push(0);
    }
    cmdline
}

/// Linux-facing metadata for every visible process.
pub fn process_infos() -> Vec<ProcessInfo> {
    ensure_registered();
    table::all_process_info()
}

/// Looks up one process directly through the namespace index.
///
/// This avoids building and copying metadata for every hosted process when a
/// caller only needs `/proc/self` or one numeric `/proc/<pid>` entry.
pub fn process_info(pid: u32) -> Option<ProcessInfo> {
    ensure_registered();
    table::process_info(pid)
}

/// Registers a child created by `posix_spawn` before its suspended primary
/// thread is resumed, returning the pid visible to the guest.
pub fn register_spawned_child(
    child_host_pid: u32,
    comm: &str,
    executable: &str,
    arguments: &[String],
) -> Option<u32> {
    ensure_registered();
    let own = table::lookup(current_pid())?;
    let child = table::register(child_host_pid, own.pgid, own.sid, own.namespace_pid, 0)?;
    let cmdline = encode_cmdline(arguments);
    table::set_process_identity(child.namespace_pid, comm, executable, &cmdline);
    Some(child.namespace_pid)
}

/// Whether `pgid` is a group in session `sid`.
///
/// This is the check `tcsetpgrp` needs before accepting a foreground group: a
/// terminal may only be handed to a group in the session that owns it, and a
/// request naming a group from another session is `EPERM` rather than a lie the
/// caller finds out about later.
pub fn group_in_session(pgid: i32, sid: i32) -> bool {
    if pgid <= 0 || sid <= 0 {
        return false;
    }
    ensure_registered();
    table::members(pgid as u32)
        .iter()
        .any(|member| member.sid == sid as u32)
}

// ---------------------------------------------------------------------------
// Delivery.
// ---------------------------------------------------------------------------

fn namespace_init(pid: u32) -> bool {
    table::namespaces::memberships(pid).is_some_and(|v| v[table::namespaces::PID] != 1)
        && table::namespaces::visible_from(pid, pid) == Some(1)
}

/// Posts `signal` to one process, reporting whether it had somewhere to land.
pub(crate) fn deliver_to(pid: u32, signal: i32) -> bool {
    // An `exec`ed program lives in a different process from the wrapper its
    // parent is waiting on. The wrapper cannot run a guest handler, so the
    // signal follows the delegate link to the process that can.
    let target = match table::resolve(pid) {
        Some(entry) => entry,
        None => return false,
    };
    if namespace_init(target.namespace_pid) {
        let target_ns =
            table::namespaces::memberships(target.namespace_pid).map(|v| v[table::namespaces::PID]);
        let caller_ns =
            table::namespaces::memberships(current_pid()).map(|v| v[table::namespaces::PID]);
        if matches!(signal, SIGKILL | SIGSTOP) && target_ns == caller_ns {
            return true;
        }
        if target.namespace_pid == current_pid()
            && signal::current_action(signal).is_ok_and(|a| {
                matches!(
                    a.disposition,
                    signal::Disposition::Default | signal::Disposition::Ignore
                )
            })
        {
            return true;
        }
    }
    if target.namespace_pid == current_pid() {
        // A self-directed signal takes the local path, which stops this process
        // at its next delivery point rather than from inside whatever call is
        // running now. Suspending our own threads from here would mean stopping
        // in the middle of an arbitrary lock scope.
        return signal::raise_signal(signal).is_ok();
    }
    let Some(bit) = signal_bit(signal) else {
        return false;
    };
    // SIGCONT resumes even a process that ignores it, so the state is cleared
    // by the sender rather than by the target's handler.
    if signal == SIGCONT {
        table::set_state(target.namespace_pid, STATE_RUNNING, 0);
        poke("cont", target.pid);
    }
    if !table::post_pending(target.namespace_pid, bit) {
        return false;
    }
    true
}

/// Returns the mask bit for `signal`, or `None` when it is out of range.
fn signal_bit(signal: i32) -> Option<u64> {
    if signal <= 0 || signal as usize >= signal::NSIG {
        return None;
    }
    Some(1u64 << (signal as u64 - 1))
}

/// Full `kill(2)` target selection.
///
/// `pid > 0` names one process, `pid == 0` the caller's own group, `pid < -1`
/// the group `-pid`, and `pid == -1` every process this one may signal — which
/// here means every other hosted process, since a native Windows process has no
/// Linux disposition to run.
pub fn kill(pid: i32, signal: i32) -> Result<(), i32> {
    if signal < 0 || (signal != 0 && (signal as usize) >= signal::NSIG) {
        return Err(EINVAL);
    }
    ensure_registered();
    match pid {
        0 => signal_process_group(current_pgid(), signal),
        -1 => broadcast(signal),
        target if target < -1 => signal_process_group(
            table::namespaces::resolve(target.checked_neg().ok_or(ESRCH)? as u32).ok_or(ESRCH)?
                as i32,
            signal,
        ),
        target => {
            let target = table::namespaces::resolve(target as u32).ok_or(ESRCH)? as i32;
            if table::lookup(target as u32).is_none() {
                return Err(ESRCH);
            }
            if signal == 0 {
                return Ok(());
            }
            if deliver_to(target as u32, signal) {
                Ok(())
            } else {
                Err(ESRCH)
            }
        }
    }
}

/// `kill(-1, sig)`: every hosted process except namespace init and the caller.
fn broadcast(signal: i32) -> Result<(), i32> {
    let own = current_pid();
    let mut reached = false;
    for entry in table::everyone() {
        if !broadcast_target(entry.namespace_pid, own)
            || table::namespaces::visible(entry.namespace_pid).is_none_or(|pid| pid <= 1)
        {
            continue;
        }
        if signal == 0 || deliver_to(entry.namespace_pid, signal) {
            reached = true;
        }
    }
    if reached { Ok(()) } else { Err(ESRCH) }
}

fn broadcast_target(pid: u32, caller: u32) -> bool {
    pid > 1 && pid != caller
}

// ---------------------------------------------------------------------------
// Stop and continue.
// ---------------------------------------------------------------------------

/// Stops this process until a `SIGCONT` arrives.
///
/// Called from whichever thread noticed the stop signal: the pump thread when it
/// came from outside, or a guest thread inside `deliver_pending` when it was
/// raised locally. That thread suspends every other thread in the process,
/// parks, and resumes them once the slot's state goes back to running.
///
/// The parent is told before anything is suspended, because telling it means
/// taking the registry's shared mutex and a thread that is about to be suspended
/// might be holding it.
pub fn stop_self(signal: i32) {
    ensure_registered();
    let Ok(_serialized) = STOPPING.lock() else {
        return;
    };
    let pid = current_pid();
    if table::state(pid) == STATE_STOPPED {
        return;
    }

    notify_parent(StateChange::Stopped(signal as u32));

    // Everything that allocates happens before the first thread is suspended. A
    // suspended thread may hold the allocator's lock, and a stop that then
    // allocated would deadlock against the process it had just frozen.
    let frozen = match unsafe { FrozenThreads::freeze() } {
        Ok(frozen) => frozen,
        Err(error) => {
            #[cfg(test)]
            eprintln!("stop_self: thread suspension failed: {error:#x}");
            #[cfg(not(test))]
            let _ = error;
            table::set_state(pid, STATE_RUNNING, 0);
            notify_parent(StateChange::Continued);
            return;
        }
    };
    let resume_event = CONTINUE_EVENT.load(Ordering::Acquire) as HANDLE;
    while table::state(pid) == STATE_STOPPED {
        if resume_event.is_null() {
            // The fallback for a process whose event could not be created.
            std::thread::sleep(std::time::Duration::from_millis(u64::from(STOP_POLL_MS)));
        } else {
            // SAFETY: the handle is owned by this process for its lifetime.
            unsafe { WaitForSingleObject(resume_event, STOP_POLL_MS) };
        }
    }
    drop(frozen);

    notify_parent(StateChange::Continued);
}

/// Clears the stopped state, which is what lets a parked stopper resume.
fn resume_self() {
    let pid = current_pid();
    if table::state(pid) != STATE_STOPPED {
        return;
    }
    table::set_state(pid, STATE_RUNNING, 0);
    let resume_event = CONTINUE_EVENT.load(Ordering::Acquire) as HANDLE;
    if !resume_event.is_null() {
        // SAFETY: the handle is owned by this process for its lifetime.
        unsafe { SetEvent(resume_event) };
    }
}

/// Suspends every thread of this process except the caller and the pump.
///
/// The pump is spared so the process still answers `SIGKILL` while stopped,
/// which is the one signal a real kernel also honours for a stopped process.
///
/// `NtGetNextThread` walks only this process and returns an already-open handle,
/// avoiding ToolHelp's global system snapshot and the subsequent `OpenThread`
/// race. Storage is fixed because a suspended thread may own the allocator.
struct FrozenThreads {
    suspended: [HANDLE; MAX_STOP_THREADS],
    suspended_len: usize,
    skipped: [HANDLE; 2],
    skipped_len: usize,
}

impl FrozenThreads {
    unsafe fn freeze() -> Result<Self, u32> {
        let mut frozen = Self {
            suspended: [core::ptr::null_mut(); MAX_STOP_THREADS],
            suspended_len: 0,
            skipped: [core::ptr::null_mut(); 2],
            skipped_len: 0,
        };
        let own_thread = unsafe { GetCurrentThreadId() };
        let pump = PUMP_THREAD.load(Ordering::Acquire);
        let mut cursor = core::ptr::null_mut();

        loop {
            let mut thread = core::ptr::null_mut();
            let status = unsafe {
                NtGetNextThread(
                    GetCurrentProcess(),
                    cursor,
                    THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    0,
                    &mut thread,
                )
            };
            if status == STATUS_NO_MORE_ENTRIES {
                break;
            }
            if status < 0 || thread.is_null() {
                return Err(status as u32);
            }
            cursor = thread;

            let id = unsafe { GetThreadId(thread) };
            if id == own_thread || id == pump {
                if frozen.skipped_len == frozen.skipped.len() {
                    unsafe { CloseHandle(thread) };
                    return Err(0);
                }
                frozen.skipped[frozen.skipped_len] = thread;
                frozen.skipped_len += 1;
                continue;
            }
            if frozen.suspended_len == MAX_STOP_THREADS {
                unsafe { CloseHandle(thread) };
                return Err(0);
            }
            if unsafe { SuspendThread(thread) } == CREATE_FAILED {
                let error = unsafe { GetLastError() };
                unsafe { CloseHandle(thread) };
                return Err(error);
            }
            frozen.suspended[frozen.suspended_len] = thread;
            frozen.suspended_len += 1;
        }

        for handle in &mut frozen.skipped[..frozen.skipped_len] {
            unsafe { CloseHandle(*handle) };
            *handle = core::ptr::null_mut();
        }
        frozen.skipped_len = 0;
        Ok(frozen)
    }
}

impl Drop for FrozenThreads {
    fn drop(&mut self) {
        for &thread in self.suspended[..self.suspended_len].iter().rev() {
            // Restore exactly the suspension count this stop added.
            unsafe {
                ResumeThread(thread);
                CloseHandle(thread);
            }
        }
        for &thread in &self.skipped[..self.skipped_len] {
            unsafe { CloseHandle(thread) };
        }
    }
}

/// Leaves a state change for the parent and wakes it with `SIGCHLD`.
///
/// A parent that is not itself a hosted process has no slot, in which case the
/// report is still filed but nobody reads it — there is no `waitpid` on the
/// other side to collect it.
fn notify_parent(change: StateChange) {
    let pid = current_pid();
    let Some(entry) = table::lookup(pid) else {
        return;
    };
    // The report is filed against this process's namespace slot; `waitpid`
    // looks it up by the same child pid.
    match change {
        StateChange::Stopped(signal) => table::post_stop(pid, signal),
        _ => table::post_report(pid, change),
    }
    if entry.ppid != 0
        && let Some(parent) = table::lookup(entry.ppid)
    {
        if matches!(change, StateChange::Stopped(_) | StateChange::Continued) {
            deliver_to(entry.ppid, SIGCHLD);
        } else {
            // Wake the observer, but let it establish kernel completion before
            // publishing SIGCHLD. Its pinned process handle wakes at teardown.
            poke("wake", parent.pid);
        }
    }
}

/// Records that this process is about to die from `signal` rather than exit.
///
/// The Windows exit code cannot express the difference — a process killed by
/// `SIGINT` and one that called `exit(130)` leave the same number behind — so
/// the distinction is written to the registry, where the parent's `waitpid`
/// reads it to build a `WIFSIGNALED` status.
pub fn publish_termination(signal: i32) {
    if !REGISTERED.load(Ordering::Relaxed) {
        return;
    }
    notify_parent(StateChange::Killed(signal as u32));
}

/// Publishes an ordinary `_exit(2)` status before the host process disappears.
///
/// The shared zombie row is authoritative for `waitid(2)`, including a child
/// created with `CLONE_PARENT` whose actual parent never owned the creator's
/// process-local Windows handle.
pub fn publish_exit(status: i32) {
    if !REGISTERED.load(Ordering::Relaxed) {
        if crate::diagnostic_target_enabled("KINAKAZE_WAIT_TRACE") {
            eprintln!(
                "kinakaze wait: host_pid={} skipped exit={} because this exec wrapper no longer owns a Linux identity",
                current_host_pid(),
                status,
            );
        }
        return;
    }
    if crate::diagnostic_target_enabled("KINAKAZE_WAIT_TRACE") {
        eprintln!(
            "kinakaze wait: host_pid={} publishing child={} exit={}",
            current_host_pid(),
            current_pid(),
            status,
        );
    }
    notify_parent(StateChange::Exited(status as u32 & 0xff));
}

// ---------------------------------------------------------------------------
// Group and session membership.
// ---------------------------------------------------------------------------

/// `getpgid(2)`.
pub fn getpgid(pid: i32) -> Result<i32, i32> {
    if pid < 0 {
        return Err(ESRCH);
    }
    ensure_registered();
    let target = if pid == 0 {
        current_pid()
    } else {
        table::namespaces::resolve(pid as u32).ok_or(ESRCH)?
    };
    table::lookup(target)
        .map(|entry| table::namespaces::visible(entry.pgid).unwrap_or(0) as i32)
        .ok_or(ESRCH)
}

/// `getsid(2)`.
pub fn getsid(pid: i32) -> Result<i32, i32> {
    if pid < 0 {
        return Err(ESRCH);
    }
    ensure_registered();
    let target = if pid == 0 {
        current_pid()
    } else {
        table::namespaces::resolve(pid as u32).ok_or(ESRCH)?
    };
    table::lookup(target)
        .map(|entry| table::namespaces::visible(entry.sid).unwrap_or(0) as i32)
        .ok_or(ESRCH)
}

/// `setpgid(2)`, with the POSIX restrictions that make job control safe.
///
/// The rules are not arbitrary and each is load-bearing for a shell:
///
/// * Only the caller or one of its children may be moved, so a process cannot
///   rearrange an unrelated pipeline.
/// * A child that has already `exec`ed is refused with `EACCES`. This is what
///   makes the shell's habit of calling `setpgid` in *both* the parent and the
///   child race-free: whichever runs second is either redundant or refused, and
///   never wrong.
/// * A session leader cannot leave its own group, and a process cannot join a
///   group in another session. Together these stop a session's groups from
///   leaking into each other, which is the invariant `tcsetpgrp` relies on.
pub fn setpgid(pid: i32, pgid: i32) -> Result<(), i32> {
    if pid < 0 || pgid < 0 {
        return Err(EINVAL);
    }
    ensure_registered();
    let own = current_pid();
    let target = if pid == 0 {
        own
    } else {
        table::namespaces::resolve(pid as u32).ok_or(ESRCH)?
    };
    let group = if pgid == 0 {
        target
    } else {
        table::namespaces::resolve(pgid as u32).ok_or(EPERM)?
    };

    let Some(caller) = table::lookup(own) else {
        return Err(ESRCH);
    };
    let Some(subject) = table::lookup(target) else {
        return Err(ESRCH);
    };
    if target != own && subject.ppid != own {
        return Err(ESRCH);
    }
    if target != own && subject.flags & FLAG_EXECED != 0 {
        return Err(EACCES);
    }
    if subject.is_session_leader() {
        return Err(EPERM);
    }
    if subject.sid != caller.sid {
        return Err(EPERM);
    }
    // Joining an existing group is only allowed within one session. A group id
    // nobody holds yet is a new group, which is legal exactly when the process
    // is becoming its own leader.
    if group != target {
        let members = table::members(group);
        if members.is_empty() || members.iter().any(|member| member.sid != subject.sid) {
            return Err(EPERM);
        }
    }
    if table::set_group(target, group) {
        Ok(())
    } else {
        Err(ESRCH)
    }
}

/// `setsid(2)`, which creates a session and returns its id.
///
/// Fails with `EPERM` when the caller already leads a process group, because the
/// new session id would collide with the existing group id. This is the one
/// place where reporting success would be actively harmful: a daemonizing
/// program calls `fork` precisely so the child is *not* a group leader, and
/// silently succeeding for a leader would hide the bug where it forgot to.
pub fn setsid() -> Result<i32, i32> {
    ensure_registered();
    let own = current_pid();
    let Some(entry) = table::lookup(own) else {
        return Err(EPERM);
    };
    if entry.is_group_leader() {
        return Err(EPERM);
    }
    if !table::set_session(own, own, own) {
        return Err(EPERM);
    }
    Ok(table::namespaces::visible(own).ok_or(ESRCH)? as i32)
}

/// Records the process group the host console treats as foreground.
///
/// The Windows console is one terminal shared by the whole session, so this is
/// one value rather than one per descriptor. A pty layer tracking a foreground
/// group per terminal owns that finer state; this is only what the console
/// control handler needs in order to aim a `^C`.
pub fn set_foreground_pgid(pgid: i32) {
    if pgid <= 0 {
        return;
    }
    ensure_registered();
    table::set_foreground_pgid(pgid as u32);
}

/// The console's foreground process group, or zero when none is established.
pub fn foreground_pgid() -> i32 {
    ensure_registered();
    table::foreground_pgid() as i32
}

// ---------------------------------------------------------------------------
// The Windows console control bridge.
// ---------------------------------------------------------------------------

/// Turns a console control event into a guest signal.
///
/// Windows delivers these to every process attached to the console, on a thread
/// it creates for the purpose. Returning true is what suppresses the default
/// action, which for `CTRL_C_EVENT` is terminating this process outright — the
/// thing that made `^C` unusable before this existed.
///
/// Every attached hosted process runs this and aims at the same foreground
/// group, so that group receives one signal per attached process. They coalesce:
/// the pending set is a bitmask, so N copies of `SIGINT` are one `SIGINT`.
///
/// A close, logoff or shutdown is a hangup — the terminal is going away.
/// Windows allows only a few seconds before killing the process regardless, so
/// this is a chance to run a handler rather than a guarantee of one.
///
/// # Safety
///
/// Called by the Windows console subsystem with a control-event code.
unsafe extern "system" fn console_handler(kind: u32) -> i32 {
    let signal = match kind {
        CTRL_C_EVENT => SIGINT,
        CTRL_BREAK_EVENT => SIGQUIT,
        CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT | CTRL_SHUTDOWN_EVENT => SIGHUP,
        _ => return 0,
    };
    let mut target = table::foreground_pgid() as i32;
    if target <= 0 {
        // No foreground group has been established, which is the state of a
        // program that never called `tcsetpgrp`. Its own group is the honest
        // target: it is the only thing on this console the process knows about.
        target = current_pgid();
    }
    let _ = signal_process_group(target, signal);
    1
}

/// Installs the console bridge once per process.
fn install_console_handler() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: the routine has the documented handler signature and stays valid
    // for the life of the process.
    unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) };
}

// ---------------------------------------------------------------------------
// Crossing fork and exec.
// ---------------------------------------------------------------------------

/// Magic word identifying the job section of the signal handoff payload.
const HANDOFF_MAGIC: u32 = 0x4a4f_4231; // "JOB1"

/// Serializes the group identity a forked child must adopt.
pub(crate) fn serialize_fork_state() -> Result<Vec<u8>, i32> {
    ensure_registered();
    let pid = current_pid();
    let entry = table::lookup(pid).ok_or(EIO)?;
    // Ordinary fork uses the caller. `clone(CLONE_PARENT)` publishes the
    // caller's parent through the process coordinator before this snapshot.
    let child_parent = kinakaze_runtime::fork_parent_override()
        .map_err(|_| EIO)?
        .unwrap_or(pid);
    serialize_identity(entry.pgid, entry.sid, child_parent)
}

/// Serializes the identity retained across an `execve` image replacement.
pub(crate) fn serialize_exec_state() -> Result<Vec<u8>, i32> {
    ensure_registered();
    let entry = table::lookup(current_pid()).ok_or(EIO)?;
    serialize_identity(entry.pgid, entry.sid, entry.ppid)
}

fn serialize_identity(pgid: u32, sid: u32, ppid: u32) -> Result<Vec<u8>, i32> {
    let mut payload = Vec::with_capacity(16);
    payload.extend_from_slice(&HANDOFF_MAGIC.to_le_bytes());
    payload.extend_from_slice(&pgid.to_le_bytes());
    payload.extend_from_slice(&sid.to_le_bytes());
    payload.extend_from_slice(&ppid.to_le_bytes());
    Ok(payload)
}

/// Adopts the parent's group identity in a freshly forked child.
///
/// Everything process-local is reset first. The child inherited the parent's
/// arena, and therefore the parent's idea of which slot is "mine", which event
/// handles are open and which thread is the pump — all of which describe a
/// process that is not this one. The mapped view is in that category too: the
/// fork backend copies the arena and the stack and nothing else, so the cached
/// base address points at memory the child never mapped.
pub(crate) fn restore_fork_state(payload: &[u8]) -> bool {
    if payload.len() != 16
        || u32::from_le_bytes(payload[0..4].try_into().unwrap_or_default()) != HANDOFF_MAGIC
    {
        return false;
    }
    let pgid = u32::from_le_bytes(payload[4..8].try_into().unwrap_or_default());
    let sid = u32::from_le_bytes(payload[8..12].try_into().unwrap_or_default());
    let ppid = u32::from_le_bytes(payload[12..16].try_into().unwrap_or_default());

    let Ok(mut inherited) = INHERITED.lock() else {
        return false;
    };
    *inherited = Some((pgid, sid, ppid));
    drop(inherited);

    // Exec restoration can enter after this provider has already registered
    // itself while restoring descriptors. Its pump is a live native thread:
    // clearing its handles creates invalid waits and duplicate consumers.
    // Only a real fork carries another host process's statics and handles.
    if REGISTERED_HOST.load(Ordering::Acquire) != current_host_pid() {
        EXEC_RETIRED.store(false, Ordering::Release);
        REGISTERED.store(false, Ordering::Release);
        REGISTERED_HOST.store(0, Ordering::Release);
        PROCESS_ID.store(0, Ordering::Release);
        PUMP_THREAD.store(0, Ordering::Release);
        PUMP_STARTED.store(false, Ordering::Release);
        PUMP_RETIRE.store(0, Ordering::Release);
        WAKE_EVENT.store(0, Ordering::Release);
        CONTINUE_EVENT.store(0, Ordering::Release);
        table::reset_after_fork();
    }
    ensure_registered();
    start_pump();
    if !REGISTERED.load(Ordering::Acquire) {
        return false;
    }
    let restored = table::lookup(current_pid());
    if std::env::var_os("KINAKAZE_FORK_TRACE").is_some() {
        eprintln!(
            "kinakaze job: restore identity expected pgid={pgid} sid={sid} ppid={ppid}, actual={restored:?}"
        );
    }
    // The registry is authoritative once the parent has published the child
    // (or rebound its identity for exec). In particular a short-lived shim
    // starter can exit after exec acknowledges the handoff but before this
    // restore completes. Reparenting then legitimately changes ppid; comparing
    // it with the serialized snapshot used to reject a healthy exec child.
    // A parent may likewise setpgid between a fork snapshot and registration.
    if let Some(entry) = restored
        && (entry.pgid != pgid || entry.sid != sid || entry.ppid != ppid)
        && let Some(directory) = std::env::var_os("KINAKAZE_NAMESPACE_TRACE_DIR")
    {
        use std::io::Write;
        let path = std::path::PathBuf::from(directory)
            .join(format!("job-restore-{}.log", std::process::id()));
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(
                file,
                "snapshot pgid={pgid} sid={sid} ppid={ppid}; live={entry:?}"
            );
        }
    }
    restored.is_some_and(|entry| {
        entry.pid == current_host_pid() && entry.namespace_pid == current_pid()
    })
}

/// Resolve an F_SETOWN target once, in the setter's PID namespace. Store the
/// internal identity: a peer writing in another namespace must not reinterpret
/// the owner's visible PID in its own namespace.
pub(crate) fn resolve_io_owner(owner: i32) -> Result<i32, i32> {
    ensure_registered();
    if owner == 0 {
        return Ok(0);
    }
    let visible = owner.checked_abs().ok_or(EINVAL)? as u32;
    let pid = table::namespaces::resolve(visible).ok_or(ESRCH)?;
    if owner < 0 {
        if table::members(pid).is_empty() {
            return Err(ESRCH);
        }
        Ok(-(pid as i32))
    } else if table::lookup(pid).is_some() {
        Ok(pid as i32)
    } else {
        Err(ESRCH)
    }
}

pub(crate) fn visible_io_owner(owner: i32) -> i32 {
    if owner == 0 {
        return 0;
    }
    let visible = table::namespaces::visible(owner.unsigned_abs()).unwrap_or(0) as i32;
    if owner < 0 { -visible } else { visible }
}

pub(crate) fn notify_io_owner(owner: i32) {
    // Only the managed process registry is addressed; native Windows PIDs are
    // never accepted as signal targets. Delivery runs at the receiver's ABI
    // boundary, after socket and queue locks have unwound.
    if owner > 0 {
        let _ = deliver_to(owner as u32, 29);
    } else if owner < 0 {
        let _ = signal_process_group(-owner, 29);
    }
}

/// Moves this process's Linux identity onto the Windows process replacing it.
///
/// The old loader remains alive only as a native wait wrapper. The shared row
/// keeps its namespace pid, parent, group and session, while its real Windows
/// pid is rebound to `child`, so every Linux-facing interface continues to see
/// one process with one stable pid.
pub fn adopt_exec_replacement(
    child: u32,
    comm: &str,
    executable: &str,
    arguments: &[String],
) -> bool {
    ensure_registered();
    let own = current_pid();
    let Some(previous) = table::process_info(own) else {
        return false;
    };
    let Ok(fds) = table::fd_links(own) else {
        return false;
    };
    let mut links = Vec::with_capacity(fds.len());
    for fd in fds {
        match table::fd_link(own, fd) {
            Ok(Some(link)) => links.push((fd, link)),
            Ok(None) => {}
            Err(_) => return false,
        }
    }
    let transaction = if let Some(authority) = kinakaze_runtime::authority::get() {
        let Ok(transaction) = (authority.prepare_exec)(child) else {
            return false;
        };
        if MANAGED_EXEC_TRANSACTION
            .compare_exchange(0, transaction, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            let _ = (authority.abort_exec)(transaction);
            return false;
        }
        Some(transaction)
    } else {
        None
    };
    if table::replace_host(own, child) {
        *EXEC_BACKUP.lock().unwrap_or_else(|p| p.into_inner()) = Some(ExecBackup {
            info: previous,
            links,
        });
        let cmdline = encode_cmdline(arguments);
        table::set_process_identity(own, comm, executable, &cmdline);
        return true;
    }
    if let Some(transaction) = transaction {
        MANAGED_EXEC_TRANSACTION.store(0, Ordering::Release);
        if let Some(authority) = kinakaze_runtime::authority::get() {
            let _ = (authority.abort_exec)(transaction);
        }
    }
    false
}

/// Finishes a successful exec handoff after the replacement's primary thread
/// has been resumed.
pub fn finish_exec_replacement() -> Result<(), i32> {
    let transaction = MANAGED_EXEC_TRANSACTION.load(Ordering::Acquire);
    if let Some(authority) = kinakaze_runtime::authority::get() {
        if transaction == 0 {
            return Err(EIO);
        }
        // commit_exec waits for static image validation and relocation. Failed
        // loading remains recoverable: the caller has not closed any old FDs.
        // Successful commit is irreversible, but the candidate stays gated
        // until this entire native process (including sibling threads) is dead.
        if let Err(error) = (authority.commit_exec)(transaction) {
            if error == 125 {
                // ECANCELED: manager proved candidate aborted.
                return Err(8); // ENOEXEC: calling image remains recoverable.
            }
            // A lost commit reply does not prove that ownership stayed here.
            // Never restore the old row over a possibly committed successor.
            eprintln!("kinakaze: exec ownership transfer outcome is unavailable");
            std::process::abort();
        }
        MANAGED_EXEC_TRANSACTION.store(0, Ordering::Release);
    }
    // Retire this image's signal pump before native process termination.
    let _registration = REGISTRATION.lock().unwrap_or_else(|p| p.into_inner());
    EXEC_RETIRED.store(true, Ordering::Release);
    REGISTERED.store(false, Ordering::Release);
    let retirement = PUMP_RETIRE.load(Ordering::Acquire) as HANDLE;
    if !retirement.is_null() && unsafe { SetEvent(retirement) } == 0 {
        std::process::abort();
    }
    EXEC_BACKUP.lock().unwrap_or_else(|p| p.into_inner()).take();
    Ok(())
}

/// Whether this native process has handed its guest identity to a new image.
pub fn exec_retired() -> bool {
    EXEC_RETIRED.load(Ordering::Acquire)
}

/// Restores this loader's ownership if a suspended exec replacement could not
/// be resumed, preserving the rule that a failed `execve` changes no PID state.
pub fn rollback_exec_replacement(namespace_pid: u32) {
    if let Some(backup) = EXEC_BACKUP.lock().unwrap_or_else(|p| p.into_inner()).take() {
        if !table::rollback_exec_owner(&backup.info)
            || table::replace_fd_links(namespace_pid, &backup.links).is_err()
        {
            std::process::abort();
        }
    } else {
        table::replace_host(namespace_pid, current_host_pid());
    }
    let transaction = MANAGED_EXEC_TRANSACTION.swap(0, Ordering::AcqRel);
    if transaction != 0 {
        if let Some(authority) = kinakaze_runtime::authority::get() {
            let _ = (authority.abort_exec)(transaction);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate this process's own registry slot.
    fn serialized() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Restores this process's group and session after a test changes them.
    struct RestoreIdentity {
        pgid: u32,
        sid: u32,
    }

    impl RestoreIdentity {
        fn capture() -> Self {
            ensure_registered();
            let entry = table::lookup(current_pid()).expect("this process must be registered");
            Self {
                pgid: entry.pgid,
                sid: entry.sid,
            }
        }
    }

    impl Drop for RestoreIdentity {
        fn drop(&mut self) {
            table::set_session(current_pid(), self.sid, self.pgid);
        }
    }

    #[test]
    fn a_process_registers_itself_as_its_own_group_and_session() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        let pid = current_pid() as i32;
        // A program started from a Windows shell has no hosted parent to inherit
        // from, so it leads its own group and session.
        table::set_session(pid as u32, pid as u32, pid as u32);
        assert_eq!(current_pgid(), pid);
        assert_eq!(current_sid(), pid);
        assert!(group_in_session(pid, pid));
        // A group that is not in the named session is not accepted, and neither
        // is a nonsensical id.
        assert!(!group_in_session(pid, pid + 1));
        assert!(!group_in_session(0, pid));
        assert!(!group_in_session(pid, 0));
    }

    #[test]
    fn setsid_is_refused_for_a_group_leader() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        // The process leads its own group, which is exactly the state POSIX says
        // `setsid` must refuse: the new session id would collide with it.
        table::set_session(current_pid(), current_pid(), current_pid());
        assert_eq!(setsid(), Err(EPERM));
    }

    #[test]
    fn setsid_succeeds_once_the_caller_is_not_a_group_leader() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        let pid = current_pid();
        // Pretend this process was placed in someone else's group, which is the
        // state a forked child is in when it calls `setsid`.
        table::set_group(pid, pid + 1);
        assert_eq!(setsid(), Ok(pid as i32));
        assert_eq!(current_sid(), pid as i32);
        assert_eq!(current_pgid(), pid as i32);
    }

    #[test]
    fn setpgid_refuses_a_process_that_is_not_the_caller_or_its_child() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        // A pid with no slot cannot be moved, and neither could a live one that
        // is not this process's child.
        assert_eq!(setpgid(0x7fff_0000, 0), Err(ESRCH));
        // Negative arguments are not pids or groups.
        assert_eq!(setpgid(-1, 0), Err(EINVAL));
        assert_eq!(setpgid(0, -1), Err(EINVAL));
    }

    #[test]
    fn setpgid_refuses_a_group_from_another_session() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        let pid = current_pid();
        // Leave leadership, so the session-leader rule is not what refuses this.
        table::set_session(pid, pid + 1000, pid + 1000);
        // The group exists in no session at all, so it cannot be joined.
        assert_eq!(setpgid(0, (pid + 2000) as i32), Err(EPERM));
    }

    #[test]
    fn setpgid_refuses_to_move_a_session_leader() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        let pid = current_pid();
        table::set_session(pid, pid, pid);
        // A session leader may not leave its own group; that would split the
        // session's foreground bookkeeping from the process that owns it.
        assert_eq!(setpgid(0, (pid + 5) as i32), Err(EPERM));
    }

    #[test]
    fn signalling_an_empty_group_reports_esrch() {
        let _serialized = serialized();
        ensure_registered();
        // Nothing is in this group, and saying so is more useful than returning
        // success for a signal nothing received.
        assert_eq!(signal_process_group(0x7fff_0001, SIGINT), Err(ESRCH));
        // A group id must be positive.
        assert_eq!(signal_process_group(0, SIGINT), Err(EINVAL));
        assert_eq!(signal_process_group(-1, SIGINT), Err(EINVAL));
        // As must the signal number be in range.
        assert_eq!(signal_process_group(current_pgid(), 9999), Err(EINVAL));
    }

    #[test]
    fn broadcast_never_targets_namespace_init_or_the_caller() {
        assert!(!broadcast_target(0, 100));
        assert!(!broadcast_target(1, 100));
        assert!(!broadcast_target(100, 100));
        assert!(broadcast_target(2, 100));
        assert!(broadcast_target(100, 1));
    }

    #[test]
    fn a_signal_to_this_processs_own_group_arrives_locally() {
        let _serialized = serialized();
        let _guard = signal::test_lock();
        ensure_registered();

        // With no handler installed SIGWINCH is ignored by default, which makes
        // it the one signal this test can send without risking termination.
        assert_eq!(
            signal_process_group(current_pgid(), signal::SIGWINCH),
            Ok(())
        );
        // Signal zero is the existence check and delivers nothing.
        assert_eq!(signal_process_group(current_pgid(), 0), Ok(()));
    }

    #[test]
    fn kill_selects_targets_the_way_posix_describes() {
        let _serialized = serialized();
        let _guard = signal::test_lock();
        let _restore = RestoreIdentity::capture();
        // Group 1 cannot be selected as -pgid: -1 denotes a broadcast, even
        // when the namespace's first process leads group 1. Use a distinct
        // group for this target-selection test and restore it afterwards.
        table::set_group(current_pid(), current_pid().max(2));
        // Zero means the caller's own group.
        assert_eq!(kill(0, 0), Ok(()));
        // A positive pid with no slot is not a process this layer can reach.
        assert_eq!(kill(0x7fff_0002, 0), Err(ESRCH));
        assert_eq!(kill(i32::MIN, 0), Err(ESRCH));
        // A negative pid below -1 names the group.
        assert_eq!(kill(-current_pgid(), 0), Ok(()));
        // Signal zero against this very process is the existence check.
        assert_eq!(kill(current_pid() as i32, 0), Ok(()));
        // An out-of-range signal is refused before any target is chosen.
        assert_eq!(kill(current_pid() as i32, 9999), Err(EINVAL));
        assert_eq!(kill(current_pid() as i32, -1), Err(EINVAL));
    }

    #[test]
    fn the_fork_payload_round_trips_a_group_identity() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        let payload = serialize_fork_state().expect("registered process state must serialize");
        assert_eq!(payload.len(), 16);
        assert_eq!(
            u32::from_le_bytes(payload[0..4].try_into().unwrap()),
            HANDOFF_MAGIC
        );
        // The parent records itself as the child's parent.
        assert_eq!(
            u32::from_le_bytes(payload[12..16].try_into().unwrap()),
            current_pid()
        );
        // A payload that is too short or misidentified is refused rather than
        // adopted as a garbage identity.
        assert!(!restore_fork_state(&payload[..8]));
        assert!(!restore_fork_state(&[0u8; 16]));
    }

    #[test]
    fn exec_restore_keeps_an_already_registered_pump_alive() {
        let _serialized = serialized();
        let _restore = RestoreIdentity::capture();
        let pid = current_pid();
        let wake = WAKE_EVENT.load(Ordering::Acquire);
        let retirement = PUMP_RETIRE.load(Ordering::Acquire);
        let payload = serialize_exec_state().unwrap();
        // Descriptor restoration can register this provider before the job
        // section of an exec handoff arrives. Repeated restoration must never
        // clear events under the live pump or start a second consumer.
        for _ in 0..8 {
            assert!(restore_fork_state(&payload));
            assert_eq!(current_pid(), pid);
            assert_eq!(WAKE_EVENT.load(Ordering::Acquire), wake);
            assert_eq!(PUMP_RETIRE.load(Ordering::Acquire), retirement);
            assert_ne!(wake, 0);
            assert_ne!(retirement, 0);
        }
    }

    #[test]
    fn the_foreground_group_survives_a_round_trip() {
        let _serialized = serialized();
        let previous = foreground_pgid();
        set_foreground_pgid(current_pgid());
        assert_eq!(foreground_pgid(), current_pgid());
        // Zero and negatives are not groups and are ignored rather than stored.
        set_foreground_pgid(0);
        set_foreground_pgid(-3);
        assert_eq!(foreground_pgid(), current_pgid());
        if previous > 0 {
            set_foreground_pgid(previous);
        }
    }

    #[test]
    fn a_registered_process_can_be_found_by_pid() {
        let _serialized = serialized();
        ensure_registered();
        let pid = current_pid() as i32;
        assert_eq!(getpgid(0), getpgid(pid));
        assert_eq!(getsid(0), getsid(pid));
        // A pid nothing has registered is not a process this layer knows.
        assert_eq!(getpgid(0x7fff_0003), Err(ESRCH));
        assert_eq!(getsid(0x7fff_0003), Err(ESRCH));
        assert_eq!(getpgid(-1), Err(ESRCH));
    }

    // -----------------------------------------------------------------------
    // End-to-end delivery between two real Windows processes.
    //
    // Everything above tests one process's view of the registry. The claim this
    // module actually makes is larger — that a signal sent by one process runs a
    // handler in another — and the only honest way to test it is with a second
    // process. These re-run this test binary with a filter that selects the
    // helper below, which is inert unless the environment names a mode.
    // -----------------------------------------------------------------------

    /// Names the group the helper joins, and the behaviour it performs.
    const HELPER_MODE: &str = "KINAKAZE_JOB_HELPER_MODE";
    const HELPER_GROUP: &str = "KINAKAZE_JOB_HELPER_GROUP";

    /// Exit code the helper uses when it runs a handler for `SIGUSR1`.
    const HELPER_HANDLED: i32 = 50;
    /// Exit code the helper uses when it waited and nothing arrived.
    const HELPER_TIMED_OUT: i32 = 90;

    /// Runs from the handler, so the exit code proves the handler ran.
    unsafe extern "sysv64" fn helper_exit(signal_number: i32) {
        std::process::exit(HELPER_HANDLED + signal_number - signal::SIGUSR1);
    }

    #[test]
    fn cross_process_signal_helper() {
        use std::io::Write;

        // Inert during an ordinary test run: without a mode this is not the
        // helper, it is just a test that has nothing to do.
        let Ok(mode) = std::env::var(HELPER_MODE) else {
            return;
        };
        let group: u32 = std::env::var(HELPER_GROUP)
            .expect("the helper needs a group")
            .parse()
            .expect("a numeric group");

        ensure_registered();
        // Join the group the parent named, the way a shell would have placed
        // this process before it started.
        table::set_group(current_pid(), group);
        signal::sigaction(
            signal::SIGUSR1,
            Some(signal::Action {
                disposition: signal::Disposition::Handle(helper_exit, 0),
                flags: 0,
                mask: 0,
                restorer: 0,
            }),
        )
        .expect("installing the helper's handler");

        // A file this thread extends on every pass. Its length is how the
        // parent tells a process that has really been suspended from one that
        // has merely been marked stopped: a suspended thread cannot write.
        let mut progress = (mode == "stop")
            .then(|| std::fs::File::create(helper_progress(group)).expect("the progress file"));

        // Poll rather than block: this is the same delivery point a guest
        // reaches when a blocking call returns EINTR.
        for _ in 0..4000 {
            signal::deliver_pending();
            if let Some(file) = progress.as_mut() {
                let _ = file.write_all(b".");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::process::exit(HELPER_TIMED_OUT);
    }

    /// Where the helper records that it is still running.
    fn helper_progress(group: u32) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kinakaze-job-progress-{group}"))
    }

    /// How far the helper has got, or zero before it has started.
    fn progress_len(group: u32) -> u64 {
        std::fs::metadata(helper_progress(group)).map_or(0, |data| data.len())
    }

    /// Starts this test binary again, running only the helper.
    fn spawn_helper(mode: &str, group: u32) -> std::process::Child {
        let exe = std::env::current_exe().expect("the test binary's own path");
        let log = std::fs::File::create(helper_log(group)).expect("the helper's log");
        std::process::Command::new(exe)
            .args([
                "job::tests::cross_process_signal_helper",
                "--exact",
                "--test-threads=1",
            ])
            .env(HELPER_MODE, mode)
            .env(HELPER_GROUP, group.to_string())
            .stdout(std::process::Stdio::from(
                log.try_clone().expect("a second handle"),
            ))
            .stderr(std::process::Stdio::from(log))
            .spawn()
            .expect("spawning the helper")
    }

    /// Where a helper's own test output lands, so a failure here reports why the
    /// second process gave up rather than only that it did.
    fn helper_log(group: u32) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kinakaze-job-helper-{group}.log"))
    }

    /// Waits for `pid` to appear in `group`, so the test does not race startup.
    fn await_membership(group: u32, helper: &mut std::process::Child) {
        ensure_registered();
        let host_pid = helper.id();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if let Some(entry) = table::lookup_host(host_pid)
                && table::members(group)
                    .iter()
                    .any(|member| member.namespace_pid == entry.namespace_pid)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!(
            "the helper never joined group {group}; host_pid={host_pid} exit={:?} slot={:?} log={:?}",
            helper.try_wait(),
            table::lookup_host(host_pid),
            std::fs::read_to_string(helper_log(group)).unwrap_or_default()
        );
    }

    /// Removes the files a helper left behind.
    fn clean_helper_files(group: u32) {
        let _ = std::fs::remove_file(helper_log(group));
        let _ = std::fs::remove_file(helper_progress(group));
    }

    /// A group id no other test or hosted process will be using.
    fn private_group(salt: u32) -> u32 {
        current_pid().wrapping_add(0x0020_0000).wrapping_add(salt)
    }

    #[test]
    fn a_group_signal_runs_a_handler_in_another_process() {
        let group = private_group(1);
        let mut helper = spawn_helper("signal", group);
        await_membership(group, &mut helper);

        // The whole point of the module: this process names a group it is not
        // in, and a handler runs in a different Windows process.
        signal_process_group(group as i32, signal::SIGUSR1).expect("signalling the group");

        let status = helper.wait().expect("waiting for the helper");
        assert_eq!(
            status.code(),
            Some(HELPER_HANDLED),
            "the helper did not run its SIGUSR1 handler"
        );
        clean_helper_files(group);
    }

    #[test]
    fn a_stop_signal_stops_another_process_and_sigcont_resumes_it() {
        let group = private_group(2);
        let mut helper = spawn_helper("stop", group);
        await_membership(group, &mut helper);
        let pid = table::lookup_host(helper.id())
            .expect("registered helper")
            .namespace_pid;

        // The helper must be making progress before it is stopped, or the
        // stillness measured below would prove nothing.
        let running = progress_len(group);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            progress_len(group) > running,
            "the helper was not running before it was stopped"
        );

        signal_process_group(group as i32, SIGSTOP).expect("stopping the group");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while table::state(pid) != STATE_STOPPED {
            assert!(
                std::time::Instant::now() < deadline,
                "the helper never stopped"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // The stop is also reported to the parent, which is what `waitpid` with
        // WUNTRACED reads.
        assert_eq!(
            table::peek_report(pid),
            Some(StateChange::Stopped(SIGSTOP as u32)),
            "a stopped child must leave a report for its parent; helper log: {}",
            std::fs::read_to_string(helper_log(group)).unwrap_or_default()
        );

        // The state word is set just before the threads are suspended, so give
        // the suspension a moment before measuring, then check that the helper
        // really has stopped moving rather than only been marked.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let stopped_at = progress_len(group);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(
            progress_len(group),
            stopped_at,
            "a stopped process must actually stop, not merely be flagged"
        );

        signal_process_group(group as i32, SIGCONT).expect("continuing the group");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while table::state(pid) != STATE_RUNNING {
            assert!(
                std::time::Instant::now() < deadline,
                "the helper never resumed"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            progress_len(group) > stopped_at,
            "SIGCONT must let the suspended threads run again"
        );

        // A resumed process is a working one: it still runs handlers.
        signal_process_group(group as i32, signal::SIGUSR1).expect("signalling the group");
        let status = helper.wait().expect("waiting for the helper");
        assert_eq!(
            status.code(),
            Some(HELPER_HANDLED),
            "the resumed helper did not run its SIGUSR1 handler"
        );
        clean_helper_files(group);
    }
}
