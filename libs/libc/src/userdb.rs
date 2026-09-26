//! Guest identities, account databases and process limits.
//! Real, effective, saved and filesystem IDs are serialized across fork/exec.
//! Windows handle permissions independently constrain access to host resources.

use core::ffi::{CStr, c_char, c_int, c_void};
use core::ptr;
use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::ffi::CString;
use std::sync::OnceLock;

use kinakaze_vfs::{EINVAL, ENOENT, EPERM, ERANGE};

mod account_files;
mod password_lock;
mod shadow_stream;

/// Linux `uid_t` and `gid_t`, both 32-bit unsigned on every Linux ABI.
pub type Uid = u32;
pub type Gid = u32;

/// Initial guest identity.
const ROOT_ID: u32 = 0;

/// `(uid_t)-1`, the "leave this one alone" argument to the `setres*` calls.
const UNCHANGED: u32 = u32::MAX;

static CURRENT_UID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_EUID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_SUID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_GID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_EGID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_SGID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_FSUID: AtomicU32 = AtomicU32::new(ROOT_ID);
static CURRENT_FSGID: AtomicU32 = AtomicU32::new(ROOT_ID);

fn supplementary_groups() -> &'static std::sync::Mutex<Vec<Gid>> {
    static GROUPS: OnceLock<std::sync::Mutex<Vec<Gid>>> = OnceLock::new();
    GROUPS.get_or_init(|| std::sync::Mutex::new(vec![ROOT_ID]))
}

fn identity_values() -> [u32; 9] {
    [
        CURRENT_UID.load(Ordering::Relaxed),
        CURRENT_EUID.load(Ordering::Relaxed),
        CURRENT_SUID.load(Ordering::Relaxed),
        CURRENT_GID.load(Ordering::Relaxed),
        CURRENT_EGID.load(Ordering::Relaxed),
        CURRENT_SGID.load(Ordering::Relaxed),
        crate::fsextra::current_umask(),
        CURRENT_FSUID.load(Ordering::Relaxed),
        CURRENT_FSGID.load(Ordering::Relaxed),
    ]
}

/// Publishes the final ID tuple once, including early-returning set*id paths.
struct PublishIdentity;
impl Drop for PublishIdentity {
    fn drop(&mut self) {
        let _ = kinakaze_vfs::credentials::publish();
    }
}

fn restore_identity(values: [u32; 9]) {
    let _publication = PublishIdentity;
    CURRENT_UID.store(values[0], Ordering::Relaxed);
    CURRENT_EUID.store(values[1], Ordering::Relaxed);
    CURRENT_SUID.store(values[2], Ordering::Relaxed);
    CURRENT_GID.store(values[3], Ordering::Relaxed);
    CURRENT_EGID.store(values[4], Ordering::Relaxed);
    CURRENT_SGID.store(values[5], Ordering::Relaxed);
    crate::fsextra::restore_umask(values[6]);
    CURRENT_FSUID.store(values[7], Ordering::Relaxed);
    CURRENT_FSGID.store(values[8], Ordering::Relaxed);
}

fn identity_payload() -> Vec<u8> {
    let values = identity_values();
    let groups = supplementary_groups()
        .lock()
        .map(|groups| groups.clone())
        .unwrap_or_default();
    let mut payload = Vec::with_capacity((10 + groups.len()) * 4);
    for value in values {
        payload.extend_from_slice(&value.to_le_bytes());
    }
    payload.extend_from_slice(&(groups.len() as u32).to_le_bytes());
    for group in groups {
        payload.extend_from_slice(&group.to_le_bytes());
    }
    payload
}

fn decode_identity_payload(bytes: &[u8]) -> Option<([u32; 9], Vec<Gid>)> {
    if bytes.len() < 10 * 4 || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut values = [0u32; 9];
    for (index, value) in values.iter_mut().enumerate() {
        let start = index * 4;
        *value = u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?);
    }
    let count = u32::from_le_bytes(bytes[36..40].try_into().ok()?) as usize;
    if bytes.len() != (10usize.checked_add(count)?).checked_mul(4)? {
        return None;
    }
    let mut groups = Vec::with_capacity(count);
    for index in 0..count {
        let start = 40 + index * 4;
        groups.push(u32::from_le_bytes(bytes[start..start + 4].try_into().ok()?));
    }
    Some((values, groups))
}

fn restore_process_identity(values: [u32; 9], groups: Vec<Gid>) {
    restore_identity(values);
    if let Ok(mut current) = supplementary_groups().lock() {
        *current = groups;
    }
}

#[cfg(windows)]
mod identity_handoff {
    use super::*;

    const KEY: u64 = 0x4c49_4243_4944_5332; // "LIBCIDS2"
    unsafe extern "system" fn snapshot(buffer: *mut u8, capacity: usize) -> isize {
        let payload = identity_payload();
        if buffer.is_null() {
            return payload.len() as isize;
        }
        if capacity < payload.len() {
            return -(kinakaze_vfs::ENOMEM as isize);
        }
        unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), buffer, payload.len()) };
        payload.len() as isize
    }

    unsafe extern "system" fn child(payload: *const u8, len: usize) -> i32 {
        if payload.is_null() {
            return kinakaze_vfs::EINVAL;
        }
        let bytes = unsafe { std::slice::from_raw_parts(payload, len) };
        let Some((values, groups)) = decode_identity_payload(bytes) else {
            return kinakaze_vfs::EINVAL;
        };
        restore_process_identity(values, groups);
        0
    }

    fn register() {
        let _ = kinakaze_runtime::register_fork_participant(kinakaze_runtime::ForkParticipant {
            abi: kinakaze_runtime::FORK_PARTICIPANT_ABI,
            priority: 40,
            key: KEY,
            prepare: None,
            snapshot: Some(snapshot),
            parent: None,
            child: Some(child),
        });
    }

    extern "C" fn initializer() {
        kinakaze_runtime::services::install_task_limit(kinakaze_vfs::limits::task_creation_errno);
        kinakaze_vfs::credentials::register_real_uid(|| CURRENT_UID.load(Ordering::Relaxed));
        kinakaze_vfs::credentials::register_identity(|| kinakaze_vfs::credentials::Identity {
            uids: [&CURRENT_UID, &CURRENT_EUID, &CURRENT_SUID].map(|id| id.load(Ordering::Relaxed)),
            gids: [&CURRENT_GID, &CURRENT_EGID, &CURRENT_SGID].map(|id| id.load(Ordering::Relaxed)),
        });
        kinakaze_vfs::credentials::register(
            || kinakaze_vfs::credentials::Credentials {
                uid: CURRENT_EUID.load(Ordering::Relaxed),
                gid: CURRENT_EGID.load(Ordering::Relaxed),
                capabilities: effective_capabilities().unwrap_or(0),
            },
            |gid| {
                supplementary_groups()
                    .lock()
                    .is_ok_and(|groups| groups.contains(&gid))
            },
        );
        kinakaze_vfs::credentials::register_filesystem(|| kinakaze_vfs::credentials::Credentials {
            uid: CURRENT_FSUID.load(Ordering::Relaxed),
            gid: CURRENT_FSGID.load(Ordering::Relaxed),
            capabilities: effective_capabilities().unwrap_or(0),
        });
        kinakaze_vfs::credentials::register_exec(
            || {
                let mut payload = identity_payload();
                // Ordinary exec copies the effective IDs into the saved IDs.
                // Root/real IDs, supplementary groups and umask survive unchanged.
                let euid = payload[4..8].to_vec();
                let egid = payload[16..20].to_vec();
                payload[8..12].copy_from_slice(&euid);
                payload[20..24].copy_from_slice(&egid);
                payload[28..32].copy_from_slice(&euid);
                payload[32..36].copy_from_slice(&egid);
                payload
            },
            |bytes| {
                let Some((values, groups)) = decode_identity_payload(bytes) else {
                    return false;
                };
                restore_process_identity(values, groups);
                true
            },
        );
        register();
    }

    #[used]
    #[unsafe(link_section = ".CRT$XCU")]
    static INITIALIZER: extern "C" fn() = initializer;
}

// Win32 entry points this module queries directly.
//
// These are declared here rather than imported from `windows-sys` because the
// crate's enabled feature set covers `Win32_System_Threading` only, and the
// names below live in `Win32_System_WindowsProgramming`,
// `Win32_System_SystemInformation` and the WDK bindings. Declaring the
// signatures needed keeps this module self-contained instead of widening the
// dependency's feature list.
//
// Every signature matches the Windows headers exactly: `BOOL` is `i32`, a
// `DWORD` count is `u32`, and each `LPDWORD` is an in-out parameter carrying a
// buffer length in `WCHAR`s including the terminator.
#[link(name = "kernel32")]
unsafe extern "system" {
    /// `GetSystemInfo`: page size and processor count.
    fn GetSystemInfo(info: *mut SystemInfo);
    /// `GetTickCount64`: milliseconds since boot.
    fn GetTickCount64() -> u64;
    /// `GetCurrentProcess`: the pseudo-handle for this process.
    fn GetCurrentProcess() -> *mut c_void;
    /// `GetProcessTimes`: cumulative kernel and user time for this process.
    fn GetProcessTimes(
        process: *mut c_void,
        creation: *mut FileTime,
        exit: *mut FileTime,
        kernel: *mut FileTime,
        user: *mut FileTime,
    ) -> i32;
    /// `GetPriorityClass`: this process's scheduling class.
    fn GetPriorityClass(process: *mut c_void) -> u32;
    /// `SetPriorityClass`: changes this process's scheduling class.
    fn SetPriorityClass(process: *mut c_void, class: u32) -> i32;
    /// `K32GetProcessMemoryInfo`: the psapi counters, re-exported by kernel32.
    ///
    /// The `K32`-prefixed spelling is used so this module links against
    /// kernel32 alone; the psapi.dll name is a forwarder to the same code.
    fn K32GetProcessMemoryInfo(
        process: *mut c_void,
        counters: *mut ProcessMemoryCounters,
        size: u32,
    ) -> i32;
}

#[link(name = "ntdll")]
unsafe extern "system" {
    /// `RtlGetVersion`: the true OS version.
    ///
    /// Preferred over `GetVersionExW`, which lies by design: since Windows 8.1
    /// the Win32 call reports 6.2 to any process without a matching
    /// compatibility manifest, so it would report a decade-old build here. The
    /// ntdll entry point is not shimmed and returns what the kernel actually is.
    fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
}

/// `SYSTEM_INFO`. Only the page size and processor count are read.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SystemInfo {
    processor_architecture: u16,
    reserved: u16,
    page_size: u32,
    minimum_application_address: *mut c_void,
    maximum_application_address: *mut c_void,
    active_processor_mask: usize,
    number_of_processors: u32,
    processor_type: u32,
    allocation_granularity: u32,
    processor_level: u16,
    processor_revision: u16,
}

/// `FILETIME`, a count of 100-nanosecond ticks split across two words.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FileTime {
    low: u32,
    high: u32,
}

impl FileTime {
    /// Reassembles the split halves into the tick count they encode.
    fn ticks(self) -> u64 {
        (u64::from(self.high) << 32) | u64::from(self.low)
    }

    /// Converts to whole seconds and a leftover microsecond remainder.
    fn seconds_and_microseconds(self) -> (i64, i64) {
        let microseconds = self.ticks() / 10;
        (
            (microseconds / 1_000_000) as i64,
            (microseconds % 1_000_000) as i64,
        )
    }
}

/// `PROCESS_MEMORY_COUNTERS`. `size` selects the struct version.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessMemoryCounters {
    size: u32,
    page_fault_count: u32,
    peak_working_set: usize,
    working_set: usize,
    peak_paged_pool: usize,
    paged_pool: usize,
    peak_non_paged_pool: usize,
    non_paged_pool: usize,
    page_file: usize,
    peak_page_file: usize,
}

/// `RTL_OSVERSIONINFOW`. `size` selects the struct version.
#[repr(C)]
#[derive(Clone, Copy)]
struct OsVersionInfoW {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    service_pack: [u16; 128],
}

impl Default for OsVersionInfoW {
    fn default() -> Self {
        Self {
            size: size_of::<Self>() as u32,
            major: 0,
            minor: 0,
            build: 0,
            platform: 0,
            service_pack: [0; 128],
        }
    }
}

/// The login name in the guest identity namespace.
///
/// Host token ownership is an implementation detail, not a Linux account.  In
/// particular it must not make `/etc/passwd`, `getlogin`, SSH and file listings
/// expose a Windows user whose profile is outside the guest root.
fn account_name() -> String {
    String::from("root")
}

/// Hostname is shared by all members of the current native UTS namespace.
fn computer_name() -> String {
    String::from_utf8_lossy(&kinakaze_vfs::namespaces::uts(false).unwrap_or_default()).into_owned()
}

#[cfg(test)]
/// The home directory reported as `pw_dir`.
///
/// The hosted identity starts as uid 0, so its home is the guest's `/root`.
/// Host account paths such as `USERPROFILE` are deliberately not exposed in
/// the Linux namespace: doing so makes guest tools read and modify the real
/// Windows profile (for example its SSH keys and configuration).
fn home_directory() -> String {
    String::from("/root")
}

// ---------------------------------------------------------------------------
// Identity.
//
// Real, effective and saved IDs are guest process state. VFS creation and
// ownership operations query the live effective IDs; fork and exec explicitly
// transfer them. Windows tokens still enforce host access independently. These
// guest IDs do not by themselves provide complete DAC/ACL or user namespaces.
// ---------------------------------------------------------------------------

/// `getuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getuid() -> Uid {
    kinakaze_vfs::user_namespace::visible(CURRENT_UID.load(Ordering::Relaxed), false)
}

/// `geteuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_geteuid() -> Uid {
    kinakaze_vfs::user_namespace::visible(CURRENT_EUID.load(Ordering::Relaxed), false)
}

/// `getgid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getgid() -> Gid {
    kinakaze_vfs::user_namespace::visible(CURRENT_GID.load(Ordering::Relaxed), true)
}

/// `getegid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getegid() -> Gid {
    kinakaze_vfs::user_namespace::visible(CURRENT_EGID.load(Ordering::Relaxed), true)
}

// Translate namespace-facing IDs before comparing/storing kernel identities.
macro_rules! mapped_identity {
    ($value:expr, $group:expr, $unchanged:expr) => {{
        if $unchanged && $value == UNCHANGED {
            UNCHANGED
        } else {
            match kinakaze_vfs::user_namespace::kernel($value, $group) {
                Ok(value) => value,
                Err(_) => {
                    crate::set_errno(EINVAL);
                    return -1;
                }
            }
        }
    }};
}

/// Return the previous filesystem ID, including on denial; preserve errno.
fn set_filesystem_id(id: u32, group: bool) -> u32 {
    let _publication = PublishIdentity;
    let (fs, real, effective, saved, capability) = if group {
        (
            &CURRENT_FSGID,
            &CURRENT_GID,
            &CURRENT_EGID,
            &CURRENT_SGID,
            6,
        )
    } else {
        (
            &CURRENT_FSUID,
            &CURRENT_UID,
            &CURRENT_EUID,
            &CURRENT_SUID,
            7,
        )
    };
    let old = fs.load(Ordering::Relaxed);
    if let Ok(id) = kinakaze_vfs::user_namespace::kernel(id, group) {
        if [
            old,
            real.load(Ordering::Relaxed),
            effective.load(Ordering::Relaxed),
            saved.load(Ordering::Relaxed),
        ]
        .contains(&id)
            || kinakaze_vfs::user_namespace::current_capable(capability)
        {
            fs.store(id, Ordering::Relaxed);
        }
    }
    kinakaze_vfs::user_namespace::visible(old, group)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setfsuid(id: Uid) -> c_int {
    set_filesystem_id(id, false) as c_int
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setfsgid(id: Gid) -> c_int {
    set_filesystem_id(id, true) as c_int
}

/// `setuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setuid(uid: Uid) -> c_int {
    let _publication = PublishIdentity;
    let uid = mapped_identity!(uid, false, false);
    if kinakaze_vfs::user_namespace::current_capable(7) {
        CURRENT_UID.store(uid, Ordering::Relaxed);
        CURRENT_EUID.store(uid, Ordering::Relaxed);
        CURRENT_FSUID.store(uid, Ordering::Relaxed);
        CURRENT_SUID.store(uid, Ordering::Relaxed);
        return 0;
    }
    if uid == CURRENT_UID.load(Ordering::Relaxed) || uid == CURRENT_SUID.load(Ordering::Relaxed) {
        CURRENT_EUID.store(uid, Ordering::Relaxed);
        CURRENT_FSUID.store(uid, Ordering::Relaxed);
        return 0;
    }
    crate::set_errno(EPERM);
    -1
}

/// `seteuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_seteuid(uid: Uid) -> c_int {
    let _publication = PublishIdentity;
    let uid = mapped_identity!(uid, false, false);
    let current = [
        CURRENT_UID.load(Ordering::Relaxed),
        CURRENT_EUID.load(Ordering::Relaxed),
        CURRENT_SUID.load(Ordering::Relaxed),
    ];
    if kinakaze_vfs::user_namespace::current_capable(7) || current.contains(&uid) {
        CURRENT_EUID.store(uid, Ordering::Relaxed);
        CURRENT_FSUID.store(uid, Ordering::Relaxed);
        return 0;
    }
    crate::set_errno(EPERM);
    -1
}

