//! Host thread-local state and ELF general-dynamic TLS support.

pub mod thread_pointer;

use std::alloc::Layout;
use std::cell::{Cell, RefCell, UnsafeCell};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{OnceLock, RwLock};

pub const MAX_TLS_MODULES: usize = 256;
pub const MAX_PTHREAD_KEYS: usize = 1024;
pub const EAGAIN: i32 = 11;
pub const EINVAL: i32 = 22;

#[cfg(target_arch = "x86_64")]
pub type KeyDestructor = unsafe extern "sysv64" fn(*mut c_void);

#[cfg(not(target_arch = "x86_64"))]
pub type KeyDestructor = unsafe extern "C" fn(*mut c_void);

#[derive(Clone, Debug)]
struct ModuleTemplate {
    image: Vec<u8>,
    memory_size: usize,
    align: usize,
}

struct TlsBlock {
    pointer: *mut u8,
    layout: Layout,
}

impl TlsBlock {
    fn new(template: &ModuleTemplate) -> Option<Self> {
        let block = Self::zeroed(template.memory_size, template.align)?;
        // SAFETY: registration guarantees the initialized image fits in memsz.
        unsafe {
            ptr::copy_nonoverlapping(template.image.as_ptr(), block.pointer, template.image.len())
        };
        Some(block)
    }

    fn zeroed(size: usize, alignment: usize) -> Option<Self> {
        let layout = Layout::from_size_align(size.max(1), alignment).ok()?;
        // ELF TLS addresses are observable guest pointers and can legally stay
        // live across fork.  Allocate them in the fixed arena so the child can
        // bind the normally reloaded DLL state back to the exact same address.
        // SAFETY: null requests a fresh allocation with this layout.
        let pointer =
            unsafe { kinakaze_alloc::reallocate(ptr::null_mut(), layout.align(), layout.size()) };
        if pointer.is_null() {
            return None;
        }
        // SAFETY: the allocation contains `layout.size()` writable bytes.
        unsafe { ptr::write_bytes(pointer, 0, layout.size()) };
        Some(Self { pointer, layout })
    }

    fn as_ptr(&self) -> *const u8 {
        self.pointer
    }

    fn len(&self) -> usize {
        self.layout.size()
    }
}

impl Drop for TlsBlock {
    fn drop(&mut self) {
        // SAFETY: this block owns the fixed-arena allocation.
        unsafe { kinakaze_alloc::free(self.pointer) };
    }
}

thread_local! {
    // POSIX locale_t is a borrowed guest handle. pthread inherits it; libc owns
    // its data and restores the surviving thread's selection after fork.
    static LOCALE: Cell<usize> = const { Cell::new(usize::MAX) };
    static ERRNO_POINTER: Cell<*mut i32> = const { Cell::new(ptr::null_mut()) };
    static ERRNO_FALLBACK: UnsafeCell<i32> = const { UnsafeCell::new(0) };
    static ELF_TLS: RefCell<Vec<Option<TlsBlock>>> = const { RefCell::new(Vec::new()) };
    /// Variant-II static TLS used by `R_X86_64_TPOFF*` and fixed `%fs:` ABI
    /// accesses. Keeping ownership in host TLS gives every guest pthread its own
    /// block and makes VEH lookup independent of any process-global pointer.
    static STATIC_ELF_TLS: RefCell<Option<thread_pointer::ThreadBlock>> = const { RefCell::new(None) };
    static PTHREAD_VALUES: RefCell<KeyValues> = const { RefCell::new(KeyValues(Vec::new())) };
    /// The block `KeyValues::drop` is currently running destructors for, or
    /// null. A raw pointer has no drop glue, so this key never registers a
    /// destructor of its own and stays readable after every other key on the
    /// thread has been torn down -- which is precisely when it is needed.
    static DESTROYING_VALUES: Cell<*mut KeyValues> = const { Cell::new(ptr::null_mut()) };
    static CXX_DESTRUCTORS: RefCell<ThreadDestructors> = const { RefCell::new(ThreadDestructors(Vec::new())) };
    static DESTROYING_CXX: Cell<*mut ThreadDestructors> = const { Cell::new(ptr::null_mut()) };
}

pub fn locale() -> usize {
    LOCALE.get()
}
pub fn set_locale(handle: usize) {
    LOCALE.set(handle);
}

#[derive(Clone, Copy)]
struct ThreadDestructor {
    callback: KeyDestructor,
    argument: usize,
    // Guest modules remain mapped for the process lifetime. Preserve the DSO
    // association for fork; implementing dlclose requires reference pinning too.
    dso: usize,
}

struct ThreadDestructors(Vec<ThreadDestructor>);

impl Drop for ThreadDestructors {
    fn drop(&mut self) {
        let pointer = &raw mut *self;
        if DESTROYING_CXX.try_with(|slot| slot.set(pointer)).is_err() {
            return;
        }
        // Pop before calling guest code: a callback can register another
        // destructor, which must run before earlier registrations.
        while let Some(record) = unsafe { (*pointer).0.pop() } {
            unsafe { (record.callback)(record.argument as *mut c_void) };
        }
        let _ = DESTROYING_CXX.try_with(|slot| slot.set(ptr::null_mut()));
    }
}

/// Register a C++ thread_local destructor. The owning guest DSO must remain
/// mapped until the callback runs, as enforced by the current guest loader.
pub fn cxa_thread_atexit(
    callback: Option<KeyDestructor>,
    argument: *mut c_void,
    dso: *mut c_void,
) -> Result<(), i32> {
    let callback = callback.ok_or(EINVAL)?;
    let record = ThreadDestructor {
        callback,
        argument: argument as usize,
        dso: dso as usize,
    };
    let append = |records: &mut ThreadDestructors| {
        records.0.try_reserve(1).map_err(|_| 12)?;
        records.0.push(record);
        Ok(())
    };
    let active = DESTROYING_CXX
        .try_with(Cell::get)
        .unwrap_or(ptr::null_mut());
    if !active.is_null() {
        // SAFETY: published only by this thread's destructor runner, which
        // retains no borrow across a callback.
        return append(unsafe { &mut *active });
    }
    CXX_DESTRUCTORS
        .try_with(|records| append(&mut records.borrow_mut()))
        .unwrap_or(Err(EINVAL))
}

/// Drain only the calling thread's C++ thread_local destructors. Process exit
/// calls this before process atexit handlers, without running pthread-key cleanup.
pub fn run_cxx_thread_destructors() {
    // Take the records out before callbacks so reentrant registration can reach
    // the published runner instead of borrowing a RefCell recursively.
    let records = CXX_DESTRUCTORS.try_with(|records| {
        std::mem::replace(&mut *records.borrow_mut(), ThreadDestructors(Vec::new()))
    });
    if let Ok(records) = records {
        drop(records);
    }
}

/// Run guest TLS cleanup while the calling thread's guest TLS is still active.
/// C++ destructors precede pthread key destructors, including explicit exit and
/// deferred cancellation. Empty host-TLS owners make later native teardown inert.
pub fn run_thread_destructors() {
    run_cxx_thread_destructors();
    let values = PTHREAD_VALUES
        .try_with(|values| std::mem::replace(&mut *values.borrow_mut(), KeyValues(Vec::new())));
    if let Ok(values) = values {
        drop(values);
    }
}

static MODULES: OnceLock<RwLock<Vec<ModuleTemplate>>> = OnceLock::new();
static PTHREAD_KEYS: OnceLock<RwLock<Vec<KeyEntry>>> = OnceLock::new();
static STATIC_TLS_TEB_SLOT: AtomicU32 = AtomicU32::new(u32::MAX);
static HOST_TRANSITION_TEB_SLOT: AtomicU32 = AtomicU32::new(u32::MAX);
static CANARY_TEB_SLOT: AtomicU32 = AtomicU32::new(u32::MAX);
static CANARY_VALUE: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy, Default)]
struct KeyEntry {
    active: bool,
    // Zero is never issued. Inactive entries retain the generation so a reused
    // numeric pthread_key_t cannot recover another thread's previous value.
    generation: u64,
    destructor: Option<KeyDestructor>,
}

