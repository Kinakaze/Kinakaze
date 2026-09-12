//! Versioned, process-local C ABI shared by the runtime and provider DLLs.
//!
//! Only serialized protocol values cross RPC. These pointers are borrowed in
//! one Windows process and remain valid only while the runtime session lives.

use core::ffi::c_void;

pub const ABI_VERSION: u32 = 1;
pub const RPC_BUFFER_SIZE: usize = 64 * 1024;
pub const STATUS_OK: i32 = 0;
pub const STATUS_INVALID_ARGUMENT: i32 = -1;
pub const STATUS_TRANSPORT: i32 = -2;
pub const STATUS_PROTOCOL: i32 = -3;
pub const STATUS_NOT_INITIALIZED: i32 = -4;
pub const STATUS_BUFFER_TOO_SMALL: i32 = -5;
pub const STATUS_INTERNAL: i32 = -6;
pub const STATUS_REMOTE: i32 = -7;

pub type RuntimeCallV1 = unsafe extern "C" fn(
    context: *mut c_void,
    request: *const u8,
    request_len: u32,
    response: *mut u8,
    response_capacity: u32,
    response_len: *mut u32,
) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RuntimeApiV1 {
    pub abi_version: u32,
    pub struct_size: u32,
    pub context: *mut c_void,
    pub call: RuntimeCallV1,
}

pub type RuntimeOpenV1 = unsafe extern "C" fn(
    config_json: *const u8,
    config_len: u32,
    api_out: *mut RuntimeApiV1,
) -> i32;
pub type RuntimeCloseV1 = unsafe extern "C" fn(api: *mut RuntimeApiV1) -> i32;
pub type ProviderInitializeV1 = unsafe extern "C" fn(api: *const RuntimeApiV1) -> i32;

/// Called after fork adoption to obtain the child's current session table.
/// Success initializes `output`; no borrowed parent session pointer is retained.
pub type RuntimeApiResolverV1 = unsafe extern "C" fn(output: *mut RuntimeApiV1) -> i32;

/// Fork policy used by the management ABI, not Linux clone flag bits.
pub const FORK_COPY: u32 = 0;
pub const FORK_SHARE: u32 = 1;
pub const FORK_RESET: u32 = 2;

/// Run one Linux ELF image inside the worker. Configuration is bounded JSON;
/// a real guest exit terminates this native worker with the guest's status.
pub type RuntimeGuestRunV1 =
    unsafe extern "C" fn(api: *const RuntimeApiV1, config_json: *const u8, config_len: u32) -> i32;

pub const HELPER_UNIX_RIGHTS: u32 = 1;
pub const HELPER_USERNET: u32 = 2;
/// Authenticate and run one native helper, without allocating a Linux PID.
pub type RuntimeHelperRunV1 =
    unsafe extern "C" fn(kind: u32, config_json: *const u8, config_len: u32) -> i32;

pub type ProviderStateDefineV1 =
    unsafe extern "C" fn(name: *const u8, name_len: u32, initial: u64, fork_policy: u32) -> i32;
pub type ProviderStateReadV1 =
    unsafe extern "C" fn(name: *const u8, name_len: u32, value_out: *mut u64) -> i32;
pub type ProviderStateWriteV1 =
    unsafe extern "C" fn(name: *const u8, name_len: u32, value: u64) -> i32;