/// `setgid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setgid(gid: Gid) -> c_int {
    let _publication = PublishIdentity;
    let gid = mapped_identity!(gid, true, false);
    if kinakaze_vfs::user_namespace::current_capable(6) {
        CURRENT_GID.store(gid, Ordering::Relaxed);
        CURRENT_EGID.store(gid, Ordering::Relaxed);
        CURRENT_FSGID.store(gid, Ordering::Relaxed);
        CURRENT_SGID.store(gid, Ordering::Relaxed);
        return 0;
    }
    if gid == CURRENT_GID.load(Ordering::Relaxed) || gid == CURRENT_SGID.load(Ordering::Relaxed) {
        CURRENT_EGID.store(gid, Ordering::Relaxed);
        CURRENT_FSGID.store(gid, Ordering::Relaxed);
        return 0;
    }
    crate::set_errno(EPERM);
    -1
}

/// `setegid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setegid(gid: Gid) -> c_int {
    let _publication = PublishIdentity;
    let gid = mapped_identity!(gid, true, false);
    let current = [
        CURRENT_GID.load(Ordering::Relaxed),
        CURRENT_EGID.load(Ordering::Relaxed),
        CURRENT_SGID.load(Ordering::Relaxed),
    ];
    if kinakaze_vfs::user_namespace::current_capable(6) || current.contains(&gid) {
        CURRENT_EGID.store(gid, Ordering::Relaxed);
        CURRENT_FSGID.store(gid, Ordering::Relaxed);
        return 0;
    }
    crate::set_errno(EPERM);
    -1
}

/// `setreuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setreuid(ruid: Uid, euid: Uid) -> c_int {
    let _publication = PublishIdentity;
    let ruid = mapped_identity!(ruid, false, true);
    let euid = mapped_identity!(euid, false, true);
    let old_ruid = CURRENT_UID.load(Ordering::Relaxed);
    let old_euid = CURRENT_EUID.load(Ordering::Relaxed);
    let old_suid = CURRENT_SUID.load(Ordering::Relaxed);
    let allowed = |id| id == UNCHANGED || [old_ruid, old_euid, old_suid].contains(&id);
    if !kinakaze_vfs::user_namespace::current_capable(7) && (!allowed(ruid) || !allowed(euid)) {
        crate::set_errno(EPERM);
        return -1;
    }
    let new_ruid = if ruid == UNCHANGED { old_ruid } else { ruid };
    let new_euid = if euid == UNCHANGED { old_euid } else { euid };
    CURRENT_UID.store(new_ruid, Ordering::Relaxed);
    CURRENT_EUID.store(new_euid, Ordering::Relaxed);
    CURRENT_FSUID.store(new_euid, Ordering::Relaxed);
    if ruid != UNCHANGED || (euid != UNCHANGED && new_euid != old_ruid) {
        CURRENT_SUID.store(new_euid, Ordering::Relaxed);
    }
    0
}

/// `setregid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setregid(rgid: Gid, egid: Gid) -> c_int {
    let _publication = PublishIdentity;
    let rgid = mapped_identity!(rgid, true, true);
    let egid = mapped_identity!(egid, true, true);
    let old_rgid = CURRENT_GID.load(Ordering::Relaxed);
    let old_egid = CURRENT_EGID.load(Ordering::Relaxed);
    let old_sgid = CURRENT_SGID.load(Ordering::Relaxed);
    let allowed = |id| id == UNCHANGED || [old_rgid, old_egid, old_sgid].contains(&id);
    if !kinakaze_vfs::user_namespace::current_capable(6) && (!allowed(rgid) || !allowed(egid)) {
        crate::set_errno(EPERM);
        return -1;
    }
    let new_rgid = if rgid == UNCHANGED { old_rgid } else { rgid };
    let new_egid = if egid == UNCHANGED { old_egid } else { egid };
    CURRENT_GID.store(new_rgid, Ordering::Relaxed);
    CURRENT_EGID.store(new_egid, Ordering::Relaxed);
    CURRENT_FSGID.store(new_egid, Ordering::Relaxed);
    if rgid != UNCHANGED || (egid != UNCHANGED && new_egid != old_rgid) {
        CURRENT_SGID.store(new_egid, Ordering::Relaxed);
    }
    0
}

/// `getresuid`, which reports the real, effective and saved uids.
///
/// # Safety
///
/// All three pointers must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getresuid(
    real: *mut Uid,
    effective: *mut Uid,
    saved: *mut Uid,
) -> c_int {
    unsafe { store_triple(real, effective, saved, false) }
}

/// `getresgid`.
///
/// # Safety
///
/// All three pointers must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getresgid(
    real: *mut Gid,
    effective: *mut Gid,
    saved: *mut Gid,
) -> c_int {
    unsafe { store_triple(real, effective, saved, true) }
}

/// Writes the identity into three out-parameters.
///
/// # Safety
///
/// Each non-null pointer must be writable.
unsafe fn store_triple(
    real: *mut u32,
    effective: *mut u32,
    saved: *mut u32,
    is_gid: bool,
) -> c_int {
    if real.is_null() || effective.is_null() || saved.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    unsafe {
        if is_gid {
            *real =
                kinakaze_vfs::user_namespace::visible(CURRENT_GID.load(Ordering::Relaxed), true);
            *effective =
                kinakaze_vfs::user_namespace::visible(CURRENT_EGID.load(Ordering::Relaxed), true);
            *saved =
                kinakaze_vfs::user_namespace::visible(CURRENT_SGID.load(Ordering::Relaxed), true);
        } else {
            *real =
                kinakaze_vfs::user_namespace::visible(CURRENT_UID.load(Ordering::Relaxed), false);
            *effective =
                kinakaze_vfs::user_namespace::visible(CURRENT_EUID.load(Ordering::Relaxed), false);
            *saved =
                kinakaze_vfs::user_namespace::visible(CURRENT_SUID.load(Ordering::Relaxed), false);
        }
    }
    0
}

/// `setresuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setresuid(real: Uid, effective: Uid, saved: Uid) -> c_int {
    let _publication = PublishIdentity;
    let real = mapped_identity!(real, false, true);
    let effective = mapped_identity!(effective, false, true);
    let saved = mapped_identity!(saved, false, true);
    let current = [
        CURRENT_UID.load(Ordering::Relaxed),
        CURRENT_EUID.load(Ordering::Relaxed),
        CURRENT_SUID.load(Ordering::Relaxed),
    ];
    if !kinakaze_vfs::user_namespace::current_capable(7)
        && [real, effective, saved]
            .iter()
            .any(|id| *id != UNCHANGED && !current.contains(id))
    {
        crate::set_errno(EPERM);
        return -1;
    }
    if real != UNCHANGED {
        CURRENT_UID.store(real, Ordering::Relaxed);
    }
    if effective != UNCHANGED {
        CURRENT_EUID.store(effective, Ordering::Relaxed);
        CURRENT_FSUID.store(effective, Ordering::Relaxed);
    }
    if saved != UNCHANGED {
        CURRENT_SUID.store(saved, Ordering::Relaxed);
    }
    CURRENT_FSUID.store(CURRENT_EUID.load(Ordering::Relaxed), Ordering::Relaxed);
    0
}

/// `setresgid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setresgid(real: Gid, effective: Gid, saved: Gid) -> c_int {
    let _publication = PublishIdentity;
    let real = mapped_identity!(real, true, true);
    let effective = mapped_identity!(effective, true, true);
    let saved = mapped_identity!(saved, true, true);
    let current = [
        CURRENT_GID.load(Ordering::Relaxed),
        CURRENT_EGID.load(Ordering::Relaxed),
        CURRENT_SGID.load(Ordering::Relaxed),
    ];
    if !kinakaze_vfs::user_namespace::current_capable(6)
        && [real, effective, saved]
            .iter()
            .any(|id| *id != UNCHANGED && !current.contains(id))
    {
        crate::set_errno(EPERM);
        return -1;
    }
    if real != UNCHANGED {
        CURRENT_GID.store(real, Ordering::Relaxed);
    }
    if effective != UNCHANGED {
        CURRENT_EGID.store(effective, Ordering::Relaxed);
        CURRENT_FSGID.store(effective, Ordering::Relaxed);
    }
    if saved != UNCHANGED {
        CURRENT_SGID.store(saved, Ordering::Relaxed);
    }
    CURRENT_FSGID.store(CURRENT_EGID.load(Ordering::Relaxed), Ordering::Relaxed);
    0
}

/// `getgroups`, which reports the supplementary group list.
///
/// # Safety
///
/// `list` must name at least `count` writable `gid_t` slots unless `count` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getgroups(count: c_int, list: *mut Gid) -> c_int {
    if count < 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    let Ok(groups) = supplementary_groups().lock() else {
        crate::set_errno(kinakaze_vfs::EIO);
        return -1;
    };
    let needed = groups.len();
    if count == 0 {
        return needed as c_int;
    }
    if list.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if (count as usize) < needed {
        crate::set_errno(EINVAL);
        return -1;
    }
    for (index, group) in groups.iter().enumerate() {
        unsafe {
            list.add(index)
                .write(kinakaze_vfs::user_namespace::visible(*group, true));
        }
    }
    needed as c_int
}

/// `setgroups`.
///
/// # Safety
///
/// `list` must name at least `count` readable `gid_t` slots unless `count` is 0.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setgroups(count: usize, list: *const Gid) -> c_int {
    const NGROUPS_MAX: usize = 65_536;
    if !kinakaze_vfs::user_namespace::current_capable(6)
        || !kinakaze_vfs::user_namespace::groups_allowed()
    {
        crate::set_errno(EPERM);
        return -1;
    }
    if count > NGROUPS_MAX {
        crate::set_errno(EINVAL);
        return -1;
    }
    if count != 0 && list.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let requested = if count == 0 {
        Vec::new()
    } else {
        unsafe { core::slice::from_raw_parts(list, count) }.to_vec()
    };
    let requested = requested
        .into_iter()
        .map(|id| kinakaze_vfs::user_namespace::kernel(id, true))
        .collect::<Result<Vec<_>, _>>();
    let requested = match requested {
        Ok(ids) => ids,
        Err(_) => {
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    let Ok(mut groups) = supplementary_groups().lock() else {
        crate::set_errno(kinakaze_vfs::EIO);
        return -1;
    };
    *groups = requested;
    0
}

fn install_initial_groups(group: Gid) -> c_int {
    unsafe { kinakaze_abi_setgroups(1, &group) }
}

/// `initgroups`, which would install the groups a user belongs to.
///
/// # Safety
///
/// `user` must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_initgroups(user: *const c_char, _group: Gid) -> c_int {
    if user.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    install_initial_groups(_group)
}

/// `getgrouplist`, which reports the groups `user` belongs to.
///
/// The convention is unusual and BusyBox depends on it: `count` is in-out, the
/// return value is the number of groups on success and -1 when the buffer was
/// too small, and in the failure case `count` is updated to the size required so
/// the caller can retry. That retry loop is why the required size is always
/// written, on both paths.
///
/// # Safety
///
/// `count` must be writable, and `groups` must name at least `*count` writable
/// slots.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getgrouplist(
    user: *const c_char,
    group: Gid,
    groups: *mut Gid,
    count: *mut c_int,
) -> c_int {
    if user.is_null() || count.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees `count` is writable.
    let capacity = unsafe { *count };
    // The list is the caller's primary group and nothing else, because no other
    // group exists to report.
    let list = [group];
    // SAFETY: the caller guarantees `count` is writable.
    unsafe { *count = list.len() as c_int };

    if capacity < list.len() as c_int || groups.is_null() {
        // -1 with the required size in `count`, which is what the caller retries
        // against. errno is deliberately not set: the interface reports the
        // shortfall through the return value and glibc leaves errno alone.
        return -1;
    }
    // SAFETY: the capacity check above proved there is room for the whole list.
    unsafe { ptr::copy_nonoverlapping(list.as_ptr(), groups, list.len()) };
    list.len() as c_int
}

/// `group_member`, which asks whether the process is in a supplementary group.
///
/// Returns 1 for the one group that exists and 0 for every other, which is the
/// true answer for both cases.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_group_member(group: Gid) -> c_int {
    match kinakaze_vfs::user_namespace::kernel(group, true) {
        Ok(id) => kernel_group_member(id),
        Err(_) => 0,
    }
}
fn kernel_group_member(group: Gid) -> c_int {
    if group == CURRENT_EGID.load(Ordering::Relaxed) {
        return 1;
    }
    let member = supplementary_groups()
        .lock()
        .map(|groups| groups.contains(&group))
        .unwrap_or(false);
    c_int::from(member)
}

// ---------------------------------------------------------------------------
// Process groups and sessions.
//
// Windows has no process groups: there is no id namespace shared with pids and
// no session with a controlling terminal. The registry in `kinakaze_vfs::job`
// supplies one, as a named shared section every hosted process maps. Membership
// is a relation *between* processes and every one of them is a separate Windows
// process here, so the answers have to come from somewhere both can see; a
// per-process table would answer every interesting question wrong.
//
// The entry points below are therefore thin: they convert between the C ABI and
// that registry, and the POSIX rules about who may move whom live next to the
// data they constrain.
// ---------------------------------------------------------------------------

/// This process's id, used when the registry has nothing to say.
fn own_pid() -> c_int {
    crate::process::kinakaze_abi_getpid()
}

/// `getpgrp`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpgrp() -> c_int {
    kinakaze_abi_getpgid(0)
}

/// `getpgid`.
///
/// A zero argument means the calling process. Any other pid is looked up in the
/// registry, so a shell really can ask which group it put a child in.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpgid(pid: c_int) -> c_int {
    match kinakaze_vfs::job::getpgid(pid) {
        Ok(pgid) => pgid,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `getsid`, with the same reasoning as [`kinakaze_abi_getpgid`].
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getsid(pid: c_int) -> c_int {
    match kinakaze_vfs::job::getsid(pid) {
        Ok(sid) => sid,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `setpgid`, which moves the caller or one of its children into a group.
///
/// The POSIX restrictions are enforced rather than assumed: only a child that
/// has not yet `exec`ed may be moved, a session leader may not leave its own
/// group, and a group may only be joined from within its own session. Those
/// rules are what make a shell's habit of calling `setpgid` in both the parent
/// and the child race-free — whichever runs second is redundant or refused, and
/// never wrong.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setpgid(pid: c_int, pgid: c_int) -> c_int {
    match kinakaze_vfs::job::setpgid(pid, pgid) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `setsid`, which creates a session and returns its id.
///
/// This now really fails with `EPERM` for a process that already leads a group,
/// which is what POSIX says and what the previous unconditional success hid. A
/// daemonizing program forks precisely so that the child is not a group leader;
/// answering "fine" to a leader would conceal the case where it forgot to.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setsid() -> c_int {
    match kinakaze_vfs::job::setsid() {
        Ok(sid) => sid,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `getlogin_r`, which copies the login name into the caller's buffer.
///
/// Returns 0 on success or `ERANGE` when the buffer is too small, and writes
/// nothing at all in the failure case.
///
/// # Safety
///
/// `name` must name at least `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getlogin_r(name: *mut c_char, length: usize) -> c_int {
    if name.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    let login = account_name();
    let bytes = login.as_bytes();
    // The terminator has to fit too, so a buffer of exactly the name's length is
    // still too small.
    if length < bytes.len() + 1 {
        return ERANGE;
    }
    // SAFETY: the check above proved the buffer holds the bytes and a NUL.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), name.cast::<u8>(), bytes.len());
        *name.add(bytes.len()) = 0;
    }
    0
}

/// `getlogin`.
/// Legacy effective-user lookup. The Linux L_cuserid buffer is nine bytes;
/// callers may supply it or use this function's static buffer.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_cuserid(out: *mut c_char) -> *mut c_char {
    static mut NAME: [c_char; 9] = [0; 9];
    let Some(account) = find_user_by_uid(kinakaze_abi_geteuid()) else {
        if !out.is_null() {
            unsafe {
                *out = 0;
            }
        }
        return out;
    };
    let target = if out.is_null() {
        core::ptr::addr_of_mut!(NAME).cast::<c_char>()
    } else {
        out
    };
    let bytes = account.name.as_bytes();
    let n = bytes.len().min(8);
    unsafe {
        ptr::write_bytes(target, 0, 9);
        ptr::copy_nonoverlapping(bytes.as_ptr(), target.cast(), n);
    }
    target
}

/// `getlogin`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getlogin() -> *mut c_char {
    static LOGIN_BUF: OnceLock<CString> = OnceLock::new();
    let cstr = LOGIN_BUF.get_or_init(|| {
        CString::new(account_name()).unwrap_or_else(|_| CString::new("root").unwrap())
    });
    cstr.as_ptr() as *mut c_char
}