#[derive(Clone, Copy, Default, Debug, Eq, PartialEq)]
struct KeyValue {
    generation: u64,
    value: usize,
}

struct KeyValues(Vec<KeyValue>);

/// Clears the in-flight publication when `KeyValues::drop` leaves by any path.
///
/// Without this, an access after the block's storage is gone would follow a
/// dangling pointer instead of reporting "no value set".
struct Unpublish;

impl Drop for Unpublish {
    fn drop(&mut self) {
        let _ = DESTROYING_VALUES.try_with(|slot| slot.set(ptr::null_mut()));
    }
}

impl Drop for KeyValues {
    fn drop(&mut self) {
        run_cxx_thread_destructors();
        // Destructors are guest code, and POSIX lets them call back into
        // `pthread_{get,set}specific`. By now `PTHREAD_VALUES` is mid-drop, so
        // `LocalKey::with` on it would raise `AccessError` -- a panic that
        // cannot cross the `extern "sysv64"` ABI boundary and aborts the
        // process. Publish this block for the duration of the callbacks so
        // those calls reach the values they are entitled to see. Every access
        // below goes through `in_flight` so the re-entrant borrows and this
        // one share a single provenance chain.
        let in_flight: *mut Self = &raw mut *self;
        let _unpublish = Unpublish;
        if DESTROYING_VALUES
            .try_with(|slot| slot.set(in_flight))
            .is_err()
        {
            return;
        }

        // POSIX permits multiple destructor passes when a destructor stores a
        // new non-null value. Four is PTHREAD_DESTRUCTOR_ITERATIONS.
        for _ in 0..4 {
            let callbacks = {
                let Ok(keys) = pthread_keys().read() else {
                    return;
                };
                let mut callbacks = Vec::new();
                // SAFETY: `in_flight` is this block, and the borrow ends with
                // this scope -- before any destructor below can re-enter.
                let values = unsafe { &mut (*in_flight).0 };
                for (key, value) in values.iter_mut().enumerate() {
                    if value.value == 0 {
                        continue;
                    }
                    let entry = keys
                        .get(key)
                        .filter(|entry| entry.active && entry.generation == value.generation);
                    if let Some(entry) = entry {
                        if let Some(destructor) = entry.destructor {
                            let argument = value.value as *mut c_void;
                            value.value = 0;
                            callbacks.push((key, entry.generation, destructor, argument));
                        }
                    } else {
                        *value = KeyValue::default();
                    }
                }
                callbacks
            };
            if callbacks.is_empty() {
                break;
            }
            for (key, generation, destructor, argument) in callbacks {
                // An earlier callback can delete/recreate a later key. The
                // captured callback belongs only to its original generation.
                let still_active = pthread_keys().read().ok().is_some_and(|keys| {
                    keys.get(key)
                        .is_some_and(|entry| entry.active && entry.generation == generation)
                });
                if !still_active {
                    continue;
                }
                // SAFETY: the creator supplied this destructor through the
                // pthread ABI and POSIX requires it to accept the stored value.
                unsafe { destructor(argument) };
            }
        }
    }
}

/// Runs `action` against the calling thread's pthread key values.
///
/// Returns `None` only when this thread has no value block to reach: it never
/// created one, or host TLS finished tearing it down. Both mean "no value is
/// set", which is exactly what POSIX says a key reads back as.
fn with_pthread_values<R>(action: impl FnOnce(&mut KeyValues) -> R) -> Option<R> {
    let in_flight = DESTROYING_VALUES
        .try_with(Cell::get)
        .unwrap_or(ptr::null_mut());
    if !in_flight.is_null() {
        // An explicit pthread exit runs cleanup before native TLS teardown, so
        // the published values take precedence even while PTHREAD_VALUES lives.
        return Some(action(unsafe { &mut *in_flight }));
    }
    // `try_with` consumes its closure whether or not it runs, so park the
    // action where the fallback path can still take it back.
    let mut pending = Some(action);
    let mut result = None;
    let reached = PTHREAD_VALUES.try_with(|values| {
        if let Some(action) = pending.take() {
            result = Some(action(&mut values.borrow_mut()));
        }
    });
    if reached.is_ok() {
        return result;
    }

    // `PTHREAD_VALUES` is being destroyed, so this call came from a key
    // destructor that `KeyValues::drop` is running. Reach that block directly.
    let in_flight = DESTROYING_VALUES
        .try_with(Cell::get)
        .unwrap_or(ptr::null_mut());
    if in_flight.is_null() {
        return None;
    }
    let action = pending.take()?;
    // SAFETY: `KeyValues::drop` publishes this pointer to its own block on
    // this same thread, holds no borrow into it across a destructor call, and
    // clears it before the storage goes away.
    Some(action(unsafe { &mut *in_flight }))
}

fn modules() -> &'static RwLock<Vec<ModuleTemplate>> {
    MODULES.get_or_init(|| RwLock::new(Vec::new()))
}

fn pthread_keys() -> &'static RwLock<Vec<KeyEntry>> {
    PTHREAD_KEYS.get_or_init(|| RwLock::new(Vec::new()))
}

static FALLBACK_GLOBAL_ERRNO: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Returns a stable pointer to the calling host thread's errno cell.
pub fn errno_location() -> *mut i32 {
    ERRNO_POINTER
        .try_with(|slot| {
            let current = slot.get();
            if !current.is_null() {
                return current;
            }
            // SAFETY: null requests a new four-byte, naturally aligned cell.
            let allocated = unsafe {
                kinakaze_alloc::reallocate(
                    ptr::null_mut(),
                    core::mem::align_of::<i32>(),
                    core::mem::size_of::<i32>(),
                )
            }
            .cast::<i32>();
            if allocated.is_null() {
                return ERRNO_FALLBACK
                    .try_with(UnsafeCell::get)
                    .unwrap_or_else(|_| FALLBACK_GLOBAL_ERRNO.as_ptr() as *mut i32);
            }
            // SAFETY: the cell was just allocated and is writable.
            unsafe { allocated.write(0) };
            slot.set(allocated);
            allocated
        })
        .unwrap_or_else(|_| FALLBACK_GLOBAL_ERRNO.as_ptr() as *mut i32)
}

pub fn errno() -> i32 {
    // SAFETY: the pointer addresses this thread's live TLS cell.
    unsafe { *errno_location() }
}

pub fn set_errno(value: i32) {
    // SAFETY: the pointer addresses this thread's live TLS cell.
    unsafe { *errno_location() = value };
}

/// Registers one ELF `PT_TLS` image and returns its one-based module ID.
pub fn register_elf_module(
    initial_image: &[u8],
    memory_size: usize,
    align: usize,
) -> Result<usize, TlsError> {
    if initial_image.len() > memory_size || !align.is_power_of_two() {
        return Err(TlsError::InvalidTemplate);
    }
    let mut modules = modules().write().map_err(|_| TlsError::Poisoned)?;
    if modules.len() >= MAX_TLS_MODULES {
        return Err(TlsError::TooManyModules);
    }
    modules.push(ModuleTemplate {
        image: initial_image.to_vec(),
        memory_size,
        align: align.max(1),
    });
    Ok(modules.len())
}

/// Publishes a newly linked module's relocated `.tdata`, before its guest
/// resolvers/constructors can use TLS. The file image registered while mapping
/// still contains link-time pointers (or zero RELA slots).
pub fn relocate_elf_module_image(module_id: usize, image: &[u8]) -> Result<(), TlsError> {
    let index = module_id.checked_sub(1).ok_or(TlsError::UnknownModule)?;
    let mut modules = modules().write().map_err(|_| TlsError::Poisoned)?;
    let module = modules.get_mut(index).ok_or(TlsError::UnknownModule)?;
    if image.len() != module.image.len() {
        return Err(TlsError::InvalidTemplate);
    }
    module.image.copy_from_slice(image);
    drop(modules);
    // Bootstrap may have installed this thread's block before linking finished.
    // Only this newly linked module's initialized bytes are replaced; unrelated
    // modules and .tbss retain their state. Later threads use the template above.
    if let Some(block) = current_elf_tls_data(module_id) {
        unsafe {
            ptr::copy_nonoverlapping(image.as_ptr(), block, image.len());
        }
    }
    Ok(())
}

