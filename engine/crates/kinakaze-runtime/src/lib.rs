//! Process runtime for the kinakaze Linux-compatibility layer.
//!
//! On Windows, `fork` is implemented by starting the same image, letting its
//! loader and CRT reach [`run`], suspending that primary thread, and replacing
//! its managed arena, native Windows stack and CPU context with the parent's.

use kinakaze_abi::Pid;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

// Allocation-free failure location capture; formatting happens after fork has
// thawed parent threads, so diagnostics cannot take a frozen allocator lock.
static LAST_NATIVE_FAILURE_LINE: AtomicU32 = AtomicU32::new(0);

#[cfg(windows)]
mod anonymous_cow;
#[cfg(all(windows, target_arch = "x86_64"))]
mod arena_copy;
pub mod authority;
#[cfg(windows)]
mod cow;
pub mod execution;
#[cfg(all(windows, target_arch = "x86_64"))]
mod guest_heap;
mod handle_slots;
pub mod immutable;
#[cfg(windows)]
pub mod memory_protection;
pub mod services;
pub use handle_slots::{register_fork_handle_slot, unregister_fork_handle_slot};

#[inline]
fn fork_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FORK_TRACE").is_some())
}

#[inline]
fn fork_mapping_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    fork_trace_enabled()
        || *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FORK_MAPPING_TRACE").is_some())
}

#[inline]
fn fork_timings_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_FORK_TIMINGS").is_some())
}

/// Child stdout/stderr can be a binary bootstrap protocol (containerd shims).
/// Optional profiling must never inject text into those inherited streams.
#[cfg(windows)]
fn fork_timing_line(arguments: core::fmt::Arguments<'_>) {
    use std::io::Write;
    let directory = std::env::var_os("KINAKAZE_FORK_TIMINGS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = directory.join(format!("kinakaze-fork-{}.log", std::process::id()));
    if let Ok(mut output) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(output, "{arguments}");
    }
}

/// Optional failure context from fork participants, kept out of inherited
/// stdout/stderr because those streams may carry a binary bootstrap protocol.
#[cfg(windows)]
pub fn fork_diagnostic(arguments: core::fmt::Arguments<'_>) {
    if fork_timings_enabled() || fork_trace_enabled() {
        fork_timing_line(arguments);
    }
}

#[inline]
fn vfork_trace_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("KINAKAZE_VFORK_TRACE").is_some())
}

#[inline]
fn wait_trace_enabled() -> bool {
    trace_target_enabled("KINAKAZE_WAIT_TRACE")
}

/// Enables a diagnostic either globally (`1`) or only while the current guest
/// argv contains the configured target. This is deliberately evaluated on
/// every call: a fork child changes argv at exec, so caching `true` in the
/// parent would leak diagnostics into the executed program's stderr.
fn trace_target_enabled(name: &str) -> bool {
    static OWNER_PID: OnceLock<u32> = OnceLock::new();
    let Some(target) = std::env::var_os(name) else {
        return false;
    };
    let target = target.to_string_lossy();
    let matches = target == "1"
        || std::env::args_os().any(|argument| argument.to_string_lossy().contains(target.as_ref()));
    matches && *OWNER_PID.get_or_init(std::process::id) == std::process::id()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForkStage {
    Allocator,
    CreateEvent,
    ExecutablePath,
    CreateProcess,
    ChildBootstrap,
    SuspendChild,
    ImageLayout,
    ArenaMapping,
    ArenaCopy,
    ParentStack,
    ChildStack,
    StackCopy,
    ChildTeb,
    ThreadContext,
    ResumeChild,
    HandoffPrepare,
    HandoffStage,
    HandoffRestore,
    ModuleManifest,
    ThreadFreeze,
    GuestMapping,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForkError {
    pub stage: ForkStage,
    pub os_code: u32,
}

/// Converts an internal clone failure to the errno returned by Linux process
/// creation calls. Host exports use the kernel-style `-errno` convention;
/// libc is the sole layer that turns it into `-1` and updates thread-local
/// `errno`.
fn fork_errno(error: ForkError) -> i32 {
    #[cfg(windows)]
    if fork_timings_enabled() || std::env::var_os("KINAKAZE_FORK_TIMINGS_DIR").is_some() {
        fork_timing_line(format_args!(
            "kinakaze: fork failure stage={:?} os_code={} native_line={}",
            error.stage,
            error.os_code,
            LAST_NATIVE_FAILURE_LINE.load(Ordering::Acquire)
        ));
    }
    match error.stage {
        ForkStage::Allocator
        | ForkStage::ArenaMapping
        | ForkStage::ArenaCopy
        | ForkStage::GuestMapping => 12, // ENOMEM
        // These Win32 resource-exhaustion codes can also surface while the
        // coordinator creates synchronization and process objects.
        _ if matches!(error.os_code, 8 | 14 | 1455) => 12, // ENOMEM
        ForkStage::Unsupported => {
            // Native Linux builds preserve a raw kernel errno here. A zero
            // code means the operation itself is unavailable.
            i32::try_from(error.os_code)
                .ok()
                .filter(|errno| (1..=4095).contains(errno))
                .unwrap_or(38) // ENOSYS
        }
        ForkStage::HandoffPrepare if error.os_code == 12 => 12, // dead PID namespace
        ForkStage::HandoffPrepare if error.os_code == 35 => 35, // EDEADLK
        _ => 11,                                                // EAGAIN
    }
}

/// Version of the cross-DLL fork participant ABI.
pub const FORK_PARTICIPANT_ABI: u32 = 1;

/// A stable identity used by one runtime subsystem across the parent and child.
pub type ForkParticipantKey = u64;

/// Called before the address-space snapshot.  A non-zero result aborts the fork.
pub type ForkPrepare = unsafe extern "system" fn() -> i32;
/// Writes a participant payload.  A null buffer queries the required length.
/// A negative result aborts the fork; otherwise the result is the byte count.
pub type ForkSnapshot = unsafe extern "system" fn(*mut u8, usize) -> isize;
/// Called in the parent after success or rollback.  Negative means failure.
pub type ForkParent = unsafe extern "system" fn(i32);
/// Rebuilds process-local state in the child from the staged payload.
pub type ForkChild = unsafe extern "system" fn(*const u8, usize) -> i32;

/// Callbacks owned by a DLL or runtime subsystem that has process-local state.
///
/// This is deliberately a C-shaped record.  Rust values, allocators and locks may
/// not cross a DLL boundary, while plain function pointers and integers can.  The
/// record itself is copied into the process coordinator when it is registered.
#[derive(Clone, Copy)]
#[repr(C)]
pub struct ForkParticipant {
    pub abi: u32,
    pub priority: i32,
    pub key: ForkParticipantKey,
    pub prepare: Option<ForkPrepare>,
    pub snapshot: Option<ForkSnapshot>,
    pub parent: Option<ForkParent>,
    pub child: Option<ForkChild>,
}

unsafe impl Send for ForkParticipant {}
unsafe impl Sync for ForkParticipant {}

const MAX_MODULES: usize = 32;
const MAX_CHILDREN: usize = 64;

/// Number of guest-visible threads in this hosted process.
///
/// The executable owns the authoritative counter. Provider DLLs statically
/// link this crate too, so the public helpers below forward through exported
/// process entry points instead of updating their private copies. The initial
/// guest thread exists before any provider is loaded and therefore contributes
/// the baseline one.
static PROCESS_THREAD_COUNT: AtomicU32 = AtomicU32::new(1);

/// Returns the explicit Linux parent requested for the active fork transaction.
///
/// The command lives in the shared process registry rather than a Rust static:
/// the executable and every provider DLL contain their own copy of this crate,
/// but they must all observe one transaction value.
pub fn fork_parent_override() -> Result<Option<u32>, ()> {
    job::fork_parent_override()
}

struct ForkParentOverrideGuard {
    owner_host_pid: u32,
}

impl ForkParentOverrideGuard {
    fn install(parent: Option<u32>) -> Result<Self, ()> {
        let owner_host_pid = std::process::id();
        job::set_fork_parent_override(owner_host_pid, parent)?;
        Ok(Self { owner_host_pid })
    }
}

impl Drop for ForkParentOverrideGuard {
    fn drop(&mut self) {
        // The guard is copied with the native stack. Only the process that
        // created the transaction owns its command; the fork child must not
        // clear state in the parent's registry row.
        if std::process::id() != self.owner_host_pid {
            return;
        }
        if job::set_fork_parent_override(self.owner_host_pid, None).is_err() {
            eprintln!("kinakaze: shared fork command could not be cleared");
            std::process::abort();
        }
    }
}

#[cfg(windows)]
type ProcessU32Export = unsafe extern "system" fn() -> u32;

#[cfg(windows)]
type ProcessUsizeExport = unsafe extern "system" fn() -> usize;

#[cfg(windows)]
type ProcessParticipantOwnerExport = unsafe extern "system" fn(u64, usize) -> i32;

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ProcessThreadExports {
    started: Option<ProcessU32Export>,
    finished: Option<ProcessU32Export>,
    count: Option<ProcessU32Export>,
    child_activity_event: Option<ProcessUsizeExport>,
    participant_owner: Option<ProcessParticipantOwnerExport>,
    register_handle_slot: Option<handle_slots::RegistrationExport>,
    unregister_handle_slot: Option<handle_slots::RegistrationExport>,
}

#[cfg(windows)]
struct ProcessThreadExportsCell(std::cell::UnsafeCell<ProcessThreadExports>);

// SAFETY: the CRT initializer is the only writer and runs before application
// threads exist. Fork children load every registered PE provider normally, so
// they run the initializer again instead of inheriting this DLL's writable data.
#[cfg(windows)]
unsafe impl Sync for ProcessThreadExportsCell {}

#[cfg(windows)]
static PROCESS_THREAD_EXPORTS: ProcessThreadExportsCell =
    ProcessThreadExportsCell(std::cell::UnsafeCell::new(ProcessThreadExports {
        started: None,
        finished: None,
        count: None,
        child_activity_event: None,
        participant_owner: None,
        register_handle_slot: None,
        unregister_handle_slot: None,
    }));

#[cfg(windows)]
extern "C" fn process_thread_exports_initializer() {
    let exports = windows::resolve_process_u32_exports(
        kinakaze_runtime_process_thread_started as *const () as usize,
    );
    // SAFETY: this CRT initializer runs once, before the module is visible to
    // application threads. The child loads this module afresh at its registered
    // image base and therefore performs the same initialization independently.
    unsafe { PROCESS_THREAD_EXPORTS.0.get().write(exports) };
}

#[cfg(windows)]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static PROCESS_THREAD_EXPORTS_INITIALIZER: extern "C" fn() = process_thread_exports_initializer;

#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_process_thread_started() -> u32 {
    let previous = PROCESS_THREAD_COUNT
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            Some(count.saturating_add(1))
        })
        .unwrap_or_else(|count| count);
    previous.saturating_add(1)
}

#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_process_thread_finished() -> u32 {
    let previous = PROCESS_THREAD_COUNT
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            Some(count.saturating_sub(1).max(1))
        })
        .unwrap_or_else(|count| count);
    previous.saturating_sub(1).max(1)
}

#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_process_thread_count() -> u32 {
    PROCESS_THREAD_COUNT.load(Ordering::Acquire).max(1)
}

/// Records a guest pthread after its native Windows thread has been reserved.
pub fn process_thread_started() -> u32 {
    #[cfg(windows)]
    {
        // SAFETY: initialized by the module CRT before application threads run.
        if let Some(function) = unsafe { (*PROCESS_THREAD_EXPORTS.0.get()).started } {
            // SAFETY: the initializer validates the process export's fixed ABI.
            return unsafe { function() };
        }
    }
    kinakaze_runtime_process_thread_started()
}

/// Retires a guest pthread immediately before its native thread exits.
pub fn process_thread_finished() -> u32 {
    #[cfg(windows)]
    {
        // SAFETY: initialized by the module CRT before application threads run.
        if let Some(function) = unsafe { (*PROCESS_THREAD_EXPORTS.0.get()).finished } {
            // SAFETY: the initializer validates the process export's fixed ABI.
            return unsafe { function() };
        }
    }
    kinakaze_runtime_process_thread_finished()
}

/// Retire one live guest task, including the initial task. The caller must
/// terminate the process when this returns true. Unlike a failed thread-create
/// reservation rollback, retirement must identify the last task atomically.
/// Pthreads and raw clone/SYS_exit share this counter and decision.
pub fn retire_guest_thread() -> bool {
    // Guest TLS destructors have run (or raw SYS_exit bypassed them). Publish
    // reusable allocation batches before this thread becomes joinable; otherwise
    // short-lived workers leave up to a MiB in each abandoned allocator shard.
    kinakaze_alloc::release_thread_cache();
    let previous = PROCESS_THREAD_COUNT
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            Some(count.saturating_sub(1).max(1))
        })
        .unwrap_or_else(|count| count);
    previous <= 1
}

/// Returns the process-wide guest thread count without taking a system snapshot.
pub fn process_thread_count() -> u32 {
    #[cfg(windows)]
    {
        // SAFETY: initialized by the module CRT before application threads run.
        if let Some(function) = unsafe { (*PROCESS_THREAD_EXPORTS.0.get()).count } {
            // SAFETY: the initializer validates the process export's fixed ABI.
            return unsafe { function() };
        }
    }
    kinakaze_runtime_process_thread_count()
}

#[cfg(test)]
mod process_thread_count_tests {
    use super::*;

    #[test]
    fn thread_count_balances_and_never_loses_the_initial_thread() {
        static SERIALIZED: Mutex<()> = Mutex::new(());
        let _guard = SERIALIZED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let before = process_thread_count();
        assert!(before >= 1);
        assert_eq!(process_thread_started(), before + 1);
        assert_eq!(process_thread_count(), before + 1);
        assert_eq!(process_thread_finished(), before);
        assert_eq!(process_thread_count(), before);

        if before == 1 {
            assert_eq!(process_thread_finished(), 1);
            assert_eq!(process_thread_count(), 1);
            process_thread_started();
            process_thread_started();
            // The initial task may exit before either of its children.
            assert!(!retire_guest_thread());
            assert!(!retire_guest_thread());
            assert!(retire_guest_thread());
            assert_eq!(process_thread_count(), 1);
        }
    }
}

/// Direct TEB TLS indices owned by the executable and required by copied guest
/// code. A fork child reserves these before loading any provider DLL so Windows
/// assigns every module the same process TLS indices as in the parent.
#[cfg(windows)]
static FORK_TLS_SLOT_MASK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Records direct TEB TLS slots whose numeric identity must survive `fork`.
///
/// Windows rebuilds its process-wide TLS allocation bitmap for a new process.
/// Guest code, however, contains already-patched direct TEB offsets, so the
/// child bootstrap must reserve these slots before it loads provider DLLs.
#[cfg(windows)]
pub fn register_fork_tls_slots(slots: &[u32]) -> bool {
    let mut mask = 0u64;
    for &slot in slots {
        if slot >= 64 {
            return false;
        }
        mask |= 1u64 << slot;
    }
    FORK_TLS_SLOT_MASK.fetch_or(mask, std::sync::atomic::Ordering::AcqRel);
    true
}

/// Reports whether the current process bootstrap reserved all requested slots.
#[cfg(windows)]
pub fn fork_tls_slots_registered(slots: &[u32]) -> bool {
    let current = FORK_TLS_SLOT_MASK.load(std::sync::atomic::Ordering::Acquire);
    slots
        .iter()
        .all(|&slot| slot < 64 && current & (1u64 << slot) != 0)
}

#[derive(Clone, Copy)]
struct RegisteredParticipant {
    hooks: ForkParticipant,
    order: u64,
    // Process-local, deliberately omitted from the fork wire records.
    owner_changed: usize,
}

#[derive(Clone)]
struct ParticipantRegistry {
    next_order: u64,
    entries: Vec<RegisteredParticipant>,
}

impl Default for ParticipantRegistry {
    fn default() -> Self {
        Self {
            next_order: 0,
            entries: Vec::new(),
        }
    }
}

impl ParticipantRegistry {
    fn clear(&mut self) {
        self.next_order = 0;
        self.entries.clear();
    }

    fn collect_entries(&self) -> Vec<RegisteredParticipant> {
        self.entries.clone()
    }

    fn insert(&mut self, hooks: ForkParticipant) -> bool {
        if let Some(existing) = self
            .entries
            .iter_mut()
            .find(|entry| entry.hooks.key == hooks.key)
        {
            if hooks.priority <= existing.hooks.priority {
                #[cfg(windows)]
                if existing.hooks.snapshot.map(|f| f as usize) != hooks.snapshot.map(|f| f as usize)
                    && existing.owner_changed != 0
                {
                    unsafe {
                        windows_sys::Win32::System::Threading::SetEvent(
                            existing.owner_changed as _,
                        );
                        windows_sys::Win32::Foundation::CloseHandle(existing.owner_changed as _);
                    }
                    existing.owner_changed = 0;
                }
                existing.hooks = hooks;
            }
            return true;
        }
        if self.entries.try_reserve(1).is_err() {
            return false;
        }
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.entries.push(RegisteredParticipant {
            hooks,
            order,
            owner_changed: 0,
        });
        true
    }
}

fn participants() -> &'static Mutex<ParticipantRegistry> {
    static REGISTRY: OnceLock<Mutex<ParticipantRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(ParticipantRegistry::default()))
}

fn register_participant_local(hooks: ForkParticipant) -> bool {
    register_participant_local_impl(hooks, fork_trace_enabled())
}

fn register_participant_local_impl(hooks: ForkParticipant, trace: bool) -> bool {
    if hooks.abi != FORK_PARTICIPANT_ABI || hooks.key == 0 {
        return false;
    }
    if trace {
        eprintln!(
            "kinakaze runtime: register_participant_local in pid {}: key=0x{:x}, priority={}, prepare={:x?}, snapshot={:x?}, parent={:x?}, child={:x?}",
            std::process::id(),
            hooks.key,
            hooks.priority,
            hooks.prepare.map(|p| p as usize),
            hooks.snapshot.map(|p| p as usize),
            hooks.parent.map(|p| p as usize),
            hooks.child.map(|p| p as usize),
        );
    }
    let Ok(mut registry) = participants().lock() else {
        return false;
    };
    registry.insert(hooks)
}

/// Process-exported registration entry used by substitute DLLs.
///
/// The executable owns the coordinator.  A statically linked runtime copy inside
/// a DLL discovers this export and forwards to it, preventing one isolated hook
/// list per PE module.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn kinakaze_runtime_register_fork_participant(
    hooks: *const ForkParticipant,
) -> i32 {
    if hooks.is_null() {
        return 0;
    }
    if fork_trace_enabled() {
        eprintln!(
            "kinakaze runtime: register_fork_participant export called for key={:#x} in pid {}",
            unsafe { (*hooks).key },
            std::process::id()
        );
    }
    #[cfg(windows)]
    if windows::is_bootstrapping() {
        return 1;
    }
    i32::from(register_participant_local(unsafe { *hooks }))
}

/// Registers or replaces one fork participant.
pub fn register_fork_participant(hooks: ForkParticipant) -> bool {
    // V2 links this rlib exactly once, into kinakaze_runtime.dll. Provider
    // facades contain no runtime copy, so local dispatch is the owner bypass.
    register_participant_local(hooks)
}

/// Whether this callback belongs to the selected owner of a process resource.
/// Modules that statically link the same subsystem must not independently
/// consume its shared notifications. Ownership is the same choice the fork
/// coordinator makes, not a second registry with its own selection rules.
pub fn fork_participant_is_owner(key: ForkParticipantKey, snapshot: ForkSnapshot) -> bool {
    #[cfg(windows)]
    if let Some(function) = unsafe { (*PROCESS_THREAD_EXPORTS.0.get()).participant_owner } {
        return unsafe { function(key, snapshot as usize) != 0 };
    }
    kinakaze_runtime_fork_participant_is_owner(key, snapshot as usize) != 0
}

#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_fork_participant_is_owner(
    key: u64,
    snapshot: usize,
) -> i32 {
    let registry = participants()
        .lock()
        .unwrap_or_else(|_| std::process::abort());
    i32::from(registry.entries.iter().any(|entry| {
        entry.hooks.key == key
            && entry
                .hooks
                .snapshot
                .is_some_and(|callback| callback as usize == snapshot)
    }))
}

#[cfg(test)]
mod participant_ownership_tests {
    use super::*;

    unsafe extern "system" fn first(_: *mut u8, _: usize) -> isize {
        0
    }
    unsafe extern "system" fn second(_: *mut u8, _: usize) -> isize {
        1
    }

    #[test]
    fn participant_storage_grows_and_replacements_preserve_order() {
        let mut registry = ParticipantRegistry::default();
        for key in 1..=96 {
            assert!(registry.insert(ForkParticipant {
                abi: FORK_PARTICIPANT_ABI,
                priority: 500,
                key,
                prepare: None,
                snapshot: None,
                parent: None,
                child: None
            }));
        }
        assert_eq!(registry.entries.len(), 96);
        let mut replacement = registry.entries[40].hooks;
        replacement.priority = 100;
        assert!(registry.insert(replacement));
        assert_eq!(registry.entries.len(), 96);
        assert_eq!(registry.entries[40].order, 40);
        assert_eq!(registry.entries[40].hooks.priority, 100);
        registry.clear();
        assert!(registry.entries.is_empty());
        assert_eq!(registry.next_order, 0);
    }

    #[test]
    fn notification_owner_is_the_selected_fork_participant() {
        const KEY: u64 = 0x5445_5354_4f57_4e31;
        let mut hooks = ForkParticipant {
            abi: FORK_PARTICIPANT_ABI,
            priority: 100,
            key: KEY,
            prepare: None,
            snapshot: Some(first),
            parent: None,
            child: None,
        };
        assert!(!fork_participant_is_owner(KEY, first));
        assert!(register_fork_participant(hooks));
        assert!(fork_participant_is_owner(KEY, first));
        #[cfg(windows)]
        let retirement = job::notifications::event("owner-test", std::process::id()).unwrap();
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            assert!(watch_fork_owner(
                KEY,
                first,
                retirement.as_raw_handle() as usize
            ));
        }
        hooks.priority = 200;
        hooks.snapshot = Some(second);
        assert!(register_fork_participant(hooks));
        assert!(fork_participant_is_owner(KEY, first));
        assert!(!fork_participant_is_owner(KEY, second));
        hooks.priority = 50;
        assert!(register_fork_participant(hooks));
        assert!(!fork_participant_is_owner(KEY, first));
        assert!(fork_participant_is_owner(KEY, second));
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::WaitForSingleObject;
            assert_eq!(
                unsafe { WaitForSingleObject(retirement.as_raw_handle(), 0) },
                WAIT_OBJECT_0
            );
        }
    }
}

/// How a private mapping participates in a fork.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ForkMappingBehavior {
    /// Recreate the reservation at the same address and copy committed pages.
    Copy = 1,
    /// Reserve zero-filled pages at the same address.
    Zero = 2,
    /// Do not put the mapping in the child.
    Omit = 3,
}

/// Windows representation required for a mapping in the fork child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ForkMappingStorage {
    /// A conventional `VirtualAlloc` reservation.
    Ordinary = 0,
    /// A replaceable `MEM_RESERVE_PLACEHOLDER` reservation.
    Placeholder = 1,
    /// A private pagefile section view replacing an exact placeholder.
    Section = 2,
    /// The same section object, owned by a registered retained handle slot.
    RetainedSection = 3,
    /// Retain the original section, map COW, and transfer only private pages.
    CopyOnWriteSection = 4,
    /// Freeze a private pagefile section into COW at its first fork.
    AnonymousSnapshot = 5,
}

impl ForkMappingStorage {
    fn is_cow(self) -> bool {
        matches!(self, Self::CopyOnWriteSection | Self::AnonymousSnapshot)
    }

    fn retains_section(self) -> bool {
        matches!(
            self,
            Self::RetainedSection | Self::CopyOnWriteSection | Self::AnonymousSnapshot
        )
    }
}

/// Ownership of memory separately from its Windows backing representation.
///
/// CLONE_VM may share guest mm contents, but native transition code and loader
/// bookkeeping must retain independent process state. Ordinary fork continues
/// to apply `ForkMappingBehavior` to both domains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ForkMappingDomain {
    HostPrivate = 0,
    GuestMm = 1,
}

impl ForkMappingDomain {
    fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::HostPrivate),
            1 => Some(Self::GuestMm),
            _ => None,
        }
    }
}

/// A non-PE mapping containing guest memory or process-private runtime code.
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct ForkMapping {
    pub base: usize,
    pub len: usize,
    pub behavior: ForkMappingBehavior,
    pub storage: ForkMappingStorage,
    /// Address of the section handle slot copied into the child arena.
    /// Zero for mappings without a section backing.
    pub backing_slot: usize,
    /// Byte offset of this fragment in the section identified by `backing_slot`.
    pub backing_offset: u64,
    /// Legal initial view protection for a retained section. Current page
    /// protections are restored before resuming the child. Zero otherwise.
    pub view_protection: u32,
    pub domain: ForkMappingDomain,
}

#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<ForkMapping>() == 48);

impl ForkMapping {
    fn valid_storage_contract(&self) -> bool {
        if self.base == 0 || self.len == 0 || self.base.checked_add(self.len).is_none() {
            return false;
        }
        if self.storage.retains_section() {
            self.behavior != ForkMappingBehavior::Zero
                && handle_slots::valid_slot_address(self.backing_slot)
                && self.backing_offset.checked_add(self.len as u64).is_some()
                // View access is an independent backing contract. The current
                // page protection can be NONE or a dirty COW page reported RW.
                && if self.storage.is_cow() {
                    matches!(self.view_protection, 0x08 | 0x80)
                } else {
                    matches!(self.view_protection, 0x02 | 0x04 | 0x20 | 0x40)
                }
        } else {
            self.view_protection == 0
        }
    }
}

#[derive(Clone, Default)]
struct MappingRegistry {
    entries: Vec<ForkMapping>,
    modules: Vec<usize>,
    handle_slots: Vec<usize>,
}

impl MappingRegistry {
    fn clear(&mut self) {
        self.entries.clear();
        self.modules.clear();
        self.handle_slots.clear();
    }

    fn collect_entries(&self) -> Vec<ForkMapping> {
        self.entries.clone()
    }

    fn collect_modules(&self) -> Vec<usize> {
        self.modules.clone()
    }

    fn insert_mapping(&mut self, mapping: ForkMapping) -> bool {
        if let Some(existing) = self.entries.iter_mut().find(|m| m.base == mapping.base) {
            *existing = mapping;
        } else {
            self.entries.push(mapping);
        }
        true
    }

    fn remove_mapping(&mut self, base: usize) {
        self.entries.retain(|m| m.base != base);
    }

    fn insert_module(&mut self, module: usize) -> bool {
        if !self.modules.contains(&module) {
            self.modules.push(module);
        }
        true
    }

    fn remove_module(&mut self, module: usize) {
        self.modules.retain(|&m| m != module);
    }
}

fn mappings() -> &'static Mutex<MappingRegistry> {
    static REGISTRY: OnceLock<Mutex<MappingRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(MappingRegistry::default()))
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ChildProcess {
    pid: u32,
    process: usize,
    completion_wait: usize,
}

/// Pins a private retirement event in the same coordinator that selects owners.
/// An old pump can sleep indefinitely and still retire promptly on replacement.
#[cfg(windows)]
pub fn watch_fork_owner(key: u64, snapshot: ForkSnapshot, event: usize) -> bool {
    kinakaze_runtime_watch_fork_owner(key, snapshot as usize, event) != 0
}

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_watch_fork_owner(
    key: u64,
    snapshot: usize,
    event: usize,
) -> i32 {
    use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetEvent};
    let mut registry = participants()
        .lock()
        .unwrap_or_else(|_| std::process::abort());
    let owner = registry.entries.iter_mut().find(|entry| {
        entry.hooks.key == key && entry.hooks.snapshot.is_some_and(|f| f as usize == snapshot)
    });
    let Some(owner) = owner else {
        return unsafe { SetEvent(event as _) };
    };
    let process = unsafe { GetCurrentProcess() };
    let mut retained = core::ptr::null_mut();
    if unsafe {
        DuplicateHandle(
            process,
            event as _,
            process,
            &mut retained,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return 0;
    }
    if owner.owner_changed != 0 {
        unsafe { CloseHandle(owner.owner_changed as _) };
    }
    owner.owner_changed = retained as usize;
    1
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct ChildProcessRegistry {
    entries: [Option<ChildProcess>; MAX_CHILDREN],
}

#[cfg(windows)]
impl Default for ChildProcessRegistry {
    fn default() -> Self {
        Self {
            entries: [None; MAX_CHILDREN],
        }
    }
}

#[cfg(windows)]
impl ChildProcessRegistry {
    fn clear(&mut self) {
        self.entries = [None; MAX_CHILDREN];
    }

    fn insert(&mut self, pid: u32, handle: usize) -> bool {
        if self.entries.iter().flatten().any(|child| child.pid == pid) {
            return false;
        }
        let Some(slot) = self.entries.iter_mut().find(|slot| slot.is_none()) else {
            return false;
        };
        let Some(completion_wait) = register_child_completion(handle) else {
            return false;
        };
        *slot = Some(ChildProcess {
            pid,
            process: handle,
            completion_wait,
        });
        if wait_trace_enabled() {
            eprintln!(
                "kinakaze wait: host_pid={} child={pid} registered process={handle:#x} activity={:#x}",
                std::process::id(),
                local_child_activity_event()
            );
        }
        true
    }

    fn get(&self, pid: u32) -> Option<usize> {
        self.entries
            .iter()
            .flatten()
            .find(|child| child.pid == pid)
            .map(|child| child.process)
    }

    fn remove(&mut self, pid: u32) -> Option<usize> {
        for slot in self.entries.iter_mut() {
            if slot.is_some_and(|child| child.pid == pid) {
                let child = slot.take().unwrap();
                unregister_child_completion(child.completion_wait);
                return Some(child.process);
            }
        }
        None
    }
}

#[cfg(windows)]
static CHILD_ACTIVITY_EVENT: AtomicUsize = AtomicUsize::new(0);

#[cfg(windows)]
fn local_child_activity_event() -> usize {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::CreateEventW;

    let existing = CHILD_ACTIVITY_EVENT.load(Ordering::Acquire);
    if existing != 0 {
        return existing;
    }
    // A manual-reset event coalesces simultaneous exits without losing an edge
    // between a signal-pump reset and its authoritative child scan.
    let created = unsafe { CreateEventW(core::ptr::null(), 1, 0, core::ptr::null()) } as usize;
    if created == 0 {
        return 0;
    }
    match CHILD_ACTIVITY_EVENT.compare_exchange(0, created, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => created,
        Err(winner) => {
            unsafe { CloseHandle(created as _) };
            winner
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn child_completion_callback(
    context: *mut core::ffi::c_void,
    _timed_out: bool,
) {
    use windows_sys::Win32::System::Threading::SetEvent;

    unsafe { SetEvent(context) };
    if wait_trace_enabled() {
        eprintln!(
            "kinakaze wait: host_pid={} child-complete activity={:#x}",
            std::process::id(),
            context as usize
        );
    }
}

#[cfg(windows)]
fn register_child_completion(process: usize) -> Option<usize> {
    let activity = local_child_activity_event();
    if activity == 0 {
        return None;
    }
    register_completion_notification(process, activity)
}

#[cfg(windows)]
fn register_completion_notification(process: usize, activity: usize) -> Option<usize> {
    use windows_sys::Win32::System::Threading::{RegisterWaitForSingleObject, WT_EXECUTEONLYONCE};

    if process == 0 || activity == 0 {
        return None;
    }
    let mut wait = core::ptr::null_mut();
    let registered = unsafe {
        RegisterWaitForSingleObject(
            &mut wait,
            process as _,
            Some(child_completion_callback),
            activity as *const core::ffi::c_void,
            u32::MAX,
            WT_EXECUTEONLYONCE,
        )
    };
    (registered != 0).then_some(wait as usize)
}

#[cfg(windows)]
fn unregister_child_completion(wait: usize) {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Threading::UnregisterWaitEx;

    if wait != 0 {
        // The callback touches only the process-lifetime activity event. Waiting
        // here keeps the registration lifetime exact before its slot is reused.
        unsafe { UnregisterWaitEx(wait as _, INVALID_HANDLE_VALUE) };
    }
}

#[cfg(windows)]
fn child_processes() -> &'static Mutex<ChildProcessRegistry> {
    static CHILDREN: OnceLock<Mutex<ChildProcessRegistry>> = OnceLock::new();
    CHILDREN.get_or_init(|| Mutex::new(ChildProcessRegistry::default()))
}

/// Registers a private mapping before its address is published to guest code.
pub fn register_fork_mapping(mapping: ForkMapping) -> bool {
    if !mapping.valid_storage_contract() {
        return false;
    }
    let behavior_u32 = match mapping.behavior {
        ForkMappingBehavior::Copy => 1,
        ForkMappingBehavior::Zero => 2,
        ForkMappingBehavior::Omit => 3,
    };
    kinakaze_alloc::register_shared_mapping(kinakaze_alloc::SharedForkMapping {
        base: mapping.base,
        len: mapping.len,
        behavior: behavior_u32,
        storage: mapping.storage as u32,
        backing_slot: mapping.backing_slot,
        backing_offset: mapping.backing_offset,
        view_protection: mapping.view_protection,
        domain: mapping.domain as u32,
    })
}

/// Guard that makes a host VM mutation atomic with respect to `fork`.
///
/// Linux snapshots the VMA tree and page state at one instant.  A Windows
/// backend therefore cannot map first and register later while another thread
/// is cloning the process.  Mapping implementations hold this guard across the
/// host operation and the registry update; the ordinary registration helpers
/// are re-entrant for that same OS thread.
pub struct ForkMappingTransaction {
    _shared: kinakaze_alloc::SharedMappingsTransaction,
}

/// Temporarily parks sibling native threads for an address-space replacement.
#[cfg(windows)]
pub struct MemoryThreadFreeze {
    _threads: windows::SuspendedThreads,
}

/// Preallocate scratch storage and acquire required locks before calling.
/// While the guard lives, perform only native memory operations: a suspended
/// sibling may own allocator/loader locks.
#[cfg(windows)]
pub unsafe fn freeze_memory_threads() -> Result<MemoryThreadFreeze, ForkError> {
    Ok(MemoryThreadFreeze {
        _threads: unsafe { windows::SuspendedThreads::freeze()? },
    })
}

pub fn begin_fork_mapping_transaction() -> Option<ForkMappingTransaction> {
    kinakaze_alloc::begin_shared_mappings_transaction()
        .map(|shared| ForkMappingTransaction { _shared: shared })
}

/// Removes a mapping immediately before its reservation is released.
pub fn unregister_fork_mapping(base: usize) {
    kinakaze_alloc::unregister_shared_mapping(base);
}

/// Records a loaded provider DLL that a fresh fork child must load normally.
///
/// Only its path and expected image base enter the bootstrap manifest.  PE memory
/// is never copied: the child runs the Windows loader and each DLL rebuilds its
/// own process-local state through a participant callback.
pub fn register_fork_module(module: usize) -> bool {
    if module == 0 {
        return false;
    }
    let Ok(mut registry) = mappings().lock() else {
        return false;
    };
    registry.insert_module(module)
}

/// Removes a provider module immediately before its final loader reference goes.
pub fn unregister_fork_module(module: usize) {
    if let Ok(mut registry) = mappings().lock() {
        registry.remove_module(module);
    }
}

/// Runs a hosted program after Windows process initialization is complete.
///
/// Fork children are intercepted by an earlier CRT initializer, so they never
/// reach this function until the parent has replaced their stack and context.
pub fn run(program: fn() -> i32) -> i32 {
    if let Some(register) = HANDOFF_REGISTRAR.get() {
        register();
    }
    program()
}

/// Intercepts a fresh fork child before ordinary user initializers run.
///
/// Microsoft sorts `.CRT$XC*` subsections lexically; `XCT` is the documented
/// slot before compiler/user `XCU` initializers. Provider DLLs also contain a
/// private runtime copy, so the handler itself verifies that it belongs to the
/// main executable before interpreting the command line.
#[cfg(all(windows, target_arch = "x86_64"))]
extern "C" fn fork_loader_initializer() {
    // SAFETY: this runs on the fresh process's primary thread before `main`.
    unsafe { windows::park_fork_loader_if_requested(false) };
}

/// Called by the worker after loading the runtime DLL and installing its
/// authority callbacks, before opening a new process session. DLL initializers
/// cannot perform this bootstrap while Windows still holds the loader lock.
#[cfg(all(windows, target_arch = "x86_64"))]
pub fn bootstrap_fork_child() {
    // SAFETY: the worker calls this on its fresh primary thread before guest use.
    unsafe { windows::park_fork_loader_if_requested(true) };
}

#[cfg(all(windows, target_arch = "x86_64"))]
#[used]
#[unsafe(link_section = ".CRT$XCT")]
static FORK_LOADER_INITIALIZER: extern "C" fn() = fork_loader_initializer;

/// Registers the handoff hooks. Called on every process, including children.
pub type HandoffRegistrar = fn();

static HANDOFF_REGISTRAR: std::sync::OnceLock<HandoffRegistrar> = std::sync::OnceLock::new();

/// Installs the function that registers fork handoff hooks.
///
/// A forked child never executes the parent's setup code, so this cannot be
/// called from ordinary program flow and have the child inherit it. The caller
/// must arrange for it to run during process startup — `kinakaze-vfs` does so
/// from a C runtime initializer, which every process including a fresh child
/// executes before reaching [`run`].
pub fn set_handoff_registrar(registrar: HandoffRegistrar) {
    let _ = HANDOFF_REGISTRAR.set(registrar);
}

/// Serializes state that must cross a `fork`.
pub type StageHook = fn() -> Vec<u8>;
/// Restores state serialized by a [`StageHook`].
pub type RestoreHook = fn(&[u8]);

static STAGE_HOOK: std::sync::OnceLock<StageHook> = std::sync::OnceLock::new();
static RESTORE_HOOK: std::sync::OnceLock<RestoreHook> = std::sync::OnceLock::new();

const HANDOFF_MAGIC: u64 = 0x4352_5946_4f52_4b34; // "CRYFORK4"
const LEGACY_HANDOFF_KEY: u64 = u64::MAX;
const MAPPING_HANDOFF_KEY: u64 = u64::MAX - 1;
const VFORK_HANDOFF_KEY: u64 = u64::MAX - 2;
const MAPPING_HANDOFF_VERSION: u32 = 1;

/// Registers the descriptor-table handoff hooks.
///
/// This inversion exists so the runtime does not depend on the descriptor table:
/// `kinakaze-vfs` is the lower layer and already depends on nothing, so it
/// registers its own serialization instead of being called directly.
pub fn set_fork_handoff(stage: StageHook, restore: RestoreHook) {
    let _ = STAGE_HOOK.set(stage);
    let _ = RESTORE_HOOK.set(restore);
}

fn ordered_participants() -> Result<Vec<RegisteredParticipant>, ForkError> {
    let mut entries = participants()
        .lock()
        .map_err(|_| ForkError {
            stage: ForkStage::HandoffPrepare,
            os_code: 0,
        })?
        .collect_entries();
    entries.sort_by_key(|entry| (entry.hooks.priority, entry.order));
    if fork_trace_enabled() {
        for (i, entry) in entries.iter().enumerate() {
            eprintln!(
                "kinakaze runtime: ordered_participants in pid {}: [#{}] key={:#x}, priority={}, order={}, parent={:x?}",
                std::process::id(),
                i,
                entry.hooks.key,
                entry.hooks.priority,
                entry.order,
                entry.hooks.parent.map(|p| p as usize),
            );
        }
    }
    Ok(entries)
}

fn prepare_participants(entries: &[RegisteredParticipant]) -> Result<(), ForkError> {
    let mut prepared = Vec::new();
    // POSIX requires prepare handlers in reverse registration order.
    for entry in entries.iter().rev() {
        if let Some(callback) = entry.hooks.prepare {
            // SAFETY: registration validated the versioned callback record.
            let status = unsafe { callback() };
            if status != 0 {
                #[cfg(windows)]
                fork_diagnostic(format_args!(
                    "kinakaze: fork prepare failed participant={:#x} priority={} error={status}",
                    entry.hooks.key, entry.hooks.priority,
                ));
                // Balance callbacks that already prepared, in parent order.
                for prepared_entry in prepared.iter().rev() {
                    let prepared_entry: &&RegisteredParticipant = prepared_entry;
                    if let Some(parent) = prepared_entry.hooks.parent {
                        // SAFETY: same registered callback contract.
                        unsafe { parent(-status.abs()) };
                    }
                }
                return Err(ForkError {
                    stage: ForkStage::HandoffPrepare,
                    os_code: status.unsigned_abs(),
                });
            }
        }
        prepared.push(entry);
    }
    Ok(())
}

fn finish_parent(entries: &[RegisteredParticipant], result: i32) {
    // Parent handlers use registration order, the inverse of prepare.
    for entry in entries {
        if let Some(callback) = entry.hooks.parent {
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze runtime: finish_parent in pid {}: invoking parent callback {:#x} for key {:#x} with result {}",
                    std::process::id(),
                    callback as *const () as usize,
                    entry.hooks.key,
                    result
                );
            }
            // SAFETY: registration validated the callback record.
            unsafe { callback(result) };
        }
    }
}

/// Serializes every registered participant into one length-delimited frame.
fn stage_handoff_state(entries: &[RegisteredParticipant]) -> Result<(), ForkError> {
    let mut records = Vec::<(Option<ForkParticipant>, u64, Vec<u8>)>::new();
    // A vfork rendezvous belongs to the executable coordinator rather than a
    // provider DLL. Put it first so even an unrelated child callback failure can
    // notify the suspended parent before terminating the unusable child.
    #[cfg(windows)]
    if let Some(payload) = windows::snapshot_vfork_state() {
        records.push((None, VFORK_HANDOFF_KEY, payload.to_vec()));
    }
    for entry in entries {
        let Some(snapshot) = entry.hooks.snapshot else {
            records.push((Some(entry.hooks), entry.hooks.key, Vec::new()));
            continue;
        };
        // SAFETY: null/zero is the documented length query operation.
        let required = unsafe { snapshot(core::ptr::null_mut(), 0) };
        if required < 0 {
            return Err(ForkError {
                stage: ForkStage::HandoffStage,
                os_code: required.unsigned_abs() as u32,
            });
        }
        let mut payload = vec![0u8; required as usize];
        // SAFETY: the buffer has exactly the queried writable capacity.
        let written = unsafe { snapshot(payload.as_mut_ptr(), payload.len()) };
        if written < 0 || written as usize > payload.len() {
            return Err(ForkError {
                stage: ForkStage::HandoffStage,
                os_code: written.unsigned_abs() as u32,
            });
        }
        payload.truncate(written as usize);
        records.push((Some(entry.hooks), entry.hooks.key, payload));
        #[cfg(all(windows, target_arch = "x86_64"))]
        trace_registered_tls_slots("participant-snapshot", entry.hooks.key);
    }
    // The mapping registry is coordinator state rather than PE memory.  It has
    // to be reconstructed after the child arena replaces every bootstrap-time
    // allocation, both for immediate correctness and for a nested fork.
    let (mapping_entries, mapping_modules, handle_slots) = {
        let registry = mappings().lock().map_err(|_| ForkError {
            stage: ForkStage::HandoffStage,
            os_code: 0,
        })?;
        (
            registry.collect_entries(),
            registry.collect_modules(),
            registry.handle_slots.clone(),
        )
    };
    let mapping_payload =
        encode_mapping_registry(&mapping_entries, &mapping_modules, &handle_slots);
    records.push((None, MAPPING_HANDOFF_KEY, mapping_payload));
    #[cfg(all(windows, target_arch = "x86_64"))]
    trace_registered_tls_slots("mapping-snapshot", MAPPING_HANDOFF_KEY);
    if let Some(stage) = STAGE_HOOK.get() {
        records.push((None, LEGACY_HANDOFF_KEY, stage()));
        #[cfg(all(windows, target_arch = "x86_64"))]
        trace_registered_tls_slots("legacy-snapshot", LEGACY_HANDOFF_KEY);
    }

    let mut frame = Vec::new();
    frame.extend_from_slice(&HANDOFF_MAGIC.to_le_bytes());
    frame.extend_from_slice(&(records.len() as u32).to_le_bytes());
    frame.extend_from_slice(&0u32.to_le_bytes());
    for (hooks, key, payload) in records {
        frame.extend_from_slice(&key.to_le_bytes());
        frame.extend_from_slice(
            &hooks
                .map_or(0, |value| value.priority as i64 as u64)
                .to_le_bytes(),
        );
        for address in hooks.map_or([0usize; 4], |value| {
            [
                value.prepare.map_or(0, |item| item as usize),
                value.snapshot.map_or(0, |item| item as usize),
                value.parent.map_or(0, |item| item as usize),
                value.child.map_or(0, |item| item as usize),
            ]
        }) {
            frame.extend_from_slice(&(address as u64).to_le_bytes());
        }
        frame.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        frame.extend_from_slice(&payload);
        while !frame.len().is_multiple_of(8) {
            frame.push(0);
        }
    }
    #[cfg(all(windows, target_arch = "x86_64"))]
    trace_registered_tls_slots("frame-built", 0);
    if !kinakaze_alloc::stage_handoff(&frame) {
        return Err(ForkError {
            stage: ForkStage::HandoffStage,
            os_code: 0,
        });
    }
    #[cfg(all(windows, target_arch = "x86_64"))]
    trace_registered_tls_slots("frame-staged", 0);
    Ok(())
}

#[cfg(all(windows, target_arch = "x86_64"))]
fn trace_registered_tls_slots(stage: &str, key: u64) {
    static CHECKPOINTS: OnceLock<bool> = OnceLock::new();
    if !fork_trace_enabled()
        && !*CHECKPOINTS.get_or_init(|| std::env::var_os("KINAKAZE_FORK_CHECKPOINT").is_some())
    {
        return;
    }
    let teb: usize;
    let stack_pointer: usize;
    unsafe {
        core::arch::asm!(
            "mov {teb}, gs:[0x30]",
            "mov {stack_pointer}, rsp",
            teb = out(reg) teb,
            stack_pointer = out(reg) stack_pointer,
            options(nostack, preserves_flags),
        );
    }
    let mask = FORK_TLS_SLOT_MASK.load(std::sync::atomic::Ordering::Acquire);
    for slot in 0..64u32 {
        if mask & (1u64 << slot) == 0 {
            continue;
        }
        let value = unsafe {
            ((teb + 0x1480 + slot as usize * core::mem::size_of::<usize>()) as *const usize)
                .read_unaligned()
        };
        let preceding = if (kinakaze_alloc::ARENA_BASE + core::mem::size_of::<usize>()
            ..kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE)
            .contains(&value)
        {
            unsafe { ((value - core::mem::size_of::<usize>()) as *const usize).read_unaligned() }
        } else {
            0
        };
        eprintln!(
            "kinakaze fork TLS checkpoint stage={stage} key={key:#x} rsp={stack_pointer:#x} slot={slot} value={value:#x} preceding={preceding:#x}"
        );
    }
}

/// Restores staged state in a freshly cloned child.
fn restore_handoff_state() -> Result<(), ForkError> {
    kinakaze_alloc::consume_handoff_with(restore_handoff_frame).unwrap_or_else(|| {
        Err(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })
    })
}

/// Restores one already-validated arena-backed command frame.
///
/// This function must not acquire an owned copy of `frame`: the first child
/// callbacks are the boundary at which freshly loaded DLLs adopt their parent
/// state, so allocator-backed temporary state is unsafe until those callbacks
/// have run.
fn restore_handoff_frame(frame: &[u8]) -> Result<(), ForkError> {
    if frame.len() < 16 || u64::from_le_bytes(frame[0..8].try_into().unwrap()) != HANDOFF_MAGIC {
        return Err(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        });
    }
    if let Ok(mut registry) = participants().lock() {
        registry.clear();
    }
    #[cfg(windows)]
    if let Ok(mut children) = child_processes().lock() {
        children.clear();
    }
    #[cfg(windows)]
    CHILD_ACTIVITY_EVENT.store(0, Ordering::Release);
    let count = u32::from_le_bytes(frame[8..12].try_into().unwrap()) as usize;
    let mut cursor = 16usize;
    for _ in 0..count {
        const RECORD_HEADER: usize = 56;
        let header_end = cursor.checked_add(RECORD_HEADER).ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
        if header_end > frame.len() {
            return Err(ForkError {
                stage: ForkStage::HandoffRestore,
                os_code: 0,
            });
        }
        let key = u64::from_le_bytes(frame[cursor..cursor + 8].try_into().unwrap());
        let priority =
            u64::from_le_bytes(frame[cursor + 8..cursor + 16].try_into().unwrap()) as i64 as i32;
        let mut callback = [0usize; 4];
        for (index, slot) in callback.iter_mut().enumerate() {
            let start = cursor + 16 + index * 8;
            *slot = u64::from_le_bytes(frame[start..start + 8].try_into().unwrap()) as usize;
        }
        let len = u64::from_le_bytes(frame[cursor + 48..header_end].try_into().unwrap()) as usize;
        let end = header_end.checked_add(len).ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
        let payload = frame.get(header_end..end).ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
        if key == VFORK_HANDOFF_KEY {
            #[cfg(windows)]
            if !windows::restore_vfork_state(payload) {
                return Err(ForkError {
                    stage: ForkStage::HandoffRestore,
                    os_code: 0,
                });
            }
            #[cfg(not(windows))]
            return Err(ForkError {
                stage: ForkStage::HandoffRestore,
                os_code: 0,
            });
        } else if key == LEGACY_HANDOFF_KEY {
            if let Some(restore) = RESTORE_HOOK.get() {
                restore(payload);
            }
        } else if key == MAPPING_HANDOFF_KEY {
            restore_mapping_registry(payload)?;
        } else {
            let hooks = ForkParticipant {
                abi: FORK_PARTICIPANT_ABI,
                priority,
                key,
                prepare: callback_from_address(callback[0]),
                snapshot: callback_from_address(callback[1]),
                parent: callback_from_address(callback[2]),
                child: callback_from_address(callback[3]),
            };
            // Rebuild coordinator state from plain records only after the arena
            // has been replaced.  This is what makes a fork from a fork child use
            // the same participant set without copying any DLL `.data`.
            if !register_participant_local_impl(hooks, false) {
                return Err(ForkError {
                    stage: ForkStage::HandoffRestore,
                    os_code: 0,
                });
            }
            if let Some(child) = hooks.child {
                // SAFETY: payload is live for the duration of the registered call.
                let status = unsafe { child(payload.as_ptr(), payload.len()) };
                if status != 0 {
                    #[cfg(windows)]
                    fork_diagnostic(format_args!(
                        "kinakaze: fork restore failed participant={key:#x} priority={priority} error={status}"
                    ));
                    return Err(ForkError {
                        stage: ForkStage::HandoffRestore,
                        os_code: status.unsigned_abs(),
                    });
                }
            }
        }
        cursor = end.next_multiple_of(8);
    }
    Ok(())
}

fn callback_from_address<T: Copy>(address: usize) -> Option<T> {
    if address == 0 {
        None
    } else {
        debug_assert_eq!(core::mem::size_of::<T>(), core::mem::size_of::<usize>());
        // SAFETY: addresses came from the same typed callback fields in the
        // parent and every owning module was loaded at the verified image base.
        Some(unsafe { core::mem::transmute_copy(&address) })
    }
}

fn encode_mapping_registry(
    entries: &[ForkMapping],
    modules: &[usize],
    handle_slots: &[usize],
) -> Vec<u8> {
    let mut payload =
        Vec::with_capacity(16 + entries.len() * 48 + (modules.len() + handle_slots.len()) * 8);
    payload.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(modules.len() as u32).to_le_bytes());
    payload.extend_from_slice(&(handle_slots.len() as u32).to_le_bytes());
    payload.extend_from_slice(&MAPPING_HANDOFF_VERSION.to_le_bytes());
    for mapping in entries {
        payload.extend_from_slice(&(mapping.base as u64).to_le_bytes());
        payload.extend_from_slice(&(mapping.len as u64).to_le_bytes());
        payload.extend_from_slice(&(mapping.behavior as u32).to_le_bytes());
        payload.extend_from_slice(&(mapping.storage as u32).to_le_bytes());
        payload.extend_from_slice(&(mapping.backing_slot as u64).to_le_bytes());
        payload.extend_from_slice(&mapping.backing_offset.to_le_bytes());
        payload.extend_from_slice(&mapping.view_protection.to_le_bytes());
        payload.extend_from_slice(&(mapping.domain as u32).to_le_bytes());
    }
    for address in modules.iter().chain(handle_slots) {
        payload.extend_from_slice(&(*address as u64).to_le_bytes());
    }
    payload
}

fn decode_mapping_registry(payload: &[u8]) -> Result<MappingRegistry, ForkError> {
    if payload.len() < 16 || payload[12..16] != MAPPING_HANDOFF_VERSION.to_le_bytes() {
        return Err(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        });
    }
    let mapping_count = u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
    let module_count = u32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
    let handle_count = u32::from_le_bytes(payload[8..12].try_into().unwrap()) as usize;
    let mappings_end = 16usize
        .checked_add(mapping_count.checked_mul(48).ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?)
        .ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
    let modules_end = mappings_end
        .checked_add(module_count.checked_mul(8).ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?)
        .ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
    let expected = modules_end
        .checked_add(handle_count.checked_mul(8).ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?)
        .ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
    if expected != payload.len() {
        return Err(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        });
    }
    let mut restored = MappingRegistry::default();
    for index in 0..mapping_count {
        let start = 16 + index * 48;
        let domain = ForkMappingDomain::from_raw(u32::from_le_bytes(
            payload[start + 44..start + 48].try_into().unwrap(),
        ))
        .ok_or(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        })?;
        let base = u64::from_le_bytes(payload[start..start + 8].try_into().unwrap()) as usize;
        let len = u64::from_le_bytes(payload[start + 8..start + 16].try_into().unwrap()) as usize;
        let behavior = match u32::from_le_bytes(payload[start + 16..start + 20].try_into().unwrap())
        {
            1 => ForkMappingBehavior::Copy,
            2 => ForkMappingBehavior::Zero,
            3 => ForkMappingBehavior::Omit,
            _ => {
                return Err(ForkError {
                    stage: ForkStage::HandoffRestore,
                    os_code: 0,
                });
            }
        };
        let storage = match u32::from_le_bytes(payload[start + 20..start + 24].try_into().unwrap())
        {
            0 => ForkMappingStorage::Ordinary,
            1 => ForkMappingStorage::Placeholder,
            2 => ForkMappingStorage::Section,
            3 => ForkMappingStorage::RetainedSection,
            4 => ForkMappingStorage::CopyOnWriteSection,
            5 => ForkMappingStorage::AnonymousSnapshot,
            _ => {
                return Err(ForkError {
                    stage: ForkStage::HandoffRestore,
                    os_code: 0,
                });
            }
        };
        let mapping = ForkMapping {
            base,
            len,
            domain,
            behavior,
            storage,
            backing_slot: u64::from_le_bytes(payload[start + 24..start + 32].try_into().unwrap())
                as usize,
            backing_offset: u64::from_le_bytes(payload[start + 32..start + 40].try_into().unwrap()),
            view_protection: u32::from_le_bytes(
                payload[start + 40..start + 44].try_into().unwrap(),
            ),
        };
        if !mapping.valid_storage_contract() {
            return Err(ForkError {
                stage: ForkStage::HandoffRestore,
                os_code: 0,
            });
        }
        restored.insert_mapping(mapping);
    }
    for index in 0..module_count {
        let start = mappings_end + index * 8;
        restored.insert_module(
            u64::from_le_bytes(payload[start..start + 8].try_into().unwrap()) as usize,
        );
    }
    for index in 0..handle_count {
        let start = modules_end + index * 8;
        let slot = u64::from_le_bytes(payload[start..start + 8].try_into().unwrap()) as usize;
        if !handle_slots::valid_slot_address(slot) || restored.handle_slots.contains(&slot) {
            return Err(ForkError {
                stage: ForkStage::HandoffRestore,
                os_code: 0,
            });
        }
        restored.handle_slots.push(slot);
    }
    if restored.entries.iter().any(|mapping| {
        mapping.storage.retains_section() && !restored.handle_slots.contains(&mapping.backing_slot)
    }) {
        return Err(ForkError {
            stage: ForkStage::HandoffRestore,
            os_code: 0,
        });
    }
    Ok(restored)
}

fn restore_mapping_registry(payload: &[u8]) -> Result<(), ForkError> {
    let restored = decode_mapping_registry(payload)?;
    *mappings().lock().map_err(|_| ForkError {
        stage: ForkStage::HandoffRestore,
        os_code: 0,
    })? = restored;
    Ok(())
}

#[cfg(test)]
mod mapping_domain_tests {
    use super::*;

    fn examples() -> [ForkMapping; 2] {
        let host = ForkMapping {
            base: 0x10000,
            len: 4096,
            behavior: ForkMappingBehavior::Copy,
            storage: ForkMappingStorage::Ordinary,
            backing_slot: 0,
            backing_offset: 0,
            view_protection: 0,
            domain: ForkMappingDomain::HostPrivate,
        };
        let guest = ForkMapping {
            base: 0x20000,
            domain: ForkMappingDomain::GuestMm,
            ..host
        };
        [host, guest]
    }

    #[test]
    fn ownership_survives_handoff_without_changing_storage_or_copy_behavior() {
        let mut entries = examples();
        entries[1].storage = ForkMappingStorage::RetainedSection;
        entries[1].backing_slot =
            kinakaze_alloc::ARENA_BASE + kinakaze_alloc::HEADER_PAGE_SIZE + 128;
        entries[1].backing_offset = 8192;
        entries[1].view_protection = 4;
        let payload = encode_mapping_registry(&entries, &[0x30000], &[entries[1].backing_slot]);
        let decoded = decode_mapping_registry(&payload).unwrap();
        for expected in entries {
            let actual = decoded
                .entries
                .iter()
                .find(|m| m.base == expected.base)
                .unwrap();
            assert_eq!(actual.domain, expected.domain);
            assert_eq!(actual.storage, expected.storage);
            assert_eq!(actual.behavior, ForkMappingBehavior::Copy);
            assert_eq!(actual.backing_slot, expected.backing_slot);
            assert_eq!(actual.backing_offset, expected.backing_offset);
        }
        assert_eq!(decoded.modules, [0x30000]);
    }

    #[test]
    fn handoff_rejects_unknown_ownership_old_version_and_truncation() {
        let payload = encode_mapping_registry(&examples(), &[], &[]);
        for end in 0..payload.len() {
            assert!(
                decode_mapping_registry(&payload[..end]).is_err(),
                "truncated at {end}"
            );
        }
        let mut corrupt = payload.clone();
        corrupt[12..16].copy_from_slice(&0u32.to_le_bytes());
        assert!(decode_mapping_registry(&corrupt).is_err());
        for raw in [2u32, u32::MAX] {
            let mut corrupt = payload.clone();
            corrupt[16 + 44..16 + 48].copy_from_slice(&raw.to_le_bytes());
            assert!(decode_mapping_registry(&corrupt).is_err());
        }
    }

    #[test]
    fn cow_handoff_requires_private_protection_and_a_retained_owner() {
        let mut entry = examples()[1];
        for storage in [
            ForkMappingStorage::CopyOnWriteSection,
            ForkMappingStorage::AnonymousSnapshot,
        ] {
            entry.storage = storage;
            entry.backing_slot =
                kinakaze_alloc::ARENA_BASE + kinakaze_alloc::HEADER_PAGE_SIZE + 128;
            for protection in [0x08, 0x80] {
                entry.view_protection = protection;
                let payload = encode_mapping_registry(&[entry], &[], &[entry.backing_slot]);
                let decoded = decode_mapping_registry(&payload).unwrap();
                assert_eq!(decoded.entries[0].storage, storage);
                assert_eq!(decoded.entries[0].view_protection, protection);
                assert!(
                    decode_mapping_registry(&encode_mapping_registry(&[entry], &[], &[])).is_err()
                );
            }
            for protection in [0, 0x02, 0x04, 0x20, 0x40] {
                entry.view_protection = protection;
                assert!(
                    decode_mapping_registry(&encode_mapping_registry(
                        &[entry],
                        &[],
                        &[entry.backing_slot],
                    ))
                    .is_err(),
                    "shared-write protection {protection:#x}"
                );
            }
        }
    }
}

/// Clones the calling process.
///
/// The parent receives the positive child PID and the child receives zero. The
/// current Windows milestone is deliberately single-threaded: callers must
/// ensure no other thread can mutate the managed arena during this operation.
///
/// # Safety
///
/// This resumes execution in a different process from a copied native stack.
/// Live values must either be plain values, refer to the managed arena, or be
/// kernel objects supported by a future handle-cloning layer. Rust and Windows
/// objects outside those categories must not be used by the child.
pub unsafe fn fork() -> Result<Pid, ForkError> {
    // SAFETY: forwarded from this function's contract.
    unsafe { fork_with_parent(None) }
}

/// Clones the calling process with an explicitly selected Linux parent.
///
/// `parent` is used for `clone(CLONE_PARENT)`: it changes only the process-tree
/// relationship, while address-space and descriptor cloning remain identical to
/// [`fork`].
///
/// # Safety
///
/// The same copied-stack and single-transaction requirements as [`fork`] apply.
pub unsafe fn fork_with_parent(parent: Option<u32>) -> Result<Pid, ForkError> {
    unsafe { fork_initialized(parent, 0, 0) }
}

// Fork and exec share handoff participants and process identity metadata. A
// sibling thread must not stage a second image over the first transaction.
static PROCESS_IMAGE_TRANSACTION: Mutex<()> = Mutex::new(());

/// Serialize an exec image handoff with native fork and other exec callers.
/// Failed exec releases this guard; successful exec terminates its whole native
/// worker. A fork child starts with the fresh runtime DLL's unlocked mutex.
pub fn lock_process_image() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_IMAGE_TRANSACTION
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

unsafe fn fork_initialized(
    parent: Option<u32>,
    initializer: usize,
    argument: u64,
) -> Result<Pid, ForkError> {
    let limit_error = services::task_creation_errno();
    if limit_error != 0 {
        return Err(ForkError {
            stage: ForkStage::HandoffPrepare,
            os_code: limit_error as u32,
        });
    }
    #[cfg(windows)]
    if job::namespaces::child_namespace_dead() {
        return Err(ForkError {
            stage: ForkStage::HandoffPrepare,
            os_code: 12,
        });
    }
    let fork_transaction = PROCESS_IMAGE_TRANSACTION
        .try_lock()
        .map_err(|_| ForkError {
            stage: ForkStage::HandoffPrepare,
            // EDEADLK: catches both an atfork re-entry and a concurrent transaction.
            os_code: 35,
        })?;
    let mut managed_fork =
        authority::ForkTransaction::prepare(parent).map_err(|errno| ForkError {
            stage: ForkStage::HandoffPrepare,
            os_code: errno as u32,
        })?;
    kinakaze_alloc::initialize().map_err(|_| ForkError {
        stage: ForkStage::Allocator,
        os_code: 0,
    })?;
    let _parent_override = ForkParentOverrideGuard::install(parent).map_err(|_| ForkError {
        stage: ForkStage::HandoffPrepare,
        os_code: 5,
    })?;

    let entries = ordered_participants()?;
    prepare_participants(&entries)?;
    // Freeze topology before serializing any participant, not only before the
    // native page copy. Otherwise mmap/close can invalidate a serialized owner
    // (including a registered handle slot) while the child is bootstrapping.
    #[cfg(windows)]
    let mapping_transaction = match begin_fork_mapping_transaction() {
        Some(transaction) => transaction,
        None => {
            finish_parent(&entries, -1);
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            });
        }
    };
    // Must precede the arena copy: the payload travels inside the arena, so
    // staging it afterwards would leave the child with nothing.
    if let Err(error) = stage_handoff_state(&entries) {
        #[cfg(windows)]
        drop(mapping_transaction);
        finish_parent(&entries, -1);
        return Err(error);
    }

    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        // SAFETY: the caller accepts the process-cloning contract above.
        let result = unsafe { windows::raw_fork(parent.unwrap_or(0)) };
        if result != 0 {
            trace_registered_tls_slots("raw-fork-returned", result as u64);
        }
        if result == 0 {
            // The child restores its participant registry from the handoff.
            // This temporary Vec belongs to the parent's private Rust heap;
            // its copied owner must neither be read nor freed in the child.
            core::mem::forget(entries);
            // The child executable started with a fresh, unlocked coordinator
            // mutex.  This guard belongs to the parent's lock state and must not
            // attempt to unlock the fresh object when its copied stack unwinds.
            core::mem::forget(fork_transaction);
            // copy_arena already reset this process's mapping lock. The copied
            // guard still belongs to the parent's recursion depth.
            core::mem::forget(mapping_transaction);
            // The executable image copy also carried the parent's cached host
            // pid and named-section view. Rebind the coordinator's own runtime
            // copy before any participant tries to register this child. DLL
            // participants reset their separate runtime copies later, but they
            // cannot repair the executable coordinator's `.data`.
            job::reset_after_fork();
            authority::reset_after_fork();
            // Only pointer-free manager reservation data crosses the clone.
            // Reconnect before VFS registration or any provider restore hook.
            if let Err(error) = managed_fork.adopt() {
                fork_diagnostic(format_args!(
                    "kinakaze: fork child adoption failed error={error}"
                ));
                unsafe { windows::terminate_failed_fork_child() };
            }
            // The child resumes here with the parent's arena already in place,
            // so the staged descriptor table can be adopted before returning to
            // guest code.
            if let Err(e) = restore_handoff_state() {
                fork_diagnostic(format_args!(
                    "kinakaze: fork child handoff failed stage={:?} error={}",
                    e.stage, e.os_code
                ));
                if fork_trace_enabled() {
                    eprintln!(
                        "kinakaze: fork child restore_handoff_state FAILED stage={:?} os={}",
                        e.stage, e.os_code
                    );
                }
                // The parent has already observed a successful host clone and
                // owns a waitable child at this point.  A Linux fork therefore
                // cannot be retroactively reported as a caller-side failure in
                // the child.  Terminate the unusable child instead.  For vfork
                // this also completes the inherited rendezvous first, so the
                // suspended parent is never stranded by restore failure.
                unsafe { windows::terminate_failed_fork_child() };
            }
            if initializer != 0 {
                let initialize: unsafe extern "sysv64" fn(u64) -> i32 =
                    unsafe { core::mem::transmute(initializer) };
                let error = unsafe { initialize(argument) };
                if error != 0 {
                    job::namespaces::set_fork_error(error as u32);
                    job::namespaces::mark_fork_restored();
                    unsafe { windows::terminate_failed_fork_child() };
                }
            }
            if let Err(error) = managed_fork.ready() {
                fork_diagnostic(format_args!(
                    "kinakaze: fork child readiness failed error={error}"
                ));
                unsafe { windows::terminate_failed_fork_child() };
            }
            if !job::namespaces::mark_fork_restored() {
                unsafe { windows::terminate_failed_fork_child() };
            }
            if managed_fork.await_activation().is_err() {
                unsafe { windows::terminate_failed_fork_child() };
            }
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze: fork child restore_handoff_state OK, pid={}",
                    std::process::id()
                );
            }
        } else {
            drop(mapping_transaction);
            // Parent hooks operate at the Windows boundary. In particular,
            // WSADuplicateSocketW requires the destination's host pid, so guest
            // pid translation must happen only after every hook has finished.
            finish_parent(&entries, result);
            trace_registered_tls_slots("parent-callbacks-finished", result as u64);
            if result > 0 {
                let Some(namespace_pid) = windows::take_last_fork_namespace_pid() else {
                    return Err(ForkError {
                        stage: ForkStage::CreateProcess,
                        os_code: 0,
                    });
                };
                windows::wait_fork_handoff(namespace_pid)?;
                if let Err(errno) = managed_fork.commit() {
                    if errno == 5 {
                        // A lost commit reply is not evidence that the child
                        // stayed gated. Do not invent a failed fork outcome.
                        eprintln!("kinakaze: fork ownership transfer outcome is unavailable");
                        std::process::abort();
                    }
                    windows::discard_failed_fork(namespace_pid);
                    return Err(ForkError {
                        stage: ForkStage::HandoffRestore,
                        os_code: errno as u32,
                    });
                }
                return Ok(job::namespaces::visible(namespace_pid).unwrap_or(namespace_pid) as i32);
            }
        }
        if result >= 0 {
            Ok(result)
        } else {
            Err(windows::last_fork_error())
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        let result: i64;
        // Linux x86_64 syscall 57 is fork. Its kernel return convention already
        // has exactly the parent/child split required by this API.
        unsafe {
            if parent.is_some() {
                core::arch::asm!(
                    "syscall",
                    inlateout("rax") 56_i64 => result,
                    in("rdi") 0x0000_8000_i64 | 17,
                    in("rsi") 0_i64,
                    in("rdx") 0_i64,
                    in("r10") 0_i64,
                    in("r8") 0_i64,
                    lateout("rcx") _,
                    lateout("r11") _,
                    options(nostack),
                );
            } else {
                core::arch::asm!(
                    "syscall",
                    inlateout("rax") 57_i64 => result,
                    lateout("rcx") _,
                    lateout("r11") _,
                    options(nostack),
                );
            }
        }
        if result < 0 {
            finish_parent(&entries, -1);
            Err(ForkError {
                stage: ForkStage::Unsupported,
                os_code: (-result) as u32,
            })
        } else {
            if result == 0 {
                restore_handoff_state()?;
            } else {
                finish_parent(&entries, result as i32);
            }
            Ok(result as Pid)
        }
    }

    #[cfg(not(any(
        all(windows, target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64")
    )))]
    {
        finish_parent(&entries, -1);
        Err(ForkError {
            stage: ForkStage::Unsupported,
            os_code: 0,
        })
    }
}

/// Fork entry exported by the host executable for substitute DLLs.
///
/// Keeping the actual clone in the executable is essential: a fresh child has
/// the executable mapped before any provider DLL, so its saved instruction
/// pointer is always valid while the bootstrap loader recreates those DLLs.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_fork() -> Pid {
    if fork_trace_enabled() {
        eprintln!(
            "kinakaze: fork coordinator entered in pid {}",
            std::process::id()
        );
    }
    // SAFETY: this is the C boundary for the same returns-twice operation.
    let result = match unsafe { fork() } {
        Ok(pid) => pid,
        Err(error) => -fork_errno(error),
    };
    if fork_trace_enabled() {
        eprintln!(
            "kinakaze: fork coordinator returned {result} in pid {}",
            std::process::id()
        );
    }
    result
}

/// Process-like clone coordinator with an explicit Linux parent.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_clone_fork(
    parent: u32,
    initializer: usize,
    flags: u64,
) -> Pid {
    // SAFETY: this is the same returns-twice boundary as `kinakaze_process_fork`.
    match unsafe { fork_initialized((parent != 0).then_some(parent), initializer, flags) } {
        Ok(pid) => pid,
        Err(error) => -fork_errno(error),
    }
}

/// `vfork` coordinator used by substitute libc providers.
///
/// The parent is suspended until the child either starts a replacement image or
/// exits. `stack_boundary` identifies the provider's private ABI-transition
/// stack; it is intentionally not copied back because it is host implementation
/// state, not memory visible to the Linux process.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_vfork(stack_boundary: usize) -> Pid {
    // SAFETY: the libc ABI wrapper supplies its live entry stack pointer.
    unsafe { windows::vfork(stack_boundary) }
}

/// Marks the active `vfork` child as having either completed exec or reached
/// `_exit`.  Calls made outside a vfork child are harmless no-ops.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_vfork_complete(exec_succeeded: i32) {
    // SAFETY: completion uses only the inheritable synchronization handles set
    // by `windows::vfork` in this process.
    unsafe { windows::complete_vfork(exec_succeeded != 0) }
}

/// Process-external storage for POSIX process groups, sessions and the
/// cross-process pending-signal masks built on top of them.
///
/// Group membership is inherently a relation *between* processes, and on this
/// host every one of those processes is a separate Windows process with its own
/// address space: `fork` starts a new image, and `exec` starts another one on
/// top of that. A per-process table would therefore answer every interesting
/// question wrong. The registry is instead a named shared section, so a slot
/// written by a shell is read by the child it just placed in a group, and a
/// pending-signal bit set by one process is drained by another.
///
/// This module deliberately contains no signal *policy* — no dispositions, no
/// handlers, no stopping. It is the storage and the liveness sweep, nothing
/// more. [`kinakaze_vfs::job`] owns the semantics, and the wait coordinator
/// below reads the same slots to answer `WUNTRACED` and `WCONTINUED`. Both
/// need it, and the runtime is the lower of the two crates, which is the only
/// reason the bytes live here rather than next to the policy that uses them.
///
/// # Divergences from a real kernel
///
/// * A slot is reclaimed by scanning for dead owners rather than by the kernel
///   dropping it, so a process killed with `TerminateProcess` leaves its slot
///   behind until the next sweep notices the pid is gone.
/// * Windows pids are recycled. A sweep that runs after recycling can find a
///   live, unrelated process behind a stale slot. Slots therefore carry the
///   creating process's start time in `token`, and a slot whose owner's token no
///   longer matches is treated as dead.
/// * The section lives in the `Local\` namespace, which is per-logon-session.
///   Hosted processes run by the same user on the same desktop share it;
///   processes in different Terminal Services sessions do not see each other,
///   where a real kernel would have them share one pid space.
#[cfg(windows)]
pub mod job {
    pub(crate) mod fork_handoff;
    pub mod namespaces;
    pub mod notifications;
    use core::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;

    use windows_sys::Win32::Foundation::{
        CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows_sys::Win32::System::SystemInformation::{
        GetSystemTimePreciseAsFileTime, GetTickCount64,
    };
    use windows_sys::Win32::System::Threading::{
        CreateMutexW, GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, INFINITE,
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ReleaseMutex,
        WaitForSingleObject,
    };

    /// Identifies a compatible layout. Bump the trailing digits whenever a field
    /// moves: a process built against the old shape must refuse the section
    /// rather than misread a neighbour's slot.
    const MAGIC: u64 = 0x4352_5950_4944_3135; // "CRYPID15"

    /// Slots in the table. Each is one hosted process.
    const CAPACITY: usize = 512;
    const SLOT_SIZE: usize = 1872;
    const HEADER_SIZE: usize = 64;
    const INDEX_CAPACITY: usize = 1024;
    const INDEX_ENTRY_SIZE: usize = 4;
    const HOST_INDEX_OFFSET: usize = HEADER_SIZE;
    const NAMESPACE_INDEX_OFFSET: usize = HOST_INDEX_OFFSET + INDEX_CAPACITY * INDEX_ENTRY_SIZE;
    const SLOTS_OFFSET: usize = NAMESPACE_INDEX_OFFSET + INDEX_CAPACITY * INDEX_ENTRY_SIZE;
    const FD_LINK_CAPACITY: usize = 8192;
    const FD_LINK_TARGET_CAPACITY: usize = 1024;
    const FD_LINK_SIZE: usize = 16 + FD_LINK_TARGET_CAPACITY;
    const FD_LINKS_OFFSET: usize = SLOTS_OFFSET + CAPACITY * SLOT_SIZE;
    const PID_NAMESPACES_OFFSET: usize = FD_LINKS_OFFSET + FD_LINK_CAPACITY * FD_LINK_SIZE;
    const SECTION_SIZE: usize = PID_NAMESPACES_OFFSET + namespaces::TABLE_SIZE;

    // Header field offsets.
    const HEADER_MAGIC: usize = 0;
    const HEADER_CAPACITY: usize = 8;
    const HEADER_SLOT_SIZE: usize = 12;
    const HEADER_NEXT_PID: usize = 16;
    const HEADER_FOREGROUND: usize = 20;
    const HEADER_NAMESPACE_ID: usize = 24;
    const HEADER_INDEX_CAPACITY: usize = 28;
    const HEADER_FD_LINK_CAPACITY: usize = 32;
    const HEADER_FD_LINK_TARGET_CAPACITY: usize = 36;
    const FIRST_NAMESPACE_PID: u32 = 1;

    // Cross-process `/proc/<pid>/fd` magic-link record offsets. The record is
    // deliberately logical metadata, never a Windows HANDLE: inherited and
    // reconstructed handles are process-local, while Linux exposes the same
    // open-file description name in every process that inherited it.
    const FD_LINK_STATE: usize = 0;
    const FD_LINK_PID: usize = 4;
    const FD_LINK_FD: usize = 8;
    const FD_LINK_LENGTH: usize = 12;
    const FD_LINK_TARGET: usize = 16;
    const FD_LINK_EMPTY: u32 = 0;
    const FD_LINK_OCCUPIED: u32 = 1;
    const FD_LINK_TOMBSTONE: u32 = 2;

    // Slot field offsets.
    const SLOT_PID: usize = 0;
    const SLOT_PGID: usize = 4;
    const SLOT_SID: usize = 8;
    const SLOT_PPID: usize = 12;
    const SLOT_PENDING: usize = 16;
    const SLOT_FLAGS: usize = 24;
    const SLOT_DELEGATE: usize = 28;
    const SLOT_STATE: usize = 32;
    const SLOT_STOP_SIGNAL: usize = 36;
    const SLOT_REPORT: usize = 40;
    const SLOT_REPORT_SIGNAL: usize = 44;
    const SLOT_TOKEN: usize = 48;
    const SLOT_NAMESPACE_PID: usize = 56;
    const SLOT_NAMESPACE: usize = 60;
    const SLOT_COMM_LENGTH: usize = 64;
    const SLOT_EXE_LENGTH: usize = 68;
    const SLOT_CMDLINE_LENGTH: usize = 72;
    const SLOT_CGROUP_LENGTH: usize = 76;
    const SLOT_START_TICKS: usize = 80;
    const SLOT_COMM: usize = 88;
    const COMM_CAPACITY: usize = 32;
    const SLOT_EXE: usize = SLOT_COMM + COMM_CAPACITY;
    const EXE_CAPACITY: usize = 240;
    const SLOT_CMDLINE: usize = SLOT_EXE + EXE_CAPACITY;
    const CMDLINE_CAPACITY: usize = 512;
    const SLOT_CGROUP: usize = SLOT_CMDLINE + CMDLINE_CAPACITY;
    const CGROUP_CAPACITY: usize = 256;
    const SLOT_PDEATHSIG: usize = SLOT_CGROUP + CGROUP_CAPACITY;
    const SLOT_PDEATH_DELIVERED: usize = SLOT_PDEATHSIG + 4;
    const SLOT_NOFILE_SOFT: usize = SLOT_PDEATH_DELIVERED + 4;
    const SLOT_NOFILE_HARD: usize = SLOT_NOFILE_SOFT + 8;
    // Per-process fork command shared by the executable and provider DLLs.
    // Zero requests ordinary fork parentage; a nonzero value implements
    // CLONE_PARENT for the transaction currently being snapshotted.
    const SLOT_FORK_PARENT: usize = SLOT_NOFILE_HARD + 8;
    const SLOT_MOUNT_NAMESPACE: usize = SLOT_FORK_PARENT + 8;
    const SLOT_OOM_SCORE_ADJ: usize = SLOT_MOUNT_NAMESPACE + 8;
    const SLOT_OOM_SCORE_ADJ_MIN: usize = SLOT_OOM_SCORE_ADJ + 4;
    const SLOT_TIME_NAMESPACE: usize = 1184;
    const SLOT_TIME_CHILDREN: usize = 1192;
    const SLOT_USER_NAMESPACE: usize = 1200;
    const SLOT_MQUEUE_SOFT: usize = 1824;
    const SLOT_MQUEUE_HARD: usize = 1832;
    const SLOT_FSIZE_SOFT: usize = 1840;
    const SLOT_NPROC_SOFT: usize = 1856;
    // Linux INIT_RLIMITS uses INR_OPEN_CUR and INR_OPEN_MAX. A privileged
    // service may raise the hard limit without allocating descriptor storage.
    const DEFAULT_NOFILE_SOFT: u64 = 1024;
    const DEFAULT_NOFILE_HARD: u64 = 4096;

    /// The first version is one root namespace shared by every loader in the
    /// current Windows session.  Keeping the id in every slot makes the layout
    /// ready for nested namespace mappings without confusing guest and host
    /// process identifiers in the meantime.
    const ROOT_NAMESPACE: u32 = 1;

    /// Process is running normally.
    pub const STATE_RUNNING: u32 = 0;
    /// Process has stopped and is waiting for `SIGCONT`.
    pub const STATE_STOPPED: u32 = 1;

    /// The slot describes a process whose image was replaced by `exec`; signals
    /// for it belong to the process named by `delegate`.
    pub const FLAG_PROXY: u32 = 1 << 0;
    /// The process has already replaced its image, which is what makes
    /// `setpgid` on it fail with `EACCES` the way Linux specifies.
    pub const FLAG_EXECED: u32 = 1 << 1;
    /// The owner is gone but the slot is retained until a parent reads the
    /// termination report out of it.
    pub const FLAG_ZOMBIE: u32 = 1 << 2;
    // The parent has observed kernel completion, not merely the child's
    // pre-termination Linux status publication.
    const FLAG_EXIT_NOTIFIED: u32 = 1 << 3;
    /// Adopt orphaned descendants in this process's PID namespace.
    pub const FLAG_SUBREAPER: u32 = 1 << 4;

    /// No report is waiting.
    const REPORT_NONE: u32 = 0;
    const REPORT_STOPPED: u32 = 1;
    const REPORT_CONTINUED: u32 = 2;
    const REPORT_KILLED: u32 = 3;
    const REPORT_EXITED: u32 = 4;
    const REPORT_UNOBSERVABLE: u32 = 5;

    /// One process's registry entry, copied out under the table lock.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Entry {
        /// Real Windows process id stored by the process table.
        pub pid: u32,
        /// PID visible to the hosted Linux program.
        pub namespace_pid: u32,
        /// Shared PID namespace containing `pid`.
        pub namespace: u32,
        pub pgid: u32,
        pub sid: u32,
        pub ppid: u32,
        pub flags: u32,
        pub delegate: u32,
        pub state: u32,
        pub stop_signal: u32,
        /// Signal configured by `PR_SET_PDEATHSIG`, or zero when disabled.
        pub parent_death_signal: u32,
        /// Windows process creation time used to reject a recycled host pid.
        pub token: u64,
        /// Linux `/proc/<pid>/stat` field 22, in the 100 Hz USER_HZ domain.
        ///
        /// This is recorded when the Linux process is born and deliberately
        /// survives an exec handoff to a different Windows host process.
        pub start_ticks: u64,
    }

    /// Process information exported through `/proc` and tools such as `ps`.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct ProcessInfo {
        pub entry: Entry,
        pub comm: String,
        pub executable: String,
        pub cmdline: Vec<u8>,
    }

    /// Why exact cross-process `/proc/<pid>/fd` metadata could not be served.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum FdLinkError {
        /// The named shared section or its mutex could not be opened.
        RegistryUnavailable,
        /// The Linux-visible process does not exist in this PID namespace.
        ProcessNotFound,
        /// A negative descriptor number was supplied.
        InvalidDescriptor,
        /// A magic-link target does not fit the registry's declared ABI limit.
        TargetTooLong,
        /// The shared registry has no free record for another descriptor.
        CapacityExhausted,
        /// A shared record was structurally invalid or not UTF-8.
        CorruptRecord,
    }

    impl Entry {
        /// Whether this process leads its session, which `setsid` and `setpgid`
        /// both have to refuse to disturb.
        pub fn is_session_leader(self) -> bool {
            self.namespace_pid == self.sid
        }

        /// Whether this process leads its group, which is what makes `setsid`
        /// fail with `EPERM`.
        pub fn is_group_leader(self) -> bool {
            self.namespace_pid == self.pgid
        }
    }

    /// A child state transition a parent's `waitpid` has not collected yet.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum StateChange {
        /// Stopped by the given signal.
        Stopped(u32),
        /// Resumed by `SIGCONT`.
        Continued,
        /// Terminated by the given signal rather than by an ordinary exit.
        Killed(u32),
        /// Terminated normally with the given low eight-bit exit status.
        Exited(u32),
        /// The host process disappeared without publishing a Linux exit cause.
        ///
        /// This is an integrity error, not a guessed signal or exit status.
        Unobservable,
    }

    /// Base address of this process's view of the section, or zero.
    static BASE: AtomicUsize = AtomicUsize::new(0);
    /// The section handle, held open only to keep the name resolvable.
    static SECTION: AtomicUsize = AtomicUsize::new(0);
    /// The named mutex serializing structural changes, or zero.
    static GUARD: AtomicUsize = AtomicUsize::new(0);
    /// This process's immutable Windows pid.
    struct HostPid(std::cell::UnsafeCell<u32>);

    // SAFETY: process startup and the single-threaded fork-child restore path
    // are the only writers. All ordinary callers run after either has finished.
    unsafe impl Sync for HostPid {}

    static HOST_PID: HostPid = HostPid(std::cell::UnsafeCell::new(0));
    /// Serializes the mapping setup itself, which the named mutex cannot: the
    /// mutex is one of the things being created.
    static SETUP: Mutex<()> = Mutex::new(());

    /// Builds a NUL-terminated UTF-16 object name.
    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(core::iter::once(0)).collect()
    }

    /// Returns this process's own Windows pid.
    pub fn current_host_pid() -> u32 {
        // SAFETY: initialized by the runtime CRT, then only rebound while the
        // fork child is single-threaded and before guest execution resumes.
        unsafe { HOST_PID.0.get().read() }
    }

    fn bind_host_pid() {
        // SAFETY: GetCurrentProcessId has no preconditions.
        let pid = unsafe { GetCurrentProcessId() };
        // SAFETY: called only from process startup or single-threaded child restore.
        unsafe { HOST_PID.0.get().write(pid) };
    }

    extern "C" fn host_pid_initializer() {
        bind_host_pid();
    }

    #[used]
    #[unsafe(link_section = ".CRT$XCU")]
    static HOST_PID_INITIALIZER: extern "C" fn() = host_pid_initializer;

    /// Returns the pid visible inside the shared Linux pid namespace.
    ///
    /// Before registration (or when named mappings are forbidden) the host pid
    /// is a safe compatibility fallback.  Normal libc entry points register
    /// first, so hosted programs consistently see the allocated guest pid.
    pub fn current_pid() -> u32 {
        if super::authority::get().is_some() {
            return super::authority::process_id();
        }
        lookup_host(current_host_pid()).map_or_else(current_host_pid, |entry| entry.namespace_pid)
    }

    /// Maps the shared section, creating it if this is the first process.
    ///
    /// Returns `None` when the section cannot be created, which makes every
    /// operation degrade to "this process is alone" rather than fail loudly.
    /// That matters because a hosted program must still run when, for example,
    /// an unusually restrictive token forbids the named object.
    fn map() -> Option<*mut u8> {
        let cached = BASE.load(Ordering::Acquire);
        if cached != 0 {
            return Some(cached as *mut u8);
        }
        let _setup = SETUP.lock().ok()?;
        // Another thread may have finished while this one waited.
        let cached = BASE.load(Ordering::Acquire);
        if cached != 0 {
            return Some(cached as *mut u8);
        }

        let epoch = super::authority::get().map(|_| super::authority::domain_id());
        let guard_name = wide(&epoch.map_or_else(
            || "Local\\kinakaze.pidns.lock.v13".to_owned(),
            |epoch| format!("Local\\kinakaze.v2.pidns.{epoch:016x}.lock.v13"),
        ));
        // SAFETY: a null attribute pointer requests the default descriptor and
        // the name is a live NUL-terminated buffer for the duration of the call.
        let guard = unsafe { CreateMutexW(core::ptr::null(), 0, guard_name.as_ptr()) };
        if guard.is_null() {
            return None;
        }

        let section_name = wide(&epoch.map_or_else(
            || "Local\\kinakaze.pidns.v13".to_owned(),
            |epoch| format!("Local\\kinakaze.v2.pidns.{epoch:016x}.v13"),
        ));
        // SAFETY: INVALID_HANDLE_VALUE requests a pagefile-backed section, and
        // the name buffer is live for the call.
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_READWRITE,
                0,
                SECTION_SIZE as u32,
                section_name.as_ptr(),
            )
        };
        if mapping.is_null() {
            // SAFETY: the mutex handle was created by this call and is unused.
            unsafe { CloseHandle(guard) };
            return None;
        }
        // SAFETY: the mapping was just created with the requested size.
        let view: MEMORY_MAPPED_VIEW_ADDRESS =
            unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, SECTION_SIZE) };
        if view.Value.is_null() {
            // SAFETY: both handles were created by this call and are unused.
            unsafe {
                CloseHandle(mapping);
                CloseHandle(guard);
            }
            return None;
        }
        // The mapping handle is deliberately kept open for the life of the
        // process. A named section is a *temporary* object: its name leaves the
        // NT object namespace as soon as the last handle to it closes, even
        // though an open view keeps the object itself alive. Closing it here —
        // which looks harmless, since the view is what this code uses — made
        // every process create a fresh private section under the same name and
        // silently talk to nobody.
        SECTION.store(mapping as usize, Ordering::Release);
        let base = view.Value.cast::<u8>();

        // Initialize the header exactly once. Section pages start zeroed, so a
        // zero magic means nobody has claimed it yet; the named mutex is what
        // stops two first-time creators from racing.
        GUARD.store(guard as usize, Ordering::Release);
        let acquired = acquire(guard);
        // SAFETY: `base` addresses SECTION_SIZE writable bytes and the header
        // offsets are within it.
        unsafe {
            if load64(base, HEADER_MAGIC) != MAGIC {
                store32(base, HEADER_CAPACITY, CAPACITY as u32);
                store32(base, HEADER_SLOT_SIZE, SLOT_SIZE as u32);
                // The first real process owns PID 1. Never renumber an existing
                // namespace, reserve a phantom init, or alias it to the caller.
                store32(base, HEADER_NEXT_PID, FIRST_NAMESPACE_PID);
                store32(base, HEADER_FOREGROUND, 0);
                store32(base, HEADER_NAMESPACE_ID, ROOT_NAMESPACE);
                store32(base, HEADER_INDEX_CAPACITY, INDEX_CAPACITY as u32);
                store32(base, HEADER_FD_LINK_CAPACITY, FD_LINK_CAPACITY as u32);
                store32(
                    base,
                    HEADER_FD_LINK_TARGET_CAPACITY,
                    FD_LINK_TARGET_CAPACITY as u32,
                );
                // The magic is published last so a concurrent opener never sees
                // it set over a half-written header.
                store64(base, HEADER_MAGIC, MAGIC);
            }
        }
        if acquired {
            release(guard);
        }

        // SAFETY: the header was just validated or written by this process.
        let compatible = unsafe {
            load64(base, HEADER_MAGIC) == MAGIC
                && load32(base, HEADER_CAPACITY) == CAPACITY as u32
                && load32(base, HEADER_SLOT_SIZE) == SLOT_SIZE as u32
                && load32(base, HEADER_NAMESPACE_ID) == ROOT_NAMESPACE
                && load32(base, HEADER_INDEX_CAPACITY) == INDEX_CAPACITY as u32
                && load32(base, HEADER_FD_LINK_CAPACITY) == FD_LINK_CAPACITY as u32
                && load32(base, HEADER_FD_LINK_TARGET_CAPACITY) == FD_LINK_TARGET_CAPACITY as u32
        };
        if !compatible {
            // A differently shaped table from another build is not usable. Drop
            // the view and behave as an unregistered process rather than
            // scribbling over slots whose fields are somewhere else.
            // SAFETY: this process owns the view and both handles.
            unsafe {
                UnmapViewOfFile(view);
                CloseHandle(mapping);
                CloseHandle(guard);
            }
            SECTION.store(0, Ordering::Release);
            GUARD.store(0, Ordering::Release);
            return None;
        }

        BASE.store(base as usize, Ordering::Release);
        Some(base)
    }

    /// Takes the named mutex, reporting whether it must be released.
    ///
    /// `WAIT_ABANDONED` means the previous owner died while holding it. The
    /// table is still usable: every field is an independently written machine
    /// word, so an interrupted writer leaves a stale value rather than a torn
    /// one, and the liveness sweep removes it.
    fn acquire(guard: HANDLE) -> bool {
        if guard.is_null() {
            return false;
        }
        // SAFETY: the handle belongs to this process and stays open.
        let waited = unsafe { WaitForSingleObject(guard, INFINITE) };
        waited == WAIT_OBJECT_0 || waited == WAIT_ABANDONED
    }

    fn release(guard: HANDLE) {
        if !guard.is_null() {
            // SAFETY: the caller acquired this mutex on this thread.
            unsafe { ReleaseMutex(guard) };
        }
    }

    /// Runs `body` with the table mapped and the named mutex held.
    ///
    /// Nothing that can block belongs inside `body`: the mutex is shared with
    /// every other hosted process, so a caller that waits while holding it
    /// stalls the whole session.
    fn with_table<T>(body: impl FnOnce(*mut u8) -> T) -> Option<T> {
        let base = map()?;
        let guard = GUARD.load(Ordering::Acquire) as HANDLE;
        let acquired = acquire(guard);
        let result = body(base);
        if acquired {
            release(guard);
        }
        Some(result)
    }

    // Field accessors. Every slot field is a naturally aligned machine word so
    // that a reader outside the mutex — the pending mask is read that way on the
    // signal fast path — never observes a torn value.

    /// # Safety
    /// `base` must address the mapped section and `offset` a 4-aligned field.
    unsafe fn load32(base: *mut u8, offset: usize) -> u32 {
        // SAFETY: forwarded from this function's contract.
        unsafe { (*base.add(offset).cast::<AtomicU32>()).load(Ordering::Acquire) }
    }

    /// # Safety
    /// Same contract as [`load32`].
    unsafe fn store32(base: *mut u8, offset: usize, value: u32) {
        // SAFETY: forwarded from this function's contract.
        unsafe { (*base.add(offset).cast::<AtomicU32>()).store(value, Ordering::Release) }
    }

    /// # Safety
    /// `base` must address the mapped section and `offset` an 8-aligned field.
    unsafe fn load64(base: *mut u8, offset: usize) -> u64 {
        // SAFETY: forwarded from this function's contract.
        unsafe { (*base.add(offset).cast::<AtomicU64>()).load(Ordering::Acquire) }
    }

    /// # Safety
    /// Same contract as [`load64`].
    unsafe fn store64(base: *mut u8, offset: usize, value: u64) {
        // SAFETY: forwarded from this function's contract.
        unsafe { (*base.add(offset).cast::<AtomicU64>()).store(value, Ordering::Release) }
    }

    /// # Safety
    /// Same contract as [`load64`].
    unsafe fn or64(base: *mut u8, offset: usize, value: u64) -> u64 {
        // SAFETY: forwarded from this function's contract.
        unsafe { (*base.add(offset).cast::<AtomicU64>()).fetch_or(value, Ordering::AcqRel) }
    }

    /// # Safety
    /// Same contract as [`load64`].
    unsafe fn swap64(base: *mut u8, offset: usize, value: u64) -> u64 {
        // SAFETY: forwarded from this function's contract.
        unsafe { (*base.add(offset).cast::<AtomicU64>()).swap(value, Ordering::AcqRel) }
    }

    /// Address of slot `index`.
    ///
    /// # Safety
    /// `base` must address the mapped section and `index` be below [`CAPACITY`].
    unsafe fn slot(base: *mut u8, index: usize) -> *mut u8 {
        // SAFETY: forwarded from this function's contract.
        unsafe { base.add(SLOTS_OFFSET + index * SLOT_SIZE) }
    }

    /// Address of one shared `/proc/<pid>/fd` record.
    ///
    /// # Safety
    /// `base` must address the validated mapping and `index` must be below
    /// [`FD_LINK_CAPACITY`].
    unsafe fn fd_link_slot(base: *mut u8, index: usize) -> *mut u8 {
        // SAFETY: forwarded from this function's contract.
        unsafe { base.add(FD_LINKS_OFFSET + index * FD_LINK_SIZE) }
    }

    fn fd_link_hash(pid: u32, fd: u32) -> usize {
        let mut value = (u64::from(pid) << 32) | u64::from(fd);
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value ^= value >> 27;
        value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        value as usize & (FD_LINK_CAPACITY - 1)
    }

    /// Finds an occupied fd-link record through open addressing.
    ///
    /// # Safety
    /// `base` must address the validated mapping.
    unsafe fn find_fd_link(base: *mut u8, pid: u32, fd: u32) -> Option<*mut u8> {
        let start = fd_link_hash(pid, fd);
        for probe in 0..FD_LINK_CAPACITY {
            let record = unsafe { fd_link_slot(base, (start + probe) & (FD_LINK_CAPACITY - 1)) };
            match unsafe { load32(record, FD_LINK_STATE) } {
                FD_LINK_EMPTY => return None,
                FD_LINK_OCCUPIED
                    if unsafe { load32(record, FD_LINK_PID) } == pid
                        && unsafe { load32(record, FD_LINK_FD) } == fd =>
                {
                    return Some(record);
                }
                _ => {}
            }
        }
        None
    }

    /// Inserts or replaces one fd-link record.
    ///
    /// # Safety
    /// The mapping mutex must be held and `target` must fit its fixed field.
    unsafe fn write_fd_link(base: *mut u8, pid: u32, fd: u32, target: &[u8]) -> bool {
        let start = fd_link_hash(pid, fd);
        let mut first_tombstone = None;
        for probe in 0..FD_LINK_CAPACITY {
            let record = unsafe { fd_link_slot(base, (start + probe) & (FD_LINK_CAPACITY - 1)) };
            let state = unsafe { load32(record, FD_LINK_STATE) };
            if state == FD_LINK_OCCUPIED
                && unsafe { load32(record, FD_LINK_PID) } == pid
                && unsafe { load32(record, FD_LINK_FD) } == fd
            {
                unsafe { write_fd_link_record(record, pid, fd, target) };
                return true;
            }
            if state == FD_LINK_TOMBSTONE && first_tombstone.is_none() {
                first_tombstone = Some(record);
            }
            if state == FD_LINK_EMPTY {
                let destination = first_tombstone.unwrap_or(record);
                unsafe { write_fd_link_record(destination, pid, fd, target) };
                return true;
            }
        }
        if let Some(destination) = first_tombstone {
            unsafe { write_fd_link_record(destination, pid, fd, target) };
            return true;
        }
        false
    }

    /// # Safety
    /// `record` must address an fd-link record, the mutex must be held and the
    /// target length must not exceed [`FD_LINK_TARGET_CAPACITY`].
    unsafe fn write_fd_link_record(record: *mut u8, pid: u32, fd: u32, target: &[u8]) {
        unsafe {
            // Keep the record unpublished until every field is complete.
            store32(record, FD_LINK_STATE, FD_LINK_TOMBSTONE);
            store32(record, FD_LINK_PID, pid);
            store32(record, FD_LINK_FD, fd);
            store32(record, FD_LINK_LENGTH, target.len() as u32);
            core::ptr::write_bytes(record.add(FD_LINK_TARGET), 0, FD_LINK_TARGET_CAPACITY);
            core::ptr::copy_nonoverlapping(
                target.as_ptr(),
                record.add(FD_LINK_TARGET),
                target.len(),
            );
            store32(record, FD_LINK_STATE, FD_LINK_OCCUPIED);
        }
    }

    /// Removes every fd-link owned by one Linux-visible process.
    ///
    /// # Safety
    /// The mapping mutex must be held.
    unsafe fn clear_fd_links(base: *mut u8, pid: u32) {
        if pid == 0 {
            return;
        }
        for index in 0..FD_LINK_CAPACITY {
            let record = unsafe { fd_link_slot(base, index) };
            if unsafe { load32(record, FD_LINK_STATE) } == FD_LINK_OCCUPIED
                && unsafe { load32(record, FD_LINK_PID) } == pid
            {
                unsafe {
                    store32(record, FD_LINK_STATE, FD_LINK_TOMBSTONE);
                    store32(record, FD_LINK_PID, 0);
                    store32(record, FD_LINK_FD, 0);
                    store32(record, FD_LINK_LENGTH, 0);
                }
            }
        }
    }

    /// Copies the inherited open-file-description names from parent to child.
    ///
    /// # Safety
    /// The mapping mutex must be held and both pids must name live rows.
    unsafe fn clone_fd_links(base: *mut u8, parent: u32, child: u32) -> bool {
        let mut links = Vec::new();
        for index in 0..FD_LINK_CAPACITY {
            let record = unsafe { fd_link_slot(base, index) };
            if unsafe { load32(record, FD_LINK_STATE) } != FD_LINK_OCCUPIED
                || unsafe { load32(record, FD_LINK_PID) } != parent
            {
                continue;
            }
            let fd = unsafe { load32(record, FD_LINK_FD) };
            let len = unsafe { load32(record, FD_LINK_LENGTH) } as usize;
            if len > FD_LINK_TARGET_CAPACITY {
                return false;
            }
            let target =
                unsafe { core::slice::from_raw_parts(record.add(FD_LINK_TARGET), len).to_vec() };
            links.push((fd, target));
        }
        unsafe { clear_fd_links(base, child) };
        for (fd, target) in links {
            if !unsafe { write_fd_link(base, child, fd, &target) } {
                unsafe { clear_fd_links(base, child) };
                return false;
            }
        }
        true
    }

    /// Reads one slot without interpreting it.
    ///
    /// # Safety
    /// `entry` must address a slot inside the mapped section.
    unsafe fn read_slot(entry: *mut u8) -> Entry {
        // SAFETY: forwarded from this function's contract.
        unsafe {
            Entry {
                pid: load32(entry, SLOT_PID),
                namespace_pid: load32(entry, SLOT_NAMESPACE_PID),
                namespace: load32(entry, SLOT_NAMESPACE),
                pgid: load32(entry, SLOT_PGID),
                sid: load32(entry, SLOT_SID),
                ppid: load32(entry, SLOT_PPID),
                flags: load32(entry, SLOT_FLAGS),
                delegate: load32(entry, SLOT_DELEGATE),
                state: load32(entry, SLOT_STATE),
                stop_signal: load32(entry, SLOT_STOP_SIGNAL),
                parent_death_signal: load32(entry, SLOT_PDEATHSIG),
                token: load64(entry, SLOT_TOKEN),
                start_ticks: load64(entry, SLOT_START_TICKS),
            }
        }
    }

    /// Makes a slot free without leaving stale process metadata behind.
    ///
    /// # Safety
    /// `entry` must address a slot and the caller must hold the table mutex.
    unsafe fn clear_slot(base: *mut u8, entry: *mut u8) {
        unsafe {
            clear_fd_links(base, load32(entry, SLOT_NAMESPACE_PID));
            // Unpublish both lookup keys before clearing the remaining fields.
            // Readers normally hold the mutex, but this ordering also keeps a
            // lock-free observer from treating a half-cleared row as live.
            store32(entry, SLOT_NAMESPACE_PID, 0);
            store32(entry, SLOT_PID, 0);
            store64(entry, SLOT_PENDING, 0);
            store32(entry, SLOT_PGID, 0);
            store32(entry, SLOT_SID, 0);
            store32(entry, SLOT_PPID, 0);
            store32(entry, SLOT_FLAGS, 0);
            store32(entry, SLOT_DELEGATE, 0);
            store32(entry, SLOT_STATE, STATE_RUNNING);
            store32(entry, SLOT_STOP_SIGNAL, 0);
            store32(entry, SLOT_REPORT, REPORT_NONE);
            store32(entry, SLOT_REPORT_SIGNAL, 0);
            store64(entry, SLOT_TOKEN, 0);
            store64(entry, SLOT_START_TICKS, 0);
            store32(entry, SLOT_NAMESPACE, 0);
            store32(entry, SLOT_COMM_LENGTH, 0);
            store32(entry, SLOT_EXE_LENGTH, 0);
            store32(entry, SLOT_CMDLINE_LENGTH, 0);
            store32(entry, SLOT_CGROUP_LENGTH, 0);
            store32(entry, SLOT_PDEATHSIG, 0);
            store32(entry, SLOT_PDEATH_DELIVERED, 0);
            store64(entry, SLOT_NOFILE_SOFT, 0);
            store64(entry, SLOT_NOFILE_HARD, 0);
            store64(entry, SLOT_MQUEUE_SOFT, 0);
            store64(entry, SLOT_MQUEUE_HARD, 0);
        }
    }

    /// Finds the slot holding `pid`, or `None`.
    ///
    /// # Safety
    /// `base` must address the mapped section.
    unsafe fn find(base: *mut u8, pid: u32) -> Option<*mut u8> {
        unsafe { indexed_find(base, NAMESPACE_INDEX_OFFSET, SLOT_NAMESPACE_PID, pid) }
    }

    /// Lock-free readers use the authoritative rows, not an index that writers
    /// temporarily clear while rebuilding. The one-entry thread cache makes
    /// the current process's signal hot path constant time, without waiting
    /// for a writer that a stopping process may already have suspended.
    unsafe fn find_stable(base: *mut u8, pid: u32) -> Option<*mut u8> {
        use core::cell::Cell;
        thread_local! {
            static LAST: Cell<(usize, u32, usize)> = const { Cell::new((0, 0, 0)) };
        }
        if pid == 0 {
            return None;
        }
        LAST.with(|cached| {
            let (mapping, key, index) = cached.get();
            if mapping == base as usize && key == pid {
                let entry = unsafe { slot(base, index) };
                if unsafe { load32(entry, SLOT_NAMESPACE_PID) } == pid {
                    return Some(entry);
                }
            }
            for index in 0..CAPACITY {
                let entry = unsafe { slot(base, index) };
                if unsafe { load32(entry, SLOT_NAMESPACE_PID) } == pid {
                    cached.set((base as usize, pid, index));
                    return Some(entry);
                }
            }
            None
        })
    }

    /// Finds a slot by the Windows pid that owns it.
    ///
    /// # Safety
    /// `base` must address the mapped section.
    unsafe fn find_host(base: *mut u8, host_pid: u32) -> Option<*mut u8> {
        unsafe { indexed_find(base, HOST_INDEX_OFFSET, SLOT_PID, host_pid) }
    }

    /// Looks up a slot through one of the two fixed open-addressed indexes.
    ///
    /// # Safety
    /// Every offset must describe the validated mapping layout.
    unsafe fn indexed_find(
        base: *mut u8,
        index_offset: usize,
        key_offset: usize,
        key: u32,
    ) -> Option<*mut u8> {
        if key == 0 {
            return None;
        }
        let start = (key.wrapping_mul(0x9e37_79b1) as usize) & (INDEX_CAPACITY - 1);
        for probe in 0..INDEX_CAPACITY {
            let bucket = (start + probe) & (INDEX_CAPACITY - 1);
            let encoded = unsafe { load32(base, index_offset + bucket * INDEX_ENTRY_SIZE) };
            if encoded == 0 {
                return None;
            }
            let slot_index = encoded as usize - 1;
            if slot_index >= CAPACITY {
                return None;
            }
            let entry = unsafe { slot(base, slot_index) };
            if unsafe { load32(entry, key_offset) } == key {
                return Some(entry);
            }
        }
        None
    }

    /// Rebuilds both indexes after a structural change. Lookups are O(1); the
    /// O(n) rebuild happens only on process creation, exit or exec.
    ///
    /// # Safety
    /// `base` must address the mapped section and the caller must hold its mutex.
    unsafe fn rebuild_indexes(base: *mut u8) {
        for bucket in 0..INDEX_CAPACITY {
            unsafe {
                store32(base, HOST_INDEX_OFFSET + bucket * INDEX_ENTRY_SIZE, 0);
                store32(base, NAMESPACE_INDEX_OFFSET + bucket * INDEX_ENTRY_SIZE, 0);
            }
        }
        for slot_index in 0..CAPACITY {
            let entry = unsafe { slot(base, slot_index) };
            let host_pid = unsafe { load32(entry, SLOT_PID) };
            let namespace_pid = unsafe { load32(entry, SLOT_NAMESPACE_PID) };
            if host_pid == 0 || namespace_pid == 0 {
                continue;
            }
            unsafe { insert_index(base, HOST_INDEX_OFFSET, host_pid, slot_index) };
            unsafe { insert_index(base, NAMESPACE_INDEX_OFFSET, namespace_pid, slot_index) };
        }
    }

    /// # Safety
    /// Same contract as [`rebuild_indexes`].
    unsafe fn insert_index(base: *mut u8, offset: usize, key: u32, slot_index: usize) {
        let start = (key.wrapping_mul(0x9e37_79b1) as usize) & (INDEX_CAPACITY - 1);
        for probe in 0..INDEX_CAPACITY {
            let bucket = (start + probe) & (INDEX_CAPACITY - 1);
            let field = offset + bucket * INDEX_ENTRY_SIZE;
            if unsafe { load32(base, field) } == 0 {
                unsafe { store32(base, field, slot_index as u32 + 1) };
                return;
            }
        }
    }

    /// Allocates an unused guest pid. The monotonically increasing cursor is
    /// shared, so separate loader.exe instances can never independently hand
    /// out the same live pid.
    ///
    /// # Safety
    /// `base` must address the mapped section and the caller must hold its mutex.
    unsafe fn allocate_pid(base: *mut u8) -> Option<u32> {
        let mut candidate = unsafe { load32(base, HEADER_NEXT_PID) }.max(1);
        for _ in 0..u32::MAX - 1 {
            if candidate != 0 && unsafe { find(base, candidate) }.is_none() {
                let next = candidate.checked_add(1).unwrap_or(1).max(1);
                unsafe { store32(base, HEADER_NEXT_PID, next) };
                return Some(candidate);
            }
            candidate = candidate.checked_add(1).unwrap_or(1).max(1);
        }
        None
    }

    /// A value that changes when a pid is recycled.
    ///
    /// The process creation time is the cheapest thing Windows offers that
    /// distinguishes "the process I registered" from "a different process that
    /// was handed the same pid after mine exited".
    fn start_token(pid: u32) -> u64 {
        // SAFETY: a limited-information handle is enough for GetProcessTimes.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return 0;
        }
        let mut created = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut ignored = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        // SAFETY: the handle is live and all four out-parameters are locals.
        let ok = unsafe {
            GetProcessTimes(
                handle,
                &mut created,
                &mut ignored,
                &mut ignored,
                &mut ignored,
            )
        };
        // SAFETY: the handle was opened by this call and is not used again.
        unsafe { CloseHandle(handle) };
        if ok == 0 {
            return 0;
        }
        (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)
    }

    /// Converts a Windows process creation timestamp into Linux USER_HZ ticks
    /// since boot.
    ///
    /// `GetProcessTimes` and `GetSystemTimePreciseAsFileTime` share the FILETIME
    /// epoch, while `GetTickCount64` is elapsed boot time and includes suspend.
    /// Sampling uptime on both sides of the wall-clock read bounds the
    /// conversion error to the duration of this function. The result is stored
    /// in the shared process row exactly once, so every reader observes one
    /// stable Linux birth time and an exec handoff preserves it.
    fn linux_start_ticks(token: u64) -> Option<u64> {
        const FILETIME_TICKS_PER_MILLISECOND: u64 = 10_000;
        const MILLISECONDS_PER_USER_HZ_TICK: u64 = 10;

        // SAFETY: both time queries have no preconditions; FILETIME points to a
        // live writable local.
        let before = unsafe { GetTickCount64() };
        let mut now = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        unsafe { GetSystemTimePreciseAsFileTime(&raw mut now) };
        let after = unsafe { GetTickCount64() };
        let sample_span = after.checked_sub(before)?;
        let uptime_ms = before.checked_add(sample_span / 2)?;
        let now = (u64::from(now.dwHighDateTime) << 32) | u64::from(now.dwLowDateTime);
        let age_ms = now.checked_sub(token)? / FILETIME_TICKS_PER_MILLISECOND;
        let started_ms = uptime_ms.checked_sub(age_ms)?;
        let ticks = started_ms / MILLISECONDS_PER_USER_HZ_TICK;
        (ticks != 0).then_some(ticks)
    }

    /// Reports whether `pid` names a process that is still running.
    ///
    /// A pid this process cannot open is reported as dead. That is the right
    /// answer for the usual cause — the process exited — and the wrong one for
    /// the rare cause, a process owned by another user. Hosted processes are all
    /// children of one another under one token, so the rare case does not arise
    /// within a session.
    pub fn alive(host_pid: u32) -> bool {
        if host_pid == 0 {
            return false;
        }
        if host_pid == current_host_pid() {
            return true;
        }
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, host_pid) };
        if handle.is_null() {
            return false;
        }
        // ExitProcess publishes its exit code before the process object becomes
        // signalled. Only this object transition proves teardown is complete;
        // GetExitCodeProcess can make a SIGCHLD/WNOHANG reaper miss the exit.
        // It also cannot distinguish a real native exit code of 259 from live.
        let result = unsafe { WaitForSingleObject(handle, 0) };
        // SAFETY: the handle was opened by this call and is not used again.
        unsafe { CloseHandle(handle) };
        result != WAIT_OBJECT_0
    }

    /// Clears slots whose owner is gone.
    ///
    /// # Safety
    /// `base` must address the mapped section and the caller must hold the
    /// table mutex.
    unsafe fn sweep(base: *mut u8) {
        for index in 0..CAPACITY {
            // SAFETY: the index is in range and the section is mapped.
            let entry = unsafe { slot(base, index) };
            // SAFETY: the slot is inside the section.
            let host_pid = unsafe { load32(entry, SLOT_PID) };
            if host_pid == 0 {
                continue;
            }
            // SAFETY: the slot is inside the section.
            let flags = unsafe { load32(entry, SLOT_FLAGS) };
            // SAFETY: the slot is inside the section.
            let token = unsafe { load64(entry, SLOT_TOKEN) };
            if flags & FLAG_ZOMBIE != 0 {
                // A retained termination report belongs to the parent. It is
                // dropped only once that parent is itself gone, which is the
                // moment nobody can ever collect it.
                // SAFETY: the slot is inside the section.
                let ppid = unsafe { load32(entry, SLOT_PPID) };
                let parent_alive = unsafe { find(base, ppid) }
                    .map(|parent| unsafe { load32(parent, SLOT_PID) })
                    .is_some_and(alive);
                if parent_alive {
                    continue;
                }
            } else {
                let live = alive(host_pid);
                let observed_token = live.then(|| start_token(host_pid)).unwrap_or(0);
                if live && token == observed_token {
                    continue;
                }
                unsafe { namespaces::exiting(base, entry) };
                // A child remains waitable after its host process exits. If it
                // vanished without first publishing an exact Linux exit cause,
                // retain an explicit integrity-error report rather than
                // guessing an exit code or silently recycling its pid.
                let ppid = unsafe { load32(entry, SLOT_PPID) };
                let parent_alive = unsafe { find(base, ppid) }
                    .map(|parent| unsafe { load32(parent, SLOT_PID) })
                    .is_some_and(alive);
                if parent_alive {
                    unsafe {
                        store32(entry, SLOT_REPORT_SIGNAL, 0);
                        store32(entry, SLOT_REPORT, REPORT_UNOBSERVABLE);
                        store32(entry, SLOT_FLAGS, flags | FLAG_ZOMBIE);
                    }
                    if super::fork_trace_enabled() {
                        eprintln!(
                            "kinakaze job: retaining unobservable exit host={} pid={} ppid={}",
                            host_pid,
                            unsafe { load32(entry, SLOT_NAMESPACE_PID) },
                            ppid,
                        );
                    }
                    continue;
                }
                if super::fork_trace_enabled() {
                    eprintln!(
                        "kinakaze job: sweep removing host={} pid={} live={} token={:#x} observed={:#x} flags={:#x}",
                        host_pid,
                        unsafe { load32(entry, SLOT_NAMESPACE_PID) },
                        live,
                        token,
                        observed_token,
                        flags,
                    );
                }
            }
            // SAFETY: the slot is inside the section and the mutex is held.
            unsafe { clear_slot(base, entry) };
        }
        unsafe { rebuild_indexes(base) };
    }

    /// Installs or updates `host_pid`'s entry and returns its guest identity.
    ///
    /// Explicit registration updates group, session, parent and flags. A VFS
    /// attaching to an existing process must use `claim` instead, so startup
    /// values cannot undo concurrent adoption or process-group changes.
    pub fn register(host_pid: u32, pgid: u32, sid: u32, ppid: u32, flags: u32) -> Option<Entry> {
        register_scoped(host_pid, pgid, sid, ppid, flags, ppid, Registration::Update)
    }

    /// Attach a local VFS to its existing Linux process row. Adoption, setpgid
    /// and exec may have changed that row since the startup identity was made.
    /// Inspect and retain it under the same lock; a stale snapshot must never
    /// overwrite a newly selected subreaper or reset process flags.
    pub fn claim(host_pid: u32, pgid: u32, sid: u32, ppid: u32) -> Option<Entry> {
        register_scoped(host_pid, pgid, sid, ppid, 0, ppid, Registration::Claim)
    }

    #[derive(PartialEq)]
    enum Registration {
        Update,
        Claim,
    }

    fn register_scoped(
        host_pid: u32,
        pgid: u32,
        sid: u32,
        ppid: u32,
        flags: u32,
        namespace_parent: u32,
        mode: Registration,
    ) -> Option<Entry> {
        let authoritative_pid = if super::authority::get().is_some() {
            if host_pid == current_host_pid() {
                Some(super::authority::identity()?.ok()?.pid)
            } else {
                Some(super::authority::reserved_pid()?)
            }
        } else {
            None
        };
        if host_pid == 0 {
            return None;
        }
        let token = start_token(host_pid);
        if token == 0 {
            return None;
        }
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            unsafe { sweep(base) };
            // SAFETY: the section is mapped and the mutex is held.
            let existing = unsafe { find_host(base, host_pid) };
            let (entry, start_ticks) = match existing {
                Some(entry) if mode == Registration::Claim => {
                    let entry = unsafe { read_slot(entry) };
                    if entry.flags & FLAG_ZOMBIE != 0
                        || authoritative_pid.is_some_and(|pid| pid != entry.namespace_pid)
                    {
                        return None;
                    }
                    return Some(entry);
                }
                Some(entry) => {
                    // Re-registering the same host process must not move its
                    // Linux birth time as wall-clock sampling advances.
                    let start_ticks = unsafe { load64(entry, SLOT_START_TICKS) };
                    if start_ticks == 0 {
                        return None;
                    }
                    (entry, start_ticks)
                }
                None => {
                    let start_ticks = linux_start_ticks(token)?;
                    let mut chosen = None;
                    for index in 0..CAPACITY {
                        // SAFETY: the index is in range.
                        let candidate = unsafe { slot(base, index) };
                        // SAFETY: the slot is inside the section.
                        if unsafe { load32(candidate, SLOT_PID) } == 0 {
                            chosen = Some(candidate);
                            break;
                        }
                    }
                    let Some(candidate) = chosen else {
                        return None;
                    };
                    // A fresh slot starts with no pending signals or reports
                    // from whichever process used it before.
                    // SAFETY: the slot is inside the section.
                    unsafe {
                        store64(candidate, SLOT_PENDING, 0);
                        store32(candidate, SLOT_REPORT, REPORT_NONE);
                        store32(candidate, SLOT_REPORT_SIGNAL, 0);
                        store32(candidate, SLOT_STATE, STATE_RUNNING);
                        store32(candidate, SLOT_STOP_SIGNAL, 0);
                        store32(candidate, SLOT_DELEGATE, 0);
                        store32(candidate, SLOT_COMM_LENGTH, 0);
                        store32(candidate, SLOT_EXE_LENGTH, 0);
                        store32(candidate, SLOT_CMDLINE_LENGTH, 0);
                        store32(candidate, SLOT_CGROUP_LENGTH, 0);
                        // Linux clears PR_SET_PDEATHSIG in a fork child.  A new
                        // process row is the exact point at which that rule is
                        // applied; exec reuses the existing row and preserves it.
                        store32(candidate, SLOT_PDEATHSIG, 0);
                        store32(candidate, SLOT_PDEATH_DELIVERED, 0);
                        store64(candidate, SLOT_NOFILE_SOFT, DEFAULT_NOFILE_SOFT);
                        store64(candidate, SLOT_NOFILE_HARD, DEFAULT_NOFILE_HARD);
                        store64(candidate, SLOT_MQUEUE_SOFT, 819200);
                        store64(candidate, SLOT_MQUEUE_HARD, 819200);
                        for offset in [SLOT_FSIZE_SOFT, SLOT_FSIZE_SOFT + 8, SLOT_NPROC_SOFT, SLOT_NPROC_SOFT + 8] {
                            store64(candidate, offset, u64::MAX);
                        }
                        let mount_namespace = find(base, namespace_parent)
                            .map(|parent| load64(parent, SLOT_MOUNT_NAMESPACE)).unwrap_or(1);
                        store64(candidate, SLOT_MOUNT_NAMESPACE, mount_namespace.max(1));
                        let time_namespace = find(base, namespace_parent)
                            .map(|parent| load64(parent, SLOT_TIME_CHILDREN)).unwrap_or(1).max(1);
                        store64(candidate, SLOT_TIME_NAMESPACE, time_namespace);
                        store64(candidate, SLOT_TIME_CHILDREN, time_namespace);
                        store64(candidate, SLOT_USER_NAMESPACE, find(base, namespace_parent).map(|p| load64(p, SLOT_USER_NAMESPACE)).unwrap_or(1).max(1));
                        store32(candidate, SLOT_OOM_SCORE_ADJ, 0);
                        store32(candidate, SLOT_OOM_SCORE_ADJ_MIN, 0);
                    }
                    (candidate, start_ticks)
                }
            };
            // Existing owners preserve their guest pid. A new owner gets the
            // next value from the shared namespace allocator.
            let namespace_pid = unsafe { load32(entry, SLOT_NAMESPACE_PID) };
            let namespace_pid = if let Some(authoritative_pid) = authoritative_pid {
                if namespace_pid != 0 && namespace_pid != authoritative_pid {
                    return None;
                }
                if unsafe { find(base, authoritative_pid) }.is_some_and(|owner| owner != entry) {
                    return None;
                }
                authoritative_pid
            } else if namespace_pid == 0 {
                unsafe { allocate_pid(base) }?
            } else {
                namespace_pid
            };
            if existing.is_none() && !unsafe { namespaces::initialize_child(base, entry, namespace_parent, namespace_pid) } {
                return None;
            }
            // SAFETY: the slot is inside the section and the mutex is held.
            unsafe {
                notifications::topology(base, ppid);
                store32(entry, SLOT_PGID, if pgid == 0 { namespace_pid } else { pgid });
                store32(entry, SLOT_SID, if sid == 0 { namespace_pid } else { sid });
                store32(entry, SLOT_PPID, ppid);
                store32(entry, SLOT_FLAGS, flags);
                store64(entry, SLOT_TOKEN, token);
                store64(entry, SLOT_START_TICKS, start_ticks);
                store32(entry, SLOT_NAMESPACE, ROOT_NAMESPACE);
                store32(entry, SLOT_PID, host_pid);
                // Guest fast paths find by namespace pid without taking the
                // table mutex, so publish that mapping only after the real
                // process row is complete.
                store32(entry, SLOT_NAMESPACE_PID, namespace_pid);
                rebuild_indexes(base);
                if super::fork_trace_enabled() {
                    eprintln!(
                        "kinakaze job: registered host={host_pid} pid={namespace_pid} ppid={ppid} pgid={} sid={} token={token:#x} start_ticks={start_ticks}",
                        load32(entry, SLOT_PGID),
                        load32(entry, SLOT_SID),
                    );
                }
                Some(read_slot(entry))
            }
        })
        .flatten()
    }

    /// Removes `pid`'s entry.
    pub fn release_slot(pid: u32) {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            if let Some(entry) = unsafe { find(base, pid) } {
                if super::fork_trace_enabled() {
                    eprintln!("kinakaze job: releasing pid={pid} host={}", unsafe {
                        load32(entry, SLOT_PID)
                    });
                }
                // SAFETY: the slot is inside the section and the mutex is held.
                unsafe {
                    clear_slot(base, entry);
                    rebuild_indexes(base);
                }
            }
        });
    }

    /// Check lifetime and identity using the same kernel object, so a recycled
    /// PID cannot match the previous owner between two separate OpenProcess calls.
    fn live_owner(host_pid: u32, token: u64) -> bool {
        let local = host_pid == current_host_pid();
        let handle = unsafe {
            if local {
                GetCurrentProcess()
            } else {
                OpenProcess(
                    PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    host_pid,
                )
            }
        };
        if handle.is_null() {
            return false;
        }
        let mut created: FILETIME = unsafe { core::mem::zeroed() };
        let mut ignored: FILETIME = unsafe { core::mem::zeroed() };
        let matches = unsafe {
            (local
                || WaitForSingleObject(handle, 0) == windows_sys::Win32::Foundation::WAIT_TIMEOUT)
                && GetProcessTimes(
                    handle,
                    &mut created,
                    &mut ignored,
                    &mut ignored,
                    &mut ignored,
                ) != 0
                && token
                    == ((u64::from(created.dwHighDateTime) << 32)
                        | u64::from(created.dwLowDateTime))
        };
        if !local {
            unsafe { CloseHandle(handle) };
        }
        matches
    }

    /// Most lookups only observe one live identity. Validate its ancestry as
    /// well: an abruptly terminated parent must still trigger subreaper adoption.
    /// Full enumeration/reaping keeps using sweep; ordinary input/filesystem
    /// queries need not open every process or rebuild the whole index.
    ///
    /// # Safety
    /// The mapped table mutex must be held and `entry` must be a current slot.
    unsafe fn checked_entry(base: *mut u8, entry: *mut u8) -> Option<Entry> {
        let pid = unsafe { load32(entry, SLOT_NAMESPACE_PID) };
        let mut at = entry;
        for depth in 0..CAPACITY {
            let flags = unsafe { load32(at, SLOT_FLAGS) };
            let zombie = flags & FLAG_ZOMBIE != 0;
            // A retained zombie is valid after its own kernel object exits,
            // but its parent must still exist to collect that report.
            if (depth != 0 || !zombie)
                && !live_owner(unsafe { load32(at, SLOT_PID) }, unsafe {
                    load64(at, SLOT_TOKEN)
                })
            {
                break;
            }
            let parent = unsafe { load32(at, SLOT_PPID) };
            let Some(parent) = (unsafe { find(base, parent) }) else {
                if !zombie {
                    return Some(unsafe { read_slot(entry) });
                }
                break;
            };
            at = parent;
        }
        // A changed lifetime can affect adoption, namespaces and wait reports.
        // Preserve that single authoritative transition before reading again.
        unsafe {
            sweep(base);
            find(base, pid).map(|entry| read_slot(entry))
        }
    }

    /// Reads `pid`'s entry.
    pub fn lookup(pid: u32) -> Option<Entry> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            unsafe { find(base, pid).and_then(|entry| checked_entry(base, entry)) }
        })
        .flatten()
    }

    /// Reads the entry owned by a Windows process id.
    pub fn lookup_host(host_pid: u32) -> Option<Entry> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            unsafe { find_host(base, host_pid).and_then(|entry| checked_entry(base, entry)) }
        })
        .flatten()
    }

    /// Returns every row whose Linux parent is `ppid`, including zombies.
    ///
    /// Unlike process-listing helpers this does not discard termination rows:
    /// they are the authoritative source consumed by `waitid(2)`.
    pub fn children_of(ppid: u32) -> Result<Vec<Entry>, ()> {
        with_table(|base| {
            let mut found = Vec::new();
            for index in 0..CAPACITY {
                // SAFETY: the index is in range and the mapping was validated.
                let entry = unsafe { slot(base, index) };
                // SAFETY: the complete row is read while holding the mutex.
                let record = unsafe { read_slot(entry) };
                if record.pid != 0 && record.ppid == ppid {
                    found.push(record);
                }
            }
            found
        })
        .ok_or(())
    }

    /// Converts a namespace pid to the real Windows pid in O(1).
    pub fn host_pid(namespace_pid: u32) -> Option<u32> {
        lookup(namespace_pid).map(|entry| entry.pid)
    }

    /// Logical mount membership, copied at process creation and retained at exec.
    /// The VFS owns the referenced kernel section; this row only publishes its id.
    pub fn mount_namespace(pid: u32) -> Option<u64> {
        with_table(|base| unsafe {
            sweep(base);
            find(base, pid).map(|entry| load64(entry, SLOT_MOUNT_NAMESPACE).max(1))
        })
        .flatten()
    }

    pub fn set_mount_namespace(pid: u32, namespace: u64) -> bool {
        if namespace == 0 {
            return false;
        }
        with_table(|base| unsafe {
            let Some(entry) = find(base, pid) else {
                return false;
            };
            store64(entry, SLOT_MOUNT_NAMESPACE, namespace);
            true
        })
        .unwrap_or(false)
    }

    pub fn user_namespace(pid: u32) -> Option<u64> {
        with_table(|base| unsafe {
            find(base, pid).map(|entry| load64(entry, SLOT_USER_NAMESPACE).max(1))
        })
        .flatten()
    }
    pub fn set_user_namespace(pid: u32, id: u64) -> bool {
        if id == 0 {
            return false;
        }
        with_table(|base| unsafe {
            let Some(entry) = find(base, pid) else {
                return false;
            };
            store64(entry, SLOT_USER_NAMESPACE, id);
            true
        })
        .unwrap_or(false)
    }

    /// Current and child time namespace membership; section ownership lives in VFS.
    pub fn time_namespaces(pid: u32) -> Option<(u64, u64)> {
        with_table(|base| unsafe {
            find(base, pid).map(|entry| {
                (
                    load64(entry, SLOT_TIME_NAMESPACE).max(1),
                    load64(entry, SLOT_TIME_CHILDREN).max(1),
                )
            })
        })
        .flatten()
    }
    pub fn set_time_namespaces(pid: u32, current: u64, children: u64) -> bool {
        if current == 0 || children == 0 {
            return false;
        }
        with_table(|base| unsafe {
            let Some(entry) = find(base, pid) else {
                return false;
            };
            store64(entry, SLOT_TIME_NAMESPACE, current);
            store64(entry, SLOT_TIME_CHILDREN, children);
            true
        })
        .unwrap_or(false)
    }

    /// Process-wide OOM selection bias and the privileged lower bound. These
    /// are guest policy values, not Windows memory-priority classes.
    pub fn oom_adjustment(pid: u32) -> Option<(i32, i32)> {
        with_table(|base| unsafe {
            find(base, pid).map(|entry| {
                (
                    load32(entry, SLOT_OOM_SCORE_ADJ) as i32,
                    load32(entry, SLOT_OOM_SCORE_ADJ_MIN) as i32,
                )
            })
        })
        .flatten()
    }

    pub fn set_oom_adjustment(
        pid: u32,
        value: i32,
        privileged: bool,
        legacy: bool,
    ) -> Result<(), i32> {
        if !(-1000..=1000).contains(&value) {
            return Err(22);
        }
        with_table(|base| unsafe {
            let entry = find(base, pid).ok_or(3)?;
            let floor = load32(
                entry,
                if legacy {
                    SLOT_OOM_SCORE_ADJ
                } else {
                    SLOT_OOM_SCORE_ADJ_MIN
                },
            ) as i32;
            if value < floor && !privileged {
                return Err(13);
            }
            store32(entry, SLOT_OOM_SCORE_ADJ, value as u32);
            if privileged && !legacy {
                store32(entry, SLOT_OOM_SCORE_ADJ_MIN, value as u32);
            }
            Ok(())
        })
        .ok_or(5)?
    }

    fn inherit_oom_adjustment(parent: u32, child: u32) -> bool {
        with_table(|base| unsafe {
            let (Some(parent), Some(child)) = (find(base, parent), find(base, child)) else {
                return false;
            };
            store32(
                child,
                SLOT_OOM_SCORE_ADJ,
                load32(parent, SLOT_OOM_SCORE_ADJ),
            );
            store32(
                child,
                SLOT_OOM_SCORE_ADJ_MIN,
                load32(parent, SLOT_OOM_SCORE_ADJ_MIN),
            );
            true
        })
        .unwrap_or(false)
    }

    /// Converts a real Windows pid to the shared namespace pid in O(1).
    pub fn namespace_pid(host_pid: u32) -> Option<u32> {
        lookup_host(host_pid).map(|entry| entry.namespace_pid)
    }

    /// Atomically replaces one process's complete `/proc/<pid>/fd` view.
    ///
    /// The operation is all-or-nothing: validation and capacity accounting
    /// happen before the old snapshot is unpublished. Callers therefore never
    /// observe a partially replaced descriptor table.
    pub fn replace_fd_links(
        namespace_pid: u32,
        links: &[(i32, String)],
    ) -> Result<(), FdLinkError> {
        if namespace_pid == 0 {
            return Err(FdLinkError::ProcessNotFound);
        }
        for (index, (fd, target)) in links.iter().enumerate() {
            if *fd < 0 {
                return Err(FdLinkError::InvalidDescriptor);
            }
            if target.len() > FD_LINK_TARGET_CAPACITY {
                return Err(FdLinkError::TargetTooLong);
            }
            if links[..index].iter().any(|(other, _)| other == fd) {
                return Err(FdLinkError::InvalidDescriptor);
            }
        }
        with_table(|base| {
            if unsafe { find(base, namespace_pid) }.is_none() {
                return Err(FdLinkError::ProcessNotFound);
            }
            let mut other_links = 0usize;
            for index in 0..FD_LINK_CAPACITY {
                let record = unsafe { fd_link_slot(base, index) };
                if unsafe { load32(record, FD_LINK_STATE) } == FD_LINK_OCCUPIED
                    && unsafe { load32(record, FD_LINK_PID) } != namespace_pid
                {
                    other_links += 1;
                }
            }
            if other_links.saturating_add(links.len()) > FD_LINK_CAPACITY {
                return Err(FdLinkError::CapacityExhausted);
            }
            unsafe { clear_fd_links(base, namespace_pid) };
            for (fd, target) in links {
                if !unsafe { write_fd_link(base, namespace_pid, *fd as u32, target.as_bytes()) } {
                    // Capacity was proven above. Reaching this branch means the
                    // shared hash structure is corrupt, so leave no partial view.
                    unsafe { clear_fd_links(base, namespace_pid) };
                    return Err(FdLinkError::CorruptRecord);
                }
            }
            Ok(())
        })
        .ok_or(FdLinkError::RegistryUnavailable)?
    }

    /// Returns the exact published magic-link target for one foreign fd.
    pub fn fd_link(namespace_pid: u32, fd: i32) -> Result<Option<String>, FdLinkError> {
        if fd < 0 {
            return Err(FdLinkError::InvalidDescriptor);
        }
        with_table(|base| {
            if unsafe { find(base, namespace_pid) }.is_none() {
                return Err(FdLinkError::ProcessNotFound);
            }
            let Some(record) = (unsafe { find_fd_link(base, namespace_pid, fd as u32) }) else {
                return Ok(None);
            };
            let len = unsafe { load32(record, FD_LINK_LENGTH) } as usize;
            if len > FD_LINK_TARGET_CAPACITY {
                return Err(FdLinkError::CorruptRecord);
            }
            let bytes = unsafe { core::slice::from_raw_parts(record.add(FD_LINK_TARGET), len) };
            let target = core::str::from_utf8(bytes)
                .map_err(|_| FdLinkError::CorruptRecord)?
                .to_owned();
            Ok(Some(target))
        })
        .ok_or(FdLinkError::RegistryUnavailable)?
    }

    /// Lists the descriptor numbers in one published foreign fd table.
    pub fn fd_links(namespace_pid: u32) -> Result<Vec<i32>, FdLinkError> {
        with_table(|base| {
            if unsafe { find(base, namespace_pid) }.is_none() {
                return Err(FdLinkError::ProcessNotFound);
            }
            let mut descriptors = Vec::new();
            for index in 0..FD_LINK_CAPACITY {
                let record = unsafe { fd_link_slot(base, index) };
                if unsafe { load32(record, FD_LINK_STATE) } == FD_LINK_OCCUPIED
                    && unsafe { load32(record, FD_LINK_PID) } == namespace_pid
                {
                    let fd = unsafe { load32(record, FD_LINK_FD) };
                    if fd > i32::MAX as u32 {
                        return Err(FdLinkError::CorruptRecord);
                    }
                    descriptors.push(fd as i32);
                }
            }
            descriptors.sort_unstable();
            Ok(descriptors)
        })
        .ok_or(FdLinkError::RegistryUnavailable)?
    }

    /// Copies a fork parent's published fd links to its child.
    fn inherit_fd_links(parent: u32, child: u32) -> Result<(), FdLinkError> {
        with_table(|base| {
            if unsafe { find(base, parent) }.is_none() || unsafe { find(base, child) }.is_none() {
                return Err(FdLinkError::ProcessNotFound);
            }
            if unsafe { clone_fd_links(base, parent, child) } {
                Ok(())
            } else {
                Err(FdLinkError::CapacityExhausted)
            }
        })
        .ok_or(FdLinkError::RegistryUnavailable)?
    }

    /// Updates the Linux-facing identity of one process.
    pub fn set_process_identity(
        namespace_pid: u32,
        comm: &str,
        executable: &str,
        cmdline: &[u8],
    ) -> bool {
        // Linux TASK_COMM_LEN includes its terminator, so only 15 bytes are
        // visible. Keep the shared string valid UTF-8 while applying that ABI
        // limit; `/proc/*/stat`, `/proc/*/status` and `ps` all consume it.
        let mut comm_length = comm.len().min(15);
        while comm_length > 0 && !comm.is_char_boundary(comm_length) {
            comm_length -= 1;
        }
        let comm = &comm.as_bytes()[..comm_length];
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            let Some(entry) = (unsafe { find(base, namespace_pid) }) else {
                return false;
            };
            // SAFETY: all ranges are fixed fields within this slot.
            unsafe {
                write_blob(entry, SLOT_COMM, COMM_CAPACITY, SLOT_COMM_LENGTH, comm);
                write_blob(
                    entry,
                    SLOT_EXE,
                    EXE_CAPACITY,
                    SLOT_EXE_LENGTH,
                    executable.as_bytes(),
                );
                write_blob(
                    entry,
                    SLOT_CMDLINE,
                    CMDLINE_CAPACITY,
                    SLOT_CMDLINE_LENGTH,
                    cmdline,
                );
            }
            true
        })
        .unwrap_or(false)
    }

    /// Records the cgroup v2 directory that contains one Linux-visible process.
    ///
    /// This is process-namespace state, so it lives beside the shared PID row
    /// rather than in one DLL's globals. A path is either published completely
    /// or rejected; it is never truncated into a different cgroup name.
    pub fn set_cgroup_path(namespace_pid: u32, path: &str) -> bool {
        if namespace_pid == 0 || path.len() > CGROUP_CAPACITY {
            return false;
        }
        with_table(|base| {
            // SAFETY: the section is mapped and the named mutex is held.
            let Some(entry) = (unsafe { find(base, namespace_pid) }) else {
                return false;
            };
            // SAFETY: the fixed cgroup range belongs to this slot and the
            // capacity check above prevents truncation.
            unsafe {
                write_blob(
                    entry,
                    SLOT_CGROUP,
                    CGROUP_CAPACITY,
                    SLOT_CGROUP_LENGTH,
                    path.as_bytes(),
                );
            }
            true
        })
        .unwrap_or(false)
    }

    /// Returns the exact cgroup v2 directory recorded for a process.
    pub fn cgroup_path(namespace_pid: u32) -> Option<String> {
        with_table(|base| {
            // SAFETY: the section is mapped and the named mutex is held.
            let entry = unsafe { find(base, namespace_pid) }?;
            let length = unsafe { load32(entry, SLOT_CGROUP_LENGTH) } as usize;
            if length > CGROUP_CAPACITY {
                return None;
            }
            // SAFETY: the validated length is within this slot's cgroup field.
            let bytes = unsafe { core::slice::from_raw_parts(entry.add(SLOT_CGROUP), length) };
            core::str::from_utf8(bytes).ok().map(str::to_owned)
        })
        .flatten()
    }

    /// Changes the signal delivered when this process's Linux parent dies.
    ///
    /// The setting is stored in the shared process row: fork starts with a
    /// fresh row and therefore clears it, while exec transfers the existing row
    /// to the replacement host process and preserves it.
    pub fn set_parent_death_signal(namespace_pid: u32, signal: u32) -> Result<(), ()> {
        with_table(|base| {
            let Some(entry) = (unsafe { find(base, namespace_pid) }) else {
                return Err(());
            };
            unsafe {
                store32(entry, SLOT_PDEATHSIG, signal);
                store32(entry, SLOT_PDEATH_DELIVERED, 0);
            }
            Ok(())
        })
        .ok_or(())?
    }

    /// Reads the current `PR_SET_PDEATHSIG` value.
    pub fn parent_death_signal(namespace_pid: u32) -> Result<u32, ()> {
        with_table(|base| {
            let entry = unsafe { find(base, namespace_pid) }.ok_or(())?;
            Ok(unsafe { load32(entry, SLOT_PDEATHSIG) })
        })
        .ok_or(())?
    }

    /// Atomically notices one parent death and claims its configured signal.
    ///
    /// Polling callers cannot deliver the signal twice: the delivered bit is
    /// committed under the same cross-process mutex as the liveness decision.
    pub fn take_parent_death_signal(namespace_pid: u32) -> Result<Option<u32>, ()> {
        with_table(|base| {
            let entry = unsafe { find(base, namespace_pid) }.ok_or(())?;
            let signal = unsafe { load32(entry, SLOT_PDEATHSIG) };
            let ppid = unsafe { load32(entry, SLOT_PPID) };
            if signal == 0 || ppid == 0 || unsafe { load32(entry, SLOT_PDEATH_DELIVERED) } != 0 {
                return Ok(None);
            }

            let mut parent = unsafe { find(base, ppid) };
            // An exec wrapper delegates the Linux process to the replacement
            // Windows process. Parent death concerns that process identity, not
            // the wrapper which is intentionally waiting for it.
            for _ in 0..8 {
                let Some(row) = parent else { break };
                let flags = unsafe { load32(row, SLOT_FLAGS) };
                let delegate = unsafe { load32(row, SLOT_DELEGATE) };
                if flags & FLAG_PROXY == 0 || delegate == 0 {
                    break;
                }
                parent = unsafe { find(base, delegate) };
            }
            let parent_alive = parent
                .map(|row| unsafe { load32(row, SLOT_PID) })
                .is_some_and(alive);
            if parent_alive {
                return Ok(None);
            }
            unsafe { store32(entry, SLOT_PDEATH_DELIVERED, 1) };
            Ok(Some(signal))
        })
        .ok_or(())?
    }

    /// Returns the soft and hard `RLIMIT_NOFILE` values for one process.
    pub fn nofile_limits(namespace_pid: u32) -> Result<(u64, u64), ()> {
        with_table(|base| {
            let entry = unsafe { find(base, namespace_pid) }.ok_or(())?;
            let soft = unsafe { load64(entry, SLOT_NOFILE_SOFT) };
            let hard = unsafe { load64(entry, SLOT_NOFILE_HARD) };
            if soft > hard {
                return Err(());
            }
            Ok((soft, hard))
        })
        .ok_or(())?
    }

    /// Atomically updates `RLIMIT_NOFILE` for a Linux-visible process.
    pub fn set_nofile_limits(
        namespace_pid: u32,
        soft: u64,
        hard: u64,
        may_raise: bool,
    ) -> Result<(), i32> {
        with_table(|base| {
            let entry = unsafe { find(base, namespace_pid) }.ok_or(3)?;
            if soft > hard {
                return Err(22);
            }
            // Check the current hard limit and publish both words under the
            // same lock; concurrent prlimit calls cannot bypass a reduction.
            if hard > unsafe { load64(entry, SLOT_NOFILE_HARD) } && !may_raise {
                return Err(1);
            }
            unsafe {
                store64(entry, SLOT_NOFILE_SOFT, soft);
                store64(entry, SLOT_NOFILE_HARD, hard);
            }
            Ok(())
        })
        .ok_or(5)?
    }

    pub fn file_process_limits(
        pid: u32,
        resource: u32,
        value: Option<(u64, u64)>,
    ) -> Result<(u64, u64), ()> {
        let offset = match resource {
            1 => SLOT_FSIZE_SOFT,
            6 => SLOT_NPROC_SOFT,
            _ => return Err(()),
        };
        with_table(|base| {
            let entry = unsafe { find(base, pid) }.ok_or(())?;
            let old = unsafe { (load64(entry, offset), load64(entry, offset + 8)) };
            if let Some((soft, hard)) = value {
                if soft > hard {
                    return Err(());
                }
                unsafe {
                    store64(entry, offset, soft);
                    store64(entry, offset + 8, hard);
                }
            }
            Ok(old)
        })
        .ok_or(())?
    }

    pub fn mqueue_limits(pid: u32, value: Option<(u64, u64)>) -> Result<(u64, u64), ()> {
        with_table(|base| {
            let entry = unsafe { find(base, pid) }.ok_or(())?;
            let old = unsafe {
                (
                    load64(entry, SLOT_MQUEUE_SOFT),
                    load64(entry, SLOT_MQUEUE_HARD),
                )
            };
            if let Some((soft, hard)) = value {
                unsafe {
                    store64(entry, SLOT_MQUEUE_SOFT, soft);
                    store64(entry, SLOT_MQUEUE_HARD, hard);
                }
            }
            Ok(old)
        })
        .ok_or(())?
    }

    /// Reads the parent command for this host process's active fork.
    pub fn fork_parent_override() -> Result<Option<u32>, ()> {
        with_table(|base| {
            let entry = unsafe { find_host(base, current_host_pid()) }.ok_or(())?;
            let parent = unsafe { load32(entry, SLOT_FORK_PARENT) };
            Ok((parent != 0).then_some(parent))
        })
        .ok_or(())?
    }

    /// Publishes or clears the parent command for this host process's fork.
    pub fn set_fork_parent_override(host_pid: u32, parent: Option<u32>) -> Result<(), ()> {
        with_table(|base| {
            let entry = unsafe { find_host(base, host_pid) }.ok_or(())?;
            unsafe { store32(entry, SLOT_FORK_PARENT, parent.unwrap_or(0)) };
            Ok(())
        })
        .ok_or(())?
    }

    /// Reads the process row and its Linux-facing metadata atomically.
    pub fn process_info(namespace_pid: u32) -> Option<ProcessInfo> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            let entry = unsafe { find(base, namespace_pid) }?;
            // SAFETY: the fixed metadata ranges belong to this slot.
            unsafe { Some(read_process_info(entry)) }
        })
        .flatten()
    }

    /// All live rows with their Linux-facing metadata.
    pub fn all_process_info() -> Vec<ProcessInfo> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            unsafe { sweep(base) };
            let mut found = Vec::new();
            for index in 0..CAPACITY {
                let entry = unsafe { slot(base, index) };
                let record = unsafe { read_slot(entry) };
                if record.pid != 0 && record.flags & FLAG_ZOMBIE == 0 {
                    found.push(unsafe { read_process_info(entry) });
                }
            }
            found
        })
        .unwrap_or_default()
    }

    /// # Safety
    /// `entry` must address a slot and the destination range must fit it.
    unsafe fn write_blob(
        entry: *mut u8,
        offset: usize,
        capacity: usize,
        length_offset: usize,
        bytes: &[u8],
    ) {
        let length = bytes.len().min(capacity);
        unsafe {
            core::ptr::write_bytes(entry.add(offset), 0, capacity);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), entry.add(offset), length);
            store32(entry, length_offset, length as u32);
        }
    }

    /// # Safety
    /// `entry` must address a complete slot.
    unsafe fn read_process_info(entry: *mut u8) -> ProcessInfo {
        let comm_len = unsafe { load32(entry, SLOT_COMM_LENGTH) as usize }.min(COMM_CAPACITY);
        let exe_len = unsafe { load32(entry, SLOT_EXE_LENGTH) as usize }.min(EXE_CAPACITY);
        let cmdline_len =
            unsafe { load32(entry, SLOT_CMDLINE_LENGTH) as usize }.min(CMDLINE_CAPACITY);
        let comm = unsafe { core::slice::from_raw_parts(entry.add(SLOT_COMM), comm_len) };
        let executable = unsafe { core::slice::from_raw_parts(entry.add(SLOT_EXE), exe_len) };
        let cmdline = unsafe { core::slice::from_raw_parts(entry.add(SLOT_CMDLINE), cmdline_len) };
        ProcessInfo {
            entry: unsafe { read_slot(entry) },
            comm: String::from_utf8_lossy(comm).into_owned(),
            executable: String::from_utf8_lossy(executable).into_owned(),
            cmdline: cmdline.to_vec(),
        }
    }

    /// Rebinds a guest process identity to the Windows process running its new
    /// image. Linux `execve` preserves pid; the old loader remains only as a
    /// wait wrapper and no longer owns a second namespace entry.
    pub fn replace_host(pid: u32, host_pid: u32) -> bool {
        if pid == 0 || host_pid == 0 {
            return false;
        }
        let token = start_token(host_pid);
        if token == 0 {
            return false;
        }
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            let Some(entry) = (unsafe { find(base, pid) }) else {
                return false;
            };
            unsafe { notifications::replacing(base, entry) };
            // The new loader is allowed to reach `ensure_registered` before
            // its old exec wrapper completes this handoff. In that race it has
            // a short-lived namespace row of its own. Linux exec has only one
            // process identity, so discard that provisional row before moving
            // the original namespace pid onto the replacement's real pid.
            if let Some(provisional) = unsafe { find_host(base, host_pid) }
                && provisional != entry
            {
                // SAFETY: both rows are in this mapping and the mutex is held.
                unsafe { clear_slot(base, provisional) };
            }
            // SAFETY: the slot is inside the section.
            unsafe {
                let flags = load32(entry, SLOT_FLAGS);
                store32(entry, SLOT_FLAGS, (flags | FLAG_EXECED) & !FLAG_PROXY);
                store32(entry, SLOT_DELEGATE, 0);
                store64(entry, SLOT_TOKEN, token);
                // The slot remains keyed by the real process id; exec simply
                // transfers ownership to the replacement's real pid.
                store32(entry, SLOT_PID, host_pid);
                rebuild_indexes(base);
            }
            true
        })
        .unwrap_or(false)
    }

    /// Restores an exec caller after its candidate failed before activation.
    /// A dead candidate may already have published a zombie report; none of
    /// that report or candidate image metadata belongs to the retained caller.
    pub fn rollback_exec_owner(previous: &ProcessInfo) -> bool {
        let original = previous.entry;
        let token = start_token(original.pid);
        if token == 0 {
            return false;
        }
        let exists = with_table(|base| unsafe { find(base, original.namespace_pid).is_some() })
            .unwrap_or(false);
        if !exists
            && register(
                original.pid,
                original.pgid,
                original.sid,
                original.ppid,
                original.flags,
            )
            .is_none()
        {
            return false;
        }
        let restored = with_table(|base| unsafe {
            let Some(entry) = find(base, original.namespace_pid) else {
                return false;
            };
            notifications::replacing(base, entry);
            store32(entry, SLOT_PID, original.pid);
            store64(entry, SLOT_TOKEN, token);
            store64(entry, SLOT_START_TICKS, original.start_ticks);
            store32(entry, SLOT_FLAGS, original.flags);
            store32(entry, SLOT_DELEGATE, original.delegate);
            store32(entry, SLOT_STATE, original.state);
            store32(entry, SLOT_STOP_SIGNAL, original.stop_signal);
            store32(entry, SLOT_REPORT, REPORT_NONE);
            store32(entry, SLOT_REPORT_SIGNAL, 0);
            rebuild_indexes(base);
            true
        })
        .unwrap_or(false);
        restored
            && set_process_identity(
                original.namespace_pid,
                &previous.comm,
                &previous.executable,
                &previous.cmdline,
            )
    }

    /// Follows a proxy's delegate link to the process that owns the signals.
    ///
    /// An `exec` leaves two live processes: the wrapper the parent is waiting
    /// on, and the fresh image doing the work. Only the second can run a guest
    /// handler, so anything aimed at the first has to arrive at the second.
    pub fn resolve(pid: u32) -> Option<Entry> {
        let mut current = lookup(pid)?;
        // The chain is bounded because each exec adds one link; the limit is
        // there so a corrupted table cannot spin forever.
        for _ in 0..8 {
            if current.flags & FLAG_PROXY == 0 || current.delegate == 0 {
                return Some(current);
            }
            match lookup(current.delegate) {
                Some(next) => current = next,
                None => return Some(current),
            }
        }
        Some(current)
    }

    /// Every live member of process group `pgid`.
    ///
    /// Proxies are omitted when their delegate is registered, so a group signal
    /// reaches an `exec`ed program exactly once rather than twice.
    pub fn members(pgid: u32) -> Vec<Entry> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            unsafe { sweep(base) };
            let mut found = Vec::new();
            let mut delegates = Vec::new();
            for index in 0..CAPACITY {
                // SAFETY: the index is in range.
                let entry = unsafe { slot(base, index) };
                // SAFETY: the slot is inside the section.
                let record = unsafe { read_slot(entry) };
                if record.pid == 0 || record.pgid != pgid || record.flags & FLAG_ZOMBIE != 0 {
                    continue;
                }
                if record.flags & FLAG_PROXY != 0 && record.delegate != 0 {
                    delegates.push(record.delegate);
                }
                found.push(record);
            }
            // A proxy whose delegate is in the same group is dropped; one whose
            // delegate has not registered yet is kept, so that a signal sent in
            // the gap between spawn and registration is still recorded.
            found.retain(|record| {
                record.flags & FLAG_PROXY == 0
                    || record.delegate == 0
                    || !delegates.contains(&record.delegate)
            });
            found
        })
        .unwrap_or_default()
    }

    /// Every live process in the registry.
    ///
    /// This is the population `kill(-1, sig)` reaches. It is not every process
    /// on the machine, which is what Linux would mean: a Windows process that
    /// was never hosted here has no signal dispositions to run.
    pub fn everyone() -> Vec<Entry> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            unsafe { sweep(base) };
            let mut found = Vec::new();
            for index in 0..CAPACITY {
                // SAFETY: the index is in range.
                let entry = unsafe { slot(base, index) };
                // SAFETY: the slot is inside the section.
                let record = unsafe { read_slot(entry) };
                if record.pid != 0 && record.flags & FLAG_ZOMBIE == 0 {
                    found.push(record);
                }
            }
            found
        })
        .unwrap_or_default()
    }

    /// Claims a slot for a child this process just forked.
    ///
    /// The child registers itself as soon as it resumes, but a shell calls
    /// `setpgid(child, child)` in the parent immediately after `fork` returns
    /// and must not be told the child does not exist yet. Registering from the
    /// parent closes that window; the child then finds the slot already correct
    /// and adopts it instead of claiming a fresh one.
    pub fn register_forked_child(child_host_pid: u32) -> Option<u32> {
        register_forked_child_with_parent(child_host_pid, None)
    }

    /// Claims a slot for a fork child with an optional explicit Linux parent.
    pub fn register_forked_child_with_parent(
        child_host_pid: u32,
        parent: Option<u32>,
    ) -> Option<u32> {
        // Fork may not manufacture a process identity from the Windows pid.
        // The caller must already own a Linux namespace row; otherwise there
        // is no exact parent/group/session identity for the child to inherit.
        let cached_host_pid = current_host_pid();
        let Some(own_entry) = lookup_host(cached_host_pid) else {
            if super::fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: parent process row missing cached_host={} actual_host={}",
                    cached_host_pid,
                    unsafe { GetCurrentProcessId() },
                );
            }
            return None;
        };
        let own = own_entry.namespace_pid;
        let (pgid, sid) = (own_entry.pgid, own_entry.sid);
        let Some(child) = register_scoped(
            child_host_pid,
            pgid,
            sid,
            parent.unwrap_or(own),
            0,
            own,
            Registration::Update,
        ) else {
            if super::fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: process-row registration failed parent={own} child_host={child_host_pid}"
                );
            }
            return None;
        };
        let Ok((soft, hard)) = nofile_limits(own) else {
            release_slot(child.namespace_pid);
            return None;
        };
        if set_nofile_limits(child.namespace_pid, soft, hard, true).is_err() {
            release_slot(child.namespace_pid);
            return None;
        }
        if mqueue_limits(own, None)
            .and_then(|limit| mqueue_limits(child.namespace_pid, Some(limit)))
            .is_err()
        {
            release_slot(child.namespace_pid);
            return None;
        }
        for resource in [1, 6] {
            if file_process_limits(own, resource, None)
                .and_then(|limit| file_process_limits(child.namespace_pid, resource, Some(limit)))
                .is_err()
            {
                release_slot(child.namespace_pid);
                return None;
            }
        }
        if !inherit_oom_adjustment(own, child.namespace_pid) {
            release_slot(child.namespace_pid);
            return None;
        }
        if let Err(error) = inherit_fd_links(own, child.namespace_pid) {
            if super::fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: fd-link inheritance failed parent={own} child={} error={error:?}",
                    child.namespace_pid
                );
            }
            release_slot(child.namespace_pid);
            return None;
        }
        if let Some(parent) = process_info(own) {
            set_process_identity(
                child.namespace_pid,
                &parent.comm,
                &parent.executable,
                &parent.cmdline,
            );
        }
        if let Some(path) = cgroup_path(own)
            && !set_cgroup_path(child.namespace_pid, &path)
        {
            release_slot(child.namespace_pid);
            return None;
        }
        Some(child.namespace_pid)
    }

    /// Publishes one completion notification per child after kernel teardown.
    /// A zombie report can be written while its host process is still alive.
    /// Sending SIGCHLD then would let a WNOHANG reaper observe no waitable child
    /// and sleep forever. Preserve the exact report and notify only when the
    /// process identified by its PID and creation token has actually exited.
    ///
    /// Exec can rebind the row before its original wrapper has finished closing
    /// inherited descriptors. Gate the notification on both process objects,
    /// matching wait(2), and never take the coordinator lock under the PID lock.
    pub fn reap_dead_children(ppid: u32) -> Result<bool, ()> {
        let mut found = false;
        for child in children_of(ppid)? {
            if child.flags & FLAG_EXIT_NOTIFIED != 0 {
                continue;
            }
            let handles = notifications::completion_sources(child)?;
            let mut completed = true;
            for handle in &handles {
                completed &= notifications::completed(handle)?;
            }
            if !completed {
                continue;
            }
            let published = with_table(|base| {
                let Some(entry) = (unsafe { find(base, child.namespace_pid) }) else {
                    return false;
                };
                let current = unsafe { read_slot(entry) };
                // A replacement or adoption during handle acquisition gets a
                // new topology event and must be observed with its new identity.
                if current.pid != child.pid
                    || current.token != child.token
                    || current.ppid != ppid
                    || current.flags & FLAG_EXIT_NOTIFIED != 0
                {
                    return false;
                }
                unsafe {
                    if current.flags & FLAG_ZOMBIE == 0 {
                        namespaces::exiting(base, entry);
                        // No exact Linux status was published before death.
                        store32(entry, SLOT_REPORT_SIGNAL, 0);
                        store32(entry, SLOT_REPORT, REPORT_UNOBSERVABLE);
                    }
                    store32(
                        entry,
                        SLOT_FLAGS,
                        current.flags | FLAG_ZOMBIE | FLAG_EXIT_NOTIFIED,
                    );
                }
                true
            })
            .ok_or(())?;
            found |= published;
        }
        Ok(found)
    }

    /// Sets a process's group.
    pub fn set_group(pid: u32, pgid: u32) -> bool {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            match unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                Some(entry) => unsafe {
                    store32(entry, SLOT_PGID, pgid);
                    true
                },
                None => false,
            }
        })
        .unwrap_or(false)
    }

    /// Sets a process's session and group together, as `setsid` requires.
    pub fn set_session(pid: u32, sid: u32, pgid: u32) -> bool {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            match unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                Some(entry) => unsafe {
                    store32(entry, SLOT_SID, sid);
                    store32(entry, SLOT_PGID, pgid);
                    true
                },
                None => false,
            }
        })
        .unwrap_or(false)
    }

    /// Adds and removes flag bits in one update.
    pub fn update_flags(pid: u32, set: u32, clear: u32) {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            if let Some(entry) = unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                unsafe {
                    let flags = load32(entry, SLOT_FLAGS);
                    store32(entry, SLOT_FLAGS, (flags & !clear) | set);
                }
            }
        });
    }

    /// Marks `pid` as a wrapper whose signals belong to `delegate`.
    pub fn set_delegate(pid: u32, delegate: u32) {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            if let Some(entry) = unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                unsafe {
                    store32(entry, SLOT_DELEGATE, delegate);
                    let flags = load32(entry, SLOT_FLAGS);
                    store32(entry, SLOT_FLAGS, flags | FLAG_PROXY | FLAG_EXECED);
                }
            }
        });
    }

    /// Records the stopped or running state a process has reached.
    pub fn set_state(pid: u32, state: u32, stop_signal: u32) {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            if let Some(entry) = unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                unsafe {
                    store32(entry, SLOT_STOP_SIGNAL, stop_signal);
                    store32(entry, SLOT_STATE, state);
                }
            }
        });
    }

    /// Reads just the running/stopped state, without the rest of the entry.
    ///
    /// This one deliberately skips the table mutex. A stopped process polls its
    /// own state while it has other threads suspended, and one of those threads
    /// could be holding the mutex — taking it here would turn a stop into a
    /// deadlock. Reading a single aligned word without the mutex is safe: the
    /// scan keys on the pid word, which a registration publishes last.
    pub fn state(pid: u32) -> u32 {
        let Some(base) = map() else {
            return STATE_RUNNING;
        };
        // SAFETY: the section stays mapped for the life of the process.
        match unsafe { find_stable(base, pid) } {
            // SAFETY: the slot is inside the section.
            Some(entry) => unsafe { load32(entry, SLOT_STATE) },
            None => STATE_RUNNING,
        }
    }

    /// Leaves a state transition for the process's parent to collect.
    ///
    /// Only one report is held at a time. Linux queues them per child; here a
    /// stop immediately followed by a continue overwrites the stop, so a parent
    /// polling slowly with `WUNTRACED` can miss the intermediate state. The
    /// alternative — a per-child queue in a fixed-size shared section — would
    /// trade a rare missed notification for a hard bound on how far behind a
    /// parent may fall, which is worse.
    pub fn post_report(pid: u32, change: StateChange) {
        publish_report(pid, change, false);
    }

    /// Publishes the stop report before the stopped state becomes observable.
    /// This must run before suspending threads that could hold the table mutex.
    pub fn post_stop(pid: u32, signal: u32) {
        publish_report(pid, StateChange::Stopped(signal), true);
    }

    fn publish_report(pid: u32, change: StateChange, stopped: bool) {
        let (kind, signal) = match change {
            StateChange::Stopped(signal) => (REPORT_STOPPED, signal),
            StateChange::Continued => (REPORT_CONTINUED, 0),
            StateChange::Killed(signal) => (REPORT_KILLED, signal),
            StateChange::Exited(status) => (REPORT_EXITED, status & 0xff),
            StateChange::Unobservable => (REPORT_UNOBSERVABLE, 0),
        };
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            if let Some(entry) = unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                unsafe {
                    store32(entry, SLOT_REPORT_SIGNAL, signal);
                    store32(entry, SLOT_REPORT, kind);
                    if stopped {
                        store32(entry, SLOT_STOP_SIGNAL, signal);
                        store32(entry, SLOT_STATE, STATE_STOPPED);
                    }
                    if matches!(kind, REPORT_KILLED | REPORT_EXITED | REPORT_UNOBSERVABLE) {
                        namespaces::exiting(base, entry);
                        let flags = load32(entry, SLOT_FLAGS);
                        store32(entry, SLOT_FLAGS, flags | FLAG_ZOMBIE);
                    }
                }
            }
        });
    }

    /// Reads a pending state transition without consuming it.
    pub fn peek_report(pid: u32) -> Option<StateChange> {
        report(pid, false)
    }

    /// Consumes a pending state transition.
    pub fn take_report(pid: u32) -> Option<StateChange> {
        report(pid, true)
    }

    fn report(pid: u32, consume: bool) -> Option<StateChange> {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            let entry = unsafe { find(base, pid) }?;
            // SAFETY: the slot is inside the section.
            let (kind, signal) = unsafe {
                (
                    load32(entry, SLOT_REPORT),
                    load32(entry, SLOT_REPORT_SIGNAL),
                )
            };
            let change = match kind {
                REPORT_STOPPED => StateChange::Stopped(signal),
                REPORT_CONTINUED => StateChange::Continued,
                REPORT_KILLED => StateChange::Killed(signal),
                REPORT_EXITED => StateChange::Exited(signal),
                REPORT_UNOBSERVABLE => StateChange::Unobservable,
                _ => return None,
            };
            if consume {
                // SAFETY: the slot is inside the section.
                unsafe { store32(entry, SLOT_REPORT, REPORT_NONE) };
            }
            Some(change)
        })
        .flatten()
    }

    /// Adds `bit` to a process's pending mask, reporting whether it landed.
    ///
    /// The mask is ORed rather than queued, which is exactly how a kernel treats
    /// standard signals: two `SIGINT`s delivered before either is handled are
    /// one `SIGINT`. Real-time signals do queue on Linux; they do not here.
    pub fn post_pending(pid: u32, bit: u64) -> bool {
        with_table(|base| {
            // SAFETY: the section is mapped and the mutex is held.
            match unsafe { find(base, pid) } {
                // SAFETY: the slot is inside the section.
                Some(entry) => unsafe {
                    if !notifications::signal(entry) {
                        return false;
                    }
                    or64(entry, SLOT_PENDING, bit);
                    true
                },
                None => false,
            }
        })
        .unwrap_or(false)
    }

    /// Removes and returns a process's pending mask.
    pub fn take_pending(pid: u32) -> u64 {
        // The mask is swapped without the table mutex. It is a single aligned
        // word and the slot cannot be recycled while its own process is running,
        // which is the only process that calls this.
        let Some(base) = map() else { return 0 };
        // SAFETY: the section is mapped for the life of the process.
        let Some(entry) = (unsafe { find(base, pid) }) else {
            return 0;
        };
        // SAFETY: the slot is inside the section.
        unsafe { swap64(entry, SLOT_PENDING, 0) }
    }

    /// The process group the host console currently treats as foreground.
    ///
    /// Zero means "not established", which the console handler reads as "send
    /// to the receiving process's own group".
    pub fn foreground_pgid() -> u32 {
        let Some(base) = map() else { return 0 };
        // SAFETY: the header is inside the mapped section.
        unsafe { load32(base, HEADER_FOREGROUND) }
    }

    /// Records the console's foreground process group.
    pub fn set_foreground_pgid(pgid: u32) {
        let Some(base) = map() else { return };
        // SAFETY: the header is inside the mapped section.
        unsafe { store32(base, HEADER_FOREGROUND, pgid) };
    }

    /// Drops this process's cached view so the next use re-opens the section.
    ///
    /// A forked child inherits the parent's *arena*, which is where these
    /// statics live, but not the parent's mapped view — the fork backend copies
    /// the arena and the stack and nothing else. The cached base pointer is
    /// therefore an address that is not mapped in the child, and using it would
    /// fault. The child calls this before registering itself.
    pub fn reset_after_fork() {
        // The stale handles belong to the parent process and are not valid
        // here, so they are dropped rather than closed.
        BASE.store(0, Ordering::Release);
        SECTION.store(0, Ordering::Release);
        GUARD.store(0, Ordering::Release);
        // Unlike the guest pid, the Windows pid changes across fork. Rebind it
        // while the child is still single-threaded so the hot path remains a
        // plain load with no lazy cache state.
        bind_host_pid();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn fresh_pid_allocator_starts_at_one_and_skips_occupied_ids() {
            // A private table tests initial allocation without resetting the
            // session-wide namespace used by running applications.
            let mut storage = vec![0u64; SECTION_SIZE.div_ceil(8)];
            let base = storage.as_mut_ptr().cast::<u8>();
            unsafe {
                store32(base, HEADER_NEXT_PID, FIRST_NAMESPACE_PID);
                assert_eq!(allocate_pid(base), Some(1));
                assert_eq!(allocate_pid(base), Some(2));
                let occupied = slot(base, 0);
                store32(occupied, SLOT_PID, 12345);
                store32(occupied, SLOT_NAMESPACE_PID, 3);
                rebuild_indexes(base);
                assert_eq!(allocate_pid(base), Some(4));
                assert_eq!(load32(base, HEADER_NEXT_PID), 5);
            }
        }

        #[test]
        fn stopped_and_pending_rows_remain_visible_while_indexes_are_empty() {
            let mut storage = vec![0u64; SECTION_SIZE.div_ceil(8)];
            let base = storage.as_mut_ptr().cast::<u8>();
            unsafe {
                let row = slot(base, 7);
                store32(row, SLOT_NAMESPACE_PID, 123);
                store32(row, SLOT_PID, 456);
                store32(row, SLOT_STATE, STATE_STOPPED);
                store64(row, SLOT_PENDING, 1 << 18);
                rebuild_indexes(base);
                assert_eq!(find_stable(base, 123), Some(row));
                for bucket in 0..INDEX_CAPACITY {
                    store32(base, NAMESPACE_INDEX_OFFSET + bucket * INDEX_ENTRY_SIZE, 0);
                }
                assert!(find(base, 123).is_none());
                assert_eq!(
                    load32(find_stable(base, 123).unwrap(), SLOT_STATE),
                    STATE_STOPPED
                );
                // A cold reader must also work during the same rebuild window.
                let address = base as usize;
                std::thread::spawn(move || {
                    let row = find_stable(address as *mut u8, 123).unwrap();
                    assert_eq!(load32(row, SLOT_STATE), STATE_STOPPED);
                    assert_eq!(load64(row, SLOT_PENDING), 1 << 18);
                })
                .join()
                .unwrap();
                store32(row, SLOT_NAMESPACE_PID, 0);
                let moved = slot(base, 9);
                store32(moved, SLOT_NAMESPACE_PID, 123);
                assert_eq!(find_stable(base, 123), Some(moved));
            }
        }

        /// Serializes tests against the one shared section.
        fn serialized() -> std::sync::MutexGuard<'static, ()> {
            static LOCK: Mutex<()> = Mutex::new(());
            LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }

        /// A real, live process to stand in for a second hosted process.
        ///
        /// A made-up pid will not do. Every structural change sweeps the table
        /// for slots whose owner has gone, so a slot registered against a number
        /// nothing is running under is reclaimed by the very next call — which
        /// is the behaviour under test elsewhere and would silently invalidate
        /// these. `cmd.exe` with its input held open blocks reading and stays
        /// alive until it is killed.
        struct LiveProcess(std::process::Child);

        impl LiveProcess {
            fn spawn() -> Self {
                use std::os::windows::process::CommandExt;
                let child = std::process::Command::new("cmd.exe")
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("a helper process");
                Self(child)
            }

            fn pid(&self) -> u32 {
                self.0.id()
            }
        }

        impl Drop for LiveProcess {
            fn drop(&mut self) {
                if let Some(entry) = lookup_host(self.0.id()) {
                    release_slot(entry.namespace_pid);
                }
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        #[test]
        fn claiming_an_adopted_process_preserves_its_live_parent_and_flags() {
            let _serialized = serialized();
            let host = current_host_pid();
            let saved = lookup_host(host);
            let own = register(host, 0, 0, 0, FLAG_SUBREAPER)
                .unwrap()
                .namespace_pid;
            let mut parent = LiveProcess::spawn();
            let parent_pid = register(parent.pid(), own, own, own, 0)
                .unwrap()
                .namespace_pid;
            let child = LiveProcess::spawn();
            let child_pid = register(
                child.pid(),
                own,
                own,
                parent_pid,
                FLAG_EXECED | FLAG_SUBREAPER,
            )
            .unwrap()
            .namespace_pid;
            parent.0.kill().unwrap();
            parent.0.wait().unwrap();
            let adopted = lookup(child_pid).unwrap();
            assert_eq!(adopted.ppid, own);
            // These stale startup values cannot undo adoption or exec flags.
            assert_eq!(claim(child.pid(), 90, 91, 0), Some(adopted));
            assert_eq!(lookup(child_pid), Some(adopted));
            drop(child);
            drop(parent);
            release_slot(own);
            if let Some(saved) = saved {
                register(host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn a_slot_round_trips_a_group_and_session() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let saved = lookup_host(own_host);

            let reg = register(own_host, 100, 200, 300, 0).expect("registered");
            let own = reg.namespace_pid;
            let entry = lookup(own).expect("the slot just registered");
            assert_eq!(entry.pid, own_host);
            assert_eq!(entry.namespace_pid, own);
            assert_eq!(entry.pgid, 100);
            assert_eq!(entry.sid, 200);
            assert_eq!(entry.ppid, 300);

            // Re-registering an existing pid updates it rather than consuming a
            // second slot, which is what the exec handoff depends on.
            assert!(register(own_host, 101, 200, 300, 0).is_some());
            assert_eq!(lookup(own).unwrap().pgid, 101);

            set_group(own, 102);
            assert_eq!(lookup(own).unwrap().pgid, 102);
            set_session(own, 103, 103);
            let entry = lookup(own).unwrap();
            assert_eq!((entry.sid, entry.pgid), (103, 103));
            assert!(entry.is_group_leader() == (entry.namespace_pid == entry.pgid));
            assert!(entry.is_session_leader() == (entry.namespace_pid == entry.sid));

            release_slot(own);
            assert_eq!(lookup(own), None);
            if let Some(saved) = saved {
                register(own_host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn fork_parent_command_round_trips_through_the_shared_process_row() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let created = if lookup_host(own_host).is_none() {
                let entry = register(own_host, 0, 0, 0, 0).expect("current process row");
                Some(entry.namespace_pid)
            } else {
                None
            };

            set_fork_parent_override(own_host, Some(4242)).expect("publish fork command");
            assert_eq!(fork_parent_override(), Ok(Some(4242)));
            set_fork_parent_override(own_host, None).expect("clear fork command");
            assert_eq!(fork_parent_override(), Ok(None));

            if let Some(pid) = created {
                release_slot(pid);
            }
        }

        #[test]
        fn pid_indexes_and_exec_rebinding_are_bidirectional() {
            let _serialized = serialized();
            let original = LiveProcess::spawn();
            let replacement = LiveProcess::spawn();

            let guest_pid = register(original.pid(), 0, 0, 0, 0)
                .expect("the original helper must get a row")
                .namespace_pid;
            let linux_birth = lookup(guest_pid).unwrap().start_ticks;
            assert_ne!(linux_birth, 0);
            assert_eq!(host_pid(guest_pid), Some(original.pid()));
            assert_eq!(namespace_pid(original.pid()), Some(guest_pid));

            let cmdline = b"wget\0https://example.test/\0";
            assert!(set_process_identity(
                guest_pid,
                "wget",
                "/bin/busybox",
                cmdline,
            ));

            // Model the race where the replacement loader registers itself
            // before its old exec wrapper performs the identity handoff.
            let provisional_pid = register(replacement.pid(), 0, 0, 0, 0)
                .expect("the replacement helper must get a provisional row")
                .namespace_pid;
            assert_ne!(provisional_pid, guest_pid);

            assert!(replace_host(guest_pid, replacement.pid()));
            assert_eq!(host_pid(guest_pid), Some(replacement.pid()));
            assert_eq!(namespace_pid(replacement.pid()), Some(guest_pid));
            assert_eq!(namespace_pid(original.pid()), None);
            assert_eq!(lookup(provisional_pid), None);

            let info = process_info(guest_pid).expect("metadata survives exec rebinding");
            assert_eq!(info.entry.pid, replacement.pid());
            assert_eq!(info.entry.namespace_pid, guest_pid);
            assert_eq!(info.entry.start_ticks, linux_birth);
            assert_eq!(info.comm, "wget");
            assert_eq!(info.executable, "/bin/busybox");
            assert_eq!(info.cmdline, cmdline);
        }

        #[test]
        fn proc_fd_links_round_trip_and_inherit_without_handle_guessing() {
            let _serialized = serialized();
            let parent = LiveProcess::spawn();
            let child = LiveProcess::spawn();
            let parent_pid = register(parent.pid(), 0, 0, 0, 0)
                .expect("parent row")
                .namespace_pid;
            let child_pid = register(child.pid(), 0, 0, parent_pid, 0)
                .expect("child row")
                .namespace_pid;

            let links = vec![
                (0, String::from("pipe:[12345]")),
                (7, String::from("/var/lib/container/state.json")),
            ];
            replace_fd_links(parent_pid, &links).expect("publish parent fd table");
            assert_eq!(fd_links(parent_pid).unwrap(), vec![0, 7]);
            assert_eq!(
                fd_link(parent_pid, 7).unwrap().as_deref(),
                Some("/var/lib/container/state.json")
            );
            assert_eq!(fd_link(parent_pid, 8).unwrap(), None);

            inherit_fd_links(parent_pid, child_pid).expect("inherit fd table");
            assert_eq!(fd_links(child_pid).unwrap(), vec![0, 7]);
            assert_eq!(
                fd_link(child_pid, 0).unwrap().as_deref(),
                Some("pipe:[12345]")
            );

            replace_fd_links(parent_pid, &[(3, String::from("socket:[9]"))])
                .expect("replace atomically");
            assert_eq!(fd_links(parent_pid).unwrap(), vec![3]);
            assert_eq!(fd_link(parent_pid, 0).unwrap(), None);
            // The fork copy is independent metadata and remains unchanged.
            assert_eq!(fd_links(child_pid).unwrap(), vec![0, 7]);
        }

        #[test]
        fn a_pending_mask_is_posted_by_one_reader_and_taken_by_another() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let saved = lookup_host(own_host);
            let entry = register(own_host, 1, 1, 0, 0).expect("registered");
            let own = entry.namespace_pid;

            assert!(post_pending(own, 1 << 1));
            assert!(post_pending(own, 1 << 8));
            // Standard signals coalesce: posting the same bit twice is one
            // signal, exactly as a kernel treats them.
            assert!(post_pending(own, 1 << 1));
            assert_eq!(take_pending(own), (1 << 1) | (1 << 8));
            // Taking is destructive, so a second drain finds nothing.
            assert_eq!(take_pending(own), 0);
            // A pid with no slot has nowhere to put a signal.
            assert!(!post_pending(0x7fff_0000, 1));

            release_slot(own);
            if let Some(saved) = saved {
                register(own_host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn a_state_report_is_held_until_a_parent_collects_it() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let saved = lookup_host(own_host);
            let parent = register(own_host, 1, 1, 0, 0).expect("parent registered");
            let helper = LiveProcess::spawn();
            let child =
                register(helper.pid(), 1, 1, parent.namespace_pid, 0).expect("child registered");
            // A zombie without a live Linux parent is collected by the orphan
            // sweep. Test a real parent/child pair, not the test runner claiming
            // to have exited with ppid=0.
            let own = child.namespace_pid;

            assert_eq!(peek_report(own), None);
            post_report(own, StateChange::Continued);
            post_stop(own, 20);
            assert_eq!(state(own), STATE_STOPPED);
            // Peeking leaves it, so a `waitpid` that decides not to report the
            // stop does not consume it.
            assert_eq!(peek_report(own), Some(StateChange::Stopped(20)));
            assert_eq!(take_report(own), Some(StateChange::Stopped(20)));
            assert_eq!(peek_report(own), None);

            set_state(own, STATE_RUNNING, 0);
            post_report(own, StateChange::Continued);
            assert_eq!(take_report(own), Some(StateChange::Continued));

            // A termination report also marks the slot a zombie, which is what
            // keeps the liveness sweep from discarding it before the parent
            // has read why the child died.
            post_report(own, StateChange::Killed(9));
            assert_ne!(lookup(own).unwrap().flags & FLAG_ZOMBIE, 0);
            assert_eq!(take_report(own), Some(StateChange::Killed(9)));

            release_slot(own);
            release_slot(parent.namespace_pid);
            if let Some(saved) = saved {
                register(own_host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn a_proxy_forwards_its_group_membership_to_its_delegate() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let saved = lookup_host(own_host);
            let helper = LiveProcess::spawn();
            let group = own_host.wrapping_add(0x0010_0000);

            let entry = register(own_host, group, group, 0, 0).expect("registered");
            let own = entry.namespace_pid;
            let helper_pid = register(helper.pid(), group, group, 0, 0)
                .unwrap()
                .namespace_pid;
            set_delegate(own, helper_pid);

            // A proxy resolves to the process that can actually run a handler.
            assert_eq!(resolve(own).unwrap().namespace_pid, helper_pid);
            // And it is dropped from the group listing, so a group signal
            // reaches the replacement once rather than twice.
            let members = members(group);
            assert_eq!(members.len(), 1, "got {members:?}");
            assert_eq!(members[0].namespace_pid, helper_pid);
            // Marking a delegate also records that the image was replaced,
            // which is what makes `setpgid` on it fail with EACCES.
            assert_ne!(lookup(own).unwrap().flags & FLAG_EXECED, 0);

            release_slot(own);
            drop(helper);
            if let Some(saved) = saved {
                register(own_host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn exit_notification_waits_for_kernel_completion_and_preserves_status() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let saved = lookup_host(own_host);
            let own = register(own_host, 1, 1, 0, 0).unwrap().namespace_pid;
            for change in [StateChange::Exited(37), StateChange::Killed(9)] {
                let mut helper = LiveProcess::spawn();
                let child = register(helper.pid(), own, own, own, 0)
                    .unwrap()
                    .namespace_pid;
                post_report(child, change);
                assert!(
                    !reap_dead_children(own).unwrap(),
                    "a status is not kernel completion"
                );
                assert_eq!(peek_report(child), Some(change));
                helper.0.kill().unwrap();
                helper.0.wait().unwrap();
                assert!(
                    reap_dead_children(own).unwrap(),
                    "published zombies must still notify"
                );
                assert_eq!(peek_report(child), Some(change));
                assert!(
                    !reap_dead_children(own).unwrap(),
                    "notification is emitted once"
                );
                release_slot(child);
            }
            release_slot(own);
            if let Some(saved) = saved {
                register(own_host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn an_unreported_dead_child_is_retained_as_an_integrity_error() {
            let _serialized = serialized();
            let own_host = current_host_pid();
            let saved = lookup_host(own_host);
            let entry = register(own_host, 1, 1, 0, 0).expect("registered");
            let own = entry.namespace_pid;

            let mut helper = LiveProcess::spawn();
            let child = register(helper.pid(), own, own, own, 0)
                .unwrap()
                .namespace_pid;
            // Nothing has died yet.
            assert!(!reap_dead_children(own).expect("shared registry"));

            let _ = helper.0.kill();
            let _ = helper.0.wait();
            // The parent notices on its next sweep, which is what becomes a
            // SIGCHLD for an ordinary exit.
            assert!(reap_dead_children(own).expect("shared registry"));
            assert_ne!(lookup(child).unwrap().flags & FLAG_ZOMBIE, 0);
            assert_eq!(peek_report(child), Some(StateChange::Unobservable));
            // And only once: the retained zombie is not rediscovered.
            assert!(!reap_dead_children(own).expect("shared registry"));

            release_slot(child);
            release_slot(own);
            if let Some(saved) = saved {
                register(own_host, saved.pgid, saved.sid, saved.ppid, saved.flags);
            }
        }

        #[test]
        fn native_exit_code_259_is_not_a_live_process() {
            use std::io::Write;
            let _serialized = serialized();
            let mut helper = LiveProcess::spawn();
            helper
                .0
                .stdin
                .take()
                .unwrap()
                .write_all(b"exit 259\r\n")
                .unwrap();
            assert_eq!(helper.0.wait().unwrap().code(), Some(259));
            assert!(!alive(helper.pid()));
        }

        #[test]
        fn liveness_is_answered_for_real_pids_only() {
            let _serialized = serialized();
            // This process is trivially alive; pid 0 is not a process at all.
            assert!(alive(current_host_pid()));
            assert!(!alive(0));
            // A pid nothing is running under reads as dead, which is what lets
            // the sweep reclaim a crashed process's slot.
            assert!(!alive(0x7fff_fff0));
        }

        #[test]
        fn lookup_reclaims_an_orphaned_row_after_its_host_exits() {
            let _serialized = serialized();
            let mut helper = LiveProcess::spawn();
            let host_pid = helper.pid();
            let guest_pid = register(host_pid, 0, 0, 0, 0)
                .expect("live helper gets a process row")
                .namespace_pid;
            assert_eq!(lookup(guest_pid).unwrap().pid, host_pid);
            assert_eq!(lookup_host(host_pid).unwrap().namespace_pid, guest_pid);

            let _ = helper.0.kill();
            let _ = helper.0.wait();

            // An orphan has no Linux parent that could wait for it, so the
            // next namespace observation removes it instead of manufacturing
            // a zombie. Both indexes must be rebuilt by the same sweep.
            assert_eq!(lookup(guest_pid), None);
            assert_eq!(lookup_host(host_pid), None);
        }

        #[test]
        fn both_lookup_indexes_reject_a_recycled_process_identity() {
            let _serialized = serialized();
            let helper = LiveProcess::spawn();
            for by_host in [false, true] {
                let record = register(helper.pid(), 0, 0, 0, 0).unwrap();
                // Model a row left behind for an earlier process with the same
                // Windows PID. Being alive is insufficient without its birth time.
                with_table(|base| unsafe {
                    let entry = find(base, record.namespace_pid).unwrap();
                    store64(entry, SLOT_TOKEN, record.token ^ 1);
                })
                .unwrap();
                let found = if by_host {
                    lookup_host(helper.pid())
                } else {
                    lookup(record.namespace_pid)
                };
                assert_eq!(found, None);
                assert!(
                    alive(helper.pid()),
                    "identity rejection must not kill the new owner"
                );
            }
        }

        #[test]
        fn the_console_foreground_group_is_shared_process_wide() {
            let _serialized = serialized();
            let previous = foreground_pgid();
            set_foreground_pgid(4242);
            assert_eq!(foreground_pgid(), 4242);
            set_foreground_pgid(previous);
            assert_eq!(foreground_pgid(), previous);
        }
    }
}

/// Reaps a child created by the fork coordinator.
///
/// Returns a positive pid, zero for `WNOHANG`, or a negated Linux errno.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_wait(
    pid: i32,
    options: i32,
    events: i32,
    status: *mut i32,
) -> i32 {
    // SAFETY: the caller provides the optional status storage.
    unsafe { windows::wait_child(pid, options, events, status) }
}

/// Gives the process-wide wait coordinator its own reference to a child handle.
///
/// `posix_spawn` runs in a substitute libc DLL, while wait state must live in
/// the executable so every provider observes the same children. The handle is
/// duplicated here, allowing the caller to drop its `Child` immediately.
#[cfg(all(windows, target_arch = "x86_64"))]
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_register_child(pid: u32, process: usize) -> i32 {
    windows::register_child_handle(pid, process)
}

/// Discards a failed fresh-image child after its owned native process has died.
/// No guest PID was returned to the caller, so it must not leave a waitable zombie.
#[cfg(windows)]
pub fn discard_uncommitted_child(pid: u32) {
    if let Ok(mut children) = child_processes().lock() {
        children.remove(pid);
    }
    job::release_slot(pid);
}

/// Pins the coordinator's original child/exec wrapper completion object.
/// The current shared PID row may already name a replacement loader; wait(2)
/// must also observe cleanup of the original wrapper's inherited descriptors.
#[cfg(windows)]
pub(crate) fn duplicate_child_wait_handle(
    pid: u32,
) -> Result<Option<std::os::windows::io::OwnedHandle>, ()> {
    use std::os::windows::io::{FromRawHandle, OwnedHandle};
    let raw = kinakaze_runtime_duplicate_child_wait_handle(pid);
    match raw {
        0 => Ok(None),
        usize::MAX => Err(()),
        _ => Ok(Some(unsafe { OwnedHandle::from_raw_handle(raw as _) })),
    }
}

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_duplicate_child_wait_handle(pid: u32) -> usize {
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let Ok(children) = child_processes().lock() else {
        return usize::MAX;
    };
    let Some(handle) = children.get(pid) else {
        return 0;
    };
    let mut duplicate = core::ptr::null_mut();
    let process = unsafe { GetCurrentProcess() };
    if unsafe {
        DuplicateHandle(
            process,
            handle as _,
            process,
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return usize::MAX;
    }
    duplicate as usize
}

/// Returns the process-wide manual-reset event signalled by exact Windows child
/// completion. Provider DLLs forward to the executable so every module waits on
/// the same child registry rather than a private statically-linked copy.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "system" fn kinakaze_runtime_child_activity_event() -> usize {
    local_child_activity_event()
}

#[cfg(windows)]
pub fn child_activity_event() -> usize {
    // SAFETY: the resolved process export has the same no-argument ABI and the
    // hosting executable remains loaded for the process lifetime.
    unsafe {
        (*PROCESS_THREAD_EXPORTS.0.get())
            .child_activity_event
            .map_or_else(local_child_activity_event, |entry| entry())
    }
}

#[cfg(all(windows, target_arch = "x86_64"))]
mod windows {
    use super::{
        ForkError, ForkMapping, ForkMappingBehavior, ForkMappingStorage, ForkStage,
        ProcessThreadExports, fork_errno, fork_mapping_trace_enabled, fork_timings_enabled,
        fork_trace_enabled, mappings, participants, vfork_trace_enabled, wait_trace_enabled,
    };
    use core::arch::{asm, naked_asm};
    use core::ffi::c_void;
    use core::mem::{size_of, zeroed};
    use core::ptr;
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use kinakaze_abi::Pid;
    use windows_sys::Win32::Foundation::{
        CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, FILETIME, GetLastError, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::Diagnostics::Debug::{
        CONTEXT, FlushInstructionCache, ReadProcessMemory, SetThreadContext, WriteProcessMemory,
    };
    use windows_sys::Win32::System::Environment::GetCommandLineW;
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW, LoadLibraryW,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEM_COMMIT, MEM_FREE,
        MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER, MEM_RESERVE,
        MEM_RESERVE_PLACEHOLDER, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile, MapViewOfFile3, PAGE_EXECUTE_READWRITE, PAGE_NOACCESS, PAGE_READWRITE,
        UnmapViewOfFile, VirtualAlloc, VirtualAlloc2, VirtualAllocEx, VirtualFree, VirtualFreeEx,
        VirtualProtectEx, VirtualQuery, VirtualQueryEx,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CreateEventW, CreateProcessW, ExitProcess, GetCurrentProcess,
        GetCurrentThreadId, GetExitCodeProcess, GetProcessTimes, GetThreadId, OpenProcess,
        PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, ResumeThread,
        STARTUPINFOW, SetEvent, SuspendThread, THREAD_QUERY_LIMITED_INFORMATION,
        THREAD_SUSPEND_RESUME, TerminateProcess, WaitForMultipleObjects, WaitForSingleObject,
    };

    const CONTEXT_AMD64: u32 = 0x0010_0000;
    const CONTEXT_CONTROL: u32 = CONTEXT_AMD64 | 0x1;
    const CONTEXT_INTEGER: u32 = CONTEXT_AMD64 | 0x2;
    const CONTEXT_FLOATING_POINT: u32 = CONTEXT_AMD64 | 0x8;
    const CAPTURED_CONTEXT_FLAGS: u32 = CONTEXT_CONTROL | CONTEXT_INTEGER | CONTEXT_FLOATING_POINT;
    const FORK_READY_TIMEOUT_MS: u32 = 10_000;
    const CREATE_FAILED: u32 = u32::MAX;
    const STATUS_NO_MORE_ENTRIES: i32 = 0x8000_001a_u32 as i32;
    const SEC_RESERVE: u32 = 0x0400_0000;
    static LAST_FORK_NAMESPACE_PID: AtomicU32 = AtomicU32::new(0);
    const BOOTSTRAP_MAGIC: u64 = 0x4352_5942_4f4f_5432; // "CRYBOOT2"
    const MAX_FORK_THREADS: usize = 4096;
    const WNOHANG: i32 = 1;
    const WNOWAIT: i32 = 0x0100_0000;
    const WAIT_EVENT_EXITED: i32 = 1;
    const WAIT_EVENT_STOPPED: i32 = 2;
    const WAIT_EVENT_CONTINUED: i32 = 4;
    const STILL_ACTIVE: u32 = 259;

    #[derive(Clone, Copy)]
    struct WaitCandidate {
        entry: super::job::Entry,
        handle: Option<usize>,
    }
    const MARKER: &[u16] = &[
        b'-' as u16,
        b'-' as u16,
        b'c' as u16,
        b'r' as u16,
        b'y' as u16,
        b's' as u16,
        b'o' as u16,
        b'a' as u16,
        b'c' as u16,
        b'u' as u16,
        b'-' as u16,
        b'f' as u16,
        b'o' as u16,
        b'r' as u16,
        b'k' as u16,
        b'-' as u16,
        b'c' as u16,
        b'h' as u16,
        b'i' as u16,
        b'l' as u16,
        b'd' as u16,
        b'=' as u16,
    ];

    static LAST_STAGE: AtomicU32 = AtomicU32::new(ForkStage::Unsupported as u32);
    static LAST_OS_ERROR: AtomicU32 = AtomicU32::new(0);
    static BOOTSTRAPPING: AtomicBool = AtomicBool::new(false);
    // Published before the fork snapshot and explicitly restored in the fresh
    // Windows child. Handles are inheritable kernel objects, but process-local
    // Rust statics are not part of the Linux guest address-space snapshot.
    static VFORK_EXEC_EVENT: AtomicUsize = AtomicUsize::new(0);
    static VFORK_EXIT_EVENT: AtomicUsize = AtomicUsize::new(0);
    static VFORK_EXIT_ACK: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn register_child_handle(pid: u32, process: usize) -> i32 {
        if pid == 0 || process == 0 {
            return 0;
        }
        let current = unsafe { GetCurrentProcess() };
        let mut duplicate = ptr::null_mut();
        let duplicated = unsafe {
            DuplicateHandle(
                current,
                process as HANDLE,
                current,
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 {
            return 0;
        }
        let inserted = super::child_processes().lock().is_ok_and(|mut children| {
            if children.get(pid).is_some() {
                false
            } else {
                children.insert(pid, duplicate as usize)
            }
        });
        if !inserted {
            unsafe { CloseHandle(duplicate) };
            return 0;
        }
        1
    }

    unsafe fn close_if_live(handle: HANDLE) {
        if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
            // SAFETY: the caller owns this local handle.
            unsafe { CloseHandle(handle) };
        }
    }

    /// Serializes an active vfork rendezvous into the generic fork command
    /// frame. Ordinary fork has no such record.
    pub(super) fn snapshot_vfork_state() -> Option<[u8; 24]> {
        let handles = [
            VFORK_EXEC_EVENT.load(Ordering::Acquire),
            VFORK_EXIT_EVENT.load(Ordering::Acquire),
            VFORK_EXIT_ACK.load(Ordering::Acquire),
        ];
        if handles.iter().all(|handle| *handle == 0) {
            return None;
        }
        if handles.iter().any(|handle| *handle == 0) {
            return None;
        }
        let mut payload = [0u8; 24];
        for (index, handle) in handles.into_iter().enumerate() {
            payload[index * 8..index * 8 + 8].copy_from_slice(&(handle as u64).to_le_bytes());
        }
        Some(payload)
    }

    /// Restores the child-owned copies of the three inherited event handles.
    pub(super) fn restore_vfork_state(payload: &[u8]) -> bool {
        if payload.len() != 24 {
            return false;
        }
        let read = |offset| {
            u64::from_le_bytes(payload[offset..offset + 8].try_into().unwrap_or_default()) as usize
        };
        let exec = read(0);
        let exit = read(8);
        let ack = read(16);
        if exec == 0 || exit == 0 || ack == 0 {
            return false;
        }
        VFORK_EXEC_EVENT.store(exec, Ordering::Release);
        VFORK_EXIT_EVENT.store(exit, Ordering::Release);
        VFORK_EXIT_ACK.store(ack, Ordering::Release);
        true
    }

    /// Forks with the parent/child rendezvous required by `vfork`.
    pub(super) unsafe fn vfork(_stack_boundary: usize) -> Pid {
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        // Separate events avoid a shared-memory status word whose mapping would
        // itself need reconstructing in the fresh child process.
        let exec_event = unsafe { CreateEventW(&security, 1, 0, ptr::null()) };
        if exec_event.is_null() {
            return -fork_errno(ForkError {
                stage: ForkStage::CreateEvent,
                os_code: unsafe { GetLastError() },
            });
        }
        let exit_event = unsafe { CreateEventW(&security, 1, 0, ptr::null()) };
        if exit_event.is_null() {
            let error = ForkError {
                stage: ForkStage::CreateEvent,
                os_code: unsafe { GetLastError() },
            };
            unsafe { close_if_live(exec_event) };
            return -fork_errno(error);
        }
        let exit_ack = unsafe { CreateEventW(&security, 1, 0, ptr::null()) };
        if exit_ack.is_null() {
            let error = ForkError {
                stage: ForkStage::CreateEvent,
                os_code: unsafe { GetLastError() },
            };
            unsafe {
                close_if_live(exec_event);
                close_if_live(exit_event);
            }
            return -fork_errno(error);
        }

        // Publish the rendezvous before the returns-twice boundary.  The fresh
        // child must inherit it even when restoring a participant fails before
        // `fork` can return zero to this function.
        VFORK_EXEC_EVENT.store(exec_event as usize, Ordering::Release);
        VFORK_EXIT_EVENT.store(exit_event as usize, Ordering::Release);
        VFORK_EXIT_ACK.store(exit_ack as usize, Ordering::Release);

        // SAFETY: this is the same returns-twice boundary as ordinary fork.
        if vfork_trace_enabled() {
            eprintln!(
                "kinakaze vfork: host_pid={} phase=fork-enter",
                std::process::id()
            );
        }
        let pid = match unsafe { super::fork() } {
            Ok(pid) => pid,
            Err(error) => {
                let errno = fork_errno(error);
                if vfork_trace_enabled() {
                    eprintln!(
                        "kinakaze vfork: host_pid={} phase=fork-failed stage={:?} os_code={} errno={errno}",
                        std::process::id(),
                        error.stage,
                        error.os_code,
                    );
                }
                VFORK_EXEC_EVENT.store(0, Ordering::Release);
                VFORK_EXIT_EVENT.store(0, Ordering::Release);
                VFORK_EXIT_ACK.store(0, Ordering::Release);
                unsafe {
                    close_if_live(exec_event);
                    close_if_live(exit_event);
                    close_if_live(exit_ack);
                }
                return -errno;
            }
        };
        if pid == 0 {
            if vfork_trace_enabled() {
                eprintln!(
                    "kinakaze vfork: host_pid={} phase=child-return",
                    std::process::id()
                );
            }
            return 0;
        }
        if vfork_trace_enabled() {
            eprintln!(
                "kinakaze vfork: host_pid={} phase=parent-wait child_pid={pid}",
                std::process::id()
            );
        }

        // The child process object is the authoritative terminal event.  The
        // explicit rendezvous events distinguish successful exec from `_exit`,
        // but restore/bootstrap failure can terminate the child before either
        // event is usable.  Keep an independent handle so a concurrent waitpid
        // cannot invalidate this wait by reaping the registry's copy.
        let child_process = {
            // fork() returns the caller's visible PID. The native handle table
            // is keyed by the stable registry PID, including inside containers.
            // Retain the guard until DuplicateHandle pins the source, so a
            // concurrent waitpid cannot close/reuse it during duplication.
            let registry_pid = super::job::namespaces::resolve(pid as u32);
            let source = super::child_processes().lock().ok();
            let handle = source.as_ref().and_then(|children| {
                registry_pid.and_then(|registry_pid| children.get(registry_pid))
            });
            let Some(handle) = handle else {
                VFORK_EXEC_EVENT.store(0, Ordering::Release);
                VFORK_EXIT_EVENT.store(0, Ordering::Release);
                VFORK_EXIT_ACK.store(0, Ordering::Release);
                unsafe {
                    close_if_live(exec_event);
                    close_if_live(exit_event);
                    close_if_live(exit_ack);
                }
                return -5;
            };
            let current = unsafe { GetCurrentProcess() };
            let mut duplicate = ptr::null_mut();
            if unsafe {
                DuplicateHandle(
                    current,
                    handle as HANDLE,
                    current,
                    &mut duplicate,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            } == 0
            {
                VFORK_EXEC_EVENT.store(0, Ordering::Release);
                VFORK_EXIT_EVENT.store(0, Ordering::Release);
                VFORK_EXIT_ACK.store(0, Ordering::Release);
                unsafe {
                    close_if_live(exec_event);
                    close_if_live(exit_event);
                    close_if_live(exit_ack);
                }
                return -5;
            }
            duplicate
        };
        let waitables = [exec_event, exit_event, child_process];
        // POSIX vfork deliberately suspends the parent until exec or exit.  A
        // terminated process also satisfies that condition, including failure
        // paths that never reached libc `_exit`.
        let outcome = unsafe {
            WaitForMultipleObjects(waitables.len() as u32, waitables.as_ptr(), 0, u32::MAX)
        };
        if vfork_trace_enabled() {
            eprintln!(
                "kinakaze vfork: host_pid={} phase=parent-wake child_pid={pid} outcome={outcome:#x}",
                std::process::id()
            );
        }
        if outcome == WAIT_OBJECT_0 + 1 {
            // The child is parked in `_exit` until the parent acknowledges the
            // rendezvous. Its copied Windows ABI-transition stack is private
            // coordinator state. Copying it over the suspended parent's live
            // frames corrupts return addresses and is not Linux CLONE_VM state.
            unsafe { SetEvent(exit_ack) };
        }

        // These are process-local copies.  The child consumed and closed its
        // inherited handles independently; clear the parent's publication
        // before closing the parent-owned copies below.
        VFORK_EXEC_EVENT.store(0, Ordering::Release);
        VFORK_EXIT_EVENT.store(0, Ordering::Release);
        VFORK_EXIT_ACK.store(0, Ordering::Release);

        unsafe {
            close_if_live(exec_event);
            close_if_live(exit_event);
            close_if_live(exit_ack);
            close_if_live(child_process);
        }
        pid
    }

    /// Ends a clone that exists at the host level but could not restore enough
    /// guest process state to return safely into user code.
    pub(super) unsafe fn terminate_failed_fork_child() -> ! {
        // A normal fork has no published vfork handles, making this a no-op.
        // A vfork child signals its suspended parent and waits for the parent's
        // acknowledgement before its process object disappears.
        unsafe { complete_vfork(false) };
        unsafe { ExitProcess(125) }
    }

    /// Wakes the suspended vfork parent after committed exec or actual exit.
    /// Exit waits for acknowledgement so the child cannot disappear before the
    /// rendezvous. A recoverable exec failure must not call this function.
    pub(super) unsafe fn complete_vfork(exec_succeeded: bool) {
        let exec_event = VFORK_EXEC_EVENT.swap(0, Ordering::AcqRel) as HANDLE;
        let exit_event = VFORK_EXIT_EVENT.swap(0, Ordering::AcqRel) as HANDLE;
        let exit_ack = VFORK_EXIT_ACK.swap(0, Ordering::AcqRel) as HANDLE;
        if exec_event.is_null() || exit_event.is_null() || exit_ack.is_null() {
            return;
        }
        if exec_succeeded {
            unsafe { SetEvent(exec_event) };
        } else {
            unsafe {
                SetEvent(exit_event);
                WaitForSingleObject(exit_ack, u32::MAX);
            }
        }
        unsafe {
            close_if_live(exec_event);
            close_if_live(exit_event);
            close_if_live(exit_ack);
        }
    }

    pub(super) fn is_bootstrapping() -> bool {
        BOOTSTRAPPING.load(Ordering::Acquire)
    }

    /// Core state is linked exactly once into the runtime DLL. There is no
    /// provider-local coordinator to forward, and resolving our own exports
    /// would recurse when a provider first enters one of these counters.
    pub(super) fn resolve_process_u32_exports(_local: usize) -> ProcessThreadExports {
        ProcessThreadExports {
            started: None,
            finished: None,
            count: None,
            child_activity_event: None,
            participant_owner: None,
            register_handle_slot: None,
            unregister_handle_slot: None,
        }
    }

    fn module_for_address(address: usize) -> Option<usize> {
        if address == 0 {
            return None;
        }
        let mut module = ptr::null_mut();
        // SAFETY: FROM_ADDRESS treats the pointer as an address, not a string.
        let found = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                    | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
                address as *const u16,
                &mut module,
            )
        };
        (found != 0 && !module.is_null()).then_some(module as usize)
    }

    fn module_path(base: usize) -> Option<Vec<u16>> {
        let mut path = vec![0u16; 32768];
        // SAFETY: base came from the loader and the buffer is writable.
        let len = unsafe {
            GetModuleFileNameW(base as *mut c_void, path.as_mut_ptr(), path.len() as u32)
        } as usize;
        if len == 0 || len >= path.len() {
            return None;
        }
        path.truncate(len + 1);
        path[len] = 0;
        Some(path)
    }

    fn collect_bootstrap_modules() -> Result<Vec<ModuleSpec>, ForkError> {
        let executable = unsafe { GetModuleHandleW(ptr::null()) } as usize;
        let mut bases = mappings()
            .lock()
            .map_err(|_| ForkError {
                stage: ForkStage::ModuleManifest,
                os_code: 0,
            })?
            .collect_modules();
        let callbacks = participants()
            .lock()
            .map_err(|_| ForkError {
                stage: ForkStage::ModuleManifest,
                os_code: 0,
            })?
            .collect_entries();
        for entry in callbacks {
            for address in [
                entry.hooks.prepare.map_or(0, |item| item as usize),
                entry.hooks.snapshot.map_or(0, |item| item as usize),
                entry.hooks.parent.map_or(0, |item| item as usize),
                entry.hooks.child.map_or(0, |item| item as usize),
            ] {
                if let Some(base) = module_for_address(address) {
                    bases.push(base);
                }
            }
        }
        bases.retain(|base| *base != 0 && *base != executable);
        bases.sort_unstable();
        bases.dedup();
        let mut modules = Vec::with_capacity(bases.len());
        for base in bases {
            let path = module_path(base).ok_or(ForkError {
                stage: ForkStage::ModuleManifest,
                os_code: 0,
            })?;
            modules.push(ModuleSpec { base, path });
        }
        Ok(modules)
    }

    fn create_bootstrap_mapping(
        security: &SECURITY_ATTRIBUTES,
        modules: &[ModuleSpec],
    ) -> Result<BootstrapMapping, ForkError> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&BOOTSTRAP_MAGIC.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(modules.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(
            &super::FORK_TLS_SLOT_MASK
                .load(std::sync::atomic::Ordering::Acquire)
                .to_le_bytes(),
        );
        for module in modules {
            bytes.extend_from_slice(&(module.base as u64).to_le_bytes());
            bytes.extend_from_slice(&(module.path.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            for unit in &module.path {
                bytes.extend_from_slice(&unit.to_le_bytes());
            }
            while !bytes.len().is_multiple_of(8) {
                bytes.push(0);
            }
        }
        let total = bytes.len() as u64;
        bytes[8..16].copy_from_slice(&total.to_le_bytes());
        // SAFETY: INVALID_HANDLE_VALUE asks for a paging-file-backed object.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                security,
                PAGE_READWRITE,
                (total >> 32) as u32,
                total as u32,
                ptr::null(),
            )
        };
        if handle.is_null() {
            return Err(os_error(ForkStage::ModuleManifest));
        }
        let mapping = BootstrapMapping { handle };
        // SAFETY: the mapping was created with read/write access for this size.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, bytes.len()) };
        if view.Value.is_null() {
            return Err(os_error(ForkStage::ModuleManifest));
        }
        // SAFETY: the view is writable and exactly `bytes.len()` bytes were mapped.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), view.Value.cast(), bytes.len());
            UnmapViewOfFile(view);
        }
        Ok(mapping)
    }

    #[repr(C)]
    struct ClientId {
        unique_process: HANDLE,
        unique_thread: HANDLE,
    }

    #[repr(C)]
    struct ThreadBasicInformation {
        exit_status: i32,
        _padding: u32,
        teb_base_address: *mut c_void,
        client_id: ClientId,
        affinity_mask: usize,
        priority: i32,
        base_priority: i32,
    }

    #[repr(C)]
    struct ProcessBasicInformation {
        exit_status: isize,
        peb_base_address: *mut c_void,
        affinity_mask: usize,
        base_priority: isize,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    #[derive(Clone)]
    struct ModuleSpec {
        base: usize,
        path: Vec<u16>,
    }

    struct BootstrapMapping {
        handle: HANDLE,
    }

    struct CreatedChild {
        pid: u32,
        process: HANDLE,
        thread: HANDLE,
        timings: ForkCopyTimings,
    }

    /// A fully bootstrapped, suspended child, not yet visible to the guest.
    ///
    /// Creating a Windows process takes the parent's PEB/loader/heap locks.
    /// It must finish while siblings can still release those locks. Only the
    /// subsequent memory/context copy belongs inside the frozen interval.
    struct PreparedChild {
        process: PROCESS_INFORMATION,
        timings: ForkCopyTimings,
    }

    impl PreparedChild {
        fn commit(mut self, mut timings: ForkCopyTimings) -> CreatedChild {
            timings.create_process_us = self.timings.create_process_us;
            timings.ready_us = self.timings.ready_us;
            timings.suspend_us = self.timings.suspend_us;
            timings.image_us = self.timings.image_us;
            let child = CreatedChild {
                pid: self.process.dwProcessId,
                process: self.process.hProcess,
                thread: self.process.hThread,
                timings,
            };
            self.process.hProcess = ptr::null_mut();
            self.process.hThread = ptr::null_mut();
            child
        }
    }

    impl Drop for PreparedChild {
        fn drop(&mut self) {
            if !self.process.hProcess.is_null() {
                // Rollback runs after the parent's freeze guards are dropped.
                // Never leave a parked bootstrap child behind on an error.
                unsafe {
                    TerminateProcess(self.process.hProcess, 127);
                    WaitForSingleObject(self.process.hProcess, FORK_READY_TIMEOUT_MS);
                    CloseHandle(self.process.hProcess);
                }
            }
            if !self.process.hThread.is_null() {
                unsafe { CloseHandle(self.process.hThread) };
            }
        }
    }

    #[derive(Clone, Copy, Default)]
    struct ForkMappingCopyStats {
        copied_bytes: usize,
        guest_mm_copied_bytes: usize,
        host_private_copied_bytes: usize,
        section_bytes: usize,
        copy_calls: usize,
        committed_regions: usize,
        cow_shared_bytes: usize,
    }

    #[derive(Clone, Copy, Default)]
    struct ForkCopyTimings {
        create_process_us: u128,
        ready_us: u128,
        suspend_us: u128,
        image_us: u128,
        arena_us: u128,
        mappings_us: u128,
        mapping_stats: ForkMappingCopyStats,
        stack_us: u128,
        resume_us: u128,
    }

    impl Drop for BootstrapMapping {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: this object owns the mapping handle.
                unsafe { CloseHandle(self.handle) };
            }
        }
    }

    /// Every other thread is absent in a POSIX fork child. Suspending them for
    /// the copy gives the arena and registered guest mappings one point-in-time
    /// image without attempting to clone their Windows stacks or TEBs.
    ///
    /// `TH32CS_SNAPTHREAD` always scans every thread in the system, even when a
    /// process id is supplied. `NtGetNextThread` walks only this process and
    /// returns the handles needed for suspension directly, avoiding both that
    /// global snapshot and the OpenThread race for threads that just exited.
    pub(super) struct SuspendedThreads {
        handles: *mut HANDLE,
        len: usize,
    }

    impl SuspendedThreads {
        pub(super) unsafe fn freeze() -> Result<Self, ForkError> {
            let bytes = MAX_FORK_THREADS * size_of::<HANDLE>();
            // Fork-coordinator bookkeeping is host-private and short-lived. It
            // must neither consume the small ABI transition stack nor enter the
            // managed arena snapshot that is about to be copied to the child.
            let handles = unsafe {
                VirtualAlloc(ptr::null(), bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)
            }
            .cast::<HANDLE>();
            if handles.is_null() {
                return Err(os_error(ForkStage::ThreadFreeze));
            }
            let mut frozen = Self { handles, len: 0 };
            let current_thread = unsafe { GetCurrentThreadId() };
            let mut cursor = ptr::null_mut();
            let mut current_thread_handle: HANDLE = ptr::null_mut();
            loop {
                let mut thread = ptr::null_mut();
                let status = unsafe {
                    NtGetNextThread(
                        GetCurrentProcess(),
                        cursor,
                        THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION | 0x0010_0000,
                        0,
                        0,
                        &mut thread,
                    )
                };
                if status == STATUS_NO_MORE_ENTRIES {
                    break;
                }
                if status < 0 || thread.is_null() {
                    if !current_thread_handle.is_null() {
                        unsafe { CloseHandle(current_thread_handle) };
                    }
                    return Err(ForkError {
                        stage: ForkStage::ThreadFreeze,
                        os_code: status as u32,
                    });
                }
                if !current_thread_handle.is_null() {
                    unsafe { CloseHandle(current_thread_handle) };
                    current_thread_handle = ptr::null_mut();
                }
                cursor = thread;

                if unsafe { GetThreadId(thread) } == current_thread {
                    current_thread_handle = thread;
                    continue;
                }
                if frozen.len == MAX_FORK_THREADS {
                    unsafe { CloseHandle(thread) };
                    if !current_thread_handle.is_null() {
                        unsafe { CloseHandle(current_thread_handle) };
                    }
                    return Err(ForkError {
                        stage: ForkStage::ThreadFreeze,
                        os_code: 0,
                    });
                }
                if unsafe { SuspendThread(thread) } == CREATE_FAILED {
                    let os_code = unsafe { GetLastError() };
                    // A joinable thread may exit while its handle is retained.
                    // Only skip a proven exit, retaining the enumeration cursor
                    // until NtGetNextThread has advanced past it.
                    if unsafe { WaitForSingleObject(thread, 0) } == WAIT_OBJECT_0 {
                        current_thread_handle = thread;
                        continue;
                    }
                    unsafe { CloseHandle(thread) };
                    if !current_thread_handle.is_null() {
                        unsafe { CloseHandle(current_thread_handle) };
                    }
                    return Err(ForkError {
                        stage: ForkStage::ThreadFreeze,
                        os_code,
                    });
                }
                unsafe { frozen.handles.add(frozen.len).write(thread) };
                frozen.len += 1;
            }
            if !current_thread_handle.is_null() {
                unsafe { CloseHandle(current_thread_handle) };
            }
            Ok(frozen)
        }
    }

    impl Drop for SuspendedThreads {
        fn drop(&mut self) {
            for index in (0..self.len).rev() {
                let thread = unsafe { self.handles.add(index).read() };
                // Restore exactly the suspension count this transaction added.
                unsafe {
                    ResumeThread(thread);
                    CloseHandle(thread);
                }
            }
            if !self.handles.is_null() {
                unsafe { VirtualFree(self.handles.cast(), 0, MEM_RELEASE) };
                self.handles = ptr::null_mut();
            }
        }
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn NtQueryInformationThread(
            thread_handle: HANDLE,
            thread_information_class: u32,
            thread_information: *mut c_void,
            thread_information_length: u32,
            return_length: *mut u32,
        ) -> i32;

        fn NtQueryInformationProcess(
            process_handle: HANDLE,
            process_information_class: u32,
            process_information: *mut c_void,
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

    /// Assembly boundary that hides the returns-twice behavior from Rust.
    ///
    /// The parent calls `clone_parent` using a temporary CONTEXT below the
    /// captured stack pointer. The child resumes at label 2 with the original
    /// stack and simply returns zero to the Rust caller.
    #[unsafe(naked)]
    pub(super) unsafe extern "C" fn raw_fork(parent: u32) -> Pid {
        naked_asm!(
            "sub rsp, 0x4f8",
            "lea r10, [rsp + 0x20]",
            "mov dword ptr [r10 + 48], {context_flags}",
            "stmxcsr [r10 + 52]",
            "mov ax, cs",
            "mov word ptr [r10 + 56], ax",
            "mov ax, ss",
            "mov word ptr [r10 + 66], ax",
            "pushfq",
            "pop rax",
            "mov dword ptr [r10 + 68], eax",
            "xor eax, eax",
            "mov [r10 + 120], rax",
            "mov [r10 + 128], rax",
            "mov [r10 + 136], rax",
            "mov [r10 + 144], rbx",
            "lea rax, [rsp + 0x4f8]",
            "mov [r10 + 152], rax",
            "mov [r10 + 160], rbp",
            "mov [r10 + 168], rsi",
            "mov [r10 + 176], rdi",
            "xor eax, eax",
            "mov [r10 + 184], rax",
            "mov [r10 + 192], rax",
            "mov [r10 + 200], rax",
            "mov [r10 + 208], rax",
            "mov [r10 + 216], r12",
            "mov [r10 + 224], r13",
            "mov [r10 + 232], r14",
            "mov [r10 + 240], r15",
            "lea rax, [rip + 2f]",
            "mov [r10 + 248], rax",
            "fxsave64 [r10 + 256]",
            "mov edx, ecx",
            "mov rcx, r10",
            "call {clone_parent}",
            "add rsp, 0x4f8",
            "ret",
            "2:",
            "xor eax, eax",
            "ret",
            context_flags = const CAPTURED_CONTEXT_FLAGS,
            clone_parent = sym clone_parent,
        )
    }

    unsafe extern "C" fn clone_parent(context: *const CONTEXT, parent: u32) -> Pid {
        super::LAST_NATIVE_FAILURE_LINE.store(0, Ordering::Release);
        LAST_FORK_NAMESPACE_PID.store(0, Ordering::Release);
        // The bootstrap's native heap/section addresses are chosen by Windows
        // before guest VMAs can be reserved. An occasional ASLR collision is
        // ERROR_INVALID_ADDRESS, not memory exhaustion. PreparedChild rolls
        // back the unpublished process and thaws the parent on every failure,
        // so another bootstrap can reserve the same frozen guest snapshot.
        // Do not retry resource exhaustion or any post-publication failure.
        let mut attempt = 0;
        let outcome = loop {
            let result = unsafe { clone_parent_inner(&*context, parent) };
            if result
                .as_ref()
                .is_err_and(|e| e.stage == ForkStage::GuestMapping && e.os_code == 487)
                && attempt < 3
            {
                attempt += 1;
                if fork_mapping_trace_enabled() {
                    eprintln!(
                        "kinakaze fork: retrying unpublished bootstrap after address collision, attempt {}",
                        attempt + 1
                    );
                }
                continue;
            }
            break result;
        };
        match outcome {
            Ok(pid) if pid <= i32::MAX as u32 => pid as Pid,
            Ok(_) => {
                set_error(ForkStage::CreateProcess, 0);
                -1
            }
            Err(error) => {
                if fork_mapping_trace_enabled() {
                    eprintln!(
                        "kinakaze: fork failed at {:?} (host error {})",
                        error.stage, error.os_code
                    );
                }
                set_error(error.stage, error.os_code);
                -1
            }
        }
    }

    unsafe fn clone_parent_inner(context: &CONTEXT, parent: u32) -> Result<u32, ForkError> {
        // Once sibling threads and the allocator are frozen, even a lazy
        // environment lookup is forbidden: `std::env` may allocate from the
        // arena whose lock this thread owns.  Resolve the diagnostic flag while
        // the process is still fully live so failure-only logging remains
        // allocation-free inside the clone critical section.
        let _ = fork_mapping_trace_enabled();
        let fork_started = std::time::Instant::now();
        super::trace_registered_tls_slots("clone-enter", 0);
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: 1,
        };
        // A one-shot auto-reset event is the sole child-loader handshake. The
        // child signals and closes its inherited handle; the parent retains its
        // own handle until the whole transaction has completed.
        let ready_handle = unsafe { CreateEventW(&security, 0, 0, ptr::null()) };
        if ready_handle.is_null() {
            return Err(os_error(ForkStage::CreateEvent));
        }
        // Use the same one-handle owner as the manifest mapping.  This keeps
        // every early `?` path from leaking the inheritable event.
        let ready = BootstrapMapping {
            handle: ready_handle,
        };

        // Everything that allocates is prepared before the arena lock is taken.
        // The lock is then held across the copy so no allocator metadata can be
        // published halfway through the child image.
        let module_specs = collect_bootstrap_modules()?;
        let bootstrap = create_bootstrap_mapping(&security, &module_specs)?;
        super::trace_registered_tls_slots("bootstrap-ready", 0);
        if fork_trace_enabled() || wait_trace_enabled() {
            eprintln!("kinakaze fork: bootstrap manifest ready");
        }
        // This child has not run guest code and cannot observe the fork until
        // its context is installed below. Preparing it early changes no Linux
        // memory semantics; it prevents CreateProcess/GetModuleHandle from
        // waiting for a host lock held by a subsequently suspended sibling.
        let prepared_child = unsafe { prepare_child(ready.handle, bootstrap.handle)? };
        // All owners are pinned by the outer topology transaction. Duplicate
        // before freezing host threads; the child owns these references even
        // if the parent dies before its ready/context handshake completes.
        // Exclude host mapping changes before reading the shared VMA table and
        // keep them excluded until the copied child is fully materialised.  All
        // potentially allocating preparation remains above this point; mapping
        // implementations may re-enter this process-shared transaction when
        // they publish their already-coordinated registry update.
        let _mapping_transaction = super::begin_fork_mapping_transaction().ok_or(ForkError {
            stage: ForkStage::GuestMapping,
            os_code: 0,
        })?;
        let mut guest_mappings = kinakaze_alloc::collect_shared_mappings()
            .into_iter()
            .map(|m| {
                Ok(ForkMapping {
                    base: m.base,
                    len: m.len,
                    behavior: match m.behavior {
                        2 => ForkMappingBehavior::Zero,
                        3 => ForkMappingBehavior::Omit,
                        1 => ForkMappingBehavior::Copy,
                        _ => {
                            return Err(ForkError {
                                stage: ForkStage::GuestMapping,
                                os_code: 0,
                            });
                        }
                    },
                    storage: match m.storage {
                        1 => ForkMappingStorage::Placeholder,
                        2 => ForkMappingStorage::Section,
                        3 => ForkMappingStorage::RetainedSection,
                        4 => ForkMappingStorage::CopyOnWriteSection,
                        5 => ForkMappingStorage::AnonymousSnapshot,
                        0 => ForkMappingStorage::Ordinary,
                        _ => {
                            return Err(ForkError {
                                stage: ForkStage::GuestMapping,
                                os_code: 0,
                            });
                        }
                    },
                    backing_slot: m.backing_slot,
                    backing_offset: m.backing_offset,
                    view_protection: m.view_protection,
                    domain: super::ForkMappingDomain::from_raw(m.domain).ok_or(ForkError {
                        stage: ForkStage::GuestMapping,
                        os_code: 0,
                    })?,
                })
            })
            .collect::<Result<Vec<_>, ForkError>>()?;
        // Registry publication order is unrelated to virtual-address order.
        // Sort before the allocator and process threads are frozen so the
        // allocation-free clone phase can install each storage class from high
        // addresses to low addresses.  On Windows, replacing a lower fragment
        // changes the AllocationBase observed by the remaining suffix.
        guest_mappings.sort_unstable_by_key(|mapping| mapping.base);
        // Never unmap the calling thread's active or captured stack. Its child
        // gets a fresh copied section; the copied provider owner remains a
        // snapshot owner and can freeze that private section at a later fork.
        let stack_marker = 0u8;
        let copied_slots = snapshot_stack_fallback(
            &mut guest_mappings,
            context.Rsp as usize,
            ptr::addr_of!(stack_marker) as usize,
        );
        let retained_handles = unsafe {
            super::handle_slots::duplicate_into_excluding(
                prepared_child.process.hProcess,
                &copied_slots,
            )?
        };
        super::trace_registered_tls_slots("guest-mappings-collected", 0);
        if fork_trace_enabled() {
            eprintln!(
                "kinakaze fork: collected {} guest mappings",
                guest_mappings.len()
            );
        }
        let mut system_info: SYSTEM_INFO = unsafe { zeroed() };
        unsafe { GetSystemInfo(&mut system_info) };
        let reservation_groups = plan_remote_reservation_groups(
            &guest_mappings,
            system_info.dwAllocationGranularity as usize,
        )?;
        // Size every scratch collection before the allocator and sibling
        // threads are frozen. The number of distinct section-backed VMAs is a
        // property of the current Linux address space, not a fixed runtime
        // constant; long-lived Go and JVM processes routinely exceed 64.
        let mut section_backings = plan_fork_section_backings(&guest_mappings)?;
        // Validate all retained owners before dereferencing any slot in the
        // allocation-free clone phase. A section cannot be reconstructed from
        // an unregistered parent HANDLE value even if that value is still live.
        for mapping in &guest_mappings {
            if !mapping.valid_storage_contract()
                || (mapping.storage.retains_section()
                    && !retained_handles
                        .iter()
                        .any(|slot| slot.address == mapping.backing_slot))
            {
                return Err(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                });
            }
        }
        // Map fresh child-owned storage before freezing allocator and sibling
        // threads. Its temporary parent view is only a bulk transport into the
        // child's independent arena, never the parent's allocator backing.
        let arena_copy = unsafe {
            super::arena_copy::ArenaCopy::prepare(
                prepared_child.process.hProcess,
                kinakaze_alloc::ARENA_BASE,
                kinakaze_alloc::ARENA_SIZE,
            )?
        };
        let profile_fork = fork_timings_enabled();
        // Growth takes topology -> guest -> host. Freeze in that same order,
        // before suspending threads which might otherwise own a guest lock.
        let guest_arena_guard = kinakaze_alloc::guest::freeze_if_initialized();
        let (arena_guard, arena_snapshot) =
            kinakaze_alloc::freeze_snapshot().map_err(|_| ForkError {
                stage: ForkStage::ArenaMapping,
                os_code: 0,
            })?;
        // Optional, allocation-free diagnostics only; never use these counts
        // to omit bytes from the private snapshot. Print only after thawing.
        let free_statistics = profile_fork.then(|| {
            let started = std::time::Instant::now();
            let stats = arena_guard.free_statistics(system_info.dwPageSize as usize);
            (stats, started.elapsed().as_micros())
        });
        super::trace_registered_tls_slots("arena-frozen", 0);
        let prepare_us = fork_started.elapsed().as_micros();
        let guest_reserved = guest_mappings
            .iter()
            .map(|mapping| mapping.len)
            .sum::<usize>();
        let freeze_started = std::time::Instant::now();
        let frozen = unsafe { SuspendedThreads::freeze()? };
        super::trace_registered_tls_slots("threads-frozen", 0);
        let freeze_us = freeze_started.elapsed().as_micros();
        let result = unsafe {
            copy_into_prepared_child(
                context,
                &prepared_child.process,
                &arena_snapshot,
                &arena_copy,
                &guest_mappings,
                &reservation_groups,
                &mut section_backings,
                &retained_handles,
                guest_arena_guard.is_some(),
            )
        };
        super::trace_registered_tls_slots("child-cloned", 0);
        // No parent writable alias survives the fork publication boundary.
        let result = result.and_then(|timings| arena_copy.finish().map(|()| timings));
        let thaw_started = std::time::Instant::now();
        drop(frozen);
        super::trace_registered_tls_slots("threads-thawed", 0);
        let thaw_us = thaw_started.elapsed().as_micros();
        drop(arena_guard);
        drop(guest_arena_guard);
        super::trace_registered_tls_slots("arena-thawed", 0);
        let result = result.map(|timings| prepared_child.commit(timings));
        // HashMap growth may allocate from the managed arena.  It must happen
        // only after the snapshot lock above has been released.
        match result {
            Ok(mut child) => {
                super::trace_registered_tls_slots("child-register-enter", 0);
                // The child handle is indexed by the guest pid immediately,
                // while this function still returns the host pid needed by
                // parent callbacks at the outer fork boundary.
                let explicit_parent = (parent != 0).then_some(parent);
                let Some(guest_pid) =
                    super::job::register_forked_child_with_parent(child.pid, explicit_parent)
                else {
                    // The Linux-visible PID row and inherited fd metadata are
                    // mandatory fork state. Do not expose the Windows pid as a
                    // substitute when either could not be published.
                    unsafe {
                        TerminateProcess(child.process, 127);
                        WaitForSingleObject(child.process, FORK_READY_TIMEOUT_MS);
                        CloseHandle(child.thread);
                        CloseHandle(child.process);
                    }
                    return Err(ForkError {
                        stage: ForkStage::CreateProcess,
                        os_code: 0,
                    });
                };
                super::trace_registered_tls_slots("child-job-registered", 0);
                let registered_handle = match super::child_processes().lock() {
                    Ok(mut children) => children.insert(guest_pid, child.process as usize),
                    Err(error) => {
                        if fork_trace_enabled() {
                            eprintln!(
                                "kinakaze fork: child-handle registry poisoned for guest pid {guest_pid}: {error}"
                            );
                        }
                        false
                    }
                };
                if registered_handle {
                    // Publish the exact child identity and its wait handle
                    // before it can run. A short-lived child may otherwise
                    // publish its exit before registration, which then erases
                    // the zombie report and makes wait(2) report EIO.
                    let resume_started = std::time::Instant::now();
                    if unsafe { ResumeThread(child.thread) } == CREATE_FAILED {
                        let error = os_error(ForkStage::ResumeChild);
                        if let Ok(mut children) = super::child_processes().lock() {
                            children.remove(guest_pid);
                        }
                        super::job::release_slot(guest_pid);
                        unsafe {
                            TerminateProcess(child.process, 127);
                            WaitForSingleObject(child.process, FORK_READY_TIMEOUT_MS);
                            CloseHandle(child.thread);
                            CloseHandle(child.process);
                        }
                        return Err(error);
                    }
                    child.timings.resume_us = resume_started.elapsed().as_micros();
                    unsafe { CloseHandle(child.thread) };
                    if fork_trace_enabled() || profile_fork {
                        super::fork_timing_line(format_args!(
                            "kinakaze: fork timings prepare={}us freeze={}us create={}us ready={}us suspend={}us image={}us arena={}us mappings={}us stack={}us resume={}us thaw={}us total={}us arena_used={} arena_mapped={} guest_mappings={} guest_reserved={} copied_bytes={} section_bytes={} copy_calls={} committed_regions={} guest_mm_copied_bytes={} host_private_copied_bytes={} cow_shared_bytes={}",
                            prepare_us,
                            freeze_us,
                            child.timings.create_process_us,
                            child.timings.ready_us,
                            child.timings.suspend_us,
                            child.timings.image_us,
                            child.timings.arena_us,
                            child.timings.mappings_us,
                            child.timings.stack_us,
                            child.timings.resume_us,
                            thaw_us,
                            fork_started.elapsed().as_micros(),
                            arena_snapshot.used_len,
                            arena_snapshot.mapped_len,
                            guest_mappings.len(),
                            guest_reserved,
                            child.timings.mapping_stats.copied_bytes,
                            child.timings.mapping_stats.section_bytes,
                            child.timings.mapping_stats.copy_calls,
                            child.timings.mapping_stats.committed_regions,
                            child.timings.mapping_stats.guest_mm_copied_bytes,
                            child.timings.mapping_stats.host_private_copied_bytes,
                            child.timings.mapping_stats.cow_shared_bytes,
                        ));
                    }
                    if let Some((stats, elapsed_us)) = free_statistics {
                        match stats {
                            Ok(stats) => super::fork_timing_line(format_args!(
                                "kinakaze: fork free statistics scan={}us large_payload={} large_whole_pages={} large_blocks={} scanned_nodes={}",
                                elapsed_us,
                                stats.large_central_payload_bytes,
                                stats.large_central_whole_pages,
                                stats.large_central_free_blocks,
                                stats.scanned_nodes
                            )),
                            Err(error) => super::fork_timing_line(format_args!(
                                "kinakaze: fork free statistics scan={elapsed_us}us unavailable={error:?}"
                            )),
                        }
                    }
                    // Keep the immutable Linux identity in the serialized fork
                    // transaction. The child may exec and rebind its host pid
                    // before slow parent callbacks finish, so translating the
                    // original host pid afterwards is inherently racy.
                    LAST_FORK_NAMESPACE_PID.store(guest_pid, Ordering::Release);
                    super::trace_registered_tls_slots("child-handle-registered", 0);
                    Ok(child.pid)
                } else {
                    if fork_trace_enabled() {
                        eprintln!(
                            "kinakaze fork: child-handle registration failed for guest pid {guest_pid}"
                        );
                    }
                    super::job::release_slot(guest_pid);
                    unsafe {
                        TerminateProcess(child.process, 127);
                        WaitForSingleObject(child.process, FORK_READY_TIMEOUT_MS);
                        CloseHandle(child.thread);
                        CloseHandle(child.process);
                    }
                    Err(ForkError {
                        stage: ForkStage::CreateProcess,
                        os_code: 0,
                    })
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn take_last_fork_namespace_pid() -> Option<u32> {
        let pid = LAST_FORK_NAMESPACE_PID.swap(0, Ordering::AcqRel);
        (pid != 0).then_some(pid)
    }

    unsafe fn prepare_child(ready: HANDLE, bootstrap: HANDLE) -> Result<PreparedChild, ForkError> {
        let mut executable = [0u16; 32768];
        let executable_len = unsafe {
            GetModuleFileNameW(
                ptr::null_mut(),
                executable.as_mut_ptr(),
                executable.len() as u32,
            )
        } as usize;
        if executable_len == 0 || executable_len >= executable.len() {
            return Err(os_error(ForkStage::ExecutablePath));
        }

        let mut command = [0u16; 128];
        let mut cursor = 0;
        append_ascii(&mut command, &mut cursor, "kinakaze-child --kinakaze-fork ")?;
        append_utf16(&mut command, &mut cursor, MARKER)?;
        append_decimal(&mut command, &mut cursor, ready as usize)?;
        append_ascii(&mut command, &mut cursor, ":")?;
        append_decimal(&mut command, &mut cursor, bootstrap as usize)?;
        command[cursor] = 0;

        let mut startup: STARTUPINFOW = unsafe { zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        // CREATE_NO_WINDOW must retain redirected diagnostics and standard I/O.
        // Without USESTDHANDLES Windows can replace them with null handles.
        startup.dwFlags = windows_sys::Win32::System::Threading::STARTF_USESTDHANDLES;
        startup.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        startup.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        startup.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
        let create_started = std::time::Instant::now();
        let created = unsafe {
            CreateProcessW(
                executable.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                CREATE_NO_WINDOW,
                ptr::null(),
                ptr::null(),
                &startup,
                &mut process,
            )
        };
        if created == 0 {
            return Err(os_error(ForkStage::CreateProcess));
        }
        let mut child = PreparedChild {
            process,
            timings: ForkCopyTimings {
                create_process_us: create_started.elapsed().as_micros(),
                ..ForkCopyTimings::default()
            },
        };
        let ready_started = std::time::Instant::now();
        let wait_handles = [ready, process.hProcess];
        let wait_status = unsafe {
            WaitForMultipleObjects(
                wait_handles.len() as u32,
                wait_handles.as_ptr(),
                0,
                FORK_READY_TIMEOUT_MS,
            )
        };
        match wait_status {
            WAIT_OBJECT_0 => {}
            status if status == WAIT_OBJECT_0 + 1 => {
                let mut exit_code = 0;
                if unsafe { GetExitCodeProcess(process.hProcess, &mut exit_code) } == 0 {
                    return Err(os_error(ForkStage::ChildBootstrap));
                }
                return Err(ForkError {
                    stage: ForkStage::ChildBootstrap,
                    os_code: exit_code,
                });
            }
            status if status == u32::MAX => {
                return Err(os_error(ForkStage::ChildBootstrap));
            }
            status => {
                return Err(ForkError {
                    stage: ForkStage::ChildBootstrap,
                    os_code: status,
                });
            }
        }
        child.timings.ready_us = ready_started.elapsed().as_micros();
        let suspend_started = std::time::Instant::now();
        let previous_suspend_count = unsafe { SuspendThread(process.hThread) };
        if previous_suspend_count == CREATE_FAILED {
            return Err(os_error(ForkStage::SuspendChild));
        }
        child.timings.suspend_us = suspend_started.elapsed().as_micros();

        let image_started = std::time::Instant::now();
        unsafe { verify_same_image_base(process.hProcess)? };
        child.timings.image_us = image_started.elapsed().as_micros();
        Ok(child)
    }

    unsafe fn copy_into_prepared_child(
        context: &CONTEXT,
        process: &PROCESS_INFORMATION,
        arena_snapshot: &kinakaze_alloc::ArenaSnapshot,
        arena_copy: &super::arena_copy::ArenaCopy,
        guest_mappings: &[ForkMapping],
        reservation_groups: &[ForkReservation],
        section_backings: &mut [ForkSectionBacking],
        retained_handles: &[super::handle_slots::ChildSlot],
        repair_guest_heap: bool,
    ) -> Result<ForkCopyTimings, ForkError> {
        // The child's loader and image verification have already completed.
        // Siblings are now suspended: only lock-independent kernel operations
        // and preallocated bookkeeping may run until the caller thaws them.
        let copy_started = std::time::Instant::now();
        unsafe { arena_copy.copy(arena_snapshot)? };
        for slot in retained_handles {
            unsafe {
                arena_copy.patch_handle(arena_snapshot.base as usize, slot.address, slot.value)?;
            }
        }
        let arena_us = copy_started.elapsed().as_micros();
        let mappings_started = std::time::Instant::now();
        let mut mapping_stats = unsafe {
            copy_registered_mappings(
                process.hProcess,
                guest_mappings,
                reservation_groups,
                section_backings,
            )?
        };
        if repair_guest_heap {
            let bytes = unsafe { super::guest_heap::repair_child_locks(process.hProcess)? };
            mapping_stats.copied_bytes += bytes;
            mapping_stats.guest_mm_copied_bytes += bytes;
            mapping_stats.copy_calls += 1;
        }
        let mappings_us = mappings_started.elapsed().as_micros();
        let stack_started = std::time::Instant::now();
        unsafe {
            copy_native_stack(
                process.hProcess,
                process.hThread,
                context,
                arena_snapshot,
                guest_mappings,
            )?
        };
        let stack_us = stack_started.elapsed().as_micros();

        if unsafe { SetThreadContext(process.hThread, context) } == 0 {
            return Err(os_error(ForkStage::ThreadContext));
        }
        // The caller first thaws the parent, registers this child's Linux PID
        // and wait handle, and only then resumes the installed guest context.
        Ok(ForkCopyTimings {
            arena_us,
            mappings_us,
            mapping_stats,
            stack_us,
            ..ForkCopyTimings::default()
        })
    }

    unsafe fn verify_same_image_base(process: HANDLE) -> Result<(), ForkError> {
        let mut basic: ProcessBasicInformation = unsafe { zeroed() };
        let status = unsafe {
            NtQueryInformationProcess(
                process,
                0,
                ptr::addr_of_mut!(basic).cast(),
                size_of::<ProcessBasicInformation>() as u32,
                ptr::null_mut(),
            )
        };
        if status < 0 {
            return Err(ForkError {
                stage: ForkStage::ImageLayout,
                os_code: status as u32,
            });
        }
        let mut remote_base = 0usize;
        unsafe {
            read_remote_exact(
                process,
                basic.peb_base_address.byte_add(0x10),
                ptr::addr_of_mut!(remote_base).cast(),
                size_of::<usize>(),
                ForkStage::ImageLayout,
            )?;
        }
        let local_base = unsafe { GetModuleHandleW(ptr::null()) } as usize;
        if local_base == 0 || local_base != remote_base {
            return Err(ForkError {
                stage: ForkStage::ImageLayout,
                os_code: 0,
            });
        }
        Ok(())
    }

    struct ForkSectionBacking {
        slot: usize,
        size: usize,
        local: HANDLE,
        // Operation-local alias of the CHILD's private backing, never an alias
        // of the parent's guest memory. It is not serialized or inherited.
        staging: *mut c_void,
    }

    #[derive(Clone, Copy)]
    struct ForkReservation {
        base: usize,
        len: usize,
        ordinary_only: bool,
        /// Host-only suffix kept until terminal section views are installed.
        temporary_tail: usize,
    }

    impl Drop for ForkSectionBacking {
        fn drop(&mut self) {
            if !self.staging.is_null() {
                unsafe {
                    UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: self.staging,
                    })
                };
            }
            if !self.local.is_null() {
                unsafe { CloseHandle(self.local) };
            }
        }
    }

    fn snapshot_stack_fallback(
        mappings: &mut [ForkMapping],
        saved_stack: usize,
        current_stack: usize,
    ) -> Vec<usize> {
        let slots: Vec<_> = mappings
            .iter()
            .filter(|m| {
                m.storage == ForkMappingStorage::AnonymousSnapshot
                    && m.base.checked_add(m.len).is_some_and(|end| {
                        (m.base..end).contains(&saved_stack)
                            || (m.base..end).contains(&current_stack)
                    })
            })
            .map(|m| m.backing_slot)
            .collect();
        for mapping in mappings {
            if mapping.storage == ForkMappingStorage::AnonymousSnapshot
                && slots.contains(&mapping.backing_slot)
            {
                mapping.storage = ForkMappingStorage::Section;
                mapping.view_protection = 0;
            }
        }
        slots
    }

    /// Builds the allocation-free section plan consumed while fork is frozen.
    fn plan_fork_section_backings(
        registered: &[ForkMapping],
    ) -> Result<Vec<ForkSectionBacking>, ForkError> {
        let mut sections: Vec<ForkSectionBacking> = Vec::new();
        for mapping in registered.iter().filter(|mapping| {
            mapping.behavior != ForkMappingBehavior::Omit
                && mapping.storage == ForkMappingStorage::Section
        }) {
            if mapping.backing_slot == 0 {
                return Err(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                });
            }
            let required = (mapping.backing_offset as usize)
                .checked_add(mapping.len)
                .ok_or(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                })?;
            if let Some(section) = sections
                .iter_mut()
                .find(|section| section.slot == mapping.backing_slot)
            {
                section.size = section.size.max(required);
            } else {
                sections.push(ForkSectionBacking {
                    slot: mapping.backing_slot,
                    size: required,
                    local: ptr::null_mut(),
                    staging: ptr::null_mut(),
                });
            }
        }
        Ok(sections)
    }

    /// Plans allocation-granularity reservation components for the fork child.
    ///
    /// Windows reserves addresses at allocation granularity, while Linux VMAs
    /// are page granular. Every connected allocation envelope is therefore
    /// reserved once as a replaceable placeholder; its exact Linux fragments
    /// are carved and replaced below. This also handles an ordinary 4 KiB VMA
    /// whose base lies inside a 64 KiB envelope without changing its address.
    fn plan_remote_reservation_groups(
        registered: &[ForkMapping],
        granularity: usize,
    ) -> Result<Vec<ForkReservation>, ForkError> {
        if granularity == 0 || !granularity.is_power_of_two() {
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            });
        }

        let mut groups: Vec<ForkReservation> = Vec::with_capacity(registered.len());
        for mapping in registered
            .iter()
            .filter(|mapping| mapping.behavior != ForkMappingBehavior::Omit)
        {
            let mapping_end = mapping.base.checked_add(mapping.len).ok_or(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            })?;
            let mut group_base = mapping.base & !(granularity - 1);
            let mut group_end = mapping_end
                .checked_add(granularity - 1)
                .map(|end| end & !(granularity - 1))
                .ok_or(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                })?;

            let mut ordinary_only = mapping.storage == ForkMappingStorage::Ordinary;

            // Merge overlapping allocation envelopes before threads and the
            // allocator are frozen. The resulting immutable plan is consumed
            // without allocation in the fork critical section.
            let mut index = 0usize;
            while index < groups.len() {
                let existing_end = groups[index].base + groups[index].len;
                // Touching host envelopes must share one placeholder
                // reservation. A page-granular section view at the final page
                // of a 64 KiB envelope is rejected by Windows when that page is
                // also the end of its placeholder reservation; retaining the
                // adjacent envelope supplies the suffix that existed in the
                // parent and keeps the Linux view address page granular.
                if group_base <= existing_end && groups[index].base <= group_end {
                    group_base = group_base.min(groups[index].base);
                    group_end = group_end.max(existing_end);
                    ordinary_only &= groups[index].ordinary_only;
                    groups.swap_remove(index);
                    continue;
                }
                index += 1;
            }
            groups.push(ForkReservation {
                base: group_base,
                len: group_end - group_base,
                ordinary_only,
                temporary_tail: 0,
            });
        }

        // This Windows build rejects a section view that ends exactly at the
        // end of its placeholder reservation, including fully 64 KiB-aligned
        // views. Preserve one temporary host envelope for a terminal section;
        // it is released immediately after all section views are installed and
        // therefore never becomes Linux-visible address-space state.
        for group in &mut groups {
            let logical_end = group.base.checked_add(group.len).ok_or(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            })?;
            let terminal_view = registered.iter().any(|mapping| {
                mapping.behavior != ForkMappingBehavior::Omit
                    && (matches!(
                        mapping.storage,
                        ForkMappingStorage::Section
                            | ForkMappingStorage::RetainedSection
                            | ForkMappingStorage::CopyOnWriteSection
                            | ForkMappingStorage::AnonymousSnapshot
                    ) || (!group.ordinary_only
                        && mapping.storage == ForkMappingStorage::Ordinary))
                    && mapping
                        .base
                        .checked_add(mapping.len)
                        .is_some_and(|end| end == logical_end)
            });
            if terminal_view {
                group.len = group.len.checked_add(granularity).ok_or(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                })?;
                group.temporary_tail = granularity;
            }
        }
        Ok(groups)
    }

    /// Produces the allocation-free child installation order for a sorted VMA
    /// registry.  Touching Linux VMAs form one component and are installed
    /// bottom-up so every section replacement retains a placeholder suffix.
    /// Components separated by an unmapped gap are installed top-down: a
    /// page-granular lower replacement may otherwise leave the higher gap with
    /// a non-aligned AllocationBase that Windows refuses to replace privately.
    struct MappingInstallationOrder<'a> {
        registered: &'a [ForkMapping],
        limit: usize,
        next: usize,
        component_end: usize,
    }

    impl<'a> MappingInstallationOrder<'a> {
        fn new(registered: &'a [ForkMapping]) -> Result<Self, ForkError> {
            for (mapping_index, mapping) in registered.iter().enumerate() {
                let end = mapping.base.checked_add(mapping.len).ok_or(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                })?;
                if mapping_index + 1 == registered.len() {
                    continue;
                }
                let next = &registered[mapping_index + 1];
                if end <= next.base {
                    continue;
                }
                if fork_mapping_trace_enabled() {
                    eprintln!(
                        "kinakaze fork: mapping order rejected overlap left=#{mapping_index} base={:#x} len={:#x} storage={:?} right=#{} base={:#x} len={:#x} storage={:?}",
                        mapping.base,
                        mapping.len,
                        mapping.storage,
                        mapping_index + 1,
                        next.base,
                        next.len,
                        next.storage,
                    );
                }
                return Err(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                });
            }
            Ok(Self {
                registered,
                limit: registered.len(),
                next: 0,
                component_end: 0,
            })
        }
    }

    impl Iterator for MappingInstallationOrder<'_> {
        type Item = usize;

        fn next(&mut self) -> Option<Self::Item> {
            if self.next < self.component_end {
                let mapping_index = self.next;
                self.next += 1;
                return Some(mapping_index);
            }

            while self.limit != 0
                && self.registered[self.limit - 1].behavior == ForkMappingBehavior::Omit
            {
                self.limit -= 1;
            }
            if self.limit == 0 {
                return None;
            }

            let component_end = self.limit;
            let mut component_start = component_end - 1;
            while component_start != 0 {
                let previous = &self.registered[component_start - 1];
                if previous.behavior == ForkMappingBehavior::Omit
                    || previous.base + previous.len != self.registered[component_start].base
                {
                    break;
                }
                component_start -= 1;
            }

            self.limit = component_start;
            self.next = component_start + 1;
            self.component_end = component_end;
            Some(component_start)
        }
    }

    /// Reserves every allocation-granularity component as a replaceable
    /// placeholder. The exact Linux fragments are split out below.
    unsafe fn reserve_remote_mapping_groups(
        process: HANDLE,
        groups: &[ForkReservation],
    ) -> Result<(), ForkError> {
        for (group_index, group) in groups.iter().enumerate() {
            let reserved = unsafe {
                VirtualAlloc2(
                    process,
                    group.base as *const c_void,
                    group.len,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            if reserved as usize != group.base {
                let os_code = unsafe { GetLastError() };
                if fork_mapping_trace_enabled() {
                    let mut remote: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
                    let _ = unsafe {
                        VirtualQueryEx(
                            process,
                            group.base as *const c_void,
                            &mut remote,
                            size_of::<MEMORY_BASIC_INFORMATION>(),
                        )
                    };
                    eprintln!(
                        "kinakaze fork: reservation group #{group_index} failed base={:#x} len={:#x} returned={:#x} error={} query_base={:#x} allocation_base={:#x} region_size={:#x} state={:#x}",
                        group.base,
                        group.len,
                        reserved as usize,
                        os_code,
                        remote.BaseAddress as usize,
                        remote.AllocationBase as usize,
                        remote.RegionSize,
                        remote.State,
                    );
                }
                super::LAST_NATIVE_FAILURE_LINE.store(line!(), Ordering::Release);
                return Err(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code,
                });
            }
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: reserved placeholder group base={:#x} len={:#x}",
                    group.base, group.len
                );
            }
        }
        Ok(())
    }

    /// Converts ordinary-only placeholder envelopes into private reservations.
    ///
    /// Windows requires the base of a private reservation to be allocation-
    /// granularity aligned even when a Linux VMA begins on an interior 4 KiB
    /// page. Reserve the complete host envelope once, then commit only the
    /// exact Linux pages while copying mappings below. The observable address,
    /// committed-page state and protection remain page granular.
    unsafe fn materialize_remote_ordinary_groups(
        process: HANDLE,
        groups: &[ForkReservation],
    ) -> Result<(), ForkError> {
        for (group_index, group) in groups.iter().enumerate() {
            if !group.ordinary_only {
                continue;
            }
            let reserved = unsafe {
                VirtualAlloc2(
                    process,
                    group.base as *const c_void,
                    group.len,
                    MEM_RESERVE | MEM_REPLACE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            if reserved as usize != group.base {
                let os_code = unsafe { GetLastError() };
                if fork_mapping_trace_enabled() {
                    eprintln!(
                        "kinakaze fork: ordinary group #{group_index} replacement failed base={:#x} len={:#x} returned={:#x} error={}",
                        group.base, group.len, reserved as usize, os_code,
                    );
                }
                super::LAST_NATIVE_FAILURE_LINE.store(line!(), Ordering::Release);
                return Err(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code,
                });
            }
        }
        Ok(())
    }

    /// Replaces one page-granular placeholder with private, lazily committed
    /// storage in another process.
    ///
    /// `VirtualAlloc2(MEM_REPLACE_PLACEHOLDER)` still requires allocation-
    /// granularity alignment for a new private reservation.  Linux VMAs are only
    /// page aligned, while a section view explicitly supports page-aligned
    /// replacement. `SEC_RESERVE` keeps its pages uncommitted until the copy loop
    /// commits exactly the runs that were committed in the parent.
    unsafe fn map_remote_private_reserve_view(
        process: HANDLE,
        address: usize,
        length: usize,
    ) -> Result<(), ForkError> {
        let section = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                ptr::null(),
                PAGE_EXECUTE_READWRITE | SEC_RESERVE,
                (length as u64 >> 32) as u32,
                length as u32,
                ptr::null(),
            )
        };
        if section.is_null() {
            return Err(os_error(ForkStage::GuestMapping));
        }
        let view = unsafe {
            MapViewOfFile3(
                section,
                process,
                address as *const c_void,
                0,
                length,
                MEM_REPLACE_PLACEHOLDER,
                // A Windows view opened without execute access cannot later
                // accept executable page protections, even when its section
                // supports them. The suspended child gets exact per-page
                // protections below before any guest instruction can execute.
                PAGE_EXECUTE_READWRITE,
                ptr::null_mut(),
                0,
            )
        };
        let os_code = if view.Value as usize == address {
            0
        } else {
            unsafe { GetLastError() }
        };
        unsafe { CloseHandle(section) };
        if view.Value as usize != address {
            if fork_mapping_trace_enabled() {
                let mut remote: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
                let _ = unsafe {
                    VirtualQueryEx(
                        process,
                        address as *const c_void,
                        &mut remote,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                eprintln!(
                    "kinakaze fork: private reserve view failed base={address:#x} len={length:#x} returned={:#x} error={} query_base={:#x} allocation_base={:#x} region_size={:#x} state={:#x}",
                    view.Value as usize,
                    os_code,
                    remote.BaseAddress as usize,
                    remote.AllocationBase as usize,
                    remote.RegionSize,
                    remote.State,
                );
            }
            super::LAST_NATIVE_FAILURE_LINE.store(line!(), Ordering::Release);
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code,
            });
        }
        Ok(())
    }

    /// Splits an enclosing remote placeholder so the target Linux mapping is
    /// one exact fragment accepted by `MEM_REPLACE_PLACEHOLDER`.
    pub(super) unsafe fn carve_remote_placeholder(
        process: HANDLE,
        address: usize,
        length: usize,
    ) -> Result<(), ForkError> {
        let target_end = address.checked_add(length).ok_or(ForkError {
            stage: ForkStage::GuestMapping,
            os_code: 0,
        })?;
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                process,
                address as *const c_void,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(os_error(ForkStage::GuestMapping));
        }
        let fragment_base = info.BaseAddress as usize;
        let fragment_end = fragment_base
            .checked_add(info.RegionSize)
            .ok_or(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            })?;
        if info.State != MEM_RESERVE || fragment_base > address || target_end > fragment_end {
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            });
        }

        if fragment_base < address
            && unsafe {
                VirtualFreeEx(
                    process,
                    fragment_base as *mut c_void,
                    address - fragment_base,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
                )
            } == 0
        {
            return Err(os_error(ForkStage::GuestMapping));
        }

        info = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                process,
                address as *const c_void,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(os_error(ForkStage::GuestMapping));
        }
        if info.BaseAddress as usize != address || info.State != MEM_RESERVE {
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            });
        }
        if info.RegionSize > length
            && unsafe {
                VirtualFreeEx(
                    process,
                    address as *mut c_void,
                    length,
                    MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
                )
            } == 0
        {
            return Err(os_error(ForkStage::GuestMapping));
        }

        // `MEM_REPLACE_PLACEHOLDER` requires one exact placeholder fragment.
        // Adjacent placeholder reservations may be coalesced by the kernel, so
        // verify the second split as well instead of letting the subsequent map
        // fail with the much less useful `ERROR_INVALID_ADDRESS`.
        info = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                process,
                address as *const c_void,
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(os_error(ForkStage::GuestMapping));
        }
        if info.BaseAddress as usize != address
            || info.RegionSize != length
            || info.State != MEM_RESERVE
        {
            if fork_mapping_trace_enabled() {
                eprintln!(
                    "kinakaze fork: carved placeholder is not exact address={address:#x} len={length:#x} query_base={:#x} allocation_base={:#x} region_size={:#x} state={:#x} protect={:#x}",
                    info.BaseAddress as usize,
                    info.AllocationBase as usize,
                    info.RegionSize,
                    info.State,
                    info.Protect,
                );
            }
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            });
        }
        Ok(())
    }

    /// Releases host-only suffixes used while installing terminal section
    /// views. They are not registered guest mappings and must not survive into
    /// the resumed Linux process.
    unsafe fn release_remote_temporary_tails(
        process: HANDLE,
        groups: &[ForkReservation],
    ) -> Result<(), ForkError> {
        for group in groups {
            if group.temporary_tail == 0 {
                continue;
            }
            let tail = group
                .base
                .checked_add(group.len)
                .and_then(|end| end.checked_sub(group.temporary_tail))
                .ok_or(ForkError {
                    stage: ForkStage::GuestMapping,
                    os_code: 0,
                })?;
            unsafe { carve_remote_placeholder(process, tail, group.temporary_tail)? };
            if unsafe { VirtualFreeEx(process, tail as *mut c_void, 0, MEM_RELEASE) } == 0 {
                return Err(os_error(ForkStage::GuestMapping));
            }
        }
        Ok(())
    }

    /// Windows COW protection describes a section, not Linux write permission.
    /// Once fork copied that view into ordinary private storage, WRITECOPY is
    /// neither meaningful nor accepted by VirtualProtectEx. Keep the Linux
    /// permissions and modifiers, without claiming anything about dirty lineage.
    fn copied_mapping_protection(storage: ForkMappingStorage, protection: u32) -> u32 {
        use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_WRITECOPY, PAGE_WRITECOPY};
        if storage.is_cow() {
            // A private COW page may be reported as RW after its first write.
            // Never grant shared write access to the original backing section.
            return (protection & !0xff)
                | match protection & 0xff {
                    PAGE_READWRITE => PAGE_WRITECOPY,
                    PAGE_EXECUTE_READWRITE => PAGE_EXECUTE_WRITECOPY,
                    other => other,
                };
        }
        // A fresh copied section has no other guest writer. Preserve its RW
        // backing contract, including stack fallbacks whose provider owner may
        // freeze it on a later fork. Installing COW page protections on an RW
        // allocation would make AllocationProtect misidentify that later state.
        if !matches!(
            storage,
            ForkMappingStorage::Ordinary | ForkMappingStorage::Section
        ) {
            return protection;
        }
        let access = match protection & 0xff {
            PAGE_WRITECOPY => PAGE_READWRITE,
            PAGE_EXECUTE_WRITECOPY => PAGE_EXECUTE_READWRITE,
            other => other,
        };
        (protection & !0xff) | access
    }

    /// Windows data sections retain their byte-exact file size. A Linux VMA
    /// includes the final partial page, but passing that page-rounded byte count
    /// to MapViewOfFile3 can fail with ERROR_ACCESS_DENIED on a read-only file.
    /// Request the actual section bytes and let the kernel round the resulting
    /// view; never shorten the registered VMA or omit a wholly backed page.
    unsafe fn retained_section_view_length(
        section: HANDLE,
        offset: u64,
        mapping_length: usize,
    ) -> Result<usize, ForkError> {
        #[repr(C)]
        struct SectionBasicInformation {
            base: *mut c_void,
            allocation_attributes: u32,
            maximum_size: i64,
        }
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtQuerySection(
                section: HANDLE,
                information_class: u32,
                information: *mut c_void,
                information_length: usize,
                return_length: *mut usize,
            ) -> i32;
            fn RtlNtStatusToDosError(status: i32) -> u32;
        }
        let mut information: SectionBasicInformation = unsafe { zeroed() };
        let status = unsafe {
            NtQuerySection(
                section,
                0,
                ptr::from_mut(&mut information).cast(),
                size_of::<SectionBasicInformation>(),
                ptr::null_mut(),
            )
        };
        if status < 0 {
            return Err(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: unsafe { RtlNtStatusToDosError(status) },
            });
        }
        let invalid = || ForkError {
            stage: ForkStage::GuestMapping,
            os_code: 87,
        };
        let maximum = u64::try_from(information.maximum_size).map_err(|_| invalid())?;
        let available = maximum.checked_sub(offset).ok_or_else(invalid)?;
        let length =
            usize::try_from(available.min(mapping_length as u64)).map_err(|_| invalid())?;
        if length == 0 {
            return Err(invalid());
        }
        if length != mapping_length {
            let mut system: SYSTEM_INFO = unsafe { zeroed() };
            unsafe { GetSystemInfo(&mut system) };
            let page = system.dwPageSize as usize;
            let rounded = length.checked_add(page - 1).ok_or_else(invalid)? & !(page - 1);
            if rounded != mapping_length {
                return Err(invalid());
            }
        }
        Ok(length)
    }

    unsafe fn copy_registered_mappings(
        process: HANDLE,
        registered: &[ForkMapping],
        reservation_groups: &[ForkReservation],
        sections: &mut [ForkSectionBacking],
    ) -> Result<ForkMappingCopyStats, ForkError> {
        let mut stats = ForkMappingCopyStats::default();
        // A section backing may have been split into several independently
        // registered views. Recreate one child-private section for the group,
        // then map every fragment from its original offset. The handle slot is
        // part of the copied arena, so patching it also makes later MAP_FIXED
        // operations in a fork child use the new child-owned backing.
        // The slice was fully sized before the allocator and sibling threads
        // were frozen. Creating host sections mutates only the prepared slots.
        for section in sections.iter_mut() {
            section.local = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE | SEC_RESERVE,
                    (section.size as u64 >> 32) as u32,
                    section.size as u32,
                    ptr::null(),
                )
            };
            if section.local.is_null() {
                return Err(os_error(ForkStage::GuestMapping));
            }
            section.staging =
                unsafe { MapViewOfFile(section.local, FILE_MAP_WRITE, 0, 0, section.size).Value };
            if section.staging.is_null() {
                return Err(os_error(ForkStage::GuestMapping));
            }
            let mut child_handle = ptr::null_mut();
            if unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    section.local,
                    process,
                    &mut child_handle,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            } == 0
            {
                return Err(os_error(ForkStage::GuestMapping));
            }
            unsafe {
                write_remote_exact(
                    process,
                    section.slot as *mut c_void,
                    ptr::addr_of!(child_handle).cast(),
                    size_of::<HANDLE>(),
                    ForkStage::GuestMapping,
                )?;
            }
        }

        unsafe { reserve_remote_mapping_groups(process, reservation_groups)? };
        unsafe { materialize_remote_ordinary_groups(process, reservation_groups)? };

        let installation_order = MappingInstallationOrder::new(registered)?;
        for mapping_index in installation_order {
            let mapping = &registered[mapping_index];
            debug_assert_ne!(mapping.behavior, ForkMappingBehavior::Omit);
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: guest mapping #{mapping_index} base={:#x} len={:#x} storage={:?} behavior={:?} backing_slot={:#x} backing_offset={:#x}",
                    mapping.base,
                    mapping.len,
                    mapping.storage,
                    mapping.behavior,
                    mapping.backing_slot,
                    mapping.backing_offset,
                );
            }
            let end = mapping.base.checked_add(mapping.len).ok_or(ForkError {
                stage: ForkStage::GuestMapping,
                os_code: 0,
            })?;
            let ordinary_group = mapping.storage == ForkMappingStorage::Ordinary
                && reservation_groups.iter().any(|group| {
                    group.ordinary_only
                        && group.base <= mapping.base
                        && group
                            .base
                            .checked_add(group.len)
                            .is_some_and(|limit| end <= limit)
                });
            let ordinary_reserve_view =
                mapping.storage == ForkMappingStorage::Ordinary && !ordinary_group;
            if mapping.storage == ForkMappingStorage::Ordinary {
                let mut remote: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
                let queried = unsafe {
                    VirtualQueryEx(
                        process,
                        mapping.base as *const c_void,
                        &mut remote,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                };
                let remote_base = remote.BaseAddress as usize;
                let remote_end = remote_base.checked_add(remote.RegionSize);
                let mapping_end = mapping.base.checked_add(mapping.len);
                let ordinary_replaces_placeholder = queried != 0
                    && remote.State == MEM_RESERVE
                    && remote_base <= mapping.base
                    && mapping_end.is_some_and(|end| remote_end.is_some_and(|limit| end <= limit));
                if !ordinary_replaces_placeholder {
                    if fork_mapping_trace_enabled() {
                        eprintln!(
                            "kinakaze fork: guest mapping #{mapping_index} unavailable: query_base={:#x} allocation_base={:#x} region_size={:#x} state={:#x} protect={:#x}",
                            remote.BaseAddress as usize,
                            remote.AllocationBase as usize,
                            remote.RegionSize,
                            remote.State,
                            remote.Protect,
                        );
                    }
                    return Err(ForkError {
                        stage: ForkStage::GuestMapping,
                        os_code: 0,
                    });
                }
            }

            let fresh_snapshot = if mapping.storage == ForkMappingStorage::AnonymousSnapshot {
                let section = unsafe {
                    (*(mapping.backing_slot as *const AtomicUsize)).load(Ordering::Acquire)
                };
                unsafe { super::anonymous_cow::snapshot(mapping, section as HANDLE)? }
            } else {
                false
            };
            match mapping.storage {
                ForkMappingStorage::Placeholder => {
                    unsafe { carve_remote_placeholder(process, mapping.base, mapping.len)? };
                    continue;
                }
                ForkMappingStorage::Section
                | ForkMappingStorage::RetainedSection
                | ForkMappingStorage::CopyOnWriteSection
                | ForkMappingStorage::AnonymousSnapshot => {
                    let (section_handle, view_protection) = if mapping.storage.retains_section() {
                        // The topology transaction pins this parent slot. Its
                        // non-inheritable child duplicate was patched into the
                        // copied arena before restoring any mapping.
                        let handle = unsafe {
                            (*(mapping.backing_slot as *const AtomicUsize)).load(Ordering::Acquire)
                        };
                        (handle as HANDLE, mapping.view_protection)
                    } else {
                        let section = sections
                            .iter()
                            .find(|section| section.slot == mapping.backing_slot)
                            .ok_or(ForkError {
                                stage: ForkStage::GuestMapping,
                                os_code: 0,
                            })?;
                        (section.local, PAGE_EXECUTE_READWRITE)
                    };
                    let view_length = if mapping.storage.retains_section() {
                        unsafe {
                            retained_section_view_length(
                                section_handle,
                                mapping.backing_offset,
                                mapping.len,
                            )?
                        }
                    } else {
                        mapping.len
                    };
                    unsafe { carve_remote_placeholder(process, mapping.base, mapping.len)? };
                    let view = unsafe {
                        MapViewOfFile3(
                            section_handle,
                            process,
                            mapping.base as *const c_void,
                            mapping.backing_offset,
                            view_length,
                            MEM_REPLACE_PLACEHOLDER,
                            view_protection,
                            ptr::null_mut(),
                            0,
                        )
                    };
                    if view.Value as usize != mapping.base {
                        let os_code = unsafe { GetLastError() };
                        if fork_mapping_trace_enabled() {
                            let mut remote: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
                            let _ = unsafe {
                                VirtualQueryEx(
                                    process,
                                    mapping.base as *const c_void,
                                    &mut remote,
                                    size_of::<MEMORY_BASIC_INFORMATION>(),
                                )
                            };
                            eprintln!(
                                "kinakaze fork: guest mapping #{mapping_index} MapViewOfFile3 failed: returned={:#x} section={:#x} error={} query_base={:#x} allocation_base={:#x} region_size={:#x} state={:#x} protect={:#x}",
                                view.Value as usize,
                                section_handle as usize,
                                os_code,
                                remote.BaseAddress as usize,
                                remote.AllocationBase as usize,
                                remote.RegionSize,
                                remote.State,
                                remote.Protect,
                            );
                        }
                        super::LAST_NATIVE_FAILURE_LINE.store(line!(), Ordering::Release);
                        return Err(ForkError {
                            stage: ForkStage::GuestMapping,
                            os_code,
                        });
                    }
                }
                ForkMappingStorage::Ordinary => {
                    if ordinary_reserve_view {
                        unsafe { carve_remote_placeholder(process, mapping.base, mapping.len)? };
                        unsafe {
                            map_remote_private_reserve_view(process, mapping.base, mapping.len)?
                        };
                    }
                }
            }

            let mut cursor = mapping.base;
            let staging = if mapping.storage == ForkMappingStorage::Section {
                let section = sections
                    .iter()
                    .find(|s| s.slot == mapping.backing_slot)
                    .ok_or(ForkError {
                        stage: ForkStage::GuestMapping,
                        os_code: 0,
                    })?;
                // The section plan checked offset + mapping length before the
                // allocator freeze; no new allocation is needed during copy.
                unsafe { section.staging.byte_add(mapping.backing_offset as usize) }
            } else {
                ptr::null_mut()
            };
            while cursor < end {
                let mut local: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
                if unsafe {
                    VirtualQuery(
                        cursor as *const c_void,
                        &mut local,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    )
                } == 0
                {
                    return Err(os_error(ForkStage::GuestMapping));
                }
                let region_end = cursor
                    .checked_add(local.RegionSize)
                    .map(|value| value.min(end))
                    .ok_or(ForkError {
                        stage: ForkStage::GuestMapping,
                        os_code: 0,
                    })?;
                let region_len = region_end - cursor;
                if local.State == MEM_COMMIT {
                    stats.committed_regions += 1;
                    if !staging.is_null() {
                        let target = unsafe { staging.byte_add(cursor - mapping.base) };
                        // SEC_RESERVE commitment is shared by its two views.
                        // Commit at the local alias, then read directly into it
                        // instead of attaching to the target for each copy.
                        if unsafe { VirtualAlloc(target, region_len, MEM_COMMIT, PAGE_READWRITE) }
                            != target
                        {
                            return Err(os_error(ForkStage::GuestMapping));
                        }
                    } else if ordinary_group || ordinary_reserve_view {
                        let committed = unsafe {
                            VirtualAllocEx(
                                process,
                                cursor as *const c_void,
                                region_len,
                                MEM_COMMIT,
                                PAGE_READWRITE,
                            )
                        };
                        if committed as usize != cursor {
                            let os_code = unsafe { GetLastError() };
                            if fork_mapping_trace_enabled() {
                                eprintln!(
                                    "kinakaze fork: guest mapping #{mapping_index} commit failed at {cursor:#x} len={region_len:#x}: returned={:#x} error={}",
                                    committed as usize, os_code,
                                );
                            }
                            super::LAST_NATIVE_FAILURE_LINE.store(line!(), Ordering::Release);
                            return Err(ForkError {
                                stage: ForkStage::GuestMapping,
                                os_code,
                            });
                        }
                    }
                    if mapping.behavior == ForkMappingBehavior::Copy
                        && mapping.storage != ForkMappingStorage::RetainedSection
                    {
                        let (copied, calls) = if fresh_snapshot {
                            stats.cow_shared_bytes += region_len;
                            (0, 0)
                        } else if mapping.storage.is_cow() {
                            let copy = |offset, length| unsafe {
                                copy_protected_mapping(
                                    process,
                                    cursor + offset,
                                    length,
                                    local.Protect,
                                    ptr::null_mut(),
                                )
                            };
                            let cow = if mapping.storage == ForkMappingStorage::AnonymousSnapshot {
                                unsafe {
                                    super::anonymous_cow::copy_private_pages(
                                        cursor,
                                        region_len,
                                        local.Protect,
                                        copy,
                                    )
                                }?
                            } else {
                                unsafe { super::cow::copy_private_pages(cursor, region_len, copy) }?
                            };
                            stats.cow_shared_bytes += cow.shared;
                            (cow.copied, cow.calls)
                        } else {
                            unsafe {
                                copy_protected_mapping(
                                    process,
                                    cursor,
                                    region_len,
                                    local.Protect,
                                    if staging.is_null() {
                                        ptr::null_mut()
                                    } else {
                                        staging.byte_add(cursor - mapping.base)
                                    },
                                )?;
                            }
                            (region_len, 1)
                        };
                        stats.copied_bytes += copied;
                        match mapping.domain {
                            super::ForkMappingDomain::GuestMm => {
                                stats.guest_mm_copied_bytes += copied
                            }
                            super::ForkMappingDomain::HostPrivate => {
                                stats.host_private_copied_bytes += copied
                            }
                        }
                        stats.copy_calls += calls;
                        if !staging.is_null() {
                            stats.section_bytes += region_len;
                        }
                    }
                    let mut previous = 0u32;
                    let protection = copied_mapping_protection(mapping.storage, local.Protect);
                    if unsafe {
                        VirtualProtectEx(
                            process,
                            cursor as *const c_void,
                            region_len,
                            protection,
                            &mut previous,
                        )
                    } == 0
                    {
                        let os_code = unsafe { GetLastError() };
                        if fork_mapping_trace_enabled() {
                            eprintln!(
                                "kinakaze fork: guest mapping #{mapping_index} protect failed at {cursor:#x} len={region_len:#x} protection={:#x}: error={}",
                                protection, os_code,
                            );
                        }
                        super::LAST_NATIVE_FAILURE_LINE.store(line!(), Ordering::Release);
                        return Err(ForkError {
                            stage: ForkStage::GuestMapping,
                            os_code,
                        });
                    }
                }
                cursor = region_end;
            }

            // WriteProcessMemory makes the bytes visible in the child, but the
            // Windows contract still requires an explicit instruction-cache
            // flush before newly written executable mappings are entered.  Do
            // this for every copied mapping: protection can vary by subregion,
            // and the syscall/JIT page that resumes the fork child is itself
            // one of these generic registered mappings.
            if mapping.behavior == ForkMappingBehavior::Copy
                && mapping.storage != ForkMappingStorage::RetainedSection
                && unsafe {
                    FlushInstructionCache(process, mapping.base as *const c_void, mapping.len)
                } == 0
            {
                return Err(os_error(ForkStage::GuestMapping));
            }
        }
        unsafe { release_remote_temporary_tails(process, reservation_groups)? };
        Ok(stats)
    }

    unsafe fn copy_native_stack(
        process: HANDLE,
        thread: HANDLE,
        context: &CONTEXT,
        arena_snapshot: &kinakaze_alloc::ArenaSnapshot,
        registered: &[ForkMapping],
    ) -> Result<(), ForkError> {
        let saved_rsp = context.Rsp as usize;

        // A compatibility call may run on a managed ABI-transition stack or a
        // runtime's movable/user-mode stack instead of the Windows thread stack
        // recorded in the TEB. Both the arena and registered guest mappings were
        // copied in preceding phases, so reserving the enormous numeric interval
        // between either one and TEB.StackBase would be incorrect. The child
        // keeps its normal TEB bounds and resumes on the restored alternate stack.
        let arena_contains_stack = (arena_snapshot.base as usize)
            .checked_add(arena_snapshot.committed_len)
            .is_some_and(|end| arena_snapshot.base as usize <= saved_rsp && saved_rsp < end);
        let mapping_contains_stack = registered.iter().any(|mapping| {
            mapping.behavior == ForkMappingBehavior::Copy
                && mapping
                    .base
                    .checked_add(mapping.len)
                    .is_some_and(|end| mapping.base <= saved_rsp && saved_rsp < end)
        });
        if arena_contains_stack || mapping_contains_stack {
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: resume stack {saved_rsp:#x} is contained in the copied {}",
                    if arena_contains_stack {
                        "managed arena"
                    } else {
                        "guest mapping"
                    }
                );
            }
            return Ok(());
        }

        let mut parent_info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        if unsafe {
            VirtualQuery(
                saved_rsp as *const c_void,
                &mut parent_info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            let os_code = unsafe { GetLastError() };
            if fork_trace_enabled() {
                eprintln!(
                    "kinakaze fork: parent stack query failed rsp={saved_rsp:#x} error={}",
                    os_code
                );
            }
            return Err(ForkError {
                stage: ForkStage::ParentStack,
                os_code,
            });
        }
        let allocation_base = parent_info.AllocationBase as usize;
        let stack_base: usize;
        let stack_limit: usize;
        unsafe {
            asm!(
                "mov {base}, gs:[0x08]",
                "mov {limit}, gs:[0x10]",
                base = out(reg) stack_base,
                limit = out(reg) stack_limit,
                options(nostack, preserves_flags),
            )
        };
        if fork_trace_enabled() {
            eprintln!(
                "kinakaze fork: parent stack rsp={saved_rsp:#x} allocation_base={allocation_base:#x} region_base={:#x} region_size={:#x} stack_limit={stack_limit:#x} stack_base={stack_base:#x}",
                parent_info.BaseAddress as usize, parent_info.RegionSize,
            );
        }
        if allocation_base >= saved_rsp || saved_rsp < stack_limit || saved_rsp >= stack_base {
            return Err(ForkError {
                stage: ForkStage::ParentStack,
                os_code: 0,
            });
        }
        let reserve_len = stack_base - allocation_base;
        unsafe {
            ensure_remote_mapping(process, allocation_base, reserve_len, ForkStage::ChildStack)?;
        }

        let page_size = 4096usize;
        let copy_start = saved_rsp & !(page_size - 1);
        unsafe {
            write_remote_exact(
                process,
                copy_start as *mut c_void,
                copy_start as *const c_void,
                stack_base - copy_start,
                ForkStage::StackCopy,
            )?;
        }

        let mut basic: ThreadBasicInformation = unsafe { zeroed() };
        let status = unsafe {
            NtQueryInformationThread(
                thread,
                0,
                ptr::addr_of_mut!(basic).cast(),
                size_of::<ThreadBasicInformation>() as u32,
                ptr::null_mut(),
            )
        };
        if status < 0 {
            return Err(ForkError {
                stage: ForkStage::ChildTeb,
                os_code: status as u32,
            });
        }
        unsafe {
            write_remote_exact(
                process,
                basic.teb_base_address.byte_add(0x08),
                ptr::addr_of!(stack_base).cast(),
                size_of::<usize>(),
                ForkStage::ChildTeb,
            )?;
            write_remote_exact(
                process,
                basic.teb_base_address.byte_add(0x10),
                ptr::addr_of!(allocation_base).cast(),
                size_of::<usize>(),
                ForkStage::ChildTeb,
            )?;
        }
        Ok(())
    }

    unsafe fn ensure_remote_mapping(
        process: HANDLE,
        address: usize,
        len: usize,
        stage: ForkStage,
    ) -> Result<(), ForkError> {
        let mut first: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                process,
                address as *const c_void,
                &mut first,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(os_error(stage));
        }

        if first.State == MEM_FREE {
            if first.RegionSize < len {
                return Err(ForkError { stage, os_code: 0 });
            }
            let mapped = unsafe {
                VirtualAllocEx(
                    process,
                    address as *const c_void,
                    len,
                    MEM_RESERVE | MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            if mapped as usize != address {
                return Err(os_error(stage));
            }
            return Ok(());
        }

        let mut last: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        if unsafe {
            VirtualQueryEx(
                process,
                (address + len - 1) as *const c_void,
                &mut last,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
            || first.AllocationBase as usize != address
            || last.AllocationBase as usize != address
        {
            return Err(ForkError { stage, os_code: 0 });
        }

        let mapped = unsafe {
            VirtualAllocEx(
                process,
                address as *const c_void,
                len,
                MEM_COMMIT,
                PAGE_READWRITE,
            )
        };
        if mapped as usize != address {
            return Err(os_error(stage));
        }
        Ok(())
    }

    unsafe fn read_remote_exact(
        process: HANDLE,
        source: *const c_void,
        destination: *mut c_void,
        len: usize,
        stage: ForkStage,
    ) -> Result<(), ForkError> {
        let mut read = 0;
        if unsafe { ReadProcessMemory(process, source, destination, len, &mut read) } == 0
            || read != len
        {
            Err(os_error(stage))
        } else {
            Ok(())
        }
    }

    /// Access protection does not discard a private page's bytes. The parent
    /// and its allocator are frozen here, so briefly allow reads of inaccessible
    /// or guarded runs, then restore protection on both success and failure.
    unsafe fn copy_protected_mapping(
        process: HANDLE,
        address: usize,
        len: usize,
        protection: u32,
        staging: *mut c_void,
    ) -> Result<(), ForkError> {
        use windows_sys::Win32::System::Memory::{
            PAGE_EXECUTE, PAGE_GUARD, PAGE_READONLY, VirtualProtect,
        };
        let changed = protection & PAGE_GUARD != 0
            || matches!(protection & 0xff, PAGE_NOACCESS | PAGE_EXECUTE);
        let mut previous = 0;
        if changed
            && unsafe { VirtualProtect(address as _, len, PAGE_READONLY, &mut previous) } == 0
        {
            return Err(os_error(ForkStage::GuestMapping));
        }
        let outcome = if staging.is_null() {
            unsafe {
                write_remote_exact(
                    process,
                    address as _,
                    address as _,
                    len,
                    ForkStage::GuestMapping,
                )
            }
        } else {
            unsafe {
                read_remote_exact(
                    GetCurrentProcess(),
                    address as _,
                    staging,
                    len,
                    ForkStage::GuestMapping,
                )
            }
        };
        if changed {
            let mut discarded = 0;
            if unsafe { VirtualProtect(address as _, len, previous, &mut discarded) } == 0 {
                // As for the managed arena, never resume a parent with altered
                // protection after a failed restoration of its memory contract.
                unsafe { TerminateProcess(process, 127) };
                std::process::abort();
            }
        }
        outcome
    }

    unsafe fn write_remote_exact(
        process: HANDLE,
        destination: *mut c_void,
        source: *const c_void,
        len: usize,
        stage: ForkStage,
    ) -> Result<(), ForkError> {
        let mut written = 0;
        if unsafe { WriteProcessMemory(process, destination, source, len, &mut written) } == 0
            || written != len
        {
            let failure = os_error(stage);
            if fork_mapping_trace_enabled() {
                eprintln!(
                    "kinakaze fork: copy failed stage={stage:?} source={source:p} destination={destination:p} len={len:#x} copied={written:#x} error={}",
                    failure.os_code
                );
            }
            Err(failure)
        } else {
            Ok(())
        }
    }

    pub(super) fn last_fork_error() -> ForkError {
        ForkError {
            stage: decode_stage(LAST_STAGE.load(Ordering::Acquire)),
            os_code: LAST_OS_ERROR.load(Ordering::Acquire),
        }
    }

    /// Linux `waitpid` over the children this coordinator created.
    ///
    /// Four selectors, all of which a shell uses: one pid, any child (`-1`),
    /// any child in the caller's own process group (`0`), and any child in a
    /// named group (`< -1`). The group forms need the process-group registry,
    /// because a Windows process handle carries no notion of a group.
    ///
    /// `WUNTRACED` and `WCONTINUED` are answered from that same registry. A
    /// stopped child is still a running Windows process — `GetExitCodeProcess`
    /// reports `STILL_ACTIVE` — so the only way for a parent to learn that its
    /// child stopped is the report the child files before suspending itself.
    ///
    /// # Divergence
    ///
    /// A child holds one pending report at a time. A stop immediately followed
    /// by a continue overwrites the stop, so a parent polling slowly with
    /// `WUNTRACED` can miss the intermediate state. Linux queues these per
    /// child; a fixed-size shared section cannot, and a bounded queue would
    /// trade a rare missed notification for a hard limit on how far behind a
    /// parent may fall, which is worse.
    pub(super) unsafe fn wait_child(pid: i32, options: i32, events: i32, status: *mut i32) -> i32 {
        let pid = match pid {
            value if value > 0 => match super::job::namespaces::resolve(value as u32) {
                Some(id) => id as i32,
                None => return -10,
            },
            value if value < -1 => match value
                .checked_neg()
                .and_then(|id| super::job::namespaces::resolve(id as u32))
            {
                Some(id) => -(id as i32),
                None => return -10,
            },
            value => value,
        };

        const ECHILD: i32 = 10;
        const EIO: i32 = 5;
        const EINVAL: i32 = 22;
        // Windows caps one wait at 64 objects; beyond that the poll below is
        // what notices an exit, a few milliseconds later.
        const MAX_WAIT_HANDLES: usize = 64;
        // Long enough that the loop is not a spin, short enough that a stop
        // report reaches a parent faster than a person can notice.
        const POLL_MS: u32 = 10;

        if options & !(WNOHANG | WNOWAIT) != 0
            || events == 0
            || events & !(WAIT_EVENT_EXITED | WAIT_EVENT_STOPPED | WAIT_EVENT_CONTINUED) != 0
        {
            return -EINVAL;
        }
        if fork_trace_enabled() {
            eprintln!(
                "kinakaze: wait coordinator pid {} request child={pid} options={options:#x}",
                std::process::id()
            );
        }
        'rescan: loop {
            let candidates = match collect_wait_candidates(pid) {
                Ok(candidates) => candidates,
                Err(error) => return -error,
            };
            if candidates.is_empty() {
                if wait_trace_enabled() {
                    eprintln!(
                        "kinakaze wait: pid={} selector={pid} has no children",
                        std::process::id(),
                    );
                }
                return -ECHILD;
            }

            // A termination report is shared process state rather than a
            // process-local handle observation. This is essential for
            // CLONE_PARENT: the creator and the Linux parent are different
            // processes, while only the latter may reap the child.
            for candidate in &candidates {
                if events & WAIT_EVENT_EXITED == 0 {
                    break;
                }
                let child_pid = candidate.entry.namespace_pid;
                let Some(change) = super::job::peek_report(child_pid) else {
                    continue;
                };
                if super::job::lookup(child_pid).is_some_and(|current| {
                    current.pid != candidate.entry.pid || current.token != candidate.entry.token
                }) {
                    continue 'rescan;
                }
                // `_exit` publishes the exact Linux status immediately before
                // terminating the host process.  Do not make that status
                // waitable until the process object is signalled: Linux tears
                // down the child's descriptor table before wait(2) can reap
                // it.  Returning in the publication window lets a parent enter
                // os/exec's pipe-drain phase while an exec wrapper still owns
                // inherited write handles, suppressing the EOF that phase is
                // waiting for.
                //
                // A missing handle means the process was already gone when an
                // exact handle was opened.  The shared zombie report is then
                // sufficient; it is never replaced with a guessed host status.
                match child_process_has_exited(*candidate) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(error) => return -error,
                }
                if wait_trace_enabled() {
                    eprintln!(
                        "kinakaze wait: pid={} child={child_pid} observed report={change:?}",
                        std::process::id(),
                    );
                }
                let encoded = match exit_status(change) {
                    Ok(Some(encoded)) => encoded,
                    Ok(None) => continue,
                    Err(()) => return -EIO,
                };
                let visible_pid = super::job::namespaces::visible(child_pid).unwrap_or(0);
                if options & WNOWAIT == 0 {
                    // Atomically claim the report. Another waiter may have
                    // consumed it after the peek above, in which case this
                    // candidate must be reconsidered instead of being reported
                    // twice.
                    let Some(claimed) = super::job::take_report(child_pid) else {
                        continue;
                    };
                    let claimed = match exit_status(claimed) {
                        Ok(Some(value)) => value,
                        Ok(None) => continue,
                        // An unobservable host failure is deliberately retained
                        // for diagnosis; it is never converted into a status.
                        Err(()) => {
                            super::job::post_report(
                                child_pid,
                                super::job::StateChange::Unobservable,
                            );
                            return -EIO;
                        }
                    };
                    if claimed != encoded {
                        return -EIO;
                    }
                    let removed = super::child_processes()
                        .lock()
                        .map_err(|_| EIO)
                        .and_then(|mut children| Ok(children.remove(child_pid)));
                    let removed = match removed {
                        Ok(handle) => handle,
                        Err(error) => return -error,
                    };
                    if let Some(handle) = removed {
                        // SAFETY: the registry handed over its owned reference.
                        unsafe { CloseHandle(handle as HANDLE) };
                    }
                    forget_child(child_pid);
                }
                if !status.is_null() {
                    // SAFETY: the caller supplied writable status storage.
                    unsafe { status.write(encoded) };
                }
                if fork_trace_enabled() || wait_trace_enabled() {
                    eprintln!(
                        "kinakaze: wait coordinator reaped pid {child_pid} status={encoded:#x}"
                    );
                }
                return visible_pid as i32;
            }

            // A signalled host handle with no published termination report is
            // not enough information to invent Linux wait status. Preserve an
            // explicit integrity failure in the shared row.
            for candidate in &candidates {
                let Some(handle) = candidate.handle else {
                    continue;
                };
                // SAFETY: the process-local registry owns this live handle.
                if unsafe { WaitForSingleObject(handle as HANDLE, 0) } == WAIT_OBJECT_0
                    && super::job::peek_report(candidate.entry.namespace_pid).is_none()
                {
                    if super::job::lookup(candidate.entry.namespace_pid).is_some_and(|current| {
                        current.pid != candidate.entry.pid || current.token != candidate.entry.token
                    }) {
                        continue 'rescan;
                    }
                    if wait_trace_enabled() {
                        eprintln!(
                            "kinakaze wait: pid={} child={} host handle signalled without a shared exit report",
                            std::process::id(),
                            candidate.entry.namespace_pid,
                        );
                    }
                    super::job::post_report(
                        candidate.entry.namespace_pid,
                        super::job::StateChange::Unobservable,
                    );
                    return -EIO;
                }
            }

            // A stop or a continue leaves the process running, so it is only
            // visible through the report the child filed for its parent.
            if events & (WAIT_EVENT_STOPPED | WAIT_EVENT_CONTINUED) != 0 {
                for candidate in &candidates {
                    let child_pid = candidate.entry.namespace_pid;
                    let owner =
                        super::job::resolve(child_pid).map_or(child_pid, |e| e.namespace_pid);
                    let encoded = match super::job::peek_report(owner) {
                        Some(super::job::StateChange::Stopped(signal))
                            if events & WAIT_EVENT_STOPPED != 0 =>
                        {
                            // Linux packs a stop as 0x7f in the low byte with
                            // the signal above it.
                            Some(((signal as i32 & 0xff) << 8) | 0x7f)
                        }
                        Some(super::job::StateChange::Continued)
                            if events & WAIT_EVENT_CONTINUED != 0 =>
                        {
                            // The one status value reserved for a continue.
                            Some(0xffff)
                        }
                        _ => None,
                    };
                    let Some(encoded) = encoded else { continue };
                    if options & WNOWAIT == 0 {
                        super::job::take_report(owner);
                    }
                    if !status.is_null() {
                        // SAFETY: the caller supplied writable status storage.
                        unsafe { status.write(encoded) };
                    }
                    return super::job::namespaces::visible(child_pid).unwrap_or(0) as i32;
                }
            }

            if options & WNOHANG != 0 {
                return 0;
            }

            // Wait on the children themselves so an exit is noticed at once,
            // with a timeout so a stop report is noticed shortly after.
            let handles: Vec<HANDLE> = candidates
                .iter()
                .filter_map(|candidate| candidate.handle)
                .take(MAX_WAIT_HANDLES)
                .map(|handle| handle as HANDLE)
                .collect();
            if handles.is_empty() {
                // Shared zombie/report state has no process-local wait handle.
                // Polling is the synchronization mechanism for that case, not
                // a guessed compatibility path.
                std::thread::sleep(std::time::Duration::from_millis(u64::from(POLL_MS)));
            } else {
                // SAFETY: every handle came from the process-local registry and
                // stays open for the duration of this wait.
                unsafe {
                    WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, POLL_MS)
                };
            }
        }
    }

    /// Namespace and other named sections must be owned by the child before
    /// the creator can exit. Socket parent hooks run first to avoid a circular
    /// wait with WSADuplicateSocket reconstruction in the child.
    pub(super) fn wait_fork_handoff(pid: u32) -> Result<(), ForkError> {
        use super::job::fork_handoff::Completion;
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        let mut handle = ptr::null_mut();
        let duplicated = super::child_processes().lock().is_ok_and(|children| {
            children.get(pid).is_some_and(|raw| unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    raw as HANDLE,
                    GetCurrentProcess(),
                    &mut handle,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                ) != 0
            })
        });
        if !duplicated {
            return Err(ForkError {
                stage: ForkStage::HandoffRestore,
                os_code: 5,
            });
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
        let began = std::time::Instant::now();
        let profile = fork_timings_enabled();
        if profile {
            super::fork_timing_line(format_args!(
                "kinakaze: handoff wait begin guest_pid={pid} child_host={} utc_us={}",
                unsafe {
                    windows_sys::Win32::System::Threading::GetProcessId(handle.as_raw_handle())
                },
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros(),
            ));
        }
        let completion = super::job::fork_handoff::wait(pid, handle.as_raw_handle(), 30_000);
        if profile {
            super::fork_timing_line(format_args!(
                "kinakaze: handoff wait end guest_pid={pid} elapsed_us={} completion={completion:?}",
                began.elapsed().as_micros(),
            ));
        }
        let result = match completion {
            Ok(Completion::Restored(0) | Completion::Exited) => Ok(()),
            other => {
                let error = match other {
                    Ok(Completion::Restored(error)) => ForkError {
                        stage: ForkStage::Unsupported,
                        os_code: error,
                    },
                    Err(error) => {
                        unsafe {
                            TerminateProcess(handle.as_raw_handle(), 127);
                        }
                        ForkError {
                            stage: ForkStage::HandoffRestore,
                            os_code: error,
                        }
                    }
                    _ => unreachable!(),
                };
                unsafe {
                    WaitForSingleObject(handle.as_raw_handle(), 1000);
                }
                // A failed creation is never exposed to guest wait(2). Retire
                // its callback and original process handle as well as its row.
                if let Ok(mut children) = super::child_processes().lock() {
                    if let Some(raw) = children.remove(pid) {
                        unsafe {
                            CloseHandle(raw as HANDLE);
                        }
                    }
                }
                forget_child(pid);
                Err(error)
            }
        };
        result
    }

    /// A manager-rejected creation is not a child visible to guest wait(2).
    pub(super) fn discard_failed_fork(pid: u32) {
        let raw = super::child_processes()
            .lock()
            .ok()
            .and_then(|mut children| children.remove(pid));
        if let Some(raw) = raw {
            unsafe {
                TerminateProcess(raw as HANDLE, 127);
                WaitForSingleObject(raw as HANDLE, FORK_READY_TIMEOUT_MS);
                CloseHandle(raw as HANDLE);
            }
        }
        forget_child(pid);
    }

    /// The shared children matching a `waitpid` selector.
    fn collect_wait_candidates(pid: i32) -> Result<Vec<WaitCandidate>, i32> {
        const EIO: i32 = 5;
        let own = super::job::lookup_host(unsafe {
            windows_sys::Win32::System::Threading::GetCurrentProcessId()
        })
        .ok_or(EIO)?;
        let rows = super::job::children_of(own.namespace_pid).map_err(|()| EIO)?;
        // `pid == 0` means the caller's own group, which is the selector a shell
        // uses to wait for one pipeline without touching the others.
        let wanted_group = match pid {
            0 => Some(own.pgid),
            group if group < -1 => Some((-(i64::from(group))) as u32),
            _ => None,
        };
        let selected = rows.into_iter().filter(|entry| match pid {
            target if target > 0 => entry.namespace_pid == target as u32,
            -1 => true,
            _ => wanted_group.is_some_and(|group| entry.pgid == group),
        });

        let mut children = super::child_processes().lock().map_err(|_| EIO)?;
        let mut candidates = Vec::new();
        for entry in selected {
            if let Some(handle) = children.get(entry.namespace_pid) {
                // Exec retires its old worker immediately. A retained handle
                // for that worker says nothing about the replacement image's
                // exit status; rebind before any signalled-handle observation.
                let actual = unsafe {
                    windows_sys::Win32::System::Threading::GetProcessId(handle as HANDLE)
                };
                if actual != entry.pid {
                    if let Some(old) = children.remove(entry.namespace_pid) {
                        unsafe { CloseHandle(old as HANDLE) };
                    }
                }
            }
            let handle = if let Some(handle) = children.get(entry.namespace_pid) {
                Some(handle)
            } else {
                // A termination report is published just before the native
                // process object becomes signalled.  Open the exact live object
                // even for a zombie row so wait(2) observes resource teardown,
                // not merely status publication.  If it is already gone,
                // `open_exact_child` validates the retained zombie and returns
                // no handle.
                let Some(handle) = open_exact_child(entry)? else {
                    candidates.push(WaitCandidate {
                        entry,
                        handle: None,
                    });
                    continue;
                };
                if !children.insert(entry.namespace_pid, handle) {
                    // SAFETY: this call owns the handle until insertion succeeds.
                    unsafe { CloseHandle(handle as HANDLE) };
                    return Err(EIO);
                }
                Some(handle)
            };
            candidates.push(WaitCandidate { entry, handle });
        }
        Ok(candidates)
    }

    /// Whether a reported child has crossed the kernel-visible exit boundary.
    ///
    /// The report contains the only unambiguous Linux status, while the process
    /// handle is the authority for completion of descriptor and handle teardown.
    /// Both facts are required when both are available.
    fn child_process_has_exited(candidate: WaitCandidate) -> Result<bool, i32> {
        const EIO: i32 = 5;
        let Some(handle) = candidate.handle else {
            return Ok(true);
        };
        match unsafe { WaitForSingleObject(handle as HANDLE, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(EIO),
            _ => Err(EIO),
        }
    }

    /// Opens exactly the host process named by one shared row and rejects PID
    /// recycling by comparing the process creation token before publishing the
    /// handle locally.
    fn open_exact_child(entry: super::job::Entry) -> Result<Option<usize>, i32> {
        const EIO: i32 = 5;
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                entry.pid,
            )
        };
        if handle.is_null() {
            // A normal child publishes its zombie report before termination.
            // Re-read only to resolve that ordering race; no host identity or
            // status is guessed when both sources are absent.
            return match super::job::lookup(entry.namespace_pid) {
                Some(current)
                    if current.pid == entry.pid
                        && current.token == entry.token
                        && current.flags & super::job::FLAG_ZOMBIE != 0 =>
                {
                    Ok(None)
                }
                _ => Err(EIO),
            };
        }
        let mut created = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut ignored = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let ok = unsafe {
            GetProcessTimes(
                handle,
                &mut created,
                &mut ignored,
                &mut ignored,
                &mut ignored,
            )
        };
        let token = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        if ok == 0 || token != entry.token {
            unsafe { CloseHandle(handle) };
            return Err(EIO);
        }
        Ok(Some(handle as usize))
    }

    /// Encodes a reaped child's Linux wait status.
    ///
    /// A Windows exit code cannot say whether the process chose it or a signal
    /// imposed it: `exit(130)` and death by `SIGINT` leave the same number. The
    /// registry carries the distinction, written by the dying process itself,
    /// and that is what turns an exit code into `WIFSIGNALED`.
    fn exit_status(change: super::job::StateChange) -> Result<Option<i32>, ()> {
        match change {
            // WIFSIGNALED: the signal number sits in the low seven bits.
            super::job::StateChange::Killed(signal) => Ok(Some(signal as i32 & 0x7f)),
            // WIFEXITED: an ordinary exit code lives in bits 8..15.
            super::job::StateChange::Exited(status) => Ok(Some(((status & 0xff) as i32) << 8)),
            super::job::StateChange::Unobservable => Err(()),
            _ => Ok(None),
        }
    }

    /// Releases the registry slots a reaped child leaves behind.
    fn forget_child(child: u32) {
        if let Some(entry) = super::job::lookup(child)
            && entry.delegate != 0
        {
            // An `exec`ed child owns two slots: the wrapper the parent waited on
            // and the replacement that did the work.
            super::job::release_slot(entry.delegate);
        }
        super::job::release_slot(child);
    }

    fn set_error(stage: ForkStage, os_code: u32) {
        LAST_OS_ERROR.store(os_code, Ordering::Release);
        LAST_STAGE.store(stage as u32, Ordering::Release);
    }

    #[track_caller]
    fn os_error(stage: ForkStage) -> ForkError {
        super::LAST_NATIVE_FAILURE_LINE
            .store(std::panic::Location::caller().line(), Ordering::Release);
        ForkError {
            stage,
            os_code: unsafe { GetLastError() },
        }
    }

    fn decode_stage(value: u32) -> ForkStage {
        const STAGES: &[ForkStage] = &[
            ForkStage::Allocator,
            ForkStage::CreateEvent,
            ForkStage::ExecutablePath,
            ForkStage::CreateProcess,
            ForkStage::ChildBootstrap,
            ForkStage::SuspendChild,
            ForkStage::ImageLayout,
            ForkStage::ArenaMapping,
            ForkStage::ArenaCopy,
            ForkStage::ParentStack,
            ForkStage::ChildStack,
            ForkStage::StackCopy,
            ForkStage::ChildTeb,
            ForkStage::ThreadContext,
            ForkStage::ResumeChild,
            ForkStage::HandoffPrepare,
            ForkStage::HandoffStage,
            ForkStage::HandoffRestore,
            ForkStage::ModuleManifest,
            ForkStage::ThreadFreeze,
            ForkStage::GuestMapping,
            ForkStage::Unsupported,
        ];
        STAGES
            .iter()
            .copied()
            .find(|stage| *stage as u32 == value)
            .unwrap_or(ForkStage::Unsupported)
    }

    fn append_ascii(buffer: &mut [u16], cursor: &mut usize, text: &str) -> Result<(), ForkError> {
        for byte in text.bytes() {
            if *cursor + 1 >= buffer.len() {
                return Err(ForkError {
                    stage: ForkStage::CreateProcess,
                    os_code: 0,
                });
            }
            buffer[*cursor] = byte as u16;
            *cursor += 1;
        }
        Ok(())
    }

    fn append_utf16(buffer: &mut [u16], cursor: &mut usize, text: &[u16]) -> Result<(), ForkError> {
        for &unit in text {
            if *cursor + 1 >= buffer.len() {
                return Err(ForkError {
                    stage: ForkStage::CreateProcess,
                    os_code: 0,
                });
            }
            buffer[*cursor] = unit;
            *cursor += 1;
        }
        Ok(())
    }

    fn append_decimal(
        buffer: &mut [u16],
        cursor: &mut usize,
        mut value: usize,
    ) -> Result<(), ForkError> {
        let mut digits = [0u16; 20];
        let mut count = 0;
        loop {
            digits[count] = (value % 10) as u16 + b'0' as u16;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        while count != 0 {
            count -= 1;
            append_utf16(buffer, cursor, &digits[count..=count])?;
        }
        Ok(())
    }

    pub(super) unsafe fn park_fork_loader_if_requested(allow_runtime_dll: bool) {
        let executable = unsafe { GetModuleHandleW(ptr::null()) };
        if executable.is_null()
            || (!allow_runtime_dll
                && module_for_address(park_fork_loader_if_requested as *const () as usize)
                    != Some(executable as usize))
        {
            return;
        }
        let Some((ready, bootstrap)) = (unsafe { parse_loader_handles() }) else {
            return;
        };
        if unsafe { load_bootstrap_modules(bootstrap) }.is_err() {
            unsafe { ExitProcess(125) };
        }
        if unsafe { SetEvent(ready) } == 0 {
            unsafe { ExitProcess(126) };
        }
        unsafe { CloseHandle(ready) };
        // Remain in user mode so `SuspendThread` can stop the loader thread
        // immediately. A non-alertable kernel wait can defer suspension until
        // its object is signalled, which is too late for context replacement.
        loop {
            core::hint::spin_loop();
        }
    }

    unsafe fn load_bootstrap_modules(mapping: HANDLE) -> Result<(), ()> {
        // SAFETY: the handle was inherited from the parent with read access.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            return Err(());
        }
        let base = view.Value.cast::<u8>();
        // Read the fixed header before trusting its total length.
        let magic = unsafe { ptr::read_unaligned(base.cast::<u64>()) };
        let total = unsafe { ptr::read_unaligned(base.add(8).cast::<u64>()) } as usize;
        let count = unsafe { ptr::read_unaligned(base.add(16).cast::<u32>()) } as usize;
        if magic != BOOTSTRAP_MAGIC || !(32..=16 * 1024 * 1024).contains(&total) {
            unsafe {
                UnmapViewOfFile(view);
                CloseHandle(mapping);
            }
            return Err(());
        }
        let tls_slots = unsafe { ptr::read_unaligned(base.add(24).cast::<u64>()) };
        unsafe { reserve_bootstrap_tls_slots(tls_slots)? };
        BOOTSTRAPPING.store(true, Ordering::Release);
        let mut cursor = 32usize;
        for _ in 0..count {
            if cursor.checked_add(16).is_none_or(|end| end > total) {
                BOOTSTRAPPING.store(false, Ordering::Release);
                unsafe {
                    UnmapViewOfFile(view);
                    CloseHandle(mapping);
                }
                return Err(());
            }
            let expected = unsafe { ptr::read_unaligned(base.add(cursor).cast::<u64>()) } as usize;
            let units = unsafe { ptr::read_unaligned(base.add(cursor + 8).cast::<u32>()) } as usize;
            let path_start = cursor + 16;
            let path_end = match path_start.checked_add(units.saturating_mul(2)) {
                Some(end) if end <= total && units != 0 => end,
                _ => {
                    BOOTSTRAPPING.store(false, Ordering::Release);
                    unsafe {
                        UnmapViewOfFile(view);
                        CloseHandle(mapping);
                    }
                    return Err(());
                }
            };
            let path = unsafe { base.add(path_start).cast::<u16>() };
            if unsafe { *path.add(units - 1) } != 0 {
                BOOTSTRAPPING.store(false, Ordering::Release);
                unsafe {
                    UnmapViewOfFile(view);
                    CloseHandle(mapping);
                }
                return Err(());
            }
            // Windows initializes the DLL through its loader.  Its PE sections,
            // loader locks, TLS and kernel objects are never copied from parent.
            let loaded = unsafe {
                windows_sys::Win32::System::LibraryLoader::LoadLibraryExW(
                    path,
                    ptr::null_mut(),
                    windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
                        | windows_sys::Win32::System::LibraryLoader::LOAD_LIBRARY_SEARCH_SYSTEM32,
                )
            };
            if loaded.is_null() || loaded as usize != expected {
                if crate::fork_trace_enabled() {
                    eprintln!(
                        "kinakaze fork: native restore expected={expected:#x} loaded={loaded:p} error={}",
                        unsafe { GetLastError() }
                    );
                }
                BOOTSTRAPPING.store(false, Ordering::Release);
                unsafe {
                    UnmapViewOfFile(view);
                    CloseHandle(mapping);
                }
                return Err(());
            }
            cursor = path_end.next_multiple_of(8);
        }
        BOOTSTRAPPING.store(false, Ordering::Release);
        unsafe {
            UnmapViewOfFile(view);
            CloseHandle(mapping);
        }
        Ok(())
    }

    /// Recreates the parent's executable-owned TLS allocation prefix before
    /// provider DLL initialization can consume any of those indices.
    unsafe fn reserve_bootstrap_tls_slots(wanted: u64) -> Result<(), ()> {
        use windows_sys::Win32::System::Threading::{TLS_OUT_OF_INDEXES, TlsAlloc};

        if wanted == 0 {
            super::FORK_TLS_SLOT_MASK.store(0, Ordering::Release);
            return Ok(());
        }
        let highest = 63 - wanted.leading_zeros();
        let mut allocated = 0u64;
        loop {
            // SAFETY: TlsAlloc has no preconditions and this runs before any
            // provider DLL from the parent is loaded into the child.
            let slot = unsafe { TlsAlloc() };
            if slot == TLS_OUT_OF_INDEXES || slot >= 64 || slot > highest {
                return Err(());
            }
            allocated |= 1u64 << slot;
            if slot == highest {
                break;
            }
        }
        if allocated & wanted != wanted {
            return Err(());
        }
        super::FORK_TLS_SLOT_MASK.store(wanted, Ordering::Release);
        Ok(())
    }

    unsafe fn parse_loader_handles() -> Option<(HANDLE, HANDLE)> {
        let command = unsafe { GetCommandLineW() };
        if command.is_null() {
            return None;
        }
        let mut len = 0usize;
        while len < 32768 && unsafe { *command.add(len) } != 0 {
            len += 1;
        }
        if len == 32768 || len < MARKER.len() {
            return None;
        }
        for start in 0..=len - MARKER.len() {
            let mut matches = true;
            for (offset, expected) in MARKER.iter().enumerate() {
                if unsafe { *command.add(start + offset) } != *expected {
                    matches = false;
                    break;
                }
            }
            if !matches {
                continue;
            }
            let mut cursor = start + MARKER.len();
            let ready = unsafe { parse_decimal(command, len, &mut cursor) }?;
            if cursor >= len || unsafe { *command.add(cursor) } != b':' as u16 {
                return None;
            }
            cursor += 1;
            let bootstrap = unsafe { parse_decimal(command, len, &mut cursor) }?;
            return Some((ready as HANDLE, bootstrap as HANDLE));
        }
        None
    }

    unsafe fn parse_decimal(command: *const u16, len: usize, cursor: &mut usize) -> Option<usize> {
        let mut value = 0usize;
        let start = *cursor;
        while *cursor < len {
            let unit = unsafe { *command.add(*cursor) };
            if !(b'0' as u16..=b'9' as u16).contains(&unit) {
                break;
            }
            value = value
                .checked_mul(10)?
                .checked_add((unit - b'0' as u16) as usize)?;
            *cursor += 1;
        }
        (*cursor != start).then_some(value)
    }

    #[cfg(test)]
    mod wait_tests {
        use super::*;

        #[test]
        fn an_exit_report_is_not_observable_before_the_process_object_is_signalled() {
            let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            assert!(!event.is_null());
            let candidate = WaitCandidate {
                entry: super::super::job::Entry::default(),
                handle: Some(event as usize),
            };

            assert_eq!(child_process_has_exited(candidate), Ok(false));
            assert_ne!(unsafe { SetEvent(event) }, 0);
            assert_eq!(child_process_has_exited(candidate), Ok(true));

            unsafe { CloseHandle(event) };
            let handleless = WaitCandidate {
                entry: super::super::job::Entry::default(),
                handle: None,
            };
            assert_eq!(child_process_has_exited(handleless), Ok(true));
        }

        #[test]
        fn registered_child_completion_signals_the_exact_activity_event() {
            let process = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            let activity = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
            assert!(!process.is_null());
            assert!(!activity.is_null());
            let wait =
                super::super::register_completion_notification(process as usize, activity as usize)
                    .expect("registered completion wait");

            assert_eq!(unsafe { WaitForSingleObject(activity, 0) }, WAIT_TIMEOUT);
            assert_ne!(unsafe { SetEvent(process) }, 0);
            assert_eq!(
                unsafe { WaitForSingleObject(activity, 1_000) },
                WAIT_OBJECT_0
            );

            super::super::unregister_child_completion(wait);
            unsafe {
                CloseHandle(activity);
                CloseHandle(process);
            }
        }
    }

    #[cfg(test)]
    mod fork_mapping_tests {
        use super::*;

        #[test]
        fn retained_cow_protection_never_becomes_a_shared_writer() {
            use windows_sys::Win32::System::Memory::{
                PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_WRITECOPY,
            };
            let storage = ForkMappingStorage::CopyOnWriteSection;
            assert_eq!(
                copied_mapping_protection(storage, PAGE_READWRITE),
                PAGE_WRITECOPY
            );
            assert_eq!(
                copied_mapping_protection(storage, PAGE_EXECUTE_READWRITE | PAGE_GUARD),
                PAGE_EXECUTE_WRITECOPY | PAGE_GUARD
            );
            for protection in [
                PAGE_NOACCESS,
                PAGE_READONLY,
                PAGE_WRITECOPY,
                PAGE_EXECUTE_WRITECOPY,
            ] {
                assert_eq!(copied_mapping_protection(storage, protection), protection);
            }
        }

        #[test]
        fn ordinary_copies_translate_section_cow_protection_without_losing_modifiers() {
            use windows_sys::Win32::System::Memory::{
                PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, PAGE_READONLY, PAGE_WRITECOPY,
            };
            assert_eq!(
                copied_mapping_protection(ForkMappingStorage::Ordinary, PAGE_WRITECOPY),
                PAGE_READWRITE
            );
            assert_eq!(
                copied_mapping_protection(
                    ForkMappingStorage::Ordinary,
                    PAGE_EXECUTE_WRITECOPY | PAGE_GUARD
                ),
                PAGE_EXECUTE_READWRITE | PAGE_GUARD
            );
            for protection in [
                PAGE_NOACCESS,
                PAGE_READONLY,
                PAGE_READWRITE,
                PAGE_EXECUTE_READWRITE,
            ] {
                assert_eq!(
                    copied_mapping_protection(ForkMappingStorage::Ordinary, protection),
                    protection
                );
            }
            assert_eq!(
                copied_mapping_protection(ForkMappingStorage::Section, PAGE_WRITECOPY),
                PAGE_READWRITE
            );
            let memory = unsafe {
                VirtualAlloc(ptr::null(), 4096, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE)
            };
            assert!(!memory.is_null());
            let mut old = 0;
            let translated =
                copied_mapping_protection(ForkMappingStorage::Ordinary, PAGE_WRITECOPY);
            assert_ne!(
                unsafe {
                    VirtualProtectEx(GetCurrentProcess(), memory, 4096, translated, &mut old)
                },
                0
            );
            unsafe { memory.cast::<u8>().write(0x5a) };
            assert_eq!(unsafe { memory.cast::<u8>().read() }, 0x5a);
            assert_ne!(unsafe { VirtualFree(memory, 0, MEM_RELEASE) }, 0);
        }

        fn mapping(base: usize, storage: ForkMappingStorage) -> ForkMapping {
            ForkMapping {
                base,
                len: 0x1000,
                behavior: ForkMappingBehavior::Copy,
                storage,
                backing_slot: 0,
                backing_offset: 0,
                view_protection: 0,
                domain: super::super::ForkMappingDomain::GuestMm,
            }
        }

        #[test]
        fn copied_stack_can_later_be_frozen_without_losing_new_writes() {
            use windows_sys::Win32::System::Memory::{
                FILE_MAP_COPY, PAGE_EXECUTE_WRITECOPY, PAGE_WRITECOPY, VirtualAlloc2,
            };
            unsafe {
                let len = 65536;
                let base = VirtualAlloc2(
                    GetCurrentProcess(),
                    ptr::null(),
                    len * 2,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                );
                assert!(!base.is_null());
                assert_ne!(
                    VirtualFree(base, len, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER),
                    0
                );
                let section = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE,
                    0,
                    len as u32,
                    ptr::null(),
                );
                assert!(!section.is_null());
                let view = MapViewOfFile3(
                    section,
                    GetCurrentProcess(),
                    base,
                    0,
                    len,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_EXECUTE_READWRITE,
                    ptr::null_mut(),
                    0,
                );
                assert_eq!(view.Value, base);
                let mut previous = 0;
                assert_ne!(
                    VirtualProtectEx(
                        GetCurrentProcess(),
                        base,
                        len,
                        copied_mapping_protection(ForkMappingStorage::Section, PAGE_WRITECOPY),
                        &mut previous
                    ),
                    0
                );
                base.cast::<u8>().write_volatile(97);
                let mut entry = mapping(base as usize, ForkMappingStorage::AnonymousSnapshot);
                entry.len = len;
                entry.view_protection = PAGE_EXECUTE_WRITECOPY;
                assert!(crate::anonymous_cow::snapshot(&entry, section).unwrap());
                let child = MapViewOfFile(section, FILE_MAP_COPY, 0, 0, len);
                assert!(!child.Value.is_null());
                assert_eq!(base.cast::<u8>().read_volatile(), 97);
                assert_eq!(child.Value.cast::<u8>().read_volatile(), 97);
                base.cast::<u8>().write_volatile(101);
                assert_eq!(child.Value.cast::<u8>().read_volatile(), 97);
                UnmapViewOfFile(child);
                UnmapViewOfFile(view);
                assert_ne!(VirtualFree(base.byte_add(len), 0, MEM_RELEASE), 0);
                CloseHandle(section);
            }
        }

        #[test]
        fn active_snapshot_stacks_receive_private_copied_backings() {
            let mut entries = [
                mapping(0x10000, ForkMappingStorage::AnonymousSnapshot),
                mapping(0x20000, ForkMappingStorage::AnonymousSnapshot),
                mapping(0x30000, ForkMappingStorage::AnonymousSnapshot),
            ];
            for (index, entry) in entries.iter_mut().enumerate() {
                entry.backing_slot = 0x40000 + index * 8;
                entry.view_protection = 0x80;
            }
            let excluded = snapshot_stack_fallback(&mut entries, 0x10080, 0x20080);
            assert_eq!(excluded, [0x40000, 0x40008]);
            assert_eq!(entries[0].storage, ForkMappingStorage::Section);
            assert_eq!(entries[1].storage, ForkMappingStorage::Section);
            assert_eq!(entries[2].storage, ForkMappingStorage::AnonymousSnapshot);
            assert_eq!(plan_fork_section_backings(&entries).unwrap().len(), 2);
            assert_eq!(entries[0].view_protection, 0);
        }

        #[test]
        fn section_plan_scales_with_the_registered_address_space() {
            let mappings = (0..128usize)
                .map(|index| {
                    let mut mapping =
                        mapping(0x10_0000 + index * 0x2_0000, ForkMappingStorage::Section);
                    mapping.backing_slot = 0x1000 + index * size_of::<HANDLE>();
                    mapping
                })
                .collect::<Vec<_>>();

            let sections = plan_fork_section_backings(&mappings)
                .expect("every distinct section must be represented without a fixed cap");
            assert_eq!(sections.len(), mappings.len());
            assert!(sections.iter().all(|section| section.size == 0x1000));
        }

        #[test]
        fn retained_section_does_not_allocate_a_private_backing() {
            let mut retained = mapping(0x1_f000, ForkMappingStorage::RetainedSection);
            retained.backing_slot = kinakaze_alloc::ARENA_BASE + kinakaze_alloc::HEADER_PAGE_SIZE;
            retained.view_protection = PAGE_READWRITE;
            assert!(retained.valid_storage_contract());
            assert!(plan_fork_section_backings(&[retained]).unwrap().is_empty());
            let groups = plan_remote_reservation_groups(&[retained], 0x10000).unwrap();
            assert_eq!(groups.len(), 1);
            assert!(!groups[0].ordinary_only);
            assert_eq!(groups[0].temporary_tail, 0x10000);
            retained.behavior = ForkMappingBehavior::Zero;
            assert!(!retained.valid_storage_contract());
            retained.behavior = ForkMappingBehavior::Copy;
            retained.view_protection = PAGE_NOACCESS;
            assert!(!retained.valid_storage_contract());
        }

        #[test]
        fn retained_file_final_partial_page_maps_without_expanding_read_only_section() {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Memory::PAGE_READONLY;
            let path = std::env::temp_dir().join(format!(
                "kinakaze-section-tail-{}-{}.bin",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            let mut system: SYSTEM_INFO = unsafe { zeroed() };
            unsafe { GetSystemInfo(&mut system) };
            let page = system.dwPageSize as usize;
            let exact = page * 2 + 1;
            let rounded = page * 3;
            std::fs::write(&path, vec![0x6cu8; exact]).unwrap();
            let file = std::fs::File::open(&path).unwrap();
            let section = unsafe {
                CreateFileMappingW(
                    file.as_raw_handle(),
                    ptr::null(),
                    PAGE_READONLY,
                    0,
                    0,
                    ptr::null(),
                )
            };
            assert!(!section.is_null());
            let length = unsafe { retained_section_view_length(section, 0, rounded) }.unwrap();
            assert_eq!(length, exact);
            assert_eq!(
                unsafe { retained_section_view_length(section, page as u64, page * 2) }.unwrap(),
                page + 1
            );
            assert!(unsafe { retained_section_view_length(section, 0, page * 4) }.is_err());
            assert!(
                unsafe { retained_section_view_length(section, rounded as u64, page) }.is_err()
            );
            let base = unsafe {
                VirtualAlloc2(
                    GetCurrentProcess(),
                    ptr::null(),
                    rounded,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null());
            let view = unsafe {
                MapViewOfFile3(
                    section,
                    GetCurrentProcess(),
                    base,
                    0,
                    length,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READONLY,
                    ptr::null_mut(),
                    0,
                )
            };
            let error = unsafe { GetLastError() };
            let mut observed: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
            let mut last_byte = 0;
            let mut padding = 1;
            if !view.Value.is_null() {
                unsafe {
                    VirtualQuery(
                        view.Value,
                        &mut observed,
                        size_of::<MEMORY_BASIC_INFORMATION>(),
                    );
                    last_byte = view.Value.cast::<u8>().add(exact - 1).read();
                    padding = view.Value.cast::<u8>().add(rounded - 1).read();
                    UnmapViewOfFile(view);
                }
            } else {
                unsafe { VirtualFree(base, 0, MEM_RELEASE) };
            }
            unsafe { CloseHandle(section) };
            drop(file);
            std::fs::remove_file(&path).unwrap();
            assert_eq!(
                view.Value, base,
                "exact file tail mapping failed with Windows error {error}"
            );
            assert_eq!(observed.RegionSize, rounded);
            assert_eq!(observed.Protect, PAGE_READONLY);
            assert_eq!(last_byte, 0x6c);
            assert_eq!(padding, 0);
        }

        #[test]
        fn placeholder_plan_absorbs_ordinary_pages_in_the_same_host_envelope() {
            let mappings = [
                mapping(0x1_1000, ForkMappingStorage::Ordinary),
                mapping(0x1_2000, ForkMappingStorage::Placeholder),
                mapping(0x3_0000, ForkMappingStorage::Ordinary),
            ];
            let groups = plan_remote_reservation_groups(&mappings, 0x1_0000)
                .expect("valid page mappings must produce a reservation plan");

            assert!(groups.iter().any(|group| {
                group.base == 0x1_0000 && group.len == 0x1_0000 && !group.ordinary_only
            }));
            assert!(groups.iter().any(|group| group.base == 0x3_0000
                && group.len == 0x1_0000
                && group.ordinary_only));
            assert_eq!(groups.len(), 2);
        }

        #[test]
        fn omitted_mappings_do_not_expand_a_placeholder_component() {
            let mut omitted = mapping(0x2_0000, ForkMappingStorage::Ordinary);
            omitted.behavior = ForkMappingBehavior::Omit;
            let mappings = [mapping(0x1_f000, ForkMappingStorage::Placeholder), omitted];
            let groups = plan_remote_reservation_groups(&mappings, 0x1_0000)
                .expect("valid page mappings must produce a reservation plan");

            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].base, 0x1_0000);
            assert_eq!(groups[0].len, 0x1_0000);
            assert!(!groups[0].ordinary_only);
        }

        #[test]
        fn interior_ordinary_pages_share_one_private_host_reservation() {
            let mappings = [
                mapping(0x4_1000, ForkMappingStorage::Ordinary),
                mapping(0x4_f000, ForkMappingStorage::Ordinary),
            ];
            let groups = plan_remote_reservation_groups(&mappings, 0x1_0000)
                .expect("valid ordinary pages must produce a reservation plan");

            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].base, 0x4_0000);
            assert_eq!(groups[0].len, 0x1_0000);
            assert!(groups[0].ordinary_only);
        }

        #[test]
        fn terminal_ordinary_page_in_a_mixed_group_keeps_a_view_suffix() {
            let mappings = [
                mapping(0x1_8000, ForkMappingStorage::Section),
                mapping(0x1_f000, ForkMappingStorage::Ordinary),
            ];
            let groups = plan_remote_reservation_groups(&mappings, 0x1_0000)
                .expect("valid mixed mappings must produce a reservation plan");

            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].base, 0x1_0000);
            assert_eq!(groups[0].len, 0x2_0000);
            assert_eq!(groups[0].temporary_tail, 0x1_0000);
            assert!(!groups[0].ordinary_only);
        }

        #[test]
        fn installation_order_is_top_down_between_gaps_and_bottom_up_when_touching() {
            let mut low = mapping(0x1_0000, ForkMappingStorage::Ordinary);
            low.len = 0x2000;
            let mut high_private = mapping(0x2_0000, ForkMappingStorage::Ordinary);
            high_private.len = 0x1_0000;
            let high_section = mapping(0x3_0000, ForkMappingStorage::Section);
            let mappings = [low, high_private, high_section];
            let order = MappingInstallationOrder::new(&mappings)
                .expect("sorted non-overlapping mappings must have an order")
                .collect::<Vec<_>>();

            assert_eq!(order, [1, 2, 0]);
        }

        #[test]
        fn final_section_page_joins_the_following_host_envelope() {
            let mut following = mapping(0x3f_10000, ForkMappingStorage::Ordinary);
            following.len = 0x37_0000;
            let mappings = [mapping(0x3f_0f000, ForkMappingStorage::Section), following];
            let groups = plan_remote_reservation_groups(&mappings, 0x1_0000)
                .expect("page mappings at a host-envelope boundary must remain exact");

            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].base, 0x3f_00000);
            assert_eq!(groups[0].len, 0x39_0000);
            assert_eq!(groups[0].temporary_tail, 0x1_0000);
            assert!(!groups[0].ordinary_only);
        }

        #[test]
        fn section_view_at_a_host_envelope_end_uses_the_reserved_suffix() {
            let process = unsafe { GetCurrentProcess() };
            let base = unsafe {
                VirtualAlloc2(
                    process,
                    ptr::null(),
                    0x2_1000,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null(), "placeholder reservation failed");
            let target = base as usize + 0x1_f000;
            unsafe {
                carve_remote_placeholder(process, target, 0x1000)
                    .expect("the final page must become one exact placeholder");
            }
            let section = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE,
                    0,
                    0x1000,
                    ptr::null(),
                )
            };
            assert!(!section.is_null(), "pagefile section creation failed");
            let view = unsafe {
                MapViewOfFile3(
                    section,
                    process,
                    target as *const c_void,
                    0,
                    0x1000,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            let error = unsafe { GetLastError() };
            unsafe {
                if !view.Value.is_null() {
                    UnmapViewOfFile(view);
                }
                CloseHandle(section);
                VirtualFree(base, 0, MEM_RELEASE);
            }
            assert_eq!(
                view.Value as usize, target,
                "final-page replacement failed with Windows error {error}"
            );
        }

        #[test]
        fn adjacent_section_views_install_low_to_high_with_one_terminal_suffix() {
            let process = unsafe { GetCurrentProcess() };
            let base = unsafe {
                VirtualAlloc2(
                    process,
                    ptr::null(),
                    0x4_0000,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null(), "placeholder reservation failed");
            let low = base as usize + 0x1_0000;
            let high = base as usize + 0x2_0000;
            let tail = base as usize + 0x3_0000;
            let low_section = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE,
                    0,
                    0x1_0000,
                    ptr::null(),
                )
            };
            let high_section = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE,
                    0,
                    0x1_0000,
                    ptr::null(),
                )
            };
            assert!(!low_section.is_null() && !high_section.is_null());

            unsafe {
                carve_remote_placeholder(process, low, 0x1_0000)
                    .expect("low section must become one exact placeholder");
            }
            let low_view = unsafe {
                MapViewOfFile3(
                    low_section,
                    process,
                    low as *const c_void,
                    0,
                    0x1_0000,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                low_view.Value as usize,
                low,
                "low section failed with Windows error {}",
                unsafe { GetLastError() }
            );

            unsafe {
                carve_remote_placeholder(process, high, 0x1_0000)
                    .expect("high section must become one exact placeholder");
            }
            let high_view = unsafe {
                MapViewOfFile3(
                    high_section,
                    process,
                    high as *const c_void,
                    0,
                    0x1_0000,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            let high_error = unsafe { GetLastError() };
            unsafe {
                carve_remote_placeholder(process, tail, 0x1_0000)
                    .expect("terminal host tail must remain an exact placeholder");
                assert_ne!(VirtualFree(tail as *mut c_void, 0, MEM_RELEASE), 0);
                if !high_view.Value.is_null() {
                    UnmapViewOfFile(high_view);
                }
                if !low_view.Value.is_null() {
                    UnmapViewOfFile(low_view);
                }
                CloseHandle(high_section);
                CloseHandle(low_section);
                VirtualFree(base, 0, MEM_RELEASE);
            }
            assert_eq!(
                high_view.Value as usize, high,
                "high section failed with Windows error {high_error}"
            );
        }

        #[test]
        fn page_granular_private_reserve_view_commits_at_its_exact_address() {
            let process = unsafe { GetCurrentProcess() };
            let base = unsafe {
                VirtualAlloc2(
                    process,
                    ptr::null(),
                    0x3_0000,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null(), "placeholder reservation failed");
            let target = base as usize + 0x1_1000;
            unsafe {
                carve_remote_placeholder(process, target, 0x1000)
                    .expect("page-granular target must become one exact placeholder");
                map_remote_private_reserve_view(process, target, 0x1000)
                    .expect("private reserve view must replace a 4 KiB target");
            }
            let committed = unsafe {
                VirtualAllocEx(
                    process,
                    target as *const c_void,
                    0x1000,
                    MEM_COMMIT,
                    PAGE_READWRITE,
                )
            };
            let error = unsafe { GetLastError() };
            if committed as usize == target {
                unsafe { (target as *mut u64).write_volatile(0xfeed_face_cafe_beef) };
                assert_eq!(
                    unsafe { (target as *const u64).read_volatile() },
                    0xfeed_face_cafe_beef
                );
            }
            unsafe {
                UnmapViewOfFile(
                    windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: target as *mut c_void,
                    },
                );
                VirtualFree(base, 0, MEM_RELEASE);
                VirtualFree((target + 0x1000) as *mut c_void, 0, MEM_RELEASE);
            }
            assert_eq!(
                committed as usize, target,
                "SEC_RESERVE page commit failed with Windows error {error}"
            );
        }

        #[test]
        fn private_prefix_installs_before_a_page_granular_section_with_a_suffix() {
            let process = unsafe { GetCurrentProcess() };
            let base = unsafe {
                VirtualAlloc2(
                    process,
                    ptr::null(),
                    0x7_0000,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null(), "placeholder reservation failed");
            let section_base = base as usize + 0x4_0000;

            unsafe {
                carve_remote_placeholder(process, base as usize, 0x4_0000)
                    .expect("private prefix must become one exact placeholder");
            }
            let private = unsafe {
                VirtualAlloc2(
                    process,
                    base,
                    0x4_0000,
                    MEM_COMMIT | MEM_RESERVE | MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                private,
                base,
                "private prefix failed with Windows error {}",
                unsafe { GetLastError() }
            );

            let section = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE,
                    0,
                    0x2_1000,
                    ptr::null(),
                )
            };
            assert!(!section.is_null(), "pagefile section creation failed");
            unsafe {
                carve_remote_placeholder(process, section_base, 0x2_1000)
                    .expect("section must become one exact placeholder");
            }
            let view = unsafe {
                MapViewOfFile3(
                    section,
                    process,
                    section_base as *const c_void,
                    0,
                    0x2_1000,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            let view_error = unsafe { GetLastError() };
            assert_eq!(
                view.Value as usize, section_base,
                "page-granular section failed with Windows error {view_error}"
            );
            unsafe {
                let alignment_padding = base as usize + 0x6_1000;
                assert_ne!(
                    VirtualFree(alignment_padding as *mut c_void, 0, MEM_RELEASE),
                    0,
                    "alignment padding release failed with Windows error {}",
                    GetLastError()
                );
                if !view.Value.is_null() {
                    UnmapViewOfFile(view);
                }
                CloseHandle(section);
                if !private.is_null() {
                    VirtualFree(private, 0, MEM_RELEASE);
                }
            }
        }

        #[test]
        fn private_allocation_replaces_one_split_placeholder() {
            let process = unsafe { GetCurrentProcess() };
            let base = unsafe {
                VirtualAlloc2(
                    process,
                    ptr::null(),
                    0x3_0000,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null(), "placeholder reservation failed");
            let target = base as usize + 0x1_0000;
            unsafe {
                carve_remote_placeholder(process, target, 0x1_0000)
                    .expect("the middle envelope must become one exact placeholder");
            }
            let replaced = unsafe {
                VirtualAlloc2(
                    process,
                    target as *const c_void,
                    0x1_0000,
                    MEM_COMMIT | MEM_RESERVE | MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            let error = unsafe { GetLastError() };
            unsafe {
                if !replaced.is_null() {
                    VirtualFree(replaced, 0, MEM_RELEASE);
                }
                VirtualFree(base, 0, MEM_RELEASE);
                VirtualFree((base as usize + 0x2_0000) as *mut c_void, 0, MEM_RELEASE);
            }
            assert_eq!(
                replaced as usize, target,
                "private placeholder replacement failed with Windows error {error}"
            );
        }

        #[test]
        fn mapped_suffix_is_installed_before_a_private_prefix() {
            let process = unsafe { GetCurrentProcess() };
            let base = unsafe {
                VirtualAlloc2(
                    process,
                    ptr::null(),
                    0x4_0000,
                    MEM_RESERVE | MEM_RESERVE_PLACEHOLDER,
                    PAGE_NOACCESS,
                    ptr::null_mut(),
                    0,
                )
            };
            assert!(!base.is_null(), "placeholder reservation failed");
            let view_base = base as usize + 0x1_0000;
            unsafe {
                carve_remote_placeholder(process, view_base, 0x2_0000)
                    .expect("mapped suffix must become one exact placeholder");
            }
            let section = unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    ptr::null(),
                    PAGE_EXECUTE_READWRITE,
                    0,
                    0x2_0000,
                    ptr::null(),
                )
            };
            assert!(!section.is_null(), "pagefile section creation failed");
            let view = unsafe {
                MapViewOfFile3(
                    section,
                    process,
                    view_base as *const c_void,
                    0,
                    0x2_0000,
                    MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                view.Value as usize,
                view_base,
                "mapped suffix failed with Windows error {}",
                unsafe { GetLastError() }
            );

            // The host-only tail exists solely to make the terminal section
            // replacement legal.  Remove it before materializing lower Linux
            // mappings so it never becomes guest-visible address-space state.
            let temporary_tail = base as usize + 0x3_0000;
            unsafe {
                carve_remote_placeholder(process, temporary_tail, 0x1_0000)
                    .expect("temporary host tail must remain an exact placeholder");
            }
            assert_ne!(
                unsafe { VirtualFree(temporary_tail as *mut c_void, 0, MEM_RELEASE) },
                0,
                "temporary host tail release failed with Windows error {}",
                unsafe { GetLastError() }
            );

            unsafe {
                carve_remote_placeholder(process, base as usize, 0x1000)
                    .expect("private prefix must become one exact placeholder");
            }
            let private = unsafe {
                VirtualAlloc2(
                    process,
                    base,
                    0x1000,
                    MEM_COMMIT | MEM_RESERVE | MEM_REPLACE_PLACEHOLDER,
                    PAGE_READWRITE,
                    ptr::null_mut(),
                    0,
                )
            };
            let error = unsafe { GetLastError() };
            unsafe {
                if !private.is_null() {
                    VirtualFree(private, 0, MEM_RELEASE);
                }
                VirtualFree((base as usize + 0x1000) as *mut c_void, 0, MEM_RELEASE);
                if !view.Value.is_null() {
                    UnmapViewOfFile(view);
                }
                CloseHandle(section);
            }
            assert_eq!(
                private, base,
                "private prefix failed with Windows error {error}"
            );
        }
    }
}
