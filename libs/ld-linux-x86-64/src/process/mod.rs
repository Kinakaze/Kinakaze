//! The unique live linker, recursive lock, fork publication and thread errors.
//! Guest entry points share this state; libc/libdl expose aliases, not caches.
mod dlinfo;
mod finalizers;
mod fork;
pub(crate) use finalizers::rtld_fini;
pub use finalizers::run_finalizers;
mod info;
mod lookup;
use crate::Linker;
pub use dlinfo::kinakaze_process_dlinfo;
pub use info::{kinakaze_process_dl_iterate_phdr, kinakaze_process_dladdr};
pub use lookup::{
    kinakaze_process_dlclose, kinakaze_process_dlopen, kinakaze_process_dlsym,
    kinakaze_process_dlvsym,
};
use std::cell::RefCell;
use std::ffi::CString;
use std::ptr;
use windows_sys::Win32::System::Threading::{
    CRITICAL_SECTION, EnterCriticalSection, InitializeCriticalSection, LeaveCriticalSection,
};

struct ActiveLinker(std::cell::UnsafeCell<*mut Linker>);

// SAFETY: runtime mutation and access are serialized by the process loader
// lock. Startup publication and fork-child restoration happen before guest
// threads can enter a dynamic-loader callback.
unsafe impl Sync for ActiveLinker {}

static ACTIVE_LINKER: ActiveLinker = ActiveLinker(std::cell::UnsafeCell::new(ptr::null_mut()));

struct DynamicLoaderLock(std::cell::UnsafeCell<CRITICAL_SECTION>);

// SAFETY: CRITICAL_SECTION provides its own synchronization. Initialization
// happens before normal execution, and fork-child restore rebuilds the fresh
// runtime's internal lock before guest code resumes.
unsafe impl Sync for DynamicLoaderLock {}

static DYNAMIC_LOADER_LOCK: DynamicLoaderLock =
    DynamicLoaderLock(std::cell::UnsafeCell::new(CRITICAL_SECTION {
        DebugInfo: ptr::null_mut(),
        LockCount: 0,
        RecursionCount: 0,
        OwningThread: ptr::null_mut(),
        LockSemaphore: ptr::null_mut(),
        SpinCount: 0,
    }));

struct DynamicLoaderGuard;

impl Drop for DynamicLoaderGuard {
    fn drop(&mut self) {
        // SAFETY: every successful lock owns one recursive acquisition.
        unsafe { LeaveCriticalSection(DYNAMIC_LOADER_LOCK.0.get()) };
    }
}

fn dynamic_loader_lock() -> DynamicLoaderGuard {
    // A Windows critical section is recursive for its owning thread, matching
    // the loader lock required when a DSO initializer calls dlopen again.
    unsafe { EnterCriticalSection(DYNAMIC_LOADER_LOCK.0.get()) };
    DynamicLoaderGuard
}

fn initialize_dynamic_loader_lock() {
    // SAFETY: called once from the runtime's early CRT slot, before the
    // fork-child interceptor can load registered provider DLLs.
    unsafe { InitializeCriticalSection(DYNAMIC_LOADER_LOCK.0.get()) };
}

extern "C" fn dynamic_loader_lock_initializer() {
    initialize_dynamic_loader_lock();
}

#[used]
#[unsafe(link_section = ".CRT$XCS")]
static DYNAMIC_LOADER_LOCK_INITIALIZER: extern "C" fn() = dynamic_loader_lock_initializer;

fn lock_dynamic_loader_for_fork() -> bool {
    let guard = dynamic_loader_lock();
    core::mem::forget(guard);
    true
}

fn unlock_dynamic_loader_after_fork() {
    // SAFETY: fork prepare retained exactly one recursive acquisition.
    unsafe { LeaveCriticalSection(DYNAMIC_LOADER_LOCK.0.get()) };
}

struct DlErrorState {
    message: Option<CString>,
    reported: bool,
}