/// Reserves one registered module in the x86_64 variant-II static TLS layout.
///
/// This is intentionally lazy: general-dynamic-only modules continue to work on
/// machines without FSGSBASE, while the initial-exec model fails during linking
/// rather than producing a bogus positive offset and corrupting address zero.
pub fn reserve_static_elf_module(module_id: usize) -> Result<usize, TlsError> {
    if let Some(offset) = thread_pointer::offset_of(module_id) {
        return Ok(offset);
    }
    let module_index = module_id.checked_sub(1).ok_or(TlsError::UnknownModule)?;
    let template = modules()
        .read()
        .map_err(|_| TlsError::Poisoned)?
        .get(module_index)
        .cloned()
        .ok_or(TlsError::UnknownModule)?;
    thread_pointer::reserve(module_id, template.memory_size, template.align)
}

/// Selects the direct TEB TLS slot used by AOT-generated host-call trampolines.
///
/// Only the first 64 `TlsAlloc` indices live in the fixed `gs:[0x1480+N*8]`
/// array. Expansion slots require an extra indirection and are rejected instead
/// of generating a trampoline that would silently address unrelated TEB state.
pub fn configure_static_tls_teb_slot(slot: u32) -> Result<(), TlsError> {
    if slot >= 64 {
        return Err(TlsError::OffsetOutOfRange);
    }
    STATIC_TLS_TEB_SLOT.store(slot, Ordering::Release);
    Ok(())
}

/// Selects the direct TEB slot holding the process-owned host transition block.
///
/// This is intentionally distinct from the Linux thread-pointer slot: a valid
/// `ARCH_SET_FS` changes the latter and must never redirect host-private state.
pub fn configure_host_transition_teb_slot(slot: u32) -> Result<(), TlsError> {
    if slot >= 64 {
        return Err(TlsError::OffsetOutOfRange);
    }
    HOST_TRANSITION_TEB_SLOT.store(slot, Ordering::Release);
    Ok(())
}

pub fn configure_canary_teb_slot(slot: u32, canary: usize) -> Result<(), TlsError> {
    if slot >= 64 {
        return Err(TlsError::OffsetOutOfRange);
    }
    CANARY_TEB_SLOT.store(slot, Ordering::Release);
    CANARY_VALUE.store(canary, Ordering::Release);
    Ok(())
}

pub fn configured_canary() -> usize {
    CANARY_VALUE.load(Ordering::Acquire)
}

pub fn initialize_thread_tls() -> Result<(), TlsError> {
    let _ = install_current_thread_static_tls();
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::TlsSetValue;
        let canary_slot = CANARY_TEB_SLOT.load(Ordering::Acquire);
        if canary_slot != u32::MAX {
            let canary = CANARY_VALUE.load(Ordering::Acquire);
            unsafe { TlsSetValue(canary_slot, canary as *mut c_void) };
        }
    }
    Ok(())
}

#[cfg(windows)]
fn publish_static_thread_pointer(pointer: usize) -> Result<(), TlsError> {
    use windows_sys::Win32::System::Threading::TlsSetValue;
    let slot = STATIC_TLS_TEB_SLOT.load(Ordering::Acquire);
    if slot == u32::MAX {
        return Ok(());
    }
    // SAFETY: configuration accepts only a live slot allocated by the loader.
    if unsafe { TlsSetValue(slot, pointer as *mut c_void) } == 0 {
        return Err(TlsError::ThreadPointerUnsupported);
    }
    Ok(())
}

#[cfg(windows)]
fn publish_host_transition_pointer(pointer: usize) -> Result<(), TlsError> {
    use windows_sys::Win32::System::Threading::TlsSetValue;
    let slot = HOST_TRANSITION_TEB_SLOT.load(Ordering::Acquire);
    if slot == u32::MAX {
        return Ok(());
    }
    // SAFETY: configuration accepts only a live direct slot owned by the loader.
    if unsafe { TlsSetValue(slot, pointer as *mut c_void) } == 0 {
        return Err(TlsError::ThreadPointerUnsupported);
    }
    Ok(())
}

#[cfg(not(windows))]
fn publish_static_thread_pointer(_pointer: usize) -> Result<(), TlsError> {
    Ok(())
}

#[cfg(not(windows))]
fn publish_host_transition_pointer(_pointer: usize) -> Result<(), TlsError> {
    Ok(())
}

/// Returns the calling thread's process-owned host transition block.
///
/// TLS state is linked once into the runtime DLL. An unconfigured slot means
/// there is no host transition yet; forwarding back to the runtime would recurse.
pub fn host_transition_pointer() -> usize {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        use windows_sys::Win32::System::Threading::TlsGetValue;

        let slot = HOST_TRANSITION_TEB_SLOT.load(Ordering::Acquire);
        if slot != u32::MAX {
            // SAFETY: configured slots are direct, live TlsAlloc indices.
            return unsafe { TlsGetValue(slot) as usize };
        }
    }
    0
}

/// Linux GS is independent of the host TEB and the guest's FS thread pointer.
pub fn guest_gs_base() -> usize {
    let transition = host_transition_pointer();
    if transition == 0 {
        return 0;
    }
    unsafe { ((transition + thread_pointer::GUEST_GS_BASE_OFFSET) as *const usize).read() }
}

pub fn set_guest_gs_base(value: usize) -> bool {
    // Linux rejects addresses outside the userspace canonical range.
    if value >= 0x0000_8000_0000_0000 {
        return false;
    }
    let transition = host_transition_pointer();
    if transition == 0 {
        return value == 0;
    }
    unsafe { ((transition + thread_pointer::GUEST_GS_BASE_OFFSET) as *mut usize).write(value) };
    true
}

/// Builds and activates the calling thread's static ELF TLS block.
///
/// Calling this again is idempotent and merely restores a base Windows may have
/// discarded. The layout must already contain every initial-exec module.
pub fn install_current_thread_static_tls() -> Result<usize, TlsError> {
    STATIC_ELF_TLS
        .try_with(|slot| {
            if let Some(block) = slot.borrow().as_ref() {
                // The published guest TP may have been replaced by ARCH_SET_FS;
                // never revert it to the bootstrap TCB merely because host code
                // re-enters the TLS helper.
                let published = get_current_thread_fs_base();
                if published == 0 {
                    block.restore()?;
                    publish_static_thread_pointer(block.thread_pointer())?;
                }
                if host_transition_pointer() == 0 {
                    publish_host_transition_pointer(block.transition_pointer())?;
                }
                return Ok(block.thread_pointer());
            }

            // A fork child has fresh Rust/native TLS, while the process fork
            // participant has already restored the parent-owned guest TP and
            // transition block into its TEB slots. Rebind to those registered
            // values without allocating a child-private block and overwriting
            // the authoritative state. Their backing mappings are kept alive by
            // the fork mapping registry, not by this module-local owner.
            let published = get_current_thread_fs_base();
            let transition = host_transition_pointer();
            if published != 0 && transition != 0 {
                thread_pointer::restore_thread_pointer(published)?;
                return Ok(published);
            }

            let modules = modules().read().map_err(|_| TlsError::Poisoned)?;
            let images = modules
                .iter()
                .enumerate()
                .map(|(index, module)| (index + 1, module.image.as_slice()))
                .collect::<Vec<_>>();
            let block = thread_pointer::install(&images)?;
            let pointer = block.thread_pointer();
            publish_static_thread_pointer(pointer)?;
            publish_host_transition_pointer(block.transition_pointer())?;
            *slot.borrow_mut() = Some(block);
            Ok(pointer)
        })
        .unwrap_or(Err(TlsError::ThreadExiting))
}