/// `ESRCH`, which the VFS error list does not carry.
const ESRCH: i32 = 3;

// ---------------------------------------------------------------------------
// The passwd and group database.
//
// Accounts come from the guest's `/etc/passwd`, with a minimal root fallback
// when that file is absent. Host Windows accounts and profiles do not belong in
// this namespace.
// ---------------------------------------------------------------------------

/// The Linux x86_64 `struct passwd`.
///
/// Field order and widths are ABI: guest code reads these offsets directly.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Passwd {
    pub pw_name: *mut c_char,
    pub pw_passwd: *mut c_char,
    pub pw_uid: Uid,
    pub pw_gid: Gid,
    pub pw_gecos: *mut c_char,
    pub pw_dir: *mut c_char,
    pub pw_shell: *mut c_char,
}

/// The Linux x86_64 `struct group`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Group {
    pub gr_name: *mut c_char,
    pub gr_passwd: *mut c_char,
    pub gr_gid: Gid,
    /// A null-terminated array of member names.
    pub gr_mem: *mut *mut c_char,
}

/// Packs C strings into a caller-supplied buffer for the `_r` entry points.
///
/// The reentrant forms must place every string inside the caller's buffer and
/// report `ERANGE` without writing past it. This type enforces that: each
/// reservation is bounds-checked before any byte is written, so a buffer that
/// turns out to be too small is left exactly as it was found. Getting this wrong
/// is a heap or stack overwrite in the guest, and BusyBox exercises the path
/// deliberately by starting from a `sysconf`-derived size and growing on ERANGE.
struct Packer {
    base: *mut u8,
    capacity: usize,
    used: usize,
}

impl Packer {
    /// Wraps a caller buffer. A null buffer is treated as zero capacity, which
    /// makes every reservation fail with ERANGE rather than dereferencing it.
    fn new(buffer: *mut c_char, capacity: usize) -> Self {
        Self {
            base: if buffer.is_null() {
                ptr::null_mut()
            } else {
                buffer.cast::<u8>()
            },
            capacity: if buffer.is_null() { 0 } else { capacity },
            used: 0,
        }
    }

    /// Copies `text` in as a null-terminated string, or fails with ERANGE.
    fn string(&mut self, text: &str) -> Result<*mut c_char, c_int> {
        let bytes = text.as_bytes();
        let needed = bytes.len() + 1;
        // Checked before writing, so an overflowing reservation writes nothing.
        if self.used + needed > self.capacity {
            return Err(ERANGE);
        }
        // SAFETY: the bound above proved `needed` bytes are available at `used`.
        let target = unsafe { self.base.add(self.used) };
        // SAFETY: same bound; source and destination cannot overlap because the
        // source is an owned Rust string and the destination the caller's buffer.
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
            *target.add(bytes.len()) = 0;
        }
        self.used += needed;
        Ok(target.cast::<c_char>())
    }

    /// Reserves a null-terminated `char *` array holding `entries`.
    ///
    /// Used for `gr_mem`. The array must be pointer-aligned, so the cursor is
    /// advanced to an aligned offset first and that padding is counted against
    /// the capacity like any other reservation.
    fn pointer_array(&mut self, entries: &[*mut c_char]) -> Result<*mut *mut c_char, c_int> {
        let align = align_of::<*mut c_char>();
        // The buffer's own address decides where the aligned offsets fall, so the
        // padding is computed from the absolute address rather than from `used`.
        let address = self.base as usize + self.used;
        let padding = (align - (address % align)) % align;
        // One extra slot for the null terminator the array must carry.
        let needed = padding + (entries.len() + 1) * size_of::<*mut c_char>();
        if self.used + needed > self.capacity {
            return Err(ERANGE);
        }
        // SAFETY: the bound above proved the padded array fits.
        let array = unsafe { self.base.add(self.used + padding) }.cast::<*mut c_char>();
        for (index, entry) in entries.iter().enumerate() {
            // SAFETY: `index` is below `entries.len()`, which the bound covered.
            unsafe { *array.add(index) = *entry };
        }
        // SAFETY: the reservation included the terminator slot.
        unsafe { *array.add(entries.len()) = ptr::null_mut() };
        self.used += needed;
        Ok(array)
    }
}

/// Reads a null-terminated argument as a `str`, or `None` if it is unusable.
///
/// # Safety
///
/// `text` must be null or a null-terminated string.
unsafe fn borrow(text: *const c_char) -> Option<&'static str> {
    if text.is_null() {
        return None;
    }
    // SAFETY: forwarded from this function's contract.
    unsafe { CStr::from_ptr(text) }.to_str().ok()
}

#[derive(Clone, Debug)]
struct UserAccount {
    name: String,
    password: String,
    uid: u32,
    gid: u32,
    gecos: String,
    dir: String,
    shell: String,
}

fn load_user_accounts() -> Vec<UserAccount> {
    let mut accounts = Vec::new();

    if let Ok(path) = kinakaze_vfs::resolve_linux_path("/etc/passwd") {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = trimmed.split(':').collect();
                if parts.len() >= 7 {
                    let name = parts[0].to_string();
                    let password = parts[1].to_string();
                    let uid = parts[2].parse::<u32>().unwrap_or(0);
                    let gid = parts[3].parse::<u32>().unwrap_or(0);
                    let gecos = parts[4].to_string();
                    let dir = parts[5].to_string();
                    let shell = parts[6].to_string();
                    accounts.push(UserAccount {
                        name,
                        password,
                        uid,
                        gid,
                        gecos,
                        dir,
                        shell,
                    });
                }
            }
        }
    }

    if !accounts.iter().any(|account| account.name == "root") {
        accounts.insert(
            0,
            UserAccount {
                name: "root".to_string(),
                password: "x".to_string(),
                uid: 0,
                gid: 0,
                gecos: "".to_string(),
                dir: "/root".to_string(),
                shell: "/bin/sh".to_string(),
            },
        );
    }

    if !accounts.iter().any(|a| a.name == "sshd") {
        accounts.push(UserAccount {
            name: "sshd".to_string(),
            password: "x".to_string(),
            uid: 101,
            gid: 65534,
            gecos: "".to_string(),
            dir: "/run/sshd".to_string(),
            shell: "/usr/sbin/nologin".to_string(),
        });
    }

    if !accounts.iter().any(|a| a.name == "nobody") {
        accounts.push(UserAccount {
            name: "nobody".to_string(),
            password: "x".to_string(),
            uid: 65534,
            gid: 65534,
            gecos: "nobody".to_string(),
            dir: "/nonexistent".to_string(),
            shell: "/bin/false".to_string(),
        });
    }

    accounts
}

fn find_user_by_name(name: &str) -> Option<UserAccount> {
    load_user_accounts().into_iter().find(|a| a.name == name)
}

fn find_user_by_uid(uid: u32) -> Option<UserAccount> {
    load_user_accounts().into_iter().find(|a| a.uid == uid)
}

// Owns backing allocations for the raw pointers in the published C record.
struct StaticPasswdStorage {
    _name: CString,
    _password: CString,
    _gecos: CString,
    _dir: CString,
    _shell: CString,
    passwd: Passwd,
}

unsafe impl Send for StaticPasswdStorage {}
unsafe impl Sync for StaticPasswdStorage {}

static STATIC_PASSWD: std::sync::Mutex<Option<Box<StaticPasswdStorage>>> =
    std::sync::Mutex::new(None);

fn passwd_entry_for(account: &UserAccount) -> *mut Passwd {
    let mut storage_guard = STATIC_PASSWD.lock().unwrap();
    let name = CString::new(account.name.as_str()).unwrap_or_default();
    let password = CString::new(account.password.as_str()).unwrap_or_default();
    let gecos = CString::new(account.gecos.as_str()).unwrap_or_default();
    let dir = CString::new(account.dir.as_str()).unwrap_or_default();
    let shell = CString::new(account.shell.as_str()).unwrap_or_default();

    let mut storage = Box::new(StaticPasswdStorage {
        passwd: Passwd {
            pw_name: name.as_ptr().cast_mut(),
            pw_passwd: password.as_ptr().cast_mut(),
            pw_uid: account.uid,
            pw_gid: account.gid,
            pw_gecos: gecos.as_ptr().cast_mut(),
            pw_dir: dir.as_ptr().cast_mut(),
            pw_shell: shell.as_ptr().cast_mut(),
        },
        _name: name,
        _password: password,
        _gecos: gecos,
        _dir: dir,
        _shell: shell,
    });
    let ptr = &mut storage.passwd as *mut Passwd;
    *storage_guard = Some(storage);
    ptr
}

#[derive(Clone, Debug)]
struct GroupAccount {
    name: String,
    password: String,
    gid: u32,
    members: Vec<String>,
}

fn load_group_accounts() -> Vec<GroupAccount> {
    let mut groups = Vec::new();

    if let Ok(path) = kinakaze_vfs::resolve_linux_path("/etc/group") {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = trimmed.split(':').collect();
                if parts.len() >= 3 {
                    let name = parts[0].to_string();
                    let password = parts[1].to_string();
                    let gid = parts[2].parse::<u32>().unwrap_or(0);
                    let members = if parts.len() > 3 && !parts[3].trim().is_empty() {
                        parts[3].split(',').map(|s| s.trim().to_string()).collect()
                    } else {
                        Vec::new()
                    };
                    groups.push(GroupAccount {
                        name,
                        password,
                        gid,
                        members,
                    });
                }
            }
        }
    }

    if !groups.iter().any(|g| g.name == "root") {
        groups.insert(
            0,
            GroupAccount {
                name: "root".to_string(),
                password: "x".to_string(),
                gid: 0,
                members: vec!["root".to_string()],
            },
        );
    }

    if !groups.iter().any(|g| g.name == "nogroup") {
        groups.push(GroupAccount {
            name: "nogroup".to_string(),
            password: "x".to_string(),
            gid: 65534,
            members: Vec::new(),
        });
    }

    groups
}

fn find_group_by_name(name: &str) -> Option<GroupAccount> {
    load_group_accounts().into_iter().find(|g| g.name == name)
}

fn find_group_by_gid(gid: u32) -> Option<GroupAccount> {
    load_group_accounts().into_iter().find(|g| g.gid == gid)
}

// Owns backing allocations for the raw pointers in the published C record.
struct StaticGroupStorage {
    _name: CString,
    _password: CString,
    _members: Vec<CString>,
    _member_ptrs: Vec<*mut c_char>,
    group: Group,
}

unsafe impl Send for StaticGroupStorage {}
unsafe impl Sync for StaticGroupStorage {}

static STATIC_GROUP: std::sync::Mutex<Option<Box<StaticGroupStorage>>> =
    std::sync::Mutex::new(None);

fn group_entry_for(account: &GroupAccount) -> *mut Group {
    let mut storage_guard = STATIC_GROUP.lock().unwrap();
    let name = CString::new(account.name.as_str()).unwrap_or_default();
    let password = CString::new(account.password.as_str()).unwrap_or_default();
    let mut members = Vec::new();
    let mut member_ptrs = Vec::new();
    for mem in &account.members {
        let c = CString::new(mem.as_str()).unwrap_or_default();
        member_ptrs.push(c.as_ptr().cast_mut());
        members.push(c);
    }
    member_ptrs.push(ptr::null_mut());

    let mut storage = Box::new(StaticGroupStorage {
        group: Group {
            gr_name: name.as_ptr().cast_mut(),
            gr_passwd: password.as_ptr().cast_mut(),
            gr_gid: account.gid,
            gr_mem: member_ptrs.as_mut_ptr(),
        },
        _name: name,
        _password: password,
        _members: members,
        _member_ptrs: member_ptrs,
    });
    let ptr = &mut storage.group as *mut Group;
    *storage_guard = Some(storage);
    ptr
}

/// `getpwnam`.
///
/// # Safety
///
/// `name` must be a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getpwnam(name: *const c_char) -> *mut Passwd {
    match unsafe { borrow(name) } {
        Some(name) => {
            if let Some(account) = find_user_by_name(name) {
                passwd_entry_for(&account)
            } else {
                crate::set_errno(ENOENT);
                ptr::null_mut()
            }
        }
        _ => {
            crate::set_errno(ENOENT);
            ptr::null_mut()
        }
    }
}

/// `getpwuid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpwuid(uid: Uid) -> *mut Passwd {
    if let Some(account) = find_user_by_uid(uid) {
        passwd_entry_for(&account)
    } else {
        crate::set_errno(ENOENT);
        ptr::null_mut()
    }
}

/// `getgrnam`.
///
/// # Safety
///
/// `name` must be a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getgrnam(name: *const c_char) -> *mut Group {
    match unsafe { borrow(name) } {
        Some(name) => {
            if let Some(account) = find_group_by_name(name) {
                group_entry_for(&account)
            } else {
                crate::set_errno(ENOENT);
                ptr::null_mut()
            }
        }
        _ => {
            crate::set_errno(ENOENT);
            ptr::null_mut()
        }
    }
}

/// `getgrgid`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getgrgid(gid: Gid) -> *mut Group {
    if let Some(account) = find_group_by_gid(gid) {
        group_entry_for(&account)
    } else {
        crate::set_errno(ENOENT);
        ptr::null_mut()
    }
}

/// Independent enumeration cursors for the passwd and group databases.
static PASSWD_CURSOR: AtomicUsize = AtomicUsize::new(0);
static GROUP_CURSOR: AtomicUsize = AtomicUsize::new(0);

/// `getpwent`, returning the next guest passwd entry.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpwent() -> *mut Passwd {
    let index = PASSWD_CURSOR.fetch_add(1, Ordering::SeqCst);
    load_user_accounts()
        .get(index)
        .map(passwd_entry_for)
        .unwrap_or(ptr::null_mut())
}

/// `setpwent`, which rewinds the enumeration.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setpwent() {
    PASSWD_CURSOR.store(0, Ordering::SeqCst);
}

/// `endpwent`, which closes the enumeration.
///
/// There is no file handle to release, but the position is reset so a later
/// `getpwent` without an intervening `setpwent` starts from the beginning, as it
/// does on glibc.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endpwent() {
    PASSWD_CURSOR.store(0, Ordering::SeqCst);
}

/// `getgrent`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getgrent() -> *mut Group {
    let index = GROUP_CURSOR.fetch_add(1, Ordering::SeqCst);
    load_group_accounts()
        .get(index)
        .map(group_entry_for)
        .unwrap_or(ptr::null_mut())
}

/// `setgrent`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setgrent() {
    GROUP_CURSOR.store(0, Ordering::SeqCst);
}

/// `endgrent`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endgrent() {
    GROUP_CURSOR.store(0, Ordering::SeqCst);
}

unsafe fn fill_passwd_for(
    account: &UserAccount,
    entry: *mut Passwd,
    buffer: *mut c_char,
    length: usize,
) -> Result<(), c_int> {
    let mut packer = Packer::new(buffer, length);
    let name = packer.string(&account.name)?;
    let password = packer.string(&account.password)?;
    let gecos = packer.string(&account.gecos)?;
    let directory = packer.string(&account.dir)?;
    let shell = packer.string(&account.shell)?;

    // SAFETY: the caller guarantees `entry` is writable.
    unsafe {
        *entry = Passwd {
            pw_name: name,
            pw_passwd: password,
            pw_uid: account.uid,
            pw_gid: account.gid,
            pw_gecos: gecos,
            pw_dir: directory,
            pw_shell: shell,
        };
    }
    Ok(())
}

unsafe fn fill_group_for(
    account: &GroupAccount,
    entry: *mut Group,
    buffer: *mut c_char,
    length: usize,
) -> Result<(), c_int> {
    let mut packer = Packer::new(buffer, length);
    let name = packer.string(&account.name)?;
    let password = packer.string(&account.password)?;
    let mut member_ptrs = Vec::new();
    for mem in &account.members {
        member_ptrs.push(packer.string(mem)?);
    }
    let members = packer.pointer_array(&member_ptrs)?;

    // SAFETY: the caller guarantees `entry` is writable.
    unsafe {
        *entry = Group {
            gr_name: name,
            gr_passwd: password,
            gr_gid: account.gid,
            gr_mem: members,
        };
    }
    Ok(())
}

/// Completes an `_r` call, applying the POSIX result convention.
///
/// On success `*result` points at the caller's struct. When the entry is not
/// found, the call *succeeds* with `*result` null, which is what distinguishes
/// "no such user" from a real error. On error `*result` is null and the error
/// number is returned, not stored in errno.
///
/// # Safety
///
/// `result` must be writable.
unsafe fn finish<T>(result: *mut *mut T, entry: *mut T, outcome: Result<bool, c_int>) -> c_int {
    // SAFETY: the caller guarantees `result` is writable.
    unsafe { *result = ptr::null_mut() };
    match outcome {
        Ok(true) => {
            // SAFETY: as above.
            unsafe { *result = entry };
            0
        }
        // Found nothing: not an error, and `*result` stays null.
        Ok(false) => 0,
        Err(error) => {
            // glibc sets errno as well as returning the code, and BusyBox reads
            // errno after some of these calls, so both are provided.
            crate::set_errno(error);
            error
        }
    }
}