thread_local! {
    static DL_ERROR: RefCell<DlErrorState> = const {
        RefCell::new(DlErrorState { message: None, reported: true })
    };
}

fn snapshot_active_linker() -> Result<Vec<u8>, crate::LinkError> {
    let pointer = loaded_linker();
    if pointer.is_null() {
        return Err(crate::LinkError::InvalidProvider(
            "no active linker to snapshot".into(),
        ));
    }
    // Fork prepare owns the loader lock until all participant snapshots finish.
    unsafe { (&*pointer).snapshot_fork() }
}

unsafe fn restore_active_linker(payload: &[u8]) -> Result<(), crate::LinkError> {
    let linker = unsafe { Linker::restore_fork(payload)? };
    unsafe { linker.restore_c_allocator() };
    // The fresh child owns all metadata; guest addresses remain in runtime-
    // restored mappings. Keep this owner until process exit, like the main run.
    unsafe { ACTIVE_LINKER.0.get().write(Box::into_raw(Box::new(linker))) };
    Ok(())
}

pub fn publish_active_linker(linker: &mut Linker) -> Result<(), crate::LinkError> {
    fork::register()?;
    // SAFETY: initial publication happens before guest code can call dl*.
    unsafe { ACTIVE_LINKER.0.get().write(linker as *mut Linker) };
    Ok(())
}

fn loaded_linker() -> *mut Linker {
    // SAFETY: every runtime caller holds the process loader lock.
    unsafe { ACTIVE_LINKER.0.get().read() }
}

/// Runs the main executable's constructor phase requested by modern libc.
///
/// Dependency constructors have already run in the loader. This entry point
/// supplies the part glibc normally obtains from its private link-map: the
/// executable's `DT_INIT` and `DT_INIT_ARRAY` entries.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_process_run_main_initializers(
    argc: i32,
    argv: *const *const u8,
    envp: *const *const u8,
) -> i32 {
    let _guard = dynamic_loader_lock();
    let linker = loaded_linker();
    if linker.is_null() {
        eprintln!("kinakaze: main initialization requested without an active linker");
        return -1;
    }
    // SAFETY: ACTIVE_LINKER names the boxed process linker and the recursive
    // loader lock serializes mutable access, including constructor dlopen.
    match unsafe { (&mut *linker).run_executable_initializers(argc, argv, envp) } {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("kinakaze: main executable initialization failed: {error}");
            -1
        }
    }
}

pub fn clear_active_linker() {
    // SAFETY: the guest has returned and its finalizers have completed.
    unsafe { kinakaze_alloc::c::install(0, 0, 0) };
    unsafe { ACTIVE_LINKER.0.get().write(ptr::null_mut()) };
}

extern "C" fn install_loader_services() {
    kinakaze_runtime::services::install_main_initializers(kinakaze_process_run_main_initializers);
    kinakaze_runtime::services::install_unwind(kinakaze_runtime::services::UnwindServices {
        backtrace: unwind::backtrace,
        address_info: info::kinakaze_process_dladdr,
        find_object: info::find_object,
    });
}

mod unwind;

#[used]
#[unsafe(link_section = ".CRT$XCU")]
static LOADER_SERVICES: extern "C" fn() = install_loader_services;

fn set_dl_error(error: impl std::fmt::Display) {
    let message =
        CString::new(error.to_string().replace('\0', "?")).expect("sanitized dlerror message");
    DL_ERROR.with(|state| {
        *state.borrow_mut() = DlErrorState {
            message: Some(message),
            reported: false,
        };
    });
}

fn clear_dl_error() {
    DL_ERROR.with(|state| state.borrow_mut().reported = true);
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_process_dlerror() -> *const core::ffi::c_char {
    DL_ERROR.with(|state| {
        let mut state = state.borrow_mut();
        if state.reported {
            return ptr::null();
        }
        state.reported = true;
        state
            .message
            .as_ref()
            .map_or(ptr::null(), |message| message.as_ptr())
    })
}