/// VEH hook: restore the calling thread's previously installed guest FS base.
///
/// `Some(tp)` means the base changed and retrying the instruction can make
/// progress. `None` means no block exists or the base was already correct.
pub fn restore_current_thread_static_tls() -> Option<usize> {
    let pointer = get_current_thread_fs_base();
    if pointer == 0 {
        return None;
    }
    thread_pointer::restore_thread_pointer(pointer)
        .ok()
        .filter(|changed| *changed)
        .map(|_| pointer)
}

/// Sets the calling thread's active FS base (used by arch_prctl and runtime.settls).
pub fn set_current_thread_fs_base(base: usize) -> Result<(), TlsError> {
    publish_static_thread_pointer(base)
}

/// Gets the calling thread's active FS base.
pub fn get_current_thread_fs_base() -> usize {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::TlsGetValue;
        let slot = STATIC_TLS_TEB_SLOT.load(Ordering::Acquire);
        if slot == u32::MAX {
            return 0;
        }
        unsafe { TlsGetValue(slot) as usize }
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Returns an already instantiated TLS block for the calling thread without
/// allocating or installing one. Used by GNU RTLD_DI_TLS_DATA's soft lookup;
/// the returned pointer remains owned by this thread's existing TLS state.
pub fn current_elf_tls_data(module_id: usize) -> Option<*mut u8> {
    let module_index = module_id.checked_sub(1)?;
    if let Some(offset) = thread_pointer::offset_of(module_id) {
        let current = STATIC_ELF_TLS
            .try_with(|slot| {
                slot.try_borrow()
                    .ok()?
                    .as_ref()?
                    .thread_pointer()
                    .checked_sub(offset)
                    .map(|base| base as *mut u8)
            })
            .ok()
            .flatten();
        if current.is_some() {
            return current;
        }
    }
    ELF_TLS
        .try_with(|slots| {
            slots
                .try_borrow()
                .ok()?
                .get(module_index)?
                .as_ref()
                .map(|block| block.pointer)
        })
        .ok()
        .flatten()
}

/// Implements the x86_64 ELF general-dynamic `__tls_get_addr` operation.
///
/// Module IDs are one-based as required by the dynamic TLS ABI. Each host
/// thread lazily receives a separately initialized block for every module it
/// touches.
pub fn elf_tls_get_addr(module_id: usize, offset: usize) -> Result<*mut u8, TlsError> {
    if let Some(static_offset) = thread_pointer::offset_of(module_id) {
        if let Ok(tp) = install_current_thread_static_tls() {
            let base = tp
                .checked_sub(static_offset)
                .ok_or(TlsError::OffsetOutOfRange)?;
            let addr = base.checked_add(offset).ok_or(TlsError::OffsetOutOfRange)?;
            return Ok(addr as *mut u8);
        }
    }

    let module_index = module_id.checked_sub(1).ok_or(TlsError::UnknownModule)?;
    let modules = modules().read().map_err(|_| TlsError::Poisoned)?;
    let template = modules.get(module_index).ok_or(TlsError::UnknownModule)?;
    if offset >= template.memory_size {
        return Err(TlsError::OffsetOutOfRange);
    }

    ELF_TLS
        .try_with(|slots| {
            let mut slots = slots.borrow_mut();
            if slots.len() <= module_index {
                slots.resize_with(module_index + 1, || None);
            }
            if slots[module_index].is_none() {
                slots[module_index] = Some(TlsBlock::new(template).ok_or(TlsError::OutOfMemory)?);
            }
            let block = slots[module_index]
                .as_ref()
                .expect("TLS block was initialized above");
            // SAFETY: the offset was checked against the allocation's memory size.
            Ok(unsafe { block.pointer.add(offset) })
        })
        .unwrap_or(Err(TlsError::ThreadExiting))
}

/// Drops all dynamically allocated ELF TLS blocks for the calling thread.
pub fn reset_current_thread_elf_tls() {
    let _ = ELF_TLS.try_with(|slots| slots.borrow_mut().clear());
}

pub fn pthread_key_create(destructor: Option<KeyDestructor>) -> Result<u32, i32> {
    let mut keys = pthread_keys().write().map_err(|_| EAGAIN)?;
    if let Some((index, entry)) = keys
        .iter_mut()
        .enumerate()
        .find(|(_, entry)| !entry.active && entry.generation != u64::MAX)
    {
        *entry = KeyEntry {
            active: true,
            generation: entry.generation + 1,
            destructor,
        };
        return Ok(index as u32);
    }
    if keys.len() >= MAX_PTHREAD_KEYS {
        return Err(EAGAIN);
    }
    keys.push(KeyEntry {
        active: true,
        generation: 1,
        destructor,
    });
    Ok((keys.len() - 1) as u32)
}

pub fn pthread_key_delete(key: u32) -> Result<(), i32> {
    let mut keys = pthread_keys().write().map_err(|_| EINVAL)?;
    let entry = keys.get_mut(key as usize).ok_or(EINVAL)?;
    if !entry.active {
        return Err(EINVAL);
    }
    entry.active = false;
    entry.destructor = None;
    with_pthread_values(|values| {
        if let Some(value) = values.0.get_mut(key as usize) {
            *value = KeyValue::default();
        }
    });
    Ok(())
}

pub fn pthread_getspecific(key: u32) -> Result<*mut c_void, i32> {
    let keys = pthread_keys().read().map_err(|_| EINVAL)?;
    let generation = keys
        .get(key as usize)
        .filter(|entry| entry.active)
        .ok_or(EINVAL)?
        .generation;
    let value = with_pthread_values(|values| {
        let Some(value) = values.0.get_mut(key as usize) else {
            return 0;
        };
        if value.generation != generation {
            *value = KeyValue::default();
        }
        value.value
    });
    Ok(value.unwrap_or(0) as *mut c_void)
}

pub fn pthread_setspecific(key: u32, value: *mut c_void) -> Result<(), i32> {
    let keys = pthread_keys().read().map_err(|_| EINVAL)?;
    let generation = keys
        .get(key as usize)
        .filter(|entry| entry.active)
        .ok_or(EINVAL)?
        .generation;
    // A store with nowhere left to land is dropped rather than reported: the
    // thread is already gone, so nothing can observe the value, and an errno
    // here would send exiting guest code down a failure path for no reason.
    with_pthread_values(|values| {
        if values.0.len() <= key as usize {
            values.0.resize(key as usize + 1, KeyValue::default());
        }
        values.0[key as usize] = KeyValue {
            generation,
            value: value as usize,
        };
    });
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TlsError {
    InvalidTemplate,
    /// This machine does not permit `wrfsbase`, so no thread pointer can be set.
    ThreadPointerUnsupported,
    /// A reservation asked for an alignment that is zero or not a power of two.
    InvalidAlignment,
    /// A thread pointer is already installed, so the static layout cannot grow.
    LayoutFrozen,
    TooManyModules,
    UnknownModule,
    OffsetOutOfRange,
    OutOfMemory,
    Poisoned,
    /// Host TLS for the calling thread is being or has been torn down, so no
    /// block can be reached. Seen when guest code runs during thread exit.
    ThreadExiting,
}

// ---------------------------------------------------------------------------
// fork handoff
// ---------------------------------------------------------------------------

#[cfg(windows)]
const FORK_TLS_MAGIC: u64 = 0x4352_5954_4c53_4636; // "CRYTLSF6"

#[cfg(windows)]
fn serialize_fork_state() -> Option<Vec<u8>> {
    let module_guard = modules().read().ok()?;
    let key_guard = pthread_keys().read().ok()?;
    let errno_pointer = errno_location();
    let errno = errno();
    let tls_slots = ELF_TLS.with(|slots| {
        slots
            .borrow()
            .iter()
            .map(|slot| {
                slot.as_ref().map(|block| {
                    // SAFETY: the block owns `layout.size()` live bytes.
                    let bytes = unsafe {
                        std::slice::from_raw_parts(block.pointer, block.layout.size()).to_vec()
                    };
                    (block.pointer as usize, block.layout.align(), bytes)
                })
            })
            .collect::<Vec<_>>()
    });
    let values = PTHREAD_VALUES.with(|values| {
        values
            .borrow()
            .0
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if key_guard
                    .get(index)
                    .is_some_and(|key| key.active && key.generation == value.generation)
                {
                    *value
                } else {
                    KeyValue::default()
                }
            })
            .collect::<Vec<_>>()
    });
    let destructors = CXX_DESTRUCTORS.with(|records| records.borrow().0.clone());
    let static_layout = thread_pointer::fork_layout()?;

    let mut out = Vec::new();
    out.extend_from_slice(&FORK_TLS_MAGIC.to_le_bytes());
    out.extend_from_slice(&(errno_pointer as usize as u64).to_le_bytes());
    out.extend_from_slice(&errno.to_le_bytes());
    out.extend_from_slice(&(module_guard.len() as u32).to_le_bytes());
    out.extend_from_slice(&(key_guard.len() as u32).to_le_bytes());
    out.extend_from_slice(&(tls_slots.len() as u32).to_le_bytes());
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    out.extend_from_slice(&(destructors.len() as u32).to_le_bytes());
    for module in module_guard.iter() {
        out.extend_from_slice(&(module.memory_size as u64).to_le_bytes());
        out.extend_from_slice(&(module.align as u64).to_le_bytes());
        out.extend_from_slice(&(module.image.len() as u64).to_le_bytes());
        out.extend_from_slice(&module.image);
        while !out.len().is_multiple_of(8) {
            out.push(0);
        }
    }
    for key in key_guard.iter() {
        out.extend_from_slice(&(key.active as u64).to_le_bytes());
        out.extend_from_slice(&key.generation.to_le_bytes());
        out.extend_from_slice(
            &(key.destructor.map_or(0, |destructor| destructor as usize) as u64).to_le_bytes(),
        );
    }
    for slot in tls_slots {
        match slot {
            Some((pointer, align, bytes)) => {
                out.extend_from_slice(&1u64.to_le_bytes());
                out.extend_from_slice(&(align as u64).to_le_bytes());
                out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
                out.extend_from_slice(&(pointer as u64).to_le_bytes());
                out.extend_from_slice(&bytes);
                while !out.len().is_multiple_of(8) {
                    out.push(0);
                }
            }
            None => {
                out.extend_from_slice(&0u64.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
                out.extend_from_slice(&0u64.to_le_bytes());
            }
        }
    }
    for value in values {
        out.extend_from_slice(&value.generation.to_le_bytes());
        out.extend_from_slice(&(value.value as u64).to_le_bytes());
    }
    for record in destructors {
        out.extend_from_slice(&(record.callback as usize as u64).to_le_bytes());
        out.extend_from_slice(&(record.argument as u64).to_le_bytes());
        out.extend_from_slice(&(record.dso as u64).to_le_bytes());
    }
    out.extend_from_slice(&(static_layout.total as u64).to_le_bytes());
    out.extend_from_slice(&(static_layout.frozen as u32).to_le_bytes());
    out.extend_from_slice(&(static_layout.modules.len() as u32).to_le_bytes());
    for module in static_layout.modules {
        out.extend_from_slice(&(module.module_id as u64).to_le_bytes());
        out.extend_from_slice(&(module.offset as u64).to_le_bytes());
    }
    Some(out)
}

#[cfg(windows)]
struct ForkReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

#[cfg(windows)]
impl<'a> ForkReader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let value = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(value)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn align8(&mut self) -> Option<()> {
        self.at = self.at.checked_add(7)? & !7;
        (self.at <= self.bytes.len()).then_some(())
    }
}

