//! Process services installed by their owning native module before guest entry.
//! The owner remains loaded for the process lifetime; fork children install their
//! own addresses during normal module initialization.
use std::sync::OnceLock;

pub type CloneThread =
    unsafe extern "sysv64" fn(usize, usize, usize, u64, *mut i32, *mut i32, u64, usize) -> i64;
pub type MainInitializers =
    unsafe extern "sysv64" fn(i32, *const *const u8, *const *const u8) -> i32;
pub type TimerSyscall = unsafe extern "sysv64" fn(u64, u64, u64, u64, u64) -> i64;

pub struct EngineServices {
    pub clone_thread: CloneThread,
}

static ENGINE: OnceLock<EngineServices> = OnceLock::new();
static MAIN_INITIALIZERS: OnceLock<MainInitializers> = OnceLock::new();
static TIMERS: OnceLock<TimerSyscall> = OnceLock::new();
static TASK_LIMIT: OnceLock<fn() -> i32> = OnceLock::new();

pub fn install_task_limit(check: fn() -> i32) {
    assert!(
        TASK_LIMIT.set(check).is_ok(),
        "task limit provider already installed"
    );
}

pub fn task_creation_errno() -> i32 {
    TASK_LIMIT.get().map_or(0, |check| check())
}

#[repr(C)]
pub struct AddressInfo {
    pub dli_fname: *const core::ffi::c_char,
    pub dli_fbase: *mut core::ffi::c_void,
    pub dli_sname: *const core::ffi::c_char,
    pub dli_saddr: *mut core::ffi::c_void,
}

/// GNU x86-64 `struct dl_find_object` (including ABI-reserved words).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FindObjectInfo {
    pub flags: u64,
    pub map_start: usize,
    pub map_end: usize,
    pub link_map: usize,
    pub eh_frame: usize,
    pub reserved: [u64; 7],
}

pub struct UnwindServices {
    pub backtrace: unsafe extern "sysv64" fn(*const u64, *mut usize, usize) -> usize,
    pub address_info: unsafe extern "sysv64" fn(*const core::ffi::c_void, *mut AddressInfo) -> i32,
    pub find_object:
        unsafe extern "sysv64" fn(*const core::ffi::c_void, *mut FindObjectInfo) -> i32,
}

static UNWIND: OnceLock<UnwindServices> = OnceLock::new();

pub fn install_unwind(services: UnwindServices) {
    assert!(
        UNWIND.set(services).is_ok(),
        "unwind services already installed"
    );
}

pub fn unwind() -> Option<&'static UnwindServices> {
    UNWIND.get()
}

pub fn install_engine(services: EngineServices) {
    assert!(
        ENGINE.set(services).is_ok(),
        "engine services already installed"
    );
}

pub fn engine() -> Option<&'static EngineServices> {
    ENGINE.get()
}

pub fn install_main_initializers(entry: MainInitializers) {
    assert!(
        MAIN_INITIALIZERS.set(entry).is_ok(),
        "loader services already installed"
    );
}

pub fn main_initializers() -> Option<MainInitializers> {
    MAIN_INITIALIZERS.get().copied()
}

pub fn install_timers(entry: TimerSyscall) {
    assert!(TIMERS.set(entry).is_ok(), "timer service already installed");
}

pub fn timers() -> Option<TimerSyscall> {
    TIMERS.get().copied()
}