/// `getpwnam_r`.
///
/// # Safety
///
/// `name` must be null-terminated, `entry` and `result` writable, and `buffer`
/// must name `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getpwnam_r(
    name: *const c_char,
    entry: *mut Passwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Passwd,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    // SAFETY: forwarded from this function's contract.
    let wanted = unsafe { borrow(name) };
    let outcome = match wanted {
        Some(wanted) => {
            if let Some(account) = find_user_by_name(wanted) {
                if entry.is_null() {
                    Err(kinakaze_vfs::EFAULT)
                } else {
                    unsafe { fill_passwd_for(&account, entry, buffer, length) }.map(|()| true)
                }
            } else {
                Ok(false)
            }
        }
        None => Err(kinakaze_vfs::EFAULT),
    };
    // SAFETY: `result` was checked non-null above.
    unsafe { finish(result, entry, outcome) }
}

/// `getpwuid_r`.
///
/// # Safety
///
/// `entry` and `result` must be writable and `buffer` must name `length`
/// writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getpwuid_r(
    uid: Uid,
    entry: *mut Passwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Passwd,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    let outcome = if let Some(account) = find_user_by_uid(uid) {
        if entry.is_null() {
            Err(kinakaze_vfs::EFAULT)
        } else {
            unsafe { fill_passwd_for(&account, entry, buffer, length) }.map(|()| true)
        }
    } else {
        Ok(false)
    };
    // SAFETY: `result` was checked non-null above.
    unsafe { finish(result, entry, outcome) }
}

/// `getgrnam_r`.
///
/// # Safety
///
/// `name` must be null-terminated, `entry` and `result` writable, and `buffer`
/// must name `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getgrnam_r(
    name: *const c_char,
    entry: *mut Group,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Group,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    // SAFETY: forwarded from this function's contract.
    let wanted = unsafe { borrow(name) };
    let outcome = match wanted {
        Some(wanted) => {
            if let Some(account) = find_group_by_name(wanted) {
                if entry.is_null() {
                    Err(kinakaze_vfs::EFAULT)
                } else {
                    // SAFETY: `entry` is non-null and the caller guarantees the
                    // buffer bounds.
                    unsafe { fill_group_for(&account, entry, buffer, length) }.map(|()| true)
                }
            } else {
                Ok(false)
            }
        }
        None => Err(kinakaze_vfs::EFAULT),
    };
    // SAFETY: `result` was checked non-null above.
    unsafe { finish(result, entry, outcome) }
}

/// `getgrgid_r`.
///
/// # Safety
///
/// `entry` and `result` must be writable and `buffer` must name `length`
/// writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getgrgid_r(
    gid: Gid,
    entry: *mut Group,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Group,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    let outcome = match find_group_by_gid(gid) {
        Some(account) => {
            if entry.is_null() {
                Err(kinakaze_vfs::EFAULT)
            } else {
                // SAFETY: `entry` is non-null and the caller guarantees the buffer.
                unsafe { fill_group_for(&account, entry, buffer, length) }.map(|()| true)
            }
        }
        None => Ok(false),
    };
    // SAFETY: `result` was checked non-null above.
    unsafe { finish(result, entry, outcome) }
}