#[cfg(windows)]
fn restore_fork_state(payload: &[u8]) -> Result<(), ()> {
    let mut reader = ForkReader {
        bytes: payload,
        at: 0,
    };
    if reader.u64() != Some(FORK_TLS_MAGIC) {
        return Err(());
    }
    let restored_errno_pointer = reader.u64().ok_or(())? as usize;
    let restored_errno = reader.u32().ok_or(())? as i32;
    let module_count = reader.u32().ok_or(())? as usize;
    let key_count = reader.u32().ok_or(())? as usize;
    let slot_count = reader.u32().ok_or(())? as usize;
    let value_count = reader.u32().ok_or(())? as usize;
    let destructor_count = reader.u32().ok_or(())? as usize;
    if module_count > MAX_TLS_MODULES
        || slot_count > module_count
        || key_count > MAX_PTHREAD_KEYS
        || value_count > key_count
    {
        return Err(());
    }

    let mut restored_modules = Vec::with_capacity(module_count);
    for _ in 0..module_count {
        let memory_size = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        let align = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        let image_len = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        if image_len > memory_size || !align.is_power_of_two() {
            return Err(());
        }
        let image = reader.take(image_len).ok_or(())?.to_vec();
        reader.align8().ok_or(())?;
        restored_modules.push(ModuleTemplate {
            image,
            memory_size,
            align,
        });
    }
    let mut restored_keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        let active = match reader.u64().ok_or(())? {
            0 => false,
            1 => true,
            _ => return Err(()),
        };
        let generation = reader.u64().ok_or(())?;
        let address = reader.u64().ok_or(())? as usize;
        if generation == 0 || (!active && address != 0) {
            return Err(());
        }
        let destructor = if address == 0 {
            None
        } else {
            // SAFETY: the owning guest/PE mapping was restored at the same base.
            Some(unsafe { core::mem::transmute::<usize, KeyDestructor>(address) })
        };
        restored_keys.push(KeyEntry {
            active,
            generation,
            destructor,
        });
    }
    let mut restored_slots = Vec::with_capacity(slot_count);
    for _ in 0..slot_count {
        let present = reader.u64().ok_or(())? != 0;
        let align = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        let len = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        let pointer = reader.u64().ok_or(())? as usize;
        if !present {
            restored_slots.push(None);
            continue;
        }
        let layout = Layout::from_size_align(len.max(1), align).map_err(|_| ())?;
        if !arena_contains(pointer, layout.size()) {
            return Err(());
        }
        let bytes = reader.take(len).ok_or(())?;
        reader.align8().ok_or(())?;
        // Parsing does not own or write the guest allocation. Later generation
        // validation may still reject the payload; constructing TlsBlock here
        // would free this live address when a rejected candidate gets dropped.
        restored_slots.push(Some((pointer, layout, bytes)));
    }
    let mut restored_values = Vec::with_capacity(value_count);
    for index in 0..value_count {
        let generation = reader.u64().ok_or(())?;
        let value = reader.u64().ok_or(())? as usize;
        if generation == 0 {
            if value != 0 {
                return Err(());
            }
        } else if !restored_keys
            .get(index)
            .is_some_and(|key| key.active && key.generation == generation)
        {
            return Err(());
        }
        restored_values.push(KeyValue { generation, value });
    }
    if destructor_count > payload.len().saturating_sub(reader.at) / 24 {
        return Err(());
    }
    let mut restored_destructors = Vec::with_capacity(destructor_count);
    for _ in 0..destructor_count {
        let address = reader.u64().ok_or(())? as usize;
        if address == 0 {
            return Err(());
        }
        restored_destructors.push(ThreadDestructor {
            // The fork restore contract keeps callback mappings at their bases.
            callback: unsafe { core::mem::transmute::<usize, KeyDestructor>(address) },
            argument: reader.u64().ok_or(())? as usize,
            dso: reader.u64().ok_or(())? as usize,
        });
    }
    let total = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
    let frozen = match reader.u32().ok_or(())? {
        0 => false,
        1 => true,
        _ => return Err(()),
    };
    let static_count = reader.u32().ok_or(())? as usize;
    if static_count > module_count {
        return Err(());
    }
    let mut static_layout = thread_pointer::Layout {
        modules: Vec::with_capacity(static_count),
        total,
        frozen,
    };
    let mut previous_end = 0;
    let mut seen = vec![false; module_count];
    for _ in 0..static_count {
        let module_id = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        let offset = usize::try_from(reader.u64().ok_or(())?).map_err(|_| ())?;
        let index = module_id.checked_sub(1).ok_or(())?;
        let template = restored_modules.get(index).ok_or(())?;
        if seen[index]
            || offset > total
            || offset % template.align != 0
            || offset
                .checked_sub(template.memory_size)
                .is_none_or(|start| start < previous_end)
        {
            return Err(());
        }
        seen[index] = true;
        previous_end = offset;
        static_layout.modules.push(thread_pointer::StaticModule {
            module_id,
            offset,
            size: template.memory_size,
        });
    }
    if previous_end != total || reader.at != payload.len() {
        return Err(());
    }
    if !arena_contains(restored_errno_pointer, core::mem::size_of::<i32>()) {
        return Err(());
    }
    let mut current_modules = modules().write().map_err(|_| ())?;
    let mut current_keys = pthread_keys().write().map_err(|_| ())?;
    thread_pointer::restore_fork_layout(static_layout)?;
    // All validation and fallible lock acquisition precede mutation or ownership
    // adoption. A malformed value generation cannot invalidate existing TLS.
    let restored_slots = restored_slots
        .into_iter()
        .map(|slot| {
            slot.map(|(pointer, layout, bytes)| {
                unsafe {
                    ptr::copy_nonoverlapping(bytes.as_ptr(), pointer as *mut u8, bytes.len())
                };
                TlsBlock {
                    pointer: pointer as *mut u8,
                    layout,
                }
            })
        })
        .collect();

    // The child bootstrap ran before the parent's fixed arena was copied over
    // it. Native TLS/statics may therefore still contain Rust owners whose
    // pointers describe the overwritten bootstrap arena. Dropping those values
    // would feed parent-owned live blocks into the restored allocator. Replace
    // the owners without running their destructors; their storage no longer
    // exists as the objects they originally represented.
    {
        let stale = std::mem::replace(&mut *current_modules, restored_modules);
        std::mem::forget(stale);
    }
    {
        let stale = std::mem::replace(&mut *current_keys, restored_keys);
        std::mem::forget(stale);
    }
    ELF_TLS.with(|slots| {
        let stale = std::mem::replace(&mut *slots.borrow_mut(), restored_slots);
        std::mem::forget(stale);
    });
    PTHREAD_VALUES.with(|values| {
        let stale = std::mem::replace(&mut values.borrow_mut().0, restored_values);
        std::mem::forget(stale);
    });
    CXX_DESTRUCTORS.with(|records| {
        let stale = std::mem::replace(&mut records.borrow_mut().0, restored_destructors);
        std::mem::forget(stale);
    });
    let restored_errno_pointer = restored_errno_pointer as *mut i32;
    ERRNO_POINTER.with(|slot| slot.set(restored_errno_pointer));
    // SAFETY: the pointer was validated inside the copied fixed arena.
    unsafe { restored_errno_pointer.write(restored_errno) };
    Ok(())
}

#[cfg(windows)]
fn arena_contains(pointer: usize, len: usize) -> bool {
    pointer >= kinakaze_alloc::ARENA_BASE + 4096
        && pointer
            .checked_add(len)
            .is_some_and(|end| end <= kinakaze_alloc::ARENA_BASE + kinakaze_alloc::ARENA_SIZE)
}

#[cfg(windows)]
unsafe extern "system" fn fork_snapshot(buffer: *mut u8, capacity: usize) -> isize {
    let Some(payload) = serialize_fork_state() else {
        return -(EAGAIN as isize);
    };
    if buffer.is_null() {
        return payload.len() as isize;
    }
    if capacity < payload.len() {
        return -(EAGAIN as isize);
    }
    // SAFETY: the runtime supplied the queried writable buffer.
    unsafe { ptr::copy_nonoverlapping(payload.as_ptr(), buffer, payload.len()) };
    payload.len() as isize
}

#[cfg(windows)]
unsafe extern "system" fn fork_child(payload: *const u8, len: usize) -> i32 {
    if payload.is_null() && len != 0 {
        return EINVAL;
    }
    // The executable bootstrap may have installed a module-local ThreadBlock
    // before the parent arena replaced its allocations. It is a stale owner,
    // not child state to preserve. Disarm it before restoration can allocate;
    // the process-level loader participant republishes the copied guest TP and
    // host transition block later in the same fork transaction.
    let _ = STATIC_ELF_TLS.try_with(|slot| {
        if let Some(stale) = slot.borrow_mut().take() {
            std::mem::forget(stale);
        }
    });
    let bytes = if len == 0 {
        &[]
    } else {
        // SAFETY: the runtime owns this readable payload for the call.
        unsafe { std::slice::from_raw_parts(payload, len) }
    };
    if restore_fork_state(bytes).is_ok() {
        0
    } else {
        EINVAL
    }
}

#[cfg(windows)]
fn register_fork_handoff() {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: FROM_ADDRESS interprets the pointer as an address, never a string.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            register_fork_handoff as *const () as *const u16,
            &mut module,
        )
    };
    if found == 0 || module.is_null() {
        return;
    }
    let key = 0x544c_535f_484f_4f4bu64 ^ (module as usize as u64).rotate_left(17);
    let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
        abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
        priority: 20,
        key,
        prepare: None,
        snapshot: Some(fork_snapshot),
        parent: None,
        child: Some(fork_child),
    });
}

#[cfg(windows)]
extern "C" fn fork_initializer() {
    register_fork_handoff();
}

#[cfg(windows)]
#[used]
#[unsafe(link_section = ".CRT$XCU")]
static FORK_INITIALIZER: extern "C" fn() = fork_initializer;

#[cfg(test)]
mod tests {
    use super::*;