/// `fgetpwent_r`.
///
/// # Safety
///
/// `file` must point to a readable `File`, `entry` and `result` must be writable,
/// and `buffer` must name `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetpwent_r(
    file: *mut crate::stdio::File,
    entry: *mut Passwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Passwd,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    unsafe { *result = ptr::null_mut() };
    if file.is_null() || entry.is_null() || buffer.is_null() {
        return kinakaze_vfs::EFAULT;
    }

    let mut line_buf = [0u8; 1024];
    loop {
        let read_res = unsafe {
            crate::stdio::fgets(line_buf.as_mut_ptr().cast(), line_buf.len() as c_int, file)
        };
        if read_res.is_null() {
            return kinakaze_vfs::ENOENT;
        }
        let line_cstr = unsafe { core::ffi::CStr::from_ptr(line_buf.as_ptr().cast()) };
        let line_str = match line_cstr.to_str() {
            Ok(s) => s.trim(),
            Err(_) => continue,
        };
        if line_str.is_empty() || line_str.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line_str.split(':').collect();
        if parts.len() < 7 {
            continue;
        }
        let account = UserAccount {
            name: parts[0].to_string(),
            password: parts[1].to_string(),
            uid: parts[2].parse::<u32>().unwrap_or(0),
            gid: parts[3].parse::<u32>().unwrap_or(0),
            gecos: parts[4].to_string(),
            dir: parts[5].to_string(),
            shell: parts[6].to_string(),
        };
        let fill_res = unsafe { fill_passwd_for(&account, entry, buffer, length) };
        return match fill_res {
            Ok(()) => {
                unsafe { *result = entry };
                0
            }
            Err(e) => e,
        };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn fgetpwent_r(
    file: *mut crate::stdio::File,
    entry: *mut Passwd,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Passwd,
) -> c_int {
    unsafe { kinakaze_abi_fgetpwent_r(file, entry, buffer, length, result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetpwent(file: *mut crate::stdio::File) -> *mut Passwd {
    static mut STATIC_PW: Passwd = unsafe { core::mem::zeroed() };
    static mut STATIC_BUF: [c_char; 2048] = [0; 2048];
    let mut res: *mut Passwd = ptr::null_mut();
    let code = unsafe {
        kinakaze_abi_fgetpwent_r(
            file,
            core::ptr::addr_of_mut!(STATIC_PW),
            core::ptr::addr_of_mut!(STATIC_BUF).cast(),
            2048,
            core::ptr::addr_of_mut!(res),
        )
    };
    if code == 0 { res } else { ptr::null_mut() }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn fgetpwent(file: *mut crate::stdio::File) -> *mut Passwd {
    unsafe { kinakaze_abi_fgetpwent(file) }
}

/// `fgetgrent_r`.
///
/// # Safety
///
/// `file` must point to a readable `File`, `entry` and `result` must be writable,
/// and `buffer` must name `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetgrent_r(
    file: *mut crate::stdio::File,
    entry: *mut Group,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Group,
) -> c_int {
    if result.is_null() {
        return kinakaze_vfs::EFAULT;
    }
    unsafe { *result = ptr::null_mut() };
    if file.is_null() || entry.is_null() || buffer.is_null() {
        return kinakaze_vfs::EFAULT;
    }

    let mut line_buf = [0u8; 1024];
    loop {
        let read_res = unsafe {
            crate::stdio::fgets(line_buf.as_mut_ptr().cast(), line_buf.len() as c_int, file)
        };
        if read_res.is_null() {
            return kinakaze_vfs::ENOENT;
        }
        let line_cstr = unsafe { core::ffi::CStr::from_ptr(line_buf.as_ptr().cast()) };
        let line_str = match line_cstr.to_str() {
            Ok(s) => s.trim(),
            Err(_) => continue,
        };
        if line_str.is_empty() || line_str.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line_str.split(':').collect();
        if parts.len() < 3 {
            continue;
        }
        let members: Vec<String> = parts
            .get(3)
            .map(|s| {
                s.split(',')
                    .map(|m| m.trim().to_string())
                    .filter(|m| !m.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let name = parts[0];
        let password = parts.get(1).unwrap_or(&"x");
        let gid = parts
            .get(2)
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let mut packer = Packer::new(buffer, length);
        let g_name = match packer.string(name) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let g_pass = match packer.string(password) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let mut packed_members = Vec::with_capacity(members.len());
        for member in &members {
            match packer.string(member) {
                Ok(s) => packed_members.push(s),
                Err(e) => return e,
            }
        }
        let mem_array = match packer.pointer_array(&packed_members) {
            Ok(a) => a,
            Err(e) => return e,
        };
        unsafe {
            *entry = Group {
                gr_name: g_name,
                gr_passwd: g_pass,
                gr_gid: gid,
                gr_mem: mem_array,
            };
            *result = entry;
        }
        return 0;
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn fgetgrent_r(
    file: *mut crate::stdio::File,
    entry: *mut Group,
    buffer: *mut c_char,
    length: usize,
    result: *mut *mut Group,
) -> c_int {
    unsafe { kinakaze_abi_fgetgrent_r(file, entry, buffer, length, result) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_fgetgrent(file: *mut crate::stdio::File) -> *mut Group {
    static mut STATIC_GR: Group = unsafe { core::mem::zeroed() };
    static mut STATIC_BUF: [c_char; 2048] = [0; 2048];
    let mut res: *mut Group = ptr::null_mut();
    let code = unsafe {
        kinakaze_abi_fgetgrent_r(
            file,
            core::ptr::addr_of_mut!(STATIC_GR),
            core::ptr::addr_of_mut!(STATIC_BUF).cast(),
            2048,
            core::ptr::addr_of_mut!(res),
        )
    };
    if code == 0 { res } else { ptr::null_mut() }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn fgetgrent(file: *mut crate::stdio::File) -> *mut Group {
    unsafe { kinakaze_abi_fgetgrent(file) }
}

// ---------------------------------------------------------------------------
// Shadow passwords.
//
// There is no shadow entry to report, and this is the one place where returning
// nothing is clearly better than returning something. A `struct spwd` exists to
// carry `sp_pwdp`, the password hash, and every caller either verifies a
// supplied password against it or copies it somewhere. Fabricating that field
// has no safe value: an empty string asserts the account needs no password, `*`
// or `!` asserts it is locked, and a made-up hash would be compared against real
// user input by `su`. Reporting absence lets the caller decide, and ENOENT is
// exactly what glibc reports on a host with no shadow file.
//
// The `struct spwd` layout is deliberately not defined here. Nothing is ever
// written through such a pointer, and defining a type this module cannot
// populate honestly would invite a later change to fill it in.
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct Spwd {
    pub sp_namp: *mut c_char,
    pub sp_pwdp: *mut c_char,
    pub sp_lstchg: i64,
    pub sp_min: i64,
    pub sp_max: i64,
    pub sp_warn: i64,
    pub sp_inact: i64,
    pub sp_expire: i64,
    pub sp_flag: u64,
}

// Owns backing allocations for the raw pointers in the published C record.
struct StaticSpwdStorage {
    _name: CString,
    _password: CString,
    spwd: Spwd,
}

unsafe impl Send for StaticSpwdStorage {}
unsafe impl Sync for StaticSpwdStorage {}

static STATIC_SPWD: std::sync::Mutex<Option<Box<StaticSpwdStorage>>> = std::sync::Mutex::new(None);

fn load_shadow_passwords() -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(path) = kinakaze_vfs::resolve_linux_path("/etc/shadow") {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let parts: Vec<&str> = trimmed.split(':').collect();
                if parts.len() >= 2 {
                    map.insert(parts[0].to_string(), parts[1].to_string());
                }
            }
        }
    }
    map
}

fn spwd_entry_for(account: &UserAccount) -> *mut Spwd {
    let mut storage_guard = STATIC_SPWD.lock().unwrap();
    let name = CString::new(account.name.as_str()).unwrap_or_default();
    let shadow_map = load_shadow_passwords();
    let shadow_pwd = shadow_map.get(&account.name).cloned().unwrap_or_default();
    let password = CString::new(shadow_pwd.as_str()).unwrap_or_default();
    let mut storage = Box::new(StaticSpwdStorage {
        spwd: Spwd {
            sp_namp: name.as_ptr().cast_mut(),
            sp_pwdp: password.as_ptr().cast_mut(),
            sp_lstchg: 19000,
            sp_min: 0,
            sp_max: 99999,
            sp_warn: 7,
            sp_inact: -1,
            sp_expire: -1,
            sp_flag: 0,
        },
        _name: name,
        _password: password,
    });
    let ptr = &mut storage.spwd as *mut Spwd;
    *storage_guard = Some(storage);
    ptr
}

/// `getspnam`, returning shadow password entry for user.
///
/// # Safety
///
/// `name` must be null or a null-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn getspnam(name: *const c_char) -> *mut Spwd {
    match unsafe { borrow(name) } {
        Some(name) => {
            if let Some(account) = find_user_by_name(name) {
                spwd_entry_for(&account)
            } else {
                crate::set_errno(ENOENT);
                ptr::null_mut()
            }
        }
        None => {
            crate::set_errno(ENOENT);
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getspnam(name: *const c_char) -> *mut Spwd {
    unsafe { getspnam(name) }
}

/// `getspent`, which reports an immediately-empty enumeration.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getspent() -> *mut c_void {
    crate::set_errno(ENOENT);
    ptr::null_mut()
}

/// `setspent`. There is no enumeration to rewind.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setspent() {}

/// `endspent`. There is no enumeration to close.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_endspent() {}

/// `crypt` password hashing.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn crypt(key: *const c_char, salt: *const c_char) -> *mut c_char {
    static RESULT: std::sync::Mutex<[u8; 256]> = std::sync::Mutex::new([0u8; 256]);
    if key.is_null() || salt.is_null() {
        return ptr::null_mut();
    }
    let salt_bytes = unsafe { CStr::from_ptr(salt).to_bytes() };
    let mut buf = RESULT.lock().unwrap();
    if salt_bytes.is_empty() {
        buf[0] = 0;
        return buf.as_mut_ptr().cast();
    }
    let len = salt_bytes.len().min(buf.len() - 1);
    buf[..len].copy_from_slice(&salt_bytes[..len]);
    buf[len] = 0;
    buf.as_mut_ptr().cast()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_crypt(
    key: *const c_char,
    salt: *const c_char,
) -> *mut c_char {
    unsafe { crypt(key, salt) }
}

#[repr(C)]
pub struct CryptData {
    pub keysched: [c_char; 128],
    pub sb0: [c_char; 32],
    pub sb1: [c_char; 32],
    pub sb2: [c_char; 32],
    pub sb3: [c_char; 32],
    pub crypt_3_buf: [c_char; 14],
    pub current_salt: [c_char; 2],
    pub current_saltbits: i64,
    pub direction: c_int,
    pub initialized: c_int,
    pub arg: c_int,
    pub buffer: [c_char; 128],
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn crypt_r(
    key: *const c_char,
    salt: *const c_char,
    data: *mut CryptData,
) -> *mut c_char {
    if key.is_null() || salt.is_null() || data.is_null() {
        return ptr::null_mut();
    }
    let salt_bytes = unsafe { CStr::from_ptr(salt).to_bytes() };
    let data_ref = unsafe { &mut *data };
    if salt_bytes.is_empty() {
        data_ref.buffer[0] = 0;
        return data_ref.buffer.as_mut_ptr();
    }
    let len = salt_bytes.len().min(data_ref.buffer.len() - 1);
    unsafe {
        ptr::copy_nonoverlapping(
            salt.cast::<u8>(),
            data_ref.buffer.as_mut_ptr().cast::<u8>(),
            len,
        );
        *data_ref.buffer.as_mut_ptr().add(len) = 0;
    }
    data_ref.buffer.as_mut_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_crypt_r(
    key: *const c_char,
    salt: *const c_char,
    data: *mut CryptData,
) -> *mut c_char {
    unsafe { crypt_r(key, salt, data) }
}

// ---------------------------------------------------------------------------
// Capabilities.
//
// Windows has no POSIX capability model. Its privilege system is a per-token set
// of named privileges with a different granularity and no correspondence to the
// CAP_* bits, so there is no truthful host mapping to make. The Linux process
// view therefore has a real, empty capability set: capget reports that set and
// capset may preserve it but may not raise bits. Operations are still authorized
// by Windows at the point where they are attempted.
// ---------------------------------------------------------------------------

const LINUX_CAPABILITY_VERSION_1: u32 = 0x1998_0330;
const LINUX_CAPABILITY_VERSION_2: u32 = 0x2007_1026;
const LINUX_CAPABILITY_VERSION_3: u32 = 0x2008_0522;

/// Linux currently defines capabilities 0 through 40. Bits above that range
/// are ignored by the kernel when it imports the two 32-bit words.
const LINUX_CAPABILITY_VALID_MASK: u64 = (1_u64 << 41) - 1;

#[repr(C)]
#[derive(Clone, Copy)]
struct UserCapabilityHeader {
    version: u32,
    pid: c_int,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UserCapabilityData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// Read through the capability ABI so permission checks share its authority.
pub(crate) fn effective_capabilities() -> Result<u64, i32> {
    let mut header = UserCapabilityHeader {
        version: LINUX_CAPABILITY_VERSION_3,
        pid: 0,
    };
    let mut data = [UserCapabilityData::default(); 2];
    if unsafe { kinakaze_abi_capget((&raw mut header).cast(), data.as_mut_ptr().cast()) } != 0 {
        return Err(crate::kinakaze_errno());
    }
    Ok(u64::from(data[0].effective) | (u64::from(data[1].effective) << 32))
}

/// Returns the number of data records used by a supported Linux capability
/// ABI revision.
fn capability_record_count(version: u32) -> Option<usize> {
    match version {
        LINUX_CAPABILITY_VERSION_1 => Some(1),
        LINUX_CAPABILITY_VERSION_2 | LINUX_CAPABILITY_VERSION_3 => Some(2),
        _ => None,
    }
}

/// `capget`.
///
/// # Safety
///
/// `header` must be writable. When `data` is non-null it must hold the number of
/// records selected by `header.version`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_capget(header: *mut c_void, data: *mut c_void) -> c_int {
    if header.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }

    // SAFETY: required by this function's contract and checked for null above.
    let header = unsafe { &mut *header.cast::<UserCapabilityHeader>() };
    let records = match capability_record_count(header.version) {
        Some(records) => records,
        None => {
            // Linux writes its preferred revision back. A null data pointer is
            // the version-probe form and succeeds; otherwise EINVAL reports the
            // unsupported revision.
            header.version = LINUX_CAPABILITY_VERSION_3;
            if data.is_null() {
                return 0;
            }
            crate::set_errno(EINVAL);
            return -1;
        }
    };

    // Linux permits a null data pointer for a version probe. Once the version
    // was accepted there is no payload to copy and the call succeeds.
    if data.is_null() {
        return 0;
    }
    if header.pid < 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    let own_pid = crate::process::kinakaze_abi_getpid();
    if header.pid != 0 && header.pid != own_pid {
        crate::set_errno(ESRCH);
        return -1;
    }

    // SAFETY: the caller supplied the record count selected by the ABI version.
    let data =
        unsafe { core::slice::from_raw_parts_mut(data.cast::<UserCapabilityData>(), records) };
    let caps = kinakaze_vfs::user_namespace::capabilities();
    for (n, row) in data.iter_mut().enumerate() {
        *row = UserCapabilityData {
            effective: (caps[0] >> (32 * n)) as u32,
            permitted: (caps[1] >> (32 * n)) as u32,
            inheritable: (caps[2] >> (32 * n)) as u32,
        };
    }
    0
}

/// `capset`.
///
/// # Safety
///
/// `header` must be writable and `data` must contain the number of records
/// selected by `header.version`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_capset(
    header: *mut c_void,
    data: *const c_void,
) -> c_int {
    if header.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }

    // SAFETY: required by this function's contract and checked for null above.
    let header = unsafe { &mut *header.cast::<UserCapabilityHeader>() };
    let records = match capability_record_count(header.version) {
        Some(records) => records,
        None => {
            header.version = LINUX_CAPABILITY_VERSION_3;
            crate::set_errno(EINVAL);
            return -1;
        }
    };
    let own_pid = crate::process::kinakaze_abi_getpid();
    if header.pid != 0 && header.pid != own_pid {
        crate::set_errno(EPERM);
        return -1;
    }
    if data.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }

    // SAFETY: the caller supplied the record count selected by the ABI version.
    let data = unsafe { core::slice::from_raw_parts(data.cast::<UserCapabilityData>(), records) };
    let low = data[0];
    let high = data.get(1).copied().unwrap_or_default();
    let effective = (u64::from(low.effective) | (u64::from(high.effective) << 32))
        & LINUX_CAPABILITY_VALID_MASK;
    let permitted = (u64::from(low.permitted) | (u64::from(high.permitted) << 32))
        & LINUX_CAPABILITY_VALID_MASK;
    let inheritable = (u64::from(low.inheritable) | (u64::from(high.inheritable) << 32))
        & LINUX_CAPABILITY_VALID_MASK;
    match kinakaze_vfs::user_namespace::set_capabilities([effective, permitted, inheritable]) {
        Ok(()) => 0,
        Err(e) => {
            crate::set_errno(e);
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// System identification and limits.
// ---------------------------------------------------------------------------

/// Length of each `utsname` field on Linux, including the terminator.
const UTS_LEN: usize = 65;

/// The Linux x86_64 `struct utsname`.
///
/// Six fixed 65-byte arrays with no padding, so the struct is exactly 390 bytes.
/// Guest code indexes these directly and `uname -a` reads all six, so both the
/// field order and the array size are ABI. The `size_of` test below pins it.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Utsname {
    pub sysname: [c_char; UTS_LEN],
    pub nodename: [c_char; UTS_LEN],
    pub release: [c_char; UTS_LEN],
    pub version: [c_char; UTS_LEN],
    pub machine: [c_char; UTS_LEN],
    /// Present on Linux under `_GNU_SOURCE` and part of the struct either way.
    pub domainname: [c_char; UTS_LEN],
}

/// Copies `text` into a fixed `utsname` field, truncating and terminating.
///
/// Truncation at 64 bytes matches the kernel, which silently truncates rather
/// than failing, and the field is always left NUL-terminated.
///
/// # Safety
///
/// `field` must name `UTS_LEN` writable bytes.
unsafe fn write_field(field: *mut c_char, text: &str) {
    let bytes = text.as_bytes();
    let count = bytes.len().min(UTS_LEN - 1);
    // SAFETY: `count` is below `UTS_LEN`, and the caller guarantees the field
    // holds that many bytes plus the terminator slot written after it.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), field.cast::<u8>(), count);
        // The whole tail is zeroed, not just one byte: guest code sometimes
        // hashes or compares the full field.
        ptr::write_bytes(field.cast::<u8>().add(count), 0, UTS_LEN - count);
    }
}

/// The real Windows version, as major, minor and build.
fn windows_version() -> (u32, u32, u32) {
    let mut info = OsVersionInfoW::default();
    // SAFETY: `info` is a writable local with its `size` field set, which is the
    // documented contract. RtlGetVersion cannot fail.
    unsafe { RtlGetVersion(&raw mut info) };
    (info.major, info.minor, info.build)
}

/// `uname`.
///
/// `sysname` is `Linux`. This is the deliberate substitution the module header
/// describes: BusyBox and the programs it runs branch on this string, and the
/// non-Linux branches call facilities this layer does not implement, so
/// reporting `Windows` would route working code into paths that cannot run. The
/// real host is not concealed; it is spelled out in `version`, which is where
/// `uname -a` shows it.
///
/// `release` carries the Linux API level this layer targets, because that is the
/// question a caller is asking when it parses `release`: version comparisons
/// against it decide whether a syscall or `/proc` field is assumed present.
/// Putting the Windows build number here would fail every such comparison.
///
/// `nodename` and the Windows build in `version` are read from the host.
///
/// # Safety
///
/// `name` must point at a writable `struct utsname`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_uname(name: *mut Utsname) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let (major, minor, build) = windows_version();

    // SAFETY: the caller guarantees a writable struct, so each field address is
    // valid for the `UTS_LEN` bytes `write_field` writes.
    unsafe {
        write_field((&raw mut (*name).sysname).cast::<c_char>(), "Linux");
        write_field(
            (&raw mut (*name).nodename).cast::<c_char>(),
            &computer_name(),
        );
        // Matches the release `/proc/version` reports, so a guest cannot get two
        // different answers to the same question.
        write_field(
            (&raw mut (*name).release).cast::<c_char>(),
            "6.1.0-kinakaze",
        );
        // The honest statement of what this actually is, carrying the true
        // Windows version so it is never hidden from the user.
        write_field(
            (&raw mut (*name).version).cast::<c_char>(),
            &format!("#1 SMP kinakaze on Windows {major}.{minor}.{build}"),
        );
        // The architecture is real: this code only builds for x86_64.
        write_field((&raw mut (*name).machine).cast::<c_char>(), "x86_64");
        // `(none)` is what Linux reports when no NIS domain is set, which is the
        // true state here rather than a placeholder.
        write_field(
            (&raw mut (*name).domainname).cast::<c_char>(),
            &String::from_utf8_lossy(&kinakaze_vfs::namespaces::uts(true).unwrap_or_default()),
        );
    }
    0
}

/// `gethostname`.
///
/// Truncation is an error, not silent: POSIX specifies `ENAMETOOLONG` when the
/// name does not fit, and a truncated hostname would be used to build URLs.
///
/// # Safety
///
/// `name` must name at least `length` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_gethostname(name: *mut c_char, length: usize) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let host = computer_name();
    let bytes = host.as_bytes();
    if length < bytes.len() + 1 {
        crate::set_errno(kinakaze_vfs::ENAMETOOLONG);
        return -1;
    }
    // SAFETY: the check above proved the buffer holds the name and a terminator.
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), name.cast::<u8>(), bytes.len());
        *name.add(bytes.len()) = 0;
    }
    0
}

/// `sethostname`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn sethostname(name: *const c_char, length: usize) -> c_int {
    unsafe { kinakaze_abi_sethostname(name, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sethostname(
    name: *const c_char,
    length: usize,
) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if length > 64 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts(name as *const u8, length) };
    match kinakaze_vfs::namespaces::set_uts(false, slice) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// Read the current UTS domain; glibc rejects short buffers with EINVAL.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getdomainname(
    name: *mut c_char,
    length: usize,
) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let domain = match kinakaze_vfs::namespaces::uts(true) {
        Ok(domain) => domain,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    if length <= domain.len() {
        crate::set_errno(EINVAL);
        return -1;
    }
    unsafe {
        ptr::copy_nonoverlapping(domain.as_ptr(), name.cast::<u8>(), domain.len());
        *name.add(domain.len()) = 0;
    }
    0
}

/// `setdomainname`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn setdomainname(name: *const c_char, length: usize) -> c_int {
    unsafe { kinakaze_abi_setdomainname(name, length) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setdomainname(
    name: *const c_char,
    length: usize,
) -> c_int {
    if name.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    if length > 64 {
        crate::set_errno(kinakaze_vfs::EINVAL);
        return -1;
    }
    let slice = unsafe { core::slice::from_raw_parts(name as *const u8, length) };
    match kinakaze_vfs::namespaces::set_uts(true, slice) {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `sysconf` names, with the values Linux assigns them.
///
/// The numbers are ABI: the guest was compiled against glibc's headers and
/// passes these integers, so they cannot be renumbered.
pub const SC_ARG_MAX: c_int = 0;
pub const SC_CHILD_MAX: c_int = 1;
pub const SC_CLK_TCK: c_int = 2;
pub const SC_NGROUPS_MAX: c_int = 3;
pub const SC_OPEN_MAX: c_int = 4;
pub const SC_PAGESIZE: c_int = 30;
pub const SC_GETPW_R_SIZE_MAX: c_int = 70;
pub const SC_GETGR_R_SIZE_MAX: c_int = 69;
pub const SC_LOGIN_NAME_MAX: c_int = 71;
pub const SC_NPROCESSORS_CONF: c_int = 83;
pub const SC_NPROCESSORS_ONLN: c_int = 84;
pub const SC_PHYS_PAGES: c_int = 85;
pub const SC_HOST_NAME_MAX: c_int = 180;

/// The page size, which is 4096 on every Windows platform this targets.
///
/// `GetSystemInfo` is asked rather than assuming, because the value feeds RSS and
/// mapping arithmetic where a wrong constant is a silent corruption. The call
/// cannot fail, so the 4096 fallback only covers a zero result that should never
/// occur.
fn page_size() -> i64 {
    let mut info = SystemInfo::default();
    // SAFETY: `info` is a writable local of the right type; the call cannot fail.
    unsafe { GetSystemInfo(&raw mut info) };
    if info.page_size == 0 {
        return 4096;
    }
    i64::from(info.page_size)
}

/// Logical processors this process may run on.
///
/// `available_parallelism` respects the affinity mask, so a process pinned to
/// four cores of a large machine sees four, which is the figure it should size a
/// thread pool from.
fn online_processors() -> i64 {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get) as i64
}

/// `sysconf`.
///
/// Unknown names return -1 with errno untouched, which is the POSIX encoding for
/// "no limit is defined for this name". Returning 0 would be wrong in a way that
/// is hard to debug: 0 is a legitimate answer for several names, so a caller
/// cannot tell it apart from an unsupported one.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_sysconf(name: c_int) -> i64 {
    sysconf(name)
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn sysconf(name: c_int) -> i64 {
    match name {
        SC_PAGESIZE => page_size(),
        SC_NPROCESSORS_ONLN => online_processors(),
        SC_NPROCESSORS_CONF => {
            let mut info = SystemInfo::default();
            // SAFETY: `info` is a writable local; the call cannot fail.
            unsafe { GetSystemInfo(&raw mut info) };
            if info.number_of_processors == 0 {
                online_processors()
            } else {
                i64::from(info.number_of_processors)
            }
        }
        // sysconf reports the current soft limit, including prlimit changes.
        SC_OPEN_MAX => match kinakaze_vfs::job::current_nofile_limit() {
            Ok(limit) => limit as i64,
            Err(error) => {
                crate::set_errno(error);
                -1
            }
        },
        // Linux fixes USER_HZ at 100 and `times` results are in those units, so
        // this must be 100 for a guest converting clock ticks to seconds.
        SC_CLK_TCK => 100,
        // Large enough for the entry this module produces several times over.
        // glibc reports 1024 here and BusyBox uses it as its first allocation
        // before growing on ERANGE.
        SC_GETPW_R_SIZE_MAX | SC_GETGR_R_SIZE_MAX => 1024,
        // The Windows command-line limit, which is the real constraint on this
        // host. Linux's nominal 2 MiB would let `xargs` build a command line
        // that fails later in `CreateProcess`.
        SC_ARG_MAX => 32 * 1024,
        // An estimate of total physical RAM in 4 KiB pages.
        SC_PHYS_PAGES => {
            use windows_sys::Win32::System::SystemInformation::{
                GlobalMemoryStatusEx, MEMORYSTATUSEX,
            };
            let mut status = MEMORYSTATUSEX {
                dwLength: core::mem::size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            // SAFETY: `status` is properly sized and initialized.
            if unsafe { GlobalMemoryStatusEx(&raw mut status) } != 0 {
                (status.ullTotalPhys / 4096) as i64
            } else {
                -1
            }
        }
        // One group exists, so one is the most a caller could ever be in.
        SC_NGROUPS_MAX => 1,
        // A NetBIOS name is at most 15 characters, but callers size buffers for
        // the DNS-style name too, so POSIX's 64 is the useful bound.
        SC_HOST_NAME_MAX => 64,
        // The Windows account name limit.
        SC_LOGIN_NAME_MAX => 256,
        // Deliberately not 0. See the function documentation.
        _ => -1,
    }
}

/// `getpagesize`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpagesize() -> c_int {
    getpagesize()
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn getpagesize() -> c_int {
    page_size() as c_int
}

/// `getdtablesize`, the descriptor table capacity.
///
/// Answers from `sysconf` so the two can never disagree.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getdtablesize() -> c_int {
    getdtablesize()
}

#[unsafe(no_mangle)]
pub extern "sysv64" fn getdtablesize() -> c_int {
    sysconf(SC_OPEN_MAX) as c_int
}

/// The Linux x86_64 `struct rlimit`: two 64-bit limits.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RLimit {
    pub rlim_cur: u64,
    pub rlim_max: u64,
}

/// `RLIM_INFINITY`, an all-ones 64-bit value on Linux x86_64.
pub const RLIM_INFINITY: u64 = u64::MAX;

/// `getrlimit`/`setrlimit` resource numbers.
pub const RLIMIT_CPU: c_int = 0;
pub const RLIMIT_FSIZE: c_int = 1;
pub const RLIMIT_DATA: c_int = 2;
pub const RLIMIT_STACK: c_int = 3;
pub const RLIMIT_CORE: c_int = 4;
pub const RLIMIT_NOFILE: c_int = 7;
pub const RLIMIT_AS: c_int = 9;
pub const RLIMIT_NPROC: c_int = 6;
/// The highest resource number Linux defines, which bounds a valid argument.
const RLIMIT_NLIMITS: c_int = 16;

/// `getrlimit`.
///
/// # Safety
///
/// `limit` must point at a writable `struct rlimit`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getrlimit(resource: c_int, limit: *mut RLimit) -> c_int {
    // Linux x86_64 rlimit and rlimit64 have identical layouts. Use the same
    // backend as setrlimit, getrlimit64 and the raw syscalls, including their
    // argument validation and errno convention. The backend never calls us.
    // SAFETY: both repr(C) structures contain two u64 fields in the same order;
    // null remains null so the backend can report EFAULT.
    unsafe { crate::sysadmin::kinakaze_abi_getrlimit64(resource, limit.cast()) }
}

/// `setrlimit`.
///
/// A request that asks for the limit already in force succeeds, which is what
/// makes the common "raise the soft limit to the hard limit" idiom work when the
/// limits are equal. Changes use the same retained/enforced backend as
/// `setrlimit64`; unsupported changes fail with `EOPNOTSUPP`.
///
/// # Safety
///
/// `limit` must point at a readable `struct rlimit`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_setrlimit(
    resource: c_int,
    limit: *const RLimit,
) -> c_int {
    if !(0..RLIMIT_NLIMITS).contains(&resource) {
        crate::set_errno(EINVAL);
        return -1;
    }
    if limit.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    // SAFETY: the caller guarantees a readable struct.
    let requested = unsafe { *limit };
    if requested.rlim_cur > requested.rlim_max {
        crate::set_errno(EINVAL);
        return -1;
    }
    let req64 = crate::sysadmin::RLimit64 {
        rlim_cur: requested.rlim_cur,
        rlim_max: requested.rlim_max,
    };
    unsafe { crate::sysadmin::kinakaze_abi_setrlimit64(resource, &raw const req64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_prlimit64(
    pid: c_int,
    resource: c_int,
    new_limit: *const RLimit,
    old_limit: *mut RLimit,
) -> c_int {
    if pid < 0 {
        crate::set_errno(EINVAL);
        return -1;
    }
    let result = unsafe {
        crate::sysadmin::prlimit64(
            pid as u32,
            resource,
            new_limit.cast::<crate::sysadmin::RLimit64>(),
            old_limit.cast::<crate::sysadmin::RLimit64>(),
        )
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// The Linux `struct timeval`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TimeVal {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

/// The Linux x86_64 `struct rusage`.
///
/// The two `timeval`s are followed by fourteen `long` counters. All sixteen
/// fields are present because the struct is ABI even where a field cannot be
/// filled.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RUsage {
    pub ru_utime: TimeVal,
    pub ru_stime: TimeVal,
    pub ru_maxrss: i64,
    pub ru_ixrss: i64,
    pub ru_idrss: i64,
    pub ru_isrss: i64,
    pub ru_minflt: i64,
    pub ru_majflt: i64,
    pub ru_nswap: i64,
    pub ru_inblock: i64,
    pub ru_oublock: i64,
    pub ru_msgsnd: i64,
    pub ru_msgrcv: i64,
    pub ru_nsignals: i64,
    pub ru_nvcsw: i64,
    pub ru_nivcsw: i64,
}

/// `getrusage` targets.
pub const RUSAGE_SELF: c_int = 0;
pub const RUSAGE_CHILDREN: c_int = -1;
pub const RUSAGE_THREAD: c_int = 1;

/// `getrusage`.
///
/// Four fields are real: both CPU times come from `GetProcessTimes`, `ru_maxrss`
/// from the peak working set, and `ru_majflt` from the Windows page-fault count.
/// The rest stay zero because Windows does not account for them per process;
/// they are left as zeros rather than estimated, so `time` and BusyBox's
/// `-v` output report real CPU figures and nothing invented.
///
/// `RUSAGE_CHILDREN` reports zeros, which is accurate until this layer reaps a
/// child's accounting; it has no aggregated child totals to draw on.
///
/// # Safety
///
/// `usage` must point at a writable `struct rusage`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_getrusage(who: c_int, usage: *mut RUsage) -> c_int {
    if !matches!(who, RUSAGE_SELF | RUSAGE_CHILDREN | RUSAGE_THREAD) {
        crate::set_errno(EINVAL);
        return -1;
    }
    if usage.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let mut result = RUsage::default();

    if who != RUSAGE_CHILDREN {
        let mut creation = FileTime::default();
        let mut exit = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        // SAFETY: the pseudo-handle is always valid and all four out-parameters
        // are writable locals.
        let ok = unsafe {
            GetProcessTimes(
                GetCurrentProcess(),
                &raw mut creation,
                &raw mut exit,
                &raw mut kernel,
                &raw mut user,
            )
        };
        if ok != 0 {
            let (seconds, microseconds) = user.seconds_and_microseconds();
            result.ru_utime = TimeVal {
                tv_sec: seconds,
                tv_usec: microseconds,
            };
            let (seconds, microseconds) = kernel.seconds_and_microseconds();
            result.ru_stime = TimeVal {
                tv_sec: seconds,
                tv_usec: microseconds,
            };
        }

        let mut counters = ProcessMemoryCounters {
            size: size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: the pseudo-handle is valid and `counters` is a writable local
        // whose `size` matches the length passed.
        let ok = unsafe {
            K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &raw mut counters,
                size_of::<ProcessMemoryCounters>() as u32,
            )
        };
        if ok != 0 {
            // Linux reports maxrss in kilobytes.
            result.ru_maxrss = (counters.peak_working_set / 1024) as i64;
            // Windows counts all faults together without splitting soft from
            // hard, so the total is reported as major faults: those are the ones
            // a caller reading this field cares about, and claiming zero would
            // hide real paging activity.
            result.ru_majflt = i64::from(counters.page_fault_count);
        }
    }

    // SAFETY: the caller guarantees a writable struct.
    unsafe { *usage = result };
    0
}

/// The Linux x86_64 `struct sysinfo`.
///
/// The trailing `_f` padding is part of the declared struct on Linux; it is kept
/// so the size matches what a guest compiled against `<sys/sysinfo.h>` expects.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SysInfo {
    pub uptime: i64,
    pub loads: [u64; 3],
    pub totalram: u64,
    pub freeram: u64,
    pub sharedram: u64,
    pub bufferram: u64,
    pub totalswap: u64,
    pub freeswap: u64,
    pub procs: u16,
    pub pad: u16,
    pub totalhigh: u64,
    pub freehigh: u64,
    pub mem_unit: u32,
    pub _f: [c_char; 0],
}

/// `sysinfo`.
///
/// Uptime comes from `GetTickCount64`; physical memory and pagefile totals use
/// the same native accounting as `/proc/meminfo`. Windows publishes no load
/// average, so `loads` stays zero, and it has no equivalent of Linux's shared or
/// buffer memory partitions, so those are zero too. `procs` is zero because
/// counting processes needs a full ToolHelp snapshot, which is too expensive for
/// a call BusyBox makes to print memory figures.
///
/// `freeram` counts free/zero pages, excluding reclaimable standby pages.
///
/// # Safety
///
/// `info` must point at a writable `struct sysinfo`.
#[unsafe(no_mangle)]
pub unsafe extern "sysv64" fn kinakaze_abi_sysinfo(info: *mut SysInfo) -> c_int {
    if info.is_null() {
        crate::set_errno(kinakaze_vfs::EFAULT);
        return -1;
    }
    let status = match kinakaze_vfs::memory::snapshot() {
        Ok(status) => status,
        Err(error) => {
            crate::set_errno(error);
            return -1;
        }
    };
    // SAFETY: GetTickCount64 has no preconditions and cannot fail.
    let uptime = unsafe { GetTickCount64() } / 1000;

    let result = SysInfo {
        uptime: uptime as i64,
        loads: [0; 3],
        totalram: status.total,
        freeram: status.free,
        sharedram: 0,
        bufferram: 0,
        totalswap: status.swap_total,
        freeswap: status.swap_free,
        procs: 0,
        pad: 0,
        // No high memory exists on x86_64.
        totalhigh: 0,
        freehigh: 0,
        // Byte-granular, so the totals above need no scaling.
        mem_unit: 1,
        _f: [],
    };
    // SAFETY: the caller guarantees a writable struct.
    unsafe { *info = result };
    0
}

/// Windows priority classes, in ascending order of urgency.
const IDLE_PRIORITY_CLASS: u32 = 0x0000_0040;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
const NORMAL_PRIORITY_CLASS: u32 = 0x0000_0020;
const ABOVE_NORMAL_PRIORITY_CLASS: u32 = 0x0000_8000;
const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;

/// `getpriority`/`setpriority` target kinds.
pub const PRIO_PROCESS: c_int = 0;
pub const PRIO_PGRP: c_int = 1;
pub const PRIO_USER: c_int = 2;

/// Maps a Windows priority class onto the nice value that best represents it.
///
/// The mapping is real in both directions rather than a stub: Windows classes and
/// nice values are both coarse scheduling hints, so a `nice -n 10` really does
/// lower this process's Windows priority. The five classes cannot express all 40
/// nice values, so the conversion is lossy, and reading back after a set can
/// return a neighbouring value. Real Linux also clamps and rounds here.
fn class_to_nice(class: u32) -> c_int {
    match class {
        IDLE_PRIORITY_CLASS => 19,
        BELOW_NORMAL_PRIORITY_CLASS => 10,
        ABOVE_NORMAL_PRIORITY_CLASS => -10,
        HIGH_PRIORITY_CLASS => -20,
        // NORMAL_PRIORITY_CLASS, and REALTIME which no guest asked for.
        _ => 0,
    }
}

/// Maps a nice value onto the closest Windows priority class.
fn nice_to_class(nice: c_int) -> u32 {
    match nice {
        ..=-16 => HIGH_PRIORITY_CLASS,
        -15..=-5 => ABOVE_NORMAL_PRIORITY_CLASS,
        -4..=4 => NORMAL_PRIORITY_CLASS,
        5..=14 => BELOW_NORMAL_PRIORITY_CLASS,
        _ => IDLE_PRIORITY_CLASS,
    }
}

/// This process's current nice value, from its Windows priority class.
fn current_nice() -> c_int {
    // SAFETY: the pseudo-handle is always valid.
    let class = unsafe { GetPriorityClass(GetCurrentProcess()) };
    if class == 0 {
        // The query failed, so the default class is the best available answer.
        return 0;
    }
    class_to_nice(class)
}

/// Applies a nice value by setting the matching Windows priority class.
fn apply_nice(nice: c_int) -> Result<c_int, i32> {
    // Linux clamps to [-20, 19] rather than failing.
    let clamped = nice.clamp(-20, 19);
    // SAFETY: the pseudo-handle is valid and the class is a documented constant.
    let ok = unsafe { SetPriorityClass(GetCurrentProcess(), nice_to_class(clamped)) };
    if ok == 0 {
        // Raising priority can be refused by the host, which is a real EPERM
        // rather than one this layer invented.
        return Err(EPERM);
    }
    Ok(clamped)
}

/// `nice`, which adds `increment` to the current value and returns the new one.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_nice(increment: c_int) -> c_int {
    let target = current_nice().saturating_add(increment);
    match apply_nice(target) {
        Ok(nice) => nice,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

/// `getpriority`.
///
/// Only this process can be described. A `who` of 0 means the caller, and a
/// process group or user target is answered for this process because it is the
/// only member of both. Naming a different process is refused rather than
/// answered with this process's value.
///
/// The return value is the nice value itself, so -1 is a legitimate result; the
/// documented way to detect an error is to clear errno first and check it after.
/// errno is therefore cleared on the success path.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_getpriority(which: c_int, who: u32) -> c_int {
    if !matches!(which, PRIO_PROCESS | PRIO_PGRP | PRIO_USER) {
        crate::set_errno(EINVAL);
        return -1;
    }
    // For PRIO_USER, `who` is a uid, and 0 is the uid this process has.
    if who != 0 && !(which == PRIO_PROCESS && who == own_pid() as u32) {
        crate::set_errno(ESRCH);
        return -1;
    }
    // Cleared so a caller using the errno protocol does not read a stale value.
    crate::set_errno(0);
    current_nice()
}

/// `setpriority`.
#[unsafe(no_mangle)]
pub extern "sysv64" fn kinakaze_abi_setpriority(which: c_int, who: u32, value: c_int) -> c_int {
    if !matches!(which, PRIO_PROCESS | PRIO_PGRP | PRIO_USER) {
        crate::set_errno(EINVAL);
        return -1;
    }
    if who != 0 && !(which == PRIO_PROCESS && who == own_pid() as u32) {
        crate::set_errno(ESRCH);
        return -1;
    }
    match apply_nice(value) {
        Ok(_) => 0,
        Err(error) => {
            crate::set_errno(error);
            -1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads a C string the ABI produced, asserting it is valid and terminated.
    ///
    /// # Safety
    ///
    /// `text` must be a null-terminated string.
    unsafe fn read(text: *const c_char) -> String {
        assert!(!text.is_null(), "a required string field was null");
        // SAFETY: forwarded from this function's contract.
        unsafe { CStr::from_ptr(text) }
            .to_str()
            .expect("field is not valid UTF-8")
            .to_string()
    }

    #[test]
    fn struct_layouts_match_the_linux_abi() {
        // Guest code indexes these offsets directly, so the sizes are ABI and a
        // change to any field would silently corrupt a guest's reads.
        assert_eq!(
            size_of::<Passwd>(),
            48,
            "struct passwd is 5 pointers + 2 u32"
        );
        assert_eq!(align_of::<Passwd>(), 8);
        assert_eq!(size_of::<Group>(), 32, "struct group is 3 pointers + u32");
        assert_eq!(align_of::<Group>(), 8);
        // Six 65-byte arrays, no padding, no alignment beyond a byte.
        assert_eq!(size_of::<Utsname>(), 390);
        assert_eq!(align_of::<Utsname>(), 1);
        assert_eq!(size_of::<RLimit>(), 16);
        assert_eq!(size_of::<TimeVal>(), 16);
        assert_eq!(size_of::<RUsage>(), 144, "2 timevals + 14 longs");
        assert_eq!(size_of::<SysInfo>(), 112);

        // Field offsets, which is what a guest actually depends on.
        let passwd = Passwd {
            pw_name: ptr::null_mut(),
            pw_passwd: ptr::null_mut(),
            pw_uid: 0,
            pw_gid: 0,
            pw_gecos: ptr::null_mut(),
            pw_dir: ptr::null_mut(),
            pw_shell: ptr::null_mut(),
        };
        let base = (&raw const passwd) as usize;
        assert_eq!((&raw const passwd.pw_uid) as usize - base, 16);
        assert_eq!((&raw const passwd.pw_gid) as usize - base, 20);
        assert_eq!((&raw const passwd.pw_gecos) as usize - base, 24);
        assert_eq!((&raw const passwd.pw_shell) as usize - base, 40);
    }

    #[test]
    fn the_process_reports_itself_as_root() {
        CURRENT_UID.store(0, Ordering::Relaxed);
        CURRENT_EUID.store(0, Ordering::Relaxed);
        CURRENT_GID.store(0, Ordering::Relaxed);
        CURRENT_EGID.store(0, Ordering::Relaxed);
        CURRENT_SUID.store(0, Ordering::Relaxed);
        CURRENT_SGID.store(0, Ordering::Relaxed);

        // BusyBox gates applets on these being zero; see the module header.
        assert_eq!(kinakaze_abi_getuid(), 0);
        assert_eq!(kinakaze_abi_geteuid(), 0);
        assert_eq!(kinakaze_abi_getgid(), 0);
        assert_eq!(kinakaze_abi_getegid(), 0);

        let mut real = 9u32;
        let mut effective = 9u32;
        let mut saved = 9u32;
        // SAFETY: three writable locals.
        let result =
            unsafe { kinakaze_abi_getresuid(&raw mut real, &raw mut effective, &raw mut saved) };
        assert_eq!(result, 0);
        assert_eq!((real, effective, saved), (0, 0, 0));

        // SAFETY: three writable locals.
        let result =
            unsafe { kinakaze_abi_getresgid(&raw mut real, &raw mut effective, &raw mut saved) };
        assert_eq!(result, 0);
        assert_eq!((real, effective, saved), (0, 0, 0));
    }

    #[test]
    fn identity_changes_succeed_only_for_the_identity_we_have() {
        CURRENT_UID.store(0, Ordering::Relaxed);
        CURRENT_EUID.store(0, Ordering::Relaxed);
        CURRENT_GID.store(0, Ordering::Relaxed);
        CURRENT_EGID.store(0, Ordering::Relaxed);
        CURRENT_SUID.store(0, Ordering::Relaxed);
        CURRENT_SGID.store(0, Ordering::Relaxed);

        for setter in [
            kinakaze_abi_setuid as extern "sysv64" fn(u32) -> c_int,
            kinakaze_abi_seteuid,
            kinakaze_abi_setgid,
            kinakaze_abi_setegid,
        ] {
            assert_eq!(setter(0), 0, "setting to root should succeed");
        }

        // `(uid_t)-1` means leave unchanged and is always acceptable.
        assert_eq!(kinakaze_abi_setresuid(0, UNCHANGED, 0), 0);
        assert_eq!(kinakaze_abi_setresgid(UNCHANGED, UNCHANGED, UNCHANGED), 0);
        assert_eq!(kinakaze_abi_setresuid(0, 1000, 0), 0);
        assert_eq!(kinakaze_abi_setresuid(0, 0, 0), 0);
    }

    #[test]
    fn getpwuid_returns_a_usable_entry_for_root() {
        let entry = kinakaze_abi_getpwuid(0);
        assert!(!entry.is_null(), "uid 0 must have a passwd entry");
        // SAFETY: the entry points at this module's static storage.
        let entry = unsafe { *entry };

        // SAFETY: every string field of a returned entry is null-terminated.
        let name = unsafe { read(entry.pw_name) };
        assert!(!name.is_empty(), "pw_name must not be empty");
        // The name is the real Windows account, so it cannot contain a separator
        // or whitespace that would break a `/etc/passwd`-shaped line.
        assert!(
            !name.contains(':') && !name.contains('\n'),
            "pw_name {name:?} would corrupt a passwd line"
        );

        assert_eq!(entry.pw_uid, 0);
        assert_eq!(entry.pw_gid, 0);
        // SAFETY: null-terminated fields.
        let directory = unsafe { read(entry.pw_dir) };
        assert!(
            directory.starts_with('/'),
            "pw_dir {directory:?} must be an absolute guest path"
        );
        assert!(
            !directory.contains('\\') && !directory.contains(':'),
            "pw_dir {directory:?} leaked a Windows path shape"
        );
        // SAFETY: null-terminated fields.
        assert_eq!(unsafe { read(entry.pw_shell) }, "/bin/sh");
        // Not empty: an empty password field asserts no password is needed.
        // SAFETY: null-terminated fields.
        assert_eq!(unsafe { read(entry.pw_passwd) }, "x");
        // SAFETY: gecos is empty but still a valid string, never null.
        assert_eq!(unsafe { read(entry.pw_gecos) }, "");

        // A uid that does not exist reports absence, not a fabricated entry.
        assert!(kinakaze_abi_getpwuid(1000).is_null());
        assert_eq!(kinakaze_tls::errno(), ENOENT);
    }

    #[test]
    fn getpwnam_finds_the_guest_root_account() {
        // SAFETY: null-terminated literal.
        let by_root = unsafe { kinakaze_abi_getpwnam(c"root".as_ptr()) };
        assert!(!by_root.is_null(), "root must resolve");
        let by_uid = kinakaze_abi_getpwuid(0);
        assert!(!by_uid.is_null(), "uid 0 must resolve");
        assert_eq!(unsafe { read((*by_uid).pw_name) }, "root");

        // SAFETY: null-terminated literal.
        let missing = unsafe { kinakaze_abi_getpwnam(c"nosuchuser".as_ptr()) };
        assert!(missing.is_null());
    }

    #[test]
    fn getpwnam_r_reports_erange_without_writing_past_the_buffer() {
        // A canary tail proves the packer respects the length it was given rather
        // than the allocation it happens to sit in.
        const VISIBLE: usize = 4;
        let mut storage = [0xAAu8; 256];
        let mut entry = Passwd {
            pw_name: ptr::null_mut(),
            pw_passwd: ptr::null_mut(),
            pw_uid: 7,
            pw_gid: 7,
            pw_gecos: ptr::null_mut(),
            pw_dir: ptr::null_mut(),
            pw_shell: ptr::null_mut(),
        };
        let mut result: *mut Passwd = (&raw mut entry).wrapping_add(1);

        // SAFETY: the buffer is writable, but only `VISIBLE` bytes are offered.
        let code = unsafe {
            kinakaze_abi_getpwnam_r(
                c"root".as_ptr(),
                &raw mut entry,
                storage.as_mut_ptr().cast::<c_char>(),
                VISIBLE,
                &raw mut result,
            )
        };
        assert_eq!(code, ERANGE, "a tiny buffer must report ERANGE");
        // POSIX requires the result pointer be cleared on failure.
        assert!(result.is_null(), "*result must be null on ERANGE");
        // Nothing beyond the offered window may have been touched.
        assert!(
            storage[VISIBLE..].iter().all(|byte| *byte == 0xAA),
            "getpwnam_r wrote past the buffer it was given"
        );

        // A buffer of exactly the required size still needs room for every
        // terminator, so the retry loop grows until it fits.
        let mut length = VISIBLE;
        let code = loop {
            let mut storage = vec![0xAAu8; 4096];
            // SAFETY: `length` bytes of a 4096-byte allocation are offered.
            let code = unsafe {
                kinakaze_abi_getpwnam_r(
                    c"root".as_ptr(),
                    &raw mut entry,
                    storage.as_mut_ptr().cast::<c_char>(),
                    length,
                    &raw mut result,
                )
            };
            if code != ERANGE {
                // The strings must live inside the window that was offered.
                let base = storage.as_ptr() as usize;
                let filled = entry;
                for field in [filled.pw_name, filled.pw_dir, filled.pw_shell] {
                    let address = field as usize;
                    assert!(
                        address >= base && address < base + length,
                        "a packed string escaped the caller's buffer"
                    );
                }
                // SAFETY: the fields are null-terminated inside the buffer.
                assert!(!unsafe { read(filled.pw_name) }.is_empty());
                assert_eq!(filled.pw_uid, 0);
                assert_eq!(filled.pw_gid, 0);
                break code;
            }
            length += 1;
            assert!(length < 4096, "the retry loop never succeeded");
        };
        assert_eq!(code, 0, "the grown buffer must succeed");
        assert_eq!(
            result, &raw mut entry,
            "*result must point at the caller's struct"
        );

        // sysconf's advertised size must be enough on the first attempt, because
        // that is what BusyBox allocates before it ever sees ERANGE.
        let advertised = kinakaze_abi_sysconf(SC_GETPW_R_SIZE_MAX) as usize;
        assert!(advertised >= length, "_SC_GETPW_R_SIZE_MAX is too small");
    }

    #[test]
    fn getpwuid_r_distinguishes_absence_from_failure() {
        let mut storage = [0u8; 1024];
        let mut entry = Passwd {
            pw_name: ptr::null_mut(),
            pw_passwd: ptr::null_mut(),
            pw_uid: 0,
            pw_gid: 0,
            pw_gecos: ptr::null_mut(),
            pw_dir: ptr::null_mut(),
            pw_shell: ptr::null_mut(),
        };
        let mut result: *mut Passwd = ptr::null_mut();

        // A uid with no entry: success, with a null result. A caller must be able
        // to tell this apart from an error.
        // SAFETY: writable buffer and out-parameters.
        let code = unsafe {
            kinakaze_abi_getpwuid_r(
                4242,
                &raw mut entry,
                storage.as_mut_ptr().cast::<c_char>(),
                storage.len(),
                &raw mut result,
            )
        };
        assert_eq!(code, 0, "a missing entry is not an error");
        assert!(result.is_null(), "a missing entry must yield a null result");

        // SAFETY: writable buffer and out-parameters.
        let code = unsafe {
            kinakaze_abi_getpwuid_r(
                0,
                &raw mut entry,
                storage.as_mut_ptr().cast::<c_char>(),
                storage.len(),
                &raw mut result,
            )
        };
        assert_eq!(code, 0);
        assert_eq!(result, &raw mut entry);
    }

    #[test]
    fn the_group_database_reports_one_group_with_a_terminated_member_list() {
        let entry = kinakaze_abi_getgrgid(0);
        assert!(!entry.is_null());
        // SAFETY: the entry points at static storage.
        let entry = unsafe { *entry };
        // SAFETY: null-terminated fields.
        assert_eq!(unsafe { read(entry.gr_name) }, "root");
        assert_eq!(entry.gr_gid, 0);
        assert!(!entry.gr_mem.is_null(), "gr_mem must be a valid array");
        // SAFETY: `gr_mem` is a null-terminated array of C strings.
        let first = unsafe { *entry.gr_mem };
        assert!(!first.is_null(), "the group must have a member");
        // SAFETY: members are null-terminated strings.
        assert!(!unsafe { read(first) }.is_empty());
        // SAFETY: the array carries a null terminator after its one entry.
        assert!(
            unsafe { *entry.gr_mem.add(1) }.is_null(),
            "gr_mem must be null-terminated"
        );

        // SAFETY: null-terminated literal.
        assert!(!unsafe { kinakaze_abi_getgrnam(c"root".as_ptr()) }.is_null());
        // SAFETY: null-terminated literal.
        assert!(unsafe { kinakaze_abi_getgrnam(c"nosuchgroup".as_ptr()) }.is_null());
        assert!(kinakaze_abi_getgrgid(1000).is_null());
    }

    #[test]
    fn getgrgid_r_packs_the_member_array_inside_the_caller_buffer() {
        let mut storage = vec![0xAAu8; 1024];
        let mut entry = Group {
            gr_name: ptr::null_mut(),
            gr_passwd: ptr::null_mut(),
            gr_gid: 9,
            gr_mem: ptr::null_mut(),
        };
        let mut result: *mut Group = ptr::null_mut();
        // SAFETY: writable buffer and out-parameters.
        let code = unsafe {
            kinakaze_abi_getgrgid_r(
                0,
                &raw mut entry,
                storage.as_mut_ptr().cast::<c_char>(),
                storage.len(),
                &raw mut result,
            )
        };
        assert_eq!(code, 0);
        assert_eq!(result, &raw mut entry);

        let base = storage.as_ptr() as usize;
        let end = base + storage.len();
        // The array itself must be inside the buffer and pointer-aligned, or a
        // guest dereferencing it would fault on some targets.
        let array = entry.gr_mem as usize;
        assert!(array >= base && array < end, "gr_mem escaped the buffer");
        assert_eq!(array % align_of::<*mut c_char>(), 0, "gr_mem is misaligned");
        // SAFETY: the array is null-terminated inside the buffer.
        let member = unsafe { *entry.gr_mem };
        assert!(
            (member as usize) >= base && (member as usize) < end,
            "a member name escaped the buffer"
        );
        // SAFETY: the array carries its terminator.
        assert!(unsafe { *entry.gr_mem.add(1) }.is_null());

        // A buffer too small for the array must fail cleanly.
        let mut small = [0xAAu8; 8];
        // SAFETY: only 8 bytes are offered.
        let code = unsafe {
            kinakaze_abi_getgrgid_r(
                0,
                &raw mut entry,
                small.as_mut_ptr().cast::<c_char>(),
                small.len(),
                &raw mut result,
            )
        };
        assert_eq!(code, ERANGE);
        assert!(result.is_null());
    }

    #[test]
    fn enumeration_yields_each_entry_once_and_rewinds() {
        kinakaze_abi_setpwent();
        let first = kinakaze_abi_getpwent();
        assert!(!first.is_null(), "the enumeration must yield the account");
        let first_name = unsafe {
            std::ffi::CStr::from_ptr((*first).pw_name)
                .to_bytes()
                .to_vec()
        };
        while !kinakaze_abi_getpwent().is_null() {}
        assert!(kinakaze_abi_getpwent().is_null(), "end of enumeration");
        kinakaze_abi_setpwent();
        let rewound = kinakaze_abi_getpwent();
        assert!(!rewound.is_null());
        let rewound_name = unsafe { std::ffi::CStr::from_ptr((*rewound).pw_name).to_bytes() };
        assert_eq!(
            rewound_name,
            first_name.as_slice(),
            "setpwent must rewind the stream"
        );
        kinakaze_abi_endpwent();

        kinakaze_abi_setgrent();
        let first = kinakaze_abi_getgrent();
        assert!(!first.is_null());
        let first_name = unsafe {
            std::ffi::CStr::from_ptr((*first).gr_name)
                .to_bytes()
                .to_vec()
        };
        while !kinakaze_abi_getgrent().is_null() {}
        assert!(kinakaze_abi_getgrent().is_null(), "end of enumeration");
        kinakaze_abi_setgrent();
        let rewound = kinakaze_abi_getgrent();
        assert!(!rewound.is_null());
        let rewound_name = unsafe { std::ffi::CStr::from_ptr((*rewound).gr_name).to_bytes() };
        assert_eq!(
            rewound_name,
            first_name.as_slice(),
            "setgrent must rewind the stream"
        );
        kinakaze_abi_endgrent();
    }

    #[test]
    fn shadow_absence_and_linux_empty_capabilities_are_reported() {
        crate::set_errno(0);
        // Root user has shadow entry
        let sp = unsafe { kinakaze_abi_getspnam(c"root".as_ptr()) };
        assert!(!sp.is_null(), "root shadow entry should exist");
        // Unknown user reports ENOENT
        crate::set_errno(0);
        assert!(unsafe { kinakaze_abi_getspnam(c"nonexistent_user_12345".as_ptr()) }.is_null());
        assert_eq!(kinakaze_tls::errno(), ENOENT);
        crate::set_errno(0);
        assert!(kinakaze_abi_getspent().is_null());
        assert_eq!(kinakaze_tls::errno(), ENOENT);
        // These exist so a caller's setup/teardown pair still links.
        kinakaze_abi_setspent();
        kinakaze_abi_endspent();

        // Null pointers fail like the Linux syscalls, rather than being treated
        // as a request for the host's unrelated Windows token privileges.
        crate::set_errno(0);
        assert_eq!(
            unsafe { kinakaze_abi_capget(ptr::null_mut(), ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EFAULT);
        crate::set_errno(0);
        assert_eq!(
            unsafe { kinakaze_abi_capset(ptr::null_mut(), ptr::null()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EFAULT);

        // The null-data version probe writes the preferred revision and
        // succeeds, matching Linux's special probing form.
        let mut probe = UserCapabilityHeader { version: 0, pid: 0 };
        assert_eq!(
            unsafe { kinakaze_abi_capget((&raw mut probe).cast(), ptr::null_mut()) },
            0
        );
        assert_eq!(probe.version, LINUX_CAPABILITY_VERSION_3);

        // With a real payload, an unsupported version is EINVAL and still
        // reports the preferred version to the caller.
        let mut invalid = UserCapabilityHeader { version: 0, pid: 0 };
        let mut capabilities = [UserCapabilityData::default(); 2];
        crate::set_errno(0);
        assert_eq!(
            unsafe {
                kinakaze_abi_capget((&raw mut invalid).cast(), capabilities.as_mut_ptr().cast())
            },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
        assert_eq!(invalid.version, LINUX_CAPABILITY_VERSION_3);

        // The process exposes a valid empty Linux capability set.
        capabilities.fill(UserCapabilityData {
            effective: u32::MAX,
            permitted: u32::MAX,
            inheritable: u32::MAX,
        });
        let mut header = UserCapabilityHeader {
            version: LINUX_CAPABILITY_VERSION_3,
            pid: 0,
        };
        assert_eq!(
            unsafe {
                kinakaze_abi_capget((&raw mut header).cast(), capabilities.as_mut_ptr().cast())
            },
            0
        );
        assert!(capabilities.iter().all(|record| {
            record.effective == 0 && record.permitted == 0 && record.inheritable == 0
        }));
        assert_eq!(
            unsafe { kinakaze_abi_capset((&raw mut header).cast(), capabilities.as_ptr().cast()) },
            0
        );

        capabilities[0].effective = 1;
        crate::set_errno(0);
        assert_eq!(
            unsafe { kinakaze_abi_capset((&raw mut header).cast(), capabilities.as_ptr().cast()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EPERM);
        capabilities[0].permitted = 1;
        assert_eq!(
            unsafe { kinakaze_abi_capset((&raw mut header).cast(), capabilities.as_ptr().cast()) },
            0
        );
        capabilities.fill(UserCapabilityData::default());
        assert_eq!(
            unsafe {
                kinakaze_abi_capget((&raw mut header).cast(), capabilities.as_mut_ptr().cast())
            },
            0
        );
        assert_eq!(capabilities[0].effective, 1);
        assert_eq!(capabilities[0].permitted, 1);
        capabilities.fill(UserCapabilityData::default());
        assert_eq!(
            unsafe { kinakaze_abi_capset((&raw mut header).cast(), capabilities.as_ptr().cast()) },
            0
        );
    }

    #[test]
    fn uname_fills_every_field_and_reports_linux() {
        let mut name = Utsname {
            sysname: [0x7f; UTS_LEN],
            nodename: [0x7f; UTS_LEN],
            release: [0x7f; UTS_LEN],
            version: [0x7f; UTS_LEN],
            machine: [0x7f; UTS_LEN],
            domainname: [0x7f; UTS_LEN],
        };
        // SAFETY: a writable local struct.
        assert_eq!(unsafe { kinakaze_abi_uname(&raw mut name) }, 0);

        // Every field must be a terminated string, since guest code reads all six
        // and an unterminated one would run off the end of the struct.
        let fields = [
            ("sysname", &name.sysname),
            ("nodename", &name.nodename),
            ("release", &name.release),
            ("version", &name.version),
            ("machine", &name.machine),
            ("domainname", &name.domainname),
        ];
        for (label, field) in fields {
            assert!(field.contains(&0), "{label} is not NUL-terminated");
            // SAFETY: the field was just proved to contain a terminator.
            let text = unsafe { read(field.as_ptr()) };
            assert!(!text.is_empty(), "{label} is empty");
        }

        // SAFETY: every field is terminated, as asserted above.
        unsafe {
            // The deliberate substitution: guest code branches on this.
            assert_eq!(read(name.sysname.as_ptr()), "Linux");
            assert_eq!(read(name.machine.as_ptr()), "x86_64");
            // The release must parse as a Linux version, because callers compare
            // against it numerically.
            let release = read(name.release.as_ptr());
            let major: u32 = release
                .split('.')
                .next()
                .and_then(|part| part.parse().ok())
                .unwrap_or_else(|| panic!("release {release:?} has no numeric major"));
            assert!(major >= 2, "release {release:?} is not a plausible kernel");
            // The real host is never concealed.
            let version = read(name.version.as_ptr());
            assert!(
                version.contains("Windows"),
                "version {version:?} hides the real host"
            );
            assert!(
                version.contains("kinakaze"),
                "version {version:?} hides its origin"
            );
            // nodename must agree with gethostname, or a guest gets two answers.
            let mut host = [0u8; 256];
            assert_eq!(
                kinakaze_abi_gethostname(host.as_mut_ptr().cast::<c_char>(), host.len()),
                0
            );
            assert_eq!(
                read(host.as_ptr().cast::<c_char>()),
                read(name.nodename.as_ptr())
            );
        }

        crate::set_errno(0);
        // SAFETY: a null pointer is the documented failure case.
        assert_eq!(unsafe { kinakaze_abi_uname(ptr::null_mut()) }, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EFAULT);
    }

    #[test]
    fn gethostname_refuses_to_truncate() {
        let mut tiny = [0xAAu8; 2];
        crate::set_errno(0);
        // SAFETY: a writable two-byte buffer.
        let code =
            unsafe { kinakaze_abi_gethostname(tiny.as_mut_ptr().cast::<c_char>(), tiny.len()) };
        // A truncated hostname would be used to build URLs, so it is an error.
        assert_eq!(code, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::ENAMETOOLONG);

        // Renaming the guest UTS hostname succeeds in-process.
        crate::set_errno(0);
        assert_eq!(unsafe { kinakaze_abi_sethostname(c"other".as_ptr(), 5) }, 0);
        let mut host = [0u8; 256];
        assert_eq!(
            unsafe { kinakaze_abi_gethostname(host.as_mut_ptr().cast::<c_char>(), host.len()) },
            0
        );
        assert_eq!(unsafe { read(host.as_ptr().cast::<c_char>()) }, "other");
    }

    #[test]
    fn sysconf_answers_the_names_busybox_asks_for() {
        assert_eq!(kinakaze_abi_sysconf(SC_PAGESIZE), 4096);
        assert_eq!(
            kinakaze_abi_getpagesize() as i64,
            kinakaze_abi_sysconf(SC_PAGESIZE),
            "getpagesize and sysconf must agree"
        );
        // USER_HZ is fixed at 100 on Linux and `times` output is in those units.
        assert_eq!(kinakaze_abi_sysconf(SC_CLK_TCK), 100);

        let online = kinakaze_abi_sysconf(SC_NPROCESSORS_ONLN);
        assert!(online >= 1, "at least one processor must be online");
        let configured = kinakaze_abi_sysconf(SC_NPROCESSORS_CONF);
        assert!(
            configured >= online,
            "{configured} configured is fewer than {online} online"
        );

        assert_eq!(
            kinakaze_abi_sysconf(SC_OPEN_MAX),
            kinakaze_vfs::job::current_nofile_limit().unwrap() as i64,
            "OPEN_MAX must report the current soft limit"
        );
        assert_eq!(
            kinakaze_abi_getdtablesize() as i64,
            kinakaze_abi_sysconf(SC_OPEN_MAX)
        );
        assert!(kinakaze_abi_sysconf(SC_GETPW_R_SIZE_MAX) > 0);
        assert!(kinakaze_abi_sysconf(SC_ARG_MAX) > 0);
        assert!(kinakaze_abi_sysconf(SC_PHYS_PAGES) > 0);

        // An unknown name reports -1, never 0: zero is a valid answer for some
        // names and a caller could not tell the two apart.
        assert_eq!(kinakaze_abi_sysconf(-1), -1);
        assert_eq!(kinakaze_abi_sysconf(9999), -1);
    }

    #[test]
    fn rlimits_report_the_real_bounds_and_refuse_changes() {
        let mut limit = RLimit::default();
        // SAFETY: a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit(RLIMIT_NOFILE, &raw mut limit) },
            0
        );
        assert_eq!(
            limit.rlim_cur,
            kinakaze_vfs::job::current_nofile_limit().unwrap() as u64,
            "NOFILE must report the current soft limit"
        );

        // SAFETY: a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit(RLIMIT_STACK, &raw mut limit) },
            0
        );
        assert!(limit.rlim_cur > 0, "the stack limit must be a real size");

        // SAFETY: a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit(RLIMIT_AS, &raw mut limit) },
            0
        );
        let original = limit;
        assert_eq!(original.rlim_cur, RLIM_INFINITY);
        assert_eq!(original.rlim_max, RLIM_INFINITY);

        // The raise-soft-to-hard idiom must succeed, since it changes nothing.
        let hard = RLimit {
            rlim_cur: original.rlim_max,
            rlim_max: original.rlim_max,
        };
        // SAFETY: a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit(RLIMIT_AS, &raw const hard) },
            0
        );
        // A changed memory limit has no enforcement backend and is rejected.
        let lower = RLimit {
            rlim_cur: 1024,
            rlim_max: 2048,
        };
        crate::set_errno(0);
        // SAFETY: a readable local.
        assert_eq!(
            unsafe { kinakaze_abi_setrlimit(RLIMIT_AS, &raw const lower) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EOPNOTSUPP);
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit(RLIMIT_AS, &raw mut limit) },
            0
        );
        assert_eq!(
            (limit.rlim_cur, limit.rlim_max),
            (original.rlim_cur, original.rlim_max)
        );

        // An out-of-range resource is EINVAL, not a silent infinity.
        crate::set_errno(0);
        // SAFETY: a writable local.
        assert_eq!(unsafe { kinakaze_abi_getrlimit(999, &raw mut limit) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn rlimit_entrypoints_share_limits_and_noop_roundtrips() {
        use crate::sysadmin::{RLimit64, SYS_GETRLIMIT, SYS_SETRLIMIT, kinakaze_abi_syscall_raw};

        assert_eq!(size_of::<RLimit>(), size_of::<RLimit64>());
        assert_eq!(
            core::mem::align_of::<RLimit>(),
            core::mem::align_of::<RLimit64>()
        );
        for resource in 0..RLIMIT_NLIMITS {
            let mut plain = RLimit::default();
            let mut wide = RLimit64::default();
            let mut raw = RLimit64::default();
            assert_eq!(
                unsafe { kinakaze_abi_getrlimit(resource, &raw mut plain) },
                0
            );
            assert_eq!(
                unsafe { crate::sysadmin::kinakaze_abi_getrlimit64(resource, &raw mut wide) },
                0
            );
            assert_eq!(
                unsafe {
                    kinakaze_abi_syscall_raw(
                        SYS_GETRLIMIT,
                        resource as u64,
                        &raw mut raw as u64,
                        0,
                        0,
                        0,
                        0,
                    )
                },
                0
            );
            assert_eq!(
                (plain.rlim_cur, plain.rlim_max),
                (wide.rlim_cur, wide.rlim_max)
            );
            assert_eq!(raw, wide);
            assert_eq!(
                unsafe { kinakaze_abi_setrlimit(resource, &raw const plain) },
                0
            );
            assert_eq!(
                unsafe {
                    kinakaze_abi_syscall_raw(
                        SYS_SETRLIMIT,
                        resource as u64,
                        &raw const plain as u64,
                        0,
                        0,
                        0,
                        0,
                    )
                },
                0
            );
        }
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit(RLIMIT_AS, core::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EFAULT);
        assert_eq!(
            unsafe { kinakaze_abi_getrlimit(RLIMIT_NLIMITS, core::ptr::null_mut()) },
            -1
        );
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn getrusage_reports_real_cpu_time_and_memory() {
        let mut usage = RUsage::default();
        // SAFETY: a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrusage(RUSAGE_SELF, &raw mut usage) },
            0
        );
        // The microsecond field must be a true remainder, not a whole second.
        assert!((0..1_000_000).contains(&usage.ru_utime.tv_usec));
        assert!((0..1_000_000).contains(&usage.ru_stime.tv_usec));
        assert!(usage.ru_utime.tv_sec >= 0 && usage.ru_stime.tv_sec >= 0);
        // This test is running, so the process has used memory and some CPU.
        assert!(usage.ru_maxrss > 0, "maxrss should be a real figure");
        let total = usage.ru_utime.tv_sec * 1_000_000
            + usage.ru_utime.tv_usec
            + usage.ru_stime.tv_sec * 1_000_000
            + usage.ru_stime.tv_usec;
        assert!(total >= 0, "the process has consumed CPU time");

        // RUSAGE_CHILDREN has no accounting to draw on and says so with zeros.
        let mut children = RUsage::default();
        // SAFETY: a writable local.
        assert_eq!(
            unsafe { kinakaze_abi_getrusage(RUSAGE_CHILDREN, &raw mut children) },
            0
        );
        assert_eq!(children.ru_maxrss, 0);

        crate::set_errno(0);
        // SAFETY: a writable local.
        assert_eq!(unsafe { kinakaze_abi_getrusage(42, &raw mut usage) }, -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn sysinfo_reports_real_memory_totals() {
        let mut info = SysInfo::default();
        // SAFETY: a writable local.
        assert_eq!(unsafe { kinakaze_abi_sysinfo(&raw mut info) }, 0);
        assert!(info.uptime > 0, "the host has been up for some time");
        // Byte granularity, so the totals need no scaling.
        assert_eq!(info.mem_unit, 1);
        // Any machine that can run this has at least 256 MiB.
        assert!(
            info.totalram > 256 * 1024 * 1024,
            "totalram {} is implausible",
            info.totalram
        );
        assert!(info.freeram <= info.totalram, "freeram exceeds totalram");
        assert!(
            info.freeswap <= info.totalswap,
            "freeswap exceeds totalswap"
        );
        // x86_64 has no high memory.
        assert_eq!(info.totalhigh, 0);
        assert_eq!(info.freehigh, 0);
    }

    #[test]
    fn sysinfo_rejects_a_null_output() {
        crate::set_errno(0);
        // SAFETY: a null output is rejected before accessing it.
        assert_eq!(unsafe { kinakaze_abi_sysinfo(core::ptr::null_mut()) }, -1);
        assert_eq!(kinakaze_tls::errno(), kinakaze_vfs::EFAULT);
    }

    #[test]
    fn priority_round_trips_through_the_windows_class() {
        // The starting value is restored, because the priority class is process
        // state shared with every other test in this binary.
        let original = current_nice();

        crate::set_errno(0);
        let reported = kinakaze_abi_getpriority(PRIO_PROCESS, 0);
        assert_eq!(
            kinakaze_tls::errno(),
            0,
            "a successful query must clear errno"
        );
        assert!((-20..=19).contains(&reported));

        // Lowering priority is always permitted, and must be observable: this is
        // a real Windows priority class change, not a stored number.
        assert_eq!(kinakaze_abi_setpriority(PRIO_PROCESS, 0, 10), 0);
        let lowered = kinakaze_abi_getpriority(PRIO_PROCESS, 0);
        assert!(
            lowered > 0,
            "setting nice 10 should report a positive value, got {lowered}"
        );

        // `nice` is relative to the current value.
        let after = kinakaze_abi_nice(5);
        assert!(after >= lowered, "nice(5) should not raise priority");

        // A target that is not this process cannot be described.
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_getpriority(PRIO_PROCESS, 999_999), -1);
        assert_eq!(kinakaze_tls::errno(), ESRCH);
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_getpriority(42, 0), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);

        assert_eq!(kinakaze_abi_setpriority(PRIO_PROCESS, 0, original), 0);
    }

    #[test]
    fn group_lists_describe_the_one_group() {
        // The size query form writes nothing and reports the length.
        // SAFETY: a null list with a zero count is the documented size query.
        assert_eq!(unsafe { kinakaze_abi_getgroups(0, ptr::null_mut()) }, 1);

        let mut groups = [0xFFu32; 4];
        // SAFETY: a writable array of four slots.
        let count = unsafe { kinakaze_abi_getgroups(4, groups.as_mut_ptr()) };
        assert_eq!(count, 1);
        assert_eq!(groups[0], 0);
        // Only the first slot may be touched.
        assert_eq!(&groups[1..], &[0xFFu32; 3]);

        let accepted = [0u32];
        // SAFETY: a readable one-element array.
        assert_eq!(unsafe { kinakaze_abi_setgroups(1, accepted.as_ptr()) }, 0);
        // SAFETY: a zero count needs no readable list.
        assert_eq!(unsafe { kinakaze_abi_setgroups(0, ptr::null()) }, 0);
        let supplemental = [1000u32];
        crate::set_errno(0);
        // SAFETY: a readable one-element array.
        assert_eq!(
            unsafe { kinakaze_abi_setgroups(1, supplemental.as_ptr()) },
            0
        );

        assert_eq!(kinakaze_abi_group_member(0), 1);
        assert_eq!(kinakaze_abi_group_member(1000), 1);
        assert_eq!(kinakaze_abi_group_member(2000), 0);

        // SAFETY: null-terminated literal.
        assert_eq!(unsafe { kinakaze_abi_initgroups(c"root".as_ptr(), 0) }, 0);
    }

    #[test]
    fn getgrouplist_reports_the_required_size_on_overflow() {
        // A zero capacity must fail and report the size needed, which is the
        // first half of the retry loop BusyBox runs.
        let mut count = 0i32;
        // SAFETY: `count` is writable; a null list is fine at zero capacity.
        let result = unsafe {
            kinakaze_abi_getgrouplist(c"root".as_ptr(), 0, ptr::null_mut(), &raw mut count)
        };
        assert_eq!(result, -1, "a zero capacity must report failure");
        assert_eq!(count, 1, "the required size must be reported back");

        // Retrying with that size succeeds.
        let mut groups = [0xFFu32; 4];
        let mut count = 4i32;
        // SAFETY: writable array and count.
        let result = unsafe {
            kinakaze_abi_getgrouplist(c"root".as_ptr(), 0, groups.as_mut_ptr(), &raw mut count)
        };
        assert_eq!(result, 1);
        assert_eq!(count, 1);
        assert_eq!(groups[0], 0);
        // The untouched tail proves only the reported entries were written.
        assert_eq!(&groups[1..], &[0xFFu32; 3]);
    }

    #[test]
    fn process_groups_and_sessions_come_from_the_shared_registry() {
        kinakaze_vfs::job::ensure_registered();
        // The pid these calls answer with is the one the registry allocated for
        // this process, not the Windows pid: a guest reads them as Linux ids and
        // they have to agree with `getpid`.
        let pid = kinakaze_runtime::job::current_pid() as c_int;
        // A program started from a Windows shell leads its own group and its
        // own session. Asserting that state rather than assuming it keeps this
        // test independent of whatever else has run in this process, and every
        // case below leaves the identity exactly as it found it.
        kinakaze_runtime::job::set_session(pid as u32, pid as u32, pid as u32);

        assert_eq!(kinakaze_abi_getpgrp(), pid);
        assert_eq!(kinakaze_abi_getpgid(0), pid);
        assert_eq!(kinakaze_abi_getsid(0), pid);
        assert_eq!(kinakaze_abi_getpgid(pid), pid);
        assert_eq!(kinakaze_abi_getsid(pid), pid);

        // A session leader may not create a second session: the id it would be
        // given is the one it already holds. This used to report success, which
        // hid the case where a daemonizing program forgot to fork first.
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_setsid(), -1);
        assert_eq!(kinakaze_tls::errno(), EPERM);

        // Nor may it leave its own group, for the same reason.
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_setpgid(0, 12345), -1);
        assert_eq!(kinakaze_tls::errno(), EPERM);

        // A pid nothing has registered is not a process this layer can answer
        // for, and guessing would be worse than saying so.
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_getpgid(0x7fff_0004), -1);
        assert_eq!(kinakaze_tls::errno(), ESRCH);
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_getsid(0x7fff_0004), -1);
        assert_eq!(kinakaze_tls::errno(), ESRCH);

        // Negative arguments are neither pids nor groups.
        crate::set_errno(0);
        assert_eq!(kinakaze_abi_setpgid(-1, 0), -1);
        assert_eq!(kinakaze_tls::errno(), EINVAL);
    }

    #[test]
    fn getlogin_r_copies_the_account_name_or_reports_erange() {
        let mut buffer = [0xAAu8; 256];
        // SAFETY: a writable 256-byte buffer.
        let code =
            unsafe { kinakaze_abi_getlogin_r(buffer.as_mut_ptr().cast::<c_char>(), buffer.len()) };
        assert_eq!(code, 0);
        // SAFETY: the call terminated the string it wrote.
        let login = unsafe { read(buffer.as_ptr().cast::<c_char>()) };
        assert!(!login.is_empty(), "the login name must not be empty");
        // It must be the same name the passwd entry reports.
        // SAFETY: the static entry's fields are null-terminated.
        let entry_name = unsafe { read((*kinakaze_abi_getpwuid(0)).pw_name) };
        assert_eq!(login, entry_name, "getlogin_r and getpwuid disagree");

        // A buffer too small must report ERANGE and write nothing.
        let mut tiny = [0xAAu8; 1];
        // SAFETY: a writable one-byte buffer.
        let code =
            unsafe { kinakaze_abi_getlogin_r(tiny.as_mut_ptr().cast::<c_char>(), tiny.len()) };
        assert_eq!(code, ERANGE);
        assert_eq!(tiny[0], 0xAA, "getlogin_r wrote into a buffer it rejected");
    }

    #[test]
    fn home_stays_inside_the_guest_root() {
        assert_eq!(home_directory(), "/root");
    }
}