    // Key reuse and fork restore mutate the process-wide registry. Run only
    // these regressions in private test processes so parallel unrelated tests
    // cannot take the recycled numeric index or lose their restored registry.
    fn isolated_case(name: &str) -> bool {
        if std::env::var("KINAKAZE_TLS_TEST_CASE").ok().as_deref() == Some(name) {
            return true;
        }
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                &format!("tests::{name}"),
                "--test-threads=1",
                "--nocapture",
            ])
            .env("KINAKAZE_TLS_TEST_CASE", name);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "isolated {name}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        false
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn deleted_key_reuse_hides_stale_values_and_skips_stale_destructors() {
        if !isolated_case("deleted_key_reuse_hides_stale_values_and_skips_stale_destructors") {
            return;
        }
        static OLD_CALLS: AtomicUsize = AtomicUsize::new(0);
        static NEW_VALUES: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "sysv64" fn old_destructor(_: *mut c_void) {
            OLD_CALLS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe extern "sysv64" fn new_destructor(value: *mut c_void) {
            NEW_VALUES.fetch_add(value as usize, Ordering::SeqCst);
        }
        let key = pthread_key_create(Some(old_destructor)).unwrap();
        let (ready, ready_rx) = std::sync::mpsc::channel();
        let (resume, resume_rx) = std::sync::mpsc::channel();
        let ready_second = ready.clone();
        let first = std::thread::spawn(move || {
            pthread_setspecific(key, 0x10usize as *mut c_void).unwrap();
            ready.send(()).unwrap();
            resume_rx.recv().unwrap();
            assert!(pthread_getspecific(key).unwrap().is_null());
            pthread_setspecific(key, 0x80usize as *mut c_void).unwrap();
        });
        let (resume_second, resume_second_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            pthread_setspecific(key, 0x20usize as *mut c_void).unwrap();
            ready_second.send(()).unwrap();
            resume_second_rx.recv().unwrap();
            // Never call getspecific: destructor dispatch itself must reject
            // this stale generation, even if no earlier read cleared the slot.
        });
        ready_rx.recv().unwrap();
        ready_rx.recv().unwrap();
        pthread_key_delete(key).unwrap();
        let reused = pthread_key_create(Some(new_destructor)).unwrap();
        assert_eq!(reused, key);
        resume.send(()).unwrap();
        resume_second.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert_eq!(OLD_CALLS.load(Ordering::SeqCst), 0);
        assert_eq!(NEW_VALUES.load(Ordering::SeqCst), 0x80);
        pthread_key_delete(reused).unwrap();
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn destructor_reusing_later_key_invalidates_queued_callback() {
        if !isolated_case("destructor_reusing_later_key_invalidates_queued_callback") {
            return;
        }
        static TARGET: AtomicU32 = AtomicU32::new(u32::MAX);
        static LATER_CALLS: AtomicUsize = AtomicUsize::new(0);
        unsafe extern "sysv64" fn later(_: *mut c_void) {
            LATER_CALLS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe extern "sysv64" fn replace(_: *mut c_void) {
            let key = TARGET.load(Ordering::SeqCst);
            pthread_key_delete(key).unwrap();
            assert_eq!(pthread_key_create(Some(later)).unwrap(), key);
        }
        let first = pthread_key_create(Some(replace)).unwrap();
        let second = pthread_key_create(Some(later)).unwrap();
        TARGET.store(second, Ordering::SeqCst);
        std::thread::spawn(move || {
            pthread_setspecific(first, 1usize as *mut c_void).unwrap();
            pthread_setspecific(second, 2usize as *mut c_void).unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(LATER_CALLS.load(Ordering::SeqCst), 0);
        pthread_key_delete(first).unwrap();
        pthread_key_delete(second).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn fork_snapshot_roundtrip_preserves_key_generations() {
        if !isolated_case("fork_snapshot_roundtrip_preserves_key_generations") {
            return;
        }
        let first = pthread_key_create(None).unwrap();
        pthread_key_delete(first).unwrap();
        assert_eq!(pthread_key_create(None).unwrap(), first);
        pthread_setspecific(first, 0x5678usize as *mut c_void).unwrap();
        let inactive = pthread_key_create(None).unwrap();
        pthread_key_delete(inactive).unwrap();
        // Another thread may retain this exact stale record at fork time.
        PTHREAD_VALUES.with(|values| {
            values
                .borrow_mut()
                .0
                .resize(inactive as usize + 1, KeyValue::default());
            values.borrow_mut().0[inactive as usize] = KeyValue {
                generation: 1,
                value: 0xdead,
            };
        });
        let payload = serialize_fork_state().unwrap();
        restore_fork_state(&payload).unwrap();
        assert_eq!(pthread_getspecific(first).unwrap() as usize, 0x5678);
        assert_eq!(pthread_getspecific(inactive), Err(EINVAL));
        assert_eq!(pthread_keys().read().unwrap()[first as usize].generation, 2);
        assert_eq!(
            pthread_keys().read().unwrap()[inactive as usize].generation,
            1
        );
        assert_eq!(
            PTHREAD_VALUES.with(|values| values.borrow().0[inactive as usize]),
            KeyValue::default()
        );
        assert_eq!(serialize_fork_state().unwrap(), payload);

        // This isolated fixture has no ELF modules or live TLS blocks. A
        // mismatched stamped value must fail without publishing key state.
        assert!(modules().read().unwrap().is_empty());
        assert!(ELF_TLS.with(|slots| slots.borrow().is_empty()));
        let count = pthread_keys().read().unwrap().len();
        let value_offset = 40 + count * 24;
        let mut corrupt = payload.clone();
        corrupt[value_offset..value_offset + 8].copy_from_slice(&3u64.to_le_bytes());
        assert_eq!(restore_fork_state(&corrupt), Err(()));
        assert_eq!(pthread_getspecific(first).unwrap() as usize, 0x5678);
        assert_eq!(pthread_key_create(None).unwrap(), inactive);
        assert_eq!(
            pthread_keys().read().unwrap()[inactive as usize].generation,
            2
        );
        pthread_key_delete(first).unwrap();
        pthread_key_delete(inactive).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn fork_static_layout_initializes_new_threads_and_rejects_overlap() {
        if !isolated_case("fork_static_layout_initializes_new_threads_and_rejects_overlap") {
            return;
        }
        if !thread_pointer::supported() {
            return;
        }
        let first = register_elf_module(&[0x31], 32, 16).unwrap();
        let second = register_elf_module(&[0x42], 64, 32).unwrap();
        let first_offset = reserve_static_elf_module(first).unwrap();
        let second_offset = reserve_static_elf_module(second).unwrap();
        let payload = serialize_fork_state().unwrap();
        let mut corrupt = payload.clone();
        let last = corrupt.len() - 8;
        corrupt[last..].copy_from_slice(&(first_offset as u64).to_le_bytes());
        assert_eq!(restore_fork_state(&corrupt), Err(()));
        assert_eq!(thread_pointer::offset_of(second), Some(second_offset));

        // The native helper starts with its own layout, independent of the
        // copied guest mappings. Restore the parent's relocations verbatim.
        thread_pointer::restore_fork_layout(thread_pointer::Layout::default()).unwrap();
        restore_fork_state(&payload).unwrap();
        assert_eq!(thread_pointer::offset_of(first), Some(first_offset));
        assert_eq!(thread_pointer::offset_of(second), Some(second_offset));
        std::thread::spawn(move || {
            let a = elf_tls_get_addr(first, 0).unwrap();
            let b = elf_tls_get_addr(second, 0).unwrap();
            assert_eq!(unsafe { a.read() }, 0x31);
            assert_eq!(unsafe { b.read() }, 0x42);
            assert_eq!(unsafe { b.add(63).read() }, 0);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn exhausted_key_generation_is_never_reused() {
        if !isolated_case("exhausted_key_generation_is_never_reused") {
            return;
        }
        let exhausted = pthread_key_create(None).unwrap();
        pthread_key_delete(exhausted).unwrap();
        pthread_keys().write().unwrap()[exhausted as usize].generation = u64::MAX;
        let fresh = pthread_key_create(None).unwrap();
        assert_ne!(fresh, exhausted);
        assert_eq!(pthread_keys().read().unwrap()[fresh as usize].generation, 1);
        pthread_key_delete(fresh).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn rejected_key_generation_does_not_adopt_or_overwrite_elf_tls() {
        if !isolated_case("rejected_key_generation_does_not_adopt_or_overwrite_elf_tls") {
            return;
        }
        let key = pthread_key_create(None).unwrap();
        pthread_setspecific(key, 0x10usize as *mut c_void).unwrap();
        let module = register_elf_module(&[0xaa, 0x55], 16, 8).unwrap();
        let pointer = elf_tls_get_addr(module, 0).unwrap();
        let mut payload = serialize_fork_state().unwrap();
        // This fixture has one value and no C++ destructor footer.
        let value_offset = payload.len() - 16 - 16; // value, then empty static-layout footer
        payload[value_offset..value_offset + 8].copy_from_slice(&2u64.to_le_bytes());
        unsafe { pointer.write(0x42) };
        assert_eq!(restore_fork_state(&payload), Err(()));
        assert_eq!(current_elf_tls_data(module), Some(pointer));
        assert_eq!(unsafe { pointer.read() }, 0x42);
        assert_eq!(pthread_getspecific(key).unwrap() as usize, 0x10);
        pthread_key_delete(key).unwrap();
        reset_current_thread_elf_tls();
    }

    #[test]
    fn soft_dynamic_tls_lookup_does_not_instantiate_a_block() {
        if !isolated_case("soft_dynamic_tls_lookup_does_not_instantiate_a_block") {
            return;
        }
        let module = register_elf_module(&[3, 4], 16, 8).unwrap();
        assert_eq!(current_elf_tls_data(0), None);
        assert_eq!(current_elf_tls_data(module), None);
        assert!(ELF_TLS.with(|slots| slots.borrow().is_empty()));
        let pointer = elf_tls_get_addr(module, 0).unwrap();
        assert_eq!(current_elf_tls_data(module), Some(pointer));
        assert!(
            std::thread::spawn(move || current_elf_tls_data(module).is_none())
                .join()
                .unwrap()
        );
        reset_current_thread_elf_tls();
        assert_eq!(current_elf_tls_data(module), None);
    }

    #[test]
    fn soft_static_tls_lookup_requires_an_existing_thread_block() {
        if !isolated_case("soft_static_tls_lookup_requires_an_existing_thread_block") {
            return;
        }
        if !thread_pointer::supported() {
            return;
        }
        let module = register_elf_module(&[7, 8], 16, 8).unwrap();
        let offset = thread_pointer::reserve(module, 16, 8).unwrap();
        assert_eq!(current_elf_tls_data(module), None);
        assert!(STATIC_ELF_TLS.with(|slot| slot.borrow().is_none()));
        let pointer = install_current_thread_static_tls().unwrap();
        let data = current_elf_tls_data(module).unwrap();
        assert_eq!(data as usize, pointer - offset);
        assert_eq!(unsafe { std::slice::from_raw_parts(data, 2) }, &[7, 8]);
        assert!(ELF_TLS.with(|slots| slots.borrow().is_empty()));
    }

    #[test]
    #[cfg(target_arch = "x86_64")]
    fn cxx_destructors_run_lifo_on_host_tls_teardown() {
        struct Records(std::sync::Mutex<Vec<usize>>);
        struct Argument {
            records: usize,
            value: usize,
        }
        unsafe extern "sysv64" fn record(argument: *mut c_void) {
            let argument = unsafe { &*argument.cast::<Argument>() };
            let records = unsafe { &*(argument.records as *const Records) };
            records.0.lock().unwrap().push(argument.value);
        }
        let records = Records(std::sync::Mutex::new(Vec::new()));
        let first = Argument {
            records: &raw const records as usize,
            value: 1,
        };
        let second = Argument {
            records: &raw const records as usize,
            value: 2,
        };
        let first_address = &raw const first as usize;
        let second_address = &raw const second as usize;
        std::thread::spawn(move || {
            cxa_thread_atexit(Some(record), first_address as *mut c_void, ptr::null_mut()).unwrap();
            cxa_thread_atexit(Some(record), second_address as *mut c_void, ptr::null_mut())
                .unwrap();
        })
        .join()
        .unwrap();
        assert_eq!(*records.0.lock().unwrap(), vec![2, 1]);
        assert_eq!(
            cxa_thread_atexit(None, ptr::null_mut(), ptr::null_mut()),
            Err(EINVAL)
        );
    }

    #[test]
    fn errno_is_thread_local() {
        set_errno(7);
        let child = std::thread::spawn(|| {
            assert_eq!(errno(), 0);
            set_errno(19);
            assert_eq!(errno(), 19);
        });
        child.join().unwrap();
        assert_eq!(errno(), 7);
    }

    #[test]
    fn elf_tls_is_initialized_per_thread() {
        let module = register_elf_module(&[1, 2, 3, 4], 16, 8).unwrap();
        let first = elf_tls_get_addr(module, 2).unwrap();
        // SAFETY: the registered TLS block has 16 bytes.
        unsafe {
            assert_eq!(*first, 3);
            *first = 42;
        }
        let child = std::thread::spawn(move || {
            let value = elf_tls_get_addr(module, 2).unwrap();
            // SAFETY: the registered TLS block has 16 bytes.
            unsafe { *value }
        });
        assert_eq!(child.join().unwrap(), 3);
        // SAFETY: `first` remains owned by the current thread TLS state.
        assert_eq!(unsafe { *first }, 42);
    }

    #[test]
    fn pthread_keys_are_isolated_by_thread() {
        let key = pthread_key_create(None).unwrap();
        pthread_setspecific(key, 0x1234usize as *mut c_void).unwrap();
        let child = std::thread::spawn(move || {
            assert!(pthread_getspecific(key).unwrap().is_null());
            pthread_setspecific(key, 0x5678usize as *mut c_void).unwrap();
            pthread_getspecific(key).unwrap() as usize
        });
        assert_eq!(child.join().unwrap(), 0x5678);
        assert_eq!(pthread_getspecific(key).unwrap() as usize, 0x1234);
        pthread_key_delete(key).unwrap();
    }

    /// A key destructor is guest code, and POSIX lets it call back into
    /// `pthread_{get,set}specific`. Those calls arrive while `PTHREAD_VALUES`
    /// is mid-drop, where `LocalKey::with` raises `AccessError` -- a panic that
    /// cannot cross the `extern "sysv64"` boundary and aborts the process
    /// instead. Real guests hit this: OpenSSH aborted on every exit.
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn key_destructors_may_call_back_into_specific() {
        static KEY: AtomicU32 = AtomicU32::new(u32::MAX);
        static PASSES: AtomicUsize = AtomicUsize::new(0);
        static OBSERVED: AtomicUsize = AtomicUsize::new(0);

        unsafe extern "sysv64" fn destructor(value: *mut c_void) {
            let key = KEY.load(Ordering::Relaxed);
            let pass = PASSES.fetch_add(1, Ordering::Relaxed);
            // POSIX clears the slot before handing the value to the destructor.
            if pthread_getspecific(key).unwrap().is_null() {
                OBSERVED.fetch_add(value as usize, Ordering::Relaxed);
            }
            // Storing a new value earns one more pass, up to the four POSIX
            // allows. Only the second pass can prove that loop still turns.
            if pass == 0 {
                pthread_setspecific(key, 0x20 as *mut c_void).unwrap();
            }
        }

        let key = pthread_key_create(Some(destructor)).unwrap();
        KEY.store(key, Ordering::Relaxed);
        std::thread::spawn(move || {
            pthread_setspecific(key, 0x10 as *mut c_void).unwrap();
        })
        .join()
        .unwrap();

        assert_eq!(PASSES.load(Ordering::Relaxed), 2);
        assert_eq!(OBSERVED.load(Ordering::Relaxed), 0x30);
        pthread_key_delete(key).unwrap();
    }

    /// Once the thread is gone there is no block to reach, and a key reads back
    /// as unset rather than taking the process down with it.
    #[test]
    fn specific_calls_after_teardown_report_no_value() {
        let key = pthread_key_create(None).unwrap();
        std::thread::spawn(move || {
            pthread_setspecific(key, 0x99 as *mut c_void).unwrap();
        })
        .join()
        .unwrap();
        assert!(pthread_getspecific(key).unwrap().is_null());
        pthread_key_delete(key).unwrap();
    }

    #[test]
    fn errno_per_thread_isolation() {
        set_errno(42);
        assert_eq!(errno(), 42);

        let child = std::thread::spawn(|| {
            assert_eq!(errno(), 0);
            set_errno(99);
            assert_eq!(errno(), 99);
        });

        child.join().unwrap();
        // Main thread's errno must remain 42
        assert_eq!(errno(), 42);
        set_errno(0);
    }

    #[test]
    fn static_tls_block_layout_and_canary() {
        use crate::thread_pointer::install;

        let block = install(&[]).expect("thread_pointer::install");
        let tp = block.thread_pointer();
        assert_ne!(tp, 0);

        // On x86_64 ABI Variant II, tp points to self-pointer at offset 0
        let self_ptr = unsafe { *(tp as *const usize) };
        assert_eq!(self_ptr, tp);

        // Stack canary is placed at offset 0x28 (%fs:0x28)
        let canary_ptr = (tp + 0x28) as *const u64;
        let canary = unsafe { *canary_ptr };
        assert_eq!(canary, block.canary());
    }
}
